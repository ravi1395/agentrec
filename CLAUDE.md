# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working in this repository.

## What this is

**agentrec** — a local-first, tool-agnostic flight recorder for coding agents. It segments agent activity into *turns*, snapshots every touched file into a content-addressed store, and answers "who broke my repo — me or the agent?" via `log` / `diff` / `blame` / `undo`. The turn engine is an extraction of the production-proven `turns.rs` from Sutra (`~/Projects/sutra/src-tauri/src/turns.rs`) — when in doubt about engine semantics, that file is the reference implementation.

## Status (update after every delivery round — house rule)

**Phase-2 spec hardening — SPEC-GATE PASS after 3 rounds (2026-07-28, `main`, docs-only, zero
code changed):** An adversarial value analysis of the shipped product (same skeptic agent
throughout, measuring the live store rather than arguing theory) found **agent-facing benefit
today is ~zero** — no MCP surface exists, the memory injection surface delivered 389 injections
of 3 facts (2 now stale), the store is 9% real-source signal (800 of 8825 file entries), and the
flagship `undo` verb has **0 confirmed production uses in 17 days** — and a follow-up Phase 2
threat analysis found every dependency-creating deliverable (MCP read/destructive) speculative
while import is structurally capped by source retention. **Four founder decisions (5–8) were
taken and written into `docs/superpowers/specs/2026-07-18-agentrec-phase-2-design.md`:**
(5) Protocol 1.0 freeze moves behind Phase 2.1 — a format freezes only after a second emitter
validates its shape; 2.0 only starts `FORMAT-CHANGELOG.md`. (6) MCP destructive is
evidence-gated: ≥20 human-confirmed `undo --confirm` in real use (today: 0; counter =
`log.jsonl` undo turns, window pre-2.3 by construction, `cli|mcp` origin discriminator owed in
2.3's first commit, indefinite deferral an accepted outcome) AND `allow_modified` is never
honored in auto mode — the modified-since rail is human-approval-only, closing the
agent-grants-itself-the-override hole. (7) MCP read is gated on a CLI demand probe: CLAUDE.md
recipe + **transcript-sweep audit** (deliberately NOT an in-product counter — that would add an
unsynchronized `state.json` writer and contradict the pinned zero-write `status --json`
behavior), ≥10 audited unprompted invocations across ≥3 non-agentrec-repo sessions; treatment
site named (`~/Projects/sutra` at minimum — which **has no `.agentrec/` today**, so `agentrec
init` there is a recorded prerequisite before the probe window can open). (8) The import gate
gains a fidelity report + VERIFY-LEDGER row (per-tier revertibility, opaque-call share 2.49:1
baseline); threshold deliberately unset until the first real measurement. **Corpus decay
measured firsthand and specced:** Claude Code's `cleanupPeriodDays` (default 30) makes the
import corpus a rolling ≤30-day window — 1600 top-level transcripts on this machine, oldest
exactly 30.0 days (613 sidechain files counted separately) — so import is re-pitched
"≤30-day backfill", the "cold-start killer" ROADMAP line owes a rewrite (companion row, rides
P1), and the "scheduled re-import = durable archive" claim is **embargoed** until a re-import
mechanism actually ships (a value claim with no mechanism is the overclaim class this round
existed to kill — the skeptic caught the fix itself reintroducing it, B3). Spec ladder amended
3→4 tiers (T1 42.5 / T1.5 25.3 [518 of 2044; the 508/508 figure is a resolve-rate check with
its own denominator] / T2-candidate 24.5 / T3 7.7; honest figure 67.9% measured on unrounded
counts, rounded shares sum 67.8; 92.3% stays banned). Plan
(`docs/superpowers/plans/2026-07-25-agentrec-phase-2-0.md`) gained decisions 8–11, the
fidelity + rolling-denominator ACs on P1, and a **stale-ladder warning** (its 349-baseline
counts predate later rounds; `main` baseline 428/0/1 re-verified at `834f477`, the 443/0/1
figure belongs to the unmerged `fix/perf-evidence-round` branch — per-phase deltas are the
plan's real content, re-base at chunking time). **Gate history worth keeping: round 1 FAIL
(7 blocking — B1 the probe was vacuous as written [recipe-induced calls are by definition
prompted, dogfood sessions self-satisfy], B2 the probe counter contradicted the read-only
guarantee and had no writer, B3 durable-archive was a pitch with no mechanism, B4–B6 surviving
text still instructed executors to build gated work, B7 the undo gate named no counter/scope),
round 2 FAIL (F1 the probe's treatment site was unspecified and clause (a) disqualified the
only natural install location — an uninstalled treatment would have killed 2.2 as "no demand";
F2 ROADMAP's Phase 2 gate is unsatisfiable as written, extension clause stale since decision 1;
plus the skeptic WITHDREW its own 67.9-vs-67.8 finding after computing from the raw audit —
gates run both directions), round 3 PASS.** The recurring lesson, again: the decisions were
sound from round 1; every FAIL was in the **enforcement surfaces** — downstream sentences,
delivery vehicles, measurable definitions. Nothing implemented; next step unchanged
(`/chunker` on the perf-evidence plan, or Phase 2.0 P1 once the founder sequences it).

**Perf-evidence plan — PLAN-GATE PASS after 6 revisions (2026-07-28, `main`, docs-only, zero
code changed):** `docs/superpowers/plans/2026-07-27-agentrec-perf-evidence-round.md` — 4 phases
(recall `elapsed_ms` + `memories --stats` + the 10k ledger measurement; dedup-hit counters on the
snapshot path; `retention` plan/execute split; eviction relocated from `status` to a daemon tick),
~9.5–11.5 h wall estimated, gap 4 (inotify watch pruning) explicitly deferred with the race-class
reason. **The plan itself was skeptic-gated to a PASS verdict as a hard finalisation condition
(founder /goal), and it took 5 FAIL rounds — 12 blocking findings, every one real and
refutation-argued against the actual code.** The instructive ones: (B8) the protected-bytes
honesty line in `status` is *manufactured by the destructive pass* — `enforce_budget` had no
dry-run, so a read-only `status` would silently kill a shipped honesty surface and red a test the
plan never named; (B9) a live-daemon fixture seeding `open.json` is destroyed by the daemon's own
`sync_journal` idle arm within ~250ms — protect-channel fixtures must use files the daemon only
appends to; (B10) "freshly-staged blob survives the tick" is vacuous — staged blobs aren't in
`log.jsonl` until the turn closes, so they are never candidates (the SR6 class, caught at plan
time instead of shipped); (B11) after splitting plan/execute, the *reference timestamp* of
execute's freshness re-check was unspecified — fresh-`now()` is strictly weaker than the unsplit
code in a hard-delete path, and the AC's offered future-dated bump could not tell the designs
apart; (B12) the both-halves guard fix then *masked its own plan-side neuter* from every
deletion-observable test, requiring a plan-report-leg discriminator. **Net: three of the five
FAIL rounds found defects in fixes to earlier findings — the loop converged inward, which is what
distinguishes a gate from a rubber stamp.** Founder decisions recorded in-plan: Q1 = (a)
(eviction moves to the daemon; `status` becomes a pure read verb; daemon-down + `undo` growth
accepted as rare/bounded/self-announcing), Q2 = default (a breached 10k p99 leaves the ledger row
OPEN-with-figure; no criterion loosened). Plan baseline 428/0/1 was re-verified by the gate's own
suite run at `834f477`, and the gate's parting scope note is preserved in the plan header: two
things only the code-level skeptic can settle (unbroken harvest→plan→execute call site; AC2b.2's
timing-coupled ~2s-seam leg, solo + repeated runs required). Also this session, pre-plan: PR #7
squash-merged (`5ea946b`) — both P5 ledger rows closed by its CI run (`834f477` records this; the
"per-phase history kept" draft claim was corrected to squash-merge reality before commit); 2
leaked test daemons killed; 7 remote + 2 local branches deleted (13 stale locals blocked by the
git-guardrails hook, command handed to founder); read-verb latency measured on the live store
(`log`/`blame` 10ms @ 2006 turns — the number that killed log-indexing as a gap). **Nothing
implemented; next step is `/chunker` on the plan.**

**Residuals round — GATE PASS (2026-07-27, branch `fix/residuals-round` cut from
`fix/honesty-round`, `3477781..8f89775`, not pushed, no PR):** All 5 phases of
`docs/superpowers/plans/2026-07-26-agentrec-residuals-round.md`. macOS **422 → 428, 0 failed,
1 ignored**; clippy `-D warnings` + fmt clean **debug and release**; release `strings` carries no
`AGENTREC_TEST`. Sonnet implementers one per phase, orchestrator re-ran the full suite after every
phase independent of every implementer (each count agreed twice), one binding **fable
skeptical-reviewer** done-gate in an isolated worktree. **Verdict PASS on the first pass** (as the
ignore-rebuild-gate round also did — an earlier draft of this entry called that a first in this
repo's history, which is false; the rounds that needed a FAIL→fix→re-gate loop were blame-honesty
and honesty-fixes), with 3 NON-BLOCKING findings, all fixed at `8f89775` rather than recorded,
because two were false claims in shipped artifacts.
**The measurement-first ordering is what earned the pass, and it should be the template.** Phase 1
was a pure spike: no repo code, a standalone probe against the raw `notify` crate, 4 probes, ~100
trials, ending in a machine-checkable `VERDICT:` token that Phase 2's entire shape branched on.
**(P1) `4b9e6da`.** Probe A **10/10** rename-ins deliver `Modify(Name(Any))`; Probe B **0/80**
(2 fixtures × 4 stimuli × 10) spurious rename kinds; Probe C **0/5** delayed replays over a 68s
hold; Probe D **5/5** rename-outs deliver with `is_dir()` false. **The load-bearing result is the
one nobody asked for:** Probe B's `touch` stimulus produced a spurious `Create(Folder)` on **2/10**
fresh-fixture trials — independently reproducing the prior round's fabrication measurement on a
brand-new harness. That is what makes the 0/80 rename null trustworthy rather than a dead watcher
reporting silence: the probe demonstrably sees coalesced historical flags. Create axis poisoned,
Name axis clean — exactly the discrimination the fix needed.
**(P2) `3b3062b` — the headline defect, closed.** A directory **moved into** the watched root lost
its contents on macOS (3/3 at HEAD, 2/2 at the pre-round baseline — pre-existing, not a regression),
the sharpest known silent-loss hole in the core claim, on the majority platform. `admit_existing_
contents` is no longer `#[cfg(target_os = "linux")]`; the **gate** is now platform-split instead:
Linux keeps `Create(_) | Modify(Name(_))`, macOS admits on `Modify(Name(_))` **only**. Decisions-log
#2 survives intact and is directly pinned by `create_kind_does_not_admit_on_macos`.
**(P3) `bd92964`** liveness-gates the stale reload line: `release_lock` never runs on kill-9, so
`epoch_nonce` stayed stamped and a **dead** epoch's rebuild count rendered as current in *both*
readers. `status` is now the fourth caller of the proven `daemon_is_running` flock probe. Founder
answered open question 3 with **option (c)**: json keeps the count and gains an always-present
`epoch_ignore_rebuilds_stale` boolean; the text line is suppressed outright.
**(P4) `d940ac9`** closes the `wait_for_live_daemon` race — the pid was written ~4.3ms before the
watcher armed, so every live-daemon test nominally raced it. The daemon now stamps
`watcher_armed_nonce` after `.watch()` succeeds; **keying on the epoch nonce rather than a bool is
what makes a crashed run's stale value self-invalidating.** **(P5) `31a2eba`** adds
`workflow_dispatch` (founder answered Q1 with **(b)+(c)**) plus `scripts/linux-leg.sh` codifying the
Colima ritual, and makes the rebuild bound *evidenced on green* — the count previously surfaced only
in the assert-**failure** message, and cargo captures stdout on pass, so a green CI run carried no
number at all.
**The skeptic ran the one thing no fixture can prove.** Live daemon, real repos, agent bracket open:
`touch <pre-existing dir>` **5/5 trials, 0 fabricated entries**, `blame` naming nobody — **plus a
control-included trial** (`fabricated=0 control_recorded=1`) proving the daemon was recording during
the exact window the absence claim covers, which is the vacuity objection this repo has been burned
by. And `mv dir-in` **5/5, all contents recorded**. A single clean run was explicitly not accepted.
**Two false claims shipped inside this round and the gate caught both — worth keeping.** (a) The
rename-out test's comment claimed "two independent discriminators" unconditionally. **False on
macOS**, demonstrated by the skeptic: the unconditional `admitted_dirs` clear for any
`Modify(Name(_))` runs **before** the admission gate, and macOS's follow-up leg is itself
`Modify(Name(RenameMode::To))` — so that clear wipes the leg-1 poison, `insert` returns true, and the
staging assertion **passes even under the neuter**. Only `is_empty` discriminates on macOS; both do
on Linux. The clear it collides with is *last round's own fix* (`f4bca8a` finding 2) — i.e. the plan
reasoned about a neuter without checking its interaction with the code the previous round added, the
"reasoned safe, unobserved" class this round existed to kill, reproduced inside the plan meant to
close it. The implementer disclosed it unprompted rather than working around it. (b) A VERIFY-LEDGER
row cited a "Local Colima-VM number … recorded below" that **exists nowhere**, while the same row
stated the container was never run. The only figure measured was a macOS one, which is not a data
point for a row about Linux.
**A third finding was a real host-mutation bug in a script that had never been executed:**
`scripts/linux-leg.sh` ran `chown -R` on an **rw bind mount**, rewriting the *host* repo's ownership
through virtiofs, and put the container's `target/` on the host's own non-triple `target/debug`
path, clobbering the macOS build cache. Now mounted read-only and copied to container-native storage
with `target/` excluded, `CARGO_TARGET_DIR` off the mount. Its `sysctl` mechanism now carries an
explicit **UNVERIFIED-inference** caveat — the round recorded the *value* (1048576), never how it was
applied, and the implementer flagged its own reasoning as inference rather than recovered history.
**Process debt, recorded not papered over: claimd declare-first held for P4 and P5 only.** P1–P3
were implemented with no claims declared beforehand; `claimd lint --range` reports **two**
touched-but-uncovered files (`cli/src/cmds.rs` from P3 and `scripts/linux-leg.sh` from P5 — the
orchestrator's own self-report to the gate named only the first, and the skeptic found the second).
Retroactive declaration was **refused**, per the skill: a declare-record postdating the code makes
the log lie about ordering, the one property claimd provides. `amend` cannot widen `stale_on`
either, so the P5 manual claim — declared *before* implementation but with the script omitted from
its globs — could not be honestly corrected. **`scripts/**` was added to `.claims/config.json`'s
`lint.ignore` at the FOUNDER's explicit direction** (contrast `IMPLEMENTATION.md`, which the prior
round records as agent-added and flags for review); the Stop hook was otherwise firing on every
turn-end with no legitimate remedy available. `cli/src/cmds.rs` is deliberately NOT ignored — it is
core source, and it stays standing as recorded debt in the same posture as last round's
`purgecmd.rs`. **The underlying rule is still undecided and will fire again:** `PROTOCOL.md` is not
ignored, so the next normative-doc edit reproduces this; and the argument *against* ignoring
`scripts/` is that this very script shipped two real defects while unexercised. 8 claims declared this round (4 for
P4, 3 for P5, plus the P4 manual), 5 EVIDENCED, 2 left DECLARED as `manual` awaiting founder
attestation — **never self-attested**. A pre-existing `illegal transition: evidence from EVIDENCED
at seq 4` warning on `clm_0TRSZXYXZBBV092VF8D1RXF7PD` (an ignore-rebuild-gate-round claim) predates
this session entirely; two independent implementers traced it to the same claim.
**Environmental incident worth recording, because it wasted a phase and the wrong diagnosis was
tempting:** mid-round every shell died — `posix_spawn failed: Resource temporarily unavailable`,
symmetric across the orchestrator, a subagent's `clang` linker, and background spawns. This repo's
memory records leaked `agentrec record` daemons as *the* known process leak, so that was the obvious
suspect; it was **wrong**. The machine held **3607 orphaned `tail -f -n +1 /dev/null`** processes
(ppid 1, one-second burst) against `kern.maxprocperuid=4000`, and exactly **one** agentrec daemon was
running — the legitimate launchd service. Contributing cause was ours: P1's first harness revision
created a fresh `notify` watcher **per trial** (80 for Probe B alone) and exhausted the process table
around trial 9. Redesigned to one watcher per probe, which also matches how the real daemon behaves.
**Every measurement taken before the cleanup was discarded and the trial counts restarted from
zero** — a fork-starved machine perturbs FSEvents delivery timing, which is precisely the quantity
P1 exists to measure.
**Still open, none blocking:** P5's AC1 (a GitHub run URL) is **structurally unclosable from a
branch** — GitHub only offers `workflow_dispatch` for workflows already on the default branch — so
it is an honest OPEN ledger row, not a pass; landing the trigger on `main` is a founder decision and
nothing was pushed. **Superseded 2026-07-27:** the first Linux rebuild-bound number now exists —
`scripts/linux-leg.sh` was executed for the first time (Colima, non-root, overlayfs fixtures, suite
**431/0/2** green): in-suite count **8** at `--test-threads=3`, solo runs 9–12, all inside `1..=45`
but *below* the prior round's 13–21 from a different VM — the bound holds in both regimes, nothing
explains the gap. The first run also found that the shipped script **silently defeated P5's own
evidence-on-green deliverable**: no `--show-output`, so libtest captured the passing test's
`rebuild_count` print — the pristine log carries zero such lines (fixed in the same commit as this
entry, plus a ran-to-completion sentinel after a real silent-truncation false-pass; a
docker-credsStore workaround was applied env-only, not committed; ledger row updated). A second
binding fable skeptic gate over the post-gate commits (`8f89775..`) returned **FAIL → all four
findings fixed at `357bde3` → re-gate by the same skeptic: PASS** (each closure refutation-proven
firsthand, incl. a fresh-boot probe of the Colima VM confirming 1048576 is the boot default — the
claim the skeptic most suspected — and a fourth same-VM rebuild count, 13): (F1, blocking) the same commit that recorded the first execution
shipped the script still claiming `STATUS: THIS SCRIPT HAS NEVER BEEN EXECUTED` — the
false-claim-in-shipped-artifact class, reproduced by the fix for it; (F2) the sentinel cleared
itself *inside* the truncation-vulnerable block, so a truncation landing before the clear plus the
stale sentinel every green run leaves in the persistent volume produced a demonstrated live false
pass — clear moved host-side into its own container run; (F3) the skeptic's own in-suite run on the
same VM measured **14** and the post-fix re-run measured **19**, refuting this entry's earlier "two
VMs, two regimes" framing twice over (the 14 and 19 both fall entirely inside the prior VM's 13–21;
this VM's observed span is 8–19, and the re-gate's own run added a **13** at the prior range's exact
floor; spread uninstrumented, all inside `1..=45`); (F4)
`e3d24d7`'s subject says "30 stale claims re-confirmed" — the true count is **44** (44 STALE →
44 CONFIRMED, 0 refuted, no manual claim self-attested; message immutable, corrected here). After
the re-gate PASS the branch was pushed and [PR #7](https://github.com/ravi1395/agentrec/pull/7)
opened (65 commits, four stacked rounds) and **squash-merged** to `main` as `5ea946b` on 2026-07-27
— per-phase history survives only in the PR, not on `main` (an earlier draft of this entry, written
before the merge, claimed "per-phase history kept") — and its `pull_request` trigger
closed **both** structurally-blocked P5 ledger rows the same evening: run
[30309231117](https://github.com/ravi1395/agentrec/actions/runs/30309231117), all 5 jobs green,
ubuntu legs 431/0/2 each, and the `--nocapture` step printed `rebuild_count observed: 11` on both
ubuntu runners — the "AC1 is unclosable from a branch" analysis was true for `workflow_dispatch`
but missed that a PR *is* the branch-reachable trigger; the rows closed by the route the round
declined to take (a PR), taken later with founder approval. P4's population-level flake claim closes only over CI history. P1's
verdict is bounded: the "aged" fixture is `rsync`-copied seconds before the watcher attaches, so it
is aged in tree shape but **not** in per-path FSEvents journal history — weeks-old production
directories remain unprobed and unprobeable by fixture; the dogfood daemon on this repo is the
natural observatory. Two leaked test daemons from prior sessions (pids 26258, 70800, watching dead
tempdirs) were live during the review and left running.

**Epoch nonce + the Linux leg — GATE PASS (2026-07-26, branch `fix/honesty-round`,
`f8c6cc9..f4bca8a`, pushed, no PR):** Two founder asks. The nonce was routine; **the Linux leg,
recorded as "owed" for three rounds, found a shipping product defect within minutes of first
running.** macOS **419 → 422**, Linux **first-ever run: 316 passed / 2 failed → 426 passed / 0
failed / 2 ignored**. Both platforms re-run by the orchestrator after every commit, independent of
every implementer. Binding fable skeptic: **FAIL → FAIL → PASS** across three rounds.
**How Linux was run, since this is now load-bearing infrastructure:** `ci.yml` fires only on
`push: branches: [main]` and `pull_request` and has **no `workflow_dispatch`**, so a branch push runs
nothing and GitHub only exposes dispatch for workflows already on the default branch — a PR was
declined, so the leg ran locally in a **Colima Linux VM** (`rust:1-bookworm`, arm64, **non-root**,
real inotify, `max_user_watches=1048576`). Fixtures land in the container's own `/tmp`, so the daemon
watches native overlayfs, not the virtiofs mount. **Run it non-root**: as root, `chmod 000` fixtures
pass vacuously because root bypasses mode bits — that alone made the two `#[cfg(unix)]` permission
tests fail spuriously on the first attempt.
**(A) `942985d` epoch nonce.** Epoch identity was the **pid**, which pid reuse defeats: reader and
writer keyed on the same comparison, so a recycled pid made a dead epoch's reload count render as
current *and* made the new epoch's first rebuild accumulate onto it. `acquire_lock` now stamps
`epoch_nonce = ulid()` (a bare ms timestamp can collide across a rapid record→stop→record cycle);
`release_lock` clears it; a pre-nonce `state.json` renders nothing rather than claiming a stale count.
Refutation-proven with **writer-only and reader-only** reverts, the asymmetry that caused the
original bug.
**(B) `034c883` — the defect Linux found, and it is a real one.** `notify` arms the watch for a
**newly created directory** only after processing the batch containing its creation event, and
inotify is edge-triggered with no catch-up — so **a file written into a brand-new directory before
the watch is armed was never recorded. Permanently, silently.** Proven with an isolated repro against
the raw `notify` crate (no agentrec code) plus instrumentation showing the watcher arms ~4.3ms after
the pid is written, which kills the competing "startup scan catches it" explanation. Fixed by
`admit_existing_contents`, which walks the new subtree and stages it. A dedicated test names the
defect, because the only assertion previously pointing at it lived inside a test entirely about
ignore-set rebuilds — it would have been misattributed forever.
**(C) The fix fabricated attribution — twice — and the gate caught both.** `247df9e` and `f4bca8a`.
First shape gated on `path.is_dir()` alone, so `touch src` / `chmod src` walked whole unchanged
subtrees and emitted every file as `op:"modify"`; with a bracket open they folded into the rich
`tool:"claude"` turn and **`blame` named the agent for files it never touched** — a macOS regression
at the product's headline surface, undoing what `17657d9` spent a gate cycle fixing. The kind gate
that followed **also failed**: FSEvents delivers **coalesced per-path flag unions**, so the first
event for a directory that pre-dates the daemon routinely carries historical `ItemCreated`, and
`admitted_dirs` is empty for every such directory — i.e. essentially the whole repo. Skeptic measured
**2 of 4** bracket trials fabricating where the implementer had reported one clean run; **a single
clean observation of a ~50% behavior**, twice in this round. Final shape: admission is
**`#[cfg(target_os = "linux")]`** — it only exists where it is load-bearing (premise verified: 10/10
tight `mkdir && write` bursts recorded on macOS with admission compiled out), plus a rename clear,
since `IN_MOVED_FROM` maps to `Modify(Name(From))` and **not** `Remove(Folder)`, so a directory
renamed away left a stale `admitted_dirs` entry that suppressed a legitimate later admission —
reopening the very event-loss class for `mv dir dir.bak && mkdir dir`. Escalation now **5/5** clean.
**The measurement that should change how these plans are written:** Linux's *correct* rebuild count
(13–21) **exceeds macOS's *neutered* count (15–18)**. A bound that discriminated perfectly on one
platform was worthless on the other, and the prior reviewer's reasoning about which way inotify would
push it was exactly backwards — FSEvents coalesces, inotify does not. `elapsed/POLL` is not a usable
derivation either (`recv_timeout` returns immediately when a message is queued). Shipped as a
`cfg(target_os)` split, each side discriminating its own neuter (Linux neuter measures 87–103).
Every "reasoned safe, unobserved" margin in the last three rounds rested on that class of intuition.
**Skeptic ruling worth keeping:** `metadata_only_event_does_not_admit_existing_directory_contents` is
now vacuous on macOS, and that is **acceptable un-gated** — unlike this repo's past vacuity failures,
it is vacuous only where the guarded code *cannot compile*, so it masks nothing, and it snaps back to
load-bearing if anyone deletes the `#[cfg]`. Real macOS coverage lives in the pre-existing-dir test.
**New residual, measured at HEAD *and* at the pre-round baseline — pre-existing, not a regression:**
on macOS a directory **moved into** the watched root loses its contents (3/3 at HEAD, 2/2 at
`f8c6cc9`); the round fixes this case on Linux via the rename-in clause. It is now the sharpest known
silent-loss hole in the core claim, on the majority platform. Durable fix would be macOS admission
triggered by rename events only, or a scan-diff on rename — founder call. Also still open: the
crashed-daemon stale reload line, the bound's single-VM statistics, and `wait_for_live_daemon` gating
on a pid written ~4.3ms before the watcher arms (every live-daemon test nominally races it).
**Process note, unflattering and worth keeping:** I repeated within one session the exact error that
produced this round's first REFUTED claim — two positional filters to `cargo test`, which takes one.
Caught only because I read the output rather than the exit code. **The whole admission mechanism is
now testable only on Linux**, so the container leg is load-bearing from here on, not optional.

**Honesty-fixes round — GATE PASS (2026-07-26, branch `fix/honesty-round`, `df442f2..0f3b474`,
pushed to `origin/fix/honesty-round`, no PR):** Founder-directed sweep of the five items the
rebuild-gate report left open. **395 → 414 tests, 0 failed, 1 ignored**, clippy `-D warnings` + fmt
clean debug and release, release `strings` carries no `AGENTREC_TEST` seam. Sonnet implementers, no
per-phase reviewer this round, one binding **fable skeptic** — plus an orchestrator re-run of the
full suite after every phase, independent of every implementer. Round-1 verdict **GATE FAIL** on one
blocking finding, fixed at `9b1b09b`; re-gate **GATE PASS**.
**(P1) `2d1bd78` — `enforce_budget` deleted live-cited blobs, and did so from a read verb.** Its
keep-set came from *parsed* `TurnRecord`s while it hard-deletes (`store.remove` → `fs::remove_file`),
so `open.json` in-flight refs, `memory.jsonl` pins and torn-line refs were all unprotected — and its
sole caller is `cmds::status_report`. **Running `agentrec status` on an over-budget store could
destroy an in-flight turn's snapshot.** Fixed with an `extra_protected` set harvested as late as
possible before the remove loop. **The obvious implementation is a trap:** passing the full
`purgecmd::referenced_hashes` protects every validly-referenced hash and **permanently defeats budget
eviction** — it broke `status_prints_over_budget_notice`. Correct cut is `open.json` + `memory.jsonl`
scanned raw, plus only those `log.jsonl` hashes on lines that **fail to parse**; validly-parsed lines
are already covered by the age-ordered walk. Hard-delete deliberately retained (archive-on-evict
frees zero disk while *reporting* bytes freed, from a habitually-run read verb); no liveness refusal
(the daemon runs 24/7, so a gate makes the budget fiction). Skeptic proved the cut correct in both
directions on a live daemon: `freed 19 B; 500 B protected`, blob survived.
**(P2) `7474e26` — `read_state` reset the entire `State` when one field failed to parse**, losing
`pid`, the honesty counters, and **`signal_offset`, whose reset replays the whole signal inbox** —
silently, with no counter. Now parses via `serde_json::Value` with per-field extraction and a
`state_parse_failures` counter surfaced in `status` and `doctor` (advisory only — `doctor` all-pass
exit 0 is the deploy gate). **The plan's taken decision was wrong and the implementer said so:**
`#[serde(default)]` rescues a *missing* field only; a struct-level parse still fails outright on a
*present* field of the wrong type, which is exactly what every criterion required to survive.
**(P3) `22b085f` — epoch-scoped the reload counter** (open question 1 answered with option (a),
reversing last round's default-by-omission) and made `status --json` carry every DEGRADED field the
text banner reports; `status --ack-degraded --json` now rejected by clap instead of printing prose
under a `--json` flag. **(P4) `6234612`** pins the over-record asymmetry the previous gate named:
classification happens at ingest but staging at flush, so a path already in `pending` under wider
rules is still staged after a mid-debounce narrowing.
**The blocking finding is the one worth remembering.** P3's epoch reset lived only on the *writer*
side; `status_report` rendered the raw field. So after a restart — until the next `.gitignore` churn,
potentially days — `status` attributed the **dead epoch's** count to the live one, and `epoch_pid`
already held the dead pid on disk with nothing reading it. The README line shipped that round ("the
text line resets on each daemon restart") was therefore **false as observed**. Fixed at `9b1b09b` by
gating the render on `epoch_pid == pid`, **not** by rewording the AC or the README — the ratchet
forbids loosening a criterion to pass. Render-gate was chosen over resetting at `acquire_lock` for a
reason the plan missed: `release_lock` zeroes `pid` and never touches `epoch_pid`, so a writer-side
reset fixes the restarted window and leaves the **stopped** window stale.
**A hole in that fix, found at re-gate and closed at `0f3b474`:** `status_report` and `status_json`
are independent readers, and neutering **only** the json seam survived the entire suite — the test
asserted the json *lifetime* total but never the epoch figure. Now pinned; the json-only neuter reds
it by name.
**Four process failures this round, all mine, none by the implementers.** (1) The plan was wrong
three times and each implementer caught and disclosed it rather than working around it — including
that **Phase 4's own named neuter cannot fail** (`gitignore_dirty` is tracked independently of
`pending`), the third round running in which an AC could not have proven itself as written; a
trigger-neuter was substituted with disclosure. (2) A second claim came back **REFUTED**
(`clm_5PCCRY1K`) — again my replay, not the code: the `awk` scanned the whole of `integration.rs` and
kept the *last* match, so tests added in later phases flipped the apparent ordering. Property
verified true by direct read; replaced by the body-scoped `clm_5YTX49CR`. That is **two** replays I
have authored that failed for reasons unrelated to the property claimed. (3) The seam-pin commit
edited claim-scoped source **before declaring anything**; both remedies the hook offers were wrong
(retroactive declare makes the log lie about ordering; `lint.ignore` on `cmds.rs` silences coverage
on core source), so at the founder's direction it was reverted and redone declare-first — the
reapplied file hashes identically (`d00d848c…`), only the record's ordering changed. (4) `df442f2..HEAD`
still flags **`cli/src/purgecmd.rs`**, touched in P1 by a one-token `pub(crate)` visibility widening
that no claim's scope covered. Left standing and recorded rather than papered over — reverting a
6-commit phase to re-declare a visibility change would be disproportionate, and the two offered
remedies remain the wrong ones. **Claim ledger: 36 claims — 27 confirmed, 6 evidenced, 2 refuted, 1
manual awaiting founder attestation.**
**Recorded, not fixed.** **Pid reuse defeats the epoch gate:** if a later daemon lands on the same
pid as the epoch that last rebuilt (macOS wraparound — the same recycling class `doctor` handled via
flock), the stale count renders as current and the new epoch's first rebuild *accumulates* onto it,
since the writer reset keys on the same comparison. Cosmetic over-count in one line, self-correcting
at the next restart under a different pid; predates this commit, shared by reader and writer. Durable
fix is an epoch nonce (a start-timestamp stamped at `acquire_lock`) instead of pid identity.
**Eviction still is not purge:** a hash on an *unmodeled field of a line that parses* is protected by
`purge --orphans`' raw scan and evicted by `enforce_budget` — demonstrated live by the skeptic. No
producer emits that today, but PROTOCOL's additive rule plus the queued 1.0-freeze fields (§4
`emitter_turn`, §5 `imported`) are exactly how one appears silently; now documented in
`extra_protected_refs`, and owed a debt line on the freeze checklist. Also: `status --json` runs no
eviction and reports no store-size/over-budget state, so the "json sees less than text" class this
round existed to close is only half closed; `last_ignore_rebuild_ms` remains lifetime-scoped while
the count is now epoch-scoped.
**Item 3, the Linux CI leg, is NOT done — founder decision, not an oversight.** `ci.yml` fires only
on `push: branches: [main]` and `pull_request`, with no `workflow_dispatch`, so the branch push runs
nothing; a PR would carry three stacked rounds at once. Offered draft-PR-to-main / PR-onto-parent /
skip; **skip was chosen**. Every timing margin across the last three rounds remains macOS/FSEvents
evidence only, and the older Linux leg for the `unreadable`/`io_failed` producers stays owed.

**Ignore-set rebuild gate — GATE PASS (2026-07-25, branch `fix/ignore-rebuild-gate`,
`bdb911c..fe57cda`, not pushed):** All 3 phases delivered against
`docs/superpowers/plans/2026-07-25-agentrec-ignore-rebuild-gate.md`. **386 → 395 tests, 0 failed,
1 ignored**, clippy `-D warnings` + fmt clean on debug **and** release, release `strings` carries no
`AGENTREC_TEST` seam. Sonnet implementers, an Opus reviewer per phase in an isolated worktree, and a
binding **fable skeptic** done-gate — plus an orchestrator re-run of the full suite at every phase,
independent of every implementer. Each count agreed three ways.
**The defect, closed:** `e453e86` fixed the *trigger* (`gitignore_dirty` set at event ingest by
filename, verdict-independent) and left the *consumption* inside `if settled || capped`, whose
predicates are armed **only** by `Class::Watch` events. Editing `.gitignore` to re-include a path and
then touching only that path therefore produced **no rebuild, ever** — the path classified against the
stale set, armed no timer, and the flush block never ran. Under-record direction, so nothing was
corrupted; the recorder simply, silently, did not record a file the user had explicitly re-enabled.
Second shipping of this class (`049a4aa` introduced the trigger defect). **(P1) `0a7b279`** moves the
rebuild to the top of `run`'s loop behind `maybe_rebuild(dirty, root)` — top-of-tick, not inside
ingest, so a hot `.gitignore` costs at most one full-repo `IgnoreSet::build` per `POLL` rather than
one per event. **(P2) `b68bc42`+`8cce05a`** makes reloads observable (`ignore_rebuilds` /
`last_ignore_rebuild_ms` in `state.json`, one stderr line per rebuild, a `status` line shown only when
the counter is non-zero, and `status --json`, which did not exist before) — the class shipped twice
partly because nothing anywhere reported whether the filter config was ever reloaded. **(P3)
`ec2874b`** adds the mid-run coverage that never existed: deletion re-widens, creation is honored
without restart, `SingleDaemonGuard` hygiene, and a matcher-count precondition.
**The skeptic verified the product, not the suite:** live binary, real daemon, real repo, all four
mid-run directions (widen / narrow / delete / create) checked against `.agentrec/log.jsonl` directly,
`ignore_rebuilds: 4` with matching stderr lines and 0700/0600 perms. It also **empirically confirmed
the documented residual window** — an event arriving in the same drain batch as the ignore-file edit
is still classified against the pre-edit set, and is honored on the path's *next* mutation (≤1 `POLL`
tick), which is the honest claim the plan makes rather than "instant".
**Three things this round got wrong and corrected rather than buried.** (a) The plan's test-count
ladder was wrong **twice** (387→388, 391→393→395), both times derivable from the plan's own contents
before any implementer touched it — Phase 3's original target of 393 was already met by Phase 2, so
it could have passed having added neither required test. (b) A claim came back **REFUTED**
(`clm_1H62NAJN`): its replay was authored as `cargo test A B`, and cargo accepts one TESTNAME, so it
errored instead of running and never proved its property — its own evidence event recorded `exit: 1`
and the orchestrator reported the ledger clean without checking exit codes. Left REFUTED in the log
deliberately, replaced by `clm_4H93KXD1`; every other evidence exit was then audited (all 0).
(c) **`nested_gitignore_precedence` was NEVER vacuous** — see the correction below, now measured in
four cells and fixed in the test comment, the plan and this file.
**AC4 could not have proven itself as written:** the plan's literal "5 rapid `.gitignore` rewrites →
counter in `1..=5`" fails to discriminate its own neuter, because 5 writes coalesce to ~3 FSEvents
events, so a per-event counter also lands ≤5. Reproduced directly by the reviewer. Delivered form is
40 writes bounded `1..=10` (correct code measures 3–4; the neuter measures 15–18). The declared claim
still carried the false text, so it was amended to replay what exists and **superseded** by
`clm_6SAFG0JJ`, not deleted. **Claim ledger: 17 claims — 10 confirmed, 6 evidenced, 1 deliberately
refuted**, declare-first per criterion at the parent commit each time (last round skipped this).
**Coverage gaps the gate names, none blocking:** (1) **narrowing while events are pending** — paths
already admitted to `pending` under the older, wider rules are still staged at the next flush even if
a mid-debounce edit now ignores them; the over-record mirror of the documented residual window,
untested and undescribed in the module comment. (2) **Everything here is macOS/FSEvents** — the
400 ms margins and the `1..=10` bound are reasoned safe (failures fall RED, and inotify widens the
margins) but unobserved; Linux CI leg owed, same posture as the `unreadable`/`io_failed` producers.
(3) counter behavior across a daemon restart never observed. (4) no test pins the sustained walk rate
on a continuously-rewritten `.gitignore`.
**Recorded, not fixed:** `status --ack-degraded --json` prints plain text under a `--json` flag;
`status --json` omits the DEGRADED fields entirely (README now says so), so a monitoring script sees
less than the text surface; `state::read_state` chains two `.ok()`s into `unwrap_or_default()`, so
**one** unparseable field silently resets `pid`, `signal_offset`, `snapshot_failures` and `io_failed`
with no diagnostic — and a reset `signal_offset` replays the entire signal inbox (pre-existing; this
round did not worsen it, and its own AC3 test is built to discriminate that fallback). Open question 1
was **resolved by defaulting** to option (c): the reload line shows whenever the counter is non-zero,
and the counter is lifetime-cumulative and not cleared by `--ack-degraded`, so a long-lived repo
eventually renders `ignore: reloaded 4821 time(s)` in the daily-driver surface. Option (a),
epoch-scoping, remains the cheap reversal — founder call. **Favorable fact for the churn-honesty
plan:** `status --json` performs **zero writes** (returns before `enforce_budget`; mtime and inode
unchanged across a run), so that plan's Phase 5 criterion starts satisfied on this path.

**Planning round — three plans committed, zero code changed (2026-07-25, branch
`fix/blame-attribution-and-noise-folding`, `8da3e16..6244afb`, not pushed):** Documented here
**before** implementation because these plans are the next things to be attacked, and an
un-registered plan file is not a durable artifact. **Nothing below was implemented at the time of
writing** — plan (2), the rebuild gate, has since shipped in full; see the GATE PASS entry above.
Test suite untouched at the prior round's **386 passed / 0 failed / 1 ignored** (a figure carried from that
round's receipt and marked UNVERIFIED in every plan header — re-run before trusting it).

**(1) Churn-honesty round — `docs/superpowers/plans/2026-07-25-agentrec-churn-honesty-round.md`
(`8da3e16`), 9 phases.** The 5-item handoff scope (fold `diff`'s per-path renderer · record-time
`ignore_globs` · `doctor` advisories + a ref→blob presence check · `enforce_budget` protect-set ·
re-measure), refined by a 3-lens adversarial fable redteam (honesty/attribution ·
daemon-crash-concurrency · vacuity/measurement/scope) whose lenses **disagreed on two daemon facts**;
reading the code settled both and one lens was wrong — subagent reports are leads, not evidence.
Three findings reshaped the scope, each verified firsthand:
**(a) `enforce_budget` deletes live-cited blobs today — a real bug in shipped code, not a plan
item.** Its keep-set is built from *parsed* `TurnRecord`s and it hard-deletes (`store.remove` →
`fs::remove_file`), while `open.json` refs, `memory.jsonl` pins and torn-line refs go unprotected —
exactly the classes `purgecmd::referenced_hashes`' raw non-parsing `sha256:` byte-scan exists to
protect. Its sole caller is `cmds::status_report`, so **running `agentrec status` on an over-budget
store can destroy an in-flight turn's snapshot**, with no daemon-liveness refusal anywhere on that
path. Fix keeps hard-delete (archive-on-evict frees zero disk — archives sit under `.agentrec/` —
while *reporting* bytes freed, and eviction fires from a read verb, so it would grow an unbounded
archive as a side effect of reading status) and adds a raw-scan protect-set harvested as late as
possible; the residual live-daemon window is narrowed-not-closed, same posture as `purge --orphans`'
undo race. **(b)** the `e453e86` rebuild-gate defect, pulled out into its own plan (below).
**(c) record-time exclusion is a non-event**, so `blame` fills the vacuum: `blame_line` falls through
to `"before recording began"` and a touched-then-excluded path prints `"· human-edited since"` —
an agent edit attributed to the human, byte-for-byte the confident-false-answer class `17657d9` just
fixed. Hence item 2 splits into two phases with the honesty surface marked **non-optional** — the
feature "works" without it, which is exactly why it would get deferred.
**Ordering deliberately reversed from the handoff's list:** measurement runs **first**, because
`status` mutates the store, so a measurement taken after any lever measures a store that lever
already changed; and the owed live-daemon E2E is promoted ahead of feature work because it builds
the harness two later phases need. **Two zero-yield results are stated up front so no receipt can
claim otherwise:** folding `diff` is render-time only (`log.jsonl` is append-only; zero bytes, zero
lines), and record-time globs measure **~0** churn reduction on this corpus (0 churn entries since
the D29 fix merged; `.claims/`, the named next instance, has 3 tracked files so the rule keeps it) —
their justification is the PROTOCOL.md:25 promise that no code has ever read, nothing else.
Every acceptance criterion names the **neuter** that must turn a *named* test RED; criteria
unprovable by fixture are marked for a live-daemon leg or a VERIFY-LEDGER row, never a PASS.
**4 open questions**, of which Q1 (config globs vs git-tracked precedence) gates Phase 5 and changes
its **file count**, not just its fixture — answer before chunking.

**(2) Ignore-set rebuild gate — `docs/superpowers/plans/2026-07-25-agentrec-ignore-rebuild-gate.md`
(`fba86e9`, corrected `6244afb`), 3 phases.** Pulled out of the above because it is a defect in
**already-merged** code. `e453e86` fixed the *trigger* (`apply_watch_result` sets `gitignore_dirty`
at ingest by filename, verdict-independent — correct, and unit-tested). It never fixed the
*consumption*: the rebuild lives inside `if settled || capped` in `daemon::run`, and both predicates
derive from `last_event`/`first_event`, armed **only** for `Class::Watch`. So editing `.gitignore` to
add `!keep.log` and then writing only `keep.log` leaves the rebuild permanently unrun — the file is
never recorded until some unrelated watched path changes. Direction is under-record, so nothing is
corrupted, but the recorder silently ignores a file the user explicitly re-enabled. **Second time
this class has shipped** (`049a4aa` introduced the trigger defect, `e453e86` fixed the trigger and
left the gate). No existing test can catch it: the unit test drives `drain_watch_events` directly and
asserts the flag is *set*, never that anything consumes it; the real-daemon test proves *ignoring*,
never *re-widening*; nothing in the suite exercises a mid-run ignore-rule change at all.
**`6244afb` corrected the plan's own errors before any code was written:** Phase 1's AC1 and AC2 were
jointly unsatisfiable (a positive control written *after* the ignore-rule edit is itself
`Class::Watch` activity — it arms the timer, `settled` fires, the rebuild runs under current code, so
the RED-first test would have gone green pre-fix and its neuter would have proven nothing); one AC
named a counter that lands a phase later plus a fallback assert inside a loop body the plan itself
calls unit-unreachable; and one Phase 2 AC asserted stderr-line counts inside a 250 ms window, which
is FSEvents-coalescing flake bait this repo has already been burned by.
**Correction to a claim this file has been repeating, now measured — `nested_gitignore_precedence`
was NEVER vacuous.** This entry previously said the "passes vacuously in a non-git tempdir" note was
true pre-D29 and stale after. Both readings are wrong. Phase 3's reviewer measured `matchers.len()`
across all four cells — {pre-D29 `build` body, post-D29} × {`git init`, none} — and got **2 in every
cell**, with every precedence assertion passing in every cell. The claim is self-refuting on its own
terms: it depends on the walk still *yielding* both `.gitignore` files without git, which is exactly
the condition under which matchers are collected and the assertions are real. What `require_git`
genuinely governs is whether ignore rules prune the **traversal**, which is why `git init` is
load-bearing in **live-daemon** fixtures (a matcher-less walk there really does prove nothing) and
merely hygiene in a unit test that builds its `IgnoreSet` directly. Corrected in three places —
here, the plan, and the test's own comment — because a false claim in a source comment is the
doc-drift class that already produced a GATE FAIL in this repo. Also **verified rather than assumed**: `IgnoreSet::is_ignored` maps
`ignore::Match::Whitelist` to `false`, so the fixture's `!keep.log` genuinely re-widens.

**(3) Phase 2.0 substrate — `docs/superpowers/plans/2026-07-25-agentrec-phase-2-0.md`** (written the
prior round, left untracked, committed in `8da3e16` so a fresh worktree can see it), 5 phases:
`import claude` gate → import persist → golden harness → `RepositoryView` → `--json` contracts.
Unchanged content; its 3 open questions are still open, including whether the 12 unchecked tasks in
the memory-v1 plan are stale bookkeeping or real work.

**Cross-plan seam, recorded so it cannot be re-fixed in parallel:** churn-honesty Phase 5 now points
at plan (2) for the loop-placement fix and adds only the `config.toml` trigger on top of it. And
folding must stay in the CLI adapter — if it migrates into Phase 2.0's `RepositoryView::diff`, then
`diff --json` and the MCP read surface inherit it silently.

**Blame-honesty + skipped_reason + noise folding — GATE PASS (2026-07-25,
branch `fix/blame-attribution-and-noise-folding`, `17657d9..c4ada8f`, not pushed):** Three changes
landed off a 4-lens adversarial redteam of store churn. Binding skeptical-reviewer done-gate in an
isolated worktree: round 1 **GATE FAIL** (one blocking finding, below), round 2 **GATE PASS** at
`7be9e4d` — all 6 findings CLOSED, each refuted by neutering the fix and observing a *named* test go
RED, sources restored byte-identically (`readcmds.rs` `53a7683e…`, `daemon.rs` `2387b22b…`,
`cmds.rs` `438a3dba…`). **349 → 386 tests, 0 failed, 1 ignored**, clippy `-D warnings` + fmt clean,
verified by the orchestrator independently of every implementer. **(A) `17657d9` blame
over-attribution — a real, live correctness bug in the product's core claim:** `load_text` swallowed
both `StoreError::Missing` and `Corrupt` into `""`, and `added_or_changed_lines("", after)` returns
*every* line of `after`, so a turn whose `before` blob did not resolve claimed authorship of **any
line queried**; the mirror direction produced a confident false `"before recording began"`. Fires
today — TTL purge, budget eviction and `purge --snapshots-before` all remove blobs by design. Fixed
by `load_text → Option<String>` plus a poisoning rule mirroring `has_gap_after`: report
`responsible` only when no *unresolvable* candidate is newer. The `before == None` create case is
preserved (empty is legitimate there) and pinned. **(B) `8216747` `skipped_reason`** — additive
`FileEntry` field, open enum (`over_cap`/`io_failed`/`unreadable`, `policy` reserved with **no
producer**), landed deliberately **before the Phase 2.0 protocol-1.0 freeze**. Closes a real lie:
`undo` said "over size cap" for all three causes. Rides along a behavior change — over-cap/io-failed
now record `after: Some(hash)`, so `modified_since` stops reporting large files modified forever.
PROTOCOL §5 + IMPLEMENTATION (D45, SR1–SR7) same commit; **conformance-fixture debt recorded against
N1, not fabricated** (the corpus still does not exist). **(C) `911d274`+`b5a3652` noise folding** —
config-declared `noise_globs`, display-only, `--all-files` (deliberately NOT overloading `--all`,
which is the turn-grade axis). Also fixed a real panic it surfaced: an absolute `FileEntry.path`
tripped `Gitignore`'s `assert!(!path.has_root())` — same log-as-trust-boundary class as the prior
P0. **Round-1 GATE FAIL was earned and is instructive:** the SR6 regression guard was **vacuous** —
it seeded `op: "modify"`, which `build_plan` refuses via an *independent* before-hash branch, so the
`skipped` gate could be deleted entirely and the test stayed green. The reachable case is
`op: "create"` (exactly what `Recorder::resolve` emits for a new over-cap file, and which change (B)
newly made reachable): the skeptic proved live that with the gate removed, `undo` **deletes the
file**. Closed in `7be9e4d` by fixing the fixture — re-refuted at round 2 by DELETING the 886-byte
`skipped` block outright (not short-circuiting it), which reds two tests; the `op: "create"` fixture
genuinely isolates the gate because `build_plan`'s before-blob branch only runs for
`modify`/`delete`. **Watch-item on implementer reports:** the fix round's own sha256 restoration
proof cited `512fdc61…`, which is `c46519c`'s hash, not `readcmds.rs`'s at `7be9e4d` — the code was
fine, the *report* was stale, and that figure was propagated into this Status entry before the gate
caught it. Verify hashes against the commit under review, not the one before it. `7be9e4d` also
closed: the unclassified 4th `skipped`
producer (symlink `read_link` arm), a missing cross-seam test for the (B)×(A) ghost-hash chain (all
five read verbs degrade honestly, none lies — now pinned), the untested `prior snapshot unavailable`
string, and D-PD6-class vocabulary drift (`build_plan` vs `print_entry` spelling the same fact two
ways; `diff` was also *less* specific than `undo` about corrupt-vs-missing). **Known and NOT to be
written up as a win: noise folding does not reduce the churn blast it was justified by.**
`fmt::turn_list_line` renders only a file *count* and `turn_detail_header` renders no file list, so
a 9602-entry churn turn goes from `9602 files` to `0 files` **plus a fold line** — output is one
line *longer*, and `show --all-files` is vacuous. The only surface that prints noise paths
one-per-line is `diff`'s `print_entry`, scoped out as an attribution surface. Extending folding to
`diff` under the same display-only contract is the actual fix — **founder decision, deliberately not
taken.** **Nothing here is verified against a live daemon:** every BL/SR/NF test seeds `log.jsonl`
directly or calls `Recorder::stage` in-process; the over-cap→ghost-hash chain is inferred from code
plus seeded fixtures, never observed end-to-end. This repo has history of exactly that gap mattering
(two green gitignore tests coexisted with a 764 MiB leak) — owe an E2E leg before merge: `record` in
a tempdir, write an 11 MiB file, wait past debounce + the 10 s quiet window, shrink it below cap,
wait again, then assert the two real `log.jsonl` entries carry `skipped_reason:"over_cap"` /
`before:<ghost>` before running `blame`/`diff`/`undo`. The `unreadable` and `io_failed` producer
fixtures are `#[cfg(unix)]` and were exercised on **macOS/APFS only** — Linux CI leg owed.
**Gate-accepted cosmetic residuals, recorded not fixed:** `undo`'s refusal now renders a double
em-dash (`REFUSE  big.bin — content not snapshotted — over size cap`) — consistent vocabulary,
awkward line; and `integration.rs:2327`'s `!stdout.contains("REVERT  big.bin")` clause is dead (the
renderer emits lowercase `revert`), pre-existing and carried forward, harmless because the
`starts_with("REFUSE")` and `.exists()` asserts carry that test. **claimd:
declare-first was skipped this round** (4 coverage findings: IMPLEMENTATION.md, daemon.rs,
readcmds.rs, integration.rs). **Retroactive declaration deliberately refused** — the skill forbids
it and a declare-record postdating the code would make the log lie about ordering, the one property
claimd provides. The ignore list lives in `.claims/config.json` under `lint.ignore` and is seeded
with `CLAUDE.md`, `VERIFY-LEDGER.md`, `docs/**` *(an earlier draft of this entry claimed a
`.claims/lint.ignore` file was missing and the seeding note was stale — both false; there is no such
file because the mechanism is the config key)*. **`IMPLEMENTATION.md` added to that list by the
agent, not the founder — review it.** The Stop hook's coverage rule fired on it three times across
this round and names exactly two remedies; retroactive declaration is the one the skill forbids, so
the ignore entry was taken as the sanctioned alternative. It is one line in
`.claims/config.json`, trivially reversible, and consistent with its two narrative siblings already
being ignored. **The underlying rule is still undecided:** `PROTOCOL.md` is NOT ignored, so the same
finding will fire on the next normative-doc edit; the real question is whether register/narrative
docs are categorically out of claim scope (they have no honest replay command) or whether spec docs
should carry grep-style claims precisely because doc drift has already caused a GATE FAIL in this
repo. The two D29 claims went STALE (daemon.rs touched) and were
re-verified **confirmed** at HEAD (`c522de1c…`, `f5b73066…`), so this round did not break the
gitignore fix. Design record for the deferred storage work: `docs/superpowers/specs/2026-07-25-store-churn-designs.md`
(`c46519c`). A detached review worktree was left at `…/scratchpad/gate` (`c46519c`) — remove when
convenient.

**Store bloat has TWO classes — correction (2026-07-25, `main`, docs-only):** the 2026-07-17
round below is right that *its* 2.55 GiB was orphaned superseded snapshots, but it reads as if
that is the only bloat class. It is not, and reaching for `purge --orphans` on the wrong class
reclaims ~nothing. **Class 1 — orphaned superseded snapshots:** blobs no turn references
(daemon `put`s every debounced batch for kill-9 recovery; the coarse `TurnRecord` cites only
first `before`/last `after`). `--orphans` archive-renames them; over-budget-with-0-freed is the
tell. **Class 2 — referenced churn from unfiltered paths:** blobs turns legitimately cite, from
directories that should never have been recorded (D29 gitignore self-match — a dir whose own
`.gitignore` is `*`). Measured live here 2026-07-25: of a **784.8 MiB** live store, **767.7 MiB
(97.8%) was churn-only** (`.remember` 567.1 + `.code-review-graph` 200.6, both self-matching
`.gitignore`), against **2.1 MiB actually unreferenced** — i.e. `--orphans` is structurally
blind to it, by design (it finds garbage *by absence* from the ref-set, which is exactly what
makes it torn-line safe). Prevention for class 2 is the merged D29 fix
([#6](https://github.com/ravi1395/agentrec/pull/6), `4201538`) — leak confirmed halted (recent
turns show zero churn paths). Reclaim has **no precise tool**: only `purge --snapshots-before
<DATE>`, which is date- not path-scoped and — unlike `--orphans` — **hard-deletes**
(`store.remove` → `fs::remove_file`, no archive). Gates checked before recommending it: blobs
content-shared between churn and source paths = **1, 0.0 MiB** (no cross-kill), and missing-blob
degradation is honest — `undo` refuses per-file in `build_plan` *before any mutation* (no partial
revert), `diff` prints `(snapshot unavailable — purged or missing)`. Collateral is ~14 MiB of
legit source snapshots that git already holds. **Known residual, not byte-reclaimable:**
archiving/deleting blobs does not clean `log.jsonl` — the 9602 `.remember` file entries survive
and those turns keep rendering as churn blasts in `log`/`show`; removing them would be a third
sanctioned append-only rewrite class. **`purge --paths` deliberately NOT built:** path
attribution requires *parsing* log lines, which is precisely what `--orphans` refuses (a torn
line's refs must still count as keep, and a torn line cannot be path-attributed at all) — its
safety argument is strictly harder than `--orphans`'. Also un-counted by every prior note: the
retained archives `.agentrec/objects.archived.1784328469` (**2.6 GiB**) + `.1784934498` (15 MiB)
are the largest items on disk and need only an `rm` — no code, founder call pending.

**P2.0 entry-gate measured + self-matching-gitignore fix (2026-07-24, branch
`fix/gitignore-self-match`, commit `049a4aa`):** Ran the Phase 2.0 hard gate against the real
corpus (`~/.claude/projects`, 1860 files / 515 MB) before planning any P2 work, and the store
audit that rode along found a live P1. **GATE PASSES: 99.1%** (1266/1277 top-level sessions;
floor is 90%). The naive denominator reads 68% — 583 of the 1860 files are
`subagents/agent-*.jsonl`, sidechains the spec already excludes; they carry no `cwd` and are
correctly unimportable. **Spec edit owed:** sidechains are now *separate files*, not only inline
`isSidechain` lines, so the importer must exclude by path too. 0 unparseable lines / 106,311;
37 MB peak RSS over 515 MB (500/500 target met with 13x margin). **Before-bytes ladder gains an
undocumented 4th tier:** `~/.claude/file-history/<session>/<hash>@<vN>` holds verbatim pre-edit
bytes referenced by `file-history-snapshot` records — 508/508 referenced backups resolved on
disk, 25.3% of file entries. Retention-limited (~30 days, 102/1277 sessions) so import fidelity
**degrades with age**; opportunistic like T1, never a guarantee — spec amendment, rides the
importer commit. Measured ladder: T1 42.5% / **T1.5 25.3%** / T2 24.5% / T3 7.7% →
**67.9% reconstructible without git**. Do NOT quote the 92.3% figure: T2 is an upper bound
(git holds committed states only, so mid-session intermediate edits were never in git).
Opaque:naming **2.49:1** — 71% of tool calls can mutate files while naming none, confirming
`files_complete:false` as mandatory. **The P1: `IgnoreSet::build` did not honor a `.gitignore`
whose own rules match itself.** It collected ignore files from the results of a gitignore-aware
walk, so a `.gitignore` containing `*` (what tool-generated cache dirs ship) filtered itself out
of the walk → no matcher for that directory → nothing beneath it ever filtered, contrary to
SPEC.md:51 and D29. Live cost in this repo's own store: **7419 file entries / 764.6 MiB across
`.remember/` + `.code-review-graph/` = 98.2% of referenced store bytes** (real repo content:
14.2 MiB), incl. 64 snapshots of a 9 MiB SQLite and 1142 of a PID file; most recent leaked
snapshot `2026-07-24T21:41Z` — live, not historical. Fix probes each directory the walk reaches
for its own `.gitignore` instead of waiting for the walk to yield it; traversal pruning retained
(still no node_modules descent), and directories the walk prunes are already excluded by an
ancestor rule or the denylist so coverage is unchanged. **Verified against the real repo:
matchers 1→4, all 5 leaked paths now filtered, 0/68 git-tracked files change verdict.**
**347 passed, 0 failed, 1 ignored observed this round** (prior note said 343; the +1 arithmetic
does not reconcile, so treat 347 as this round's observation, not a corrected baseline), clippy
`-D warnings` + fmt clean. **DEPLOYED AND PROVEN IN PRODUCTION (2026-07-25).** The leak was
live throughout the fix round — the service (`com.agentrec.bfa6bde6eaa4`, pid 1484, started
07-22) ran the `~/.local/bin/agentrec` binary dated 07-12, and the store grew 789.3 → 798.6 MiB
during the session itself. Deployed: release build → old binary backed up to
`~/.local/bin/agentrec.bak-2026-07-12` → installed by atomic rename (a running executable cannot
be overwritten in place) → `launchctl kickstart -k` → new pid, `doctor` all-pass exit 0.
**Real-behavior proof, with a positive control so "absent" could not mean "daemon dead":** wrote
`.remember/leak-check.tmp` + `.remember/tmp/leak-check.pid` + `verify-scratch.txt`, waited past
the debounce and the 10s quiet window → exactly one new turn, `files=['verify-scratch.txt']`,
**zero `.remember/` paths**. Leak stopped. Clean stop→purge→restart cycle recorded epochs
correctly (`gaps: 0`). **Correction to this entry's earlier claim — `purge --orphans` does NOT
reclaim the churn.** It freed only 285 blobs / 13.9 MiB. The 768.4 MiB of churn is **referenced
by real historical turns** (the daemon recorded them as genuine file entries), so those blobs are
not orphans and never become orphans; the earlier "they orphan once recording stops touching
those paths" was wrong. Store is now 784.7 MiB = 768.4 MiB churn history + **14.3 MiB of actual
repo content**. The only reclaim path is `purge --snapshots-before <DATE>`, which **deletes**
snapshot blobs for every turn before that date — legitimate source history included, not just
churn — so it is a deliberate founder decision, deliberately NOT taken here. Also still on disk:
`objects.archived.1784328469` (2.6 GiB, last round) and `objects.archived.1784934498` (15 MiB,
this round); both are archives, safe to `rm` at the founder's discretion. **Debugging gotcha,
now recorded in the test:** the first repro REFUTED the hypothesis because the fixture tempdir
was not a git repo — `ignore::WalkBuilder::require_git` defaults true, so no ignore rules applied
and the fixture tested nothing; `git init` flipped it to a clean RED. The pre-existing
`nested_gitignore_precedence` test has exactly this gap and passes vacuously. **Corrected by the
done-gate — an earlier draft of this entry claimed `IgnoreSet::build` "runs once at startup and
never refreshes". That was FALSE:** `daemon.rs:142` computes `gitignore_touched` from `pending`
and `daemon.rs:162` rebuilds the set, so a newly created `.gitignore` IS honored without a
restart. The real, narrower defect is the opposite and **this round introduced it**: after the
fix a self-matching `.gitignore` classifies `Ignore`, so it never enters `pending`, so the
rebuild never fires *for that file* — editing `.remember/.gitignore` (e.g. adding `!keep.log`)
was unhonored until daemon restart, where pre-fix it was picked up. Direction was
fail-toward-ignore (never over-records), hence LOW. **Both gate findings are now FIXED
(`e453e86`), not merely recorded.** (a) The rebuild trigger is set at event-ingest time in
`apply_watch_result`, independent of the classification result, and cleared after the rebuild —
a `.gitignore` is filter *configuration*, not watched content, so the trigger must not depend on
its own ignore verdict. The `.gitignore` still never enters `pending`, so no churn returns.
(b) The disclosed unit-only coverage gap is closed by a real-daemon integration test. Both tests
are refutation-proven: neutering the ingest-time flag reds the unit test; reverting
`IgnoreSet::build` to its pre-`049a4aa` form reds the integration test; source restored
byte-identically after each (sha256 `a2b11d46…`). The unit test asserts its own precondition
(that the file classifies `Ignore`) so it cannot pass for the wrong reason. **349 passed,
0 failed, 1 ignored** (+2 from 347), clippy `-D warnings` + fmt clean. Note the existing
`records_rich_turn_..._filters_ignored` integration test only ever exercised a **root-level,
non-self-matching** `.gitignore` — that, plus `nested_gitignore_precedence` passing vacuously in
a non-git tempdir, is why two green gitignore tests coexisted with a 764 MiB leak. Both claims
declared **before** implementing under the newly-adopted claimd protocol
(`clm_0PW9CEDK…`, `clm_5SQ4C1F5…`), now EVIDENCED.
**Binding skeptical-reviewer done-gate ran in an isolated worktree at `c8ac73e`: GATE FAIL on
documentation accuracy only, code PASS.** AC1–AC5 + AC7 all PASS, refutation-proven in three
independent channels: unit RED/GREEN (neutered `build` → named test FAILED → restored
byte-identically, sha256 verified); a dual-implementation harness over the real repo
(`old_matchers=1` → `new_matchers=4`, `tracked=71 tracked_flips=0`, full-tree
`walked=525 newly_ignored=118 newly_watched=0`); and a **live-daemon E2E** the skeptic built
(old build records `.remember/session.log` + `.remember/session.pid`, new build records only
`src/main.rs`) — proving the fix at the recording path, not just at `is_ignored`. Also
independently corroborated the store audit from the raw log: `98.2%` leak share exactly as
claimed, 767.2 MiB (grown from 764.6 — consistent with a still-live leak), 0 unparseable lines.
Vacuity check confirmed load-bearing: old build + `git init` deleted → test passes. The sole
FAIL was the false "never refreshes" claim above, now corrected. Skeptic's other findings, all
accepted as recorded-not-fixed: root-level self-matching `*` would hide force-added tracked
source (INFO, 0/71 here); `dir.join(".gitignore")` resolves `.GITIGNORE` on case-insensitive
APFS, a new macOS/Linux divergence that matches git-on-that-filesystem (INFO); `.claims/` is
itself now recorded content — same class as the bug just fixed, negligible magnitude (INFO).
True prior-baseline test count is **346** at `5003ae1` (347 − the 1 added test; the "343" note
came from the unmerged `fix/purge-orphans-gc` branch, not `main`). **Reframes last round:** the 2.55 GiB
of orphans reclaimed 2026-07-17 were almost certainly this same churn, so `purge --orphans` was
a workaround that masked this bug for a week rather than "the durable fix" it was recorded as
(the feature is still correct — intermediate snapshots genuinely orphan). Store not yet
re-measured post-fix; the 19.5x blob-compression figure from this round was measured **on the
churn** and will not hold against a 14 MiB source-only store — re-measure before acting on it.
Not pushed/PR'd (no ask). Full findings + scripts in the session scratchpad `GATE-FINDINGS.md`.

**Phase 2 spec finalized (2026-07-18, `main`, docs-only):** Ironed out the P2 spec —
new `docs/superpowers/specs/2026-07-18-agentrec-phase-2-design.md` supersedes the P2 half of
the 2026-07-12 draft (P3 half stays draft). **Four founder decisions locked:** (1) VS Code
DEFERRED out of P2 (demand-driven return; "v2 done" = O+P+Q); (2) PR↔turn association
exact-only (trailers/export, no heuristic mode ever); (3) Sutra = external agentrec recorder +
`.sutra` sidecar (no embedded second recorder); (4) ROADMAP Phase-1 truth substrate folded in
as **Phase 2.0 at full scope** — `import claude` (K) + `import aider` (L) + git trailers +
`git-agentrec` shim (M) + Protocol 1.0 freeze + conformance fixtures (N) + npm/mise wrappers
(Z2) + RepositoryView/UndoCoordinator seam extraction + `diff`/`blame`/`status --json`.
Entry-gate reality check that forced 2.0: 0/4 built today (no import, no trailer code, no
fixtures, `--json` only on log/doctor/memory verbs). Phases 2.0→2.4 risk-ordered (substrate →
Codex spike-first → MCP read → MCP destructive → setup/packaging), self-healing E2E = 2.3 exit,
≥90%-real-transcript import gate = hard stop. Additive protocol changes queued for the freeze:
§4 `emitter_turn`, §5 `imported`, §8 `agentrec_status`. Companion edits (PROTOCOL/IMPLEMENTATION
/ROADMAP) deferred to the commits that ship the code. Next: `/phases` plan off the new spec.

**Memory-dogfood prep + orphan-GC round (2026-07-17, branch `fix/purge-orphans-gc`, commit `c881cfe`):**
Prepped this repo's live store for the 1-week memory dogfood and closed the store-bloat mystery
from the D36 round's dogfood observations. **The "eviction bug (3.1 GiB over budget, 0 B freed)"
was NOT a bug** — root-caused: of the 3.29 GiB store, only **0.74 GiB was turn-referenced
snapshots** (< 2 GiB budget → `retention::enforce_budget`'s "0 freed" was *correct*); the real
bloat was **2.55 GiB / 5644 ORPHANED blobs** — superseded intermediate snapshots the daemon
`put`s into the CAS on **every** debounced batch (`Recorder::stage`, load-bearing for kill-9
crash recovery) that the coarse first-`before`/last-`after` `TurnRecord` never references. Nothing
reclaimed them (budget eviction walks only turn-referenced snapshots; TTL purge only prompts).
Completeness verified before any delete (greedy raw-hex grep matched structured extraction; 0/10
orphans in raw log). **Shipped `purge --orphans`** (founder chose the durable fix over one-time
reclaim): archive-*renames* (never deletes) every CAS blob no `log.jsonl` turn/prompt, no
in-flight `open.json`, and no `memory.jsonl` pin references, into `.agentrec/objects.archived.<ts>/`;
daemon-liveness refusal + `pass_start` mtime guard (undo race). **The load-bearing safety choice:
the ref-set is a raw `sha256:` byte-scan, NEVER `load_log`** — a blob cited only by a torn/unknown
log line is never mistaken for an orphan (RED-proven: the torn-line test fails under a parse-drop
ref-set). Also fixed the **dishonest status message** (was "oldest snapshots evicted" on a
total-size trigger even when ~0 freed → now attributes bloat to unreferenced blobs + names
`purge --orphans`). New: `BlobStore::list_hashes`/`archive`. **343 tests, 0 failed** (+10),
clippy `-D warnings` + fmt clean. **Binding skeptical-reviewer done-gate in an isolated worktree:
GATE PASS** (all 5 ACs MET; refuted AC1 twice by neutering — drop-torn-lines and drop-open.json —
each RED then restored byte-identical). **Post-gate fixes** (skeptic-surfaced): (a) real defect —
`memory.jsonl` pins also cite CAS blobs (`verify`'s pin-diff `store.get`s them), so added
memory.jsonl to `referenced_hashes` + corrected the "complete citer set" comment (RED-proven the
pin blob is now kept — and confirmed in production: the reclaim below preserved the drifted
torture pin's old blob so `verify`'s diff still rendered); (b) permanent test for the status
orphan-attribution clause; (c) undo/purge microsecond TOCTOU documented (archive-only, "narrowed
not closed" like `purge_log_duplicates`). **Live dogfood-baseline prep executed** (release binary,
launchd service `com.agentrec.bfa6bde6eaa4` cleanly stopped→reclaim→restarted, new pid healthy):
killed 4 leaked scratchpad/worktree daemons; **`purge --orphans` reclaimed 5835 blobs / 2.6 GiB →
store 3.4 GiB → 775 MiB (under budget, `status` over-budget notice gone, `gaps: 0`)**; re-pinned
the 1 drifted fact (`verify --replace-pin`, torture.rs `5729ba1f`→`7a0f3f8c`, reverify appended,
original preserved) → **memory 1 stale → 3 fresh** after seeding 2 genuine file-grounded dogfood
facts (recall verified: both queries return the right fresh fact + pins); installed the
`agentrec-memory` SKILL to `.claude/skills/` (candidate emission now enabled — was staged-only,
the reason 0 candidates had ever been emitted); **`doctor` all-pass exit 0** (incl. `hook presence
pass`). **rich-rate warning diagnosed as honest-bare, not a hook hole** — the hook fires (all 217
lifetime rich turns are `tool=claude`; `doctor` hook-presence pass), the low trailing-20 rate is
genuine non-Claude churn (worktree build-agents in-tree, git ops, torture harness, and this
session's own cargo rebuilds); trust trailing-20 during a quiet single-session dogfood week, not
the lifetime rate. **Two stale launchd services** (`com.agentrec.6c11f4b457d4` status 1,
`com.agentrec.848b7acb02a1` status 78 — prior worktree/temp inits, not running) left in place —
cruft, not harming the dogfood; flag for a cleanup pass. Debugging gotcha this round: a rapid
`sed`-neuter/`mv`-restore cycle produced a **stale-build false-RED** (test failed on restored code
until a clean rebuild) — always let cargo settle between RED/GREEN, verify the source with `grep`.
**Not merged/pushed** (no ask) — branch `fix/purge-orphans-gc` at `c881cfe`; the 2.6 GiB
`objects.archived.<ts>/` is retained (reversible) pending the founder's confidence to `rm` it.
The 1-week memory-dogfood ladder clock can now start on a clean, under-budget, 3-fresh-fact store.

**Review-findings fix round (2026-07-17, branch `fix/review-findings-043c749`, commits `1b638c6..eda01b5`):** Skeptical review of `1e3c63b..043c749` (14 commits/~1.9k lines, fanned out to 4 concern-scoped skeptics + firsthand merge-resolution/cross-seam checks) surfaced 5 findings; all 5 fixed this round, binding **skeptical-reviewer** done-gate verdict **GATE PASS** (refuted #1/#2/#3 by neutering → RED → restore; #4/#5 confirmed comment-only via diff filter; no new defects; tree restored clean). **336 tests, 0 failed, 1 ignored** (+3), clippy `-D warnings` + fmt clean, serial `--test-threads=3`. Findings: **#1 (MED, common-path)** `1b638c6` — `show <undo> --prompt` mislabeled every undo turn (`tool:agentrec`, synthetic `"undo of <id>"` excerpt, `prompt_ref:None`) as `"write failed at record time"` + exit 1 while `status` showed no DEGRADED, because the D35 discriminator keyed on `prompt_excerpt.is_some()` alone; fix excludes synthetic turns (`tool` agentrec/git) from the put-failure branch and hedges the message to cover the over-cap shape too. **#2 (LOW)** `473f62b` — `config_home()` accepted a relative `XDG_CONFIG_HOME` (→ CWD-relative systemd unit path); now requires `Path::is_absolute()` per XDG basedir spec. **#3 (LOW→data-safety)** `cb3386d` — new `cli/src/loglock.rs` (`log.lock`, mirrors `memlock.rs`): `undo --confirm` (sole non-daemon writer) takes it BLOCKING around both appends via `append_log_locked`, `purge --log-duplicates` holds it NONBLOCKING across archive+recheck+rewrite+rename — closes the microsecond recheck→rename TOCTOU where a racing undo append was lost from both `log.jsonl` and the archive; daemon persist stays off the lock (hot path, excluded by liveness refusal + length recheck). **#4 (LOW, docs)** `0fe3c17` — ci.yml `--test-threads=3` comment corrected: it's a no-op on 3-vCPU macos-14 (the named FSEvents platform), only trims Ubuntu 4→3, and the workflow is unpushed/never-CI-run. **#5 (INFO, docs)** `eda01b5` — engine `fold_recent_bares` comment now notes `opened_at >= cutoff` also bounds the bracket path (b3ceb84 msg imprecise), strictly safe-direction (excludes only, never fabricates). **Known residuals (documented, not closable in-session):** #3 sub-recheck→rename window vs a daemon defeating the liveness guard (advisory flock, single-user scope); #4 needs a real GitHub CI run to verify; #1 over-cap sub-case still points at a silent `status` (near-unreachable >10 MiB). Not pushed/PR'd (branch only). The `1e3c63b..043c749` range itself reviewed clean otherwise (engine trio, purge log-repair, merge resolutions, state.json + pathenc×sanitize cross-seams all PASS); memory dogfood untouched by the range and still cold (1 stale fact — `verify --replace-pin` candidate — + 1 injection).

**Debt-burn round (2026-07-17, `main`, `ed21bc8..4da51b5`):** 5 parallel **Sonnet** worktree
agents (one isolated git worktree each — no shared-tree clobber), Opus orchestrator, binding
**skeptical-reviewer done-gate in an isolated worktree: GATE PASS** (all 10 ACs MET; refuted the
4 highest-stakes guards by neutering each in-worktree → named test RED → restored byte-identical).
**333 tests, 0 failed, 1 ignored** (was 299; +34), fmt/clippy clean debug+release, torture_smoke
green, `strings` on release binary shows no `AGENTREC_TEST_PAUSE_*`. Landed: **(D-PD6)** one
turn renderer — `fmt::SEP` (` · `) + `turn_list_line`/`turn_detail_header` in `fmt.rs`,
`format_turn`/`render_turn` now thin wrappers, third renderer (diff `header_line`) found and
SEP-unified, `--json` byte-identical, SPEC.md sample reconciled; **(log repair, Option 2)**
`purge --log-duplicates` — archive-fsync-first rewrite dropping only exact `same_revert` same-id
dups (pre-fix-daemon residue), ambiguous pairs/epoch/unknown-type lines byte-preserved, daemon-
liveness refusal, concurrent-growth length-recheck abort (TOCTOU narrowed-not-closed, documented
— no new log.lock, deliberate); **(watch-list)** the 2 remaining runtime test-pause seams
cfg(debug_assertions)-gated (+ the round's own new log-rewrite seam, caught at integration —
parallel agents drift: one agent re-introduced the exact pattern another was concurrently gating),
`sanitize_terminal` now strips C1 (U+0080–U+009F, CSI/OSC) not just C0+DEL, F4 advisory-flock
scope documented README+spec; **(engine)** stop-only fold bounded by `FOLD_WINDOW_MS` on **two**
paths the agent found (fold filter never bounded a bare's own `opened_at`; direct Quiet→rich stop
conversion had no bound at all), non-UTF8 paths skip+count (`non_utf8_path_skips`, lossy
conversion would break undo/blame hash keys; APFS rejects non-UTF8 at syscall so e2e is
Linux-gated), `.git/packed-refs` admitted to git-turn classification; **(small)** CI integration
`--test-threads=3` cap (FSEvents de-flake; checkout@v5 + `--no-fail-fast` were already landed —
prior "owed" notes stale), XDG_CONFIG_HOME via single `config_home()` resolver in `service.rs`
(launchd untouched), D35 prompt-put-failure taxonomy (`prompt_put_failures` in persist +
recover_orphan, status/doctor DEGRADED, `--ack-degraded` clears, `show --prompt` 4-way honest
message). Doctor store-health now fails on all three counters (non-UTF8 leg wired at integration,
RED-proven). **Deliberately NOT done: P3 polish** (no enumerated list exists — needs founder
pointer). Skeptic residual risks (accepted): length-recheck TOCTOU microsecond window;
Linux-specific legs (XDG/systemd, non-UTF8 e2e) verified via CI matrix not local Linux. Not
pushed (no ask).

**D36 closed + repo truth-up (2026-07-17, `main`):** **D36 7-consecutive-green-nights torture
streak is CLOSED** — 7 consecutive scheduled `nightly.yml` runs green 2026-07-11 → 2026-07-17
(run ids in VERIFY-LEDGER "Closed here" row; wall-clock-derived per-night seeds, macOS + Ubuntu
legs, 0 invariant violations). That was the last M3-era launch gate. Ledger updated accordingly.
**Stale-note corrections after `gh` re-auth:** [PR #2](https://github.com/ravi1395/agentrec/pull/2)
was in fact **squash-merged to `main` on 2026-07-12** (`7a83628`) — the "not pushed/PR'd, left on
`feat/memory-v1` for the founder's call" notes in the two rounds below were already stale when
written into history; `git diff main feat/memory-v1` shows **all F1–F10 code is in `main`**, the
only real delta was the F8–F10 doc-reconcile (`9b09f15`: memory design-spec + plan edits), now
ported onto `main`. Live dogfood observations (this repo, daemon recording since the post-publish
re-init 2026-07-11: 832 agent turns, 0 recording gaps, daemon live): (a) `status` warns
`rich-rate 45%` (<90% trailing-20) — distinguish genuinely-bare human/other-tool windows from a
hook-coverage hole before trusting the warning; (b) **store 3.1 GiB over the 2.0 GiB budget yet
eviction reports `0 B freed`** — keep-set/eviction interplay at real scale needs investigation
(possible A5-residual class beyond the sole-turn case); (c) memory store holds 1 fact, now
**stale** (its `cli/tests/torture.rs` pin drifted; `verify --replace-pin` candidate), 1 lifetime
injection — the 1-week memory-dogfood ladder row is effectively not started. Docs-only round, no
code changed.

**Phase 2–3 design-spec round (2026-07-12, `main`, uncommitted):** Added
`docs/superpowers/specs/2026-07-12-agentrec-phase-2-3-design.md`, grounding MCP/Codex/VS Code,
PR reporting/signing, and Sutra rebase in current source plus current official Codex hooks/MCP
docs. The recommended architecture extracts typed `RepositoryView`/`UndoCoordinator` seams
before adapters, gates Phase 2 on Phase 1 import/conformance/commit-association truth, and
treats Sutra as a control surface over the external recorder. Adversarial reviewer looped
twice on same-root hook correlation, atomic undo reservations, JCS semantics, PR token safety,
signature trust inputs, and honest legacy migration; final verdict **GATE PASS**. No runtime
code changed. Founder confirmation remains blocking on: VS Code via long-lived MCP vs a TS
reader, exact-only PR association, and external-recorder ownership for Sutra.

**5.6-sol spec-gap fix round F8–F10 (2026-07-12, branch `feat/memory-v1`, commits `78cf735..4a368f3`):** Closed the three PR #2 second-pass spec findings (F8/F9/F10) that codex **gpt-5.6-sol** raised via `/code-review` (Spec axis FAIL, captured in `HANDOFF.md`). Opus orchestrator, one **Sonnet-medium** implementer per finding (TDD RED-first, one commit each, **sequential** on the shared tree — heavy overlap: memory.rs in F8/F10, cmds.rs in F8/F10, integration.rs in all — parallel would clobber). **Binding done-gate = codex `gpt-5.6-terra` adversarial `codex exec` review** (per founder instruction, not the Opus skeptic this round). **Pre-flight:** the 6 "unattributed" integration failures HANDOFF flagged were **environmental, not regressions** — leaked debug test daemons (from a prior run) + parallel-FSEvents contention; killed the leaks → full integration suite green. All three gaps **independently verified REAL** (Explore agent + firsthand code read + spec: design §L231 literally promises `verify` re-pin, §L84 the latency budget). Fixes: **F8** `78cf735` hard **outer wall** on the UserPromptSubmit hook recall — runs recall on a detached worker thread, waits only the remaining ~50ms via `mpsc recv_timeout`; timeout → fail-open (inject nothing, exit 0, one `budget_exceeded` stat), so a single blocking `fs::read` of a pinned file can no longer overrun the budget (was cooperative+retrospective only). **F10** `f75267f` corrupt-store observability — `load_effective_checked` returns a distinct `store_corrupt` signal (existing non-empty memory.jsonl with any unparseable line / unopenable) vs healthy-empty; recall bails (never a partial fact), hook appends `{"failure":true,"reason":"store_corrupt"}` (bounded enum, never raw fs/error text), `status` shows an aggregate memory-failure count from the append-only stats — no longer indistinguishable from "no matches". **F9** `742ef04` explicit orphan re-pin — `verify <id> --confirm --replace-pin <old>=<new>` (repeatable) re-points an existing pin to a successor validated exactly like `remember` (in-root/exists/non-secret, reuses `validate_pin_path`), appends one `Reverify` under the same id/fact; **no** automatic rename guessing (a wrong guess would re-ground a fact against the wrong source); closes the sole-pin-rename dead end. **F7-style docs reconcile** `9b09f15` (README `--replace-pin` example + refusal behavior; IMPLEMENTATION INV-M4 corrected — the budget is now a real outer wall, not "hard via cooperative checks"; 7 trailing-whitespace plan lines stripped → `git diff --check` clean). **Terra gate loop (3 rounds):** round 1 **FAIL** — F8.1 the slow-read test seam (`AGENTREC_TEST_SLOW_PIN_READ_MS`) was an **unconditional runtime env read** (prod-reachable arbitrary sleep in every `hash_pin`), F10.6 the "concurrent daemon" test raced only an in-process writer (no real daemon, never checked state.json), F9.4 the refusal test asserted no-append but not empty-**stdout**; round 2 remediation `46f9968` — gated both seams (F8's slow-read **and** the pre-existing `TEST_FORCE_BUDGET_EXCEEDED_VAR`, closing the prior round's watch-item) behind `#[cfg(debug_assertions)]` (compiled out of release — `strings` on the release binary confirms neither var present), added stdout-empty asserts on all 7 F9 refusal classes (RED-proven by hoisting the print), added a **real** `agentrec record` daemon corrupt-store test; **still FAIL** — the new daemon test's `signal_offset>0` liveness proof was **flaky** (one-shot sample raced teardown SIGKILL); round 3 de-flake `4a368f3` — `wait_for_live_daemon` on spawn + bounded 5s poll for `signal_offset` to advance **while the daemon is alive** (proves concurrent signal consumption without the teardown race; still fails on a dead daemon — RED-verified), 15 solo runs clean. **Terra round-3 verdict GATE: PASS** — F8.1/F9.4/F10.6 all PASS with tests that go red when the fix is removed; AC-F10.6 6/6 solo green. **299 tests, 0 failed, 1 ignored** (torture heavy) + 1 release-only F8 seam test (was 289; +10 net); clippy `-D warnings` + fmt clean on **both** debug and release profiles; `git diff --check` clean. **Known non-blocking watch-item:** `cargo test --workspace` at full parallelism intermittently fails ~6 daemon integration tests via **FSEvents/watch contention** (many real daemons spawning at once) — pre-existing, not introduced here (same set failed at session start before any edit); each passes reliably alone or under `--test-threads=3` (integration 86/0 clean). CI should cap integration test-threads or serialize the daemon-heavy tests. Not pushed/PR'd (user didn't ask) — left on `feat/memory-v1`. HANDOFF.md deleted (its F8–F10 tasks are complete).

**PR #2 review-findings fix round (2026-07-12, branch `feat/memory-v1`, commits `77fa65c..7b27df0`):** Orchestrated the F1–F7 residue from the three-axis PR #2 review (HANDOFF.md) — Opus orchestrator, one **Sonnet** implementer per finding (TDD, RED-first, one commit each, run **sequentially** on the shared tree because of heavy file overlap — memorycmds.rs in F1/F3/F4/F6/F7, daemon.rs in F4/F5/F6, memory.rs in F2/F3/F7 — parallel would clobber), binding **skeptical-reviewer** done-gate in an **isolated worktree**. **9 commits, 289 tests, 0 failed** (was 275; +14), clippy `-D warnings` + fmt clean. Two founder decisions taken up front (not silently defaulted): **F2 → option (a)** hard cooperative deadline (not the cheaper spec-amend), **F7 → implement** the promised behaviors (not amend spec to match impl). Findings: **F0** `77fa65c` committed the carried-over curative dedup (`resolve_turn`/`same_revert`, supersedes the prior "not committed" note below). **F1** `6246cb0` route every rendered fact/pin/reason — incl. the `--for-hook` block — through `fmt::sanitize_terminal` (spec §Security; agent-emitted `\x1b]0;…\x07` no longer renders raw). **F2** `9b6e570` threaded a cooperative `Instant` deadline into the recall load/rank/verify loops (`recall_with_deadline` → `RecallOutcome{hits,budget_exceeded}`); expired deadline bails to empty + `budget_exceeded` stat in `memory-stats.jsonl`, exit 0, no stdout block — makes the "hard 50ms" claim true where it was retrospective-only; the manual `recall --for-hook` CLI stays unbounded (skeptic ruled it an honest scope boundary — the real UserPromptSubmit hook is `hook claude`→`inject_memory`, the bounded path). **F3** `8622e4d` `capped: bool` on `RecallOutcome`; human `recall` prints a **stderr** notice when the 128-cap verify walk truncates, hook records `capped` in stats (never stdout) — closes the silent-truncation-looks-like-no-match honesty gap; cap added to spec §Read path. **F4** `c352f65` (highest stakes, never-delete-user-data) dedicated `.agentrec/memory.lock` (0600, NOT daemon.lock — would deadlock): all writers (remember/verify/forget/daemon ingest_candidate) take it around append; `purge --memories-retracted` holds nonblocking `LOCK_EX` across archive+rewrite+rename, refuses loudly if unavailable — `purge_rewrite_never_loses_concurrent_append` loses the record pre-fix, preserves it post (real subprocess flock race, deterministic ordering not sleeps). **F5** `29bd88b` explicit `apply_signal` dispatch: unknown `type` (present, ≠memory-candidate, no stop event) → ignore + `unknown_signal_ignored` state counter, turn stays open; legacy no-type stop still closes (PROTOCOL additive-versioning rule enforced). **F6** `d2ffaf7` behavior-neutral polish — char-safe `short_id` (`chars().take(8)`, no non-ASCII panic), dead `#[allow]` deleted, `already_logged` data-clump → `&OrphanJournal` (cb5dcd1 dedup byte-preserved), 5th `wall_now_ms` + 3rd config-scanner deduped; tmp+fsync reuse **skipped** (put_result is hash-fanout, can't express fixed-path rewrite — skeptic ruled legitimate). **F7** `a002a33`+`7b27df0` docs reconcile (engine.rs `source_turns` cite → design spec, memory.rs drops false "protocol-additive", spec "provably" softened, reserve-id-at-OPEN recorded in Decisions log) + **implemented** `memories --stale` drift-join (names the drifted pin + when via turn-log join) and `verify <id>` CAS diff summary (reuses `agentrec-core/src/diff.rs`, honest "blob unavailable" fallback when purged, all sanitized). **Skeptical-reviewer GATE PASS** (isolated worktree at `7b27df0`; 289/0/1, clippy+fmt clean; **refuted** F2/F4/F5 by neutering each fix in-worktree → RED → restored byte-identical). Non-blocking watch-list (skeptic): 2 test-only env vars are runtime reads not `cfg(test)`-gated (both safe-direction — fail-open / a sleep, neither corrupts data); `sanitize_terminal` strips C0+DEL not C1 (0x80–0x9f, rarely-honored, low risk); F4 flock is advisory (all prod writers routed, external hand-rolled writer unprotected — out of scope single-user tool); F4/F5 verified on macOS only — **recommend one Linux CI leg + the still-open `--no-fail-fast` chip before merge**. Not pushed/PR'd (user didn't ask) — left on `feat/memory-v1`.

**Read-side curative dedup (2026-07-12, branch `feat/memory-v1`):** Closed the first PR #2 follow-up chip — the engine fix (`cb5dcd1`) was **preventive** (stops a post-fix daemon writing a same-id dup), so a `log.jsonl` **already** carrying two same-id `TurnRecord`s (written by a pre-fix daemon that hit the kill-9 window) still broke `undo <full_ulid>` with "ambiguous turn id — matches 2 turns". Made `resolve_turn` (`cli/src/readcmds.rs`) **curative**: on ≥2 matches it collapses them to one iff they'd revert the worktree identically — same `id` + same `FileEntry` set (path + before/after hashes + op + flags, order-independent) — via new `same_revert`. `diff`/`show`/`undo` all inherit the cure (single choke point). **Discriminator is exact, not naive:** advisor + a field-by-field read of `persist` (l.1084) vs `recover_orphan` (l.1253) proved a real recovery double-emit drifts on **more than `ended`** — recovery forces `model: None` and `truncated: true` (bracket turn), so a "compare all fields except timestamps" rule would wrongly REFUSE to collapse a real dup; and "id + path-set only" is too loose (differing before/after → different revert → must stay ambiguous, never silently pick one). Keying on the full `FileEntry` set is the precise undo-safety invariant. TDD: RED reproduced the exact prod error; strengthened test `undo_collapses_orphan_recovery_duplicate_same_id` seeds the dup with real `model`+`truncated`+`ended` drift (proven to fail a naive `model`-comparing discriminator, pass `same_revert`); guard test `undo_still_errors_on_distinct_turns_sharing_id` (disjoint files) stays ambiguous. **275 tests, 0 failed** (+2), clippy `-D warnings` + fmt clean; `TurnRecord` derive untouched (no speculative `PartialEq`). Still open from PR #2: read-side migration/repair path NOT built (Option 2 — deferred, `resolve_turn` cure makes it non-urgent) and `--no-fail-fast` in CI workflow (second chip). Not committed (user didn't ask).

**PR #2 CI-red fix (2026-07-12, branch `feat/memory-v1`, commits `76a716d`/`cb5dcd1`):** [PR #2](https://github.com/ravi1395/agentrec/pull/2) CI red on Linux only (macOS + clippy green). **Two failures, one masked behind the other.** (1) `hook_fail_open_and_budget` (integration.rs) asserted a real injection at 3001-record scale, but the hook's 50ms `RECALL_BUDGET_MS` self-budget is measured around an in-process recall in a **debug** test binary — a debug BM25 fold over 3001 records on a 2-core CI runner blows 50ms and correctly fails open to a no-op, so the assertion was coupled to runner speed (fast macOS green, slow Linux red). Fixed by **decoupling**: anti-silent-degradation now proven at a runner-speed-independent ~200-record store (deterministic hard injection); the 3000-record leg asserts only exit-0 + 500ms wall (fail-open-under-load); the true 50ms@3000+/10k envelope stays laddered in VERIFY-LEDGER, never a per-push gate. (2) Fixing #1 **unmasked** a latent `torture_smoke` failure — `cargo test --workspace` has no `--no-fail-fast`, so run 1 stopped at the integration binary and never reached the torture binary. Real engine bug: `undo <ulid>` → "ambiguous turn id — matches 2 turns" (**duplicate turn id** in `log.jsonl`). Root cause: `recover_orphan`'s idempotency guard `already_logged` keyed on root+**started+ended**+files, never turn id; a turn closed by an incoming start signal is logged with `ended=now` (engine `observe_start`→`finish`) while its crash journal stored `last_change_wall_ms` (< now), so recovery's recomputed `ended` mismatches and it re-appends — and since the **memory-v1 "reserve id at OPEN" change** made ids stable open→journal→recover, that duplicate now carries the **same id** (pre-memory-v1 it minted a fresh id → benign distinct-id dup). Reachable via the kill-9 window between `persist` and the `sync_journal` that clears `open.json`; Linux-timing-exposed under the fixed torture seed, invisible on macOS. Fixed: `already_logged` dedups on turn id first (unique + stable), content match kept as legacy pre-reserved-id-journal fallback (serde-default fresh ULID can't false-match) + deterministic regression test `recover_orphan_skips_duplicate_by_id_even_when_ended_drifted` (fails pre-fix `left:2`, passes post). **[CI run 29196010340](https://github.com/ravi1395/agentrec/actions/runs/29196010340) all 5 jobs green** — both `hook_fail_open_and_budget` and `torture_smoke` `ok` on the exact ubuntu-22.04/24.04 runners that failed. Isolated-worktree **skeptical-reviewer GATE PASS** (273 tests, clippy `-D warnings`, fmt clean; refutation: reverted id-check → test failed, restored → passed; verified the only two `Turn`-append sites — steady `persist` + guarded recovery — so the sibling double-emit class is closed). **Two non-blocking follow-ups flagged (task chips):** the fix is **preventive not curative** — a `log.jsonl` already carrying a same-id dup (written by a pre-fix daemon) still breaks `undo <id>`; no read-side dedup/migration. And add `--no-fail-fast` to the CI workflow so co-occurring failures surface in one run. Process note: the first skeptic subagent ran `git reset` in the **shared** working tree (I'd suggested "revert the fix to test it") and clobbered the local commit — recovered from the object store, re-gated in an isolated worktree; a `git reset --hard` guardrail hook fired correctly during recovery.

**Memory v1 round (2026-07-12):** Subagent-driven build of the hash-pinned semantic-memory feature (spec+plan committed `06ac228`/`a050c13`/`d5aecf2`) — Opus orchestrator, **Sonnet** implementers (one per plan task), per-task **Opus** review, binding **skeptical-reviewer** done-gate. 12 tasks → **19 commits** (`d5aecf2..43e47eb` on `feat/memory-v1`), **269 tests, 0 failed** (was 233; +36), clippy `-D warnings` + fmt clean. Feature: facts pinned to file sha256 hashes, lazily verified fresh at read time (`memory.jsonl` append-only, deterministic ts-ordered fold with total-order tiebreak), written manually (`remember`) + by agents (`candidate` → daemon `ingest_candidate`), recalled via from-scratch BM25 rank-then-verify (`recall`/`memories`, fresh-only), injected via the UserPromptSubmit hook (budgeted, fail-open, never writes `state.json` — hook-owned `memory-stats.jsonl`), lifecycle `verify`/`forget` (append reverify/retract, origin-provenance preserved), `purge --memories-retracted` (only sanctioned rewrite: archive-fsync-then-atomic-rename, daemon-liveness flock guard). New: `agentrec-core/src/memory.rs`, `cli/src/memorycmds.rs`, `claude-setup/skills/agentrec-memory/SKILL.md`; +`memory-candidate` signal type (PROTOCOL §4, additive, routed around the stop arm — the non-negotiable spurious-stop guard, reviewer-verified by removing it → 2 fabricated turns). One engine change: turn id now reserved at OPEN (was lazy-at-close) so `source_turns` references a live turn — threaded through the crash journal, reviewer-proven reserved-id==closed-id. **Independent codex (gpt-5.6-terra, CLI 0.144.1 after the user upgraded mid-round) cross-review ran alongside the Opus gates and earned its keep** — found 2 real bugs the per-task reviews missed: equal-ts fold nondeterminism (INV-M5) and a crash-window where a candidate's `source_turns` could dangle after kill-9 (fixed: journal open turn before the referencing candidate is fsynced). Done-gate loop: **round 1 GATE FAIL** (INV-M4 concurrent-append leg untested; `memory_enabled` gated only injection not ingestion — a binding-spec §184 violation + wrong README; INV-M5 same-op same-ts tiebreak) → 3 targeted fixes → **round 2 GATE PASS** (all 5 INV PASS, INV-M5 a real total order not "unreachable"). One in-round loop-back caught by review: `purge` had a silent data-loss window vs the concurrent daemon appender (guard added). 3 accepted v1 limitations (local single-user, no-network — PROTOCOL hard rule): validate→hash TOCTOU in `ingest_candidate` (only a hash lands on disk, INV-M3 intact, self-corrects at recall), best-effort non-fsynced recovery journal under power-loss (pre-existing M1 posture), `purge` vs concurrent manual writers (documented). **Laddered (env-gated, NOT done — VERIFY-LEDGER rows):** 10k/50ms hard perf envelope, real-Claude-Code-session injected-block proof, 1-week dogfood with `status` counters, ≥1 useful skill-emitted candidate. No merge/PR run — left on `feat/memory-v1` for the founder's integration call.

**doctor inotify-estimate fix (2026-07-11, draft [PR #1](https://github.com/ravi1395/agentrec/pull/1), branch `worktree-fix-doctor-inotify`, commit `982b625`):** Surfaced while measuring agentrec's real resource cost (idle ~6–10 MB RSS, ~0.018% CPU over 29 h; active ~100 ms CPU + ~1:1 disk per MB touched — all bounded by 10 MiB/blob + 2 GiB store caps). `doctor`'s **inotify headroom** check undercounted the watches the daemon actually arms → could report **PASS while the daemon dies at startup** on `max_user_watches`. Root cause: daemon watches the whole root recursively (`daemon.rs:76`, `RecursiveMode::Recursive`) and filters *events* afterward via `IgnoreSet`, not which dirs are watched — so `notify` arms one inotify watch **per directory** over the entire tree, but `estimate_watch_count` used a gitignore-aware `ignore::Walk` that (a) dropped every gitignored dir (`target/` ≈ 600 here) and (b) didn't follow directory symlinks (a pnpm `node_modules` is almost all dir symlinks — notify follows + watches each). Both undercounts, the false-PASS direction. **Fix** (`cli/src/doctorcmd.rs`): rewrote the estimate to **mirror notify-6's `INotifyWatcher::add_watch` exactly** — `walkdir::WalkDir::new(root).follow_links(true)` counting every entry whose `.metadata()` is a dir (same crate/flags/`filter_dir`/loop-handling as `notify-6.1.1/src/inotify.rs`, source-verified); made it **threshold-aware** (early-stop once count exceeds the caller's limit — a fixed cap could false-PASS when the limit is raised above it; returns a lower bound, message says "at least N"; hard 2M `CEILING` with the >2M/>2M residual documented honestly); **un-gated** the pure helper to `cfg(any(target_os = "linux", test))` so it's unit-tested on every platform though its caller stays Linux-only. **236 tests, 0 failed** (was 233; +3 new, each proven to FAIL on the old impl: gitignored-subtree delta +3, symlink-follow delta +2, stops-above-limit). fmt/clippy `-D warnings`/`cargo test --workspace` all exit 0; Linux-only `check_inotify` body compile-checked on macOS via a temporary `cfg(test)` flip. Independent skeptical-reviewer: first pass **GATE FAIL** (caught a masked `cargo fmt --check | tail` exit-code bug that hid a real fmt failure — fixed + amended), re-check **GATE PASS**. Three self-skeptic passes each caught a real flaw first: false ".git hardcode-skip" claim (disproven by the regression harness), `MAX_DIRS=100_000` < common raised limit `524288`, and notify's `follow_links(true)`. Laddered (unchanged): `check_inotify`'s runtime `>`-vs-`/proc` fail path still needs real-Linux CI (`inotify-low-watches` job / VERIFY-LEDGER) — this round's Linux proof is compile-only.

**Pre-launch hardening round (2026-07-11):** Full-repo skeptical scan → 2-wave Sonnet fix → Opus adversarial gate, verdict **GATE PASS** (all 32 findings PASS, none FAIL/UNTESTED). 4 parallel scanners (engine / store+security / daemon / CLI) surfaced 32 real defects: **1 P0** — `object_path` joined unvalidated hashes, so a crafted `before`/`after` string in `log.jsonl` made purge/eviction delete arbitrary absolute paths (now: strict 64-lowercase-hex validation, `Option` return, malformed = never touches disk); **13 P1** — engine: `fold_recent_bares` stole pre-bracket human bare turns into agent rich turns (fabricated attribution), long-open quiet turns swallowed git checkouts as 400-file bare turns, retro-merge kept the *later* fragment's `before_hash` (undo restored mid-turn state), first-obs dedup froze `deleted`/`snapshotted`/`withheld` flags (wrong `op`); daemon: SIGTERM unhandled (ctrlc default = SIGINT-only; every launchd/systemd stop was an unclean kill — now `termination` feature), pid-JSON lock TOCTOU → real flock on `.agentrec/daemon.lock` (also fixes doctor PID-recycling false-pass), runtime watcher errors silently swallowed (now DEGRADED counter), wall-clock drift under laptop sleep (now 2s re-anchor with monotonic floor), service units had zero escaping (spaces/`%`/`&` broke systemd/plist); scrub: entropy threshold 4.5 bits/char was mathematically unreachable for hex (log2(16)=4.0 — 64-hex tokens leaked; now alphabet-aware + 40+-hex rule), quoted multi-word secrets bypassed the credential regex, `scrub()` flattened all prompt whitespace; CLI: partial undo failure left reverted files recorded nowhere (now integrity pre-flight + partial-turn append), clean daemon stop→start was never a "recording gap" (blame guessed "human"); **~18 P2** (cross-type purge keep-sets on the shared CAS, signal-tailer truncation deafness + O(file) reads, journal double-apply, walks descending `.git`/`.agentrec`, uninstall deleting unrelated hook keys, init clobbering unreadable settings, terminal-escape injection via prompt excerpts, concurrent-undo guard clobber, `--files` silent no-op, more). **233 tests, 0 failed** (was 157; +76 across new `cli/tests/hardening_{store,cli,daemon}.rs` + inline), clippy `-D warnings` + fmt clean, no existing test weakened (gate-verified). Declared deviations, both honest: A5 residual (a *sole* over-budget turn still evicts its own blob — pinned by existing `status_prints_over_budget_notice`; multi-turn case now protects newest) and D11 (service-reload code fixed, runtime proof laddered — new VERIFY-LEDGER row). Skeptic watch-list (non-blocking): stop-only fold path has no pre-turn bound (pre-existing, by-design until v2 Codex stop-only emitter — add bound + test then), dedup-hit now re-reads blob (perf, watch in dogfood), `state.json.tmp.<pid>` crash litter. Deferred (round ledger): non-UTF8 paths, `.git/packed-refs` classification, XDG_CONFIG_HOME, prompt-put-failure D35 taxonomy, P3 polish. Codex cross-review attempted but blocked (CLI 1.0.4 too old for gpt-5.6-*; ChatGPT account rejects gpt-5.1/5.3) — rerun after Codex upgrade if wanted.

**v0.1.0 + Y+1/Y+2/GIF round (2026-07-10):** Closed every laddered gap except D36. (1) **README blame GIF** — `docs/blame-demo.gif`, rendered with `vhs` (`docs/blame-demo.tape`) against a real fixture repo (`docs/generate-blame-fixture.sh` drives a genuine `init`/`record`/`hook`/`blame` session, no staged text). (2) **Fixed a real shipping bug**: `install.sh`'s default `release_base` and README's `curl` line pointed at `github.com/agentrec/agentrec` — wrong org, would 404 every real install; reconciled to `ravi1395/agentrec`. (3) **Cut v0.1.0**: `.github/workflows/release.yml` (tag-triggered) builds the 4-target binary matrix (macOS arm64/x86_64, Linux x86_64/arm64) and publishes a real GitHub Release. First attempt caught a genuine bug the Y+1 leg exists to catch — Linux x86_64 built on `ubuntu-24.04` (glibc 2.39) failed `GLIBC_2.39 not found` on `ubuntu-22.04` (glibc 2.35); fixed by building on the older `ubuntu-22.04` baseline (glibc is forward-compatible), then v0.1.0 was deleted and recut clean. (4) **Y+1 CLOSED** (both legs, for real — not the local-binary leg): run [29112445275](https://github.com/ravi1395/agentrec/actions/runs/29112445275), all 9 jobs green — the README's literal `curl \| sh` proven via real network fetch + checksum verify + PATH install on fresh `ubuntu-22.04`, `ubuntu-24.04`, `macos-14` (arm64), and `macos-15-intel` (x86_64) runners. (5) **Y+2 CLOSED**: new public tap `ravi1395/homebrew-agentrec` (`Formula/agentrec.rb`, sha256s cross-checked against the release's own asset digests before pinning), verified with a real local `brew tap` + `brew install ravi1395/agentrec/agentrec` → `agentrec --version` → `agentrec 0.1.0`, `brew test` passed; tap/install cleaned up afterward. **Only D36 (7-consecutive-green-nights) remains open** — nightly workflow live, 1 green night recorded so far (2026-07-10), needs the clock.

**Y++2 round (2026-07-10):** Closed the last laddered gap from the J3 round — `doctor`'s inotify-headroom **fail** branch was compiling but never actually exercised (Linux CI ran with a generous default `fs.inotify.max_user_watches`, so only the pass path ran). Added `doctor_inotify_low_watches_fails` (`cli/tests/integration.rs`, `#[cfg(target_os = "linux")]`, `#[ignore]`d — self-guards with a loud panic if `max_user_watches` isn't actually low, so it can never silently pass) + a dedicated `inotify-low-watches` CI job (`.github/workflows/ci.yml`) that `sudo sysctl -w fs.inotify.max_user_watches=1` on an ephemeral ubuntu-24.04 runner, echoes the value back (proof it took), then runs the test `--ignored --exact`. Run [29104565488](https://github.com/ravi1395/agentrec/actions/runs/29104565488): **all 5 jobs green** (lint, macOS-14, Ubuntu-22.04/24.04, inotify-low-watches) — `doctor` genuinely returned `fail`/`inotify headroom`/exit 1 on real induced-low Linux state. **Y++2 CLOSED.** Remaining laddered: D36 7-night streak, Ubuntu-container/clean-macOS-VM/brew installer legs, README blame GIF.

**J3 CI + publish round (2026-07-10):** Wired GitHub Actions and open-sourced the repo. **`.github/workflows/ci.yml`** — per-push/PR matrix: `fmt + clippy` (ubuntu-24.04) + `cargo test --workspace` on **macOS-14 + Ubuntu-22.04 + Ubuntu-24.04**; `#[ignore]d` torture heavy-run excluded (torture_smoke still runs). **`nightly.yml`** — cron (07:17 UTC) + `workflow_dispatch`, runs the torture harness `--ignored` on macOS-14 + ubuntu-24.04 with `AGENTREC_TORTURE_SEED` **unset → wall-clock-derived per night** (the D36 7-green-nights vehicle), uploads the log (printed seed → reproducible). First `cargo fmt --check` enforcement caught 2 unformatted asserts in `integration.rs` (auto-fixed). **Relicensed dual MIT-OR-Apache → Apache-2.0** (open-core, founder decision, supersedes D17) — canonical `LICENSE` (sha256 `a60eea81…f759f2`) + `NOTICE`, all doc refs collapsed. Pre-publish safety: untracked machine-local `.claude/`/`.codex/`/`.sutra/`/`.mcp.json` (held a localhost Sutra MCP token + home paths) and **squashed history to one clean initial commit** (token in zero public commits; old history kept locally as `old-history-local`). Published **https://github.com/ravi1395/agentrec** (public). **CI matrix GREEN** — run [29098847491](https://github.com/ravi1395/agentrec/actions/runs/29098847491), all 4 jobs success → **J3 CLOSED**; the Ubuntu legs ran `lock_file_sets_0600`/`lock_dir_sets_0700`/`d37_daemon_writes_land_at_locked_permissions` green on real Linux → **I++1 (Linux perms) CLOSED** too. 157 tests unchanged; clippy clean. **Still open (laddered):** D36 7-night streak (workflow live, needs the clock), Y++2 inotify induced-low-watches (normal path green on Linux CI; no step lowers `fs.inotify.max_user_watches` yet), Ubuntu-container/clean-macOS-VM/brew installer legs, README blame GIF. Minor CI follow-up: `actions/checkout@v4` emits a Node-20-deprecation annotation (non-blocking; bump to `@v5` later).

**PD-review fixes round (2026-07-10):** 6 senior-product-designer findings against the real binary in a fresh repo (2 launch-blocking, 4 polish) fixed under `/ratchet` — Opus orchestrator, 4 Sonnet implementers, Opus skeptic gate. Final verdict **GATE PASS**: all 8 AC ids (PD1–PD5) PASS with a named non-weakened test + pasted real-binary repro (stream + exit code verified). Then, on founder go-ahead, 2 further honest-posture edges the skeptic surfaced were fixed in the same round (commit `0b208a4`): `show --prompt` on a corrupt (hash-mismatch) blob now says `prompt blob corrupt (hash mismatch)` (distinct from `Missing`→`prompt purged — TTL expired`); idempotent `init` that recreates a separately-deleted `.agentrec/objects/` no longer claims `nothing changed` (gate now reflects actual disk mutation, genuine no-op still says `already initialized — nothing changed`). **157 tests, 0 failed** (49 cli-unit + 59 integration + 1 torture-smoke [+1 ignored] + 48 core), clippy clean; baseline was 142 → +15. Commits `9eb5e3d`/`bdb3cb6`/`a769ad6`/`759a779`/`0b208a4` on baseline `e9551e8` (git repo initialized for this round). Delivered: (PD1) `doctor` init-gate — first check `initialized`, absent `.agentrec/` → fail + remedy `not initialized — run \`agentrec init\`` and every other check `n/a`, exit 1, no fabricated `permissions too open`/`agent active but no signals` (`doctorcmd.rs`, additive `--json`); (PD2) `agentrec show <turn> [--prompt]` — bare = header via `readcmds::render_turn`, `--prompt` = full post-scrub blob to stdout, dangling/bare → honest stderr + exit 1, resolver shared with `diff` (`readcmds.rs`, `main.rs`, README row) — closes SPEC §Prompt-posture item 4 excerpt-discipline drift; (PD3) zero-turn `status` → `rich-rate:  n/a (no agent turns yet)`, never vacuous 100%; (PD4) zero-turn `log --json` → `[]` exit 0 (`cmds.rs`); (PD5) doctor check `store degraded`→`store health`, status jargon → `(agent turns; git activity hidden)`, idempotent `init` re-run leads `already initialized — nothing changed`, low-rich-rate remedy → `agentrec doctor` (`doctorcmd.rs`/`cmds.rs`/`initcmd.rs`).
**Tracked debt (this round):** _D-PD6 (deferred, founder-confirmed) — the only open item:_ two parallel turn renderers (`cmds::format_turn` vs `readcmds::render_turn`) + separator drift (`—`/`·`/`->`/`;`) — unify in a later round, NOT bundled here. (The two skeptic-surfaced honest-posture edges — corrupt-blob→false-TTL and init→false-"nothing changed" — were FIXED this round in `0b208a4`, not deferred.)

**Done (as of 2026-07-10):** M1 "Record" + M2 "Answer" + **M3 "Ship" code-complete** — under `/sdd`, opus-4.8-high final gate verdict **SHIP** (every AC verifiable on macOS PASS with pasted command output; environment-gated ACs laddered in `VERIFY-LEDGER.md`, never marked PASS). **142 tests, 0 failed** (46 cli-unit + 47 integration + 1 torture-smoke [+1 ignored gated run] + 48 core), clippy clean, zero orphan daemons. Delivered: (I5/I6/I+) `purge` [expired-prompt default, `--all-prompts`, `--snapshots-before`] with keep-set dedup safety + `MAX_STORE_BYTES` 2 GiB oldest-snapshot eviction (`agentrec-core/src/retention.rs`) + `status` over-budget notice; (I++/D37/D38) umask-independent 0700 dir / 0600 files at EVERY write site via `agentrec-core/src/perms.rs` — **caught+fixed blocker: daemon-created files were 0644, only init-time config was locked** — plus 3-location planted-secret grep test; (Y+3/4/5/D39-40) one-command `init` writes+loads a per-repo launchd/systemd unit (`cli/src/service.rs`, canonicalized root), lists actions + reverse command, byte-for-byte no-op re-run, `--dry-run` (touches nothing), `--no-service`; `uninstall` (`cli/src/uninstallcmd.rs`) command-granular hook removal + archive-only `.agentrec.archived.<ts>` (never deletes); (Y++/D41) `doctor` (`cli/src/doctorcmd.rs`) — daemon-liveness/hook/signal-freshness/DEGRADED/perms/inotify checks, `--json`, exit 0/1, 8 failure-mode tests; (Z+2/3/4/D43) relative times default + `--utc`, `NO_COLOR`/TTY color, `--explain` glossary-for-present-terms (`cli/src/fmt.rs`); (H++/D36) undo torture harness (`cli/tests/torture.rs`) — 1200 randomized ops (bursts/human/git-checkout/kill-9/undo), INV1 modified-since-refusal + INV2 undo-of-undo-byte-exact asserted after every undo, 0 violations, Drop-guard leaves zero orphan daemons, nightly seed now wall-clock-varying; `install.sh` (checksum-verified `~/.local/bin`, no sudo, local-binary leg proven), PROTOCOL.md v0.2 (additive `merges`/`baseline_unknown`), README (line-level-blame + "how we try to break it"). New modules: `agentrec-core/src/{retention.rs,perms.rs}`, `cli/src/{purgecmd.rs,service.rs,uninstallcmd.rs,doctorcmd.rs,fmt.rs}`, `cli/tests/torture.rs`. Launch-gated (laddered, NOT done): D36 7-consecutive-green-nights streak, Ubuntu/clean-VM/brew installer legs, Linux inotify+perms CI, README blame GIF, CI matrix. No git repo → no branch/PR flow run; dogfood week still running.

**Prior (as of 2026-07-10):** M1 "Record" (see below) **+ M2 "Answer" complete** — all M2 ACs PASS under adversarial opus final-gate review (verdict: SHIP). Delivered: (L+/D34) fsynced ledger closes (`append_log` sync_all; signal inbox exempt), atomic race-free blobs (typed `PutResult`, unique `.tmp.<pid>.<seq>`, fsync-before-rename + dir fsync), kill-9-around-close harness + concurrent-identical-blob test; (M+/D35) snapshot-failure taxonomy — persisted `snapshot_failures` counter + `io_failed` list in `state.json`, `status` DEGRADED banner, `status --ack-degraded`, undo says "write failed at record time" vs "over size cap" (wire format unchanged), proven end-to-end via real daemon fault injection; (F1–4) `diff` (unified/binary/skipped/unknown-id); (G1–6) `blame` incl. line-level (before/after snapshot diff) + epoch-gap staleness; (H1–7) `undo` — modified-since (D30) rail, `--allow-modified`, per-file `--files` subset, skipped/withheld refusal, create/delete inverse, undo-is-a-turn (`tool:agentrec`, re-revertible), H7 concurrent via `.agentrec/undo-guard.json`; (Z+1/D42) preview-first panic `undo` targeting last non-git rich turn, refuses on trailing bare/empty. New modules: `agentrec-core/src/diff.rs`, `cli/src/{readcmds.rs,state.rs}`; added `similar` dep. **84 tests, 0 failed** (41 core + 27 integration + 14 cli-unit + 2 added), clippy clean. Known v1 limits (non-blocking): H7 guard drops genuine concurrent edits to guarded paths during the ~3s linger (by-design, bounded); G5 duplicate-identical-line heuristic. No git repo → no branch/PR flow run.

_M1 "Record":_ `agentrec-core` engine (bracketing + retroactive merge, git-turn classification, quiet fallback, crash journal), blob store with integrity-check-on-read, scrub + secret-path withholding, daemon (`record`) with watcher/signal-tailer/kill-9 recovery, `init` (idempotent hook merge), `log`, `status`, `hook`. Dogfood week started 2026-07-06 (M1-exit tripwire closes ~07-13). Design suite amended through v0.4 (durability D34–D38, accessibility D39–D44).

**Next:** M3 launch gates (all in `VERIFY-LEDGER.md`, none code-blocked) — wire CI (`cargo test` matrix macOS-14 + Ubuntu-22.04/24.04, J3) with the torture `--ignored` run nightly using a per-night `AGENTREC_TORTURE_SEED`, then the **7 consecutive green nights** gate (D36); prove the installer's network/Ubuntu-container/clean-macOS-VM legs + author+publish the brew tap (Y+1/Y+2); record the line-level-blame README GIF; Linux-only CI assertions (Y++2 inotify headroom, I++1 perms). Then the launch checklist: one full dogfood week with the DEGRADED banner never falsely firing. Nice-to-haves surfaced at the final gate: broaden torture INV2 coverage (only 2/21 checkpoints executed undo-of-undo per run — the varying nightly seed accumulates it); cosmetic idempotent-`init` re-run messaging.

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
