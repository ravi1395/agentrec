# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working in this repository.

## What this is

**agentrec** — a local-first, tool-agnostic flight recorder for coding agents. It segments agent activity into *turns*, snapshots every touched file into a content-addressed store, and answers "who broke my repo — me or the agent?" via `log` / `diff` / `blame` / `undo`. The turn engine is an extraction of the production-proven `turns.rs` from Sutra (`~/Projects/sutra/src-tauri/src/turns.rs`) — when in doubt about engine semantics, that file is the reference implementation.

## Status (update after every delivery round — house rule)

**Slimmed 2026-07-29 at the founder's direction:** delivered-round narratives removed; full
history lives in `git log -- CLAUDE.md` (last full version at `204ff04`). This section now
carries only current state, what's next, and standing debts.

### Current state

- **`main`** — carries the perf-evidence round: [PR #8](https://github.com/ravi1395/agentrec/pull/8)
  **squash-merged** to `main` as `4e04438` on 2026-07-30 (per-phase history survives only in the
  PR, not on `main`), then reconciled here with the local docs-only chain (`204ff04`..`2ade38a`)
  by merge. Test baseline **443 / 0 / 1**. Live daemon records this repo; store healthy
  post-purge.
- **Perf-evidence round (delivered, GATE PASS, now on `main`)** — 10k/50ms recall envelope
  closed (p99 11–23 ms, AC1.3 founder-attested), dedup counters, retention plan/execute split,
  eviction moved to a daemon tick, 3 real defects fixed (symlink dedup loss, `--stats` uninit,
  tick gap). **Linux CI leg now real:** run
  [30552318400](https://github.com/ravi1395/agentrec/actions/runs/30552318400) — all 5 jobs
  green (`ubuntu-22.04`, `ubuntu-24.04`, `macos-14`, lint, induced-low-watches). The macOS leg
  went red on its first attempt and green on rerun; cause recorded under residuals, not
  hand-waved.
- **Phase 2 spec hardened + gated (2026-07-28, 6 skeptic rounds, commits
  `bd0b679`/`bde8146`/`90202f6`/`204ff04`):** founder decisions 5–10 in
  `docs/superpowers/specs/2026-07-18-agentrec-phase-2-design.md` — (5) protocol freeze behind
  Codex 2.1; (6) MCP destructive gated on ≥20 human-confirmed `undo --confirm` (today 0) and
  `allow_modified` never honored in auto mode; (7) CLI demand probe defined
  (transcript-sweep instrument) then (10) **waived as a gate by founder override — MCP read
  2.2 is unconditional Phase 2 scope**, probe survives as post-ship evaluation; (8) import
  gate carries a fidelity report + ledger row, parse-only pass insufficient; (9) **Sutra
  parked** — Phase 2 re-centered on accessibility (import, trailers, `--json`, distribution,
  setup, Codex). Corpus decay measured: import is a **≤30-day rolling backfill**
  (`cleanupPeriodDays` default 30); "durable archive" claim embargoed until a re-import
  mechanism ships; 4-tier before-ladder (T1 42.5 / T1.5 25.3 / T2-cand 24.5 / T3 7.7 → 67.9%
  honest, 92.3% banned).
- **Phase 2.0 plan chunked:** `docs/superpowers/plans/tasks/P1..P5.md` (standalone,
  fresh-executor-ready); plan has a 9-checkbox "Final acceptance — plan exit" section.
  Execution lives on `feat/phase-2-0-substrate` (P1–P4 landed there; P4b planned, `d364ef6`) —
  that branch carries its own Status; this section tracks `main`.

### Now / next (in order)

1. **Execute P1** — `docs/superpowers/plans/tasks/P1.md`: `import claude --dry-run`
   classifier + 4-tier ladder + fidelity report over the real corpus; retires (or honestly
   fails) the Phase 2.0 hard gate. Branch `feat/phase-2-0-substrate` (create it), claimd
   declare-first, skeptic gate in isolated worktree. P3 (golden harness) may run in parallel.
2. P2 (persist + undo refusal) → P4 (`RepositoryView`) → P5 (`--json`), then the plan-exit
   checklist. Wave 2 (aider import, trailers + shim, npm/mise) after or parallel per open
   question 1.
3. Then: Codex 2.1 → Protocol 1.0 freeze → MCP read 2.2 (unconditional, thin adapter over
   P5's serializer). 2.3 stays evidence-gated.

### Founder-pending (agent cannot or may not do these)

- `rm` retained archives `.agentrec/objects.archived.1784328469` (2.6 GiB) + `.1784934498`
  (15 MiB) — reversible-until-deleted, disk-only.
- 13 stale local branches (git-guardrails hook blocks agent `branch -D`; command was handed
  over 2026-07-28).
- 4 claimd claims DECLARED awaiting manual attestation (never self-attested).
- Undecided claimd doc-scope rule: `PROTOCOL.md` is not lint-ignored — next normative-doc
  edit fires the Stop hook again.
- Demand/launch gate (Show HN etc.) never run — ROADMAP Phase 0's 30-day kill criterion has
  no data; 2.2's post-ship evaluation row needs a probe repo picked + `agentrec init` there.
- Plan open questions 2–3: memory-plan stale checkboxes; store-churn reclaim.

### Standing debts & residuals (recorded, not blocking)

- `log.jsonl` churn history (9602 `.remember` entries) still renders as churn blasts in
  `log`/`show`; not byte-reclaimable without a third sanctioned rewrite class — deliberately
  not built.
- claimd coverage debt rows: `cli/src/cmds.rs` (P3 residuals round) and `cli/src/purgecmd.rs`
  (honesty round) — touched-uncovered, retroactive declaration refused by design.
- P1 probe verdict bounded: fixture aging can't reproduce weeks-old FSEvents journal history;
  the dogfood daemon is the observatory.
- Perf/timing margins are macOS+one-Colima-VM evidence; population-level claims close only
  over CI history. PR #8 contributed the first two GitHub-runner Linux data points for this
  round; one PR is not a population.
- **`hook_recall_hard_wall_deadline` is runner-coupled** (first observed PR #8, macos-14 job
  `90903831567`): it timed **204.1 ms** against a `< 200 ms` assert and passed on rerun. The
  invariant held both times — a 600 ms blocked pin read was abandoned, not waited on — but the
  assert measures *whole-process* wall including fork+exec, leaving ~4 ms of margin on a shared
  runner. Bound not yet changed; see the next commit.
- Memory dogfood ladder effectively not started (store prepped 2026-07-17; clock never ran
  clean).

## Working method

Work runs as the `/ratchet` loop (`.claude/commands/ratchet.md`; staged in `claude-setup/` until installed — a ratchet only tightens, ACs are never loosened to pass): Opus orchestrates, Sonnet `implementer` agents build against named AC ids, an Opus `skeptic` agent with fresh context reviews per-AC (PASS/FAIL/UNTESTED — no PASS without a test), failures loop back, and every round ends by updating the Status section above. Never weaken an AC to pass; escalate ambiguity to the founder.

Read before building; do not invent semantics that contradict these docs:

| Doc | Contents |
|---|---|
| PROBLEM.md | Why this exists; who has the pain; why tool-neutral |
| SPEC.md | v1 reference implementation: daemon, 5 CLI verbs, prompt posture |
| PROTOCOL.md | **Normative.** Signal + turn-record schemas, conformance levels L0–L3, MCP surface, versioning rules |
| ROADMAP.md | v1 → v4+ phases, measurable gates, kill criteria |
| INTEGRATIONS.md | Four-ring integration thesis; v2 designs (Claude Code, Codex, VS Code) |
| IMPLEMENTATION.md | Decision register D1–D44 + exhaustive acceptance criteria per release (incl. v0.3 durability + v0.4 accessibility amendments) |
| REVIEW.md | Hostile review that forced the v0.2 capture redesign — read before touching capture semantics |

Conflict resolution order: PROTOCOL.md > IMPLEMENTATION.md decision register > SPEC.md > everything else.

## Architecture

```
EMITTERS                      RECORDER                      CONSUMERS
Claude Code Stop hook ─┐
Codex Stop hook (v2) ──┼─→ .agentrec/signal.jsonl ─┐
(any L1+ tool) ────────┘      (append-only inbox)  │
                                                   ▼
                              agentrec daemon ("record")
fs mutations ─→ watcher ─→    TurnEngine: signal boundary,
(notify, 1.5s debounce,       else 10s quiet window;        ─→ .agentrec/log.jsonl   ─→ CLI (log/diff/blame/undo)
 denylist filter)             one open turn per root            (canonical, JSONL)    ─→ MCP server (v2)
                                   │                        ─→ .agentrec/objects/     ─→ VS Code ext (v2, read-only)
transcripts (prompt) ─→ scrub ─────┘                            (sha256 CAS, 10MiB cap)─→ Sutra GUI (v3)
```

Two crates in one cargo workspace: `agentrec-core` (lib: TurnEngine, BlobStore, formats, scrub) and `agentrec` (bin: daemon + clap CLI). Threads + `notify`, no async runtime. Consumers never need the daemon running — all reads are file-based.

### Key semantics (do not violate)

- **Turn grades:** `rich` (hook signal → tool/model/prompt attached) vs `bare` (quiet-window inferred). A bare turn is an *unattributed activity window* — a human vim save produces the same fs signature — so bare turns are never rendered as agent activity and never fabricate attribution.
- **Bracketing (v0.2, load-bearing):** Claude Code integration installs `UserPromptSubmit` (start) + `Stop` (stop). Open bracket suppresses quiet-window closure; on stop, interim bare turns are retroactively merged into the rich turn. Start-without-stop = close at last mutation, `truncated: true`.
- **Git turns:** mutation bursts coinciding with `.git/HEAD`/index/ref transitions are rich turns with `tool: "git"`; hidden from `log` by default. Never let a `git checkout` become a 400-file bare turn.
- **Epochs & gaps:** daemon start/stop append `type:"epoch"` records; blame across an uncovered interval must say "attribution stale — recording gap", never guess.
- **Append-only everything:** `log.jsonl` and `signal.jsonl` are never rewritten. History is corrected by appending. Signal consumption tracked by byte offset in `state.json`. Turn ids are machine-scoped ULIDs.
- **Two predicates, never conflated:** `modified-since` (hash ≠ turn's after) gates every destructive op; `human-edited-since` (modified AND not covered by any *rich* turn) is blame display only. Bare turns never count as coverage.
- **Undo is a turn:** every revert (CLI or MCP) snapshots current state first and appends a new turn with `tool: "agentrec"`. Reverts are blame-able and re-revertible. `skipped` (over-cap) and `withheld` (secret-pattern) files are never revertible.
- **Prompts and snapshots both scrubbed:** prompt scrub (secret regexes + entropy) runs *inside* the persistence function; secret-file patterns (`.env*`, `*.pem`, credentials) are never snapshotted (`withheld: true`). Local-only; no network code exists in v1–v2; never claim "provably" safe.
- **Agent-driven undo** is gated by `config.toml: mcp_destructive = off|confirm|auto` (default off); `auto` requires the two-phase confirm-token flow; safety keys on `allow_modified` (PROTOCOL.md §8).
- **Watch filtering:** gitignore-derived by default, plus denylist `.git` contents (except HEAD/index/refs — needed for git-turn classification), `.agentrec` (self-write suppression — no feedback loops), `node_modules`, `target`, `dist`, and user globs. Linux inotify limit exhaustion = loud startup error.
- **Clocks:** UTC wall clock in records; monotonic clock for quiet-window measurement.

## Commands (once code lands)

```bash
cargo test                    # unit + engine table tests (agentrec-core)
cargo test --test integration # drives the real binary against tempdir fixtures
cargo clippy && cargo fmt     # CI-enforced
agentrec init && agentrec record   # dogfood in this repo itself
```

## Conventions

- Every acceptance criterion in IMPLEMENTATION.md §3–§7 maps to at least one automated test; new features add their AC there first.
- Protocol changes: additive-only within a major `v`; update PROTOCOL.md + conformance fixtures in the same commit; consumers must tolerate unknown fields.
- Platforms: macOS + Linux. No Windows-specific code, but no hardcoded path separators either (D19).
- License: Apache-2.0 (open-core; all v1–v3 code). Hosted/team features v4+ commercial.
- Never delete user data: `purge` is the only deletion path and only for objects past TTL / by explicit flag; archives, never silent removal.
