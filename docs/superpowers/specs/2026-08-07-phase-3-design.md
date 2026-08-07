# Phase 3 design — leverage the ledger (analytics, search, annotate, bisect, undo power, digest, policy signals)

Date: 2026-08-07. Status: **SKEPTIC GATE PASS (round 3, Fable, 2026-08-07)** —
pending founder approval before implementation planning.
Gate history: round 1 GATE FAIL (B1 partial-undo already shipped in Phase F —
section rewritten as delta; B2 restore gate violated frozen predicate-2 MUST
NOTs — replaced with recorded-state-hash gate; B3 checkpoints didn't store
bytes — creation now snapshots; B4 rework-rate under-defined — event-level
definition with exclusion buckets; B5 bisect walk self-contradictory — single
reverse-apply algorithm; advisories A1–A8 folded).
Round 2 GATE FAIL (NB1 stale cross-cutting clauses contradicting B1 fix —
deleted; NB2 gap-fatal bisect probes near-inoperative — gaps now tolerated
with caveat line; NB3 rework clause (c) uncomputable — additive `reverts`
field chosen; NA1–NA6 folded).
Author: brainstormed with founder; decisions below are founder-confirmed in-session.

## Vision

Phase 2 made the ledger trustworthy (protocol frozen, import real, MCP read live,
destructive gated). Phase 3 exploits it: answer questions nobody else can answer
(git cannot distinguish agent from human), and deepen undo. **agentrec stays a
recorder, never a gatekeeper** — nothing in this phase blocks, intercepts, or
vetoes any write. Everything local; no network code is added.

## Founder decisions (in-session, 2026-08-07 — not re-litigable by executors)

1. Scope shape: all eight capabilities, sub-phased 3.0 → 3.1 → 3.2, read-only wedge first.
2. Output surface: CLI verbs only (text/`--json`); no GitHub integration, no export-file
   pipeline. agentrec never posts anywhere.
3. Partial undo granularity: file-level only.
4. Digest source: direct fold over `log.jsonl`; zero dependency on memory v1
   (whose candidate path is CLOSED FAILED, 2026-07-31).
5. Policy alerts surface: append-only `alerts.jsonl` + `status`/`digest`/`alerts` verbs;
   no push channel (proposed by agent in-session, not objected to — treat as confirmed
   unless founder overrides at spec review).

## Sub-phase 3.0 — read-only wedge

No new write paths. Every feature is a fold over `log.jsonl` + CAS through
`RepositoryView` (the P4/P4b seam). Adding a new direct `load_log` caller in
`cli/src` is a spec violation (that seam was closed deliberately; residual
callers in `readcmds.rs`/`memorycmds.rs` are recorded debt, not precedent).

### 3.0.1 `agentrec stats [--since <dur>] [--json]`

Per-repo analytics over the turn ledger:

- Turn counts by grade (rich/bare), tool, model.
- Files touched, ranked by churn bytes (sum over turns of byte-delta between
  before/after blobs; files with `skipped`/`withheld` entries contribute 0 for
  those entries and are footnoted as undercounted).
- Agent-vs-human share of change: agent = union of rich-turn coverage
  EXCLUDING `tool:"git"` turns (a checkout is not agent authorship) and
  imported turns (no epoch coverage; reported in their own `imported`
  bucket); human = `human-edited-since` windows; **recording gaps render as
  a third bucket, "unattributable" — never allocated to either side.** Bare
  turns are unattributable by definition (never rendered as agent activity).
- **Rework rate** (the headline metric). Unit of analysis is the
  **(turn, file) write event**, not the file — a file written twice enters
  the denominator twice, each event judged over its own window. Definition:
  - Denominator: every write event (create or write op) by a rich agent turn
    whose `ended` is ≥ N days (default 7, `--rework-window`) before query
    time. Younger events are right-censored: excluded from both sides,
    counted in `censored_recent`. Excluded from the denominator entirely:
    bare turns; `tool:"git"` turns; `tool:"agentrec"` (undo) turns; imported
    turns (keyed on `imported: true` — NOT `files_complete`, which is
    reserved for a future attesting importer — import history has no epoch coverage,
    so their windows are all-gap by construction; counted in
    `excluded_imported`). On import-heavy repos this can empty the metric —
    the output prints the exclusion counts and, when the denominator is 0,
    says "no measurable events" instead of rendering 0%.
  - Numerator: a denominator event is rework iff, within its N-day window,
    the file is subsequently (a) modified by a change not covered by any
    rich turn (`human-edited-since` fires for that span — display-predicate
    use, which is its sanctioned role; nothing destructive keys on it), or
    (b) **deleted** by a change not covered by any rich turn (deletion is
    rework; phrased like (a) — the ledger cannot attribute a bare deletion
    to "non-agent", it can only say it is uncovered), or
    (c) reverted by an undo turn whose new additive `reverts` field names
    that write's turn id (an undo of agent work is rework by the human's
    hand). **`reverts` is a new additive field on undo turn records**
    (PROTOCOL §5 amendment + conformance fixture, same commit) — today the
    target survives only in display prose (`prompt_excerpt`), which is not
    computable and MUST NOT be parsed; clause (c) therefore only counts
    undo turns written by 3.x-era binaries, and the output's exclusion
    note says so. `reverts` is a single turn id (an undo targets exactly
    one turn today; widening to an array is a future additive change).
    Checkpoint-restore undo turns carry NO `reverts` (they target a
    checkpoint, not a turn) and are deliberately not counted by (c).
    A rename observed as delete+create counts via (b) — no
    rename tracking exists in the ledger and none is invented here.
  - **Gaps are TOLERATED, not excluding (T0 spike amendment, founder-ruled
    2026-08-07):** the original any-gap-in-window exclusion measured
    `measurable=0` on the dogfood corpus (`docs/verify/p30-rework-spike.md`
    — daemon-restart gaps are seconds–minutes and every 7-day window
    contains one; same defect class as the bisect gap-fatality fixed at
    round 2). Instead: gap-overlapped windows stay measurable, the output
    counts them (`gap_overlapped`), and the rework rate is labeled a
    **lower bound** — an edit hidden inside a gap is invisible to the
    ledger, so rework can be undercounted, never overcounted, and the
    output says so.
  - Unevaluable events: current hash ≠ last recorded `after` with no subsequent
    turn AND no recorded gap (missed-watch / noise-glob shadow) — the
    modification time is unknowable → `excluded_unknown_mtime`. Both printed.
  Every exclusion bucket prints beside the rate; the rate is
  numerator/denominator over MEASURABLE events only, and the output must
  render the measurable count.

Honesty rules: every figure that has an exclusion (gaps, skipped, withheld,
right-censored, **dangling blob refs** — TTL/eviction-legalized per PROTOCOL
§6, they undercount churn bytes) must print its exclusion count next to it.
No figure may be rendered without its denominator.

### 3.0.2 `agentrec search <query> [--regex] [--json]`

Substring (default) or regex match over: stored prompt text (scrubbed form —
the only form persisted; prompts live in CAS via `prompt_ref`, so search DOES
read CAS blobs for prompts — a dangling `prompt_ref` is handled gracefully
per PROTOCOL and counted in the output, never a crash or silent skip), turn
metadata (tool, model), and turn file paths. Returns turn ids + matched-field
snippet + timestamp. Paged with the existing `Page<T>`/cursor machinery
(occurrence-ordinal cursors — same-id duplicate turns are a documented
reality).

Non-goal, worded precisely: content search over **file snapshot** blobs
(cost/scope; revisit on demand — a future `--content` flag slot is reserved
in the interface but MUST error "not implemented" if passed, not silently
degrade to metadata search). Prompt blobs are in scope as above.

### 3.0.3 `agentrec annotate <git-range> [--json|--md]`

Joins `git blame --porcelain` over the range's changed files against turn
records: for each blamed commit+line-range, reports which turn(s) (if any)
wrote content overlapping those lines, with prompt excerpt, tool, model,
timestamp; otherwise "human" or "unattributable (gap)".

**Precision contract (normative, prevents overclaim):** attribution is
file-and-time certain, **line-level best-effort**. Turns store file-level
before/after snapshots, not per-line provenance; line mapping is derived by
diffing the turn's before/after blobs and intersecting with blame ranges.
Renames, reformat-only commits, and interleaved human+agent edits within one
file between commits WILL misattribute at line granularity. Output must carry
a one-line disclaimer in text/md modes and a `"line_precision": "best_effort"`
key in JSON. Any wording implying line-exactness is a defect. Dangling
turn blobs (TTL/eviction-legalized) degrade line mapping for that turn:
those ranges render "unattributable (blob evicted)", counted in the
output — never a crash, never silently attributed file-level-only.

Output is local only. Posting to a PR is the user's/CI's job.

### 3.0.4 `agentrec bisect --test <cmd> [--good <turn>] [--bad <turn>]`

Binary search over the turn sequence for the first turn where `<cmd>` fails.

- Materializes each probe state into a **scratch directory** (never the
  working tree). **Single algorithm (rewritten after gate B5 — round 1 gave
  two contradictory descriptions):** copy the current working tree, then
  walk turns AFTER the probe point in reverse order (latest first),
  reverse-applying each: for every file entry, write the entry's `before`
  bytes (or delete the file for a create op, recreate for a delete op).
  Latest-first order makes each file's final scratch state the `before` of
  the EARLIEST post-probe turn touching it — exactly "working tree minus
  later agent turns".
  - Sequence membership: **rich, non-imported turns only** by default. Bare
    turns are never subtracted (their bytes may be human work) and never
    probe candidates; `--include-bare` exists for completeness, prints a
    misattribution warning, subtracts them like any turn, AND makes them
    probe candidates (a bisect result may then name a bare turn — the
    warning states the attribution caveat).
  - A probe is **unanswerable** when any needed `before` is absent
    (skipped/withheld/dangling blob) or is `after_synthesized`-derived
    (derived bytes are not recorded fact — PROTOCOL §import-honesty — and
    MUST NOT silently materialize into a probe state). Unanswerable probes
    are reported; bisect returns the narrowest answerable span instead of
    guessing.
  - **Recording gaps are TOLERATED, not fatal** (round-2 correction: every
    daemon restart mints a gap, so gap-fatal probes would make bisect
    near-inoperative on any real repo). This is consistent with the stated
    fidelity posture: the scratch state already tolerates human edits
    staying current, and a gap is just a window of possibly-unrecorded
    edits — same class. Bisect's report lists gap windows intersecting the
    searched span as a caveat line, mirroring blame's honesty without
    refusing to answer.
  - `--good`/`--bad` turn refs resolve through the same ambiguity path as
    `show`/`diff`: a same-id duplicate ref is a hard "ambiguous turn id"
    error, never first-match.
- Runs `<cmd>` with cwd = scratch dir; exit 0 = good, nonzero = bad,
  configurable `--flaky-retries N` (default 0; retried disagreement =
  unanswerable probe, reported).
- Read-only by construction: the working tree and `.agentrec/` are never
  written. The scratch dir lives under the system temp root and is cleaned
  unless `--keep`.
- Turns interleaved with human edits limit fidelity: the scratch state is
  "working tree minus later agent turns", which is NOT a historical snapshot
  of the whole repo at that time. This is stated in `--help` and the docs;
  bisect answers "which agent turn introduced the failure given everything
  else stays current", which is the actionable question.

## Sub-phase 3.1 — undo power

### 3.1.1 File-level partial undo — ALREADY SHIPPED (Phase F); scope is a delta, not a build

**Correction from gate round 1: this capability exists on both surfaces.**
CLI `undo --files` (`main.rs` undo args → `readcmds::undo(..., &files)`) and
MCP `paths` on the destructive request (`mcpcmd.rs` input schema;
`UndoRequest.paths: Option<Vec<PathBuf>>`; path-level reservations;
`UndoError::UnknownPaths` for typos). Shipped semantics are normative and
MUST NOT be silently changed:

- Per-file classification: proceed on the `Revert` subset, list
  `Excluded`/`Refused` rows (`readcmds.rs` rendering). There is NO
  abort-on-any-refusal default; introducing one would be a behavior break to
  a live verb and is NOT in scope.
- Unknown paths error lists the unmatched inputs (shipped `UnknownPaths`),
  not the turn's file inventory.
- No new `files` field anywhere — the existing `paths` field IS the subset
  mechanism; adding a second subset field is banned (undefined conflict
  semantics + redundant §8 row).

Remaining 3.1.1 scope (the actual deltas):

1. **Imported-turn interaction statement + test:** the pinned imported-K2
   invariant ("never a partial revert of the other genuinely-revertible
   entries in the same turn", `undo_coordinator.rs::same-turn` refusal path)
   must be traced against `--files` on an imported turn; the resolved
   behavior (whole-turn refusal wins over path subsetting) gets a
   refusal-matrix test and a doc sentence. If the trace shows subsetting can
   bypass the K2 refusal, that is a defect fix, not a design choice.
2. **Confirm-mode visibility trace:** verify (and test) that the MCP `paths`
   subset is rendered in the confirm-mode `approve` view, so the human
   approves the actual subset, not the whole turn.
3. **User docs:** README/help surface for `--files` (currently undocumented
   for end users).

### 3.1.2 Checkpoints — `agentrec checkpoint <name>` / `restore <name>`

- `checkpoint <name>`: appends a new record type `type:"checkpoint"` to
  `log.jsonl` pinning `{name, ts, files: [{path, hash|null, state}]}` for
  the **agent-touched set** (files appearing in any turn's entries), where
  `state` ∈ {`pinned`, `deleted`, `withheld`, `skipped`} — the record
  carries the exclusion markers its own prose requires. A currently-deleted
  agent-touched file pins `state:"deleted"` (hash null); restore then
  deletes the file iff the recorded-state-hash gate passes for its current
  bytes — absence is a restorable state, not an error. Explicit
  boundary: files never touched by any turn are OUT of checkpoint scope —
  git covers them; the spec bans marketing checkpoints as full-repo snapshots.
  - **Additive-protocol note (gate A1):** PROTOCOL v1's freeze clause
    enumerates open enums but not `type`; the tolerated conformance fixture
    (`log_unknown_record_type.jsonl`) already anticipates `type:"checkpoint"`.
    The implementation plan must include the explicit §5 amendment adding
    checkpoint + tombstone record rows (with fixtures, same commit), not rely
    on tolerance alone.
  - **Checkpoint creation STORES bytes (gate B3):** for each in-scope file
    whose current content hash is not already present in the CAS, checkpoint
    writes the blob at creation time — otherwise a checkpoint can be
    unrestorable at birth (human-edited-since files, `skipped` over-cap
    entries, daemon-down windows all leave live bytes unsnapshotted). These
    writes obey the SAME persist rules as turn snapshots: secret-pattern
    files are excluded from the checkpoint set and listed in the record as
    `withheld` (never read, never stored); files over the blob cap are
    listed as `skipped` (pinned by neither hash nor bytes). `checkpoint`
    prints the excluded lists; a checkpoint whose set is partially excluded
    says so at creation, not at restore.
- Checkpoint records pin blobs: the eviction protect-set gains checkpoint-
  referenced hashes (same mechanism as open-turn/memory-pin protection).
  Unbounded pinning is real cost — `purge --checkpoints-expired` is NOT built
  in this phase; instead `checkpoint --delete <name>` appends a tombstone
  record (append-only discipline; the pair drops out of the protect set).
- `restore <name> [--dry-run]`: computes per-file revert plan: current hash
  == pinned hash → skip; else revert from the pinned blob. **Safety gate
  (rewritten after gate B2 — round 1's coverage-based gate violated
  PROTOCOL's frozen MUST NOTs at §"two predicates", which ban keying any
  destructive op on `human-edited-since`):** a file may be reverted only if
  its CURRENT content hash equals some recorded snapshot in the ledger (any
  turn's `before` or `after`, or a checkpoint pin) — i.e. the state being
  destroyed is provably recorded and re-restorable. Current bytes matching
  nothing in the ledger → refuse that file by default (unrecorded work would
  be destroyed); `--allow-unrecorded` proceeds after snapshotting the
  current bytes first (the restore turn's own `before` entries make even
  this reversible — undo-is-a-turn does the work). This gate never consults
  coverage/`human-edited-since` and never treats bare turns as attribution.
  Executes as ONE undo turn listing all reverted files, per-file
  Revert/Refused/Excluded classification matching the shipped `undo --files`
  rendering. Missing CAS blob → refuse that file, list it, never
  silent-partial (rare post-B3 — only TTL/purge can remove a pinned blob,
  and pins protect against eviction).
  - **Coordinator note (gate A8):** `UndoCoordinator::preview`/`build_plan`
    are turn-keyed today; restore needs a non-turn-keyed plan entry point.
    The plan must budget this as a real coordinator extension (new entry
    point sharing the classification/persist internals), not "rides
    preview". Failure-path persistence (11(b)) requirements apply to the new
    path identically and get their own fault-injection test.
- Name rules: unique among live (non-tombstoned) checkpoints; collision →
  error. Name reuse after tombstoning is allowed; the pair-matching rule is
  replay order — a tombstone retires the LATEST live checkpoint of that name
  at its point in the log, so any fold over `log.jsonl` resolves liveness
  deterministically. Daemon ignores checkpoint records entirely (no behavior
  change).
- Protect-set edit site named (gate A2): checkpoint pins enter eviction
  protection via the parsed-record path — `cmds.rs::extra_protected_refs`
  only raw-harvests unparseable lines today and must gain a checkpoint arm
  (its own doc comment predicts exactly this edit). Old binaries fail to
  parse checkpoint records and over-protect via raw harvest — safe
  direction, noted not relied on.

## Sub-phase 3.2 — digest + policy signals

### 3.2.1 `agentrec digest [--since <dur>] [--json]` + MCP `agentrec_digest`

Deterministic fold over `log.jsonl`: per-tool/model turn counts; top files by
churn; prompt excerpts of the N most recent rich turns (N default 10); open
(unacked) alerts; gap windows in the period. Purpose: paste or MCP-inject
into a next agent session's context ("what happened here lately").

- MCP tool is the sixth read tool, same shape as Phase E's five; PROTOCOL §8
  additive row; payload parity with `--json` (byte-equal, parity-pinned like
  diff/blame).
- Prompt excerpts are already-scrubbed text (scrub runs at persist); digest
  adds NO new scrub pass but MUST NOT read any unscrubbed source (there is
  none in the ledger — stated for the skeptic, verifiable).
- Memory v1 seam: recorded as future work; nothing in 3.2 reads or writes
  `memory.jsonl`. Founder owns memory v1 repair.

### 3.2.2 Policy signals — observers, alerts, never blocks

- `config.toml` gains `[policy]`: `protected_paths = [glob...]`,
  `watch_paths = [glob...]`, `max_files_per_turn = N`. Parsed by the Phase B
  loader (hard-error on invalid TOML at CLI/daemon-startup, per D16 pattern).
- Daemon evaluates rules **at turn close** — after the fact by construction;
  the turn is already closed and persisted, so blocking is impossible even by
  bug. Gatekeeper posture unreachable by design, not by discipline.
- Rule hit → append `{id, ts, turn_id, rule, matched_paths}` to a new
  append-only `alerts.jsonl` (same discipline as signal/log: append-only,
  no rewrite class, torn-tail tolerated). **Debt recorded at birth (gate
  A6):** `alerts.jsonl` is unbounded, same class as the recorded
  `undo-requests.jsonl` debt; reclaim needs a future decision-register
  entry and is NOT built here. `status` inbox-style byte accounting
  includes it so growth is visible.
- Surfacing: `status` prints open-alert count; `digest` includes open alerts;
  new `agentrec alerts [--json]` lists, `alerts --ack <id>` appends an ack
  record (never rewrites).
- Non-goals with reasons: no push channel, no exec-on-alert, no webhooks —
  push is gatekeeper-adjacent posture creep and no daemon→user channel
  exists; adding one is a separate founder decision.

## Cross-cutting rules (whole phase)

- **PROTOCOL v1 is frozen; every change here is additive** and lands with
  conformance fixtures in the same commit: checkpoint + tombstone record
  types, the additive `reverts` field on undo turn records (see rework
  clause (c)), `alerts.jsonl` (documented as implementation file, not wire —
  decide at freeze-review whether it enters PROTOCOL at all), and the §8 row
  for `agentrec_digest`. NO destructive-request payload change — the shipped
  `paths` field already carries file subsets (§3.1.1).
- **Non-goals, recorded:** hunk-level undo (diff/merge machinery + conflict
  surface; file-level covers the common case); content search over file
  snapshot blobs (prompt blobs ARE searched — §3.0.2); GitHub/
  network posting; team mode; blocking/intercepting anything; memory v1
  coupling; full-repo checkpoint snapshots.
- **Testing bar:** every verb gets unit + integration (real binary, tempdir
  fixture) + goldens. Analytics get a **hand-computed fixture corpus** — a
  small ledger whose stats/rework figures are computed by hand in the test
  file, defending against plausible-but-wrong math (this repo's signature
  defect class is confident-false assertions; a self-checking fixture is the
  countermeasure). Bisect gets adversarial fixtures: flaky test cmd, probe
  with missing snapshots, turn set with human interleaving. Partial undo +
  restore get refusal-matrix tests (modified/skipped/withheld/missing-blob ×
  CLI/MCP surface, asserting the shipped per-file Revert/Refused/Excluded
  classification; restore additionally × recorded/unrecorded current state
  and `--allow-unrecorded`). Real-corpus checks where shape claims are
  made (fixture-only evidence cannot close corpus-shape claims — recorded
  lesson).
- **Sequencing & gating:** 3.0 → 3.1 → 3.2; each sub-phase independently
  mergeable, each ends with a Fable skeptic GATE PASS before the next starts.
  This spec itself is skeptic-gated before any implementation plan is written
  (plan-level gating caught 12 defects pre-code last time it was used).
- **Effort split (house policy):** Sonnet implementers per task, Opus
  reviewers where warranted, Fable skeptic gates. Analytics math and bisect
  walk are the hard tasks; rank them for the stronger models.

## Risks (named now so the gate can probe them)

1. **Rework-rate definitional traps** — right-censoring, gap overlap, undo
   turns. Definition above is normative; any implementation deviation is a
   defect, not a judgment call.
2. **Annotate line-precision overclaim** — the precision contract is the
   defense; goldens must include a misattribution-shaped fixture proving the
   disclaimer renders.
3. **Bisect scratch-state fidelity** — "not a historical snapshot" must be
   documented and tested (fixture with human interleaving asserting the
   reported semantics).
4. **Checkpoint pin growth** — protect-set bloat on long-lived checkpoints;
   tombstone path must be tested for actually releasing pins; store-budget
   interaction (pinned bytes count toward budget and are reported, mirroring
   memory pins).
5. **Alerts channel noise** — `max_files_per_turn` will fire on legitimate
   big refactors; ack flow must be cheap; default config ships with policy
   table EMPTY (no rules = no alerts = no noise for non-opted-in users).
6. **Search over same-id duplicate turns** — cursors must use occurrence
   ordinals (P4b lesson pinned in code already).

## Open questions — RESOLVED by founder (2026-08-07, in-session)

1. `alerts.jsonl`: implementation-private in 3.2; promote to PROTOCOL on
   first external-consumer demand.
2. `stats` default `--since`: 30 days; `--since all` available.
3. Bisect `--test`: no sandbox — user's own command on their own machine,
   documented explicitly.
4. Decision 5 (alerts surface, pull-only, no push channel): founder-confirmed.
