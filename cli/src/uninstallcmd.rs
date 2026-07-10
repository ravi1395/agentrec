//! `agentrec uninstall` (AC-Y+4): the reverse of `init` — remove agentrec's
//! own Claude Code hook entries, unload+remove the per-repo service unit,
//! and archive `.agentrec/` to a sibling directory. Nothing is ever deleted:
//! a later `agentrec init` starts fresh while the archive remains on disk.

use crate::initcmd::HOOK_MARKER;
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
    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for event in ["UserPromptSubmit", "Stop"] {
            if let Some(arr) = hooks.get_mut(event).and_then(|e| e.as_array_mut()) {
                let before = arr.len();
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
                            changed = true;
                        }
                    }
                }
                // Drop an entry only once its inner `hooks[]` is empty;
                // malformed entries with no `hooks[]` array are left as-is.
                arr.retain(|entry| {
                    entry
                        .get("hooks")
                        .and_then(|h| h.as_array())
                        .map(|inner| !inner.is_empty())
                        .unwrap_or(true)
                });
                if arr.len() != before {
                    changed = true;
                }
            }
        }
        hooks.retain(|_, v| !v.as_array().map(|a| a.is_empty()).unwrap_or(false));
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
        let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        fs::write(&path, text).map_err(|e| e.to_string())?;
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
