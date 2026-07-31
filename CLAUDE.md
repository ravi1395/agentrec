# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working in this repository.

## What this is

**agentrec** — a local-first, tool-agnostic flight recorder for coding agents. It segments agent activity into *turns*, snapshots every touched file into a content-addressed store, and answers "who broke my repo — me or the agent?" via `log` / `diff` / `blame` / `undo`. The turn engine is an extraction of the production-proven `turns.rs` from Sutra (`~/Projects/sutra/src-tauri/src/turns.rs`) — when in doubt about engine semantics, that file is the reference implementation.

## Status (update after every delivery round — house rule)

**Slimmed 2026-07-29 at the founder's direction:** delivered-round narratives removed; full
history lives in `git log -- CLAUDE.md` (last full version at `204ff04`). This section now
carries only current state, what's next, and standing debts.

### Current state

- **D46 SERVICE-LEAK FIX delivered (2026-07-31) on `feat/phase-2-0-view-completion`, after the
  Phase 2.0 plan exit. Not gated by a skeptic yet; not pushed; no PR.** Closes the *product-defect*
  half of the 40-orphaned-LaunchAgents founder-pending entry: `init` no longer installs a
  `RunAtLoad`+`KeepAlive` unit under a temp root (with `--service` to force), and `doctor` reports
  units whose `--root` vanished as advisory-`pass`. `service prune` deliberately NOT built (only
  piece that shells `launchctl`; destructive; plist removal is founder-reserved). ACs S1–S9 added
  to `IMPLEMENTATION.md` §4 block **S** + register row **D46**; 9 claimd claims (7 CONFIRMED, 1 REFUTED-and-fixed, S9
  DECLARED-manual by design — it pins a moving corpus; see Founder-pending for the refutation). Suite **635 / 0 / 3** (from 615/0/2);
  clippy+fmt clean debug **and** release; new `AGENTREC_TEST_SERVICE_DIR` seam absent from release
  `strings`. **The 40 existing plists are untouched** — still 41 installed, re-counted after the
  full test run. Evidence incl. real-corpus run and five neuter proofs: `VERIFY-LEDGER.md` § "D46".
- **PHASE 2.0 PLAN EXIT REACHED (2026-07-31) on `feat/phase-2-0-view-completion` at `88c7e9b`
  + this commit. P5 GATE-PASSED; all 9 plan-exit checkboxes verified. Pushed, no PR.**
  P5 shipped `--json` read contracts for `diff`/`blame`/`status` as adapters over P4b's typed
  values — `Serialize` on `RepositoryHealth`/`Page<T>`/`Cursor`/`DiffResult`/`FileDiff`/
  `FileDiffState`/`BlameResult`/`BlameState`; `status --json` now flattens the pre-existing
  operational payload with the exact `RepositoryHealth` the view returned. Test baseline
  **615 / 0 / 2** (was 602/0/2 at P4b), 13 new `json_contracts::` integration tests, **33
  goldens**. Fable skeptic in an isolated worktree: **GATE PASS, 12/12**. The **plan exit
  itself** then took its own binding Fable-skeptic round in an isolated worktree at `46e5bf0`
  — **GATE PASS, 9/9**, both substitutions judged defensible only after the skeptic verified
  their premises in source (not on the orchestrator's word), and the moving-corpus honesty
  note reproduced independently: its own real-corpus re-run measured 1937/1944 = 99.6%
  against this round's 1930/1937 = 99.6% — same ratio, different absolutes, hours apart.
  Its five findings are in the ledger; three were corrections to this repo's own record and
  are fixed, two are carried as debts (a claim whose replay is narrower than its text; three
  replays hardcoded to this worktree path). Plan-exit checklist
  verified by the orchestrator independently: hard gate retired at the final commit (99.6%
  importable, denominator re-measured to 1937), zero-bytes dry-run re-verified, gap logic
  proven unified (one `view.rs::has_gap_after`, zero copies in `cli/src`), suite/clippy/fmt
  green on debug **and** release, no test seam in release `strings`, scope honesty clean
  (`PROTOCOL.md` untouched, zero MCP/Sutra paths). Full evidence:
  `VERIFY-LEDGER.md` § "Phase 2.0 plan exit" and the plan's own Final-acceptance section,
  each checkbox now carrying its verdict inline.
  - **Two plan-exit ACs were founder-ratified substitutions, not passes-as-written. Both are
    recorded in the plan rather than quietly satisfied:**
    1. **Item 4 (and P5's own AC-2) named an impossible event.** "...while bare `status` in
       the same fixture **still evicts**" cannot hold — the perf-evidence round moved eviction
       to the daemon tick; `status` is read-only now and a test pins that. Substituted with a
       **parity** assertion: *both* `status` and `status --json` must be zero-write. Stronger,
       not weaker — a write reintroduced on either path still reds. Re-adding eviction to
       `status` to satisfy the original text is forbidden.
    2. **Item 2 collided with P5's AC-1.** "Every P3 golden byte-identical" could not coexist
       with "route `status --json` through `RepositoryHealth`", which necessarily adds that
       struct's fields. Scoped to **human-form** goldens (absolute), with `--json` goldens
       permitted to change **additively only**. Verified: exactly one modified golden
       (`status_json.golden`, a `--json` golden), all 11 pre-existing keys byte-identical,
       **+7 keys, 0 removed**; six recall goldens *added*, none modified.
  - **Open questions 1–3 are carried forward UNANSWERED, explicitly** (founder ruling: invent
    no answers). Q1 wave-2 cut, Q2 the memory plan's 12 unchecked boxes vs memory v1 recorded
    merged, Q3 store reclaim. Disposition table in the plan. All three founder-owned.
  - **Residual closed by P5:** `cli/src/readcmds.rs` was in P5's edit scope, but the `load_log`
    direct-call residual below was **not** part of P5's ACs and is **not** closed — it stands.
  - **Sequenced next (unchanged by this exit):** Codex 2.1 → Protocol 1.0 freeze → MCP read
    2.2, which mirrors these exact serializers. No MCP code exists in this plan by design.

- **P4b EXECUTED and GATE-PASSED (2026-07-30) on `feat/phase-2-0-view-completion`.** Closes
  `RepositoryView` against the parent spec's six-method list (`open`/`list`/`diff`/`blame`/
  `recall`/`health`) — P4 had left `diff`/`blame`/`recall` unimplemented by design; P4b lands
  them plus `DiffResult`/`DiffError`/`BlameError`/`RecallPage`/`RecallError` and a shared
  `select_turns` walk (`list_records`/`list_records_of`, with `list()` surviving as a
  delegating wrapper). Test baseline **602 / 0 / 2** (was 595/0/2 pre-P4b-4); **33 goldens**
  (not 32, not 21 — the stale `21` was corrected in `P3.md`/`P4.md`/`P5.md`, and a stale `32`,
  which P4b-5's gate caught surviving in `P4b-1.md` through `P4b-5.md`, was corrected there too;
  32 was already wrong from `9d30e51` onward, when P4b-1's own gate added a 6th golden mid-task).
  Manual E2E: all 9 steps pass, pre- vs post-P4b binaries byte-identical on both a frozen
  dogfood clone (steps 1/2/4/5/9) and the live dogfood repo read-only (steps 6/7/8) — **caveat:
  step 7 (`recall --json`) is weakly discriminating**, the dogfood store has 0 fresh/3 stale
  memories so both binaries print `[]`. **P5 is now executable** — its dependency (P4b's typed
  values) is satisfied and its `{"files":[]}` literal is corrected (see P5.md amendment,
  derived from the merged `DiffResult`, not observed via a live `--json` flag since `diff
  --json` doesn't exist yet — that's P5's own scope).
  - **Residual, out of P4b's scope, not fixed here:** `cli/src/readcmds.rs:202`, `:426`, and
    `cli/src/memorycmds.rs:985` still call `load_log` directly in production paths — AC17's
    `rg` was scoped to `cmds.rs` only. P5 or a dedicated cleanup task should close this.
- **P2 + P3 EXECUTED and GATE-PASSED (2026-07-30) — merged into `feat/phase-2-0-substrate` at
  `73d01b9`, ledger + hygiene follow-ups at `6fa0de1`. Nothing pushed, no PR.**
  Method: sonnet implementers in parallel worktrees → opus reviewers → a Fable skeptic in an
  isolated worktree as the binding done-gate. **Round 1 of the gate FAILED; round 2 PASSED all
  13 ACs** (8 P2 incl. an added AC5b, 5 P3). `cargo test --workspace -- --test-threads=3` →
  **504 / 0 / 2** (pre-P2/P3 baseline 460/0/1); clippy `-D warnings` + fmt clean on debug **and**
  release; both import debug seams absent from release `strings`. 12 of 13 claims `EVIDENCED`.
  Full evidence, caveats, and residuals: `VERIFY-LEDGER.md` § "Phase 2.0 P2 + P3".
  - **T2 resolution measured at last: 17 resolved / n=1133 = 1.50%.** Two caveats are mandatory
    and must travel with the figure: **never render it "897 → 17"** (P1's 897 is a different
    definition), and **the oracle and population channels are disjoint, overlap 0** (both
    coincidentally n=17), so **none of the 17 population resolutions is scorable for
    correctness**. A rate far below the 41.5% candidate share is the honest outcome the plan
    predicted — git holds committed states only.
  - **Founder decision: both T2 gates retained**, at a measured cost of **127 of 144 correct
    resolutions discarded to remove 7 fabricated** (gate 1 alone: 137 resolved / 5.1% fabricated;
    both: 17 / 0%). Zero fabrication chosen — a wrong `before` is a wrong-byte revert source in
    undo history. The cost is recorded, not silent.
  - **AC5b's ≤1% bar was set wrong (by the orchestrating agent) and is unmeetable: NEVER quote
    it.** n=17 gives a 95% upper bound of 16.2%; ≤1% needs ~299 clean samples against a channel
    of 228. Honest phrasing: *0 fabrications in 17 guard-admitted samples (95% UB 16.2%)*. Claim
    `clm_4KSWSEZXS894D2P0DC92ZHH8MA` stays DECLARED-unattested as the permanent honesty record.
  - **The gate's round-1 blocker was the third instance of this plan's signature defect:** a
    confidently-worded comment asserting a real-corpus fact that was false. `strip_prefix(cwd)
    .ok()?` silently dropped **507 of 2,170 (23.4%)** file-producing entries, uncounted, beside a
    comment claiming cwd "is lexically a prefix in every real transcript". Two implementers, two
    opus reviewers, and an integration audit all read that line without measuring it. Fixed by a
    `skipped_out_of_cwd` counter — **countability was required; recovery was not authorized.**
    The figure now has three independent agreeing derivations.
  - Import honesty semantics established: derived `after` bytes carry `after_synthesized` and are
    never presented as observed (before this, `undo` fabricated "human or external edit" on files
    nothing touched); `status`'s rich-rate **excludes** imported turns, so a bulk import can no
    longer mask a dead hook at 100% rich.
  - P1's figures re-measured and **NOT stale** after the secret-path parity fix
    (`skipped_secret_path: 0`).

- **P4 EXECUTED and GATE-PASSED (2026-07-30) on `feat/phase-2-0-p4`. Nothing pushed, no PR; NOT
  yet merged to `feat/phase-2-0-substrate` at time of writing.**
  `RepositoryView` extracted into `agentrec-core/src/view.rs`. **537 / 0 / 2** (baseline
  re-measured on clean substrate: 504 / 0 / 2 — P4.md's "410 tests" and "21 goldens" are BOTH
  stale, goldens are 27). Fable skeptic in an isolated worktree: **round 1 GATE FAIL, round 2
  GATE PASS on all 7 ACs**, both mutation proofs re-run by the skeptic itself.
  - Gap logic **unified, not relocated**: one `view::recording_gaps` returns every uncovered
    interval tagged `Crash`/`Restart`/`TrailingStop`. The three old callers are NOT equivalent —
    `status` and blame's no-turn fallback are crash-only, staleness is any-kind-after-a-timestamp
    — so a naive collapse to one boolean would have moved blame output.
  - `health()` is a pure read; `status` calls it and then `enforce_budget` explicitly. Proven both
    ways (re-inserting the eviction fails the purity test).
  - **AC6 failed round 1 for a reason worth keeping: a cursor keyed on a turn id assumed
    uniqueness the ledger itself documents as false.** Orphan-recovery double-emit and resumed
    import `sessionId`s both put two records under one id; resolving by first match re-delivered
    records after a **pure append**, silently. Cursors now carry an occurrence ordinal.
  - **The signature defect struck a fourth time, and this time the agent introduced it.**
    `parse_log_line` guarded on the `type` tag being a *string*, so `{"type": 5}` was coerced into
    a turn — beside a retained comment promising a line carrying a `type` is never coerced. The
    gate caught it by probing the comment rather than reading it. Pre-P4 behavior (presence-keyed)
    restored.
  - A **+54% `status` latency regression** (13.4 → 20.6 ms on the real 2143-line log) from parsing
    `log.jsonl` twice was found by measuring rather than shipped: `load_log` and `load_ledger` now
    share one per-line classifier. Flat at **6.4 ms vs 6.6 ms**.
  - **Deviations, recorded in VERIFY-LEDGER.md:** `health(&self, budget)` not the contract's no-arg
    form (core stays free of CLI config); **`diff`/`blame`/`recall` NOT implemented** — that is
    **P5's entry condition**, since a `--json` serializer reimplementing diff or blame
    interpretation in `cli/src` reopens the seam P4 closed.

- **`main`** — carries the perf-evidence round: [PR #8](https://github.com/ravi1395/agentrec/pull/8)
  **squash-merged** to `main` as `4e04438` on 2026-07-30 (per-phase history survives only in the
  PR, not on `main`), then reconciled with the local docs-only chain by merge, then absorbed into
  `feat/phase-2-0-substrate` (this branch) by a further merge. Test baseline on `main` **443 / 0
  / 1**; the substrate's own baseline is tracked separately above (P4: 537/0/2, pre-merge).
- **Perf-evidence round (delivered, GATE PASS, now on `main` and merged into substrate)** —
  10k/50ms recall envelope closed (p99 11–23 ms, AC1.3 founder-attested), dedup counters,
  retention plan/execute split, eviction moved to a daemon tick, 3 real defects fixed (symlink
  dedup loss, `--stats` uninit, tick gap). **Linux CI leg now real:** run
  [30552318400](https://github.com/ravi1395/agentrec/actions/runs/30552318400) — all 5 jobs
  green (`ubuntu-22.04`, `ubuntu-24.04`, `macos-14`, lint, induced-low-watches). The macOS leg
  went red on its first attempt and green on rerun; cause recorded under residuals, not
  hand-waved. **Collision with P4b-4:** this round split `enforce_budget` into
  `plan_eviction`/`execute` and moved eviction off the `status` read verb onto a daemon tick —
  P4b-4's AC16 fixture must target `plan_eviction`, not `enforce_budget` called from `status`;
  see `docs/superpowers/plans/2026-07-30-p4b-branch-merge-state.md` §3.
- **Phase 2 spec hardened + gated (2026-07-28, 6 skeptic rounds, commits
  `bd0b679`/`bde8146`/`90202f6`/`204ff04`):** founder decisions 5–10 in
  `docs/superpowers/specs/2026-07-18-agentrec-phase-2-design.md` — (5) protocol freeze behind
  Codex 2.1; (6) MCP destructive gated on ≥20 human-confirmed `undo --confirm` (today 0) and
  `allow_modified` never honored in auto mode; (7) CLI demand probe defined
  (transcript-sweep instrument) then (10) **waived as a gate by founder override — MCP read
  2.2 is unconditional Phase 2 scope**, probe survives as post-ship evaluation; (8) import
  gate carries a fidelity report + ledger row, parse-only pass insufficient; (9) **Sutra
  parked** — Phase 2 re-centered on accessibility (import, trailers, `--json`, distribution,
  setup, Codex). Corpus decay measured: import is a **≤30-day rolling backfill**
  (`cleanupPeriodDays` default 30); "durable archive" claim embargoed until a re-import
  mechanism ships; 4-tier before-ladder predicted this day (T1 42.5 / T1.5 25.3 / T2-cand 24.5
  / T3 7.7 → 67.9% honest, 92.3% banned) — **superseded, twice, by P1's real-corpus runs, see
  below: honest figures are 43.8% (counting `create` ops as reconstructible) / 31.7% (actual
  pre-edit bytes only); an intermediate 40.4% reading was also wrong and is retired.**
- **Phase 2.0 plan chunked:** `docs/superpowers/plans/tasks/P1..P5.md` (standalone,
  fresh-executor-ready); plan has a 9-checkbox "Final acceptance — plan exit" section.
- **P1 EXECUTED + GATE PASS (2026-07-29) — merged into `feat/phase-2-0-substrate` at `cc026c9`;
  nothing pushed, no PR.**
  `agentrec import claude --dry-run` built; **460 / 0 / 1** on `fix/p1-t15-path-normalization`
  (pre-P1 baseline 428; the 8/8 gate ran at 447, before the two T1.5 defect fixes added
  coverage); clippy+fmt clean debug & release; debug seam absent from release `strings`.
  Final Fable skeptic in an isolated worktree: **8/8 ACs PASS**, every figure independently
  reproduced (against figures since proven wrong twice over — see below). Real-corpus gate
  run, corrected 2026-07-29 evening (`docs/verify/p1-gate-run-t15fix2.txt`): **1631 sessions,
  1625 importable — 99.6%** (bar ≥90%), peak RSS **17.17 MB** (bar <500 MB). Fidelity row +
  anti-overclaim rider in `VERIFY-LEDGER.md`. **The Phase 2.0 hard stop is retired with a
  fidelity row, not a parse-only pass** (spec decision 8 satisfied).
  - **The plan's before-ladder prediction did NOT hold, and the correction itself was wrong
    once before landing.** Predicted T1 42.5 / T1.5 25.3 / T2-cand 24.5 / T3 7.7. A first
    real-corpus reading (`34d858c`) measured T1 39.9 / **T1.5 0.5** / T2-cand 44.8 / T3 15.0
    and claimed T1.5 was "genuinely near-empty, not under-detected" — **that claim was false.**
    T1.5 was under-detected by a bug (lookup compared an absolute `filePath` against
    `trackedFileBackups` keys that are relative to session `cwd`, so the raw compare almost
    never matched); fixing it raised the count, but then over-counted via stale file-history
    blobs (written at snapshot time, not per edit — ~30% of the fixed reading's resolved bytes
    were fabricated pre-*snapshot* states). Corrected figures, now measured over **2159**
    entries: T1 856 (39.6%, of which 595/27.6% inline `originalFile` + 261/12.1% `create` ops)
    / T1.5 90 (4.2%, ground-truth 101 correct / 1 fabricated) / T2-cand 897 (41.5%) / T3 316
    (14.6%). **Honest reconstructible is 43.8% (T1+T1.5 counting `create` ops) or 31.7%
    (actual-bytes only) — not 67.9%, and not the intermediate 40.4% either** — propagated
    across spec/plan/task/measurement docs/VERIFY-LEDGER. 92.3% stays banned, and the same ban
    now covers the **~85% ceiling** (43.8 + 41.5 T2-cand) — both are upper bounds by the
    identical argument (git holds committed states only).
  - **99.6% is an ingestion rate, never a recovery rate** — only ~184/1631 sessions (11%) carry
    any file mutation at all. Never quote it bare; the rider in VERIFY-LEDGER.md travels with
    it.
  - Defect worth remembering: first gate run reported `t1_5 = 0` because the classifier
    matched a `type: "snapshot"` literal the *synthetic fixture had invented*; the real corpus
    uses `file-history-snapshot`. Every test passed while nothing real resolved. Fixed to
    presence-based harvesting. **Fixture-only evidence cannot close a corpus-shape claim.**
  - 2 claimd claims REFUTED on malformed replay commands (multi-filter `cargo test`, which
    takes one TESTNAME) — the underlying tests pass under a corrected invocation, but REFUTED
    is not amendable and re-declaring an equivalent is forbidden as dodging. Founder call.

### Now / next (in order)

1. ~~**P4**~~ → ~~**P4b**~~ → ~~**P5**~~ → ~~**plan-exit checklist**~~ **all done and gated
   (2026-07-31).** Phase 2.0 is at plan exit; see Current state. The goldens were never
   weakened or regenerated to make extraction pass — every human-form golden is byte-identical
   from `10d235d` to exit, which was the whole ratchet.
   **Remaining on this branch: open a PR.** Nothing is merged to `main` yet.
   Wave 2 (aider import, trailers + shim, npm/mise) after or parallel per open question 1,
   which is still unanswered.
2. **NOT DONE — plan exit was reached without it, deliberately and on the record.** It was
   never a plan-exit checkbox, so doing it would have been scope the exit did not authorize;
   saying so beats letting a "recommended before plan exit" line rot into a false implication
   that it happened. Still owed, still cheap. Original note follows verbatim.
   **One-line fix recommended before plan exit** (skeptic-flagged, non-blocking): import's
   `existing_ids` never gains ids appended during the current run, so two session files sharing a
   `sessionId` in one run would append two turns with the same id and different `files`, making
   `diff`/`show`/`undo <id>` error "ambiguous". The shape is real, not hypothetical — the corpus
   holds one such `sessionId` across two project dirs (worktree-resumed session), inert today only
   because one copy is a 1-line cwd-less stub.
3. Then: Codex 2.1 → Protocol 1.0 freeze → MCP read 2.2 (unconditional, thin adapter over
   P5's serializer). 2.3 stays evidence-gated.

### Founder-pending (agent cannot or may not do these)

- ~~**40 orphaned `com.agentrec.*` LaunchAgents**~~ — **REAPED 2026-07-31 at the founder's explicit
  instruction.** The machine now carries **exactly one** agentrec unit,
  `com.agentrec.bfa6bde6eaa4` → `~/Projects/agentrec`, loaded and healthy (pid 865, launchd status
  **0**; before the reap 23 orphans were loaded and failing with status **78**, i.e. launchd was
  repeatedly respawning recorders whose `--root` was gone). Method: every plist archived first to
  `AGENTREC_LAUNCHAGENT_ARCHIVE` (below) — reversible, per the house never-delete rule — then the
  removal list built from agentrec's OWN classifier (`service::scan_units` `VanishedRoot` bucket),
  each root independently re-checked absent, the live unit asserted out of the list, and every
  entry asserted present in the archive before a single `rm`. Then `launchctl bootout
  gui/$(id -u)/<label>` followed by `rm` — 39 removed, 0 failures. `doctor`'s orphaned-services
  check is now a silent `pass` with no note, which is the D46 detection half confirming its own
  fix end-to-end on the real machine.
  - **Archive (delete when satisfied; nothing else references it):** `/Users/ravichandrasekhar/agentrec-launchagents-archive-20260731-152414` — 40 plists.
  - **Two of the 42 were removed before this pass and NOT by the agent.** `com.agentrec.039364bb7dfe`
    (the label `doctor` happened to print as its example) and `com.agentrec.c0bf764acce7` (the unit
    D46's own suite leaked, whose removal command was surfaced in a runnable block) both vanished
    between checks. Almost certainly the founder ran the two printed commands; that is inference
    from which labels disappeared, not proof, and it is recorded as inference.
  - **The product defect that produced all 40 is fixed (D46)** — see Current state. `init` under a
    temp root installs no unit, so this cannot silently re-accumulate; `doctor` reports any that do.
- **BLOCKS/AFFECTS A PUBLIC PHASE 2: `init` bakes a CANONICALIZED exec path, which breaks every
  Homebrew user on upgrade.** `initcmd::current_exe()` does `current_exe().canonicalize()`;
  canonicalize fully resolves symlinks, and Homebrew installs binaries as symlinks into
  version-pinned Cellar paths (verified on this machine: `/opt/homebrew/bin/rg ->
  ../Cellar/ripgrep/15.2.0/bin/rg`). README.md:51 documents `brew install
  ravi1395/agentrec/agentrec` as a supported path, so a brew user's unit records
  `…/Cellar/agentrec/<version>/bin/agentrec`. Two consequences — **(1) certain: after `brew
  upgrade` the service keeps running the OLD binary forever** (user upgrades, recorder doesn't);
  (2) once the old Cellar version is cleaned up, the unit becomes a launchd status-78 respawn loop
  on the user's machine — exactly the failure this repo just cleaned 39 of. Fix is small and does
  not need canonicalize: record the INVOCATION path (`/opt/homebrew/bin/agentrec`), which is stable
  across upgrades. Canonicalizing is correct for the **root** (dedupes `.` vs absolute, D5) and
  wrong for the **exec**. NOT BUILT — found 2026-07-31 while root-causing the 40 orphans. No brew
  formula exists in this repo (`find` for `*.rb` → none), so the tap was not inspected; the claim
  is about the documented install path plus the verified symlink layout.
- **The 40 orphans' root cause was NOT what this file said this morning, and the correction
  matters.** The recorded cause was "`init` in a temp dir without a matching uninstall". The
  archived plists say otherwise: **39 of 40 pointed at a `target/debug` build directory** — 21
  `~/.gate3`, 6 `~/.gate2`, 4 `agentrec-phase2`, 4 `~/.gate-linux`, plus scratchpad worktrees — i.e.
  the dominant vector was **agents running `init` from throwaway build trees**, not temp roots. The
  one surviving unit is the one installed from a stable path (`~/.local/bin`). Mechanism: `init`
  bakes a SNAPSHOT OF TWO PATHS (root + `current_exe()`), neither validated at load time, and
  `KeepAlive` turns a stale snapshot into permanent noise rather than one clean failure. launchd
  status **78** is an exec failure, not the daemon refusing a missing root.
- **Residual gap D46 does NOT close, and the one that matters for real users:** a unit whose
  recorded path — root **or** exec — vanishes on a **non-temp** path. Deleting, moving, or renaming
  an ordinary repo leaks an identical unit and the temp guard never fires. `doctor` reports it;
  nothing prevents or reaps it. This is also the new evidence bearing on **`service prune`**, whose
  deferral rationale (only piece that shells `launchctl`; destructive; founder-reserved) still
  holds — but whose stated mitigation was "the user reaps by hand", and the leak source is now
  known to be systemic rather than agent-only. Founder decision, not an agent reversal.
- **Re-declare AC5b's claim with honest wording** (founder decided the approach 2026-07-30; the
  agent must not run it — an agent re-declaring its own unmeetable claim with weaker text is
  indistinguishable from dodging a refutation). Text to use: *0 observed fabrications on the
  guard-admitted channel (n=17); 95% upper bound 16.2%; the whole verifiable channel is 228
  cases, so a bar below that bound is not establishable by this oracle.*
- **Pinned decision 14 conflicts with the corpus — needs a ruling.** P1.md pins `cwd` as
  session-level (first line carrying one wins) and marks it non-re-litigable, but **67 sessions
  carry more than one distinct `cwd`** (usually a subdirectory move). Resolving each backup key
  against the `cwd` in effect at its own snapshot line finds ~271 T1.5 candidates vs the current
  rule's 210-before-staleness-gating, at marginally *better* precision. Deliberately NOT changed —
  a pinned decision is not an executor's to overturn. Founder decides whether to amend it.
- **D46 AC-S2's claim REFUTED, correctly, and NOT re-declared** (`clm_4AFDDT3XCSFHKDZ06D926XJVNY`,
  exit 101). `claimd verify` replays committed state from a checkout under `$TMPDIR`; the test
  backing it used `env!("CARGO_MANIFEST_DIR")` as its non-temp control, beside a comment asserting
  that is "a real, non-temp path" — false in exactly that checkout, where the path IS temp and
  `Install` is the wrong expectation. Reproduced by cloning to `$TMPDIR`. **Production code was
  never wrong; the test's control path was.** Fixed (fixed non-temp paths covering both the
  canonicalize-succeeds and canonicalize-fails branches) and re-verified green from a `$TMPDIR`
  checkout — but REFUTED is not amendable and re-declaring an equivalent is forbidden as dodging,
  so AC-S2 now carries a passing test and no live claim. Founder decides whether a re-declaration
  is warranted. Same disposition as the two P1 claims below. Worth keeping: this is the
  fifth instance of this repo's signature defect — a confidently-worded comment asserting an
  environment fact nobody measured — and the first one caught by the claim protocol itself rather
  than by a skeptic.
- 2 claimd claims REFUTED on malformed replay commands (`clm_2DDM03JPR1N9JT4QHYHBTZC1SM`,
  `clm_07FVVKJGHZS8ZFSR2998HE7QRP`) — multi-filter `cargo test`. Underlying tests pass under a
  corrected invocation, but REFUTED is not amendable and re-declaring an equivalent is forbidden
  as dodging.
- ~~Open the PR for `fix/perf-evidence-round`~~ **done** — PR #8 merged to `main`
  (`4e04438`/`72c4b82`/`34bbb0c`) and absorbed into this branch by merge; Linux CI leg is real
  (run `30552318400`, 5/5 green — note this run predates `import`, which lives on
  `feat/phase-2-0-substrate`, not `main`; the Linux matrix has never run this branch's HEAD).
  **P1's AC7 residual is NOT closed by this** — checked before claiming it: `cli/tests/
  import_claude.rs` does execute `peak_rss_mb()` in-process (spawns the real binary, asserts
  `report["peak_rss_mb"].is_number()`), but `is_number()` passes under either the macOS or Linux
  `RSS_DIVISOR` — it can't discriminate which constant fired. `rss_raw_to_mb` itself is
  unit-tested with both divisors passed explicitly, which proves the arithmetic, not the
  `#[cfg(target_os = "linux")]` selection. Still open; needs a magnitude assertion on `import
  --dry-run`'s reported `peak_rss_mb` running under Linux CI on this branch's HEAD to close.
- `rm` retained archives `.agentrec/objects.archived.1784328469` (2.6 GiB) + `.1784934498`
  (15 MiB) — reversible-until-deleted, disk-only.
- 13 stale local branches (git-guardrails hook blocks agent `branch -D`; command was handed
  over 2026-07-28).
- 5 claimd claims DECLARED awaiting manual attestation (never self-attested).
- **66 claimd claims STALE on scope-drift** after the PR #8 merge brought the round's file
  content onto `main` (`claimd status`, 2026-07-30). Re-confirmation is owed and was NOT done
  in the merge — same shape as the prior rounds' "re-confirm N stale claims" commits.
- Undecided claimd doc-scope rule: `PROTOCOL.md` is not lint-ignored — next normative-doc
  edit fires the Stop hook again.
- Demand/launch gate (Show HN etc.) never run — ROADMAP Phase 0's 30-day kill criterion has
  no data; 2.2's post-ship evaluation row needs a probe repo picked + `agentrec init` there.
- Plan open questions 2–3: memory-plan stale checkboxes; store-churn reclaim.

### Standing debts & residuals (recorded, not blocking)

- `log.jsonl` churn history (9602 `.remember` entries) still renders as churn blasts in
  `log`/`show`; not byte-reclaimable without a third sanctioned rewrite class — deliberately
  not built.
- claimd coverage debt rows: `cli/src/cmds.rs` (P3 residuals round) and `cli/src/purgecmd.rs`
  (honesty round) — touched-uncovered, retroactive declaration refused by design.
- P1 probe verdict bounded: fixture aging can't reproduce weeks-old FSEvents journal history;
  the dogfood daemon is the observatory.
- Perf/timing margins are macOS+one-Colima-VM evidence; population-level claims close only
  over CI history. PR #8 contributed the first two GitHub-runner Linux data points for this
  round; one PR is not a population.
- **`hook_recall_hard_wall_deadline` is runner-coupled** (first observed PR #8, macos-14 job
  `90903831567`): it timed **204.1 ms** against a `< 200 ms` assert and passed on rerun. The
  invariant held both times — a 600 ms blocked pin read was abandoned, not waited on — but the
  assert measures *whole-process* wall including fork+exec, leaving ~4 ms of margin on a shared
  runner. Bound raised to 300 ms (founder decision 2026-07-30): still 2× under the 600 ms
  block, so the neuter that removes the wall still reds. The tighter fix (subtract a measured
  spawn baseline in-test) is **not** done and stays available if 300 ms also proves flaky.
- Memory dogfood ladder effectively not started (store prepped 2026-07-17; clock never ran
  clean).

## Working method

Work runs as the `/ratchet` loop (`.claude/commands/ratchet.md`; staged in `claude-setup/` until installed — a ratchet only tightens, ACs are never loosened to pass): Opus orchestrates, Sonnet `implementer` agents build against named AC ids, an Opus `skeptic` agent with fresh context reviews per-AC (PASS/FAIL/UNTESTED — no PASS without a test), failures loop back, and every round ends by updating the Status section above. Never weaken an AC to pass; escalate ambiguity to the founder.

Read before building; do not invent semantics that contradict these docs:

| Doc | Contents |
|---|---|
| PROBLEM.md | Why this exists; who has the pain; why tool-neutral |
| SPEC.md | v1 reference implementation: daemon, 5 CLI verbs, prompt posture |
| PROTOCOL.md | **Normative.** Signal + turn-record schemas, conformance levels L0–L3, MCP surface, versioning rules |
| ROADMAP.md | v1 → v4+ phases, measurable gates, kill criteria |
| INTEGRATIONS.md | Four-ring integration thesis; v2 designs (Claude Code, Codex, VS Code) |
| IMPLEMENTATION.md | Decision register D1–D44 + exhaustive acceptance criteria per release (incl. v0.3 durability + v0.4 accessibility amendments) |
| REVIEW.md | Hostile review that forced the v0.2 capture redesign — read before touching capture semantics |

Conflict resolution order: PROTOCOL.md > IMPLEMENTATION.md decision register > SPEC.md > everything else.

## Architecture

```
EMITTERS                      RECORDER                      CONSUMERS
Claude Code Stop hook ─┐
Codex Stop hook (v2) ──┼─→ .agentrec/signal.jsonl ─┐
(any L1+ tool) ────────┘      (append-only inbox)  │
                                                   ▼
                              agentrec daemon ("record")
fs mutations ─→ watcher ─→    TurnEngine: signal boundary,
(notify, 1.5s debounce,       else 10s quiet window;        ─→ .agentrec/log.jsonl   ─→ CLI (log/diff/blame/undo)
 denylist filter)             one open turn per root            (canonical, JSONL)    ─→ MCP server (v2)
                                   │                        ─→ .agentrec/objects/     ─→ VS Code ext (v2, read-only)
transcripts (prompt) ─→ scrub ─────┘                            (sha256 CAS, 10MiB cap)─→ Sutra GUI (v3)
```

Two crates in one cargo workspace: `agentrec-core` (lib: TurnEngine, BlobStore, formats, scrub) and `agentrec` (bin: daemon + clap CLI). Threads + `notify`, no async runtime. Consumers never need the daemon running — all reads are file-based.

### Key semantics (do not violate)

- **Turn grades:** `rich` (hook signal → tool/model/prompt attached) vs `bare` (quiet-window inferred). A bare turn is an *unattributed activity window* — a human vim save produces the same fs signature — so bare turns are never rendered as agent activity and never fabricate attribution.
- **Bracketing (v0.2, load-bearing):** Claude Code integration installs `UserPromptSubmit` (start) + `Stop` (stop). Open bracket suppresses quiet-window closure; on stop, interim bare turns are retroactively merged into the rich turn. Start-without-stop = close at last mutation, `truncated: true`.
- **Git turns:** mutation bursts coinciding with `.git/HEAD`/index/ref transitions are rich turns with `tool: "git"`; hidden from `log` by default. Never let a `git checkout` become a 400-file bare turn.
- **Epochs & gaps:** daemon start/stop append `type:"epoch"` records; blame across an uncovered interval must say "attribution stale — recording gap", never guess.
- **Append-only everything:** `log.jsonl` and `signal.jsonl` are never rewritten. History is corrected by appending. Signal consumption tracked by byte offset in `state.json`. Turn ids are machine-scoped ULIDs.
- **Two predicates, never conflated:** `modified-since` (hash ≠ turn's after) gates every destructive op; `human-edited-since` (modified AND not covered by any *rich* turn) is blame display only. Bare turns never count as coverage.
- **Undo is a turn:** every revert (CLI or MCP) snapshots current state first and appends a new turn with `tool: "agentrec"`. Reverts are blame-able and re-revertible. `skipped` (over-cap) and `withheld` (secret-pattern) files are never revertible.
- **Prompts and snapshots both scrubbed:** prompt scrub (secret regexes + entropy) runs *inside* the persistence function; secret-file patterns (`.env*`, `*.pem`, credentials) are never snapshotted (`withheld: true`). Local-only; no network code exists in v1–v2; never claim "provably" safe.
- **Agent-driven undo** is gated by `config.toml: mcp_destructive = off|confirm|auto` (default off); `auto` requires the two-phase confirm-token flow; safety keys on `allow_modified` (PROTOCOL.md §8).
- **Watch filtering:** gitignore-derived by default, plus denylist `.git` contents (except HEAD/index/refs — needed for git-turn classification), `.agentrec` (self-write suppression — no feedback loops), `node_modules`, `target`, `dist`, and user globs. Linux inotify limit exhaustion = loud startup error.
- **Clocks:** UTC wall clock in records; monotonic clock for quiet-window measurement.

## Commands (once code lands)

```bash
cargo test                    # unit + engine table tests (agentrec-core)
cargo test --test integration # drives the real binary against tempdir fixtures
cargo clippy && cargo fmt     # CI-enforced
agentrec init && agentrec record   # dogfood in this repo itself
```

## Conventions

- Every acceptance criterion in IMPLEMENTATION.md §3–§7 maps to at least one automated test; new features add their AC there first.
- Protocol changes: additive-only within a major `v`; update PROTOCOL.md + conformance fixtures in the same commit; consumers must tolerate unknown fields.
- Platforms: macOS + Linux. No Windows-specific code, but no hardcoded path separators either (D19).
- License: Apache-2.0 (open-core; all v1–v3 code). Hosted/team features v4+ commercial.
- Never delete user data: `purge` is the only deletion path and only for objects past TTL / by explicit flag; archives, never silent removal.
