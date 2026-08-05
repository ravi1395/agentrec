# O5 — live two-tool session evidence (Phase C exit)

**Date:** 2026-08-05. **Worktree:** `agentrec-phase2-tail` @ `feat/phase-2-tail`.
**Binary under test:** debug build, `cargo build -p agentrec` (NOT `--release`) —
`target/debug/agentrec`, `agentrec 0.2.0`. Disclosed per the task's constraint; O5 is a
protocol/attribution correctness check, not a performance measurement, so debug is adequate and
faster to iterate against.

Disposable scratch git repo under this session's scratchpad (`/private/tmp/.../scratchpad/o5/`,
redacted below as `/REDACTED/scratch-repo`), never the agentrec repo itself. Isolated `CODEX_HOME`
(`/REDACTED/codex-home`, only `auth.json` copied in) — mirrors the Phase A spike's isolation
method (`docs/verify/codex-spike.md`) so this round never touched the real `~/.codex/hooks.json`
or the machine's real per-repo Codex trust state. Pinned Codex CLI: `codex-cli 0.146.0`
(`/opt/homebrew/bin/codex`) — same pin as Phase A; an update to 0.146.1 was available and NOT
taken, to keep this round on the already-spiked version.

## Part 0 — temp-root service guard (D46), verified rather than assumed

`agentrec init --codex` in the scratch repo (a `/private/tmp/...` root) printed:

```
skipped service install: root is under a temporary directory (/private/tmp) — a service
installed here outlives the directory; pass --service to install anyway
```

`launchctl list | grep agentrec` and `ls ~/Library/LaunchAgents | grep agentrec` were checked
**before this round started** and **after every step**: exactly one unit throughout,
`com.agentrec.bfa6bde6eaa4` (the pre-existing live dogfood daemon for `~/Projects/agentrec`,
unrelated to this round). No plist was ever written for the scratch repo. `--service` was never
passed. Guard confirmed live, not assumed from the D46 changelog entry.

## Part 1 — the headline finding: hook-process cwd, root launch vs. subdirectory launch

**Question (posed by the task, never previously measured by anyone):** the installed Codex hook
entry is the bare command `agentrec hook codex`, no `--root` flag
(`cli/src/initcmd.rs::CODEX_HOOK_COMMAND`) — so `agentrec`'s root defaults to
`std::env::current_dir()` of the hook process (`cli/src/main.rs:386-389`). What cwd does Codex
actually give its hook processes?

**Method:** a wrapper script (`cwd_probe_wrapper.sh`, not committed — scratchpad-local, see
below for its logic) was substituted for `agentrec hook codex` in `.codex/hooks.json`'s three
hook entries. It records `pwd -P` (the hook subprocess's real cwd) and the payload's own `cwd`
field to a capture file **outside** the repo, then forwards stdin unmodified to the real
`agentrec hook codex` (resolved via `PATH`, exactly as Codex invokes it) so downstream behavior
is unaffected. Substituting the command changes its hash and un-trusts the hook (Phase A:
"ANY field change... revokes trust for exactly that hook"), so this leg used
`codex exec --dangerously-bypass-hook-trust` — sanctioned per the plan text for scripted/CI legs.
Two `codex exec` runs, **both from the same trusted scratch repo**, launched from two different
directories:

1. `cd /REDACTED/scratch-repo && codex exec ...` (repo root)
2. `cd /REDACTED/scratch-repo/sub && codex exec ...` (a tracked subdirectory of the same repo)

**Raw capture (redacted):**

```
event=UserPromptSubmit
process_pwd=/REDACTED/scratch-repo
payload_cwd=/REDACTED/scratch-repo
---
event=PostToolUse
process_pwd=/REDACTED/scratch-repo
payload_cwd=/REDACTED/scratch-repo
---
event=Stop
process_pwd=/REDACTED/scratch-repo
payload_cwd=/REDACTED/scratch-repo
---
event=UserPromptSubmit
process_pwd=/REDACTED/scratch-repo/sub
payload_cwd=/REDACTED/scratch-repo/sub
---
event=PostToolUse
process_pwd=/REDACTED/scratch-repo/sub
payload_cwd=/REDACTED/scratch-repo/sub
---
event=Stop
process_pwd=/REDACTED/scratch-repo/sub
payload_cwd=/REDACTED/scratch-repo/sub
---
```

**Finding, stated plainly: Codex's hook-process cwd IS the directory Codex was launched from —
it tracks the launch directory, not the git repo root.** Confirmed by a second, independent
channel: the interactive TUI's own startup banner (see Part 2 below) printed
`directory: /REDACTED/scratch-repo` when launched from the repo root — same answer as the `pwd`
probe, for the root-launch case.

**Consequence, observed directly (not inferred):** after the subdirectory-launch run,
`find /REDACTED/scratch-repo -iname .agentrec -type d` returned **two** directories:
`/REDACTED/scratch-repo/.agentrec` (2 signal lines, from the earlier root-launch run) and
`/REDACTED/scratch-repo/sub/.agentrec` (a **second, independent** `signal.jsonl`, `config.toml`
absent — `record.rs::open_append`/hook append path creates parent dirs with no error). A daemon
running `agentrec record --root /REDACTED/scratch-repo` (the correct root) has no way to see
`sub/.agentrec/signal.jsonl` — the daemon only tails its own root's inbox. Every signal from a
Codex session launched from that subdirectory is **silently and permanently invisible** to the
repo's real recorder. No error, no warning, at any point in this chain.

**This is a BLOCKING product defect, reported here rather than fixed** (out of scope for this
verification round; a fix is a design decision — most plausibly baking `--root <repo-root>`
into the installed hook command instead of relying on cwd, which needs the actual repo root
resolved and pinned at `init --codex` time — and must be escalated to the founder, not decided
by the executor). **Scope of what was and wasn't measured:** both positions were driven via
`codex exec`, not the interactive TUI (the TUI has no non-interactive way to re-launch per
position without a fresh trust cycle each time); the root-launch cwd is corroborated by the TUI
channel independently, the subdirectory-launch cwd is **not** — but there is no mechanism by
which the TUI and `codex exec` would source cwd differently (both are the same `codex-cli`
binary reading `std::env::current_dir()` at the same startup point), so this is recorded as a
scope note, not a live caveat on the finding itself.

## Part 2 — real `/hooks` trust flow (interactive TUI, via `tmux`)

Per the Phase A spike's method (`send-keys`/`capture-pane` — a bare `expect` pty hangs on
Ratatui's terminal-capability queries): `tmux new-session` launched a small wrapper script that
`cd`s into the scratch repo, sets `CODEX_HOME`/`PATH`, and `exec`s `codex` with **no** bypass
flag, against the **unmodified** `.codex/hooks.json` (`"command": "agentrec hook codex"`,
`agentrec` resolved via a `PATH` entry pointing at the debug binary). First launch showed the
same gate the spike documented:

```
  Hooks need review
  3 hooks are new or changed.
  Hooks can run outside the sandbox after you trust them.

› 1. Review hooks
  2. Trust all and continue
  3. Continue without trusting (hooks won't run)
```

"Review hooks" (option 1) was chosen, landing on the `/hooks` browser showing all three events
(`PostToolUse`/`UserPromptSubmit`/`Stop`) each `Installed=1 Active=0 Review=1`. Pressing `t`
("trust all") flipped every row to `Active=1`, Review column disappeared. This is the real,
live, un-bypassed trust UX — the one leg of O5 the plan explicitly requires stay unscripted.

## Part 3 — the honesty leg itself: one repo, both tools, one `log.jsonl`

`agentrec --root /REDACTED/scratch-repo record` (debug binary) started in the background against
the scratch repo (the correct root — this leg deliberately avoids Part 1's subdirectory defect
so it measures cross-attribution, not restates the cwd bug).

**Codex leg — LIVE, real interactive session, now-trusted hooks, no bypass flag.** In the same
`tmux` session from Part 2, prompted: *"Create a file named codex_leg.txt containing exactly one
line: edited by codex. Do not run any other commands."* Codex created the file for real
(`Added codex_leg.txt (+1 -0)` in the TUI transcript). All three hooks fired
(`UserPromptSubmit`/`PostToolUse`/`Stop` all appeared in prior spike transcripts as `Completed`;
this session's `signal.jsonl` shows the resulting `start`/`stop` pair below).

**Claude leg — NOT a live Claude Code process. Driven by invoking `agentrec hook claude`
directly with realistic UserPromptSubmit/Stop JSON payloads on stdin, exactly as Claude Code's
own hooks would invoke it, plus a real file write (`echo ... > claude_leg.txt`) in between so the
daemon's fs watcher observes a genuine mutation.** This is explicit and disclosed per the task's
instruction — no nested interactive Claude Code session was or could be spawned by this agent.
Commands run:

```
echo '{"hook_event_name":"UserPromptSubmit","session_id":"<id>","prompt":"Create claude_leg.txt
  with the line: edited by claude-code. Do not run any other commands."}' \
  | agentrec --root /REDACTED/scratch-repo hook claude
echo "edited by claude-code" > claude_leg.txt
echo '{"hook_event_name":"Stop","session_id":"<id>"}' \
  | agentrec --root /REDACTED/scratch-repo hook claude
```

**Resulting `signal.jsonl` (redacted, chronological, both legs):**

```
{"v":1,"tool":"codex","event":"start", ...,"prompt":"Create a file named codex_leg.txt containing exactly one line: edited by codex. Do not run any other commands.","emitter_turn":"019fd332-...","model":"gpt-5.6-terra"}
{"v":1,"tool":"codex","event":"stop", ...,"files_written":["/REDACTED/repo/codex_leg.txt"],"emitter_turn":"019fd332-..."}
{"v":1,"tool":"claude","event":"start", ...,"prompt":"Create claude_leg.txt with the line: edited by claude-code. Do not run any other commands."}
{"v":1,"tool":"claude","event":"stop", ...}
```

**Resulting `log.jsonl` (redacted):**

```
{"type":"epoch","event":"start", ...}
{"type":"turn","id":"t_...6D4C","grade":"rich","tool":"codex","model":"gpt-5.6-terra", ...,"prompt_excerpt":"Create a file named codex_leg.txt...","files":[]}
{"type":"turn","id":"t_...QG4B","grade":"bare", ...,"files":[{"path":"codex_leg.txt","op":"create", ...}]}
{"type":"turn","id":"t_...WGTR","grade":"rich","tool":"claude", ...,"prompt_excerpt":"Create claude_leg.txt...","files":[]}
{"type":"turn","id":"t_...ZT3N","grade":"bare", ...,"files":[{"path":"claude_leg.txt","op":"create", ...}]}
{"type":"epoch","event":"stop", ...}
```

`agentrec log`:

```
t_01KZ…ZT3N · bare · — · just now · 1 file
t_01KZ…WGTR · rich · claude · just now · 0 files · "Create claude_leg.txt with the line: edited by claude-code. Do not run any other commands."
t_01KZ…QG4B · bare · — · 2m ago · 1 file
t_01KZ…6D4C · rich · codex · 2m ago · 0 files · "Create a file named codex_leg.txt containing exactly one line: edited by codex. Do not run any other commands."
```

`agentrec show <id>` on both rich turns confirms `tool` renders as `codex` / `claude`
respectively, with the correct prompt excerpt attached to each.

## Cross-attribution check — jq-scoped, not substring grep

A naive `grep claude log.jsonl | grep codex_leg` returns false positives, because this session's
own scratchpad path contains the literal substring `claude-501` (the harness's own tmp-dir
naming) — a reminder that substring grep is the wrong tool here. Scoped `jq` queries instead:

```
$ jq -c 'select(.tool=="codex") | .files // .prompt_excerpt' log.jsonl
[]
$ jq -c 'select(.tool=="claude") | .files // .prompt_excerpt' log.jsonl
[]
$ jq -c 'select(.type=="turn" and .grade=="bare") | .files[].path' log.jsonl
"codex_leg.txt"
"claude_leg.txt"
```

**Zero cross-attribution, confirmed: `codex_leg.txt` never appears under `tool=="claude"`;
`claude_leg.txt` never appears under `tool=="codex"`.** Each rich turn's own `files` array is
empty for both tools — see the disclosed limitation below, which affects both tools identically
and is not a cross-attribution defect.

**Design note on why this check can't fail silently:** the two legs were run strictly
sequentially, each session's `Stop` fully processed before the next leg's `UserPromptSubmit`
was sent, with distinct filenames per leg (`codex_leg.txt` / `claude_leg.txt`). An open
bracket (start-without-stop) suppresses quiet-window closure and retroactively folds interim
bare turns into the open rich turn (PROTOCOL bracketing semantics) — running the legs
concurrently or interleaved would risk manufacturing cross-attribution from the test's own
design rather than measuring the real thing. **Only the sequential case was exercised here.**
Interleaved/concurrent multi-tool sessions are the pre-existing, already-disclosed D6
misattribution risk (README threat model, `fix/redteam-immediate-actions` T1) — not a new
finding of this round.

## Disclosed limitation, observed directly on both legs (not new — matches documented status)

Both rich turns (`codex` and `claude`) rendered `"files":[]`; the actual file write in each case
landed in a separate, immediately-following **bare** (unattributed) turn instead. This is
consistent with the CLAUDE.md Status section's own disclosure that the D6 `files_written`
wedge's `resolve_declared` tier ladder "landed dark" — phases 1-2b wired the signal field
end-to-end onto the wire (confirmed above: `files_written` is present and correct on the
`codex` stop signal) but nothing yet consumes it to populate a turn's rendered `files` list at
persist time ("producer at persist" is the next phase, per CLAUDE.md's Now/next list). This
round is live confirmation of that documented gap, not a new defect — and critically, it affects
**both tools identically**, so it does not compromise the cross-attribution check above (a file
landing in an unattributed bare turn is not the same failure as a file landing under the wrong
tool's rich turn — the latter never happened).

## What was NOT done — OPEN, stated rather than papered over

- The Claude Code leg is direct hook-invocation, never a live Claude Code process, per the
  task's own acknowledged constraint (no nested interactive Claude Code session is possible from
  this environment). The Codex leg IS live and real, hooks trusted through the genuine `/hooks`
  TUI flow, no bypass flag on the honesty-leg session itself.
- Interleaved/concurrent two-tool sessions on the same repo were not exercised (see Design note
  above) — only sequential, non-overlapping brackets.
- The subdirectory-launch cwd measurement (Part 1) used `codex exec`, not the TUI, for the
  reason stated there.
- `agentrec-release`/publish flows, MCP surfaces, and everything outside O5's stated scope were
  untouched.

## Cleanup performed

`tmux kill-session`; the daemon process (this round's own, PID captured at start) was `kill`ed
by PID — **other, pre-existing `agentrec record` processes found running against unrelated
`/var/folders/.../T/.tmp*` roots (leftover from other work, started hours before this round) were
deliberately left untouched**, out of this task's scope. `launchctl list`/`~/Library/LaunchAgents`
re-checked after cleanup: unchanged from the Part 0 baseline (one unit, the pre-existing dogfood
daemon). Scratch repo and captures left on scratchpad disk (session-local, not committed).
