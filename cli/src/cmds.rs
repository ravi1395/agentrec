//! Read verbs (`log`, `status`) and the emitter-side `hook` command. All reads
//! are file-based — they never need the daemon running.

use crate::fmt;
use crate::state::{current_epoch_reloads, read_state, write_state};
use crate::{log_path, memorycmds, objects_dir, signal_path};
use agentrec_core::record::{append_log_line, LogRecord, SignalEvent, TurnRecord};
use agentrec_core::scrub;
use agentrec_core::store::BlobStore;
use std::collections::HashSet;
use std::io::{IsTerminal, Read};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Task 9 / F2 / F8 (INV-M4): hard wall-time budget for the in-process
/// `recall_for_hook_with_deadline` call inside the UserPromptSubmit hook
/// arm. Unlike the original Task 9 shape (measured only AFTER
/// `recall_for_hook` returned, so a slow recall still ran to completion and
/// only its *output* was discarded), `inject_memory` passes a
/// `deadline = Instant::now() + RECALL_BUDGET_MS` INTO the recall call —
/// `memory::recall_with_deadline` checks it at each internal loop boundary
/// (`load_effective`'s fold, `bm25_rank`'s scoring, the freshness-verify
/// walk) and bails out empty the instant it passes. That cooperative check
/// alone is not a hard wall, though: it only runs BETWEEN loop steps, so one
/// slow blocking call inside a step (e.g. `memory::hash_pin`'s `fs::read` on
/// a stalled volume) can still overrun the budget by however long that one
/// call blocks. F8 closes that gap by running the recall on a detached
/// worker thread and waiting only for the REMAINING budget via
/// `mpsc::Receiver::recv_timeout` — see `inject_memory`'s doc comment. The
/// retrospective `elapsed_ms > RECALL_BUDGET_MS` check inside `inject_memory`
/// is kept as a cheap defense-in-depth net, not the primary bound.
const RECALL_BUDGET_MS: u128 = 50;

/// `log`: turns newest-first. Git turns and superseded (merged) turns are hidden
/// unless `--all`. Bare turns render without fabricated tool/prompt columns.
/// Times are relative by default ("3m ago"); `--utc` prints the absolute
/// RFC 3339 timestamp instead (`--json` already emits absolute timestamps and
/// is unaffected by either). `--explain` appends a glossary of only the
/// domain terms that appear in this invocation's rendered output (D43).
///
/// NF-C: `--all-files` is orthogonal to `--all` — `--all` controls which
/// TURNS are visible (git/superseded), `--all-files` controls which FILE
/// ENTRIES within a visible turn are counted individually vs. folded into a
/// `noise_globs` (config.toml) summary line (NF-A/NF-B). Folding is
/// human-render only: `--json` output is never touched by either the
/// matching or the flag (NF-D.1).
pub fn log(
    root: &Path,
    all: bool,
    json: bool,
    limit: usize,
    utc: bool,
    explain: bool,
    all_files: bool,
) -> Result<(), String> {
    // G5: both `log` forms read through the seam. `TurnQuery.limit` stays
    // `None` on purpose — the view limits from the FIRST n oldest-first,
    // while `log` renders newest-first, so `--limit` is applied by the
    // adapter's `.rev().take(limit)` below (design R18). Pushing it into the
    // query would also turn `--limit 0` into `CursorError::ZeroLimit`, a
    // behavior change.
    let view = agentrec_core::view::RepositoryView::open(root).map_err(|e| e.to_string())?;
    let page = view
        .list_records(&agentrec_core::view::TurnQuery {
            include_all: all,
            limit: None,
            after: None,
        })
        .map_err(|e| cursor_error_text(&e))?;
    let turns = &page.items;

    if turns.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("no turns recorded — is `agentrec record` running?");
        }
        return Ok(());
    }

    // Design R18 — the adapter's ONLY sanctioned reshaping: the view returns
    // oldest-first and limits from the FIRST n, while `log` renders
    // newest-first, so the slice happens here, before the renderer, and the
    // glossary is applied after it. That is why `limit` and `explain` are
    // not render opts.
    let selected: Vec<&TurnRecord> = turns.iter().rev().take(limit).collect();

    if json {
        // AC3/AC4: one `serde_json::to_string` line per selected item, of the
        // view's own record. The adapter names no field, so a field added to
        // `TurnRecord` appears here with no edit.
        for turn in &selected {
            let line = serde_json::to_string(turn).map_err(|e| e.to_string())?;
            println!("{line}");
        }
        return Ok(());
    }

    // NF1: absent/empty `noise_globs` yields `None` here, so every branch
    // below that consults `noise_matcher` behaves exactly as it did before
    // this feature existed — no separate "is the feature configured" flag
    // needed anywhere else in this function.
    let noise_globs = crate::noise::read_noise_globs(root);
    let noise_matcher = crate::noise::NoiseMatcher::build(root, &noise_globs);

    let rendered = render_log(
        &selected,
        LogOpts {
            now_ms: wall_now_ms(),
            utc,
            color: fmt::should_color(
                std::io::stdout().is_terminal(),
                std::env::var_os("NO_COLOR").is_some(),
            ),
            all_files,
            noise: noise_matcher.as_ref(),
        },
    );
    print!("{rendered}");

    if explain {
        // D43: the glossary scans exactly what THIS invocation rendered, so
        // it is applied to the renderer's output rather than passed into it.
        let entries = fmt::glossary_for(&rendered);
        if !entries.is_empty() {
            println!();
            println!("── glossary ──");
            for definition in entries {
                println!("  {definition}");
            }
        }
    }
    Ok(())
}

/// Sanctioned human-`log` render opts (design R17/AC17), complete:
/// `now_ms`/`utc`/`color` for [`fmt::turn_list_line`], `all_files` and a
/// PREBUILT [`crate::noise::NoiseMatcher`] for the NF-B fold. The matcher is
/// root-derived at BUILD time and pure at render time, which is why it is
/// admissible where a `&Path` is not. Nothing else — no root, no store, no
/// ledger handle, and no closure over any of them, is reachable from here.
struct LogOpts<'a> {
    now_ms: u64,
    utc: bool,
    color: bool,
    all_files: bool,
    noise: Option<&'a crate::noise::NoiseMatcher>,
}

/// Human `log`'s whole stdout, built purely from the view's typed records
/// plus [`LogOpts`] — the same wiring gate as `readcmds::render_diff`/
/// `render_blame` and `memorycmds::render_recall`: the signature admits no
/// `&Path`, no [`BlobStore`], no records-from-disk handle, so this cannot
/// render from a direct read even if someone wanted it to.
///
/// `items` arrives already reversed and limited by the adapter (R18).
fn render_log(items: &[&TurnRecord], opts: LogOpts) -> String {
    let mut out = String::new();
    for turn in items {
        // NF-B/NF-D.4: fold counts a matched entry out of the visible count
        // regardless of the turn's grade/tool — a turn whose entries are ALL
        // noise still prints its own list line (turn selection is untouched)
        // plus this fold line, never silently disappears.
        let noise_n = if opts.all_files {
            0
        } else {
            opts.noise
                .map(|m| turn.files.iter().filter(|f| m.is_noise(&f.path)).count())
                .unwrap_or(0)
        };
        let visible_files = turn.files.len() - noise_n;
        out.push_str(&format_turn(
            turn,
            opts.now_ms,
            opts.utc,
            opts.color,
            visible_files,
        ));
        out.push('\n');
        if noise_n > 0 {
            out.push_str(&format!("+{noise_n} noise files (--all-files to show)"));
            out.push('\n');
        }
    }
    out
}

/// Human prose for a [`agentrec_core::view::CursorError`]. Unreachable from
/// `log` today (it sets neither `after` nor `limit`), but stated rather than
/// unwrapped, and owned by the adapter — no prose crosses into
/// `agentrec-core`.
fn cursor_error_text(e: &agentrec_core::view::CursorError) -> String {
    match e {
        agentrec_core::view::CursorError::Stale => {
            "the log was rewritten since this cursor — restart the query".to_string()
        }
        agentrec_core::view::CursorError::QueryMismatch => {
            "cursor came from a different query — restart the query".to_string()
        }
        agentrec_core::view::CursorError::ZeroLimit => {
            "a limit of 0 has no honest page".to_string()
        }
    }
}

/// Thin wrapper: computes `log`'s caller-owned fields (relative/UTC time,
/// file count) and hands off to the shared [`fmt::turn_list_line`] renderer
/// (D-PD6 — this used to be a fully independent implementation).
///
/// `visible_files` is the caller-computed count AFTER any `noise_globs`
/// fold (NF-B) — `format_turn` itself stays ignorant of noise matching, same
/// as it was ignorant of turn-grade filtering before this feature existed.
fn format_turn(
    t: &TurnRecord,
    now_ms: u64,
    utc: bool,
    color: bool,
    visible_files: usize,
) -> String {
    let when = if utc {
        t.started.clone()
    } else {
        fmt::relative_time(&t.started, now_ms)
    };
    let files = if visible_files == 1 {
        "1 file".to_string()
    } else {
        format!("{visible_files} files")
    };
    fmt::turn_list_line(t, &when, &files, color)
}

/// `status`: store size, recording gaps (every uncovered-interval kind, with
/// the crash/restart/since-last-stop breakdown — F13), recorder liveness
/// (F31), and rich-rate (the health stat that catches silently broken
/// hooks). `ack_degraded` clears a prior DEGRADED
/// snapshot-failure banner (D35) instead of printing status; clap rejects
/// combining it with `json` (see `main.rs`'s `Status` variant) — the ack
/// path is prose-on-success by design, and prose on stdout under a `--json`
/// flag would break any consumer piping to `jq`. `json` emits
/// machine-readable operational fields instead of the text report: the
/// ignore-reload counters (lifetime + epoch-scoped) plus, since Phase 3 of
/// the honesty-fixes round, every DEGRADED field the text banner reports
/// (additive; more fields can join later).
pub fn status(root: &Path, ack_degraded: bool, json: bool) -> Result<(), String> {
    if ack_degraded {
        let mut state = read_state(root);
        state.snapshot_failures = 0;
        state.io_failed.clear();
        state.non_utf8_path_skips = 0;
        // D35 gap closure: the prompt-put-failure counter is a distinct
        // DEGRADED cause but the same acknowledgement gesture — one
        // `--ack-degraded` clears every "a write silently didn't happen"
        // counter, not just the file-scoped one.
        state.prompt_put_failures = 0;
        // D2: state.json is a real persistence path now (its tmp file can
        // fail to write/rename, e.g. disk full) — a swallowed error here
        // would print "cleared" while the DEGRADED counter is still on disk.
        if let Err(e) = write_state(root, &state) {
            return Err(format!("could not clear DEGRADED state: {e}"));
        }
        println!("acknowledged — DEGRADED cleared");
        return Ok(());
    }
    if json {
        println!("{}", status_json(root)?);
        return Ok(());
    }
    print!("{}", status_report(root, effective_store_budget(root))?);
    Ok(())
}

/// `status --json`'s payload (P5, AC-1): the pre-existing `state.json`
/// operational fields, `#[serde(flatten)]`-joined with the exact
/// [`agentrec_core::view::RepositoryHealth`] `RepositoryView::health`
/// returned — no hand-built JSON, so a field added to `RepositoryHealth`
/// (e.g. `unparsed_lines`, the tolerant-parse counter AC-6 needs) appears
/// here with no edit to this struct.
///
/// Additive, not a replacement: `status --json` shipped in the
/// ignore-rebuild round with a payload `RepositoryHealth` does not cover
/// (`ignore_rebuilds`, `snapshot_failures`, dedup counters, …), and P5.md's
/// "pre-existing overlap" note is explicit that the AC here is routing the
/// EXISTING flag through the view's serializer, not narrowing it down to
/// only what `RepositoryHealth` carries — every field the pre-P5 payload
/// emitted stays exactly where it was, and `#[test] status_omits_stale_
/// epoch_reload_line`'s `payload["ignore_rebuilds"]`-style indexing keeps
/// compiling and passing unchanged (`serde_json::Value` indexing is
/// unaffected by whether a sibling key arrived via `flatten` or a literal
/// field).
///
/// Field order is flatten-then-literal: `RepositoryHealth`'s fields
/// (`store_bytes`, `budget`, `over_budget`, `turn_count`, `crash_gaps`,
/// `restart_gaps`, `trailing_stop_gaps`, `unknown_type_lines`,
/// `unparsed_lines`) appear first, followed by the
/// operational fields below in their declared order — nothing pins this
/// order as a contract (unlike `DiffResult`'s empty-case literal), so this
/// is a legible default, not a promise.
#[derive(serde::Serialize)]
struct StatusJson {
    #[serde(flatten)]
    health: agentrec_core::view::RepositoryHealth,
    ignore_rebuilds: u64,
    epoch_ignore_rebuilds: u64,
    epoch_ignore_rebuilds_stale: bool,
    last_ignore_rebuild_ms: Option<u64>,
    snapshot_failures: u64,
    io_failed: Vec<String>,
    prompt_put_failures: u64,
    state_parse_failures: u64,
    last_bad_field: Option<String>,
    dedup_hits: u64,
    dedup_reread_bytes: u64,
    // T3/D48: the same inbox accounting the text report renders, so a
    // monitoring script watching store growth sees the file that actually
    // grew (13.4 MB on the dogfood store vs. 2.5 MB of log.jsonl). Both
    // fields are ALWAYS present — never only-when-nonzero — the same
    // posture `epoch_ignore_rebuilds_stale` argues for: absence must
    // never be the encoding of "zero", or a consumer cannot distinguish it
    // from an older binary that had no such field. `signal_consumed_bytes`
    // is clamped to the file size for the same reason the text line is.
    signal_bytes: u64,
    signal_consumed_bytes: u64,
}

/// Builds `status --json`'s payload (split out from [`status`] so it's
/// unit-testable without capturing stdout). `state.json` is OPERATIONAL
/// data, not the PROTOCOL wire format (PROTOCOL §5 deliberately keeps it off
/// the wire) — this is a separate, additive JSON surface, not a
/// serialization of a wire record.
///
/// Phase 3 (honesty-fixes round): before this phase, this payload carried
/// only the ignore-reload counters, while the text `status` report also
/// prints a DEGRADED banner — a monitoring script running
/// `status --json | jq .snapshot_failures` got `null`, indistinguishable
/// from healthy, while a human running bare `status` saw the banner. Every
/// field the text banner reports is now here too: `snapshot_failures` +
/// `io_failed` (the affected-files list), `prompt_put_failures`,
/// `state_parse_failures` + `last_bad_field` (Phase 2's per-field-degrade
/// counter). `ignore_rebuilds` keeps its established lifetime-cumulative
/// meaning (additive field, unchanged); `epoch_ignore_rebuilds` is the new
/// current-epoch figure `status`'s text report now renders instead — and,
/// like the text report, it goes through `current_epoch_reloads` rather than
/// the raw field, so a monitoring script sees the same epoch-scoped truth a
/// human sees, including in the stale-epoch window right after a restart or
/// while the daemon is stopped (raw field still holds the previous epoch's
/// count there).
///
/// Residuals round, Phase 3 (Q3 answered as option (c)): `release_lock`
/// never runs on `kill -9`, so a crashed daemon leaves `epoch_nonce` stamped
/// on disk — `current_epoch_reloads` (which has no `root` and therefore
/// cannot probe liveness itself) then renders the dead epoch's count as if
/// it were the live daemon's. `epoch_ignore_rebuilds` itself is deliberately
/// LEFT UNCHANGED here (same value, same computation, live or dead) — the
/// count is still an honest epoch-scoped figure, just not necessarily a
/// LIVE one. The new sibling `epoch_ignore_rebuilds_stale` carries that
/// distinction instead, always present (never only-when-true) so a consumer
/// can tell the two cases apart without inferring it from field absence.
///
/// P5 (AC-2): read-only, same as [`status_report`] — `read_state` never
/// writes back (a corrupted field's healed default lives only in the
/// returned `State`, persisted only by an explicit `write_state` call this
/// function never makes) and `daemon_is_running`'s flock probe is a
/// non-blocking check that creates nothing. `RepositoryView::health` is
/// documented as a pure read. Nothing on this path writes to
/// `.agentrec/objects/`, `log.jsonl`, or `state.json`.
fn status_json(root: &Path) -> Result<serde_json::Value, String> {
    let state = read_state(root);
    let daemon_live = crate::daemon::daemon_is_running(root);
    let signal_bytes = std::fs::metadata(signal_path(root))
        .map(|m| m.len())
        .unwrap_or(0);
    let view = agentrec_core::view::RepositoryView::open(root).map_err(|e| e.to_string())?;
    let health = view
        .health(effective_store_budget(root))
        .map_err(|e| e.to_string())?;
    let payload = StatusJson {
        health,
        ignore_rebuilds: state.ignore_rebuilds,
        epoch_ignore_rebuilds: current_epoch_reloads(&state),
        epoch_ignore_rebuilds_stale: !daemon_live,
        last_ignore_rebuild_ms: if state.ignore_rebuilds > 0 {
            Some(state.last_ignore_rebuild_ms)
        } else {
            None
        },
        snapshot_failures: state.snapshot_failures,
        io_failed: state.io_failed,
        prompt_put_failures: state.prompt_put_failures,
        state_parse_failures: state.state_parse_failures,
        last_bad_field: state.last_bad_field,
        dedup_hits: state.dedup_hits,
        dedup_reread_bytes: state.dedup_reread_bytes,
        signal_bytes,
        signal_consumed_bytes: state.signal_offset.min(signal_bytes),
    };
    serde_json::to_value(&payload).map_err(|e| e.to_string())
}

/// Builds `status`'s full output as a string (split out from [`status`] so
/// the over-budget eviction path is unit-testable with a tiny injected
/// `budget`, instead of requiring a real 2 GiB store — AC I+).
fn status_report(root: &Path, budget: u64) -> Result<String, String> {
    let store = BlobStore::new(objects_dir(root));
    // P4: the read is the view's; the eviction below is this adapter's own
    // explicit call. `status` keeps today's user-visible behavior by making
    // both — but a caller that only wants the facts now has one that writes
    // nothing. One ledger read feeds both this function's record walks and
    // the health figures.
    let view = agentrec_core::view::RepositoryView::open(root).map_err(|e| e.to_string())?;
    let ledger = view.ledger();
    let health = view.health_of(&ledger, budget).map_err(|e| e.to_string())?;
    let size = health.store_bytes;

    // G4: the counting surface — turn count, rich-rate window, git-hidden
    // accounting — IS `TurnQuery { include_all: false }`, so it comes from
    // the summary projection rather than from a filter re-derived here.
    // Every field it needs (`grade`, `tool`, `imported`, `.len()`) is on
    // `TurnSummary`; no widening. The eviction protect-set below is a
    // DIFFERENT set with a different type — see `eviction_plan`.
    let turns = view
        .list_of(
            &ledger,
            &agentrec_core::view::TurnQuery {
                include_all: false,
                limit: None,
                after: None,
            },
        )
        .map_err(|e| cursor_error_text(&e))?
        .items;

    // F13 (redteam round 2): this was `health.crash_gaps` — the crash shape
    // ONLY — so the two other uncovered-interval kinds `view::recording_gaps`
    // already tagged were rendered as no gap at all. `kill <daemon>` → damage
    // → restart is a `Restart` gap and read `gaps: 0`; a cleanly stopped
    // recorder (`TrailingStop`, everything from the stop to now uncovered)
    // read `gaps: 0` too. Silent non-recording is the worst failure mode for a
    // flight recorder, so the total is what leads and the kinds are broken out
    // rather than collapsed — "the recorder crashed" and "the recorder was
    // deliberately off" call for different responses.
    let gap_counts = agentrec_core::view::GapCounts {
        crash: health.crash_gaps,
        restart: health.restart_gaps,
        trailing_stop: health.trailing_stop_gaps,
    };
    let gaps = gap_counts.total();

    // Read once, reused below for the ignore-reload line and (further down)
    // the memory/DEGRADED sections — same single-read pattern those already
    // used, just hoisted so this line can consult it too.
    let state = read_state(root);

    // F31 (redteam round 2): the recorder's own liveness, hoisted from the
    // ignore-reload line below (which already gated on it) so the `daemon:`
    // line can render it too. Deliberately the SAME primitive `doctor`'s
    // `check_daemon` and `purge`'s refusal use — `daemon::daemon_is_running`,
    // a non-blocking `flock` probe on `.agentrec/daemon.lock` (D2) — not a
    // second liveness notion: a pid check false-passes on pid recycling, and
    // two probes that can disagree would be worse than the missing line was.
    let daemon_live = crate::daemon::daemon_is_running(root);

    // Rich-rate over the trailing 20 agent turns (E+): < 90 % warns. With zero
    // agent turns there is no rate to report — a computed 100% would be
    // vacuous (D-PD3), so this prints an honest "n/a" instead.
    //
    // FOUNDER DECISION (P2 integration-gate fix round, Fix 4): imported
    // turns are excluded from this window, same shape as the git-tool
    // exclusion above — an imported turn carries `grade: "rich"` without
    // any hook ever having fired, so a bulk import could otherwise flood
    // the trailing-20 window and make a genuinely broken hook read as
    // 100% healthy (the metric exists specifically to warn "your hooks may
    // be broken", cmds.rs:342-353 below). Deliberately a SEPARATE list
    // from `turns` (not a further narrowing reused elsewhere): `turns:`
    // above still reports the total including imported ones — only the
    // rich-rate window's membership changes.
    let rich_rate_turns: Vec<&agentrec_core::view::TurnSummary> =
        turns.iter().filter(|t| !t.imported).collect();
    let trailing: Vec<&&agentrec_core::view::TurnSummary> =
        rich_rate_turns.iter().rev().take(20).collect();

    // T3/D48: the hook inbox is store accounting `status` never showed. On the
    // dogfood store it reached 13.4 MB against a 2.5 MB `log.jsonl` — bigger
    // than anything else this report renders — because nothing ever removed a
    // signal line. Unconditional (like `store:`/`gaps:`, unlike the derived
    // `rich-rate` and the activity-implying `ignore:` line): a size of 0 B is
    // a fact about accounting, not a vacuous derived figure, and a consumer
    // watching inbox growth needs the line to exist at 0 too. The consumed
    // clause is the same honest-attribution shape as the over-budget notice's
    // orphan clause — it names the bytes AND the only command that reclaims
    // them. `signal_offset` is clamped to the file size before rendering: a
    // stale/corrupt offset must never make this line claim more consumed bytes
    // than the file contains (that inconsistency is `purge`'s to refuse on,
    // not `status`' to render as fact).
    let signal_bytes = std::fs::metadata(signal_path(root))
        .map(|m| m.len())
        .unwrap_or(0);
    let signal_consumed = state.signal_offset.min(signal_bytes);

    let mut out = String::new();
    // F26 (redteam round 2): this line used to render ONE number — every byte
    // under `objects/` — while the budget it implied was enforced over a
    // different, smaller set (the snapshot blobs turn records reference). A
    // store can be far over budget on disk with nothing for the evictor to
    // take, or evict aggressively while this figure barely moves; a single
    // number cannot say which. Both are rendered unconditionally, including
    // when they are equal — the same "a measurement is a fact about
    // accounting, print it at 0 too" posture as the `inbox:` line, and the
    // reason the two notice branches below can each name only their own
    // remedy. `budgeted_bytes <= store_bytes` always, so the second figure
    // never exceeds the first.
    out.push_str(&format!(
        "store:      {} on disk, {} counted toward the budget\n",
        human_bytes(size),
        human_bytes(health.budgeted_bytes)
    ));
    out.push_str(&format!(
        "inbox:      {} signal.jsonl",
        human_bytes(signal_bytes)
    ));
    if signal_consumed > 0 {
        out.push_str(&format!(
            " ({} consumed — reclaim with `agentrec purge --signals-consumed`)",
            human_bytes(signal_consumed)
        ));
    }
    out.push('\n');
    out.push_str(&format!(
        "turns:      {} (agent turns; git activity hidden)\n",
        turns.len()
    ));
    out.push_str(&format!("gaps:       {gaps} recording gap(s)"));
    // Breakdown only when there is something to break down: at zero every
    // kind is zero and "0 recording gap(s)" already says so unambiguously, so
    // a healthy repo's line stays exactly the bytes it has always been. (The
    // inbox line's "0 B is a fact about accounting" argument does not carry
    // here — that line reports a measurement, this one reports a census whose
    // total already encodes the parts when it is 0.) "since last stop" rather
    // than the type name `TrailingStop`: the human report should say what the
    // interval IS (uncovered from the last stop until now), not name a
    // variant.
    if gaps > 0 {
        out.push_str(&format!(
            " ({} crash, {} restart, {} since last stop)",
            gap_counts.crash, gap_counts.restart, gap_counts.trailing_stop
        ));
    }
    out.push('\n');
    // F31: a dead recorder was invisible here — worse, the inbox line above
    // looks HEALTHIER the longer the outage runs (the hook keeps appending to
    // signal.jsonl and succeeds whether or not anything consumes it, so
    // nothing "unconsumed" accumulates in the human's field of view). Nothing
    // in an agent session surfaces the recorder's absence, and `status` is one
    // of only two verbs that could; it was reporting everything except whether
    // recording is happening at all. Unconditional (both states rendered): a
    // line that appears only when dead is a line a human learns to not look
    // for. The warning row uses the rich-rate warning's shape, and its remedy
    // is worded to match `doctor`'s `check_daemon` and the over-budget
    // branch's "daemon not running — nothing is evicting" below, so a stopped
    // recorder reads as one fact restated, not as separate claims.
    if daemon_live {
        out.push_str("daemon:     running\n");
    } else {
        out.push_str("daemon:     not running\n");
        out.push_str(
            "  ⚠ recorder not running — nothing is being recorded (run `agentrec record`)\n",
        );
    }
    // Only rendered once a rebuild has ever happened THIS DAEMON EPOCH — a
    // repo whose .gitignore never churned since the daemon last started has
    // nothing to report, and printing "0 reloads" would be exactly the
    // vacuous line the zero-turn rich-rate line above already refuses to
    // print (D-PD3 precedent). Phase 3 (honesty-fixes round, open question 1
    // option (a)): deliberately `epoch_ignore_rebuilds`, not the lifetime
    // `ignore_rebuilds` — a long-lived repo would otherwise eventually render
    // "reloaded 4821 time(s)" in this daily-driver surface. The lifetime
    // total is still preserved in `state.json` (and in `status --json`); it
    // is just not what this line renders.
    //
    // Blocking gate finding (honesty-round follow-up): the raw field alone is
    // not enough — `record_ignore_rebuild`'s epoch reset only fires on the
    // NEXT rebuild, so right after a restart (or while stopped) the field
    // still holds the previous epoch's count under the previous epoch's
    // identity. `current_epoch_reloads` re-checks `epoch_reload_nonce ==
    // epoch_nonce` at render time — the state needed to detect this was
    // already on disk, nothing read it. (A second honesty-round finding
    // replaced pid identity with a nonce here — see `State::epoch_nonce`'s
    // doc: pid reuse on a long-lived machine could make a later epoch
    // inherit a dead epoch's stale count.)
    let epoch_reloads = current_epoch_reloads(&state);
    // Residuals round, Phase 3: a crashed daemon (`kill -9` skips
    // `release_lock`) leaves `epoch_nonce` stamped, so `epoch_reloads` above
    // can be nonzero for an epoch that is no longer running. The text line
    // is a daily-driver surface a human reads as "current" — so it is
    // gated on an actual liveness probe (the same non-blocking flock check
    // `doctor`/`purge` already use), not just on the epoch-nonce match. F31
    // hoisted that probe above (the `daemon:` line needs the same bit); this
    // gate is unchanged, it just reuses the one binding instead of probing a
    // second time.
    if daemon_live && epoch_reloads > 0 {
        let when = fmt::relative_time(
            &agentrec_core::time::rfc3339(state.last_ignore_rebuild_ms),
            wall_now_ms(),
        );
        out.push_str(&format!(
            "ignore:     reloaded {epoch_reloads} time(s), last {when}\n"
        ));
    }
    if trailing.is_empty() {
        out.push_str("rich-rate:  n/a (no agent turns yet)\n");
    } else {
        let rich = trailing.iter().filter(|t| t.grade == "rich").count();
        let rate = rich as f64 / trailing.len() as f64;
        out.push_str(&format!(
            "rich-rate:  {:.0}% over trailing {} agent turn(s)\n",
            rate * 100.0,
            trailing.len()
        ));
        if rate < 0.90 {
            out.push_str(
                "  ⚠ rich-rate below 90% — hooks may be broken (check `agentrec doctor`)\n",
            );
        }
    }

    // Memory v1: fresh/stale are computed by scanning `load_effective` +
    // re-verifying pin hashes right now (never persisted — INV-M2); rejects
    // is the daemon's persisted counter (`memory_rejects`, the
    // `snapshot_failures` honesty pattern — a rejected candidate leaves no
    // trace in `memory.jsonl`, so this counter is the only visible evidence
    // it happened).
    let all_memories = agentrec_core::memory::load_effective(root).unwrap_or_default();
    let (mut mem_fresh, mut mem_stale) = (0usize, 0usize);
    for m in all_memories.iter().filter(|m| !m.retracted) {
        match agentrec_core::memory::pin_freshness(root, &m.pins) {
            agentrec_core::memory::Freshness::Fresh => mem_fresh += 1,
            agentrec_core::memory::Freshness::Stale
            | agentrec_core::memory::Freshness::Orphaned => mem_stale += 1,
        }
    }
    // I = memory-stats.jsonl lines that recorded a real injection (carry
    // `n`) — the hook-owned injection log (Task 9); this is the only
    // visible evidence recall actually fired into a prompt, so status
    // surfaces it verbatim. F2 also appends `budget_exceeded` lines, and F3
    // a bare `{"ts","capped":true}` line for a capped-and-empty recall, to
    // the same file (neither is an injection) — those must NOT inflate this
    // count, so the filter is content-aware (keys on `n`), not a raw line
    // count. A capped injection still carries `n` (plus `capped`), so it IS
    // counted here, correctly — it really did inject something.
    let injections = std::fs::read_to_string(crate::memory_stats_path(root))
        .map(|text| {
            text.lines()
                .filter(|l| !l.trim().is_empty())
                .filter(|l| {
                    serde_json::from_str::<serde_json::Value>(l)
                        .map(|v| v.get("n").is_some())
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    // F10: memory-store recall failures (a malformed/unreadable NON-EMPTY
    // memory.jsonl, per hook attempt) — same read-only
    // parse-and-count-defensively pattern as `injections` above, keyed on
    // `failure` instead of `n`. Malformed lines in memory-stats.jsonl
    // itself (any JSON parse failure, or a value that isn't a JSON object)
    // are ignored, never a panic — same `.unwrap_or(false)` posture as
    // `injections`. Never mutates state.json (only the daemon writes that).
    let mem_failures = std::fs::read_to_string(crate::memory_stats_path(root))
        .map(|text| {
            text.lines()
                .filter(|l| !l.trim().is_empty())
                .filter(|l| {
                    serde_json::from_str::<serde_json::Value>(l)
                        .map(|v| v.get("failure").and_then(|f| f.as_bool()) == Some(true))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    out.push_str(&format!(
        "memory:     {mem_fresh} fresh, {mem_stale} stale, {} rejects, {injections} injections, {mem_failures} failures\n",
        state.memory_rejects
    ));

    // AC I+ (perf-evidence round, Phase 2b, Decision 7 Q1=(a)): eviction
    // itself now runs from a daemon tick (`daemon::run_eviction_pass`), NOT
    // from this read verb — `status` renders `plan_eviction`'s read-only
    // dry-run report, matching what the daemon would do on its next pass,
    // and deletes nothing. This is what makes AC2b.1 ("status performs zero
    // store writes") true by construction rather than by discipline.
    // F26: the gate is `health.over_budget` — `budgeted_bytes > budget`, which
    // is provably the same condition as "`plan_eviction` has candidates" (see
    // `retention::managed_bytes`) — NOT the old `size > budget`, which fired
    // this eviction dry-run for stores the evictor could do nothing about and
    // reported "over budget" forever while every tick freed nothing.
    //
    // AC I+ (perf-evidence round, Phase 2b, Decision 7 Q1=(a)): eviction
    // itself now runs from a daemon tick (`daemon::run_eviction_pass`), NOT
    // from this read verb — `status` renders `plan_eviction`'s read-only
    // dry-run report, matching what the daemon would do on its next pass,
    // and deletes nothing. This is what makes AC2b.1 ("status performs zero
    // store writes") true by construction rather than by discipline.
    if health.over_budget {
        let plan = eviction_plan(root, &store, &view, &ledger, budget)?;
        // Honesty (B): budget enforcement here only evicts turn-referenced
        // snapshot blobs. Most store bloat is usually ORPHANED blobs —
        // superseded intermediate snapshots the daemon `put` for crash
        // recovery that no committed turn references — which eviction can't
        // touch. Attribute that share explicitly and point at its only
        // reclaim path, instead of claiming "snapshots evicted" when the
        // freed figure is ~0. Still computed here (and not hoisted next to
        // `size` above) because `orphan_bytes` re-walks the store AND
        // re-reads `log.jsonl` raw — a cost neither notice branch's absence
        // should make `status` pay.
        let orphans = crate::purgecmd::orphan_bytes(root, &store);
        out.push_str(&format!(
            "store {} counted toward budget, over {} — would free {}",
            human_bytes(health.budgeted_bytes),
            human_bytes(budget),
            human_bytes(plan.freed_bytes_projected)
        ));
        // Honesty (Phase 1): protecting pinned/in-flight refs shrinks
        // `freed_bytes_projected` — sometimes to 0 even while genuinely over
        // budget — and an unexplained "0 freed" is exactly the dishonest
        // status class the orphan-bloat attribution above already fixed.
        if plan.protected_bytes > 0 {
            out.push_str(&format!(
                "; {} protected (pinned or in-flight — never evicted)",
                human_bytes(plan.protected_bytes)
            ));
        }
        if orphans > 0 {
            out.push_str(&format!(
                "; {} is unreferenced (superseded snapshots) — run `agentrec purge --orphans` to reclaim",
                human_bytes(orphans)
            ));
        }
        // AC2b.3: the daemon-down + `undo`-growth residual (Decisions log
        // #7) is accepted as rare/bounded, but must be self-announcing — a
        // human reading `status` on a stopped daemon must not be left
        // thinking eviction is happening when nothing is evicting anything.
        // F31: reuses the single `daemon_live` binding hoisted above rather
        // than re-probing. Two flock probes in one report can disagree if the
        // daemon exits between them — an over-budget store could otherwise
        // render `daemon:     running` and "daemon not running — nothing is
        // evicting" in the same output. Same value, one observation.
        if !daemon_live {
            out.push_str("; daemon not running — nothing is evicting");
        }
        out.push('\n');
    } else if size > budget {
        // F26's actual failure state, which had no output of its own before:
        // the store exceeds the budget ON DISK while the bytes the evictor
        // ranges over do not, so every tick will free nothing no matter how
        // long it runs. Previously this rendered the eviction dry-run — "over
        // budget — would free 0 B" — which points a user at a mechanism that
        // cannot help them. The remedy is `purge --orphans`, and naming it is
        // the whole point of splitting this branch out.
        //
        // Deliberately does NOT contain "would free": nothing here is going
        // to be freed by eviction, and
        // `status_orphan_bloat_alone_names_purge_not_eviction` asserts that
        // absence, not just the presence of the right words.
        let orphans = crate::purgecmd::orphan_bytes(root, &store);
        out.push_str(&format!(
            "store {} on disk is over the {} budget, but only {} is subject to eviction — the evictor has no candidates and will free nothing",
            human_bytes(size),
            human_bytes(budget),
            human_bytes(health.budgeted_bytes)
        ));
        if orphans > 0 {
            out.push_str(&format!(
                "; {} is unreferenced (superseded snapshots) — run `agentrec purge --orphans` to reclaim",
                human_bytes(orphans)
            ));
        }
        out.push('\n');
    }

    if state.snapshot_failures > 0 || state.non_utf8_path_skips > 0 {
        out.push('\n');
        if state.snapshot_failures > 0 {
            out.push_str(&format!(
                "DEGRADED — {} snapshot write(s) failed (likely disk full or permissions); \
                 undo on affected files has no snapshot. Run `agentrec status --ack-degraded` to acknowledge.\n",
                state.snapshot_failures
            ));
            if !state.io_failed.is_empty() {
                out.push_str("  affected files:\n");
                for path in &state.io_failed {
                    out.push_str(&format!("    {path}\n"));
                }
            }
        }
        if state.non_utf8_path_skips > 0 {
            out.push_str(&format!(
                "DEGRADED — {} file change(s) skipped (non-UTF8 path — cannot be represented \
                 in a wire record); those files have no coverage and undo/blame can't track \
                 them. Run `agentrec status --ack-degraded` to acknowledge.\n",
                state.non_utf8_path_skips
            ));
        }
    }
    // D35 gap closure: a prompt-blob write failure is a SEPARATE line from
    // the file-snapshot banner above — deliberately not folded into the
    // same counter, since the remedy differs (there's no per-file undo
    // refusal for a prompt; the loss is that turn's full-text prompt, the
    // excerpt is unaffected). `--ack-degraded` clears both counters
    // together (see `status`) since they're the same operational concept —
    // "the daemon knows a write silently didn't happen" — just different
    // failure sites.
    if state.prompt_put_failures > 0 {
        out.push('\n');
        out.push_str(&format!(
            "DEGRADED — {} prompt write(s) failed (likely disk full or permissions); \
             `show --prompt` on affected turns has no full-text prompt (the excerpt is unaffected). \
             Run `agentrec status --ack-degraded` to acknowledge.\n",
            state.prompt_put_failures
        ));
    }
    // Phase 2 (honesty-fixes round): `read_state` degrades per FIELD instead
    // of resetting the whole `state.json` struct on one bad value — this is
    // the visible evidence that happened. Deliberately not folded into the
    // "DEGRADED" banners above and not cleared by `--ack-degraded`: unlike a
    // failed write, a corrupt field self-heals the moment any code path next
    // calls `write_state` (the in-memory default gets serialized back), so
    // there is nothing here for a human to acknowledge — only to notice.
    if state.state_parse_failures > 0 {
        out.push('\n');
        out.push_str(&format!(
            "state.json had {} field(s) fall back to defaults (last: {}) — \
             self-heals on the next write; run `agentrec doctor` for detail.\n",
            state.state_parse_failures,
            state.last_bad_field.as_deref().unwrap_or("unknown"),
        ));
    }
    Ok(out)
}

/// The read-only eviction dry-run `status` renders, and the phase's only
/// data-loss path (AC16).
///
/// **The protect-set's turn source is the UNFILTERED turn set** —
/// `TurnQuery { include_all: true }`, never `status`'s filtered counting
/// page. `plan_eviction` protects `prompt_ref` from ANY turn ("protect any
/// blob referenced as a prompt by ANY turn, not just kept ones") and builds
/// its keep-set from every turn's `files[]` hashes. Because the CAS is
/// content-addressed, a superseded or git turn routinely shares a blob with
/// a surviving turn; dropping those turns removes the blob from
/// `protected_prompts` and from `keep`, and eviction then deletes a blob
/// still referenced by recorded history — breaking `undo` of merged turns.
/// `cmds::tests::eviction_protect_set_comes_from_the_unfiltered_turn_set`
/// proves both loss classes.
///
/// Do NOT repair a failure here by widening [`extra_protected_refs`]: its
/// "already reachable through `owned_turns`" premise is true only while
/// `owned_turns` is unfiltered, so widening masks the bug and breaks that
/// deliberate boundary.
///
/// Takes the already-read `ledger` rather than reading its own: `status`
/// makes exactly ONE parse of `log.jsonl`, feeding `health_of`, `list_of`,
/// and this function from it. A second parse would let a daemon append
/// desync the printed turn count from the health and eviction figures.
///
/// Read-only by construction — planning deletes nothing (`execute` is the
/// daemon tick's, never a read verb's).
fn eviction_plan(
    root: &Path,
    store: &BlobStore,
    view: &agentrec_core::view::RepositoryView,
    ledger: &agentrec_core::view::Ledger,
    budget: u64,
) -> Result<agentrec_core::retention::EvictionPlan, String> {
    let owned_turns: Vec<TurnRecord> = view
        .list_records_of(
            ledger,
            &agentrec_core::view::TurnQuery {
                include_all: true,
                limit: None,
                after: None,
            },
        )
        .map_err(|e| cursor_error_text(&e))?
        .items;
    // SAFETY (Phase 1 honesty fix, still load-bearing for the dry-run
    // report): harvest the protect-set as late as possible, immediately
    // before planning, to narrow the window a live daemon (running
    // continuously under launchd — Decisions log #2, no liveness refusal
    // here) could append a new in-flight blob after we've read
    // log.jsonl/open.json but before the plan is built.
    let extra_protected = extra_protected_refs(root);
    Ok(agentrec_core::retention::plan_eviction(
        store,
        &owned_turns,
        budget,
        &extra_protected,
    ))
}

/// Phase 1 honesty fix: hashes `enforce_budget`'s own structured walk over
/// its `entries` argument cannot see, so must be protected separately —
/// `open.json` (the in-flight turn's crash journal), `memory.jsonl` (pin
/// hashes `verify`'s pin-diff resolves as CAS blobs), and any `log.jsonl`
/// line `load_log` silently dropped as torn/unparseable.
///
/// Deliberately NOT `purgecmd::referenced_hashes` wholesale: that function
/// also folds in every VALIDLY-referenced `log.jsonl` hash, which is
/// correct for `--orphans`' absence-based test (anything cited anywhere,
/// however old, must survive) but wrong here — `enforce_budget`'s own
/// age-based walk already decided a validly-referenced blob's fate
/// (including evicting a sole old turn's blob when nothing older exists to
/// sacrifice, `status_prints_over_budget_notice`); passing the FULL
/// referenced-anywhere set as `extra_protected` would protect every
/// snapshot ever committed and silently defeat the budget. Only refs
/// invisible to a structured `load_log` parse are "extra".
///
/// Honesty note (blocking-gate follow-up): the "only torn lines can hide a
/// hash" premise holds for TORN lines specifically, not as a general
/// guarantee. A line that parses fine as `LogRecord` but carries a hash on
/// some field this struct doesn't model (e.g. a future additive PROTOCOL
/// field) would be invisible here — `load_log` succeeds, so `owned_turns`
/// never sees the unmodeled field, and this function only re-scans lines
/// `serde_json::from_str::<LogRecord>` failed on. `purge --orphans`' raw
/// byte-scan has no such blind spot (any `sha256:`-shaped substring counts,
/// parse success or not), so the two would disagree on that hash. No
/// producer emits such a field today; a future additive protocol field
/// carrying a hash needs revisiting this function, not just PROTOCOL.md.
pub(crate) fn extra_protected_refs(root: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    // open.json + memory.jsonl are never themselves a `TurnRecord`, so
    // every ref in them is "extra" by construction — no validity filter
    // needed (mirrors `recover_orphan`'s own tolerance of a corrupt
    // journal: raw bytes are scanned whether or not they parse).
    for path in [
        crate::open_path(root),
        agentrec_core::memory::memory_path(root),
    ] {
        if let Ok(text) = std::fs::read_to_string(&path) {
            crate::purgecmd::harvest_refs(&text, &mut out);
        }
    }
    // log.jsonl: only lines `load_log` could NOT parse contribute — a
    // validly-parsed line's hashes are already reachable through
    // `owned_turns`, so re-adding them here would over-protect (see the
    // doc comment above).
    if let Ok(text) = std::fs::read_to_string(log_path(root)) {
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if serde_json::from_str::<LogRecord>(trimmed).is_err() {
                crate::purgecmd::harvest_refs(trimmed, &mut out);
            }
        }
    }
    out
}

/// `hook`: invoked by a Claude Code lifecycle hook with the JSON payload on
/// stdin. Maps the hook event to a start/stop signal and appends it to the
/// inbox. Prompt text is scrubbed HERE — `signal.jsonl` is on disk, so no
/// pre-scrub prompt may ever reach it (AC I4).
pub fn hook(root: &Path, tool: &str) -> Result<(), String> {
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    let payload: serde_json::Value =
        serde_json::from_str(buf.trim()).unwrap_or(serde_json::json!({}));

    let event_name = payload
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Stop");
    let event = match event_name {
        "UserPromptSubmit" => "start",
        _ => "stop",
    };
    let prompt_raw = payload
        .get("prompt")
        .and_then(|v| v.as_str())
        .map(String::from);
    let prompt = prompt_raw.as_deref().map(scrub::scrub);
    let session = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let transcript = payload
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .map(String::from);

    // D6 (PROTOCOL §4): the emitter itself declares what it wrote. Stop-only;
    // absolute paths verbatim from the transcript; scoped to the current turn
    // by the parser. Best-effort — an unreadable transcript yields None
    // ("did not declare"), never an empty assertion. An empty parse result is
    // also None for the same reason: the recorder's fallback tier applies the
    // identical rule, so the two tiers cannot disagree on emptiness.
    let files_written = if event == "stop" {
        transcript
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|text| crate::daemon::declared_writes_from_transcript(&text))
            .filter(|w| !w.is_empty())
    } else {
        None
    };

    let signal = SignalEvent {
        v: 1,
        ts: wall_now_ms(),
        tool: tool.to_string(),
        event: Some(event.to_string()),
        session,
        transcript,
        prompt,
        files_written,
        kind: None,
        fact: None,
        pins: None,
    };
    let line = serde_json::to_string(&signal).map_err(|e| e.to_string())?;
    append_log_line(&signal_path(root), &line)?;

    // Task 9 (INV-M4): memory injection is strictly additive to the signal
    // append above, which — per the existing contract — must ALWAYS happen
    // regardless of memory outcome. This runs only on the "start" event
    // (UserPromptSubmit); the Stop arm is untouched. Every failure mode
    // (disabled, uninitialized, corrupt store, recall error, over budget,
    // no matches) fails open to "print nothing" — `inject_memory` never
    // returns an error to this function.
    if event == "start" && memorycmds::read_memory_enabled(root) {
        inject_memory(root, prompt_raw.as_deref().unwrap_or(""));
    }

    Ok(())
}

/// Test-only override (Phase 1, `cli/tests/integration.rs`; Phase 2b of the
/// perf-evidence round reuses the SAME seam for the daemon's own eviction
/// tick — deliberately not a second env var, per the plan's infeasible
/// list) that lets an integration test exercise a real over-budget/eviction
/// path without building a genuine ~2 GiB store. Same `#[cfg(debug_assertions)]`
/// fail-safe class as [`TEST_FORCE_BUDGET_EXCEEDED_VAR`] below — compiled
/// out of release builds, so it can never override a real user's budget.
#[cfg(debug_assertions)]
const TEST_STORE_BUDGET_BYTES_VAR: &str = "AGENTREC_TEST_STORE_BUDGET_BYTES";

/// `.agentrec/config.toml` key holding the store budget, in bytes (F28,
/// redteam round 2). Before this the budget was reachable ONLY through
/// [`TEST_STORE_BUDGET_BYTES_VAR`], which is `#[cfg(debug_assertions)]` and
/// therefore compiled out of every shipped binary — a release user had a
/// non-negotiable 2 GiB per root, and the README's own remedy for D6 ("put
/// concurrent work in a separate worktree") multiplies roots.
pub(crate) const STORE_BUDGET_CONFIG_KEY: &str = "store_budget_bytes";

/// Resolution order, highest first:
///
/// 1. [`TEST_STORE_BUDGET_BYTES_VAR`] — debug builds only, never present in a
///    release binary (this repo audits release `strings` for exactly that).
///    It stays highest so the existing integration seams keep driving a tiny
///    budget in fixtures that also carry an `init`-written `config.toml`.
/// 2. `store_budget_bytes` in `.agentrec/config.toml`, via the shared
///    [`config_values`] scanner — same convention as `ttl_days`,
///    `memory_enabled`, `memory_inject_max`.
/// 3. [`agentrec_core::MAX_STORE_BYTES`].
///
/// Levels 2 and 3 are covered by `store_budget_is_settable_from_config_toml`;
/// level 1 beating level 2 is a **control-flow** fact readable three lines
/// below — the env arm `return`s before the config read is reached — and is
/// deliberately NOT asserted by a test: `std::env::set_var` is process-global,
/// and `status`/`status_json` in this same binary call this function, so such
/// a test would race every one of them. Stated as mechanism rather than as a
/// measured outcome on purpose. (`store_budget_override_is_a_no_op_in_release`
/// does cover the release side, where level 1 does not exist at all.)
///
/// A missing file, a missing key, an unparseable value, and an explicit `0`
/// all fall through to the default. The zero case is a deliberate extra
/// condition rather than the bare `parse` other readers use: with F26's
/// managed-byte semantics a budget of 0 makes every evictable snapshot a
/// candidate on the daemon's next tick, so a stray `store_budget_bytes = 0`
/// would be a silent history-wipe. Same protective class as A5 — refuse the
/// value, keep the data. A user who really wants an aggressive budget can set
/// a small non-zero one.
pub(crate) fn effective_store_budget(root: &Path) -> u64 {
    #[cfg(debug_assertions)]
    if let Ok(v) = std::env::var(TEST_STORE_BUDGET_BYTES_VAR) {
        if let Ok(n) = v.parse::<u64>() {
            return n;
        }
    }
    if let Some(text) = read_config_text(root) {
        for value in config_values(&text, STORE_BUDGET_CONFIG_KEY) {
            if let Ok(n) = value.parse::<u64>() {
                if n > 0 {
                    return n;
                }
            }
        }
    }
    agentrec_core::MAX_STORE_BYTES
}

/// Test-only override (`cli/tests/integration.rs`,
/// `hook_recall_bails_at_injected_deadline`) that forces `recall_deadline`
/// to return a deadline already in the past. The real ~50ms window is far
/// too fast for an external test process to race deterministically (and
/// doing so would just reintroduce the exact runner-speed coupling commit
/// 76a716d removed), so this substitutes a deadline that has already
/// expired *before any recall work starts*, regardless of how fast or slow
/// the machine is. Compiled out of release builds entirely (same
/// `#[cfg(debug_assertions)]` fail-safe class as
/// `memory::test_slow_pin_read_delay`) — the const declaration itself is
/// also gated, otherwise it would be dead code once its only reader's env
/// read is compiled away in release.
#[cfg(debug_assertions)]
const TEST_FORCE_BUDGET_EXCEEDED_VAR: &str = "AGENTREC_TEST_FORCE_RECALL_BUDGET_EXCEEDED";

/// The cooperative deadline (F2) `inject_memory` passes into
/// `recall_for_hook_with_deadline`: `started + RECALL_BUDGET_MS`, unless
/// [`TEST_FORCE_BUDGET_EXCEEDED_VAR`] is set, in which case it is a fixed
/// point already 1s in the past. The override is a no-op — the env is never
/// read — in release builds.
fn recall_deadline(started: Instant) -> Instant {
    #[cfg(debug_assertions)]
    if std::env::var_os(TEST_FORCE_BUDGET_EXCEEDED_VAR).is_some() {
        return started
            .checked_sub(Duration::from_secs(1))
            .unwrap_or(started);
    }
    started + Duration::from_millis(RECALL_BUDGET_MS as u64)
}

/// Recalls fresh matching memories for `query` in-process
/// (`memorycmds::recall_for_hook_with_deadline`, not a subprocess) and
/// prints the fenced block to stdout — hard-budgeted and fail-open (INV-M4,
/// F2). The deadline is threaded INTO the recall call (see
/// [`RECALL_BUDGET_MS`]'s doc comment) so a slow recall bails out of its own
/// internal loops rather than running to completion; the retrospective
/// `elapsed_ms` check below is kept only as defense-in-depth. Either a
/// cooperative bail (`budget_exceeded`) or the retrospective net tripping
/// appends a `{"ts","budget_exceeded":true,"elapsed_ms":N}` line to
/// `memory-stats.jsonl` — visible evidence the budget was actually hit, not
/// silent degradation. On an actual injection (non-empty block, within
/// budget) appends `{"ts","n","elapsed_ms":N}` instead, plus `"capped":true`
/// (F3) when the verify walk hit `RECALL_VERIFY_CAP` — recorded as a stat
/// only, NEVER stdout noise; the `--for-hook`/hook stdout contract (block or
/// nothing, exit 0 always) is unconditional. F3 also covers the case a plain
/// `n`-vs-nothing split would miss: a capped walk that found ZERO fresh hits
/// (`outcome.block.is_empty()`) is exactly the silent-truncation scenario
/// this fix exists for, so it gets its own
/// `{"ts","capped":true,"elapsed_ms":N}` line rather than returning with no
/// stat at all. `elapsed_ms` (Phase 1, perf-evidence round) is
/// `started.elapsed().as_millis()`, monotonic-clock-derived, and is now
/// carried on **every** append site in this function, including the two
/// early-bail sites above (thread-spawn failure, `recv_timeout`
/// timeout/disconnect), which previously never computed it at all. Its
/// timing is not uniform across sites, though (finding 7, review round):
/// only the two early-bail sites (spawn failure, `recv_timeout` bail) compute
/// it fresh at `started.elapsed()` immediately before their own append. The
/// four later sites (budget-exceeded-after-recv, store-corrupt,
/// capped-with-no-hits, successful injection) all reuse one shared
/// `elapsed_ms` binding computed once, right after `recv_timeout` returns —
/// not recomputed at each site's own append — so it undercounts whatever
/// those sites do afterward; the successful-injection site in particular
/// writes the fenced block to stdout in between. Never `state.json`, which
/// only the daemon writes (the hazard this task is explicitly gated
/// against).
/// F8: the recall call runs on a detached worker thread; this function
/// waits only for the REMAINING wall budget (`deadline - now`) via
/// `mpsc::Receiver::recv_timeout`, not for the worker itself. That is the
/// hard wall — a single blocking `fs::read` deep inside recall (see
/// [`RECALL_BUDGET_MS`]'s doc comment) can no longer push the OBSERVABLE
/// wall time past budget, because this thread stops waiting the instant the
/// budget elapses regardless of what the worker is still doing.
///
/// The worker is never joined. On a `recv_timeout` timeout (or a
/// disconnect, e.g. the worker panicked before sending), `inject_memory`
/// records `budget_exceeded` and returns immediately; `hook`'s `main()`
/// returns right after, and the process exits — Rust does not wait for
/// detached threads on exit, so a thread still stuck in a slow read cannot
/// hang process shutdown or leave anything running once the process is
/// gone. Abandoning it is safe: `recall_for_hook_with_deadline` only reads
/// (`memory.jsonl` and pinned working-tree files) and never touches
/// `memlock` (only writers — `remember`/`verify`/`forget`/daemon ingest —
/// take that lock), so there is no lock for an abandoned reader to hold
/// across process exit.
///
/// REJECTED alternatives (do not reintroduce, see F8 finding): checking the
/// deadline immediately before/after the blocking read (one call still
/// blows the budget regardless); a file-size cap as a latency proxy (size
/// != latency for slow volumes/special files, and it would change valid-pin
/// semantics).
fn inject_memory(root: &Path, query: &str) {
    let max_facts = memorycmds::read_memory_inject_max(root);
    let started = Instant::now();
    let deadline = recall_deadline(started);

    let (tx, rx) = mpsc::channel::<memorycmds::HookRecallOutcome>();
    let root_owned = root.to_path_buf();
    let query_owned = query.to_string();
    let spawn_result = std::thread::Builder::new()
        .name("agentrec-hook-recall".into())
        .spawn(move || {
            let outcome = memorycmds::recall_for_hook_with_deadline(
                &root_owned,
                &query_owned,
                max_facts,
                deadline,
            );
            // Best-effort: if the receiver already timed out (dropped),
            // there is nothing left to deliver to — the worker just
            // finishes on its own, same fail-open posture as everywhere
            // else in this path.
            let _ = tx.send(outcome);
        });
    // `Builder::spawn` only fails on OS-level thread-creation exhaustion —
    // an extreme edge. Fail open exactly like a timed-out recv: no stdout,
    // one budget_exceeded stat line.
    if spawn_result.is_err() {
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let stats_line = serde_json::json!({
            "ts": wall_now_ms(),
            "budget_exceeded": true,
            "elapsed_ms": elapsed_ms,
        })
        .to_string();
        let _ = append_log_line(&crate::memory_stats_path(root), &stats_line);
        return;
    }

    let remaining = deadline.saturating_duration_since(Instant::now());
    let outcome = match rx.recv_timeout(remaining) {
        Ok(outcome) => outcome,
        Err(_) => {
            // Hard wall tripped (Timeout), or the worker vanished without
            // sending (Disconnected — e.g. a panic). Both fail open
            // identically: no stdout, one budget_exceeded stat line. Any
            // still-running worker is abandoned here, per this function's
            // doc comment.
            let elapsed_ms = started.elapsed().as_millis() as u64;
            let stats_line = serde_json::json!({
                "ts": wall_now_ms(),
                "budget_exceeded": true,
                "elapsed_ms": elapsed_ms,
            })
            .to_string();
            let _ = append_log_line(&crate::memory_stats_path(root), &stats_line);
            return;
        }
    };
    let elapsed_ms = started.elapsed().as_millis();

    if outcome.budget_exceeded || elapsed_ms > RECALL_BUDGET_MS {
        let stats_line = serde_json::json!({
            "ts": wall_now_ms(),
            "budget_exceeded": true,
            "elapsed_ms": elapsed_ms as u64,
        })
        .to_string();
        // Best-effort, same fail-open posture as the recall itself.
        let _ = append_log_line(&crate::memory_stats_path(root), &stats_line);
        return;
    }
    // F10: a malformed/unreadable NON-EMPTY memory.jsonl is a RECALL
    // FAILURE, distinct from budget_exceeded and from a healthy "no
    // matches" (`outcome.block` empty with every flag `false`). Still
    // fail-open (inject nothing, exit 0 — the caller, `hook`, never sees an
    // error), but — unlike the old silent-skip behavior — record it as one
    // bounded, sanitizer-safe `reason` string, never raw filesystem/error/
    // path text (which could carry terminal-control bytes).
    if outcome.store_corrupt {
        let stats_line = serde_json::json!({
            "ts": wall_now_ms(),
            "failure": true,
            "reason": "store_corrupt",
            "elapsed_ms": elapsed_ms as u64,
        })
        .to_string();
        let _ = append_log_line(&crate::memory_stats_path(root), &stats_line);
        return;
    }
    if outcome.block.is_empty() {
        // F3: a capped walk that surfaced no fresh hits is the exact
        // silent-truncation case — record it even though there's no
        // injection to report. Best-effort, same fail-open posture as
        // everywhere else in this function.
        if outcome.capped {
            let stats_line = serde_json::json!({
                "ts": wall_now_ms(),
                "capped": true,
                "elapsed_ms": elapsed_ms as u64,
            })
            .to_string();
            let _ = append_log_line(&crate::memory_stats_path(root), &stats_line);
        }
        return;
    }
    print!("{}", outcome.block);
    let n = outcome
        .block
        .lines()
        .filter(|l| l.starts_with("- "))
        .count();
    let mut stats =
        serde_json::json!({ "ts": wall_now_ms(), "n": n, "elapsed_ms": elapsed_ms as u64 });
    if outcome.capped {
        stats["capped"] = serde_json::json!(true);
    }
    let stats_line = stats.to_string();
    // Best-effort: a memory-stats write failure must not turn a successful
    // injection into a hook failure (same fail-open posture as the recall
    // itself).
    let _ = append_log_line(&crate::memory_stats_path(root), &stats_line);
}

// ---- helpers ----------------------------------------------------------------

/// Ids absorbed by a later rich turn's retroactive merge — consumers drop them.
pub(crate) fn merged_ids(records: &[LogRecord]) -> HashSet<String> {
    let mut out = HashSet::new();
    for r in records {
        if let LogRecord::Turn(t) = r {
            for id in &t.merges {
                out.insert(id.clone());
            }
        }
    }
    out
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Current wall-clock time in milliseconds since the Unix epoch. Shared
/// across CLI modules (daemon crash-journal timestamps, purge TTL math,
/// memory record timestamps, etc.) — the single definition here is
/// canonical; do not add another local copy.
pub(crate) fn wall_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Read `.agentrec/config.toml`'s raw text, or `None` if it doesn't exist.
/// Shared entry point for the hand-rolled `key = value` scanners below (not
/// worth a `toml` dependency for a handful of scalar keys).
pub(crate) fn read_config_text(root: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(crate::agentrec_dir(root).join("config.toml")).ok()
}

/// Every `key = value` line in `text` (comments stripped after `#`), in file
/// order, with the raw trimmed value text. `key` is matched as a literal
/// prefix before whitespace + `=`, so `memory_enabled_foo = true` never
/// matches `key: "memory_enabled"`. Multiple matching lines are all
/// yielded — callers that skip unparseable values fall through to a later
/// line, exactly like the original per-site scanners.
pub(crate) fn config_values<'a>(text: &'a str, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    text.lines().filter_map(move |line| {
        let line = line.split('#').next().unwrap_or("").trim();
        let rest = line.strip_prefix(key)?;
        let value = rest.trim_start().strip_prefix('=')?;
        Some(value.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentrec_core::record::{append_log, FileEntry};
    use std::os::unix::io::AsRawFd;

    /// Residuals round, Phase 3: `status_report`/`status_json` now gate the
    /// reload line/field on `daemon::daemon_is_running`, a REAL non-blocking
    /// `libc::flock` probe against `.agentrec/daemon.lock` — not an
    /// injection seam (this repo's release-`strings` audits exist
    /// specifically to keep test-only seams out of the shipped binary, and
    /// `daemon_is_running`'s premise is already proven in-process by
    /// `daemon_is_running_true_while_held_false_after_release`,
    /// `daemon.rs:2599`: a same-process flock on a distinct `File`/fd
    /// defeats `LOCK_EX|LOCK_NB` exactly like a second process would). This
    /// helper creates the lock file and takes a real exclusive flock on it,
    /// returning the open `File` so the caller holds the lock for as long as
    /// it's kept alive — deliberately WITHOUT going through `daemon::
    /// acquire_lock`, which is private to `daemon.rs` and also mutates
    /// `state.json` (stamping a fresh `epoch_nonce`), which these fixtures
    /// must not have happen out from under their own hand-built `State`.
    fn hold_daemon_lock(root: &std::path::Path) -> std::fs::File {
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();
        let path = crate::agentrec_dir(root).join("daemon.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .unwrap();
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(rc, 0, "test helper failed to acquire its own daemon.lock");
        file
    }

    /// In a release build, `TEST_STORE_BUDGET_BYTES_VAR` must be a no-op —
    /// proves the `#[cfg(debug_assertions)]` arm actually compiles out the
    /// env read rather than merely being unreachable dead code. Only runs
    /// under `cargo test --release` (the debug test build never exercises
    /// this arm at all). Same fail-safe class as
    /// `memory::slow_pin_read_delay_is_none_in_release_even_with_env_set`.
    ///
    /// F28 strengthening: the resolver now has a THIRD level between the env
    /// seam and the default, so "release ignores the env" is asserted twice —
    /// once against the default (no config) and once against a config value
    /// that must win outright. The second half is what distinguishes "the env
    /// read is compiled out" from "the env happened to parse to the default".
    #[test]
    #[cfg(not(debug_assertions))]
    fn store_budget_override_is_a_no_op_in_release() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();

        std::env::set_var("AGENTREC_TEST_STORE_BUDGET_BYTES", "5");
        assert_eq!(
            effective_store_budget(root),
            agentrec_core::MAX_STORE_BYTES,
            "release builds must never honor AGENTREC_TEST_STORE_BUDGET_BYTES"
        );

        std::fs::write(
            crate::agentrec_dir(root).join("config.toml"),
            "store_budget_bytes = 4096\n",
        )
        .unwrap();
        assert_eq!(
            effective_store_budget(root),
            4096,
            "with the env compiled out, config.toml is what a release user has"
        );
        std::env::remove_var("AGENTREC_TEST_STORE_BUDGET_BYTES");
    }

    fn turn_with_snapshot(id: &str, path: &str, hash: &str) -> TurnRecord {
        TurnRecord {
            v: 1,
            id: id.to_string(),
            grade: "rich".into(),
            truncated: false,
            started: "2026-01-01T00:00:00.000Z".into(),
            ended: "2026-01-01T00:00:01.000Z".into(),
            tool: Some("claude".into()),
            model: None,
            session: None,
            root: "/repo".into(),
            prompt_ref: None,
            prompt_excerpt: None,
            merges: vec![],
            imported: None,
            files_complete: None,
            files: vec![FileEntry {
                path: path.into(),
                before: None,
                after: Some(hash.into()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
                after_synthesized: None,
                link_kind: None,
                attribution: None,
            }],
        }
    }

    // AC I+: a store over the (injected, tiny-for-testing) budget prints an
    // over-budget notice and actually evicts — real 2 GiB data is infeasible
    // in a unit test, so `status_report` takes the budget as a parameter.
    #[test]
    fn status_prints_over_budget_notice() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));
        let hash = store.put(&[0u8; 5_000]).unwrap();

        let turn = turn_with_snapshot("t_BUDGETTEST000000000000001", "big.bin", &hash);
        append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();

        let out = status_report(root, 1_000).unwrap();
        assert!(out.contains("over"), "expected over-budget notice: {out}");
        assert!(
            out.contains("would free"),
            "expected the dry-run eviction mention: {out}"
        );
        // Phase 2b (perf-evidence round, Decision 7 Q1=(a)): INVERTED from
        // this test's pre-round form — `status` no longer evicts anything;
        // that moved to the daemon tick (`daemon::run_eviction_pass`).
        assert!(
            store.contains(&hash),
            "status must never delete a blob — it only reports what a daemon tick would do"
        );
    }

    // P4 AC4: reading a repository's health is a pure read. An over-budget
    // store is the case where that is falsifiable — the pre-P4 read path
    // evicted from inside `status_report`, so a caller that only wanted to
    // ask "how is this repo doing?" silently deleted blobs. Asserted on all
    // three write surfaces at once (store bytes, log length, state.json
    // mtime) because eviction touches the first and any accidental
    // re-introduction of a write would most likely land on one of the other
    // two.
    #[test]
    fn health_performs_no_writes_on_an_over_budget_store() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));
        let hash = store.put(&[0u8; 5_000]).unwrap();
        let turn = turn_with_snapshot("t_PUREHEALTH00000000000001", "big.bin", &hash);
        append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();
        crate::state::write_state(root, &crate::state::State::default()).unwrap();

        let state_path = crate::state_path(root);
        let before_bytes = store.total_bytes();
        let before_log = std::fs::metadata(log_path(root)).unwrap().len();
        let before_mtime = std::fs::metadata(&state_path).unwrap().modified().unwrap();

        let view = agentrec_core::view::RepositoryView::open(root).unwrap();
        let health = view.health(1_000).unwrap();

        assert!(health.over_budget, "fixture must actually be over budget");
        assert_eq!(
            store.total_bytes(),
            before_bytes,
            "health() evicted from the store"
        );
        assert_eq!(
            std::fs::metadata(log_path(root)).unwrap().len(),
            before_log,
            "health() appended to log.jsonl"
        );
        assert_eq!(
            std::fs::metadata(&state_path).unwrap().modified().unwrap(),
            before_mtime,
            "health() rewrote state.json"
        );
        assert!(store.contains(&hash), "the blob must survive a pure read");
    }

    // P4 AC7, second half: tolerating an unknown record `type` must not
    // drop the blobs that record references out of the protect-set. A future
    // producer's record kind is unreadable to this binary — which is exactly
    // why its refs must be harvested from the raw line rather than inferred
    // from a parse that did not happen.
    #[test]
    fn an_unknown_record_type_still_contributes_to_the_protected_ref_set() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));
        let referenced = store.put(&[0xCCu8; 4_000]).unwrap();

        let line = serde_json::json!({
            "type": "future_thing",
            "v": 1,
            "files": [{"path": "x.bin", "before": null, "after": referenced, "op": "modify"}]
        })
        .to_string();
        let log = log_path(root);
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, format!("{line}\n")).unwrap();

        assert!(
            extra_protected_refs(root).contains(&referenced),
            "a blob referenced only by an unknown-type record must stay protected"
        );

        // F26 follow-up: this fixture used to reach the eviction dry-run purely
        // because `store_bytes > budget`, and a record this binary cannot parse
        // contributes NOTHING to `budgeted_bytes` — so under the corrected
        // predicate the eviction branch would no longer run at all and the
        // survival assertion below would be vacuous. A real, parseable turn is
        // added so the fixture still lands on the branch it is about.
        let parseable = store.put(&[0xDDu8; 4_000]).unwrap();
        let turn = turn_with_snapshot("t_UNKNOWNTYPEPEER00000001", "peer.bin", &parseable);
        append_log(&log, &LogRecord::Turn(turn)).unwrap();

        // And it survives the real over-budget path, not just the harvest.
        let out = status_report(root, 100).unwrap();
        assert!(
            out.contains("would free"),
            "fixture must reach the eviction dry-run branch: {out}"
        );
        assert!(
            store.contains(&referenced),
            "eviction dropped a blob referenced by a record it could not parse"
        );
    }

    // Honesty (B): when the store is over budget AND holds unreferenced
    // (orphan) blobs, the notice must attribute that bloat to superseded
    // snapshots and point at `purge --orphans` — not just claim eviction.
    #[test]
    fn status_over_budget_attributes_orphan_bloat_and_names_reclaim() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        // A referenced blob (kept) plus an unreferenced orphan blob.
        let kept = store.put(&[0xAAu8; 400]).unwrap();
        let _orphan = store.put(&[0xBBu8; 600]).unwrap();
        let turn = turn_with_snapshot("t_ORPHANBLOAT0000000000001", "kept.bin", &kept);
        append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();

        // F26: the budget must be under the EVICTABLE bytes (400), not merely
        // under the 1000 B on disk — the old `500` sat between the two, which
        // is precisely the state this round proved the eviction dry-run must
        // NOT claim (see `status_orphan_bloat_alone_names_purge_not_eviction`,
        // which now owns that case). Tightening, not loosening: both
        // assertions below are unchanged and still required.
        let out = status_report(root, 300).unwrap();
        assert!(
            out.contains("would free"),
            "fixture must reach the eviction dry-run branch: {out}"
        );
        assert!(
            out.contains("unreferenced (superseded snapshots)"),
            "expected orphan attribution: {out}"
        );
        assert!(
            out.contains("purge --orphans"),
            "expected the reclaim command named: {out}"
        );
    }

    /// F26: a store over budget ON DISK whose evictable set is under it must
    /// name `purge --orphans` and must NOT render the eviction dry-run. Before
    /// this round `status` printed "over budget — would free 0 B" here forever,
    /// pointing the user at the one mechanism that cannot help.
    ///
    /// The negative assertion is the one carrying the finding: "names purge"
    /// was already true (the orphan clause hung off the eviction branch), so a
    /// presence-only test would have passed before the fix too.
    ///
    /// Neuter: change `status_report`'s gate back to `size > budget` and the
    /// `would free` assertion reds.
    #[test]
    fn status_orphan_bloat_alone_names_purge_not_eviction() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let kept = store.put(&[0xAAu8; 400]).unwrap();
        let orphan = store.put(&[0xBBu8; 600]).unwrap();
        let turn = turn_with_snapshot("t_ORPHANONLY00000000000001", "kept.bin", &kept);
        append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();

        // 1000 B on disk, 400 B evictable, 500 B budget — over on disk, under
        // on the set eviction ranges over.
        let out = status_report(root, 500).unwrap();
        assert!(
            !out.contains("would free"),
            "eviction cannot help here and must not be offered: {out}"
        );
        assert!(
            out.contains("subject to eviction"),
            "expected the disk-vs-evictable split to be named: {out}"
        );
        assert!(
            out.contains("unreferenced (superseded snapshots)") && out.contains("purge --orphans"),
            "expected the orphan attribution and its reclaim command: {out}"
        );
        assert!(store.contains(&orphan) && store.contains(&kept));
    }

    /// F26: the `store:` line renders BOTH figures, so a human can tell disk
    /// pressure from evictor-reclaimable pressure without running `purge`.
    ///
    /// Neuter: drop the second figure from the format string and the
    /// `counted toward the budget` assertion reds.
    #[test]
    fn status_store_line_shows_disk_and_budgeted_bytes_separately() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let kept = store.put(&[0xAAu8; 400]).unwrap();
        let _orphan = store.put(&[0xBBu8; 600]).unwrap();
        let turn = turn_with_snapshot("t_STORELINE000000000000001", "kept.bin", &kept);
        append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();

        let out = status_report(root, 1_000_000).unwrap();
        assert!(
            out.contains(&format!(
                "store:      {} on disk, {} counted toward the budget",
                human_bytes(1_000),
                human_bytes(400)
            )),
            "expected both figures on the store line: {out}"
        );
    }

    #[test]
    fn status_under_budget_prints_no_notice() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));
        let hash = store.put(b"tiny").unwrap();

        let turn = turn_with_snapshot("t_SMALLTEST000000000000001", "small.txt", &hash);
        append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();

        let out = status_report(root, 1_000_000).unwrap();
        // F26: this was a single `!out.contains("budget")`, which stopped being
        // a valid proxy once the unconditional `store:` line began naming the
        // budget it is measured against. Replaced by an assertion against every
        // phrase either notice branch can emit — strictly more coverage than
        // the one substring gave (it could not distinguish the two branches at
        // all), not a narrowing.
        for phrase in [
            "would free",
            "subject to eviction",
            "purge --orphans",
            "nothing is evicting",
            "protected (pinned or in-flight",
        ] {
            assert!(
                !out.contains(phrase),
                "no over-budget notice expected, found {phrase:?}: {out}"
            );
        }
        assert!(store.contains(&hash));
    }

    /// F28: the budget is settable from `.agentrec/config.toml` in a RELEASE
    /// build — before this it was reachable only through a
    /// `#[cfg(debug_assertions)]` env var, i.e. not at all for a shipped
    /// binary. Drives the resolver directly (not the env seam) so it proves
    /// the config path, which is the half that ships.
    #[test]
    fn store_budget_is_settable_from_config_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();

        // Absent file → documented default.
        assert_eq!(
            effective_store_budget(root),
            agentrec_core::MAX_STORE_BYTES,
            "no config.toml must fall back to the default"
        );

        let config = crate::agentrec_dir(root).join("config.toml");
        std::fs::write(&config, "ttl_days = 90\nstore_budget_bytes = 1048576\n").unwrap();
        assert_eq!(
            effective_store_budget(root),
            1_048_576,
            "config.toml's store_budget_bytes must win over the default"
        );

        // Unparseable → default, same posture as `read_ttl_days`.
        std::fs::write(&config, "store_budget_bytes = \"lots\"\n").unwrap();
        assert_eq!(effective_store_budget(root), agentrec_core::MAX_STORE_BYTES);

        // Zero → default. A 0 budget makes every evictable snapshot a
        // candidate on the daemon's next tick; honoring it would turn one
        // stray config line into a silent history wipe.
        std::fs::write(&config, "store_budget_bytes = 0\n").unwrap();
        assert_eq!(
            effective_store_budget(root),
            agentrec_core::MAX_STORE_BYTES,
            "a zero budget must be refused, not honored"
        );

        // Prefix discipline inherited from `config_values`: a longer key that
        // merely starts with ours must not match.
        std::fs::write(&config, "store_budget_bytes_extra = 42\n").unwrap();
        assert_eq!(effective_store_budget(root), agentrec_core::MAX_STORE_BYTES);
    }

    // Item 2 (non-UTF8 path handling): a persisted `non_utf8_path_skips`
    // count must surface via the same DEGRADED mechanism as I/O snapshot
    // failures, and `--ack-degraded` must clear it the same way.
    #[test]
    fn status_surfaces_non_utf8_path_skips_as_degraded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let mut state = crate::state::State::default();
        crate::state::record_non_utf8_path_skip(&mut state);
        crate::state::record_non_utf8_path_skip(&mut state);
        write_state(root, &state).unwrap();

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(out.contains("DEGRADED"), "expected DEGRADED notice: {out}");
        assert!(out.contains("non-UTF8"), "expected non-UTF8 mention: {out}");
        assert!(out.contains('2'), "expected the count in the notice: {out}");
    }

    #[test]
    fn status_ack_degraded_clears_non_utf8_path_skips() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let mut state = crate::state::State::default();
        crate::state::record_non_utf8_path_skip(&mut state);
        write_state(root, &state).unwrap();

        status(root, true, false).unwrap();

        let after = read_state(root);
        assert_eq!(after.non_utf8_path_skips, 0);
    }

    // Phase 2 (honesty-fixes round): a corrupt state.json field bumps
    // `state_parse_failures` (via `read_state`'s per-field degrade), and
    // `status` must say so — silent loss of this counter is exactly the
    // failure shape every other counter on this page exists to prevent.
    #[test]
    fn status_report_surfaces_state_parse_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        std::fs::write(
            crate::state_path(root),
            r#"{"pid":0,"signal_offset":"nope"}"#,
        )
        .unwrap();

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains("fall back to defaults"),
            "expected the state-parse notice: {out}"
        );
        assert!(
            out.contains("signal_offset"),
            "expected the bad field named: {out}"
        );
    }

    // D-PD5: the turns line drops implementer jargon ("agent, git/merged
    // excluded") for plain language ("agent turns; git activity hidden").
    #[test]
    fn status_turns_line_uses_plain_language_not_jargon() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains("(agent turns; git activity hidden)"),
            "expected plain-language parenthetical: {out}"
        );
        assert!(
            !out.contains("git/merged excluded"),
            "old jargon must be gone: {out}"
        );
    }

    fn turn_with_grade(id: &str, grade: &str) -> TurnRecord {
        TurnRecord {
            v: 1,
            id: id.to_string(),
            grade: grade.to_string(),
            truncated: false,
            started: "2026-01-01T00:00:00.000Z".into(),
            ended: "2026-01-01T00:00:01.000Z".into(),
            tool: Some("claude".into()),
            model: None,
            session: None,
            root: "/repo".into(),
            prompt_ref: None,
            prompt_excerpt: None,
            merges: vec![],
            imported: None,
            files_complete: None,
            files: vec![],
        }
    }

    // D-PD5: the low-rich-rate remedy now points at `agentrec doctor` (the
    // diagnosis command) instead of `agentrec init` (which does nothing to
    // fix broken hooks on a repo that's already initialized).
    #[test]
    fn status_low_rich_rate_remedy_points_at_doctor_not_init() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        // All-bare turns: 0% rich, well under the 90% warn threshold.
        for i in 0..10 {
            let turn = turn_with_grade(&format!("t_BARE{i:021}"), "bare");
            append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();
        }

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains("agentrec doctor"),
            "expected the remedy to point at `agentrec doctor`: {out}"
        );
        assert!(
            !out.contains("check `agentrec init`"),
            "remedy must no longer point at `agentrec init`: {out}"
        );
    }

    // FOUNDER DECISION (P2 integration-gate fix round, Fix 4): an imported
    // turn (`grade: "rich"`, but no hook ever fired) must not count toward
    // the rich-rate window at all — not just "not count as rich", but not
    // even occupy a window SLOT, since the whole failure mode is a bulk
    // import diluting a genuinely broken hook's signal. All 10 turns here
    // are imported; the window must have nothing to compute a rate over.
    #[test]
    fn status_rich_rate_excludes_imported_turns_entirely() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        for i in 0..10 {
            let mut turn = turn_with_grade(&format!("t_imp_{i:021}"), "rich");
            turn.imported = Some(true);
            turn.files_complete = Some(false);
            append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();
        }

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains("rich-rate:  n/a (no agent turns yet)"),
            "an all-imported log must report n/a — nothing live to rate: {out}"
        );
    }

    // The property actually worth protecting: a genuinely broken hook
    // (bare turns) must still drive the rate down and trip the warning,
    // even when imported turns are ALSO present in the same log — a bulk
    // import must never mask a real regression by diluting the window with
    // turns no hook ever produced.
    #[test]
    fn status_rich_rate_still_reflects_broken_hooks_alongside_imported_turns() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        // 20 imported turns first (would fill the entire trailing-20
        // window under the old, unfixed behavior).
        for i in 0..20 {
            let mut turn = turn_with_grade(&format!("t_imp_{i:021}"), "rich");
            turn.imported = Some(true);
            turn.files_complete = Some(false);
            append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();
        }
        // Then 10 bare (live, hook-not-firing) turns — the real signal.
        for i in 0..10 {
            let turn = turn_with_grade(&format!("t_BARE{i:021}"), "bare");
            append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();
        }

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains("rich-rate:  0% over trailing 10 agent turn(s)"),
            "the window must be the 10 live bare turns only, not diluted by \
             the 20 imported ones ahead of them: {out}"
        );
        assert!(
            out.contains("hooks may be broken"),
            "a genuinely broken-hook signal must still trip the warning \
             even with imported turns present in the log: {out}"
        );
    }

    // AC-F10.4: `status` derives its memory-failure count from
    // memory-stats.jsonl `failure:true` lines, and malformed lines in that
    // same file (not valid JSON at all, or a JSON value that isn't an
    // object) are ignored rather than panicking status — same defensive
    // posture as the pre-existing `injections` count just above it.
    #[test]
    fn status_counts_memory_failures_and_tolerates_malformed_stats_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        let stats_path = crate::memory_stats_path(root);
        std::fs::create_dir_all(stats_path.parent().unwrap()).unwrap();
        let lines = [
            r#"{"ts":1,"failure":true,"reason":"store_corrupt"}"#,
            r#"{"ts":2,"n":3}"#, // a real injection — not a failure
            "not json at all — must not panic status",
            r#"[1,2,3]"#, // valid JSON but not an object — must not panic
            r#"{"ts":3,"failure":true,"reason":"store_corrupt"}"#,
        ];
        std::fs::write(&stats_path, lines.join("\n") + "\n").unwrap();

        // Must not panic despite the malformed lines.
        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains("2 failures"),
            "expected exactly 2 memory-store failures counted, malformed lines ignored: {out}"
        );
    }

    // Phase 2 of the rebuild-gate fix: a repo whose `.gitignore` never
    // churned must never print a vacuous "0 reloads" line (the zero-turn
    // `rich-rate: n/a` precedent, D-PD3). The second half of this test
    // (nonzero counter -> line DOES appear) is load-bearing, not padding: a
    // version of `status_report` that never prints a reload line at all
    // would pass the first half for the wrong reason. The second half stamps
    // a real `epoch_nonce` (mirroring what `acquire_lock` does at epoch
    // start) rather than relying on `State::default()`'s empty-string nonce
    // — a genuinely live epoch always has a non-empty nonce; using the
    // default here would only coincidentally match `current_epoch_reloads`'s
    // own default and wouldn't represent a real daemon epoch.
    #[test]
    fn status_omits_reload_line_when_never_reloaded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            !out.to_lowercase().contains("reload"),
            "no reload line expected when ignore_rebuilds is 0: {out}"
        );

        // Residuals round, Phase 3: the reload line now also requires a live
        // daemon (`daemon_is_running`'s real flock probe) — hold the lock so
        // this presence assertion still discriminates the thing it always
        // meant to discriminate (a nonzero epoch count), not liveness.
        let _guard = hold_daemon_lock(root);
        let mut state = crate::state::State {
            pid: 111,
            epoch_nonce: "epoch-a".to_string(),
            ..crate::state::State::default()
        };
        crate::state::record_ignore_rebuild(&mut state, 1_000);
        write_state(root, &state).unwrap();
        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.to_lowercase().contains("reload"),
            "expected a reload line once ignore_rebuilds > 0: {out}"
        );
    }

    // Phase 3 (honesty-fixes round), open question 1 answered as option (a):
    // the rendered reload figure must be scoped to the CURRENT daemon epoch,
    // not the lifetime total, once a daemon restart has happened. Simulates
    // an old epoch (nonce "epoch-a", 3 rebuilds) followed by a restart
    // (nonce "epoch-b", 2 more rebuilds) — the same shape `acquire_lock` +
    // `record_ignore_rebuild` produce in production (honesty round: epoch
    // identity is the nonce, not the pid — see `State::epoch_nonce`'s doc).
    // Sibling non-default value pinned per the vacuity trap: `ignore_rebuilds`
    // (5, lifetime) is asserted alongside the rendered epoch figure (2) — a
    // version of `status_report` that renders the lifetime total would print
    // "5", not "2", and this test would catch it. Neuter: render
    // `state.ignore_rebuilds` instead of `state.epoch_ignore_rebuilds` →
    // RED.
    #[test]
    fn status_reload_line_is_epoch_scoped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        // Residuals round, Phase 3: presence assertion below now also needs
        // a live daemon.
        let _guard = hold_daemon_lock(root);

        let mut state = crate::state::State {
            pid: 111,
            epoch_nonce: "epoch-a".to_string(),
            ..crate::state::State::default()
        };
        crate::state::record_ignore_rebuild(&mut state, 1_000);
        crate::state::record_ignore_rebuild(&mut state, 2_000);
        crate::state::record_ignore_rebuild(&mut state, 3_000);
        // Simulate the daemon restart `acquire_lock` performs: a fresh pid
        // AND a fresh nonce stamped before any rebuild in the new epoch
        // happens.
        state.pid = 222;
        state.epoch_nonce = "epoch-b".to_string();
        crate::state::record_ignore_rebuild(&mut state, 4_000);
        crate::state::record_ignore_rebuild(&mut state, 5_000);
        write_state(root, &state).unwrap();

        assert_eq!(
            read_state(root).ignore_rebuilds,
            5,
            "lifetime total must be preserved across the simulated restart"
        );

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains("reloaded 2 time(s)"),
            "expected the CURRENT-EPOCH figure (2), not the lifetime total: {out}"
        );
        assert!(
            !out.contains("reloaded 5 time(s)"),
            "must not render the lifetime-cumulative figure: {out}"
        );
    }

    // Blocking gate finding (honesty-round follow-up): `status_reload_line_is_
    // epoch_scoped` above only covers a restart that has ALREADY seen a
    // rebuild in the new epoch (record_ignore_rebuild does the reset+bump
    // together). The gate found the real hole is the window BEFORE that:
    // right after a restart, or while the daemon is stopped, nothing has
    // called `record_ignore_rebuild` yet, so `epoch_ignore_rebuilds`/
    // `epoch_reload_nonce` on disk still belong to the OLD epoch — and the
    // old `status_report` rendered them unconditionally, attributing a dead
    // daemon's reloads to the live one. Neuter: remove the
    // epoch_reload_nonce==epoch_nonce gate (render `state.
    // epoch_ignore_rebuilds` unconditionally again) → RED on both sub-cases
    // below.
    #[test]
    fn status_omits_stale_epoch_reload_line() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        // Old epoch (pid 111, nonce "epoch-a") rebuilt 3 times; nothing has
        // rebuilt yet since.
        let mut state = crate::state::State {
            pid: 111,
            epoch_nonce: "epoch-a".to_string(),
            ..crate::state::State::default()
        };
        crate::state::record_ignore_rebuild(&mut state, 1_000);
        crate::state::record_ignore_rebuild(&mut state, 2_000);
        crate::state::record_ignore_rebuild(&mut state, 3_000);
        assert_eq!(state.epoch_reload_nonce, "epoch-a");
        assert_eq!(state.epoch_ignore_rebuilds, 3);

        // Case A: daemon restarted under a NEW pid AND a NEW nonce
        // (acquire_lock already stamped both) but no rebuild has happened in
        // the new epoch yet.
        state.pid = 222;
        state.epoch_nonce = "epoch-b".to_string();
        write_state(root, &state).unwrap();

        assert_eq!(
            read_state(root).ignore_rebuilds,
            3,
            "lifetime total must still be on disk — this is not a wipe"
        );
        let payload = status_json(root).unwrap();
        assert_eq!(
            payload["ignore_rebuilds"], 3,
            "status --json lifetime total must survive the stale-epoch window"
        );
        // The `--json` seam needs its own assert, not just the lifetime one:
        // `status_report` and `status_json` are two independent readers of
        // `epoch_ignore_rebuilds`, and the gate review proved that neutering
        // ONLY `status_json` back to the raw field survived the whole suite.
        // Correct behavior was live-observed; nothing pinned it, so a refactor
        // could have silently reverted this seam alone.
        assert_eq!(
            payload["epoch_ignore_rebuilds"], 0,
            "status --json must not attribute the dead epoch's reloads to the live one"
        );

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            !out.contains("ignore:"),
            "no rebuild has happened in the NEW epoch (pid 222) yet — the \
             line must be omitted, not attribute pid 111's reloads to it: {out}"
        );

        // Case B: daemon is stopped — release_lock clears BOTH pid and the
        // epoch nonce (its own "no live epoch" sentinel, same shape as
        // pid == 0).
        state.pid = 0;
        state.epoch_nonce = String::new();
        write_state(root, &state).unwrap();
        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            !out.contains("ignore:"),
            "daemon stopped — must not render the last epoch's reload count: {out}"
        );

        // Once the new epoch actually rebuilds, the line must reappear with
        // ONLY the new epoch's count, never the old one. Restart: pid 222
        // again under the same live epoch it was stamped with in Case A
        // (status was merely checked once while stopped in between).
        // Residuals round, Phase 3: this restart is a live daemon again —
        // hold the lock so this presence assertion still discriminates
        // epoch-scoping, not liveness.
        let _guard = hold_daemon_lock(root);
        state.pid = 222;
        state.epoch_nonce = "epoch-b".to_string();
        crate::state::record_ignore_rebuild(&mut state, 6_000);
        write_state(root, &state).unwrap();
        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains("reloaded 1 time(s)"),
            "expected the new epoch's own count (1): {out}"
        );
        assert!(
            !out.contains("reloaded 3 time(s)") && !out.contains("reloaded 4 time(s)"),
            "must never blend in the old epoch's count: {out}"
        );
    }

    // Honesty round: epoch identity was previously the pid, and pid reuse
    // (real on a long-lived machine — the same recycling class `doctor`'s
    // daemon-liveness check already handles via flock, not pid comparison)
    // let a later daemon epoch inherit a dead epoch's stale reload count.
    // Simulates a dead epoch (pid 111, nonce "epoch-a", 3 rebuilds) followed
    // by a new epoch that reuses pid 111 but is stamped with a fresh nonce
    // ("epoch-b") — the case pid alone cannot distinguish. Neuter: key
    // either side (`record_ignore_rebuild` or `current_epoch_reloads`) back
    // on `pid` instead of the nonce → RED (the reused pid makes the new
    // epoch look like a continuation of the old one).
    #[test]
    fn pid_reuse_does_not_resurrect_a_dead_epoch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        // Dead epoch: pid 111, nonce "epoch-a", rebuilt 3 times.
        let mut state = crate::state::State {
            pid: 111,
            epoch_nonce: "epoch-a".to_string(),
            ..crate::state::State::default()
        };
        crate::state::record_ignore_rebuild(&mut state, 1_000);
        crate::state::record_ignore_rebuild(&mut state, 2_000);
        crate::state::record_ignore_rebuild(&mut state, 3_000);
        assert_eq!(state.epoch_ignore_rebuilds, 3);

        // New epoch REUSES the same pid (111) — pid wraparound — but
        // acquire_lock stamps a fresh, distinct nonce.
        state.epoch_nonce = "epoch-b".to_string();
        write_state(root, &state).unwrap();

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            !out.contains("ignore:"),
            "a new epoch that reused the dead epoch's pid must render zero reloads until its \
             own first rebuild, not the dead epoch's 3: {out}"
        );

        // Its own first rebuild must start counting from 1, not accumulate
        // onto the dead epoch's 3.
        // Residuals round, Phase 3: this rebuild belongs to a live epoch —
        // hold the lock so the presence assertion below discriminates
        // pid-reuse-vs-nonce, not liveness.
        let _guard = hold_daemon_lock(root);
        let mut state = read_state(root);
        crate::state::record_ignore_rebuild(&mut state, 4_000);
        write_state(root, &state).unwrap();
        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains("reloaded 1 time(s)"),
            "the reused-pid epoch's first rebuild must read 1, not accumulate onto the dead \
             epoch's count: {out}"
        );
        assert!(
            !out.contains("reloaded 4 time(s)"),
            "must never blend the dead epoch's 3 into the new epoch's count: {out}"
        );
    }

    // Honesty round: reader (`current_epoch_reloads`) and writer
    // (`record_ignore_rebuild`) must agree on what counts as "a new epoch"
    // using the SAME identity (the nonce) — whether or not the pid also
    // happened to change alongside it. Two fixtures that differ only in
    // whether pid changed must render byte-identical `status` output.
    // Neuter: make one side compare `pid` and the other compare
    // `epoch_nonce` (i.e. revert just one of the two functions) → RED, both
    // because the two scenarios stop matching each other and because the
    // "no rebuild yet in the new epoch" assertion fails.
    #[test]
    fn epoch_detection_is_symmetric_regardless_of_whether_pid_also_changed() {
        // Scenario 1: nonce changes, pid does NOT (pid reuse).
        let tmp1 = tempfile::tempdir().unwrap();
        let root1 = tmp1.path();
        std::fs::create_dir_all(objects_dir(root1)).unwrap();
        let mut s1 = crate::state::State {
            pid: 111,
            epoch_nonce: "epoch-a".to_string(),
            ..crate::state::State::default()
        };
        crate::state::record_ignore_rebuild(&mut s1, 1_000);
        s1.epoch_nonce = "epoch-b".to_string(); // new epoch, pid unchanged
        write_state(root1, &s1).unwrap();
        let out1 = status_report(root1, agentrec_core::MAX_STORE_BYTES).unwrap();

        // Scenario 2: both nonce and pid change (the ordinary restart case).
        let tmp2 = tempfile::tempdir().unwrap();
        let root2 = tmp2.path();
        std::fs::create_dir_all(objects_dir(root2)).unwrap();
        let mut s2 = crate::state::State {
            pid: 111,
            epoch_nonce: "epoch-a".to_string(),
            ..crate::state::State::default()
        };
        crate::state::record_ignore_rebuild(&mut s2, 1_000);
        s2.pid = 222;
        s2.epoch_nonce = "epoch-b".to_string(); // new epoch, pid also changed
        write_state(root2, &s2).unwrap();
        let out2 = status_report(root2, agentrec_core::MAX_STORE_BYTES).unwrap();

        assert_eq!(
            out1, out2,
            "epoch detection must depend only on the nonce — whether pid also happened to \
             change must not change the rendered output"
        );
        assert!(
            !out1.contains("ignore:"),
            "a brand-new epoch nonce with no rebuild yet must render nothing: {out1}"
        );
    }

    // Honesty round, AC #3: a `state.json` written by a binary that predates
    // the nonce field entirely has neither `epoch_nonce` nor
    // `epoch_reload_nonce` — both deserialize to their shared default `""`.
    // A reader that treated that coincidental match as "current epoch" would
    // resurrect whatever `epoch_ignore_rebuilds` figure an OLD, pid-keyed
    // binary had accumulated. Neuter: drop the `!epoch_nonce.is_empty()`
    // guard in `current_epoch_reloads` (treat empty-equals-empty as a match)
    // → RED.
    #[test]
    fn pre_nonce_state_json_renders_no_reload_line() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        // Simulate a state.json written by an OLDER binary: it has
        // accumulated real lifetime + "epoch" history under its old
        // pid-keyed bookkeeping (the now-unused `epoch_pid` key included, to
        // prove it's simply ignored), but no `epoch_nonce` /
        // `epoch_reload_nonce` keys at all — those fields didn't exist yet.
        std::fs::write(
            crate::state_path(root),
            r#"{"pid":111,"ignore_rebuilds":4821,"epoch_ignore_rebuilds":37,
                "epoch_pid":111,"last_ignore_rebuild_ms":123456}"#,
        )
        .unwrap();

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            !out.contains("ignore:"),
            "a pre-nonce state.json must not claim the old epoch's count is current: {out}"
        );

        let payload = status_json(root).unwrap();
        assert_eq!(
            payload["ignore_rebuilds"], 4821,
            "the lifetime total must still survive"
        );
        assert_eq!(
            payload["epoch_ignore_rebuilds"], 0,
            "must not resurrect the pre-nonce file's stale epoch count"
        );
        // AC3.2: this fixture also predates the perf-evidence round's two
        // dedup fields entirely (neither key is in the literal JSON above) —
        // per-field `#[serde(default)]` must render 0 for both without
        // resetting any sibling (proven above: `ignore_rebuilds` survives at
        // its real value, not 0 — a struct-level reset would have taken
        // these down together).
        assert_eq!(
            payload["dedup_hits"], 0,
            "a pre-instrumentation state.json must render 0, not resurrect garbage"
        );
        assert_eq!(payload["dedup_reread_bytes"], 0);
    }

    // Phase 3: `status --json` must carry every field the text DEGRADED
    // banner reports, not just the ignore-reload counters — a monitoring
    // script piping `--json` to `jq` must never see less than a human running
    // bare `status` sees. Neuter: drop one field from the payload's
    // construction → this loop's named assert fires for exactly that field.
    #[test]
    fn status_json_carries_degraded_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        let mut state = crate::state::State::default();
        crate::state::record_io_failure(&mut state, "src/a.rs");
        crate::state::record_prompt_put_failure(&mut state);
        state.state_parse_failures = 3;
        state.last_bad_field = Some("signal_offset".to_string());
        // Perf-evidence round (AC3.2): the dedup-hit counters must ride
        // along with every other DEGRADED/operational field this payload
        // carries — a monitoring script must see them too, not just a
        // human running `agentrec doctor`.
        state.dedup_hits = 5;
        state.dedup_reread_bytes = 4096;
        write_state(root, &state).unwrap();

        let payload = status_json(root).unwrap();
        for field in [
            "snapshot_failures",
            "prompt_put_failures",
            "io_failed",
            "state_parse_failures",
            "dedup_hits",
            "dedup_reread_bytes",
        ] {
            assert!(
                payload.get(field).is_some(),
                "status --json missing field: {field} (payload: {payload})"
            );
        }
        assert_eq!(payload["snapshot_failures"], 1);
        assert_eq!(payload["prompt_put_failures"], 1);
        assert_eq!(payload["io_failed"], serde_json::json!(["src/a.rs"]));
        assert_eq!(payload["state_parse_failures"], 3);
        assert_eq!(payload["dedup_hits"], 5);
        assert_eq!(payload["dedup_reread_bytes"], 4096);
    }

    // Bare `status` output for a healthy store must be unchanged by this
    // phase — three renderers (text, --json, --ack-degraded --json) now read
    // the same `State`, so pin the plain case to catch any of them drifting
    // it.
    #[test]
    fn status_healthy_store_output_is_pinned() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert_eq!(
            out,
            // F26: the `store:` line gained its second figure. Both read 0 B
            // on an empty store — rendered anyway, on the same "a measurement
            // is a fact about accounting" ground as the `inbox:` line, so the
            // line's shape does not change with the data.
            "store:      0 B on disk, 0 B counted toward the budget\n\
             inbox:      0 B signal.jsonl\n\
             turns:      0 (agent turns; git activity hidden)\n\
             gaps:       0 recording gap(s)\n\
             daemon:     not running\n\
             \x20 ⚠ recorder not running — nothing is being recorded (run `agentrec record`)\n\
             rich-rate:  n/a (no agent turns yet)\n\
             memory:     0 fresh, 0 stale, 0 rejects, 0 injections, 0 failures\n",
            "healthy-store status output must be unchanged: {out}"
        );
    }

    /// F31, stated as the asymmetry that motivates the line: this fixture is
    /// a repo where **nothing is recording** — no daemon holds the lock —
    /// and every other line of the report is a clean bill of health,
    /// `gaps: 0` included (there are no epoch records, so there is no
    /// uncovered interval to name; the report cannot infer non-recording
    /// from a ledger that was never written to). Before F31 that output had
    /// no way to say so. Probed, not asserted from reasoning: the pinned
    /// bytes above are what `status_report` actually emits for this fixture.
    ///
    /// Neuter (both directions): drop the `daemon:` line → RED here and in
    /// the pinned test above; render it unconditionally as "running" → RED
    /// on the dead half; render it unconditionally as "not running" → RED on
    /// the live half.
    #[test]
    fn status_reports_daemon_liveness_both_ways() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        let dead = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            dead.contains("daemon:     not running\n"),
            "a repo with no recorder must say so: {dead}"
        );
        assert!(
            dead.contains("⚠ recorder not running — nothing is being recorded"),
            "the dead case must warn, not merely state: {dead}"
        );

        // Same real `libc::flock` probe `doctor`/`purge` use — `status` must
        // not have grown a second liveness notion that can disagree with
        // theirs.
        let _guard = hold_daemon_lock(root);
        let live = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            live.contains("daemon:     running\n"),
            "a held daemon.lock must read as running: {live}"
        );
        assert!(
            !live.contains("⚠ recorder not running"),
            "a live daemon must not carry the dead-recorder warning: {live}"
        );
    }

    /// F13: `status` rendered `health.crash_gaps`, so the `Restart` and
    /// `TrailingStop` intervals `view::recording_gaps` already tagged were
    /// reported as no gap at all — "kill the daemon, damage happens, restart"
    /// read `gaps: 0`. This ledger holds 1 crash + 1 restart + 1 trailing;
    /// the crash-only figure would be 1.
    ///
    /// The three kinds are asserted separately AND the total is asserted, so
    /// a renderer that sums them into an opaque number, or one that keeps
    /// rendering only the crash count, both go RED. Neuter: restore
    /// `let gaps = health.crash_gaps;` → RED (total reads 1, breakdown gone).
    #[test]
    fn status_counts_restart_and_trailing_stop_gaps() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        for (event, ts) in [
            ("start", "2026-01-01T00:00:00.000Z"),
            ("start", "2026-01-01T01:00:00.000Z"), // crash
            ("stop", "2026-01-01T02:00:00.000Z"),
            ("start", "2026-01-01T03:00:00.000Z"), // restart
            ("stop", "2026-01-01T04:00:00.000Z"),  // trailing
        ] {
            append_log(
                &log_path(root),
                &LogRecord::Epoch(agentrec_core::record::EpochRecord {
                    v: 1,
                    event: event.to_string(),
                    ts: ts.to_string(),
                }),
            )
            .unwrap();
        }

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains(
                "gaps:       3 recording gap(s) (1 crash, 1 restart, 1 since last stop)\n"
            ),
            "every uncovered-interval kind must be counted and stay \
             distinguishable: {out}"
        );

        // The pre-F13 rendering, pinned as the thing that must NOT come back.
        assert!(
            !out.contains("gaps:       1 recording gap(s)"),
            "crash-only gap reporting must not survive: {out}"
        );
    }

    /// The zero case keeps its exact pre-F13 bytes: with no gaps at all there
    /// is nothing to break down, and "(0 crash, 0 restart, 0 since last
    /// stop)" would be the vacuous line the zero-turn `rich-rate: n/a`
    /// precedent (D-PD3) refuses. Guards against the breakdown being made
    /// unconditional later.
    #[test]
    fn status_gap_breakdown_is_omitted_at_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        // A single open `start` is coverage, not a gap (see
        // `view::recording_gaps`).
        append_log(
            &log_path(root),
            &LogRecord::Epoch(agentrec_core::record::EpochRecord {
                v: 1,
                event: "start".to_string(),
                ts: "2026-01-01T00:00:00.000Z".to_string(),
            }),
        )
        .unwrap();

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains("gaps:       0 recording gap(s)\n"),
            "no gaps must render bare, with no breakdown: {out}"
        );
    }

    /// F13's JSON half: `crash_gaps` keeps its established meaning and value
    /// (a consumer already reading it sees no change), and the two other
    /// kinds arrive as additive siblings rather than being folded into it.
    /// Vacuity guard: distinct counts per kind (2/1/1).
    #[test]
    fn status_health_carries_every_gap_kind_separately() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        for (event, ts) in [
            ("start", "2026-01-01T00:00:00.000Z"),
            ("start", "2026-01-01T01:00:00.000Z"), // crash
            ("start", "2026-01-01T02:00:00.000Z"), // crash
            ("stop", "2026-01-01T03:00:00.000Z"),
            ("start", "2026-01-01T04:00:00.000Z"), // restart
            ("stop", "2026-01-01T05:00:00.000Z"),  // trailing
        ] {
            append_log(
                &log_path(root),
                &LogRecord::Epoch(agentrec_core::record::EpochRecord {
                    v: 1,
                    event: event.to_string(),
                    ts: ts.to_string(),
                }),
            )
            .unwrap();
        }

        let payload = status_json(root).unwrap();
        assert_eq!(payload["crash_gaps"], 2, "{payload}");
        assert_eq!(payload["restart_gaps"], 1, "{payload}");
        assert_eq!(payload["trailing_stop_gaps"], 1, "{payload}");
    }

    // AC3.2 (T3/D48): the hook inbox is the file that actually grew on the
    // dogfood store (13.4 MB vs. 2.5 MB of log.jsonl) and `status` never
    // accounted for it. The line must report the real byte size AND attribute
    // the already-consumed share to its reclaim command — the same honest
    // attribution shape the over-budget orphan clause uses. Neuter: drop the
    // inbox line → RED here and in the pinned test above.
    #[test]
    fn status_reports_signal_inbox_size_and_consumed_share() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let body = vec![b'x'; 4096];
        std::fs::write(signal_path(root), &body).unwrap();
        crate::state::write_state(
            root,
            &crate::state::State {
                signal_offset: 3072,
                ..Default::default()
            },
        )
        .unwrap();

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains("inbox:      4.0 KiB signal.jsonl (3.0 KiB consumed — reclaim with `agentrec purge --signals-consumed`)\n"),
            "inbox line must carry size + consumed share + remedy: {out}"
        );
    }

    // A stale/corrupt `signal_offset` past EOF must never make `status` claim
    // more consumed bytes than the file holds — that inconsistency is
    // `purge --signals-consumed`'s to refuse on, not `status`' to render as
    // fact. Neuter: drop the `.min(signal_bytes)` clamp → RED (renders
    // "9.8 KiB consumed" of a 100 B file).
    #[test]
    fn status_clamps_a_signal_offset_past_eof() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        std::fs::write(signal_path(root), vec![b'x'; 100]).unwrap();
        crate::state::write_state(
            root,
            &crate::state::State {
                signal_offset: 10_000,
                ..Default::default()
            },
        )
        .unwrap();

        let out = status_report(root, agentrec_core::MAX_STORE_BYTES).unwrap();
        assert!(
            out.contains("inbox:      100 B signal.jsonl (100 B consumed"),
            "consumed must be clamped to the real file size: {out}"
        );
        let json = status_json(root).unwrap();
        assert_eq!(json["signal_bytes"], 100);
        assert_eq!(json["signal_consumed_bytes"], 100);
    }

    // AC3.2, `--json` leg: both fields are always present (never
    // only-when-nonzero), so a monitoring script can tell "zero" from "older
    // binary without the field".
    #[test]
    fn status_json_carries_signal_inbox_fields_even_at_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();

        let json = status_json(root).unwrap();
        assert_eq!(json["signal_bytes"], 0, "field present at zero");
        assert_eq!(json["signal_consumed_bytes"], 0, "field present at zero");

        std::fs::write(signal_path(root), vec![b'x'; 2048]).unwrap();
        crate::state::write_state(
            root,
            &crate::state::State {
                signal_offset: 1024,
                ..Default::default()
            },
        )
        .unwrap();
        let json = status_json(root).unwrap();
        assert_eq!(json["signal_bytes"], 2048);
        assert_eq!(json["signal_consumed_bytes"], 1024);
    }

    // ---- Phase 1: enforce_budget's extra_protected wiring -----------------
    //
    // Backdates a blob's mtime well before `enforce_budget`'s internal
    // `pass_start` so the pre-existing A3(c) freshness guard cannot rescue
    // it vacuously — these fixtures must genuinely be old, evictable
    // candidates that only survive because of the Phase 1 protect-set.
    fn backdate(objects_dir: &std::path::Path, hash: &str, secs_ago: u64) {
        let hex = hash.strip_prefix("sha256:").unwrap();
        let path = objects_dir.join(&hex[..2]).join(&hex[2..]);
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(past)
            .unwrap();
    }

    fn owned_turns(root: &Path) -> Vec<TurnRecord> {
        agentrec_core::record::load_log(&log_path(root))
            .into_iter()
            .filter_map(|r| match r {
                LogRecord::Turn(t) => Some(t),
                LogRecord::Epoch(_) => None,
            })
            .collect()
    }

    // A blob cited only by the in-flight turn's crash journal (`open.json`)
    // and one otherwise-evictable OLD turn must survive: `open.json`'s
    // `before` for a file is exactly the prior committed turn's `after` for
    // that same file, so a live daemon's in-flight state and an "old"
    // budget-eviction candidate are frequently the SAME blob. Neuter: drop
    // `open.json` from `purgecmd::referenced_hashes`'s scanned paths.
    #[test]
    fn eviction_keeps_open_turn_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let old = store.put(&[0xAAu8; 500]).unwrap();
        backdate(&objects_dir(root), &old, 3600);
        let new = store.put(&[0xCCu8; 5]).unwrap();

        append_log(
            &log_path(root),
            &LogRecord::Turn(turn_with_snapshot(
                "t_OPENOLD00000000000000001",
                "old.bin",
                &old,
            )),
        )
        .unwrap();
        append_log(
            &log_path(root),
            &LogRecord::Turn(turn_with_snapshot(
                "t_OPENNEW00000000000000001",
                "new.bin",
                &new,
            )),
        )
        .unwrap();

        // Simulates the daemon's crash journal: the in-flight open turn's
        // `before` cites the same blob. Raw text is enough here —
        // `referenced_hashes` scans for `sha256:<hex>` regardless of JSON
        // shape.
        std::fs::write(crate::open_path(root), format!(r#"{{"before":"{old}"}}"#)).unwrap();

        let extra_protected = extra_protected_refs(root);
        let evicted = agentrec_core::retention::enforce_budget(
            &store,
            &owned_turns(root),
            5,
            &extra_protected,
        );

        assert!(
            store.contains(&old),
            "blob cited by the in-flight turn must survive"
        );
        assert!(store.contains(&new));
        assert_eq!(evicted.bytes, 0, "nothing freed — old blob was protected");
    }

    // A blob cited only by a `memory.jsonl` pin (which `verify`'s pin-diff
    // resolves as a CAS blob to render old content) must survive the same
    // way. Neuter: drop `memory.jsonl` from the scanned paths.
    #[test]
    fn eviction_keeps_pinned_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let old = store.put(&[0xBBu8; 500]).unwrap();
        backdate(&objects_dir(root), &old, 3600);
        let new = store.put(&[0xCCu8; 5]).unwrap();

        append_log(
            &log_path(root),
            &LogRecord::Turn(turn_with_snapshot(
                "t_PINOLD00000000000000001",
                "old.bin",
                &old,
            )),
        )
        .unwrap();
        append_log(
            &log_path(root),
            &LogRecord::Turn(turn_with_snapshot(
                "t_PINNEW00000000000000001",
                "new.bin",
                &new,
            )),
        )
        .unwrap();

        // A real, parseable memory pin citing `old` — exactly the shape
        // `verify`'s pin-diff resolves.
        agentrec_core::memory::append_memory(
            root,
            &agentrec_core::memory::MemoryRecord {
                v: 1,
                kind: "memory".into(),
                id: "mem_TESTPIN000000000000001".into(),
                op: agentrec_core::memory::MemoryOp::Assert,
                fact: "test pinned fact".into(),
                pins: vec![agentrec_core::memory::Pin {
                    path: "old.bin".into(),
                    hash: old.clone(),
                }],
                source_turns: vec![],
                origin: "human".into(),
                ts: 0,
                reason: None,
            },
        )
        .unwrap();

        let extra_protected = extra_protected_refs(root);
        let evicted = agentrec_core::retention::enforce_budget(
            &store,
            &owned_turns(root),
            5,
            &extra_protected,
        );

        assert!(store.contains(&old), "pinned blob must survive");
        assert!(store.contains(&new));
        assert_eq!(
            evicted.bytes, 0,
            "nothing freed — pinned blob was protected"
        );
    }

    // A blob cited only by a torn/unparseable line survives too — in
    // log.jsonl (a corrupted committed-turn line `load_log` silently drops)
    // and, separately, in a truncated open.json (which `recover_orphan`
    // itself already treats as discardable, per its own comment). This is
    // the criterion that lifts eviction to purge's own guarantee: the
    // protect-set must come from the RAW scan, never from re-parsing.
    // Neuter: build the protect-set from `load_log`/serde instead of the
    // raw scan (both for log.jsonl and open.json).
    #[test]
    fn eviction_keeps_torn_line_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        // Sub-case 1: a torn log.jsonl line.
        let torn_log = store.put(&[0x11u8; 500]).unwrap();
        backdate(&objects_dir(root), &torn_log, 3600);
        // Sub-case 2: a truncated open.json.
        let torn_open = store.put(&[0x22u8; 500]).unwrap();
        backdate(&objects_dir(root), &torn_open, 3600);
        let new = store.put(&[0xCCu8; 5]).unwrap();

        // Each torn blob is ALSO a normal, structurally-visible eviction
        // candidate via one old committed turn — proving survival is due to
        // the torn-line/open.json ref, not ordinary A5/A2 protection.
        append_log(
            &log_path(root),
            &LogRecord::Turn(turn_with_snapshot(
                "t_TORNLOG0000000000000001",
                "log.bin",
                &torn_log,
            )),
        )
        .unwrap();
        append_log(
            &log_path(root),
            &LogRecord::Turn(turn_with_snapshot(
                "t_TORNOPEN00000000000001",
                "open.bin",
                &torn_open,
            )),
        )
        .unwrap();
        append_log(
            &log_path(root),
            &LogRecord::Turn(turn_with_snapshot(
                "t_TORNNEW0000000000000001",
                "new.bin",
                &new,
            )),
        )
        .unwrap();

        // A truncated mid-JSON line, invalid on its own — `load_log` skips
        // it — but carrying a valid `sha256:<64hex>` the raw scan finds.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(log_path(root))
            .unwrap();
        use std::io::Write;
        writeln!(
            f,
            r#"{{"type":"turn","id":"t_TORN","files":[{{"before":"{torn_log}"#
        )
        .unwrap();

        // A truncated open.json — not valid `OrphanJournal` JSON, but still
        // carrying the hash.
        std::fs::write(
            crate::open_path(root),
            format!(r#"{{"files":[{{"before":"{torn_open}"#),
        )
        .unwrap();

        let extra_protected = extra_protected_refs(root);
        let evicted = agentrec_core::retention::enforce_budget(
            &store,
            &owned_turns(root),
            5,
            &extra_protected,
        );

        assert!(
            store.contains(&torn_log),
            "blob cited only by a torn log.jsonl line must survive"
        );
        assert!(
            store.contains(&torn_open),
            "blob cited only by a truncated open.json must survive"
        );
        assert!(store.contains(&new));
        assert_eq!(
            evicted.bytes, 0,
            "nothing freed — both torn-cited blobs were protected"
        );
    }

    // Over-budget status where EVERY eviction candidate is protected must
    // explain why 0 bytes were freed, naming pinned/in-flight refs — "over
    // budget, 0 freed" with no explanation is the exact dishonest-status
    // shape an earlier round fixed for the orphan-bloat case. Neuter: delete
    // the attribution clause.
    #[test]
    fn status_attributes_protected_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let old = store.put(&[0xAAu8; 800]).unwrap();
        backdate(&objects_dir(root), &old, 3600);

        let turn = turn_with_snapshot("t_ALLPROTECTED0000000000001", "old.bin", &old);
        append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();

        // The sole turn's own blob is protected via A5 (newest-turn), so
        // exercise the in-flight path instead by seeding open.json with the
        // SAME hash after also making it look like a low-budget candidate
        // via a second, newer turn that pushes it out of `keep`.
        let new = store.put(&[0xCCu8; 5]).unwrap();
        let newer = turn_with_snapshot("t_ALLPROTECTEDNEW000000001", "new.bin", &new);
        append_log(&log_path(root), &LogRecord::Turn(newer)).unwrap();
        std::fs::write(crate::open_path(root), format!(r#"{{"before":"{old}"}}"#)).unwrap();

        // Budget small enough that `old` is a candidate.
        let out = status_report(root, 5).unwrap();
        assert!(out.contains("over"), "expected over-budget notice: {out}");
        assert!(
            out.contains("would free 0 B"),
            "expected 0 freed (dry-run wording): {out}"
        );
        assert!(
            out.contains("protected") && (out.contains("pinned") || out.contains("in-flight")),
            "expected a protected-bytes attribution naming pinned/in-flight: {out}"
        );
    }

    /// Recursive snapshot of every regular file under `dir`: (path relative
    /// to `dir`, mtime, byte length). Used by AC2b.1 to prove `status`
    /// mutates nothing under `.agentrec/objects/` — a plain byte-count
    /// comparison wouldn't catch a delete-then-recreate-identical-content
    /// sequence, and a plain existence check wouldn't catch a touched mtime.
    fn snapshot_objects_dir(
        dir: &std::path::Path,
    ) -> Vec<(std::path::PathBuf, std::time::SystemTime, u64)> {
        fn walk(
            dir: &std::path::Path,
            base: &std::path::Path,
            out: &mut Vec<(std::path::PathBuf, std::time::SystemTime, u64)>,
        ) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    walk(&path, base, out);
                } else {
                    let rel = path.strip_prefix(base).unwrap().to_path_buf();
                    out.push((rel, meta.modified().unwrap(), meta.len()));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    // AC2b.1 (perf-evidence round, Phase 2b): `status` on an over-budget
    // store must leave `.agentrec/objects/` byte-, mtime-, and
    // count-identical while STILL printing the over-budget notice AND the
    // protected-bytes clause — eviction moved to the daemon tick; this read
    // verb only reports what a tick would do. Neuter: swap `plan_eviction`
    // back to `enforce_budget` in `status_report` -> this test reds (the
    // snapshot comparison catches the delete; the two content asserts still
    // pass either way, which is why the store-snapshot assert is the one
    // that carries the AC, not the text asserts alone).
    #[test]
    fn status_performs_zero_store_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let old = store.put(&[0xAAu8; 800]).unwrap();
        backdate(&objects_dir(root), &old, 3600);
        let turn = turn_with_snapshot("t_ZEROWRITE0000000000000001", "old.bin", &old);
        append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();

        let new = store.put(&[0xCCu8; 5]).unwrap();
        let newer = turn_with_snapshot("t_ZEROWRITENEW000000000001", "new.bin", &new);
        append_log(&log_path(root), &LogRecord::Turn(newer)).unwrap();

        let before = snapshot_objects_dir(&objects_dir(root));

        // Budget fits only `new` — `old` is a genuine, structurally-visible
        // eviction candidate that a real eviction pass WOULD delete.
        let out = status_report(root, 5).unwrap();
        assert!(out.contains("over"), "expected over-budget notice: {out}");
        assert!(
            out.contains("would free"),
            "expected the dry-run eviction report: {out}"
        );

        let after = snapshot_objects_dir(&objects_dir(root));
        assert_eq!(
            before, after,
            "status must not mutate .agentrec/objects/ at all — a real \
             eviction pass would have deleted `old`, changing this snapshot"
        );
        assert!(store.contains(&old), "old must survive a `status` call");
    }

    // ---- P4b-4 -----------------------------------------------------------

    /// A turn with an arbitrary file list — `turn_with_snapshot`'s
    /// general form, needed here for the fileless git turn and for the
    /// `merges`/`prompt_ref` fields the AC16 fixture turns on.
    fn turn_full(id: &str, snapshot: Option<&str>) -> TurnRecord {
        TurnRecord {
            v: 1,
            id: id.to_string(),
            grade: "rich".into(),
            truncated: false,
            started: "2026-01-01T00:00:00.000Z".into(),
            ended: "2026-01-01T00:00:01.000Z".into(),
            tool: Some("claude".into()),
            model: None,
            session: None,
            root: "/repo".into(),
            prompt_ref: None,
            prompt_excerpt: None,
            merges: vec![],
            imported: None,
            files_complete: None,
            files: snapshot
                .map(|h| {
                    vec![FileEntry {
                        path: format!("{id}.bin"),
                        before: None,
                        after: Some(h.into()),
                        op: "create".into(),
                        skipped: false,
                        withheld: false,
                        baseline_unknown: false,
                        skipped_reason: None,
                        after_synthesized: None,
                        link_kind: None,
                        attribution: None,
                    }]
                })
                .unwrap_or_default(),
        }
    }

    // AC16 — THE DATA-LOSS GATE. `status`'s eviction protect-set must be
    // built from the UNFILTERED turn set. `plan_eviction` derives its
    // eviction `candidates` EXCLUSIVELY from the turns it is passed, so a
    // blob referenced only by an excluded turn is invisible to eviction and
    // survives under both implementations — a fixture like that would
    // discriminate nothing. This one therefore SHARES HASHES across the
    // filter boundary, in both loss classes:
    //
    //   (a) prompt-protection loss — A is both the oldest surviving turn's
    //       snapshot AND the excluded GIT turn's `prompt_ref`. Unfiltered,
    //       `protected_prompts` saves it; filtered, the git turn is gone,
    //       its `prompt_ref` never enters `protected_prompts`, and A is
    //       evicted.
    //   (b) keep-set loss — B is shared between the excluded SUPERSEDED
    //       turn near the tail (inside the keep window when included) and an
    //       older candidate turn. Unfiltered, `keep` holds it via the tail
    //       turn so the candidate walk skips it; filtered, it is absent from
    //       `keep` and is evicted.
    //
    // Every blob is backdated well past `pass_start` so `plan_eviction`'s
    // A3(c) same-pass freshness guard cannot rescue any of them vacuously
    // and mask the difference.
    //
    // Neuter (verified both directions): flip `eviction_plan`'s
    // `include_all: true` to `false` -> A and B become victims,
    // `freed_bytes_projected` jumps 20 -> 70, `status`'s "would free" line
    // changes with it, and `execute` deletes two blobs recorded history
    // still references. Do NOT repair that by widening
    // `extra_protected_refs` — the fix is the unfiltered turn set.
    #[test]
    fn eviction_protect_set_comes_from_the_unfiltered_turn_set() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let a = store.put(&[0xAAu8; 40]).unwrap(); // loss class (a)
        let b = store.put(&[0xBBu8; 30]).unwrap(); // loss class (b)
        let c = store.put(&[0xCCu8; 20]).unwrap(); // the genuine victim
        let n = store.put(&[0xEEu8; 5]).unwrap(); // newest, kept by budget
        for h in [&a, &b, &c, &n] {
            backdate(&objects_dir(root), h, 3600);
        }

        // Oldest first, matching `load_log`/ledger order.
        let t1 = turn_full("t_P4B4OLDA0000000000000001", Some(&a));
        let t2 = turn_full("t_P4B4OLDB0000000000000001", Some(&b));
        let t3 = turn_full("t_P4B4OLDC0000000000000001", Some(&c));
        // The excluded GIT turn: no files of its own, but its prompt hashes
        // to the same content as T1's snapshot.
        let mut git = turn_full("t_P4B4GIT00000000000000001", None);
        git.tool = Some("git".into());
        git.prompt_ref = Some(a.clone());
        // The excluded SUPERSEDED turn, near the tail, re-referencing B.
        let superseded = turn_full("t_P4B4SUPERSEDED0000000001", Some(&b));
        let mut newest = turn_full("t_P4B4NEWEST000000000001", Some(&n));
        newest.merges = vec![superseded.id.clone()];

        for t in [t1, t2, t3, git, superseded, newest] {
            append_log(&log_path(root), &LogRecord::Turn(t)).unwrap();
        }

        // 95 bytes of store against a 45-byte budget. Unfiltered walk:
        // NEW(5) then SUPERSEDED(+30 = 35, still under) fill `keep`; T3 trips
        // the boundary at 55 > 45, so C and A become candidates and B does
        // not. Filtered walk: NEW(5) then T3(+20 = 25) fill `keep`; T2 trips
        // at 55 > 45, so B and A become candidates and C does not.
        let budget = 45;
        let view = agentrec_core::view::RepositoryView::open(root).unwrap();
        let ledger = view.ledger();
        let plan = eviction_plan(root, &store, &view, &ledger, budget).unwrap();

        assert_eq!(
            plan.victims,
            vec![(c.clone(), 20)],
            "only C — the blob no excluded turn protects — may be planned \
             for eviction; A is prompt-protected by the git turn and B is \
             held by the superseded tail turn's keep-set entry"
        );
        assert_eq!(plan.freed_bytes_projected, 20);

        // The production wiring: `status` renders THIS plan's figure.
        let out = status_report(root, budget).unwrap();
        assert!(
            out.contains(&format!("would free {}", human_bytes(20))),
            "status must render the unfiltered plan's projection \
             (filtered would read {}): {out}",
            human_bytes(70)
        );

        // Deletion-level proof. `status` itself never evicts
        // (`status_performs_zero_store_writes`); the daemon tick executes
        // the same plan, so the loss class is observable only by executing
        // it — here, in the test only.
        let evicted = agentrec_core::retention::execute(&store, plan);
        assert_eq!(evicted.count, 1);
        assert_eq!(evicted.bytes, 20);
        assert!(
            store.contains(&a),
            "loss class (a): a blob shared with an excluded git turn's \
             prompt_ref must survive"
        );
        assert!(
            store.contains(&b),
            "loss class (b): a blob shared with an excluded superseded \
             turn's keep-set entry must survive"
        );
        assert!(store.contains(&n));
        assert!(!store.contains(&c), "C is the only genuine victim");
    }

    /// The body text of one top-level `fn` in this file, from its signature
    /// to its closing brace at column 0.
    fn fn_body(name: &str) -> &'static str {
        let src = include_str!("cmds.rs");
        let needle = format!("\nfn {name}(");
        let start = src
            .find(&needle)
            .unwrap_or_else(|| panic!("no `fn {name}(` in cmds.rs"));
        let rest = &src[start + 1..];
        let end = rest.find("\n}\n").expect("unterminated fn");
        &rest[..end]
    }

    // AC5 + the single-parse decision, as a body-text tripwire.
    //
    // AC5's condition is function-scoped ("RED if `status_report` still
    // calls `merged_ids` itself"; `undo`'s surviving caller is out of
    // scope), and the single-ledger-parse decision is likewise a property
    // of these two bodies, not of any output — two parses print the same
    // bytes on a quiescent repo and differ only against a concurrent daemon
    // append, which is not reproducible in a unit test. After the P4b-4
    // rewiring both follow structurally from the signatures: `health_of`,
    // `list_of`, and `list_records_of` all take an already-read `&Ledger`
    // and cannot re-read. This test pins that the bodies keep using those
    // forms.
    //
    // Scoped to LEDGER parses. Two RAW-BYTE re-reads of `log.jsonl` remain
    // in the over-budget path: `eviction_plan` -> `extra_protected_refs`,
    // which harvests hashes off torn lines a structured parse cannot see,
    // and `status_report` -> `purgecmd::orphan_bytes` ->
    // `purgecmd::referenced_hashes`. Both are pre-existing, deliberately
    // late-bound (each narrows the window a live daemon can append an
    // in-flight blob in), and neither is a second interpretation of the
    // ledger, so both are outside what this pins.
    //
    // Honest limitation: a text scan, not a runtime parse counter. It
    // cannot see a re-derivation written with different words. It is a
    // tripwire on the exact regression shapes this task removed — the
    // behavioral cross-check below is what pins the counting itself.
    #[test]
    fn status_derives_its_counts_from_the_seam_and_parses_the_ledger_once() {
        let report = fn_body("status_report");
        let plan = fn_body("eviction_plan");

        for (name, body) in [("status_report", report), ("eviction_plan", plan)] {
            assert!(
                !body.contains("merged_ids("),
                "{name} must not re-derive the superseded filter — that IS \
                 `TurnQuery {{ include_all: false }}`"
            );
            assert!(
                !body.contains(r#"Some("git")"#),
                "{name} must not re-derive the git-hidden filter"
            );
            assert!(
                !body.contains("load_log("),
                "{name} must read through the view, not the record loader"
            );
            // The ledger-taking forms only; the `&self`-reading twins
            // (`health`, `list`, `list_records`) would each add a parse.
            for reader in [".health(", ".list(", ".list_records("] {
                assert!(
                    !body.contains(reader),
                    "{name} must not call `{reader}` — it re-parses the ledger"
                );
            }
        }
        assert_eq!(
            report.matches(".ledger()").count(),
            1,
            "status_report must parse the LEDGER exactly once"
        );
        assert_eq!(
            plan.matches(".ledger()").count(),
            0,
            "eviction_plan must consume status_report's ledger, not read its own"
        );
    }

    // AC5's behavioral half: the counting surface's three figures come from
    // the summary projection. Fixture carries one of every class the filter
    // decides on — a plain rich turn, a bare turn, an IMPORTED rich turn
    // (counted in `turns:` but excluded from the rich-rate window), a GIT
    // turn (hidden), and a SUPERSEDED turn (hidden) — so a re-derived
    // filter that disagreed with `TurnQuery { include_all: false }` on any
    // one of them would show up here.
    #[test]
    fn status_counting_surface_matches_the_summary_projection() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();

        let mut rich = turn_full("t_P4B4COUNTRICH0000000001", None);
        rich.grade = "rich".into();
        let mut bare = turn_full("t_P4B4COUNTBARE0000000001", None);
        bare.grade = "bare".into();
        bare.tool = None;
        let mut imported = turn_full("t_P4B4COUNTIMPORTED000001", None);
        imported.imported = Some(true);
        let mut git = turn_full("t_P4B4COUNTGIT00000000001", None);
        git.tool = Some("git".into());
        let superseded = turn_full("t_P4B4COUNTSUPERSEDED0001", None);
        let mut newest = turn_full("t_P4B4COUNTNEWEST00000001", None);
        newest.merges = vec![superseded.id.clone()];

        for t in [rich, bare, imported, git, superseded, newest] {
            append_log(&log_path(root), &LogRecord::Turn(t)).unwrap();
        }

        let view = agentrec_core::view::RepositoryView::open(root).unwrap();
        let page = view
            .list_of(
                &view.ledger(),
                &agentrec_core::view::TurnQuery {
                    include_all: false,
                    limit: None,
                    after: None,
                },
            )
            .unwrap();
        // 6 turns recorded, 2 hidden (git + superseded).
        assert_eq!(page.items.len(), 4, "fixture precondition");

        let out = status_report(root, 1_000_000).unwrap();
        assert!(
            out.contains(&format!(
                "turns:      {} (agent turns; git activity hidden)",
                page.items.len()
            )),
            "turn count must be the projection's page length: {out}"
        );
        let non_imported = page.items.iter().filter(|t| !t.imported).count();
        assert_eq!(non_imported, 3, "fixture precondition");
        assert!(
            out.contains(&format!("over trailing {non_imported} agent turn(s)")),
            "the rich-rate window must be the projection's non-imported \
             members: {out}"
        );
        let rich_n = page
            .items
            .iter()
            .filter(|t| !t.imported && t.grade == "rich")
            .count();
        assert!(
            out.contains(&format!(
                "rich-rate:  {:.0}%",
                rich_n as f64 / non_imported as f64 * 100.0
            )),
            "rich-rate numerator must come from the projection's grades: {out}"
        );
    }

    // AC2b.3: with no daemon running (the common `hold_daemon_lock` probe
    // this repo already uses to fake liveness — never held here), an
    // over-budget `status` must say so explicitly rather than leaving a
    // human to assume eviction is happening. Discriminated against the
    // opposite case (lock held) so this isn't just "the line always
    // prints" — it must NOT print once a daemon is live.
    #[test]
    fn status_reports_no_evictor_when_daemon_down() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));
        let hash = store.put(&[0u8; 5_000]).unwrap();
        let turn = turn_with_snapshot("t_NOEVICTOR0000000000000001", "big.bin", &hash);
        append_log(&log_path(root), &LogRecord::Turn(turn)).unwrap();

        let out_down = status_report(root, 1_000).unwrap();
        assert!(
            out_down.contains("nothing is evicting"),
            "expected the daemon-down evictor notice: {out_down}"
        );
        assert!(
            store.contains(&hash),
            "status must delete nothing either way"
        );

        let _lock = hold_daemon_lock(root);
        let out_live = status_report(root, 1_000).unwrap();
        assert!(
            !out_live.contains("nothing is evicting"),
            "a live daemon must not get the down-evictor notice: {out_live}"
        );
    }

    // AC2b.4: `extra_protected_refs` is a side-effect-free reader (three
    // `fs::read_to_string` calls, cmds.rs:558-568) — it cannot be fed
    // in-memory inputs, so this unit test seeds a REAL `open.json` on disk
    // with NO daemon running and drives the harvested set straight into
    // `plan_eviction`, proving the harvest -> plan wiring at the level
    // `open.json`'s protection now lives at. `open.json` specifically
    // cannot be proven at the live-daemon level (B9: the daemon's own
    // `sync_journal` idle arm deletes it within ~250ms of there being no
    // open turn) — `daemon_eviction_keeps_protected_refs`
    // (`cli/tests/integration.rs`) covers the torn-log-line and
    // memory.jsonl-pin channels there instead.
    #[test]
    fn harvest_protects_open_json_refs_at_plan_level() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(objects_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let old = store.put(&[0xAAu8; 500]).unwrap();
        backdate(&objects_dir(root), &old, 3600);
        let new = store.put(&[0xCCu8; 5]).unwrap();

        // Entries handed directly to `plan_eviction` — only `open.json`
        // needs to be a real on-disk file for this test's claim.
        let entries = vec![
            turn_with_snapshot("t_OPENPLAN0000000000000001", "old.bin", &old),
            turn_with_snapshot("t_OPENPLANNEW00000000000001", "new.bin", &new),
        ];

        std::fs::write(crate::open_path(root), format!(r#"{{"before":"{old}"}}"#)).unwrap();

        let extra_protected = extra_protected_refs(root);
        assert!(
            extra_protected.contains(&old),
            "extra_protected_refs must harvest open.json's hash"
        );

        let plan = agentrec_core::retention::plan_eviction(&store, &entries, 5, &extra_protected);
        assert!(
            !plan.victims.iter().any(|(h, _)| h == &old),
            "plan_eviction must not list the open.json-protected blob as a victim: {:?}",
            plan.victims
        );
        assert_eq!(
            plan.protected_bytes, 500,
            "protected_bytes must attribute the open.json-protected blob's size"
        );
    }
}
