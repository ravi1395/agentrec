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
appends `stale`. Every writer — the daemon included — takes the append lock
(`cli/src/attest/lock.rs`, Phase 3).

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

`fold_claims(&[AttestEvent]) -> BTreeMap<ClaimId, ClaimState>` is a deterministic
fold and a **dumb applier**: it detects nothing, decides nothing, and schedules
nothing. Every transition below is a mechanical consequence of an event already
written.

| From | Event | To | Note |
|---|---|---|---|
| — | `derive` | `DERIVED` | mints the claim in the fold's map |
| — | `manual-declare` | `DECLARED` | manual criterion awaiting a human |
| `DERIVED` | `evidence` | `EVIDENCED` | an author's run stops here, always |
| `EVIDENCED` and later | `evidence` | unchanged | counted in history |
| any but `CLAIM_FALSE` | `verdict: confirmed` | `CONFIRMED` | the only route to `CONFIRMED` |
| any but `CLAIM_FALSE` | `verdict: claim-false` | `CLAIM_FALSE` | **permanent** |
| `CLAIM_FALSE` | any `verdict` | `CLAIM_FALSE` | refused transition, counted in history |
| any but `CLAIM_FALSE` | `verdict: recipe-invalid` | `RECIPE_INVALID{cause}` | retryable; `confirmed` later still reaches `CONFIRMED` |
| any but `CLAIM_FALSE` | `verdict: flaky-observation` | `FLAKY` | statistical, never blocking |
| any | `stale` | unchanged status, `STALE` overlay set | |
| any | `human` | `HUMAN{answer}` | |

**Gate blocking:** only `CLAIM_FALSE` blocks (`ClaimStatus::blocks_gate`).
`RECIPE_INVALID` and `FLAKY` never block on their own — `recipe-invalid` queues a
retry and surfaces in `attest status`. (An unanswered blocking `manual-declare`
also blocks, but that is the gate's own read of `manual_severity`, not a status.)

### The `STALE` overlay

`stale` sets the overlay; a verdict that **establishes a state** — `confirmed` or
`claim-false` — drops it. `recipe-invalid` and `flaky-observation` leave it set.

The spec says the overlay "drops on the next verdict", which is ambiguous about
those two. They are exactly the "we still do not know" outcomes, and spec
decision 5 says stale MORE when unsure, so they do not clear it. Resolved here
because Phase 3 builds against it.

## Verdict naming

One concept, three casings, no fourth: the **wire kind** is `flaky-observation`,
the **fold state** is `FLAKY`, and spec decision 4's word "flaky" names that
state.
