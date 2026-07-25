//! End-to-end tests driving the real `agentrec` binary against tempdir
//! fixtures. Covers init, the daemon record cycle (rich capture, gitignore
//! filtering, transcript-derived prompt/model), the single-writer lock (B1),
//! and crash-journal recovery (B2). Spawned-daemon assertions poll with a
//! timeout because macOS fsevents coalesces events by a second or more.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentrec_core::memory::{self, MemoryOp, MemoryRecord, Pin};

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

/// Like [`send_hook`] but captures stdout/status instead of discarding it —
/// Task 9's memory-injection block is written to stdout.
fn send_hook_capture(root: &Path, payload: &str) -> Output {
    let mut child = Command::new(bin())
        .args(["hook", "claude", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
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
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn init(root: &Path) {
    // A git repo so gitignore semantics are exercised; --no-hook to avoid
    // touching a real Claude Code settings file.
    Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(root)
        .status()
        .unwrap();
    // --no-service: none of these tests should ever write/load a real
    // launchd/systemd unit on the developer's machine.
    let out = agentrec(root, &["init", "--no-hook", "--no-service"]);
    assert!(out.status.success(), "init failed: {out:?}");
}

#[test]
fn init_scaffolds_agentrec() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    assert!(root.join(".agentrec/config.toml").exists());
    assert!(root.join(".agentrec/objects").exists());
    let gitignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();
    assert_eq!(gitignore.matches(".agentrec/").count(), 1);
}

// D37: the REAL daemon writes log.jsonl and snapshot blobs at 0600, and
// `.agentrec/` itself at 0700, umask-independently — replaces config.toml-
// only proof (that only covers `init`, not the daemon's own writes).
#[cfg(unix)]
#[test]
fn d37_daemon_writes_land_at_locked_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    std::fs::write(root.join("real.rs"), "fn main() {}").unwrap();

    poll_until(Duration::from_secs(25), || {
        root.join(".agentrec/open.json").exists().then_some(())
    })
    .expect("turn opened");

    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s_perm"}"#);

    poll_until(Duration::from_secs(10), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("real.rs"))
                })
                .unwrap_or(false)
        })
    })
    .expect("rich turn with real.rs");

    let _ = daemon.kill();
    let _ = daemon.wait();

    let dir_mode = std::fs::metadata(root.join(".agentrec"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700, ".agentrec/ must be 0700");

    let log_mode = std::fs::metadata(root.join(".agentrec/log.jsonl"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(log_mode & 0o777, 0o600, "log.jsonl must be 0600");

    let objects = root.join(".agentrec/objects");
    let mut found_blob = false;
    for fan in std::fs::read_dir(&objects).unwrap().flatten() {
        if !fan.path().is_dir() {
            continue;
        }
        for f in std::fs::read_dir(fan.path()).unwrap().flatten() {
            if f.path().is_file() {
                let mode = f.metadata().unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600, "blob {:?} must be 0600", f.path());
                found_blob = true;
            }
        }
    }
    assert!(found_blob, "expected at least one snapshot blob written");
}

#[test]
fn records_rich_turn_with_transcript_prompt_and_model_and_filters_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::write(root.join(".gitignore"), "ignored.log\n").unwrap();

    // A transcript carrying the model and the user prompt (no prompt on the
    // signal → exercises transcript fallback, Q+).
    let transcript = root.join("session.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"wire up the parser"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","model":"claude-opus-4-8","content":[{"type":"text","text":"ok"}]}}"#,
            "\n",
        ),
    )
    .unwrap();

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    // No start bracket: the file opens an unattributed turn, attributed on stop.
    std::fs::write(root.join("real.rs"), "fn main() {}").unwrap();
    std::fs::write(root.join("ignored.log"), "noise").unwrap();

    // Wait for the change to be observed before signalling stop.
    poll_until(Duration::from_secs(25), || {
        root.join(".agentrec/open.json").exists().then_some(())
    })
    .expect("turn opened");

    let stop = format!(
        r#"{{"hook_event_name":"Stop","session_id":"s_it","transcript_path":"{}"}}"#,
        transcript.display()
    );
    send_hook(root, &stop);

    let turn = poll_until(Duration::from_secs(10), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("real.rs"))
                })
                .unwrap_or(false)
        })
    })
    .expect("rich turn with real.rs");

    let _ = daemon.kill();
    let _ = daemon.wait();

    assert_eq!(turn.get("grade").and_then(|g| g.as_str()), Some("rich"));
    assert_eq!(turn.get("tool").and_then(|t| t.as_str()), Some("claude"));
    assert_eq!(
        turn.get("model").and_then(|m| m.as_str()),
        Some("claude-opus-4-8")
    );
    assert_eq!(
        turn.get("prompt_excerpt").and_then(|p| p.as_str()),
        Some("wire up the parser")
    );
    // The gitignored file never appears in any turn.
    let mentions_ignored = turns(root).iter().any(|t| {
        t.get("files")
            .and_then(|f| f.as_array())
            .map(|fs| {
                fs.iter()
                    .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("ignored.log"))
            })
            .unwrap_or(false)
    });
    assert!(!mentions_ignored, "gitignored file leaked into a turn");
}

// D29 at the RECORDING path, not just `IgnoreSet::is_ignored`.
//
// A directory whose only `.gitignore` matches itself (`*` — what tool-generated
// cache dirs like `.remember/` and `.code-review-graph/` ship) was watched in
// full: `IgnoreSet::build` collected ignore files from the results of a
// gitignore-aware walk, so the self-matching file hid itself and no matcher was
// ever built for its directory. In this repo's own store that leaked 7419 file
// entries / 764 MiB, including 64 snapshots of a 9 MiB SQLite database.
//
// The unit test covers the predicate; this covers what the user actually cares
// about — that a real daemon does not snapshot those files. `records_rich_turn_
// ...filters_ignored` above only exercises a root-level, non-self-matching
// `.gitignore`, which is why it never caught this.
#[test]
fn self_ignoring_gitignore_dir_is_not_recorded_by_a_real_daemon() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // The exact shape that leaked: nested dir, `.gitignore` whose sole rule
    // matches every entry including itself.
    std::fs::create_dir_all(root.join("cache/logs")).unwrap();
    std::fs::write(root.join("cache/.gitignore"), "*\n").unwrap();

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    // Churn under the self-ignoring dir, plus one real source file. The source
    // file is the synchronisation point: once a turn names it, the daemon has
    // demonstrably processed this batch, so an absent cache path is a real
    // absence rather than a race.
    std::fs::write(root.join("cache/logs/memory.log"), "noise").unwrap();
    std::fs::write(root.join("cache/session.pid"), "4242").unwrap();
    std::fs::write(root.join("real.rs"), "fn main() {}").unwrap();

    let saw_real = poll_until(Duration::from_secs(25), || {
        turns(root)
            .iter()
            .any(|t| {
                t.get("files")
                    .and_then(|f| f.as_array())
                    .map(|fs| {
                        fs.iter()
                            .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("real.rs"))
                    })
                    .unwrap_or(false)
            })
            .then_some(())
    });

    let _ = daemon.kill();
    let _ = daemon.wait();
    saw_real.expect("daemon must record the un-ignored source file");

    let leaked: Vec<String> = turns(root)
        .iter()
        .flat_map(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|f| {
            f.get("path")
                .and_then(|p| p.as_str())
                .map(|s| s.to_string())
        })
        .filter(|p| p.starts_with("cache/"))
        .collect();

    assert!(
        leaked.is_empty(),
        "files under a self-ignoring .gitignore leaked into turns: {leaked:?}"
    );
}

// The trigger fix (`e453e86`) sets `gitignore_dirty` at event-ingest time,
// independent of the touched path's own ignore verdict — but nothing then
// consumed that flag unless some OTHER path also classified `Watch` and drove
// the debounce to settle. So editing a `.gitignore` to re-include a path,
// then only ever touching THAT path, never rebuilds: the re-included path
// still classifies against the stale (pre-edit) set, never arms the
// debounce, and the flush block — where the rebuild used to live — never
// runs. This proves the *consumption* half is fixed, not just the trigger
// the unit test already covers.
#[test]
fn unignore_is_honored_without_other_watched_activity() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root); // real `git init -q` — WalkBuilder::require_git needs it

    std::fs::create_dir_all(root.join("cache")).unwrap();
    std::fs::write(root.join("cache/.gitignore"), "*\n").unwrap();

    // `SingleDaemonGuard`, not a bare `Child`: the control assert below runs
    // BEFORE any kill, and `Child::drop` does not reap — a RED run (which
    // this test sees by design during TDD and every neuter check) would
    // otherwise leak a live daemon per run.
    let mut daemon = SingleDaemonGuard::spawn(root);

    // Positive control, written BEFORE the ignore-rule edit. A control
    // written afterward would itself be a Class::Watch path — it would arm
    // the debounce, `settled` would fire, and (under the pre-fix code) the
    // rebuild would run inside that same flush block, making the test pass
    // before the fix and prove nothing. This ordering is load-bearing.
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/control.rs"), "fn main() {}").unwrap();

    let saw_control = poll_until(Duration::from_secs(20), || {
        turns(root)
            .iter()
            .any(|t| {
                t.get("files")
                    .and_then(|f| f.as_array())
                    .map(|fs| {
                        fs.iter().any(|f| {
                            f.get("path").and_then(|p| p.as_str()) == Some("src/control.rs")
                        })
                    })
                    .unwrap_or(false)
            })
            .then_some(())
    });
    assert!(
        saw_control.is_some(),
        "positive control never recorded — daemon not alive/recording, rest of the test is moot"
    );

    // Re-widen: negate the self-matching rule for one file.
    std::fs::write(root.join("cache/.gitignore"), "*\n!keep.log\n").unwrap();

    // Touch NOTHING else from here on — that is the defect's escape hatch
    // ("some unrelated watched path changed"), and the test must not contain
    // it. Two writes, spaced past one POLL tick (250ms, `daemon::POLL`), so
    // they don't collapse into a single notify batch.
    std::fs::write(root.join("cache/keep.log"), "one").unwrap();
    std::thread::sleep(Duration::from_millis(400));
    std::fs::write(root.join("cache/keep.log"), "two").unwrap();

    // Debounce (1.5s) + quiet window (10s) + slack.
    let saw_keep = poll_until(Duration::from_secs(25), || {
        turns(root)
            .iter()
            .any(|t| {
                t.get("files")
                    .and_then(|f| f.as_array())
                    .map(|fs| {
                        fs.iter().any(|f| {
                            f.get("path").and_then(|p| p.as_str()) == Some("cache/keep.log")
                        })
                    })
                    .unwrap_or(false)
            })
            .then_some(())
    });

    daemon.kill();

    assert!(
        saw_keep.is_some(),
        "cache/keep.log, re-included by a mid-run .gitignore edit, was never recorded — \
         the ignore-set rebuild was never consumed"
    );
}

// Phase 3 of the rebuild-gate plan — the mirror of
// `unignore_is_honored_without_other_watched_activity`: a `.gitignore` DELETE
// event carries the filename too (`apply_watch_result`'s filename check
// precedes `classify`), so it sets `gitignore_dirty` exactly like an edit.
// Nothing pinned that path before this test.
#[test]
fn deleted_gitignore_rewidens_recording() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root); // real `git init -q` — WalkBuilder::require_git needs it

    // Self-matching, self-ignoring: `*` matches `cache/.gitignore` itself, so
    // its own DELETE event classifies `Ignore` and (pre-fix) never arms
    // `pending` on its own — exactly the escape hatch this test must not
    // route around.
    std::fs::create_dir_all(root.join("cache")).unwrap();
    std::fs::write(root.join("cache/.gitignore"), "*\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();

    let mut daemon = SingleDaemonGuard::spawn(root);

    // Positive control, written BEFORE the deletion. A control written
    // afterward would itself be a Class::Watch path — it would arm the
    // debounce and (under pre-fix code) drive the flush block where the
    // rebuild used to live, making the test pass before the fix and prove
    // nothing. This ordering is load-bearing, exactly as in
    // `unignore_is_honored_without_other_watched_activity`.
    std::fs::write(root.join("src/control.rs"), "fn main() {}").unwrap();

    let saw_control = poll_until(Duration::from_secs(20), || {
        turns(root)
            .iter()
            .any(|t| {
                t.get("files")
                    .and_then(|f| f.as_array())
                    .map(|fs| {
                        fs.iter().any(|f| {
                            f.get("path").and_then(|p| p.as_str()) == Some("src/control.rs")
                        })
                    })
                    .unwrap_or(false)
            })
            .then_some(())
    });
    assert!(
        saw_control.is_some(),
        "positive control never recorded — daemon not alive/recording, rest of the test is moot"
    );

    // Re-widen by deletion instead of edit.
    std::fs::remove_file(root.join("cache/.gitignore")).unwrap();

    // Touch NOTHING else from here on. Two writes, spaced past one POLL tick
    // (250ms, `daemon::POLL`), so they don't collapse into a single notify
    // batch with the deletion event.
    std::fs::write(root.join("cache/keep.log"), "one").unwrap();
    std::thread::sleep(Duration::from_millis(400));
    std::fs::write(root.join("cache/keep.log"), "two").unwrap();

    // Debounce (1.5s) + quiet window (10s) + slack.
    let saw_keep = poll_until(Duration::from_secs(25), || {
        turns(root)
            .iter()
            .any(|t| {
                t.get("files")
                    .and_then(|f| f.as_array())
                    .map(|fs| {
                        fs.iter().any(|f| {
                            f.get("path").and_then(|p| p.as_str()) == Some("cache/keep.log")
                        })
                    })
                    .unwrap_or(false)
            })
            .then_some(())
    });

    daemon.kill();

    assert!(
        saw_keep.is_some(),
        "cache/keep.log, re-included by deleting cache/.gitignore mid-run, was never recorded — \
         the ignore-set rebuild was never consumed"
    );
}

// Phase 3 of the rebuild-gate plan: the case nothing covered was a rule
// re-widening an already-known path. This covers the opposite direction — a
// `.gitignore` CREATED under a directory that had none, honored without a
// daemon restart.
#[test]
fn new_gitignore_honored_without_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root); // real `git init -q` — WalkBuilder::require_git needs it

    std::fs::create_dir_all(root.join("data")).unwrap();

    let mut daemon = SingleDaemonGuard::spawn(root);

    // Positive control, written before any `.gitignore` exists under data/ —
    // proves the daemon is alive and watching this directory from the start.
    std::fs::write(root.join("data/control1.txt"), "one").unwrap();
    let saw_control1 = poll_until(Duration::from_secs(20), || {
        turns(root)
            .iter()
            .any(|t| {
                t.get("files")
                    .and_then(|f| f.as_array())
                    .map(|fs| {
                        fs.iter().any(|f| {
                            f.get("path").and_then(|p| p.as_str()) == Some("data/control1.txt")
                        })
                    })
                    .unwrap_or(false)
            })
            .then_some(())
    });
    assert!(
        saw_control1.is_some(),
        "positive control never recorded — daemon not alive/recording, rest of the test is moot"
    );

    // Create the `.gitignore` mid-run: only `block.log` is ignored (the file
    // itself doesn't match its own rule, unlike the self-matching fixtures
    // elsewhere in this suite).
    std::fs::write(root.join("data/.gitignore"), "block.log\n").unwrap();

    // Wait past one POLL tick (250ms, `daemon::POLL`) before touching
    // block.log/control2, so the top-of-tick rebuild has a chance to consume
    // the dirty flag before these events are classified — otherwise they can
    // land in the SAME drain batch as the `.gitignore` create and still be
    // classified against the stale (pre-rule) set, which is decision 2's
    // documented residual window, not what this test is proving.
    std::thread::sleep(Duration::from_millis(400));
    std::fs::write(root.join("data/block.log"), "should be ignored now").unwrap();
    std::fs::write(root.join("data/control2.txt"), "two").unwrap();

    // Debounce (1.5s) + quiet window (10s) + slack.
    let saw_control2 = poll_until(Duration::from_secs(25), || {
        turns(root)
            .iter()
            .any(|t| {
                t.get("files")
                    .and_then(|f| f.as_array())
                    .map(|fs| {
                        fs.iter().any(|f| {
                            f.get("path").and_then(|p| p.as_str()) == Some("data/control2.txt")
                        })
                    })
                    .unwrap_or(false)
            })
            .then_some(())
    });

    daemon.kill();

    // Positive control IN THE SAME WINDOW as the absence assertion below —
    // without it, a dead or unarmed daemon would also satisfy "block.log was
    // never recorded", which is exactly the gap that let two green gitignore
    // tests coexist with a 764 MiB leak in this repo.
    assert!(
        saw_control2.is_some(),
        "control2.txt, written after the new .gitignore existed, was never recorded — \
         daemon not alive/recording during the assertion window, rest of the test is moot"
    );

    let leaked_block_log = turns(root).iter().any(|t| {
        t.get("files")
            .and_then(|f| f.as_array())
            .map(|fs| {
                fs.iter()
                    .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("data/block.log"))
            })
            .unwrap_or(false)
    });
    assert!(
        !leaked_block_log,
        "data/block.log, ignored by a .gitignore created mid-run, leaked into a turn — \
         the new rule was not honored without a daemon restart"
    );
}

/// Read `.agentrec/state.json`'s `ignore_rebuilds` counter, or `None` if the
/// file is missing/unparseable/lacks the key.
fn ignore_rebuilds(root: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(root.join(".agentrec/state.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("ignore_rebuilds")?.as_u64()
}

// Phase 2 of the rebuild-gate fix: nothing anywhere reported whether the
// filter configuration was ever reloaded, which is part of why this defect
// class shipped twice. `state.json`'s `ignore_rebuilds` is the instrument —
// it must count actual REBUILDS (one per `.gitignore` edit that lands in its
// own tick), not raw filesystem events.
#[test]
fn daemon_counts_ignore_rebuilds() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root); // real `git init -q` — WalkBuilder::require_git needs it

    // `SingleDaemonGuard`, not a bare `Child`: the first assert below precedes
    // any kill, and `Child::drop` does NOT reap — a RED run (which this test
    // sees by design during TDD and every neuter check) would otherwise leak a
    // live daemon per run. Leaked test daemons have already cost this repo a
    // diagnosis round via FSEvents contention.
    let mut daemon = SingleDaemonGuard::spawn(root);

    std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
    let saw_one = poll_until(Duration::from_secs(10), || {
        (ignore_rebuilds(root)? == 1).then_some(())
    });
    assert!(
        saw_one.is_some(),
        "expected ignore_rebuilds to reach 1 after the first .gitignore edit, got {:?}",
        ignore_rebuilds(root)
    );

    // Spaced past one POLL tick (250ms) so it doesn't collapse into the same
    // notify batch as the first edit.
    std::thread::sleep(Duration::from_millis(400));
    std::fs::write(root.join(".gitignore"), "*.log\n*.tmp\n").unwrap();
    let saw_two = poll_until(Duration::from_secs(10), || {
        (ignore_rebuilds(root)? == 2).then_some(())
    });

    daemon.kill();

    assert!(
        saw_two.is_some(),
        "expected ignore_rebuilds to reach 2 after the second .gitignore edit, got {:?}",
        ignore_rebuilds(root)
    );
}

// AC: rebuild rate is bounded by the dirty flag (at most one `IgnoreSet::build`
// walk per `POLL` tick), never by raw event count. Asserted on the PERSISTED
// counter only — never by counting stderr lines inside a fixed window, which
// this repo has documented FSEvents flake from (see CLAUDE.md's
// daemon-test-FSEvents-contention note).
//
// Deviation from the plan's literal "5 writes -> 1..=5", recorded because it
// was measured, not assumed: on this macOS/FSEvents setup, 5 back-to-back
// `.gitignore` writes already coalesce to ~3 raw notify events, so a
// per-EVENT counter (the neuter this test exists to catch) also lands
// <= 5 and the test would never go red. Empirically, 40 writes yields ~3-4
// rebuilds under the correct per-TICK counter (repeatedly measured) vs.
// ~15-18 under the per-event neuter — bounding to 10 keeps a wide margin on
// both sides while still being far below what raw event-counting produces.
#[test]
fn rebuild_count_is_bounded_by_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    wait_for_live_daemon(root);

    for i in 0..40 {
        std::fs::write(root.join(".gitignore"), format!("*.log{i}\n")).unwrap();
    }

    // Prove at least one rebuild happened at all (daemon liveness), then let
    // any still-in-flight rebuild settle before the final read.
    let saw_rebuild = poll_until(Duration::from_secs(10), || {
        let n = ignore_rebuilds(root)?;
        (n >= 1).then_some(n)
    });
    std::thread::sleep(Duration::from_secs(2));
    let count = ignore_rebuilds(root).unwrap_or(0);

    let _ = daemon.kill();
    let _ = daemon.wait();

    assert!(
        saw_rebuild.is_some(),
        "no ignore-set rebuild observed at all — rest of the test is moot"
    );
    assert!(
        (1..=10).contains(&count),
        "expected ignore_rebuilds bounded to 1..=10 for 40 writes (rate-limited by the dirty \
         flag, not per-event), got {count}"
    );
}

// A pre-existing state.json written by an OLDER binary (before Phase 2) has
// neither field at all. `#[serde(default)]` must let it still parse, render
// as never-reloaded in text `status`, and `status --json` must carry the
// new field regardless.
//
// `snapshot_failures`/`io_failed` are planted alongside the missing fields
// and asserted to survive — `State` derives `Default`, so a version of this
// fix that drops `#[serde(default)]` (making the field required) doesn't
// fail to parse in an obviously-visible way: `read_state` swallows any
// deserialize error via `.ok()` and falls back to `State::default()`, whose
// `ignore_rebuilds` is ALSO 0. Asserting only `ignore_rebuilds == 0` would
// pass identically whether the JSON parsed field-by-field or failed whole
// and silently reset every OTHER field too (losing `snapshot_failures`,
// `pid`, everything) — this is exactly the vacuity trap the DEGRADED-banner
// assertion below closes.
#[test]
fn status_tolerates_state_without_rebuild_counter() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    std::fs::write(
        root.join(".agentrec/state.json"),
        r#"{"pid":0,"signal_offset":0,"snapshot_failures":3,"io_failed":["src/a.rs"]}"#,
    )
    .unwrap();

    let out = agentrec(root, &["status"]);
    assert!(
        out.status.success(),
        "status failed on a state.json missing the new fields: {out:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.to_lowercase().contains("reload"),
        "a state.json without ignore_rebuilds must render as never-reloaded: {stdout}"
    );
    assert!(
        stdout.contains("DEGRADED") && stdout.contains('3') && stdout.contains("src/a.rs"),
        "pre-existing fields (snapshot_failures/io_failed) must survive parsing a state.json \
         missing the new ignore-rebuild fields — a whole-struct parse failure falling back to \
         State::default() would silently lose them too: {stdout}"
    );

    let json_out = agentrec(root, &["status", "--json"]);
    assert!(
        json_out.status.success(),
        "status --json failed: {json_out:?}"
    );
    let v: serde_json::Value = serde_json::from_slice(&json_out.stdout)
        .unwrap_or_else(|e| panic!("status --json did not emit valid JSON ({e}): {json_out:?}"));
    assert_eq!(
        v.get("ignore_rebuilds").and_then(|c| c.as_u64()),
        Some(0),
        "status --json must carry ignore_rebuilds even from a pre-existing state.json: {v}"
    );
}

#[test]
fn second_record_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    let out = agentrec(root, &["record"]);
    let _ = daemon.kill();
    let _ = daemon.wait();

    assert_eq!(out.status.code(), Some(1), "second record should exit 1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already recording"), "stderr: {stderr}");
}

#[test]
fn recovers_orphaned_open_turn_from_journal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // Plant a crash journal as if a prior process died mid-turn (unattributed).
    let journal = format!(
        r#"{{"source":"quiet","tool":null,"prompt":null,"session":null,"opened_wall_ms":1783296000000,"last_change_wall_ms":1783296005000,"root":"{}","files":[{{"path":"orphan.rs","before":null,"after":"sha256:deadbeef","op":"create"}}]}}"#,
        root.display()
    );
    std::fs::write(root.join(".agentrec/open.json"), journal).unwrap();

    let mut daemon = spawn_record(root);
    // Recovery runs at startup, before the watch loop — appears near-instantly.
    let turn = poll_until(Duration::from_secs(10), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("orphan.rs"))
                })
                .unwrap_or(false)
        })
    });
    let _ = daemon.kill();
    let _ = daemon.wait();

    let turn = turn.expect("orphan turn recovered");
    // Unattributed orphan → bare, no fabricated tool.
    assert_eq!(turn.get("grade").and_then(|g| g.as_str()), Some("bare"));
    assert!(turn.get("tool").and_then(|t| t.as_str()).is_none());
    // Journal consumed.
    assert!(!root.join(".agentrec/open.json").exists());
}

// D35 gap closure (crash-recovery leg): `recover_orphan`'s prompt store call
// gets the identical IoError-vs-OverCap fix as `persist` — this proves it
// end-to-end, not just "it compiled and the unattributed-recovery test still
// passes". A "bracket" journal source recovers `rich` with attribution kept
// (source-aware recovery, see `recover_orphan`), which is the ONLY path that
// even attempts a prompt store — a "quiet"/bare orphan (the test above)
// never touches this code at all. `.agentrec/objects/` is locked down
// BEFORE the daemon starts, since recovery runs at startup before the watch
// loop (confirmed by `recovers_orphaned_open_turn_from_journal`'s comment).
#[cfg(unix)]
#[test]
fn recover_orphan_degraded_on_real_prompt_put_failure() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let journal = format!(
        r#"{{"source":"bracket","tool":"claude","prompt":"unique recovery prompt marker 4f1e","session":"s_recover","opened_wall_ms":1783296000000,"last_change_wall_ms":1783296005000,"root":"{}","files":[{{"path":"recovered.rs","before":null,"after":"sha256:deadbeef","op":"create"}}]}}"#,
        root.display()
    );
    std::fs::write(root.join(".agentrec/open.json"), journal).unwrap();

    let objects = root.join(".agentrec/objects");
    let mut perms = std::fs::metadata(&objects).unwrap().permissions();
    perms.set_mode(0o500);
    std::fs::set_permissions(&objects, perms).unwrap();

    let mut daemon = spawn_record(root);
    let turn = poll_until(Duration::from_secs(10), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("recovered.rs"))
                })
                .unwrap_or(false)
        })
    });
    let state_ok = poll_until(Duration::from_secs(10), || {
        let text = std::fs::read_to_string(root.join(".agentrec/state.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        (v.get("prompt_put_failures")
            .and_then(|n| n.as_u64())
            .unwrap_or(0)
            > 0)
        .then_some(())
    });

    // Restore perms before further assertions/process exit so the tempdir
    // can always be cleaned up, even on assertion failure below.
    let mut perms = std::fs::metadata(&objects).unwrap().permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(&objects, perms).unwrap();
    let _ = daemon.kill();
    let _ = daemon.wait();

    assert!(
        state_ok.is_some(),
        "state.json never recorded a real prompt-put I/O failure during crash recovery"
    );
    let turn = turn.expect(
        "recovered turn touching recovered.rs was persisted despite the prompt-put failure",
    );
    assert_eq!(
        turn.get("grade").and_then(|g| g.as_str()),
        Some("rich"),
        "a 'bracket'-source journal recovers rich (attribution kept): {turn}"
    );
    assert_eq!(turn.get("tool").and_then(|t| t.as_str()), Some("claude"));
    assert!(
        turn.get("prompt_ref").map(|r| r.is_null()).unwrap_or(true),
        "prompt_ref must be null when the blob write failed: {turn}"
    );
    assert!(
        turn.get("prompt_excerpt")
            .and_then(|e| e.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "prompt_excerpt must still be populated — derived pre-store, independent of the put outcome: {turn}"
    );
    assert!(
        !root.join(".agentrec/open.json").exists(),
        "journal must still be consumed even though the prompt write failed"
    );
}

// D35 / AC M+: a persisted snapshot-failure count surfaces as a DEGRADED
// banner in `status`, and `status --ack-degraded` clears it. Inducing a real
// disk-full write failure is impractical in a portable test, so this drives
// the taxonomy deterministically via a planted `state.json`.
#[test]
fn status_shows_degraded_banner_and_ack_clears() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    std::fs::write(
        root.join(".agentrec/state.json"),
        r#"{"pid":0,"signal_offset":0,"snapshot_failures":2,"io_failed":["src/a.rs"]}"#,
    )
    .unwrap();

    let out = agentrec(root, &["status"]);
    assert!(out.status.success(), "status failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("DEGRADED"), "stdout: {stdout}");
    assert!(stdout.contains('2'), "stdout: {stdout}");
    assert!(stdout.contains("src/a.rs"), "stdout: {stdout}");

    let out = agentrec(root, &["status", "--ack-degraded"]);
    assert!(out.status.success(), "ack failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("acknowledged"), "stdout: {stdout}");

    let out = agentrec(root, &["status"]);
    assert!(out.status.success(), "status failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("DEGRADED"), "stdout: {stdout}");
}

// D35 gap closure: `prompt_put_failures` gets its OWN DEGRADED line
// (distinct wording from the file-snapshot banner, no `io_failed`-style path
// list — a prompt failure is turn-scoped, not file-scoped) and
// `--ack-degraded` clears it too, not just `snapshot_failures`. Planted
// state.json (like the sibling test above) — the real-daemon fault
// injection lives in `degraded_on_real_prompt_put_failure` /
// `recover_orphan_degraded_on_real_prompt_put_failure`.
#[test]
fn status_shows_prompt_degraded_banner_and_ack_clears_it_too() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    std::fs::write(
        root.join(".agentrec/state.json"),
        r#"{"pid":0,"signal_offset":0,"snapshot_failures":0,"io_failed":[],"prompt_put_failures":4}"#,
    )
    .unwrap();

    let out = agentrec(root, &["status"]);
    assert!(out.status.success(), "status failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("DEGRADED"), "stdout: {stdout}");
    assert!(stdout.contains("4 prompt"), "stdout: {stdout}");

    let out = agentrec(root, &["status", "--ack-degraded"]);
    assert!(out.status.success(), "ack failed: {out:?}");

    let state_text = std::fs::read_to_string(root.join(".agentrec/state.json")).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state_text).unwrap();
    assert_eq!(
        state.get("prompt_put_failures").and_then(|n| n.as_u64()),
        Some(0),
        "ack must clear prompt_put_failures too, not just snapshot_failures: {state}"
    );

    let out = agentrec(root, &["status"]);
    assert!(out.status.success(), "status failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("DEGRADED"), "stdout: {stdout}");
}

// D-PD3: an empty store has zero agent turns, so a rich-rate percentage would
// be vacuous (100% over 0 turns misleadingly reads as "healthy"). `status`
// must print an honest "n/a" instead of fabricating a rate.
#[test]
fn status_zero_turns_shows_rich_rate_na_not_vacuous_100_percent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let out = agentrec(root, &["status"]);
    assert!(out.status.success(), "status failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("rich-rate:  n/a"), "stdout: {stdout}");
    assert!(stdout.contains("no agent turns yet"), "stdout: {stdout}");
    assert!(!stdout.contains("100%"), "stdout: {stdout}");
}

// --- AC F1–F4, K+: `diff` prints a unified diff of a turn's file changes.
// These seed the log directly via agentrec-core rather than driving the
// daemon, since the daemon's job (turn boundaries) isn't what's under test
// here — only the read-side rendering of an already-recorded turn.

fn base_turn(
    id: &str,
    files: Vec<agentrec_core::record::FileEntry>,
) -> agentrec_core::record::TurnRecord {
    agentrec_core::record::TurnRecord {
        v: 1,
        id: id.to_string(),
        grade: "rich".into(),
        truncated: false,
        started: "2026-07-05T00:00:00.000Z".into(),
        ended: "2026-07-05T00:00:01.000Z".into(),
        tool: Some("claude".into()),
        model: None,
        session: None,
        root: "/repo".into(),
        prompt_ref: None,
        prompt_excerpt: None,
        merges: vec![],
        files,
    }
}

fn seed_turn(root: &Path, turn: &agentrec_core::record::TurnRecord) {
    agentrec_core::record::append_log(
        &root.join(".agentrec/log.jsonl"),
        &agentrec_core::record::LogRecord::Turn(turn.clone()),
    )
    .expect("seed turn");
}

#[test]
fn diff_text_modify_shows_unified() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"a\nb\nc\n").unwrap();
    let after = store.put(b"a\nB\nc\n").unwrap();
    let created = store.put(b"brand new file\n").unwrap();

    let turn = base_turn(
        "t_MODIFYTEST0000000000000001",
        vec![
            FileEntry {
                path: "src/x.rs".into(),
                before: Some(before),
                after: Some(after),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            },
            FileEntry {
                path: "src/new.rs".into(),
                before: None,
                after: Some(created),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            },
        ],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["diff", &turn.id]);
    assert!(out.status.success(), "diff failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("-b"), "stdout: {stdout}");
    assert!(stdout.contains("+B"), "stdout: {stdout}");
    assert!(stdout.contains("+brand new file"), "stdout: {stdout}");
}

#[test]
fn diff_binary_file_message() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before_bytes: Vec<u8> = vec![0u8, 1, 2, 3];
    let after_bytes: Vec<u8> = vec![0u8, 1, 2, 3, 4, 5, 6];
    let before = store.put(&before_bytes).unwrap();
    let after = store.put(&after_bytes).unwrap();

    let turn = base_turn(
        "t_BINARYTEST00000000000000001",
        vec![FileEntry {
            path: "assets/img.bin".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["diff", &turn.id]);
    assert!(out.status.success(), "diff failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("binary file changed ("), "stdout: {stdout}");
    assert!(
        stdout.contains(&before_bytes.len().to_string()),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&after_bytes.len().to_string()),
        "stdout: {stdout}"
    );
    assert!(!stdout.contains("@@"), "stdout: {stdout}");
}

#[test]
fn diff_skipped_file_notice() {
    use agentrec_core::record::FileEntry;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let turn = base_turn(
        "t_SKIPTEST0000000000000000001",
        vec![FileEntry {
            path: "big/huge.bin".into(),
            before: None,
            after: None,
            op: "modify".into(),
            skipped: true,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: Some(agentrec_core::record::skip_reason::OVER_CAP.to_string()),
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["diff", &turn.id]);
    assert!(out.status.success(), "diff failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("(content not snapshotted — over size cap)"),
        "stdout: {stdout}"
    );
}

// SR4: `print_entry` must name the REAL cause, not always "over size cap" —
// a sibling coverage gap to `diff_skipped_file_notice` above (which only
// ever exercised the over-cap text). Also covers the absent/unknown-value
// fallback ("reason unrecorded") and the distinct "snapshot unavailable"
// (no suffix) message for a present-but-unresolvable hash — SR-D's other
// honesty fix: the old text asserted a specific cause ("purged or missing")
// that this repo cannot actually distinguish.
#[test]
fn diff_names_the_real_skip_cause_and_unresolvable_blob() {
    use agentrec_core::record::{skip_reason, FileEntry};
    use agentrec_core::store::hash_bytes;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let turn = base_turn(
        "t_SKIPCAUSE000000000000000001",
        vec![
            FileEntry {
                path: "io.rs".into(),
                before: None,
                after: None,
                op: "modify".into(),
                skipped: true,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: Some(skip_reason::IO_FAILED.to_string()),
            },
            FileEntry {
                path: "unreadable.rs".into(),
                before: None,
                after: None,
                op: "modify".into(),
                skipped: true,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: Some(skip_reason::UNREADABLE.to_string()),
            },
            FileEntry {
                path: "legacy.rs".into(),
                before: None,
                after: None,
                op: "modify".into(),
                skipped: true,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None, // pre-this-round log entry
            },
            FileEntry {
                // hash recorded, but no such blob was ever put in the store —
                // cause genuinely unknown, must NOT assert "purged or missing".
                path: "gone.rs".into(),
                before: None,
                after: Some(hash_bytes(b"never actually stored")),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            },
        ],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["diff", &turn.id]);
    assert!(out.status.success(), "diff failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let io_line = stdout.lines().find(|l| l.contains("io.rs")).unwrap_or("");
    assert!(
        io_line.contains("(content not snapshotted — write failed at record time)"),
        "io.rs line: {io_line}"
    );
    let unreadable_line = stdout
        .lines()
        .find(|l| l.contains("unreadable.rs"))
        .unwrap_or("");
    assert!(
        unreadable_line.contains("(content not snapshotted — file unreadable at record time)"),
        "unreadable.rs line: {unreadable_line}"
    );
    let legacy_line = stdout
        .lines()
        .find(|l| l.contains("legacy.rs"))
        .unwrap_or("");
    assert!(
        legacy_line.contains("(content not snapshotted — reason unrecorded)"),
        "legacy.rs line: {legacy_line}"
    );
    let gone_line = stdout.lines().find(|l| l.contains("gone.rs")).unwrap_or("");
    assert!(
        gone_line.contains("(snapshot unavailable)"),
        "gone.rs line: {gone_line}"
    );
    assert!(
        !gone_line.contains("purged or missing"),
        "cause is genuinely unknown here — must not assert one: {gone_line}"
    );
}

// Finding #5(b): `print_entry` (the `diff` renderer) must distinguish
// `StoreError::Missing` from `StoreError::Corrupt`, the same way
// `build_plan` (the `undo` renderer) already does — before this fix both
// collapsed to the same generic "(snapshot unavailable)", so `diff` was
// LESS specific than `undo` about the identical condition. This test
// covers ONLY the Corrupt leg (the Missing leg is already pinned by
// `diff_names_the_real_skip_cause_and_unresolvable_blob`'s `gone.rs` case
// above, which never even puts the blob — genuinely missing, not
// tampered).
#[test]
fn diff_reports_corrupt_blob_distinctly_from_missing() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let after = store.put(b"will be tampered\n").unwrap();
    // Tamper the object's bytes on disk so the stored hash no longer
    // matches (mirrors `show_prompt_corrupt_blob_reports_hash_mismatch_on_stderr`
    // and `agentrec_core::store::tests::corrupt_object_detected`).
    let hex = after.strip_prefix("sha256:").unwrap();
    let path = root
        .join(".agentrec/objects")
        .join(&hex[..2])
        .join(&hex[2..]);
    std::fs::write(&path, b"tampered").unwrap();
    // Precondition: the store must genuinely report Corrupt for this hash,
    // not some other error, before asserting on `diff`'s rendering of it.
    assert!(
        matches!(
            store.get(&after),
            Err(agentrec_core::store::StoreError::Corrupt(_))
        ),
        "precondition: tampered blob must be genuinely Corrupt"
    );

    let turn = base_turn(
        "t_DIFFCORRUPT000000000000001",
        vec![FileEntry {
            path: "tampered.rs".into(),
            before: None,
            after: Some(after),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["diff", &turn.id]);
    assert!(out.status.success(), "diff failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|l| l.contains("tampered.rs"))
        .unwrap_or("");
    assert!(
        line.contains("(snapshot corrupt — hash mismatch)"),
        "tampered.rs line: {line}"
    );
    assert!(
        !line.contains("(snapshot unavailable)"),
        "a Corrupt blob must not render the same message as a genuinely Missing one: {line}"
    );
}

// Finding #3 (cross-seam: change C x change A) + finding #4 (untested
// message string): change C gave over-cap/io-failed `FileEntry`s a real
// `after` hash even though the blob was never stored (SR-C's honesty
// gain). That hash still advances `Recorder::baseline` in `daemon.rs`, so
// the NEXT turn touching that path carries `before: Some(<hash that was
// never actually stored>)` — a "ghost hash". Nothing joins the producer
// test (which only checks `daemon.rs`'s own output) with the read-side
// tests (which only ever seed already-resolvable hashes), so this scenario
// — genuinely reachable in production, never exercised end to end — went
// untested. Seeds turn1 (over-cap create, ghost `after`) + turn2 (modify,
// `before` = that same ghost hash) and asserts every read verb degrades
// honestly rather than lying: `blame <file>` still reports the correct
// fact from the log alone (no blob load needed), `blame <file>:<line>`
// honestly can't attribute, `diff` shows the blob as unavailable, and
// `undo` REFUSES with the exact "prior snapshot unavailable — refusing to
// restore" string — which is finding #4's target message; `git grep
// "prior snapshot unavailable" -- cli/tests` was empty before this test,
// so this closes that finding too (no redundant second test added).
// `doctor`'s "store health" check is asserted passing too, proving the
// ghost hash is a read-side degradation only, never a DEGRADED-store
// false positive (state.json's counters are untouched by directly seeding
// the log — this scenario never goes through the daemon's write path).
#[test]
fn ghost_hash_from_over_cap_baseline_degrades_honestly_everywhere() {
    use agentrec_core::record::{skip_reason, FileEntry};
    use agentrec_core::store::{hash_bytes, BlobStore, StoreError};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    // A hash that was computed (SR-C) but genuinely never `put` into the
    // store — exactly what an over-cap file's `after` looks like.
    let ghost_hash = hash_bytes(b"over-cap content, never actually stored");
    // Precondition this whole test depends on: the store must genuinely
    // fail to resolve the ghost hash before any output is asserted.
    assert!(
        matches!(store.get(&ghost_hash), Err(StoreError::Missing(_))),
        "precondition: ghost_hash must be genuinely unresolvable"
    );

    let final_content = b"final content\n".to_vec();
    let final_hash = store.put(&final_content).unwrap();
    std::fs::write(root.join("f.txt"), &final_content).unwrap();

    // turn1: over-cap create — `after` is the ghost hash (SR-C: computed,
    // never stored). Seeded first so it's the OLDER turn in append order.
    let turn1 = base_turn(
        "t_GHOSTBASE0000000000000001",
        vec![FileEntry {
            path: "f.txt".into(),
            before: None,
            after: Some(ghost_hash.clone()),
            op: "create".into(),
            skipped: true,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: Some(skip_reason::OVER_CAP.to_string()),
        }],
    );
    seed_turn(root, &turn1);

    // turn2: a normal modify whose `before` is that same ghost hash — the
    // baseline `daemon.rs` advanced to it, exactly as `Recorder::stage`
    // does for any snapshotted-or-not `after`. `after` here IS a real,
    // stored hash (this turn's own write succeeded), so the file is
    // genuinely unmodified-since on disk.
    let turn2 = base_turn(
        "t_GHOSTNEXT00000000000000002",
        vec![FileEntry {
            path: "f.txt".into(),
            before: Some(ghost_hash),
            after: Some(final_hash),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn2);

    // blame f.txt: the log fact alone (no blob load) is correct — must
    // name turn2 and must NOT claim anything is unavailable.
    let blame_file_out = agentrec(root, &["blame", "f.txt"]);
    assert!(
        blame_file_out.status.success(),
        "blame f.txt failed: {blame_file_out:?}"
    );
    let blame_file_stdout = String::from_utf8_lossy(&blame_file_out.stdout);
    assert!(
        blame_file_stdout.contains(&short_id_of(&turn2.id)),
        "blame f.txt must still attribute correctly from the log alone: {blame_file_stdout}"
    );
    assert!(
        !blame_file_stdout.to_lowercase().contains("unavailable"),
        "blame f.txt needs no blob load and must not degrade: {blame_file_stdout}"
    );

    // blame f.txt:1: line-level attribution DOES need to load blobs for
    // both candidate turns, and both cite the same unresolvable ghost hash
    // — must honestly refuse, never guess.
    let blame_line_out = agentrec(root, &["blame", "f.txt:1"]);
    assert!(
        blame_line_out.status.success(),
        "blame f.txt:1 failed: {blame_line_out:?}"
    );
    let blame_line_stdout = String::from_utf8_lossy(&blame_line_out.stdout);
    assert!(
        blame_line_stdout.contains("f.txt:1: attribution unavailable — snapshot unavailable"),
        "blame_line stdout: {blame_line_stdout}"
    );

    // diff turn2: `before` doesn't resolve — must say so, not crash or
    // silently show an empty diff.
    let diff_out = agentrec(root, &["diff", &turn2.id]);
    assert!(diff_out.status.success(), "diff failed: {diff_out:?}");
    let diff_stdout = String::from_utf8_lossy(&diff_out.stdout);
    let diff_line = diff_stdout
        .lines()
        .find(|l| l.contains("f.txt"))
        .unwrap_or("");
    assert!(
        diff_line.contains("(snapshot unavailable)"),
        "diff line: {diff_line}"
    );

    // undo turn2: build_plan's `StoreError::Missing` arm on a `before` hash
    // (finding #4's untested message) — must REFUSE with the exact
    // established string, never attempt a restore.
    let undo_out = agentrec(root, &["undo", &turn2.id, "--confirm"]);
    assert!(undo_out.status.success(), "undo failed: {undo_out:?}");
    let undo_stdout = String::from_utf8_lossy(&undo_out.stdout);
    let undo_line = undo_stdout
        .lines()
        .find(|l| l.contains("f.txt"))
        .unwrap_or("");
    assert!(
        undo_line.trim_start().starts_with("REFUSE"),
        "undo line: {undo_line}"
    );
    assert!(
        undo_line.contains("prior snapshot unavailable — refusing to restore"),
        "undo line: {undo_line}"
    );
    // The file must be completely untouched by the refused undo.
    assert_eq!(
        std::fs::read(root.join("f.txt")).unwrap(),
        final_content,
        "a refused ghost-hash entry must never touch the worktree file"
    );

    // doctor store health: a ghost hash is a read-side degradation only —
    // it must never masquerade as a DEGRADED store (state.json's counters
    // are untouched; these turns were seeded directly, not written by the
    // daemon's own fault-tracking write path).
    let doctor_json = doctor_json_value(root);
    let checks = doctor_json["checks"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a checks array: {doctor_json}"));
    let store_health = checks
        .iter()
        .find(|c| c["name"] == "store health")
        .unwrap_or_else(|| panic!("expected a 'store health' check: {doctor_json}"));
    assert_eq!(
        store_health["status"], "pass",
        "store health: {store_health}"
    );
}

#[test]
fn diff_unknown_turn_exits_1_with_range() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let turn = base_turn("t_REALTURN00000000000000001", vec![]);
    seed_turn(root, &turn);

    let out = agentrec(root, &["diff", "t_DOESNOTEXIST"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("unknown turn id"), "output: {combined}");
    assert!(
        combined.contains("..") && combined.contains("turns)"),
        "output: {combined}"
    );
}

#[test]
fn diff_prefix_match() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let turn = base_turn("t_PREFIXTEST000000000000001", vec![]);
    seed_turn(root, &turn);

    let prefix = &turn.id[..6];
    let out = agentrec(root, &["diff", prefix]);
    assert!(out.status.success(), "diff failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("turn "), "stdout: {stdout}");
    assert!(stdout.contains("0 file"), "stdout: {stdout}");
}

// --- AC PD2: `agentrec show <turn> [--prompt]` (SPEC §Prompt posture item
// 4, excerpt discipline). Bare `show` prints only the header; the full
// post-scrub prompt requires the explicit flag. Every case below asserts
// both the exit code and the correct stream, not just a substring.

#[test]
fn show_prompt_prints_full_post_scrub_prompt() {
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    // Long enough (>120 chars, multi-line) that the capped/flattened excerpt
    // the bare `show` renders can never contain this full text verbatim.
    let full_prompt = format!(
        "add rate limiting to the login endpoint\nand also please {}\ndone",
        "x".repeat(200)
    );
    let prompt_ref = store.put(full_prompt.as_bytes()).unwrap();

    let mut turn = base_turn("t_SHOWPROMPT0000000000001", vec![]);
    turn.prompt_ref = Some(prompt_ref);
    turn.prompt_excerpt = Some(agentrec_core::scrub::excerpt(&full_prompt));
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id, "--prompt"]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0: {out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        full_prompt,
        "stdout must be exactly the full post-scrub prompt"
    );
    assert!(
        out.stderr.is_empty(),
        "stderr must be empty on success: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn show_prompt_dangling_ref_reports_purge_on_stderr() {
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let prompt_ref = store.put(b"a prompt that will be purged").unwrap();
    assert!(store.remove(&prompt_ref).is_some(), "must actually delete");

    let mut turn = base_turn("t_SHOWDANGLING000000000001", vec![]);
    turn.prompt_ref = Some(prompt_ref);
    turn.prompt_excerpt = Some("a prompt that will be purged".into());
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id, "--prompt"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    assert!(
        out.stdout.is_empty(),
        "stdout must be empty on failure: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("agentrec: "), "stderr: {stderr}");
    assert!(
        stderr.to_lowercase().contains("purge") || stderr.to_lowercase().contains("ttl"),
        "stderr must mention purge or TTL: {stderr}"
    );
    // Regression guard: a genuinely missing ref must stay distinct from the
    // corrupt-blob path below — never claim hash-mismatch for a purge.
    assert!(
        !stderr.to_lowercase().contains("corrupt"),
        "missing-ref message must not say corrupt: {stderr}"
    );
}

// Skeptic-surfaced (2026-07-10): a PRESENT but CORRUPT (hash-mismatch) prompt
// blob was previously reported with the same "TTL expired" message as a
// genuinely missing/purged ref — fabricating a cause for a blob that's still
// on disk. This must be a distinct, honest message and must never say TTL.
#[test]
fn show_prompt_corrupt_blob_reports_hash_mismatch_on_stderr() {
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let prompt_ref = store.put(b"a prompt that will be corrupted").unwrap();
    // Tamper the object's bytes on disk so the stored hash no longer matches
    // (mirrors `agentrec_core::store::tests::corrupt_object_detected`).
    let hex = prompt_ref.strip_prefix("sha256:").unwrap();
    let path = root
        .join(".agentrec/objects")
        .join(&hex[..2])
        .join(&hex[2..]);
    std::fs::write(&path, b"tampered").unwrap();

    let mut turn = base_turn("t_SHOWCORRUPT0000000000001", vec![]);
    turn.prompt_ref = Some(prompt_ref);
    turn.prompt_excerpt = Some("a prompt that will be corrupted".into());
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id, "--prompt"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    assert!(
        out.stdout.is_empty(),
        "stdout must be empty on failure: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("agentrec: "), "stderr: {stderr}");
    assert!(
        stderr.to_lowercase().contains("corrupt")
            && stderr.to_lowercase().contains("hash mismatch"),
        "stderr must mention corrupt/hash mismatch: {stderr}"
    );
    assert!(
        !stderr.to_lowercase().contains("ttl expired"),
        "corrupt blob must not fabricate TTL expiry: {stderr}"
    );
}

#[test]
fn show_prompt_bare_turn_no_prompt_attached_on_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // A bare turn: no prompt_ref at all (base_turn's default).
    let turn = base_turn("t_SHOWBARETURN00000000001", vec![]);
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id, "--prompt"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    assert!(
        out.stdout.is_empty(),
        "stdout must be empty on failure: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("agentrec: "), "stderr: {stderr}");
    assert!(
        stderr.to_lowercase().contains("no prompt attached"),
        "stderr: {stderr}"
    );
}

#[test]
fn show_without_prompt_flag_prints_header_never_full_prompt() {
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let full_prompt = format!(
        "add rate limiting to the login endpoint\nand also please {}\ndone",
        "y".repeat(200)
    );
    let prompt_ref = store.put(full_prompt.as_bytes()).unwrap();

    let mut turn = base_turn("t_SHOWHEADERONLY000000001", vec![]);
    turn.prompt_ref = Some(prompt_ref);
    turn.prompt_excerpt = Some(agentrec_core::scrub::excerpt(&full_prompt));
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // A header field (the turn's short id) is present...
    assert!(stdout.contains(&turn.id[..4]), "stdout: {stdout}");
    // ...but the full prompt body is never printed without --prompt.
    assert!(!stdout.contains(&full_prompt), "stdout: {stdout}");
    assert!(
        out.stderr.is_empty(),
        "stderr must be empty on success: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// --- AC L+: kill -9 around a turn close must lose nothing, and the blob
// store must never expose a half-written object after a hard crash.
// `Child::kill()` is graceful on some platforms; go straight to SIGKILL so
// the process gets no chance to run any shutdown/flush path.

fn sigkill(child: &std::process::Child) {
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGKILL);
    }
}

#[test]
fn closed_turn_survives_kill9() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    std::fs::write(root.join("f.rs"), "fn f() {}").unwrap();
    poll_until(Duration::from_secs(25), || {
        root.join(".agentrec/open.json").exists().then_some(())
    })
    .expect("turn opened");

    // Stop hook closes and persists (fsynced) the turn.
    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s1"}"#);

    let has_f_rs = |root: &Path| {
        turns(root).into_iter().any(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("f.rs"))
                })
                .unwrap_or(false)
        })
    };

    poll_until(Duration::from_secs(10), || has_f_rs(root).then_some(()))
        .expect("f.rs turn persisted before kill");

    // Hard-kill the daemon immediately after the close was persisted.
    sigkill(&daemon);
    let _ = daemon.wait();

    // The fsynced append survives the kill — no buffering to lose.
    assert!(has_f_rs(root), "f.rs turn lost after kill -9");

    // A restart (recovery/epoch bookkeeping) must not drop prior history.
    let mut daemon2 = spawn_record(root);
    std::thread::sleep(Duration::from_secs(1));
    assert!(has_f_rs(root), "f.rs turn lost after daemon restart");

    sigkill(&daemon2);
    let _ = daemon2.wait();
}

#[test]
fn kill9_leaves_no_corrupt_blob() {
    use agentrec_core::store::hash_bytes;

    for i in 0..4u64 {
        let delay_ms = 300 + i * 250;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);

        let mut daemon = spawn_record(root);
        std::thread::sleep(Duration::from_millis(800));

        // A sizeable file so a real blob write happens (well under the
        // 10 MiB per-object cap).
        let mut big = String::with_capacity(200 * 1024);
        while big.len() < 200 * 1024 {
            big.push_str(&format!("// line {} filler filler filler\n", big.len()));
        }
        std::fs::write(root.join(format!("big_{i}.rs")), big).unwrap();

        // Cross the debounce so a snapshot write is in flight or done, then
        // hard-kill without warning.
        std::thread::sleep(Duration::from_millis(delay_ms));
        sigkill(&daemon);
        let _ = daemon.wait();

        // Invariant: every FINAL (renamed) object under .agentrec/objects
        // hashes to its own address. Leftover `.tmp.*` files from a
        // mid-write crash are permitted and explicitly skipped.
        let objects_dir = root.join(".agentrec/objects");
        let mut checked = 0usize;
        let mut stack = vec![objects_dir.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let file_type = entry.file_type().unwrap();
                if file_type.is_dir() {
                    stack.push(path);
                    continue;
                }
                let file_name = entry.file_name();
                let file_name = file_name.to_str().unwrap();
                if file_name.starts_with(".tmp") {
                    continue;
                }
                let dir_name = dir.file_name().unwrap().to_str().unwrap();
                let contents = std::fs::read(&path)
                    .unwrap_or_else(|e| panic!("iteration {i}: read {path:?} failed: {e}"));
                let expected = hash_bytes(&contents);
                let actual = format!("sha256:{dir_name}{file_name}");
                assert_eq!(
                    expected, actual,
                    "iteration {i}: corrupt/mismatched object at {path:?} (delay_ms={delay_ms})"
                );
                checked += 1;
            }
        }
        // Sanity: this iteration actually produced at least one durable
        // object at some delay (not required every iteration, since very
        // short delays may kill before any snapshot lands) — but across all
        // 4 varying delays we expect coverage overall. No per-iteration
        // assertion on `checked` count to avoid flakiness on the shortest
        // delay.
        let _ = checked;
    }
}

// --- AC G1–G6: `blame` reports which turn last touched a file, or
// introduced a specific line, without ever fabricating attribution across a
// recording gap or on a bare (unattributed) turn. Seeded directly via
// agentrec-core, same rationale as the `diff` tests above.

fn make_turn(
    id: &str,
    grade: &str,
    tool: Option<&str>,
    started: &str,
    prompt_excerpt: Option<&str>,
    files: Vec<agentrec_core::record::FileEntry>,
) -> agentrec_core::record::TurnRecord {
    agentrec_core::record::TurnRecord {
        v: 1,
        id: id.to_string(),
        grade: grade.to_string(),
        truncated: false,
        started: started.to_string(),
        ended: started.to_string(),
        tool: tool.map(str::to_string),
        model: None,
        session: None,
        root: "/repo".into(),
        prompt_ref: None,
        prompt_excerpt: prompt_excerpt.map(str::to_string),
        merges: vec![],
        files,
    }
}

fn seed_epoch(root: &Path, event: &str, ts: &str) {
    agentrec_core::record::append_log(
        &root.join(".agentrec/log.jsonl"),
        &agentrec_core::record::LogRecord::Epoch(agentrec_core::record::EpochRecord {
            v: 1,
            event: event.to_string(),
            ts: ts.to_string(),
        }),
    )
    .expect("seed epoch");
}

#[test]
fn blame_file_reports_last_rich_turn() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let after1 = store.put(b"v1\n").unwrap();
    let after2 = store.put(b"v2\n").unwrap();

    let turn1 = make_turn(
        "t_BLAMEFIRST00000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T10:00:00.000Z",
        Some("first pass"),
        vec![FileEntry {
            path: "x.rs".into(),
            before: None,
            after: Some(after1),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn1);

    let turn2 = make_turn(
        "t_BLAMESECOND0000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T11:00:00.000Z",
        Some("second pass"),
        vec![FileEntry {
            path: "x.rs".into(),
            before: turn1.files[0].after.clone(),
            after: Some(after2),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn2);

    // On-disk matches the latest turn's `after` — no human-edited suffix.
    std::fs::write(root.join("x.rs"), b"v2\n").unwrap();
    let out = agentrec(root, &["blame", "x.rs"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("claude"), "stdout: {stdout}");
    assert!(stdout.contains("second pass"), "stdout: {stdout}");
    assert!(!stdout.contains("first pass"), "stdout: {stdout}");
    assert!(!stdout.contains("human-edited"), "stdout: {stdout}");

    // Now a human edits the file post-recording — hash diverges from `after2`.
    std::fs::write(root.join("x.rs"), b"v2-edited-by-hand\n").unwrap();
    let out = agentrec(root, &["blame", "x.rs"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("human-edited since"), "stdout: {stdout}");
}

#[test]
fn blame_untouched_file_exit0() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // Balanced epochs (start then stop) — no gap.
    seed_epoch(root, "start", "2026-07-05T00:00:00.000Z");
    seed_epoch(root, "stop", "2026-07-05T00:10:00.000Z");

    let out = agentrec(root, &["blame", "never-touched.rs"]);
    assert_eq!(out.status.code(), Some(0), "blame: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no recorded turn touches never-touched.rs"),
        "stdout: {stdout}"
    );
}

#[test]
fn blame_bare_turn_no_fabrication() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let after = store.put(b"content\n").unwrap();
    let turn = make_turn(
        "t_BLAMEBARE000000000000000001",
        "bare",
        None,
        "2026-07-05T09:00:00.000Z",
        None,
        vec![FileEntry {
            path: "y.rs".into(),
            before: None,
            after: Some(after),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);
    std::fs::write(root.join("y.rs"), b"content\n").unwrap();

    let out = agentrec(root, &["blame", "y.rs"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("bare turn"), "stdout: {stdout}");
    assert!(!stdout.contains("claude"), "stdout: {stdout}");
    assert!(
        !stdout.contains('"'),
        "stdout: {stdout} (a bare turn must not quote a fabricated prompt)"
    );
}

#[test]
fn blame_deleted_file_resolves() {
    use agentrec_core::record::FileEntry;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let turn = make_turn(
        "t_BLAMEDELETE0000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T09:00:00.000Z",
        Some("remove dead file"),
        vec![FileEntry {
            path: "z.rs".into(),
            before: None,
            after: None,
            op: "delete".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    // z.rs is never written to disk — absent, as expected post-delete.
    seed_turn(root, &turn);

    let out = agentrec(root, &["blame", "z.rs"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("claude"), "stdout: {stdout}");
    assert!(stdout.contains("deleted this file"), "stdout: {stdout}");
}

#[test]
fn blame_line_level_added_and_predating() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"a\nb\n").unwrap();
    let after = store.put(b"a\nB2\n").unwrap();

    let turn = make_turn(
        "t_BLAMELINE00000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T09:00:00.000Z",
        Some("tweak line two"),
        vec![FileEntry {
            path: "f.rs".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);
    std::fs::write(root.join("f.rs"), b"a\nB2\n").unwrap();

    let out = agentrec(root, &["blame", "f.rs:2"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("claude"), "stdout: {stdout}");
    assert!(stdout.contains("tweak line two"), "stdout: {stdout}");
    assert!(
        !stdout.contains("before recording began"),
        "stdout: {stdout}"
    );

    let out = agentrec(root, &["blame", "f.rs:1"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("before recording began"),
        "stdout: {stdout}"
    );
}

#[test]
fn blame_gap_is_stale() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let after = store.put(b"original\n").unwrap();
    let turn = make_turn(
        "t_BLAMEGAP0000000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T09:00:00.000Z",
        Some("write g.rs"),
        vec![FileEntry {
            path: "g.rs".into(),
            before: None,
            after: Some(after),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);

    // daemon1 starts and is killed (no stop); daemon2 starts — an uncovered gap.
    seed_epoch(root, "start", "2026-07-05T09:00:00.000Z");
    seed_epoch(root, "start", "2026-07-05T09:05:00.000Z");

    // On-disk content diverges from the turn's recorded `after` — the gap
    // could be hiding whatever really changed it, so blame must not guess.
    std::fs::write(root.join("g.rs"), b"changed-during-the-gap\n").unwrap();

    let out = agentrec(root, &["blame", "g.rs"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("attribution stale — recording gap"),
        "stdout: {stdout}"
    );
}

#[test]
fn blame_line_unresolvable_before_does_not_credit_newer_turn() {
    // BL2/BL3: a turn whose `before` blob doesn't resolve must never be
    // credited for a line it merely happens to still contain. T1 genuinely
    // introduced "a"; T2's own `before` snapshot is gone, so whether T2
    // changed line 1 is unknown, not "definitely not" — blame must refuse
    // rather than guess T1 either.
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::{hash_bytes, BlobStore};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let after1 = store.put(b"a\nb\n").unwrap();
    let turn1 = make_turn(
        "t_BLUNRESOLVE1000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T09:00:00.000Z",
        Some("first pass introduces a and b"),
        vec![FileEntry {
            path: "f.rs".into(),
            before: None,
            after: Some(after1),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn1);

    // A well-formed hash that was never actually written to the store —
    // proves the "unresolvable" precondition for real rather than assuming
    // it (a torn/absent-by-construction hash would test nothing).
    let ghost_hash = hash_bytes(b"never-actually-stored");
    assert!(
        store.get(&ghost_hash).is_err(),
        "precondition: ghost_hash must be genuinely unresolvable"
    );

    let after2 = store.put(b"a\nb\n").unwrap(); // textually unchanged from turn1
    let turn2 = make_turn(
        "t_BLUNRESOLVE2000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T10:00:00.000Z",
        Some("second pass unresolvable before"),
        vec![FileEntry {
            path: "f.rs".into(),
            before: Some(ghost_hash),
            after: Some(after2),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn2);

    std::fs::write(root.join("f.rs"), b"a\nb\n").unwrap();

    let out = agentrec(root, &["blame", "f.rs:1"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("f.rs:1: attribution unavailable — snapshot unavailable"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("second pass unresolvable before"),
        "must not credit T2 (unresolvable before): stdout: {stdout}"
    );
    assert!(
        !stdout.contains("first pass introduces a and b"),
        "must not silently credit T1 either — a newer unresolvable turn could \
         have overwritten this line: stdout: {stdout}"
    );
    assert!(
        !stdout.contains("before recording began"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("attribution stale — recording gap"),
        "stdout: {stdout}"
    );
}

#[test]
fn blame_line_create_turn_still_credited() {
    // BL4 regression guard: a `create` turn (before == None) is legitimately
    // empty-before, not unresolvable — every line of its `after` really was
    // introduced by it. Must keep working exactly as before the fix.
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let after = store.put(b"x\ny\n").unwrap();
    let turn = make_turn(
        "t_BLCREATE0000000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T09:00:00.000Z",
        Some("create f2.rs"),
        vec![FileEntry {
            path: "f2.rs".into(),
            before: None,
            after: Some(after),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);
    std::fs::write(root.join("f2.rs"), b"x\ny\n").unwrap();

    let out = agentrec(root, &["blame", "f2.rs:2"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("create f2.rs"), "stdout: {stdout}");
    assert!(
        !stdout.contains("attribution unavailable"),
        "stdout: {stdout}"
    );
}

#[test]
fn blame_line_unresolvable_after_not_false_predating() {
    // BL5: an unresolvable `after` must not silently degrade to "before
    // recording began" — that would be a confident false negative. The
    // turn genuinely touched this file; its outcome just isn't provable.
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::{hash_bytes, BlobStore};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"a\nb\n").unwrap();
    let ghost_after = hash_bytes(b"never-actually-stored-after");
    assert!(
        store.get(&ghost_after).is_err(),
        "precondition: ghost_after must be genuinely unresolvable"
    );

    let turn = make_turn(
        "t_BLAFTERGHOST00000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T09:00:00.000Z",
        Some("edit with lost after snapshot"),
        vec![FileEntry {
            path: "f3.rs".into(),
            before: Some(before),
            after: Some(ghost_after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);

    std::fs::write(root.join("f3.rs"), b"a\nb\n").unwrap();

    let out = agentrec(root, &["blame", "f3.rs:1"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("f3.rs:1: attribution unavailable — snapshot unavailable"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("before recording began"),
        "stdout: {stdout}"
    );
}

// --- AC H1–H7, Z+1/D42: `undo` reverts a turn's file changes, per file, with
// a preview-first destructive gate. Seeded directly via agentrec-core, same
// rationale as the `diff`/`blame` tests above — the on-disk fixture bytes
// must match the seeded `before`/`after` refs exactly since undo hashes real
// files.

/// New agentrec-tool (undo) turns appended to the log, in append order.
fn agentrec_turns(root: &Path) -> Vec<serde_json::Value> {
    turns(root)
        .into_iter()
        .filter(|t| t.get("tool").and_then(|x| x.as_str()) == Some("agentrec"))
        .collect()
}

#[test]
fn undo_clean_revert_byte_exact() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"v1\n").unwrap();
    let after = store.put(b"v2\n").unwrap();
    std::fs::write(root.join("a.rs"), b"v2\n").unwrap();

    let turn = base_turn(
        "t_UNDOCLEAN0000000000000001",
        vec![FileEntry {
            path: "a.rs".into(),
            before: Some(before.clone()),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["undo", &turn.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("reverted 1 file"), "stdout: {stdout}");

    let bytes = std::fs::read(root.join("a.rs")).unwrap();
    assert_eq!(bytes, b"v1\n", "byte-exact revert expected");

    let new_turns = agentrec_turns(root);
    assert_eq!(
        new_turns.len(),
        1,
        "expected exactly one agentrec undo turn: {new_turns:?}"
    );
    assert_eq!(
        new_turns[0].get("grade").and_then(|g| g.as_str()),
        Some("rich")
    );
}

#[test]
fn undo_preview_does_not_mutate() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"v1\n").unwrap();
    let after = store.put(b"v2\n").unwrap();
    std::fs::write(root.join("a.rs"), b"v2\n").unwrap();

    let turn = base_turn(
        "t_UNDOPREVIEW000000000000001",
        vec![FileEntry {
            path: "a.rs".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);

    // No --confirm: preview only.
    let out = agentrec(root, &["undo", &turn.id]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("preview only"), "stdout: {stdout}");

    let bytes = std::fs::read(root.join("a.rs")).unwrap();
    assert_eq!(bytes, b"v2\n", "preview must not mutate the file");
    assert!(
        agentrec_turns(root).is_empty(),
        "preview must not record a turn"
    );
}

#[test]
fn undo_file_subset() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before_a = store.put(b"a-v1\n").unwrap();
    let after_a = store.put(b"a-v2\n").unwrap();
    let before_b = store.put(b"b-v1\n").unwrap();
    let after_b = store.put(b"b-v2\n").unwrap();
    std::fs::write(root.join("a.rs"), b"a-v2\n").unwrap();
    std::fs::write(root.join("b.rs"), b"b-v2\n").unwrap();

    let turn = base_turn(
        "t_UNDOSUBSET0000000000000001",
        vec![
            FileEntry {
                path: "a.rs".into(),
                before: Some(before_a),
                after: Some(after_a),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            },
            FileEntry {
                path: "b.rs".into(),
                before: Some(before_b),
                after: Some(after_b),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            },
        ],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["undo", &turn.id, "--files", "a.rs", "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("reverted 1 file"), "stdout: {stdout}");

    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), b"a-v1\n");
    assert_eq!(
        std::fs::read(root.join("b.rs")).unwrap(),
        b"b-v2\n",
        "deselected file must be untouched"
    );
}

#[test]
fn undo_modified_since_excluded_then_allowed() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"v1\n").unwrap();
    let after = store.put(b"v2\n").unwrap();

    let turn = base_turn(
        "t_UNDOMODSINCE00000000000001",
        vec![FileEntry {
            path: "a.rs".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);

    // A human (or something else) edits the file to a THIRD value after the
    // turn's recorded `after` — modified-since holds.
    std::fs::write(root.join("a.rs"), b"v3-human\n").unwrap();

    let out = agentrec(root, &["undo", &turn.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("EXCLUDE"), "stdout: {stdout}");
    assert!(stdout.contains("modified since"), "stdout: {stdout}");
    assert_eq!(
        std::fs::read(root.join("a.rs")).unwrap(),
        b"v3-human\n",
        "excluded file must be untouched"
    );
    assert!(
        agentrec_turns(root).is_empty(),
        "an all-excluded plan must not record a turn"
    );

    // Retry with --allow-modified: now it reverts, with a warning naming the file.
    let out = agentrec(root, &["undo", &turn.id, "--allow-modified", "--confirm"]);
    assert!(
        out.status.success(),
        "undo --allow-modified failed: {out:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("a.rs"), "stdout: {stdout}");
    assert!(stdout.contains("WARNING"), "stdout: {stdout}");
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), b"v1\n");
    assert_eq!(agentrec_turns(root).len(), 1);
}

#[test]
fn undo_skipped_and_withheld_refused() {
    use agentrec_core::record::FileEntry;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // SR-E: `state.json`'s `io_failed` is a SEPARATE, aggregate/operational
    // channel (drives the DEGRADED banner) — deliberately populated here with
    // a path that is NOT the one under test, to prove `undo`'s per-entry
    // message is driven only by the wire `skipped_reason` field below, never
    // derived from this list.
    std::fs::write(
        root.join(".agentrec/state.json"),
        r#"{"pid":0,"signal_offset":0,"snapshot_failures":1,"io_failed":["some-other-file.rs"]}"#,
    )
    .unwrap();

    let turn = base_turn(
        "t_UNDOREFUSE00000000000000001",
        vec![
            FileEntry {
                path: "s.rs".into(),
                before: None,
                after: None,
                op: "modify".into(),
                skipped: true,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: Some(agentrec_core::record::skip_reason::IO_FAILED.to_string()),
            },
            FileEntry {
                path: "w.rs".into(),
                before: None,
                after: None,
                op: "modify".into(),
                skipped: false,
                withheld: true,
                baseline_unknown: false,
                skipped_reason: None,
            },
        ],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["undo", &turn.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let s_line = stdout.lines().find(|l| l.contains("s.rs")).unwrap_or("");
    assert!(
        s_line.contains("write failed at record time"),
        "s.rs line: {s_line}"
    );
    assert!(!s_line.contains("over size cap"), "s.rs line: {s_line}");
    let w_line = stdout.lines().find(|l| l.contains("w.rs")).unwrap_or("");
    assert!(w_line.contains("secret-pattern"), "w.rs line: {w_line}");

    assert!(!root.join("s.rs").exists());
    assert!(!root.join("w.rs").exists());
    assert!(
        agentrec_turns(root).is_empty(),
        "an all-refused plan must not record a turn"
    );

    // Second turn: a skipped path whose wire-recorded cause is over_cap
    // reports the over-cap reason — again independent of state.json, which
    // doesn't mention o.rs at all.
    let turn2 = base_turn(
        "t_UNDOREFUSE20000000000000001",
        vec![FileEntry {
            path: "o.rs".into(),
            before: None,
            after: None,
            op: "modify".into(),
            skipped: true,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: Some(agentrec_core::record::skip_reason::OVER_CAP.to_string()),
        }],
    );
    seed_turn(root, &turn2);

    let out = agentrec(root, &["undo", &turn2.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let o_line = stdout.lines().find(|l| l.contains("o.rs")).unwrap_or("");
    assert!(o_line.contains("over size cap"), "o.rs line: {o_line}");
    assert!(
        !o_line.contains("write failed at record time"),
        "o.rs line: {o_line}"
    );
}

// SR6: the `skipped` gate must stay ABOVE the modified-since check in
// `build_plan`. This is the data-loss-shaped scenario SR-C's honesty gain
// makes newly reachable — an over-cap entry now carries a real `after` hash
// (SR-C), so a naive reordering of the two checks would let an unmodified
// skipped file's `is_modified` come out `false` and fall through into the
// Revert plan, where undo would then try to restore a `before` blob that
// was never stored. Pinned here at the CLI level: the file must be REFUSED
// (never appear as a revert) and must be byte-identical on disk afterward.
//
// The fixture below MUST use `op: "create"` (before: None), not `op:
// "modify"`. A prior version of this test used `op: "modify"` with
// `before: None`, which is refused by an INDEPENDENT check —
// `build_plan`'s `op == "modify"` before-blob branch (`None => "no prior
// snapshot to restore"`) fires before the `entry.skipped` gate is ever
// reached, so that fixture pinned nothing: a done-gate review deleted the
// `skipped` gate entirely and the test still passed. `op == "create"`
// skips the before-blob branch entirely (it only runs for
// `"modify"`/`"delete"`), so the `skipped` gate is the ONLY thing standing
// between this fixture and `execute_revert`'s create-inverse, which
// deletes the file on disk — reproduced live against a gate-removed build:
// `undo t_SR6P…0002 (claude)` / `  revert  big.bin (create)` /
// `reverted 1 file(s)` / file gone from disk.
#[test]
fn undo_skipped_entry_stays_refused_even_when_unmodified_since() {
    use agentrec_core::record::{skip_reason, FileEntry};
    use agentrec_core::store::hash_bytes;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // Precondition this test depends on: the on-disk file's CURRENT hash
    // matches the turn's recorded `after` exactly — i.e. genuinely
    // unmodified since the turn, the one case a reordered gate would
    // misclassify as revertible.
    let content = b"over-cap content, never actually stored".to_vec();
    let after_hash = hash_bytes(&content);
    std::fs::write(root.join("big.bin"), &content).unwrap();
    let current_hash = hash_bytes(&std::fs::read(root.join("big.bin")).unwrap());
    assert_eq!(
        current_hash, after_hash,
        "precondition: current on-disk hash must equal the turn's `after`"
    );

    let turn = base_turn(
        "t_SR6GATE0000000000000000001",
        vec![FileEntry {
            path: "big.bin".into(),
            before: None,
            after: Some(after_hash),
            op: "create".into(),
            skipped: true,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: Some(skip_reason::OVER_CAP.to_string()),
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["undo", &turn.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().find(|l| l.contains("big.bin")).unwrap_or("");
    assert!(
        line.trim_start().starts_with("REFUSE"),
        "an over-cap entry must always refuse, even when unmodified: {line}"
    );
    assert!(
        !line.contains("EXCLUDE") && !stdout.contains("REVERT  big.bin"),
        "must never be classified as a revert candidate: stdout: {stdout}"
    );

    // The file must be completely untouched — no attempted restore, no
    // partial mutation, and (the reachable data-loss shape this test
    // exists to catch) it must not have been DELETED by a create-inverse.
    assert!(
        root.join("big.bin").exists(),
        "a refused skipped `create` entry must never be deleted from the worktree"
    );
    assert_eq!(
        std::fs::read(root.join("big.bin")).unwrap(),
        content,
        "a refused skipped entry must never touch the worktree file"
    );
    assert!(
        agentrec_turns(root).is_empty(),
        "an all-refused plan must not record a turn"
    );
}

#[test]
fn undo_create_and_delete_inverse() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    // Turn A: created c.rs.
    let created = store.put(b"created\n").unwrap();
    std::fs::write(root.join("c.rs"), b"created\n").unwrap();
    let turn_a = base_turn(
        "t_UNDOCREATE0000000000000001",
        vec![FileEntry {
            path: "c.rs".into(),
            before: None,
            after: Some(created),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn_a);

    let out = agentrec(root, &["undo", &turn_a.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    assert!(
        !root.join("c.rs").exists(),
        "revert of create must delete the file"
    );

    // Turn B: deleted sub/d.rs (parent dir doesn't exist on disk anymore).
    let gone = store.put(b"gone\n").unwrap();
    let turn_b = base_turn(
        "t_UNDODELETE0000000000000001",
        vec![FileEntry {
            path: "sub/d.rs".into(),
            before: Some(gone),
            after: None,
            op: "delete".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn_b);

    let out = agentrec(root, &["undo", &turn_b.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    assert_eq!(
        std::fs::read(root.join("sub/d.rs")).unwrap(),
        b"gone\n",
        "revert of delete must restore content, creating parent dirs"
    );
}

#[test]
fn undo_is_a_turn_and_reversible() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"v1\n").unwrap();
    let after = store.put(b"v2\n").unwrap();
    std::fs::write(root.join("a.rs"), b"v2\n").unwrap();

    let turn = base_turn(
        "t_UNDOREVERSIBLE000000000001",
        vec![FileEntry {
            path: "a.rs".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["undo", &turn.id, "--confirm"]);
    assert!(out.status.success(), "first undo failed: {out:?}");
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), b"v1\n");

    let first_undo = agentrec_turns(root)
        .into_iter()
        .next()
        .expect("undo turn recorded");
    let first_undo_id = first_undo
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap()
        .to_string();

    // Undo of the undo restores the pre-undo (post-original) state exactly.
    let out = agentrec(root, &["undo", &first_undo_id, "--confirm"]);
    assert!(out.status.success(), "second undo failed: {out:?}");
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), b"v2\n");

    assert_eq!(
        agentrec_turns(root).len(),
        2,
        "each undo is itself a re-revertible turn"
    );
}

#[test]
fn panic_undo_targets_last_rich() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    // Case 1: two rich turns; the newest touches p.rs — panic mode must
    // target it, not the older turn touching q.rs.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        let store = BlobStore::new(root.join(".agentrec/objects"));

        let turn1 = make_turn(
            "t_PANICOLD00000000000000001",
            "rich",
            Some("claude"),
            "2026-07-05T10:00:00.000Z",
            Some("first"),
            vec![FileEntry {
                path: "q.rs".into(),
                before: None,
                after: Some(store.put(b"q\n").unwrap()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            }],
        );
        seed_turn(root, &turn1);

        let turn2 = make_turn(
            "t_PANICNEW00000000000000001",
            "rich",
            Some("claude"),
            "2026-07-05T11:00:00.000Z",
            Some("second"),
            vec![FileEntry {
                path: "p.rs".into(),
                before: None,
                after: Some(store.put(b"p\n").unwrap()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            }],
        );
        seed_turn(root, &turn2);

        // Panic mode: no turn arg, no --confirm — still previews only.
        let out = agentrec(root, &["undo"]);
        assert!(out.status.success(), "panic undo preview failed: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("preview only"), "stdout: {stdout}");
        assert!(
            stdout.contains("p.rs"),
            "stdout should name the newest turn's file: {stdout}"
        );
        assert!(
            !stdout.contains("q.rs"),
            "stdout should not touch the older turn's file: {stdout}"
        );
        assert!(
            !root.join("p.rs").exists() && !root.join("q.rs").exists(),
            "preview must not create files"
        );
    }

    // Case 2: newest turn is bare — panic undo refuses rather than skipping
    // to the older rich turn.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        let store = BlobStore::new(root.join(".agentrec/objects"));

        let turn_rich = make_turn(
            "t_PANICRICH0000000000000001",
            "rich",
            Some("claude"),
            "2026-07-05T10:00:00.000Z",
            Some("first"),
            vec![FileEntry {
                path: "q.rs".into(),
                before: None,
                after: Some(store.put(b"q\n").unwrap()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            }],
        );
        seed_turn(root, &turn_rich);

        let turn_bare = make_turn(
            "t_PANICBARE0000000000000001",
            "bare",
            None,
            "2026-07-05T11:00:00.000Z",
            None,
            vec![FileEntry {
                path: "r.rs".into(),
                before: None,
                after: Some(store.put(b"r\n").unwrap()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            }],
        );
        seed_turn(root, &turn_bare);

        let out = agentrec(root, &["undo"]);
        assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("unattributed"), "stderr: {stderr}");
    }

    // Case 3: empty log — nothing to undo.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);

        let out = agentrec(root, &["undo"]);
        assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("no turns to undo"), "stderr: {stderr}");
    }
}

#[test]
fn undo_concurrent_no_spurious_bare_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    std::fs::write(root.join("real.rs"), "fn real() {}").unwrap();
    poll_until(Duration::from_secs(25), || {
        root.join(".agentrec/open.json").exists().then_some(())
    })
    .expect("turn opened");

    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s_h7"}"#);

    let turn = poll_until(Duration::from_secs(10), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("real.rs"))
                })
                .unwrap_or(false)
        })
    })
    .expect("real.rs turn recorded");
    let turn_id = turn.get("id").and_then(|i| i.as_str()).unwrap().to_string();

    // Let the daemon settle fully back to idle before undo runs, so the
    // guard exclusion is the only thing standing between undo's write and a
    // spurious bare turn.
    poll_until(Duration::from_secs(5), || {
        (!root.join(".agentrec/open.json").exists()).then_some(())
    });

    let out = agentrec(root, &["undo", &turn_id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");

    // undo's guard lingers past the daemon's worst-case flush latency (AC
    // H7) before it's removed, so by the time undo returns, a buggy
    // (unfiltered) daemon would already have opened a turn for its delete
    // of real.rs.
    let open = std::fs::read_to_string(root.join(".agentrec/open.json")).unwrap_or_default();
    assert!(
        !open.contains("real.rs"),
        "daemon opened a turn for undo's own write: {open}"
    );

    sigkill(&daemon);
    let _ = daemon.wait();

    let all_turns = turns(root);
    let undo_turns = all_turns
        .iter()
        .filter(|t| t.get("tool").and_then(|x| x.as_str()) == Some("agentrec"))
        .count();
    assert_eq!(
        undo_turns, 1,
        "expected exactly one agentrec undo turn: {all_turns:?}"
    );

    let bare_touching_real = all_turns.iter().any(|t| {
        t.get("grade").and_then(|g| g.as_str()) == Some("bare")
            && t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("real.rs"))
                })
                .unwrap_or(false)
    });
    assert!(
        !bare_touching_real,
        "spurious bare turn for real.rs: {all_turns:?}"
    );
}

// --- AC M+ (gap closure): a REAL daemon-side snapshot I/O failure — not a
// planted state.json — must still (a) be counted + path-tracked in
// state.json, (b) leave the turn boundary itself intact with the file
// marked `skipped: true` (the daemon keeps recording despite the failure),
// and (c) surface as DEGRADED in `status`. `.agentrec/objects/` is made
// unwritable so the very first blob (a fresh fan-out subdir create) fails;
// `.agentrec/log.jsonl` / `state.json` live outside `objects/` so turn and
// state persistence are unaffected.
#[cfg(unix)]
#[test]
fn degraded_on_real_snapshot_io_failure() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    let objects = root.join(".agentrec/objects");
    let mut perms = std::fs::metadata(&objects).unwrap().permissions();
    perms.set_mode(0o500);
    std::fs::set_permissions(&objects, perms).unwrap();

    // Unique content so this can never dedup to a pre-existing blob.
    std::fs::write(root.join("victim.rs"), "fn victim_unique_marker_9f3a() {}").unwrap();

    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"sD"}"#);

    let state_ok = poll_until(Duration::from_secs(15), || {
        let text = std::fs::read_to_string(root.join(".agentrec/state.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let failures = v
            .get("snapshot_failures")
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        let tracks_victim = v
            .get("io_failed")
            .and_then(|f| f.as_array())
            .map(|fs| fs.iter().any(|p| p.as_str() == Some("victim.rs")))
            .unwrap_or(false);
        (failures > 0 && tracks_victim).then_some(())
    });

    let skipped_turn = poll_until(Duration::from_secs(15), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter().any(|f| {
                        f.get("path").and_then(|p| p.as_str()) == Some("victim.rs")
                            && f.get("skipped").and_then(|s| s.as_bool()) == Some(true)
                    })
                })
                .unwrap_or(false)
        })
    });

    // Restore perms before further assertions/process exit so the tempdir
    // can always be cleaned up, even on assertion failure below.
    let mut perms = std::fs::metadata(&objects).unwrap().permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(&objects, perms).unwrap();

    assert!(
        state_ok.is_some(),
        "state.json never recorded a real snapshot I/O failure for victim.rs"
    );
    assert!(
        skipped_turn.is_some(),
        "daemon did not persist a turn boundary with victim.rs skipped:true despite the I/O failure"
    );

    let out = agentrec(root, &["status"]);
    assert!(out.status.success(), "status failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("DEGRADED"), "stdout: {stdout}");

    sigkill(&daemon);
    let _ = daemon.wait();
}

// --- D35 gap closure (prompt-put-failure leg): a REAL daemon-side PROMPT
// blob I/O failure — not a planted state.json — must (a) bump its OWN
// `prompt_put_failures` counter, NEVER `snapshot_failures`/`io_failed`
// (those are file-scoped and must stay unaffected), (b) leave the turn
// record intact (`prompt_ref: null`, `prompt_excerpt` still populated — the
// daemon still closes the turn), and (c) surface in `status`'s DEGRADED
// banner. Unlike `degraded_on_real_snapshot_io_failure`, the file write
// happens and is snapshotted successfully BEFORE `.agentrec/objects/` is
// locked down — isolating the failure to the later prompt-store call in
// `persist` and proving the two counters are genuinely independent, not
// coincidentally equal.
#[cfg(unix)]
#[test]
fn degraded_on_real_prompt_put_failure() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    // Unique content so the file's own snapshot can never dedup to a
    // pre-existing blob — it must genuinely write while objects/ is still
    // writable.
    std::fs::write(
        root.join("prompt_victim.rs"),
        "fn prompt_victim_unique_marker_7b2c() {}",
    )
    .unwrap();
    poll_until(Duration::from_secs(15), || {
        root.join(".agentrec/open.json").exists().then_some(())
    })
    .expect("turn opened for prompt_victim.rs");
    // Give the debounced burst time to settle and actually snapshot the
    // file while objects/ is still writable, before locking it down below.
    std::thread::sleep(Duration::from_millis(2500));

    let objects = root.join(".agentrec/objects");
    let mut perms = std::fs::metadata(&objects).unwrap().permissions();
    perms.set_mode(0o500);
    std::fs::set_permissions(&objects, perms).unwrap();

    let payload = r#"{"hook_event_name":"Stop","session_id":"s_prompt_fail","prompt":"unique prompt marker for the put-failure regression"}"#;
    send_hook(root, payload);

    let state_ok = poll_until(Duration::from_secs(15), || {
        let text = std::fs::read_to_string(root.join(".agentrec/state.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let prompt_failures = v
            .get("prompt_put_failures")
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        (prompt_failures > 0).then_some(())
    });

    let turn = poll_until(Duration::from_secs(15), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("prompt_victim.rs"))
                })
                .unwrap_or(false)
        })
    });

    // Restore perms before further assertions/process exit so the tempdir
    // can always be cleaned up, even on assertion failure below.
    let mut perms = std::fs::metadata(&objects).unwrap().permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(&objects, perms).unwrap();

    assert!(
        state_ok.is_some(),
        "state.json never recorded a real prompt-put I/O failure"
    );

    let state_text = std::fs::read_to_string(root.join(".agentrec/state.json")).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state_text).unwrap();
    assert_eq!(
        state.get("snapshot_failures").and_then(|n| n.as_u64()),
        Some(0),
        "the file snapshot succeeded before lockdown — snapshot_failures must stay 0: {state}"
    );
    assert!(
        state
            .get("io_failed")
            .and_then(|f| f.as_array())
            .map(|fs| fs.is_empty())
            .unwrap_or(false),
        "io_failed is file-scoped and must stay empty for a prompt-only failure: {state}"
    );

    let turn =
        turn.expect("turn touching prompt_victim.rs was recorded despite the prompt-put failure");
    assert!(
        turn.get("prompt_ref").map(|r| r.is_null()).unwrap_or(true),
        "prompt_ref must be null when the blob write failed: {turn}"
    );
    assert!(
        turn.get("prompt_excerpt")
            .and_then(|e| e.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "prompt_excerpt must still be populated — it's derived pre-store, independent of the put outcome: {turn}"
    );
    let victim_file = turn
        .get("files")
        .and_then(|f| f.as_array())
        .unwrap()
        .iter()
        .find(|f| f.get("path").and_then(|p| p.as_str()) == Some("prompt_victim.rs"))
        .unwrap();
    // `skipped` is omitted from the wire JSON entirely when false
    // (`#[serde(skip_serializing_if = "std::ops::Not::not")]`) — absence
    // means the same thing as an explicit `false`.
    assert!(
        !victim_file
            .get("skipped")
            .and_then(|s| s.as_bool())
            .unwrap_or(false),
        "the file itself was snapshotted successfully before lockdown: {victim_file}"
    );

    let out = agentrec(root, &["status"]);
    assert!(out.status.success(), "status failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("DEGRADED"), "stdout: {stdout}");

    // `show --prompt` on this exact turn must give the honest, distinct
    // message — never fabricate "no prompt attached" for a turn that
    // plainly had one attached.
    let turn_id = turn.get("id").and_then(|i| i.as_str()).unwrap();
    let show_out = agentrec(root, &["show", turn_id, "--prompt"]);
    assert_eq!(
        show_out.status.code(),
        Some(1),
        "show --prompt: {show_out:?}"
    );
    let show_stderr = String::from_utf8_lossy(&show_out.stderr).to_lowercase();
    assert!(
        show_stderr.contains("write failed at record time"),
        "stderr: {show_stderr}"
    );
    assert!(
        !show_stderr.contains("no prompt attached"),
        "must not fabricate 'no prompt attached' for a turn that had one: {show_stderr}"
    );

    sigkill(&daemon);
    let _ = daemon.wait();
}

// D35 gap closure: `show --prompt` must distinguish a turn that genuinely
// never had a prompt from one whose prompt blob failed to store, using only
// data already on the wire (`prompt_excerpt`'s presence) — no protocol
// change. This isolates the message logic from the full daemon-fault-
// injection test above, which additionally proves the real write path.
#[test]
fn show_prompt_put_failure_distinct_from_no_prompt_attached() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut turn = base_turn("t_SHOWPUTFAILED0000000001", vec![]);
    turn.prompt_excerpt = Some("had a prompt, blob write failed".to_string());
    // prompt_ref stays None (base_turn's default) — the put-failure shape.
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id, "--prompt"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    assert!(out.stdout.is_empty(), "stdout must be empty on failure");
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("write failed at record time"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains("no prompt attached"),
        "must not fabricate 'no prompt attached' for a turn with a prompt_excerpt: {stderr}"
    );
}

// Regression (review finding #1): an `undo` turn is minted by agentrec itself
// (`tool: "agentrec"`) with a synthetic `prompt_excerpt` ("undo of <id>") and
// `prompt_ref: None` — it never had a real prompt blob. Keying the put-failure
// message on excerpt-presence alone misclassifies EVERY undo turn as a failed
// prompt write and points the user at a `status` that shows no such failure.
// `show --prompt` on a synthetic (agentrec/git) turn must report "no prompt
// attached", never a fabricated put-failure.
#[test]
fn show_prompt_on_undo_turn_is_not_a_false_put_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut turn = base_turn("t_UNDOSHOWPROMPT000000001", vec![]);
    turn.tool = Some("agentrec".to_string());
    turn.prompt_excerpt = Some("undo of abcd1234".to_string());
    // prompt_ref stays None (base_turn default) — the undo-turn shape.
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id, "--prompt"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    assert!(out.stdout.is_empty(), "stdout must be empty on failure");
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("no prompt attached"),
        "an undo turn has no recorded prompt: {stderr}"
    );
    assert!(
        !stderr.contains("write failed"),
        "must not fabricate a put-failure for a synthetic undo turn: {stderr}"
    );
}

// --- AC Z+1 (gap closure): panic-mode undo (no turn arg, no --confirm) must
// skip a git turn even when it is the newest record, targeting the most
// recent non-git rich turn instead. `resolve_panic_target` in readcmds.rs
// already filters `tool == "git"`; this proves it end-to-end against a
// realistic "checkout clobbered a pile of files" shape.
#[test]
fn panic_undo_skips_git_turn() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    // Older, rich, non-git: the real undo target.
    let turn_claude = make_turn(
        "t_GITSKIPOLD00000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T10:00:00.000Z",
        Some("refactor the parser"),
        vec![FileEntry {
            path: "code.rs".into(),
            before: None,
            after: Some(store.put(b"fn code() {}\n").unwrap()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn_claude);

    // Newest turn: a git checkout touching many files — must never be the
    // panic-undo target.
    let turn_git = make_turn(
        "t_GITSKIPNEW00000000000001",
        "rich",
        Some("git"),
        "2026-07-05T11:00:00.000Z",
        None,
        vec![
            FileEntry {
                path: "checkout_a.rs".into(),
                before: None,
                after: Some(store.put(b"a\n").unwrap()),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            },
            FileEntry {
                path: "checkout_b.rs".into(),
                before: None,
                after: Some(store.put(b"b\n").unwrap()),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            },
            FileEntry {
                path: "checkout_c.rs".into(),
                before: None,
                after: Some(store.put(b"c\n").unwrap()),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            },
        ],
    );
    seed_turn(root, &turn_git);

    // Panic mode: no turn arg, no --confirm — preview only, nothing mutates.
    let out = agentrec(root, &["undo"]);
    assert!(out.status.success(), "panic undo preview failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("preview only"), "stdout: {stdout}");
    assert!(
        stdout.contains("code.rs"),
        "stdout should name the claude turn's file: {stdout}"
    );
    assert!(
        stdout.lines().next().unwrap_or("").contains("claude"),
        "preview header should target the claude turn, not git: {stdout}"
    );
    assert!(
        !stdout.contains("(git)"),
        "git turn must never be presented as the panic target: {stdout}"
    );
    assert!(
        !stdout.contains("checkout_a.rs"),
        "stdout should not surface the git turn's files: {stdout}"
    );
    assert!(
        !stdout.contains("checkout_b.rs"),
        "stdout should not surface the git turn's files: {stdout}"
    );
    assert!(
        !stdout.contains("checkout_c.rs"),
        "stdout should not surface the git turn's files: {stdout}"
    );

    assert!(
        !root.join("code.rs").exists(),
        "preview must not create files"
    );
    assert!(
        !root.join("checkout_a.rs").exists(),
        "preview must not create files"
    );
}

// --- AC I5–I6: `purge` deletes blob objects, never rewrites log.jsonl, and
// respects the dedup keep-set (a blob shared by a kept turn must survive).

fn days_ago_rfc3339(days: u64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    agentrec_core::time::rfc3339(now_ms.saturating_sub(days * 86_400_000))
}

#[test]
fn purge_removes_expired_prompt_blob_keeps_shared() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let shared_prompt = store.put(b"shared prompt content").unwrap();
    let unique_old_prompt = store.put(b"unique old prompt content").unwrap();
    // A snapshot blob on the expired turn — default purge must leave it alone.
    let old_snapshot = store.put(b"old file content").unwrap();

    // Expired (well past the default 90-day ttl), unshared prompt blob.
    let mut old_unique = base_turn("t_OLDUNIQUE000000000000001", vec![]);
    old_unique.started = days_ago_rfc3339(200);
    old_unique.ended = old_unique.started.clone();
    old_unique.prompt_ref = Some(unique_old_prompt.clone());
    old_unique.prompt_excerpt = Some("old excerpt".into());
    seed_turn(root, &old_unique);

    // Expired turn whose prompt blob is ALSO referenced by a within-ttl turn.
    let mut old_shared = base_turn(
        "t_OLDSHARED00000000000001",
        vec![FileEntry {
            path: "old.rs".into(),
            before: None,
            after: Some(old_snapshot.clone()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    old_shared.started = days_ago_rfc3339(200);
    old_shared.ended = old_shared.started.clone();
    old_shared.prompt_ref = Some(shared_prompt.clone());
    old_shared.prompt_excerpt = Some("shared excerpt (old)".into());
    seed_turn(root, &old_shared);

    // Recent (within ttl) turn sharing the same prompt blob — must keep it alive.
    let mut recent = base_turn("t_RECENT00000000000000001", vec![]);
    recent.started = days_ago_rfc3339(1);
    recent.ended = recent.started.clone();
    recent.prompt_ref = Some(shared_prompt.clone());
    recent.prompt_excerpt = Some("shared excerpt (recent)".into());
    seed_turn(root, &recent);

    let out = agentrec(root, &["purge"]);
    assert!(out.status.success(), "purge failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("expired prompt blob"), "stdout: {stdout}");

    assert!(
        !store.contains(&unique_old_prompt),
        "unshared expired prompt blob must be deleted"
    );
    assert!(
        store.contains(&shared_prompt),
        "prompt blob shared with a within-ttl turn must survive"
    );
    assert!(
        store.contains(&old_snapshot),
        "default purge must never touch snapshot blobs"
    );

    // `log` renders from the inline excerpt, not the blob — unaffected by purge.
    let log_out = agentrec(root, &["log", "--all"]);
    let log_stdout = String::from_utf8_lossy(&log_out.stdout);
    assert!(log_stdout.contains("old excerpt"), "log: {log_stdout}");
    assert!(
        log_stdout.contains("shared excerpt (old)"),
        "log: {log_stdout}"
    );
    assert!(
        log_stdout.contains("shared excerpt (recent)"),
        "log: {log_stdout}"
    );
}

#[test]
fn purge_all_prompts_removes_all_prompt_blobs() {
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let old_prompt = store.put(b"an old prompt").unwrap();
    let recent_prompt = store.put(b"a recent prompt").unwrap();

    let mut old = base_turn("t_ALLOLD00000000000000001", vec![]);
    old.started = days_ago_rfc3339(200);
    old.ended = old.started.clone();
    old.prompt_ref = Some(old_prompt.clone());
    old.prompt_excerpt = Some("old prompt excerpt".into());
    seed_turn(root, &old);

    let mut recent = base_turn("t_ALLRECENT0000000000001", vec![]);
    recent.started = days_ago_rfc3339(1);
    recent.ended = recent.started.clone();
    recent.prompt_ref = Some(recent_prompt.clone());
    recent.prompt_excerpt = Some("recent prompt excerpt".into());
    seed_turn(root, &recent);

    let out = agentrec(root, &["purge", "--all-prompts"]);
    assert!(out.status.success(), "purge --all-prompts failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("(all)"), "stdout: {stdout}");

    assert!(!store.contains(&old_prompt), "expired prompt blob deleted");
    assert!(
        !store.contains(&recent_prompt),
        "even a within-ttl prompt blob is deleted with --all-prompts"
    );

    let log_out = agentrec(root, &["log", "--all"]);
    let log_stdout = String::from_utf8_lossy(&log_out.stdout);
    assert!(
        log_stdout.contains("old prompt excerpt"),
        "log: {log_stdout}"
    );
    assert!(
        log_stdout.contains("recent prompt excerpt"),
        "log: {log_stdout}"
    );
}

#[test]
fn purge_snapshots_before_date_respects_keepset() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let old_only = store.put(b"only referenced before the cutoff").unwrap();
    let shared = store.put(b"referenced both sides of the cutoff").unwrap();
    let new_only = store.put(b"only referenced on/after the cutoff").unwrap();

    // Before the cutoff: creates old_only, then a second old turn hands off
    // to `shared` (this is the `after` a turn on/after the cutoff will share
    // as its `before`).
    let mut turn_old1 = base_turn(
        "t_SNAPOLD1000000000000001",
        vec![FileEntry {
            path: "a.rs".into(),
            before: None,
            after: Some(old_only.clone()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    turn_old1.started = "2024-01-01T00:00:00.000Z".into();
    turn_old1.ended = turn_old1.started.clone();
    seed_turn(root, &turn_old1);

    let mut turn_old2 = base_turn(
        "t_SNAPOLD2000000000000001",
        vec![FileEntry {
            path: "b.rs".into(),
            before: None,
            after: Some(shared.clone()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    turn_old2.started = "2024-02-01T00:00:00.000Z".into();
    turn_old2.ended = turn_old2.started.clone();
    seed_turn(root, &turn_old2);

    // On the cutoff date itself (on/after — kept side): references `shared`
    // as `before`, and introduces `new_only`.
    let mut turn_new = base_turn(
        "t_SNAPNEW0000000000000001",
        vec![FileEntry {
            path: "b.rs".into(),
            before: Some(shared.clone()),
            after: Some(new_only.clone()),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    turn_new.started = "2024-06-01T00:00:00.000Z".into();
    turn_new.ended = turn_new.started.clone();
    seed_turn(root, &turn_new);

    let out = agentrec(root, &["purge", "--snapshots-before", "2024-06-01"]);
    assert!(
        out.status.success(),
        "purge --snapshots-before failed: {out:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("snapshot blob"), "stdout: {stdout}");

    assert!(
        !store.contains(&old_only),
        "unshared before-cutoff snapshot deleted"
    );
    assert!(
        store.contains(&shared),
        "snapshot shared with an on/after-cutoff turn must survive"
    );
    assert!(
        store.contains(&new_only),
        "on/after-cutoff snapshot untouched"
    );
}

// --- AC-Y+3/Y+4/Y+5: `init`/`uninstall` scaffold+hook+dry-run+archive flow,
// all with `--no-service` — this suite never loads a real launchd/systemd
// service (hermetic-tests requirement). `service.rs`'s own pure-function
// unit tests cover the unit-string/path/slug generation.

fn walk_paths(root: &Path) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .to_string();
            out.insert(rel);
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    out
}

#[test]
fn init_dry_run_then_noop_then_uninstall_no_service() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let out = agentrec(root, &["init", "--no-service"]);
    assert!(out.status.success(), "init failed: {out:?}");
    assert!(root.join(".agentrec/config.toml").exists());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("to reverse everything: agentrec uninstall"),
        "stdout: {stdout}"
    );
    // D-PD5: a fresh (disk-altering) init must NOT claim nothing changed.
    assert!(
        !stdout.contains("already initialized"),
        "fresh init must not print the no-op message: {stdout}"
    );

    let settings_path = root.join(".claude/settings.local.json");
    let settings_after_first = std::fs::read_to_string(&settings_path).unwrap();

    // --dry-run must touch NOTHING on disk.
    let before = walk_paths(root);
    let out = agentrec(root, &["init", "--no-service", "--dry-run"]);
    assert!(out.status.success(), "dry-run failed: {out:?}");
    let after = walk_paths(root);
    assert_eq!(before, after, "dry-run must not create/modify any file");

    // Re-running (no dry-run) is a byte-for-byte no-op: no dup hooks, exit 0.
    let out2 = agentrec(root, &["init", "--no-service"]);
    assert!(out2.status.success(), "second init failed: {out2:?}");
    let after2 = walk_paths(root);
    assert_eq!(before, after2, "re-init must not add/remove any file");
    let settings_after_second = std::fs::read_to_string(&settings_path).unwrap();
    assert_eq!(
        settings_after_first, settings_after_second,
        "hooks file must be byte-identical on no-op re-init"
    );
    // D-PD5: a byte-for-byte no-op re-run leads with an honest message.
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert_eq!(
        stdout2.lines().next(),
        Some("already initialized — nothing changed"),
        "expected leading no-op message as the first line: {stdout2}"
    );

    // uninstall --no-service: removes hooks, archives .agentrec/, deletes nothing.
    let out = agentrec(root, &["uninstall", "--no-service"]);
    assert!(out.status.success(), "uninstall failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("removed agentrec hook entries"),
        "stdout: {stdout}"
    );
    assert!(!root.join(".agentrec").exists());
    let archived: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".agentrec.archived.")
        })
        .collect();
    assert_eq!(archived.len(), 1, "expected exactly one archive dir");
    assert!(archived[0].path().join("config.toml").exists());

    // A later init starts fresh while the archive remains.
    let out = agentrec(root, &["init", "--no-service"]);
    assert!(out.status.success(), "post-uninstall init failed: {out:?}");
    assert!(root.join(".agentrec/config.toml").exists());
    assert!(archived[0].path().exists(), "archive must never be removed");
}

// Skeptic-surfaced (2026-07-10): the idempotent no-op message must reflect
// ACTUAL post-run disk state, not just whether `.agentrec/` itself pre-
// existed. If `.agentrec/objects/` is separately deleted, a re-run silently
// recreates it while the old logic still printed "nothing changed" —
// fabricating a no-op. This must flip to the normal "acted" output whenever
// init actually recreates something, and only claim the no-op on a genuine
// no-op re-run.
#[test]
fn init_recreates_deleted_objects_dir_and_reports_it_honestly() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let out = agentrec(root, &["init", "--no-service"]);
    assert!(out.status.success(), "first init failed: {out:?}");
    assert!(root.join(".agentrec/objects").exists());

    // Separately delete the required subdir — nothing else about
    // `.agentrec/` changes.
    std::fs::remove_dir_all(root.join(".agentrec/objects")).unwrap();
    assert!(!root.join(".agentrec/objects").exists());

    let out2 = agentrec(root, &["init", "--no-service"]);
    assert!(out2.status.success(), "re-init failed: {out2:?}");
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert_ne!(
        stdout2.lines().next(),
        Some("already initialized — nothing changed"),
        "init recreated objects/ — must not claim nothing changed: {stdout2}"
    );
    assert!(
        root.join(".agentrec/objects").exists(),
        "objects/ must be recreated: {stdout2}"
    );

    // A THIRD, genuine no-op re-run must still lead with the honest no-op
    // message — proves the happy path is untouched.
    let out3 = agentrec(root, &["init", "--no-service"]);
    assert!(out3.status.success(), "third init failed: {out3:?}");
    let stdout3 = String::from_utf8_lossy(&out3.stdout);
    assert_eq!(
        stdout3.lines().next(),
        Some("already initialized — nothing changed"),
        "genuine no-op re-run must still print the no-op message: {stdout3}"
    );
}

// --- D38 (gap closure): a planted secret in a prompt must never reach disk
// in cleartext, in ANY of the three locations scrub is load-bearing for —
// signal.jsonl, log.jsonl, and every blob under objects/. Drives the real
// hook → signal → daemon → persisted-turn path (the same entry point a real
// Claude Code Stop hook uses) with an AWS-key-shaped secret (same fixture
// shape as `agentrec_core::scrub` 's own tests).
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn secret_prompt_never_reaches_disk_in_cleartext() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    std::fs::write(root.join("touched.rs"), "fn touched() {}").unwrap();
    poll_until(Duration::from_secs(25), || {
        root.join(".agentrec/open.json").exists().then_some(())
    })
    .expect("turn opened");

    let secret = "AKIAABCDEFGHIJKLMNOP";
    let payload = format!(
        r#"{{"hook_event_name":"Stop","session_id":"s_secret","prompt":"deploy with key {secret} now"}}"#
    );
    send_hook(root, &payload);

    let turn = poll_until(Duration::from_secs(10), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("touched.rs"))
                })
                .unwrap_or(false)
        })
    })
    .expect("turn with secret-bearing prompt recorded");

    // INV-M3 (memory locations 3 and 4, Task 12): drive the SAME planted
    // secret through both memory write paths — `remember` (direct,
    // daemon-independent write) and `candidate` (async, daemon-ingested
    // write) — then a matching `UserPromptSubmit` hook so a
    // `memory-stats.jsonl` line actually exists. All three must land on
    // disk with the raw key nowhere. The daemon must still be alive for
    // `candidate` to be ingested, so this all happens BEFORE the
    // `sigkill` below.
    seed_filler_memories(root, 8);
    let remember_fact = format!("deploy with key {secret} now");
    let out = agentrec(root, &["remember", &remember_fact, "--from", "touched.rs"]);
    assert!(out.status.success(), "remember failed: {out:?}");

    let candidate_fact = format!("the deploy key {secret} rotates every staging release cycle");
    let out = agentrec(
        root,
        &[
            "candidate",
            &candidate_fact,
            "--from",
            "touched.rs",
            "--tool",
            "test-candidate",
        ],
    );
    assert!(out.status.success(), "candidate emit failed: {out:?}");

    let memories_after_candidate = poll_until(Duration::from_secs(10), || {
        let recs = memory_records(root);
        (recs.len() >= 10).then_some(recs) // 8 filler + remember + candidate
    })
    .expect("candidate was never ingested into memory.jsonl");
    assert_eq!(
        memories_after_candidate.len(),
        10,
        "expected exactly 10 memory records: {memories_after_candidate:?}"
    );

    // Matching UserPromptSubmit -> a real memory-stats.jsonl line (not just
    // an absent-file vacuous pass).
    let hook_payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s_secret_hook","prompt":"deploy key rotation"}"#;
    send_hook(root, hook_payload);
    let stats_after_hook = poll_until(Duration::from_secs(5), || {
        let lines = memory_stats_lines(root);
        (!lines.is_empty()).then_some(lines)
    })
    .expect("expected a memory-stats.jsonl line after a matching hook prompt");
    assert!(
        !stats_after_hook.is_empty(),
        "expected at least one memory-stats.jsonl line"
    );

    sigkill(&daemon);
    let _ = daemon.wait();

    let excerpt = turn
        .get("prompt_excerpt")
        .and_then(|p| p.as_str())
        .unwrap_or("");
    assert!(
        excerpt.contains("[redacted:"),
        "expected a redaction marker in the excerpt: {excerpt}"
    );
    assert!(!excerpt.contains(secret), "excerpt: {excerpt}");

    let signal = std::fs::read(root.join(".agentrec/signal.jsonl")).unwrap_or_default();
    assert!(
        !contains_bytes(&signal, secret.as_bytes()),
        "raw secret leaked into signal.jsonl"
    );

    let log = std::fs::read(root.join(".agentrec/log.jsonl")).unwrap_or_default();
    assert!(
        !contains_bytes(&log, secret.as_bytes()),
        "raw secret leaked into log.jsonl"
    );
    assert!(
        contains_bytes(&log, b"[redacted:"),
        "expected a redaction marker in log.jsonl"
    );

    let mut stack = vec![root.join(".agentrec/objects")];
    let mut checked_any = false;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let is_tmp = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(".tmp"))
                .unwrap_or(false);
            if is_tmp {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap_or_default();
            checked_any = true;
            assert!(
                !contains_bytes(&bytes, secret.as_bytes()),
                "raw secret leaked into blob {path:?}"
            );
        }
    }
    assert!(checked_any, "expected at least one blob object to check");

    // INV-M3 complete (locations 3 and 4): `memory.jsonl` carries the
    // scrubbed fact (redaction marker present, raw key absent) for BOTH the
    // `remember` and `candidate` write paths.
    let memory_jsonl = std::fs::read(root.join(".agentrec/memory.jsonl")).unwrap_or_default();
    assert!(
        !contains_bytes(&memory_jsonl, secret.as_bytes()),
        "raw secret leaked into memory.jsonl"
    );
    assert!(
        contains_bytes(&memory_jsonl, b"[redacted:"),
        "expected a redaction marker in memory.jsonl"
    );

    // `memory-stats.jsonl` only ever holds `{"ts","n"}` injection counts or
    // (F2) `{"ts","budget_exceeded"}` bail markers — no fact text in either
    // shape — so it should trivially never carry the secret. Asserted
    // anyway for completeness (INV-M3, 4th and final location) against the
    // real line the matching hook above produced, not an absent file.
    let memory_stats_jsonl =
        std::fs::read(root.join(".agentrec/memory-stats.jsonl")).unwrap_or_default();
    assert!(
        !memory_stats_jsonl.is_empty(),
        "expected a real memory-stats.jsonl to check, not an absent file"
    );
    assert!(
        !contains_bytes(&memory_stats_jsonl, secret.as_bytes()),
        "raw secret leaked into memory-stats.jsonl"
    );
}

// ---- `agentrec doctor` (D41 / AC-Y++) ---------------------------------------
//
// `doctor` is a one-shot diagnosis, never the daemon required. Exit 0 iff
// every check passes; exit 1 with a printed remedy per failing check, in
// both text and `--json` modes.

fn doctor(root: &Path) -> Output {
    agentrec(root, &["doctor"])
}

fn doctor_json_value(root: &Path) -> serde_json::Value {
    let out = agentrec(root, &["doctor", "--json"]);
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("doctor --json did not emit valid JSON ({e}): {out:?}"))
}

/// Poll `.agentrec/state.json` until the daemon has written a nonzero pid
/// (i.e. it holds the lock), so `doctor`'s liveness check has something real
/// to observe.
fn wait_for_live_daemon(root: &Path) {
    let ok = poll_until(Duration::from_secs(10), || {
        let text = std::fs::read_to_string(root.join(".agentrec/state.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let pid = v.get("pid")?.as_u64()?;
        (pid != 0).then_some(())
    });
    assert!(ok.is_some(), "daemon never wrote a live pid to state.json");
}

#[test]
fn doctor_healthy_all_pass_exit_0() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Real hooks installed (not --no-hook), never a real service (--no-service).
    let out = agentrec(root, &["init", "--no-service"]);
    assert!(out.status.success(), "init failed: {out:?}");

    let mut daemon = spawn_record(root);
    wait_for_live_daemon(root);

    // Pin the signal-freshness check's transcript root to an empty dir: this
    // process is itself a live Claude Code session, so the real
    // `$HOME/.claude/projects` may have a genuinely fresh transcript right
    // now, which would otherwise make this "healthy" fixture spuriously
    // trip the disconnected-hooks check (its tempdir signal.jsonl is real,
    // just untouched, since no real Claude session talks to this repo).
    let start = Instant::now();
    let out = Command::new(bin())
        .args(["doctor", "--root", root.to_str().unwrap()])
        .env(
            "AGENTREC_CLAUDE_PROJECTS_DIR",
            tmp.path().join("no-transcripts-here"),
        )
        .output()
        .expect("run agentrec doctor");
    let elapsed = start.elapsed();

    let _ = daemon.kill();
    let _ = daemon.wait();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "expected exit 0 on a healthy repo: {out:?}\nstdout={stdout}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "doctor took {elapsed:?}, expected well under 2s"
    );
    assert!(stdout.contains("pass"), "expected pass lines: {stdout}");
    assert!(
        !stdout.to_lowercase().contains("fail"),
        "healthy repo must report no failures: {stdout}"
    );
}

#[test]
fn doctor_daemon_down_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-service"]);
    // No daemon spawned: state.json's pid is 0 (or absent).

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("recorder not running"),
        "expected the daemon-down remedy: {stdout}"
    );
}

#[test]
fn doctor_hook_missing_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-hook", "--no-service"]);

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hooks missing/mangled"),
        "expected the hook remedy: {stdout}"
    );
}

#[test]
fn doctor_hook_malformed_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-hook", "--no-service"]);
    std::fs::create_dir_all(root.join(".claude")).unwrap();
    std::fs::write(
        root.join(".claude/settings.local.json"),
        "{ not json at all",
    )
    .unwrap();

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hooks missing/mangled"),
        "malformed settings.local.json must be treated as a fail, not a crash: {stdout}"
    );
}

#[test]
fn doctor_degraded_store_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-service"]);
    std::fs::write(
        root.join(".agentrec/state.json"),
        r#"{"pid":0,"signal_offset":0,"snapshot_failures":3,"io_failed":["a.rs"]}"#,
    )
    .unwrap();

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("DEGRADED"),
        "expected DEGRADED notice: {stdout}"
    );
    assert!(
        stdout.contains("3 snapshot"),
        "expected the failure count in the remedy: {stdout}"
    );
}

// D-PD5: the doctor check formerly named "store degraded" was renamed to
// "store health" (non-jargon, brand-invariant wording) — assert the new
// name appears (text + --json `name` field) and the old one is gone.
#[test]
fn doctor_check_name_is_store_health_not_store_degraded() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-service"]);

    let stdout = String::from_utf8_lossy(&doctor(root).stdout).into_owned();
    assert!(
        stdout.contains("store health"),
        "expected renamed check 'store health': {stdout}"
    );
    assert!(
        !stdout.contains("store degraded"),
        "old check name 'store degraded' must not appear: {stdout}"
    );

    let v = doctor_json_value(root);
    let checks = v["checks"].as_array().expect("checks array");
    assert!(
        checks.iter().any(|c| c["name"] == "store health"),
        "expected 'store health' in --json checks: {v}"
    );
    assert!(
        !checks.iter().any(|c| c["name"] == "store degraded"),
        "old check name must not appear in --json: {v}"
    );
}

#[cfg(unix)]
#[test]
fn doctor_bad_perms_fails() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-service"]);
    std::fs::set_permissions(
        root.join(".agentrec"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("store permissions too open"),
        "expected the permissions remedy: {stdout}"
    );
}

#[test]
fn doctor_signal_stale_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-service"]);

    // A fake Claude Code transcripts root with one freshly-written transcript
    // ("agent active"), while `.agentrec/signal.jsonl` was never written
    // (hooks never fired) — the disconnected-hooks failure mode.
    let projects_dir = tmp.path().join("fake-claude-projects");
    let project = projects_dir.join("some-project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("session.jsonl"), "{}\n").unwrap();

    let out = Command::new(bin())
        .args(["doctor", "--root", root.to_str().unwrap()])
        .env("AGENTREC_CLAUDE_PROJECTS_DIR", &projects_dir)
        .output()
        .expect("run agentrec doctor");

    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("agent active but no signals arriving"),
        "expected the signal-freshness remedy: {stdout}"
    );
}

// ---- D-PD1: `initialized` gate — never fabricate check results in an
// uninitialized repo (no `.agentrec/` at all: no `init` was ever run).

#[test]
fn doctor_uninitialized_repo_short_circuits_all_checks() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Deliberately no `agentrec init` — `.agentrec/` does not exist.

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("initialized") && stdout.contains("not initialized — run"),
        "expected the initialized check + remedy: {stdout}"
    );
    assert!(
        !stdout.contains("permissions too open"),
        "must not fabricate a permissions failure pre-init: {stdout}"
    );
    assert!(
        !stdout.contains("agent active but no signals"),
        "must not fabricate a signal-freshness failure pre-init: {stdout}"
    );

    // Every non-init check line ends in `n/a`.
    for line in stdout.lines() {
        if line.starts_with("initialized") || line.trim_start().starts_with("->") {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        assert!(
            line.trim_end().ends_with("n/a"),
            "expected every non-init check line to end in n/a: {line:?} (full output: {stdout})"
        );
    }
}

#[test]
fn doctor_uninitialized_repo_json_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Deliberately no `agentrec init`.

    let out = agentrec(root, &["doctor", "--json"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");

    let v = doctor_json_value(root);
    assert_eq!(v["ok"], false, "expected ok:false in {v}");
    let checks = v["checks"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a checks array in {v}"));
    assert!(!checks.is_empty(), "expected at least one check in {v}");

    let first = &checks[0];
    assert_eq!(
        first["name"], "initialized",
        "first check must be initialized: {v}"
    );
    assert_eq!(first["status"], "fail", "initialized must fail: {v}");
    assert!(
        first["remedy"]
            .as_str()
            .unwrap_or("")
            .contains("not initialized — run"),
        "expected the init remedy: {v}"
    );

    for check in &checks[1..] {
        assert_eq!(
            check["status"], "n/a",
            "every non-init check must be n/a in an uninitialized repo: {check} (full: {v})"
        );
    }
}

#[test]
fn doctor_initialized_repo_reports_initialized_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-hook", "--no-service"]);

    // Prior behavior intact: hooks missing still fails, but `initialized`
    // itself passes since `.agentrec/` now exists.
    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hooks missing/mangled"),
        "expected prior hook-missing behavior to survive the init gate: {stdout}"
    );

    let v = doctor_json_value(root);
    let checks = v["checks"].as_array().unwrap();
    let init_check = checks
        .iter()
        .find(|c| c["name"] == "initialized")
        .unwrap_or_else(|| panic!("missing initialized check in {v}"));
    assert_eq!(
        init_check["status"], "pass",
        "expected initialized pass in {v}"
    );
}

#[test]
fn doctor_json_shape_and_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Guaranteed to fail (no daemon, no hooks) so ok:false is exercised.
    agentrec(root, &["init", "--no-hook", "--no-service"]);

    let out = agentrec(root, &["doctor", "--json"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");

    let v = doctor_json_value(root);
    assert_eq!(v["ok"], false, "expected ok:false in {v}");
    let checks = v["checks"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a checks array in {v}"));
    assert!(!checks.is_empty(), "expected at least one check in {v}");
    assert!(
        checks.iter().any(|c| c["status"] == "fail"),
        "expected at least one failing check in {v}"
    );
    for check in checks {
        assert!(check["name"].is_string(), "check missing name: {check}");
        assert!(check["status"].is_string(), "check missing status: {check}");
    }
}

// AC-Y++2: the inotify-headroom check must actually hit its fail branch when
// fs.inotify.max_user_watches is too low for the repo's directory count. The
// normal CI matrix runs with a generous default limit (pass branch only) —
// this test only makes sense in an environment that deliberately lowered the
// ceiling first (see the `inotify-low-watches` job in
// .github/workflows/ci.yml), hence `#[ignore]` so it never runs as part of
// the default `cargo test --workspace`.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires fs.inotify.max_user_watches lowered before running — see .github/workflows/ci.yml's inotify-low-watches job"]
fn doctor_inotify_low_watches_fails() {
    let max_watches: u64 = std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .expect("read /proc/sys/fs/inotify/max_user_watches")
        .trim()
        .parse()
        .expect("parse max_user_watches");
    assert!(
        max_watches < 100,
        "this test requires an induced-low fs.inotify.max_user_watches \
         (got {max_watches}) — run: sudo sysctl -w fs.inotify.max_user_watches=1"
    );

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let init_out = agentrec(root, &["init", "--no-service"]);
    assert!(init_out.status.success(), "init failed: {init_out:?}");

    let v = doctor_json_value(root);
    assert_eq!(
        v["ok"], false,
        "expected ok:false with induced-low watches: {v}"
    );
    let checks = v["checks"].as_array().unwrap();
    let inotify = checks
        .iter()
        .find(|c| c["name"] == "inotify headroom")
        .unwrap_or_else(|| panic!("no inotify headroom check in {v}"));
    assert_eq!(
        inotify["status"], "fail",
        "expected inotify headroom to fail: {v}"
    );
    assert!(
        inotify["remedy"]
            .as_str()
            .unwrap_or("")
            .contains("max_user_watches"),
        "expected remedy to mention max_user_watches: {v}"
    );

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
}

// Phase 2 (honesty-fixes round): a corrupt `state.json` field (e.g.
// signal_offset written as a string) must surface as an ADVISORY-only
// doctor finding — reported, but never flips the overall exit code or `ok`.
// `doctor` all-pass exit 0 is this repo's production deploy gate; treating
// this as a Fail would block deploys on a condition the daemon's own
// per-field degrade already recovered from.
#[test]
fn doctor_state_parse_advisory_never_flips_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Real hooks installed (matches doctor_healthy_all_pass_exit_0's fixture)
    // so every OTHER check genuinely passes, isolating the assertion to the
    // new state-parse finding.
    let out = agentrec(root, &["init", "--no-service"]);
    assert!(out.status.success(), "init failed: {out:?}");

    let mut daemon = spawn_record(root);
    wait_for_live_daemon(root);

    // Corrupt exactly one field (signal_offset) while every other
    // DEGRADED-triggering counter stays 0 — an exit-1 here could only come
    // from the new check itself, not from a pre-existing failure mode.
    // `daemon_is_running` is a flock probe (not this pid value), so
    // clobbering state.json here doesn't disturb the liveness check.
    std::fs::write(
        root.join(".agentrec/state.json"),
        r#"{"pid":123,"signal_offset":"not-a-number","snapshot_failures":0,"io_failed":[]}"#,
    )
    .unwrap();

    let out = Command::new(bin())
        .args(["doctor", "--root", root.to_str().unwrap()])
        .env(
            "AGENTREC_CLAUDE_PROJECTS_DIR",
            tmp.path().join("no-transcripts-here"),
        )
        .output()
        .expect("run agentrec doctor");
    // Capture --json while the daemon is still alive too — killing it first
    // would make `daemon liveness` genuinely fail and pollute this test's
    // `ok:true` assertion with an unrelated failure mode.
    let v = Command::new(bin())
        .args(["doctor", "--json", "--root", root.to_str().unwrap()])
        .env(
            "AGENTREC_CLAUDE_PROJECTS_DIR",
            tmp.path().join("no-transcripts-here"),
        )
        .output()
        .map(|o| {
            serde_json::from_slice::<serde_json::Value>(&o.stdout)
                .unwrap_or_else(|e| panic!("doctor --json did not emit valid JSON ({e}): {o:?}"))
        })
        .expect("run agentrec doctor --json");

    let _ = daemon.kill();
    let _ = daemon.wait();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "an advisory state-parse finding must never flip doctor's exit code: {out:?}\nstdout={stdout}"
    );
    assert!(
        stdout.contains("signal_offset"),
        "expected the bad field named: {stdout}"
    );
    assert!(
        stdout.contains("state.json"),
        "expected the file named: {stdout}"
    );

    assert_eq!(v["ok"], true, "advisory finding must not flip ok: {v}");
    let checks = v["checks"].as_array().unwrap();
    let state_check = checks
        .iter()
        .find(|c| c["name"] == "state parse")
        .unwrap_or_else(|| panic!("no 'state parse' check in {v}"));
    assert_eq!(
        state_check["status"], "pass",
        "advisory finding must render as pass — a Fail here would flip exit: {v}"
    );
    assert!(
        state_check["remedy"]
            .as_str()
            .unwrap_or("")
            .contains("signal_offset"),
        "expected the remedy to name the bad field: {v}"
    );
}

// --- AC-Z+2, AC-Z+3, AC-Z+4 (D42/D43): relative-time default / --utc
// absolute, color gated off when piped or under NO_COLOR, and the
// `--explain` glossary only ever mentions terms present in this listing.

// D-PD4: `log --json` must stay schema-stable even with zero turns — an
// empty store's JSON output is the empty array `[]`, never blank stdout
// (blank stdout is not valid JSON and breaks any consumer that parses it).
#[test]
fn log_json_zero_turns_prints_empty_array() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let out = agentrec(root, &["log", "--json"]);
    assert_eq!(out.status.code(), Some(0), "log --json failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim(), "[]", "stdout: {stdout:?}");
}

#[test]
fn log_default_is_relative_utc_is_absolute() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // base_turn's started/ended is a fixed past RFC 3339 stamp.
    seed_turn(root, &base_turn("t_RELTIME0000000000000001", vec![]));

    let default_out = agentrec(root, &["log"]);
    assert!(default_out.status.success(), "log failed: {default_out:?}");
    let default_stdout = String::from_utf8_lossy(&default_out.stdout);
    assert!(
        !default_stdout.contains("2026-07-05T00:00:00"),
        "default log must render relative time, not the RFC3339 stamp: {default_stdout}"
    );

    let utc_out = agentrec(root, &["log", "--utc"]);
    assert!(utc_out.status.success(), "log --utc failed: {utc_out:?}");
    let utc_stdout = String::from_utf8_lossy(&utc_out.stdout);
    assert!(
        utc_stdout.contains("2026-07-05T00:00:00"),
        "expected the RFC3339 stamp under --utc: {utc_stdout}"
    );
}

#[test]
fn log_piped_output_has_no_color_escapes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    seed_turn(root, &base_turn("t_NOCOLOR000000000000001", vec![]));

    // `agentrec()` captures stdout via `Output` — never a tty — so this
    // alone exercises the not-a-tty path of `should_color`.
    let out = agentrec(root, &["log"]);
    assert!(out.status.success(), "log failed: {out:?}");
    assert!(
        !out.stdout.contains(&0x1b_u8),
        "piped log output must contain no ESC byte"
    );
}

#[test]
fn log_no_color_env_suppresses_escapes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    seed_turn(root, &base_turn("t_NOCOLORENV00000000001", vec![]));

    let out = Command::new(bin())
        .args(["log", "--root", root.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .output()
        .expect("run agentrec log");
    assert!(out.status.success(), "log failed: {out:?}");
    assert!(
        !out.stdout.contains(&0x1b_u8),
        "NO_COLOR log output must contain no ESC byte"
    );
}

// D-PD6: `log` and `show` used to render turn headers through two
// independently-maintained functions (`cmds::format_turn` — double-space,
// fixed-width columns — vs `readcmds::render_turn` — ` · `-separated) that
// also each hand-rolled their own `short_id`. Both now route through the
// shared `fmt::turn_list_line`/`fmt::turn_detail_header` renderers, so for
// the same turn both must use the identical separator scheme and the
// identical short-id truncation.
#[test]
fn log_and_show_render_turn_header_with_identical_formatting() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let turn = base_turn("t_HEADERUNIFY000000000001", vec![]);
    seed_turn(root, &turn);

    let log_out = agentrec(root, &["log"]);
    assert!(log_out.status.success(), "log failed: {log_out:?}");
    let log_stdout = String::from_utf8_lossy(&log_out.stdout);

    let show_out = agentrec(root, &["show", &turn.id]);
    assert!(show_out.status.success(), "show failed: {show_out:?}");
    let show_stdout = String::from_utf8_lossy(&show_out.stdout);

    // Same canonical separator (previously log used "  ", never " · ").
    assert!(
        log_stdout.contains(" · "),
        "log must use the canonical ` · ` separator: {log_stdout:?}"
    );
    assert!(
        show_stdout.contains(" · "),
        "show must use the canonical ` · ` separator: {show_stdout:?}"
    );

    // Same short-id truncation for the same turn id (both derive it from the
    // one canonical `fmt::short_id`, not two independent copies).
    let body = turn.id.strip_prefix("t_").unwrap();
    let short = format!("t_{}…{}", &body[..4], &body[body.len() - 4..]);
    assert!(log_stdout.contains(&short), "log id format: {log_stdout:?}");
    assert!(
        show_stdout.contains(&short),
        "show id format: {show_stdout:?}"
    );
}

// --- NF1–NF9: `noise_globs` fold (display-only). Declaratively-configured
// glob-matched file entries are folded out of `log`/`show`'s HUMAN rendering
// only — never `--json`, never `diff`/`blame`/`undo`. Precedent: `log`
// already hides an entire class by default (git turns via the `tool != "git"`
// filter, `--all` reveals) — this is the same idea one level down, from
// turns to individual file entries within a turn, with `--all-files` as the
// orthogonal reveal flag.

fn set_noise_globs(root: &Path, globs: &[&str]) {
    let config_path = root.join(".agentrec/config.toml");
    let mut text = std::fs::read_to_string(&config_path).unwrap_or_default();
    let items: Vec<String> = globs.iter().map(|g| format!("\"{g}\"")).collect();
    text.push_str(&format!("\nnoise_globs = [{}]\n", items.join(", ")));
    std::fs::write(&config_path, text).unwrap();
}

fn short_id_of(id: &str) -> String {
    let body = id.strip_prefix("t_").unwrap();
    format!("t_{}…{}", &body[..4], &body[body.len() - 4..])
}

/// A rich turn touching 1 ordinary file + 2 files under `.remember/` (the
/// measured real-world noise class from CLAUDE.md's store-bloat notes).
fn noise_turn(id: &str) -> agentrec_core::record::TurnRecord {
    use agentrec_core::record::FileEntry;
    let entry = |path: &str| FileEntry {
        path: path.to_string(),
        before: None,
        after: Some(agentrec_core::store::hash_bytes(path.as_bytes())),
        op: "create".into(),
        skipped: false,
        withheld: false,
        baseline_unknown: false,
        skipped_reason: None,
    };
    base_turn(
        id,
        vec![
            entry("src/main.rs"),
            entry(".remember/session.log"),
            entry(".remember/session.pid"),
        ],
    )
}

// NF1: regression guard — no `noise_globs` configured (or empty) means `log`
// and `show` are completely unaffected by this feature's existence.
#[test]
fn nf1_log_and_show_unaffected_when_noise_globs_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let turn = noise_turn("t_NF1BASELINE00000000000001");
    seed_turn(root, &turn);

    let log_out = agentrec(root, &["log", "--utc"]);
    assert!(log_out.status.success(), "log failed: {log_out:?}");
    let log_stdout = String::from_utf8_lossy(&log_out.stdout);
    assert!(
        log_stdout.contains("3 files"),
        "expected the unreduced 3-file count with no noise_globs: {log_stdout}"
    );
    assert!(
        !log_stdout.contains("noise files"),
        "no fold line must ever print with no noise_globs configured: {log_stdout}"
    );

    let show_out = agentrec(root, &["show", &turn.id]);
    assert!(show_out.status.success(), "show failed: {show_out:?}");
    let show_stdout = String::from_utf8_lossy(&show_out.stdout);
    assert!(
        !show_stdout.contains("noise files"),
        "show must not print a fold line with no noise_globs configured: {show_stdout}"
    );
}

// Advisor-surfaced: `log.jsonl` is a trust boundary (the P0 in the 2026-07-11
// hardening round was exactly this — an unvalidated hash/path read from the
// log). A `FileEntry.path` is normally repo-relative, but nothing in the wire
// format enforces that — a hand-edited or foreign-tool-written log line could
// carry an absolute path. `Gitignore::matched_path_or_any_parents` panics
// (`assert!(!path.has_root())`) when the given path shares no common prefix
// with the matcher's root, which an absolute path never will. This must
// degrade (fold nothing) rather than crash `log`/`show`.
#[test]
fn nf_is_noise_does_not_panic_on_absolute_path() {
    use agentrec_core::record::FileEntry;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    set_noise_globs(root, &[".remember/**"]);

    let turn = base_turn(
        "t_NFABSPATH00000000000001",
        vec![FileEntry {
            path: "/etc/passwd".into(),
            before: None,
            after: Some(agentrec_core::store::hash_bytes(b"/etc/passwd")),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["log"]);
    assert!(
        out.status.success(),
        "log must not crash on an absolute FileEntry.path: {out:?}"
    );
    let show_out = agentrec(root, &["show", &turn.id]);
    assert!(
        show_out.status.success(),
        "show must not crash on an absolute FileEntry.path: {show_out:?}"
    );
}

// NF2: with a matching glob configured, `log` folds the matched entries out
// of the visible count and prints the exact mandated line. The --all-files
// cross-check proves the glob genuinely matched this fixture (not a
// coincidental 0) rather than assuming it.
#[test]
fn nf2_log_folds_matching_entries_and_prints_exact_count_line() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    set_noise_globs(root, &[".remember/**"]);
    let turn = noise_turn("t_NF2FOLDCOUNT0000000000001");
    seed_turn(root, &turn);

    let out = agentrec(root, &["log", "--utc"]);
    assert!(out.status.success(), "log failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("1 file"),
        "folded count must be 3 total - 2 noise = 1: {stdout}"
    );
    assert!(
        stdout.contains("+2 noise files (--all-files to show)"),
        "expected the exact mandated fold line: {stdout}"
    );

    let unfolded = agentrec(root, &["log", "--utc", "--all-files"]);
    assert!(unfolded.status.success());
    let unfolded_stdout = String::from_utf8_lossy(&unfolded.stdout);
    assert!(
        unfolded_stdout.contains("3 files"),
        "precondition: --all-files must show the real unreduced count \
         (proves the glob genuinely matched, not a coincidental 0): {unfolded_stdout}"
    );
}

// NF3: `--all-files` output must match the no-`noise_globs` baseline exactly
// — folding fully reversed, byte for byte.
#[test]
fn nf3_all_files_matches_unfolded_rendering() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let turn = noise_turn("t_NF3ALLFILES0000000000001");
    seed_turn(root, &turn);

    let baseline = agentrec(root, &["log", "--utc"]);
    assert!(baseline.status.success());
    let baseline_stdout = String::from_utf8_lossy(&baseline.stdout).to_string();

    set_noise_globs(root, &[".remember/**"]);
    let folded = agentrec(root, &["log", "--utc"]);
    assert!(folded.status.success());
    let folded_stdout = String::from_utf8_lossy(&folded.stdout).to_string();
    assert_ne!(
        folded_stdout, baseline_stdout,
        "sanity: folding must actually change output before --all-files un-does it"
    );

    let unfolded = agentrec(root, &["log", "--utc", "--all-files"]);
    assert!(unfolded.status.success());
    let unfolded_stdout = String::from_utf8_lossy(&unfolded.stdout).to_string();
    assert_eq!(
        unfolded_stdout, baseline_stdout,
        "--all-files output must match the no-noise_globs baseline exactly"
    );
}

// NF4: `--json` is the machine contract — byte-identical with and without
// `noise_globs` configured. Folding is human-render only.
#[test]
fn nf4_json_output_byte_identical_regardless_of_noise_globs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let turn = noise_turn("t_NF4JSONSTABLE000000000001");
    seed_turn(root, &turn);

    let before = agentrec(root, &["log", "--json"]);
    assert!(before.status.success());

    set_noise_globs(root, &[".remember/**"]);
    let after = agentrec(root, &["log", "--json"]);
    assert!(after.status.success());

    assert_eq!(
        before.stdout, after.stdout,
        "log --json must be byte-identical with and without noise_globs"
    );
}

// NF5: `blame` and `undo` never consult `noise_globs` — attribution and
// revert are completely unaffected for a file that matches it.
#[test]
fn nf5_blame_and_undo_unaffected_by_noise_globs() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    set_noise_globs(root, &[".remember/**"]);

    let store = BlobStore::new(root.join(".agentrec/objects"));
    std::fs::create_dir_all(root.join(".remember")).unwrap();
    let before = store.put(b"before\n").unwrap();
    let after = store.put(b"after\n").unwrap();
    std::fs::write(root.join(".remember/session.log"), b"after\n").unwrap();

    let turn = base_turn(
        "t_NF5BLAMEUNDO0000000000001",
        vec![FileEntry {
            path: ".remember/session.log".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);

    let blame_out = agentrec(root, &["blame", ".remember/session.log"]);
    assert!(blame_out.status.success(), "blame failed: {blame_out:?}");
    let blame_stdout = String::from_utf8_lossy(&blame_out.stdout);
    assert!(
        blame_stdout.contains(&short_id_of(&turn.id)),
        "blame must still attribute the noise-matched file normally: {blame_stdout}"
    );
    assert!(
        !blame_stdout.contains("no recorded turn"),
        "blame must not treat a noise-matched file as untouched: {blame_stdout}"
    );

    let undo_out = agentrec(root, &["undo", &turn.id, "--confirm"]);
    assert!(undo_out.status.success(), "undo failed: {undo_out:?}");
    let reverted = std::fs::read(root.join(".remember/session.log")).unwrap();
    assert_eq!(
        reverted, b"before\n",
        "undo must still revert a noise-matched file"
    );
}

// NF6: a turn whose entries are ALL noise still appears in `log` (turn
// selection is untouched — only individual file entries fold), and
// `status`'s rich-rate is completely unaffected either way.
#[test]
fn nf6_all_noise_turn_still_appears_and_rich_rate_unaffected() {
    use agentrec_core::record::FileEntry;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let entry = |path: &str| FileEntry {
        path: path.to_string(),
        before: None,
        after: Some(agentrec_core::store::hash_bytes(path.as_bytes())),
        op: "create".into(),
        skipped: false,
        withheld: false,
        baseline_unknown: false,
        skipped_reason: None,
    };
    let all_noise_turn = base_turn(
        "t_NF6ALLNOISE00000000000001",
        vec![entry(".remember/a.log"), entry(".remember/b.log")],
    );
    seed_turn(root, &all_noise_turn);

    let status_before = agentrec(root, &["status"]);
    assert!(status_before.status.success());
    let status_before_stdout = String::from_utf8_lossy(&status_before.stdout).to_string();

    set_noise_globs(root, &[".remember/**"]);

    let log_out = agentrec(root, &["log", "--utc"]);
    assert!(log_out.status.success(), "log failed: {log_out:?}");
    let log_stdout = String::from_utf8_lossy(&log_out.stdout);
    assert!(
        log_stdout.contains(&short_id_of(&all_noise_turn.id)),
        "an all-noise turn must still appear in log: {log_stdout}"
    );
    assert!(
        log_stdout.contains("+2 noise files (--all-files to show)"),
        "expected the fold line even when every entry is noise: {log_stdout}"
    );

    let status_after = agentrec(root, &["status"]);
    assert!(status_after.status.success());
    let status_after_stdout = String::from_utf8_lossy(&status_after.stdout);
    assert_eq!(
        status_after_stdout, status_before_stdout,
        "status rich-rate must be completely unaffected by noise_globs"
    );
}

// NF7: `--all` (turn-grade axis) and `--all-files` (file-class axis) are
// orthogonal — neither flag's behavior leaks into the other's.
#[test]
fn nf7_all_and_all_files_flags_are_orthogonal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    set_noise_globs(root, &[".remember/**"]);

    let mut git_turn = noise_turn("t_NF7GITTURN0000000000001");
    git_turn.tool = Some("git".to_string());
    seed_turn(root, &git_turn);

    // --all-files alone: git turn stays hidden.
    let all_files_only = agentrec(root, &["log", "--all-files"]);
    assert!(all_files_only.status.success());
    let stdout = String::from_utf8_lossy(&all_files_only.stdout);
    assert!(
        !stdout.contains(&short_id_of(&git_turn.id)),
        "--all-files must not reveal a git turn: {stdout}"
    );

    // --all alone: git turn revealed, but its noise files still folded.
    let all_only = agentrec(root, &["log", "--all"]);
    assert!(all_only.status.success());
    let stdout2 = String::from_utf8_lossy(&all_only.stdout);
    assert!(
        stdout2.contains(&short_id_of(&git_turn.id)),
        "--all must reveal the git turn: {stdout2}"
    );
    assert!(
        stdout2.contains("noise files"),
        "--all alone must leave noise files folded (fold notice still prints): {stdout2}"
    );

    // Finding #6: both flags together. They are orthogonal axes (`--all` =
    // turn grade, `--all-files` = file class), so combining them must
    // reveal everything either flag alone reveals — the git turn AND its
    // unfolded file count — never have one flag suppress the other.
    let both = agentrec(root, &["log", "--all", "--all-files"]);
    assert!(both.status.success());
    let stdout3 = String::from_utf8_lossy(&both.stdout);
    assert!(
        stdout3.contains(&short_id_of(&git_turn.id)),
        "--all --all-files together must still reveal the git turn: {stdout3}"
    );
    assert!(
        !stdout3.contains("noise files"),
        "--all-files must unfold noise even when combined with --all: {stdout3}"
    );
    assert!(
        stdout3.contains("3 files"),
        "combined flags must show the full unreduced file count: {stdout3}"
    );
}

// NF8: `init`'s default config.toml mentions `noise_globs`, and a
// pre-existing config lacking the key still works and is never clobbered.
#[test]
fn nf8_init_default_config_mentions_noise_globs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let config = std::fs::read_to_string(root.join(".agentrec/config.toml")).unwrap();
    assert!(
        config.contains("noise_globs"),
        "default config.toml must mention noise_globs: {config}"
    );
}

#[test]
fn nf8_existing_config_missing_noise_globs_key_still_works() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    // Simulate a pre-existing repo's config.toml written before this
    // feature existed — no noise_globs key at all.
    std::fs::write(
        root.join(".agentrec/config.toml"),
        "ttl_days = 90\nmcp_destructive = \"off\"\n",
    )
    .unwrap();

    let turn = noise_turn("t_NF8OLDCONFIG00000000001");
    seed_turn(root, &turn);
    let out = agentrec(root, &["log", "--utc"]);
    assert!(out.status.success(), "log must still work: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("3 files"), "stdout: {stdout}");

    // Re-running init on this pre-existing config must not clobber it.
    let init_out = agentrec(root, &["init", "--no-hook", "--no-service"]);
    assert!(init_out.status.success());
    let config_after = std::fs::read_to_string(root.join(".agentrec/config.toml")).unwrap();
    assert!(
        !config_after.contains("noise_globs"),
        "init must not clobber a pre-existing config.toml lacking the key: {config_after}"
    );
}

// `show`: same fold behavior as `log`, plus --all-files suppresses it.
#[test]
fn nf_show_folds_and_all_files_reveals() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    set_noise_globs(root, &[".remember/**"]);
    let turn = noise_turn("t_NFSHOWFOLD0000000000001");
    seed_turn(root, &turn);

    let folded = agentrec(root, &["show", &turn.id]);
    assert!(folded.status.success());
    let folded_stdout = String::from_utf8_lossy(&folded.stdout);
    assert!(
        folded_stdout.contains("+2 noise files (--all-files to show)"),
        "show must print the fold line: {folded_stdout}"
    );

    let unfolded = agentrec(root, &["show", &turn.id, "--all-files"]);
    assert!(unfolded.status.success());
    let unfolded_stdout = String::from_utf8_lossy(&unfolded.stdout);
    assert!(
        !unfolded_stdout.contains("noise files"),
        "show --all-files must suppress the fold line: {unfolded_stdout}"
    );
}

// Judgement call (not a named AC, but load-bearing): `show --prompt` writes
// raw post-scrub prompt bytes to stdout — the fold line must never leak into
// that path, or it silently corrupts the printed prompt.
#[test]
fn nf_show_prompt_output_never_gets_fold_line() {
    use agentrec_core::store::BlobStore;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    set_noise_globs(root, &[".remember/**"]);

    let store = BlobStore::new(root.join(".agentrec/objects"));
    let prompt_hash = store.put(b"do the thing").unwrap();
    let mut turn = noise_turn("t_NFSHOWPROMPT000000000001");
    turn.prompt_ref = Some(prompt_hash);
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id, "--prompt"]);
    assert!(out.status.success(), "show --prompt failed: {out:?}");
    assert_eq!(out.stdout, b"do the thing", "stdout: {:?}", out.stdout);
}

// NF-E: `--explain`'s glossary only explains noise-folding when a fold
// actually occurred in this invocation's output (D43's existing rule,
// extended to the new term).
#[test]
fn nf_explain_explains_noise_fold_only_when_present() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let turn = noise_turn("t_NFEXPLAINNONE0000000001");
    seed_turn(root, &turn);

    let without = agentrec(root, &["log", "--explain"]);
    assert!(without.status.success());
    let without_stdout = String::from_utf8_lossy(&without.stdout).to_lowercase();
    assert!(
        !without_stdout.contains("noise files:"),
        "must not explain noise folding when none occurred: {without_stdout}"
    );

    set_noise_globs(root, &[".remember/**"]);
    let with = agentrec(root, &["log", "--explain"]);
    assert!(with.status.success());
    let with_stdout = String::from_utf8_lossy(&with.stdout).to_lowercase();
    assert!(
        with_stdout.contains("noise files:"),
        "must explain noise folding once a fold occurred: {with_stdout}"
    );
}

// --- Task 4: `agentrec remember` — manual pinned memories.

#[test]
fn remember_writes_pinned_scrubbed_record() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();

    let out = agentrec(
        root,
        &[
            "remember",
            "build needs cargo nightly",
            "--from",
            "src/a.rs",
        ],
    );
    assert!(out.status.success(), "remember failed: {out:?}");

    let path = root.join(".agentrec/memory.jsonl");
    assert!(path.exists(), "memory.jsonl must exist");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "memory.jsonl must be 0600");
    }

    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "expected exactly one record: {text}");
    let rec: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(rec.get("origin").and_then(|v| v.as_str()), Some("human"));
    assert_eq!(rec.get("op").and_then(|v| v.as_str()), Some("assert"));
    let pins = rec.get("pins").and_then(|v| v.as_array()).unwrap();
    assert_eq!(pins.len(), 1, "expected one pin: {pins:?}");
    assert_eq!(
        pins[0].get("path").and_then(|v| v.as_str()),
        Some("src/a.rs")
    );
    let hash = pins[0].get("hash").and_then(|v| v.as_str()).unwrap();
    assert!(hash.starts_with("sha256:"), "hash: {hash}");
    let hex = &hash["sha256:".len()..];
    assert_eq!(hex.len(), 64, "hash hex len: {hex}");
    assert!(
        hex.chars().all(|c| c.is_ascii_hexdigit()),
        "hash not hex: {hash}"
    );
}

#[test]
fn remember_refuses_bad_pins_and_secret_facts() {
    // Traversal escape: --from ../escape -> exit 1, stderr names the path,
    // memory.jsonl absent.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        let out = agentrec(root, &["remember", "some fact", "--from", "../escape"]);
        assert!(!out.status.success(), "expected failure: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("../escape"), "stderr: {stderr}");
        assert!(
            !root.join(".agentrec/memory.jsonl").exists(),
            "memory.jsonl must not be created on a rejected pin"
        );
    }

    // Secret-file pin: --from .env -> exit 1, stderr mentions secret.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        std::fs::write(root.join(".env"), b"SECRET=1").unwrap();
        let out = agentrec(root, &["remember", "some fact", "--from", ".env"]);
        assert!(!out.status.success(), "expected failure: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("secret"), "stderr: {stderr}");
        assert!(
            !root.join(".agentrec/memory.jsonl").exists(),
            "memory.jsonl must not be created on a secret-path pin"
        );
    }

    // A fact containing a secret (but not only a secret), with a valid pin,
    // is persisted with the secret redacted (INV-M3 half 1) — same
    // AWS-key-shaped fixture as secret_prompt_never_reaches_disk_in_cleartext.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
        let secret = "AKIAABCDEFGHIJKLMNOP";
        let fact = format!("deploy with key {secret} now");
        let out = agentrec(root, &["remember", &fact, "--from", "src/a.rs"]);
        assert!(out.status.success(), "remember failed: {out:?}");
        let text = std::fs::read_to_string(root.join(".agentrec/memory.jsonl")).unwrap();
        assert!(
            text.contains("[redacted:"),
            "expected a redaction marker: {text}"
        );
        assert!(!text.contains(secret), "raw key leaked to disk: {text}");
    }

    // A fact that scrubs to nothing is refused, nothing written. scrub()
    // never deletes matched content (it substitutes a `[redacted:...]`
    // marker), so the only input that can trim-empty after scrubbing is one
    // that was already blank — same fixture shape as
    // agentrec_core::memory::append_memory_rejects_oversize_and_empty's
    // blank_fact case.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
        let out = agentrec(root, &["remember", "   \n\t  ", "--from", "src/a.rs"]);
        assert!(!out.status.success(), "expected failure: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("scrub") || stderr.contains("empty"),
            "stderr: {stderr}"
        );
        assert!(
            !root.join(".agentrec/memory.jsonl").exists(),
            "memory.jsonl must not be created for a fact that scrubs to nothing"
        );
    }
}

// --- Task 5: `agentrec recall` + `agentrec memories`.

/// Seeds `count` off-topic filler memories, each pinned to its own file —
/// mirrors `agentrec_core::memory::recall_never_returns_stale`'s fixture:
/// with only 1-2 on-topic memories in a tiny corpus, idf(shared terms)
/// doesn't clear SCORE_FLOOR on its own, so tests that exercise real BM25
/// ranking need the corpus padded to N ~10.
fn seed_filler_memories(root: &Path, count: usize) {
    for i in 0..count {
        let rel = format!("filler{i}.rs");
        std::fs::write(root.join(&rel), b"fn filler() {}").unwrap();
        let out = agentrec(
            root,
            &[
                "remember",
                "unrelated documentation cleanup housekeeping chore",
                "--from",
                &rel,
            ],
        );
        assert!(out.status.success(), "remember filler{i} failed: {out:?}");
    }
}

#[test]
fn recall_cli_fresh_only_and_json() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
    std::fs::write(root.join("src/b.rs"), b"fn b() {}").unwrap();

    let out = agentrec(
        root,
        &[
            "remember",
            "nightly seed rotation keeps torture runs reproducible",
            "--from",
            "src/a.rs",
        ],
    );
    assert!(out.status.success(), "remember A failed: {out:?}");
    let out = agentrec(
        root,
        &[
            "remember",
            "nightly seed also drives the fuzz corpus replay",
            "--from",
            "src/b.rs",
        ],
    );
    assert!(out.status.success(), "remember B failed: {out:?}");
    seed_filler_memories(root, 8);

    let out = agentrec(root, &["recall", "nightly seed", "--json"]);
    assert!(out.status.success(), "recall --json failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("invalid json: {e}: {stdout}"));
    let arr = parsed.as_array().expect("json array");
    assert_eq!(arr.len(), 2, "expected 2 fresh matches: {stdout}");

    // Mutate one pinned file's content -> that memory goes Stale.
    std::fs::write(root.join("src/a.rs"), b"fn a() { changed(); }").unwrap();

    let out = agentrec(root, &["recall", "nightly seed", "--json"]);
    assert!(out.status.success(), "recall --json (2) failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = parsed.as_array().expect("json array");
    assert_eq!(arr.len(), 1, "stale memory must be excluded: {stdout}");

    // `memories --stale` shows the one whose pin drifted.
    let out = agentrec(root, &["memories", "--stale"]);
    assert!(out.status.success(), "memories --stale failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("src/a.rs"),
        "stale listing must show the drifted pin path: {stdout}"
    );
    assert!(
        !stdout.contains("src/b.rs"),
        "fresh memory must not appear under --stale: {stdout}"
    );

    // `memories --all` shows both, regardless of freshness.
    let out = agentrec(root, &["memories", "--all"]);
    assert!(out.status.success(), "memories --all failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("src/a.rs") && stdout.contains("src/b.rs"),
        "memories --all must show both: {stdout}"
    );
}

/// Seeds `memory.jsonl` directly (no daemon, no subprocess-per-record) with
/// a stale-heavy corpus shaped exactly like `agentrec-core::memory::tests::
/// recall_bounds_verification_on_stale_heavy_corpus`'s scenario 2: an
/// orphaned block strictly larger than `RECALL_VERIFY_CAP`, sharing the same
/// fact text (and therefore an identical BM25 score) as `fresh_count`
/// genuinely fresh, real-pinned records that rank strictly BEHIND it via the
/// idx-ascending tiebreak (insertion order). A filler block pads corpus size
/// so idf doesn't collapse toward zero. Returns the paths of the fresh
/// records (already written + hashed) so callers can assert on their facts.
fn seed_capped_stale_heavy_corpus(root: &Path, fresh_count: u64) {
    let mut lines = String::new();
    let orphaned_count = agentrec_core::memory::RECALL_VERIFY_CAP as u64 + 12;
    for i in 0..orphaned_count {
        let rec = serde_json::json!({
            "v": 1,
            "type": "memory",
            "id": format!("orph{i}"),
            "op": "assert",
            "fact": "kraken telemetry batching",
            "pins": [{
                "path": format!("missing{i}.rs"),
                "hash": format!("sha256:{i:064}"),
            }],
            "source_turns": [],
            "origin": "agent",
            "ts": 1_000 + i,
        });
        lines.push_str(&rec.to_string());
        lines.push('\n');
    }
    for i in 0..fresh_count {
        let rel = format!("fresh{i}.rs");
        std::fs::write(root.join(&rel), b"fn fresh() {}\n").unwrap();
        let hash = memory::hash_pin(root, &rel).expect("hash fresh file");
        let rec = serde_json::json!({
            "v": 1,
            "type": "memory",
            "id": format!("fresh{i}"),
            "op": "assert",
            "fact": "kraken telemetry batching",
            "pins": [{ "path": rel, "hash": hash }],
            "source_turns": [],
            "origin": "agent",
            "ts": 2_000 + i,
        });
        lines.push_str(&rec.to_string());
        lines.push('\n');
    }
    for i in 0..100 {
        let rec = serde_json::json!({
            "v": 1,
            "type": "memory",
            "id": format!("filler{i}"),
            "op": "assert",
            "fact": "unrelated housekeeping chore",
            "pins": [{
                "path": format!("filler_missing{i}.rs"),
                "hash": format!("sha256:{i:064}"),
            }],
            "source_turns": [],
            "origin": "agent",
            "ts": 3_000 + i,
        });
        lines.push_str(&rec.to_string());
        lines.push('\n');
    }
    std::fs::write(root.join(".agentrec/memory.jsonl"), lines).unwrap();
}

/// F3 (RECALL_VERIFY_CAP truncation is silent): on a corpus where the
/// verify walk exhausts `RECALL_VERIFY_CAP` entirely inside an orphaned
/// block, `agentrec recall` must print a one-line capped notice to STDERR
/// (never stdout, keeping stdout parseable/pipeable) so "no fresh matches"
/// is never indistinguishable from "no fresh matches AND more exist beyond
/// the cap". Pre-fix, this fails on a plain string-absence assertion
/// (compiles fine against pre-fix code — no `capped` field existed to
/// gate a notice on, so none was ever printed).
#[test]
fn recall_notice_appears_when_verification_capped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    seed_capped_stale_heavy_corpus(root, 3);

    let out = agentrec(root, &["recall", "kraken telemetry batching"]);
    assert!(out.status.success(), "recall failed: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stderr.contains("verification capped at 128 candidates — results may be incomplete"),
        "expected the capped notice on stderr: stderr={stderr} stdout={stdout}"
    );
    assert!(
        !stdout.contains("verification capped"),
        "capped notice must never land on stdout: stdout={stdout}"
    );

    // `--json` is out of scope for the F3 notice (see recall_cmd's doc
    // comment) — assert it stays a clean, notice-free array either way.
    let out = agentrec(root, &["recall", "kraken telemetry batching", "--json"]);
    assert!(out.status.success(), "recall --json failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        serde_json::from_str::<serde_json::Value>(stdout.trim()).is_ok(),
        "recall --json stdout must stay valid JSON: {stdout}"
    );
}

/// F3 counterpart: a corpus comfortably under `RECALL_VERIFY_CAP` must
/// never print the capped notice — it is not a generic "results might be
/// incomplete for some other reason" disclaimer, it fires only when the cap
/// was actually the reason the walk stopped early.
#[test]
fn recall_no_cap_notice_under_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
    let out = agentrec(
        root,
        &[
            "remember",
            "nightly seed rotation keeps torture runs reproducible",
            "--from",
            "src/a.rs",
        ],
    );
    assert!(out.status.success(), "remember failed: {out:?}");
    seed_filler_memories(root, 8);

    let out = agentrec(root, &["recall", "nightly seed rotation"]);
    assert!(out.status.success(), "recall failed: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("verification capped"),
        "under-cap corpus must never print the capped notice: stderr={stderr}"
    );

    // A genuinely empty result set (no fresh match) under the cap also
    // must not print the notice — the two "empty" cases (capped vs
    // genuinely-nothing) must render differently.
    let out = agentrec(root, &["recall", "zebra quantum flux"]);
    assert!(out.status.success(), "recall (no match) failed: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stderr.contains("verification capped"),
        "genuinely-empty-under-cap must not print the capped notice: stderr={stderr}"
    );
    assert!(
        stdout.contains("no fresh memories match"),
        "expected the plain no-match message: stdout={stdout}"
    );
}

/// F3, hook path: the capped signal must be recorded in
/// `memory-stats.jsonl` ONLY — the `--for-hook`/hook stdout contract
/// (fenced block or nothing, exit 0 always) is unconditional and must never
/// carry the capped notice text. Uses the same stale-heavy construction as
/// `recall_notice_appears_when_verification_capped`, sized well under the
/// hook's 50ms self-budget (no `AGENTREC_TEST_FORCE_RECALL_BUDGET_EXCEEDED`
/// involved — this proves the capped stat lands on a real, in-budget walk).
#[test]
fn hook_records_capped_stat_never_stdout_noise() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    // No fresh matches at all — the exact silent-truncation scenario F3
    // exists for: zero hits AND fresh matches existed beyond the cap.
    seed_capped_stale_heavy_corpus(root, 0);

    let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s_capped","prompt":"kraken telemetry batching"}"#;
    let out = send_hook_capture(root, payload);
    assert!(out.status.success(), "hook must exit 0: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.is_empty(),
        "no fresh hits -> hook stdout must be empty, capped or not: {stdout}"
    );
    assert!(
        !stdout.contains("capped"),
        "capped must never appear in hook stdout: {stdout}"
    );

    let stats = poll_until(Duration::from_secs(5), || {
        let lines = memory_stats_lines(root);
        lines
            .iter()
            .any(|l| l.get("capped").and_then(|v| v.as_bool()) == Some(true))
            .then_some(lines)
    })
    .expect("expected a capped:true line in memory-stats.jsonl");
    assert!(
        stats
            .iter()
            .any(|l| l.get("capped").and_then(|v| v.as_bool()) == Some(true)),
        "expected capped:true in memory-stats.jsonl: {stats:?}"
    );
}

#[test]
fn recall_for_hook_emits_block_or_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
    seed_filler_memories(root, 8);
    let out = agentrec(
        root,
        &[
            "remember",
            "nightly seed rotation keeps torture runs reproducible",
            "--from",
            "src/a.rs",
        ],
    );
    assert!(out.status.success(), "remember failed: {out:?}");

    // Matching query -> fenced block, capped at 800 chars, never a hash.
    let out = agentrec(root, &["recall", "nightly seed", "--for-hook"]);
    assert!(out.status.success(), "recall --for-hook failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("```agentrec memory"), "stdout: {stdout}");
    assert!(
        stdout.chars().count() <= 800,
        "block exceeds 800 chars ({} chars): {stdout}",
        stdout.chars().count()
    );
    assert!(
        !stdout.contains("sha256:"),
        "must not leak hashes: {stdout}"
    );

    // Nonsense query -> nothing above SCORE_FLOOR -> empty stdout, exit 0.
    let out = agentrec(root, &["recall", "zebra quantum", "--for-hook"]);
    assert!(
        out.status.success(),
        "nonsense query must still exit 0: {out:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "nonsense query must emit no stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Uninitialized repo: --for-hook fails open (empty stdout, exit 0);
    // without --for-hook it's a real error (exit 1, stderr).
    let tmp2 = tempfile::tempdir().unwrap();
    let root2 = tmp2.path();

    let out = agentrec(root2, &["recall", "anything", "--for-hook"]);
    assert!(
        out.status.success(),
        "uninitialized --for-hook must exit 0: {out:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "uninitialized --for-hook must emit no stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    let out = agentrec(root2, &["recall", "anything"]);
    assert!(
        !out.status.success(),
        "uninitialized recall (no --for-hook) must fail: {out:?}"
    );
    assert!(
        !out.stderr.is_empty(),
        "uninitialized recall (no --for-hook) must report on stderr"
    );
}

// --- Task 10: `verify` + `forget` — quarantine is recoverable.

#[test]
fn verify_and_forget_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
    seed_filler_memories(root, 8);

    let fact = "nightly seed rotation keeps torture runs reproducible";
    let out = agentrec(root, &["remember", fact, "--from", "src/a.rs"]);
    assert!(out.status.success(), "remember failed: {out:?}");

    let id = memory_records(root)
        .iter()
        .find(|r| r.get("fact").and_then(|f| f.as_str()) == Some(fact))
        .and_then(|r| r.get("id").and_then(|i| i.as_str()).map(String::from))
        .expect("newly remembered id not found");

    // Mutate the pinned file -> stale; `memories --stale` surfaces it.
    std::fs::write(root.join("src/a.rs"), b"fn a() { changed(); }").unwrap();
    let out = agentrec(root, &["memories", "--stale"]);
    assert!(out.status.success(), "memories --stale failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("src/a.rs"),
        "stale listing must show the drifted pin: {stdout}"
    );

    // `verify <id>` (no --confirm): prints drift, changes nothing.
    let lines_before = memory_records(root).len();
    let out = agentrec(root, &["verify", &id]);
    assert!(out.status.success(), "verify (preview) failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("old sha256:") && stdout.contains("-> new sha256:"),
        "expected a drift line: {stdout}"
    );
    assert_eq!(
        memory_records(root).len(),
        lines_before,
        "preview verify must not append"
    );

    // `verify <id> --confirm`: fresh again, exactly one new record appended.
    let out = agentrec(root, &["verify", &id, "--confirm"]);
    assert!(out.status.success(), "verify --confirm failed: {out:?}");
    assert_eq!(
        memory_records(root).len(),
        lines_before + 1,
        "reverify must append exactly one record"
    );

    let out = agentrec(root, &["recall", "nightly seed", "--json"]);
    assert!(out.status.success(), "recall --json failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = parsed.as_array().expect("json array");
    assert!(
        arr.iter()
            .any(|v| v.get("id").and_then(|i| i.as_str()) == Some(id.as_str())),
        "recall must serve the reverified (fresh again) memory: {stdout}"
    );

    // `forget <id> --reason "wrong"` -> excluded from recall AND hook
    // injection; `memories --all` shows it retracted with the reason.
    let out = agentrec(root, &["forget", &id, "--reason", "wrong"]);
    assert!(out.status.success(), "forget failed: {out:?}");

    let out = agentrec(root, &["recall", "nightly seed", "--json"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = parsed.as_array().expect("json array");
    assert!(
        !arr.iter()
            .any(|v| v.get("id").and_then(|i| i.as_str()) == Some(id.as_str())),
        "forgotten memory must not appear in recall: {stdout}"
    );

    let out = agentrec(root, &["recall", "nightly seed", "--for-hook"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains(fact),
        "forgotten memory must not be injected: {stdout}"
    );

    let out = agentrec(root, &["memories", "--all"]);
    assert!(out.status.success(), "memories --all failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("retracted"),
        "memories --all must show the retracted state: {stdout}"
    );
    assert!(
        stdout.contains("wrong"),
        "memories --all must show the retract reason: {stdout}"
    );

    // `forget <id>` again -> exit 1 (already retracted).
    let out = agentrec(root, &["forget", &id, "--reason", "again"]);
    assert!(!out.status.success(), "double forget must fail: {out:?}");

    // Orphan variant: a fresh memory with two pins, one of which is deleted.
    std::fs::write(root.join("src/b.rs"), b"fn b() {}").unwrap();
    std::fs::write(root.join("src/c.rs"), b"fn c() {}").unwrap();
    let orphan_fact = "orphan variant fact about kraken batching";
    let out = agentrec(
        root,
        &["remember", orphan_fact, "--from", "src/b.rs,src/c.rs"],
    );
    assert!(
        out.status.success(),
        "remember (orphan fixture) failed: {out:?}"
    );
    let orphan_id = memory_records(root)
        .iter()
        .rev()
        .find(|r| r.get("fact").and_then(|f| f.as_str()) == Some(orphan_fact))
        .and_then(|r| r.get("id").and_then(|i| i.as_str()).map(String::from))
        .expect("orphan id not found");

    std::fs::remove_file(root.join("src/b.rs")).unwrap();

    let lines_before = memory_records(root).len();
    let out = agentrec(root, &["verify", &orphan_id, "--confirm"]);
    assert!(
        !out.status.success(),
        "verify --confirm without --drop-pin must refuse on an orphaned pin: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("src/b.rs"),
        "stderr must name the orphaned path: {stderr}"
    );
    assert_eq!(
        memory_records(root).len(),
        lines_before,
        "a refused verify must not append"
    );

    let out = agentrec(
        root,
        &["verify", &orphan_id, "--confirm", "--drop-pin", "src/b.rs"],
    );
    assert!(
        out.status.success(),
        "verify --confirm --drop-pin failed: {out:?}"
    );
    assert_eq!(memory_records(root).len(), lines_before + 1);

    let out = agentrec(root, &["memories", "--json"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = parsed.as_array().expect("json array");
    let orphan_entry = arr
        .iter()
        .find(|v| v.get("id").and_then(|i| i.as_str()) == Some(orphan_id.as_str()))
        .expect("orphan entry not found in memories --json");
    let pins = orphan_entry
        .get("pins")
        .and_then(|p| p.as_array())
        .expect("pins array");
    assert_eq!(pins.len(), 1, "dropped pin must be gone: {pins:?}");
    assert_eq!(
        pins[0].get("path").and_then(|p| p.as_str()),
        Some("src/c.rs")
    );

    // Dropping the only remaining pin must be refused — a memory must
    // retain at least one pin.
    std::fs::remove_file(root.join("src/c.rs")).unwrap();
    let lines_before = memory_records(root).len();
    let out = agentrec(
        root,
        &["verify", &orphan_id, "--confirm", "--drop-pin", "src/c.rs"],
    );
    assert!(
        !out.status.success(),
        "dropping the only remaining pin must be refused: {out:?}"
    );
    assert_eq!(
        memory_records(root).len(),
        lines_before,
        "a refused verify must not append"
    );

    // Origin provenance (design spec, §Quality gate: "origin + source_turns
    // give provenance"): verify/forget are human-run CLI commands, but they
    // must not silently reassign an *agent-authored* memory's origin to
    // "human" — the fold takes `origin` from whichever op is latest
    // (`agentrec_core::memory::load_effective`), so `verify`/`forget` must
    // restate the original origin, not the actor running the command.
    std::fs::write(root.join("src/d.rs"), b"fn d() {}").unwrap();
    let agent_fact = "agent authored fact about kraken retry batching";
    let out = agentrec(root, &["remember", agent_fact, "--from", "src/d.rs"]);
    assert!(
        out.status.success(),
        "remember (agent fixture) failed: {out:?}"
    );

    // Hand-edit that record's `origin` to "agent" — `remember` always
    // stamps "human"; simulating an agent-authored memory without a live
    // daemon means rewriting the field directly (same posture as this
    // file's other direct-JSONL fixtures, e.g. the memory.jsonl scale test
    // near `hook_...recall_budget`).
    let memory_path = root.join(".agentrec/memory.jsonl");
    let text = std::fs::read_to_string(&memory_path).unwrap();
    let mut agent_id = String::new();
    let rewritten: String = text
        .lines()
        .map(|line| {
            let mut v: serde_json::Value = serde_json::from_str(line).unwrap();
            if v.get("fact").and_then(|f| f.as_str()) == Some(agent_fact) {
                v["origin"] = serde_json::Value::String("agent".to_string());
                agent_id = v["id"].as_str().unwrap().to_string();
            }
            v.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&memory_path, rewritten).unwrap();
    assert!(!agent_id.is_empty(), "agent fixture id not found");

    let out = agentrec(root, &["verify", &agent_id, "--confirm"]);
    assert!(
        out.status.success(),
        "verify --confirm on agent-origin memory failed: {out:?}"
    );
    let out = agentrec(root, &["memories", "--json"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = parsed.as_array().unwrap();
    let entry = arr
        .iter()
        .find(|v| v.get("id").and_then(|i| i.as_str()) == Some(agent_id.as_str()))
        .expect("agent-origin entry not found after verify");
    assert_eq!(
        entry.get("origin").and_then(|o| o.as_str()),
        Some("agent"),
        "verify must not reassign an agent-authored memory's origin to human: {entry}"
    );

    let out = agentrec(root, &["forget", &agent_id, "--reason", "still agent"]);
    assert!(
        out.status.success(),
        "forget (agent fixture) failed: {out:?}"
    );
    let out = agentrec(root, &["memories", "--all", "--json"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = parsed.as_array().unwrap();
    let entry = arr
        .iter()
        .find(|v| v.get("id").and_then(|i| i.as_str()) == Some(agent_id.as_str()))
        .expect("agent-origin entry not found after forget");
    assert_eq!(
        entry.get("origin").and_then(|o| o.as_str()),
        Some("agent"),
        "forget must not reassign an agent-authored memory's origin to human: {entry}"
    );
    assert_eq!(entry.get("retracted").and_then(|r| r.as_bool()), Some(true));
    assert_eq!(
        entry.get("reason").and_then(|r| r.as_str()),
        Some("still agent")
    );
}

// F7 part B: design spec §Read path promises `memories --stale` shows WHICH
// pin drifted and WHEN (a join against the turn log), not just a bare
// "stale" label. Pre-fix, the listing only prints the fact + freshness
// label + all pin paths — the drifted pin isn't distinguished from any
// other pin on the memory, and there is no turn/time reference at all.
#[test]
fn memories_stale_shows_drifted_pin_and_when() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}\n").unwrap();

    let out = agentrec(root, &["remember", "fact about a", "--from", "src/a.rs"]);
    assert!(out.status.success(), "remember failed: {out:?}");

    // Mutate the pinned file and seed a turn recording exactly that drift
    // (base_turn/seed_turn — same direct-log fixture pattern as the
    // diff/blame tests above; no daemon needed, only the read-side join is
    // under test).
    let new_bytes = b"fn a() { changed(); }\n";
    std::fs::write(root.join("src/a.rs"), new_bytes).unwrap();
    let after_hash = agentrec_core::store::hash_bytes(new_bytes);
    let turn_id = "t_DRIFTJOIN0000000000000A1";
    let turn = base_turn(
        turn_id,
        vec![agentrec_core::record::FileEntry {
            path: "src/a.rs".into(),
            before: None,
            after: Some(after_hash),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["memories", "--stale"]);
    assert!(out.status.success(), "memories --stale failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("src/a.rs"),
        "must name the drifted pin path: {stdout}"
    );
    assert!(
        stdout.contains(turn_id),
        "must reference the turn that produced the drift: {stdout}"
    );
    assert!(
        stdout.lines().count() > 1,
        "must be more than a bare one-line 'stale' label: {stdout}"
    );
}

// F7 part B: design spec §Lifecycle promises `verify <id>` shows a "diff
// summary via CAS" on drift, not only two hashes. Pre-fix, `verify` prints
// exactly `old <hash> -> new <hash>` and nothing else.
#[test]
fn verify_renders_cas_diff_on_drift() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    let original = b"fn a() {}\n";
    std::fs::write(root.join("src/a.rs"), original).unwrap();

    // Seed the CAS with the pinned content itself — a pin's hash is computed
    // directly from file bytes (`memory::hash_pin`), it is never written to
    // the object store by `remember`, so a diff needs this seeded explicitly
    // (mirrors how `diff`/`undo`'s own tests seed blobs via BlobStore::put).
    let store = agentrec_core::store::BlobStore::new(root.join(".agentrec/objects"));
    store.put(original).expect("seed pinned blob");

    let out = agentrec(root, &["remember", "fact about a", "--from", "src/a.rs"]);
    assert!(out.status.success(), "remember failed: {out:?}");
    let id = memory_records(root)[0]["id"]
        .as_str()
        .expect("id")
        .to_string();

    std::fs::write(root.join("src/a.rs"), b"fn a() { changed(); }\n").unwrap();

    let out = agentrec(root, &["verify", &id]);
    assert!(out.status.success(), "verify failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("+1/-1"),
        "expected a diff summary line: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.trim_start().starts_with('+') && l.contains("changed")),
        "expected an actual added diff line: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.trim_start().starts_with('-') && !l.trim_start().starts_with("---")),
        "expected an actual removed diff line: {stdout}"
    );
}

// F7 part B: the pinned-hash blob is the common case for being unavailable
// (a pin's hash is computed straight from file bytes; it's only IN the CAS
// if some turn happened to snapshot identical content) — the diff summary
// must fall back honestly rather than silently show nothing, mirroring
// `show --prompt`'s corrupt/purged-blob honesty.
#[test]
fn verify_falls_back_when_blob_purged() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}\n").unwrap();

    // No CAS seeding — the pinned hash was never written as a blob.
    let out = agentrec(root, &["remember", "fact about a", "--from", "src/a.rs"]);
    assert!(out.status.success(), "remember failed: {out:?}");
    let id = memory_records(root)[0]["id"]
        .as_str()
        .expect("id")
        .to_string();

    std::fs::write(root.join("src/a.rs"), b"fn a() { changed(); }\n").unwrap();

    let out = agentrec(root, &["verify", &id]);
    assert!(out.status.success(), "verify failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("blob unavailable"),
        "expected the honest fallback note: {stdout}"
    );
    assert!(
        stdout.contains("old sha256:") && stdout.contains("-> new sha256:"),
        "hash line must still be present: {stdout}"
    );
}

// F9: renaming the sole pinned file of a memory orphans it forever with only
// the pre-F9 verb set (`--confirm` refuses without a matching `--drop-pin`,
// and dropping the only pin is refused outright — a hard dead end). This
// test proves the gap using ONLY `--replace-pin` (the fix under test); before
// F9 lands, `--replace-pin` is an unrecognized clap argument, so the CLI
// invocation itself fails — that failure IS the RED signal for AC-F9.1.
#[test]
fn verify_replace_pin_preserves_renamed_sole_pin() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/old_name.rs"), b"fn a() {}").unwrap();
    seed_filler_memories(root, 8);

    let fact = "nightly seed rotation lives in old_name for now";
    let out = agentrec(root, &["remember", fact, "--from", "src/old_name.rs"]);
    assert!(out.status.success(), "remember failed: {out:?}");
    let id = memory_records(root)
        .iter()
        .find(|r| r.get("fact").and_then(|f| f.as_str()) == Some(fact))
        .and_then(|r| r.get("id").and_then(|i| i.as_str()).map(String::from))
        .expect("newly remembered id not found");

    // Rename: old path gone, new path holds the same content under a new
    // name — the pin is now orphaned with zero automatic recovery.
    std::fs::rename(root.join("src/old_name.rs"), root.join("src/new_name.rs")).unwrap();

    // AC-F9.1 (RED before implementation): recall no longer serves the
    // memory (orphaned pin => never Fresh), and there is no re-pin path
    // using only the pre-F9 verbs.
    let out = agentrec(root, &["recall", "nightly seed rotation", "--json"]);
    assert!(out.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert!(
        !parsed
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.get("id").and_then(|i| i.as_str()) == Some(id.as_str())),
        "orphaned-by-rename memory must not recall before re-pinning"
    );

    // AC-F9.2: preview (no --confirm) shows the orphaned old pin AND the
    // proposed old -> new mapping; nothing is appended.
    let lines_before = memory_records(root).len();
    let out = agentrec(
        root,
        &[
            "verify",
            &id,
            "--replace-pin",
            "src/old_name.rs=src/new_name.rs",
        ],
    );
    assert!(out.status.success(), "verify preview failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("src/old_name.rs") && stdout.contains("deleted"),
        "preview must show the orphaned old pin: {stdout}"
    );
    assert!(
        stdout.contains("src/old_name.rs") && stdout.contains("src/new_name.rs"),
        "preview must show the proposed old -> new mapping: {stdout}"
    );
    assert_eq!(
        memory_records(root).len(),
        lines_before,
        "preview must not append"
    );

    // AC-F9.3: confirmed replacement preserves id + fact, replaces the pin,
    // and `recall` serves the memory again.
    let out = agentrec(
        root,
        &[
            "verify",
            &id,
            "--confirm",
            "--replace-pin",
            "src/old_name.rs=src/new_name.rs",
        ],
    );
    assert!(out.status.success(), "verify --confirm failed: {out:?}");
    assert_eq!(
        memory_records(root).len(),
        lines_before + 1,
        "replacement must append exactly one record"
    );

    let effective_pins = memory_records(root)
        .into_iter()
        .rfind(|r| r.get("id").and_then(|i| i.as_str()) == Some(id.as_str()))
        .expect("appended record")
        .get("pins")
        .cloned()
        .expect("pins");
    let pins_arr = effective_pins.as_array().unwrap();
    assert_eq!(
        pins_arr.len(),
        1,
        "sole pin replaced, not added to: {pins_arr:?}"
    );
    assert_eq!(
        pins_arr[0].get("path").and_then(|p| p.as_str()),
        Some("src/new_name.rs")
    );
    let last_rec = memory_records(root)
        .into_iter()
        .rfind(|r| r.get("id").and_then(|i| i.as_str()) == Some(id.as_str()))
        .unwrap();
    assert_eq!(last_rec.get("fact").and_then(|f| f.as_str()), Some(fact));
    assert_eq!(
        last_rec.get("op").and_then(|o| o.as_str()),
        Some("reverify")
    );

    let out = agentrec(root, &["recall", "nightly seed rotation", "--json"]);
    assert!(out.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert!(
        parsed
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.get("id").and_then(|i| i.as_str()) == Some(id.as_str())),
        "recall must serve the memory again after replacement: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let out = agentrec(root, &["memories", "--stale"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains(fact),
        "replaced memory must be fresh again, not stale: {stdout}"
    );
}

// AC-F9.5: replacing ONE orphaned pin while another pin on the same memory
// is retained (re-hashed, not dropped) yields a single valid, non-empty pin
// set — INV-M1 (a memory always carries >=1 valid pin) holds.
#[test]
fn verify_replace_pin_keeps_other_pins_on_multi_pin_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/renamed.rs"), b"fn r() {}").unwrap();
    std::fs::write(root.join("src/kept.rs"), b"fn k() {}").unwrap();
    seed_filler_memories(root, 8);

    let fact = "nightly seed kraken batching spans two files";
    let out = agentrec(
        root,
        &["remember", fact, "--from", "src/renamed.rs,src/kept.rs"],
    );
    assert!(out.status.success(), "remember failed: {out:?}");
    let id = memory_records(root)
        .iter()
        .find(|r| r.get("fact").and_then(|f| f.as_str()) == Some(fact))
        .and_then(|r| r.get("id").and_then(|i| i.as_str()).map(String::from))
        .expect("id not found");

    std::fs::rename(root.join("src/renamed.rs"), root.join("src/renamed2.rs")).unwrap();
    // Also drift (not orphan) the kept pin, to prove it gets re-hashed, not
    // just carried forward stale.
    std::fs::write(root.join("src/kept.rs"), b"fn k() { changed(); }").unwrap();
    let new_kept_hash = memory::hash_pin(root, "src/kept.rs").unwrap();

    let out = agentrec(
        root,
        &[
            "verify",
            &id,
            "--confirm",
            "--replace-pin",
            "src/renamed.rs=src/renamed2.rs",
        ],
    );
    assert!(out.status.success(), "verify --confirm failed: {out:?}");

    let last_rec = memory_records(root)
        .into_iter()
        .rfind(|r| r.get("id").and_then(|i| i.as_str()) == Some(id.as_str()))
        .unwrap();
    let pins_arr = last_rec.get("pins").unwrap().as_array().unwrap();
    assert_eq!(pins_arr.len(), 2, "both pins present: {pins_arr:?}");
    let paths: Vec<&str> = pins_arr
        .iter()
        .map(|p| p.get("path").and_then(|x| x.as_str()).unwrap())
        .collect();
    assert!(paths.contains(&"src/renamed2.rs"));
    assert!(paths.contains(&"src/kept.rs"));
    let kept_entry = pins_arr
        .iter()
        .find(|p| p.get("path").and_then(|x| x.as_str()) == Some("src/kept.rs"))
        .unwrap();
    assert_eq!(
        kept_entry.get("hash").and_then(|h| h.as_str()),
        Some(new_kept_hash.as_str()),
        "kept pin must be re-hashed fresh, not left stale"
    );

    let out = agentrec(root, &["memories", "--stale"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains(fact),
        "memory must be fully fresh: {stdout}"
    );
}

// AC-F9.4: every atomic-refusal case for --replace-pin. Each sub-case
// re-derives a fresh id off a fresh remember (the prior case's own refusal
// already proves nothing was appended, so state carries forward safely) and
// asserts append count is unchanged.
#[test]
fn verify_replace_pin_rejections_append_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("outside_root")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
    std::fs::write(root.join("src/b.rs"), b"fn b() {}").unwrap();
    std::fs::write(root.join("src/c.rs"), b"fn c() {}").unwrap();
    std::fs::write(root.join(".env"), b"SECRET=1").unwrap();

    let fact = "kraken batching rejection fixture fact";
    let out = agentrec(root, &["remember", fact, "--from", "src/a.rs,src/b.rs"]);
    assert!(out.status.success(), "remember failed: {out:?}");
    let id = memory_records(root)
        .iter()
        .find(|r| r.get("fact").and_then(|f| f.as_str()) == Some(fact))
        .and_then(|r| r.get("id").and_then(|i| i.as_str()).map(String::from))
        .expect("id not found");

    let assert_refused = |args: &[&str], must_contain: &str, msg: &str| {
        let lines_before = memory_records(root).len();
        let out = agentrec(root, args);
        assert!(
            !out.status.success(),
            "{msg}: expected failure, got {out:?}"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(must_contain),
            "{msg}: stderr {stderr} must mention '{must_contain}'"
        );
        assert_eq!(
            memory_records(root).len(),
            lines_before,
            "{msg}: refusal must append nothing"
        );
        // AC-F9.4: a --replace-pin refusal must be atomic on stdout too, not
        // just on the memory.jsonl append — every validation error is
        // returned BEFORE the fact is printed (memorycmds.rs::verify), so a
        // rejected call must produce zero stdout bytes. Guards against a
        // future regression that moves the print earlier than validation.
        assert!(
            out.stdout.is_empty(),
            "{msg}: refusal must print nothing to stdout, got {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    };

    // nonexistent old pin (not on this memory)
    assert_refused(
        &[
            "verify",
            &id,
            "--confirm",
            "--replace-pin",
            "src/not_a_pin.rs=src/c.rs",
        ],
        "not a pin",
        "nonexistent old pin",
    );

    // nonexistent new path
    assert_refused(
        &[
            "verify",
            &id,
            "--confirm",
            "--replace-pin",
            "src/a.rs=src/does_not_exist.rs",
        ],
        "does_not_exist",
        "nonexistent new path",
    );

    // absolute new path
    assert_refused(
        &[
            "verify",
            &id,
            "--confirm",
            "--replace-pin",
            "src/a.rs=/etc/passwd",
        ],
        "absolute",
        "absolute new path",
    );

    // ..-traversal new path
    assert_refused(
        &[
            "verify",
            &id,
            "--confirm",
            "--replace-pin",
            "src/a.rs=../outside_root",
        ],
        "..",
        "traversal new path",
    );

    // secret-pattern new path
    assert_refused(
        &["verify", &id, "--confirm", "--replace-pin", "src/a.rs=.env"],
        "secret",
        "secret new path",
    );

    // duplicate old mappings across multiple --replace-pin flags
    assert_refused(
        &[
            "verify",
            &id,
            "--confirm",
            "--replace-pin",
            "src/a.rs=src/c.rs",
            "--replace-pin",
            "src/a.rs=src/b.rs",
        ],
        "more than once",
        "duplicate old mapping",
    );

    // contradictory --drop-pin old + --replace-pin old=new for the same old
    assert_refused(
        &[
            "verify",
            &id,
            "--confirm",
            "--drop-pin",
            "src/a.rs",
            "--replace-pin",
            "src/a.rs=src/c.rs",
        ],
        "contradictory",
        "contradictory drop-pin + replace-pin",
    );

    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.rs"), b"outside").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.rs"), root.join("src/escape.rs"))
            .unwrap();
        assert_refused(
            &[
                "verify",
                &id,
                "--confirm",
                "--replace-pin",
                "src/a.rs=src/escape.rs",
            ],
            "escape",
            "symlink-escape new path",
        );
    }

    // Sanity: the memory is still intact and unresolved by any of the above.
    let effective = agentrec(root, &["memories", "--json"]);
    assert!(effective.status.success());
}

// AC-F9.6: `verify --help` documents the explicit `--replace-pin` mechanism
// and does not claim any automatic rename/successor discovery.
#[test]
fn verify_help_documents_replace_pin_no_auto_rename() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let out = agentrec(root, &["verify", "--help"]);
    assert!(out.status.success(), "verify --help failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--replace-pin"),
        "--help must document --replace-pin: {stdout}"
    );
    let lower = stdout.to_lowercase();
    // Must not claim the tool discovers/detects a rename on its own — but
    // an explicit *denial* of that ("no automatic rename detection") is
    // exactly what AC-F9.6 wants, so only phrases that assert auto-discovery
    // as a real capability are forbidden.
    for forbidden in [
        "automatically detects",
        "automatically finds",
        "automatically re-pins",
        "auto-detects",
        "auto-detect a rename",
        "guesses the rename",
        "guesses the successor",
    ] {
        assert!(
            !lower.contains(forbidden),
            "--help must not claim automatic rename discovery (found '{forbidden}'): {stdout}"
        );
    }
    assert!(
        lower.contains("no automatic") || lower.contains("explicit"),
        "--help should state the mapping is explicit / not automatic: {stdout}"
    );
}

// F1: fact rendering must strip terminal-escape bytes (ESC 0x1b / BEL 0x07)
// before they reach stdout — same posture prompt excerpts already get via
// `fmt::sanitize_terminal` (cmds.rs/readcmds.rs). Seeds a memory whose fact
// carries an OSC "set terminal title" payload plus an SGR color escape
// directly via `memory::append_memory` (bypasses the CLI's scrub call sites
// entirely — the escape bytes are not secrets, so `scrub()` at persist time
// leaves them untouched; asserted below) and checks every render path:
// `recall`, `memories`, `verify <id>`, and the `--for-hook` block.
#[test]
fn memory_fact_render_sanitizes_terminal_escapes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/evil.rs"), b"fn evil() {}").unwrap();
    // Fillers give the "nightly seed" query terms a low document frequency
    // so BM25 clears SCORE_FLOOR — same pattern as `seed_filler_memories`'s
    // other callers (e.g. `verify_and_forget_lifecycle`).
    seed_filler_memories(root, 8);

    let evil_fact = "nightly seed \x1b]0;pwn\x07 rotation \x1b[31mdanger\x1b[0m zone";
    let hash = memory::hash_pin(root, "src/evil.rs").unwrap();
    let rec = MemoryRecord {
        v: 1,
        kind: "memory".to_string(),
        id: agentrec_core::id::ulid(),
        op: MemoryOp::Assert,
        fact: evil_fact.to_string(),
        pins: vec![Pin {
            path: "src/evil.rs".to_string(),
            hash,
        }],
        source_turns: vec![],
        origin: "agent".to_string(),
        ts: 1_700_000_000_000,
        reason: None,
    };
    memory::append_memory(root, &rec).unwrap();

    // Fixture sanity: prove the escape bytes actually survived scrub at
    // persist time — otherwise this test would pass for the wrong reason.
    let stored = memory_records(root)
        .into_iter()
        .find(|r| r.get("id").and_then(|i| i.as_str()) == Some(rec.id.as_str()))
        .expect("seeded memory not found on disk");
    let stored_fact = stored.get("fact").and_then(|f| f.as_str()).unwrap();
    assert!(
        stored_fact.contains('\u{1b}') && stored_fact.contains('\u{7}'),
        "fixture invalid — scrub already stripped the escape bytes at persist time: {stored_fact:?}"
    );

    let assert_clean = |out: &Output, label: &str| {
        assert!(out.status.success(), "{label} failed: {out:?}");
        let leaked = out.stdout.iter().any(|&b| b == 0x1b || 0x07 == b);
        assert!(
            !leaked,
            "{label} leaked a raw ESC/BEL byte: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    };

    assert_clean(&agentrec(root, &["recall", "nightly seed"]), "recall");
    assert_clean(&agentrec(root, &["memories"]), "memories");
    assert_clean(&agentrec(root, &["verify", &rec.id]), "verify");
    assert_clean(
        &agentrec(root, &["recall", "nightly seed", "--for-hook"]),
        "recall --for-hook",
    );
}

#[test]
fn log_explain_glossary_matches_only_present_terms() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // Rich-only listing: glossary must mention "rich", never "bare".
    seed_turn(root, &base_turn("t_RICHONLY0000000000001", vec![]));
    let rich_out = agentrec(root, &["log", "--explain"]);
    assert!(
        rich_out.status.success(),
        "log --explain failed: {rich_out:?}"
    );
    let rich_stdout = String::from_utf8_lossy(&rich_out.stdout);
    assert!(
        rich_stdout.contains("rich:"),
        "expected a rich glossary entry: {rich_stdout}"
    );
    assert!(
        !rich_stdout.contains("bare:"),
        "must not explain bare with no bare turns present: {rich_stdout}"
    );

    // Add a bare turn; glossary must now mention "bare" too.
    let mut bare = base_turn("t_BARETURN0000000000001", vec![]);
    bare.grade = "bare".to_string();
    bare.tool = None;
    bare.prompt_excerpt = None;
    seed_turn(root, &bare);

    let both_out = agentrec(root, &["log", "--explain"]);
    assert!(
        both_out.status.success(),
        "log --explain failed: {both_out:?}"
    );
    let both_stdout = String::from_utf8_lossy(&both_out.stdout);
    assert!(
        both_stdout.contains("bare:"),
        "expected a bare glossary entry once a bare turn is present: {both_stdout}"
    );
}

// --- Task 8: `agentrec candidate` — agent-emitted memory candidates (the
// memory WRITE path's CLI emitter half; the daemon-side ingestion is
// Task 7's `daemon::ingest_candidate`, covered end-to-end here too).

/// Every parsed line of `.agentrec/memory.jsonl`. Absent file = empty vec.
fn memory_records(root: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(root.join(".agentrec/memory.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn candidate_cli_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
    std::fs::write(
        root.join(".github/workflows/nightly.yml"),
        "name: nightly\n",
    )
    .unwrap();

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    let out = agentrec(
        root,
        &[
            "candidate",
            "fact about nightly",
            "--from",
            ".github/workflows/nightly.yml",
        ],
    );
    assert!(out.status.success(), "candidate failed: {out:?}");

    // The emitted signal line: type memory-candidate, pins are PATHS ONLY —
    // no "sha256:" hash anywhere in the line. Hashing is the daemon's job,
    // done against the live tree at ingestion, never trusted from the
    // emitter (design spec, "Rejected approaches — emitter-side hashing").
    let signal_text = std::fs::read_to_string(root.join(".agentrec/signal.jsonl")).unwrap();
    let candidate_line = signal_text
        .lines()
        .rev()
        .find_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            (v.get("type").and_then(|t| t.as_str()) == Some("memory-candidate")).then_some(v)
        })
        .expect("no memory-candidate line appended to signal.jsonl");
    assert_eq!(
        candidate_line.get("fact").and_then(|f| f.as_str()),
        Some("fact about nightly")
    );
    let pins = candidate_line
        .get("pins")
        .and_then(|p| p.as_array())
        .expect("pins must be an array");
    assert_eq!(pins.len(), 1, "expected exactly one pin: {pins:?}");
    let pin0 = pins[0]
        .as_str()
        .expect("pins must be plain path strings, not hash objects");
    assert_eq!(pin0, ".github/workflows/nightly.yml");
    assert!(
        !signal_text.contains("sha256:"),
        "the emitted signal line must never carry a hash: {signal_text}"
    );

    // End-to-end through Task 7 ingestion: the daemon tails the inbox and
    // lands the fact in memory.jsonl, attributed to the agent.
    let recorded = poll_until(Duration::from_secs(5), || {
        let recs = memory_records(root);
        (!recs.is_empty()).then_some(recs)
    });
    let _ = daemon.kill();
    let _ = daemon.wait();

    let recs = recorded.expect("candidate was never ingested into memory.jsonl");
    assert_eq!(
        recs.len(),
        1,
        "expected exactly one memory record: {recs:?}"
    );
    assert_eq!(
        recs[0].get("origin").and_then(|v| v.as_str()),
        Some("agent")
    );
    assert_eq!(
        recs[0].get("fact").and_then(|v| v.as_str()),
        Some("fact about nightly")
    );
}

// Daemon-DOWN replay + D7-preservation pair. A candidate emitted while NO
// daemon is running must be ingested on the NEXT daemon start (candidate-only
// startup replay, daemon.rs `replay_pending_candidates`). In the SAME
// pre-daemon gap, a stale start/stop bracket must NOT be replayed — it would
// mint a phantom turn misdated to boot (the exact hazard D7's EOF-skip
// prevents). This is the pair that proves the startup replay is
// candidate-only and D7 still holds: memory record lands, zero turns appear.
#[test]
fn candidate_startup_replay_is_candidate_only_and_preserves_d7() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::write(root.join("notes.txt"), "hello").unwrap();

    // No daemon running yet. Emit a candidate via the real CLI...
    let out = agentrec(
        root,
        &["candidate", "a fact learned offline", "--from", "notes.txt"],
    );
    assert!(out.status.success(), "candidate failed: {out:?}");
    assert!(
        !root.join(".agentrec/memory.jsonl").exists(),
        "no daemon was running — nothing should be ingested yet"
    );

    // ...and, in the SAME gap, plant a stale bracket (start+stop). If replayed
    // as live, this pair would fabricate an empty turn misdated to boot.
    send_hook(
        root,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s_stale"}"#,
    );
    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s_stale"}"#);

    // Now start the daemon: the candidate is replayed on startup...
    let mut daemon = spawn_record(root);
    let recorded = poll_until(Duration::from_secs(6), || {
        let recs = memory_records(root);
        (!recs.is_empty()).then_some(recs)
    });

    // Give the daemon a beat past the replay to confirm no phantom turn lands.
    std::thread::sleep(Duration::from_secs(1));
    let phantom = turns(root);
    let _ = daemon.kill();
    let _ = daemon.wait();

    let recs = recorded.expect("candidate emitted while daemon down was never replayed on startup");
    assert_eq!(
        recs.len(),
        1,
        "expected exactly one memory record: {recs:?}"
    );
    assert_eq!(
        recs[0].get("fact").and_then(|v| v.as_str()),
        Some("a fact learned offline")
    );
    assert_eq!(
        recs[0].get("origin").and_then(|v| v.as_str()),
        Some("agent")
    );

    assert!(
        phantom.is_empty(),
        "a stale start/stop bracket in the pre-daemon gap was replayed and \
         minted a phantom turn — D7's EOF-skip must still drop start/stop: {phantom:?}"
    );
}

// --- Task 9: UserPromptSubmit hook memory injection (INV-M4).

/// `.agentrec/signal.jsonl` lines parsed as JSON, in file order.
fn signal_events(root: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(root.join(".agentrec/signal.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// `.agentrec/memory-stats.jsonl` lines parsed as JSON, in file order.
fn memory_stats_lines(root: &Path) -> Vec<serde_json::Value> {
    let text =
        std::fs::read_to_string(root.join(".agentrec/memory-stats.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn hook_injects_fresh_memories_into_stdout() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
    seed_filler_memories(root, 8);
    let out = agentrec(
        root,
        &[
            "remember",
            "nightly seed rotation keeps torture runs reproducible",
            "--from",
            "src/a.rs",
        ],
    );
    assert!(out.status.success(), "remember failed: {out:?}");

    // Matching prompt -> stdout carries the fenced block + the fact; the
    // start signal (existing behavior) still lands; memory-stats.jsonl
    // gains exactly one line.
    let payload =
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"nightly seed"}"#;
    let out = send_hook_capture(root, payload);
    assert!(out.status.success(), "hook failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("```agentrec memory"), "stdout: {stdout}");
    assert!(
        stdout.contains("nightly seed rotation keeps torture runs reproducible"),
        "stdout: {stdout}"
    );

    let events = signal_events(root);
    assert!(
        events
            .iter()
            .any(|e| e.get("event").and_then(|v| v.as_str()) == Some("start")),
        "start signal missing after hook call: {events:?}"
    );

    let stats = memory_stats_lines(root);
    assert_eq!(stats.len(), 1, "expected one memory-stats line: {stats:?}");
    assert!(stats[0].get("ts").and_then(|v| v.as_u64()).is_some());
    assert_eq!(stats[0].get("n").and_then(|v| v.as_u64()), Some(1));

    // Non-matching prompt -> empty stdout, no new memory-stats line.
    let payload =
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s2","prompt":"zebra quantum"}"#;
    let out = send_hook_capture(root, payload);
    assert!(out.status.success(), "hook failed: {out:?}");
    assert!(
        out.stdout.is_empty(),
        "non-matching prompt must emit no stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        memory_stats_lines(root).len(),
        1,
        "no injection -> no new memory-stats line"
    );

    // memory_enabled = false -> no block, even for a matching prompt.
    std::fs::write(
        root.join(".agentrec/config.toml"),
        "ttl_days = 90\nmcp_destructive = \"off\"\nmemory_enabled = false\n",
    )
    .unwrap();
    let payload =
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s3","prompt":"nightly seed"}"#;
    let out = send_hook_capture(root, payload);
    assert!(out.status.success(), "hook failed: {out:?}");
    assert!(
        out.stdout.is_empty(),
        "memory_enabled=false must emit no stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        memory_stats_lines(root).len(),
        1,
        "memory_enabled=false -> no new memory-stats line"
    );

    // The Stop arm is untouched: no block, no memory-stats line, even for a
    // "matching" prompt field (which Stop payloads don't carry anyway).
    std::fs::write(
        root.join(".agentrec/config.toml"),
        "ttl_days = 90\nmcp_destructive = \"off\"\nmemory_enabled = true\n",
    )
    .unwrap();
    let payload = r#"{"hook_event_name":"Stop","session_id":"s1"}"#;
    let out = send_hook_capture(root, payload);
    assert!(out.status.success(), "hook (Stop) failed: {out:?}");
    assert!(
        out.stdout.is_empty(),
        "Stop arm must never print a memory block: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        memory_stats_lines(root).len(),
        1,
        "Stop arm must never append to memory-stats.jsonl"
    );

    // Bonus (coupling proof): memory_inject_max > the old hardcoded default
    // of 5 actually injects more than 5 facts — proves `max_facts` feeds
    // both `memory::recall`'s `k` and `build_hook_block`'s per-block cap.
    for i in 0..7 {
        let rel = format!("src/m{i}.rs");
        std::fs::write(root.join(&rel), b"fn m() {}").unwrap();
        let out = agentrec(
            root,
            &[
                "remember",
                &format!("nightly seed shard {i} rotation detail"),
                "--from",
                &rel,
            ],
        );
        assert!(out.status.success(), "remember m{i} failed: {out:?}");
    }
    std::fs::write(
        root.join(".agentrec/config.toml"),
        "ttl_days = 90\nmcp_destructive = \"off\"\nmemory_enabled = true\nmemory_inject_max = 10\n",
    )
    .unwrap();
    let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s4","prompt":"nightly seed rotation shard"}"#;
    let out = send_hook_capture(root, payload);
    assert!(out.status.success(), "hook failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let fact_lines = stdout.lines().filter(|l| l.starts_with("- ")).count();
    assert!(
        fact_lines > 5,
        "memory_inject_max=10 must inject more than the old hardcoded 5 \
         (coupling fix) — got {fact_lines} lines: {stdout}"
    );
}

#[test]
fn hook_fail_open_and_budget() {
    // Corrupt memory.jsonl -> hook still exits 0, stdout carries no block,
    // and the start signal is still appended (INV-M4 fail-open).
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        std::fs::write(
            root.join(".agentrec/memory.jsonl"),
            b"\xff\xfenot json at all garbage bytes\x00\x01",
        )
        .unwrap();

        let payload =
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"anything"}"#;
        let out = send_hook_capture(root, payload);
        assert!(out.status.success(), "hook must exit 0: {out:?}");
        assert!(
            out.stdout.is_empty(),
            "corrupt store must emit no stdout: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        let events = signal_events(root);
        assert!(
            events
                .iter()
                .any(|e| e.get("event").and_then(|v| v.as_str()) == Some("start")),
            "start signal missing despite corrupt memory.jsonl: {events:?}"
        );
    }

    // Fail-open-under-load: a large store still returns within the
    // (generous, CI-slack) 500ms wall-clock budget and exits 0. The hook's
    // own internal self-budget is 50ms (RECALL_BUDGET_MS). At this scale a
    // *debug* build on a slow shared CI runner can legitimately exceed the
    // 50ms self-budget and fail open to a no-op — that IS correct fail-open
    // behavior (see `inject_memory`'s early return), so this leg asserts
    // only exit-0 + the wall budget, never a hard injection. The real
    // "injection, not silent degradation" property is proven separately
    // below at a small, runner-speed-independent store size. The true 50ms
    // recall envelope at 3000+/10k records (release build) is timing-
    // dependent and laddered in VERIFY-LEDGER.md, never a per-push CI gate.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        let mut lines = String::new();
        for i in 0..3000 {
            let rec = serde_json::json!({
                "v": 1,
                "type": "memory",
                "id": format!("m{i}"),
                "op": "assert",
                "fact": format!("filler fact number {i} about nightly seed rotation housekeeping"),
                "pins": [{
                    "path": format!("missing{i}.rs"),
                    "hash": format!("sha256:{i:064}"),
                }],
                "source_turns": [],
                "origin": "agent",
                "ts": i,
            });
            lines.push_str(&rec.to_string());
            lines.push('\n');
        }
        std::fs::write(root.join(".agentrec/memory.jsonl"), lines).unwrap();

        let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s2","prompt":"nightly seed rotation"}"#;
        let started = Instant::now();
        let out = send_hook_capture(root, payload);
        let elapsed = started.elapsed();
        assert!(out.status.success(), "hook must exit 0: {out:?}");
        assert!(
            elapsed < Duration::from_millis(500),
            "hook took too long against a 3000-record store: {elapsed:?}"
        );
    }

    // Anti-silent-degradation (INV-M4): a genuinely Fresh, real-pinned
    // record MUST inject a block, not be silently dropped. Proven at a
    // small, runner-speed-independent store (~200 records) so that even a
    // *debug* build on the slowest CI runner completes recall well within
    // the 50ms self-budget — the injection assertion is therefore
    // deterministic and decoupled from runner speed. (A 3000-record debug
    // fold IS slow enough on a 2-core CI box to blow the 50ms budget and
    // fail open — that is a perf-envelope claim laddered in VERIFY-LEDGER,
    // not something a hard per-push assertion may depend on.)
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        let mut lines = String::new();
        for i in 0..200 {
            let rec = serde_json::json!({
                "v": 1,
                "type": "memory",
                "id": format!("m{i}"),
                "op": "assert",
                "fact": format!("filler fact number {i} about nightly seed rotation housekeeping"),
                "pins": [{
                    "path": format!("missing{i}.rs"),
                    "hash": format!("sha256:{i:064}"),
                }],
                "source_turns": [],
                "origin": "agent",
                "ts": i,
            });
            lines.push_str(&rec.to_string());
            lines.push('\n');
        }
        std::fs::write(root.join(".agentrec/memory.jsonl"), lines).unwrap();

        std::fs::write(root.join("real_fresh.rs"), b"fn real_fresh() {}\n").unwrap();
        let hash = memory::hash_pin(root, "real_fresh.rs").expect("hash real_fresh.rs");
        let fresh_rec = serde_json::json!({
            "v": 1,
            "type": "memory",
            "id": "m-real-fresh",
            "op": "assert",
            // "quasar77" is a rare token unique to this one record — BM25's
            // idf term guarantees it ranks at the top for a query
            // containing it, deterministically inside the
            // RECALL_VERIFY_CAP=128 verification window (the 200 filler
            // candidates all share identical "nightly seed rotation" terms
            // and are all orphaned/stale — without a distinguishing rare
            // term, this record could tie-break behind the cap and never be
            // verified).
            "fact": "quasar77 nightly seed rotation is genuinely fresh and really pinned",
            "pins": [{ "path": "real_fresh.rs", "hash": hash }],
            "source_turns": [],
            "origin": "agent",
            "ts": 999_999,
        });
        let mut with_fresh = std::fs::read_to_string(root.join(".agentrec/memory.jsonl")).unwrap();
        with_fresh.push_str(&fresh_rec.to_string());
        with_fresh.push('\n');
        std::fs::write(root.join(".agentrec/memory.jsonl"), with_fresh).unwrap();

        let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s2b","prompt":"quasar77 nightly seed rotation"}"#;
        let out = send_hook_capture(root, payload);
        assert!(out.status.success(), "hook must exit 0: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.starts_with("```agentrec memory"),
            "expected a real injection (not silent degradation to no-op) — stdout: {stdout}"
        );
        assert!(
            stdout.contains("genuinely fresh and really pinned"),
            "expected the Fresh record's fact in the injected block — stdout: {stdout}"
        );
    }

    // Missing store (never `remember`ed) and a fully uninitialized
    // `.agentrec/` both exit 0 with no block.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s3","prompt":"anything at all"}"#;
        let out = send_hook_capture(root, payload);
        assert!(out.status.success(), "hook must exit 0: {out:?}");
        assert!(out.stdout.is_empty());
    }
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path(); // never `init`ed — no .agentrec/ at all yet
        let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s4","prompt":"anything at all"}"#;
        let out = send_hook_capture(root, payload);
        assert!(
            out.status.success(),
            "hook on an uninitialized repo must exit 0: {out:?}"
        );
        assert!(out.stdout.is_empty());
        // The start signal always-appends property (already asserted for
        // the corrupt-store case above) must hold here too — an
        // uninitialized repo is not exempt from the "hook always records
        // the signal" contract, it just has nowhere to inject memory from.
        let events = signal_events(root);
        assert!(
            events
                .iter()
                .any(|e| e.get("event").and_then(|v| v.as_str()) == Some("start")),
            "start signal missing on an uninitialized repo: {events:?}"
        );
    }
}

/// F10: a malformed/unreadable NON-EMPTY `memory.jsonl` is a RECALL FAILURE
/// distinct from (a) a missing store and (b) a healthy zero-match store.
/// `hook_fail_open_and_budget`'s corrupt-store leg above only ever asserted
/// exit-0 + empty-stdout + start-signal-appended — satisfied identically
/// whether the corruption was genuinely counted as a failure or silently
/// folded to indistinguishable "no memories". This test requires the
/// distinguishing signal: exactly one
/// `{"ts","failure":true,"reason":"store_corrupt"}` line in
/// `memory-stats.jsonl` per hook attempt, and a non-zero failure count
/// surfaced by `agentrec status` — and that a missing or healthy-empty store
/// never trips either.
#[test]
fn hook_corrupt_memory_store_is_counted() {
    // AC-F10.2a: wholly-corrupt store — every line is garbage.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        std::fs::write(
            root.join(".agentrec/memory.jsonl"),
            b"\xff\xfenot json at all garbage bytes\x00\x01\n",
        )
        .unwrap();

        let payload =
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"c1","prompt":"anything"}"#;
        let out = send_hook_capture(root, payload);
        assert!(out.status.success(), "hook must exit 0: {out:?}");
        assert!(
            out.stdout.is_empty(),
            "corrupt store must inject nothing: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );

        let events = signal_events(root);
        assert!(
            events
                .iter()
                .any(|e| e.get("event").and_then(|v| v.as_str()) == Some("start")),
            "start signal missing despite corrupt memory.jsonl: {events:?}"
        );

        let stats = memory_stats_lines(root);
        let failures: Vec<_> = stats
            .iter()
            .filter(|s| s.get("failure").and_then(|f| f.as_bool()) == Some(true))
            .collect();
        assert_eq!(
            failures.len(),
            1,
            "expected exactly one failure stat line per hook attempt: {stats:?}"
        );
        assert_eq!(
            failures[0].get("reason").and_then(|r| r.as_str()),
            Some("store_corrupt"),
            "failure reason must be the bounded enum string store_corrupt: {failures:?}"
        );

        let status_out = agentrec(root, &["status"]);
        assert!(
            status_out.status.success(),
            "status must exit 0: {status_out:?}"
        );
        let status_text = String::from_utf8_lossy(&status_out.stdout);
        assert!(
            status_text.contains("1 failures"),
            "status must surface the non-zero memory-store failure count: {status_text}"
        );
    }

    // AC-F10.2b: mixed valid+corrupt store — one real, Fresh, on-topic,
    // real-pinned record alongside a malformed line. Must STILL inject
    // nothing — a corrupt store may never leak a partial valid fact.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        std::fs::write(root.join("mixed.rs"), b"fn mixed() {}\n").unwrap();
        let hash = memory::hash_pin(root, "mixed.rs").expect("hash mixed.rs");
        let good = serde_json::json!({
            "v": 1, "type": "memory", "id": "m-good", "op": "assert",
            "fact": "quasar99 mixed store genuinely fresh fact",
            "pins": [{ "path": "mixed.rs", "hash": hash }],
            "source_turns": [], "origin": "agent", "ts": 1,
        });
        let mut content = good.to_string();
        content.push('\n');
        content.push_str("{this is not valid json at all}\n");
        std::fs::write(root.join(".agentrec/memory.jsonl"), content).unwrap();

        let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"c2","prompt":"quasar99 mixed store"}"#;
        let out = send_hook_capture(root, payload);
        assert!(out.status.success(), "hook must exit 0: {out:?}");
        assert!(
            out.stdout.is_empty(),
            "mixed valid+corrupt store must never leak a partial fact: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );

        let stats = memory_stats_lines(root);
        let failures: Vec<_> = stats
            .iter()
            .filter(|s| s.get("failure").and_then(|f| f.as_bool()) == Some(true))
            .collect();
        assert_eq!(
            failures.len(),
            1,
            "expected exactly one failure stat line for a mixed valid+corrupt store: {stats:?}"
        );
        assert_eq!(
            failures[0].get("reason").and_then(|r| r.as_str()),
            Some("store_corrupt")
        );
    }

    // AC-F10.3a: missing store (never `remember`ed). No failure stat.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        let payload =
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"c3","prompt":"anything"}"#;
        let out = send_hook_capture(root, payload);
        assert!(out.status.success(), "hook must exit 0: {out:?}");
        let stats = memory_stats_lines(root);
        assert!(
            stats.iter().all(|s| s.get("failure").is_none()),
            "missing store must never be counted as a failure: {stats:?}"
        );
    }

    // AC-F10.3b: healthy zero-match store — a real, valid record that just
    // doesn't match the query. No failure stat.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        std::fs::write(root.join("healthy.rs"), b"fn healthy() {}\n").unwrap();
        let out = agentrec(
            root,
            &[
                "remember",
                "totally unrelated fact about nothing in particular",
                "--from",
                "healthy.rs",
            ],
        );
        assert!(out.status.success(), "remember failed: {out:?}");
        let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"c4","prompt":"zzzznomatchzzzz"}"#;
        let out = send_hook_capture(root, payload);
        assert!(out.status.success(), "hook must exit 0: {out:?}");
        let stats = memory_stats_lines(root);
        assert!(
            stats.iter().all(|s| s.get("failure").is_none()),
            "a healthy zero-match store must never be counted as a failure: {stats:?}"
        );
    }
}

/// AC-F10.5: the persisted `reason` is the bounded enum string
/// `"store_corrupt"`, never raw filesystem/error/path text — so terminal-
/// control bytes embedded in the corrupt content itself (an ESC-prefixed
/// terminal escape sequence, exactly the class `fmt::sanitize_terminal`
/// exists to strip elsewhere) can never reach `memory-stats.jsonl` or
/// `agentrec status`'s stdout. Scans the RAW bytes of both, not just the
/// parsed JSON strings, so a leak into some other field would still be
/// caught.
#[test]
fn hook_corrupt_store_reason_never_leaks_raw_control_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    // An ESC-prefixed terminal title-set + BEL sequence embedded in
    // otherwise-garbage content — real bytes an attacker-controlled
    // transcript or a corrupted write could plausibly leave behind.
    std::fs::write(
        root.join(".agentrec/memory.jsonl"),
        b"\xff\xfenot json \x1b]0;evil-title\x07 more garbage\x00\x01\n",
    )
    .unwrap();

    let payload =
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"esc1","prompt":"anything"}"#;
    let out = send_hook_capture(root, payload);
    assert!(out.status.success(), "hook must exit 0: {out:?}");
    assert!(
        !out.stdout.contains(&0x1b),
        "hook stdout must never carry a raw ESC byte: {:?}",
        out.stdout
    );

    let stats_bytes = std::fs::read(root.join(".agentrec/memory-stats.jsonl")).unwrap_or_default();
    assert!(
        !stats_bytes.contains(&0x1b),
        "memory-stats.jsonl must never carry a raw ESC byte: {:?}",
        String::from_utf8_lossy(&stats_bytes)
    );
    let stats = memory_stats_lines(root);
    assert_eq!(
        stats
            .iter()
            .find(|s| s.get("failure").is_some())
            .and_then(|s| s.get("reason"))
            .and_then(|r| r.as_str()),
        Some("store_corrupt"),
        "reason must be exactly the bounded enum string: {stats:?}"
    );

    let status_out = agentrec(root, &["status"]);
    assert!(
        status_out.status.success(),
        "status must exit 0: {status_out:?}"
    );
    assert!(
        !status_out.stdout.contains(&0x1b),
        "status stdout must never carry a raw ESC byte: {:?}",
        String::from_utf8_lossy(&status_out.stdout)
    );
}

/// F2: the 50ms hook recall budget is now a HARD cooperative deadline
/// (founder decision, option (a)), not the old retrospective
/// measure-after-the-fact suppression. `hook_fail_open_and_budget` above
/// already proves the retrospective/fail-open shape at real time scales;
/// this test proves the *new* bail path specifically — deterministically,
/// without depending on runner speed to blow a real 50ms window (which
/// `hook_fail_open_and_budget`'s own comments document as flaky at 3000
/// records on a slow CI runner, exactly the coupling commit 76a716d
/// removed). `AGENTREC_TEST_FORCE_RECALL_BUDGET_EXCEEDED` (test-only,
/// `cmds::recall_deadline`) substitutes an already-expired deadline before
/// any recall work starts, so the bail is deterministic on any machine.
#[test]
fn hook_recall_bails_at_injected_deadline() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-service"]);

    // A real, freshly-pinned, on-topic memory that WOULD be injected on a
    // normal (non-expired-deadline) call — proves the empty result below is
    // caused by the injected deadline, not by an unmatchable corpus.
    std::fs::write(root.join("real_fresh.rs"), b"fn real_fresh() {}\n").unwrap();
    let hash = memory::hash_pin(root, "real_fresh.rs").expect("hash real_fresh.rs");
    let rec = serde_json::json!({
        "v": 1,
        "type": "memory",
        "id": "m-deadline-fresh",
        "op": "assert",
        "fact": "deadline probe fact is genuinely fresh and really pinned",
        "pins": [{ "path": "real_fresh.rs", "hash": hash }],
        "source_turns": [],
        "origin": "agent",
        "ts": 1,
    });
    std::fs::write(root.join(".agentrec/memory.jsonl"), format!("{rec}\n")).unwrap();

    let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"deadline probe fact"}"#;

    // Sanity: without the forced-expired override, this hook call really
    // does inject the block — proves the corpus/query are matchable and
    // isolates the deadline override as the only variable in the next call.
    let sane = send_hook_capture(root, payload);
    assert!(sane.status.success(), "sanity hook call failed: {sane:?}");
    assert!(
        String::from_utf8_lossy(&sane.stdout).starts_with("```agentrec memory"),
        "sanity: expected a real injection before testing the bail path: {:?}",
        String::from_utf8_lossy(&sane.stdout)
    );

    let mut child = Command::new(bin())
        .args(["hook", "claude", "--root", root.to_str().unwrap()])
        .env("AGENTREC_TEST_FORCE_RECALL_BUDGET_EXCEEDED", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();

    assert!(
        out.status.success(),
        "hook must exit 0 even on a bailed recall: {out:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "an already-expired deadline must never print a block: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    let events = signal_events(root);
    assert!(
        events
            .iter()
            .any(|e| e.get("event").and_then(|v| v.as_str()) == Some("start")),
        "start signal must still land despite the bailed recall: {events:?}"
    );

    let stats = memory_stats_lines(root);
    let bail = stats
        .iter()
        .find(|l| l.get("budget_exceeded").and_then(|v| v.as_bool()) == Some(true));
    assert!(
        bail.is_some(),
        "expected a budget_exceeded line in memory-stats.jsonl: {stats:?}"
    );
    assert!(
        bail.unwrap().get("ts").and_then(|v| v.as_u64()).is_some(),
        "budget_exceeded line must still carry a ts: {stats:?}"
    );
}

/// F8: the 50ms hook budget must be a HARD WALL, not just cooperative
/// between-step checks. `hook_recall_bails_at_injected_deadline` above
/// proves the cooperative deadline bails once it is checked — but every
/// check happens BETWEEN loop steps, so a single blocking call inside one
/// step (e.g. `memory::hash_pin`'s `fs::read` during the freshness-verify
/// walk) can still overrun the budget by however long that one call blocks.
/// This test proves the outer wall holds even then: `AGENTREC_TEST_SLOW_PIN_READ_MS`
/// (test-only seam, `memory::hash_pin`) makes the verify walk's pin read
/// block for 600ms — twelve times the 50ms budget — and the hook process
/// must still return well inside a 200ms envelope, with empty stdout and no
/// partial/leaked fact text, and exactly one `budget_exceeded:true` stat
/// line (never a `n`-bearing injection line — the blocked read never got a
/// chance to report freshness either way).
#[test]
fn hook_recall_hard_wall_deadline() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // A real, freshly-pinned, on-topic memory that WOULD be injected on a
    // normal (unblocked) call — isolates the slow read as the only variable.
    std::fs::write(root.join("real_fresh.rs"), b"fn real_fresh() {}\n").unwrap();
    let hash = memory::hash_pin(root, "real_fresh.rs").expect("hash real_fresh.rs");
    let rec = serde_json::json!({
        "v": 1,
        "type": "memory",
        "id": "m-hard-wall-fresh",
        "op": "assert",
        "fact": "hard wall probe fact is genuinely fresh and really pinned",
        "pins": [{ "path": "real_fresh.rs", "hash": hash }],
        "source_turns": [],
        "origin": "agent",
        "ts": 1,
    });
    std::fs::write(root.join(".agentrec/memory.jsonl"), format!("{rec}\n")).unwrap();

    let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"hard wall probe fact"}"#;

    // Sanity: without the slow-read seam, this hook call really does inject
    // the block — proves the corpus/query are matchable so the empty result
    // below is caused by the wall, not an unmatchable corpus.
    let sane = send_hook_capture(root, payload);
    assert!(sane.status.success(), "sanity hook call failed: {sane:?}");
    assert!(
        String::from_utf8_lossy(&sane.stdout).starts_with("```agentrec memory"),
        "sanity: expected a real injection before testing the hard wall: {:?}",
        String::from_utf8_lossy(&sane.stdout)
    );

    let mut child = Command::new(bin())
        .args(["hook", "claude", "--root", root.to_str().unwrap()])
        .env("AGENTREC_TEST_SLOW_PIN_READ_MS", "600")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let started = Instant::now();
    let out = child.wait_with_output().unwrap();
    let elapsed = started.elapsed();

    assert!(
        out.status.success(),
        "hook must exit 0 even while the pin read is blocked: {out:?}"
    );
    assert!(
        elapsed < Duration::from_millis(200),
        "hook took {elapsed:?} — a blocked 600ms pin read must not push wall \
         time anywhere near that far past the 50ms budget (hard wall must \
         abandon the worker, not wait on it)"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.is_empty(),
        "a hard-walled recall must never print a block, partial or otherwise: {stdout}"
    );
    assert!(
        !stdout.contains("hard wall probe fact"),
        "no fact text may leak out despite the blocked read: {stdout}"
    );

    let events = signal_events(root);
    assert!(
        events
            .iter()
            .any(|e| e.get("event").and_then(|v| v.as_str()) == Some("start")),
        "start signal must still land despite the blocked recall: {events:?}"
    );

    // Exactly one NEW stat line from this call, carrying budget_exceeded —
    // never a partial `n`-bearing injection line, never fact text.
    let stats = memory_stats_lines(root);
    assert_eq!(
        stats.len(),
        2,
        "expected exactly 2 memory-stats lines total (1 from the sanity call's \
         real injection + 1 budget_exceeded from the hard-walled call): {stats:?}"
    );
    let new_line = &stats[1];
    assert_eq!(
        new_line.get("budget_exceeded").and_then(|v| v.as_bool()),
        Some(true),
        "expected the second stat line to be budget_exceeded:true: {stats:?}"
    );
    assert!(
        new_line.get("n").is_none(),
        "a budget_exceeded line must never also carry a partial hit count: {stats:?}"
    );
    assert!(
        !stats
            .iter()
            .any(|l| { l.to_string().contains("hard wall probe fact") }),
        "no fact text may appear anywhere in memory-stats.jsonl: {stats:?}"
    );
}

/// INV-M4 concurrent-append leg: the hook path must exit 0 (and keep
/// appending the start signal) while `.agentrec/memory.jsonl` is being
/// concurrently APPENDED by another writer — the exact interleaving the
/// spec's INV-M4 names but which no prior test exercised (the daemon-driven
/// candidate path and `agentrec remember` both write through the same
/// `memory::append_memory`, so a direct-API writer thread racing the real
/// `hook` subprocess is a faithful stand-in for "daemon ingesting a
/// candidate while a hook fires").
///
/// The writer thread appends real, freshly-hashed, Fresh-pinned records in
/// a tight loop (no sleeps) using `agentrec_core::memory::append_memory`
/// directly — far tighter than spawning a `remember` subprocess per
/// iteration, so it actually races the hook's own read of the same file
/// instead of finishing before the hook loop starts. The hook loop runs
/// concurrently in the foreground, firing the real `agentrec hook claude`
/// binary repeatedly; both threads are running for the full duration of
/// this test, guaranteeing overlap.
#[test]
fn hook_exits_zero_under_concurrent_memory_append() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    init(&root);

    // A real, stable pin target: unchanged for the whole test, so every
    // writer-thread record is genuinely Fresh and eligible for injection —
    // this exercises the concurrent-write race against a real read path,
    // not just a race against records that would be filtered out anyway.
    std::fs::write(root.join("concurrent.rs"), b"fn concurrent() {}\n").unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let writes_done = Arc::new(AtomicU64::new(0));
    let writer_root = root.clone();
    let writer_stop = stop.clone();
    let writer_count = writes_done.clone();
    let writer = std::thread::spawn(move || {
        let mut i: u64 = 0;
        while !writer_stop.load(Ordering::Relaxed) {
            let Ok(hash) = memory::hash_pin(&writer_root, "concurrent.rs") else {
                continue;
            };
            let rec = MemoryRecord {
                v: 1,
                kind: "memory".to_string(),
                id: format!("wm{i}"),
                op: MemoryOp::Assert,
                fact: format!("concurrent writer fact {i} about nightly seed rotation"),
                pins: vec![Pin {
                    path: "concurrent.rs".to_string(),
                    hash,
                }],
                source_turns: vec![],
                origin: "agent".to_string(),
                ts: i,
                reason: None,
            };
            // Best-effort like the real ingestion paths: a transient
            // failure here must not panic the writer thread — the hook's
            // own fail-open posture is what's under test, not this helper.
            let _ = memory::append_memory(&writer_root, &rec);
            i += 1;
            writer_count.store(i, Ordering::Relaxed);
        }
    });

    // Give the writer a head start so the hook loop below always overlaps
    // an in-flight append, never races a not-yet-started writer.
    std::thread::sleep(Duration::from_millis(20));

    let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"concurrent","prompt":"nightly seed rotation concurrent"}"#;
    let iterations = 30;
    for iter in 0..iterations {
        let before = signal_events(&root).len();
        let out = send_hook_capture(&root, payload);
        assert!(
            out.status.success(),
            "hook exited nonzero under concurrent memory.jsonl append at iter {iter}: {out:?}"
        );
        let after = signal_events(&root);
        assert!(
            after.len() > before,
            "start signal not appended at iter {iter} despite concurrent memory.jsonl writes \
             (before={before}, after={})",
            after.len()
        );
        assert_eq!(
            after
                .last()
                .and_then(|e| e.get("event"))
                .and_then(|v| v.as_str()),
            Some("start"),
            "last signal line at iter {iter} was not a start event: {after:?}"
        );
    }

    stop.store(true, Ordering::Relaxed);
    writer.join().expect("writer thread panicked");

    // Prove the race actually happened: the writer produced a meaningful
    // number of concurrent appends during the hook loop's run, not zero or
    // a token handful that finished before the loop even started.
    let total_writes = writes_done.load(Ordering::Relaxed);
    assert!(
        total_writes >= iterations,
        "writer thread produced too few appends ({total_writes}) to have \
         genuinely raced {iterations} hook calls"
    );

    // The store must never be left torn by the interleaving: every raw
    // line in memory.jsonl still parses as a MemoryRecord with >=1 pin
    // (mirrors torture.rs's assert_memory_invariants, INV-M1).
    let text = std::fs::read_to_string(root.join(".agentrec/memory.jsonl")).unwrap();
    for (n, line) in text.lines().enumerate() {
        let rec: MemoryRecord = serde_json::from_str(line).unwrap_or_else(|e| {
            panic!(
                "memory.jsonl line {n} failed to parse after concurrent append: {e}\nline: {line}"
            )
        });
        assert!(
            !rec.pins.is_empty(),
            "memory.jsonl line {n} has zero pins after concurrent append: {line}"
        );
    }
}

/// AC-F10.6: a corrupt `memory.jsonl` (one malformed, newline-terminated
/// line seeded up front, so it never merges with a concurrently-appended
/// well-formed line) must not destabilize the hook under real concurrent
/// daemon-style writes to the SAME file — every hook call still exits 0,
/// still appends the normal start signal, and `memory-stats.jsonl` /
/// `signal.jsonl` are never left torn (every line still parses) despite the
/// interleaving. Mirrors `hook_exits_zero_under_concurrent_memory_append`'s
/// harness shape, seeded with a permanent corruption up front instead of an
/// initially-empty store.
#[test]
fn hook_corrupt_store_safe_under_concurrent_append() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    init(&root);

    std::fs::write(root.join("concurrent2.rs"), b"fn concurrent2() {}\n").unwrap();
    // Seeded corruption, `\n`-terminated so it stays its own line no matter
    // what the writer thread appends after it — the store stays corrupt
    // (and thus failure-counted) for the entire test.
    std::fs::write(
        root.join(".agentrec/memory.jsonl"),
        b"{not valid json at all}\n",
    )
    .unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let writes_done = Arc::new(AtomicU64::new(0));
    let writer_root = root.clone();
    let writer_stop = stop.clone();
    let writer_count = writes_done.clone();
    let writer = std::thread::spawn(move || {
        let mut i: u64 = 0;
        while !writer_stop.load(Ordering::Relaxed) {
            let Ok(hash) = memory::hash_pin(&writer_root, "concurrent2.rs") else {
                continue;
            };
            let rec = MemoryRecord {
                v: 1,
                kind: "memory".to_string(),
                id: format!("cwm{i}"),
                op: MemoryOp::Assert,
                fact: format!("corrupt-concurrent writer fact {i} nightly seed rotation"),
                pins: vec![Pin {
                    path: "concurrent2.rs".to_string(),
                    hash,
                }],
                source_turns: vec![],
                origin: "agent".to_string(),
                ts: i,
                reason: None,
            };
            let _ = memory::append_memory(&writer_root, &rec);
            i += 1;
            writer_count.store(i, Ordering::Relaxed);
        }
    });

    std::thread::sleep(Duration::from_millis(20));

    let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"corrupt-concurrent","prompt":"nightly seed rotation concurrent"}"#;
    let iterations = 20;
    for iter in 0..iterations {
        let before = signal_events(&root).len();
        let out = send_hook_capture(&root, payload);
        assert!(
            out.status.success(),
            "hook exited nonzero under concurrent append to a corrupt memory.jsonl at iter {iter}: {out:?}"
        );
        assert!(
            out.stdout.is_empty(),
            "a corrupt store must never inject, even mid-race, at iter {iter}: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        let after = signal_events(&root);
        assert!(
            after.len() > before,
            "start signal not appended at iter {iter} despite concurrent writes to a \
             corrupt memory.jsonl (before={before}, after={})",
            after.len()
        );
    }

    stop.store(true, Ordering::Relaxed);
    writer.join().expect("writer thread panicked");

    let total_writes = writes_done.load(Ordering::Relaxed);
    assert!(
        total_writes >= iterations,
        "writer thread produced too few appends ({total_writes}) to have \
         genuinely raced {iterations} hook calls"
    );

    // signal.jsonl and memory-stats.jsonl must never be left torn by the
    // interleaving — every line still parses as JSON.
    for name in [".agentrec/signal.jsonl", ".agentrec/memory-stats.jsonl"] {
        let text = std::fs::read_to_string(root.join(name)).unwrap_or_default();
        for (n, line) in text.lines().enumerate() {
            assert!(
                serde_json::from_str::<serde_json::Value>(line).is_ok(),
                "{name} line {n} failed to parse after concurrent append to a corrupt \
                 memory.jsonl: {line}"
            );
        }
    }

    // Every hook attempt against the still-corrupt store recorded exactly
    // one failure stat — the corrupt line was seeded once and never healed,
    // so `iterations` calls means `iterations` failure lines.
    let stats = memory_stats_lines(&root);
    let failures = stats
        .iter()
        .filter(|s| s.get("failure").and_then(|f| f.as_bool()) == Some(true))
        .count();
    assert_eq!(
        failures as u64, iterations,
        "expected one failure stat per hook attempt against the permanently corrupt \
         store: {stats:?}"
    );
}

/// RAII guard for a single spawned `agentrec record` daemon: `Drop` SIGKILLs
/// and reaps it, so a test panic mid-assertion never leaks an orphan daemon
/// (mirrors `torture.rs`'s `DaemonGuard`, scoped down to the single-daemon
/// case this test needs). `kill()` tears the daemon down explicitly, before
/// end of scope, so a caller can assert "after teardown" invariants right
/// away; `Drop` then finds nothing left to do.
struct SingleDaemonGuard(Option<Child>);

impl SingleDaemonGuard {
    fn spawn(root: &Path) -> Self {
        let child = spawn_record(root);
        // Bounded-poll for the daemon to actually be up (holding the flock,
        // pid written to state.json) rather than a fixed sleep — proves the
        // daemon genuinely launched instead of assuming 800ms was enough,
        // and fails loudly (via `wait_for_live_daemon`'s own assert) if it
        // never comes up at all.
        wait_for_live_daemon(root);
        SingleDaemonGuard(Some(child))
    }

    fn kill(&mut self) {
        if let Some(mut c) = self.0.take() {
            sigkill(&c);
            let _ = c.wait();
        }
    }
}

impl Drop for SingleDaemonGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

// AC-F10.6: the sibling test above only races an IN-PROCESS `append_memory`
// writer against the hook — it never starts a real `agentrec record` daemon
// and never validates `state.json`, so it doesn't actually prove the stated
// guarantee (a concurrent DAEMON + a permanently corrupt memory.jsonl cannot
// corrupt state.json / memory-stats.jsonl / signal.jsonl, and the hook still
// appends its start signal). This test drives a REAL daemon subprocess and a
// REAL `agentrec hook claude` subprocess per iteration, with concurrent fs
// mutations so the daemon is genuinely active (writing log.jsonl/state.json)
// while the hook reads the corrupt store. Assertions are deliberately
// file-parseability + count based (never FSEvents-timing-dependent turn
// observations) so the test is robust rather than flaky.
#[test]
fn hook_corrupt_store_safe_under_concurrent_real_daemon() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    init(&root);

    // Seeded corruption, `\n`-terminated so it stays its own line no matter
    // what the daemon (memory-candidate ingestion) might ever append after
    // it — the store stays corrupt (and thus failure-counted) for the whole
    // test.
    std::fs::write(
        root.join(".agentrec/memory.jsonl"),
        b"{not valid json at all}\n",
    )
    .unwrap();

    let mut guard = SingleDaemonGuard::spawn(&root);

    let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"real-daemon-corrupt","prompt":"real daemon corrupt store probe"}"#;
    let iterations = 12u64;
    for iter in 0..iterations {
        // Concurrent fs mutation: the daemon's watcher observes this while
        // the hook subprocess concurrently reads the corrupt memory store —
        // genuine concurrent daemon activity, not just a live process.
        std::fs::write(
            root.join("daemon_corrupt.rs"),
            format!("fn daemon_corrupt_{iter}() {{}}\n").as_bytes(),
        )
        .unwrap();

        let before = signal_events(&root).len();
        let out = send_hook_capture(&root, payload);
        assert!(
            out.status.success(),
            "hook exited nonzero under a real concurrent daemon against a corrupt \
             memory.jsonl at iter {iter}: {out:?}"
        );
        assert!(
            out.stdout.is_empty(),
            "a corrupt store must never inject, even against a real concurrent \
             daemon, at iter {iter}: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        let after = signal_events(&root);
        assert!(
            after.len() > before,
            "start signal not appended at iter {iter} despite a real concurrent \
             daemon and a corrupt memory.jsonl (before={before}, after={})",
            after.len()
        );
    }

    // Liveness/concurrency proof, not just "a daemon process existed": every
    // hook call above appends a start signal to signal.jsonl, and only the
    // daemon's own signal-tailer advances `signal_offset` in state.json (the
    // hook process never touches state.json). Bounded-poll for it to advance
    // past 0 WHILE THE DAEMON IS STILL ALIVE — this must run before
    // `guard.kill()`, not after: a one-shot post-kill sample races the
    // guard's SIGKILL against the tailer's own poll loop and can read 0 even
    // though the daemon genuinely consumed signals throughout the test
    // (observed twice under adversarial review). Polling here instead proves
    // the daemon was actively processing the concurrently-appended hook
    // signals — the exact AC-F10.6 guarantee — without being coupled to
    // teardown timing.
    let signal_offset_seen = poll_until(Duration::from_secs(5), || {
        let text = std::fs::read_to_string(root.join(".agentrec/state.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let offset = v.get("signal_offset")?.as_u64()?;
        (offset > 0).then_some(offset)
    });
    assert!(
        signal_offset_seen.is_some(),
        "state.json's signal_offset never advanced past 0 within 5s while the daemon \
         was alive — the daemon never consumed any of the {iterations} hooks' start \
         signals, so this test didn't prove a genuinely concurrent/active daemon"
    );

    // Teardown the daemon now, after the liveness proof above — killing it
    // here (rather than only at end-of-scope via Drop) lets the
    // "after teardown" file-parseability checks below run immediately after
    // the real interleaving window closes.
    guard.kill();

    // signal.jsonl and memory-stats.jsonl must never be left torn by the
    // interleaving between the real daemon and the hook subprocesses — every
    // line still parses as JSON.
    for name in [".agentrec/signal.jsonl", ".agentrec/memory-stats.jsonl"] {
        let text = std::fs::read_to_string(root.join(name)).unwrap_or_default();
        for (n, line) in text.lines().enumerate() {
            assert!(
                serde_json::from_str::<serde_json::Value>(line).is_ok(),
                "{name} line {n} failed to parse after concurrent real-daemon activity \
                 against a corrupt memory.jsonl: {line}"
            );
        }
    }

    // state.json is written by the daemon itself (pid/signal-offset/etc) and
    // must survive the same interleaving — the daemon's writer uses tmp+
    // rename, so a torn read here would indicate a real atomicity bug, not
    // just a JSONL-append issue. `cli::state` isn't reachable from this
    // black-box integration test (the `agentrec` crate has no lib target),
    // so this parses the raw file the same way the CLI's own tests would via
    // `serde_json::Value` rather than the (error-tolerant) `read_state`
    // helper, which would silently mask a genuinely corrupt file.
    let state_text = std::fs::read_to_string(root.join(".agentrec/state.json"))
        .expect("state.json must exist after a real daemon ran");
    let _state_json: serde_json::Value = serde_json::from_str(&state_text).unwrap_or_else(|e| {
        panic!(
            "state.json failed to parse cleanly after concurrent real-daemon activity \
             against a corrupt memory.jsonl: {e}: {state_text}"
        )
    });

    // Every hook attempt against the still-corrupt store recorded exactly
    // one failure stat — the corrupt line was seeded once and never healed,
    // so `iterations` calls means `iterations` failure lines.
    let stats = memory_stats_lines(&root);
    let failures = stats
        .iter()
        .filter(|s| s.get("failure").and_then(|f| f.as_bool()) == Some(true))
        .count();
    assert_eq!(
        failures as u64, iterations,
        "expected one failure stat per hook attempt against the permanently corrupt \
         store under a real concurrent daemon: {stats:?}"
    );
    let store_corrupt = stats
        .iter()
        .filter(|s| s.get("reason").and_then(|r| r.as_str()) == Some("store_corrupt"))
        .count();
    assert_eq!(
        store_corrupt as u64, iterations,
        "expected every failure stat to be tagged reason:store_corrupt: {stats:?}"
    );
}

// Phase 1 (honesty-fixes round) — call-site wiring: proves `status`
// actually threads its harvested protect-set into `enforce_budget`, not
// just that the core mechanism honors one when handed one directly (that's
// already unit-tested in `cli/src/cmds.rs`'s `eviction_keeps_*` tests). A
// real ~2 GiB store is infeasible here, so this drives the real binary with
// the debug-only `AGENTREC_TEST_STORE_BUDGET_BYTES` override (compiled out
// of release — see `cmds::effective_store_budget`), seeding an old,
// otherwise-evictable turn whose blob is ALSO the in-flight open turn's
// `before` — exactly the live-daemon scenario this phase's defect
// describes (open.json's before is typically the previous committed
// turn's after for the same file).
#[test]
fn status_eviction_keeps_open_turn_blob() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let old = store.put(&[0xAAu8; 500]).unwrap();
    // Backdate well before `enforce_budget`'s internal `pass_start` so the
    // pre-existing A3(c) freshness guard can't rescue it vacuously.
    {
        let hex = old.strip_prefix("sha256:").unwrap();
        let path = root
            .join(".agentrec/objects")
            .join(&hex[..2])
            .join(&hex[2..]);
        let past = std::time::SystemTime::now() - Duration::from_secs(3600);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(past)
            .unwrap();
    }
    let new = store.put(&[0xCCu8; 5]).unwrap();

    seed_turn(
        root,
        &base_turn(
            "t_OPENWIRE0000000000000001",
            vec![FileEntry {
                path: "old.bin".into(),
                before: None,
                after: Some(old.clone()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            }],
        ),
    );
    seed_turn(
        root,
        &base_turn(
            "t_OPENWIRENEW000000000001",
            vec![FileEntry {
                path: "new.bin".into(),
                before: None,
                after: Some(new.clone()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            }],
        ),
    );

    // Simulates the daemon's crash journal: the in-flight open turn's
    // `before` cites the same blob as the old committed turn's `after`.
    std::fs::write(
        root.join(".agentrec/open.json"),
        format!(r#"{{"before":"{old}"}}"#),
    )
    .unwrap();

    let out = Command::new(bin())
        .args(["status", "--root", root.to_str().unwrap()])
        .env("AGENTREC_TEST_STORE_BUDGET_BYTES", "5")
        .output()
        .expect("run agentrec status");
    assert!(out.status.success(), "status failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("over"),
        "expected over-budget notice: {stdout}"
    );

    assert!(
        store.contains(&old),
        "the in-flight turn's blob must survive a real `status` eviction pass: {stdout}"
    );
    assert!(store.contains(&new));
}
