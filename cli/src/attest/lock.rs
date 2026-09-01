//! Dedicated advisory lock guarding appends to `.agentrec/attest.jsonl`.
//!
//! **Every writer takes this lock, the daemon included.** That is the one
//! deliberate difference from `loglock.rs`, whose module doc exempts the
//! daemon's steady `persist` append because for `log.jsonl` the daemon is the
//! SOLE steady writer. `attest.jsonl` has multiple routine writers: CLI
//! commands append `evidence` / `verdict` / `human` / `manual-declare`, and
//! the daemon appends `stale` (`ATTEST-FORMAT.md` § "`.agentrec/attest.jsonl`"
//! — "this file has multiple routine writers ... Every writer — the daemon
//! included — will take the append lock"). Copying loglock's exemption would
//! let the daemon's `stale` append interleave with a CLI `evidence` append and
//! tear a line. Phase 4 wires the daemon's writer through this same function.
//!
//! A SEPARATE lock file from `daemon.lock`, for the reason `memlock.rs`
//! records: `daemon.lock` is held for the daemon's entire lifetime, so a
//! writer blocking on it would wait forever rather than for one append.
//! `attest.lock` is only ever held for the duration of a single append.
//!
//! There is no non-blocking acquire here and no rewrite class for
//! `attest.jsonl` (`ATTEST-FORMAT.md`: adding one requires a decision-register
//! entry), so the `try_acquire` half of `memlock.rs` has no caller to serve.

// The append path has no production caller until chunk B's `attest derive` /
// `attest run` land (this chunk ships the reader, `attest status`). Kept
// allowed rather than deleted: the plan assigns the first-writer wiring here.
#![allow(dead_code)]

use agentrec_core::attest::events::{parse_log, AttestEvent, AttestParseCensus};
use std::fs::File;
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

pub fn attest_path(root: &Path) -> PathBuf {
    crate::agentrec_dir(root).join("attest.jsonl")
}

fn lock_path(root: &Path) -> PathBuf {
    crate::agentrec_dir(root).join("attest.lock")
}

fn open_lock_file(root: &Path) -> Result<File, String> {
    let path = lock_path(root);
    // Write-side fsguard mirror (the read half is clippy-enforced, the write
    // half is not): a FIFO here blocks the open until a reader appears,
    // wedging every attest writer before the flock is even attempted.
    if agentrec_core::fsguard::is_nonregular(&path) {
        return Err(format!(
            "{} is not a regular file — refusing to lock",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        // Never truncate: this file's only role is to be `flock`'d.
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("cannot open lock file {}: {e}", path.display()))?;
    agentrec_core::perms::lock_file(&path);
    Ok(file)
}

/// Blocking acquire: a real OS `flock` wait, not a poll loop. No append is
/// ever dropped for contention — it waits its turn.
fn acquire_blocking(root: &Path) -> Result<File, String> {
    let file = open_lock_file(root)?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        return Err(format!(
            "could not lock {}: {}",
            lock_path(root).display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(file)
}

/// The single choke point for every `attest.jsonl` append. Serializes the
/// whole batch first, then acquires the lock, then does ONE `write_all` and
/// fsyncs — so a batch is all-or-nothing against concurrent appenders and a
/// serialization failure never leaves a half-written batch behind.
pub fn append_attest_locked(root: &Path, events: &[AttestEvent]) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }
    let mut buf = String::new();
    for event in events {
        let line = serde_json::to_string(event).map_err(|e| format!("serialize event: {e}"))?;
        buf.push_str(&line);
        buf.push('\n');
    }

    let path = attest_path(root);
    let _lock = acquire_blocking(root)?;
    // Same write-side guard as the lock file: a FIFO at `attest.jsonl` would
    // block this open forever.
    if agentrec_core::fsguard::is_nonregular(&path) {
        return Err(format!(
            "{} is not a regular file — refusing to append",
            path.display()
        ));
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    // umask-independent (D37): OpenOptions' create mode is masked by umask.
    agentrec_core::perms::lock_file(&path);
    file.write_all(buf.as_bytes())
        .map_err(|e| format!("cannot append to {}: {e}", path.display()))?;
    file.sync_all()
        .map_err(|e| format!("cannot fsync {}: {e}", path.display()))?;
    Ok(())
    // `_lock` drops here, releasing the flock.
}

/// Tolerant read of the whole log. A missing file is an EMPTY log, not an
/// error — nothing has attested yet is a normal state, not a failure.
pub fn read_attest(root: &Path) -> Result<(Vec<AttestEvent>, AttestParseCensus), String> {
    let path = attest_path(root);
    match agentrec_core::fsguard::read_regular_to_string(&path) {
        Ok(body) => Ok(parse_log(&body)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok((Vec::new(), AttestParseCensus::default()))
        }
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

#[cfg(test)]
// Test code reads its own tempdir fixtures; no attacker-supplied FIFO can
// block these. Production reads stay lint-enforced (clippy.toml).
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use agentrec_core::attest::types::{ClaimId, TestIdentity, TestOutcome};

    fn derive_event(ts: u64) -> AttestEvent {
        AttestEvent::Derive {
            ts,
            claim_id: ClaimId::mint_with(ts, &[0u8; 10]),
            test_identity: TestIdentity::new("t", "f"),
            body_hash: None,
            renamed_from: None,
        }
    }

    #[test]
    fn a_missing_log_reads_as_empty_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let (events, census) = read_attest(tmp.path()).unwrap();
        assert!(events.is_empty());
        assert_eq!(census, AttestParseCensus::default());
    }

    #[test]
    fn an_append_round_trips_through_the_tolerant_reader() {
        let tmp = tempfile::tempdir().unwrap();
        let evs = vec![derive_event(1), derive_event(2)];
        append_attest_locked(tmp.path(), &evs).unwrap();
        append_attest_locked(tmp.path(), &[]).unwrap(); // empty batch is a no-op
        let (read, census) = read_attest(tmp.path()).unwrap();
        assert_eq!(read, evs);
        assert_eq!(census.events, 2);
        assert_eq!(census.unparsed_lines, 0);
        // Append-only: a second append must not truncate the first.
        append_attest_locked(tmp.path(), &[derive_event(3)]).unwrap();
        assert_eq!(read_attest(tmp.path()).unwrap().0.len(), 3);
    }

    /// `open_lock_file` opens `.agentrec/attest.lock` for WRITE before the
    /// flock is attempted, so a FIFO planted there would block every attest
    /// writer. HANGS rather than fails on regression: terminating is the
    /// property.
    #[test]
    #[cfg(unix)]
    fn a_fifo_lock_path_is_refused_instead_of_hanging() {
        let tmp = tempfile::tempdir().unwrap();
        let path = lock_path(tmp.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);

        let err = append_attest_locked(tmp.path(), &[derive_event(1)]).unwrap_err();
        assert!(err.contains("not a regular file"), "{err}");

        // ALLOW half: an ordinary path must still append, or a
        // refuse-everything guard would pass the assert above.
        let ok = tempfile::tempdir().unwrap();
        append_attest_locked(ok.path(), &[derive_event(1)]).expect("ordinary path must append");
    }

    /// AC-ATTEST-P3-5 — the Phase 4 concurrent-append AC, landed with the
    /// lock. N threads x K events, ~64 KiB of padding per event.
    ///
    /// HONESTY RIDER, measured: this test does NOT by itself prove the lock
    /// works. Neutering `append_attest_locked` (acquire the lock, then
    /// `drop` it immediately, so appends race) leaves it GREEN — probed on
    /// macOS/APFS at 256 KiB, 2 MiB and 32 MiB batches. `write_all` to an
    /// `O_APPEND` fd did not short-write and interleave at any size tried, so
    /// no batch size available here makes this shape discriminating. It
    /// pins the OUTCOME (every event survives, nothing is torn);
    /// `an_append_blocks_while_another_writer_holds_the_lock` below is the
    /// discriminating half. Isolated neuter — replacing the `libc::flock`
    /// call in `acquire_blocking` with `let rc = 0`, so the lock FILE is
    /// still opened and `.agentrec/` still created — reds that test alone
    /// (1 failed / 5 passed) and leaves this one green.
    ///
    /// Threads (not processes) are the right vehicle because `flock` locks
    /// the open file description: two threads each calling `open_lock_file`
    /// hold two descriptions and do contend.
    #[test]
    fn ac_p3_5_concurrent_appenders_leave_no_torn_lines_phase4_concurrent_append_ac() {
        use agentrec_core::attest::events::ManualSeverity;

        const THREADS: u64 = 6;
        const PER_THREAD: u64 = 4;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let handles: Vec<_> = (0..THREADS)
            .map(|t| {
                let root = root.clone();
                std::thread::spawn(move || {
                    let events: Vec<AttestEvent> = (0..PER_THREAD)
                        .map(|k| {
                            let n = t * PER_THREAD + k + 1;
                            AttestEvent::ManualDeclare {
                                ts: n,
                                claim_id: ClaimId::mint_with(n, &[0u8; 10]),
                                text: "x".repeat(64 * 1024),
                                severity: ManualSeverity::Blocking,
                            }
                        })
                        .collect();
                    append_attest_locked(&root, &events).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let (events, census) = read_attest(&root).unwrap();
        assert_eq!(census.unparsed_lines, 0, "torn line(s) in attest.jsonl");
        assert_eq!(census.unknown_kind_lines, 0);
        assert_eq!(events.len() as u64, THREADS * PER_THREAD);
    }

    /// The discriminating half of AC-ATTEST-P3-5: while one writer holds the
    /// lock, another writer's append must WAIT. Under the release-immediately
    /// neuter the waiting append completes at once and this reds.
    ///
    /// Timing-based, with a 10x margin between the 50 ms "must still be
    /// waiting" check and the 500 ms hold.
    #[test]
    fn an_append_blocks_while_another_writer_holds_the_lock() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        // Create `.agentrec/` first so the waiter's only obstacle is the lock.
        append_attest_locked(&root, &[derive_event(1)]).unwrap();

        let held = acquire_blocking(&root).unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let handle = {
            let (root, done) = (root.clone(), Arc::clone(&done));
            std::thread::spawn(move || {
                append_attest_locked(&root, &[derive_event(2)]).unwrap();
                done.store(true, Ordering::SeqCst);
            })
        };

        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !done.load(Ordering::SeqCst),
            "the second append must still be waiting on the lock"
        );
        drop(held);
        handle.join().unwrap();
        assert!(done.load(Ordering::SeqCst));
        assert_eq!(read_attest(&root).unwrap().0.len(), 2);
    }

    #[test]
    fn evidence_events_serialize_to_the_documented_wire_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let id = ClaimId::parse("c_00000000010W3GE1R70W3GE1R7").unwrap();
        let ev = AttestEvent::Evidence {
            ts: 3,
            claim_id: id,
            turn_id: Some("t_01ARZ3NDEKTSV4RRFFQ69G5FAV".into()),
            dirty: true,
            output_blob: Some("deadbeef".into()),
            result: agentrec_core::attest::types::StructuredResult::outcome(
                TestIdentity::new("agentrec--import_claude", "ac3_zero_bytes"),
                TestOutcome::Passed,
            ),
        };
        append_attest_locked(tmp.path(), std::slice::from_ref(&ev)).unwrap();
        let body = std::fs::read_to_string(attest_path(tmp.path())).unwrap();
        assert!(body.starts_with(r#"{"kind":"evidence","ts":3,"#), "{body}");
        assert!(body.ends_with('\n'));
        assert_eq!(read_attest(tmp.path()).unwrap().0, vec![ev]);
    }
}
