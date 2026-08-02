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
10. **SPIKE GATE RULING (founder, 2026-08-01): PROCEED with option 1 — emitter-side declaration.**
    The Stop signal gains an additive `files_written` field (L2+ emitters compute the declared-write
    list themselves, before signaling); the daemon-side transcript parse survives as a FALLBACK for
    signals that name a transcript but carry no list; no list AND no transcript → all files
    unattributed, never guessed. Rationale: same honesty model and same coverage as daemon-side
    parsing, but tool-agnostic (Codex 2.1 gets attribution the day its hook exists), no transcript
    IO on the daemon hot path, and the emitter's list is complete before the signal exists — closing
    the write-lag hole the spike measured (declares landing 20–50 s after turn end). The PROTOCOL.md
    signal-schema addition MUST land before the 1.0 freeze — this branch's window is the only one.
    Spike's other binding finding stands: correlation keys off the CLOSING SIGNAL's data at
    Stop-time, never persisted `turn.session` (F4 contamination, 739/803 stub sessions measured).

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

## Phase 2a — `files_written` signal field (protocol, freeze-critical)
**Description:** Per decision 10: additive `files_written` field on the Stop-signal schema. Wire
format only — no daemon behavior change; the field parses and is ignored. This is the piece that
must precede the PROTOCOL 1.0 freeze; landing it first and small de-risks the freeze deadline
against phase 2b's larger surface.
**Files:** `PROTOCOL.md` (edit — §2 signal schema), `agentrec-core/src/record.rs` (edit —
`SignalEvent.files_written: Option<Vec<String>>`), conformance fixtures `(mech)`
**Changes:**
- PROTOCOL.md §2: `files_written` on `event:"stop"` — optional array of absolute paths the emitter
  itself wrote during the turn; L2+ emitters SHOULD populate; consumers MUST tolerate absence.
  Conformance fixture updated in the same commit (protocol house rule).
- `SignalEvent`: new optional field, `#[serde(default)]` — old signal lines parse unchanged.
**Acceptance criteria:**
- [ ] Old-format signal lines (no field) deserialize unchanged; new-format lines round-trip with
      the field intact.
- [x] Conformance coverage via `signal_stream_with_files_written_keeps_existing_routing` — NOTE:
      the original text named "PROTOCOL conformance fixtures", a corpus that does not exist in
      this repo (skeptic-verified); satisfied as restated, recorded rather than quietly reworded.
      **Founder-ratified 2026-08-02** — the restatement (commit `6c86e3a`) is a legitimate
      correction, not an AC weakening; both skeptic engagements verified its factual premises
      (30 golden files; no conformance-fixture corpus anywhere; the named test exists), closing
      re-gate residual 4.
- [ ] Zero behavior change: daemon ignores the field at this phase; all goldens byte-identical.
- [ ] claimd doc-scope note: PROTOCOL.md is not lint-ignored — declare the doc-edit claim before
      the edit (the Stop hook WILL fire; the undecided doc-scope rule is a known repo debt, not a
      surprise).
**Expected test outputs:** `cargo test --workspace -- --test-threads=3` → 666+N / 0 / 3, N ≥ 2
(serde round-trip + conformance); all 30 golden fixture files byte-identical.

## Phase 2b — Tiered declared-writes resolution in the daemon + hook emitter (dark)
**Description:** Daemon resolves a `DeclaredWrites` value on the Stop path via a three-tier ladder —
signal field, transcript-parse fallback, honest `None` — and drops it (nothing persisted; dark).
Claude Code hook emitter computes the list before signaling. Shapes and normalization informed by
phase 1's measured corpus (probe-first rule).
**Files:** `cli/src/daemon.rs` (edit), `cli/tests/misattribution.rs` (edit), hook emitter script
(edit — the installed Stop hook under `init`'s hook template)
**Changes:**
- daemon.rs resolution ladder: (1) `files_written` present → normalize, done, transcript untouched;
  (2) else transcript named → fallback parse from the single existing `signal_context` read (fn
  near daemon.rs:1692-1713); (3) else `None`. Tier recorded in the value — phase 3's tri-state and
  phase 4's wording need to know declared-by-list vs declared-by-parse vs nothing.
- Hook emitter: compute list from its transcript pre-emit; keep emitting the transcript path too
  (fallback tier stays live for other emitters, not silently dead code).
- Normalization (both tiers): absolutize→strip root prefix→lexical normalize; out-of-root declared
  paths excluded and counted, never silently dropped (skipped_out_of_cwd precedent).
**Acceptance criteria:**
- [ ] Signal with `files_written` → exactly that set (normalized); transcript NOT read in this tier
      (fixture names a nonexistent transcript path — still resolves from the field, no error).
- [ ] Signal without the field but readable transcript → same set via fallback parse; fixture
      transcripts cover each corpus-observed shape (`tool_use` input + `toolUseResult`, both — the
      spike measured both firing ~equally).
- [ ] No field AND no/unreadable transcript → `None`, no panic, Stop path otherwise unchanged
      (existing tests green).
- [ ] Zero change to any golden and zero change to persisted output (dark).
**Expected test outputs:** `cargo test --workspace -- --test-threads=3` → prior+N / 0 / 3, N ≥ 6
new tests named `d6_signal_field_*` / `d6_extract_*`; all goldens byte-identical.

## Phase 3 — Attribution producer at persist [gated on periphery merge; Q1 ruled PROCEED, Q2 ruled tri-state — decisions 6/7/10]
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
- [ ] **Wiring proven, not inherited (2a+2b skeptic residual):** the dark `let _declared` binding
      is replaced by real consumption; deleting the `resolve_declared` call site must red an E2E
      (attribution values observed in `log.jsonl` through the real daemon). The `#[allow(dead_code)]`
      markers on `DeclaredWrites` are removed in the same commit.
- [ ] **`out_of_root` surfaced (2a+2b skeptic residual):** the per-turn out-of-root declare count
      is visible somewhere a user can find it (status counter or DEGRADED-adjacent accounting —
      exact surface decided at implementation), not counted into a field nothing reads. The spike
      measured 559/1439 = 39% out-of-root; that number must not be invisible.
- [ ] **Value set matches the relayed contract:** wire values exactly `"declared"` / `"undeclared"`
      / absent, per `D6-ATTRIBUTION-CONTRACT.md` (relayed to the periphery worktree 2026-08-01) and
      periphery's open-enum consumer rules (unknown values degrade to unattributed, never
      refuse-to-parse/act).
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
