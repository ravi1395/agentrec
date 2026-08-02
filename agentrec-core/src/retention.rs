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

/// Read-only report from `plan_eviction`: which blobs a budget-enforcement
/// pass would evict, and why the rest survive. Nothing in `plan_eviction`
/// touches disk — this is what lets a read verb (`status`) render the same
/// honesty figures `enforce_budget` used to only produce as a side effect of
/// deleting (perf-evidence round, closes B8).
#[derive(Debug)]
pub struct EvictionPlan {
    /// `(hash, size)` pairs that would be evicted if this plan is executed.
    pub victims: Vec<(String, u64)>,
    /// Bytes saved by `extra_protected` — same accumulation semantics as
    /// `Evicted::protected_bytes` (B12: computed here, before the freshness
    /// guard runs, so a candidate that is both externally protected and
    /// fresh is counted exactly once).
    pub protected_bytes: u64,
    /// Sum of `victims`' sizes — projected, not actual: `execute` may still
    /// spare a victim whose mtime moved past `pass_start` between plan and
    /// execute (B11), so this is an upper bound on what `execute` frees.
    pub freed_bytes_projected: u64,
    /// Captured once, at the start of the plan pass. `execute` re-checks
    /// each victim's freshness against THIS value, never a fresh
    /// `SystemTime::now()` — a fresh-now recheck would let a blob touched
    /// between plan and execute-start be deleted, i.e. strictly *less*
    /// protection than the unsplit code offered in a hard-delete path
    /// (B11, non-negotiable).
    pub pass_start: std::time::SystemTime,
}

/// Pure planning half of budget eviction (perf-evidence round, closes B8):
/// walks the turn log and partitions candidates exactly like the old
/// unsplit `enforce_budget` did, but deletes nothing. `execute` turns the
/// result into real deletions; `enforce_budget` composes the two so its
/// external behavior stays byte-identical to before the split. See
/// `enforce_budget`'s doc comment below for the full walk/candidate/A2/A5
/// semantics — they are unchanged, just relocated to this pure half.
pub fn plan_eviction(
    store: &BlobStore,
    entries: &[TurnRecord],
    budget: u64,
    extra_protected: &HashSet<String>,
) -> EvictionPlan {
    // A3(c): a candidate touched *during this pass* — e.g. a concurrent
    // daemon's dedup-hit landing between the caller's log.jsonl read and
    // this call — must never be reported as a victim even though the
    // keep-set computed from that (now slightly stale) log doesn't yet
    // mention it.
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

    // Phase 1: subtract the caller's protect-set from `candidates` first —
    // this ordering (protect-retain BEFORE the freshness guard below) must
    // never change: a candidate that is both externally protected and
    // fresh must be counted in `protected_bytes` exactly once (AC2a.2);
    // reordering silently shrinks the figure `status` renders.
    let mut protected_bytes = 0u64;
    candidates.retain(|hash| {
        if extra_protected.contains(hash) {
            protected_bytes += store.size(hash).unwrap_or(0);
            false
        } else {
            true
        }
    });

    let mut victims: Vec<(String, u64)> = Vec::new();
    let mut freed_bytes_projected = 0u64;
    for hash in candidates {
        if keep.contains(&hash) {
            continue; // referenced by a kept (newer) turn — never a victim
        }
        if protected_prompts.contains(hash.as_str()) {
            continue; // A2: shared with a prompt blob — never a victim
        }
        if store.mtime(&hash).is_some_and(|mtime| mtime > pass_start) {
            continue; // A3(c) plan-side: too fresh to report as a victim —
                      // a dry-run that lists a fresh blob as a victim
                      // would be a wrong report.
        }
        let size = store.size(&hash).unwrap_or(0);
        freed_bytes_projected += size;
        victims.push((hash, size));
    }

    EvictionPlan {
        victims,
        protected_bytes,
        freed_bytes_projected,
        pass_start,
    }
}

/// Executes an `EvictionPlan`: deletes each victim, re-checking freshness
/// against the plan's *carried* `pass_start` immediately before each
/// `store.remove` (never a fresh `SystemTime::now()` — B11). This preserves
/// today's delete-time semantics exactly: the freshness guard used to be
/// evaluated once, at delete time, inside the same pass that built the
/// keep-set; splitting into plan+execute must not widen that window by
/// letting a blob touched between plan and execute-start survive undetected.
pub fn execute(store: &BlobStore, plan: EvictionPlan) -> Evicted {
    let mut evicted = Evicted {
        protected_bytes: plan.protected_bytes,
        ..Evicted::default()
    };
    for (hash, _projected_size) in plan.victims {
        if store
            .mtime(&hash)
            .is_some_and(|mtime| mtime > plan.pass_start)
        {
            continue; // touched after the plan was taken — too fresh
        }
        if let Some(size) = store.remove(&hash) {
            evicted.count += 1;
            // From `remove`'s return, never the plan's projected size — a
            // blob already gone (raced away between plan and execute)
            // contributes nothing, matching the old unsplit behavior.
            evicted.bytes += size;
        }
    }
    evicted
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
///
/// Thin wrapper over `plan_eviction` + `execute` (perf-evidence round) —
/// externally byte-identical to the prior single-pass implementation.
pub fn enforce_budget(
    store: &BlobStore,
    entries: &[TurnRecord],
    budget: u64,
    extra_protected: &HashSet<String>,
) -> Evicted {
    execute(
        store,
        plan_eviction(store, entries, budget, extra_protected),
    )
}

/// The byte set the budget is actually enforced over: the sum of the
/// **unique** snapshot-blob (`FileEntry.before`/`after`) sizes referenced by
/// `entries`. This is, by construction, the same accumulation
/// [`plan_eviction`] runs against `budget` — same `turn_snapshot_hashes`
/// dedup, same `store.size(..).unwrap_or(0)` lookup — which is the whole
/// point of it living here rather than being re-derived by a caller.
///
/// **The equivalence this exists to make true (F26, redteam round 2):**
/// `managed_bytes(store, entries) > budget` ⟺ `plan_eviction(store, entries,
/// budget, _)` sets its boundary, i.e. produces a non-empty candidate set.
/// Forward: `plan_eviction`'s `running` only ever grows by a hash's size the
/// first time that hash is seen, so `running <= managed_bytes` at every step
/// — if `managed_bytes <= budget` the guard `running + new_bytes > budget` can
/// never fire and nothing is ever a candidate. Reverse: the `new_bytes` summed
/// across a full boundary-free walk is exactly `managed_bytes`, so a walk that
/// never trips the guard ends with `running == managed_bytes <= budget`;
/// `managed_bytes > budget` therefore contradicts a boundary-free walk. (A5's
/// `protect_newest` only suppresses the guard at `i == 0`, so it can delay the
/// boundary by one turn but never prevent it.) Pinned both directions by
/// `managed_bytes_at_or_under_budget_means_no_candidates` /
/// `managed_bytes_over_budget_means_candidates_even_with_a_huge_orphan`.
///
/// **What this does NOT promise:** that being over budget frees anything. A2
/// (prompt-shared), A5 (newest turn), `extra_protected`, and the A3(c)
/// freshness guard can each spare a candidate, so `freed_bytes_projected` may
/// legitimately be 0 on a genuinely over-budget store — which is why
/// [`EvictionPlan::protected_bytes`] exists and why `status` renders it.
///
/// **What is outside this set, deliberately:** orphaned blobs (on disk,
/// referenced by no turn — reclaimable only by `purge --orphans`), blobs
/// referenced *only* as a `prompt_ref` (disjoint TTL policy, `purge`), and
/// anything under `.agentrec/` that is not in `objects/` at all. That last
/// class now includes the daemon's own `daemon.log`: the launchd unit routes
/// `StandardOutPath`/`StandardErrorPath` to `<root>/.agentrec/daemon.log`
/// (`cli/src/service.rs::daemon_log_path`), a sibling of `objects/`, and
/// [`BlobStore::total_bytes`] walks `objects/` only — so neither this figure
/// nor `store_bytes` sees it grow (F24 disclosure; unbounded, not fixed here).
///
/// Cost: one `fs::metadata` per unique referenced hash, on top of the
/// `total_bytes` walk a caller computing both already pays.
pub fn managed_bytes<'a>(
    store: &BlobStore,
    entries: impl IntoIterator<Item = &'a TurnRecord>,
) -> u64 {
    let mut seen: HashSet<String> = HashSet::new();
    let mut total = 0u64;
    for turn in entries {
        for h in turn_snapshot_hashes(turn) {
            if seen.insert(h.clone()) {
                total += store.size(&h).unwrap_or(0);
            }
        }
    }
    total
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
            after_synthesized: None,
            link_kind: None,
            attribution: None,
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

    // AC2a.2 (perf-evidence round): carries the plan-side freshness guard's
    // ONLY discriminator (B12) — `enforce_budget_skips_a_candidate_touched_
    // after_pass_start` above pins the guard *pair* via the delete outcome
    // only, so under the both-halves design `execute`'s re-check masks a
    // deleted plan-side guard from every deletion-observable assertion.
    // Five turns, oldest → newest: V (backdated, genuinely evicted), P
    // (extra_protected only), F (a boundary candidate future-dated past
    // `pass_start` — must survive planning via the freshness guard, and
    // must NOT contribute to `protected_bytes`, matching today's semantics
    // where the freshness guard is a silent `continue`, not a protect-set
    // entry), D (BOTH extra_protected AND future-dated — the case that
    // proves protect-retain runs before the freshness guard: reordering
    // would remove D via freshness first, so the later protect-retain step
    // never sees it and `protected_bytes` silently loses its contribution),
    // and NEW (newest, tiny, budget fits it exactly so it alone is kept).
    #[test]
    fn plan_reports_without_deleting_then_execute_deletes_exactly_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());

        let v = store.put(&[0xAAu8; 50]).unwrap(); // victim
        let p = store.put(&[0xBBu8; 20]).unwrap(); // extra_protected only
        let f = store.put(&[0xCCu8; 15]).unwrap(); // future-dated only
        let d = store.put(&[0xDDu8; 10]).unwrap(); // extra_protected AND future-dated
        let new = store.put(&[0xEEu8; 5]).unwrap(); // newest, kept by budget

        let entries = vec![
            turn(
                "t_PLANV00000000000000001",
                "2026-01-01T00:00:00.000Z",
                vec![entry("v", None, Some(&v), "create")],
            ),
            turn(
                "t_PLANP00000000000000001",
                "2026-01-02T00:00:00.000Z",
                vec![entry("p", None, Some(&p), "create")],
            ),
            turn(
                "t_PLANF00000000000000001",
                "2026-01-03T00:00:00.000Z",
                vec![entry("f", None, Some(&f), "create")],
            ),
            turn(
                "t_PLAND00000000000000001",
                "2026-01-04T00:00:00.000Z",
                vec![entry("d", None, Some(&d), "create")],
            ),
            turn(
                "t_PLANNEW0000000000000001",
                "2026-01-05T00:00:00.000Z",
                vec![entry("n", None, Some(&new), "create")],
            ),
        ];

        // V: backdated well before pass_start — no timestamp coincidence
        // can rescue it from the freshness guard.
        let hex_v = v.strip_prefix("sha256:").unwrap();
        let path_v = tmp.path().join(&hex_v[..2]).join(&hex_v[2..]);
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path_v)
            .unwrap()
            .set_modified(past)
            .unwrap();

        // F and D: future-dated past any plausible pass_start (mirrors
        // `enforce_budget_skips_a_candidate_touched_after_pass_start`).
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        for h in [&f, &d] {
            let hex = h.strip_prefix("sha256:").unwrap();
            let path = tmp.path().join(&hex[..2]).join(&hex[2..]);
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(future)
                .unwrap();
        }

        // P is left at its natural (pre-plan) creation mtime — well before
        // `pass_start`, which is captured only once `plan_eviction` runs.
        let extra_protected: HashSet<String> = [p.clone(), d.clone()].into_iter().collect();

        // Budget fits only NEW's 5 bytes — V, P, F, D all become candidates.
        let plan = plan_eviction(&store, &entries, 5, &extra_protected);

        assert_eq!(
            plan.victims,
            vec![(v.clone(), 50)],
            "only V is a genuine victim — F excluded by freshness, P/D by extra_protected"
        );
        assert_eq!(
            plan.protected_bytes,
            20 + 10,
            "P (20) + D (10) — D counted exactly once despite being both \
             extra_protected and fresh"
        );
        assert_eq!(
            plan.freed_bytes_projected, 50,
            "excludes F's 15 bytes — F is not a victim"
        );

        // Planning must not touch disk.
        for h in [&v, &p, &f, &d, &new] {
            assert!(store.contains(h), "plan_eviction must delete nothing");
        }

        let evicted = execute(&store, plan);
        assert_eq!(
            evicted,
            Evicted {
                count: 1,
                bytes: 50,
                protected_bytes: 30,
            }
        );
        assert!(!store.contains(&v), "V is the only thing execute deletes");
        assert!(store.contains(&p));
        assert!(store.contains(&f));
        assert!(store.contains(&d));
        assert!(store.contains(&new));
    }

    // AC2a.3 (perf-evidence round): the deterministic replacement for the
    // plan's rejected vacuous future-dated-bump design (B11). A blob that
    // `plan_eviction` genuinely lists as a victim gets its mtime bumped by a
    // REAL-NOW dedup-put (store.rs:83-84's `set_modified(SystemTime::now())`
    // path) AFTER the plan is taken — never a future-dated filetime, which
    // would satisfy `mtime > t` for any plausible `t` and so cannot tell
    // apart "execute re-checks against the plan's carried pass_start" from
    // "execute re-checks against a fresh now()" (the two designs B11 exists
    // to distinguish). `execute` must then spare it.
    #[test]
    fn execute_recheck_spares_blob_touched_after_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let x_bytes = [0xAAu8; 50];
        let x = store.put(&x_bytes).unwrap();
        let y = store.put(&[0xCCu8; 5]).unwrap();

        let entries = vec![
            turn(
                "t_RECHECKOLD000000000001",
                "2026-01-01T00:00:00.000Z",
                vec![entry("x", None, Some(&x), "create")],
            ),
            turn(
                "t_RECHECKNEW000000000001",
                "2026-01-02T00:00:00.000Z",
                vec![entry("y", None, Some(&y), "create")],
            ),
        ];

        // Budget fits only the newest turn (Y) — X is the sole (non-newest)
        // boundary victim, so A5's newest-turn protection cannot mask this.
        let plan = plan_eviction(&store, &entries, 5, &HashSet::new());
        assert_eq!(
            plan.victims,
            vec![(x.clone(), 50)],
            "X is the plan's sole victim before any mtime bump"
        );

        // Granularity guard (NB14): give the filesystem's mtime clock room
        // to move forward, mirroring `dedup_hit_touches_mtime`'s own 20ms
        // gap for the same strict `>` comparison.
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Real-now dedup-put: identical bytes, so this is a genuine dedup
        // hit (not a fresh write), and `put_result` bumps the existing
        // object's mtime to `SystemTime::now()` as part of that path.
        assert_eq!(store.put(&x_bytes).unwrap(), x, "dedup hit on X");

        // Self-guarding precondition: if the filesystem's mtime granularity
        // didn't actually move X's mtime past `plan.pass_start`, fail here
        // with a clear precondition message rather than silently passing
        // (or silently failing) the real assertions below for the wrong
        // reason — this is the exact false-RED class the NB14 guard exists
        // to surface, not paper over.
        let x_mtime = store
            .mtime(&x)
            .expect("X must still exist before the precondition check");
        assert!(
            x_mtime > plan.pass_start,
            "PRECONDITION FAILED: filesystem mtime granularity did not move \
             X's mtime past plan.pass_start ({x_mtime:?} vs {:?}) — this is \
             the false-RED the NB14 sleep+precondition guard exists to \
             surface; the 20ms gap was insufficient on this filesystem",
            plan.pass_start
        );

        let evicted = execute(&store, plan);
        assert_eq!(
            evicted,
            Evicted {
                count: 0,
                bytes: 0,
                protected_bytes: 0,
            },
            "X was touched after the plan was taken — execute must spare it"
        );
        assert!(store.contains(&x), "X survives execute");
        assert!(store.contains(&y));
    }

    /// F26, forward direction. `managed_bytes <= budget` must mean
    /// `plan_eviction` has nothing to do — asserted at the boundary
    /// (`managed == budget`, the tightest under-budget case) rather than
    /// comfortably below it, so an off-by-one in the guard reds.
    ///
    /// Neuter: change `managed_bytes`' dedup to count `shared` twice (drop the
    /// `seen.insert` guard) and `managed` reads 25 > 20, contradicting the
    /// empty victim set this asserts alongside it.
    #[test]
    fn managed_bytes_at_or_under_budget_means_no_candidates() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());

        let a = store.put(&[0xAAu8; 8]).unwrap();
        let shared = store.put(&[0xBBu8; 5]).unwrap();
        let b = store.put(&[0xCCu8; 7]).unwrap();

        let t1 = turn(
            "t_MB0000000000000000000001",
            "2026-01-01T00:00:00.000Z",
            vec![entry("x", None, Some(&a), "create")],
        );
        let t2 = turn(
            "t_MB0000000000000000000002",
            "2026-01-02T00:00:00.000Z",
            // `shared` appears as this turn's `after` AND the next turn's
            // `before` — counted once, which is what makes 20 the right
            // figure rather than 25.
            vec![entry("x", Some(&a), Some(&shared), "modify")],
        );
        let t3 = turn(
            "t_MB0000000000000000000003",
            "2026-01-03T00:00:00.000Z",
            vec![entry("x", Some(&shared), Some(&b), "modify")],
        );
        let entries = vec![t1, t2, t3];

        let managed = managed_bytes(&store, entries.iter());
        assert_eq!(managed, 20, "8 + 5 + 7, `shared` counted once");

        let plan = plan_eviction(&store, &entries, managed, &HashSet::new());
        assert!(
            plan.victims.is_empty(),
            "managed_bytes == budget must leave nothing to evict: {:?}",
            plan.victims
        );
        assert_eq!(plan.freed_bytes_projected, 0);
    }

    /// F26, reverse direction AND the defect itself. A store whose *disk*
    /// bytes are dominated by an orphan the evictor can never touch must not
    /// be judged over budget by those bytes — while a store whose *managed*
    /// bytes exceed the budget must always yield candidates.
    ///
    /// The orphan here is 10x the referenced bytes, which is the shape F26
    /// measured (43.4% orphaned, measured then on the dogfood store — not
    /// re-measured here, and not a claim about any store's state now).
    ///
    /// Neuter: point `managed_bytes` at `store.total_bytes()` instead of the
    /// referenced set and the first assertion reds (200 != 20).
    #[test]
    fn managed_bytes_over_budget_means_candidates_even_with_a_huge_orphan() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());

        let a = store.put(&[0xAAu8; 10]).unwrap();
        let b = store.put(&[0xCCu8; 10]).unwrap();
        // Referenced by nothing: ineligible as a victim, yet on disk.
        let orphan = store.put(&[0xDDu8; 180]).unwrap();

        let t1 = turn(
            "t_MB0000000000000000000011",
            "2026-01-01T00:00:00.000Z",
            vec![entry("x", None, Some(&a), "create")],
        );
        let t2 = turn(
            "t_MB0000000000000000000012",
            "2026-01-02T00:00:00.000Z",
            vec![entry("y", None, Some(&b), "create")],
        );
        let entries = vec![t1, t2];

        assert_eq!(store.total_bytes(), 200, "disk bytes include the orphan");
        assert_eq!(
            managed_bytes(&store, entries.iter()),
            20,
            "the budget's byte set excludes the orphan the evictor cannot reclaim"
        );

        // A budget between the two: over budget on disk, under it on the set
        // eviction actually ranges over. This is exactly F26's permanent
        // "over budget, evicting nothing" state — the evictor must be idle.
        let idle = plan_eviction(&store, &entries, 100, &HashSet::new());
        assert!(
            idle.victims.is_empty(),
            "disk-only over-budget must not produce victims: {:?}",
            idle.victims
        );
        assert!(store.contains(&orphan), "the orphan is never a victim");

        // And above the managed figure the boundary really does trip.
        let busy = plan_eviction(&store, &entries, 19, &HashSet::new());
        assert!(
            !busy.victims.is_empty(),
            "managed_bytes > budget must yield candidates"
        );
    }
}
