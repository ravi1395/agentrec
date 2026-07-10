# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working in this repository.

## What this is

**agentrec** — a local-first, tool-agnostic flight recorder for coding agents. It segments agent activity into *turns*, snapshots every touched file into a content-addressed store, and answers "who broke my repo — me or the agent?" via `log` / `diff` / `blame` / `undo`. The turn engine is an extraction of the production-proven `turns.rs` from Sutra (`~/Projects/sutra/src-tauri/src/turns.rs`) — when in doubt about engine semantics, that file is the reference implementation.

## Status (update after every delivery round — house rule)

**J3 CI + publish round (2026-07-10):** Wired GitHub Actions and open-sourced the repo. **`.github/workflows/ci.yml`** — per-push/PR matrix: `fmt + clippy` (ubuntu-24.04) + `cargo test --workspace` on **macOS-14 + Ubuntu-22.04 + Ubuntu-24.04**; `#[ignore]d` torture heavy-run excluded (torture_smoke still runs). **`nightly.yml`** — cron (07:17 UTC) + `workflow_dispatch`, runs the torture harness `--ignored` on macOS-14 + ubuntu-24.04 with `AGENTREC_TORTURE_SEED` **unset → wall-clock-derived per night** (the D36 7-green-nights vehicle), uploads the log (printed seed → reproducible). First `cargo fmt --check` enforcement caught 2 unformatted asserts in `integration.rs` (auto-fixed). **Relicensed dual MIT-OR-Apache → Apache-2.0** (open-core, founder decision, supersedes D17) — canonical `LICENSE` (sha256 `a60eea81…f759f2`) + `NOTICE`, all doc refs collapsed. Pre-publish safety: untracked machine-local `.claude/`/`.codex/`/`.sutra/`/`.mcp.json` (held a localhost Sutra MCP token + home paths) and **squashed history to one clean initial commit** (token in zero public commits; old history kept locally as `old-history-local`). Published **https://github.com/ravi1395/agentrec** (public). **CI matrix GREEN** — run [29098847491](https://github.com/ravi1395/agentrec/actions/runs/29098847491), all 4 jobs success → **J3 CLOSED**; the Ubuntu legs ran `lock_file_sets_0600`/`lock_dir_sets_0700`/`d37_daemon_writes_land_at_locked_permissions` green on real Linux → **I++1 (Linux perms) CLOSED** too. 157 tests unchanged; clippy clean. **Still open (laddered):** D36 7-night streak (workflow live, needs the clock), Y++2 inotify induced-low-watches (normal path green on Linux CI; no step lowers `fs.inotify.max_user_watches` yet), Ubuntu-container/clean-macOS-VM/brew installer legs, README blame GIF. Minor CI follow-up: `actions/checkout@v4` emits a Node-20-deprecation annotation (non-blocking; bump to `@v5` later).

**PD-review fixes round (2026-07-10):** 6 senior-product-designer findings against the real binary in a fresh repo (2 launch-blocking, 4 polish) fixed under `/ratchet` — Opus orchestrator, 4 Sonnet implementers, Opus skeptic gate. Final verdict **GATE PASS**: all 8 AC ids (PD1–PD5) PASS with a named non-weakened test + pasted real-binary repro (stream + exit code verified). Then, on founder go-ahead, 2 further honest-posture edges the skeptic surfaced were fixed in the same round (commit `0b208a4`): `show --prompt` on a corrupt (hash-mismatch) blob now says `prompt blob corrupt (hash mismatch)` (distinct from `Missing`→`prompt purged — TTL expired`); idempotent `init` that recreates a separately-deleted `.agentrec/objects/` no longer claims `nothing changed` (gate now reflects actual disk mutation, genuine no-op still says `already initialized — nothing changed`). **157 tests, 0 failed** (49 cli-unit + 59 integration + 1 torture-smoke [+1 ignored] + 48 core), clippy clean; baseline was 142 → +15. Commits `9eb5e3d`/`bdb3cb6`/`a769ad6`/`759a779`/`0b208a4` on baseline `e9551e8` (git repo initialized for this round). Delivered: (PD1) `doctor` init-gate — first check `initialized`, absent `.agentrec/` → fail + remedy `not initialized — run \`agentrec init\`` and every other check `n/a`, exit 1, no fabricated `permissions too open`/`agent active but no signals` (`doctorcmd.rs`, additive `--json`); (PD2) `agentrec show <turn> [--prompt]` — bare = header via `readcmds::render_turn`, `--prompt` = full post-scrub blob to stdout, dangling/bare → honest stderr + exit 1, resolver shared with `diff` (`readcmds.rs`, `main.rs`, README row) — closes SPEC §Prompt-posture item 4 excerpt-discipline drift; (PD3) zero-turn `status` → `rich-rate:  n/a (no agent turns yet)`, never vacuous 100%; (PD4) zero-turn `log --json` → `[]` exit 0 (`cmds.rs`); (PD5) doctor check `store degraded`→`store health`, status jargon → `(agent turns; git activity hidden)`, idempotent `init` re-run leads `already initialized — nothing changed`, low-rich-rate remedy → `agentrec doctor` (`doctorcmd.rs`/`cmds.rs`/`initcmd.rs`).
**Tracked debt (this round):** _D-PD6 (deferred, founder-confirmed) — the only open item:_ two parallel turn renderers (`cmds::format_turn` vs `readcmds::render_turn`) + separator drift (`—`/`·`/`->`/`;`) — unify in a later round, NOT bundled here. (The two skeptic-surfaced honest-posture edges — corrupt-blob→false-TTL and init→false-"nothing changed" — were FIXED this round in `0b208a4`, not deferred.)

**Done (as of 2026-07-10):** M1 "Record" + M2 "Answer" + **M3 "Ship" code-complete** — under `/sdd`, opus-4.8-high final gate verdict **SHIP** (every AC verifiable on macOS PASS with pasted command output; environment-gated ACs laddered in `VERIFY-LEDGER.md`, never marked PASS). **142 tests, 0 failed** (46 cli-unit + 47 integration + 1 torture-smoke [+1 ignored gated run] + 48 core), clippy clean, zero orphan daemons. Delivered: (I5/I6/I+) `purge` [expired-prompt default, `--all-prompts`, `--snapshots-before`] with keep-set dedup safety + `MAX_STORE_BYTES` 2 GiB oldest-snapshot eviction (`agentrec-core/src/retention.rs`) + `status` over-budget notice; (I++/D37/D38) umask-independent 0700 dir / 0600 files at EVERY write site via `agentrec-core/src/perms.rs` — **caught+fixed blocker: daemon-created files were 0644, only init-time config was locked** — plus 3-location planted-secret grep test; (Y+3/4/5/D39-40) one-command `init` writes+loads a per-repo launchd/systemd unit (`cli/src/service.rs`, canonicalized root), lists actions + reverse command, byte-for-byte no-op re-run, `--dry-run` (touches nothing), `--no-service`; `uninstall` (`cli/src/uninstallcmd.rs`) command-granular hook removal + archive-only `.agentrec.archived.<ts>` (never deletes); (Y++/D41) `doctor` (`cli/src/doctorcmd.rs`) — daemon-liveness/hook/signal-freshness/DEGRADED/perms/inotify checks, `--json`, exit 0/1, 8 failure-mode tests; (Z+2/3/4/D43) relative times default + `--utc`, `NO_COLOR`/TTY color, `--explain` glossary-for-present-terms (`cli/src/fmt.rs`); (H++/D36) undo torture harness (`cli/tests/torture.rs`) — 1200 randomized ops (bursts/human/git-checkout/kill-9/undo), INV1 modified-since-refusal + INV2 undo-of-undo-byte-exact asserted after every undo, 0 violations, Drop-guard leaves zero orphan daemons, nightly seed now wall-clock-varying; `install.sh` (checksum-verified `~/.local/bin`, no sudo, local-binary leg proven), PROTOCOL.md v0.2 (additive `merges`/`baseline_unknown`), README (line-level-blame + "how we try to break it"). New modules: `agentrec-core/src/{retention.rs,perms.rs}`, `cli/src/{purgecmd.rs,service.rs,uninstallcmd.rs,doctorcmd.rs,fmt.rs}`, `cli/tests/torture.rs`. Launch-gated (laddered, NOT done): D36 7-consecutive-green-nights streak, Ubuntu/clean-VM/brew installer legs, Linux inotify+perms CI, README blame GIF, CI matrix. No git repo → no branch/PR flow run; dogfood week still running.

**Prior (as of 2026-07-10):** M1 "Record" (see below) **+ M2 "Answer" complete** — all M2 ACs PASS under adversarial opus final-gate review (verdict: SHIP). Delivered: (L+/D34) fsynced ledger closes (`append_log` sync_all; signal inbox exempt), atomic race-free blobs (typed `PutResult`, unique `.tmp.<pid>.<seq>`, fsync-before-rename + dir fsync), kill-9-around-close harness + concurrent-identical-blob test; (M+/D35) snapshot-failure taxonomy — persisted `snapshot_failures` counter + `io_failed` list in `state.json`, `status` DEGRADED banner, `status --ack-degraded`, undo says "write failed at record time" vs "over size cap" (wire format unchanged), proven end-to-end via real daemon fault injection; (F1–4) `diff` (unified/binary/skipped/unknown-id); (G1–6) `blame` incl. line-level (before/after snapshot diff) + epoch-gap staleness; (H1–7) `undo` — modified-since (D30) rail, `--allow-modified`, per-file `--files` subset, skipped/withheld refusal, create/delete inverse, undo-is-a-turn (`tool:agentrec`, re-revertible), H7 concurrent via `.agentrec/undo-guard.json`; (Z+1/D42) preview-first panic `undo` targeting last non-git rich turn, refuses on trailing bare/empty. New modules: `agentrec-core/src/diff.rs`, `cli/src/{readcmds.rs,state.rs}`; added `similar` dep. **84 tests, 0 failed** (41 core + 27 integration + 14 cli-unit + 2 added), clippy clean. Known v1 limits (non-blocking): H7 guard drops genuine concurrent edits to guarded paths during the ~3s linger (by-design, bounded); G5 duplicate-identical-line heuristic. No git repo → no branch/PR flow run.

_M1 "Record":_ `agentrec-core` engine (bracketing + retroactive merge, git-turn classification, quiet fallback, crash journal), blob store with integrity-check-on-read, scrub + secret-path withholding, daemon (`record`) with watcher/signal-tailer/kill-9 recovery, `init` (idempotent hook merge), `log`, `status`, `hook`. Dogfood week started 2026-07-06 (M1-exit tripwire closes ~07-13). Design suite amended through v0.4 (durability D34–D38, accessibility D39–D44).

**Next:** M3 launch gates (all in `VERIFY-LEDGER.md`, none code-blocked) — wire CI (`cargo test` matrix macOS-14 + Ubuntu-22.04/24.04, J3) with the torture `--ignored` run nightly using a per-night `AGENTREC_TORTURE_SEED`, then the **7 consecutive green nights** gate (D36); prove the installer's network/Ubuntu-container/clean-macOS-VM legs + author+publish the brew tap (Y+1/Y+2); record the line-level-blame README GIF; Linux-only CI assertions (Y++2 inotify headroom, I++1 perms). Then the launch checklist: one full dogfood week with the DEGRADED banner never falsely firing. Nice-to-haves surfaced at the final gate: broaden torture INV2 coverage (only 2/21 checkpoints executed undo-of-undo per run — the varying nightly seed accumulates it); cosmetic idempotent-`init` re-run messaging.

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
