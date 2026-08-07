//! Dedicated advisory lock (F4) guarding `.agentrec/memory.jsonl`'s
//! single-writer discipline across `remember`/`verify`/`forget`/the daemon's
//! `ingest_candidate`, and `purge --memories-retracted`'s sanctioned rewrite.
//!
//! A SEPARATE lock file from `daemon.lock` (`daemon.rs`'s D2 lock) —
//! `daemon.lock` is held by the `record` daemon for its ENTIRE lifetime
//! (`daemon::acquire_lock`'s `_lock` binding lives until `run()` returns), so
//! a writer blocking on `daemon.lock` would deadlock against a live daemon
//! forever, not just for the duration of one append. `memory.lock` is only
//! ever held for the duration of a single append or a single purge rewrite,
//! by design.
//!
//! Contention shape (no lock-ordering cycle — every caller takes ONLY this
//! one lock):
//! - every writer (`append_memory_locked`) takes it BLOCKING — if a purge is
//!   mid-rewrite, the writer simply waits its turn, and its append lands
//!   right after the rewrite completes. No append is ever silently lost.
//! - `purge --memories-retracted` takes it NONBLOCKING (`try_acquire`) and
//!   holds it across its entire read -> archive -> rewrite -> rename
//!   sequence. If it can't acquire (another writer or another purge already
//!   holds it), it refuses loudly rather than proceeding without the lock or
//!   blocking indefinitely.

use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

fn lock_path(root: &Path) -> PathBuf {
    crate::agentrec_dir(root).join("memory.lock")
}

fn open_lock_file(root: &Path) -> Result<File, String> {
    let path = lock_path(root);
    // Write-side fsguard mirror: a FIFO here blocks the open until a reader
    // appears, wedging every memory writer before the flock is even attempted.
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
        // Never truncate: this file's only role is to be `flock`'d, same
        // rationale as `daemon.rs::acquire_lock`.
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("cannot open lock file {}: {e}", path.display()))?;
    agentrec_core::perms::lock_file(&path);
    Ok(file)
}

/// Blocking acquire, for writers: waits (real OS `flock` wait, not a poll
/// loop) until any in-progress purge releases the lock, then holds
/// `LOCK_EX` until the returned `File` drops.
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

/// Non-blocking acquire, for `purge --memories-retracted`: refuses loudly
/// instead of blocking or proceeding unlocked. Purge must never hang behind
/// a writer indefinitely, and must never rewrite `memory.jsonl` without
/// holding this lock the whole time.
pub fn try_acquire(root: &Path) -> Result<File, String> {
    let file = open_lock_file(root)?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(format!(
            "could not acquire {} — another memory write is in progress, try again",
            lock_path(root).display()
        ));
    }
    Ok(file)
}

/// Single choke point for every `append_memory` call site (`remember`,
/// `verify`, `forget`, the daemon's `ingest_candidate`): acquire the lock,
/// append, release. Routing every writer through this one function (rather
/// than each call site pairing its own acquire/append) means a future new
/// writer can't forget to take the lock.
pub fn append_memory_locked(
    root: &Path,
    rec: &agentrec_core::memory::MemoryRecord,
) -> Result<(), String> {
    let _lock = acquire_blocking(root)?;
    agentrec_core::memory::append_memory(root, rec)
    // `_lock` drops here (end of scope), releasing the flock.
}

#[cfg(test)]
// Test code reads its own tempdir fixtures; no attacker-supplied FIFO can
// block these, so the fsguard wrappers buy nothing. Scoped to this module
// so production reads in this file stay lint-enforced (clippy.toml).
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    /// `open_lock_file` opens `.agentrec/memory.lock` for WRITE before the
    /// flock is attempted, so a FIFO planted there blocks every memory writer
    /// (`remember`/`verify`/`forget`, the daemon's `ingest_candidate`) and
    /// `purge --memories-retracted` alike.
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
        // blocking every memory writer.
        let ok = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(crate::agentrec_dir(ok.path())).unwrap();
        drop(try_acquire(ok.path()).expect("an ordinary lock path must still be acquirable"));
    }
}
