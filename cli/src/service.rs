//! Per-repo service unit (macOS launchd / Linux systemd `--user`) generation
//! and (un)load, for `agentrec init` / `agentrec uninstall` (AC Y+3/Y+4). The
//! unit runs `<agentrec> record --root <repo>` so the daemon survives reboots
//! and restarts on crash (`KeepAlive true` / `Restart=always`).
//!
//! `slug`/`launchd_plist`/`systemd_unit`/`unit_path`/`service_dir` are pure —
//! no filesystem or process access — and are unit-tested directly below.
//! `parse_unit_root`/`scan_units` (D46, the discovery half) touch the
//! filesystem but spawn NOTHING, so unlike the install path they are fully
//! covered by the automated suite. `install`/`uninstall` do real (best-effort,
//! failure-tolerant) I/O, including spawning `launchctl`/`systemctl`; per the
//! hermetic-tests requirement they are exercised ONLY through manual
//! verification and `agentrec init/uninstall --no-service` in the CLI test
//! suite, never called from an automated test.

use agentrec_core::store::hash_bytes;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Canonicalize `root` so a relative `--root` never gets baked verbatim into
/// the service unit (`ExecStart`/`ProgramArguments`) and so two relative
/// spellings of the same repo (`.` vs the absolute path) yield one slug, not
/// two competing service units. Falls back to the given path unchanged if
/// canonicalization fails (e.g. the path doesn't exist yet) — install/
/// uninstall must still degrade gracefully rather than error out.
fn resolve_root(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

/// Stable short slug for a repo root: first 12 hex chars of sha256(abs
/// path). Used in both the launchd label and the unit filename so one repo's
/// service never collides with another's.
pub fn slug(root: &Path) -> String {
    let hash = hash_bytes(root.to_string_lossy().as_bytes());
    let hex = hash.strip_prefix("sha256:").unwrap_or(&hash);
    hex.chars().take(12).collect()
}

/// D5: XML-escape a value before it's interpolated into a `<string>…</string>`
/// element. An unescaped `&`, `<`, `>`, `"`, or `'` in the repo path (an `&`
/// is not exotic — plenty of real directory names have one) produces an
/// invalid plist; `launchctl load` then fails with no useful diagnostic while
/// `init` had already reported success.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// macOS launchd plist: runs `record --root <root>` at load and on crash,
/// but ONLY while the root still exists.
///
/// `KeepAlive` is a dict carrying `PathState`, not a bare `<true/>`. A bare
/// `KeepAlive` is what turned a stale path snapshot into permanent noise: the
/// unit records the root at `init` time, nothing re-validates it at load
/// time, and when the root is deleted launchd respawns the doomed job forever
/// (39 such units accumulated on the author's machine by 2026-07-31).
/// `PathState` makes the precondition declarative, so the init system —
/// which is the only thing still running once a path goes stale — enforces
/// it.
///
/// **Only the ROOT is listed, and that is a measured constraint, not an
/// omission.** A probed two-key `PathState` with one path absent still
/// produced 3 spawns in 30s (macOS, 2026-07-31), so a co-listed path that
/// still exists defeats the gate: `{root, exec}` would keep the job alive
/// whenever EITHER exists, making the exec gate inert in exactly the case
/// that motivates it (root present, binary upgraded away).
///
/// Precise about the mechanism, because the gate round caught this doc
/// overstating it: the man page's "launchd ORs them" is stated at the
/// `KeepAlive`-dict level, and the probe does not uniquely distinguish
/// "`PathState` ORs its entries" from "an absent path is disregarded while a
/// present co-key is satisfied". Both hypotheses give the same consequence
/// for this decision, which is why root-only is right either way — but the
/// OR label itself is inference, not measurement. No AND is reachable in
/// either reading: `false` values invert individual conditions, they cannot
/// negate whatever combines them. A vanished exec is therefore DETECTED
/// (`doctorcmd::check_orphan_services`), not prevented.
///
/// Measured with a missing root, macOS 2026-07-31: `RunAtLoad` fires once
/// and the job then reads `runs = 1`, `state = not running` — versus
/// `state = spawn scheduled` for a job launchd is still respawning, which is
/// what makes the two distinguishable. Read as "no respawn observed in that
/// window", not "every restart blocked forever": it is ONE load cycle whose
/// duration went unrecorded.
///
/// **F24: `StandardOutPath`/`StandardErrorPath` both point at
/// `<root>/.agentrec/daemon.log`.** Without them a launchd job's stdio goes to
/// `/dev/null`, which discarded the daemon's whole diagnostic channel on
/// macOS — including the single stderr line that is the only record eviction
/// deleted snapshot blobs (`daemon.rs`'s eviction tick has no persistent
/// counter). Linux needs no counterpart: systemd defaults
/// `StandardError=journal`, so this stays macOS-only by design.
///
/// The destination is inside the recorded root on purpose, so a repo's
/// diagnostics travel with the repo and `uninstall`-ing one root cannot
/// orphan another's log. It does NOT feed the watcher back into itself:
/// `daemon::classify` returns `Class::Ignore` for any path with a
/// `.agentrec` component (`daemon.rs`, the built-in denylist arm), and
/// `daemon::prune_git_and_agentrec` prunes the directory out of both
/// `IgnoreSet::build`'s and `Recorder::scan`'s one-shot walks — so writes here
/// open no turns and land in no `known` set. Both were read in source, not
/// assumed.
///
/// Two consequences, disclosed rather than fixed: nothing rotates this file
/// (no rotation code exists in this repo, and launchd does not rotate a
/// `StandardErrorPath`), and it is invisible to the store budget —
/// `BlobStore::total_bytes` walks `objects/` only, and `daemon.log` is a
/// sibling of that directory.
pub fn launchd_plist(exec: &Path, root: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
\t<key>Label</key>\n\
\t<string>com.agentrec.{slug}</string>\n\
\t<key>ProgramArguments</key>\n\
\t<array>\n\
\t\t<string>{exec}</string>\n\
\t\t<string>record</string>\n\
\t\t<string>--root</string>\n\
\t\t<string>{root}</string>\n\
\t</array>\n\
\t<key>RunAtLoad</key>\n\
\t<true/>\n\
\t<key>KeepAlive</key>\n\
\t<dict>\n\
\t\t<key>PathState</key>\n\
\t\t<dict>\n\
\t\t\t<key>{root}</key>\n\
\t\t\t<true/>\n\
\t\t</dict>\n\
\t</dict>\n\
\t<key>StandardOutPath</key>\n\
\t<string>{log}</string>\n\
\t<key>StandardErrorPath</key>\n\
\t<string>{log}</string>\n\
</dict>\n\
</plist>\n",
        slug = slug(root),
        exec = xml_escape(&exec.display().to_string()),
        root = xml_escape(&root.display().to_string()),
        log = xml_escape(&daemon_log_path(root).display().to_string()),
    )
}

/// Where the launchd unit routes the daemon's stdout/stderr (F24). Derived
/// through `crate::agentrec_dir` — the same helper `daemon.rs`/`main.rs` use
/// for `log.jsonl`/`signal.jsonl`/`objects/` — so the log cannot drift away
/// from the repo's `.agentrec` layout. Pure: no filesystem access, and the
/// `root` reaching it from `install` has already been through `resolve_root`.
fn daemon_log_path(root: &Path) -> PathBuf {
    crate::agentrec_dir(root).join("daemon.log")
}

/// D5: quote+escape a value for embedding as one `ExecStart=` command-line
/// token. Systemd's unit-file argument syntax (not a shell): a literal `%`
/// always starts a specifier expansion (`%h`, `%n`, …) unless doubled, so an
/// unescaped `%` in a path silently mangles it into something else entirely;
/// a value containing whitespace splits into extra argv entries unless
/// double-quoted, with embedded `"`/`\` backslash-escaped inside the quotes.
/// Plain values (the overwhelmingly common case — no space/quote/backslash)
/// are left byte-identical, only `%` doubled, so ordinary paths don't grow a
/// cosmetic pair of quotes they never needed.
fn systemd_escape(s: &str) -> String {
    let percent_escaped = s.replace('%', "%%");
    let needs_quoting = s
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '"' | '\\'));
    if !needs_quoting {
        return percent_escaped;
    }
    let mut out = String::with_capacity(percent_escaped.len() + 2);
    out.push('"');
    for c in percent_escaped.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Escape a path for a systemd setting that takes ONE value rather than a
/// command line (`ConditionPathIsDirectory=`, …). Only `%` needs doubling: a
/// literal `%` would otherwise start a specifier expansion. Deliberately does
/// NOT reuse `systemd_escape`, whose whitespace quoting exists to stop a path
/// splitting into extra `ExecStart=` argv entries — there is no argv to split
/// here, and a quoted value would likely be taken with its quotes.
fn systemd_value_escape(s: &str) -> String {
    s.replace('%', "%%")
}

/// Linux systemd user unit: same exec; `Restart=always` mirrors `KeepAlive`.
///
/// `ConditionPathIsDirectory` is systemd's counterpart to launchd's
/// `PathState` — a start job whose condition is false is skipped, leaving the
/// unit inactive rather than failed, so a deleted root stops producing restart
/// churn. Kept to the ROOT alone for parity with the launchd side, where
/// listing the exec too was measured to be actively wrong (see
/// `launchd_plist`); the exec is DETECTED by `doctor`, not gated here.
///
/// **Honesty note:** unlike the launchd behavior above, this one is NOT
/// measured. It was written on a macOS host with no systemd to probe, so the
/// generator is unit-tested but the runtime effect rests on documentation and
/// carries a VERIFY-LEDGER row. Do not upgrade this to a proven claim without
/// running it on Linux.
pub fn systemd_unit(exec: &Path, root: &Path) -> String {
    format!(
        "[Unit]\n\
Description=agentrec recorder for {root}\n\
ConditionPathIsDirectory={root_cond}\n\
\n\
[Service]\n\
ExecStart={exec} record --root {root_q}\n\
Restart=always\n\
\n\
[Install]\n\
WantedBy=default.target\n",
        root = root.display(),
        exec = systemd_escape(&exec.display().to_string()),
        root_q = systemd_escape(&root.display().to_string()),
        root_cond = systemd_value_escape(&root.display().to_string()),
    )
}

/// XDG Base Directory config home: `$XDG_CONFIG_HOME` if set to a non-empty
/// ABSOLUTE value, else `$HOME/.config` (the XDG basedir spec's documented
/// fallback). Per the spec, an empty value is treated as unset, and a relative
/// value "should be considered invalid and ignored" — a relative unit path
/// would otherwise be resolved against the CWD of whichever process ran
/// `install`/`uninstall`. Linux-only call site — launchd's
/// `~/Library/LaunchAgents` is not an XDG path and must never route through
/// this helper.
fn config_home() -> Result<PathBuf, String> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() && Path::new(&xdg).is_absolute() {
            return Ok(PathBuf::from(xdg));
        }
    }
    let home = std::env::var("HOME").map_err(|_| {
        "HOME environment variable is not set — cannot locate the service directory".to_string()
    })?;
    Ok(PathBuf::from(home).join(".config"))
}

/// Per-OS directory holding installed unit files. A bare env read (no
/// filesystem access), so it errs rather than panics when `HOME` is unset, and
/// stays hermetically unit-testable. macOS always uses
/// `$HOME/Library/LaunchAgents` (not an XDG path); Linux resolves the systemd
/// user-unit directory via `config_home` (`$XDG_CONFIG_HOME`, falling back to
/// `$HOME/.config`) — the single resolver `install`/`uninstall`/`unit_path`
/// and `doctor`'s orphan scan all go through.
pub fn service_dir() -> Result<PathBuf, String> {
    // D46: debug-only seam so `doctor`'s orphan scan can be driven against a
    // fixture directory from the integration suite instead of the developer's
    // real `~/Library/LaunchAgents`. `#[cfg(debug_assertions)]` keeps it out
    // of release binaries — verified by the release-`strings` check.
    #[cfg(debug_assertions)]
    if let Ok(dir) = std::env::var("AGENTREC_TEST_SERVICE_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    if cfg!(target_os = "macos") {
        let home = std::env::var("HOME").map_err(|_| {
            "HOME environment variable is not set — cannot locate the service directory".to_string()
        })?;
        Ok(PathBuf::from(home).join("Library").join("LaunchAgents"))
    } else {
        Ok(config_home()?.join("systemd").join("user"))
    }
}

/// Per-OS unit file path for one repo root.
pub fn unit_path(root: &Path) -> Result<PathBuf, String> {
    let s = slug(root);
    let name = if cfg!(target_os = "macos") {
        format!("com.agentrec.{s}.plist")
    } else {
        format!("agentrec-{s}.service")
    };
    Ok(service_dir()?.join(name))
}

// ---- D46: installed-unit discovery ------------------------------------------
//
// `slug()` is a ONE-WAY hash of the repo path, so the set of installed units
// cannot be mapped back to roots by inverting filenames — the only way to learn
// which root a unit records is to parse the unit's own contents. Everything
// below is pure text/filesystem reading: no `launchctl`/`systemctl` process is
// spawned, so unlike `install`/`uninstall` this code IS exercised by the
// automated suite.

/// What an installed unit's recorded `--root` currently looks like on disk.
/// Three disjoint states — `Unparseable` is deliberately NOT folded into
/// `VanishedRoot`: a unit we failed to read tells us nothing about its root,
/// and reporting it as an orphan would be a fabricated finding.
#[derive(Debug, PartialEq, Eq)]
pub enum UnitState {
    /// Recorded root parsed and the directory is present.
    Live(PathBuf),
    /// Recorded root parsed and the directory is NOT present. Note this is
    /// exactly what an unmounted volume or a detached network mount also looks
    /// like — "vanished" is an observation, not a verdict that the unit is
    /// abandoned.
    VanishedRoot(PathBuf),
    /// No `--root` could be recovered from the unit's contents.
    Unparseable,
}

#[derive(Debug)]
pub struct InstalledUnit {
    /// launchd label (`com.agentrec.<slug>`) or systemd unit name
    /// (`agentrec-<slug>.service`) — whichever the platform's tooling takes.
    pub label: String,
    pub path: PathBuf,
    pub state: UnitState,
    /// The recorded exec, when it was recoverable AND no longer exists on
    /// disk. `None` covers three DIFFERENT situations on purpose — exec
    /// present, exec unrecoverable, unit unreadable — because none of them is
    /// evidence the exec is missing.
    ///
    /// Deliberately a separate field rather than a `UnitState` variant: the
    /// two paths a unit bakes go stale INDEPENDENTLY. A vanished root with a
    /// live exec and a live root with a vanished exec are both real, and the
    /// second is the shape a `brew upgrade` produces — invisible to a
    /// root-only classification, and invisible to the daemon too, since a
    /// missing exec fails at spawn before any of this code runs.
    ///
    /// `Unparseable` must never carry `Some`: a unit we could not read tells
    /// us nothing about its exec, and reporting one would be a fabricated
    /// finding — the same rule that keeps `Unparseable` out of `VanishedRoot`.
    pub missing_exec: Option<PathBuf>,
}

/// Does `name` look like a unit file this tool installed? Both platforms'
/// spellings are recognized regardless of the host OS, so a fixture of either
/// form is scannable from any test runner.
fn is_agentrec_unit_name(name: &str) -> bool {
    (name.starts_with("com.agentrec.") && name.ends_with(".plist"))
        || (name.starts_with("agentrec-") && name.ends_with(".service"))
}

/// The identifier `launchctl bootout` / `systemctl --user disable` expects:
/// launchd wants the bare label (filename minus `.plist`), systemd wants the
/// unit filename including its `.service` suffix.
fn unit_label(name: &str) -> String {
    name.strip_suffix(".plist").unwrap_or(name).to_string()
}

/// Every agentrec unit installed in `dir`, each classified by whether its
/// recorded root still exists. A `dir` that doesn't exist yields an empty list
/// (nothing installed is not an error). Sorted by path so output is stable.
pub fn scan_units(dir: &Path) -> Vec<InstalledUnit> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut units: Vec<InstalledUnit> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_string();
            if !is_agentrec_unit_name(&name) {
                return None;
            }
            // Guarded: a unit file replaced by a fifo would otherwise hang
            // `doctor`/`status`. Refusal folds into the same `None` an
            // unreadable unit already produced -> `UnitState::Unparseable`.
            let content = agentrec_core::fsguard::read_regular_to_string(&path).ok();
            let state = match content.as_deref().and_then(parse_unit_root) {
                Some(root) if root.is_dir() => UnitState::Live(root),
                Some(root) => UnitState::VanishedRoot(root),
                None => UnitState::Unparseable,
            };
            // Read from the same `content`, so an unreadable file yields
            // `None` here for the same reason it yields `Unparseable` above,
            // rather than by a second independent read that could disagree.
            let missing_exec = content
                .as_deref()
                .and_then(parse_unit_exec)
                .filter(|exec| !exec.exists());
            Some(InstalledUnit {
                label: unit_label(&name),
                path,
                state,
                missing_exec,
            })
        })
        .collect();
    units.sort_by(|a, b| a.path.cmp(&b.path));
    units
}

/// Recover the exec a unit file records — `ProgramArguments[0]` on launchd,
/// the first `ExecStart=` token on systemd. Dispatches on CONTENT like
/// `parse_unit_root`, so both writers' output is parseable from either host.
pub fn parse_unit_exec(content: &str) -> Option<PathBuf> {
    if content.trim_start().starts_with('<') {
        parse_launchd_exec(content)
    } else {
        parse_systemd_exec(content)
    }
}

fn parse_launchd_exec(content: &str) -> Option<PathBuf> {
    let after_key = content.split("<key>ProgramArguments</key>").nth(1)?;
    let array = after_key
        .split("<array>")
        .nth(1)?
        .split("</array>")
        .next()?;
    array
        .split("<string>")
        .nth(1)?
        .split("</string>")
        .next()
        .map(|v| xml_unescape(v.trim()))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn parse_systemd_exec(content: &str) -> Option<PathBuf> {
    let line = content
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("ExecStart="))?;
    split_exec_start(line)
        .into_iter()
        .next()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Recover the `--root` a unit file records. Dispatches on CONTENT, not on the
/// host OS, so both parsers are exercised on every platform's test run.
pub fn parse_unit_root(content: &str) -> Option<PathBuf> {
    if content.trim_start().starts_with('<') {
        parse_launchd_root(content)
    } else {
        parse_systemd_root(content)
    }
}

/// Inverse of `xml_escape` — only the five entities that function emits.
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        // `&amp;` LAST: an escaped literal `&amp;lt;` in a path must decode to
        // the text `&lt;`, not be re-decoded into `<`.
        .replace("&amp;", "&")
}

/// Pull `ProgramArguments`' `<string>` values and return the one after
/// `--root`. Deliberately a narrow scan rather than a real XML parse: the only
/// documents this ever sees are the ones `launchd_plist` writes.
fn parse_launchd_root(content: &str) -> Option<PathBuf> {
    let after_key = content.split("<key>ProgramArguments</key>").nth(1)?;
    let array = after_key
        .split("<array>")
        .nth(1)?
        .split("</array>")
        .next()?;
    let mut args = array.split("<string>").skip(1).filter_map(|chunk| {
        chunk
            .split("</string>")
            .next()
            .map(|v| xml_unescape(v.trim()))
    });
    args.find(|a| a == "--root")?;
    args.next().filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Split an `ExecStart=` value into argv the way systemd does for the subset
/// `systemd_escape` can produce: whitespace separates tokens, a double quote
/// opens/closes a quoted run, and a backslash inside quotes escapes the next
/// character. `%%` -> `%` is undone per-token afterwards.
fn split_exec_start(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut in_quotes = false;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                started = true;
                in_quotes = !in_quotes;
            }
            '\\' if in_quotes => {
                started = true;
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if started {
                    tokens.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                started = true;
                current.push(c);
            }
        }
    }
    if started {
        tokens.push(current);
    }
    tokens.into_iter().map(|t| t.replace("%%", "%")).collect()
}

fn parse_systemd_root(content: &str) -> Option<PathBuf> {
    let line = content
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("ExecStart="))?;
    let tokens = split_exec_start(line);
    let idx = tokens.iter().position(|t| t == "--root")?;
    tokens
        .get(idx + 1)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The exact command pair a human runs to reap one unit. Displayed by
/// `doctor`; never executed here — removing a unit shells out to
/// `launchctl`/`systemctl`, and that stays a human action (see D46).
pub fn manual_remove_command(label: &str, path: &Path) -> String {
    if cfg!(target_os = "macos") {
        format!(
            "launchctl bootout gui/$(id -u)/{label} && rm {}",
            path.display()
        )
    } else {
        format!(
            "systemctl --user disable --now {label} && rm {}",
            path.display()
        )
    }
}

/// Write (or refresh) the unit file for `root` and attempt to load it.
/// Returns one line per action taken. A load failure is tolerated: it yields
/// an action line with the exact manual command instead of failing `init`.
pub fn install(root: &Path, exec: &Path) -> Result<Vec<String>, String> {
    let root = &resolve_root(root);
    let path = unit_path(root)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = if cfg!(target_os = "macos") {
        launchd_plist(exec, root)
    } else {
        systemd_unit(exec, root)
    };

    let mut actions = Vec::new();
    // A non-regular file at the unit path (FIFO, socket, directory) would
    // make the `fs::write` below block or fail confusingly — refuse instead
    // of proceeding. An absent path is the normal first-install case, and
    // `is_nonregular` returns false for it.
    if agentrec_core::fsguard::is_nonregular(&path) {
        return Err(format!(
            "refusing to write service unit: {} exists and is not a regular file",
            path.display()
        ));
    }
    // Guarded; a read error on the now-known-regular file folds into
    // `unwrap_or(false)`, i.e. the unit is treated as not-up-to-date and
    // rewritten.
    let unchanged = agentrec_core::fsguard::read_regular_to_string(&path)
        .map(|existing| existing == content)
        .unwrap_or(false);
    if unchanged {
        actions.push(format!(
            "service unit already up to date: {}",
            path.display()
        ));
    } else {
        std::fs::write(&path, &content).map_err(|e| e.to_string())?;
        actions.push(format!("wrote service unit: {}", path.display()));
    }

    match load(&path) {
        Ok(()) => actions.push("loaded service (starts on login, restarts on crash)".to_string()),
        Err(()) => actions.push(format!(
            "could not auto-load the service — run manually: {}",
            manual_load_command(&path)
        )),
    }
    Ok(actions)
}

/// Unload and remove the unit file. Absence and load/unload failures are
/// tolerated — uninstall must never fail because the service was never
/// loaded (or already gone). Returns one line per action taken.
pub fn uninstall(root: &Path) -> Vec<String> {
    let root = &resolve_root(root);
    let mut actions = Vec::new();
    let path = match unit_path(root) {
        Ok(p) => p,
        Err(e) => {
            actions.push(format!("skipped service removal: {e}"));
            return actions;
        }
    };
    if path.exists() {
        let _ = unload(&path);
        actions.push(format!("unloaded service: {}", path.display()));
        match std::fs::remove_file(&path) {
            Ok(()) => actions.push(format!("removed service unit: {}", path.display())),
            Err(e) => actions.push(format!(
                "could not remove service unit {}: {e}",
                path.display()
            )),
        }
    } else {
        actions.push("no service unit installed".to_string());
    }
    actions
}

/// D11: re-`init` on an already-loaded service previously failed silently —
/// `launchctl load` on a label that's already loaded refuses (needs an
/// explicit `unload` first), and a rewritten unit file on Linux was never
/// picked up because nothing told systemd to re-read it off disk. Both
/// failures were swallowed by `load`'s existing "could not auto-load, run
/// manually" fallback, so `init` still reported success while the OLD unit
/// content kept running.
// Spawns `launchctl` / `systemctl`: fixed program names, never a
// repo-authored command, and installing a service unit is inherently an
// out-of-process operation. Both arms are `cfg!` (runtime), so clippy sees
// both on every platform.
#[allow(clippy::disallowed_methods)]
fn load(path: &Path) -> Result<(), ()> {
    let status = if cfg!(target_os = "macos") {
        // Best-effort: fails harmlessly if nothing was loaded yet.
        let _ = Command::new("launchctl").arg("unload").arg(path).status();
        Command::new("launchctl")
            .args(["load", "-w"])
            .arg(path)
            .status()
    } else {
        // Best-effort: makes systemd re-read the unit file we may have just
        // rewritten, before `enable --now` acts on it.
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        Command::new("systemctl")
            .args(["--user", "enable", "--now"])
            .arg(path)
            .status()
    };
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => Err(()),
    }
}

// Spawns `launchctl` / `systemctl` to unload the unit `load` installed —
// same fixed program names, same reason.
#[allow(clippy::disallowed_methods)]
fn unload(path: &Path) -> Result<(), ()> {
    let status = if cfg!(target_os = "macos") {
        Command::new("launchctl").arg("unload").arg(path).status()
    } else {
        Command::new("systemctl")
            .args(["--user", "disable", "--now"])
            .arg(path)
            .status()
    };
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => Err(()),
    }
}

fn manual_load_command(path: &Path) -> String {
    if cfg!(target_os = "macos") {
        format!("launchctl load -w {}", path.display())
    } else {
        format!("systemctl --user enable --now {}", path.display())
    }
}

#[cfg(test)]
// Test code reads its own tempdir fixtures; no attacker-supplied FIFO can
// block these, so the fsguard wrappers buy nothing. Scoped to this module
// so production reads in this file stay lint-enforced (clippy.toml).
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    // Env vars are process-global, not thread-local — Rust runs #[test]
    // functions on parallel threads by default, so any test that mutates
    // XDG_CONFIG_HOME (or relies on its absence) must serialize against every
    // other such test via this lock, or the mutations race.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // CONCERN A: two relative spellings of one repo canonicalize to the same
    // path, so a relative `--root` doesn't fork the slug/unit off from what
    // `agentrec init` used for the same repo.
    #[test]
    fn resolve_root_canonicalizes_relative_spellings_to_one_path() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();

        let via_dot = canonical.join(".");
        let via_dotdot = canonical.join("sub").join("..");
        std::fs::create_dir_all(canonical.join("sub")).unwrap();

        assert_eq!(resolve_root(&via_dot), canonical);
        assert_eq!(resolve_root(&via_dotdot), canonical);
        assert_eq!(
            slug(&resolve_root(&via_dot)),
            slug(&resolve_root(&via_dotdot))
        );
    }

    #[test]
    fn resolve_root_falls_back_when_path_does_not_exist() {
        let missing = Path::new("/definitely/does/not/exist/agentrec-test");
        assert_eq!(resolve_root(missing), missing);
    }

    #[test]
    fn slug_is_stable_hex12_and_repo_specific() {
        let a = slug(Path::new("/Users/x/proj"));
        let b = slug(Path::new("/Users/x/proj"));
        let c = slug(Path::new("/Users/x/other"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 12);
        assert!(a.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn launchd_plist_shape() {
        let root = Path::new("/repo");
        let plist = launchd_plist(Path::new("/usr/local/bin/agentrec"), root);
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<string>/usr/local/bin/agentrec</string>"));
        assert!(plist.contains("<string>record</string>"));
        assert!(plist.contains("<string>--root</string>"));
        assert!(plist.contains("<string>/repo</string>"));
        assert!(plist.contains(&format!("com.agentrec.{}", slug(root))));
    }

    // F24: with no `StandardOutPath`/`StandardErrorPath`, launchd sends the
    // job's stdio to `/dev/null` and every daemon `eprintln!` — including the
    // only record that eviction deleted snapshot blobs — is discarded on
    // macOS. Both keys must be present AND must name the log inside the
    // recorded root's `.agentrec`, which the watcher denylists.
    #[test]
    fn launchd_plist_routes_stdio_to_the_repo_daemon_log() {
        let root = Path::new("/repo");
        let plist = launchd_plist(Path::new("/usr/local/bin/agentrec"), root);
        let expected = root.join(".agentrec").join("daemon.log");
        let expected = expected.display().to_string();
        assert!(
            plist.contains("<key>StandardOutPath</key>"),
            "stdout must be routed: {plist}"
        );
        assert!(
            plist.contains("<key>StandardErrorPath</key>"),
            "stderr must be routed: {plist}"
        );
        for key in ["StandardOutPath", "StandardErrorPath"] {
            let after = plist
                .split(&format!("<key>{key}</key>"))
                .nth(1)
                .unwrap_or_else(|| panic!("{key} section missing: {plist}"));
            let value = after
                .split("<string>")
                .nth(1)
                .and_then(|c| c.split("</string>").next())
                .unwrap_or_else(|| panic!("{key} has no <string> value: {plist}"));
            assert_eq!(value.trim(), expected, "{key} must name the repo log");
        }
        // The two stdio keys sit alongside the existing ones, not instead of
        // them: a regression that replaced the exec-discovery surface would
        // silently blind `doctor`'s vanished-exec check.
        assert_eq!(
            parse_unit_exec(&plist).as_deref(),
            Some(Path::new("/usr/local/bin/agentrec"))
        );
        assert_eq!(parse_unit_root(&plist).as_deref(), Some(root));
    }

    // D5 applies to the new value too: an `&` in the repo path reaches the log
    // path as well, and an unescaped one makes the plist invalid XML — so
    // `launchctl load` fails while `init` already claimed success.
    #[test]
    fn launchd_plist_escapes_the_daemon_log_path() {
        let root = Path::new("/repo/AT&T");
        let plist = launchd_plist(Path::new("/usr/local/bin/agentrec"), root);
        let expected = root.join(".agentrec").join("daemon.log");
        let escaped = xml_escape(&expected.display().to_string());
        assert!(
            plist.contains(&format!("<string>{escaped}</string>")),
            "escaped log path missing: {plist}"
        );
        assert!(
            !plist.contains(&format!("<string>{}</string>", expected.display())),
            "raw ampersand leaked into the plist: {plist}"
        );
    }

    #[test]
    fn systemd_unit_shape() {
        let unit = systemd_unit(Path::new("/usr/local/bin/agentrec"), Path::new("/repo"));
        assert!(unit.contains("ExecStart=/usr/local/bin/agentrec record --root /repo"));
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("[Install]"));
    }

    // D5: a root containing a space must not split into extra ExecStart argv
    // entries — the whole value is double-quoted.
    #[test]
    fn systemd_unit_quotes_a_root_containing_a_space() {
        let unit = systemd_unit(
            Path::new("/usr/local/bin/agentrec"),
            Path::new("/repo with space"),
        );
        assert!(
            unit.contains(r#"ExecStart=/usr/local/bin/agentrec record --root "/repo with space""#),
            "unit: {unit}"
        );
    }

    // D5: `%` starts a systemd specifier expansion (`%h`, `%n`, …) unless
    // doubled — an unescaped `%` in a path would silently mangle it.
    #[test]
    fn systemd_unit_doubles_a_literal_percent() {
        let unit = systemd_unit(
            Path::new("/usr/local/bin/agentrec"),
            Path::new("/repo/100%done"),
        );
        assert!(
            unit.contains("ExecStart=/usr/local/bin/agentrec record --root /repo/100%%done"),
            "unit: {unit}"
        );
    }

    // D5: `&` has no special meaning in systemd unit-file argument syntax
    // (it is not passed through a shell) — it must survive unquoted and
    // unescaped, unlike the launchd XML case below. No whitespace here
    // deliberately, to isolate `&` handling from the separate quote-on-space
    // rule proven by `systemd_unit_quotes_a_root_containing_a_space`.
    #[test]
    fn systemd_unit_leaves_ampersand_untouched() {
        let unit = systemd_unit(
            Path::new("/usr/local/bin/agentrec"),
            Path::new("/repo/AT&T"),
        );
        assert!(
            unit.contains("ExecStart=/usr/local/bin/agentrec record --root /repo/AT&T"),
            "unit: {unit}"
        );
    }

    // D5: an embedded quote/backslash inside a space-triggered quoted value
    // must itself be backslash-escaped, or the generated unit is unparsable.
    #[test]
    fn systemd_unit_escapes_embedded_quote_when_quoting() {
        let unit = systemd_unit(
            Path::new("/usr/local/bin/agentrec"),
            Path::new("/repo \"weird\" dir"),
        );
        assert!(unit.contains(r#""/repo \"weird\" dir""#), "unit: {unit}");
    }

    // D5: XML special characters in the repo path must be entity-escaped, or
    // the generated plist is not well-formed XML and `launchctl load` fails
    // with no useful diagnostic while `init` already reported success.
    #[test]
    fn launchd_plist_escapes_ampersand_and_quotes() {
        let plist = launchd_plist(
            Path::new("/usr/local/bin/agentrec"),
            Path::new("/repo & co"),
        );
        assert!(
            plist.contains("<string>/repo &amp; co</string>"),
            "plist: {plist}"
        );
        assert!(!plist.contains("<string>/repo & co</string>"));
    }

    // D5: launchd plists have no `%`-specifier concept (that's systemd-only)
    // — a literal `%` must pass through untouched, unlike `systemd_escape`.
    #[test]
    fn launchd_plist_leaves_percent_untouched() {
        let plist = launchd_plist(
            Path::new("/usr/local/bin/agentrec"),
            Path::new("/repo/100%done"),
        );
        assert!(
            plist.contains("<string>/repo/100%done</string>"),
            "plist: {plist}"
        );
    }

    // D5: a space needs no XML escaping (only the five XML special chars
    // do) — must survive verbatim, no spurious quoting either (this is XML,
    // not a systemd command line).
    #[test]
    fn launchd_plist_space_survives_unescaped() {
        let plist = launchd_plist(
            Path::new("/usr/local/bin/agentrec"),
            Path::new("/repo with space"),
        );
        assert!(
            plist.contains("<string>/repo with space</string>"),
            "plist: {plist}"
        );
    }

    #[test]
    fn unit_path_uses_home_and_slug() {
        // Deterministic regardless of the ambient environment (and immune to
        // racing the XDG_CONFIG_HOME tests below): explicitly clear it so
        // Linux falls back to $HOME/.config, the behavior this test asserts.
        let _guard = ENV_LOCK.lock().unwrap();
        let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::remove_var("XDG_CONFIG_HOME");

        let root = Path::new("/repo/example");
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        let path = unit_path(root).expect("HOME is set in this test environment");

        if let Some(v) = prev_xdg {
            std::env::set_var("XDG_CONFIG_HOME", v);
        }

        assert!(path.starts_with(&home));
        assert!(path.to_string_lossy().contains(&slug(root)));
        #[cfg(target_os = "macos")]
        {
            assert!(path.to_string_lossy().contains("Library/LaunchAgents"));
            assert_eq!(path.extension().and_then(|e| e.to_str()), Some("plist"));
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert!(path.to_string_lossy().contains(".config/systemd/user"));
            assert_eq!(path.extension().and_then(|e| e.to_str()), Some("service"));
        }
    }

    // XDG basedir spec: an explicit, non-empty XDG_CONFIG_HOME overrides
    // $HOME/.config outright. `config_home` is the resolver both `unit_path`
    // (Linux branch) and any future call site must share.
    #[test]
    fn config_home_prefers_xdg_config_home_when_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("XDG_CONFIG_HOME").ok();

        std::env::set_var("XDG_CONFIG_HOME", "/custom/xdg-config");
        let result = config_home();

        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }

        assert_eq!(result, Ok(PathBuf::from("/custom/xdg-config")));
    }

    // Both the "never set" and the "set but empty" cases fall back to
    // $HOME/.config per the XDG basedir spec (an empty value is treated as
    // unset, not as "use the current directory").
    #[test]
    fn config_home_falls_back_to_home_dot_config_when_xdg_unset_or_empty() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("XDG_CONFIG_HOME").ok();
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        let expected = PathBuf::from(&home).join(".config");

        std::env::remove_var("XDG_CONFIG_HOME");
        let unset_result = config_home();

        std::env::set_var("XDG_CONFIG_HOME", "");
        let empty_result = config_home();

        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }

        assert_eq!(unset_result, Ok(expected.clone()));
        assert_eq!(empty_result, Ok(expected));
    }

    // Review finding #2 / XDG basedir spec: "If an implementation encounters a
    // relative path ... it should consider the path invalid and ignore it."
    // A relative XDG_CONFIG_HOME must fall back to $HOME/.config, not produce a
    // CWD-relative systemd unit path that `install`/`uninstall` would write to
    // wherever the command happened to run.
    #[test]
    fn config_home_ignores_relative_xdg_config_home() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("XDG_CONFIG_HOME").ok();
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        let expected = PathBuf::from(&home).join(".config");

        std::env::set_var("XDG_CONFIG_HOME", "relcfg");
        let relative_result = config_home();

        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }

        assert_eq!(
            relative_result,
            Ok(expected),
            "a relative XDG_CONFIG_HOME must be ignored per the XDG basedir spec"
        );
    }

    // unit_path itself (not just the config_home helper) must actually route
    // the systemd user-unit path through XDG_CONFIG_HOME on Linux — this is
    // the regression the two config_home tests above can't catch alone since
    // they don't exercise unit_path's branch wiring.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unit_path_respects_xdg_config_home_on_linux() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("XDG_CONFIG_HOME").ok();
        let tmp = tempfile::tempdir().unwrap();

        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        let root = Path::new("/repo/example");
        let path = unit_path(root);

        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }

        let path = path.unwrap();
        assert!(path.starts_with(tmp.path().join("systemd").join("user")));
        assert!(path.to_string_lossy().contains(&slug(root)));
    }

    // No test drives `install`/`uninstall` directly — both shell out to the
    // real `launchctl`/`systemctl`, which the hermetic-tests requirement
    // forbids touching from the automated suite.

    // ---- D46 AC-S5/S6: unit discovery -----------------------------------

    /// Roots that stress every escaping rule on both writers at once: the XML
    /// entities, the systemd `%` specifier, quote/backslash escaping, and the
    /// whitespace-triggered quoting.
    const TRICKY_ROOTS: &[&str] = &[
        "/repo/plain",
        "/repo with space",
        "/repo/AT&T",
        "/repo/100%done",
        "/repo/\"weird\" dir",
        "/repo/<angle>&amp;lt;",
        "/repo/it's mine",
    ];

    // The unit must gate its own restarts on the root still existing, or a
    // deleted repo respawns a doomed job forever — the mechanism behind 39
    // leaked units. A bare `KeepAlive <true/>` is exactly that defect.
    #[test]
    fn launchd_plist_gates_keepalive_on_the_root_path() {
        let plist = launchd_plist(Path::new("/usr/local/bin/agentrec"), Path::new("/repo"));
        assert!(
            plist.contains("<key>PathState</key>"),
            "KeepAlive must be conditional: {plist}"
        );
        // The gated path must be the ROOT.
        let path_state = plist
            .split("<key>PathState</key>")
            .nth(1)
            .expect("PathState section");
        assert!(
            path_state.contains("<key>/repo</key>"),
            "PathState must name the root: {plist}"
        );
        assert!(
            !plist.contains("<key>KeepAlive</key>\n\t<true/>"),
            "a bare KeepAlive is the defect this replaces: {plist}"
        );
    }

    // MEASURED CONSTRAINT, not a style choice: launchd ORs the entries in a
    // KeepAlive dict, and the OR extends inside PathState — a probe with two
    // keys, one absent, still produced 3 spawns in 30s. Listing the exec
    // alongside the root would therefore keep a job alive whenever EITHER
    // exists, silently making the gate useless in the very case it is meant
    // for (root present, binary upgraded away). This test exists so a future
    // reader who thinks "why isn't the exec gated too?" adds it and reds
    // here instead of shipping an inert condition.
    #[test]
    fn launchd_pathstate_lists_only_the_root_never_the_exec() {
        let exec = Path::new("/opt/homebrew/bin/agentrec");
        let plist = launchd_plist(exec, Path::new("/repo"));
        let path_state = plist
            .split("<key>PathState</key>")
            .nth(1)
            .expect("PathState section");
        assert!(
            !path_state.contains(exec.to_str().unwrap()),
            "PathState ORs its entries — adding the exec makes the gate inert: {plist}"
        );
    }

    // systemd counterpart. NOT measured (no systemd host available when this
    // was written) — the generator is pinned here, the runtime effect carries
    // a VERIFY-LEDGER row.
    #[test]
    fn systemd_unit_gates_start_on_the_root_directory() {
        let unit = systemd_unit(Path::new("/usr/local/bin/agentrec"), Path::new("/repo"));
        assert!(
            unit.contains("ConditionPathIsDirectory=/repo"),
            "start must be conditional on the root: {unit}"
        );
    }

    // A `%` in the root would start a systemd specifier expansion. The
    // Condition value takes ONE value, so it must be `%`-doubled but NOT
    // shell-quoted the way an ExecStart token is.
    #[test]
    fn condition_value_escapes_percent_without_quoting() {
        let unit = systemd_unit(Path::new("/bin/agentrec"), Path::new("/repo/100%done"));
        assert!(
            unit.contains("ConditionPathIsDirectory=/repo/100%%done"),
            "% must be doubled: {unit}"
        );
        let unit = systemd_unit(Path::new("/bin/agentrec"), Path::new("/repo with space"));
        assert!(
            unit.contains("ConditionPathIsDirectory=/repo with space\n"),
            "a single-value setting must not gain argv quoting: {unit}"
        );
    }

    // The exec is the unit's OTHER baked path, and it needs the same
    // round-trip guarantee as the root: a mangled exec would make the scan
    // stat a path the unit never named and report a healthy install as
    // stranded. Reuses TRICKY_ROOTS as exec paths — the escaping rules are
    // per-writer, not per-field, so the same corpus stresses both.
    #[test]
    fn parse_unit_exec_round_trips_both_unit_forms() {
        let root = Path::new("/repo/plain");
        for raw in TRICKY_ROOTS {
            let exec = Path::new(raw);
            let plist = launchd_plist(exec, root);
            assert_eq!(
                parse_unit_exec(&plist).as_deref(),
                Some(exec),
                "launchd exec round-trip failed for {raw}: {plist}"
            );
            let unit = systemd_unit(exec, root);
            assert_eq!(
                parse_unit_exec(&unit).as_deref(),
                Some(exec),
                "systemd exec round-trip failed for {raw}: {unit}"
            );
        }
    }

    // The two parsers must not cross-talk: an exec and a root that differ have
    // to come back distinct from the SAME document. A `nth(0)`/`nth(1)` slip
    // in either direction would return the exec as the root, which the scan
    // would then stat as a repo.
    #[test]
    fn exec_and_root_are_recovered_independently() {
        let exec = Path::new("/opt/homebrew/bin/agentrec");
        let root = Path::new("/Users/x/repo with space");
        for doc in [launchd_plist(exec, root), systemd_unit(exec, root)] {
            assert_eq!(parse_unit_exec(&doc).as_deref(), Some(exec), "{doc}");
            assert_eq!(parse_unit_root(&doc).as_deref(), Some(root), "{doc}");
        }
    }

    // AC-S5: parsing must invert BOTH writers exactly. A root that survives
    // `xml_escape`/`systemd_escape` but comes back mangled would make the
    // orphan scan compare a wrong path against the filesystem and report a
    // live repo as vanished.
    #[test]
    fn parse_unit_root_round_trips_both_unit_forms() {
        let exec = Path::new("/usr/local/bin/agentrec");
        for raw in TRICKY_ROOTS {
            let root = Path::new(raw);
            let plist = launchd_plist(exec, root);
            assert_eq!(
                parse_unit_root(&plist).as_deref(),
                Some(root),
                "launchd round-trip failed for {raw}: {plist}"
            );
            let unit = systemd_unit(exec, root);
            assert_eq!(
                parse_unit_root(&unit).as_deref(),
                Some(root),
                "systemd round-trip failed for {raw}: {unit}"
            );
        }
    }

    // The dispatcher keys on CONTENT, not on the host OS — otherwise half the
    // parsing surface would be dead code on any given test runner.
    #[test]
    fn parse_unit_root_dispatches_on_content_not_platform() {
        let exec = Path::new("/usr/local/bin/agentrec");
        let root = Path::new("/repo/example");
        // Both forms parse on THIS platform, whichever it is.
        assert!(parse_unit_root(&launchd_plist(exec, root)).is_some());
        assert!(parse_unit_root(&systemd_unit(exec, root)).is_some());
    }

    // A unit whose contents carry no recoverable `--root` must yield None, so
    // `scan_units` can classify it `Unparseable` rather than inventing a root.
    #[test]
    fn parse_unit_root_returns_none_on_unrecoverable_contents() {
        assert_eq!(parse_unit_root(""), None);
        assert_eq!(parse_unit_root("<plist><dict></dict></plist>"), None);
        assert_eq!(parse_unit_root("[Service]\nExecStart=/bin/true\n"), None);
        // `--root` present but with no following value.
        assert_eq!(
            parse_unit_root("[Service]\nExecStart=/bin/agentrec record --root\n"),
            None
        );
    }

    // AC-S6: three DISJOINT buckets, and an unparseable unit is never counted
    // as an orphan — that would be a fabricated finding about a unit we could
    // not read. Unrelated files sharing the directory are ignored entirely.
    #[test]
    fn scan_units_classifies_live_vanished_and_unparseable_disjointly() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("units");
        std::fs::create_dir_all(&dir).unwrap();
        let live_root = tmp.path().join("live-repo");
        std::fs::create_dir_all(&live_root).unwrap();
        let gone_root = tmp.path().join("deleted-repo"); // never created

        let exec = Path::new("/usr/local/bin/agentrec");
        std::fs::write(
            dir.join(format!("com.agentrec.{}.plist", slug(&live_root))),
            launchd_plist(exec, &live_root),
        )
        .unwrap();
        std::fs::write(
            dir.join(format!("com.agentrec.{}.plist", slug(&gone_root))),
            launchd_plist(exec, &gone_root),
        )
        .unwrap();
        std::fs::write(
            dir.join("com.agentrec.deadbeef0000.plist"),
            "<plist><dict></dict></plist>",
        )
        .unwrap();
        // Neither of these is ours; both must be invisible to the scan.
        std::fs::write(dir.join("com.other.tool.plist"), "<plist/>").unwrap();
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();

        let units = scan_units(&dir);
        assert_eq!(units.len(), 3, "only agentrec units: {units:?}");

        let live: Vec<_> = units
            .iter()
            .filter(|u| matches!(u.state, UnitState::Live(_)))
            .collect();
        let vanished: Vec<_> = units
            .iter()
            .filter(|u| matches!(u.state, UnitState::VanishedRoot(_)))
            .collect();
        let unparseable: Vec<_> = units
            .iter()
            .filter(|u| u.state == UnitState::Unparseable)
            .collect();
        assert_eq!(live.len(), 1, "{units:?}");
        assert_eq!(vanished.len(), 1, "{units:?}");
        assert_eq!(unparseable.len(), 1, "{units:?}");
        assert_eq!(live[0].state, UnitState::Live(live_root));
        assert_eq!(vanished[0].state, UnitState::VanishedRoot(gone_root));
        // The label must be what `launchctl bootout` takes — no `.plist`.
        assert!(
            vanished[0].label.starts_with("com.agentrec."),
            "{:?}",
            vanished[0].label
        );
        assert!(!vanished[0].label.ends_with(".plist"));
    }

    // A systemd-form unit is scanned identically (and on any host OS), so the
    // Linux install shape is covered without a Linux runner.
    #[test]
    fn scan_units_reads_systemd_form_units() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("units");
        std::fs::create_dir_all(&dir).unwrap();
        let gone_root = tmp.path().join("deleted-repo");
        std::fs::write(
            dir.join(format!("agentrec-{}.service", slug(&gone_root))),
            systemd_unit(Path::new("/usr/local/bin/agentrec"), &gone_root),
        )
        .unwrap();

        let units = scan_units(&dir);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].state, UnitState::VanishedRoot(gone_root));
        // systemd's own tooling takes the full unit filename.
        assert!(units[0].label.ends_with(".service"));
    }

    // Nothing installed (or no service directory at all) is not an error.
    #[test]
    fn scan_units_of_missing_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(scan_units(&tmp.path().join("nope")).is_empty());
    }

    // AC-S9: REAL-CORPUS probe, `#[ignore]`d by design — it reads this
    // machine's actual service directory, so it is environment-coupled and
    // must never run as part of the hermetic suite. Run it explicitly:
    //
    //   cargo test --bin agentrec real_corpus_unit_scan -- --ignored --nocapture
    //
    // Its whole purpose is that a round-trip test against our OWN writer
    // cannot establish that the parser reads the plists actually installed
    // here (this repo has been bitten four times by fixture-only evidence for
    // a corpus-shape claim). It asserts nothing about counts — the corpus is
    // expected to shrink once the orphans are reaped — it REPORTS the three
    // bucket sizes for the ledger, and fails only if a unit exists that the
    // parser cannot read at all, which would be a real parser bug.
    #[test]
    #[ignore = "reads the real user service directory; run explicitly for corpus evidence"]
    fn real_corpus_unit_scan() {
        let dir = service_dir().expect("HOME must be set");
        let units = scan_units(&dir);
        let live = units
            .iter()
            .filter(|u| matches!(u.state, UnitState::Live(_)))
            .count();
        let vanished = units
            .iter()
            .filter(|u| matches!(u.state, UnitState::VanishedRoot(_)))
            .count();
        let unparseable: Vec<_> = units
            .iter()
            .filter(|u| u.state == UnitState::Unparseable)
            .collect();
        println!(
            "real-corpus scan of {}: {} agentrec unit(s) — {} parsed ({} live, {} vanished-root), {} unparseable",
            dir.display(),
            units.len(),
            units.len() - unparseable.len(),
            live,
            vanished,
            unparseable.len()
        );
        for u in &units {
            println!("  {:?} {}", u.state, u.label);
        }
        assert!(
            unparseable.is_empty(),
            "every unit this tool installed must be parseable; unreadable: {unparseable:?}"
        );
    }

    // A FIFO at the unit path makes `fs::write` block forever on
    // `open(O_WRONLY)` with no reader — `install` must refuse on the type
    // check instead of hanging `init`. The whole assertion is "this test
    // returns at all"; the message check only pins WHICH refusal fired.
    //
    // Env is restored before any assertion so a failure cannot leak
    // AGENTREC_TEST_SERVICE_DIR into the rest of the process or poison
    // ENV_LOCK for every other test that takes it.
    #[test]
    #[cfg(unix)]
    fn install_refuses_when_unit_path_is_a_fifo() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("AGENTREC_TEST_SERVICE_DIR").ok();

        let service_dir = tempfile::tempdir().unwrap();
        let root_dir = tempfile::tempdir().unwrap();
        std::env::set_var("AGENTREC_TEST_SERVICE_DIR", service_dir.path());

        // `install` computes the unit path from the RESOLVED root, and `slug`
        // keys on that canonicalized path — deriving the FIFO path any other
        // way puts it at a filename `install` never touches, which would let
        // the test sail past the guard into `launchctl`.
        let resolved = resolve_root(root_dir.path());
        let path = unit_path(&resolved).unwrap();
        let mkfifo = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let is_fifo = std::fs::symlink_metadata(&path)
            .map(|m| !m.file_type().is_file())
            .unwrap_or(false);

        let result = if mkfifo && is_fifo {
            Some(install(
                root_dir.path(),
                Path::new("/usr/local/bin/agentrec"),
            ))
        } else {
            None
        };

        // Remove the FIFO before the tempdir drops (and before any assert).
        let _ = std::fs::remove_file(&path);
        match prev {
            Some(v) => std::env::set_var("AGENTREC_TEST_SERVICE_DIR", v),
            None => std::env::remove_var("AGENTREC_TEST_SERVICE_DIR"),
        }

        assert!(mkfifo && is_fifo, "fixture failed: no FIFO at {path:?}");
        let err = result
            .unwrap()
            .expect_err("install must refuse a non-regular unit path");
        assert!(
            err.contains("not a regular file"),
            "wrong refusal reason: {err}"
        );
        assert!(err.contains(&path.display().to_string()), "{err}");
    }

    // ALLOW control for the guard above: with the unit path ABSENT, `install`
    // must get past the type check and reach `fs::write`.
    //
    // It deliberately does NOT let a successful write happen: `install` then
    // shells `launchctl load -w`, and this repo has already leaked ~39 real
    // LaunchAgents from tests. So the unit directory is made unwritable
    // (0o500) — `create_dir_all` is a no-op on an existing dir, the guard
    // passes, and `fs::write` dies with EACCES. A permission-denied error is
    // therefore positive proof that execution reached the write, while
    // `launchctl` is provably unreachable.
    #[test]
    #[cfg(unix)]
    fn install_proceeds_past_guard_when_unit_path_is_absent() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("AGENTREC_TEST_SERVICE_DIR").ok();

        let tmp = tempfile::tempdir().unwrap();
        let service_dir = tmp.path().join("units");
        std::fs::create_dir_all(&service_dir).unwrap();
        let root_dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(&service_dir, std::fs::Permissions::from_mode(0o500)).unwrap();

        // Probe FIRST: if the filesystem/uid does not enforce the mode (root,
        // odd FS), `install` would succeed and reach `launchctl`. Skip rather
        // than risk that.
        let enforced = std::fs::write(service_dir.join(".perm_probe"), b"x").is_err();

        let outcome = if enforced {
            std::env::set_var("AGENTREC_TEST_SERVICE_DIR", &service_dir);
            let resolved = resolve_root(root_dir.path());
            let path = unit_path(&resolved).unwrap();
            let absent = !path.exists();
            let guarded = agentrec_core::fsguard::is_nonregular(&path);
            let r = install(root_dir.path(), Path::new("/usr/local/bin/agentrec"));
            Some((absent, guarded, r))
        } else {
            None
        };

        match prev {
            Some(v) => std::env::set_var("AGENTREC_TEST_SERVICE_DIR", v),
            None => std::env::remove_var("AGENTREC_TEST_SERVICE_DIR"),
        }
        // Restore write permission so the tempdir can be cleaned up.
        let _ = std::fs::set_permissions(&service_dir, std::fs::Permissions::from_mode(0o700));

        let Some((absent, guarded, result)) = outcome else {
            // Not enforced (likely root): the control cannot run safely.
            return;
        };
        assert!(absent, "fixture: unit path must start absent");
        assert!(!guarded, "an absent path is not non-regular");
        let err = result.expect_err("the unwritable unit dir must fail the write");
        assert!(
            !err.contains("not a regular file"),
            "the refusal guard fired on an absent path: {err}"
        );
        assert!(
            err.to_lowercase().contains("permission denied"),
            "expected the fs::write EACCES, got: {err}"
        );
    }

    // The printed remedy must name the unit and its file — a remedy the user
    // has to reconstruct by hand is not a remedy.
    #[test]
    fn manual_remove_command_names_label_and_path() {
        let cmd = manual_remove_command("com.agentrec.abc123", Path::new("/tmp/x.plist"));
        assert!(cmd.contains("com.agentrec.abc123"), "{cmd}");
        assert!(cmd.contains("/tmp/x.plist"), "{cmd}");
    }
}
