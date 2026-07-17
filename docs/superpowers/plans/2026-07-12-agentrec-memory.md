# agentrec memory — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Hash-pinned semantic memory for agentrec — facts pinned to file hashes, lazily verified at read time, written manually + by agents, recalled via CLI and injected via the UserPromptSubmit hook.

**Architecture:** New `agentrec-core/src/memory.rs` (types, fold, pin verification, BM25, rank-then-verify recall) + `cli/src/memorycmds.rs` (CLI verbs + config scanners) + one routing arm in the daemon signal tailer + an extension of `cmds::hook`'s UserPromptSubmit path. Watcher untouched. Spec: `docs/superpowers/specs/2026-07-12-agentrec-memory-design.md` — its Decisions log and Rejected approaches are binding; executors may not re-litigate.

**Tech Stack:** Rust (existing workspace), serde/serde_json, no new dependencies. BM25 built from scratch (no crate).

## Global constraints (from spec — apply to every task)

- Zero network code. No new dependencies without founder approval.
- `memory.jsonl` is append-only; corrections append (`reverify`/`retract`). Never rewrite, never delete bytes.
- Scrub (`agentrec_core::scrub::scrub`) at every persistence site; scrubbed-to-empty = refusal.
- Every created file/dir goes through `perms::lock_file`/`perms::lock_dir` (the `record::open_append` path already does this).
- Freshness is derived at read time, never persisted.
- Hook path is fail-open: memory errors must never break the prompt or exit nonzero.
- Timestamps: epoch-ms u64 (`wall_now_ms` pattern), consistent with `SignalEvent.ts`.
- Protocol changes additive-only (PROTOCOL.md §10).
- `cargo clippy -- -D warnings` + `cargo fmt` clean at every commit.
- Constants: `FACT_MAX_CHARS = 500`, `PINS_MAX = 8`, `INJECT_MAX_DEFAULT = 5`, `INJECT_BUDGET_CHARS = 800`, `RECALL_BUDGET_MS = 50`.

## Executor protocol

One task = one commit (message given per task). TDD: write the test step first, watch it fail, implement, watch it pass. Run `cargo test -p <crate>` scoped as given, plus `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check` before each commit. If a step is ambiguous against the spec, STOP and ask — never invent semantics. Pattern pointers are `file:symbol` — read the referenced symbol before writing code that mirrors it.

## Hazard register (read before Tasks 6–7)

- **Spurious-stop:** the tailer parses every `signal.jsonl` line as `SignalEvent`; anything that is not `event:"start"` currently falls through to the stop arm (`cli/src/daemon.rs:apply_signal`). A memory-candidate line MUST be routed away before `apply_signal` or it fabricates a turn closure. Task 6 exists to prevent this; its regression test is non-negotiable.
- **state.json single writer:** only the daemon writes `state.json`. The hook process must never write it (Task 9's injection counter uses a separate append-only stats file instead).

---

### Task 1: memory.rs — record types, fsynced append, fold

**Files:**
- Create: `agentrec-core/src/memory.rs`
- Modify: `agentrec-core/src/lib.rs` (add `pub mod memory;`), `agentrec-core/src/record.rs` (expose synced line append), `IMPLEMENTATION.md` (AC register)
- Test: inline `#[cfg(test)] mod tests` in `memory.rs`

**Interfaces (Produces — later tasks consume exactly these):**

```rust
pub const FACT_MAX_CHARS: usize = 500;
pub const PINS_MAX: usize = 8;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pin { pub path: String, pub hash: String }   // hash = "sha256:<64-hex>"

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryOp { Assert, Reverify, Retract }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MemoryRecord {
    pub v: u32,                       // 1
    #[serde(rename = "type")] pub kind: String,   // always "memory"
    pub id: String,                   // ulid; reverify/retract reference the assert's id
    pub op: MemoryOp,
    pub fact: String,
    pub pins: Vec<Pin>,
    #[serde(default)] pub source_turns: Vec<String>,
    pub origin: String,               // "human" | "agent"
    pub ts: u64,                      // epoch ms
    #[serde(default, skip_serializing_if = "Option::is_none")] pub reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct EffectiveMemory {
    pub id: String, pub fact: String, pub pins: Vec<Pin>,
    pub origin: String, pub ts: u64,          // ts of latest op
    pub retracted: bool,
}

pub fn memory_path(root: &Path) -> PathBuf;                       // .agentrec/memory.jsonl
pub fn append_memory(root: &Path, rec: &MemoryRecord) -> Result<(), String>;  // fsynced
pub fn load_effective(root: &Path) -> Result<Vec<EffectiveMemory>, String>;   // fold; includes retracted (flagged)
```

- Consumes: `record.rs` append plumbing; `id::ulid()`.

**Steps:**

- [ ] **Step 1: AC register.** Add to IMPLEMENTATION.md a `### Memory (v1)` AC section listing INV-M1..M5 verbatim from the spec §Testing, each with "maps to test: <named test added in Tasks 1–12>". Fill test names as tasks land (this task fills INV-M5's).

- [ ] **Step 2: Failing tests** (fold semantics — INV-M5 core):

```rust
#[test]
fn fold_latest_op_wins_any_order() {
    // assert(id=A, fact F1) + reverify(id=A, new pins) + retract(id=A)
    // shuffled into every permutation ordered by ts: effective state identical —
    // retracted=true, pins = reverify's pins. Build records in-memory, write to a
    // tempdir memory.jsonl in each permutation, assert load_effective equal.
}
#[test]
fn fold_ignores_unknown_fields_and_bad_lines() {
    // a line with extra fields parses; a malformed JSON line is skipped (counted,
    // not fatal); a record with unknown op string is skipped. load_effective still
    // returns the good records.
}
#[test]
fn append_memory_rejects_oversize_and_empty() {
    // fact > FACT_MAX_CHARS -> Err naming the cap; pins empty or > PINS_MAX -> Err;
    // fact whose scrub().trim() is empty -> Err containing "scrub".
}
```

- [ ] **Step 3: Run, verify FAIL** — `cargo test -p agentrec-core memory::` → 3 failed (module missing → compile fail first is fine).

- [ ] **Step 4: Implement.** In `record.rs`: extract `append_log`'s body into `pub(crate) fn append_line_synced(path: &Path, line: &str) -> Result<(), String>` (open_append + write + `sync_all`), make `append_log` call it; make it `pub` for the workspace (`pub fn`). In `memory.rs`: types above; `append_memory` validates (caps, scrub-empty, op/id invariants) then serializes one line via `append_line_synced`. `load_effective`: read file (absent = empty vec), parse lines individually (skip bad), group by `id`, fold ordered by `ts` (tie: file order), latest op wins; `reverify` replaces pins, `retract` sets flag. Mirror `cmds.rs:merged_ids` for the grouping style.

- [ ] **Step 5: Run, verify PASS** — `cargo test -p agentrec-core memory::` → `3 passed`.

- [ ] **Step 6: Commit** — `feat(core): memory record types, fsynced append, deterministic fold (INV-M5)`

### Task 2: pin validation + freshness

**Files:**
- Modify: `agentrec-core/src/memory.rs`
- Test: inline

**Interfaces (Produces):**

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Freshness { Fresh, Stale, Orphaned }

/// Reject: absolute, `..` traversal, symlink escaping root, secret-file paths,
/// nonexistent file. Returns root-relative normalized path.
pub fn validate_pin_path(root: &Path, path: &str) -> Result<String, String>;

/// Hash file at root/rel now. "sha256:<hex>" via store::hash_bytes.
pub fn hash_pin(root: &Path, rel: &str) -> Result<String, String>;

/// All pins match -> Fresh; any path missing -> Orphaned; any hash differs -> Stale.
/// (Orphaned checked first; it is a labeled sub-case of stale.)
pub fn pin_freshness(root: &Path, pins: &[Pin]) -> Freshness;
```

- Consumes: `store::hash_bytes(bytes) -> String`; `scrub::is_secret_path(path) -> bool`.

**Steps:**

- [ ] **Step 1: Failing tests** (INV-M1 core):

```rust
#[test]
fn pin_path_rejections() {
    // in a tempdir root: "/etc/passwd" (absolute), "../x" and "a/../../x" (traversal),
    // symlink inside root pointing outside (unix: create with std::os::unix::fs::symlink)
    // -> Err mentioning the offending path. ".env" and "key.pem" -> Err mentioning "secret".
    // nonexistent "ghost.rs" -> Err. "src/ok.rs" (created) -> Ok("src/ok.rs").
}
#[test]
fn freshness_transitions() {
    // pin file, hash -> Fresh. Overwrite content -> Stale. Delete file -> Orphaned.
    // Two pins, one changed -> Stale.
}
```

- [ ] **Step 2: FAIL** — `cargo test -p agentrec-core memory::pin` → 2 failed.
- [ ] **Step 3: Implement.** Path validation: reject `Path::new(p).is_absolute()` and any `..` component; join to root, `fs::canonicalize` both, require `starts_with(canonical_root)` (this catches symlink escape); `scrub::is_secret_path` gate. Mirror the strict-validation posture of `store.rs:BlobStore` object-path hardening (A1) — malformed input never touches disk logic.
- [ ] **Step 4: PASS** — `cargo test -p agentrec-core memory::` → all green.
- [ ] **Step 5: Commit** — `feat(core): pin validation + derived freshness (INV-M1 core)`

### Task 3: BM25 + rank-then-verify recall

**Files:**
- Modify: `agentrec-core/src/memory.rs`
- Test: inline

**Interfaces (Produces):**

```rust
pub const SCORE_FLOOR: f64 = 0.8;   // BM25 floor; tune in Task 9 against dogfood

/// Tokenize: lowercase, split on non-alphanumeric, drop empties.
/// Doc text = fact + all pin path segments ("cli/src/service.rs" -> cli, src, service, rs).
pub fn tokenize(text: &str) -> Vec<String>;

/// BM25 (k1=1.2, b=0.75) over non-retracted memories. Returns (index, score),
/// score >= SCORE_FLOOR only, sorted desc. Empty query -> empty.
pub fn bm25_rank(corpus: &[EffectiveMemory], query: &str) -> Vec<(usize, f64)>;

/// load_effective -> bm25_rank -> walk rank order, pin_freshness each candidate,
/// keep Fresh until k collected. Empty query -> k freshest by ts (recency order).
pub fn recall(root: &Path, query: &str, k: usize) -> Result<Vec<EffectiveMemory>, String>;
```

**Steps:**

- [ ] **Step 1: Failing tests** (INV-M2 core + ranking sanity):

```rust
#[test]
fn recall_never_returns_stale() {
    // two memories pinning distinct files, both matching query "nightly seed".
    // recall -> both. Mutate one pinned file. recall -> exactly the other. (INV-M2)
}
#[test]
fn bm25_ranks_specific_over_generic_and_floors_noise() {
    // corpus: fact about "torture seed nightly", fact about "install script", fact
    // about "doctor inotify". query "nightly torture seed" -> torture fact first;
    // query "zebra quantum" -> empty (floor).
}
#[test]
fn recall_verifies_only_top_candidates() {
    // corpus of 50; query matches 3 strongly. Instrument via pins: make 40 low-score
    // memories pin a deleted file (Orphaned). recall(k=3) returns the 3 fresh matches —
    // proving orphaned low-rankers were never needed. (rank-then-verify shape)
}
```

- [ ] **Step 2: FAIL** → 3 failed. **Step 3: Implement** (textbook BM25, avg-doc-len over corpus; no index persistence — build per call, corpus is small). **Step 4: PASS** — `cargo test -p agentrec-core` → all green (core suite still 48+ passing).
- [ ] **Step 5: Commit** — `feat(core): BM25 + rank-then-verify recall, fresh-only (INV-M2 core)`

### Task 4: CLI `remember`

**Files:**
- Create: `cli/src/memorycmds.rs`
- Modify: `cli/src/main.rs` (mod + `Command::Remember` + match arm)
- Test: `cli/tests/integration.rs`

**Interfaces:**
- Produces: `memorycmds::remember(root: &Path, fact: &str, from: &str) -> Result<(), String>` (`from` = comma-separated paths); clap variant `Remember { fact: String, #[arg(long)] from: String }`.
- Consumes: Task 1–2 (`append_memory`, `validate_pin_path`, `hash_pin`); `scrub::scrub`.

**Steps:**

- [ ] **Step 1: Failing integration tests** (drive real binary via `integration.rs:agentrec` helper; init fixture via `integration.rs:init`):

```rust
#[test]
fn remember_writes_pinned_scrubbed_record() {
    // init repo, write src/a.rs; run: remember "build needs cargo nightly" --from src/a.rs
    // -> exit 0. memory.jsonl exists, 0600 (unix), one record: origin "human",
    // pin path "src/a.rs", hash "sha256:" + 64 hex, op assert.
}
#[test]
fn remember_refuses_bad_pins_and_secret_facts() {
    // --from ../escape -> exit 1, stderr names path, memory.jsonl absent.
    // --from .env -> exit 1, stderr mentions secret.
    // fact "AKIA..." (AWS-key shape, reuse fixture from
    // integration.rs:secret_prompt_never_reaches_disk_in_cleartext) with valid pin ->
    // record persisted but fact contains "[redacted:" and never the raw key (INV-M3 half 1);
    // fact that is ONLY a secret -> exit 1 "scrubbed to empty", nothing written.
}
```

- [ ] **Step 2: FAIL** — `cargo test --test integration remember` → 2 failed.
- [ ] **Step 3: Implement.** `remember`: split `from`, validate each (`validate_pin_path`), hash each, scrub fact (refuse if `scrub(fact).trim().is_empty()`), build `MemoryRecord{origin:"human", source_turns: vec![], ...}` with `id::ulid()`, `append_memory`. Wire clap arm mirroring `main.rs:Command::Purge` dispatch shape. Errors to stderr, exit 1 via the existing `Result<(), String>` main pattern.
- [ ] **Step 4: PASS.** **Step 5: Commit** — `feat(cli): agentrec remember — manual pinned memories`

### Task 5: CLI `recall` + `memories`

**Files:**
- Modify: `cli/src/memorycmds.rs`, `cli/src/main.rs`
- Test: `cli/tests/integration.rs`

**Interfaces:**
- Produces: `memorycmds::recall_cmd(root, query: &str, k: usize, json: bool, for_hook: bool) -> Result<(), String>`; `memorycmds::memories(root, stale: bool, all: bool, json: bool) -> Result<(), String>`; clap `Recall { query, #[arg(short, default_value="5")] k, #[arg(long)] json, #[arg(long, hide=true)] for_hook }`, `Memories { #[arg(long)] stale, #[arg(long)] all, #[arg(long)] json }`. `--for-hook` output contract (consumed verbatim by Task 9): fenced block

  ````
  ```agentrec memory
  - <fact>  [pins: <p1>, <p2>]
  ```
  ````
  capped at `memory_inject_max` facts / 800 chars total; **empty stdout when no hit ≥ floor**. No ids/hashes in hook output; plain listing (id, freshness, fact, pins, relative ts via `fmt.rs` helpers) for humans; `--json` = array of effective records + freshness.
- Consumes: Task 3 `recall`; Task 1 `load_effective`; Task 2 `pin_freshness`; `fmt.rs` relative-time + NO_COLOR conventions.

**Steps:**

- [ ] **Step 1: Failing integration tests:**

```rust
#[test]
fn recall_cli_fresh_only_and_json() {
    // remember 2 facts pinning 2 files; recall "<query matching both>" --json -> 2 entries;
    // mutate one pin; recall again -> 1; memories --stale -> shows the stale one with
    // its drifted pin path; memories --all -> both.
}
#[test]
fn recall_for_hook_emits_block_or_nothing() {
    // matching query -> stdout starts "```agentrec memory", <= 800 chars, no "sha256:";
    // nonsense query -> stdout EMPTY, exit 0. Uninitialized repo -> stdout empty, exit 0
    // when --for-hook (fail-open), exit 1 with stderr otherwise.
}
```

- [ ] **Step 2: FAIL.** **Step 3: Implement** (zero-state honesty: bare `recall` on empty store prints `no memories yet — agentrec remember "<fact>" --from <file>` to stderr, exit 0 — PD3 posture). **Step 4: PASS** — `cargo test --test integration memor recall` green.
- [ ] **Step 5: Commit** — `feat(cli): recall + memories verbs; --for-hook contract (dogfoodable P1)`

### Task 6: memory-candidate signal — format, routing, spurious-stop guard

**Files:**
- Modify: `agentrec-core/src/record.rs` (`SignalEvent` fields), `cli/src/daemon.rs` (routing), `PROTOCOL.md` (§4 additive + §8 `agentrec_recall` read-tier row)
- Test: `cli/tests/hardening_daemon.rs`

**Interfaces:**
- Produces: `SignalEvent` gains
  ```rust
  #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")] pub kind: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")] pub fact: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")] pub pins: Option<Vec<String>>,
  ```
  plus `pub fn is_memory_candidate(&self) -> bool` (`kind.as_deref() == Some("memory-candidate")`). Daemon loop: candidates routed to `ingest_candidate` (Task 7 stub: `fn ingest_candidate(...)` no-op behind a compile-firewall), **never** into `apply_signal`.
- Consumes: `daemon.rs:SignalTailer::poll`, `daemon.rs:apply_signal`.

**Steps:**

- [ ] **Step 1: Failing test (the hazard-register test — non-negotiable):**

```rust
#[test]
fn memory_candidate_signal_never_closes_a_turn() {
    // init + spawn_record; send_hook start (opens bracket); append a memory-candidate
    // signal line to signal.jsonl (kind=memory-candidate, fact, pins, NO event field);
    // mutate a repo file; poll: NO turn closes (bracket still suppresses quiet window);
    // then send_hook stop -> exactly ONE rich turn. A legacy SignalEvent with no kind
    // still closes normally (regression pair).
}
```

- [ ] **Step 2: FAIL** (candidate line, `event:None`, currently hits the stop arm → premature close). **Step 3: Implement** fields + routing guard **before** `apply_signal` in the daemon's poll-dispatch loop. PROTOCOL.md: §4 gains the memory-candidate signal (fields, "paths only — recorder hashes at ingestion"), §8 table gains `agentrec_recall | read`, §10 note: additive, schemas stay `v:1`. Same commit as code (house rule).
- [ ] **Step 4: PASS** — `cargo test --test hardening_daemon memory_candidate` + full `cargo test --workspace` (no existing signal test regresses).
- [ ] **Step 5: Commit** — `feat(protocol): memory-candidate signal type; route around stop arm (spurious-stop guard)`

### Task 7: daemon ingestion + rejects counter

**Files:**
- Modify: `cli/src/daemon.rs` (`ingest_candidate` real body), `cli/src/state.rs` (`memory_rejects: u64`, `#[serde(default)]`), `cli/src/cmds.rs` (`status` memory line)
- Test: `cli/tests/hardening_daemon.rs`

**Interfaces:**
- Produces: `fn ingest_candidate(root: &Path, state: &mut State, sig: &SignalEvent, current_turn: Option<&str>)` — validate each pin path (Task 2), hash now, scrub fact, dedup (lowercase+whitespace-collapsed fact + pin-path set vs `load_effective` live set → drop), append `origin:"agent"`, `source_turns` = enclosing open turn id if any. Any rejection → `state.memory_rejects += 1` + `write_state` (daemon is the only state writer — this runs on the daemon thread). `status` prints `memory: N fresh, M stale, R rejects` (fresh/stale by scan, rejects from state).
- Consumes: Tasks 1–2, 6; `state.rs:record_io_failure` pattern; `state::write_state`.

**Steps:**

- [ ] **Step 1: Failing tests:**

```rust
#[test]
fn candidate_ingestion_end_to_end() {
    // spawn_record; emit candidate signal (valid pin) -> poll_until memory.jsonl has
    // origin "agent" record with daemon-computed hash + source_turns non-empty when
    // emitted inside an open bracket; duplicate candidate -> still 1 record.
}
#[test]
fn candidate_rejects_counted_never_fabricated() {
    // candidates with: traversal pin, .env pin, secret-only fact, 9 pins ->
    // memory.jsonl gains NOTHING (INV-M1), state.json memory_rejects == 4,
    // status stdout contains "4 rejects".
}
#[test]
fn candidate_secret_fact_scrubbed_on_disk() {
    // AWS-key-shaped fact + valid pin -> persisted fact has "[redacted:", raw key
    // absent from memory.jsonl AND signal.jsonl grep is allowed to contain it
    // (inbox is pre-scrub by design for prompts? NO — candidates are agent-authored:
    // emit via Task 8 CLI which scrubs BEFORE append; this test asserts raw secret in
    // NEITHER file. See Task 8 note.)   (INV-M3 half 2)
}
```

- [ ] **Step 2: FAIL.** **Step 3: Implement.** **Step 4: PASS** — suite green, `status` line verified in test output.
- [ ] **Step 5: Commit** — `feat(daemon): candidate ingestion — validate, hash, scrub, dedup; rejects counter (INV-M1/M3)`

### Task 8: `agentrec candidate` emitter CLI + companion skill

**Files:**
- Modify: `cli/src/memorycmds.rs`, `cli/src/main.rs`
- Create: `claude-setup/skills/agentrec-memory/SKILL.md`
- Test: `cli/tests/integration.rs`

**Interfaces:**
- Produces: `memorycmds::candidate(root, fact: &str, from: &str, tool: &str) -> Result<(), String>` — scrubs fact FIRST (defence in depth: secret never enters even the inbox), light-validates (length, 1–8 paths syntactically), appends memory-candidate `SignalEvent` line via `record::append_log_line` (unsynced — inbox exemption D34). Clap: `Candidate { fact, #[arg(long)] from, #[arg(long, default_value="agent")] tool }`. Skill file: instructs agent, at end of a substantive turn, to run `agentrec candidate "<durable, non-obvious, file-grounded fact>" --from <files>` for 0–3 facts; explicitly forbids narrating what the turn did; staged in `claude-setup/` (installed manually, like ratchet.md).
- Consumes: Task 6 `SignalEvent` fields; Task 7 ingestion (end-to-end).

**Steps:**

- [ ] **Step 1: Failing test:**

```rust
#[test]
fn candidate_cli_round_trip() {
    // spawn_record; run: candidate "fact about nightly" --from .github/workflows/nightly.yml
    // -> signal.jsonl line has type memory-candidate + paths-only pins (no hashes);
    // poll_until memory.jsonl has the agent record. Daemon down variant: emit with no
    // daemon, then spawn_record -> ingested on startup replay (offset machinery).
}
```

- [ ] **Step 2: FAIL.** **Step 3: Implement + write SKILL.md** (≤ 40 lines: trigger = turn end; quality bar with 2 good / 2 bad examples; hard rules: file-groundable only, no narration, ≤ 3 per turn). **Step 4: PASS.**
- [ ] **Step 5: Commit** — `feat(cli): candidate emitter verb + companion skill (write path complete)`

### Task 9: hook injection

**Files:**
- Modify: `cli/src/cmds.rs` (`hook`), `cli/src/memorycmds.rs` (config scanners), `cli/src/initcmd.rs` (`DEFAULT_CONFIG`)
- Test: `cli/tests/integration.rs`

**Interfaces:**
- Produces: in `cmds::hook`, UserPromptSubmit arm only — after appending the start signal, if `read_memory_enabled(root)` (default true), call `memorycmds::recall_for_hook(root, prompt_raw)` in-process (no subprocess) and print its block to stdout; total added wall time self-measured, bail to empty output past `RECALL_BUDGET_MS`. Config scanners mirroring `purgecmd::read_ttl_days`: `read_memory_enabled(root) -> bool` (`memory_enabled`, default true), `read_memory_inject_max(root) -> usize` (`memory_inject_max`, default 5). `DEFAULT_CONFIG` gains both keys with comments. Every injection appends one line `{"ts":<ms>,"n":<facts injected>}` to `.agentrec/memory-stats.jsonl` (append-only, hook-owned — daemon never touches it; O_APPEND small-write atomicity, same posture as signal inbox). Stop arm untouched.
- Consumes: Task 5 `--for-hook` logic (shared fn, not subprocess); `cmds.rs:hook` stdin shape (`hook_event_name`, `prompt`).

**Steps:**

- [ ] **Step 1: Failing tests** (INV-M4):

```rust
#[test]
fn hook_injects_fresh_memories_into_stdout() {
    // remember a fact pinning README.md; send UserPromptSubmit payload whose prompt
    // matches -> stdout contains "```agentrec memory" + the fact; signal.jsonl still
    // gains the start signal (existing behavior intact); memory-stats.jsonl gained a line.
    // Non-matching prompt -> stdout empty. memory_enabled=false in config -> empty.
}
#[test]
fn hook_fail_open_and_budget() {
    // corrupt memory.jsonl (garbage bytes) -> hook exit 0, stdout empty, start signal
    // still appended. 10k-record store (generated) -> exit 0 within budget: assert
    // wall time < 500ms in test (CI slack; 50ms self-budget internally). Missing store,
    // uninitialized .agentrec -> exit 0. (INV-M4)
}
```

- [ ] **Step 2: FAIL.** **Step 3: Implement.** **Step 4: PASS + manual smoke:** in this repo run `echo '{"hook_event_name":"UserPromptSubmit","prompt":"nightly torture seed"}' | agentrec hook claude` → block printed. Paste output into PR/commit body.
- [ ] **Step 5: Commit** — `feat(cli): UserPromptSubmit memory injection — budgeted, fail-open (INV-M4)`

### Task 10: lifecycle — `verify` + `forget`

**Files:**
- Modify: `cli/src/memorycmds.rs`, `cli/src/main.rs`
- Test: `cli/tests/integration.rs`

**Interfaces:**
- Produces: `memorycmds::verify(root, id: &str, confirm: bool) -> Result<(), String>` — resolve id (prefix match allowed, ambiguous → error listing candidates, mirroring `readcmds` turn-id resolution); print fact + per-pin drift (`old sha256:ab.. -> new sha256:cd..`, or `deleted`); `--confirm` appends `reverify` with current hashes (orphaned pins must be dropped explicitly via `--drop-pin <path>` or the verify refuses — a reverify never silently un-pins). `memorycmds::forget(root, id, reason: Option<&str>)` appends `retract`. Both refuse on already-retracted (honest error, exit 1).
- Consumes: Tasks 1–2.

**Steps:**

- [ ] **Step 1: Failing test:**

```rust
#[test]
fn verify_and_forget_lifecycle() {
    // remember -> mutate pin -> memories --stale shows it -> verify <id> (no --confirm)
    // prints drift, changes nothing -> verify <id> --confirm -> fresh again, recall
    // serves it -> forget <id> --reason "wrong" -> excluded from recall AND injection,
    // memories --all shows retracted with reason; forget again -> exit 1.
    // Orphan variant: delete pinned file -> verify --confirm refuses without --drop-pin.
}
```

- [ ] **Step 2: FAIL.** **Step 3: Implement.** **Step 4: PASS.**
- [ ] **Step 5: Commit** — `feat(cli): verify/forget lifecycle — quarantine is recoverable`

### Task 11: purge integration + status counters complete

**Files:**
- Modify: `cli/src/purgecmd.rs`, `cli/src/main.rs` (flag), `cli/src/cmds.rs` (status)
- Test: `cli/tests/hardening_cli.rs`

**Interfaces:**
- Produces: `purge --memories-retracted` — chains fully retracted for > `ttl_days` move to `.agentrec/memory.archived.<unix_ts>.jsonl` (append archived lines, then rewrite memory.jsonl WITHOUT them — the ONLY sanctioned rewrite, done atomically: write tmp, fsync, rename, mirroring `store.rs` atomic-blob pattern; archive written+fsynced BEFORE source rewrite). Live + stale-but-unretracted memories are NEVER touched. `status` memory line final form: `memory: N fresh, M stale, R rejects, I injections` (I = memory-stats.jsonl line count).
- Consumes: `purgecmd::run` + `read_ttl_days`; `uninstallcmd` archive-naming pattern.

**Steps:**

- [ ] **Step 1: Failing test:**

```rust
#[test]
fn purge_archives_only_expired_retracted_chains() {
    // build memory.jsonl with: live fact, stale fact, retracted-yesterday fact,
    // retracted-100-days-ago fact (ts forged). purge --memories-retracted ->
    // only the 100-day chain moved to memory.archived.*.jsonl (all its records,
    // assert+retract); memory.jsonl retains the other three intact byte-for-byte
    // (compare remaining lines); second run -> no-op. kill -9 between archive write
    // and rewrite (simulate: run the two halves via test hooks or assert archive
    // exists before rename in code order) -> memory.jsonl still valid superset (no loss).
}
```

- [ ] **Step 2: FAIL.** **Step 3: Implement.** **Step 4: PASS + full workspace green.**
- [ ] **Step 5: Commit** — `feat(cli): purge --memories-retracted (archive-never-delete) + status counters`

### Task 12: torture join, secret 4th location, docs

**Files:**
- Modify: `cli/tests/torture.rs`, `cli/tests/integration.rs` (extend `secret_prompt_never_reaches_disk_in_cleartext`), `README.md`, `IMPLEMENTATION.md` (AC register test names final), spec (mark implemented)
- Test: the above

**Steps:**

- [ ] **Step 1: Torture ops.** Add to the randomized op mix: `remember` (random fact + pin to a tracked fixture file), `mutate-pinned-file`, `recall` (assert INV-M2: no returned memory has a drifted pin — re-hash to check), `forget`, `candidate` (when daemon alive). After every op batch, assert INV-M1 (every memory.jsonl record parses with ≥1 pin) and INV-M2. kill-9 recovery already in harness — memory.jsonl must never contain a torn/invalid line after recovery (fsynced appends).
- [ ] **Step 2: Run smoke** — `cargo test --test torture torture_smoke` → pass; full `--ignored` run once locally, paste op-count + 0-violations output into commit body.
- [ ] **Step 3: Secret 4th location.** Extend the planted-secret test to also grep `memory.jsonl` + `memory-stats.jsonl` after driving remember/candidate with the secret fixture → raw secret nowhere (INV-M3 complete).
- [ ] **Step 4: Docs.** README: `remember`/`recall`/`memories`/`verify`/`forget`/`candidate` rows in the command table + a "Memory" section (what pins are, staleness guarantee, kill-switch key); IMPLEMENTATION.md AC register: every INV-M1..M5 row now names its test(s); spec status line → implemented.
- [ ] **Step 5: Gate + commit.** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check` → all green. Commit — `test+docs: memory torture join, secret 4th location, README/AC register (memory v1 complete)`

---

## Post-plan gates (not tasks — round exit criteria)

- `skeptical-reviewer` agent pass against INV-M1..M5 + spec Decisions log before any "done" claim (house rule).
- VERIFY-LEDGER.md rows: (1) real Claude Code session shows injected block in context (manual, this repo); (2) 1-week dogfood with `status` counters — hit-rate observed, no false staleness; (3) skill-driven candidate emission produces ≥1 useful memory in real use.
- Update CLAUDE.md Status section (house rule).

## Self-review notes (done at write time)

- Spec coverage: data model→T1, pins/freshness→T2, retrieval→T3, manual write→T4, read CLI→T5, protocol+routing→T6, ingestion→T7, emitter+skill→T8, injection→T9, lifecycle→T10, purge/counters→T11, torture/secret/docs→T12. MCP `agentrec_recall`: protocol table row in T6 (implementation deferred to v2 MCP server per spec decision 1 — no task, by design).
- Type consistency: `EffectiveMemory`/`Pin`/`Freshness`/`recall` signatures identical across T1–T5, T9–T10 consumers.
- No placeholder steps; test snippets are the acceptance criteria (house plan style — implementation bodies intentionally intent-level with `file:symbol` pattern pointers, per global CLAUDE.md which overrides the skill's full-code preference).
