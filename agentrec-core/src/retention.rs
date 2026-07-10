//! Store-size budget eviction (AC I+ / D-retention). TTL-based prompt purge
//! and the `--snapshots-before` date purge are CLI-level concerns (they read
//! `.agentrec/config.toml` and a user-supplied date) — see `cli/src/purgecmd.rs`.
//! This module is the pure, unit-testable half: given the turn log and a byte
//! budget, evict the oldest snapshot blobs (never prompt blobs — those have
//! their own TTL path) while never touching a blob a newer, kept turn shares.

use crate::record::TurnRecord;
use crate::store::BlobStore;
use std::collections::HashSet;

/// Outcome of a budget eviction pass.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Evicted {
    pub count: usize,
    pub bytes: u64,
}

/// Evict snapshot blobs (each turn's `FileEntry.before`/`after`) belonging to
/// the oldest turns once their accumulated size would exceed `budget`.
///
/// `entries` must be in append (oldest-first) log order — the same order
/// `record::load_log` returns turns in. Walks newest → oldest, accumulating
/// each turn's *unique* (not already counted) snapshot-blob bytes. The first
/// turn whose inclusion would push the running total over `budget` — and
/// every turn older than it — becomes an eviction candidate for any of its
/// blobs not already kept by a newer, already-accepted turn (content-addressed
/// sharing means the same blob can belong to both an old and a new turn).
pub fn enforce_budget(store: &BlobStore, entries: &[TurnRecord], budget: u64) -> Evicted {
    let mut keep: HashSet<String> = HashSet::new();
    let mut running: u64 = 0;
    let mut boundary = false;
    let mut candidates: HashSet<String> = HashSet::new();

    for turn in entries.iter().rev() {
        let hashes = turn_snapshot_hashes(turn);
        if !boundary {
            let mut new_bytes: u64 = 0;
            let mut new_hashes: Vec<String> = Vec::new();
            for h in &hashes {
                if keep.contains(h) {
                    continue;
                }
                new_bytes += store.size(h).unwrap_or(0);
                new_hashes.push(h.clone());
            }
            if running + new_bytes > budget {
                boundary = true;
                for h in hashes {
                    if !keep.contains(&h) {
                        candidates.insert(h);
                    }
                }
            } else {
                running += new_bytes;
                keep.extend(new_hashes);
            }
        } else {
            for h in hashes {
                if !keep.contains(&h) {
                    candidates.insert(h);
                }
            }
        }
    }

    let mut evicted = Evicted::default();
    for hash in candidates {
        if keep.contains(&hash) {
            continue; // referenced by a kept (newer) turn — never evict
        }
        if let Some(size) = store.remove(&hash) {
            evicted.count += 1;
            evicted.bytes += size;
        }
    }
    evicted
}

/// Unique snapshot-blob hashes (`before` + `after`) referenced by one turn.
fn turn_snapshot_hashes(turn: &TurnRecord) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for f in &turn.files {
        for h in [f.before.as_deref(), f.after.as_deref()]
            .into_iter()
            .flatten()
        {
            if seen.insert(h.to_string()) {
                out.push(h.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::FileEntry;

    fn turn(id: &str, started: &str, files: Vec<FileEntry>) -> TurnRecord {
        TurnRecord {
            v: 1,
            id: id.to_string(),
            grade: "rich".into(),
            truncated: false,
            started: started.to_string(),
            ended: started.to_string(),
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

    fn entry(path: &str, before: Option<&str>, after: Option<&str>, op: &str) -> FileEntry {
        FileEntry {
            path: path.into(),
            before: before.map(String::from),
            after: after.map(String::from),
            op: op.into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }
    }

    #[test]
    fn enforce_budget_evicts_oldest_snapshots_only() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());

        // OLD: unique to the oldest turn (8 bytes).
        let old = store.put(&[0xAAu8; 8]).unwrap();
        // SHARED: the oldest turn's sibling created it as `after`; the middle
        // turn's `before` references the same content (10 bytes).
        let shared = store.put(&[0xBBu8; 10]).unwrap();
        // NEW: unique to the newest turn (5 bytes).
        let new = store.put(&[0xCCu8; 5]).unwrap();

        let t1 = turn(
            "t_OLD0000000000000000000001",
            "2026-01-01T00:00:00.000Z",
            vec![entry("x", None, Some(&old), "create")],
        );
        let t2 = turn(
            "t_MID0000000000000000000001",
            "2026-01-02T00:00:00.000Z",
            vec![entry("x", Some(&old), Some(&shared), "modify")],
        );
        let t3 = turn(
            "t_NEW0000000000000000000001",
            "2026-01-03T00:00:00.000Z",
            vec![entry("x", Some(&shared), Some(&new), "modify")],
        );
        // Oldest-first, matching load_log order.
        let entries = vec![t1, t2, t3];

        // Budget fits exactly the newest turn's unique bytes (shared + new = 15).
        let evicted = enforce_budget(&store, &entries, 15);

        assert_eq!(evicted, Evicted { count: 1, bytes: 8 });
        assert!(!store.contains(&old), "oldest unique blob evicted");
        assert!(
            store.contains(&shared),
            "shared blob kept (referenced by newest turn)"
        );
        assert!(store.contains(&new), "newest blob kept");
    }

    #[test]
    fn enforce_budget_under_budget_evicts_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let h = store.put(b"small").unwrap();
        let entries = vec![turn(
            "t_ONE0000000000000000000001",
            "2026-01-01T00:00:00.000Z",
            vec![entry("x", None, Some(&h), "create")],
        )];

        let evicted = enforce_budget(&store, &entries, 1_000_000);

        assert_eq!(evicted, Evicted::default());
        assert!(store.contains(&h));
    }
}
