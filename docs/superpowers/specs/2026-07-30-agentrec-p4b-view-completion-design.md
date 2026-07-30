# P4b — `RepositoryView` completion (diff / blame / recall / wire `list`)

> **Status:** design, awaiting binding skeptic gate. Written 2026-07-30 against
> `feat/phase-2-0-substrate` @ `638290d`. Parent spec:
> `docs/superpowers/specs/2026-07-18-agentrec-phase-2-design.md` (§Deep module 1).
> Parent plan: `docs/superpowers/plans/2026-07-25-agentrec-phase-2-0.md`.
> Conflict order: PROTOCOL.md > IMPLEMENTATION.md > parent spec > this doc.

## Why this phase exists

Phase 2.0's parent spec §Deep module 1 defines `RepositoryView` as a six-method seam:

```rust
open, list, diff, blame, recall, health
```

P4 (merged `638290d`, skeptic GATE PASS) delivered `open`, `list`, `health` — and the three
deletions its acceptance criteria actually asserted. It did **not** deliver `diff`, `blame`, or
`recall`, which appeared only in P4.md's prose "Interface contract" block. `list` was delivered
but left with **zero callers**.

This is not a retroactive attack on P4's gate. P4's verdict was sound for what its ACs
asserted; the ACs were the defect. See §Why P4 gate-PASSed, which is a load-bearing part of
this design, not an aside.

The immediate consequence is that **P5 cannot be executed as written.** P5.md states
"DEPENDENCY: P4 must be merged — this phase is adapters over P4's typed values", and its AC1
requires `serde_json` of "the exact typed value returned by `RepositoryView`". No such value
exists for `diff` or `blame`. A fresh-context P5 executor would either build the missing
view.rs plumbing inside P5's commit (scope drift, and the gate would not catch it — same AC
blind spot as P4) or hand-build JSON off the print logic (direct AC violation).

## Measured starting state (verified 2026-07-30, not asserted)

| Fact | Evidence |
|---|---|
| Test baseline | `cargo test --workspace -- --test-threads=3` → **537 passed, 0 failed, 2 ignored** |
| Golden suite | 27 tests over 24 golden files (`cli/tests/golden.rs`, `cli/tests/fixtures/golden/`) |
| `view.rs` public methods | `open`, `ledger`, `health`, `health_of`, `list` — plus free functions `recording_gaps`, `has_crash_gap`, `crash_gap_count`, `has_gap_after`, `resolve_turn`, `same_revert`, `load_ledger` |
| `DiffQuery` / `BlameQuery` / `RecallQuery` / `FileDiff` / `BlameResult` / `MemoryHit` | `rg` across `agentrec-core/src` + `cli/src` → **0 matches**; none exist |
| `view::list` callers | `rg '\.list\('` outside `view.rs` → **0 matches**; dead code |
| `diff` / `blame` shape | `cli/src/readcmds.rs:20` / `:296` — `println!` direct, `Result<(), String>` |
| `recall` shape | `cli/src/memorycmds.rs:526` — calls `memory::recall_outcome` directly |
| `log --json` shape | `cli/src/cmds.rs:99` — inline `serde_json::to_string(turn)` |
| `recall --json` golden | **none** — `cli/tests/fixtures/golden/` has `log_json`, `status_json`, no recall |

The "21 passed" golden figure carried in P3.md / P4.md / P5.md is stale (actual 27); P4b's own
verification section states measured deltas against 537 / 27, and corrects those three files.

## Gap inventory

### In scope (each has a Phase 2.0 consumer)

- **G1 — `RepositoryView::diff` missing.** No `DiffQuery`, no `FileDiff`.
- **G2 — `RepositoryView::blame` missing.** No `BlameQuery`, no `BlameResult`.
- **G3 — `RepositoryView::recall` missing.** No `RecallQuery`, no `MemoryHit`.
- **G4 — `view::list` has zero callers.** `cmds::log` re-implements it: `load_log`
  (`cmds.rs:57`) plus the merged-turn and git-turn filters (`cmds.rs:66-67`). Wiring this is not
  scope creep — `TurnQuery::include_all`'s own doc comment ("include turns superseded by a
  retroactive merge, and git turns") describes those two filters verbatim. `list` was built for
  that call site. An unwired seam is an unverified seam, which is precisely how P4's drift
  survived a gate.
- **G5 — `log --json` hand-rolls its serializer.** A fourth one-serializer violation, outside
  P5's scope (P5 covers diff/blame/status only). The parent spec's MCP table requires
  `agentrec_log` to mirror the `--json` contract exactly, through the same serializer.

### Out of scope, with the reason recorded so a fresh executor cannot resurrect it

- **`UndoCoordinator`** — parent plan's wave-2 table names it explicitly *together with this
  exact trap*: "Its full contract exists to serve MCP destructive (2.3). Bound it explicitly or
  a fresh-context executor builds the whole 2.3 ledger early."
- **Report-safe sanitization** — parent spec ties it to `ReportBuilder`, a Phase 3 module that
  "Phase 2 must not pre-build, only avoid foreclosing".
- **L3 sidecar merge** — no L3 emitter exists. Codex is Phase 2.1 at L2, and the format freeze
  lands only after 2.1's exit (spec decision 5).
- **`FORMAT-CHANGELOG.md`, `import aider`, git trailers + shim, npm/mise wrappers** — all named
  wave-2 items in the parent plan.
- **Blob integrity** — not separate work; it rides with `diff`, which already reads through the
  integrity-checked `BlobStore`.
- **The memory hook path** — `recall_for_hook`, `recall_for_hook_with_deadline`, and
  `cmds::inject_memory` are untouched. Different consumer, different budget contract
  (`RECALL_BUDGET_MS`, INV-M4, hardened by F8's detached-thread hard wall). Routing a
  50 ms-budgeted fail-open path through a read seam buys nothing and risks a regression in the
  one place the repo has already had to harden twice.
- **Line *ranges* in blame** — the parent spec's MCP table says "optional line/range", but
  `readcmds::parse_target` is single-line only today. `BlameQuery` carries
  `line: Option<usize>`; range support is deferred to whichever phase needs it, named here so
  it is a known deferral rather than a silent omission.

## Why P4 gate-PASSed without diff/blame/recall

Every P4 acceptance criterion is one of two shapes:

1. A **negative-space** assertion — `rg 'fn has_recording_gap|fn has_gap_after|fn count_gaps'
   cli/src` → 0 matches; `rg 'fn same_revert|fn resolve_turn' cli/src` → 0 matches.
2. A **golden-byte** or behavioral check — goldens byte-identical, `health()` writes nothing,
   human `status` still evicts, cursor staleness, parse tolerance.

Not one AC asserted that a method **exists**, or that an adapter **calls** it. The three
deletions were gated, so the three deletions landed. The three additions were prose, and prose
carries no gate weight in this repo's AC style.

**The design consequence, which is the whole point of recording this:** P4b expresses every
addition as an adapter-side negative-space AC — "the old path is gone from the adapter" — not as
an interface-contract block. The interface block below exists for P5's benefit as a consumer
contract; it carries **zero** AC weight, and this doc says so explicitly so no future executor
mistakes it for one again.

## Interface contract (consumed by P5 and, later, MCP 2.2)

Unchanged from P4 — correct as built:

```rust
RepositoryView::open(root: &Path)          -> Result<RepositoryView, RepoError>
RepositoryView::list(q: &TurnQuery)        -> Result<Page<TurnSummary>, CursorError>
RepositoryView::health(budget: u64)        -> Result<RepositoryHealth, RepoError>
```

P4b additions:

```rust
RepositoryView::list_records(q: &TurnQuery) -> Result<Page<TurnRecord>, CursorError>
RepositoryView::diff(q: &DiffQuery)         -> Result<Page<FileDiff>, DiffError>
RepositoryView::blame(q: &BlameQuery)       -> Result<BlameResult, RepoError>
RepositoryView::recall(q: &RecallQuery)     -> Result<RecallPage, RecallError>
```

Query types mirror the parent spec's MCP tool inputs, so 2.2 needs no second interpretation:

```rust
pub struct DiffQuery   { pub turn: String, pub paths: Option<Vec<String>>,
                         pub limit: Option<usize>, pub after: Option<Cursor> }
pub struct BlameQuery  { pub path: String, pub line: Option<usize> }
pub struct RecallQuery { pub query: String, pub k: usize, pub after: Option<Cursor> }
pub struct RecallPage  { pub page: Page<MemoryHit>, pub capped: bool,
                         pub store_corrupt: bool }
```

`blame` is deliberately unpaginated: one path (optionally one line) yields one result. This
matches the parent spec's own signature and its MCP table ("one line/range result").

## Decisions (founder-confirmed 2026-07-30; executors may not re-litigate)

1. **P4b is a distinct phase, sequenced before P5.** Not folded into P5 (P5's file budget is
   `readcmds.rs`/`cmds.rs`/`main.rs`/`README.md`; recall additionally touches `memorycmds.rs`),
   and not deferred to MCP 2.2 (which would leave P5's stated dependency false and let the
   one-serializer invariant stay violated while more code stacks on it).
2. **Scope is all three missing methods at once**, not diff/blame first and recall later. One
   re-open, not two.
3. **`recall --json` output stays byte-identical.** Same discipline P4 applied to diff/blame/
   status human forms: `RepositoryView::recall` is a seam swap underneath a stable contract, not
   a contract change. `MemoryHit` therefore carries `EffectiveJson`'s exact serde shape
   (`id`, `fact`, `pins`, `origin`, `ts`, `retracted`, `freshness`, and `reason` omitted-when-absent).
4. **The memory hook path is out of scope** (rationale above, under out-of-scope).
5. **Two typed values for turns, not one.** `TurnSummary` is a lossy projection — it drops `v`,
   `truncated`, `model`, `session`, `root`, `prompt_ref`, `prompt_excerpt`, `merges`,
   `files_complete`, and reduces `files[]` to a count. `log --json` emits the **full**
   `TurnRecord` today (pinned in `log_json.golden`), so routing it through
   `Page<TurnSummary>` would silently drop ten fields. `list()` keeps `Page<TurnSummary>` and
   serves the human `log` line plus MCP 2.2's bounded "≤200 turn summaries";
   `list_records()` returns `Page<TurnRecord>` and serves `log --json`.
6. **`log --json`'s empty/non-empty inconsistency is fixed, as consistent JSONL.** Today the
   empty case prints `[]` (a JSON array) while the non-empty case prints one object per line
   (JSONL) — `cmds.rs:71-76` vs `:99`. `log_json.golden` pins only the non-empty case, so the
   empty case is unpinned in either direction. The empty case becomes **zero lines of stdout**.
   `log_json.golden` bytes are unchanged; a new golden pins the empty case. Rejected: making
   both a JSON array (changes `log_json.golden` bytes, breaks decision 3's discipline and any
   line-oriented consumer); keeping the wart (MCP 2.2 would mirror it permanently).
7. **`recall --json` goldens are captured before any refactor.** Decision 3 is unfalsifiable
   without an instrument — the same failure mode as P4's prose contract. Commit 1 captures them
   against current code, touching zero production files. This mirrors the P3→P4 pattern that
   made byte-equivalence a real claim rather than an assertion.
8. **Signature drift resolves toward the code; the parent spec is amended.**
   `health(budget: u64)` stays: budget comes from config, and the view deliberately holds no
   config (re-coupling it would also disturb P5's `status --json` AC surface).
   `list -> CursorError` stays: it is strictly more precise than `RepoError`.
9. **Per-method error enums, not one blanket error type.** `view.rs`'s own module doc states
   errors are typed "so the human CLI keeps ownership of its prose". `diff` has three genuine
   failure classes (`LookupError` for unknown/ambiguous turn, `CursorError`, IO); collapsing
   them forces the adapter to re-guess which prose to print — exactly the coupling this seam
   exists to remove.
10. **`recall` returns `RecallPage`, not the parent spec's bare `Page<MemoryHit>`.** `capped`
    (F3) drives a real stderr notice on the human path and `store_corrupt` (F10) is a distinct
    recall-failure signal; returning only a `Page` would regress both. Amended in the parent
    spec alongside decision 8.

## Commit sequence

1. **Goldens only — zero production files.** Capture `recall --json` (populated, no-match
   against a non-empty store, empty store) and the `log --json` empty case, against current
   code. Makes decisions 3 and 6 falsifiable before anything moves.
2. **`diff` + `blame` into `view.rs`**, adapters rewritten as render-only. `readcmds.rs` stops
   loading the log and stops touching `BlobStore`.
3. **`recall` into `view.rs`** + adapter. `MemoryHit` carries `EffectiveJson`'s serde shape.
4. **G4 + G5:** `cmds::log` → `list()` (human) and `list_records()` (`--json`); the empty-case
   JSONL fix from decision 6.
5. **Docs:** parent spec §Deep module 1 amendment (decisions 8 and 10); `P5.md` dependency line
   corrected `P4` → `P4b`; the stale "21 passed" golden figure corrected in P3.md/P4.md/P5.md;
   `README.md` if any documented flag behavior changed; `CLAUDE.md` Status per house rule.

## Acceptance criteria

Negative-space, adapter-side — the only AC shape that held in P4:

- [ ] **AC1** `rg 'load_log|BlobStore' cli/src/readcmds.rs` → **0 matches**. `diff` and `blame`
      obtain every value from `RepositoryView`; the adapter only renders.
- [ ] **AC2** `rg 'load_log' cli/src/cmds.rs` → **0 matches on the `log` path**
      (`cmds::log` reaches the ledger only through `list`/`list_records`).
- [ ] **AC3** `rg 'memory::recall' cli/src/memorycmds.rs` → matches **only** inside the
      hook-path functions (`recall_for_hook`, `recall_for_hook_with_deadline`); the
      `recall_cmd` path routes through `RepositoryView::recall`.
- [ ] **AC4** `rg 'serde_json::to_string' cli/src` → **0 matches on the `log` and `recall`
      paths**; both serialize the typed value the view returned.

Positive, byte-pinned:

- [ ] **AC5** All 27 pre-existing goldens byte-identical at the final commit — including
      `log_json.golden`, which decision 5 exists to protect.
- [ ] **AC6** The 4 goldens captured in commit 1 are byte-identical after commits 2–4. RED
      proof required: `recall --json` routed through a `MemoryHit` that omits any
      `EffectiveJson` field must fail AC6 (demonstrate once in-test, then fix).
- [ ] **AC7** `log --json` on a zero-turn repo emits **zero bytes** on stdout, exit 0 — pinned
      by a new golden. The pre-change `[]` behavior is gone and no golden still asserts it.
- [ ] **AC8** A field added to `TurnRecord` appears in `log --json` with no adapter edit; a
      field added to `MemoryHit` appears in `recall --json` with no adapter edit (both proven by
      adding a temp field in-test).
- [ ] **AC9** `RepositoryView::diff` on an unknown turn ref returns `DiffError::Lookup(Unknown)`
      and on a duplicated id returns the collapsed turn (not `Ambiguous`) — the P3
      duplicate-collapse golden still passes, and a distinct-turns-sharing-an-id fixture still
      errors ambiguous.
- [ ] **AC10** `RepositoryView::blame` on an uncovered path returns a `BlameResult` carrying the
      recording-gap state with **no** attributor — never a guess. The gap-honesty goldens are
      unchanged.
- [ ] **AC11** `RecallPage::capped` is set when the verify walk stops at `RECALL_VERIFY_CAP`,
      and the human adapter still prints the F3 stderr notice; `--json` still omits it
      (decision 3 — byte-identical).
- [ ] **AC12** `RepositoryView::recall` performs **zero writes**: `memory.jsonl` length and
      `memory-stats.jsonl` mtime unchanged across a call.
- [ ] **AC13** The hook path is untouched: `git diff --stat` over the phase shows no change to
      `recall_for_hook`, `recall_for_hook_with_deadline`, or `cmds::inject_memory`, and
      `hook_recall_hard_wall_deadline` passes unmodified.

## Verification

Assert **deltas against the measured baseline** (537 / 0 / 2 and 27 goldens), not absolute
counts — every prior task file's absolute figure has gone stale:

```
cargo test -p agentrec-core view::                        # baseline + new diff/blame/recall tests
cargo test -p agentrec --test golden                      # 27 + 4 new = 31 passed, 0 changed bytes
cargo test --workspace -- --test-threads=3                # 537 + delta passed, 0 failed, 2 ignored
cargo clippy --workspace --all-targets -- -D warnings     # clean, debug and release
cargo fmt --check
```

Manual, on the dogfood repo: `agentrec log --json | jq -s .` parses; `agentrec recall --json
<query> | jq .` parses and matches pre-change output byte-for-byte; `agentrec log --json` in a
zero-turn tempdir emits nothing, exit 0.

## Edge cases

- Zero-turn repo (`log`, `log --json`, `diff`, `blame`).
- Log containing only epoch records.
- `diff` on a fileless turn (P5 will need `{"files":[]}`; P4b must return an empty
  `Page<FileDiff>`, not an error).
- Duplicated turn id (collapse) vs distinct turns sharing an id (ambiguous) — AC9.
- `recall` against an empty store vs a non-empty store with no fresh match — distinct outputs
  today; both pinned in commit 1.
- Corrupt `memory.jsonl` → `store_corrupt`, fail-open posture preserved on the CLI path.
- Cursor replayed across a `purge --log-duplicates` rewrite → `stale_cursor` (already proven for
  `list`; `list_records` and `diff` inherit it and must not silently diverge).
- `NO_COLOR` / non-TTY — affects human rendering only; JSON paths unaffected.

## Risks

- **Highest risk is a repeat of P4's failure mode**: an executor satisfies the negative-space
  ACs by deleting the old path while the new seam is thin or partially bypassed. AC5–AC8 are the
  counterweight — they fail if behavior moved at all.
- `readcmds.rs` is large and blame is the most intricate logic in it (`blame_line`'s exact-line-
  text heuristic, with a documented v1 limitation). Extraction must preserve that heuristic
  verbatim; the blame goldens are the instrument.
- Commit 4 changes user-visible `log --json` behavior in the empty case. It is a deliberate,
  founder-decided fix (decision 6) and rides with its own golden and a README line if the flag
  is documented there.
