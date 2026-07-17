//! Content-addressed blob store (PROTOCOL §6): sha256, two-char fan-out dirs,
//! per-file cap, dedup on write, integrity check on read.

use crate::perms;
use crate::MAX_SNAPSHOT_BYTES;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct BlobStore {
    dir: PathBuf,
    /// Per-store write counter — makes tmp file names unique across
    /// concurrent writers, even when they're writing identical content.
    seq: AtomicU64,
}

/// True iff every byte is a lowercase-hex digit (`0-9a-f`). Shared by
/// `list_hashes`' fan-out validation and mirrors `object_path`'s inline rule.
fn is_hex(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `sha256:<hex>` over raw bytes — the hash form used everywhere in the log.
pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(71);
    hex.push_str("sha256:");
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

impl BlobStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        BlobStore {
            dir: dir.into(),
            seq: AtomicU64::new(0),
        }
    }

    /// Store bytes, returning the `sha256:` ref; None when over the cap or
    /// on any I/O failure. Prefer [`BlobStore::put_result`] when the caller
    /// needs to distinguish those two cases (D35).
    pub fn put(&self, bytes: &[u8]) -> Option<String> {
        match self.put_result(bytes) {
            PutResult::Stored(hash) => Some(hash),
            PutResult::OverCap | PutResult::IoError(_) => None,
        }
    }

    /// Store bytes with a typed result (D35) and crash-safe write (D34):
    /// content is written to a per-writer-unique tmp file inside the
    /// object's fan-out dir, fsynced, renamed into place, then the fan-out
    /// dir itself is fsynced so the rename survives a crash. Dedup: an
    /// existing intact object is never rewritten.
    pub fn put_result(&self, bytes: &[u8]) -> PutResult {
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return PutResult::OverCap;
        }
        let hash = hash_bytes(bytes);
        let Some(path) = self.object_path(&hash) else {
            // hash_bytes always produces a well-formed sha256 hex ref (A1);
            // this is unreachable in practice, but keeps object_path's
            // validation load-bearing rather than assumed-away.
            return PutResult::IoError("invalid object hash".to_string());
        };
        if path.exists() {
            // A4: dedup normally trusts the existing object verbatim — but
            // if it's silently corrupt, discarding the good bytes we were
            // just handed (by returning early) would be worse than
            // rewriting. Verify before trusting.
            let intact = fs::read(&path).is_ok_and(|existing| hash_bytes(&existing) == hash);
            if intact {
                // A3: best-effort mtime touch — narrows the TOCTOU window
                // where a retention pass snapshots its keep-set from
                // log.jsonl just before this dedup-hit lands, by giving a
                // concurrent reader a fresher mtime to notice.
                if let Ok(file) = fs::OpenOptions::new().write(true).open(&path) {
                    let _ = file.set_modified(std::time::SystemTime::now());
                }
                // A4: dedup previously skipped the dir fsync entirely (a
                // D34 hole) — fsync it too, since a prior crash could have
                // lost the directory entry even though this inode survived.
                if let Some(parent) = path.parent() {
                    if let Ok(dir) = fs::File::open(parent) {
                        let _ = dir.sync_all();
                    }
                }
                return PutResult::Stored(hash);
            }
            // Corrupt — fall through and rewrite via the normal tmp+rename
            // path below instead of trusting it.
        }
        let Some(parent) = path.parent() else {
            return PutResult::IoError("object path has no parent directory".to_string());
        };
        if let Err(e) = fs::create_dir_all(parent) {
            return PutResult::IoError(e.to_string());
        }
        // umask-independent (D37): the fan-out dir may have just been created.
        perms::lock_dir(parent);
        // Unique per writer+call so two threads snapshotting identical
        // content never share a tmp file.
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(".tmp.{}.{}", std::process::id(), seq));

        let write = (|| -> std::io::Result<()> {
            // A6: create at 0600 directly (unix) — mode is set at the same
            // syscall that creates the file, so there is no window where
            // the tmp file sits at the process umask default before a
            // later chmod locks it down.
            let mut file = create_tmp_file(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&tmp, &path)?;
            Ok(())
        })();

        match write {
            // A7: the blob is durably at its final name the moment
            // rename() returns — a dir-fsync failure past this point (e.g.
            // a read-only remount racing the write) must never be reported
            // as "no snapshot" when the snapshot plainly exists.
            Ok(()) => finish_stored(hash, parent),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                PutResult::IoError(e.to_string())
            }
        }
    }

    /// Read a blob back; verifies content integrity against its address
    /// (a corrupted object is reported as an error, never silently served).
    pub fn get(&self, hash: &str) -> Result<Vec<u8>, StoreError> {
        let Some(path) = self.object_path(hash) else {
            return Err(StoreError::Missing(hash.to_string()));
        };
        let bytes = fs::read(&path).map_err(|_| StoreError::Missing(hash.to_string()))?;
        if hash_bytes(&bytes) != hash {
            return Err(StoreError::Corrupt(hash.to_string()));
        }
        Ok(bytes)
    }

    pub fn contains(&self, hash: &str) -> bool {
        self.object_path(hash).is_some_and(|p| p.exists())
    }

    /// Size in bytes of the object at `hash`, or `None` if it doesn't exist
    /// (including when `hash` is malformed — A1).
    pub fn size(&self, hash: &str) -> Option<u64> {
        let path = self.object_path(hash)?;
        fs::metadata(path).ok().map(|m| m.len())
    }

    /// Last-modified time of the object at `hash`, if present (A3) — lets a
    /// retention pass give a just-touched blob (e.g. a concurrent dedup-hit)
    /// a grace window before it's eligible for eviction.
    pub fn mtime(&self, hash: &str) -> Option<std::time::SystemTime> {
        let path = self.object_path(hash)?;
        fs::metadata(path).ok().and_then(|m| m.modified().ok())
    }

    /// Delete the object at `hash` if present, returning its size (bytes)
    /// beforehand. Used by retention (purge / budget eviction) — never by the
    /// write/read paths, which must never lose a live blob. A malformed
    /// `hash` (A1) is treated as missing — never touches disk.
    pub fn remove(&self, hash: &str) -> Option<u64> {
        let path = self.object_path(hash)?;
        let size = fs::metadata(&path).ok().map(|m| m.len());
        if size.is_some() {
            let _ = fs::remove_file(&path);
        }
        size
    }

    pub fn total_bytes(&self) -> u64 {
        fn walk(dir: &PathBuf) -> u64 {
            let Ok(entries) = fs::read_dir(dir) else {
                return 0;
            };
            entries
                .filter_map(|e| e.ok())
                .map(|e| {
                    let p = e.path();
                    if p.is_dir() {
                        walk(&p)
                    } else {
                        e.metadata().map(|m| m.len()).unwrap_or(0)
                    }
                })
                .sum()
        }
        walk(&self.dir)
    }

    /// Every well-formed blob currently on disk, as `sha256:` refs. Walks the
    /// two-char fan-out dirs and reconstructs `sha256:<fan><rest>` for each
    /// leaf whose name completes a valid 64-lowercase-hex hash. Anything that
    /// isn't a proper `<2hex>/<62hex>` pair — a stray `.tmp.<pid>` write, a
    /// non-hex file, an unexpected nesting depth — is silently skipped, never
    /// returned as a blob (so `purge --orphans` can never archive it based on
    /// a malformed name). Order is unspecified (directory order).
    pub fn list_hashes(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(fans) = fs::read_dir(&self.dir) else {
            return out;
        };
        for fan in fans.filter_map(|e| e.ok()) {
            let fan_name = fan.file_name();
            let Some(fan_str) = fan_name.to_str() else {
                continue;
            };
            if fan_str.len() != 2 || !is_hex(fan_str.as_bytes()) || !fan.path().is_dir() {
                continue;
            }
            let Ok(rest_entries) = fs::read_dir(fan.path()) else {
                continue;
            };
            for leaf in rest_entries.filter_map(|e| e.ok()) {
                let leaf_name = leaf.file_name();
                let Some(rest_str) = leaf_name.to_str() else {
                    continue;
                };
                if rest_str.len() == 62 && is_hex(rest_str.as_bytes()) {
                    out.push(format!("sha256:{fan_str}{rest_str}"));
                }
            }
        }
        out
    }

    /// Move the blob at `hash` into `archive_dir`, preserving the same
    /// `<fan>/<rest>` fan-out layout, and return the bytes moved (or `None`
    /// if the hash is malformed or absent). A same-filesystem `rename`, so
    /// this is cheap regardless of blob size — `purge --orphans` archives
    /// (never deletes) reclaimable blobs, so a rewrite/undo mistake is always
    /// recoverable by moving the archive dir back. Never used on the
    /// write/read hot paths.
    pub fn archive(&self, hash: &str, archive_dir: &std::path::Path) -> Option<u64> {
        let src = self.object_path(hash)?;
        let size = fs::metadata(&src).ok().map(|m| m.len())?;
        let hex = hash.strip_prefix("sha256:")?;
        let (fan, rest) = hex.split_at(2);
        let dest_fan = archive_dir.join(fan);
        fs::create_dir_all(&dest_fan).ok()?;
        fs::rename(&src, dest_fan.join(rest)).ok()?;
        Some(size)
    }

    /// `sha256:aabb...` → `<dir>/aa/bb...`, or `None` if `hash` isn't a
    /// well-formed `sha256:` ref — exactly 64 lowercase hex chars (A1). A
    /// crafted hash containing `/`, `..`, or an absolute-path component
    /// must never be joined onto a store path: `PathBuf::join` silently
    /// *replaces* the base when the argument is absolute, which would let a
    /// hash like `"sha256:xx/abs/path"` turn a store operation (including
    /// `remove`, used by purge/eviction) into an arbitrary-path delete.
    /// Malformed hashes are treated as missing everywhere — never touched
    /// on disk.
    fn object_path(&self, hash: &str) -> Option<PathBuf> {
        let hex = hash.strip_prefix("sha256:")?;
        if hex.len() != 64 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let (fan, rest) = hex.split_at(2);
        Some(self.dir.join(fan).join(rest))
    }
}

/// Create the write-tmp file at mode 0600 (unix) in the same syscall that
/// creates it — set-at-create rather than create-then-chmod, so there is no
/// window where the tmp file is briefly reachable at the process umask
/// default (A6).
#[cfg(unix)]
fn create_tmp_file(path: &std::path::Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_tmp_file(path: &std::path::Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Best-effort fsync of the fan-out dir after a successful rename, then
/// unconditionally report `Stored` (A7): the blob already exists at its
/// final name by the time this runs, so a dir-fsync failure past this point
/// is a durability warning, never a "no snapshot" report.
fn finish_stored(hash: String, parent: &std::path::Path) -> PutResult {
    if let Ok(dir) = fs::File::open(parent) {
        if let Err(e) = dir.sync_all() {
            eprintln!("agentrec: snapshot {hash} stored but parent-dir fsync failed: {e}");
        }
    }
    PutResult::Stored(hash)
}

/// Typed outcome of a write (D35) — replaces the lossy `Option<String>`
/// where callers need to tell "too big" apart from "disk/permission error".
#[derive(Debug, PartialEq)]
pub enum PutResult {
    /// Written (or already present) under this `sha256:` ref.
    Stored(String),
    /// Rejected: exceeds `MAX_SNAPSHOT_BYTES`.
    OverCap,
    /// Write failed; message is `io::Error`'s `Display` output.
    IoError(String),
}

#[derive(Debug, PartialEq)]
pub enum StoreError {
    Missing(String),
    Corrupt(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Missing(h) => write!(f, "missing snapshot object {h}"),
            StoreError::Corrupt(h) => write!(f, "corrupt snapshot object {h} (hash mismatch)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_dedup_and_fanout() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let h = store.put(b"hello").unwrap();
        assert!(h.starts_with("sha256:"));
        assert_eq!(store.get(&h).unwrap(), b"hello");
        assert_eq!(store.put(b"hello").unwrap(), h);
        // fan-out layout: first two hex chars are a directory
        let hex = h.strip_prefix("sha256:").unwrap();
        assert!(tmp.path().join(&hex[..2]).join(&hex[2..]).exists());
    }

    #[test]
    fn over_cap_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let big = vec![0u8; crate::MAX_SNAPSHOT_BYTES + 1];
        assert!(store.put(&big).is_none());
    }

    #[test]
    fn corrupt_object_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let h = store.put(b"payload").unwrap();
        let hex = h.strip_prefix("sha256:").unwrap();
        let path = tmp.path().join(&hex[..2]).join(&hex[2..]);
        fs::write(&path, b"tampered").unwrap();
        assert_eq!(store.get(&h), Err(StoreError::Corrupt(h.clone())));
    }

    #[test]
    fn put_result_over_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let big = vec![0u8; crate::MAX_SNAPSHOT_BYTES + 1];
        assert_eq!(store.put_result(&big), PutResult::OverCap);
    }

    #[test]
    fn put_result_roundtrip_and_dedup() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let h = match store.put_result(b"payload") {
            PutResult::Stored(h) => h,
            other => panic!("expected Stored, got {other:?}"),
        };
        assert_eq!(store.get(&h).unwrap(), b"payload");
        // second put dedups to the same hash, no rewrite
        assert_eq!(store.put_result(b"payload"), PutResult::Stored(h.clone()));

        let hex = h.strip_prefix("sha256:").unwrap();
        let fan_dir = tmp.path().join(&hex[..2]);
        let entries: Vec<_> = fs::read_dir(&fan_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1, "exactly one object file, no leftover tmp");
        assert!(fan_dir.join(&hex[2..]).exists());
    }

    #[test]
    fn concurrent_identical_snapshot_one_intact_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());

        let results: Vec<PutResult> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| store.put_result(b"same-bytes")))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let hashes: Vec<String> = results
            .into_iter()
            .map(|r| match r {
                PutResult::Stored(h) => h,
                other => panic!("expected Stored, got {other:?}"),
            })
            .collect();
        assert!(
            hashes.windows(2).all(|w| w[0] == w[1]),
            "all writers must agree on the hash"
        );
        assert_eq!(store.get(&hashes[0]).unwrap(), b"same-bytes");

        // No leftover .tmp.* file anywhere under the store dir.
        fn walk_tmp(dir: &std::path::Path, found: &mut Vec<PathBuf>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.is_dir() {
                    walk_tmp(&p, found);
                } else if p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(".tmp."))
                {
                    found.push(p);
                }
            }
        }
        let mut leftovers = Vec::new();
        walk_tmp(tmp.path(), &mut leftovers);
        assert!(leftovers.is_empty(), "leftover tmp files: {leftovers:?}");
    }

    #[test]
    fn size_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let h = store.put(b"twelve bytes").unwrap();
        assert_eq!(store.size(&h), Some(12));
        assert_eq!(store.size("sha256:missing"), None);

        assert_eq!(store.remove(&h), Some(12));
        assert!(!store.contains(&h));
        assert_eq!(
            store.remove(&h),
            None,
            "already removed — no size to report"
        );
    }

    // D37: every blob the store writes lands at mode 0600, regardless of the
    // process umask (the default test umask would otherwise yield 644).
    #[cfg(unix)]
    #[test]
    fn put_result_writes_blob_at_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let h = match store.put_result(b"secret-bytes") {
            PutResult::Stored(h) => h,
            other => panic!("expected Stored, got {other:?}"),
        };
        let hex = h.strip_prefix("sha256:").unwrap();
        let path = tmp.path().join(&hex[..2]).join(&hex[2..]);
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "blob must be 0600 umask-independent");

        let dir_mode = fs::metadata(tmp.path().join(&hex[..2]))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700, "fan-out dir must be 0700");
    }

    #[cfg(unix)]
    #[test]
    fn io_error_is_typed() {
        use std::os::unix::fs::PermissionsExt;

        let base = tempfile::tempdir().unwrap();
        let locked = base.path().join("locked");
        fs::create_dir(&locked).unwrap();
        // store dir itself does not exist yet; its parent (`locked`) will be
        // made read-only so `create_dir_all` of the fan-out dir must fail.
        let store_dir = locked.join("store");
        let store = BlobStore::new(&store_dir);

        let mut perms = fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o500);
        fs::set_permissions(&locked, perms).unwrap();

        let result = store.put_result(b"data");

        // restore perms first so tempdir cleanup can remove `locked`
        let mut perms = fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&locked, perms).unwrap();

        assert!(
            matches!(result, PutResult::IoError(_)),
            "expected IoError, got {result:?}"
        );
    }

    // A1: a crafted hash with an embedded absolute-path component must never
    // reach disk. `PathBuf::join` silently *replaces* the base when its
    // argument is absolute, so pre-fix `object_path` for
    // `"sha256:xx" + "/abs/path"` returned the victim path verbatim once
    // split at 2 hex chars — every store op (crucially `remove`, used by
    // purge/eviction) would then touch a file completely outside the store.
    #[test]
    fn malformed_hash_never_touches_disk_outside_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());

        // Plant a victim file outside the store dir entirely.
        let victim_dir = tempfile::tempdir().unwrap();
        let victim = victim_dir.path().join("planted-file.txt");
        fs::write(&victim, b"do not delete me").unwrap();

        // `strip_prefix("sha256:")` leaves an absolute path once the first
        // two hex-fan chars are split off.
        let evil_hash = format!("sha256:xx{}", victim.display());

        assert!(!store.contains(&evil_hash), "malformed hash must not exist");
        assert_eq!(store.size(&evil_hash), None);
        assert_eq!(
            store.get(&evil_hash),
            Err(StoreError::Missing(evil_hash.clone()))
        );
        assert_eq!(
            store.remove(&evil_hash),
            None,
            "remove on a malformed hash must be a no-op"
        );

        assert!(
            victim.exists(),
            "victim file outside the store must survive"
        );
        assert_eq!(fs::read(&victim).unwrap(), b"do not delete me");
    }

    // A1: a relative `..` traversal component must also be rejected, not
    // just an absolute path.
    #[test]
    fn malformed_hash_rejects_traversal_components() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let evil_hash = "sha256:aa/../../../../etc/hosts";
        assert!(!store.contains(evil_hash));
        assert_eq!(store.remove(evil_hash), None);
    }

    // A4: a dedup-hit on a silently-corrupted existing object must heal it
    // (rewrite with the good bytes just handed in) rather than discarding
    // the good bytes and reporting success over corrupt content.
    #[test]
    fn dedup_hit_heals_corrupt_existing_object() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let h = store.put(b"good content").unwrap();
        let hex = h.strip_prefix("sha256:").unwrap();
        let path = tmp.path().join(&hex[..2]).join(&hex[2..]);
        fs::write(&path, b"corrupted!!!").unwrap();
        assert_eq!(store.get(&h), Err(StoreError::Corrupt(h.clone())));

        // Re-put the same content: pre-fix, `path.exists()` short-circuited
        // to `Stored(hash)` without checking the bytes on disk, leaving the
        // corruption in place forever.
        let result = store.put_result(b"good content");
        assert_eq!(result, PutResult::Stored(h.clone()));
        assert_eq!(store.get(&h).unwrap(), b"good content", "corruption healed");
    }

    // A3: a dedup-hit touches the existing object's mtime — a retention pass
    // consulting mtime can use it to detect "this blob was just referenced
    // again," narrowing the TOCTOU window against a keep-set computed from a
    // slightly-stale log.jsonl read.
    #[test]
    fn dedup_hit_touches_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let h = store.put(b"touch-me").unwrap();
        let before = store.mtime(&h).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(store.put(b"touch-me").unwrap(), h, "dedup hit");
        let after = store.mtime(&h).unwrap();

        assert!(after > before, "dedup hit must bump mtime forward");
    }

    // A6: the tmp file must land at 0600 as part of its creation call, not
    // via a later chmod — there is no window at any other mode.
    #[cfg(unix)]
    #[test]
    fn create_tmp_file_is_0600_at_creation() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".tmp.probe");
        let file = create_tmp_file(&path).unwrap();
        let mode = file.metadata().unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "must be 0600 at the create() syscall itself"
        );
    }

    // A7: once rename() has landed the blob at its final name, a parent-dir
    // fsync failure (dir vanished, read-only remount, etc.) must never
    // downgrade an already-durable write to IoError / "no snapshot".
    #[test]
    fn finish_stored_never_downgrades_a_landed_blob() {
        let bogus_parent = std::path::Path::new("/nonexistent/agentrec-a7-probe-dir");
        let result = finish_stored("sha256:deadbeef".to_string(), bogus_parent);
        assert_eq!(result, PutResult::Stored("sha256:deadbeef".to_string()));
    }

    // list_hashes enumerates real blobs and NEVER returns a stray tmp write or
    // a malformed (wrong-length / non-hex) file name — so `purge --orphans`
    // can't archive garbage based on a bad name.
    #[test]
    fn list_hashes_enumerates_blobs_and_skips_tmp_and_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let a = store.put(b"alpha").unwrap();
        let b = store.put(b"beta").unwrap();

        // Litter: an in-flight tmp write in a fan dir, a wrong-length leaf,
        // and a non-hex fan dir — none may appear in list_hashes.
        let fan = &a.strip_prefix("sha256:").unwrap()[..2];
        std::fs::write(tmp.path().join(fan).join(".tmp.9999.1"), b"x").unwrap();
        std::fs::write(tmp.path().join(fan).join("short"), b"x").unwrap();
        std::fs::create_dir_all(tmp.path().join("zz")).unwrap();
        std::fs::write(tmp.path().join("zz").join("z".repeat(62)), b"x").unwrap();

        let mut got = store.list_hashes();
        got.sort();
        let mut want = vec![a, b];
        want.sort();
        assert_eq!(got, want, "only the two real blobs, no litter");
    }

    // archive() renames a blob into the archive dir (same fan-out) and reports
    // its size; the source is gone, the archived copy exists.
    #[test]
    fn archive_moves_blob_preserving_fanout() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let h = store.put(&[0x42u8; 16]).unwrap();
        let archive = tmp.path().join("archived");

        let moved = store.archive(&h, &archive);

        assert_eq!(moved, Some(16));
        assert!(!store.contains(&h), "source blob moved out");
        let hex = h.strip_prefix("sha256:").unwrap();
        assert!(
            archive.join(&hex[..2]).join(&hex[2..]).exists(),
            "blob preserved in archive with fan-out layout"
        );
    }
}
