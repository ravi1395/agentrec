# agentrec — Integrations: the long-term surface and the v2 scope

*Companions: PROTOCOL.md (the format every integration rides on), ROADMAP.md (when).*

## The integration thesis — four rings

How agentrec integrates with the world changes over time. Think of it as four rings, each making the next one cheaper, moving from adapters we write to integrations that write themselves:

**Ring 1 — Capture adapters (v1–v2).** Per-tool hooks and importers we build: a Stop hook here, a transcript importer there. Cost: roughly one config file per tool. This ring never scales past a handful of tools by design — it exists to prove the record is worth having.

**Ring 2 — Agent-side, via MCP (v2).** One MCP server and agentrec is inside every MCP-capable host — Claude Code, Codex, Cursor, Windsurf, next month's tool — with a single config entry. This ring inverts the relationship: agents stop being things we record and become *users* of the record — self-diagnosis via `blame`, cross-agent coordination via `log`, and user-consented reversion via `undo` (PROTOCOL.md §8). We integrate with N agents by implementing one protocol they all already speak.

**Ring 3 — Render surfaces (v2–v3).** Editors, git, PR review. Pure consumers of the open format with zero capture logic: a VS Code gutter, `Agent-Turn:` commit trailers, a PR bot. Because rendering needs only PROTOCOL.md and a JSONL file, this ring is community-buildable — we ship one flagship per surface and let the JetBrains plugin and the Neovim gutter be someone else's weekend project.

**Ring 4 — Native emitters (L3, the end state).** Agent tools write turn records themselves (PROTOCOL.md §7, Level 3), likely as sidecar logs. The daemon becomes optional; capture adapters retire; agentrec's durable role is the neutral read/verify/attest layer. Ring 4 requires no code from us at all — only a spec good enough that vendors prefer emitting it to inventing their own.

The direction of travel is the strategy: every version should move integration effort outward — from our code (ring 1) to shared protocols (ring 2) to other people's code (rings 3–4).

## v2 — three pinned integrations

v2 deliberately overrides the demand-driven principle once: the two dominant agent CLIs plus the dominant editor are table stakes, not a guess. From integration #4 onward, issue counts decide.

### 1. Claude Code — deepen from v1

v1 baseline: `Stop` hook → signal line with `transcript` path (L2 emitter).

v2 adds: `UserPromptSubmit` hook so the prompt is captured at turn *start* (better excerpts, and prompt survives even if the transcript is cleaned up); `agentrec import claude` backfill from `~/.claude/projects/*.jsonl` (if not already shipped in v1.x); project `.mcp.json` registration written by `agentrec init`; a documented CLAUDE.md recipe for the self-healing loop ("after breaking a test, call `agentrec_blame` before re-editing"); and the **Claude Code plugin** (IMPLEMENTATION.md D44) — plugins bundle hooks + MCP config in one marketplace install, collapsing the entire setup to a single action for the largest agent-user population. The plugin is packaging only: it generates configuration through the same code path as `init`, asserted byte-identical in CI, so there is never a second setup story to support. This distribution channel postdates the original draft of this document and is the cheapest reach-expansion in the whole plan.

Conformance: **L2**.

### 2. Codex CLI — the neutrality proof

Codex now ships a first-class lifecycle hook system — `Stop`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, and more, each a shell command receiving a JSON payload on stdin, loaded from `hooks.json` or `[hooks]` tables in `config.toml` — plus the older `notify` mechanism for `agent-turn-complete` events. Sessions are stored as complete JSONL rollout transcripts under `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.

The integration is therefore a structural mirror of Claude Code's:

- `agentrec init --codex` installs a `Stop` hook emitting the standard signal line with `tool: "codex"` and the rollout path as `transcript`.
- `agentrec import codex` backfills history from rollout files — same cold-start killer, second ecosystem.
- MCP registration for Codex's MCP client config, so Codex agents get `blame`/`log`/`diff` (and gated `undo`) too.

Conformance: **L2**. Strategic weight: one `signal.jsonl` containing both `claude-code` and `codex` lines is the screenshot that makes the "open standard" claim credible — nobody believes a standard with one emitter.

**Pinned minimum version (live-verified, 2026-08-05):** `codex-cli 0.146.0` — the hook payload
shapes, trust flow, and continuation semantics this integration is built against were captured
live against this exact pinned binary, not inferred from docs. Full spike writeup:
`docs/verify/codex-spike.md`; ledger row: `VERIFY-LEDGER.md` § "Phase A — Codex hook spike".

References: [Codex hooks](https://developers.openai.com/codex/hooks), [advanced config / notify](https://developers.openai.com/codex/config-advanced), [session storage](https://codex.danielvaughan.com/2026/06/02/codex-cli-session-archiving-lifecycle-management-v0136/).

### 3. VS Code — the first render surface

Read-only extension, zero capture logic: it reads `.agentrec/log.jsonl` and the object store directly and watches for appends. No daemon dependency for rendering; if the daemon isn't running, the extension still shows history.

Surfaces, in priority order: gutter decorations with turn blame per line (hover: turn id, tool, model, prompt excerpt, human-edited-since); per-turn diff as a virtual document (click a turn, see exactly what it changed); a turns panel (the `agentrec log` view, filterable by tool/session/file); a status-bar recording indicator; and command-palette actions for blame/diff, with `undo` shelling out to the CLI's interactive checklist so the human-confirmation semantics stay identical everywhere.

The precedent is GitLens: one of the most-installed extensions ever, built by visualizing data git already had. This visualizes data nobody has.

Conformance: consumer per PROTOCOL.md §7 — tolerates unknown fields, both grades, dangling `prompt_ref`.

## Sequencing inside v2

1. **Codex hook + import** first — days of work (it mirrors the Claude Code adapter), and it unlocks the neutrality screenshot that reframes everything that follows.
2. **MCP server** second — the meat of the release: read tools plus `mcp_destructive`-gated undo, and the self-healing demo gif.
3. **VS Code extension** last — it renders whatever the first two produce, so it ships against a stable, two-emitter log.

## Explicitly not in v2

JetBrains and Neovim (community, via the protocol — publish a "build a renderer in a weekend" guide instead); Cursor capture (quiet-window already covers it as L0; a rich adapter waits for Cursor to expose hooks); Aider (import-only path; its auto-commit trail converts to turns without any hook); the PR bot and git trailers as a team workflow (v3, per ROADMAP.md). Each of these is a pull-based decision: build when the issues ask for it.
