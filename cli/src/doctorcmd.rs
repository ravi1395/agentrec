//! `agentrec doctor`: one-shot diagnosis of the whole recording chain (D41 /
//! AC-Y++). A SETUP command alongside `init`/`uninstall`, not a query verb —
//! it never mutates anything, only reads state and reports. Reuses the same
//! liveness (`daemon::daemon_is_running` — a non-blocking flock probe, D2),
//! hook-marker (`initcmd::event_has_marker`), and state (`state::read_state`)
//! logic as `status`/`record`/`init` rather than re-deriving it.

use crate::state::read_state;
use crate::{agentrec_dir, log_path, objects_dir, signal_path, state_path};
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
    /// A non-blocking finding: rendered as `pass` (never contributes to
    /// `ok`/exit code) but still carries a printed note, unlike a plain
    /// `pass()`. Used for conditions worth surfacing but not worth breaking
    /// `doctor`'s all-pass exit-0 deploy gate over (Phase 2, honesty-fixes
    /// round: a corrupt `state.json` field is exactly this shape — real, but
    /// self-healing and non-fatal).
    fn advisory(name: &str, note: impl Into<String>) -> Self {
        Check {
            name: name.to_string(),
            status: CheckStatus::Pass,
            remedy: Some(note.into()),
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
                "state parse",
                "store permissions",
                "inotify headroom",
                "orphaned services",
                "codex hooks",
                "codex hook flags",
                "mcp registration",
                "mcp server",
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
        check_state_parse(root),
        check_permissions(root),
        check_inotify(root),
        check_orphan_services(),
        check_codex_hooks(root),
        check_codex_hook_flags(root),
        check_mcp_registration(root),
        check_mcp_server(root),
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
    let Ok(text) = agentrec_core::fsguard::read_regular_to_string(&path) else {
        return Check::fail("hook presence", REMEDY);
    };
    let Ok(settings) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Check::fail("hook presence", REMEDY);
    };
    // Only the BRACKETING pair is required. `init` also installs
    // `PostToolUse[Bash]` for attest capture, but requiring it here would fail
    // `doctor` on every repo initialized before that hook existed — and turn
    // recording, which is what this check is about, does not depend on it.
    let has_both = crate::initcmd::CLAUDE_BRACKET_EVENTS
        .iter()
        .all(|event| crate::initcmd::event_has_marker(&settings, event));
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
    if state.snapshot_failures > 0 || state.prompt_put_failures > 0 || state.non_utf8_path_skips > 0
    {
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
        if state.non_utf8_path_skips > 0 {
            reasons.push(format!(
                "{} non-UTF8 paths skipped",
                state.non_utf8_path_skips
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

// ---- state.json parse health (Phase 2, honesty-fixes round) ----------------

/// Reports a nonzero `state_parse_failures` (a `state.json` field that
/// failed to parse and fell back to its default — see `state::read_state`).
/// Deliberately ADVISORY ONLY, always rendered as `pass`: the condition is
/// real but self-healing (the next `write_state` call anywhere serializes
/// the in-memory default back to disk, curing it) and `doctor` all-pass
/// exit 0 is this repo's production deploy gate — a Fail here would make a
/// transient, already-recovering parse hiccup block deploys.
fn check_state_parse(root: &Path) -> Check {
    let state = read_state(root);
    if state.state_parse_failures > 0 {
        Check::advisory(
            "state parse",
            format!(
                "{} field(s) in {} fell back to defaults (last: {}) — advisory only, \
                 self-heals on the next write; investigate if this recurs",
                state.state_parse_failures,
                state_path(root).display(),
                state.last_bad_field.as_deref().unwrap_or("unknown"),
            ),
        )
    } else {
        Check::pass("state parse")
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

// ---- orphaned service units (D46) -------------------------------------------

/// Reports installed service units whose recorded `--root` is not present on
/// disk. `init` installs a user-scoped unit with `RunAtLoad` + `KeepAlive`;
/// when the root is later deleted — the normal fate of a temp dir, and of any
/// crashed or abandoned test run — nothing reaps the unit, so they accumulate
/// silently (measured on the author's machine 2026-07-31: 40 of 41 installed
/// units pointed at deleted roots).
///
/// ADVISORY ONLY, always rendered as `pass`, for a structural reason and not
/// as a softening: the unit directory is USER-GLOBAL, so a `Fail` here would
/// make one stale unit from some unrelated scratch repo break `doctor`'s
/// all-pass exit-0 deploy gate in every other repo on the machine. The
/// condition also isn't a property of the repo being diagnosed at all.
///
/// The note reports what was observed — a root that is not present — and
/// deliberately does NOT assert the unit is abandoned: an unmounted external
/// volume or a detached network mount reads exactly the same from here.
/// Removal is left to the user (see D46 on why `service prune` isn't built);
/// the exact command pair is printed instead.
fn check_orphan_services() -> Check {
    const NAME: &str = "orphaned services";
    let Ok(dir) = crate::service::service_dir() else {
        // No HOME: nothing locatable to scan, so nothing to report.
        return Check::pass(NAME);
    };
    let units = crate::service::scan_units(&dir);
    let vanished: Vec<_> = units
        .iter()
        .filter_map(|u| match &u.state {
            crate::service::UnitState::VanishedRoot(root) => Some((u, root)),
            _ => None,
        })
        .collect();
    // A unit whose ROOT is live but whose EXEC has vanished is a DIFFERENT
    // and separately actionable failure: it never reaches agentrec code at
    // all — launchd fails at spawn (status 78) — so unlike a vanished root it
    // cannot be noticed by anything the daemon does. Reported on its own line
    // rather than merged into the count above, because the remedy differs: a
    // vanished root usually wants the unit removed, a vanished exec usually
    // wants `agentrec init` re-run to re-record the current binary.
    let missing_exec: Vec<_> = units
        .iter()
        .filter(|u| matches!(u.state, crate::service::UnitState::Live(_)))
        .filter_map(|u| u.missing_exec.as_ref().map(|exec| (u, exec)))
        .collect();

    let mut notes: Vec<String> = Vec::new();
    if let Some((first, first_root)) = vanished.first() {
        let plural = if vanished.len() == 1 { "" } else { "s" };
        notes.push(format!(
            "{} installed service unit{plural} record a --root that is not present \
             (e.g. {} -> {}) — usually a leftover `agentrec init` in a directory since \
             deleted, though an unmounted volume looks the same from here. If the root \
             is genuinely gone, remove each with: {}",
            vanished.len(),
            first.label,
            first_root.display(),
            crate::service::manual_remove_command(&first.label, &first.path),
        ));
    }
    if let Some((first, first_exec)) = missing_exec.first() {
        let plural = if missing_exec.len() == 1 { "" } else { "s" };
        notes.push(format!(
            "{} installed service unit{plural} record an agentrec binary that is not \
             present (e.g. {} -> {}) — the recorded exec is a snapshot taken at `init` \
             time and is never re-resolved, so moving or upgrading the binary strands \
             it. These fail at spawn and will not record anything at next login; \
             re-run `agentrec init` in that repo to re-record the current binary.",
            missing_exec.len(),
            first.label,
            first_exec.display(),
        ));
    }
    if notes.is_empty() {
        return Check::pass(NAME);
    }
    Check::advisory(NAME, notes.join(" "))
}

// ---- Codex hooks (C3) --------------------------------------------------------

/// Shape + "recent signals" validation for agentrec's own Codex hook
/// entries. `n/a` when Codex integration was never opted into (`agentrec
/// init --codex`) — this is an opt-in feature, so a repo that never asked
/// for it must not report a fabricated failure. Once installed:
/// - malformed/missing entries -> `Fail` (mirrors Claude's `check_hooks`);
/// - shape OK but zero `tool:"codex"` lines ever recorded in `signal.jsonl`
///   -> ADVISORY pass. This is deliberately non-blocking, unlike Claude's
///   signal-freshness check: that check is gated on independent evidence of
///   RECENT transcript activity (so it only fires when a signal is actually
///   overdue); this repo has no equivalent external oracle for Codex
///   without scanning `~/.codex/sessions` (out of this round's scope — see
///   docs/verify/codex-spike.md's "what was NOT probed"), so a freshly
///   `init --codex`'d repo that hasn't run a Codex turn yet must not be
///   told it's broken. The note still carries real information: it is
///   `doctor`'s only behavioral signal for the spike's confirmed
///   silent-skip-on-untrusted-hook failure mode, since trust state itself
///   is not inspectable (no documented location — spike, "Trust flow").
fn check_codex_hooks(root: &Path) -> Check {
    const NAME: &str = "codex hooks";
    if !crate::initcmd::codex_hooks_installed(root) {
        return Check::na(NAME);
    }
    if let Err(reason) = crate::initcmd::codex_hooks_shape(root) {
        return Check::fail(
            NAME,
            format!("Codex hook entries missing/mangled ({reason}) — run `agentrec init --codex`"),
        );
    }
    if codex_signal_ever_seen(root) {
        Check::pass(NAME)
    } else {
        Check::advisory(
            NAME,
            "Codex hooks are installed but no tool:\"codex\" line has ever appeared in \
             signal.jsonl — if you've used Codex in this repo, the hooks are likely still \
             untrusted: inside Codex run `/hooks` and trust them (`codex exec` gives no \
             warning at all when hooks are silently skipped — confirmed live, \
             docs/verify/codex-spike.md). Support verified live against codex-cli 0.146.0.",
        )
    }
}

fn codex_signal_ever_seen(root: &Path) -> bool {
    let Ok(text) = agentrec_core::fsguard::read_regular_to_string(&signal_path(root)) else {
        return false;
    };
    text.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| v.get("tool").and_then(|t| t.as_str()).map(String::from))
            .is_some_and(|tool| tool == "codex")
    })
}

/// Reads repo-local `.codex/config.toml` only — `$CODEX_HOME`'s user-global
/// layer (and any managed/MDM `requirements.toml` layer) is a stated gap,
/// not built (spike, "what was NOT probed": "Managed/enterprise hooks
/// (`requirements.toml`, `allow_managed_hooks_only`) ... no live evidence").
///
/// Two conditions, both confirmed by this task round to silently disable
/// hooks without any other visible symptom, so both get LOUD `Fail`
/// severity — same as an absent-trust condition would if it were directly
/// inspectable (it is not; see `check_codex_hooks`):
///
/// - `[features] hooks = false` — **measured live this round**, same
///   pinned `codex-cli 0.146.0`: `codex features list` reports
///   `hooks  stable  true` (default-on). Checked as `[features].hooks`,
///   the canonical location (`codex --help`: `--enable <FEATURE>` is
///   "Equivalent to `-c features.<name>=true`", a dotted `features.<name>`
///   path). `[features] codex_hooks = false` is ALSO checked, defensively,
///   but this is NOT confirmed to work the same way: `codex_hooks` does not
///   appear anywhere in `codex features list`'s registry output on
///   0.146.0 (only `hooks` and the unrelated, `removed`, `plugin_hooks`
///   do) — it is checked in case it is a legacy alias, not because that
///   was verified.
/// - `allow_managed_hooks_only = true` — the spike explicitly did NOT probe
///   this live; checked at the top level of `.codex/config.toml` per this
///   task's instruction. Binary-string inspection of the pinned
///   `codex-cli` 0.146.0 executable (`strings ... | grep
///   allow_managed_hooks_only`) shows this key as a field of
///   `ConfigRequirementsToml`, associated with the enterprise/MDM
///   `requirements.toml` layer — NOT confirmed to live in, or be honored
///   from, a repo-local `.codex/config.toml` at all. The remedy text below
///   states what the config declares, not what Codex does with it.
fn check_codex_hook_flags(root: &Path) -> Check {
    const NAME: &str = "codex hook flags";
    if !crate::initcmd::codex_hooks_installed(root) {
        return Check::na(NAME);
    }
    let path = crate::initcmd::codex_config_toml_path(root);
    let Ok(text) = agentrec_core::fsguard::read_regular_to_string(&path) else {
        return Check::pass(NAME); // no config.toml at all -> nothing declares a disable
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        return Check::fail(NAME, format!("{} is not valid TOML", path.display()));
    };

    let mut reasons: Vec<String> = Vec::new();
    if let Some(features) = table.get("features").and_then(|v| v.as_table()) {
        if features.get("hooks").and_then(|v| v.as_bool()) == Some(false) {
            reasons.push(
                "[features] hooks = false disables ALL Codex hooks (confirmed live, \
                 codex-cli 0.146.0: `codex features list` reports `hooks  stable  true` by \
                 default)"
                    .to_string(),
            );
        }
        if features.get("codex_hooks").and_then(|v| v.as_bool()) == Some(false) {
            reasons.push(
                "[features] codex_hooks = false — checked defensively as a possible legacy \
                 alias for `hooks`; NOT confirmed live (this key does not appear in `codex \
                 features list`'s registry on codex-cli 0.146.0)"
                    .to_string(),
            );
        }
    }
    if table
        .get("allow_managed_hooks_only")
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        reasons.push(
            "allow_managed_hooks_only = true is declared in this config — Codex's own \
             suppression behavior for this key was NOT verified live (spike scope gap); this \
             key is also associated with the enterprise/managed requirements.toml layer, not \
             confirmed to be read from repo-local .codex/config.toml"
                .to_string(),
        );
    }

    if reasons.is_empty() {
        Check::pass(NAME)
    } else {
        Check::fail(NAME, reasons.join("; "))
    }
}

// ---- MCP registration + server probe (E4) -----------------------------------

/// Are the repo-local MCP registrations in the state `init` would leave them?
///
/// **Advisory, never `fail`, and that is a decision rather than timidity.**
/// `doctor`'s all-pass exit 0 is a deploy gate; an MCP registration is a
/// consumer-side convenience (the server answers whether or not any host has
/// it registered), and every repo initialized before E4 has no registration at
/// all. Turning those into hard failures would red the gate for a condition
/// that breaks no recording. A foreign server occupying our key is likewise
/// reported and left alone — `init` refuses to overwrite it (see
/// `initcmd::mcp_entry_is_ours`), so surfacing it is all `doctor` can honestly
/// do.
///
/// The Codex leg is `n/a`-shaped: Codex integration is opt-in (`init
/// --codex`), so a repo that never asked for it is not reported on.
fn check_mcp_registration(root: &Path) -> Check {
    const NAME: &str = "mcp registration";
    use crate::initcmd::McpRegState;
    let mut notes: Vec<String> = Vec::new();

    let claude_path = crate::initcmd::mcp_json_path(root);
    match crate::initcmd::mcp_json_state(root) {
        Ok(McpRegState::Ours) => {}
        Ok(McpRegState::Absent) => notes.push(format!(
            "the agentrec MCP server is not registered in {} — run `agentrec init` to add it",
            claude_path.display()
        )),
        Ok(McpRegState::Foreign) => notes.push(format!(
            "{} declares a server named \"agentrec\" that is not `agentrec mcp` — left \
             untouched by init and uninstall",
            claude_path.display()
        )),
        Err(e) => notes.push(e),
    }

    let codex_path = crate::initcmd::codex_config_toml_path(root);
    let codex_state = crate::initcmd::codex_mcp_state(root);
    let codex_opted_in =
        crate::initcmd::codex_hooks_installed(root) || matches!(codex_state, Ok(McpRegState::Ours));
    if codex_opted_in {
        match codex_state {
            Ok(McpRegState::Ours) => {}
            Ok(McpRegState::Absent) => notes.push(format!(
                "Codex hooks are installed but {} declares no [mcp_servers.agentrec] — run \
                 `agentrec init --codex`",
                codex_path.display()
            )),
            Ok(McpRegState::Foreign) => notes.push(format!(
                "{} declares an [mcp_servers.agentrec] that is not `agentrec mcp` — left \
                 untouched by init and uninstall",
                codex_path.display()
            )),
            Err(e) => notes.push(e),
        }
    }

    if notes.is_empty() {
        Check::pass(NAME)
    } else {
        Check::advisory(NAME, notes.join("; "))
    }
}

/// How long the `initialize` probe waits for the server's first frame before
/// giving up and killing the child. Deliberately short: `doctor` is asserted
/// to finish well under 2 s by `doctor_completes_quickly`, and every other
/// check is sub-millisecond, so this is the only one that could blow that
/// budget. A local stdio handshake that has not answered in a second is
/// broken, not slow.
const MCP_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Does `agentrec mcp` actually come up and answer `initialize` against this
/// root? Spawns the server, writes one frame, reads one line, kills the child.
///
/// **Advisory, like the registration check** — a probe failure means the MCP
/// consumer surface is broken, not that recording is. It never contributes to
/// the exit code.
///
/// **The probe spawns `current_exe()`, not a PATH lookup of `agentrec`**: a
/// PATH hit could be an entirely different (older, or absent) build than the
/// one being asked to diagnose itself. The consequence is that under the test
/// harness `current_exe()` is the *test binary*, which would be spawned with
/// `--root … mcp` and answer nothing useful; rather than add an env-var test
/// seam (which would then have to be proven absent from release `strings`),
/// the probe declares itself skipped when it is not running from a binary
/// named `agentrec`. Integration tests drive the real binary, so the live path
/// is exercised there — see `doctor_mcp_server_answers_initialize`.
fn check_mcp_server(root: &Path) -> Check {
    const NAME: &str = "mcp server";
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            return Check::advisory(NAME, format!("cannot resolve agentrec's own binary ({e})"))
        }
    };
    if exe.file_stem() != Some(std::ffi::OsStr::new("agentrec")) {
        return Check::advisory(
            NAME,
            "probe skipped — doctor is not running from the `agentrec` binary (in-process test \
             harness); run `agentrec doctor` to exercise it",
        );
    }
    match probe_mcp_initialize(&exe, root) {
        Ok(()) => Check::pass(NAME),
        Err(reason) => Check::advisory(
            NAME,
            format!(
                "`agentrec mcp` did not answer `initialize` ({reason}) — MCP hosts will see \
                     a dead server; try running `agentrec mcp` by hand to see the startup error"
            ),
        ),
    }
}

/// One `initialize` round-trip against a freshly spawned `agentrec mcp`.
/// Returns `Ok(())` iff a JSON-RPC success frame carrying a
/// `result.protocolVersion` came back within [`MCP_PROBE_TIMEOUT`].
fn probe_mcp_initialize(exe: &Path, root: &Path) -> Result<(), String> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    // Spawns THIS binary (`exe` is our own resolved path) to probe the
    // daemon; never a repo-authored command.
    #[allow(clippy::disallowed_methods)]
    let mut child = Command::new(exe)
        .arg("--root")
        .arg(root)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;

    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": crate::mcpcmd::PINNED_REVISION,
                   "capabilities": {},
                   "clientInfo": {"name": "agentrec-doctor", "version": env!("CARGO_PKG_VERSION")}},
    });
    let write_result = child
        .stdin
        .as_mut()
        .ok_or_else(|| "no stdin pipe".to_string())
        .and_then(|stdin| {
            writeln!(stdin, "{frame}")
                .and_then(|()| stdin.flush())
                .map_err(|e| format!("write failed: {e}"))
        });

    // Read the first line on a helper thread so a server that never answers
    // cannot wedge `doctor`: the child is killed either way below.
    let outcome = write_result.and_then(|()| {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "no stdout pipe".to_string())?;
        let (tx, rx) = std::sync::mpsc::channel::<std::io::Result<String>>();
        std::thread::spawn(move || {
            let mut line = String::new();
            let read = std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut line)
                .map(|_| line);
            let _ = tx.send(read);
        });
        match rx.recv_timeout(MCP_PROBE_TIMEOUT) {
            Err(_) => Err(format!("no response within {MCP_PROBE_TIMEOUT:?}")),
            Ok(Err(e)) => Err(format!("read failed: {e}")),
            Ok(Ok(line)) => {
                let value: serde_json::Value = serde_json::from_str(line.trim())
                    .map_err(|e| format!("response was not JSON ({e})"))?;
                if value
                    .get("result")
                    .and_then(|r| r.get("protocolVersion"))
                    .and_then(|v| v.as_str())
                    .is_some()
                {
                    Ok(())
                } else {
                    Err(format!("unexpected first frame: {}", line.trim()))
                }
            }
        }
    });

    let _ = child.kill();
    let _ = child.wait();
    outcome
}

// ---- inotify headroom (Linux only) ------------------------------------------

/// LADDERED: this Linux-only path cannot be exercised on macOS dev machines.
/// The code path is implemented and compiles under `#[cfg(target_os =
/// "linux")]`; its assertion is skipped here and belongs on
/// VERIFY-LEDGER.md for a real Linux run.
#[cfg(target_os = "linux")]
fn check_inotify(root: &Path) -> Check {
    // Fixed procfs path: a kernel-synthesized file, not attacker-
    // substitutable, and reads never block. `#[cfg(target_os = "linux")]`
    // means darwin clippy never lints this line; annotated for the Linux leg.
    #[allow(clippy::disallowed_methods)]
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
// This mod spawns `git` to build its own tempdir fixtures. Scoped to the test
// mod so the single production spawn in this file stays per-site annotated
// (clippy.toml).
#[allow(clippy::disallowed_methods)]
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

    // Same false-PASS class as the prompt-put leg: a non-UTF8 path skip is a
    // change the daemon saw but could not record — doctor's "store health"
    // must not PASS while state.json's non_utf8_path_skips is nonzero and
    // the other counters happen to be 0.
    #[test]
    fn non_utf8_skip_alone_fails_store_check() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        std::fs::write(
            crate::state_path(root),
            r#"{"pid":0,"signal_offset":0,"snapshot_failures":0,"io_failed":[],"non_utf8_path_skips":2}"#,
        )
        .unwrap();

        let check = check_degraded(root);
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.remedy.unwrap().contains("2 non-UTF8"));
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

    // ---- D46 AC-S7/S8: orphaned service units ---------------------------

    // Env vars are process-global; every test below drives the debug-only
    // AGENTREC_TEST_SERVICE_DIR seam, so they must serialize against each
    // other or the mutations race across cargo's parallel test threads.
    #[cfg(debug_assertions)]
    static SERVICE_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(debug_assertions)]
    fn with_service_dir<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
        let _guard = SERVICE_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("AGENTREC_TEST_SERVICE_DIR").ok();
        std::env::set_var("AGENTREC_TEST_SERVICE_DIR", dir);
        let out = f();
        match prev {
            Some(v) => std::env::set_var("AGENTREC_TEST_SERVICE_DIR", v),
            None => std::env::remove_var("AGENTREC_TEST_SERVICE_DIR"),
        }
        out
    }

    // AC-S7: a vanished root is reported, but as ADVISORY — `status` stays
    // `pass` so a stale unit from an unrelated scratch repo cannot break this
    // repo's all-pass exit-0 gate, while the note still carries the count and
    // the runnable removal command.
    // Debug-only: drives the AGENTREC_TEST_SERVICE_DIR seam, which is
    // #[cfg(debug_assertions)] in service.rs::service_dir — in a release build
    // the seam is ignored and the test would scan the developer's real
    // service directory (and can pass or fail on ambient machine state).
    #[cfg(debug_assertions)]
    #[test]
    fn orphaned_unit_is_advisory_not_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let units = tmp.path().join("units");
        std::fs::create_dir_all(&units).unwrap();
        let gone = tmp.path().join("deleted-repo"); // deliberately never created
        std::fs::write(
            units.join("com.agentrec.0123456789ab.plist"),
            crate::service::launchd_plist(Path::new("/usr/local/bin/agentrec"), &gone),
        )
        .unwrap();

        let check = with_service_dir(&units, check_orphan_services);
        assert_eq!(
            check.status,
            CheckStatus::Pass,
            "a user-global condition must never fail this repo's gate"
        );
        let remedy = check.remedy.expect("advisory must carry a note");
        assert!(remedy.starts_with("1 installed service unit "), "{remedy}");
        assert!(remedy.contains("com.agentrec.0123456789ab"), "{remedy}");
        assert!(remedy.contains(&gone.display().to_string()), "{remedy}");
        // The note must not assert abandonment — an unmounted volume reads
        // identically from here, and this repo has failed four gate rounds on
        // confidently-worded claims about real-world state.
        assert!(
            remedy.contains("unmounted volume"),
            "note must state the false-positive class: {remedy}"
        );
    }

    // The whole-report leg: `diagnose` on an otherwise-healthy-shaped repo
    // must carry the check, and an orphan must not flip `report.ok`.
    // Debug-only: drives the AGENTREC_TEST_SERVICE_DIR seam, which is
    // #[cfg(debug_assertions)] in service.rs::service_dir — in a release build
    // the seam is ignored and the test would scan the developer's real
    // service directory (and can pass or fail on ambient machine state).
    #[cfg(debug_assertions)]
    #[test]
    fn orphaned_unit_does_not_flip_report_ok_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let units = tmp.path().join("units");
        std::fs::create_dir_all(&units).unwrap();
        let gone = tmp.path().join("deleted-repo");
        std::fs::write(
            units.join("com.agentrec.0123456789ab.plist"),
            crate::service::launchd_plist(Path::new("/usr/local/bin/agentrec"), &gone),
        )
        .unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(agentrec_dir(&root)).unwrap();

        let report = with_service_dir(&units, || diagnose(&root));
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "orphaned services")
            .expect("diagnose must include the orphaned-services check");
        assert_eq!(check.status, CheckStatus::Pass);
    }

    // No orphan -> a plain pass with NO note, so a healthy machine's `doctor`
    // output gains no noise.
    // Debug-only: drives the AGENTREC_TEST_SERVICE_DIR seam, which is
    // #[cfg(debug_assertions)] in service.rs::service_dir — in a release build
    // the seam is ignored and the test would scan the developer's real
    // service directory (and can pass or fail on ambient machine state).
    #[cfg(debug_assertions)]
    #[test]
    fn no_orphans_is_a_silent_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let units = tmp.path().join("units");
        std::fs::create_dir_all(&units).unwrap();
        let live = tmp.path().join("live-repo");
        std::fs::create_dir_all(&live).unwrap();
        // The exec must exist for this to be a HEALTHY unit. It previously
        // named `/usr/local/bin/agentrec`, which passed only because nothing
        // checked the exec — i.e. the fixture depended on ambient machine
        // state to mean what it claimed. Pinned to a file this test creates.
        let exec = tmp.path().join("bin-agentrec");
        std::fs::write(&exec, b"").unwrap();
        std::fs::write(
            units.join("com.agentrec.0123456789ab.plist"),
            crate::service::launchd_plist(&exec, &live),
        )
        .unwrap();
        // An unreadable unit must not be reported either — we know nothing
        // about its root, and a fabricated orphan is worse than a missed one.
        std::fs::write(units.join("com.agentrec.deadbeef0000.plist"), "<plist/>").unwrap();

        let check = with_service_dir(&units, check_orphan_services);
        assert_eq!(check.status, CheckStatus::Pass);
        assert_eq!(check.remedy, None, "healthy machine must print no note");
    }

    // A LIVE root whose EXEC has vanished is the `brew upgrade` shape, and it
    // is invisible to everything else this tool has: the daemon never runs
    // (launchd fails at spawn, status 78), and the root-only classification
    // reports nothing. Advisory like its sibling, and it must name the exec
    // and the re-init remedy — NOT the removal command, since the repo is
    // still there and the user almost certainly wants recording back.
    // Debug-only: drives the AGENTREC_TEST_SERVICE_DIR seam, which is
    // #[cfg(debug_assertions)] in service.rs::service_dir — in a release build
    // the seam is ignored and the test would scan the developer's real
    // service directory (and can pass or fail on ambient machine state).
    #[cfg(debug_assertions)]
    #[test]
    fn vanished_exec_on_a_live_root_is_reported_advisory() {
        let tmp = tempfile::tempdir().unwrap();
        let units = tmp.path().join("units");
        std::fs::create_dir_all(&units).unwrap();
        let live = tmp.path().join("live-repo");
        std::fs::create_dir_all(&live).unwrap();
        let gone_exec = tmp.path().join("Cellar/agentrec/0.1.0/bin/agentrec"); // never created
        std::fs::write(
            units.join("com.agentrec.0123456789ab.plist"),
            crate::service::launchd_plist(&gone_exec, &live),
        )
        .unwrap();

        let check = with_service_dir(&units, check_orphan_services);
        assert_eq!(
            check.status,
            CheckStatus::Pass,
            "user-global condition must stay advisory"
        );
        let remedy = check.remedy.expect("advisory must carry a note");
        assert!(
            remedy.contains(&gone_exec.display().to_string()),
            "the note must name the missing exec: {remedy}"
        );
        assert!(
            remedy.contains("agentrec init"),
            "the note must give the re-init remedy: {remedy}"
        );
        // The root is present, so the vanished-ROOT clause must stay silent —
        // the two conditions are reported separately on purpose.
        assert!(
            !remedy.contains("--root that is not present"),
            "a live root must not be reported as vanished: {remedy}"
        );
    }

    // The two staleness axes are ORTHOGONAL, and an unreadable unit must gain
    // no exec finding at all: `Unparseable` means we know nothing, and a
    // fabricated finding is the failure mode this check's design forbids.
    // Debug-only: drives the AGENTREC_TEST_SERVICE_DIR seam, which is
    // #[cfg(debug_assertions)] in service.rs::service_dir — in a release build
    // the seam is ignored and the test would scan the developer's real
    // service directory (and can pass or fail on ambient machine state).
    #[cfg(debug_assertions)]
    #[test]
    fn vanished_root_and_unparseable_units_carry_no_exec_finding() {
        let tmp = tempfile::tempdir().unwrap();
        let units = tmp.path().join("units");
        std::fs::create_dir_all(&units).unwrap();
        let gone_root = tmp.path().join("deleted-repo");
        let gone_exec = tmp.path().join("deleted-bin/agentrec");
        std::fs::write(
            units.join("com.agentrec.0123456789ab.plist"),
            crate::service::launchd_plist(&gone_exec, &gone_root),
        )
        .unwrap();
        std::fs::write(units.join("com.agentrec.deadbeef0000.plist"), "<plist/>").unwrap();

        let scanned = crate::service::scan_units(&units);
        let unparseable = scanned
            .iter()
            .find(|u| matches!(u.state, crate::service::UnitState::Unparseable))
            .expect("the malformed unit must classify Unparseable");
        assert_eq!(
            unparseable.missing_exec, None,
            "an unreadable unit must never carry a fabricated exec finding"
        );

        // Both paths stale — the 39/40 archived shape. Reported as a vanished
        // root; the exec clause is scoped to LIVE roots so one unit never
        // produces two competing remedies.
        let check = with_service_dir(&units, check_orphan_services);
        let remedy = check.remedy.expect("advisory must carry a note");
        assert!(remedy.contains("--root that is not present"), "{remedy}");
        // Match the exec clause's own distinctive wording, NOT the bare
        // string "agentrec init" — the vanished-root note happens to contain
        // that phrase too ("usually a leftover `agentrec init` in a directory
        // since deleted"), so asserting on it would have passed vacuously.
        assert!(
            !remedy.contains("record an agentrec binary that is not present"),
            "a unit with a dead root must not also raise the exec clause: {remedy}"
        );
    }

    // AC-S8: the uninitialized-repo short-circuit returns a HARDCODED list of
    // check names; a new check missing from it makes the report shape differ
    // between the two paths. Asserted as set equality, not by name, so any
    // future check that forgets the list also reds here.
    // Debug-only: drives the AGENTREC_TEST_SERVICE_DIR seam, which is
    // #[cfg(debug_assertions)] in service.rs::service_dir — in a release build
    // the seam is ignored and the test would scan the developer's real
    // service directory (and can pass or fail on ambient machine state).
    #[cfg(debug_assertions)]
    #[test]
    fn uninitialized_report_has_the_same_check_set_as_an_initialized_one() {
        let tmp = tempfile::tempdir().unwrap();
        let units = tmp.path().join("units");
        std::fs::create_dir_all(&units).unwrap();
        let uninitialized = tmp.path().join("bare"); // no .agentrec/
        std::fs::create_dir_all(&uninitialized).unwrap();
        let initialized = tmp.path().join("repo");
        std::fs::create_dir_all(agentrec_dir(&initialized)).unwrap();

        let (bare, real) = with_service_dir(&units, || {
            (diagnose(&uninitialized), diagnose(&initialized))
        });
        let names = |r: &Report| {
            let mut n: Vec<String> = r.checks.iter().map(|c| c.name.clone()).collect();
            n.sort();
            n
        };
        assert_eq!(
            names(&bare),
            names(&real),
            "uninitialized short-circuit must cover every check"
        );
        assert!(names(&bare).contains(&"orphaned services".to_string()));
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

    // ---- C3: Codex hooks doctor checks ------------------------------------

    #[test]
    fn codex_checks_are_na_when_never_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();

        assert_eq!(check_codex_hooks(root).status, CheckStatus::Na);
        assert_eq!(check_codex_hook_flags(root).status, CheckStatus::Na);
    }

    #[test]
    fn codex_checks_pass_on_a_freshly_installed_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        crate::initcmd::install_codex_hooks(root).unwrap();

        // Shape is fine but no codex signal has ever been recorded ->
        // advisory pass, not a hard fail — a fresh install hasn't run a
        // Codex turn yet, which is not evidence of a real problem.
        let hooks_check = check_codex_hooks(root);
        assert_eq!(hooks_check.status, CheckStatus::Pass);
        assert!(hooks_check.remedy.is_some(), "advisory note expected");

        assert_eq!(check_codex_hook_flags(root).status, CheckStatus::Pass);
    }

    #[test]
    fn codex_hooks_shape_check_fails_on_mangled_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        std::fs::create_dir_all(root.join(".codex")).unwrap();
        // Only Stop installed — UserPromptSubmit/PostToolUse missing.
        std::fs::write(
            crate::initcmd::codex_hooks_json_path(root),
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": {
                    "Stop": [ { "hooks": [ { "type": "command", "command": "agentrec hook codex", "timeout": 10 } ] } ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let check = check_codex_hooks(root);
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.remedy.unwrap().contains("agentrec init --codex"));
    }

    // AC-C3: `[features] hooks = false` must produce a LOUD (Fail) degraded
    // result.
    #[test]
    fn codex_hook_flags_fails_when_hooks_feature_off() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        crate::initcmd::install_codex_hooks(root).unwrap();
        std::fs::write(
            crate::initcmd::codex_config_toml_path(root),
            "[features]\nhooks = false\n",
        )
        .unwrap();

        let check = check_codex_hook_flags(root);
        assert_eq!(check.status, CheckStatus::Fail);
        let remedy = check.remedy.unwrap();
        assert!(remedy.contains("[features] hooks = false"), "{remedy}");
        assert!(remedy.contains("hooks  stable  true"), "{remedy}");
    }

    // AC-C3: `allow_managed_hooks_only = true` must ALSO produce a LOUD
    // (Fail) degraded result, same severity as the feature-flag case.
    #[test]
    fn codex_hook_flags_fails_when_managed_hooks_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        crate::initcmd::install_codex_hooks(root).unwrap();
        std::fs::write(
            crate::initcmd::codex_config_toml_path(root),
            "allow_managed_hooks_only = true\n",
        )
        .unwrap();

        let check = check_codex_hook_flags(root);
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check
            .remedy
            .unwrap()
            .contains("allow_managed_hooks_only = true"));
    }

    #[test]
    fn codex_hook_flags_reports_both_reasons_when_both_present() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        crate::initcmd::install_codex_hooks(root).unwrap();
        std::fs::write(
            crate::initcmd::codex_config_toml_path(root),
            "allow_managed_hooks_only = true\n\n[features]\nhooks = false\n",
        )
        .unwrap();

        let check = check_codex_hook_flags(root);
        assert_eq!(check.status, CheckStatus::Fail);
        let remedy = check.remedy.unwrap();
        assert!(remedy.contains("[features] hooks = false"), "{remedy}");
        assert!(
            remedy.contains("allow_managed_hooks_only = true"),
            "{remedy}"
        );
    }

    #[test]
    fn codex_hook_flags_defensive_codex_hooks_alias_also_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        crate::initcmd::install_codex_hooks(root).unwrap();
        std::fs::write(
            crate::initcmd::codex_config_toml_path(root),
            "[features]\ncodex_hooks = false\n",
        )
        .unwrap();

        let check = check_codex_hook_flags(root);
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.remedy.unwrap().contains("codex_hooks = false"));
    }

    #[test]
    fn codex_signal_ever_seen_detects_a_codex_tool_line() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        assert!(!codex_signal_ever_seen(root));

        std::fs::write(
            signal_path(root),
            format!(
                "{}\n{}\n",
                serde_json::json!({"v":1,"ts":0,"tool":"claude","event":"start"}),
                serde_json::json!({"v":1,"ts":0,"tool":"codex","event":"start"}),
            ),
        )
        .unwrap();
        assert!(codex_signal_ever_seen(root));
    }

    // The whole-report leg: `diagnose` on a repo with codex hooks installed
    // and a disabling flag set must flip `report.ok` to false via THIS
    // check specifically.
    #[test]
    fn diagnose_reports_codex_hook_flags_failure_in_full_report() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        crate::initcmd::install_codex_hooks(root).unwrap();
        std::fs::write(
            crate::initcmd::codex_config_toml_path(root),
            "[features]\nhooks = false\n",
        )
        .unwrap();

        let report = diagnose(root);
        assert!(!report.ok);
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "codex hook flags")
            .expect("diagnose must include the codex hook flags check");
        assert_eq!(check.status, CheckStatus::Fail);
    }
}
