//! Dedicated advisory lock guarding `log.jsonl` against the one race
//! `purge --log-duplicates`'s length-recheck can't fully close: a concurrent
//! `undo --confirm` append landing in the microseconds between purge's
//! recheck and its atomic rename (review finding #3). The recheck aborts on
//! any pre-recheck growth, but a write in that final sub-recheck window would
//! be clobbered by the rewrite — lost from both `log.jsonl` AND the archive,
//! violating "never delete user data".
//!
//! Scope, deliberately narrow (mirrors `memlock.rs`'s F4 posture):
//! - `undo --confirm` (the only NON-daemon `log.jsonl` writer) takes this lock
//!   BLOCKING around its append. If a purge is mid-rewrite it simply waits,
//!   and its append lands right after the rewrite completes — never lost.
//! - `purge --log-duplicates` takes it NONBLOCKING (`try_acquire`) and holds
//!   it across its whole read -> archive -> recheck -> rewrite -> rename
//!   sequence. If it can't acquire (an undo is mid-append) it refuses loudly
//!   rather than proceeding without the lock.
//!
//! The daemon's steady `persist` append does NOT take this lock (it is a
//! per-turn hot path, and `purge --log-duplicates` already refuses outright
//! while the daemon holds `daemon.lock`, so the daemon never appends during a
//! purge). The length-recheck stays as belt-and-suspenders for a daemon that
//! *starts* after purge's liveness check — that writer, by design, does not
//! route through here.
//!
//! A SEPARATE lock file from `daemon.lock` for the same reason `memory.lock`
//! is: `daemon.lock` is held for the daemon's entire lifetime, so blocking on
//! it would deadlock against a live daemon, not wait out one append.

use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

fn lock_path(root: &Path) -> PathBuf {
    crate::agentrec_dir(root).join("log.lock")
}

fn open_lock_file(root: &Path) -> Result<File, String> {
    let path = lock_path(root);
    // Write-side fsguard mirror: a FIFO here blocks the open until a reader
    // appears, wedging every log writer before the flock is even attempted.
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

/// Blocking acquire, for `undo --confirm`: waits (real OS `flock` wait) until
/// any in-progress purge releases the lock, then holds `LOCK_EX` until the
/// returned `File` drops.
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

/// Non-blocking acquire, for `purge --log-duplicates`: refuses loudly instead
/// of blocking or proceeding unlocked. Purge must never hang behind a writer
/// indefinitely, and must never rewrite `log.jsonl` without holding this lock
/// the whole time. The returned `File` must be kept alive for the entire
/// archive+rewrite sequence.
pub fn try_acquire(root: &Path) -> Result<File, String> {
    let file = open_lock_file(root)?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(format!(
            "could not acquire {} — an undo is writing log.jsonl, try again",
            lock_path(root).display()
        ));
    }
    Ok(file)
}

/// Single choke point for `undo --confirm`'s `log.jsonl` appends (both the
/// success turn and the partial-abort turn): acquire the lock, append,
/// release. Routing both call sites through one function means a future new
/// undo append path can't forget to take the lock.
pub fn append_log_locked(
    log_path: &Path,
    rec: &agentrec_core::record::LogRecord,
) -> Result<(), String> {
    let root = log_path
        .parent() // .agentrec/
        .and_then(|p| p.parent()) // repo root
        .ok_or_else(|| "log.jsonl has no repo root".to_string())?;
    let _lock = acquire_blocking(root)?;
    agentrec_core::record::append_log(log_path, rec)
    // `_lock` drops here (end of scope), releasing the flock.
}

#[cfg(test)]
// Test code reads its own tempdir fixtures; no attacker-supplied FIFO can
// block these, so the fsguard wrappers buy nothing. Scoped to this module
// so production reads in this file stay lint-enforced (clippy.toml).
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    /// `open_lock_file` opens `.agentrec/log.lock` for WRITE before the flock
    /// is attempted, so a FIFO planted there blocks every `log.jsonl` writer
    /// (`undo --confirm`'s appends) and `purge --log-duplicates` alike —
    /// indefinitely, with no timeout and no error.
    /// HANGS rather than fails on regression: terminating is the property.
    #[test]
    #[cfg(unix)]
    fn open_lock_file_refuses_a_fifo_lock_instead_of_hanging() {
        let tmp = tempfile::tempdir().unwrap();
        let path = lock_path(tmp.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(
            unsafe { libc::mkfifo(c.as_ptr(), 0o600) },
            0,
            "fixture must actually create a fifo"
        );

        let err = open_lock_file(tmp.path()).unwrap_err();
        assert!(
            err.contains("not a regular file"),
            "refusal must name the reason: {err}"
        );
        // Reached through the public entry point too, not just the primitive.
        assert!(try_acquire(tmp.path())
            .unwrap_err()
            .contains("not a regular file"));

        // ALLOW half: an ordinary lock path must still be acquirable, or a
        // refuse-everything guard would pass the asserts above while silently
        // blocking every log writer.
        let ok = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(crate::agentrec_dir(ok.path())).unwrap();
        drop(try_acquire(ok.path()).expect("an ordinary lock path must still be acquirable"));
    }
}
