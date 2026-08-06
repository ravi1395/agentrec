# Format changelog

Tracks additive, wire-visible changes to `agentrec-core/src/record.rs`'s
protocol types (`SignalEvent`, `TurnRecord`, `FileEntry`, `EpochRecord`,
`LogRecord`). Every entry here is additive-only within the current major
protocol version (PROTOCOL.md's versioning rule): existing readers must
tolerate an unknown field, and a record that never sets a new field must
serialize byte-identically to its pre-change form.

**The protocol is frozen at 1.0 (2026-08-06).** Every "UNFROZEN until the
Phase 2.1 protocol freeze" note in the entries below is now discharged — see
the freeze entry directly beneath this paragraph. Those notes are left in
place verbatim: they were true when written, and this file is a history.
Conformance fixtures now exist, at `docs/fixtures/conformance/`.

## `agentrec_status` added to the §8 MCP table (2026-08-06)

**No wire change** — neither the signal schema (§4) nor the record schema
(§5) is touched, and no serializer, `Cargo.toml`, or golden moves. §8's tool
table gains one row, `agentrec_status` (read tier), between `agentrec_recall`
and `agentrec_undo`.

Permitted post-freeze because it is purely additive under the rule the freeze
binds: a new row is a new tool, no existing row changes tier, name, or
meaning, and a consumer that does not know the tool simply does not call it.
Delta decision 16 names this row and 2.3's origin discriminator as the two
post-freeze additions anticipated at freeze time.

**Placement is a deliberate deviation.** The parent Phase 2 design
(`docs/superpowers/specs/2026-07-18-agentrec-phase-2-design.md`, the §8
reservation at :384-387) reserved this row for "2.2's first commit"; it lands
instead with the commit that implements the tool (plan task E3), per that same
document's own rationale that normative wire text rides the code it describes.
Recorded as gap 7 in the delta spec
(`docs/superpowers/specs/2026-08-05-phase-2-tail-codex-mcp-design.md`).

What the tool returns is `agentrec-core`'s `RepositoryHealth` plus this
server's own operational facts (recorder liveness, effective agent-undo mode,
negotiated MCP revision, and whether `config.toml` changed since startup). The
§8 row deliberately does not enumerate that payload: §8 defines the tool
surface a conforming server exposes, not one implementation's response shape.

## Protocol 1.0 freeze (2026-08-06)

No wire change. `PROTOCOL.md` moves from `v0.2` to **1.0, frozen**: within
major `v: 1`, changes are additive only, permanently. `SCHEMA_MAJOR` is
untouched and every line's `v` stays `1` — the 1.0 versions the document,
not the wire — so no existing `log.jsonl` or `signal.jsonl` is affected and
no golden moves.

What the freeze binds, for every entry below and every future one:

- A field MAY be added. No field may be removed, renamed, retyped, or have
  its meaning narrowed; no already-defined value may change meaning.
- The open string enums (`link_kind`, `skipped_reason`, `attribution`) MAY
  gain values — consumers are already required to tolerate unrecognized
  ones. `skipped_reason: "policy"` stays reserved with no producer.
- Anything not expressible additively needs a NEW major, which is a new
  schema, not an edit to this one (PROTOCOL §10).

Every previously-UNFROZEN field is therefore now frozen as shipped:
`FileEntry.link_kind`, `FileEntry.attribution`, `FileEntry.skipped_reason`,
`SignalEvent.emitter_turn`, `SignalEvent.model`, `SignalEvent.files_written`,
and `EpochRecord.dropped_signals` — all seven already described by
PROTOCOL.md — plus the four the freeze itself added rows for, immediately
below.

### Four undescribed fields documented at freeze time — RESOLVED

Four fields were already on the wire with **no row in PROTOCOL.md's §4 or §5
tables**, so a freeze of the document would not have bound them. The founder
ruled they are **inside** the freeze; the rows were written from the shipped
code rather than from the record, and are documentation of existing behavior,
**not a wire change**. No serializer, no `Cargo.toml`, and no golden moved.

| Field | Where the row now lives | Landed in | Prior marker |
|---|---|---|---|
| `SignalEvent.prompt` | §4 signal table | the original Claude Code hook emitter (`cli/src/cmds.rs::hook`), extended to Codex by `cli/src/hookcmds.rs::hook_codex` | none — it predates the UNFROZEN convention entirely, and is the oldest and least-documented of the set |
| `TurnRecord.imported` | §5 record table | the import round (Phase 2.0 P2); both importers set it — `importcmd.rs`'s `persist::persist_session_file` (Claude) and `codex::persist_session_file` (Codex) | "Additive + UNFROZEN per spec decision 5" |
| `TurnRecord.files_complete` | §5 record table | same two sites, same round | same |
| `FileEntry.after_synthesized` | §5 file-entry prose, beside `link_kind`/`attribution`/`skipped_reason` | the Phase 2.0 P2 fix round, founder decision 2; written by the Claude importer only (`persist::classify_and_resolve`) — the Codex importer never derives an `after`, so it never sets it | "Additive, UNFROZEN … founder decision 2" |

Three things the rows say that the code says and the prior record did not:

- **`prompt` is not gated on `event`.** `cmds.rs::hook` reads the payload's
  `prompt` key whichever hook fired, and `daemon.rs::signal_context` consumes
  it on `stop` as well as `start` — on a stop it becomes `observe_stop`'s
  `prompt_fallback`, which is how a stop-only emitter gets a prompt at all.
  The row says MAY-on-either-event, not "start only".
- **`prompt` serializes as an explicit `null`.** It has no
  `skip_serializing_if`, unlike every additive field added after it. The row
  pins explicit `null` as identical to absence (same posture `dropped_signals`
  takes for an explicit `0`).
- **`files_complete` discriminates "imported, so partial" and nothing finer.**
  It is `false` unconditionally on every imported turn, so it can never mean
  "entries were dropped from *this* turn". The row forbids reading it that way.

Their shapes were already pinned by the conformance corpus
(`valid/signal_start_with_prompt.jsonl`, `valid/turn_imported_partial.jsonl`),
so no fixture was added or changed — the corpus counts below are unchanged.
`docs/fixtures/conformance/README.md`'s recorded-gap section is updated to
point at the new rows.

Conformance corpus: `docs/fixtures/conformance/` — 29 fixtures across
`valid/` (17), `tolerated/` (3) and `invalid/` (9), pinned by
`cli/tests/conformance.rs`. 19 of the 29 are emitted by the real serializers
in `agentrec-core/src/record.rs` (16 of the 17 in `valid/`, plus the three
wrong-major lines). The other 10 are hand-written because no serializer can
produce them: malformed JSON, unknown fields, an unknown record `type`, and
`valid/signal_stop_protocol_example.jsonl` — PROTOCOL §4's own example line,
which omits the optional keys our emitters always write as explicit `null`.

Decision refs: spec decision 5 (freeze behind Codex 2.1), delta decision 16,
D51 (`dropped_signals`, landed immediately prior so it is inside the freeze).

## Phase 2 tail D0 — `EpochRecord.dropped_signals` (2026-08-05)

Added to `EpochRecord`, after `ts`:

```rust
#[serde(default, skip_serializing_if = "is_zero_u32")]
pub dropped_signals: u32,
```

Additive, `v` stays `1`. The key never appears on the wire when `0`, and `0`
is what every pre-existing epoch line parses to, so every existing
`log.jsonl` epoch line round-trips byte-identically
(`record.rs::epoch_dropped_signals_roundtrips_and_zero_stays_absent`).
UNFROZEN until the Phase 2.1 protocol freeze, like every field above — though
this one lands specifically so it is *inside* that freeze rather than after it.

A count of the turn-boundary signals (`start`/`stop`) the recorder found in
the **pre-startup gap** — the window of `signal.jsonl` that accumulated while
no daemon was running — and **dropped**. D7's posture is unchanged: those
signals are still never fed to the engine, because replaying a stale
start/stop would mint an empty turn misdated to daemon boot. What changes is
that the drop is no longer silent (decision D51; PROTOCOL §5).

Count only. Nothing about a dropped signal's content is captured: no
`scrub_prompt`, no `BlobStore` write, no excerpt. A tool session whose whole
bracket elapsed offline still leaves no turn and no prompt — only the fact
that N boundaries were dropped.

- **Writer: `cli/src/daemon.rs`.** `replay_pending_candidates` now returns
  `ReplayOutcome { consumed, dropped_signals }` instead of a bare `u64`, and
  counts, inside its scan loop, every parsed signal the LIVE poll loop would
  have routed to `apply_signal`'s start/stop arms. The predicate is
  `is_start() || kind.is_none()`, evaluated AFTER an unconditional
  memory-candidate exclusion — the same order the live loop uses.
  `apply_signal` tests `is_start()` before its `kind` guard, so a typed
  `start` (`{"event":"start","type":"<unknown>"}`) opens a real turn live and
  is therefore a real dropped boundary
  (`daemon.rs::replay_gap_drop_count_follows_live_routing`); a typed
  NON-`start` line is tolerated-unknown (§10) and is not counted. Deliberately
  NOT "everything the ingest branch skipped": that set also holds
  memory-candidates when the `memory_enabled` kill-switch is off, and any
  future typed non-`start` `kind`
  (`daemon.rs::replay_gap_drop_count_excludes_non_boundary_kinds`, which also
  pins that a candidate carrying `event: "start"` is routed away as a
  candidate and never counted, kill-switch either way). **Corrects
  `deb2f85`'s commit message**, which stated the predicate as `kind.is_none()`
  and recorded a mutation probe ("widen the predicate to count the whole
  else-branch → only `replay_gap_drop_count_excludes_non_boundary_kinds`
  REDs"). That verdict is **not reproducible** against this predicate — the
  probe is not inverted, its catcher moved. Re-run here: the widening still
  reds, but as `replay_gap_drop_count_follows_live_routing` (`left: 3, right:
  2` — its typed non-`start` line gets counted), while the test the probe
  named now passes, because the candidate exclusion runs ahead of the
  predicate unconditionally and `true` cannot reach a candidate.
  `daemon::run` threads `replay.dropped_signals` into the `append_epoch(&root,
  "start", …)` call that immediately follows the scan; `append_epoch` gained a
  fourth parameter and every other caller passes `0` — the `stop` call site's 0 rests on
  the argument that a clean shutdown scans no gap, NOT on coverage (no test pins it; every
  daemon integration test SIGKILLs, so a clean-shutdown `stop` epoch is never written under
  test). Wiring of the START count is proven against
  the real daemon binary, not just in-process
  (`integration.rs::live_daemon_start_epoch_carries_the_offline_dropped_signal_count`).
- **Zero is not "no loss".** Every early bail-out in the scan returns `0`
  because it read no lines at all. The `len < start` (inbox shrunk) branch is
  the one worth naming: bytes were genuinely lost there, but they were never
  parsed, so nothing can say how many boundaries they held. That loss keeps
  its own louder channel (`resync_shrunk_signal_offset` → stderr +
  `record_io_failure` → DEGRADED in `status`/`doctor`).
- **Readers: none yet.** `view::recording_gaps`/`GapKind` are untouched and no
  renderer reads the field, so every human-form golden is byte-identical. A
  `blame` clause surfacing a nonzero count is available and deliberately not
  built in this round.

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
