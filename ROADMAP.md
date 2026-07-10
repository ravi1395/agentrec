# agentrec — Roadmap: v1 to end state

*Companions: PROBLEM.md (why), SPEC.md (v1 what), PROTOCOL.md (the format everything rides on).*

## Operating principles

These govern every phase and every cut decision:

1. **Single-player value first.** Every phase must deliver value to one developer with no team, no server, no account. Team value stacks on top; it never replaces this.
2. **One flagship integration per port.** Each category (capture, git, MCP, editor, CI) gets exactly one first-party integration. The protocol — not the roadmap — is how the rest get built, by others.
3. **Demand-driven expansion.** Integrations are chosen by counting "does it support X?" issues, never by guessing — with one founder-judgment exception: v2 pins Claude Code, Codex, and VS Code (INTEGRATIONS.md), because the two dominant agent CLIs plus the dominant editor are table stakes, not a guess. From integration #4 onward, demand decides.
4. **PLG only.** Nothing on this roadmap requires a sales conversation. The compliance layer is built for inbound pull, not outbound push.
5. **Gates, not dates.** Each phase has a measurable gate. Failing a gate means stop and rethink, not push harder.

## Phase 0 (v1) — Extraction (~8 weekend-equivalents, milestones M1–M3 in SPEC.md)

**Goal:** the core exists outside Sutra, produces a *readable, correctly attributed* log on real repos, and one developer (you) uses it daily.

**Ships:** `agentrec-core` crate extracted from `turns.rs` plus the compensation layer the hostile review demanded (REVIEW.md F1/F2/F4/S2): Claude Code start+stop hook **bracketing** with retroactive bare-turn merge; git-turn classification (ref-change boundaries, hidden from `log` by default); epoch records with stale-blame warnings; gitignore-derived watch filtering; service install (launchd/systemd). Six CLI verbs (`record`, `log`, `diff`, line-level `blame`, `undo` on the modified-since rail, `status` with rich-rate health); scrub + snapshot withholding + default TTL/eviction; the **durability & trust hardening layer** (2026-07 code review, IMPLEMENTATION.md D34–D38): fsynced ledger appends, atomic race-free blob writes, snapshot-failure taxonomy with a DEGRADED `status` banner, 0700/0600 store permissions, inbox-inclusive planted-secret scan, and the nightly public **undo torture harness** (randomized agent/human/git/kill-9/undo interleavings asserting the two undo invariants); PROTOCOL.md v0.2 published in-repo; README led by the line-level blame gif and the "how we try to break it" harness section; and the **accessibility layer** (IMPLEMENTATION.md D39–D43): prebuilt-binary installer matrix (`curl | sh`, brew — cargo is never the headline path), one-command `init` with agent-tool auto-detection and symmetric archive-only `uninstall`, `agentrec doctor` full-chain diagnosis, panic `undo` (bare invocation targets the last agent turn, preview-first), and comprehension defaults (relative times, TTY color, `--explain`).

**Launch:** Show HN + r/LocalLLaMA — *"Who broke my repo — me or the agent?"*

**Pre-launch tripwire (end of M1):** one week of dogfood on Sutra + one JS/TS repo must yield a readable log — git noise hidden, rich-rate ≥ 90 % of agent turns, no misattributed human bursts in spot checks. Fail with all fixes in → the extraction thesis is wrong; fold back into Sutra (REVIEW.md steelman) having spent three weekends, not a year.

**Pre-launch gate (end of M3):** the undo torture harness (D36) green for 7 consecutive nights, and one full week of daily-driver dogfood with the DEGRADED banner never falsely firing. A safety net launches only after it has survived a sustained attempt to break it.

**Gate (30 days post-launch):** meaningful stars; ≥5 "does it support <tool>?" issues (demand signal for the next capture integration); ≥1 external person running it on a repo that isn't yours. Miss all three → the problem framing is wrong; go back to discovery before writing more code.

## Phase 1 (v1.x) — Cold start and git (weeks 3–8)

**Goal:** value on the first minute of first run, and provenance that travels with the repo.

**Ships:**
- `agentrec import` — backfill turns from Claude Code's local session transcripts (`~/.claude/projects/*.jsonl`) and from Aider's auto-commit trail. A recorder is empty on day one; import makes `blame` work on last month's changes at install time. This is the cold-start killer and the second README gif.
- Git trailers — post-commit hook appends `Agent-Turn: t_NNNN` to commit messages; provenance survives push and renders in every git UI ever built, for free.
- `git-agentrec` on PATH — `git agentrec blame` works with zero new muscle memory.
- Distribution wrappers (D39): npm (`npx agentrec`) and mise/asdf plugins fetching the release binary — meets JS and polyglot developers inside package managers they already trust; packaging only, one binary, one behavior.
- Protocol v1.0 freeze, incorporating early-adopter feedback. After freeze: additive changes only.

**Gate:** import works on ≥90% of real-world Claude Code transcript files thrown at it (issues will tell you); first external PROTOCOL.md conformance question or third-party emitter interest appears.

## Phase 2 (v2) — The integration release (months 2–4)

**Goal:** flip the product from "humans watching agents" to "agents depending on the record," and prove tool-neutrality with a second emitter and a first editor surface. Full designs: INTEGRATIONS.md.

**Ships:**
- `agentrec mcp` — MCP server exposing `agentrec_log`, `agentrec_diff`, `agentrec_blame`, and `agentrec_undo` gated by the user's `mcp_destructive` mode (PROTOCOL.md §8: off / confirm / auto, two-phase confirm token, every undo recorded as a turn). Read tools ship enabled; agent-driven reversion ships as an explicit per-repo user opt-in — the self-healing loop is the product, the consent switch is the guardrail. One protocol, and agentrec is inside every MCP-capable host — Claude Code, Cursor, Windsurf, and next month's tool — with one config entry.
- The self-healing demo: agent breaks a test → calls `agentrec_blame` → identifies its own bad turn → previews and reverts it with a confirm token → retries differently. Captured as the third gif; this is the strongest demo the product will ever have.
- Codex CLI capture — Stop hook via Codex's lifecycle-hook system emitting the standard signal schema (`tool: "codex"`), plus `agentrec import codex` from rollout transcripts. One `signal.jsonl` with both `claude-code` and `codex` lines is the neutrality proof — the screenshot that makes the "open standard" claim credible.
- Claude Code hardening — `.mcp.json` registration via `init`, documented CLAUDE.md self-healing recipe, transcript-format canary test in CI, and rich-rate alerting (one vendor update silently downgrading every turn to bare must be caught by `status`, not by a user's empty blame — REVIEW.md S6). (Start-signal bracketing moved into v1.)
- Claude Code plugin (D44) — marketplace packaging bundling hook config + MCP registration into a one-step install for the largest agent-user population; strictly packaging over the same code path as `init` (byte-identical config, single source of truth), so distribution widens without forking setup logic.
- VS Code extension (pulled forward from Phase 3), read-only: turn blame in the gutter, per-turn diffs, turns panel. The GitLens play — renders the open format, zero capture logic; `undo` shells out to the CLI so confirmation semantics stay identical everywhere.
- Parallel-agent dogfood: two worktree agents coordinating via `agentrec_log` before touching shared files.

**Gate:** observed real sessions (not demos) where an agent's tool call to agentrec changed its behavior for the better; MCP config appears in strangers' dotfiles/CLAUDE.md files on GitHub search; extension installs growing without paid promotion; repos in the wild with both `claude-code` and `codex` turns in one log.

## Phase 3 (v3) — Where code gets reviewed (months 4–8)

**Goal:** show up where teams read code — the pull request — and lay the attestation groundwork.

**Ships:**
- PR bot / GitHub Action: "this PR = 14 turns · 3 tools · 2 files human-edited after agent changes," with turn-level diffs linked. Requires opt-in committed or artifact-uploaded `log.jsonl`.
- Signed log entries (experimental, behind a flag) — attestation groundwork; cheap to design in now, per SPEC.md open questions.
- Sutra rebased onto `agentrec-core` as the flagship GUI client.

**Gate:** ≥3 teams (not individuals) using the PR bot on real repositories. Teams appearing organically is the signal that Phase 4 has a customer.

## Phase 4 (v4+) — The business (months 8–18)

**Goal:** monetize the audience that arrived on its own. Open core: the CLI, daemon, protocol, and every single-player feature are free forever; money comes from what organizations need.

**Ships:**
- Team visibility and reporting: how AI changes flow through a codebase — per repo, per tool, per review status — as signed, deterministic reports. Sold as what it is: reporting on the recorded history, not forensic proof of authorship. Audit-grade **attestation** is explicitly gated behind L3 vendor-native signed emission (REVIEW.md F3: a self-held key over heuristic attribution is a diary, not evidence) and ships only if/when vendors emit.
- Org retention and policy: centrally configured TTL, scrub rules, and required-review policies enforced via the PR bot.
- Team dashboard (first and only hosted component; self-host option preserved for the regulated buyers who will demand it).

**Distribution stays inbound:** compliance buyers arrive because their engineers already run agentrec and their auditors started asking questions. Pricing published on the website; buy with a credit card. No sales team — by constraint and by design.

**Gate:** first paying team without a single sales call. That event is also the go-full-time decision point.

## End state

The turn-log protocol is the standard agent tools emit natively — the recorder daemon becomes optional because emitters write Level-3 records themselves (PROTOCOL.md §7), and agentrec's surviving role is the neutral read/verify/attest layer: the blame engine, the undo safety model, the signature verification, the compliance exports. Sutra is the reference client. The company is small, profitable, and owns the boring, load-bearing substrate everyone else builds on — the git of agent provenance, not the GitHub. (GitHub can come later.)

## Standing kill criteria

Honesty checks at every phase: if a major editor ships cross-tool recording *and* an open format (not just their own walled version), the neutral-substrate thesis is dead — fold into contributing to their format and keep the compliance layer. If import + hooks can't reach reliable attribution outside Sutra (the Phase 0/1 riskiest assumption), the product contracts to a Sutra feature. If nobody files "does it support X?" issues, the pain isn't real; stop.
