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
| Memory: 1-week dogfood + skill-driven candidate | `status` counters (`memory: N fresh, M stale, R rejects, I injections`) and the candidate emitter + `agentrec-memory` SKILL.md are code-complete + tested; real-world hit-rate, no-false-staleness, and ≥1 useful skill-emitted memory need live use over a week. **BASELINE PREPPED (2026-07-17):** store reclaimed 3.4 GiB→775 MiB (orphan-GC, under budget, `status` clean), `agentrec-memory` SKILL installed to `.claude/skills/` (candidate emission now enabled — was staged-only), memory store re-pinned + seeded to 3 fresh / 0 stale (recall verified), `doctor` all-pass incl. hook-presence. The 1-week clock can now start honestly. **CLOCK STARTED T0 = 2026-07-18** (PR #5 merged to `main` at `8079299`; run on this repo's canonical `main`). **T0 baseline:** daemon live (launchd `com.agentrec.bfa6bde6eaa4`, RunAtLoad+KeepAlive, boots at login), `store 793 MiB` (under budget), `gaps 0`, `memory: 3 fresh / 0 stale / 0 rejects / 13 injections / 0 failures`, `rich-rate 60% trailing-20` (recovering as build churn quiets — honest-bare, `doctor` hook-presence pass), 3 seeded facts re-pinned to canonical `main` content after the rebase drifted `purgecmd.rs`. **Window closes ~2026-07-25.** Check at close: hit-rate non-trivial, DEGRADED/staleness never falsely fired (distinguish real file-drift staleness from false), and ≥1 `agentrec candidate` (SKILL-emitted) became a genuinely useful recalled memory. | One week recording in this repo with the SKILL installed: `status` shows a non-trivial hit-rate, DEGRADED/staleness never falsely fires, and ≥1 agent-emitted candidate becomes a genuinely useful recalled memory. |
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
| purge --memories-retracted (only sanctioned rewrite) | `purge_archives_only_expired_retracted_chains` (archive-fsync-before-rewrite atomic; survivors byte-identical; second-run no-op), `purge_memories_retracted_refuses_while_daemon_running` (daemon-liveness flock guard — no lost concurrent append). |
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
_(see the Open table near the top.)_ **J3 (CI matrix), I++1 (Linux perms), Y++2 (inotify induced-low-watches), Y+1 (Ubuntu + macOS clean-box installer legs), Y+2 (brew tap), the README blame GIF, and — as of 2026-07-17 — the 7-night D36 streak all closed** — repo published at https://github.com/ravi1395/agentrec, v0.1.0 tagged and released, Actions matrices green. Every M3-era launch gate is now closed; the remaining open rows are the memory-v1 ladder (10k/50ms envelope, real-session injected block, 1-week memory dogfood + skill candidate) and D11 service-reload, all in the Open table above.

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
