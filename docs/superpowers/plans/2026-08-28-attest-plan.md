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
- **`attest.jsonl` has multiple writers** (CLI commands appending `evidence`/
  `verdict`/`human`, the daemon appending `stale`) — this is a NEW class,
  `log.jsonl` and `signal.jsonl` are each single-writer-per-process by
  contrast. Reuse the existing solved pattern, don't reinvent it:
  `cli/src/loglock.rs`'s documented blocking/non-blocking append discipline
  (see also `cli/src/memlock.rs`) is the precedent. Phase 3 first-writer task
  wires the lock; phase 4 adds an AC asserting no torn line under a
  daemon-appends-`stale`-while-CLI-appends-`verdict` concurrent test.
- Every task lands its ACs in `IMPLEMENTATION.md` §attest before code (house
  rule) — **phase 1's task creates that section** (it does not exist yet:
  `IMPLEMENTATION.md` currently mentions "attest" only incidentally at lines
  38 and 518, neither is a section); every later phase appends to it, none
  create it.
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
- Exit criteria (explicit, downstream contract):
  1. A table of wall-time/bytes for (a) full suite and (b) the 50-test sample,
     both MEASURED (not estimated) — (b)'s workspace-wide extrapolation from
     the measured sample is a documented arithmetic step, distinct from
     estimating the sample itself.
  2. Probe (d)'s finding stated as a plain yes/no with evidence: does an
     instrumented run of an integration test that spawns
     `env!("CARGO_BIN_EXE_agentrec")` (e.g. `cli/tests/approve.rs:24`,
     `bisect.rs:19`, `golden.rs:235`, `hardening_daemon.rs:23` — real spawn
     sites, not a hypothetical) produce a non-empty coverage map for the
     spawned child, under `LLVM_PROFILE_FILE` inherited unmodified by the
     child process. This is the plan's own named "likely killer" — it does
     not get to be probed without becoming an exit criterion.
  3. The cargo JSON test-event field to use for `test_identity`'s
     target/binary component (phase 2's ClaimId depends on this by name, not
     by re-derivation — see phase 2).
  4. A WRITTEN ruling choosing per-test vs suite-level fallback (spec decision
     5 sanctions the fallback) — **and this ruling requires founder sign-off
     before phase 2 dispatch begins.** No pre-set numeric bar is set here
     deliberately (there's no basis for one before the numbers exist); the
     check against "any ruling satisfies the criterion" is the founder
     reading the actual measured table before phase 2 starts, not a
     threshold the spike author picks unsupervised.
  5. The chosen map's schema sketch for phase 4's consumer.
- Failure exit: if child-process coverage is unattainable for integration tests,
  the ruling documents scope = "unit-test claims only, integration claims stale on
  any src write" — still shippable, recorded honestly.
- Verification: the findings doc exists with items 1–3 and 5 measured/produced
  as specified above (item 2 is a yes/no + evidence, not a number; item 4 is the
  ruling + recorded founder sign-off); commit
  `spike: attest coverage economics on the dogfood corpus`.

## Phase 2 — claim core: events, fold, derive (pure, no execution)

- Files: `agentrec-core/src/attest/events.rs`, `agentrec-core/src/attest/fold.rs`,
  `agentrec-core/src/attest/types.rs` (new — see ownership note below),
  `ATTEST-FORMAT.md` (draft-unstable header).
- **Type ownership, pinned (phase 3's adapter and phase 2's fold share these —
  a seam neither side may re-declare):** `TestIdentity` and `StructuredResult`
  are defined in `agentrec-core/src/attest/types.rs`, NOT in `cli`. Core's
  `fold_claims` needs to hash/consume them, and core cannot depend on `cli`
  (dependency direction is `cli` → `agentrec-core`, never the reverse — same
  rule as every other core/cli split in this repo). Phase 3's adapter trait
  (`cli/src/attest/adapter_cargo.rs`) *constructs* these core types by
  running `cargo test`; it does not own their shape.
- Interface contract (phase 3+ consume): `AttestEvent` enum {Derive, Evidence,
  Verdict, Stale, Human, ManualDeclare} with serde JSONL round-trip;
  `fold_claims(&[AttestEvent]) -> BTreeMap<ClaimId, ClaimState>`; `ClaimId =
  hash(adapter_id, test_identity)` stable across runs; `ClaimState` carries the
  spec's state machine incl. STALE overlay.
  **`test_identity` MUST include the cargo target/binary name, not just the bare
  fn name** — measured collision, not hypothetical: this repo today has
  `opaque_call_counts_as_importable_with_no_file_entries` (`cli/tests/
  import_codex.rs:204`, `cli/tests/import_claude.rs:201`) and
  `missing_cwd_disqualifies_session_but_does_not_abort` (`import_codex.rs:242`,
  `import_claude.rs:281`) as `#[test]` fns in BOTH files (two separate compiled
  test binaries). A bare-fn-name identity folds these into one claim, silently
  cross-attributing evidence/verdicts between unrelated tests. Compose
  `test_identity` from cargo's own JSON test-event target field (phase 1's
  `--message-format=json` probe already surfaces it). **Phase 1 exit criteria
  (below) must additionally output the exact field name** — phase 2 consumes
  it, phase 1 must produce it; don't leave phase 2 to re-derive it.
  **Decided (founder, this session): renaming a `#[test]` fn carries claim
  continuity — same ClaimId, history preserved — not a fresh DERIVED claim.**
  Mechanism (adapter-generated, no hand-authored recipe, per spec lesson 1):
  - `derive` events gain a `body_hash: Option<[u8; 32]>` field — sha256 over
    the test fn's source byte span (`fn <name>(...) { ... }` through its
    matching closing brace, located by a depth-counting scan from the `fn`
    keyword; `None` if the scan can't find a balanced match, e.g. a
    macro-generated test body it can't locate in source). **Disclose the
    boundary explicitly in the code comment, per this repo's own recorded
    lesson (`awk-cannot-lex-rust`): this is a brace-depth scan, not a Rust
    parser — a brace inside a string/char literal or comment can desync it.
    A desync must fail closed to `None`, never emit a wrong hash silently.**
  - Fold reconciliation (in `fold_claims`, deterministic, not a heuristic
    applied ad hoc): within one `attest derive` invocation's event batch, if a
    previously-known `test_identity` is ABSENT from the new `discover()` set
    (vanished) and a NEW `test_identity` appears in the same batch whose
    `body_hash` matches the vanished claim's last recorded `body_hash`, that
    is a rename — keep the OLD `ClaimId`, append the new derive event under
    it with `renamed_from: Some(old_test_identity)`. No match (hash `None`,
    or no vanished claim with that hash) → new claim, no continuity (this is
    the sole fallback path, always available, never a hard failure).
  - Exercised end-to-end by a phase 3 AC (below), against phase 3's fixture
    crate — not restated here.
- AC-shaped test snippet: fold of [derive, evidence, stale, verdict(confirmed)]
  ends CONFIRMED with stale overlay dropped; verdict(recipe-invalid) then
  verdict(confirmed) ends CONFIRMED (retryable proven); author-run evidence alone
  can never reach CONFIRMED (state-machine test, not convention).
- Verification: `cargo test -p agentrec-core attest::` green; commit
  `attest: claim events + deterministic fold (core, pure)`.

## Phase 3 — cargo adapter + passive capture

- Files: `cli/src/attest/adapter_cargo.rs`, `cli/src/attest/capture.rs`,
  `cli/src/attest/statuscmd.rs` (new — minimal read-only status surface, see
  below), `cli/tests/attest_capture.rs`, wiring in `cli/src/main.rs` (hidden
  subcommands `attest derive`, `attest run`, `attest status`).
- **`attest status` is built here, minimally.** It's referenced by this
  phase's own AC (below), by the E2E tail item 2, and by spec:171
  ("`recipe-invalid` never blocks a gate by itself; it queues a retry and
  surfaces in `status`") — it has no owning phase otherwise. Scope for phase
  3: read `attest.jsonl`, fold, render claim counts by state + the dirty-bit/
  dev-loop-only flag from this phase's own capture AC. `recipe-invalid`
  surfacing (needs phase 4's verdict events to exist) and richer rendering are
  phase 4/5 extensions to this same file, not a new one.
- Contract (produces): adapter trait `discover() -> Vec<TestIdentity>`,
  `run(filter) -> Vec<StructuredResult>` (libtest JSON, no stdout regex) —
  both types defined in `agentrec-core/src/attest/types.rs` per phase 2's
  ownership note, constructed here from `cargo test`'s JSON output; capture
  writes `evidence` events with `turn_id` = open turn from the existing
  signal path, blob = output into CAS.
- **Capture has two paths, both in scope here (spec:138–140 defines both;
  only the wrapper was in the prior draft):** (1) the explicit
  `agentrec attest run -- <cmd>` wrapper; (2) recognition of test invocations
  arriving through the existing hook path (a `PostToolUse` signal whose
  command matches a test-runner pattern) — this is NOT a new ceremony step
  (spec lesson 3 forbids that): it rides the hook signal that already fires
  for any tool call, same as every other passive-capture path in this repo.
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
- ACs: `attest derive` on a fixture crate (new, `cli/tests/fixtures/
  attest_sample_crate/`) creates one claim per discovered test, idempotent
  re-run appends nothing; renaming a fixture test with its body unchanged →
  re-derive reuses the old ClaimId, changing the body too → new claim (phase
  2's reconciliation rule, exercised end-to-end here); `attest run --
  cargo test` inside a hook-bracketed session records evidence joined to that
  turn (fixture: emit the open-bracket signal first, the O5-proven pattern);
  a `cargo test` invocation arriving through the existing hook signal path
  (no explicit `attest run` wrapper) is recognized and captured the same way;
  dirty-tree evidence is recorded with the dirty bit set and renders as
  dev-loop-only in `attest status`; `attest status` on a fixture log renders
  claim counts by state.
- Verification: `cargo test --test attest_capture`; commit
  `attest: cargo adapter + passive evidence capture`.

## Phase 4 — verify (independent replay) + coverage staleness

- Files: `cli/src/attest/replaycmd.rs`, `cli/src/attest/coverage.rs`, daemon delta
  (stale-marking only) in `cli/src/daemon.rs` — **NOT
  `agentrec-core/src/daemon.rs`, which does not exist; the daemon lives in the
  `cli` crate (`cli/src/main.rs:9: mod daemon;`)**. Corrected from the prior
  draft, which named a nonexistent path. Also touches `cli/src/main.rs`
  (register `attest verify`) and `cli/tests/attest_verify.rs`. **5 files —
  over the ≤2–3/task guideline at phase granularity, same as phase 5;
  `/pchunker` should split the daemon delta (security-sensitive, gets its own
  task + the heaviest review) from replaycmd+coverage+main.rs wiring.**
- Contract: `attest verify [--all-stale|<id>]` extracts pinned HEAD via
  `git archive`, scrubbed env, adapter-runs the single test, appends verdict per
  spec taxonomy; coverage maps per phase-1 ruling; daemon consults maps on write
  events it already receives and appends `stale`.
  **Purity check — binding mechanism is a lint, not a grep.** A text scan
  (`grep`/`awk`) over `cli/src/daemon.rs` cannot distinguish "the daemon calls
  `Command::new` directly" from "the daemon calls into
  `cli/src/attest/coverage.rs`, which calls `Command::new`" — the second is
  the actual violation (spec:169's "no attest execution code path lives in
  the daemon [call] path") and a same-file grep is blind to it by
  construction. Use the mechanism this repo already proved for this exact
  defect class (`clippy.toml disallowed-methods`, PR #20 / CLAUDE.md's
  fsguard round — confirmed present: `clippy.toml:42` has a live
  `disallowed-methods` list, and `clippy.toml:38` documents that exemptions
  are per-site `#[allow(clippy::disallowed_methods)]`, not config-level
  scoping). Add `std::process::Command::new` to that list, crate-wide (the
  lint has no per-module scoping in `clippy.toml` itself — same as the
  existing fs-read entries). The 2–3 legitimate spawn sites this plan itself
  introduces (`adapter_cargo.rs`'s `cargo test` invocation,
  `replaycmd.rs`'s `git archive`, `coverage.rs`'s `cargo-llvm-cov`) each get
  an explicit `#[allow(clippy::disallowed_methods)]` at the call site, same
  pattern as the fsguard census. `daemon.rs` gets none — so any path that
  reaches a spawn from the daemon, direct or transitive, reds the build.
  This is enforced by the compiler across the whole call graph, not just one
  file — closes the transitive-exec hole a grep cannot see.
  **Consequence to budget for:** `daemon.rs`'s existing 4 test-fixture
  `Command::new("git")` sites (all inside `#[cfg(test)]`, listed above) will
  also need `#[allow(clippy::disallowed_methods)]` once this entry lands,
  since `cargo clippy --all-targets` (global constraints: "clippy debug+
  release") lints test code too — they are legitimate test-only git-init
  calls, not attest scope creep. Annotate them in this same task; a clippy
  red on pre-existing tests is not a phase-4 regression to chase down later.
  For a human-readable spot-check during review only (not the gate):
  `awk '/^#\[cfg\(test\)\]/{t=1} !t && /Command::new/{c++} END{print c+0}'
  cli/src/daemon.rs` currently prints `0` (the file's one `#[cfg(test)]`
  block, at the tail, holds all 4 existing `Command::new("git")` test-fixture
  sites: `nested_gitignore_precedence`,
  `self_matching_gitignore_still_filters_its_directory`,
  `self_matching_gitignore_edit_flags_and_rebuild_consumes_it`,
  `maybe_rebuild_runs_once_per_dirty_flag`). **Disclosed boundary: this is a
  brace-unaware text scan over Rust source, the repo's own recorded
  `awk-cannot-lex-rust` failure class — it happens to work today because
  `daemon.rs` has exactly one top-level `#[cfg(test)]`, and breaks the moment
  a second one is added above the existing tests. It is NOT the gating
  mechanism; the clippy lint is.**
- ACs: mutation probe — break the tested fn, verify → claim-false; delete the test,
  verify → recipe-invalid (not claim-false); restore + verify → confirmed (retry
  proven end-to-end); write to a covered file → daemon appends stale for exactly
  the mapped claims (and the over-stale direction for unmapped-but-suspect per
  phase-1 ruling); build with a stale binary hazard guarded (`cargo build` before
  probe — the recorded probe-hygiene scar); daemon appending `stale` concurrently
  with a CLI `attest verify` appending `verdict` produces no torn/interleaved
  line in `attest.jsonl` (the `loglock.rs`-pattern append test, global
  constraints); `cargo clippy` reds if a `Command::new` call is introduced
  reachable from the daemon's stale-marking path (plant-probe: temporarily add
  one, confirm the lint fires, then remove it — same proof shape as PR #20's
  fsguard `disallowed-methods` census).
- Verification: `cargo test --test attest_verify` + the two mutation probes in the
  task file run live; commit `attest: independent replay + coverage staleness`.

## Phase 5 — review cards, advisory gate, report (the UX phase)

- Files: `cli/src/attest/reviewcmd.rs`, `cli/src/attest/gatecmd.rs` +
  `cli/src/attest/reportcmd.rs`, `cli/tests/attest_gate.rs`, unhide the
  subcommand in `cli/src/main.rs`. 5 touched files exceeds the ≤2–3/task
  guideline at phase granularity — expected, this phase is not yet
  task-chunked; `/pchunker` should split review+unhide from gate+report.
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
