# agentrec — v1 specification

*Status: draft for extraction from Sutra. Companion doc: PROBLEM.md.*

## Overview

`agentrec` is a local-first, tool-agnostic recorder for coding-agent turns. It watches a repository, segments agent activity into **turns**, snapshots the files each turn touched, and exposes six CLI verbs to query and reverse that history. The engine core is extracted from Sutra's turn engine (`turns.rs`, battle-tested in daily personal use) — but the extraction is honest about what it loses: inside an editor, human buffer saves are ground truth; standalone, they are inference. The v0.2 capture design (start/stop bracketing, git-operation boundaries, epoch records, gitignore filtering) exists precisely to compensate for those lost signals — see REVIEW.md F1/F2/F4 for why each piece is load-bearing.

## Goals

Record every agent turn in a repo regardless of which tool or model produced it. Attribute any line change to a turn, with prompt context where available. Make any snapshotted turn safely reversible, refusing to touch anything modified since. Represent recording gaps honestly rather than blaming confidently across them. Define an open format other tools can emit natively.

## Non-goals (v1)

No GUI (Sutra is the reference client). No cloud sync, dashboard, or telemetry — nothing leaves the machine. No multi-repo aggregation. No agent orchestration, policy enforcement, or eval scoring (later layers, same substrate). No Windows in v1 (macOS + Linux).

## The v1 product in use

Install, initialize, forget:

```
curl -fsSL https://raw.githubusercontent.com/ravi1395/agentrec/main/install.sh | sh   # or: brew install ravi1395/agentrec/agentrec · npm i -g agentrec · cargo install agentrec
cd myrepo && agentrec init        # one command: writes .agentrec/ + gitignore entry, detects Claude Code, installs UserPromptSubmit + Stop hooks (idempotent merge), registers + starts the service unit, begins recording — and prints how to reverse all of it
agentrec doctor                   # optional: verifies the whole chain end-to-end (hooks · daemon · signals · store), pass/fail with remedies
```

The headline install path is the prebuilt-binary installer, not cargo — the target user is a JS/Python developer running agents, not a Rust developer (D39). `agentrec uninstall` is the symmetric exit: hooks and service removed, `.agentrec/` archived, never deleted. A tool asking to watch every file you write must make leaving as easy as arriving.

Nothing about the agent workflow changes — the developer runs Claude Code, Cursor, anything, exactly as before. The product is invisible until the bad moment:

```
$ agentrec log                    # git turns hidden by default; --all shows them
t_…R8MB · rich · claude-code · 15:12 · 1 file · "tighten retry backoff"
t_…Q2KX · bare · — · 15:03 · 3 files
t_…N4VD · rich · claude-code · 14:47 · 6 files · "migrate auth middleware to tok…"

$ agentrec blame src/auth.ts:42
t_…N4VD · claude-code · "migrate auth middleware to tokens" · edited since (human)

$ agentrec diff t_…N4VD           # unified diff of exactly that turn
$ agentrec undo t_…N4VD           # per-file checklist; modified-since files excluded by default
$ agentrec status                 # store size · gaps · rich-rate 94%
```

Six verbs (`record`, `log`, `diff`, `blame`, `undo`, `status`), two hooks, no account, no cloud, nothing leaves the machine. `status` shows store size, recording gaps, and the rich-rate (fraction of turns with attribution — the health stat that catches silent hook breakage). v1 ends there deliberately — import, MCP (including user-gated agent-driven undo), git trailers, and editor surfaces are Phases 1–3 (ROADMAP.md). The `mcp_destructive` config key is parsed but unused in v1 so Phase 2 is a flag-flip, not a migration.

## Architecture

Two pieces:

**Daemon (`agentrec record` / service).** Per-repo watcher process, installed as a launchd/systemd user unit by default. Detects turn boundaries, snapshots touched files into the object store, appends turn and epoch records to the log. Noise filtering is **gitignore-derived**: everything `.gitignore` excludes is excluded from watching (that's `.next/`, `.venv/`, `__pycache__/`, `coverage/`, and every ecosystem's build dir, maintained by the ecosystem itself), plus a built-in denylist (`.git` contents except HEAD/index/refs, `.agentrec`, `node_modules`, `target`, `dist`) and user globs. On Linux, inotify watch-limit exhaustion fails loudly with the sysctl remedy — silent partial watching is a hole in a ledger and is treated as a startup error.

**CLI.** Reads the log and object store; never needs the daemon running to query history.

## Capture strategy

Two capture sources, producing two **turn grades**:

### Rich turns (hook-sourced, bracketed)

Agent tools with lifecycle hooks emit signal lines (required fields: `ts` in unix ms, `tool`; optional: `event`, `session`, `transcript` — PROTOCOL §4). Claude Code is the day-one integration and installs **two** hooks: `UserPromptSubmit` (start) and `Stop` (stop). Start+stop **bracketing** is what makes attribution correct standalone: while a bracket is open, quiet-window closure is suppressed (agents pause >10 s mid-turn constantly — thinking, test runs), and any bare turns mistakenly closed inside the bracket are retroactively folded into the rich turn on stop. Prompt text comes from the start signal, with transcript extraction as fallback. Crash rule: a start with no stop closes at the last observed mutation, grade rich, `truncated: true`.

### Git turns (ref-change boundary)

`git checkout`, `pull`, `stash pop`, and `rebase` rewrite hundreds of worktree files; without special handling every branch switch becomes a garbage 400-file "unknown" turn and the log drowns (REVIEW.md F2). The daemon therefore watches `.git/HEAD`, index, and refs — not to depend on git, but to *classify* it: a mutation burst coinciding with a ref transition is recorded as a rich turn with `tool: "git"`. `log` hides git turns by default (`--all` shows them); `blame` reports them honestly ("changed by git checkout at 14:02").

### Bare turns (quiet-window fallback) — unattributed activity windows

For activity with no signal and no git correlation, the daemon falls back to heuristic segmentation: a mutation burst followed by a **10-second quiet window**. A bare turn asserts only "these files changed together in this window." It is *not* evidence of agent activity — a human saving in vim produces the identical fs signature. Bare turns are never presented as agent turns, never fabricate a `tool`, and never weaken safety checks.

### The two change predicates

**modified-since** (content hash ≠ turn's `after` hash) is absolute and gates `undo` — it catches later agent turns, human edits, git operations, and recording gaps alike. **human-edited-since** (modified-since AND not covered by any *rich* turn) is display-level context in `blame` output only. Bare turns never count as coverage — an unattributed window can't certify a human didn't edit. Destructive operations key exclusively on modified-since.

### Epochs and gaps

The daemon appends epoch records on start and clean shutdown. Any uncovered interval is a recording gap; `blame` crossing a gap, or finding current content unexplained by the log, says **"attribution stale — recording gap"** instead of confidently naming the last recorded turn. A provenance tool that guesses across gaps torches its own trust (REVIEW.md F4).

## Data model

### Storage layout

```
.agentrec/
  config.toml          # scrub rules, TTL, ignore globs
  signal.jsonl         # hook-emitted turn signals (append-only inbox)
  log.jsonl            # turn records, append-only (the canonical history)
  objects/             # content-addressed snapshots, sha256, fan-out dirs
    ab/cdef1234...     # 10 MiB per-file cap (larger files: boundary recorded, content skipped)
```

`.agentrec/` is **gitignored by default** (the daemon writes the ignore entry on init). Teams may opt in to committing `log.jsonl` for shared blame — an explicit decision, never a default, because rich turns reference prompt text.

### Turn record (log.jsonl, one JSON object per line)

```json
{
  "v": 1,
  "id": "t_01J8ZC3T9GV5H2Q4W7E6R8N0MB",   // machine-scoped ULID; display truncates
  "grade": "rich",                  // "rich" | "bare"
  "started": "2026-07-05T14:03:11Z",
  "ended": "2026-07-05T14:04:02Z",
  "tool": "claude-code",            // optional on bare
  "model": "claude-fable-5",        // optional
  "session": "s_9f2c",              // optional; groups turns
  "root": "/repo",                  // worktree root; parallel agents = distinct roots
  "prompt_ref": "sha256:ab12...",   // optional; object-store ref to prompt text
  "prompt_excerpt": "add rate limiting to login",  // ≤120 chars, post-scrub
  "files": [
    { "path": "src/auth.ts", "before": "sha256:...", "after": "sha256:...", "op": "modify" },
    { "path": "src/limits.ts", "before": null, "after": "sha256:...", "op": "create" }
  ]
}
```

`before: null` = file created; `after: null` = deleted; content not captured to the store (over-cap, a snapshot write that failed, or a file that was unreadable at record time) = `"skipped": true` with a `"skipped_reason"` naming which (PROTOCOL §5); secret-denylisted = `"withheld": true` (never snapshotted, never revertible). Epoch records (`type: "epoch"`, daemon start/stop) interleave with turns so gaps are first-class data. Format is versioned (`v`); the normative spec is published in-repo as `PROTOCOL.md` so other tools can emit it.

## Prompt posture

Prompts are the product's magic and its liability. Rules, enforced from the first commit:

1. **Local-only.** Prompt objects live in `.agentrec/objects/` on the developer's machine; no network code path exists in v1. Honest caveat instead of a "provably" claim: the store is plaintext on disk — readable by any local process (including recorded agents) and carried along by Dropbox/Time Machine backups. Snapshots are therefore defended too: secret-file patterns (`.env*`, `*.pem`, `credentials*`, key files) are never snapshotted (`withheld: true`).
2. **Scrub before persist.** A scrub pipeline runs on prompt text before it touches disk: regex rules for known secret shapes (API keys, tokens, connection strings) plus an entropy detector for high-entropy substrings; matches are replaced with `[redacted:reason]`. Pluggable via `config.toml` for org-specific patterns.
3. **Retention.** `ttl_days` defaults to 90 (shipped default, not opt-in); snapshot store has a size budget (default 2 GiB) with oldest-snapshot eviction; `agentrec purge` deletes prompt objects (and optionally snapshots) past TTL or wholesale; `agentrec status` shows store size and what's eligible. Deleting a prompt object leaves the turn record valid — `prompt_ref` dangles gracefully.
4. **Excerpt discipline.** CLI output shows the post-scrub excerpt only; full prompt requires explicit `agentrec show <turn> --prompt`.
5. **Inbox included.** Hooks scrub prompt text *before* emitting the signal line; the planted-secret test covers `signal.jsonl` as well as the log and object store (D38). No pre-scrub byte persists anywhere.

## Durability posture

A flight recorder that can lose the flight is not a flight recorder. These are v1 commitments (IMPLEMENTATION.md D34–D37), not future work:

1. **The ledger is fsynced.** Turn-close and epoch appends hit disk (`sync_all`) before the daemon considers them written; kill -9 or power loss immediately after a close loses nothing. Signal-inbox appends are exempt (hot path, reconstructible).
2. **Snapshots are atomic and race-free.** Blob writes go tmp → fsync → rename → dir-fsync, with per-writer-unique tmp names so concurrent snapshot threads can never interleave into a torn blob. Integrity is verified against the content address on every read — a corrupted object is reported, never silently served.
3. **Failure is loud, never silent.** Snapshot write failures (disk full, permissions) are distinguished from over-cap skips at the API level, counted persistently, and surfaced as a `DEGRADED — N snapshot writes failed` banner in `status`. `undo` tells the truth about *why* a file has no snapshot. A safety net that is silently absent is worse than none.
4. **Undo is adversarially tested, in public.** A nightly torture harness throws randomized interleavings of agent bursts, human edits, git checkouts, kill -9, and undo/redo at the real binary and asserts two invariants after every undo: never touch a modified-since file; every undo is itself undoable. The harness is public and linked from the README; seven consecutive green nights gate the launch.
5. **The store is private.** `.agentrec/` is 0700, files 0600, umask-independent. Post-scrub is not an excuse for world-readable prompts.

## CLI

| Verb | Does | Example output |
|---|---|---|
| `record` | Run the daemon (or via service unit) | `recording /repo (hooks: claude-code · git-aware · fallback: quiet-10s)` |
| `log` | List turns, newest first; git turns hidden unless `--all` | `t_01J8…MB rich claude-code 14:04 2 files "add rate limiting to login"` |
| `diff <turn>` | Unified diff of a turn's changes | standard diff, per file |
| `blame <file>[:line]` | Which turn last touched this file/line (line-level in v1) | `t_01J8…MB · claude-code · "add rate limiting..." · edited since (human)` — or `attribution stale — recording gap` |
| `undo [turn]` | Revert a turn's changes, per file | interactive checklist; excludes modified-since files by default; refuses skipped/withheld. Bare `undo` = panic mode: targets the last non-git rich turn, preview-first (D42) |
| `status` | Health + store report | store size, TTL/eviction eligibility, gaps, rich-rate %, DEGRADED banner on snapshot write failures |

Setup commands (not query verbs): `init`, `uninstall`, and `doctor` — the last runs a full-chain diagnosis (hooks, daemon, signal freshness, store health, watch limits, permissions) with pass/fail and a one-line remedy per check (D41). Read-verb output uses relative times and TTY color by default, with `--utc`, `--json`, `NO_COLOR`, and `--explain` for glossary annotations (D43).

`undo` semantics mirror Sutra's rollback dialog: per-file selection, exclusion of modified-since files unless explicitly overridden, refusal of skipped/withheld files, never a silent bulk revert. `blame` at line granularity is computed by diffing the turn's before/after snapshots and **ships in v1** — the launch gif is a line-level query, so the launch build answers it (REVIEW.md S5).

## Relationship to Sutra

Sutra's `turns.rs` engine is extracted into a standalone crate (`agentrec-core`); Sutra depends on the crate and becomes the reference GUI — turn headers, AI-stitch marginalia, and the rollback dialog all render `agentrec` data. One turn engine, two frontends. Sutra's `.sutra/turn-signal.jsonl` and `.sutra/turns/objects` migrate to the open `.agentrec/` layout.

## v1 scope — cut list

Shipping: daemon + service install, six verbs (line-level blame included), Claude Code start+stop hook integration with bracketing, git-turn classification, gitignore-derived filtering, epoch/gap honesty, scrub + snapshot withholding + default TTL + purge, durability hardening (fsynced ledger, atomic race-free snapshots, DEGRADED status honesty, 0700 store, nightly public undo torture harness), the accessibility layer (installer matrix, one-command init + symmetric uninstall, doctor, panic undo, relative times/color/--explain), PROTOCOL.md v0.2.
Cut: GUI, dashboards, multi-repo, Windows, any second agent-tool integration (v2 pins Codex — see INTEGRATIONS.md).

## Launch plan (honest schedule: ~8 weekend-equivalents)

The old "two weekends" figure contradicted this project's own acceptance criteria (REVIEW.md S7). Three milestones, each ending in a usable state:

**M1 — Record (weekends 1–3):** extract `agentrec-core`; watcher with gitignore filtering; bracketed capture against Claude Code start+stop hooks; git-turn classification; epoch records; `record`/`log`/`status` working. Dogfood on the Sutra repo from the first build — M1's exit test is that one week of the dogfood log is *readable* (git noise hidden, rich-rate ≥ 90 %).
**M2 — Answer (weekends 4–6):** `diff`, line-level `blame` with gap-staleness output, `undo` with the modified-since rail; scrub + withholding; crash/kill-9 hardening; fsync durability + snapshot-failure taxonomy (D34/D35) land here, *before* undo ships — undo must never run against a store that can lie.
**M3 — Ship (weekends 7–8+):** service install, TTL/eviction defaults, undo torture harness running nightly (launch gates on 7 consecutive green nights — D36), the accessibility layer (installer matrix + one-command init/uninstall + doctor + panic undo + comprehension defaults, D39–D43), README led by the line-level blame gif plus the "how we try to break it" harness section, PROTOCOL.md v0.2 published; Show HN + r/LocalLLaMA, titled around the question: *"Who broke my repo — me or the agent?"*
**Success metric:** within 30 days — meaningful stars, a handful of "does it support <tool>?" issues, and one external person running it on a repo that isn't yours. The dogfood readability test in M1 is the self-kill tripwire: if my own log is noise with all fixes in, the extraction thesis is wrong (REVIEW.md, steelman).

## Resolved design decisions

*(Formerly open questions — resolved in IMPLEMENTATION.md's decision register.)* Signed entries: yes — the `sig` field is reserved in PROTOCOL.md §5 now and implemented in v3 as Ed25519 over JCS-canonicalized records (D20). Bare-turn attribution: stays honest with `unknown` — process sniffing is unreliable and erodes the trust posture (IMPLEMENTATION.md §3, AC C). License: Apache-2.0 for core and CLI (D17).
