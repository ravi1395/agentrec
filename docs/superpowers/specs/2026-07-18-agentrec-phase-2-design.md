# agentrec Phase 2 — agents use the record

Date: 2026-07-18
Status: finalized (founder-confirmed decisions below)
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
  snapshot.
- Commits carry exact `Agent-Turn:` provenance; `git agentrec <verb>` works.
- Protocol 1.0 is frozen with a published conformance fixture suite.
- Claude Code and Codex produce L2 records in the same repo with correct bracketing
  under the supported one-active-agent-per-root model.
- Any stdio-MCP host can query log/diff/blame/recall/status and, with explicit
  consent, undo.
- A real agent uses agentrec to identify its own bad turn, safely reverts it, and
  retries.
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

### Protocol 1.0 freeze + conformance fixtures (N)

All schema changes since v0.2 folded in — including this phase's additive fields
(below) — `FORMAT-CHANGELOG.md` started, and a golden-file conformance fixture
suite published in-repo: every record/signal variant, both grades, all op types,
unknown-field tolerance cases. This suite is what third-party emitters and every
internal consumer test against. After freeze: additive changes only.

Additive protocol changes shipped with the freeze:

- Signal §4: optional `emitter_turn` (Codex idempotency, below).
- Turn record §5: turn-level `imported: true` marker for backfilled turns and
  `files_complete: false` (imported file lists are non-exhaustive — see import
  honesty model below); imported entries with unreconstructable `before` are
  never revertible. File-entry `baseline_unknown` is NOT reused for
  import-missing-before — it keeps its live-recording first-observation meaning.
- MCP §8: add `agentrec_status` to the tool table (already shipped: `agentrec_recall`).

### `import claude` (K-series ACs)

Backfill from `~/.claude/projects/*.jsonl`. Honesty model is the load-bearing
design rule, refined by the 2026-07-18 corpus audit (real transcripts, this
machine):

**Before-bytes come from a best-effort source ladder**, per file entry:

1. Transcript `toolUseResult.originalFile` — Claude Code embeds the full pre-edit
   file content on Edit/Write results, but **unreliably** (~30% of edit results
   in the audited corpus, varying 4–70% per session with no version correlation).
   Opportunistic, never assumed.
2. Git history blob (commit-time reconstruction) when the repo's git log covers
   the file at the turn's timestamp.
3. Neither → `before: null` + provenance-only; refused by undo with an explicit
   imported-history reason. Import never fabricates a revertible snapshot.

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

### Phase 2.0 gate (ROADMAP Phase 1 gate, unchanged)

Import works on ≥90% of real-world Claude Code transcript files thrown at it.
If real transcripts cannot reach this, **stop** — do not build Phase 2 consumers
on ambiguous history.

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
modified-since files excluded unless `allow_modified: true` AND (mode `auto` or
human approval); executed undos recorded as turns (P5). Undo carries destructive
MCP annotations and still obeys agentrec's stronger internal consent state.

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
| Historical imports cannot reconstruct safe before/after state | Import corpus classifies every item reconstructible or non-revertible; zero fabricated snapshots; ≥90% of real transcripts import |
| Codex hook/trust behavior drifts by version | Live fixture on minimum + current versions; start/stop correlation and `/hooks` trust observed |
| MCP host approval differs across clients | Internal off/confirm/auto state machine passes without host UI; host annotations remain additive UX |
| Trailer association under- or over-counts | Fixture repos prove intersect-at-commit selection; rebase/squash documented as out of scope, never guessed |

## Verification strategy

- Unit: typed query/undo interfaces, total error/status enums, importer
  classification.
- Integration: real binary over temp repos; MCP initialize/list/call/shutdown;
  hook stdin/stdout; config merge/reverse; pending approvals across restart;
  simultaneous undo conflict; import idempotency/resume.
- Conformance: Protocol 1.0 golden records consumed by every internal reader; the
  published suite is the third-party contract.
- Adversarial: malformed/torn/huge JSONL, cursor invalidation, prompt/ANSI/
  Markdown injection, path traversal, symlink escape, token replay, approval
  race, hostile transcript lines in importers.
- Performance: 10k turns; MCP p95 read <100 ms warm; 500 MB transcript import
  <500 MB RSS.
- Live behavior: actual Claude/Codex hooks, MCP host session, real-transcript
  import corpus, npm/mise installs on clean machines.

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
   successfully.
5. Break a test from an agent; run the self-healing story in confirm mode, then in
   auto mode. Observe preview, consent, exact file restoration, new undo turn,
   green retry, and rejection of a replayed token.
6. `npx agentrec@latest log` and a mise install on a machine with no Rust
   toolchain; checksum failure path produces a loud error.

## Companion updates required (same commits as the code they describe)

- `PROTOCOL.md` §4: optional signal `emitter_turn`; retain D6's
  one-open-bracket-per-root limitation explicitly.
- `PROTOCOL.md` §5: `imported: true` semantics; imported non-reconstructible
  entries never revertible.
- `PROTOCOL.md` §8: add `agentrec_status` to the normative tool table.
- `IMPLEMENTATION.md` §O: Codex capture is `UserPromptSubmit` + `Stop`, not Stop
  only.
- `IMPLEMENTATION.md` §5: "v2 done =" becomes O + P + Q (R deferred,
  demand-driven; decision 1).
- `ROADMAP.md` Phase 2 "Ships" list: annotate VS Code extension as deferred out
  of v2 by founder decision 2026-07-18.

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
- **VS Code TypeScript protocol reader (and the extension itself) in Phase 2:**
  deferred (decision 1); conformance fixtures are published so the community can
  build readers; revisit demand-driven.
