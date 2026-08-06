# VERIFY-LEDGER — M3 "Ship"

Done-definition for this round (founder-confirmed 2026-07-10): **M3 code-complete + laddered.**
Every AC verifiable on this macOS box is PASSed with pasted evidence in the SDD final gate.
The rows below are ACs whose *only* proof requires an environment absent here (Linux, containers,
Homebrew, a 7-night clock, a GIF pipeline). They are **OPEN** — never counted as PASS, never
allowing a "M3 shipped" claim. Each names the exact command/environment that closes it.

## Open (gated) rows

| AC | Gate | Closes when |
|---|---|---|
| Residuals round P2 — macOS rename-only admission, live bracket escalation | `directory_moved_into_root_records_contents` (both platforms) and `create_kind_does_not_admit_on_macos`/`metadata_only_event_on_directory_that_predates_daemon_stages_nothing` (macOS) prove the fixture-level behavior deterministically, but FSEvents delivery timing under a real agent bracket (`UserPromptSubmit` → activity → `Stop`) is not unit-testable — Phase 1's own Probe B measured an intermittent ~2/10-trial `Create(Folder)` coalescence signature on this exact machine, and a prior round measured a ~50%-behavior class (`f4bca8a`, 2/4 bracket trials) that a single clean implementer run had reported as fully clean. **A single clean run here is NOT acceptance.** | A skeptic runs, on a real daemon against a real repo, with an agent bracket open: **5/5** trials of `touch <pre-existing dir>` (no content change inside it) produce **zero** fabricated entries, AND **5/5** trials of `mv dir-in <root>` (a directory built outside the root, moved in) record **all** of its contents. Both legs, same protocol as `f4bca8a`'s 5/5. Mark this row CLOSED only after that run, not after this round's own unit/integration proof. |
| Residuals round P5 — Linux leg green on a real GitHub runner, this branch's HEAD — **CLOSED 2026-07-27** | Closed by the route the row itself anticipated but this branch could not take alone: not `workflow_dispatch` (still default-branch-blocked at close time) but the **`pull_request` trigger** — [PR #7](https://github.com/ravi1395/agentrec/pull/7) fired the full matrix on this branch's HEAD (`ff76b58`). Run [30309231117](https://github.com/ravi1395/agentrec/actions/runs/30309231117): **all 5 jobs green** — `test (ubuntu-22.04)` and `test (ubuntu-24.04)` each **431 passed / 0 failed / 2 ignored** (summed from the 8 `test result:` lines per job), plus macos-14, lint, and the induced-low-watches job. First GitHub-runner execution of the admission machinery since it became Linux-only. | Closed here — run URL above satisfies the criterion verbatim (ubuntu matrix legs, green, triggered on this branch's HEAD). |
| Residuals round P5 — rebuild bound observed on ≥2 distinct Linux environments — **CLOSED 2026-07-27** | **Second and third environments recorded same day:** run [30309231117](https://github.com/ravi1395/agentrec/actions/runs/30309231117) (PR #7, this branch's HEAD) — the dedicated `--exact --nocapture` CI step printed `rebuild_count observed: 11` on **both** `ubuntu-22.04` and `ubuntu-24.04` GitHub runners (solo-run population, matching the Colima solo figures 9–12; both inside `1..=45`). With the Colima VM below, the bound now has data from three distinct Linux environments. **First Linux data point recorded 2026-07-27** — `scripts/linux-leg.sh` executed for the first time (Colima VM, rust:1-bookworm arm64, non-root uid=1000, fixtures on overlayfs, suite green **431 passed / 0 failed / 2 ignored**): `rebuild_count observed: 8` in-suite at `--test-threads=3`; five solo runs of the same test measured 9, 11, 12, 10, 10; a post-run skeptic's independent in-suite run on the **same VM** measured **14**, and the post-fix re-run (sentinel clear moved host-side) measured **19**. All inside `1..=45`. Two honesty notes ride with the numbers: (a) an earlier revision of this row framed 8–12 vs the prior round's 13–21 as "two VMs, two regimes" — refuted twice within hours: the same VM went on to measure 14 and then 19, overlapping the prior VM's range entirely; the honest claim is only that observed in-suite counts span **8–19** on this VM and 13–21 on the prior one, all inside the bound, with the spread uninstrumented — nothing bounds how close correct code drifts toward a neutered count's range on an untested environment; (b) the number only exists because the run added `--show-output` — the shipped script omitted it, libtest captures `eprintln!` from passing tests, and the pristine run's log verifiably contained **zero** `rebuild_count observed` lines, i.e. the script as shipped silently defeated the very evidence-on-green this phase added. (The earlier false "Local Colima-VM number … recorded below" claim in this row's history was caught at the round's gate and corrected before any number existed; this entry is the first real one.) | Closed here — GH-runner data points recorded above. **Full record:** Colima VM 2026-07-27 — in-suite 8, 13, 14, 19 (`--test-threads=3`, four independent full-leg runs), solo 9–12. GH runners (run 30309231117): solo 11 on ubuntu-22.04, solo 11 on ubuntu-24.04. All inside `1..=45`; Linux neutered code measures 87–103. |
| Memory: 10k/50ms hard perf envelope — **CLOSED 2026-07-28** | **Measured (perf-evidence round P1, AC1.3).** Release build at worktree `051a9a4` (`--stats` present, `strings \| grep -c AGENTREC_TEST` = **0**), macOS arm64, throwaway root under `scratchpad/ac13` — deliberately NOT the live dogfood store, whose 3-fact corpus cannot reach 10k, and NOT `~/Projects/agentrec`, which the production daemon (pid 783) watches and would have recorded the whole seeding pass into. 10,000 records seeded per `seed_capped_stale_heavy_corpus`'s schema (integration.rs:6196): 140 orphaned (`RECALL_VERIFY_CAP` 128 + 12, shared fact text, missing pins) + 60 fresh (real files, real sha256 pins) + 9800 filler; 2.8 MB `memory.jsonl`. **360 real `agentrec hook claude` subprocess runs, 0 nonzero exits**, in three legs of 120, each read out via `agentrec memories --stats`: **(A) stale-heavy / pathological** — orphans win the idx-ascending tiebreak so the verify walk caps out and injects nothing: **p50=11 p90=12 p99=16 max=16 ms**, `injected=0 budget_exceeded=0 failure=0 capped_empty=120 capped_total=120`. **(B) fresh-first** — fresh block inserted first so injection succeeds: **p50=10 p90=11 p99=13 max=16 ms**, `injected=120 budget_exceeded=0 capped_total=0`, and 0/120 empty stdout (every run emitted a real block, `n=5`). **(C) leg B under 8-way CPU load** — **p50=16 p90=18 p99=22 max=23 ms**, `injected=120 budget_exceeded=0`. **Criterion met with ~2.3× margin at worst (22 ms vs 50 ms), and zero `budget_exceeded` events across all 360 runs** — i.e. the envelope holds on the merits, never via the fail-open suppression the criterion's parenthetical would also have accepted. Leg C exists because an unloaded sequential run is the "reasoned safe, unobserved" margin class this repo has been burned by; it carries its own **positive control** — the load demonstrably shifted the distribution (p50 10→16, p99 13→22), so it was not a no-op. **Honest bounds on this figure:** synthetic corpus, not organic; macOS/APFS only (Linux unmeasured, same posture as every other timing margin in this repo); warm page cache, though the effect is small (the very first cold probe measured 17 ms against leg A's p99 of 16); a 10k `memory.jsonl` is fully re-parsed per hook process by design (the hook is a fresh process per prompt, so nothing caches — see the plan's rejected list), so this figure scales with corpus size and should be re-measured if a store materially exceeds 10k. **Process note:** leg C's 8 background spinners leaked — `kill $LOADPIDS` failed silently because the subshells had reparented to pid 1 — and were found still at ~100% CPU each and killed by pid. Legs A and B ran before the spinners existed and are unaffected; this is the same orphaned-process class that once starved this machine to `posix_spawn failed` and perturbed FSEvents timing, caught here only because the cleanup was verified rather than assumed. | Closed here — figure above satisfies the criterion verbatim (10k-record store, release build, p99 < 50 ms, on the dogfood machine). Seeding + measurement commands recorded verbatim below this table rather than as a committed script (the plan's rejected list forbids one: this repo shipped two defects in a never-executed script). |
| Memory: 10k/50ms hard perf envelope — original gate text (superseded by the CLOSED row above) | F2 (2026-07-12) made `RECALL_BUDGET_MS`=50 a genuinely **hard** cooperative deadline — `cmds::inject_memory` threads `deadline: Instant` into `memory::recall_with_deadline`, which checks it at each `load_effective`/`bm25_rank`/verify loop boundary and bails to an empty, `budget_exceeded`-flagged result the instant it passes, instead of the old measure-after-the-fact suppression. This closes the *correctness* gap (a slow recall can no longer delay the prompt unboundedly — proven deterministically by `hook_recall_bails_at_injected_deadline` via a test-only already-expired-deadline injection, no runner-speed coupling) but the *10k-record p99 latency envelope itself* is still timing-dependent and still not CI-provable: CI proves exit-0 + (now, typically faster) bailout at 3000 records within a 500ms slack bound, not the true steady-state 50ms envelope at 10k records on a release build. | A dogfood-machine timing run: 10k-record store, `agentrec hook` p99 recall < 50ms (or block correctly suppressed past budget, now via the hard deadline rather than a race). Marked LADDERED in IMPLEMENTATION.md INV-M4, never claimed CI-proven. |
| Memory: real Claude Code session shows injected block | The UserPromptSubmit hook injection is proven by integration tests (`hook_injects_fresh_memories_into_stdout`, concurrent-append, fail-open) driving `agentrec hook` directly; a live agent turn actually receiving the block in-context is env-gated. | A real Claude Code session in this repo with the memory hook installed: a prompt matching a remembered fact shows the `\`\`\`agentrec memory` block in the agent's context. |
| Memory: 1-week dogfood + skill-driven candidate — **CLOSED FAILED 2026-07-31** | **FAILED — the window expired without a clean run and the store decayed instead.** T0 = 2026-07-18, window closed ~2026-07-25; measured at close on **2026-07-31**, 6 days after the window closed and 13 days after T0 (exact commands + output: subsection *"Memory dogfood — measured at close (verbatim, 2026-07-31)"* below). Baseline at T0 was `3 fresh / 0 stale / 0 rejects / 13 injections / 0 failures`; the same counter now reads **`memory: 1 fresh, 2 stale, 0 rejects, 628 injections, 0 failures`**. The close condition is a **conjunction** of three criteria; each is adjudicated separately and none is flattened into the others. **(1) Non-trivial hit-rate — UNMEASURABLE, not merely "low".** `.agentrec/memory-stats.jsonl` appends one line per *successful* injection only (`{"n":k,"ts":…}`) and records nothing for a hook run that recalled zero, so the counter has **no denominator** and no hit-rate is derivable from this store at all. What it does show: **629** lines at the stats read (**628** at the `status` read ~1 minute earlier — the counter is live-monotonic and this very session is injecting; the red-team's earlier reading of 626 is the same counter, earlier), `n`=1 on 516 / `n`=2 on 61 / `n`=3 on 51, cumulative since 2026-07-12 — so **628 is not a window figure**: **4 pre-T0 / 131 in-window (07-18…07-25) / 494 post-window**, and **zero injections on 07-19 through 07-23** — five consecutive days inside the window with no recall at all. (Cumulative through 07-18 is 25, straddling the T0 baseline's 13 — baseline and stats file corroborate each other.) **(2) DEGRADED/staleness never falsely fires — MET; the one conjunct that passed.** Both stale facts are *genuine* drift, not false staleness: `agentrec-core/src/retention.rs` and `cli/src/purgecmd.rs` moved to new hashes inside real recorded turns (`t_01KYSR5958KXWRJ2WRHHRXDCJV`, `t_01KYJT06M1R4E2X6RAY8K8K6Y9`). This conjunct passing does **not** soften the row. **(3) ≥1 SKILL-emitted candidate becomes a genuinely useful recalled memory — DEFINITIVELY FAILED, and the emitter never fired at all.** All **6** records in `memory.jsonl` are `origin: "human"` (3 `assert` + 3 `reverify`); **0** agent-origin. This is not "emitted then dropped": across **all 1944** lines in `signal.jsonl` — **974 `start` / 970 `stop`** — **not one carries the `kind: "memory-candidate"` field** that `cli/src/memorycmds.rs:124` stamps on every candidate, and `rejects` is `0`, so no candidate signal was ever written to this store. `.claude/skills/agentrec-memory/SKILL.md` **is** installed, so the cause is that no agent session ever invoked it, not a missing install. | **Not closeable as PASS; closed FAILED, row retained — never counted as evidence for memory v1 real-world use.** A rerun requires all of: (a) a **fresh T0** and a fresh 7-day window; (b) a **clean baseline** — the 2 genuinely drifted facts re-pinned (`agentrec verify --confirm`) back to `3 fresh / 0 stale`; and (c) **first**, an end-to-end proof that the candidate path fires at all — one real agent session emitting `agentrec candidate` and the daemon ingesting it to an `origin: "agent"` record. Without (c), a rerun fails on conjunct 3 identically no matter how cleanly the clock runs. A hit-rate criterion also needs an instrument that records recall *attempts*, not only successes; as written it is unfalsifiable against this store. **Cascade / scope:** this failure does not close and does not alter the sibling memory rows — *"Memory: real Claude Code session shows injected block"* stays **OPEN** (independent env-gate) and the two perf-envelope rows stay CLOSED 2026-07-28. The `## Open (still gated …)` prose summary near the end of this file **was amended in this same commit** and now records "1-week memory dogfood + skill candidate" as **CLOSED FAILED 2026-07-31** rather than open; the two statements agree. |
| Memory: 1-week dogfood + skill-driven candidate — original gate text (superseded by the FAILED row above) | `status` counters (`memory: N fresh, M stale, R rejects, I injections`) and the candidate emitter + `agentrec-memory` SKILL.md are code-complete + tested; real-world hit-rate, no-false-staleness, and ≥1 useful skill-emitted memory need live use over a week. **BASELINE PREPPED (2026-07-17):** store reclaimed 3.4 GiB→775 MiB (orphan-GC, under budget, `status` clean), `agentrec-memory` SKILL installed to `.claude/skills/` (candidate emission now enabled — was staged-only), memory store re-pinned + seeded to 3 fresh / 0 stale (recall verified), `doctor` all-pass incl. hook-presence. The 1-week clock can now start honestly. **CLOCK STARTED T0 = 2026-07-18** (PR #5 merged to `main` at `8079299`; run on this repo's canonical `main`). **T0 baseline:** daemon live (launchd `com.agentrec.bfa6bde6eaa4`, RunAtLoad+KeepAlive, boots at login), `store 793 MiB` (under budget), `gaps 0`, `memory: 3 fresh / 0 stale / 0 rejects / 13 injections / 0 failures`, `rich-rate 60% trailing-20` (recovering as build churn quiets — honest-bare, `doctor` hook-presence pass), 3 seeded facts re-pinned to canonical `main` content after the rebase drifted `purgecmd.rs`. **Window closes ~2026-07-25.** Check at close: hit-rate non-trivial, DEGRADED/staleness never falsely fired (distinguish real file-drift staleness from false), and ≥1 `agentrec candidate` (SKILL-emitted) became a genuinely useful recalled memory. | One week recording in this repo with the SKILL installed: `status` shows a non-trivial hit-rate, DEGRADED/staleness never falsely fires, and ≥1 agent-emitted candidate becomes a genuinely useful recalled memory. |
| Store bloat over budget with 0 B freed (D36 dogfood observation) — **RESOLVED 2026-07-17, not a bug** | Root-caused: budget eviction (`retention::enforce_budget`) walks only turn-referenced snapshot blobs (0.74 GiB, under budget → "0 freed" correct); the 2.55 GiB was ORPHANED superseded intermediate snapshots the daemon `put`s for crash recovery, which nothing reclaimed. Fixed by shipping `purge --orphans` (branch `fix/purge-orphans-gc`, `c881cfe`; skeptic GATE PASS; 343 tests). Proven live: reclaimed 5835 blobs / 2.6 GiB, store→775 MiB, over-budget notice gone. | Closed here — verified on the real dogfood store. Future weeks: re-run `purge --orphans` (daemon stopped) when `status` flags orphan bloat. |
| D11 (service reload on re-init) | `service.rs`'s `install`/`uninstall`/`load`/`unload` shell out to real `launchctl`/`systemctl` and are, by this file's own long-standing design (see its module doc comment), deliberately never invoked from the automated suite — only `--no-service` paths are. The fix (launchd: `unload` best-effort then `load -w`; systemd: `daemon-reload` before `enable --now`) is implemented and reads correctly, but "re-`init` on an already-loaded service actually restarts it with the new unit content" needs a real macOS box with a previously-loaded `com.agentrec.<slug>` label, and a real Linux box with a previously-loaded `agentrec-<slug>.service`, to prove `launchctl load` no longer silently no-ops and `systemctl` actually re-reads the rewritten unit. | A manual run on both a macOS box and a Linux box: `agentrec init` twice in a row against a real (non-`--no-service`) repo, second run — confirm via `launchctl list \| grep com.agentrec` / `systemctl --user status agentrec-<slug>` that the service is loaded and its `ExecStart` matches the freshly written unit content. |
| Residuals round P4 — `wait_for_live_daemon` race-closure at the population level | `watcher_arm_stamp_keys_on_current_epoch_nonce` (unit) and `live_daemon_reports_watcher_armed` (integration) prove the MECHANISM: the daemon stamps `watcher_armed_nonce == epoch_nonce` only after `.watch()` succeeds, keyed on the current epoch's nonce so a crashed prior epoch's stale value self-invalidates, and `wait_for_live_daemon` now blocks on that condition instead of a bare `pid != 0`. Neither test — nor any fixture — can directly observe "no live-daemon test loses an event emitted in the ~4.3ms window between `acquire_lock` writing the pid and `.watch()` returning `Ok`" going forward, because that is an absence-of-a-flake claim across the whole suite's history, not a single assertion. | Measured over CI/local-run history: after this fix lands, zero live-daemon integration test failures attributable to "event emitted before the watcher armed" (as opposed to genuine FSEvents/inotify coalescing flake, already documented separately) across N subsequent full-suite runs. Not closeable by a single run; track failures of any live-daemon test (`live_daemon_reports_watcher_armed`, `doctor_healthy_all_pass_exit_0`, `status_suppresses_reload_line_after_daemon_crash`, `daemon_counts_ignore_rebuilds`, etc.) going forward and attribute root cause before counting one against this claim. |

## Phase 2.0 P1 — `import claude` fidelity report (spec decision 8's required row)

Recorded 2026-07-29 from the first real-corpus gate run of the built importer. Command:
`agentrec import claude --dry-run` (release build, `--source` defaulted to `~/.claude`), full
output captured at `docs/verify/p1-gate-run.txt`. **No fidelity threshold is asserted** — per
spec decision 8 the founder sets one from this first measurement.

**Importability predicate used for these numbers** (P1.md Pinned decision 3, recorded verbatim
so the figure's definition cannot float):
- *Denominator* = every `*.jsonl` directly under `~/.claude/projects/<project>/` (depth 1;
  never `subagents/`). Re-measured at run time — the corpus is a rolling ≤30-day window.
- *Numerator* = a denominator session that is not entirely sidechain-excluded AND completes
  classification without ever establishing a `cwd` on any line (AC8's loud path). Malformed
  lines alone do not disqualify a session; a session of only opaque calls still counts.

**THIS ROW WAS WRONG TWICE AND IS NOW ON ITS THIRD SET OF NUMBERS.** The figures below are the
corrected ones (gate run `docs/verify/p1-gate-run-t15fix2.txt`, 2026-07-29 evening). The two
superseded readings and exactly why each was wrong are recorded beneath the table — deleting
that history would hide the failure mode that produced it three times running.

| Figure | Measured 2026-07-29 (corrected) |
|---|---|
| sessions_total (denominator) | **1631** |
| sessions_importable | **1625 — 99.6%** (AC1 bar is ≥90%) |
| tier: T1 total | 856 entries — 39.6% of 2159 file entries |
| — of which inline `originalFile` (**yields real pre-edit bytes**) | **595 — 27.6%** |
| — of which `create` ops (no pre-edit bytes exist; correctly none) | 261 — 12.1% |
| tier: T1.5 (`file-history` blob, staleness-gated) | **90 — 4.2%** (1 structurally-inferred) |
| tier: T2-candidate (git-tracked, bytes NOT resolved in P1) | 897 — 41.5% |
| tier: T3 (no recoverable before) | 316 — 14.6% |
| T1.5 rejected — blob stale by an intervening edit | **170** |
| T1.5 rejected — `oldString` not found in blob | 3 |
| T1.5 rejected — unsafe path component / blob missing | 0 / 0 |
| peak RSS | **17.17 MB** (AC7 bar is <500 MB) |

**Honest reconstructible figure — read the two numbers separately.**
- **43.8%** (T1 856 + T1.5 90 = 946 of 2159) counting `create` ops as reconstructible, since a
  new file's correct `before` genuinely is "nothing".
- **31.7%** (595 + 90 = 685 of 2159) counting only entries that yield **actual pre-edit bytes**.

Quote whichever you mean and say which. Neither is 67.9% (the 2026-07-24 prediction) and
neither is 40.4% (this row's own superseded second reading).

**Accuracy of the surviving T1.5 entries — measured, not asserted.** Ground-truth channel:
entries that carry an inline `originalFile` (so the true pre-edit bytes are known) **and** also
resolve a file-history backup **and** pass the classifier's checks — compare the resolved blob
against `originalFile`. Result: **101 correct, 1 fabricated (1.0% residual)**, reproduced
independently by two parties. Before the staleness fix the same channel measured **29.6-30.3%
fabricated**. The single residual traces to an out-of-order snapshot line in one transcript
(`memory/MEMORY.md`, a concurrent writer invisible to a linear read) — recorded, not chased.

### Superseded reading #1 (first gate run): "T1.5 = 0"

Cause: the classifier gated backup harvesting on `type == "snapshot"` — a literal the
*synthetic fixture had invented*. The real corpus emits it on `type: "file-history-snapshot"`
(884 lines, zero under any other type). Fixed to presence-based harvesting.

### Superseded reading #2: "T1.5 = 11 (0.5%), genuinely near-empty, not under-detected"

That sentence was **false** and this row asserted it. Cause: the T1.5 lookup compared
`toolUseResult.filePath` (always absolute) against `trackedFileBackups` keys that are
**relative to the session `cwd`** in 1606 of 1883 cases — so the raw string compare almost
never matched. The existing fixture happened to use an absolute key, so every test passed and
an 8/8 skeptic gate cleared it.

Worse, this row cited "a standalone Python sweep of the corpus, written without reference to
the importer" as independent corroboration. That sweep **re-implemented the importer's own
raw-path assumption**. It was independent of the *author*, not of the *assumption* — which is
the only independence that mattered. Two rounds of review and a passing gate all missed it.

### What actually drove T1.5 down, and the correction to the correction

Fixing the path compare raised T1.5 to 210 — and *that* number was also wrong, in the opposite
and more dangerous direction. Claude Code writes file-history backups **at snapshot time, not
per edit**, so for the 2nd-and-later edit of a file the blob is the pre-*snapshot* state. The
`oldString`-containment guard still passed, because a later edit's `oldString` usually sits in
a region earlier edits didn't touch. 115 of those 210 were provably stale; the ground-truth
channel put the fabrication rate at ~30%. Import would have handed P2 fabricated pre-states to
persist as recoverable history — a direct violation of the never-fabricate invariant.

The correct predicate is **structural, not textual**: a blob is a valid pre-edit state only if
no edit to that same path intervened between the snapshot that recorded the backup and the edit
being classified. 170 entries fail that test and now fall through instead of resolving. T1.5's
honest share is **4.2%**, not 25.3% (predicted), not 0.5% (under-detected), not 9.7% (inflated
by fabrication).

**The standing lesson, now with three instances behind it:** every T1.5 fixture was a
single-snapshot / single-edit session, so no fixture could ever exercise version alignment —
and each bug was a case of *the fixture and the code agreeing on a shape the real corpus does
not have*. Fixture-only evidence cannot close a corpus-shape claim, and a cross-check that
re-implements the implementation's assumption is not a cross-check. Fixtures for the
multi-edit and redundant-announcement shapes now exist.

**Defect this row exists to record (found by the gate run, not by tests):** the first gate run
reported `t1_5 = 0`. Root cause — the importer gated backup harvesting on `type == "snapshot"`,
a literal the *synthetic fixture had invented*; the real corpus emits this bookkeeping on
`type: "file-history-snapshot"` (measured: 884 such lines, zero under any other type). Every
unit test passed against the fixture while no real T1.5 entry could ever resolve. Fixed by
harvesting on the **presence** of `snapshot.trackedFileBackups` rather than any `type` literal,
and the fixture was corrected to carry the real type value. This is the exact failure mode the
independent-fixture-authoring rule was meant to catch and did not — the fixture author and the
implementer were independent of each other, but both derived the type literal from the same
under-specified brief. Recorded as a standing lesson: **fixture-only evidence cannot close a
corpus-shape claim; the real-corpus run is the gate.**

**Closed by P2 (2026-07-30), not still open:** T2-candidate bytes were detected but never
resolved in P1 scope — the **897** candidates (corrected; an earlier reading of 963 is
superseded and must not be re-cited) were always a ceiling, never a proven recovery rate. P2
resolved git blobs and converted a small fraction to real recoveries, the rest to T3. See
the P2 row below for the measured outcome and its caveats.

### Anti-overclaim rider (added at the final skeptic gate — read before quoting any figure)

The 8/8 AC gate that produced this rider was passed against figures **since proven wrong** (see
the two superseded readings above) — treat "the skeptic reproduced every figure" as reproducing
what the code then did, not as validating the numbers. The misreadings it named all still hold:

1. **"99.6% importable" is an ingestion-without-loud-failure rate, not a recovery rate.** It
   must never be quoted bare. Context this table omitted: only ~**184 of 1631 sessions (11%)**
   contain any file-mutation entry at all — every tier-laddered entry lives in those — and some
   have zero T1/T1.5-recoverable entries. A session whose every entry is T3 still counts
   importable (correctly: it imports as provenance-only turns with `before: null`, the pinned
   semantics — import never fabricates a snapshot). The honest recoverable figure is **43.8%**
   of entries (**31.7%** if you count only entries yielding actual bytes), with a ceiling near
   85% only if P2 converts every T2 candidate — and P2 will not, since git holds committed
   states only. Quoted bare, "99.6% importable" will be heard as "99.6% recoverable."
1b. **The 85% ceiling needs the same ban 92.3% has.** 43.8 + 41.5 (T2-candidate) ≈ 85%, and it
   is an upper bound by exactly the argument that banned 92.3%: a mid-session intermediate edit
   was never committed, so its bytes are not in git at all. **Do not quote ~85% as a recovery
   rate.** It is the ceiling if every candidate resolved, which is known to be false.
2. **The opaque bucket is not "Bash/Task."** An earlier revision of this row labeled it so; the
   8618 actually include ~1434 `Read` results, ~430 string-form `toolUseResult`s (mostly
   errors), plus Grep/TodoWrite/AskUserQuestion. The count is honest; the old parenthetical was
   not, and `mean_opaque_share_pct` is therefore **not** an "unattributable-mutation share."
   Corrected above. Anyone setting a fidelity threshold off 13.25% must know this.
3. **AC7's Linux leg is arithmetic-tested, not platform-proven.** The `ru_maxrss` divisor
   selection is `#[cfg]`-gated; the unit test covers both divisors' math from one machine, but
   which constant Linux actually selects closes only on the CI Linux run — still blocked on the
   unopened `fix/perf-evidence-round` PR. Same for the release-only debug-seam guard test,
   which never runs in the default suite (the `strings` check is the real evidence there).
4. **This is one machine's 30-day window.** ~1631 sessions of one user's Claude Code habits.
   The 4-tier shares — especially T1.5 at 4.2% — are a property of this corpus and this Claude
   Code version's snapshot behavior. Not a population claim.
5. **Open, unverifiable by any channel available today:** the ~90 surviving T1.5 entries that
   have no inline `originalFile` cannot be checked against ground truth — by construction there
   is nothing to compare them to. An edit by another tool or a human between the snapshot and
   the recorded edit would silently invalidate one, and the transcript cannot see it. The only
   channel that would close this is P2 comparing a resolved `before` against the git blob at
   the session timestamp for the T1.5∩T2 overlap. Until then the 1.0% residual error rate is
   measured over the *checkable* subset only, not over all of T1.5.
6. **Pinned decision 14 (`cwd` is session-level, first line wins) is contradicted by the
   corpus** — 67 sessions carry more than one distinct `cwd`, usually a subdirectory move.
   Resolving each key against the `cwd` in effect at its own snapshot line finds more
   candidates at slightly better precision. The decision is marked non-re-litigable, so this
   was deliberately NOT changed and is escalated to the founder as an open question.
### AC1.3 reproduction — 10k recall-latency envelope (verbatim, 2026-07-28)

Deliberately not a committed script: this repo shipped two real defects inside
`scripts/linux-leg.sh` while it had never been executed, so the perf-evidence plan's
rejected list forbids adding another. Run these against a **throwaway root only** — never a
root any daemon is watching.

```bash
# 0. release binary, built INSIDE the worktree (a bare `cargo build --release` with the
#    session cwd elsewhere silently builds the wrong tree — this happened once here)
cd <worktree> && cargo build --release
strings target/release/agentrec | grep -c AGENTREC_TEST   # must print 0

# 1. throwaway root
BIN=<worktree>/target/release/agentrec
SCRATCH=$(mktemp -d)/ac13; mkdir -p "$SCRATCH"; cd "$SCRATCH"
git init -q . && "$BIN" init --no-service

# 2. seed 10,000 records, schema per integration.rs:6196
#    140 orphaned (RECALL_VERIFY_CAP+12, shared fact, missing pins)
#    + 60 fresh (real files, real sha256 pins) + 9800 filler.
#    Leg A = orphans first (they win the idx-ascending tiebreak -> walk caps out).
#    Leg B = fresh first (fresh wins -> injection succeeds).
python3 seed10k.py "$SCRATCH"        # leg A ordering
python3 seed10k_legB.py "$SCRATCH"   # leg B ordering

# 3. 120 real subprocess runs per leg
rm -f .agentrec/memory-stats.jsonl
for i in $(seq 1 120); do
  echo "{\"hook_event_name\":\"UserPromptSubmit\",\"prompt\":\"kraken telemetry batching\",\"session_id\":\"leg-$i\"}" \
    | "$BIN" hook claude >/dev/null 2>&1 || echo "NONZERO EXIT at $i"
done

# 4. read the figure out of the new P1 surface
"$BIN" memories --stats

# 5. leg C only: 8-way CPU load for the duration, as a margin check.
#    WARNING: `kill $LOADPIDS` does NOT reliably reap these — the subshells reparent to
#    pid 1. Verify with `ps -Ao pid,pcpu,command | awk '$2 > 50'` and kill by pid.
for c in $(seq 1 8); do (while :; do :; done) & done
```

The seeder, verbatim (one script; `--fresh-first` selects leg B's ordering). Pin hashes are
`"sha256:" + sha256(file_bytes).hexdigest()`, matching `memory::hash_pin` → `store::hash_bytes`.

```python
#!/usr/bin/env python3
import hashlib, json, os, sys
root = sys.argv[1]
fresh_first = "--fresh-first" in sys.argv
CAP, ORPH, FRESH, TOTAL = 128, 128 + 12, 60, 10_000
FILLER = TOTAL - ORPH - FRESH
FACT = "kraken telemetry batching"

def rec(i, kind, fact, path, h, ts):
    return json.dumps({"v": 1, "type": "memory", "id": f"{kind}{i}", "op": "assert",
                       "fact": fact, "pins": [{"path": path, "hash": h}],
                       "source_turns": [], "origin": "agent", "ts": ts})

def fresh_block(base):                      # real files, real hashes
    out = []
    for i in range(FRESH):
        rel, body = f"fresh{i}.rs", b"fn fresh() {}\n"
        open(os.path.join(root, rel), "wb").write(body)
        out.append(rec(i, "fresh", FACT, rel,
                       "sha256:" + hashlib.sha256(body).hexdigest(), base + i))
    return out

def orph_block(base):                       # pins point at paths that do not exist
    return [rec(i, "orph", FACT, f"missing{i}.rs", "sha256:" + f"{i:064}", base + i)
            for i in range(ORPH)]

lines = (fresh_block(1000) + orph_block(2000)) if fresh_first \
        else (orph_block(1000) + fresh_block(2000))

fb = b"fn filler() {}\n"                    # padding so idf does not collapse
open(os.path.join(root, "filler.rs"), "wb").write(fb)
fh = "sha256:" + hashlib.sha256(fb).hexdigest()
lines += [rec(i, "filler", f"unrelated subsystem note {i} about parsing and layout",
              "filler.rs", fh, 3000 + i) for i in range(FILLER)]

open(os.path.join(root, ".agentrec", "memory.jsonl"), "w").write("\n".join(lines) + "\n")
print(f"seeded {len(lines)} (fresh_first={fresh_first}) orph={ORPH} fresh={FRESH} filler={FILLER}")
```

### P2b ride-along — eviction-pass cost on the live dogfood store (2026-07-28)

Required by the plan's Phase 2b: the 10 ms/2006-turn read-verb figure in the rejected list was
measured for a different purpose and is not evidence about a store where eviction actually fires.
Measured here on the **real production store** (`~/Projects/agentrec`, **2064 turns**, **76 MB**
objects, 256 fanout dirs) using the release binary at this round's HEAD. `status`'s text path is
exactly `load_log → extra_protected_refs → plan_eviction` after P2b — the same sequence the daemon
tick runs — so timing the whole `status` process is a strict **upper bound** on the tick's cost
(it additionally pays process spawn, the orphan scan, and rendering).

**30 runs, all exit 0: p50 = 12.9 ms, p90 = 13.1 ms, max = 14.2 ms.** Object count unchanged at
256 across all 40 invocations taken that session — the zero-write property (AC2b.1) confirmed on
the real store, not only on a fixture. At ~14 ms worst case against a 10-minute `EVICT_INTERVAL`,
the pass occupies ~2×10⁻⁵ of the daemon's single-threaded loop; it is not a turn-closure risk at
this store size. **Bound:** one store, one machine, macOS/APFS; the walk is over `log.jsonl` turns
plus the object tree, so this figure grows with both and should be re-measured before assuming it
stays negligible on a materially larger store.

### Memory dogfood — measured at close (verbatim, 2026-07-31)

Evidence for the **FAILED** 1-week dogfood row above. Read-only verbs only, run against the
**live production dogfood store** (`~/Projects/agentrec`, daemon recording) — nothing was
recorded, re-pinned, purged or undone to produce these numbers. Binary provenance:
`./target/release/agentrec` → `agentrec 0.1.0`, mtime `2026-07-28T10:26:54`. There is no
`agentrec memory status` verb on this build and `memories --stats` errors here
(`unexpected argument '--stats'` — that surface postdates this binary), so the counters come
from `status`, which is where this row's criterion locates them.

```console
$ ./target/release/agentrec status
store:      83.9 MiB
turns:      2243 (agent turns; git activity hidden)
gaps:       0 recording gap(s)
rich-rate:  100% over trailing 20 agent turn(s)
memory:     1 fresh, 2 stale, 0 rejects, 628 injections, 0 failures

$ ./target/release/agentrec memories
01KXS4AE  stale     2026-07-17  store bloat is orphaned CAS blobs (superseded intermediate snapshots the daemon put()s every debounced batch for crash recovery); budget eviction only walks turn-referenced snapshots so 'over budget, 0 freed' is correct not a bug — purge --orphans is the only reclaim path  [pins: agentrec-core/src/retention.rs, cli/src/purgecmd.rs]
    agentrec-core/src/retention.rs: pinned sha256:5c63bde37eeb4f4b925c33c7e84ba01803f7a69947ed96a4208aff0791a7dd68 -> now sha256:e00e601e29828c0de8b9e18ae88a7b386d42323b333c7099e85a8881acd16389  (drifted in turn t_01KYSR5958KXWRJ2WRHHRXDCJV at yesterday)
    cli/src/purgecmd.rs: pinned sha256:3629b7e8760e73e6e299f1627bb5a267dc53e5a7dfcbcf22e2106df4bbf35023 -> now sha256:668fee2bed5890e3f898f17be038bad826c7d7606cfaf1a3c54b5850c02c4a48  (drifted in turn t_01KYJT06M1R4E2X6RAY8K8K6Y9 at 4d ago)
01KXS4AE  stale     2026-07-17  deleting CAS blobs by absence requires a COMPLETE ref-set: referenced_hashes raw-scans sha256: refs from log.jsonl+open.json+memory.jsonl (never load_log, which drops torn lines) so a blob cited only by an unparseable line is never mistaken for an orphan  [pins: cli/src/purgecmd.rs]
    cli/src/purgecmd.rs: pinned sha256:3629b7e8760e73e6e299f1627bb5a267dc53e5a7dfcbcf22e2106df4bbf35023 -> now sha256:668fee2bed5890e3f898f17be038bad826c7d7606cfaf1a3c54b5850c02c4a48  (drifted in turn t_01KYJT06M1R4E2X6RAY8K8K6Y9 at 4d ago)
01KXAWQM  fresh     2026-07-17  nightly torture seed rotation is wall-clock-derived, see AGENTREC_TORTURE_SEED  [pins: cli/tests/torture.rs]
```

The three derived claims in the row come from these read-only passes over the store — commands
verbatim and rerunnable, `cd`'d to the repo root:

```console
# conjunct 3, part 1 — origins. 0 agent-authored records, so no candidate exists to evaluate.
$ python3 -c "
import json,collections
c=collections.Counter(); ops=collections.Counter()
for l in open('.agentrec/memory.jsonl'):
    l=l.strip()
    if not l: continue
    try: r=json.loads(l)
    except: c['UNPARSEABLE']+=1; continue
    c[r.get('origin')]+=1; ops[r.get('op')]+=1
print('origin:',dict(c)); print('op:',dict(ops))
"
origin: {'human': 6}
op: {'assert': 3, 'reverify': 3}

# conjunct 3, part 2 — the emitter never fired. Every signal line has the same 7-key shape and
# none carries the `kind` key that cli/src/memorycmds.rs:124 stamps on a memory-candidate.
$ python3 -c "
import json,collections
k=collections.Counter()
for l in open('.agentrec/signal.jsonl'):
    l=l.strip()
    if not l: continue
    k[tuple(sorted(json.loads(l).keys()))]+=1
for kk,v in k.items(): print(v,kk)
"
1944 ('event', 'prompt', 'session', 'tool', 'transcript', 'ts', 'v')

$ python3 -c "
import json,collections
c=collections.Counter()
for l in open('.agentrec/signal.jsonl'):
    l=l.strip()
    if l: c[json.loads(l)['event']]+=1
print(dict(c))
"
{'stop': 970, 'start': 974}

# conjunct 1 — injection counter shape: successes only, cumulative since 2026-07-12.
$ python3 -c "
import json,collections
n=collections.Counter(); tot=0
for l in open('.agentrec/memory-stats.jsonl'):
    l=l.strip()
    if not l: continue
    n[json.loads(l).get('n')]+=1; tot+=1
print('lines',tot,'n-dist',dict(n))
"
lines 628 n-dist {1: 516, 3: 51, 2: 61}

$ python3 -c "
import json,datetime as d
T0=d.datetime(2026,7,18); TC=d.datetime(2026,7,26)
pre=inw=post=0
for l in open('.agentrec/memory-stats.jsonl'):
    l=l.strip()
    if not l: continue
    ts=d.datetime.fromtimestamp(json.loads(l)['ts']/1000)
    if ts<T0: pre+=1
    elif ts<TC: inw+=1
    else: post+=1
print('pre-T0(<07-18):',pre,' in-window(07-18..07-25):',inw,' post-window(>=07-26):',post)
"
pre-T0(<07-18): 4  in-window(07-18..07-25): 131  post-window(>=07-26): 494

$ python3 -c "
import json,datetime as d,collections
c=collections.Counter()
for l in open('.agentrec/memory-stats.jsonl'):
    l=l.strip()
    if l:
        ts=d.datetime.fromtimestamp(json.loads(l)['ts']/1000)
        c[ts.date().isoformat()]+=1
run=0
for k in sorted(c):
    run+=c[k]; print(k,c[k],'cum',run)
"
2026-07-12 1 cum 1
2026-07-17 3 cum 4
2026-07-18 21 cum 25
2026-07-24 25 cum 50
2026-07-25 85 cum 135
2026-07-26 184 cum 319
2026-07-27 45 cum 364
2026-07-28 30 cum 394
2026-07-29 37 cum 431
2026-07-30 124 cum 555
2026-07-31 74 cum 629

$ ls -d .claude/skills/agentrec-memory && ls .claude/skills/agentrec-memory
.claude/skills/agentrec-memory
SKILL.md
```

Notes on the numbers above, in order: 07-19…07-23 are **absent** from the per-day table, i.e.
five consecutive days inside the window with zero injections. The `lines 628` and the daily
`cum 629` differ by one because the counter is **live-monotonic** — one more injection landed
between the two reads, and both postdate the red-team's 626. The `n`-distribution and the
`{'stop': 970, 'start': 974}` split are counts of records, not of turns. **Every count here is a
snapshot of a store the daemon is still writing to and will read higher on a rerun** — re-running
the two loops minutes later returned `1949` signal lines and `lines 631` — so the load-bearing
claim is the *shape*, not the totals: the signal file still exhibits exactly one key-tuple and it
does not contain `kind`, and the injection counter still has no failure/attempt column to divide
by. Both hold at any snapshot; neither can be repaired by waiting.

## Phase 2.0 plan exit — hard-gate re-verification at the final commit (2026-07-31, `88c7e9b`)

Plan-exit item 1 requires the hard gate retired **at the plan's final commit**, not only at P1's,
with the denominator re-measured at run time (the corpus is a rolling ≤30-day window). Item 6
requires P1's zero-bytes dir-digest AC re-verified against that same real-corpus run. Both were
re-run here; the P1 row above is the earlier measurement and is **not** superseded — it is the
same instrument at a different point on a moving corpus, and the drift between them is the point.

Release build at `88c7e9b`. Command: `agentrec import claude --dry-run` (`--source` defaulted to
`~/.claude`). Full output: `docs/verify/plan-exit-gate-run.txt`. Importability predicate is
unchanged from the P1 row above (P1.md pinned decision 3) and is not restated here, so it cannot
drift between the two rows.

**Units differ by row and the table mixes them — read the Unit column before dividing anything.**
(Flagged by the plan-exit gate: the tier counts do **not** sum to the session denominator —
839+87+881+416 = 2223 file entries against 1937 sessions — because they count different things.
The tool's own output has the same shape; this column is the fix.)

| Figure | Unit | P1 gate run (2026-07-29) | Plan exit (2026-07-31) |
|---|---|---|---|
| sessions_total (denominator, re-measured) | sessions | 1631 | **1937** |
| sessions_importable | sessions | 1625 — 99.6% | **1930 — 99.6%** (bar is ≥90%) |
| tier: T1 | file entries | 856 | **839** |
| tier: T1.5 | file entries | 90 | **87** (of which `t15_unverified=1` — the tool's own field name; "structurally-inferred" elsewhere in this ledger is a gloss on that same field, not a second figure) |
| tier: T2-candidate | file entries | — | **881** |
| tier: T3 | file entries | — | **416** |
| opaque_calls | tool calls | — | **9558** |
| mean_opaque_share_pct | % per session, averaged | — | **10.91** |
| T1.5 rejected — blob stale by intervening edit | file entries | 170 | **170** |
| T1.5 rejected — unverifiable / blob missing / unsafe path | file entries | 3 / 0 / 0 | **3 / 1 / 0** |
| skipped_sidechain | sessions | — | **1320** |
| skipped malformed / non-UTF8 / io-error | lines | — | **0 / 0 / 0** |
| skipped_missing_field: cwd | lines | — | **7** (schema drift, loud on stderr; the affected sessions are *not* counted importable) |
| peak_rss_mb | MB | — | **16.56** |

**Verdict: the hard gate is RETIRED.** 99.6% ≥ the 90% bar, against a denominator re-measured at
run time, with per-tier and per-session opaque-share fidelity figures recorded — which is what
spec decision 8 requires and what a parse-only pass would not have satisfied.

**Zero-bytes (item 6): PASS.** Recursive digest over `.agentrec/` was byte-identical across the
run — `e682de84…` before and after.

### Honesty notes on these numbers — read before citing them

1. **The corpus moved *during this session*.** Two dry runs minutes apart measured
   `opaque_calls` 9554 then 9558, and `peak_rss_mb` 16.66 then 16.56. The source is this
   machine's live `~/.claude`, which the very session doing the verification is writing to. Every
   figure here is a **timestamped sample of a moving corpus**, not a repeatable constant; a re-run
   will differ and that is not a regression. Only the *ratio* (99.6%) is stable across the two
   runs and the two dates.
2. **T1 fell 856 → 839 and T1.5 fell 90 → 87 while the denominator rose 1631 → 1937.** This is
   the rolling ≤30-day window doing exactly what the spec says it does: old sessions with
   reconstructible pre-edit bytes aged out while newer sessions aged in. It is **not** a
   classifier regression, but nothing in this run *proves* that — the two runs share no pinned
   session set. Corpus decay is the reason "durable archive" stays embargoed.
3. **The first attempt at the zero-bytes check was confounded and would have read as a FAIL.**
   The scratch repo was created with `agentrec init`, which installs and starts a **live
   daemon**; the daemon then snapshotted the run's own redirected output file into `.agentrec`,
   changing the digest. The importer wrote nothing — the recorder did. The valid measurement uses
   a repo with a hand-written `.agentrec/config.toml` and no daemon. **Any future
   re-verification of this row must not run under a live recorder.** (The stray LaunchAgent from
   that first attempt was uninstalled; the production daemon on `~/Projects/agentrec` was never
   touched.)
4. **`sessions_in_root: 0`** — no session was attributed to a bare `~/.claude/projects` root, so
   the depth-1 denominator rule was not silently widened.

### Binding plan-exit gate — Fable skeptic, isolated worktree at `46e5bf0`: **GATE PASS (9/9)**

The plan mandates this round after the orchestrator's own verification. The skeptic re-derived
every checkbox independently rather than reading the verdicts, including **re-running the
importer against the real corpus itself** (daemon-free scratch repo, per the item-6 trap):
**1937/1944 = 99.6%** on its run vs this row's 1930/1937 = 99.6% — *different absolutes, same
ratio, hours apart*, which is honesty note 1 reproduced by an independent party rather than
merely asserted by the party that benefits from it. It also independently confirmed 615/0/2,
both clippy profiles + fmt clean, release-seam count 0 vs debug 6, the zero-write digest, the
single `has_gap_after` definition, and that the two manual founder-judgment claims carry **zero
attest events** — not self-attested.

Both substitutions were judged **defensible on their premises, verified in source, not accepted
on the orchestrator's word**: item 4's impossibility confirmed (every `enforce_budget` caller in
`cli/src` is inside `mod tests`; real eviction exists only at `daemon.rs:92 run_eviction_pass`),
and item 2's seven added keys confirmed to be **exactly** `RepositoryHealth`'s fields
(`view.rs:269-279`) — nothing smuggled in alongside the change AC-1 forces.

**Five findings, all non-blocking; three are corrections to this repo's own record and are fixed
in the commit that carries this paragraph** (an overstated "no longer exist in any form"
sentence, a too-generous characterization of `AGENTREC_CLAUDE_PROJECTS_DIR`, and this table's
unlabeled units). The two carried forward as debts:

- **`clm_75W2H9NC0YF3Q2GMHG6V2Q98NT`'s replay is weaker than its claim text.** It automates only
  the "exactly one modified golden" half; the **additive-only key property was verified by hand,
  and has no standing replay**. A future `status_json` key *removal* is caught only implicitly,
  by the golden test re-rendering. Adequate, not airtight — and stated here rather than left for
  a reader to assume the replay covers the whole sentence.
- **The three deterministic replays hardcode `/Users/ravichandrasekhar/Projects/agentrec-phase2`**,
  so they are checkout-coupled and will not replay from another clone or worktree.

Also noted, not fixed: `clm_16WKRQZSM5K8YV72EAJN0FHT7S`'s neuter *discriminates* (the skeptic
duplicated the fn on a scratch copy and the replay failed), but it counts *files* rather than
*definitions* — a second definition added inside `view.rs` itself would slip past it.

## Memory v1 — closed at the done-gate (skeptical-reviewer GATE PASS, 2026-07-12)

Verdict: **GATE PASS** (binding done-gate, opus skeptical-reviewer, round 2 after one loop-back). 269 tests, 0 failed; clippy `-D warnings` + fmt clean. Independent codex (gpt-5.6-terra) cross-review ran alongside and surfaced 2 real bugs the per-task reviews missed (equal-ts fold nondeterminism, crash-window dangling `source_turns`) — both fixed + re-verified before the gate.

| INV | Evidence (test — all in-tree, run green) |
|---|---|
| INV-M1 (no record without ≥1 valid in-root non-secret pin; rejects persist nothing) | `pin_path_rejections`, `candidate_rejects_counted_never_fabricated` (4 genuine rejects, memory.jsonl untouched, counter==4), torture `assert_memory_invariants` (every raw line parses, ≥1 pin, incl. post-kill-9). |
| INV-M2 (recall never returns drifted/stale) | `recall_never_returns_stale` (mutate-then-recall asserts *which* survives), `recall_verifies_only_top_candidates`, `recall_bounds_verification_on_stale_heavy_corpus` (cap returns fewer, never stale), CLI `recall_cli_fresh_only_and_json`, torture `op_recall` re-hashes returned pins→Fresh under chaos. |
| INV-M3 (secret never cleartext on disk, both write paths) | `secret_prompt_never_reaches_disk_in_cleartext` extended: `remember` + `candidate` paths, greps all 4 locations (signal.jsonl, log.jsonl+objects, memory.jsonl, memory-stats.jsonl) — raw key absent, `[redacted:` present, files non-empty (non-vacuous). |
| INV-M4 (hook exits 0 + within budget: corrupt/missing/uninit/concurrent-append; always appends start signal) | `hook_fail_open_and_budget` (corrupt/missing/uninit exit 0 + start-signal asserted; 3000-record store = exit-0 + 500ms wall fail-open-under-load; real block-injection anti-silent-degradation proven at a runner-speed-independent ~200-record store — the 50ms-at-3000+ envelope is timing-dependent, laddered above, not a CI gate), `hook_exits_zero_under_concurrent_memory_append` (live writer races 30 hook subprocs, exit 0 + start-signal every iter). 10k/50ms hard envelope = LADDERED (above). |
| INV-M5 (fold deterministic in any order) | `fold_latest_op_wins_any_order` (6 perms), `fold_equal_ts_retract_wins_any_order`, `fold_equal_ts_reverify_wins_over_assert_any_order`, `fold_equal_ts_same_op_reverify_deterministic_any_order` — total order via `(ts, op_rank, content_key)`. |
| memory_enabled kill-switch (spec §184: disables injection + ingestion) | `candidate_ingestion_disabled_when_memory_disabled` (live loop) + `replay_ingestion_disabled_when_memory_disabled` (replay path) — both gate ingestion; manual `remember` deliberately ungated. |
| purge --memories-retracted (first of the three sanctioned rewrite classes; see D48) | `purge_archives_only_expired_retracted_chains` (archive-fsync-before-rewrite atomic; survivors byte-identical; second-run no-op), `purge_memories_retracted_refuses_while_daemon_running` (daemon-liveness flock guard — no lost concurrent append). |
| crash-window source_turns (Task 7 engine reserve-id) | `dangling_source_turns_closed_by_pre_persist_journal_sync` (env-gated pause widens the sub-ms window, kill-9, restart, referenced turn id present in log.jsonl post-recovery). |

**Accepted v1 limitations (local single-user, no-network — PROTOCOL.md hard rule):** validate→hash TOCTOU in `ingest_candidate` (only a hash lands on disk, INV-M3 intact, self-corrects at recall); `open.json` recovery-journal is best-effort/non-fsynced under power-loss (pre-existing M1 posture; the crash-window fix delivered kill-9 ordering, its scope); `purge` vs concurrent manual `remember`/`verify`/`forget` (documented in README + purge help; archive-never-delete = no byte loss).

## Closed here

| AC | Evidence |
|---|---|
| **H++ / D36 (7-consecutive-green-nights torture streak) — CLOSED 2026-07-17** | 7 consecutive *scheduled* `nightly.yml` runs, all `success`, each with the wall-clock-derived per-night `AGENTREC_TORTURE_SEED` (macOS-14 + ubuntu-24.04 torture legs, logs uploaded → reproducible): 2026-07-11 [29145391987](https://github.com/ravi1395/agentrec/actions/runs/29145391987), 2026-07-12 [29185161534](https://github.com/ravi1395/agentrec/actions/runs/29185161534), 2026-07-13 [29235408668](https://github.com/ravi1395/agentrec/actions/runs/29235408668), 2026-07-14 [29316472654](https://github.com/ravi1395/agentrec/actions/runs/29316472654), 2026-07-15 [29399284382](https://github.com/ravi1395/agentrec/actions/runs/29399284382), 2026-07-16 [29481922234](https://github.com/ravi1395/agentrec/actions/runs/29481922234), 2026-07-17 [29564943664](https://github.com/ravi1395/agentrec/actions/runs/29564943664). 0 invariant violations across the streak. Last remaining M3 launch gate — closed by the clock, as designed. |
| Y+1 (macOS local-binary leg) | `install.sh` supports `AGENTREC_LOCAL_BINARY`/`--local` + `--sha256`. Ran: `AGENTREC_LOCAL_BINARY=./target/debug/agentrec sh install.sh --sha256 $(shasum -a 256 ./target/debug/agentrec \| cut -d' ' -f1)` → `checksum verified: ceebf69bc654e3f57601d1c04aee5986a481b7a01e152a463a7b216dd7c2f2a1`, installed to `~/.local/bin/agentrec`, `~/.local/bin/agentrec --version` → `agentrec 0.1.0`. Mismatch path also verified: `--sha256 deadbeef` → `error: checksum mismatch: expected deadbeef, got ceebf6…` and exit 1. |
| Y+1 (install.sh shell hygiene) | `shellcheck install.sh` → no output (clean), shellcheck 0.11.0. |
| **Y+1 (Ubuntu leg + macOS clean-box leg) — CLOSED** | `.github/workflows/release.yml`, triggered by tag `v0.1.0`: builds the real 4-target binary matrix (macOS arm64 on `macos-14`, macOS x86_64 on `macos-15-intel`, Linux x86_64/arm64 on `ubuntu-22.04`/`ubuntu-22.04-arm`), publishes a real GitHub Release, then runs the README's literal `curl -fsSL .../install.sh \| sh` on four **fresh, ephemeral CI runners** — `ubuntu-22.04`, `ubuntu-24.04`, `macos-14` (arm64), `macos-15-intel` (x86_64) — each ok clean-box (no prior agentrec toolchain), asserting `checksum verified:` and `agentrec --version` matching the tag. Run [29112445275](https://github.com/ravi1395/agentrec/actions/runs/29112445275): **all 9 jobs green** (4 build + publish + 4 verify-install). First attempt ([29111007141](https://github.com/ravi1395/agentrec/actions/runs/29111007141)) caught a real bug this leg exists to catch: the Linux x86_64 binary built on `ubuntu-24.04` (glibc 2.39) failed `GLIBC_2.39 not found` on `ubuntu-22.04` (glibc 2.35) — fixed by building on the older `ubuntu-22.04` baseline (forward-compatible), then v0.1.0 was deleted and recut clean. Also fixed in the same round: `install.sh`'s default `release_base` and README's `curl` line pointed at `github.com/agentrec/agentrec` (wrong org — repo is `ravi1395/agentrec`), which would have 404'd every real install. |
| **Y+2 (brew tap) — CLOSED** | Formula authored at `ravi1395/homebrew-agentrec` (`Formula/agentrec.rb`, `on_macos`/`on_linux` × `on_arm`/`on_intel`, sha256 pinned per asset — cross-checked against the release's own asset digests before pinning, all 4 matched). Real local verification: `brew tap ravi1395/agentrec https://github.com/ravi1395/homebrew-agentrec.git && brew install ravi1395/agentrec/agentrec` → succeeded, `agentrec --version` → `agentrec 0.1.0`, `brew test ravi1395/agentrec/agentrec` → passed. Tap + install uninstalled/untapped afterward to leave the dev machine clean. |
| **README blame GIF — CLOSED** | `docs/blame-demo.gif` — rendered with `vhs` (`docs/blame-demo.tape`) against a real fixture repo built by `docs/generate-blame-fixture.sh` (real `agentrec init` + `record` daemon + `hook` signals + real `blame` output, not staged text). Shows `agentrec blame src/auth.ts:42` then, after a real file edit, `agentrec blame src/auth.ts` with the `human-edited since` marker — matches the README's adjacent text examples. Embedded in README, "GIF pending" placeholder text removed. |
| **CI matrix (J3)** | GitHub Actions run [29098847491](https://github.com/ravi1395/agentrec/actions/runs/29098847491) on push to `main`: **all 4 jobs success** — `fmt + clippy` (ubuntu-24.04), `test (macos-14)`, `test (ubuntu-22.04)`, `test (ubuntu-24.04)`. `.github/workflows/ci.yml` runs `cargo fmt --check` + `cargo clippy -D warnings` + `cargo test --workspace` on the macOS-14 + Ubuntu-22.04/24.04 matrix per push/PR. Nightly torture workflow (`nightly.yml`) validated by dispatched run [29098977888](https://github.com/ravi1395/agentrec/actions/runs/29098977888): torture `--ignored` **green on both macos-14 + ubuntu-24.04**, `AGENTREC_TORTURE_SEED` unset → wall-clock-derived and **distinct per platform** (macOS `SEED=1783692826448782391`, Ubuntu `SEED=1783692826259150257`), each `800 ops … INV1+INV2 held on every undo` with 0 violations / 0 skips — proving the D36 per-night seed-variance vehicle (Ubuntu executed 5 undo-of-undo INV2 checks vs macOS 1). Seed printed + log-uploaded → any failing night is reproducible. |
| **I++1 (Linux perms)** | Closed by the same run's Ubuntu legs: `lock_file_sets_0600_regardless_of_create_mode` + `lock_dir_sets_0700` (`agentrec-core/src/perms.rs`, plain `#[test]`, not platform-gated) and `d37_daemon_writes_land_at_locked_permissions` (`cli/tests/integration.rs`) executed green on **ubuntu-22.04 + ubuntu-24.04** — the 0700/0600 assertion now proven on real Linux CI, not just macOS. |
| **Y++2 (inotify mode)** | New `inotify-low-watches` CI job (`.github/workflows/ci.yml`): `sudo sysctl -w fs.inotify.max_user_watches=1` on ubuntu-24.04, then `cargo test -p agentrec --test integration --all-features -- --ignored --exact doctor_inotify_low_watches_fails`. Run [29104565488](https://github.com/ravi1395/agentrec/actions/runs/29104565488): job **success** — `doctor`'s inotify-headroom check actually returned `fail` with the `max_user_watches` remedy and exit 1, on real induced-low Linux state (not just the pass-branch compile check the main matrix already ran). Full matrix (lint + macOS-14 + Ubuntu-22.04/24.04 + this job) all green in the same run. |
| PROTOCOL v0.2 published | Version marker + changelog line at top of PROTOCOL.md; added previously-undocumented wire fields `merges` (turn) and `baseline_unknown` (file entry) — both already emitted by `agentrec-core/src/record.rs` and `cli/src/daemon.rs` — additive within v0.2, cross-checked field-for-field against `record.rs`. DEGRADED/snapshot-failure state confirmed local-only (`cli/src/state.rs`) and explicitly excluded from the wire protocol rather than invented as a field. |
| README sections | `## Line-level blame` (leads the doc, GIF placeholder + real CLI-output examples matching `readcmds.rs` line/whole-file formats), `## How we try to break it` (D36 torture harness framing + local run command), `## Commands` (cross-checked against `cli/src/main.rs` subcommand/flag list — no invented flags), install section pointing at `install.sh` + `agentrec init`, Apache-2.0 license line. |

## Closed at the M3 final gate (opus-4.8-high, commands executed, output pasted)

Verdict: **SHIP** — all ACs verifiable on macOS PASS. 142 tests, 0 failed; clippy clean; zero orphan daemons.

| AC | Evidence |
|---|---|
| I5 / I6 (purge) | `purge_removes_expired_prompt_blob_keeps_shared`, `purge_all_prompts_*`, `purge_snapshots_before_date_respects_keepset` — keep-set dedup real (shared blob survives), `log` still renders excerpts after purge. |
| I+ (TTL / eviction) | `enforce_budget_evicts_oldest_snapshots_only` → `Evicted{count:1,bytes:8}`; oldest evicted, shared+newest kept; `status` over-budget notice. |
| I++ perms (D37) | Live daemon write path: `.agentrec`=0700; `log.jsonl`/`signal.jsonl`/`state.json`/blob all 0600. (Caught+fixed blocker: daemon files had been 0644.) |
| I++ scrub (D38) | `secret_prompt_never_reaches_disk_in_cleartext` greps signal.jsonl + log.jsonl + every blob; cleartext absent, `[redacted:` present. |
| Y+3 / Y+4 / Y+5 | Live: init prints `to reverse everything: agentrec uninstall`, re-run no-op; uninstall archives to `.agentrec.archived.<ts>` (nothing deleted); `--dry-run` touches nothing. Unit: `launchd_plist_shape`/`systemd_unit_shape`. |
| Y++ doctor | 8 doctor tests (daemon-down, hook missing/malformed, signal-stale, degraded, bad-perms, json, healthy); live `--json` `{"ok":false}` exit 1, healthy exit 0. |
| Z+2 / Z+3 / Z+4 | Golden relative-time buckets; `should_color` truth table + no ESC when piped/NO_COLOR; `--explain` glossary only for present terms. |
| H++ (one run) | 1200 ops, INV1+INV2 asserted every undo, 0 violations, 0 orphans; deterministic via `AGENTREC_TORTURE_SEED`. |

## Open (still gated — environment/time absent here)
_(see the Open table near the top.)_ **J3 (CI matrix), I++1 (Linux perms), Y++2 (inotify induced-low-watches), Y+1 (Ubuntu + macOS clean-box installer legs), Y+2 (brew tap), the README blame GIF, and — as of 2026-07-17 — the 7-night D36 streak all closed** — repo published at https://github.com/ravi1395/agentrec, v0.1.0 tagged and released, Actions matrices green. Every M3-era launch gate is now closed; the remaining open rows are the memory-v1 ladder (10k/50ms envelope CLOSED 2026-07-28; 1-week memory dogfood + skill candidate **CLOSED FAILED 2026-07-31** — see its row; real-session injected block still open) and D11 service-reload, in the tables above.

## Follow-ups from the final gate (accepted, non-blocking)
- **CONCERN #2 — FIXED:** the torture default seed was a fixed constant, so a nightly D36 run with an unset seed would repeat one interleaving 7× and never broaden INV2 coverage. `env_seed()` now derives from the wall clock when `AGENTREC_TORTURE_SEED` is unset (explicit seed still honored + printed for reproducibility). Verified: two unset-seed runs print different seeds.
- **CONCERN #1 (accepted):** the 1200-op run exercised INV2 (undo-of-undo byte-exact) on only 2/21 checkpoints; INV2 also has dedicated integration coverage. With the varying nightly seed (above), the 7-night streak will accumulate broader INV2 coverage — folded into the D36 launch-gate ladder row.
- **CONCERN #3 (accepted, cosmetic):** idempotent `init` re-run reprints "scaffolded"/"set 0700" lines though it redid no work (operation is genuinely idempotent — no hook dup). Message-only nicety, deferred.

---

## Phase 2.0 P2 + P3 — GATE PASS (2026-07-30, skeptic round 2 at `32ee9b7`)

Binding done-gate: a Fable skeptic in an isolated worktree, per-AC, after a FAILED round 1.
**All 13 ACs PASS** (8 P2 incl. the added AC5b, 5 P3). Verified on the merged tree:
`cargo test --workspace -- --test-threads=3` → **504 passed / 0 failed / 2 ignored**
(pre-P2/P3 baseline **460 / 0 / 1** measured at `c6bd069`); clippy `-D warnings` clean debug
**and** release; `fmt --check` exit 0; `strings target/release/agentrec` → 0 hits for both
debug seams (`AGENTREC_IMPORT_DEBUG_ENTRIES`, `AGENTREC_IMPORT_T2_ORACLE`).

Round 1 FAILED on two blockers, both since resolved — recorded because the first is the third
instance on this plan of the same defect class:

1. **Silent uncounted drop of 23.4% of file-producing entries.** The persist path's
   `strip_prefix(cwd).ok()?` discarded any entry whose `filePath` was not under the session's
   first `cwd` — no `FileEntry`, no counter, no stderr — while the adjacent comment asserted cwd
   "is lexically a prefix of `file_path` in every real transcript". Measured false: **507 of
   2,170** entries (24 mid-session cwd moves; 196 under the first cwd's *parent*). Dry-run
   tier-counted those same entries as reconstructible, so the two ladders disagreed on ~23% of
   the corpus. Fixed by a `skipped_out_of_cwd` counter (text + `--json`), a corrected comment,
   and `blocker1_out_of_cwd_entry_is_counted_not_silently_dropped`. **Countability was the
   requirement; recovery of the 507 was explicitly not authorized.** The figure now has three
   independent derivations that agree (round-1 analysis, the implementer's counter, the round-2
   skeptic's own release-binary run).
2. **AC5b's claim text was unattestable** — see the rider below.

### T2 resolution — the measured outcome, with its mandatory caveats

**17 T2 resolutions, n=1133 → 1.50%**, over the population the deployed guard actually serves
(entries with no inline `originalFile`, not a `create` op). Refusal breakdown: gate 1 (path
already edited earlier in the session) 853, `structuredPatch` line check 188, uniqueness 43,
other (not git / no commit / no `oldString`) 32.

**Two caveats travel with that figure, always:**

- **(a) Never render it as "897 → 17".** P1's 897 T2-candidates and this 1133 use different
  definitions; the denominators are not comparable and the arrow implies a conversion rate that
  was never measured.
- **(b) The oracle channel and the population channel are DISJOINT — overlap 0** — both
  coincidentally n=17. So **none of the 17 population resolutions is scorable for correctness**:
  the fabrication rate on the population T2 actually serves is unmeasured, and unmeasurable by
  this oracle. The 0/17 fabrication result below applies to a different 17.

A T2 resolution rate far below the 41.5% candidate share is **the honest outcome the plan
predicted, not a failure** — git holds committed states only, so a mid-session intermediate edit
was never committed at all. Gate 1 causes 75% of the loss and its refusals are structurally
correct (an agent editing one file repeatedly in a session is the normal case).

### Founder decision — both T2 gates retained (2026-07-30)

Measured trade-off over 1655 real transcripts: **gate 1 alone → 137 resolved / 5.1% fabricated /
130 correct**; **both gates → 17 resolved / 0% / 17 correct**. Gate 2 therefore discards **127 of
144 previously-correct resolutions (88%)** to remove 7 fabricated ones — a 7.6x recall cost.
The founder chose zero fabrication: a wrong `before` is a wrong-byte revert source in
user-visible undo history, and no per-entry "unverified" marker exists that would make shipping
one acceptable. Recorded so the cost is visible, not silent. An earlier code comment claimed the
discarded cases "were exactly the fabrication-prone ones" — **false, 60% of what gate 2 refuses
was correct**; the comment has been corrected to measured reality.

### Anti-overclaim rider — AC5b's bar was set wrong and cannot be met

**Never state that import's fabrication rate is "≤1%".** The bar was added mid-flight (by the
orchestrating agent, not the founder) without checking whether the sample size could support it.
It cannot: the oracle's guard-admitted channel is **n=17**, giving a one-sided Clopper-Pearson
95% upper bound of **16.2%**; establishing ≤1% needs ~299 clean samples and the entire verifiable
channel holds **228**. This is an impossibility result, not a "measure more later".

**The defensible statement, verbatim:** *0 fabrications observed in 17 guard-admitted real-corpus
samples (95% upper bound 16.2%). The ≤1% bar is not establishable on this corpus by this oracle;
the whole verifiable channel is 228 cases.*

Claim `clm_4KSWSEZXS894D2P0DC92ZHH8MA` carries the unmeetable "at most 1 percent" wording. It
stays **DECLARED and unattested, permanently, as the honesty record** — a bar was set, tested,
and found unmeetable. It was never self-attested and must not be. The founder re-declared the
criterion with the honest wording; the implemented ratchet is `mismatches == 0`, asserted (not
merely printed) in `ac5b_oracle_real_corpus_measurement`.

### Import honesty semantics established here

- **Derived bytes are marked, never presented as observed.** `after` is sometimes reconstructed
  by applying `oldString`→`newString` to a resolved `before`. Such bytes carry
  `FileEntry.after_synthesized`, `modified_cause` returns a derived-bytes reason *before* any
  later-turn or gap signal, and `diff` prints an explicit DERIVED notice. Before this marker
  existed, `undo` reported `modified since (human or external edit)` on files nothing had
  edited — fabricated attribution, and it trained users toward `--allow-modified`, the flag spec
  decision 6 says must never be auto-honored. Synthesized bytes never reach a working tree:
  `execute_revert` re-snapshots actual disk bytes first.
- **`status`'s rich-rate excludes imported turns** (founder decision, 2026-07-30). The metric
  warns "your hooks may be broken"; an imported turn is `rich` without any hook having fired, so
  a bulk import could flood the trailing-20 window and make a dead hook read 100% healthy. Git
  turns were already excluded. Protected by
  `status_rich_rate_still_reflects_broken_hooks_alongside_imported_turns` (20 imported + 10 bare
  → `0% over trailing 10`, warning still fires).
- **P1's figures re-measured and NOT stale** after the secret-path parity fix: fresh dry-run gave
  `skipped_secret_path: 0`, reproducing 99.6% importable (with its rider), t1 859, t1_5 90,
  t2_cand 898, t3 323, RSS 16.56 MB. The bans on **92.3%**, the **~85% ceiling**, and bare
  **99.6%** without its rider all stand.

### Goldens: two deliberate recaptures, each audited to the line

P3's goldens pin today's CLI bytes so P4's byte-equivalence claim becomes falsifiable. Their
**capture point is the integration commit, not the P3 branch** — the imported turn's
serialization only exists once P2 lands, and its rendering shifts again with P2's marker
(reasoning and the two rejected alternatives are recorded in `P3.md`). Recapture 1 (P2 merge):
exactly 4 lines — the `partial file list (imported)` marker on `log`/`log --all`/`log --explain`,
plus the two new keys on `log --json`. Recapture 2 (rich-rate fix): exactly 1 line in
`status.golden`. **P4 must not regenerate these to make extraction pass** — that is the ratchet.

### Non-blocking residuals carried forward (skeptic-accepted)

- **Latent duplicate-`sessionId` append** — `existing_ids` never gains ids appended during the
  current run. The shape is real, not hypothetical: the corpus holds one `sessionId` in two
  project dirs (a worktree-resumed session), inert today only because one copy is a 1-line
  cwd-less stub. Two in-root copies with file entries would append two turns sharing an id, and
  `diff`/`show`/`undo <id>` would error "ambiguous". One-line fix; **recommended before plan
  exit.**
- Persist path lacks session-level skip counters (no-cwd / canonicalize-fail / out-of-root
  sessions return empty silently) where dry-run counts them — posture parity gap, not AC-required.
- `skipped_out_of_cwd` is corpus-wide, so it counts entries in sessions wholly outside `--root`
  that would never import — overstates loss in the conservative direction.
- Imported turns store the raw `--root` string (e.g. `"root":"."`) where daemon records carry
  absolute paths. Cosmetic today (all verbs resolve from `--root`); take the `root_canon`
  one-liner in the next wire-touching round with a FORMAT-CHANGELOG entry. Golden-safe.
- Real `kill -9` mid-import remains **UNTESTED** — simulated at a deterministic interruption
  point. The mechanism argument was independently checked: `append_line_synced` is a single
  `write_all` + `sync_all`, so a SIGKILL cannot tear a line; torn-line duplication is
  power-loss-only. Idempotency is re-derived from `log.jsonl` ids, not `state.json` offsets.
- `UPDATE_GOLDEN=1` regeneration verified in round 1 but **permission-blocked** for the round-2
  skeptic, which confirmed that branch by code-read only. The RED half it proved itself: a
  one-char `fmt::SEP` mutation failed 13 of 27 goldens, restored `shasum`-identical.
- P3 AC2 has no failing-invocation golden for `log` — `log` takes no id, so the AC's
  "(unknown id)" form does not exist for it. Ruled inapplicable rather than unmet.
- Linux legs (the `ru_maxrss` divisor, two release-only `#[cfg(not(debug_assertions))]` tests)
  still close only on the unopened CI PR.

## Phase 2.0 P4 — `RepositoryView` extraction (agent round, pre-skeptic)

Branch `feat/phase-2-0-p4` off `feat/phase-2-0-substrate` @ `f19c17d`. Baseline re-measured on
clean substrate: **504 / 0 / 2** (P4.md's "410" and its "21 goldens" are both stale — goldens
are 27). Exit measured at ****533 / 0 / 2** (delta +29)**.

| AC | Verdict | Evidence |
|---|---|---|
| AC1 gap logic unified, not relocated | MET | `rg 'fn has_recording_gap\|fn has_gap_after\|fn count_gaps' cli/src` → 0. One primitive `view::recording_gaps` returns every uncovered interval tagged `Crash`/`Restart`/`TrailingStop`; the three callers are one-line filters over it. |
| AC2 lookup choke point moved | MET | `rg 'fn same_revert\|fn resolve_turn' cli/src` → 0. `resolve_turn` returns a typed `LookupError`; the CLI shim renders prose and holds no matching logic. `purgecmd` rewired to the core symbol. |
| AC3 goldens byte-identical | MET | 27/27 pass; `git diff` over `cli/tests/fixtures/golden` empty at every commit. |
| AC4 `health()` is a pure read | MET | `health_performs_no_writes_on_an_over_budget_store` asserts store bytes + `log.jsonl` length + `state.json` mtime unchanged. **Falsifiability proven**: re-inserting `enforce_budget` into `health()` fails it on the store-bytes assertion; restored → green. |
| AC5 human `status` still evicts | MET | `status_prints_over_budget_notice` unmodified (`git diff` on it empty) and passing. |
| AC6 cursor bound to query + ledger identity | MET | Cursor carries the last item's **id** plus a query fingerprint. Same-length `purge --log-duplicates`-shaped rewrite → `Stale` (the test asserts the rewrite really is same-length, or it proves nothing). Truncation → `Stale`. Different query → `QueryMismatch`. |
| AC7 unknown fields/types tolerated and counted | MET | `Ledger` carries `unknown_type_lines` + `unparsed_lines`, surfaced through `health()`. Blobs referenced only by an unknown-type record survive a real over-budget eviction. |

### Two defects caught in review, both fixed with the regression test that catches them

- **Cursor went spuriously `Stale` on a pure append.** The cursor resolved its id inside the
  *post-filter* list, so a retroactive merge (PROTOCOL §4 — append-only, documented engine
  behavior) absorbing an already-returned turn dropped it from `selected` and read as a rewrite.
  AC6 names exactly that case. Now resolved against the unfiltered ledger:
  `a_retroactive_merge_appended_after_a_cursor_is_not_treated_as_a_rewrite`, proven falsifying by
  reverting the fix.
- **`unknown_type_lines` counted malformed known records.** A `{"type":"turn"}` line missing
  required fields incremented the "a newer producer wrote a kind we predate" counter — a counter
  asserting a false fact about the corpus, the same defect class that blocked P2/P3 round 1.
  Now gated on the `type` *value* against `record::KNOWN_RECORD_TYPES`.

### Deliberate deviations (recorded, not silent)

- **`health(&self, budget: u64)`, not the contract's no-arg `health()`.** The budget stays
  injected for the same reason `status_report(root, budget)` already injects it (AC I+: an
  over-budget store is otherwise untestable without a real multi-GiB store), and it keeps
  `agentrec-core` free of the CLI's config surface. P5 consumes this signature.
- **`diff`/`blame`/`recall` are NOT implemented this phase.** No AC constrains them, and shipping
  an unexercised second interpretation path is how a byte-equivalence claim gets quietly broken.
  **This is P5's entry condition, not a free pass**: a `--json` serializer that reimplements diff
  or blame interpretation in `cli/src` reopens the seam P4 exists to close.
- **AC7 reading, stated so it is evaluated as written**: "counted" attaches to unknown record
  *types*; unknown *fields* on a known record are tolerated by serde and are correctly counted as
  neither unknown-type nor unparsed. A test pins that reading.
- **`RepositoryView::open` deliberately has no "not initialized" error.** An early cut returned
  one, which silently changed `agentrec status` in an uninitialized directory from a printed
  report to exit 1 — an unsanctioned behavior change (`log` there still prints "no turns
  recorded"). Caught by running the binary, not by a test; a test now pins the tolerant reading.

### Measured, not assumed

- **`status` latency is flat.** Routing `status` through the view initially made it parse
  `log.jsonl` twice: **13.4 ms → 20.6 ms** per invocation on a copy of this project's real
  2143-line / 2.3 MB dogfood log (50 warm runs, macOS release build). Fixed by giving
  `load_log` and `load_ledger` one shared per-line classifier (`record::parse_log_line`) and
  letting `status_report` read the ledger once: **6.4 ms vs 6.6 ms baseline**. The shared
  classifier is also a correctness win — the records a reader gets and the census of what it
  skipped can no longer disagree.

### Residuals

- Claims for P4 were declared **mid-phase, after the first commit landed**, not declare-first per
  AC. Recorded rather than backdated.
- Test delta is **+29**, not the task file's "+18" — the ladder there is stale, and the ratchet
  only tightens.

### Round 1 of the gate FAILED. Three defects, all real, each now fixed with the test that catches it

- **AC6 FAIL — a cursor's `after_id` is not identity.** Turn ids are NOT unique in real ledgers:
  a pre-fix daemon's orphan recovery re-appends a turn under its reserved id (the shape
  `same_revert` and `purge --log-duplicates` exist for), and the known import defect appends a
  second turn under a resumed `sessionId`. `list` resolved the cursor by FIRST match, so on
  ledger `[t_0, t_DUP, t_1, t_DUP, t_2]` a cursor minted at the second `t_DUP` re-delivered
  `t_1` and `t_DUP` after a **pure append** — silently, no `Stale`. That is AC6's growth clause
  violated verbatim. `Cursor` now carries `after_occurrence`; losing the named occurrence is
  `Stale`. Tests: `a_cursor_after_a_duplicated_id_resumes_at_the_right_occurrence`,
  `losing_the_named_occurrence_of_a_duplicated_id_is_stale`.
  **Note the shape: this is the second time this phase that binding to a "unique" identifier was
  wrong, and both times the ledger already documented the non-uniqueness.**
- **The signature defect, fourth instance — mine.** `record::parse_log_line` guarded on the
  `type` tag being a *string*, so `{"type": 5}` fell through to the legacy no-tag fallback and
  was coerced into a turn — beside a retained comment promising that a line carrying a `type`
  is never coerced. Pre-P4 `load_log` keyed on **presence**. An unsanctioned behavior change for
  every reader, caught by the gate probing the comment rather than reading it. Now presence-keyed;
  `a_non_string_type_tag_is_never_coerced_into_a_turn` pins it.
- **`limit: Some(0)` reported end-of-ledger** on a non-empty ledger. An empty page has no honest
  continuation (no record to sit after), so it is now refused with `CursorError::ZeroLimit`
  rather than answered with a lie a pager would act on.

### Skeptic's honest UNTESTED rows (round 1), carried forward

- The 504 baseline was not re-executed in the gate worktree (`git checkout` is forbidden there).
  Arithmetic is consistent and matches this ledger; it rests on the orchestrator's run.
- The latency figures need the live dogfood log the gate must not touch. Not an AC; UNTESTED by
  the gate, measured by the orchestrator.
- No release-mode **test** run (clippy/fmt were verified on release; the tests were not).

### Round 2: GATE PASS (all 7 ACs), with one residual found and NOT charged

Both mutations (the `health()` eviction re-insert and a `render_turn` perturbation) were re-run by
the skeptic on the fix commit, RED then green. Six further cursor attacks held: triple-duplicate
ids paged one at a time, growth appending another record under the cursor's own id, purge-collapse
of an occurrence *earlier* than the named one (→ `Stale`, correct: occurrence indices shift down
and re-anchoring would silently skip), truncation, and query mismatch. The parser fix was verified
by construction over five tag shapes, including that the legacy no-`type` C6 fallback still works.

- **Residual, recorded not charged — an id-preserving REORDER rewrite defeats the cursor.** Two
  same-id records with different content, reordered in place: page 2 re-delivers the seen one and
  never delivers the other, with no `Stale`. Not charged because the only sanctioned rewrite is
  `purge --log-duplicates`, which collapses and never reorders, and everything else is append-only
  — no cursor keyed on anything short of a full-record content hash could tell the two apart.
  **Revisit if MCP 2.2 ever pages a ledger exposed to hand edits.**
- The gate hit and corrected a multi-filter `cargo test` invocation that silently runs nothing —
  the same malformed-replay shape that permanently REFUTED two P1 claims. One TESTNAME per replay.

## D46 — service-unit leak: temp-root guard + orphan detection (2026-07-31)

Closes the founder-pending "40 orphaned `com.agentrec.*` LaunchAgents" entry's **product-defect**
half. It does **not** remove any plist from this machine — that stays founder-reserved, and the
40 units are all still installed and untouched (re-counted at 41 before and after this round's
full test run, including the run that exercises `init` under temp roots).

### What is automated, and what is not — stated narrowly

The honest scope line is **not** "`service.rs` is untested". This change adds no code to the
`launchctl`/`systemctl`-spawning path and calls none of it:

| Surface | Coverage |
|---|---|
| `parse_unit_root`, `scan_units`, `manual_remove_command` (D46 discovery half) | **Fully automated.** Filesystem + text only, spawns nothing. 8 unit tests in `service.rs`, both unit-file forms, on every platform (the parser dispatches on content, not host OS). |
| `service_decision`, `temp_prefixes`, `temp_skip_line` (D46 prevention half) | **Fully automated.** Pure; full flag×path matrix asserted in `initcmd.rs`, plus a real-binary leg in `integration.rs::service_leak_guard`. |
| `check_orphan_services` (`doctor`) | **Fully automated** against an injected fixture directory via the debug-only `AGENTREC_TEST_SERVICE_DIR` seam. Verified absent from the release binary: `strings target/release/agentrec \| grep -c AGENTREC_TEST_SERVICE_DIR` = **0** (debug = 1). |
| Production resolution of the REAL service directory | Not directly asserted (it reads `$HOME`/`$XDG_CONFIG_HOME`). Covered indirectly by the pre-existing `unit_path`/`config_home` tests, which now route through the same `service_dir` resolver, **and** by the real-corpus run below, which resolved and scanned the real directory. |
| `install`/`unload`/`load` (spawn `launchctl`/`systemctl`) | **Unchanged and still never invoked from an automated test** — see the existing "D11 (service reload on re-init)" open row above, which this round neither closes nor widens. |

### Real-corpus evidence (AC-S9) — not a fixture round-trip

A round-trip test proves the parser inverts *our own writer*; it cannot prove it reads the plists
actually installed here. This repo has been bitten four times by fixture-only evidence for a
corpus-shape claim, so the parser was run read-only over the real directory before the ACs were
declared met. `cargo test --bin agentrec real_corpus_unit_scan -- --ignored --nocapture`
(`#[ignore]`d by design — environment-coupled, never part of the hermetic suite), output archived
at `docs/verify/d46-real-corpus-unit-scan.txt`:

> `real-corpus scan of /Users/ravichandrasekhar/Library/LaunchAgents: 41 agentrec unit(s) — 41 parsed (1 live, 40 vanished-root), 0 unparseable`

**41/41 parsed, 40 vanished, 1 live, 0 unparseable** — independently reproducing the 40-of-41 figure
recorded in CLAUDE.md from a separate `plistlib` enumeration. The one live unit is
`com.agentrec.bfa6bde6eaa4` → `/Users/ravichandrasekhar/Projects/agentrec`, the real dogfood daemon.

**This claim pins a moving target and is declared `manual`, not `deterministic`** (`clm_5JMKH9FFCNA4T0EDSVD5M8B2C0`,
DECLARED — never self-attested). The corpus is *expected* to shrink to 1 once the founder reaps
the orphans; a deterministic replay asserting 40 would then go REFUTED for the right thing
happening. The probe itself asserts only the invariant that survives reaping: **zero unparseable
units**.

### Discriminating-neuter proofs (a green test is not evidence until the broken version reds)

| Neuter | Test that must red | Result |
|---|---|---|
| `service_decision` stops canonicalizing the root | `service_decision_skips_under_a_real_temp_dir` | **FAILED** (correct). This is the load-bearing one: on macOS `$TMPDIR` reads `/var/folders/…` while `resolve_root` stores `/private/var/folders/…`, so an uncanonicalized compare makes the guard silently never fire — it would have shipped looking correct. |
| `scan_units` folds `Unparseable` into `VanishedRoot` | `scan_units_classifies_live_vanished_and_unparseable_disjointly` | **FAILED** (correct) — a unit we could not read must never be reported as an orphan. |
| `check_orphan_services` returns `Check::fail` instead of `Check::advisory` | `orphaned_unit_is_advisory_not_fail` | **FAILED** (correct) — the exit-0 gate rail holds. |
| `"orphaned services"` dropped from `diagnose`'s hardcoded uninitialized-repo `n/a` list | `uninitialized_report_has_the_same_check_set_as_an_initialized_one` | **FAILED** (correct), diffing the two name sets — so the rail catches any FUTURE check that forgets the list too, not just this one. |

The full-init leg (`init_under_temp_root_still_does_everything_but_the_service`) was **deliberately
not neutered**: removing the guard makes that test install a real launchd unit, i.e. leak exactly
the thing being fixed. Only the pure decision function was neutered, and only the pure tests were
run under it.

### Suite hermeticity — fixed, not waived

`check_orphan_services` scans the user-global service directory, so every `doctor` invocation in
the integration suite would otherwise read whatever units happen to be installed on the machine
running the tests (41 here, 0 on a fresh runner). The check is advisory, so no assertion would have
flipped — but a test whose behavior depends on ambient user state is the exact defect class this
repo keeps charging, so it was pinned rather than reasoned away: `integration.rs`'s `agentrec()`
helper now sets `AGENTREC_TEST_SERVICE_DIR` to one process-wide empty tempdir. Verified both
directions against the real binary: with the fixture, `doctor` prints `orphaned services  pass`
with **no note**; without it, the same binary prints the 40-unit note and a runnable
`launchctl bootout … && rm` pair — which is also this round's end-to-end production evidence that
the check works on the real corpus, not only through unit tests.

### Suite

`cargo test --workspace -- --test-threads=3` → **635 passed / 0 failed / 3 ignored**
(baseline at the Phase 2.0 plan exit: 615 / 0 / 2 — **+19 tests, +1 ignored**, the new ignored one
being the real-corpus probe). clippy `-D warnings` + `fmt --check` clean on **debug and release**.

### Deliberately NOT built, with reasons

- **`service prune`** — the third candidate fix. Three facts, not a preference: it is the only
  piece here that would shell out to `launchctl`/`systemctl` (which `service.rs`'s module contract
  forbids the automated suite from exercising), it is destructive, and plist removal on this
  machine is an explicitly founder-reserved item. `doctor` prints the exact command pair instead,
  which closes the user-facing problem with zero untestable code.
- **Reaping the existing 40 units.** Founder's, unchanged. `doctor` now finds and prints them.

### The claim protocol caught a defect the four neuters did not

`claimd verify` REFUTED AC-S2's claim (`clm_4AFDDT3XCSFHKDZ06D926XJVNY`, exit 101) after the first
commit. It was right. `service_decision_matrix` and `service_decision_installs_for_an_ordinary_root`
used `env!("CARGO_MANIFEST_DIR")` as their non-temp control — carrying a comment calling it "a real,
non-temp path" — which holds only while the checkout is not itself under a temp prefix. `verify`
replays committed state from a checkout under `$TMPDIR`, where that path genuinely is temp, so
asserting `Install` was wrong there. Reproduced directly by `git clone` into `$TMPDIR`
(`SkipTemp("/private/var/folders/…/T")` vs expected `Install`), fixed with fixed non-temp paths
covering both predicate branches (`/usr` — canonicalization succeeds; a nonexistent absolute path —
canonicalization fails and falls back), and re-verified green from a fresh `$TMPDIR` checkout.

Two things worth recording. **Production code was never wrong** — the defect was entirely the tests'
choice of control path, and it would have bitten any CI runner building in a temp workdir. And
**AC-S4's test carried the identical defect while its claim verified CONFIRMED** — it happened to
run somewhere non-temp. A green verify is not evidence the test is environment-independent.

Per house rule the refuted claim is **not amended and no equivalent is re-declared**; AC-S2 now has
a passing, fixed test and no live claim. Recorded in CLAUDE.md § Founder-pending for a ruling,
same disposition as the two P1 claims refuted on malformed replays.

### The guard leaked a 42nd unit before it was tight enough — recorded, not buried

**This round's own test suite installed a real launchd unit** (`com.agentrec.c0bf764acce7` →
`/private/var/folders/…/T/.tmpdW8v92`, mtime 2026-07-31 10:57:05), i.e. the guard failed at exactly
the thing it exists to prevent. Caught by re-counting the plists after each suite run rather than
assuming; the count went 41 → 42.

Cause: the first version of `temp_prefixes` recognized macOS's per-user temp dir
(`/var/folders/<x>/<y>/T/`) **only** by reading `$TMPDIR`. That read is not reliable —
`std::env::set_var` on another thread races a concurrent `var()` (the data race that made
`set_var` unsafe in edition 2024), and this very test binary mutates env in `service.rs`'s
`ENV_LOCK` tests and `doctorcmd.rs`'s `with_service_dir`. One missed read left
`/private/var/folders/…` matching no prefix, so `service_decision` returned `Install` and `init`
wrote a permanent `KeepAlive` unit for a directory about to be deleted. The same miss also produced
one transient assertion failure (199/1) that passed on re-run — a flake that was a real signal.

The env-read was the wrong mechanism, not just unlucky: `$TMPDIR` is simply **unset** under
launchd and cron, so the guard would have silently no-opped there in production too. Fixed by
matching `/var/folders` as a STATIC prefix, so the common macOS case never reads an env var at
all. `$TMPDIR` is still consulted for non-default and non-macOS values. Pinned by
`macos_per_user_temp_is_recognized_without_reading_tmpdir`, neuter-proven (fifth neuter: dropping
the static prefix reds it), and confirmed by counting plists across **four** subsequent full runs —
41 held, then 42 held with zero further growth.

**The 42nd plist was not removed by the agent when this was written.** It was subsequently reaped
along with the other orphans on 2026-07-31, on the founder's explicit instruction — see
"Orphans reaped" below.

### Residual — the guard is preventive only, and only for future inits

`doctor`'s check is user-global, but the temp guard changes only what THIS binary installs from now
on. Any `agentrec` build predating this change still leaks a unit per temp-dir `init`. Nothing
detects or blocks that, and nothing here reaps what already exists.

### Orphans reaped (2026-07-31, founder-instructed) — and the detection half self-confirmed

The founder instructed removal, so the 40 were reaped. **39 by the agent in one pass; 2 had already
gone** (`com.agentrec.039364bb7dfe`, `com.agentrec.c0bf764acce7`) — inferred, not proven, to be the
founder running the two removal commands this session had printed, since those are exactly the two
labels that appeared in runnable form.

Method, in the order it ran, because a destructive pass with no rails is not evidence of care:

1. **Archive first** — all 40 plists copied to `ARCHIVE` (below) before anything was touched. Fully
   reversible, consistent with the repo's never-delete house rule.
2. **Removal list built from agentrec's own classifier**, not an ad-hoc `plistlib` script: the
   `VanishedRoot` bucket of `service::scan_units`, i.e. the code D46 shipped.
3. **Independently re-checked** — each listed root re-tested absent at removal time (a root that
   reappeared, e.g. a remounted volume, would have aborted the pass).
4. **Live unit asserted OUT of the list** (`grep -c bfa6bde6eaa4` = 0) and **every entry asserted
   present in the archive** before any `rm`.
5. `launchctl bootout gui/$(id -u)/<label>` then `rm` — **39 removed, 0 failures**.

Verified after: exactly one plist remains (`com.agentrec.bfa6bde6eaa4`), `launchctl list` shows one
agentrec job at **status 0** (before: 23 loaded orphans at status **78** — launchd respawning
recorders whose roots were gone), and the real dogfood daemon is untouched and still running as
pid 865 against `~/Projects/agentrec`.

**`doctor`'s orphaned-services check now prints a silent `pass` with no note** — the D46 detection
half confirming its own fix end-to-end against the real machine, having previously reported all 40.

- **ARCHIVE:** `/Users/ravichandrasekhar/agentrec-launchagents-archive-20260731-152414` (40 plists). Safe to delete once the founder is satisfied; nothing
  references it.

## D47 — declarative liveness: stop the respawn loop at the unit (2026-07-31)

Follows D46. D46 stopped `init` from *creating* units under temp roots; D47 stops a unit that
already exists from **respawn-looping once its baked path goes stale**. The two baked paths (root,
exec) are never re-validated at load time, and `KeepAlive true` turns one clean failure into
permanent noise.

### Measured on macOS 26.5 (Darwin 25.5.0), 2026-07-31 — three launchd probes

Run with throwaway labels (`local.pathstateprobe*`, deliberately NOT `com.agentrec.*`), plists in
a scratch dir, `launchctl bootout` in an EXIT trap. Residue re-checked after each: 0 probe jobs
left, `~/Library/LaunchAgents` back to exactly 1 agentrec unit (the live dogfood daemon).

| Probe | `KeepAlive` config | Result | Reading |
|---|---|---|---|
| 1 | `PathState {existing: true, missing: true}` | `runs = 3` in 30s, `state = spawn scheduled` | a present co-key defeats the gate (see the mechanism caveat below) |
| 2 | `PathState {missing: true}`, no explicit `RunAtLoad` | `runs = 0`, `state = not running` | single failing key suppresses the job entirely |
| 3 | `PathState {missing: true}` + explicit `RunAtLoad` | `runs = 1`, `state = not running` | `RunAtLoad` fires **once**, then every restart is blocked |

**Probe 1 refuted the design this round started from.** The plan specified
`PathState {root, exec}` on the assumption it ANDs. It does not: a `{root, exec}` pair would keep
the job alive whenever **either** path exists, so the exec gate would have been **inert in exactly
the case that motivates it** (root present, binary upgraded away) while reading as fixed. No AND
is reachable — `false` values invert individual conditions, they cannot negate whatever combines
them.

**Mechanism caveat, added by the D47 gate round (2026-07-31) after this section overstated it.**
`launchd.plist(5)`'s "If multiple keys are provided, launchd ORs them" is stated at the
`KeepAlive`-**dict** level, not inside `PathState`. Probe 1 does NOT uniquely establish
"`PathState` ORs its entries": it is equally consistent with "an absent path is disregarded while
a present co-key is satisfied". The two are operationally identical for this decision — under
either, a co-listed exec adds no gate — so root-only is correctly supported. **The OR label is
inference; the consequence is what was measured.** Do not quote "PathState ORs" as measured fact.

**Probe 3's observation window went unrecorded**, so "blocks every restart / one attempt per
login" is an extrapolation from ONE load cycle. It is supported by a real discriminating
observable — `state = not running` versus probe 1's `state = spawn scheduled`, which is how a
throttled pending respawn reads — but "every" is not established. To close: reload the probe-3
plist, wait ≥30s (3× the 10s default `ThrottleInterval`), confirm `runs` stays 1.

**Shipped:** `PathState` on the **root only**. Probe 3 is the shipped configuration's behavior —
one attempt per login, no loop. `launchd_pathstate_lists_only_the_root_never_the_exec` exists so a
later reader who "fixes" the omission reds instead of shipping an inert condition.

**Exec staleness is DETECTED, not prevented** — `doctor`'s second advisory clause (this round),
naming the exec and the re-init remedy. Mitigating context: `eeef849` already made the recorded
exec stable across `brew upgrade`, which was the motivating case. `ThrottleInterval` was
considered to bound exec-failure noise and **rejected**: it would equally delay legitimate crash
restarts, and a multi-minute recording gap is a worse defect than log noise in a tool whose
purpose is not missing activity.

### Open row — Linux, NOT measured

`systemd_unit` emits `ConditionPathIsDirectory=<root>`. The generator is unit-tested; the
**runtime effect is unverified** — written on a macOS host with no systemd to probe. Do not
upgrade to a proven claim without running it on Linux. Needed: load a new-posture unit, delete
the root, confirm the start job is skipped (unit inactive, not failed) and no restart churn
appears in `systemctl --user status`.

### Gate outcome (2026-07-31)

Binding fable skeptic in an isolated worktree (`~/.gate-d47`, detached at `4c58b07`):
**GATE PASS, 8/8 ACs.** It re-ran the full suite itself (644/0/3), executed 5 neuter/restore
cycles rather than trusting the round's reported ones, confirmed each redded on a WRONG VALUE
rather than absent output, and independently replayed the archive classification (39 temp-rooted
/ 1 live daemon). It verified the edited fixture `no_orphans_is_a_silent_pass` was a genuine
de-ambient-ing, not a weakening — the old fixture named an exec absent on this machine and would
fail under correct new code. Its two wording findings are folded in above; both were this
document's own overstatements.

**Residual risk it named, carried deliberately:** no generator-emitted `PathState` plist has ever
been loaded end-to-end — every probe plist was hand-written, and the hermetic-tests rule keeps
`install` out of the suite. The first `agentrec init` after merge (the dogfood rewrite below) IS
that end-to-end test; check `launchctl print gui/$(id -u)/com.agentrec.bfa6bde6eaa4` afterwards.

### Founder decision pending

The posture change means the **next `agentrec init` in `~/Projects/agentrec` rewrites and
reloads the live dogfood unit** (`service::install` rewrites on content difference). Harmless in
principle; flagged because that daemon is production evidence infrastructure.

## Phase A — Codex hook spike (2026-08-05, `feat/phase-2-tail` worktree)

Live-probe gate on `docs/superpowers/plans/2026-08-05-phase-2-tail-plan.md` Phase A, before any
Codex-emitter production code (Phase C) is written. Pinned Codex CLI: **`codex-cli 0.146.0`**
(`/opt/homebrew/bin/codex`, Homebrew cask, confirmed via both `codex --version` and `codex
doctor`). All work in a disposable scratch git repo under `scratchpad/`, isolated `CODEX_HOME`
(only `auth.json` copied), never the agentrec repo or the user's real `~/.codex`. Full writeup,
raw terminal transcripts, and field-inventory table: `docs/verify/codex-spike.md`. Redacted
fixtures for all three required events (+2 bonus continuation-pair fixtures):
`docs/fixtures/codex/*.json`, all `jq .`-valid.

**Execution-branch baseline (global constraint: re-measure at start, never trust a stale count):**
`cargo test --workspace -- --test-threads=3` on `feat/phase-2-tail` @ `64f2bcf` (forked from
`origin/main` @ `dd4238f`) → **751 passed / 0 failed / 3 ignored**. Matches `main`'s last
recorded figure (`fix/import-existing-ids-staleness` merge) — no drift since fork. This is the
number every later phase in this branch diffs against.

**Exit criteria 1–5 (plan Phase A), status:**
1. Pinned version recorded here + `INTEGRATIONS.md` (Codex CLI section) — **DONE**.
2. Redacted fixtures for all three events + field-inventory table — **DONE**
   (`docs/fixtures/codex/`, table in `docs/verify/codex-spike.md`).
3. Trust-flow writeup incl. hash-re-review on a changed command field AND a changed non-command
   field — **DONE**, both tested independently and live via the real `/hooks` TUI (driven through
   `tmux`, which — unlike a bare `expect` pty — answers the Ratatui terminal-capability queries
   the TUI blocks on at startup).
4. Continuation-semantics answers — **DONE**, see finding below.
5. This row — **DONE**.

**Headline finding — NOT a blocker.** The plan's scariest-unknown question (Phase A probe 5) was
whether `turn_id` is stable across a `Stop`-hook `decision:"block"` continuation, since decision
17's file-accumulator is keyed on `turn_id`. **Measured twice, independently: `turn_id` is
STABLE** across the block-continuation (identical value on both `Stop` firings), `stop_hook_active`
flips `false`→`true` exactly as documented, and `UserPromptSubmit` does **not** re-fire for the
synthetic continuation prompt. This is the opposite of the plan's feared outcome — the
pre-authorized founder-escalation contingency ("if `turn_id` proves UNSTABLE ... go back to the
founder before Phase C") is **not triggered**. Phase C's drain-by-`turn_id` keying is sound as
designed.

**Other confirmed-live findings feeding Phase B/C directly:**
- `PostToolUse` (`apply_patch`) `tool_input.command` is the raw patch-DSL text, not a structured
  file list — confirmed the path-extraction rule must regex `^\*\*\* (Add|Update|Delete) File:
  (.+)$` over that string. One firing = one `apply_patch` call, which can bundle several file
  operations in one patch (observed 3 ops in one firing) — the accumulator must union across
  potentially several `PostToolUse` firings per turn, not assume one-to-one.
- Trust is **per-hook**, keyed on the parsed hook definition (not per-file, not per-session), and
  **ANY** field change — command or non-command (`timeout` tested) — revokes trust for exactly
  that hook while leaving untouched sibling hooks trusted; reverting to a previously-approved
  definition silently restores trust with no re-review. C3's byte-stability requirement is
  therefore load-bearing exactly as the plan assumed.
- `hooks.json` + inline `[hooks]` in the same config layer **does** merge (not override) and
  **does** print the documented startup warning verbatim, both in `codex exec`'s plain transcript
  and inside the `/hooks` browser's "Issues" line.
- **Silent-skip gap, not in the docs:** `codex exec` (non-interactive) with an untrusted
  repo-local `hooks.json` and no `--dangerously-bypass-hook-trust` produces **zero** warning
  anywhere in stdout/stderr — the documented "prints a warning" behavior is TUI-only. CI/automation
  relying on stderr to notice an untrusted-hook config will see nothing; `--dangerously-bypass-
  hook-trust` (confirmed live, exact flag name matches docs) is the correct automation answer, not
  a fallback warning.
- `Stop` fires on normal turn end; does **not** fire on a `SIGINT` mid-tool-call interrupt of
  `codex exec` (`turn interrupted`, exit 1, no `Stop`); does **not** fire on `/clear` (the
  abandoned session gets no `Stop`, a fresh `session_id`/`turn_id` starts on the next prompt).
  `SubagentStop` not exercised (out of this round's effort budget).
- `Stop` genuinely rejects non-empty non-JSON stdout (`Stop Failed` in the transcript, session
  still completes normally) while **empty** stdout is treated as success — the emitter's "write
  nothing on success" design (C2) is confirmed correct for `Stop`, not merely assumed.
  `SubagentStop` was never exercised; whether it behaves the same is documented (not measured).

**Not probed, stated explicitly rather than inferred:** `PreToolUse`, `PermissionRequest`,
`SessionStart`/`SessionEnd` payload shapes (only `/clear`'s side effects were observed, not the
`SessionStart` event itself — no `SessionStart` hook was wired this round), `PreCompact`/
`PostCompact`, managed/enterprise/plugin-bundled hooks, Windows command variants, and interactive-
TUI Ctrl-C specifically (only process-level `SIGINT` against `codex exec` was tested for the
interrupt probe). None of these were required by the plan's exit criteria; listed so a later
reader doesn't assume silence means "confirmed absent."

Closes here — all 5 Phase A exit criteria met, no founder escalation triggered, Phase C's
decision-17 keying assumption is upheld by live measurement rather than inference.

## Task C4 — `agentrec import codex` (2026-08-05, `feat/phase-2-tail` worktree)

K-series: streams Codex's `<source>/sessions/YYYY/MM/DD/rollout-*.jsonl` corpus (default
`~/.codex`), same `--dry-run`/persist split as `import claude`. Files:
`cli/src/importcmd.rs` (`mod codex`, new), `cli/src/main.rs` (`ImportSource::Codex` arm — shared
file, only these lines are C4's), `cli/tests/import_codex.rs` (new, 14 tests, all green),
`cli/tests/fixtures/import/codex/*` (new, synthetic fixtures grounded in the corpus-shape
measurements below), `docs/fixtures/codex/rollout-add-sample-redacted.jsonl` (new — one genuine
redacted rollout excerpt, real Position.java `add`, used by
`real_redacted_rollout_dry_run_report_keys_match_claude` so at least one test is not
fixture-only evidence, per this repo's standing lesson).

**Real-corpus grounding (616 rollout files, this machine, measured 2026-08-05, commands behind
every number — this is what decided the design, not an inference from Codex's docs):**
- `session_meta.id`/`session_meta.cwd` present on line 1 of every file, `turn_context.cwd`
  cross-checked identical on 860/860 sampled lines (0 diffs) — `cwd` is genuinely session-level,
  no Claude-style "first line that carries one" ladder needed.
- File changes are NOT parsed from the `apply_patch` tool call's raw patch-DSL text (that text
  carries only diff hunks for updates, never a full file). Instead this importer keys off
  `event_msg` type `patch_apply_end` — a structured, Codex-verified per-file signal
  (`success: bool`, `changes: {<abs path>: {type, content?, unified_diff?, move_path?}}`).
  Measured: 726 `apply_patch` calls, 1421 `patch_apply_end` events, `success` true on 1421/1421
  observed (checked defensively anyway — never trust `changes` when `success != true`). 18 of 726
  real `apply_patch` calls (2.5%) verification-FAILED before any `patch_apply_end` fired at all
  (a completely different failure shape: `"apply_patch verification failed: ..."`, no exit code
  at all) — invisible to every counter in this importer (a coverage rider, see below), and pinned
  by the `failed_patch_apply_is_never_trusted` test using that exact real failure shape.
- `changes[path].type == "add"` AND `"delete"` both carry `content` — the FULL new file (add) or
  FULL pre-deletion file (delete), real/observed, never derived. Both map to T1 ("before or after
  bytes known with certainty from the transcript itself"). `changes[path].type == "update"` carries
  only `unified_diff` (hunks, real line numbers, never a full file either side) — **this importer
  does not attempt to reconstruct `update` before/after bytes** (advisor-directed descope: no AC
  requires it, and Codex's rollout format has no snapshot/backup mechanism analogous to Claude's
  `trackedFileBackups` that would let it be done without fabrication risk). `update` entries are
  persisted with `before: None`, `after: None`, `baseline_unknown: true` (the same field the live
  daemon uses for "existed before we could see it" — `agentrec-core/src/daemon.rs`/`view.rs`) —
  never dropped, since a "we know it changed but not to what" entry is still real information, and
  never silently equivalent to a null-content file.
- `t1_5` (and every `t15_*` sub-counter) is structurally unreachable, not merely measured
  zero — no code path increments it (no per-edit snapshot mechanism exists in this format).
  `skipped_sidechain` and `skipped_missing_field.tool_use_result` are likewise always 0 by
  construction (no analogous concept in Codex's rollout shape). Kept in the wire report purely
  for AC-C4's report-key-parity requirement — the shared `ImportReport`/`TierCounts` struct is
  reused verbatim (same struct = same JSON, not "keys happen to match").

**Real-corpus `--dry-run` run** (`cargo run --release -p agentrec -- import codex --dry-run --root
~/Projects/agentrec`, release binary, this machine, 2026-08-05):
```
sessions_total: 616
sessions_importable: 616 (100.0%)
sessions_in_root: 36
tier_counts: t1=382 t1_5=0 t2_candidate=1139 t3=431
opaque_calls: 15000
mean_opaque_share_pct: 68.32
skipped_secret_path: 0 / skipped_sidechain: 0 / skipped_malformed_line: 0
skipped_non_utf8_line: 0 / skipped_io_error: 0
skipped_missing_field: cwd=0 tool_use_result=0
peak_rss_mb: 31.59
```
**Coverage rider (Codex's analogue of P1's "99.6% is an ingestion rate, never a recovery rate" —
never quote the numbers above without it):** this importer only recovers `apply_patch`-driven
changes with a successful `patch_apply_end`. File mutations via `exec`/`exec_command`-family shell
calls (redirects, `sed -i`, heredocs — a real and common Codex pattern, not a hypothetical) are
completely invisible to it; `opaque_calls: 15000` on this corpus is dominated by exactly this
population, not idle/no-op tool calls. No threshold gate is attached to any of these figures per
plan decision 8 — report only, exactly as instructed.

**Test baseline:** 806 → **842 passed / 0 failed / 3 ignored** on `cargo test --workspace --
--test-threads=3` (parallel `feat/phase-2-tail` C3 work landed concurrently in the same run; the
14 new tests here are `cli/tests/import_codex.rs`, all green, run in isolation and as part of the
full suite). `cargo clippy --workspace --all-targets [--release] -- -D warnings` clean; `cargo
fmt --check` clean on `cli/src/importcmd.rs` and `cli/tests/import_codex.rs` specifically (ran
targeted `rustfmt`, not workspace-wide `cargo fmt`, to avoid touching C3's in-flight files —
residual fmt drift in `doctorcmd.rs`/`initcmd.rs`/`uninstallcmd.rs`/`integration.rs` is C3's, not
this task's). `cargo build --release --locked -p agentrec` succeeds; `AGENTREC_IMPORT_DEBUG_ENTRIES`
(the existing debug seam, reused rather than minting a Codex-specific one) absent from release
`strings`.

**Real defect found and fixed by testing against a real tempdir root (not by inspection):** the
first `resolve_codex_file_entry` implementation compared a raw transcript `abs_path` directly
against a canonicalized `--root` — on macOS, `tempfile::tempdir()` roots live behind
`/var/folders` -> `/private/var/folders` symlinks, so every persist test failed with
`skipped_out_of_root: 1`, `appended: 0`. Fixed by routing the path through `session_cwd`,
canonicalizing THAT, then rejoining — the exact same fix `import claude`'s own
`classify_and_resolve` doc comment already documents for the identical bug class. Caught by
`same_session_reimport_appends_zero` and `intra_run_duplicate_session_id_...` both failing with
`appended: 0` on a first run (should never happen when a session file legitimately touches its own
cwd) rather than by reasoning about the code.

**Turn-id derivation:** `hash(session_id:turn_index)`, identical formula to `import claude`
(duplicated as a tiny 4-line function, not shared across sibling modules — same posture as the
duplicated `default_source_for_home`, deliberate per "diff ∝ request" over threading `pub(super)`
visibility through tested Claude code). `session_id` = `session_meta.id` (first line, 616/616
files carry exactly one). `turn_index` = a local counter incremented on each `event_msg
user_message` boundary (mirrors Claude's `is_genuine_user_prompt` turn-boundary judgment call).
**No new id-derivation rule was invented and none was needed** — nothing about Codex's rollout
shape required deviating from the pinned formula.

**No new wire/protocol field was added.** `FileEntry`'s existing `baseline_unknown` field is used
for `update` entries exactly as its doc comment already describes it, not repurposed.

**Not done, disclosed rather than silently skipped:**
- `update` before/after byte reconstruction (see grounding notes above) — the highest-effort,
  least-mandated piece per the advisor consult that shaped this task's scope; not built.
- No fallback path when a `patch_apply_end` event never fires for an `apply_patch` call (verified
  or interrupted) — such an attempt is invisible to every counter, not merely uncounted-but-flagged.
- `Move to:`/`move_path` (rename) semantics: `move_path` was `null` on all 1952 real `changes`
  entries sampled — never exercised live, so this importer makes no attempt to represent a rename
  as anything other than independent per-path entries (which is what the `changes` map already
  gives it).
- Real corpus persist run: NOT executed against the live dogfood repos this machine's real Codex
  sessions are rooted in (`~/Projects/agentrec`, `~/Projects/sutra`, etc.) — that would write real
  turns into those repos' production `.agentrec/log.jsonl`, which this task has no mandate to do.
  Persist correctness against real corpus *shapes* is covered by 7 integration tests built from
  real-measured field structures (including the symlink defect above, only findable by actually
  running persist against a real filesystem); the `--dry-run` real-corpus run (this task's explicit
  verification step) covers the full 616-file real corpus.

## O5 — live two-tool session evidence (Phase C exit, 2026-08-05, `feat/phase-2-tail` worktree)

Manual, not-unit-testable honesty row per the plan's Phase C exit criterion. Full method, raw
(redacted) captures, and every command's actual output: `docs/verify/o5-two-tool-session.md`.
Binary: debug build (`target/debug/agentrec`, `agentrec 0.2.0`), NOT release — disclosed; this
round checks protocol/attribution correctness, not performance. Disposable scratch git repo
under this session's scratchpad, never the agentrec repo; isolated `CODEX_HOME` (`auth.json`
only), mirroring Phase A's isolation method. Pinned `codex-cli 0.146.0` (same pin as Phase A;
0.146.1 was available and deliberately not taken).

**Execution-branch baseline, re-measured at the start of this round:** `cargo test --workspace --
--test-threads=3` on `feat/phase-2-tail` @ `af3175a` → **855 passed / 0 failed / 3 ignored**.
Re-measured again after the doc edits below (no Rust files touched) — same **855 / 0 / 3**.

**Temp-root service guard (D46), verified live, not assumed:** `agentrec init --codex` on the
`/private/tmp/...` scratch root printed `skipped service install: root is under a temporary
directory`; `launchctl list` and `~/Library/LaunchAgents` were enumerated before this round
started and after every step — **exactly one unit throughout**, `com.agentrec.bfa6bde6eaa4` (the
pre-existing live dogfood daemon for `~/Projects/agentrec`, unrelated to this round). No plist
was ever written for the scratch repo; `--service` was never passed.

**Headline finding — BLOCKING product defect, confirmed live, reported here per the task's
instruction rather than fixed:** the installed Codex hook entry is the bare command `agentrec
hook codex`, no `--root` (`cli/src/initcmd.rs::CODEX_HOOK_COMMAND`; root then defaults to
`std::env::current_dir()` at `cli/src/main.rs:386-389`). **Codex's hook-process cwd tracks the
directory Codex was launched from, not the git repo root.** Measured directly with a `pwd -P` +
payload-`cwd` probe wrapper substituted for the real hook command (`codex exec
--dangerously-bypass-hook-trust`, sanctioned for this scripted leg per the plan), launched from
two positions on the same trusted scratch repo:
- From the repo root: `process_pwd` == `payload_cwd` == the repo root, on all three hook firings.
  Corroborated by a second, independent channel — the interactive TUI's own startup banner
  (`directory: /REDACTED/scratch-repo`) for a root-launch session (Part 2 of the writeup).
- From a tracked subdirectory (`repo/sub/`): `process_pwd` == `payload_cwd` == **the
  subdirectory**, on all three firings.

**Consequence, observed directly:** the subdirectory run left a **second, independent**
`sub/.agentrec/signal.jsonl` (`record.rs`'s append path creates parent dirs with no error) that a
daemon running `agentrec record --root <repo-root>` never tails — every Codex signal from a
session launched inside that subdirectory is silently and permanently invisible to the repo's
real recorder. No error, no warning, anywhere in the chain. **This needs a founder-level design
decision (most plausibly: `init --codex` bakes an explicit `--root <resolved-repo-root>` into the
installed hook command) — not decided or built here**, per the task's explicit instruction not to
attempt a redesign mid-verification.

**Real `/hooks` trust flow — live, via `tmux` (bare `expect` hangs on Ratatui's terminal-capability
queries, per the Phase A spike's method):** first launch in the scratch repo showed the same
"Hooks need review" gate the spike documented; "Review hooks" → `/hooks` browser showed all three
events `Installed=1 Active=0 Review=1`; `t` ("trust all") flipped every row to `Active=1`, Review
column cleared. No bypass flag on this leg.

**Cross-attribution — clean, jq-scoped (not substring grep, which the redacted scratchpad path's
own `claude-501` component would false-positive on — caught and avoided):**
- **Codex leg: LIVE**, real interactive session, now-trusted hooks, no bypass flag. Prompted to
  create `codex_leg.txt`; all three hooks fired for real; `signal.jsonl`'s `stop` line carries
  `files_written: [".../codex_leg.txt"]` and a stable `emitter_turn`.
- **Claude leg: NOT live.** Driven by invoking `agentrec hook claude` directly with realistic
  `UserPromptSubmit`/`Stop` JSON on stdin — exactly as Claude Code's own hooks would invoke it —
  plus a real `claude_leg.txt` write in between so the daemon's fs watcher observes a genuine
  mutation. Stated explicitly here because overstating this is exactly what this repo's history
  punishes: **this is a direct-invocation leg, not a live Claude Code process.**
- Resulting `log.jsonl`: one rich `codex` turn (prompt attached, `codex_leg.txt` mentioned nowhere
  but its own signal), one rich `claude` turn (prompt attached, `claude_leg.txt` mentioned nowhere
  but its own signal), plus two bare turns each holding exactly one file. `jq -c 'select(.tool==
  "codex") | .files' log.jsonl` → `[]`; `jq -c 'select(.tool=="claude") | .files' log.jsonl` →
  `[]`; `jq -c 'select(.grade=="bare") | .files[].path' log.jsonl` → `"codex_leg.txt"` then
  `"claude_leg.txt"`. **Zero cross-attribution**: `codex_leg.txt` never appears under
  `tool=="claude"`; `claude_leg.txt` never appears under `tool=="codex"`. `agentrec show <id>`
  confirms both rich turns render the correct `tool` and prompt excerpt. **Exact wire value:**
  the emitter writes `"tool":"claude"`, never `"claude-code"` — INTEGRATIONS.md's
  "`claude-code`-and-`codex` lines" phrasing is prose shorthand for the product, not a literal
  field; this row does not claim `"claude-code"` was observed on the wire.
- Legs run strictly sequentially (each `Stop` fully processed before the next `UserPromptSubmit`),
  distinct filenames per leg — deliberate, because an open start/stop bracket suppresses
  quiet-window closure and retroactively folds interim bare turns into the rich turn (PROTOCOL
  bracketing), so an interleaved run could manufacture cross-attribution from the test's own
  design rather than measure the real thing. **Only the sequential case was exercised; interleaved
  multi-tool sessions are the pre-existing, already-disclosed D6 misattribution risk (README
  threat model), not a new finding here.**

**Disclosed, not new — and two different mechanisms, not one:** both rich turns rendered
`files:[]`; each leg's actual file write landed in an immediately-following bare (unattributed)
turn instead. For `codex`, `files_written` WAS populated on the stop signal (confirmed above)
and this matches CLAUDE.md's own disclosure that the D6 `resolve_declared` tier ladder "landed
dark" — wired onto the wire, nothing yet consumes it at persist time. For `claude`,
`files_written` was **never populated at all**: `cmds.rs::hook`'s Stop-event `files_written`
block only runs when the payload carries a `transcript_path`, and this round's direct-invocation
Stop payload omitted one (a real Claude Code payload supplies it) — a different cause (this
test's own incomplete synthetic payload) producing the identical rendered symptom, not a second
instance of the same dark-wiring gap. Neither compromises the cross-attribution result above — a
file in an unattributed bare turn is a different failure than a file under the wrong tool's rich
turn, and the latter never happened, on either leg, for either reason.

**OPEN, not claimed as passed:** the Claude leg is direct-invocation, not live, per the task's
own acknowledged environment constraint (no nested interactive Claude Code session is possible
here) — the Codex leg IS live throughout, hooks trusted through the genuine un-bypassed `/hooks`
flow. Interleaved/concurrent two-tool sessions were not exercised. The subdirectory-launch cwd
measurement used `codex exec`, not the TUI (no non-interactive way to re-launch per position
without a fresh trust cycle each time) — recorded as a scope note in the writeup, since both
surfaces read cwd via the same `std::env::current_dir()` call in the same binary.

**Cleanup:** `tmux kill-session`; this round's own daemon process killed by PID. Pre-existing,
unrelated `agentrec record` processes found running against `/var/folders/.../T/.tmp*` roots
(started hours before this round, leftover from other work) were deliberately left untouched —
out of scope. `launchctl`/`~/Library/LaunchAgents` re-checked after cleanup: unchanged from the
Part-0 baseline.

Plan's O5 exit criterion: **DONE, with the two disclosures above carried forward as OPEN, not
silently closed** — the live/direct-invocation split on the Claude leg, and the confirmed
subdirectory-cwd defect now escalated rather than papered over.

### APPENDED 2026-08-05 — the subdirectory-cwd defect above is CLOSED

The finding above stands as written; this records its fix, it does not amend it. Founder
ruled (2026-08-05) on the escalation: resolve the hook's root by **walking up from cwd to the
nearest ancestor carrying `.agentrec/`**, git-style, for **both** emitters — explicitly chosen
over baking an absolute `--root` into the installed hook command, because a baked path is a
snapshot and this repo already carries the scar of that failure mode (the 39 orphaned
LaunchAgents, all from a stale baked `--root`). Walking up also survives repo moves/renames.
`hook claude` was fixed alongside `hook codex` although its defect was latent, not observed:
it shares the identical bare-command shape and only escapes the bug because Claude Code
happens to set cwd to the project root — an undocumented behavior worth not depending on.

Fix commit: see `main.rs::resolve_hook_root` / `discover_agentrec_root`. Scoped to the `hook`
verb alone; every other subcommand's root resolution is byte-identical to before. An explicit
`--root` still wins outright — discovery is the default for an ABSENT root, never an override
of a supplied one. Discovery failure falls back to cwd rather than erroring, deliberately:
INV-M4 pins the hook path as fail-open (always append, always exit 0), and cwd is exactly what
happened unconditionally before this change.

**Re-measured, same method that found it** (debug binary, scratch repo, hook invoked two
levels below the root with a committed fixture on stdin, no `--root`):

```
--- cwd for hook invocation: /private/var/folders/.../T/tmp.XXXX/sub/deeper
--- exit=0
--- sub/.agentrec exists? (defect shape; must be NO)
NO
deeper: NO
--- root signal.jsonl:
       1
{"v":1,"ts":...,"tool":"codex","event":"start","session":"019fd1b4-...
```

**Mutation-probed in both directions**, not merely observed green: neutering
`resolve_hook_root` back to `cwd.to_path_buf()` (the pre-fix behavior), rebuilding, and
re-running reds EXACTLY the two subdirectory regression tests — one per emitter
(`hook_root_discovery::hook_codex_from_subdirectory_finds_root_and_creates_no_nested_agentrec`,
`…hook_claude_…`) — while the root-itself, explicit-`--root`, and fail-open legs stay green.
Restoring returns all 5 to green. The absence of `sub/.agentrec/` is asserted explicitly in
both tests; that absence IS the defect being closed.

Suite 855 → 866/0/3. clippy `-D warnings` clean debug+release, fmt clean, release build
`--locked`, 0 test seams in release `strings` — all confirmed by real exit codes, after an
initial `| tail` pipeline masked a genuine `clippy::question_mark` failure (this repo's own
recorded "`fmt --check | tail` exit-code trap", hit again; the lint was real and is fixed).

**Still OPEN, unchanged by this fix:** the Claude leg's live/direct-invocation split above.
Nothing here converts that simulated leg into a live one.

### APPENDED 2026-08-05 (later same day) — the Claude leg IS now live; gap CLOSED

Full method, raw captures, every command's actual output:
`docs/verify/o5-two-tool-session.md`'s second section (below the original "Cleanup performed").
Worktree unchanged at `3c4f598`; no Rust touched. Debug binary, same choice as every prior round.

**Both legs live in one session, one `log.jsonl`, sequential not simultaneous:** `claude -p
"Create a file named claude_leg.txt..." --allowedTools "Write"` ran a real Claude Code process
against the scratch repo (`.claude/settings.local.json` installed by `init`, debug binary
resolved via `PATH`); `-p`'s own help text confirms the trust dialog is skipped non-interactively
and settings still load. Codex leg **also re-driven live** (not required by the task, done
anyway) through the real `/hooks` TUI trust flow in the same repo, same method as the first O5
round. **Cross-attribution: zero, jq-scoped, both directions** — `codex_leg.txt` never appears
under `tool=="claude"`, `claude_leg.txt` never appears under `tool=="codex"`.

**§4 wire path observed for the first time with a real `transcript_path`:** the live claude
Stop signal carried both a genuine `transcript` field (verified: 85,068-byte, 25-line real
transcript file) and a populated `files_written: ["claude_leg.txt"]` — the prior round's
direct-invocation payload had omitted `transcript_path` entirely, so this exact wire shape had
never been observed before in a two-tool context.

**Correction to the prior round's diagnosis, proven from source + controlled repro, not
asserted:** neither "D6 dark-wiring" (codex) nor "missing `transcript_path`" (claude) is what
caused the prior round's `files:[]` on both rich turns. `agentrec-core/src/engine.rs::
observe_changes` appends changes onto the open turn directly, independent of D6; `cli/src/
daemon.rs` discards D6's resolution unconditionally (`let _declared = ...; // resolved but not
yet consumed`) — confirmed live here, since this round's `files_written`-carrying, transcript-
carrying claude Stop STILL required the (older, non-D6) watcher path to populate `turn.files`.
A controlled repro (fresh scratch repos, the prior round's own direct-invocation method)
reproduces the exact symptom on demand: a write immediately followed by Stop closes a
**zero-duration bracket** (`started == ended` to the millisecond) with `files:[]`, the write
landing in a separate bare turn instead; inserting a 2.5s gap between write and Stop closes a
~977ms bracket that correctly captures the file. **The mechanism is fs-watcher/bracket timing,
not D6 and not `transcript_path`** — both facts the prior round cited were true in isolation,
neither was the actual cause. **Not claimed:** that the prior round's own live Codex leg lost
this specific race — no write-to-Stop timing was recorded for that run, so it is undiagnosable
now; only that the identical symptom is reproducible from timing alone. This is a **new,
previously-undocumented, reproducible defect surface** (a fast-enough bracket can silently
misattribute its own write to an orphaned bare turn), distinct from D6's phase-3 plan — recorded
for founder disposition, not fixed here (out of this round's scope).

**Second measurement, independent of the above — done twice, because the first attempt didn't
measure the right process:** `3c4f598`'s commit message asserted `hook claude`'s pre-fix defect
was latent because "Claude Code happens to set cwd to the project root" — an unverified
assumption. A first live `claude -p` session from `<repo>/sub/` had the session's own **Bash
tool** run `pwd` into a file — that measures the Bash tool subprocess's cwd, not necessarily the
separate hook subprocess's cwd, so it was non-discriminating (a self-caught gap, not found by a
second party). Redone with the Codex leg's own method: a wrapper substituted for the installed
hook command itself, capturing `pwd -P` from inside the hook subprocess (plus the payload's own
`"cwd"` JSON field as a second channel), daemon running throughout. **Both channels agree, both
firings: the hook subprocess's own cwd was the subdirectory when launched from the
subdirectory.** The assumption in `3c4f598` is false as stated; `resolve_hook_root` correctly
discovered the root from a subdirectory for the **claude** emitter too — the fix earns its keep
for claude for a now-properly-measured reason. Full corrected method + raw capture:
`docs/verify/o5-two-tool-session.md` § "Correction to the subdirectory-cwd measurement above".

**One honest, undiagnosed anomaly, not swept:** after a daemon restart, `state.json`'s
`signal_offset` advanced to consume the subdirectory session's `start`/`stop` signals in full,
but **no turn record for that session appears anywhere in `log.jsonl`** — zero occurrences of
its session id or its file. Source tracing found no obvious filter that would drop a matched
bracket's Stop turn, and no error was logged. **Cause not established**; candidate explanations
are listed in the writeup and explicitly not asserted as the mechanism, to avoid repeating this
document's own signature defect.

**Suite:** `cargo test --workspace -- --test-threads=3` @ `3c4f598` (unchanged, no Rust touched),
summed directly from all 14 `test result:` lines (not through `| tail`): **866 passed / 0 failed
/ 3 ignored** — matches `3c4f598`'s own baseline exactly.

**launchd: exactly one unit throughout** (`com.agentrec.bfa6bde6eaa4`, pre-existing dogfood
daemon), before and after — no plist written for any scratch repo this round touched (main
two-tool repo, `timing-fast`, `timing-slow`). All daemons this round started were killed by PID
and confirmed stopped; the four pre-existing unrelated `/var/folders/.../T/.tmp*` recorders from
other work were left untouched, per the first round's own precedent. Scratch repos removed after
evidence was captured verbatim into the writeup.

**Plan's O5 exit criterion: DONE, live, both legs, one `log.jsonl`, zero cross-attribution — the
gap this repo disclosed (Claude leg not live) is closed.** Carried forward as genuinely OPEN:
interleaved/concurrent two-tool sessions (pre-existing D6 risk); the newly-found bracket-timing
race (new); the zero-turn-after-offline-bracket anomaly (new, undiagnosed). None fixed —
verification only, per this round's scope.

### FOUNDER WAIVER 2026-08-05 — claimd claims not declared for Phase C

Recorded as a debt, not silently closed. The Phase 2 tail plan's executor protocol requires
declaring a claimd claim per AC *before* implementing, plus one covering each normative-doc
edit (C1 edited `PROTOCOL.md` §4). **This was not done for any of Phase C** — C1, C2, C3, C4,
the §4 edit, or the O5 hook-root fix. Two successive skeptic gates found the gap; it is
unchanged between them. Measured, not asserted: the worktree has no `.claims/` at all, and the
main repo's `.claims/claims.jsonl` carries zero claim statements mentioning
codex/`emitter_turn`/hook-root/O5 and zero events dated 2026-08-05 — the date of every Phase C
commit.

**Founder ruling (2026-08-05): waived, proceed.** Phase C ships without claims, carried as a
coverage debt in the same shape as this repo's existing touched-uncovered rows
(`cli/src/cmds.rs` from the P3 residuals round; `cli/src/purgecmd.rs` from the honesty round).
Retroactive declaration was offered and **declined** on the founder's own standing rule that
it is refused by design — a claim declared after its evidence exists is a weaker artifact than
one declared before, and an agent minting claims for work it just finished is exactly the
self-attestation the protocol exists to prevent.

Scope of the debt, stated so it cannot be read as narrower than it is: every Phase C
acceptance criterion rests on the automated suite plus two independent adversarial gate rounds
(both of which re-ran the load-bearing mutation probes themselves rather than trusting the
implementers), and on **no** claimd replay. Nothing here is claim-attested. A future round
that wants claim coverage over this surface must declare fresh claims against live code, not
backfill these.

### CORRECTION 2026-08-05 — `3c4f598`'s commit message asserted an unmeasured live fact, now falsified

Recorded because this repo's signature defect is exactly this shape, and this instance was
written by the orchestrating agent into a commit message that cannot be amended.

`3c4f598` (the hook-root walk-up fix) justified covering `hook claude` with:

> "`hook claude`'s defect was **latent rather than observed**: identical bare-command shape,
> escaping the bug **only because Claude Code happens to set cwd to the project root**, which
> is an undocumented behavior not worth depending on silently."

The bolded clause was never measured. It was inference presented as fact, in the same commit
that fixed a defect found by refusing to accept exactly that kind of inference.

**It is false.** The O5 Claude-leg round measured it directly, using the Codex leg's own
wrapper-substitution method (substitute a wrapper for the installed hook command; capture the
hook subprocess's real `pwd -P`, plus the payload's own `"cwd"` JSON field as an independent
second channel). Launched from a subdirectory, both channels agree on both firings:

```
hook_process_pwd=/REDACTED/repo/sub
payload={"...","cwd":"/REDACTED/repo/sub","hook_event_name":"UserPromptSubmit",...}
hook_process_pwd=/REDACTED/repo/sub
payload={"...","cwd":"/REDACTED/repo/sub","hook_event_name":"Stop",...}
```

Claude Code does **not** pin hook cwd to the project root; it inherits the launch directory,
same as Codex. So the `hook claude` defect was **real and live, not latent** — a user running
`claude` from a subdirectory of an agentrec repo was losing signals silently, exactly as Codex
users were. The fix in `3c4f598` is correct and is load-bearing for both emitters; only its
stated *reason* for covering claude was wrong, and it was wrong in the direction that
understated the bug.

Worth keeping for the pattern: the first attempt at this measurement was itself
non-discriminating (it measured Claude Code's *Bash-tool* cwd, not the hook subprocess's), and
was caught in review rather than by the agent that ran it — corrected in `1b34f5b`. Two
successive measurements of the same fact, the first wrong, before the record was right.

### CORRECTION 2026-08-05 (b) — conflicting provenance for the non-discriminating-probe catch

A gate round found three mutually incompatible statements, across three gated commits, about
who caught the first (non-discriminating) Claude hook-cwd probe:

- `1b34f5b` commit message: "Caught before being relied on further **(not by a second party)**"
- this ledger, above (`### ... (a)` section's neighbourhood, the O5 Claude-leg row): "**a
  self-caught gap, not found by a second party**"
- `ecd09bc`'s appendix: "was **caught in review rather than by the agent that ran it**"
- `docs/verify/o5-two-tool-session.md`: "it was caught **(not by this round)**"

"Self-caught, not by a second party" and "caught in review rather than by the agent that ran
it" are direct negations. At least one gated commit carries a false sentence. This matters
because this repo's honesty machinery explicitly tracks whether self-correction fires WITHOUT
a reviewer — that datum is the whole point of distinguishing the two (cf. `f8d8f75`, labelled
"advisor-caught" precisely to avoid claiming an unearned self-catch).

**No account is endorsed here, because the artifacts cannot settle it.** What CAN be
established, and is the only new fact this correction adds: the implementing agent gave the
orchestrator a completion report stating the probe was "caught during review, not by me" —
which contradicts that same agent's own commit message (`1b34f5b`) claiming a self-catch not
found by a second party. The agent's two accounts of its own work disagree. `ecd09bc`'s
sentence is traceable: the orchestrator transcribed the agent's completion report without
reconciling it against the commit message the same agent had already written — a transcription
of an unverified claim, which is its own instance of the pattern this file keeps recording.

Disposition: the substantive conclusion is untouched by any of this — the redone probe's
measurement stands (two agreeing channels, both firings), and `3c4f598`'s falsified claim
stays falsified. Only the catch-provenance is unresolved, and it is left unresolved on the
record rather than settled by picking the flattering account.

### RE-RECORDED 2026-08-05 — the "offline bracket anomaly" is D7-by-design, not an anomaly

The O5 Claude-leg round recorded, as an undiagnosed anomaly for founder disposition, that a
bracket whose entire lifetime elapsed while the daemon was offline left `signal_offset` fully
consumed but produced zero turn record. A gate round then reproduced it **deterministically**
(signals appended with no daemon → fresh daemon start → `signal_offset` = exact file length,
zero turns, epoch start/stop only) and found the mechanism, which is not a mystery at all:
`cli/src/daemon.rs::replay_pending_candidates` drops start/stop in the pre-daemon window **by
design**, with a comment saying so ("D7 preserved: ... never fed to the engine, so no phantom
turn can be minted here"). This repo's own CLAUDE.md already records the same fact ("startup
replay never feeds start/stop to the engine at any offset").

The original round's trace of `apply_signal`/`observe_*`/`persist` was literally accurate but
missed the startup site, so it reported a mystery where the answer was a documented decision.

**Re-framed, because the framing changes what the founder actually owns:** this is not a
diagnosis round to commission. It is a product-posture call on D7 — any tool session whose
FULL bracket elapses while the daemon is down is silently dropped as-if-consumed, prompt
included. That is the known D7 tradeoff (never mint a phantom turn) doing exactly what it was
built to do, at a cost that is now measured rather than theoretical.

#### Rider to CORRECTION (b) — its own evidence is unpersisted, and must not rot into fact

Added after a gate round caught CORRECTION (b) committing the pattern it diagnoses. That
paragraph labels `ecd09bc` "a transcription of an unverified claim" and then, one sentence
later, offers as "what CAN be established" a verbatim quote from the implementing agent's
completion report — an artifact that **exists nowhere in this repo**. `grep -rn "caught during
review" docs/ VERIFY-LEDGER.md .claims/` returns exactly one hit: the sentence asserting it.

Therefore, explicitly:

- The completion report lives only in the orchestrating agent's session context. Its quote
  cannot be checked by any reader of this repository — not a future round, not a skeptic, not
  the founder.
- **The only repo-verifiable fact here is the textual contradiction** between `1b34f5b`'s
  commit message ("not by a second party") and `ecd09bc`'s ledger sentence ("caught in review
  rather than by the agent that ran it"). Both strings are in git; anyone can diff them.
- "The agent's two accounts of its own work disagree" is NOT repo-verifiable, because the
  bridge — that the same agent authored the report, and that the report contained those words
  — is unpersisted. Restating it without this rider would let it rot into fact.
- `ecd09bc`'s sentence being a transcription is the orchestrator's own account of its own
  action, offered as such, and carries exactly the weight of an unattested self-report.

This is the same rider convention CLAUDE.md already applies to AC8(a) ("round 1's per-AC
verdict table was never persisted ... restating it without this rider would let it rot into
fact"). The catch-provenance question rests permanently unresolved; no artifact can settle it,
and this rider is the fix rather than further investigation.

#### SUPERSEDED 2026-08-05 — the artifact exists; it was never "nowhere," only outside git

Founder directed further investigation rather than accepting the rider above. The rider's own
scope was too narrow: `grep -rn "caught during review" docs/ VERIFY-LEDGER.md` (the rider's
`.claims/` in that command does not exist on this branch — a reader running it verbatim gets
exit 2, per the handoff's residual-nits note; corrected here rather than repeated) only searches
this git tree. It does not search Claude Code's session transcript store, which is not part of
the repo but is part of the durable record of who did what.

**Found:** `~/.claude/projects/-Users-ravichandrasekhar-Projects-agentrec/01c71c15-e742-49c9-9391-f539cefd0287/subagents/agent-a87a3ec2e766145e4.jsonl`, final message (line 327,
`assistant`, `isSidechain:true`, `2026-08-05T21:24:18.586Z`), a subagent of orchestrating
session `01c71c15-e742-49c9-9391-f539cefd0287.jsonl`, labeled (its `.meta.json`)
`{"agentType":"general-purpose","description":"O5 live Claude Code leg","model":"sonnet"}`.
Verbatim: *"a first attempt measured Claude Code's Bash-tool cwd (non-discriminating — **caught
during review, not by me**)."*

Commit `1b34f5b` lands at `22:23:36+0100` = 21:23:36 UTC, fifty seconds after this report at
21:24:18 UTC — the timing is consistent with the orchestrator committing right after reading it
(NOT proof of causation on its own; timing alone is compatible with coincidence, and is offered
here only as corroboration alongside the direct textual match).

**This settles it, against `1b34f5b`:** the subagent that did the work reported "caught during
review, not by me" (disclaiming self-catch). `1b34f5b`'s commit message says "not by a second
party" (claiming self-catch) — the inverse. `ecd09bc`'s ledger sentence matches the subagent's
own report; `1b34f5b`'s commit message does not. The orchestrator's commit message inverted or
misremembered the subagent's actual words — itself another instance of this repo's
confidently-worded-but-unverified pattern, this time in the artifact that started the whole
correction chain.

**Caveats that must travel with this, because they're the reason the search almost didn't
happen:** (1) the settling artifact lives in a session transcript directory, not this repo — it
is not git-tracked, not backed up by this project, and can rotate out or be deleted by tooling
this repo doesn't control; a future reader following this citation may find it gone, which would
NOT reopen the question, since the finding is recorded here verbatim. (2) This is single-machine,
single-user evidence (`~/.claude/projects/...`), same class of caveat this repo already applies
to perf/timing claims. (3) The search was scoped to transcripts under this repo's own Claude Code
project directory; it did not exhaustively search every possible location a completion report
could live.

The prior rider's operational lesson stands regardless of this outcome: a repo-scoped grep cannot
prove a claim is unrecoverable, only that it isn't in the repo. "No artifact can settle it" was
itself an unverified claim dressed as a methodological conclusion.

## Task E4 — MCP host registration, demand sweep, P.7 review (2026-08-06, `feat/phase-2-tail`)

### DECLARED / manual — MCP demand sweep (delta spec gap 10; D7/D10)

| Row | State | Closes when |
|---|---|---|
| MCP demand sweep — first run | **DECLARED, manual, not yet due** | `scripts/mcp-demand-sweep.sh` is run once, **one month after 2.2 ships**, and its output is pasted into this row. 2.2 is **unshipped as of 2026-08-06**, so no absolute date can be written here yet; the release that cuts 2.2 fills it in (`claude-setup/skills/agentrec-release`). An empty date in this row therefore means "2.2 has not shipped", never "the sweep was forgotten". |

Why the date matters rather than the cadence: Claude Code prunes `~/.claude/projects` on a
~30-day rolling window (measured in the Phase 2.0 P1 round — `-mtime +30` → 0 files). A sweep
skipped by a month does not run late, it runs against a corpus that no longer contains the
evidence. Slippage is lossy, not merely delayed.

What the instrument does and does not do: it greps transcripts for `"name":"…agentrec_<tool>"`
(the `mcp__<server>__<tool>` shape measured on this machine's real transcripts, plus the bare
form) and prints per-tool and per-transcript candidate counts. **It judges nothing** — per the
D7 clauses the audit is manual, and a name in a transcript is a candidate, not a verified
invocation (it can appear in prose or in an echoed tool *definition*). Read-only, no network,
absent directory → explanation + exit 0. Pinned by `mcp_demand_sweep_prints_candidate_invocations`
and `mcp_demand_sweep_handles_absent_transcript_dir` (`cli/tests/integration.rs`, `HOME`
overridden so the script's DEFAULT path is what runs).

### Codex MCP registration — format verified live, location pinned, loading NOT confirmed

Format measured, not recalled: `codex mcp add agentrec -- agentrec mcp` under a throwaway
`CODEX_HOME` on the pinned `codex-cli 0.146.0` wrote exactly

```toml
[mcp_servers.agentrec]
command = "agentrec"
args = ["mcp"]
```

Location is **parent spec :582** ("Project `.codex/config.toml` owns the stdio MCP
registration"), which is founder-owned and not an executor's to overturn.

**Open gap, stated rather than resolved:** that Codex honors `mcp_servers` from a *repo-local*
`.codex/config.toml` is **not confirmed**. `codex mcp list` under an isolated `CODEX_HOME` did
not list a project-local `[mcp_servers.projlocal]` entry — but that probe is **weakly
discriminating**: the management subcommand may only ever consult `CODEX_HOME`, regardless of
what the agent runtime loads. What *is* confirmed live is that Codex loads repo-local
`.codex/config.toml` for the `[hooks]` layer (spike: the dual-representation merge warning names
the scratch repo's own path). The discriminating probe — `codex exec` in a temp repo carrying a
project-local `mcp_servers` entry whose command leaves a filesystem trace — was **not run**
(needs auth + a live model call). This code therefore states what the config declares, not what
Codex does with it, the same honest form `doctorcmd.rs::check_codex_hook_flags` already uses.

### P.7 review (IMPLEMENTATION.md §P.7) — reviewed 2026-08-06, stance unchanged

Reviewed against the surface `agentrec mcp` actually ships today (five read tools, `mcpcmd.rs`).

**Holds.** (a) All five tool descriptions are declarative statements of what is returned —
no directives aimed at the model, no urgency, no authority claims. (b) Tool results are the
`view.rs` typed serializers' JSON carried in `content:[{type:"text"}]`; no imperative text is
synthesized anywhere in the result path. (c) Every tool carries `readOnlyHint: true` /
`destructiveHint: false` (plus `idempotentHint: true`, `openWorldHint: false`), asserted per
tool over the exact five-name list by `cli/tests/mcp.rs` (`names == READ_TOOLS`, then the
per-tool annotation loop) — the Phase E exit item, already satisfied by E1's test, verified
here rather than re-implemented.

**Two findings, REPORTED not fixed** (both would be changes to shipped wire text or to
founder-owned normative text, neither of which this task is authorized to rewrite):

1. `agentrec_recall`'s description ends "raise `k` to see more hits", and its `cursor` property
   says "see the tool description". These are imperatives, addressed to the caller about the
   tool's own API rather than instructions to act in the world — benign on any reading, but
   they are literally the shape §P.7's first clause bans ("no instructions"). Either the clause
   wants narrowing to *instructions to act outside the tool call*, or those two sentences want
   rephrasing declaratively. Founder's call.
2. **The stance does not cover the real exposure.** Both clauses are about text *agentrec*
   writes. The untrusted text on this surface is what agentrec *replays*: recorded prompt
   excerpts and file diffs flow back into an agent's context through `agentrec_log`/
   `agentrec_diff`/`agentrec_recall`, and current MCP security guidance treats tool-result
   content as untrusted data that a host must not follow. Nothing here is a defect in the
   code — the data is faithfully what was recorded — but §P.7 as written would be satisfied by
   a server that echoed an attacker-authored prompt verbatim. A clause naming replayed content
   as untrusted-by-construction is the gap.

**Also stale, found during this review and likewise not rewritten:** §P.2 says read tools
"return structured JSON mirroring `--json` output". Two later founder-series decisions narrow
that — P4b decision 12 (`agentrec_log` mirrors `list()`'s `Page<TurnSummary>`, **not**
`log --json`, which emits protocol JSONL) and delta decision 14 (`agentrec_recall` has no
`--json` contract to mirror and carries its own flags). The code follows the decisions; §P.2's
sentence is the thing out of date.

### E4 residuals and scope statements (recorded, not fixed)

- **Correction to `f4a96c3`'s commit message (Phase E gate finding 1, measured by the gate):**
  the message claims the key-only-predicate neuter "reds exactly the **three** foreign-key
  tests and nothing else." The gate re-ran the neuter (`mcp_entry_is_ours` → `true`):
  **four** tests red — the three named plus
  `uninstallcmd::tests::uninstall_does_not_remove_a_foreign_server_named_agentrec`
  (346 passed / 4 failed). The guard is stronger than claimed; the count in the immutable
  commit message is wrong. Signature-defect class (confident count in a gated record); this
  entry is the correction.
- **Uninstall of a pre-init `{"mcpServers":{}}` is not value-identical (gate finding 2):**
  `remove_mcp_json` drops the now-empty `mcpServers` key, so an empty-table file does not
  round-trip to its pre-init parsed value. No foreign registration is harmed — AC-E4's
  letter holds — recorded here because it was previously recorded nowhere.
- **No parallel D46 leak class.** Both registration targets are repo-local
  (`initcmd::mcp_json_path` = `root/.mcp.json`, `codex_config_toml_path` =
  `root/.codex/config.toml`); nothing user-scoped or global is written, and `uninstall`
  removes both. `init` under a temp root writes them *inside* the temp root, so they vanish
  with it — unlike the launchd unit `service_decision` guards, which outlives its root. The
  temp guard is moot here by construction, not by exemption.
- **`init` does not touch `.gitignore` for `.mcp.json`.** `ensure_gitignore` only ever
  appends `.agentrec/`. `.mcp.json` is the *shared* registration (unlike
  `.claude/settings.local.json`) and is left for the user to commit or not. Note this
  repo's own `.gitignore:10` already lists `.mcp.json` — pre-existing, untouched by E4.
- **Codex `Refuse`-state asymmetry, deliberate:** registration proceeds on `HooksJson` and
  `ConfigToml`, and is withheld on `Refuse`, because `codex_refuse_line` promises the user
  both hook files are left alone. Our own registration cannot push a repo into `Refuse` on a
  later run — `[mcp_servers]` is not a hook representation, pinned by
  `initcmd::tests::mcp_registration_alone_does_not_change_the_codex_hook_target`.
- **Comment-dropping, inherited:** on the non-`Refuse` paths, registering re-serializes the
  WHOLE `.codex/config.toml` through the `toml` crate, so a user's comments and key ordering
  are lost on a file they did not ask to have reformatted. Same documented limitation as
  `install_codex_hooks_toml` (fixing it needs `toml_edit`, a new dependency). Disclosed by
  the printed action line; not silent. **`.mcp.json` reformats the same way** (post-review
  correction — this bullet previously read as Codex-only): `install_mcp_json`/`remove_mcp_json`
  re-serialize whole through `serde_json`, whose `Map` is a `BTreeMap` here, so keys re-sort
  alphabetically and `to_string_pretty` normalizes layout. Foreign entries survive as parsed
  values, never as bytes. Its printed action line carries no equivalent disclosure — recorded,
  and deliberately NOT added (a new user-facing line is a behavior change nobody asked for).
- **Declared `cursor` type is narrower than the implementation, and narrower than the only
  cursor a client can hold — found by the P.7 F1 schema sweep, recorded NOT fixed.**
  `agentrec_log`/`agentrec_diff`/`agentrec_recall` all declare
  `"cursor": {"type": "string"}`, but `Page::next` serializes `view::Cursor` as a JSON
  OBJECT (parity with `--json` forbids stringifying it), and `mcpcmd::cursor_arg`
  deliberately accepts both shapes. A host that validates `tools/call` arguments against
  the advertised schema would therefore reject the obvious move — lifting `next` verbatim
  out of the page it just read. Widening the declared type is a wire-contract decision the
  review did not mandate and is founder-owned; the descriptions ("Opaque cursor from a
  previous page") are accurate as written. This is the one further schema/implementation
  mismatch the F1 sweep turned up; the four non-diff tools' parameter *descriptions* are
  accurate (`log.limit` and `recall.k` both clamp, `log.include_hidden` names both halves
  of `include_all`, `blame.line` matches, `status` takes no parameters).
- **`uninstall` aborts on an unparseable registration file before service removal and
  `.agentrec/` archival.** An invalid `.mcp.json` (or `.codex/config.toml`) returns `Err` and
  ends the run early. Pre-existing pattern — `remove_claude_hooks` aborts identically — and
  extended by E4 to the two registration files; `init`'s counterpart degrades gracefully
  ("MCP registration skipped: {e}" as an action line). Tested in neither direction. Changing
  the ordering, or making uninstall degrade like init, is founder-owned.
- The sweep script is committed `100755` but every caller invokes it as
  `bash scripts/mcp-demand-sweep.sh` (test and docs both), matching the `check-versions.sh`
  precedent — nothing depends on git's recording mode.
- Release-seam check re-run against a **freshly built** `target/release/agentrec` (the
  earlier reading was against a stale binary and is not the one recorded here):
  `strings … | grep -c AGENTREC_TEST` → **0**. E4 adds no env seam.

## Task F5 — undo `origin` discriminator (2026-08-06, `feat/phase-2-tail`)

### DECLARED / manual — post-ship undo counts by origin (delta decision 11)

| Row | State | Closes when |
|---|---|---|
| Executed-undo counts by origin — first reading | **DECLARED, manual, not yet due** | Delta decision 11 waived decision 6's ≥20-human-confirmed-`undo --confirm` gate and demoted it to this row (the same demotion decision 10 applied to decision 7). Count at waiver: **0** — `log.jsonl` held zero `tool: "agentrec"` turns. Closes when, **one month after 2.3 ships**, the counts are read off real `log.jsonl` files and pasted here as three numbers: undo turns with `origin: "mcp"`, with `origin: "cli"`, and with **no `origin` key at all**. 2.3 is **unshipped as of 2026-08-06**, so no absolute date can be written yet; the release that cuts 2.3 fills it in (`claude-setup/skills/agentrec-release`). An empty date means "2.3 has not shipped", never "the reading was forgotten". |

**Three numbers, not two, and the third is why.** §5 says an absent `origin` reads as `"cli"`,
so the arithmetic answer is to fold absent into cli and report two. Do not: an absent key is a
**pre-F5** undo turn, and folding it in silently contaminates the post-ship denominator this
row exists to establish with undos executed before the discriminator existed. Report it
separately and subtract it, or the row measures a population it did not scope.

**`"cli"` is two populations, and this row cannot separate them.** `origin` names the surface
that EXECUTED the writes, not the one that requested them, so `"cli"` covers both a human's own
`agentrec undo --confirm` and an `agentrec approve` of an undo an **agent requested over MCP**.
Decision 6's gate was about human confirmation, and both of those are human-confirmed — which is
why `approve` records `"cli"` and why the count is still the right instrument for the gate. But
a reader wanting *human-initiated* undos specifically must join to `.agentrec/undo-requests.jsonl`
(an approved undo has a `request` event followed by an `execute` event — only the `execute`
event carries `undo_turn`; no `approve` event is ever written, `EVENT_APPROVE` is reserved and
`undo_coordinator.rs` documents the durable approval as `EVENT_EXECUTE` itself; a bare
`undo --confirm` has no ledger row at all). **A count reported as "N human-initiated undos"
without that join is wrong**, and this paragraph must travel with any figure derived from the
field.

**No instrument is committed for this row**, deliberately, and that is a difference from E4's
sweep: the counts are a `grep`/`jq` over `log.jsonl` files whose locations are not knowable in
advance (every user's repos), so a script here would measure only the dogfood repo and read as
if it had measured the population. The reading is manual and its scope must be stated with it.

### Suite

`cargo test --workspace -- --test-threads=3` → **985 passed / 0 failed / 4 ignored**
(base at `93e43c2`: 981/0/4). The **+4 is exactly the four new
`cli/tests/undo_origin.rs` tests** and nothing else: F5's other test edits are
assertions added inside existing tests (`cli/tests/conformance.rs`) or two existing
assertions inverted (below), neither of which moves a count. The conformance half
of that was verified rather than reasoned: `cargo test --test conformance -- --list`
returns the same **4** test names as before, because the new `MANIFEST` row is data
iterated by one existing `#[test]`, not a new one. clippy `-D warnings`
debug **and** release: exit 0. `cargo fmt --check`: exit 0.

**Two pre-existing assertions were INVERTED, not deleted, and that is disclosed
rather than quiet.** F3 and F4 each planted a guard that `origin` must be ABSENT
("F5 owns `origin`; F3/F4 must not add it early") —
`the_approved_undo_turn_has_the_shape_cli_undo_produces` (`cli/tests/approve.rs`)
and `ac_f4_execute_reverts_records_a_turn_and_the_undo_is_re_undoable`
(`cli/tests/undo_execute.rs`). Both now assert F5's specified value, `"cli"` and
`"mcp"` respectively. The guards did their job: both went RED on this change
before being touched, which is how F5's threading was proven to reach both
transports and not merely the new test file.

### There is no fifth undo-turn writer — checked, not assumed

Four `append_undo_turn` call sites were threaded, but "four" came from grepping
`append_undo_turn` itself, which cannot find a writer that bypasses it. The
discriminating check is the other direction: `git grep -n 'Some("agentrec"' --
cli/src agentrec-core/src` returns **exactly one constructor**,
`cli/src/readcmds.rs::append_undo_turn`. Every other hit is a *reader* predicate
(`readcmds.rs::…is_synthetic`, `undo_coordinator.rs`'s rich-coverage test) or an
unrelated `initcmd` string. So no path appends an `agentrec`-tooled turn without
stating an origin, and the eleven mechanical `origin: None` insertions into other
`TurnRecord` literals are all on non-undo turns, where `None` is the correct and
required value.

### Mutation probes (run at `2706704`, on a clean tree, after `cargo build`)

Both are **per-site** and both discriminate: an all-sites-fail probe cannot
distinguish three independently-wired call sites from one shared default.

| Probe | Result |
|---|---|
| `mcpcmd.rs` execute → `UndoOrigin::Cli` | `ac_f5_mcp_execute_records_origin_mcp` RED (`left: "cli"`, `right: "mcp"`); the other **three** `undo_origin` tests stay GREEN. `ac_f4_execute_reverts_records_a_turn_and_the_undo_is_re_undoable` also REDs — the F4 guard covers the same site |
| `approvecmd.rs::approve` → `UndoOrigin::Mcp` | `ac_f5_approve_of_an_mcp_request_records_origin_cli` RED (`left: "mcp"`, `right: "cli"`); the CLI-undo and MCP-execute tests stay GREEN, proving `approve` is wired separately and does not inherit either |

Pre-implementation RED, for the record: with the field present but nothing
writing it, the three behavior tests failed on `left: Null` against their
expected values while the legacy-reading test already passed — i.e. they
failed as assertions, not as compile errors.

### Verified here (automated)

| AC | Evidence |
|---|---|
| AC-F5.1 CLI undo → `"cli"` | `ac_f5_cli_undo_confirm_records_origin_cli` (`cli/tests/undo_origin.rs`) — drives the real binary, asserts the raw `log.jsonl` bytes |
| AC-F5.2 MCP undo → `"mcp"` | `ac_f5_mcp_execute_records_origin_mcp` — real `agentrec mcp` subprocess, auto-mode token |
| AC-F5.3 approve → `"cli"` | `ac_f5_approve_of_an_mcp_request_records_origin_cli` — also asserts the request ledger names the undo turn, so the MCP provenance `origin` drops is demonstrably recoverable |
| AC-F5.4 pre-F5 line parses, reads as cli, round-trips | `ac_f5_a_pre_f5_undo_turn_without_origin_reads_as_cli`; plus `named_fixtures_carry_the_semantics_they_are_named_for` (`cli/tests/conformance.rs`) against a stripped §5 fixture line |
| Renders/blames identically | `cargo test --test golden` → **33 passed / 0 failed**, and all **30** golden files are byte-identical (`git status --short cli/tests/fixtures/golden/` empty afterwards). Both figures measured here, and they are different things: **30 `.golden` files, 33 golden tests** — CLAUDE.md's bare "33 goldens" is the test count, and quoting it as a file count is the arithmetic this row refuses. Their seeded undo turn keeps `origin` absent **on purpose** — it IS the pre-F5 record, so every log/show/blame golden built from it is the unchanged-rendering proof (`cli/tests/golden.rs`) |

## Phase F residuals recorded at gate time (2026-08-06, gate round 1 blocker 2 + findings)

- **`.agentrec/undo-requests.jsonl` growth is unbounded, and no sanctioned rewrite class
  covers it** (Phase F review finding 13, recorded here because the gate found it recorded
  nowhere). Every auto `preview` and confirm `request` appends rows; terminal events resolve
  a reservation but no line is ever removed, and the file is deliberately outside the three
  sanctioned `purge` rewrite classes ("never a wire surface" — `LEDGER_V`'s comment — does
  not make it non-disk). An agent can grow it without bound by spamming preview/request.
  Growth rate in real use: unmeasured. Adding a reclaim path requires a decision-register
  entry, same bar as D48's. Founder-owned.
- **The coordinator's auto `allow_modified` rail is downgrade-with-warning, not refusal**
  (gate round 1, non-blocking finding 1). §8's "refused outright" behavior exists only at
  the transport rail (`mcpcmd`'s router); `UndoCoordinator::preview` in Auto coerces the
  flag false, warns, and returns a successful preview with modified files excluded
  `ModifiedSince`. The safety property was probe-proven intact at both layers
  independently, but any sentence claiming "refused at both rails" is wrong as written — a
  future non-reference transport built directly on the coordinator would ignore-with-warn,
  not refuse. Recorded so the §8 wording and the coordinator's behavior are not conflated.
- **Kill-9 flake provenance, pinned by the gate:** `approve.rs::
  a_killed_approve_never_leaves_a_phantom_approval` fails intermittently (gate measured 1/26
  runs at stock `335f076`, 2/~14 at `e32d247`, same assertion `approve.rs:440` both times)
  when the kill lands between `append_undo_turn` and the EXECUTE row — a window the crash
  matrix itself documents. Pre-existing; NOT introduced by `e32d247` (test body byte-identical
  across the range, e32d247's diff touches only the failure branch). The test doc's
  "window-independent" claim is true of the ledger's two-state invariant and FALSE of the
  strong-form log↔ledger agreement the failing assertion adds. A crash in that window is
  SAFE (re-approve → `preview_stale`); the defect is the test's claim, not the crash
  behavior. Disposition (weaken the assertion + fix the doc vs re-order the writes)
  founder-owned.
- **Confirm-path variant of the consumed-but-unresolved residual, stated explicitly** (gate
  round 1, non-blocking finding 3): a kill-9'd approve leaves worktree reverted + turn
  logged + request pending + reservation live until `REQUEST_TTL_MS` (10 min), not the 60 s
  token TTL — the earlier residual named only the auto path.

## Phase F exit — self-healing acceptance story, run live (2026-08-06, `feat/phase-2-tail`)

Manual, not-unit-testable honesty row per the plan's Phase F exit criterion. It exists to close
the gate's recorded blocker — *"the destructive loop end-to-end has zero live evidence"*. Full
method, every command, every raw payload and hash: `docs/verify/f-exit-self-healing.md`. Run at
worktree HEAD `78fbc36` (clean tree).

**Binary: RELEASE, deliberately** — `target/release/agentrec`, `agentrec 0.2.0`, sha256
`c1c00649dc3b1147120e0c2f1ffbe083eed00eaa4f912ce3a5c9ac10f3ade65e`. This row is a
product-behavior story about the artifact users install, so unlike O5 (debug, disclosed) it runs
the release build.

**Agent identity is SIMULATED, stated first because it bounds everything below.** The two agent
turns were driven through the real hook emitter (`agentrec hook claude` with `UserPromptSubmit` /
`Stop` payloads on stdin, the way `cli/tests/misattribution.rs::send_hook` drives it) wrapping
real file edits — the sanctioned hook-driven option. Bracketing, signal inbox, daemon
consumption, turn minting, CAS snapshots, and every read/destructive verb are production paths;
only the identity of the writing process is simulated. No `claude -p` session was involved.

**Both legs green, on a disposable python repo (`calc.py` + two independently-failing named
tests), never the agentrec repo.** Leg 1 (`mcp_destructive = "confirm"`): agent breaks `test_sub`
→ daemon stopped → `log`/`blame`/`diff` all find the agent's own turn with no daemon running →
MCP over real newline-delimited JSON-RPC frames (revision `2025-11-25`) lists **6 tools** (5 read
+ `agentrec_undo`, listed because mode ≠ `off`) → `preview` (no token, worktree digest unchanged
after it) → `request` (10-min expiry observed on the wire as `expires - requested = 600000`) →
human `agentrec approve <id>` → `test_sub` green → undo turn appended with **`"origin":"cli"`**.
Leg 2 (`auto`): agent breaks `test_add` → both rails fired with their documented messages
(`wrong_mode` for `request`-in-auto; `allow_modified_refused` for `allow_modified: true`) →
`preview` issues a 60 s token → **`execute` run from a SECOND `agentrec mcp` process**, the
preview process having already exited, with 40.99 s of TTL left → `test_add` green → undo turn
with **`"origin":"mcp"`**. `grep -o '"origin":"[^"]*"' log.jsonl` over the one repo returns
exactly `cli` then `mcp`. Replaying the spent token → `token_consumed`, worktree digest unchanged.
Request ledger ended `0600` with all five events (`request`/`execute`, then
`reserve`/`consume`/`execute`); F4's spend-before-writes order is visible in the last pair —
`consume` at `…363051` precedes `execute` at `…363067`, **16 ms apart**. That figure is the
consume→execute gap ONLY: `reserve`→`consume` is 19,015 ms, the wall-clock interval between the
preview process and the separate execute process, and says nothing about write ordering. The raw
token occurs 0 times in the ledger's bytes, only its sha256.

**`origin: "cli"` for an agent-requested, human-approved undo is the SPECIFIED behavior, not a
miss** — `record.rs`'s doc comment: *"It names the surface that wrote, never the one that asked."*
Both observations carry the key explicitly, so neither rests on the absent-means-`cli` default.

**The first attempt did not work, and it reshaped the run — recorded, not papered over.** A
pre-check smoke of the undo round-trip produced `"before": null, "baseline_unknown": true` and
nothing to revert. Not a defect: `record.rs:154` documents the field as exactly "before
unrecoverable (first seen post-change)" — a daemon started moments before the edit has never
observed the file, and git having it committed is irrelevant (the CAS is the daemon's, not git's).
**Consequence, which is a genuine limit on this row's strength:** each leg required a **warm-up
human write** (visible as a `bare` turn in every log) so the daemon held a baseline, and it was
needed **twice** because a fresh daemon process starts baseline-less again. That a
continuously-running real daemon supplies this for free is an *argument*, not something this round
measured — **the story was never run against a daemon that had been up for days.**

**Hash tie, observed rather than asserted:** at every transition in both legs, the turn record's
`before`/`after` CAS refs are byte-identical to `shasum -a 256` of the file at that moment, and
each post-undo digest equals its leg's baseline digest exactly.

**Hygiene, measured at both ends.** `agentrec record|mcp` processes **5 → 5**, the same five PIDs
with identical `lstart` (binary-path predicate). LaunchAgents **1 → 1 → 1** across before / after
`init` / teardown; `init` on the `/private/tmp` root printed the D46 temp-root skip and
`--service` was never passed. `pkill -f agentrec` was never run.

**Two corrections to the round's own framing, both measured:**
1. The task briefed **two** pre-existing leaked debug daemons (42419/42421). There are **four** —
   41925 and 41952 too, all four `lstart` Wed Aug 5 15:37–15:38, all `target/debug` under
   `/var/folders/...T/` tempdir roots (the same shape D46 reaped). None touched. A "count
   restored" claim against a denominator of 2 would have been reported against the wrong number.
2. A `pgrep -f "agentrec (record|mcp)"` predicate reported 7 mid-round and triggered a false
   alarm: two `claude -p` processes carried the string `agentrec record` **in their prompt text on
   the command line**. Every count in the evidence doc uses the binary-path predicate instead.

**Not established, enumerated rather than implied:** agent identity simulated (above); the
warm-up's dependence on a long-lived daemon unmeasured (above); one file / one path per turn, so
**no** `skipped`/`withheld`/`modified_since` refusal, no path-subset reservation, and no
`undo_conflict` was exercised live (`refusals` was empty in every preview — those rest on F2's
fixtures, not on this row); macOS only, no Linux leg; strictly sequential, so no two-actor race on
a reservation; `deny` never run; and **neither expiry path was reached** — the 10-min request
expiry and 60 s token TTL were observed only as fields on the wire, beaten by ~9.5 min and ~41 s
respectively.

**The `allow_modified` refusal observed here is the TRANSPORT rail only** (`mcpcmd`'s router,
which intercepts before the coordinator). Per this file's own Phase F gate-round-1 finding 1,
`UndoCoordinator::preview` in Auto **downgrades-with-warning rather than refusing**. This round
did not probe that layer, so nothing here is evidence of refusal at depth, and "both rails fired"
above names two distinct *rails* (the mode matrix and the `allow_modified` check), never two
*layers*.

## Phase F gate round 2 (2026-08-06, HEAD `d8c69e3` + this commit)

Adversarial re-gate over the two blocker closures. Verdict on the closures themselves: **both
HOLD** — the E2E evidence was independently re-verified (the raw token quoted in the preview
hashes to exactly the `token_sha256` in the reserve row — two independently-quoted values,
cryptographically bound; all timestamp arithmetic exact; the surviving disposable repo's
`log.jsonl`/`undo-requests.jsonl`/`calc.py` all match the doc), and finding 13 was confirmed
truthful against source (`undo_coordinator.rs::append_event` sole writer, no
truncate/rotate/reclaim path, `purgecmd.rs` has zero references). The round still **GATE-FAILED
on two record defects**, both fixed in this commit:

1. **The F5 "two populations" paragraph's join instruction was false against the code beside
   it** (introduced at `2706704`): it said an approved undo has a `reserve`+`approve`/`execute`
   row. The confirm path writes `request` (not `reserve` — `RESERVING_EVENTS` holds both as
   distinct events) and **no `approve` event is ever written** — `undo_coordinator.rs::
   EVENT_APPROVE` documents the durable approval as `EVENT_EXECUTE` itself, and the E2E's own
   Part 4 shows the real confirm-leg rows (`request` then `execute`). A reader executing the
   join as written greps for events that do not exist and reads every approved undo as
   unmatched. Corrected in place; only `execute` carries `undo_turn`. Signature-defect class:
   confidently-worded record text contradicted by the code beside it.
2. **A flake that does not exist was briefed to the gate**: `hook_kill_9_closes_open_epoch`
   (~3/6 rate) appears nowhere in the tree, in git history (`git log -S`, all branches), or in
   any doc — it existed only in a session handoff transcribed from chat memory. The only
   durably recorded flake is `approve.rs::a_killed_approve_never_leaves_a_phantom_approval`
   (rates 1/26 and 2/~14, pinned in gate round 1's provenance bullet above). The false name is
   recorded here precisely so no future exit narrative resurrects it.

**Suite baseline, re-measured and recorded (not restated):**
`cargo test --workspace -- --test-threads=3` at this HEAD → **987 passed / 0 failed / 4
ignored** — measured independently by the orchestrator (twice: once foreground, once summed
across binaries) and by the gate itself (twice, clean tree before and after, zero
`FAILED`/`panicked` strings). Arithmetic from the last recorded baseline closes: 985
(pre-`e32d247`) + 2 (`e32d247`'s fault-injection tests, counted from its diff) = 987.

**Residuals the gate left open, recorded not fixed:** coordinator-layer `allow_modified`
downgrade (finding 1) stands; the fexit corroboration repo lives in a session scratchpad and
will vanish — the doc's hashes are the durable evidence. Clippy (`-D warnings`, debug and
release) and `cargo fmt --check` were run clean by the orchestrator after the gate; the
round-3 re-gate re-ran both clean (`--all-targets`, both profiles) and additionally ran the
release-seam check itself: `strings target/release/agentrec | grep -i AGENTREC_TEST` → zero
lines, on a release binary whose mtime postdates the last code commit (`e32d247`; every later
commit is docs-only, so the artifact corresponds to this HEAD's code). Caveat carried from
that gate: the seam grep keys on the `AGENTREC_TEST` prefix convention — a seam named outside
that prefix would evade it (none known).

## Phase 2 tail — plan exit round (2026-08-06, HEAD `670fba3` + this commit)

Evidence roll-up for the plan's "Final acceptance — plan exit" checklist
(`docs/superpowers/plans/2026-08-05-phase-2-tail-plan.md`, on `docs/user-onboarding`).
Per-box refs, then this round's own measurements.

- **Phase A fixtures + pin:** ledger § "Phase A — Codex hook spike"; `codex-cli 0.146.0` pin
  (INTEGRATIONS.md, live-verified); `docs/fixtures/codex/*.json` (7, git-tracked);
  `docs/verify/codex-spike.md`.
- **Config loader:** `cli/src/config.rs` live, 5 legacy read sites migrated, scanner deleted
  (remaining "scanner" hits are historical prose in doc comments only); D16 semantics tested
  (`config.rs::tests` ×5 + `integration.rs::daemon_startup_refuses_on_malformed_config` +
  `mcp.rs::invalid_config_toml_is_a_startup_hard_error`).
- **O5:** ledger § "O5" + `docs/verify/o5-two-tool-session.md`; Claude leg re-run LIVE
  (appended row); carried disclosures stand (sequential not simultaneous; debug binary).
- **PROTOCOL 1.0:** `PROTOCOL.md` header "1.0 (frozen 2026-08-06)"; 29 conformance fixtures
  + manifest test (`cli/tests/conformance.rs`), all in `8d8e1db`. **Founder review of the
  freeze diff is asserted ONLY in `8d8e1db`'s commit message — no ledger row records it and
  there is no Phase D ledger section at all.** Recorded as provenance, not as fact
  established by this round; founder attestation would close it.
- **Five read tools + parity + §8 status row:** `mcpcmd.rs` (five read tools + gated undo);
  parity pinned in `cli/tests/mcp.rs` (`ac_e2_diff/blame_*_byte_equals_*`; log/recall
  typed-shape by decision 12 / delta 14); `PROTOCOL.md` §8 `agentrec_status` row.
- **Mode matrix + §8 `allow_modified` + origin:** `ac_f1_*` (mcp.rs), `ac_f4_*`
  (undo_execute.rs), approve.rs suite; §8 fixed at `e264d7c`; origin tests
  `cli/tests/undo_origin.rs::ac_f5_*` + conformance fixture. Coordinator-layer downgrade
  residual stands (gate finding 1).
- **Self-healing story:** ledger § "Phase F exit" + `docs/verify/f-exit-self-healing.md`
  (`d8c69e3`); Phase F gate rounds 2–4 above.
- **Baselines:** debug `cargo test --workspace -- --test-threads=3` → **987/0/4** (§ "Phase F
  gate round 2"). Clippy `-D warnings` debug+release, `cargo fmt --check`, release-seam grep:
  all clean (same section + round-3 fix).

**Release-profile suite, run for the first time in this repo's recorded history — and the
checklist box's "debug+release" cannot be read as "the full suite passes under
`--release`". THREE runs, stated separately** (the plan-exit gate's round-1 blocker was an
earlier version of this paragraph conflating them into one figure — the doctorcmd
observations below came from run 1, while `964/24/4` is run 3's; no verdict assignment could
reconcile the two, and the gate proved it by reproducing 964/24/4 at HEAD with zero
`doctorcmd` names among the 24):

1. **Run 1, PRE-doctorcmd-gating, fail-fast:** aborted inside the `agentrec` bin's unit
   tests at **355 passed / 3 failed / 1 ignored** (partial — cargo stopped at the first
   failing binary). The 3 failures were the `doctorcmd` service-unit tests; this run is the
   evidence behind the gating fix below. Its full-suite totals were never measured and are
   unrecoverable.
2. **Run 2, POST-gating, fail-fast:** aborted at the known recorded flake
   `approve.rs::a_killed_approve_never_leaves_a_phantom_approval` — **369 / 1 / 1**
   (partial). The flake's rates and disposition are already recorded above (Phase F gate
   round 1); this was one more observation of it, in release.
3. **Run 3, POST-gating, `--no-fail-fast` (the complete measurement):**
   `cargo test --release --workspace --no-fail-fast -- --test-threads=3` →
   **964 passed / 24 failed / 4 ignored** (debug at the same tree: 987/0/4). The known
   approve flake did NOT fire in this run.

All 24 of run 3's failures were individually classified against source (Opus classification
round; table persisted durably in `docs/verify/release-seam-classification.md`, and the
plan-exit gate independently reproduced the totals, the 24 names, 3 seams read in source,
and 1 empirical probe + positive control): **24 SEAM / 0 TIMING / 0 REAL** — every failure
is a test driving a `#[cfg(debug_assertions)]`-gated seam that release strips BY DESIGN (the
same design the release-seam `strings` check exists to enforce). Family arithmetic, summing
to exactly 24: hardening_cli 3 (`purgecmd.rs` test-pause sleeps) + import_claude 11 (9 ×
`importcmd.rs::debug_dump_entries_enabled` + 2 × `importcmd.rs::t2_oracle_enabled`) +
import_codex 3 (debug_entries family, incl. the key-parity list hardcoding `debug_entries`)
+ integration 7 (3 × `cmds.rs::effective_store_budget`/`daemon.rs::effective_evict_interval`
+ 3 × `cmds.rs::recall_deadline`/`memory.rs::test_slow_pin_read_delay` + 1 ×
json_contracts budget). Positive control:
`persist::ac5b_oracle_seam_disabled_in_release_even_with_env_set` PASSES in release.
**No release-only product defect found.** Honest coverage statement: ~20 of the 24 fail at a
seam-gated precondition, so their subject invariant is UNEXERCISED in release (not proven
equivalent) — notably the zero-write status parity assertion; four others' invariants are
visibly confirmed in the captured release output despite the test failing (daemon degrades
to defaults on mid-tick corruption; codex/claude report key parity at 14 keys; two
tier-classification runs print correct `tier_counts`).

**Seam-caveat correction (plan-exit gate round-1 blocker 2):** the round-3 caveat above
("a seam named outside the `AGENTREC_TEST` prefix would evade it — none known") is FALSE:
`AGENTREC_CLAUDE_PROJECTS_DIR` (`doctorcmd.rs::claude_projects_dir`) is an UNGATED env
override present in release `strings`, whose own doc comment says it exists "so this check
is hermetically testable". Pre-existing since the repo's initial commit (`5c3a915`, on
`main`); low severity — it redirects a read-only doctor advisory's scan directory, writes
nothing. Recorded, not changed: whether to gate it `#[cfg(debug_assertions)]` (making the
doctor check untestable in release builds) or sanction it as a production knob (documenting
it) is a founder call. Until then, "no seams in release strings" must be read as "no
`AGENTREC_TEST`-prefix seams, plus this one named exception".

**Fix landed this round (code, small):** the three `doctorcmd` service-unit tests that
FAILED in release (`orphaned_unit_is_advisory_not_fail`,
`vanished_exec_on_a_live_root_is_reported_advisory`,
`vanished_root_and_unparseable_units_carry_no_exec_finding`) — plus the three sibling tests
that PASSED in release only by ACCIDENT — all drive the debug-only
`AGENTREC_TEST_SERVICE_DIR` seam; in a release build the seam is ignored and all six scan
the developer's REAL service directory, so their verdicts depend on ambient machine state
(the accidental passes are the worse failure mode). All six + the `with_service_dir` helper
are now `#[cfg(debug_assertions)]`. The other 24 seam tests are deliberately NOT gated: they
fail deterministically at a seam precondition and never touch real user state, so leaving
them visible in a release run is a truthful signal rather than a hazard; gating them is
available if a release-suite CI leg is ever added. Debug suite after the gating change,
re-measured: **987 / 0 / 4** — unchanged, because `cfg(debug_assertions)` is true in the
debug profile so all six still compile and run there. Clippy `-D warnings` `--all-targets`
debug AND release, `cargo fmt --check`: clean after the change.

**Stale-doc fixes landed this round:** IMPLEMENTATION.md §P.1 (three-tool list → five,
amended-note style), §P.2 ("mirroring `--json`" → parity/typed split per decision 12 /
delta 14), §O.5 (O5 evidence pointer added); INTEGRATIONS.md Codex bullet + Ring 2 verb
lists (three tools → five); CLAUDE.md worktree Status rewritten through Phase F (the stale
"Next: Phase D BLOCKED" block replaced; the false "J1/J2/I3 recorded-not-fixed" bullet
corrected — Phase B closed that debt).

## Branch review (PR #20) — root-containment blockers 1+2, FIXED (2026-08-06)

Whole-branch adversarial review of `feat/phase-2-tail` at `e5d7ea8` (58 commits vs
`main`@`23a2e0d`), scoped deliberately to what per-phase gates structurally could not see:
cross-phase seam drift, freeze discipline across the whole history, and the security posture
of the new agent-triggerable write surface. It returned **NOT MERGE-READY on two BLOCKERs**,
both proven end-to-end against the real binary rather than inferred. Fixed at `f501f07`.

**The defect.** `entry.path` is wire data, and `root.join(&entry.path)` in
`undo_coordinator.rs::execute_revert`/`restore_from_before` was an arbitrary-write primitive:
`Path::join` with an ABSOLUTE path silently discards the base, and a `..` component walks
straight out. `preview` validated requested paths only against `target.files` — the same
attacker-supplied record. Two shapes, each proven live in `mcp_destructive = "auto"` over
real JSON-RPC frames, zero refusals, no human in the loop:

1. `"path":"../outside/victim.txt"` → preview issued a token, execute reported
   `{"reverted":1}`, and the outside file's bytes changed.
2. `"path":"linkdir/c.txt"` where `linkdir` is a symlink OUT of the repo → same result.
   `is_symlink_on_disk` lstats the FINAL component only, while `restore_from_before` runs
   `create_dir_all(parent)` and writes THROUGH the intermediate link. **This shape is the
   worse one**: the rendered path looks entirely ordinary, so the CLI's human-reads-the-plan
   mitigation fails too.

**Provenance, stated rather than shaded.** The primitive is PRE-EXISTING on `main`
(`23a2e0d:cli/src/readcmds.rs:1063` is the identical `root.join`); F3 moved it verbatim.
What this branch changed is the ACTOR — on `main` the only caller is a human who sees
`revert ../outside/victim.txt` printed before typing confirm; `agentrec_undo` in `auto` mode
removes that human. Phase F's own gate validated its rails against the refusal inventory
inherited from the CLI (symlink, skipped, withheld, modified-since), and root containment was
never in that inventory *because the CLI's actor was a human reading a list*. That is exactly
the class a per-phase gate cannot catch, and why the whole-branch pass earned its cost.
Threat model is the SANDBOXED AGENT: agent permissions commonly confine edits to cwd,
`.agentrec/` is inside cwd, the victim is not — agentrec becomes a confused deputy performing
the out-of-sandbox write. Precondition bounded and checked: `import` is NOT a vector (both
sites `strip_prefix(root_canon)` → `skipped_out_of_root`), and L3 sidecar-log merging does not
exist, so the precondition is a direct write to `.agentrec/log.jsonl`.

**The fix (`f501f07`).** `undo_coordinator.rs::escape_refusal`, called FIRST in `build_plan`'s
per-entry loop — above even the symlink refusal, and unconditional w.r.t. `allow_modified`.
`build_plan` is the shared planner both legs reach (`readcmds.rs::undo`,
`undo_coordinator.rs::preview`, `::claim_grant`), so ONE gate covers CLI and MCP; the review's
disposition explicitly warned against treating the CLI's rendered plan as a mitigation for the
MCP leg. Two checks, and probe-proven non-redundant:
1. LEXICAL — reject absolute paths and any `ParentDir`/`RootDir`/`Prefix` component.
2. RESOLVED — canonicalize the nearest EXISTING ancestor of the target, require canonical
   `root` as a prefix. Absent deeper components cannot be links, which is why a not-yet-created
   directory is still allowed.
Both sides canonicalized because a symlinked root is ordinary (macOS `/tmp` → `/private/tmp`);
comparing a canonical child against a non-canonical root would refuse every legitimate revert.
Canonicalize failure REFUSES — fail closed.

**A fail-open bug in the fix's own first draft, caught before commit and recorded because the
pattern is subtle:** the draft wrote `root.canonicalize().ok()?` inside a
`-> Option<PlanKind>` where `None` MEANS ALLOWED, so `?` would have failed OPEN on an
unresolvable root — inverting the doc comment sitting directly above it. Now an explicit
`return refuse()`, with a comment naming the trap.

**Evidence, all run:**
- Live re-run of BOTH exploits against the release binary, `auto` mode, real JSON-RPC:
  `files: []`, one refusal each carrying the containment reason, **`token issued: False`**,
  and both victim files byte-unchanged. CLI leg on the same forged record:
  `REFUSE … nothing to revert`.
- Mutation probe 1 (gate call neutered): exactly the 3 escape tests red, all 3 positive
  controls green.
- Mutation probe 2 (RESOLVED check short-circuited, LEXICAL retained): exactly the
  symlinked-parent test red, other 8 green — check 2 earns its place.
- Suite **993 / 0 / 4** (base 987 + 6 new). An intermediate run with the gate but no new tests
  measured 987/0/4, which is the evidence that containment over-refuses NO existing path.
- Clippy `-D warnings` `--all-targets` debug AND release, `cargo fmt --check`: clean.
  Release-seam grep `AGENTREC_TEST`: 0.

Positive controls are DISCRIMINATING by construction: each asserts the entry reaches the
store gate BELOW containment (`prior snapshot unavailable — refusing to restore`), not merely
that it was "not refused", so a gate that refused everything reds.

**Not fixed here, recorded:** the review's finding that `main`'s CLI leg carries the same
primitive today (mitigated only by a human reading a path that shape 2 shows can be made to
look innocent). This branch's fix lands in shared core, so merging carries it to the CLI too —
but v0.2.0 is already published with the unfixed CLI, and whether that warrants a security
note is a founder call. Also unverified by anyone: no Linux leg has run this branch's HEAD.

### Re-gate round 2 — the hardlink escape (a THIRD shape, found by hunting for one)

The scoped re-gate confirmed both original blockers fixed — it rebuilt the exploits from
scratch in its own repo rather than re-running the orchestrator's, and reported `files: []`,
one containment refusal each, **`token issued: False`**, victims byte-unchanged, on both legs.
It also confirmed the RESOLVED check earns its place by a cleaner channel than the mutation
probe: `linkdir/c.txt` contains no `ParentDir`/`RootDir` component, so the LEXICAL check
provably cannot be what refused it.

**But the re-gate's brief asked it to hunt for a third escape shape, and it found one — same
harm, same agent-triggerable path, surviving the fix.** GATE FAIL, one blocker, fixed at
`ce641c4`.

**HARDLINKS DEFEAT PATH CONTAINMENT, STRUCTURALLY.** `canonicalize` resolves SYMLINKS. A
hardlink is a second directory entry for the same inode, and there is **no path-level evidence
that the inode is reachable elsewhere** — so no amount of path checking can see it. A
repo-internal `hard.txt` hardlinked to a file outside the repo is lexically ordinary,
canonicalizes inside `root`, and is not a symlink: it passed `escape_refusal` AND
`symlink_refusal`, and `fs::write` truncated the shared inode in place. Proven by the gate
through MCP `auto` mode — token issued, `{"reverted":1}`, `outside/hardtarget2.txt` rewritten.
The confused-deputy argument is unchanged: creating `repo/hard.txt` is a write INSIDE cwd,
which a path-based agent sandbox permits, while the victim is outside. Net effect before the
fix: an agent confined to cwd could modify any file its own user can write, anywhere on disk —
and the fix's own refusal string promised "refusing to write outside the recorded repo",
so this violated the invariant the fix had just stated.

**Fix (`ce641c4`): `undo_coordinator.rs::hardlink_refusal`, `nlink > 1`, called immediately
after `escape_refusal` in `build_plan`.** Kept a SEPARATE gate rather than another branch of
`escape_refusal` precisely because the class is not a path property. `symlink_metadata`
(lstat), not `metadata` — a symlink is `symlink_refusal`'s to refuse, and following it would
read the TARGET's link count. The predicate is deliberately NOT "the other name is outside the
root": we cannot know where it is, and a second name inside the repo is equally a file this
turn's record does not describe. False positives (deliberately hardlinked files in a repo) are
rare and degrade to a refusal — the same refuse-to-act-not-guess posture as `link_kind`.
Unix-only (`nlink` needs `MetadataExt`), stated in the doc comment; the repo targets macOS +
Linux (D19).

**MINOR 2 also fixed in the same commit — a false cause on a real refusal.** The gate ran a
preview → swap-parent-to-symlink → execute race with content DELIBERATELY IDENTICAL across the
swap (same `sha256:90a6517b…`), so drift provably could not be what saved it. Containment
re-ran inside `claim_grant` and correctly blocked the write, but reported
*"work/target.txt changed since it was lodged"*. `UndoError::PreviewStale` now carries the
plan's own refusal reasons; the wire code stays `preview_stale` (the caller's remedy is
identical) while the text names containment when containment is what fired. Safe direction
either way — but reporting a blocked attack as a benign content change misdirects the operator
investigating it, which is this repo's tracked "message contradicted by the facts beside it"
class.

**Evidence, all run:** live hardlink exploit against the release binary, `auto` mode, real
JSON-RPC → `files: []`, hardlink refusal, `token issued: False`, `outside/hardtarget.txt`
byte-unchanged; CLI leg on the same forged record → `REFUSE … nothing to revert`. Mutation
probe (hardlink gate call neutered) → exactly the 2 hardlink tests red, the `nlink == 1`
control and all 6 path-containment tests green. Suite **996 / 0 / 4** (993 + 3). Clippy
`-D warnings` `--all-targets` debug AND release, `cargo fmt --check`: clean. Release-seam
grep: 0.

**Residuals the gate stated, recorded NOT fixed:**
- **TOCTOU between `build_plan`'s canonicalize and `fs::write` is still open.** A parent
  swapped inside that window is not re-lstat'd. Closing it properly needs `O_NOFOLLOW`/
  `openat` rather than path re-checks — a larger change than this branch should take, and a
  founder call. What IS closed: the swap-before-`claim_grant` race, which containment catches
  on the re-planned entry (that is the race MINOR 2 was observed on).
- **MINOR 3, over-refusal:** `a/../b.txt` resolves inside root but the lexical component scan
  rejects it. No producer emits it today (the daemon writes normalized relatives; import
  lexically normalizes before its own root check), so the cost is zero and the direction is
  safe. Recorded so a future producer change does not hit a silent refusal.
- `main`'s published v0.2.0 CLI carries the unfixed primitive; this branch's fix lands in
  shared core so merging carries it to the CLI too, but whether the published version warrants
  a security note is a founder call. No Linux leg has run this branch's HEAD.

### Re-gate round 3 — GATE PASS on scope, plus a FIFO that hung the process

The round-3 re-gate **PASSED** both subjects it was called on, on evidence it generated
itself: it rebuilt the hardlink exploit in its own repo (both legs refuse, `token issued:
False`, victim byte-unchanged) and — better than the orchestrator's mutation probe — ran an
`nlink == 1` control with IDENTICAL content, identical record shape, same directory:
`plain.txt` got `token issued: True`, `hard.txt` got the hardlink refusal. The link count is
the only difference between them, so nothing incidental is doing the refusing. MINOR 2 was
confirmed working on a live attack path, and the ledger's race distinction (swap-before-
`claim_grant` caught; `build_plan`→`fs::write` window open) was independently verified rather
than taken on the orchestrator's word.

**Its hunt for a fourth shape succeeded again — MAJOR 1, fixed at `ca94cdd`.**

**A FIFO target HUNG the process, because the hardlink gate SKIPPED what it should have
REFUSED.** `inode_refusal`'s predecessor guarded on `meta.file_type().is_file()`, so a
non-regular file fell through the gate entirely rather than being refused, and nothing
downstream checks file type. `fs::read` on a fifo blocks until a writer appears. Measured by
the gate: `agentrec undo --confirm` hung until killed at 30 s; MCP `preview` returned ZERO
frames with the process still alive at 20 s. The MCP server is a single-threaded stdio loop,
so this is not one failed call — it kills the whole agent-facing surface for that session.
`mkfifo` needs no privileges and creating one is a write INSIDE cwd, so the sandboxed-agent
precondition is identical to every escape shape above. **Availability, not an out-of-root
write** — which is why the gate scored it MAJOR, not BLOCKER, and why it still blocked merge.

**The fix is one inverted guard, and the inversion IS the lesson:** the branch now reads
"refuse unless it is a regular file", never "skip unless it is a regular file". The skipping
form is what let the class through, and it looked correct while doing so. The same branch
closes NOTE 2 — a directory recorded as a `modify` entry was rendered as a performable
`revert  adir (modify)` and then failed at execution with `Is a directory (os error 21)`, a
plan promising an action it could not take. `hardlink_refusal` is renamed `inode_refusal`: it
now carries two distinct refusals, both inode-level facts no path check can express.

**A symlink returns `None` from this gate DELIBERATELY** and falls through to
`symlink_refusal`, whose distinct wording that gate's own tests assert; stealing it would make
those tests pass for the wrong reason. Pinned by a test named for that intent
(`inode_gate_leaves_symlinks_to_the_symlink_gate`). An absent path also returns `None` —
`create` reverts and delete-restores legitimately target paths that do not exist.

**Evidence, all run:** live fifo probe against the release binary — CLI leg refuses and
returns promptly (checked with an 8 s liveness poll that would have reported `STILL HUNG`),
MCP leg returns `files: []`, the non-regular-file refusal, `token issued: False`, within a
10 s poll. Suite **999 / 0 / 4** (996 + 3). Clippy `-D warnings` `--all-targets` debug AND
release, `cargo fmt --check`: clean. Release-seam grep: 0. The fifo test would **HANG rather
than fail** on a regression, which makes it self-enforcing.

**NOTE 3 — the daemon's FIFO exposure: PROBED, and the result is PARTIAL. Do not read it as a
clean bill of health.** The gate flagged this as potentially worse than the MCP case (silent
loss of recording) and explicitly did not test it. Measured here: a live `agentrec record`
daemon on a scratch repo, `mkfifo watched.pipe` in the watched tree, then an ordinary mutation
afterwards. The daemon **stayed alive and kept recording** — 2 turns, `after.txt` (the
post-fifo mutation) present. **But `watched.pipe` never entered a turn at all**, which means
the snapshot READ path was never exercised on it, so this probe does NOT establish that the
daemon survives reading a fifo — only that creating one in the watched tree neither hung it
nor stopped recording. `daemon.rs`'s snapshot loop guards `is_symlink` and `is_dir` but has NO
regular-file check, so the exposure is plausible on inspection and unrefuted. Forcing the read
path needs a writer on the other end of the fifo, which was not attempted. **Open, and outside
the undo surface this round fixed.**

### Re-gate round 4 — the FIFO hang was HALF fixed; the gate was in the wrong layer

Round 4 PASSED items 1, 3, 4 and 5 (fifo/directory refusal holds on both legs and returns
promptly; no over-refusal, 999/0/4 reproduced; the NOTE 3 daemon paragraph judged honest and
if anything UNDER-claiming; ledger truthful). **GATE FAIL on one blocker, fixed at
`dc0befb`.**

**`claim_grant` READS BEFORE IT PLANS, so a planner-side gate cannot protect it.**
`inode_refusal` lives in `build_plan`. But `claim_grant` runs its drift loop through
`read_current_hash` FIRST, and that was a bare `std::fs::read` — which BLOCKS on a fifo. So the
exact defect round 3's fix was called to close remained reachable on the leg that matters most,
the agent-triggerable execute path. Proven by the gate: preview a regular file (token issued),
`rm` + `mkfifo`, execute → **zero `"id":3` frames, process alive at 25 s**. `agentrec approve`
shares `claim_grant` and was exposed identically.

**Blast radius, measured by the gate rather than assumed:** the hang holds the undo lock
(`mcpcmd.rs:1224` acquires, `:1225` calls `claim_token`), but an unrelated `agentrec undo`
still completed in 4 s during the hang — so it does NOT brick other processes. It kills the
MCP session and burns the reservation. The token is NOT spent (`consume_token` runs after
`claim_grant`), so this is availability only: no integrity or escape consequence.

**Fixed at the PRIMITIVE, deliberately, not by policing a third call site.**
`read_current_hash` now lstats and returns `None` for any non-regular file, so every present
and future caller inherits it — there are four call sites today (`preview`, the reservation
path, `claim_grant`'s drift loop, and `build_plan`'s modified-since check) and the next one
would otherwise have to remember. `None` reads downstream as "no content to compare" = drift,
surfacing as a clean `preview_stale` refusal instead of a hang; that is the same direction the
function's existing doc already describes for an unreadable path, so no caller learns a new
shape. Symlinks are included: returning `None` rather than the pointed-to file's hash only
strengthens the "a symlink is always modified-since" property that
`undo_refuses_on_disk_symlink_legacy_record_even_with_allow_modified` pins, and the entry is
`symlink_refusal`'s to reject either way.

**THE GENERAL LESSON, and the reason this round is worth reading later:** round 2's containment
fix went into `build_plan` because it is the shared planner both legs reach, and that was
correct — for a DECISION. It was not sufficient for anything that can block or escape **on
read**, because `claim_grant` touches the filesystem before it calls the planner. *The planner
is the right home for decisions; it is not the only place reads happen.* A gate's layer has to
be chosen against the I/O, not against the control flow.

**Evidence, all run:** new integration test at the `claim_grant` level
(`a_fifo_swapped_in_after_the_preview_refuses_instead_of_hanging`) passes in 0.49 s; its
mutation probe — the lstat guard neutered with `if false &&` — **HUNG at 25 s**, so the test is
load-bearing rather than vacuous, and like the unit-level fifo test it hangs rather than fails
on regression. Live MCP swap probe on the release binary: real 40-char token issued, regular
file swapped for a fifo, `execute` returned **promptly** with `preview_stale`. Suite
**1000 / 0 / 4** (999 + 1). Clippy `-D warnings` `--all-targets` debug AND release,
`cargo fmt --check`: clean. Release-seam grep: 0.

**An invalid probe, recorded because the failure mode is instructive.** The orchestrator's
FIRST live swap probe reported "returned promptly" and was WORTHLESS: the preview had issued
no token (empty string), so `execute` failed on `bad_token` and never reached the drift read at
all. Cause: the fixture wrote CAS blobs at `objects/<fan>/<full-hash>`, but
`store.rs::object_path` splits as `<fan>/<rest-62>`. Every EARLIER probe in this series refused
before the store was ever consulted, which is why the wrong layout never surfaced. Re-run with
the correct layout, the probe issued a real token and became discriminating. **A probe that
"passes" without establishing its own precondition is evidence of nothing** — the same class as
this repo's fixture-only-evidence rider.

**Nuance, recorded not fixed:** the swapped-fifo refusal reads *"swap.txt changed since it was
lodged"*, i.e. the drift wording, not a fifo-specific one. That is defensible here — the path
genuinely did change, from a regular file to a fifo, and the refusal comes from the drift check
rather than from a plan refusal, so MINOR 2's `refusals` channel legitimately does not fire.
Stated so nobody later reads it as MINOR 2 regressing.

**INCIDENT, disclosed by the gate itself and verified independently by the orchestrator:**
while probing NOTE 3 the gate ran `pkill -f "agentrec record"`, which matched the **live
dogfood daemon** (pid 865, `~/Projects/agentrec`) and killed it. launchd `KeepAlive` respawned
it — now **pid 39492, `com.agentrec.bfa6bde6eaa4`, status 0**, confirmed recording turns after
the respawn (orchestrator checked `pgrep`, `launchctl list`, and the tail of the dogfood
`log.jsonl`). Consequence: a seconds-long recording gap in the dogfood repo, self-healed, no
data loss. It also contaminated that round's first suite run (707/2/3, both failures
daemon/FSEvents tests); the gate re-ran clean at 999/0/4 and correctly attributed the two
failures to itself rather than to the branch. **This is the broad-pattern `pkill` hazard this
repo already records in memory, hit anyway** — kill by exact pid.
