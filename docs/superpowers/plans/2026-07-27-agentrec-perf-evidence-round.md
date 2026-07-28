# Plan: Perf-evidence round — measure the three evidenced hot-path gaps (4 phases, branch/worktree: `fix/perf-evidence-round`, cut from `main` @ `834f477`)

**Revision 6 — plan-level skeptic gate: PASS (2026-07-28).** Gate history: v1 FAIL (B1–B7:
wrong enum-change shape, unnamed test landmines, unobservable counters, ambiguous neuters,
ladder arithmetic); v2 FAIL (B8: the protected-bytes honesty line is manufactured by the
destructive pass and `enforce_budget` had no dry-run; B9: the retarget fixture's `open.json`
seed is deleted by the live daemon's own tick); v3 FAIL (B10: the tick-interleave AC was
vacuous — a freshly-staged blob is never an eviction candidate — the SR6
green-with-guard-deleted class); v4 FAIL (B11: execute's re-check timestamp was unspecified;
a fresh-`now()` re-check would be *weaker* than the unsplit code, and the offered filetime
bump could not discriminate); v5 FAIL (B12: execute's re-check masks a plan-only guard
neuter from every deletion-observable test — the plan-report leg now discriminates it);
v6 PASS with NB15/I4/I5 fixed post-gate. All 12 blocking findings closed; every one was
verified firsthand against real code by the gate. **The gate's own scope note stands: this
is a plan-level pass — two things only the final code-level skeptic can settle are (1) the
tick's harvest→plan→execute being one unbroken statement at the emitted call site, and
(2) AC2b.2's ~2s-seam timing against a real daemon, which must be run solo and repeatedly
(a single clean run of a timing-coupled behavior is not evidence in this repo).**

**Goal (≤5 lines):** agentrec has zero performance instrumentation. Three gaps have real
evidence behind them: (1) the hook recall path silently degrades to zero memories as the
store grows, with no p99 number anywhere; (2) `status` (text path) hard-deletes blobs from a
read verb — already produced one live data-loss bug; (3) the dedup-hit heal re-read is an
unmeasured per-batch I/O cost flagged "watch in dogfood" and never watched. This round adds
measurement seams and takes the 10k measurement the VERIFY-LEDGER envelope row names.
**Evidence round, not optimization round** — no index, no cache, no watch pruning.

**Baseline (VERIFIED at `834f477`, 2026-07-27, by the v1 gate's own run):** macOS
`cargo test --workspace -- --test-threads=3` → **428 passed / 0 failed / 1 ignored**, exit 0.
Executor re-runs at worktree HEAD; if ≠428, STOP and report.

## Decisions log (numbered; executors may not re-litigate)
1. **Instrumentation-first, optimization-never (this round).**
2. **The dedup-hit re-read stays.** `store.rs:72` re-reads deliberately (A4 heal :73-77,
   A3 mtime bump :79-85). Phase 3 counts it; nothing removes or conditionalizes it.
3. **The ledger envelope row is governed by its written criterion.** VERIFY-LEDGER.md:16
   closes on "p99 < 50ms (or block correctly suppressed past budget)". P1 records the figure
   either way; CLOSED only if the criterion is met, else OPEN-with-figure. Superseding is a
   founder act (Q2) — the ratchet forbids loosening here.
4. **Phase 2b is (a)-shaped or not this round.** ACs written for Q1=(a) only; (b)/(c) →
   revision.
5. **The protected-bytes honesty line survives via a dry-run, not by deletion (closes B8).**
   `status_attributes_protected_bytes` (cmds.rs:1915) pins a deliberate honesty-round
   deliverable ("an unexplained '0 freed' is exactly the dishonest status class" —
   cmds.rs:448-451), but its `protected` figure is manufactured *inside* the destructive
   pass (retention.rs:129-142, same function that deletes at :153). Deleting the line
   (option i) was rejected: it removes a shipped honesty surface to make a refactor easier.
   Instead Phase 2a splits `enforce_budget` into plan/execute so `status` can render the
   same truth read-only.
6. **`dedup_reread_bytes` counts clean dedup-hit verification reads only.** The
   corrupt-fallthrough path (store.rs:96-97 → `finish_stored`) re-reads but returns
   `deduped: false, reread_bytes: 0` — its cost is a heal event, not steady-state dedup
   cost, and mixing them would blur the number this round exists to get (closes NB2). The
   heal test at store.rs:590 therefore asserts exactly those concrete values.
7. **Q1 = (a), founder-confirmed 2026-07-27.** Eviction moves to the daemon tick; `status`
   becomes a pure read verb; the daemon-down + `undo`-growth residual is accepted as rare,
   bounded, and self-announcing (AC2b.3's in-product line). Phase 2b is unblocked.
8. **Q2 = default, founder-confirmed 2026-07-27.** Decision 3 stands as written: a breached
   p99 leaves the ledger row OPEN-with-figure. No criterion is superseded. Revisit only if
   the measured number actually breaches.

## Infeasible / rejected (verified against real code — do not resurrect)
- **Removing the dedup-hit re-read** — Decision 2 (behavior tests: heal store.rs:591,
  mtime store.rs:609).
- **Linux inotify watch pruning (gap 4)** — deferred; reintroduces the new-dir arming race
  class the residuals round closed. No exhaustion evidence. Revisit only with a real
  `max_user_watches` incident or measured count near limit.
- **log.jsonl indexing / blob compression / incremental IgnoreSet** — measured or reasoned
  non-issues (log full-parse 10ms @ 2006 turns, 2026-07-27; store 98% frozen churn;
  IgnoreSet bounded per POLL tick, daemon.rs:186-192).
- **In-process recall cache** — hook is a fresh process per prompt; caches nothing.
- **A committed 10k-seeding script** — this repo just shipped two defects in a
  never-executed script. Seeding commands go verbatim in the ledger row instead.
- **A sibling `PutResult::Deduped` variant** — `concurrent_identical_snapshot_one_intact_blob`
  (store.rs:404-421) pattern-matches `Stored` on all 4 racing threads; a variant split makes
  it race-dependent (v1 gate B1).
- **Seeding `open.json` as the protect-channel in a live-daemon eviction test** —
  gate-proven infeasible (B9): `sync_journal` runs every tick (daemon.rs:313, POLL=250ms)
  and its idle arm deletes `open.json` (daemon.rs:1692-1697) within ~250ms, long before any
  eviction seam fires. Live-daemon protect proofs must use channels the daemon never
  rewrites: a torn `log.jsonl` line and a `memory.jsonl` pin (daemon only ever appends to
  both).

## Global constraints
- Append-only `log.jsonl`/`signal.jsonl`/`memory-stats.jsonl`; never delete user data
  outside sanctioned paths; additive protocol only.
- Test seams `#[cfg(debug_assertions)]` only; release `strings` clean of `AGENTREC_TEST`
  (CI-checked).
- claimd declare-first per criterion at the parent commit, before implementing.
- README.md rides in the same commit as any public-surface change; README/VERIFY-LEDGER
  edits are docs ride-alongs outside the file cap.
- Executor protocol: one Sonnet implementer per phase in the worktree; orchestrator re-runs
  the full suite after every phase independently; binding fable skeptical-reviewer done-gate
  at the end. **Merge order P1 → P3 → P2a → P2b** — P2b depends on P2a (plan/execute API);
  P2a/P2b run last as the largest coupled pair (the Q1 gate that originally forced this
  order is resolved — Decision 7). P1 and P3 both touch `cli/src/cmds.rs` (P3's touch is
  two mech lines + one assert extension); sequential merge, trivial overlap.

---

## Phase 1 — Recall latency evidence: persist `elapsed_ms`, add a stats summarizer, take the 10k measurement
**Description:** `started: Instant` (`cmds.rs:750`) is in scope at all six stats append
sites, but elapsed is only computed on the retrospective path (`cmds.rs:796`, feeding :798)
— the early-bail sites (:777, :792) never compute it, and no site persists it. Persist on
**all six** outcome sites, add `agentrec memories --stats`, run the release-build 10k
measurement, record the figure under Decision 3.
**Files:** `cli/src/cmds.rs` (edit — sites :777/:792/:802/:819/:829/:847),
`cli/src/memorycmds.rs` (edit — summarizer), `cli/tests/integration.rs` (edit — 2 tests).
`cli/src/main.rs` (mech — clap arm). README.md + VERIFY-LEDGER.md (docs).
**Changes:**
- `cmds.rs`: every stats line gains `"elapsed_ms": <u64>` from `started` (monotonic).
  Both budget-bail sites :792 and :802 emit byte-identical lines today and race under the
  forced-bail seam (`AGENTREC_TEST_FORCE_RECALL_BUDGET_EXCEEDED`, cmds.rs:686 → past
  deadline → `recv_timeout(0)`, either arm wins) — both must carry the field.
- `memorycmds.rs`: `memories --stats` reads `memory-stats.jsonl` **directly**, never through
  `memories()`'s `load_effective` preconditions (memorycmds.rs:613-618) — a corrupt
  `memory.jsonl` must not poison a stats readout of a different file. Three buckets, all
  reported: (i) measurable, (ii) parseable-pre-upgrade → `N pre-upgrade lines (no
  elapsed_ms)`, (iii) torn → `N unparseable lines skipped`. Prints count/p50/p90/p99/max
  over bucket (i) + per-outcome counts. Empty/absent → `no hook invocations recorded`,
  exit 0. Percentiles: nearest-rank, sorted sample.
**Acceptance criteria:**
- [x] AC1.1 **MET** (`c784714`): forced-bail seam active → appended line carries `elapsed_ms`
      whichever bail site wins. Neuter: remove from **both** :792 and :802 → red
      deterministically. Separate assertion covers the success path (:847).
      *Independently re-derived by the phase reviewer: neuter RED 3/3 for the right reason
      (assertion message, not a compile error), source restored byte-identically to
      `d1f3f30a…`. The success-path assert is load-bearing — neutering site 6 alone reds it
      by name.*
      **PLAN CORRECTION (measured, 2026-07-28):** this AC's stated rationale — that the two
      bail sites "race under the forced-bail seam (either arm wins)" — is **FALSE**. The
      reviewer instrumented both arms with a discriminator and measured **60/60 in favour of
      the `recv_timeout` Err arm** (30 clean + 30 under 6-way CPU load); site 3 never fired.
      Cause: std's `recv_timeout` does an optimistic `try_recv` before `recv_deadline(now+0)`,
      and the freshly-spawned worker has not sent yet. The AC still holds as written (its
      neuter removes the field from **both** sites, so it cannot pass on one arm alone), but
      the consequence is a real residual: **sites 3, 4 and 5 carry `elapsed_ms` with no test
      asserting it**, and site 3 is precisely the one whose value passes through the
      `u128 as u64` cast. Not closed this round — the ratchet forbids the reviewer widening
      an AC, and widening it now would be tightening after the fact. Recorded as a residual.
- [x] AC1.2 **MET** (`c784714`): fixture with elapsed values 1..=100 + one pre-upgrade line +
      one torn line → exactly p50=50, p99=99, `1 pre-upgrade lines (no elapsed_ms)`,
      `1 unparseable lines skipped`. Neuter: merge either bucket into another → red.
      *Reviewer re-derived the neuter in **both** directions (pre-upgrade→torn and
      torn→pre-upgrade), each RED for the right reason, restored to `0bdc2c87…`. Percentiles
      confirmed exactly nearest-rank on the real binary (p50=50 p90=90 p99=99 max=100 — no
      interpolation, no off-by-one). The load-bearing isolation constraint was verified
      empirically, not just read: a corrupt `memory.jsonl` alongside a healthy
      `memory-stats.jsonl` still reports, while plain `memories` on the same store returns
      nothing.*
      *Note: the **count** asserts carry the neuter; the percentile asserts do not
      discriminate a pre-upgrade→measurable merge (n=101 over [0,1..100] still yields p50=50,
      p99=99) — they exist to pin the exact-value demand, which is what the AC asked for.*
- [x] AC1.3 **MET → ledger row CLOSED** (2026-07-28, release build at `051a9a4`): 10k-record
      `memory.jsonl` (schema per `seed_capped_stale_heavy_corpus`, integration.rs:6196),
      **360** real `agentrec hook` subprocess runs in a throwaway root, 0 nonzero exits.
      Three legs of 120: (A) stale-heavy/capped **p99=16 ms**, (B) fresh-first/injecting
      **p99=13 ms** with 120/120 injected, (C) leg B under 8-way CPU load **p99=22 ms**.
      **Zero `budget_exceeded` across all 360 runs** — the envelope holds on the merits, not
      via the fail-open suppression the criterion would also have accepted. Row transitions
      to CLOSED per Decision 3 (criterion is p99 < 50 ms; worst leg is 22 ms, ~2.3× margin).
      Leg C was added beyond the AC because an unloaded sequential run is the "reasoned safe,
      unobserved" class this repo has repeatedly been burned by; it carries a positive control
      (the load shifted p50 10→16, p99 13→22, so it was not a no-op). Full figure, honest
      bounds, and verbatim reproduction commands in VERIFY-LEDGER.md.
**Expected test outputs:** **430 / 0 / 1** (+2:
`hook_stats_lines_carry_elapsed_ms_on_either_bail_site`,
`memories_stats_three_bucket_summary`).
**Estimate:** 1.5–2 h wall.
**Commit:** `feat(memory): persist per-hook elapsed_ms on all outcome paths + memories --stats; take the 10k measurement`

## Phase 3 — Dedup-hit instrumentation on the daemon snapshot path *(runs second)*
**Description:** `Recorder::stage` (`daemon.rs:935`) does `fs::read` + `put_result`
(`store.rs:61`) per touched file per batch; a dedup hit re-reads the stored object
(`store.rs:72`). Count hits + re-read bytes. **Scope: snapshot puts inside `stage` only** —
prompt/symlink `put_result` calls in `persist` (daemon.rs:1584, 1759) deliberately
uncounted (attribution).
**Files:** `agentrec-core/src/store.rs` (edit), `cli/src/daemon.rs` (edit — counters +
AC3.1/AC3.4 unit tests alongside `stage_*` tests at daemon.rs:2258+), `cli/src/state.rs`
(edit — `field!` pattern, state.rs:175-225). `cli/src/cmds.rs` (mech — two `status_json`
map lines + extend `status_json_carries_degraded_fields` at cmds.rs:1602).
**Changes:**
- `store.rs`: `PutResult::Stored` → `Stored { hash: String, deduped: bool,
  reread_bytes: u64 }` — **breaking change to a `pub` enum, not additive**;
  workspace-internal, no external consumers. Sites, corrected per NB1 — **value-choosing
  construction sites:** store.rs:94 (dedup-hit return — `deduped: true`, `reread_bytes:
  <object len>`; requires restructuring the `fs::read(&path).is_ok_and(...)` at :77, which
  currently drops the bytes) and store.rs:307 (`finish_stored` — `deduped: false,
  reread_bytes: 0`, including the corrupt-fallthrough per Decision 6); **enum definition**
  store.rs:313; **destructuring/assert sites:** store.rs:51 (match arm in `put()`), :386,
  :391, :418, :478, :590 (heal test — asserts the Decision 6 concrete values), :637;
  daemon.rs:989, :1584, :1759. Compile-caught set; the list is the map, rustc is the check.
- `daemon.rs`: `Recorder` gains `dedup_hits`/`dedup_reread_bytes` (u64, `saturating_add`),
  accumulated in `stage`. **Persistence (v1 gate B5, feasibility re-verified at re-gate):**
  `drain_io_failures` — called unconditionally post-flush at daemon.rs:251 and on shutdown
  flush at :326; only its own early-return guard (:1139) is conditional — is renamed
  `drain_recorder_stats`, guard extended to fire when the dedup counters advanced since
  last drain. ≤1 `state.json` write per flush; no per-event writes.
- `state.rs`: two fields, per-field parse, default 0; pre-instrumentation `state.json`
  renders 0, never resets siblings.
**Acceptance criteria:**
- [ ] AC3.1 (unit, daemon.rs tests module, driving `Recorder::stage` directly — no live
      daemon, no hook, so persist-path callers cannot inflate): same content staged twice →
      `dedup_hits == 1`, `dedup_reread_bytes == <len>`; content change → no increment;
      over-cap file → no increment. Neuter: hardcode `deduped: false` at store.rs:94 → red.
- [ ] AC3.2: `status --json` exposes both counters (extension inside
      `status_json_carries_degraded_fields`); missing-field `state.json` renders 0 for both,
      siblings preserved (extend the existing forward-compat test; cite the extension in
      the claim).
- [ ] AC3.3: heal (store.rs:591) and mtime-bump (store.rs:609) assert the same *behavior*;
      the heal test's `assert_eq!` names the Decision 6 concrete values
      (`deduped: false, reread_bytes: 0`) — an executor inventing different values is a
      plan violation, not a judgment call.
- [ ] AC3.4: `Recorder::stage` alone never touches `state.json` (assert the file is absent/
      byte-identical after staging a batch containing dedup hits); it changes only after
      `drain_recorder_stats` runs. Neuter: move the counter persistence into `stage`'s
      per-file loop → red. *(Retargeted per gate NB7 — v3 probed the drain fn itself, which
      is called once per flush by construction, so that probe could not fail.)*
**Expected test outputs:** **432 / 0 / 1** (+2: `dedup_hit_counters_accumulate_in_stage`,
`stage_never_writes_state_json_drain_does`).
**Estimate:** 2–2.5 h wall.
**Commit:** `feat(store): count dedup hits + heal re-read bytes on the snapshot path`

## Phase 2a — `retention::enforce_budget` splits into plan + execute *(pure refactor, mergeable dark; closes B8)*
**Description:** `enforce_budget` (retention.rs:60) computes the keep/evict/protected
partition and hard-deletes in one pass; the `protected_bytes` figure `status` renders
(cmds.rs:443-456) exists only in its return value. Split: `plan_eviction(store, entries,
budget, extra_protected) -> EvictionPlan` (pure — walks, partitions, deletes nothing) and
`enforce_budget(...)` becomes `execute(plan_eviction(...))` — behavior byte-identical.
This is what lets P2b's `status` stay honest read-only.
**Files:** `agentrec-core/src/retention.rs` (edit). `cli/tests/` none — unit tests live in
retention.rs's own tests module alongside the existing ones (retention.rs:247+).
**Changes:**
- `EvictionPlan { victims: Vec<(hash, bytes)>, protected_bytes, freed_bytes_projected, … }`
  — exact fields implementer's call, but `protected_bytes` semantics must be identical to
  today's accumulation at retention.rs:129-142. **The A3(c) freshness guard
  (retention.rs:150-152) lives in BOTH halves (closes B10's root cause):** in `plan` (a
  dry-run that lists a fresh blob as victim is a wrong report) AND re-checked by `execute`
  immediately before each `store.remove` — today's guard is evaluated at delete time
  (:150-153), and a plan-only guard would widen that window across the plan→execute gap
  (a dedup mtime-bump landing between them would go unseen). Execute's re-check preserves
  today's delete-time semantics exactly. **The reference timestamp is the plan's (B11):**
  `EvictionPlan` carries the `pass_start` captured by `plan_eviction` (today's
  retention.rs:70), and `execute` re-checks each victim's mtime against **that carried
  value, never a fresh `SystemTime::now()`** — a fresh-now re-check would let a blob
  touched between plan and execute-start be deleted, i.e. *less* protection than the
  unsplit code, in a hard-delete path. Inside `plan`, the protect-retain accumulation
  (today's :129-142) runs **before** the freshness guard (:150) — today's order — so a
  candidate that is both externally protected and fresh is counted in `protected_bytes`
  exactly once; reordering silently shrinks the figure `status` renders (AC2a.2 neuter ii).
- `execute` accumulates `Evicted.bytes` from `store.remove`'s **return** (today's
  retention.rs:153-156 behavior — a blob already gone contributes nothing), never from the
  plan's projected victim sizes (NB6: summing the plan over-reports on any blob that
  vanished between plan and execute).
- Public behavior of `enforce_budget` unchanged: same deletions, same `Evicted` return.
  Existing retention tests (retention.rs:247-520) pass **unmodified** — this is the
  refactor's whole discriminator.
**Acceptance criteria:**
- [ ] AC2a.1: every existing retention test passes with zero edits — pure regression
      criterion. *(B12 correction: `enforce_budget_skips_a_candidate_touched_after_pass_
      start` (retention.rs:444-480) pins the guard **pair** via the delete outcome only —
      under the both-halves design, execute's re-check masks a deleted plan-side guard from
      every deletion-observable assertion, so this test canNOT red a plan-only neuter. The
      plan-side guard's own discriminator lives in AC2a.2.)*
- [ ] AC2a.2 (new unit — carries the plan-side guard's discriminator, per B12): fixture
      where `enforce_budget` would evict V and protect P, **plus** a boundary candidate F
      whose mtime is future-dated past `pass_start` (future-dating is correct on THIS leg —
      the plan *report* is under test, not the delete-time re-check), **plus** one candidate
      (D) that is both `extra_protected` and mtime-fresh; V is **backdated** (`now − 3600s`,
      mirroring retention.rs:511-517) so no timestamp coincidence can rescue it. Asserts:
      `plan.victims == {V}`; `plan.protected_bytes == size(P) + size(D)`;
      `freed_bytes_projected == size(V)`; `plan.victims` excludes F and
      `freed_bytes_projected` excludes F's bytes; D is counted in `protected_bytes` exactly once
      (the protect-retain at today's retention.rs:129-142 runs **before** the freshness
      guard in `plan`, same order as today's :129→:150 — reordering silently shrinks the
      figure `status` renders); store byte-identical after planning; then `execute` of
      that plan deletes exactly V. Neuters, each must red this test: (i) drop the
      plan-side freshness guard → F appears in victims; (ii) reorder the guard ahead of
      protect-retain → `protected_bytes` shrinks; (iii) make `plan_eviction` delete →
      byte-identical assert reds.
- [ ] AC2a.3 (new unit — the deterministic replacement for v3's vacuous AC2b.4): an old,
      boundary-evictable turn references blob X (plus a newer turn, so A5 newest-turn
      protection cannot mask the result); `plan_eviction` lists X as victim; the test then
      bumps X's mtime via a **real-now dedup-put of identical bytes** (store.rs:83-84 sets
      `set_modified(SystemTime::now())`) — **explicitly NOT a future-dated filetime bump**:
      the nearest existing fixture (`retention.rs:464`, `now + 3600s`) satisfies
      `mtime > t` for *any* plausible reference `t` and therefore cannot discriminate B11's
      two designs; copying it would make this test vacuous (B11). Then `execute(plan)` →
      **X survives** and `Evicted.bytes` excludes it. Two neuters, both must red this test:
      (i) delete execute's mtime re-check; (ii) make execute re-check against a fresh
      `now()` instead of the plan's carried `pass_start`. Fully deterministic — the test
      controls plan/bump/execute ordering directly; no live daemon. **Granularity guard
      (NB14):** a ≥20ms sleep between `plan_eviction` and the dedup-put (the store.rs:605
      precedent — `dedup_hit_touches_mtime` needs the same gap for the same strict-`>`
      comparison), plus a self-guarding precondition assert
      `store.mtime(&X).unwrap() > plan.pass_start` **before** calling `execute` — so a
      filesystem-granularity failure announces itself as a precondition, never masquerades
      as a guard failure. (False-RED protection only; both neuters still red regardless.)
**Expected test outputs:** **434 / 0 / 1** (+2:
`plan_reports_without_deleting_then_execute_deletes_exactly_plan`,
`execute_recheck_spares_blob_touched_after_plan`).
**Estimate:** 1.5–2 h wall.
**Commit:** `refactor(retention): split enforce_budget into plan_eviction + execute, behavior-identical`

## Phase 2b — Eviction leaves the read path *(Q1 answered (a) — Decision 7; runs after P2a, which it depends on)*
**Description:** `status_report` is `enforce_budget`'s sole production caller (guard
cmds.rs:419, harvest :427, call :428-433). Under (a): eviction moves onto a daemon tick
alongside `maybe_rebuild` (daemon.rs:193), interval 10 min + one pass at startup **after**
`recover_orphan` (daemon.rs:1706) — recovery first, so the recovered turn is a real
`log.jsonl` reference before any eviction walk (NB4). `status` text path calls
`plan_eviction` (P2a) read-only: over-budget notice, orphan-attribution, and the
protected-bytes honesty line all keep rendering — from the plan, with wording shifted from
"freed" to what the daemon will do (`would free N B; M protected (pinned or in-flight —
never evicted)`), plus `daemon not running — nothing is evicting` when
`!daemon_is_running`.
**Files:** `cli/src/daemon.rs` (edit — tick + startup pass + stderr line),
`cli/src/cmds.rs` (edit — status rewire onto `plan_eviction`; `extra_protected_refs` :555
and `effective_store_budget` :663 become `pub(crate)` or move — implementer names which),
`cli/tests/integration.rs` (edit — tests below, incl. a `spawn_record` variant capturing
stderr to a file: today's helper nulls it, integration.rs:28-35, and no test asserts daemon
stderr — the "precedent" is state/log-asserted, so this phase builds the stderr capture it
needs (NB3)). README.md (docs).
**Changes:**
- `daemon.rs`: every `EVICT_INTERVAL` (10 min prod; `#[cfg(debug_assertions)]` env seam)
  and once at startup post-recovery: `load_log` → harvest `extra_protected_refs`
  immediately before → `plan_eviction` → execute; budget from `effective_store_budget`
  (reuses existing `AGENTREC_TEST_STORE_BUDGET_BYTES` seam, cmds.rs:658, already driven by
  integration.rs:8864 — no second seam). One stderr line per pass that evicted ≥1 blob.
  **No state.json counters this phase** — stderr + the status dry-run report are the
  observables; a persistent eviction-history counter is recorded residual (P3 establishes
  the pattern if wanted later). **Ledger ride-along:** once P2b lands, time the tick's
  `load_log` + `plan_eviction` on the live dogfood store and record it next to P1's figure
  — the 10ms/2006-turn number in the rejected list was measured for a different purpose
  and is not evidence about a store where eviction actually fires.
- `cmds.rs`: text `status` stops executing; renders from `plan_eviction`.
- **Named test dispositions (complete set — the re-gate swept every small-budget
  `status_report` call at cmds.rs:1030/1059/1937 and all of `cli/tests/`; the orphan-clause
  test at :1059 renders from `orphan_bytes`, computed independently of eviction, and
  genuinely needs no disposition):**
  1. `status_prints_over_budget_notice` (cmds.rs:1030-1039): the two notice asserts survive
     with their expected substring updated to the new dry-run wording (`snapshot eviction
     freed` → `would free`; a substring update is not a weakening — NB9); the third assert
     `!store.contains(&hash)` is a **deletion** assert → **inverted** under (a).
  2. `status_attributes_protected_bytes` (cmds.rs:1915): both honesty asserts survive in
     substance against the new wording ("would free 0 B" / `protected` + pinned/in-flight
     clause); the figure now comes from `plan_eviction` — same accumulation semantics by
     AC2a.1.
  3. `status_eviction_keeps_open_turn_blob` (integration.rs:8793): retargeted at the daemon
     pass and renamed `daemon_eviction_keeps_protected_refs`. **Fixture changes (B9):** the
     protect-channels seeded are an **unparseable-but-newline-terminated `log.jsonl` line**
     citing the old blob — newline-terminated so the daemon's boot epoch append lands on a
     fresh line instead of concatenating onto it (NB8: `open_append` is O_APPEND with no
     leading newline; `harvest_refs` is a raw byte scan and survives either way, purgecmd.rs:
     885-900, but the fixture must exist in the shape the test claims) — and a
     **`memory.jsonl` pin**. Both are channels the daemon only appends to — NOT `open.json`,
     which `sync_journal`'s idle arm deletes every tick (daemon.rs:1692-1697). Same survival
     asserts (:8874-8878 shape). The `open.json` protect-channel keeps its coverage at unit
     level: a direct test of harvest→plan with a seeded `open.json` file and no daemon
     (`extra_protected_refs` is a side-effect-free *reader* — three `fs::read_to_string`
     calls, cmds.rs:558-568 — so the unit test seeds real files on disk; it cannot be fed
     in-memory inputs — NB10).
- **Interleave posture, stated for the gate to verify at the call site, not in prose:** the
  tick's `load_log` → harvest → `plan_eviction` → `execute` is one unbroken statement
  sequence in the single-threaded run loop — no event drain between them — and cross-window
  staleness is covered by execute's mtime re-check (AC2a.3). The honesty-round's
  "narrowed not closed" `undo`-race residual carries over unchanged.
**Acceptance criteria (Q1 = (a)):**
- [ ] AC2b.1: text `status` on an over-budget store leaves `.agentrec/objects/` byte-,
      mtime-, count-identical, while still printing the over-budget notice AND the
      protected-bytes clause. (Objects-tree scope only — the store-mutation claim, not
      "status writes nothing anywhere".) Neuter: swap `plan_eviction` back to
      `enforce_budget` in `status_report` → red.
- [ ] AC2b.2: live daemon (seam interval ~2s, seam budget) on an over-budget store evicts
      within 2× interval: oldest unprotected blob gone + one stderr eviction line (captured
      via the new stderr-file spawn variant); torn-line ref and pin blob survive. Neuter:
      delete the tick call → red.
- [ ] AC2b.3: daemon-down + over-budget → `status` prints `nothing is evicting`, deletes
      nothing. (Accepted residual, stated in-product: CLI-only writers — `undo` — can grow
      a store no daemon shrinks.)
- [ ] AC2b.4: full suite green with exactly the three dispositions above; no other test
      deleted or weakened; unit-level `open.json` harvest test present. *(v3's AC2b.4 —
      "freshly-staged blob survives the tick" — is deleted as vacuous per gate B10: a
      staged blob is not in `log.jsonl` until the turn closes, so it is never a candidate
      and the assert cannot fail. Its real content — the plan→execute staleness window —
      is pinned deterministically by AC2a.3 instead.)*
**Expected test outputs:** **437 / 0 / 1** (+3: `status_performs_zero_store_writes`,
`status_reports_no_evictor_when_daemon_down`,
`harvest_protects_open_json_refs_at_plan_level`; the retarget renames but adds no name).
**Estimate:** 3–4 h wall (stderr-capture helper + timing-coupled daemon test are the slow
parts).
**Commit:** `fix(retention): evict from the daemon tick, not from a read verb`

---

## Edge cases considered
- **P1:** torn/concurrent stats appends (O_APPEND line-atomic; three-bucket summarizer never
  fails); empty/absent file; pre-upgrade lines; `Instant` only; huge file (linear fold).
- **P3:** pre-existing `state.json` (AC3.2); over-cap/io_failed never increment (AC3.1);
  persist-path puts out of scope; corrupt-fallthrough semantics fixed by Decision 6;
  `saturating_add`; write cadence bounded (AC3.4).
- **P2a:** plan/execute drift (AC2a.1/2a.2); freshness guard in **both halves** with the
  plan's carried `pass_start` as the sole reference timestamp (AC2a.3, B10/B11);
  protect-retain-before-guard ordering inside `plan` (AC2a.2 neuter ii).
- **P2b:** daemon killed mid-eviction (per-blob `remove_file`, same crash posture as
  today); eviction vs `undo` race (late harvest narrows-not-closes — honesty-round
  residual, documented); seams absent from release; daemon-down growth via `undo`
  (AC2b.3); tick-vs-flush interleave (pinned by AC2a.3 plus the Interleave-posture bullet —
  single-threaded loop, unbroken harvest→plan→execute sequence); startup ordering after
  `recover_orphan` (NB4).

## Open questions
None open. Q1 answered (a) and Q2 answered default by the founder, 2026-07-27 — recorded as
Decisions 7 and 8. All phases are unblocked once the plan-level skeptic gate passes.

## Pre-round housekeeping (founder-run — blocked for agents by the git-guardrails hook)
Stale local branches from the four merged/squashed rounds (remote copies already deleted
2026-07-27; content verified in `main` via PR state before deletion was proposed). The
`-D` is required because squash-merge breaks `--merged` ancestry — `-d` refuses:

```bash
git branch -D fix/residuals-round fix/honesty-round fix/blame-attribution-and-noise-folding fix/ignore-rebuild-gate fix/gitignore-self-match fix/review-findings-043c749 feat/memory-v1 fix/purge-orphans-gc gate-review-orphans claude/magical-mendeleev-723a61 claude/strange-gagarin-bbd4af claude/xenodochial-matsumoto-10ff1b worktree-fix-doctor-inotify
```

Keep: `main`, `old-history-local` (pre-squash history — never push, never delete).
Not required for any phase to start; the round's worktree cuts from `main` regardless.

## Time summary
| Phase | Wall estimate |
|---|---|
| P1 recall evidence + 10k run | 1.5–2 h |
| P3 dedup counters | 2–2.5 h |
| P2a retention plan/execute split | 1.5–2 h |
| P2b eviction relocation [Q1=(a)] | 3–4 h |
| Final skeptic gate + bookkeeping | ~1 h |
| **Total** | **~9.5–11.5 h wall (1.5 working days); no open gates — Q1/Q2 answered (Decisions 7–8)** |
