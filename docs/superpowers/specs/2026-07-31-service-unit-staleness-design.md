# Stale-path service units — design brainstorm

**Status:** brainstorm + recommendation. NOT built, NOT approved. Three founder decisions at the
tail. Produced 2026-07-31 by a fable subagent with fresh context, at the founder's direction,
after the orphan root-cause round (see CLAUDE.md).

## Problem

`agentrec init` installs a per-repo user-scoped unit (launchd plist / systemd user unit) with
`RunAtLoad` + `KeepAlive`. The unit bakes a **snapshot of two paths**, neither re-validated at
load time: the repo root (`--root <path>`) and the exec (the agentrec binary). When either goes
stale, KeepAlive converts one clean failure into a permanent respawn loop (launchd status 78 =
exec failure).

Measured population (40 archived plists, re-parsed independently by a skeptic agent):

| | count |
|---|---|
| orphans | 39 (40th is the live dogfood daemon) |
| orphan roots under a temp prefix | 39 / 39 |
| orphan execs in a throwaway build tree | 39 (38 `target/debug` + 1 `target/release`) |

Both baked paths were typically stale at once. D46's temp-root guard (as of `58c517f`) prevents
this whole population at the source. The **remaining gap** is a root **or** exec vanishing on a
**non-temp** path — delete/move/rename an ordinary repo, or upgrade the binary — which leaks an
identical permanently-respawning unit that the temp guard never sees. Zero measured instances,
but it is the case that reaches real users rather than agents in scratch dirs.

## The observation that drives the recommendation

`daemon.rs::run` already canonicalizes the root at startup and exits nonzero when it is gone. So:

- **vanished root** → clean process exit, our code runs.
- **vanished exec** → failure at *spawn*, status 78, **before any agentrec code runs at all**.

The measured population failed at exec. **Any fix implemented purely inside the daemon binary
would have prevented none of the measured leaks.** That kills the intuitive "make the daemon
self-reap" answer on evidence rather than taste.

## Design space

### A. Declarative liveness conditions in the unit (fix the KeepAlive posture) — RECOMMENDED

Tell the init system the preconditions instead of `KeepAlive true` / `Restart=always`.

- macOS: emit `KeepAlive` as a **dict** with `PathState` naming both the root and the exec.
  Crash-restart survives while both exist; a stale unit degrades to **one failed `RunAtLoad`
  attempt per login, no respawn loop**.
- Linux: `ConditionPathIsDirectory=<root>` + `ConditionPathExists=<exec>`. A false condition
  **skips** the start (inactive, not failed), silently. systemd was already less bad — the
  default start-limit parks the unit in `failed` — but conditions make it intentional.

Prevents the loop for **both** stale paths, including the exec shape no in-process code can ever
see. Deletes nothing: the leak becomes an inert dormant file, still visible to `doctor`.

Unmounted volume / detached network mount: conditions go false → not respawned; on remount it
comes back. Strictly better than today (today it respawn-loops against a missing mount), and it
never misclassifies-and-deletes, because it deletes nothing.

Migration is free: `service::install` already rewrites-and-reloads on content difference, so the
next `init` in any live repo upgrades its unit.

### B. Self-validating daemon (`SuccessfulExit: false` / `Restart=on-failure`)

Root half only. A vanished exec never runs the daemon, so the status-78 loop — the dominant
measured shape — persists. Would have fixed roughly none of the population. Also inverts the
crash-restart promise (a real bug that exits 0 goes dormant). Subsumed by A.

### C. Detection only — louder `doctor`, self-healing `init`

`scan_units` currently classifies on **root only**; a live root with a vanished exec (the brew
shape) is not a reported state. Extending it is cheap and fully testable. But detection without a
consumer demonstrably does not drain the pool — the 40 accumulated while the check existed.
Adopt the exec-aware scan as a **component**, not as the whole answer.

### D. Gated `service prune`

Engaging the recorded deferral rather than talking past it:

1. *"Only piece that shells destructively to launchctl"* — **factually stale**:
   `service::uninstall` → `unload()` already shells `launchctl unload` / `systemctl --user
   disable --now`, ships, and works.
2. *"The user reaps by hand"* — the founder just did, and the manual procedure is exactly the
   algorithm prune would encode. The mitigation was executed once and was expensive.

What the new evidence does **not** change: the population was 100% temp-rooted and D46 now stops
it at the source; the non-temp leak has **zero measured instances**, and A makes future ones
inert. The surviving objection is not the shell-out — it is that prune acts on a **user-global**
directory across repos, on a classification the code itself documents as *"an observation, not a
verdict"* (an unmounted volume is indistinguishable from a deleted root). Only the human can
resolve that.

If ever built: plan/execute split (retention precedent), `--dry-run` default, `--confirm` to
execute, only `VanishedRoot`, re-check at execution time, refuse the current repo's unit, and
**archive** the unit file — never `rm`.

### E. Drop the KeepAlive service entirely (hook-launched, WatchPaths, StartInterval)

Rejected: fixes the pathological case by degrading the healthy one. "Survives reboots, restarts
on crash" is the promise the unit exists to deliver.

### F. Self-reaping unit (daemon deletes its own unit)

Rejected twice: cannot fire in the exec-vanished case (daemon never runs), and it would destroy
its own unit on a detached network mount — an irreversible autonomous wrong action, against both
the never-delete rule and destructive-founder-reserved. A achieves the safe subset with zero code
running.

## Recommendation — A + C's exec-aware scan; prune stays deferred

- **Behavior:** A is the only option neutralizing both stale paths, including the 39/40 exec
  shape. Converts "permanent respawn loop" into "at most one failed attempt per login (macOS) /
  silent skip (Linux)" — the defect statement, inverted.
- **Complexity:** two pure string-generator edits, one parser addition, one doctor clause. No new
  verbs, no new destructive surface, no daemon changes.
- **Risk:** deletes nothing, so misclassification cannot destroy anything. Worst case is launchd's
  `PathState` timing being lazier than documented → degrades to today's throttled retries, i.e.
  never worse than current behavior.

### Smallest edit (`file:symbol`)

- `cli/src/service.rs:launchd_plist` — `KeepAlive` as `{PathState: {<root>: true, <exec>: true}}`,
  escaped through the existing `xml_escape`.
- `cli/src/service.rs:systemd_unit` — add `ConditionPathIsDirectory=` (root) +
  `ConditionPathExists=` (exec). **Note:** `Condition*=` values are not command-line tokens; `%`
  specifier escaping applies but quoting differs from `ExecStart` — reuse only the `%%` half of
  `systemd_escape`, or add a tiny dedicated escaper.
- `cli/src/service.rs:parse_unit_root` — sibling `parse_unit_exec` (first `ProgramArguments`
  string / first `ExecStart` token; both tokenizers exist).
- `cli/src/service.rs:InstalledUnit` — add `missing_exec: Option<PathBuf>`. **Do NOT fold into
  `UnitState`:** root-state and exec-state are orthogonal, and `Unparseable`'s
  no-fabricated-findings rule must survive — an unparseable exec says nothing.
- `cli/src/doctorcmd.rs:check_orphan_services` — second advisory clause for live root + missing
  exec, phrased "will fail at next login", same never-a-Fail posture.
- No `initcmd.rs` change — `install`'s content-diff rewrite migrates units on re-init.

### Acceptance criteria (each → one runnable test)

1. `launchd_plist` emits a `KeepAlive` dict with `PathState` for both paths, and no bare
   `KeepAlive`/`<true/>`.
2. `systemd_unit` emits `ConditionPathIsDirectory=` (root) and `ConditionPathExists=` (exec).
3. `parse_unit_root` round-trips unchanged over the new plist form for every `TRICKY_ROOTS` entry
   — the root now appears **twice** in the document and the parser must still return it once.
4. `parse_unit_exec` round-trips tricky exec paths over both writers.
5. `scan_units` classifies (live root, present exec) as `missing_exec: None` and (live root,
   absent exec) as `missing_exec: Some(..)`, disjoint from `VanishedRoot` and `Unparseable`.
6. `doctor` against `AGENTREC_TEST_SERVICE_DIR` holding a live-root/vanished-exec unit renders an
   advisory `pass` naming the exec and remedy, and does not flip `report.ok`.
7. Migration: generator output for (exec, root) differs from a checked-in old-form fixture.
   **Caveat, stated rather than papered over:** this cannot be asserted through `service::install`
   itself, because `install` calls `load()` which shells out — so it is a fixture-diff test on the
   pure generators, not an end-to-end migration test.

### VERIFY-LEDGER rows — explicitly NOT automatable

The suite cannot execute `install`/`load` (they shell to the real `launchctl`/`systemctl`; a test
that does so leaks a live unit on the developer's machine). So the platform's *honoring* of the
conditions is ledger-verified once, matching the existing D46 posture:

1. macOS: load a new-posture unit for a scratch repo, delete the repo, observe no respawn
   accumulation in `launchctl list` and at most one failed attempt after re-login.
2. Same with the exec removed.
3. Linux: `Condition` skip visible in `systemctl --user status`.

This is the design's honest ceiling — the generator is fully testable, the platform semantics are
not.

### Deliberately NOT fixed

- Stale unit **files** still accumulate. No platform "remove thyself" primitive exists, and
  autonomous removal is ruled out by the unmounted-volume ambiguity plus the never-delete rule.
  Detection + dormancy is the ceiling without prune. Units already leaked on other machines stay
  until a re-init or manual reap.
- Linux symlinked-exec staleness (`/proc/self/exe`, kernel-resolved) remains the bounded residual
  `service_exec_path` documents. A makes it **quiet**, not **correct**.
- Mid-run root deletion: a running daemon keeps running until it exits on its own; only the
  *restart* is gated.

## Founder decisions required

1. **Reopen `service prune` or keep it deferred.** The brainstorm's read: the deferral's stated
   rationale is partly stale (uninstall already shells destructively; the manual reap was
   expensive and followed prune's exact algorithm), but the new evidence **weakens** demand for it
   (population 100% temp-rooted and already prevented; non-temp leak has zero measured instances;
   A makes future ones inert). Recommendation: **keep deferred**; if built, only in the
   plan/execute + archive + per-unit-confirm shape above. Founder call by rule — destructive
   surface is founder-reserved.
2. **The posture migration touches the live dogfood unit.** The next `agentrec init` in
   `~/Projects/agentrec` rewrites and unload/loads the one healthy daemon. Harmless in principle;
   flagged because that unit is production evidence infrastructure.
3. **Accepting the three VERIFY-LEDGER rows as the closure mechanism** for the platform-semantics
   claims — no automated test can exist for them.
