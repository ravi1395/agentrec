//! Read verbs (`log`, `status`) and the emitter-side `hook` command. All reads
//! are file-based — they never need the daemon running.

use crate::fmt;
use crate::state::{read_state, write_state};
use crate::{log_path, objects_dir, signal_path};
use agentrec_core::record::{append_log_line, LogRecord, SignalEvent, TurnRecord};
use agentrec_core::scrub;
use agentrec_core::store::BlobStore;
use std::collections::HashSet;
use std::io::{IsTerminal, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

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
    out.push_str(&format!(
        "memory:     {mem_fresh} fresh, {mem_stale} stale, {} rejects\n",
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
    let prompt = payload
        .get("prompt")
        .and_then(|v| v.as_str())
        .map(scrub::scrub);
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
    append_log_line(&signal_path(root), &line)
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

fn wall_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
}
