# Protocol 1.0 conformance corpus

Example wire lines for the two schemas `PROTOCOL.md` freezes at 1.0: the
signal schema (§4, `signal.jsonl`) and the record schema (§5, `log.jsonl`).
An external emitter or consumer can test itself against these; the reference
implementation tests itself against them in `cli/tests/conformance.rs`, whose
`MANIFEST` declares the expected verdict for every file here and is compared
to this directory by **set equality** (a deleted, added, or renamed fixture
fails the suite).

One JSON line per file. `v` is `1` on both schemas and **stays `1`** — "protocol
1.0" names the document, not the per-line version (§10).

## Classes

| Directory | Meaning |
|---|---|
| `valid/` | MUST parse. The reader yields a `SignalEvent` or a `LogRecord`. |
| `tolerated/` | MUST NOT be treated as damage. Either it parses (unknown *fields* are additive change, §10) or it is refused as "a schema this reader does not implement" — never counted as corruption. |
| `invalid/` | MUST be refused. Either a wrong schema major (§10 "MUST refuse the line") or malformed bytes. |

`tolerated/` exists because §10 draws a line that a two-way valid/invalid split
erases: a record `type` this reader predates and a `v` major it predates are
both *refused* but are **not corruption** — they mean "upgrade your reader",
where a torn line means "your log took damage". A `v` that is malformed rather
than merely unknown (string, float, negative, null) declares no version at all
and is corruption; those live in `invalid/`.

Filename prefixes route each fixture to a reader, and an unknown prefix is a
hard test failure rather than a silent skip:

- `signal_*` → `agentrec_core::record::parse_signals`
- `turn_*`, `epoch_*`, `log_*` → `agentrec_core::record::parse_log_line`

## Provenance

Everything in `valid/`, plus the three `*_wrong_major_v99` lines, is **emitted
by the real serializers** in `agentrec-core::record` — regenerate with:

```bash
cargo test --test conformance -- --ignored regenerate_conformance_fixtures
```

Hand-authored, because no serializer can produce these shapes:
`valid/signal_stop_protocol_example.jsonl` (PROTOCOL §4's own example line —
our emitters always write the optional keys as explicit `null`, but the spec
shows this shape to third parties, so the reader must accept it), everything in
`tolerated/`, and the six malformed lines in `invalid/`. Hand-authored fixtures
that drift from the real wire are this repository's known failure mode; keep
the hand-written set to shapes the serializer provably cannot emit.

The Codex full-stop and emitter-turn start fixtures also carry
`emitter_event`, the per-hook-invocation identity added by D52 so a recorder
can distinguish a blocked continuation from an exact replay without relying
on timestamp resolution.

## Closed gap — four shipped fields PROTOCOL.md now describes

An earlier revision of this file recorded these four as appearing on the wire
with **no row** in §4's or §5's tables, and left it open whether the 1.0 freeze
bound a field the document did not describe. **Resolved by founder decision:
the rows were added inside the freeze**, written from the shipped code, as
documentation of existing behavior rather than as a wire change. No fixture
here moved — the corpus already exercised all four.

| Field | Struct | Fixture | Writer |
|---|---|---|---|
| `prompt` | `SignalEvent` (§4) | `valid/signal_start_with_prompt.jsonl` | `cli/src/cmds.rs::hook` and `cli/src/hookcmds.rs::hook_codex`, post-scrub. In practice a start-event field, but **not gated on `event`** — the §4 row says MAY on either, because the daemon consumes it on a `stop` too (a stop-only emitter's prompt) |
| `imported` | `TurnRecord` (§5) | `valid/turn_imported_partial.jsonl` | `cli/src/importcmd.rs` — both importers: `persist::persist_session_file` (Claude) and `codex::persist_session_file` (Codex) |
| `files_complete` | `TurnRecord` (§5) | `valid/turn_imported_partial.jsonl` | same two sites; `false` unconditionally on every imported turn, so it means "imported, therefore partial" and never "entries were dropped here" |
| `after_synthesized` | `FileEntry` (§5) | `valid/turn_imported_partial.jsonl` | `cli/src/importcmd.rs::persist::classify_and_resolve` only — the Codex importer never derives an `after`, so it never sets it |

## `origin` — two fixtures, and the shape that has none

`origin` (§5, additive, F5 / delta decision 11) gets **one fixture per
value** — `valid/turn_undo.jsonl` (`"cli"`) and
`valid/turn_undo_mcp_origin.jsonl` (`"mcp"`) — rather than one fixture and a
note, because it is the field a consumer counting agent-initiated reverts
keys on, and a corpus showing only `"cli"` lets a reader that hardcodes that
value pass. Both are emitted by the real serializer; the writer is
`cli/src/readcmds.rs::append_undo_turn`, the single site that decides an undo
turn's shape.

The **third** shape — an undo turn with no `origin` key, which is every one
written before the field existed and which §5 says reads as `"cli"` — has no
fixture here on purpose. The serializer cannot emit it (it always writes the
field now), and hand-authoring it would put a line in `valid/` that no
producer produces, which is this corpus's stated failure mode. It is pinned
instead where the reader lives: `cli/tests/conformance.rs` strips the key
from `turn_undo.jsonl` and asserts the default, and
`cli/tests/undo_origin.rs` round-trips a key-less line byte-identically.

## Out of scope

`memory.jsonl` is **not** part of the protocol — §3's repository layout defines
`config.toml`, `signal.jsonl`, `log.jsonl`, and `objects/` only. The "memory"
fixture here is therefore the **memory-candidate signal** of §4
(`valid/signal_memory_candidate.jsonl`), which is a variant of the frozen
signal schema. The memory store's own record format is implementation-local
and is not frozen by this corpus.
