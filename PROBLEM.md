# agentrec — Problem statement

*Working title: `agentrec`. The flight recorder for coding agents.*

## The problem, in one line

**"Who broke my repo — me or the agent?"** Nobody running coding agents today can answer that question quickly, safely, or across tools.

## The problem, in full

In repos where developers work alongside coding agents, a single session can involve dozens of agent turns, several parallel agents in separate worktrees, and a human editing in between. When something breaks — a test regresses, a subtle behavior changes, a file looks wrong — the developer has no record of *which turn* made the change, *what prompt* caused it, *which tool and model* executed it, or *whether a human touched the file since*. Git can't answer this: agent work happens on dirty trees between commits, and a commit squashes many turns into one anonymous blob.

The result is a trust ceiling. Developers throttle their own agent usage — smaller tasks, more babysitting, one agent at a time — not because agents can't do more, but because the blast radius of an unattributable mistake is too expensive to debug. Our thesis: for a growing class of users, the binding constraint on agent adoption is shifting from model capability to **accountability infrastructure**. The Phase 0 launch is the test of that thesis, not its proof.

## Who has this pain

Anyone running coding agents seriously: individual developers using Claude Code, Cursor, Codex, or Aider on real codebases; teams experimenting with parallel agents in worktrees; and, on the horizon, engineering orgs that want visibility into how AI changes flow through their codebases — with true audit-grade provenance arriving only when agent vendors emit signed records natively (the L3 path in PROTOCOL.md; see REVIEW.md F3 for why a filesystem recorder alone cannot provide it).

## What people do today, and why it fails

**Git-commit-per-turn hacks.** Some tools auto-commit after each agent action, and within one branch this works better than critics of git admit — aider's commit trail even preserves enough metadata that agentrec can import it as attributed history. What it can't do: cover tools that don't auto-commit, record the dirty-tree work between commits, capture human edits interleaved with agent work, or keep `git log` readable — provenance and version history are different jobs, and jamming one into the other degrades both.

**Editor-native tracking.** Some editors show "AI wrote this" highlights. This is locked to one tool: the moment you run Claude Code in a terminal next to Cursor, the record fragments. No editor will ever record its competitors' turns.

**Session checkpoints (`/rewind` and kin).** Claude Code's checkpointing covers the acute "undo that last thing" moment — but only for its own file-tool edits, only within the session. It does not capture changes made by bash commands (`rm`, `mv`, build scripts), your manual edits, or other concurrent sessions and tools, and it is documented as session-level recovery, not permanent history. It has no answer to "which turn, which tool, which prompt introduced this line, and has a human touched it since?" — no blame, no cross-session query, no provenance an auditor could be shown. A checkpoint is an eraser; the problem needs a ledger. Its existence is validation, not competition: vendors agree turn-level reversibility matters, and each will build the single-tool, closed, session-scoped version — which is exactly why the neutral, permanent, cross-tool record has to come from outside the editors.

**Scrollback archaeology.** Reading the agent's chat transcript and mentally mapping it to file changes. Doesn't scale past one session, and evaporates when the terminal closes.

**Nothing.** The most common answer. `git diff`, squint, hope.

## Why now

Three curves are crossing. First, parallel agents went mainstream — worktree-based multi-agent workflows are now built into the major tools, multiplying the attribution problem. Second, model churn is constant — every model update silently changes agent behavior on your repo, and without a turn-level record there is no way to even notice, let alone diagnose. Third, the visibility wave is coming — teams and orgs increasingly want to know how AI changes flow through their code, and you can only report on the past if you were recording at the time (which is also why the recorder installs as an always-on service, not a terminal tab: a ledger with gaps must know its gaps).

## Why this benefits *any* model and *any* agent tool

This is the key design position: **agentrec sits below every agent, not inside any of them.** It is model-agnostic and tool-agnostic by construction, and every model benefits equally:

**Attribution without allegiance.** A neutral recorder is the only party that can attribute turns across Claude Code, Cursor, Codex, Aider, and whatever ships next month. Vendors can't do this for each other; a substrate can.

**Trust converts to throughput.** The reason to record isn't paranoia — it's leverage. When every turn is snapshotted and reversible, a developer can safely delegate bigger tasks to *any* model, run more agents in parallel, and review after the fact instead of babysitting in real time. Better recording makes every model more useful.

**Safe rollback, per turn.** Undo *turn 47* — not "reset to the last commit and lose the three good turns after it." Rollback refuses, by default, to touch any file whose content changed since the turn — by a human, another agent, or a git operation — so recovery never silently destroys later work.

**Free evaluation signal.** A corpus of recorded turns — prompt, model, diff, outcome — is exactly the dataset needed to compare models on *your* codebase and detect regressions when a model updates. Recording is the prerequisite for measurement.

**An open format is upside, not the moat.** The turn-log format is an open spec any agent can emit. We hold no illusions about standards adoption — protocols get adopted when a dominant player forces them, and vendors have lock-in incentives pointing the other way (REVIEW.md S8). The tool must be worth running alone, on day one, for one developer; if the format later becomes shared infrastructure, that's compounding upside, not the plan of record.

## Why us

The hard parts already exist, battle-tested in daily personal use inside Sutra: a turn-boundary engine (hook signals with a quiet-window fallback), a content-addressed snapshot store, per-turn diffs, change tracking, and safe per-file rollback. The honest caveat: inside an editor, the engine enjoys ground-truth signals a standalone daemon loses, which is why the extraction adds bracketing, git-turn classification, and gap records rather than shipping the naked state machine (SPEC.md capture design; REVIEW.md F1/F2/F4). This project is an extraction plus a compensation layer, not an invention. Sutra becomes the reference GUI client; `agentrec` becomes the substrate.

## What success looks like

v1: a developer installs one CLI, runs their existing agent unchanged, and can answer "who broke my repo?" in one command — `agentrec blame src/auth.ts:42` → `turn 47 · claude-code · "add rate limiting to login" · human-edited since`. Traction test: launch the open-source CLI, lead with that gif, and measure whether the question resonates (stars, issues, "does it support X?" demand within a month). Long game: the paid layer is team **visibility and reporting** — how AI changes flow through a codebase, per repo, per tool, per review status — sold to inbound buyers, never through a sales team. Audit-grade attestation is explicitly gated behind vendor-native signed emission (L3): we sell reporting on what the record shows, and we do not claim the record is forensic evidence until the parties at the point of ground truth sign it.
