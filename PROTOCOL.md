# The agentrec protocol — v0.2

An open, local-first format for recording coding-agent turns: who changed what, under which prompt, with which tool and model, and whether it changed since. Any tool may emit it; any tool may consume it. This document is the normative spec; SPEC.md describes the reference implementation.

*v0.2 incorporates the hostile-review fixes (REVIEW.md): start/stop bracketing with retroactive merge, epoch records, ULID ids, millisecond timestamps, the two-predicate change model, git-operation turns, and withheld snapshots. Published 2026-07-10 alongside M2 "Answer" + M3 "Ship": additive-only wire changes since the draft are the turn-record `merges` field (retroactive-merge bookkeeping, §5) and the file-entry `baseline_unknown` flag (§5). Snapshot-failure/DEGRADED state is implementation-local (`.agentrec/state.json`) and is deliberately NOT part of the wire protocol — see the note at the end of §5. A further additive field, the file-entry `skipped_reason` open enum (§5), landed pre-1.0-freeze to give `skipped: true` an honest per-cause explanation (`over_cap` / `io_failed` / `unreadable`, with `policy` reserved for a future producer) instead of consumers assuming a single cause. So did the epoch-record `dropped_signals` count (§5, D51), which makes an offline-dropped tool bracket visible instead of silent without replaying it.*

Requirement words **MUST**, **SHOULD**, **MAY** are used per RFC 2119.

## 1. Design principles

**Local-first.** All artifacts live in the repository's `.agentrec/` directory. The protocol defines no network component. **Tool-neutral.** No field privileges any vendor; `tool` is a free string. **Graceful degradation.** Every enrichment field is optional; a consumer MUST render a useful result from the minimum record. **Append-only.** Logs are JSONL, append-only; history is corrected by appending, never rewriting. **Privacy by construction.** Prompt text MUST pass a scrub step before persistence (§9).

## 2. Terminology

**Turn** — one contiguous unit of agent activity, from initiation to stop, touching zero or more files.
**Grade** — `rich` (boundary from an authoritative signal, attribution attached) or `bare` (boundary inferred heuristically). A bare turn is an **unattributed activity window** — it asserts only "these files changed together in this window," never who changed them. Consumers MUST NOT present bare turns as agent activity, and bare turns MUST NOT weaken change-safety checks (§5, two predicates). Authoritative boundaries include agent-tool signals **and git ref changes** — a mutation burst coinciding with a `.git/HEAD`/index/ref transition is recorded as a rich turn with `tool: "git"`.
**Emitter** — anything that announces turns: an agent tool's hook, a wrapper, the agent itself.
**Recorder** — the process that consumes signals, watches the filesystem, snapshots files, and writes turn records (reference: the `agentrec` daemon).
**Consumer** — anything that reads records: CLI, editor plugin, PR bot, another agent.

## 3. Repository layout

```
.agentrec/
  config.toml      # scrub rules, ttl_days, ignore globs (implementation-defined keys allowed)
  signal.jsonl     # emitter → recorder inbox (§4), append-only
  log.jsonl        # canonical turn records (§5), append-only
  objects/         # content-addressed blob store (§6)
```

"Append-only" for these two files means precisely: writers only ever append; no line is mutated, reordered, or rewritten in place; history is corrected by appending. A recorder MAY additionally reclaim `signal.jsonl` by removing whole lines it has already consumed (strictly before its persisted consumption offset), provided the removal is atomic, the removed lines are archived first, the unconsumed tail is preserved byte-identical, and the consumption offset is rebased in the same operation (reference implementation: `purge --signals-consumed`, decision D48). Emitters MUST NOT assume stable byte offsets across such a reclaim; consumers of `log.jsonl` get the stronger guarantee — turn records are never removed, only superseded by appended corrections (the reference implementation's narrow `--log-duplicates` repair for a historical daemon defect removes only same-id records carrying an identical file-entry set — order-independent, because crash-recovery's journaled file order need not match the steady-close order; record metadata such as `ended` or the prompt excerpt is not compared — and predates this clarification).

`.agentrec/` SHOULD be gitignored by default. Shared team history is a valid, explicit opt-in — but never by committing the live `log.jsonl` (append-only files merge badly and id collisions were only solved for time-ordering, not for merge conflicts). The sharing mechanism is per-writer sidecar exports (`shared/log.<writer-id>.jsonl`, one append-only file per human/machine, merged at read time ordered by `started` — same model as L3 tool sidecars, D21). Prompt objects SHOULD NOT be committed in any arrangement.

## 4. Signal format — emitter → recorder

One JSON object per line in `signal.jsonl`:

| Field | Type | Req | Meaning |
|---|---|---|---|
| `v` | int | MUST | Signal schema version; this document defines `1` |
| `ts` | int (unix **milliseconds**) | MUST | Event time |
| `tool` | string | MUST | Emitting tool, e.g. `"claude-code"` |
| `event` | string | SHOULD | `"stop"` (default) or `"start"`; stop-only emitters are conformant but degraded (see bracketing) |
| `session` | string | MAY | Tool-native session id |
| `transcript` | string (path) | MAY | Path to the tool's transcript for prompt extraction |
| `files_written` | array of string (absolute paths) | MAY | On `"stop"` only: files the emitting tool **itself wrote** during the turn, as known to the tool (e.g. its own edit-tool invocations). An L2+ emitter SHOULD populate it — the emitter's own record is the only author-level signal; a recorder cannot distinguish a human save from an agent write at the filesystem layer. A recorder MAY use it for per-file attribution and MUST NOT infer authorship for observed mutations absent from it. Absence of the field means "emitter did not declare", never "no files written" |
| `emitter_turn` | string | MAY | On `"start"`/`"stop"` only: a stable turn identity the **emitter** assigns (e.g. Codex's own hook `turn_id`) — not a recorder-side id. A recorder MAY use it to detect a signal the emitter resent (its own retry, or a resend racing a recorder restart) by `(tool, event, session, emitter_turn)`, and MAY use a mismatch between a `stop`'s `emitter_turn` and an open bracket's own to recognize the `stop` as not closing that bracket. Absence means "emitter has no such id" (every Claude Code hook today), never "no upstream turn" — a recorder MUST fall back to ordinary tool-based start/stop matching whenever either side of a comparison lacks it. Two signals both lacking `emitter_turn` are never considered a match on that basis |
| `model` | string | MAY | The model the emitting tool was running, when the emitter has it (e.g. Codex's hook payload). A recorder MAY use it directly for model attribution and MUST fall back to any other model-attribution mechanism it already has (e.g. transcript parsing) whenever this is absent. Absence means "emitter did not declare", never "no model" |

Example — a Claude Code Stop hook is a one-liner:

```json
{"v":1,"ts":1751724242183,"tool":"claude-code","event":"stop","transcript":"~/.claude/projects/x/y.jsonl"}
```

A recorder MUST treat signal lines as authoritative turn boundaries and MUST tolerate unknown fields. A recorder SHOULD fall back to heuristic segmentation (quiet-window; reference uses 10 s) when no emitter is present — such turns are grade `bare`.

**Bracketing and retroactive merge.** A `start` signal opens a bracket for its tool; the matching `stop` closes it. While a bracket is open, the recorder MUST suppress quiet-window closure for mutations inside it — agents pause mid-turn (thinking, test runs) far longer than any quiet window. On `stop`, any heuristic (bare) turns the recorder closed between `start` and `stop` MUST be retroactively folded into the resulting rich turn. Stop-only emitters cannot be bracketed; their turns MAY be split by the quiet window, and consumers MUST expect that.

**Crash rule.** A `start` with no matching `stop` (tool crash, Ctrl-C): the recorder MUST close the bracket at the last observed mutation inside it, grade `rich`, with `truncated: true` — attribution is kept (the start signal is authoritative) but the record admits the end is inferred.

**Memory-candidate signal (additive, memory v1).** A distinct signal shape carries a fact-extraction hint rather than a turn boundary:

| Field | Type | Req | Meaning |
|---|---|---|---|
| `type` | string | MUST | `"memory-candidate"` |
| `fact` | string | MUST | Extracted fact text (pre-scrub is the emitter's responsibility; the recorder scrubs before persistence, as with prompts) |
| `pins` | array of string (paths) | MAY | Paths only — the recorder hashes them at ingestion; an emitter never computes or sends hashes |

```json
{"v":1,"ts":1751724242183,"tool":"claude-code","type":"memory-candidate","fact":"repo uses pnpm, not npm","pins":["package.json"]}
```

A memory-candidate line has no `event` field and MUST NOT be treated as a `start` or `stop` signal — a recorder MUST route it to memory ingestion before any start/stop dispatch, never into turn-boundary handling. Consumers (including recorders) MUST tolerate this shape appearing interleaved with ordinary signal lines in `signal.jsonl`.

## 5. Turn record format — the canonical log

One JSON object per line in `log.jsonl`:

| Field | Type | Req | Meaning |
|---|---|---|---|
| `v` | int | MUST | Record schema version; this document defines `1` |
| `type` | string | SHOULD | `"turn"` (default when absent) or `"epoch"` (below) |
| `id` | string | MUST | `t_<ULID>` — machine-scoped ULID: globally unique, lexically time-ordered, merge-safe across writers and branches. Display MAY truncate (`t_01JX…A4`). |
| `grade` | string | MUST | `"rich"` or `"bare"` |
| `truncated` | bool | MAY | Rich turn whose end boundary was inferred (crash rule, §4) |
| `started` / `ended` | RFC 3339 | MUST | Turn boundaries (best-effort `started` on bare turns) |
| `root` | string | MUST | Worktree root; parallel agents ⇒ distinct roots |
| `files` | array | MUST | Touched files (below); MAY be empty |
| `tool` | string | SHOULD on rich | Producing tool; omit rather than guess on bare |
| `model` | string | MAY | Model identifier as reported by the tool |
| `session` | string | MAY | Groups turns into a session |
| `prompt_ref` | string | MAY | `sha256:<hex>` object ref to post-scrub prompt text |
| `prompt_excerpt` | string | MAY | ≤120 chars, post-scrub |
| `merges` | array of string | MAY | Ids of earlier `bare` turns folded into this rich turn by retroactive merge (§4). Consumers MUST treat a merged turn's own record as superseded by the turn that lists it — its files are already reflected here. Omitted/empty when nothing was merged. |
| `sig` | object | MAY | Reserved for signed entries (future minor version) |

File entry: `{ "path": "src/auth.ts", "before": "sha256:...|null", "after": "sha256:...|null", "op": "create|modify|delete", "skipped": true?, "skipped_reason": "over_cap|io_failed|unreadable|policy"?, "withheld": true?, "baseline_unknown": true?, "link_kind": "symlink"?, "attribution": "<producer-defined>"? }` — `before: null` means created, `after: null` means deleted, `skipped: true` means the file's content was NOT captured to the object store — this can happen for more than one reason (below) — `withheld: true` means content matched the secret-file denylist and was deliberately not snapshotted (§9). Both flagged kinds record the boundary only and are never revertible. `baseline_unknown: true` marks a `modify` entry whose `before` is `null` not because the file was created, but because the recorder first observed it mid-session (e.g. daemon started after the file already existed and was then touched) — the true prior content is unrecoverable; consumers MUST render this distinctly from a normal create and MUST NOT infer the file was new.

**`link_kind` (additive, v1.0+).** An open string enum naming the kind of the filesystem object at `path` **at snapshot time**, when it is something other than an ordinary file. Absent on every ordinary-file entry and on every entry recorded before this field existed — an absent `link_kind` therefore means "ordinary file, or unknown", never "proven ordinary file". Defined value:

- `symlink` — the path was a symbolic link. Recorders MUST NOT follow symbolic links; the entry's `before`/`after` hashes address the **link target string**, not the content of the pointed-to file.

Consumers MUST refuse to *act* on an entry carrying any non-absent `link_kind`, including a value they do not recognize — **refuse to act, not refuse to parse**. The line MUST still deserialize, and MUST NOT be counted as corruption. Two concrete rails, both mandatory for destructive operations:

- Restoring a `link_kind` entry's `before` bytes as file content is forbidden. Those bytes are a path string; writing them produces an ordinary file where a link was, and the link is not recoverable from the record.
- A destructive operation MUST additionally test the *live* path with an `lstat`-equivalent (a check that describes the link, not its target) and MUST refuse when the path is currently a symbolic link — **independently of `link_kind`**. Records written before this field existed carry no `link_kind` and never will, so the live check is the only guard for them; conversely a deleted link cannot be `lstat`ed, so `link_kind` is the only guard there. Neither check subsumes the other.

Neither refusal may be overridden by a caller opt-in such as MCP `allow_modified` (§8) or a CLI equivalent: those loosen the *modified-since* predicate only. Writing to a path that is a symbolic link follows the link and truncates a file that appears nowhere in the operation's plan, and a read-back verification follows the link too — so the corruption verifies clean and reports success.

**`attribution` (additive, v1.0+).** An open string enum carrying per-file attribution — which actor within a turn is believed to have produced this file's change. **Writer-optional**: producers MAY omit it entirely, and the value set is defined by the attribution producer, not by this document. Consumers MUST tolerate any value, including unknown ones, MUST treat its absence as "not attributed" rather than as any particular actor, and MUST NOT gate a destructive operation on it — undo safety keys on *modified-since* (below) and on the refusal rails above, never on attribution.

**`skipped_reason` (additive, v1.0+).** An open string enum naming WHY `skipped` is `true`; absent on any entry recorded before this field existed, and MAY be absent even on a new entry if an implementation doesn't distinguish causes. Consumers MUST treat any value outside the set below (including a genuinely unknown future value) as plain `skipped: true` with an unrecorded cause — never match this enum exhaustively, and never fail closed on an unrecognized value. Defined values:

- `over_cap` — content exceeded the per-object snapshot cap (§6).
- `io_failed` — the snapshot write itself failed at record time (e.g. disk full, permission error on the object store).
- `unreadable` — the file could not be read at record time (e.g. permission denied, removed between the change event and the read).
- `policy` — **reserved, no producer in this version.** A future rate/size-based demotion feature (deliberately not specified yet) will emit this value; it is named now so a v1.0-frozen protocol already has room for it without a breaking change.

When the underlying bytes were actually read by the recorder (`over_cap`, `io_failed`) but not durably stored, implementations SHOULD still record `after` as the content hash — it is honestly known even though the blob itself doesn't exist in the object store. This keeps *modified-since* (below) meaningful for a skipped file that is never subsequently edited. When the bytes were never read at all (`unreadable`), `after` MUST stay `null` — implementations MUST NOT fabricate a hash for content they never saw. Regardless of `after`, a `skipped: true` entry is never revertible — the `skipped` gate MUST be checked, and MUST refuse, before any *modified-since* comparison is used to decide revertibility.

**Local-only state, not wire protocol.** Snapshot-failure counters and the resulting DEGRADED status (`agentrec status`, `agentrec status --ack-degraded`) live entirely in the implementation-local `.agentrec/state.json` and are never written to `log.jsonl` or `signal.jsonl`. They are reference-implementation operational state, not part of the interchange format, and are intentionally out of scope for this document.

**Epoch records.** Recording gaps must be representable or blame lies. The recorder MUST append `{"v":1,"type":"epoch","event":"start"|"stop","ts":"<RFC 3339>"}` on daemon start and clean shutdown. Any interval not covered by an epoch is a **gap**: consumers MUST treat attribution across a gap as stale and say so.

| Epoch field | Type | Required | Notes |
|---|---|---|---|
| `v` | integer | MUST | Schema major (§10). |
| `type` | string | MUST | `"epoch"`. |
| `event` | string | MUST | `"start"` or `"stop"`. |
| `ts` | string | MUST | RFC 3339. |
| `dropped_signals` | integer | MAY | Additive (D51). Turn-boundary signals found in the pre-startup gap and **dropped**, counted by the recorder's own routing rules, which MAY over-report what live routing would in fact have produced (see below). SHOULD be omitted when `0`. Absent means only that the recorder reported nothing: consumers MUST treat it as `0` for arithmetic, and MUST NOT infer "no loss" — a pre-D51 or non-conforming recorder drops boundaries without counting them. |

**`dropped_signals` (additive, D51).** A recorder MUST NOT replay a `start`/`stop` signal that arrived while it was not running — doing so would mint an empty turn misdated to recorder boot. That drop is correct and unchanged, but a silent drop is indistinguishable from no activity at all: a tool session whose entire bracket elapses while the recorder is down otherwise leaves no trace whatsoever. A recorder that drops such signals SHOULD report **how many** it dropped, as `dropped_signals` on the `start` epoch record it appends for that boot. It is a **count only** — the dropped signals' prompt text and other payload MUST NOT be captured or persisted, and a recorder MUST NOT synthesize a turn, a file entry, or a prompt blob from a dropped signal. **What counts as a turn boundary is defined by the recorder's own live routing**, not by a fixed field test: a recorder that reports this count MUST derive it from its own routing rules, and MUST NOT count a gap line those rules classify as anything other than a start or a stop whenever that classification is decidable from the line alone (a memory-candidate, §4; a tolerated-unknown `type`, §10). Routing that also turns on live recorder state a gap cannot supply — a dedup key, an open bracket — is out of reach of a replay scan, so a recorder MAY count a line its live routing would in fact have swallowed; among the lines it read, the count therefore leans toward over- rather than under-reporting, which is the safe direction for a field whose purpose is to break silence. *(Informative, not a conformance requirement — this reference implementation's line-decidable routed set is `{event == "start"} ∪ {no "type" field}`, with memory-candidate lines excluded first: a typed `start` opens a turn live and so is a counted drop, while a typed non-`start` line is tolerated-unknown and is not a boundary. It over-reports in one known case: its live `emitter_turn` pre-filter suppresses a duplicate resend or a mismatched stop before routing, and that verdict is a function of engine state the recorder deliberately does not reconstruct at boot. A conforming recorder with different routing counts its own set, not this one.)* The field is meaningful on `event: "start"` only; a `stop` epoch is a clean shutdown, which scans no gap. Writers SHOULD omit a count of `0` from the wire — this implementation does, so every epoch line written before this field existed is byte-identical — and readers MUST accept an explicit `0`, which is semantically identical to absence. A zero or absent value asserts only that the recorder counted nothing — never that no activity was lost: bytes lost to an inbox truncation, for example, are never read and so cannot be counted. Consumers rendering gap staleness MAY mention a nonzero count; they MUST NOT treat it as an activity record.

**Two predicates — never conflate them.** (1) *modified-since(turn, file)*: current content hash ≠ the turn's `after` hash. Cheap, absolute, catches everything — later agent turns, human edits, gaps, git operations. This predicate, and only this predicate, gates undo safety. (2) *human-edited-since(turn, file)*: modified-since holds AND the change is not covered by any rich turn record. This predicate is display-level context for blame output. Bare turns MUST NOT satisfy "covered" for predicate 2 — an unattributed window never certifies that a human didn't edit. Consumers MUST NOT use predicate 2 for anything destructive.

## 6. Object store

Blobs stored at `objects/<first2>/<rest-of-sha256>`, addressed as `sha256:<hex>` over raw content. Per-file snapshot cap: 10 MiB (implementations MAY raise it; they MUST then still set `skipped` semantics consistently). Prompt text is stored as ordinary objects, referenced only from `prompt_ref`. Deleting an object MUST NOT invalidate the log — consumers MUST handle dangling refs gracefully (this is how `purge`/TTL works).

## 7. Emitter conformance levels

| Level | Emits | Result |
|---|---|---|
| **L0** | Nothing | Recorder infers bare turns via heuristics; works with any tool, today |
| **L1** | Stop signals (§4 minimum) | Rich boundaries, tool attribution |
| **L2** | Signals + `transcript`/prompt access | Full attribution: prompt-aware blame |
| **L3** | Complete turn records (§5) written natively | Recorder daemon becomes optional |

L3 is the end state: agent tools writing the format themselves, with agentrec as the neutral read/verify layer. **Consumer conformance:** a consumer MUST tolerate unknown fields, MUST handle both grades, MUST handle dangling `prompt_ref`, and MUST NOT assume `tool` or `model` are present.

## 8. MCP surface

A conforming MCP server exposes the record to agents. Tools and their capability tiers:

| Tool | Tier | Notes |
|---|---|---|
| `agentrec_log` | read | List/filter turns |
| `agentrec_diff` | read | Per-turn unified diff |
| `agentrec_blame` | read | File/line → turn, attribution, human-touched-since |
| `agentrec_recall` | read | Rank-then-verify recall over pinned/derived memory facts (memory v1) |
| `agentrec_undo` | **destructive** | Gated by the user's `mcp_destructive` mode (below) |

**Destructive tier — the user decides.** Reversion is the point of the record; a recorder that can only watch mistakes is half a product. The user grants agent-driven undo per repo via `config.toml: mcp_destructive = "off" | "confirm" | "auto"` (default `"off"`):

- `off` — `agentrec_undo` is not exposed to agents.
- `confirm` — the agent MAY request an undo; the recorder holds it pending explicit human approval (CLI prompt or host approval UI) before executing.
- `auto` — the agent MAY execute autonomously via two-phase commit: the first call returns a preview (per-file diff, warnings, and a one-time `confirm_token` with 60 s TTL); a second call presenting the token executes. No single call can revert anything.

Rails that hold in every mode: files with `skipped: true` or `withheld: true` are never revertible (no snapshot content exists). Files failing the **modified-since** predicate check (§5 — content changed since the turn, by anyone or anything) are excluded unless the call sets `allow_modified: true` — honored only in `auto` mode or with human approval in `confirm` mode. Undo safety never keys on the weaker human-edited-since predicate.

**The property that makes loosening safe:** an executed undo MUST itself be recorded as a new turn (`tool: "agentrec"`, normal before/after file entries). History is append-only and snapshotted, so undo never destroys information — it moves the working tree, and is itself blame-able and reversible. With the user's standing consent, an agent can find its bad turn, revert it, and retry — the self-healing loop — while every reversion stays on the record.

## 9. Security and privacy requirements

Prompt text MUST pass a scrub pipeline (secret-shape regexes plus entropy detection; pluggable rules) before persistence; scrubbed spans are replaced with `[redacted:<reason>]`. **Snapshots are a second secret channel and MUST be defended too:** files matching a secret-file denylist (`.env*`, `*.pem`, `*key*`, `credentials*`, `id_rsa*`, etc.; pluggable) MUST NOT be snapshotted — their entries carry `withheld: true`. The object store holds raw file contents readable by any local process, including other recorded agents; implementations MUST document this and MUST ship a default retention TTL rather than unbounded default retention. Implementations MUST support retention TTL and full purge of prompt objects. The protocol defines no telemetry, no network calls, no cloud path; any implementation adding them MUST make them opt-in and say so loudly. Future signed entries (`sig`) will use detached signatures over the canonical record bytes — reserved, not yet specified.

## 10. Versioning

`v` is per-line, per-schema. Within a major version, changes are additive only; consumers MUST ignore unknown fields and MUST NOT fail on them. A major bump is a new schema, and writers MUST NOT mix majors within one file. This draft is v0.1 of the *document*; both schemas it defines are `v: 1` and freeze as protocol 1.0 per the roadmap's Phase 1 gate. The memory-candidate signal (§4) is an additive variant of the existing signal schema — it does not bump `v`.

**Reading a `v` you do not implement.** `v` is a bare integer with no minor component, so every distinct `v` is a distinct major. A consumer meeting a line whose `v` names a major it does not implement:

- **MUST refuse the line.** It is not a record this consumer can interpret.
- **MUST NOT interpret it as a known major.** Unknown *fields* are tolerated (above) because additive change is carried by fields; an unknown *major* is a different schema, and reading its fields under this major's meanings is a misread, not graceful degradation. This matters most where records gate destructive operations: a misread record is a wrong-bytes revert source.
- **SHOULD count it distinctly from corruption** in any census of skipped lines it exposes. The two prescribe different user actions — an unimplemented major means "upgrade your reader", a torn line means "your log took damage". A malformed `v` (string, float, negative, null) declares no version at all and is evidence of corruption, not of a newer producer. This is a SHOULD, not a MUST, precisely because a census is optional: a consumer that keeps no per-line census owes no counter. The reference implementation keeps one for the record schema (`log.jsonl`) and **none for the signal schema** (`signal.jsonl`), where an unsupported major is dropped indistinguishably from an unparseable line.
- **MUST read an absent `v` as major 1.** `v` is a writer MUST in §4 and §5, but this section constrains writers only; §1's graceful degradation and the legacy lines that predate the field settle the consumer's reading.

## 11. Resolved design decisions

*(Formerly open questions — resolved in IMPLEMENTATION.md's decision register; normative from protocol 1.0.)* Canonical form for signing: RFC 8785 (JCS); signatures are Ed25519 detached, carried in `sig` as `{alg, key_id, val}` (D20). `start` signals: optional forever — stop-only emitters remain fully conformant; start events exist solely to improve prompt capture (D25). L3 emitters: write sidecar logs (`log.<tool>.jsonl`) merged at read time ordered by `started` — one append-only file per writer, no lock contention (D21).
