//! Read verbs (`log`, `status`) and the emitter-side `hook` command. All reads
//! are file-based — they never need the daemon running.

use crate::fmt;
use crate::state::{read_state, write_state};
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
pub fn log(
    root: &Path,
    all: bool,
    json: bool,
    limit: usize,
    utc: bool,
    explain: bool,
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

    // Rendered lines (non-JSON only) are accumulated so `--explain` can scan
    // exactly what this invocation printed, not every term that ever exists.
    let mut rendered = String::new();

    // Newest first; the log is append-order (oldest first).
    for turn in turns.iter().rev().take(limit) {
        if json {
            let line = serde_json::to_string(turn).map_err(|e| e.to_string())?;
            println!("{line}");
        } else {
            let line = format_turn(turn, now_ms, utc, color);
            println!("{line}");
            rendered.push_str(&line);
            rendered.push('\n');
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

fn format_turn(t: &TurnRecord, now_ms: u64, utc: bool, color: bool) -> String {
    let id = fmt::paint(&short_id(&t.id), "36", color);
    let tool = t.tool.as_deref().unwrap_or("—");
    let when = if utc {
        t.started.clone()
    } else {
        fmt::relative_time(&t.started, now_ms)
    };
    let n = t.files.len();
    let files = if n == 1 {
        "1 file".to_string()
    } else {
        format!("{n} files")
    };
    let excerpt = t
        .prompt_excerpt
        .as_deref()
        .map(|e| format!("  \"{}\"", fmt::sanitize_terminal(e)))
        .unwrap_or_default();
    let trunc = if t.truncated { " (truncated)" } else { "" };
    format!(
        "{id}  {:5}  {tool:12}  {when}  {files}{excerpt}{trunc}",
        t.grade
    )
}

/// `status`: store size, recording gaps, and rich-rate (the health stat that
/// catches silently broken hooks). `ack_degraded` clears a prior DEGRADED
/// snapshot-failure banner (D35) instead of printing status.
pub fn status(root: &Path, ack_degraded: bool) -> Result<(), String> {
    if ack_degraded {
        let mut state = read_state(root);
        state.snapshot_failures = 0;
        state.io_failed.clear();
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
    print!("{}", status_report(root, agentrec_core::MAX_STORE_BYTES)?);
    Ok(())
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
    let state = read_state(root);
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
        let evicted = agentrec_core::retention::enforce_budget(&store, &owned_turns, budget);
        out.push_str(&format!(
            "store {} over {} budget — oldest snapshots evicted ({} freed)\n",
            human_bytes(size),
            human_bytes(budget),
            human_bytes(evicted.bytes)
        ));
    }

    if state.snapshot_failures > 0 {
        out.push('\n');
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
    Ok(out)
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
/// appends a `{"ts","budget_exceeded":true}` line to `memory-stats.jsonl` —
/// visible evidence the budget was actually hit, not silent degradation. On
/// an actual injection (non-empty block, within budget) appends
/// `{"ts","n"}` instead, plus `"capped":true` (F3) when the verify walk hit
/// `RECALL_VERIFY_CAP` — recorded as a stat only, NEVER stdout noise; the
/// `--for-hook`/hook stdout contract (block or nothing, exit 0 always) is
/// unconditional. F3 also covers the case a plain `n`-vs-nothing split would
/// miss: a capped walk that found ZERO fresh hits (`outcome.block.is_empty()`)
/// is exactly the silent-truncation scenario this fix exists for, so it gets
/// its own `{"ts","capped":true}` line rather than returning with no stat at
/// all. Never `state.json`, which only the daemon writes (the hazard this
/// task is explicitly gated against).
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
        let stats_line =
            serde_json::json!({ "ts": wall_now_ms(), "budget_exceeded": true }).to_string();
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
            let stats_line =
                serde_json::json!({ "ts": wall_now_ms(), "budget_exceeded": true }).to_string();
            let _ = append_log_line(&crate::memory_stats_path(root), &stats_line);
            return;
        }
    };
    let elapsed_ms = started.elapsed().as_millis();

    if outcome.budget_exceeded || elapsed_ms > RECALL_BUDGET_MS {
        let stats_line =
            serde_json::json!({ "ts": wall_now_ms(), "budget_exceeded": true }).to_string();
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
            let stats_line = serde_json::json!({ "ts": wall_now_ms(), "capped": true }).to_string();
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
    let mut stats = serde_json::json!({ "ts": wall_now_ms(), "n": n });
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

fn short_id(id: &str) -> String {
    // `t_<ULID>` → keep the prefix + last 4 chars for readability; prefix-match
    // on the full id is unambiguous per K+.
    let body = id.strip_prefix("t_").unwrap_or(id);
    if body.len() <= 8 {
        return id.to_string();
    }
    format!("t_{}…{}", &body[..4], &body[body.len() - 4..])
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
        assert!(out.contains("evicted"), "expected eviction mention: {out}");
        assert!(
            !store.contains(&hash),
            "the only snapshot blob should have been evicted"
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
}
