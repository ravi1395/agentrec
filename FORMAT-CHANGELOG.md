# Format changelog

Tracks additive, wire-visible changes to `agentrec-core/src/record.rs`'s
protocol types (`SignalEvent`, `TurnRecord`, `FileEntry`, `EpochRecord`,
`LogRecord`). Every entry here is additive-only within the current major
protocol version (PROTOCOL.md's versioning rule): existing readers must
tolerate an unknown field, and a record that never sets a new field must
serialize byte-identically to its pre-change form.

Conformance fixtures for these fields are **not** created yet — the
protocol freeze is Phase 2.1 (spec decision 5). Until then, these fields
are additive but UNFROZEN: their shape can still change before 2.1 locks it.

## Phase 2.0 P2 — `imported`, `files_complete` (2026-07-29)

Added to `TurnRecord`, immediately after `merges` and before `files`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub imported: Option<bool>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub files_complete: Option<bool>,
```

- `imported`: `None` on every live-recorded turn (the daemon's `persist` in
  `cli/src/daemon.rs` and `undo`'s turn-append path never set it) — no key
  appears on the wire for those records, so this is a strict no-op for
  existing consumers and existing `log.jsonl` lines. `Some(true)` on a turn
  produced by `agentrec import claude` (no `--dry-run`, i.e. persistence
  mode — see `cli/src/importcmd.rs::persist`).
- `files_complete`: `Some(false)` alongside every `imported: Some(true)`
  turn — the source transcript's opaque tool calls (Bash, Task, …) and any
  activity outside file-producing tool results are never captured, so the
  turn's `files` list is a known-partial accounting, not a complete one.
  `None` on every live-recorded turn, same as `imported`.
  **Extended (P2 integration-gate fix round, BLOCKER 1):** also covers
  file-producing entries dropped because `filePath` is not lexically
  under the session's `cwd` (`ScopeCounters::skipped_out_of_cwd` in
  `cli/src/importcmd.rs`, real-corpus measured at 507/2,170 = 23.4% of
  non-sidechain file-producing entries) — these are refused, not
  imported, so `files_complete: false` already covered them correctly in
  spirit, but the loss used to be invisible (no counter, no FileEntry, no
  stderr line) before this fix.

Consumers:
- `cli/src/fmt.rs::turn_list_line` (used by `log`) renders a `partial file
  list (imported)` clause when `files_complete == Some(false)`.
- `cli/src/readcmds.rs::undo` refuses (before any working-tree write, exit
  1) when `imported == Some(true)` and any selected file entry has
  `before: null` on a non-`create` op — provenance-only, never fabricated.
  Distinct wording from the `withheld`/`skipped`/modified-since refusal
  messages.
- `baseline_unknown` (an existing `FileEntry` field) keeps its live
  first-observation-only meaning and is never set `true` for an
  import-missing-`before` entry — those stay `before: null` with
  `baseline_unknown` absent/`false` (spec decision, P1.md "Pinned
  decisions" #7).
- `cli/src/cmds.rs::status_report`'s rich-rate window (FOUNDER DECISION,
  P2 integration-gate fix round, Fix 4): imported turns are excluded from
  the trailing-20-agent-turn window entirely, the same shape as the
  pre-existing git-tool exclusion. An imported turn carries `grade:
  "rich"` without any hook ever having fired — recapturing P3's goldens
  at integration moved the rich-rate from 67% to 86% purely because
  imported turns filled the window, which would let a bulk import mask a
  genuinely broken hook (the rich-rate metric's whole purpose). `turns:`
  (the total agent-turn count `status` also reports) is UNCHANGED by this
  — only the rich-rate window's membership narrows; imported turns still
  count toward the total. This regenerated `status.golden`'s rich-rate
  line only (`86% over trailing 7` -> `83% over trailing 6`); no other
  golden file changed.

Judgment call: `agentrec import claude`'s turn segmentation buckets a
session's transcript into candidate turns at each genuine user-authored
prompt boundary (never a live-recording boundary signal, since none
exists for imported history) — an approximation, not a protocol
guarantee; `files_complete: false` is the honest marker that covers any
imprecision here.

Known limitation (not fixed this round, tracked here per D9): `import
claude`'s deterministic turn id is a function of `(session_id,
turn_index)` only. If a session CONTINUES after being imported (the
transcript file grows with new turns past the last `turn_index` already
persisted), a later import run reuses the same set of ids for the
already-seen prefix and never revisits it — new turns appended past the
previously-seen `turn_index` boundary in an already-imported session are
picked up correctly (new `turn_index` values), but if the underlying
transcript file's EARLIER turns are ever re-segmented differently by a
future importer change (e.g. a `turn_index` boundary shifts), old and new
ids diverge silently. Relevant to the already-embargoed "durable archive"
claim (see Status doc): re-imports are not a stable substitute for one.

## Phase 2.0 P2 fix round — `FileEntry.after_synthesized` (2026-07-29)

Added to `FileEntry`, as the last field (after `skipped_reason`):

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub after_synthesized: Option<bool>,
```

Founder decision 2 (P2 fix round): `agentrec import claude` sometimes has
no `content` field on a transcript's tool-result entry (Edit-tool shape
carries `oldString`/`newString`, not the resulting file), so `after` is
DERIVED by applying that substitution to `before` (itself possibly a T2
git-blob approximation, or entirely absent). This is not observed data,
and comparing it against the real on-disk file and reporting a mismatch
as "human or external edit" — `cli/src/readcmds.rs::modified_cause`'s
prior behavior — fabricates attribution.

- `Some(true)` only when `after` was actually derived this way AND a
  real ref made it onto the wire (never set when `skipped` nulled the
  ref out, since there's nothing on the wire to misread at that point).
  `None` everywhere else, including every live-recorded entry — no key
  appears on the wire for those, so this is additive/no-op for existing
  consumers and existing `log.jsonl` lines.
- `cli/src/readcmds.rs::modified_cause` returns a distinct, honest cause
  ("imported turn's after-state was derived (not observed) — cannot
  attribute this difference") for a synthesized entry, checked before
  the later-agent-turn/recording-gap signals — an unobserved comparison
  point isn't made trustworthy by either of those being present.
- `cli/src/readcmds.rs::print_entry` (used by `diff`) prints an explicit
  "(after-state DERIVED ..., not observed)" notice before the unified
  diff for a synthesized entry, instead of presenting it as recorded
  fact.
- Mitigating factor, not a reason to skip this: synthesized bytes are
  never used as an undo *source* — `undo` re-snapshots real on-disk
  content before reverting, so this defect was false attribution, never
  data loss.

## Phase 2.0 P2 fix round — documented, not fixed, limitations

Recorded here per the reviewer's D8/D9/D10/D11 findings — none of these are
wire-format changes, but all bear on the durability/re-import claims this
file otherwise documents:

- **D8**: `load_log`'s tolerant parsing (a bad line is skipped, never
  fatal) means a torn LAST line in `log.jsonl` — reachable only via power
  loss, NOT a `kill -9` (see `importcmd.rs::persist::run`'s comment) —
  makes `import claude`'s idempotency check miss that turn's id and
  re-append a duplicate on the next run.
- **D9**: `import claude`'s deterministic id is `(session_id,
  turn_index)`-only; see the "Known limitation" note above this section.
- **D10/D11**: CAS writes happen before the idempotency check (an
  `appended: 0` re-run still touches the object store and re-shells every
  T2 git command), and each turn takes `log.lock` individually rather than
  once for the whole batch. Both are performance/posture items — the
  append-only invariant holds either way.
