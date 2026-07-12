//! Recorder operational state (`state.json`): daemon lock pid, signal-tailer
//! offset, and the snapshot-failure taxonomy (D35 / AC M+). This is
//! OPERATIONAL data only — never part of the wire log/protocol format.

use crate::state_path;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Default)]
pub struct State {
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub signal_offset: u64,
    /// Count of genuine I/O write failures while snapshotting (never
    /// over-cap skips — those are expected and not "degraded").
    #[serde(default)]
    pub snapshot_failures: u64,
    /// Repo-relative paths whose snapshot hit an I/O failure, for undo's
    /// per-file "no snapshot available" message later.
    #[serde(default)]
    pub io_failed: Vec<String>,
    /// Count of memory-candidate signals the daemon rejected at ingestion
    /// (invalid pin, oversize/scrub-empty fact, out-of-bounds pin count) —
    /// the `snapshot_failures` honesty pattern: a rejection leaves no trace
    /// in `memory.jsonl`, so this counter is the only evidence it happened.
    #[serde(default)]
    pub memory_rejects: u64,
    /// Count of signal lines dropped because their `type` (PROTOCOL §4) was
    /// present but not one this daemon recognizes (F5) — forward-compat
    /// tolerance, not an error. `memory-candidate` is the only recognized
    /// `type` today and is excluded; every other populated `type` increments
    /// this. The `memory_rejects` honesty pattern: a dropped signal leaves no
    /// trace in `log.jsonl`, so this counter is the only visible evidence it
    /// happened.
    #[serde(default)]
    pub unknown_signal_ignored: u64,
}

pub fn read_state(root: &Path) -> State {
    std::fs::read_to_string(state_path(root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Persist `state` atomically (tmp+rename): a crash mid-write must not leave
/// a torn state.json that parses as default (pid 0 → lost lock; offset 0 →
/// replayed inbox). D2: the tmp name is unique per writing process
/// (`state.json.tmp.<pid>`) — the daemon and a concurrent `status
/// --ack-degraded` (or two racing writers) must never share one tmp path and
/// clobber each other's in-flight write. Returns the write/rename error
/// instead of swallowing it, so a caller for whom persistence is load-bearing
/// (the lock-acquire path) can treat failure as fatal.
pub fn write_state(root: &Path, state: &State) -> std::io::Result<()> {
    let text = serde_json::to_string(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let path = state_path(root);
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, &text)?;
    // Lock down before the rename makes it visible under its final name
    // (D37) — no window where state.json is reachable at 0644.
    agentrec_core::perms::lock_file(&tmp);
    std::fs::rename(&tmp, &path)
}

/// Record a genuine snapshot I/O failure: bumps the counter and tracks the
/// path (deduped) so `status` can name it and undo can refuse it later.
pub fn record_io_failure(state: &mut State, rel_path: &str) {
    state.snapshot_failures += 1;
    if !state.io_failed.iter().any(|p| p == rel_path) {
        state.io_failed.push(rel_path.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_io_failure_increments_and_dedups() {
        let mut state = State::default();
        record_io_failure(&mut state, "src/a.rs");
        record_io_failure(&mut state, "src/b.rs");
        record_io_failure(&mut state, "src/a.rs"); // repeat — dedup path, still counts
        assert_eq!(state.snapshot_failures, 3);
        assert_eq!(
            state.io_failed,
            vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
        );
    }

    #[test]
    fn state_round_trips_through_serde() {
        let mut state = State::default();
        record_io_failure(&mut state, "src/a.rs");
        state.pid = 42;
        state.signal_offset = 100;

        let text = serde_json::to_string(&state).unwrap();
        let back: State = serde_json::from_str(&text).unwrap();
        assert_eq!(back.pid, 42);
        assert_eq!(back.signal_offset, 100);
        assert_eq!(back.snapshot_failures, 1);
        assert_eq!(back.io_failed, vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn missing_fields_default_on_deserialize() {
        // Forward-compat: an old state.json without the new fields must not
        // fail to parse.
        let state: State = serde_json::from_str(r#"{"pid":7,"signal_offset":3}"#).unwrap();
        assert_eq!(state.pid, 7);
        assert_eq!(state.snapshot_failures, 0);
        assert!(state.io_failed.is_empty());
    }
}
