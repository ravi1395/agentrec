# P4b — `RepositoryView` completion (diff / blame / recall / wire `list`)

> **Status:** design, revision 3 — amended after skeptic rounds 1 (FAIL, 5 blocking) and 2
> (FAIL, 3 blocking, all in revision 2's new material). Every finding independently verified
> against the tree before amending. Written 2026-07-30 against `feat/phase-2-0-substrate` @
> `638290d`. Parent spec:
> `docs/superpowers/specs/2026-07-18-agentrec-phase-2-design.md` (§Deep module 1).
> Parent plan: `docs/superpowers/plans/2026-07-25-agentrec-phase-2-0.md`.
> Conflict order: PROTOCOL.md > IMPLEMENTATION.md > parent spec > this doc.
>
> **Revision 2 changelog** (§Skeptic round 1 records the full findings):
> R1 dropped decision 6 — it would have reversed the gate-PASSED D-PD4; R2 retargeted G4 from
> `log` to `status`, whose filter `list()` actually fits; R3 scoped AC1 around `show`/`undo`,
> which revision 1 failed to inventory; R4 replaced the unmechanizable `rg`-path ACs with
> positive behavioral gates; R5 added `store_empty` to `RecallPage`; R6 corrected the
> §Why P4 gate-PASSed thesis, which was literally false as first written.
>
> **Revision 3 changelog** (§Skeptic round 2 records the full findings):
> R7 corrected decision 5 / G4 again — `status_report` has **two** turn consumers, and the
> eviction path needs unfiltered full records (revision 2 inventoried only the rich-rate
> window, leaving a **blob-eviction data-loss path**); R8 rewrote AC3, which was jointly
> unsatisfiable with AC6 and wrong on ordering and on `recall`'s target; R9 added positive
> wiring ACs for `diff`/`blame`, which revision 2 pinned by nothing load-bearing — P4's exact
> failure mode had survived for two of the three new methods.

## Why this phase exists

Phase 2.0's parent spec §Deep module 1 defines `RepositoryView` as a six-method seam:

```rust
open, list, diff, blame, recall, health
```

P4 (merged `638290d`, skeptic GATE PASS) delivered `open`, `list`, `health` — and the three
deletions its acceptance criteria actually asserted. It did **not** deliver `diff`, `blame`, or
`recall`, which appeared only in P4.md's prose "Interface contract" block. `list` was delivered
but left with **zero callers**.

This is not a retroactive attack on P4's gate. P4's verdict was sound for what its ACs asserted;
the ACs had a hole. See §Why P4 gate-PASSed.

The immediate consequence is that **P5 cannot be executed as written.** P5.md states
"DEPENDENCY: P4 must be merged — this phase is adapters over P4's typed values", and its AC1
requires `serde_json` of "the exact typed value returned by `RepositoryView`". No such value
exists for `diff` or `blame`, and P5's own file budget (`readcmds.rs`/`cmds.rs`/`main.rs`/
`README.md`) forbids the `view.rs` edits needed to create them. A fresh-context P5 executor
would either violate P5's file list or hand-build JSON off the print logic — a direct AC
violation. *(Skeptic round 1 confirmed this diagnosis as correct and not overstated.)*

## Measured starting state (verified 2026-07-30; re-verified by skeptic round 1)

| Fact | Evidence |
|---|---|
| Test baseline | `cargo test --workspace -- --test-threads=3` → **537 passed, 0 failed, 2 ignored** |
| Golden suite | 27 tests over 24 golden files (`cli/tests/golden.rs`, `cli/tests/fixtures/golden/`) |
| `view.rs` public methods | `open`, `ledger`, `health`, `health_of`, `list` — plus free functions `recording_gaps`, `has_crash_gap`, `crash_gap_count`, `has_gap_after`, `resolve_turn`, `same_revert`, `load_ledger` |
| `DiffQuery` / `BlameQuery` / `RecallQuery` / `FileDiff` / `BlameResult` / `MemoryHit` | `rg` across `agentrec-core/src` + `cli/src` → **0 matches**; none exist |
| `view::list` callers | `rg '\.list\('` outside `view.rs` → **0 matches**; dead code |
| `diff` / `blame` shape | `readcmds::diff` / `readcmds::blame` — `println!` direct, `Result<(), String>` |
| `recall` shape | `memorycmds::recall_cmd` — calls `memory::recall_outcome` directly |
| `log --json` shape | `cmds::log` — inline `serde_json::to_string(turn)` |
| `recall --json` golden | **none** — `cli/tests/fixtures/golden/` has `log_json`, `status_json`, no recall |
| `readcmds.rs` ledger/blob users | `rg -c 'load_log\|BlobStore' cli/src/readcmds.rs` → **17** — `diff`, **`show`**, `blame`, **`undo`/`execute_revert`**, and shared helpers `print_entry`, `load_blob`, `load_text` |
| `status_report` duplicates `list()`'s filter | `cmds::status_report` re-derives `merged_ids` + superseded filter + `tool != "git"` — `TurnQuery{include_all:false}` verbatim |
| `log --json` empty case **is** pinned | `cli/tests/integration.rs::log_json_zero_turns_prints_empty_array` asserts `[]`; **D-PD4**, `HANDOVER-PD-FIXES.md:20` |

Revision 1 asserted the `log --json` empty case was unpinned. It is pinned — by an integration
test, not a golden; revision 1 checked only golden files. Revision 1 also inventoried only
`diff`/`blame` as `readcmds.rs`'s ledger users, missing `show` and `undo`. Both errors are
corrected above and both changed the design (R1, R3).

Pointers in this document are `file:symbol`, not line numbers, per the house rule that lines rot
(revision 1 violated this).

The "21 passed" golden figure carried in P3.md / P4.md / P5.md is stale (actual 27); this phase's
doc commit corrects those three files.

## Gap inventory

### In scope (each has a Phase 2.0 consumer)

- **G1 — `RepositoryView::diff` missing.** No `DiffQuery`, no `FileDiff`.
- **G2 — `RepositoryView::blame` missing.** No `BlameQuery`, no `BlameResult`.
- **G3 — `RepositoryView::recall` missing.** No `RecallQuery`, no `MemoryHit`.
- **G4 — `view::list` has zero callers; `cmds::status_report` re-implements it.**
  `status_report` derives `merged_ids`, filters superseded ids, and filters `tool != "git"` —
  which is `TurnQuery{include_all:false}` verbatim, matching `TurnQuery::include_all`'s own doc
  comment ("include turns superseded by a retroactive merge, and git turns").

  **`status_report` has TWO turn consumers, and they need different types** (R7 — revision 2
  inventoried only the first, and skeptic round 2 showed the omission opens a data-loss path):

  | Consumer | Set | Fields needed | Served by |
  |---|---|---|---|
  | turn count, rich-rate window | **filtered** (`turns`) | `imported`, `grade`, `tool`, `.len()` | `list(&TurnQuery{include_all:false})` — all present on `TurnSummary`, no widening |
  | eviction protect-set (`retention::enforce_budget`) | **unfiltered** (`all_turns`) | `prompt_ref` and every `files[]` blob hash | `view.ledger().records`, or `list_records(&TurnQuery{include_all:true})` — **never** the filtered page |

  `enforce_budget` protects `prompt_ref` from **any** turn (its own comment: "protect any blob
  referenced as a prompt by ANY turn, not just kept ones") and derives snapshot hashes via
  `turn_snapshot_hashes`. `TurnSummary` carries neither `prompt_ref` nor `files[]`.

  **This is a data-loss path, not a golden break.** An executor who routes `status` wholesale
  through `list()` and derives `owned_turns` from the filtered page hands `enforce_budget` a
  protect-set missing every superseded and git turn's blobs; eviction then deletes blobs still
  referenced by superseded turns, breaking `undo` of merged history. AC16 exists to make that
  RED.
- **G5 — `log --json` hand-rolls its serializer.** A fourth one-serializer violation, outside
  P5's scope (P5 covers diff/blame/status only). Justified by the one-serializer invariant
  itself — *not* by an MCP mirror: under decision 12 the parent spec's `agentrec_log` row
  mirrors `list()`'s summary contract, not `log --json`.

### Out of scope, with the reason recorded so a fresh executor cannot resurrect it

- **`UndoCoordinator` and everything `undo` touches** — parent plan's wave-2 table names it
  explicitly *together with this exact trap*: "Its full contract exists to serve MCP destructive
  (2.3). Bound it explicitly or a fresh-context executor builds the whole 2.3 ledger early."
  `readcmds::undo` and `execute_revert` therefore keep their direct `load_log`/`BlobStore` use;
  AC1 is scoped accordingly (R3).
- **`agentrec show`** — `readcmds::show` also reads the ledger and blobs directly, and carries
  D-PD2 prompt-posture semantics (bare header vs `--prompt` full post-scrub text, `--all-files`
  noise reveal) that no view method models. Designing a prompt-blob read into the seam is real
  work with a prompt-posture surface; it is **deferred and named**, not silently omitted. `show`
  keeps its direct reads this phase.
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
  `readcmds::parse_target` is single-line only today. `BlameQuery` carries `line: Option<usize>`;
  range support is deferred, named here so it is a known deferral.
- **The `log --json` array/JSONL asymmetry** — empty prints `[]`, non-empty prints one object
  per line. Genuinely inconsistent, but **D-PD4 chose the empty-case `[]` deliberately** ("blank
  stdout is not valid JSON and breaks any consumer that parses it") and that decision is
  gate-PASSED. Recorded as a known contract wart that MCP 2.2 will mirror; not fixed here
  (decision 6, R1).

## Why P4 gate-PASSed without diff/blame/recall

The precise failure — corrected from revision 1, which overstated it:

P4 **did** have positive, behavior-asserting ACs, and they held. Its AC4 names `health()` and
asserts it performs zero writes (RED without the split); its cursor AC is exercised by tests
that call `view.list()`. Revision 1 claimed "not one AC asserted that a method exists" — that is
literally false.

The real hole is narrower and more instructive: **`diff`, `blame`, and `recall` had no AC of any
kind.** They existed only in P4.md's prose "Interface contract" block, and prose carries no gate
weight in this repo. Everything P4 gated, P4 delivered.

**The design consequence:** the load-bearing gates for this phase are **positive behavioral
ACs** — the shape that actually worked in P4 — not negative-space `rg` assertions. Revision 1
over-rotated into `rg` ACs, and skeptic round 1 demonstrated two ways to game them (see §Skeptic
round 1, B1 and B4). `rg` assertions remain in the AC set, but only where they are literally
runnable and go to zero, and they are supplementary. The interface block below exists as a
consumer contract for P5; it carries **zero** AC weight, and this sentence exists so no future
executor mistakes it for one again.

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
                         pub store_corrupt: bool, pub store_empty: bool }
```

`blame` is deliberately unpaginated: one path (optionally one line) yields one result, matching
the parent spec's signature and its MCP table ("one line/range result").

`store_empty` (R5) is load-bearing: `recall_cmd` today calls `memory::load_effective` directly to
split "no memories yet" (PD3 zero-state, stderr) from "no fresh memories match" (stdout).
Without it on `RecallPage`, the adapter must reach around the seam to render its own empty
states — a half-bypassed seam that revision 1's ACs did not catch.

## Decisions (executors may not re-litigate)

1. **P4b is a distinct phase, sequenced before P5.** Not folded into P5 (P5's file budget is
   `readcmds.rs`/`cmds.rs`/`main.rs`/`README.md`; recall additionally touches `memorycmds.rs`),
   and not deferred to MCP 2.2 (which would leave P5's stated dependency false and let the
   one-serializer invariant stay violated while more code stacks on it).
2. **Scope is all three missing methods at once**, not diff/blame first and recall later.
3. **`recall --json` output stays byte-identical.** `RepositoryView::recall` is a seam swap
   underneath a stable contract. `MemoryHit` carries `EffectiveJson`'s exact serde shape
   (`id`, `fact`, `pins`, `origin`, `ts`, `retracted`, `freshness`, `reason` omitted-when-absent).
   `capped`/`store_corrupt`/`store_empty` ride on `RecallPage`, **not** in the JSON — today's
   `--json` deliberately omits the capped notice, and that omission is preserved.
4. **The memory hook path is out of scope** (rationale under out-of-scope).
5. **Two typed values for turns, with distinct consumers.** `TurnSummary` is a lossy projection —
   it drops `v`, `truncated`, `model`, `session`, `root`, `prompt_ref`, `prompt_excerpt`,
   `merges`, `files_complete`, and reduces `files[]` to a count.
   - `list() -> Page<TurnSummary>` serves **`status`'s counting surface** — turn count and the
     rich-rate window (G4's first row) — and, later, MCP 2.2's bounded "≤200 turn summaries".
   - `list_records() -> Page<TurnRecord>` serves **both `log` forms** and **`status`'s eviction
     path**. The human `log` line needs `prompt_excerpt`, `truncated`, and `files_complete`
     (`fmt::turn_list_line`) and the noise fold needs per-file paths (`cmds::log`); `log --json`
     emits the full record, pinned by `log_json.golden`; the eviction protect-set needs
     `prompt_ref` + `files[]` over the **unfiltered** set (G4's second row).
   - `status` therefore makes **two** view calls, or one `ledger()` read plus one `list()` — it
     is not a single-query consumer. Revision 1 wrongly claimed `list()` serves human `log`
     (R2); revision 2 wrongly claimed `TurnSummary` covers all of `status` (R7).

   Rejected: widening `TurnSummary` to cover the human `log` line — the parent spec's rule
   "prompt contents are opt-in data, never included in generic summaries" makes
   `prompt_excerpt`-in-a-summary-type a direct collision.
6. **Withdrawn (R1).** Revision 1 proposed changing `log --json`'s empty case from `[]` to zero
   lines. That reverses **D-PD4** (`HANDOVER-PD-FIXES.md:20`, gate-PASSED, pinned by
   `integration.rs::log_json_zero_turns_prints_empty_array`, rationale "blank stdout is not
   valid JSON"), on the false premise that the case was unpinned. `[]` stays; the empty-case
   test is not weakened, deleted, or inverted. G5 narrows to *routing* `log --json` through the
   view's serializer without touching the empty-case contract. The asymmetry is a recorded wart
   (see out-of-scope).
7. **`recall --json` goldens are captured before any refactor** — decision 3 is unfalsifiable
   without an instrument, the same failure mode as P4's prose contract. Commit 1 captures them
   against current code, touching zero production files, mirroring the P3→P4 pattern.
8. **Signature drift resolves toward the code; the parent spec is amended.**
   `health(budget: u64)` stays: budget comes from config, and the view deliberately holds no
   config (re-coupling it would also disturb P5's `status --json` AC surface).
   `list -> CursorError` stays: strictly more precise than `RepoError`. Both amendments *add*
   precision — they are ratchet-ups, not loosenings.
9. **Per-method error enums, not one blanket error type.** `view.rs`'s module doc states errors
   are typed "so the human CLI keeps ownership of its prose". `diff` has three genuine failure
   classes (`LookupError` unknown/ambiguous turn, `CursorError`, IO); collapsing them forces the
   adapter to re-guess which prose to print — the coupling this seam exists to remove.
10. **`recall` returns `RecallPage`, not the parent spec's bare `Page<MemoryHit>`** — `capped`
    (F3), `store_corrupt` (F10), and `store_empty` (PD3) each drive real adapter behavior that a
    bare `Page` would drop. Amended in the parent spec alongside decision 8.
11. **`show` and `undo` keep their direct ledger/blob reads this phase** (R3), with the reasons
    recorded under out-of-scope. AC1 is scoped to the `diff`/`blame` surface accordingly rather
    than demanding a file-wide zero it cannot honestly reach.
12. **The parent spec's `agentrec_log` MCP row is amended in the same doc commit.** It promises
    "≤200 turn **summaries**" while also promising read tools "mirror the Phase 2.0 `--json`
    contracts exactly … same typed values, one serializer". Under decision 5 those are
    irreconcilable for `agentrec_log`: `log --json` is full records, MCP summaries are
    `TurnSummary`. The amendment states that `agentrec_log` mirrors `list()`'s summary contract,
    and defers a full-record tool or parameter.

## Commit sequence

Five commits, not one. The parent plan's "one phase = one commit" protocol is deviated from
deliberately and this is the sanctioned exception: commit 1 must contain **zero production
changes** for its goldens to be a valid pre-refactor instrument, exactly as P3's goldens were
for P4.

1. **Goldens only — zero production files.** Capture `recall --json` (populated; no-match against
   a non-empty store; empty store) **and** the two human `recall` empty-state outputs
   ("no memories yet" on stderr vs "no fresh memories match" on stdout), which are the states
   `store_empty` exists to preserve. Makes decision 3 and R5 falsifiable before anything moves.
2. **`diff` + `blame` into `view.rs`**, adapters rewritten as render-only for those two verbs.
   README rides this commit if any documented behavior of `diff`/`blame` changes.
3. **`recall` into `view.rs`** + adapter. `MemoryHit` carries `EffectiveJson`'s serde shape;
   `RecallPage` carries `capped`/`store_corrupt`/`store_empty`.
4. **G4 + G5:** `cmds::status_report`'s counting surface → `list()`; its **eviction path** →
   `view.ledger().records` or `list_records(&TurnQuery{include_all:true})`, never the filtered
   page (G4's table, AC16); `cmds::log` (both forms) → `list_records()`, keeping the
   `.rev().take(limit)` reordering in the adapter (AC3). No empty-case contract change
   (decision 6). README rides this commit if any documented flag behavior changed.
5. **Docs:** parent spec §Deep module 1 amendment (decisions 8, 10) and the `agentrec_log` MCP
   row amendment (decision 12); `P5.md` dependency line corrected `P4` → `P4b`; the stale
   "21 passed" golden figure corrected in P3.md/P4.md/P5.md; `CLAUDE.md` Status per house rule.

## Acceptance criteria

AC ids are **stable identifiers, appended in discovery order and never renumbered** — the skeptic
rounds and claimd declarations reference them by number. That is why the load-bearing block runs
AC1–AC13 plus AC16–AC17 while the supplementary block holds AC14–AC15: the two blocks are
semantic, the numbering is chronological.

**Load-bearing — positive and behavioral.** These are the gates; they fail if behavior moved at
all.

- [ ] **AC1** Every pre-existing golden is byte-identical at the final commit — all 27, including
      `log_json.golden` (which decision 5 exists to protect) and the blame/gap-honesty goldens.
- [ ] **AC2** The 5 goldens captured in commit 1 are byte-identical after commits 2–4. RED proof
      required: a `MemoryHit` omitting any `EffectiveJson` field, or a `RecallPage` without
      `store_empty`, must fail this AC — demonstrate the failure once in-test, then fix.
- [ ] **AC3** The `--json` adapters serialize the view's returned items and nothing else, stated
      as an **expression-level** contract (R8 — revision 2's "byte-equal to the items" wording
      was unsatisfiable three ways: `cmds::log` renders newest-first via `.rev().take(limit)`
      with `--limit` default 50 while `list_records` returns oldest-first and its `TurnQuery.limit`
      takes the *first* n, so the limit cannot be pushed into the query; the empty case emits
      `[]`, which is not the per-line serialization of zero items; and serializing `RecallPage`
      whole would break decision 3):
      - On a **non-empty** fixture, `log --json` stdout ≡ `page.items.iter().rev().take(limit)`
        with one `serde_json::to_string` line each. The `.rev().take(limit)` reordering is the
        adapter's **only** sanctioned transformation; no field construction, no reshaping.
      - The **empty** case is governed solely by AC6; the retained `println!("[]")` branch is the
        one sanctioned hand-built JSON literal in this phase, named here so AC3 and AC6 are not
        read as contradictory.
      - `recall --json` stdout ≡ `serde_json::to_string(&view.recall(..).page.items)` — the
        `items`, never the `RecallPage` (whose `capped`/`store_corrupt`/`store_empty` must not
        reach the JSON, decision 3).
- [ ] **AC4** A field added to `TurnRecord` appears in `log --json` with no adapter edit; a field
      added to `MemoryHit` appears in `recall --json` with no adapter edit (both proven by adding
      a temp field in-test).
- [ ] **AC5** `list()` has at least one production caller, and it is `status`: `rg '\.list\('
      cli/src` → **≥1 match**, and a test asserts `status`'s turn count, rich-rate, and
      git-hidden accounting are computed from `list()`'s page rather than a re-derived filter
      (RED if `status_report` still calls `merged_ids` itself). This AC exists because revision 1
      would have left `list()` dead a second time.
- [ ] **AC6** `log --json` on a zero-turn repo still prints `[]`, exit 0 —
      `integration.rs::log_json_zero_turns_prints_empty_array` passes **unmodified**. D-PD4 is
      not weakened, deleted, or inverted (decision 6).
- [ ] **AC7** `RepositoryView::diff` on an unknown turn ref returns `DiffError::Lookup(Unknown)`;
      on a duplicated id it returns the collapsed turn (not `Ambiguous`) — the P3
      duplicate-collapse golden still passes, and a distinct-turns-sharing-an-id fixture still
      errors ambiguous.
- [ ] **AC8** `RepositoryView::diff` on a fileless turn returns an empty `Page<FileDiff>`, not an
      error (P5 needs `{"files":[]}` over this).
- [ ] **AC9** `RepositoryView::blame` on an uncovered path returns a `BlameResult` carrying the
      recording-gap state with **no** attributor — never a guess. Gap-honesty goldens unchanged.
      `blame_line`'s exact-line-text heuristic is preserved verbatim, proven by the line-level
      goldens.
- [ ] **AC10** `RecallPage::capped` is set when the verify walk stops at `RECALL_VERIFY_CAP` and
      the human adapter still prints the F3 stderr notice; `--json` still omits it (decision 3).
- [ ] **AC11** `RecallPage::store_empty` distinguishes an empty store from a no-fresh-match
      result, and both human empty-state outputs are byte-identical to commit 1's goldens. RED
      without `store_empty`.
- [ ] **AC12** `RepositoryView::recall` performs **zero writes**: `memory.jsonl` length and
      `memory-stats.jsonl` mtime unchanged across a call.
- [ ] **AC13** Cursor semantics carry over: a cursor from `list_records` or `diff` replayed after
      a `purge --log-duplicates` rewrite returns `stale_cursor`, never a silently holed page —
      the same guarantee already proven for `list`.
- [ ] **AC16** *(R7 — the data-loss gate.)* `status`'s eviction protect-set is built from the
      **unfiltered** turn set. RED fixture: an over-budget store containing a superseded turn
      whose snapshot blob is referenced by no surviving turn, plus a git turn with a
      `prompt_ref`. After `status` evicts, both blobs still exist. Deriving `owned_turns` from
      `list()`'s filtered page must fail this AC.
- [ ] **AC17** *(R9 — the `diff`/`blame` wiring gate.)* The `diff` and `blame` **adapters** call
      the view, proven positively and not by `rg` alone: `rg '\.diff\(|\.blame\(' cli/src` → ≥1
      match each (cheap check), plus a behavioral pin per verb whose output is reachable *only*
      through the view — a fixture where `view::diff` returns `DiffError::Lookup(Unknown)` and
      the adapter's prose is produced by mapping that typed error, and likewise for `blame`'s
      gap-honesty state. RED before the rewiring: with the adapters still on their direct
      `load_log`/`BlobStore` path, these pins must fail. This AC exists because revision 2 pinned
      `diff`/`blame` behavior only at the view level, so an executor could have landed the view
      methods, left the adapters untouched, and passed every load-bearing AC — P4's exact failure
      mode, one revision after diagnosing it.

**Supplementary — negative-space, scoped to be literally runnable** (revision 1's versions were
not; see §Skeptic round 1, B1 and B4):

- [ ] **AC14** `rg 'load_log|BlobStore' cli/src/readcmds.rs` no longer matches inside
      `readcmds::diff` or `readcmds::blame`. The file-wide count drops from **17** to the
      `show` + `undo` + shared-helper remainder, and the executor states the new count with the
      surviving matches attributed by function. A file-wide zero is **not** the target
      (decision 11).
- [ ] **AC15** `rg 'memory::' cli/src/memorycmds.rs` no longer matches inside `recall_cmd` —
      including `memory::load_effective`, which revision 1's `memory::recall`-only grep missed.
      Hook-path functions keep their matches. Two caveats stated so this AC is not over-read:
      (a) `recall_cmd` also references `memory::RECALL_VERIFY_CAP` inside the F3 notice, so the
      cap must reach the adapter another way — carried on `RecallPage`, or via a `view`
      re-export named as sanctioned in the task file; (b) a module-top
      `use agentrec_core::memory::…` defeats this grep while preserving the coupling, so AC15 is
      a tripwire, not a proof — AC2/AC10/AC11 are what actually pin the behavior.

Note on AC2's RED proof: without `store_empty`, an adapter that bypasses the seam via
`memory::load_effective` still renders both human empty states correctly and passes AC2's
goldens. AC2's RED claim therefore holds only jointly with AC15's bypass prohibition. Stated
rather than left implicit.

## Verification

Assert **deltas against the measured baseline** (537 / 0 / 2 and 27 goldens), never absolute
counts — every prior task file's absolute figure has gone stale:

```
cargo test -p agentrec-core view::                        # baseline + new diff/blame/recall tests
cargo test -p agentrec --test golden                      # 27 + 5 new = 32 passed, 0 changed bytes
cargo test -p agentrec --test integration                 # includes log_json_zero_turns_prints_empty_array, unmodified
cargo test --workspace -- --test-threads=3                # 537 + delta passed, 0 failed, 2 ignored
cargo clippy --workspace --all-targets -- -D warnings     # clean, debug and release
cargo fmt --check
```

Manual, on the dogfood repo: `agentrec log --json | jq -s .` parses; `agentrec recall --json
<query> | jq .` parses and matches pre-change output byte-for-byte; `agentrec log --json` in a
zero-turn tempdir prints `[]`, exit 0; `agentrec status` output unchanged before/after the
`list()` rewiring.

## Edge cases

- Zero-turn repo (`log`, `log --json` → `[]`, `status`, `diff`, `blame`).
- Log containing only epoch records.
- `diff` on a fileless turn → empty page, not an error (AC8).
- Duplicated turn id (collapse) vs distinct turns sharing an id (ambiguous) — AC7.
- `recall` against an empty store vs a non-empty store with no fresh match — **distinct only on
  the human path**; in `--json` both emit `[]` because the json branch returns before the
  distinction is drawn. Commit 1 captures the human pair, which is where the difference is
  observable (revision 1 mis-stated this).
- Corrupt `memory.jsonl` → `store_corrupt`, fail-open posture preserved on the CLI path.
- Cursor replayed across a `purge --log-duplicates` rewrite → `stale_cursor` (AC13).
- Concurrent daemon append during a read — all reads are ledger-fresh per call (`view.rs`'s
  "holds no open handles" contract); no read path takes `log.lock`.
- `NO_COLOR` / non-TTY — human rendering only; JSON paths unaffected.
- `--explain` glossary (`fmt::glossary_for`) scans this invocation's rendered human output; the
  rewiring must not change what `log` renders, or the glossary AC silently breaks. Covered by
  AC1's `log_explain.golden`.

## Risks

- **Highest risk is a repeat of P4's failure mode** in a new form: an executor satisfies the
  supplementary `rg` ACs by deleting or relocating the old path while the new seam is thin or
  bypassed. AC1–AC5 are the counterweight, and AC5 exists specifically because skeptic round 1
  showed revision 1's AC set passed with `list()` still dead.
- `readcmds.rs` is large and shared: `print_entry`, `load_blob`, and `load_text` serve `diff`,
  `show`, **and** `undo`. Extracting diff/blame must not break `show`/`undo`, which keep their
  direct reads. Ownership of those three helpers is an explicit design question for the task
  file, not something to settle by moving code and seeing what compiles.
- `blame_line`'s exact-line-text heuristic (with its documented v1 duplicate-line limitation) is
  the most intricate logic in the file. It must move verbatim; the blame goldens are the
  instrument (AC9).
- `status` is behaviorally dense (rich-rate window, DEGRADED section, eviction on the human path
  from P4's split) and has **two** turn consumers with different type needs. Rewiring its turn
  source must not disturb the eviction behavior P4 deliberately preserved — `status.golden` and
  `status_json.golden` are the instrument for the rendering, but the over-budget line is not in
  the golden fixture, so **AC16's dedicated RED fixture is the only real gate on the protect-set**.
  Do not rely on an unnamed existing eviction test to catch it; skeptic round 2 explicitly
  declined to certify that one exists.
- Two revisions of this document each broke on the same class of error: asserting which fields a
  consumer needs after reading only part of the consuming function. Before wiring any adapter to
  a view method, read the **whole** consumer, including its error, over-budget, and DEGRADED
  branches.

## Skeptic round 1 (2026-07-30) — FAIL, 5 blocking, all verified real

Recorded so revision 2's changes are traceable and so no executor re-introduces a resolved
defect. Findings verified independently against the tree before amending.

| # | Finding | Resolution |
|---|---|---|
| B1 | `rg 'load_log\|BlobStore' cli/src/readcmds.rs` → **17** matches, spanning `show` and `undo`/`execute_revert` plus shared helpers — revision 1's AC1 ("→ 0 matches") was unsatisfiable without rewiring out-of-scope `undo` or undesigned `show`, leaving file-shuffling as the only way to "pass" | R3: decision 11 + AC14 scope the assertion to `diff`/`blame`; `show`/`undo` named out-of-scope; helper ownership raised as an explicit task-file question |
| B2 | The `log --json` empty case **is** pinned — `integration.rs::log_json_zero_turns_prints_empty_array`, implementing gate-PASSED **D-PD4**. Revision 1's decision 6 would have deleted a gated test on a false premise, the exact loosening the ratchet rule forbids | R1: decision 6 withdrawn; AC6 pins the test as unmodified; asymmetry recorded as a known wart |
| B3 | `TurnSummary` cannot render the human `log` line (`fmt::turn_list_line` needs `prompt_excerpt`/`truncated`/`files_complete`; noise fold needs per-file paths). Revision 1's claim that `list()` serves human `log` was false, and the only golden-preserving fix left `list()` **dead again** — G4 recurring, unpinned by any AC | R2: decision 5 rewritten; `list()`'s real consumer is `status`, whose filter it matches verbatim and whose fields `TurnSummary` already carries; AC5 pins it |
| B4 | Revision 1's AC4 ("`rg 'serde_json::to_string' cli/src` → 0 on the log and recall paths") was unmechanizable — `rg` matches lines, not call paths — and banned the very call the design requires; its AC2 also matched doc comments | R4: replaced by AC3, a byte-equality assertion against the view's serialized return value |
| B5 | `RecallPage` lacked a store-empty discriminator; `recall_cmd` calls `memory::load_effective` directly for its PD3 empty state, which revision 1's `memory::recall`-only grep did not catch — seam half-bypassed with every AC passing | R5: `store_empty` added; AC11 pins both human empty states; AC15 widens the grep to `memory::` |

Non-blocking findings also folded in: the §Why P4 gate-PASSed thesis corrected (R6 — P4's AC4
*did* assert `health()`'s behavior; the real hole was diff/blame/recall having no AC at all, and
revision 1's corrective over-rotated toward the gameable `rg` shape); the `agentrec_log`
summaries-vs-one-serializer contradiction now amended explicitly (decision 12); the `--json`
recall empty-state edge case corrected; revision 1's `git diff --stat` hook-path AC dropped as
theater (`memorycmds.rs` necessarily changes, so `--stat` can never show the intended
invariant) — the surviving real gate is `hook_recall_hard_wall_deadline` passing unmodified,
folded into decision 4's scope statement; line-number pointers replaced with `file:symbol`;
README-in-same-commit moved from commit 5 onto commits 2 and 4; the five-commit deviation from
"one phase = one commit" stated as a sanctioned exception with its reason.

Skeptic round 1 also **confirmed** the central diagnosis (P5 unexecutable as written) and
independently re-verified every factual claim in §Measured starting state except the two errors
corrected above. It flagged that the "founder-confirmed" annotation on revision 1's decisions had
no artifact in the tree; decisions 1–5 and 7–12 are founder-confirmed in session on 2026-07-30,
decision 6 was withdrawn by the founder after B2 was surfaced, and this document is the artifact.

## Skeptic round 2 (2026-07-30) — FAIL, 3 blocking, all in revision 2's new material

Round 2 confirmed R1, R3, R5, and R6 as closing their round-1 findings, and re-verified the
"17" match count, the `status_report`-duplicates-`list()`-verbatim claim, the golden harness's
stderr capture, and the 27 + 5 = 32 arithmetic. It **refuted** one concern raised against
revision 2 (AC5's RED wording survives the `merged_ids` other-caller check: the surviving `undo`
caller does not falsify a function-scoped RED condition). Three new blocking findings:

| # | Finding | Resolution |
|---|---|---|
| B1 | Decision 5 / G4's field claim was false **a second time**: `status_report`'s eviction path (`cmds::status_report` → `retention::enforce_budget`) consumes the **unfiltered** turn set's `prompt_ref` and `files[]` blob hashes — neither on `TurnSummary`. Revision 2 inventoried only the rich-rate window. Not a golden break but a **data-loss path**: a filtered protect-set lets eviction delete blobs still referenced by superseded turns, breaking `undo` of merged history | R7: G4 now tables both consumers with their type needs; decision 5 states `status` makes two calls; commit 4 names the eviction path's source explicitly; **AC16** is a dedicated RED fixture |
| B2 | AC3 was unsatisfiable three ways: `cmds::log` renders newest-first (`.rev().take(limit)`, `--limit` default 50) while `list_records` returns oldest-first and limits from the *first* n, so the limit cannot be pushed into the query; the `[]` empty case is not the per-line serialization of zero items, making AC3 jointly unsatisfiable with AC6; and `recall --json` must serialize `page.items`, not the `RecallPage` | R8: AC3 rewritten as an expression-level contract naming the sanctioned `.rev().take(limit)` transformation, deferring the empty case wholly to AC6, and targeting `page.items` for recall |
| B3 | `diff`/`blame` adapter wiring was pinned by **nothing load-bearing** — AC7/AC8/AC9 assert view-level behavior, satisfiable with the adapters still on their direct `load_log`/`BlobStore` path, and AC14 (supplementary) is satisfiable by relocating the functions. P4's exact failure mode survived for two of the three new methods, in the revision that exists to prevent it. `log`/`recall` had AC3 as a wiring pin; `diff`/`blame` had no analogue | R9: **AC17** adds a positive per-verb wiring pin — the adapter's prose must be produced by mapping the view's typed error, RED before rewiring |

Non-blocking, folded in: AC15 now states both its limits (the `RECALL_VERIFY_CAP` reference needs
a sanctioned source; a module-top `use` defeats the grep, so AC15 is a tripwire and AC2/AC10/AC11
are the real pins); AC2's `store_empty` RED proof is stated as holding only jointly with AC15;
G5's rationale corrected — it rests on the one-serializer invariant, not on an MCP mirror that
decision 12 redirects.

Round 2 again declined to certify the "founder-confirmed" annotation absent an artifact, and
noted that B1 and B2 alter decision 5's factual basis and AC3's substance, so those two need
re-confirmation. Decision 5's *shape* (two typed values, distinct consumers) is unchanged and
stands; what changed is the inventory of which consumer needs which — a correction of fact, not a
reversal of the decision. AC3's rewrite likewise preserves its intent (the adapter serializes
what the view returned) and fixes only its expression.
