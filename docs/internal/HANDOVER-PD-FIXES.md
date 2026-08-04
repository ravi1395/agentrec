# Handover: agentrec PD-review fixes (post-M3, pre-launch)

## Description

You are an **Opus orchestrator** in `~/Projects/agentrec`. A senior-product-designer review (2026-07-10, run against the real binary in a fresh repo) found 2 launch-blocking defects and 4 polish items in the CLI's user-facing surface. Your job: fix them via **Sonnet implementer subagents** (one per task, fresh context, explicit scope), then gate the whole round with an **Opus skeptical-reviewer subagent** against the AC ids below before any "done" claim. Ratchet rules apply: never weaken an AC to pass; escalate ambiguity to the founder.

## Context (read before dispatching)

- **Product:** agentrec — local-first flight recorder for coding agents. Brand invariant: **never fabricate attribution or state**; honest refusals over confident guesses. Every finding below is a place delivered behavior quietly violates that posture.
- **Read first:** repo `CLAUDE.md` (key semantics + status protocol), `SPEC.md` §CLI + §Prompt posture, `IMPLEMENTATION.md` decision register. Conflict order: PROTOCOL.md > IMPLEMENTATION.md > SPEC.md > rest.
- **Code map (CLI surface):** `cli/src/cmds.rs` (`log`, `status`, `format_turn`), `cli/src/readcmds.rs` (`diff`/`blame`/`undo`, `render_turn`), `cli/src/doctorcmd.rs`, `cli/src/initcmd.rs`, `cli/src/fmt.rs` (relative times, glossary), `cli/src/main.rs` (clap wiring, error prefix + exit codes). Core: `agentrec-core/src/` (BlobStore, scrub, formats).
- **Test layout:** unit tests in-module; integration in `cli/tests/` (drives real binary against tempdir fixtures). Baseline: **142 tests, 0 failed**; `cargo clippy` clean. That count only grows.
- **Reproduce findings** (fresh tempdir, no `.agentrec/`): `agentrec doctor` → false "store permissions too open" + "agent active but no signals arriving"; `agentrec status` → "rich-rate: 100% over trailing 0 agent turn(s)"; `agentrec log --json` → empty output; `agentrec show` → command does not exist despite SPEC.md §Prompt posture item 4.

## Decisions log (founder-confirmed via PD review; executors may not re-litigate)

1. **D-PD1:** `doctor` gains a first check `initialized`. If `.agentrec/` absent: that check fails with remedy "not initialized — run \`agentrec init\`", **all other checks report `n/a`** (no fabricated fail states), exit 1. JSON shape keeps the same `{checks:[…],ok}` schema — additive only.
2. **D-PD2:** Build `agentrec show <turn> --prompt` (SPEC.md §Prompt posture item 4 is normative; amending the spec instead was rejected — excerpt discipline *depends* on `show` existing as the explicit full-prompt path). Prompt blobs are post-scrub at rest, so printing is safe. Dangling `prompt_ref` (purged) → "prompt purged — TTL expired" style honest message, exit 1. Turn with no prompt (bare/git) → say so, exit 1. Bare `show <turn>` without `--prompt`: print the turn header (reuse existing turn renderer) — never dump the prompt without the flag.
3. **D-PD3:** `status` with 0 trailing agent turns prints `rich-rate:  n/a (no agent turns yet)` — never a vacuous 100%.
4. **D-PD4:** `log --json` with zero turns prints `[]` (valid JSON, schema-stable in every state).
5. **D-PD5 (polish batch):** rename doctor check `store degraded` → `store health`; `status` turns line drops implementer jargon (`(agent, git/merged excluded)` → `(agent turns; git activity hidden)`); idempotent `init` re-run leads with `already initialized — nothing changed` when no action altered disk; `status` low-rich-rate remedy points at `agentrec doctor` (not `init`).
6. **D-PD6 (P2, non-blocking):** renderer/voice unification (two parallel turn renderers `cmds::format_turn` vs `readcmds::render_turn`; separators `—`/`·`/`->`/`;` drift). **Deferred** — do NOT bundle into this round; log as a tracked debt line in CLAUDE.md Status.

## Infeasible / rejected (do not resurrect)

- Amend SPEC to remove `show` (rejected — see D-PD2).
- `doctor` exiting 0 in uninitialized repos ("nothing to check") — rejected: uninitialized is a fail state the user must act on.
- Changing `doctor --json` schema shape — rejected: additive only, consumers may already parse it.

## Global constraints

- AC ids PD1–PD5 below each map to ≥1 automated test added in the same task (house rule, IMPLEMENTATION.md convention).
- No wire-format / PROTOCOL.md changes anywhere in this round.
- Match surrounding output style per command; do not "improve" unrelated strings (D-PD6 deferred).
- Platforms macOS + Linux; no hardcoded separators.
- **Open question (non-blocking, default given):** repo currently has **no `.git`**. Default: `git init` + baseline commit before task 1 so per-task commits work; skip branch/PR flow. If founder objects, tasks still land uncommitted.

## Executor protocol

- One **Sonnet** subagent per task, fresh context. Prompt each with: exact task block below, the Context section, and the relevant decision(s) — assume zero shared context. Failing-test-first. Per-task commit.
- Merge order: T1 → T2 → T3 → T4 (independent files mostly; T3/T4 both touch `cmds.rs`, so serialize). 
- After T4: dispatch **Opus skeptical-reviewer** subagent (fresh context) with the AC table; verdict per AC id PASS/FAIL/UNTESTED — no PASS without a named test + observed CLI output. Any FAIL/UNTESTED loops back to a new Sonnet task. No "done" claim before skeptic verdict is all-PASS.
- Round ends by updating the **Status** section of `CLAUDE.md` (house rule) with what shipped + the D-PD6 debt line.

---

### T1 — doctor init-gate (AC: PD1)

**Files:** `cli/src/doctorcmd.rs`, test in `cli/tests/` (follow existing doctor failure-mode test pattern).
**Change:** add `initialized` as check #0 keyed on `.agentrec/` existence at the resolved root. Absent → check fails with remedy `not initialized — run \`agentrec init\``; every subsequent check short-circuits to `n/a`; process exits 1. Present → check passes, all existing behavior unchanged.
**Interface contract (consumed by PD1 test):** text output line 1 = `initialized              fail` + remedy line; `--json` gains `{"name":"initialized","status":"fail","remedy":"not initialized — run `agentrec init`"}` as first element; no other check may report `pass` or `fail` in that state.
**Acceptance test (must fail first):**
```rust
// cli/tests/…: doctor in tempdir with NO .agentrec
// assert stdout contains "initialized" + "not initialized — run"
// assert stdout does NOT contain "permissions too open"
// assert stdout does NOT contain "agent active but no signals"
// assert every non-init check line ends in "n/a"; exit code == 1
```
**Verify:** `cargo test --test <that file> doctor` → new test passes; `cargo test` → 0 failed.
**Commit:** `fix(doctor): init-gate — never fabricate check results in uninitialized repos (PD1)`

### T2 — `agentrec show <turn> [--prompt]` (AC: PD2)

**Files:** new `cli/src/showcmd.rs` (or extend `cli/src/readcmds.rs` if <~80 lines — executor's call), `cli/src/main.rs` (clap subcommand), README.md command table row. ≤3 files.
**Change:** per D-PD2. Resolve turn id/prefix with the same resolver `diff` uses (unknown/ambiguous messages must match `diff`'s wording — reuse, don't duplicate). `--prompt` loads full post-scrub prompt via `prompt_ref` from the blob store (integrity-checked read path already exists in core).
**Interface contract:** `show <id>` → turn header via existing renderer, exit 0. `show <id> --prompt` → full prompt text to stdout, exit 0. Dangling ref → stderr `agentrec: prompt purged — …`, exit 1. No prompt (bare/git turn) → stderr honest message, exit 1.
**Acceptance test (must fail first):**
```rust
// integration: record fixture turn with known prompt →
//   `show <id> --prompt` stdout == full prompt (post-scrub), exit 0
// purge prompt object → `show <id> --prompt` exit 1, msg mentions purge/TTL
// bare turn → exit 1, message says no prompt attached
```
**Verify:** `cargo test --test integration show` → new tests pass; `cargo test` → 0 failed; README row present.
**Commit:** `feat(cli): agentrec show <turn> --prompt — close SPEC excerpt-discipline drift (PD2)`

### T3 — status/log empty states (AC: PD3, PD4)

**Files:** `cli/src/cmds.rs`, tests alongside existing cmds tests.
**Change:** D-PD3 + D-PD4 exactly as worded.
**Acceptance test (must fail first):**
```rust
// status on zero-turn store: stdout contains "rich-rate:  n/a", not "100%"
// log --json on zero-turn store: stdout.trim() == "[]", exit 0
```
**Verify:** `cargo test -p agentrec cmds` (or matching filter) → new tests pass; `cargo test` → 0 failed.
**Commit:** `fix(cli): honest empty states — rich-rate n/a, log --json [] (PD3, PD4)`

### T4 — polish batch (AC: PD5)

**Files:** `cli/src/doctorcmd.rs`, `cli/src/cmds.rs`, `cli/src/initcmd.rs` (3 files, small diffs).
**Change:** the four D-PD5 items, nothing else. `store health` rename must update the doctor tests that assert check names.
**Acceptance test:** update/extend existing assertions: doctor output contains `store health` and not `store degraded` as a check name; status contains `git activity hidden`; second `init` run's first line == `already initialized — nothing changed`; low-rich-rate warning mentions `agentrec doctor`.
**Verify:** `cargo test` → 0 failed; `cargo clippy` → clean.
**Commit:** `polish(cli): doctor check name, status jargon, init re-run message, rich-rate remedy (PD5)`

### T5 — skeptic gate (Opus subagent, fresh context)

Prompt it with: AC table PD1–PD5 + decisions log + "verdict per AC: PASS only with a named automated test AND pasted real-binary output from a fresh tempdir repro; anything else FAIL/UNTESTED." Also have it re-run the two original repro sequences from Context and paste before/after. Loop failures back; then update CLAUDE.md Status (append this round + D-PD6 debt line) and report final test count (`cargo test` → expect 142 + new, 0 failed).
