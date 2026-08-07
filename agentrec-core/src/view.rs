//! Read-side interpretation, split out of the renderer-coupled CLI.
//!
//! Everything here answers "what does the ledger mean" and nothing here
//! renders. Errors are typed so the human CLI keeps ownership of its prose
//! (and of `fmt::short_id`) while `--json` and, later, MCP read the same
//! interpretation without going through a string.

use crate::memory::{self, Pin};
use crate::record::{FileEntry, LogRecord, TurnRecord};
use serde::{Deserialize, Serialize};

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

/// How many times (E2 crash shape only). Kept as the crash-specific
/// primitive; `status` no longer reports ONLY this — see [`gap_counts`].
pub fn crash_gap_count(records: &[LogRecord]) -> usize {
    recording_gaps(records)
        .iter()
        .filter(|g| g.kind == GapKind::Crash)
        .count()
}

/// Every uncovered interval, counted per kind (redteam round 2, F13).
///
/// `status` previously surfaced [`crash_gap_count`] alone, so the intervals
/// [`recording_gaps`] tags [`GapKind::Restart`] and [`GapKind::TrailingStop`]
/// were reported as no gap at all — a daemon killed, damage done, daemon
/// restarted read `gaps: 0` on the one verb README advertises as reporting
/// recording gaps. The kinds stay separate rather than collapsing into one
/// number: "the recorder crashed" and "the recorder was deliberately off"
/// are different facts about the same missing coverage, and the callers that
/// legitimately want only one shape ([`has_crash_gap`]) keep it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GapCounts {
    /// A `start` while one was already open ([`GapKind::Crash`]).
    pub crash: usize,
    /// A clean `stop` later followed by a `start` ([`GapKind::Restart`]).
    pub restart: usize,
    /// A trailing `stop` with no `start` after it — everything from that
    /// stop onward is uncovered ([`GapKind::TrailingStop`]). This is the
    /// shape a stopped recorder leaves behind.
    pub trailing_stop: usize,
}

impl GapCounts {
    /// Uncovered intervals of any kind.
    pub fn total(&self) -> usize {
        self.crash + self.restart + self.trailing_stop
    }
}

/// Count [`recording_gaps`] by kind — one walk of the already-parsed
/// records per call.
pub fn gap_counts(records: &[LogRecord]) -> GapCounts {
    let mut c = GapCounts::default();
    for g in recording_gaps(records) {
        match g.kind {
            GapKind::Crash => c.crash += 1,
            GapKind::Restart => c.restart += 1,
            GapKind::TrailingStop => c.trailing_stop += 1,
        }
    }
    c
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
    /// Well-formed JSON objects this binary does not implement: a `type` it
    /// does not know, or a known `type` at an unimplemented schema major
    /// (record.rs refuses those at deserialization). Tolerated (a newer
    /// producer is allowed to write them) and counted.
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
    // fsguard: this is a second log reader that `record::load_log`'s guard
    // does not cover.
    let Ok(file) = crate::fsguard::open_regular(path) else {
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
///
/// `Serialize` (P5): `status --json` flattens this in alongside its
/// pre-existing `state.json`-derived operational fields (additive, not a
/// replacement — see `cmds::StatusJson`'s doc comment). Every field here is
/// unconditionally present; none is "usually absent" in the sense that would
/// call for `skip_serializing_if`.
#[derive(Debug, Clone, Serialize)]
pub struct RepositoryHealth {
    /// Every byte under `.agentrec/objects/`, orphans included — a `read_dir`
    /// walk ([`crate::store::BlobStore::total_bytes`]). Unchanged meaning; it
    /// is simply no longer what `over_budget` compares (F26).
    ///
    /// Scope note (F24): this walks `objects/` only, so `.agentrec/daemon.log`
    /// — where the launchd unit now routes the daemon's stdio — is a sibling
    /// of the store and invisible to both this figure and `budgeted_bytes`.
    pub store_bytes: u64,
    /// The bytes `budget` is actually enforced against:
    /// [`crate::retention::managed_bytes`] over every turn record in the
    /// ledger — the unique snapshot blobs `retention::plan_eviction`
    /// accumulates. Strictly `<= store_bytes`; the difference is orphans,
    /// prompt-only blobs, and anything else the evictor cannot reclaim.
    ///
    /// F26 (redteam round 2): `over_budget` used to be `store_bytes > budget`,
    /// so a store whose orphan share alone exceeded the budget sat
    /// permanently "over budget" while every eviction tick freed nothing. The
    /// two figures are now both reported rather than one silently standing in
    /// for the other — a consumer that wants disk pressure reads
    /// `store_bytes`, one that wants "will eviction help" reads this.
    pub budgeted_bytes: u64,
    pub budget: u64,
    /// `budgeted_bytes > budget` — equivalently (see
    /// [`crate::retention::managed_bytes`]'s proof) "the evictor has
    /// candidates". NOT "eviction will free bytes": A2/A5/protected/freshness
    /// can spare every candidate. NOT "the store is over budget on disk"
    /// either — that is `store_bytes > budget`, a strictly weaker condition
    /// this field deliberately no longer answers.
    pub over_budget: bool,
    /// Turns as recorded, before any superseded/git filtering.
    pub turn_count: usize,
    /// Crash-shaped recording gaps ([`GapKind::Crash`]).
    pub crash_gaps: usize,
    /// Restart-shaped recording gaps ([`GapKind::Restart`]) — F13. Additive
    /// sibling of `crash_gaps`, never a replacement: `crash_gaps` keeps its
    /// established meaning and value for every consumer already reading it.
    pub restart_gaps: usize,
    /// Trailing-stop recording gaps ([`GapKind::TrailingStop`]) — F13. At
    /// most 1 by construction (only the last unmatched `stop` produces one).
    pub trailing_stop_gaps: usize,
    pub unknown_type_lines: usize,
    pub unparsed_lines: usize,
}

/// One page of results plus the cursor that continues it.
///
/// `Serialize` (P5): `next` deliberately has NO `skip_serializing_if` — it is
/// a pagination terminator, and its absence must never be confusable with a
/// serializer that omits nulls (see `view.rs`'s module doc / P5.md's note on
/// this exact asymmetry, raised by the P4b-5 gate).
#[derive(Debug, Clone, Serialize)]
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
/// `Deserialize` (E2): the MCP read tools hand the cursor to the agent as
/// data and take it back on the next call, so the round-trip needs a reader
/// as well as a writer. Purely additive — no CLI path deserializes a cursor
/// (no CLI verb accepts one), and the field set is unchanged, so nothing the
/// serializer emits moves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
///
/// `Serialize` (E2): `agentrec_log` returns `Page<TurnSummary>` verbatim
/// (P4b decision 12 — the MCP log tool mirrors `list()`, NOT `log --json`,
/// which emits protocol JSONL). Additive and CLI-invisible: the only CLI
/// reader of this type is `cmds.rs`'s rich-rate window, which counts `grade`
/// and `imported` and never serializes a summary
/// (`grep -rn TurnSummary cli/src/`).
#[derive(Debug, Clone, Serialize)]
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
///
/// `Serialize` (P5): field declaration order IS the wire order (`serde_json`
/// emits declaration order) — `turn_id`, `tool`, `total_files`, `files` —
/// pinned by `diff --json`'s empty-case literal
/// (`{"turn_id":"t_…","total_files":0,"files":{"items":[],"next":null}}`
/// when `tool` is absent). `tool` follows the `TurnRecord::tool` /
/// `MemoryHit::reason` convention: `skip_serializing_if = "Option::is_none"`
/// so a genuinely bare (toolless) turn omits the key entirely rather than
/// emitting `"tool":null`.
#[derive(Debug, Clone, Serialize)]
pub struct DiffResult {
    /// FULL id — `fmt::short_id` truncation is the adapter's.
    pub turn_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The turn's OWN file count, deliberately distinct from
    /// `files.items.len()`: the page may be limited while the header still
    /// reports what the turn recorded.
    pub total_files: usize,
    pub files: Page<FileDiff>,
}

/// One file entry with its content already resolved, so a renderer needs no
/// blob store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
///
/// `Serialize` (P5): internally tagged (`"type"`, `snake_case`) — the same
/// shape `LogRecord` already uses for its turn/epoch discriminant
/// (`record.rs`) — rather than serde's default externally-tagged
/// `{"Withheld":null}` / `{"Text":{...}}`, which is both PascalCase and an
/// extra wrapper layer a consumer would have to unwrap for every arm. No
/// other precedent for a data-carrying multi-variant enum exists in this
/// crate to follow instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
///
/// `Serialize` (P5): plain field-for-field — `path`/`line` are always
/// meaningful (unlike `DiffResult::tool`, `line: None` is a real "file-level
/// query" answer, not an absent fact), so neither carries
/// `skip_serializing_if`.
#[derive(Debug, Clone, Serialize)]
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
///
/// `Serialize` (P5): internally tagged (`"type"`, `snake_case`), the same
/// convention as [`FileDiffState`]. This is what gives AC-3's "uncovered
/// path → `recording_gap`, no attributor" its structural guarantee: the
/// `NoTurnRecordingGap`/`LineRecordingGap`/`LineOriginGap` arms carry no
/// `turn` field at all (the type system, not an adapter, makes a guessed
/// attributor impossible), and their tag names say "recording_gap" — a
/// hand-built extra `"recording_gap": true` boolean was deliberately NOT
/// added on top: that would be exactly the second, adapter-owned
/// interpretation AC-1 forbids on this path.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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

// ---- recall ---------------------------------------------------------------

/// Verify-walk cap re-export (R23(b)): `recall_cmd`'s F3 notice embeds this
/// number. Re-exporting it here lets the adapter read `view::RECALL_VERIFY_CAP`
/// instead of reaching into `memory::` directly, which is what AC15's grep
/// checks for, without widening [`RecallPage`] past its spec'd four fields
/// (the rejected alternative, R23(a)).
pub use crate::memory::RECALL_VERIFY_CAP;

/// Which memories to recall, how many, and (for a future paginating caller —
/// MCP 2.2, not the CLI, which never sets `after`) where to resume.
///
/// `after`'s staleness check is one-sided: it catches a memory that LEFT the
/// candidate set between two calls (retracted, or its pin verified no
/// longer Fresh) and correctly reports `CursorError::Stale` rather than
/// silently resuming past it. It does NOT catch the opposite case — a
/// memory that was Stale (or didn't exist) when the cursor was minted and
/// is Fresh by the time the next page is fetched. Such a memory can ENTER
/// the ranking and sort ahead of the cursor's identity; the next page then
/// starts strictly after that identity and never returns it, with no error
/// and no gap reported. `list`/`diff` don't have this asymmetry because
/// their underlying corpus (the turn log) is append-only; `recall`'s
/// freshness is derived from the live worktree and can change in either
/// direction between two calls.
#[derive(Debug, Clone, Default)]
pub struct RecallQuery {
    pub query: String,
    pub k: usize,
    pub after: Option<Cursor>,
}

impl RecallQuery {
    /// Stable identity of the query a cursor was minted against, mirroring
    /// [`DiffQuery::fingerprint`]. `k` is excluded — it is page size, not
    /// filter identity.
    fn fingerprint(&self) -> String {
        format!("recall:query={}", self.query)
    }
}

/// One memory, reduced to what `recall`/`memories --json` render — byte-
/// identical to today's `EffectiveJson` (decision 3): field order and
/// `reason`'s `skip_serializing_if` are load-bearing, `serde_json` emits
/// declaration order.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryHit {
    pub id: String,
    pub fact: String,
    pub pins: Vec<Pin>,
    pub origin: String,
    pub ts: u64,
    pub retracted: bool,
    pub freshness: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A page of recall hits plus the flags that split `recall_cmd`'s empty-
/// state branches (F3 `capped`, F10 `store_corrupt`, PD3 `store_empty`).
///
/// **`Serialize` since E3, reversing this type's original stance — read the
/// reversal, do not re-derive it.** Until MCP 2.2 this type was deliberately
/// left unserializable, so that decision 3 ("`recall --json` emits only
/// `page.items`, never these flags") was structurally impossible to violate
/// by accident rather than merely conventional. Delta decision 14 requires
/// the opposite on the MCP wire: `agentrec_recall` returns the WHOLE page
/// including `capped`/`store_corrupt`/`store_empty`, because hiding store
/// corruption from an agent consumer would violate the same gap-honesty
/// stance the CLI's human renderer already honors. Both cannot be had; the
/// derive lands and the structural guard is replaced by a **test** one:
/// `recall --json`'s byte contract is pinned by
/// `cli/tests/fixtures/golden/recall_json_{hits,empty_store,no_match,stale_pin}.golden`,
/// and the CLI adapter (`memorycmds::recall_cmd`) still reaches for
/// `.page.items` explicitly. A future edit that serialized the whole page
/// from the CLI reds those four goldens. That is the compensating control —
/// strictly weaker than "does not compile", and named here so it is not
/// mistaken for the old guarantee.
#[derive(Debug, Clone, Serialize)]
pub struct RecallPage {
    pub page: Page<MemoryHit>,
    /// The verify walk stopped at [`RECALL_VERIFY_CAP`] with candidates still
    /// unchecked, **and this page reaches the end of what that walk fetched**
    /// — i.e. "matches may be missing from what you are holding".
    ///
    /// The second half is why this is not simply the underlying walk's flag.
    /// On the cursored path the fetch is taken at `RECALL_VERIFY_CAP` rather
    /// than at `k`, so a caller asking for `k = 2` and receiving 2 items would
    /// otherwise see `capped: true` and render "results may be incomplete"
    /// about a page that is complete. On the uncursored path the two are the
    /// same value: `recall_impl` breaks on `out.len() >= k`, so the fetch
    /// never exceeds `k` and the page always consumes it.
    pub capped: bool,
    pub store_corrupt: bool,
    pub store_empty: bool,
}

/// Why a recall could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecallError {
    /// A paginated (`after`-bearing) call's cursor could not be honored —
    /// unreachable from the CLI today (it never pages recall), reachable
    /// only by a future MCP 2.2 caller. Reuses [`CursorError`] rather than
    /// inventing recall-specific prose, the same choice [`DiffError::Cursor`]
    /// already made for the identical shared-`Cursor` contract.
    Cursor(CursorError),
    /// Unreachable today: every arm of `memory::recall_impl` returns `Ok`
    /// (R21) — corruption, cap, and deadline are surfaced as
    /// [`memory::RecallOutcome`] flags, not errors. Kept so a stricter
    /// reader has somewhere to report instead of a panic.
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
        // F26: the turn set fed to `managed_bytes` must be the UNFILTERED one
        // — every `LogRecord::Turn` in ledger order, no git/superseded
        // narrowing — because that is exactly what both eviction call sites
        // pass to `plan_eviction` (`cmds::eviction_plan` uses
        // `TurnQuery { include_all: true }`; `daemon::run_eviction_pass` uses
        // a raw `load_log`). Narrowing here would silently break the
        // `managed_bytes > budget` <-> "the evictor has candidates"
        // equivalence that makes `over_budget` meaningful.
        let turns: Vec<&TurnRecord> = ledger
            .records
            .iter()
            .filter_map(|r| match r {
                LogRecord::Turn(t) => Some(t),
                LogRecord::Epoch(_) => None,
            })
            .collect();
        let budgeted_bytes = crate::retention::managed_bytes(&store, turns.iter().copied());
        // One walk, three fields (residual handed over from the F13 wave:
        // this was three `gap_counts(&ledger.records)` calls, i.e. three full
        // passes over every record to answer one census).
        let gaps = gap_counts(&ledger.records);
        Ok(RepositoryHealth {
            store_bytes,
            budgeted_bytes,
            budget,
            over_budget: budgeted_bytes > budget,
            turn_count: turns.len(),
            crash_gaps: gaps.crash,
            restart_gaps: gaps.restart,
            trailing_stop_gaps: gaps.trailing_stop,
            unknown_type_lines: ledger.unknown_type_lines,
            unparsed_lines: ledger.unparsed_lines,
        })
    }

    /// List turns oldest-first, the order they were appended.
    ///
    /// Thin wrapper over [`Self::list_of`] on a freshly-read ledger (P4b-4
    /// open question 2, decided: `list` survives as the delegating form so
    /// its existing unit tests keep exercising the same selection logic).
    pub fn list(&self, q: &TurnQuery) -> Result<Page<TurnSummary>, CursorError> {
        self.list_of(&self.ledger(), q)
    }

    /// [`Self::list`] over a ledger the caller already read — the same
    /// `health`/`health_of` pairing, and for the same reason: `status` feeds
    /// its counting surface, its health figures, and its eviction protect-set
    /// from ONE parse of `log.jsonl`. A second parse would open a window in
    /// which a daemon append makes the printed turn count disagree with the
    /// health figures computed off the other read.
    pub fn list_of(
        &self,
        ledger: &Ledger,
        q: &TurnQuery,
    ) -> Result<Page<TurnSummary>, CursorError> {
        let page = select_turns(ledger, q)?;
        Ok(Page {
            items: page
                .items
                .into_iter()
                .map(|t| TurnSummary {
                    id: t.id.clone(),
                    grade: t.grade.clone(),
                    tool: t.tool.clone(),
                    started: t.started.clone(),
                    ended: t.ended.clone(),
                    file_count: t.files.len(),
                    imported: t.imported.unwrap_or(false),
                })
                .collect(),
            next: page.next,
        })
    }

    /// The same selection as [`Self::list`], but carrying whole
    /// [`TurnRecord`]s instead of the lossy [`TurnSummary`] projection.
    ///
    /// Two consumers need the full record and cannot be served by a summary:
    /// the human `log` line renders `prompt_excerpt`/`truncated`/
    /// `files_complete` and folds per-file paths, `log --json` emits the
    /// record verbatim, and `status`'s eviction protect-set needs
    /// `prompt_ref` plus every `files[]` blob hash. Widening `TurnSummary`
    /// to cover them is rejected: "prompt contents are opt-in data, never
    /// included in generic summaries".
    pub fn list_records(&self, q: &TurnQuery) -> Result<Page<TurnRecord>, CursorError> {
        self.list_records_of(&self.ledger(), q)
    }

    /// [`Self::list_records`] over an already-read ledger — the single-parse
    /// twin, same rationale as [`Self::list_of`].
    pub fn list_records_of(
        &self,
        ledger: &Ledger,
        q: &TurnQuery,
    ) -> Result<Page<TurnRecord>, CursorError> {
        let page = select_turns(ledger, q)?;
        Ok(Page {
            items: page.items.into_iter().cloned().collect(),
            next: page.next,
        })
    }

    /// Per-repository analytics (Phase 3.0 T1) — turn census, file churn,
    /// agent-vs-human share, and the normative rework rate with every
    /// exclusion bucket beside it. Definitions live in
    /// [`crate::stats`] and, normatively, in the phase-3 design §3.0.1.
    ///
    /// A pure read of three surfaces: `log.jsonl`, the CAS (blob sizes), and
    /// the current working tree (live content hashes — `excluded_unknown_mtime`
    /// and the human share are uncomputable from the ledger alone). Writes
    /// nothing.
    ///
    /// The wall clock is read here and passed down, so the fold itself is
    /// deterministic and its right-censoring boundary is testable.
    pub fn stats(
        &self,
        opts: &crate::stats::StatsOptions,
    ) -> Result<crate::stats::StatsResult, crate::stats::StatsError> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0);
        crate::stats::compute_stats(&self.ledger(), &self.root, opts, now_ms)
    }
}

/// The one selection + pagination walk behind [`RepositoryView::list_of`] and
/// [`RepositoryView::list_records_of`]. Borrows out of `ledger`; the callers
/// decide whether to project or to clone, so the filter, the cursor
/// resolution, and the staleness rules cannot drift between the summary and
/// the record form.
fn select_turns<'a>(
    ledger: &'a Ledger,
    q: &TurnQuery,
) -> Result<Page<&'a TurnRecord>, CursorError> {
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
    let items: Vec<&TurnRecord> = selected[start.min(selected.len())..end]
        .iter()
        .map(|(_, t)| *t)
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

impl RepositoryView {
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

        // fsguard on a WIRE-SUPPLIED working-tree path. This one needed no
        // hostile precondition at all: default config, read-only tool, and
        // any repo that merely CONTAINS a named pipe hung the MCP server the
        // moment an agent blamed that path.
        let disk_bytes = crate::fsguard::read_regular(&self.root.join(&q.path)).ok();
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

    /// Rank-then-verify recall (INV-M2) over `.agentrec/memory.jsonl`. A seam
    /// swap under a stable contract (decision 3): this delegates to the
    /// already-hardened walk in [`memory::recall_outcome`] rather than
    /// reimplementing BM25 ranking or freshness verification here — F2's
    /// deadline cooperation, F3's cap accounting, and F10's corruption
    /// detection all stay exactly as memory.rs already proved them, once.
    ///
    /// Pagination (`q.after`) is honest, not speculative, about what a
    /// single call can know: the CLI (the only caller today) never sets it,
    /// so the RANKING/VERIFY WORK on that path is EXACTLY today's
    /// `recall_outcome(root, query, k)` call. (`store_empty`'s own extra
    /// `load_effective` parse below is a separate, spec-mandated cost that
    /// applies on this path too when the page is empty — see its own
    /// comment; it is not new relative to today's human `recall_cmd`, only
    /// relative to today's `--json` empty case.) A cursored call resolves
    /// identity against one `k = RECALL_VERIFY_CAP` fetch (the largest
    /// result a single call can ever produce — recall's own hard budget
    /// wall), so `next` is only ever minted when that same fetch already
    /// proves more items exist, never guessed from "the page happened to be
    /// full".
    pub fn recall(&self, q: &RecallQuery) -> Result<RecallPage, RecallError> {
        struct Fetch {
            hits: Vec<memory::EffectiveMemory>,
            capped: bool,
            store_corrupt: bool,
            start: usize,
            /// Whether this fetch went through the cursor-resolution path
            /// (`k = RECALL_VERIFY_CAP`), so `hits` is the full within-cap
            /// view rather than only-as-much-as-`q.k`-demanded — letting
            /// `next` be computed exactly instead of left unknown. NOT a
            /// claim that the walk found every Fresh match in the corpus;
            /// `capped` (recomputed below, page-relative) still answers
            /// that question independently.
            from_cursor: bool,
        }

        let fetch = match &q.after {
            None => {
                let outcome =
                    memory::recall_outcome(&self.root, &q.query, q.k).map_err(RecallError::Io)?;
                Fetch {
                    hits: outcome.hits,
                    capped: outcome.capped,
                    store_corrupt: outcome.store_corrupt,
                    start: 0,
                    from_cursor: false,
                }
            }
            Some(c) => {
                if c.query != q.fingerprint() {
                    return Err(RecallError::Cursor(CursorError::QueryMismatch));
                }
                let outcome = memory::recall_outcome(&self.root, &q.query, RECALL_VERIFY_CAP)
                    .map_err(RecallError::Io)?;
                // Identity, not position (mirrors `list`/`diff`): a memory
                // that dropped out of the candidate set (retracted, or
                // verified no-longer-Fresh) since the cursor was minted must
                // surface as stale rather than silently resume at whatever
                // now sits nearby.
                //
                // One-sided by construction, not a bug this check can close:
                // a memory that was Stale when page 1 was minted and is
                // Fresh now can ENTER the ranking and sort ahead of the
                // cursor's id — if so, page 2 starts past it and it is
                // never returned, silently, no `Stale` reported. `list`/
                // `diff` don't have this because their corpus is
                // append-only; recall's freshness is worktree-mutable. See
                // `RecallQuery::after`'s doc comment.
                let Some(idx) = outcome.hits.iter().position(|m| m.id == c.after_id) else {
                    return Err(RecallError::Cursor(CursorError::Stale));
                };
                // A page of size 0 past a cursor has no honest continuation
                // to report (mirrors `list`/`diff`'s `CursorError::ZeroLimit`
                // — see that variant's doc comment) — unlike `q.after: None`,
                // where `k=0` is a real, already-supported "give me nothing"
                // request with no pagination promise attached to it.
                if q.k == 0 {
                    return Err(RecallError::Cursor(CursorError::ZeroLimit));
                }
                Fetch {
                    hits: outcome.hits,
                    capped: outcome.capped,
                    store_corrupt: outcome.store_corrupt,
                    start: idx + 1,
                    from_cursor: true,
                }
            }
        };

        let start = fetch.start.min(fetch.hits.len());
        let end = (start + q.k).min(fetch.hits.len());
        let items: Vec<MemoryHit> = fetch.hits[start..end]
            .iter()
            .map(|m| MemoryHit {
                id: m.id.clone(),
                fact: m.fact.clone(),
                pins: m.pins.clone(),
                origin: m.origin.clone(),
                ts: m.ts,
                retracted: m.retracted,
                // INV-M2: `recall_outcome` only ever returns Fresh,
                // non-retracted entries, so this is never anything but
                // "fresh" reachable from here — the general 3-state label
                // (and a live `reason`) is `memories`' concern, not this
                // method's.
                freshness: "fresh",
                reason: None,
            })
            .collect();
        // Minted from the PAGE slice, not `fetch.hits` positionally: when
        // the page is empty (`start == end`, reachable via `q.k == 0` on the
        // `after: None` path — deliberately still an `Ok`, not refused, see
        // above), `fetch.hits.get(end - 1)` would return the item at
        // `start - 1` — the cursor's OWN previous item on the cursored path,
        // or an unrelated item on the uncursored one — minting a `next`
        // that never advances. `[start..end].last()` is `None` on an empty
        // slice by construction, the same idiom `list`/`diff` use.
        let next = if fetch.from_cursor && end < fetch.hits.len() {
            fetch.hits[start..end].last().map(|m| Cursor {
                after_id: m.id.clone(),
                after_occurrence: 0,
                query: q.fingerprint(),
            })
        } else {
            None
        };

        // F3, page-relative: a cursored fetch always requests
        // `RECALL_VERIFY_CAP` internally regardless of `q.k` (see above), so
        // `fetch.capped` by itself answers "did the WHOLE within-cap fetch
        // hit the wall", not "is THIS page capped" — a caller paging with a
        // small `q.k` must not see `capped: true` merely because the
        // 128-candidate probe happened to run out, when this page was
        // already fully satisfied by items already in hand. A page is
        // honestly capped only when it consumes every fetched item AND that
        // fetch hit the wall; `end == fetch.hits.len()` always holds on the
        // uncursored path (that fetch's own `k` was `q.k`), so this reduces
        // to `fetch.capped` there unchanged.
        let capped = fetch.capped && end >= fetch.hits.len();

        // PD3: distinguishes "no memory ever recorded" (stderr zero-state)
        // from "nothing fresh matched" (stdout notice) — `recall_cmd`'s
        // existing split. Computed lazily (only when this page is empty) to
        // preserve the current work profile on the human path: a non-empty
        // page trivially implies a non-empty store, and `load_effective`'s
        // tolerant fold (not `recall_outcome`'s corruption-sensitive one) is
        // the same function `recall_cmd` already called for this today. This
        // IS a genuinely new parse on the `--json` empty-result path
        // specifically — today's `--json` branch returns `[]` before ever
        // touching `load_effective`; `RecallPage::store_empty` existing at
        // all is spec-mandated (design decision, not re-litigable here), so
        // that added cost is accepted, not an oversight.
        let store_empty = if items.is_empty() {
            memory::load_effective(&self.root)
                .map_err(RecallError::Io)?
                .is_empty()
        } else {
            false
        };

        Ok(RecallPage {
            page: Page { items, next },
            capped,
            store_corrupt: fetch.store_corrupt,
            store_empty,
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
// Test code reads its own tempdir fixtures; no attacker-supplied FIFO can
// block these, so the fsguard wrappers buy nothing. Scoped to this module
// so production reads in this file stay lint-enforced (clippy.toml).
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::record::{EpochRecord, FileEntry};

    fn epoch(event: &str, ts: &str) -> LogRecord {
        LogRecord::Epoch(EpochRecord {
            v: 1,
            event: event.to_string(),
            ts: ts.to_string(),
            dropped_signals: 0,
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
            link_kind: None,
            attribution: None,
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
            origin: None,
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
        // The distinction is load-bearing for the CRASH-shaped predicates —
        // `has_crash_gap` and `crash_gap_count` must not count a deliberate
        // restart. It is NOT a licence to drop the interval from the total:
        // F13 found `status` doing exactly that, so `gap_counts` sees it.
        // (This comment previously read "`status` counts crashes only"; that
        // stopped being true when F13 was fixed.)
        assert_eq!(crash_gap_count(&recs), 0);
        assert!(!has_crash_gap(&recs));
        assert_eq!(gap_counts(&recs).restart, 1);
        assert_eq!(gap_counts(&recs).total(), 1);
    }

    /// F13: every kind [`recording_gaps`] can tag is counted, and the total
    /// is their sum — the property `status` violated by reporting
    /// `crash_gaps` alone. One ledger carrying all three shapes at once:
    /// start, start (crash), stop, start (restart), stop (trailing).
    ///
    /// Neuter (both directions): make `gap_counts` filter to
    /// `GapKind::Crash` — the pre-F13 behavior — and `restart`/
    /// `trailing_stop`/`total` all go RED. Make it count every gap as
    /// `crash` and the per-kind asserts go RED. Vacuity guard: the three
    /// kinds carry DISTINCT counts (2/1/1), so a renderer that reads one
    /// field where it means another cannot pass by coincidence.
    #[test]
    fn gap_counts_counts_every_kind_not_just_crash() {
        let recs = vec![
            epoch("start", "2026-01-01T00:00:00Z"),
            epoch("start", "2026-01-01T01:00:00Z"), // crash 1
            epoch("start", "2026-01-01T02:00:00Z"), // crash 2
            epoch("stop", "2026-01-01T03:00:00Z"),
            epoch("start", "2026-01-01T04:00:00Z"), // restart 1
            epoch("stop", "2026-01-01T05:00:00Z"),  // trailing 1
        ];
        let counts = gap_counts(&recs);
        assert_eq!(counts.crash, 2, "{counts:?}");
        assert_eq!(counts.restart, 1, "{counts:?}");
        assert_eq!(counts.trailing_stop, 1, "{counts:?}");
        assert_eq!(counts.total(), 4, "{counts:?}");
        // The pre-F13 figure, retained as the contrast: crash-only reporting
        // hid 2 of these 4 uncovered intervals.
        assert_eq!(crash_gap_count(&recs), 2);
        assert_eq!(counts.total(), recording_gaps(&recs).len());
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
        assert_eq!(h.budgeted_bytes, 0);
    }

    /// F26 at the health level: `over_budget` must key on the bytes eviction
    /// ranges over, not on disk bytes. The fixture is the failure state the
    /// redteam measured — a store dominated by blobs no turn references —
    /// with a budget deliberately between the two figures, so the OLD
    /// predicate (`store_bytes > budget`) and the new one disagree and only
    /// one of them can pass.
    ///
    /// Neuter: restore `over_budget: store_bytes > budget` and the third
    /// assertion reds.
    #[test]
    fn over_budget_keys_on_evictable_bytes_not_disk_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_log(root, &[]);
        let store = crate::store::BlobStore::new(root.join(".agentrec").join("objects"));

        let referenced = store.put(&[0xAAu8; 20]).unwrap();
        // No turn cites this: on disk, ineligible as an eviction victim.
        store.put(&[0xBBu8; 180]).unwrap();

        // Written as a wire line rather than a constructed `FileEntry` so this
        // fixture does not have to be edited every time PROTOCOL gains an
        // additive optional field — `#[serde(default)]` on the optionals is
        // what makes the short form legal, and exercising it here is a small
        // bonus check of that tolerance.
        let line = format!(
            r#"{{"type":"turn","v":1,"id":"t_F26HEALTH","grade":"rich","started":"2026-01-01T00:00:00Z","ended":"2026-01-01T00:00:00Z","root":"/repo","files":[{{"path":"x.bin","after":"{referenced}","op":"create"}}]}}"#
        );
        write_log(root, &[&line]);

        let h = RepositoryView::open(root).unwrap().health(100).unwrap();
        assert_eq!(h.store_bytes, 200, "disk bytes still include the orphan");
        assert_eq!(h.budgeted_bytes, 20, "only the referenced snapshot counts");
        assert!(
            !h.over_budget,
            "200 B on disk over a 100 B budget must NOT read as over-budget \
             when only 20 B of it is evictable — that is the permanent \
             'over budget, evicting nothing' state F26 describes"
        );

        // And the predicate still fires when the evictable set really is over.
        let h2 = RepositoryView::open(root).unwrap().health(19).unwrap();
        assert!(h2.over_budget, "20 B evictable against a 19 B budget");
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

    // ---- list_of / list_records (P4b-4) ----------------------------------

    /// The summary and the record form must be the SAME selection — one
    /// `select_turns` walk behind both — or `status`'s counting page and its
    /// eviction protect-set could disagree about which turns exist. Fixture
    /// carries the two classes the filter decides on (a git turn and a
    /// superseded turn) plus a plain one, checked under both `include_all`
    /// settings so a filter that only differed in one direction still reds.
    #[test]
    fn list_of_and_list_records_of_select_identically() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let mut git = turn("t_LISTGIT", "2026-01-01T00:00:01Z", vec![]);
        git.tool = Some("git".to_string());
        let dropped = turn("t_LISTDROPPED", "2026-01-01T00:00:02Z", vec![]);
        let mut newest = turn("t_LISTNEWEST", "2026-01-01T00:00:03Z", vec![]);
        newest.merges = vec![dropped.id.clone()];
        write_log(
            root,
            &[
                &turn_line("t_LISTPLAIN"),
                &serde_json::to_string(&LogRecord::Turn(git)).unwrap(),
                &serde_json::to_string(&LogRecord::Turn(dropped)).unwrap(),
                &serde_json::to_string(&LogRecord::Turn(newest)).unwrap(),
            ],
        );

        let view = RepositoryView::open(root).unwrap();
        let ledger = view.ledger();
        for include_all in [false, true] {
            let q = TurnQuery {
                include_all,
                limit: None,
                after: None,
            };
            let summaries = view.list_of(&ledger, &q).unwrap();
            let records = view.list_records_of(&ledger, &q).unwrap();
            let ids: Vec<&str> = summaries.items.iter().map(|t| t.id.as_str()).collect();
            let rec_ids: Vec<&str> = records.items.iter().map(|t| t.id.as_str()).collect();
            assert_eq!(ids, rec_ids, "include_all={include_all}");
            assert_eq!(summaries.next, records.next, "include_all={include_all}");
        }
        // The filter itself, so the equality above cannot be satisfied
        // vacuously by two identically-broken selections.
        assert_eq!(
            view.list_records_of(&ledger, &TurnQuery::default())
                .unwrap()
                .items
                .len(),
            2,
            "git and superseded turns are hidden by default"
        );
        assert_eq!(
            view.list_records_of(
                &ledger,
                &TurnQuery {
                    include_all: true,
                    limit: None,
                    after: None,
                }
            )
            .unwrap()
            .items
            .len(),
            4
        );
    }

    /// `list_records` carries fields `TurnSummary` drops — the exact reason
    /// the record form exists (`log`'s human line, `log --json`, and the
    /// eviction protect-set's `prompt_ref` + `files[]` hashes).
    #[test]
    fn list_records_carries_the_fields_the_summary_projection_drops() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut t = turn(
            "t_LISTRECFIELDS",
            "2026-01-01T00:00:01Z",
            vec![FileEntry {
                path: "a.rs".to_string(),
                before: None,
                after: Some("sha256:aa".to_string()),
                op: "create".to_string(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
                after_synthesized: None,
                link_kind: None,
                attribution: None,
            }],
        );
        t.prompt_ref = Some("sha256:bb".to_string());
        t.prompt_excerpt = Some("do the thing".to_string());
        t.truncated = true;
        t.files_complete = Some(false);
        write_log(
            root,
            &[&serde_json::to_string(&LogRecord::Turn(t)).unwrap()],
        );

        let view = RepositoryView::open(root).unwrap();
        let got = view.list_records(&TurnQuery::default()).unwrap();
        let rec = &got.items[0];
        assert_eq!(rec.prompt_ref.as_deref(), Some("sha256:bb"));
        assert_eq!(rec.prompt_excerpt.as_deref(), Some("do the thing"));
        assert!(rec.truncated);
        assert_eq!(rec.files_complete, Some(false));
        assert_eq!(rec.files[0].after.as_deref(), Some("sha256:aa"));
    }

    /// Cursor semantics are the shared walk's, so they hold for the record
    /// form too: a page boundary continues, and a cursor whose record the
    /// ledger no longer carries is `Stale`, never a silently holed page.
    #[test]
    fn list_records_paginates_and_reports_a_stale_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_log(
            root,
            &[&turn_line("t_RA"), &turn_line("t_RB"), &turn_line("t_RC")],
        );
        let view = RepositoryView::open(root).unwrap();

        let first = view
            .list_records(&TurnQuery {
                include_all: false,
                limit: Some(2),
                after: None,
            })
            .unwrap();
        assert_eq!(
            first
                .items
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["t_RA", "t_RB"]
        );
        let cursor = first.next.clone().expect("a third turn remains");

        let second = view
            .list_records(&TurnQuery {
                include_all: false,
                limit: Some(2),
                after: Some(cursor.clone()),
            })
            .unwrap();
        assert_eq!(
            second
                .items
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["t_RC"]
        );
        assert!(second.next.is_none());

        // A rewrite that drops the cursor's record must surface as Stale.
        write_log(root, &[&turn_line("t_RA"), &turn_line("t_RC")]);
        assert_eq!(
            view.list_records(&TurnQuery {
                include_all: false,
                limit: Some(2),
                after: Some(cursor),
            })
            .err(),
            Some(CursorError::Stale)
        );
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

    // ---- recall (P4b-3, AC2-AC4, AC10-AC12) ------------------------------

    /// Appends one hand-built `assert` line to `.agentrec/memory.jsonl` —
    /// never through `memory::append_memory` (which scrubs/fsyncs
    /// unnecessarily for a fixture), mirroring `cli/tests/golden.rs`'s
    /// `seed_memory` helper.
    fn seed_memory(
        root: &std::path::Path,
        id: &str,
        fact: &str,
        pin_path: &str,
        pin_hash: &str,
        ts: u64,
        retracted: bool,
    ) {
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        let path = memory::memory_path(root);
        let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
        let assert_rec = memory::MemoryRecord {
            v: 1,
            kind: "memory".to_string(),
            id: id.to_string(),
            op: memory::MemoryOp::Assert,
            fact: fact.to_string(),
            pins: vec![Pin {
                path: pin_path.to_string(),
                hash: pin_hash.to_string(),
            }],
            source_turns: vec![],
            origin: "human".to_string(),
            ts,
            reason: None,
        };
        existing.push_str(&serde_json::to_string(&assert_rec).unwrap());
        existing.push('\n');
        if retracted {
            let retract_rec = memory::MemoryRecord {
                op: memory::MemoryOp::Retract,
                ts: ts + 1,
                ..assert_rec
            };
            existing.push_str(&serde_json::to_string(&retract_rec).unwrap());
            existing.push('\n');
        }
        std::fs::write(&path, existing).unwrap();
    }

    /// Writes `content` to `root/rel` and returns its hash, so a seeded
    /// memory's pin verifies Fresh.
    fn write_pin(root: &std::path::Path, rel: &str, content: &[u8]) -> String {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        crate::store::hash_bytes(content)
    }

    /// Same two-doc-corpus shape `cli/tests/golden.rs::build_recall_fixture`
    /// uses, and for the same reason (see that fixture's doc comment): a
    /// single-document corpus can't clear `SCORE_FLOOR`, so a genuine BM25
    /// match needs a second, non-matching document to keep `idf` above the
    /// floor. `throttle` occurring three times (twice in the fact, once in
    /// the pin path) is load-bearing, not a wording choice.
    fn seed_recall_corpus(root: &std::path::Path) {
        let match_hash = write_pin(root, "src/throttle.rs", b"fn throttle() {}\n");
        seed_memory(
            root,
            "MATCH0000000000000000MEM1A",
            "throttle limiter guards the API from bursty traffic via throttle checks",
            "src/throttle.rs",
            &match_hash,
            1_000,
            false,
        );
        let other_hash = write_pin(root, "src/changelog.rs", b"fn changelog() {}\n");
        seed_memory(
            root,
            "THER0000000000000000MEM2AB",
            "the release changelog script lives under scripts",
            "src/changelog.rs",
            &other_hash,
            2_000,
            false,
        );
    }

    fn recall_of(root: &std::path::Path, query: &str, k: usize) -> RecallPage {
        RepositoryView::open(root)
            .unwrap()
            .recall(&RecallQuery {
                query: query.to_string(),
                k,
                after: None,
            })
            .unwrap()
    }

    #[test]
    fn recall_hit_carries_every_effective_json_field() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        seed_recall_corpus(root);
        let page = recall_of(root, "throttle", 5);
        assert_eq!(page.page.items.len(), 1, "{:?}", page.page.items);
        let hit = &page.page.items[0];
        assert_eq!(hit.id, "MATCH0000000000000000MEM1A");
        assert_eq!(
            hit.fact,
            "throttle limiter guards the API from bursty traffic via throttle checks"
        );
        assert_eq!(hit.pins.len(), 1);
        assert_eq!(hit.pins[0].path, "src/throttle.rs");
        assert_eq!(hit.origin, "human");
        assert_eq!(hit.ts, 1_000);
        assert!(!hit.retracted);
        assert_eq!(hit.freshness, "fresh");
        assert_eq!(hit.reason, None);
        assert!(!page.capped);
        assert!(!page.store_corrupt);
        assert!(!page.store_empty);

        // Field-level struct assertions above can't catch a reordered field
        // or a dropped `skip_serializing_if` — the two properties this
        // test's name and AC2 are actually about (order IS separately pinned
        // by `recall_json_hits.golden`, but that lives at the CLI level; the
        // load-bearing property should also be tested where the type
        // itself lives). Serialize and compare bytes directly.
        let match_hash = crate::store::hash_bytes(b"fn throttle() {}\n");
        let expected = format!(
            r#"{{"id":"MATCH0000000000000000MEM1A","fact":"throttle limiter guards the API from bursty traffic via throttle checks","pins":[{{"path":"src/throttle.rs","hash":"{match_hash}"}}],"origin":"human","ts":1000,"retracted":false,"freshness":"fresh"}}"#
        );
        assert_eq!(serde_json::to_string(hit).unwrap(), expected);
    }

    /// Two-sided pin on the verify walk (mirrors
    /// `cli/tests/golden.rs::golden_recall_json_stale_pin`'s rationale at the
    /// view level): drifting the pin AFTER the hash is recorded must exclude
    /// the memory, proving `view::recall` doesn't silently skip freshness
    /// verification.
    #[test]
    fn recall_excludes_a_memory_whose_pin_has_drifted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        seed_recall_corpus(root);
        write_pin(root, "src/throttle.rs", b"fn throttle() { /* edited */ }\n");
        let page = recall_of(root, "throttle", 5);
        assert!(
            page.page.items.is_empty(),
            "a drifted pin must never verify Fresh: {:?}",
            page.page.items
        );
        assert!(!page.store_empty, "the store itself is non-empty");
    }

    #[test]
    fn recall_store_empty_true_when_no_memory_ever_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        let page = recall_of(root, "anything", 5);
        assert!(page.page.items.is_empty());
        assert!(page.store_empty);
    }

    #[test]
    fn recall_store_empty_false_when_nonempty_store_has_no_match() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        seed_recall_corpus(root);
        let page = recall_of(root, "zzzznomatch", 5);
        assert!(page.page.items.is_empty());
        assert!(
            !page.store_empty,
            "a non-empty store with no fresh match must not read as store_empty"
        );
    }

    /// Blind spot a permissive `store_empty` implementation could miss: a
    /// store holding ONLY a retracted memory is non-empty by
    /// `load_effective`'s fold (retracted entries are folded in, not
    /// dropped), so `store_empty` must stay `false` even though `recall`
    /// itself never returns a retracted hit (INV-M2).
    #[test]
    fn recall_store_empty_false_for_a_retracted_only_store() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let hash = write_pin(root, "src/only.rs", b"fn only() {}\n");
        seed_memory(
            root,
            "RETD0000000000000000MEM3AC",
            "a fact that gets retracted",
            "src/only.rs",
            &hash,
            1_000,
            true,
        );
        let page = recall_of(root, "zzzznomatch", 5);
        assert!(page.page.items.is_empty());
        assert!(
            !page.store_empty,
            "a retracted-only store is non-empty, not store_empty"
        );
    }

    /// `k=0` is a real, reachable CLI input (`agentrec recall <q> -k 0`) and
    /// today's `memory::recall_outcome(root, query, 0)` answers it with an
    /// empty, non-error result — never `CursorError::ZeroLimit`, which
    /// exists for `list`/`diff`'s "no honest continuation to report" reason.
    /// `recall`'s `k` is a results budget, not a pagination limit; nothing
    /// about `k=0` is dishonest to answer directly, so this must NOT error.
    #[test]
    fn recall_k_zero_is_an_empty_ok_result_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        seed_recall_corpus(root);
        let page = recall_of(root, "throttle", 0);
        assert!(page.page.items.is_empty());
        assert!(!page.capped);
    }

    /// The verify-cap re-export AC15 needs: `view::RECALL_VERIFY_CAP` must
    /// be the exact same constant `memory::recall_outcome` bounds its walk
    /// by, not a second, driftable copy.
    ///
    /// This cannot fail today — `pub use crate::memory::RECALL_VERIFY_CAP`
    /// is the same item under two names, so the two sides of `assert_eq!`
    /// are identical by construction. Its only job is becoming a real
    /// tripwire if a future edit replaces the re-export with a copied
    /// `const RECALL_VERIFY_CAP: usize = 128;` — the exact drift this test
    /// exists to catch, even though it is presently unfalsifiable.
    #[test]
    fn recall_verify_cap_reexport_matches_memory() {
        assert_eq!(RECALL_VERIFY_CAP, memory::RECALL_VERIFY_CAP);
    }

    /// F3: `capped` mirrors `memory::RecallOutcome::capped` through the seam
    /// unchanged. Reuses the same stale-heavy-corpus shape
    /// `cli/tests/integration.rs::seed_capped_stale_heavy_corpus` uses: an
    /// orphaned block strictly larger than `RECALL_VERIFY_CAP`, sharing one
    /// high-scoring fact, so the walk exhausts the cap before finding any
    /// Fresh match.
    #[test]
    fn recall_capped_is_set_when_the_verify_walk_hits_the_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        let path = memory::memory_path(root);
        let mut lines = String::new();
        // Mirrors `cli/tests/integration.rs::seed_capped_stale_heavy_corpus`
        // exactly (fresh_count=0): an orphaned block strictly larger than
        // `RECALL_VERIFY_CAP`, sharing one fact, PLUS a filler block of a
        // different fact — the filler is load-bearing, not padding: without
        // it `df(kraken)` == corpus size and `idf` collapses toward zero,
        // dropping every candidate below `SCORE_FLOOR` before the verify
        // walk (and hence `capped`) is ever reached — the same
        // vacuous-for-the-wrong-reason trap `build_stale_recall_fixture`'s
        // doc comment (golden.rs) warns about.
        let orphaned_count = RECALL_VERIFY_CAP as u64 + 12;
        for i in 0..orphaned_count {
            let rec = serde_json::json!({
                "v": 1,
                "type": "memory",
                "id": format!("orph{i:0>10}"),
                "op": "assert",
                "fact": "kraken telemetry batching",
                "pins": [{"path": format!("missing{i}.rs"), "hash": format!("sha256:{i:064}")}],
                "source_turns": [],
                "origin": "agent",
                "ts": 1_000 + i,
            });
            lines.push_str(&rec.to_string());
            lines.push('\n');
        }
        for i in 0..100u64 {
            let rec = serde_json::json!({
                "v": 1,
                "type": "memory",
                "id": format!("filler{i:0>6}"),
                "op": "assert",
                "fact": "unrelated housekeeping chore",
                "pins": [{"path": format!("filler_missing{i}.rs"), "hash": format!("sha256:{i:064}")}],
                "source_turns": [],
                "origin": "agent",
                "ts": 3_000 + i,
            });
            lines.push_str(&rec.to_string());
            lines.push('\n');
        }
        std::fs::write(&path, lines).unwrap();

        let page = recall_of(root, "kraken telemetry batching", 5);
        assert!(page.page.items.is_empty(), "{:?}", page.page.items);
        assert!(page.capped, "the walk must report it hit the verify cap");
        assert!(!page.store_empty);
    }

    /// AC12: `view::recall` performs zero writes. Checked at the
    /// filesystem level (byte length + a full directory listing of
    /// `.agentrec`, not a single file's mtime, which is exactly the vacuity
    /// trap the task file warns about — mtime equality holds trivially if
    /// the file in question never existed either before or after).
    #[test]
    fn recall_performs_zero_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        seed_recall_corpus(root);

        /// Recursive (unlike a bare `read_dir`, which would miss everything
        /// under `.agentrec/objects/`) name + length + mtime snapshot — AC12
        /// names `memory.jsonl`'s length AND `memory-stats.jsonl`'s mtime
        /// verbatim; a length-only, non-recursive check would pass on an
        /// equal-length in-place rewrite, or on a write nested under
        /// `objects/`, neither of which is "zero writes".
        fn snapshot(dir: &std::path::Path) -> Vec<(String, u64, std::time::SystemTime)> {
            fn walk(
                dir: &std::path::Path,
                root: &std::path::Path,
                out: &mut Vec<(String, u64, std::time::SystemTime)>,
            ) {
                for entry in std::fs::read_dir(dir).unwrap() {
                    let entry = entry.unwrap();
                    let path = entry.path();
                    let meta = entry.metadata().unwrap();
                    if meta.is_dir() {
                        walk(&path, root, out);
                    } else {
                        let rel = path
                            .strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .to_string();
                        out.push((rel, meta.len(), meta.modified().unwrap()));
                    }
                }
            }
            let mut out = Vec::new();
            walk(dir, dir, &mut out);
            out.sort();
            out
        }

        let before = snapshot(&root.join(".agentrec"));
        let page = recall_of(root, "throttle", 5);
        assert!(!page.page.items.is_empty(), "sanity: fixture must match");
        let after = snapshot(&root.join(".agentrec"));
        assert_eq!(
            before, after,
            "recall must not create, grow, shrink, or touch the mtime of any file under .agentrec (recursively, including objects/)"
        );
    }

    #[test]
    fn recall_cursor_pages_by_identity_with_no_repeats() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Empty query -> the ts-descending fallback (`memory::recall_impl`),
        // not BM25 — deterministic order with no `SCORE_FLOOR` concern. FOUR
        // memories (not three) so a `k=1` cursored page still has real
        // remainder behind it — the shape that exercises the production
        // `next`-minting branch at all (a page that exactly exhausts the
        // fetch, as three memories at `k=2` did in an earlier revision of
        // this test, always takes the `end == fetch.hits.len()` -> `None`
        // arm and never runs the `Some` arm it's supposed to prove).
        for (i, id) in ["AAAA", "BBBB", "CCCC", "DDDD"].iter().enumerate() {
            let rel = format!("src/f{i}.rs");
            let hash = write_pin(root, &rel, format!("content {i}").as_bytes());
            seed_memory(
                root,
                &format!("{id}0000000000000000MEM{i}A"),
                &format!("fact number {i}"),
                &rel,
                &hash,
                1_000 + i as u64,
                false,
            );
        }

        let view = RepositoryView::open(root).unwrap();
        let q1 = RecallQuery {
            query: String::new(),
            k: 1,
            after: None,
        };
        let page1 = view.recall(&q1).unwrap();
        assert_eq!(page1.page.items.len(), 1);
        assert!(
            page1.page.next.is_none(),
            "the first (no-cursor) page never speculatively mints a next cursor"
        );

        // The full ts-descending order, for asserting identity below —
        // reading the answer independently, never used as a substitute for
        // production cursor minting (see page2/page3 below, which chain
        // through `view.recall`'s OWN `page.next`, never a hand-built one).
        let full = view
            .recall(&RecallQuery {
                query: String::new(),
                k: 4,
                after: None,
            })
            .unwrap();
        assert_eq!(full.page.items.len(), 4);

        // The ONE hand-built cursor in this test: entering the cursored
        // regime at all requires a starting point, and page 1 above
        // deliberately never mints one (by design — see its own assertion).
        // Every cursor from here on is production-minted.
        let cursor1 = Cursor {
            after_id: page1.page.items[0].id.clone(),
            after_occurrence: 0,
            query: q1.fingerprint(),
        };

        // Page 2: k=1, one cursored page in from the start, with 2 more
        // memories still behind it (4 total - 1 already consumed - 1 this
        // page = 2 remain) — this is what actually runs
        // `view.rs`'s `Some` arm of the `next`-minting `if`, not merely
        // compiles it.
        let page2 = view
            .recall(&RecallQuery {
                query: String::new(),
                k: 1,
                after: Some(cursor1),
            })
            .unwrap();
        assert_eq!(
            page2.page.items.len(),
            1,
            "sanity: page 2 must return exactly one item"
        );
        assert_eq!(page2.page.items[0].id, full.page.items[1].id);
        let cursor2 = page2.page.next.clone().unwrap_or_else(|| {
            panic!(
                "page 2 must mint a next cursor — 2 more memories remain behind it: {:?}",
                page2.page.items
            )
        });
        assert_eq!(
            cursor2.after_id, page2.page.items[0].id,
            "a minted cursor must name the LAST item this page actually returned"
        );

        // Page 3, fed with page 2's OWN production-minted cursor: must
        // continue exactly where page 2 left off, no repeat, and — having
        // now consumed everything — must NOT mint a further cursor.
        let page3 = view
            .recall(&RecallQuery {
                query: String::new(),
                k: 2,
                after: Some(cursor2),
            })
            .unwrap();
        let page3_ids: Vec<&str> = page3.page.items.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            page3_ids,
            vec![
                full.page.items[2].id.as_str(),
                full.page.items[3].id.as_str()
            ],
            "page 3 must continue exactly where page 2's production cursor left off, no repeat"
        );
        assert!(
            page3.page.next.is_none(),
            "a fetch that reaches the end must not mint a further next cursor"
        );
    }

    /// MAJOR 1 regression pin: `after: Some(cursor)` with `q.k == 0` must be
    /// refused, not answered with a silent, self-referential `next` cursor
    /// (the bug: `fetch.hits.get(end - 1)` on an empty page resolved to the
    /// CURSOR'S OWN item, so a paginating caller asking for zero more items
    /// got back a `next` identical to what it already had — an infinite
    /// loop that never advances). Mirrors `CursorError::ZeroLimit`'s
    /// existing role for `list`/`diff`. `after: None` with `k == 0` stays
    /// non-erroring (see `recall_k_zero_is_an_empty_ok_result_not_an_error`)
    /// — the two are deliberately different because only one of them
    /// carries a pagination promise to break.
    #[test]
    fn recall_cursored_k_zero_is_refused_not_a_silent_self_referential_next() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        seed_recall_corpus(root);
        let view = RepositoryView::open(root).unwrap();
        let q1 = RecallQuery {
            query: "throttle".to_string(),
            k: 1,
            after: None,
        };
        let page1 = view.recall(&q1).unwrap();
        assert_eq!(page1.page.items.len(), 1);
        let cursor = Cursor {
            after_id: page1.page.items[0].id.clone(),
            after_occurrence: 0,
            query: q1.fingerprint(),
        };
        let err = view
            .recall(&RecallQuery {
                query: "throttle".to_string(),
                k: 0,
                after: Some(cursor),
            })
            .unwrap_err();
        assert_eq!(err, RecallError::Cursor(CursorError::ZeroLimit));
    }

    #[test]
    fn recall_cursor_from_a_different_query_is_a_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        seed_recall_corpus(root);
        let foreign = Cursor {
            after_id: "MATCH0000000000000000MEM1A".to_string(),
            after_occurrence: 0,
            query: "recall:query=some other query".to_string(),
        };
        let err = RepositoryView::open(root)
            .unwrap()
            .recall(&RecallQuery {
                query: "throttle".to_string(),
                k: 1,
                after: Some(foreign),
            })
            .unwrap_err();
        assert_eq!(err, RecallError::Cursor(CursorError::QueryMismatch));
    }

    /// A memory a cursor named can drop out of the candidate set entirely
    /// (here: retracted between the two calls) — this must surface as
    /// `Stale`, never a silent resume at whatever now sits nearby.
    #[test]
    fn recall_cursor_naming_a_now_retracted_memory_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        seed_recall_corpus(root);
        let cursor = Cursor {
            after_id: "MATCH0000000000000000MEM1A".to_string(),
            after_occurrence: 0,
            query: "recall:query=throttle".to_string(),
        };

        // Retract the only match by appending a retract record.
        let path = memory::memory_path(root);
        let existing = std::fs::read_to_string(&path).unwrap();
        let retract_rec = memory::MemoryRecord {
            v: 1,
            kind: "memory".to_string(),
            id: "MATCH0000000000000000MEM1A".to_string(),
            op: memory::MemoryOp::Retract,
            fact: "throttle limiter guards the API from bursty traffic via throttle checks"
                .to_string(),
            pins: vec![],
            source_turns: vec![],
            origin: "human".to_string(),
            ts: 9_999,
            reason: None,
        };
        std::fs::write(
            &path,
            existing + &serde_json::to_string(&retract_rec).unwrap() + "\n",
        )
        .unwrap();

        let err = RepositoryView::open(root)
            .unwrap()
            .recall(&RecallQuery {
                query: "throttle".to_string(),
                k: 1,
                after: Some(cursor),
            })
            .unwrap_err();
        assert_eq!(err, RecallError::Cursor(CursorError::Stale));
    }
}
