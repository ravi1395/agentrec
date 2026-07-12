//! Pre-launch hardening round (2026-07-11), Batch D (daemon/state/service/
//! doctor). Integration coverage for findings that need a real spawned
//! `agentrec record` process — D1 (SIGTERM shutdown) and D2 (the flock lock
//! actually releasing on exit). Everything else in Batch D is covered by
//! inline unit tests in `cli/src/daemon.rs`/`doctorcmd.rs`/`service.rs`.
//!
//! Deliberately a separate file from `cli/tests/integration.rs` (owned by
//! another concurrent hardening pass) — helpers below are intentionally
//! duplicated rather than shared, to avoid touching that file.

use std::collections::HashSet;
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

/// Like `spawn_record`, but with extra environment variables — used by the
/// D-M6 crash-window test to set `AGENTREC_TEST_PAUSE_AFTER_CANDIDATE_MS`.
fn spawn_record_with_env(root: &Path, envs: &[(&str, &str)]) -> Child {
    let mut cmd = Command::new(bin());
    cmd.args(["record", "--root", root.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.spawn().expect("spawn record")
}

fn send_hook(root: &Path, payload: &str) {
    let mut child = Command::new(bin())
        .args(["hook", "claude", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait().unwrap();
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

/// Appends a raw line directly to `signal.jsonl` — used to plant signal
/// shapes (like a memory-candidate line) that no CLI-facing emitter writes
/// yet, bypassing `agentrec hook` entirely.
fn append_signal_line(root: &Path, line: &str) {
    let path = root.join(".agentrec/signal.jsonl");
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open signal.jsonl");
    writeln!(f, "{line}").unwrap();
}

/// Appends several raw lines to `signal.jsonl` in a SINGLE `write_all` call.
/// `write()` on a regular file opened `O_APPEND` is atomic — the whole buffer
/// lands or none of it does — so this guarantees `lines` land in the file
/// together, with no window in which the daemon's tailer could observe only
/// a prefix. Used to force two signals (a `start` and a `memory-candidate`)
/// into the SAME `SignalTailer::poll()` batch, which the D-M6 race requires;
/// two separate `append_signal_line` calls would only make that likely, not
/// certain.
fn append_signal_lines_atomic(root: &Path, lines: &[String]) {
    let path = root.join(".agentrec/signal.jsonl");
    let mut buf = String::new();
    for line in lines {
        buf.push_str(line);
        buf.push('\n');
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open signal.jsonl");
    f.write_all(buf.as_bytes()).unwrap();
}

/// A raw `start` bracket signal (PROTOCOL §4), ready for
/// `append_signal_lines_atomic` — bypasses `agentrec hook`'s subprocess spawn
/// so it can be combined with a candidate line in one atomic write.
fn start_signal(session: &str) -> String {
    serde_json::json!({
        "v": 1,
        "ts": 1_700_000_000_000u64,
        "tool": "claude-code",
        "event": "start",
        "session": session,
    })
    .to_string()
}

/// Every line of `.agentrec/memory.jsonl`, parsed. Absent file = empty vec.
fn memories(root: &Path) -> Vec<serde_json::Value> {
    let path = root.join(".agentrec/memory.jsonl");
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
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

/// A memory-candidate signal line (PROTOCOL §4 additive), ready for
/// `append_signal_line` — no CLI-facing emitter exists yet (Task 8), so
/// Task 7's tests plant the shape directly, same convention as Task 6's
/// `memory_candidate_signal_never_closes_a_turn`.
fn memory_signal(fact: &str, pins: &[&str]) -> String {
    serde_json::json!({
        "v": 1,
        "ts": 1_700_000_000_000u64,
        "tool": "claude-code",
        "type": "memory-candidate",
        "fact": fact,
        "pins": pins,
    })
    .to_string()
}

fn init(root: &Path) {
    // A git repo so gitignore semantics are exercised; --no-hook/--no-service
    // avoid touching real Claude Code settings or a real launchd/systemd unit.
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

// D1: SIGTERM is exactly what launchd (`KeepAlive`) / systemd (`Restart=
// always`) send on every service stop/restart. Before this fix, `ctrlc` was
// registered with default features (SIGINT only), so SIGTERM fell through to
// the OS default action (an unclean kill) — no stop epoch, no released lock,
// a stale crash journal on the next start. This drives the real binary and
// asserts the SAME clean-shutdown behavior SIGINT already had: a "stop"
// epoch appended, and the process exiting on its own (never needing a
// follow-up SIGKILL).
#[test]
fn sigterm_triggers_clean_shutdown_with_stop_epoch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    let started = poll_until(Duration::from_secs(5), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(started.is_some(), "daemon never appended a start epoch");

    let pid = daemon.id();
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    assert_eq!(
        rc,
        0,
        "SIGTERM delivery failed: {}",
        std::io::Error::last_os_error()
    );

    // The daemon must exit ON ITS OWN in response to the signal — no kill()
    // fallback here proves the handler actually ran the shutdown path rather
    // than the process ignoring SIGTERM outright.
    let exited = poll_until(Duration::from_secs(5), || daemon.try_wait().ok().flatten());
    assert!(
        exited.is_some(),
        "daemon did not exit within 5s of SIGTERM — handler not registered for it"
    );
    assert!(
        exited.unwrap().success(),
        "daemon should exit 0 on a clean SIGTERM shutdown"
    );

    let events = epoch_events(root);
    assert!(
        events.contains(&"stop".to_string()),
        "no stop epoch appended after SIGTERM: {events:?}"
    );
}

// D1 + D2 together: after a SIGTERM shutdown, a fresh daemon must be able to
// acquire the recorder lock immediately. If the flock were leaked (e.g. the
// signal handler bypassed the code path that drops the lock-holding File),
// this would hang at "already recording" forever instead of opening a new
// epoch.
#[test]
fn sigterm_releases_the_lock_for_a_subsequent_daemon() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut first = spawn_record(root);
    poll_until(Duration::from_secs(5), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });

    let pid = first.id();
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    let exited = poll_until(Duration::from_secs(5), || first.try_wait().ok().flatten());
    assert!(exited.is_some(), "first daemon did not exit after SIGTERM");

    let mut second = spawn_record(root);
    let ok = poll_until(Duration::from_secs(5), || {
        let starts = epoch_events(root)
            .iter()
            .filter(|e| e.as_str() == "start")
            .count();
        (starts >= 2).then_some(())
    });
    let _ = second.kill();
    let _ = second.wait();
    assert!(
        ok.is_some(),
        "second daemon never started — the lock from the SIGTERM'd first daemon was leaked"
    );
}

// Task 6 hazard-register test. A memory-candidate signal line has no `event`
// field. Before the routing guard, `SignalEvent::is_start()` on such a line
// is false, so the daemon's poll-dispatch loop fell into the stop arm
// (`apply_signal` -> `engine.observe_stop`) and closed whatever bracket was
// currently open — a fabricated turn closure the agent never asked for. This
// drives the real daemon against that exact shape (planted directly into
// `signal.jsonl`, since no emitter writes it yet — that's Task 7) and
// asserts the open bracket survives it; only a genuine Stop hook may close
// the turn. The tail end doubles as the regression pair: a legacy signal
// with no `kind`/`type` (the real Stop hook payload) must still close
// normally.
#[test]
fn memory_candidate_signal_never_closes_a_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    let started = poll_until(Duration::from_secs(5), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(started.is_some(), "daemon never appended a start epoch");

    // Open the bracket.
    send_hook(
        root,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s_mc"}"#,
    );

    // Plant a memory-candidate signal line directly. Deliberately NO "event"
    // field: this is exactly the shape that, pre-guard, falls through
    // `SignalEvent::is_start() == false` and hits the stop arm.
    let candidate = serde_json::json!({
        "v": 1,
        "ts": 1_700_000_000_000u64,
        "tool": "claude-code",
        "type": "memory-candidate",
        "fact": "user prefers dark mode",
        "pins": ["src/main.rs"]
    });
    append_signal_line(root, &candidate.to_string());

    // Mutate a file inside the (should-still-be-open) bracket.
    std::fs::write(root.join("notes.txt"), "hello").unwrap();

    // Several poll cycles (POLL = 250ms in the daemon loop) for the daemon to
    // tail the candidate line and, if unguarded, fabricate a closure. No
    // quiet-window wait needed — the bug fires as soon as the signal is
    // tailed, not on a debounce/quiet-window timer.
    std::thread::sleep(Duration::from_secs(3));
    let premature = turns(root);
    assert!(
        premature.is_empty(),
        "a memory-candidate signal (no `event` field) closed a turn — \
         it must never reach the stop arm: {premature:?}"
    );

    // Regression pair: the real Stop hook (a legacy signal, no `kind`/`type`)
    // still closes the bracket normally.
    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s_mc"}"#);
    let closed = poll_until(Duration::from_secs(5), || {
        let t = turns(root);
        (!t.is_empty()).then_some(t)
    });

    let _ = daemon.kill();
    let _ = daemon.wait();

    let t = closed.expect("no turn ever closed after the real Stop hook");
    assert_eq!(t.len(), 1, "expected exactly one rich turn: {t:?}");
    assert_eq!(
        t[0].get("grade").and_then(|g| g.as_str()),
        Some("rich"),
        "turn should be rich (bracket-closed): {t:?}"
    );
}

// F5 hazard-register test. `memory_candidate_signal_never_closes_a_turn`
// above proves the specific `type: "memory-candidate"` shape can't fabricate
// a close because it's intercepted before `apply_signal` ever runs. But
// `apply_signal`'s own dispatch (`if sig.is_start() { .. } else { ..stop.. }`)
// is a catch-all `else` — ANY signal that isn't a recognized start, memory-
// candidate included or not, falls into the stop arm. A signal with a `type`
// this daemon has never heard of (a genuinely forward-compat shape a future
// producer might emit) has no `event` field either, so pre-fix it fabricates
// a close exactly like the memory-candidate case did before its guard.
// PROTOCOL §10 requires unknown fields/types be tolerated, never
// reinterpreted as a different signal — this drives the real daemon against
// that shape and asserts the open bracket survives it, AND that the drop is
// visible somewhere (the `unknown_signal_ignored` state counter), not just
// silently swallowed with zero trace.
#[test]
fn unknown_signal_type_never_closes_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    let started = poll_until(Duration::from_secs(5), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(started.is_some(), "daemon never appended a start epoch");

    // Open the bracket.
    send_hook(
        root,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s_unk"}"#,
    );

    // Plant a signal with an unrecognized `type` and no `event` field — a
    // hypothetical future-minor-version signal shape, not memory-candidate.
    let unknown = serde_json::json!({
        "v": 1,
        "ts": 1_700_000_000_000u64,
        "tool": "claude-code",
        "type": "future-thing",
    });
    append_signal_line(root, &unknown.to_string());

    // Mutate a file inside the (should-still-be-open) bracket.
    std::fs::write(root.join("notes.txt"), "hello").unwrap();

    // Several poll cycles for the daemon to tail the unknown-type line and,
    // if unguarded, fabricate a closure.
    std::thread::sleep(Duration::from_secs(3));
    let premature = turns(root);
    assert!(
        premature.is_empty(),
        "an unknown-type signal (no `event` field) closed a turn — it must \
         never reach the stop arm: {premature:?}"
    );

    let ignored = state_json(root)
        .get("unknown_signal_ignored")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    assert!(
        ignored >= 1,
        "unknown signal was dropped but never counted: {:?}",
        state_json(root)
    );

    // Regression pair: a legacy Stop hook signal (no `kind`/`type` at all)
    // still closes the bracket normally — the fix must not over-correct into
    // refusing genuine stops.
    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s_unk"}"#);
    let closed = poll_until(Duration::from_secs(5), || {
        let t = turns(root);
        (!t.is_empty()).then_some(t)
    });

    let _ = daemon.kill();
    let _ = daemon.wait();

    let t = closed.expect("no turn ever closed after the real Stop hook");
    assert_eq!(t.len(), 1, "expected exactly one rich turn: {t:?}");
    assert_eq!(
        t[0].get("grade").and_then(|g| g.as_str()),
        Some("rich"),
        "turn should be rich (bracket-closed): {t:?}"
    );
}

// ---- Task 7: daemon ingestion + rejects counter ---------------------------

// End-to-end happy path: a candidate emitted while a bracket is open is
// ingested with a DAEMON-computed pin hash (never a hash the emitter could
// have supplied — the candidate signal carries paths only) and a non-empty
// `source_turns`. The id in `source_turns` is asserted to be the SAME id the
// enclosing turn actually closes under (real provenance, not a dangling
// reference to an id that never lands in log.jsonl — the reason
// `TurnEngine::open_turn_id()` reserves the id at open time rather than
// generating a fresh one at close). A byte-identical duplicate candidate
// must not create a second record (dedup, silent drop).
#[test]
fn candidate_ingestion_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::write(root.join("notes.txt"), "hello world").unwrap();

    let mut daemon = spawn_record(root);
    let started = poll_until(Duration::from_secs(5), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(started.is_some(), "daemon never appended a start epoch");

    // Open the bracket.
    send_hook(
        root,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s_mem1"}"#,
    );

    let candidate = memory_signal("the daemon debounces bursts for 1.5s", &["notes.txt"]);
    append_signal_line(root, &candidate);

    let recorded = poll_until(Duration::from_secs(5), || {
        let m = memories(root);
        (!m.is_empty()).then_some(m)
    });
    let recs = recorded.expect("candidate was never ingested into memory.jsonl");
    assert_eq!(
        recs.len(),
        1,
        "expected exactly one memory record: {recs:?}"
    );
    let rec = recs[0].clone();
    assert_eq!(rec.get("origin").and_then(|v| v.as_str()), Some("agent"));

    let expected_hash = agentrec_core::memory::hash_pin(root, "notes.txt")
        .expect("daemon and test read the same file, hashing must succeed");
    let actual_hash = rec["pins"][0]["hash"].as_str().expect("pin hash present");
    assert_eq!(
        actual_hash, expected_hash,
        "the daemon must compute the pin hash itself at ingestion, not trust the emitter"
    );

    let source_turns = rec["source_turns"]
        .as_array()
        .expect("source_turns must be an array");
    assert!(
        !source_turns.is_empty(),
        "expected non-empty source_turns while a bracket is open: {rec:?}"
    );

    // Duplicate candidate (byte-identical fact + pin set) -> still exactly 1
    // record; several poll cycles (POLL = 250ms) for the daemon to tail it.
    append_signal_line(root, &candidate);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        memories(root).len(),
        1,
        "a duplicate candidate must not create a second record"
    );

    // Close the bracket and confirm source_turns really points at the turn
    // that ends up in log.jsonl.
    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s_mem1"}"#);
    let closed = poll_until(Duration::from_secs(5), || {
        let t = turns(root);
        (!t.is_empty()).then_some(t)
    });
    let _ = daemon.kill();
    let _ = daemon.wait();

    let t = closed.expect("no turn ever closed after Stop");
    let closed_id = t[0]["id"].as_str().expect("closed turn has an id");
    assert_eq!(
        source_turns[0].as_str(),
        Some(closed_id),
        "source_turns must reference the id the enclosing turn actually \
         closed under, not a fabricated/dangling one: memory={rec:?} turn={:?}",
        t[0]
    );
}

/// Overwrites `.agentrec/config.toml` with a single `memory_enabled = <val>`
/// line — matches the hand-rolled `key = value` scan `memorycmds::
/// read_memory_enabled` performs (no `toml` dependency), so this is a valid
/// config regardless of what `init` wrote as the default.
fn write_memory_enabled(root: &Path, enabled: bool) {
    std::fs::write(
        root.join(".agentrec/config.toml"),
        format!("memory_enabled = {enabled}\n"),
    )
    .unwrap();
}

// Fix D (codex cross-review): the design spec's kill-switch (line 184,
// binding) reads "`[memory] enabled = false` disables injection **+
// candidate ingestion**" — but pre-fix, `memory_enabled` was only checked on
// the hook-injection path (`cmds.rs`); the daemon's live ingestion
// (`daemon.rs` main loop) and startup replay (`replay_pending_candidates`)
// ingested candidates regardless. This drives the real daemon with
// `memory_enabled = false` in config.toml, plants a valid memory-candidate
// signal exactly like `candidate_ingestion_end_to_end`, and asserts NOTHING
// lands in memory.jsonl — plus the regression pair from Task 6: a disabled
// candidate must still never fabricate a turn closure (the routing guard
// that keeps candidate lines away from the stop arm is orthogonal to the
// kill-switch and must stay intact).
#[test]
fn candidate_ingestion_disabled_when_memory_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    write_memory_enabled(root, false);
    std::fs::write(root.join("notes.txt"), "hello world").unwrap();

    let mut daemon = spawn_record(root);
    let started = poll_until(Duration::from_secs(5), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(started.is_some(), "daemon never appended a start epoch");

    // Open the bracket (same shape as candidate_ingestion_end_to_end).
    send_hook(
        root,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s_disabled"}"#,
    );

    let candidate = memory_signal("memory should be off right now", &["notes.txt"]);
    append_signal_line(root, &candidate);

    // Several poll cycles (POLL = 250ms) for the daemon to tail the line and,
    // if the kill-switch were unguarded, ingest it.
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        memories(root).is_empty(),
        "memory_enabled = false must block candidate ingestion: {:?}",
        memories(root)
    );

    // Regression pair (Task 6 guard): even disabled, the candidate line must
    // not fall through to the stop arm and fabricate a turn closure.
    let premature = turns(root);
    assert!(
        premature.is_empty(),
        "a disabled memory-candidate signal closed a turn — routing guard \
         regressed: {premature:?}"
    );

    // Regression pair (rejects counter): a disabled candidate is a disabled
    // FEATURE, not an invalid candidate — it must not be counted as a reject.
    let state = state_json(root);
    assert_eq!(
        state.get("memory_rejects").and_then(|v| v.as_u64()),
        Some(0),
        "a disabled candidate must not be counted as a reject: {state:?}"
    );

    // Close the bracket normally — confirms the real Stop hook still works
    // and the daemon isn't wedged by the disabled-candidate path.
    send_hook(
        root,
        r#"{"hook_event_name":"Stop","session_id":"s_disabled"}"#,
    );
    let closed = poll_until(Duration::from_secs(5), || {
        let t = turns(root);
        (!t.is_empty()).then_some(t)
    });

    let _ = daemon.kill();
    let _ = daemon.wait();

    assert!(
        closed.is_some(),
        "no turn ever closed after the real Stop hook — daemon wedged?"
    );

    // Control: with memory_enabled = true (default), the identical candidate
    // IS ingested — proves the assertions above test the kill-switch, not a
    // broken ingestion path in general.
    let tmp2 = tempfile::tempdir().unwrap();
    let root2 = tmp2.path();
    init(root2);
    std::fs::write(root2.join("notes.txt"), "hello world").unwrap();

    let mut daemon2 = spawn_record(root2);
    let started2 = poll_until(Duration::from_secs(5), || {
        epoch_events(root2)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(
        started2.is_some(),
        "control daemon never appended a start epoch"
    );

    let candidate2 = memory_signal("memory should be on right now", &["notes.txt"]);
    append_signal_line(root2, &candidate2);

    let recorded = poll_until(Duration::from_secs(5), || {
        let m = memories(root2);
        (!m.is_empty()).then_some(m)
    });

    let _ = daemon2.kill();
    let _ = daemon2.wait();

    assert!(
        recorded.is_some(),
        "control: default memory_enabled must still ingest a valid candidate"
    );
}

// GAP: `candidate_ingestion_disabled_when_memory_disabled` above only drives
// the LIVE ingestion site (daemon.rs main loop, ~:178). The kill-switch is
// also checked at a SECOND site — `replay_pending_candidates` (daemon.rs
// ~:864, `if sig.is_memory_candidate() && memory_enabled`) — which handles
// candidates emitted in the pre-daemon gap (no daemon running yet, same
// scenario as `candidate_startup_replay_is_candidate_only_and_preserves_d7`
// in integration.rs). Nothing previously exercised that conjunct with
// `memory_enabled = false`. This plants a candidate signal while no daemon
// is running, then starts the daemon and asserts the startup replay honors
// the kill-switch — plus a control (default `memory_enabled = true`) proving
// the identical scenario DOES ingest on replay, so the assertions above test
// the kill-switch specifically, not a broken replay path in general.
#[test]
fn replay_ingestion_disabled_when_memory_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    write_memory_enabled(root, false);
    std::fs::write(root.join("notes.txt"), "hello world").unwrap();

    // No daemon running yet — plant the candidate directly into the inbox,
    // landing in the pre-daemon gap so only `replay_pending_candidates`
    // (never the live loop) ever sees it.
    let candidate = memory_signal("replay should be off right now", &["notes.txt"]);
    append_signal_line(root, &candidate);
    assert!(
        !root.join(".agentrec/memory.jsonl").exists(),
        "no daemon was running — nothing should be ingested yet"
    );

    // Start the daemon: `run()` calls `replay_pending_candidates` BEFORE
    // appending the "start" epoch, so observing "start" in the log proves
    // the replay already ran to completion.
    let mut daemon = spawn_record(root);
    let started = poll_until(Duration::from_secs(5), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(started.is_some(), "daemon never appended a start epoch");

    // A beat past startup in case the live loop (wrongly) re-tailed the line.
    std::thread::sleep(Duration::from_secs(2));
    let _ = daemon.kill();
    let _ = daemon.wait();

    assert!(
        memories(root).is_empty(),
        "memory_enabled = false must block candidate ingestion on startup replay: {:?}",
        memories(root)
    );

    // Control: with memory_enabled = true (default), the identical
    // pre-daemon-gap scenario DOES ingest on replay.
    let tmp2 = tempfile::tempdir().unwrap();
    let root2 = tmp2.path();
    init(root2);
    std::fs::write(root2.join("notes.txt"), "hello world").unwrap();

    let candidate2 = memory_signal("replay should be on right now", &["notes.txt"]);
    append_signal_line(root2, &candidate2);
    assert!(
        !root2.join(".agentrec/memory.jsonl").exists(),
        "no daemon was running — nothing should be ingested yet (control)"
    );

    let mut daemon2 = spawn_record(root2);
    let recorded = poll_until(Duration::from_secs(5), || {
        let m = memories(root2);
        (!m.is_empty()).then_some(m)
    });

    let _ = daemon2.kill();
    let _ = daemon2.wait();

    let recs = recorded
        .expect("control: default memory_enabled must ingest a valid candidate on startup replay");
    assert_eq!(
        recs.len(),
        1,
        "expected exactly one memory record from replay: {recs:?}"
    );
}

// Four genuinely-rejecting candidates — traversal pin, secret-file (`.env`)
// pin, a whitespace-only fact (scrubs to empty — NOT a secret-only fact:
// scrub redacts rather than deletes, so `"AKIA..."` alone would scrub to a
// non-empty `[redacted:...]` and PERSIST, not reject; see brief note), and 9
// pins (over PINS_MAX=8) — must leave memory.jsonl untouched (INV-M1),
// persist `memory_rejects == 4` in state.json, and surface "4 rejects" in
// `agentrec status` stdout. No daemon bracket needed; rejects are counted
// independent of any open turn.
#[test]
fn candidate_rejects_counted_never_fabricated() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::write(root.join("notes.txt"), "hello").unwrap();
    std::fs::write(root.join(".env"), "SECRET=1").unwrap();
    for i in 0..9 {
        std::fs::write(root.join(format!("f{i}.txt")), "x").unwrap();
    }

    let mut daemon = spawn_record(root);
    let started = poll_until(Duration::from_secs(5), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(started.is_some(), "daemon never appended a start epoch");

    // 1. Traversal pin — rejected unconditionally by `validate_pin_path`,
    //    regardless of whether the target exists.
    append_signal_line(
        root,
        &memory_signal("fact about something outside the repo", &["../outside.txt"]),
    );
    // 2. Secret-file pin.
    append_signal_line(
        root,
        &memory_signal("fact pinned to a secret file", &[".env"]),
    );
    // 3. Whitespace-only fact (valid pin, but scrubs to empty).
    append_signal_line(root, &memory_signal("   ", &["notes.txt"]));
    // 4. 9 pins — every individual pin is valid, but the record as a whole
    //    exceeds PINS_MAX=8 (enforced by `append_memory`).
    let nine: Vec<String> = (0..9).map(|i| format!("f{i}.txt")).collect();
    let nine_refs: Vec<&str> = nine.iter().map(|s| s.as_str()).collect();
    append_signal_line(root, &memory_signal("fact with too many pins", &nine_refs));

    // Poll: state.json's memory_rejects must reach 4.
    let rejects = poll_until(Duration::from_secs(5), || {
        state_json(root)
            .get("memory_rejects")
            .and_then(|v| v.as_u64())
            .filter(|&n| n >= 4)
    });

    let out = agentrec(root, &["status"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();

    let _ = daemon.kill();
    let _ = daemon.wait();

    assert_eq!(
        rejects,
        Some(4),
        "expected exactly 4 rejects, state.json: {:?}",
        state_json(root)
    );
    assert!(
        memories(root).is_empty(),
        "INV-M1: a rejected candidate must persist NOTHING to memory.jsonl: {:?}",
        memories(root)
    );
    assert!(
        stdout.contains("4 rejects"),
        "expected `agentrec status` to report 4 rejects, got: {stdout}"
    );
}

// INV-M3 half 2: a fact shaped like a live AWS key is scrubbed before it
// lands on disk. The persisted fact carries the `[redacted:` marker and the
// raw key never appears anywhere in `memory.jsonl`. (`signal.jsonl` DOES
// still carry the raw key here — this test plants the candidate directly,
// bypassing the not-yet-built Task 8 emitter, which is the component
// responsible for scrubbing before anything reaches `signal.jsonl`; that is
// out of scope for this assertion, which is about the persistence site.)
#[test]
fn candidate_secret_fact_scrubbed_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::write(root.join("notes.txt"), "hello").unwrap();

    let mut daemon = spawn_record(root);
    let started = poll_until(Duration::from_secs(5), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(started.is_some(), "daemon never appended a start epoch");

    let secret_fact = "the deploy key is AKIAABCDEFGHIJKLMNOP for staging";
    let candidate = memory_signal(secret_fact, &["notes.txt"]);
    append_signal_line(root, &candidate);

    let recorded = poll_until(Duration::from_secs(5), || {
        let m = memories(root);
        (!m.is_empty()).then_some(m)
    });
    let _ = daemon.kill();
    let _ = daemon.wait();

    let recs = recorded.expect("candidate was never ingested into memory.jsonl");
    assert_eq!(
        recs.len(),
        1,
        "expected exactly one memory record: {recs:?}"
    );
    let persisted_fact = recs[0]["fact"].as_str().expect("fact present");
    assert!(
        persisted_fact.contains("[redacted:"),
        "persisted fact must be scrubbed: {persisted_fact}"
    );
    assert!(
        !persisted_fact.contains("AKIAABCDEFGHIJKLMNOP"),
        "raw key must not survive scrub: {persisted_fact}"
    );

    let memory_text =
        std::fs::read_to_string(root.join(".agentrec/memory.jsonl")).expect("memory.jsonl exists");
    assert!(
        !memory_text.contains("AKIAABCDEFGHIJKLMNOP"),
        "raw key must not appear anywhere in memory.jsonl: {memory_text}"
    );
}

// ---- D-M6: dangling source_turns on a same-batch [start, candidate] crash --

// Independent adversarial review found: when a `start` signal and a
// `memory-candidate` signal land in the SAME polled batch, the daemon's
// bracket-open reserves a turn id purely in memory
// (`TurnEngine::open_turn_id`), then `ingest_candidate` durably fsyncs a
// memory record whose `source_turns` names that id — all BEFORE the
// once-per-loop-iteration `sync_journal` call (which runs only after the
// whole `for sig in tailer.poll(...)` loop finishes) ever makes the open
// turn recoverable. A kill-9 landing in that window means: on restart,
// `recover_orphan` finds no crash journal, so the turn is never written to
// `log.jsonl` — the memory record's `source_turns` id dangles, referencing a
// turn that does not exist. This defeats the entire point of reserving the
// id at open time (PROTOCOL.md memory-candidate design): `source_turns` is
// supposed to be a real, always-resolvable reference.
//
// The real window is a handful of in-process instructions wide (same loop
// iteration, no I/O in between beyond the two fsyncs) — far too narrow for
// an external test process to land a SIGKILL inside via timing alone. This
// test widens it deterministically instead of hoping for luck: the daemon
// under test is started with `AGENTREC_TEST_PAUSE_AFTER_CANDIDATE_MS` set,
// which makes it sleep for several seconds immediately after
// `ingest_candidate` durably persists the candidate — i.e. still inside the
// real race window, just held open long enough for the test to land its
// kill with certainty. `append_signal_lines_atomic` guarantees the `start`
// and candidate land in the daemon's SAME poll() batch (a single O_APPEND
// `write()` on a regular file is atomic), which is the race's precondition.
//
// Coverage note: this proves the exact ordering hazard described above —
// candidate-persisted-before-journaled, same batch, kill before the
// post-loop journal sync would otherwise run. It does not (and cannot, from
// outside the process) prove anything about crash windows narrower than one
// full statement, e.g. a kill landing between `fs::write` and `fs::rename`
// inside `sync_journal` itself — that residual is far narrower still and
// out of scope for this fix.
//
// RED (pre-fix): the pause sits entirely inside `ingest_candidate`, which
// runs BEFORE the post-loop `sync_journal` call, so no journal is ever
// written before the kill — `recover_orphan` finds nothing, the turn is
// never recovered, and `referenced_id` is absent from `log.jsonl` post-
// restart: the assertion below fails.
// GREEN (post-fix): the batch loop now calls `sync_journal` up front, before
// `ingest_candidate`, whenever a candidate arrives with a turn open — the
// journal (naming the same reserved id) is written and renamed into place
// before the candidate is even persisted, long before the pause is reached.
// `recover_orphan` on restart finds it, logs a truncated-rich turn under
// exactly `referenced_id`, and the assertion passes.
#[test]
fn dangling_source_turns_closed_by_pre_persist_journal_sync() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::write(root.join("notes.txt"), "hello").unwrap();

    let mut daemon =
        spawn_record_with_env(root, &[("AGENTREC_TEST_PAUSE_AFTER_CANDIDATE_MS", "4000")]);
    let started = poll_until(Duration::from_secs(5), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(started.is_some(), "daemon never appended a start epoch");

    // One atomic write puts both lines in the SAME poll() batch: the
    // candidate's source_turns will name the id the start signal reserves
    // moments earlier, in that very batch.
    let candidate = memory_signal("the daemon debounces bursts for 1.5s", &["notes.txt"]);
    append_signal_lines_atomic(root, &[start_signal("s_dangle"), candidate]);

    // The candidate is durably fsynced to memory.jsonl; thanks to the env
    // var, the daemon then parks for 4s still inside the pre-fix race
    // window (before the post-loop journal sync would otherwise run).
    let recorded = poll_until(Duration::from_secs(5), || {
        let m = memories(root);
        (!m.is_empty()).then_some(m)
    });
    let recs = recorded.expect("candidate was never ingested into memory.jsonl");
    assert_eq!(
        recs.len(),
        1,
        "expected exactly one memory record: {recs:?}"
    );
    let source_turns = recs[0]["source_turns"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        source_turns.len(),
        1,
        "candidate must reference the open turn reserved in the same batch: {:?}",
        recs[0]
    );
    let referenced_id = source_turns[0]
        .as_str()
        .expect("source_turns[0] is a string")
        .to_string();

    // Kill -9 while the daemon is still parked in the pause — i.e. strictly
    // before the post-loop `sync_journal` call could run on unfixed code.
    daemon.kill().expect("kill -9 the daemon");
    let _ = daemon.wait();

    // Restart: recover_orphan runs before the new epoch is appended, so by
    // the time a second "start" epoch is visible, recovery has already
    // happened (or not, if there was no journal to recover).
    let mut second = spawn_record(root);
    let restarted = poll_until(Duration::from_secs(5), || {
        let starts = epoch_events(root)
            .iter()
            .filter(|e| e.as_str() == "start")
            .count();
        (starts >= 2).then_some(())
    });
    assert!(restarted.is_some(), "second daemon never started");
    // Let the fresh daemon settle briefly, then shut it down cleanly so it
    // can't itself mint a fresh turn under the same id and mask a failure.
    std::thread::sleep(Duration::from_millis(500));
    let _ = second.kill();
    let _ = second.wait();

    let logged_ids: HashSet<String> = turns(root)
        .iter()
        .filter_map(|t| t["id"].as_str().map(|s| s.to_string()))
        .collect();
    assert!(
        logged_ids.contains(&referenced_id),
        "DANGLE: memory record's source_turns references turn {referenced_id}, \
         which never appears in log.jsonl after crash recovery — \
         logged turn ids: {logged_ids:?}"
    );
}
