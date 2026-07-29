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

Judgment call: `agentrec import claude`'s turn segmentation buckets a
session's transcript into candidate turns at each genuine user-authored
prompt boundary (never a live-recording boundary signal, since none
exists for imported history) — an approximation, not a protocol
guarantee; `files_complete: false` is the honest marker that covers any
imprecision here.
