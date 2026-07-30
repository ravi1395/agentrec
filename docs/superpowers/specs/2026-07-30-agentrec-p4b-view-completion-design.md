# P4b — `RepositoryView` completion (diff / blame / recall / wire `list`)

> **Status:** design, revision 6 — amended after skeptic rounds 1 (FAIL, 5 blocking), 2 (FAIL, 3),
> 3 (FAIL, 3), 4 (FAIL, 1 + 4 non-blocking), and 5 (FAIL, 1 + 2 non-blocking). Each round found its
> defects in the *previous revision's own new material*, so each round's fixes were the next
> round's attack surface. Rounds 4 and 5 both assessed the defects as **converging, not
> structural**: the core (interface shape, decisions 1–12, commit sequence, golden-first
> instrument, AC16/AC17 mechanisms) has now survived four consecutive rounds untouched, and each
> round's finding is the same named disease at shrinking scale — round 4 a missing error enum,
> round 5 a single missing string in one enumeration. Every finding independently verified against
> the tree before amending. Written 2026-07-30 against
> `feat/phase-2-0-substrate` @ `638290d`. Parent spec:
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
>
> **Revision 4 changelog** (§Skeptic round 3 records the full findings):
> R10 respecified AC16's fixture around **shared hashes** — revision 3's version was theater,
> passing identically under the buggy implementation because eviction candidates come only from
> the turns passed in, so a blob referenced *only* by an excluded turn is invisible rather than
> deleted; R11 replaced AC17's mechanism with **signature-constrained renderers** (revision 3's
> error-path pin was satisfiable while the happy path still read the ledger directly, and the
> human `log`/`recall` branches were pinned by prose alone) and added AC18, because the
> unknown-turn prose embeds a ledger range a bare typed error cannot carry — making revision 3's
> AC17 jointly unsatisfiable with AC1; R12 added `list_of` so `status` keeps its single ledger
> parse instead of gaining a two-parse consistency window.
>
> **Revision 5 changelog** (§Skeptic round 4 records the full findings):
> R13 added `BlameError` + AC19 — revision 4's "`blame` needs no analogue" was true of the ledger
> range and false of blame's own error data, leaving AC17/AC18/AC1/decision 9 jointly unsatisfiable
> for `blame`; and made `DiffError::Lookup.ledger` an `Option` since `NoTurns` has no range to name.
> R14 enumerated the sanctioned render opts and closed the closure-capture loophole. R15 gave
> `recall`'s two-stream output an explicit contract. R16 recorded the `Page<FileDiff>`
> materialization as an accepted resource-shape change.
>
> **Revision 6 changelog** (§Skeptic round 5 records the full findings):
> R17 re-derived the whole render-opts enumeration **from the code** instead of from memory —
> revision 5's list omitted `recall`'s `query`, without which the golden-pinned no-match line
> `no fresh memories match "<query>"` had no compliant implementation (AC17 + AC2 jointly
> unsatisfiable), and the pass also found `diff`/`blame` need **no** opts at all. R18 stated the
> adapter-transformation rule for the human `log` form, which is why `limit`/`explain` are not
> opts. R19 added the decision-9 error-type signatures to commit 5's parent-spec amendment list.
>
> **Revision 7 changelog** (§Skeptic round 6 records the full findings — a **fresh** reviewer, cold
> read of the whole design):
> R20 replaced `diff`'s bare `Page<FileDiff>` with a `DiffResult` envelope — the golden-pinned turn
> header had no sanctioned carrier under AC17's zero-opt rule, and `Page`'s serde
> (`{"items":…,"next":…}`) could never satisfy P5's required `{"files":[]}`, so the phase built to
> unblock P5 would have left it blocked; one envelope closes both, plus an explicit fileless-turn
> ruling. R21 defined `RecallError`, named in revision 5 but never specified. R22 corrected two
> false instrument citations (AC9's line-level arms and AC19's not-found arm are pinned by
> integration tests, not goldens — AC19's arm by nothing at all yet).
>
> **Revision 8 changelog** (§Skeptic round 7 records the full findings): both blockers were
> **incomplete propagation of R20 itself**, not new design defects.
> R24 propagated the envelope into AC17 — revision 7 left `render_diff` taking `&Page<FileDiff>`,
> so the binding wiring gate still could not reach the header, re-creating round 6's B1 verbatim.
> R25 corrected revision 7's over-claim that the envelope satisfies P5's `{"files":[]}`: it does
> not (three extra keys, and `files` is an object not an array), so commit 5 now **amends P5.md's
> literal** in both places rather than pretending the conflict is resolved. R26 actually added the
> `Page<FileDiff>` → `DiffResult` parent-spec delta to commit 5's list — revision 7's interface
> block claimed it was listed there when it was not.

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
| `log --json` empty case **is** pinned | `cli/tests/integration.rs::log_json_zero_turns_prints_empty_array` asserts `[]`; **D-PD4**, `HANDOVER-PD-FIXES.md` §D-PD4 |

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
  | turn count, rich-rate window | **filtered** (`turns`) | `imported`, `grade`, `tool`, `.len()` | `list_of(&ledger, &TurnQuery{include_all:false})` — all present on `TurnSummary`, no widening |
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
RepositoryView::list_of(l: &Ledger, q: &TurnQuery) -> Result<Page<TurnSummary>, CursorError>
RepositoryView::diff(q: &DiffQuery)         -> Result<DiffResult, DiffError>
RepositoryView::blame(q: &BlameQuery)       -> Result<BlameResult, BlameError>
RepositoryView::recall(q: &RecallQuery)     -> Result<RecallPage, RecallError>
```

**`diff` returns an envelope, not a bare `Page`** (R20 — a correction of the parent spec's own
`Page<FileDiff>` signature, amended in commit 5):

```rust
pub struct DiffResult {
    pub turn_id: String,      // full id; `fmt::short_id` truncation stays adapter-side
    pub tool: Option<String>,
    pub total_files: usize,   // the turn's own count, independent of the page
    pub files: Page<FileDiff>,
}
```

Two independent defects forced this, both found by a cold sixth-round read:

1. **The header had no sanctioned carrier.** Every diff golden opens with
   `turn t_RICH…TUR1 · claude · 5 files`, produced by `readcmds::header_line` from the turn's id,
   tool, and file count. `Page<T>` is `{items, next}` — no turn-level slot — and AC17's opts table
   gives `diff` *zero* opts, so under revision 6 nothing could carry those three facts to the
   renderer. Embedding them on each `FileDiff` fails outright on **AC8**: a fileless turn returns an
   empty page, so `turn <id> · <tool> · 0 files` would be unproducible — and no test pins the
   fileless case (`rg '0 files' cli/tests` → 0), so that regression would have been silent.
2. **P5 would have stayed unexecutable** — the very thing this phase exists to fix. P5's AC1
   requires `diff --json` to emit `serde_json` of the exact typed value, and its empty-case AC
   requires a fileless turn to yield `{"files":[]}`. Serde of `Page<FileDiff>` is
   `{"items":[],"next":null}`; `Page` is generic and shared with
   `Page<TurnSummary>`/`Page<TurnRecord>`, so renaming `items` → `files` is unavailable, and adapter
   reshaping is banned by P5's own AC1.

   **The envelope does not by itself satisfy P5's literal, and revision 7 wrongly claimed it did**
   (R25). Serde of `DiffResult` for a fileless turn is:

   ```json
   {"turn_id":"t_…","tool":"claude","total_files":0,"files":{"items":[],"next":null}}
   ```

   — which differs from `{"files":[]}` in **both** ways: three extra top-level keys, and a `files`
   value that is an object, never a bare array. A custom serializer flattening `files` to an array
   would drop the cursor, which the parent spec's `agentrec_diff` MCP row requires ("limit,
   cursor"), and would still leave the extra keys.

   **Resolution: P5.md's literal is amended in commit 5, not worked around.** P5's `{"files":[]}`
   was written before anyone had designed the type; its *intent* — "empty cases are data, not
   errors, exit 0" — survives intact and is what AC8 preserves. The literal changes to the
   envelope's true serde. `blame --json` and `status --json` are unaffected.

`total_files` is deliberately distinct from `files.items.len()`: the page may be limited while the
header reports the turn's own count — conflating them would break the header under any paginated
`DiffQuery`.

**Fileless-turn ruling (R20):** `diff` on a fileless turn renders the header alone
(`turn <id> · <tool> · 0 files`), exit 0 — today's behavior, stated explicitly because AC8 makes the
empty page routine and nothing currently pins it.

Error types, per decision 9 (each carries the data its adapter's prose needs, and no prose):

```rust
pub struct TurnRangeSummary { pub oldest_id: String, pub newest_id: String, pub count: usize }

pub enum DiffError {
    /// `ledger` is `None` only for `LookupError::NoTurns`, where there is no
    /// oldest/newest id to name (R13).
    Lookup { err: LookupError, ledger: Option<TurnRangeSummary> },
    Cursor(CursorError),
    Io(String),
}

pub enum BlameError {                 // R13 — blame is NOT error-data-free
    FileNotFound,                     // adapter renders "<path>: not found"
    LineOutOfRange { lines: usize },  // adapter renders "<path> has only N line(s)"
    Io(String),
}

pub enum RecallError { Io(String) }   // R21 — named in revision 5, defined here
```

`RecallError` was declared in revision 5's interface block and never defined (R21). It is a bare
`Io(String)`: today every arm of `memory::recall_impl` and `load_effective_checked` returns `Ok`,
with corruption, deadline, and cap surfaced as `RecallOutcome` flags rather than errors, so no
pinned byte depends on its shape. Making `recall` infallible is the equally valid alternative and
the task file may take it; what is not acceptable is leaving the type named-but-undefined in a
document whose last three gate failures were all "an error type nobody designed".

`BlameError` exists because revision 4 asserted "`blame` needs no analogue". That was true of the
*ledger range* and false of blame's own error data (R13): `blame_line` has two user-error arms —
`"<path>: not found"` and `"<path> has only N line(s)"`, the latter byte-pinned by
`blame_line_out_of_range.golden` — and the line count is computed from the worktree bytes that this
phase moves *into* `view::blame`. With `blame -> Result<_, RepoError>` (a single `Io(String)`
variant), the only way `?` reproduces those bytes is prose crafted in core, which violates
decision 9 head-on and misfiles a user-input error as I/O. The path comes from the query, so the
adapter owns both prose strings from a bare discriminant plus `lines`.

`list_of` (R12) takes an already-parsed `Ledger`, mirroring the `health`/`health_of` pair P4
established for exactly this reason. `status` needs it: today it derives its count, rich-rate,
gaps, and eviction set from **one** `view.ledger()` parse, so calling `list()` (which re-reads
`log.jsonl` internally) would open a window where a daemon append makes the printed turn count
disagree with the health figures from the other parse. `status` therefore does one `ledger()` read
feeding `health_of` + `list_of` + the unfiltered eviction set.

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
   - `status` therefore does **one** `ledger()` read feeding `health_of` + `list_of` + the
     unfiltered eviction set — not a single *query*, but a single *parse* (R12; a second parse
     would let a daemon append desync the printed count from the health figures). Revision 1
     wrongly claimed `list()` serves human `log` (R2); revision 2 wrongly claimed `TurnSummary`
     covers all of `status` (R7).

   Rejected: widening `TurnSummary` to cover the human `log` line — the parent spec's rule
   "prompt contents are opt-in data, never included in generic summaries" makes
   `prompt_excerpt`-in-a-summary-type a direct collision.
6. **Withdrawn (R1).** Revision 1 proposed changing `log --json`'s empty case from `[]` to zero
   lines. That reverses **D-PD4** (`HANDOVER-PD-FIXES.md` §D-PD4, gate-PASSED, pinned by
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
4. **G4 + G5:** `cmds::status_report` keeps its **single** `view.ledger()` parse, feeding
   `health_of` + `list_of` (counting surface) + the **unfiltered** records (eviction protect-set,
   never the filtered page — G4's table, AC16); `cmds::log` (both forms) → `list_records()`,
   keeping the `.rev().take(limit)` reordering in the adapter (AC3). Both `log` forms and both
   `recall` forms render through signature-constrained `render_*` functions (AC17). No empty-case
   contract change (decision 6). README rides this commit if any documented flag behavior changed.
5. **Docs:** parent spec §Deep module 1 amendments — decisions 8 and 10; the **error-type
   signatures per decision 9** (`diff -> DiffError`, `blame -> BlameError`, `list -> CursorError`,
   where the spec says `RepoError` for all; R19 — revision 5 left these off the list even though
   they are real spec deltas); **`diff`'s Ok-side signature** (`DiffResult`, where the spec says
   `Page<FileDiff>` — R20's delta, which revision 7 claimed was listed here but was not, R26); and
   the `agentrec_log` MCP row (decision 12).

   **`P5.md` amendments** (all three, in this commit): the dependency line `P4` → `P4b`; the stale
   "21 passed" golden figure (also corrected in P3.md/P4.md); and **the empty-case literal**
   `{"files":[]}` → the envelope's true serde, in both places P5.md states it — its AC block and
   its manual-verification line (R25). `CLAUDE.md` Status per house rule.

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
- [ ] **AC5** The summary projection has at least one production caller, and it is `status`:
      `rg '\.list_of\(|\.list\(' cli/src` → **≥1 match**, and a test asserts `status`'s turn
      count, rich-rate, and git-hidden accounting are computed from that page rather than a
      re-derived filter (RED if `status_report` still calls `merged_ids` itself — a
      function-scoped condition; `undo`'s surviving `merged_ids` caller is out of scope and does
      not falsify it). This AC exists because revision 1 would have left the projection dead a
      second time.
- [ ] **AC6** `log --json` on a zero-turn repo still prints `[]`, exit 0 —
      `integration.rs::log_json_zero_turns_prints_empty_array` passes **unmodified**. D-PD4 is
      not weakened, deleted, or inverted (decision 6).
- [ ] **AC7** `RepositoryView::diff` on an unknown turn ref returns `DiffError::Lookup(Unknown)`;
      on a duplicated id it returns the collapsed turn (not `Ambiguous`) — the P3
      duplicate-collapse golden still passes, and a distinct-turns-sharing-an-id fixture still
      errors ambiguous.
- [ ] **AC8** `RepositoryView::diff` on a fileless turn returns a `DiffResult` whose `files` page is
      empty **while `turn_id`/`tool`/`total_files` still carry the turn's header facts** (R20), not an
      error (P5's empty-case AC rides on this — its `{"files":[]}` literal is amended in commit 5 to
      the envelope's true serde, R25; the AC's intent, "empty cases are data not errors, exit 0",
      is what this AC preserves).
- [ ] **AC9** `RepositoryView::blame` on an uncovered path returns a `BlameResult` carrying the
      recording-gap state with **no** attributor — never a guess. Gap-honesty goldens unchanged.
      `blame_line`'s exact-line-text heuristic is preserved verbatim. **Instrument correction
      (R22):** its most intricate arms — "attribution unavailable — snapshot unavailable", "before
      recording began", and the unresolvable-poisoning logic — are pinned by **integration tests**,
      not by goldens (`golden.rs`'s residual list says so explicitly). Revision 6 named the wrong
      instrument here; a task-file author would have copied it.
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
- [ ] **AC16** *(R7 — the data-loss gate; fixture respecified in R10.)* `status`'s eviction
      protect-set is built from the **unfiltered** turn set.

      **The fixture must share hashes.** `enforce_budget` derives its eviction `candidates`
      *exclusively* from `turn_snapshot_hashes` over the turns it is passed, and the remove loop
      iterates only `candidates` — so a blob referenced **only** by an excluded turn is invisible
      to eviction and survives under both implementations. Revision 3's fixture (a superseded
      turn whose blob no surviving turn references) therefore distinguished nothing (R10). Two
      shared-hash loss classes, both required:
      - **(a) prompt-protection loss.** One blob that is simultaneously an old, past-boundary
        surviving turn's snapshot hash **and** the excluded git turn's `prompt_ref`. Unfiltered:
        `protected_prompts` saves it ("protect any blob referenced as a prompt by ANY turn, not
        just kept ones"). Filtered: the git turn is absent, its `prompt_ref` never enters
        `protected_prompts`, and the blob is evicted.
      - **(b) keep-set loss.** One blob shared between an excluded superseded turn near the
        ledger tail (inside the keep window when included) and an older surviving candidate turn.
        Unfiltered: `keep` holds it via the tail turn, so the candidate walk skips it. Filtered:
        it is absent from `keep` and is evicted.

      Backdate blob mtimes past `pass_start` (the existing `cmds::tests::backdate` pattern) so
      `enforce_budget`'s A3(c) same-pass guard cannot rescue the blobs vacuously and mask the
      difference. Assert **deletion** under the filtered derivation (RED) and **survival** under
      the unfiltered one.

      Do **not** satisfy a failing AC16 by widening `extra_protected_refs`. Its comment — "a
      validly-parsed line's hashes are already reachable through `owned_turns`, so re-adding them
      here would over-protect" — is true only while `owned_turns` is unfiltered; widening it would
      mask the bug and break the deliberate over-protect boundary. The fix is unfiltering
      `owned_turns`.
- [ ] **AC17** *(R9 — the wiring gate; mechanism replaced in R11.)* Bypass must be **structurally
      impossible**, not merely greppable. Each read verb's human and `--json` output is produced
      by a pure `render_*` function **whose signature admits only the view's typed values** — no
      `&Path`/`root`, no `BlobStore`, no `&[LogRecord]`, no store or ledger handle of any kind:

      ```rust
      fn render_diff(result: &DiffResult) -> String    // zero opts — no opts parameter at all
      fn render_blame(result: &BlameResult) -> String  // zero opts
      ```

      `diff` and `blame` take **no opts parameter whatsoever** (R24) — the table below gives both an
      empty set, so an opts struct would be an empty slot inviting exactly the smuggling R14 closed.
      Revision 7 left `render_diff` taking `&Page<FileDiff>` after R20 had moved the header facts
      onto `DiffResult`, which re-created round 6's B1 verbatim: the sanctioned renderer could not
      reach the header its own goldens require. A signature change in the interface block must
      propagate here in the same edit.

      A per-verb expression contract then pins stdout: `diff <turn>` ≡ `render_diff(&view.diff(q)?)`,
      `blame <target>` ≡ `render_blame(&view.blame(q)?)`, and the same shape for the human `log`
      and human `recall` forms. Because the renderer cannot reach a store or a ledger, an adapter
      physically cannot render from a direct read while satisfying the contract.

      **The opts slot is the residual smuggling channel, so it is enumerated, not open** (R14).
      The enumeration below was **re-derived by reading each renderer's actual inputs from the
      code** (R17) — revision 5's version was written from memory and omitted two, one of which was
      a blocking joint-unsatisfiability:

      | Verb | Sanctioned opts | Derived from |
      |---|---|---|
      | `log` (human) | `now_ms`, `utc`, `color`, `all_files`, prebuilt `NoiseMatcher` | `fmt::turn_list_line` + `cmds::log`'s fold branch. **`limit` and `explain` are NOT opts** — see the adapter-transformation rule below |
      | `recall` (human) | `now_ms`, `color`, **`query: &str`** | `format_memory_line` takes `now_ms`/`color`; the no-match line embeds the query (R17) |
      | `diff` | **none** | The turn header's facts ride `DiffResult` (`turn_id`/`tool`/`total_files`, R20 — they are *not* opts); `print_entry` needs only the resolved `FileDiff`; `rg 'paint\|should_color' cli/src/readcmds.rs` → 0 — `diff` has never colorized |
      | `blame` | **none** | `render_turn` → `fmt::turn_detail_header` with `hhmm` derived from the record's own `started`; no color, no clock |

      Anything not in this table requires amending this design. A closure or trait object that
      captures a store, ledger, or root (e.g. `impl Fn(&str) -> Vec<u8>`) **counts as a handle** and
      is banned by the same rule — the constraint is on reachability, not on the literal type name.

      **Adapter-transformation rule (R18).** AC3 sanctions `.rev().take(limit)` for `log --json`;
      the same rule governs the **human** `log` form, which is why `limit` and `explain` are not
      render opts: the adapter slices `page.items.iter().rev().take(limit)` *before* calling the
      renderer, and applies `fmt::glossary_for` to the renderer's output *after*. Both stay outside
      `render_log`, so the enumeration above is complete rather than merely minimal.

      **`recall` renders to two streams**, so its contract is stated separately (R15): the F3 capped
      notice and the "no memories yet" zero-state go to **stderr** while hits and "no fresh memories
      match" go to **stdout**, and a `-> String` renderer cannot produce both. Sanctioned shape:
      `render_recall(&RecallPage, opts) -> (String, String)` (stdout, stderr), or a `-> String`
      renderer for stdout plus adapter-side `eprintln!` driven **solely** by `RecallPage`'s
      `capped`/`store_empty`/`store_corrupt` flags — which are view-typed data, so either shape
      keeps the constraint intact. This is not a conflict with AC2/AC10/AC11; it is how they are
      jointly satisfied.

      The **stdout** side needs the query text: the no-match line is `no fresh memories match
      "<query>"` and commit 1 golden-pins it *including the query*, so under either shape the stdout
      renderer must receive it. `query: &str` is therefore a sanctioned recall opt (R17). The
      alternative — echoing the query on `RecallPage` — is also acceptable and additionally serves
      MCP 2.2's request-echo, but is not required.

      This replaces revision 3's mechanism, which was gameable three ways (R11): an adapter could
      call `view.diff()` only to map its error, discard the page on `Ok`, and render the happy
      path from a **relocated** direct-read helper — passing AC17's error pin, AC14 (no tokens
      left *inside* `readcmds::diff`), and AC1 (goldens unchanged by construction). Revision 3
      also left the human `log` and human `recall` branches pinned by commit-4 prose alone, and
      by this document's own thesis prose carries no gate weight.

      Supplementary mechanizable check: `rg 'load_log\(' cli/src/cmds.rs` → **0 matches outside
      `mod tests`** (today: production at `cmds::log` plus a test helper — the AC scopes out the
      helper).
- [ ] **AC18** *(R11.)* `diff`'s lookup-failure prose is byte-identical to
      `diff_unknown_id.golden` **while the adapter computes nothing from the ledger.** Today
      `readcmds::turn_by_ref` renders `LookupError::Unknown` as `unknown turn id '<ref>' —
      recorded turns: <oldest>..<newest> (N turns)` via `turn_range`, which needs the oldest id,
      newest id, and count over the unfiltered turn list. A bare `DiffError::Lookup(Unknown)`
      cannot reproduce those bytes, and AC14 forbids the adapter re-loading the log — so revision
      3's AC17 was jointly unsatisfiable with AC1 (R11). The typed error therefore **carries the
      range**:

      ```rust
      pub struct TurnRangeSummary { pub oldest_id: String, pub newest_id: String, pub count: usize }
      DiffError::Lookup { err: LookupError, ledger: TurnRangeSummary }
      ```

      Both the `Unknown` and `Ambiguous` arms are covered; `NoTurns` carries `None` (R13 — a bare
      `TurnRangeSummary` is unconstructible on an empty ledger, so the field is `Option`).

      The count is over the **unfiltered** turn vec — today's `readcmds::diff` feeds `turn_range` its
      unfiltered `turns`, which is why the golden reads `(8 turns)` over 7 distinct ids (the DUP id
      appears twice). `fmt::short_id` truncation stays in the CLI adapter; the view ships full ids.
- [ ] **AC19** *(R13.)* `blame`'s two user-error arms are byte-identical to today while the adapter
      owns their prose: `blame <file>:<n>` past end of file → stderr `agentrec: <path> has only N
      line(s)`, exit 1, matching `blame_line_out_of_range.golden`; `blame <missing>:<n>` → stderr
      `agentrec: <path>: not found`, exit 1. Both are rendered by the adapter from
      `BlameError::{LineOutOfRange{lines}, FileNotFound}` plus the query's path — **no prose
      crosses into `agentrec-core`** (decision 9). RED if `view::blame` returns a message string.

      Note: the not-found arm is **not** currently pinned by anything — no golden, and
      `rg ': not found' cli/tests/integration.rs` → 0 — so this AC's instrument for that arm is a
      **new** integration assertion. Revision 6 claimed it sat in `golden.rs`'s documented residual
      list; it does not (R22 — the substantive claim was right, the citation was false).

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
  branches. Rounds 3 and 4 then hit the same class *on error paths specifically* — an error arm is
  a consumer too, and is the easiest one to forget.
- **Accepted resource-shape change (R16):** `Page<FileDiff>` pre-resolves every entry's content, so
  a pure `render_diff` holds a whole turn's blobs at once, where `print_entry` streams one pair at
  a time and drops it. Worst case is bounded by the store's existing 10 MiB per-blob cap times the
  turn's entry count, plus the assembled output `String`. Correctness and goldens are unaffected;
  peak memory is. Accepted deliberately as the price of the signature constraint — if it ever
  bites, the fix is the CLI adapter paging `DiffQuery.limit` internally, which the type already
  supports.

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

## Skeptic round 3 (2026-07-30) — FAIL, 3 blocking, all in revision 3's new material

Round 3 confirmed R8 as writable-as-specified, confirmed R7's two-consumer table complete after
reading all of `status_report`, confirmed R7's data-loss premise real, and verified every new
factual claim (`enforce_budget` protecting `prompt_ref` from any turn and deriving hashes via
`turn_snapshot_hashes`; `TurnSummary` carrying neither `prompt_ref` nor `files[]`; `cmds::log`'s
newest-first render with `--limit` default 50; `list`/`list_records` returning oldest-first and
limiting from the first n). It additionally confirmed `recall`'s human renderer needs nothing
beyond `MemoryHit` + `RecallPage`'s flags, and that `blame` needs no ledger-range analogue. Three
new blocking findings — every one in AC16/AC17, the two ACs revision 3 added:

| # | Finding | Resolution |
|---|---|---|
| B1 | **AC16 was theater.** `enforce_budget` derives eviction `candidates` *exclusively* from `turn_snapshot_hashes` over the turns passed in, and the remove loop iterates only `candidates` — so a blob referenced **only** by an excluded turn is *invisible* to eviction, not deleted. Revision 3's fixture (superseded turn whose blob no surviving turn references) therefore **passes under the buggy filtered-page implementation too**; the RED condition never fires. The data-loss premise is real, but only via **shared-hash** shapes, which the fixture lacked | R10: fixture respecified around two shared-hash loss classes — prompt-protection loss (blob is both a surviving turn's snapshot hash and the excluded git turn's `prompt_ref`) and keep-set loss (blob shared between an excluded tail-window superseded turn and an older surviving candidate) — with mtime backdating so A3(c) cannot rescue vacuously, asserting deletion under the filtered derivation and survival under the unfiltered |
| B2 | **AC17's prose-mapping mechanism could not reproduce the pinned bytes.** `readcmds::turn_by_ref` renders `LookupError::Unknown` as `unknown turn id '<ref>' — recorded turns: <oldest>..<newest> (N turns)` via `turn_range`, needing the oldest id, newest id, and count over the unfiltered turn list — bytes pinned by `diff_unknown_id.golden`. A bare `DiffError::Lookup(Unknown)` carries none of it, and AC14 forbids the adapter re-loading the log, making revision 3's AC17 jointly unsatisfiable with AC1. Same disease as rounds 1–2, this time on the *error* consumer | R11: **AC18** added — the typed error carries `TurnRangeSummary{oldest_id, newest_id, count}`, covering both the `Unknown` and `Ambiguous` arms; `NoTurns` needs no range, and `blame` needs no analogue since it takes a path, not a turn ref |
| B3 | **Wiring pins covered only a slice of each verb.** An adapter could call `view.diff()` solely to map its error, discard the page on `Ok`, and render the happy path from a **relocated** direct-read helper — passing AC17's error pin, AC14 (no tokens inside `readcmds::diff`), and AC1 (goldens unchanged by construction). The human `log` and human `recall` branches were pinned by commit-4 prose alone, which by this document's own thesis carries no gate weight | R11: AC17's mechanism replaced with **signature-constrained renderers** — `render_*` functions whose signatures admit only the view's typed values (no `&Path`, no `BlobStore`, no `&[LogRecord]`), plus per-verb expression contracts for every human and `--json` form, making happy-path bypass structurally impossible rather than merely detectable; supplementary `rg 'load_log\(' cli/src/cmds.rs` → 0 outside `mod tests` |

Non-blocking, folded in: **R12** adds `list_of(&Ledger, &TurnQuery)` alongside P4's existing
`health_of`, so `status` keeps its **single** ledger parse — revision 3's "two view calls" would
have opened a window where a daemon append desyncs the printed turn count from the health figures
computed off the other parse. AC16 now also carries the warning that `extra_protected_refs`' "a
validly-parsed line's hashes are already reachable through `owned_turns`" comment is true only
while `owned_turns` is unfiltered, so a failing AC16 must be fixed by unfiltering `owned_turns`,
never by widening the protect-set harvest (which would mask the bug and break the deliberate
over-protect boundary).

**Pattern worth carrying into the task file.** Three revisions, three rounds, and every round
found its defects in the immediately preceding revision's *new* material — never in the parts
that had already survived a round. Two distinct recurring failure modes:

1. **Partial-consumer reading.** Asserting which fields a consumer needs after reading only part
   of the consuming function. Round 1 hit it on `log`'s renderer, round 2 on `status`'s eviction
   path, round 3 on `diff`'s *error* renderer. The mitigation now in §Risks — read the whole
   consumer including its error, over-budget, and degraded branches — was written after round 2
   and still did not prevent round 3's B2, because the error path was not read as a "consumer".
2. **Detection instead of prevention.** Every AC that tried to *detect* bypass (`rg` shapes,
   error-path pins) was gamed within one round. The AC that finally holds — AC17's
   signature-constrained renderers — makes the bypass unrepresentable instead. Prefer
   type-level impossibility over greppable prohibition for anything load-bearing.

## Skeptic round 4 (2026-07-30) — FAIL, 1 blocking + 4 non-blocking; defects converging

Round 4 verified R10's fixture genuinely discriminates (walking `enforce_budget`'s boundary
arithmetic for both loss classes in both directions, and confirming `protect_newest` does not
interfere with either shape), confirmed AC18 reproduces `diff_unknown_id.golden` byte-for-byte
including `fmt::short_id` truncation and the `(8 turns)` count over the unfiltered vec, confirmed
`FileDiff` **can** carry every arm `print_entry` renders without a render-time store lookup (the
mandate's suspected hole — it is not one), confirmed R12's `list_of` preserves both status goldens,
and confirmed AC17's supplementary grep is correctly scoped. One blocking finding:

| # | Finding | Resolution |
|---|---|---|
| B1 | **Revision 4's "`blame` needs no analogue" was half-true and load-bearing.** True of the ledger range; false of blame's own error data. `blame_line` has two user-error arms — `"<path>: not found"` and `"<path> has only N line(s)"`, the latter byte-pinned by `blame_line_out_of_range.golden` — and the line count comes from the worktree bytes this phase moves *into* `view::blame`. With `blame -> Result<_, RepoError>` (one `Io(String)` variant), the only way `?` yields those bytes is prose crafted in core, violating decision 9 and misfiling a user-input error as I/O. AC17 + AC18 + AC1 + decision 9 were jointly unsatisfiable for `blame`. Same disease as round 3's B2 — the error path not read as a consumer — one seam over, in the very sentence revision 4 added to close it | R13: `BlameError{FileNotFound, LineOutOfRange{lines}, Io}` added to the interface contract; **AC19** pins both arms byte-identical with the prose adapter-side; AC18's sentence corrected to "no *ledger-range* analogue"; the not-found arm flagged as not currently golden-pinned so its instrument is named honestly |

Non-blocking, all folded in: **R13** also made `DiffError::Lookup.ledger` an `Option<TurnRangeSummary>`
(a bare struct is unconstructible on an empty ledger, which the `NoTurns` arm reaches). **R14**
enumerated the sanctioned render opts per verb and stated that a closure or trait object capturing a
store/ledger/root counts as a handle — the constraint is on reachability, not on type names, closing
the `/* render opts */` smuggling channel. **R15** gave `recall` a two-stream contract, since its F3
capped notice and zero-state go to stderr while hits go to stdout and a `-> String` renderer cannot
produce both. **R16** recorded the `Page<FileDiff>` materialization as an accepted resource-shape
change with its 10 MiB-per-blob bound named and the paging escape hatch identified.

**Convergence, as assessed by the reviewer and recorded here because it governs whether to keep
revising or restructure:** round 1 found five blockers, several resting on false factual premises;
round 2 found three including a data-loss path; round 3 found three, both in the two newest ACs;
round 4 found one — a single error-arm typing gap in one sentence — plus four wording/enumeration
patches. The core has been stable since revision 3. R13 applies the design's own already-proven
pattern (a typed error carrying the data its adapter's prose needs) to the one verb that lacked it,
so it creates no new attack surface of the kind rounds 2–4 exploited. A rewrite is not warranted.

**Two lessons for the task file, both earned the hard way:**

1. **Error arms are consumers.** Three consecutive rounds found the same class of defect — asserting
   what a consumer needs after reading only part of it — and rounds 3 and 4 both found it on an
   *error* path specifically, after §Risks had already been amended to warn about partial-consumer
   reading. When extracting a seam, inventory every `Err` arm's data needs before designing the
   error type, not after.
2. **Prefer impossibility to prohibition.** Every AC that tried to *detect* bypass — `rg` shapes,
   relocation checks, error-path pins — was gamed within one round. The mechanisms that finally hold
   are the ones that make the wrong thing unrepresentable: signature-constrained renderers, typed
   errors carrying their own render data, and a fixture whose RED depends on shared state rather than
   on absence.

## Skeptic round 5 (2026-07-30) — FAIL, 1 blocking + 2 non-blocking; still converging

Round 5 confirmed R13 closes round-4's B1 with an exhaustive sweep of `blame`/`blame_file`/
`blame_line` for un-inventoried error arms (`blame_file` returns no `Err` at all — every outcome is
a `BlameResult` state; `blame()` itself has none; `blame_line`'s only two are the arms AC19 names,
and `line_no == 0` shares the out-of-range arm). It confirmed `BlameQuery.path` carries exactly what
both prose strings print (`parse_target` splits `file:line` only when the suffix parses as a
positive integer), that AC19 is consistent with AC1/AC14/AC17, that `Option<TurnRangeSummary>` fixes
the `NoTurns` unconstructibility, that `all_files` is correctly a render opt rather than a query
field (it folds file entries in the human count only — putting it in `TurnQuery` would wrongly
change returned data), and that R16's bound is accurate.

| # | Finding | Resolution |
|---|---|---|
| B1 | **R14's `recall` opts enumeration omitted the query string.** The human no-match line is `no fresh memories match "<query>"` on **stdout**, and commit 1 golden-pins it *including the query text*. R14 said recall's opts are "exactly `now_ms`, `color`" and that anything else requires amending the design — and nothing sanctioned carried the query (not on `RecallPage`, not in the opts, `&RecallQuery` off-list). **No compliant implementation existed**: AC17 + AC2 jointly unsatisfiable for `recall`, the same shape as round 3's B2 and round 4's B1, from the same disease — an enumeration written without reading the renderer's full inputs | R17: `query: &str` added to recall's sanctioned opts (echoing it on `RecallPage` also accepted, and additionally serves MCP 2.2's request-echo). The **whole** enumeration was then re-derived from the code as a table with a "derived from" column per verb |

Non-blocking, folded in: **R18** — `log`'s enumeration omitted `limit` and `explain`, satisfiable
only under an ownership reading the doc never stated, so the adapter-transformation rule is now
explicit (slice `.rev().take(limit)` before the renderer, apply `fmt::glossary_for` after), which is
also why neither is an opt. **R19** — commit 5's parent-spec amendment list omitted the decision-9
error-type signatures (`diff -> DiffError`, `blame -> BlameError`, `list -> CursorError` against a
spec that says `RepoError` for all), which are real spec deltas.

The completeness pass R17 demanded produced a **stronger** result than the finding required:
reading the renderers showed `diff` and `blame` need **no** opts whatsoever — `rg 'paint|should_color'
cli/src/readcmds.rs` → 0 (diff has never colorized), and `render_turn` derives its timestamp from
the record's own `started` via `hhmm`. Revision 5 had guessed "`color` and whatever the task file
enumerates"; the truth is an empty set, which is a tighter constraint than the guess.

**Process finding, recorded for the gate trail.** Round 5's worktree was still at revision 4's
commit (`f9a1f1d`) — the coordinator's move command used `checkout --detach HEAD`, a no-op when HEAD
already pointed at the old commit. The reviewer read revision 5 from the object store instead, and
the two commits are code-identical, so the verdict stands; but a gate verdict must never attach to
a commit the tree does not show. The worktree was corrected to `b7a5c26` before round 6.

**Convergence, round 5's assessment:** still converging, no new front. Round 4 one blocker (a
missing error enum), round 5 one blocker (one missing string in one list) plus wording patches —
both the same disease at shrinking scale, and both *created by* the strictness the design adopted
deliberately: R14's "exactly this list, anything else amends the design" converts any enumeration
omission into a formal blocker. That is the ratchet working, not a structural fault.

## What the task file must carry that this design does not

Previewed by round 5 so the next artifact's scope is already fixed. A fresh-context executor reading
only this spec would still be missing:

- **Field-level definitions** for `FileDiff`, `BlameResult`, and `MemoryHit` — the goldens are the
  behavioral pin; the field sets are executor-defined within them.
- **Ownership of `print_entry` / `load_blob` / `load_text`**, shared today by `diff`, `show`, and
  `undo` — a named open question in §Risks, not a settled decision.
- **The `RECALL_VERIFY_CAP` transport choice** — `RecallPage` field versus a named `view` re-export
  (AC15(a) sanctions either).
- **AC16's concrete arithmetic** — blob sizes, budget value, and which turns sit inside the keep
  window, so the RED/GREEN discrimination is reproducible rather than described.
- **Test placement and the claimd declare-first mapping** of AC1–AC19 to specific test names and
  files, including which existing suites gain assertions versus which files are new.
- **The `blame` not-found integration assertion's exact location**, since that arm has no golden.
- **`diff`'s `Cursor.query` fingerprint scheme** (R23) — `TurnQuery::fingerprint` covers only
  `include_all`, so a diff-scoped fingerprint must incorporate the turn ref and the `paths` filter.
  **AC13 is unimplementable for `diff` without this decision**, which makes it the highest-priority
  item on this list.
- **The five new goldens' fixture-identity constants**, per `golden.rs`'s "documented stable
  mapping" discipline — every id/timestamp/hash reaching a golden must originate from a named
  `const`, never from a generator.
- **Whether `recall` is infallible or keeps `RecallError::Io`** (R21 sanctions either).

## Skeptic round 6 (2026-07-30) — FAIL, 2 blocking (shared root cause) + 4 non-blocking

**A fresh reviewer, deliberately unanchored.** The reviewer who gated rounds 1–5 hit an API session
limit before it could read revision 6. Rather than resume it, a new Fable skeptic was given a cold
read of the *whole* design — five rounds with one reviewer can accumulate shared blind spots, and
revision 6 had never been read by anyone without prior anchoring. That choice paid for itself
immediately: the fresh reader found a defect on `diff`'s **happy path** that five rounds of
delta-focused review had walked past.

| # | Finding | Resolution |
|---|---|---|
| B1 | **`diff`'s golden-pinned header had no sanctioned carrier.** Every diff golden opens with `turn t_RICH…TUR1 · claude · 5 files` (`readcmds::header_line`, needing the turn's id, tool, and file count). `Page<T>` is `{items, next}` — no turn-level slot — and AC17's opts table gives `diff` **zero** opts, so nothing could carry those three facts to `render_diff`. Embedding them per-`FileDiff` fails on **AC8**: a fileless turn yields an empty page, making `turn <id> · <tool> · 0 files` unproducible — and `rg '0 files' cli/tests` → 0, so the regression would have been **silent**, contradicting the AC preamble's claim that the load-bearing gates fail if behavior moved at all | R20: `DiffResult{turn_id, tool, total_files, files: Page<FileDiff>}`; `total_files` deliberately distinct from `files.items.len()` so pagination cannot corrupt the header; explicit fileless-turn ruling added |
| B2 | **P5 would have stayed unexecutable for `diff --json`** — the exact condition this phase exists to remove. P5's AC1 demands `serde_json` of the exact typed value and its empty-case AC demands `{"files":[]}`; serde of `Page<FileDiff>` is `{"items":[],"next":null}`, `Page` is generic and shared so `items`→`files` renaming is unavailable, and adapter reshaping is banned by P5's own AC1 — jointly unsatisfiable | R20 + R25: the envelope is necessary but **not sufficient** — revision 7 wrongly claimed it closed this. `DiffResult`'s serde still is not `{"files":[]}`, so commit 5 amends P5.md's literal in both places it appears, and also carries the `Page<FileDiff>` → `DiffResult` parent-spec delta that revision 7 claimed to have listed but had not (R26) |

Non-blocking, all folded in: **R21** defined `RecallError`, which revision 5 named in the interface
block and never specified — pointed out as especially poor form in a document whose previous three
gate failures were all "an error type nobody designed" (the task file may instead make `recall`
infallible; today every arm of `memory::recall_impl` returns `Ok`, with corruption/deadline/cap as
`RecallOutcome` flags). **R22** corrected two false instrument citations: AC19 claimed the blame
not-found arm sits in `golden.rs`'s residual list — it does not, and `rg ': not found'
cli/tests/integration.rs` → 0, so it is pinned by *nothing* today; and AC9 attributed
`blame_line`'s intricate arms to "the line-level goldens" when they are pinned by integration tests.
Both substantive claims were right and both citations were wrong — the kind of error a task-file
author copies forward. **R23** added `diff`'s `Cursor.query` fingerprint scheme to the task-file
scope list and flagged it as the highest-priority item there: `TurnQuery::fingerprint` covers only
`include_all`, so **AC13 is unimplementable for `diff`** until that decision is made.

**What round 6 confirmed by independent re-derivation** (not inherited from earlier rounds):
AC16's two shared-hash loss classes genuinely discriminate under `enforce_budget`'s candidate walk;
AC18/AC19's typed errors carry exactly what the pinned prose needs with no prose in core; AC3's
expression contract is writable; the 537/0/2 baseline and 27/24 golden counts; the central P5
diagnosis (which it judged *understated*, per B2); and that decisions 8/10/12 are genuine ratchet-ups
against real parent-spec deltas.

**Reviewer's judgment on whether further rounds have value:** the remaining defect class is one a
design document *can* eliminate — B1 is the same enumeration-completeness defect blocked in rounds
4, 5, and 6 (blame's error data, recall's query, now diff's turn header), and the design's own
mechanism closes it mechanically once applied to `header_line`'s inputs. It is not an
implementation-time residual. Everything else the reviewer attacked survived.

## Skeptic round 7 (2026-07-30) — FAIL, 2 blocking, both incomplete propagation of R20

Round 7 verified R20's envelope itself against every attack: `DiffResult` carries exactly what
`header_line` needs and nothing more (no prompt fields, so no collision with the parent spec's
prompt-in-summaries rule); every byte of all three diff goldens is reachable from it, re-walked
arm-by-arm through `print_entry` (withheld, skipped, Missing/Corrupt via `unresolvable_msg`, binary
byte counts, baseline-unknown, `after_synthesized`, per-op unified diffs); the fileless ruling
matches today's `readcmds::diff` exactly; `total_files` distinct from `files.items.len()` is sound
under pagination; the duplicate-collapse header is reproducible; and AC7/AC13/AC18 stay coherent
across the signature change. `RecallError`, both R22 citation corrections, the N4 fix, and R23's
additions all checked out. **The envelope survived; its propagation did not.**

| # | Finding | Resolution |
|---|---|---|
| B1' | **R20 was not propagated into AC17.** The binding wiring gate still specified `fn render_diff(page: &Page<FileDiff>, opts: &DiffRenderOpts) -> String`, while the expression contract `diff <turn>` ≡ `render_diff(&view.diff(q)?)` now passes a `&DiffResult` — type-incoherent, and the header facts R20 added were formally unreachable by the very function AC17 names. Round 6's B1, restated in the fix for round 6's B1 | R24: signature is `fn render_diff(result: &DiffResult) -> String`, with **no opts parameter at all** for `diff` or `blame` (their sanctioned sets are empty, so an opts struct would be a smuggling slot with no legitimate contents); the opts table's `diff` row now states the header facts ride the envelope and are not opts |
| B2' | **Round 6's B2 was moved, not closed, and revision 7 claimed otherwise** — worse than silence, because a task-file author copies the claim. Serde of `DiffResult` for a fileless turn is `{"turn_id":…,"tool":…,"total_files":0,"files":{"items":[],"next":null}}`, differing from P5's `{"files":[]}` in both the extra keys and the object-vs-array shape. Flattening `files` to a bare array would drop the cursor the parent spec's `agentrec_diff` row requires. Commit 5 also still did **not** list the `Page<FileDiff>` → `DiffResult` parent-spec delta, despite the interface block asserting it was listed | R25: the over-claim is replaced with the actual serde bytes and an explicit resolution — **P5.md's literal is amended in commit 5**, in both the AC block and the manual-verification line. P5's `{"files":[]}` predates the type's design; its intent ("empty cases are data, not errors, exit 0") survives and is what AC8 preserves. R26: the parent-spec delta is now genuinely in commit 5's list |

**Process note.** The coordinator told the reviewer that commit 5 already covered `diff`'s
signature change. It did not — the claim existed only as a parenthetical in the interface block.
The reviewer checked the tree rather than accepting the summary, and recorded "the tree governs".
That is the correct posture for every claim in this document, including this sentence.

**Reviewer's judgment on further rounds — recorded because it ends the loop.** Both blockers are
clerical propagation gaps in a fix that was itself correct, not new structural defects. After these
edits, *nothing remains that a design document can catch*: the residual risk — does the implemented
`FileDiff` reproduce every `print_entry` arm, does AC16's fixture arithmetic actually discriminate,
does the seam stay unbypassed — is precisely what AC1–AC19's specified tests exist to catch at
implementation time. Round 8 is therefore scoped as a **narrow verification of these edits only**
(AC17's signature, the opts-table row, the AC8 parenthetical, and commit 5's two new entries), not
another adversarial pass. A further full round would have **negative** value: the document's defects
are now being generated faster by revision churn than by design content.
