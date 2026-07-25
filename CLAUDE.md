# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working in this repository.

## What this is

**agentrec** — a local-first, tool-agnostic flight recorder for coding agents. It segments agent activity into *turns*, snapshots every touched file into a content-addressed store, and answers "who broke my repo — me or the agent?" via `log` / `diff` / `blame` / `undo`. The turn engine is an extraction of the production-proven `turns.rs` from Sutra (`~/Projects/sutra/src-tauri/src/turns.rs`) — when in doubt about engine semantics, that file is the reference implementation.

## Status (update after every delivery round — house rule)

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
claimd provides. Also: `.claims/lint.ignore` does **not** exist despite an earlier note saying it
was seeded — that note is stale. The two D29 claims went STALE (daemon.rs touched) and were
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
