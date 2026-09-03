//! Probe-state computation for the `bisect` verb (Phase 3.0 T5, spec
//! `docs/superpowers/specs/2026-08-07-phase-3-design.md` §3.0.4).
//!
//! **What a probe state is.** Bisect never reconstructs a historical snapshot
//! of the repository. It takes the CURRENT working tree and subtracts the
//! agent turns that came after the probe point: for every file entry of every
//! post-probe turn, the file is put back to that entry's `before` bytes (or
//! removed, for a `create` op; or recreated, for a `delete` op). Everything
//! the ledger does not know about — human edits, untracked scratch files,
//! build output — stays exactly as it is now. The honest name for the result
//! is *"working tree minus later agent turns"*, and no output of this feature
//! may word it as a snapshot.
//!
//! **Why latest-first.** The walk visits post-probe turns in reverse ledger
//! order and lets each visit OVERWRITE the pending action for a path. Because
//! the earliest post-probe turn is visited last, its `before` is what
//! survives — which is precisely the state the file was in when the probe
//! point ended. Walking oldest-first would leave the LATEST turn's `before`,
//! i.e. a state that already contains the intervening turn's writes. The
//! 3-turn unit fixture below pins this direction.
//!
//! **Sequence membership** (spec §3.0.4): rich, non-imported turns by
//! default. `git`-tool turns are rich and non-imported and are deliberately
//! KEPT — dropping a git turn from the subtraction would leave every probe
//! state after a checkout wrong, which is a worse failure than bisect being
//! able to name a turn whose tool is `git`. Turns superseded by a retroactive
//! merge (PROTOCOL §4) are always dropped: the merging turn already carries
//! their file entries, and subtracting both would apply one turn's changes
//! twice.
//!
//! **This module performs no I/O beyond CAS blob reads.** Materializing a
//! probe state onto disk is the CLI's job (`cli/src/bisectcmd.rs`); the
//! working tree and `.agentrec/` are never written by bisect at all.

use crate::record::{FileEntry, LogRecord, TurnRecord};
use crate::store::BlobStore;
use crate::view::{recording_gaps, GapKind, Ledger};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Component, Path};

/// The fidelity sentence. A constant so `--help`, the rendered report and the
/// tests cannot drift apart (the `LINE_PRECISION` precedent from T4).
pub const FIDELITY: &str =
    "a probe state is the working tree minus later agent turns; not a historical snapshot";

/// What bisect may include in the searched sequence.
#[derive(Debug, Clone, Default)]
pub struct BisectOptions {
    /// Admit bare turns — both as subtraction and as probe candidates. Off by
    /// default: a bare turn is an unattributed activity window, so its bytes
    /// may be human work, and naming one as "the first bad turn" asserts
    /// attribution the recorder never had. The CLI prints a misattribution
    /// warning whenever this is on.
    pub include_bare: bool,
}

/// Why a probe could not be computed or decided. String-tagged on the wire so
/// a future reason is an additive VALUE, not a key break (the `rate_bound`
/// precedent from T1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnanswerableReason {
    /// The entry was over the snapshot cap: no `before` bytes exist.
    SkippedEntry,
    /// The entry matched a secret-file pattern and was never snapshotted.
    WithheldEntry,
    /// A non-`create` entry carries no `before` hash (`baseline_unknown`, or
    /// a producer that omitted it).
    MissingBefore,
    /// The `before` hash is recorded but the blob does not resolve (evicted,
    /// purged, corrupt). Branching is on the blob failing to resolve, never
    /// on why.
    DanglingBlob,
    /// `after_synthesized` — the entry's bytes were DERIVED, not observed
    /// (PROTOCOL import-honesty). Derived bytes are not recorded fact and
    /// must not silently materialize into a probe state. The whole entry is
    /// refused, not just its `after`: `before` on such an entry is the input
    /// the derivation ran against, and treating it as observed truth in a
    /// state the user then runs a test suite over is the same overclaim.
    SynthesizedAfter,
    /// The entry describes something other than an ordinary file (a symlink,
    /// per `link_kind`). PROTOCOL §5 requires consumers to refuse to ACT on
    /// such an entry, including on an unknown future value.
    NonRegularEntry,
    /// The entry's `path` is wire data that is not a repo-relative name — it
    /// is absolute, or walks out with `..`. Refusing here is what stops the
    /// materializer's `scratch.join(path)` from being an arbitrary-write
    /// primitive (`Path::join` DISCARDS the base for an absolute path). This
    /// LEXICAL check is sufficient only because the materializer copies
    /// regular files and never recreates symlinks — if that ever changes, a
    /// resolved check is required too, exactly as
    /// `undo_coordinator::escape_refusal` documents.
    UnsafePath,
    /// The test command's repeated runs disagreed (`--flaky-retries`), so the
    /// probe has no verdict. Produced by the CLI driver, not by this walk.
    FlakyDisagreement,
}

/// One reason a probe could not be answered, named to a turn (and, where the
/// walk knows it, to the offending path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unanswerable {
    pub turn_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub reason: UnanswerableReason,
}

/// What the materializer must do to one path to reach the probe state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileAction {
    /// Write these exact bytes (creating parent directories as needed).
    Write(Vec<u8>),
    /// Remove the file if it is present. Produced for a `create` op: the file
    /// did not exist at the probe point.
    Delete,
}

/// The full set of path→action edits that turn a copy of the current working
/// tree into a probe state. Paths are repo-relative and have passed the
/// lexical containment check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeState {
    pub files: BTreeMap<String, FileAction>,
}

/// A recording gap intersecting the searched span, reported as a caveat.
///
/// Gaps are TOLERATED, never fatal (spec round-2 correction): every daemon
/// restart mints one, so a gap-fatal probe would make bisect inoperative on
/// any real repository. A gap is a window of possibly-unrecorded edits —
/// the same class of imprecision the probe state already tolerates by
/// leaving human edits current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GapWindow {
    pub since: String,
    pub kind: String,
}

/// Which turn bisect blames, or why it cannot narrow further.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Verdict {
    /// The first turn in the sequence whose probe state fails the test.
    FirstBad { turn_id: String },
    /// The span could not be narrowed to one turn: every candidate inside it
    /// was unanswerable. The ids are the remaining suspects, oldest-first.
    AmbiguousSpan { ids: Vec<String>, reason: String },
}

/// The whole bisect report — the same value the text renderer and `--json`
/// consume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BisectResult {
    pub verdict: Verdict,
    pub unanswerable: Vec<Unanswerable>,
    pub gap_windows: Vec<GapWindow>,
    /// Probe points at which the test command was actually executed. A probe
    /// rejected by the walk (unanswerable state) never runs the command and
    /// is NOT counted; a flaky-disagreement probe ran it and IS counted.
    pub probes: u64,
    /// Working-tree entries the scratch copy could not reproduce because they
    /// are not ordinary files (symlinks, sockets, fifos). Counted rather than
    /// silently dropped — a probe run against a tree missing them may fail
    /// for reasons that have nothing to do with any turn.
    pub copy_skipped: u64,
    /// Present when `--include-bare` was used: the attribution caveat that
    /// admitting bare turns carries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_bare_warning: Option<String>,
    /// The fidelity posture, machine-readable so a `--json` consumer cannot
    /// present a probe state as a snapshot either.
    pub fidelity: String,
}

/// The misattribution warning `--include-bare` carries.
pub const INCLUDE_BARE_WARNING: &str =
    "--include-bare: bare turns are unattributed activity windows — a human edit produces the \
same filesystem signature — so a verdict naming a bare turn is not evidence an agent wrote it";

/// The searched sequence, oldest-first, in ledger order.
///
/// Built explicitly here rather than through `RepositoryView::list_records`:
/// the default `TurnQuery` drops git turns as well as superseded ones, and
/// which turns bisect subtracts is a decision this module must own and state,
/// not inherit from another verb's display filter.
pub fn sequence<'a>(ledger: &'a Ledger, opts: &BisectOptions) -> Vec<&'a TurnRecord> {
    let turns: Vec<&TurnRecord> = ledger
        .records
        .iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        })
        .collect();
    let superseded: std::collections::HashSet<&str> = turns
        .iter()
        .flat_map(|t| t.merges.iter().map(String::as_str))
        .collect();
    turns
        .into_iter()
        .filter(|t| !superseded.contains(t.id.as_str()))
        // Imported turns are excluded unconditionally: `import` reconstructs
        // them from another tool's transcript, and their `after` bytes are
        // frequently synthesized rather than observed.
        .filter(|t| !t.imported.unwrap_or(false))
        .filter(|t| t.grade == "rich" || (opts.include_bare && t.grade == "bare"))
        .collect()
}

/// The turns a probe at `after` subtracts: exactly those at a LATER sequence
/// index. `after == None` is the baseline probe point — before the first turn
/// in the sequence — so every turn is subtracted.
///
/// Separated from [`probe_state`] so the indexing convention can be asserted
/// by NAME (which turn ids get subtracted) rather than inferred from bytes,
/// where an off-by-one lands on a neighbouring turn while every byte
/// assertion still holds.
pub fn subtracted<'a, 'b>(seq: &'b [&'a TurnRecord], after: Option<usize>) -> &'b [&'a TurnRecord] {
    let start = match after {
        None => 0,
        Some(i) => i.saturating_add(1),
    };
    if start >= seq.len() {
        &[]
    } else {
        &seq[start..]
    }
}

/// Is `path` a repo-relative name that provably stays inside the root when
/// joined to it? See [`UnanswerableReason::UnsafePath`] for why this is the
/// gate and why lexical is enough here.
fn safe_relative(path: &str) -> bool {
    let p = Path::new(path);
    !path.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)))
}

/// Compute the probe state at sequence position `after` (see [`subtracted`]).
///
/// `Err` carries every reason the probe is unanswerable — all of them, not
/// just the first, so a user fixing one obstruction can see the rest.
pub fn probe_state(
    seq: &[&TurnRecord],
    after: Option<usize>,
    store: &BlobStore,
) -> Result<ProbeState, Vec<Unanswerable>> {
    let mut files: BTreeMap<String, FileAction> = BTreeMap::new();
    let mut blocked: Vec<Unanswerable> = Vec::new();
    // Latest-first: each visit overwrites, so the EARLIEST post-probe turn
    // touching a path is the one whose `before` survives.
    for turn in subtracted(seq, after).iter().rev() {
        for entry in &turn.files {
            match action_for(entry, store) {
                Ok(action) => {
                    files.insert(entry.path.clone(), action);
                }
                Err(reason) => blocked.push(Unanswerable {
                    turn_id: turn.id.clone(),
                    path: Some(entry.path.clone()),
                    reason,
                }),
            }
        }
    }
    if blocked.is_empty() {
        Ok(ProbeState { files })
    } else {
        Err(blocked)
    }
}

/// One entry's contribution to a probe state, or why it blocks the probe.
fn action_for(entry: &FileEntry, store: &BlobStore) -> Result<FileAction, UnanswerableReason> {
    if !safe_relative(&entry.path) {
        return Err(UnanswerableReason::UnsafePath);
    }
    if entry.link_kind.is_some() {
        return Err(UnanswerableReason::NonRegularEntry);
    }
    if entry.after_synthesized.unwrap_or(false) {
        return Err(UnanswerableReason::SynthesizedAfter);
    }
    if entry.skipped {
        return Err(UnanswerableReason::SkippedEntry);
    }
    if entry.withheld {
        return Err(UnanswerableReason::WithheldEntry);
    }
    if entry.op == "create" {
        // The file did not exist at the probe point, so no blob is needed —
        // and a `create` entry legitimately carries no `before`.
        return Ok(FileAction::Delete);
    }
    // `modify` and `delete` both restore the recorded pre-turn bytes; the
    // only difference is that for `delete` the file is currently absent, and
    // writing it is a recreation. Any other op string is treated the same
    // way: it changed the file, so its `before` is what the probe state
    // needs (refuse-to-act would be wrong here — we are RESTORING).
    let Some(hash) = entry.before.as_deref() else {
        return Err(UnanswerableReason::MissingBefore);
    };
    match store.get(hash) {
        Ok(bytes) => Ok(FileAction::Write(bytes)),
        Err(_) => Err(UnanswerableReason::DanglingBlob),
    }
}

/// Recording gaps that cannot be proven disjoint from the searched span.
///
/// Deliberately OVER-lists. `Gap::since` means different things per kind: for
/// `Crash`/`Restart` it is the later epoch that proves the preceding span was
/// uncovered, while for `TrailingStop` the uncovered interval extends FORWARD
/// from it. A naive `since ∈ [from, to]` filter would drop a `TrailingStop`
/// that begins before the span and covers all of it — a silent clean bill of
/// health. An extra caveat line costs a reader nothing; a missing one is a
/// fidelity claim nobody measured.
pub fn gap_windows(records: &[LogRecord], from: &str, to: &str) -> Vec<GapWindow> {
    recording_gaps(records)
        .into_iter()
        .filter(|g| match g.kind {
            // Extends forward: relevant unless it starts after the span ends.
            GapKind::TrailingStop => g.since.as_str() <= to,
            // Proves the interval BEFORE `since` was uncovered: relevant
            // unless it is proven entirely before the span began.
            GapKind::Crash | GapKind::Restart => g.since.as_str() >= from,
        })
        .map(|g| GapWindow {
            since: g.since,
            kind: match g.kind {
                GapKind::Crash => "crash",
                GapKind::Restart => "restart",
                GapKind::TrailingStop => "trailing_stop",
            }
            .to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{EpochRecord, TurnRecord};

    fn fe(path: &str, before: Option<&str>, op: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            before: before.map(str::to_string),
            after: Some("after-hash".to_string()),
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

    fn turn(id: &str, grade: &str, files: Vec<FileEntry>) -> TurnRecord {
        TurnRecord {
            v: 1,
            id: id.to_string(),
            grade: grade.to_string(),
            truncated: false,
            started: "2026-01-01T00:00:00.000Z".to_string(),
            ended: "2026-01-01T00:00:01.000Z".to_string(),
            tool: Some("claude".to_string()),
            model: None,
            session: None,
            root: "/repo".to_string(),
            prompt_ref: None,
            prompt_excerpt: None,
            merges: vec![],
            imported: None,
            files_complete: None,
            origin: None,
            files,
        }
    }

    fn ledger_of(records: Vec<LogRecord>) -> Ledger {
        Ledger {
            records,
            unknown_type_lines: 0,
            unparsed_lines: 0,
        }
    }

    /// A store seeded with named contents; returns (tempdir, store).
    fn store_with(contents: &[(&str, &str)]) -> (tempfile::TempDir, BlobStore, Vec<String>) {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("objects"));
        let hashes = contents
            .iter()
            .map(|(_, body)| store.put(body.as_bytes()).unwrap())
            .collect();
        (dir, store, hashes)
    }

    // ---------------------------------------------------------------
    // reverse-apply order
    // ---------------------------------------------------------------

    /// HAND-COMPUTED FIXTURE. Three turns, all touching `a.txt`:
    ///   t1: modify, before = "v0"
    ///   t2: modify, before = "v1"
    ///   t3: modify, before = "v2"
    /// Probing at the baseline (before t1) subtracts t1,t2,t3. The earliest
    /// post-probe turn touching `a.txt` is t1, so the probe state must carry
    /// t1's before, "v0" — NOT t3's "v2", which is what an oldest-first walk
    /// (or a first-write-wins map) would leave.
    /// Probing after t1 subtracts t2,t3 → earliest is t2 → "v1".
    /// Probing after t2 subtracts t3 → "v2". Probing after t3 subtracts
    /// nothing → no edits at all.
    #[test]
    fn walk_keeps_the_earliest_post_probe_before() {
        let (_d, store, h) = store_with(&[("v0", "v0"), ("v1", "v1"), ("v2", "v2")]);
        let t1 = turn("t1", "rich", vec![fe("a.txt", Some(&h[0]), "modify")]);
        let t2 = turn("t2", "rich", vec![fe("a.txt", Some(&h[1]), "modify")]);
        let t3 = turn("t3", "rich", vec![fe("a.txt", Some(&h[2]), "modify")]);
        let seq: Vec<&TurnRecord> = vec![&t1, &t2, &t3];

        let at = |after| {
            let s = probe_state(&seq, after, &store).expect("answerable");
            s.files.get("a.txt").cloned()
        };
        assert_eq!(at(None), Some(FileAction::Write(b"v0".to_vec())));
        assert_eq!(at(Some(0)), Some(FileAction::Write(b"v1".to_vec())));
        assert_eq!(at(Some(1)), Some(FileAction::Write(b"v2".to_vec())));
        assert_eq!(at(Some(2)), None, "nothing after the last turn to subtract");
    }

    /// `create` → the file was absent at the probe point; `delete` → it was
    /// present, so its `before` bytes are written back.
    #[test]
    fn create_deletes_and_delete_recreates() {
        let (_d, store, h) = store_with(&[("gone", "gone-body")]);
        let t1 = turn(
            "t1",
            "rich",
            vec![
                fe("new.txt", None, "create"),
                fe("gone.txt", Some(&h[0]), "delete"),
            ],
        );
        let seq: Vec<&TurnRecord> = vec![&t1];
        let s = probe_state(&seq, None, &store).expect("answerable");
        assert_eq!(s.files.get("new.txt"), Some(&FileAction::Delete));
        assert_eq!(
            s.files.get("gone.txt"),
            Some(&FileAction::Write(b"gone-body".to_vec()))
        );
    }

    // ---------------------------------------------------------------
    // indexing convention
    // ---------------------------------------------------------------

    /// Probing at index i subtracts exactly indices > i. Asserted by NAME:
    /// a byte assertion cannot tell a correct walk from one that is off by
    /// one turn but still produces plausible content.
    #[test]
    fn subtracted_is_strictly_after_the_probe_index() {
        let t1 = turn("t1", "rich", vec![]);
        let t2 = turn("t2", "rich", vec![]);
        let t3 = turn("t3", "rich", vec![]);
        let seq: Vec<&TurnRecord> = vec![&t1, &t2, &t3];
        let ids = |after| {
            subtracted(&seq, after)
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(None), vec!["t1", "t2", "t3"]);
        assert_eq!(ids(Some(0)), vec!["t2", "t3"]);
        assert_eq!(ids(Some(1)), vec!["t3"]);
        assert_eq!(ids(Some(2)), Vec::<&str>::new());
        assert_eq!(ids(Some(99)), Vec::<&str>::new(), "out of range is empty");
    }

    // ---------------------------------------------------------------
    // membership
    // ---------------------------------------------------------------

    #[test]
    fn default_sequence_excludes_bare_and_imported() {
        let rich = turn("rich1", "rich", vec![]);
        let bare = turn("bare1", "bare", vec![]);
        let mut imported = turn("imp1", "rich", vec![]);
        imported.imported = Some(true);
        let led = ledger_of(vec![
            LogRecord::Turn(rich.clone()),
            LogRecord::Turn(bare.clone()),
            LogRecord::Turn(imported),
        ]);

        let ids: Vec<&str> = sequence(&led, &BisectOptions::default())
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(ids, vec!["rich1"]);

        let ids: Vec<&str> = sequence(&led, &BisectOptions { include_bare: true })
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["rich1", "bare1"],
            "--include-bare admits bare turns; imported stays out unconditionally"
        );
    }

    /// A bare turn admitted by `--include-bare` is a probe candidate AND is
    /// subtracted like any other turn: its file entry appears in the probe
    /// state at the baseline.
    #[test]
    fn include_bare_turn_is_subtracted_and_probeable() {
        let (_d, store, h) = store_with(&[("b", "bare-before")]);
        let bare = turn("bare1", "bare", vec![fe("b.txt", Some(&h[0]), "modify")]);
        let led = ledger_of(vec![LogRecord::Turn(bare)]);
        let opts = BisectOptions { include_bare: true };
        let seq = sequence(&led, &opts);
        assert_eq!(seq.len(), 1, "probe candidate");
        let s = probe_state(&seq, None, &store).expect("answerable");
        assert_eq!(
            s.files.get("b.txt"),
            Some(&FileAction::Write(b"bare-before".to_vec())),
            "subtracted like any other turn"
        );
    }

    /// A turn listed in a later turn's `merges` was folded into it; counting
    /// both would subtract one turn's work twice.
    #[test]
    fn superseded_turns_are_never_in_the_sequence() {
        let folded = turn("folded", "bare", vec![]);
        let mut merger = turn("merger", "rich", vec![]);
        merger.merges = vec!["folded".to_string()];
        let led = ledger_of(vec![LogRecord::Turn(folded), LogRecord::Turn(merger)]);
        let ids: Vec<&str> = sequence(&led, &BisectOptions { include_bare: true })
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(ids, vec!["merger"]);
    }

    /// Git turns stay in: dropping one would leave every probe state after a
    /// checkout wrong.
    #[test]
    fn git_turns_stay_in_the_sequence() {
        let mut g = turn("g1", "rich", vec![]);
        g.tool = Some("git".to_string());
        let led = ledger_of(vec![LogRecord::Turn(g)]);
        assert_eq!(sequence(&led, &BisectOptions::default()).len(), 1);
    }

    // ---------------------------------------------------------------
    // unanswerable
    // ---------------------------------------------------------------

    fn blocked_reason(entry: FileEntry, store: &BlobStore) -> UnanswerableReason {
        let t = turn("t1", "rich", vec![entry]);
        let seq: Vec<&TurnRecord> = vec![&t];
        let errs = probe_state(&seq, None, store).expect_err("must be unanswerable");
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].turn_id, "t1");
        errs[0].reason
    }

    #[test]
    fn skipped_withheld_and_missing_before_are_unanswerable() {
        let (_d, store, _h) = store_with(&[]);
        let mut skipped = fe("a.txt", Some("deadbeef"), "modify");
        skipped.skipped = true;
        assert_eq!(
            blocked_reason(skipped, &store),
            UnanswerableReason::SkippedEntry
        );

        let mut withheld = fe("b.txt", None, "modify");
        withheld.withheld = true;
        assert_eq!(
            blocked_reason(withheld, &store),
            UnanswerableReason::WithheldEntry
        );

        assert_eq!(
            blocked_reason(fe("c.txt", None, "modify"), &store),
            UnanswerableReason::MissingBefore
        );
    }

    #[test]
    fn a_dangling_before_blob_is_unanswerable() {
        let (_d, store, _h) = store_with(&[]);
        // A well-formed hash that was never stored.
        let hash = crate::store::hash_bytes(b"never stored");
        assert_eq!(
            blocked_reason(fe("a.txt", Some(&hash), "modify"), &store),
            UnanswerableReason::DanglingBlob
        );
    }

    /// CONTRACT test, not a corpus observation. `after_synthesized` is
    /// written only by `import`, and imported turns are already out of the
    /// sequence — so this shape does not occur in a log this workspace
    /// produces. The guard exists because `log.jsonl` is wire data a foreign
    /// producer may write, and derived bytes must never materialize into a
    /// state the user runs a test suite over.
    #[test]
    fn a_synthesized_entry_is_unanswerable() {
        let (_d, store, h) = store_with(&[("x", "x")]);
        let mut e = fe("a.txt", Some(&h[0]), "modify");
        e.after_synthesized = Some(true);
        assert_eq!(
            blocked_reason(e, &store),
            UnanswerableReason::SynthesizedAfter
        );
    }

    #[test]
    fn a_symlink_entry_and_an_escaping_path_are_unanswerable() {
        let (_d, store, h) = store_with(&[("x", "x")]);
        let mut link = fe("a.txt", Some(&h[0]), "modify");
        link.link_kind = Some("symlink".to_string());
        assert_eq!(
            blocked_reason(link, &store),
            UnanswerableReason::NonRegularEntry
        );

        for path in ["../outside.txt", "/etc/passwd", "a/../../b.txt", ""] {
            assert_eq!(
                blocked_reason(fe(path, Some(&h[0]), "modify"), &store),
                UnanswerableReason::UnsafePath,
                "path {path:?} must not reach a join"
            );
        }
        assert!(safe_relative("a/b/c.txt"), "ordinary paths still pass");
    }

    /// Every obstruction is reported, not just the first — a user clearing
    /// one should see the rest in the same run.
    #[test]
    fn all_obstructions_are_reported() {
        let (_d, store, _h) = store_with(&[]);
        let mut skipped = fe("a.txt", None, "modify");
        skipped.skipped = true;
        let t = turn("t1", "rich", vec![skipped, fe("b.txt", None, "modify")]);
        let seq: Vec<&TurnRecord> = vec![&t];
        let errs = probe_state(&seq, None, &store).expect_err("unanswerable");
        assert_eq!(errs.len(), 2);
        assert_eq!(
            errs.iter().map(|u| u.reason).collect::<Vec<_>>(),
            vec![
                UnanswerableReason::SkippedEntry,
                UnanswerableReason::MissingBefore
            ]
        );
    }

    /// The narrowing rule, at the walk level: a probe point whose state is
    /// unanswerable yields no verdict, while its neighbours still do — which
    /// is what lets the driver fall back to the narrowest ANSWERABLE span
    /// instead of guessing. Here t2 carries a withheld entry, so probing
    /// after t1 (which subtracts t2 and t3) is unanswerable, while probing
    /// after t2 (subtracts t3 only) is answerable.
    #[test]
    fn an_unanswerable_turn_only_blocks_probes_that_subtract_it() {
        let (_d, store, h) = store_with(&[("v", "v")]);
        let t1 = turn("t1", "rich", vec![fe("a.txt", Some(&h[0]), "modify")]);
        let mut secret = fe("s.env", None, "modify");
        secret.withheld = true;
        let t2 = turn("t2", "rich", vec![secret]);
        let t3 = turn("t3", "rich", vec![fe("c.txt", Some(&h[0]), "modify")]);
        let seq: Vec<&TurnRecord> = vec![&t1, &t2, &t3];
        assert!(probe_state(&seq, Some(0), &store).is_err(), "subtracts t2");
        assert!(probe_state(&seq, Some(1), &store).is_ok(), "skips t2");
        assert!(probe_state(&seq, None, &store).is_err(), "subtracts t2");
    }

    // ---------------------------------------------------------------
    // gaps
    // ---------------------------------------------------------------

    fn epoch(event: &str, ts: &str) -> LogRecord {
        LogRecord::Epoch(EpochRecord {
            v: 1,
            event: event.to_string(),
            ts: ts.to_string(),
            dropped_signals: 0,
        })
    }

    /// HAND-COMPUTED: start@00, stop@02, start@04 (a mid-span daemon
    /// restart), and the log stays open. `recording_gaps` reports one
    /// `Restart` at the LATER epoch, 04:00. Searching the span 01:00→05:00
    /// must list it — and probes over that span must still be answerable,
    /// which is the whole point of the round-2 correction.
    #[test]
    fn a_mid_span_restart_is_listed_and_probes_still_answer() {
        let (_d, store, h) = store_with(&[("v", "v")]);
        let t1 = turn("t1", "rich", vec![fe("a.txt", Some(&h[0]), "modify")]);
        let t2 = turn("t2", "rich", vec![fe("a.txt", Some(&h[0]), "modify")]);
        let records = vec![
            epoch("start", "2026-01-01T00:00:00.000Z"),
            LogRecord::Turn(t1.clone()),
            epoch("stop", "2026-01-01T00:02:00.000Z"),
            epoch("start", "2026-01-01T00:04:00.000Z"),
            LogRecord::Turn(t2.clone()),
        ];
        let gaps = gap_windows(
            &records,
            "2026-01-01T00:01:00.000Z",
            "2026-01-01T00:05:00.000Z",
        );
        assert_eq!(
            gaps,
            vec![GapWindow {
                since: "2026-01-01T00:04:00.000Z".to_string(),
                kind: "restart".to_string(),
            }]
        );

        let led = ledger_of(records);
        let seq = sequence(&led, &BisectOptions::default());
        assert_eq!(seq.len(), 2);
        assert!(
            probe_state(&seq, Some(0), &store).is_ok(),
            "a gap must not make the probe fatal"
        );
    }

    /// A trailing stop BEFORE the span still covers it, so it must be
    /// listed: the naive `since >= from` filter would drop it silently.
    #[test]
    fn a_trailing_stop_before_the_span_is_still_listed() {
        let records = vec![
            epoch("start", "2026-01-01T00:00:00.000Z"),
            epoch("stop", "2026-01-01T00:01:00.000Z"),
        ];
        let gaps = gap_windows(
            &records,
            "2026-01-01T00:03:00.000Z",
            "2026-01-01T00:09:00.000Z",
        );
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].kind, "trailing_stop");
    }

    /// A restart proven entirely before the span is not a caveat for it.
    #[test]
    fn a_restart_before_the_span_is_not_listed() {
        let records = vec![
            epoch("start", "2026-01-01T00:00:00.000Z"),
            epoch("stop", "2026-01-01T00:01:00.000Z"),
            epoch("start", "2026-01-01T00:02:00.000Z"),
        ];
        let gaps = gap_windows(
            &records,
            "2026-01-01T00:05:00.000Z",
            "2026-01-01T00:09:00.000Z",
        );
        assert!(gaps.is_empty(), "{gaps:?}");
    }
}
