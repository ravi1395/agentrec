# Plan — P4b: `RepositoryView` completion (diff / blame / recall / wire the summary projection)

**Spec:** `docs/superpowers/specs/2026-07-30-agentrec-p4b-view-completion-design.md` @ `e411a45`
(revision 8, **skeptic GATE PASS** after 8 rounds / 17 blocking defects / 2 reviewers).
**Parent spec:** `docs/superpowers/specs/2026-07-18-agentrec-phase-2-design.md` §Deep module 1.
**Parent plan:** `docs/superpowers/plans/2026-07-25-agentrec-phase-2-0.md`.
Conflict order: PROTOCOL.md > IMPLEMENTATION.md > parent spec > design spec > this plan > task file.

## Goal + architecture (≤5 lines)

P4 delivered `open`/`list`/`health` and left `list` with zero callers; `diff`, `blame`, and `recall`
existed only in prose. P4b adds those three to `agentrec-core::view`, gives the summary projection a
real consumer (`status`), and routes `log --json` through the view's serializer. Adapters become
render-only over typed values. **P5 is unexecutable until this lands** — its ACs demand typed values
that do not exist.

## Baseline (measured, re-verify before starting)

`cargo test --workspace -- --test-threads=3` → **537 passed / 0 failed / 2 ignored**;
`cargo test -p agentrec --test golden` → **27 passed** over 24 golden files.
Assert **deltas** against your own measured baseline, never these absolutes — every prior task
file's absolute figure went stale.

## Decisions log (founder-confirmed; executors may NOT re-litigate)

The design spec carries 12 numbered decisions and 27 numbered revisions (R1–R27). Read
§Decisions and §Skeptic rounds 1–8 before starting. The ones most often re-litigated by fresh
executors:

1. **`log --json`'s empty case stays `[]`** (D-PD4, gate-PASSED). Do not change it; do not weaken,
   delete, or invert `integration.rs::log_json_zero_turns_prints_empty_array`. The array/JSONL
   asymmetry is a recorded wart, not a bug to fix here.
2. **`recall --json` output stays byte-identical.** `RecallPage`'s `capped`/`store_corrupt`/
   `store_empty` never reach the JSON.
3. **`show` and `undo` keep their direct `load_log`/`BlobStore` reads.** Not scope. `UndoCoordinator`
   is wave-2 by name, with the parent plan's own warning that a fresh executor will otherwise build
   the whole 2.3 ledger early.
4. **The memory hook path is untouched** (`recall_for_hook*`, `cmds::inject_memory`, INV-M4).
5. **`status` makes ONE ledger parse**, feeding `health_of` + `list_of` + the unfiltered eviction
   set. Two parses would let a daemon append desync the printed count from the health figures.
6. **The eviction protect-set is built from the UNFILTERED turn set.** This is a data-loss path, not
   a style question — see P4b-4 and the design's AC16.

## Infeasible / rejected (killed against real code — do not resurrect)

| Approach | Why it dies |
|---|---|
| Route human `log` through `list()`/`TurnSummary` | `fmt::turn_list_line` needs `prompt_excerpt`/`truncated`/`files_complete`; the noise fold needs per-file paths. `TurnSummary` has none. Round 2 B3. |
| Widen `TurnSummary` to cover human `log` | Parent spec: "prompt contents are opt-in data, never included in generic summaries" — `prompt_excerpt` in a summary type is a direct collision. |
| `diff -> Page<FileDiff>` (the parent spec's own signature) | `Page<T>` is `{items,next}` with no turn-level slot, so the golden-pinned header `turn <id> · <tool> · N files` has no carrier; and a fileless turn's empty page makes it unproducible. Round 6 B1. |
| Give `render_diff`/`render_blame` an opts struct | Their sanctioned opt sets are **empty**; an opts struct is a smuggling slot with no legitimate contents. Round 8. |
| Detect adapter bypass with `rg` assertions | Gamed within one round, three times (relocation, error-path-only wiring, `use`-import). Only signature-level impossibility held. |
| Satisfy P5's `{"files":[]}` by flattening `files` to a bare array | Drops the cursor the parent spec's `agentrec_diff` MCP row requires, and still leaves the envelope's other keys. P5.md's literal is amended instead. Round 7 B2'. |
| Fix a failing AC16 by widening `extra_protected_refs` | Masks the bug and breaks its deliberate over-protect boundary. The fix is unfiltering `owned_turns`. |

## Global constraints + invariants (once — not per task)

- Append-only `log.jsonl`/`signal.jsonl`; `perms.rs` at write sites; no network.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` clean, **debug and
  release**.
- Integration tests run `--test-threads=3` (FSEvents contention above that).
- Test seams `#[cfg(debug_assertions)]`.
- **No prose in `agentrec-core`.** Every typed error carries the data its adapter's prose needs and
  nothing more (decision 9). A `String` message crossing into core is an automatic FAIL.
- **Renderers take only view types.** No `&Path`, no `BlobStore`, no `&[LogRecord]`, no ledger
  handle — and a closure or trait object capturing any of those counts as a handle.
- Pointers are `file:symbol`, never line numbers.
- README rides the same commit as any public-facing change.

## Executor protocol

Per task file, in order: `claimd` declare-first per AC id → failing test first → implement → verify
with the exact commands in the task file → **binding `skeptical-reviewer` gate in an ISOLATED
WORKTREE** (see the worktree lesson: reviewers can `git reset` a shared tree; commit and push
before dispatching) → per-task commit. Never weaken an AC to pass; escalate to the founder.

**Branch:** `feat/phase-2-0-view-completion` off `feat/phase-2-0-substrate`.

## Task order + dependency rationale

| # | Task | Depends on | Why this order |
|---|---|---|---|
| P4b-1 | Goldens only, zero production files | — | The pre-refactor instrument. Must contain **no** production delta or it pins post-change behavior and proves nothing. Mirrors P3→P4. |
| P4b-2 | `diff` + `blame` into `view.rs` | P4b-1 | The two verbs whose goldens P4b-1 did not need to add (they exist) but whose byte-equivalence P4b-1's baseline run confirms. Largest task; `blame_line`'s heuristic is the delicate part. |
| P4b-3 | `recall` into `view.rs` | P4b-1 | Independent of P4b-2 — **may run in parallel** in a separate worktree. Consumes P4b-1's five new goldens. |
| P4b-4 | Wire `status` + `log` (G4/G5) | P4b-2, P4b-3 | Touches `cmds.rs`, which both prior tasks may have moved code out of. Carries the data-loss gate (AC16). |
| P4b-5 | Docs + amendments | P4b-4 | Amends the parent spec, `P5.md`, `P3.md`/`P4.md`, `CLAUDE.md`. Must be last so the amended literals match what actually shipped. |

## Open questions (blocking / non-blocking)

1. **(non-blocking)** `recall` infallible vs `RecallError::Io` — R21 sanctions either. P4b-3 picks
   one and states which; no downstream task depends on the choice.
2. **(non-blocking)** Whether `list()` survives as a delegating wrapper over `list_of` or is replaced
   by it. Round 6 noted `list()`'s 17 unit tests exercise it either way. P4b-4 decides.
3. **(BLOCKING for P4b-4; P4b-1..3 unaffected)** `fix/perf-evidence-round` is not in the
   substrate and collides semantically with P4b-4 — it splits `enforce_budget` into
   `plan_eviction`/`execute` and moves eviction out of `status` onto the daemon tick, which makes
   P4b-4's "`status` still evicts" AC false by design there and points AC16's fixture at the wrong
   function. AC16's substance survives (the unfiltered `owned_turns` slice is still what
   `plan_eviction` receives). Full analysis and three candidate orderings:
   `docs/superpowers/plans/2026-07-30-p4b-branch-merge-state.md` §3. **Answer before P4b-4 starts.**
4. **(blocking, answered in P4b-2)** `print_entry`/`load_blob`/`load_text` are shared with `show`
   and `undo`. P4b-2 must not break either. The resolution is stated in that task file: the shared
   helpers **stay** in `readcmds.rs` for `show`/`undo`, and `view::diff` gets its own resolution
   logic rather than importing the CLI's. Duplication here is deliberate and bounded — the
   alternative is dragging out-of-scope `undo` into the seam.

## Manual E2E (after P4b-4, before P4b-5)

Run against the live dogfood repo, observing output — not just exit codes:

1. `agentrec log | head -5` → unchanged from before the branch (compare against a saved capture).
2. `agentrec log --json | jq -s 'length'` → parses, count matches `agentrec log --json | wc -l`.
3. `agentrec log --json` in an empty tempdir → prints exactly `[]`, exit 0.
4. `agentrec diff <a-rich-turn>` → header + per-file diffs, byte-identical to a pre-branch capture.
5. `agentrec diff <a-fileless-turn>` → header alone, exit 0.
6. `agentrec blame <file>` and `agentrec blame <file>:<n>` → unchanged; `blame <file>:99999` →
   `agentrec: <file> has only N line(s)`, exit 1.
7. `agentrec recall <query> --json | jq .` → parses, byte-identical to a pre-branch capture.
8. `agentrec recall <no-match-query>` → `no fresh memories match "<query>"` on stdout.
9. `agentrec status` → turn count, rich-rate, and git-hidden accounting unchanged.
