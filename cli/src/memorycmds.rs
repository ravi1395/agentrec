//! `agentrec remember` (memory v1 Task 4): manual, human-authored pinned
//! memories. Validates every `--from` path and hashes it into a `Pin` before
//! anything is written; the first invalid path aborts with no disk mutation.
//! The fact is scrubbed at this layer (not only inside `append_memory`) so
//! the empty-after-scrub refusal happens before a `MemoryRecord` is even
//! built, and only the scrubbed text — never the raw one — is ever placed
//! into the record.

use agentrec_core::memory::{self, MemoryOp, MemoryRecord, Pin};
use agentrec_core::{id, scrub};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// `from` is a comma-separated list of repo-relative paths. Each is
/// validated (`memory::validate_pin_path`) and hashed (`memory::hash_pin`)
/// in order — the first failure returns immediately, naming the offending
/// path, with nothing written to `memory.jsonl`.
pub fn remember(root: &Path, fact: &str, from: &str) -> Result<(), String> {
    let mut pins = Vec::new();
    for raw in from.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let rel = memory::validate_pin_path(root, raw)?;
        let hash = memory::hash_pin(root, &rel)?;
        pins.push(Pin { path: rel, hash });
    }
    if pins.is_empty() {
        return Err("--from must name at least one path".to_string());
    }

    let scrubbed_fact = scrub::scrub(fact);
    if scrubbed_fact.trim().is_empty() {
        return Err("fact scrubbed to empty — refusing to store an empty husk".to_string());
    }

    let rec = MemoryRecord {
        v: 1,
        kind: "memory".to_string(),
        id: id::ulid(),
        op: MemoryOp::Assert,
        fact: scrubbed_fact,
        pins,
        source_turns: vec![],
        origin: "human".to_string(),
        ts: wall_now_ms(),
        reason: None,
    };
    memory::append_memory(root, &rec)
}

/// Mirrors the same one-line helper repeated across `cmds.rs`/`purgecmd.rs`/
/// `daemon.rs`/`readcmds.rs` — a 3-line `SystemTime` call, not worth sharing.
fn wall_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_when_from_has_no_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        let err = remember(root, "a fact", "").unwrap_err();
        assert!(err.contains("--from"), "{err}");
        assert!(
            !memory::memory_path(root).exists(),
            "nothing written on empty --from"
        );
    }

    #[test]
    fn rejects_first_bad_pin_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        let err = remember(root, "a fact", "/etc/passwd").unwrap_err();
        assert!(err.contains("/etc/passwd"), "{err}");
        assert!(!memory::memory_path(root).exists());
    }

    #[test]
    fn writes_a_valid_record() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        std::fs::write(root.join("a.rs"), b"fn a() {}").unwrap();
        remember(root, "a plain fact about a.rs", "a.rs").unwrap();
        let effective = memory::load_effective(root).unwrap();
        assert_eq!(effective.len(), 1);
        assert_eq!(effective[0].origin, "human");
        assert_eq!(effective[0].pins[0].path, "a.rs");
    }
}
