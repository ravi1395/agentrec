# Plan: D6 transcript correlation — per-file attribution (4 phases, worktree: ~/Projects/agentrec-mvp-promise, branch feat/mvp-promise)

**Goal + architecture (≤5 lines):** At bracket close, intersect transcript-declared writes
(`toolUseResult.filePath` from the Claude Code transcript the Stop signal already names) against
the fs mutations folded into the rich turn. Declared∩observed → attributed to agent; everything
else stays honestly unattributed at file level, excluded from `undo`'s default plan, rendered as
such by `blame`. Correlation is daemon-side only (`cli/src/daemon.rs`); `agentrec-core/engine.rs`
stays transcript-free. Spec: `BRANCH-SCOPE.md` item 4; findings doc `docs/redteam/2026-08-01-redteam-round-2.md`.

**Decisions log (inherited, non-re-litigable):**
1. D6 is a code fix, not disclosure-only (founder-accepted, red team + skeptic concur).
2. Per-file timestamps REJECTED as attribution mechanism — fs layer cannot distinguish human save
   from agent write; transcript is the only distinguishing signal.
3. Periphery branch owns the `FileEntry` schema change (defines link_kind + attribution field in
   one additive commit). This branch ships a **producer only**. Field name/shape confirmed with
   periphery before producer code.
4. Stop-only / L1 emitters and bare turns stay honestly unattributed — never default to "agent".
5. Undo default-plan treatment of unattributed files: excluded (BRANCH-SCOPE), exact UX per Q3.
6. **Q1 RESOLVED (founder, 2026-08-01):** spike gate decided by founder reading the phase-1
   figures — no pre-committed threshold. GATE ROW carries a recommendation only.
7. **Q2 RESOLVED (founder, 2026-08-01):** attribution field is TRI-STATE
   (declared / undeclared-with-transcript / no-transcript). Shape to be relayed to the periphery
   worktree, which still owns the schema commit.
8. **Q3 RESOLVED (founder, 2026-08-01):** unattributed files excluded from default undo plan;
   explicit `undo --file <path>` still reverts them (explicit-name = consent). No new flag.
9. **Q4 RESOLVED (founder, 2026-08-01):** spike is python in `scripts/`, committed as replayable
   evidence.

**Infeasible/rejected (against real code):**
- Reusing `importcmd.rs::process_session_file`/`persist_session_file` directly — both private,
  coupled to dry-run `Counters`/`DebugSink`; two separate walkers exist (importcmd.rs:618-867,
  :1726-). Reuse the transcript **line shapes** they document, not the functions.
- Attribution inside `agentrec-core::TurnEngine` — engine signatures (`observe_start`/`observe_stop`,
  engine.rs:250-361) carry no transcript param; threading one in violates the core/cli seam P4
  closed and gives the engine an fs-external input.
- Naive string intersect of declared vs observed paths — `FileEntry.path` is root-relative,
  `toolUseResult.filePath` absolute; this exact mismatch was importcmd's T1.5 under-detection bug
  (normalization precedent: importcmd.rs:900-914).

**Global constraints:**
- Never build or run tests in `~/Projects/agentrec` (production daemon watches it). Build here.
- Baseline before first edit: `cargo test --workspace -- --test-threads=3` → expect 666 / 0 / 3.
- Honesty semantics are a ratchet: existing `misattribution.rs` CAUTION assertions
  (cli/tests/misattribution.rs:31-40) must not weaken. Bare turns never render as agent activity.
- Additive wire format only: new `FileEntry` field is `#[serde(default, skip_serializing_if)]`
  (precedent `after_synthesized`, record.rs:85-96); old log lines stay byte-identical.
- Probe before writing normative text (signature-defect rule; spike exists for this).
- `claimd:declare-claims` per AC before implementing; fresh-context skeptical-reviewer gates each
  phase.

**Executor protocol:** one phase per commit minimum; phases 1→2 sequential (2's design confirmed by
1's figures), 3 gated on periphery merge + Q1/Q2, 4 after 3. Each phase independently mergeable
(2 lands dark — extraction built + tested, not wired to output).

---

## Phase 1 — Spike gate: measure correlation viability on the real dogfood corpus
**Description:** Offline measurement joining `~/Projects/agentrec/.agentrec/log.jsonl` rich
bracketed turns (via `session` UUID) to `~/.claude/projects/<encoded-cwd>/<session>.jsonl`
transcripts, intersecting declared writes against each turn's persisted `files[]`. Scariest
unknown first: if declared-coverage is low, the design changes and phases 2–4 do not proceed as
written. Read-only against both corpora.
**Files:** `scripts/d6_spike.py` (new), `docs/verify/d6-spike.md` (new)
**Changes:**
- Script: for each rich turn with `session` — resolve transcript (report join rate vs 30-day
  retention); extract declared write paths (`toolUseResult.filePath`, `Write`/`Edit`/`MultiEdit`
  tool_use inputs; note which shapes actually occur); normalize abs→root-relative; compute per-turn
  coverage = |declared∩observed|/|observed|, phantom rate = |declared∖observed|/|declared|, and
  write-lag = (last declared-write transcript ts) − (turn end ts) distribution.
- Results doc: figures WITH denominators, per-conjunct; explicit statement that true
  false-attribution (declared AND observed but human-authored) is NOT measurable without ground
  truth — the phantom rate and coverage bound it, they do not equal it.
**Acceptance criteria:**
- [ ] Join rate reported: N rich bracketed turns with `session`, M with resolvable transcript, with
      the retention bound stated.
- [ ] Declared-coverage distribution reported (per-turn and aggregate) over ≥50 joined turns, or
      the shortfall stated with the actual N — no extrapolation past the denominator.
- [ ] Phantom-declaration rate reported with per-cause buckets where determinable (watch-filtered
      path, out-of-root, lag).
- [ ] Write-lag distribution reported; fraction of turns where Stop precedes last declared write
      quantified.
- [ ] Every figure in the doc reproducible by re-running the script (command line in the doc);
      script committed.
- [ ] GATE ROW: explicit PROCEED / REDESIGN recommendation against Q1's threshold, founder-decided.
**Expected test outputs:** no product code — `cargo test` untouched (666 / 0 / 3 baseline recorded
in the doc from the pre-edit run). `python3 scripts/d6_spike.py --root ~/Projects/agentrec` →
prints the same figures the doc carries.

## Phase 2 — Declared-writes extraction in the daemon (dark)
**Description:** Extend the daemon's existing single transcript read to also extract the declared
write-path set. Pure function + plumbing to the Stop path; nothing persisted yet, so the phase
lands dark and mergeable regardless of periphery timing. Shapes and normalization informed by
phase 1's measured corpus (probe-first rule).
**Files:** `cli/src/daemon.rs` (edit), `cli/tests/misattribution.rs` (edit)
**Changes:**
- New fn (near `signal_context`, daemon.rs:1692-1713): parse the already-read transcript content →
  `DeclaredWrites { paths: HashSet<PathBuf> /* root-relative */, counts... }`; single
  `read_to_string`, no second read.
- Normalization: absolutize→strip root prefix→lexical normalize; out-of-root declared paths
  excluded and counted, never silently dropped (skipped_out_of_cwd precedent).
- Unit tests: fixture transcript lines for each shape phase 1 found live; missing/unreadable/
  absent transcript → `None`, no panic; out-of-root exclusion counted.
**Acceptance criteria:**
- [ ] Given a fixture transcript with declared writes in each corpus-observed shape, extraction
      returns exactly the expected root-relative set.
- [ ] `transcript: None` (L1 emitter) and unreadable-file cases return `None` without error;
      daemon Stop path behavior otherwise unchanged (existing tests green).
- [ ] Transcript file is read exactly once per Stop (no added read syscall on the hot path —
      assert by construction: extraction takes `&str` already held by `signal_context`).
- [ ] Zero change to any golden and zero change to persisted output (dark).
**Expected test outputs:** `cargo test --workspace -- --test-threads=3` → 666+N / 0 / 3 where N =
new extraction tests (≥5, named `d6_extract_*` in misattribution.rs + daemon.rs unit tests); all
33 goldens byte-identical.

## Phase 3 — Attribution producer at persist [gated on Q1 PROCEED, Q2 field shape, periphery merge]
**Description:** Populate the periphery-defined `FileEntry` attribution field at the single
`ChangeObs`→`FileEntry` conversion point. Rebase onto merged `main` first (coordination protocol).
`persist()` gains an `Option<&DeclaredWrites>` — only the sig-loop call site (daemon.rs:387) has
one; tick (:392) and shutdown (:411) pass `None`.
**Files:** `cli/src/daemon.rs` (edit), `cli/tests/misattribution.rs` (edit), `IMPLEMENTATION.md` (edit — AC rows + register cross-ref)
**Changes:**
- `persist` (daemon.rs:1771-1832): at :1778 conversion, mark declared∩observed per Q2's shape;
  undeclared observed files get the unattributed value; bare turns and `None` signal → all
  unattributed.
- If Q2's shape distinguishes "no transcript" from "transcript present, file undeclared", populate
  both honestly (per-option criterion below).
- IMPLEMENTATION.md: AC rows D6-ATTR-1..n mapped to named tests.
**Acceptance criteria:**
- [ ] E2E against the real daemon+binary (misattribution.rs harness): bracketed rich turn with a
      transcript declaring file A, fs mutations on A and B → A carries agent attribution, B does
      not. Extends, never weakens, the existing D6 E2E.
- [ ] Bare turn: no file carries agent attribution, ever.
- [ ] Stop without transcript (L1): no file carries agent attribution; if Q2=tri-state, files carry
      the "no signal" value, not "undeclared".
- [ ] Pre-existing log lines and all pre-periphery goldens byte-identical (serde skip); goldens
      change additively only, regenerated with `UPDATE_GOLDEN=1` and said so in the commit.
- [ ] `withheld`/`skipped` files: attribution never contradicts their existing semantics (they stay
      non-revertible regardless of attribution).
**Expected test outputs:** `cargo test --workspace -- --test-threads=3` → prior+M / 0 / 3, M ≥ 4
new named `d6_attr_*` tests (exact prior count = post-rebase baseline, recorded in the commit).

## Phase 4 — Consumers: blame rendering + undo default-plan exclusion [gated on phase 3]
**Description:** Make attribution visible and load-bearing: `blame` renders unattributed files as
unattributed (never as agent activity), and `undo`'s default plan excludes them — closing the exact
D6 data-loss chain (human edit inside bracket silently reverted).
**Files:** `agentrec-core/src/view.rs` (edit — blame read path :528/:1034), `cli/src/readcmds.rs` (edit — `build_plan` :680, `PlanKind` :666), `cli/tests/misattribution.rs` (edit)
**Changes:**
- view.rs blame: per-file attribution surfaces in `BlameResult`; unattributed file inside an agent
  turn renders with unattributed wording, not the agent's tool/prompt.
- readcmds.rs `build_plan`: unattributed files → `PlanKind::Excluded { cause }` in the default
  plan; inclusion path per Q3.
- The existing unconditional CAUTION lines remain (ratchet): attribution narrows the caution's
  *scope*, it does not remove it.
**Acceptance criteria:**
- [ ] E2E: the phase-3 A/B fixture — `blame B` names no tool/prompt from the turn; `undo <turn>`
      default plan lists B as excluded with cause, reverts only A.
- [ ] Existing CAUTION_SUBSTR / CAUTION_SCOPE_SUBSTR assertions still pass unmodified.
- [ ] `undo` on a turn where ALL files are unattributed refuses with an explanatory message rather
      than emitting an empty revert.
- [ ] Human-form goldens: changes only where attribution data exists in the fixture; regenerated
      explicitly.
**Expected test outputs:** `cargo test --workspace -- --test-threads=3` → prior+K / 0 / 3, K ≥ 5
new named tests (blame render ×2, undo exclusion ×2, all-unattributed refusal ×1); clippy `-D
warnings` + fmt clean, debug and release.

---

## Edge cases considered
- **Transcript absent/unreadable/truncated mid-line** (P2): extraction returns `None`/partial set
  without panic; daemon never blocks the Stop path on transcript IO.
- **Write-lag — Stop before last tool result flushed** (P1 measures, P3 consumes): quantified in
  spike; if material, mitigation is a bounded re-read, decided at gate — NOT silently assumed zero.
- **Abs/rel/symlink path mismatch** (P2): normalization with counted exclusions; symlinked roots
  follow the root-canonicalize convention (D5).
- **Same file declared by agent AND human-edited in same bracket** (P3/P4): attribution says
  "agent declared it", modified-since rail (D30) still owns post-turn divergence — attribution
  never suppresses that rail.
- **Multiple sessions interleaved** (out of scope here): item 2 (F4) fixes session matching;
  correlation keys off the closing signal's transcript only.
- **Secret/withheld/skipped files** (P3): attribution never makes them revertible.
- **Bare turns, L1 emitters** (P3): structurally unattributed — the honest residual, disclosed.
- **Empty declared set with non-empty observed set** (P3/P4): valid outcome — everything
  unattributed; undo refuses per P4 criterion.
- **Concurrent daemon tick closing the turn before the sig-loop persist** (P3): tick/shutdown call
  sites pass `None` — a turn closed by timeout gets no attribution rather than wrong attribution.

## Open questions
All four resolved by founder 2026-08-01 — see Decisions log entries 6–9. Remaining external
dependency: periphery worktree lands the tri-state field per decision 7 before P3 starts.
