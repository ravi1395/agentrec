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
    /// existing object is never rewritten, and never re-fsynced.
    pub fn put_result(&self, bytes: &[u8]) -> PutResult {
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return PutResult::OverCap;
        }
        let hash = hash_bytes(bytes);
        let path = self.object_path(&hash);
        if path.exists() {
            return PutResult::Stored(hash);
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
            let mut file = fs::File::create(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            // Lock down before the rename makes it visible under its final
            // name (D37) — no window where the blob is reachable at 0644.
            perms::lock_file(&tmp);
            fs::rename(&tmp, &path)?;
            let dir = fs::File::open(parent)?;
            dir.sync_all()?;
            Ok(())
        })();

        match write {
            Ok(()) => PutResult::Stored(hash),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                PutResult::IoError(e.to_string())
            }
        }
    }

    /// Read a blob back; verifies content integrity against its address
    /// (a corrupted object is reported as an error, never silently served).
    pub fn get(&self, hash: &str) -> Result<Vec<u8>, StoreError> {
        let path = self.object_path(hash);
        let bytes = fs::read(&path).map_err(|_| StoreError::Missing(hash.to_string()))?;
        if hash_bytes(&bytes) != hash {
            return Err(StoreError::Corrupt(hash.to_string()));
        }
        Ok(bytes)
    }

    pub fn contains(&self, hash: &str) -> bool {
        self.object_path(hash).exists()
    }

    /// Size in bytes of the object at `hash`, or `None` if it doesn't exist.
    pub fn size(&self, hash: &str) -> Option<u64> {
        fs::metadata(self.object_path(hash)).ok().map(|m| m.len())
    }

    /// Delete the object at `hash` if present, returning its size (bytes)
    /// beforehand. Used by retention (purge / budget eviction) — never by the
    /// write/read paths, which must never lose a live blob.
    pub fn remove(&self, hash: &str) -> Option<u64> {
        let size = self.size(hash);
        if size.is_some() {
            let _ = fs::remove_file(self.object_path(hash));
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

    /// `sha256:aabb...` → `<dir>/aa/bb...`
    fn object_path(&self, hash: &str) -> PathBuf {
        let hex = hash.strip_prefix("sha256:").unwrap_or(hash);
        let (fan, rest) = hex.split_at(hex.len().min(2));
        self.dir.join(fan).join(rest)
    }
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
}
