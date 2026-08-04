# agentrec — Claude Code plugin

Packaging for [agentrec](https://github.com/ravi1395/agentrec), the local-first
flight recorder for coding agents. The plugin bundles the two lifecycle hooks
(`UserPromptSubmit` + `Stop`) that give agentrec fully-attributed *rich* turns,
and a `/agentrec:setup` command that wires any repo in one step.

## Install

```
/plugin marketplace add ravi1395/agentrec
/plugin install agentrec@agentrec
```

Then, in any repo you want recorded:

```
/agentrec:setup
```

## What the hooks do

Both hooks run the same guarded command:

```sh
sh -c '[ -d .agentrec ] && command -v agentrec >/dev/null 2>&1 && exec agentrec hook claude; exit 0'
```

- **No-op everywhere except initialized repos.** The `[ -d .agentrec ]` guard
  means repos you never ran `agentrec init` in are untouched — no files
  created, no signals written. (This is a deliberate deviation from the
  repo-local hook `agentrec init` installs, which needs no guard because it
  only exists in initialized repos: a global hook without the guard would
  create `.agentrec/signal.jsonl` in every project you open.)
- **No-op when the binary is missing** — the plugin never errors a session
  that doesn't have agentrec installed.
- Otherwise it forwards the hook payload to `agentrec hook claude` unchanged —
  the exact command the repo-local install uses, so turn semantics are
  identical (PROTOCOL.md §4).

## Plugin + `agentrec init` interaction

Use `agentrec init --no-hook` in repos where this plugin is active (that is
what `/agentrec:setup` runs). Bare `init` additionally merges repo-local hooks
into `.claude/settings.local.json`, and with both in place every signal is
emitted twice. Existing repo-local hooks can be left as-is if you prefer them —
just don't run both.

## What you get

- `agentrec blame <file>[:line]` — the turn, tool, and prompt that wrote a line,
  and whether it was edited since
- `agentrec undo <turn>` — per-turn revert that refuses files modified since
- `agentrec log` / `diff` — readable per-turn history, git churn hidden
- `agentrec import claude` — backfill attribution from existing session
  transcripts

Everything is local: JSONL + content-addressed snapshots under `.agentrec/` in
the repo. No network code in the recording path.
