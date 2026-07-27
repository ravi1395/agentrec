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
    let records = agentrec_core::record::load_log(&log_path(root));
    let superseded = merged_ids(&records);

    let turns: Vec<&TurnRecord> = records
        .iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        })
        .filter(|t| all || !superseded.contains(&t.id))
        .filter(|t| all || t.tool.as_deref() != Some("git"))
        .collect();

    if turns.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("no turns recorded — is `agentrec record` running?");
        }
        return Ok(());
    }

    let now_ms = wall_now_ms();
    let color = fmt::should_color(
        std::io::stdout().is_terminal(),
        std::env::var_os("NO_COLOR").is_some(),
    );

    // NF1: absent/empty `noise_globs` yields `None` here, so every branch
    // below that consults `noise_matcher` behaves exactly as it did before
    // this feature existed — no separate "is the feature configured" flag
    // needed anywhere else in this function.
    let noise_globs = crate::noise::read_noise_globs(root);
    let noise_matcher = crate::noise::NoiseMatcher::build(root, &noise_globs);

    // Rendered lines (non-JSON only) are accumulated so `--explain` can scan
    // exactly what this invocation printed, not every term that ever exists.
    let mut rendered = String::new();

    // Newest first; the log is append-order (oldest first).
    for turn in turns.iter().rev().take(limit) {
        if json {
            let line = serde_json::to_string(turn).map_err(|e| e.to_string())?;
            println!("{line}");
        } else {
            // NF-B/NF-D.4: fold counts a matched entry out of the visible
            // count regardless of the turn's grade/tool — a turn whose
            // entries are ALL noise still prints its own list line (turn
            // selection above is untouched) plus this fold line, never
            // silently disappears.
            let noise_n = if all_files {
                0
            } else {
                noise_matcher
                    .as_ref()
                    .map(|m| turn.files.iter().filter(|f| m.is_noise(&f.path)).count())
                    .unwrap_or(0)
            };
            let visible_files = turn.files.len() - noise_n;
            let line = format_turn(turn, now_ms, utc, color, visible_files);
            println!("{line}");
            rendered.push_str(&line);
            rendered.push('\n');
            if noise_n > 0 {
                let fold_line = format!("+{noise_n} noise files (--all-files to show)");
                println!("{fold_line}");
                rendered.push_str(&fold_line);
                rendered.push('\n');
            }
        }
    }

    if explain && !json {
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

/// `status`: store size, recording gaps, and rich-rate (the health stat that
/// catches silently broken hooks). `ack_degraded` clears a prior DEGRADED
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
    print!("{}", status_report(root, effective_store_budget())?);
    Ok(())
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
fn status_json(root: &Path) -> Result<serde_json::Value, String> {
    let state = read_state(root);
    let daemon_live = crate::daemon::daemon_is_running(root);
    Ok(serde_json::json!({
        "ignore_rebuilds": state.ignore_rebuilds,
        "epoch_ignore_rebuilds": current_epoch_reloads(&state),
        "epoch_ignore_rebuilds_stale": !daemon_live,
        "last_ignore_rebuild_ms": if state.ignore_rebuilds > 0 {
            Some(state.last_ignore_rebuild_ms)
        } else {
            None
        },
        "snapshot_failures": state.snapshot_failures,
        "io_failed": state.io_failed,
        "prompt_put_failures": state.prompt_put_failures,
        "state_parse_failures": state.state_parse_failures,
        "last_bad_field": state.last_bad_field,
    }))
}

/// Builds `status`'s full output as a string (split out from [`status`] so
/// the over-budget eviction path is unit-testable with a tiny injected
/// `budget`, instead of requiring a real 2 GiB store — AC I+).
fn status_report(root: &Path, budget: u64) -> Result<String, String> {
    let records = agentrec_core::record::load_log(&log_path(root));
    let store = BlobStore::new(objects_dir(root));
    let size = store.total_bytes();

    let superseded = merged_ids(&records);
    let all_turns: Vec<&TurnRecord> = records
        .iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        })
        .collect();
    let turns: Vec<&TurnRecord> = all_turns
        .iter()
        .copied()
        .filter(|t| !superseded.contains(&t.id) && t.tool.as_deref() != Some("git"))
        .collect();

    let gaps = count_gaps(&records);

    // Read once, reused below for the ignore-reload line and (further down)
    // the memory/DEGRADED sections — same single-read pattern those already
    // used, just hoisted so this line can consult it too.
    let state = read_state(root);

    // Rich-rate over the trailing 20 agent turns (E+): < 90 % warns. With zero
    // agent turns there is no rate to report — a computed 100% would be
    // vacuous (D-PD3), so this prints an honest "n/a" instead.
    let trailing: Vec<&&TurnRecord> = turns.iter().rev().take(20).collect();

    let mut out = String::new();
    out.push_str(&format!("store:      {}\n", human_bytes(size)));
    out.push_str(&format!(
        "turns:      {} (agent turns; git activity hidden)\n",
        turns.len()
    ));
    out.push_str(&format!("gaps:       {gaps} recording gap(s)\n"));
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
    // `doctor`/`purge` already use), not just on the epoch-nonce match.
    let daemon_live = crate::daemon::daemon_is_running(root);
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

    // AC I+: the store is checked (and, if over, evicted) here rather than
    // from the daemon's turn-close path — see DEVIATIONS in the delivery
    // receipt for why. Prompt blobs are exempt; only snapshot blobs evict.
    if size > budget {
        let owned_turns: Vec<TurnRecord> = all_turns.iter().map(|t| (*t).clone()).collect();
        // SAFETY (Phase 1 honesty fix): harvest the protect-set as late as
        // possible, immediately before calling `enforce_budget`, to narrow
        // the window a live daemon (running continuously under launchd —
        // Decisions log #2, no liveness refusal here) could append a new
        // in-flight blob after we've read log.jsonl/open.json but before the
        // remove loop runs.
        let extra_protected = extra_protected_refs(root);
        let evicted = agentrec_core::retention::enforce_budget(
            &store,
            &owned_turns,
            budget,
            &extra_protected,
        );
        // Honesty (B): budget enforcement here only evicts turn-referenced
        // snapshot blobs. Most store bloat is usually ORPHANED blobs —
        // superseded intermediate snapshots the daemon `put` for crash
        // recovery that no committed turn references — which eviction can't
        // touch. Attribute that share explicitly and point at its only
        // reclaim path, instead of claiming "snapshots evicted" when the
        // freed figure is ~0.
        let orphans = crate::purgecmd::orphan_bytes(root, &store);
        out.push_str(&format!(
            "store {} over {} budget — snapshot eviction freed {}",
            human_bytes(size),
            human_bytes(budget),
            human_bytes(evicted.bytes)
        ));
        // Honesty (Phase 1): protecting pinned/in-flight refs shrinks
        // `evicted.bytes` — sometimes to 0 even while genuinely over
        // budget — and an unexplained "0 freed" is exactly the dishonest
        // status class the orphan-bloat attribution above already fixed.
        if evicted.protected_bytes > 0 {
            out.push_str(&format!(
                "; {} protected (pinned or in-flight — never evicted)",
                human_bytes(evicted.protected_bytes)
            ));
        }
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
fn extra_protected_refs(root: &Path) -> HashSet<String> {
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

    let signal = SignalEvent {
        v: 1,
        ts: wall_now_ms(),
        tool: tool.to_string(),
        event: Some(event.to_string()),
        session,
        transcript,
        prompt,
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

/// Test-only override (Phase 1, `cli/tests/integration.rs`,
/// `status_eviction_keeps_open_turn_blob`) that lets an integration test
/// exercise the real `status` verb's over-budget/eviction path — the AC
/// this test proves is call-site wiring (does `status` actually pass the
/// harvested protect-set into `enforce_budget`?), not the eviction
/// mechanism itself (already core-unit-tested), and a genuine ~2 GiB store
/// is infeasible to build in a test. Same `#[cfg(debug_assertions)]`
/// fail-safe class as [`TEST_FORCE_BUDGET_EXCEEDED_VAR`] below — compiled
/// out of release builds, so it can never override a real user's budget.
#[cfg(debug_assertions)]
const TEST_STORE_BUDGET_BYTES_VAR: &str = "AGENTREC_TEST_STORE_BUDGET_BYTES";

/// [`agentrec_core::MAX_STORE_BYTES`] unless [`TEST_STORE_BUDGET_BYTES_VAR`]
/// is set to a valid `u64`, in which case that value is used instead. The
/// override is a no-op — the env is never read — in release builds.
fn effective_store_budget() -> u64 {
    #[cfg(debug_assertions)]
    if let Ok(v) = std::env::var(TEST_STORE_BUDGET_BYTES_VAR) {
        if let Ok(n) = v.parse::<u64>() {
            return n;
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
/// `started.elapsed().as_millis()` — wall time from the top of this
/// function to the append, monotonic-clock-derived — and is now carried on
/// **every** append site in this function, including the two early-bail
/// sites above (thread-spawn failure, `recv_timeout` timeout/disconnect),
/// which previously never computed it at all. Never `state.json`, which
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

/// A recording gap is any `start` epoch that follows a prior `start` with no
/// intervening `stop` (kill -9 left the first unterminated).
fn count_gaps(records: &[LogRecord]) -> usize {
    let mut gaps = 0;
    let mut open = false;
    for r in records {
        if let LogRecord::Epoch(e) = r {
            match e.event.as_str() {
                "start" => {
                    if open {
                        gaps += 1; // previous session never cleanly stopped
                    }
                    open = true;
                }
                "stop" => open = false,
                _ => {}
            }
        }
    }
    gaps
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
    #[test]
    #[cfg(not(debug_assertions))]
    fn store_budget_override_is_a_no_op_in_release() {
        std::env::set_var("AGENTREC_TEST_STORE_BUDGET_BYTES", "5");
        assert_eq!(
            effective_store_budget(),
            agentrec_core::MAX_STORE_BYTES,
            "release builds must never honor AGENTREC_TEST_STORE_BUDGET_BYTES"
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
            files: vec![FileEntry {
                path: path.into(),
                before: None,
                after: Some(hash.into()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
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
            out.contains("snapshot eviction freed"),
            "expected eviction mention: {out}"
        );
        assert!(
            !store.contains(&hash),
            "the only snapshot blob should have been evicted"
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

        // Budget below total (1000 B) so the notice fires.
        let out = status_report(root, 500).unwrap();
        assert!(
            out.contains("unreferenced (superseded snapshots)"),
            "expected orphan attribution: {out}"
        );
        assert!(
            out.contains("purge --orphans"),
            "expected the reclaim command named: {out}"
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
        assert!(
            !out.contains("budget"),
            "no over-budget notice expected: {out}"
        );
        assert!(store.contains(&hash));
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
        write_state(root, &state).unwrap();

        let payload = status_json(root).unwrap();
        for field in [
            "snapshot_failures",
            "prompt_put_failures",
            "io_failed",
            "state_parse_failures",
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
            "store:      0 B\n\
             turns:      0 (agent turns; git activity hidden)\n\
             gaps:       0 recording gap(s)\n\
             rich-rate:  n/a (no agent turns yet)\n\
             memory:     0 fresh, 0 stale, 0 rejects, 0 injections, 0 failures\n",
            "healthy-store status output must be unchanged: {out}"
        );
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
            out.contains("snapshot eviction freed 0 B"),
            "expected 0 freed: {out}"
        );
        assert!(
            out.contains("protected") && (out.contains("pinned") || out.contains("in-flight")),
            "expected a protected-bytes attribution naming pinned/in-flight: {out}"
        );
    }
}
