# Phase F exit — the self-healing acceptance story, run live

**Date:** 2026-08-06. **Worktree:** `agentrec-phase2-tail` @ `feat/phase-2-tail`, HEAD `78fbc36`
(clean tree at the start of the round; verified with `git status --porcelain` → empty).

**Binary under test — release, deliberately** (this is the artifact users get, and the plan's
Phase F exit is a product-behavior story, not a unit-level check):

```
$ cargo build --release            # exit 0
$ target/release/agentrec --version
agentrec 0.2.0
$ shasum -a 256 target/release/agentrec
c1c00649dc3b1147120e0c2f1ffbe083eed00eaa4f912ce3a5c9ac10f3ade65e  .../target/release/agentrec
```

**The story this round is discharging** (plan `2026-08-05-phase-2-tail-plan.md`, "Phase F exit —
self-healing acceptance story", verbatim):

> Disposable real repo: agent breaks named test → blame/diff finds its own turn → preview → grant
> (confirm once, auto once) → execute → test green → undo visible in log as turn. Trace + tool
> calls + hashes retained as evidence; runnable with daemon stopped during reads. VERIFY-LEDGER
> row. NOT scripted-output theater.

It exists because the Phase F gate recorded a blocker: *"the destructive loop end-to-end has zero
live evidence."* Every line below is the output of a command actually run in this session. Where
the first attempt did not do what was expected, the first attempt is written down (see § "Blocking
pre-check" — it changed the shape of the run).

**Redaction.** The disposable repo lives at
`/private/tmp/claude-501/-Users-.../scratchpad/fexit`. It is written `/REDACTED/fexit` below;
`agentrec`'s own output prints the real absolute path, so those blocks are shown as emitted except
for the same substitution.

**Agent-turn simulation — stated up front.** The "agent" in this story is driven through the real
hook emitter (`agentrec hook claude` with `UserPromptSubmit` / `Stop` payloads on stdin), exactly
the way `cli/tests/misattribution.rs::send_hook` drives it, wrapping a real file edit. This is the
sanctioned hook-driven option in the task text; no `claude -p` session was used in this round. What
that buys and what it does not: the bracketing, signal inbox, daemon consumption, turn minting, CAS
snapshotting, and every read/destructive verb below are the real production paths. What is
simulated is only the identity of the process performing the write.

---

## Part 0 — hygiene baseline, taken before anything was started

```
$ pgrep -f "/agentrec record --root|/agentrec mcp"     # PID, start time, command
  865 Fri Jul 31 01:20:27 2026  /Users/.../.local/bin/agentrec record --root /Users/.../Projects/agentrec
41925 Wed Aug  5 15:37:24 2026  .../agentrec-phase2-tail/target/debug/agentrec record --root /var/folders/...
41952 Wed Aug  5 15:37:29 2026  .../agentrec-phase2-tail/target/debug/agentrec record --root /var/folders/...
42419 Wed Aug  5 15:38:11 2026  .../agentrec-phase2-tail/target/debug/agentrec record --root /var/folders/...
42421 Wed Aug  5 15:38:11 2026  .../agentrec-phase2-tail/target/debug/agentrec record --root /var/folders/...
count=5

$ launchctl list | grep agentrec
865	0	com.agentrec.bfa6bde6eaa4
count=1
```

**Correction to the task's own framing, measured not assumed.** The task briefed *two* leaked
debug daemons from before this session (42419/42421). There are **four**: 41925 and 41952 are
leaked too. All four carry `lstart` of **Wed Aug 5 15:37–15:38**, i.e. they pre-date this session
by a day, and all four are `target/debug` binaries under `/var/folders/...T/` tempdir roots — the
same shape as the 39 orphans the D46 round reaped. **None of the four was touched.** They are
recorded here because a round that reports "process count restored" against a briefed number of 2
would be reporting against a wrong denominator.

**pgrep predicate footgun, recorded because it produced a false alarm mid-round.** The first
teardown check used `pgrep -f "agentrec (record|mcp)"` and reported **7**, up from 5, after the
leg-1 daemon had been confirmed dead. The two extra hits were `claude -p` processes whose *prompt
text on the command line* contained the string `agentrec record`:

```
 2460 Thu Aug  6 19:16:28 2026  claude -p Output raw markdown text only ... repo /Users/ravichandras...
 3317 Thu Aug  6 19:16:38 2026  claude -p Output raw markdown text only ... repo /Users/ravichandras...
```

Not daemons. Every count in this document uses the binary-path predicate
`pgrep -f "/agentrec record --root|/agentrec mcp"` instead. `pkill -f agentrec` was never run at
any point — it would have killed the founder's live dogfood daemon (PID 865) and all four
pre-existing leaks.

---

## Part 1 — the disposable repo

A real Python project with two real, independently-failing named tests (`test_add`, `test_sub`).
`python3 -B` throughout, so no `__pycache__` is written inside a hook bracket and the recorded turn
file list stays exactly the one source file.

```
$ printf 'def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n' > calc.py
$ printf '...unittest CalcTest with test_add and test_sub...' > test_calc.py
$ git init -q . && git add -A && git -c user.email=f@f -c user.name=f commit -qm "initial: calc + tests"

$ python3 -B -m unittest test_calc -v
test_sub (test_calc.CalcTest.test_sub) ... ok
----------------------------------------------------------------------
Ran 2 tests in 0.000s

OK
```

### `agentrec init` — the D46 temp-root guard, verified live

```
$ agentrec init
scaffolded /REDACTED/fexit/.agentrec
wrote default config.toml
set .agentrec/ to 0700 (files 0600)
Claude Code hooks installed
Claude Code MCP registration written (/REDACTED/fexit/.mcp.json)
skipped Codex hook install (pass --codex to enable)
skipped Codex MCP registration (pass --codex to enable)
skipped service install: root is under a temporary directory (/private/tmp) — a service installed
here outlives the directory; pass --service to install anyway
to reverse everything: agentrec uninstall

$ launchctl list | grep -c agentrec
1
```

Count unchanged (1, the pre-existing dogfood unit). `--service` was never passed in this round.

---

## Blocking pre-check — the first attempt did NOT work, and it reshaped the run

Before running the story, the undo round-trip was smoked in a throwaway directory: does reverting a
turn on a **committed** file actually restore the pre-edit bytes? The first attempt says no, for a
documented reason:

```
# daemon started, then UserPromptSubmit → edit calc.py → Stop
{"type":"turn",...,"grade":"rich","tool":"claude","session":"s_smoke",
 "files":[{"path":"calc.py","before":null,
           "after":"sha256:e1a894022d1a082987b87adecb623438c9e386d86b2b621cff4a5fe7fdf7edc8",
           "op":"modify","baseline_unknown":true}]}
```

`"before": null, "baseline_unknown": true`. An undo of that turn has no bytes to restore. This is
**not a defect** — `agentrec-core/src/record.rs:154` documents the field as exactly this case:

```rust
pub baseline_unknown: bool, // before unrecoverable (first seen post-change)
```

A daemon started moments before the edit had never observed `calc.py`, so it holds no snapshot of
its pre-edit content. Git having the file committed is irrelevant: the CAS is the daemon's, not
git's.

**What this changes about the run, disclosed rather than papered over.** The story is faithful only
if the daemon has already seen the file, which is the steady state of a real continuously-recorded
repo (the machine's own dogfood daemon, PID 865, has been up since Jul 31). This round reproduces
that steady state explicitly with a **warm-up write** — an ordinary human edit to `calc.py`,
outside any hook bracket, whose snapshot becomes the baseline the agent's later edit is diffed
against. The warm-up is visible in every log below as a `bare` turn, and it is done **once per
daemon process**, because a fresh daemon process starts with no baseline again (leg 2 needed its
own — same reason, disclosed there too).

Re-smoked with the warm-up in place, the round-trip works:

```
$ shasum -a 256 calc.py        # baseline, before the agent's edit
af626eb7a9c3d865bc4d7d84f96982d7349f4931c403286d23603d85e6552442  calc.py
$ shasum -a 256 calc.py        # after the agent's edit
7649802ce0c503a5cec07c36fb5dbf0cf1745587581b6f9b325c5cc9ce964abf  calc.py

# the minted rich turn:
"files":[{"path":"calc.py",
          "before":"sha256:af626eb7a9c3d865bc4d7d84f96982d7349f4931c403286d23603d85e6552442",
          "after":"sha256:7649802ce0c503a5cec07c36fb5dbf0cf1745587581b6f9b325c5cc9ce964abf",
          "op":"modify"}]

$ agentrec undo t_01KZC4HN25K0BABNVZCTZHG9DE --confirm
reverted 1 file(s); recorded as turn t_01KZ…2A00
$ shasum -a 256 calc.py
af626eb7a9c3d865bc4d7d84f96982d7349f4931c403286d23603d85e6552442  calc.py
$ python3 -B -m unittest test_calc
OK
```

**Hash tie, observed not asserted:** the turn record's `before`/`after` CAS refs are byte-identical
to the raw `shasum -a 256` digests of the file at those two moments. That identity holds at every
transition in both legs below, and it is what lets the hashes in the turn records be read as file
states rather than opaque handles.

---

## Part 2 — leg 1: confirm mode

`config.toml` was edited to `mcp_destructive = "confirm"` **before** any `agentrec mcp` process was
spawned. `cli/src/mcpcmd.rs` reads config at startup and documents that there is deliberately no
reload path, so a mid-session edit would have silently done nothing.

```
$ grep -n mcp_destructive .agentrec/config.toml
3:mcp_destructive = "confirm"
```

### Daemon up, warm-up baseline

```
$ agentrec record --root "$PWD" > fexit-daemon-leg1.log 2>&1 &
DAEMON PID=1091
{"type":"epoch","v":1,"event":"start","ts":"2026-08-06T18:15:59.130Z"}

# warm-up human write (establishes the daemon's baseline snapshot of calc.py)
$ shasum -a 256 calc.py
ad1102fd6d1bc9de7071c088f25d38cd1d081c3ff7ac1adf2178a5f74259e325  calc.py
```

which the daemon closed as a `bare` turn after the 10s quiet window — carrying
`baseline_unknown: true` itself, as expected for the first sighting:

```
{"type":"turn","v":1,"id":"t_01KZC4M4DDNDNGFZ7Z2D3D6ETP","grade":"bare",...,
 "files":[{"path":"calc.py","before":null,
           "after":"sha256:ad1102fd6d1bc9de7071c088f25d38cd1d081c3ff7ac1adf2178a5f74259e325",
           "op":"modify","baseline_unknown":true}]}
```

### The agent breaks a named test

```
$ echo '{"hook_event_name":"UserPromptSubmit","session_id":"s_fexit_leg1",
         "prompt":"refactor sub() in calc.py to use a helper"}' | agentrec hook claude --root "$PWD"
hook UserPromptSubmit rc=0

# the agent's edit, inside the bracket: sub() operands inverted
$ shasum -a 256 calc.py
750a57cf7acdf03406d8fdeaf95bfe97b483da409c7e14f3a3a704cc81212b51  calc.py

$ python3 -B -m unittest test_calc -v
FAIL: test_sub (test_calc.CalcTest.test_sub)
----------------------------------------------------------------------
Traceback (most recent call last):
  File "/REDACTED/fexit/test_calc.py", line 10, in test_sub
    self.assertEqual(sub(5, 3), 2)
    ~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^
AssertionError: -2 != 2

----------------------------------------------------------------------
Ran 2 tests in 0.000s

FAILED (failures=1)

$ echo '{"hook_event_name":"Stop","session_id":"s_fexit_leg1"}' | agentrec hook claude --root "$PWD"
hook Stop rc=0
```

The minted rich turn, raw from `log.jsonl` — one file, the `before`/`after` pair equal to the two
`shasum` digests above:

```json
{"type":"turn","v":1,"id":"t_01KZC4MV04RQJYT12PP02EM3K5","grade":"rich",
 "started":"2026-08-06T18:16:25.941Z","ended":"2026-08-06T18:16:27.510Z",
 "tool":"claude","session":"s_fexit_leg1","root":"/REDACTED/fexit",
 "prompt_ref":"sha256:b4e7a3e79f8dbe8fd9e945ee41c239ea9f3ef8101f12cffad11ece8878dcce5d",
 "prompt_excerpt":"refactor sub() in calc.py to use a helper",
 "files":[{"path":"calc.py",
           "before":"sha256:ad1102fd6d1bc9de7071c088f25d38cd1d081c3ff7ac1adf2178a5f74259e325",
           "after":"sha256:750a57cf7acdf03406d8fdeaf95bfe97b483da409c7e14f3a3a704cc81212b51",
           "op":"modify"}]}
```

### Daemon DOWN — everything from here on is daemon-stopped

The daemon is stopped before any read or destructive step, for two reasons: the story requires
reads to work without it, and `cli/tests/misattribution.rs` states the operational one outright —
*"Stop watching BEFORE undo runs: undo's own writes would otherwise be observed by the daemon and
mint a turn mid-assertion."* Only the daemon this round started was killed, by captured PID:

```
$ kill 1091
### daemon stopped; my pid 1091 alive? NO
```

### blame / diff find the agent's own turn, with the daemon stopped

```
$ agentrec log
t_01KZ…M3K5 · rich · claude · just now · 1 file · "refactor sub() in calc.py to use a helper"
t_01KZ…6ETP · bare · — · just now · 1 file

$ agentrec blame calc.py
t_01KZ…M3K5 · claude · "refactor sub() in calc.py to use a helper" · 18:16

$ agentrec diff t_01KZC4MV04RQJYT12PP02EM3K5
turn t_01KZ…M3K5 · claude · 1 file
--- a/calc.py
+++ b/calc.py
@@ -3,7 +3,7 @@
 
 
 def sub(a, b):
-    return a - b
+    return b - a
 
 
 def mul(a, b):
```

`blame` names the agent's turn; `diff` shows precisely the breaking edit and nothing else.

### The MCP surface, over real JSON-RPC frames on stdio — also daemon-stopped

Newline-delimited frames piped to `agentrec mcp --root "$PWD"`, protocol revision `2025-11-25`
(the build's `PINNED_REVISION`). Server stderr:

```
agentrec mcp: root /REDACTED/fexit — agent-undo mode confirm
```

`tools/list` — **six** tools, the five read tools plus `agentrec_undo`, which is listed only
because the mode is not `off`:

```
count= 6
 - agentrec_log
 - agentrec_diff
 - agentrec_blame
 - agentrec_recall
 - agentrec_status
 - agentrec_undo
```

`tools/call agentrec_undo {"action":"preview","turn":"t_01KZC4MV04RQJYT12PP02EM3K5"}` → the
`result.content[0].text` payload, verbatim:

```json
{"turn":"t_01KZC4MV04RQJYT12PP02EM3K5","tool":"claude","grade":"rich","mode":"confirm",
 "allow_modified_effective":false,
 "files":[{"path":"calc.py","op":"modify",
           "before":"sha256:ad1102fd6d1bc9de7071c088f25d38cd1d081c3ff7ac1adf2178a5f74259e325",
           "after":"sha256:750a57cf7acdf03406d8fdeaf95bfe97b483da409c7e14f3a3a704cc81212b51",
           "before_bytes":100,"after_bytes":100,"modified_since":false}],
 "refusals":[],
 "warnings":["CAUTION: this turn's file list is an activity window, not an authorship record — agentrec cannot distinguish the recorded tool's own writes from concurrent human edits made in the same window (D6), and every file marked `revert` above is reverted regardless of who wrote it. Review the list before confirming."]}
```

No `token` key — correct for confirm mode, where the reservation is not taken at preview.

`tools/call agentrec_undo {"action":"request","turn":"..."}`:

```json
{"request":"01KZC4PEZFFZ0BWB87ZK4ZVH7G","state":"pending",
 "turn":"t_01KZC4MV04RQJYT12PP02EM3K5","paths":["calc.py"],"allow_modified":false,
 "requested_unix_ms":1786040237039,"expires_unix_ms":1786040837039,
 "preview":{...as above...}}
```

`expires - requested = 600000` ms — the D23 10-minute expiry, observed on the wire.

**Preview and request wrote nothing to the worktree**, checked immediately after both calls:

```
$ shasum -a 256 calc.py
750a57cf7acdf03406d8fdeaf95bfe97b483da409c7e14f3a3a704cc81212b51  calc.py   # still BROKEN
```

### The human grant

```
$ agentrec approve                      # no id → list what is awaiting a decision
01KZC4PEZFFZ0BWB87ZK4ZVH7G  undo of t_01KZ…M3K5  1 file(s): calc.py
approve <id> to execute, deny <id> to refuse

$ agentrec approve 01KZC4PEZFFZ0BWB87ZK4ZVH7G
undo t_01KZ…M3K5 (claude)
  revert  calc.py (modify)
  CAUTION: this turn's file list is an activity window, not an authorship record — agentrec cannot
  distinguish the recorded tool's own writes from concurrent human edits made in the same window
  (D6), and every file marked `revert` above is reverted regardless of who wrote it. Review the
  list before confirming.
approved request 01KZC4PEZFFZ0BWB87ZK4ZVH7G; reverted 1 file(s); recorded as turn t_01KZ…KRNK
```

### Test green

```
$ shasum -a 256 calc.py
ad1102fd6d1bc9de7071c088f25d38cd1d081c3ff7ac1adf2178a5f74259e325  calc.py   # == the baseline

$ python3 -B -m unittest test_calc -v
test_sub (test_calc.CalcTest.test_sub) ... ok
----------------------------------------------------------------------
Ran 2 tests in 0.000s

OK
```

### The undo is itself a turn, and it names its origin

```
$ agentrec log
t_01KZ…KRNK · rich · agentrec · just now · 1 file · "undo of t_01KZ…M3K5"
t_01KZ…M3K5 · rich · claude · 1m ago · 1 file · "refactor sub() in calc.py to use a helper"
t_01KZ…6ETP · bare · — · 1m ago · 1 file
```

Raw `log.jsonl` line:

```json
{"type":"turn","v":1,"id":"t_01KZC4Q5YQ22X3BHXFY8Q3KRNK","grade":"rich",
 "started":"2026-08-06T18:17:40.567Z","ended":"2026-08-06T18:17:40.567Z",
 "tool":"agentrec","root":"/REDACTED/fexit","prompt_excerpt":"undo of t_01KZ…M3K5",
 "origin":"cli",
 "files":[{"path":"calc.py",
           "before":"sha256:750a57cf7acdf03406d8fdeaf95bfe97b483da409c7e14f3a3a704cc81212b51",
           "after":"sha256:ad1102fd6d1bc9de7071c088f25d38cd1d081c3ff7ac1adf2178a5f74259e325",
           "op":"modify"}]}
```

`"origin":"cli"` is present as an explicit key — it is not the absent-means-cli default path, so
this observation discriminates. And it is `cli` for an undo the *agent* requested over MCP, which
is what `record.rs`'s doc comment specifies: *"It names the surface that wrote, never the one that
asked: an `agentrec approve` of an undo an agent requested over MCP is `cli`, because a human at a
keyboard performed it."* Observed behavior matches the normative text.

---

## Part 3 — leg 2: auto mode

`config.toml` set to `mcp_destructive = "auto"` while no `agentrec mcp` process existed. Daemon
restarted (PID 7948, second `start` epoch), and — **disclosed, same reason as leg 1** — a second
warm-up write was needed, because the fresh daemon process again holds no baseline for `calc.py`.

```
$ shasum -a 256 calc.py          # leg-2 baseline
6d04251108f92ed2a82c38ff055ae113837079d216253d5d694b39188c6c95a2  calc.py
```

### The agent breaks the *other* named test

```
$ echo '{"hook_event_name":"UserPromptSubmit","session_id":"s_fexit_leg2",
         "prompt":"make add() variadic"}' | agentrec hook claude --root "$PWD"

# the agent's edit: add() returns a + b + 1
$ shasum -a 256 calc.py
c6150d5601dc0f9612da6fce8178e3c15a5b193fe07024dd79fb031551edc417  calc.py

$ python3 -B -m unittest test_calc -v
    self.assertEqual(add(2, 2), 4)
    ~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^
AssertionError: 5 != 4
----------------------------------------------------------------------
Ran 2 tests in 0.000s

FAILED (failures=1)

$ echo '{"hook_event_name":"Stop","session_id":"s_fexit_leg2"}' | agentrec hook claude --root "$PWD"
$ kill 7948
### daemon 7948 alive? NO
```

```json
{"type":"turn","v":1,"id":"t_01KZC4S10RVSKG1PZN2MKSAXRJ","grade":"rich",
 "tool":"claude","session":"s_fexit_leg2","root":"/REDACTED/fexit",
 "prompt_excerpt":"make add() variadic",
 "files":[{"path":"calc.py",
           "before":"sha256:6d04251108f92ed2a82c38ff055ae113837079d216253d5d694b39188c6c95a2",
           "after":"sha256:c6150d5601dc0f9612da6fce8178e3c15a5b193fe07024dd79fb031551edc417",
           "op":"modify"}]}
```

### `diff` on leg 2's turn, daemon stopped

Run for symmetry with leg 1 (it was initially skipped here; re-run afterwards against the same
still-daemon-stopped repo, which is why it appears out of chronological order — nothing about the
turn or the store changed in between):

```
$ agentrec diff t_01KZC4S10RVSKG1PZN2MKSAXRJ
turn t_01KZ…AXRJ · claude · 1 file
--- a/calc.py
+++ b/calc.py
@@ -1,5 +1,5 @@
 def add(a, b):
-    return a + b
+    return a + b + 1
 
 
 def sub(a, b):
```

**`blame calc.py` was NOT re-run against leg 2's agent turn, and could not honestly be** — by the
time this gap was noticed the undo had already landed, so blame legitimately names the undo turn
now. Run anyway, and recorded as what it is rather than dropped:

```
$ agentrec blame calc.py          # after the leg-2 undo
t_01KZ…K77K · agentrec · "undo of t_01KZ…AXRJ" · 18:19
```

That is correct behavior — the last turn to touch `calc.py` really is the undo — not a blame miss.
`blame` naming the *agent's* turn is evidenced by leg 1 only.

### Both auto-mode rails, exercised on the way through

Server stderr: `agentrec mcp: root /REDACTED/fexit — agent-undo mode auto`.

`{"action":"request",...}` in auto mode:

```json
{"error":"wrong_mode","message":"agent-undo mode is \"auto\": the \"request\" sub-action requires \"confirm\" mode. `agentrec_status` reports this repository's effective mode as `agent_undo_mode`; changing it is a `config.toml` edit plus a server restart, which this agent cannot do."}
```

`{"action":"preview","allow_modified":true,...}` in auto mode — refused, not ignored (PROTOCOL §8):

```json
{"error":"allow_modified_refused","message":"agent-undo mode is \"auto\": `allow_modified: true` is refused, not ignored — files changed since the turn stay excluded. Reverting a modified file is available only through explicit human approval in \"confirm\" mode. Retry without `allow_modified`."}
```

### preview → token → execute

`{"action":"preview","turn":"t_01KZC4S10RVSKG1PZN2MKSAXRJ"}`:

```json
{"turn":"t_01KZC4S10RVSKG1PZN2MKSAXRJ","tool":"claude","grade":"rich","mode":"auto",
 "allow_modified_effective":false,
 "files":[{"path":"calc.py","op":"modify",
           "before":"sha256:6d04251108f92ed2a82c38ff055ae113837079d216253d5d694b39188c6c95a2",
           "after":"sha256:c6150d5601dc0f9612da6fce8178e3c15a5b193fe07024dd79fb031551edc417",
           "before_bytes":128,"after_bytes":132,"modified_since":false}],
 "refusals":[],"warnings":["CAUTION: ...activity window, not an authorship record..."],
 "token":"696c977f4852c1c5b40fbae08d12c90d9f0d7094",
 "token_expires_unix_ms":1786040404036,
 "reservation":"01KZC4SQF4TB5E5Z09P10J2XWA"}
```

The token is issued only in auto mode, and only here.

**`execute` was run from a second, separate `agentrec mcp` process** — the preview process had
already exited. This makes the cross-process durability of the reservation an observed property
rather than an assumed one:

```
now_ms=1786040363043  token_expires=1786040404036      # 40.99s of the 60s TTL remaining
```

```json
{"files":["calc.py"],"reservation":"01KZC4SQF4TB5E5Z09P10J2XWA","reverted":1,
 "turn":"t_01KZC4S10RVSKG1PZN2MKSAXRJ","undo_turn":"t_01KZC4TA1PBP0CV6Z34CQ1K77K"}
```

### Test green

```
$ shasum -a 256 calc.py
6d04251108f92ed2a82c38ff055ae113837079d216253d5d694b39188c6c95a2  calc.py   # == leg-2 baseline
$ python3 -B -m unittest test_calc -v
----------------------------------------------------------------------
Ran 2 tests in 0.000s

OK
```

### The undo turn, with the other origin

```json
{"type":"turn","v":1,"id":"t_01KZC4TA1PBP0CV6Z34CQ1K77K","grade":"rich",
 "started":"2026-08-06T18:19:23.062Z","ended":"2026-08-06T18:19:23.062Z",
 "tool":"agentrec","root":"/REDACTED/fexit","prompt_excerpt":"undo of t_01KZ…AXRJ",
 "origin":"mcp",
 "files":[{"path":"calc.py",
           "before":"sha256:c6150d5601dc0f9612da6fce8178e3c15a5b193fe07024dd79fb031551edc417",
           "after":"sha256:6d04251108f92ed2a82c38ff055ae113837079d216253d5d694b39188c6c95a2",
           "op":"modify"}]}
```

Both discriminator values, from one repo's `log.jsonl`:

```
$ grep -o '"origin":"[^"]*"' .agentrec/log.jsonl
"origin":"cli"
"origin":"mcp"
```

Full history at the end of both legs — six turns, the two undos among them, all readable with no
daemon running:

```
$ agentrec log
t_01KZ…K77K · rich · agentrec · just now · 1 file · "undo of t_01KZ…AXRJ"
t_01KZ…AXRJ · rich · claude · just now · 1 file · "make add() variadic"
t_01KZ…7TYF · bare · — · 1m ago · 1 file
t_01KZ…KRNK · rich · agentrec · 1m ago · 1 file · "undo of t_01KZ…M3K5"
t_01KZ…M3K5 · rich · claude · 3m ago · 1 file · "refactor sub() in calc.py to use a helper"
t_01KZ…6ETP · bare · — · 3m ago · 1 file
```

### Token replay probe (extra, not required by the story)

Re-sending the identical spent token:

```json
{"error":"token_consumed","message":"that undo token has already been spent — nothing was written; a token is single-use, so call `preview` again for a fresh one"}
```

```
$ shasum -a 256 calc.py
6d04251108f92ed2a82c38ff055ae113837079d216253d5d694b39188c6c95a2  calc.py   # unchanged
```

---

## Part 4 — the request ledger, as it actually ended up on disk

```
$ ls -l .agentrec/undo-requests.jsonl
-rw-------  1 ravichandrasekhar  wheel  1230 Aug  6 19:19 .agentrec/undo-requests.jsonl
```

Mode `0600`. All five events, verbatim — leg 1's `request`/`execute`, leg 2's
`reserve`/`consume`/`execute` (the F4 spend-before-writes ordering, visible as `consume` at
`…363051` preceding `execute` at `…363067`, 16 ms apart):

```json
{"v":1,"event":"request","id":"01KZC4PEZFFZ0BWB87ZK4ZVH7G","turn":"t_01KZC4MV04RQJYT12PP02EM3K5","paths":["calc.py"],"expires_unix_ms":1786040837039,"at_unix_ms":1786040237039,"hashes":["sha256:750a57cf7acdf03406d8fdeaf95bfe97b483da409c7e14f3a3a704cc81212b51"]}
{"v":1,"event":"execute","id":"01KZC4PEZFFZ0BWB87ZK4ZVH7G","turn":"t_01KZC4MV04RQJYT12PP02EM3K5","paths":["calc.py"],"expires_unix_ms":1786040837039,"at_unix_ms":1786040260572,"undo_turn":"t_01KZC4Q5YQ22X3BHXFY8Q3KRNK"}
{"v":1,"event":"reserve","id":"01KZC4SQF4TB5E5Z09P10J2XWA","turn":"t_01KZC4S10RVSKG1PZN2MKSAXRJ","paths":["calc.py"],"token_sha256":"sha256:6591972d571bcbe49911450a1be5b2de782e13791b3e9a8029bef58834796950","expires_unix_ms":1786040404036,"at_unix_ms":1786040344036,"hashes":["sha256:c6150d5601dc0f9612da6fce8178e3c15a5b193fe07024dd79fb031551edc417"]}
{"v":1,"event":"consume","id":"01KZC4SQF4TB5E5Z09P10J2XWA","turn":"t_01KZC4S10RVSKG1PZN2MKSAXRJ","paths":["calc.py"],"expires_unix_ms":1786040404036,"at_unix_ms":1786040363051}
{"v":1,"event":"execute","id":"01KZC4SQF4TB5E5Z09P10J2XWA","turn":"t_01KZC4S10RVSKG1PZN2MKSAXRJ","paths":["calc.py"],"expires_unix_ms":1786040404036,"at_unix_ms":1786040363067,"undo_turn":"t_01KZC4TA1PBP0CV6Z34CQ1K77K"}
```

The raw token appears nowhere in the ledger; only its sha256 does:

```
$ grep -c '696c977f4852c1c5b40fbae08d12c90d9f0d7094' .agentrec/undo-requests.jsonl
0
```

---

## Part 5 — teardown and hygiene, verified not asserted

```
$ ps -p 1091 ; ps -p 7948          # the two daemons this round started
1091 DEAD
7948 DEAD

$ pgrep -f "/agentrec record --root|/agentrec mcp"
  865 Fri Jul 31 01:20:27 2026  /Users/.../.local/bin/agentrec record --root /Users/.../Projects/agentrec
41925 Wed Aug  5 15:37:24 2026  .../target/debug/agentrec record --root /var/folders/...
41952 Wed Aug  5 15:37:29 2026  .../target/debug/agentrec record --root /var/folders/...
42419 Wed Aug  5 15:38:11 2026  .../target/debug/agentrec record --root /var/folders/...
42421 Wed Aug  5 15:38:11 2026  .../target/debug/agentrec record --root /var/folders/...
count=5

$ launchctl list | grep agentrec
865	0	com.agentrec.bfa6bde6eaa4
count=1
```

Process count **5 → 5**, same five PIDs with the same start times — every `agentrec mcp` process
this round spawned exits on stdin EOF, and both daemons were killed by captured PID. LaunchAgent
count **1 → 1 → 1** (before / after `init` / after teardown); no plist was ever written for the
disposable repo. The four pre-existing leaked debug daemons and the live dogfood unit are exactly
as they were found.

---

## What this round did and did not establish

**Established live, end to end, on the release binary:** the destructive loop runs — agent turn is
recorded from a real hook bracket, `blame`/`diff`/`log` find it with the daemon stopped, MCP
`tools/list` gates `agentrec_undo` on the configured mode and shows six tools, `preview` writes
nothing, both grant paths work (confirm: `request` → human `agentrec approve`; auto: token →
`execute` from a different process), the named failing test goes green in both legs with the file
restored to a byte-identical baseline digest, each undo appends its own turn, and the two turns
carry `origin: "cli"` and `origin: "mcp"` respectively. Both auto rails (`request` refused,
`allow_modified: true` refused) fired with their documented messages. Token replay is refused.

**Not established, stated rather than implied:**

- **The agent is simulated at the identity level.** The writes were made by this session's shell
  inside a real hook bracket, not by a `claude -p` process; the hook-driven option is the sanctioned
  one for this row, but nothing here measures a real Claude Code session's hook firing.
- **The warm-up write is a setup step this round introduced**, not something the story text asked
  for. It is load-bearing: without a daemon that has already seen the file, `before` is `null`,
  `baseline_unknown` is `true`, and there is nothing to revert. That the steady state of a real
  long-running daemon supplies this automatically is an argument, not a measurement — this round
  did **not** run the story against a daemon that had been up for days.
- **One file, one path, per turn.** No multi-file turn, no `skipped`/`withheld`/`modified_since`
  refusal class, no path-subset reservation, and no `undo_conflict` was exercised live here; the
  `refusals` array was empty in every preview. Those are covered by fixtures under F2's ACs, not by
  this row.
- **macOS only.** No Linux leg.
- **No concurrency.** Both legs are strictly sequential; nothing here probes two agents, or an
  agent and a human, racing on the same reservation.
- **`deny` was never run.** Only the approve path of the confirm flow was exercised.
- **Expiry paths were never reached.** The 10-minute request expiry and the 60-second token TTL
  were both observed as *fields on the wire* and both were beaten by ~41s and ~9.5min respectively;
  neither expired-and-refused path was executed.
- **Only the TRANSPORT rail was probed for `allow_modified`.** The `allow_modified_refused` error
  above comes from `mcpcmd`'s router, which intercepts before the coordinator is reached.
  `VERIFY-LEDGER.md`'s Phase F gate-round-1 finding 1 records that
  `UndoCoordinator::preview` in Auto **downgrades-with-warning rather than refusing**. This round
  did not exercise the coordinator directly, so nothing here is evidence of refusal at depth; a
  non-reference transport built straight on the coordinator would ignore-with-warn. "Both rails
  fired" in this document names two distinct *rails* (the mode matrix and the `allow_modified`
  check), never two *layers*.
- **`blame` on the agent's own turn is evidenced by leg 1 only** — see the note in Part 3.
