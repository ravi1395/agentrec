//! Line-range attribution for the `annotate` verb (Phase 3.0 T4, spec
//! `docs/superpowers/specs/2026-08-07-phase-3-design.md` §3.0.3).
//!
//! **Git-free by construction.** This module never spawns, links, or parses
//! git: it consumes an already-parsed `Vec<BlameRange>` (the CLI layer owns
//! `git blame --porcelain`) and joins it against turn records + CAS blobs.
//! That is the plan's stated seam — core stays git-free, mirroring how the
//! daemon classifies git turns without shelling out.
//!
//! **Precision contract (normative, spec §3.0.3, plan decision 5).**
//! Attribution is file-and-time certain; line-level it is BEST-EFFORT and
//! nothing here may be worded otherwise. Turns store file-level before/after
//! snapshots, not per-line provenance: the line set a turn "wrote" is derived
//! by diffing its `before` against its `after` and taking the new-side indices
//! of inserted lines. Renames, reformat-only commits, and interleaved
//! human+agent edits within one file between two commits WILL misattribute at
//! line granularity, because a later edit shifts every subsequent line number
//! and this mapping has no way to follow it. [`AnnotateResult::line_precision`]
//! is the machine-readable form of that admission; the CLI renders the prose
//! form.
//!
//! **Precedence for one blamed range, in this exact order** (each arm has its
//! own test):
//! 1. some turn's derived line set intersects the range → [`Attribution::Turns`]
//!    (a Vec: several turns can overlap one range, spec "which turn(s)");
//! 2. else, some turn touched the path but its snapshot blob does not resolve
//!    → [`Attribution::Unattributable`] with [`UnattributableReason::BlobEvicted`],
//!    counted in [`AnnotateResult::evicted_ranges`]. Branching is on the blob
//!    failing to resolve, never on WHY (TTL eviction and corruption are the
//!    same fact to a reader);
//! 3. else, the ledger carries any recording gap at all → [`Attribution::Human`]
//!    would be a guess, so [`UnattributableReason::Gap`]. This mirrors
//!    `view.rs`'s shipped `BlameState::LineOriginGap` predicate
//!    (`has_gap_after(records, "")`) rather than inventing a second gap
//!    semantics in core: a line added during an uncovered interval and later
//!    folded into some turn's unchanged `before` is invisible, so "no turn
//!    claims it" cannot be read as "a human wrote it" while any gap exists;
//! 4. else → [`Attribution::Human`].

use crate::record::{LogRecord, TurnRecord};
use crate::store::BlobStore;
use crate::view::{has_gap_after, Ledger};
use serde::Serialize;
use similar::{ChangeTag, TextDiff};
use std::collections::BTreeSet;
use std::path::Path;

/// The literal value of [`AnnotateResult::line_precision`]. A constant so the
/// renderer, the JSON, and the tests cannot drift.
pub const LINE_PRECISION: &str = "best_effort";

/// One contiguous blamed range, as produced by the CLI's
/// `git blame --porcelain` parse. An INPUT — not `Serialize`, mirroring
/// `SearchQuery`/`StatsOptions`; the produced types are what the wire pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameRange {
    /// Repository-relative path, as git reports it.
    pub path: String,
    /// The commit git blames this range to.
    pub commit: String,
    /// 1-based first line of the range in the range's final file.
    pub start_line: u32,
    /// How many lines the range covers. Zero-length ranges are attributed
    /// like any other (git does not emit them; refusing would be a silent
    /// drop).
    pub line_count: u32,
}

impl BlameRange {
    /// Does 1-based line `line` fall inside this range?
    fn contains_line(&self, line: u32) -> bool {
        line >= self.start_line && line < self.start_line.saturating_add(self.line_count)
    }
}

/// Enough of a turn to name it in output, no more. Deliberately not the whole
/// [`TurnRecord`]: `annotate`'s job is attribution, and echoing every file
/// entry of every overlapping turn would bury it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TurnRef {
    pub id: String,
    pub tool: Option<String>,
    pub model: Option<String>,
    /// The turn's `ended` timestamp, the same field `search` reports as `ts`.
    pub ts: String,
    pub prompt_excerpt: Option<String>,
}

impl TurnRef {
    fn of(t: &TurnRecord) -> Self {
        TurnRef {
            id: t.id.clone(),
            tool: t.tool.clone(),
            model: t.model.clone(),
            ts: t.ended.clone(),
            prompt_excerpt: t.prompt_excerpt.clone(),
        }
    }
}

/// Why a range could not be attributed. String-tagged on the wire so a future
/// reason is an additive VALUE, not a key break (the `rate_bound` precedent
/// from T1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnattributableReason {
    /// The blamed range falls in a history that carries a recording gap, and
    /// no turn claims it — "human" would be a guess.
    Gap,
    /// A turn touched this path but its snapshot blob does not resolve, so no
    /// line mapping can be derived for it.
    BlobEvicted,
}

/// Who wrote a blamed range, to the precision this module can honestly claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attribution {
    /// One or more turns whose derived line set intersects the range. A Vec
    /// because turns overlap; the renderer collapses a one-element Vec.
    Turns {
        turns: Vec<TurnRef>,
    },
    /// Recording covered this history and no turn claims the range.
    Human,
    Unattributable {
        reason: UnattributableReason,
    },
}

/// One blamed range with its verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnnotatedRange {
    pub commit: String,
    pub start_line: u32,
    pub line_count: u32,
    pub attribution: Attribution,
}

/// All ranges blamed within one file, in the order the CLI supplied them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnnotatedFile {
    pub path: String,
    pub ranges: Vec<AnnotatedRange>,
}

/// The `annotate` wire contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnnotateResult {
    pub files: Vec<AnnotatedFile>,
    /// Always [`LINE_PRECISION`]. Present unconditionally — a consumer must
    /// not have to infer the contract from the absence of a key.
    pub line_precision: &'static str,
    /// Ranges that rendered `unattributable (blob evicted)`.
    pub evicted_ranges: u64,
}

/// New-side (1-based) line numbers this before→after transition inserted.
///
/// `Insert` covers pure additions AND the new side of a replacement — the
/// same tag `diff::added_or_changed_lines` selects for `blame`, taken here as
/// indices rather than text.
fn inserted_lines(before: &str, after: &str) -> BTreeSet<u32> {
    TextDiff::from_lines(before, after)
        .iter_all_changes()
        .filter(|c| c.tag() == ChangeTag::Insert)
        .filter_map(|c| c.new_index())
        .map(|i| i as u32 + 1)
        .collect()
}

/// What one turn can say about one path.
enum PathLines {
    /// Derived line set (possibly empty — a delete op writes no lines).
    Lines(BTreeSet<u32>),
    /// A recorded blob hash did not resolve. Branching is on the failure, not
    /// its cause.
    Unresolvable,
    /// This turn does not touch the path at all.
    Untouched,
}

fn turn_lines_for(store: &BlobStore, t: &TurnRecord, path: &str) -> PathLines {
    let Some(entry) = t.files.iter().find(|f| f.path == path) else {
        return PathLines::Untouched;
    };
    // `skipped`/`withheld` entries have no content by design; they are a real
    // touch with no derivable line set, which is exactly the "no line mapping
    // for this turn" shape — not an eviction, and not a claim.
    let load = |hash: &Option<String>| -> Result<String, ()> {
        match hash {
            // A `create` op has no `before`: legitimately-empty text, not an
            // unresolvable blob (view.rs's `blob_text` draws the same line).
            None => Ok(String::new()),
            Some(h) => match store.get(h) {
                Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
                Err(_) => Err(()),
            },
        }
    };
    let (Ok(before), Ok(after)) = (load(&entry.before), load(&entry.after)) else {
        return PathLines::Unresolvable;
    };
    PathLines::Lines(inserted_lines(&before, &after))
}

/// The one fold behind [`crate::view::RepositoryView::annotate`].
///
/// `ranges` arrive already parsed from `git blame --porcelain`; grouping is by
/// path, in first-seen order, so output order is the CLI's order and not a
/// hash-map accident.
pub fn attribute(ledger: &Ledger, objects_dir: &Path, ranges: &[BlameRange]) -> AnnotateResult {
    let store = BlobStore::new(objects_dir);
    let turns: Vec<&TurnRecord> = ledger
        .records
        .iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        })
        .collect();
    // Precedence arm 3's predicate, evaluated once: exactly view.rs's
    // `BlameState::LineOriginGap` test.
    let any_gap = has_gap_after(&ledger.records, "");

    let mut files: Vec<AnnotatedFile> = Vec::new();
    let mut evicted_ranges: u64 = 0;

    for r in ranges {
        let mut claimants: Vec<TurnRef> = Vec::new();
        let mut saw_unresolvable = false;
        for t in &turns {
            match turn_lines_for(&store, t, &r.path) {
                PathLines::Lines(lines) => {
                    if lines.iter().any(|&l| r.contains_line(l)) {
                        claimants.push(TurnRef::of(t));
                    }
                }
                PathLines::Unresolvable => saw_unresolvable = true,
                PathLines::Untouched => {}
            }
        }

        let attribution = if !claimants.is_empty() {
            Attribution::Turns { turns: claimants }
        } else if saw_unresolvable {
            evicted_ranges += 1;
            Attribution::Unattributable {
                reason: UnattributableReason::BlobEvicted,
            }
        } else if any_gap {
            Attribution::Unattributable {
                reason: UnattributableReason::Gap,
            }
        } else {
            Attribution::Human
        };

        let annotated = AnnotatedRange {
            commit: r.commit.clone(),
            start_line: r.start_line,
            line_count: r.line_count,
            attribution,
        };
        match files.iter_mut().find(|f| f.path == r.path) {
            Some(f) => f.ranges.push(annotated),
            None => files.push(AnnotatedFile {
                path: r.path.clone(),
                ranges: vec![annotated],
            }),
        }
    }

    AnnotateResult {
        files,
        line_precision: LINE_PRECISION,
        evicted_ranges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{EpochRecord, FileEntry};

    fn fe(path: &str, before: Option<String>, after: Option<String>, op: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            before,
            after,
            op: op.to_string(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
            link_kind: None,
            attribution: None,
        }
    }

    fn turn(id: &str, files: Vec<FileEntry>) -> TurnRecord {
        TurnRecord {
            v: 1,
            id: id.to_string(),
            grade: "rich".to_string(),
            truncated: false,
            started: "2020-06-01T00:00:00.000Z".to_string(),
            ended: "2020-06-01T00:00:05.000Z".to_string(),
            tool: Some("claude".to_string()),
            model: Some("opus-9".to_string()),
            session: None,
            root: "/repo".to_string(),
            prompt_ref: None,
            prompt_excerpt: Some("make it green".to_string()),
            merges: vec![],
            imported: None,
            files_complete: None,
            origin: None,
            files,
        }
    }

    fn epoch(event: &str, ts: &str) -> LogRecord {
        LogRecord::Epoch(EpochRecord {
            v: 1,
            event: event.to_string(),
            ts: ts.to_string(),
            dropped_signals: 0,
        })
    }

    fn ledger(records: Vec<LogRecord>) -> Ledger {
        Ledger {
            records,
            unknown_type_lines: 0,
            unparsed_lines: 0,
        }
    }

    fn range(path: &str, start: u32, count: u32) -> BlameRange {
        BlameRange {
            path: path.to_string(),
            commit: "c0ffee".to_string(),
            start_line: start,
            line_count: count,
        }
    }

    /// A turn that appended line 3 to a 2-line file. Hand-derived: the diff of
    /// "a\nb\n" → "a\nb\nc\n" inserts exactly new-side index 2, i.e. line 3.
    #[test]
    fn inserted_lines_are_new_side_one_based_indices() {
        let lines = inserted_lines("a\nb\n", "a\nb\nc\n");
        assert_eq!(lines.into_iter().collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn a_turn_claims_only_the_range_its_inserted_lines_intersect() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let store = BlobStore::new(&objects);
        let before = store.put(b"a\nb\n").unwrap();
        let after = store.put(b"a\nb\nc\n").unwrap();
        // Hand-derived: the turn inserted line 3 only.
        let l = ledger(vec![LogRecord::Turn(turn(
            "t_1",
            vec![fe("src/x.rs", Some(before), Some(after), "modify")],
        ))]);

        let out = attribute(
            &l,
            &objects,
            &[range("src/x.rs", 1, 2), range("src/x.rs", 3, 1)],
        );
        assert_eq!(out.line_precision, LINE_PRECISION);
        assert_eq!(out.evicted_ranges, 0);
        assert_eq!(out.files.len(), 1, "one path → one AnnotatedFile group");
        let ranges = &out.files[0].ranges;
        assert_eq!(ranges.len(), 2);
        // Lines 1–2 predate this turn and no gap exists → human.
        assert_eq!(ranges[0].attribution, Attribution::Human);
        // Line 3 is the turn's insert.
        match &ranges[1].attribution {
            Attribution::Turns { turns } => {
                assert_eq!(turns.len(), 1);
                assert_eq!(turns[0].id, "t_1");
                assert_eq!(turns[0].tool.as_deref(), Some("claude"));
                assert_eq!(turns[0].model.as_deref(), Some("opus-9"));
                assert_eq!(turns[0].ts, "2020-06-01T00:00:05.000Z");
                assert_eq!(turns[0].prompt_excerpt.as_deref(), Some("make it green"));
            }
            other => panic!("expected Turns, got {other:?}"),
        }
    }

    /// Spec §3.0.3 says "which turn(s)" — the Vec is load-bearing, so two
    /// turns both inserting into one blamed range must BOTH be named. A
    /// single-turn-wins implementation fails this.
    #[test]
    fn two_turns_overlapping_one_range_are_both_named() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let store = BlobStore::new(&objects);
        let v0 = store.put(b"a\nb\nc\nd\n").unwrap();
        // t_1 rewrites line 2 → new-side line 2 is an Insert.
        let v1 = store.put(b"a\nB\nc\nd\n").unwrap();
        // t_2 rewrites line 3 → new-side line 3 is an Insert.
        let v2 = store.put(b"a\nB\nC\nd\n").unwrap();
        let l = ledger(vec![
            LogRecord::Turn(turn(
                "t_1",
                vec![fe("src/x.rs", Some(v0), Some(v1.clone()), "modify")],
            )),
            LogRecord::Turn(turn(
                "t_2",
                vec![fe("src/x.rs", Some(v1), Some(v2), "modify")],
            )),
        ]);

        // One range covering lines 2–3 — hand-derived: t_1 owns 2, t_2 owns 3.
        let out = attribute(&l, &objects, &[range("src/x.rs", 2, 2)]);
        match &out.files[0].ranges[0].attribution {
            Attribution::Turns { turns } => {
                let ids: Vec<&str> = turns.iter().map(|t| t.id.as_str()).collect();
                assert_eq!(ids, vec!["t_1", "t_2"]);
            }
            other => panic!("expected two Turns, got {other:?}"),
        }
    }

    /// A `create` op has no `before`: every line of `after` is the turn's.
    #[test]
    fn a_create_op_claims_every_line_of_its_after() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let store = BlobStore::new(&objects);
        let after = store.put(b"one\ntwo\n").unwrap();
        let l = ledger(vec![LogRecord::Turn(turn(
            "t_new",
            vec![fe("src/new.rs", None, Some(after), "create")],
        ))]);
        let out = attribute(&l, &objects, &[range("src/new.rs", 1, 2)]);
        assert!(matches!(
            out.files[0].ranges[0].attribution,
            Attribution::Turns { .. }
        ));
    }

    // ---- precedence arm 2: evicted blob ---------------------------------

    #[test]
    fn an_unresolvable_blob_renders_blob_evicted_and_is_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        std::fs::create_dir_all(&objects).unwrap();
        let missing =
            "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string();
        let l = ledger(vec![LogRecord::Turn(turn(
            "t_evicted",
            vec![fe(
                "src/x.rs",
                Some(missing.clone()),
                Some(missing),
                "modify",
            )],
        ))]);

        let out = attribute(&l, &objects, &[range("src/x.rs", 1, 5)]);
        assert_eq!(
            out.files[0].ranges[0].attribution,
            Attribution::Unattributable {
                reason: UnattributableReason::BlobEvicted
            }
        );
        assert_eq!(out.evicted_ranges, 1);
    }

    /// Precedence arm 1 beats arm 2: a resolvable turn that claims the range
    /// wins even when another turn's blob is evicted. Otherwise an unrelated
    /// eviction would erase a perfectly good attribution.
    #[test]
    fn a_claiming_turn_outranks_an_unrelated_eviction() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let store = BlobStore::new(&objects);
        let after = store.put(b"one\n").unwrap();
        let missing =
            "sha256:1111111111111111111111111111111111111111111111111111111111111111".to_string();
        let l = ledger(vec![
            LogRecord::Turn(turn(
                "t_evicted",
                vec![fe(
                    "src/x.rs",
                    Some(missing.clone()),
                    Some(missing),
                    "modify",
                )],
            )),
            LogRecord::Turn(turn(
                "t_good",
                vec![fe("src/x.rs", None, Some(after), "create")],
            )),
        ]);
        let out = attribute(&l, &objects, &[range("src/x.rs", 1, 1)]);
        assert!(matches!(
            out.files[0].ranges[0].attribution,
            Attribution::Turns { .. }
        ));
        assert_eq!(out.evicted_ranges, 0, "counted only on the evicted ARM");
    }

    // ---- precedence arm 3: gap ------------------------------------------

    #[test]
    fn an_unclaimed_range_in_a_gapped_history_is_unattributable_not_human() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        // stop → start = a Restart gap (view.rs::recording_gaps).
        let l = ledger(vec![
            epoch("start", "2020-06-01T00:00:00.000Z"),
            epoch("stop", "2020-06-01T00:01:00.000Z"),
            epoch("start", "2020-06-01T00:02:00.000Z"),
        ]);
        let out = attribute(&l, &objects, &[range("src/x.rs", 1, 3)]);
        assert_eq!(
            out.files[0].ranges[0].attribution,
            Attribution::Unattributable {
                reason: UnattributableReason::Gap
            },
            "never guessed as human while any recording gap exists"
        );
        assert_eq!(out.evicted_ranges, 0);
    }

    /// Arm 1 beats arm 3 too: a gap elsewhere in history must not erase a
    /// range a turn actually claims.
    #[test]
    fn a_claiming_turn_outranks_a_gap() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let store = BlobStore::new(&objects);
        let after = store.put(b"one\n").unwrap();
        let l = ledger(vec![
            epoch("start", "2020-06-01T00:00:00.000Z"),
            epoch("stop", "2020-06-01T00:01:00.000Z"),
            epoch("start", "2020-06-01T00:02:00.000Z"),
            LogRecord::Turn(turn(
                "t_1",
                vec![fe("src/x.rs", None, Some(after), "create")],
            )),
        ]);
        let out = attribute(&l, &objects, &[range("src/x.rs", 1, 1)]);
        assert!(matches!(
            out.files[0].ranges[0].attribution,
            Attribution::Turns { .. }
        ));
    }

    // ---- precedence arm 4: human ----------------------------------------

    #[test]
    fn an_empty_ledger_attributes_everything_to_human() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let l = ledger(vec![]);
        let out = attribute(&l, &objects, &[range("a.rs", 1, 4), range("b.rs", 1, 1)]);
        assert_eq!(out.files.len(), 2);
        for f in &out.files {
            assert_eq!(f.ranges[0].attribution, Attribution::Human);
        }
        assert_eq!(out.evicted_ranges, 0);
        assert_eq!(out.line_precision, LINE_PRECISION);
    }

    /// A turn touching a DIFFERENT path never claims, evicts, or otherwise
    /// disturbs this path's ranges.
    #[test]
    fn a_turn_on_another_path_does_not_touch_this_paths_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let missing =
            "sha256:2222222222222222222222222222222222222222222222222222222222222222".to_string();
        let l = ledger(vec![LogRecord::Turn(turn(
            "t_other",
            vec![fe(
                "other.rs",
                Some(missing.clone()),
                Some(missing),
                "modify",
            )],
        ))]);
        let out = attribute(&l, &objects, &[range("src/x.rs", 1, 1)]);
        assert_eq!(out.files[0].ranges[0].attribution, Attribution::Human);
        assert_eq!(out.evicted_ranges, 0);
    }

    #[test]
    fn ranges_group_by_path_in_first_seen_order() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let l = ledger(vec![]);
        let out = attribute(
            &l,
            &objects,
            &[
                range("b.rs", 1, 1),
                range("a.rs", 1, 1),
                range("b.rs", 5, 2),
            ],
        );
        assert_eq!(
            out.files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["b.rs", "a.rs"]
        );
        assert_eq!(
            out.files[0].ranges.len(),
            2,
            "both b.rs ranges in one group"
        );
    }
}
