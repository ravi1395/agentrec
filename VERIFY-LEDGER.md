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
| purge --memories-retracted (first of the three sanctioned rewrite classes; see D46) | `purge_archives_only_expired_retracted_chains` (archive-fsync-before-rewrite atomic; survivors byte-identical; second-run no-op), `purge_memories_retracted_refuses_while_daemon_running` (daemon-liveness flock guard — no lost concurrent append). |
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
