# The agentrec protocol — v0.2

An open, local-first format for recording coding-agent turns: who changed what, under which prompt, with which tool and model, and whether it changed since. Any tool may emit it; any tool may consume it. This document is the normative spec; SPEC.md describes the reference implementation.

*v0.2 incorporates the hostile-review fixes (REVIEW.md): start/stop bracketing with retroactive merge, epoch records, ULID ids, millisecond timestamps, the two-predicate change model, git-operation turns, and withheld snapshots. Published 2026-07-10 alongside M2 "Answer" + M3 "Ship": additive-only wire changes since the draft are the turn-record `merges` field (retroactive-merge bookkeeping, §5) and the file-entry `baseline_unknown` flag (§5). Snapshot-failure/DEGRADED state is implementation-local (`.agentrec/state.json`) and is deliberately NOT part of the wire protocol — see the note at the end of §5.*

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

File entry: `{ "path": "src/auth.ts", "before": "sha256:...|null", "after": "sha256:...|null", "op": "create|modify|delete", "skipped": true?, "withheld": true?, "baseline_unknown": true? }` — `before: null` means created, `after: null` means deleted, `skipped: true` means content exceeded the snapshot cap, `withheld: true` means content matched the secret-file denylist and was deliberately not snapshotted (§9). Both flagged kinds record the boundary only and are never revertible. `baseline_unknown: true` marks a `modify` entry whose `before` is `null` not because the file was created, but because the recorder first observed it mid-session (e.g. daemon started after the file already existed and was then touched) — the true prior content is unrecoverable; consumers MUST render this distinctly from a normal create and MUST NOT infer the file was new.

**Local-only state, not wire protocol.** Snapshot-failure counters and the resulting DEGRADED status (`agentrec status`, `agentrec status --ack-degraded`) live entirely in the implementation-local `.agentrec/state.json` and are never written to `log.jsonl` or `signal.jsonl`. They are reference-implementation operational state, not part of the interchange format, and are intentionally out of scope for this document.

**Epoch records.** Recording gaps must be representable or blame lies. The recorder MUST append `{"v":1,"type":"epoch","event":"start"|"stop","ts":"<RFC 3339>"}` on daemon start and clean shutdown. Any interval not covered by an epoch is a **gap**: consumers MUST treat attribution across a gap as stale and say so.

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

## 11. Resolved design decisions

*(Formerly open questions — resolved in IMPLEMENTATION.md's decision register; normative from protocol 1.0.)* Canonical form for signing: RFC 8785 (JCS); signatures are Ed25519 detached, carried in `sig` as `{alg, key_id, val}` (D20). `start` signals: optional forever — stop-only emitters remain fully conformant; start events exist solely to improve prompt capture (D25). L3 emitters: write sidecar logs (`log.<tool>.jsonl`) merged at read time ordered by `started` — one append-only file per writer, no lock contention (D21).
