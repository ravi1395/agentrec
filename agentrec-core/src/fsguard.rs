//! Blocking-I/O guards: open and read only ORDINARY files.
//!
//! Every function here exists for one reason. `std::fs::read` and
//! `File::open` on a FIFO **block until a peer appears** — indefinitely, with
//! no timeout and no error. Anywhere agentrec reads a path an untrusted party
//! can create, that is a hang, and for the single-threaded stdio MCP server a
//! hang is the whole agent-facing surface dying for that session.
//!
//! The class was found at one site per round across five branch-review
//! rounds, each fix correct and each insufficient — `inode_refusal` in the
//! planner, then `read_current_hash`, then this module, then the read tools
//! and the undo ledger. Patching the site
//! the last probe happened to hit does not converge, because the property is
//! not "this path is safe" but "we never open something that is not a
//! regular file". These helpers make that property expressible once, so a new
//! call site inherits it instead of having to remember it.
//!
//! **Three deliberate exemption CLASSES**, each justified rather than
//! overlooked. The per-site `#[allow(clippy::disallowed_methods)]`
//! annotations point back here for their rationale, so this list is the
//! registry and must name every production site — `grep -rn
//! 'allow(clippy::disallowed_methods)' agentrec-core/src cli/src` is the
//! authoritative cross-check (its remaining hits are `#[cfg(test)]` modules
//! reading their own tempdir fixtures, plus this module's own three wrappers).
//!
//! (1) Character devices agentrec opens BY NAME and by design —
//! `/dev/urandom` in `id.rs::os_random_bytes` (two sites: the primary read
//! and its retry) — must not be guarded: `read_regular` would refuse them,
//! and refusing the entropy source is a worse failure than the hang this
//! module prevents.
//!
//! (2) Fixed system paths under `/proc`, which no local party can replace:
//! `doctorcmd.rs::check_inotify` reads
//! `/proc/sys/fs/inotify/max_user_watches`. That site is
//! `#[cfg(target_os = "linux")]`, so darwin clippy never lints it and the
//! annotation exists for the Linux leg.
//!
//! (3) `File::open` on a DIRECTORY for fsync, five sites:
//! `store.rs::put` (the dedup-hit arm), `store.rs::finish_stored`,
//! `purgecmd.rs::rewrite_memory_atomic`, `purgecmd.rs::rewrite_log_atomic`,
//! and `purgecmd.rs::rewrite_signal_atomic`. Opening a real directory cannot
//! block. That third class carries an assumption worth stating, because
//! "this path is the type I expect" is the exact assumption class that
//! produced this whole series — it holds only while the parent really is a
//! directory, and a FIFO pre-created at a fan-out path would make
//! `create_dir_all` fail first, so the fsync open is not reached with a FIFO
//! in hand.
//!
//! **The lint covers READS only; write-opens are guarded by hand.**
//! `clippy.toml` disallows the three blocking READ entry points
//! (`fs::read`, `fs::read_to_string`, `File::open`). It does NOT cover the
//! write side — `OpenOptions::open` with `write`/`append`, `fs::write`, and
//! `fs::copy` all block on a FIFO just as hard, and none of them is
//! expressible as a `disallowed-methods` entry, because `OpenOptions::open`
//! is the same method for a guarded read and an unguarded write. Every
//! production write-open onto an in-repo path therefore calls
//! [`is_nonregular`] inline before opening: `record.rs::open_append` (the one
//! primitive behind every `log.jsonl`/`signal.jsonl`/`memory.jsonl` append),
//! `undo_coordinator.rs::append_event`, `UndoLock::acquire`,
//! `daemon.rs::acquire_lock`, `daemon.rs::daemon_is_running`, the daemon's
//! `open.json` journal write, `loglock.rs`/`memlock.rs`/`hookcmds.rs`
//! lock-file opens, `hookcmds.rs::write_scratch_entries`,
//! `state.rs::write_state`, `readcmds.rs::write_undo_guard`, and purgecmd's
//! `append_lines_synced`/`write_full_file_synced` archive writers. Because
//! that is a convention rather than a lint, a NEW write-open can still be
//! added unguarded — a disclosed open class, not a closed one.
//!
//! Two production write-sites are deliberately NOT guarded inline, each
//! because a guard upstream makes the site unreachable with a FIFO in hand:
//! `store.rs::put`'s mtime touch (reached only after `read_regular` returned
//! bytes that hash to the expected value, which a FIFO cannot do) and
//! `initcmd.rs`/`uninstallcmd.rs`'s `fs::copy`-then-`fs::write` config
//! merges (each reads the target through `read_regular_to_string` first and
//! aborts on any non-`NotFound` error before the copy). Outside the repo
//! threat model entirely, and unguarded: `service.rs`'s service-unit write
//! under `~/Library/LaunchAgents` (or the XDG systemd dir), whose read
//! refusal folds into `unwrap_or(false)` and proceeds to write.
//!
//! The precondition is the same one that grounds the containment refusals:
//! an agent sandboxed to the repository can create a FIFO inside the repo
//! (`mkfifo` needs no privileges), including inside `.agentrec/`, and every
//! path below lives there.
//!
//! **check-then-open is not atomic** and is not claimed to be: a path swapped
//! between the type check and the open still reaches the raw call. Closing that
//! properly needs `O_NONBLOCK`/`openat` on the descriptor itself. What these
//! guards remove is the *durable* hazard — a FIFO sitting on disk when the
//! read arrives — which is every case observed so far. The residual race is
//! recorded in `VERIFY-LEDGER.md` alongside the `build_plan`→`fs::write`
//! TOCTOU it shares a shape with.

use std::io::{Error, ErrorKind, Result};
use std::path::Path;

/// `true` when `path` RESOLVES to something other than a regular file.
/// A missing path — including a broken symlink — is `false`: absence is the
/// caller's business (a `create` revert legitimately targets one), and only
/// an existing non-regular file can block a read.
///
/// `std::fs::metadata` is a `stat`: it FOLLOWS a symlink and describes the
/// TARGET, which is deliberate — a symlink itself can never block on open,
/// only the thing it eventually resolves to can (round-7 branch-review
/// blocker: an earlier version of this function used `symlink_metadata`
/// (an lstat) and therefore refused every symlink outright, including a
/// symlink to an ordinary file. `.agentrec/log.jsonl` relocated behind a
/// symlink — an ordinary setup, e.g. moving the store to another volume —
/// then made `load_log` return an EMPTY ledger, and `log`/`diff`/`blame`/
/// `undo` all silently reported "no history" with the real data one hop
/// away. The full test suite passed throughout, because every test written
/// for this guard asserted a case that must be REFUSED and none asserted a
/// case that must still be ALLOWED.) The blocking hazard this module exists
/// for — FIFO, socket, device — is unchanged by following the link: a
/// symlink TO a fifo still resolves to a fifo and is still refused, which
/// `a_symlink_to_a_fifo_is_still_refused` below pins.
///
/// This is deliberately NOT what the undo WRITE path uses. Where following a
/// symlink is itself the hazard — reverting through a link would write the
/// pointed-to file — `symlink_refusal` in `undo_coordinator.rs` handles that
/// separately, with its own distinct refusal text, and must keep doing so;
/// merging the two predicates would make that gate's tests pass for the
/// wrong reason.
pub fn is_nonregular(path: &Path) -> bool {
    #[cfg(unix)]
    {
        std::fs::metadata(path)
            .map(|m| !m.file_type().is_file())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// [`std::fs::read`] that refuses to block: resolve the path's type first
/// (see [`is_nonregular`]), and error rather than open anything that does not
/// resolve to a regular file.
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
    // This IS the guard: the refusal above already ran.
    #[allow(clippy::disallowed_methods)]
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
    // This IS the guard: the refusal above already ran.
    #[allow(clippy::disallowed_methods)]
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
    // This IS the guard: the refusal above already ran.
    #[allow(clippy::disallowed_methods)]
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

    // Round-7 blocker: a symlink CANNOT block on open — only the resolved
    // target can — so refusing a symlink to an ordinary file was pure
    // over-rejection. `load_log`'s empty-ledger-on-refusal fallback turned
    // this into a SILENT loss of the user's entire history.
    #[test]
    #[cfg(unix)]
    fn a_symlink_to_an_ordinary_file_reads_normally() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.txt");
        std::fs::write(&real, b"content\n").unwrap();
        let link = tmp.path().join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(
            !is_nonregular(&link),
            "a symlink to a regular file must be allowed"
        );
        assert_eq!(read_regular(&link).unwrap(), b"content\n");
        assert!(open_regular(&link).is_ok());
    }

    // The predicate resolves the link, so a symlink INTO the blocking
    // hazard class must still be refused — this is what stops the fix above
    // from becoming a new bypass.
    #[test]
    #[cfg(unix)]
    fn a_symlink_to_a_fifo_is_still_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("pipe");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let link = tmp.path().join("link_to_pipe");
        std::os::unix::fs::symlink(&fifo, &link).unwrap();

        assert!(
            is_nonregular(&link),
            "following the link must still land on the fifo"
        );
        assert_eq!(
            read_regular(&link).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
    }

    // A broken symlink must read as ABSENT (NotFound), not as a refusal —
    // `std::fs::metadata` errors on a dangling target, and `unwrap_or(false)`
    // must keep that case out of `is_nonregular`.
    #[test]
    #[cfg(unix)]
    fn a_broken_symlink_reads_as_not_found_not_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let link = tmp.path().join("dangling");
        std::os::unix::fs::symlink(tmp.path().join("does-not-exist"), &link).unwrap();

        assert!(!is_nonregular(&link));
        assert_eq!(read_regular(&link).unwrap_err().kind(), ErrorKind::NotFound);
    }
}
