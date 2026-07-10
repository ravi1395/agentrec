# agentrec

A local-first, tool-agnostic flight recorder for coding agents. It segments agent activity into *turns*, snapshots every touched file into a content-addressed store, and answers the question that matters after a bad session:

**Who broke my repo — me or the agent?**

No cloud, no telemetry, no vendor lock — works with any tool that can fire a lifecycle hook (Claude Code today; Codex, and anything else, additively).

## Line-level blame

The headline feature: point at a file and a line, get the turn that wrote it, the tool and prompt behind it, and whether it's been touched since.

![line-level blame demo](docs/blame-demo.gif)
*(GIF pending — recorded from a real session; the example below is real CLI output shown as text in the meantime.)*

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
curl -fsSL https://raw.githubusercontent.com/agentrec/agentrec/main/install.sh | sh
```

This downloads a prebuilt binary for your OS/arch, verifies its sha256 checksum, and installs it to `~/.local/bin/agentrec` — no sudo, ever. If `~/.local/bin` isn't on your `PATH`, the installer tells you exactly what to add.

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
| `agentrec purge` | Delete blob objects: expired prompts by default (TTL from `config.toml`). `--all-prompts`, `--snapshots-before <DATE>` |
| `agentrec uninstall` | Remove hooks + service unit, archive `.agentrec/` to a sibling directory. Nothing is ever deleted. `--no-service` |

Run `agentrec <command> --help` for the full flag reference.

## License

Apache-2.0 (see `LICENSE`). The open-source core is Apache-licensed; hosted /
team features on the roadmap (v4+) are offered separately under commercial terms.
