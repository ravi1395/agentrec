//! Hash-pinned semantic memory (design spec
//! `docs/superpowers/specs/2026-07-12-agentrec-memory-design.md`). Records are
//! append-only JSONL at `.agentrec/memory.jsonl`; effective state is derived
//! by folding `assert`/`reverify`/`retract` ops per id, latest-op-wins (the
//! same correction-by-append pattern as the turn log). Freshness (pin hash
//! verification) is deliberately NOT this module's job — it is derived at
//! read time by the recall path, never persisted here (INV-M2).

use crate::record::append_line_synced;
use crate::scrub;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Max stored length of a memory's `fact` text (design spec §Data model).
pub const FACT_MAX_CHARS: usize = 500;
/// Max pins a single memory record may carry (design spec §Data model).
pub const PINS_MAX: usize = 8;

/// A content-address pin: the hash of `path` at the time it was pinned.
/// `hash` is `"sha256:<64-hex>"`, matching the CAS blob-id format elsewhere
/// in the workspace. Hash verification against the working tree happens in
/// the (later) recall path, not here.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pin {
    pub path: String,
    pub hash: String,
}

/// The three memory ops (design spec §Data model). `Reverify` re-asserts the
/// fact with fresh pins; `Retract` marks the chain dead. Both reference the
/// original `assert`'s `id` — never a new id.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryOp {
    Assert,
    Reverify,
    Retract,
}

/// One line of `.agentrec/memory.jsonl` (protocol-additive, `v: 1`).
/// Consumers MUST tolerate unknown fields (serde default: unrecognized keys
/// are ignored on read).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MemoryRecord {
    pub v: u32,
    #[serde(rename = "type")]
    pub kind: String,
    /// ulid; `reverify`/`retract` records reference the originating
    /// `assert`'s id, never mint a new one.
    pub id: String,
    pub op: MemoryOp,
    pub fact: String,
    pub pins: Vec<Pin>,
    #[serde(default)]
    pub source_turns: Vec<String>,
    /// "human" | "agent"
    pub origin: String,
    /// epoch ms
    pub ts: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Folded effective state for one memory id — what `assert` + the latest
/// `reverify`/`retract` chain currently says. `retracted` memories are
/// included (flagged), never dropped: callers decide whether to display or
/// skip them (recall skips; `memories --all` shows).
#[derive(Clone, Debug)]
pub struct EffectiveMemory {
    pub id: String,
    pub fact: String,
    pub pins: Vec<Pin>,
    pub origin: String,
    /// ts of the latest op folded into this state (assert, reverify, or
    /// retract — whichever is most recent).
    pub ts: u64,
    pub retracted: bool,
}

/// `.agentrec/memory.jsonl` under `root`.
pub fn memory_path(root: &Path) -> PathBuf {
    root.join(".agentrec").join("memory.jsonl")
}

/// Derived (never persisted — INV-M2) freshness of a memory's pins against
/// the current working tree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Freshness {
    /// Every pinned path exists and hashes to the pinned value.
    Fresh,
    /// Every pinned path exists but at least one hash no longer matches.
    Stale,
    /// At least one pinned path no longer exists. Checked before `Stale` —
    /// a missing file is reported as orphaned, not folded into "stale".
    Orphaned,
}

/// Validate a pin candidate path and normalize it to a root-relative string.
///
/// Rejects (mirrors `store::BlobStore::object_path`'s strict-validation
/// posture — malformed input never reaches disk logic):
/// - absolute paths
/// - any `..` path component, regardless of whether it would normalize back
///   inside `root` — rejecting unconditionally is simplest and safest, and
///   matches the design spec's rejected-approaches list (no clever
///   normalization that could be bypassed).
/// - paths that don't exist under `root` (canonicalization requires it)
/// - symlinks that resolve outside `root` (caught by comparing the
///   canonicalized target against the canonicalized root)
/// - secret-file paths per `scrub::is_secret_path`
///
/// On success, returns the root-relative path as given (already relative,
/// already `..`-free, verified to live inside `root`).
pub fn validate_pin_path(root: &Path, path: &str) -> Result<String, String> {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return Err(format!("pin path must be relative, got absolute: {path}"));
    }
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!("pin path must not contain '..': {path}"));
    }

    let canonical_root = fs::canonicalize(root).map_err(|e| {
        format!(
            "pin root {} could not be canonicalized: {e}",
            root.display()
        )
    })?;
    let joined = root.join(candidate);
    let canonical_target =
        fs::canonicalize(&joined).map_err(|e| format!("pin path does not exist: {path} ({e})"))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(format!("pin path escapes root: {path}"));
    }

    if scrub::is_secret_path(path) {
        return Err(format!(
            "pin path looks like a secret file, refusing: {path}"
        ));
    }

    Ok(path.to_string())
}

/// Hash the file at `root/rel` right now, as `"sha256:<hex>"` via
/// `store::hash_bytes` — the same CAS blob-id format used elsewhere.
pub fn hash_pin(root: &Path, rel: &str) -> Result<String, String> {
    let bytes = fs::read(root.join(rel))
        .map_err(|e| format!("could not read pin path {rel} for hashing: {e}"))?;
    Ok(crate::store::hash_bytes(&bytes))
}

/// Derive freshness of `pins` against the current working tree. Orphaned
/// (any pinned path missing) is checked before stale (any hash mismatch) —
/// orphaned is a labeled sub-case of stale, and takes priority in the
/// result.
pub fn pin_freshness(root: &Path, pins: &[Pin]) -> Freshness {
    let mut any_stale = false;
    for p in pins {
        match hash_pin(root, &p.path) {
            Ok(current) => {
                if current != p.hash {
                    any_stale = true;
                }
            }
            Err(_) => return Freshness::Orphaned,
        }
    }
    if any_stale {
        Freshness::Stale
    } else {
        Freshness::Fresh
    }
}

/// Validate and append one memory record. Fsynced (mirrors `record.rs`
/// turn-close durability — a memory write must survive a kill-9 immediately
/// after this call returns `Ok`).
///
/// Validation (structural only, per design spec §Write path quality gate):
/// - `id` non-empty.
/// - `fact` ≤ `FACT_MAX_CHARS` chars.
/// - `pins` has 1..=`PINS_MAX` entries.
/// - `fact`, once scrubbed, is not empty — a scrubbed-to-nothing fact is a
///   refusal, never a stored husk (design spec §Data model).
///
/// The record persisted carries the *scrubbed* fact, not the caller's raw
/// text — same discipline as prompt persistence elsewhere in the workspace
/// (secrets never reach disk, not even transiently in this call).
pub fn append_memory(root: &Path, rec: &MemoryRecord) -> Result<(), String> {
    if rec.id.trim().is_empty() {
        return Err("memory record id must not be empty".to_string());
    }
    let fact_len = rec.fact.chars().count();
    if fact_len > FACT_MAX_CHARS {
        return Err(format!(
            "fact exceeds {FACT_MAX_CHARS}-char cap ({fact_len} chars)"
        ));
    }
    if rec.pins.is_empty() {
        return Err("memory record must carry at least 1 pin".to_string());
    }
    if rec.pins.len() > PINS_MAX {
        return Err(format!(
            "memory record exceeds {PINS_MAX}-pin cap ({} pins)",
            rec.pins.len()
        ));
    }
    let scrubbed_fact = scrub::scrub(&rec.fact);
    if scrubbed_fact.trim().is_empty() {
        return Err(
            "fact rejected: scrub redacted the entire fact, refusing to store an empty husk"
                .to_string(),
        );
    }

    let mut to_persist = rec.clone();
    to_persist.fact = scrubbed_fact;
    let line = serde_json::to_string(&to_persist).map_err(|e| e.to_string())?;
    append_line_synced(&memory_path(root), &line)
}

/// Tie-break precedence for `load_effective`'s fold when two records share
/// the exact same `ts`: retract > reverify > assert. Returned value is the
/// sort rank (higher sorts later, i.e. wins the "latest op" fold) — do not
/// reorder these without also re-reading `load_effective`'s fold loop.
fn op_rank(op: &MemoryOp) -> u8 {
    match op {
        MemoryOp::Assert => 0,
        MemoryOp::Reverify => 1,
        MemoryOp::Retract => 2,
    }
}

/// Final tie-break key for `load_effective`'s fold sort, used only when two
/// records share the exact same `(ts, op_rank)` — e.g. two `reverify`
/// records for the same id at the same millisecond, with different pins
/// (INV-M5). `(ts, op_rank)` alone is not a total order over such records,
/// and `Vec::sort_by_key` is stable, so without a content tiebreak the
/// winner would silently fall back to file (append) order — exactly the
/// non-determinism INV-M5 forbids ("same records in any order produce the
/// same effective state").
///
/// Built from the record's distinguishing fields — pins (sorted by
/// `(path, hash)` so pin *order* within the record doesn't matter), then
/// `fact`, then `origin` — so:
/// - two records with the same id/ts/op but different content always sort
///   into the same relative order regardless of file order (deterministic
///   winner), and
/// - two byte-identical records (any internal field order) map to the same
///   key, so permuting them is a no-op — idempotency is preserved.
fn content_key(rec: &MemoryRecord) -> (Vec<(String, String)>, String, String) {
    let mut pins: Vec<(String, String)> = rec
        .pins
        .iter()
        .map(|p| (p.path.clone(), p.hash.clone()))
        .collect();
    pins.sort();
    (pins, rec.fact.clone(), rec.origin.clone())
}

/// Load every parseable record, fold per id (latest op wins, ordered by
/// `ts` ascending; equal `ts` broken by op precedence, see `op_rank` —
/// never by file order), and return one `EffectiveMemory` per id. Absent
/// file = empty vec (never an error — a repo with no memories yet is a
/// normal state, not a failure). Malformed lines (bad JSON, or an `op`
/// string this build doesn't recognize) are skipped, never fatal — mirrors
/// `record::load_log`'s tolerance.
pub fn load_effective(root: &Path) -> Result<Vec<EffectiveMemory>, String> {
    Ok(load_effective_checked(root, None)?.0)
}

/// Deadline-cooperative twin of [`load_effective`] (F2). Behaves
/// byte-identically to `load_effective` when `deadline` is `None` — every
/// check below is a cheap `Option`-match that short-circuits to "never
/// exceeded". When `Some`, checks at each record/id loop boundary (not
/// per-byte) and bails to `(vec![], true)` the moment the deadline has
/// passed, discarding whatever partial fold was in progress — the hook path
/// needs a clean "did or didn't finish in budget" signal, not a partial
/// read.
fn load_effective_checked(
    root: &Path,
    deadline: Option<Instant>,
) -> Result<(Vec<EffectiveMemory>, bool), String> {
    let path = memory_path(root);
    let records = match fs::File::open(&path) {
        Ok(file) => {
            let reader = std::io::BufReader::new(file);
            let mut out = Vec::new();
            for line in reader.lines() {
                if deadline_exceeded(deadline) {
                    return Ok((Vec::new(), true));
                }
                let Ok(line) = line else { continue };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(rec) = serde_json::from_str::<MemoryRecord>(trimmed) {
                    out.push(rec);
                }
                // Unparseable line (malformed JSON, or a well-formed object
                // whose `op` isn't one of assert/reverify/retract) is
                // skipped silently — never fatal to the fold.
            }
            out
        }
        Err(_) => return Ok((vec![], false)),
    };

    // Group by id. File (insertion) order within each group is irrelevant
    // to the fold — `group.sort_by_key` below reorders by (ts, op_rank), so
    // equal-ts ties are broken by op precedence, not by this insertion
    // order.
    let mut by_id: HashMap<String, Vec<MemoryRecord>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for rec in records {
        if !by_id.contains_key(&rec.id) {
            order.push(rec.id.clone());
        }
        by_id.entry(rec.id.clone()).or_default().push(rec);
    }

    let mut out = Vec::with_capacity(order.len());
    for id in order {
        if deadline_exceeded(deadline) {
            return Ok((Vec::new(), true));
        }
        let mut group = by_id.remove(&id).unwrap_or_default();
        // Total order: `ts` ascending, then op precedence (retract >
        // reverify > assert), then `content_key` as a final deterministic
        // tiebreak. The fold below takes the *last* element of the sorted
        // group as the effective state, so this must be a genuine total
        // order — `(ts, op_rank)` alone ties whenever two same-op records
        // for the same id share a `ts` (e.g. two `reverify`s with
        // different pins), and `sort_by_key`'s stability would then leak
        // file order into the fold, violating INV-M5. A retraction is
        // never lost to a same-ms assert/reverify, a reverify's fresh pins
        // are never lost to a same-ms assert, and two same-(ts, op)
        // records with different content always resolve to the same
        // winner regardless of file order.
        group.sort_by_key(|r| (r.ts, op_rank(&r.op), content_key(r)));

        let mut fact = String::new();
        let mut pins = Vec::new();
        let mut origin = String::new();
        let mut ts = 0u64;
        let mut retracted = false;
        for rec in group {
            match rec.op {
                MemoryOp::Assert | MemoryOp::Reverify => {
                    fact = rec.fact;
                    pins = rec.pins;
                    origin = rec.origin;
                    ts = rec.ts;
                    retracted = false;
                }
                MemoryOp::Retract => {
                    retracted = true;
                    origin = rec.origin;
                    ts = rec.ts;
                    // fact/pins deliberately left as whatever the last
                    // assert/reverify set — a retraction doesn't carry a
                    // replacement fact.
                }
            }
        }
        out.push(EffectiveMemory {
            id,
            fact,
            pins,
            origin,
            ts,
            retracted,
        });
    }
    Ok((out, false))
}

/// BM25 score floor for a candidate to be recall-eligible at all (tuned in
/// Task 9 against dogfood data — do not retune casually, it interacts with
/// `bm25_rank`'s corpus-relative idf).
pub const SCORE_FLOOR: f64 = 0.8;

const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

/// Lowercase, split on any non-alphanumeric byte, drop empty pieces. Used
/// for both query and doc-text tokenization so `/` and `.` in pin paths
/// naturally split into path segments (`"cli/src/service.rs"` ->
/// `["cli", "src", "service", "rs"]`).
pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Doc text for BM25: `fact` plus every pin's path (path segments fall out
/// of `tokenize`'s non-alphanumeric split — no separate join needed).
fn doc_text(m: &EffectiveMemory) -> String {
    let mut text = m.fact.clone();
    for p in &m.pins {
        text.push(' ');
        text.push_str(&p.path);
    }
    text
}

/// Textbook BM25 (k1=1.2, b=0.75) over the non-retracted entries of
/// `corpus`. Index persistence deliberately omitted — the index is rebuilt
/// per call because the corpus (one repo's memories) is small; see design
/// spec's performance envelope. Returns `(index, score)` pairs, `index`
/// into `corpus` (including retracted slots — they're just never
/// produced), filtered to `score >= SCORE_FLOOR`, sorted score-descending
/// with index-ascending as a deterministic tiebreak. Empty (or
/// all-stopword-stripped-to-nothing) query -> empty vec, no vacuous "top
/// of nothing" match.
pub fn bm25_rank(corpus: &[EffectiveMemory], query: &str) -> Vec<(usize, f64)> {
    bm25_rank_checked(corpus, query, None).0
}

/// Deadline-cooperative twin of [`bm25_rank`] (F2) — byte-identical output
/// to `bm25_rank` when `deadline` is `None`. `Some` checks the deadline at
/// each doc-tokenization and doc-scoring loop boundary (the two `O(corpus)`
/// passes — a "debug BM25 fold over 3001 records" is exactly what blew the
/// old retrospective-only 50ms budget in CI, per commit 76a716d) and bails
/// to `(vec![], true)` the instant it has passed, discarding whatever
/// partial ranking was in progress.
fn bm25_rank_checked(
    corpus: &[EffectiveMemory],
    query: &str,
    deadline: Option<Instant>,
) -> (Vec<(usize, f64)>, bool) {
    let q_tokens = tokenize(query);
    if q_tokens.is_empty() {
        return (Vec::new(), false);
    }

    let mut docs: Vec<(usize, Vec<String>)> = Vec::new();
    for (i, m) in corpus.iter().enumerate().filter(|(_, m)| !m.retracted) {
        if deadline_exceeded(deadline) {
            return (Vec::new(), true);
        }
        docs.push((i, tokenize(&doc_text(m))));
    }

    let n = docs.len();
    if n == 0 {
        return (Vec::new(), false);
    }

    let avg_dl: f64 = {
        let total: f64 = docs.iter().map(|(_, t)| t.len() as f64).sum();
        let avg = total / n as f64;
        if avg > 0.0 {
            avg
        } else {
            1.0
        }
    };

    // Unique query terms — repeats in the query don't double-count idf.
    let mut seen = std::collections::HashSet::new();
    let unique_q_terms: Vec<&String> = q_tokens
        .iter()
        .filter(|t| seen.insert(t.as_str()))
        .collect();

    let mut idf: HashMap<&str, f64> = HashMap::new();
    for term in &unique_q_terms {
        if deadline_exceeded(deadline) {
            return (Vec::new(), true);
        }
        let df = docs
            .iter()
            .filter(|(_, tokens)| tokens.iter().any(|w| w == *term))
            .count() as f64;
        let val = ((n as f64 - df + 0.5) / (df + 0.5) + 1.0).ln();
        idf.insert(term.as_str(), val);
    }

    let mut results: Vec<(usize, f64)> = Vec::new();
    for (orig_idx, tokens) in &docs {
        if deadline_exceeded(deadline) {
            return (Vec::new(), true);
        }
        let dl = tokens.len() as f64;
        let mut score = 0.0;
        for term in &unique_q_terms {
            let tf = tokens.iter().filter(|w| *w == *term).count() as f64;
            if tf == 0.0 {
                continue;
            }
            let term_idf = *idf.get(term.as_str()).unwrap_or(&0.0);
            score += term_idf * (tf * (BM25_K1 + 1.0))
                / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avg_dl));
        }
        if score >= SCORE_FLOOR {
            results.push((*orig_idx, score));
        }
    }

    results.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    (results, false)
}

/// Cheap deadline check shared by every F2 cooperative loop boundary
/// (`load_effective_checked`, `bm25_rank_checked`, `recall_impl`'s verify
/// walk). `None` (no deadline — every non-hook caller) always returns
/// `false`, so threading `deadline: Option<Instant>` through these
/// functions has zero behavioral effect when unset.
fn deadline_exceeded(deadline: Option<Instant>) -> bool {
    matches!(deadline, Some(dl) if Instant::now() >= dl)
}

/// Hard cap on how many ranked candidates a single `recall` call will
/// freshness-verify (`pin_freshness`, which reads + hashes every pinned
/// file), regardless of `k`. On a corpus where many high-ranked candidates
/// are stale/orphaned, rank-then-verify without a cap would hash an
/// unbounded number of files — unbounded filesystem I/O in the
/// `UserPromptSubmit` injection path, which has a tight (50ms) budget.
///
/// 128 comfortably exceeds any reasonable `k` (recall is used to fill a
/// handful of injected-memory slots, not to page through the corpus) while
/// keeping worst-case verification work bounded and constant regardless of
/// corpus size. Capping out means recall may return *fewer than `k`*
/// results on a mostly-stale corpus — it never weakens INV-M2 (fresh-only):
/// a stale or orphaned candidate is still never returned, it is just never
/// reached.
///
/// F3: hitting this cap while still short of `k` is silent by construction
/// (INV-M2 forbids ever surfacing what's beyond it) — `RecallOutcome::capped`
/// is the load-bearing signal that lets callers say so instead of an
/// indistinguishable "no fresh matches". See `recall_outcome` /
/// `recall_with_deadline`.
pub const RECALL_VERIFY_CAP: usize = 128;

/// Rank-then-verify recall (INV-M2, load-bearing): rank candidates by BM25
/// relevance, then walk the ranking in order verifying each candidate's pin
/// freshness against the current working tree, keeping only `Fresh`
/// memories until `k` are collected. Stale and Orphaned candidates are
/// skipped, never returned, and never block later (lower-ranked)
/// candidates from being checked. Verification is lazy — candidates past
/// the k-th Fresh one are never touched, and the walk never verifies more
/// than `RECALL_VERIFY_CAP` candidates total, even if fewer than `k` Fresh
/// results have been found by then (bounded worst-case I/O).
///
/// Empty query -> no ranking signal, so falls back to the `k` freshest
/// (by `ts`, most-recent-first) non-retracted memories that are Fresh,
/// walked in the same lazy-verify, cap-bounded style.
pub fn recall(root: &Path, query: &str, k: usize) -> Result<Vec<EffectiveMemory>, String> {
    Ok(recall_impl(root, query, k, None)?.hits)
}

/// Non-deadline twin of [`recall`] that surfaces the full [`RecallOutcome`]
/// (F3) — specifically `capped`, so a caller (the CLI `recall`/`memories`
/// commands, the `--for-hook` deadline path) can tell "the verify walk
/// examined the whole ranking" apart from "it stopped at
/// [`RECALL_VERIFY_CAP`] with candidates left unchecked", and warn rather
/// than silently presenting a possibly-incomplete result set as if it were
/// exhaustive. `budget_exceeded` is always `false` here (no deadline is
/// passed) — kept on the shared struct only so `recall_with_deadline` and
/// this function can return the same type.
pub fn recall_outcome(root: &Path, query: &str, k: usize) -> Result<RecallOutcome, String> {
    recall_impl(root, query, k, None)
}

/// Result of a deadline-bounded [`recall_with_deadline`] call (F2), also
/// returned by the non-deadline [`recall_outcome`] (F3).
/// `budget_exceeded` is set iff the cooperative deadline tripped before the
/// walk completed — when it is `true`, `hits` is always empty (a hard
/// bail, never a partial result, so a caller can't confuse "budget blown"
/// with "genuinely no matches").
///
/// `capped` (F3) is set iff the verify walk hit [`RECALL_VERIFY_CAP`] with
/// at least one candidate still unexamined — i.e. `hits` may be missing
/// Fresh matches that exist beyond the cap. It is only ever `true` when
/// `hits.len() < k` (a caller who already got their `k` results was not
/// truncated, regardless of what lies further down the ranking) and is
/// always `false` on a `budget_exceeded` outcome (a deadline bail never
/// walked far enough to distinguish "capped" from any other reason it
/// stopped early). See `recall_impl`'s loop for the exact boundary.
pub struct RecallOutcome {
    pub hits: Vec<EffectiveMemory>,
    pub budget_exceeded: bool,
    pub capped: bool,
}

/// Deadline-bounded [`recall`] (F2, founder-decided option (a) — a hard
/// cooperative budget, not the old measure-after-the-fact suppression).
/// Identical rank-then-verify semantics to `recall`, except every
/// `O(corpus)` pass — `load_effective`'s fold, `bm25_rank`'s scoring, and
/// this function's own freshness-verify walk — checks `deadline` at each
/// loop boundary and bails out to an empty, `budget_exceeded: true` result
/// the instant `Instant::now() >= deadline`, rather than running the full
/// (potentially unboundedly slow) computation to completion and only
/// discarding the *output* afterward. Intended for exactly one caller: the
/// `UserPromptSubmit` hook's injection path (`cmds::inject_memory`), which
/// must never let recall delay the user's prompt past its budget.
pub fn recall_with_deadline(
    root: &Path,
    query: &str,
    k: usize,
    deadline: Instant,
) -> Result<RecallOutcome, String> {
    recall_impl(root, query, k, Some(deadline))
}

fn recall_impl(
    root: &Path,
    query: &str,
    k: usize,
    deadline: Option<Instant>,
) -> Result<RecallOutcome, String> {
    if deadline_exceeded(deadline) {
        return Ok(RecallOutcome {
            hits: Vec::new(),
            budget_exceeded: true,
            capped: false,
        });
    }

    let (all, load_exceeded) = load_effective_checked(root, deadline)?;
    if load_exceeded {
        return Ok(RecallOutcome {
            hits: Vec::new(),
            budget_exceeded: true,
            capped: false,
        });
    }

    let ordered_candidates: Vec<&EffectiveMemory> = if tokenize(query).is_empty() {
        let mut candidates: Vec<&EffectiveMemory> = all.iter().filter(|m| !m.retracted).collect();
        candidates.sort_by_key(|m| std::cmp::Reverse(m.ts));
        candidates
    } else {
        let (ranked, rank_exceeded) = bm25_rank_checked(&all, query, deadline);
        if rank_exceeded {
            return Ok(RecallOutcome {
                hits: Vec::new(),
                budget_exceeded: true,
                capped: false,
            });
        }
        ranked.into_iter().map(|(idx, _score)| &all[idx]).collect()
    };

    // F3: `capped` must record WHY the walk stopped, not just THAT it
    // stopped — `out.len() >= k` ("caller got everything they asked for")
    // is checked first and is never a truncation; only reaching index
    // `RECALL_VERIFY_CAP` while still hungry (`i >= RECALL_VERIFY_CAP` is
    // only reachable when the prior check didn't already break) proves an
    // unexamined candidate exists beyond the cap. A combined `||` condition
    // (the pre-F3 shape) can't distinguish the two, which is exactly how
    // "zero fresh matches" and "zero fresh matches AND more exist beyond
    // the cap" became indistinguishable to callers.
    let mut out = Vec::new();
    let mut capped = false;
    for (i, m) in ordered_candidates.into_iter().enumerate() {
        if out.len() >= k {
            break;
        }
        if i >= RECALL_VERIFY_CAP {
            capped = true;
            break;
        }
        if deadline_exceeded(deadline) {
            return Ok(RecallOutcome {
                hits: Vec::new(),
                budget_exceeded: true,
                capped: false,
            });
        }
        if pin_freshness(root, &m.pins) == Freshness::Fresh {
            out.push(m.clone());
        }
    }
    Ok(RecallOutcome {
        hits: out,
        budget_exceeded: false,
        capped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn pin(path: &str, hash: &str) -> Pin {
        Pin {
            path: path.to_string(),
            hash: format!("sha256:{hash}"),
        }
    }

    fn rec(id: &str, op: MemoryOp, fact: &str, pins: Vec<Pin>, ts: u64) -> MemoryRecord {
        MemoryRecord {
            v: 1,
            kind: "memory".to_string(),
            id: id.to_string(),
            op,
            fact: fact.to_string(),
            pins,
            source_turns: vec![],
            origin: "human".to_string(),
            ts,
            reason: None,
        }
    }

    // INV-M5: fold determinism. assert(id=A, fact F1) + reverify(id=A, new
    // pins) + retract(id=A), written to the log in every permutation of line
    // order but with fixed, ascending `ts` values — load_effective must fold
    // by `ts` (not file order), so every permutation yields the identical
    // effective state: retracted=true, pins = reverify's pins, fact = F1.
    #[test]
    fn fold_latest_op_wins_any_order() {
        let assert_rec = rec(
            "A",
            MemoryOp::Assert,
            "F1",
            vec![pin("src/a.rs", "aaaa")],
            1_000,
        );
        let reverify_rec = rec(
            "A",
            MemoryOp::Reverify,
            "F1",
            vec![pin("src/a.rs", "bbbb"), pin("src/b.rs", "cccc")],
            2_000,
        );
        let retract_rec = rec(
            "A",
            MemoryOp::Retract,
            "F1",
            vec![pin("src/a.rs", "bbbb")],
            3_000,
        );
        let all = [assert_rec, reverify_rec, retract_rec];

        // Every permutation of the 3 records, written to a fresh tempdir
        // memory.jsonl each time.
        let perms: [[usize; 3]; 6] = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];
        for perm in perms {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            for &idx in &perm {
                append_memory(root, &all[idx]).unwrap();
            }
            let effective = load_effective(root).unwrap();
            assert_eq!(effective.len(), 1, "perm {perm:?}");
            let m = &effective[0];
            assert_eq!(m.id, "A", "perm {perm:?}");
            assert_eq!(m.fact, "F1", "perm {perm:?}");
            assert_eq!(
                m.pins,
                vec![pin("src/a.rs", "bbbb"), pin("src/b.rs", "cccc")],
                "perm {perm:?}: pins must be reverify's pins"
            );
            assert!(m.retracted, "perm {perm:?}: must be retracted");
            assert_eq!(m.ts, 3_000, "perm {perm:?}: ts of latest op");
        }
    }

    // INV-M5, equal-ts case: two records for the same id sharing the exact
    // same `ts` must still fold deterministically regardless of file order.
    // Tie-break rule: retract > reverify > assert — a retraction at the same
    // ts as an assert is never lost to file order.
    #[test]
    fn fold_equal_ts_retract_wins_any_order() {
        let assert_rec = rec(
            "A",
            MemoryOp::Assert,
            "F1",
            vec![pin("src/a.rs", "aaaa")],
            1_000,
        );
        let retract_rec = rec(
            "A",
            MemoryOp::Retract,
            "F1",
            vec![pin("src/a.rs", "aaaa")],
            1_000,
        );

        // Ordering 1: assert then retract (file order matches ts-tie winner).
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            append_memory(root, &assert_rec).unwrap();
            append_memory(root, &retract_rec).unwrap();
            let effective = load_effective(root).unwrap();
            assert_eq!(effective.len(), 1);
            assert!(
                effective[0].retracted,
                "assert-then-retract at equal ts must retract"
            );
        }

        // Ordering 2: retract then assert (file order opposes ts-tie winner
        // — this is the case a stable sort on ts alone gets wrong).
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            append_memory(root, &retract_rec).unwrap();
            append_memory(root, &assert_rec).unwrap();
            let effective = load_effective(root).unwrap();
            assert_eq!(effective.len(), 1);
            assert!(
                effective[0].retracted,
                "retract-then-assert at equal ts must still retract (retract wins ties)"
            );
        }
    }

    // Equal-ts tie between reverify and assert: reverify's pins must win
    // over a same-ts assert regardless of file order (reverify outranks
    // assert in the tie-break precedence).
    #[test]
    fn fold_equal_ts_reverify_wins_over_assert_any_order() {
        let assert_rec = rec(
            "A",
            MemoryOp::Assert,
            "F1",
            vec![pin("src/a.rs", "aaaa")],
            1_000,
        );
        let reverify_rec = rec(
            "A",
            MemoryOp::Reverify,
            "F1",
            vec![pin("src/a.rs", "bbbb")],
            1_000,
        );

        for (first, second) in [
            (assert_rec.clone(), reverify_rec.clone()),
            (reverify_rec.clone(), assert_rec.clone()),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            append_memory(root, &first).unwrap();
            append_memory(root, &second).unwrap();
            let effective = load_effective(root).unwrap();
            assert_eq!(effective.len(), 1);
            assert_eq!(
                effective[0].pins,
                vec![pin("src/a.rs", "bbbb")],
                "reverify's pins must win over a same-ts assert regardless of file order"
            );
        }
    }

    // INV-M5 literal: `(ts, op_rank)` is not a total order by itself. Two
    // `reverify` records for the *same* id, at the *same* ts, with
    // *different* pins tie on both `ts` and `op_rank` — a sort keyed only
    // on those (Vec::sort_by_key is stable) falls back to file/append
    // order for the tie, so reversing the write order flips which pins
    // win. INV-M5 says "same records in ANY order produce the same
    // effective state" — that is a literal violation. The `content_key`
    // tiebreak must make the fold pick the same winner regardless of which
    // record was written first.
    #[test]
    fn fold_equal_ts_same_op_reverify_deterministic_any_order() {
        let rev_a = rec(
            "A",
            MemoryOp::Reverify,
            "F1",
            vec![pin("src/a.rs", "aaaa")],
            1_000,
        );
        let rev_b = rec(
            "A",
            MemoryOp::Reverify,
            "F1",
            vec![pin("src/b.rs", "bbbb")],
            1_000,
        );

        let mut winners = Vec::new();
        for (first, second) in [
            (rev_a.clone(), rev_b.clone()),
            (rev_b.clone(), rev_a.clone()),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            append_memory(root, &first).unwrap();
            append_memory(root, &second).unwrap();
            let effective = load_effective(root).unwrap();
            assert_eq!(effective.len(), 1);
            winners.push(effective[0].pins.clone());
        }
        assert_eq!(
            winners[0], winners[1],
            "same id/ts/op reverify records with different pins must fold \
             to the identical winner regardless of file order: {winners:?}"
        );
    }

    // Bad-line tolerance: a line with unknown extra fields still parses; a
    // malformed JSON line is skipped (counted, not fatal); a record whose
    // `op` is an unrecognized string is skipped. load_effective still
    // returns every good record's effective state.
    #[test]
    fn fold_ignores_unknown_fields_and_bad_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let path = memory_path(root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        let good_with_extra = concat!(
            r#"{"v":1,"type":"memory","id":"B","op":"assert","fact":"F2","#,
            r#""pins":[{"path":"src/c.rs","hash":"sha256:dddd"}],"#,
            r#""source_turns":[],"origin":"agent","ts":5000,"future_field":"whatever"}"#
        );
        let malformed = "{not json at all";
        let unknown_op = concat!(
            r#"{"v":1,"type":"memory","id":"C","op":"bogus","fact":"F3","#,
            r#""pins":[{"path":"src/d.rs","hash":"sha256:eeee"}],"#,
            r#""source_turns":[],"origin":"human","ts":6000}"#
        );
        let another_good = concat!(
            r#"{"v":1,"type":"memory","id":"D","op":"assert","fact":"F4","#,
            r#""pins":[{"path":"src/e.rs","hash":"sha256:ffff"}],"#,
            r#""source_turns":[],"origin":"human","ts":7000}"#
        );

        std::fs::write(
            &path,
            format!("{good_with_extra}\n{malformed}\n{unknown_op}\n{another_good}\n"),
        )
        .unwrap();

        let effective = load_effective(root).unwrap();
        let mut ids: Vec<&str> = effective.iter().map(|m| m.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["B", "D"], "only the two good records survive");

        let b = effective.iter().find(|m| m.id == "B").unwrap();
        assert_eq!(b.fact, "F2");
        assert!(!b.retracted);
    }

    // append_memory validation: fact over FACT_MAX_CHARS is rejected naming
    // the cap; pins empty or over PINS_MAX is rejected; a fact that scrubs
    // down to nothing (all content redacted) is rejected with "scrub" in the
    // error, never silently stored as an empty husk.
    #[test]
    fn append_memory_rejects_oversize_and_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let oversize_fact = "x".repeat(FACT_MAX_CHARS + 1);
        let err = append_memory(
            root,
            &rec(
                "E1",
                MemoryOp::Assert,
                &oversize_fact,
                vec![pin("src/f.rs", "1111")],
                1,
            ),
        )
        .unwrap_err();
        assert!(
            err.contains(&FACT_MAX_CHARS.to_string()),
            "error must name the cap: {err}"
        );

        let err =
            append_memory(root, &rec("E2", MemoryOp::Assert, "fine fact", vec![], 2)).unwrap_err();
        assert!(err.contains("pin"), "empty-pins error: {err}");

        let too_many_pins: Vec<Pin> = (0..PINS_MAX + 1)
            .map(|i| pin(&format!("src/f{i}.rs"), "2222"))
            .collect();
        let err = append_memory(
            root,
            &rec("E3", MemoryOp::Assert, "fine fact", too_many_pins, 3),
        )
        .unwrap_err();
        assert!(err.contains("pin"), "too-many-pins error: {err}");

        // scrub() never deletes matched content — it replaces it with a
        // `[redacted:...]` marker — so the only way `scrub(fact).trim()`
        // comes back empty is a fact that was already blank going in. That
        // is exactly the "stored husk" the design spec forbids.
        let blank_fact = "   \n\t  ";
        let err = append_memory(
            root,
            &rec(
                "E4",
                MemoryOp::Assert,
                blank_fact,
                vec![pin("src/f.rs", "3333")],
                4,
            ),
        )
        .unwrap_err();
        assert!(err.contains("scrub"), "scrub-empty error: {err}");

        // A well-formed record is accepted and lands on disk.
        append_memory(
            root,
            &rec(
                "E5",
                MemoryOp::Assert,
                "the daemon uses signal-tailer offsets",
                vec![pin("src/g.rs", "4444")],
                5,
            ),
        )
        .unwrap();
        let effective = load_effective(root).unwrap();
        assert!(effective.iter().any(|m| m.id == "E5"));
    }

    // INV-M1 core: pin path validation rejects absolute paths, `..`
    // traversal (however it normalizes), symlink escape out of root, and
    // secret-file names — each with an error mentioning the offending
    // path/reason. A nonexistent file is rejected (can't hash what isn't
    // there). A legitimate in-root file normalizes to a root-relative path.
    #[test]
    fn pin_path_rejections() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/ok.rs"), b"fn main() {}").unwrap();

        let err = validate_pin_path(root, "/etc/passwd").unwrap_err();
        assert!(err.contains("/etc/passwd"), "absolute-path error: {err}");

        let err = validate_pin_path(root, "../x").unwrap_err();
        assert!(err.contains("../x"), "traversal error: {err}");

        let err = validate_pin_path(root, "a/../../x").unwrap_err();
        assert!(err.contains("a/../../x"), "traversal error: {err}");

        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("secret.rs"), b"outside").unwrap();
            std::os::unix::fs::symlink(outside.path().join("secret.rs"), root.join("escape.rs"))
                .unwrap();
            let err = validate_pin_path(root, "escape.rs").unwrap_err();
            assert!(err.contains("escape.rs"), "symlink-escape error: {err}");
        }

        fs::write(root.join(".env"), b"SECRET=1").unwrap();
        let err = validate_pin_path(root, ".env").unwrap_err();
        assert!(err.contains("secret"), "secret-path error: {err}");

        fs::write(root.join("key.pem"), b"-----BEGIN-----").unwrap();
        let err = validate_pin_path(root, "key.pem").unwrap_err();
        assert!(err.contains("secret"), "secret-path error: {err}");

        let err = validate_pin_path(root, "ghost.rs").unwrap_err();
        assert!(err.contains("ghost.rs"), "nonexistent-file error: {err}");

        let ok = validate_pin_path(root, "src/ok.rs").unwrap();
        assert_eq!(ok, "src/ok.rs");
    }

    // INV-M1 core: freshness is derived, never persisted. Pinning a file
    // (hash now) yields Fresh; overwriting its content yields Stale;
    // deleting it yields Orphaned (checked before Stale); with two pins,
    // changing only one still yields Stale for the whole set.
    #[test]
    fn freshness_transitions() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.rs"), b"fn a() {}").unwrap();
        fs::write(root.join("b.rs"), b"fn b() {}").unwrap();

        let hash_a = hash_pin(root, "a.rs").unwrap();
        let hash_b = hash_pin(root, "b.rs").unwrap();

        let one_pin = vec![Pin {
            path: "a.rs".to_string(),
            hash: hash_a.clone(),
        }];
        assert_eq!(pin_freshness(root, &one_pin), Freshness::Fresh);

        fs::write(root.join("a.rs"), b"fn a() { changed(); }").unwrap();
        assert_eq!(pin_freshness(root, &one_pin), Freshness::Stale);

        fs::remove_file(root.join("a.rs")).unwrap();
        assert_eq!(pin_freshness(root, &one_pin), Freshness::Orphaned);

        let two_pins = vec![
            Pin {
                path: "a.rs".to_string(),
                hash: hash_a,
            },
            Pin {
                path: "b.rs".to_string(),
                hash: hash_b,
            },
        ];
        // a.rs deleted above -> Orphaned takes priority over b.rs still
        // matching.
        assert_eq!(pin_freshness(root, &two_pins), Freshness::Orphaned);

        fs::write(root.join("a.rs"), b"fn a() {}").unwrap();
        fs::write(root.join("b.rs"), b"fn b() { changed(); }").unwrap();
        assert_eq!(pin_freshness(root, &two_pins), Freshness::Stale);
    }

    // Helper: write `content` to `root/rel`, hash it, append an assert
    // record pinning it with `fact`, return the memory id used.
    fn seed_memory(root: &Path, id: &str, fact: &str, rel: &str, content: &[u8], ts: u64) {
        fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        fs::write(root.join(rel), content).unwrap();
        let hash = hash_pin(root, rel).unwrap();
        append_memory(
            root,
            &rec(id, MemoryOp::Assert, fact, vec![pin(rel, &hash[7..])], ts),
        )
        .unwrap();
    }

    // INV-M2 core: recall never returns a memory whose pins are Stale (or
    // Orphaned). Two memories, both on-topic for the query, both start
    // Fresh -> recall returns both. Mutating one pinned file's content makes
    // that memory Stale -> recall now returns exactly the other, even
    // though the mutated one still ranks (BM25 doesn't see file content).
    // A handful of off-topic filler memories pad the corpus so the shared
    // query terms clear SCORE_FLOOR (idf shrinks toward the floor as df
    // approaches N in a tiny two-doc corpus — see task-3 BM25-floor notes).
    #[test]
    fn recall_never_returns_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        seed_memory(
            root,
            "A",
            "nightly seed rotation keeps torture runs reproducible",
            "a.rs",
            b"fn a() {}",
            1_000,
        );
        seed_memory(
            root,
            "B",
            "nightly seed also drives the fuzz corpus replay",
            "b.rs",
            b"fn b() {}",
            2_000,
        );
        // Off-topic filler: shares no vocabulary with the query, exists
        // only to grow corpus N so idf(nightly)/idf(seed) clear the floor.
        for i in 0..8 {
            seed_memory(
                root,
                &format!("filler{i}"),
                "unrelated documentation cleanup housekeeping chore",
                &format!("filler{i}.rs"),
                b"fn filler() {}",
                3_000 + i as u64,
            );
        }

        let found = recall(root, "nightly seed", 2).unwrap();
        let mut ids: Vec<&str> = found.iter().map(|m| m.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["A", "B"], "both fresh matches recalled");

        // Mutate A's pinned file -> Stale.
        fs::write(root.join("a.rs"), b"fn a() { changed(); }").unwrap();

        let found = recall(root, "nightly seed", 2).unwrap();
        assert_eq!(found.len(), 1, "stale memory must be excluded: {found:?}");
        assert_eq!(found[0].id, "B");
    }

    // BM25 ranking sanity: a fact specifically about the query terms outranks
    // generic facts sharing no vocabulary, and a nonsense query that matches
    // nothing floors to empty (INV-M2's sibling: rank-then-verify starts
    // from a ranking that actually discriminates).
    #[test]
    fn bm25_ranks_specific_over_generic_and_floors_noise() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        seed_memory(
            root,
            "torture",
            "nightly torture seed is derived from wall clock per run",
            "cli/tests/torture.rs",
            b"fn torture() {}",
            1_000,
        );
        seed_memory(
            root,
            "install",
            "install script verifies checksum before placing the binary",
            "install.sh",
            b"echo install",
            2_000,
        );
        seed_memory(
            root,
            "doctor",
            "doctor checks inotify watch headroom on linux",
            "cli/src/doctorcmd.rs",
            b"fn doctor() {}",
            3_000,
        );

        let corpus = load_effective(root).unwrap();
        let ranked = bm25_rank(&corpus, "nightly torture seed");
        assert!(
            !ranked.is_empty(),
            "expected at least one match above floor"
        );
        assert!(
            ranked[0].1 >= SCORE_FLOOR,
            "top score must clear the floor: {ranked:?}"
        );
        let top_id = &corpus[ranked[0].0].id;
        assert_eq!(
            top_id, "torture",
            "specific fact must rank first: {ranked:?}"
        );

        let noise = bm25_rank(&corpus, "zebra quantum");
        assert!(
            noise.is_empty(),
            "nonsense query must floor to empty: {noise:?}"
        );
    }

    // Rank-then-verify shape: a large corpus where only 3 memories are
    // relevant to the query and fresh; the other ~40 don't match the query
    // vocabulary at all (score under the floor) and — as a trap for a
    // buggy implementation that verified everything regardless of rank —
    // point at files that have been deleted (Orphaned). recall(k=3) must
    // return exactly the 3 fresh matches: the orphaned noise never needed
    // to be (and, if ranking is correct, never is) touched.
    #[test]
    fn recall_verifies_only_top_candidates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        for i in 0..3 {
            seed_memory(
                root,
                &format!("relevant{i}"),
                "kraken telemetry batching flushes every ninety seconds",
                &format!("relevant{i}.rs"),
                format!("fn relevant{i}() {{}}").as_bytes(),
                1_000 + i as u64,
            );
        }
        for i in 0..40 {
            let rel = format!("noise{i}.rs");
            seed_memory(
                root,
                &format!("noise{i}"),
                "generic housekeeping note about formatting whitespace",
                &rel,
                b"fn noise() {}",
                2_000 + i as u64,
            );
            fs::remove_file(root.join(&rel)).unwrap();
        }

        let found = recall(root, "kraken telemetry batching", 3).unwrap();
        assert_eq!(found.len(), 3, "expected exactly the 3 relevant matches");
        let mut ids: Vec<&str> = found.iter().map(|m| m.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["relevant0", "relevant1", "relevant2"]);
    }

    // FIX B: rank-then-verify's I/O must be bounded by RECALL_VERIFY_CAP,
    // not by k or corpus size. Corpus is built so the orphaned block and
    // the fresh block get an *identical* BM25 score (same 2-token fact
    // "kraken telemetry", same doc length, so tf/dl normalization is
    // identical) — `bm25_rank`'s idx-ascending tiebreak then makes
    // insertion order decide rank order, letting this test place a large,
    // deterministically-ordered orphaned block strictly ahead of the fresh
    // one. Filler docs (disjoint vocabulary) pad total corpus size N so
    // idf doesn't collapse toward zero when hundreds of docs share the
    // query vocabulary (BM25 idf ~ ln((N-df+0.5)/(df+0.5)+1) — needs N
    // meaningfully larger than df to clear SCORE_FLOOR).
    //
    // Scenario 1 (orphaned count << cap): sanity check on the construction
    // itself — with nothing to exhaust the cap, recall must still find all
    // 3 fresh memories. This isolates the cap (not some scoring artifact
    // of the identical-fact construction) as the cause of scenario 2.
    //
    // Scenario 2 (orphaned count > RECALL_VERIFY_CAP, all ranked strictly
    // ahead of the 3 fresh ones): recall's verification walk exhausts its
    // cap entirely inside the orphaned block and never reaches the fresh
    // one, so it returns *zero* results — even though 3 genuinely fresh,
    // on-topic memories exist in the corpus. Before the Fix B cap, this
    // exact construction returns all 3 (an unbounded walk eventually skips
    // past every orphaned candidate and reaches them); this test fails
    // against the pre-fix code with `found.len() == 3`, not empty. INV-M2
    // (fresh-only) holds in both scenarios — capping only ever removes
    // results, it never returns a stale/orphaned one.
    //
    // What this test does NOT prove: it doesn't instrument `pin_freshness`
    // call counts directly, so it's an outcome-level (not an
    // instrumentation-level) proof of the cap. The deterministic
    // score-tie + insertion-order construction makes that outcome-level
    // proof exact rather than probabilistic, which is why no
    // instrumentation was added.
    //
    // F3 extension: both scenarios now also assert `RecallOutcome::capped`
    // via `recall_outcome` — scenario 1 (nothing exhausts the cap) must
    // report `capped: false`; scenario 2 (the cap is exhausted entirely
    // inside the orphaned block, 3 genuinely fresh matches left unreached)
    // must report `capped: true`, so a caller can distinguish "verified
    // everything, genuinely nothing fresh" from "stopped early, may have
    // missed fresh matches" instead of both collapsing into an identical
    // empty `Vec`. Pre-F3 this doesn't compile (`RecallOutcome`/
    // `recall_outcome` didn't exist) — the closest RED equivalent is the
    // CLI-level `recall_notice_appears_when_verification_capped`
    // (`cli/tests/integration.rs`), which drives the same construction
    // through the real binary and fails on a plain string-absence
    // assertion against pre-fix code.
    #[test]
    fn recall_bounds_verification_on_stale_heavy_corpus() {
        fn seed_orphaned(root: &Path, id: &str, rel: &str, ts: u64) {
            seed_memory(root, id, "kraken telemetry", rel, b"x", ts);
            fs::remove_file(root.join(rel)).unwrap();
        }
        fn seed_filler(root: &Path, id: &str, rel: &str, ts: u64) {
            seed_memory(root, id, "unrelated housekeeping", rel, b"y", ts);
        }

        // Scenario 1: orphaned block comfortably below the cap.
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            for i in 0..5 {
                seed_orphaned(root, &format!("orph{i}"), &format!("orph{i}.rs"), 1_000 + i);
            }
            for i in 0..3 {
                seed_memory(
                    root,
                    &format!("fresh{i}"),
                    "kraken telemetry",
                    &format!("fresh{i}.rs"),
                    b"z",
                    2_000 + i,
                );
            }
            for i in 0..20 {
                seed_filler(
                    root,
                    &format!("filler{i}"),
                    &format!("filler{i}.rs"),
                    3_000 + i,
                );
            }

            let found = recall(root, "kraken telemetry", 3).unwrap();
            assert_eq!(
                found.len(),
                3,
                "below-cap sanity check: nothing but the cap should keep these 3 from being found"
            );

            let outcome = recall_outcome(root, "kraken telemetry", 3).unwrap();
            assert_eq!(outcome.hits.len(), 3);
            assert!(
                !outcome.capped,
                "walk never reached RECALL_VERIFY_CAP — must not report capped"
            );
        }

        // Scenario 2: orphaned block exceeds RECALL_VERIFY_CAP, ranked
        // strictly ahead of the 3 fresh candidates.
        {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            let orphaned_count = RECALL_VERIFY_CAP as u64 + 12;
            for i in 0..orphaned_count {
                seed_orphaned(root, &format!("orph{i}"), &format!("orph{i}.rs"), 1_000 + i);
            }
            for i in 0..3 {
                seed_memory(
                    root,
                    &format!("fresh{i}"),
                    "kraken telemetry",
                    &format!("fresh{i}.rs"),
                    b"z",
                    2_000 + i,
                );
            }
            for i in 0..100 {
                seed_filler(
                    root,
                    &format!("filler{i}"),
                    &format!("filler{i}.rs"),
                    3_000 + i,
                );
            }

            let found = recall(root, "kraken telemetry", 3).unwrap();
            assert!(
                found.is_empty(),
                "verification cap must be exhausted inside the orphaned \
                 block before ever reaching the fresh one: {found:?}"
            );

            let outcome = recall_outcome(root, "kraken telemetry", 3).unwrap();
            assert!(outcome.hits.is_empty());
            assert!(
                outcome.capped,
                "walk exhausted RECALL_VERIFY_CAP with the 3 fresh matches \
                 still unreached — must report capped: true"
            );
        }
    }

    // F2: the cooperative deadline is a HARD bail, not the old
    // measure-after-the-fact suppression — proven by injecting a deadline
    // that is already in the past (deterministic, no runner-speed
    // coupling) against a corpus that would otherwise recall real, fresh
    // hits. `recall` (the un-deadlined, pre-F2-shaped call) still returns
    // the real match — the only thing that changed is that
    // `recall_with_deadline` now has a way to bail *before* doing any of
    // that work, which `recall`'s old retrospective-only budget check
    // (measured after the call returned) could never express: an
    // already-expired deadline passed in has nothing to cooperate with
    // that pre-F2 code, because pre-F2 there was no deadline parameter at
    // all, so this exact assertion could not even be written against HEAD.
    #[test]
    fn recall_with_deadline_bails_when_already_expired() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        seed_memory(
            root,
            "A",
            "nightly seed rotation keeps torture runs reproducible",
            "a.rs",
            b"fn a() {}",
            1_000,
        );
        // Off-topic filler, same reason as `recall_never_returns_stale`:
        // pads corpus N so idf(nightly)/idf(seed) clears SCORE_FLOOR.
        for i in 0..8 {
            seed_memory(
                root,
                &format!("filler{i}"),
                "unrelated documentation cleanup housekeeping chore",
                &format!("filler{i}.rs"),
                b"fn filler() {}",
                2_000 + i as u64,
            );
        }

        // Sanity: with no deadline, this corpus really does recall a hit —
        // proves the "empty" result below is caused by the expired
        // deadline, not by the corpus being unmatchable.
        let undeadlined = recall(root, "nightly seed", 5).unwrap();
        assert_eq!(
            undeadlined.len(),
            1,
            "sanity: query must match without a deadline"
        );

        let already_expired = Instant::now() - Duration::from_secs(1);
        let outcome = recall_with_deadline(root, "nightly seed", 5, already_expired).unwrap();
        assert!(
            outcome.budget_exceeded,
            "an already-expired deadline must report budget_exceeded"
        );
        assert!(
            outcome.hits.is_empty(),
            "a budget_exceeded outcome must never carry partial hits: {:?}",
            outcome.hits
        );
    }

    // Empty-query fallback path (freshest-first, no BM25) is also bounded
    // by the deadline — an already-expired deadline bails before even the
    // `load_effective` fold, regardless of which ranking branch would have
    // run next.
    #[test]
    fn recall_with_deadline_bails_on_empty_query_fallback_too() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        seed_memory(root, "A", "anything", "a.rs", b"fn a() {}", 1_000);

        let already_expired = Instant::now() - Duration::from_secs(1);
        let outcome = recall_with_deadline(root, "", 5, already_expired).unwrap();
        assert!(outcome.budget_exceeded);
        assert!(outcome.hits.is_empty());
    }
}
