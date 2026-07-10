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
        exec = exec.display(),
        root = root.display(),
    )
}

/// Linux systemd user unit: same exec; `Restart=always` mirrors `KeepAlive`.
pub fn systemd_unit(exec: &Path, root: &Path) -> String {
    format!(
        "[Unit]\n\
Description=agentrec recorder for {root}\n\
\n\
[Service]\n\
ExecStart={exec} record --root {root}\n\
Restart=always\n\
\n\
[Install]\n\
WantedBy=default.target\n",
        exec = exec.display(),
        root = root.display(),
    )
}

/// Per-OS unit file path under `$HOME`. A bare env read (no filesystem
/// access), so it errs rather than panics when `HOME` is unset, and stays
/// hermetically unit-testable.
pub fn unit_path(root: &Path) -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| {
        "HOME environment variable is not set — cannot locate the service directory".to_string()
    })?;
    let s = slug(root);
    let path = if cfg!(target_os = "macos") {
        PathBuf::from(home)
            .join("Library")
            .join("LaunchAgents")
            .join(format!("com.agentrec.{s}.plist"))
    } else {
        PathBuf::from(home)
            .join(".config")
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

fn load(path: &Path) -> Result<(), ()> {
    let status = if cfg!(target_os = "macos") {
        Command::new("launchctl")
            .args(["load", "-w"])
            .arg(path)
            .status()
    } else {
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

    #[test]
    fn unit_path_uses_home_and_slug() {
        let root = Path::new("/repo/example");
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        let path = unit_path(root).expect("HOME is set in this test environment");
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

    // No test drives `install`/`uninstall` directly — both shell out to the
    // real `launchctl`/`systemctl`, which the hermetic-tests requirement
    // forbids touching from the automated suite.
}
