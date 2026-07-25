# Plan: agentrec churn-honesty round — 9 phases, branch `fix/blame-attribution-and-noise-folding` (continue) → per-phase branches off it

Source scope: the 5-item handoff (diff fold · record-time ignore globs · doctor advisories +
presence check · `enforce_budget` protect-set · re-measure), refined by a 3-lens fable redteam
(honesty/attribution · daemon-crash-concurrency · vacuity/measurement/scope) run 2026-07-25.

Design record: `docs/superpowers/specs/2026-07-25-store-churn-designs.md` (measurements + §4 REFUTED).
Conflict order: PROTOCOL.md > IMPLEMENTATION.md > SPEC.md > this plan.

**Goal:** stop `agentrec` from lying in three newly-identified places (eviction deleting live-cited
blobs, `blame` filling config-excluded intervals with confident answers, `doctor` passing clean on a
log citing never-stored hashes), and make the churn story honest end-to-end — render-time folding
where history is immutable, record-time prevention where PROTOCOL already promised it, and a
stamped measurement that stops the four-mutually-inconsistent-figures pattern.

**Deviation from the ≤5-phase house norm, stated not hidden:** this is a 5-item round plus inherited
E2E debt, not one feature. Every phase still obeys ≤2–3 substantive files and independent
mergeability. Phases 1, 2, 3, 4 are mutually independent and may run in parallel worktrees.

## Baseline — **UNVERIFIED, re-run before Phase 1**

`cargo test --workspace -- --test-threads=3` → **386 passed, 0 failed, 1 ignored** at `c4ada8f`
— *stated from the prior round's receipt, NOT re-run when this plan was written.* Every phase below
asserts an absolute total derived from it, so if 386 is wrong, all nine are wrong and the first
executor's receipt will read as a regression. This repo has already recorded a test-count delta that
did not reconcile ("prior note said 343; the +1 arithmetic does not reconcile"). Re-run first, and
if the figure differs, correct every phase total in one edit before starting.
clippy `-D warnings` + `cargo fmt --check` clean, debug **and** release.

## Decisions log (executors may not re-litigate)

1. **Founder-confirmed:** noise folding extends to `diff` (reverses `cli/src/noise.rs`'s module-doc
   invariant "never `diff`/`blame`/`undo`" for `diff` only). `blame` and `undo` stay unfolded forever.
2. **Founder-confirmed:** config ignore globs are **declarative only**. No rate/heuristic demotion
   this round (that is designs-spec §3.3, a separate feature with its own disclosure obligations).
3. **Agent call (flag for founder, reversible):** `diff`'s header prints **both** numbers —
   `turn <id> · <tool> · 9602 files (9599 noise folded)`. Neither the raw total alone (arithmetic
   contradicts the visible lines) nor the folded count alone (contradicts the turn record and the
   future `diff --json`).
4. **Agent call:** `withheld: true` and `skipped: true` entries are **never folded**, even when they
   match a noise glob. `withheld` is the only per-turn evidence a secret-pattern file was touched;
   `skipped_reason` visibility is what change (B) just shipped. Measured cost of the exemption:
   45 `skipped` + 0 `withheld` of 8346 entries — it does not weaken the churn fold.
5. **Agent call:** eviction keeps **hard-delete**. Archive-on-evict frees zero disk (archives live
   under `.agentrec/` on the same filesystem) while reporting bytes freed, and eviction fires from
   `status` — a read verb run habitually — so it would grow an unbounded archive as a side effect of
   reading status. The deliverable is a **complete** protect-set, not a softer delete.
6. **Agent call:** no daemon-liveness refusal on eviction. The daemon runs 24/7 under launchd here,
   so a liveness gate makes the budget fiction and routes users to `purge --snapshots-before`, which
   hard-deletes legitimate history.
7. **Agent call:** config globs win over git-tracked status (explicit user config is not silently
   overridden), **but** `doctor` warns when an active glob matches tracked files. See Q1.
8. Folding lives in the CLI adapter only. It must never migrate into Phase 2.0's
   `RepositoryView::diff`, or `diff --json` and MCP `agentrec_diff` inherit it silently.

## Infeasible / rejected (killed against real code or measured evidence)

- Everything in designs-spec **§4**: no-op suppression in `Recorder::stage`, zstd inside
  `BlobStore`, reclaim-at-turn-close, and the tail-delta/first-obs-only priority ranking.
- **Crediting item 5 (record-time globs) with churn reduction.** Measured payoff on this corpus is
  **~0**: 0 churn entries since the D29 fix merged, and `.claims/` — the named next instance — has 3
  tracked files, so the rule keeps it. Justification is honoring PROTOCOL.md:25, which no code has
  ever read. Any receipt claiming store-growth reduction is rejected at the gate.
- **Crediting item 1 (diff fold) with byte or line reduction.** `log.jsonl` is append-only; folding
  is render-time. Its value is retrospective legibility over the 8346 already-appended entries.
- **Per-suppressed-event `skipped_reason: "policy"` entries** for config exclusion. It re-creates
  the line blast it exists to prevent (O(events) not O(config changes)), and §3.3 reserves `policy`
  for the rate-budget producer.
- **Store-probe-dependent folding** (deciding fold by whether a blob resolves): `store.get` is a
  full read + rehash; 9602 of them per header, and output becomes nondeterministic across purges.
- **Retro-hiding already-recorded entries when a glob is added.** Append-only. Prevention and
  folding are permanently separate keys (`ignore_globs` vs `noise_globs`).
- **`purge --paths`** (unchanged from prior rounds): path attribution requires parsing, which is
  exactly what `--orphans`' torn-line safety refuses.

## Global constraints / invariants

- Append-only: `log.jsonl` / `signal.jsonl` are never rewritten by any phase here.
- Every write site goes through `agentrec-core/src/perms.rs` (0700 dirs / 0600 files).
- No network code. No new crates.
- `cargo clippy -- -D warnings` + `cargo fmt --check` clean on debug **and** release. Any test-only
  env seam is `#[cfg(debug_assertions)]`-gated; release `strings` must not contain it.
- Integration tests run `--test-threads=3` (FSEvents contention).
- **Config parsing fails toward the safe direction, and the direction differs per key:**
  `noise_globs` (display) degrades to *no folding*; `ignore_globs` (record) degrades to
  *record everything*. `cmds::config_values` is a hand-rolled line scanner that splits on `#`
  before parsing and does not understand multi-line arrays — both keys inherit that, both must
  pin the degrade direction with a test.
- Every renderer string that appears in two places goes through `fmt::` (D-PD6).

## Executor protocol

One phase per commit, per-phase branch off `fix/blame-attribution-and-noise-folding`. Use
`/surgeon` per chunked task. **claimd declare-first, one claim per acceptance criterion, BEFORE
implementing** — last round skipped this and four coverage findings are still standing; retroactive
declaration is forbidden by the skill. After each phase: `skeptical-reviewer` gates against that
phase's criteria **in an isolated git worktree** (never the shared tree — a prior reviewer
`git reset` clobbered a commit). Every criterion below names its **neuter**: the cheapest deletion
or short-circuit that must turn a **named** test RED. A criterion whose neuter leaves every test
green is vacuous and is rejected, not renegotiated.

Merge order: 1 → 2 → 3 → 4 → 5 → 6 → 7 → (8) → 9. Phases 1–4 are independent of each other.
Phase 6 depends on 5. Phase 9 depends on 5 (its rate figures are stale-by-construction otherwise).

---

## Phase 1 — Stamped baseline measurement (docs only, no code)

**Description:** Four mutually inconsistent store figures circulate in this repo's own notes
(12 MB/399 blobs; 285 blobs/2799 referenced; 784.8 MiB total/767.7 churn; 789.3→798.6 measured
while the daemon was live and growing it). Produce one stamped baseline that reconciles them and
that every later phase cites. **Hazard, first line of the doc:** `agentrec status` **evicts blobs
as a side effect** on an over-budget store (`cli/src/cmds.rs` → `retention::enforce_budget`) — the
measurement must not run it.

**Files:** `docs/2026-07-26-store-measurement-baseline.md` (new),
`scripts/measure-store.sh` (new, read-only commands only).

**Changes:**
- Read-only harvest: `log.jsonl` lines/bytes, turn/epoch split, FileEntry count, distinct paths,
  blob-ref slots, distinct refs (raw `sha256:` scan matching `purgecmd::harvest_refs` semantics);
  `objects/` file count + bytes; each `objects.archived.*` dir separately; **`signal.jsonl` bytes +
  lines** (2.4 MB / 487 lines, larger than the live object store, counted by no prior note);
  `state.json`; churn-vs-source split by path class; grade counts (bare/rich) as the pre-glob
  rich-rate baseline.
- Mandatory stamp on the doc and on every figure quoted out of it:
  `as-of <UTC> · HEAD <sha> · daemon <stopped | live, size-delta N> · method <exact command> ·
  population <what was and was not counted>`.
- Script runs from and writes to the scratchpad, never in-tree (a measurement script writing
  in-tree gets recorded by the daemon it is measuring).
- Explicit reconciliation paragraph: why 12 MB/399, 285/2799 and 784.8 MiB differ (different dates
  straddling a purge, different populations) — otherwise this becomes the fifth inconsistent figure.
- Survivor-biased figures (compressibility, dedup ratios, the 58x tail-delta) carry the house-style
  ban sentence *adjacent to the number*, per the "do NOT quote the 92.3% figure" precedent.
- "Store" means `objects/` live bytes. Archives and `signal.jsonl` are never summed into it.

**Acceptance criteria:**
- [ ] Doc contains no bare number: every figure carries as-of, method, and population inline.
      Neuter: n/a (prose) — enforced at the gate review, not by a test. Marked as such.
- [ ] Script exits non-zero (or refuses with a loud annotation) if `daemon_is_running(root)` or if
      `objects/` byte count differs between start and end of the run. Neuter: delete the liveness
      guard → `measure_store_refuses_live_daemon` RED.
- [ ] Script never invokes `agentrec status`, `purge`, or any mutating verb — asserted by grepping
      the script itself in the test. Neuter: add `status` to the script → RED.
- [ ] Referenced/orphan split equals `purgecmd::orphan_bytes` on a seeded fixture including one
      **torn** log line citing a real blob. Neuter: switch the script's harvest to a JSON parse →
      `measure_matches_orphan_bytes_with_torn_line` RED.

**Expected test outputs:** `cargo test -p agentrec measure::` → 3 passed (new). Workspace →
**389 passed, 0 failed, 1 ignored**. Manual: script output pasted verbatim into the doc.

---

## Phase 2 — Owed live-daemon E2E (over-cap → ghost-hash chain) + reusable harness

**Description:** Inherited debt from the branch under review: every BL/SR/NF test seeds `log.jsonl`
or calls `Recorder::stage` in-process; the over-cap→ghost-hash chain is *inferred*, never observed.
This repo has shipped two green gitignore tests alongside a 764 MiB live leak. This phase pays the
debt and builds the harness helpers phases 5 and 3 reuse — which is why it comes before them.

**Files:** `cli/tests/e2e_daemon.rs` (new), `cli/tests/integration.rs` (edit — extract/expose
`spawn_record`/`wait_for_live_daemon`-style helpers).

**Changes:**
- Harness: tempdir + **real `git init`** (mandatory — `ignore::WalkBuilder::require_git` defaults
  true, and a non-git fixture silently applies no ignore rules; this exact trap made
  `nested_gitignore_precedence` pass vacuously for months), real `agentrec init && agentrec record`
  subprocess, spawn gate, and a **bounded deadline poll on `log.jsonl` line count** — never a blind
  sleep — with waits sized to debounce (1.5 s) + quiet window (10 s) + slack.
- E2E-1: write an 11 MiB file → wait → shrink below cap → wait → assert the two real `log.jsonl`
  entries carry `skipped_reason: "over_cap"` and `before: <ghost hash>`; then run `blame`, `diff`,
  `show`, `undo` and assert each degrades with its pinned wording and **`undo` does not delete the
  file**.
- Every E2E asserts a **positive control** in the same window (a normal file that must appear), so
  "absent" can never mean "daemon dead" — the exact gap the 2026-07-25 deploy proof named.
- Delete the known-dead `!stdout.contains("REVERT  big.bin")` clause at `integration.rs` (renderer
  emits lowercase `revert`; the assert has never been able to fail).

**Acceptance criteria:**
- [ ] The over-cap entry observed from a **live daemon** carries `skipped_reason: "over_cap"` and a
      non-null `before`. Neuter: revert `Recorder::resolve`'s over-cap arm to `after: None` →
      `e2e_over_cap_records_ghost_hash` RED.
- [ ] `undo` of that turn refuses the over-cap file and leaves it on disk (`.exists()` asserted).
      Neuter: delete the `skipped` gate in `build_plan` → `e2e_undo_refuses_over_cap_file` RED
      (this is the round-1 GATE FAIL case; the fixture must use the reachable `op: "create"` shape,
      not `op: "modify"`, which an independent before-hash branch refuses anyway).
- [ ] `blame`/`diff` on the ghost hash degrade honestly and never attribute. Neuter: remove the
      poisoning rule in `blame`'s candidate walk → `e2e_blame_does_not_attribute_ghost` RED.
- [ ] Positive control present in every E2E assertion window. Neuter: kill the daemon before the
      write → the control assert fails, proving absence-because-dead is caught.

**Expected test outputs:** `cargo test -p agentrec --test e2e_daemon -- --test-threads=3` →
4 passed (new). Workspace → **393 passed, 0 failed, 1 ignored**.

---

## Phase 3 — `enforce_budget` protect-set (fixes live data loss)

**Description:** The only item that deletes user data **today**. `retention::enforce_budget` builds
its keep-set from **parsed** `TurnRecord`s and hard-deletes (`store.remove` → `fs::remove_file`),
while `purgecmd::referenced_hashes` — the raw, deliberately non-parsing `sha256:` byte-scan over
`log.jsonl` + `open.json` + `memory.jsonl` — is the repo's own stated safety standard. Eviction
protects neither in-flight (`open.json`) nor pinned (`memory.jsonl`) refs, and drops torn-line refs
entirely. Its sole caller is `cmds::status_report` — a read verb with no liveness refusal.

**Files:** `agentrec-core/src/retention.rs` (edit), `cli/src/cmds.rs` (edit),
`cli/src/purgecmd.rs` (mech — widen `referenced_hashes` visibility).

**Changes:**
- `enforce_budget` gains `extra_protected: &HashSet<String>` and subtracts it from `candidates`
  immediately before the remove loop. Core stays dependency-free; the CLI caller supplies the set.
- `cmds::status_report` harvests via `purgecmd::referenced_hashes(root)` **after** candidate
  computation, as late as possible before the remove loop, to narrow the live-daemon window. The
  ordering is load-bearing and gets a SAFETY comment in the style of `purgecmd`'s.
- Over-budget status message gains a **protected-bytes attribution clause** (pinned/in-flight kept)
  alongside the existing orphan attribution — protecting more shrinks `evicted.bytes`, and
  "over budget, 0 freed" with no explanation is the exact dishonest-status class fixed 2026-07-17.
- Persist eviction counters to `state.json` (`evicted_count`, `last_eviction_ts`) — the only future
  discriminator between "purged by design" and "never landed", which Phase 8 needs.
- Documented residual (narrowed, not closed): a `before`-hash blob cited only by an open turn's
  first observation is never re-`put`, so the A3(c) mtime guard cannot rescue it; `sync_journal`
  silently returns on write failure and can leave `open.json` stale. Same posture as
  `purge --orphans`' undo race.

**Acceptance criteria:**
- [ ] A blob cited only by `open.json` and one evictable old turn survives an over-budget pass.
      Fixture **must be RED pre-fix**: the blob is in no kept turn and its mtime is safely older
      than `pass_start`, or the A3(c) freshness guard rescues it vacuously (the SR6 vacuity class).
      Neuter: drop `open.json` from the harvest → `eviction_keeps_open_turn_blob` RED.
- [ ] A blob cited only by a `memory.jsonl` pin survives, and `verify`'s pin-diff still renders
      after the pass. Neuter: drop `memory.jsonl` → `eviction_keeps_pinned_blob` RED.
- [ ] A blob cited **only by a torn line** (truncated mid-JSON, valid `sha256:` inside) in
      `log.jsonl`, and separately in a truncated `open.json`, survives. Neuter: build the protect-set
      with `load_log`/serde instead of the raw scan → `eviction_keeps_torn_line_refs` RED. This is
      the criterion that actually lifts eviction to purge's guarantee.
- [ ] Call-site wiring proven, not just the core fn: an integration-level `status` run on an
      over-budget store with an open turn keeps the blob. Neuter: pass an empty set at the call site
      → `status_eviction_keeps_open_turn_blob` RED while the core unit test stays green (this is the
      anti-vacuity keystone — item 3 has two seams).
- [ ] Over-budget `status` with every candidate protected explains why 0 was freed, naming
      pinned/in-flight. Neuter: delete the attribution clause → `status_attributes_protected_bytes`
      RED.
- [ ] Existing A2/A3(c)/A5 retention tests and `status_prints_over_budget_notice` unchanged and
      un-weakened.

**Expected test outputs:** `cargo test -p agentrec-core retention::` → 11 passed (+5);
`cargo test -p agentrec --test integration status_` → +2. Workspace →
**400 passed, 0 failed, 1 ignored**.

---

## Phase 4 — Extend display-only noise folding to `diff`

**Description:** `diff`'s `print_entry` is the only renderer that prints one line per file path, so
it is the only surface where a 9602-entry churn turn actually blasts output. `log`/`show` render a
count and a fold line, which is why last round's folding made output one line *longer*. Render-time
only: zero effect on store bytes or log lines, and the receipt must say so.

**Files:** `cli/src/readcmds.rs` (edit — `diff`, `header_line`), `cli/src/fmt.rs` (edit — hoist the
fold-line constant, currently duplicated verbatim at `readcmds::show` and `cmds::log`),
`cli/src/noise.rs` (edit — `..`-traversal guard + correct the now-false module doc),
`cli/src/main.rs` (mech — `--all-files` on `diff`).

**Changes:**
- `diff` builds the matcher once (as `show` does), partitions `turn.files` before the render loop,
  prints visible entries then one fold line. Fold decision is glob-only and pure — never a store probe.
- `header_line` renders total **and** folded (decision 3).
- `withheld` / `skipped` entries are exempt from folding (decision 4).
- `noise::is_noise` rejects any path with a `..` component, mirroring its existing absolute-path
  rejection — `matched_path_or_any_parents` matches lexically, so `.remember/../src/auth.ts` folds
  today and would hide the one file a user needs to answer "who broke auth.ts".
- `noise.rs` module doc rewritten: the "never `diff`" invariant now holds for `blame`/`undo` only,
  and states that folding must stay in the CLI adapter (decision 8).

**Acceptance criteria:**
- [ ] Fixture precondition asserted first (NF2 precedent): `read_noise_globs` returns non-empty and
      `matcher.is_noise(<exact seeded FileEntry.path>)` is true — kills the multi-line-array and
      `#`-in-glob silent no-ops of `cmds::config_values`.
- [ ] Default `diff` on 1 noise + 1 source entry prints the source diff, prints the noise path
      nowhere, prints one fold line, and a header from which total and folded are both readable.
      Neuter: delete the `is_noise` filter → `diff_folds_noise_entries` RED.
- [ ] `--all-files` restores byte-identical pre-feature output **in the same fixture that proved
      folding engaged** (alone, this assertion survives every neuter — paths always print today).
      Neuter: short-circuit the flag → `diff_all_files_reveals` RED.
- [ ] A noise-matching `withheld` or `skipped` entry still prints its notice. Neuter: remove the
      exemption → `diff_never_folds_withheld_or_skipped` RED.
- [ ] `is_noise(".remember/../src/x.rs")` is false and does not panic. Neuter: delete the guard →
      `is_noise_rejects_parent_traversal` RED.
- [ ] Fold line is one constant: `diff`'s and `show`'s outputs are cross-asserted byte-equal on the
      same fixture. Neuter: change the string in one renderer → `fold_line_is_one_constant` RED.
- [ ] All-noise turn still prints header + fold line (the turn never disappears). Neuter:
      short-circuit on `visible == 0` → RED.
- [ ] PIN (guards the *next* implementer, no neuter): `blame` and `undo` output byte-unchanged with
      `noise_globs` covering their target path.

**Expected test outputs:** `cargo test -p agentrec --test integration diff_` → +5;
`cargo test -p agentrec noise::` → +1. Workspace → **406 passed, 0 failed, 1 ignored**.

---

## Phase 5 — Record-time `ignore_globs` (the daemon path; scariest unknown)

**Description:** PROTOCOL.md:25 promises `config.toml` "ignore globs" and **no code has ever read
them** (`ttl_days`, `memory_enabled`, `memory_inject_max`, `noise_globs` are the only keys read).
This is the one item that can silently turn the flight recorder **off** with every green light still
on, so it is risk-ordered here with a live-daemon exit criterion. **Measured churn payoff on this
corpus: ~0.** Justification is the protocol promise, nothing else.

**Files:** `cli/src/ignoreglobs.rs` (new — reader + matcher), `cli/src/daemon.rs` (edit — `classify`
consult, ingest-time dirty flag, per-tick rebuild), `cli/src/main.rs` (mech — `mod`).

**Changes:**
- Key `ignore_globs`, read through `cmds::config_values`. **Every parse failure degrades to
  "record it"** — the inverse of `noise.rs`'s degrade-to-no-folding, which is safe for display and
  catastrophic at record time.
- Consulted in `daemon::classify` as a separate matcher after the denylist — **not** merged into
  `IgnoreSet`, whose per-directory precedence would let a nested `.gitignore` `!pattern` silently
  override an explicit user exclusion. Classify-time is the correct point: `apply_watch_result` only
  arms timers for `Class::Watch`, so excluded paths never reset the debounce, never reach
  `Recorder::stage` (no blob is written), and never bump `last_change_at` — so no bare turn opens
  and the quiet window is untouched.
- **Refresh, and the latent defect it exposes:** the dirty flag is set at event ingest by filename
  (the `e453e86` pattern — the check must precede `classify`, since `.agentrec/*` always classifies
  `Ignore`; events for it *do* reach `apply_watch_result`, verified at `daemon.rs:375`). But the
  existing `.gitignore` rebuild sits **inside `if settled || capped`** (`daemon.rs:145`), and those
  derive from `last_event`/`first_event`, which only Watch-class events set. So with only-ignored
  activity the rebuild never fires — for `.gitignore` today that is fail-toward-stale-filter; for
  `ignore_globs` it means **removing a glob never resumes recording**. The rebuild moves to the
  loop tick, gated on the dirty flag, independent of pending/flush. The one-batch
  stale-classification window (classify runs at ingest, rebuild after) is inherent and gets
  documented, not papered over.
  **Pulled out and now owned by `docs/superpowers/plans/2026-07-25-agentrec-ignore-rebuild-gate.md`**
  (it is a defect in already-merged code and ships on its own). If that plan has landed, this phase
  reuses the corrected loop placement and adds only the `config.toml` trigger; if it has not, this
  phase must not re-fix it in parallel.
- Mid-turn glob change: entries already staged in the open turn (and in `open.json`) **persist
  unchanged at close**. Never re-filter in `Recorder::resolve`/`persist` — that orphans already-`put`
  blobs, makes the turn record lie by omission, and makes `recover_orphan` diverge from steady state.
- Startup disclosure: one stderr line naming the active globs.

**Acceptance criteria:**
- [ ] Malformed `ignore_globs` (unclosed bracket, bad glob, multi-line array) yields an **empty**
      set and `classify` returns `Watch`. Neuter: make the parser degrade to "ignore on error" →
      `ignore_globs_malformed_records_everything` RED. This is the fail-direction pin.
- [ ] `classify` returns `Ignore` for a globbed path **in a fixture with no `git init`**. Neuter:
      route matching through gitignore-aware machinery that no-ops outside a repo →
      `config_globs_apply_outside_git_repo` RED (direct inverse of the `require_git` vacuity).
- [ ] **Live-daemon E2E:** `ignore_globs = ["churn/**"]`; write `churn/a.log` **and** a positive
      control `src/ok.rs`; wait past debounce + quiet → exactly one turn, `files == ["src/ok.rs"]`,
      and `objects/` blob count unchanged by the churn write (proves classify-time, not stage-time,
      filtering). Neuter: delete the consult in `classify` → `e2e_daemon_honors_ignore_globs` RED.
- [ ] **Rebuild without a flush:** daemon idle; edit `config.toml` to add `churn2/**`; touch nothing
      else; write `churn2/b.log` → zero new turns. Then remove the glob, touch nothing else, write
      `churn/a.log` → one turn recorded. Neuter: gate the rebuild behind `settled || capped` (i.e.
      today's `.gitignore` behavior) → `e2e_glob_change_honored_without_other_activity` RED.
      **No seeded fixture can catch this** — it is E2E-only by construction.
- [ ] Open turn with staged entries under a path, glob added mid-turn, turn closes → persisted
      `TurnRecord` still carries the entries. Neuter: filter in `resolve` → `open_turn_entries_survive_glob_change` RED.
- [ ] Startup names the active globs on stderr. Neuter: delete the line → RED.
- [ ] `status` rich-rate warning threshold re-examined in the same commit (suppressed turns are
      predominantly bare, so the denominator moves; a rich-rate false alarm already cost this repo a
      diagnosis round). Recorded as a decision either way.

**Expected test outputs:** `cargo test -p agentrec ignoreglobs::` → 5 passed (new);
`cargo test -p agentrec --test e2e_daemon` → 7 passed (+3). Workspace →
**414 passed, 0 failed, 1 ignored**.

---

## Phase 6 — Ignore-glob honesty surface (blame/diff wording + persisted config record)

**Description:** Record-time exclusion is a **non-event**: nothing in `log.jsonl` marks it, the
epoch machinery reports clean coverage, and every existing `blame` branch confidently fills the
vacuum. Today, for an excluded path, `blame <path>:<line>` falls through to **"before recording
began"**, and a path recorded before the glob then edited after prints **"· human-edited since"** —
attributing an agent edit to the human. That is byte-for-byte the confident-false-answer class
commit `17657d9` just spent a gate cycle fixing. **This phase is not optional and must not be
deferred to a later round** — the feature "works" without it, which is exactly why it would be.

**Files:** `cli/src/readcmds.rs` (edit — `blame_file`, `blame_line`, `build_plan`'s
`modified_cause`), `cli/src/daemon.rs` (edit — persist the effective glob set), `PROTOCOL.md` (edit
— **substantive, not mech**: this adds a record type to a normative document about to freeze at 1.0,
so the phase is at its 3-file cap. If that cap binds, the record's *schema* lands in Phase 5's
commit and Phase 6 documents it — decide before chunking, do not let an executor discover it).

**Changes:**
- New vocabulary, distinct from every existing message: a currently-excluded path with no touching
  turn says `not recorded — matches ignore glob '<g>' (config.toml)`, never "no recorded turn
  touches" and never "before recording began". A touched-then-excluded path says
  `modified since — now matches ignore glob '<g>'; later changes were not recorded`, never
  `human-edited since`.
- Daemon start epoch carries the effective glob set; a config change appends an epoch-shaped record.
  Additive per PROTOCOL §10 (`count_gaps`/`has_gap_after`/`has_recording_gap` all have tolerant
  `_ => {}` arms), and **materially cheaper before the Phase 2.0 protocol-1.0 freeze**. Without it,
  the excluded interval is permanently unexplainable — a reader can never distinguish "excluded
  then" from "human edited it".
- Interval-aware blame (*was* this path excluded between turn-end and now) is explicitly **out of
  scope**; persisting the records is what keeps it possible later.

**Acceptance criteria:**
- [ ] `blame <excluded>` and `blame <excluded>:<line>` both print the excluded wording; neither
      output contains "before recording began" or "no recorded turn touches". Neuter: delete the
      glob consult in `blame_line` → `blame_line_names_ignore_glob` RED (the line-level branch is
      the load-bearing one — it is the confident lie).
- [ ] Seeded log where a turn touched `x`, `x` later matches a glob, `x` edited on disk: output
      does **not** contain "human-edited since". Neuter: remove the excluded-path override →
      `blame_does_not_blame_human_for_excluded_path` RED.
- [ ] Daemon start appends the glob set; a mid-run config edit appends a config record within one
      poll tick. Neuter: delete the persist call → `daemon_records_active_ignore_globs` RED.
- [ ] A pre-existing log with no config records still parses and renders unchanged (backward
      compatibility of the additive record). Neuter: make the reader require the field → RED.
- [ ] `undo`'s `modified_cause` no longer says "human or external edit" for a currently-excluded
      path. Neuter: remove the branch → RED.

**Expected test outputs:** `cargo test -p agentrec --test integration blame_` → +4;
`cargo test -p agentrec --test e2e_daemon` → +1. Workspace → **419 passed, 0 failed, 1 ignored**.

---

## Phase 7 — `doctor`: advisory severity + churn advisor + archive advisory + glob sanity

**Description:** `doctorcmd::Check` is Pass/Fail/Na today and `pass` carries no remedy, so there is
no shape for "worth telling you, not broken". `doctor` all-pass exit 0 is this repo's production
deploy check — an advisory that flips exit 1 breaks it on a store that is fine, and the operator's
rational response is to stop trusting doctor.

**Files:** `cli/src/doctorcmd.rs` (edit — `Check` gains an advisory variant + three checks),
`cli/tests/integration.rs` (edit — doctor tests).

**Changes:**
- Advisory variant; `Report.ok` stays "no Fail"; `--json` gains the new status string (doctor's JSON
  is operational state, deliberately off the wire per PROTOCOL §5 — say so in the check's doc).
- (a) **Churn advisor, windowed.** Top-N paths by entry count **within a trailing window**, remedy
  "add to `ignore_globs`", diffed against the currently active globs so it never recommends what is
  already configured. Unwindowed, it flags `.remember`'s 7659 historical entries forever on this
  very repo — permanently noisy doctor for a leak fixed weeks ago.
- (b) **Archive advisory:** name each `objects.archived.*` with size and the manual `rm` remedy,
  stating that removal destroys the recovery path. Never Fail — failing on the output of the
  archive-never-delete safety feature punishes correct use.
- (c) **Glob sanity:** N active globs, M currently-matching **git-tracked** files (tracked files
  matching an ignore glob is almost always a mistake), and a loud warning for a glob set matching
  the repo root.
- Every new check is registered in `doctorcmd::diagnose`'s list **and** in the uninitialized-repo
  `n/a` list, asserted by name through `diagnose(root)` — a test calling the check fn directly stays
  green while production doctor never runs it.

**Acceptance criteria:**
- [ ] Fixture with an `objects.archived.<ts>/` dir and a churn-heavy recent log: `doctor` exits
      **0**, `ok: true`, both advisories present. Neuter: make either a Fail → `doctor_advisories_never_flip_exit_code` RED.
- [ ] Heavy churn strictly **older** than the window plus clean recent turns → no churn advice.
      Neuter: remove the window filter → `doctor_ignores_historical_churn` RED. This, not the
      positive case, is the load-bearing criterion.
- [ ] Both checks appear in `diagnose(root).checks` by name and in the uninitialized `n/a` list.
      Neuter: remove from the `diagnose` vec → `doctor_registers_new_checks` RED.
- [ ] Glob sanity reports the tracked-file count on a fixture whose glob matches a tracked file.
      Neuter: delete the tracked-file probe → RED.

**Expected test outputs:** `cargo test -p agentrec --test integration doctor_` → +5;
`cargo test -p agentrec doctorcmd::` → +2. Workspace → **426 passed, 0 failed, 1 ignored**.

---

## Phase 8 — Ref→blob presence check **[SPIKE — may resolve to DEFER; gated on Q2]**

**Description:** Today a `log.jsonl` citing hashes that were never stored passes `doctor` clean,
exit 0 (`check_degraded` reads only `state.json` counters). The obvious fix collides head-on with
PROTOCOL §6: TTL prompt purge, `purge --snapshots-before` and `enforce_budget` all **hard-delete
referenced blobs by design**, and nothing on disk distinguishes "legitimately purged" from "never
landed" — the exact ambiguity change (A) taught `blame` to respect. Run this as a spike with an
explicit exit: if it cannot produce a signal that is honest **and** non-flapping, it is deferred
with the reason recorded, not shipped as a check that is red on every correctly-purged store.

**Files:** `cli/src/doctorcmd.rs` (edit), `cli/tests/integration.rs` (edit).

**Changes (if the spike passes):**
- **Parsed** entries, not the raw byte-scan — this is the one place raw is wrong: since change (B),
  over-cap and io-failed entries carry `after: Some(hash)` for content deliberately never stored, so
  a scan-driven check flags every large file as missing. Skip `skipped`/`withheld` entries.
- Two legs: exhaustive over the **newest K turns** (eviction and TTL purge are oldest-first, so an
  unresolvable ref among the newest is genuinely anomalous), plus a deterministic sample of older
  refs — exhaustive at test-store size, so the positive test cannot pass flakily.
- Cause-agnostic wording only: `unresolvable`, with the sample denominator inline
  (`0 unresolvable in a sample of 200 of 10,636 refs`). Never `corrupt`, `lost`, or `healthy`.
  A blob found under `objects.archived.*` is reported as archived, not missing.
- Advisory severity, never gates the exit code. Phase 3's eviction counters are the only discriminator.

**Acceptance criteria:**
- [ ] Entry with `skipped_reason: "over_cap"` and a ghost `after` reports **0** anomalies. Neuter:
      drop the skipped-entry exclusion → `presence_check_ignores_ghost_hashes` RED.
- [ ] Fixture that has run a real `purge --snapshots-before` → not Fail, exit 0. Neuter: gate the
      exit code on the check → `presence_check_never_fails_a_purged_store` RED.
- [ ] Hash present under `objects.archived.*` classified archived, not missing. Neuter: delete the
      archive lookup → RED.
- [ ] Output contains "unresolvable" and the sample denominator; contains none of "corrupt",
      "lost", "healthy". Neuter: reword to "store healthy" → RED (string-pin tests are weak in
      general; here the string *is* the deliverable).
- [ ] Sample is deterministic and exhaustive at test-store size. Neuter: unconditioned random
      sampling → the positive test goes flaky, which the gate treats as RED.

**Expected test outputs:** `cargo test -p agentrec --test integration doctor_presence` → 5 passed.
Workspace → **431 passed, 0 failed, 1 ignored**. If deferred: 426 stands and the reason lands in
`VERIFY-LEDGER.md` + the Status entry.

---

## Phase 9 — Delta re-measure + Status/doc reconcile (docs only)

**Description:** Phase 5 changes the record path, so every **rate** figure in Phase 1's baseline is
stale by construction the moment it lands (byte *totals* stay valid as-of their stamp; rates do
not). Re-run the same script, diff against the baseline, and write the round's Status entry with
the ban sentences and as-of stamps intact.

**Files:** `docs/2026-07-26-store-measurement-baseline.md` (edit — delta section),
`CLAUDE.md` (edit — Status), `VERIFY-LEDGER.md` (edit — any laddered row).

**Changes:**
- Delta table: baseline vs post-round, same commands, both stamps shown.
- Explicit statement of which baseline figures died with Phase 5 and which survived.
- Status entry states plainly: item 1 produced **zero** byte/line reduction (render-time only) and
  item 5 produced **~0 measured churn reduction on this corpus** (its justification is the protocol
  promise). Any receipt claiming otherwise is rejected.
- Founder-decision rows carried forward, not silently closed: archive `rm` (2.6 GiB + 15 MiB),
  `purge --snapshots-before`, and the still-undecided claimd register/narrative-doc ignore rule.

**Acceptance criteria:**
- [ ] Every figure in the delta section carries as-of, method, and population. Gate-enforced (prose).
- [ ] The Status entry names the two zero-yield results explicitly. Gate-enforced.
- [ ] `VERIFY-LEDGER.md` carries a row for every criterion above marked unprovable-by-fixture, with
      the exact human-run command.

**Expected test outputs:** no test delta. `cargo test --workspace -- --test-threads=3` unchanged
from Phase 8's total; clippy + fmt clean.

---

## Edge cases considered

- **P1:** live daemon growing the store mid-measurement (the documented corruption mode of the
  789.3→798.6 figure); over-budget store where merely *reading* status would evict; archives on a
  different filesystem than `objects/`.
- **P2:** FSEvents coalescing (one event per burst) vs inotify; APFS case-insensitivity; blind
  sleeps replaced by bounded deadline polls; daemon teardown racing the final assertion (the
  `signal_offset` flake precedent).
- **P3:** blob whose only citation is an open turn's `before` (never re-`put`, so the mtime guard
  cannot rescue it); `sync_journal` silently failing and leaving `open.json` stale; torn `open.json`
  under power loss; a fixture whose blob is accidentally fresh enough for A3(c) to rescue (vacuity).
- **P4:** absolute and `..`-bearing `FileEntry.path` from a hand-edited or foreign log; all-noise
  turn; turn with zero files; multi-line `noise_globs` array silently yielding no key; a `#`
  inside a fixture glob (the scanner comment-splits first).
- **P5:** glob matching the repo root; glob matching tracked files; glob added or removed mid-turn;
  config edited while the daemon is idle with no other activity; a path recorded, then excluded,
  then unexcluded (resurfaces as `create` or `baseline_unknown` — both honest); `Config.TOML` on
  case-insensitive APFS (same INFO-class divergence already recorded for `.gitignore`).
- **P6:** log with no config records (pre-existing history); glob set changed several times within
  one blame window; a path excluded *and* covered by a rich turn.
- **P7:** repo with no archives; repo with no globs configured; uninitialized repo (`n/a` list).
- **P8:** store that has legitimately purged 2790 referenced blobs (this repo); blob mid-`put`
  (non-issue — `stage` puts before `persist` appends, and `put_result` is tmp+fsync+rename, so any
  hash in `log.jsonl` is already durable under its final name); `open.json` refs excluded entirely.

## Open questions (answer before implementation)

1. **[gates Phase 5 + Phase 7(c)] Config glob vs git-tracked file precedence.** Decision 7 takes
   "config wins, doctor warns" — the alternative is "tracked files are never excluded, glob or not"
   (safer for the flight-recorder mission, surprising for a user who explicitly configured the
   glob). `.claims/` is the named next churn instance and has 3 tracked files, so this is not
   hypothetical. **The answer changes Phase 5's shape, not just its fixture.** If Q1 = *config
   wins*: the E2E fixture asserts a tracked file under a glob is not recorded, doctor warns, and
   `classify` stays git-free. If Q1 = *tracked never excluded*: `classify` needs a git-tracked-status
   probe — a new dependency on git state in the daemon's hot path, on a code path that touches no
   git today (cost: an index read or a cached tracked-set with its own refresh story). That is
   materially more work, and Phase 5's file count and criteria change with it.
2. **[gates Phase 8] Is an advisory-only presence check worth shipping at all**, given that
   "purged by design" and "never landed" are indistinguishable until Phase 3's eviction counters
   have accumulated history? Options: (a) ship advisory-only now; (b) ship only the newest-K leg
   (the one with teeth) and drop sampling; (c) defer the whole check until the counters exist and
   record the reason.
3. **Founder-called cleanup, unchanged from the last two rounds:** `rm` the retained archives
   (`objects.archived.1784328469` 2.6 GiB + `.1784934498` 15 MiB)? They are the largest items on
   disk, need no code, and Phase 1 will re-state their exact sizes.
4. **Still-undecided from last round:** are register/narrative docs categorically out of claimd
   scope (they have no honest replay command), or should spec docs carry grep-style claims precisely
   because doc drift has already caused a GATE FAIL here? `PROTOCOL.md` is **not** in
   `.claims/config.json`'s `lint.ignore`, so the same finding fires the moment Phase 6 edits it.
