# attest v1 — manual E2E tail, run once end to end

Plan: `docs/superpowers/plans/2026-08-28-attest-plan.md` § "Manual E2E tail".
Run 2026-09-02 on branch `feat/attest`. The first pass ran at `09276fd` + the
Phase 5 working tree; **sections 6, 7 and 9** were re-run after the 2026-09-02
founder ruling on `fyi` and the `attest verify` output fix (post-`bcccabc`
working tree), and carry that re-run's output — their claim ids begin
`c_01M1H5…`, the first pass's begin `c_01M1H4…`, which is how to tell them
apart. Sections 1–5 and 8 show first-pass output. The re-run **did execute**
steps 1, 4 and 5 — it had to, to reach a refuted claim — but their output was
not re-recorded here, because neither fix changes them: section 5's gate output
is a `claim-false` blocker, which the `fyi` ruling does not touch. What the
re-run did NOT run at all is `attest run` and `coverage`. Both passes used the
**release** binary `target/release/agentrec` against a fresh fixture crate.

**Not run on this repository.** The script ran against a throwaway two-test cargo
crate under the session scratchpad
(`/private/tmp/claude-501/-Users-ravichandrasekhar-Projects-agentrec/a4344423-6308-452d-bf5b-21c7acda01bf/scratchpad/e2e`), created for this
run. The production root was never `agentrec record`-ed and the dogfood daemon
(pid 39492) was not touched. Consequences of that choice are named per step
below rather than papered over.

## What could not run, and why

- **Plan step 2 — "real session: implement a small change with Claude Code, watch
  evidence attach to the session's turns."** NOT RUN. It needs a live recording
  daemon and a real agent session; this round was restricted to synchronous
  commands and to a fixture root. Every `evidence` event below therefore carries
  `turn_id: null`, which the report renders as `evidence: no turn joined` — so
  the turn-join half of the report's provenance chain is **unexercised by this
  run**. The review card's diff-pointer rendering for a joined turn IS covered by
  a test instead (`attest_gate::p5_5_review_scripted_stdin_appends_three_human_events`
  asserts the `agentrec diff t_…` line), which is a test, not a live session.
- **Plan step 4 — `attest report --range main..HEAD`.** SUBSTITUTED: the fixture
  crate has no `main` branch history, so the range used its own two shas
  (root commit .. HEAD). Same code path, different revisions.
- **Review-card evidence — a structural gap, not a run-scope one.** Every card
  in section 7 reads `evidence: none recorded` / `diff: no related turn`, and
  re-running this script with a live daemon would not change that. Cards are
  manual claims; `attest run` joins `evidence` to a claim by TEST IDENTITY; a
  manual claim has no test identity by construction. **No sanctioned writer can
  populate a card's evidence today.** The populated rendering exists and is
  covered by `attest_gate::p5_5_…`, which seeds an `evidence` event onto a
  manual claim id — a shape no writer emits. Recorded for founder disposition,
  not redesigned; see `IMPLEMENTATION.md` §attest, AC-ATTEST-P5-5.
- **The daemon-written `stale` overlay.** No daemon ran, so no `stale` event was
  produced by the real producer; `attest verify --all-stale` correctly reported
  `no claims to verify`. The overlay's rendering in gate/report is covered by
  fixture tests (`p5_2_gate_exit_code_table`'s "stale only" case,
  `golden_attest_report`), not by this run.

## The run

### 1. init + derive — claims ≈ test count

```
$ agentrec init --root $S
skipped service install: root is under a temporary directory (/private/tmp) …
$ agentrec attest derive --root $S
discovered 2 test(s): 2 new, 0 renamed, 0 rehashed, 0 unchanged
$ agentrec attest status --root $S
claims: 2
  DERIVED: 2
  …
manual: blocking 0 unanswered / 0 answered, fyi 0 unanswered / 0 answered
stale (overlay): 0
dev-loop-only (dirty tree): 0
```

Two tests in the crate, two claims. The `manual:` line is Phase 5's addition.

### 2. attest run — evidence captured

```
$ agentrec attest run --root $S -- cargo test
test result: ok. 2 passed; 0 failed; …          # the wrapped command's own output
$ agentrec attest status --root $S
claims: 2
  EVIDENCED: 2
  …
dev-loop-only (dirty tree): 2
```

Both runs were on a dirty tree (the `.agentrec/` writes themselves), so both
claims are dev-loop-only — evidence, never a verdict.

### 3. coverage

```
$ agentrec attest coverage --root $S --all
captured 2 test(s), 0 skipped (no built target), 0 leaked profraw file(s) moved aside
wrote …/.agentrec/attest-coverage.json
```

### 4. break a function AND COMMIT it, then verify

```
$ sed -i '' 's/    a + b/    a + b + 1/' src/lib.rs && git commit -am "break add"   # d1b6f12
$ agentrec attest verify --root $S c_01M1H4JHN4DF0N44P83WKXX1YT c_01M1H4JHN4E5F1XB8AX1BVQX3W
c_01M1H4JHN4DF0N44P83WKXX1YT attestdemo::tests::add_is_commutative -> confirmed (1 run)
c_01M1H4JHN4E5F1XB8AX1BVQX3W attestdemo::tests::add_is_sum -> claim-false (3 runs)
```

Exactly the mapped claim went claim-false. `add_is_commutative` still holds under
the break (`add(1,5) == add(5,1)` either way) and stayed confirmed — which is the
right answer, not a miss.

### 5. gate goes red on the refutation

```
$ agentrec attest gate --root $S
blocking:
  c_01M1H4JHN4E5F1XB8AX1BVQX3W claim-false: independent replay refuted this claim
attest gate: FAIL (1 blocking)
exit=1
```

### 6. revert the break, re-verify — and the plan step that CANNOT hold

```
$ git revert --no-edit HEAD    # b7ecdfc, src/lib.rs back to `a + b`
$ agentrec attest verify --root $S c_01M1H5KC7C5DYQ096ANH3HGFMF
c_01M1H5KC7C5DYQ096ANH3HGFMF attestdemo::tests::add_is_sum -> verdict confirmed appended (1 run); claim remains CLAIM_FALSE (permanent, decision 4)
```

**The plan's step 3 says "revert the commit; re-verify → confirmed". As written
that step cannot pass, and this is a plan/spec conflict, not a defect found in
Phase 5's code.** Spec decision 4 makes `claim-false` **permanent**: the fold
counts the later `confirmed` verdict and moves nothing
(`fold.rs`, "CLAIM_FALSE is permanent against EVERY later event kind"). The
claim is still `CLAIM_FALSE` above, and the gate below is still red because of
it. Founder-owned: either the plan's step is reworded, or decision 4 changes.

**Fixed after the first pass, on coordinator direction (AC-ATTEST-P5-13).** The
first run of this script had `attest verify` print a bare `-> confirmed` for a
claim the fold left `CLAIM_FALSE` — true of the verdict it appended, but read
beside `attest status` the two looked contradictory. `replaycmd.rs` now appends
first, re-folds the log (the same reader `attest status` uses), and prints both
clauses when they differ; when they agree the line is unchanged. The output
above is from the re-run. Tests:
`attest_verify::p5_13_a_passing_replay_on_a_refuted_claim_prints_both` and
`p5_13_an_agreeing_verdict_keeps_the_short_line`. Phase 5's report line is
labelled `last verdict appended` for the same reason.

### 7. manual-declare, review, gate

```
$ agentrec attest manual-declare --root $S --text "README documents the add() contract" --severity blocking
c_01M1H5MBFTRYC8BWFBAFMV7AN5
$ agentrec attest manual-declare --root $S --text "CHANGELOG entry is nice to have" --severity fyi
c_01M1H5MBG8SKEWT2TW76TRY9C4

$ agentrec attest gate --root $S
blocking:
  c_01M1H5KC7C5DYQ096ANH3HGFMF claim-false: independent replay refuted this claim
  c_01M1H5MBFTRYC8BWFBAFMV7AN5 blocking manual criterion unanswered — README documents the add() contract
attest gate: FAIL (2 blocking)
exit=1

$ printf 'y\n' | agentrec attest review --root $S
[1/1] c_01M1H5MBFTRYC8BWFBAFMV7AN5 (blocking)
  criterion: README documents the add() contract
  status: DECLARED
  evidence: none recorded
  diff: no related turn
  history: 0 evidence, 0 verdict(s), 0 stale event(s)
  [y]es / [n]o / [s]kip

$ agentrec attest gate --root $S
blocking:
  c_01M1H5KC7C5DYQ096ANH3HGFMF claim-false: independent replay refuted this claim
attest gate: FAIL (1 blocking)
exit=1
```

The plan's step 5 — "one manual-declare blocking item; gate red; review y; gate
green" — **passed for its manual half**: the blocking manual item left the
blocking list after `y`, dropping the count 2 → 1. The gate did not go fully
green, because the permanent `claim-false` from step 4 is still there (step 6).

**The `fyi` item appears nowhere above, which is the point.** Founder ruling
2026-09-02 (the plan's Phase 5 AC, "fyi items never appear in review or gate
output"): `attest review` offered ONE card, not two — the `fyi` item was never
prompted for — and `attest gate` listed it in neither the blocking nor an
advisory section, before or after. It is recorded, not dropped:

```
$ agentrec attest report --root $S --json    # manual claims only
c_01M1H5MBFTRYC8BWFBAFMV7AN5 blocking 'README documents the add() contract'
c_01M1H5MBG8SKEWT2TW76TRY9C4 fyi 'CHANGELOG entry is nice to have'
$ agentrec attest status --root $S --json    # the manual key
{"blocking": {"answered": 1, "unanswered": 0}, "fyi": {"answered": 0, "unanswered": 1}}
```

An earlier version of this document showed the `fyi` item as a review card and a
gate advisory line; that was the pre-ruling behaviour and is gone from both the
code and this record.

### 8. report, with and without `--range`

*Provenance: this section is the FIRST pass's output (claim ids `c_01M1H4…`).
The re-run that produced sections 6–7 ran `derive` and `verify` but NOT
`attest run` or `coverage`, so in that pass no claim carries an `evidence`
event: `add_is_sum` reached `CLAIM_FALSE` through verify alone and
`add_is_commutative` stayed `DERIVED`. Its log also holds **two** `confirmed`
verdicts at `b7ecdfc` — the post-revert verify was invoked twice while the
output wording was being fixed — and both are counted while the claim stays
refuted, which is decision 4 working, not an artefact. Its `--range` window also holds 0 claims,
because every event it wrote landed after its last commit. Neither the report
renderer nor the range filter changed between the two passes; only the
`last verdict` label did, and the line below carries the current wording.*

```
$ agentrec attest report --root $S
# attest report

Every claim in `.agentrec/attest.jsonl`.

claims: 4

## c_01M1H4JHN4E5F1XB8AX1BVQX3W
- identity: attestdemo::tests::add_is_sum
- status: CLAIM_FALSE
- stale: false
- history: 1 derive(s), 1 evidence, 3 verdict(s) (confirmed 1, claim-false 2, recipe-invalid 0, flaky 0), 0 stale event(s), 0 human answer(s)
- last verdict appended: confirmed (replay 1ff6a8ec8071f6b6c242b8c9ced6150c986ed2e4)
- evidence: no turn joined
…

$ agentrec attest report --root $S --range df37905…..1ff6a8e…
Range `df37905…..1ff6a8e…`: claims with at least one event between the two
commits' committer timestamps. A time window, not a causal one.

claims: 2

$ agentrec attest report --root $S --range deadbeefdeadbeefdeadbeefdeadbeefdeadbeef..HEAD
agentrec: --range: not a revision in this repo: deadbeefdeadbeefdeadbeefdeadbeefdeadbeef
exit=1
```

`--range` dropped the two manual claims, whose `manual-declare` events were
appended after the last commit's committer timestamp. That is the filter working
as documented and is also its honest limit: it is a **time** window, so a claim
whose events land outside the range is dropped regardless of which commit
actually caused it.

### 9. status --json additive keys

*Re-run output (post-ruling).*

```
$ agentrec attest status --root $S --json    # the two new keys only
{
  "recipe_invalid_causes": {},
  "manual": {
    "blocking": {"answered": 1, "unanswered": 0},
    "fyi": {"answered": 0, "unanswered": 1}
  }
}
```

`fyi.unanswered` stays 1 and cannot be driven to 0 by `attest review`: review
does not offer `fyi` cards. Answering one requires a `human` event from another
writer, which nothing in this script produces. The first pass showed
`fyi.answered: 1` because review prompted for it then; that behaviour is gone.

`recipe_invalid_causes` is empty because no run in this script produced a
recipe-invalid verdict — that arm is covered by
`attest_gate::p5_3_recipe_invalid_and_flaky_never_block` over all four causes,
by fixture, not here.
