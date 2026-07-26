# Plan: honesty-fixes round — 4 phases, branch `fix/honesty-round` (off `fix/ignore-rebuild-gate`)

Founder-directed: fix all five items surfaced by the rebuild-gate round's gate report. Item 1 is
lifted verbatim from `2026-07-25-agentrec-churn-honesty-round.md` Phase 3 (that plan keeps the other
eight phases). Item 3 (Linux CI) is not a code phase — it is a push, done last, after everything is
green locally.

Conflict order: PROTOCOL.md > IMPLEMENTATION.md > SPEC.md > this plan.

## Baseline

`cargo test --workspace -- --test-threads=3` → **395 passed, 0 failed, 1 ignored** at `897f584`
(orchestrator-verified twice this session). clippy `-D warnings` + fmt clean, debug and release.

## Executor protocol

Sonnet implementer per phase, **sequential on the shared tree** — `cmds.rs` appears in three of four
phases and `state.rs` in two, so parallel worktrees would clobber. Orchestrator re-runs the full
suite independently after every phase. Binding **fable skeptic** done-gate at the end, in an isolated
worktree; `/goal`: the work is not done until that gate passes. claimd declare-first, one claim per
criterion, **before** implementing. Every criterion names the **neuter** that must turn a *named*
test RED; a criterion that survives its neuter is vacuous and is rejected, not renegotiated.

Merge order 1 → 2 → 3 → 4. Phase 2 and 3 both touch `state.rs`; 3 depends on 2 landing first.

## Decisions log

1. **Eviction keeps hard-delete.** Archive-on-evict frees zero disk (archives live under
   `.agentrec/` on the same filesystem) while *reporting* bytes freed, and eviction fires from
   `status` — a read verb run habitually — so it would grow an unbounded archive as a side effect of
   reading status. The deliverable is a **complete protect-set**, not a softer delete.
2. **No daemon-liveness refusal on eviction.** The daemon runs 24/7 under launchd here; a liveness
   gate makes the budget fiction and routes users to `purge --snapshots-before`, which hard-deletes
   legitimate history.
3. **`read_state` degrades per-field, and says so.** A parse failure must never silently reset
   `signal_offset` — that replays the entire signal inbox. Unknown/corrupt fields fall back
   individually and increment a counter that `status`/`doctor` surface.
4. **The reload counter becomes epoch-scoped** (open question 1, option (a)), reversing the
   default-by-omission taken last round. Lifetime-cumulative meant `ignore: reloaded 4821 time(s)`
   would eventually occupy a line in the daily-driver surface whose budget the question called
   contested.

---

## Phase 1 — `enforce_budget` protect-set (fixes live data loss)

**Description:** `retention::enforce_budget` builds its keep-set from **parsed** `TurnRecord`s and
hard-deletes (`store.remove` → `fs::remove_file`), while `purgecmd::referenced_hashes` — the raw,
deliberately non-parsing `sha256:` byte-scan over `log.jsonl` + `open.json` + `memory.jsonl` — is
this repo's own stated safety standard. Eviction protects neither in-flight (`open.json`) nor pinned
(`memory.jsonl`) refs, and drops torn-line refs entirely. Its sole caller is `cmds::status_report`,
a read verb with no liveness refusal — so **running `agentrec status` on an over-budget store can
destroy an in-flight turn's snapshot**.

**Files:** `agentrec-core/src/retention.rs` (edit), `cli/src/cmds.rs` (edit),
`cli/src/purgecmd.rs` (mech — widen `referenced_hashes` visibility).

**Changes:**
- `enforce_budget` gains `extra_protected: &HashSet<String>`, subtracted from `candidates`
  immediately before the remove loop. Core stays dependency-free; the CLI caller supplies the set.
- `status_report` harvests via `purgecmd::referenced_hashes(root)` **after** candidate computation,
  as late as possible before the remove loop, to narrow the live-daemon window. That ordering is
  load-bearing and gets a SAFETY comment in the style of `purgecmd`'s.
- The over-budget status message gains a **protected-bytes attribution clause**: protecting more
  shrinks `evicted.bytes`, and "over budget, 0 freed" with no explanation is exactly the dishonest
  status class fixed in the 2026-07-17 round.

**Acceptance criteria:**
- [ ] A blob cited only by `open.json` and one evictable old turn survives. Fixture **must be RED
      pre-fix**: the blob is in no kept turn and its mtime is safely older than `pass_start`, or the
      A3(c) freshness guard rescues it vacuously. Neuter: drop `open.json` from the harvest →
      `eviction_keeps_open_turn_blob` RED.
- [ ] A blob cited only by a `memory.jsonl` pin survives, and `verify`'s pin-diff still renders
      afterwards. Neuter: drop `memory.jsonl` → `eviction_keeps_pinned_blob` RED.
- [ ] A blob cited **only by a torn line** (truncated mid-JSON, valid `sha256:` inside) survives —
      in `log.jsonl` and, separately, in a truncated `open.json`. Neuter: build the protect-set from
      `load_log`/serde instead of the raw scan → `eviction_keeps_torn_line_refs` RED. This is the
      criterion that lifts eviction to purge's guarantee.
- [ ] Call-site wiring proven, not just the core fn: an integration-level `status` run on an
      over-budget store with an open turn keeps the blob. Neuter: pass an empty set at the call site
      → `status_eviction_keeps_open_turn_blob` RED **while the core unit test stays green**. This is
      the anti-vacuity keystone — the change has two seams and a core-only test survives a caller
      that forgets the argument.
- [ ] Over-budget `status` where every candidate is protected explains why 0 was freed, naming
      pinned/in-flight. Neuter: delete the attribution clause → `status_attributes_protected_bytes` RED.
- [ ] Existing A2/A3(c)/A5 retention tests and `status_prints_over_budget_notice` unchanged and
      un-weakened.

**Expected test outputs:** `cargo test -p agentrec-core retention::` → +4; `cargo test -p agentrec
--test integration status_` → +2. Workspace → **401 passed, 0 failed, 1 ignored**.

---

## Phase 2 — `read_state` degrades per-field instead of resetting everything

**Description:** `state::read_state` chains two `.ok()`s into `unwrap_or_default()`, so **one**
unparseable field silently resets the whole `State` — `pid`, `signal_offset`, `snapshot_failures`,
`io_failed` — with no counter and no diagnostic. A reset `signal_offset` **replays the entire signal
inbox**. Pre-existing, but demonstrated live by the previous round's own AC3 neuter, and every
honesty counter in this codebase exists to prevent exactly this shape of silent loss.

**Files:** `cli/src/state.rs` (edit), `cli/src/doctorcmd.rs` (edit), `cli/src/cmds.rs` (edit).

**Changes:**
- Parse into a tolerant intermediate (`serde_json::Value` → per-field extraction, or
  `#[serde(default)]` on every field plus a deny-nothing policy) so an unparseable *field* costs that
  field, never the file. A wholly unreadable or non-JSON file still degrades to default — that case
  is genuinely unrecoverable — but it must be **counted**, not silent.
- New persisted counter `state_parse_failures` plus the name of the last field that failed.
- `status` surfaces a non-zero count; `doctor` gains a check that reports it and names the file.
  Advisory severity — it must not flip `doctor`'s exit code, which is this repo's deploy gate.

**Acceptance criteria:**
- [ ] A `state.json` whose `signal_offset` is a string (not a number) preserves every **other**
      field — `pid`, `snapshot_failures`, `io_failed` — and leaves `signal_offset` at its default,
      incrementing the counter. Neuter: restore the whole-struct `unwrap_or_default()` →
      `state_survives_one_bad_field` RED.
- [ ] `signal_offset` specifically: a corrupt *other* field must not reset it. Seed offset 4096 +
      a corrupt `io_failed`, assert the offset is still 4096. Neuter: same → `corrupt_field_does_not_replay_signal_inbox` RED.
- [ ] A wholly unparseable `state.json` still yields defaults **and** a non-zero counter, and
      `status` says so. Neuter: drop the counter increment on that path → RED.
- [ ] `doctor` reports the condition, exits **0**, and `ok` stays true. Neuter: make it a Fail →
      `doctor_state_parse_advisory_never_flips_exit` RED.
- [ ] Every existing `state.rs` test passes un-weakened; a healthy `state.json` round-trips
      byte-identically.

**Expected test outputs:** `cargo test -p agentrec state::` → +4; `cargo test -p agentrec --test
integration doctor_` → +1. Workspace → **406 passed, 0 failed, 1 ignored**.

---

## Phase 3 — epoch-scope the reload counter; make `--json` tell the whole truth

**Description:** Three disclosed-but-unfixed honesty gaps from the rebuild-gate round, all in the
`status` surface, landing together because they touch the same two files.

**Files:** `cli/src/cmds.rs` (edit), `cli/src/state.rs` (edit), `cli/src/main.rs` (mech — flag conflict).

**Changes:**
- **Open question 1 answered (option a):** the reload line is scoped to the **current daemon epoch**.
  Keep the lifetime total in `state.json` (it is real history), add an epoch-scoped figure reset when
  the daemon starts, and render the epoch figure. Rationale in the same commit: a lifetime-cumulative
  `reloaded 4821 time(s)` in the daily-driver surface is what option (a) existed to avoid.
- **`status --json` carries the DEGRADED fields** — `snapshot_failures`, `prompt_put_failures`,
  `io_failed`, plus phase 2's parse-failure counter. A monitoring script must not see *less* than the
  text surface; that is the exact inversion of this round's purpose.
- **`status --ack-degraded --json`** either emits JSON describing what was acknowledged, or clap
  rejects the combination loudly. It must not print prose under a `--json` flag.

**Acceptance criteria:**
- [ ] After a daemon restart, the rendered reload figure restarts from zero while the lifetime total
      in `state.json` is preserved. Neuter: render the lifetime figure → `status_reload_line_is_epoch_scoped` RED.
- [ ] `status --json` on a DEGRADED store carries every field the text banner reports. Neuter: drop
      one field from the payload → `status_json_carries_degraded_fields` RED, naming the field.
- [ ] `status --ack-degraded --json` produces parseable JSON (or exits non-zero with a clap conflict
      message) and never prose on stdout. Neuter: restore the early text return →
      `ack_degraded_json_is_not_prose` RED.
- [ ] Bare `status` output is unchanged for a healthy store — pin it, since three renderers now read
      the same state.

**Expected test outputs:** `cargo test -p agentrec cmds::` → +3; `cargo test -p agentrec --test
integration status_` → +1. Workspace → **410 passed, 0 failed, 1 ignored**.

---

## Phase 4 — the untested asymmetry: narrowing while events are pending

**Description:** The gate named this as the round's coverage gap. Paths already admitted to `pending`
under the older, **wider** rules are still staged at the next flush even if a mid-debounce
`.gitignore` edit now ignores them — the over-record mirror of the documented residual window. The
module comment describes only the under-record direction. Magnitude is one extra snapshot; the defect
is that nothing describes or pins it.

**Files:** `cli/tests/integration.rs` (edit), `cli/src/daemon.rs` (edit — comment only, no
production-code change).

**Changes:**
- Live-daemon test: write a path, and **within the debounce window** add a `.gitignore` rule that
  ignores it; assert the observed behavior (recorded once, then never again) and pin it as
  deliberate rather than accidental.
- Extend the `maybe_rebuild` call-site comment to describe **both** directions, so the next reader
  does not have to rediscover the asymmetry.

**Acceptance criteria:**
- [ ] The test pins the current behavior with an in-window positive control, and its name states
      which direction it covers (`narrowing_mid_debounce_still_stages_pending_paths`). No neuter is
      claimed — this pins existing behavior rather than proving a fix, and the AC says so rather than
      dressing it as a proof.
- [ ] A **second** mutation of the same path after the rule lands is **not** recorded — that half
      does have a neuter: revert Phase 1 of the rebuild-gate plan (move `maybe_rebuild` back inside
      `if settled || capped`) → RED.
- [ ] The comment describes both directions and does not overstate either.

**Expected test outputs:** `cargo test -p agentrec --test integration -- --test-threads=3` → +1.
Workspace → **411 passed, 0 failed, 1 ignored**.

---

## Delivered

| Phase | Commit | Suite | Note |
|---|---|---|---|
| 1 — `enforce_budget` protect-set | `2d1bd78` | 395 → **401** | on target |
| 2 — `read_state` per-field | `7474e26` | **407** | +1 over target, an extra real test kept |
| 3 — epoch counter + honest `--json` | `22b085f` | **412** | |
| 4 — narrowing asymmetry pinned | `6234612` | **413** | |

Every figure re-run by the orchestrator independently of the implementer. clippy `-D warnings` + fmt
clean on debug and release; the Phase 1 test seam is `#[cfg(debug_assertions)]`-gated and `strings`
on the release binary contains no `AGENTREC_TEST`. 15 claims declared per criterion **before**
implementing, each at its phase's parent commit, all evidenced at exit 0.

**Three plan defects the implementers caught, all disclosed rather than worked around:**
1. **Phase 1's obvious implementation is a trap.** Passing the full `purgecmd::referenced_hashes`
   as `extra_protected` protects every validly-referenced hash and **permanently defeats budget
   eviction** — it broke `status_prints_over_budget_notice`. The correct cut is narrower:
   `open.json` + `memory.jsonl` scanned raw unconditionally, plus only those `log.jsonl` hashes on
   lines that **fail to parse**. Validly-parsed lines are already covered by `enforce_budget`'s own
   age-ordered walk. The plan did not say this; it should have.
2. **Phase 2's taken decision was wrong.** `#[serde(default)]` on every field rescues a *missing*
   field only — a struct-level `from_str::<State>` still fails outright the moment one *present*
   field has the wrong type, which is exactly what every criterion required to survive. The
   implementer used the plan's stated fallback (`serde_json::Value` + per-field extraction) and said
   so, rather than silently switching.
3. **Phase 4's own named neuter is structurally vacuous.** Reverting the loop-tick move does **not**
   red the test: `gitignore_dirty` is tracked independently of `pending`, and the criterion itself
   requires the `.gitignore` edit to land before the target's flush — which is what consumes the flag
   in *either* call-site position. Substituted a neuter of the **trigger** (deleting
   `*gitignore_dirty = true`), which reds at an added precondition assert. Third time in three rounds
   that an AC could not have proven itself as written.

## Then: item 3 — the Linux CI leg (not a code phase)

Push `fix/honesty-round` so `.github/workflows/ci.yml`'s existing matrix (macOS-14 + Ubuntu-22.04 +
Ubuntu-24.04) runs everything. Every piece of evidence in the rebuild-gate round and in this one is
**macOS/FSEvents**; the timing margins and the `1..=10` rebuild bound are reasoned safe (failures fall
RED, inotify widens the margins) but unobserved. This also clears the older owed Linux leg for the
`unreadable`/`io_failed` producers. Report the run URL and the per-job result; a red Linux leg is a
finding for this round, not a separate one.

**OUTCOME: DONE, and it paid for itself immediately — see the round record in CLAUDE.md.** Run
locally in a Colima Linux VM (`rust:1-bookworm`, non-root, real inotify at
`max_user_watches=1048576`) rather than via CI, because `ci.yml` has no `workflow_dispatch` and a PR
was declined. **The first-ever Linux run found a shipping product defect**, a macOS-tuned test bound
that could not discriminate its own neuter on Linux, and — in the fixes for those — two fabricated-
attribution regressions. Four commits: `034c883`, `247df9e`, `f4bca8a` (plus the nonce at `942985d`).
**Correction to this plan's own reasoning, measured:** the note below (and the Phase-2 review it came
from) claimed "inotify widens the margins" and that Linux moves both sides of the rebuild bound in
the safe direction. That is **backwards**. FSEvents coalesces 40 writes into ~3 batches; inotify
delivers them individually across many POLL ticks, so Linux's *correct* count (13–21) is far higher
than macOS's (3–4) — higher, in fact, than macOS's *neutered* count (15–18). One global constant was
therefore mathematically impossible, and `elapsed/POLL` is unusable as a derivation because
`recv_timeout` returns immediately whenever a message is queued. Shipped as a `cfg(target_os)` split.

**Superseded — the original text, for the record:** The branch is pushed
(`origin/fix/honesty-round`), but `.github/workflows/ci.yml` fires only on `push: branches: [main]`
and on `pull_request` — there is no `workflow_dispatch` — so **a branch push alone runs nothing**.
The only route is a PR, and because this branch stacks on two earlier unmerged branches
(`fix/blame-attribution-and-noise-folding` → `fix/ignore-rebuild-gate` → here), a PR to `main` would
carry all three rounds at once. Offered: draft PR to `main`, PR onto the parent branch, or skip.
**Founder chose skip.** So the Linux leg remains **owed**, and every timing margin in this round and
the previous two is macOS/FSEvents evidence only. Same posture the repo already carries for the
`unreadable`/`io_failed` producers — recorded, not pretended.

## Edge cases considered

- **P1:** blob cited only by an open turn's `before` (never re-`put`, so the A3(c) mtime guard cannot
  rescue it); `sync_journal` failing silently and leaving `open.json` stale; torn `open.json` under
  power loss; a fixture whose blob is accidentally fresh enough to be rescued (vacuity).
- **P2:** `state.json` empty, truncated mid-write, valid JSON but wrong types, unknown extra fields
  (must be tolerated — PROTOCOL §10 posture), file unreadable by permissions.
- **P3:** zero-turn store; healthy store (no DEGRADED fields at all); `--json` combined with
  `--utc`/`--explain`; a `state.json` predating the epoch field.
- **P4:** FSEvents coalescing the write and the `.gitignore` edit into one batch (the test must
  tolerate both interleavings or assert only the stable half); non-git fixture (forbidden — real
  `git init` everywhere).

## Open questions

1. **P2 shape:** per-field `#[serde(default)]` on the existing struct, or parse via
   `serde_json::Value` and extract field-by-field? The former is less code and covers missing/wrong-type
   fields; the latter also survives a field that is structurally valid JSON but semantically junk.
   Taken: **the former**, unless the implementer finds a case it cannot cover — say so rather than
   silently switching.
