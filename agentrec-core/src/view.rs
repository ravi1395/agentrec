//! Read-side interpretation, split out of the renderer-coupled CLI.
//!
//! Everything here answers "what does the ledger mean" and nothing here
//! renders. Errors are typed so the human CLI keeps ownership of its prose
//! (and of `fmt::short_id`) while `--json` and, later, MCP read the same
//! interpretation without going through a string.

use crate::record::{FileEntry, LogRecord, TurnRecord};

/// Why an interval of wall time carries no recording coverage.
///
/// The three shapes are kept distinct because the callers genuinely differ:
/// `status`' gap count and `blame`'s "no recording gap" line are about the
/// crash shape only, while staleness ("was the daemon off since this turn
/// ended?") is about any uncovered interval however it arose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapKind {
    /// A `start` epoch following a still-open `start`: the previous daemon
    /// died without writing `stop` (kill -9 left the first unterminated).
    Crash,
    /// A `stop` followed later by a `start` — the daemon was deliberately
    /// off for that interval, however short.
    Restart,
    /// The log's last epoch is a `stop` with nothing after it: uncovered
    /// from that timestamp onward.
    TrailingStop,
}

/// One uncovered interval. `since` is the timestamp at which the interval is
/// known to be uncovered — the *later* epoch for [`GapKind::Crash`] and
/// [`GapKind::Restart`] (the one that proves the preceding span was not
/// recorded), and the `stop` itself for [`GapKind::TrailingStop`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    pub since: String,
    pub kind: GapKind,
}

/// The single recording-gap primitive (E2). Every gap question in every
/// adapter is a filter over this vector; no caller re-walks the epoch
/// records itself.
pub fn recording_gaps(records: &[LogRecord]) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let mut open = false;
    let mut pending_stop: Option<&str> = None;
    for r in records {
        let LogRecord::Epoch(e) = r else { continue };
        match e.event.as_str() {
            "start" => {
                if open {
                    gaps.push(Gap {
                        since: e.ts.clone(),
                        kind: GapKind::Crash,
                    });
                } else if pending_stop.is_some() {
                    gaps.push(Gap {
                        since: e.ts.clone(),
                        kind: GapKind::Restart,
                    });
                }
                open = true;
                pending_stop = None;
            }
            "stop" => {
                open = false;
                pending_stop = Some(e.ts.as_str());
            }
            _ => {}
        }
    }
    if let Some(stop_ts) = pending_stop {
        gaps.push(Gap {
            since: stop_ts.to_string(),
            kind: GapKind::TrailingStop,
        });
    }
    gaps
}

/// Did the daemon crash at least once (a `start` that never got its `stop`)?
pub fn has_crash_gap(records: &[LogRecord]) -> bool {
    recording_gaps(records)
        .iter()
        .any(|g| g.kind == GapKind::Crash)
}

/// How many times (E2 crash shape only — `status` reports crashes, not
/// deliberate restarts).
pub fn crash_gap_count(records: &[LogRecord]) -> usize {
    recording_gaps(records)
        .iter()
        .filter(|g| g.kind == GapKind::Crash)
        .count()
}

/// Was any interval after `since` uncovered, of any shape? RFC 3339 strings
/// compare lexically in time order at fixed width. Only the *start* of the
/// uncovered interval needs to be after `since` — once recording has
/// stopped, everything from there on is uncovered.
pub fn has_gap_after(records: &[LogRecord], since: &str) -> bool {
    recording_gaps(records)
        .iter()
        .any(|g| g.since.as_str() > since)
}

/// Why a `turn_ref` did not resolve to exactly one turn. Typed rather than
/// pre-rendered: the human CLI owns the id-shortening and the "recorded
/// turns: a..b" suffix, and a machine consumer should not have to parse
/// prose to tell "unknown" from "ambiguous".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupError {
    /// The ledger holds no turns at all.
    NoTurns,
    /// No recorded id is equal to, or prefixed by, the reference.
    Unknown,
    /// More than one *distinct* turn matched — see [`same_revert`] for the
    /// duplicates that are collapsed instead of reported here.
    Ambiguous { matched: usize },
}

/// Match `turn_ref` against recorded turn ids, exact or unambiguous prefix
/// (K+). Multiple matches are [`LookupError::Ambiguous`] UNLESS they are the
/// same turn re-emitted under one id by a pre-fix daemon's orphan recovery
/// (PR #2), in which case they collapse to one — see [`same_revert`].
///
/// The single lookup choke point: no adapter reimplements this.
pub fn resolve_turn<'a>(
    turns: &[&'a TurnRecord],
    turn_ref: &str,
) -> Result<&'a TurnRecord, LookupError> {
    if turns.is_empty() {
        return Err(LookupError::NoTurns);
    }
    let matches: Vec<&'a TurnRecord> = turns
        .iter()
        .copied()
        .filter(|t| t.id == turn_ref || t.id.starts_with(turn_ref))
        .collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(LookupError::Unknown),
        n => {
            // Curative dedup for PR #2: the engine fix stops a post-fix
            // daemon WRITING a same-id duplicate, but a log.jsonl already
            // written by a pre-fix daemon can still hold two records for one
            // turn (the kill-9 window between persist and journal clear made
            // orphan recovery re-append it under its reserved id). Collapse
            // them when resolving to either yields an identical revert; a
            // genuine id collision (two DIFFERENT turns minted with one id)
            // does not, and still surfaces as ambiguous.
            let first = matches[0];
            if matches.iter().copied().all(|t| same_revert(first, t)) {
                Ok(first)
            } else {
                Err(LookupError::Ambiguous { matched: n })
            }
        }
    }
}

/// Would undoing `a` and undoing `b` touch the worktree identically? True iff
/// they share an id AND the exact same set of file entries (path + before/
/// after hashes + op + flags), order-independent.
///
/// This is the precise safety condition for collapsing PR #2's orphan-recovery
/// double-emit: `undo` consumes only `id` and `files`, so two records equal on
/// both revert byte-for-byte the same and either may be picked. Fields that
/// legitimately drift between the steady `persist` path and `recover_orphan`
/// for the *same* turn are deliberately NOT compared — recovery recomputes
/// `ended` from the crash journal's last-change time, forces `model: None`,
/// and forces `truncated: true` for a bracket turn — so comparing them would
/// wrongly refuse to collapse a real duplicate. Conversely, two records whose
/// `before`/`after` differ would revert to DIFFERENT content, so they are left
/// ambiguous rather than silently collapsed to an arbitrary one.
///
/// Also the discriminator for `purgecmd::purge_log_duplicates` (the
/// store-level repair of this same class of duplicate) — a single choke point
/// for "these two turn records are the same duplicate", never reimplemented
/// at the second call site.
pub fn same_revert(a: &TurnRecord, b: &TurnRecord) -> bool {
    if a.id != b.id || a.files.len() != b.files.len() {
        return false;
    }
    // A turn holds at most one entry per path, so equal length + every entry of
    // `a` present in `b` is set equality (order-independent — recovery's
    // journaled file order need not match the steady-close order).
    a.files.iter().all(|fa| b.files.contains(fa))
}

/// Why a repository could not be read.
///
/// Deliberately has no "not initialized" variant: an absent `.agentrec` is an
/// EMPTY repository, not an error. That is how every read verb in this
/// codebase already behaves (`load_log` on a missing file returns no
/// records, and `log` prints "no turns recorded"), and making the view
/// stricter than the readers it replaces would change `status`' exit code in
/// an uninitialized directory — a behavior change this phase did not
/// sanction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoError {
    /// A read failed for a reason that is not "absent" (permissions, I/O).
    Io(String),
}

impl std::fmt::Display for RepoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepoError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RepoError {}

/// The ledger as read, with the lines that did NOT become records counted
/// rather than silently discarded.
///
/// PROTOCOL §5 requires consumers to tolerate unknown fields and unknown
/// record `type`s. Tolerating them is not the same as pretending they were
/// not there: a reader that drops them silently reports a smaller ledger
/// than the file holds and gives no way to notice a producer writing a
/// record kind this binary predates.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    pub records: Vec<LogRecord>,
    /// Well-formed JSON objects carrying a `type` this binary does not know.
    /// Tolerated (a newer producer is allowed to write them) and counted.
    pub unknown_type_lines: usize,
    /// Non-empty lines that are not parseable JSON at all — a torn tail line
    /// after a crash, most often.
    pub unparsed_lines: usize,
}

/// Read `log.jsonl` in one pass, counting what it could not interpret.
/// Classification is [`crate::record::parse_log_line`]'s — the records and
/// the census come from the same walk, so they cannot drift apart.
pub fn load_ledger(path: &std::path::Path) -> Ledger {
    use crate::record::ParsedLine;
    use std::io::BufRead;

    let mut ledger = Ledger::default();
    let Ok(file) = std::fs::File::open(path) else {
        return ledger;
    };
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        match crate::record::parse_log_line(&line) {
            ParsedLine::Record(r) => ledger.records.push(r),
            ParsedLine::UnknownType => ledger.unknown_type_lines += 1,
            ParsedLine::Unparsed => ledger.unparsed_lines += 1,
            ParsedLine::Blank => {}
        }
    }
    ledger
}

/// A repository's read-only facts. Produced by [`RepositoryView::health`],
/// which writes nothing — deciding to act on `over_budget` is the caller's,
/// and is a separate, explicit call.
#[derive(Debug, Clone)]
pub struct RepositoryHealth {
    pub store_bytes: u64,
    pub budget: u64,
    pub over_budget: bool,
    /// Turns as recorded, before any superseded/git filtering.
    pub turn_count: usize,
    /// Crash-shaped recording gaps ([`GapKind::Crash`]).
    pub crash_gaps: usize,
    pub unknown_type_lines: usize,
    pub unparsed_lines: usize,
}

/// One page of results plus the cursor that continues it.
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `None` when this page reached the end of the ledger.
    pub next: Option<Cursor>,
}

/// Where a paginated read left off.
///
/// A cursor is bound to BOTH the query it produced from and the ledger state
/// it observed. The observed part is the last returned record's id, not a
/// byte offset or an index: `purge --log-duplicates` is a sanctioned rewrite
/// that can leave `log.jsonl` the same length or longer while shifting every
/// position, so a length- or offset-keyed cursor would silently resume at
/// the wrong record instead of reporting staleness.
///
/// The id alone is NOT identity. A ledger written by a pre-fix daemon can
/// hold two records under one id (PR #2's orphan-recovery double-emit — the
/// shape `same_revert` and `purge --log-duplicates` exist for), and an
/// import can append a second turn under a resumed `sessionId`. Resolving
/// such a cursor by first match re-delivers every record between the two
/// occurrences. `after_occurrence` disambiguates: it is the 0-based index of
/// this id among the records sharing it, in ledger order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub after_id: String,
    /// Which record bearing `after_id` this cursor sits after, counting from
    /// the start of the ledger. Zero whenever the id is unique.
    pub after_occurrence: usize,
    /// Fingerprint of the query that produced this cursor; replaying it
    /// against a different query is a caller bug, not a page boundary.
    pub query: String,
}

/// A cursor could not be honored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorError {
    /// The record the cursor named is no longer in the ledger — it was
    /// rewritten away (purge) or the ledger was truncated. The caller must
    /// restart the query rather than receive a page with a silent hole.
    Stale,
    /// The cursor came from a different query.
    QueryMismatch,
    /// `limit: Some(0)` was asked for. Refused rather than served: an empty
    /// page over a non-empty ledger has no honest continuation to report —
    /// there is no record to sit "after" — and answering "no more" would
    /// stop a pager early on data that is still there.
    ZeroLimit,
}

/// What to list, and how much of it.
#[derive(Debug, Clone, Default)]
pub struct TurnQuery {
    /// Include turns superseded by a retroactive merge, and git turns.
    pub include_all: bool,
    /// `None` = unpaginated (every CLI adapter today; MCP 2.2 is the first
    /// caller that pages).
    pub limit: Option<usize>,
    pub after: Option<Cursor>,
}

impl TurnQuery {
    /// Stable identity of the filtering this query applies. Pagination
    /// parameters are deliberately excluded — they vary page to page, while
    /// the filter must not.
    fn fingerprint(&self) -> String {
        format!("turns:all={}", self.include_all)
    }
}

/// A turn, reduced to what a list renders.
#[derive(Debug, Clone)]
pub struct TurnSummary {
    pub id: String,
    pub grade: String,
    pub tool: Option<String>,
    pub started: String,
    pub ended: String,
    pub file_count: usize,
    pub imported: bool,
}

// ---- diff ---------------------------------------------------------------

/// Which turn to diff, which of its paths, and how much of it.
#[derive(Debug, Clone, Default)]
pub struct DiffQuery {
    /// Full turn id or unambiguous prefix — [`resolve_turn`]'s input.
    pub turn: String,
    /// `None` = every file entry the turn recorded.
    pub paths: Option<Vec<String>>,
    /// `None` = unpaginated (the CLI adapter today; MCP 2.2 pages).
    pub limit: Option<usize>,
    pub after: Option<Cursor>,
}

impl DiffQuery {
    /// Stable identity of the filtering this query applies, mirroring
    /// [`TurnQuery::fingerprint`]'s exclusion of the pagination parameters.
    ///
    /// Unlike a turn list, a diff is scoped by BOTH a turn ref and a path
    /// filter, so both must bind: a cursor minted against one turn would
    /// otherwise silently resume inside another turn's entry list. The path
    /// separator is NUL, which no path on either supported platform can
    /// contain, so no path list can forge another's fingerprint.
    fn fingerprint(&self) -> String {
        let paths = match &self.paths {
            None => "*".to_string(),
            Some(p) => p.join("\u{0}"),
        };
        format!("diff:turn={};paths={paths}", self.turn)
    }
}

/// One turn's diff: the header facts plus a page of resolved file states.
///
/// An envelope rather than a bare [`Page`] because the header
/// (`turn <id> · <tool> · N files`) has no slot on `Page`, and a fileless
/// turn's empty page would make those facts unproducible.
#[derive(Debug, Clone)]
pub struct DiffResult {
    /// FULL id — `fmt::short_id` truncation is the adapter's.
    pub turn_id: String,
    pub tool: Option<String>,
    /// The turn's OWN file count, deliberately distinct from
    /// `files.items.len()`: the page may be limited while the header still
    /// reports what the turn recorded.
    pub total_files: usize,
    pub files: Page<FileDiff>,
}

/// One file entry with its content already resolved, so a renderer needs no
/// blob store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub state: FileDiffState,
}

/// The pre-resolved shape of one file's change.
///
/// The variants are the arms a renderer must distinguish, and the order they
/// are decided in is load-bearing: withheld → skipped → before-blob error →
/// after-blob error → binary → baseline-unknown → text. `BaselineUnknown` is
/// decided BEFORE the synthesized-after question, which is why
/// `after_synthesized` rides only on [`FileDiffState::Text`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileDiffState {
    /// Secret-pattern file: never snapshotted, nothing to show.
    Withheld,
    /// Content was not snapshotted; `reason` is the raw wire value, whose
    /// prose mapping is the adapter's.
    Skipped { reason: Option<String> },
    /// A recorded hash the store could not serve. `corrupt` separates a hash
    /// mismatch (an honest, distinct fact) from a blob that is simply gone
    /// (whose cause stays genuinely unknown).
    Unresolvable { corrupt: bool },
    /// Either side is binary; byte counts are all a diff can honestly say.
    Binary { before_len: usize, after_len: usize },
    /// First seen mid-session with no baseline: only the new content is
    /// showable.
    BaselineUnknown { after: String },
    /// Both sides resolved to text. `op` selects which side the renderer
    /// diffs against; `after_synthesized` marks a DERIVED after-state (an
    /// imported oldString/newString substitution), which a renderer must not
    /// present as observed fact.
    Text {
        before: String,
        after: String,
        op: String,
        after_synthesized: bool,
    },
}

/// The oldest/newest ids and count of a ledger's turns — the data the
/// adapter's "recorded turns: a..b (N turns)" suffix needs, without any of
/// its prose. The count is over the UNFILTERED turn vec, duplicates
/// included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRangeSummary {
    pub oldest_id: String,
    pub newest_id: String,
    pub count: usize,
}

/// Why a diff could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffError {
    /// The turn ref did not resolve. `ledger` is `None` only for
    /// [`LookupError::NoTurns`], where there is no oldest/newest id to name.
    Lookup {
        err: LookupError,
        ledger: Option<TurnRangeSummary>,
    },
    Cursor(CursorError),
    /// An I/O failure that is not "absent". Carries the error, not prose.
    ///
    /// Unreachable today: `load_ledger` treats an unreadable log as an empty
    /// one and every blob failure resolves to
    /// [`FileDiffState::Unresolvable`] instead — which is the pre-existing
    /// behavior this seam preserves verbatim. Kept so a stricter reader has
    /// somewhere to report, and so the adapter has an arm rather than a
    /// panic.
    Io(String),
}

/// The whole-file or single-line question `blame` answers.
#[derive(Debug, Clone, Default)]
pub struct BlameQuery {
    pub path: String,
    /// `None` = file-level blame.
    pub line: Option<usize>,
}

/// A blame answer, echoing the query it answers so a renderer that holds
/// nothing else can still name the path and line.
#[derive(Debug, Clone)]
pub struct BlameResult {
    pub path: String,
    pub line: Option<usize>,
    pub state: BlameState,
}

/// What blame could honestly conclude.
///
/// The file-level and line-level arms are deliberately separate variants
/// even where their wording overlaps: the line-level renderings carry a
/// `<path>:<line>: ` prefix that the file-level ones do not, so collapsing
/// them would change bytes.
///
/// Not `PartialEq`: [`TurnRecord`] is not, and widening the record type's
/// derives to make a test assertion shorter is not this task's business.
#[derive(Debug, Clone)]
pub enum BlameState {
    /// No turn touches the path AND the daemon crashed at least once — the
    /// turn that touched it may simply not have been recorded.
    NoTurnRecordingGap,
    /// No turn touches the path and recording has no crash gap to blame.
    NoTurnTouches,
    /// The last turn to touch the path.
    ///
    /// `modified` is the `modified-since` predicate (on-disk hash ≠ this
    /// turn's recorded `after`). `gap_stale` is the strictly stronger
    /// `human-edited-since`-adjacent question: modified AND an uncovered
    /// interval began after this turn ENDED. They are never conflated.
    File {
        turn: TurnRecord,
        deleted: bool,
        gap_stale: bool,
        modified: bool,
    },
    /// Line-level staleness, decided before any line walk. Renders WITHOUT
    /// the `<path>:<line>: ` prefix — the answer is about recording
    /// coverage, not about a line.
    LineRecordingGap,
    /// The file is deleted or absent, so any line resolves to the last turn
    /// that touched it.
    LineDeletedOrAbsent { turn: TurnRecord, deleted: bool },
    /// The newest turn whose recorded diff introduced this exact line text,
    /// with no unresolvable candidate newer than it.
    LineAttributed { turn: TurnRecord },
    /// Some candidate's snapshot did not resolve, so naming any turn would
    /// be a guess.
    LineSnapshotUnavailable,
    /// No candidate introduced the line and history is NOT gap-free, so
    /// "before recording began" would be a fabrication. Renders WITH the
    /// `<path>:<line>: ` prefix — this is the walk's verdict about a line.
    LineOriginGap,
    /// No candidate introduced the line and the whole history is gap-free.
    LineBeforeRecording,
}

/// Why a blame could not be produced. Each variant carries the data its
/// adapter's prose needs (and the path comes from the query), so no message
/// is ever built here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlameError {
    /// A line was asked for in a file that is not on disk and that no turn
    /// touches.
    FileNotFound,
    /// The line number is past the end of the file (or zero).
    LineOutOfRange { lines: usize },
    /// An I/O failure that is not "absent". Unreachable today: the worktree
    /// read deliberately collapses every read error into "absent", which is
    /// the pre-existing behavior this move preserves verbatim.
    Io(String),
}

/// One seam over a recorded repository. Holds no open handles: every method
/// reads the ledger fresh, so a view is safe to keep across daemon writes.
#[derive(Debug)]
pub struct RepositoryView {
    root: std::path::PathBuf,
}

impl RepositoryView {
    pub fn open(root: &std::path::Path) -> Result<Self, RepoError> {
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    fn log_path(&self) -> std::path::PathBuf {
        self.root.join(".agentrec").join("log.jsonl")
    }

    fn objects_dir(&self) -> std::path::PathBuf {
        self.root.join(".agentrec").join("objects")
    }

    pub fn ledger(&self) -> Ledger {
        load_ledger(&self.log_path())
    }

    /// A pure read. `budget` is injected rather than read from config for
    /// the same reason `status_report` already takes it — a tiny budget is
    /// how over-budget behavior is testable without a real multi-GiB store —
    /// and it keeps this crate free of the CLI's config surface.
    ///
    /// Being over budget is reported, never acted on: enforcement is
    /// [`crate::retention::enforce_budget`], which the caller invokes
    /// explicitly when it wants the side effect.
    pub fn health(&self, budget: u64) -> Result<RepositoryHealth, RepoError> {
        self.health_of(&self.ledger(), budget)
    }

    /// [`Self::health`] over a ledger the caller already read. Exists so an
    /// adapter that needs both the records and the health figures pays for
    /// one parse of `log.jsonl`, not two.
    pub fn health_of(&self, ledger: &Ledger, budget: u64) -> Result<RepositoryHealth, RepoError> {
        let store = crate::store::BlobStore::new(self.objects_dir());
        let store_bytes = store.total_bytes();
        let turn_count = ledger
            .records
            .iter()
            .filter(|r| matches!(r, LogRecord::Turn(_)))
            .count();
        Ok(RepositoryHealth {
            store_bytes,
            budget,
            over_budget: store_bytes > budget,
            turn_count,
            crash_gaps: crash_gap_count(&ledger.records),
            unknown_type_lines: ledger.unknown_type_lines,
            unparsed_lines: ledger.unparsed_lines,
        })
    }

    /// List turns oldest-first, the order they were appended.
    pub fn list(&self, q: &TurnQuery) -> Result<Page<TurnSummary>, CursorError> {
        let ledger = self.ledger();
        let superseded: std::collections::HashSet<&str> = ledger
            .records
            .iter()
            .filter_map(|r| match r {
                LogRecord::Turn(t) => Some(t),
                LogRecord::Epoch(_) => None,
            })
            .flat_map(|t| t.merges.iter().map(String::as_str))
            .collect();
        let all: Vec<&TurnRecord> = ledger
            .records
            .iter()
            .filter_map(|r| match r {
                LogRecord::Turn(t) => Some(t),
                LogRecord::Epoch(_) => None,
            })
            .collect();
        // Carries each selected turn's position in the UNFILTERED ledger:
        // staleness is a question about the ledger, not about this query's
        // filter (see the cursor resolution below).
        let selected: Vec<(usize, &TurnRecord)> = all
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, t)| {
                q.include_all
                    || (!superseded.contains(t.id.as_str()) && t.tool.as_deref() != Some("git"))
            })
            .collect();

        let start = match &q.after {
            None => 0,
            Some(c) => {
                if c.query != q.fingerprint() {
                    return Err(CursorError::QueryMismatch);
                }
                // Identity, not position: a rewrite that dropped the named
                // record must surface as stale rather than resume at
                // whatever now sits at that index.
                //
                // Resolved against the unfiltered ledger on purpose. A pure
                // append can retroactively merge an already-returned turn
                // (PROTOCOL §4), which removes it from `selected` while it is
                // still very much present in the log — resolving against the
                // filtered list would call that append a rewrite and report
                // Stale on a ledger nothing rewrote.
                let Some(ledger_idx) = all
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.id == c.after_id)
                    .map(|(i, _)| i)
                    .nth(c.after_occurrence)
                else {
                    // Either the id is gone entirely, or a rewrite removed
                    // the occurrence this cursor sat after. Both are stale;
                    // falling back to another occurrence would re-deliver
                    // every record between them.
                    return Err(CursorError::Stale);
                };
                selected
                    .iter()
                    .position(|(i, _)| *i > ledger_idx)
                    .unwrap_or(selected.len())
            }
        };

        if q.limit == Some(0) {
            return Err(CursorError::ZeroLimit);
        }
        let end = match q.limit {
            Some(n) => (start + n).min(selected.len()),
            None => selected.len(),
        };
        let items: Vec<TurnSummary> = selected[start.min(selected.len())..end]
            .iter()
            .map(|(_, t)| TurnSummary {
                id: t.id.clone(),
                grade: t.grade.clone(),
                tool: t.tool.clone(),
                started: t.started.clone(),
                ended: t.ended.clone(),
                file_count: t.files.len(),
                imported: t.imported.unwrap_or(false),
            })
            .collect();
        let next = if end < selected.len() {
            selected[start.min(selected.len())..end]
                .last()
                .map(|(ledger_idx, t)| Cursor {
                    after_id: t.id.clone(),
                    after_occurrence: all[..*ledger_idx]
                        .iter()
                        .filter(|prior| prior.id == t.id)
                        .count(),
                    query: q.fingerprint(),
                })
        } else {
            None
        };
        Ok(Page { items, next })
    }

    /// One turn's file changes, with every entry's content already resolved.
    ///
    /// The turn is looked up against the UNFILTERED turn vec — superseded
    /// and git turns are diffable by id, and the range a lookup failure
    /// reports counts that same unfiltered vec (duplicated ids included).
    pub fn diff(&self, q: &DiffQuery) -> Result<DiffResult, DiffError> {
        let ledger = self.ledger();
        let turns: Vec<&TurnRecord> = ledger
            .records
            .iter()
            .filter_map(|r| match r {
                LogRecord::Turn(t) => Some(t),
                LogRecord::Epoch(_) => None,
            })
            .collect();

        let turn = resolve_turn(&turns, &q.turn).map_err(|err| DiffError::Lookup {
            err,
            ledger: turn_range_summary(&turns),
        })?;

        let selected: Vec<&FileEntry> = turn
            .files
            .iter()
            .filter(|e| match &q.paths {
                None => true,
                Some(paths) => paths.iter().any(|p| p == &e.path),
            })
            .collect();

        // Cursor resolution mirrors `list`: identity first (a rewrite that
        // dropped the named entry is stale, never a silent reslide), then
        // the zero-limit refusal.
        let start = match &q.after {
            None => 0,
            Some(c) => {
                if c.query != q.fingerprint() {
                    return Err(DiffError::Cursor(CursorError::QueryMismatch));
                }
                let Some(idx) = selected
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.path == c.after_id)
                    .map(|(i, _)| i)
                    .nth(c.after_occurrence)
                else {
                    return Err(DiffError::Cursor(CursorError::Stale));
                };
                idx + 1
            }
        };
        if q.limit == Some(0) {
            return Err(DiffError::Cursor(CursorError::ZeroLimit));
        }
        let end = match q.limit {
            Some(n) => (start + n).min(selected.len()),
            None => selected.len(),
        };
        let start = start.min(selected.len());

        let store = crate::store::BlobStore::new(self.objects_dir());
        let items: Vec<FileDiff> = selected[start..end]
            .iter()
            .map(|e| resolve_file_diff(&store, e))
            .collect();
        let next = if end < selected.len() {
            // Clamped in the slice expression, as `list` does: `end >= 1`
            // holds here only because a zero-limit query was refused above,
            // and an index must not rest on a non-local argument.
            let last_idx = end.saturating_sub(1);
            selected[start..end].last().map(|e| Cursor {
                after_id: e.path.clone(),
                after_occurrence: selected[..last_idx]
                    .iter()
                    .filter(|prior| prior.path == e.path)
                    .count(),
                query: q.fingerprint(),
            })
        } else {
            None
        };

        Ok(DiffResult {
            turn_id: turn.id.clone(),
            tool: turn.tool.clone(),
            total_files: turn.files.len(),
            files: Page { items, next },
        })
    }

    /// Which turn last touched a path, or introduced one of its current
    /// lines. Reads the worktree as well as the ledger — the current bytes
    /// are what a line query is asked about.
    pub fn blame(&self, q: &BlameQuery) -> Result<BlameResult, BlameError> {
        let ledger = self.ledger();
        let records = &ledger.records;
        // Global fallback for the "no touching turn at all" case, where there
        // is no turn to bound an interval-aware check against.
        let no_turn_gap = has_crash_gap(records);

        // Append order (oldest first) in the log — preserved here.
        let touching: Vec<&TurnRecord> = records
            .iter()
            .filter_map(|r| match r {
                LogRecord::Turn(t) => Some(t),
                LogRecord::Epoch(_) => None,
            })
            .filter(|t| t.files.iter().any(|f| f.path == q.path))
            .collect();

        let disk_bytes = std::fs::read(self.root.join(&q.path)).ok();
        let current_hash = disk_bytes.as_deref().map(crate::store::hash_bytes);

        let state = match q.line {
            None => blame_file_state(&touching, records, &q.path, &current_hash, no_turn_gap),
            Some(n) => {
                let store = crate::store::BlobStore::new(self.objects_dir());
                blame_line_state(
                    &store,
                    &touching,
                    records,
                    &q.path,
                    n,
                    &current_hash,
                    disk_bytes.as_deref(),
                    no_turn_gap,
                )?
            }
        };
        Ok(BlameResult {
            path: q.path.clone(),
            line: q.line,
            state,
        })
    }
}

/// The ledger's turn range, or `None` when it holds no turns at all.
fn turn_range_summary(turns: &[&TurnRecord]) -> Option<TurnRangeSummary> {
    let (first, last) = (turns.first()?, turns.last()?);
    Some(TurnRangeSummary {
        oldest_id: first.id.clone(),
        newest_id: last.id.clone(),
        count: turns.len(),
    })
}

/// Resolve one file entry into the state a renderer can print without a
/// store. Arm order is the contract — see [`FileDiffState`].
fn resolve_file_diff(store: &crate::store::BlobStore, entry: &FileEntry) -> FileDiff {
    let state = resolve_file_diff_state(store, entry);
    FileDiff {
        path: entry.path.clone(),
        state,
    }
}

fn resolve_file_diff_state(store: &crate::store::BlobStore, entry: &FileEntry) -> FileDiffState {
    if entry.withheld {
        return FileDiffState::Withheld;
    }
    if entry.skipped {
        return FileDiffState::Skipped {
            reason: entry.skipped_reason.clone(),
        };
    }
    // `before` is decided first: when both sides fail, the before-side error
    // is the one reported.
    let before = match load_blob(store, entry.before.as_deref()) {
        Ok(bytes) => bytes,
        Err(e) => return unresolvable(&e),
    };
    let after = match load_blob(store, entry.after.as_deref()) {
        Ok(bytes) => bytes,
        Err(e) => return unresolvable(&e),
    };

    if crate::diff::is_binary(&before) || crate::diff::is_binary(&after) {
        return FileDiffState::Binary {
            before_len: before.len(),
            after_len: after.len(),
        };
    }

    // Already checked valid UTF-8 by is_binary above.
    let before_str = std::str::from_utf8(&before).unwrap_or("").to_string();
    let after_str = std::str::from_utf8(&after).unwrap_or("").to_string();

    if entry.baseline_unknown && entry.op == "modify" && entry.before.is_none() {
        return FileDiffState::BaselineUnknown { after: after_str };
    }
    FileDiffState::Text {
        before: before_str,
        after: after_str,
        op: entry.op.clone(),
        after_synthesized: entry.after_synthesized == Some(true),
    }
}

fn unresolvable(e: &crate::store::StoreError) -> FileDiffState {
    FileDiffState::Unresolvable {
        corrupt: matches!(e, crate::store::StoreError::Corrupt(_)),
    }
}

/// Load a blob by its optional hash ref; `None` (e.g. a create's `before`)
/// yields empty content, not an error. A present hash the store cannot serve
/// is the only error case, and the real [`crate::store::StoreError`] is
/// preserved so Missing and Corrupt stay distinguishable.
fn load_blob(
    store: &crate::store::BlobStore,
    hash: Option<&str>,
) -> Result<Vec<u8>, crate::store::StoreError> {
    match hash {
        None => Ok(Vec::new()),
        Some(h) => store.get(h),
    }
}

/// True when the file's on-disk content differs from `turn`'s recorded
/// `after` for it (PROTOCOL §5 `modified-since`). Covers "went missing when
/// it shouldn't have" and "reappeared after a delete" the same way, since
/// both sides collapse to `None` when absent.
fn modified_since(turn: &TurnRecord, file: &str, current_hash: &Option<String>) -> bool {
    let entry_after = turn
        .files
        .iter()
        .find(|f| f.path == file)
        .and_then(|f| f.after.clone());
    current_hash.as_deref() != entry_after.as_deref()
}

fn deleted_by(turn: &TurnRecord, file: &str) -> bool {
    turn.files
        .iter()
        .find(|f| f.path == file)
        .map(|f| f.op.as_str())
        == Some("delete")
}

/// File-level blame: the last turn to touch `file`, with deletion, human
/// edits since, and gap staleness reported rather than papered over.
fn blame_file_state(
    touching: &[&TurnRecord],
    records: &[LogRecord],
    file: &str,
    current_hash: &Option<String>,
    no_turn_gap: bool,
) -> BlameState {
    let Some(t) = touching.last() else {
        // A gap may hide the turn that actually touched this file.
        return if no_turn_gap {
            BlameState::NoTurnRecordingGap
        } else {
            BlameState::NoTurnTouches
        };
    };

    let modified = modified_since(t, file, current_hash);
    // E2: interval-aware — a gap only makes THIS turn's attribution stale
    // when it occurs after the turn ended (a gap entirely before it is
    // irrelevant to whether the current on-disk state is explained).
    BlameState::File {
        deleted: deleted_by(t, file),
        gap_stale: has_gap_after(records, &t.ended) && modified,
        modified,
        turn: (*t).clone(),
    }
}

/// Line-level blame: which turn introduced the current text of
/// `file:line_no`, walking touching turns oldest→newest so the LAST turn to
/// introduce that exact line text wins.
///
/// Known v1 limitation: matching is by exact line text, not a tracked
/// identity — a line duplicated verbatim elsewhere in the file cannot be
/// told apart from its duplicate by this heuristic.
#[allow(clippy::too_many_arguments)]
fn blame_line_state(
    store: &crate::store::BlobStore,
    touching: &[&TurnRecord],
    records: &[LogRecord],
    file: &str,
    line_no: usize,
    current_hash: &Option<String>,
    disk_bytes: Option<&[u8]>,
    no_turn_gap: bool,
) -> Result<BlameState, BlameError> {
    let last = touching.last().copied();

    // E2: interval-aware, bounded to the last touching turn's end — mirrors
    // blame_file_state's gap_stale check.
    let gap_stale = match last {
        Some(t) => has_gap_after(records, &t.ended) && modified_since(t, file, current_hash),
        None => no_turn_gap,
    };
    if gap_stale {
        return Ok(BlameState::LineRecordingGap);
    }

    // A deleted-or-absent file resolves any line query to the last turn that
    // touched it — there is no current line N to walk toward otherwise.
    if let Some(t) = last {
        let deleted = deleted_by(t, file);
        if deleted || disk_bytes.is_none() {
            return Ok(BlameState::LineDeletedOrAbsent {
                turn: t.clone(),
                deleted,
            });
        }
    }

    let Some(bytes) = disk_bytes else {
        return Err(BlameError::FileNotFound);
    };
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    if line_no == 0 || line_no > lines.len() {
        return Err(BlameError::LineOutOfRange { lines: lines.len() });
    }
    let target_text = lines[line_no - 1];

    // `touching` is oldest→newest (append order). `responsible` tracks the
    // newest turn whose recorded diff introduces `target_text`;
    // `newest_unresolvable_idx` tracks the newest candidate whose
    // `before`/`after` snapshot didn't resolve, so a turn can't be silently
    // skipped past — mirrors the `has_gap_after` poisoning idiom below, but
    // for missing snapshots instead of missing recording coverage.
    let mut responsible: Option<(usize, &TurnRecord)> = None;
    let mut newest_unresolvable_idx: Option<usize> = None;
    for (idx, t) in touching.iter().enumerate() {
        let Some(entry) = t.files.iter().find(|f| f.path == file) else {
            continue;
        };
        let before = load_text(store, entry.before.as_deref());
        let after = load_text(store, entry.after.as_deref());
        let (Some(before), Some(after)) = (before, after) else {
            // Can't compute this turn's diff at all — it might have
            // introduced or removed `target_text`; treat it as poisoning
            // rather than silently skipping it (that would either wrongly
            // credit an older turn or wrongly fall through to "before
            // recording began").
            newest_unresolvable_idx = Some(idx);
            continue;
        };
        let intro = crate::diff::added_or_changed_lines(&before, &after);
        if intro.iter().any(|l| l == target_text) {
            responsible = Some((idx, t));
        }
    }

    // A responsible turn is only honestly reportable when no unresolvable
    // candidate is NEWER than it — a newer unresolvable turn could have
    // overwritten the line, so naming the older turn would be a guess.
    let responsible_poisoned = matches!(
        (responsible, newest_unresolvable_idx),
        (Some((r_idx, _)), Some(u_idx)) if u_idx > r_idx
    );

    Ok(match responsible {
        Some((_, t)) if !responsible_poisoned => BlameState::LineAttributed { turn: t.clone() },
        // Some candidate's snapshot didn't resolve (whether or not a
        // now-poisoned `responsible` was also found) — the CRITICAL SCOPE
        // RULE: this branches only on `store.get` failing, never on why.
        _ if newest_unresolvable_idx.is_some() => BlameState::LineSnapshotUnavailable,
        // E3: no turn's recorded diff introduces this exact line text, and
        // every candidate's snapshot resolved cleanly. That is only honestly
        // "before recording began" when the whole history is actually
        // gap-free — a line silently added during an uncovered interval
        // (then folded into a later turn's unchanged `before`) would
        // otherwise be misreported as predating all recording.
        None if has_gap_after(records, "") => BlameState::LineOriginGap,
        _ => BlameState::LineBeforeRecording,
    })
}

/// Loads blob text by optional hash, distinguishing two shapes callers must
/// not conflate: `None` hash is legitimately-empty text (a `create` op has
/// no `before` — every line of its `after` really was introduced by that
/// turn, and collapsing that to "unresolvable" would wrongly deny credit
/// for every file-creating turn). `Some(hash)` that fails to resolve (any
/// error — the caller must not, and does not, care which) is UNRESOLVABLE
/// and returned as `None`, never silently coerced to empty text: an
/// unresolvable snapshot is not "no text there", it's "no idea what text
/// was there", and must poison the comparison rather than let it pass
/// through as if nothing changed.
fn load_text(store: &crate::store::BlobStore, hash: Option<&str>) -> Option<String> {
    match hash {
        None => Some(String::new()),
        Some(h) => store
            .get(h)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{EpochRecord, FileEntry};

    fn epoch(event: &str, ts: &str) -> LogRecord {
        LogRecord::Epoch(EpochRecord {
            v: 1,
            event: event.to_string(),
            ts: ts.to_string(),
        })
    }

    fn entry(path: &str, before: Option<&str>, after: Option<&str>) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            before: before.map(str::to_string),
            after: after.map(str::to_string),
            op: "modify".to_string(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
        }
    }

    fn turn(id: &str, ended: &str, files: Vec<FileEntry>) -> TurnRecord {
        TurnRecord {
            v: 1,
            id: id.to_string(),
            grade: "rich".to_string(),
            truncated: false,
            started: "2026-01-01T00:00:00Z".to_string(),
            ended: ended.to_string(),
            tool: None,
            model: None,
            session: None,
            root: "/repo".to_string(),
            prompt_ref: None,
            prompt_excerpt: None,
            merges: vec![],
            imported: None,
            files_complete: None,
            files,
        }
    }

    #[test]
    fn balanced_start_stop_is_not_a_gap_while_open() {
        let recs = vec![epoch("start", "2026-01-01T00:00:00Z")];
        assert!(recording_gaps(&recs).is_empty());
        assert!(!has_crash_gap(&recs));
        assert_eq!(crash_gap_count(&recs), 0);
    }

    #[test]
    fn unbalanced_start_is_a_crash_gap_stamped_at_the_later_start() {
        let recs = vec![
            epoch("start", "2026-01-01T00:00:00Z"),
            epoch("start", "2026-01-01T05:00:00Z"),
        ];
        let gaps = recording_gaps(&recs);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].kind, GapKind::Crash);
        assert_eq!(gaps[0].since, "2026-01-01T05:00:00Z");
    }

    #[test]
    fn clean_restart_is_a_restart_gap_not_a_crash() {
        let recs = vec![
            epoch("start", "2026-01-01T00:00:00Z"),
            epoch("stop", "2026-01-01T01:00:00Z"),
            epoch("start", "2026-01-01T02:00:00Z"),
        ];
        let gaps = recording_gaps(&recs);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].kind, GapKind::Restart);
        // The distinction is load-bearing: `status` counts crashes only, so a
        // deliberate restart must not inflate its gap count.
        assert_eq!(crash_gap_count(&recs), 0);
        assert!(!has_crash_gap(&recs));
    }

    #[test]
    fn trailing_stop_leaves_everything_after_it_uncovered() {
        let recs = vec![
            epoch("start", "2026-01-01T00:00:00Z"),
            epoch("stop", "2026-01-01T01:00:00Z"),
        ];
        let gaps = recording_gaps(&recs);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].kind, GapKind::TrailingStop);
        assert_eq!(gaps[0].since, "2026-01-01T01:00:00Z");
        assert!(!has_crash_gap(&recs));
    }

    #[test]
    fn gap_after_is_bounded_by_the_since_timestamp() {
        let recs = vec![
            epoch("start", "2026-01-01T00:00:00Z"),
            epoch("stop", "2026-01-01T01:00:00Z"),
            epoch("start", "2026-01-01T02:00:00Z"),
        ];
        assert!(has_gap_after(&recs, "2026-01-01T00:30:00Z"));
        // The only gap starts at 02:00; a turn that ended after it is not
        // made stale by it.
        assert!(!has_gap_after(&recs, "2026-01-01T03:00:00Z"));
    }

    #[test]
    fn empty_since_asks_whether_any_gap_exists_at_all() {
        let balanced = vec![epoch("start", "2026-01-01T00:00:00Z")];
        assert!(!has_gap_after(&balanced, ""));
        let stopped = vec![
            epoch("start", "2026-01-01T00:00:00Z"),
            epoch("stop", "2026-01-01T01:00:00Z"),
        ];
        assert!(has_gap_after(&stopped, ""));
    }

    #[test]
    fn unknown_epoch_events_are_ignored_not_treated_as_boundaries() {
        let recs = vec![
            epoch("start", "2026-01-01T00:00:00Z"),
            epoch("paused", "2026-01-01T00:30:00Z"),
            epoch("start", "2026-01-01T01:00:00Z"),
        ];
        let gaps = recording_gaps(&recs);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].kind, GapKind::Crash);
    }

    #[test]
    fn a_log_with_no_epochs_has_no_gaps() {
        let recs = vec![LogRecord::Turn(turn("t_a", "2026-01-01T00:00:00Z", vec![]))];
        assert!(recording_gaps(&recs).is_empty());
        assert!(!has_gap_after(&recs, ""));
    }

    #[test]
    fn resolve_turn_on_an_empty_ledger_says_so() {
        assert!(matches!(
            resolve_turn(&[], "t_a"),
            Err(LookupError::NoTurns)
        ));
    }

    #[test]
    fn resolve_turn_matches_an_unambiguous_prefix() {
        let a = turn("t_AAAA1111", "2026-01-01T00:00:00Z", vec![]);
        let b = turn("t_BBBB2222", "2026-01-01T00:00:00Z", vec![]);
        let turns = vec![&a, &b];
        assert_eq!(resolve_turn(&turns, "t_AAAA").unwrap().id, "t_AAAA1111");
        assert!(matches!(
            resolve_turn(&turns, "t_ZZ"),
            Err(LookupError::Unknown)
        ));
    }

    #[test]
    fn two_records_of_one_turn_collapse_instead_of_erroring() {
        let files = vec![entry("src/a.rs", Some("h1"), Some("h2"))];
        let a = turn("t_DUP", "2026-01-01T00:00:00Z", files.clone());
        // Recovery legitimately drifts `ended`/`truncated`; neither is part of
        // the revert, so the pair must still collapse.
        let mut b = turn("t_DUP", "2026-01-01T09:99:99Z", files);
        b.truncated = true;
        let turns = vec![&a, &b];
        assert_eq!(resolve_turn(&turns, "t_DUP").unwrap().id, "t_DUP");
    }

    #[test]
    fn distinct_turns_sharing_an_id_stay_ambiguous() {
        let a = turn(
            "t_DUP",
            "2026-01-01T00:00:00Z",
            vec![entry("src/a.rs", Some("h1"), Some("h2"))],
        );
        let b = turn(
            "t_DUP",
            "2026-01-01T00:00:00Z",
            vec![entry("src/a.rs", Some("h1"), Some("DIFFERENT"))],
        );
        let turns = vec![&a, &b];
        assert!(matches!(
            resolve_turn(&turns, "t_DUP"),
            Err(LookupError::Ambiguous { matched: 2 })
        ));
    }

    #[test]
    fn same_revert_is_order_independent_over_file_entries() {
        let a = turn(
            "t_X",
            "2026-01-01T00:00:00Z",
            vec![
                entry("src/a.rs", Some("h1"), Some("h2")),
                entry("src/b.rs", None, Some("h3")),
            ],
        );
        let b = turn(
            "t_X",
            "2026-01-01T00:00:00Z",
            vec![
                entry("src/b.rs", None, Some("h3")),
                entry("src/a.rs", Some("h1"), Some("h2")),
            ],
        );
        assert!(same_revert(&a, &b));
    }

    #[test]
    fn same_revert_rejects_differing_ids_and_file_counts() {
        let a = turn(
            "t_X",
            "2026-01-01T00:00:00Z",
            vec![entry("src/a.rs", Some("h1"), Some("h2"))],
        );
        let mut b = a.clone();
        b.id = "t_Y".to_string();
        assert!(!same_revert(&a, &b));
        let mut c = a.clone();
        c.files.push(entry("src/b.rs", None, Some("h3")));
        assert!(!same_revert(&a, &c));
    }
    // ---- Ledger tolerance (AC7) -----------------------------------------

    fn write_log(root: &std::path::Path, lines: &[&str]) {
        let dir = root.join(".agentrec");
        std::fs::create_dir_all(dir.join("objects")).unwrap();
        std::fs::write(dir.join("log.jsonl"), lines.join("\n") + "\n").unwrap();
    }

    fn turn_line(id: &str) -> String {
        serde_json::to_string(&LogRecord::Turn(turn(id, "2026-01-01T00:00:00Z", vec![]))).unwrap()
    }

    #[test]
    fn an_unknown_record_type_is_tolerated_and_counted_not_dropped_silently() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_log(
            root,
            &[
                &turn_line("t_A"),
                r#"{"type":"future_thing","v":1,"whatever":true}"#,
                &turn_line("t_B"),
            ],
        );
        let view = RepositoryView::open(root).unwrap();
        let ledger = view.ledger();
        // Tolerated: the surrounding turns still parse.
        assert_eq!(ledger.records.len(), 2);
        // Counted: the caller can tell a newer producer wrote something.
        assert_eq!(ledger.unknown_type_lines, 1);
        assert_eq!(ledger.unparsed_lines, 0);
        assert_eq!(view.health(u64::MAX).unwrap().unknown_type_lines, 1);
    }

    #[test]
    fn unknown_fields_on_a_known_record_do_not_make_it_unparsed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut v: serde_json::Value = serde_json::from_str(&turn_line("t_A")).unwrap();
        v["field_from_a_later_version"] = serde_json::json!("hello");
        write_log(root, &[&v.to_string()]);
        let ledger = RepositoryView::open(root).unwrap().ledger();
        assert_eq!(ledger.records.len(), 1);
        assert_eq!(ledger.unknown_type_lines, 0);
        assert_eq!(ledger.unparsed_lines, 0);
    }

    #[test]
    fn a_torn_tail_line_is_counted_as_unparsed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let good = turn_line("t_A");
        write_log(root, &[&good, &good[..good.len() / 2]]);
        let ledger = RepositoryView::open(root).unwrap().ledger();
        assert_eq!(ledger.records.len(), 1);
        assert_eq!(ledger.unparsed_lines, 1);
    }

    // Pre-P4, `status` printed a report in a directory that was never
    // `init`ed, while `log` printed "no turns recorded". Routing status
    // through the view must not turn that into an error exit.
    #[test]
    fn an_uninitialized_directory_reads_as_an_empty_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let view = RepositoryView::open(tmp.path()).unwrap();
        let h = view.health(1_000).unwrap();
        assert_eq!(h.turn_count, 0);
        assert_eq!(h.store_bytes, 0);
        assert!(!h.over_budget);
        assert!(view.list(&TurnQuery::default()).unwrap().items.is_empty());
    }

    // ---- Cursors (AC6) ---------------------------------------------------

    #[test]
    fn paging_covers_every_turn_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lines: Vec<String> = (0..5).map(|i| turn_line(&format!("t_{i}"))).collect();
        write_log(root, &lines.iter().map(String::as_str).collect::<Vec<_>>());
        let view = RepositoryView::open(root).unwrap();

        let mut seen = Vec::new();
        let mut q = TurnQuery {
            limit: Some(2),
            ..Default::default()
        };
        loop {
            let page = view.list(&q).unwrap();
            seen.extend(page.items.iter().map(|t| t.id.clone()));
            match page.next {
                Some(c) => q.after = Some(c),
                None => break,
            }
        }
        assert_eq!(seen, vec!["t_0", "t_1", "t_2", "t_3", "t_4"]);
    }

    #[test]
    fn a_cursor_replayed_after_the_ledger_grows_skips_and_duplicates_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lines: Vec<String> = (0..3).map(|i| turn_line(&format!("t_{i}"))).collect();
        write_log(root, &lines.iter().map(String::as_str).collect::<Vec<_>>());
        let view = RepositoryView::open(root).unwrap();
        let q = TurnQuery {
            limit: Some(2),
            ..Default::default()
        };
        let first = view.list(&q).unwrap();
        assert_eq!(first.items.len(), 2);

        // The daemon appends while the consumer holds the cursor.
        let mut all: Vec<String> = lines.clone();
        all.push(turn_line("t_3"));
        write_log(root, &all.iter().map(String::as_str).collect::<Vec<_>>());

        let q2 = TurnQuery {
            limit: Some(10),
            after: first.next,
            ..Default::default()
        };
        let second = view.list(&q2).unwrap();
        let ids: Vec<&str> = second.items.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["t_2", "t_3"]);
    }

    #[test]
    fn a_cursor_into_a_rewritten_ledger_is_stale_not_a_silent_reslide() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lines: Vec<String> = (0..4).map(|i| turn_line(&format!("t_{i}"))).collect();
        write_log(root, &lines.iter().map(String::as_str).collect::<Vec<_>>());
        let view = RepositoryView::open(root).unwrap();
        let first = view
            .list(&TurnQuery {
                limit: Some(2),
                ..Default::default()
            })
            .unwrap();
        let cursor = first.next.unwrap();

        // `purge --log-duplicates` shape: the record the cursor names is
        // rewritten away while the file stays the SAME LENGTH — a
        // length- or offset-keyed cursor would resume at the wrong record
        // and silently drop t_2.
        let before_len = std::fs::metadata(root.join(".agentrec/log.jsonl"))
            .unwrap()
            .len();
        let rewritten = [
            turn_line("t_0"),
            turn_line("t_9"), // replaces t_1, the record the cursor names
            turn_line("t_2"),
            turn_line("t_3"),
        ];
        write_log(
            root,
            &rewritten.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert_eq!(
            std::fs::metadata(root.join(".agentrec/log.jsonl"))
                .unwrap()
                .len(),
            before_len,
            "the rewrite must be same-length for this test to prove anything"
        );

        assert_eq!(
            view.list(&TurnQuery {
                limit: Some(2),
                after: Some(cursor),
                ..Default::default()
            })
            .unwrap_err(),
            CursorError::Stale
        );
    }

    #[test]
    fn a_cursor_replayed_under_a_different_query_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lines: Vec<String> = (0..3).map(|i| turn_line(&format!("t_{i}"))).collect();
        write_log(root, &lines.iter().map(String::as_str).collect::<Vec<_>>());
        let view = RepositoryView::open(root).unwrap();
        let cursor = view
            .list(&TurnQuery {
                limit: Some(1),
                ..Default::default()
            })
            .unwrap()
            .next
            .unwrap();
        assert_eq!(
            view.list(&TurnQuery {
                include_all: true,
                limit: Some(1),
                after: Some(cursor),
            })
            .unwrap_err(),
            CursorError::QueryMismatch
        );
    }

    #[test]
    fn a_truncated_ledger_makes_an_outstanding_cursor_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lines: Vec<String> = (0..4).map(|i| turn_line(&format!("t_{i}"))).collect();
        write_log(root, &lines.iter().map(String::as_str).collect::<Vec<_>>());
        let view = RepositoryView::open(root).unwrap();
        let cursor = view
            .list(&TurnQuery {
                limit: Some(3),
                ..Default::default()
            })
            .unwrap()
            .next
            .unwrap();
        write_log(root, &[&turn_line("t_0")]);
        assert_eq!(
            view.list(&TurnQuery {
                limit: Some(3),
                after: Some(cursor),
                ..Default::default()
            })
            .unwrap_err(),
            CursorError::Stale
        );
    }

    #[test]
    fn an_unpaginated_query_returns_everything_and_no_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lines: Vec<String> = (0..3).map(|i| turn_line(&format!("t_{i}"))).collect();
        write_log(root, &lines.iter().map(String::as_str).collect::<Vec<_>>());
        let page = RepositoryView::open(root)
            .unwrap()
            .list(&TurnQuery::default())
            .unwrap();
        assert_eq!(page.items.len(), 3);
        assert!(page.next.is_none());
    }

    #[test]
    fn health_on_a_zero_turn_repo_reports_zeroes_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_log(root, &[]);
        let h = RepositoryView::open(root).unwrap().health(1_000).unwrap();
        assert_eq!(h.turn_count, 0);
        assert_eq!(h.crash_gaps, 0);
        assert!(!h.over_budget);
    }
    #[test]
    fn a_known_type_written_malformed_is_unparsed_not_an_unknown_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Tagged `turn` — a kind this binary knows — but missing required
        // fields. Calling this an unknown record kind would assert a newer
        // producer exists on evidence of corruption.
        write_log(root, &[r#"{"type":"turn","v":1}"#]);
        let ledger = RepositoryView::open(root).unwrap().ledger();
        assert_eq!(ledger.records.len(), 0);
        assert_eq!(ledger.unknown_type_lines, 0);
        assert_eq!(ledger.unparsed_lines, 1);
    }

    #[test]
    fn a_retroactive_merge_appended_after_a_cursor_is_not_treated_as_a_rewrite() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let bare: Vec<String> = (0..3).map(|i| turn_line(&format!("t_{i}"))).collect();
        write_log(root, &bare.iter().map(String::as_str).collect::<Vec<_>>());
        let view = RepositoryView::open(root).unwrap();

        let first = view
            .list(&TurnQuery {
                limit: Some(2),
                ..Default::default()
            })
            .unwrap();
        let ids: Vec<&str> = first.items.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["t_0", "t_1"]);
        let cursor = first.next.unwrap();

        // Append-only, nothing rewritten: a rich turn absorbs t_1, which the
        // consumer has already been handed. t_1 leaves the default view while
        // remaining in the ledger — that is a merge, not a purge, and paging
        // must continue rather than report the ledger stale.
        let mut merger = turn("t_RICH", "2026-01-01T00:00:00Z", vec![]);
        merger.merges = vec!["t_1".to_string()];
        let mut grown = bare.clone();
        grown.push(serde_json::to_string(&LogRecord::Turn(merger)).unwrap());
        write_log(root, &grown.iter().map(String::as_str).collect::<Vec<_>>());

        let page = view
            .list(&TurnQuery {
                limit: Some(10),
                after: Some(cursor),
                ..Default::default()
            })
            .expect("a pure append must not invalidate an outstanding cursor");
        let ids: Vec<&str> = page.items.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["t_2", "t_RICH"]);
    }
    // ---- Gate round 1 findings -------------------------------------------

    // GATE FAIL (round 1, AC6). Ids are NOT unique in real ledgers: a pre-fix
    // daemon's orphan recovery re-appended a turn under its reserved id (the
    // shape `same_revert` and `purge --log-duplicates` exist for), and an
    // import can append a second turn under a resumed sessionId. Resolving a
    // cursor by FIRST match re-delivered every record between the two
    // occurrences — silently, with no Stale.
    #[test]
    fn a_cursor_after_a_duplicated_id_resumes_at_the_right_occurrence() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ledger = [
            turn_line("t_0"),
            turn_line("t_DUP"),
            turn_line("t_1"),
            turn_line("t_DUP"),
            turn_line("t_2"),
        ];
        write_log(root, &ledger.iter().map(String::as_str).collect::<Vec<_>>());
        let view = RepositoryView::open(root).unwrap();

        let first = view
            .list(&TurnQuery {
                limit: Some(4),
                ..Default::default()
            })
            .unwrap();
        let ids: Vec<&str> = first.items.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["t_0", "t_DUP", "t_1", "t_DUP"]);
        let cursor = first.next.unwrap();
        assert_eq!(
            cursor.after_occurrence, 1,
            "cursor sits after the SECOND t_DUP"
        );

        // Pure append — the AC's growth case.
        let mut grown: Vec<String> = ledger.to_vec();
        grown.push(turn_line("t_3"));
        write_log(root, &grown.iter().map(String::as_str).collect::<Vec<_>>());

        let page = view
            .list(&TurnQuery {
                limit: Some(10),
                after: Some(cursor),
                ..Default::default()
            })
            .unwrap();
        let ids: Vec<&str> = page.items.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["t_2", "t_3"], "no record may be delivered twice");
    }

    #[test]
    fn losing_the_named_occurrence_of_a_duplicated_id_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ledger = [turn_line("t_0"), turn_line("t_DUP"), turn_line("t_DUP")];
        write_log(root, &ledger.iter().map(String::as_str).collect::<Vec<_>>());
        let view = RepositoryView::open(root).unwrap();
        let cursor = view
            .list(&TurnQuery {
                limit: Some(3),
                ..Default::default()
            })
            .unwrap();
        // Page covered the whole ledger, so mint a cursor by paging shorter.
        assert!(cursor.next.is_none());
        let cursor = view
            .list(&TurnQuery {
                limit: Some(2),
                ..Default::default()
            })
            .unwrap()
            .next
            .unwrap();
        assert_eq!(cursor.after_occurrence, 0);

        // `purge --log-duplicates` collapses the pair to one record. The
        // occurrence the cursor named is still there, so paging continues —
        // but dropping to ONE occurrence when the cursor named the second
        // must be stale, not a silent restart.
        write_log(root, &[&turn_line("t_0"), &turn_line("t_DUP")]);
        let after_second = Cursor {
            after_id: "t_DUP".to_string(),
            after_occurrence: 1,
            query: TurnQuery::default().fingerprint(),
        };
        assert_eq!(
            view.list(&TurnQuery {
                limit: Some(2),
                after: Some(after_second),
                ..Default::default()
            })
            .unwrap_err(),
            CursorError::Stale
        );
    }

    // A zero-length page is not end-of-ledger, and it has no honest cursor
    // either — so it is refused rather than answered. Dark-launched surface,
    // but a pager told "no more" on a non-empty ledger stops early and loses
    // data.
    #[test]
    fn a_zero_limit_query_is_refused_rather_than_answered_end_of_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lines: Vec<String> = (0..3).map(|i| turn_line(&format!("t_{i}"))).collect();
        write_log(root, &lines.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(
            RepositoryView::open(root)
                .unwrap()
                .list(&TurnQuery {
                    limit: Some(0),
                    ..Default::default()
                })
                .unwrap_err(),
            CursorError::ZeroLimit
        );
    }
    // GATE finding (round 1): the `type` guard keyed on the tag being a
    // STRING, so `"type": 5` fell through to the legacy no-tag fallback and
    // was coerced into a turn — while the comment beside it promised a line
    // carrying a `type` is never coerced. Pre-P4 `load_log` keyed on
    // presence; this pins that back.
    #[test]
    fn a_non_string_type_tag_is_never_coerced_into_a_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut v: serde_json::Value = serde_json::from_str(&turn_line("t_A")).unwrap();
        v["type"] = serde_json::json!(5);
        write_log(root, &[&v.to_string()]);
        let ledger = RepositoryView::open(root).unwrap().ledger();
        assert!(
            ledger.records.is_empty(),
            "a line carrying a type tag must never be read as a turn"
        );
        assert_eq!(
            ledger.unknown_type_lines, 0,
            "a number is not a record kind"
        );
        assert_eq!(ledger.unparsed_lines, 1);
    }

    // ---- diff (AC7, AC8, AC13) -------------------------------------------

    fn store_of(root: &std::path::Path) -> crate::store::BlobStore {
        crate::store::BlobStore::new(root.join(".agentrec").join("objects"))
    }

    /// Overwrite a stored object's bytes so its hash no longer verifies —
    /// the only way to reach `StoreError::Corrupt` honestly.
    fn corrupt_blob(root: &std::path::Path, hash: &str, tampered: &[u8]) {
        let hex = hash.strip_prefix("sha256:").unwrap();
        let (fan, rest) = hex.split_at(2);
        let path = root.join(".agentrec").join("objects").join(fan).join(rest);
        std::fs::write(&path, tampered).unwrap();
    }

    fn epoch_line(event: &str, ts: &str) -> String {
        serde_json::to_string(&epoch(event, ts)).unwrap()
    }

    fn seed(root: &std::path::Path, turns: &[&TurnRecord]) {
        let lines: Vec<String> = turns
            .iter()
            .map(|t| serde_json::to_string(&LogRecord::Turn((*t).clone())).unwrap())
            .collect();
        write_log(root, &lines.iter().map(String::as_str).collect::<Vec<_>>());
    }

    #[test]
    fn diff_of_an_unknown_ref_carries_the_unfiltered_ledger_range() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Two records under one id, exactly as `diff_unknown_id.golden`'s
        // fixture has: the count is over the UNFILTERED vec, so it exceeds
        // the number of distinct ids.
        write_log(
            root,
            &[
                &turn_line("t_AAAA1"),
                &turn_line("t_DUPXX"),
                &turn_line("t_DUPXX"),
                &turn_line("t_ZZZZ9"),
            ],
        );
        let view = RepositoryView::open(root).unwrap();
        let err = view
            .diff(&DiffQuery {
                turn: "t_NOPE".to_string(),
                ..Default::default()
            })
            .unwrap_err();
        let DiffError::Lookup { err, ledger } = err else {
            panic!("expected a lookup failure, got {err:?}");
        };
        assert_eq!(err, LookupError::Unknown);
        assert_eq!(
            ledger,
            Some(TurnRangeSummary {
                oldest_id: "t_AAAA1".to_string(),
                newest_id: "t_ZZZZ9".to_string(),
                count: 4,
            }),
            "the range counts records, not distinct ids"
        );
    }

    #[test]
    fn diff_on_an_empty_ledger_has_no_range_to_name() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_log(root, &[]);
        let err = RepositoryView::open(root)
            .unwrap()
            .diff(&DiffQuery {
                turn: "t_ANY".to_string(),
                ..Default::default()
            })
            .unwrap_err();
        assert!(
            matches!(
                err,
                DiffError::Lookup {
                    err: LookupError::NoTurns,
                    ledger: None
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn diff_of_a_duplicated_id_collapses_but_distinct_turns_stay_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let files = vec![entry("src/a.rs", None, None)];
        // `same_revert` compares id + file entries only, so `tool` may drift
        // between the two records of one turn — which makes it the field
        // that reveals WHICH record the collapse picked. `turn_id` cannot:
        // it is equal on both by construction.
        let mut a = turn("t_DUP", "2026-01-01T00:00:00Z", files.clone());
        a.tool = Some("first-record".to_string());
        let mut b = turn("t_DUP", "2026-01-01T09:00:00Z", files);
        b.truncated = true;
        b.tool = Some("second-record".to_string());
        seed(root, &[&a, &b]);
        let view = RepositoryView::open(root).unwrap();
        let q = DiffQuery {
            turn: "t_DUP".to_string(),
            ..Default::default()
        };
        let collapsed = view.diff(&q).unwrap();
        assert_eq!(collapsed.turn_id, "t_DUP");
        assert_eq!(
            collapsed.tool.as_deref(),
            Some("first-record"),
            "the collapse resolves to the FIRST matching record"
        );

        // Same id, DIFFERENT revert — must not collapse.
        let mut c = turn(
            "t_DUP",
            "2026-01-01T00:00:00Z",
            vec![entry("src/b.rs", None, None)],
        );
        c.truncated = true;
        seed(root, &[&a, &c]);
        assert!(
            matches!(
                view.diff(&q).unwrap_err(),
                DiffError::Lookup {
                    err: LookupError::Ambiguous { matched: 2 },
                    ..
                }
            ),
            "distinct turns sharing an id must stay ambiguous"
        );
    }

    // AC8. A fileless turn is data, not an error: the header facts must
    // survive an empty page or `turn <id> · <tool> · 0 files` becomes
    // unproducible.
    #[test]
    fn diff_of_a_fileless_turn_keeps_the_header_facts_on_an_empty_page() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut t = turn("t_FILELESS", "2026-01-01T00:00:00Z", vec![]);
        t.tool = Some("claude".to_string());
        seed(root, &[&t]);
        let r = RepositoryView::open(root)
            .unwrap()
            .diff(&DiffQuery {
                turn: "t_FILELESS".to_string(),
                ..Default::default()
            })
            .unwrap();
        assert!(r.files.items.is_empty());
        assert!(r.files.next.is_none());
        assert_eq!(r.turn_id, "t_FILELESS");
        assert_eq!(r.tool.as_deref(), Some("claude"));
        assert_eq!(r.total_files, 0);
    }

    #[test]
    fn diff_resolves_every_print_entry_arm_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_log(root, &[]);
        let store = store_of(root);
        let before = store.put(b"a\nb\n").unwrap();
        let after = store.put(b"a\nB\n").unwrap();
        let binary = store.put(&[0u8, 1, 2, 3]).unwrap();
        let created = store.put(b"new\n").unwrap();
        let gone = crate::store::hash_bytes(b"never-stored");
        let rotten = store.put(b"rots").unwrap();
        corrupt_blob(root, &rotten, b"tampered");

        let mut withheld = entry("secret.env", None, None);
        withheld.withheld = true;
        let mut skipped = entry("big.bin", None, None);
        skipped.skipped = true;
        skipped.skipped_reason = Some("over_cap".to_string());
        // Both sides fail; the BEFORE side is the one reported.
        let both_bad = entry("both.rs", Some(&gone), Some(&rotten));
        let corrupt_after = entry("corrupt.rs", Some(&before), Some(&rotten));
        let bin = entry("img.bin", Some(&binary), Some(&binary));
        let mut baseline = entry("fresh.rs", None, Some(&created));
        baseline.baseline_unknown = true;
        let mut synth = entry("synth.rs", Some(&before), Some(&after));
        synth.after_synthesized = Some(true);

        let t = turn(
            "t_ARMS",
            "2026-01-01T00:00:00Z",
            vec![
                withheld,
                skipped,
                both_bad,
                corrupt_after,
                bin,
                baseline,
                synth,
            ],
        );
        seed(root, &[&t]);
        let r = RepositoryView::open(root)
            .unwrap()
            .diff(&DiffQuery {
                turn: "t_ARMS".to_string(),
                ..Default::default()
            })
            .unwrap();
        let states: Vec<&FileDiffState> = r.files.items.iter().map(|f| &f.state).collect();
        assert_eq!(states[0], &FileDiffState::Withheld);
        assert_eq!(
            states[1],
            &FileDiffState::Skipped {
                reason: Some("over_cap".to_string())
            }
        );
        assert_eq!(
            states[2],
            &FileDiffState::Unresolvable { corrupt: false },
            "a missing BEFORE outranks a corrupt after"
        );
        assert_eq!(states[3], &FileDiffState::Unresolvable { corrupt: true });
        assert_eq!(
            states[4],
            &FileDiffState::Binary {
                before_len: 4,
                after_len: 4
            }
        );
        assert_eq!(
            states[5],
            &FileDiffState::BaselineUnknown {
                after: "new\n".to_string()
            },
            "baseline-unknown is decided before the synthesized-after question"
        );
        assert_eq!(
            states[6],
            &FileDiffState::Text {
                before: "a\nb\n".to_string(),
                after: "a\nB\n".to_string(),
                op: "modify".to_string(),
                after_synthesized: true,
            }
        );
        assert_eq!(r.total_files, 7);
    }

    // A `create`'s absent `before` hash is legitimately-empty content, never
    // unresolvable — collapsing the two would deny credit to every
    // file-creating turn (the same rule `load_text` carries for blame).
    #[test]
    fn a_creates_absent_before_hash_is_empty_text_not_unresolvable() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_log(root, &[]);
        let store = store_of(root);
        let created = store.put(b"hello\n").unwrap();
        let mut e = entry("new.rs", None, Some(&created));
        e.op = "create".to_string();
        let t = turn("t_CREATE", "2026-01-01T00:00:00Z", vec![e]);
        seed(root, &[&t]);
        let r = RepositoryView::open(root)
            .unwrap()
            .diff(&DiffQuery {
                turn: "t_CREATE".to_string(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            r.files.items[0].state,
            FileDiffState::Text {
                before: String::new(),
                after: "hello\n".to_string(),
                op: "create".to_string(),
                after_synthesized: false,
            }
        );
    }

    fn four_entry_turn(root: &std::path::Path, second: &str) -> RepositoryView {
        let t = turn(
            "t_PAGE",
            "2026-01-01T00:00:00Z",
            vec![
                entry("a.rs", None, None),
                entry(second, None, None),
                entry("c.rs", None, None),
                entry("d.rs", None, None),
            ],
        );
        seed(root, &[&t]);
        RepositoryView::open(root).unwrap()
    }

    #[test]
    fn diff_paging_covers_every_entry_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let view = four_entry_turn(root, "b.rs");
        let mut q = DiffQuery {
            turn: "t_PAGE".to_string(),
            limit: Some(2),
            ..Default::default()
        };
        let mut seen = Vec::new();
        loop {
            let page = view.diff(&q).unwrap();
            assert_eq!(page.total_files, 4, "the header count is never the page's");
            seen.extend(page.files.items.iter().map(|f| f.path.clone()));
            match page.files.next {
                Some(c) => q.after = Some(c),
                None => break,
            }
        }
        assert_eq!(seen, vec!["a.rs", "b.rs", "c.rs", "d.rs"]);
    }

    // AC13 (diff half). The cursor names an entry, not an index: a rewrite
    // that dropped it must be Stale, never a silent reslide onto whatever
    // now sits at that position.
    //
    // SUBSTITUTED REWRITE (founder-ratified). AC13 names `purge
    // --log-duplicates`, which a diff cursor provably cannot be staled by:
    // that command drops only duplicates `same_revert` accepts, and
    // `same_revert` requires SET-EQUAL file entries — so the surviving
    // record's entry list is identical to the dropped one's, and a
    // path-keyed cursor still resolves. The same rewrite CLASS (a
    // same-length, in-place `log.jsonl` rewrite that changes what the
    // resolved turn holds) stands in, and the `before_len` guard below is
    // what keeps the substitution honest: without it the test would also
    // pass against a length-keyed cursor, which is the very design this
    // assertion exists to refuse.
    #[test]
    fn a_diff_cursor_into_a_rewritten_turn_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let view = four_entry_turn(root, "b.rs");
        let cursor = view
            .diff(&DiffQuery {
                turn: "t_PAGE".to_string(),
                limit: Some(2),
                ..Default::default()
            })
            .unwrap()
            .files
            .next
            .unwrap();
        assert_eq!(cursor.after_id, "b.rs");

        let before_len = std::fs::metadata(root.join(".agentrec/log.jsonl"))
            .unwrap()
            .len();
        // Same-length rewrite: `b.rs` becomes `z.rs`, so a length- or
        // index-keyed cursor would resume mid-list and silently drop c.rs.
        four_entry_turn(root, "z.rs");
        assert_eq!(
            std::fs::metadata(root.join(".agentrec/log.jsonl"))
                .unwrap()
                .len(),
            before_len,
            "the rewrite must be same-length for this test to prove anything"
        );

        assert!(
            matches!(
                view.diff(&DiffQuery {
                    turn: "t_PAGE".to_string(),
                    limit: Some(2),
                    after: Some(cursor),
                    ..Default::default()
                })
                .unwrap_err(),
                DiffError::Cursor(CursorError::Stale)
            ),
            "a dropped entry is stale, not a reslide"
        );
    }

    // The fingerprint binds BOTH discriminators — a cursor that bound only
    // the turn would pass the first half of this test and fail the second.
    #[test]
    fn a_diff_cursor_from_another_query_is_refused_on_turn_and_on_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let a = turn(
            "t_ONE",
            "2026-01-01T00:00:00Z",
            vec![
                entry("a.rs", None, None),
                entry("b.rs", None, None),
                entry("c.rs", None, None),
            ],
        );
        let b = turn(
            "t_TWO",
            "2026-01-01T00:00:00Z",
            vec![
                entry("a.rs", None, None),
                entry("b.rs", None, None),
                entry("c.rs", None, None),
            ],
        );
        seed(root, &[&a, &b]);
        let view = RepositoryView::open(root).unwrap();
        let cursor = view
            .diff(&DiffQuery {
                turn: "t_ONE".to_string(),
                limit: Some(1),
                ..Default::default()
            })
            .unwrap()
            .files
            .next
            .unwrap();

        // Different turn — the entry names would resolve, so only the
        // fingerprint can catch this.
        assert!(matches!(
            view.diff(&DiffQuery {
                turn: "t_TWO".to_string(),
                limit: Some(1),
                after: Some(cursor.clone()),
                ..Default::default()
            })
            .unwrap_err(),
            DiffError::Cursor(CursorError::QueryMismatch)
        ));
        // Same turn, different path filter.
        assert!(matches!(
            view.diff(&DiffQuery {
                turn: "t_ONE".to_string(),
                paths: Some(vec!["a.rs".to_string(), "b.rs".to_string()]),
                limit: Some(1),
                after: Some(cursor),
            })
            .unwrap_err(),
            DiffError::Cursor(CursorError::QueryMismatch)
        ));
    }

    #[test]
    fn a_diff_paths_filter_selects_entries_but_never_the_header_count() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let view = four_entry_turn(root, "b.rs");
        let r = view
            .diff(&DiffQuery {
                turn: "t_PAGE".to_string(),
                paths: Some(vec!["c.rs".to_string()]),
                ..Default::default()
            })
            .unwrap();
        let paths: Vec<&str> = r.files.items.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["c.rs"]);
        assert_eq!(r.total_files, 4);
    }

    #[test]
    fn a_zero_limit_diff_is_refused_rather_than_answered_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let view = four_entry_turn(root, "b.rs");
        assert!(matches!(
            view.diff(&DiffQuery {
                turn: "t_PAGE".to_string(),
                limit: Some(0),
                ..Default::default()
            })
            .unwrap_err(),
            DiffError::Cursor(CursorError::ZeroLimit)
        ));
    }

    // ---- blame (AC9) ------------------------------------------------------

    fn touch(root: &std::path::Path, rel: &str, content: &[u8]) {
        std::fs::write(root.join(rel), content).unwrap();
    }

    fn blame_of(root: &std::path::Path, path: &str, line: Option<usize>) -> BlameResult {
        RepositoryView::open(root)
            .unwrap()
            .blame(&BlameQuery {
                path: path.to_string(),
                line,
            })
            .unwrap()
    }

    // AC9: an uncovered path names NO attributor — the gap state carries no
    // turn at all, so a renderer physically cannot print one.
    #[test]
    fn blame_of_an_uncovered_path_names_no_attributor() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Two `start`s with no `stop` between them: the daemon crashed.
        write_log(
            root,
            &[
                &epoch_line("start", "2026-01-01T00:00:00Z"),
                &epoch_line("start", "2026-01-01T05:00:00Z"),
            ],
        );
        touch(root, "orphan.rs", b"line one\n");
        assert!(matches!(
            blame_of(root, "orphan.rs", None).state,
            BlameState::NoTurnRecordingGap
        ));
        assert!(matches!(
            blame_of(root, "orphan.rs", Some(1)).state,
            BlameState::LineRecordingGap
        ));
    }

    // A TRAILING STOP is an uncovered interval but not a CRASH, and the two
    // levels answer it differently on one fixture: the file level keys on
    // `has_crash_gap` and so says "no turn touches", while the line walk
    // keys on `has_gap_after(records, "")` and so refuses to claim the line
    // predates recording. Holding both on one ledger is what makes this more
    // than a restatement of `blame_untouched_file_no_gap.golden`.
    #[test]
    fn an_untouched_path_answers_differently_at_file_and_line_level() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_log(
            root,
            &[
                &epoch_line("start", "2026-01-01T00:00:00Z"),
                &epoch_line("stop", "2026-01-01T01:00:00Z"),
            ],
        );
        touch(root, "orphan.rs", b"line one\n");
        assert!(
            matches!(
                blame_of(root, "orphan.rs", None).state,
                BlameState::NoTurnTouches
            ),
            "a deliberate stop is not a crash, so the file level names no gap"
        );
        assert!(
            matches!(
                blame_of(root, "orphan.rs", Some(1)).state,
                BlameState::LineOriginGap
            ),
            "the line walk must not call an uncovered origin 'before recording began'"
        );
    }

    // The two predicates stay unconflated: `modified` is hash-≠-after;
    // `gap_stale` additionally requires an uncovered interval AFTER the turn
    // ended. A fixture whose gap predates the turn must set one and not the
    // other.
    #[test]
    fn blame_file_keeps_modified_and_gap_stale_unconflated() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let store_dir = root.join(".agentrec").join("objects");
        std::fs::create_dir_all(&store_dir).unwrap();
        let store = crate::store::BlobStore::new(&store_dir);
        let recorded = store.put(b"recorded\n").unwrap();

        let t = turn(
            "t_TOUCH",
            "2026-01-01T03:00:00Z",
            vec![entry("x.rs", None, Some(&recorded))],
        );
        let turn_line = serde_json::to_string(&LogRecord::Turn(t.clone())).unwrap();

        // Gap entirely BEFORE the turn ended.
        write_log(
            root,
            &[
                &epoch_line("start", "2026-01-01T00:00:00Z"),
                &epoch_line("stop", "2026-01-01T01:00:00Z"),
                &epoch_line("start", "2026-01-01T02:00:00Z"),
                &turn_line,
            ],
        );
        touch(root, "x.rs", b"edited by a human\n");
        let BlameState::File {
            modified,
            gap_stale,
            deleted,
            ..
        } = blame_of(root, "x.rs", None).state
        else {
            panic!("expected a file-level attribution");
        };
        assert!(modified, "on-disk bytes differ from the recorded after");
        assert!(!gap_stale, "a gap before the turn ended does not stale it");
        assert!(!deleted);

        // Same turn, gap AFTER it ended.
        write_log(
            root,
            &[
                &epoch_line("start", "2026-01-01T00:00:00Z"),
                &turn_line,
                &epoch_line("stop", "2026-01-01T04:00:00Z"),
            ],
        );
        let BlameState::File {
            modified,
            gap_stale,
            ..
        } = blame_of(root, "x.rs", None).state
        else {
            panic!("expected a file-level attribution");
        };
        assert!(modified);
        assert!(gap_stale);

        // Unmodified: neither predicate fires, even with the same trailing gap.
        touch(root, "x.rs", b"recorded\n");
        let BlameState::File {
            modified,
            gap_stale,
            ..
        } = blame_of(root, "x.rs", None).state
        else {
            panic!("expected a file-level attribution");
        };
        assert!(!modified);
        assert!(!gap_stale, "gap_stale can never outlive modified");
    }

    /// A repo whose ledger is gap-free, holding `turns` in append order.
    fn gap_free(root: &std::path::Path, turns: &[&TurnRecord]) {
        let mut lines = vec![epoch_line("start", "2026-01-01T00:00:00Z")];
        lines.extend(
            turns
                .iter()
                .map(|t| serde_json::to_string(&LogRecord::Turn((*t).clone())).unwrap()),
        );
        write_log(root, &lines.iter().map(String::as_str).collect::<Vec<_>>());
    }

    #[test]
    fn blame_line_credits_the_newest_turn_that_introduced_the_text() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec").join("objects")).unwrap();
        let store = store_of(root);
        let v1 = store.put(b"one\n").unwrap();
        let v2 = store.put(b"one\ntwo\n").unwrap();
        let v3 = store.put(b"one\ntwo\nthree\n").unwrap();

        let a = turn(
            "t_OLD",
            "2026-01-01T01:00:00Z",
            vec![entry("f.rs", Some(&v1), Some(&v2))],
        );
        let b = turn(
            "t_NEW",
            "2026-01-01T02:00:00Z",
            vec![entry("f.rs", Some(&v2), Some(&v3))],
        );
        gap_free(root, &[&a, &b]);
        touch(root, "f.rs", b"one\ntwo\nthree\n");

        let BlameState::LineAttributed { turn } = blame_of(root, "f.rs", Some(3)).state else {
            panic!("line 3 must be attributed");
        };
        assert_eq!(turn.id, "t_NEW");
        let BlameState::LineAttributed { turn } = blame_of(root, "f.rs", Some(2)).state else {
            panic!("line 2 must be attributed");
        };
        assert_eq!(turn.id, "t_OLD");
    }

    // The poisoning rule, in both directions: a NEWER unresolvable candidate
    // suppresses the credit; an OLDER one does not.
    #[test]
    fn blame_line_credit_is_poisoned_only_by_a_newer_unresolvable_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec").join("objects")).unwrap();
        let store = store_of(root);
        let v1 = store.put(b"one\n").unwrap();
        let v2 = store.put(b"one\ntwo\n").unwrap();
        let ghost = crate::store::hash_bytes(b"never-stored");
        assert!(
            store.get(&ghost).is_err(),
            "precondition: ghost is unresolvable"
        );

        let introducer = turn(
            "t_INTRO",
            "2026-01-01T01:00:00Z",
            vec![entry("f.rs", Some(&v1), Some(&v2))],
        );
        let broken_after = turn(
            "t_BROKEN",
            "2026-01-01T02:00:00Z",
            vec![entry("f.rs", Some(&v2), Some(&ghost))],
        );
        let broken_before = turn(
            "t_BROKEN0",
            "2026-01-01T00:30:00Z",
            vec![entry("f.rs", Some(&ghost), Some(&v1))],
        );
        touch(root, "f.rs", b"one\ntwo\n");

        // Newer unresolvable candidate → the credit is withheld.
        gap_free(root, &[&introducer, &broken_after]);
        assert!(
            matches!(
                blame_of(root, "f.rs", Some(2)).state,
                BlameState::LineSnapshotUnavailable
            ),
            "a newer unresolvable candidate could have overwritten the line"
        );

        // Older unresolvable candidate → the credit stands.
        gap_free(root, &[&broken_before, &introducer]);
        let BlameState::LineAttributed { turn } = blame_of(root, "f.rs", Some(2)).state else {
            panic!("an OLDER unresolvable candidate must not poison the credit");
        };
        assert_eq!(turn.id, "t_INTRO");
    }

    // "before recording began" is only honest on a gap-free history; the
    // same walk over a ledger with any gap must say so instead — and the two
    // are distinct states because the CLI prefixes one and not the other.
    #[test]
    fn blame_line_never_claims_before_recording_when_history_has_a_gap() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec").join("objects")).unwrap();
        let store = store_of(root);
        let v1 = store.put(b"one\n").unwrap();
        let v2 = store.put(b"one\nappended\n").unwrap();
        // The turn touches the file but introduces neither current line's
        // text at line 1 — line 1 predates it.
        let t = turn(
            "t_ONLY",
            "2026-01-01T01:00:00Z",
            vec![entry("f.rs", Some(&v1), Some(&v2))],
        );
        touch(root, "f.rs", b"one\nappended\n");

        gap_free(root, &[&t]);
        assert!(matches!(
            blame_of(root, "f.rs", Some(1)).state,
            BlameState::LineBeforeRecording
        ));

        // Same walk, but the daemon was off for an interval: the origin is
        // unknown, not "before recording began".
        let turn_line = serde_json::to_string(&LogRecord::Turn(t.clone())).unwrap();
        write_log(
            root,
            &[
                &epoch_line("start", "2026-01-01T00:00:00Z"),
                &turn_line,
                &epoch_line("stop", "2026-01-01T02:00:00Z"),
            ],
        );
        assert!(matches!(
            blame_of(root, "f.rs", Some(1)).state,
            BlameState::LineOriginGap
        ));
    }

    #[test]
    fn blame_line_on_a_deleted_file_resolves_to_the_deleting_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec").join("objects")).unwrap();
        let store = store_of(root);
        let v1 = store.put(b"one\n").unwrap();
        let mut e = entry("gone.rs", Some(&v1), None);
        e.op = "delete".to_string();
        let t = turn("t_DEL", "2026-01-01T01:00:00Z", vec![e]);
        gap_free(root, &[&t]);
        let BlameState::LineDeletedOrAbsent { turn, deleted } =
            blame_of(root, "gone.rs", Some(1)).state
        else {
            panic!("a deleted file resolves any line to the deleting turn");
        };
        assert_eq!(turn.id, "t_DEL");
        assert!(deleted);
    }

    // AC19's two user-error arms, as typed data. Neither carries prose.
    #[test]
    fn blame_line_past_eof_reports_the_line_count_not_a_message() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_log(root, &[&epoch_line("start", "2026-01-01T00:00:00Z")]);
        touch(root, "f.rs", b"one\ntwo\n");
        let err = RepositoryView::open(root)
            .unwrap()
            .blame(&BlameQuery {
                path: "f.rs".to_string(),
                line: Some(99),
            })
            .unwrap_err();
        assert_eq!(err, BlameError::LineOutOfRange { lines: 2 });
        // Unreachable from the CLI (`parse_target` requires a positive
        // integer), but covered for a future MCP caller.
        let zero = RepositoryView::open(root)
            .unwrap()
            .blame(&BlameQuery {
                path: "f.rs".to_string(),
                line: Some(0),
            })
            .unwrap_err();
        assert_eq!(zero, BlameError::LineOutOfRange { lines: 2 });
    }

    #[test]
    fn blame_line_on_an_absent_untouched_file_is_file_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_log(root, &[&epoch_line("start", "2026-01-01T00:00:00Z")]);
        let err = RepositoryView::open(root)
            .unwrap()
            .blame(&BlameQuery {
                path: "nope.rs".to_string(),
                line: Some(1),
            })
            .unwrap_err();
        assert_eq!(err, BlameError::FileNotFound);
    }

    // BL1 (moved from `readcmds` with the logic it guards): `load_text` must
    // tell "legitimately empty" (no hash at all, e.g. a `create` op's
    // `before`) apart from "unresolvable" (a hash is recorded but the blob
    // won't load) — collapsing both to `""` is exactly the bug (blame
    // credits any turn for any line once one candidate's `before` goes
    // missing).
    #[test]
    fn load_text_distinguishes_absent_resolvable_and_unresolvable() {
        let tmp = tempfile::tempdir().unwrap();
        let store = crate::store::BlobStore::new(tmp.path().join("objects"));

        // Absent hash (e.g. a `create` op's `before`) — legitimately empty.
        assert_eq!(load_text(&store, None), Some(String::new()));

        // Resolvable hash — real content comes back.
        let hash = store.put(b"hello\n").unwrap();
        assert_eq!(load_text(&store, Some(&hash)), Some("hello\n".to_string()));

        // Present-but-unresolvable hash: well-formed, genuinely never
        // stored — proves the store really can't resolve it, rather than
        // assuming so.
        let ghost = crate::store::hash_bytes(b"never-actually-stored");
        assert!(
            store.get(&ghost).is_err(),
            "precondition: ghost must be genuinely unresolvable"
        );
        assert_eq!(load_text(&store, Some(&ghost)), None);
    }
}
