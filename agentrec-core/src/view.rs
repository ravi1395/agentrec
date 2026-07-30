//! Read-side interpretation, split out of the renderer-coupled CLI.
//!
//! Everything here answers "what does the ledger mean" and nothing here
//! renders. Errors are typed so the human CLI keeps ownership of its prose
//! (and of `fmt::short_id`) while `--json` and, later, MCP read the same
//! interpretation without going through a string.

use crate::record::{LogRecord, TurnRecord};

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
/// A cursor is bound to BOTH the query it was produced by and the ledger
/// state it observed. `observed` is the id of the last item returned, not a
/// byte offset or an index: `purge --log-duplicates` is a sanctioned rewrite
/// that can leave `log.jsonl` the same length or longer while shifting every
/// position, so a length- or offset-keyed cursor would silently resume at
/// the wrong record instead of reporting staleness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub after_id: String,
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
                let Some(ledger_idx) = all.iter().position(|t| t.id == c.after_id) else {
                    return Err(CursorError::Stale);
                };
                selected
                    .iter()
                    .position(|(i, _)| *i > ledger_idx)
                    .unwrap_or(selected.len())
            }
        };

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
            items.last().map(|t| Cursor {
                after_id: t.id.clone(),
                query: q.fingerprint(),
            })
        } else {
            None
        };
        Ok(Page { items, next })
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
}
