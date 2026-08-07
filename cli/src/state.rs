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
    /// Count of file-change events skipped because the OS path's raw bytes
    /// are not valid UTF-8 (`agentrec_core::pathenc::utf8_path` returned
    /// `None` — rare, Linux-only in practice). Records serialize paths as
    /// JSON strings, so such a path can never round-trip; a lossy
    /// `to_string_lossy()` conversion would silently record a different
    /// path than the one that actually changed, breaking undo/blame hash
    /// lookups keyed on that string. No path list here (unlike
    /// `io_failed`) — the whole problem is that the path has no valid
    /// String form to store. The `snapshot_failures` honesty pattern: a
    /// skip leaves no trace in `log.jsonl`, so this counter is the only
    /// visible evidence it happened.
    #[serde(default)]
    pub non_utf8_path_skips: u64,
    /// Count of genuine I/O failures storing a turn's PROMPT blob (D35 gap
    /// closure — the taxonomy originally covered only file snapshots).
    /// Deliberately a SEPARATE counter from `snapshot_failures`/`io_failed`:
    /// those are file-scoped (a path can be named and refused at undo time),
    /// a prompt failure is turn-scoped and has no file path to track. A
    /// failed prompt put leaves `prompt_ref: null` on the closed turn —
    /// indistinguishable on the wire from a turn that simply had no prompt
    /// — so this counter (plus the `status` DEGRADED banner) is the only
    /// evidence the write was attempted and failed, not silently absent.
    #[serde(default)]
    pub prompt_put_failures: u64,
    /// Count of times the daemon rebuilt its in-memory `IgnoreSet` after a
    /// `.gitignore` change (Phase 1 of the rebuild-gate fix, `0a7b279`, made
    /// this happen reliably; this counter makes its RATE observable rather
    /// than theoretical — see `daemon::run`'s loop-tick comment for why that
    /// rate matters). Same `snapshot_failures` honesty pattern as every
    /// other counter here: nothing else on disk shows whether a filter
    /// reload ever happened, so a user cannot otherwise tell "my new rule is
    /// active" from "the daemon is still running last week's rules".
    /// OPERATIONAL state only — never part of the PROTOCOL wire format
    /// (PROTOCOL §5 deliberately keeps `state.json` off the wire).
    #[serde(default)]
    pub ignore_rebuilds: u64,
    /// Wall-clock ms of the most recent ignore-set rebuild, or 0 when
    /// `ignore_rebuilds` is 0 (never happened). Same operational, off-wire
    /// posture as `ignore_rebuilds`.
    #[serde(default)]
    pub last_ignore_rebuild_ms: u64,
    /// Ignore-set rebuilds since the CURRENT daemon epoch started (Phase 3,
    /// honesty-fixes round — open question 1 answered as option (a)).
    /// `status` renders THIS figure, never `ignore_rebuilds`: a long-lived
    /// repo would otherwise eventually render `reloaded 4821 time(s)` in a
    /// daily-driver surface whose line budget is contested. `ignore_rebuilds`
    /// itself is untouched by the reset below and keeps accumulating — it is
    /// real lifetime history and must not be destroyed.
    #[serde(default)]
    pub epoch_ignore_rebuilds: u64,
    /// Collision-resistant identity of the CURRENT daemon epoch. Stamped by
    /// `acquire_lock` (daemon.rs) at the moment a fresh epoch begins, using
    /// the same ULID machinery `agentrec_core::id::ulid()` already uses for
    /// turn ids (48-bit wall-clock ms + 80 random bits) — deliberately NOT
    /// the pid. Pid is reused by the OS over a long-lived machine (the same
    /// recycling class `doctor`'s daemon-liveness check already handles via
    /// flock rather than pid comparison); keying epoch identity on pid let a
    /// later epoch that happened to reuse a dead epoch's pid inherit its
    /// stale reload count. A bare wall-clock-ms stamp alone could still
    /// collide if two epochs started within the same millisecond (e.g. rapid
    /// record/stop/record cycles in a test loop); the ULID's random suffix
    /// rules that out. Empty string is the sentinel for "no epoch is
    /// currently live" — `release_lock` clears it back to empty on a clean
    /// stop, and it is also what a pre-this-field `state.json` deserializes
    /// to (never confusable with a real epoch: `ulid()` never returns an
    /// empty string). Not surfaced to `status`/`--json`; purely bookkeeping.
    #[serde(default)]
    pub epoch_nonce: String,
    /// The `epoch_nonce` `epoch_ignore_rebuilds` was last reset for (replaces
    /// the old pid-keyed `epoch_pid` field for the same reason described on
    /// `epoch_nonce` above). Not surfaced to `status`/`--json`; purely
    /// bookkeeping.
    #[serde(default)]
    pub epoch_reload_nonce: String,
    /// Count of individual `state.json` FIELDS that failed to parse and fell
    /// back to their default (Phase 2, honesty-fixes round). Distinct from
    /// every other counter here in one way: it counts a failure in reading
    /// this very struct, not a failure in some other subsystem. Persisted
    /// like the rest — a corrupt field heals itself the next time any code
    /// path calls `write_state` (the in-memory default gets serialized back),
    /// so this counter is the only durable evidence the corruption ever
    /// happened once that heal fires.
    #[serde(default)]
    pub state_parse_failures: u64,
    /// Name of the last field that failed to parse (e.g. `"signal_offset"`),
    /// or the sentinel below when the whole file was unreadable/not JSON.
    /// `None` only when `state_parse_failures` is 0.
    #[serde(default)]
    pub last_bad_field: Option<String>,
    /// The `epoch_nonce` the watcher has actually finished arming for
    /// (residuals round, Phase 4). `acquire_lock` stamps `epoch_nonce` ~4.3ms
    /// BEFORE the watcher is armed (`daemon::run`'s `.watch()` call) — a test
    /// helper that treats a non-zero `pid` alone as "daemon ready" nominally
    /// races the watcher, silently dropping any event emitted in that window.
    /// `daemon::stamp_watcher_armed` sets this to the CURRENT `epoch_nonce`
    /// immediately after `.watch()` returns `Ok`; a caller is safe to assume
    /// the watcher is listening only once `watcher_armed_nonce ==
    /// epoch_nonce`. Keyed on the nonce rather than a bare bool for the same
    /// reason `epoch_reload_nonce` is: a stale value left by a crashed prior
    /// run names a DEAD epoch's nonce, so it can never equal a fresh epoch's
    /// nonce and is self-invalidating by construction — no explicit reset
    /// needed beyond `release_lock` clearing it on a clean stop. Empty string
    /// (the shared default with every other nonce field here) means "not
    /// armed / unknown", including for a pre-this-field `state.json`.
    #[serde(default)]
    pub watcher_armed_nonce: String,
    /// Count of clean dedup-hit verification reads on the daemon's
    /// snapshot path (perf-evidence round): every `put_result` call inside
    /// `Recorder::stage`, including the symlink-target put — `persist`'s
    /// and `recover_orphan`'s prompt `put_result` calls are the only
    /// deliberately-uncounted ones. **Epoch-scoped, not a lifetime total:**
    /// this field MIRRORS `Recorder::dedup_hits` at each
    /// `drain_recorder_stats` call (an overwrite, not an accumulation), so
    /// after a daemon restart the previous epoch's figure stays here,
    /// unchanged, until the new epoch's first dedup hit overwrites it —
    /// there is no epoch-nonce gate on this pair the way there is on the
    /// ignore-reload counters. A pre-instrumentation `state.json` has no
    /// such key and renders 0, same `#[serde(default)]` posture as every
    /// other counter here.
    #[serde(default)]
    pub dedup_hits: u64,
    /// Total bytes re-read across all hits counted by `dedup_hits`
    /// (Decision 6: the corrupt-fallthrough heal path never contributes —
    /// see `agentrec_core::store::PutResult::Stored`'s doc).
    #[serde(default)]
    pub dedup_reread_bytes: u64,
    /// Dedup key of the most recently PROCESSED start/stop signal that
    /// carried a PROTOCOL §4 `emitter_turn` (Phase 2 tail C1):
    /// `"{tool}\u{1}{event}\u{1}{session}\u{1}{emitter_turn}"`. `None` when
    /// no such signal has been processed yet — including every daemon that
    /// predates this field, and every repo whose emitter never sets
    /// `emitter_turn` (Claude Code today), which never writes this field at
    /// all. Restart-safe by construction (persisted in `state.json`, not
    /// engine memory): the emitter can resend the exact signal it already
    /// sent (its own retry, or a resend racing a daemon restart) and the
    /// daemon recognizes the repeat by this key rather than reopening a
    /// second bracket for it. Single-slot, deliberately: this catches an
    /// immediately-following resend of the last processed signal, not an
    /// arbitrary-history duplicate — a genuinely different signal arriving
    /// in between clears the slot. Mirrors `SignalTailer::poll`'s own
    /// mark-before-apply posture (this key is written as soon as a signal is
    /// recognized as new, before `apply_signal` runs) — never-duplicate over
    /// never-lose, the same tradeoff `resync_shrunk_signal_offset`'s doc
    /// comment already makes for `signal_offset`.
    #[serde(default)]
    pub last_emitter_turn_key: Option<String>,
    /// Count of start/stop signals dropped as a resend of
    /// `last_emitter_turn_key` (Phase 2 tail C1) — the `memory_rejects`
    /// honesty pattern: a dropped resend leaves no trace in `log.jsonl`, so
    /// this counter is the only visible evidence it happened.
    #[serde(default)]
    pub duplicate_emitter_turn_signals: u64,
    /// Count of stop signals whose `emitter_turn` mismatched the currently
    /// open bracket's own (Phase 2 tail C1) — the bracket was left open
    /// rather than closed; same honesty pattern as every counter above.
    #[serde(default)]
    pub mismatched_stop_emitter_turns: u64,
    /// Content fingerprint of the signal `last_emitter_turn_key` was last
    /// set for (Phase 2 tail, C1 fix 1 — content-aware dedup). Identity
    /// alone (`last_emitter_turn_key`) cannot distinguish a genuine emitter
    /// RETRY (byte-identical resend) from a genuine SECOND firing that
    /// happens to share the same `(tool, event, session, emitter_turn)`
    /// tuple: Codex's `Stop` hook fires twice for one `turn_id` on a
    /// `decision:"block"` continuation (`docs/verify/codex-spike.md`,
    /// "Continuation semantics"), and the second firing can carry
    /// genuinely NEW `files_written` from `apply_patch` calls made during
    /// the continuation. `daemon.rs::emitter_turn_content_fingerprint`
    /// computes what this holds for each event kind. `None` when no key
    /// has been recorded yet, OR when the currently-stored key predates
    /// this field (every pre-fix `state.json`, which has
    /// `last_emitter_turn_key` but never wrote this one): a key match
    /// against a `None` fingerprint is treated the same as the pre-fix
    /// behavior — identity alone means duplicate — rather than risk
    /// double-applying a genuine crash-restart resend in the one-time
    /// window right after a binary upgrade. That comparison ALSO stamps
    /// this field with the incoming signal's fingerprint before returning
    /// (see `handle_emitter_turn_signal`), so the gap self-heals on this
    /// exact occurrence, not just on some later non-duplicate signal.
    #[serde(default)]
    pub last_emitter_turn_fingerprint: Option<String>,
}

/// Sentinel `last_bad_field` value for a file that could not be parsed as a
/// JSON object at all (unreadable, truncated, or valid JSON of the wrong
/// shape) — there is no single field name to blame.
const WHOLE_FILE_SENTINEL: &str = "<state.json: unreadable or not a JSON object>";

/// Read `state.json`, degrading PER FIELD rather than resetting the whole
/// struct on one bad value (Phase 2, honesty-fixes round). A single corrupt
/// field — e.g. `signal_offset` written as a string — must cost only that
/// field: every sibling field (`pid`, `snapshot_failures`, `io_failed`, ...)
/// keeps its real persisted value. This matters most for `signal_offset`
/// itself: resetting it to 0 on an unrelated field's corruption would replay
/// the entire `signal.jsonl` hook inbox from byte zero.
///
/// `#[serde(default)]` alone (the taken plan decision) only rescues a field
/// that is MISSING from the JSON — a struct-level `serde_json::from_str::
/// <State>` still fails outright the instant one PRESENT field has the wrong
/// type (e.g. a string where a `u64` is expected), which is exactly the shape
/// every acceptance criterion here needs to survive. So this parses into a
/// generic `serde_json::Value` first and converts each field independently,
/// falling back to that field's `Default` and counting the miss. This is the
/// case flagged in the plan's open question: per-field `#[serde(default)]`
/// alone cannot cover a wrong-TYPE field, only a missing one.
///
/// A missing `state.json` (first run, nothing to degrade) returns plain
/// defaults with no counter bump — that is normal, not a failure. A file that
/// exists but is unreadable, or whose content is not a JSON object at all, is
/// genuinely unrecoverable field-by-field; it still degrades to defaults, but
/// counts as exactly one failure (`state_parse_failures = 1`,
/// `last_bad_field` = the whole-file sentinel) rather than being silent.
pub fn read_state(root: &Path) -> State {
    let text = match agentrec_core::fsguard::read_regular_to_string(&state_path(root)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return State::default(),
        Err(_) => return whole_file_failure(), // exists but unreadable (e.g. permissions)
    };
    let obj = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(obj)) => obj,
        _ => return whole_file_failure(), // not JSON, or valid JSON that isn't an object
    };

    let mut failures = 0u64;
    let mut last_bad: Option<String> = None;
    macro_rules! field {
        ($name:literal) => {
            match obj.get($name) {
                None => Default::default(),
                Some(v) => match serde_json::from_value(v.clone()) {
                    Ok(parsed) => parsed,
                    Err(_) => {
                        failures += 1;
                        last_bad = Some($name.to_string());
                        Default::default()
                    }
                },
            }
        };
    }

    let mut state = State {
        pid: field!("pid"),
        signal_offset: field!("signal_offset"),
        snapshot_failures: field!("snapshot_failures"),
        io_failed: field!("io_failed"),
        memory_rejects: field!("memory_rejects"),
        unknown_signal_ignored: field!("unknown_signal_ignored"),
        non_utf8_path_skips: field!("non_utf8_path_skips"),
        prompt_put_failures: field!("prompt_put_failures"),
        ignore_rebuilds: field!("ignore_rebuilds"),
        last_ignore_rebuild_ms: field!("last_ignore_rebuild_ms"),
        state_parse_failures: field!("state_parse_failures"),
        last_bad_field: field!("last_bad_field"),
        epoch_ignore_rebuilds: field!("epoch_ignore_rebuilds"),
        epoch_nonce: field!("epoch_nonce"),
        epoch_reload_nonce: field!("epoch_reload_nonce"),
        watcher_armed_nonce: field!("watcher_armed_nonce"),
        dedup_hits: field!("dedup_hits"),
        dedup_reread_bytes: field!("dedup_reread_bytes"),
        last_emitter_turn_key: field!("last_emitter_turn_key"),
        duplicate_emitter_turn_signals: field!("duplicate_emitter_turn_signals"),
        mismatched_stop_emitter_turns: field!("mismatched_stop_emitter_turns"),
        last_emitter_turn_fingerprint: field!("last_emitter_turn_fingerprint"),
    };

    // Accumulate onto whatever count was already persisted (itself read
    // tolerantly above) — same accumulation pattern as `record_io_failure`.
    state.state_parse_failures += failures;
    if last_bad.is_some() {
        state.last_bad_field = last_bad;
    }
    state
}

fn whole_file_failure() -> State {
    State {
        state_parse_failures: 1,
        last_bad_field: Some(WHOLE_FILE_SENTINEL.to_string()),
        ..State::default()
    }
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
    // Write-side fsguard mirror: `fs::write` opens create+truncate, which
    // blocks forever on a FIFO pre-created at the tmp name. The pid makes the
    // name predictable enough to plant one, and this runs on the daemon's
    // hot path.
    if agentrec_core::fsguard::is_nonregular(&tmp) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "{} is not a regular file — refusing to write",
                tmp.display()
            ),
        ));
    }
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

/// Record a file-change event skipped for having a non-UTF8 path. No path
/// argument (there is no valid `String` to take — see the field doc).
pub fn record_non_utf8_path_skip(state: &mut State) {
    state.non_utf8_path_skips += 1;
}

/// Record a genuine PROMPT blob I/O failure (D35 gap closure). Bumps its own
/// counter — never `record_io_failure`'s — so a prompt-store failure and a
/// file-snapshot failure are never conflated in `status`'s DEGRADED banner
/// or in `doctor`'s reading of the same state.
pub fn record_prompt_put_failure(state: &mut State) {
    state.prompt_put_failures += 1;
}

/// Record a completed `IgnoreSet` rebuild: bumps the lifetime and epoch
/// counters and stamps the wall-clock time it happened, so `status` can
/// render "reloaded N time(s), last ... ago" — and print nothing at all when
/// the epoch counter is still 0 (never a vacuous "0 reloads" line).
///
/// Epoch detection (Phase 3, revised in the honesty round to use a nonce
/// instead of the pid — see `State::epoch_nonce`'s doc for why pid identity
/// was unsafe): if `state.epoch_nonce` (set by `acquire_lock` before
/// anything else runs in a fresh daemon process) no longer matches
/// `state.epoch_reload_nonce` (the nonce the epoch counter was last reset
/// for), a new daemon epoch has begun since the last rebuild — reset
/// `epoch_ignore_rebuilds` to 0 and adopt the new nonce before incrementing.
/// `ignore_rebuilds` (lifetime) is never reset.
pub fn record_ignore_rebuild(state: &mut State, wall_ms: u64) {
    if state.epoch_reload_nonce != state.epoch_nonce {
        state.epoch_ignore_rebuilds = 0;
        state.epoch_reload_nonce = state.epoch_nonce.clone();
    }
    state.ignore_rebuilds += 1;
    state.epoch_ignore_rebuilds += 1;
    state.last_ignore_rebuild_ms = wall_ms;
}

/// The genuinely-current-epoch reload count, for every reader of `state.json`
/// (`status` text and `status --json` alike) — not just `record_ignore_
/// rebuild`'s writer side.
///
/// The reset in `record_ignore_rebuild` above only fires the next time a
/// rebuild happens; it does nothing at the moment a new epoch actually
/// *starts* (`acquire_lock` stamping a fresh `state.epoch_nonce`), and
/// nothing at all while the daemon is stopped. In both windows,
/// `epoch_ignore_rebuilds` and `epoch_reload_nonce` on disk still describe
/// whichever epoch last rebuilt — which may be a dead epoch (possibly one
/// whose pid has since been reused by an unrelated process, or reused by a
/// later agentrec daemon epoch — pid identity cannot distinguish the two),
/// or (when stopped) `state.epoch_nonce == ""` while `epoch_reload_nonce` is
/// still the last live epoch's nonce. A reader that used
/// `state.epoch_ignore_rebuilds` directly would attribute that stale epoch's
/// reloads to "now".
///
/// So every reader must ask the same question `record_ignore_rebuild` asks
/// before trusting the field: does `epoch_reload_nonce` still match the
/// CURRENT `epoch_nonce`? If not, no rebuild has happened in the current
/// epoch yet, and the true current-epoch count is 0 — not "unknown", not the
/// stale figure. The empty-string sentinel is checked explicitly (`!state.
/// epoch_nonce.is_empty()`), not left to fall out of the equality check
/// alone: a `state.json` written by a pre-nonce binary has neither field at
/// all, so BOTH `epoch_nonce` and `epoch_reload_nonce` deserialize to their
/// shared default `""` and would compare equal — while `epoch_ignore_
/// rebuilds` could still hold a real accumulated figure from that older
/// binary's pid-keyed bookkeeping. Without the explicit non-empty check,
/// that stale figure would render as "current".
///
/// Residuals round, Phase 3: this function CANNOT close the crashed-daemon
/// window by itself — `release_lock` never runs on `kill -9`, so a dead
/// epoch's `epoch_nonce`/`epoch_reload_nonce` stay matched on disk exactly
/// as if the epoch were still live, and this function has no `root` to probe
/// liveness with. Callers (`status` text and `status --json` alike) MUST
/// additionally gate on an actual liveness check (`daemon::daemon_is_running`)
/// before trusting this return value as "the current daemon's count" — this
/// function only answers "what does the on-disk epoch bookkeeping say",
/// never "is anything actually running".
pub fn current_epoch_reloads(state: &State) -> u64 {
    if !state.epoch_nonce.is_empty() && state.epoch_reload_nonce == state.epoch_nonce {
        state.epoch_ignore_rebuilds
    } else {
        0
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
        assert_eq!(state.non_utf8_path_skips, 0);
        assert_eq!(state.prompt_put_failures, 0);
        assert_eq!(state.ignore_rebuilds, 0);
        assert_eq!(state.last_ignore_rebuild_ms, 0);
        assert_eq!(state.epoch_ignore_rebuilds, 0);
        assert_eq!(state.epoch_nonce, "");
        assert_eq!(state.epoch_reload_nonce, "");
        assert_eq!(state.watcher_armed_nonce, "");
    }

    // Phase 3 (honesty-fixes round), revised in the follow-up honesty round:
    // the epoch counter must reset when the EPOCH NONCE changes (simulating
    // a daemon restart via `acquire_lock` stamping a fresh
    // `state.epoch_nonce`), NOT the pid — keying on pid let a later epoch
    // that reused a dead epoch's pid (real over a long-lived machine)
    // inherit its stale count; see `State::epoch_nonce`'s doc. The lifetime
    // counter keeps accumulating across the epoch boundary rather than
    // resetting too. Sibling non-default value pinned per the vacuity trap:
    // `ignore_rebuilds` (5, non-zero) is asserted in the SAME test as
    // `epoch_ignore_rebuilds` reading a smaller, epoch-only figure (2) — a
    // version of `record_ignore_rebuild` that never resets would show 5 for
    // both. Pid is deliberately left UNCHANGED (111 throughout) to prove the
    // reset is keyed on the nonce, not on pid — this is the exact reused-pid
    // shape the fix exists for. Neuter: key the reset back on `state.pid`
    // (compare `epoch_pid` again) → RED (pid never changes here, so a
    // pid-keyed version never resets).
    #[test]
    fn record_ignore_rebuild_resets_epoch_counter_on_nonce_change() {
        let mut state = State {
            pid: 111,
            epoch_nonce: "epoch-a".to_string(),
            ..State::default()
        };
        record_ignore_rebuild(&mut state, 1_000);
        record_ignore_rebuild(&mut state, 2_000);
        record_ignore_rebuild(&mut state, 3_000);
        assert_eq!(state.ignore_rebuilds, 3);
        assert_eq!(state.epoch_ignore_rebuilds, 3);

        // Simulate a daemon restart that REUSES the same pid but is stamped
        // with a fresh nonce by acquire_lock before any rebuild in the new
        // epoch can happen.
        state.epoch_nonce = "epoch-b".to_string();
        record_ignore_rebuild(&mut state, 4_000);
        record_ignore_rebuild(&mut state, 5_000);

        assert_eq!(
            state.ignore_rebuilds, 5,
            "lifetime total must keep accumulating across the epoch boundary"
        );
        assert_eq!(
            state.epoch_ignore_rebuilds, 2,
            "epoch counter must have restarted from zero at the new epoch, keyed on the \
             nonce even though pid (111) never changed"
        );
    }

    #[test]
    fn record_ignore_rebuild_increments_and_stamps_time_and_round_trips() {
        let mut state = State::default();
        record_ignore_rebuild(&mut state, 1_000);
        record_ignore_rebuild(&mut state, 2_000);
        assert_eq!(state.ignore_rebuilds, 2);
        assert_eq!(state.last_ignore_rebuild_ms, 2_000);

        let text = serde_json::to_string(&state).unwrap();
        let back: State = serde_json::from_str(&text).unwrap();
        assert_eq!(back.ignore_rebuilds, 2);
        assert_eq!(back.last_ignore_rebuild_ms, 2_000);
    }

    #[test]
    fn record_non_utf8_path_skip_increments_and_round_trips() {
        let mut state = State::default();
        record_non_utf8_path_skip(&mut state);
        record_non_utf8_path_skip(&mut state);
        assert_eq!(state.non_utf8_path_skips, 2);
        let text = serde_json::to_string(&state).unwrap();
        let back: State = serde_json::from_str(&text).unwrap();
        assert_eq!(back.non_utf8_path_skips, 2);
    }

    // D35 gap closure: a prompt-put failure must bump its OWN counter, never
    // the file-scoped `snapshot_failures`/`io_failed` — the two failure
    // modes are unrelated causes with unrelated remedies (undo per-file vs.
    // "this turn's attribution excerpt has no full-text backing").
    #[test]
    fn record_prompt_put_failure_increments_its_own_counter_only() {
        let mut state = State::default();
        record_prompt_put_failure(&mut state);
        record_prompt_put_failure(&mut state);
        assert_eq!(state.prompt_put_failures, 2);
        assert_eq!(state.snapshot_failures, 0);
        assert!(state.io_failed.is_empty());
    }

    #[test]
    fn prompt_put_failures_round_trips_through_serde() {
        let mut state = State::default();
        record_prompt_put_failure(&mut state);
        let text = serde_json::to_string(&state).unwrap();
        let back: State = serde_json::from_str(&text).unwrap();
        assert_eq!(back.prompt_put_failures, 1);
    }

    // Phase 2 (honesty-fixes round): `read_state` must degrade PER FIELD, not
    // reset the whole struct on one bad field. A `signal_offset` that fails
    // to parse must not cost `pid`/`snapshot_failures`/`io_failed` — those
    // are sibling fields with no relationship to the corrupt one. Neuter:
    // restore the old `.ok().and_then(...).unwrap_or_default()` chain (which
    // discards the entire struct on any single field error) → RED.
    #[test]
    fn state_survives_one_bad_field() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        std::fs::write(
            state_path(root),
            r#"{"pid":7,"signal_offset":"not-a-number","snapshot_failures":2,"io_failed":["a.rs"]}"#,
        )
        .unwrap();

        let state = read_state(root);
        assert_eq!(
            state.pid, 7,
            "sibling field pid must survive a corrupt signal_offset"
        );
        assert_eq!(
            state.snapshot_failures, 2,
            "sibling field snapshot_failures must survive"
        );
        assert_eq!(
            state.io_failed,
            vec!["a.rs".to_string()],
            "sibling field io_failed must survive"
        );
        assert_eq!(
            state.signal_offset, 0,
            "the corrupt field itself falls back to its default"
        );
        assert_eq!(
            state.state_parse_failures, 1,
            "the corrupt field must be counted, not silent"
        );
        assert_eq!(state.last_bad_field.as_deref(), Some("signal_offset"));
    }

    // The criterion that matters most: a corrupt field OTHER than
    // signal_offset must never reset signal_offset to 0, because that
    // replays the entire signal.jsonl inbox from byte 0. Neuter: same whole-
    // struct reset → RED.
    #[test]
    fn corrupt_field_does_not_replay_signal_inbox() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        std::fs::write(
            state_path(root),
            r#"{"pid":1,"signal_offset":4096,"io_failed":"not-an-array"}"#,
        )
        .unwrap();

        let state = read_state(root);
        assert_eq!(
            state.signal_offset, 4096,
            "a corrupt UNRELATED field must never reset signal_offset — \
             that would replay the whole signal inbox"
        );
        assert_eq!(state.state_parse_failures, 1);
        assert_eq!(state.last_bad_field.as_deref(), Some("io_failed"));
    }

    // A file that is not JSON at all (or unreadable) genuinely cannot be
    // recovered field-by-field — but that must still be COUNTED, never
    // silent. Neuter: drop the counter increment on this path → RED.
    #[test]
    fn wholly_unparseable_state_counts_failure_and_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        std::fs::write(state_path(root), "not json at all { garbage").unwrap();

        let state = read_state(root);
        assert_eq!(state.pid, 0);
        assert_eq!(state.signal_offset, 0);
        assert_eq!(
            state.state_parse_failures, 1,
            "a wholly unparseable file must still be counted, not silent"
        );
        assert!(state.last_bad_field.is_some());
    }

    // A healthy state.json (nothing corrupt) must round-trip byte-identically
    // through read_state -> write_state: the per-field extraction must not
    // introduce drift for the common case.
    #[test]
    fn read_state_round_trips_healthy_file_byte_identically() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        let mut state = State {
            pid: 55,
            signal_offset: 999,
            ..State::default()
        };
        record_io_failure(&mut state, "x.rs");
        write_state(root, &state).unwrap();

        let text_before = std::fs::read_to_string(state_path(root)).unwrap();
        let reloaded = read_state(root);
        assert_eq!(
            reloaded.state_parse_failures, 0,
            "no failures on a healthy file"
        );
        write_state(root, &reloaded).unwrap();
        let text_after = std::fs::read_to_string(state_path(root)).unwrap();

        assert_eq!(
            text_before, text_after,
            "a healthy state.json must round-trip byte-identically"
        );
    }

    // C1 (verification item 6): a state.json written before
    // `last_emitter_turn_key`/`duplicate_emitter_turn_signals`/
    // `mismatched_stop_emitter_turns` existed — carrying only pre-C1 keys —
    // must still parse cleanly. The new fields must default (never a parse
    // failure): a MISSING key is not corruption, only a present key of the
    // wrong type is. Neuter: swap any of the three `#[serde(default)]`s for
    // a bare (non-defaulted) field -> RED (struct-level deserialize fails
    // outright the instant a present sibling key exists without it).
    #[test]
    fn pre_c1_state_json_without_emitter_turn_fields_parses_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        // Exactly the field set a pre-C1 daemon would have written — no
        // emitter_turn keys anywhere.
        std::fs::write(
            state_path(root),
            r#"{"pid":42,"signal_offset":777,"snapshot_failures":0,"io_failed":[],
               "memory_rejects":0,"unknown_signal_ignored":0,"non_utf8_path_skips":0,
               "prompt_put_failures":0,"ignore_rebuilds":0,"last_ignore_rebuild_ms":0,
               "epoch_ignore_rebuilds":0,"epoch_nonce":"","epoch_reload_nonce":"",
               "state_parse_failures":0,"last_bad_field":null,"watcher_armed_nonce":"",
               "dedup_hits":0,"dedup_reread_bytes":0}"#,
        )
        .unwrap();

        let state = read_state(root);
        assert_eq!(
            state.state_parse_failures, 0,
            "a missing (not wrong-typed) new field must never be counted as corruption"
        );
        assert_eq!(state.last_bad_field, None);
        assert_eq!(state.pid, 42);
        assert_eq!(
            state.signal_offset, 777,
            "sibling fields must survive intact"
        );
        assert_eq!(state.last_emitter_turn_key, None);
        assert_eq!(state.duplicate_emitter_turn_signals, 0);
        assert_eq!(state.mismatched_stop_emitter_turns, 0);
    }

    // C1 fix 1: the specific transitional shape this fix must tolerate — a
    // state.json written by a binary that HAD `last_emitter_turn_key` but
    // predates `last_emitter_turn_fingerprint`. Missing (not wrong-typed)
    // must default to None, never a parse failure — same posture as the
    // test above, scoped to the one new field this fix adds. Neuter: swap
    // the field's `#[serde(default)]` for a bare one -> RED.
    #[test]
    fn state_json_with_key_but_no_fingerprint_parses_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        // The delimiter inside `last_emitter_turn_key` is U+0001, a JSON
        // control character that MUST be `\u0001`-escaped to be valid
        // JSON (exactly what `serde_json::to_string` always emits for it)
        // -- a raw unescaped control byte here would make the whole file
        // fail to parse as JSON at all, a different (irrelevant) failure
        // mode than the one this test targets.
        std::fs::write(
            state_path(root),
            "{\"pid\":9,\"signal_offset\":5,\
             \"last_emitter_turn_key\":\"codex\\u0001stop\\u0001s1\\u0001et-1\",\
             \"duplicate_emitter_turn_signals\":0,\"mismatched_stop_emitter_turns\":0}",
        )
        .unwrap();

        let state = read_state(root);
        assert_eq!(
            state.state_parse_failures, 0,
            "a missing (not wrong-typed) new field must never be counted as corruption"
        );
        assert_eq!(
            state.last_emitter_turn_key.as_deref(),
            Some("codex\u{1}stop\u{1}s1\u{1}et-1"),
            "the pre-fix key must survive intact"
        );
        assert_eq!(state.last_emitter_turn_fingerprint, None);
    }

    /// `write_state`'s tmp name is `state.json.tmp.<pid>` — predictable
    /// enough for a sandboxed agent to plant a FIFO at it, and `fs::write`
    /// opens create+truncate, which blocks until a reader appears. This is
    /// the daemon's hot path (every offset/nonce persist), so a hang here
    /// stops recording entirely. HANGS rather than fails on regression.
    #[test]
    #[cfg(unix)]
    fn write_state_refuses_a_fifo_tmp_instead_of_hanging() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();
        let tmp_path = state_path(root).with_extension(format!("json.tmp.{}", std::process::id()));
        let c = std::ffi::CString::new(tmp_path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(
            unsafe { libc::mkfifo(c.as_ptr(), 0o600) },
            0,
            "fixture must actually create a fifo"
        );

        let err = write_state(root, &State::default()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("not a regular file"),
            "refusal must name the reason: {err}"
        );
        assert!(
            !state_path(root).exists(),
            "a refused write must not rename anything into place"
        );

        // ALLOW half: without the fifo the same call must still persist, or a
        // refuse-everything guard would pass the asserts above while silently
        // disabling every state write.
        let ok = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(crate::agentrec_dir(ok.path())).unwrap();
        write_state(ok.path(), &State::default()).expect("an ordinary tmp path must still write");
        assert!(state_path(ok.path()).exists());
    }
}
