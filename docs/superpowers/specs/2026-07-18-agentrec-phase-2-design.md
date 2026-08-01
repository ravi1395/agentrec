# agentrec Phase 2 — the record becomes accessible (read everywhere; agent-driven undo only when evidence arrives)

Date: 2026-07-18
Status: finalized (founder-confirmed decisions below); **hardened 2026-07-28** after an
adversarial value/threat analysis — founder decisions 5–8 added, corpus-decay honesty
measured in, before-ladder amended to four tiers, MCP phases evidence-gated; **decision 9
added later the same evening** — Sutra parked, Phase 2 re-centered on accessibility.
Owner: Ravi
Supersedes: the Phase 2 half of `2026-07-12-agentrec-phase-2-3-design.md`. That
document's Phase 3 content (PR report Action, signing, Sutra rebase) remains the
current draft for Phase 3 and is not re-litigated here.
Companions: `PROTOCOL.md` (normative wire format), `IMPLEMENTATION.md` (AC register,
sections 4–5), `INTEGRATIONS.md`, `ROADMAP.md` (Phase 1 + Phase 2 gates)

## Goal

Phase 2 makes the record **accessible** (decision 9, in this repo's v0.4 sense:
adoption, reach, ease of getting at the data): backfill via import, git-native
provenance via trailers, machine-readable `--json` contracts, a second emitter
(Codex), and painless install/setup. It folds the ROADMAP Phase 1 truth substrate
(import, trailers, machine-readable read contracts; the protocol freeze moved
behind 2.1 — decision 5) in as Phase 2.0, because everything downstream depends
on it. The stdio MCP **read** server is in scope by founder override (decision
10) as a thin adapter over the same serializer; the **destructive** surface and
the self-healing loop remain behind decision 6's evidence gate — built when
humans demonstrate the verb matters, not because the phase is named after them.

The architecture is one semantic core with thin adapters. CLI and MCP must not
independently reinterpret turn selection, blame, diff, modified-since, or undo
safety.

## Outcome

At the end of Phase 2:

- `agentrec import` backfills Claude Code and Aider history honestly: every imported
  turn is either reconstructible or explicitly non-revertible, never a fabricated
  snapshot. The Claude backfill window is ≤30 days by source retention (see corpus
  decay, below) — the pitch is "≤30-day backfill", never "full history"; the
  durable-archive framing stays embargoed until a re-import mechanism exists.
- Commits carry exact `Agent-Turn:` provenance; `git agentrec <verb>` works.
- Protocol 1.0 is frozen with a published conformance fixture suite — **after** the
  Codex emitter validates the two-emitter shape (decision 5).
- Claude Code and Codex produce L2 records in the same repo with correct bracketing
  under the supported one-active-agent-per-root model.
- Any stdio-MCP host can query log/diff/blame/recall/status (demand probe waived
  by decision 10; post-ship usage still measured by the decision 7 sweep).
- **Conditional on the undo-evidence gate (decision 6):** with explicit consent, an
  agent identifies its own bad turn, safely reverts it, and retries.
- `npx agentrec` and the mise/asdf plugin install the release binary.

## Founder decisions — this round (2026-07-18); executors may not re-litigate

1. **VS Code is deferred out of Phase 2.** IMPLEMENTATION section R leaves the
   "v2 done" definition; the extension returns demand-driven (ROADMAP principle 3).
   The MCP server remains designed so a future extension can consume it
   (long-lived process per root), but no extension code ships in Phase 2.
2. **PR↔turn association is exact-only.** `Agent-Turn:` trailers / exported
   association, never a timestamp or file-overlap heuristic. Phase 3 report work
   builds only on this; Phase 2.0 must therefore ship the trailer substrate.
3. **Sutra ownership: external agentrec recorder + Sutra sidecar.** Sutra does not
   embed a second active recorder. *(Superseded in part by decision 9, 2026-07-28:
   Sutra is parked and this decision no longer binds Phase 2 seam design — the
   ownership model itself stands for whenever the integration returns, but
   `RepositoryView`/`UndoCoordinator` are justified in Phase 2 by their CLI/MCP
   consumers alone.)*
4. **Phase 2.0 carries the full ROADMAP Phase 1 scope** — `import claude`,
   `import aider`, git trailers, `git-agentrec` shim, protocol freeze + conformance
   fixtures, npm/mise distribution wrappers — plus the seam extraction and JSON
   read contracts the 2026-07-12 entry gate demanded. *(Amended by decision 5,
   2026-07-28: the protocol freeze + conformance fixtures moved behind Phase 2.1
   — 2.0 only starts `FORMAT-CHANGELOG.md`. The rest of this scope stands.)*

## Founder decisions — hardening round (2026-07-28); executors may not re-litigate

Grounded in an adversarial analysis of this spec against live data (this repo's own
dogfood store: 2056 turns / 17 days, 0 human-confirmed undos, 9% real-source file
entries; corpus measurements below).

5. **Protocol 1.0 freeze moves behind Phase 2.1 (Codex).** Freezing after import
   fields land but before a second emitter exists would make the format additive-only
   forever on a shape validated against one emitter, with zero external consumers
   today. `FORMAT-CHANGELOG.md` still starts in Phase 2.0 (additive fields documented
   as they land, unfrozen); conformance fixtures ship together with the freeze, after
   the Codex exit (O5) proves the two-emitter shape.
6. **MCP destructive (2.3) is evidence-gated and auto mode never overrides
   modified-since.** Phase 2.3 work does not start until **≥20 human-confirmed
   `undo --confirm` executions in real use** (count at decision time: 0 in 17 days
   of dogfood). **Counter and scope:** the count is auditable from `log.jsonl`
   itself — every executed undo appends a `tool: "agentrec"` turn — summed across
   any real repo of any user (author's repos included, agentrec's own dev repo
   included; an undo of a real mistake is real use regardless of repo), plus any
   externally reported use. **The count window is pre-2.3 by construction**: the
   gate blocks 2.3 from starting, so every qualifying undo turn necessarily
   predates MCP undo existing and cannot be self-satisfied by it; when 2.3 does
   ship, the undo turn gains an additive origin discriminator (`cli` vs `mcp`) in
   the same commit so the distinction stays auditable afterward. **Reachability,
   stated honestly:** with zero external users today, ≥20 may never arrive —
   indefinite deferral is the accepted outcome of this gate, not a failure of it.
   Independently: `allow_modified: true` is honored only with explicit
   human approval (confirm mode); in auto mode, modified-since files are excluded
   unconditionally — the rail exists to protect human edits, and an agent frustrated
   by refusals must never be able to lower it autonomously.
7. **MCP read (2.2) is gated on a CLI demand probe.** *(Waived as a gate by
   decision 10 later the same evening — retained below as the post-ship
   evaluation method; the instrument and "unprompted" definition still apply to
   measuring real usage after 2.2 ships.)* Agents can already run
   `agentrec blame|log|status` via shell. Before any MCP server is built, publish a
   CLAUDE.md recipe pointing agents at the read verbs, then measure whether they
   reach for them. **Instrument: the Claude Code transcripts themselves, not the
   product** — `~/.claude/projects/**/*.jsonl` already records every Bash
   invocation, so a documented sweep script greps top-level transcripts for
   read-verb calls. Zero product code, zero new writers, and no conflict with the
   read-only guarantees on `status --json`/`health()` (an in-product counter was
   considered and rejected: it would add an unsynchronized `state.json` writer and
   contradict the pinned zero-write behavior). **"Unprompted" is defined and
   audited, not inferred:** an invocation counts only if (a) the session's `cwd`
   is not an agentrec development repo — dogfooding the recorder's own rounds is
   orchestration, not demand — and (b) the founder reads the counted transcript
   and confirms the preceding user message did not request agentrec use. The
   recipe being present does not disqualify a hit; the recipe *is* the treatment
   under test (capability discovery), exactly what MCP schemas would provide at
   ~1000× the context cost. Sessions are transcript files (real session identity).
   **Treatment site (without this, clause (a) disqualifies every natural hit):**
   the recipe is installed in ≥1 of the founder's active *non-agentrec*
   development repos, each with an initialized `.agentrec/` store and a running
   daemon; a probe repo without an initialized store is invalid (read verbs must
   return real data, not init errors). **The probe cannot start until a
   VERIFY-LEDGER row names the chosen repo(s) and records the recipe-install
   date** — that row is what bounds the sweep window and proves the treatment
   was installed, so "never fires" can only ever mean measured indifference,
   never an uninstalled treatment. (Sutra was originally named the minimum site;
   decision 9 parks all Sutra changes, so the site choice moves to the ledger
   row at probe start, Sutra excluded while parked.) Probe passes at **≥10 audited invocations across
   ≥3 distinct sessions**; the window is bounded by transcript retention
   (~30 days rolling), so the sweep must run at least monthly or hits are
   silently lost. If the probe never fires *with the treatment verifiably
   installed*, 2.2 is never built — that outcome is success, not failure: dead
   weight avoided for the cost of a doc paragraph and a grep. *(This
   never-built consequence is the part decision 10 waives — 2.2 builds
   regardless; the instrument, thresholds, and sweep cadence carry over
   unchanged into the post-ship evaluation row.)*
8. **The import gate gains a fidelity report + ledger row (no threshold yet).** The
   ≥90% parse gate says nothing about how much imported history is actually usable.
   The dry-run must additionally report per-tier revertibility (% of extracted
   entries resolving at T1/T1.5, T2 candidates separately) and the opaque-call share
   per session; the figures land in a VERIFY-LEDGER row. A hard fidelity threshold is
   deliberately NOT set before the first real measurement — inventing one without
   baseline data would be theater.
9. **(2026-07-28, later the same evening) Sutra is parked; Phase 2 is re-centered
   on accessibility.** All Sutra integration work — and every Phase 2 constraint
   whose only justification was Sutra — is deferred to a later phase, on the
   founder's direction. Concretely: decision 3 (2026-07-18) no longer binds Phase 2
   seam design (the seams stand on their own feet: CLI `--json` contracts and the
   gated MCP surface are their consumers); the demand probe's treatment site is no
   longer Sutra (see decision 7's amended site rule); and no Phase 2 work item may
   touch the Sutra repo. Phase 2's center of gravity is **accessibility** in this
   repo's established sense (the v0.4 D39–D44 axis: adoption, reach, ease of
   getting at the record): `import claude`/`aider`, git trailers + shim,
   `--json` read contracts, distribution (npm/mise), setup/packaging (2.4), and
   Codex capture (a second tool reaching the record is reach). The
   evidence-gated MCP surfaces (decisions 6–7) were already conditional; this
   decision does not reopen them, it just removes Sutra as a reason to build
   anything early. *(Decision 10, minutes later, waives decision 7's probe for
   the read surface — see below; decision 6's undo gate stands untouched.)*
10. **(2026-07-28, after decision 9) Founder waives the decision 7 demand probe:
    MCP read (2.2) joins Phase 2 scope unconditionally.** Explicit override,
    recorded as such: the skeptical case (schemas cost every prompt; the shipped
    agent-facing surface measured 389 injections of 3 facts; no observed
    unprompted agent query) was presented and the founder chose to build anyway.
    The probe machinery above stays in the text as the *evaluation* method — a
    post-ship transcript sweep still measures whether agents actually call the
    tools, it just no longer gates the build. Sequencing is unchanged by
    technical dependency: 2.2 is a thin adapter over the Phase 2.0 `--json`
    serializer, so seams + `--json` (P4/P5) land first regardless. Decision 6
    (2.3 destructive, ≥20 human-confirmed undos, allow_modified never in auto)
    is NOT waived — the read/destructive asymmetry is deliberate: a read tool's
    failure mode is wasted context; a destructive tool's failure mode lands on
    the never-destroy-user-work promise.

## Inherited decisions — confirmed earlier; executors may not re-litigate

1. `PROTOCOL.md` outranks this document on wire semantics.
2. MCP transport is stdio through Phase 3 (D24).
3. MCP destructive modes remain `off | confirm | auto`, default `off`;
   modified-since is the destructive rail; every executed undo is a new turn
   (D15/D23/D30).
4. Codex joins Claude Code as the second first-party L2 emitter (start+stop, not
   stop-only).
5. L3 writers use one append-only sidecar per writer, merged at read time (D21).
6. All Phase 2 code is Apache-2.0 (D17/D22).
7. The Claude plugin is packaging over the same setup behavior as `agentrec init`,
   not a second implementation (D44).
8. Bare turns remain unattributed windows everywhere. Renderers never relabel them
   as human or agent activity.
9. Parallel agents use distinct worktree roots (D6). No correct-attribution claim
   for concurrent agents in one root.

## Non-goals

- No VS Code extension (decision 1), no JetBrains/Neovim/Cursor/Windsurf adapter.
- No HTTP/SSE MCP transport.
- No hosted service, accounts, telemetry, policy enforcement, or team dashboard.
- No PR report Action, no signing, no Sutra migration — Phase 3. **Strengthened
  by decision 9 (2026-07-28): no Phase 2 work item touches the Sutra repo at
  all, and no Phase 2 constraint is justified by Sutra.**
- No prompt text in any generic summary, tool output, or report by default.
- No second capture engine anywhere.

## Architecture

```text
EMITTER ADAPTERS              SEMANTIC CORE                 SURFACE ADAPTERS

Claude hooks ───────┐                                    ┌─ CLI renderers
Codex hooks ────────┼─> signal inbox -> recorder         ├─ stdio MCP
import claude/aider ┘            │                       └─ (Phase 3: PR report,
                                 v                           Sutra, VS Code)
                      .agentrec log + CAS + memory
                                 │
                                 v
                    RepositoryView + UndoCoordinator
```

`RepositoryView`, `UndoCoordinator` live in `agentrec-core`; adapters do not import
the CLI crate. `IntegrationPlanner` lives in the CLI crate because it owns
tool-specific config and service installation. `ReportBuilder` and `LedgerSigner`
are Phase 3 modules; Phase 2 must not pre-build them, only avoid foreclosing them
(raw-line append stays a single seam).

### Deep module 1: `RepositoryView`

Seam: read-only interpretation of a repository record. It owns log loading, L3
sidecar merge, merged-turn suppression, tolerant parsing, blob integrity, diff,
blame, gap honesty, memory recall, pagination, and report-safe sanitization.

```rust
RepositoryView::open(root: &Path) -> Result<RepositoryView, RepoError>
RepositoryView::list(query: TurnQuery) -> Result<Page<TurnSummary>, CursorError>
RepositoryView::diff(query: DiffQuery) -> Result<DiffResult, DiffError>
RepositoryView::blame(query: BlameQuery) -> Result<BlameResult, BlameError>
RepositoryView::recall(query: RecallQuery) -> Result<RecallPage, RecallError>
RepositoryView::health(budget: u64) -> Result<RepositoryHealth, RepoError>
```

> **Amended 2026-07-30 (P4b, decisions 8–10; signature drift resolves toward the code — the
> parent spec is amended, not the code). Each amendment below is a ratchet-up: it adds
> precision or information the original signature could not carry, never removes a
> guarantee.**
>
> - `health` takes an explicit `budget: u64`. The view deliberately holds no config; the
>   budget comes from the caller (config), and re-coupling the view to config would also
>   disturb P5's `status --json` AC surface. **Ratchet-up:** the pure/impure split this makes
>   possible is strictly more information than a config-reading `health()` could expose.
> - `list` returns `Result<.., CursorError>`, not the blanket `RepoError`. **Ratchet-up:**
>   strictly more precise — a cursor failure (e.g. `stale_cursor`) is no longer indistinguishable
>   from an I/O failure.
> - `diff` returns `Result<DiffResult, DiffError>`, not `Result<Page<FileDiff>, RepoError>`.
>   A bare `Page<FileDiff>` has no slot for the turn-level header
>   (`turn <id> · <tool> · N files`), so a fileless turn's empty page would make that header
>   unproducible — `DiffResult` carries `turn_id`/`tool`/`total_files` alongside the page.
>   `DiffError` separates turn lookup failure from cursor failure from I/O, which a single
>   `RepoError` cannot. **Ratchet-up:** adds the header carrier and the error-cause distinction;
>   loses nothing `Page<FileDiff>` had.
> - `blame` returns `Result<BlameResult, BlameError>`, not `Result<BlameResult, RepoError>`.
>   `RepoError` is a single `Io(String)`; reproducing prose like
>   `<path> has only N line(s)` through it would require crafting that prose inside
>   `agentrec-core`, which violates decision 9 (the human CLI keeps ownership of its prose) and
>   misfiles a user-input error as I/O. **Ratchet-up:** the per-cause error enum is strictly
>   more information than one `Io(String)` variant.
> - `recall` returns `Result<RecallPage, RecallError>`, not `Result<Page<MemoryHit>, RepoError>`.
>   A bare `Page<MemoryHit>` drops `capped` (F3 — results may be missing from what was
>   fetched), `store_corrupt` (F10), and `store_empty` (PD3), each of which drives real adapter
>   behavior today. **Ratchet-up:** `RecallPage` is a strict superset — the underlying `Page`
>   is still there (`RecallPage.page`), plus the three flags a bare `Page` could not carry.

Rules:

- Results are typed data. Human text and MCP JSON are adapters over the same values.
- `open` canonicalizes one root; no query accepts an arbitrary second root.
- Cursors bind to query + observed ledger identity. Rewrite/truncation returns
  `stale_cursor`; it never silently skips or duplicates pages.
- Unknown record fields/types are tolerated per Protocol 1.0 fixtures.
- Prompt contents are opt-in data, never included in generic summaries.

### Deep module 2: `UndoCoordinator`

Seam: every destructive decision, independent of CLI or MCP transport.

```rust
UndoCoordinator::preview(request: UndoRequest) -> Result<UndoPreview, UndoError>
UndoCoordinator::execute(preview: PreviewId, grant: UndoGrant)
    -> Result<UndoReceipt, UndoError>
UndoCoordinator::request_human(preview: PreviewId) -> Result<PendingUndo, UndoError>
UndoCoordinator::resolve(request: RequestId, decision: HumanDecision)
    -> Result<PendingResolution, UndoError>
UndoCoordinator::status(request: RequestId) -> Result<PendingStatus, UndoError>
```

`UndoRequest` contains the turn reference, optional path subset, and
`allow_modified`. A preview binds the target record, selected paths, current
hashes, refusals, config mode, and expiry. Execution takes an OS-backed exclusive
`.agentrec/undo.lock` before claiming a preview, then holds it across hash recheck,
all working-tree writes, undo-turn append/fsync, and terminal request-state append.
The existing advisory guard remains recorder filtering, not concurrency exclusion.
Drift means `preview_stale`, no writes. Skipped/withheld entries never enter the
executable set. Imported turns whose `before` was unreconstructable are refused
with an explicit imported-history reason (K2), never a generic error.

Pending human requests use an append-only, 0600 event ledger under `.agentrec/`;
request, approve, deny, expire, execute, and fail are state transitions. Restart
cannot convert an unapproved request into an approval. Auto-mode confirm tokens
are single-use, stored only as hashes, expire after 60 seconds, and bind the
complete preview. Creating a confirm-mode pending request or issuing an auto-mode
token takes `.agentrec/undo.lock`, rechecks the preview, and atomically reserves
its executable paths in that ledger. Any overlap with a live reservation is
rejected immediately with `undo_conflict`. Deny, expiry, failure, or successful
execution appends the terminal transition that releases the reservation.

### Deep module 3: `IntegrationPlanner`

Seam: setup intent before filesystem mutation.

```rust
IntegrationPlanner::inspect(root: &Path) -> Result<DetectedIntegrations, SetupError>
IntegrationPlanner::plan(request: SetupRequest) -> Result<SetupPlan, SetupError>
SetupPlan::apply() -> Result<SetupReceipt, SetupError>
SetupPlan::reverse() -> Result<SetupReceipt, SetupError>
```

The plan describes exact file merges, backups, trust actions, service changes, and
MCP registration. `init`, `uninstall`, `doctor`, Claude packaging, and Codex
packaging consume this contract. Planning validates every input before any write;
malformed config aborts byte-identical.

## Phase 2.0 — Truth substrate

### Seam extraction + JSON read contracts

Extract `RepositoryView` and `UndoCoordinator` from the current renderer-coupled
`readcmds.rs`/`cmds.rs` paths. Exit: CLI behavior byte-equivalent (golden-output
tests over the existing integration corpus); typed-interface tests cover current
ACs. Then add `--json` to `diff`, `blame`, and `status` as adapters over the typed
values — stable machine-readable contracts that MCP tools mirror exactly (P2).

Extraction constraints surfaced by the 2026-07-18 code audit:

- Gap/epoch interpretation exists in three independent implementations
  (`readcmds::has_recording_gap`/`has_gap_after`, `cmds::count_gaps`). Extraction
  MUST unify them into one primitive inside `RepositoryView` — relocating three
  copies is not extraction.
- `resolve_turn` + `same_revert` (duplicate-id collapse) is the single
  correctness-critical lookup choke point; it moves into `RepositoryView` and no
  adapter reimplements it.
- **Known behavior change, deliberate:** today's `status` report *mutates* the
  store (over-budget eviction runs inside the read path). Phase 2.0 splits a pure
  `health()` read from an explicit enforcement call; `agentrec status` keeps
  today's user-visible behavior by calling both, but MCP `agentrec_status` and
  `status --json` are strictly read-only. This is the one place byte-equivalence
  is not the whole story — the side effect moves, and a test pins that
  `agentrec_status` never evicts.
- `Page<T>`/cursor types are defined in Phase 2.0 (MCP consumes them in 2.2), but
  the CLI adapters may pass unpaginated queries.

### Protocol 1.0 freeze + conformance fixtures (N) — **moved behind Phase 2.1 (decision 5, 2026-07-28)**

The freeze itself no longer lands in Phase 2.0. What Phase 2.0 ships:
`FORMAT-CHANGELOG.md` started, and this phase's additive fields (below) documented
there as they land, explicitly unfrozen. The freeze + the golden-file conformance
fixture suite (every record/signal variant, both grades, all op types,
unknown-field tolerance cases) ship together **after the Codex exit (O5)** — a
format is frozen only once a second emitter has validated its shape. After freeze:
additive changes only. This suite is what third-party emitters and every internal
consumer test against.

Additive protocol changes shipped in Phase 2.0 (pre-freeze, changelog-documented):

- Signal §4: optional `emitter_turn` (Codex idempotency, below).
- Turn record §5: turn-level `imported: true` marker for backfilled turns and
  `files_complete: false` (imported file lists are non-exhaustive — see import
  honesty model below); imported entries with unreconstructable `before` are
  never revertible. File-entry `baseline_unknown` is NOT reused for
  import-missing-before — it keeps its live-recording first-observation meaning.
- MCP §8 `agentrec_status` tool-table row: lands in **2.2's first commit**
  (2.2 in scope per decision 10; documented in `FORMAT-CHANGELOG.md` as reserved
  until then — normative wire text rides the code it describes, not this spec).
  (Already shipped: `agentrec_recall`.)

### `import claude` (K-series ACs)

Backfill from `~/.claude/projects/*.jsonl`. Honesty model is the load-bearing
design rule, refined by the 2026-07-18 corpus audit (real transcripts, this
machine):

**Corpus decay — measured 2026-07-28, this machine.** Claude Code deletes
transcripts after `cleanupPeriodDays` (default 30, unset here): **1600 top-level
transcript files, oldest exactly 30.0 days** (2213 counting the 613
`subagents/agent-*.jsonl` sidechain files, which import excludes — quote the
top-level figure for anything about the importable corpus);
`~/.claude/file-history` shows the identical ceiling. Three consequences, all
load-bearing:

- Import is a **≤30-day backfill, ceiling not floor** — it decays daily and the
  full historical corpus PROBLEM.md gestures at does not exist on disk. No pitch,
  README line, or gate figure may imply otherwise (delivery vehicle: the
  ROADMAP/PROBLEM/README companion rows below).
- Because import is idempotent, repeated re-import *would* accumulate history
  beyond the source's 30-day window — but **no scheduling mechanism is specced or
  scoped in Phase 2** (no timer, no `--since`, no service integration). Until one
  ships, "durable archive" is a **potential, not a claim**: it may not appear in
  any README, pitch, or Outcome line. What may be said: "re-running `import`
  extends coverage; automating that is future work."
- The gate denominator is a rolling window: the 2026-07-18 baseline (1277
  sessions) is already stale. **Re-measure the corpus at import time**; never
  hard-code a session count into a gate assertion.

**Before-bytes come from a best-effort source ladder**, per file entry
(four tiers — T1.5 added 2026-07-24 by corpus audit, folded in here 2026-07-28):

1. **T1** — transcript `toolUseResult.originalFile`: Claude Code embeds the full
   pre-edit file content on Edit/Write results, but **unreliably** (~30% of edit
   results in the audited corpus, varying 4–70% per session with no version
   correlation). Opportunistic, never assumed. Measured share (P1's real-corpus
   importer run, corrected 2026-07-29 evening, 2159 file entries): **856 entries
   — 39.6%** (of which 595 — 27.6% — carry inline `originalFile` and yield real
   pre-edit bytes; 261 — 12.1% — are `create` ops, which correctly have no
   pre-edit bytes) — supersedes the 2026-07-24 audit's 42.5% prediction.
2. **T1.5** — `~/.claude/file-history/<sessionId>/<hash>@<vN>`, resolved via
   `snapshot.trackedFileBackups[path].backupFileName`: verbatim pre-edit bytes,
   admitted only when a **structural** check passes — no edit to the same path
   intervened between the snapshot that recorded the backup and the edit being
   classified. (File-history blobs are written at snapshot time, not per edit,
   so for a 2nd-or-later edit the naive blob is a pre-*snapshot*, not pre-edit,
   state.) The 2026-07-24 audit predicted 25.3% of entries, but that figure
   counted files with *any* same-session backup reference, not entries the
   ladder's first-hit-wins logic actually resolves via T1.5. **T1.5 was wrong
   twice, in opposite directions, before this figure — it was never "genuinely
   near-empty."** First it was *under-detected* to 0.5% (11 entries) by a bug
   comparing an absolute `filePath` against `trackedFileBackups` keys that are
   relative to the session `cwd` in most of the corpus. Fixing that path
   compare raised the count to 210 — which was then found to be *over-counted*:
   a ground-truth subsample put the fabrication rate at ~30% (115 of that
   sample provably stale), because the textual `oldString`-containment guard
   let stale blobs (pre-snapshot, not pre-edit) through. Gating on the
   structural check above rejects **170 entries corpus-wide** as stale;
   P1's corrected, twice-re-measured marginal contribution is **90 entries —
   4.2%** (1 structurally-inferred). Ground-truth
   check (entries that also carry an inline `originalFile`, so the true bytes
   are known): **101 correct / 1 fabricated — 1.0% residual**, down from ~30%
   fabricated before the structural fix. Retention-limited (~30 days) so
   coverage degrades with age — opportunistic, same class as T1.
3. **T2** — git history blob (commit-time reconstruction) when the repo's git log
   covers the file at the turn's timestamp. **Upper bound by construction**: git
   holds committed states only, so a mid-session intermediate edit was never in
   git — a T2 candidate whose exact bytes were never committed falls through to
   T3, never to a nearby commit's bytes. 2026-07-24 predicted candidate share
   24.5%; P1's real-corpus run measured T2-candidate share at **41.5%** (897 of
   2159 entries; bytes still not resolved — see "Do not quote 92.3%" below).
4. **T3** — none of the above → `before: null` + provenance-only; refused by undo
   with an explicit imported-history reason. Import never fabricates a revertible
   snapshot. 2026-07-24 predicted 7.7%; P1's real-corpus run measured **14.6%**
   (316 of 2159).

**Honest reconstructible figure — two numbers, read separately, never collapsed
to one** (measured 2026-07-29 evening by the built importer's real-corpus run —
task P1; see `VERIFY-LEDGER.md`'s "Phase 2.0 P1" section and
`docs/verify/p1-gate-run-t15fix2.txt`):
- **43.8%** (T1 856 + T1.5 90 = 946 of 2159) counting `create` ops as
  reconstructible, since a new file's correct `before` genuinely is "nothing".
- **31.7%** (595 + 90 = 685 of 2159) counting only entries that yield **actual
  pre-edit bytes**.

This figure has been wrong twice: the 2026-07-24 prediction (67.9%), then a
first real-corpus reading (40.4%) that undercounted T1.5 via the path-compare
bug and — had it not been caught — would next have overcounted it via stale
blobs. Neither superseded number should be quoted again.
Do not quote 92.3% — T2-candidate measured 41.5% (P1), still an unresolved
upper bound. **The same ban applies to the ~85% ceiling** (43.8 + 41.5
T2-candidate): it is an upper bound by the identical argument (git holds
committed states only, so a mid-session intermediate edit was never in git at
all), and P2 will not resolve every T2 candidate. Do not quote ~85% as a
recovery rate.

**File lists are structurally incomplete** — the deeper honesty problem. In the
audited corpus, Bash tool calls outnumber Edit+Write ~2:1, and Bash and subagent
(`Agent`) results name no touched files at all: a turn that ran `cargo fmt` or
delegated edits to a subagent mutated files the transcript never lists. Therefore
every imported turn carries additive `files_complete: false`: its listed files
are genuinely agent-touched, but absence of a file from an imported turn proves
nothing. Consumers MUST NOT treat imported turns as exhaustive coverage — in
particular, the display-level human-edited-since predicate may over-report
"possibly human" across imported history (safe direction: never fabricates agent
attribution), and renderers say "partial file list (imported)". Sidechain
(subagent) transcript lines are skipped, not imported as top-level turns.

- Session→repo mapping by transcript `cwd`; only sessions inside the target root.
- Idempotent (session id + turn index); interrupted import resumes cleanly.
- Malformed lines skipped with per-file counts; never aborts on one bad line.
- Streaming parse: 500 MB of transcripts under 500 MB RSS.
- Prompts pass the same scrub pipeline as live capture.
- Transcript format is explicitly unstable: importer fixtures isolate the risk;
  a CI canary fails loudly when required fields disappear.

### `import aider` (L-series ACs)

Aider auto-commits by message convention → `rich` turns, `tool: "aider"`,
before/after from git parents (fully reconstructible by construction). Non-aider
commits never imported; merge commits skipped with a notice; idempotent by SHA.

### Git trailers + `git-agentrec` (M-series ACs)

- Opt-in `agentrec init --git-trailers` installs a post-commit hook appending
  `Agent-Turn: <ids>` for turns whose files intersect the commit; idempotent,
  chains existing hooks, never overwrites.
- Commits with no intersecting turns get no trailer.
- History rewrites (rebase/squash) are out of scope: trailers reflect commit-time
  knowledge; documented. This trailer is the **only** sanctioned PR↔turn
  association substrate (decision 2) — Phase 3 consumes it exactly, never a
  heuristic.
- `git agentrec <verb>` == `agentrec <verb>` (thin exec shim on PATH), works from
  subdirectories.

### Distribution wrappers (Z2)

`npx agentrec@latest` postinstall fetches the checksum-verified platform binary;
version locked one-to-one to the release; publishing is a release-CI step, never
manual. mise/asdf plugin installs and pins versions. README install matrix
documents all five paths. Release-infra work with no code dependency on the other
2.0 items — may land any time inside Phase 2.0.

### Phase 2.0 gate (ROADMAP Phase 1 gate + fidelity report, decision 8)

Import works on ≥90% of real-world Claude Code transcript files thrown at it.
If real transcripts cannot reach this, **stop** — do not build Phase 2 consumers
on ambiguous history.

**Fidelity report (additive, decision 8):** the parse gate alone measures the
wrong thing — a session can import at 100% while every entry is provenance-only.
The dry-run therefore also reports: % of extracted entries resolving at T1, at
T1.5 (revertible-in-principle), T2 candidates (separately — candidates, not
resolved bytes), T3; and the opaque-call share per session (Bash/Task calls that
can mutate files while naming none — corpus baseline 2.49:1). Figures land in a
VERIFY-LEDGER row. No hard fidelity threshold before the first real measurement;
the founder sets one from the measured baseline, not from hope.

## Phase 2.1 — Codex L2 capture

Spike first, adapter second: live hook payload/trust/install behavior on a pinned
minimum Codex version before any adapter code.

- Use Codex `UserPromptSubmit` + `Stop`, not Stop-only. `UserPromptSubmit`
  supplies `turn_id`, `prompt`, `session_id`, `cwd`, model, and transcript path.
  `agentrec hook codex` scrubs before appending the start signal.
- Optional signal field `emitter_turn` carries Codex `turn_id`. Router and crash
  journal carry it end-to-end. `(tool, event, session, emitter_turn)` makes
  duplicate starts/stops idempotent, including across a daemon restart. A
  mismatched stop never closes the current bracket. This field does not create
  multi-bracket support.
- `Stop` closes the one open root bracket under D6 semantics; prompt/model/session
  fall back to the start signal; transcript parsing is fallback only. A second
  non-duplicate start while another agent owns the root closes/truncates the prior
  bracket honestly; never claims clean cross-attribution.
- Store redacted hook-payload fixtures; CI canaries fail loudly when required
  fields disappear.
- Subagent events do not create top-level turns in the first release; their
  changes stay inside the parent bracket. Revisit only with dogfood evidence.
- `import codex` from `~/.codex/sessions/**/rollout-*.jsonl` with the full
  K-series AC applied (O4).

Setup policy (current Codex behavior):

- Prefer repo-local `.codex/hooks.json`; repo hooks require project trust and
  per-hook review. `init` prints the required `/hooks` action — it cannot silently
  grant trust.
- `IntegrationPlanner::inspect` searches accessible user/project/plugin hook
  sources for the agentrec marker; reuses one active installation; refuses a
  visible duplicate. Managed sources may be opaque, so runtime `emitter_turn`
  dedup is the final safety layer.
- If exactly one project hook representation exists (`hooks.json` or inline
  `[hooks]`), merge there. If both define hooks, refuse untouched and explain
  that Codex loads both; never create a duplicate callback.
- Project `.codex/config.toml` owns the stdio MCP registration.
- `uninstall` removes only entries carrying agentrec's exact marker. `doctor`
  validates shape and recent signals; absence of trusted execution is a loud
  degraded result.

Official surface references: [Codex hooks](https://developers.openai.com/codex/hooks/),
[Codex MCP configuration](https://developers.openai.com/codex/mcp/),
[Codex configuration reference](https://developers.openai.com/codex/config-reference/).

Exit (O5): one live session each of Claude Code and Codex on the same repo yields
one `log.jsonl` with both tools attributed correctly and zero cross-attribution.

## Phase 2.2 — MCP read tools

**Entry precondition: none — the decision 7 demand probe was waived by founder
override (decision 10, 2026-07-28).** The skeptical record stands: MCP tool
schemas are paid for in every prompt of every session, and the nearest shipped
agent-facing surface (memory injection) measured 389 injections of 3 facts. The
probe's transcript-sweep instrument survives as the **post-ship evaluation**,
inheriting decision 7's discipline wholesale: same instrument, same "unprompted"
audit clauses, same ≥10-across-≥3-sessions reference figure, monthly sweep
cadence (30-day transcript retention loses hits), first sweep dated one month
after 2.2 ships. The VERIFY-LEDGER row records the observed count against that
reference — so the question "do agents actually use this?" gets answered by
measurement either way; it just no longer blocks the build. Technical sequencing unchanged:
2.2 consumes Phase 2.0's `Page<T>`/cursor types and `--json` serializer, so it
lands after P4/P5.

Command: `agentrec mcp`, stdio, rooted at its canonical launch cwd. Reads work
with the daemon stopped (P3). Config loads once at startup; changing destructive
mode requires restart and is reported in `agentrec_status`.

| Tool | Input | Bounded result |
|---|---|---|
| `agentrec_log` | filters, limit, cursor | ≤200 turn summaries + cursor |
| `agentrec_diff` | turn, optional paths, limit, cursor | ≤2,000 diff lines + file metadata |
| `agentrec_blame` | path, optional line/range | one line/range result; gap honesty included |
| `agentrec_recall` | query, k, cursor | fresh hash-verified memory hits only |
| `agentrec_status` | none | repo/daemon/config/health summary |

> **Amended 2026-07-30 (P4b, decision 12).** This row promised both "≤200 turn **summaries**"
> and, in the paragraph below, that read tools "mirror the Phase 2.0 `--json` contracts exactly
> … same typed values, one serializer." Under decision 5 those are irreconcilable for
> `agentrec_log` specifically: `log --json` serializes full `TurnRecord`s, while the row's own
> "summaries" promise is `TurnSummary` — a different, smaller typed value with its own
> serializer (`list()`'s). **`agentrec_log` mirrors `list()`'s summary contract**
> (`Page<TurnSummary>`), not `log --json`'s full-record contract. A full-record MCP tool or
> parameter is deferred, not designed here. This is consistent with the spec's own "prompt
> contents never in generic summaries" rule — `TurnSummary` already excludes prompt fields.
> The other four rows are unaffected: `agentrec_diff`/`agentrec_blame`/`agentrec_recall`/
> `agentrec_status` each mirror one `--json` contract with no second typed value competing for
> the name.

Read tool JSON mirrors the Phase 2.0 `--json` contracts exactly (P2) — same typed
values, one serializer, **except `agentrec_log`, which mirrors `list()`'s summary contract per
the amendment above.** Tool descriptions state capability and data shape only;
outputs contain data, not instructions to the model (P7). Read tools carry
read-only MCP annotations.

## Phase 2.3 — MCP destructive

**Entry precondition — undo evidence gate (decision 6, 2026-07-28; counter,
scope, and reachability defined there).** Phase 2.3 does not start until **≥20
human-confirmed `undo --confirm` executions in real use**, audited from
`log.jsonl` undo turns across all real repos (count at decision time: 0 across
17 days of dogfood, 193 commits in the window; the window is pre-2.3 by
construction, so the gate cannot be satisfied by MCP undos). Building agent
self-revert before the human verb has fired once would be new bug surface with
negative demonstrated demand; the state machine below stays specced so the
design is ready when the evidence arrives — and if the evidence never arrives,
2.3 is never built.

`agentrec_undo` joins `tools/list` only when `mcp_destructive != "off"` (P1).
Action contract:

- `preview`: bound preview, refusals, warnings, effective mode; no working-tree
  writes. In auto mode, successful preview atomically reserves executable paths
  and issues the token. In confirm mode it does not reserve until `request`.
- `request`: confirm mode only; appends a pending request for
  `agentrec approve|deny`; requests expire in 10 min (D23).
- `status`: pending/approved/denied/expired/executed — distinct statuses.
- `execute`: auto mode only; single-use 60 s token; wrong/reused/expired tokens
  error without side effects; preview-to-execution drift aborts with a
  fresh-preview instruction (P4).

In confirm mode, `agentrec approve <id>` atomically claims the request, rechecks,
and executes while holding `.agentrec/undo.lock`; `agentrec deny <id>` records
denial without writes. The agent only polls `status` and can never execute a
merely-approved request a second time. Approval drift records `preview_stale` and
requires a new request. Multiple previews/pending requests may coexist; only one
execution may hold the lock, and live pending/token reservations must be
path-disjoint — a second overlapping request is rejected at creation (P6).

In every mode: `skipped`/`withheld`/non-reconstructible-imported files refused;
executed undos recorded as turns (P5). **Modified-since is honored asymmetrically
(decision 6): in auto mode, modified-since files are excluded unconditionally —
`allow_modified: true` in an auto-mode request is refused, not ignored.** The
override exists only through explicit human approval (confirm mode). The rail
protects human edits; an agent frustrated by refusals must never be able to lower
it autonomously, and a token-bound auto grant is still the agent granting itself.
Undo carries destructive MCP annotations and still obeys agentrec's stronger
internal consent state.

### Self-healing acceptance story

Not scripted output. In a disposable real repo:

1. Agent makes a change that breaks a named test.
2. Agent calls blame/diff and identifies its own causal turn.
3. Agent previews undo; the user/config grants the permitted path.
4. Agentrec executes, records the undo as a turn, and the test returns to green.
5. Agent retries with a different implementation.

The trace, tool calls, working-tree hashes, and final log are retained as test
evidence. This E2E — runnable with the daemon stopped during reads — is the
Phase 2.3 exit.

## Phase 2.4 — Setup and packaging

- `init` merges `.mcp.json` without replacing unrelated servers; malformed input
  aborts untouched with the same backup policy as hook setup (Q2).
- Claude Code hardening (Q1): transcript-format canary in CI per supported
  version; rich-rate alerting in `status` and the MCP `status` field;
  managed-settings environments detected with a clear "hooks unavailable —
  running degraded" notice.
- CLAUDE.md self-healing recipe published and validated in ≥3 scripted scenarios
  (Q3).
- Claude plugin (D44/Q4): one install action configures hooks + MCP registration;
  binary installation always requires explicit consent; plugin uninstall removes
  only plugin-owned config and delegates archive/service removal to
  `agentrec uninstall`.
- Single source of truth (Q5): plugin and `agentrec init` generate configuration
  from the same `IntegrationPlanner` path, asserted by a byte-for-byte comparison
  test.

Exit: install/re-init/uninstall leaves unrelated config byte-preserved, on both
Claude and Codex config surfaces.

## Scariest unknowns and spike exits

| Unknown | Spike exit |
|---|---|
| Historical imports cannot reconstruct safe before/after state | Import corpus classifies every item reconstructible or non-revertible; zero fabricated snapshots; ≥90% of real transcripts import; fidelity report figures recorded (decision 8) |
| Imported history is parseable but useless (all provenance-only) | Fidelity report over the real corpus; founder sets the threshold from measured baseline, then the gate re-runs against it |
| Agents never query the record unprompted | Accepted risk by founder override (decision 10): 2.2 builds anyway; the decision 7 transcript sweep runs post-ship as evaluation and its figure lands in a VERIFY-LEDGER row |
| Humans never use undo, so agent self-revert has no base rate | Undo evidence gate (decision 6): ≥20 human-confirmed `undo --confirm` in real use before any 2.3 code |
| Codex hook/trust behavior drifts by version | Live fixture on minimum + current versions; start/stop correlation and `/hooks` trust observed |
| MCP host approval differs across clients | Internal off/confirm/auto state machine passes without host UI; host annotations remain additive UX |
| Trailer association under- or over-counts | Fixture repos prove intersect-at-commit selection; rebase/squash documented as out of scope, never guessed |

## Verification strategy

- Unit: typed query/undo interfaces, total error/status enums, importer
  classification.
- Integration: real binary over temp repos; hook stdin/stdout; config
  merge/reverse; import idempotency/resume; MCP initialize/list/call/shutdown
  (2.2 in scope per decision 10). **Gated on decision 6 (only if
  2.3 builds):** pending approvals across restart; simultaneous undo conflict;
  token replay; approval race.
- Conformance: Protocol 1.0 golden records consumed by every internal reader; the
  published suite is the third-party contract (lands with the freeze, after the
  Codex exit — decision 5).
- Adversarial: malformed/torn/huge JSONL, cursor invalidation, prompt/ANSI/
  Markdown injection, path traversal, symlink escape, hostile transcript lines
  in importers.
- Performance: 10k turns; 500 MB transcript import <500 MB RSS; MCP p95 read
  <100 ms warm.
- Live behavior: actual Claude/Codex hooks, real-transcript import corpus,
  npm/mise installs on clean machines, MCP host session.

## Manual E2E script

1. Initialize one disposable repo with Claude Code and Codex installed; review and
   trust hooks. Observe both start+stop signals and two rich, correctly attributed
   turns in one log.
2. Run `agentrec import` against a real Claude Code transcript history and an
   aider repo copy. Observe reconstructible vs provenance-only classification,
   idempotent re-run, and undo refusal on a provenance-only turn with the
   imported-history reason.
3. Commit with trailers enabled; observe `Agent-Turn:` in `git log` and
   `git agentrec blame` resolving it.
4. Stop the daemon. From an MCP host, list, diff, blame, recall, and read status
   successfully (2.2 in scope per decision 10).
5. **[Gated on decision 6 — runs only if 2.3 builds]** Break a test from an
   agent; run the self-healing story in confirm mode, then in auto mode (no
   modified-since files in the auto leg — auto never overrides that rail).
   Observe preview, consent, exact file restoration, new undo turn, green retry,
   and rejection of a replayed token.
6. `npx agentrec@latest log` and a mise install on a machine with no Rust
   toolchain; checksum failure path produces a loud error.

## Companion updates required (same commits as the code they describe)

- `PROTOCOL.md` §4: optional signal `emitter_turn`; retain D6's
  one-open-bracket-per-root limitation explicitly.
- `PROTOCOL.md` §5: `imported: true` semantics; imported non-reconstructible
  entries never revertible.
- `PROTOCOL.md` §8: add `agentrec_status` to the normative tool table — **rides
  2.2's first commit** (2.2 unconditional per decision 10).
- `IMPLEMENTATION.md` §O: Codex capture is `UserPromptSubmit` + `Stop`, not Stop
  only.
- `IMPLEMENTATION.md` §5: "v2 done =" becomes **O + Q + P-read (2.2, in scope
  per decision 10), plus P-destructive (2.3) if decision 6's evidence gate
  opens** (R deferred, demand-driven; decision 1). If the undo gate never opens,
  v2 closes without 2.3, recorded as evidence-blocked — a founder re-scope note
  at Phase 2 close, not a silent failure to finish.
- `ROADMAP.md` Phase 2 "Ships" list: annotate VS Code extension as deferred out
  of v2 by founder decision 2026-07-18.
- **`ROADMAP.md` cold-start line (currently "import makes `blame` work on last
  month's changes at install time. This is the cold-start killer"): rewrite to
  the ≤30-day-backfill framing — rides P1's import commit (decision 8's
  prohibition needs a delivery vehicle, and this is it).**
- **`ROADMAP.md` Phase 2 gate (~line 55, "MCP config appears in strangers'
  dotfiles… extension installs growing"): the extension clause has been
  unsatisfiable since decision 1 (2026-07-18) — drop or annotate it. The
  MCP-dotfiles clause stands as written: 2.2 ships unconditionally per decision
  10, so the clause is measurable again (it was briefly conditional under
  decision 7's probe, superseded). Rides P1's import commit alongside the
  cold-start rewrite.**
- **`PROBLEM.md` cold-start framing: same rewrite, same commit.**
- **`README.md` import section (when import ships): backfill window stated as
  ≤30 days rolling; no durable-archive claim until a re-import mechanism
  exists.**

## Rejected approaches

- **Reuse CLI stdout inside MCP:** unstable text, no typed errors, duplicates
  parsing. MCP mirrors typed `--json` contracts instead.
- **Copy query logic into each adapter:** turns protocol tolerance into divergent
  products. One `RepositoryView`.
- **Always expose MCP undo and reject at call time:** violates `off` mode's
  capability boundary.
- **Timestamp/file-overlap PR association:** fabricates precision; an impressive
  wrong number damages the trust product (decision 2).
- **Import turns as revertible when `before` is unreconstructable:** fabricated
  snapshots would make undo destructive. Provenance-only is the honest class.
- **Freezing Protocol 1.0 before a second emitter exists (rejected 2026-07-28):**
  additive-only forever on a one-emitter shape, for third-party consumers who do
  not exist yet, is pure downside; the freeze waits for the Codex exit (decision 5).
- **Honoring `allow_modified` in auto mode (rejected 2026-07-28):** the
  modified-since rail protects human edits; any path where the agent can grant
  itself the override converts refusal friction into data loss on the product's
  one non-negotiable promise (decision 6).
- **Building MCP read before demand evidence (rejected 2026-07-28, then
  overridden by founder decision 10 the same evening):** the skeptical argument
  — schemas cost every prompt; the CLI-via-shell probe measures the same demand
  for free; "available to the agent" measured ≠ "used by the agent" on the
  memory surface — was presented in full and the founder chose to build anyway.
  Kept here so the record shows the override was informed, not accidental; the
  probe survives as post-ship evaluation.
- **Pitching import as full-history cold-start recovery (rejected 2026-07-28):**
  the source corpus is a rolling ≤30-day window (`cleanupPeriodDays`); the honest
  claim is "≤30-day backfill". The durable-archive framing is also embargoed —
  a value claim with no shipped mechanism (no scheduler, no `--since`) is the
  same class of overclaim.
- **VS Code TypeScript protocol reader (and the extension itself) in Phase 2:**
  deferred (decision 1); conformance fixtures are published so the community can
  build readers; revisit demand-driven.
