//! `agentrec doctor`: one-shot diagnosis of the whole recording chain (D41 /
//! AC-Y++). A SETUP command alongside `init`/`uninstall`, not a query verb —
//! it never mutates anything, only reads state and reports. Reuses the same
//! liveness (`daemon::daemon_is_running` — a non-blocking flock probe, D2),
//! hook-marker (`initcmd::event_has_marker`), and state (`state::read_state`)
//! logic as `status`/`record`/`init` rather than re-deriving it.

use crate::state::read_state;
use crate::{agentrec_dir, log_path, objects_dir, signal_path};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A transcript younger than this counts as "recent agent activity" for the
/// signal-freshness check; a signal inbox younger than this counts as "fresh".
/// 15 minutes comfortably covers a live coding session's think-time between
/// turns while still catching a hooks-disconnected repo quickly.
const RECENT_WINDOW: Duration = Duration::from_secs(15 * 60);

#[derive(Serialize, PartialEq, Eq, Clone, Copy, Debug)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Fail,
    #[serde(rename = "n/a")]
    Na,
}

#[derive(Serialize)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub remedy: Option<String>,
}

impl Check {
    fn pass(name: &str) -> Self {
        Check {
            name: name.to_string(),
            status: CheckStatus::Pass,
            remedy: None,
        }
    }
    fn na(name: &str) -> Self {
        Check {
            name: name.to_string(),
            status: CheckStatus::Na,
            remedy: None,
        }
    }
    fn fail(name: &str, remedy: impl Into<String>) -> Self {
        Check {
            name: name.to_string(),
            status: CheckStatus::Fail,
            remedy: Some(remedy.into()),
        }
    }
}

#[derive(Serialize)]
pub struct Report {
    pub checks: Vec<Check>,
    pub ok: bool,
}

/// Entry point for the CLI: run every check, print (text or `--json`), and
/// return whether all checks passed so `main` can set the process exit code.
pub fn run(root: &Path, json: bool) -> Result<bool, String> {
    let report = diagnose(root);
    if json {
        let text = serde_json::to_string(&report).map_err(|e| e.to_string())?;
        println!("{text}");
    } else {
        print_text(&report);
    }
    Ok(report.ok)
}

fn print_text(report: &Report) {
    for check in &report.checks {
        let label = match check.status {
            CheckStatus::Pass => "pass",
            CheckStatus::Fail => "fail",
            CheckStatus::Na => "n/a",
        };
        println!("{:<24} {label}", check.name);
        if let Some(remedy) = &check.remedy {
            println!("  -> {remedy}");
        }
    }
}

/// Runs every check against `root`. `pub(crate)` (not `pub`) — exposed for
/// direct unit/integration assertions on the typed result, without shelling
/// out and re-parsing text/JSON.
///
/// D-PD1: `initialized` runs FIRST and gates everything else. Every other
/// check reads daemon/hook/signal/store state that simply doesn't exist yet
/// in an uninitialized repo (no `.agentrec/`) — running them anyway produces
/// fabricated failures ("store permissions too open", "agent active but no
/// signals arriving") that mislead a user who hasn't run `init`. If
/// `initialized` fails, every other check short-circuits to `n/a` instead of
/// executing its real logic.
pub(crate) fn diagnose(root: &Path) -> Report {
    let initialized = check_initialized(root);
    if initialized.status == CheckStatus::Fail {
        let mut checks = vec![initialized];
        checks.extend(
            [
                "daemon liveness",
                "hook presence",
                "signal freshness",
                "store health",
                "store permissions",
                "inotify headroom",
            ]
            .iter()
            .map(|name| Check::na(name)),
        );
        return Report { checks, ok: false };
    }

    let checks = vec![
        initialized,
        check_daemon(root),
        check_hooks(root),
        check_signal_freshness(root),
        check_degraded(root),
        check_permissions(root),
        check_inotify(root),
    ];
    let ok = checks.iter().all(|c| c.status != CheckStatus::Fail);
    Report { checks, ok }
}

// ---- initialized (D-PD1) ----------------------------------------------------

/// FIRST check: is this even an `agentrec init`-ed repo? Everything else
/// below reads daemon/hook/signal/store state that presumes `.agentrec/`
/// exists, so this gates all other checks (see `diagnose`).
fn check_initialized(root: &Path) -> Check {
    if agentrec_dir(root).is_dir() {
        Check::pass("initialized")
    } else {
        Check::fail("initialized", "not initialized — run `agentrec init`")
    }
}

// ---- daemon liveness --------------------------------------------------------

fn check_daemon(root: &Path) -> Check {
    // D2: a pid-liveness check false-passes when an unrelated process
    // recycles a dead daemon's pid (state.json still names it, `kill(pid,0)`
    // succeeds against the new occupant). The flock probe asks the only
    // question that actually matters — does something hold the recorder
    // lock right now — so pid recycling can't fool it.
    if crate::daemon::daemon_is_running(root) {
        Check::pass("daemon liveness")
    } else {
        Check::fail(
            "daemon liveness",
            "recorder not running — run `agentrec record` (or install the service via `agentrec init`)",
        )
    }
}

// ---- hook presence/validity -------------------------------------------------

fn check_hooks(root: &Path) -> Check {
    const REMEDY: &str = "hooks missing/mangled — run `agentrec init`";
    let path = root.join(".claude").join("settings.local.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Check::fail("hook presence", REMEDY);
    };
    let Ok(settings) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Check::fail("hook presence", REMEDY);
    };
    let has_both = crate::initcmd::event_has_marker(&settings, "UserPromptSubmit")
        && crate::initcmd::event_has_marker(&settings, "Stop");
    if has_both {
        Check::pass("hook presence")
    } else {
        Check::fail("hook presence", REMEDY)
    }
}

// ---- signal freshness vs transcript activity --------------------------------

/// Root of Claude Code's transcript tree. Production reads `$HOME/.claude/
/// projects` (Claude Code's real layout: `<projects>/<encoded-cwd>/<session>.
/// jsonl`); `AGENTREC_CLAUDE_PROJECTS_DIR` overrides it so this check is
/// hermetically testable without touching the real developer's `$HOME`. See
/// DEVIATIONS in the delivery receipt: this check does not reconstruct
/// Claude Code's cwd-encoding scheme to find *this repo's* transcript
/// directory specifically — it takes the newest `*.jsonl` mtime anywhere
/// under the projects root as a proxy for "agent activity is happening
/// somewhere". That is deliberately conservative (never a false pass) at the
/// cost of being imprecise across multiple concurrently-active repos on the
/// same machine.
fn claude_projects_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("AGENTREC_CLAUDE_PROJECTS_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".claude").join("projects"))
}

fn check_signal_freshness(root: &Path) -> Check {
    const REMEDY: &str =
        "agent active but no signals arriving — hooks may be disconnected (`agentrec doctor` after `agentrec init`)";
    let Some(dir) = claude_projects_dir() else {
        return Check::pass("signal freshness"); // no HOME, no override: nothing to check
    };
    let Some(transcript_mtime) = newest_jsonl_mtime(&dir) else {
        return Check::pass("signal freshness"); // no transcript evidence at all
    };
    if !within_window(transcript_mtime, RECENT_WINDOW) {
        return Check::pass("signal freshness"); // transcript activity is old news
    }

    // Transcript activity is recent — the signal inbox must be too.
    let signal_mtime = std::fs::metadata(signal_path(root))
        .ok()
        .and_then(|m| m.modified().ok());
    let signal_fresh = signal_mtime
        .map(|t| within_window(t, RECENT_WINDOW))
        .unwrap_or(false); // missing signal.jsonl while the agent is active is exactly the failure mode
    if signal_fresh {
        Check::pass("signal freshness")
    } else {
        Check::fail("signal freshness", REMEDY)
    }
}

fn within_window(t: SystemTime, window: Duration) -> bool {
    SystemTime::now()
        .duration_since(t)
        .map(|age| age < window)
        .unwrap_or(true) // `t` is in the future (clock skew) — treat as recent, never a false negative
}

/// Newest mtime of any `*.jsonl` file under `dir`, bounded to a shallow
/// depth and a visit cap so a huge real `~/.claude/projects` can't turn this
/// into a heavy scan (AC-Y++1: no heavy scan).
fn newest_jsonl_mtime(dir: &Path) -> Option<SystemTime> {
    const MAX_DEPTH: u32 = 3;
    const MAX_VISITS: u32 = 2_000;

    let mut newest: Option<SystemTime> = None;
    let mut stack = vec![(dir.to_path_buf(), 0u32)];
    let mut visited = 0u32;
    while let Some((current, depth)) = stack.pop() {
        if depth > MAX_DEPTH || visited > MAX_VISITS {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_VISITS {
                break;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth + 1));
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
                    if newest.map(|n| mtime > n).unwrap_or(true) {
                        newest = Some(mtime);
                    }
                }
            }
        }
    }
    newest
}

// ---- DEGRADED store ----------------------------------------------------------

fn check_degraded(root: &Path) -> Check {
    let state = read_state(root);
    // D35 gap closure: a prompt-blob write failure is just as much a "store
    // DEGRADED" condition as a file-snapshot one — both mean a write the
    // daemon attempted silently didn't land. Checked together so `doctor`
    // can't false-PASS store health while either counter is nonzero.
    if state.snapshot_failures > 0 || state.prompt_put_failures > 0 {
        let mut reasons = Vec::new();
        if state.snapshot_failures > 0 {
            reasons.push(format!(
                "{} snapshot writes failed",
                state.snapshot_failures
            ));
        }
        if state.prompt_put_failures > 0 {
            reasons.push(format!(
                "{} prompt writes failed",
                state.prompt_put_failures
            ));
        }
        Check::fail(
            "store health",
            format!(
                "store DEGRADED — {}; investigate disk/permissions, then `agentrec status --ack-degraded`",
                reasons.join(", ")
            ),
        )
    } else {
        Check::pass("store health")
    }
}

// ---- store permissions (unix only) ------------------------------------------

#[cfg(unix)]
fn check_permissions(root: &Path) -> Check {
    use std::os::unix::fs::PermissionsExt;
    const REMEDY: &str =
        "store permissions too open — reinitialize or chmod 0700/.agentrec 0600 files";

    let mode_of = |p: &Path| -> Option<u32> {
        std::fs::metadata(p)
            .ok()
            .map(|m| m.permissions().mode() & 0o777)
    };

    let dir_ok = mode_of(&agentrec_dir(root)) == Some(0o700);
    // A not-yet-created log/blob has nothing to flag — only check what exists.
    let log_ok = mode_of(&log_path(root)).map(|m| m == 0o600).unwrap_or(true);
    let blob_ok = first_blob(&objects_dir(root))
        .and_then(|p| mode_of(&p))
        .map(|m| m == 0o600)
        .unwrap_or(true);

    if dir_ok && log_ok && blob_ok {
        Check::pass("store permissions")
    } else {
        Check::fail("store permissions", REMEDY)
    }
}

#[cfg(not(unix))]
fn check_permissions(_root: &Path) -> Check {
    Check::na("store permissions")
}

/// First regular file found under the sha256 fan-out dirs (`objects/<xx>/*`),
/// or `None` if the store has no blobs yet.
#[cfg(unix)]
fn first_blob(objects_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(objects_dir).ok()?;
    for fanout in entries.flatten() {
        let fanout_path = fanout.path();
        if !fanout_path.is_dir() {
            continue;
        }
        if let Ok(files) = std::fs::read_dir(&fanout_path) {
            for f in files.flatten() {
                let p = f.path();
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

// ---- inotify headroom (Linux only) ------------------------------------------

/// LADDERED: this Linux-only path cannot be exercised on macOS dev machines.
/// The code path is implemented and compiles under `#[cfg(target_os =
/// "linux")]`; its assertion is skipped here and belongs on
/// VERIFY-LEDGER.md for a real Linux run.
#[cfg(target_os = "linux")]
fn check_inotify(root: &Path) -> Check {
    let max_watches: u64 = std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(8192);
    // Pass `max_watches` so the walk can stop as soon as it proves the tree
    // exceeds the limit — a blind cap could false-PASS when the limit was
    // raised above it. `estimate` is a lower bound (may stop early), so the
    // message says "at least".
    let estimate = estimate_watch_count(root, max_watches);
    if estimate > max_watches {
        Check::fail(
            "inotify headroom",
            format!(
                "inotify headroom too low (at least {estimate} dirs to watch vs \
                 {max_watches} max_user_watches) — raise it: sudo sysctl \
                 fs.inotify.max_user_watches=524288"
            ),
        )
    } else {
        Check::pass("inotify headroom")
    }
}

/// Estimate of directories `notify`'s recursive watcher registers — one
/// inotify watch per directory. The daemon watches the whole root
/// (`cli/src/daemon.rs`, `RecursiveMode::Recursive`) and filters *events*
/// afterward via `IgnoreSet`, not which directories are watched — so notify
/// arms a watch on EVERY directory under `root`, including gitignored trees
/// (`target`, `node_modules`) and `.git`. This estimate must count all of
/// them.
///
/// The previous implementation used a gitignore-aware `ignore::Walk`, which
/// silently dropped every gitignored directory (on this repo, `target/` alone
/// is ~600 dirs). It therefore reported far fewer dirs than the kernel
/// actually consumed, so `doctor` could PASS while the daemon hit
/// `max_user_watches` and failed to arm its watch at startup.
///
/// This mirrors notify 6's `INotifyWatcher::add_watch` exactly: the same
/// `walkdir::WalkDir` with `follow_links(true)`, watching every entry whose
/// metadata is a directory. Following symlinks matters — a pnpm `node_modules`
/// is almost entirely directory symlinks, and notify arms a watch on each
/// followed target; a non-following walk would miss them all and under-report
/// by orders of magnitude. `walkdir` detects symlink loops and yields an error
/// for the offending entry (skipped here), so cycles cannot hang the walk.
/// Symlink aliasing may over-count (two links to one dir counted twice while
/// the kernel dedups by inode) — the safe direction for a headroom check
/// (over-count risks a false FAIL, never a false PASS).
///
/// Returns as soon as the count exceeds `stop_above` (the caller's
/// `max_user_watches`): the fail verdict is already decided, so the rest of a
/// huge tree needn't be walked. Unlike a blind fixed cap, this does not
/// false-PASS for any limit up to `CEILING` — the walk runs until it either
/// exceeds the limit or exhausts the tree. The returned value is a lower bound
/// (walk may stop early); callers phrase it as "at least N".
///
/// `CEILING` (2M, far above any realistic `max_user_watches`) bounds the walk
/// so an effectively-unlimited limit can't scan forever. The one residual
/// blind spot: a limit AND a true dir count both above `CEILING` — the walk
/// caps at `CEILING` and could report PASS. That needs a >2M-directory tree
/// and a >2M sysctl simultaneously, neither of which occurs in practice.
///
/// Not `#[cfg(target_os = "linux")]`-only: the estimate is pure filesystem
/// walking and is unit-tested on every platform (`test` cfg below), even
/// though its sole caller `check_inotify` is Linux-only.
#[cfg(any(target_os = "linux", test))]
fn estimate_watch_count(root: &Path, stop_above: u64) -> u64 {
    // Hard ceiling so an "unlimited" max_user_watches can't make this walk an
    // enormous tree forever. 2M is orders of magnitude past any real setting.
    const CEILING: u64 = 2_000_000;
    let limit = stop_above.saturating_add(1).min(CEILING);
    let mut count: u64 = 0;
    // Same traversal notify uses to arm inotify watches (notify-6 inotify.rs).
    for entry in walkdir::WalkDir::new(root).follow_links(true) {
        // A walk error (permission denied, symlink loop) means notify could
        // not watch that entry either — skip it, don't abort the estimate.
        let Ok(entry) = entry else { continue };
        // notify's `filter_dir`: metadata (follows symlinks) must be a dir.
        if entry.metadata().map(|m| m.is_dir()).unwrap_or(false) {
            count += 1;
            if count >= limit {
                break;
            }
        }
    }
    count
}

#[cfg(not(target_os = "linux"))]
fn check_inotify(_root: &Path) -> Check {
    // Reported as `n/a` in the status field; text mode prints
    // "inotify headroom  n/a" — Linux-only concern (AC-Y++2).
    Check::na("inotify headroom")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_root_fails_daemon_and_hooks_but_not_signal_or_degraded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();

        let report = diagnose(root);
        assert!(!report.ok);
        let by_name = |name: &str| {
            report
                .checks
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| panic!("missing check {name}"))
        };
        assert_eq!(by_name("daemon liveness").status, CheckStatus::Fail);
        assert_eq!(by_name("hook presence").status, CheckStatus::Fail);
        assert_eq!(by_name("store health").status, CheckStatus::Pass);
        // D-PD5: the check was renamed from "store degraded" — assert the
        // old name is fully gone, not just that the new one exists.
        assert!(
            report.checks.iter().all(|c| c.name != "store degraded"),
            "old check name 'store degraded' must not appear: {:?}",
            report.checks.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
    }

    // D-PD5: the doctor check name is "store health", not "store degraded" —
    // covers both the pass and fail paths of `check_degraded` directly.
    #[test]
    fn check_degraded_uses_store_health_as_its_name() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();

        let healthy = check_degraded(root);
        assert_eq!(healthy.name, "store health");

        std::fs::write(
            crate::state_path(root),
            r#"{"pid":0,"signal_offset":0,"snapshot_failures":1,"io_failed":[]}"#,
        )
        .unwrap();
        let degraded = check_degraded(root);
        assert_eq!(degraded.name, "store health");
    }

    #[test]
    fn degraded_state_fails_store_check_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        std::fs::write(
            crate::state_path(root),
            r#"{"pid":0,"signal_offset":0,"snapshot_failures":2,"io_failed":["a.rs"]}"#,
        )
        .unwrap();

        let check = check_degraded(root);
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.remedy.unwrap().contains("2 snapshot"));
    }

    // D35 gap closure: a prompt-put failure must ALSO fail "store health" —
    // doctor must not false-PASS while state.json's prompt_put_failures is
    // nonzero just because snapshot_failures happens to be 0.
    #[test]
    fn prompt_put_failure_alone_fails_store_check() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        std::fs::write(
            crate::state_path(root),
            r#"{"pid":0,"signal_offset":0,"snapshot_failures":0,"io_failed":[],"prompt_put_failures":3}"#,
        )
        .unwrap();

        let check = check_degraded(root);
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.remedy.unwrap().contains("3 prompt"));
    }

    // D2/D6: `check_daemon` used to trust `state.pid` liveness alone, which
    // false-passes when an unrelated live process recycles a dead daemon's
    // pid. It's now a non-blocking flock probe on `.agentrec/daemon.lock` —
    // a live pid sitting in state.json with no lock actually held must still
    // fail. Calls `check_daemon` directly (not `diagnose`) so this doesn't
    // also depend on `check_signal_freshness`'s real-`$HOME` transcript scan.
    #[test]
    fn check_daemon_fails_on_recycled_pid_with_no_lock_held() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        std::fs::write(
            crate::state_path(root),
            format!(
                r#"{{"pid":{},"signal_offset":0,"snapshot_failures":0,"io_failed":[]}}"#,
                std::process::id() // this test process is definitely alive
            ),
        )
        .unwrap();

        let check = check_daemon(root);
        assert_eq!(check.status, CheckStatus::Fail);
    }

    #[test]
    fn signal_freshness_passes_when_no_transcript_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // AGENTREC_CLAUDE_PROJECTS_DIR unset in this process and pointing at
        // an empty dir either way yields no evidence -> pass.
        std::env::set_var(
            "AGENTREC_CLAUDE_PROJECTS_DIR",
            tmp.path().join("does-not-exist"),
        );
        let check = check_signal_freshness(root);
        std::env::remove_var("AGENTREC_CLAUDE_PROJECTS_DIR");
        assert_eq!(check.status, CheckStatus::Pass);
    }

    // Regression: `estimate_watch_count` used a gitignore-aware `ignore::Walk`
    // that dropped every gitignored directory, so it undercounted the inotify
    // watches the daemon actually arms — notify watches the whole root
    // recursively, gitignored trees included. doctor could report PASS while
    // the daemon blew past `max_user_watches` and failed to arm at startup.
    // Runs on every platform because the estimate is now cross-platform
    // (`cfg(test)`), even though its production caller is Linux-only.
    //
    // A real `git init` is required so the `.gitignore` is honored by a
    // gitignore-aware walker (the `ignore` crate applies ignore rules only
    // inside a git repo by default) — otherwise the old code would count
    // everything too and the regression would be invisible. The count delta
    // over a whole nested gitignored subtree isolates the fix from `.git` and
    // other noise (identical in both counts).
    #[test]
    fn estimate_watch_count_includes_gitignored_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let initialized = std::process::Command::new("git")
            .arg("init")
            .arg(root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(initialized, "git must be available to run this test");

        std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        // A nested gitignored subtree: 3 directories the old walk skipped
        // wholesale but notify still watches at every level.
        std::fs::create_dir_all(root.join("ignored/deep/deeper")).unwrap();

        // `u64::MAX` disables the early-stop so the full (small) tree is walked.
        let with_subtree = estimate_watch_count(root, u64::MAX);
        std::fs::remove_dir_all(root.join("ignored")).unwrap();
        let without_subtree = estimate_watch_count(root, u64::MAX);

        // Every level of the gitignored subtree consumes an inotify watch.
        // Old gitignore-aware code: delta 0 (whole subtree skipped) -> fails.
        assert_eq!(
            with_subtree,
            without_subtree + 3,
            "all 3 levels of the gitignored subtree must be counted: \
             with={with_subtree} without={without_subtree}"
        );
    }

    // The walk must stop as soon as it exceeds the caller's limit and report a
    // value > the limit — so `check_inotify` fails a tree that overflows even a
    // raised `max_user_watches`, rather than being blindly capped below it (the
    // false-PASS the fixed-cap version risked).
    #[test]
    fn estimate_watch_count_stops_above_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // root + 3 children = 4 dirs, comfortably over a limit of 1.
        std::fs::create_dir(root.join("a")).unwrap();
        std::fs::create_dir(root.join("b")).unwrap();
        std::fs::create_dir(root.join("c")).unwrap();

        let est = estimate_watch_count(root, 1);
        assert!(
            est > 1,
            "must return a value exceeding the limit to trigger the fail verdict, got {est}"
        );
        // Lower bound only: early-stop means it need not equal the true count (4).
        assert!(
            est <= 4,
            "cannot exceed the true directory count, got {est}"
        );
    }

    // notify walks with `follow_links(true)`, so it arms watches on the targets
    // of directory symlinks (a pnpm `node_modules` is almost all such links).
    // The estimate must follow them too, or it under-reports the kernel's real
    // watch consumption. Fails on any non-following walk (delta 0).
    #[cfg(unix)]
    #[test]
    fn estimate_watch_count_follows_directory_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("real/sub")).unwrap();
        // root, real, real/sub = 3
        let before = estimate_watch_count(root, u64::MAX);
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        let after = estimate_watch_count(root, u64::MAX);
        // Following `link` adds the link dir itself and its `sub` child (+2).
        assert_eq!(
            after,
            before + 2,
            "symlinked dir tree must be followed like notify does: \
             before={before} after={after}"
        );
    }
}
