# ATTEST-FORMAT — attest v1 wire formats

**DRAFT — UNSTABLE, not covered by PROTOCOL.md versioning (spec decision 9).**

Formats here may change incompatibly without a version bump until the attest
subsystem earns a section in `PROTOCOL.md`. There is deliberately **no schema
version field** on attest events: adding one would assert a versioning contract
decision 9 declined to give. Consumers tolerate unknown fields and unknown event
kinds instead (below).

Spec: `docs/superpowers/specs/2026-08-28-attest-design.md`. Plan:
`docs/superpowers/plans/2026-08-28-attest-plan.md`. Acceptance criteria:
`IMPLEMENTATION.md` §attest. Types: `agentrec-core/src/attest/`.

## `.agentrec/attest.jsonl`

One JSON object per line. **Append-only** — no line is mutated, reordered or
rewritten, and no `purge` rewrite class exists for this file in v1 (adding one
requires a decision-register entry). Corrections are made by appending.

Unlike `log.jsonl`, this file has **multiple routine writers**: CLI commands
append `evidence` / `verdict` / `human` / `manual-declare`, and the daemon
appends `stale`. Every writer — the daemon included — takes the append lock in
`cli/src/attest/lock.rs` (`append_attest_locked`, blocking); the daemon's own
`stale` writer lands in Phase 4 and must use the same call.

Every event carries `kind` (the serde tag) and `ts` (unix milliseconds), and
names the claim it applies to with `claim_id`.

### Reading tolerantly

`agentrec_core::attest::events::parse_log` classifies each line and counts what
it tolerated, so nothing is dropped silently:

| Line | Bucket | What it means for the user |
|---|---|---|
| A known `kind`, parsed | event | — |
| Well-formed JSON, unrecognized `kind` | `unknown_kind_lines` | a newer agentrec wrote this; upgrade |
| Unparseable, or a known `kind` written malformed | `unparsed_lines` | the log took damage (torn tail after a crash) |
| Empty | ignored | — |

Unknown *fields* on a known kind are accepted and ignored.

## Identity

**`ClaimId`** — a stable surrogate, `c_<ULID>` (machine-scoped, the turn-id
scheme from `agentrec-core/src/id.rs`). Minted once when a claim is first
derived and never changed for the life of that claim. It is deliberately **not**
a hash of the test identity: identity includes the fn name, so an
identity-derived key could not survive a rename (spec decision 10). The `c_`
prefix keeps it distinguishable from a turn id (`t_`), which appears alongside
it on `evidence` events.

**`TestIdentity`** — `{ "target": "<cargo target/binary>", "fn_path": "<test fn
module path>" }`. **Both components are load-bearing.** Two identically-named
test fns in different cargo targets are two different tests; keying on `fn_path`
alone silently cross-attributes their evidence and verdicts. The target name
comes from the adapter's own per-target `--list` loop.

## Event kinds

### `derive`

A test was discovered. Creates a claim, or — when `renamed_from` is present —
remaps an existing claim's identity, keeping the same `claim_id` and all its
history. `body_hash` is the 32-byte hash of the test's body as 64 lowercase hex
chars; both it and `renamed_from` are omitted when absent.

Rename detection happens in `attest derive` (Phase 3), which holds both the old
and new discovery sets. The fold never detects a rename — it applies the
already-resolved result.

```json
{"kind":"derive","ts":1,"claim_id":"c_00000000010W3GE1R70W3GE1R7","test_identity":{"target":"agentrec--import_claude","fn_path":"ac3_zero_bytes"},"body_hash":"abababababababababababababababababababababababababababababababab","renamed_from":{"target":"agentrec--import_claude","fn_path":"old_name"}}
```

### `evidence`

A captured run, joined to the turn it happened inside — the prompt → diff → test
run chain. `turn_id` is the open turn (null when there was none), `dirty` records
whether the working tree had uncommitted changes, `output_blob` is the CAS hash
of the captured output. `result` is a `StructuredResult`: `outcome` is what
libtest reported (`passed` / `failed` / `ignored`), `recipe_invalid` is a cause
(`build` / `missing` / `ignored` / `harness`) when the run was unusable rather
than informative, `parse_failed` is the parser's fail-closed flag, and
`raw_blob` is the CAS hash of the retained raw output.

**An author's own run is evidence and can never produce `CONFIRMED`** —
`evidence` and `verdict` are disjoint kinds written by disjoint code paths.
`StructuredResult` deliberately carries no verdict candidacy.

```json
{"kind":"evidence","ts":3,"claim_id":"c_00000000010W3GE1R70W3GE1R7","turn_id":"t_01ARZ3NDEKTSV4RRFFQ69G5FAV","dirty":true,"output_blob":"deadbeef","result":{"identity":{"target":"agentrec--import_claude","fn_path":"ac3_zero_bytes"},"outcome":"passed","recipe_invalid":null,"parse_failed":false,"raw_blob":null}}
```

### `verdict`

The result of an independent replay in an extracted tree pinned at
`replay_commit`. The only kind that can reach `CONFIRMED`.

```json
{"kind":"verdict","ts":5,"claim_id":"c_00000000010W3GE1R70W3GE1R7","verdict":"confirmed","replay_commit":"abc123"}
{"kind":"verdict","ts":6,"claim_id":"c_00000000010W3GE1R70W3GE1R7","verdict":"recipe-invalid","cause":"missing","replay_commit":"abc123"}
{"kind":"verdict","ts":7,"claim_id":"c_00000000010W3GE1R70W3GE1R7","verdict":"flaky-observation","replay_commit":"abc123"}
{"kind":"verdict","ts":8,"claim_id":"c_00000000010W3GE1R70W3GE1R7","verdict":"claim-false","replay_commit":"abc123"}
```

### `stale`

The daemon's note-taking: a write landed in a claim's coverage scope. The daemon
appends these and nothing else — it never executes a test.

```json
{"kind":"stale","ts":9,"claim_id":"c_00000000010W3GE1R70W3GE1R7","cause":"file-write","path":"cli/src/importcmd.rs"}
{"kind":"stale","ts":10,"claim_id":"c_00000000010W3GE1R70W3GE1R7","cause":"coverage-incomplete","scope":"cli/src/**"}
```

### `human`

A manual card review outcome: `yes` / `no` / `skip` (the `y`/`n`/`s` keypresses
of `attest review`), with an optional note. Manual attestation is always an
explicit keypress; observed usage only pre-fills a card.

```json
{"kind":"human","ts":11,"claim_id":"c_00000000010W3GE1R70W3GE1R7","answer":"yes","note":"checked by hand"}
```

### `manual-declare`

The only hand-authored event: an un-testable criterion's text, `blocking` or
`fyi`. `fyi` never nags.

```json
{"kind":"manual-declare","ts":12,"claim_id":"c_00000000010W3GE1R70W3GE1R7","text":"the README install line is correct","severity":"blocking"}
```

## State machine

`fold_claims(&[AttestEvent]) -> FoldResult` is a deterministic fold and a **dumb
applier**: it detects nothing, decides nothing, and schedules
nothing. Every transition below is a mechanical consequence of an event already
written. Two further consequences of that, both deliberate:

- A `derive` whose `test_identity` differs from the claim's current one and
  carries **no** `renamed_from` still remaps that claim's identity, silently.
  The fold cannot tell an unannounced rename from a writer correcting itself,
  and guessing would be worse than applying what was written. `attest derive`
  (Phase 3) is what decides.
- `body_hash` **merges**: a later `derive` carrying `null` never erases a hash a
  previous `derive` recorded. Absence means "this derive computed no hash", not
  "the test has no body".

| From | Event | To | Note |
|---|---|---|---|
| — (first sight of the claim id) | `derive` | `DERIVED` | mints the claim |
| — (first sight of the claim id) | `manual-declare` | `DECLARED` | manual criterion awaiting a human |
| any established status | `derive` | unchanged | identity, body hash and index update; a re-derive only re-asserts that the test exists |
| any established status | `manual-declare` | unchanged | text and severity recorded; a hand-authored event must not demote a machine verdict |
| `DERIVED` | `evidence` | `EVIDENCED` | an author's run stops here, always |
| `EVIDENCED` and later | `evidence` | unchanged | counted in history |
| any | `verdict: confirmed` | `CONFIRMED` | the only route to `CONFIRMED` |
| any | `verdict: claim-false` | `CLAIM_FALSE` | |
| any | `verdict: recipe-invalid` | `RECIPE_INVALID{cause}` | retryable; a later `confirmed` still reaches `CONFIRMED` |
| any | `verdict: flaky-observation` | `FLAKY` | statistical |
| any | `human` | `HUMAN{answer}` | |
| any | `stale` | status unchanged, `STALE` overlay set | |
| `CLAIM_FALSE` | **any event of any kind** | `CLAIM_FALSE` | the exception below overrides every row above |

**`CLAIM_FALSE` is permanent against every later event kind**, not only later
verdicts: a `human` answer, a `manual-declare`, an `evidence` capture, a
re-`derive` and a `stale` all leave a refuted claim exactly where it is, and are
counted in `ClaimHistory` instead. A permanent refutation that one human
keypress could erase would not be permanent.

`stale` is included in that list on purpose: a refuted claim refuses every
verdict, and verdicts are the overlay's only exits (§ "The `STALE` overlay") — so
an overlay set on one could never clear, and Phase 4 would re-verify a permanent
refutation forever. A claim that is stale when it becomes `CLAIM_FALSE` has the
overlay dropped, since a permanent verdict is an established state.

**Gate blocking:** only `CLAIM_FALSE` blocks. See "Gate blocking, precisely".

### The `STALE` overlay

**This is the single statement of the rule; every other document points here.**

`stale` sets the overlay. It is dropped by a verdict that **establishes a
state** — `confirmed` or `claim-false`. `recipe-invalid` and `flaky-observation`
leave it set, and `stale` on an already-refuted claim never sets it.

This **narrows** the spec's Architecture line, which said the overlay "drops on
the next verdict" without saying what the two non-establishing verdicts do. They
are exactly the "we still do not know" outcomes, and spec decision 5 says stale
MORE when unsure. Narrowed 2026-09-01, orchestrator-ratified under decision 5;
the founder may override. The spec line carries the same amendment.

## The identity index

`FoldResult` returns the `test_identity → ClaimId` index alongside the states
(`claim_for(&TestIdentity) -> Option<&ClaimId>`). It is part of the contract, not
internal bookkeeping: `attest derive` (Phase 3) needs exactly this lookup to
resolve a rename — it must find the OLD claim's id to write into the
`renamed_from` event. The identity a rename moved away from is removed.

The index answers "which claim owns this identity **now**", not "every claim
carrying it". Two claims can declare one identity — `derive(B, x)` then
`derive(A, x)`, a writer bug — and then the index holds only `x → A` while `B`
still carries `x` as its `test_identity`. The fold does not evict `B`'s
identity: a dumb applier has no basis for deciding which writer was wrong.

`ClaimState.test_identity` is filled from a `derive`, and — when no `derive` has
been seen yet — from an `evidence` event's `StructuredResult`, so an
out-of-order or partially-read log still attributes. It stays absent only for a
claim seen exclusively through kinds that carry no identity at all: a `verdict`,
`stale` or `human` arriving before its `derive` (a torn tail, or two writers
interleaving), and — the one legitimate permanent case — a `manual-declare`
claim, which describes a criterion no test covers and therefore has no test
identity to carry.

## Gate blocking, precisely

`ClaimStatus::blocks_gate()` answers for the **status alone**, and only
`CLAIM_FALSE` returns true. The gate's other blocking condition — an unanswered
or rejected `blocking` manual item — is not a status: it is the gate reading
`manual_severity` against the claim's `human` answer, and it lands in Phase 5.

## Verdict naming

One concept, three casings, no fourth: the **wire kind** is `flaky-observation`,
the **fold state** is `FLAKY`, and spec decision 4's word "flaky" names that
state.
