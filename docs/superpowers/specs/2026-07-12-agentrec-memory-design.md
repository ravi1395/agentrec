# agentrec memory — design spec

Date: 2026-07-12
Status: implemented (v1, memory Tasks 1–12 complete — see IMPLEMENTATION.md § Memory (v1))
Owner: Ravi (founder)

## Problem

Agents re-derive the same repo context every session — re-reading files, re-running
searches, re-discovering build/test/architecture facts. That is pure token cost and
latency. agentrec already records episodic history (turns), holds ground truth
(CAS + file hashes), and installs the hooks that sit at the agent's context boundary.
The missing layer is **semantic memory**: distilled, durable facts about the repo,
served back to the agent at the right moment.

The differentiator over existing memory products (mem0, Zep, Letta): every memory is
**content-addressed-invalidated**. A memory pins the file hashes it derives from; when
the code changes, the pin hash no longer matches and staleness is detected — not
guessed — and the memory is never served stale. A Fresh pin proves the file hasn't
changed since pinning, not that the fact is still true.

## Decisions log (user-confirmed; executors may not re-litigate)

1. **Consumers:** all three surfaces — hook injection (agent), CLI (human), MCP read
   tool (other agents). MCP is designed + protocol-frozen now, implemented with the
   existing v2 MCP server milestone (see §Read path).
2. **Memory scope v1:** hash-pinned facts ONLY. Every memory MUST carry
   `derived_from`-style pins. No unpinned notes, no preferences, no episode summaries.
   Pure-chat facts with no file basis are out of scope (belong to the harness /
   CLAUDE.md / vault).
3. **Write path:** manual CLI (`agentrec remember`) + agent-emitted
   `memory-candidate` signals. Both, from v1.
4. **Retrieval:** BM25/keyword, pure-local, no embeddings, no network. Revisit only
   with dogfood evidence.
5. **Stale policy:** quarantine + re-verify. Stale is never injected/served by
   default; `agentrec verify <id>` re-pins, `agentrec forget <id>` retracts.
6. **Success gate v1:** hard invariants (INV-M1..M5) + 1-week dogfood with hit-rate
   counters in `status`. Token A/B harness is a later eval round, not v1.
7. **Architecture:** Approach A — integrated feature with **lazy (read-time) pin
   verification**. No watcher coupling.
8. **Bucketing:** structural, never inferred. Per-repo store; pins must resolve
   inside the repo root; provenance via `source_turns` join. No classifier.
9. **Retro-recorded (process note, added F7):** implementing decision 8's
   `source_turns` join required a turn id that's known before the turn closes
   — the engine change that made this possible (turn id reserved at
   `observe_start`/OPEN, was previously minted lazily at close,
   `agentrec-core/src/engine.rs:OpenTurn.id`) was outside this plan's
   original task scope; it landed as an implicit prerequisite rather than a
   planned task. That change is what caused the PR #2 kill-9 duplicate-id
   bug repaired by commit `cb5dcd1` (a pre-fix daemon's orphan recovery could
   re-append a closed turn under its now-stable reserved id instead of a
   fresh one). Recorded here after the fact so the causal chain — decision 8
   → reserve-at-open → the dup-id bug class — is traceable from the spec that
   motivated it, not just from the bugfix commit log.

## Rejected approaches (with reasons — do not resurrect)

- **Eager watcher-driven invalidation (Approach B):** touches the freshly hardened
  watcher hot path; persisted stale flags can disagree with reality after a crash, so
  read-time hash verification must exist anyway — eager flags are a pure optimization
  layer, deferred until the corpus size demands it.
- **Skill-layer-only prototype (Approach C):** validates fact quality cheaply but
  produces no product, no invariants, and dogfood answers the value question anyway.
- **Local embedding retrieval in v1:** model shipping/fetching conflicts with the
  zero-network posture; 10–100x hook latency; corpus is hundreds of facts — lexical
  is likely sufficient. Measure first.
- **Persisted `status` field on memory records:** persisted status can lie after a
  crash; freshness is always derived at read time (same philosophy as the
  `modified-since` predicate).
- **Emitter-side hashing of pins:** emitter cannot pin honestly (file may change
  between emit and ingest); the recorder daemon is the trust boundary and owns
  hashing at ingestion.

## Constraints and invariants (global)

- agentrec ships **zero network code** (PROTOCOL.md §9). Nothing in this feature may
  add any.
- Append-only everything: `memory.jsonl` is never rewritten; corrections append.
- Never delete user data: `forget` appends a retraction; purge archives, never
  silently removes.
- Scrub pipeline runs at every persistence site.
- Protocol changes are additive-only within major `v` (PROTOCOL.md §10).
- Consumers never need the daemon running for reads.
- The `UserPromptSubmit` hook path is fail-open: memory failures must never break or
  delay the user's prompt beyond the latency budget.

## Architecture (Approach A)

```
WRITE                                      READ
agentrec remember (CLI) ──────┐            recall(query, k)  [agentrec-core]
                              ├─→ scrub ─→ .agentrec/memory.jsonl
Stop-hook skill → agent emits │  + pin       │  load → fold ops → pin-verify (hash
memory-candidate signal ──────┘  validate    │  now, in-memory) → BM25 fresh-only
  → signal.jsonl → daemon tailer ingests     ├─→ CLI: recall / memories / verify / forget
                                             ├─→ UserPromptSubmit hook → injected context
                                             └─→ MCP agentrec_recall (v2 server milestone)
```

New code: `agentrec-core/src/memory.rs` (types, append/read, fold, pin verification,
BM25) + `cli/src/memorycmds.rs` (verbs) + daemon signal-tailer arm + companion
Claude Code skill (emitter side). Watcher untouched. `state.json` untouched except
new counters.

## Data model

Store: `.agentrec/memory.jsonl` — append-only JSONL, perms via existing `perms.rs`
(0600 file / 0700 dir). `.agentrec/` is already watcher-denylisted, so self-writes
cannot feed back.

Record (protocol-additive, `v: 1`):

```json
{"v":1,"type":"memory","id":"<ulid>","op":"assert",
 "fact":"nightly torture seed must stay unset in CI so each night varies",
 "pins":[{"path":".github/workflows/nightly.yml","hash":"<sha256>"}],
 "source_turns":["<turn-ulid>"],"origin":"agent",
 "ts":"<utc>"}
```

- `op`: `assert` | `reverify` (new pin hashes, fact re-asserted) | `retract`
  (with `reason`). `reverify`/`retract` records reference the original `id`.
- `origin`: `human` | `agent`.
- Effective state = fold over records per id, latest op wins — the turn-correction
  pattern. Consumers MUST tolerate unknown fields.
- Freshness is **derived at read time**, never persisted: hash each pin against the
  working tree → `fresh` (all match) / `stale` (any differ) / `orphaned` (pin path
  gone; a labeled sub-case of stale).

Field constraints: `fact` ≤ 500 chars; `pins` 1–8 entries; pin paths MUST resolve
inside the repo root (reject traversal/absolute/symlink-escape); fact MUST pass
scrub before append; scrubbed-to-empty is a refusal, not a stored husk.

## Write path

### Manual

`agentrec remember "<fact>" --from <path>[,<path>...]`

- CLI hashes each path at invocation, validates in-root, scrubs fact, appends
  `origin:"human"` with flock (same discipline as other writers). No daemon needed.
- Errors: nonexistent/out-of-root path → refuse naming the exact path; fact
  scrubbed-to-empty → refuse loudly.

### Agent-emitted

New signal type (PROTOCOL.md §4, additive):

```json
{"v":1,"type":"memory-candidate","fact":"...",
 "pins":["cli/src/service.rs"],"session":"...","ts":"..."}
```

- Emitter: companion Claude Code skill instructs the agent at turn end to emit 0–3
  candidates for durable, non-obvious, file-groundable facts — explicitly NOT
  narration of what the turn did (that is the turn log's job).
- Candidates carry **paths only**; the daemon hashes at ingestion (trust boundary).
- Daemon ingestion via existing signal-tailer offset machinery: validate paths
  in-root → hash now → scrub → dedup (normalization = lowercase +
  whitespace-collapse; same normalized fact + same pin-path set already live →
  drop) → append `origin:"agent"` with `source_turns` linked to the enclosing turn.
- Reject reasons counted in `state.json` (`memory_rejects`), surfaced by `status` —
  the `snapshot_failures` honesty pattern.
- Daemon down: candidates wait in `signal.jsonl`, ingested on next start; offset
  tracking prevents double-apply.

### Quality gate (v1)

Structural only: pins resolve, length caps, dedup, scrub. No semantic truth
judgment. `origin` + `source_turns` give provenance; `forget` is the human
counterweight. Tighten only on dogfood evidence.

## Read path

Core: `recall(query, k)` in `agentrec-core` — load → fold → BM25 over `fact` text +
pin path segments → **rank-then-verify**: pin-verify candidates in rank order until
k fresh results found (verification cost scales with k, not corpus size — this is
what keeps INV-M4's 10k-record/50 ms budget honest). Stale/orphaned candidates are
skipped, never served. Cold read; no
daemon. Tokenization: lowercase, split non-alphanumeric, path segments are terms.
Empty query → recency-ordered fresh list.

**Verify cap (F3, `RECALL_VERIFY_CAP = 128`):** the rank-then-verify walk never
freshness-verifies more than 128 ranked candidates per call, regardless of `k` —
without this, a stale/orphaned-heavy corpus would hash an unbounded number of files
on every recall (unbounded I/O inside the hook's 50 ms budget). On a corpus where
the 128 highest-ranked candidates are all stale/orphaned, recall can return fewer
than `k` (even zero) results while genuinely Fresh, on-topic memories exist further
down the ranking — capping never serves a stale/orphaned candidate (INV-M2 intact),
it only ever means some Fresh ones past the cap are never reached. This is silent by
construction, so the read path surfaces it explicitly: `recall_outcome`/
`recall_with_deadline` return `capped: bool` (`RecallOutcome`), and `agentrec
recall`'s human output prints a one-line `verification capped at 128 candidates —
results may be incomplete` notice to **stderr** (never stdout — keeps stdout
parseable/pipeable) when it fires. `--json` output and `agentrec memories` (which
never verify-caps — it walks every record via `pin_freshness`, not `recall`) are
unaffected. The hook injection path (below) records `capped` as a
`memory-stats.jsonl` stat only, never as injected text.

1. **CLI:** `agentrec recall "<query>" [-k N] [--json]`;
   `agentrec memories [--stale|--all]` — stale rows show which pin drifted and when
   (join against turn log), pointing at the verify-or-forget decision.
2. **Hook injection:** the installed `UserPromptSubmit` hook additionally runs
   `agentrec recall --for-hook "<prompt>"`, stdout injected as context.
   - Budget: max 5 memories / ~800 chars (`[memory] inject_max` in config.toml).
   - BM25 score floor: no strong match → inject NOTHING.
   - Hard 50 ms self-budget; corrupt/missing store → inject nothing, exit 0.
     Failures increment a counter, never break the prompt.
   - Format: fenced `agentrec memory` block, one fact per line + pin paths; no
     ids/hashes in injected text.
   - Verify cap (F3): a capped walk (see above) records `capped: true` in the
     `memory-stats.jsonl` line it appends — including the zero-hit case, where
     a bare `{"ts","capped":true}` line lands even though no block is
     injected. Never appears in stdout; the block-or-nothing/exit-0 contract
     is unconditional.
3. **MCP:** `agentrec_recall`, read tier — added to PROTOCOL.md §8 table + schema
   now (frozen), implemented when the v2 MCP server lands.

Kill-switch: `memory_enabled = false` disables injection + candidate ingestion;
store stays readable.

## Lifecycle

- `agentrec verify <id>`: show fact + pin drift (old→new hash; diff summary via CAS
  where snapshots exist); `--confirm` appends `reverify` with current hashes.
  Agent-driven reverify is deferred to the MCP round (trust question).
- `agentrec forget <id>`: appends `retract`. Never deletes bytes.
- Purge: `purge --memories-retracted` compacts fully-retracted chains past TTL into
  the `.agentrec.archived.<ts>` pattern — archive, never silent removal. No TTL on
  live memories: pins are their staleness signal, not time.
- Orphaned pins: stale-with-label; `verify` can re-pin to a successor file or
  retract.

## Security

- Scrub at both write sites; planted-secret test extended to `memory.jsonl`
  (existing 3-location grep test gains a 4th location).
- Pins are paths + hashes only, never content. Secret-file denylist paths (`.env*`,
  `*.pem`, etc.) are rejected as pins outright — even a hash of `.env` leaks a
  change-detection signal.
- Injection is local stdout into a local hook — no new trust surface. Existing
  terminal-escape sanitization applies to fact rendering.
- No network code added anywhere.

## Testing / acceptance

Invariants (each maps to ≥1 automated test; add to IMPLEMENTATION.md AC register):

- **INV-M1:** no memory record exists without ≥1 valid in-root pin — fuzz malformed
  candidates (traversal, absolute paths, symlink escape, empty pins).
- **INV-M2:** stale/orphaned memories are never emitted by the injection path —
  mutate a pinned file, recall again, memory gone.
- **INV-M3:** planted secret never lands in `memory.jsonl` via either write path.
- **INV-M4:** hook path exits 0 and within budget under: corrupt store, missing
  store, 10k-record store, concurrent append.
- **INV-M5:** fold determinism — same records ingested in any order produce the
  same effective state (property test).
- Torture harness: memory ops (remember / mutate / recall / verify / kill-9) joined
  into the existing 1200-op run; INV-M1/M2 asserted at checkpoints.

Success gate: all invariants green + 1-week dogfood in this repo with counters in
`status` (injections / rejects / stale-quarantined). Token A/B harness = later eval
round.

## Performance envelope (low-end target: 8 GB laptop)

Baseline measured on the live v0.1.0 daemon (29 h run): 9.8 MB idle RSS, 0.018 %
idle CPU, ~100 ms per 50-file mutation burst, sha256 at 555 MB/s. Budgets this
feature must hold:

- Daemon idle RSS ≤ 25 MB including the live dedup set (facts are ≤ 500 chars;
  10k facts ≈ 3 MB — nothing resident scales with repo size).
- No new timers, no polling: ingestion is signal-tailer-driven; recall is a
  transient process (~5–10 MB RSS, exits).
- Recall hook: 50 ms hard self-budget via rank-then-verify (§Read path).
  Optional `(path, mtime, size)` hash cache is an advisory fast-path only —
  mtime can lie; any cache miss or doubt falls back to full hashing.
- `memory.jsonl` disk growth is KB–low-MB; CAS remains the only large store and
  keeps its existing 2 GiB eviction cap.

## Phasing (independently mergeable)

- **P1** — `memory.rs` core (types, fold, pins, BM25) + `remember`/`recall`/
  `memories` CLI. Dogfoodable alone.
- **P2** — `memory-candidate` signal + daemon ingestion + companion skill.
  PROTOCOL.md additive bump rides here.
- **P3** — `UserPromptSubmit` hook injection + budgets + fail-open tests.
- **P4** — `verify`/`forget` lifecycle + purge integration + torture join +
  dogfood counters.

## Open questions (non-blocking)

- Injected-block exact wording/format — settle in P3 against real Claude Code
  rendering.
- Whether `memories --stale` should hint a suggested successor pin for renamed
  files (CAS content match) — nice-to-have, not v1.
