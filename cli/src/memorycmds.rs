//! `agentrec remember` (memory v1 Task 4): manual, human-authored pinned
//! memories. Validates every `--from` path and hashes it into a `Pin` before
//! anything is written; the first invalid path aborts with no disk mutation.
//! The fact is scrubbed at this layer (not only inside `append_memory`) so
//! the empty-after-scrub refusal happens before a `MemoryRecord` is even
//! built, and only the scrubbed text — never the raw one — is ever placed
//! into the record.

use crate::fmt;
use agentrec_core::memory::{self, EffectiveMemory, Freshness, MemoryOp, MemoryRecord, Pin};
use agentrec_core::record::{append_log_line, SignalEvent};
use agentrec_core::{id, scrub};
use serde::Serialize;
use std::io::IsTerminal;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// `from` is a comma-separated list of repo-relative paths. Each is
/// validated (`memory::validate_pin_path`) and hashed (`memory::hash_pin`)
/// in order — the first failure returns immediately, naming the offending
/// path, with nothing written to `memory.jsonl`.
pub fn remember(root: &Path, fact: &str, from: &str) -> Result<(), String> {
    let mut pins = Vec::new();
    for raw in from.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let rel = memory::validate_pin_path(root, raw)?;
        let hash = memory::hash_pin(root, &rel)?;
        pins.push(Pin { path: rel, hash });
    }
    if pins.is_empty() {
        return Err("--from must name at least one path".to_string());
    }

    let scrubbed_fact = scrub::scrub(fact);
    if scrubbed_fact.trim().is_empty() {
        return Err("fact scrubbed to empty — refusing to store an empty husk".to_string());
    }

    let rec = MemoryRecord {
        v: 1,
        kind: "memory".to_string(),
        id: id::ulid(),
        op: MemoryOp::Assert,
        fact: scrubbed_fact,
        pins,
        source_turns: vec![],
        origin: "human".to_string(),
        ts: wall_now_ms(),
        reason: None,
    };
    memory::append_memory(root, &rec)
}

/// `agentrec candidate` (Task 8): the memory WRITE path's agent-facing
/// emitter. Appends one memory-candidate `SignalEvent` line to the signal
/// inbox via `record::append_log_line` — unsynced, same D34 inbox exemption
/// `cmds::hook` relies on for start/stop lines. The daemon
/// (`daemon::ingest_candidate`, Task 7) does the real work: hashing pins
/// against the live tree, deduping, and persisting to `memory.jsonl`. This
/// layer never hashes or checks path existence (design spec, "Rejected
/// approaches — emitter-side hashing": a hash computed here could be stale
/// by the time the daemon reads it) — it only scrubs and light-validates.
///
/// Order: scrub the fact FIRST (defence in depth — a secret must never
/// enter even the unsynced inbox, not even transiently; this is the layer
/// Task 7's own tests explicitly deferred to Task 8) -> refuse if the
/// scrubbed fact is empty or over `memory::FACT_MAX_CHARS` -> split `from`
/// on `,` (mirrors `remember`), trim, strip a leading `./` (closes the
/// dedup gap Task 7 noted: `./a.rs` and `a.rs` naming the same file must
/// normalize to one pin) -> refuse on zero paths or more than
/// `memory::PINS_MAX`. Nothing heavier: full validation (existence,
/// traversal, secret-path, hashing) is the daemon's job at ingestion.
pub fn candidate(root: &Path, fact: &str, from: &str, tool: &str) -> Result<(), String> {
    let scrubbed_fact = scrub::scrub(fact);
    if scrubbed_fact.trim().is_empty() {
        return Err("fact scrubbed to empty — refusing to store an empty husk".to_string());
    }
    let fact_len = scrubbed_fact.chars().count();
    if fact_len > memory::FACT_MAX_CHARS {
        return Err(format!(
            "fact exceeds {}-char cap ({fact_len} chars)",
            memory::FACT_MAX_CHARS
        ));
    }

    let mut pins = Vec::new();
    for raw in from.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let normalized = raw.strip_prefix("./").unwrap_or(raw);
        if normalized.is_empty() {
            continue;
        }
        pins.push(normalized.to_string());
    }
    if pins.is_empty() {
        return Err("--from must name at least one path".to_string());
    }
    if pins.len() > memory::PINS_MAX {
        return Err(format!(
            "--from names more than the {}-path cap ({} paths)",
            memory::PINS_MAX,
            pins.len()
        ));
    }

    let signal = SignalEvent {
        v: 1,
        ts: wall_now_ms(),
        tool: tool.to_string(),
        event: None,
        session: None,
        transcript: None,
        prompt: None,
        kind: Some("memory-candidate".to_string()),
        fact: Some(scrubbed_fact),
        pins: Some(pins),
    };
    let line = serde_json::to_string(&signal).map_err(|e| e.to_string())?;
    append_log_line(&crate::signal_path(root), &line)
}

/// Mirrors the same one-line helper repeated across `cmds.rs`/`purgecmd.rs`/
/// `daemon.rs`/`readcmds.rs` — a 3-line `SystemTime` call, not worth sharing.
fn wall_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Read `memory_enabled` from `.agentrec/config.toml` — mirrors
/// `purgecmd::read_ttl_days`'s hand-rolled `key = value` scan (not worth a
/// `toml` dependency for two more keys). Missing file, missing key, or an
/// unparseable value all fall back to the documented default of `true`.
pub fn read_memory_enabled(root: &Path) -> bool {
    let path = crate::agentrec_dir(root).join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return true;
    };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some(rest) = line.strip_prefix("memory_enabled") else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        match value.trim() {
            "true" => return true,
            "false" => return false,
            _ => continue,
        }
    }
    true
}

/// Read `memory_inject_max` from `.agentrec/config.toml` — same scanning
/// pattern as [`read_memory_enabled`] / `purgecmd::read_ttl_days`. Missing
/// file, missing key, or an unparseable value all fall back to
/// [`HOOK_MAX_FACTS_DEFAULT`].
pub fn read_memory_inject_max(root: &Path) -> usize {
    let path = crate::agentrec_dir(root).join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return HOOK_MAX_FACTS_DEFAULT;
    };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some(rest) = line.strip_prefix("memory_inject_max") else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        if let Ok(n) = value.trim().parse::<usize>() {
            return n;
        }
    }
    HOOK_MAX_FACTS_DEFAULT
}

/// Default `memory_inject_max` (design spec) when `config.toml` has no such
/// key — `agentrec recall --for-hook`'s own `-k` default matches this.
pub const HOOK_MAX_FACTS_DEFAULT: usize = 5;
/// Max total chars (fence lines included) of a `--for-hook` block.
const HOOK_MAX_CHARS: usize = 800;

/// `recall`: rank-then-verify search over pinned memories (`memory::recall`
/// already guarantees every returned entry is `Freshness::Fresh` — INV-M2).
///
/// Three output modes, checked in this order:
/// - `for_hook`: fail-open always. An uninitialized repo, a recall error, or
///   zero hits all produce empty stdout + exit 0 — never stderr, never a
///   nonzero exit. A hit prints exactly one fenced block (see
///   `build_hook_block`), no ids/hashes.
/// - `json`: the effective records (all `Freshness::Fresh` by construction)
///   as a JSON array — `[]` on zero hits, same PD4 convention as `log --json`.
/// - human: a plain per-line listing; an empty *store* (never any memory
///   recorded) gets the PD3 zero-state message on stderr, exit 0; an empty
///   *result set* against a nonempty store gets a plain stdout notice.
///
/// Outside `for_hook`, an uninitialized repo (`.agentrec/` missing) is a
/// real error — exit 1, stderr — matching every other read verb's posture.
pub fn recall_cmd(
    root: &Path,
    query: &str,
    k: usize,
    json: bool,
    for_hook: bool,
) -> Result<(), String> {
    if for_hook {
        let block = recall_for_hook(root, query, k);
        if !block.is_empty() {
            print!("{block}");
        }
        return Ok(());
    }

    if !crate::agentrec_dir(root).is_dir() {
        return Err("not initialized — run `agentrec init`".to_string());
    }

    let hits = memory::recall(root, query, k)?;

    if json {
        let arr: Vec<EffectiveJson> = hits
            .iter()
            .map(|m| effective_json(m, Freshness::Fresh))
            .collect();
        println!(
            "{}",
            serde_json::to_string(&arr).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    if hits.is_empty() {
        let all = memory::load_effective(root).map_err(|e| e.to_string())?;
        if all.is_empty() {
            eprintln!("no memories yet — agentrec remember \"<fact>\" --from <file>");
        } else {
            println!("no fresh memories match \"{query}\"");
        }
        return Ok(());
    }

    let now_ms = wall_now_ms();
    let color = fmt::should_color(
        std::io::stdout().is_terminal(),
        std::env::var_os("NO_COLOR").is_some(),
    );
    for m in &hits {
        println!("{}", format_memory_line(m, Freshness::Fresh, now_ms, color));
    }
    Ok(())
}

/// `memories`: full audit listing (not a query). Base set is every
/// non-retracted memory unless `all`, which also includes retracted ones;
/// `stale` then narrows that set to only non-`Fresh` (Stale or Orphaned)
/// entries. Freshness is derived per row (`memory::pin_freshness`), never
/// cached. Newest (`ts`) first.
pub fn memories(root: &Path, stale: bool, all: bool, json: bool) -> Result<(), String> {
    if !crate::agentrec_dir(root).is_dir() {
        return Err("not initialized — run `agentrec init`".to_string());
    }

    let effective = memory::load_effective(root)?;
    let mut rows: Vec<(&EffectiveMemory, Freshness)> = effective
        .iter()
        .filter(|m| all || !m.retracted)
        .map(|m| (m, memory::pin_freshness(root, &m.pins)))
        .filter(|(_, f)| !stale || *f != Freshness::Fresh)
        .collect();
    rows.sort_by_key(|(m, _)| std::cmp::Reverse(m.ts));

    if json {
        let arr: Vec<EffectiveJson> = rows.iter().map(|(m, f)| effective_json(m, *f)).collect();
        println!(
            "{}",
            serde_json::to_string(&arr).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    if rows.is_empty() {
        println!("no memories recorded");
        return Ok(());
    }

    let now_ms = wall_now_ms();
    let color = fmt::should_color(
        std::io::stdout().is_terminal(),
        std::env::var_os("NO_COLOR").is_some(),
    );
    for (m, freshness) in rows {
        println!("{}", format_memory_line(m, freshness, now_ms, color));
    }
    Ok(())
}

/// Task 9: the recall-for-hook logic, shared IN-PROCESS by `agentrec recall
/// --for-hook` (Task 5, CLI `-k`) and `cmds::hook`'s UserPromptSubmit arm
/// (Task 9, config-driven `memory_inject_max`) — never a subprocess. Fail-
/// open on every edge (uninitialized repo, corrupt store, any
/// `memory::recall` error): always returns `""`, never panics.
///
/// `max_facts` feeds BOTH the `k` passed to `memory::recall` AND
/// `build_hook_block`'s per-block cap — this is the fix for the coupling bug
/// a prior review flagged: passing `max_facts` only to one side would let a
/// `memory_inject_max=10` config get silently capped at the old hardcoded
/// `HOOK_MAX_FACTS_DEFAULT=5` (`build_hook_block` still took at most 5 lines
/// regardless of how many `recall` returned).
pub fn recall_for_hook(root: &Path, query: &str, max_facts: usize) -> String {
    if !crate::agentrec_dir(root).is_dir() {
        return String::new();
    }
    let hits = match memory::recall(root, query, max_facts) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    build_hook_block(&hits, max_facts)
}

/// Builds the exact `--for-hook` fenced block (Task 9's parse contract):
/// ```text
/// ```agentrec memory
/// - <fact>  [pins: <p1>, <p2>]
/// ```
/// ```
/// Takes at most `max_facts` of `hits` (already rank-ordered, Fresh only),
/// then — if the rendered block still exceeds `HOOK_MAX_CHARS` — drops whole
/// trailing (lowest-ranked) facts, never truncates a line mid-way, until it
/// fits or nothing is left (-> `""`, meaning "print nothing"). No id or hash
/// ever appears in a line.
fn build_hook_block(hits: &[EffectiveMemory], max_facts: usize) -> String {
    let mut lines: Vec<String> = hits
        .iter()
        .take(max_facts)
        .map(|m| {
            let pins: Vec<&str> = m.pins.iter().map(|p| p.path.as_str()).collect();
            format!("- {}  [pins: {}]", m.fact, pins.join(", "))
        })
        .collect();

    loop {
        if lines.is_empty() {
            return String::new();
        }
        let block = format!("```agentrec memory\n{}\n```\n", lines.join("\n"));
        if block.chars().count() <= HOOK_MAX_CHARS {
            return block;
        }
        lines.pop();
    }
}

/// One line of the human `recall`/`memories` listing: short id, freshness
/// label, relative timestamp (via `fmt::relative_time` — `EffectiveMemory.ts`
/// is epoch ms, converted through `agentrec_core::time::rfc3339` the same
/// way `agentrec_core::time` and `fmt` already agree on), the fact, and pin
/// paths (paths only — no hashes, this is the human view, not the hook one).
fn format_memory_line(
    m: &EffectiveMemory,
    freshness: Freshness,
    now_ms: u64,
    color: bool,
) -> String {
    let id = fmt::paint(&short_id(&m.id), "36", color);
    let label = freshness_str(freshness);
    let when = fmt::relative_time(&agentrec_core::time::rfc3339(m.ts), now_ms);
    let pins: Vec<&str> = m.pins.iter().map(|p| p.path.as_str()).collect();
    format!(
        "{id}  {label:8}  {when}  {}  [pins: {}]",
        m.fact,
        pins.join(", ")
    )
}

fn short_id(id: &str) -> String {
    if id.len() <= 8 {
        id.to_string()
    } else {
        id[..8].to_string()
    }
}

fn freshness_str(f: Freshness) -> &'static str {
    match f {
        Freshness::Fresh => "fresh",
        Freshness::Stale => "stale",
        Freshness::Orphaned => "orphaned",
    }
}

/// `--json` shape for both `recall` and `memories`: the full effective
/// record plus the derived freshness label. Unlike the `--for-hook` block,
/// this is a machine format — id and pin hashes are included in full, same
/// posture as every other `--json` output in this CLI (e.g. `log --json`).
#[derive(Serialize)]
struct EffectiveJson<'a> {
    id: &'a str,
    fact: &'a str,
    pins: &'a [Pin],
    origin: &'a str,
    ts: u64,
    retracted: bool,
    freshness: &'static str,
}

fn effective_json(m: &EffectiveMemory, freshness: Freshness) -> EffectiveJson<'_> {
    EffectiveJson {
        id: &m.id,
        fact: &m.fact,
        pins: &m.pins,
        origin: &m.origin,
        ts: m.ts,
        retracted: m.retracted,
        freshness: freshness_str(freshness),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_when_from_has_no_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        let err = remember(root, "a fact", "").unwrap_err();
        assert!(err.contains("--from"), "{err}");
        assert!(
            !memory::memory_path(root).exists(),
            "nothing written on empty --from"
        );
    }

    #[test]
    fn rejects_first_bad_pin_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        let err = remember(root, "a fact", "/etc/passwd").unwrap_err();
        assert!(err.contains("/etc/passwd"), "{err}");
        assert!(!memory::memory_path(root).exists());
    }

    #[test]
    fn writes_a_valid_record() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        std::fs::write(root.join("a.rs"), b"fn a() {}").unwrap();
        remember(root, "a plain fact about a.rs", "a.rs").unwrap();
        let effective = memory::load_effective(root).unwrap();
        assert_eq!(effective.len(), 1);
        assert_eq!(effective[0].origin, "human");
        assert_eq!(effective[0].pins[0].path, "a.rs");
    }
}
