# agentrec — Implementation plan (v1 → end state)

*Companions: PROBLEM.md (why), SPEC.md (v1 what), PROTOCOL.md (formats), ROADMAP.md (when/gates), INTEGRATIONS.md (v2 designs). This document is the how: context, decisions, tasks, and acceptance criteria per release. It contains no open questions — every ambiguity encountered while writing it was either resolved in the Decision register (§2) or escalated to the founder (§8).*

## 1. Context

agentrec is a local-first, tool-agnostic recorder for coding-agent turns: a daemon that segments agent activity into turns, snapshots touched files into a content-addressed store, and a CLI that answers "who broke my repo — me or the agent?" with `log`, `diff`, `blame`, and safe per-turn `undo`. The core engine already exists and is proven in production inside Sutra (`src-tauri/src/turns.rs`: hook/quiet boundary detection, blob store with 10 MiB cap, human-touch flags, per-file rollback; 166 passing Rust tests in the host project). This plan extracts it, wraps it, and grows it through five releases.

Builder: one senior engineer (Rust + TS), nights and weekends initially. Platforms: macOS and Linux; Windows is out of scope until demand is proven (D19). Release map: v1 = Phase 0 (extraction), v1.x = Phase 1 (import + git), v2 = Phase 2 (Codex, MCP, VS Code), v3 = Phase 3 (PR bot, signing, Sutra rebase), v4+ = Phase 4 (compliance business). Gates between releases are defined in ROADMAP.md and are pass/fail — this plan assumes each gate passes; if one fails, ROADMAP.md's kill criteria govern, not this document.

## 2. Decision register

Every call that could have been an open question, resolved. Renegotiable, but the default stands unless the founder overrides.

| # | Decision |
|---|---|
| D1 | Extraction is copy-based. Sutra's source is not modified in v1; Sutra rebases onto `agentrec-core` in v3. No risk to the working editor while the recorder stabilizes. |
| D2 | Stack: Rust stable, cargo workspace — `agentrec-core` (lib: engine, store, formats) + `agentrec` (bin: daemon + CLI via clap). No async runtime: threads, `notify` crate for fs events with 1.5 s debounce, 1 s tick thread for quiet-window checks (mirrors Sutra's proven cadences). |
| D3 | *(revised per REVIEW.md F2)* No git **dependency** — snapshots are their own baseline and non-git dirs work — but git **awareness** is mandatory: the daemon watches `.git/HEAD`/index/refs and classifies coinciding mutation bursts as rich turns with `tool: "git"`, hidden from `log` by default. Without this, branch switches flood the ledger with garbage bare turns. |
| D4 | Renames are recorded as delete + create. FS watchers cannot detect renames reliably; no special casing. |
| D5 | Binary files: snapshotted as blobs under the cap; `diff` prints `binary file changed (N → M bytes)`; no textual diff attempted. |
| D6 | One open turn per root. A signal or fresh burst while a turn is open closes the current turn first. Interleaved multi-tool activity in a single root is attributed to the open turn — documented limitation; parallel agents belong in worktrees (distinct roots). |
| D7 | *(revised per REVIEW.md S6)* Prompt comes primarily from the `UserPromptSubmit` start signal; transcript extraction is the fallback and must be defensive (role + content-type filtering, not "last line"). A transcript-format canary test runs in CI; `status` surfaces rich-rate so silent vendor-format changes are caught. Excerpt = first 120 chars post-scrub. |
| D8 | Crash recovery: manifest journal written on every turn close; on daemon restart an orphaned open turn is closed with boundary source `quiet` at the last observed change time. No turn is ever silently dropped. |
| D9 | One daemon per root, enforced by a pidfile lock (`.agentrec/daemon.lock`); stale locks (dead PID) are broken automatically with a logged notice. |
| D10 | `signal.jsonl` consumption is tracked by a persisted byte offset in `.agentrec/state.json`. `purge` may truncate the consumed prefix; the offset resets accordingly. |
| D11 | Scrub defaults built in: AWS key shapes, GitHub/GitLab tokens, `key=`/`token=`/`secret=` assignments, PEM blocks, Slack/Stripe token prefixes, plus entropy > 4.5 bits/char on strings ≥ 20 chars. User rules in `config.toml` are additive; there is no way to disable scrubbing entirely (deliberate). |
| D12 | Timestamps: UTC wall clock in records; monotonic clock for quiet-window measurement (immune to wall-clock jumps/NTP). |
| D13 | Read verbs (`log`, `blame`, `diff`) get `--json` from v1 — machine-readable output is the substrate for the VS Code extension and PR bot; retrofitting it later would churn every consumer. |
| D14 | *(reversed per REVIEW.md S5)* Line-level `blame` ships in v1 — the launch gif is a line-level query, so the launch build answers it. Extra M2 time is budgeted in the honest schedule (D33). |
| D15 | `undo` snapshots current file state before reverting and records the undo as a new turn (`tool: "agentrec"`) — in the v1 CLI, not just the v2 MCP path. Reverts are blame-able and re-revertible from day one. |
| D16 | Config: TOML; invalid TOML is a hard error with line numbers; unknown keys warn and continue. `mcp_destructive` is parsed and validated from v1, used from v2. |
| D17 | License: Apache-2.0 for `agentrec-core`, `agentrec`, protocol, and all v1–v3 components (open-core; superseded the original MIT-OR-Apache dual on founder direction 2026-07-10 — permissive core, patent grant, single license). Hosted/org components in v4 are commercial (see D22 and §8-A3). |
| D18 | Names: crate/binary `agentrec`, org-level repo `agentrec/agentrec`. Name availability on crates.io/GitHub must be verified before first publish (§8-A1). |
| D19 | Windows: excluded through v2; reassessed at v3 from issue demand. All paths handled via `PathBuf`/no hardcoded separators so the port stays cheap. |
| D20 | Signing (v3): Ed25519 detached signatures over RFC 8785 (JCS) canonicalized record bytes; `sig` object = `{alg, key_id, val}`. Decided now so the `sig` reservation in PROTOCOL.md §5 is not an open question; implemented in v3. |
| D21 | L3 sidecar model (resolves PROTOCOL.md former open question): native emitters write `log.<tool>.jsonl` sidecars; readers merge at read time ordered by `started`. One append-only file per writer, no lock contention. |
| D22 | v4 monetization: core stays open forever; attestation service, org policy, and dashboard are commercial, self-hostable, sold via a merchant-of-record checkout (Paddle-class) — no invoicing, no sales motion. |
| D23 | `confirm` mode approval UX (v2): `agentrec approve` lists pending undo requests; approve/deny by id; pending requests expire after 10 minutes. Host-native approval UIs can layer on later without protocol change. |
| D24 | MCP transport: stdio only through v3 (all target hosts speak it); no HTTP/SSE server until a concrete consumer requires it. |
| D25 | *(revised per REVIEW.md F1)* Start signals stay optional in the protocol (stop-only emitters remain conformant, degraded), but the Claude Code integration installs `UserPromptSubmit` + `Stop` **from v1**: bracketing suppresses quiet-window splits and enables retroactive bare-turn merge. |
| D26 | Turn ids are machine-scoped ULIDs (`t_<ULID>`): globally unique, time-ordered, merge-safe across writers/branches (REVIEW.md S4). Shared team history uses per-writer sidecar exports, never a committed live `log.jsonl`. |
| D27 | Epoch records (`type:"epoch"`, daemon start/stop) are part of the v1 data model; `blame` must emit "attribution stale — recording gap" across uncovered intervals or unexplained content (REVIEW.md F4). Service install (launchd/systemd) is v1, not post-v1. |
| D28 | Signal `ts` is unix **milliseconds** (sub-second signal ordering is testable; REVIEW.md minor). |
| D29 | Watch filtering is gitignore-derived by default, plus built-in denylist and user globs; Linux inotify watch-limit exhaustion is a loud startup error with the sysctl remedy, never silent partial watching (REVIEW.md S2/S9). |
| D30 | Two change predicates, never conflated (REVIEW.md S3): `modified-since` (hash) gates all destructive ops; `human-edited-since` (derivation, rich-coverage only) is blame display context. The MCP undo parameter is `allow_modified`, not `allow_human_touched`. |
| D31 | Snapshot-side secret defense (REVIEW.md S1): secret-file patterns are never snapshotted (`withheld: true`, never revertible); prompt TTL defaults to 90 days; object store has a 2 GiB default budget with oldest-eviction; `status` reports store size. The phrase "provably nothing to leak" is banned from all docs. |
| D32 | `status` is the sixth v1 verb: store size, gaps, rich-rate. Rich-rate is the early-warning system for hook/transcript breakage. |
| D33 | Honest v1 schedule: ~8 weekend-equivalents across milestones M1 (record) / M2 (answer) / M3 (ship), with the M1-exit dogfood readability tripwire as self-kill criterion (REVIEW.md S7 + steelman). |
| D34 | *(hardening review 2026-07-09)* Durability policy: `log.jsonl` appends for turn closes and epoch records are fsynced (`File::sync_all`) before the append returns; signal-inbox appends are exempt (reconstructible from re-emission, and hot-path). Blob writes fsync the tmp file before rename and fsync the containing directory after rename; tmp names are unique per writer (`.tmp.<pid>.<seq>`) so concurrent snapshot threads can never interleave writes into one tmp file. A ledger that can lose a just-closed turn to power loss is not a ledger. |
| D35 | *(hardening review 2026-07-09; prompt-put leg closed 2026-07-17)* Snapshot-failure taxonomy: `BlobStore::put` returns a typed result distinguishing over-cap from I/O failure (disk full, EPERM). The wire format is unchanged — `skipped: true` covers both, no PROTOCOL change — but the *reason* is operational data: I/O failures increment a counter persisted in `state.json`, log to stderr at occurrence, and surface in `status` as a `DEGRADED — N snapshot writes failed` banner naming the likely cause. `undo` on an I/O-skipped file says "no snapshot exists (write failed at record time)", never "over size cap". Extended to PROMPT blob writes: a failed prompt store (in both the steady-state `persist` path and crash-recovery's `recover_orphan`) increments a *separate* `state.json` counter (`prompt_put_failures` — never conflated with `snapshot_failures`/`io_failed`, which are file-scoped) and gets its own `status`/`doctor` DEGRADED line; `show --prompt` distinguishes "no prompt attached" from "prompt blob missing — write failed at record time" using the wire-visible `prompt_excerpt`/`prompt_ref` combination (excerpt present, ref null), since the wire format itself gets no new field. A safety net that is silently absent is worse than none. |
| D36 | *(hardening review 2026-07-09)* Undo torture harness: a nightly CI job drives the real binary through randomized interleavings of agent bursts, human edits, git checkouts, daemon kill -9, and undo/redo cycles, asserting two invariants after every undo: (1) undo never writes to a file whose current hash ≠ the turn's `after` (the D30 rail), and (2) every executed undo is itself undoable to the exact pre-undo state. Violations block release with the full operation trace as a CI artifact. The harness is public and linked from the README — the CI job that tries to destroy data *is* the trust story. |
| D37 | *(hardening review 2026-07-09)* Store permissions: `init` creates `.agentrec/` mode 0700; the daemon creates all files 0600, umask-independent. Prompts are post-scrub but still private text; other local users have no business reading them. Non-POSIX filesystems log a one-time warning rather than failing. |
| D38 | *(hardening review 2026-07-09)* Planted-secret coverage extends to the signal inbox: hooks scrub prompt text *before* emitting the signal line, and the planted-secret test (AC I4) greps `signal.jsonl` as well as `log.jsonl` and `objects/`. No pre-scrub byte persists anywhere, including the inbox. |
| D39 | *(accessibility review 2026-07-09)* Distribution matrix: v1 launch ships prebuilt binaries (macOS arm64/x86_64, Linux x86_64/arm64), a checksum-verified `curl \| sh` installer, and the brew formula; `cargo install` remains for Rust users but is never the headline path — the target user is a JS/Python dev running agents, not a Rust dev. v1.x adds an npm wrapper (`npx agentrec`, postinstall fetches the platform binary) and a mise/asdf plugin. All wrappers are packaging only: one binary, one behavior. |
| D40 | *(accessibility review 2026-07-09)* One-command onboarding: `agentrec init` performs full setup by default — detects installed agent tools (Claude Code in v1; Codex joins in v2), installs hooks, registers and starts the service unit, and begins recording. `--no-hook` / `--no-service` / `--dry-run` opt out. Init prints exactly what it did and how to reverse it. `agentrec uninstall` is the symmetric exit: removes hooks and the service unit, moves `.agentrec/` to an archive path — never deletes (house rule). Trust requires a visible exit door. |
| D41 | *(accessibility review 2026-07-09)* `agentrec doctor`: one-shot diagnosis of the whole chain — binary version, daemon liveness, hook presence and validity in tool configs, signal freshness, rich-rate, store health/DEGRADED, inotify headroom (Linux), store permissions. Every check prints pass/fail plus a one-line remedy; exit 0 all-pass, 1 otherwise (scriptable); `--json` for CI. Doctor is a setup command alongside `init`/`service`/`uninstall`, not a seventh query verb — the six-verb surface is unchanged. |
| D42 | *(accessibility review 2026-07-09)* Panic undo: bare `agentrec undo` (no turn id) targets the most recent non-git rich turn, always preview-first (checklist shown, explicit confirmation required). If the latest activity is a bare turn it refuses with guidance ("last activity window is unattributed — pick a turn from `agentrec log`") rather than guessing. Rationale: the user reaching for undo is mid-disaster and should not be hunting ULIDs. |
| D43 | *(accessibility review 2026-07-09)* Comprehension defaults: `log` prints human-relative times ("14 min ago"; absolute under `--utc` and always in `--json`); grades are color-coded in TTYs (off under `NO_COLOR` or non-TTY); `--explain` on read verbs appends a short glossary of exactly the terms present in that output. The README leads with the disaster-recovery story in its first ten lines; architecture comes after. |
| D44 | *(accessibility review 2026-07-09)* Claude Code plugin (v2): marketplace packaging that bundles the hook config and MCP registration into a one-step install for Claude Code users, fetching the binary via the D39 installer (with explicit user consent) when absent. Packaging only — the plugin and `agentrec init` must produce identical configuration from a single source of truth; no logic forks. |

## 3. v1 — Extraction (target: ~8 weekend-equivalents, milestones M1–M3 per D33)

### Workstreams

**WS1 — Scaffold.** Repo, workspace layout, CI (fmt + clippy + `cargo test` on macOS/Linux matrix), Apache-2.0 `LICENSE` + `NOTICE`, release profile.

**WS2 — Core port.** `BlobStore` (content-addressed, sha256, fan-out dirs, 10 MiB cap), `TurnEngine` (signal + quiet boundary, open-turn state machine), manifest journal, `state.json`. Ported from `turns.rs` with Sutra-specific types removed; unit tests ported and extended.

**WS3 — Watcher.** `notify`-based recursive watch, 1.5 s debounce, built-in denylist (`.git`, `.agentrec`, `node_modules`, `target`, `dist`, plus user globs), self-write suppression.

**WS4 — Signals and hooks.** Signal-file tailer with offset persistence; `agentrec init`: creates `.agentrec/`, writes gitignore entry, installs Claude Code Stop hook by idempotent JSON merge into settings (Sutra's merge logic reused); transcript prompt extraction; scrub pipeline.

**WS5 — CLI.** `record`, `log`, `diff`, `blame`, `undo` with `--json` on read verbs; exit codes: 0 success, 1 user error, 2 internal error.

**WS6 — Retention.** `ttl_days`, `purge` (prompts, optionally snapshots), dangling-ref tolerance.

**WS7 — Launch.** README with blame gif, PROTOCOL.md v0.1 in-repo, dogfooding on the Sutra repo itself from the first working build.

**WS8 — Accessibility.** Installer matrix (prebuilt binaries, `curl | sh`, brew); one-command `init` with agent-tool auto-detection and symmetric `uninstall`; `agentrec doctor`; panic-undo default; comprehension defaults (relative times, color, `--explain`).

### Acceptance criteria — v1

**A. `init`**
1. Fresh directory: creates `.agentrec/{config.toml,signal.jsonl,log.jsonl,objects/}`, appends `.agentrec/` to `.gitignore` (creating it if the dir is a git repo and no `.gitignore` exists).
2. Re-run is a no-op (idempotent): no duplicate gitignore lines, no hook duplication, exit 0.
3. Non-git directory: init succeeds; no `.gitignore` is created; a notice explains git is not required.
4. Existing Claude Code settings with unrelated hooks: merge preserves them byte-for-byte apart from the added hook; a backup of the pre-merge file is written.
5. Malformed existing settings JSON: init aborts with a clear error, original file untouched.
6. `--no-hook` flag skips hook installation (bare-turn-only setups).
 /
**B. Daemon / `record`**
1. Second `record` on the same root exits 1 with a message naming the live PID; stale lock (dead PID) is broken and startup proceeds with a logged notice.
2. Kill -9 mid-turn, restart: orphaned open turn appears in `log` closed with boundary `quiet`; no manifest corruption; subsequent turns record normally.
3. Writes under denylisted dirs never open a turn; a burst that touches both `src/` and `node_modules/` records only `src/` files.
4. The daemon's own writes to `.agentrec/` never trigger the watcher (no feedback loop) — verified by a soak test: idle daemon on a quiet repo opens zero turns in 24 h.
5. Symlinked files/dirs are not followed; the symlink itself is recorded on change.
6. File > 10 MiB in a turn: entry carries `skipped: true`, no blob written, warning logged; the rest of the turn records normally.
7. Unreadable file (permissions) at snapshot time: entry carries `skipped: true` and a warning; the daemon does not crash.
8. Idle CPU < 1 % of one core; RSS < 50 MB on a 100k-file repo (measured in CI perf smoke on Linux).

**C. Turn segmentation**
1. Hook signal closes the open turn with grade `rich`, `tool` from the signal, boundary source `hook`.
2. No emitter: a mutation burst followed by 10 s of quiet closes a `bare` turn.
3. Signal arriving while a turn is open: current turn closes first (D6); attribution matrix documented in tests for hook-then-quiet, quiet-then-hook, hook-then-hook.
4. Two signals < 1 s apart: two turns, correctly ordered, no file double-attribution.
5. Signal lines with unknown fields are consumed normally (PROTOCOL §4 tolerance).
6. Corrupt/truncated signal line: skipped with a warning; the tailer continues from the next line; offset never regresses.
7. Wall-clock jump (manual clock change during an open turn): quiet-window still closes on monotonic time (D12); record timestamps remain sane (end ≥ start enforced by clamping with a logged notice).
8. Mutations with zero net content change (touch, same bytes rewritten): turn records the file with identical before/after hashes and `op: modify`; `diff` shows it as unchanged.

**D. Snapshots / object store**
1. Identical content across files/turns deduplicates to one blob.
2. `op` correctness for create (`before: null`), modify, delete (`after: null`); rename yields delete + create (D4).
3. Empty (0-byte) files snapshot and revert correctly.
4. Unicode and deeply nested paths (< OS limits) round-trip exactly.
5. Object integrity: a corrupted blob (bit-flipped on disk) is detected by hash mismatch at read time and reported, never silently served.

**E. `log`**
1. Newest-first; `--tool`, `--session`, `--file` filters compose (AND semantics).
2. Empty state prints a friendly hint (`no turns recorded — is "agentrec record" running?`), exit 0.
3. `--json` emits one JSON object per line matching PROTOCOL §5 verbatim; 10k-turn log renders in < 1 s.
4. Bare turns render without tool/prompt columns rather than with fabricated values.

**F. `diff`**
1. Text files: correct unified diff per file, created and deleted files shown as full add/remove.
2. Binary files: the D5 message, no garbage output.
3. `skipped` files: explicit `(content not snapshotted — over size cap)` notice.
4. Unknown turn id: exit 1 with the valid id range.

**G. `blame`**
1. `blame <file>`: last turn to touch the file, with tool/prompt excerpt when rich, and `· human-edited since` when derived (PROTOCOL §5 derivation).
2. File never touched by any turn: `no recorded turn touches <file>`, exit 0.
3. File touched only by a bare turn: shows turn id and time, no fabricated attribution.
4. Deleted file: blame still resolves from history.
5. `blame <file>:<line>`: line-level attribution computed from before/after snapshot diffs (D14): correct turn for added and modified lines; deleted-line queries resolve to the deleting turn; lines predating recorded history return "before recording began", never a guess.
6. Gap honesty: blame across an epoch gap, or on content no recorded turn explains, prints `attribution stale — recording gap` (D27); never names the last recorded turn as if certain.

**H. `undo`**
1. Clean revert of a whole turn restores every file's `before` blob exactly (byte-for-byte, verified by hash).
2. Per-file checklist: any subset revertible; deselected files untouched.
3. Modified-since files (current content ≠ turn's `after` hash — the D30 predicate, regardless of who changed it): excluded by default, included only behind an explicit `--allow-modified` confirmation; warning names each file and, where derivable, what changed it (later turn, human, git, gap).
4. `skipped` files: refused always, with explanation (no content exists).
5. Reverting a `create` deletes the file; reverting a `delete` restores it (including parent dirs).
6. The undo itself appears in `log` as a new turn, `tool: "agentrec"` (D15); undoing the undo restores the pre-undo state.
7. Concurrent daemon activity: undo's own writes are attributed to the undo turn, not a spurious bare turn.

**I. Scrub / retention**
1. Each built-in pattern class (D11) has a positive and negative test; scrubbed spans render as `[redacted:<reason>]`.
2. Entropy rule catches a random 32-char token in prose; does not fire on ordinary code identifiers.
3. User-supplied rule in `config.toml` applies additively.
4. `prompt_excerpt` is always post-scrub — no path exists where pre-scrub text is persisted (asserted by construction: scrub runs inside the persistence function, covered by a test that greps the store for a planted secret).
5. `purge` removes prompt objects past `ttl_days`; `log`/`blame` on purged turns still work with the excerpt; `diff`/`undo` unaffected (snapshots are separate objects).
6. `purge --all-prompts` and `purge --snapshots-before <date>` behave as named; both print what they deleted.

**J. Config & platform**
1. Missing config: all defaults apply. Invalid TOML: hard error with line number. Unknown keys: warning, continue (D16).
2. `mcp_destructive` accepts exactly `off|confirm|auto`, defaults `off`, rejects others with an error naming valid values — parsed in v1, acted on in v2.
3. CI green on macOS 14 + Ubuntu 22.04/24.04 for every AC above that is automatable; the soak (B4) and perf (B8) checks run nightly, not per-commit.

**v0.2 amendment — additional v1 AC (from REVIEW.md; these supersede any conflicting criterion above):**

**C+. Bracketing (F1):** start signal suppresses quiet-window closure inside the bracket (agent pauses of 60 s+ mid-turn stay one turn); bare turns closed inside a bracket are retroactively merged into the rich turn on stop, with file lists unioned and no duplicate ids; start-without-stop closes at last mutation with `truncated: true` and keeps attribution; human editor-save bursts *outside* any bracket produce bare turns that are never rendered as agent activity and never satisfy rich coverage for the human-edited-since predicate.
**C++. Git turns (F2, D3):** `git checkout`/`pull`/`stash pop`/`rebase` bursts are classified `tool: "git"` (rich) when coinciding with ref/index transitions; `log` hides them by default, `--all` shows them; `blame` reports them honestly; a checkout touching 400 files never appears as an unknown bare turn.
**B+. Filtering & watch integrity (S2/S9, D29):** gitignore semantics respected by default — a running Next.js dev server (`.next/`), Python venv, and `__pycache__` churn produce zero turns; inotify watch-limit exhaustion on Linux is a startup error naming the sysctl fix; nested-gitignore precedence follows git's own rules.
**E+. Epochs & status (F4, D27/D32):** epoch records appended on daemon start/clean stop; kill -9 leaves a derivable gap (no false "covered" interval); `status` reports store size, gap list, and rich-rate; rich-rate < 90 % over trailing 20 agent turns prints a hook-health warning.
**I+. Snapshot secrets & budgets (S1, D31):** `.env`/`*.pem`/credential-pattern files are `withheld: true` — never in the object store (verified by planted-secret scan), never revertible; TTL default 90 d active without config; 2 GiB budget evicts oldest snapshots with a `status` notice; no doc or output uses the word "provably".
**K+. Ids (S4, D26):** ULID ids monotone within a writer, unique across two machines writing the same repo (sidecar model); display truncation unambiguous with prefix match on all verbs.
**Q+. Transcript canary (S6, D7):** CI fixture parses a pinned real transcript sample per supported tool version; extraction failure fails CI, and at runtime downgrades gracefully to promptless rich turns (never wrong-prompt attribution).

**v0.3 amendment — durability & trust hardening (code review 2026-07-09; these add to, and where conflicting supersede, the criteria above):**

**L+. Ledger durability (D34):** kill -9 (or simulated power loss) immediately after a turn close loses nothing — the closed turn is present after restart, verified by a harness that kills the daemon at randomized points around close; a blob is never observed half-written after crash (unique tmp + fsync-before-rename + dir fsync); two threads snapshotting identical content concurrently produce exactly one intact blob (no tmp-name collision).
**M+. Failure honesty (D35):** with the object store on a full disk, the daemon keeps recording turn boundaries, marks affected entries `skipped: true`, increments the persisted failure counter, and `status` shows the DEGRADED banner with count and likely cause; `undo` on such files refuses with "no snapshot exists (write failed at record time)" — never the over-cap message; the counter and banner clear only by explicit `status --ack-degraded` so a transient failure is never silently forgotten. **Prompt-put leg (closed 2026-07-17):** the same fault (object store unwritable) hitting a turn's PROMPT blob write, in isolation from any file-snapshot failure on the same turn, increments its own `prompt_put_failures` counter (proven independent of `snapshot_failures`/`io_failed` by a real-daemon test that snapshots a file successfully *before* locking the store down, then fails only the later prompt write); `status` and `doctor` both show a distinct DEGRADED line for it; `show --prompt` on the affected turn says "prompt blob missing — write failed at record time", never fabricating "no prompt attached"; `status --ack-degraded` clears both counters together.
**H++. Torture harness (D36):** nightly randomized-interleaving run (≥ 1,000 operations per run: bursts, human edits, checkouts, kill -9, undo/redo) with both D36 invariants asserted after every undo; any violation fails CI with the operation trace attached; **harness green for 7 consecutive nights is a launch gate** (added to ROADMAP Phase 0).
**I++. Permissions & inbox scrub (D37/D38):** fresh `init` yields 0700 `.agentrec/` and 0600 files, asserted on macOS and Linux CI; a planted secret in a prompt never appears in `signal.jsonl`, `log.jsonl`, or `objects/` (byte-level grep across all three, run per-commit).

**v0.4 amendment — accessibility (review 2026-07-09; additive):**

**Y+. Install & first run (D39/D40)**
1. The `curl | sh` installer on a clean macOS and a clean Ubuntu container yields a working `agentrec` on PATH: checksum-verified download, no sudo required (installs to `~/.local/bin`, or an alternative with explicit consent), uninstall instructions printed on completion.
2. `brew install` works at launch (own tap acceptable; homebrew-core submission tracked as a post-launch task).
3. `agentrec init` on a repo where Claude Code is installed produces hooks + service unit + live recording in one command; output lists every action taken and the single command that reverses it; re-run is a byte-for-byte no-op.
4. `agentrec uninstall` removes hooks and the service unit and moves `.agentrec/` to an archive path (`.agentrec.archived.<ts>/`); nothing is deleted; a later `init` starts fresh while the archive remains.
5. `init --dry-run` prints intended actions and touches nothing (asserted by fs snapshot comparison).

**Y++. Doctor (D41)**
1. On a healthy repo, `doctor` prints all-pass and exits 0 in < 2 s.
2. Each induced failure is detected and named with its remedy, one integration test per mode: daemon not running; hook missing or mangled in settings; signal inbox stale while a transcript shows recent agent activity; DEGRADED store; Linux inotify headroom below watch count; wrong store permissions.
3. `doctor --json` emits machine-readable results (CI-consumable); exit codes: 0 all-pass, 1 any-fail.

**Z+. Panic undo & comprehension (D42/D43)**
1. Bare `agentrec undo` targets the most recent non-git rich turn, preview-first with explicit confirmation; when the latest activity is bare or no turns exist, it refuses with the D42 guidance message; never guesses.
2. `log` renders relative times by default; `--utc` and `--json` render absolutes; golden-file tests cover both.
3. Color is on in a TTY, off under `NO_COLOR` or when piped; asserted in both environments.
4. `--explain` appends glossary entries for exactly the terms present in that invocation's output (a bare-turn-free listing does not explain bare turns).

**v1 done =** all AC (including amendments) pass; the M1-exit tripwire (ROADMAP.md Phase 0) passed on two repos; `agentrec` has recorded, blamed, and undone real turns on the Sutra repo for one week of dogfooding; README + PROTOCOL.md v0.2 published.

## 4. v1.x — Cold start and git

### Tasks and acceptance criteria

**K. `import claude`** (backfill from `~/.claude/projects/*.jsonl`)
1. Maps sessions to the repo by transcript `cwd`; only sessions whose cwd is inside the target root import.
2. Each assistant turn with file mutations becomes a `rich` turn; `before` hashes reconstructed from git history when available, else recorded with `before: null` and an `imported: true` metadata field — imported turns are never `undo`-able when `before` is unreconstructable (refused with explanation).
3. Idempotent: re-import skips already-imported turns (keyed on session id + turn index); interrupted import resumes cleanly.
4. Malformed transcript lines are skipped with a per-file summary count; import never aborts on one bad line.
5. 500 MB of transcripts imports without exceeding 500 MB RSS (streaming parse).
6. Prompts pass the same scrub pipeline as live capture.

**L. `import aider`**
1. Detects aider auto-commits by message convention; each becomes a `rich` turn with `tool: "aider"`, before/after from git parents.
2. Non-aider commits are never imported; merge commits skipped with a notice.
3. Idempotent by commit SHA.

**M. Git trailers + `git-agentrec`**
1. Opt-in `agentrec init --git-trailers` installs a post-commit hook appending `Agent-Turn: <ids>` for turns whose files intersect the commit; hook is idempotent and preserves existing hooks (chains, does not overwrite).
2. Commits with no intersecting turns get no trailer.
3. `git agentrec <verb>` == `agentrec <verb>` exactly (thin exec shim on PATH); works when invoked from repo subdirectories.
4. History rewrites (rebase/squash) are explicitly out of scope: trailers reflect commit-time knowledge; documented.

**N. Protocol freeze**
1. All schema changes since v0.1 folded in; `FORMAT-CHANGELOG.md` started; conformance fixture suite (golden files for every record/signal variant, both grades, all op types) published in-repo — this suite is what third-party emitters test against.

**Z2. Distribution wrappers (D39)**
1. `npx agentrec@latest log` works on macOS and Linux with no Rust toolchain present: postinstall fetches the platform binary, checksum-verified; unsupported platforms fail with a clear message naming the supported matrix, never a cryptic build error.
2. The npm package version is locked one-to-one to the binary release version; publishing is a CI step of the release workflow, not a manual action.
3. The mise/asdf plugin installs and pins versions; the README install matrix documents all five paths (installer script, brew, npm, mise, cargo) with one-line commands.

### Memory (v1)

Hash-pinned semantic memory (see `docs/superpowers/specs/2026-07-12-agentrec-memory-design.md`,
status: implemented). All 12 memory tasks landed; every invariant below now names
its complete test set — no "pending" rows remain.

1. **INV-M1** — no memory record exists without ≥1 valid in-root pin: fuzz malformed
   candidates (traversal, absolute paths, symlink escape, empty pins). Maps to tests:
   `agentrec-core::memory::tests::pin_path_rejections` (traversal/absolute/symlink-escape/
   secret-path/nonexistent-path rejection, unit level); `cli/tests/hardening_daemon.rs::
   candidate_rejects_counted_never_fabricated` (4 genuinely-rejecting daemon-ingested
   candidates — traversal pin, secret-file pin, empty-after-scrub fact, >PINS_MAX pins —
   leave `memory.jsonl` untouched, counted in `state.json.memory_rejects`); `cli/tests/
   torture.rs::{torture_smoke,torture_survives_chaos}` (`assert_memory_invariants`,
   Task 12 — every RAW line in `memory.jsonl` parses as a `MemoryRecord` with ≥1 pin,
   checked after every op batch including immediately after a daemon `kill -9` +
   respawn, proving fsynced appends never leave a torn line).
2. **INV-M2** — stale/orphaned memories are never emitted by the injection path:
   mutate a pinned file, recall again, memory gone. Maps to tests: `agentrec-core::
   memory::tests::{recall_never_returns_stale,recall_verifies_only_top_candidates,
   freshness_transitions}` (core rank-then-verify + freshness derivation);
   `cli/tests/integration.rs::recall_cli_fresh_only_and_json` (CLI-level fresh-only
   contract); `cli/tests/torture.rs::{torture_smoke,torture_survives_chaos}`
   (`op_recall`, Task 12 — every memory an in-flight `recall --json` returns is
   re-hashed against the live working tree and asserted `Freshness::Fresh`, exercised
   under concurrent chaos mutation/kill-9/undo; a run-long ANCHOR memory guarantees
   the check is never vacuously skipped).
3. **INV-M3** — planted secret never lands in `memory.jsonl` via either write path.
   Maps to tests: `cli/tests/integration.rs::remember_refuses_bad_pins_and_secret_facts`
   (manual `remember` path); `cli/tests/hardening_daemon.rs::
   candidate_secret_fact_scrubbed_on_disk` (agent `candidate` → daemon-ingestion path);
   `cli/tests/integration.rs::secret_prompt_never_reaches_disk_in_cleartext` (Task 12 —
   extended to drive the SAME planted AWS-key-shaped secret through both `remember` and
   `candidate`, plus a matching `UserPromptSubmit` hook, then grep all 4 locations —
   `signal.jsonl`, `log.jsonl`, `.agentrec/objects/**`, `memory.jsonl`, and
   `memory-stats.jsonl` — proving the raw secret is nowhere while a redaction marker is
   present everywhere it should be).
4. **INV-M4** — hook path exits 0 and within budget under: corrupt store, missing
   store, uninitialized repo, 3000-record store, injection-within-budget-at-that-scale,
   and concurrent append. Test-closeable legs, each PASS with a named test: `cli/tests/
   integration.rs::hook_injects_fresh_memories_into_stdout` (enabled-path injection,
   `memory_enabled=false` kill switch, Stop-arm never injects, `memory_inject_max`
   coupling); `cli/tests/integration.rs::hook_fail_open_and_budget` (corrupt
   `memory.jsonl` → exit 0 + no block + start signal still appended; missing store and
   a fully uninitialized `.agentrec/` both exit 0 with no block, start signal still
   appended even when `.agentrec/` never existed; a 3000-record store — 3000 orphaned
   fillers + 1 genuinely Fresh real-pinned record — answers within the 500ms CI-slack
   budget AND actually emits the injected block for the Fresh record, proving the
   budget pass isn't silent degradation to a no-op); `cli/tests/integration.rs::
   hook_exits_zero_under_concurrent_memory_append` (a direct-API writer thread races
   `memory::append_memory` against 30 real `agentrec hook claude` subprocess calls with
   no synchronization; every call exits 0, appends its start signal, and every raw
   `memory.jsonl` line still parses with >=1 pin afterward — the property held on
   first write, no source change needed).

   The 50ms `RECALL_BUDGET_MS` bound is **not** enforced by the cooperative
   in-loop deadline checks alone (`memory::recall_with_deadline` re-checking
   `Instant::now()` between `load_effective`'s fold, `bm25_rank`'s scoring, and
   the freshness-verify walk) — that shape still lets one slow blocking call
   *inside* a step (e.g. a stalled-volume `fs::read` in `memory::hash_pin`)
   overrun the budget by however long that call blocks, so it was cooperative
   between steps, not a hard outer wall. **F8** (`78cf735`) closes that gap:
   `inject_memory` (`cli/src/cmds.rs`) now runs the recall on a detached
   worker thread and waits only for the *remaining* budget via
   `mpsc::Receiver::recv_timeout`, so a hang anywhere inside the recall —
   including inside one blocking step — can no longer push the observable
   hook past `RECALL_BUDGET_MS`; a timed-out or budget-exceeded worker fails
   open (inject nothing, exit 0) and records one `{"budget_exceeded":true}`
   line in `memory-stats.jsonl`. The retrospective `elapsed_ms >
   RECALL_BUDGET_MS` check stays as cheap defense-in-depth, not the primary
   bound. Test: `cli/tests/integration.rs::hook_recall_hard_wall_deadline`.

   **F10** (`f75267f`) closes a related honesty gap: a malformed/unreadable
   *non-empty* `memory.jsonl` was previously folded to "no matches" —
   indistinguishable from a healthy empty-result recall. It is now a distinct
   RECALL FAILURE: the hook still fails open (inject nothing, exit 0) but
   appends `{"ts","failure":true,"reason":"store_corrupt"}` to
   `memory-stats.jsonl` instead of a plain no-match line, and `status` prints
   an aggregate memory-failure count read from those stat lines. Tests:
   `cli/tests/integration.rs::hook_corrupt_memory_store_is_counted`,
   `cli/tests/integration.rs::hook_corrupt_store_reason_never_leaks_raw_control_bytes`,
   `cli/tests/integration.rs::hook_corrupt_store_safe_under_concurrent_append`,
   `cli/src/cmds.rs::tests::status_counts_memory_failures_and_tolerates_malformed_stats_lines`.

   **LADDERED, not test-closeable on CI:** the spec's 10k-record / 50ms
   **hard** performance envelope (`docs/superpowers/specs/
   2026-07-12-agentrec-memory-design.md` §Performance envelope) is wall-clock
   timing that varies by CI runner load — the tests above prove correctness
   (fail-open, real injection, no torn writes, hard-wall enforcement, corrupt-
   store honesty) at a 3000-record/500ms CI-slack scale, not the spec's exact
   10k/50ms hard budget; that number is verified via the 1-week dogfood
   counters in `status` (injections/rejects/stale-quarantined/failures) per
   the spec's Success gate, never claimed as CI-test-proven.
5. **INV-M5** — fold determinism: same records ingested in any order produce the
   same effective state (property test). Maps to tests: `agentrec-core::memory::
   tests::fold_latest_op_wins_any_order` (Task 1 — 6-permutation assert/reverify/retract
   fold); `agentrec-core::memory::tests::{fold_equal_ts_retract_wins_any_order,
   fold_equal_ts_reverify_wins_over_assert_any_order}` (codex-found equal-timestamp fix —
   op-precedence tie-break, both file orderings); `cli/tests/hardening_daemon.rs::
   dangling_source_turns_closed_by_pre_persist_journal_sync` (codex-found crash-window
   fix — a candidate's `source_turns` id is always journal-recoverable before it's
   durably referenced, closing the same-iteration start+candidate race).

**F8/F9/F10 (PR #2 review-findings fix round, 2026-07-12, branch `feat/memory-v1`):**

- **F8** (`78cf735`) — see INV-M4 above: replaces the cooperative-only recall
  deadline with a hard outer wall via a detached worker thread +
  `mpsc::Receiver::recv_timeout` in `inject_memory` (`cli/src/cmds.rs`), so a
  blocking call inside one recall step can no longer push the hook past
  `RECALL_BUDGET_MS`. Test: `cli/tests/integration.rs::
  hook_recall_hard_wall_deadline`.
- **F9** (`742ef04`) — `agentrec verify <id> [--confirm] --replace-pin
  <old>=<new>` (repeatable): re-points an existing pin (fresh, stale, or
  orphaned) to an explicit successor path instead of only allowing
  `--drop-pin`, closing the "sole pinned file renamed" recovery gap without
  minting a new memory id (`op: Reverify`, same id, per the design spec's
  "reverify/retract never mint a new id" rule). `new` is validated identically
  to `remember --from` (`memory::validate_pin_path`: in-root, no `..`
  traversal, no symlink escape, must exist, not a secret path); every
  `--replace-pin` is parsed and validated up front, atomically, before
  anything is printed or appended — malformed `OLD=NEW` syntax, a duplicate
  `old`, an `old` also named by `--drop-pin`, an `old` not currently a pin on
  the memory, or an invalid `new` all abort with nothing appended. Deliberately
  **no automatic rename detection** — an ambiguous guess could silently
  re-ground a fact against the wrong source (design spec's rejected-approaches
  list). Implementation: `cli/src/memorycmds.rs::{parse_replace_pin,verify}`,
  flag wired in `cli/src/main.rs`. Tests: `cli/tests/integration.rs::
  {verify_replace_pin_preserves_renamed_sole_pin,
  verify_replace_pin_keeps_other_pins_on_multi_pin_memory,
  verify_replace_pin_rejections_append_nothing}`.
- **F10** (`f75267f`) — see INV-M4 above: a malformed/unreadable non-empty
  `memory.jsonl` is now a distinct RECALL FAILURE (`memory-stats.jsonl`
  `{"failure":true,"reason":"store_corrupt"}`, never silently folded to
  "no matches"), still exit 0 / inject nothing; `status` surfaces an
  aggregate failure count. Tests: `cli/tests/integration.rs::
  {hook_corrupt_memory_store_is_counted,
  hook_corrupt_store_reason_never_leaks_raw_control_bytes,
  hook_corrupt_store_safe_under_concurrent_append}`, `cli/src/cmds.rs::
  tests::status_counts_memory_failures_and_tolerates_malformed_stats_lines`.

## 5. v2 — The integration release (Codex, MCP, VS Code)

### O. Codex CLI capture
1. `agentrec init --codex` merges a Stop hook into Codex's hook config (`hooks.json` or `[hooks]` in `config.toml`, whichever exists; `hooks.json` created if neither), idempotently, preserving unrelated hooks; malformed existing config aborts untouched (mirror of A5).
2. Hook emits the standard signal line with `tool: "codex"` and the rollout path as `transcript`.
3. Prompt extraction from rollout format (last user message, D7); scrubbed.
4. `import codex` from `~/.codex/sessions/**/rollout-*.jsonl` with the full K-series AC applied (cwd mapping, idempotency, malformed-line tolerance, streaming, scrub).
5. Neutrality proof test: one live session each of Claude Code and Codex on the same repo yields one `log.jsonl` with both tools attributed correctly and zero cross-attribution.

### P. MCP server (`agentrec mcp`)
1. stdio transport (D24); `tools/list` exposes `agentrec_log`, `agentrec_diff`, `agentrec_blame` always, `agentrec_undo` only when `mcp_destructive != "off"`.
2. Read tools return structured JSON mirroring `--json` output; results over 200 turns / 2,000 diff lines paginate with a cursor — no unbounded payloads into agent context.
3. Works with the daemon stopped (reads are file-based); says so in a `status` field rather than erroring.
4. Mode matrix — `off`: undo absent from tools/list; calls to it error. `confirm`: undo call registers a pending request and returns `pending` + request id; `agentrec approve` lists/approves/denies; approval executes and returns the result on the agent's next poll; requests expire in 10 min (D23); denied and expired requests return distinct statuses. `auto`: two-phase — call 1 returns preview (per-file diff summary, warnings, `confirm_token`); call 2 with the token executes; token is single-use, 60 s TTL; wrong, reused, or expired tokens error without side effects; preview-to-execution drift (files changed between calls) aborts with a fresh-preview instruction.
5. In every mode: `skipped` and `withheld` files refused; modified-since files excluded unless `allow_modified: true` AND (mode `auto` or human approval) — the D30 predicate, never the display-level one; executed undos recorded as turns (H6 semantics).
6. Concurrency: two simultaneous undo requests on overlapping files — second is rejected with a conflict error; no partial interleaved reverts.
7. Prompt-injection stance tested: tool descriptions contain no instructions that could be construed as commands to the agent; tool outputs are data-only (no imperative text) — reviewed against the current MCP security guidance at build time.

### Q. Claude Code deepening
1. (Start-signal bracketing moved to v1 — D25.) v2 hardens the integration: transcript-format canary in CI per supported version, rich-rate alerting surfaced in `status` and MCP `status` field, managed-settings environments detected with a clear "hooks unavailable — running degraded" notice.
2. `.mcp.json` registration via init: idempotent merge, unrelated servers preserved.
3. CLAUDE.md self-healing recipe published and validated: in ≥ 3 scripted scenarios (broken test, wrong-file edit, regression), Claude Code with the recipe uses `agentrec_blame` before re-editing.
4. Claude Code plugin published (D44): one install action configures hooks + MCP registration for the project; on a machine without the binary, the plugin runs the D39 installer only after explicit user consent; plugin uninstall reverses cleanly with archive semantics (mirror of Y+4).
5. Single source of truth: the plugin and `agentrec init` generate their hook/MCP configuration from the same code path, asserted by a test comparing both outputs byte-for-byte — support ever has exactly one story.

### R. VS Code extension
1. Activates only when `.agentrec/log.jsonl` exists in the workspace; zero activation cost otherwise.
2. Gutter blame per line: turn id, tool, excerpt, human-edited-since on hover; bare turns show grade + time only; files with no turn history show nothing (no noise).
3. Tail-follows `log.jsonl`: new turns appear in ≤ 2 s without reload; log rewritten by `purge` triggers a full refresh, not a crash.
4. Per-turn diff opens as a read-only virtual document; works for created/deleted/binary/skipped files with the F-series renderings.
5. Turns panel filterable by tool/session/file; 10k turns scroll without jank (virtualized list).
6. `undo` action opens the integrated terminal running `agentrec undo <id>` — the extension itself never writes to the store or the working tree.
7. Multi-root workspaces: each root with `.agentrec/` gets independent blame; roots without it are ignored.
8. Read tolerance per PROTOCOL §7: unknown fields, both grades, dangling `prompt_ref`.
9. Packaged and published to Marketplace + Open VSX; extension size < 5 MB; no telemetry.

**v2 done =** O + P + Q + R pass; ROADMAP v2 gate metrics being tracked; the self-healing gif recorded from a real (unscripted) session.

## 6. v3 — Review surfaces and signing

**S. PR bot / GitHub Action**
1. Reads `log.jsonl` from an uploaded artifact or committed path (opt-in); absent log = silent skip, never a failing check.
2. Posts one summary comment (turns, tools, files human-edited-after-agent) and updates it on force-push — never a second comment.
3. Fork PRs without secrets: degrades to no-op with a log line, no failure.
4. Monorepo: `paths` filter scopes analysis; turn/PR intersection computed on changed files only.
5. Never fails a build in v3 (report-only); policy enforcement is a v4 org feature.

**T. Signed entries (D20)**
1. `agentrec key gen|export` manages an Ed25519 keypair (OS keychain where available, file with 0600 elsewhere).
2. With signing enabled, every appended record carries `sig`; `agentrec verify` validates a whole log, reporting the first tampered/unsigned record; a single flipped byte anywhere in a signed record is detected.
3. Verification requires only the public key and the log — no daemon, no network.
4. Mixed logs (signing enabled mid-history) verify with an explicit `unsigned-before <id>` result, not a failure.

**U. Sutra rebase**
1. Sutra's `turns.rs` replaced by the `agentrec-core` dependency; all 166+ existing Rust tests and 288+ TS tests still pass; `.sutra/turns` store migrated to `.agentrec/` by a one-shot migration on first launch (old store archived, per house rule, not deleted).

## 7. v4+ — The business (contract-level criteria)

**V. Attestation export:** deterministic report (same log → byte-identical output) covering AI-authored change %, per-turn provenance, human review trail; verifiable offline against the public key; formats: JSON + PDF.
**W. Org policy:** central config for TTL/scrub/required-review distributed as a signed policy file; repo config may tighten, never loosen, org policy; violations surface in the PR bot (which gains an enforcing mode here).
**X. Dashboard:** self-hostable, reads committed/uploaded logs only, no agent-side telemetry ever; commercial license (D22); checkout via merchant of record; first paying team without a sales call is the ROADMAP Phase 4 gate.

## 8. Escalated to founder (the only genuinely open items — none block v1 start)

**A1. Name availability.** `agentrec` on crates.io / GitHub / npm (for the extension publisher) could not be conclusively verified from this session. Verify before first publish; if taken, D18 needs a new name — nothing else in this plan changes.
**A2. GitHub home.** Personal account vs. fresh org. Default assumption: fresh org (matches the "neutral standard" positioning).
**A3. v4 commercial split.** D22 (open core forever, commercial hosted/org layer, merchant-of-record checkout) is a business-model call made by your VC-hat sparring partner, not a technical default — confirm it's the business you want before v3's signing work, since attestation design leans into it.
**A4. Approval UX evolution.** D23's `agentrec approve` CLI is correct for v2, but if MCP hosts ship native approval UIs for destructive tools, adopting them should take precedence — revisit at v2 kickoff.

## 9. Cross-cutting test strategy

Unit tests live beside code in `agentrec-core` (engine state machine exhaustively table-tested: every boundary-source × grade × op combination). Integration tests drive the real binary against tempdir fixtures — scripted mutation bursts, synthetic signal files, planted secrets, kill -9 harness. The undo torture harness (D36) is the adversarial tier above integration: randomized interleavings, nightly, public, launch-gating. The protocol conformance fixture suite (N1) doubles as the regression net for every consumer. Perf and soak checks run nightly in CI. Every AC in this document maps to at least one automated test except B4/B8 and H++ (nightly) and launch items (WS7), which are checklist-verified.
