//! `agentrec init`: scaffold .agentrec/, gitignore entry, Claude Code hooks
//! (UserPromptSubmit + Stop, both calling `agentrec hook claude`), a locked-
//! down `.agentrec/` (0700 dirs / 0600 files, unix, D37), and a per-repo
//! recorder service unit (launchd/systemd, AC Y+3). Idempotent — re-running
//! `init` is a byte-for-byte no-op wherever nothing needs to change; that
//! case leads with "already initialized — nothing changed" (D-PD5).
//! `--dry-run` prints the intended actions without touching disk;
//! `agentrec uninstall` (`uninstallcmd.rs`) reverses everything it installs.

use crate::{agentrec_dir, log_path, objects_dir, service, signal_path, state_path};
use std::fs;
use std::path::Path;

pub(crate) const HOOK_MARKER: &str = "agentrec hook";
const HOOK_COMMAND: &str = "agentrec hook claude";
const DEFAULT_CONFIG: &str = "\
# agentrec configuration (PROTOCOL.md / SPEC.md)
ttl_days = 90
mcp_destructive = \"off\"   # off | confirm | auto (used from v2)
memory_enabled = true      # inject recalled memories into UserPromptSubmit hooks
memory_inject_max = 5      # max facts injected per hook call
# noise_globs = [\".remember/**\"]  # fold matching file entries out of log/show (see README); --all-files to reveal
";

/// D46: whether `init` should install the per-repo service unit, and if not,
/// why. Split out as a PURE function so the whole flag×path matrix is
/// assertable without executing `service::install`, which shells out to the
/// real `launchctl`/`systemctl` and is therefore off-limits to the automated
/// suite.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ServiceDecision {
    Install,
    /// `--no-service` was passed.
    SkipFlag,
    /// The root lives under a temp prefix (the carried path is the prefix that
    /// matched, so the printed reason can name it).
    SkipTemp(std::path::PathBuf),
}

/// Directories whose contents exist to be deleted. A repo root under any of
/// them gets no service unit by default: `init` installs a user-scoped unit
/// with `RunAtLoad` + `KeepAlive`, and when the root is deleted nothing reaps
/// the unit — every crashed or abandoned test run leaks one permanently.
///
/// Canonicalized on BOTH sides, which is load-bearing rather than tidiness: on
/// macOS `$TMPDIR` reads `/var/folders/…/T/` while `service::resolve_root`
/// stores the canonical `/private/var/folders/…/T/`, so a raw prefix compare
/// would never match and this guard would silently never fire.
///
/// `$TMPDIR` alone is not enough: of the 40 leaked units measured 2026-07-31,
/// 37 were under `$TMPDIR` and 3 under `/private/tmp/claude-501/…` (agent
/// session scratchpads), which is not `$TMPDIR` on any of these runs.
fn temp_prefixes() -> Vec<std::path::PathBuf> {
    let mut prefixes: Vec<std::path::PathBuf> = Vec::new();
    let mut push = |p: std::path::PathBuf| {
        let canonical = p.canonicalize().unwrap_or(p);
        if !prefixes.contains(&canonical) {
            prefixes.push(canonical);
        }
    };
    if let Ok(tmpdir) = std::env::var("TMPDIR") {
        if !tmpdir.is_empty() {
            push(std::path::PathBuf::from(tmpdir));
        }
    }
    push(std::path::PathBuf::from("/tmp"));
    push(std::path::PathBuf::from("/private/tmp"));
    prefixes
}

pub(crate) fn service_decision(
    root: &Path,
    no_service: bool,
    force_service: bool,
) -> ServiceDecision {
    if no_service {
        return ServiceDecision::SkipFlag;
    }
    if force_service {
        return ServiceDecision::Install;
    }
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    match temp_prefixes()
        .into_iter()
        .find(|prefix| canonical.starts_with(prefix))
    {
        Some(prefix) => ServiceDecision::SkipTemp(prefix),
        None => ServiceDecision::Install,
    }
}

/// The line `init` prints when it declines to install a service unit under a
/// temp root. Shared by the real run and `--dry-run` so the two can't drift.
fn temp_skip_line(prefix: &Path) -> String {
    format!(
        "skipped service install: root is under a temporary directory ({}) — \
         a service installed here outlives the directory; pass --service to install anyway",
        prefix.display()
    )
}

pub fn run(
    root: &Path,
    no_hook: bool,
    no_service: bool,
    force_service: bool,
    dry_run: bool,
) -> Result<(), String> {
    if dry_run {
        print_dry_run(root, no_hook, no_service, force_service);
        return Ok(());
    }

    let mut actions: Vec<String> = Vec::new();
    // D-PD5: whether *any* step below actually wrote/modified something on
    // disk. When a re-run finds `.agentrec/` already present and this stays
    // false, nothing changed — lead with an honest "already initialized"
    // line instead of implying work was done.
    let mut changed = false;

    let dir_already_existed = agentrec_dir(root).is_dir();
    // Tracked separately from `.agentrec/` itself: a required subdir can be
    // deleted out from under an otherwise-intact `.agentrec/` (e.g. `rm -rf
    // .agentrec/objects`). Recreating it is real disk mutation and must flip
    // `changed`, or the no-op message below fabricates "nothing changed".
    let objects_already_existed = objects_dir(root).is_dir();
    fs::create_dir_all(objects_dir(root)).map_err(|e| e.to_string())?;
    actions.push(format!("scaffolded {}", agentrec_dir(root).display()));
    changed = changed || !dir_already_existed || !objects_already_existed;

    let config = agentrec_dir(root).join("config.toml");
    if !config.exists() {
        fs::write(&config, DEFAULT_CONFIG).map_err(|e| e.to_string())?;
        actions.push("wrote default config.toml".to_string());
        changed = true;
    }

    changed = ensure_gitignore(root)? || changed;

    // E9: a perms repair (e.g. a file drifted looser since the last init) is
    // a real disk mutation and must flip `changed`, or the summary line
    // below fabricates "nothing changed" while a chmod just ran.
    let perms_changed = apply_permissions(root)?;
    changed = changed || perms_changed;
    if cfg!(unix) {
        actions.push("set .agentrec/ to 0700 (files 0600)".to_string());
    }

    if no_hook {
        actions.push("skipped Claude Code hook install (--no-hook)".to_string());
    } else {
        let hooks_changed = install_claude_hooks(root)?;
        changed = changed || hooks_changed;
        actions.push(format!(
            "Claude Code hooks {}",
            if hooks_changed {
                "installed"
            } else {
                "already present"
            }
        ));
    }

    match service_decision(root, no_service, force_service) {
        ServiceDecision::SkipFlag => {
            actions.push("skipped service install (--no-service)".to_string());
        }
        ServiceDecision::SkipTemp(prefix) => actions.push(temp_skip_line(&prefix)),
        ServiceDecision::Install => match current_exe() {
            Ok(exec) => match service::install(root, &exec) {
                Ok(mut lines) => {
                    changed = changed || lines.iter().any(|l| l.starts_with("wrote service unit"));
                    actions.append(&mut lines);
                }
                Err(e) => actions.push(format!("service install skipped: {e}")),
            },
            Err(e) => actions.push(format!("service install skipped: {e}")),
        },
    }

    if dir_already_existed && !changed {
        println!("already initialized — nothing changed");
    }
    for line in &actions {
        println!("{line}");
    }
    println!("to reverse everything: agentrec uninstall");
    Ok(())
}

/// Print every action `run` would take, without creating/writing/loading
/// anything (AC-Y+5).
fn print_dry_run(root: &Path, no_hook: bool, no_service: bool, force_service: bool) {
    println!("[dry-run] would scaffold {}", agentrec_dir(root).display());
    println!("[dry-run] would write default config.toml (if missing)");
    println!("[dry-run] would ensure .gitignore entry (git repos only)");
    if cfg!(unix) {
        println!("[dry-run] would set .agentrec/ to 0700 (files 0600)");
    }
    if no_hook {
        println!("[dry-run] would skip Claude Code hook install (--no-hook)");
    } else {
        println!("[dry-run] would install Claude Code hooks (UserPromptSubmit + Stop)");
    }
    // Same decision function as the real run, so `--dry-run` can never claim
    // an install the real run would skip.
    match service_decision(root, no_service, force_service) {
        ServiceDecision::SkipFlag => {
            println!("[dry-run] would skip service install (--no-service)")
        }
        ServiceDecision::SkipTemp(prefix) => {
            println!("[dry-run] would {}", temp_skip_line(&prefix))
        }
        ServiceDecision::Install => {
            println!("[dry-run] would write and load a per-repo service unit")
        }
    }
    println!("[dry-run] nothing on disk was touched");
}

/// Absolute path to this running `agentrec` binary — the exec the generated
/// service unit invokes.
fn current_exe() -> Result<std::path::PathBuf, String> {
    std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("cannot resolve agentrec's own executable path: {e}"))
}

/// Lock down `.agentrec/` (D37): the dir and its subdirs to 0700, files
/// (config.toml, signal.jsonl, log.jsonl, state.json, objects/*) to 0600.
/// Applied after creation; only touches what already exists at init time.
/// Returns whether any mode actually changed (E9) — a re-run that finds
/// everything already locked down correctly must report no mutation, but a
/// genuine repair (something drifted looser) must be counted as real work.
#[cfg(unix)]
fn apply_permissions(root: &Path) -> Result<bool, String> {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    let dir_mode = Permissions::from_mode(0o700);
    let file_mode = Permissions::from_mode(0o600);
    let mut changed = false;

    let dir = agentrec_dir(root);
    changed |= chmod_if_needed(&dir, &dir_mode)?;

    let obj_dir = objects_dir(root);
    if obj_dir.exists() {
        changed |= chmod_if_needed(&obj_dir, &dir_mode)?;
        changed |= chmod_tree(&obj_dir, &dir_mode, &file_mode)?;
    }

    for path in [
        dir.join("config.toml"),
        signal_path(root),
        log_path(root),
        state_path(root),
    ] {
        if path.exists() {
            changed |= chmod_if_needed(&path, &file_mode)?;
        }
    }
    Ok(changed)
}

/// Sets `mode` on `path` only when its current mode differs — returns
/// whether it actually changed anything.
#[cfg(unix)]
fn chmod_if_needed(path: &Path, mode: &std::fs::Permissions) -> Result<bool, String> {
    use std::os::unix::fs::PermissionsExt;
    let current = fs::metadata(path).map_err(|e| e.to_string())?.permissions();
    if current.mode() & 0o777 == mode.mode() & 0o777 {
        return Ok(false);
    }
    fs::set_permissions(path, mode.clone()).map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(unix)]
fn chmod_tree(
    dir: &Path,
    dir_mode: &std::fs::Permissions,
    file_mode: &std::fs::Permissions,
) -> Result<bool, String> {
    let mut changed = false;
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            changed |= chmod_if_needed(&path, dir_mode)?;
            changed |= chmod_tree(&path, dir_mode, file_mode)?;
        } else {
            changed |= chmod_if_needed(&path, file_mode)?;
        }
    }
    Ok(changed)
}

#[cfg(not(unix))]
fn apply_permissions(_root: &Path) -> Result<bool, String> {
    Ok(false)
}

/// Append `.agentrec/` to .gitignore when this is a git repo; idempotent.
/// Returns whether the file was actually written (D-PD5: feeds the
/// re-run-changed-nothing check in `run`).
fn ensure_gitignore(root: &Path) -> Result<bool, String> {
    if !root.join(".git").exists() {
        return Ok(false);
    }
    let path = root.join(".gitignore");
    let current = fs::read_to_string(&path).unwrap_or_default();
    if current
        .lines()
        .any(|l| l.trim() == ".agentrec/" || l.trim() == ".agentrec")
    {
        return Ok(false);
    }
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(".agentrec/\n");
    fs::write(&path, next).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Merge UserPromptSubmit + Stop hooks into .claude/settings.local.json.
/// Preserves unrelated hooks; backs up the pre-merge file; aborts untouched
/// on malformed JSON (IMPLEMENTATION AC A4/A5).
fn install_claude_hooks(root: &Path) -> Result<bool, String> {
    let path = root.join(".claude").join("settings.local.json");
    let current = match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<serde_json::Value>(&text).map_err(|e| {
            format!(
                "existing {} is not valid JSON ({e}); not touching it",
                path.display()
            )
        })?,
        // E5: only a genuinely absent file means "nothing to merge with" —
        // ANY other read error (permissions, invalid UTF-8, ...) must abort
        // untouched with a clear message, never silently treated as an empty
        // settings object (which would drop the user's live hooks on write).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => {
            return Err(format!(
                "cannot read existing {} ({e}); not touching it",
                path.display()
            ))
        }
    };
    let mut settings = current.clone();
    let mut changed = false;
    for event in ["UserPromptSubmit", "Stop"] {
        let (next, event_changed) = merge_hook(settings, event)?;
        settings = next;
        changed = changed || event_changed;
    }
    if changed {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if path.exists() {
            let backup = path.with_extension("local.json.bak");
            fs::copy(&path, &backup).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        fs::write(&path, text).map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

/// Add the agentrec hook under `hooks.<event>` unless the marker is present.
fn merge_hook(
    mut settings: serde_json::Value,
    event: &str,
) -> Result<(serde_json::Value, bool), String> {
    if event_has_marker(&settings, event) {
        return Ok((settings, false));
    }
    let obj = settings
        .as_object_mut()
        .ok_or("settings.local.json is not a JSON object")?;
    let hooks = obj.entry("hooks").or_insert_with(|| serde_json::json!({}));
    let hooks_obj = hooks
        .as_object_mut()
        .ok_or("settings \"hooks\" is not a JSON object")?;
    let entry = hooks_obj
        .entry(event)
        .or_insert_with(|| serde_json::json!([]));
    let arr = entry
        .as_array_mut()
        .ok_or_else(|| format!("settings \"hooks.{event}\" is not a JSON array"))?;
    arr.push(serde_json::json!({
        "hooks": [ { "type": "command", "command": HOOK_COMMAND } ]
    }));
    Ok((settings, true))
}

pub(crate) fn event_has_marker(settings: &serde_json::Value, event: &str) -> bool {
    settings
        .get("hooks")
        .and_then(|h| h.get(event))
        .and_then(|s| s.as_array())
        .map(|entries| {
            entries.iter().any(|entry| {
                entry
                    .get("hooks")
                    .and_then(|h| h.as_array())
                    .map(|hooks| {
                        hooks.iter().any(|hook| {
                            hook.get("command")
                                .and_then(|c| c.as_str())
                                .map(|c| c.contains(HOOK_MARKER))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_is_idempotent_and_preserves_unrelated_hooks() {
        let existing = serde_json::json!({
            "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "other-tool" } ] } ] }
        });
        let (merged, changed) = merge_hook(existing, "Stop").unwrap();
        assert!(changed);
        let stops = merged["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stops.len(), 2); // other-tool preserved, ours appended
        let (again, changed2) = merge_hook(merged, "Stop").unwrap();
        assert!(!changed2);
        assert_eq!(again["hooks"]["Stop"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn merge_rejects_wrong_shapes() {
        assert!(merge_hook(serde_json::json!([]), "Stop").is_err());
        assert!(merge_hook(serde_json::json!({"hooks": []}), "Stop").is_err());
        assert!(merge_hook(serde_json::json!({"hooks": {"Stop": "x"}}), "Stop").is_err());
    }

    // E5: a settings.local.json that exists but fails to READ as UTF-8 text
    // must abort `init` untouched (byte-identical) with a clear error — the
    // old code treated ANY read error (not just NotFound) as "file absent",
    // which would go on to write a fresh settings object and drop the
    // user's live hooks.
    #[test]
    fn init_aborts_untouched_on_unreadable_settings_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        let settings_dir = root.join(".claude");
        fs::create_dir_all(&settings_dir).unwrap();
        let settings_path = settings_dir.join("settings.local.json");
        // Invalid UTF-8 bytes — fs::read_to_string fails with InvalidData,
        // not NotFound.
        fs::write(&settings_path, [0x7b, 0xff, 0xfe, 0x7d]).unwrap();
        let before = fs::read(&settings_path).unwrap();

        let err = run(root, false, true, false, false).unwrap_err();
        assert!(
            err.contains("settings.local.json"),
            "error should name the file: {err}"
        );

        let after = fs::read(&settings_path).unwrap();
        assert_eq!(before, after, "unreadable settings file must be untouched");
    }

    #[test]
    fn init_scaffolds_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        run(root, true, true, false, false).unwrap();
        run(root, true, true, false, false).unwrap(); // second run: no error, no duplicates
        assert!(root.join(".agentrec/config.toml").exists());
        assert!(root.join(".agentrec/objects").exists());
        let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert_eq!(gitignore.matches(".agentrec/").count(), 1);
    }

    #[test]
    fn init_without_git_skips_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        run(tmp.path(), true, true, false, false).unwrap();
        assert!(!tmp.path().join(".gitignore").exists());
    }

    // AC-Y+5: `--dry-run` must touch nothing on disk at all.
    #[test]
    fn dry_run_touches_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let before = walk(root);
        run(root, true, true, false, true).unwrap();
        let after = walk(root);
        assert_eq!(before, after, "dry-run must not create or modify anything");
    }

    // AC-Y+3: re-running `init --no-service` (byte-for-byte no-op path) must
    // not duplicate hooks or scaffold files; exercised end-to-end here since
    // `merge_is_idempotent_and_preserves_unrelated_hooks` only covers the
    // hook-merge helper in isolation.
    #[test]
    fn init_no_service_rerun_is_byte_for_byte_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        run(root, false, true, false, false).unwrap();
        let settings_path = root.join(".claude/settings.local.json");
        let first = fs::read_to_string(&settings_path).unwrap();
        let config_first = fs::read_to_string(root.join(".agentrec/config.toml")).unwrap();

        run(root, false, true, false, false).unwrap();
        let second = fs::read_to_string(&settings_path).unwrap();
        let config_second = fs::read_to_string(root.join(".agentrec/config.toml")).unwrap();

        assert_eq!(
            first, second,
            "hooks file must be byte-identical on re-init"
        );
        assert_eq!(config_first, config_second);
    }

    // AC-I++ perms (D37): a fresh init locks .agentrec/ to 0700 and its files
    // to 0600.
    #[cfg(unix)]
    #[test]
    fn fresh_init_locks_down_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        run(root, true, true, false, false).unwrap();

        let dir_mode = fs::metadata(agentrec_dir(root))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);

        let file_mode = fs::metadata(agentrec_dir(root).join("config.toml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(file_mode & 0o777, 0o600);
    }

    // ---- D46 AC-S1..S4: temp-root service guard -------------------------

    // AC-S1/S2: the guard must actually FIRE on a real temp directory. This is
    // the trap the whole design turns on: `tempfile::tempdir()` hands back
    // `/var/folders/…` on macOS while `$TMPDIR` also reads `/var/folders/…`,
    // but `service::resolve_root` stores the canonicalized `/private/var/…` —
    // compare either side raw and this guard silently never fires. Uses a real
    // tempdir rather than a hand-written "/tmp/x" string for exactly that
    // reason.
    #[test]
    fn service_decision_skips_under_a_real_temp_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let decision = service_decision(tmp.path(), false, false);
        assert!(
            matches!(decision, ServiceDecision::SkipTemp(_)),
            "a real tempdir must be recognized as temp, got {decision:?}"
        );
    }

    // The three leaked scratchpad units of 2026-07-31 sat under /private/tmp,
    // which is NOT $TMPDIR on this machine — $TMPDIR alone covers only 37 of
    // the 40. Asserted against the prefix list directly since a test can't
    // create a directory under /private/tmp without polluting the machine.
    #[test]
    fn temp_prefixes_cover_tmp_as_well_as_tmpdir() {
        let prefixes = temp_prefixes();
        let has = |p: &str| {
            let canonical = std::path::Path::new(p)
                .canonicalize()
                .unwrap_or_else(|_| std::path::PathBuf::from(p));
            prefixes.contains(&canonical)
        };
        assert!(has("/tmp"), "{prefixes:?}");
        assert!(has("/private/tmp"), "{prefixes:?}");
        if let Ok(t) = std::env::var("TMPDIR") {
            if !t.is_empty() {
                assert!(has(&t), "$TMPDIR must be covered too: {prefixes:?}");
            }
        }
    }

    /// A non-temp root that stays non-temp no matter where this repo is
    /// checked out.
    ///
    /// This used to be `env!("CARGO_MANIFEST_DIR")` — "the repo this test is
    /// compiled from: a real, non-temp path" — which is only true when the
    /// checkout itself isn't under a temp prefix. A `claimd verify` run
    /// against a checkout under `$TMPDIR` REFUTED the claim these tests back
    /// (`clm_4AFDDT3XCSFHKDZ06D926XJVNY`, exit 101), and it was right to: the
    /// tests were environment-coupled, asserting `Install` for a path that
    /// genuinely IS temp there. Reproduced by cloning to `$TMPDIR` and
    /// re-running. Anyone building in a temp workdir — CI included — hit it.
    ///
    /// Both branches of the predicate are covered deliberately: `/usr` exists
    /// (canonicalization succeeds) and the second path does not
    /// (canonicalization fails and `service_decision` falls back to the path
    /// as given), so neither branch can silently start reporting temp.
    const ORDINARY_ROOTS: &[&str] = &["/usr", "/definitely-not-a-temp-dir/repo"];

    // AC-S4: an ordinary repo path must still install — the rail against an
    // over-broad predicate that would disable the service for everyone.
    #[test]
    fn service_decision_installs_for_an_ordinary_root() {
        for raw in ORDINARY_ROOTS {
            assert_eq!(
                service_decision(std::path::Path::new(raw), false, false),
                ServiceDecision::Install,
                "{raw} is not a temp path and must still install"
            );
        }
    }

    // AC-S2/S3: the whole matrix in one place. `--no-service` wins outright;
    // `--service` forces an install that the temp guard would otherwise skip.
    // (`--no-service` + `--service` together never reaches here — clap rejects
    // it; see the CLI-level conflict test in the integration suite.)
    #[test]
    fn service_decision_matrix() {
        let tmp = tempfile::tempdir().unwrap();
        let temp_root = tmp.path();
        // NOT `CARGO_MANIFEST_DIR` — see ORDINARY_ROOTS: a checkout under
        // $TMPDIR made that assertion false and refuted this test's claim.
        let ordinary = std::path::Path::new(ORDINARY_ROOTS[0]);

        assert_eq!(
            service_decision(temp_root, true, false),
            ServiceDecision::SkipFlag
        );
        assert_eq!(
            service_decision(ordinary, true, false),
            ServiceDecision::SkipFlag
        );
        assert_eq!(
            service_decision(temp_root, false, true),
            ServiceDecision::Install,
            "--service must override the temp-root skip"
        );
        assert_eq!(
            service_decision(ordinary, false, false),
            ServiceDecision::Install
        );
        assert!(matches!(
            service_decision(temp_root, false, false),
            ServiceDecision::SkipTemp(_)
        ));
    }

    // AC-S1: the guard must skip ONLY the service. Every other init step still
    // runs, and the printed reason names both the cause and the override — a
    // silent skip would just move the confusion instead of removing it.
    #[test]
    fn init_under_temp_root_still_does_everything_but_the_service() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();

        // `no_service: false` — the guard, not the flag, is what must skip it.
        run(root, false, false, false, false).unwrap();

        assert!(root.join(".agentrec/config.toml").exists());
        assert!(root.join(".agentrec/objects").exists());
        assert!(root.join(".claude/settings.local.json").exists());
        assert!(fs::read_to_string(root.join(".gitignore"))
            .unwrap()
            .contains(".agentrec/"));
        // No unit file was written for this root.
        let unit = service::unit_path(root).unwrap();
        assert!(
            !unit.exists(),
            "a temp-root init must not install a service unit: {}",
            unit.display()
        );
        // And the reason names the override.
        let line = temp_skip_line(tmp.path());
        assert!(line.contains("--service"), "{line}");
        assert!(line.contains("temporary directory"), "{line}");
    }

    fn walk(root: &Path) -> Vec<std::path::PathBuf> {
        let mut out = vec![];
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                out.push(path);
            }
        }
        out.sort();
        out
    }
}
