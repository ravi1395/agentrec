# D6 spike gate — transcript-declared coverage on the real dogfood corpus

Phase 1 of `docs/superpowers/plans/2026-08-01-d6-transcript-correlation.md`.
Run 2026-08-01 against the live dogfood repo (read-only). Replay:

```bash
python3 scripts/d6_spike.py --root ~/Projects/agentrec
```

Corpus is a moving target (live daemon) — absolutes drift between runs, ratios have been
stable across the three runs made while building the script (population grew 1440→1454
during the session). Test baseline in this worktree before any phase-2 edit:
`cargo test --workspace -- --test-threads=3` → **666 passed / 0 failed / 3 ignored** (11
suites, full-output run, warm build).

## Population and join

| Figure | Value |
|---|---|
| Rich `tool:"claude"` turns with `session` | 1454 (803 distinct sessions) |
| Transcript join rate | **803/803 = 100%** (whole log inside the 30-day retention window; a re-import corpus would NOT enjoy this) |
| Declared writes found (all shapes) | 1439, of which **559 out-of-root** (worktree discipline pushes real edits out of this root) |
| Sessions with ≥1 in-root declared write | **28 of 803** |
| Sessions with ≥1 observed file | 227 |

## Coverage (declared ∩ observed / observed)

| Metric | Work files | Hook artifacts¹ |
|---|---|---|
| Session-level ceiling (union vs union) | **64/442 = 14.5%** | 3/2822 = **0.1%** |
| Turn-level, session-joined (lead 120s/late 60s) | **83/541 = 15.3%** | 5/3511 = 0.1% |
| Turn-level, ANY-session index (lead 300s) | **132/541 = 24.4%** | — |

¹ `.remember/`, `.claims/`, `.superpowers/`, `_rsrc_probe/`, `.code-review-graph/`.

- Of 176 turns with work files (any-session pass): **81 fully covered, 84 zero-covered, 11
  partial** → when any declared hit exists, the turn is fully covered 81/92 = 88% of the
  time. Correlation is **precise where the signal exists; the signal often does not exist.**
- Declared shapes both fire in the wild: `tool_use` 391, `toolUseResult` 374 in-window
  matches — harvest both (they cross-confirm; neither alone suffices).
- Late declares (transcript ts after `ended`): 7 instances, 20–50 s. Small; `ended`
  under-approximates Stop arrival, so production Stop-time reads will see fewer.

## Why coverage is low — measured, not guessed

1. **Hook churn dominates observation.** 3360 of 4135 (81%) files in rich claude turns are
   `.remember/` writes, plus `.claims/` etc. — session-hook output the transcript never
   declares. Today ALL of it renders as agent activity inside brackets (the D6 defect).
   Under correlation it goes unattributed at **0.1% false-match** — this is the headline
   win, not a failure: the misattributed mass stops being blamed on the agent and stops
   being silently revertible.
2. **Bash/git-mediated mutations are undeclared.** Uncovered work files are dominated by
   `cli/` (273), `docs/` (42), `agentrec-core/` (33) — main-root mutations from `git merge`
   of worktree branches, heredoc/script writes, `cargo` side effects. Genuinely
   agent-caused, structurally absent from Write/Edit declarations. Honest outcome:
   unattributed (safe direction for undo — cost is undo utility, not data loss).
3. **This corpus is policy-skewed.** The house worktree rule forbids editing in this root;
   559/1439 declared writes target worktrees/other repos. A typical user letting an agent
   edit the repo directly would sit far closer to the 88%-when-present figure than to
   14.5%. Not provable from this corpus — stated as skew, not extrapolated.

## F4 is live in this corpus and poisons `turn.session`

**739 of 803 joined sessions (92%) are hook-only stubs** — transcripts with zero
`tool_use` lines (background memory-save / attachment sessions). Their Stop signals closed
brackets around other activity and got recorded as the turn's `session`. Consequences:

- Any correlation keyed on **persisted** `turn.session` inherits F4's lie. The producer
  must correlate at Stop-time from the **closing signal's** transcript path — and item 2
  (F4 session-aware matching) is a de-facto prerequisite for the session on the record to
  mean anything.
- The session-joined coverage figures above are therefore a floor; the any-session pass
  (24.4%) approximates what Stop-time correlation with correct session matching could see.

## Phantom rate (declared in-root, never observed anywhere in the session)

39 total: 8 `.claude/*` (watch-filtered — gitignored worktrees/settings), 30
exists-never-observed (recording gaps while daemon down + files created-then-deleted, e.g.
HANDOFF.md ritual), 1 hook-artifact. All bucketed, none unexplained at the
"declared-but-agent-never-wrote" level this metric bounds. **True false-attribution
(declared AND observed but human-authored) remains unmeasurable without ground truth;
coverage precision (88% full-cover when present) and phantom buckets bound it.**

## GATE ROW

**Recommendation: PROCEED, with the claim narrowed.** Correlation earns attribution only
for transcript-declared writes: precise where present (88% full-cover, 0.1% hook
false-match), honestly absent elsewhere (bash/git-mediated and hook-driven mutations stay
unattributed — the safe direction). The design does NOT change structurally, but two
consequences bind phases 2–4:

1. Correlate from the closing signal's transcript at Stop-time; never from persisted
   `turn.session` (F4 contamination, 92% stub rate here).
2. Expect majority-unattributed turns on bash-heavy workflows: `undo` default-plan
   shrinkage must be presented as scope-narrowing, not breakage; `blame` wording must not
   imply "human" for unattributed files (tri-state, decision 7).

Founder decision per plan decision 6: **PROCEED, as option 1 (emitter-side declaration) —
ruled 2026-08-01.** The Stop signal gains an additive `files_written` field; the daemon-side
transcript parse becomes the fallback tier. Plan decision 10 records the ruling and rationale;
phases 2a/2b in the plan carry the revised scope. Both binding consequences above stand.

Bash-command path extraction (parsing `tool_use` Bash inputs for redirect/heredoc targets)
is a possible future coverage extension; out of scope for this branch, recorded here so it
is not reinvented as scope creep.

## Addendum (2026-08-01, post-2b gate): production scoping rule, population-level

The skeptic gate noted the last-user-prompt cutoff (the PRODUCTION scoping rule in
`declared_writes_from_transcript`) was only spot-checked on two transcripts — the spike above
measured time-windows, not this rule. Closed by running the real release `hook` binary over the
first 300 transcripts in `~/.claude/projects/-Users-ravichandrasekhar-Projects-agentrec/` against
a scratch root and reading the emitted `signal.jsonl`:

- 300 stop signals: **290 carry no `files_written` (97%)**, zero empty lists — honest silence.
  286 of the 300 transcripts have zero session writes at all, so the bulk of that silence is
  "nothing to declare", not the cutoff rule working: **the discriminating base for the cutoff
  rule is the 14 write-bearing transcripts**, not 300.
- 10 declaring signals: set sizes, full multiset (percentile conventions disagree on n=10, so
  no percentiles): **[1, 1, 1, 1, 1, 2, 2, 3, 4, 5]**.

**Corrected by the 2026-08-01 re-gate (the first version of this addendum said "sessions hold up
to ~40 writes" — false at this scope; skeptic-measured max over these 300 is 16):**

- Scoping evidence proper: **5 of 10** declaring transcripts emit fewer paths than their session
  total (gaps: 2<11, 1<6, 1<5, 1<4, 1<2) — the clean pair is `3104ac02-…jsonl`: **emitted 2 vs
  11 session-total**. The other **5** have emitted == session-total, indistinguishable from an
  unscoped parser. The cutoff demonstrably scopes; the base is narrow and a different 300 could
  move these numbers. (Re-gate round 2 corrected this row: round 1's report said 4-of-10 — the
  skeptic's own prose miscount of its own gap table — and the first version of this correction
  copied it verbatim. Numbers here are from the round-2 fresh replay.)
- **Under-declaration magnitude, first measurement:** 4 of the 14 write-bearing transcripts
  (session totals 6/7/9/16) declared **nothing** — 29% fully silent. The code comment discloses
  under-declaration as a direction; this is its measured size on this sample. Phase 3/4 must not
  present declared-coverage as approximating agent activity.

Replay: loop `printf '{"hook_event_name":"Stop","session_id":"pop","transcript_path":"<t>"}' |
agentrec hook claude --root <scratch>` over the transcript glob, then histogram `files_written`
lengths in `<scratch>/.agentrec/signal.jsonl`.

Still open (recorded, not closable here): Linux leg for this branch — macOS-only evidence until
CI runs it. Also open: the >64 MiB cap path has never executed anywhere (corpus max 27 MiB) — an
oversize E2E plus a daemon-tier observation would empirically pin the tier-shift behavior the cap
comment now describes.
