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

/// Load every parseable record, fold per id (latest op wins, ordered by
/// `ts` — ties broken by file order), and return one `EffectiveMemory` per
/// id. Absent file = empty vec (never an error — a repo with no memories
/// yet is a normal state, not a failure). Malformed lines (bad JSON, or an
/// `op` string this build doesn't recognize) are skipped, never fatal —
/// mirrors `record::load_log`'s tolerance.
pub fn load_effective(root: &Path) -> Result<Vec<EffectiveMemory>, String> {
    let path = memory_path(root);
    let records = match fs::File::open(&path) {
        Ok(file) => {
            let reader = std::io::BufReader::new(file);
            let mut out = Vec::new();
            for line in reader.lines() {
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
        Err(_) => return Ok(vec![]),
    };

    // Group by id, preserving file (insertion) order within each group —
    // the tie-break for equal `ts` values.
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
        let mut group = by_id.remove(&id).unwrap_or_default();
        // Stable sort: equal `ts` values keep their original (file) order.
        group.sort_by_key(|r| r.ts);

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
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
