# Phase 3 design — leverage the ledger (analytics, search, annotate, bisect, undo power, digest, policy signals)

Date: 2026-08-07. Status: DRAFT — pending skeptic gate + founder approval.
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
- Agent-vs-human share of change: agent = union of rich-turn coverage;
  human = `human-edited-since` windows; **recording gaps render as a third
  bucket, "unattributable" — never allocated to either side.** Bare turns are
  unattributable by definition (never rendered as agent activity).
- **Rework rate** (the headline metric), defined precisely:
  numerator = files written by a rich agent turn T that are subsequently
  modified within N days (default 7, `--rework-window`) by a change NOT covered
  by any rich turn (i.e. `human-edited-since` fires for that span);
  denominator = all files written by rich agent turns at least N days before
  the query time (files younger than N days are excluded from BOTH sides —
  right-censoring, not silent inclusion). Bare turns count in neither side.
  Undo turns (`tool: "agentrec"`) count as agent coverage, not as rework.
  If a recording gap overlaps a file's window, that file is excluded and
  counted in an `excluded_gap` figure the output must print.

Honesty rules: every figure that has an exclusion (gaps, skipped, withheld,
right-censored) must print its exclusion count next to it. No figure may be
rendered without its denominator.

### 3.0.2 `agentrec search <query> [--regex] [--json]`

Substring (default) or regex match over: stored prompt text (scrubbed form —
the only form persisted), turn metadata (tool, model), and turn file paths.
Returns turn ids + matched-field snippet + timestamp. Paged with the existing
`Page<T>`/cursor machinery (occurrence-ordinal cursors — same-id duplicate
turns are a documented reality).

Non-goal: content search over CAS blobs (cost/scope; revisit on demand — a
future `--content` flag slot is reserved in the interface but MUST error
"not implemented" if passed, not silently degrade to metadata search).

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
key in JSON. Any wording implying line-exactness is a defect.

Output is local only. Posting to a PR is the user's/CI's job.

### 3.0.4 `agentrec bisect --test <cmd> [--good <turn>] [--bad <turn>]`

Binary search over the turn sequence for the first turn where `<cmd>` fails.

- Materializes each probe state into a **scratch directory** (never the working
  tree): start from current working tree copy, then for each turn after the
  probe point, restore touched files to their state as of the probe turn's
  `after` snapshots, walking the ledger. Files without snapshots at the needed
  point (skipped/withheld/gap) make that probe **unanswerable**: bisect reports
  the ambiguous span instead of guessing (mirror of blame's gap honesty).
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

### 3.1.1 File-level partial undo — `agentrec undo <turn> --file <path> ...`

- Reverts only the named files from the turn's snapshot set; other files in
  the turn untouched. Unknown path (not in turn) → hard error listing the
  turn's files, before any write.
- Invariants preserved (not amended): undo-is-a-turn — the revert appends a
  new turn (`tool: "agentrec"`) recording exactly the reverted files;
  `modified-since` gates per file. Default: any refused file aborts the whole
  operation before any write (no surprise partial results). Explicit
  `--continue-on-refusal` proceeds on the clean subset and lists refusals.
- `skipped`/`withheld` files remain non-revertible — unchanged rule.
- MCP: the existing destructive request payload gains an optional `files`
  array (additive, PROTOCOL §8). All existing gating (mcp_destructive modes,
  two-phase token, path reservations, `origin` discriminator) applies
  unchanged; no new mode, no new bypass.

### 3.1.2 Checkpoints — `agentrec checkpoint <name>` / `restore <name>`

- `checkpoint <name>`: appends a new record type `type:"checkpoint"` to
  `log.jsonl` (additive within PROTOCOL v1; unknown-type tolerance is already
  normative for consumers) pinning `{name, ts, files: [{path, hash}]}` for the
  **agent-touched set** (files appearing in any turn's entries whose current
  on-disk hash is computable). Explicit boundary: files never touched by any
  turn are OUT of checkpoint scope — git covers them; the spec bans marketing
  checkpoints as full-repo snapshots.
- Checkpoint records pin blobs: the eviction protect-set gains checkpoint-
  referenced hashes (same mechanism as open-turn/memory-pin protection).
  Unbounded pinning is real cost — `purge --checkpoints-expired` is NOT built
  in this phase; instead `checkpoint --delete <name>` appends a tombstone
  record (append-only discipline; the pair drops out of the protect set).
- `restore <name> [--dry-run]`: computes per-file revert plan (current hash vs
  pinned hash; identical → skip; pinned blob present in CAS → revert;
  modified-since semantics: restore IS a modification-tolerant operation by
  intent, but files whose current state is not covered by any turn since the
  checkpoint (human-edited) are refused by default, `--continue-on-refusal`
  as above). Executes as ONE undo turn listing all reverted files. Rides
  `UndoCoordinator` preview + failure-path persistence (Phase F, incl. the
  11(b) mid-revert fix). Missing CAS blob → refuse that file, list it, never
  silent-partial.
- Name rules: unique among live (non-tombstoned) checkpoints; collision →
  error. Daemon ignores checkpoint records entirely (no behavior change).

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
  no rewrite class, torn-tail tolerated).
- Surfacing: `status` prints open-alert count; `digest` includes open alerts;
  new `agentrec alerts [--json]` lists, `alerts --ack <id>` appends an ack
  record (never rewrites).
- Non-goals with reasons: no push channel, no exec-on-alert, no webhooks —
  push is gatekeeper-adjacent posture creep and no daemon→user channel
  exists; adding one is a separate founder decision.

## Cross-cutting rules (whole phase)

- **PROTOCOL v1 is frozen; every change here is additive** and lands with
  conformance fixtures in the same commit: checkpoint + tombstone record
  types, `alerts.jsonl` (documented as implementation file, not wire — decide
  at freeze-review whether it enters PROTOCOL at all), §8 rows for
  `agentrec_digest` and the `files` array on destructive requests.
- **Non-goals, recorded:** hunk-level undo (diff/merge machinery + conflict
  surface; file-level covers the common case); CAS content search; GitHub/
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
  default/continue-on-refusal). Real-corpus checks where shape claims are
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

## Open questions (founder)

1. Does `alerts.jsonl` enter PROTOCOL (wire) or stay implementation-private?
   Leaning: private in 3.2, promote on first external-consumer demand.
2. `stats` default `--since`: all-history or 30d? Leaning 30d (import
   backfill window symmetry), `--since all` available.
3. Bisect `--test` command execution: sandbox/no? Leaning: none — it is the
   user's own command on their own machine, same trust as running it by hand
   (documented, not silently assumed).
