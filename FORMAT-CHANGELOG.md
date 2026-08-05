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

## Phase 2 tail C1 — `SignalEvent.emitter_turn` (2026-08-05)

Added to `SignalEvent`, after `files_written`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub emitter_turn: Option<String>,
```

Additive, `v` stays `1`. The key never appears on the wire when `None`, and
`None` is what every pre-existing signal line parses to, so every existing
`signal.jsonl` line round-trips byte-identically
(`record.rs::signal_emitter_turn_roundtrips_and_absence_stays_absent`).
UNFROZEN until the Phase 2.1 protocol freeze, like every field above.

An open string carrying a stable turn identity the **emitter** assigns,
present on `start`/`stop` signals only (PROTOCOL §4). Motivating producer:
Codex's own hook `turn_id` (see `docs/verify/codex-spike.md`) — Task C2
(`agentrec hook codex`, not built in this round) is its intended writer.
Claude Code's hook emitter has no equivalent stable id today and omits the
field entirely; `None` means "emitter did not declare", never "no upstream
turn".

- **Reader: `agentrec-core::engine::TurnEngine`.** `observe_start` gained a
  fifth parameter, `emitter_turn: Option<String>`, stored on the newly
  opened bracket. A new query method,
  `TurnEngine::stop_mismatches_open_bracket(tool, emitter_turn)`, reports
  whether a stop's `emitter_turn` differs from the open bracket's own
  (both present) — `observe_stop`'s own signature and matching logic are
  untouched; a caller that gets `true` back is expected to skip calling
  `observe_stop` for that signal entirely, leaving the bracket open.
- **Writer/consumer: `cli/src/daemon.rs`.** `handle_emitter_turn_signal`
  (called from the poll loop before `apply_signal`, gated on
  `sig.kind.is_none()` so an unrecognized future `type` never enters it)
  does two things for a signal that carries `emitter_turn`: (1) restart-safe
  dedup — a resend of the immediately-previous processed
  `(tool, event, session, emitter_turn)` tuple is dropped, counted in
  `state.json`'s new `duplicate_emitter_turn_signals`, and never reaches the
  engine; (2) a mismatched stop (per `stop_mismatches_open_bracket`) is
  likewise dropped and counted in `mismatched_stop_emitter_turns`, leaving
  the bracket open. Both counters, plus the dedup key itself
  (`last_emitter_turn_key`), are additive `#[serde(default)]` fields on
  `state.json`'s `State` — itself operational, off-wire, per PROTOCOL §5's
  note — so a pre-C1 `state.json` still parses with zero counted failures
  (`state::tests::pre_c1_state_json_without_emitter_turn_fields_parses_cleanly`).
  A signal with no `emitter_turn` never enters either check: zero extra
  `state.json` reads or writes.
- Single-slot dedup, deliberately: `last_emitter_turn_key` holds only the
  most-recently-processed key, so it catches an immediately-following
  resend, not an arbitrary-history duplicate — a different signal arriving
  in between clears the slot (state.rs doc comment on the field).

## Phase 2 tail C1 fix 1 — content-aware `emitter_turn` dedup (2026-08-05)

Not a wire change (`state.json` is operational, off-wire per PROTOCOL §5) —
recorded here for continuity with the C1 entry above, which this directly
amends. Bug: Codex's `Stop` hook fires twice for one `turn_id` on a
`decision:"block"` continuation (`docs/verify/codex-spike.md`,
"Continuation semantics"), so both firings share the identical
`(tool, event, session, emitter_turn)` dedup key — identity-only dedup
silently dropped the second firing even when it carried genuinely NEW
`files_written` from `apply_patch` calls made during the continuation.

`cli/src/daemon.rs::handle_emitter_turn_signal` now additionally compares a
content fingerprint (`emitter_turn_content_fingerprint`: constant for
`start` — spike-confirmed `UserPromptSubmit` never re-fires mid-turn — and
`files_written`-keyed for `stop`) before treating a key match as a
duplicate. `state.json`'s `State` gained one more additive
`#[serde(default)]` field, `last_emitter_turn_fingerprint`, alongside
`last_emitter_turn_key`; a pre-fix `state.json` (key present, fingerprint
absent) falls back to the original identity-only verdict and self-heals on
that exact read (`state::tests::state_json_with_key_but_no_fingerprint_parses_cleanly`,
`daemon::tests::handle_emitter_turn_signal_dedups_by_identity_when_fingerprint_is_legacy_missing`).
Load-bearing test:
`daemon::tests::handle_emitter_turn_signal_second_stop_with_new_files_written_is_applied_not_dropped`.

## Phase 2 tail C2 fix 2 — `SignalEvent.model` (2026-08-05)

Added to `SignalEvent`, after `emitter_turn`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub model: Option<String>,
```

Additive, `v` stays `1`. The key never appears on the wire when `None`,
and `None` is what every pre-existing signal line parses to, so every
existing `signal.jsonl` line round-trips byte-identically
(`record.rs::signal_model_roundtrips_and_absence_stays_absent`). UNFROZEN
until the Phase 2.1 protocol freeze, like every field above.

The model the emitting tool was running, when the emitter has it.
Motivating producer: Codex's hook payload carries `model` on all three of
its lifecycle events (`docs/verify/codex-spike.md`'s field inventory);
`cli/src/hookcmds.rs::hook_codex` sets it on `UserPromptSubmit` only,
mirroring `prompt`'s existing start-only posture rather than
`emitter_turn`'s carried-on-both-signals one (`emitter_turn` is carried on
both because the daemon's dedup/mismatch logic needs it at both ends;
`model` is purely descriptive and needs no re-assertion at stop time).
Claude Code's hook emitter has no such field and omits it entirely; `None`
means "emitter did not declare", never "no model".

- **Reader/consumer: `cli/src/daemon.rs::signal_context`.** Now seeds
  `model` from `sig.model` first (mirroring the existing `prompt`
  precedence exactly), falling back to the pre-existing transcript-parse
  extraction (`parse_transcript`) only when the signal didn't declare one.
  Claude signals never set `model`, so this path is unchanged for them
  (`daemon::tests::signal_context_prefers_hook_provided_model_over_transcript`
  pins both the override and the fallback). Residual: `Recorder.models` is
  in-memory and session-keyed, so a daemon restart strictly between a
  session's `start` and `stop` loses the model for that one turn — the
  same residual class an existing restart already has for `prompt`.

## MVP periphery — `FileEntry.link_kind`, `FileEntry.attribution` (2026-08-01)

Red team round 2, finding F2. Added to `FileEntry`, at the end, after
`after_synthesized`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub link_kind: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub attribution: Option<String>,
```

Additive, `v` stays `1`. Neither key appears on the wire when `None`, and
`None` is what every pre-existing record parses to, so every existing
`log.jsonl` line round-trips byte-identically
(`record.rs::link_kind_and_attribution_absent_when_none`). Both are
UNFROZEN until the Phase 2.1 protocol freeze, like every field above.

**One coordinated change, two fields, deliberately.** `attribution` is
declared here and written by nothing on this branch — the sibling
`feat/mvp-promise` branch's D6 transcript-correlation work is its producer.
Landing the pair together fixes the wire shape once rather than twice.

- `link_kind` — open string enum (`agentrec_core::record::link_kind`). The
  only value written today is `"symlink"`, meaning the path was a symbolic
  link when it was snapshotted and the entry's `before`/`after` hashes
  address the **link target string**, not the pointed-to file's content
  (the recorder does not follow links — IMPLEMENTATION.md AC B5; probed,
  not assumed, by
  `daemon.rs::stage_symlink_records_link_kind_and_snapshots_target_string`).
  Absent means "ordinary file, **or** unknown" — never "proven ordinary
  file": records written before this field will never carry it.
  - Writer: the daemon (`cli/src/daemon.rs`, `Recorder::stage` records the
    kind, `Recorder::resolve` puts it on the entry). A `delete` observation
    keeps the mark — the record is then the only surviving evidence that the
    vanished path was a link.
  - Consumers MUST refuse to **act** on any non-absent value, including
    unrecognized future ones, and MUST still **parse** the line
    (refuse-to-act, not refuse-to-parse). `cli/src/readcmds.rs::undo`
    refuses such an entry with a `REFUSE` row before any working-tree write,
    and independently refuses when the live path is a symbolic link by
    `lstat` — the second check is the only guard for pre-`link_kind`
    records. Neither refusal is overridable by `--allow-modified`, which
    loosens *modified-since* and nothing else.
- `attribution` — open string enum, **writer-optional and written by
  nothing in this workspace**. Values are defined by the attribution
  producer, not by PROTOCOL.md. Consumers MUST tolerate unknown values,
  MUST read absence as "not attributed" rather than as any actor, and MUST
  NOT gate a destructive operation on it.

Specified in PROTOCOL.md §5 (`link_kind` / `attribution` paragraphs).

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
