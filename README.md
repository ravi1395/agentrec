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

## Threat model

**agentrec is built against accidents, not against an adversary.** The cases it is designed for
are an honest agent doing the wrong thing, a human overwriting a file without noticing, a process
killed mid-write, and a snapshot rotting on disk. Those it handles: turns and file snapshots are
written by the daemon as it observes the filesystem, not reconstructed afterward from anyone's
memory; the object store verifies a blob against its own address when it reads it and reports a
mismatch as an error rather than serving corrupt bytes (`store.rs`, `BlobStore::get`); and
a recording gap is surfaced rather than guessed across — a file whose last turn is followed by an
uncovered interval blames as `· attribution stale — recording gap`
(`view.rs`, `gap_stale`), and a path no turn touches at all, in a history with a
crash gap, names the gap instead of naming a turn (`view.rs`, `BlameState::NoTurnRecordingGap`).

**It does not defend against a malicious or prompt-injected agent, or against a repo whose build
scripts and hooks you have not read.** Those run under your uid, and so does agentrec. Anything
with your uid can write `.agentrec/` directly — append to `signal.jsonl`, rewrite `log.jsonl`,
replace objects, reset `state.json`. The signal inbox carries no provenance: `tool` is a free-form
string (`record.rs`, `SignalEvent::tool`) and a line is accepted by a bare deserialize with no
check of who wrote it (`record.rs`, `parse_signals`), so an appended line mints a turn that `log` and
`blame` then render exactly like one the daemon observed. `log.jsonl` carries no chain or MAC over
its lines; append-only is a discipline the writer keeps, not a property the format enforces. And
tampering with the record is the one class of filesystem change the recorder structurally cannot
witness, because `.agentrec` is in the watcher's denylist (`daemon.rs`, `classify`) — with no
second copy to compare against, since `init` adds `.agentrec/` to `.gitignore`
(`initcmd.rs`, `ensure_gitignore`). A record deleted outright prints `no turns recorded — is agentrec
record running?` (printed by `cmds.rs`; the indistinguishability comes from `record.rs`,
`load_log`'s open-failure fallback returning an empty ledger), which is what a repo where
nothing has happened yet also prints.

**"Independent," above, means independent of the agent's self-report — not tamper-resistant
against the agent.** Attribution here comes from watching the filesystem rather than from trusting
a transcript, which is what makes it useful when a tool's own logs are missing, disabled or wrong.
It is not evidence against the actor it records. The torture harness above covers the accident
half — that `undo` does not corrupt your tree when turns, human edits and crashes interleave — and
bears on the adversarial half not at all. Signing or hash-chaining the log would not change that:
the key would sit on the same machine, under the same uid, as the process being recorded.

## Known limitations

**A human edit made while an agent's turn is open is attributed to the agent.** The recorder
keeps one open turn per root (decision D6 in `IMPLEMENTATION.md`), so any file mutation landing
between the agent's start signal and its stop signal is folded into that turn — including your
own save from another editor window. The turn is rich, so `blame` will name the agent's turn,
tool and prompt for those lines. The documented remedy is separation by root: put concurrent
work in a separate worktree.

**`undo` of such a turn discards those human edits with no modified-since warning.** The edit
happened inside the turn, so it is part of what the turn recorded as its `after` state. The
modified-since rail (D30) compares the file's current hash against that `after` hash — it is
built to catch edits made *after* the turn closed, and there is nothing for it to detect here.
`undo` therefore restores the file to its pre-turn `before` content, dropping the human edit,
and the per-file output shows an ordinary clean revert — that rail has nothing to fire on.

What `undo` does flag is the turn as a whole. Previewing a rich turn it did not itself record —
or a bare turn, which gets its own wording because it has no recorded tool to name at all —
prints — whenever the plan actually reverts anything — a CAUTION that the turn's file list is an activity window rather than an authorship
record, and that every file the plan reverts is reverted regardless of who wrote it. That is a
blanket disclosure, not per-file detection: agentrec still cannot tell your edits from the
agent's inside the window, so the CAUTION is deliberately always-true rather than a heuristic
that fires only when it thinks it found one. The remedy is still separation by root.

## Commands

| Command | Does |
|---|---|
| `agentrec init` | Scaffold `.agentrec/`, install agent hooks, install+load a per-repo recorder service. Idempotent. `--no-hook`, `--no-service`, `--dry-run`. **Under a temporary directory** (`$TMPDIR`, `/tmp`, `/private/tmp`) the service unit is skipped by default and the reason is printed: the unit is user-scoped with `RunAtLoad`+`KeepAlive`, so it outlives the directory it records and nothing reaps it when that directory is deleted. `--service` installs it anyway; `--service` together with `--no-service` is rejected |
| `agentrec record` | Run the recorder daemon in the foreground for this repo |
| `agentrec log` | List recorded turns, newest first (git turns hidden by default). `--all`, `--json`, `--limit`, `--utc`, `--explain`, `--all-files` (show file entries `noise_globs` would otherwise fold into a count line) |
| `agentrec diff <turn>` | Unified diff of a turn's file changes (turn id or unambiguous prefix). `--json` emits `serde_json` of the exact `DiffResult` the read layer produced — `{"turn_id","tool","total_files","files":{"items","next"}}` — instead of the unified-diff text; `tool` is omitted entirely (never `"tool":null`) on a bare/toolless turn |
| `agentrec blame <file>[:line]` | Which turn last touched a file, or introduced a current line. `--json` emits `serde_json` of the exact `BlameResult` the read layer produced — `{"path","line","state"}`, `state` internally tagged by `type` — instead of the prose line; an uncovered path's `state.type` names the recording-gap arm (e.g. `no_turn_recording_gap`) and carries no `turn`/attributor field at all, never a guess |
| `agentrec show <turn> [--prompt]` | Print a turn's header (excerpt discipline: no full prompt without the flag); `--prompt` prints the full post-scrub prompt text. `--all-files` (see `agentrec log`) |
| `agentrec undo [turn]` | Revert a turn's changes, per file. Preview-only unless `--confirm`. `--allow-modified`, `--files a,b,c`. Omit `turn` for panic mode: targets the most recent non-git rich turn |
| `agentrec status` | Store size, recording gaps (every kind — crash, restart, and since-last-stop — with a per-kind breakdown whenever the total is non-zero), a `daemon:` liveness line (plus a ⚠ remedy row when the recorder is not running), rich-rate, an `inbox:` line sizing `signal.jsonl` and how much of it the daemon has already consumed (reclaimable with `purge --signals-consumed`; `status` itself reclaims nothing), DEGRADED banner on snapshot or prompt-blob write failures, and — only once one has happened this daemon epoch — how many times the ignore rules were reloaded (the lifetime total is preserved in `state.json`/`--json` but the text line resets on each daemon restart). The `store:` line shows both bytes on disk and bytes counted toward the budget (only turn-referenced snapshot blobs — what the evictor manages; the budget defaults to 2 GiB and is settable via `config.toml`'s `store_budget_bytes`). Over budget — measured against the counted bytes, not raw disk — `status` prints a **read-only dry-run** — "would free N B" plus, when anything is protected, "M B protected (pinned or in-flight — never evicted)" — it deletes nothing itself; a running `agentrec record` daemon is the sole evictor, sweeping the budget on its own recurring tick (default every 10 minutes, plus once at startup) and printing one stderr line per pass that actually evicts something. If no daemon is running, the over-budget line adds `daemon not running — nothing is evicting` instead, so a stopped daemon's store can only grow (via `undo`) until the next `agentrec record` run sweeps it. `--ack-degraded` clears the banner; `--json` emits machine-readable operational fields instead of the text report (operational state, not the `log.jsonl` protocol) — this now carries every DEGRADED field the text banner reports (`snapshot_failures`, `io_failed`, `prompt_put_failures`, `state_parse_failures`), not just the ignore-reload counters, plus two JSON-only perf-evidence counters with no text-banner equivalent (they are not a DEGRADED condition): `dedup_hits` (clean dedup-hit verification reads on the daemon's snapshot path — every `stage` put, including symlink targets; only the prompt puts are excluded) and `dedup_reread_bytes` (total bytes re-read across them). Unlike the ignore-reload counters, these two are **epoch-scoped mirrors of the current daemon run, not lifetime totals** — they are overwritten (not accumulated) on each drain, so after a restart the previous epoch's figures remain visible until the new epoch's own first dedup hit. The payload is also flattened with the exact `RepositoryHealth` value the read layer's `RepositoryView::health` returned — `store_bytes`, `budgeted_bytes`, `budget`, `over_budget` (keyed on budgeted, not disk, bytes), `turn_count`, `crash_gaps`, `restart_gaps`, `trailing_stop_gaps`, `unknown_type_lines`, `unparsed_lines` (the last two are the tolerant-parse census: well-formed-but-unrecognized `type` lines — including known types at an unimplemented schema major — and lines that are not parseable JSON at all, e.g. a torn tail line after a crash) — additive alongside every field above, not a replacement of them, as are the always-present `signal_bytes`/`signal_consumed_bytes` inbox fields (D48). `--ack-degraded --json` together is rejected by clap (the ack path is prose-on-success; never combine it with `--json`) |
| `agentrec doctor` | One-shot diagnosis of the whole recording chain: daemon liveness, hooks, signal freshness, store health, permissions, (Linux) inotify headroom, and installed service units whose recorded `--root` is no longer present. `--json`. The orphaned-services check is **advisory** — it renders `pass` and never changes the exit code, because the unit directory is user-global and one stale unit from an unrelated scratch repo must not break this repo's all-pass gate. It prints the exact `launchctl bootout`/`systemctl --user disable` + `rm` pair to run; agentrec never removes units for you. A root that is merely unmounted reads the same as a deleted one, so the note reports what was observed rather than asserting the unit is abandoned |
| `agentrec purge` | Delete blob objects: expired prompts by default (TTL from `config.toml`). `--all-prompts`, `--snapshots-before <DATE>`, `--orphans` (archives CAS blobs no turn or prompt references — superseded intermediate snapshots the daemon wrote for crash recovery, never deletes), `--memories-retracted` (archives expired retracted memory chains, never deletes), `--log-duplicates` (archives+repairs a `log.jsonl` carrying a pre-fix duplicate turn record, never deletes), `--signals-consumed` (archives the already-consumed prefix of `signal.jsonl` — not the whole file; prefix archive + live tail together reconstruct it — then truncates that prefix and rebases the offset, never deletes), `--path <PATTERN>` (forgets the recorded content of matching files: archives every snapshot blob referenced only by those paths, keeps and reports blobs whose identical bytes are also referenced elsewhere, and leaves `log.jsonl` unchanged — so `diff` reports the snapshot unavailable and `undo` refuses those entries; refuses combination with every other purge flag, and does **not** run the default prompt purge). `--orphans`, `--memories-retracted`, `--log-duplicates`, `--signals-consumed` and `--path` all archive first and all refuse to run while the recorder daemon is live |
| `agentrec uninstall` | Remove hooks + service unit, archive `.agentrec/` to a sibling directory. Nothing is ever deleted. `--no-service` |
| `agentrec remember <fact> --from <paths>` | Record a manual, human-authored pinned memory. `--from` is a comma-separated list of repo-relative paths |
| `agentrec recall <query>` | Rank-then-verify search over pinned memories — only returns Fresh matches. `-k <n>`, `--json`. Verification is capped at 128 candidates per call; if the cap is hit, a notice prints to stderr ("results may be incomplete") since fresh matches could exist beyond it |
| `agentrec memories` | List recorded memories (audit view, not a query). `--stale` (non-fresh only), `--all` (include retracted), `--json`. `--stats` summarizes per-hook recall latency instead — rejected together with `--stale`/`--all`/`--json` (they shape the listing branch `--stats` bypasses entirely; same clap-conflict precedent as `status --ack-degraded --json`), and requires `.agentrec/` to exist (an uninitialized repo gets the same refusal + exit 1 as plain `memories`, never a measured-looking "no hook invocations recorded"). Reads `memory-stats.jsonl` directly (never through the `memory.jsonl` load path, so a corrupt memory store never blocks this readout) and reports count/p50/p90/p99/max over lines carrying `elapsed_ms`, a per-outcome breakdown that is mutually exclusive (`injected`/`budget_exceeded`/`failure`/`capped_empty` — the last being a capped verify walk that surfaced zero fresh hits), plus a separate cross-cutting `capped_total`: every measurable line where the verify walk hit `RECALL_VERIFY_CAP`, whether or not it also injected — a capped-but-successful injection lands in `injected`, not `capped_empty`, so `capped_total` is the only figure that answers "how many recalls hit the cap at all". Also reports honest counts of pre-upgrade lines (parseable, no `elapsed_ms` — written before this field existed) and unparseable lines skipped. An empty-or-absent stats file (in an initialized repo) prints `no hook invocations recorded` |
| `agentrec candidate <fact> --from <paths>` | Emit an agent-authored memory candidate for the daemon to validate, hash, and ingest. `--tool <name>` |
| `agentrec verify <id>` | Preview a memory's per-pin drift against the working tree; touches nothing without `--confirm`. `--confirm` re-pins, `--drop-pin <path>` (repeatable) explicitly drops an orphaned pin, `--replace-pin <old>=<new>` (repeatable) re-points an existing pin to an explicit successor path |
| `agentrec forget <id>` | Retract a memory — recoverable (quarantined, not deleted) until `purge --memories-retracted` archives it past TTL. `--reason <text>` |

Run `agentrec <command> --help` for the full flag reference.

`log.jsonl` is append-only, so a class of file entries that never should have been recorded (a
tool-generated cache directory, say) can never be *removed* from it — but `log`/`show` can stop
*rendering* them individually. `config.toml`'s `noise_globs` is an optional array of
gitignore-style glob patterns (matched against the same repo-relative paths `FileEntry.path`
carries — a trailing `/**` or a bare directory name both fold everything beneath it):

```toml
noise_globs = [".remember/**", ".code-review-graph/**"]
```

When set, `log` and `show` fold matching file entries out of the per-turn count and print
`+{n} noise files (--all-files to show)` so it's always visible that something was hidden — never
a silent count drop. `--all-files` reveals them again for that invocation (same idea as `--all`
already does for whole git/superseded turns, one level down to individual file entries; the two
flags are independent). This is **display-only**: absent or empty `noise_globs` leaves output
byte-identical to before the feature existed, `--json` is never affected by it, and `diff` /
`blame` / `undo` never consult it — a file being visually noisy has nothing to do with whether
it's revertible or who wrote it. Only a single-line TOML array is recognized (the config parser
is a hand-rolled scalar-per-line scanner, not a full TOML parser); a multi-line array is not
picked up and silently behaves as if `noise_globs` were unset.

`log.jsonl` and `signal.jsonl` are append-only by design (history is corrected by appending, never rewritten), with one narrow, manual exception each. `purge --log-duplicates` repairs a `log.jsonl` that a pre-fix daemon (a since-fixed kill-9 crash window) wrote a same-id duplicate turn record into. It archives the whole file, unmodified, to `.agentrec/log.archived.<ts>.jsonl` before touching anything, then rewrites `log.jsonl` dropping only lines that are exact duplicates of an earlier same-id record — a same-id pair that genuinely touched different files is left untouched rather than guessed at. It refuses outright while the daemon is recording, and a run that finds nothing to fix touches no files and creates no archive. `purge --signals-consumed` is the `signal.jsonl` counterpart: it archives the consumed prefix first (only the bytes it is about to drop — the live tail stays in place, and prefix + tail together reconstruct the file), then drops only whole lines the daemon has already consumed (strictly before `state.json`'s byte offset — their prompts are already in `log.jsonl` and the object store), rebasing that offset in the same operation, leaving the unconsumed tail byte-for-byte intact, and refusing — like the above — while the daemon is recording.

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
