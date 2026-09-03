//! Per-repository analytics as a pure read over the ledger, the CAS, and the
//! current working tree (Phase 3.0 T1).
//!
//! The normative definitions live in
//! `docs/superpowers/specs/2026-08-07-phase-3-design.md` §3.0.1 (as amended by
//! the T0 spike: the op enum is `create|modify|delete`, gaps are TOLERATED
//! rather than excluding, and the rework rate is labeled an approximate lower
//! bound). Where this module and that section disagree, the section wins and
//! this is a bug.
//!
//! Two things here are deliberately NOT log-only:
//!
//!  - `ReworkRate::excluded_unknown_mtime` — an event whose file's current
//!    on-disk content no longer matches the recorded `after`, with no later
//!    turn and no recording gap to date the change, has an unknowable
//!    modification time and cannot be judged.
//!  - `ChangeShare::human` — the `human-edited-since` display predicate is a
//!    comparison against live bytes.
//!
//! Both read the working tree through [`crate::undo_coordinator::read_current_hash`],
//! the existing guarded hashing seam — hashing is never re-implemented here.

use crate::record::{FileEntry, LogRecord, TurnRecord};
use crate::store::BlobStore;
use crate::undo_coordinator::read_current_hash;
use crate::view::{recording_gaps, Gap, GapKind, Ledger};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

const DAY_MS: u64 = 86_400_000;

/// Default analysis window (founder decision 2: `stats` defaults to 30 days).
pub const DEFAULT_SINCE_DAYS: u64 = 30;
/// Default rework window N (spec §3.0.1; CLI `--rework-window`).
pub const DEFAULT_REWORK_WINDOW_DAYS: u32 = 7;

/// The only value [`ReworkRate::rate_bound`] carries today.
///
/// String-typed rather than boolean on purpose: if a future round measures the
/// two disclosed bias channels and finds a different direction, that is an
/// additive VALUE change, not a wire-key break.
pub const RATE_BOUND_APPROX_LOWER: &str = "approx_lower";

/// Query knobs. Not `Serialize` — this is an input, and the produced types are
/// what the `--json` contract pins.
#[derive(Debug, Clone)]
pub struct StatsOptions {
    /// How far back the fold looks. `None` = all recorded history
    /// (`--since all`).
    pub since: Option<Duration>,
    /// N in "rework within N days of the write event".
    pub rework_window_days: u32,
}

impl Default for StatsOptions {
    fn default() -> Self {
        Self {
            since: Some(Duration::from_secs(DEFAULT_SINCE_DAYS * 86_400)),
            rework_window_days: DEFAULT_REWORK_WINDOW_DAYS,
        }
    }
}

/// What was folded, so no figure is ever rendered without its window.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StatsWindow {
    /// Inclusive lower bound on turn `ended`, RFC 3339; `None` for all history.
    pub since: Option<String>,
    /// Query time, RFC 3339 — the upper bound and the clock the
    /// right-censoring cutoff is measured from.
    pub until: String,
    pub rework_window_days: u32,
}

/// Turn census over the window.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct TurnCounts {
    pub total: u64,
    pub rich: u64,
    pub bare: u64,
    /// Turns carrying `imported: true`. Overlaps `rich`/`bare` by
    /// construction — an imported turn also has a grade.
    pub imported: u64,
    /// Keyed by the record's `tool`; turns with no `tool` are absent from the
    /// map rather than bucketed under an invented name.
    pub by_tool: BTreeMap<String, u64>,
    /// Same reading as `by_tool`, for `model`.
    pub by_model: BTreeMap<String, u64>,
}

/// One file's churn, with every undercount channel counted beside it (spec
/// honesty rule: no figure without its exclusions).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileChurn {
    pub path: String,
    /// Sum over entries of `|size(after) - size(before)|`. An entry whose
    /// recorded hash is not in the store contributes 0 — see `dangling_refs`.
    pub churn_bytes: u64,
    /// ENTRIES naming at least one blob absent from the CAS
    /// (TTL/eviction-legalized, PROTOCOL §6) — one per entry, not one per
    /// missing hash, since an entry missing either side is equally
    /// unmeasurable. Each contributes 0 bytes, so churn here is a lower bound.
    pub dangling_refs: u64,
    /// Entries over the snapshot cap: 0 bytes, counted.
    pub skipped_entries: u64,
    /// Secret-pattern entries never snapshotted: 0 bytes, counted.
    pub withheld_entries: u64,
}

/// Agent-vs-human share of change.
///
/// **Units, stated because they are not all the same** (disclosed rather than
/// hidden): `agent` and `imported` count (turn, file) entries recorded in the
/// ledger — ALL entries, deletes included, because a deletion is a change
/// (the same reading rework clause (b) takes); `unattributable` counts BOTH
/// bare-turn entries AND files whose divergence falls after a recording gap
/// (again all entries, not only writes); `human` counts FILES
/// whose current on-disk content diverges from the last `after` any turn
/// recorded for them — the `human-edited-since` display predicate, which is
/// the only evidence of an unrecorded edit that exists. The four are
/// therefore independent counts, not parts of a whole; nothing here computes
/// a percentage, and a renderer must not present them as summing to 100%.
///
/// `tool: "git"` turns are in NO bucket: a checkout is not agent authorship
/// (spec §3.0.1) and it is not a human edit either, and `unattributable` is
/// spec-enumerated as gaps + bare turns — widening it to git would change a
/// defined bucket.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct ChangeShare {
    /// All entries of rich, non-git, non-imported turns — a delete entry is
    /// a change and is counted here, not held out as a non-write.
    pub agent: u64,
    /// Files currently diverging from their last recorded `after` (see the
    /// unit note above). Entries whose `after` was synthesized are never
    /// counted — derived bytes are not observed fact (PROTOCOL import honesty).
    pub human: u64,
    /// All entries of bare turns (an unattributed activity window, never
    /// rendered as agent activity) PLUS divergent files whose divergence
    /// could have happened inside a recording gap. Spec §3.0.1: gaps are a
    /// third bucket and are "never allocated to either side", so such a file
    /// must not land in `human`.
    pub unattributable: u64,
    /// All entries of imported turns — no epoch coverage, own bucket.
    pub imported: u64,
}

/// The headline metric and every exclusion channel beside it.
///
/// Bucket precedence for a candidate event, in this order and no other:
/// imported → right-censored → unknown-mtime → measurable. No event is ever
/// in two buckets. `gap_overlapped` and `undo_unevaluable_c` are DISCLOSURE
/// counts layered on top: they shrink nothing.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ReworkRate {
    /// Denominator: write events actually judged.
    pub measurable: u64,
    /// Numerator: measurable events reworked within their window.
    pub reworked: u64,
    /// `reworked / measurable`; `None` when `measurable == 0` (renderers say
    /// "no measurable events" rather than "0%").
    pub rate: Option<f64>,
    /// Always [`RATE_BOUND_APPROX_LOWER`] today. Two disclosed bias channels,
    /// opposite directions, NEITHER measured: an edit hidden inside a
    /// recording gap is invisible (undercount), and a gap that strips bracket
    /// coverage mints bare turns clause (a) then counts as rework (overcount).
    pub rate_bound: String,
    /// Events younger than the rework window: excluded from both sides.
    pub censored_recent: u64,
    /// Events on imported turns: their windows are all-gap by construction.
    pub excluded_imported: u64,
    /// DISCLOSURE ONLY — measurable events whose window overlaps a recording
    /// gap. They stay measurable (T0 amendment: the any-gap exclusion measured
    /// `measurable = 0` on the real corpus).
    pub gap_overlapped: u64,
    /// Events whose file now diverges on disk with no later turn and no gap to
    /// date the change — the modification time is unknowable, so the event is
    /// unjudgeable. This one DOES shrink `measurable`.
    pub excluded_unknown_mtime: u64,
    /// DISCLOSURE ONLY — denominator-CANDIDATE turns (rich, non-git,
    /// non-undo) whose `ended` could not be parsed, so none of their entries
    /// could be placed in time and the whole turn was skipped. Counted per
    /// TURN, not per entry. Additive to the plan's nine-field contract,
    /// because a silently dropped turn is exactly the undisclosed exclusion
    /// the honesty rule exists to forbid.
    pub unparsed_ended: u64,
    /// DISCLOSURE ONLY — `tool: "agentrec"` undo turns ending inside some
    /// measurable event's window. Clause (c) of the spec's numerator keys on a
    /// `reverts` field that does not exist before 3.1, so those turns cannot be
    /// evaluated. Shrinks nothing and adds nothing to `reworked`.
    pub undo_unevaluable_c: u64,
}

/// Everything `agentrec stats` renders.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StatsResult {
    pub window: StatsWindow,
    pub turns: TurnCounts,
    /// Ranked by `churn_bytes` descending, then path ascending (a total order,
    /// so goldens over equal-churn files are stable).
    pub files: Vec<FileChurn>,
    pub share: ChangeShare,
    pub rework: ReworkRate,
}

/// Why a stats fold could not be produced. One variant today; typed so the
/// signature does not have to change when a second reason appears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatsError {
    Io(String),
}

impl std::fmt::Display for StatsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatsError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for StatsError {}

/// Parse `YYYY-MM-DDTHH:MM:SS(.fff)?Z` to unix milliseconds.
///
/// Window arithmetic (`ended + N days`) cannot be done on the lexical form the
/// rest of the read side compares, so this is needed. It duplicates a private
/// helper in `cli/src/fmt.rs`; hoisting one of them into `time.rs` is a
/// recorded residual, not this task's scope (T1 is three files).
fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut dp = date.splitn(3, '-');
    let y: i64 = dp.next()?.parse().ok()?;
    let mo: u32 = dp.next()?.parse().ok()?;
    let d: u32 = dp.next()?.parse().ok()?;
    let (hms, frac) = match time.split_once('.') {
        Some((h, f)) => (h, Some(f)),
        None => (time, None),
    };
    let mut tp = hms.splitn(3, ':');
    let h: i64 = tp.next()?.parse().ok()?;
    let mi: i64 = tp.next()?.parse().ok()?;
    let se: i64 = tp.next()?.parse().ok()?;
    let millis: u64 = match frac {
        Some(f) => {
            let mut digits: String = f.chars().take(3).collect();
            while digits.len() < 3 {
                digits.push('0');
            }
            digits.parse().ok()?
        }
        None => 0,
    };
    let days = days_from_civil(y, mo, d);
    let secs = days * 86_400 + h * 3_600 + mi * 60 + se;
    if secs < 0 {
        return None;
    }
    Some(secs as u64 * 1000 + millis)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Does a recording gap overlap `[start, end]`?
///
/// The two shapes are NOT the same test, because [`Gap::since`] means
/// different things per kind (view.rs): for `Crash`/`Restart` it is the point
/// at which the preceding span is PROVEN uncovered, so it must fall inside the
/// window; for `TrailingStop` everything from `since` onward is uncovered, so
/// a stop before the window still covers all of it.
fn gap_overlaps(g: &Gap, start_ms: u64, end_ms: u64) -> bool {
    let Some(since) = parse_rfc3339_ms(&g.since) else {
        return false;
    };
    match g.kind {
        GapKind::TrailingStop => since <= end_ms,
        GapKind::Crash | GapKind::Restart => since >= start_ms && since <= end_ms,
    }
}

/// A denominator-candidate write event: `create` or `modify` (the wire enum is
/// `create|modify|delete`, PROTOCOL §5; a `delete` is not a write).
fn is_write_op(e: &FileEntry) -> bool {
    e.op == "create" || e.op == "modify"
}

fn is_git(t: &TurnRecord) -> bool {
    t.tool.as_deref() == Some("git")
}

fn is_undo(t: &TurnRecord) -> bool {
    t.tool.as_deref() == Some("agentrec")
}

fn is_imported(t: &TurnRecord) -> bool {
    t.imported.unwrap_or(false)
}

/// The fold, with the clock injected.
///
/// `now_ms` is a parameter (and this function private) so the right-censoring
/// boundary — "an event exactly N days old is IN the denominator" — is pinnable
/// by a test instead of racing the system clock. The public entry point is
/// [`crate::view::RepositoryView::stats`]; nothing here is a test seam that
/// could reach a release binary.
pub(crate) fn compute_stats(
    ledger: &Ledger,
    root: &Path,
    opts: &StatsOptions,
    now_ms: u64,
) -> Result<StatsResult, StatsError> {
    let store = BlobStore::new(root.join(".agentrec").join("objects"));
    let gaps = recording_gaps(&ledger.records);

    // Every turn in ledger order, with `ended` parsed once.
    let all: Vec<(&TurnRecord, Option<u64>)> = ledger
        .records
        .iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some((t, parse_rfc3339_ms(&t.ended))),
            LogRecord::Epoch(_) => None,
        })
        .collect();

    let since_ms = opts
        .since
        .map(|d| now_ms.saturating_sub(d.as_millis().min(u128::from(u64::MAX)) as u64));
    // An unparseable `ended` cannot be placed in time: it is out of a bounded
    // window, and in an unbounded one.
    let in_window = |ended: Option<u64>| match (since_ms, ended) {
        (None, _) => true,
        (Some(lo), Some(e)) => e >= lo,
        (Some(_), None) => false,
    };
    let windowed: Vec<(usize, &TurnRecord, Option<u64>)> = all
        .iter()
        .enumerate()
        .filter(|(_, (_, ended))| in_window(*ended))
        .map(|(i, (t, ended))| (i, *t, *ended))
        .collect();

    // ---- turn census -----------------------------------------------------
    let mut turns = TurnCounts::default();
    for (_, t, _) in &windowed {
        turns.total += 1;
        match t.grade.as_str() {
            "rich" => turns.rich += 1,
            "bare" => turns.bare += 1,
            _ => {}
        }
        if is_imported(t) {
            turns.imported += 1;
        }
        if let Some(tool) = &t.tool {
            *turns.by_tool.entry(tool.clone()).or_insert(0) += 1;
        }
        if let Some(model) = &t.model {
            *turns.by_model.entry(model.clone()).or_insert(0) += 1;
        }
    }

    // ---- churn -----------------------------------------------------------
    #[derive(Default)]
    struct Churn {
        bytes: u64,
        dangling: u64,
        skipped: u64,
        withheld: u64,
    }
    let mut churn: BTreeMap<&str, Churn> = BTreeMap::new();
    for (_, t, _) in &windowed {
        for e in &t.files {
            let c = churn.entry(e.path.as_str()).or_default();
            if e.skipped {
                c.skipped += 1;
                continue;
            }
            if e.withheld {
                c.withheld += 1;
                continue;
            }
            // `None` is legitimate absence (a create has no `before`); only a
            // hash the store cannot resolve is a dangling ref.
            let mut dangling = false;
            let mut size_of = |h: &Option<String>| -> u64 {
                match h {
                    None => 0,
                    Some(h) => match store.size(h) {
                        Some(n) => n,
                        None => {
                            dangling = true;
                            0
                        }
                    },
                }
            };
            let before = size_of(&e.before);
            let after = size_of(&e.after);
            if dangling {
                c.dangling += 1;
                continue;
            }
            c.bytes += after.abs_diff(before);
        }
    }
    let mut files: Vec<FileChurn> = churn
        .into_iter()
        .map(|(path, c)| FileChurn {
            path: path.to_string(),
            churn_bytes: c.bytes,
            dangling_refs: c.dangling,
            skipped_entries: c.skipped,
            withheld_entries: c.withheld,
        })
        .collect();
    files.sort_by(|a, b| {
        b.churn_bytes
            .cmp(&a.churn_bytes)
            .then_with(|| a.path.cmp(&b.path))
    });

    // ---- share -----------------------------------------------------------
    let mut share = ChangeShare::default();
    for (_, t, _) in &windowed {
        let n = t.files.len() as u64;
        if is_imported(t) {
            share.imported += n;
        } else if t.grade == "bare" {
            share.unattributable += n;
        } else if is_git(t) {
            // deliberately no bucket — see ChangeShare's doc comment.
        } else {
            share.agent += n;
        }
    }
    // `human`: files whose live bytes diverge from the last `after` anyone
    // recorded for them. Keyed off the LATEST-DATED touching entry in the
    // whole ledger (not the windowed subset, and NOT the last one in file
    // order): `import claude` appends historically-dated turns after live
    // ones, so ledger position is not time order. Ledger position breaks
    // ties between equal `ended` values.
    let paths: BTreeSet<&str> = windowed
        .iter()
        .flat_map(|(_, t, _)| t.files.iter().map(|e| e.path.as_str()))
        .collect();
    for path in paths {
        let Some((last_turn, last)) = all
            .iter()
            .enumerate()
            .filter_map(|(i, (t, ended))| {
                t.files
                    .iter()
                    .rev()
                    .find(|e| e.path == path)
                    .map(|e| (*ended, i, *t, e))
            })
            .max_by_key(|(ended, i, _, _)| (*ended, *i))
            .map(|(_, _, t, e)| (t, e))
        else {
            continue;
        };
        if last.after_synthesized == Some(true) {
            continue; // derived bytes are not observed fact
        }
        let Some(recorded) = &last.after else {
            continue;
        };
        if read_current_hash(root, path).as_deref() == Some(recorded.as_str()) {
            continue;
        }
        // The divergence happened at some unknown time after the last
        // recorded entry. If any interval since then was uncovered it may
        // have happened there, and a gap is its own bucket — "never
        // allocated to either side" (spec §3.0.1).
        if crate::view::has_gap_after(&ledger.records, &last_turn.ended) {
            share.unattributable += 1;
        } else {
            share.human += 1;
        }
    }

    // ---- rework ----------------------------------------------------------
    let window_ms = u64::from(opts.rework_window_days) * DAY_MS;
    // An event is old enough exactly when its age is >= N days, i.e. when its
    // `ended` is at or before this cutoff. Equality is IN the denominator.
    let censor_cutoff = now_ms.saturating_sub(window_ms);
    let mut rework = ReworkRate {
        measurable: 0,
        reworked: 0,
        rate: None,
        rate_bound: RATE_BOUND_APPROX_LOWER.to_string(),
        censored_recent: 0,
        excluded_imported: 0,
        gap_overlapped: 0,
        excluded_unknown_mtime: 0,
        unparsed_ended: 0,
        undo_unevaluable_c: 0,
    };
    // Windows of measurable events; undo turns are matched against these.
    let mut measurable_windows: Vec<(u64, u64)> = Vec::new();

    for (idx, t, ended) in &windowed {
        // Excluded from the denominator entirely (spec §3.0.1): bare turns,
        // git turns, undo turns.
        if t.grade != "rich" || is_git(t) || is_undo(t) {
            continue;
        }
        let Some(t_end) = *ended else {
            // An unplaceable turn is skipped — and said so (A2).
            rework.unparsed_ended += 1;
            continue;
        };
        for e in t.files.iter().filter(|e| is_write_op(e)) {
            if is_imported(t) {
                rework.excluded_imported += 1;
                continue;
            }
            if t_end > censor_cutoff {
                rework.censored_recent += 1;
                continue;
            }
            let win_end = t_end.saturating_add(window_ms);
            // Later turns touching this file, in ledger order. Used only by
            // the rework hit test below, which time-filters its own matches.
            let later: Vec<&(&TurnRecord, Option<u64>)> = all
                .iter()
                .skip(idx + 1)
                .filter(|(u, _)| u.files.iter().any(|f| f.path == e.path))
                .collect();
            // "Nothing recorded after this event" is a question about TIME,
            // not about ledger position: `import claude` appends turns dated
            // in the past, so a backfill import sitting later in the file
            // must not count as evidence of a later change (and, symmetrically,
            // a live turn appended before it must still count).
            let recorded_after_in_time = all.iter().enumerate().any(|(j, (u, u_end))| {
                j != *idx
                    && u_end.is_some_and(|ue| ue > t_end)
                    && u.files.iter().any(|f| f.path == e.path)
            });
            // Unknown-mtime: nothing recorded after this event, the file's
            // live bytes disagree with what was recorded, and no gap exists to
            // date the change. Checked BEFORE anything counts the event.
            if !recorded_after_in_time && e.after_synthesized != Some(true) {
                if let Some(recorded) = &e.after {
                    let diverged =
                        read_current_hash(root, &e.path).as_deref() != Some(recorded.as_str());
                    let dateable = gaps
                        .iter()
                        .any(|g| parse_rfc3339_ms(&g.since).is_some_and(|s| s >= t_end));
                    if diverged && !dateable {
                        rework.excluded_unknown_mtime += 1;
                        continue;
                    }
                }
            }
            rework.measurable += 1;
            measurable_windows.push((t_end, win_end));
            if gaps.iter().any(|g| gap_overlaps(g, t_end, win_end)) {
                rework.gap_overlapped += 1;
            }
            // Clauses (a) and (b): the file is subsequently modified or
            // deleted inside the window by a change no rich turn covers. Undo
            // turns are grade `rich`, so they are not matched here and do not
            // suppress a match either — clause (c) is what would judge them,
            // and it cannot fire before 3.1. (An `is_undo` exclusion would be
            // dead code: no undo turn is graded bare.)
            let reworked = later.iter().any(|(u, u_end)| {
                u.grade == "bare" && u_end.is_some_and(|ue| ue >= t_end && ue <= win_end)
            });
            if reworked {
                rework.reworked += 1;
            }
        }
    }
    if rework.measurable > 0 {
        rework.rate = Some(rework.reworked as f64 / rework.measurable as f64);
    }
    rework.undo_unevaluable_c = all
        .iter()
        .filter(|(t, ended)| {
            is_undo(t)
                && ended.is_some_and(|e| {
                    measurable_windows
                        .iter()
                        .any(|(start, end)| e >= *start && e <= *end)
                })
        })
        .count() as u64;

    Ok(StatsResult {
        window: StatsWindow {
            since: since_ms.map(crate::time::rfc3339),
            until: crate::time::rfc3339(now_ms),
            rework_window_days: opts.rework_window_days,
        },
        turns,
        files,
        share,
        rework,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::EpochRecord;
    use crate::store::hash_bytes;

    // A fixed clock so every window boundary below is exact arithmetic, not a
    // race with the system clock. 2026-08-01T00:00:00.000Z.
    const NOW: u64 = 1_785_542_400_000;

    fn ts(ms: u64) -> String {
        crate::time::rfc3339(ms)
    }

    fn entry(path: &str, op: &str, before: Option<&str>, after: Option<&str>) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            before: before.map(str::to_string),
            after: after.map(str::to_string),
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

    fn turn(
        id: &str,
        grade: &str,
        tool: Option<&str>,
        ended_ms: u64,
        files: Vec<FileEntry>,
    ) -> TurnRecord {
        TurnRecord {
            v: 1,
            id: id.to_string(),
            grade: grade.to_string(),
            truncated: false,
            started: ts(ended_ms.saturating_sub(1000)),
            ended: ts(ended_ms),
            tool: tool.map(str::to_string),
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

    fn epoch(event: &str, ms: u64) -> LogRecord {
        LogRecord::Epoch(EpochRecord {
            v: 1,
            ts: ts(ms),
            event: event.to_string(),
            dropped_signals: 0,
        })
    }

    fn opts(rework_window_days: u32) -> StatsOptions {
        StatsOptions {
            since: None,
            rework_window_days,
        }
    }

    /// A tempdir standing in for the repo root. Tests that do not care about
    /// the working tree still need one, because the unknown-mtime and human
    /// predicates hash live files.
    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    // ---- rework: censoring boundary -------------------------------------

    #[test]
    fn right_censoring_boundary_is_inclusive_at_exactly_n_days() {
        // N = 7 days. NOW - 7d is the cutoff.
        // e1 ended exactly 7 days ago  -> age == N -> MEASURABLE (hand: 1)
        // e2 ended 7d minus 1ms ago    -> younger  -> censored_recent (hand: 1)
        let tmp = tmp();
        let seven = 7 * DAY_MS;
        let led = ledger_of(vec![
            LogRecord::Turn(turn(
                "t1",
                "rich",
                Some("claude"),
                NOW - seven,
                vec![entry("a.rs", "modify", None, None)],
            )),
            LogRecord::Turn(turn(
                "t2",
                "rich",
                Some("claude"),
                NOW - seven + 1,
                vec![entry("b.rs", "modify", None, None)],
            )),
        ]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.measurable, 1);
        assert_eq!(s.rework.censored_recent, 1);
        assert_eq!(s.rework.reworked, 0);
        // 0 reworked over 1 measurable.
        assert_eq!(s.rework.rate, Some(0.0));
        assert_eq!(s.rework.rate_bound, "approx_lower");
    }

    // ---- rework: numerator clauses --------------------------------------

    #[test]
    fn uncovered_deletion_inside_the_window_is_rework() {
        // t1 (rich, 20d ago) writes a.rs        -> measurable event (hand: 1)
        // t2 (BARE, 18d ago) DELETES a.rs       -> uncovered change, clause (b)
        // hand: measurable 1, reworked 1, rate 1.0
        let tmp = tmp();
        let led = ledger_of(vec![
            LogRecord::Turn(turn(
                "t1",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![entry("a.rs", "create", None, None)],
            )),
            LogRecord::Turn(turn(
                "t2",
                "bare",
                None,
                NOW - 18 * DAY_MS,
                vec![entry("a.rs", "delete", None, None)],
            )),
        ]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.measurable, 1);
        assert_eq!(s.rework.reworked, 1);
        assert_eq!(s.rework.rate, Some(1.0));
    }

    #[test]
    fn uncovered_change_outside_the_window_is_not_rework() {
        // t1 (rich, 20d ago) writes a.rs; t2 (bare) touches a.rs 9 days later
        // — outside the 7-day window. hand: measurable 1, reworked 0.
        let tmp = tmp();
        let led = ledger_of(vec![
            LogRecord::Turn(turn(
                "t1",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![entry("a.rs", "modify", None, None)],
            )),
            LogRecord::Turn(turn(
                "t2",
                "bare",
                None,
                NOW - 11 * DAY_MS,
                vec![entry("a.rs", "modify", None, None)],
            )),
        ]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.measurable, 1);
        assert_eq!(s.rework.reworked, 0);
    }

    // ---- rework: exclusions ---------------------------------------------

    #[test]
    fn imported_git_bare_and_undo_turns_never_enter_the_denominator() {
        // t_imp: imported rich, 2 write entries -> excluded_imported = 2
        // t_git: tool "git", 1 entry            -> no bucket at all
        // t_bare: bare, 1 entry                 -> no bucket at all
        // t_undo: tool "agentrec", 1 entry      -> no bucket at all
        // t_ok:  rich claude, 1 entry           -> measurable = 1
        let tmp = tmp();
        let mut imported = turn(
            "t_imp",
            "rich",
            Some("claude"),
            NOW - 20 * DAY_MS,
            vec![
                entry("i1.rs", "create", None, None),
                entry("i2.rs", "modify", None, None),
            ],
        );
        imported.imported = Some(true);
        let led = ledger_of(vec![
            LogRecord::Turn(imported),
            LogRecord::Turn(turn(
                "t_git",
                "rich",
                Some("git"),
                NOW - 20 * DAY_MS,
                vec![entry("g.rs", "modify", None, None)],
            )),
            LogRecord::Turn(turn(
                "t_bare",
                "bare",
                None,
                NOW - 20 * DAY_MS,
                vec![entry("z.rs", "modify", None, None)],
            )),
            LogRecord::Turn(turn(
                "t_undo",
                "rich",
                Some("agentrec"),
                NOW - 20 * DAY_MS,
                vec![entry("u.rs", "modify", None, None)],
            )),
            LogRecord::Turn(turn(
                "t_ok",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![entry("ok.rs", "modify", None, None)],
            )),
        ]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.excluded_imported, 2);
        assert_eq!(s.rework.measurable, 1);
        assert_eq!(s.rework.censored_recent, 0);
        assert_eq!(s.rework.reworked, 0);
    }

    #[test]
    fn delete_ops_are_not_denominator_events() {
        // one rich turn with a create and a delete -> only the create counts.
        let tmp = tmp();
        let led = ledger_of(vec![LogRecord::Turn(turn(
            "t1",
            "rich",
            Some("claude"),
            NOW - 20 * DAY_MS,
            vec![
                entry("a.rs", "create", None, None),
                entry("b.rs", "delete", None, None),
            ],
        ))]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.measurable, 1);
    }

    #[test]
    fn zero_denominator_yields_no_rate() {
        // A ledger of one bare turn: nothing is measurable.
        let tmp = tmp();
        let led = ledger_of(vec![LogRecord::Turn(turn(
            "t1",
            "bare",
            None,
            NOW - 20 * DAY_MS,
            vec![entry("a.rs", "modify", None, None)],
        ))]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.measurable, 0);
        assert_eq!(s.rework.rate, None);
    }

    // ---- rework: gap disclosure -----------------------------------------

    #[test]
    fn gap_inside_a_window_discloses_without_excluding() {
        // t1 rich at NOW-20d -> window [NOW-20d, NOW-13d].
        // stop at NOW-19d + start at NOW-18d = a Restart gap whose `since`
        // (the start) is inside that window.
        // hand: measurable 1, gap_overlapped 1, rate 0.0 approx_lower.
        let tmp = tmp();
        let led = ledger_of(vec![
            LogRecord::Turn(turn(
                "t1",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![entry("a.rs", "modify", None, None)],
            )),
            epoch("stop", NOW - 19 * DAY_MS),
            epoch("start", NOW - 18 * DAY_MS),
        ]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.measurable, 1);
        assert_eq!(s.rework.gap_overlapped, 1);
        assert_eq!(s.rework.rate, Some(0.0));
        // The disclosure is on the wire under a string-typed key.
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["rework"]["rate_bound"], "approx_lower");
        assert_eq!(j["rework"]["gap_overlapped"], 1);
    }

    #[test]
    fn gap_outside_every_window_is_not_disclosed() {
        // Same shape, gap moved to NOW-2d: outside [NOW-20d, NOW-13d].
        let tmp = tmp();
        let led = ledger_of(vec![
            LogRecord::Turn(turn(
                "t1",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![entry("a.rs", "modify", None, None)],
            )),
            epoch("start", NOW - 30 * DAY_MS),
            epoch("stop", NOW - 3 * DAY_MS),
            epoch("start", NOW - 2 * DAY_MS),
        ]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.measurable, 1);
        assert_eq!(s.rework.gap_overlapped, 0);
    }

    // ---- rework: unknown mtime (live working tree) ----------------------

    #[test]
    fn divergent_on_disk_file_with_no_later_turn_and_no_gap_is_unevaluable() {
        // t1 records after = hash("recorded"); disk holds "diverged" and no
        // later turn / no epoch record exists to date the change.
        // hand: excluded_unknown_mtime 1, measurable 0 (it SHRINKS the
        // denominator — unlike gap_overlapped and undo_unevaluable_c).
        let tmp = tmp();
        std::fs::write(tmp.path().join("a.rs"), b"diverged").unwrap();
        let recorded = hash_bytes(b"recorded");
        let led = ledger_of(vec![LogRecord::Turn(turn(
            "t1",
            "rich",
            Some("claude"),
            NOW - 20 * DAY_MS,
            vec![entry("a.rs", "modify", None, Some(&recorded))],
        ))]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.excluded_unknown_mtime, 1);
        assert_eq!(s.rework.measurable, 0);
        assert_eq!(s.rework.rate, None);
    }

    #[test]
    fn matching_on_disk_file_stays_measurable() {
        // Same fixture, disk bytes MATCH the recorded `after`: nothing is
        // unevaluable. hand: measurable 1, excluded_unknown_mtime 0.
        let tmp = tmp();
        std::fs::write(tmp.path().join("a.rs"), b"recorded").unwrap();
        let recorded = hash_bytes(b"recorded");
        let led = ledger_of(vec![LogRecord::Turn(turn(
            "t1",
            "rich",
            Some("claude"),
            NOW - 20 * DAY_MS,
            vec![entry("a.rs", "modify", None, Some(&recorded))],
        ))]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.excluded_unknown_mtime, 0);
        assert_eq!(s.rework.measurable, 1);
    }

    #[test]
    fn divergence_datable_by_a_recording_gap_stays_measurable() {
        // Same divergence, but a trailing stop after the turn dates it.
        let tmp = tmp();
        std::fs::write(tmp.path().join("a.rs"), b"diverged").unwrap();
        let recorded = hash_bytes(b"recorded");
        let led = ledger_of(vec![
            LogRecord::Turn(turn(
                "t1",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![entry("a.rs", "modify", None, Some(&recorded))],
            )),
            epoch("start", NOW - 21 * DAY_MS),
            epoch("stop", NOW - 19 * DAY_MS),
        ]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.excluded_unknown_mtime, 0);
        assert_eq!(s.rework.measurable, 1);
    }

    // ---- rework: undo turns are disclosure-only -------------------------

    #[test]
    fn pre_reverts_undo_turn_in_window_discloses_but_shrinks_nothing() {
        // Discriminating fixture: the undo turn touches THE SAME FILE as the
        // rework event, so a rule that treated it as coverage (it is grade
        // rich) would move `reworked`. Only that direction is pinned here —
        // an undo turn is never grade `bare`, so the fixture cannot
        // discriminate a rule that counted it as an uncovered change.
        // Base ledger (no undo): t1 rich writes a.rs at NOW-20d,
        //   t2 bare touches a.rs at NOW-19d -> measurable 1, reworked 1.
        // With t_undo (tool "agentrec") at NOW-18d, inside t1's window:
        //   measurable 1, reworked 1 UNCHANGED, undo_unevaluable_c 1.
        let tmp = tmp();
        let base = vec![
            LogRecord::Turn(turn(
                "t1",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![entry("a.rs", "modify", None, None)],
            )),
            LogRecord::Turn(turn(
                "t2",
                "bare",
                None,
                NOW - 19 * DAY_MS,
                vec![entry("a.rs", "modify", None, None)],
            )),
        ];
        let without = compute_stats(&ledger_of(base.clone()), tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(without.rework.measurable, 1);
        assert_eq!(without.rework.reworked, 1);
        assert_eq!(without.rework.undo_unevaluable_c, 0);

        let mut with = base;
        with.push(LogRecord::Turn(turn(
            "t_undo",
            "rich",
            Some("agentrec"),
            NOW - 18 * DAY_MS,
            vec![entry("a.rs", "modify", None, None)],
        )));
        let s = compute_stats(&ledger_of(with), tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.undo_unevaluable_c, 1);
        assert_eq!(s.rework.measurable, without.rework.measurable);
        assert_eq!(s.rework.reworked, without.rework.reworked);
    }

    // ---- churn -----------------------------------------------------------

    #[test]
    fn churn_counts_byte_delta_and_ranks_descending() {
        // store holds: "aa" (2 bytes), "bbbbbbbb" (8 bytes), "c" (1 byte).
        // big.rs: before "aa"(2) -> after "bbbbbbbb"(8)  => 6 bytes
        // sml.rs: before None(0) -> after "c"(1)         => 1 byte
        // ranked: big.rs (6) then sml.rs (1).
        let tmp = tmp();
        let store = BlobStore::new(tmp.path().join(".agentrec").join("objects"));
        let two = store.put(b"aa").unwrap();
        let eight = store.put(b"bbbbbbbb").unwrap();
        let one = store.put(b"c").unwrap();
        let led = ledger_of(vec![LogRecord::Turn(turn(
            "t1",
            "rich",
            Some("claude"),
            NOW - 20 * DAY_MS,
            vec![
                entry("big.rs", "modify", Some(&two), Some(&eight)),
                entry("sml.rs", "create", None, Some(&one)),
            ],
        ))]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.files.len(), 2);
        assert_eq!(s.files[0].path, "big.rs");
        assert_eq!(s.files[0].churn_bytes, 6);
        assert_eq!(s.files[1].path, "sml.rs");
        assert_eq!(s.files[1].churn_bytes, 1);
        assert_eq!(s.files[0].dangling_refs, 0);
    }

    #[test]
    fn dangling_blob_ref_contributes_zero_bytes_and_is_counted() {
        // `after` names a hash no blob exists for (evicted): the entry
        // contributes 0 and raises dangling_refs to 1. The second entry on the
        // same file is resolvable and still contributes its 3 bytes.
        let tmp = tmp();
        let store = BlobStore::new(tmp.path().join(".agentrec").join("objects"));
        let three = store.put(b"ccc").unwrap();
        let missing = hash_bytes(b"never stored");
        let led = ledger_of(vec![
            LogRecord::Turn(turn(
                "t1",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![entry("a.rs", "modify", None, Some(&missing))],
            )),
            LogRecord::Turn(turn(
                "t2",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![entry("a.rs", "modify", None, Some(&three))],
            )),
        ]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.files[0].dangling_refs, 1);
        assert_eq!(s.files[0].churn_bytes, 3);
    }

    #[test]
    fn skipped_and_withheld_entries_contribute_zero_and_are_counted() {
        // one skipped entry + one withheld entry on the same path, plus a
        // real 4-byte create. hand: churn 4, skipped 1, withheld 1.
        let tmp = tmp();
        let store = BlobStore::new(tmp.path().join(".agentrec").join("objects"));
        let four = store.put(b"dddd").unwrap();
        let mut skipped = entry("a.rs", "modify", None, None);
        skipped.skipped = true;
        let mut withheld = entry("a.rs", "modify", None, None);
        withheld.withheld = true;
        let led = ledger_of(vec![LogRecord::Turn(turn(
            "t1",
            "rich",
            Some("claude"),
            NOW - 20 * DAY_MS,
            vec![
                entry("a.rs", "create", None, Some(&four)),
                skipped,
                withheld,
            ],
        ))]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.files[0].churn_bytes, 4);
        assert_eq!(s.files[0].skipped_entries, 1);
        assert_eq!(s.files[0].withheld_entries, 1);
    }

    // ---- share -----------------------------------------------------------

    #[test]
    fn git_turn_files_land_in_neither_agent_nor_human() {
        // A git turn's single entry, and its file present on disk with bytes
        // MATCHING the recorded `after` (so the human predicate cannot fire
        // for an unrelated reason). hand: agent 0, human 0, unattributable 0,
        // imported 0.
        let tmp = tmp();
        std::fs::write(tmp.path().join("g.rs"), b"same").unwrap();
        let same = hash_bytes(b"same");
        let led = ledger_of(vec![LogRecord::Turn(turn(
            "t_git",
            "rich",
            Some("git"),
            NOW - 20 * DAY_MS,
            vec![entry("g.rs", "modify", None, Some(&same))],
        ))]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.share.agent, 0);
        assert_eq!(s.share.human, 0);
        assert_eq!(s.share.unattributable, 0);
        assert_eq!(s.share.imported, 0);
    }

    #[test]
    fn bare_turn_entries_are_unattributable_never_agent_or_human() {
        // bare turn, 2 entries, both files on disk matching their recorded
        // `after`. hand: unattributable 2, agent 0, human 0.
        let tmp = tmp();
        std::fs::write(tmp.path().join("x.rs"), b"x").unwrap();
        std::fs::write(tmp.path().join("y.rs"), b"y").unwrap();
        let hx = hash_bytes(b"x");
        let hy = hash_bytes(b"y");
        let led = ledger_of(vec![LogRecord::Turn(turn(
            "t_bare",
            "bare",
            None,
            NOW - 20 * DAY_MS,
            vec![
                entry("x.rs", "modify", None, Some(&hx)),
                entry("y.rs", "modify", None, Some(&hy)),
            ],
        ))]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.share.unattributable, 2);
        assert_eq!(s.share.agent, 0);
        assert_eq!(s.share.human, 0);
    }

    #[test]
    fn agent_and_imported_share_split_and_human_keys_off_live_bytes() {
        // t_agent (rich claude): a.rs, b.rs        -> agent 2
        // t_imp   (imported)   : i.rs              -> imported 1
        // on disk: a.rs diverges from its recorded after -> human 1
        //          b.rs matches                         -> not human
        //          i.rs absent (no bytes)               -> human 1
        // hand: agent 2, imported 1, human 2, unattributable 0.
        let tmp = tmp();
        std::fs::write(tmp.path().join("a.rs"), b"changed by hand").unwrap();
        std::fs::write(tmp.path().join("b.rs"), b"b").unwrap();
        let ha = hash_bytes(b"a");
        let hb = hash_bytes(b"b");
        let hi = hash_bytes(b"i");
        let mut imported = turn(
            "t_imp",
            "rich",
            Some("claude"),
            NOW - 20 * DAY_MS,
            vec![entry("i.rs", "create", None, Some(&hi))],
        );
        imported.imported = Some(true);
        let led = ledger_of(vec![
            LogRecord::Turn(turn(
                "t_agent",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![
                    entry("a.rs", "modify", None, Some(&ha)),
                    entry("b.rs", "modify", None, Some(&hb)),
                ],
            )),
            LogRecord::Turn(imported),
        ]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.share.agent, 2);
        assert_eq!(s.share.imported, 1);
        assert_eq!(s.share.human, 2);
        assert_eq!(s.share.unattributable, 0);
    }

    #[test]
    fn a_delete_entry_is_a_change_and_counts_in_agent() {
        // Gate B1, founder-pinned: share counts ALL entries, deletes
        // included. One rich claude turn with a create AND a delete.
        // hand: agent 2, human 0 (neither file exists on disk and neither
        // entry records an `after`), unattributable 0, imported 0.
        let tmp = tmp();
        let led = ledger_of(vec![LogRecord::Turn(turn(
            "t1",
            "rich",
            Some("claude"),
            NOW - 20 * DAY_MS,
            vec![
                entry("a.rs", "create", None, None),
                entry("gone.rs", "delete", None, None),
            ],
        ))]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.share.agent, 2);
        assert_eq!(s.share.human, 0);
        assert_eq!(s.share.unattributable, 0);
        assert_eq!(s.share.imported, 0);
    }

    #[test]
    fn a_backfilled_import_appended_later_but_dated_earlier_is_not_later_in_time() {
        // `import claude` appends historically-dated turns AFTER live ones, so
        // ledger position is not time order. Fixture:
        //   t_live: rich claude, ended NOW-20d, a.rs after = hash("recorded")
        //   t_imp:  imported,    ended NOW-40d, a.rs after = hash("old")
        //           — appended SECOND, dated FIRST.
        //   disk a.rs = "old" (matches the IMPORT's after, not the live one).
        // Under time order (correct):
        //   - t_imp is not "recorded after" t_live, so t_live's event is still
        //     undateable -> excluded_unknown_mtime 1, measurable 0.
        //   - the latest-DATED entry for a.rs is t_live's, whose `after`
        //     ("recorded") disagrees with disk -> human 1.
        // Under ledger order (the defect) both flip: the import would look
        // later, suppressing the exclusion, and its `after` would match disk,
        // giving human 0. Also hand-counted: excluded_imported 1 (the
        // import's own entry), agent 1, imported 1.
        let tmp = tmp();
        std::fs::write(tmp.path().join("a.rs"), b"old").unwrap();
        let recorded = hash_bytes(b"recorded");
        let old = hash_bytes(b"old");
        let mut imp = turn(
            "t_imp",
            "rich",
            Some("claude"),
            NOW - 40 * DAY_MS,
            vec![entry("a.rs", "modify", None, Some(&old))],
        );
        imp.imported = Some(true);
        let led = ledger_of(vec![
            LogRecord::Turn(turn(
                "t_live",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![entry("a.rs", "modify", None, Some(&recorded))],
            )),
            LogRecord::Turn(imp),
        ]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.excluded_unknown_mtime, 1);
        assert_eq!(s.rework.measurable, 0);
        assert_eq!(s.rework.excluded_imported, 1);
        assert_eq!(s.share.human, 1);
        assert_eq!(s.share.agent, 1);
        assert_eq!(s.share.imported, 1);
    }

    #[test]
    fn a_candidate_turn_with_unparseable_ended_is_skipped_and_disclosed() {
        // A2: a rich turn nothing can place in time is dropped from the fold —
        // and says so. hand: unparsed_ended 1, measurable 0.
        let tmp = tmp();
        let mut t = turn(
            "t1",
            "rich",
            Some("claude"),
            NOW - 20 * DAY_MS,
            vec![entry("a.rs", "modify", None, None)],
        );
        t.ended = "not a timestamp".to_string();
        let led = ledger_of(vec![LogRecord::Turn(t)]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.rework.unparsed_ended, 1);
        assert_eq!(s.rework.measurable, 0);
    }

    #[test]
    fn divergence_after_a_recording_gap_is_unattributable_not_human() {
        // Identical divergence to the previous test's a.rs, but the daemon
        // stopped after the turn: the edit may have happened inside the
        // uncovered interval, so it is allocated to neither side.
        // hand: human 0, unattributable 1, agent 1 (the turn's one entry).
        let tmp = tmp();
        std::fs::write(tmp.path().join("a.rs"), b"changed by hand").unwrap();
        let ha = hash_bytes(b"a");
        let led = ledger_of(vec![
            epoch("start", NOW - 21 * DAY_MS),
            LogRecord::Turn(turn(
                "t1",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![entry("a.rs", "modify", None, Some(&ha))],
            )),
            epoch("stop", NOW - 19 * DAY_MS),
        ]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.share.human, 0);
        assert_eq!(s.share.unattributable, 1);
        assert_eq!(s.share.agent, 1);
    }

    #[test]
    fn synthesized_after_is_never_counted_as_a_human_edit() {
        // Same divergence as above, but `after_synthesized` marks the bytes as
        // DERIVED — a mismatch against derived bytes is not evidence of a
        // human edit (PROTOCOL import honesty). hand: human 0.
        let tmp = tmp();
        std::fs::write(tmp.path().join("a.rs"), b"changed by hand").unwrap();
        let ha = hash_bytes(b"a");
        let mut e = entry("a.rs", "modify", None, Some(&ha));
        e.after_synthesized = Some(true);
        let led = ledger_of(vec![LogRecord::Turn(turn(
            "t1",
            "rich",
            Some("claude"),
            NOW - 20 * DAY_MS,
            vec![e],
        ))]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.share.human, 0);
    }

    // ---- census and window ----------------------------------------------

    #[test]
    fn turn_census_counts_grades_tools_models_and_imports() {
        // t1 rich/claude/sonnet, t2 bare (no tool/model), t3 imported rich
        // claude. hand: total 3, rich 2, bare 1, imported 1,
        // by_tool{claude:2}, by_model{sonnet:1}.
        let tmp = tmp();
        let mut t1 = turn("t1", "rich", Some("claude"), NOW - 20 * DAY_MS, vec![]);
        t1.model = Some("sonnet".to_string());
        let mut t3 = turn("t3", "rich", Some("claude"), NOW - 20 * DAY_MS, vec![]);
        t3.imported = Some(true);
        let led = ledger_of(vec![
            LogRecord::Turn(t1),
            LogRecord::Turn(turn("t2", "bare", None, NOW - 20 * DAY_MS, vec![])),
            LogRecord::Turn(t3),
        ]);
        let s = compute_stats(&led, tmp.path(), &opts(7), NOW).unwrap();
        assert_eq!(s.turns.total, 3);
        assert_eq!(s.turns.rich, 2);
        assert_eq!(s.turns.bare, 1);
        assert_eq!(s.turns.imported, 1);
        assert_eq!(s.turns.by_tool.get("claude"), Some(&2));
        assert_eq!(s.turns.by_model.get("sonnet"), Some(&1));
        assert_eq!(s.turns.by_model.len(), 1);
    }

    #[test]
    fn since_window_excludes_older_turns_from_every_figure() {
        // since = 10 days. t_old ended 20d ago -> out; t_new 5d ago -> in.
        // hand: total 1, and t_old's file is absent from `files`.
        let tmp = tmp();
        let led = ledger_of(vec![
            LogRecord::Turn(turn(
                "t_old",
                "rich",
                Some("claude"),
                NOW - 20 * DAY_MS,
                vec![entry("old.rs", "modify", None, None)],
            )),
            LogRecord::Turn(turn(
                "t_new",
                "rich",
                Some("claude"),
                NOW - 5 * DAY_MS,
                vec![entry("new.rs", "modify", None, None)],
            )),
        ]);
        let o = StatsOptions {
            since: Some(Duration::from_secs(10 * 86_400)),
            rework_window_days: 7,
        };
        let s = compute_stats(&led, tmp.path(), &o, NOW).unwrap();
        assert_eq!(s.turns.total, 1);
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.files[0].path, "new.rs");
        assert_eq!(s.window.since.as_deref(), Some("2026-07-22T00:00:00.000Z"));
        assert_eq!(s.window.until, "2026-08-01T00:00:00.000Z");
        assert_eq!(s.window.rework_window_days, 7);
    }

    #[test]
    fn parse_rfc3339_round_trips_the_formatter() {
        assert_eq!(parse_rfc3339_ms(&crate::time::rfc3339(NOW)), Some(NOW));
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_rfc3339_ms("2026-08-01T00:00:00Z"), Some(NOW));
        assert_eq!(parse_rfc3339_ms("not a timestamp"), None);
    }
}
