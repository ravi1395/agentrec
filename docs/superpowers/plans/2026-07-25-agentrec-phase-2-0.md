# Plan: agentrec Phase 2.0 — truth substrate, gate-critical chain (5 phases, worktree/branch: `feat/phase-2-0-substrate`)

Spec: `docs/superpowers/specs/2026-07-18-agentrec-phase-2-design.md` (§Phase 2.0,
hardened 2026-07-28 — decisions 5–8 bind this plan).
Evidence: `docs/2026-07-24-phase-2-0-entry-gate-measurement.md`.
Conflict order: PROTOCOL.md > IMPLEMENTATION.md > spec > this plan.

**STALE LADDER WARNING (2026-07-28):** every absolute test count below (349 baseline,
361/371/392/410/424 per phase) predates later rounds — `main`'s recorded baseline is
**428 passed / 0 failed / 1 ignored** (re-verified at `834f477` per CLAUDE.md), and
the unmerged `fix/perf-evidence-round` branch measured **443/0/1**. The per-phase
*deltas* (+12, +10, +21, +18, +14) are the plan's real content; re-base the ladder
against a fresh `cargo test --workspace -- --test-threads=3` run on the actual
cut-point branch at chunking time. This repo's rebuild-gate round shipped a wrong
ladder twice; do not repeat it by trusting these absolutes.

**Goal:** retire the Phase 2.0 hard stop (import honesty) and land the read seam +
machine-readable contracts every Phase 2 consumer depends on. Architecture: one
semantic core (`RepositoryView` in `agentrec-core`), thin adapters (CLI renderers,
later MCP). Adapters never reinterpret turn selection, gaps, blame, or undo safety.

**Scope of THIS plan (explicit, not a silent narrowing):** the gate-critical chain
only — import gate → import persist → golden harness → `RepositoryView` → `--json`.
Phase 2.0 wave 2 (below) and Phases 2.1–2.4 are named, not detailed, because they
consume seams that don't exist yet or sit behind the un-retired gate.

## Decisions log (founder-confirmed; executors may not re-litigate)

1. VS Code deferred out of Phase 2 entirely (spec decision 1).
2. PR↔turn association is exact-only via `Agent-Turn:` trailers — no heuristic mode, ever (decision 2).
3. Sutra = external agentrec recorder + sidecar; `RepositoryView`/`UndoCoordinator` are the interfaces it will consume (decision 3). *(Superseded in part 2026-07-28 by spec decision 9: Sutra parked; seams justified by CLI `--json` + gated MCP alone; no Phase 2 work touches the Sutra repo.)*
4. Phase 2.0 carries full ROADMAP Phase 1 scope (decision 4).
5. Import never fabricates a revertible snapshot. Unreconstructable `before` → `before: null`, provenance-only, refused by undo with an explicit imported-history reason.
6. Bare turns stay unattributed windows in every renderer.
7. `baseline_unknown` is NOT reused for import-missing-`before` — it keeps its live first-observation meaning (spec line 229).
8. **(2026-07-28)** Protocol 1.0 freeze moves behind Phase 2.1 (Codex) — spec decision 5. `FORMAT-CHANGELOG.md` still starts in 2.0; fixtures ship with the freeze.
9. **(2026-07-28)** MCP destructive (2.3) is evidence-gated: ≥20 human-confirmed `undo --confirm` in real use (audited from `log.jsonl` undo turns, all real repos; window is pre-2.3 by construction; indefinite deferral is an accepted outcome) before any 2.3 code; `allow_modified` is never honored in auto mode — spec decision 6.
10. **(2026-07-28)** MCP read (2.2) is gated on the CLI demand probe: ≥10 **audited** unprompted agent invocations across ≥3 non-agentrec-repo sessions, measured by transcript sweep (never an in-product counter — read verbs stay zero-write) — spec decision 7.
11. **(2026-07-28)** The import gate gains a fidelity report + VERIFY-LEDGER row (per-tier revertibility %, opaque-call share); no hard threshold until the first real measurement — spec decision 8.
12. **(2026-07-28)** Sutra parked; Phase 2 re-centered on accessibility (import, trailers, `--json`, distribution, setup, Codex). No Phase 2 work item touches the Sutra repo; the demand probe's treatment site is ledger-named at probe start, Sutra excluded — spec decision 9.
13. **(2026-07-28)** Founder waived the decision 7 demand probe: MCP read (2.2) is in Phase 2 scope unconditionally; the transcript sweep becomes post-ship evaluation. Decision 6's undo evidence gate on 2.3 stands untouched — spec decision 10. Sequencing unchanged: 2.2 still lands after P4/P5 (consumes `Page<T>` + the `--json` serializer).

## Infeasible / rejected (killed against real code or measured evidence)

- **Quoting 92.3% as the reconstructible rate.** T2 (git-tracked) is an upper bound: git holds committed states only, so mid-session intermediate edits were never in git. Honest figure is **40.4%** (T1 39.9% + T1.5 0.5%, measured 2026-07-29 by P1's real-corpus importer run — supersedes this entry's earlier 67.9%, whose T1.5 term counted same-session backup references rather than entries the first-hit-wins ladder actually resolves via T1.5; see `VERIFY-LEDGER.md`'s "Phase 2.0 P1" section) + "git recovers some unknown share of the remaining 44.8% T2-candidate" (measurement §2; P1 measured T2-candidate share at 44.8%, still unresolved).
- **Excluding sidechains by `isSidechain` field alone.** 583 of 1860 corpus files are now separate `subagents/agent-*.jsonl` files carrying no `cwd`. Must exclude by **path** as well (measurement §1 note).
- **Treating imported file lists as exhaustive.** Opaque:naming tool-call ratio measured 2.49:1 — 71% of tool calls can mutate files while naming none. `files_complete: false` is mandatory.
- **Reusing CLI stdout as the machine contract.** Unstable text, no typed errors, duplicate parsing (spec §Rejected).
- **Extracting the seams first.** They are consumer-enabling work behind an un-retired hard stop ("if real transcripts cannot reach 90%, **stop**"). Import gates first.
- **Claiming the gate is already passed at 99.1%.** That number is corpus feasibility (parse + `cwd`-mappability), not importer behavior. The spec's gate requires a built importer.

## Global constraints / invariants

- Append-only: `log.jsonl` / `signal.jsonl` are never rewritten by any phase here.
- Every write site goes through `agentrec-core/src/perms.rs` (0700 dirs / 0600 files).
- Prompts pass the existing scrub pipeline *inside* the persistence function — import included.
- No network code. No new crates without a stated reason.
- `cargo clippy -- -D warnings` + `cargo fmt --check` clean on debug **and** release; any test-only env seam is `#[cfg(debug_assertions)]`-gated (release `strings` must not contain it).
- Integration tests run `--test-threads=3` (FSEvents contention; see CLAUDE.md).

## Executor protocol

One phase per commit, per-phase branch off `feat/phase-2-0-substrate`. Use `/surgeon`
per chunked task. After each phase: `skeptical-reviewer` agent gates against that
phase's acceptance criteria **in an isolated worktree** (never the shared tree — a
prior reviewer `git reset` clobbered a commit). Merge order is phase order; P4
depends on P3's goldens, P5 on P4's typed values, P2 on P1's classifier. P1 and P3
are independent of each other and may run in parallel worktrees.

---

## Phase 1 — `import claude`: classification + before-ladder, dry-run only (the gate)

**Description:** Build the importer's read half and prove the Phase 2.0 hard stop on the
real corpus before anything consumes imported history. Emits a classification report and
writes nothing, so it is mergeable inert. This phase retires the scariest unknown in the
spec's table: "historical imports cannot reconstruct safe before/after state".

**Files:** `cli/src/importcmd.rs` (new), `cli/tests/import_claude.rs` (new),
`cli/tests/fixtures/import/claude/` (new fixtures), `cli/src/main.rs` (mech — verb + `mod`).

**Changes:**
- Streaming JSONL parse over `~/.claude/projects/**/*.jsonl` (never load a file whole).
- Session→root mapping by transcript `cwd`; sessions outside the target root counted and skipped.
- Sidechain exclusion by **path** (`subagents/`) **and** field (`isSidechain: true`).
- File-entry extraction from `Edit`/`Write` `toolUseResult`; `Bash`/`Task` results name no files (recorded as opaque, never inferred).
- Before-bytes ladder, per entry, first hit wins: **T1** `toolUseResult.originalFile` or `create` op → **T1.5** `~/.claude/file-history/<sessionId>/<hash>@<vN>` (resolved via `snapshot.trackedFileBackups[path].backupFileName`) → **T2** git blob at latest commit ≤ turn timestamp → **T3** `None`.
- **T2 is detected, not resolved, in this phase.** Resolution needs a per-session git object read path (open the repo at `cwd`, replay `git log --until <turn ts>`) — the measurement doc explicitly scoped that out. P1 classifies an entry T2 when its path is git-tracked in the session's repo; actually reading the blob lands in P2. The gate measures **session importability**, which does not depend on T2 resolving, so this keeps P1 at three substantive files without weakening the gate.
- `agentrec import claude --dry-run [--source <dir>] [--json]` prints per-tier counts, per-file skip counts, session totals, peak RSS.

**Acceptance criteria:**
- [ ] `agentrec import claude --dry-run` over the real `~/.claude/projects` corpus reports **≥90% of top-level sessions importable**; the printed denominator is top-level sessions, not all `.jsonl` files, and is **re-measured at run time** — the corpus is a rolling ≤30-day window (`cleanupPeriodDays` default 30; oldest transcript measured exactly 30.0 days on 2026-07-28), so the 2026-07-18 baseline (1266/1277) is a shape reference, never an assertable count.
- [ ] The dry-run report includes the **fidelity figures** (spec decision 8): % of extracted entries at T1, T1.5, T2-candidate, T3, and per-session opaque-call share; the figures land in a VERIFY-LEDGER row. No fidelity threshold is asserted — the founder sets one from this first measurement.
- [ ] The command creates or modifies **zero** bytes under `.agentrec/` — asserted by a recursive dir digest taken before and after the run.
- [ ] Every extracted file entry carries exactly one tier tag `T1|T1.5|T2|T3`. Fixture entries resolved at T1 and at T1.5 have `sha256(resolved before bytes)` byte-equal to the known-good pre-edit content; T2 entries are reported as **candidates** (path git-tracked in the session's repo), never as resolved bytes. No tier ever yields bytes it did not read from a source.
- [ ] A `subagents/agent-*.jsonl` file and an inline `isSidechain: true` line are both excluded, counted under `skipped_sidechain`, and produce zero top-level turns.
- [ ] A file whose every line is malformed reports one skip per line, exits 0, and does not abort the run or the other files.
- [ ] Peak RSS over the 515 MB corpus is printed and **< 500 MB** (measurement baseline: 37 MB).
- [ ] A transcript missing a required field (`cwd`, `toolUseResult`) fails the CI canary loudly with the field name, not silently importing less.

**Expected test outputs:** `cargo test -p agentrec --test import_claude` → 12 passed (new file, 0 pre-existing); `cargo test --workspace -- --test-threads=3` → **361 passed, 0 failed, 1 ignored** (baseline measured 2026-07-25: 349 passed / 0 failed / 1 ignored). Manual gate run: `agentrec import claude --dry-run` → `sessions: 1277 · importable: ≥1150 (≥90%) · T1 n · T1.5 n · T2-candidate n · T3 n · rss_peak_mb <500`.

---

## Phase 2 — `import claude`: persist imported turns + undo refusal

**Description:** Write classified turns into `log.jsonl` with the additive honesty markers,
idempotently and resumably, and make undo refuse provenance-only entries with a distinct
reason. Only after P1's gate passes.

**Files:** `cli/src/importcmd.rs` (edit), `agentrec-core/src/record.rs` (edit),
`cli/src/readcmds.rs` (edit — `build_plan` refusal at `readcmds.rs:756`).

**Changes:**
- `record.rs`: additive `imported: bool` and `files_complete: bool` on `TurnRecord` (both `#[serde(default)]`, `skip_serializing_if` to keep existing records byte-identical).
- `importcmd.rs`: append path (reusing the existing fsynced `append_log`), prompt blobs through the live scrub pipeline, idempotency key `(session_id, turn_index)`, resume from the last appended key.
- `readcmds.rs`: undo pre-flight distinguishes imported-non-reconstructable from `skipped`/`withheld`/over-cap.

**Acceptance criteria:**
- [ ] Imported turns land as `grade: "rich"`, `tool: "claude"`, `imported: true`, `files_complete: false`; a live-recorded turn in the same log serializes byte-identically to its pre-change form (no field added to non-imported records).
- [ ] Re-running the same import appends **0** new records (log length unchanged); a run `kill -9`'d mid-import then resumed yields a log byte-identical (after sort by id) to an uninterrupted run.
- [ ] `undo <imported-turn>` where any selected entry has `before: null` refuses **before any working-tree write** with a message naming imported history — textually distinct from the `withheld`, `skipped`, and modified-since messages, exit 1.
- [ ] Import-missing-`before` entries have `baseline_unknown == false` (asserted directly on the serialized record).
- [ ] **T2 resolution lands here** (deferred from P1): for a fixture session whose repo has a commit before the turn timestamp, the entry's `before` is the git blob at the latest commit ≤ that timestamp, byte-equal to the known-good content; a T2 *candidate* whose bytes were never committed (mid-session intermediate edit) falls through to T3 `before: null`, never to a nearby commit's bytes.
- [ ] A planted secret (`sk-...`, 64-hex, quoted multi-word credential) in a transcript prompt appears in neither the CAS prompt blob nor `prompt_excerpt`.
- [ ] `log` renders imported turns with a "partial file list (imported)" marker; a bare turn is still never relabeled.

**Expected test outputs:** `cargo test -p agentrec --test import_claude` → 22 passed (+10, incl. the T2 git-blob resolution moved here from P1); `cargo test --workspace -- --test-threads=3` → **371 passed, 0 failed, 1 ignored**. Manual: `agentrec import claude && agentrec import claude` → second run prints `appended: 0`.

---

## Phase 3 — Golden-output harness (pins today's CLI bytes before any refactor)

**Description:** The extraction in P4 claims byte-equivalence; nothing today can falsify that
claim (`cli/tests/integration.rs` is assert-based, no `insta`/snapshot dependency exists).
Build the baseline first, from a fixture repo covering every renderer branch.

**Files:** `cli/tests/golden.rs` (new), `cli/tests/fixtures/golden/` (new — fixture builder + captured goldens), `cli/Cargo.toml` (mech — dev-dep if a snapshot crate is chosen).

**Changes:**
- Deterministic fixture-repo builder (real `git init` — required, or gitignore-aware walkers silently apply no rules; see `daemon.rs:2295` and measurement §4).
- Capture stdout + stderr + exit code per command into checked-in golden files.
- Normalization: ULIDs and timestamps substituted by a documented stable mapping — **substituted, never deleted** (a deleted field can't regress).

**Acceptance criteria:**
- [ ] Fixture contains ≥1 each of: rich turn, bare turn, git turn, epoch gap, undo turn, `skipped` entry, `withheld` entry, imported turn, duplicate-id pair collapsible by `same_revert`.
- [ ] Goldens captured for `log`, `log --all`, `log --json`, `log --explain`, `status`, `diff <t>`, `blame <path>`, `blame <path>:<line>`, `show <t>`, `show <t> --prompt`, plus one failing invocation per command (unknown id) — stdout, stderr, and exit code each.
- [ ] Three consecutive runs over a freshly built fixture produce byte-identical goldens (determinism proof).
- [ ] `UPDATE_GOLDEN=1 cargo test --test golden` regenerates; without it, a one-character change to `fmt::SEP` makes ≥1 golden fail (RED-proven, then reverted).
- [ ] Harness runs with the daemon stopped and spawns none.

**Expected test outputs:** `cargo test -p agentrec --test golden` → 21 passed (new file); `cargo test --workspace -- --test-threads=3` → **392 passed, 0 failed, 1 ignored**. A deliberate `fmt::SEP` mutation → golden binary reports `≥1 failed` with a printed byte diff; restored after.

---

## Phase 4 — Extract `RepositoryView` (unify gaps, one lookup choke point, pure `health()`)

**Description:** Move read interpretation out of the renderer-coupled CLI into one seam in
`agentrec-core`. Relocating three copies of gap logic is not extraction — they collapse to
one primitive. Split the store-mutating side effect out of the read path. **`Page<T>`/cursor
types are defined here but dark-launched** — CLI adapters pass unpaginated queries; MCP 2.2
is the first caller (spec line 207). The cursor AC below tests a capability with no in-plan
consumer by design; that is not unwired scope creep.

**Files:** `agentrec-core/src/view.rs` (new), `cli/src/readcmds.rs` (edit),
`cli/src/cmds.rs` (edit), `agentrec-core/src/lib.rs` (mech — `pub mod view`).

**Changes:**
- `RepositoryView::{open,list,diff,blame,recall,health}` over typed `TurnQuery`/`DiffQuery`/`BlameQuery`/`RecallQuery`, returning `Page<T>` + cursor types.
- One gap primitive replaces `readcmds::has_recording_gap` (`readcmds.rs:361`), `readcmds::has_gap_after` (`readcmds.rs:892`), `cmds::count_gaps` (`cmds.rs:594`).
- `resolve_turn` (`readcmds.rs:110`) + `same_revert` (`readcmds.rs:166`) move into `view.rs`; no adapter reimplements them.
- `health()` is a pure read. `enforce_budget` (called today from the read path at `cmds.rs:271`) becomes an explicit call the human `status` adapter makes after `health()`.

**Interface contract (consumed by P5 and, later, MCP 2.2):**
```rust
RepositoryView::open(root: &Path) -> Result<RepositoryView, RepoError>
RepositoryView::list(q: TurnQuery)   -> Result<Page<TurnSummary>, RepoError>
RepositoryView::diff(q: DiffQuery)   -> Result<Page<FileDiff>, RepoError>
RepositoryView::blame(q: BlameQuery) -> Result<BlameResult, RepoError>
RepositoryView::health()             -> Result<RepositoryHealth, RepoError>  // pure
```

**Acceptance criteria:**
- [ ] `rg 'fn has_recording_gap|fn has_gap_after|fn count_gaps' cli/src` → **0 matches**; all three former call sites resolve gaps through the single `view.rs` primitive, and the gap-honesty golden is unchanged.
- [ ] `rg 'fn same_revert|fn resolve_turn' cli/src` → **0 matches**; the duplicate-id collapse golden from P3 still passes, and a distinct-turns-sharing-an-id fixture still errors ambiguous.
- [ ] **Every P3 golden byte-identical** after extraction (this is the byte-equivalence claim, now falsifiable).
- [ ] `RepositoryView::health()` performs zero writes: store byte count, `log.jsonl` length, and `state.json` mtime unchanged across a call on an **over-budget** store (RED without the split).
- [ ] `agentrec status` (human) **still evicts** on an over-budget store — today's user-visible behavior preserved; the existing `status_prints_over_budget_notice` test is not weakened.
- [ ] Cursor bound to query + observed ledger identity: a cursor replayed after the ledger grows returns the next page with no skip/duplicate; replayed after truncation returns `stale_cursor`, never silent data loss.
- [ ] Unknown record fields and unknown `type` values are tolerated (parse-tolerant, counted), not dropped from the ref-set.

**Expected test outputs:** `cargo test -p agentrec-core view::` → 18 passed (new); `cargo test -p agentrec --test golden` → 21 passed, **0 changed golden bytes**; `cargo test --workspace -- --test-threads=3` → **410 passed, 0 failed, 1 ignored**.

---

## Phase 5 — `--json` read contracts for `diff`, `blame`, `status`

**Description:** Adapters over P4's typed values — the stable machine-readable contracts MCP
2.2 will mirror exactly through the same serializer. No second interpretation anywhere.

**Files:** `cli/src/readcmds.rs` (edit), `cli/src/cmds.rs` (edit), `cli/src/main.rs` (mech — three `--json` flags), `README.md` (mech — flag rows).

**Changes:** serde serialization of `Page<FileDiff>`, `BlameResult`, `RepositoryHealth`; no hand-built JSON strings anywhere on these paths.

**Acceptance criteria:**
- [ ] `diff --json`, `blame --json`, `status --json` each emit `serde_json` of the exact typed value returned by `RepositoryView` — `rg 'format!\("\{\{' cli/src` finds no hand-built JSON on these paths; a field added to the struct appears in output with no adapter edit (proven by adding a temp field in-test).
- [ ] `status --json` writes nothing: store bytes, log length, and `state.json` mtime unchanged on an **over-budget** store, while bare `status` in the same fixture still evicts (paired assertion).
- [ ] Empty cases are data, not errors: `diff` on a fileless turn → `{"files":[]}` exit 0; `blame` on an uncovered path → JSON carrying `recording_gap: true` and no attributor field, never a guess.
- [ ] Bare turns serialize with no `tool`/`model` and are not labeled human or agent in any JSON field.
- [ ] All P3 goldens for the human forms unchanged (JSON is additive).
- [ ] Malformed/torn log line → the tolerant-parse counter appears in `status --json`; the command exits 0.

**Expected test outputs:** `cargo test -p agentrec --test integration json_contracts::` → 14 passed (new); `cargo test -p agentrec --test golden` → 21 passed, 0 changed golden bytes; `cargo test --workspace -- --test-threads=3` → **424 passed, 0 failed, 1 ignored**. Manual: `agentrec status --json | jq .` parses; `agentrec diff <fileless-turn> --json` → `{"files":[]}`.

---

## Phase 2.0 wave 2 — named, planned separately (NOT dropped)

Each is real Phase 2.0 scope, excluded from this plan for a stated reason:

| Item | Spec ref | Why not here |
|---|---|---|
| `UndoCoordinator` extraction (`preview`/`execute` **only** — no pending ledger, no `undo.lock` reservations, no confirm tokens) | §Deep module 2 | Its full contract exists to serve MCP destructive (2.3). Bound it explicitly or a fresh-context executor builds the whole 2.3 ledger early. |
| `FORMAT-CHANGELOG.md` start (freeze itself REMOVED from 2.0 — decision 8 above / spec decision 5, 2026-07-28) | §Phase 2.0 (N) | The freeze + conformance fixtures now land **after Phase 2.1's Codex exit** — a format is frozen only once a second emitter validates its shape. 2.0 only starts the changelog and documents this plan's additive fields (`imported`, `files_complete`, `emitter_turn`) as unfrozen. |
| `import aider` | §Phase 2.0 (L) | Fully reconstructible by construction (git parents) — carries no gate risk; sequence after the Claude importer proves the shared append path. |
| Git trailers + `git-agentrec` shim | §Phase 2.0 (M) | Zero code dependency on the seams. The only sanctioned PR↔turn substrate (decision 2). |
| npm/mise distribution wrappers (Z2) | §Phase 2.0 | Release-infra; spec says "may land any time inside Phase 2.0". |

Phases **2.1 (Codex L2) / 2.2 (MCP read) / 2.3 (MCP destructive) / 2.4 (setup+packaging)**
are out of scope until the 2.0 gate is retired by P1 and the seams exist. *(2.2 is
unconditional Phase 2 scope per spec decision 10, but still lands after this plan — it
consumes P4's `Page<T>` types and P5's serializer. 2.3 additionally needs spec decision 6's
evidence gate to open.)*

## Edge cases considered

- **P1:** empty transcript dir; session whose `cwd` no longer exists; `cwd` outside target root; symlinked `cwd` (canonicalize before compare); a `file-history` backup referenced but deleted by retention (~30-day limit — falls through to T2/T3, never errors); huge single line; non-UTF8 path in a transcript (skip + count, matching the engine's existing `non_utf8_path_skips` posture); duplicate session ids across dirs.
- **P2:** concurrent daemon appending while import appends (import takes `log.lock` per `cli/src/loglock.rs`); import over a log already containing the same session (idempotency); over-cap prompt blob (existing `prompt_put_failures` taxonomy applies).
- **P3:** fixture built in a non-git tempdir silently disables all ignore rules — the harness must `git init` (this exact trap inverted a probe result once, measurement §4).
- **P4:** over-budget store (the pure/impure split); zero-turn repo; log with only epoch records; cursor replayed across a `purge --log-duplicates` rewrite (→ `stale_cursor`).
- **P5:** `NO_COLOR`/non-TTY (JSON unaffected); `--json` combined with `--utc`/`--explain` (JSON is already absolute-time; `--explain` is human-only — reject the combination loudly rather than emitting a glossary into JSON).

## Recommendation taken (not a blocking question — P1 assumes it; say so to reverse)

**T1.5 (`~/.claude/file-history/`) is IN, and the spec's 3-tier ladder is amended to four.**
*(Update 2026-07-28: the spec amendment landed in the hardening round — the spec now carries
the four-tier ladder with measured shares; P1's commit no longer owes it.)*
The corpus audit found this undocumented source covering 25.3% of *entries with a
same-session backup reference*, at a 100% on-disk resolve rate (508/508 referenced backups),
holding verbatim pre-edit bytes. It is retention-limited (~30 days, 102 of 1277 sessions), so
it is treated as **opportunistic** — exactly the class the spec already accepts for T1 (which
itself hits only ~30%, varying 4–70%). Including an opportunistic source that reads real
bytes off disk cannot weaken the honesty model, whose rule is "never fabricate" — that
reasoning holds. **Falsified by P1's real-corpus run (2026-07-29):** the 25.3% figure counted
backup *references*, not entries the ladder's first-hit-wins logic would actually resolve via
T1.5 — T1 already covers nearly everything a same-session backup could have covered, so T1.5
only fires when T1 misses AND a backup exists for that exact path. P1 measured T1.5's true
marginal contribution at 0.5% (11 of 2151 entries), raising honest reconstructible coverage
39.9% → 40.4%, not 42.5% → 67.9% as predicted here. T1.5 was still correctly adopted into the
ladder — it resolves real bytes with no downside — it is simply near-empty in practice. Spec
§Phase 2.0 ladder carries the amendment; see `VERIFY-LEDGER.md`'s "Phase 2.0 P1" section and
`docs/verify/p1-gate-run.txt` for the measurement.

## Open questions (answer before implementation)

1. **Scope split confirmation.** Is "this plan = gate-critical chain, wave 2 = separate plan after P1's gate" the right cut, or should wave 2's independent items (`import aider`, trailers + shim, npm/mise) run in parallel worktrees *now* since they have no code dependency on the gate?
2. **`docs/superpowers/plans/2026-07-12-agentrec-memory.md` shows 12 unchecked tasks**, but CLAUDE.md records memory v1 as merged to `main` (PR #2, `7a83628`) with the dogfood clock started 2026-07-18. Are those checkboxes stale bookkeeping (close the plan), or genuinely open work that belongs in Phase 2 scope?
3. **Store reclaim decision, adjacent to P1's corpus work.** The retained archives `.agentrec/objects.archived.1784328469` (2.6 GiB) + `.1784934498` (15 MiB) need only an `rm`, and 767.7 MiB of referenced `.remember`/`.code-review-graph` churn has no precise tool (only date-scoped `purge --snapshots-before`, which hard-deletes). Does this plan carry a phase for it, or is it a separate founder-called cleanup?

## Final acceptance — plan exit (added 2026-07-28, founder-directed; the "done" bar for the whole plan)

Per-phase ACs above gate each commit; **this section is the bar for calling the plan itself
complete.** Verified by the orchestrator independently of every implementer, then a binding
skeptical-reviewer round in an isolated worktree. No item may be weakened to pass (ratchet).

- [ ] **The Phase 2.0 hard gate is retired with evidence:** `agentrec import claude --dry-run`
      over the real corpus (denominator re-measured at run time — rolling ≤30-day window)
      reports ≥90% of top-level sessions importable, **and** the fidelity figures (per-tier
      T1/T1.5/T2-candidate/T3 %, per-session opaque-call share) are recorded in a
      VERIFY-LEDGER row (spec decision 8). A parse-only pass without the fidelity row does
      **not** retire the gate.
- [ ] **Byte-equivalence holds at exit, not just at P4:** every P3 golden is byte-identical at
      the plan's final commit (stdout, stderr, exit code).
- [ ] **Suite green at the re-based ladder** (absolute counts in this plan are stale — see the
      warning in the header): `cargo test --workspace -- --test-threads=3` → 0 failed on the
      cut-point branch; clippy `-D warnings` + fmt clean on debug **and** release; release
      `strings` carries no test seam.
- [ ] **The pure-read split is proven both ways:** `status --json` / `health()` perform zero
      writes on an over-budget store while bare `status` in the same fixture still evicts
      (paired assertion, P4/P5 ACs).
- [ ] **Import honesty is enforced at the undo boundary:** `undo` on a provenance-only
      imported turn refuses before any working-tree write, with a message textually distinct
      from `withheld` / `skipped` / modified-since (P2 AC).
- [ ] **`--dry-run` wrote zero bytes under `.agentrec/`** — P1's recursive dir-digest AC
      re-verified at plan exit against the real corpus run, not only the fixture run (the
      import path's sole destructive-safety check; the one most likely to be satisfied
      loosely).
- [ ] **Gap logic was unified, not relocated:** the three former implementations
      (`has_recording_gap` / `has_gap_after` / `count_gaps`) resolve through ONE `view.rs`
      primitive — `rg` finds zero copies in `cli/src` (P4 AC), asserted at plan exit because
      byte-identical goldens pass either way and the spec names relocation as the
      extraction's failure mode.
- [ ] **Scope honesty:** no protocol-freeze artifacts (decision 8 / spec D5 — changelog only),
      no Sutra-repo touches (decision 12 / spec D9), no MCP code in this plan (2.2 is
      unconditional Phase 2 scope per spec D10 but sequenced **after** P4/P5 as its own
      round), no founder decision re-litigated.
- [ ] **Closing bookkeeping (house rule):** CLAUDE.md Status entry + VERIFY-LEDGER updated in
      the closing commit; open questions 1–3 above either answered or explicitly carried
      forward — never silently dropped.
