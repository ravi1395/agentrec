# agentrec Phase 2 — agents use the record

Date: 2026-07-18
Status: finalized (founder-confirmed decisions below); **hardened 2026-07-28** after an
adversarial value/threat analysis — founder decisions 5–8 added, corpus-decay honesty
measured in, before-ladder amended to four tiers, MCP phases evidence-gated.
Owner: Ravi
Supersedes: the Phase 2 half of `2026-07-12-agentrec-phase-2-3-design.md`. That
document's Phase 3 content (PR report Action, signing, Sutra rebase) remains the
current draft for Phase 3 and is not re-litigated here.
Companions: `PROTOCOL.md` (normative wire format), `IMPLEMENTATION.md` (AC register,
sections 4–5), `INTEGRATIONS.md`, `ROADMAP.md` (Phase 1 + Phase 2 gates)

## Goal

Phase 2 turns agentrec from a recorder humans query into infrastructure agents use.
It folds the ROADMAP Phase 1 truth substrate (import, trailers, protocol freeze,
machine-readable read contracts) in as Phase 2.0, because every Phase 2 consumer
depends on it, then ships two rich emitters, one stdio MCP server, and one
self-healing loop.

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
- **Conditional on the demand probe (decision 7):** any stdio-MCP host can query
  log/diff/blame/recall/status.
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
   embed a second active recorder. (Phase 3 executes this; recorded here because
   it constrains Phase 2 seam design — `RepositoryView`/`UndoCoordinator` are the
   interfaces Sutra will consume.)
4. **Phase 2.0 carries the full ROADMAP Phase 1 scope** — `import claude`,
   `import aider`, git trailers, `git-agentrec` shim, protocol freeze + conformance
   fixtures, npm/mise distribution wrappers — plus the seam extraction and JSON
   read contracts the 2026-07-12 entry gate demanded.

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
7. **MCP read (2.2) is gated on a CLI demand probe.** Agents can already run
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
   the recipe is installed in the founder's active *non-agentrec* development
   repos — at decision time `~/Projects/sutra` at minimum, plus any other repo
   that has an initialized `.agentrec/` store and a running daemon; a probe repo
   without an initialized store is invalid (read verbs must return real data,
   not init errors). The recipe-install date is recorded in a VERIFY-LEDGER row
   and starts the sweep window. Probe passes at **≥10 audited invocations across
   ≥3 distinct sessions**; the window is bounded by transcript retention
   (~30 days rolling), so the sweep must run at least monthly or hits are
   silently lost. If the probe never fires *with the treatment verifiably
   installed*, 2.2 is never built — that outcome is success, not failure: dead
   weight avoided for the cost of a doc paragraph and a grep.
8. **The import gate gains a fidelity report + ledger row (no threshold yet).** The
   ≥90% parse gate says nothing about how much imported history is actually usable.
   The dry-run must additionally report per-tier revertibility (% of extracted
   entries resolving at T1/T1.5, T2 candidates separately) and the opaque-call share
   per session; the figures land in a VERIFY-LEDGER row. A hard fidelity threshold is
   deliberately NOT set before the first real measurement — inventing one without
   baseline data would be theater.

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
- No PR report Action, no signing, no Sutra migration — Phase 3.
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
RepositoryView::list(query: TurnQuery) -> Result<Page<TurnSummary>, RepoError>
RepositoryView::diff(query: DiffQuery) -> Result<Page<FileDiff>, RepoError>
RepositoryView::blame(query: BlameQuery) -> Result<BlameResult, RepoError>
RepositoryView::recall(query: RecallQuery) -> Result<Page<MemoryHit>, RepoError>
RepositoryView::health() -> Result<RepositoryHealth, RepoError>
```

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
- MCP §8 `agentrec_status` tool-table row: **deferred with 2.2** (decision 7) —
  no normative wire text for a tool whose phase may never build. Documented in
  `FORMAT-CHANGELOG.md` as reserved; the §8 row lands in 2.2's first commit, if
  the demand probe opens it. (Already shipped: `agentrec_recall`.)

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
   correlation). Opportunistic, never assumed. Measured share: 42.5% of file
   entries (incl. create ops).
2. **T1.5** — `~/.claude/file-history/<sessionId>/<hash>@<vN>`, resolved via
   `snapshot.trackedFileBackups[path].backupFileName`: verbatim pre-edit bytes.
   Measured 25.3% of entries (518 of 2044); separately, 508/508 backups
   *referenced by the audit sample* resolved on disk — a resolve-rate check
   with its own denominator, not the tier count. Retention-limited (~30 days)
   so coverage degrades with age — opportunistic, same class as T1.
3. **T2** — git history blob (commit-time reconstruction) when the repo's git log
   covers the file at the turn's timestamp. **Upper bound by construction**: git
   holds committed states only, so a mid-session intermediate edit was never in
   git — a T2 candidate whose exact bytes were never committed falls through to
   T3, never to a nearby commit's bytes. Measured candidate share: 24.5%.
4. **T3** — none of the above → `before: null` + provenance-only; refused by undo
   with an explicit imported-history reason. Import never fabricates a revertible
   snapshot. Measured share: 7.7%.

Honest reconstructible figure: **67.9% without git** (T1 + T1.5, as measured on
unrounded entry counts; the rounded tier shares above sum to 67.8 — quote 67.9,
the direct measurement), plus an unknown resolved share of the 24.5% T2
candidates. Do not quote 92.3%.

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

**Entry precondition — CLI demand probe (decision 7, 2026-07-28; full definition
there).** No MCP server code is written until agents demonstrably want the data:
a published CLAUDE.md recipe points agents at `agentrec blame|log|status`, and a
transcript-sweep audit (instrument and "unprompted" definition in decision 7 —
deliberately not an in-product counter) must show **≥10 audited unprompted agent
invocations across ≥3 distinct non-agentrec-repo sessions**. Rationale: MCP tool
schemas are paid for in every prompt of every session; the nearest shipped
agent-facing surface (memory injection) measured 389 injections of 3 facts —
evidence that "available to the agent" is not "used by the agent".

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

Read tool JSON mirrors the Phase 2.0 `--json` contracts exactly (P2) — same typed
values, one serializer. Tool descriptions state capability and data shape only;
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
| Agents never query the record unprompted | CLI demand probe (decision 7): ≥10 unprompted agent invocations across ≥3 sessions before any 2.2 code |
| Humans never use undo, so agent self-revert has no base rate | Undo evidence gate (decision 6): ≥20 human-confirmed `undo --confirm` in real use before any 2.3 code |
| Codex hook/trust behavior drifts by version | Live fixture on minimum + current versions; start/stop correlation and `/hooks` trust observed |
| MCP host approval differs across clients | Internal off/confirm/auto state machine passes without host UI; host annotations remain additive UX |
| Trailer association under- or over-counts | Fixture repos prove intersect-at-commit selection; rebase/squash documented as out of scope, never guessed |

## Verification strategy

- Unit: typed query/undo interfaces, total error/status enums, importer
  classification.
- Integration: real binary over temp repos; hook stdin/stdout; config
  merge/reverse; import idempotency/resume. **Gated on decision 7 (only if 2.2
  builds):** MCP initialize/list/call/shutdown. **Gated on decision 6 (only if
  2.3 builds):** pending approvals across restart; simultaneous undo conflict;
  token replay; approval race.
- Conformance: Protocol 1.0 golden records consumed by every internal reader; the
  published suite is the third-party contract (lands with the freeze, after the
  Codex exit — decision 5).
- Adversarial: malformed/torn/huge JSONL, cursor invalidation, prompt/ANSI/
  Markdown injection, path traversal, symlink escape, hostile transcript lines
  in importers.
- Performance: 10k turns; 500 MB transcript import <500 MB RSS. **Gated on
  decision 7:** MCP p95 read <100 ms warm.
- Live behavior: actual Claude/Codex hooks, real-transcript import corpus,
  npm/mise installs on clean machines. **Gated on decision 7:** MCP host session.

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
4. **[Gated on decision 7 — runs only if 2.2 builds]** Stop the daemon. From an
   MCP host, list, diff, blame, recall, and read status successfully.
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
  2.2's first commit, only if decision 7's probe opens the phase** (no normative
  text for conditional tools).
- `IMPLEMENTATION.md` §O: Codex capture is `UserPromptSubmit` + `Stop`, not Stop
  only.
- `IMPLEMENTATION.md` §5: "v2 done =" becomes **O + Q, plus P if decisions 6/7
  open it** (R deferred, demand-driven; decision 1). P is double-gated as of
  2026-07-28; if either gate never opens, v2 closes as O + Q with P recorded as
  evidence-blocked — a founder re-scope note at Phase 2 close, not a silent
  failure to finish.
- `ROADMAP.md` Phase 2 "Ships" list: annotate VS Code extension as deferred out
  of v2 by founder decision 2026-07-18.
- **`ROADMAP.md` cold-start line (currently "import makes `blame` work on last
  month's changes at install time. This is the cold-start killer"): rewrite to
  the ≤30-day-backfill framing — rides P1's import commit (decision 8's
  prohibition needs a delivery vehicle, and this is it).**
- **`ROADMAP.md` Phase 2 gate (~line 55, "MCP config appears in strangers'
  dotfiles… extension installs growing"): two of its clauses are unsatisfiable
  as written — the extension clause has been stale since decision 1 (2026-07-18)
  and the MCP-dotfiles clause is now conditional on decision 7. Rewrite the gate
  to condition the MCP clause on 2.2 building and drop or annotate the extension
  clause — rides P1's import commit alongside the cold-start rewrite.**
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
- **Building MCP read before demand evidence (rejected 2026-07-28):** schemas cost
  every prompt; the CLI-via-shell probe measures the same demand for free
  (decision 7). "Available to the agent" has already measured ≠ "used by the
  agent" on the memory surface.
- **Pitching import as full-history cold-start recovery (rejected 2026-07-28):**
  the source corpus is a rolling ≤30-day window (`cleanupPeriodDays`); the honest
  claim is "≤30-day backfill". The durable-archive framing is also embargoed —
  a value claim with no shipped mechanism (no scheduler, no `--since`) is the
  same class of overclaim.
- **VS Code TypeScript protocol reader (and the extension itself) in Phase 2:**
  deferred (decision 1); conformance fixtures are published so the community can
  build readers; revisit demand-driven.
