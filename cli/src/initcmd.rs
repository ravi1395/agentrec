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
# store_budget_bytes = 2147483648  # snapshot-store budget in bytes (default 2 GiB); the daemon's eviction tick enforces it
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
///
/// **`$TMPDIR` is also not sufficient for the ones it does cover, and this
/// suite proved it by leaking a 42nd unit.** macOS's per-user temp dir is
/// `/var/folders/<x>/<y>/T/`, reachable ONLY via `$TMPDIR` unless matched
/// structurally — and that env read is not reliable: (a) `$TMPDIR` is simply
/// unset in launchd/cron contexts, and (b) `std::env::set_var` mutating the
/// environment on another thread can make a concurrent `var()` miss (the data
/// race that made `set_var` unsafe in edition 2024) — this test binary does
/// exactly that in `service.rs`/`doctorcmd.rs`, and one such miss let the
/// guard fall through to `Install` and write a real launchd unit for a
/// tempdir root. `/var/folders` is therefore matched as a STATIC prefix, so
/// the common case never depends on reading an env var at all. `$TMPDIR` is
/// still consulted for non-default and non-macOS values.
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
    // macOS per-user temp/cache root. Exclusively OS-managed scratch space —
    // no one keeps a repo here on purpose, and `--service` overrides anyway.
    push(std::path::PathBuf::from("/var/folders"));
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
    codex: bool,
    no_service: bool,
    force_service: bool,
    dry_run: bool,
) -> Result<(), String> {
    if dry_run {
        print_dry_run(root, no_hook, codex, no_service, force_service);
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
        actions.push("skipped Claude Code MCP registration (--no-hook)".to_string());
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
        // E4: the `.mcp.json` registration rides the same `--no-hook` gate as
        // the hooks — both are the Claude Code integration surface, and a user
        // who declined one has declined the other.
        match install_mcp_json(root) {
            Ok(McpRegOutcome::Registered { changed: c }) => {
                changed = changed || c;
                actions.push(format!(
                    "Claude Code MCP registration {} ({})",
                    if c { "written" } else { "already present" },
                    mcp_json_path(root).display()
                ));
            }
            Ok(McpRegOutcome::Foreign) => actions.push(mcp_foreign_line(&mcp_json_path(root))),
            Err(e) => actions.push(format!("Claude Code MCP registration skipped: {e}")),
        }
    }

    if !codex {
        actions.push("skipped Codex hook install (pass --codex to enable)".to_string());
        actions.push("skipped Codex MCP registration (pass --codex to enable)".to_string());
    } else {
        match install_codex_hooks(root) {
            Ok(CodexHooksOutcome::Refused) => actions.push(codex_refuse_line(root)),
            Ok(CodexHooksOutcome::Installed { target, changed: c }) => {
                changed = changed || c;
                actions.push(format!(
                    "Codex hooks {} ({})",
                    if c { "installed" } else { "already present" },
                    codex_target_label(target, root),
                ));
                actions.push(CODEX_TRUST_REMINDER.to_string());
            }
            Err(e) => actions.push(format!("Codex hook install skipped: {e}")),
        }
        // MCP registration lives in a DIFFERENT table (`[mcp_servers]`) of a
        // different layer than `[hooks]`, so it does not create a second hook
        // representation — but it is still gated on the dual-representation
        // refusal, because `codex_refuse_line` promises the user both files
        // are left alone. Writing `[mcp_servers.agentrec]` into their
        // `config.toml` (re-serialized, comments dropped, `.bak` left behind)
        // while printing that we refused to touch it would make that printed
        // line false. Pinned by `codex_init_both_present_refuses_untouched`.
        if matches!(codex_hooks_target(root), Ok(CodexHooksTarget::Refuse)) {
            actions.push(
                "skipped Codex MCP registration: the dual hook-representation state above \
                 leaves .codex/config.toml untouched — consolidate to one hook file, then \
                 re-run `agentrec init --codex`"
                    .to_string(),
            );
        } else {
            match install_codex_mcp(root) {
                Ok(McpRegOutcome::Registered { changed: c }) => {
                    changed = changed || c;
                    actions.push(format!(
                        "Codex MCP registration {} ({})",
                        if c { "written" } else { "already present" },
                        codex_config_toml_path(root).display()
                    ));
                }
                Ok(McpRegOutcome::Foreign) => {
                    actions.push(mcp_foreign_line(&codex_config_toml_path(root)))
                }
                Err(e) => actions.push(format!("Codex MCP registration skipped: {e}")),
            }
        }
    }

    match service_decision(root, no_service, force_service) {
        ServiceDecision::SkipFlag => {
            actions.push("skipped service install (--no-service)".to_string());
        }
        ServiceDecision::SkipTemp(prefix) => actions.push(temp_skip_line(&prefix)),
        ServiceDecision::Install => match service_exec_path() {
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
fn print_dry_run(root: &Path, no_hook: bool, codex: bool, no_service: bool, force_service: bool) {
    println!("[dry-run] would scaffold {}", agentrec_dir(root).display());
    println!("[dry-run] would write default config.toml (if missing)");
    println!("[dry-run] would ensure .gitignore entry (git repos only)");
    if cfg!(unix) {
        println!("[dry-run] would set .agentrec/ to 0700 (files 0600)");
    }
    if no_hook {
        println!("[dry-run] would skip Claude Code hook install (--no-hook)");
        println!("[dry-run] would skip Claude Code MCP registration (--no-hook)");
    } else {
        println!(
            "[dry-run] would install Claude Code hooks (UserPromptSubmit + \
             PostToolUse[Bash] + Stop)"
        );
        // Same read-only inspect function the real run decides with, so a dry
        // run can never claim a registration the real run would refuse.
        print_dry_run_mcp("Claude Code", &mcp_json_path(root), mcp_json_state(root));
    }
    // Same read-only inspect function the real run uses to decide, so
    // `--dry-run` can never claim an install (or a refuse) the real run
    // would not also do.
    if !codex {
        println!("[dry-run] would skip Codex hook install (pass --codex to enable)");
        println!("[dry-run] would skip Codex MCP registration (pass --codex to enable)");
    } else {
        match codex_hooks_target(root) {
            Ok(CodexHooksTarget::Refuse) => println!("[dry-run] {}", codex_refuse_line(root)),
            Ok(target) => println!(
                "[dry-run] would install Codex hooks (UserPromptSubmit + PostToolUse[apply_patch] \
                 + Stop) into {}",
                codex_target_label(target, root)
            ),
            Err(e) => println!("[dry-run] Codex hook install would be skipped: {e}"),
        }
        // Same Refuse gate as the real run, decided by the same inspect
        // function, so a dry run cannot promise a registration the real run
        // withholds.
        if matches!(codex_hooks_target(root), Ok(CodexHooksTarget::Refuse)) {
            println!(
                "[dry-run] would skip Codex MCP registration too — the dual \
                 hook-representation state leaves .codex/config.toml untouched"
            );
        } else {
            print_dry_run_mcp(
                "Codex",
                &codex_config_toml_path(root),
                codex_mcp_state(root),
            );
        }
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
            println!("[dry-run] would write and load a per-repo service unit");
            // The exec path is a SNAPSHOT baked into the unit and never
            // re-resolved at load time, so it is the one value a dry run most
            // needs to show: a wrong exec here is a silently dead recorder
            // months later, not a visible error now.
            match service_exec_path() {
                Ok(exec) => println!("[dry-run]   recording exec: {}", exec.display()),
                Err(e) => println!("[dry-run]   service install would be skipped: {e}"),
            }
        }
    }
    println!("[dry-run] nothing on disk was touched");
}

/// One `--dry-run` line per MCP registration surface, derived from the SAME
/// read-only state function the real run branches on.
fn print_dry_run_mcp(host: &str, path: &Path, state: Result<McpRegState, String>) {
    match state {
        Ok(McpRegState::Absent) => println!(
            "[dry-run] would register the agentrec MCP server in {}",
            path.display()
        ),
        Ok(McpRegState::Ours) => println!(
            "[dry-run] {host} MCP registration already present in {}",
            path.display()
        ),
        Ok(McpRegState::Foreign) => println!("[dry-run] {}", mcp_foreign_line(path)),
        Err(e) => println!("[dry-run] {host} MCP registration would be skipped: {e}"),
    }
}

/// Absolute path to this running `agentrec` binary — the exec the generated
/// service unit invokes.
///
/// **Deliberately NOT `canonicalize`d, and this is the whole point of the
/// function.** Canonicalize fully resolves symlinks, and Homebrew installs
/// `/opt/homebrew/bin/<tool>` as a symlink into a version-pinned Cellar
/// directory (verified: `/opt/homebrew/bin/rg -> ../Cellar/ripgrep/15.2.0/
/// bin/rg`). A unit that records the resolved target pins one Cellar version
/// forever: after `brew upgrade` the service keeps running the OLD binary,
/// and once the old version is reaped the unit becomes a permanent launchd
/// status-78 respawn loop. The invocation path is stable across upgrades.
///
/// Canonicalizing is right for the **root** (`service::resolve_root` dedupes
/// two spellings of one repo into one unit) and wrong for the **exec**.
///
/// Linux is not fixed by this and cannot be: `std::env::current_exe()` there
/// reads `/proc/self/exe`, which the kernel has already resolved, so a
/// symlinked install records the target no matter what this function does.
/// Bounded residual, not an oversight.
fn service_exec_path() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot resolve agentrec's own executable path: {e}"))?;
    // A unit's exec must be absolute — launchd/systemd run it with no useful
    // cwd. `current_exe` is absolute on both supported platforms; joining cwd
    // is a belt-and-braces fallback that still never resolves a symlink.
    if exe.is_absolute() {
        return Ok(exe);
    }
    let cwd = std::env::current_dir()
        .map_err(|e| format!("cannot resolve agentrec's own executable path: {e}"))?;
    Ok(cwd.join(exe))
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
    // Guarded like every other config read here: this function MERGES (it
    // appends to whatever it read and writes the result back), so a read that
    // silently produced "" would truncate the user's .gitignore. A fifo would
    // additionally hang the read outright. Only the non-regular case is
    // escalated; every other read failure keeps its existing empty-string
    // meaning, which is what makes a first `init` in a repo with no
    // `.gitignore` still work.
    let current = match agentrec_core::fsguard::read_regular_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
            return Err(format!(
                "cannot read existing {} ({e}); not touching it",
                path.display()
            ))
        }
        Err(_) => String::new(),
    };
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

/// Claude Code's three hook events and their matchers, mirroring
/// [`CODEX_HOOK_EVENTS`]. `PostToolUse` is scoped to `Bash` by matcher for the
/// same reason Codex's is scoped to `apply_patch`: only that tool's firings can
/// carry a test-runner invocation, and an unmatched entry would wake a hook
/// process on every tool call.
///
/// The `PostToolUse` entry feeds attest capture path 2
/// (`cmds::hook` → `attest::capture::capture_from_hook_payload`). **The payload
/// field names that path reads — `tool_input.command`, `tool_response.stdout`,
/// `tool_response.stderr` — are Claude Code's DOCUMENTED names and are not
/// backed by any captured fixture in this repo** (every committed `PostToolUse`
/// fixture is a Codex `apply_patch` one). A payload missing them is simply not
/// captured, so a wrong guess costs evidence, never correctness.
pub(crate) const CLAUDE_HOOK_EVENTS: &[(&str, Option<&str>)] = &[
    ("UserPromptSubmit", None),
    ("PostToolUse", Some("Bash")),
    ("Stop", None),
];

/// The two events agentrec's TURN BRACKETING depends on. `doctor` requires
/// exactly these — `PostToolUse` is an attest-capture addition, and requiring
/// it would fail `doctor` on every repo initialized before it existed.
pub(crate) const CLAUDE_BRACKET_EVENTS: &[&str] = &["UserPromptSubmit", "Stop"];

/// Merge Claude Code's hooks into .claude/settings.local.json.
/// Preserves unrelated hooks; backs up the pre-merge file; aborts untouched
/// on malformed JSON (IMPLEMENTATION AC A4/A5).
fn install_claude_hooks(root: &Path) -> Result<bool, String> {
    let path = root.join(".claude").join("settings.local.json");
    let current = match agentrec_core::fsguard::read_regular_to_string(&path) {
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
    for (event, matcher) in CLAUDE_HOOK_EVENTS {
        let (next, event_changed) = merge_hook(settings, event, *matcher)?;
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
///
/// `matcher`, when set, goes BESIDE the inner `hooks[]` on the entry — the same
/// place Codex's shape puts it ([`codex_hook_json_entry`]), which is also
/// Claude Code's documented `settings.json` shape.
fn merge_hook(
    mut settings: serde_json::Value,
    event: &str,
    matcher: Option<&str>,
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
    let mut entry = serde_json::Map::new();
    if let Some(m) = matcher {
        entry.insert("matcher".to_string(), serde_json::json!(m));
    }
    entry.insert(
        "hooks".to_string(),
        serde_json::json!([ { "type": "command", "command": HOOK_COMMAND } ]),
    );
    arr.push(serde_json::Value::Object(entry));
    Ok((settings, true))
}

pub(crate) fn event_has_marker(settings: &serde_json::Value, event: &str) -> bool {
    event_has_marker_with(settings, event, HOOK_MARKER)
}

/// Generalized over the marker string so the Codex hooks.json installer
/// below can reuse the exact same "does `hooks.<event>[].hooks[].command`
/// contain our marker" shape-walk instead of re-deriving it — the shape is
/// identical between Claude's `settings.local.json` and Codex's
/// `hooks.json` (`hooks.<Event>` = array of groups, each group has an inner
/// `hooks[]` of `{type, command, ...}`).
fn event_has_marker_with(settings: &serde_json::Value, event: &str, marker: &str) -> bool {
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
                                .map(|c| c.contains(marker))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

// ============================================================================
// Codex hooks (C3): `.codex/hooks.json` or inline `[hooks]` in
// `.codex/config.toml`, whichever the repo already uses — mirrors the
// Claude Code installer above structurally (marker-scoped merge, backup
// before rewrite, idempotent re-run), generalized for Codex's two possible
// config locations and its extra `PostToolUse` matcher + explicit timeout.
// Grounded throughout by the live spike against pinned `codex-cli 0.146.0`
// (`docs/verify/codex-spike.md`) — nothing below invents a payload or
// config shape the spike didn't observe or that this round didn't itself
// measure live (see the timeout and `[features] hooks` comments).
// ============================================================================

pub(crate) const CODEX_HOOK_MARKER: &str = "agentrec hook codex";
const CODEX_HOOK_COMMAND: &str = "agentrec hook codex";

/// Per-hook timeout (seconds) for every installed Codex hook entry.
///
/// Codex's default is 600s (10 minutes) per this task's own brief ("spike/
/// docs say the default is 600 s") — NOT independently measured by the
/// spike itself (`grep -n 600 docs/verify/codex-spike.md` is empty; the
/// spike's live probes never let a hook run long enough to hit any
/// timeout). A wedged `agentrec hook codex` (e.g. a stuck flock on
/// `codex-scratch.lock`) must not hold a Codex turn hostage for 10 minutes
/// regardless of the exact documented figure. 10s is chosen, not merely
/// "small": it IS spike-measured — the exact value the spike's own probe
/// hooks ran under live and completed well within
/// (`docs/verify/codex-spike.md`'s TUI capture, "Timeout 10s"), and every
/// operation `agentrec hook codex` performs is local — one stdin read, one
/// flock+append to `codex-scratch.jsonl` or `signal.jsonl`, no network call
/// — so 10s leaves roughly two orders of magnitude of headroom over the
/// actual (sub-millisecond, unmeasured-but-structurally-bounded) work it
/// does.
const CODEX_HOOK_TIMEOUT_SECS: u64 = 10;

/// The three hook events this integration installs, paired with the
/// `PostToolUse` matcher (decision 17: only `apply_patch` firings carry a
/// spike-confirmed extraction rule). `pub(crate)` — `doctorcmd.rs` walks
/// this same list to validate installed shape without re-deriving it.
pub(crate) const CODEX_HOOK_EVENTS: &[(&str, Option<&str>)] = &[
    ("UserPromptSubmit", None),
    ("PostToolUse", Some("apply_patch")),
    ("Stop", None),
];

pub(crate) fn codex_hooks_json_path(root: &Path) -> std::path::PathBuf {
    root.join(".codex").join("hooks.json")
}

pub(crate) fn codex_config_toml_path(root: &Path) -> std::path::PathBuf {
    root.join(".codex").join("config.toml")
}

/// Which Codex config file `install_codex_hooks` would write to, decided
/// PURELY by inspection (read-only — never writes) so `--dry-run` and the
/// real run share one decision and can never diverge, same precedent as
/// `service_decision`/`print_dry_run` above.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum CodexHooksTarget {
    /// `.codex/hooks.json` — the default: written when it already exists,
    /// or when NEITHER it nor an inline `[hooks]` table exists (preference
    /// order per IMPLEMENTATION.md 454 / this task: hooks.json first).
    HooksJson,
    /// The inline `[hooks]` table in `.codex/config.toml` — used only when
    /// hooks.json is absent AND config.toml already has that table.
    ConfigToml,
    /// Both exist. Codex itself loads and merges both, printing a startup
    /// warning ("prefer a single representation for this layer" — confirmed
    /// live, spike doc "hooks.json + inline [hooks] merge + startup
    /// warning"). agentrec refuses to add a third source and contribute to
    /// that already-warned state.
    Refuse,
}

pub(crate) fn codex_hooks_target(root: &Path) -> Result<CodexHooksTarget, String> {
    let hooks_json_present = codex_hooks_json_path(root).is_file();
    let config_toml_path = codex_config_toml_path(root);
    let inline_present = match agentrec_core::fsguard::read_regular_to_string(&config_toml_path) {
        Ok(text) => {
            let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
                format!(
                    "existing {} is not valid TOML ({e}); not touching Codex hook config",
                    config_toml_path.display()
                )
            })?;
            table.contains_key("hooks")
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            return Err(format!(
                "cannot read {} ({e}); not touching Codex hook config",
                config_toml_path.display()
            ))
        }
    };
    Ok(match (hooks_json_present, inline_present) {
        (true, true) => CodexHooksTarget::Refuse,
        (true, false) | (false, false) => CodexHooksTarget::HooksJson,
        (false, true) => CodexHooksTarget::ConfigToml,
    })
}

pub(crate) enum CodexHooksOutcome {
    Installed {
        target: CodexHooksTarget,
        changed: bool,
    },
    Refused,
}

pub(crate) fn install_codex_hooks(root: &Path) -> Result<CodexHooksOutcome, String> {
    match codex_hooks_target(root)? {
        CodexHooksTarget::Refuse => Ok(CodexHooksOutcome::Refused),
        CodexHooksTarget::HooksJson => Ok(CodexHooksOutcome::Installed {
            target: CodexHooksTarget::HooksJson,
            changed: install_codex_hooks_json(root)?,
        }),
        CodexHooksTarget::ConfigToml => Ok(CodexHooksOutcome::Installed {
            target: CodexHooksTarget::ConfigToml,
            changed: install_codex_hooks_toml(root)?,
        }),
    }
}

/// One installed hook-command object: `{"type":"command","command":...,
/// "timeout":...}` plus an optional `matcher` on the entry itself (Codex's
/// shape puts `matcher` beside `hooks[]`, not inside each hook — same place
/// Claude Code puts it, confirmed by the spike's `/hooks` review screen
/// showing `Matcher` as a per-entry field alongside `Command`/`Timeout`).
fn codex_hook_json_entry(matcher: Option<&str>) -> serde_json::Value {
    let mut entry = serde_json::Map::new();
    if let Some(m) = matcher {
        entry.insert("matcher".to_string(), serde_json::json!(m));
    }
    entry.insert(
        "hooks".to_string(),
        serde_json::json!([{
            "type": "command",
            "command": CODEX_HOOK_COMMAND,
            "timeout": CODEX_HOOK_TIMEOUT_SECS,
        }]),
    );
    serde_json::Value::Object(entry)
}

/// Merge `.codex/hooks.json`: same read/merge/backup/write shape as
/// `install_claude_hooks` above, generalized over the 3 Codex events.
fn install_codex_hooks_json(root: &Path) -> Result<bool, String> {
    let path = codex_hooks_json_path(root);
    let current = match agentrec_core::fsguard::read_regular_to_string(&path) {
        Ok(text) => serde_json::from_str::<serde_json::Value>(&text).map_err(|e| {
            format!(
                "existing {} is not valid JSON ({e}); not touching it",
                path.display()
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => {
            return Err(format!(
                "cannot read existing {} ({e}); not touching it",
                path.display()
            ))
        }
    };
    let mut settings = current;
    let mut changed = false;
    for (event, matcher) in CODEX_HOOK_EVENTS {
        if event_has_marker_with(&settings, event, CODEX_HOOK_MARKER) {
            continue;
        }
        let obj = settings
            .as_object_mut()
            .ok_or("hooks.json is not a JSON object")?;
        let hooks = obj.entry("hooks").or_insert_with(|| serde_json::json!({}));
        let hooks_obj = hooks
            .as_object_mut()
            .ok_or("hooks.json \"hooks\" is not a JSON object")?;
        let arr_entry = hooks_obj
            .entry(*event)
            .or_insert_with(|| serde_json::json!([]));
        let arr = arr_entry
            .as_array_mut()
            .ok_or_else(|| format!("hooks.json \"hooks.{event}\" is not a JSON array"))?;
        arr.push(codex_hook_json_entry(*matcher));
        changed = true;
    }
    if changed {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if path.exists() {
            let backup = path.with_extension("json.bak");
            fs::copy(&path, &backup).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        fs::write(&path, text).map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

fn toml_event_has_marker(hooks_table: &toml::Table, event: &str, marker: &str) -> bool {
    hooks_table
        .get(event)
        .and_then(|v| v.as_array())
        .map(|entries| {
            entries.iter().any(|entry| {
                entry
                    .as_table()
                    .and_then(|t| t.get("hooks"))
                    .and_then(|h| h.as_array())
                    .map(|hooks| {
                        hooks.iter().any(|hook| {
                            hook.as_table()
                                .and_then(|t| t.get("command"))
                                .and_then(|c| c.as_str())
                                .map(|c| c.contains(marker))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn codex_hook_toml_entry(matcher: Option<&str>) -> toml::Value {
    let mut hook = toml::Table::new();
    hook.insert(
        "type".to_string(),
        toml::Value::String("command".to_string()),
    );
    hook.insert(
        "command".to_string(),
        toml::Value::String(CODEX_HOOK_COMMAND.to_string()),
    );
    hook.insert(
        "timeout".to_string(),
        toml::Value::Integer(CODEX_HOOK_TIMEOUT_SECS as i64),
    );
    let mut entry = toml::Table::new();
    if let Some(m) = matcher {
        entry.insert("matcher".to_string(), toml::Value::String(m.to_string()));
    }
    entry.insert(
        "hooks".to_string(),
        toml::Value::Array(vec![toml::Value::Table(hook)]),
    );
    toml::Value::Table(entry)
}

/// Merge the inline `[hooks]` table in `.codex/config.toml`. Re-serializes
/// the WHOLE file (`toml::to_string_pretty`) — this reformats the user's
/// existing file (comments and key ordering are not preserved by the `toml`
/// crate's `Value` round-trip; adding `toml_edit` for format-preservation is
/// out of scope — no new dependency, and `Cargo.lock`/`--locked` must not
/// move). This is disclosed to the user via the printed action line, not
/// silent. It does not un-trust the user's OTHER hooks: the spike measured
/// that Codex's hook trust is keyed on the PARSED hook definition, not raw
/// file bytes — re-serializing with different JSON whitespace made no
/// difference to any trust decision observed (spike "Hash-invalidation"
/// section, bonus finding).
fn install_codex_hooks_toml(root: &Path) -> Result<bool, String> {
    let path = codex_config_toml_path(root);
    let text = match agentrec_core::fsguard::read_regular_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(format!(
                "cannot read existing {} ({e}); not touching it",
                path.display()
            ))
        }
    };
    let mut doc: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        format!(
            "existing {} is not valid TOML ({e}); not touching it",
            path.display()
        )
    })?;
    let mut changed = false;
    let hooks_val = doc
        .entry("hooks")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let hooks_table = hooks_val
        .as_table_mut()
        .ok_or_else(|| format!("{} \"hooks\" is not a TOML table", path.display()))?;
    for (event, matcher) in CODEX_HOOK_EVENTS {
        if toml_event_has_marker(hooks_table, event, CODEX_HOOK_MARKER) {
            continue;
        }
        let arr_val = hooks_table
            .entry(event.to_string())
            .or_insert_with(|| toml::Value::Array(Vec::new()));
        let arr = arr_val
            .as_array_mut()
            .ok_or_else(|| format!("{} \"hooks.{event}\" is not a TOML array", path.display()))?;
        arr.push(codex_hook_toml_entry(*matcher));
        changed = true;
    }
    if changed {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if path.exists() {
            let backup = path.with_extension("toml.bak");
            fs::copy(&path, &backup).map_err(|e| e.to_string())?;
        }
        let out = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        fs::write(&path, out).map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

/// Printed once Codex hooks are actually written (installed or already
/// present) — never on refuse. Two spike-confirmed facts, cited rather than
/// hedged (`docs/verify/codex-spike.md`): (1) `codex exec` gives ZERO
/// stdout/stderr indication of untrusted hooks — the documented warning is
/// TUI-only, so this line is automation's only signal; (2) trust is keyed
/// on a hash of each hook's exact parsed definition — changing ANY field
/// (command, timeout, or matcher) later silently drops JUST that one hook
/// back to skipped-until-re-trusted, demonstrated live independently on a
/// command-only change and a timeout-only change ("Hash-invalidation"
/// section, Test A / Test B).
const CODEX_TRUST_REMINDER: &str = "Codex hooks are installed but not yet trusted — inside \
    Codex, run /hooks and trust the 3 new/changed hooks (or pass \
    --dangerously-bypass-hook-trust for CI/automation). `codex exec` runs silently with \
    untrusted hooks skipped — no warning at all outside the interactive TUI. Trust is keyed on \
    a hash of each hook's exact definition: changing ANY field later (command, timeout, \
    matcher) silently un-trusts just that hook again (docs/verify/codex-spike.md).";

fn codex_refuse_line(root: &Path) -> String {
    format!(
        "skipped Codex hook install: both {} and an inline [hooks] table in {} exist — Codex \
         loads and merges both, printing a startup warning (\"prefer a single representation \
         for this layer\", confirmed live); agentrec refuses to add a third source and \
         contribute to that warned state. Consolidate to one file, then re-run \
         `agentrec init --codex`.",
        codex_hooks_json_path(root).display(),
        codex_config_toml_path(root).display(),
    )
}

/// Whether ANY agentrec Codex hook marker is present, in either possible
/// config location. Read-only; `doctorcmd.rs` uses this to decide whether
/// its Codex-specific checks apply at all — Codex integration is opt-in
/// (`--codex`), so a repo that never asked for it must report `n/a`, not a
/// fabricated failure.
pub(crate) fn codex_hooks_installed(root: &Path) -> bool {
    if let Ok(text) = agentrec_core::fsguard::read_regular_to_string(&codex_hooks_json_path(root)) {
        if let Ok(settings) = serde_json::from_str::<serde_json::Value>(&text) {
            if CODEX_HOOK_EVENTS
                .iter()
                .any(|(event, _)| event_has_marker_with(&settings, event, CODEX_HOOK_MARKER))
            {
                return true;
            }
        }
    }
    if let Ok(text) = agentrec_core::fsguard::read_regular_to_string(&codex_config_toml_path(root))
    {
        if let Ok(table) = text.parse::<toml::Table>() {
            if let Some(hooks_table) = table.get("hooks").and_then(|h| h.as_table()) {
                if CODEX_HOOK_EVENTS
                    .iter()
                    .any(|(event, _)| toml_event_has_marker(hooks_table, event, CODEX_HOOK_MARKER))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Validates that every one of the 3 events carries an agentrec-marked
/// entry with the right shape (`PostToolUse`'s `matcher` == `"apply_patch"`),
/// in whichever config location currently carries our marker (hooks.json
/// checked first, matching install's own preference order). `Err` names
/// what's missing/wrong — `doctorcmd.rs` renders that as the failing
/// check's remedy text.
pub(crate) fn codex_hooks_shape(root: &Path) -> Result<(), String> {
    if let Ok(text) = agentrec_core::fsguard::read_regular_to_string(&codex_hooks_json_path(root)) {
        if let Ok(settings) = serde_json::from_str::<serde_json::Value>(&text) {
            if CODEX_HOOK_EVENTS
                .iter()
                .any(|(event, _)| event_has_marker_with(&settings, event, CODEX_HOOK_MARKER))
            {
                return codex_hooks_shape_json(&settings);
            }
        }
    }
    if let Ok(text) = agentrec_core::fsguard::read_regular_to_string(&codex_config_toml_path(root))
    {
        if let Ok(table) = text.parse::<toml::Table>() {
            if let Some(hooks_table) = table.get("hooks").and_then(|h| h.as_table()) {
                return codex_hooks_shape_toml(hooks_table);
            }
        }
    }
    Err("no Codex hook config found (neither hooks.json nor config.toml [hooks])".to_string())
}

fn codex_hooks_shape_json(settings: &serde_json::Value) -> Result<(), String> {
    for (event, matcher) in CODEX_HOOK_EVENTS {
        if !event_has_marker_with(settings, event, CODEX_HOOK_MARKER) {
            return Err(format!("{event} entry missing from hooks.json"));
        }
        if let Some(m) = matcher {
            let has_matcher = settings
                .get("hooks")
                .and_then(|h| h.get(event))
                .and_then(|s| s.as_array())
                .map(|arr| {
                    arr.iter()
                        .any(|e| e.get("matcher").and_then(|v| v.as_str()) == Some(*m))
                })
                .unwrap_or(false);
            if !has_matcher {
                return Err(format!("{event} entry missing matcher {m:?} in hooks.json"));
            }
        }
    }
    Ok(())
}

fn codex_hooks_shape_toml(hooks_table: &toml::Table) -> Result<(), String> {
    for (event, matcher) in CODEX_HOOK_EVENTS {
        if !toml_event_has_marker(hooks_table, event, CODEX_HOOK_MARKER) {
            return Err(format!("{event} entry missing from config.toml [hooks]"));
        }
        if let Some(m) = matcher {
            let has_matcher = hooks_table
                .get(*event)
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter().any(|e| {
                        e.as_table()
                            .and_then(|t| t.get("matcher"))
                            .and_then(|v| v.as_str())
                            == Some(*m)
                    })
                })
                .unwrap_or(false);
            if !has_matcher {
                return Err(format!(
                    "{event} entry missing matcher {m:?} in config.toml [hooks]"
                ));
            }
        }
    }
    Ok(())
}

// ============================================================================
// MCP server registration (Task E4; IMPLEMENTATION.md Q.2 for Claude Code,
// parent spec :582 for Codex — "Project `.codex/config.toml` owns the stdio
// MCP registration"). Both targets are REPO-LOCAL files under `root`, written
// by `init`, validated by `doctor`, removed by `uninstall`, with the same
// marker discipline the hook installers above use.
//
// Wire shapes, both verified rather than recalled:
// * Claude Code `.mcp.json`: `{"mcpServers": {"<name>": {"command":…,
//   "args":[…]}}}`.
// * Codex `[mcp_servers.<name>]` with `command` / `args` — measured live on
//   the pinned `codex-cli 0.146.0` by running `codex mcp add agentrec --
//   agentrec mcp` under a throwaway `CODEX_HOME` and reading the file it
//   wrote (exactly those three keys, in that shape).
//
// Codex's official MCP documentation now explicitly supports project-scoped
// `.codex/config.toml` in trusted projects and specifies
// `[mcp_servers.<name>]` with `command` / `args` for stdio servers:
// https://developers.openai.com/codex/mcp (verified 2026-09-06). This closes
// the earlier documentation gap where only user-global `codex mcp add` had
// been measured live. Project trust remains load-bearing: an untrusted Codex
// project ignores its `.codex/config.toml`, including this registration.
// ============================================================================

/// The registry key agentrec registers itself under in both hosts.
pub(crate) const MCP_SERVER_NAME: &str = "agentrec";
/// The command + argv registered. Bare `agentrec` (PATH-resolved by the host),
/// matching what `codex mcp add agentrec -- agentrec mcp` itself writes; NOT
/// the absolute `current_exe()` the service unit bakes, because a host config
/// is user-visible, commonly committed, and must survive a reinstall to a
/// different prefix — the opposite trade-off from `service_exec_path`.
const MCP_SERVER_COMMAND: &str = "agentrec";
const MCP_SERVER_ARGS: &[&str] = &["mcp"];

/// Whether a registry entry is one agentrec wrote.
///
/// **This predicate is the whole marker discipline for MCP registration, and
/// it is not the registry key.** Hook entries carry `agentrec hook` INSIDE the
/// command string; an MCP entry has no such slot — it is a bare program plus
/// argv, under a name the user could also have chosen. So identity is
/// structural: command file-stem `agentrec` AND `mcp` among its args. A
/// foreign server that merely occupies the `agentrec` key does NOT match, so
/// `init` refuses it untouched and `uninstall` leaves it alone. A key-only
/// predicate would silently eat that user's config, which is the failure this
/// function exists to prevent (pinned by
/// `foreign_entry_under_our_key_is_not_ours` and the round-trip tests).
fn mcp_entry_is_ours(command: Option<&str>, args: &[&str]) -> bool {
    let command_is_ours = command
        .and_then(|c| {
            Path::new(c)
                .file_stem()
                .map(|s| s == std::ffi::OsStr::new(MCP_SERVER_NAME))
        })
        .unwrap_or(false);
    command_is_ours && args.contains(&"mcp")
}

fn json_mcp_entry_is_ours(entry: &serde_json::Value) -> bool {
    let command = entry.get("command").and_then(|c| c.as_str());
    let args: Vec<&str> = entry
        .get("args")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    mcp_entry_is_ours(command, &args)
}

fn toml_mcp_entry_is_ours(entry: &toml::Value) -> bool {
    let command = entry.get("command").and_then(|c| c.as_str());
    let args: Vec<&str> = entry
        .get("args")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    mcp_entry_is_ours(command, &args)
}

/// What the `agentrec` key currently holds in one registry file.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum McpRegState {
    /// No entry under our key.
    Absent,
    /// Our entry (per [`mcp_entry_is_ours`]).
    Ours,
    /// Someone else's entry occupying our key — never overwritten, never
    /// removed.
    Foreign,
}

/// Result of an install attempt. Mirrors `CodexHooksOutcome`'s shape so both
/// registration paths read the same at the call site.
pub(crate) enum McpRegOutcome {
    Registered { changed: bool },
    Foreign,
}

pub(crate) fn mcp_json_path(root: &Path) -> std::path::PathBuf {
    root.join(".mcp.json")
}

/// Read-only inspection of `.mcp.json`. Shared by the real run, `--dry-run`
/// and `doctor` so none of the three can drift from the others (the same
/// precedent `codex_hooks_target`/`service_decision` set).
pub(crate) fn mcp_json_state(root: &Path) -> Result<McpRegState, String> {
    let path = mcp_json_path(root);
    let text = match agentrec_core::fsguard::read_regular_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(McpRegState::Absent),
        Err(e) => {
            return Err(format!(
                "cannot read existing {} ({e}); not touching it",
                path.display()
            ))
        }
    };
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        format!(
            "existing {} is not valid JSON ({e}); not touching it",
            path.display()
        )
    })?;
    Ok(
        match value.get("mcpServers").and_then(|s| s.get(MCP_SERVER_NAME)) {
            None => McpRegState::Absent,
            Some(entry) if json_mcp_entry_is_ours(entry) => McpRegState::Ours,
            Some(_) => McpRegState::Foreign,
        },
    )
}

/// Read-only inspection of the repo-local `.codex/config.toml`
/// `[mcp_servers.agentrec]` table.
pub(crate) fn codex_mcp_state(root: &Path) -> Result<McpRegState, String> {
    let path = codex_config_toml_path(root);
    let text = match agentrec_core::fsguard::read_regular_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(McpRegState::Absent),
        Err(e) => {
            return Err(format!(
                "cannot read existing {} ({e}); not touching it",
                path.display()
            ))
        }
    };
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        format!(
            "existing {} is not valid TOML ({e}); not touching it",
            path.display()
        )
    })?;
    Ok(
        match table
            .get("mcp_servers")
            .and_then(|s| s.as_table())
            .and_then(|s| s.get(MCP_SERVER_NAME))
        {
            None => McpRegState::Absent,
            Some(entry) if toml_mcp_entry_is_ours(entry) => McpRegState::Ours,
            Some(_) => McpRegState::Foreign,
        },
    )
}

/// Merge `mcpServers.agentrec` into `.mcp.json`. Foreign entry under our key →
/// no write at all, and no rewrite happens on any path where we add nothing.
///
/// When we DO write, every other key (and every other server) survives as a
/// PARSED VALUE, not as bytes: the file is re-serialized whole through
/// `serde_json`, whose `Map` is a `BTreeMap` here (no `preserve_order`
/// feature), so keys come back alphabetically ordered and
/// `to_string_pretty` normalizes the formatting. Same caveat as
/// [`install_codex_mcp`]; parsed-value identity is the strongest true claim
/// on this path and is what the round-trip test asserts.
pub(crate) fn install_mcp_json(root: &Path) -> Result<McpRegOutcome, String> {
    let path = mcp_json_path(root);
    match mcp_json_state(root)? {
        McpRegState::Foreign => return Ok(McpRegOutcome::Foreign),
        McpRegState::Ours => return Ok(McpRegOutcome::Registered { changed: false }),
        McpRegState::Absent => {}
    }
    let mut value: serde_json::Value = match agentrec_core::fsguard::read_regular_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            format!(
                "existing {} is not valid JSON ({e}); not touching it",
                path.display()
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => {
            return Err(format!(
                "cannot read existing {} ({e}); not touching it",
                path.display()
            ))
        }
    };
    let obj = value
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", path.display()))?;
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| format!("{} \"mcpServers\" is not a JSON object", path.display()))?;
    servers_obj.insert(
        MCP_SERVER_NAME.to_string(),
        serde_json::json!({"command": MCP_SERVER_COMMAND, "args": MCP_SERVER_ARGS}),
    );
    if path.exists() {
        let backup = path.with_extension("json.bak");
        fs::copy(&path, &backup).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(McpRegOutcome::Registered { changed: true })
}

/// Merge `[mcp_servers.agentrec]` into the repo-local `.codex/config.toml`.
///
/// Same `toml`-crate whole-file re-serialization caveat as
/// `install_codex_hooks_toml`: comments and key ordering are NOT preserved
/// (adding `toml_edit` would be a new dependency). Foreign servers therefore
/// survive as PARSED VALUES, not as bytes — the round-trip test asserts
/// parsed-value identity here, and the same on the `.mcp.json` path, which
/// re-serializes whole for its own reasons (see [`install_mcp_json`]). That
/// is the strongest true statement available on either path.
pub(crate) fn install_codex_mcp(root: &Path) -> Result<McpRegOutcome, String> {
    let path = codex_config_toml_path(root);
    match codex_mcp_state(root)? {
        McpRegState::Foreign => return Ok(McpRegOutcome::Foreign),
        McpRegState::Ours => return Ok(McpRegOutcome::Registered { changed: false }),
        McpRegState::Absent => {}
    }
    let text = match agentrec_core::fsguard::read_regular_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(format!(
                "cannot read existing {} ({e}); not touching it",
                path.display()
            ))
        }
    };
    let mut doc: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        format!(
            "existing {} is not valid TOML ({e}); not touching it",
            path.display()
        )
    })?;
    let servers_val = doc
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let servers = servers_val
        .as_table_mut()
        .ok_or_else(|| format!("{} \"mcp_servers\" is not a TOML table", path.display()))?;
    let mut entry = toml::Table::new();
    entry.insert(
        "command".to_string(),
        toml::Value::String(MCP_SERVER_COMMAND.to_string()),
    );
    entry.insert(
        "args".to_string(),
        toml::Value::Array(
            MCP_SERVER_ARGS
                .iter()
                .map(|a| toml::Value::String((*a).to_string()))
                .collect(),
        ),
    );
    servers.insert(MCP_SERVER_NAME.to_string(), toml::Value::Table(entry));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if path.exists() {
        let backup = path.with_extension("toml.bak");
        fs::copy(&path, &backup).map_err(|e| e.to_string())?;
    }
    let out = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    fs::write(&path, out).map_err(|e| e.to_string())?;
    Ok(McpRegOutcome::Registered { changed: true })
}

/// Printed when our key is occupied by someone else's server. Shared by the
/// real run and `--dry-run`.
fn mcp_foreign_line(path: &Path) -> String {
    format!(
        "skipped MCP registration: {} already declares a server named {:?} that is not \
         agentrec's own (`{} {}`) — left untouched; rename or remove it, then re-run \
         `agentrec init`",
        path.display(),
        MCP_SERVER_NAME,
        MCP_SERVER_COMMAND,
        MCP_SERVER_ARGS.join(" "),
    )
}

fn codex_target_label(target: CodexHooksTarget, root: &Path) -> String {
    match target {
        CodexHooksTarget::HooksJson => codex_hooks_json_path(root).display().to_string(),
        CodexHooksTarget::ConfigToml => codex_config_toml_path(root).display().to_string(),
        CodexHooksTarget::Refuse => unreachable!("Refuse has no install target label"),
    }
}

#[cfg(test)]
// Test code reads its own tempdir fixtures; no attacker-supplied FIFO can
// block these, so the fsguard wrappers buy nothing. Scoped to this module
// so production reads in this file stay lint-enforced (clippy.toml).
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn merge_is_idempotent_and_preserves_unrelated_hooks() {
        let existing = serde_json::json!({
            "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "other-tool" } ] } ] }
        });
        let (merged, changed) = merge_hook(existing, "Stop", None).unwrap();
        assert!(changed);
        let stops = merged["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stops.len(), 2); // other-tool preserved, ours appended
        let (again, changed2) = merge_hook(merged, "Stop", None).unwrap();
        assert!(!changed2);
        assert_eq!(again["hooks"]["Stop"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn merge_rejects_wrong_shapes() {
        assert!(merge_hook(serde_json::json!([]), "Stop", None).is_err());
        assert!(merge_hook(serde_json::json!({"hooks": []}), "Stop", None).is_err());
        assert!(merge_hook(serde_json::json!({"hooks": {"Stop": "x"}}), "Stop", None).is_err());
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

        let err = run(root, false, false, true, false, false).unwrap_err();
        assert!(
            err.contains("settings.local.json"),
            "error should name the file: {err}"
        );

        let after = fs::read(&settings_path).unwrap();
        assert_eq!(before, after, "unreadable settings file must be untouched");
    }

    #[cfg(unix)]
    fn mkfifo_at(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(
            unsafe { libc::mkfifo(c.as_ptr(), 0o600) },
            0,
            "fixture must actually create a fifo"
        );
    }

    // Same class as `init_aborts_untouched_on_unreadable_settings_file`, but
    // the failure mode is worse than an abort: a bare `read_to_string` on a
    // fifo BLOCKS until a writer appears, so `agentrec init` hung forever.
    // These two paths abort `run` outright (both are merge targets — treating
    // a refused read as "" would write back a file missing the user's own
    // content). The test terminating at all is the property.
    #[test]
    #[cfg(unix)]
    fn init_refuses_a_fifo_at_a_merge_target_instead_of_hanging() {
        for rel in [".claude/settings.local.json", ".gitignore"] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            fs::create_dir_all(root.join(".git")).unwrap();
            mkfifo_at(&root.join(rel));

            let err = run(root, false, false, true, false, false).unwrap_err();
            assert!(
                err.contains(rel.rsplit('/').next().unwrap()) && err.contains("not a regular file"),
                "{rel}: refusal must name the file: {err}"
            );
            use std::os::unix::fs::FileTypeExt;
            assert!(
                std::fs::symlink_metadata(root.join(rel))
                    .unwrap()
                    .file_type()
                    .is_fifo(),
                "{rel}: the refused path must be left exactly as found"
            );
        }
    }

    // The remaining config readers `init --codex` drives. `run` folds their
    // errors into a printed "skipped: {e}" action line rather than aborting,
    // so the refusal is asserted at the function that performs the read —
    // which is also where the hang used to be.
    #[test]
    #[cfg(unix)]
    fn codex_and_mcp_readers_refuse_a_fifo_instead_of_hanging() {
        let names_it = |e: &str, name: &str| {
            assert!(
                e.contains(name) && e.contains("not a regular file"),
                "refusal must name {name}: {e}"
            );
        };

        let tmp = tempfile::tempdir().unwrap();
        mkfifo_at(&mcp_json_path(tmp.path()));
        names_it(&mcp_json_state(tmp.path()).unwrap_err(), ".mcp.json");
        let Err(e) = install_mcp_json(tmp.path()) else {
            panic!("install_mcp_json must refuse a fifo .mcp.json");
        };
        names_it(&e, ".mcp.json");

        let tmp = tempfile::tempdir().unwrap();
        mkfifo_at(&codex_config_toml_path(tmp.path()));
        names_it(&codex_hooks_target(tmp.path()).unwrap_err(), "config.toml");
        names_it(&codex_mcp_state(tmp.path()).unwrap_err(), "config.toml");
        let Err(e) = install_codex_mcp(tmp.path()) else {
            panic!("install_codex_mcp must refuse a fifo config.toml");
        };
        names_it(&e, "config.toml");
        names_it(
            &install_codex_hooks_toml(tmp.path()).unwrap_err(),
            "config.toml",
        );
        // Read-only detection: a refused read cannot hang and cannot clobber,
        // so it falls through to "not installed" exactly as an unreadable
        // file already did.
        assert!(!codex_hooks_installed(tmp.path()));

        let tmp = tempfile::tempdir().unwrap();
        mkfifo_at(&codex_hooks_json_path(tmp.path()));
        names_it(
            &install_codex_hooks_json(tmp.path()).unwrap_err(),
            "hooks.json",
        );
    }

    // The allow half. A config file behind a symlink (dotfiles repo) is an
    // ordinary setup, and `is_nonregular` resolves the link deliberately —
    // an over-refusal here would make `init` abort on, or silently overwrite,
    // a perfectly readable file. Asserts CONTENT, not just exit status: the
    // user's own hook must survive the merge that reads through the link.
    #[test]
    #[cfg(unix)]
    fn init_merges_through_a_symlinked_settings_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join(".claude")).unwrap();
        let real = tmp.path().join("dotfiles-settings.json");
        fs::write(
            &real,
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": { "Stop": [ { "hooks": [
                    { "type": "command", "command": "other-tool" }
                ] } ] }
            }))
            .unwrap(),
        )
        .unwrap();
        std::os::unix::fs::symlink(&real, root.join(".claude/settings.local.json")).unwrap();

        run(root, false, false, true, false, false).unwrap();

        let after: serde_json::Value = serde_json::from_str(&fs::read_to_string(&real).unwrap())
            .expect("the merge must land on the symlink target");
        let stop = after["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "the user's own Stop hook must survive");
        assert!(stop
            .iter()
            .any(|e| e["hooks"][0]["command"].as_str() == Some("other-tool")));
        assert!(stop.iter().any(|e| e["hooks"][0]["command"]
            .as_str()
            .is_some_and(|c| c.contains(HOOK_MARKER))));
    }

    #[test]
    fn init_scaffolds_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        run(root, true, false, true, false, false).unwrap();
        run(root, true, false, true, false, false).unwrap(); // second run: no error, no duplicates
        assert!(root.join(".agentrec/config.toml").exists());
        assert!(root.join(".agentrec/objects").exists());
        let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert_eq!(gitignore.matches(".agentrec/").count(), 1);
    }

    #[test]
    fn init_without_git_skips_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        run(tmp.path(), true, false, true, false, false).unwrap();
        assert!(!tmp.path().join(".gitignore").exists());
    }

    // AC-Y+5: `--dry-run` must touch nothing on disk at all.
    #[test]
    fn dry_run_touches_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let before = walk(root);
        run(root, true, false, true, false, true).unwrap();
        let after = walk(root);
        assert_eq!(before, after, "dry-run must not create or modify anything");
    }

    // Same as `dry_run_touches_nothing` but with `--codex` — every previous
    // dry-run test passed `codex: false`, so the branch that actually calls
    // `codex_hooks_target` (which reads `.codex/hooks.json` /
    // `.codex/config.toml`) and could plausibly reach
    // `install_codex_hooks_json`'s `fs::create_dir_all` had zero coverage.
    // Asserted twice over: no `.codex/` directory at all afterward, AND the
    // generic whole-tree walk stays empty (belt-and-braces against a write
    // anywhere else this test didn't anticipate).
    #[test]
    fn dry_run_with_codex_touches_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let before = walk(root);
        run(root, true, true, true, false, true).unwrap();
        let after = walk(root);
        assert_eq!(
            before, after,
            "dry-run --codex must not create or modify anything"
        );
        assert!(
            !root.join(".codex").exists(),
            "dry-run --codex must not create .codex/"
        );
    }

    // Dry-run's PRINTED codex decision (install vs refuse) is asserted at
    // the integration level instead of here — `codex_init_dry_run_*` in
    // `cli/tests/integration.rs` drive the real binary and read its real
    // stdout. An in-process version needs process-global stdout fd
    // redirection, which is unsound under cargo's threaded test runner:
    // it hijacks every concurrently-running test's output for the
    // duration. The filesystem half of dry-run's contract stays here
    // (`dry_run_with_codex_touches_nothing`) because it needs no capture.

    // AC-Y+3: re-running `init --no-service` (byte-for-byte no-op path) must
    // not duplicate hooks or scaffold files; exercised end-to-end here since
    // `merge_is_idempotent_and_preserves_unrelated_hooks` only covers the
    // hook-merge helper in isolation.
    #[test]
    fn init_no_service_rerun_is_byte_for_byte_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        run(root, false, false, true, false, false).unwrap();
        let settings_path = root.join(".claude/settings.local.json");
        let first = fs::read_to_string(&settings_path).unwrap();
        let config_first = fs::read_to_string(root.join(".agentrec/config.toml")).unwrap();

        run(root, false, false, true, false, false).unwrap();
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
        run(root, true, false, true, false, false).unwrap();

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

    // REGRESSION, and not a hypothetical: this suite leaked a real launchd
    // unit (`com.agentrec.c0bf764acce7` → `…/T/.tmpdW8v92`, 2026-07-31 10:57)
    // because `temp_prefixes` read `$TMPDIR` to recognize macOS's per-user
    // temp dir, and the read can miss — `set_var` on another test thread
    // races `var()`, and launchd/cron contexts have no `$TMPDIR` at all. When
    // it missed, `/private/var/folders/…` matched no prefix and `init`
    // installed a permanent KeepAlive unit for a directory about to be
    // deleted: the exact defect D46 exists to stop, reproduced by D46's own
    // tests. `/var/folders` is now a static prefix, so this holds with no
    // environment at all.
    //
    // Deliberately does NOT mutate `$TMPDIR` to prove it — that would add
    // another env-mutating test to the binary whose races caused this.
    // Asserts the property directly instead: the macOS per-user temp shape is
    // recognized without consulting the environment.
    #[test]
    fn macos_per_user_temp_is_recognized_without_reading_tmpdir() {
        let prefixes = temp_prefixes();
        let var_folders = std::path::Path::new("/var/folders")
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from("/var/folders"));
        assert!(
            prefixes.contains(&var_folders),
            "the macOS per-user temp root must be a STATIC prefix, not one \
             reachable only through $TMPDIR: {prefixes:?}"
        );
        // And a concrete path of that shape must decide SkipTemp.
        let shaped = var_folders.join("m5/somethinglong/T/.tmpABCDEF");
        assert!(
            matches!(
                service_decision(&shaped, false, false),
                ServiceDecision::SkipTemp(_)
            ),
            "{} must be recognized as temp",
            shaped.display()
        );
    }

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
        run(root, false, false, false, false, false).unwrap();

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

    // ---- C3: Codex hooks --------------------------------------------------

    /// Pins the EXACT serialized entry `codex_hook_json_entry` produces,
    /// field-for-field — the test that actually reds if `command`,
    /// `timeout`, or `matcher` ever drifts. A marker-presence-only
    /// idempotency test would pass vacuously even if this builder became
    /// nondeterministic, because a marker hit skips the builder entirely on
    /// re-install; this test calls the builder directly regardless of any
    /// marker state.
    #[test]
    fn codex_hook_json_entry_is_pinned() {
        assert_eq!(
            codex_hook_json_entry(None),
            serde_json::json!({
                "hooks": [ { "type": "command", "command": "agentrec hook codex", "timeout": 10 } ]
            })
        );
        assert_eq!(
            codex_hook_json_entry(Some("apply_patch")),
            serde_json::json!({
                "matcher": "apply_patch",
                "hooks": [ { "type": "command", "command": "agentrec hook codex", "timeout": 10 } ]
            })
        );
    }

    #[test]
    fn codex_hook_toml_entry_is_pinned() {
        // Built field-by-field (not via a TOML literal macro — the `toml`
        // crate's macro feature isn't a dependency this repo carries, and
        // adding one would move Cargo.lock, which the release build's
        // `--locked` check must not tolerate) so this test exercises the
        // exact same `toml::Value` construction independently of
        // `codex_hook_toml_entry` itself.
        let mut hook = toml::Table::new();
        hook.insert("type".into(), toml::Value::String("command".into()));
        hook.insert(
            "command".into(),
            toml::Value::String("agentrec hook codex".into()),
        );
        hook.insert("timeout".into(), toml::Value::Integer(10));

        let mut want_no_matcher = toml::Table::new();
        want_no_matcher.insert(
            "hooks".into(),
            toml::Value::Array(vec![toml::Value::Table(hook.clone())]),
        );
        assert_eq!(
            codex_hook_toml_entry(None),
            toml::Value::Table(want_no_matcher)
        );

        let mut want_matcher = toml::Table::new();
        want_matcher.insert("matcher".into(), toml::Value::String("apply_patch".into()));
        want_matcher.insert(
            "hooks".into(),
            toml::Value::Array(vec![toml::Value::Table(hook)]),
        );
        assert_eq!(
            codex_hook_toml_entry(Some("apply_patch")),
            toml::Value::Table(want_matcher)
        );
    }

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn codex_install_none_creates_hooks_json_with_all_three_events() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let outcome = install_codex_hooks(root).unwrap();
        assert!(matches!(
            outcome,
            CodexHooksOutcome::Installed {
                target: CodexHooksTarget::HooksJson,
                changed: true,
            }
        ));

        let settings = read_json(&codex_hooks_json_path(root));
        for (event, matcher) in CODEX_HOOK_EVENTS {
            assert!(
                event_has_marker_with(&settings, event, CODEX_HOOK_MARKER),
                "{event} missing agentrec marker: {settings}"
            );
            if let Some(m) = matcher {
                let arr = settings["hooks"][event].as_array().unwrap();
                assert_eq!(arr[0]["matcher"].as_str(), Some(*m));
            }
        }
        assert!(!codex_config_toml_path(root).exists());
    }

    #[test]
    fn codex_install_hooks_json_only_preserves_foreign_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::write(
            codex_hooks_json_path(root),
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": {
                    "Stop": [ { "hooks": [ { "type": "command", "command": "other-tool" } ] } ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let outcome = install_codex_hooks(root).unwrap();
        assert!(matches!(
            outcome,
            CodexHooksOutcome::Installed {
                target: CodexHooksTarget::HooksJson,
                changed: true,
            }
        ));

        let settings = read_json(&codex_hooks_json_path(root));
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "foreign entry preserved alongside ours");
        assert!(stop
            .iter()
            .any(|e| e["hooks"][0]["command"] == "other-tool"));
        assert!(event_has_marker_with(&settings, "Stop", CODEX_HOOK_MARKER));
    }

    #[test]
    fn codex_install_config_toml_only_merges_inline_hooks_table() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::write(codex_config_toml_path(root), "model = \"o3\"\n\n[hooks]\n").unwrap();

        let outcome = install_codex_hooks(root).unwrap();
        assert!(matches!(
            outcome,
            CodexHooksOutcome::Installed {
                target: CodexHooksTarget::ConfigToml,
                changed: true,
            }
        ));
        assert!(
            !codex_hooks_json_path(root).is_file(),
            "must not also create hooks.json"
        );

        let text = fs::read_to_string(codex_config_toml_path(root)).unwrap();
        let table: toml::Table = text.parse().unwrap();
        assert_eq!(
            table.get("model").and_then(|v| v.as_str()),
            Some("o3"),
            "unrelated top-level key must survive"
        );
        let hooks_table = table["hooks"].as_table().unwrap();
        for (event, _) in CODEX_HOOK_EVENTS {
            assert!(
                toml_event_has_marker(hooks_table, event, CODEX_HOOK_MARKER),
                "{event} missing from merged config.toml: {text}"
            );
        }
    }

    // AC: exactly one of hooks.json / inline [hooks] present -> merge there.
    // Neither present defaults to hooks.json (covered above). This case:
    // config.toml exists WITHOUT a [hooks] table at all -> still defaults to
    // hooks.json (a config.toml with unrelated keys must not count as
    // "inline hooks present").
    #[test]
    fn codex_install_config_toml_without_hooks_table_still_defaults_to_hooks_json() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::write(codex_config_toml_path(root), "model = \"o3\"\n").unwrap();

        let outcome = install_codex_hooks(root).unwrap();
        assert!(matches!(
            outcome,
            CodexHooksOutcome::Installed {
                target: CodexHooksTarget::HooksJson,
                ..
            }
        ));
        assert!(codex_hooks_json_path(root).is_file());
        // config.toml's unrelated content must be untouched — we never even
        // opened it for writing on this path.
        let text = fs::read_to_string(codex_config_toml_path(root)).unwrap();
        assert_eq!(text, "model = \"o3\"\n");
    }

    // AC: both hooks.json AND inline [hooks] present -> refuse untouched.
    // Neither file's bytes may change, and no .bak may appear.
    #[test]
    fn codex_install_both_present_refuses_and_leaves_both_files_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".codex")).unwrap();
        let hooks_json_before = serde_json::to_string_pretty(&serde_json::json!({
            "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "other-tool" } ] } ] }
        }))
        .unwrap();
        fs::write(codex_hooks_json_path(root), &hooks_json_before).unwrap();
        let config_toml_before = "model = \"o3\"\n\n[hooks]\n";
        fs::write(codex_config_toml_path(root), config_toml_before).unwrap();

        let outcome = install_codex_hooks(root).unwrap();
        assert!(matches!(outcome, CodexHooksOutcome::Refused));

        assert_eq!(
            fs::read_to_string(codex_hooks_json_path(root)).unwrap(),
            hooks_json_before,
            "hooks.json must be byte-identical after a refuse"
        );
        assert_eq!(
            fs::read_to_string(codex_config_toml_path(root)).unwrap(),
            config_toml_before,
            "config.toml must be byte-identical after a refuse"
        );
        assert!(
            !codex_hooks_json_path(root)
                .with_extension("json.bak")
                .exists(),
            "a refuse must not even take a backup"
        );
        assert!(
            !codex_config_toml_path(root)
                .with_extension("toml.bak")
                .exists(),
            "a refuse must not even take a backup"
        );
    }

    // AC: idempotent reinstall — every agentrec hook ENTRY (not just the
    // command string) is byte-identical across two installs. Extracts the
    // whole entry `serde_json::Value` both times and compares it directly —
    // a changed timeout or statusMessage would fail this exactly as a
    // changed command would.
    #[test]
    fn codex_install_idempotent_reinstall_entries_are_identical() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let first = install_codex_hooks(root).unwrap();
        assert!(matches!(
            first,
            CodexHooksOutcome::Installed { changed: true, .. }
        ));
        let first_bytes = fs::read_to_string(codex_hooks_json_path(root)).unwrap();
        let first_settings = read_json(&codex_hooks_json_path(root));

        let second = install_codex_hooks(root).unwrap();
        assert!(
            matches!(second, CodexHooksOutcome::Installed { changed: false, .. }),
            "re-install must be a no-op: {:?}",
            match second {
                CodexHooksOutcome::Installed { changed, .. } => changed,
                CodexHooksOutcome::Refused => panic!("must not refuse on reinstall"),
            }
        );
        let second_bytes = fs::read_to_string(codex_hooks_json_path(root)).unwrap();
        let second_settings = read_json(&codex_hooks_json_path(root));

        assert_eq!(first_bytes, second_bytes, "whole file byte-identical");
        for (event, _) in CODEX_HOOK_EVENTS {
            assert_eq!(
                first_settings["hooks"][event], second_settings["hooks"][event],
                "whole entry for {event} must be identical across reinstalls, not just its command"
            );
        }
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

    // ---- MCP registration (E4) --------------------------------------------

    /// The predicate that decides whether `init` overwrites and `uninstall`
    /// removes. A foreign server sitting on the `agentrec` KEY must not match
    /// — a key-only predicate passes every other test in this file and eats
    /// that user's config.
    #[test]
    fn foreign_entry_under_our_key_is_not_ours() {
        assert!(mcp_entry_is_ours(Some("agentrec"), &["mcp"]));
        assert!(mcp_entry_is_ours(
            Some("/opt/homebrew/bin/agentrec"),
            &["mcp"]
        ));
        // Same key, someone else's server.
        assert!(!mcp_entry_is_ours(Some("other-tool"), &["mcp"]));
        // Our binary, but not the MCP subcommand (e.g. a user's own wrapper).
        assert!(!mcp_entry_is_ours(Some("agentrec"), &["record"]));
        assert!(!mcp_entry_is_ours(None, &["mcp"]));
    }

    #[test]
    fn install_mcp_json_writes_entry_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(mcp_json_state(root).unwrap(), McpRegState::Absent);
        assert!(matches!(
            install_mcp_json(root).unwrap(),
            McpRegOutcome::Registered { changed: true }
        ));
        assert_eq!(mcp_json_state(root).unwrap(), McpRegState::Ours);
        let written = fs::read_to_string(mcp_json_path(root)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(value["mcpServers"]["agentrec"]["command"], "agentrec");
        assert_eq!(value["mcpServers"]["agentrec"]["args"][0], "mcp");
        // Re-run: no change, byte-identical file.
        assert!(matches!(
            install_mcp_json(root).unwrap(),
            McpRegOutcome::Registered { changed: false }
        ));
        assert_eq!(fs::read_to_string(mcp_json_path(root)).unwrap(), written);
    }

    #[test]
    fn install_mcp_json_preserves_foreign_servers() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let foreign = serde_json::json!({"command": "other-tool", "args": ["serve", "--x"]});
        fs::write(
            mcp_json_path(root),
            serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {"other": foreign.clone()},
                "unrelatedKey": {"kept": true},
            }))
            .unwrap(),
        )
        .unwrap();
        install_mcp_json(root).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(mcp_json_path(root)).unwrap()).unwrap();
        assert_eq!(value["mcpServers"]["other"], foreign);
        assert_eq!(value["unrelatedKey"]["kept"], true);
        assert_eq!(value["mcpServers"]["agentrec"]["command"], "agentrec");
    }

    /// A foreign server occupying OUR key: `init` writes nothing at all and
    /// the file is byte-identical afterwards.
    #[test]
    fn install_mcp_json_refuses_foreign_entry_under_our_key() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": {"agentrec": {"command": "other-tool", "args": ["mcp"]}}
        }))
        .unwrap();
        fs::write(mcp_json_path(root), &text).unwrap();
        assert_eq!(mcp_json_state(root).unwrap(), McpRegState::Foreign);
        assert!(matches!(
            install_mcp_json(root).unwrap(),
            McpRegOutcome::Foreign
        ));
        assert_eq!(
            fs::read_to_string(mcp_json_path(root)).unwrap(),
            text,
            "a foreign entry under our key must be left byte-identical"
        );
    }

    #[test]
    fn install_codex_mcp_writes_table_and_preserves_foreign_servers() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::write(
            codex_config_toml_path(root),
            "model = \"gpt-5\"\n\n[mcp_servers.other]\ncommand = \"other-tool\"\nargs = [\"serve\"]\n",
        )
        .unwrap();
        assert_eq!(codex_mcp_state(root).unwrap(), McpRegState::Absent);
        assert!(matches!(
            install_codex_mcp(root).unwrap(),
            McpRegOutcome::Registered { changed: true }
        ));
        let doc: toml::Table = fs::read_to_string(codex_config_toml_path(root))
            .unwrap()
            .parse()
            .unwrap();
        let servers = doc["mcp_servers"].as_table().unwrap();
        assert_eq!(servers["agentrec"]["command"].as_str(), Some("agentrec"));
        assert_eq!(servers["agentrec"]["args"][0].as_str(), Some("mcp"));
        // The foreign server survives as a parsed value (the `toml` crate
        // round-trip reformats the whole file — documented on
        // `install_codex_mcp`).
        assert_eq!(servers["other"]["command"].as_str(), Some("other-tool"));
        assert_eq!(doc["model"].as_str(), Some("gpt-5"));
        assert!(matches!(
            install_codex_mcp(root).unwrap(),
            McpRegOutcome::Registered { changed: false }
        ));
    }

    /// An MCP registration alone must NOT flip the hook-target decision: a
    /// `config.toml` carrying only `[mcp_servers]` has no `[hooks]` table, so
    /// the next `init --codex` still merges hooks into `hooks.json` and never
    /// enters the dual-representation refusal because of our own write.
    #[test]
    fn mcp_registration_alone_does_not_change_the_codex_hook_target() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(
            codex_hooks_target(root).unwrap(),
            CodexHooksTarget::HooksJson
        );
        install_codex_mcp(root).unwrap();
        assert!(codex_config_toml_path(root).is_file());
        assert_eq!(
            codex_hooks_target(root).unwrap(),
            CodexHooksTarget::HooksJson,
            "an [mcp_servers]-only config.toml is not a hook representation"
        );
    }

    #[test]
    fn install_codex_mcp_refuses_foreign_entry_under_our_key() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".codex")).unwrap();
        let text = "[mcp_servers.agentrec]\ncommand = \"other-tool\"\nargs = [\"mcp\"]\n";
        fs::write(codex_config_toml_path(root), text).unwrap();
        assert_eq!(codex_mcp_state(root).unwrap(), McpRegState::Foreign);
        assert!(matches!(
            install_codex_mcp(root).unwrap(),
            McpRegOutcome::Foreign
        ));
        assert_eq!(
            fs::read_to_string(codex_config_toml_path(root)).unwrap(),
            text
        );
    }
}
