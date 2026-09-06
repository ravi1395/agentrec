//! Phase 2 tail, Task C1: `SignalEvent.emitter_turn` plus D52's
//! `SignalEvent.emitter_event` (PROTOCOL §4 additive) and their two daemon-
//! side behaviors — restart-safe dedup of a resent start/stop, and a
//! mismatched-emitter_turn stop leaving the open bracket alone. Deliberately
//! a separate file from `cli/tests/hardening_daemon.rs`
//! (whose own header states the same convention): helpers below are
//! intentionally duplicated rather than shared.
//!
//! Every signal below is planted directly into `signal.jsonl`, bypassing
//! `agentrec hook`; the explicit `emitter_event` values model the stable
//! per-invocation IDs that the Codex hook now emits.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures, which this harness created;
//    there is no attacker-supplied FIFO to block on, so the fsguard wrappers
//    buy nothing here. Production reads stay lint-enforced (clippy.toml).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

fn agentrec(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .args(["--root", root.to_str().unwrap()])
        .output()
        .expect("run agentrec")
}

fn spawn_record(root: &Path) -> Child {
    Command::new(bin())
        .args(["record", "--root", root.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn record")
}

fn append_signal_line(root: &Path, line: &str) {
    let path = root.join(".agentrec/signal.jsonl");
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open signal.jsonl");
    writeln!(f, "{line}").unwrap();
}

fn start_signal(tool: &str, session: &str, emitter_turn: &str) -> String {
    serde_json::json!({
        "v": 1,
        "ts": 1_700_000_000_000u64,
        "tool": tool,
        "event": "start",
        "session": session,
        "emitter_turn": emitter_turn,
        "emitter_event": "event-start-1",
    })
    .to_string()
}

fn stop_signal(tool: &str, session: &str, emitter_turn: &str) -> String {
    serde_json::json!({
        "v": 1,
        "ts": 1_700_000_001_000u64,
        "tool": tool,
        "event": "stop",
        "session": session,
        "emitter_turn": emitter_turn,
        "emitter_event": "event-stop-1",
    })
    .to_string()
}

/// Turn records currently in the log (epoch lines skipped).
fn turns(root: &Path) -> Vec<serde_json::Value> {
    let path = root.join(".agentrec/log.jsonl");
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("turn"))
        .collect()
}

/// `event` values ("start"/"stop") of every epoch line in the log, in order.
fn epoch_events(root: &Path) -> Vec<String> {
    let path = root.join(".agentrec/log.jsonl");
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("epoch"))
        .filter_map(|v| {
            v.get("event")
                .and_then(|e| e.as_str())
                .map(|s| s.to_string())
        })
        .collect()
}

/// Parsed `.agentrec/state.json`. Absent/corrupt file = `{}` (matches the
/// daemon's own `State::default()` tolerance).
fn state_json(root: &Path) -> serde_json::Value {
    std::fs::read_to_string(root.join(".agentrec/state.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn init(root: &Path) {
    Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(root)
        .status()
        .unwrap();
    let out = agentrec(root, &["init", "--no-hook", "--no-service"]);
    assert!(out.status.success(), "init failed: {out:?}");
}

/// Poll `f` until it returns Some or the deadline passes.
fn poll_until<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_start_epoch(root: &Path) {
    let started = poll_until(Duration::from_secs(5), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(started.is_some(), "daemon never appended a start epoch");
}

/// Waits for the log to carry MORE "start" epochs than `prior_count` — unlike
/// `wait_for_start_epoch`, this correctly detects a SECOND daemon's start
/// after a restart, where the first daemon's own "start" epoch is already
/// sitting in the log and would otherwise make a bare `.contains(&"start")`
/// check pass immediately, before the second daemon has done anything.
fn wait_for_nth_start_epoch(root: &Path, prior_count: usize) {
    let started = poll_until(Duration::from_secs(5), || {
        let n = epoch_events(root)
            .iter()
            .filter(|e| e.as_str() == "start")
            .count();
        (n > prior_count).then_some(())
    });
    assert!(
        started.is_some(),
        "daemon never appended a start epoch beyond the prior {prior_count}"
    );
}

fn sigkill(child: &mut Child) {
    let pid = child.id();
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    assert_eq!(
        rc,
        0,
        "SIGKILL delivery failed: {}",
        std::io::Error::last_os_error()
    );
    let exited = poll_until(Duration::from_secs(5), || child.try_wait().ok().flatten());
    assert!(exited.is_some(), "daemon did not die within 5s of SIGKILL");
}

fn sigterm_and_wait(child: &mut Child) {
    let pid = child.id();
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    assert_eq!(
        rc,
        0,
        "SIGTERM delivery failed: {}",
        std::io::Error::last_os_error()
    );
    let exited = poll_until(Duration::from_secs(5), || child.try_wait().ok().flatten());
    assert!(exited.is_some(), "daemon did not exit within 5s of SIGTERM");
}

// C1: a start signal carrying `emitter_turn` that gets resent — the exact
// scenario a crash-restart racing the emitter's own retry produces — must
// mint exactly ONE turn, not two. Discriminating design (NOT "wait a bit and
// count turns": a bracket only becomes a `log.jsonl` turn via a long
// timeout, clean shutdown, or (suppressed) quiet-window, so a naive
// turns-count check would pass with or without the fix). Instead: assert the
// crash journal (`open.json`) is durably absent after the resend — with the
// bug, the resend reopens a bracket and `open.json` reappears; with the fix,
// nothing opens and it stays gone — plus the dedup counter, plus a final
// turn count of exactly 1 after a clean shutdown.
#[test]
fn duplicate_start_after_restart_yields_one_turn_not_two() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon1 = spawn_record(root);
    wait_for_start_epoch(root);

    let sig = start_signal("codex", "s1", "et-abc");
    append_signal_line(root, &sig);

    // Durability gate BEFORE the crash: the daemon must have (a) processed
    // the start (bracket open, journaled) and (b) persisted the dedup key —
    // otherwise the baseline this test measures against is undefined.
    let ready = poll_until(Duration::from_secs(5), || {
        let key_set = state_json(root)
            .get("last_emitter_turn_key")
            .and_then(|v| v.as_str())
            == Some("codex\u{1}start\u{1}s1\u{1}et-abc");
        let journaled = root.join(".agentrec/open.json").exists();
        (key_set && journaled).then_some(())
    });
    assert!(
        ready.is_some(),
        "daemon never journaled the open bracket + persisted the dedup key before the crash"
    );

    let epochs_before_restart = epoch_events(root)
        .iter()
        .filter(|e| e.as_str() == "start")
        .count();

    // Unclean kill: leaves the crash journal in place, nothing cleanly
    // closed — exactly what a real crash-then-restart looks like.
    sigkill(&mut daemon1);

    // Restart. `recover_orphan` runs before the fresh engine is even
    // constructed and before the "start" epoch is appended (daemon.rs::run),
    // so waiting for the SECOND start epoch (not just "a" start epoch — the
    // first daemon's is already in the log) guarantees recovery already ran.
    let mut daemon2 = spawn_record(root);
    wait_for_nth_start_epoch(root, epochs_before_restart);
    assert_eq!(
        turns(root).len(),
        1,
        "recover_orphan must have closed the crash-journaled bracket into exactly one \
         truncated rich turn"
    );
    assert!(
        !root.join(".agentrec/open.json").exists(),
        "recover_orphan removes the journal once it's logged"
    );

    // The emitter resends the IDENTICAL start signal (its own retry, or a
    // resend racing the restart) — appended fresh, so the restarted daemon
    // reads it as a genuinely new inbox line.
    append_signal_line(root, &sig);

    let counted = poll_until(Duration::from_secs(5), || {
        let n = state_json(root)
            .get("duplicate_emitter_turn_signals")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        (n == 1).then_some(())
    });
    assert!(
        counted.is_some(),
        "the resent start must be counted as a duplicate — dedup either never fired, \
         or fired more than once"
    );
    assert!(
        !root.join(".agentrec/open.json").exists(),
        "the resend must not reopen a bracket — with the bug this would journal a second \
         open turn here"
    );

    sigterm_and_wait(&mut daemon2);
    assert_eq!(
        turns(root).len(),
        1,
        "final tally: the resent start must not have minted a second turn"
    );
}

// C1: a stop whose emitter_turn mismatches the currently open bracket's own
// must NOT close it — the bracket stays open, and a later MATCHING stop can
// still close it normally (positive control). Discriminating without the
// fix: the mismatched stop's tool matches, so `observe_stop`'s existing
// logic would close it as an ordinary rich bracket turn.
#[test]
fn mismatched_stop_emitter_turn_leaves_bracket_open() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    wait_for_start_epoch(root);

    append_signal_line(root, &start_signal("claude-code", "s1", "et-1"));
    let opened = poll_until(Duration::from_secs(5), || {
        root.join(".agentrec/open.json").exists().then_some(())
    });
    assert!(
        opened.is_some(),
        "bracket never opened for the start signal"
    );
    assert_eq!(turns(root).len(), 0, "nothing should have closed yet");

    // Mismatched stop: same tool, different emitter_turn.
    append_signal_line(root, &stop_signal("claude-code", "s1", "et-2"));
    let counted = poll_until(Duration::from_secs(5), || {
        let n = state_json(root)
            .get("mismatched_stop_emitter_turns")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        (n == 1).then_some(())
    });
    assert!(
        counted.is_some(),
        "the mismatched stop must be counted, not silently dropped"
    );
    assert!(
        root.join(".agentrec/open.json").exists(),
        "the bracket must still be open — a mismatched stop must not close it"
    );
    assert_eq!(
        turns(root).len(),
        0,
        "with the bug, this mismatched stop would have closed the bracket as a rich turn here"
    );

    // Positive control: a MATCHING stop closes the same bracket normally.
    append_signal_line(root, &stop_signal("claude-code", "s1", "et-1"));
    let closed = poll_until(Duration::from_secs(5), || {
        (turns(root).len() == 1).then_some(())
    });
    assert!(
        closed.is_some(),
        "a matching stop must still close the bracket — the mismatch check must not over-fire"
    );
    assert!(!root.join(".agentrec/open.json").exists());

    sigterm_and_wait(&mut daemon);
    assert_eq!(turns(root).len(), 1);
}

// AC-C1 byte-identical claim, at the real-daemon layer (the wire-byte half
// lives in `agentrec_core::record::tests::
// signal_emitter_turn_roundtrips_and_absence_stays_absent`; the
// zero-extra-writes half in `daemon::tests::
// handle_emitter_turn_signal_noop_and_zero_writes_when_emitter_turn_absent`).
// This test is the third leg: a real spawned daemon processing an ordinary
// start/stop pair with NO `emitter_turn` at all (today's actual Claude Code
// shape) must write no emitter_turn-related keys to state.json, and must
// close the turn exactly as it always has.
#[test]
fn signals_without_emitter_turn_write_no_new_state_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    wait_for_start_epoch(root);

    let no_et_start = serde_json::json!({
        "v": 1, "ts": 1_700_000_000_000u64, "tool": "claude-code",
        "event": "start", "session": "s1",
    })
    .to_string();
    let no_et_stop = serde_json::json!({
        "v": 1, "ts": 1_700_000_001_000u64, "tool": "claude-code",
        "event": "stop", "session": "s1",
    })
    .to_string();
    append_signal_line(root, &no_et_start);
    append_signal_line(root, &no_et_stop);

    let closed = poll_until(Duration::from_secs(5), || {
        (turns(root).len() == 1).then_some(())
    });
    assert!(
        closed.is_some(),
        "the ordinary bracket must still close normally"
    );
    let t = &turns(root)[0];
    assert_eq!(t.get("grade").and_then(|v| v.as_str()), Some("rich"));
    assert_eq!(t.get("tool").and_then(|v| v.as_str()), Some("claude-code"));

    let st = state_json(root);
    assert!(
        st.get("last_emitter_turn_key").is_none()
            || st.get("last_emitter_turn_key") == Some(&serde_json::Value::Null),
        "no emitter_turn-carrying signal was ever sent — this key must never be set"
    );
    assert_eq!(
        st.get("duplicate_emitter_turn_signals")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        0
    );
    assert_eq!(
        st.get("mismatched_stop_emitter_turns")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        0
    );

    sigterm_and_wait(&mut daemon);
}
