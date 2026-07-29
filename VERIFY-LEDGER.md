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
| Memory: 10k/50ms hard perf envelope | F2 (2026-07-12) made `RECALL_BUDGET_MS`=50 a genuinely **hard** cooperative deadline — `cmds::inject_memory` threads `deadline: Instant` into `memory::recall_with_deadline`, which checks it at each `load_effective`/`bm25_rank`/verify loop boundary and bails to an empty, `budget_exceeded`-flagged result the instant it passes, instead of the old measure-after-the-fact suppression. This closes the *correctness* gap (a slow recall can no longer delay the prompt unboundedly — proven deterministically by `hook_recall_bails_at_injected_deadline` via a test-only already-expired-deadline injection, no runner-speed coupling) but the *10k-record p99 latency envelope itself* is still timing-dependent and still not CI-provable: CI proves exit-0 + (now, typically faster) bailout at 3000 records within a 500ms slack bound, not the true steady-state 50ms envelope at 10k records on a release build. | A dogfood-machine timing run: 10k-record store, `agentrec hook` p99 recall < 50ms (or block correctly suppressed past budget, now via the hard deadline rather than a race). Marked LADDERED in IMPLEMENTATION.md INV-M4, never claimed CI-proven. |
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
| peak RSS | **16.7 MB** (AC7 bar is <500 MB) |

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

**Still open after this row:** T2-candidate bytes are detected, never resolved (P1 scope — the
963 candidates are a ceiling, not a proven recovery rate; P2 resolves git blobs and will
convert some fraction of them to real recoveries and the rest to T3).

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
