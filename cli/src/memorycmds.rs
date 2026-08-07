//! `agentrec remember` (memory v1 Task 4): manual, human-authored pinned
//! memories. Validates every `--from` path and hashes it into a `Pin` before
//! anything is written; the first invalid path aborts with no disk mutation.
//! The fact is scrubbed at this layer (not only inside `append_memory`) so
//! the empty-after-scrub refusal happens before a `MemoryRecord` is even
//! built, and only the scrubbed text — never the raw one — is ever placed
//! into the record.

use crate::cmds::wall_now_ms;
use crate::fmt;
use agentrec_core::diff;
use agentrec_core::memory::{self, EffectiveMemory, Freshness, MemoryOp, MemoryRecord, Pin};
use agentrec_core::record::{append_log_line, LogRecord, SignalEvent, TurnRecord};
use agentrec_core::store::{BlobStore, StoreError};
use agentrec_core::view;
use agentrec_core::{id, scrub};
use serde::Serialize;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;
use std::time::Instant;

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
    crate::memlock::append_memory_locked(root, &rec)
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
        files_written: None,
        emitter_turn: None,
        model: None,
        kind: Some("memory-candidate".to_string()),
        fact: Some(scrubbed_fact),
        pins: Some(pins),
    };
    let line = serde_json::to_string(&signal).map_err(|e| e.to_string())?;
    append_log_line(&crate::signal_path(root), &line)
}

/// Parse one `--replace-pin` value as `OLD=NEW`, splitting on the FIRST `=`
/// (a path could legitimately contain `=` on the right-hand side, though not
/// the left — `old` is always matched verbatim against an existing pin
/// path). Rejects a missing `=` or either side being empty after the split;
/// neither side is trimmed of whitespace — mirrors `remember`'s from-path
/// handling, no implicit normalization that could mask a typo.
fn parse_replace_pin(raw: &str) -> Result<(String, String), String> {
    match raw.split_once('=') {
        Some((old, new)) if !old.is_empty() && !new.is_empty() => {
            Ok((old.to_string(), new.to_string()))
        }
        _ => Err(format!(
            "--replace-pin '{raw}' must be OLD=NEW with both sides non-empty"
        )),
    }
}

/// Match `id_ref` against effective memory ids, exact or unambiguous prefix
/// — mirrors `readcmds::resolve_turn`'s resolution style, adapted to
/// `EffectiveMemory` (already deduped by id via `load_effective`'s fold, so
/// no group-by is needed here). Zero or multiple matches is an error naming
/// the candidates.
fn resolve_memory<'a>(
    effective: &'a [EffectiveMemory],
    id_ref: &str,
) -> Result<&'a EffectiveMemory, String> {
    if effective.is_empty() {
        return Err("no memories recorded".to_string());
    }
    let matches: Vec<&EffectiveMemory> = effective
        .iter()
        .filter(|m| m.id == id_ref || m.id.starts_with(id_ref))
        .collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(format!("unknown memory id '{id_ref}'")),
        n => {
            let ids: Vec<String> = matches.iter().map(|m| short_id(&m.id)).collect();
            Err(format!(
                "ambiguous memory id '{id_ref}' — matches {n} memories: {}",
                ids.join(", ")
            ))
        }
    }
}

/// `verify <id> [--confirm] [--drop-pin <path>]... [--replace-pin
/// <old>=<new>]...` (Task 10, CAS diff added F7 part B, explicit re-pin
/// added F9): re-pins a drifted memory. Without `--confirm`, prints the fact
/// and per-pin drift against the current working tree and touches nothing on
/// disk. With `--confirm`, appends a `reverify` record (same id, per the
/// design spec — `reverify`/`retract` never mint a new id) carrying a fresh
/// hash for every still-present or replaced pin.
///
/// On a hash-drifted (not orphaned) pin, also renders a diff summary via the
/// CAS (design spec §Lifecycle: "diff summary via CAS where snapshots
/// exist") — see [`print_pin_diff`].
///
/// An orphaned pin (pinned path no longer exists) is never silently dropped:
/// `--confirm` refuses — naming the orphaned path, nothing appended — unless
/// the caller also names that exact path via one or more `--drop-pin` flags,
/// or replaces it via `--replace-pin`. Dropping every pin (leaving the
/// memory with zero pins) is refused outright; a memory must retain at least
/// one pin at all times, the same invariant `append_memory` enforces on
/// every other write path (INV-M1).
///
/// `--replace-pin old=new` (F9) re-points an EXISTING pin of this memory
/// (`old` must already be one of `m.pins` — fresh, stale, or orphaned) to a
/// validated successor path `new`. `new` passes the exact same validation as
/// `remember --from` (`memory::validate_pin_path`: in-root, no `..`
/// traversal, no symlink escape, must exist, not a secret path), reusing
/// that validator rather than re-implementing it. This is the ONLY way to
/// recover a memory whose sole pinned file was renamed — there is
/// deliberately no automatic rename/successor guessing (design spec's
/// rejected-approaches: an ambiguous rename could silently re-ground a fact
/// against the wrong source).
///
/// Every `--replace-pin` is validated up front, atomically, before anything
/// is printed: malformed `OLD=NEW` syntax, a duplicate `old` across multiple
/// flags, an `old` also named by `--drop-pin` (contradictory), an `old` not
/// currently a pin on this memory, or an invalid `new` all abort with
/// nothing appended and nothing printed — same fail-fast posture as
/// `remember`'s pin validation.
pub fn verify(
    root: &Path,
    id: &str,
    confirm: bool,
    drop_pins: &[String],
    replace_pins: &[String],
) -> Result<(), String> {
    if !crate::agentrec_dir(root).is_dir() {
        return Err("not initialized — run `agentrec init`".to_string());
    }
    let effective = memory::load_effective(root)?;
    let m = resolve_memory(&effective, id)?;
    if m.retracted {
        return Err(format!(
            "memory {} is already retracted — nothing to verify",
            short_id(&m.id)
        ));
    }

    // F9: parse + fully validate every --replace-pin before printing or
    // touching anything else — an atomic all-or-nothing gate.
    let mut replace_map: HashMap<String, (String, String)> = HashMap::new();
    for raw in replace_pins {
        let (old, new) = parse_replace_pin(raw)?;
        if replace_map.contains_key(&old) {
            return Err(format!(
                "--replace-pin names '{old}' as the old path more than once"
            ));
        }
        if drop_pins.iter().any(|d| d == &old) {
            return Err(format!(
                "'{old}' is named by both --drop-pin and --replace-pin — contradictory"
            ));
        }
        if !m.pins.iter().any(|p| p.path == old) {
            return Err(format!(
                "'{old}' is not a pin on memory {}",
                short_id(&m.id)
            ));
        }
        let validated_new = memory::validate_pin_path(root, &new)?;
        let new_hash = memory::hash_pin(root, &validated_new)?;
        replace_map.insert(old, (validated_new, new_hash));
    }

    let store = BlobStore::new(crate::objects_dir(root));
    println!("{}", fmt::sanitize_terminal(&m.fact));
    let mut orphaned: Vec<String> = Vec::new();
    let mut fresh_pins: Vec<Pin> = Vec::new();
    for pin in &m.pins {
        let path = fmt::sanitize_terminal(&pin.path);
        let being_replaced = replace_map.contains_key(&pin.path);
        match memory::hash_pin(root, &pin.path) {
            Ok(current) if current != pin.hash => {
                println!("  {path}: old {} -> new {current}", pin.hash);
                print_pin_diff(&store, pin, root);
                if !being_replaced {
                    fresh_pins.push(Pin {
                        path: pin.path.clone(),
                        hash: current,
                    });
                }
            }
            Ok(current) => {
                println!("  {path}: unchanged");
                if !being_replaced {
                    fresh_pins.push(Pin {
                        path: pin.path.clone(),
                        hash: current,
                    });
                }
            }
            Err(_) => {
                println!("  {path}: deleted");
                orphaned.push(pin.path.clone());
            }
        }
    }
    for (old, (new_path, _)) in &replace_map {
        println!(
            "  {} -> {}",
            fmt::sanitize_terminal(old),
            fmt::sanitize_terminal(new_path)
        );
    }

    if !confirm {
        return Ok(());
    }

    for path in &orphaned {
        if replace_map.contains_key(path) {
            // Being re-pointed via --replace-pin, not dropped.
            continue;
        }
        if !drop_pins.iter().any(|d| d == path) {
            return Err(format!(
                "pin '{path}' no longer exists — reverify refuses to silently drop it; pass --drop-pin {path} to confirm dropping it, or --replace-pin {path}=<successor> to re-point it"
            ));
        }
    }
    for (new_path, new_hash) in replace_map.values() {
        fresh_pins.push(Pin {
            path: new_path.clone(),
            hash: new_hash.clone(),
        });
    }
    if fresh_pins.is_empty() {
        return Err(
            "dropping every orphaned pin would leave this memory with zero pins — refusing"
                .to_string(),
        );
    }

    let rec = MemoryRecord {
        v: 1,
        kind: "memory".to_string(),
        id: m.id.clone(),
        op: MemoryOp::Reverify,
        fact: m.fact.clone(),
        pins: fresh_pins,
        source_turns: vec![],
        // Carry the fact's original authorship forward (design spec
        // §Quality gate: "origin + source_turns give provenance") — a
        // human re-pinning an agent-authored memory does not make the
        // agent stop having asserted the fact. `load_effective`'s fold
        // takes `origin` from whichever op is latest, so this record must
        // restate it explicitly or an agent memory would silently flip to
        // "human" on its first reverify.
        origin: m.origin.clone(),
        ts: wall_now_ms(),
        reason: None,
    };
    crate::memlock::append_memory_locked(root, &rec)
}

/// `forget <id> [--reason <text>]` (Task 10): retracts a memory. Refuses
/// (honest error, exit 1) if `id` is already retracted. The retract record
/// carries the memory's current fact and pins forward unchanged — never
/// empty, since `append_memory` rejects an empty fact or a zero-pin record
/// and both are already guaranteed non-empty on every live memory — so the
/// appended line is self-describing without a reader needing to look up the
/// original assert. `reason`, if given, is scrubbed the same as `remember`'s
/// fact (defence in depth — a pasted secret in a retraction reason must
/// never reach disk); a reason that scrubs to nothing is treated as absent
/// rather than stored as an empty husk.
pub fn forget(root: &Path, id: &str, reason: Option<&str>) -> Result<(), String> {
    if !crate::agentrec_dir(root).is_dir() {
        return Err("not initialized — run `agentrec init`".to_string());
    }
    let effective = memory::load_effective(root)?;
    let m = resolve_memory(&effective, id)?;
    if m.retracted {
        return Err(format!("memory {} is already retracted", short_id(&m.id)));
    }

    let scrubbed_reason = reason.and_then(|r| {
        let s = scrub::scrub(r);
        if s.trim().is_empty() {
            None
        } else {
            Some(s)
        }
    });

    let rec = MemoryRecord {
        v: 1,
        kind: "memory".to_string(),
        id: m.id.clone(),
        op: MemoryOp::Retract,
        fact: m.fact.clone(),
        pins: m.pins.clone(),
        source_turns: vec![],
        // Same provenance-preservation rationale as `verify`: `forget` is
        // "the human counterweight" (design spec) to a bad assertion, but
        // retracting an agent's memory doesn't retroactively make it
        // human-authored — the fold takes `origin` from the retract record
        // itself, so it must restate the original.
        origin: m.origin.clone(),
        ts: wall_now_ms(),
        reason: scrubbed_reason,
    };
    crate::memlock::append_memory_locked(root, &rec)
}

/// F7 part B: render a diff summary for one drifted (hash-mismatched, not
/// orphaned — caller already confirmed the path is still readable) pin, via
/// the CAS — reuses `diff::is_binary`/`diff::unified` (agentrec-core),
/// never a hand-rolled diff.
///
/// A pin's hash is computed directly from file bytes at pin/reverify time
/// (`memory::hash_pin`) — `remember`/`verify` never call `BlobStore::put`,
/// so the pinned content is only IN the CAS by coincidence (some turn
/// happened to snapshot identical bytes). A missing or corrupt blob is
/// therefore the common case, not an edge case, and is reported honestly —
/// same posture as `readcmds::show --prompt`'s corrupt/purged-blob handling
/// — rather than silently producing no diff. All rendered text is routed
/// through `fmt::sanitize_terminal` (fact/pin/path/diff-line convention).
fn print_pin_diff(store: &BlobStore, pin: &Pin, root: &Path) {
    let path = fmt::sanitize_terminal(&pin.path);
    let old = match store.get(&pin.hash) {
        Ok(bytes) => bytes,
        Err(StoreError::Missing(_)) => {
            println!("    {path}: blob unavailable (never captured or purged) — hash only");
            return;
        }
        Err(StoreError::Corrupt(_)) => {
            println!("    {path}: blob unavailable (corrupt — hash mismatch) — hash only");
            return;
        }
    };
    // The caller only reaches here after `memory::hash_pin` succeeded on
    // this same path, so this read should also succeed; a race (deleted
    // between the two reads) degrades to silently skipping the diff rather
    // than erroring — the hash-drift line above already told the caller
    // enough to act on.
    // fsguard: a recorded pin path in the working tree.
    let Ok(current) = agentrec_core::fsguard::read_regular(&root.join(&pin.path)) else {
        return;
    };

    if diff::is_binary(&old) || diff::is_binary(&current) {
        println!(
            "    {path}: binary content changed ({} -> {} bytes)",
            old.len(),
            current.len()
        );
        return;
    }

    let old_str = std::str::from_utf8(&old).unwrap_or("");
    let cur_str = std::str::from_utf8(&current).unwrap_or("");
    let text = diff::unified(old_str, cur_str, &pin.path);
    let (added, removed) = count_diff_lines(&text);
    println!("    {path}: +{added}/-{removed} lines");
    for line in text.lines() {
        println!("    {}", fmt::sanitize_terminal(line));
    }
}

/// Count added/removed content lines in a `diff::unified` output — skips
/// the `+++`/`---` file-header lines so only real hunk lines count.
fn count_diff_lines(unified_text: &str) -> (usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in unified_text.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        match line.chars().next() {
            Some('+') => added += 1,
            Some('-') => removed += 1,
            _ => {}
        }
    }
    (added, removed)
}

/// Read `memory_enabled` from `.agentrec/config.toml` via
/// [`crate::config::load_or_default`]. Missing file, missing key, an
/// unparseable value, or a file-level TOML parse error all fall back to the
/// documented default of `true`.
///
/// **Deliberately stays tolerant** (gate finding, D16 remediation reviewed
/// this call site and kept it as-is): every production caller today is
/// either the daemon (`daemon.rs`'s live-loop and startup-replay reads, both
/// per-tick freshness reads that must never hard-fail) or `cmds::hook`,
/// whose own doc comment documents an explicit fail-open contract — Claude
/// Code invokes `hook` on every prompt, and hard-failing here would exit
/// nonzero AFTER `hook` has already appended the start/stop signal it gates
/// (`cmds.rs`, `append_log_line` runs before this read), i.e. noise with no
/// protective value. No CLI verb (`status`, `purge`, `log`, `show`) reads
/// `memory_enabled` today; if one starts to, route it through
/// [`crate::config::load`] directly, the same way `effective_store_budget_
/// checked` and `read_ttl_days` do, rather than widening this function.
pub fn read_memory_enabled(root: &Path) -> bool {
    crate::config::load_or_default(root).memory_enabled
}

/// Read `memory_inject_max` from `.agentrec/config.toml` via
/// [`crate::config::load_or_default`] — same loader as
/// [`read_memory_enabled`]. Missing file, missing key, an unparseable value,
/// or a file-level TOML parse error all fall back to
/// [`HOOK_MAX_FACTS_DEFAULT`].
///
/// **Deliberately stays tolerant, same rationale as [`read_memory_enabled`]**
/// — its only production caller is `cmds::inject_memory`, itself only ever
/// called from `cmds::hook`'s fail-open path.
pub fn read_memory_inject_max(root: &Path) -> usize {
    crate::config::load_or_default(root).memory_inject_max
}

/// Default `memory_inject_max` (design spec) when `config.toml` has no such
/// key — `agentrec recall --for-hook`'s own `-k` default matches this.
pub const HOOK_MAX_FACTS_DEFAULT: usize = 5;
/// Max total chars (fence lines included) of a `--for-hook` block.
const HOOK_MAX_CHARS: usize = 800;

/// `recall`: rank-then-verify search over pinned memories, routed through
/// [`view::RepositoryView::recall`] (P4b-3) — this function is a renderer
/// over the view's typed `RecallPage`, never a second interpreter of
/// `memory.jsonl`.
///
/// Three output modes, checked in this order:
/// - `for_hook`: fail-open always. An uninitialized repo, a recall error, or
///   zero hits all produce empty stdout + exit 0 — never stderr, never a
///   nonzero exit. A hit prints exactly one fenced block (see
///   `build_hook_block`), no ids/hashes. Untouched by this seam — see
///   [`recall_for_hook`]'s own doc comment.
/// - `json`: `page.items` (all `Freshness::Fresh` by construction, INV-M2)
///   as a JSON array — `[]` on zero hits, same PD4 convention as `log
///   --json`. Serializes the items alone, never `RecallPage` itself
///   (decision 3 — `capped`/`store_corrupt`/`store_empty` must never reach
///   `--json`).
/// - human: a plain per-line listing via [`render_recall`]; an empty
///   *store* (never any memory recorded) gets the PD3 zero-state message on
///   stderr, exit 0; an empty *result set* against a nonempty store gets a
///   plain stdout notice.
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

    let repo = view::RepositoryView::open(root).map_err(|e| e.to_string())?;
    let q = view::RecallQuery {
        query: query.to_string(),
        k,
        after: None,
    };
    let page = repo.recall(&q).map_err(|e| recall_error_text(&e))?;

    if json {
        println!(
            "{}",
            serde_json::to_string(&page.page.items).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    let now_ms = wall_now_ms();
    let color = fmt::should_color(
        std::io::stdout().is_terminal(),
        std::env::var_os("NO_COLOR").is_some(),
    );
    let (stdout, stderr) = render_recall(
        &page,
        RecallOpts {
            now_ms,
            color,
            query,
        },
    );
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    Ok(())
}

/// The human prose for a [`view::RecallError`] — unreachable from this CLI
/// path today (it never sets `RecallQuery::after`, the only way to reach a
/// [`view::CursorError`], and every arm of the underlying `memory::` calls
/// returns `Ok`), but stated rather than unwrapped so a future paging caller
/// gets a real message instead of a panic. Mirrors `readcmds::diff_error_text`'s
/// `Cursor` arm verbatim — the same shared `CursorError` type, the same prose.
fn recall_error_text(e: &view::RecallError) -> String {
    match e {
        view::RecallError::Cursor(c) => match c {
            view::CursorError::Stale => {
                "cursor is stale — the log was rewritten; restart the query".to_string()
            }
            view::CursorError::QueryMismatch => {
                "cursor came from a different query — restart the query".to_string()
            }
            view::CursorError::ZeroLimit => "a limit of 0 has no honest page".to_string(),
        },
        view::RecallError::Io(msg) => msg.clone(),
    }
}

/// Sanctioned `recall` render opts (design R17), complete: `now_ms`/`color`
/// for [`format_memory_hit_line`], and `query` because the no-match line
/// embeds it verbatim (`no fresh memories match "<query>"`, golden-pinned).
/// Nothing else — no root, no store, no ledger handle is reachable from
/// here or from [`render_recall`]'s signature.
struct RecallOpts<'a> {
    now_ms: u64,
    color: bool,
    query: &'a str,
}

/// `recall`'s whole two-stream output (design AC17/R15), built purely from
/// the view's typed [`view::RecallPage`] plus [`RecallOpts`] — no `&Path`,
/// no [`BlobStore`], no records slice, and no closure over any of them is
/// reachable from this signature, so a bypass of the seam is structurally
/// impossible here, not merely disciplined by convention (mirrors
/// `readcmds::render_diff`/`render_blame`'s wiring gate). Returns
/// `(stdout, stderr)`; emission order matches today's exactly — the F3
/// capped notice always precedes the empty-state branch.
///
/// NOTE: `page.store_corrupt` (F10) is never read here — a corrupt store
/// renders identically to a healthy "no fresh matches" (via `store_empty`,
/// which `recall_outcome` also leaves `false` on that path). This is
/// unchanged from pre-refactor `recall_cmd`, which never distinguished the
/// two either; F10's `store_corrupt` bit exists on `RecallPage` for a future
/// consumer (this task's Notes section: "fail-open posture preserved on the
/// CLI path"), not wired to a distinct human message yet.
fn render_recall(page: &view::RecallPage, opts: RecallOpts) -> (String, String) {
    let mut stdout = String::new();
    let mut stderr = String::new();

    // F3: the verify walk may have stopped at RECALL_VERIFY_CAP with fresh
    // matches still unreached — the page alone can't distinguish that from
    // "genuinely nothing fresh matched", so surface it explicitly. STDERR
    // (not stdout) keeps `recall`'s stdout parseable/pipeable.
    if page.capped {
        stderr.push_str(&format!(
            "verification capped at {} candidates — results may be incomplete\n",
            view::RECALL_VERIFY_CAP
        ));
    }

    if page.page.items.is_empty() {
        if page.store_empty {
            stderr.push_str("no memories yet — agentrec remember \"<fact>\" --from <file>\n");
        } else {
            stdout.push_str(&format!("no fresh memories match \"{}\"\n", opts.query));
        }
        return (stdout, stderr);
    }

    for m in &page.page.items {
        stdout.push_str(&format_memory_hit_line(m, opts.now_ms, opts.color));
        stdout.push('\n');
    }
    (stdout, stderr)
}

/// One line of `recall`'s human hit listing — same visual shape as
/// [`format_memory_line`], built from a [`view::MemoryHit`] (the `recall`
/// seam's own type) rather than an `EffectiveMemory`. `recall`'s hits are
/// always Fresh and never retracted (INV-M2), so unlike `format_memory_line`
/// this never threads a separately-derived freshness or a joined-in reason —
/// both already ride on `MemoryHit` itself. The duplication with
/// `format_memory_line` is bounded and deliberate: P4b-2's open-question-4
/// resolution already established that a shared helper across `recall` and
/// `memories` doesn't survive the seam split cleanly once one side takes a
/// view type and the other an `EffectiveMemory`.
fn format_memory_hit_line(m: &view::MemoryHit, now_ms: u64, color: bool) -> String {
    let id = fmt::paint(&short_id(&m.id), "36", color);
    let label = if m.retracted {
        "retracted"
    } else {
        m.freshness
    };
    let when = fmt::relative_time(&agentrec_core::time::rfc3339(m.ts), now_ms);
    let pins: Vec<String> = m
        .pins
        .iter()
        .map(|p| fmt::sanitize_terminal(&p.path))
        .collect();
    let mut line = format!(
        "{id}  {label:8}  {when}  {}  [pins: {}]",
        fmt::sanitize_terminal(&m.fact),
        pins.join(", ")
    );
    if let Some(r) = &m.reason {
        line.push_str(&format!("  reason: {}", fmt::sanitize_terminal(r)));
    }
    line
}

/// `memories`: full audit listing (not a query). Base set is every
/// non-retracted memory unless `all`, which also includes retracted ones;
/// `stale` then narrows that set to only non-`Fresh` (Stale or Orphaned)
/// entries. Freshness is derived per row (`memory::pin_freshness`), never
/// cached. Newest (`ts`) first. When `all` surfaces retracted entries, each
/// row's `forget --reason` (if any) is looked up via
/// [`latest_retract_reasons`] and shown alongside — `EffectiveMemory` itself
/// deliberately doesn't carry `reason` (design comment on
/// `memory::load_effective`'s fold), so this is a small parallel scan purely
/// for display.
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

    let reasons: HashMap<String, String> = if all {
        latest_retract_reasons(root)
    } else {
        HashMap::new()
    };

    if json {
        let arr: Vec<EffectiveJson> = rows
            .iter()
            .map(|(m, f)| effective_json(m, *f, reasons.get(&m.id).map(String::as_str)))
            .collect();
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
        println!(
            "{}",
            format_memory_line(
                m,
                freshness,
                now_ms,
                color,
                reasons.get(&m.id).map(String::as_str)
            )
        );
        // F7 part B, design spec §Read path: "stale rows show which pin
        // drifted and when (join against turn log)" — a bare freshness
        // label alone doesn't say WHICH of a memory's (up to 8) pins
        // drifted, or when. Fresh rows have nothing to show here.
        if freshness != Freshness::Fresh {
            for drift in pin_drifts(root, m) {
                println!("{}", format_drift_line(&drift, now_ms));
            }
        }
    }
    Ok(())
}

/// `memories --stats` (perf-evidence round, AC1.2): summarizes per-hook
/// recall latency from `memory-stats.jsonl` — the hook-owned append-only log
/// `cmds::inject_memory` writes on every UserPromptSubmit recall outcome,
/// carrying `elapsed_ms` on every site since Phase 1 of that round.
///
/// **Gates on initialization first** (finding 5, review round): an absent
/// `.agentrec/` reads `memory-stats.jsonl` as `""` via `unwrap_or_default`
/// exactly like a genuinely-empty file in an *initialized* repo would — those
/// are different facts (never-set-up vs set-up-but-quiet) and must render
/// differently. An uninitialized repo gets the same honest refusal + exit 1
/// as [`memories`]; only past that gate does an empty/absent stats file print
/// the honest `no hook invocations recorded` at exit 0.
///
/// **Deliberately does NOT call [`memories`] or `memory::load_effective`.**
/// This reads `memory-stats.jsonl` directly and only that file — a corrupt
/// `memory.jsonl` (the file `load_effective` parses) must not poison a stats
/// readout of a *different*, unrelated file. Getting this wrong (routing
/// through `memories()`'s preconditions at this function's own `load_effective`
/// call above) would make `--stats` fail on a store whose memory feature is
/// broken but whose hook latency log is perfectly readable.
///
/// Every line is sorted into exactly one of three buckets:
/// - **measurable** — parses as a JSON object AND carries `elapsed_ms` as an
///   integer. Feeds the p50/p90/p99/max (nearest-rank on the sorted sample)
///   and a per-outcome breakdown that is mutually exclusive and sums to the
///   measurable count: injected / budget_exceeded / failure / `capped_empty`
///   (a capped walk that surfaced zero fresh hits — the one case with no
///   other outcome to bucket under).
/// - **parseable-pre-upgrade** — parses as a JSON object but yields no
///   *u64* `elapsed_ms` (a line written before this phase). Precisely: the
///   key is absent, OR present with a non-u64 type, since both fail
///   `as_u64()` identically. No producer emits the wrong-typed shape today
///   — every append site writes `elapsed_ms` as a u64 — so in practice this
///   bucket is exactly the pre-phase lines; the wider wording is here so a
///   future reader is not surprised by a hand-edited or foreign line landing
///   here rather than in "torn". Counted, never silently dropped.
/// - **torn** — fails to parse as a JSON object at all. Counted, never
///   silently dropped or folded into "pre-upgrade" — conflating the two
///   would hide real corruption behind an honest-looking version skew.
///
/// **`capped_total` is a separate, cross-cutting count (finding 3, review
/// round)**, deliberately NOT one of the four mutually-exclusive buckets
/// above: `cmds::inject_memory` site 6 (a successful injection, `n` present)
/// can *also* carry `capped:true` when the verify walk hit
/// `RECALL_VERIFY_CAP` on the way to finding those hits — that line lands in
/// `injected`, not `capped_empty`, because `injected`/`capped_empty` answer
/// "what happened to this recall" (mutually exclusive by construction) while
/// `capped_total` answers a different question — "how many recalls hit the
/// verify cap at all" — which needs every line where `capped:true`,
/// regardless of which of the four buckets it landed in. Naming both
/// `capped` would silently hide a capped-but-injected recall from the
/// question `capped_total` exists to answer.
///
/// Blank lines are skipped entirely (not counted in any bucket), matching
/// the convention `status`'s own memory-stats readers already use. An
/// empty-or-absent (but initialized) file prints `no hook invocations
/// recorded` and returns — this is checked before any bucket accounting, so
/// it never fires merely because every line happened to land in one bucket.
pub fn memories_stats(root: &Path) -> Result<(), String> {
    if !crate::agentrec_dir(root).is_dir() {
        return Err("not initialized — run `agentrec init`".to_string());
    }

    let text = agentrec_core::fsguard::read_regular_to_string(&crate::memory_stats_path(root))
        .unwrap_or_default();
    if text.trim().is_empty() {
        println!("no hook invocations recorded");
        return Ok(());
    }

    let mut measurable: Vec<u64> = Vec::new();
    let mut pre_upgrade = 0usize;
    let mut torn = 0usize;
    let mut injected = 0usize;
    let mut budget_exceeded = 0usize;
    let mut failure = 0usize;
    let mut capped_empty = 0usize;
    let mut capped_total = 0usize;

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let value = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) if v.is_object() => v,
            _ => {
                torn += 1;
                continue;
            }
        };
        match value.get("elapsed_ms").and_then(|v| v.as_u64()) {
            Some(ms) => {
                measurable.push(ms);
                let is_capped = value.get("capped").and_then(|v| v.as_bool()) == Some(true);
                if is_capped {
                    capped_total += 1;
                }
                if value.get("budget_exceeded").and_then(|v| v.as_bool()) == Some(true) {
                    budget_exceeded += 1;
                } else if value.get("failure").and_then(|v| v.as_bool()) == Some(true) {
                    failure += 1;
                } else if value.get("n").is_some() {
                    injected += 1;
                } else if is_capped {
                    capped_empty += 1;
                }
            }
            None => pre_upgrade += 1,
        }
    }

    if !measurable.is_empty() {
        measurable.sort_unstable();
        let n = measurable.len();
        // Nearest-rank: rank = ceil(p/100 * n), 1-indexed into the sorted
        // sample, clamped into range (rank can't exceed n or fall below 1).
        let percentile = |p: f64| -> u64 {
            let rank = ((p / 100.0) * n as f64).ceil() as usize;
            let idx = rank.clamp(1, n) - 1;
            measurable[idx]
        };
        let max = *measurable.last().expect("non-empty checked above");
        println!(
            "{n} hook invocations recorded — p50={} p90={} p99={} max={}",
            percentile(50.0),
            percentile(90.0),
            percentile(99.0),
            max,
        );
        println!(
            "  injected={injected} budget_exceeded={budget_exceeded} failure={failure} capped_empty={capped_empty}"
        );
        println!(
            "  capped_total={capped_total} (recalls that hit RECALL_VERIFY_CAP, injected or not)"
        );
    }

    if pre_upgrade > 0 {
        println!("{pre_upgrade} pre-upgrade lines (no elapsed_ms)");
    }
    if torn > 0 {
        println!("{torn} unparseable lines skipped");
    }

    Ok(())
}

/// One drifted pin's turn-log join (design spec §Read path). `current` is
/// `None` for an orphaned pin (path no longer readable); `turn` is `None`
/// when neither join strategy in [`find_drift_turn`] finds a candidate — an
/// honest "drift time unknown", never a guess.
struct PinDrift {
    path: String,
    pinned_hash: String,
    current: Option<String>,
    turn: Option<(String, String)>, // (turn id, turn `ended` RFC3339)
}

/// Every non-fresh pin on `m`, each joined against the turn log via
/// [`find_drift_turn`]. A memory's freshness is the worst case across its
/// pins (`memory::pin_freshness`), so a Stale/Orphaned memory can still have
/// some individually-fresh pins — those are skipped here, only the pins that
/// actually drifted are returned.
fn pin_drifts(root: &Path, m: &EffectiveMemory) -> Vec<PinDrift> {
    let mut out = Vec::new();
    for pin in &m.pins {
        match memory::hash_pin(root, &pin.path) {
            Ok(current) if current == pin.hash => continue,
            Ok(current) => {
                let turn = find_drift_turn(root, &pin.path, Some(&current), m.ts);
                out.push(PinDrift {
                    path: pin.path.clone(),
                    pinned_hash: pin.hash.clone(),
                    current: Some(current),
                    turn,
                });
            }
            Err(_) => {
                let turn = find_drift_turn(root, &pin.path, None, m.ts);
                out.push(PinDrift {
                    path: pin.path.clone(),
                    pinned_hash: pin.hash.clone(),
                    current: None,
                    turn,
                });
            }
        }
    }
    out
}

/// Join a drifted pin against `log.jsonl`. Preferred: the turn whose
/// recorded `after` hash for `pin_path` equals the file's current content —
/// the turn that produced the drift, scanned newest-first so the most
/// recent producer wins if content ever repeats. Fallback (no exact-hash
/// match — e.g. an edit the watcher hasn't recorded yet, or an orphaned
/// pin, which has no current hash to match at all): the most recent turn
/// that touched `pin_path` at or after the memory's own `ts` (the pin
/// couldn't have drifted before it was pinned). `None` when neither
/// strategy finds a candidate.
fn find_drift_turn(
    root: &Path,
    pin_path: &str,
    current_hash: Option<&str>,
    pinned_ts_ms: u64,
) -> Option<(String, String)> {
    let records = agentrec_core::record::load_log(&crate::log_path(root));
    let turns: Vec<&TurnRecord> = records
        .iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        })
        .collect();

    if let Some(cur) = current_hash {
        if let Some(t) = turns.iter().rev().find(|t| {
            t.files
                .iter()
                .any(|f| f.path == pin_path && f.after.as_deref() == Some(cur))
        }) {
            return Some((t.id.clone(), t.ended.clone()));
        }
    }

    let since = agentrec_core::time::rfc3339(pinned_ts_ms);
    turns
        .iter()
        .rev()
        .find(|t| t.ended.as_str() >= since.as_str() && t.files.iter().any(|f| f.path == pin_path))
        .map(|t| (t.id.clone(), t.ended.clone()))
}

/// Render one [`PinDrift`] detail line under a `memories` row.
fn format_drift_line(d: &PinDrift, now_ms: u64) -> String {
    let path = fmt::sanitize_terminal(&d.path);
    let current_disp = d.current.as_deref().unwrap_or("missing");
    let when = match &d.turn {
        Some((turn_id, ended)) => format!(
            "drifted in turn {} at {}",
            fmt::sanitize_terminal(turn_id),
            fmt::relative_time(ended, now_ms)
        ),
        None => "drift time unknown".to_string(),
    };
    format!(
        "    {path}: pinned {} -> now {current_disp}  ({when})",
        d.pinned_hash
    )
}

/// Scan raw `memory.jsonl` for the latest (highest `ts`) `retract` record's
/// `reason` per memory id. See [`memories`]'s doc comment for why this lives
/// here rather than on `EffectiveMemory`. Malformed lines are skipped, same
/// tolerance as `memory::load_effective`; a retract with no `reason` simply
/// has no entry.
fn latest_retract_reasons(root: &Path) -> HashMap<String, String> {
    let mut out: HashMap<String, (u64, String)> = HashMap::new();
    let Ok(text) = agentrec_core::fsguard::read_regular_to_string(&memory::memory_path(root))
    else {
        return HashMap::new();
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<MemoryRecord>(line) else {
            continue;
        };
        if rec.op != MemoryOp::Retract {
            continue;
        }
        let Some(reason) = rec.reason else {
            continue;
        };
        match out.get(&rec.id) {
            Some((ts, _)) if *ts >= rec.ts => {}
            _ => {
                out.insert(rec.id, (rec.ts, reason));
            }
        }
    }
    out.into_iter()
        .map(|(id, (_, reason))| (id, reason))
        .collect()
}

/// Task 9: the recall-for-hook logic for the `agentrec recall --for-hook`
/// CLI path (Task 5, CLI `-k`) — in-process, never a subprocess. Fail-open
/// on every edge (uninitialized repo, corrupt store, any `memory::recall`
/// error): always returns `""`, never panics.
///
/// F2 (2026-07-12): `cmds::hook`'s UserPromptSubmit arm no longer calls this
/// — it uses [`recall_for_hook_with_deadline`] instead, so the hard 50ms
/// budget can be threaded in. This function stays unbounded/undeadlined; it
/// is a deliberate scope call (F2 was scoped to `inject_memory` only), not
/// an oversight — `--for-hook` is a manual CLI simulation, not the
/// wall-clock-sensitive lifecycle hook path.
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

/// Outcome of [`recall_for_hook_with_deadline`] (F2): `budget_exceeded`
/// distinguishes "the cooperative deadline tripped" (which the hook must
/// record as a stat, not treat as an ordinary no-match) from "recall
/// genuinely found nothing" (empty `block`, `budget_exceeded: false`, no
/// stat line — the existing INV-M4 fail-open contract). When
/// `budget_exceeded` is `true`, `block` is always empty.
///
/// `capped` (F3) mirrors `memory::RecallOutcome::capped` — set when the
/// verify walk hit `RECALL_VERIFY_CAP` with fresh matches possibly still
/// unreached. The caller (`cmds::inject_memory`) records it in
/// `memory-stats.jsonl` ONLY — this struct's `block` never carries the
/// capped notice text; the `--for-hook`/hook stdout contract (block or
/// nothing, exit 0 always) is unconditional and F3 does not touch it.
///
/// `store_corrupt` (F10) mirrors `memory::RecallOutcome::store_corrupt` —
/// set when `memory.jsonl` exists, is non-empty, and contains at least one
/// unreadable line or unparseable non-empty record. `block` is always empty
/// when it is `true` (a corrupt store never leaks a partial fact); the
/// caller (`cmds::inject_memory`) records ONE
/// `{"failure":true,"reason":"store_corrupt"}` stat line, distinct from
/// `budget_exceeded`.
pub struct HookRecallOutcome {
    pub block: String,
    pub budget_exceeded: bool,
    pub capped: bool,
    pub store_corrupt: bool,
}

/// F2 (founder decision, option (a)): the hard-deadline twin of
/// [`recall_for_hook`], used ONLY by `cmds::inject_memory` (the
/// `UserPromptSubmit` hook arm). Threads `deadline` into
/// `memory::recall_with_deadline`, which cooperatively bails out of its
/// internal `load_effective`/`bm25_rank`/verify loops the instant the
/// deadline passes, instead of running the full (possibly unbounded)
/// computation to completion and only discarding the *output* afterward.
/// Fail-open on every other edge exactly like `recall_for_hook`
/// (uninitialized repo, corrupt store, any `memory::recall_with_deadline`
/// error): `budget_exceeded: false`, empty block, never panics.
pub fn recall_for_hook_with_deadline(
    root: &Path,
    query: &str,
    max_facts: usize,
    deadline: Instant,
) -> HookRecallOutcome {
    if !crate::agentrec_dir(root).is_dir() {
        return HookRecallOutcome {
            block: String::new(),
            budget_exceeded: false,
            capped: false,
            store_corrupt: false,
        };
    }
    match memory::recall_with_deadline(root, query, max_facts, deadline) {
        Ok(outcome) if outcome.budget_exceeded => HookRecallOutcome {
            block: String::new(),
            budget_exceeded: true,
            capped: false,
            store_corrupt: false,
        },
        Ok(outcome) if outcome.store_corrupt => HookRecallOutcome {
            block: String::new(),
            budget_exceeded: false,
            capped: false,
            store_corrupt: true,
        },
        Ok(outcome) => HookRecallOutcome {
            block: build_hook_block(&outcome.hits, max_facts),
            budget_exceeded: false,
            capped: outcome.capped,
            store_corrupt: false,
        },
        Err(_) => HookRecallOutcome {
            block: String::new(),
            budget_exceeded: false,
            capped: false,
            store_corrupt: false,
        },
    }
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
            let pins: Vec<String> = m
                .pins
                .iter()
                .map(|p| fmt::sanitize_terminal(&p.path))
                .collect();
            format!(
                "- {}  [pins: {}]",
                fmt::sanitize_terminal(&m.fact),
                pins.join(", ")
            )
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
/// A retracted memory (only ever reached via `memories --all`) shows
/// "retracted" in place of the freshness label, plus its `forget --reason`
/// (if any) trailing the line.
fn format_memory_line(
    m: &EffectiveMemory,
    freshness: Freshness,
    now_ms: u64,
    color: bool,
    reason: Option<&str>,
) -> String {
    let id = fmt::paint(&short_id(&m.id), "36", color);
    let label = if m.retracted {
        "retracted"
    } else {
        freshness_str(freshness)
    };
    let when = fmt::relative_time(&agentrec_core::time::rfc3339(m.ts), now_ms);
    let pins: Vec<String> = m
        .pins
        .iter()
        .map(|p| fmt::sanitize_terminal(&p.path))
        .collect();
    let mut line = format!(
        "{id}  {label:8}  {when}  {}  [pins: {}]",
        fmt::sanitize_terminal(&m.fact),
        pins.join(", ")
    );
    if let Some(r) = reason {
        line.push_str(&format!("  reason: {}", fmt::sanitize_terminal(r)));
    }
    line
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
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
/// `reason` (the latest `forget --reason`, via [`latest_retract_reasons`])
/// is omitted entirely when absent — never emitted as `null` clutter on the
/// overwhelming majority of (non-retracted) rows.
#[derive(Serialize)]
struct EffectiveJson<'a> {
    id: &'a str,
    fact: &'a str,
    pins: &'a [Pin],
    origin: &'a str,
    ts: u64,
    retracted: bool,
    freshness: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
}

fn effective_json<'a>(
    m: &'a EffectiveMemory,
    freshness: Freshness,
    reason: Option<&'a str>,
) -> EffectiveJson<'a> {
    EffectiveJson {
        id: &m.id,
        fact: &m.fact,
        pins: &m.pins,
        origin: &m.origin,
        ts: m.ts,
        retracted: m.retracted,
        freshness: freshness_str(freshness),
        reason,
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

    /// Byte-slicing `id[..8]` panics when byte 8 lands mid-character (e.g.
    /// multi-byte UTF-8). A hand-edited `memory.jsonl` can carry a non-ASCII
    /// id, and the "malformed line never crashes" posture requires this to
    /// degrade gracefully, not panic. `short_id` must be char-boundary-safe.
    #[test]
    fn short_id_non_ascii_does_not_panic() {
        // "€€€a" is 3 three-byte chars + 1 one-byte char = 10 bytes; byte
        // index 8 falls inside the 3rd '€', which is exactly the panic the
        // old `id[..8]` byte-slice implementation hit.
        let id = "€€€a";
        assert_eq!(short_id(id), "€€€a");

        let longer = "€€€€€€€€€€"; // 10 chars, well past the 8-char cutoff
        assert_eq!(short_id(longer), "€€€€€€€€");
    }
}
