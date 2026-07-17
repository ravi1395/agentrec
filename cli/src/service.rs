//! Per-repo service unit (macOS launchd / Linux systemd `--user`) generation
//! and (un)load, for `agentrec init` / `agentrec uninstall` (AC Y+3/Y+4). The
//! unit runs `<agentrec> record --root <repo>` so the daemon survives reboots
//! and restarts on crash (`KeepAlive true` / `Restart=always`).
//!
//! `slug`/`launchd_plist`/`systemd_unit`/`unit_path` are pure — no filesystem
//! or process access — and are unit-tested directly below. `install`/
//! `uninstall` do real (best-effort, failure-tolerant) I/O, including
//! spawning `launchctl`/`systemctl`; per the hermetic-tests requirement they
//! are exercised ONLY through manual verification and `agentrec init/
//! uninstall --no-service` in the CLI test suite, never called from an
//! automated test.

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

/// macOS launchd plist: runs `record --root <root>` at load and on crash.
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
\t<true/>\n\
</dict>\n\
</plist>\n",
        slug = slug(root),
        exec = xml_escape(&exec.display().to_string()),
        root = xml_escape(&root.display().to_string()),
    )
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

/// Linux systemd user unit: same exec; `Restart=always` mirrors `KeepAlive`.
pub fn systemd_unit(exec: &Path, root: &Path) -> String {
    format!(
        "[Unit]\n\
Description=agentrec recorder for {root}\n\
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
    )
}

/// XDG Base Directory config home: `$XDG_CONFIG_HOME` if set to a non-empty
/// value, else `$HOME/.config` (the XDG basedir spec's documented fallback).
/// Linux-only call site — launchd's `~/Library/LaunchAgents` is not an XDG
/// path and must never route through this helper.
fn config_home() -> Result<PathBuf, String> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg));
        }
    }
    let home = std::env::var("HOME").map_err(|_| {
        "HOME environment variable is not set — cannot locate the service directory".to_string()
    })?;
    Ok(PathBuf::from(home).join(".config"))
}

/// Per-OS unit file path. A bare env read (no filesystem access), so it errs
/// rather than panics when `HOME` is unset, and stays hermetically
/// unit-testable. macOS always uses `$HOME/Library/LaunchAgents` (not an XDG
/// path); Linux resolves the systemd user-unit directory via `config_home`
/// (`$XDG_CONFIG_HOME`, falling back to `$HOME/.config`) — the single
/// resolver `install`/`uninstall` both go through.
pub fn unit_path(root: &Path) -> Result<PathBuf, String> {
    let s = slug(root);
    let path = if cfg!(target_os = "macos") {
        let home = std::env::var("HOME").map_err(|_| {
            "HOME environment variable is not set — cannot locate the service directory".to_string()
        })?;
        PathBuf::from(home)
            .join("Library")
            .join("LaunchAgents")
            .join(format!("com.agentrec.{s}.plist"))
    } else {
        config_home()?
            .join("systemd")
            .join("user")
            .join(format!("agentrec-{s}.service"))
    };
    Ok(path)
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
    let unchanged = std::fs::read_to_string(&path)
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
}
