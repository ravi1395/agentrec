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
    /// Bytes of candidates that would otherwise have been evicted but were
    /// saved by `extra_protected` (Phase 1 honesty fix) — surfaced so a
    /// caller can explain a low/zero `bytes` figure instead of reporting
    /// "0 freed" with no reason, the exact dishonest-status shape an
    /// earlier round already fixed for the orphan-bloat case.
    pub protected_bytes: u64,
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
///
/// Two blobs are never evicted regardless of budget: (A2) any blob that's
/// also referenced as *any* turn's `prompt_ref` — prompt retention is a
/// disjoint TTL-based policy (`purge`), not budget-based, so a blob shared
/// between the two must survive here even if its snapshot side is old; and
/// (A5) the newest turn's own blobs, *as long as an older turn exists to
/// evict instead* — a single oversized newest turn must never be evicted
/// wholesale just because it's also the boundary turn (that would delete
/// the most recent snapshot, the one most likely to be needed for
/// `diff`/`undo`) while cheaper, older data still exists to sacrifice
/// first. When the newest turn is the *only* turn, this protection does not
/// apply — there is nothing else to sacrifice, so budget enforcement falls
/// back to evicting it (see `cmds::tests::status_prints_over_budget_notice`,
/// which asserts exactly this single-turn case).
///
/// `extra_protected` (Phase 1 honesty fix): a caller-supplied set of hashes
/// that must never be evicted regardless of the above, subtracted from
/// `candidates` immediately before the remove loop. This module only knows
/// about *parsed* `TurnRecord`s, so it cannot on its own see a blob cited
/// only by the in-flight turn's crash journal (`open.json`), a memory pin
/// (`memory.jsonl`), or a torn/unparseable `log.jsonl` line — exactly the
/// classes `cli/src/purgecmd.rs::referenced_hashes` (this repo's own stated
/// safety standard for CAS deletion) protects via a raw, non-parsing
/// `sha256:` byte-scan. `agentrec-core` stays dependency-free of that CLI
/// module, so the caller harvests the set and passes it in.
pub fn enforce_budget(
    store: &BlobStore,
    entries: &[TurnRecord],
    budget: u64,
    extra_protected: &HashSet<String>,
) -> Evicted {
    // A3(c): a candidate touched *during this pass* — e.g. a concurrent
    // daemon's dedup-hit landing between the caller's log.jsonl read and
    // this call — must never be evicted even though the keep-set computed
    // from that (now slightly stale) log doesn't yet mention it.
    let pass_start = std::time::SystemTime::now();

    // A2: prompt blobs share the same CAS as snapshot blobs; protect any
    // blob referenced as a prompt by ANY turn, not just kept ones.
    let protected_prompts: HashSet<&str> = entries
        .iter()
        .filter_map(|t| t.prompt_ref.as_deref())
        .collect();

    let mut keep: HashSet<String> = HashSet::new();
    let mut running: u64 = 0;
    let mut boundary = false;
    let mut candidates: HashSet<String> = HashSet::new();

    for (i, turn) in entries.iter().rev().enumerate() {
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
            // A5: the newest turn is protected from eviction as long as
            // there's older data to sacrifice instead — never wipe the most
            // recent turn while anything older still exists to evict
            // first. When the newest turn is the *only* turn, there is
            // nothing else to sacrifice and budget enforcement has no
            // alternative but to evict it (matches the single-turn AC in
            // `cmds::tests::status_prints_over_budget_notice`).
            let protect_newest = i == 0 && entries.len() > 1;
            if running + new_bytes > budget && !protect_newest {
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

    // Phase 1: subtract the caller's protect-set from `candidates`
    // immediately before the remove loop — as late as possible, so a
    // narrower window exists between harvesting refs and this pass
    // deleting anything. Tally the bytes saved so the caller can attribute
    // a low/zero `evicted.bytes` instead of reporting it unexplained.
    let mut protected_bytes = 0u64;
    candidates.retain(|hash| {
        if extra_protected.contains(hash) {
            protected_bytes += store.size(hash).unwrap_or(0);
            false
        } else {
            true
        }
    });

    let mut evicted = Evicted {
        protected_bytes,
        ..Evicted::default()
    };
    for hash in candidates {
        if keep.contains(&hash) {
            continue; // referenced by a kept (newer) turn — never evict
        }
        if protected_prompts.contains(hash.as_str()) {
            continue; // A2: shared with a prompt blob — never evict here
        }
        if store.mtime(&hash).is_some_and(|mtime| mtime > pass_start) {
            continue; // A3(c): touched after this pass started — too fresh
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
            imported: None,
            files_complete: None,
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
            skipped_reason: None,
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
        let evicted = enforce_budget(&store, &entries, 15, &HashSet::new());

        assert_eq!(
            evicted,
            Evicted {
                count: 1,
                bytes: 8,
                protected_bytes: 0
            }
        );
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

        let evicted = enforce_budget(&store, &entries, 1_000_000, &HashSet::new());

        assert_eq!(evicted, Evicted::default());
        assert!(store.contains(&h));
    }

    // A5: when the newest turn's own snapshot bytes alone exceed budget but
    // an older turn also exists, the newest turn must never be evicted —
    // only the older turn's blob is sacrificed. Pre-fix, boundary tripped
    // at the very first iteration (the newest turn), `keep` never
    // accumulated anything, and every turn's hashes (including the
    // newest's) landed in `candidates` — wiping the entire store just
    // because the newest turn alone didn't fit budget.
    #[test]
    fn enforce_budget_never_evicts_newest_turn_when_older_data_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let old_small = store.put(&[0x11u8; 3]).unwrap();
        let huge = store.put(&[0xDDu8; 100]).unwrap();
        let entries = vec![
            turn(
                "t_OLDSMALL0000000000000001",
                "2026-01-01T00:00:00.000Z",
                vec![entry("a", None, Some(&old_small), "create")],
            ),
            turn(
                "t_HUGENEWEST00000000000001",
                "2026-01-02T00:00:00.000Z",
                vec![entry("b", None, Some(&huge), "create")],
            ),
        ];

        // Budget smaller than even the newest turn's own blob alone.
        let evicted = enforce_budget(&store, &entries, 10, &HashSet::new());

        assert_eq!(
            evicted,
            Evicted {
                count: 1,
                bytes: 3,
                protected_bytes: 0
            },
            "only the older turn's blob is sacrificed"
        );
        assert!(
            store.contains(&huge),
            "newest turn's oversized blob survives"
        );
        assert!(
            !store.contains(&old_small),
            "older blob evicted to make room"
        );
    }

    // A5 residual (documented, not fixed — see per-finding writeup):
    // when the newest turn is the ONLY turn, there is nothing older to
    // sacrifice instead, so budget enforcement still evicts it. This
    // mirrors `cmds::tests::status_prints_over_budget_notice` (not owned by
    // this module), which asserts exactly this single-turn eviction as
    // intended product behavior — the two could not both be satisfied, so
    // A5's protection is scoped to "newest turn while older data exists."
    #[test]
    fn enforce_budget_still_evicts_a_sole_newest_turn_with_nothing_older() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let big = store.put(&[0xDDu8; 100]).unwrap();
        let entries = vec![turn(
            "t_SOLE0000000000000000001",
            "2026-01-01T00:00:00.000Z",
            vec![entry("x", None, Some(&big), "create")],
        )];

        let evicted = enforce_budget(&store, &entries, 10, &HashSet::new());

        assert_eq!(
            evicted,
            Evicted {
                count: 1,
                bytes: 100,
                protected_bytes: 0
            }
        );
        assert!(!store.contains(&big));
    }

    // A5 (multi-turn variant): an older turn that's the actual boundary
    // (not the newest) is still evicted normally — only the newest turn
    // gets the special no-eviction treatment.
    #[test]
    fn enforce_budget_still_evicts_a_non_newest_boundary_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let old = store.put(&[0xAAu8; 50]).unwrap();
        let new = store.put(&[0xCCu8; 5]).unwrap();
        let entries = vec![
            turn(
                "t_OLDBOUNDARY0000000000001",
                "2026-01-01T00:00:00.000Z",
                vec![entry("x", None, Some(&old), "create")],
            ),
            turn(
                "t_NEWEST000000000000000001",
                "2026-01-02T00:00:00.000Z",
                vec![entry("y", None, Some(&new), "create")],
            ),
        ];

        let evicted = enforce_budget(&store, &entries, 5, &HashSet::new());

        assert_eq!(
            evicted,
            Evicted {
                count: 1,
                bytes: 50,
                protected_bytes: 0
            }
        );
        assert!(!store.contains(&old));
        assert!(store.contains(&new));
    }

    // A2: a blob content-shared as BOTH a turn's prompt_ref AND another
    // turn's snapshot must survive budget eviction even when the snapshot
    // side alone would be evicted as an old, over-budget blob — prompt
    // retention is TTL-based (`purge`), never budget-based.
    #[test]
    fn enforce_budget_never_evicts_a_blob_shared_with_a_prompt_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let shared = store.put(b"shared between prompt and snapshot").unwrap();
        let new = store.put(&[0xEEu8; 5]).unwrap();

        let old_snapshot_turn = turn(
            "t_OLDSNAP00000000000000001",
            "2026-01-01T00:00:00.000Z",
            vec![entry("x", None, Some(&shared), "create")],
        );

        let mut newest_turn = turn(
            "t_NEWEST200000000000000001",
            "2026-01-02T00:00:00.000Z",
            vec![entry("y", None, Some(&new), "create")],
        );
        // A DIFFERENT turn's prompt happens to hash to the same content.
        newest_turn.prompt_ref = Some(shared.clone());

        let entries = vec![old_snapshot_turn, newest_turn];

        // Budget fits only the newest turn's unique bytes — `shared` would
        // normally be evicted as the old turn's blob.
        let evicted = enforce_budget(&store, &entries, 5, &HashSet::new());

        assert_eq!(
            evicted,
            Evicted::default(),
            "the only candidate is prompt-protected — nothing evicted"
        );
        assert!(store.contains(&shared), "prompt-shared blob must survive");
        assert!(store.contains(&new));
    }

    // A3(c): a blob whose mtime is after this eviction pass's `pass_start`
    // must not be evicted, even though the caller's `entries` snapshot
    // (read moments earlier) doesn't reference it — models a concurrent
    // dedup-hit landing between the caller's log.jsonl read and this call.
    // A future mtime is used instead of a real sleep-then-touch race so the
    // test is deterministic rather than timing-dependent.
    #[test]
    fn enforce_budget_skips_a_candidate_touched_after_pass_start() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let old = store.put(&[0xAAu8; 50]).unwrap();
        let new = store.put(&[0xCCu8; 5]).unwrap();
        let entries = vec![
            turn(
                "t_RACEOLD0000000000000001",
                "2026-01-01T00:00:00.000Z",
                vec![entry("x", None, Some(&old), "create")],
            ),
            turn(
                "t_RACENEW0000000000000001",
                "2026-01-02T00:00:00.000Z",
                vec![entry("y", None, Some(&new), "create")],
            ),
        ];

        let hex = old.strip_prefix("sha256:").unwrap();
        let path = tmp.path().join(&hex[..2]).join(&hex[2..]);
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(future)
            .unwrap();

        let evicted = enforce_budget(&store, &entries, 5, &HashSet::new());

        assert_eq!(
            evicted,
            Evicted::default(),
            "future-touched (racing) candidate must be skipped"
        );
        assert!(store.contains(&old), "old blob survives due to fresh mtime");
    }

    // Phase 1 (core mechanism): `extra_protected` must save a genuine
    // eviction candidate — a blob referenced only by an OLD turn, backdated
    // well before `pass_start` so A3(c)'s freshness guard cannot rescue it
    // vacuously — regardless of what real-world file the caller harvested
    // that hash from (open.json / memory.jsonl / a torn log.jsonl line all
    // funnel into the same set from the CLI side; this module only cares
    // that the hash is IN the set). `eviction.protected_bytes` also reports
    // what was saved, for the caller's status attribution message.
    #[test]
    fn enforce_budget_honors_extra_protected_argument() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let old = store.put(&[0xAAu8; 50]).unwrap();
        let new = store.put(&[0xCCu8; 5]).unwrap();
        let entries = vec![
            turn(
                "t_EXTRAOLD00000000000001",
                "2026-01-01T00:00:00.000Z",
                vec![entry("x", None, Some(&old), "create")],
            ),
            turn(
                "t_EXTRANEW00000000000001",
                "2026-01-02T00:00:00.000Z",
                vec![entry("y", None, Some(&new), "create")],
            ),
        ];

        let hex = old.strip_prefix("sha256:").unwrap();
        let path = tmp.path().join(&hex[..2]).join(&hex[2..]);
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(past)
            .unwrap();

        let extra_protected: HashSet<String> = [old.clone()].into_iter().collect();
        let evicted = enforce_budget(&store, &entries, 5, &extra_protected);

        assert_eq!(
            evicted,
            Evicted {
                count: 0,
                bytes: 0,
                protected_bytes: 50
            },
            "the sole candidate was saved by extra_protected, not evicted"
        );
        assert!(store.contains(&old), "extra_protected blob must survive");
        assert!(store.contains(&new));
    }
}
