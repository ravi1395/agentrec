# attest v1 — implementation plan (DRAFT)

Status: draft, not chunked, not dispatched. Spec:
`docs/superpowers/specs/2026-08-28-attest-design.md` — read it first; its "Lessons
from claimd" section is the rejected-approaches register and is binding.

## Goal + architecture (5 lines)

Derive claims from tests (cargo adapter first), capture evidence passively joined to
turn records, CONFIRM only via independent replay in an extracted pinned tree, stale
via measured coverage maps consulted by the note-taking daemon, review manual items
as evidence-first cards. New: `agentrec-core/src/attest/` (pure events+fold),
`cli/src/attest/*` (adapter, capture, replay, coverage, review, gate, report).

## Decisions log (founder-confirmed; executors may not re-litigate)

Numbered 1–9 in the spec's "Founder decisions" section. The ones executors hit
daily: (2) no claim DSL — adapters generate recipes; (4) verdicts are
claim-false / recipe-invalid / flaky, only claim-false is permanent; (5) coverage
scope, over-stale when unsure; (6) daemon never executes tests; (9) PROTOCOL.md
untouched — formats go in `ATTEST-FORMAT.md` marked unstable.

## Infeasible / rejected (with the code reason)

- Static (graphify) scope for v1 — rejected in spec; misses dynamic deps.
- Daemon-side replay execution — daemon.rs has no exec path for repo commands and
  must not gain one; the D46/launchd scar shows what background execution debt costs.
- `git worktree add` for replay isolation — writes into the production repo's
  `.git`; the phase-3.0 gate precedent is `git archive` extraction. Use that.
- Reusing claimd's TS code via subprocess — two runtimes, one product; rewrite the
  (small) event+fold core in Rust.
- Parsing human-form `cargo test` stdout — libtest JSON / `--message-format=json`
  only; stdout parsing is the `--match` regex failure mode reborn.

## Global constraints

- All agentrec invariants (CLAUDE.md "Key semantics") hold; `attest.jsonl` is
  append-only with no rewrite class.
- Every task lands its ACs in `IMPLEMENTATION.md` §attest before code (house rule).
- Suite baseline at branch fork must be recorded in the first commit message;
  clippy debug+release + fmt clean per task; no test seams in release `strings`.
- Executor protocol: fresh-context Sonnet implementers per task, Opus review,
  Fable skeptic gate per phase (the ONE adversarial pass — no stacked
  self-verification loops).
- Merge order = phase order; each phase independently mergeable behind the
  `attest` subcommand being absent from help until phase 5 (flag-gated visibility,
  single const).

## Phase 1 — SPIKE: per-test coverage economics (scariest unknown, gate everything on it)

The whole staleness design rests on per-test executed-file maps being affordable.
Nobody has measured cargo-llvm-cov per-test granularity cost on this repo.

- Files: `docs/verify/attest-coverage-spike.md` (findings only; throwaway scripts in
  scratchpad, labeled throwaway).
- Prerequisite: `cargo-llvm-cov` + the `llvm-tools-preview` rustup component —
  NEITHER is installed on the dev machine as of this plan (`cargo llvm-cov
  --version` → "no such command"; `rustup component list --installed | grep
  llvm` → empty). Install first (`cargo install cargo-llvm-cov`, `rustup
  component add llvm-tools-preview`). If the executor's sandbox blocks
  installation (no network / no rustup access), that is itself a phase 1
  finding to record, not a silent stall.
- Probe: on agentrec itself (~1150 tests), measure (a) wall time of one full
  instrumented suite run, suite-level map; (b) per-test profile capture via
  `LLVM_PROFILE_FILE` templating over an isolated-process run of a 50-test sample,
  extrapolate; (c) map sizes; (d) file-set quality — do fixture reads and spawned
  `agentrec` binary invocations appear? (Integration tests spawn the real binary —
  instrumentation must follow the child or those tests' maps are empty. MEASURE
  this; it is the likely killer.)
- Exit criteria (explicit, downstream contract): a table wall-time/bytes for (a) and
  (b); a WRITTEN ruling choosing per-test vs suite-level fallback (spec decision 5
  sanctions the fallback); the chosen map's schema sketch for phase 4's consumer.
- Failure exit: if child-process coverage is unattainable for integration tests,
  the ruling documents scope = "unit-test claims only, integration claims stale on
  any src write" — still shippable, recorded honestly.
- Verification: the findings doc exists with all three numbers measured, not
  estimated; commit `spike: attest coverage economics on the dogfood corpus`.

## Phase 2 — claim core: events, fold, derive (pure, no execution)

- Files: `agentrec-core/src/attest/events.rs`, `agentrec-core/src/attest/fold.rs`,
  `ATTEST-FORMAT.md` (draft-unstable header).
- Interface contract (phase 3+ consume): `AttestEvent` enum {Derive, Evidence,
  Verdict, Stale, Human, ManualDeclare} with serde JSONL round-trip;
  `fold_claims(&[AttestEvent]) -> BTreeMap<ClaimId, ClaimState>`; `ClaimId =
  hash(adapter_id, test_identity)` stable across runs; `ClaimState` carries the
  spec's state machine incl. STALE overlay.
  **`test_identity` MUST include the cargo target/binary name, not just the bare
  fn name** — measured collision, not hypothetical: this repo today has
  `opaque_call_counts_as_importable_with_no_file_entries` and
  `missing_cwd_disqualifies_session_but_does_not_abort` as `#[test]` fns in BOTH
  `cli/tests/import_codex.rs` and `cli/tests/import_claude.rs` (two separate
  compiled test binaries). A bare-fn-name identity folds these into one claim,
  silently cross-attributing evidence/verdicts between unrelated tests. Compose
  `test_identity` from cargo's own JSON test-event target field (phase 1's
  `--message-format=json` probe already surfaces it — confirm the exact field
  during the spike, don't re-derive it in phase 2).
  **Open, founder-owned: does renaming a `#[test]` fn mint a new claim (old one
  orphaned/goes stale-forever) or carry continuity?** Two defensible answers,
  not decided anywhere in the spec or this plan. Recommend: new claim, no
  continuity — consistent with "tests are the claim language, never
  hand-authored" (spec decision 2); the cost is a renamed test's claim history
  starting over at DERIVED. State the choice here before phase 2 is chunked.
- AC-shaped test snippet: fold of [derive, evidence, stale, verdict(confirmed)]
  ends CONFIRMED with stale overlay dropped; verdict(recipe-invalid) then
  verdict(confirmed) ends CONFIRMED (retryable proven); author-run evidence alone
  can never reach CONFIRMED (state-machine test, not convention).
- Verification: `cargo test -p agentrec-core attest::` green; commit
  `attest: claim events + deterministic fold (core, pure)`.

## Phase 3 — cargo adapter + passive capture

- Files: `cli/src/attest/adapter_cargo.rs`, `cli/src/attest/capture.rs`, wiring in
  `cli/src/main.rs` (hidden subcommands `attest derive`, `attest run`).
- Contract (produces): adapter trait `discover() -> Vec<TestIdentity>`,
  `run(filter) -> Vec<StructuredResult>` (libtest JSON, no stdout regex);
  capture writes `evidence` events with `turn_id` = open turn from the existing
  signal path, blob = output into CAS.
  **Turn-id resolution mechanism, pinned (this is the one place phase 3/4 seam
  drift is likeliest):** `attest run` is a short-lived CLI process — it cannot
  call `TurnEngine::open_turn_id()` (`agentrec-core/src/engine.rs:148`)
  directly, that is live in-process state the DAEMON holds, and CLAUDE.md's own
  architecture invariant is "consumers never need the daemon running — all
  reads are file-based." Read the daemon's persisted mirror,
  `.agentrec/open.json` (see `cli/src/daemon.rs:2071`'s comment: "the open turn
  is mirrored to `.agentrec/open.json`"), same file the crash-recovery journal
  already reads. Missing/stale `open.json` → no open turn → the AC's existing
  dirty-bit/dev-loop-only path, not a new failure mode.
- ACs: `attest derive` on a fixture crate creates one claim per discovered test,
  idempotent re-run appends nothing; `attest run -- cargo test` inside a
  hook-bracketed session records evidence joined to that turn (fixture: emit the
  open-bracket signal first, the O5-proven pattern); dirty-tree evidence is
  recorded with the dirty bit set and renders as dev-loop-only in `attest status`.
- Verification: `cargo test --test attest_capture`; commit
  `attest: cargo adapter + passive evidence capture`.

## Phase 4 — verify (independent replay) + coverage staleness

- Files: `cli/src/attest/replaycmd.rs`, `cli/src/attest/coverage.rs`, daemon delta
  (stale-marking only) in `cli/src/daemon.rs` — **NOT
  `agentrec-core/src/daemon.rs`, which does not exist; the daemon lives in the
  `cli` crate (`cli/src/main.rs:9: mod daemon;`)**. Corrected from the prior
  draft, which named a nonexistent path.
- Contract: `attest verify [--all-stale|<id>]` extracts pinned HEAD via
  `git archive`, scrubbed env, adapter-runs the single test, appends verdict per
  spec taxonomy; coverage maps per phase-1 ruling; daemon consults maps on write
  events it already receives and appends `stale`.
  **Purity check, made diffable, not just "grep-provable":** `cli/src/daemon.rs`
  already contains 4 `Command::new("git")` call sites today, all inside
  `#[cfg(test)]` fixture setup (`nested_gitignore_precedence`,
  `self_matching_gitignore_still_filters_its_directory`,
  `self_matching_gitignore_edit_flags_and_rebuild_consumes_it`,
  `maybe_rebuild_runs_once_per_dirty_flag`) — a bare "grep for Command::new ==
  0" is already false today and would be an unmeetable AC as originally
  worded. Correct predicate: `grep -c 'Command::new' cli/src/daemon.rs`
  outside `#[cfg(test)]` blocks stays at the pre-phase-4 baseline (0
  non-test call sites, measured this session) — a diff against that baseline,
  not an absolute-zero grep.
- ACs: mutation probe — break the tested fn, verify → claim-false; delete the test,
  verify → recipe-invalid (not claim-false); restore + verify → confirmed (retry
  proven end-to-end); write to a covered file → daemon appends stale for exactly
  the mapped claims (and the over-stale direction for unmapped-but-suspect per
  phase-1 ruling); build with a stale binary hazard guarded (`cargo build` before
  probe — the recorded probe-hygiene scar).
- Verification: `cargo test --test attest_verify` + the two mutation probes in the
  task file run live; commit `attest: independent replay + coverage staleness`.

## Phase 5 — review cards, advisory gate, report (the UX phase)

- Files: `cli/src/attest/reviewcmd.rs`, `cli/src/attest/gatecmd.rs` +
  `cli/src/attest/reportcmd.rs`, unhide the subcommand. 4 touched files exceeds
  the ≤2–3/task guideline at phase granularity — expected, this phase is not
  yet task-chunked; `/pchunker` should split review+unhide from gate+report.
- Contract: `attest review` iterates pending manual/blocking items, renders
  criterion + evidence (diff via existing view, output blobs, related-turn list as
  pre-fill context), keys y/n/s append `human` events; `attest gate` folds and
  exits nonzero only on claim-false or unattested blocking manual items
  (recipe-invalid surfaces, never blocks); `attest report --range` emits md+json
  bundle with provenance chains.
- ACs: golden for the report on a fixture log; gate exit-code table test (all four
  verdict kinds × blocking/fyi); review flow test via scripted stdin — n-with-note
  appends the note verbatim; fyi items never appear in review or gate output.
- Verification: `cargo test --test attest_gate` + goldens; commit
  `attest: review cards, advisory gate, attestation report`.

## Risk ordering rationale

Phase 1 gates the design's only unmeasured assumption (coverage economics —
including the child-process question that could hollow out integration-test
claims). Phases 2–3 are low-risk pure/plumbing work that dogfoods immediately.
Phase 4 carries the security-sensitive surface (replay isolation, daemon purity)
and gets the heaviest gate. Phase 5 is UX polish on proven substrate.

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
