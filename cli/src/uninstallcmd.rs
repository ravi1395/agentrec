//! `agentrec uninstall` (AC-Y+4): the reverse of `init` — remove agentrec's
//! own Claude Code hook entries, unload+remove the per-repo service unit,
//! and archive `.agentrec/` to a sibling directory. Nothing is ever deleted:
//! a later `agentrec init` starts fresh while the archive remains on disk.

use crate::initcmd::{
    codex_config_toml_path, codex_hooks_json_path, CODEX_HOOK_EVENTS, CODEX_HOOK_MARKER,
    HOOK_MARKER,
};
use crate::{agentrec_dir, service};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run(root: &Path, no_service: bool) -> Result<(), String> {
    let mut actions: Vec<String> = Vec::new();

    if remove_claude_hooks(root)? {
        actions.push("removed agentrec hook entries from .claude/settings.local.json".to_string());
    } else {
        actions.push("no agentrec hook entries found".to_string());
    }

    // C3 "reverse": unconditional (no flag), mirroring the Claude Code
    // removal above — a no-op when Codex hooks were never installed (`init
    // --codex` is itself opt-in, so most repos hit the no-op path).
    match remove_codex_hooks(root) {
        Ok(true) => {
            actions.push("removed agentrec hook entries from Codex hook config".to_string())
        }
        Ok(false) => actions.push("no agentrec Codex hook entries found".to_string()),
        Err(e) => return Err(e),
    }

    if no_service {
        actions.push("skipped service removal (--no-service)".to_string());
    } else {
        actions.extend(service::uninstall(root));
    }

    let dir = agentrec_dir(root);
    if dir.exists() {
        let dest = archive_path(root);
        fs::rename(&dir, &dest).map_err(|e| e.to_string())?;
        actions.push(format!("archived {} to {}", dir.display(), dest.display()));
    } else {
        actions.push("no .agentrec/ directory to archive".to_string());
    }

    for line in &actions {
        println!("{line}");
    }
    println!("nothing was deleted — a later `agentrec init` starts fresh");
    Ok(())
}

/// Remove only agentrec's own hook entries (matched by [`HOOK_MARKER`]) from
/// `UserPromptSubmit`/`Stop`, preserving unrelated hooks and every other key
/// byte-compatibly (no rewrite at all when nothing changes). Now-empty event
/// arrays and an empty `hooks` object are dropped tidily. Returns whether
/// anything changed.
fn remove_claude_hooks(root: &Path) -> Result<bool, String> {
    let path = root.join(".claude").join("settings.local.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(false);
    };
    let mut settings: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        format!(
            "existing {} is not valid JSON ({e}); not touching it",
            path.display()
        )
    })?;

    let mut changed = false;
    // E4: only the specific `UserPromptSubmit`/`Stop` keys that OUR OWN
    // removal just emptied are candidates for deletion — a pre-existing
    // empty array under some other key (or one that was already empty
    // before we touched it) is the user's own placeholder and must survive.
    let mut emptied_by_us: Vec<&str> = Vec::new();
    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for event in ["UserPromptSubmit", "Stop"] {
            if let Some(arr) = hooks.get_mut(event).and_then(|e| e.as_array_mut()) {
                let mut event_changed = false;
                // Command-granular: strip only the agentrec command(s) out of
                // each entry's inner `hooks[]`, preserving any other commands
                // in the same group (e.g. a mixed entry with `prettier`).
                for entry in arr.iter_mut() {
                    if let Some(inner) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                        let inner_before = inner.len();
                        inner.retain(|hook| {
                            !hook
                                .get("command")
                                .and_then(|c| c.as_str())
                                .map(|c| c.contains(HOOK_MARKER))
                                .unwrap_or(false)
                        });
                        if inner.len() != inner_before {
                            event_changed = true;
                        }
                    }
                }
                // Drop an entry only once its inner `hooks[]` is empty;
                // malformed entries with no `hooks[]` array are left as-is.
                let before = arr.len();
                arr.retain(|entry| {
                    entry
                        .get("hooks")
                        .and_then(|h| h.as_array())
                        .map(|inner| !inner.is_empty())
                        .unwrap_or(true)
                });
                if arr.len() != before {
                    event_changed = true;
                }
                if event_changed {
                    changed = true;
                    // Only queue this key for removal if OUR edit is what
                    // left it empty — an array that was already empty before
                    // we touched it never sets `event_changed`, so it's
                    // never queued here.
                    if arr.is_empty() {
                        emptied_by_us.push(event);
                    }
                }
            }
        }
        for event in &emptied_by_us {
            hooks.remove(*event);
        }
    }
    if settings
        .get("hooks")
        .and_then(|h| h.as_object())
        .map(|o| o.is_empty())
        .unwrap_or(false)
    {
        if let Some(obj) = settings.as_object_mut() {
            obj.remove("hooks");
        }
    }

    if changed {
        // Mirror init: back up the pre-rewrite file before overwriting it.
        let backup = path.with_extension("local.json.bak");
        fs::copy(&path, &backup).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        fs::write(&path, text).map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

// ---- C3 "reverse": Codex hook removal --------------------------------

/// Strips agentrec's own entries from BOTH possible Codex hook config
/// locations. Both are checked unconditionally (not just whichever
/// `install_codex_hooks` would currently pick) — a repo that was `init
/// --codex`'d before a config change (e.g. the user later added an inline
/// `[hooks]` table by hand) could carry agentrec's marker in a file
/// `codex_hooks_target` would no longer select, and uninstall's whole job
/// is to find and remove agentrec's own writes regardless of current
/// preference order.
fn remove_codex_hooks(root: &Path) -> Result<bool, String> {
    let json_changed = remove_codex_hooks_json(root)?;
    let toml_changed = remove_codex_hooks_toml(root)?;
    Ok(json_changed || toml_changed)
}

/// Command-granular removal from `.codex/hooks.json`, same shape as
/// `remove_claude_hooks` above but over the 3 Codex events and marker.
fn remove_codex_hooks_json(root: &Path) -> Result<bool, String> {
    let path = codex_hooks_json_path(root);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(false);
    };
    let mut settings: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        format!(
            "existing {} is not valid JSON ({e}); not touching it",
            path.display()
        )
    })?;

    let mut changed = false;
    let mut emptied_by_us: Vec<&str> = Vec::new();
    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (event, _) in CODEX_HOOK_EVENTS {
            if let Some(arr) = hooks.get_mut(*event).and_then(|e| e.as_array_mut()) {
                let mut event_changed = false;
                for entry in arr.iter_mut() {
                    if let Some(inner) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                        let before = inner.len();
                        inner.retain(|hook| {
                            !hook
                                .get("command")
                                .and_then(|c| c.as_str())
                                .map(|c| c.contains(CODEX_HOOK_MARKER))
                                .unwrap_or(false)
                        });
                        if inner.len() != before {
                            event_changed = true;
                        }
                    }
                }
                let before = arr.len();
                arr.retain(|entry| {
                    entry
                        .get("hooks")
                        .and_then(|h| h.as_array())
                        .map(|inner| !inner.is_empty())
                        .unwrap_or(true)
                });
                if arr.len() != before {
                    event_changed = true;
                }
                if event_changed {
                    changed = true;
                    if arr.is_empty() {
                        emptied_by_us.push(event);
                    }
                }
            }
        }
        for event in &emptied_by_us {
            hooks.remove(*event);
        }
    }
    if settings
        .get("hooks")
        .and_then(|h| h.as_object())
        .map(|o| o.is_empty())
        .unwrap_or(false)
    {
        if let Some(obj) = settings.as_object_mut() {
            obj.remove("hooks");
        }
    }

    if changed {
        let backup = path.with_extension("json.bak");
        fs::copy(&path, &backup).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        fs::write(&path, text).map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

/// Same removal, over the inline `[hooks]` table in `.codex/config.toml`.
/// Re-serializes the whole file on a change (same disclosed reformatting
/// tradeoff as `initcmd::install_codex_hooks_toml` — trust is keyed on
/// parsed content per the spike, not raw bytes).
fn remove_codex_hooks_toml(root: &Path) -> Result<bool, String> {
    let path = codex_config_toml_path(root);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(false);
    };
    let mut doc: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        format!(
            "existing {} is not valid TOML ({e}); not touching it",
            path.display()
        )
    })?;

    let mut changed = false;
    if let Some(hooks_table) = doc.get_mut("hooks").and_then(|h| h.as_table_mut()) {
        let mut emptied_by_us: Vec<String> = Vec::new();
        for (event, _) in CODEX_HOOK_EVENTS {
            if let Some(arr) = hooks_table.get_mut(*event).and_then(|e| e.as_array_mut()) {
                let mut event_changed = false;
                for entry in arr.iter_mut() {
                    if let Some(inner) = entry
                        .as_table_mut()
                        .and_then(|t| t.get_mut("hooks"))
                        .and_then(|h| h.as_array_mut())
                    {
                        let before = inner.len();
                        inner.retain(|hook| {
                            !hook
                                .as_table()
                                .and_then(|t| t.get("command"))
                                .and_then(|c| c.as_str())
                                .map(|c| c.contains(CODEX_HOOK_MARKER))
                                .unwrap_or(false)
                        });
                        if inner.len() != before {
                            event_changed = true;
                        }
                    }
                }
                let before = arr.len();
                arr.retain(|entry| {
                    entry
                        .as_table()
                        .and_then(|t| t.get("hooks"))
                        .and_then(|h| h.as_array())
                        .map(|inner| !inner.is_empty())
                        .unwrap_or(true)
                });
                if arr.len() != before {
                    event_changed = true;
                }
                if event_changed {
                    changed = true;
                    if arr.is_empty() {
                        emptied_by_us.push(event.to_string());
                    }
                }
            }
        }
        for event in &emptied_by_us {
            hooks_table.remove(event);
        }
    }
    if doc
        .get("hooks")
        .and_then(|h| h.as_table())
        .map(|t| t.is_empty())
        .unwrap_or(false)
    {
        doc.remove("hooks");
    }

    if changed {
        let backup = path.with_extension("toml.bak");
        fs::copy(&path, &backup).map_err(|e| e.to_string())?;
        let out = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        fs::write(&path, out).map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

/// Sibling archive dir name: `.agentrec.archived.<unix_ts>`.
fn archive_path(root: &Path) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    root.join(format!(".agentrec.archived.{ts}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_marked_entries_preserves_others() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".claude")).unwrap();
        let settings = serde_json::json!({
            "hooks": {
                "Stop": [
                    { "hooks": [ { "type": "command", "command": "other-tool" } ] },
                    { "hooks": [ { "type": "command", "command": "agentrec hook claude" } ] }
                ],
                "UserPromptSubmit": [
                    { "hooks": [ { "type": "command", "command": "agentrec hook claude" } ] }
                ]
            },
            "unrelatedTopLevelKey": true
        });
        fs::write(
            root.join(".claude/settings.local.json"),
            serde_json::to_string_pretty(&settings).unwrap(),
        )
        .unwrap();

        let changed = remove_claude_hooks(root).unwrap();
        assert!(changed);

        let text = fs::read_to_string(root.join(".claude/settings.local.json")).unwrap();
        let after: serde_json::Value = serde_json::from_str(&text).unwrap();
        // UserPromptSubmit had only our hook -> emptied -> key dropped tidily.
        assert!(after["hooks"].get("UserPromptSubmit").is_none());
        // Stop kept the unrelated hook only.
        let stop = after["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        assert_eq!(stop[0]["hooks"][0]["command"].as_str(), Some("other-tool"));
        assert_eq!(after["unrelatedTopLevelKey"], serde_json::json!(true));
    }

    // CONCERN B: command-granular removal — a mixed group entry containing
    // both the agentrec hook and an unrelated command (e.g. prettier) keeps
    // the unrelated command and the entry itself; only the agentrec command
    // is stripped out of the inner hooks[] array.
    #[test]
    fn mixed_hook_group_keeps_unrelated_command_and_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".claude")).unwrap();
        let settings = serde_json::json!({
            "hooks": {
                "Stop": [
                    { "hooks": [
                        { "type": "command", "command": "agentrec hook claude" },
                        { "type": "command", "command": "prettier --write ." }
                    ] }
                ]
            }
        });
        fs::write(
            root.join(".claude/settings.local.json"),
            serde_json::to_string_pretty(&settings).unwrap(),
        )
        .unwrap();

        let changed = remove_claude_hooks(root).unwrap();
        assert!(changed);

        let text = fs::read_to_string(root.join(".claude/settings.local.json")).unwrap();
        let after: serde_json::Value = serde_json::from_str(&text).unwrap();
        let stop = after["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(
            stop.len(),
            1,
            "entry must survive — prettier is still there"
        );
        let inner = stop[0]["hooks"].as_array().unwrap();
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0]["command"].as_str(), Some("prettier --write ."));
    }

    #[test]
    fn removing_all_hooks_drops_hooks_key_entirely() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".claude")).unwrap();
        let settings = serde_json::json!({
            "hooks": {
                "Stop": [ { "hooks": [ { "type": "command", "command": "agentrec hook claude" } ] } ]
            }
        });
        fs::write(
            root.join(".claude/settings.local.json"),
            serde_json::to_string_pretty(&settings).unwrap(),
        )
        .unwrap();

        assert!(remove_claude_hooks(root).unwrap());
        let text = fs::read_to_string(root.join(".claude/settings.local.json")).unwrap();
        let after: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(after.get("hooks").is_none());
    }

    #[test]
    fn no_settings_file_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!remove_claude_hooks(tmp.path()).unwrap());
    }

    // E4: a pre-existing empty array under an UNRELATED hook key (never
    // touched by agentrec, and not one we emptied) must survive uninstall —
    // the old code dropped ANY empty-array hook key, agentrec's own or not.
    #[test]
    fn pre_existing_empty_hook_key_survives_uninstall() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".claude")).unwrap();
        let settings = serde_json::json!({
            "hooks": {
                "PreToolUse": [],
                "Stop": [ { "hooks": [ { "type": "command", "command": "agentrec hook claude" } ] } ]
            }
        });
        fs::write(
            root.join(".claude/settings.local.json"),
            serde_json::to_string_pretty(&settings).unwrap(),
        )
        .unwrap();

        assert!(remove_claude_hooks(root).unwrap());

        let text = fs::read_to_string(root.join(".claude/settings.local.json")).unwrap();
        let after: serde_json::Value = serde_json::from_str(&text).unwrap();
        // Our own Stop entry (which we emptied) is gone...
        assert!(after["hooks"].get("Stop").is_none());
        // ...but the user's own pre-existing empty placeholder survives.
        assert_eq!(
            after["hooks"]["PreToolUse"],
            serde_json::json!([]),
            "pre-existing empty hook key must not be dropped: {after}"
        );
    }

    // E4: uninstall takes a .bak of settings.local.json before rewriting it,
    // mirroring init's own backup-before-merge behavior.
    #[test]
    fn uninstall_backs_up_settings_before_rewrite() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".claude")).unwrap();
        let settings = serde_json::json!({
            "hooks": {
                "Stop": [ { "hooks": [ { "type": "command", "command": "agentrec hook claude" } ] } ]
            }
        });
        let original = serde_json::to_string_pretty(&settings).unwrap();
        fs::write(root.join(".claude/settings.local.json"), &original).unwrap();

        assert!(remove_claude_hooks(root).unwrap());

        // Mirrors initcmd's own `path.with_extension("local.json.bak")`
        // pattern byte-for-byte (including its known "double .local" naming
        // oddity — CLAUDE.md tracks that as deferred, not this round's job).
        let backup_path = root.join(".claude/settings.local.local.json.bak");
        assert!(
            backup_path.exists(),
            "expected a .bak of the pre-rewrite file at {}",
            backup_path.display()
        );
        let backup = fs::read_to_string(&backup_path).unwrap();
        assert_eq!(backup, original, "backup must be the pre-rewrite content");
    }

    #[test]
    fn no_settings_rewrite_when_nothing_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".claude")).unwrap();
        let original = serde_json::to_string_pretty(&serde_json::json!({
            "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "other-tool" } ] } ] }
        }))
        .unwrap();
        fs::write(root.join(".claude/settings.local.json"), &original).unwrap();

        assert!(!remove_claude_hooks(root).unwrap());
        let after = fs::read_to_string(root.join(".claude/settings.local.json")).unwrap();
        assert_eq!(original, after, "untouched file must stay byte-identical");
    }

    // ---- C3 "reverse": Codex hook removal --------------------------------

    #[test]
    fn remove_codex_hooks_json_strips_only_agentrec_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".codex")).unwrap();
        let settings = serde_json::json!({
            "hooks": {
                "Stop": [
                    { "hooks": [ { "type": "command", "command": "other-tool" } ] },
                    { "hooks": [ { "type": "command", "command": "agentrec hook codex", "timeout": 10 } ] }
                ],
                "UserPromptSubmit": [
                    { "hooks": [ { "type": "command", "command": "agentrec hook codex", "timeout": 10 } ] }
                ],
                "PostToolUse": [
                    { "matcher": "apply_patch", "hooks": [ { "type": "command", "command": "agentrec hook codex", "timeout": 10 } ] }
                ]
            }
        });
        fs::write(
            codex_hooks_json_path(root),
            serde_json::to_string_pretty(&settings).unwrap(),
        )
        .unwrap();

        assert!(remove_codex_hooks_json(root).unwrap());

        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(codex_hooks_json_path(root)).unwrap())
                .unwrap();
        let stop = after["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "foreign Stop entry survives");
        assert_eq!(stop[0]["hooks"][0]["command"], "other-tool");
        assert!(after["hooks"].get("UserPromptSubmit").is_none());
        assert!(after["hooks"].get("PostToolUse").is_none());
    }

    #[test]
    fn remove_codex_hooks_toml_strips_only_agentrec_entries_and_preserves_unrelated_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".codex")).unwrap();
        let text = "model = \"o3\"\n\n\
            [[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"other-tool\"\n\n\
            [[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"agentrec hook codex\"\ntimeout = 10\n\n\
            [[hooks.UserPromptSubmit]]\n[[hooks.UserPromptSubmit.hooks]]\ntype = \"command\"\ncommand = \"agentrec hook codex\"\ntimeout = 10\n";
        fs::write(codex_config_toml_path(root), text).unwrap();

        assert!(remove_codex_hooks_toml(root).unwrap());

        let after_text = fs::read_to_string(codex_config_toml_path(root)).unwrap();
        let after: toml::Table = after_text.parse().unwrap();
        assert_eq!(after["model"].as_str(), Some("o3"));
        assert!(after["hooks"]
            .as_table()
            .unwrap()
            .get("UserPromptSubmit")
            .is_none());
        let stop = after["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "foreign Stop entry survives");
        assert_eq!(stop[0]["hooks"][0]["command"].as_str(), Some("other-tool"));
    }

    #[test]
    fn remove_codex_hooks_no_config_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!remove_codex_hooks(tmp.path()).unwrap());
    }

    // End-to-end: install then uninstall via the real entry points, on both
    // possible Codex hook targets.
    #[test]
    fn uninstall_removes_codex_hooks_json_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        crate::initcmd::install_codex_hooks(root).unwrap();
        assert!(codex_hooks_json_path(root).is_file());

        assert!(remove_codex_hooks(root).unwrap());
        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(codex_hooks_json_path(root)).unwrap())
                .unwrap();
        assert!(
            after.get("hooks").is_none(),
            "all three agentrec-only events must be fully removed: {after}"
        );
    }

    #[test]
    fn uninstall_removes_codex_hooks_from_config_toml_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::write(codex_config_toml_path(root), "[hooks]\n").unwrap();
        crate::initcmd::install_codex_hooks(root).unwrap();
        let installed = fs::read_to_string(codex_config_toml_path(root)).unwrap();
        assert!(installed.contains("agentrec hook codex"));

        assert!(remove_codex_hooks(root).unwrap());
        let after_text = fs::read_to_string(codex_config_toml_path(root)).unwrap();
        assert!(!after_text.contains("agentrec hook codex"));
    }

    #[test]
    fn archives_agentrec_dir_without_deleting() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(agentrec_dir(root)).unwrap();
        fs::write(agentrec_dir(root).join("config.toml"), "ttl_days = 90\n").unwrap();

        run(root, true).unwrap();

        assert!(!agentrec_dir(root).exists());
        let archived = fs::read_dir(root)
            .unwrap()
            .flatten()
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".agentrec.archived.")
            })
            .expect("archive dir present");
        assert!(archived.path().join("config.toml").exists());
    }

    #[test]
    fn uninstall_with_no_agentrec_dir_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(run(tmp.path(), true).is_ok());
    }
}
