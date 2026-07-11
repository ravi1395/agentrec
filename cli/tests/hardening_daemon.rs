//! Pre-launch hardening round (2026-07-11), Batch D (daemon/state/service/
//! doctor). Integration coverage for findings that need a real spawned
//! `agentrec record` process — D1 (SIGTERM shutdown) and D2 (the flock lock
//! actually releasing on exit). Everything else in Batch D is covered by
//! inline unit tests in `cli/src/daemon.rs`/`doctorcmd.rs`/`service.rs`.
//!
//! Deliberately a separate file from `cli/tests/integration.rs` (owned by
//! another concurrent hardening pass) — helpers below are intentionally
//! duplicated rather than shared, to avoid touching that file.

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
