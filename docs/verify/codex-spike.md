# Codex hook spike — live verification (Phase A)

**Date:** 2026-08-05. **Worktree:** `agentrec-phase2-tail` @ `feat/phase-2-tail`.
**Pinned Codex CLI version (observed, not assumed):**

```
$ /opt/homebrew/bin/codex --version
codex-cli 0.146.0
```

Also cross-confirmed via `codex doctor` (`Codex Doctor v0.146.0 · macos-aarch64`, `runtime: brew
(package /opt/homebrew/Caskroom/codex/0.146.0, ...)`). Installed at `/opt/homebrew/bin/codex`,
Homebrew cask, macOS 26.5.2 (Darwin 25.5.0), logged in via ChatGPT.

Everything below is from **live runs against this exact pinned binary**, not from reading
`docs/reference/codex-hooks-docs-2026-08-05.md` and assuming it matches. Where the live behavior
confirms the doc, that's stated as "confirmed live". Where it diverges or the doc is silent, that's
called out explicitly. Nothing here is inferred without a command behind it (repo house rule).

## Method

- Disposable scratch git repo: `$SCRATCH/codex-spike-repo` (a `/private/tmp/...scratchpad/`
  path — never the agentrec repo itself, per the task's isolation requirement).
- Isolated `CODEX_HOME` (`$SCRATCH/codex-home`, only `auth.json` copied in from the real
  `~/.codex/auth.json` for login — nothing else copied) so probes never touched or mutated the
  user's real `~/.codex/hooks.json`, `~/.codex/config.toml`, or real per-repo trust state. All
  captures went to `$SPIKE_CAPTURE_DIR`, never inside the scratch repo's tracked tree.
- Capture hooks: three tiny Python scripts under `.codex/hooks/` in the scratch repo —
  `capture.py <event-name>` (dumps raw stdin JSON to a timestamped file, exits 0, no stdout —
  safe default for `Stop`'s JSON-or-nothing rule), `stop_toggle.py` (same capture, plus on first
  invocation only — gated by a state-file the probe controls — emits
  `{"decision":"block","reason":"..."}` on stdout), `stop_plaintext.py` (emits plain text on
  stdout, for the rejection probe).
- `codex exec <prompt>` was used for scripted/reproducible probes; a real interactive TUI session
  (driven via `tmux send-keys` + `capture-pane`, which — unlike a bare `expect`/pty script —
  answers the terminal capability queries (`\x1b[6n`, `\x1b[c`, OSC 10/11) the Ratatui-based TUI
  blocks on at startup) was used for the trust-flow (`/hooks`) and `/clear` probes, which have no
  `codex exec` equivalent.
- `--dangerously-bypass-hook-trust` was used only where the probe target was *not* the trust flow
  itself (functional captures); the trust flow itself (probe 1, probe 8) was run **without** that
  flag, through the real `/hooks` review UX.

All raw transcripts and every captured JSON payload from every run are preserved under
`$SCRATCH/captures/` (scratchpad-local, not committed — see fixture selection below for what
*is* committed).

## Field inventory (exact keys, as observed)

| Field | UserPromptSubmit | PostToolUse (`apply_patch`) | Stop |
|---|---|---|---|
| `session_id` | present | present | present |
| `turn_id` | present | present | present |
| `transcript_path` | present | present | present |
| `cwd` | present | present | present |
| `hook_event_name` | present | present | present |
| `model` | present | present | present |
| `permission_mode` | present (`"bypassPermissions"` under `--dangerously-bypass-approvals-and-sandbox`) | present | present |
| `prompt` | present | absent | absent |
| `tool_name` | absent | present (`"apply_patch"`) | absent |
| `tool_use_id` | absent | present | absent |
| `tool_input` | absent | present (`{"command": "<apply_patch diff text>"}`) | absent |
| `tool_response` | absent | present (human-readable exec summary string) | absent |
| `stop_hook_active` | absent | absent | present (`bool`) |
| `last_assistant_message` | absent | absent | present (`string \| null`) |

Every field name matches `docs/reference/codex-hooks-docs-2026-08-05.md` exactly — no
undocumented fields observed, no documented field observed absent on 0.146.0. `permission_mode`
is a *shared* field per the docs table; observed value in every capture was `"bypassPermissions"`
because all functional-capture runs used `-s workspace-write --dangerously-bypass-approvals-and-sandbox`
or the interactive default with `approval: never`; other `permission_mode` values (`default`,
`acceptEdits`, `plan`, `dontAsk`) were not exercised — not needed for decision 17's file-list
question, recorded as a gap rather than assumed.

Fixtures (redacted, `jq . <file>` exits 0 on all): `docs/fixtures/codex/`
- `user_prompt_submit.json`
- `post_tool_use_apply_patch.json` (multi-op: Update + Add + Add in one `apply_patch` call)
- `post_tool_use_apply_patch_delete.json` (second `apply_patch` call in the same turn, Delete)
- `stop.json`
- `stop_continuation_first_block.json` / `stop_continuation_second_after_block.json` (paired —
  see continuation semantics below; kept as a bonus pair beyond the required three events because
  they're the direct evidence for the turn_id-stability finding)

Redaction: the scratch repo's absolute path → `/REDACTED/scratch-repo`, `CODEX_HOME`'s absolute
path → `/REDACTED/codex-home`. `session_id`/`turn_id` (UUIDv7-shaped, no PII) and prompt text
(harmless test strings, e.g. `"Say hello. Do not run any commands or edit any files."`) were left
intact — they carry no secrets and the exact shape (UUID-like strings) is itself useful evidence.

## Decision 17 — PostToolUse `apply_patch` accumulator, path-extraction rule

**Confirmed live: `tool_input` has no structured file list.** `tool_input.command` is the raw
`apply_patch` custom-DSL patch text, e.g.:

```
*** Begin Patch
*** Update File: hello.txt
@@
+line two
*** Add File: second.txt
+second file
*** Add File: to_delete.txt
+delete me
*** End Patch
```

Observed op headers: `*** Add File: <path>`, `*** Update File: <path>`, `*** Delete File: <path>`.
(`*** Move to: <path>` for renames is documented in the general `apply_patch` DSL elsewhere but was
**not exercised live** in this spike — no live evidence for it; do not assume its exact syntax
without a run.)

**Path-extraction rule (confirmed live):** regex over lines of `tool_input.command` matching
`^\*\*\* (Add|Update|Delete) File: (.+)$`, collecting group 2. **One `PostToolUse` firing = one
`apply_patch` tool call**, which **can bundle multiple file operations** (observed: 3 ops — one
Update, two Add — in a single `tool_input.command`, confirmed by `tool_response`: `"Success.
Updated the following files:\nA second.txt\nA to_delete.txt\nM hello.txt\n"`). A second,
separate `apply_patch` call in the same turn (a `Delete`) produced a **second, separate**
`PostToolUse` firing with its own `tool_use_id` — do not assume 1 firing per turn; it is 1 firing
per `apply_patch` tool call, and a turn can contain several.

This directly grounds decision 17: no transcript parsing needed or usable for the file list; the
accumulator must parse `tool_input.command` per `PostToolUse` firing and union path sets across
however many firings occur before the paired `Stop`.

## Trust flow

### Confirmed live: silent skip is the default in `codex exec`, not a warning

With a repo-local `.codex/hooks.json` present and **never trusted**, and **no**
`--dangerously-bypass-hook-trust`: `codex exec` ran clean, exit 0, **zero mention of hooks
anywhere in stdout or stderr**, and the capture directory stayed empty. The documented "Codex
prints a warning that tells you to open `/hooks`" behavior is real, but **it is TUI-only** — the
non-interactive `exec` path has no such warning. Anything relying on that warning for
CI/automation visibility will see nothing.

### Confirmed live: the real `/hooks` trust UX (interactive TUI, via `tmux`)

First interactive launch in the (still-untrusted) scratch repo shows a full-screen gate before any
prompt can be entered:

```
  Hooks need review
  3 hooks are new or changed.
  Hooks can run outside the sandbox after you trust them.

› 1. Review hooks
  2. Trust all and continue
  3. Continue without trusting (hooks won't run)

  Press enter to confirm or esc to go back
```

Choosing "Review hooks" opens the `/hooks` browser (same screen `/hooks` opens directly later):

```
  Hooks
  Lifecycle hooks from config and enabled plugins.

  ⚠ 3 hooks need review before they can run.

  Event                 Installed   Active      Review      Description
  PreToolUse            0           0           0           Before a tool executes
  PermissionRequest     0           0           0           When permission is requested
  PostToolUse           1           0           1           After a tool executes
  ...
  UserPromptSubmit      1           0           1           When the user submits a prompt
  ...
  Stop                  1           0           1           Right before Codex ends its turn

  Press t to trust all; enter to review hooks; esc to close
```

Drilling into one event shows the **exact hook definition** — this is the "review the exact hook
definition" the docs describe, confirmed field-for-field:

```
  PostToolUse hooks
  1 hook needs review before it can run.

  [!] Hook 1 · new

  Event     PostToolUse
  Matcher   apply_patch
  Source    Project config - /REDACTED/scratch-repo/.codex/hooks.json
  Command   python3 /REDACTED/scratch-repo/.codex/hooks/capture.py PostToolUse_apply_patch
  Timeout   10s
  Trust     New hook - review required

  Press t to trust; esc to go back
```

Pressing `t` there flips `Trust` to `Trusted` and the per-hook checkbox to `[x]`; going back to the
event list shows `Review` decremented for that event only (the other two events' counts were
untouched — trust is genuinely per-hook, not per-event or per-file). "Trust all" (`t` from the
top-level `/hooks` list) trusts every currently-listed unreviewed hook in one action and the
Review column disappears once the count hits 0.

**Confirmed: trust persists outside the interactive session and covers `codex exec` too.** After
trusting all 3 hooks via `/hooks`, a subsequent `codex exec` run on the **same** scratch repo, with
**no** `--dangerously-bypass-hook-trust` flag, ran the hooks cleanly (`hook: UserPromptSubmit` /
`Completed`, `hook: Stop` / `Completed` in the transcript, real capture files written). Trust is
stored keyed to the hook (by content, not by session) and consumed by both surfaces.

### `init` implication (informational, feeds C3 — not built in this phase)

Because trust is per-hook-hash and TUI-gated, `agentrec init --codex` cannot silently grant trust;
it can only write the hook entries and must tell the user to run `/hooks` (or, for
automation/CI, use `--dangerously-bypass-hook-trust` — see below). This spike observed the UX
`init`'s printed instructions should point at; it did not modify `init` (no production code in
this phase).

## Hash-invalidation — "ANY field invalidates trust" (converted from inference to fact)

Two independent live tests, both against a **trusted** 3-hook `hooks.json`:

**Test A — non-command field only (`timeout` on the `Stop` hook, 10 → 45, command/matcher
untouched):** next `codex exec` (no bypass flag) ran `UserPromptSubmit` normally but **`Stop` did
not fire at all** (no `hook: Stop` line, no capture) — same silent-skip as an unreviewed hook.
Opening `/hooks` interactively confirmed structurally: startup banner read **"1 hook is new or
changed"** (not 3), and the event table showed `Stop: Installed=1 Active=0 Review=1` while
`PostToolUse`/`UserPromptSubmit` stayed `Active=1 Review=0` — i.e. **only the mutated hook lost
trust; its two untouched siblings stayed trusted.**

**Test B — command field only (appended `" --extra-flag"` to the `UserPromptSubmit` hook's
`command`, `Stop`'s timeout reverted to its original `10`):** next `codex exec` (no bypass) showed
`UserPromptSubmit` silently skipped (command changed → untrusted) while **`Stop` fired
normally** — because its live definition was now byte-for-semantic-content identical to what had
already been approved.

**Conclusion: the plan's C3 "ANY changed field (command, timeout, statusMessage) is skipped until
re-trusted" claim is CONFIRMED as fact, not inference** — demonstrated on both a command-field
change and a non-command-field change, independently, each isolated to one hook while its siblings
were unaffected.

**Bonus finding, not in the original probe list but load-bearing for C3's "keep each installed
hook entry byte-stable across reinstalls" requirement:** reverting the `Stop` hook's `timeout`
back to its original value (`45` → `10`) **silently restored trust with no re-review** — the next
`codex exec` ran it clean. This means trust is keyed on a hash of the **parsed hook definition**
(and Codex appears to remember more than just the single most-recently-approved hash — reverting
to a prior state that was once approved works without a new approval step), not on raw file bytes:
re-serializing the same `hooks.json` with different JSON whitespace (compact → `indent=2`, which
happened incidentally between test runs here) made **no difference** to any trust decision
observed in this spike.

## `hooks.json` + inline `[hooks]` merge + startup warning

Confirmed live, exact wording. With the trusted `hooks.json` (repo-local) **and** a new
`.codex/config.toml` containing a second `UserPromptSubmit` hook (`[[hooks.UserPromptSubmit]]` /
`[[hooks.UserPromptSubmit.hooks]]`) added in the same layer:

```
warning: loading hooks from both /REDACTED/scratch-repo/.codex/hooks.json and
/REDACTED/scratch-repo/.codex/config.toml; prefer a single representation for this layer
```

This warning appears **both** in the `codex exec` transcript (stderr/stdout, unconditionally, even
without any interactive session) **and** as an "Issues" line inside the `/hooks` browser. The
`/hooks` event table showed `UserPromptSubmit: Installed=2 Active=1 Review=1` — **both** the
`hooks.json` entry and the new `config.toml` entry are loaded (Installed=2, confirming true merge,
not one silently overriding the other); only the previously-trusted one was `Active` until the new
one was reviewed. This matches the plan's C3 assumption exactly: **merge, not replace; warn, don't
refuse.**

## Continuation semantics (decision 17 / C1 dedup — the scariest unknown)

**Setup:** a `Stop` hook (`stop_toggle.py`) that on its *first* invocation per run returns
`{"decision":"block","reason":"continue once - spike probe"}` and on every subsequent invocation
returns nothing (empty stdout, exit 0). Run twice independently against a fresh scratch prompt
each time.

**Observed both times (2/2 independent runs):**

| | Run 1 | Run 2 |
|---|---|---|
| `Stop` #1 `turn_id` | `019fd1b6-1221-…` | `019fd1b6-7762-…` |
| `Stop` #1 `stop_hook_active` | `false` | `false` |
| `Stop` #2 `turn_id` | `019fd1b6-1221-…` (**identical to #1**) | `019fd1b6-7762-…` (**identical to #1**) |
| `Stop` #2 `stop_hook_active` | `true` | `true` |
| `UserPromptSubmit` firings this run | 1 (only the original prompt) | 1 (only the original prompt) |

Terminal evidence (Run 1): `hook: UserPromptSubmit` → `Completed`, then `codex` responds
`Hello!`, then `hook: Stop` → **`Stop Blocked`**, then `codex` responds `Hello again.` (the
model's continuation, driven by the hook's `reason` text acting as the synthetic next prompt, per
docs), then `hook: Stop` → `Stop Completed`.

**Answers to the plan's exact questions:**
1. **Does `turn_id` change for the continuation turn?** — **NO. `turn_id` is STABLE across the
   block-continuation, confirmed identically on two independent runs.** This is the **opposite**
   of the plan's feared blocking scenario. Decision 17's drain-by-`turn_id` keying is **NOT**
   broken by this mechanism — a `PostToolUse`→`Stop` accumulator keyed on `turn_id` will correctly
   see all `PostToolUse` firings across a block-continuation as belonging to the same turn.
2. **Does `UserPromptSubmit` fire again for the synthetic continuation prompt?** — **NO.** Exactly
   one `UserPromptSubmit` capture per run, for the original prompt only. The synthetic
   continuation prompt is invisible to `UserPromptSubmit` hooks entirely.
3. **Is there a `stop_hook_active`-style continuation indicator?** — **YES, and it works exactly as
   documented**: `false` on the first `Stop` firing, `true` on every firing after a block has
   already fired once for this turn.

**No founder escalation needed for this finding** — the plan's contingency ("if `turn_id` proves
UNSTABLE ... go back to the founder before Phase C") is not triggered; the opposite was observed.
This is recorded here as the authoritative live answer superseding the plan's open question.

## `Stop`/`Stop`-adjacent behavior: interrupt and `/clear`

- **Normal turn end:** `Stop` fires once, `stop_hook_active: false`. Confirmed on essentially every
  run above.
- **Interrupt (SIGINT mid-turn, sent to the `codex exec` process while a shell command was
  running):** `codex exec` printed `turn interrupted` and exited with code 1. **`Stop` did NOT
  fire** — only the earlier `UserPromptSubmit` capture existed afterward. Caveat: this was tested
  against `codex exec` process-level `SIGINT` (the closest safe, scriptable analogue to
  interactive Ctrl-C available in this non-interactive harness); the interactive TUI's Ctrl-C
  handling was not separately probed and could plausibly differ — recorded as a gap, not
  extrapolated.
- **`/clear` (interactive TUI only — no `codex exec` equivalent exists):** driven live via `tmux`.
  Sending `/clear` after a completed turn printed `Token usage: ...` /
  `To continue this session, run codex resume <old-session-id>` and reset the composer.
  **No `Stop` fired for the abandoned session** (no new capture appeared), and **no
  `UserPromptSubmit` fired for `/clear` itself** (it's a slash command, not a prompt). The next
  real prompt after `/clear` carried a **brand-new `session_id` and `turn_id`**, confirmed by
  diffing the pre- and post-clear `UserPromptSubmit` captures. **Gap, stated plainly:** this spike
  did not wire a `SessionStart` hook, so the documented `SessionStart` event with
  `source: "clear"` was not directly captured — only its *absence-of-Stop* and *fresh-IDs*
  side effects were observed. If a future phase needs the exact `SessionStart` payload shape for
  `clear`, that is a follow-up live probe, not something to infer from this spike.
- **`SubagentStop`:** not exercised — reliably triggering a Codex subagent spawn was out of scope
  for this spike's effort budget. No claim is made about it either way.

## `Stop` rejects plain-text stdout (confirmed live)

With the `Stop` hook swapped for one that writes `"this is plain text, not json\n"` to stdout and
exits 0 (trusted via `--dangerously-bypass-hook-trust` for this functional test, since the
mutated command needed re-review and trust-flow itself was already covered above): the transcript
showed `hook: Stop` → **`Stop Failed`** (vs. `Stop Completed` for empty/JSON stdout). The turn
still completed normally and the session did not hang — a failed `Stop` hook is reported and
otherwise ignored, it does not block turn termination. **Confirmed separately: empty stdout (exit
0, no output) is treated as success**, not as the invalid case — every `capture.py`-based `Stop`
run above (which emits nothing) showed `Stop Completed`. So the actual rule is "non-empty
non-JSON stdout is invalid", matching "Exit 0 with no output is treated as success" from the
docs' common-output-fields section, not "any non-JSON output including none."

The `--json` event stream (`codex exec --json`) does **not** surface hook pass/fail as a
structured event — the `hook: Stop Failed` line is TUI/human-transcript-only. Automation reading
`--json` output has no direct signal that a `Stop` hook failed; recorded as a gap for C2 to be
aware of (the emitter itself never needs to read this, since it never emits non-JSON, but doctor
or CI tooling wanting to detect broken *other* hooks in a shared config can't rely on `--json`
for it).

## `--dangerously-bypass-hook-trust` (confirmed live, exact flag name matches docs)

`codex exec --help` lists it verbatim:

```
--dangerously-bypass-hook-trust
    Run enabled hooks without requiring persisted hook trust for this invocation. DANGEROUS.
    Intended only for automation that already vets hook sources
```

Functionally confirmed: on an otherwise-never-trusted scratch repo, adding this flag to `codex
exec` made all three configured hooks fire (`hook: UserPromptSubmit`/`Stop`/`PostToolUse` +
`Completed` lines) and printed this banner twice per invocation (once per triggered hook event in
this run):

```
warning: `--dangerously-bypass-hook-trust` is enabled. Enabled hooks may run without review for
this invocation.
```

Trust is **not** persisted by this flag — it's a one-invocation bypass, not an implicit trust-all;
confirmed by the fact that a subsequent bypass-free `codex exec` on the same untouched repo went
back to silent-skip (see Trust flow above). Sanctioned for CI/automation legs per the plan (O5).

## VERIFY-LEDGER row

See `/Users/ravichandrasekhar/Projects/agentrec-phase2-tail/VERIFY-LEDGER.md` — row appended
under a new "## Phase A — Codex hook spike" heading, same commit as this document.

## What was NOT probed (explicit, not silently skipped)

- `SubagentStop`, `SubagentStart`, `SessionStart`, `SessionEnd`, `PreToolUse`,
  `PermissionRequest`, `PreCompact`, `PostCompact` — out of this phase's required probe set;
  zero live evidence gathered, no claims made about their behavior on 0.146.0.
- Interactive-TUI Ctrl-C (only process-level `SIGINT` against `codex exec` was tested for the
  interrupt probe — see above caveat).
- `SessionStart` payload shape for `source: "clear"` — only `/clear`'s *side effects* (no Stop,
  fresh IDs) were observed, not the event itself (no `SessionStart` hook was wired this round).
- Managed/enterprise hooks (`requirements.toml`, `allow_managed_hooks_only`), plugin-bundled hooks,
  Windows `commandWindows` — no live evidence, not in this phase's scope.
- `codex mcp-server` / MCP-tool `PreToolUse`/`PostToolUse` matcher paths — out of scope (this spike
  is about the Codex-as-hook-emitter side, not agentrec's own future MCP server).
