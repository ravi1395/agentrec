//! Blocking-I/O guards: open and read only ORDINARY files.
//!
//! Every function here exists for one reason. `std::fs::read` and
//! `File::open` on a FIFO **block until a peer appears** — indefinitely, with
//! no timeout and no error. Anywhere agentrec reads a path an untrusted party
//! can create, that is a hang, and for the single-threaded stdio MCP server a
//! hang is the whole agent-facing surface dying for that session.
//!
//! The class was found four times across two branch-review rounds, one site
//! per round, each fix correct and each insufficient — `inode_refusal` in the
//! planner, then `read_current_hash`'s lstat, then this. Patching the site
//! the last probe happened to hit does not converge, because the property is
//! not "this path is safe" but "we never open something that is not a
//! regular file". These helpers make that property expressible once, so a new
//! call site inherits it instead of having to remember it.
//!
//! **Three deliberate exemptions**, each justified rather than overlooked.
//! (1) Character devices agentrec opens BY NAME and by design —
//! `/dev/urandom` in `id.rs` — must not be guarded: `read_regular` would
//! refuse them, and refusing the entropy source is a worse failure than the
//! hang this module prevents. (2) Fixed system paths under `/proc`, which no
//! local party can replace. (3) `File::open` on a DIRECTORY for fsync
//! (`store.rs`): opening a real directory cannot block. That third one
//! carries an assumption worth stating, because "this path is the type I
//! expect" is the exact assumption class that produced this whole series —
//! it holds only while the parent really is a directory, and a FIFO
//! pre-created at a fan-out path would make `create_dir_all` fail first, so
//! the fsync open is not reached with a FIFO in hand.
//!
//! The precondition is the same one that grounds the containment refusals:
//! an agent sandboxed to the repository can create a FIFO inside the repo
//! (`mkfifo` needs no privileges), including inside `.agentrec/`, and every
//! path below lives there.
//!
//! **lstat-then-open is not atomic** and is not claimed to be: a path swapped
//! between the check and the open still reaches the raw call. Closing that
//! properly needs `O_NONBLOCK`/`openat` on the descriptor itself. What these
//! guards remove is the *durable* hazard — a FIFO sitting on disk when the
//! read arrives — which is every case observed so far. The residual race is
//! recorded in `VERIFY-LEDGER.md` alongside the `build_plan`→`fs::write`
//! TOCTOU it shares a shape with.

use std::io::{Error, ErrorKind, Result};
use std::path::Path;

/// `true` when `path` exists and is something other than a regular file.
/// A missing path is `false` — absence is the caller's business (a `create`
/// revert legitimately targets one), and only an existing non-regular file
/// can block a read.
///
/// `symlink_metadata` is an lstat: it describes the link itself, where
/// `metadata` would describe (and a read would follow to) the target. A
/// symlink therefore counts as non-regular here — callers that want the
/// pointed-to file must resolve it deliberately, which is the safe default
/// for every current caller.
pub fn is_nonregular(path: &Path) -> bool {
    #[cfg(unix)]
    {
        std::fs::symlink_metadata(path)
            .map(|m| !m.file_type().is_file())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// [`std::fs::read`] that refuses to block: lstat first, and error rather
/// than open anything that is not a regular file.
///
/// The error is [`ErrorKind::InvalidInput`] so callers can keep treating a
/// failure the way they already treat an unreadable file — no caller learns a
/// new shape, which is what let this be applied across several sites at once
/// without moving any of their observable behavior for ordinary files.
pub fn read_regular(path: &Path) -> Result<Vec<u8>> {
    if is_nonregular(path) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "not a regular file — refusing to read (a fifo would block)",
        ));
    }
    std::fs::read(path)
}

/// [`std::fs::read_to_string`] with the same guard.
pub fn read_regular_to_string(path: &Path) -> Result<String> {
    if is_nonregular(path) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "not a regular file — refusing to read (a fifo would block)",
        ));
    }
    std::fs::read_to_string(path)
}

/// [`std::fs::File::open`] with the same guard, for callers that stream
/// rather than slurp (a multi-MB `log.jsonl` is read line-by-line, and
/// reading it into memory to gain the guard would be a real regression).
pub fn open_regular(path: &Path) -> Result<std::fs::File> {
    if is_nonregular(path) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "not a regular file — refusing to open (a fifo would block)",
        ));
    }
    std::fs::File::open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn a_fifo_is_refused_rather_than_opened() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("pipe");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);

        // Both would HANG rather than fail without the guard, which is why
        // this test's value is that it terminates at all.
        assert_eq!(
            read_regular(&fifo).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
        assert_eq!(
            open_regular(&fifo).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
    }

    #[test]
    fn an_ordinary_file_reads_normally() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("f.txt");
        std::fs::write(&p, b"content\n").unwrap();

        assert_eq!(read_regular(&p).unwrap(), b"content\n");
        assert!(open_regular(&p).is_ok());
    }

    #[test]
    fn a_missing_path_keeps_its_ordinary_not_found_error() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("nope");

        // NotFound, not InvalidInput: callers distinguish "absent" from
        // "refused", and absence must not start reading as a refusal.
        assert_eq!(read_regular(&p).unwrap_err().kind(), ErrorKind::NotFound);
        assert_eq!(open_regular(&p).unwrap_err().kind(), ErrorKind::NotFound);
    }

    #[test]
    #[cfg(unix)]
    fn a_directory_is_refused_by_the_guard_not_by_the_read() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            read_regular(tmp.path()).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
    }
}
