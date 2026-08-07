# Phase 3.0 — read-only wedge implementation plan (stats, search, annotate, bisect)

> Plan gate: round 1 GATE FAIL (P1–P5: rework-window channel, SearchPage
> wrapper, undo_unevaluable_c definition, working-tree-hash seam, frozen-clone
> exit protocol), round 2 **GATE PASS** (Fable, 2026-08-07, @ 986bf94).

> **For agentic workers:** execute task-by-task with fresh-context executors
> (subagent-driven). Each task stands alone; do not read other tasks' internals
> beyond the Interface blocks. Skeptic gate per task AC set; plan-exit gate at end.

**Goal:** four read verbs over the existing ledger — `stats`, `search`,
`annotate`, `bisect` — per the gated spec
`docs/superpowers/specs/2026-08-07-phase-3-design.md` §3.0 (SKEPTIC GATE PASS
round 3; founder questions resolved: stats default `--since 30d`, no bisect
sandbox).

**Architecture:** every feature is a read-only fold over `log.jsonl` + CAS
plus — for stats only — current working-tree content hashes (the normative
`excluded_unknown_mtime` bucket and the human-share predicate both compare
live file hashes against recorded `after` hashes; a log-only fold cannot
compute them). All ledger access through `agentrec-core`'s `RepositoryView`
(view.rs). New core logic lives in `agentrec-core` (new modules beside
`view.rs`); CLI verbs are thin adapters in `cli/src`, mirroring the P5
pattern (typed core value → text + `--json` serializer). Zero new write
paths; zero PROTOCOL changes in 3.0 (the `reverts` field is 3.1 scope —
rework clause (c) structurally cannot fire in 3.0; see the
`undo_unevaluable_c` disclosure figure in T1).

**Decisions log (founder-confirmed, not re-litigable):**
1. Output surface CLI-only, text + `--json`. No network.
2. `stats` default window 30 days; `--since all` supported.
3. Bisect `--test` unsandboxed, documented.
4. Rework-rate definition is normative as written in spec §3.0.1 — deviations
   are defects, not judgment calls.
5. Annotate precision contract: file-and-time certain, line-level best-effort,
   disclaimer mandatory in all output modes.

**Infeasible/rejected (verified against code — do not resurrect):**
- New direct `load_log` callers in `cli/src` — the P4 seam; go through
  `RepositoryView` (existing residual callers are recorded debt, not precedent).
- Rework clause (c) via `prompt_excerpt` parsing — banned by spec; the
  structured `reverts` field does not exist until 3.1.
- Abort-on-any-refusal semantics anywhere — Phase F shipped per-file
  Revert/Refused/Excluded; 3.0 touches no undo paths at all.
- Gap-fatal bisect probes — spec round-2 correction; gaps are tolerated with a
  caveat line.

**Global constraints (apply to every task):**
- Suite baseline at plan start: measure with
  `cargo test --workspace -- --test-threads=3` before T0 and record in the
  ledger; every task's verification includes no regression of that figure.
- clippy `--all-features -D warnings` + `cargo fmt --check`, debug AND release.
- fsguard: all file opens via the guarded helpers (clippy.toml
  `disallowed-methods` enforces; do not add `#[allow]` without a ledger row).
- No test seams in release binaries (`strings` check where a seam is added).
- Goldens: human-form goldens byte-stable once landed; `--json` additive-only.
- Analytics figures: hand-computed fixture corpus — the test file computes
  expected numbers BY HAND in comments, never by running the same code path
  twice (self-checking fixture, defends the signature-defect class).
- Merge order: T0 → T1 → T2 → T3 → T4 → T5 → exit, SERIALIZED — T3/T4/T5
  all modify `main.rs` (clap enum + dispatch), so worktree parallelism
  guarantees conflicts there; do not parallelize.
- Task commit discipline: T3/T4/T5 each land TWO commits — (1) core module
  (+ view.rs method + lib.rs export), (2) CLI adapter (+ main.rs + goldens +
  integration tests). Reviewer gates each commit; keeps every reviewed unit
  within the ≤2–3-file house rule.
- Plan-start endpoints recorded at T0 (same ledger row as the suite
  baseline): plan-start commit hash (for exit item 6's `git diff --stat`)
  and `rg 'load_log' cli/src` count (for exit item 5).

---

## Task 0 — Spike/gate: rework-rate viability on the real corpus

**Files:** Create: `scripts/p30_rework_spike.py` (throwaway, committed for
provenance); Create: `docs/verify/p30-rework-spike.md`.

**Why first:** the skeptic's round-1 B4(e) warning — on import-heavy repos the
exclusion buckets may empty the headline metric. If the metric is near-empty on
the dogfood repo (the only real corpus), the stats task must lead with a
different headline (churn/share) and demote rework to a footnote. Measure
before building.

**Work:** a standalone script folding the dogfood `~/Projects/agentrec`
`.agentrec/log.jsonl` implementing spec §3.0.1's event-level definition
(denominator events, buckets: `censored_recent`, `excluded_imported`,
`gap_overlapped` disclosure; numerator clauses (a) and (b) only — (c) is
structurally 0 pre-3.1). `excluded_unknown_mtime` requires live working-tree hashing, which
this log-only spike does NOT do: the spike prints that bucket as literally
`not computed by this spike`, never 0. Report all bucket counts + measurable
count + rate.

**Exit criteria (gate, binding):**
- Bucket counts + rate recorded in `docs/verify/p30-rework-spike.md` with the
  exact command.
- Decision recorded: measurable-events ≥ 20 → rework stays headline; < 20 →
  plan amendment demoting it (founder pinged, not silently decided).
- Downstream contract confirmed or corrected: every record field the fold
  needed exists as spec §3.0.1 assumes (grade, tool, model, ended, files ops,
  imported flag, epoch records for gaps). Any mismatch = spec amendment before
  T1, re-gated.

**Verification:** `python3 scripts/p30_rework_spike.py ~/Projects/agentrec/.agentrec/log.jsonl`
prints the bucket table; doc committed. Commit:
`spike(p3.0): rework-rate viability measured on dogfood corpus`.

---

## Task 1 — Core stats fold: `view::stats`

**Files:** Create: `agentrec-core/src/stats.rs`; Modify:
`agentrec-core/src/view.rs` (method delegating to stats module),
`agentrec-core/src/lib.rs` (export).

**Interfaces:**
- Consumes: `RepositoryView::list_records_of` / ledger iteration (existing,
  view.rs), `view::recording_gaps` (existing — gap windows),
  `BlobStore::size(hash)` (existing, used by retention.rs::plan_eviction),
  **current working-tree content hashing** — the seam
  `undo_coordinator::read_current_hash` (undo_coordinator.rs) is the
  existing helper; reuse or extract it, do NOT re-implement hashing.
  `excluded_unknown_mtime` and the human-share predicate are uncomputable
  without disk reads; a log-only implementation is a decision-4 defect.
- Produces (T2 relies on these exact names):
  `pub struct StatsOptions { pub since: Option<Duration>, pub rework_window_days: u32 }`
  (default window 7 — carries T2's `--rework-window` flag; P1 fix);
  `pub struct StatsResult { pub window: StatsWindow, pub turns: TurnCounts,
  pub files: Vec<FileChurn>, pub share: ChangeShare, pub rework: ReworkRate }`,
  all `Serialize`; `RepositoryView::stats(&self, opts: &StatsOptions) ->
  Result<StatsResult, StatsError>`. `ReworkRate` carries
  `{ measurable, reworked, rate: Option<f64>, censored_recent,
  excluded_imported, gap_overlapped, excluded_unknown_mtime, undo_unevaluable_c }`
  — `gap_overlapped` is a DISCLOSURE count, not an exclusion (T0 amendment,
  founder-ruled: gap-overlapped windows stay measurable; rate labeled
  **approximate lower bound** — two disclosed channels, spec §3.0.1 — in
  both text and `--json` via a string-typed key `rate_bound: "approx_lower"`;
  string not boolean so a future measured direction change is an additive
  value, not a key break)
  — `rate: None` when `measurable == 0` (renders "no measurable events").
  **`undo_unevaluable_c` definition (P3 fix):** count of `tool:"agentrec"`
  undo turns whose `ended` falls inside any denominator event's window but
  which carry no `reverts` field (all pre-3.1 undo turns) — clause (c)
  cannot be evaluated against them. Disclosure-only: it does NOT shrink
  `measurable` and does NOT add to `reworked` (the spec keeps such events
  measurable; (c) simply cannot fire). Nonzero on real corpora today.
  `ChangeShare` buckets: `agent`, `human`, `unattributable`, `imported`
  (spec: git-tool and imported turns never in `agent`).
  `FileChurn` carries `{ path, churn_bytes, dangling_refs, skipped_entries,
  withheld_entries }` — the last two feed the spec's "footnoted as
  undercounted" honesty rule (A1 fix).

**Test-first (acceptance):** unit tests in `stats.rs` against a hand-computed
fixture — a ledger PLUS on-disk working-tree state in a tempdir (the
`excluded_unknown_mtime` and human-share cases are undecidable from the
ledger alone; the fixture writes real files whose hashes match or diverge
from recorded `after` hashes by construction). Required cases, each its own
test, expected numbers hand-derived in comments:
- rich/bare/git/imported/undo turn mix → across the test SET each exclusion
  bucket hits at least once; denominator/numerator counted by hand.
- right-censoring boundary: event exactly at N days is IN denominator; N-ε
  younger is `censored_recent` (pin the ≥ comparison).
- deletion-by-uncovered-change counts as rework (clause b).
- gap overlapping one event's window → event STAYS measurable,
  `gap_overlapped` incremented, result labeled approximate lower bound
  (T0 amendment; `rate_bound: "approx_lower"` asserted in the json case).
- zero-denominator ledger → `rate: None`.
- on-disk file whose current hash ≠ last recorded `after`, no subsequent
  turn, no recorded gap → `excluded_unknown_mtime` (fixture writes the
  divergent bytes to disk).
- pre-`reverts` undo turn inside a denominator window → `undo_unevaluable_c`
  incremented; `measurable` and `reworked` unchanged (both asserted).
- churn: dangling blob ref → counted in `dangling_refs`, contributes 0 bytes.
- churn: `skipped`/`withheld` entries → contribute 0 bytes, counted in
  `skipped_entries`/`withheld_entries`.
- share: git turn's files land in neither `agent` nor `human`.
- share: bare turn's window lands in `unattributable`, never `agent` or
  `human` (spec sentence gets its test row — A7 fix).

```rust
// shape of the hand-computed assertions (executor writes real fixtures):
let s = view.stats(&StatsOptions { since: Some(days(30)), rework_window_days: 7 })?;
assert_eq!(s.rework.measurable, 3);      // hand count: events e1,e2,e5
assert_eq!(s.rework.reworked, 1);        // e2: deleted by uncovered change
assert_eq!(s.rework.excluded_imported, 2);
```

**Verification:** `cargo test -p agentrec-core stats` all green; suite no
regression; clippy+fmt. Commit: `feat(core): stats fold with normative
rework-rate and exclusion buckets`.

---

## Task 2 — CLI `agentrec stats`

**Files:** Modify: `cli/src/main.rs` (clap verb), `cli/src/cmds.rs` or new
`cli/src/statscmd.rs` (follow the thinnest existing read-verb pattern —
`status` adapter at `cmds.rs::status`, `diff` adapter in `readcmds.rs`); Create: goldens under the existing golden
test layout; Test: `cli/tests/` new integration file `stats.rs`.

**Interfaces:** Consumes T1's `RepositoryView::stats(&StatsOptions)` +
`StatsResult` (Serialize) — `--rework-window` maps to
`StatsOptions.rework_window_days`, `--since` to `StatsOptions.since`
(`all` → `None`). Produces: `agentrec stats [--since <dur|all>]
[--rework-window <days>] [--json]`; default since = 30d (founder decision).

**Acceptance:**
- Text output prints every figure WITH its exclusion counts beside it (spec
  honesty rule); zero-measurable renders literally `no measurable events`
  (golden-pinned).
- `--json` byte-equal to serializing `StatsResult` (parity test, same pattern
  as diff/blame parity pins).
- Read-only: parity test asserts zero writes under `.agentrec/` across the
  invocation (mirror of the P5 status zero-write parity test).
- Golden for text on the hand-computed fixture; `--json` golden additive-only
  rule noted in test comment.
- `--since all` covers full history; bad duration string → clap-level error.

**Verification:** `cargo test --test stats`; full suite; clippy+fmt debug+release.
Commit: `feat(cli): stats verb — text + --json over view::stats`.

---

## Task 3 — Search: core fold + CLI verb

**Files:** Create: `agentrec-core/src/search.rs`; Modify: view.rs (method),
lib.rs; Modify: `cli/src/main.rs` + thin adapter; Test: core unit +
`cli/tests/search.rs`.

**Interfaces:**
- Consumes: ledger iteration + `Page`/`Cursor` machinery (view.rs — occurrence
  ordinals, P4b), CAS blob read for `prompt_ref` (existing store read used by
  show/fmt paths).
- Produces: `RepositoryView::search(&self, q: &SearchQuery, cursor:
  Option<Cursor>) -> Result<SearchPage, SearchError>`;
  `SearchQuery { pattern: String, regex: bool }`;
  `SearchHit { turn_id, ts, field: MatchedField, snippet }`,
  `MatchedField ∈ {Prompt, Tool, Model, Path}`;
  **`SearchPage { page: Page<SearchHit>, dangling_prompt_refs: u64 }`** —
  a wrapper, NOT a field added to the shared generic `Page<T>` (that would
  additively change every existing `Page` serialization — log/recall
  `--json` goldens; P2 fix, decided here not by the executor).

**Acceptance (each a test):**
- substring match across prompt text (read from CAS via prompt_ref), tool,
  model, path; one fixture per field.
- `--regex` invalid pattern → typed error, not panic.
- dangling `prompt_ref` → hit skipped for prompt field, counted in
  `dangling_prompt_refs`, other fields still searchable; never a crash.
- same-id duplicate turns: both occurrences returned; cursor resumes across
  the duplicate boundary without re-delivery (reuse the P4b two-records-one-id
  fixture pattern in view.rs tests).
- file-snapshot blobs never read (test: fixture with searchable content ONLY
  inside a file blob → zero hits).
- CLI: `agentrec search <q> [--regex] [--json]`, `--json` parity with
  `SearchPage`, zero-write assertion.
- `--content` flag exists in clap and errors `content search not implemented`
  (spec-reserved slot; test pins the error, guarding against silent
  degradation to metadata search).

**Verification:** `cargo test -p agentrec-core search && cargo test --test search`;
suite; clippy+fmt. Commit: `feat: search verb over prompts and turn metadata`.

---

## Task 4 — Annotate: blame join + CLI verb

**Files:** Create: `agentrec-core/src/annotate.rs` (line mapping from turn
before/after blob diffs); Create: `cli/src/annotatecmd.rs` (git blame
--porcelain invocation + join + render); Modify: main.rs; Test: core unit +
`cli/tests/annotate.rs` (integration drives the real binary inside a scripted
git fixture repo).

**Interfaces:**
- Consumes: turn records + CAS blobs via view; `git blame --porcelain`
  (spawned by CLI layer only — core stays git-free, mirroring how the daemon
  classifies git turns without shelling out).
- Produces: `agentrec annotate <git-range> [--json|--md]`;
  `AnnotateResult { files: Vec<AnnotatedFile>, line_precision: &'static str
  ("best_effort"), evicted_ranges: u64 }`; per-range attribution
  `∈ {Turns(Vec<TurnRef>), Human, Unattributable{reason}}` where
  `TurnRef { id, tool, model, ts, prompt_excerpt }` — a Vec because multiple
  turns can overlap one blame range (spec says "which turn(s)"; A2 fix);
  single-turn is a one-element Vec, render collapses it.

**Acceptance:**
- disclaimer line present in text AND `--md`; `"line_precision":
  "best_effort"` key in `--json` (golden-pinned all three).
- misattribution-shaped fixture (spec risk 2): human+agent interleaved edits
  in one file between commits → test asserts output still renders and carries
  the disclaimer; NO assertion that lines are correctly attributed (that would
  overclaim what best-effort promises).
- gap fixture: blamed range in an uncovered window → `Unattributable{gap}`,
  never guessed.
- evicted-blob fixture: turn's blob purged → range renders
  `unattributable (blob evicted)`, `evicted_ranges` counted.
- range with zero agentrec history → all-Human output, exit 0.
- not-a-git-repo / bad range → clean typed error from the git spawn, no panic.

**Verification:** `cargo test --test annotate`; suite; clippy+fmt.
Commit: `feat: annotate — git blame join with best-effort line precision`.

---

## Task 5 — Bisect: reverse-apply walk + CLI verb

**Files:** Create: `agentrec-core/src/bisect.rs` (probe-state computation:
which files/bytes for a given probe point — pure, no I/O beyond store reads);
Create: `cli/src/bisectcmd.rs` (scratch dir materialization under the system
temp root, `--test` spawn, binary search driver, `--keep`); Modify: main.rs;
Test: core unit + `cli/tests/bisect.rs`.

**Interfaces:**
- Consumes: view ledger iteration; blob reads; ambiguity resolution path used
  by show/diff (`view::resolve_turn` — ambiguous id must hard-error).
- Produces: `agentrec bisect --test <cmd> [--good <id>] [--bad <id>]
  [--include-bare] [--flaky-retries N] [--keep] [--json]`; report
  `BisectResult { verdict: FirstBad{turn_id} | AmbiguousSpan{ids, reason},
  unanswerable: Vec<{turn_id, reason}>, gap_windows: Vec<GapWindow>, probes: u64 }`.

**Acceptance:**
- core walk unit tests (pure, no process spawn): latest-first reverse-apply of
  `before` bytes; create op → file absent in probe state; delete op → file
  recreated; file touched by two later turns → probe state carries the
  EARLIEST post-probe turn's `before` (hand-built 3-turn fixture pins order).
- membership: bare + imported turns absent from default sequence;
  `--include-bare` includes as subtraction AND probe candidates.
- unanswerable: skipped/withheld/dangling `before` → probe reported
  unanswerable; `after_synthesized`-derived → same; walk returns narrowest
  answerable span.
- gaps tolerated: fixture with a mid-span epoch restart → probes still answer,
  `gap_windows` listed in report (spec round-2 correction pinned by test).
- integration: scripted fixture repo + failing `--test` script → binary search
  lands on the hand-known first-bad turn; working tree + `.agentrec/`
  byte-identical before/after (hash the trees — the read-only claim gets a
  measurement, not an assertion).
- flaky cmd fixture (`--flaky-retries 1`, disagreeing runs) → probe
  unanswerable, reported.
- ambiguous `--good` id (same-id duplicate fixture) → hard `ambiguous turn id`
  error, exit ≠ 0.
- `--help` carries the fidelity sentence ("working tree minus later agent
  turns; not a historical snapshot") and the no-sandbox statement — doc
  assertions in the integration test.

**Verification:** `cargo test -p agentrec-core bisect && cargo test --test bisect`;
suite; clippy+fmt debug+release. Commit:
`feat: bisect — first-bad-turn search over scratch-materialized probe states`.

---

## Plan exit (all must hold; then Fable skeptic gate on the sub-phase)

1. Suite green, zero regression from the T0-recorded baseline; figure recorded.
2. clippy `--all-features -D warnings` + fmt, debug AND release.
3. All four verbs run against a **FROZEN CLONE** of the dogfood repo
   (`cp -R` of `~/Projects/agentrec` incl. `.agentrec/`, no daemon attached —
   the live repo has a running writer, pid-checked at plan time, so hashing
   the live `.agentrec/` false-fails on daemon ticks; frozen-clone is the
   P4b E2E precedent). Commands + outputs into `docs/verify/p30-exit.md`;
   clone's `.agentrec/` hashed before/after each verb — byte-identical
   (read-only proven on the real corpus, not fixtures).
4. `stats` rework headline decision from T0 honored (headline or demoted).
5. No new `load_log` callers in `cli/src` (`rg 'load_log' cli/src` count
   unchanged from plan start; record both counts).
6. PROTOCOL.md untouched this sub-phase (`git diff --stat` proof).
7. Fable skeptic gate over the whole sub-phase: GATE PASS required before 3.1
   planning. Skeptic must re-run at least the T5 tree-hash proof and one
   hand-computed stats fixture independently.

## Manual E2E script (tail)

1. `agentrec stats` on dogfood → figures with exclusion counts, no crash.
2. `agentrec search "undo"` → hits with snippets; page through with cursor.
3. `agentrec annotate HEAD~5..HEAD` on dogfood → renders, disclaimer present.
4. `agentrec bisect --test 'cargo check -q'` on a scratch clone with a known
   bad turn → names it; `--keep` scratch dir inspectable.
