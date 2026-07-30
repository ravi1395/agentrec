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
        assert!(matches!(resolve_turn(&[], "t_a"), Err(LookupError::NoTurns)));
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
}
