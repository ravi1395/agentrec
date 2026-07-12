//! `agentrec remember` (memory v1 Task 4): manual, human-authored pinned
//! memories. Validates every `--from` path and hashes it into a `Pin` before
//! anything is written; the first invalid path aborts with no disk mutation.
//! The fact is scrubbed at this layer (not only inside `append_memory`) so
//! the empty-after-scrub refusal happens before a `MemoryRecord` is even
//! built, and only the scrubbed text — never the raw one — is ever placed
//! into the record.

use crate::fmt;
use agentrec_core::memory::{self, EffectiveMemory, Freshness, MemoryOp, MemoryRecord, Pin};
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

/// Mirrors the same one-line helper repeated across `cmds.rs`/`purgecmd.rs`/
/// `daemon.rs`/`readcmds.rs` — a 3-line `SystemTime` call, not worth sharing.
fn wall_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Max facts injected into a `--for-hook` block (design spec
/// `memory_inject_max`; hardcoded here, config wiring is Task 9).
const HOOK_MAX_FACTS: usize = 5;
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
    if !crate::agentrec_dir(root).is_dir() {
        if for_hook {
            return Ok(());
        }
        return Err("not initialized — run `agentrec init`".to_string());
    }

    let hits = match memory::recall(root, query, k) {
        Ok(v) => v,
        Err(e) => {
            if for_hook {
                return Ok(());
            }
            return Err(e);
        }
    };

    if for_hook {
        let block = build_hook_block(&hits);
        if !block.is_empty() {
            print!("{block}");
        }
        return Ok(());
    }

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

/// Builds the exact `--for-hook` fenced block (Task 9's parse contract):
/// ```text
/// ```agentrec memory
/// - <fact>  [pins: <p1>, <p2>]
/// ```
/// ```
/// Takes at most `HOOK_MAX_FACTS` of `hits` (already rank-ordered, Fresh
/// only), then — if the rendered block still exceeds `HOOK_MAX_CHARS` —
/// drops whole trailing (lowest-ranked) facts, never truncates a line mid-
/// way, until it fits or nothing is left (-> `""`, meaning "print nothing").
/// No id or hash ever appears in a line.
fn build_hook_block(hits: &[EffectiveMemory]) -> String {
    let mut lines: Vec<String> = hits
        .iter()
        .take(HOOK_MAX_FACTS)
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
