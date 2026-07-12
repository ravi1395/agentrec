//! Pre-launch hardening round (2026-07-11), Batch D (daemon/state/service/
//! doctor). Integration coverage for findings that need a real spawned
//! `agentrec record` process — D1 (SIGTERM shutdown) and D2 (the flock lock
//! actually releasing on exit). Everything else in Batch D is covered by
//! inline unit tests in `cli/src/daemon.rs`/`doctorcmd.rs`/`service.rs`.
//!
//! Deliberately a separate file from `cli/tests/integration.rs` (owned by
//! another concurrent hardening pass) — helpers below are intentionally
//! duplicated rather than shared, to avoid touching that file.

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
