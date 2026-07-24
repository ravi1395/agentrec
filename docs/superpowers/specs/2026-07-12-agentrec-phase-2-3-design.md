# agentrec Phase 2–3 — agents use the record; reviewers see it

Date: 2026-07-12
Status: Phase 2 content SUPERSEDED by `2026-07-18-agentrec-phase-2-design.md`
(founder decisions 2026-07-18: VS Code deferred out of Phase 2; exact-only PR
association; Sutra external-recorder ownership; entry-gate truth substrate folded
in as Phase 2.0). Phase 3 content (PR report Action, signing, Sutra rebase)
remains the current draft.
Owner: Ravi
Companions: `PROTOCOL.md` (normative wire format), `IMPLEMENTATION.md` (AC register),
`INTEGRATIONS.md` (integration thesis), `ROADMAP.md` (release gates)

## Goal

Phase 2 turns agentrec from a recorder humans query into infrastructure agents use:
two rich emitters, one stdio MCP server, one self-healing loop, and one editor surface.
Phase 3 carries the same record into pull-request review, adds experimental local-log
integrity, and makes Sutra a control surface over agentrec rather than a second recorder.

The architecture is one semantic core with thin adapters. CLI, MCP, VS Code, GitHub,
and Sutra must not independently reinterpret turn selection, blame, diff, modified-since,
or undo safety.

## Outcome

At the end of Phase 2:

- Claude Code and Codex produce L2 records in the same repo with correct bracketing under
  the supported one-active-agent-per-root model.
- Any stdio-MCP host can query log/diff/blame/recall and, with explicit consent, undo.
- A real agent uses agentrec to identify its own bad turn, safely reverts it, and retries.
- VS Code renders turns without owning capture or destructive semantics.

At the end of Phase 3:

- A PR can show a deterministic, prompt-free agentrec report without becoming a gate.
- Signed logs detect changes to signed JSON meaning and identify the local signing key; they do not claim
  vendor-authenticated authorship.
- Sutra reads and acts through agentrec while keeping Sutra-only task/test metadata in
  Sutra-owned sidecars.

## Non-goals

- No HTTP/SSE MCP transport through Phase 3.
- No hosted service, accounts, telemetry, policy enforcement, or team dashboard.
- No first-party JetBrains, Neovim, Cursor, or Windsurf adapter.
- No claim that local signatures prove which human/model authored a change.
- No prompt text in PR comments, reports, attestations, or VS Code hover by default.
- No second capture engine inside VS Code, GitHub Actions, or Sutra.
- No correct attribution claim for concurrent agents in one root. D6 remains explicit:
  parallel agents use distinct worktree roots.

## Inherited decisions — confirmed; executors may not re-litigate

1. `PROTOCOL.md` outranks this document on wire semantics.
2. MCP transport is stdio through Phase 3 (D24).
3. MCP destructive modes remain `off | confirm | auto`, default `off`; modified-since
   is the destructive rail; every executed undo is a new turn (D15/D23/D30).
4. Codex joins Claude Code as the second first-party L2 emitter; VS Code is the first
   editor renderer; integration four onward is demand-driven.
5. Phase 3 signatures are Ed25519 over RFC 8785/JCS canonical bytes (D20).
6. L3 writers use one append-only sidecar per writer, merged at read time (D21).
7. All Phase 2–3 code is Apache-2.0; hosted/org behavior stays Phase 4+ (D17/D22).
8. The Claude plugin is packaging over the same setup behavior as `agentrec init`, not
   a second implementation (D44).
9. Bare turns remain unattributed windows everywhere. Renderers never relabel them as
   human or agent activity.

## Entry gate: Phase 1 truth substrate

Phase 2 implementation starts only after these Phase 1 contracts exist and pass:

1. Protocol 1.0 conformance fixtures for tolerant turn/signal readers.
2. A defined import honesty model. Historical transcripts rarely contain arbitrary
   before/after bytes; import must distinguish reconstructible turns from provenance-only
   hints and must never fabricate revertible snapshots.
3. `Agent-Turn:` trailer semantics or another exact commit↔turn association. Phase 3
   must not infer “turns in this PR” from file intersection alone.
4. Stable machine-readable read contracts. Current `log --json` exists, but `diff` and
   `blame` are still renderer-coupled and do not provide the promised JSON substrate.

If Phase 1 imports cannot reach the ROADMAP gate on real transcripts, stop. Do not build
Phase 2 consumers on ambiguous history.

## Architecture

```text
EMITTER ADAPTERS              SEMANTIC CORE                 SURFACE ADAPTERS

Claude hooks ───────┐                                    ┌─ CLI renderers
Codex hooks ────────┼─> signal inbox -> recorder         ├─ stdio MCP
Phase-1 importers ──┘            │                       ├─ VS Code
                                 v                       ├─ PR report action
                      .agentrec log + CAS + memory ──────┤
                                 │                       └─ Sutra/Tauri
                                 v
                    RepositoryView + UndoCoordinator
                    ReportBuilder + LedgerSigner
```

### Deep module 1: `RepositoryView`

Seam: read-only interpretation of a repository record. It owns log loading, L3 sidecar
merge, merged-turn suppression, tolerant parsing, blob integrity, diff, blame, gap
honesty, memory recall, pagination, and report-safe sanitization.

Interface contract:

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

Interface contract:

```rust
UndoCoordinator::preview(request: UndoRequest) -> Result<UndoPreview, UndoError>
UndoCoordinator::execute(preview: PreviewId, grant: UndoGrant)
    -> Result<UndoReceipt, UndoError>
UndoCoordinator::request_human(preview: PreviewId) -> Result<PendingUndo, UndoError>
UndoCoordinator::resolve(request: RequestId, decision: HumanDecision)
    -> Result<PendingResolution, UndoError>
UndoCoordinator::status(request: RequestId) -> Result<PendingStatus, UndoError>
```

`RepositoryView`, `UndoCoordinator`, `ReportBuilder`, and `LedgerSigner` live in
`agentrec-core`; adapters do not import the CLI crate. `IntegrationPlanner` lives in the CLI
crate because it owns tool-specific config and service installation.

`UndoRequest` contains the turn reference, optional path subset, and `allow_modified`.
A preview binds the target record, selected paths, current hashes, refusals, config mode,
and expiry. Execution takes an OS-backed exclusive `.agentrec/undo.lock` before claiming a
preview, then holds it across hash recheck, all working-tree writes, undo-turn append/fsync,
and terminal request-state append. The existing advisory guard remains recorder filtering,
not concurrency exclusion. Drift means `preview_stale`, no writes. Skipped/withheld entries
never enter the executable set.

Pending human requests use an append-only, 0600 event ledger under `.agentrec/`; request,
approve, deny, expire, execute, and fail are state transitions. Restart cannot convert an
unapproved request into an approval. Auto-mode confirm tokens are single-use, stored only
as hashes, expire after 60 seconds, and bind the complete preview. Creating a confirm-mode
pending request or issuing an auto-mode token takes `.agentrec/undo.lock`, rechecks the
preview, and atomically reserves its executable paths in that ledger. Any overlap with a live
reservation is rejected immediately with `undo_conflict`. Deny, expiry, failure, or successful
execution appends the terminal transition that releases the reservation.

### Deep module 3: `IntegrationPlanner`

Seam: setup intent before filesystem mutation.

Interface contract:

```rust
IntegrationPlanner::inspect(root: &Path) -> Result<DetectedIntegrations, SetupError>
IntegrationPlanner::plan(request: SetupRequest) -> Result<SetupPlan, SetupError>
SetupPlan::apply() -> Result<SetupReceipt, SetupError>
SetupPlan::reverse() -> Result<SetupReceipt, SetupError>
```

The plan describes exact file merges, backups, trust actions, service changes, and MCP
registration. `init`, `uninstall`, `doctor`, Claude packaging, and Codex packaging consume
this contract. Planning validates every input before any write; malformed config aborts
byte-identical.

### Deep module 4: `ReportBuilder`

Seam: deterministic review facts, separate from GitHub transport.

```rust
ReportBuilder::build(view: &RepositoryView, scope: CommitScope)
    -> Result<ReviewReport, ReportError>
ReviewReport::to_json() -> canonical JSON
ReviewReport::to_markdown() -> deterministic Markdown
```

The report selects turns through exact Phase 1 association, intersects files with the PR,
computes later-rich/bare/human-unknown context through `RepositoryView`, and excludes
prompts. Same inputs produce byte-identical JSON and Markdown.

### Deep module 5: `LedgerSigner`

Seam: raw-line signing and verification at the append/read edge.

```rust
LedgerSigner::sign(unsigned_record: &serde_json::Value) -> Result<Sig, SignError>
LedgerVerifier::verify_line(raw_json: &[u8], keys: &Keyring)
    -> Result<Verification, VerifyError>
```

Typed log loading is not the verification seam: it discards unknown fields. Verification
parses each raw JSON line, removes `sig`, canonicalizes the remaining object with JCS, and
verifies that byte string.

## Phase 2 design

### Codex L2 capture

Use Codex `UserPromptSubmit` + `Stop`, not Stop-only capture:

- `UserPromptSubmit` supplies `turn_id`, `prompt`, `session_id`, `cwd`, model, and
  transcript path. `agentrec hook codex` scrubs before appending the start signal.
- Add optional signal field `emitter_turn` carrying Codex `turn_id`. The signal router and
  crash journal carry it end-to-end. `(tool, event, session, emitter_turn)` makes duplicate
  starts/stops idempotent, including a restart between duplicates. A mismatched stop never
  closes the current bracket. This field does not create multi-bracket support.
- `Stop` closes the one open root bracket under existing D6 semantics; prompt/model/session
  fall back to the start signal. Transcript parsing is fallback only. A second non-duplicate
  start while another agent owns the root closes/truncates the prior bracket honestly; it
  never claims clean cross-attribution.
- Pin a minimum tested Codex version and store redacted hook-payload fixtures. CI canaries
  fail loudly when required fields disappear. Transcript format is explicitly unstable;
  importer fixtures isolate that risk.
- Subagent events do not create top-level turns in the first release. Their changes remain
  inside the parent bracket; revisit only with dogfood evidence.

Setup policy follows current Codex behavior:

- Prefer repo-local `.codex/hooks.json`; repo hooks require project trust and per-hook
  review. `init` prints the required `/hooks` action—it cannot silently grant trust.
- `IntegrationPlanner::inspect` searches accessible user/project/plugin hook sources for the
  agentrec marker. It reuses one active installation and refuses a visible duplicate. Managed
  sources may be opaque, so runtime `emitter_turn` dedup is the final safety layer.
- If exactly one project hook representation already exists (`hooks.json` or inline
  `[hooks]`), merge there. If both define hooks, refuse untouched and explain that Codex
  loads both; never create a duplicate callback.
- Project `.codex/config.toml` owns the stdio MCP registration. Hook definitions and MCP
  config may coexist; only duplicate hook representations are refused.
- `uninstall` removes only entries carrying agentrec's exact marker. `doctor` validates
  shape and recent signals; absence of trusted execution remains a loud degraded result.

Official surface references: [Codex hooks](https://developers.openai.com/codex/hooks/),
[Codex MCP configuration](https://developers.openai.com/codex/mcp/), and
[Codex configuration reference](https://developers.openai.com/codex/config-reference/).

### MCP server

Command: `agentrec mcp`, stdio, rooted at its canonical launch cwd. Reads work with the
daemon stopped. Config is loaded once at server startup; changing destructive mode requires
restart and is reported in `agentrec_status`.

Tools:

| Tool | Input | Bounded result |
|---|---|---|
| `agentrec_log` | filters, limit, cursor | ≤200 turn summaries + cursor |
| `agentrec_diff` | turn, optional paths, limit, cursor | ≤2,000 diff lines + file metadata |
| `agentrec_blame` | path, optional line/range | one line/range result; gap honesty included |
| `agentrec_recall` | query, k, cursor | fresh hash-verified memory hits only |
| `agentrec_status` | none | repo/daemon/config/health summary |
| `agentrec_undo` | action + mode-specific fields | preview/pending/status/receipt |

`agentrec_undo` is absent from `tools/list` in `off`. Its action contract is:

- `preview`: returns bound preview, refusals, warnings, and effective mode; no working-tree
  writes. In auto mode, successful preview atomically reserves executable paths and issues
  the token. In confirm mode it does not reserve until `request`.
- `request`: confirm mode only; appends a pending request for `agentrec approve|deny`.
- `status`: reads pending/approved/denied/expired/executed state.
- `execute`: auto mode only; requires the single-use 60-second token.

In confirm mode, `agentrec approve <id>` atomically claims the request, rechecks, and executes
while holding `.agentrec/undo.lock`;
`agentrec deny <id>` records denial without writes. The agent only polls `status` and can
never execute a merely-approved request a second time. Approval drift records `preview_stale`
and requires a new request. Multiple previews/pending requests may coexist; only one execution
may hold the lock, but live pending/token reservations must be path-disjoint. A second
overlapping request is rejected at creation, matching IMPLEMENTATION P6.

Tool descriptions state capability and data shape only. Outputs contain data, not
instructions to the model. Read tools carry read-only MCP annotations; undo carries
destructive annotations and still obeys agentrec's stronger internal consent state.

### Claude Code deepening and packaging

- `init` merges `.mcp.json` without replacing unrelated servers; malformed input aborts
  untouched with a backup policy matching hook setup.
- The Claude plugin invokes the installed setup command or ships generated artifacts
  checked byte-for-byte against `IntegrationPlanner` fixtures. Static plugin config is not
  allowed to drift into a second setup implementation.
- Binary installation always requires explicit consent. Plugin uninstall removes only
  plugin-owned config and delegates archive/service removal to `agentrec uninstall`.
- A transcript-format canary and rich-rate health alert remain mandatory.

### VS Code adapter

Recommended design: spawn one long-lived `agentrec mcp` process per initialized workspace
root and consume the typed query contracts. This keeps protocol interpretation, gap logic,
diff rendering, and blame semantics in Rust while retaining a small extension. It also
proves MCP is a useful consumer interface rather than agent-only plumbing.

- Activate only for roots containing `.agentrec/log.jsonl`.
- Request full visible-file blame in one range query; never one subprocess per line.
- Tail through server notifications if implemented; otherwise debounce a file watcher and
  invalidate/requery. Ledger shrink/rewrite forces full refresh.
- Use native Tree View + editor decorations first. A webview is allowed only for a diff
  surface native virtual documents cannot express.
- `undo` opens an integrated terminal with the CLI preview command. The extension never
  calls MCP undo in Phase 2 and never writes the working tree.
- Multi-root means one isolated process/state/cache per initialized root.
- No telemetry. Missing/incompatible binary yields one actionable status item, not repeated
  notifications.

This follows VS Code's native-extension guidance: use Tree Views and editor decorations for
native data, and reserve webviews for UI the workbench cannot express. See the
[Tree View API](https://code.visualstudio.com/api/extension-guides/tree-view) and
[webview guidance](https://code.visualstudio.com/api/ux-guidelines/webviews).

Alternative rejected for Phase 2: a TypeScript protocol reader. It would prove a second
language can consume the format, but duplicates the hardest semantics before conformance
fixtures and Protocol 1.0 have dogfood evidence. Publish fixtures so the community can build
one; reconsider when a second non-Rust first-party consumer actually needs it.

### Self-healing acceptance story

The demo is not scripted output. In a disposable real repo:

1. Agent makes a change that breaks a named test.
2. Agent calls blame/diff and identifies its own causal turn.
3. Agent previews undo; the user/config grants the permitted path.
4. Agentrec executes, records the undo as a turn, and the test returns to green.
5. Agent retries with a different implementation.

The trace, tool calls, working-tree hashes, and final log are retained as test evidence.

## Phase 3 design

### PR report Action

Phase 3 ships a report engine plus a thin GitHub Action, not a policy bot.

- Inputs: exact base/head SHAs, explicit log path or exported artifact, optional path
  filters, optional trusted public-key bundle, and comment mode.
- Selection: exact Phase 1 commit↔turn association. File overlap narrows selected turns;
  it never creates the association.
- Output: associated turns, tools, grades, files, recording gaps, files changed after their
  selected agent turn, association completeness, and optional signature status. Rewritten
  commits whose association did not survive are `association_unavailable` or `partial`, never
  guessed. Prompt text and absolute local roots are omitted.
- Same-repo PR with `pull-requests: write`: create/update one sticky comment marked
  `<!-- agentrec-report:v1 -->`.
- Fork/read-only token: write the same Markdown to the job summary and log `comment skipped`;
  success, never a failed check.
- In comment mode, report code comes only from a digest-pinned immutable release or trusted
  base SHA. Fetch PR log/data separately as inert bytes; never run `uses: ./`, repository
  scripts, binaries, actions, or build steps from the PR head/merge checkout while a write
  token exists. Disable persisted credentials. Treat every log/path/string as untrusted
  input; sanitize terminal/Markdown control and bound records, files, and bytes.
- Missing/invalid record yields an honest partial/unavailable report, never a greenwashed
  zero-turn report.
- Signature states are `unsigned`, `valid_untrusted_key`, `valid_trusted_key`, `invalid`, and
  `unknown_key`. A key is trusted only when supplied separately by workflow configuration;
  a public key carried beside an untrusted artifact can establish validity, not identity.
- Phase 3 is report-only. Enforcement belongs to Phase 4 org policy.

The workflow uses least privilege. Comment mode requests `pull-requests: write`; report-only
mode stays `contents: read`. GitHub artifact attestations, if added to release builds, remain
separate from turn-log signatures; see
[GitHub artifact attestations](https://docs.github.com/en/actions/concepts/security/artifact-attestations).

### Signed entries

Signature payload:

1. Strictly validate one I-JSON object, rejecting duplicate member names, non-finite/
   non-interoperable numbers, invalid Unicode, and a non-object top level.
2. Remove the top-level `sig` member.
3. JCS-canonicalize the remaining object.
4. Ed25519-sign those bytes.
5. Compute `key_id` as lowercase-hex SHA-256 of the raw 32-byte Ed25519 public key.
6. Store `sig = {"alg":"ed25519-jcs","key_id":"<64 lowercase hex>","val":"<base64url-no-pad>"}`.

All turn and epoch records appended after signing is enabled are signed through the single
ledger append seam, including undo and crash recovery. Signal and memory ledgers are not
signed in Phase 3. A verifier processes raw lines and returns per-line status plus one log
summary: `verified`, `unsigned_prefix`, `unknown_key`, `malformed`, or `tampered`.

Keys:

- Generate/export public key via `agentrec key gen|export`.
- Use OS keychain where available; 0600 file fallback for headless Linux/CI.
- Never export a private key through CLI stdout.
- Rotation starts a new key id. Old records remain verifiable when the verifier receives a
  key bundle containing both public keys; `sig.key_id` selects the key. In-ledger rotation
  statements, revocation, and distributed identity are out of scope.

Honesty text is mandatory: this proves the signed JSON meaning—member names and values—has
not changed since a holder of this local key signed it. JCS deliberately normalizes member
order and whitespace, so byte-only formatting changes remain valid. This does not prove model
identity, human identity, or vendor-native attestation. GitHub build artifact attestations
prove build provenance, a separate claim.

### Sutra rebase

Current Sutra is not a drop-in `turns.rs` replacement. It has numeric ids, consume-once poll
deltas, mutable rollback/test flags, xxh3-era storage, and `.sutra/turns` manifests. Agentrec
has ULIDs, SHA-256 CAS, an external recorder, append-only corrections, and different safety
semantics.

Target ownership:

- Agentrec owns capture, turn facts, CAS, blame/diff, gap honesty, memory, and undo.
- Sutra owns task links, test status, review state, UI selection, and presentation in a
  `.sutra` sidecar keyed by agentrec ULID.
- Sutra does not run a second `TurnEngine` for an initialized root.

Migration:

- Add the migration engine to `agentrec-core`, with `agentrec import sutra-legacy` as its
  manually testable CLI adapter. To preserve current IMPLEMENTATION U1, Sutra invokes the
  same engine automatically on first launch after the compatibility spike. Detection,
  validation, conversion, fsync, and archive rename complete before Sutra switches readers;
  any failure leaves the old store and current reader untouched.
- Legacy entries without reconstructible/current snapshot semantics are imported as
  non-revertible history with `skipped: true` plus additive
  `non_revertible_reason: "legacy_snapshot_unavailable"`, not silently upgraded to safe
  agentrec turns. Renderers must show that reason; they must not call it over-cap or I/O
  failure. `baseline_unknown` remains reserved for unknown prior state, not missing snapshot
  bytes.
- Existing `.agentrec` data is merged append-only; migration never replaces it.
- Tauri exposes a compatibility adapter matching current UI needs while frontend ids move to
  ULID strings. A durable `.sutra` old-id→ULID map preserves task/test links. Test status and
  rolled-back presentation become Sutra sidecar state.
- Cutover requires all Sutra Rust/TS tests plus manual multi-root, restart hydration, task
  evidence, rollback, and migration verification.

## Implementation brainstorm — recommended order

### Phase 2: five mergeable phases

1. **Truth/API gate:** finish Phase 1 import/trailer/conformance work; extract
   `RepositoryView` and `UndoCoordinator`; add JSON contracts for diff/blame/status.
   Exit: CLI behavior byte-equivalent; typed-interface tests cover current ACs.
2. **Codex neutrality:** spike live hook payload/trust/install on the pinned Codex version,
   then add start+stop adapter, importer fixtures, init/uninstall/doctor symmetry.
   Exit: one real Claude + one real Codex turn attributed in the same repo.
3. **MCP read then destructive:** ship bounded read tools first; add typed config, pending
   approval ledger, tokens, concurrency, and refutation tests second.
   Exit: self-healing E2E passes with daemon stopped during reads.
4. **Setup/package:** `IntegrationPlanner`, Claude/Codex MCP merges, plugin packaging, and
   byte-identity tests.
   Exit: install/re-init/uninstall leaves unrelated config byte-preserved.
5. **VS Code:** native decorations/tree/virtual diff over one MCP process per root.
   Exit: 10k-turn/multi-root live test, <2 s refresh, <5 MB package, no telemetry.

### Phase 3: four mergeable phases

1. **Association/report spike:** prove exact PR turn selection from Phase 1 trailers/export;
   add deterministic `ReportBuilder` and hostile-data fixtures.
2. **GitHub adapter:** job summary first, then sticky same-repo comment; fork no-op and
   least-privilege permissions tested.
3. **Signing:** raw-line canonicalization spike, centralized signed append, key storage,
   key-bundle rotation, verification, and tamper corpus.
4. **Sutra compatibility spike then cutover:** freeze mapping/sidecar/migration contracts
   before editing Sutra; migrate backend seams, then frontend ids/presentation.

No phase starts with UI. Each starts at the semantic seam its later adapters consume.

## Scariest unknowns and spike exits

| Unknown | Spike exit |
|---|---|
| Historical imports cannot reconstruct safe before/after state | Import corpus classifies every item as reconstructible or non-revertible; zero fabricated snapshots |
| Codex hook/trust behavior drifts by version | Live fixture on minimum + current versions; start/stop correlation and `/hooks` trust observed |
| MCP host approval differs across clients | Internal off/confirm/auto state machine passes without host UI; host annotations remain additive UX |
| PR↔turn association over/under-counts | Fixtures prove direct associations; squash/rebase/force-push without a surviving export report explicit partial/unavailable |
| Signing canonicalization differs across implementations | Rust vectors cross-check against an independent RFC 8785 + Ed25519 implementation |
| Sutra legacy semantics cannot map honestly | Migration fixture labels every legacy field kept/dropped/sidecarred; rollback equivalence proven or explicitly refused |

## Verification strategy

- Unit: typed query/undo/report/signing interfaces, total error/status enums, canonical vectors.
- Integration: real binary over temp repos; MCP initialize/list/call/shutdown; hook stdin/stdout;
  config merge/reverse; pending approvals across restart; simultaneous undo conflict.
- Conformance: Protocol 1.0 golden records consumed by Rust and every external adapter.
- Adversarial: malformed/torn/huge JSONL, cursor invalidation, prompt/ANSI/Markdown injection,
  path traversal, symlink escape, token replay, approval race, signed key/value mutation.
- Performance: 10k turns; MCP p95 read <100 ms warm; VS Code visible-file refresh <2 s;
  PR report bounded by explicit byte/record caps.
- Live behavior: actual Claude/Codex hooks, MCP host session, VS Code multi-root window,
  GitHub same-repo/fork PRs, keychain + Linux fallback, Sutra migration and rollback.

## Manual E2E script

1. Initialize one disposable repo with Claude Code and Codex installed; review/trust hooks.
   Observe both start+stop signals and two rich, correctly attributed turns.
2. Stop the daemon. From an MCP host, list, diff, blame, and recall successfully.
3. Break a test from an agent; run the self-healing story. Observe preview, consent, exact
   file restoration, new undo turn, and green retry.
4. Open the repo plus an uninitialized root in a VS Code multi-root workspace. Observe blame
   only in the initialized root, live append refresh, virtual diff, and terminal-only undo.
5. Open same-repo and fork PR fixtures. Observe one updated comment for same-repo, job-summary
   fallback for fork, no prompts/absolute roots, and a successful report-only check.
6. Sign new records, change one signed field value and verify failure, then restore it,
   reorder JSON members and verify success, rotate the key, and verify old/new segments.
7. Migrate a copied legacy Sutra store. Observe archive preservation, explicit non-revertible
   legacy rows, ULID task/test sidecars, restart hydration, and agentrec-owned rollback.

## Proposed choices needing founder confirmation

1. **Blocking — VS Code seam.** Recommend long-lived `agentrec mcp` per root, not a new
   TypeScript protocol reader. Tradeoff: requires the binary; wins semantic locality and
   keeps the extension small.
2. **Blocking — Phase 3 PR association.** Recommend exact Phase 1 trailers/export only.
   Do not offer a timestamp/file-overlap heuristic mode; an impressive wrong number damages
   the trust product.
3. **Blocking — Sutra ownership.** Recommend external agentrec recorder + Sutra sidecar/UI,
   not embedding another active recorder through `agentrec-core`.
4. **Non-blocking — subagents.** Keep Codex subagent mutations inside the parent rich turn in
   first release; add subagent-level turns only after observed debugging value.
5. **Non-blocking — signature scope.** Sign turn+epoch ledgers only in Phase 3; memory/signals
   wait for a concrete attestation consumer.

## Required companion updates after founder confirmation

- Add `agentrec_status` to `PROTOCOL.md` §8 or remove it from this design; it is not in the
  current normative tool table.
- Add optional signal `emitter_turn` to `PROTOCOL.md` §4 for duplicate-hook idempotency;
  explicitly retain D6's one-open-bracket-per-root limitation.
- Add file-entry `non_revertible_reason: "legacy_snapshot_unavailable"` to `PROTOCOL.md` §5
  plus tolerant-reader/render/undo fixtures before Sutra migration can emit it.
- Define the exact `sig.alg`, key-id encoding, base64 variant, duplicate-key rejection, and
  unsigned-prefix/semantic-integrity rules in `PROTOCOL.md` before signing code lands.
- Amend `IMPLEMENTATION.md` O to require Codex `UserPromptSubmit` + `Stop`, not Stop only.
- Amend `IMPLEMENTATION.md` R if VS Code consumes `agentrec mcp` instead of directly parsing
  the store.
- Replace the mechanical Sutra “replace `turns.rs`” AC with the automatic-but-transactional
  compatibility/migration gate above after the ownership choice is confirmed.

## Rejected approaches

- **Reuse CLI stdout inside MCP:** unstable text, no typed errors, duplicates parsing.
- **Copy query logic into each adapter:** turns protocol tolerance into five divergent products.
- **VS Code direct-write undo:** forks consent and modified-since safety.
- **Always expose MCP undo and reject at call time:** violates `off` mode's capability boundary.
- **Infer PR turns from changed-file overlap:** overcounts historical activity and fabricates
  precision.
- **Treat local signatures as authorship proof:** a self-held key signs a diary, not vendor
  identity.
- **Replace Sutra `turns.rs` mechanically:** incompatible ids, storage, mutation model, polling,
  and rollback semantics make this a migration, not a dependency swap.
