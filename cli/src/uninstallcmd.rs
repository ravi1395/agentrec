//! `agentrec uninstall` (AC-Y+4): the reverse of `init` — remove agentrec's
//! own Claude Code hook entries, unload+remove the per-repo service unit,
//! and archive `.agentrec/` to a sibling directory. Nothing is ever deleted:
//! a later `agentrec init` starts fresh while the archive remains on disk.

use crate::initcmd::{
    codex_config_toml_path, codex_hooks_json_path, codex_mcp_state, mcp_json_path, mcp_json_state,
    McpRegState, CODEX_HOOK_EVENTS, CODEX_HOOK_MARKER, HOOK_MARKER, MCP_SERVER_NAME,
};
use crate::{agentrec_dir, service};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Read a config file this command may rewrite, refusing a non-regular path.
///
/// `Ok(None)` is "there is nothing here to remove" — the meaning the bare
/// `let Ok(text) = ... else { return Ok(false) }` at each call site already
/// had for an absent or unreadable file, kept unchanged so no ordinary file
/// moves. A path that is not a regular file (fifo, socket, device, directory)
/// is escalated to a named error instead: the bare read BLOCKS forever on a
/// fifo, and the caller would otherwise report "no agentrec entries found"
/// about a file it never managed to look at.
fn read_config_or_skip(path: &Path) -> Result<Option<String>, String> {
    match agentrec_core::fsguard::read_regular_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => Err(format!(
            "cannot read existing {} ({e}); not touching it",
            path.display()
        )),
        Err(_) => Ok(None),
    }
}

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

    // E4: MCP registration, both hosts. Unconditional like the Codex hook
    // removal above — a no-op when nothing was registered.
    match remove_mcp_registrations(root) {
        Ok(true) => actions.push("removed the agentrec MCP server registration".to_string()),
        Ok(false) => actions.push("no agentrec MCP server registration found".to_string()),
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
/// `UserPromptSubmit`/`Stop`, preserving unrelated hooks and every other key.
/// Nothing is rewritten at all when nothing changes — that path IS
/// byte-preserving. When something does change the file is re-serialized
/// whole, so survivors survive as parsed values, not as bytes (keys re-sort,
/// `to_string_pretty` reformats): the same caveat as
/// `initcmd::install_mcp_json`. Now-empty event arrays and an empty `hooks`
/// object are dropped tidily. Returns whether anything changed.
fn remove_claude_hooks(root: &Path) -> Result<bool, String> {
    let path = root.join(".claude").join("settings.local.json");
    let Some(text) = read_config_or_skip(&path)? else {
        return Ok(false);
    };
    let mut settings: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        format!(
            "existing {} is not valid JSON ({e}); not touching it",
            path.display()
        )
    })?;

    let mut changed = false;
    // E4: only the specific keys that OUR OWN removal just emptied are
    // candidates for deletion — a pre-existing empty array under some other
    // key (or one that was already empty before we touched it) is the user's
    // own placeholder and must survive.
    //
    // The event list is `initcmd::CLAUDE_HOOK_EVENTS`, not a second literal:
    // an event installed but not enumerated here is an entry uninstall leaves
    // behind, which is how the `PostToolUse[Bash]` attest-capture hook would
    // have leaked if this had been copied instead of shared.
    let mut emptied_by_us: Vec<&str> = Vec::new();
    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (event, _matcher) in crate::initcmd::CLAUDE_HOOK_EVENTS {
            let event = *event;
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
    let Some(text) = read_config_or_skip(&path)? else {
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
    let Some(text) = read_config_or_skip(&path)? else {
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

// ---- MCP registration removal (E4) ------------------------------------------

/// Remove agentrec's own MCP server registration from both repo-local
/// registries. **Identity is `initcmd::mcp_state_*`'s predicate, not the
/// registry key**: a foreign server occupying the `agentrec` key is left
/// exactly where it is, byte-for-byte, and this function reports no change for
/// it. That is the case a key-only removal would silently eat.
fn remove_mcp_registrations(root: &Path) -> Result<bool, String> {
    let json_changed = remove_mcp_json(root)?;
    let toml_changed = remove_codex_mcp(root)?;
    Ok(json_changed || toml_changed)
}

fn remove_mcp_json(root: &Path) -> Result<bool, String> {
    let path = mcp_json_path(root);
    if mcp_json_state(root)? != McpRegState::Ours {
        return Ok(false);
    }
    let text = agentrec_core::fsguard::read_regular_to_string(&path).map_err(|e| e.to_string())?;
    let mut value: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        format!(
            "existing {} is not valid JSON ({e}); not touching it",
            path.display()
        )
    })?;
    let mut drop_servers_key = false;
    if let Some(servers) = value.get_mut("mcpServers").and_then(|s| s.as_object_mut()) {
        servers.remove(MCP_SERVER_NAME);
        drop_servers_key = servers.is_empty();
    }
    if drop_servers_key {
        if let Some(obj) = value.as_object_mut() {
            obj.remove("mcpServers");
        }
    }
    let backup = path.with_extension("json.bak");
    fs::copy(&path, &backup).map_err(|e| e.to_string())?;
    let out = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    fs::write(&path, out).map_err(|e| e.to_string())?;
    Ok(true)
}

fn remove_codex_mcp(root: &Path) -> Result<bool, String> {
    let path = codex_config_toml_path(root);
    if codex_mcp_state(root)? != McpRegState::Ours {
        return Ok(false);
    }
    let text = agentrec_core::fsguard::read_regular_to_string(&path).map_err(|e| e.to_string())?;
    let mut doc: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        format!(
            "existing {} is not valid TOML ({e}); not touching it",
            path.display()
        )
    })?;
    let mut drop_servers_key = false;
    if let Some(servers) = doc.get_mut("mcp_servers").and_then(|s| s.as_table_mut()) {
        servers.remove(MCP_SERVER_NAME);
        drop_servers_key = servers.is_empty();
    }
    if drop_servers_key {
        doc.remove("mcp_servers");
    }
    let backup = path.with_extension("toml.bak");
    fs::copy(&path, &backup).map_err(|e| e.to_string())?;
    let out = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    fs::write(&path, out).map_err(|e| e.to_string())?;
    Ok(true)
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
// Test code reads its own tempdir fixtures; no attacker-supplied FIFO can
// block these, so the fsguard wrappers buy nothing. Scoped to this module
// so production reads in this file stay lint-enforced (clippy.toml).
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

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

    // Every config path `uninstall` reads. A fifo at any of them made the
    // whole command HANG (a bare `read_to_string` on a fifo blocks until a
    // writer appears, forever), so this test's first value is that it
    // TERMINATES; its second is that the refusal names the file rather than
    // being swallowed into "no agentrec hook entries found".
    #[test]
    #[cfg(unix)]
    fn uninstall_refuses_a_fifo_at_each_config_path() {
        for rel in [
            ".claude/settings.local.json",
            ".codex/hooks.json",
            ".codex/config.toml",
            ".mcp.json",
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            mkfifo_at(&root.join(rel));

            let err = run(root, true).unwrap_err();
            assert!(
                err.contains(rel.rsplit('/').next().unwrap()) && err.contains("not a regular file"),
                "{rel}: refusal must name the file: {err}"
            );
        }
    }

    // The allow half, and the shape that already burned this guard once: a
    // config file relocated behind a symlink (a dotfiles repo) is an ordinary
    // setup and must still be read AND rewritten through the link. A
    // refuse-only suite stays green through an over-refusal that silently
    // turns uninstall into a no-op.
    #[test]
    #[cfg(unix)]
    fn uninstall_reads_and_rewrites_a_symlinked_settings_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let real = tmp.path().join("dotfiles-settings.json");
        let settings = serde_json::json!({
            "hooks": {
                "Stop": [
                    { "hooks": [ { "type": "command", "command": "other-tool" } ] },
                    { "hooks": [ { "type": "command", "command": "agentrec hook claude" } ] }
                ]
            }
        });
        fs::write(&real, serde_json::to_string_pretty(&settings).unwrap()).unwrap();
        fs::create_dir_all(root.join(".claude")).unwrap();
        std::os::unix::fs::symlink(&real, root.join(".claude/settings.local.json")).unwrap();

        assert!(
            remove_claude_hooks(root).unwrap(),
            "a symlinked settings file must still be read and rewritten"
        );

        let after: serde_json::Value = serde_json::from_str(&fs::read_to_string(&real).unwrap())
            .expect("rewrite must land on the symlink target");
        let stop = after["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "only the agentrec entry is removed");
        assert_eq!(stop[0]["hooks"][0]["command"].as_str(), Some("other-tool"));
    }

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

    // ---- MCP registration removal (E4) ------------------------------------

    /// AC-E4 round trip: a foreign server pre-seeded in BOTH registries
    /// survives `init` + `uninstall` untouched, while our own entry is added
    /// and then removed. BOTH files are asserted parsed-value-identical, not
    /// byte-identical: each is re-serialized whole when we write it, so keys
    /// re-sort and formatting normalizes (`serde_json`'s `Map` is a
    /// `BTreeMap` + `to_string_pretty`; the `toml` crate reformats the same
    /// way, an existing documented property of `install_codex_hooks_toml`
    /// that path inherits). Byte-identity would be a false claim on either.
    /// The `.mcp.json` fixture is seeded out of alphabetical order and
    /// compact so that the reordering is inside the test's reach — under a
    /// canonically seeded fixture even the old byte assertion passed.
    #[test]
    fn init_uninstall_round_trip_leaves_foreign_mcp_registrations_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".codex")).unwrap();

        // Seeded in NON-canonical form on purpose: keys out of alphabetical
        // order and compact (not pretty). A canonically seeded fixture would
        // pass a byte assertion for the wrong reason — it cannot observe the
        // `BTreeMap` re-sort and the `to_string_pretty` reformat that
        // `install_mcp_json` actually performs.
        let json_before = "{\"zzz_note\":\"keep me\",\
                           \"mcpServers\":{\"other\":{\"command\":\"other-tool\",\"args\":[\"serve\"]}},\
                           \"aaa_note\":\"keep me too\"}";
        fs::write(crate::initcmd::mcp_json_path(root), json_before).unwrap();
        fs::write(
            codex_config_toml_path(root),
            "[mcp_servers.other]\ncommand = \"other-tool\"\nargs = [\"serve\"]\n",
        )
        .unwrap();
        let toml_before: toml::Table = fs::read_to_string(codex_config_toml_path(root))
            .unwrap()
            .parse()
            .unwrap();

        crate::initcmd::install_mcp_json(root).unwrap();
        crate::initcmd::install_codex_mcp(root).unwrap();
        assert_eq!(
            crate::initcmd::mcp_json_state(root).unwrap(),
            McpRegState::Ours
        );
        assert_eq!(
            crate::initcmd::codex_mcp_state(root).unwrap(),
            McpRegState::Ours
        );

        assert!(remove_mcp_registrations(root).unwrap());
        assert_eq!(
            crate::initcmd::mcp_json_state(root).unwrap(),
            McpRegState::Absent
        );
        assert_eq!(
            crate::initcmd::codex_mcp_state(root).unwrap(),
            McpRegState::Absent
        );

        let json_after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(crate::initcmd::mcp_json_path(root)).unwrap())
                .unwrap();
        assert_eq!(
            json_after,
            serde_json::from_str::<serde_json::Value>(json_before).unwrap(),
            ".mcp.json must return to its pre-init parsed value"
        );
        let toml_after: toml::Table = fs::read_to_string(codex_config_toml_path(root))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(toml_after, toml_before);
    }

    /// The discriminating case: a FOREIGN server sitting on the `agentrec`
    /// key. `uninstall` must not remove it. A key-only identity predicate
    /// deletes it here and passes every other test in this file.
    #[test]
    fn uninstall_does_not_remove_a_foreign_server_named_agentrec() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".codex")).unwrap();
        let json_before = serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": {"agentrec": {"command": "other-tool", "args": ["mcp"]}}
        }))
        .unwrap();
        fs::write(crate::initcmd::mcp_json_path(root), &json_before).unwrap();
        let toml_before = "[mcp_servers.agentrec]\ncommand = \"other-tool\"\nargs = [\"mcp\"]\n";
        fs::write(codex_config_toml_path(root), toml_before).unwrap();

        assert!(
            !remove_mcp_registrations(root).unwrap(),
            "nothing of ours is registered, so nothing may be removed"
        );
        assert_eq!(
            fs::read_to_string(crate::initcmd::mcp_json_path(root)).unwrap(),
            json_before
        );
        assert_eq!(
            fs::read_to_string(codex_config_toml_path(root)).unwrap(),
            toml_before
        );
    }

    #[test]
    fn remove_mcp_registrations_with_no_files_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!remove_mcp_registrations(tmp.path()).unwrap());
    }
}
