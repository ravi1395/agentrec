# Plan: residuals round — moved-in-dir loss, stale reload line, watcher-arm race, CI Linux leg  (5 phases, branch/worktree: `fix/residuals-round`, cut from `fix/honesty-round`)

**Goal + architecture (≤5 lines):** Close the five residuals the nonce/Linux-leg round recorded.
Headline: on macOS a directory moved INTO the watched root loses its contents silently (3/3 at
HEAD) — the sharpest known hole in the core "who broke my repo" claim, on the majority platform.
The admission machinery (`admit_existing_contents`, `daemon.rs:706`) already exists but is
`#[cfg(target_os = "linux")]`; the question is whether FSEvents rename kinds are trustworthy
enough to drive it on macOS without reopening the fabrication class `f4bca8a` closed.

**Baseline (UNVERIFIED — re-run before trusting):** macOS 422 passed / 0 failed / 1 ignored,
Linux 426 / 0 / 2 ignored, at `d78d328`. Clippy `-D warnings` + fmt clean both profiles.

## Decisions log (user-confirmed)

1. (2026-07-26) This plan is gated by a binding **fable skeptical-reviewer**; plan is final only
   at skeptic GATE PASS. Per-phase implementation gates remain skeptic-bound as house rule.
2. Carried from prior rounds, executors may not re-litigate: admission stays compiled out on
   macOS for **Create-kind** events (FSEvents coalesced flag unions make `Create(Folder)`
   untrustworthy — measured 2/2, `f4bca8a`). Any macOS admission this plan adds must be gated on
   rename kinds ONLY, and only if Phase 1's measurement says those kinds are clean.
3. Carried: never loosen an AC to pass; direction of failure must be RED, not silent-green.

## Infeasible/rejected, with the code reason

- **macOS admission gated on `Create(_)`** — killed at `f4bca8a`: FSEvents delivers coalesced
  per-path flag unions; first event on a pre-existing dir routinely carries historical
  `ItemCreated`. Measured fabricating 2/2. Do not resurrect.
- **"Smarter" FSEvents flag interpretation (raw flag inspection below notify's EventKind)** —
  fighting the platform; notify already collapses the union, and the union itself is ambiguous
  at the FSEvents layer. Rejected in `f4bca8a`'s review.
- **`elapsed/POLL`-derived rebuild bound** — `recv_timeout` returns immediately when a message
  is queued; derivation invalid (measured, prior round).
- **Fixing the watcher-arm race by moving `acquire_lock` after watcher creation** — the flock IS
  the single-daemon gate (`daemon.rs:59`); arming a watcher before holding the lock lets two
  daemons watch simultaneously during a race window. The pid's meaning ("lock held") is correct;
  the test's interpretation ("daemon ready") is what must change. Add a separate armed signal.

## Global constraints + invariants

- Under-record is the tolerated failure direction; **fabricated attribution is never tolerated**
  (`blame` naming the agent for untouched files is the product's headline surface).
- `state.json` fields are additive; readers tolerate missing fields (`#[serde(default)]`,
  per-field parse via the Phase-2 honesty-round machinery in `state.rs:read_state`).
- `log.jsonl`/`signal.jsonl` append-only. No new deletion paths.
- Every AC names the neuter that must turn a **named** test RED. Criteria unprovable by fixture
  are marked → live-daemon leg or VERIFY-LEDGER row, never a PASS.
- Both `status` text and `status --json` are independent readers — any render-gate change must
  pin BOTH seams (lesson: `0f3b474`, the json-only-neuter hole).
- Platform claims are measured per-platform or `cfg`-split; no cross-platform intuition
  (the round's central lesson).

## Executor protocol

Sub-skill: `/surgeon` per chunked task, TDD RED-first, one commit per task, commit messages given
per phase. Merge order: P1 → P2 (hard dependency: P2 is gated on P1's measurement); P3, P4
independent, any order after P1 starts; P5 last (its VERIFY row needs P4's de-flaked harness to
mean anything on CI). Multi-commit → run in a dedicated worktree on `fix/residuals-round`.
Orchestrator re-runs the full suite (macOS + Colima Linux container, non-root, fixtures in
container-native `/tmp`) after every phase, independent of every implementer.

---

## Phase 1 — Spike: measure FSEvents rename-kind trustworthiness (the scariest unknown)
**Description:** Every viable macOS fix hangs on one unmeasured fact: does FSEvents deliver
`Modify(Name(_))` for directories *only* on genuine renames, or does it coalesce/replay rename
flags onto unrelated first-events the way it does `ItemCreated`? Prior-round intuition about
FSEvents was wrong every time it went unmeasured; this phase buys the fact before any code.
**Files:** `docs/superpowers/specs/2026-07-26-fsevents-rename-measurements.md` (new). Probe
harness lives in the session scratchpad (isolated repro against the raw `notify` crate, same
method as `034c883`'s repro) — deliberately NOT committed to the repo.
**Changes:**
- Probe A (rename-in): `mv <outside-dir-with-3-files> <root>/dir` — record full EventKind
  sequence for the dir path. ≥10 trials.
- Probe B (fabrication trap): on a repo whose directories all predate the watcher: `touch dir`,
  `chmod dir`, xattr change, first file-write inside dir — record the FULL per-delivery
  EventKind sequence for the dir path (notify's FSEvents backend can emit multiple EventKinds
  from one coalesced flag union — that is exactly how historical `ItemCreated` surfaced; a
  first-kind-wins recording would miss the poison). Any `Modify(Name(_))` anywhere in any
  sequence counts as dirty. ≥10 trials per stimulus, ≥2 distinct repo fixtures (fresh tempdir
  + aged real-repo copy), since coalescence keys on per-path history.
- Probe C (replay): after a genuine rename-in is recorded, wait ≥60s with unrelated activity;
  record whether the dir receives a delayed spurious `Modify(Name(_))` redelivery (the analogue
  of the 3/5 `Create(Folder)` replay measured at `247df9e`). ≥5 trials.
- Probe D (rename-out): `mv <root>/dir <outside>` — confirm from-side kind and that the path no
  longer resolves (`is_dir()` false) at delivery time. ≥5 trials.
- Measurement doc records: raw counts per probe, exact EventKinds observed, verdict.
**Interface contract (consumed by Phase 2):** the doc's final section is a machine-checkable
verdict line, one of: `VERDICT: CLEAN` (Probe B: 0 spurious rename-kinds across all trials AND
Probe C: 0 replays), `VERDICT: REPLAY_ONLY` (B clean, C dirty), `VERDICT: POISONED` (B dirty).
Phase 2's shape branches on exactly this token.
**Acceptance criteria:**
- [ ] Probe A: ≥10/10 rename-in trials deliver at least one `Modify(Name(_))` event for the
      moved dir's destination path. (If this fails, rename-gated admission is impossible and
      Phase 2 collapses to its POISONED arm regardless of B/C.)
- [ ] Probe B executed on ≥2 fixtures × ≥4 stimuli × ≥10 trials each, raw counts in the doc —
      no "reasoned safe" rows; every cell is an observation.
- [ ] Probe C executed ≥5 trials with the ≥60s window; counts recorded.
- [ ] Doc carries the single `VERDICT:` token and the probe harness source inline (so the
      measurement is re-runnable without the scratchpad).
**Expected test outputs:** none (no repo code changes). `cargo test --workspace` unchanged from
baseline. Verification = the doc exists, carries raw counts, and the verdict token parses.
**Commit:** `docs: measure FSEvents rename-kind semantics for macOS admission (spike)`

## Phase 2 — macOS rename-in admission [gated on P1 verdict]
**Description:** Close the moved-in-directory silent loss on macOS, in whichever shape P1's
verdict permits. The Linux side already handles rename-in via the `Modify(Name(_))` arm of its
gate (`daemon.rs:573-589`); this phase extends the *action* to macOS behind a rename-only gate —
or, on a POISONED verdict, documents the founder-visible dead end honestly instead of shipping
a fabricator.
**Files:** `cli/src/daemon.rs` (edit), `cli/tests/integration.rs` (edit)
**Changes (per P1 verdict):**
- If `VERDICT: CLEAN`: remove the `#[cfg(target_os = "linux")]` from `admit_existing_contents`
  (`daemon.rs:705`) and from the admission block; split the gate:
  Linux keeps `Create(_) | Modify(Name(_))` exactly as at `daemon.rs:573-589`; macOS admits on
  `Modify(Name(_))` ONLY (never `Create(_)` — Decisions log #2). `admitted_dirs` populated on
  both platforms; existing clears (`:465-467`, `:486-488`) already unconditional.
  The existing admission unit tests (`daemon.rs:2917-3210` region) STAY `#[cfg(target_os =
  "linux")]` — they pin the Linux gate, and e.g. the dedup test's two synthetic
  `Create(Folder)` events would fail on macOS's rename-only gate by design. Add a new macOS
  counterpart unit test (`#[cfg(not(target_os = "linux"))]`)
  `create_kind_does_not_admit_on_macos`: synthetic `Create(Folder)` for a real, populated
  directory → nothing staged, `admitted_dirs` empty — the direct pin that Decisions log #2
  survived the un-cfg. Harness pattern: the existing synthetic-event channel driving
  `drain_watch_events` (`daemon.rs:2956-2978`).
- If `VERDICT: REPLAY_ONLY`: same as CLEAN — `admitted_dirs` exists precisely to absorb delayed
  replays — plus a comment citing P1's Probe C counts as the load-bearing reason the set is now
  populated on macOS.
- If `VERDICT: POISONED`: no daemon change. Update the module comment (`daemon.rs:106-124`) from
  "founder call" to "measured dead end, see measurement doc", and add the scan-diff design as a
  named open question to the founder (it requires a baseline-hash index the daemon does not
  have — a design round, not a task).
- Either non-POISONED arm: new integration test
  `directory_moved_into_root_records_contents` — build a dir with 2 files OUTSIDE the root,
  `mv` it in, wait past debounce + quiet window, assert both files appear in `log.jsonl` with
  `op:"create"`. Compiled on both platforms (Linux already passes via its existing arm — the
  test pins that too). Existing macOS fabrication guard
  `metadata_only_event_on_directory_that_predates_daemon_stages_nothing` must stay green
  untouched — it is the other direction of this same gate.
**Acceptance criteria (non-POISONED arms):**
- [ ] `directory_moved_into_root_records_contents` passes on macOS (was: contents lost 3/3) and
      on Linux. Neuter: re-add the `cfg` to the macOS admission arm → this test REDs on macOS
      by name.
- [ ] `metadata_only_event_on_directory_that_predates_daemon_stages_nothing` still green on
      macOS. Neuter: widen the macOS gate to include `Create(_)` → this test REDs. (This is the
      discriminator proving the split gate, not the old blanket cfg, is what protects macOS.)
- [ ] Rename-out does not walk AND does not poison: pinned by a unit test injecting a synthetic
      `Modify(Name(From))` for a nonexistent path (harness: the synthetic-event channel at
      `daemon.rs:2956-2978`), asserting BOTH nothing staged AND `admitted_dirs.is_empty()`
      (assertion pattern: `daemon.rs:2992-2995`), then following with a second synthetic event
      at the same now-real path whose contents MUST stage — the follow-up leg is
      platform-split: `Create(Folder)` under `#[cfg(target_os = "linux")]`,
      `Modify(Name(RenameMode::To))` under `#[cfg(not(target_os = "linux"))]` (macOS's gate
      excludes `Create(_)` by design — a Create follow-up there would contradict
      `create_kind_does_not_admit_on_macos`). Neuter:
      drop the `is_dir()` conjunct → the nonexistent path is inserted into `admitted_dirs`
      (the walk itself yields nothing — `.flatten()` at `daemon.rs:714-719` swallows the walk
      error, so "nothing staged" alone CANNOT discriminate) → test REDs on the `is_empty`
      assertion, and the follow-up admission is suppressed (`insert` returns false) → REDs the
      staging assertion too. Two independent discriminators.
- [ ] Live escalation trial (NOT unit-testable — FSEvents timing): 5/5 bracket trials of
      `touch <pre-existing dir>` produce zero fabricated entries, AND 5/5 trials of
      `mv dir-in` record all contents. → VERIFY-LEDGER row + skeptic live leg, same protocol as
      `f4bca8a`'s 5/5. A single clean run is NOT acceptance (the round's ~50%-behavior lesson).
**Acceptance criteria (POISONED arm):**
- [ ] Module comment updated with measurement citation; no daemon behavior change;
      `cargo test --workspace` unchanged from baseline; open question filed in this plan's
      tracker section for the founder.
**Expected test outputs (non-POISONED):** `cargo test --test integration
directory_moved_into_root_records_contents` → integration binary reports `1 passed` (filtered).
Full suite: 0 failed on both platforms; per-platform deltas derived at execution against the
re-run baseline and recorded in the round's Status entry — NOT predicted here (the ladder-error
class this repo has hit three recorded times; new tests are named above, counts are not
promised).
**Commit:** `fix: admit moved-in directory contents on macOS — rename kinds measured clean` (or
`docs: macOS moved-in-dir admission is a measured dead end` for POISONED).

## Phase 3 — Stale reload line after a crashed daemon: liveness-gate both status readers
**Description:** `release_lock` never runs on kill-9, so `epoch_nonce` stays stamped and
`current_epoch_reloads` (`state.rs:317`, gate at `:318`) renders a dead epoch's count as
current — in both `status_report` (`cmds.rs:309-310`) and `status_json` (`cmds.rs:233`).
Neither reader checks liveness; the flock probe already exists (`daemon_is_running`,
`daemon.rs:1920`, `pub(crate)`, consumers today: `doctorcmd.rs:172` and `purgecmd.rs:244`,
`:520`, `:795` — so `status` joining them is the fourth caller of proven machinery, not a new
pattern).
**Files:** `cli/src/cmds.rs` (edit), `cli/tests/integration.rs` (edit), `cli/src/state.rs`
(edit — doc comment on `current_epoch_reloads` only)
**Changes:**
- In the status path, compute `let daemon_live = crate::daemon::daemon_is_running(root);` once;
  gate BOTH the text line and the json epoch field on it. Dead daemon → text line suppressed,
  json epoch field reports per Q3's answer. Lifetime counter (`ignore_rebuilds`) untouched —
  it is honestly lifetime-scoped.
- `current_epoch_reloads` doc comment gains the caller obligation ("callers must additionally
  gate on liveness; this function cannot — it has no root").
- New integration test `status_suppresses_reload_line_after_daemon_crash`: start daemon, force
  ≥1 rebuild (`.gitignore` churn), confirm line renders while live, `kill -9`, run `status` +
  `status --json`, assert the text line absent AND the json epoch field per Q3. Both seams in
  ONE test (0f3b474 lesson).
- **Fixture migration (load-bearing — the gate is otherwise unsatisfiable):** FOUR existing
  cmds.rs unit tests seed `state.json` in a bare tempdir with NO flock held and assert the
  reload line RENDERS — under the liveness gate they all red immediately, before any neuter:
  `status_omits_reload_line_when_never_reloaded` (presence half, `cmds.rs:1225`),
  `status_reload_line_is_epoch_scoped` (`:1274`),
  `status_omits_stale_epoch_reload_line` (live-epoch sub-case, `:1371`), and
  `pid_reuse_does_not_resurrect_a_dead_epoch` (`:1426`). This roster is exhaustive by grep of
  `contains("reload"` / `contains("ignore:"` over cmds.rs tests — notably
  `epoch_detection_is_symmetric_regardless_of_whether_pid_also_changed` (`cmds.rs:1446-1485`)
  is NOT in it: it asserts absence + output-equality only, stays green under the gate with no
  flock, and needs no migration (do not "migrate" it — a hold-the-lock edit there would make
  its Neuter-4 run produce a spurious vacuity verdict, since it cannot red on presence).
  No integration test asserts presence (absence-only at `integration.rs:1216`) — cmds.rs is
  the whole migration surface. Add a test helper
  `hold_daemon_lock(root) -> std::fs::File` in the cmds.rs test module that creates
  `.agentrec/daemon.lock` and takes a REAL exclusive flock on it (raw `libc::flock`;
  `acquire_lock` itself is private to daemon.rs and also mutates state — the helper must hold
  the lock without touching `state.json`). Each of the four holds the guard across its
  presence assertions; dropping the guard is itself exploited where a test wants the absence
  direction. Real flock, deliberately NOT an injection seam — this repo's test-seam history
  (release-`strings` audits) is why. The helper's premise is already proven in-process:
  `daemon_is_running_true_while_held_false_after_release` (`daemon.rs:2573-2586`) shows a
  same-process flock on another fd defeats the probe's `LOCK_EX|LOCK_NB`.
**Acceptance criteria:**
- [ ] `status_suppresses_reload_line_after_daemon_crash` passes. Neuter 1: gate only the text
      seam → test REDs on its json assertion. Neuter 2: gate only the json seam → test REDs on
      its text assertion. Both neuters exercised at review.
- [ ] Liveness-gated presence: the FOUR migrated fixtures (roster above), each now holding the
      real flock via `hold_daemon_lock`, all green with their ORIGINAL presence assertions
      unweakened. Neuter 3: unconditionally suppress the line → all four RED. Neuter 4: remove
      `hold_daemon_lock` from any one of the FOUR (never the symmetric test — it cannot red on
      presence) → that test REDs (proving the gate is real, not vacuously true) — exercised on
      at least one of the four at review.
- [ ] Clean-stop behavior unchanged (nonce already cleared by `release_lock` — this phase must
      not double-report): existing `acquire_lock_stamps_a_fresh_epoch_nonce_and_release_clears_it`
      (`daemon.rs:2526`) green.
- [ ] The flock probe adds no daemon interference: `status` against a LIVE daemon returns the
      same output before/after this change (probe is non-blocking and releases immediately —
      same call doctor already makes). Pinned by the existing live-daemon status tests.
**Expected test outputs:** `cargo test --test integration
status_suppresses_reload_line_after_daemon_crash` → `1 passed`. Full suite baseline+1 per
platform, 0 failed.
**Commit:** `fix: status reload line requires a live daemon — crash left the dead epoch rendering as current`

## Phase 4 — Watcher-armed signal: close the wait_for_live_daemon race
**Description:** `wait_for_live_daemon` (`integration.rs:4613-4621`) polls `state.json` for
`pid != 0`, but the pid is written by `acquire_lock` (`daemon.rs:59`) ~4.3ms before the watcher
is armed (`daemon.rs:81-87`) — every live-daemon test nominally races the watcher. Moving the
lock after the watcher is rejected (see Infeasible list); instead the daemon writes an explicit
armed signal after `.watch()` succeeds, and the test helper waits for it.
**Files:** `cli/src/state.rs` (edit), `cli/src/daemon.rs` (edit), `cli/tests/integration.rs` (edit)
**Changes:**
- `State` gains `watcher_armed_nonce: String` (`#[serde(default)]`, additive, wired through the
  per-field `read_state` parse like every other field). Empty = not armed / unknown.
- **Interface contract (the unit-testable seam):** extracted helper in daemon.rs,
  `fn stamp_watcher_armed(root: &Path)` — reads state, sets
  `watcher_armed_nonce = state.epoch_nonce` (whatever it currently is), writes state. `run()`
  calls it immediately after `.watch(&root, RecursiveMode::Recursive)` returns Ok
  (`daemon.rs:86`). Same extraction shape as `acquire_lock`/`release_lock` — which is exactly
  why THOSE are unit-testable today (`daemon.rs:2526`) and an inline stamp would not be.
  Keying on the epoch nonce (not a bool) makes a stale value from a crashed prior run
  self-invalidating: a fresh epoch has a fresh nonce, so `watcher_armed_nonce == epoch_nonce`
  is false until THIS epoch's watcher armed. `release_lock` clears it alongside `epoch_nonce`.
- `wait_for_live_daemon` waits for `pid != 0 && !epoch_nonce.is_empty() &&
  watcher_armed_nonce == epoch_nonce` (same 10s bound).
**Acceptance criteria:**
- [ ] New unit test `watcher_arm_stamp_keys_on_current_epoch_nonce` calling the REAL
      `stamp_watcher_armed` (not a simulation): seed a state carrying a PREVIOUS epoch's armed
      nonce, `acquire_lock` (stamps a fresh epoch nonce), assert
      `watcher_armed_nonce != epoch_nonce` (not yet armed), call `stamp_watcher_armed(root)`,
      assert equality. Neuter: make the helper stamp a constant instead of the nonce → REDs the
      equality assertion. Neuter 2: make it a no-op → REDs (armed nonce still the stale one).
      The call-site itself (`run()` invoking the helper) is NOT reachable from a unit test —
      that deletion is covered by AC2's integration test, stated explicitly so no one claims
      this unit test covers it.
- [ ] New integration test `live_daemon_reports_watcher_armed`: spawn real daemon, assert
      state.json reaches `watcher_armed_nonce == epoch_nonce` within the wait bound. Neuter:
      delete the post-watch stamp → this test REDs (and every wait_for_live_daemon caller
      times out — loud, not silent).
- [ ] `release_lock` clears the armed nonce: extend
      `acquire_lock_stamps_a_fresh_epoch_nonce_and_release_clears_it` with the new field.
      Neuter: drop the clear → extended assertion REDs.
- [ ] The race-closure itself (no live-daemon test can observe an event emitted in the 4.3ms
      window anymore) is NOT directly assertable — mark honestly: the mechanism tests above are
      the proxy; the population-level claim ("flake class removed") goes to a VERIFY-LEDGER
      row measured over CI history, not a green checkbox now.
**Expected test outputs:** `cargo test -p agentrec --bin agentrec watcher_arm_stamp` → bin
target reports `1 passed` (filtered — the `agentrec` crate is bin-only, no lib target, so
`--lib` would error `no library targets found`; `--bin agentrec` scopes to the target the unit
tests live in; an unscoped `watcher_arm` filter also matches the integration test and reports
across both binaries);
`cargo test --test integration live_daemon_reports_watcher_armed` → integration binary reports
`1 passed`. Full suite 0 failed; deltas recorded at execution.
**Commit:** `fix: daemon stamps watcher-armed nonce; tests wait for armed, not just pid`

## Phase 5 — CI Linux leg + bound re-measurement [gated on Q1]
**Description:** The admission mechanism is now testable ONLY on Linux, so the Linux leg is
load-bearing infrastructure, currently a manual Colima ritual documented in prose. Give CI a
way to run it on branch work, and use the first real runner to close the single-VM statistics
residual on the rebuild bound (`rebuild_count_is_bounded_by_writes`, `integration.rs:1139`,
Linux `1..=45` / macOS `1..=10`).
**Files:** `.github/workflows/ci.yml` (edit), `VERIFY-LEDGER.md` (edit),
`cli/tests/integration.rs` (edit — one `eprintln!` in `rebuild_count_is_bounded_by_writes`)
**Changes:**
- Trigger edit per Q1's answer (options below — not chosen here). Note the GitHub constraint
  either way: `workflow_dispatch` is only offered for workflows already on the default branch,
  so a dispatch-only answer implies landing the trigger change via main first.
- VERIFY-LEDGER gains two rows: (a) "Linux leg green on a real GitHub runner for this branch's
  HEAD" — closed by a run URL, not a local container transcript; (b) "rebuild bound observed on
  ≥2 distinct Linux environments (Colima VM + GH runner): correct-code counts inside
  `1..=45`, neutered counts outside" — closed with the observed numbers written into the
  ledger row.
**Acceptance criteria:**
- [ ] A branch-push or dispatch run of `ci.yml` executes the full test job on ubuntu — evidenced
      by a run URL with the `test` job green on the ubuntu matrix legs. (Not falsifiable by a
      local test; the criterion IS the run URL.)
- [ ] `rebuild_count_is_bounded_by_writes` passes on the GH runner; observed count recorded in
      the ledger row. **Evidence mechanism (a green run alone carries no number — the count
      currently only surfaces in the assert-failure message, `integration.rs:1175-1179`, and
      cargo captures stdout on pass):** the test gains an unconditional
      `eprintln!("rebuild_count observed: {n}")`, and ci.yml gains a dedicated step running
      exactly this test with `-- --nocapture`, so the run log carries the observed count on
      green. If the observed correct-code count falls OUTSIDE `1..=45`, the bound is
      re-derived from the new measurement and the test's platform-measurement comment updated
      in the same commit — a measurement update, never a loosen-to-pass (the ratchet forbids
      loosening only *acceptance criteria*; this bound is itself a measurement, and the ledger
      row records both old and new numbers).
- [ ] No trigger regression: `push: branches: [main]` and `pull_request` behavior preserved
      whatever Q1's answer adds.
**Expected test outputs:** GH Actions run URL, all jobs green, ubuntu `test` legs showing the
full suite count matching the Linux baseline±this plan's deltas.
**Commit:** `ci: run the Linux leg on branch work — admission machinery is Linux-only now`

---

## Edge cases considered

- **P2:** rename WITHIN root (`mv root/a root/b`): destination gets a walk; contents staged as
  `create` at new paths — correct (they ARE new paths; old paths get their own rename-away
  events). Rename-out: `is_dir()` false at delivery (P1 Probe D) → no walk. File (non-dir)
  rename: `is_dir()` conjunct excludes. Ignored dir moved in: `classify` → `Class::Ignore`,
  never reaches admission. Case-only rename on case-insensitive APFS: same-path clear + admit,
  harmless (walk stages nothing new). Dir moved onto an existing admitted path: `Modify(Name)`
  clear fires first (`:486-488`), then re-admission — the ordering already shipped in `f4bca8a`.
- **P3:** status run mid-daemon-start (lock held, nonce stamped, zero rebuilds yet): line
  already suppressed by the `> 0` gate — unchanged. Two rapid status invocations racing the
  probe: probe is stateless, no interference. Crashed daemon then NEW daemon starts: new epoch
  nonce ≠ `epoch_reload_nonce` → reader gate at `state.rs:318` already returns 0 — this phase
  only covers the window BEFORE a new daemon starts. **Recorded residual (pre-existing class,
  frequency raised):** when NO daemon runs, the probe transiently takes `LOCK_EX`
  (`daemon.rs:1928-1933`); a daemon calling `acquire_lock` in exactly that microsecond gets a
  spurious "already recording" refusal. Already reachable via doctor and purge; `status` is a
  habitual command, so exposure frequency rises. Narrowed-not-closed posture, same as
  `purge --orphans`' documented windows — recorded here and in the commit message, not fixed.
- **P4:** kill-9 after arm-stamp: stale `watcher_armed_nonce` persists but references a dead
  epoch's nonce — self-invalidating by construction. `.watch()` returning Err: stamp never
  written, daemon exits loudly (existing behavior), waiters time out loudly. Pre-this-field
  `state.json`: serde default → empty string → waiters treat as not-armed (correct for an old
  binary's daemon — a 10s timeout against a mixed-version daemon is the honest failure).
  **In-process writes race-free** (daemon state writes are all sequential on the run thread;
  the watcher callback only sends on a channel, the ctrlc handler only stores an AtomicBool).
  **External-writer residual:** `status --ack-degraded` (`cmds.rs:193`) is an unlocked
  whole-file read-modify-write; interleaved exactly across the arm-stamp it can erase
  `watcher_armed_nonce` for the epoch — every waiter then times out at 10s. Failure direction
  is LOUD (timeout, never a silent pass), window is microseconds, and the racer is a manual
  ack command — recorded, not fixed (fixing it means bringing state.json under a lock, a
  standalone round).
- **P5:** fork PRs and secrets: the test job needs no secrets; no change. Concurrency pileup
  from branch pushes: out of scope unless Q1 chooses branch-push, in which case a
  `concurrency:` cancel-in-progress group rides the same commit.
- **P1:** probe fixtures must be non-root and outside any virtiofs mount (the Colima lesson —
  but P1 is macOS-native, so the analogue is: fixtures on the real APFS volume, not a tmpfs).

## Open questions (answer before implementation)

1. **[gates P5] CI trigger shape.** Options: (a) add `workflow_dispatch` AND
   `push: branches: [main, 'fix/**', 'feat/**']` — every branch push runs the matrix
   (~10-15 min × pushes; cost is real); (b) `workflow_dispatch` only — zero automatic cost, but
   the trigger must land on `main` first (GitHub only offers dispatch for default-branch
   workflows) and every run is a manual step someone must remember; (c) keep CI untouched and
   instead commit the Colima container leg as a repo script (`scripts/linux-leg.sh`) so the
   manual ritual is at least reproducible. Not mutually exclusive with (b).
2. **[gates P2's POISONED arm only] If P1 measures POISONED:** (a) accept the documented
   residual permanently (status quo, now with measurement instead of intuition), or (b)
   commission a scan-diff design round (admission via subtree walk diffed against a baseline
   index the daemon would need to start maintaining — a real design effort with storage and
   correctness surface, not a task in this plan). No default; POISONED halts P2 pending answer.
3. **[shapes P3's json seam] Dead-daemon `status --json` epoch field:** (a) report `0`
   (consistent with "nothing currently claimable"; risk: indistinguishable from a live daemon
   that hasn't rebuilt yet), (b) omit the field entirely (schema-visible absence; consumers
   must tolerate missing fields anyway per the additive rule — but note: this option carries a
   named test edit, `pre_nonce_state_json_renders_no_reload_line` at `cmds.rs:1524-1527`
   asserts `payload["epoch_ignore_rebuilds"] == 0` with no daemon and REDs on a missing field;
   options (a)/(c) collide with nothing), or (c) keep the count but add a sibling
   `"stale": true` (most honest, most surface). Text line is suppressed regardless — only the
   json shape is open.

## Verification tail — manual E2E script (run after all phases, observable outcomes)

1. macOS: `agentrec record` in a fresh repo → `mkdir -p /tmp/outside/d && echo a > /tmp/outside/d/f1 && echo b > /tmp/outside/d/f2` → `mv /tmp/outside/d <root>/d` → wait 12s →
   `agentrec log --json | jq` shows a turn whose files include `d/f1` AND `d/f2`, `op:"create"`. Repeat ×5, 5/5.
2. macOS: `touch <pre-existing dir>` under an open bracket ×5 → `agentrec blame <any file inside>` never names the agent; `log.jsonl` gains zero entries for unchanged files. 5/5.
3. Any platform: start daemon, edit `.gitignore` twice, `agentrec status` shows the reload line; `kill -9 <pid>`; `status` shows NO reload line; `status --json` per Q3's answer; restart daemon → line absent until next real rebuild.
4. CI: run URL green per P5.
