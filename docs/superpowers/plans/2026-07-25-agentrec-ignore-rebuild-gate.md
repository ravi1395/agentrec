# Plan: ignore-set rebuild gate — 3 phases, branch `fix/ignore-rebuild-gate` (off `fix/blame-attribution-and-noise-folding`)

Pulled out of `docs/superpowers/plans/2026-07-25-agentrec-churn-honesty-round.md` Phase 5, where it
was a rider. It is a real defect in already-merged code (`e453e86`), so it ships on its own.

Conflict order: PROTOCOL.md > IMPLEMENTATION.md > SPEC.md > this plan.

## The defect (verified firsthand at HEAD `8da3e16`, not inferred)

`e453e86` fixed the *trigger*: `apply_watch_result` sets `gitignore_dirty` at event-ingest time by
filename, independent of the path's own ignore verdict (`daemon.rs`, the `.gitignore` filename
check precedes `classify`). That fix is correct and its unit test
(`daemon::tests::self_matching_gitignore_edit_still_flags_a_rebuild`) is real.

It never fixed the *consumption*. The rebuild lives inside `if settled || capped` in `daemon::run`,
and both predicates derive from `last_event`/`first_event` — which `apply_watch_result` arms **only
for `Class::Watch`**. So:

> Edit `.gitignore` to add `!keep.log` → event ingested, `gitignore_dirty = true`, path classifies
> `Ignore`, no timer armed. Write `keep.log` → classified against the **stale** set → `Ignore`, no
> timer armed. `settled == false`, `capped == false`, forever. **The rebuild never runs and the file
> is never recorded**, until some unrelated watched path happens to change.

Direction is under-record (never over-record), which is why it has not corrupted anything — but this
is a flight recorder silently not recording a file the user explicitly re-enabled. It is also the
second time this exact class has shipped: `049a4aa` introduced the trigger defect, `e453e86` fixed
the trigger and left the gate.

**Why no existing test catches it.** The unit test asserts the flag is *set*; it drives
`drain_watch_events` directly and never reaches the loop that consumes it. The real-daemon test
`self_ignoring_gitignore_dir_is_not_recorded_by_a_real_daemon` (`cli/tests/integration.rs`) proves
*ignoring*, never *re-widening*. `records_rich_turn_..._filters_ignored` uses a root-level,
non-self-matching `.gitignore` written before the daemon starts, so no rebuild is involved at all.
Nothing in the suite exercises a mid-run ignore-rule change.

## Decisions log

1. **Rebuild moves to the top of the loop tick, gated on the dirty flag, before
   `drain_watch_events`** — not into `apply_watch_result`. Rebuilding inside ingest would call
   `IgnoreSet::build` (a full repo walk) once per event on a hot `.gitignore`; top-of-tick bounds it
   to at most one walk per `POLL` (250 ms), and events drained in that tick are then classified
   against the fresh set.
2. **The residual window is stated, not eliminated.** Classification happens at ingest, so an event
   arriving in the *same* drain batch as the ignore-file edit is still classified against the
   pre-edit set. After this fix the hole is "the next mutation of that path is honored" (≤1 tick +
   notify latency) instead of "never, until unrelated activity". Closing it fully means reclassifying
   `pending` at flush time — a larger change, deliberately not taken here. Recorded in the module doc.
3. **Losing the triggering event is acceptable and must be said plainly.** `Recorder::stage` reads
   *current* file bytes at flush, so a dropped intermediate event costs an intermediate snapshot, not
   the content — the next mutation snapshots the file as it then stands.
4. **Scope stays on `.gitignore`.** `IgnoreSet::build` collects matchers exclusively from
   per-directory `.gitignore` probes (`dir.join(".gitignore")`), so `.ignore` and `.git/info/exclude`
   are not matcher sources and need no trigger. (They *do* prune `build`'s traversal walk, an
   unrelated asymmetry — noted, not addressed. See Q2.)
5. **No behavior change to what is recorded under a stable config.** This fix only affects the
   window after an ignore-rule change.

## Global constraints / invariants

- Append-only: `log.jsonl` / `signal.jsonl` never rewritten.
- `cargo clippy -- -D warnings` + `cargo fmt --check` clean on debug **and** release.
- Integration tests run `--test-threads=3` (FSEvents contention).
- Every fixture touching `ignore::` does a real `git init` — `WalkBuilder::require_git` defaults
  true, and a non-git tempdir applies no ignore rules at all, so the fixture tests nothing. This trap
  already made `nested_gitignore_precedence` pass vacuously for months.
- Any test-only env seam is `#[cfg(debug_assertions)]`-gated; release `strings` must not contain it.

## Executor protocol

One phase per commit on `fix/ignore-rebuild-gate`. **claimd declare-first, one claim per acceptance
criterion, before implementing.** After each phase, `skeptical-reviewer` gates against that phase's
criteria **in an isolated git worktree**. Every criterion names its **neuter** — the cheapest
deletion or short-circuit that must turn a **named** test RED. Phase 1 is TDD: write the E2E, prove
it RED against unmodified `daemon.rs`, then fix; both land in one commit so the tree stays green.

Merge order 1 → 2 → 3. Phase 3 is tests-only and independent; it may run in a parallel worktree.

## Baseline — **UNVERIFIED, re-run before Phase 1**

`cargo test --workspace -- --test-threads=3` → **386 passed, 0 failed, 1 ignored** at `c4ada8f`,
carried from the prior round's receipt and not re-run. Correct every phase total below in one edit
if it differs.

---

## Phase 1 — Consume the dirty flag on the loop tick, not on flush

**Description:** Move the `IgnoreSet` rebuild out of the `settled || capped` block to the top of
`daemon::run`'s loop, gated on `gitignore_dirty`, so an ignore-rule change is honored even when the
only subsequent activity is on paths the stale rules ignore. Prove it with a live daemon, because no
seeded fixture can reach this — the bug lives in the loop's control flow, not in a predicate.

**Files:** `cli/src/daemon.rs` (edit — `run`'s loop; strengthen
`self_matching_gitignore_edit_still_flags_a_rebuild`), `cli/tests/integration.rs` (edit — new
real-daemon test).

**Changes:**
- `run`: rebuild block moves above `drain_watch_events`, executed when `gitignore_dirty`, clearing
  the flag after. Removed from the flush block.
- Module doc records decision 2's residual window and decision 3's "intermediate snapshot lost, not
  content" consequence, in the same commit as the code — a stale invariant comment beside the
  invariant is the doc-drift class that already produced a GATE FAIL here.
- Strengthen the existing unit test so it asserts *consumption*, not just the flag: after the drain,
  assert the flag is observable to the rebuild step and that a rebuilt `IgnoreSet` reflects the
  edited rules. Rename it if the name now overstates what it proves.
- Extract `maybe_rebuild(dirty: &mut bool, root: &Path) -> Option<IgnoreSet>` so the rate rule has a
  unit-reachable target — `run`'s loop itself is not unit-testable, which is exactly why the bug
  survived.
- New live-daemon test `unignore_is_honored_without_other_watched_activity`: `git init` tempdir,
  `cache/.gitignore` = `*`, real `agentrec init && agentrec record`; wait for a live daemon; **write
  the positive-control file FIRST and assert it lands in a turn**; only then rewrite
  `cache/.gitignore` to `*\n!keep.log`; **touch nothing else for the rest of the test**; write
  `cache/keep.log` twice, spaced past one `POLL` tick; wait past debounce + quiet window; assert a
  turn exists whose files contain `cache/keep.log`.
  **The control must precede the ignore-rule edit, not follow it.** A control written afterwards is a
  `Class::Watch` path: it arms `last_event`, `settled` fires, the `settled || capped` block runs, and
  the rebuild happens **under current code** — the test would pass pre-fix and prove nothing. "Some
  unrelated watched path changed" is the escape hatch this defect is defined by; the test must not
  contain it. Control-first is sufficient because the post-fix assertion is *presence*: a dead daemon
  yields a false RED, never a false pass. The control exists only to make the RED run attributable.
- **Verified before writing this plan:** `IgnoreSet::is_ignored` maps `ignore::Match::Whitelist` to
  `false` (`daemon.rs`), so `!keep.log` genuinely re-widens after a rebuild. The fixture does not
  rest on an unchecked assumption about negation semantics.

**Acceptance criteria:**
- [ ] `unignore_is_honored_without_other_watched_activity` **fails against unmodified `daemon.rs`**
      and passes after the move — demonstrated in the receipt with both outputs, not asserted.
      Neuter: move the rebuild back inside `if settled || capped` → that named test RED.
- [ ] The test's daemon is proven alive by a positive control written **before** the ignore-rule
      edit and asserted recorded there, after which nothing but `cache/keep.log` is touched. Neuter:
      move the control to after the edit → the test passes against unmodified `daemon.rs`, which the
      executor must demonstrate and then revert (it is the reason the ordering is specified).
- [ ] `maybe_rebuild` returns `Some` exactly once for a run of N consecutive dirty-flag settings and
      clears the flag. Neuter: drop the flag clear → `maybe_rebuild_runs_once_per_dirty_flag` RED.
      (This is why the helper is extracted in this phase — the loop body it lives in is not
      unit-reachable, and an AC with no possible test is not an AC.)
- [ ] Behavior under a stable config is unchanged: `self_ignoring_gitignore_dir_is_not_recorded_by_a_real_daemon`
      and `records_rich_turn_..._filters_ignored` pass untouched and un-weakened.
- [ ] The strengthened unit test asserts its own precondition (the edited `.gitignore` classifies
      `Ignore`), so it cannot pass for the wrong reason.

**Expected test outputs:** `cargo test -p agentrec --test integration -- --test-threads=3` → +1;
`cargo test -p agentrec daemon::` → **+1** (`maybe_rebuild_runs_once_per_dirty_flag` is new; the
other unit test is strengthened, not added). Workspace → **388 passed, 0 failed, 1 ignored**.
Receipt must paste the RED run.

**Corrected in flight (was 387):** the original ladder forgot that this phase's own AC names a new
unit test. The whole ladder shifts by one — Phase 2 → 391, Phase 3 → 393. Caught by the Phase 1
reviewer, not by the plan author.

**DELIVERED at `0a7b279`.** Baseline independently re-run at the parent (`bdb911c`) → **386/0/1**,
so the carried figure was right. Post-fix workspace **388/0/1**, confirmed three ways (implementer,
Opus reviewer, orchestrator). Opus reviewer verdict **DONE**: every criterion proven by a neuter that
went RED — placement neuter reds the E2E; flag-clear neuter reds two unit tests; and the reviewer
demonstrated that moving the positive control to *after* the ignore-rule edit makes the E2E pass
against pre-fix code, which is the vacuity this plan was corrected to avoid. Claims
`clm_3NC55…`, `clm_5CJWF…`, `clm_5PCCR…`, `clm_29RN5…`, `clm_1H62N…` declared at the parent commit
and evidenced.

---

## Phase 2 — Make ignore reloads observable (the defect was invisible for a reason)

**Description:** This class has now shipped twice because nothing anywhere reports whether the
filter configuration was ever reloaded. A user cannot distinguish "my new rule is active" from
"the daemon is still running last week's rules" by any means short of reading source. Phase 1 also
makes rebuilds possible once per tick, so their frequency becomes worth watching on a 24/7 daemon.

**Files:** `cli/src/daemon.rs` (edit — count + log the rebuild), `cli/src/state.rs` (edit —
persisted counter), `cli/src/cmds.rs` (edit — `status` line).

**Changes:**
- `state.json` gains `ignore_rebuilds: u64` and `last_ignore_rebuild_ms`, written on the existing
  state-persist path (no new write site; `agentrec-core/src/perms.rs` still owns permissions).
- One stderr line per rebuild naming the matcher count, in the daemon's existing log voice —
  `ignore rules reloaded (4 matchers)`. Rate is already bounded by the dirty flag.
- `status` renders the last reload time when a rebuild has ever happened; renders nothing when the
  counter is zero (never a vacuous "0 reloads" line on a repo with no `.gitignore` churn — the
  zero-turn `rich-rate: n/a` precedent from D-PD3).
- Additive JSON field only; nothing on the PROTOCOL wire (`state.json` is operational state, which
  PROTOCOL §5 deliberately keeps off the wire — say so in the field's doc).

**Acceptance criteria:**
- [ ] A live daemon that reloads once writes `ignore_rebuilds: 1`; a second `.gitignore` edit makes
      it 2. Neuter: delete the increment → `daemon_counts_ignore_rebuilds` RED.
- [ ] A daemon run with no `.gitignore` change ends with the counter at 0 and `status` prints **no**
      reload line. Neuter: make the line unconditional → `status_omits_reload_line_when_never_reloaded` RED.
- [ ] `status --json` carries the field; a pre-existing `state.json` without it parses (serde
      default) and renders as never-reloaded. Neuter: drop the default → `status_tolerates_state_without_rebuild_counter` RED.
- [ ] `.gitignore` rewritten 5 times in quick succession yields `ignore_rebuilds >= 1` and
      `<= 5` — asserted on the **persisted counter**, never by counting stderr lines inside a 250 ms
      window. FSEvents coalesces and this repo has documented flake from exactly that timing
      assumption. Neuter: increment per event in `apply_watch_result` instead of per rebuild →
      `rebuild_count_is_bounded_by_writes` RED on the upper bound.

**Expected test outputs:** `cargo test -p agentrec --test integration -- --test-threads=3` → +2;
`cargo test -p agentrec cmds::` → +1. Workspace → **391 passed, 0 failed, 1 ignored** (ladder
corrected from 390; see Phase 1).

**Carried in from the Phase 1 review — a new cost this phase is the mitigation for:** pre-fix, a repo
where only `.gitignore` churns and nothing watched ever changes did **zero** `IgnoreSet::build`
walks, because the flush block never ran. Post-fix it does up to **4 full-repo walks per second,
indefinitely**. Decision 1 chose top-of-tick precisely to bound this to one walk per `POLL`, so it is
a declared trade — but the inline comment at the call site documents only the residual window and the
lost-event consequence and says nothing about walk rate. Add that sentence here, and treat
`ignore_rebuilds` as the instrument that makes the rate observable rather than theoretical.

---

## Phase 3 — Close the vacuity in the neighbouring ignore tests (tests only)

**Description:** The reason this defect survived two rounds is that nothing exercises a mid-run
ignore-rule change at all. Add the two cases nothing covers, so the next change to this code cannot
pass for the wrong reason. Tests only — independently mergeable, no behavior change.

**Correction, made before this plan was committed:** CLAUDE.md records `nested_gitignore_precedence`
as passing **vacuously** in a non-git tempdir. That describes the **pre-D29** state and is no longer
true at HEAD. `IgnoreSet::build` now probes `dir.join(".gitignore")` for each directory the walk
reaches instead of relying on the walk to *yield* the ignore file, and a non-git walk still yields
directories — so both matchers are collected and the test's precedence assertions are real. Adding
`git init` is still worth doing (it makes the fixture match how the code runs in production, where
`require_git` governs pruning), but it is **hygiene, not a vacuity fix**, and no AC may claim
otherwise.

**Files:** `cli/src/daemon.rs` (edit — unit tests only), `cli/tests/integration.rs` (edit).

**Changes:**
- `nested_gitignore_precedence`: add a real `git init` plus a precondition assert on the matcher
  count, so the test states what it depends on. Print `matchers.len()` once while doing this and
  record the number in the receipt — if it is 0, the correction above is wrong and Phase 3 becomes a
  genuine vacuity fix.
- New: **deleting** a `.gitignore` mid-run re-widens recording (the mirror of Phase 1's un-ignore
  case; the trigger fires on deletion events too, and nothing pins it).
- New: a `.gitignore` **created** mid-run under a directory that had none is honored without a
  daemon restart.
- Audit and repair dead assertions in the touched neighbourhood, in the style of the known-dead
  `!stdout.contains("REVERT  big.bin")` clause (renderer emits lowercase, so that assert has never
  been able to fail): every negative `contains` assert gets a sibling positive assert proving the
  string is emitted somewhere.

**Acceptance criteria:**
- [ ] `nested_gitignore_precedence` asserts `matchers.len() >= 2` before its precedence assertions,
      and its fixture is a real git repo. **No neuter exists and none is claimed** — per the
      correction above, removing `git init` is expected to change nothing at HEAD. This is a
      documented hygiene change, recorded as such rather than dressed up as a proof.
- [ ] Deleting a `.gitignore` mid-run causes previously-ignored paths to be recorded on their next
      mutation. Neuter: revert Phase 1's move → `deleted_gitignore_rewidens_recording` RED.
- [ ] A newly created `.gitignore` is honored without restart. Neuter: same → `new_gitignore_honored_without_restart` RED.
- [ ] Every negative `contains` assert in the touched tests has a sibling positive assert. Verified
      by review, not by a test — marked as such, not claimed as a PASS.

**Expected test outputs:** `cargo test -p agentrec daemon::` → +0 (one test strengthened);
`cargo test -p agentrec --test integration -- --test-threads=3` → +2. Workspace →
**393 passed, 0 failed, 1 ignored** (ladder corrected from 392; see Phase 1).

**Also in scope, carried from the Phase 1 review:** the new E2E calls `daemon.kill()` after the
control assert, so an early control failure drops the `Child` unkilled. Ten integration tests share
that bare `let _ = daemon.kill()` pattern and only one uses `SingleDaemonGuard`. Leaked test daemons
have already cost this repo a diagnosis round (FSEvents contention, ~6 unattributed failures), and
this test is deliberately run RED during TDD, so it leaks by design during development. Route the new
test through the guard; converting the other nine is explicitly NOT in scope.

---

## Edge cases considered

- **Same-drain-batch ordering:** an event for a path arriving in the same batch as the `.gitignore`
  edit is classified against the stale set (decision 2's residual). Pinned by comment, not by test —
  a test would pin the *bug*, not the contract.
- **Hot `.gitignore`:** a tool rewriting it continuously bounds to one `IgnoreSet::build` per tick;
  Phase 2's counter makes that visible if a real repo ever hits it.
- **Deletion vs modification:** both produce events carrying the filename, so both set the flag.
  Phase 3 pins deletion.
- **`.gitignore` under a pruned subtree** (`node_modules`, `.git`, `.agentrec`): the trigger is
  filename-only, so it fires and causes one harmless extra rebuild. Cheap; not worth a path filter.
- **Case-insensitive APFS:** `.GITIGNORE` resolves to the same file but the `file_name()` compare
  misses it — the same macOS/Linux divergence already recorded as INFO for `dir.join(".gitignore")`.
  Carried forward unchanged, not introduced here.
- **Crash between rebuild and flush:** none — the rebuild mutates in-memory filter state only; the
  crash journal and `open.json` are untouched.
- **Non-git fixture:** applies no ignore rules at all; every fixture in this plan does `git init`.
- **Zero `.gitignore` in the repo:** flag never set, rebuild never runs, `status` prints no reload
  line, counter stays 0.

## Open questions (answer before implementation)

1. **[gates Phase 2's `status` line]** Should the reload line appear in bare `status` at all, or only
   in `doctor` / `status --json`? `status` is the daily-driver surface and its line budget is already
   contested (rich-rate, memory, over-budget notice). Options: (a) `status` shows it only when a
   reload happened in the current daemon epoch; (b) `doctor` only, with `status --json` carrying the
   field; (c) `status` always when the counter is non-zero (this plan's assumption).
2. **Not in scope, decide whether it becomes its own item:** `IgnoreSet::build`'s traversal walk
   honors `.ignore` and `.git/info/exclude` (WalkBuilder defaults), but `is_ignored` consults only
   collected `.gitignore` matchers. So a directory excluded by `.git/info/exclude` is pruned from the
   walk — its own `.gitignore` is never collected — while its files are *not* filtered at classify
   time. Direction is over-record (safe), magnitude unmeasured. Separate defect, separate round?
