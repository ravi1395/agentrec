# attest v1 — implementation plan (DRAFT)

Status: draft, not chunked, not dispatched. Spec:
`docs/superpowers/specs/2026-08-28-attest-design.md` — read it first; its "Lessons
from claimd" section is the rejected-approaches register and is binding. Its
"Review findings" section (added during this plan's review) reports measured
gaps against founder-confirmed decisions 2, 4, and the daemon-purity
invariant — those three are NOT yet ratified; this plan proceeds on the
candidate replacements stated there, flagged wherever they matter below.

**Editorial note (round 5): this plan was previously ~490 lines and had
drifted into re-deriving the same facts in 3–4 places per topic, which is
exactly what caused rounds 2–4 to each find a stale copy the prior round's
fix missed. This version states each fact ONCE, in the phase that owns it,
and cross-references by pointer everywhere else. Implementation mechanism
detail that belongs to a task executor, not a plan (per CLAUDE.md's plan
rule: "NO implementation bodies"), has been deliberately removed in favor of
a named constraint the task must satisfy — the four rounds of review did
real spike-level investigation; that investigation's CONCLUSIONS are kept as
constraints below, its algorithmic detail is not.**

## Goal + architecture (5 lines)

Derive claims from tests (cargo adapter first), capture evidence passively joined to
turn records, CONFIRM only via independent replay in an extracted pinned tree, stale
via measured coverage maps consulted by the note-taking daemon, review manual items
as evidence-first cards. New: `agentrec-core/src/attest/` (pure events+fold),
`cli/src/attest/*` (adapter, capture, replay, coverage, review, gate, report).

## Decisions log (founder-confirmed; executors may not re-litigate)

Numbered 1–9 in the spec's "Founder decisions" section. The ones executors hit
daily: (2) no claim DSL — adapters generate recipes, mechanism under review,
see spec's "Review findings"; (4) verdicts are claim-false / recipe-invalid /
flaky, only claim-false is permanent — `flaky`'s producer is not yet designed,
see Phase 4; (5) coverage scope, over-stale when unsure; (6) daemon never
executes tests — enforcement mechanism under review, see spec's "Review
findings"; (9) PROTOCOL.md untouched — formats go in `ATTEST-FORMAT.md`
marked unstable.

## Infeasible / rejected (with the code reason)

- Static (graphify) scope for v1 — rejected in spec; misses dynamic deps.
- Daemon-side replay execution — daemon.rs has no exec path for repo commands and
  must not gain one; the D46/launchd scar shows what background execution debt costs.
- `git worktree add` for replay isolation — writes into the production repo's
  `.git`; the phase-3.0 gate precedent is `git archive` extraction. Use that.
- Reusing claimd's TS code via subprocess — two runtimes, one product; rewrite the
  (small) event+fold core in Rust.
- Per-claim hand-authored stdout matching (a `--match` regex per claim) — the
  claimd failure mode reborn. The actual per-test result channel on this
  repo's stable toolchain is Phase 1 item 2b's territory, not restated here.

## Global constraints

- All agentrec invariants (CLAUDE.md "Key semantics") hold; `attest.jsonl` is
  append-only with no rewrite class.
- **`attest.jsonl` has multiple writers, including the daemon itself as a
  ROUTINE writer** (CLI commands appending `evidence`/`verdict`/`human`, the
  daemon appending `stale`) — a genuinely NEW class for this repo. `log.jsonl`'s
  lock (`cli/src/loglock.rs`) is not a drop-in precedent: its own module doc
  says the daemon's steady `persist` append does NOT take it, because for
  `log.jsonl` the daemon is the sole steady writer. For `attest.jsonl` the
  daemon must take the lock. Adapt the API shape (blocking/non-blocking
  append, `loglock.rs`/`memlock.rs`), not the daemon-exemption. Phase 3's
  first-writer task wires it; Phase 4 adds a concurrent-append AC.
- Every task lands its ACs in `IMPLEMENTATION.md` §attest before code (house
  rule) — no such section exists yet (`grep -n '^#.*[Aa]ttest' IMPLEMENTATION.md`
  finds no heading); Phase 1's task creates it, every later phase appends.
- Suite baseline at branch fork must be recorded in the first commit message;
  clippy debug+release + fmt clean per task; no test seams in release `strings`.
- Executor protocol: fresh-context Sonnet implementers per task, Opus review,
  Fable skeptic gate per phase (the ONE adversarial pass — no stacked
  self-verification loops).
- Merge order = phase order; each phase independently mergeable behind the
  `attest` subcommand being absent from help until phase 5 (flag-gated visibility,
  single const).
- This repo uses zero directory modules today (`find agentrec-core/src
  cli/src -name mod.rs` → empty) — new submodule trees (`agentrec-core/src/
  attest/`, `cli/src/attest/`) each need a sibling module-root file
  (`attest.rs`, not `attest/mod.rs`) registered in the parent's `pub mod`
  list, 2018-edition style. Both phases that introduce one are listed below.

## Phase 1 — SPIKE: adapter output channel + per-test coverage economics (gate everything on it)

Two unmeasured assumptions, not one — item 2b was found during plan review,
not in the original scope, and is no less load-bearing than the coverage
question.

- Files: `docs/verify/attest-coverage-spike.md` (findings only; throwaway
  scripts in scratchpad, labeled throwaway); `IMPLEMENTATION.md` (creates
  the empty §attest section).
- Prerequisite: `cargo-llvm-cov` + the `llvm-tools-preview` rustup component
  — neither is installed on the dev machine as of this plan. Install first
  (`cargo install cargo-llvm-cov`, `rustup component add llvm-tools-preview`).
  If the sandbox blocks installation, that's itself a phase 1 finding.
- Probe A (coverage economics): on agentrec itself (~1150 tests), measure
  (a) wall time of one full instrumented suite run, suite-level map; (b)
  per-test profile capture via `LLVM_PROFILE_FILE` templating over an
  isolated-process run of a 50-test sample, extrapolate; (c) map sizes; (d)
  file-set quality — do fixture reads and spawned `agentrec` binary
  invocations appear in the child's coverage map? (real spawn call sites,
  `file:symbol`: `cli/tests/bisect.rs:agentrec`, `cli/tests/approve.rs:run`,
  `cli/tests/golden.rs:agentrec`, `cli/tests/hardening_daemon.rs:
  spawn_record`. This is the plan's own named "likely killer" — it MUST be
  in the exit criteria as a plain yes/no with evidence, not just probed.)
- Probe B (output channel, item 2b): determine and PIN the per-test result
  channel this repo's stable toolchain actually supports. Measured
  constraints the spike must satisfy, established across four rounds of
  plan review — treat these as the spike's entry conditions, not its
  conclusions to re-derive:
  - `cargo test --message-format=json` carries no per-test data on stable
    (`rustc 1.97.1`, no toolchain pin) — only compiler messages. Do not
    revisit this; it's measured twice independently.
  - `<binary> --list` (no `--format=json`) gives a stable, trivially
    parseable `<name>: test` enumeration — discovery is solved by this
    alone, no ruling needed.
  - A single test's summary line (`test result: ok. N passed; M failed; K
    ignored; … filtered out`) is a real, stable, parseable format — but
    an UNSCOPED `--exact <name>` runs it against every target in the
    workspace at once (measured: ~23 summary lines for this repo), and a
    name can collide across targets (measured, real, not hypothetical:
    `opaque_call_counts_as_importable_with_no_file_entries` exists as a
    `#[test]` fn in BOTH `cli/tests/import_codex.rs:204` and `cli/tests/
    import_claude.rs:201` — the same collision that motivates Phase 2's
    target-scoped `ClaimId`). Any result-interpretation mechanism the spike
    lands on MUST be scoped to one target (`--test <name>` / `--lib` /
    `--bin agentrec`, derived from that test's `cargo metadata` target
    kind), never a bare unscoped `--exact`.
  - Exit code alone conflates real states: a missing test and an
    `#[ignore]`d test both exit `0`; a genuine failure and a build that
    doesn't compile both exit `101`. A mechanism that treats exit code as
    the verdict will CONFIRM a deleted test or PERMANENTLY `claim-false` a
    broken build (spec decision 4 makes `claim-false` permanent). The
    summary line's counts (not the exit code) are the thing to parse.
  - This is the SAME channel decision for Phase 4's single-test `attest
    verify` and Phase 3's bulk `attest run -- cargo test` (many tests, one
    invocation) — the spike should rule on both from one investigation, not
    two. Phase 3's case additionally needs to decide between: (i) the same
    target-scoped summary-line parser, extended to per-test `test <name>
    ... ok|FAILED` lines within one run — one adapter-owned parser, not a
    per-claim recipe (lesson 1), fails closed (unparseable batch → evidence
    kept with a `parse_failed` flag, raw blob retained, never silently
    dropped); (ii) `cargo-nextest` as a second external tool dependency; (iii)
    N individual scoped-`--exact` invocations, reusing the same parser as
    Phase 4, at the cost of one process-spawn per test — measure against
    Probe A's suite-scale numbers before ruling it out.
- Exit criteria (explicit, downstream contract):
  1. A table of wall-time/bytes for Probe A (a) and (b), both MEASURED not
     estimated.
  2. Probe A (d)'s finding as a plain yes/no with evidence.
  3. A WRITTEN ruling on Probe A: per-test vs suite-level coverage fallback
     (spec decision 5 sanctions the fallback).
  4. A WRITTEN ruling on Probe B: the exact result-interpretation mechanism
     for both the single-test (verify) and bulk (capture) cases, honoring
     the measured constraints above. **This ruling, once written, is the
     candidate replacement for spec decision 2's mechanism — flag it back
     to the founder for ratification per the spec's "Review findings"
     section; don't let it stand as a self-ratified decision.**
  5. The chosen coverage map's schema sketch for Phase 4's consumer.
  6. `IMPLEMENTATION.md` §attest section created (`grep -n '^#.*[Aa]ttest'
     IMPLEMENTATION.md` non-empty).
  7. **Rulings 3 and 4 require founder sign-off before Phase 2 dispatch
     begins.** No pre-set numeric bar for ruling 3 (no basis before the
     numbers exist) — the founder reads the measured table, not a threshold
     the spike author picks alone.
- Failure exit (Probe A): if child-process coverage is unattainable for
  integration tests, the ruling documents scope = "unit-test claims only,
  integration claims stale on any src write" — still shippable, honest.
- Verification: the findings doc has items 1, 2, 5 measured/produced as
  specified; items 3/4 are the two rulings + recorded founder sign-off;
  item 6 is the grep above; commit `spike: attest coverage economics +
  adapter output channel on the dogfood corpus`.

## Phase 2 — claim core: events, fold, derive (pure, no execution)

- Files: `agentrec-core/src/attest.rs` (module-root, global constraints),
  `agentrec-core/src/attest/events.rs`, `agentrec-core/src/attest/fold.rs`,
  `agentrec-core/src/attest/types.rs`, `agentrec-core/src/lib.rs` (register
  `pub mod attest;`, pattern at `lib.rs:15-33`), `ATTEST-FORMAT.md`
  (draft-unstable header). 6 files, over the ≤2–3/task guideline —
  `/pchunker` should split `attest.rs`+`types.rs`+`lib.rs` (plumbing) from
  `events.rs`+`fold.rs`+test (the logic).
- **Type ownership, pinned (Phase 3's adapter and Phase 2's fold share
  these):** `TestIdentity` and `StructuredResult` are defined in
  `agentrec-core/src/attest/types.rs`, not in `cli` — core cannot depend on
  `cli` (dependency direction is `cli` → `agentrec-core`, never reversed).
  Phase 3's adapter constructs these core types; it does not own their
  shape.
- Interface contract (Phase 3+ consume):
  - `AttestEvent` enum: `Derive { claim_id: ClaimId, test_identity:
    TestIdentity, body_hash: Option<[u8; 32]>, renamed_from:
    Option<TestIdentity> }`, `Evidence`, `Verdict`, `Stale`, `Human`,
    `ManualDeclare` — serde JSONL round-trip.
  - `fold_claims(&[AttestEvent]) -> BTreeMap<ClaimId, ClaimState>`;
    `ClaimId = hash(adapter_id, test_identity)`, and `test_identity` MUST
    include the cargo target/binary component, not just the bare fn name
    (measured collision, Phase 1 Probe B — `ClaimId` collapsing two
    different tests into one claim silently cross-attributes their
    evidence/verdicts). The target name comes from the adapter's own
    per-target `--list` invocation loop (Phase 3) — no JSON field involved,
    there isn't one on stable.
  - `ClaimState` carries the spec's state machine plus STALE overlay, plus
    `last_body_hash: Option<[u8; 32]>` and `renamed_from:
    Option<TestIdentity>` (mirroring the `Derive` event fields above — fold
    just copies them onto state, no reconciliation logic lives in core).
- **Decided (founder, this session): renaming a `#[test]` fn carries claim
  continuity — same `ClaimId`, history preserved — not a fresh DERIVED
  claim.** `fold_claims` is a DUMB APPLIER of this: if a `Derive` event
  carries `renamed_from: Some(old_identity)`, look up the claim currently
  keyed by `old_identity`'s `ClaimId`, re-key it to the new identity's
  `ClaimId`, carry its history forward. Fold does not detect renames itself
  — it has no way to represent "which identities this derive invocation did
  NOT see" from a flat append-only event stream, and shouldn't try. That
  detection — comparing the new `discover()` set against previously-known
  identities, computing `body_hash` for anything that vanished-and-appeared
  in the same run, matching hashes — happens in `attest derive` (Phase 3,
  `cli`), which has both sets in memory at command time and writes the
  already-resolved `renamed_from` into the event it emits. Phase 3 owns:
  locating a test's source (cargo metadata's `src_path` for integration
  targets; a `mod`-tree walk from the crate root for unit tests inside
  `#[cfg(test)] mod tests` — the real shape of `daemon.rs`'s own tests,
  which `src_path` alone does NOT resolve since it points at the crate
  root, not the test's actual file) and hashing the located test's body
  with `syn` (see Phase 3 for the dependency and its disclosed
  incompleteness). No hand-rolled scanner — this repo's own recorded lesson
  (`awk-cannot-lex-rust`) is disclose the boundary or use a real lexer; use
  the real one.
- AC-shaped test snippet: fold of [derive, evidence, stale,
  verdict(confirmed)] ends CONFIRMED with stale overlay dropped;
  verdict(recipe-invalid) then verdict(confirmed) ends CONFIRMED (retryable
  proven); author-run evidence alone can never reach CONFIRMED
  (state-machine test); a `Derive` event with `renamed_from: Some(old)`
  re-keys an existing claim's full history under the new identity without
  losing prior evidence/verdicts (state-machine test, not exercised
  end-to-end here — Phase 3's fixture AC does that).
- Verification: `cargo test -p agentrec-core attest::` green; commit
  `attest: claim events + deterministic fold (core, pure)`.

## Phase 3 — cargo adapter + passive capture

- Files: `cli/src/attest.rs` (module-root), `cli/src/attest/
  adapter_cargo.rs`, `cli/src/attest/capture.rs`, `cli/src/attest/
  statuscmd.rs` (minimal read-only status), `cli/tests/attest_capture.rs`,
  `cli/tests/fixtures/attest_sample_crate/` (new fixture — MUST include
  both a `tests/` integration target and a `#[cfg(test)] mod tests` inside
  its own `src/`, exercising both of Phase 2's location-resolution cases,
  not just the easier integration-target one; note this fixture sits
  inside the workspace, so it needs its own `[workspace]` table or a
  `workspace.exclude` entry in the root `Cargo.toml` to build standalone —
  confirm which at task time), wiring in `cli/src/main.rs` (hidden
  subcommands `attest derive`, `attest run`, `attest status`), `cli/
  Cargo.toml` (new direct dependency: `syn` + `proc-macro2`, needed for
  rename location-resolution — currently reaches this workspace only
  transitively as a proc-macro build dependency of `clap_derive`/
  `serde_derive`; this is a genuinely new runtime dependency with real
  `features` needs (parsing + token-stream hashing), not free, and is NOT
  currently a field `check-versions.sh` tracks). 8 files, well over budget
  — `/pchunker` should split the fixture crate + `attest.rs` + `Cargo.toml`
  from `adapter_cargo.rs` + `capture.rs` + its test, and `statuscmd.rs`
  from both.
- **`attest status` is built here, minimally.** Referenced by this phase's
  own AC, the E2E tail, and spec's `recipe-invalid`-surfaces-in-status
  clause — no other phase owns it. Scope: read `attest.jsonl`, fold, render
  claim counts by state + the dirty-bit/dev-loop-only flag. Richer
  rendering (`recipe-invalid` surfacing, needing Phase 4's verdict events)
  is a Phase 4/5 extension of this same file, not a new one.
- Contract (produces): adapter trait `discover() -> Vec<TestIdentity>`,
  `run(filter) -> Vec<StructuredResult>` — both types owned by
  `agentrec-core::attest::types` (Phase 2); channel per Phase 1's ruling
  (item 4), scoped per-target as that ruling requires. Capture writes
  `evidence` events with `turn_id` = open turn, blob = output into CAS.
- **Capture has two paths, both in scope (spec's capture-component bullet
  defines both):** (1) the explicit `agentrec attest run -- <cmd>` wrapper;
  (2) recognition of test invocations arriving through the existing hook
  path (a `PostToolUse` signal whose command matches a test-runner pattern)
  — not a new ceremony step (lesson 3): it rides the hook signal that
  already fires for any tool call.
- **Turn-id resolution, pinned (the likeliest Phase 3/4 seam-drift point):**
  `attest run` is a short-lived CLI process — it cannot call
  `TurnEngine::open_turn_id()` (`agentrec-core/src/engine.rs:148`)
  directly, that's live in-process daemon state, and this repo's
  architecture invariant is "consumers never need the daemon running — all
  reads are file-based." Read the daemon's persisted mirror, `.agentrec/
  open.json` (`cli/src/daemon.rs:2071`'s comment names it explicitly, same
  file the crash-recovery journal reads). Missing/stale `open.json` → no
  open turn → the existing dirty-bit/dev-loop-only path, not a new failure
  mode.
- ACs: `attest derive` on the fixture crate creates one claim per
  discovered test (both the integration-target test and the `mod
  tests`-shaped unit test), idempotent re-run appends nothing; renaming
  either fixture test with its body unchanged → re-derive reuses the old
  `ClaimId` (Phase 2's dumb-applier fold + this phase's detection),
  changing the body too → new claim; `attest run -- cargo test` inside a
  hook-bracketed session records evidence joined to that turn (fixture:
  emit the open-bracket signal first, the O5-proven pattern); a `cargo
  test` invocation arriving through the hook path (no explicit wrapper) is
  captured the same way; dirty-tree evidence is recorded with the dirty
  bit set, renders as dev-loop-only in `attest status`; `attest status` on
  a fixture log renders claim counts by state.
- Verification: `cargo test --test attest_capture`; commit
  `attest: cargo adapter + passive evidence capture`.

## Phase 4 — verify (independent replay) + coverage staleness

- Files: `cli/src/attest/replaycmd.rs`, `cli/src/attest/coverage.rs`,
  daemon delta (stale-marking only) in `cli/src/daemon.rs` — not
  `agentrec-core/src/daemon.rs`, which does not exist; the daemon lives in
  the `cli` crate (`cli/src/main.rs:9: mod daemon;`) — `cli/src/main.rs`
  (register `attest verify`), `cli/tests/attest_verify.rs`, a new
  standalone script (`scripts/check-attest-exec-boundary.sh` or similar,
  see purity mechanism below) + its allowlist file, wired into the CI lint
  job beside `scripts/check-write-opens.sh`. 7 files, well over budget —
  `/pchunker` should split the daemon delta (security-sensitive, its own
  task + heaviest review) from replaycmd+coverage+main.rs, and the new
  script from both.
- Contract: `attest verify [--all-stale|<id>]` extracts pinned HEAD via
  `git archive`, scrubbed env, adapter-runs the single test via Phase 1's
  ruling (item 4) — target-scoped, summary-line-based, NOT bare exit code
  — appends verdict per spec taxonomy; coverage maps per Phase 1's ruling
  (item 3); daemon consults maps on write events it already receives and
  appends `stale`.
- **`flaky` verdict producer — currently designed nowhere, must land
  here.** Spec decision 4 declares `flaky` and makes `claim-false`
  permanent; this repo has a recorded live flake
  (`approve.rs::a_killed_approve_never_leaves_a_phantom_approval`,
  CLAUDE.md) that a single-run verify would mint as a PERMANENT
  `claim-false`. Constraint: `attest verify` must not decide `claim-false`
  from one run alone — some retry-and-compare policy (precedent:
  `bisectcmd`'s `--flaky-retries`) resolves disagreement to
  `flaky-observation` instead. This task pins the exact policy (retry
  count, what counts as "disagree"); not designed further here.
- **Purity check for "the daemon never executes repo-authored commands"
  — mechanism corrected twice during review, use the CURRENT one, don't
  resurrect either predecessor:**
  1. A same-file `grep`/`awk` scan (round 1's draft) is blind to a spawn
     reachable through a sibling module — rejected.
  2. A `clippy.toml disallowed-methods` entry for `Command::new` (round 2's
     draft) doesn't deliver even a per-site census in this repo: 19 files
     (test fixtures, plus `daemon.rs`'s own `#[cfg(test)] mod tests` at
     `daemon.rs:2866`) already carry a blanket `#![allow(clippy::
     disallowed_methods)]` for an unrelated lint (the fsguard FIFO-read
     guard) — that attribute silently exempts everything in scope
     regardless of which config entry fires, so most of the codebase's
     `Command::new` sites (measured: 123 in `cli/tests`, plus the 4 in
     `daemon.rs`'s own test module) get zero review the moment this entry
     lands — rejected.
  3. **Current mechanism: a standalone script, this repo's own precedent
     for exactly this shape** (`scripts/check-write-opens.sh` — "scans
     production source for write-open call sites and fails on any not in
     `scripts/write-open-allowlist.txt`"). A new script scans non-test
     `cli/src` + `agentrec-core/src` for `Command::new` call sites against
     its OWN allowlist file (immune to clippy's attribute-based
     suppression, since it isn't clippy). Wire into the CI lint job beside
     `check-write-opens.sh`. **Scope this claim honestly, the way the
     write-open script's own header does: catches an unannotated DIRECT
     `Command::new` in non-test production code only. It does NOT prove no
     call is reachable from the daemon's functions through another
     module's allowlisted spawn (e.g. this same phase's `coverage.rs`,
     which legitimately spawns `cargo-llvm-cov`) — that needs real
     call-graph analysis, out of v1 scope, not claimed by this mechanism.
     It does NOT cover test code (123 measured `Command::new` sites in
     `cli/tests`, all legitimate, out of scope by design — same as the
     write-open script's own test exclusion).** Budget the annotation pass
     for the real production count — re-enumerate exactly at task time
     (measured this round: `daemon.rs` 4 sites all inside its own
     `#[cfg(test)]` block so zero production; the non-test production
     count across `cli/src`+`agentrec-core/src` needs a fresh count scoped
     to non-test code specifically, not the raw 19).
- ACs: mutation probe — break the tested fn, verify → claim-false; delete
  the test, verify → recipe-invalid (via the `--list` membership check,
  not exit code — a deleted test and a build failure must NOT both resolve
  the same way; the AC needs a companion assertion that the build itself
  was green, to actually discriminate the two); restore + verify →
  confirmed (retry proven end-to-end); flaky-test fixture (a test that
  passes/fails nondeterministically) → `flaky-observation`, never
  `claim-false`, per the retry policy above; write to a covered file →
  daemon appends stale for exactly the mapped claims (and over-stale for
  unmapped-but-suspect per Phase 1's ruling); build with a stale binary
  hazard guarded (`cargo build` before probe — the recorded probe-hygiene
  scar); daemon appending `stale` concurrently with a CLI `attest verify`
  appending `verdict` produces no torn line in `attest.jsonl` (the
  adapted-lock append test, global constraints); the new script reds if an
  unannotated `Command::new` is added to non-test `cli/src`/`agentrec-core/
  src` (plant-probe: add one, confirm the script fails, remove it).
- Verification: `cargo test --test attest_verify` + the mutation probes in
  the task file run live; **also run at least one real repo test binary
  (`cli/tests/golden.rs` or `cli/tests/integration.rs`) inside a `git
  archive`-extracted tree once, by hand** — `git archive` strips `.git`,
  and while a spot-check found most git-dependent fixtures `git_init` their
  OWN tempdir (`golden.rs:git_init`, independent of the checkout root),
  that was a spot-check, not exhaustive, and `claim-false` is a PERMANENT
  verdict; commit `attest: independent replay + coverage staleness`.

## Phase 5 — review cards, advisory gate, report (the UX phase)

- Files: `cli/src/attest/reviewcmd.rs`, `cli/src/attest/gatecmd.rs`,
  `cli/src/attest/reportcmd.rs`, `cli/tests/attest_gate.rs`, unhide the
  subcommand in `cli/src/main.rs`. 5 files, over budget — `/pchunker`
  should split review+unhide from gate+report.
- Contract: `attest review` iterates pending manual/blocking items, renders
  criterion + evidence (diff via existing view, output blobs, related-turn
  list as pre-fill context), keys y/n/s append `human` events; `attest
  gate` folds and exits nonzero only on claim-false or unattested blocking
  manual items (recipe-invalid and flaky-observation surface, never
  block); `attest report --range` emits md+json bundle with provenance
  chains.
- ACs: golden for the report on a fixture log; gate exit-code table test
  (all four verdict kinds × blocking/fyi — `flaky-observation` only
  produces once Phase 4's retry policy lands, don't stub it here); review
  flow test via scripted stdin — n-with-note appends the note verbatim;
  fyi items never appear in review or gate output.
- Verification: `cargo test --test attest_gate` + goldens; commit
  `attest: review cards, advisory gate, attestation report`.

## Risk ordering rationale

Phase 1 gates the design's two unmeasured assumptions (coverage economics
and the adapter output channel — both found load-bearing during review, not
just the one originally scoped). Phases 2–3 are low-risk pure/plumbing work
that dogfoods immediately. Phase 4 carries the security-sensitive surface
(replay isolation, daemon purity, the flaky-retry policy) and gets the
heaviest gate. Phase 5 is UX polish on proven substrate.

## Manual E2E tail (numbered, run before any release claim)

1. On agentrec itself: `attest derive` → claims ≈ test count.
2. Real session: implement a small change with Claude Code, watch evidence attach
   to the session's turns via `attest status`.
3. Break a function, `attest verify --all-stale` → exactly the mapped claims go
   claim-false; fix; re-verify → confirmed.
4. `attest report --range main..HEAD` → bundle names the prompt that produced the
   change (provenance chain end-to-end).
5. One manual-declare blocking item; `attest gate` red; `attest review` y; gate
   green.

## Deferred (returns as its own plan)

Authoritative CI gate + split-token action; PR-checklist sync; node/jest/pytest
adapters; opt-in worker; signing; `ATTEST-FORMAT.md` graduation into PROTOCOL.
