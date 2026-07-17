# agentrec

[![CI](https://github.com/ravi1395/agentrec/actions/workflows/ci.yml/badge.svg)](https://github.com/ravi1395/agentrec/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

Agentrec is the first local, tool-agnostic flight recorder for coding agents: it captures each agent turn, snapshots every touched file, and lets developers answer "what changed, who changed it, and can I safely undo it?" across tools such as Codex and Claude Code. Unlike vendor-specific transcripts or Git—which records only committed outcomes—agentrec preserves the otherwise-missing layer between an agent's intent and the repository's final state, addressing weak attribution, unsafe rollback, silent failures, and lost context. In the broader agent ecosystem, it is the independent observability and recovery layer: agents act, Git versions, and agentrec makes their work inspectable, attributable, and reversible.

It segments agent activity into *turns*, snapshots every touched file into a content-addressed store, and answers the question that matters after a bad session:

**Who broke my repo — me or the agent?**

No cloud, no telemetry, no vendor lock — works with any tool that can fire a lifecycle hook (Claude Code today; Codex, and anything else, additively).

## Line-level blame

The headline feature: point at a file and a line, get the turn that wrote it, the tool and prompt behind it, and whether it's been touched since.

![line-level blame demo](docs/blame-demo.gif)
*(Recorded from a real daemon session — regenerate with `./docs/generate-blame-fixture.sh && vhs docs/blame-demo.tape`.)*

```
$ agentrec blame src/auth.ts:42
src/auth.ts:42: t_01J8…MB · claude-code · "add rate limiting to login" · 14:04
```

If the answer would require guessing across a recording gap (daemon wasn't running), agentrec says so instead of fabricating attribution:

```
$ agentrec blame src/auth.ts:42
attribution stale — recording gap
```

Whole-file blame drops the `:line` suffix and adds a `human-edited since` marker if the file changed after the turn shown:

```
$ agentrec blame src/auth.ts
t_01J8…MB · claude-code · "add rate limiting to login" · 14:04 · human-edited since
```

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/ravi1395/agentrec/main/install.sh | sh
```

This downloads a prebuilt binary for your OS/arch, verifies its sha256 checksum, and installs it to `~/.local/bin/agentrec` — no sudo, ever. If `~/.local/bin` isn't on your `PATH`, the installer tells you exactly what to add.

Or via Homebrew:

```sh
brew install ravi1395/agentrec/agentrec
```

Building an already-checked-out binary locally instead of downloading a release:

```sh
AGENTREC_LOCAL_BINARY=./target/release/agentrec sh install.sh --sha256 "$(shasum -a 256 ./target/release/agentrec | cut -d' ' -f1)"
```

Uninstall the binary: `rm ~/.local/bin/agentrec`. Uninstall everything agentrec added to a repo (hooks, service unit, archive `.agentrec/`): `agentrec uninstall`.

Then, in any repo:

```sh
agentrec init      # scaffolds .agentrec/, installs Claude Code hooks + a per-repo recorder service
agentrec record    # or let the service unit run it for you
```

## How we try to break it

Trust in a flight recorder comes from adversarial testing, not from reading the source. The riskiest surface here is `undo`: it mutates your working tree, and getting it wrong (reverting the wrong file, or destroying an edit it shouldn't have touched) is worse than not having the feature at all.

So there's a torture harness: randomized interleavings of agent turns, human edits, `git checkout`/branch switches, kill-9s mid-write, and undos, all firing concurrently against the same repo. After every undo it re-asserts both invariants from the protocol:

- **modified-since** never gates on stale information — an undo never reverts a file that changed after the turn it's undoing, unless explicitly told to.
- **undo is itself a turn** — every revert is snapshotted and appended to the log, so it's blame-able and re-revertible, and history is never destroyed.

This harness is a **launch gate**, not a nice-to-have: it runs nightly, and the release doesn't ship until it's posted 7 consecutive green nights with zero invariant violations.

Run it yourself:

```sh
AGENTREC_TORTURE_OPS=1200 cargo test --test torture -- --ignored
```

## Commands

| Command | Does |
|---|---|
| `agentrec init` | Scaffold `.agentrec/`, install agent hooks, install+load a per-repo recorder service. Idempotent. `--no-hook`, `--no-service`, `--dry-run` |
| `agentrec record` | Run the recorder daemon in the foreground for this repo |
| `agentrec log` | List recorded turns, newest first (git turns hidden by default). `--all`, `--json`, `--limit`, `--utc`, `--explain` |
| `agentrec diff <turn>` | Unified diff of a turn's file changes (turn id or unambiguous prefix) |
| `agentrec blame <file>[:line]` | Which turn last touched a file, or introduced a current line |
| `agentrec show <turn> [--prompt]` | Print a turn's header (excerpt discipline: no full prompt without the flag); `--prompt` prints the full post-scrub prompt text |
| `agentrec undo [turn]` | Revert a turn's changes, per file. Preview-only unless `--confirm`. `--allow-modified`, `--files a,b,c`. Omit `turn` for panic mode: targets the most recent non-git rich turn |
| `agentrec status` | Store size, recording gaps, rich-rate, DEGRADED banner on snapshot failures. `--ack-degraded` clears it |
| `agentrec doctor` | One-shot diagnosis of the whole recording chain: daemon liveness, hooks, signal freshness, store health, permissions, (Linux) inotify headroom. `--json` |
| `agentrec purge` | Delete blob objects: expired prompts by default (TTL from `config.toml`). `--all-prompts`, `--snapshots-before <DATE>`, `--memories-retracted` (archives expired retracted memory chains, never deletes) |
| `agentrec uninstall` | Remove hooks + service unit, archive `.agentrec/` to a sibling directory. Nothing is ever deleted. `--no-service` |
| `agentrec remember <fact> --from <paths>` | Record a manual, human-authored pinned memory. `--from` is a comma-separated list of repo-relative paths |
| `agentrec recall <query>` | Rank-then-verify search over pinned memories — only returns Fresh matches. `-k <n>`, `--json`. Verification is capped at 128 candidates per call; if the cap is hit, a notice prints to stderr ("results may be incomplete") since fresh matches could exist beyond it |
| `agentrec memories` | List recorded memories (audit view, not a query). `--stale` (non-fresh only), `--all` (include retracted), `--json` |
| `agentrec candidate <fact> --from <paths>` | Emit an agent-authored memory candidate for the daemon to validate, hash, and ingest. `--tool <name>` |
| `agentrec verify <id>` | Preview a memory's per-pin drift against the working tree; touches nothing without `--confirm`. `--confirm` re-pins, `--drop-pin <path>` (repeatable) explicitly drops an orphaned pin, `--replace-pin <old>=<new>` (repeatable) re-points an existing pin to an explicit successor path |
| `agentrec forget <id>` | Retract a memory — recoverable (quarantined, not deleted) until `purge --memories-retracted` archives it past TTL. `--reason <text>` |

Run `agentrec <command> --help` for the full flag reference.

## Memory

Beyond turns, agentrec can hold small, durable facts about a repo — "this endpoint requires an API key", "this test is flaky under load" — and hand the relevant ones back to the agent at the start of its next turn.

Every memory is **pinned**: it's tied to the content hash of the file(s) it was true about, not just the file's path. When a pinned file changes, the pin drifts — the memory is still recorded, but `recall` stops returning it until someone re-verifies it (`agentrec verify <id> --confirm`) or explicitly retracts it (`agentrec forget <id>`). This is the staleness guarantee: **recall only ever returns memories whose pins are still Fresh right now** — a memory about code that's since changed underneath it is never silently handed back as if it were still true.

A pinned file can also be *renamed* rather than edited — the old path is gone, so the pin looks orphaned even though the fact is still true of the file at its new location. `verify --replace-pin <old>=<new>` re-points that one pin to the successor path instead of dropping it:

```
agentrec verify a1b2c3d4 --confirm --replace-pin src/old_name.rs=src/new_name.rs
```

`new` is validated exactly like `remember --from`: it must resolve inside the repo root, contain no `..` traversal or symlink escape, actually exist on disk, and not match a secret-file pattern — any violation refuses with nothing appended. There is deliberately **no automatic rename detection**: `agentrec` never guesses that a deleted path and a new path are "the same file," since a wrong guess would silently re-ground a fact against the wrong source. You always name the successor explicitly.

Two write paths: `agentrec remember` for a human asserting a fact directly, and `agentrec candidate` for an agent proposing one (routed through the daemon, which validates, hashes, and scrubs it before it ever reaches disk — same as prompt persistence). Facts are always scrubbed of secrets before they touch `.agentrec/memory.jsonl`, matching the discipline the rest of the store already applies to prompts and snapshots.

On Claude Code's `UserPromptSubmit` hook, agentrec injects the top matching Fresh memories as a fenced block into the agent's context — budgeted and fail-open, so a corrupt or oversized store never blocks a prompt. Two `config.toml` keys control this:

- `memory_enabled` (default `true`) — kill switch; set `false` to disable **both** automatic paths: hook injection (no fenced block is ever printed) and the daemon's candidate ingestion (an `agentrec candidate` signal is consumed from the inbox but never written to `memory.jsonl`, live or on startup replay). It does not gate the manual `agentrec remember` / `verify` / `forget` verbs — those are deliberate user actions and always work.
- `memory_inject_max` (default `5`) — max facts injected per prompt.

`agentrec status` reports memory counters (`fresh`/`stale`/`rejects`/`injections`) alongside the usual turn stats.

`purge --memories-retracted` rewrites `memory.jsonl` in place; it refuses while the daemon is recording. A dedicated `.agentrec/memory.lock` (separate from the daemon's own lock) coordinates it with `remember`/`verify`/`forget`: a concurrent write simply waits for the purge to finish and lands right after — it is never silently lost. This flock is advisory and only covers agentrec's own writers (`remember`/`verify`/`forget`/`purge`/the daemon's candidate ingestion); a hand-rolled process that writes `memory.jsonl` directly, bypassing the lock, is out of scope for a local single-user tool.

## License

Apache-2.0 (see `LICENSE`). The open-source core is Apache-licensed; hosted /
team features on the roadmap (v4+) are offered separately under commercial terms.
