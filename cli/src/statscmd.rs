//! `stats`: per-repository analytics — turn census, file churn, agent-vs-
//! human share, and the normative rework rate. Thin adapter over
//! `agentrec_core::stats`/`RepositoryView::stats` (Phase 3.0 T1); everything
//! rendered here comes off the typed [`agentrec_core::stats::StatsResult`]
//! the view returned — no figure is computed in this file.
//!
//! Spec honesty rule (`docs/superpowers/specs/2026-08-07-phase-3-design.md`
//! §3.0.1): every figure prints beside its exclusion counts, never bare. The
//! `ChangeShare` buckets are rendered as raw counts with their unit labels —
//! never as a percentage breakdown, because the four buckets are NOT
//! commensurable (see `stats::ChangeShare`'s doc comment) and a renderer
//! implying they sum to 100% would misrepresent the type.

use agentrec_core::stats::{
    ChangeShare, FileChurn, ReworkRate, StatsOptions, StatsResult, TurnCounts,
};
use agentrec_core::view;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

/// `--since <dur|all>` — `all` means unbounded history (`StatsOptions.since
/// = None`); anything else is `<N><unit>` with unit in `d`/`h`/`m`/`s`
/// (days/hours/minutes/seconds). A custom `FromStr` rather than a runtime
/// check in [`stats`] is deliberate: it makes a bad duration a CLAP-level
/// error (exit 2, clap's own usage text) rather than this command's own
/// error path, per the plan's acceptance criterion.
#[derive(Clone, Debug)]
pub struct SinceArg(pub Option<Duration>);

impl FromStr for SinceArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "all" {
            return Ok(SinceArg(None));
        }
        if s.is_empty() {
            return Err(format!(
                "invalid duration '{s}' (expected e.g. '30d' or 'all')"
            ));
        }
        // Char-boundary-safe split of the final character as the unit — a
        // byte-index split (`s.len() - 1`) panics on any multibyte trailing
        // char (e.g. '3µ'), turning a bad-input case into exit 101 instead
        // of a clap-level error.
        let (num, unit) = match s.char_indices().last() {
            Some((idx, _)) => s.split_at(idx),
            None => (s, ""),
        };
        let n: u64 = num
            .parse()
            .map_err(|_| format!("invalid duration '{s}' (expected e.g. '30d' or 'all')"))?;
        let secs = match unit {
            "d" => n.saturating_mul(86_400),
            "h" => n.saturating_mul(3_600),
            "m" => n.saturating_mul(60),
            "s" => n,
            _ => {
                return Err(format!(
                    "invalid duration unit in '{s}' (expected one of d/h/m/s, or 'all')"
                ))
            }
        };
        Ok(SinceArg(Some(Duration::from_secs(secs))))
    }
}

/// `agentrec stats [--since <dur|all>] [--rework-window <days>] [--json]`.
/// Read-only: opens the view, folds, prints. No write path exists here.
pub fn stats(
    root: &Path,
    since: SinceArg,
    rework_window_days: u32,
    json: bool,
) -> Result<(), String> {
    let view = view::RepositoryView::open(root).map_err(|e| e.to_string())?;
    let opts = StatsOptions {
        since: since.0,
        rework_window_days,
    };
    let result = view.stats(&opts).map_err(|e| e.to_string())?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    print!("{}", render_stats(&result));
    Ok(())
}

/// `stats`'s whole text stdout, from the typed value alone — same wiring
/// discipline as `readcmds::render_diff`/`render_blame`: no `&Path`, no
/// store, no direct ledger read.
fn render_stats(r: &StatsResult) -> String {
    let mut out = String::new();
    out.push_str(&render_window(r));
    out.push('\n');
    out.push_str(&render_turns(&r.turns));
    out.push('\n');
    out.push_str(&render_files(&r.files));
    out.push('\n');
    out.push_str(&render_share(&r.share));
    out.push('\n');
    out.push_str(&render_rework(&r.rework));
    out
}

fn render_window(r: &StatsResult) -> String {
    let since = r.window.since.as_deref().unwrap_or("all");
    format!(
        "stats window: since {since} until {until} \u{b7} rework-window {days}d\n",
        until = r.window.until,
        days = r.window.rework_window_days,
    )
}

fn render_turns(t: &TurnCounts) -> String {
    let mut out = format!(
        "turns: total {total} (rich {rich}, bare {bare}) \u{b7} imported {imported}\n",
        total = t.total,
        rich = t.rich,
        bare = t.bare,
        imported = t.imported,
    );
    if !t.by_tool.is_empty() {
        out.push_str("  by tool: ");
        out.push_str(&join_counts(&t.by_tool));
        out.push('\n');
    }
    if !t.by_model.is_empty() {
        out.push_str("  by model: ");
        out.push_str(&join_counts(&t.by_model));
        out.push('\n');
    }
    out
}

fn join_counts(m: &std::collections::BTreeMap<String, u64>) -> String {
    m.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Ranked by `churn_bytes` descending (the type's own order — see
/// `StatsResult::files`'s doc comment), one line per file, with
/// `skipped_entries`/`withheld_entries` rendered as an undercount footnote
/// ONLY when nonzero (spec A1 fix: never silently absorbed into
/// `churn_bytes`, never noise when there is nothing to disclose).
fn render_files(files: &[FileChurn]) -> String {
    if files.is_empty() {
        return "files: (none in window)\n".to_string();
    }
    let mut out = String::from("files (churn_bytes, dangling_refs):\n");
    for f in files {
        out.push_str(&format!(
            "  {path}  churn_bytes={bytes} dangling_refs={dangling}\n",
            path = f.path,
            bytes = f.churn_bytes,
            dangling = f.dangling_refs,
        ));
        if f.skipped_entries != 0 || f.withheld_entries != 0 {
            out.push_str(&format!(
                "    undercounted: {skipped} skipped, {withheld} withheld entries not reflected in churn_bytes\n",
                skipped = f.skipped_entries,
                withheld = f.withheld_entries,
            ));
        }
    }
    out
}

/// Raw counts, deliberately never a percentage (see module doc + the type's
/// own doc comment: the four buckets are not commensurable units).
fn render_share(s: &ChangeShare) -> String {
    format!(
        "share (raw counts, not percentages \u{2014} buckets use different units, see docs):\n  \
         agent={agent} human={human} unattributable={unattributable} imported={imported}\n",
        agent = s.agent,
        human = s.human,
        unattributable = s.unattributable,
        imported = s.imported,
    )
}

/// spec §3.0.1 requires the OUTPUT (not just the machine-readable
/// `rate_bound` token) to say the rate is an approximate lower bound and to
/// NAME both disclosed bias channels — a recording gap hiding an edit
/// (undercount) and a dropped start signal stripping bracket coverage so
/// agent activity reads as a bare-turn "rework" hit (overcount). Neither
/// channel is measured (see `ReworkRate::rate_bound`'s doc comment in
/// `agentrec-core/src/stats.rs`); this line only names them.
const RATE_BOUND_CHANNELS_LINE: &str = "rate is an approximate lower bound: edits hidden in \
     recording gaps are invisible (undercount); a dropped start signal can strip bracket \
     coverage so agent activity reads as rework (overcount)";

fn render_rework(r: &ReworkRate) -> String {
    let rate_line = match r.rate {
        Some(rate) => format!("rate={rate:.4} ({bound})", bound = r.rate_bound),
        None => "no measurable events".to_string(),
    };
    format!(
        "rework ({bound}):\n  \
         measurable={measurable} reworked={reworked} {rate_line}\n  \
         {channels}\n  \
         excluded: censored_recent={censored} excluded_imported={imported} excluded_unknown_mtime={unknown_mtime}\n  \
         disclosed (not excluded): gap_overlapped={gap} undo_unevaluable_c={undo_c} unparsed_ended={unparsed}\n",
        bound = r.rate_bound,
        measurable = r.measurable,
        reworked = r.reworked,
        channels = RATE_BOUND_CHANNELS_LINE,
        censored = r.censored_recent,
        imported = r.excluded_imported,
        unknown_mtime = r.excluded_unknown_mtime,
        gap = r.gap_overlapped,
        undo_c = r.undo_unevaluable_c,
        unparsed = r.unparsed_ended,
    )
}
