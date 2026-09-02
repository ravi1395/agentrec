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
| D45 | *(pre-1.0-freeze honesty fix, 2026-07-25)* **Corrects D35's "wire format is unchanged" claim, which was only ever a two-way split** (I/O-failure vs. everything-else, and only distinguished via `state.json`'s operational `io_failed` path list — not per-entry, and blind to a third real cause, an unreadable file at record time, which always rendered as "over size cap"). `FileEntry` gains `skipped_reason: Option<String>` (PROTOCOL §5, additive open enum: `over_cap` \| `io_failed` \| `unreadable`, `policy` reserved with no producer yet) as permanent per-entry wire truth, landed now specifically because it is materially cheaper before the protocol 1.0 freeze (N1) than after. `state.json`'s `io_failed`/`snapshot_failures` are UNCHANGED and keep their D35 job (operational, aggregate, drive the DEGRADED banner via `--ack-degraded`) — the two channels are deliberately independent (neither is derived from the other) and `build_plan`'s per-entry refusal message now reads only the wire field, never `state.json`. Companion honesty gain: when the recorder actually read the bytes (`over_cap`, `io_failed`) but didn't durably store them, `after` is now set to the real content hash instead of `null` — fixes a permanent modified-since false positive on any never-touched-again skipped file (`unreadable` has no bytes, so `after` correctly stays `null`, never fabricated). The `skipped` gate in `undo`'s `build_plan` is unconditionally checked, and refuses, ABOVE the modified-since comparison — the previously-inert case where a skipped entry's `after` equals the live file's hash is now reachable (thanks to the honesty gain above) and must never fall through to a revert attempt against a blob that was never stored. `D35`'s undo message text ("no snapshot exists (write failed at record time)") is superseded by the shared `fmt::skip_reason_text` vocabulary: `over_cap` → "over size cap", `io_failed` → "write failed at record time", `unreadable` → "file unreadable at record time", absent/unknown → "reason unrecorded". |
| D46 | *(service-leak fix, 2026-07-31)* **Qualifies D40's "init registers and starts the service unit" default, which is wrong under a temp root.** `init` installs a user-scoped, `RunAtLoad`+`KeepAlive` unit; when the root is a temp directory — which exists to be deleted — nothing reaps the unit afterwards, and the machine accumulates one permanently-loaded service per crashed or abandoned test run (measured 2026-07-31: 40 of 41 installed units pointed at deleted roots). Two changes, deliberately asymmetric in risk: (1) PREVENT — under a canonicalized temp prefix (`$TMPDIR`, `/tmp`, `/private/tmp`) `init` skips **only** the service install, prints why, and offers `--service` to force it; canonicalization on both sides is load-bearing on macOS, where `resolve_root` stores `/private/var/folders/…` while `$TMPDIR` reads `/var/folders/…` and a raw prefix compare would never fire. (2) DETECT — `doctor` parses installed unit files (necessary because `slug()` is a one-way hash, so orphans cannot be found by inverting filenames) and reports units whose `--root` has vanished as **advisory**, never `fail`: the unit directory is machine-global and a `fail` would break the exit-0 gate of unrelated repos. Reaping is NOT automated: `service prune` is deferred because it is the only piece that shells out to `launchctl`/`systemctl` (which `service.rs` forbids the automated suite from exercising), it is destructive, and plist removal on this machine is founder-reserved. Doctor prints the exact removal commands instead. A vanished root is reported as vanished, never asserted to be abandoned — an unmounted volume reads identically. |
| D48 | *(red-team T3, 2026-07-31 — implements the second half of D10, unbuilt until now. Numbered D48 on 2026-08-01: D46/D47 belong to `feat/phase-2-0-view-completion`'s service-leak and KeepAlive decisions; commits predating the renumber call this decision D46 in their messages)* **Third sanctioned rewrite class: `purge --signals-consumed`** (after `--memories-retracted` and `--log-duplicates`, in landing order). `signal.jsonl` carries the scrubbed prompt text of every hook fire and nothing ever removed a line — on the dogfood store the inbox reached 13.4 MB / 1932 lines against a 2.5 MB `log.jsonl`, while retention (TTL purge, budget eviction, `--orphans`) covers the object store only. **What it does:** drops the bytes strictly before `state.json`'s `signal_offset`, archives them verbatim to `.agentrec/signal.archived.<ts>.jsonl` (never deletes — house rule), rewrites the file to its unconsumed tail with the same tmp+fsync+rename+dir-fsync idiom as `--log-duplicates`/`--memories-retracted`, and rebases `signal_offset` to 0 in the same operation. **Why safe:** the consumed prefix is redundant, not merely old — the daemon transcribes a signal's prompt into the CAS and cites it from the closed turn's `prompt_ref` at consumption time, and nothing in the codebase ever re-reads the inbox below `signal_offset` (`SignalTailer::poll` and `replay_pending_candidates` both start AT that offset and never read behind it). The offset advances on every consumption; its one non-forward movement is a detected shrink resyncing it *down* to the file's real EOF (below), which reads no consumed bytes — those bytes are already gone from the file — so the redundancy argument is unaffected. **What survives of "append-only":** emitters still only ever append; no line is mutated, reordered, or rewritten in place; only WHOLE already-consumed lines leave the file. What changes is byte offsets, which is exactly why the offset rebase is part of the same operation rather than a follow-up. **Gates and refusals** (each test-proven): refuses while the daemon holds `daemon.lock`; refuses when `state.json` is absent (the only record of what was consumed — *corrected 2026-08-01, re-gate N6, probe-validated: an earlier revision claimed a low guess "replays consumed signals as duplicate turns"; it does not — D7's replay discipline feeds only memory-candidate lines to the engine and drops start/stop in the gap, so a daemon starting without `state.json` mints no turns and silently advances the offset to EOF, after which this command archives-and-drops never-processed signals as if consumed. That silent as-if-consumed loss is the reason for the refusal; a high guess destroys unconsumed ones directly*); refuses on an offset past EOF; refuses on an offset that does not land just after a `\n` (truncating mid-line would decapitate a signal). *Corrected 2026-08-01 (re-gate N1): this entry previously called a mid-line offset proof of a corrupt state.json. False — the shrink resync this same entry documents is a legitimate producer of one: it persists the file's real EOF, which sits mid-line whenever the last line was torn mid-write, and the state is durable across restarts (`len == start` boots are silent) until the next hook fire appends a whole line and consumption advances back onto a boundary. The refusal is correct either way; the diagnosis was not, and its user-facing text steered people toward deleting state.json — which destroys the consumption record and sets up the silent as-if-consumed loss described above (an earlier revision of this very correction called that refusal "permanent"; also false — one record cycle rebuilds state.json at EOF and the retried purge succeeds, which is exactly the loss path).* Offset 0 is a no-op; offset == EOF is a success leaving a **0-byte file, never a deleted one** (`doctor` reads a missing `signal.jsonl` as hooks-disconnected). The tail is copied as raw **bytes**, never lines: it routinely ends mid-line while a hook is appending, and a line-oriented rewrite would append a newline that promotes a partial signal into one the daemon parses as complete. **Ordering is rename-then-rebase, and the direction is load-bearing:** a crash between them leaves a large offset against a short file, which the daemon detects **at startup or mid-run** and reconciles through one shared path (`daemon::resync_shrunk_signal_offset`): loud log, `record_io_failure` so `status`/`doctor` read DEGRADED, and a resync to the real EOF that is **persisted to `state.json`** (unconsumed bytes survive on disk, just unprocessed). *Corrected 2026-08-01 (red-team D1): this entry previously claimed `SignalTailer::poll` alone covered it. It did not — the startup path (`replay_pending_candidates`) returned the file length on `len <= start` without persisting, and daemon boot seeds the tailer from that return, making `poll`'s `len < self.offset` branch unreachable at startup. So a crash in exactly this window was never reconciled by restarting the daemon, this command's refusal remedy ("start `agentrec record` once") was false, the refusal looped with no documented escape, and the unconsumed tail was dropped with no DEGRADED signal at all. The startup branch now calls the same helper; `len == start` (the ordinary fully-consumed boot state) stays silent, guarded by its own test.* The reverse order would leave offset 0 against the full file — *corrected 2026-08-01 (re-gate N6, same probe): not "duplicate turns" as previously claimed — D7's replay discipline drops start/stop at any offset; the actual failure is that the daemon silently advances the offset to EOF across the UNCONSUMED tail, which the next `--signals-consumed` run then archives-and-drops as if consumed. Rename-first makes the crash announce itself (DEGRADED resync); rebase-first loses the tail silently. Announced loss over silent loss is the load-bearing direction.* **Residual, deliberately weaker than `--log-duplicates` and NOT lock-backed:** `signal.jsonl` has two non-daemon writers (`cmds::hook`, the memory-candidate emitter) and neither takes a lock — a `signal.lock` was rejected because the hook path is the latency-critical surface guarded by `hook_recall_hard_wall_deadline`, and covering one writer would buy false closure. A length recheck immediately before the rename aborts the whole operation (with a rerun instruction) on any growth; an append landing in the sub-recheck-to-rename window is lost from the rewritten file and is not in the archive either. Second, smaller residual: truncation refreshes the file's mtime, so `doctor`'s signal-freshness check reads "fresh" for its window after a reclaim — one-shot, cosmetic, unfixed. Companion surface: `status` (text + `--json`) now reports inbox size and the consumed share with this command as the named remedy. **PROTOCOL §3 amended 2026-08-01 (founder-directed):** the bare "append-only" annotation now carries the precise definition — appends only, plus a MAY-reclaim clause for whole consumed lines (archive-first, atomic, tail byte-identical, offset rebased same-op) naming this decision; emitters MUST NOT assume stable byte offsets across a reclaim. |
| D49 | *(red-team T2 guard, 2026-07-31; bare variant added 2026-08-01 by founder decision, closing the residual the earlier round escalated rather than decided)* **`undo` discloses that a turn's file list is an activity window, not an authorship record — a blanket CAUTION line, never a per-file heuristic.** The finding it answers is D6's: while a bracket is open, every mutation in the root folds into that one turn, so a human's own save becomes one of the turn's files and the recorded `after` hash for it IS the human's content — which makes the D30 modified-since rail structurally unable to fire (the file is not modified-since, it is *mis-attributed*), so no post-hoc detector can separate the two. A statement that is always true beats a detector that is sometimes a lie. **Two variants, mutually exclusive, one branch each — never concatenated** (that is what makes non-bleed structural rather than test-dependent in both directions): the **rich** variant says agentrec cannot distinguish the recorded tool's own writes from concurrent human edits made in the same window (D6); the **bare** variant says the turn is an unattributed activity window with no recorded tool and agentrec cannot say who or what made the writes. The bare turn gets its own sentence precisely because the rich one names "the recorded tool" and D6 — printing it on a bare turn would fabricate the attribution the grade exists to withhold. "No recorded tool" is an invariant of every producer on this branch, not an observation: bare closes run only through `Source::Quiet`, whose `OpenTurn` is constructed `tool: None` (`agentrec-core/src/engine.rs`), and crash recovery hard-codes `None` for a bare grade (`cli/src/daemon.rs`); a future producer minting bare-with-tool falsifies the sentence and must change it. The gate is `grade` alone (founder-specified) — the invariant is documented at the function, not defensively re-checked. **Both variants are gated on the plan carrying ≥1 line marked `revert`:** with nothing to be written there is no scope to caution about, and a line printed unconditionally stops being read. **`tool: "agentrec"` turns are excluded** (rich only — the exclusion is moot for bare): their file list is built from a revert plan, i.e. exactly what this process itself wrote, not from a watch window. `tool: "git"` turns are deliberately INCLUDED — a checkout burst is a watch window like any other — which is why the rich wording says "the recorded tool's own writes" rather than "the agent's". **Text is truth-validated against the real plan shape, not written from prose:** the sentence names only files marked `revert`, the one set reverted under every flag combination (`--allow-modified` moves a file INTO that set, never out of it) — an earlier wording said "every file listed above is reverted", which is false on any MIXED plan carrying an `EXCLUDE`/`REFUSE` line, i.e. exactly where a user is most likely to be reading it. Deliberately NOT prefixed `WARNING:` — that token is already the per-file modified-since marker, and conflating the two makes each unreadable as evidence of the other. Printed once from `print_plan`, so preview and `--confirm` share a single emission point by construction. Undo's refusal semantics are untouched: panic mode (D42) still refuses a trailing bare turn outright, so the bare caution appears only on bare turns undo actually offers to revert (explicit turn id). |
| D50 | *(MVP periphery round, 2026-08-01 — red team round 2, findings F1/F2/F6/F7/F8/F9/F11/F13/F19/F24/F26/F27/F28/F31; branch `fix/mvp-periphery`)* **"Safe to install" hardening, ten items, each mapped to tests in its landing commit.** The decisions that bind future work: **(a) Schema-major enforcement (F27)** — `record.rs::SCHEMA_MAJOR = 1`; every wire deserialization site refuses a `v` naming an unimplemented major (PROTOCOL §10 consumer clause added); refused record lines count as `unknown_type_lines` (upgrade signal), never `unparsed_lines` (corruption signal); a refused *signal* is dropped, so the turn degrades to an honestly-unattributed bare turn rather than a confidently wrong rich one. `MemoryRecord.v` stays unenforced (outside PROTOCOL, documented there). **(b) Symlink refusal (F2)** — `FileEntry.link_kind` + `FileEntry.attribution` (see FORMAT-CHANGELOG; `attribution` is declared for `feat/mvp-promise`'s D6 producer and written by nothing on this branch). Undo refuses on `link_kind` presence (any value — refuse-to-act, never refuse-to-parse) OR a live-lstat symlink; neither guard subsumes the other (legacy records carry no `link_kind`; a deleted link cannot be lstat'ed); `--allow-modified` overrides neither. Known residual: a link→regular-file flip inside one turn resolves as the regular file (honest record, test-pinned); closing it needs per-observation plumbing in `engine.rs`. **(c) Budget honesty (F26/F28)** — `over_budget` keys on `retention::managed_bytes` (unique turn-referenced `before`/`after` blob bytes — what eviction can actually consider), proven equivalent to `plan_eviction`'s candidate predicate; NOT a wire change (`RepositoryHealth` is `status --json` operational data, outside FORMAT-CHANGELOG's record.rs scope — the meaning change is recorded here instead). `store:` line shows disk and budgeted bytes; the disk-over-but-nothing-evictable state names `purge --orphans` instead of promising eviction. `config.toml: store_budget_bytes` (bytes, default 2 GiB, 0/unparseable → default; debug-only env seam proven absent from release). **(d) Path-targeted forgetting (F9)** — `purge --path <PATTERN>` archives snapshot blobs referenced ONLY by matching entries, keeps and counts shared blobs, never touches prompts, never rewrites `log.jsonl` — blob archival like `--orphans`, NOT a fourth rewrite class — and deliberately suppresses the default prompt purge (forget-one-file must not archive 90 days of prompts). Composition with other purge flags is refused. Scrub/denylist misses are therefore recoverable (the F6/F7 table edits land with it as one unit). **(e) Prompt-blob purge ordering (F11)** — verb-level liveness gate above every destructive site in `purgecmd::run`; `purge_prompts` archives-before-touch like its siblings. **(f) Disclosure surfaces** — `status` reports all gap kinds + daemon liveness (F13/F31; every cleanly-stopped daemon now honestly shows a trailing-stop gap); launchd units carry `StandardOutPath`/`StandardErrorPath` → `.agentrec/daemon.log` (F24; no rotation, bytes invisible to the store budget — disclosed at the accounting sites); README carries the threat model (F1 — whose claimed §5-predicate suppression mechanism was refuted in source during drafting: blame keys on `gap_stale`/`modified`, not grade); undo-plan and turn renderers sanitize wire strings (F8). Torture harness covers imported turns (F19, riding the nightly `--ignored` gate). |
| D51 | *(Phase 2 tail, Task D0, 2026-08-05 — founder-ratified as a precondition of the Protocol 1.0 freeze, which is a one-way door)* **D7's offline-drop posture is RATIFIED and unchanged; the drop stops being silent.** D7 says a `start`/`stop` signal that arrived while no recorder was running is never fed to the engine at startup — replaying one would mint an empty turn misdated to daemon boot, i.e. fabricate attribution. That stays exactly as it was; `replay_pending_candidates` still routes only memory-candidate lines to `ingest_candidate`, and nothing in this decision mints, replays, or reconstructs a turn. **What the decision fixes is the measured cost of that posture**, recorded in `VERIFY-LEDGER.md` § "RE-RECORDED 2026-08-05" and § "SUPERSEDED 2026-08-05": a tool session whose ENTIRE bracket elapses while the daemon is down leaves `signal_offset` fully consumed, zero turn records, and no trace of any kind — which reads identically to no activity at all. A gate round reproduced this deterministically and (correctly) found no code defect: the mechanism is this documented decision doing its job. Silence, not the drop, was the defect. **Mechanism: a field on the existing epoch record, not a new record type** — `EpochRecord.dropped_signals: u32` (PROTOCOL §5, FORMAT-CHANGELOG "Phase 2 tail D0"), the count of turn-boundary lines the gap scan dropped, threaded from `replay_pending_candidates`'s new `ReplayOutcome` into the `start` epoch `daemon::run` appends immediately afterwards. A new top-level record type was rejected: this lands days before the 1.0 freeze, and one additive integer on a record consumers already parse is a far smaller surface to be stuck with forever than a record type every consumer must learn. **Count only, and that boundary is load-bearing** — no `scrub_prompt`, no `BlobStore` write, no excerpt, nothing derived from the dropped line's payload. Capturing the prompt would re-import exactly the phantom-data problem D7 exists to prevent, one level down: a prompt with no turn, no files, and no `after` hashes is unanchored text presented beside real records. **Predicate is the LIVE routed set, mirrored branch-for-branch** — memory-candidates excluded first and unconditionally, then `is_start() || kind.is_none()`, in the same order as `daemon::run`'s poll loop. *(Review round 1, 2026-08-06, founder ruling WIDEN THE PREDICATE: the field shipped in `deb2f85` counting `kind.is_none()` alone, which diverges from live. `apply_signal` tests `is_start()` BEFORE its `kind` guard, so `{"event":"start","type":"<unknown>"}` opens a real turn while the daemon is up, yet in the gap it was dropped AND uncounted — D51's own silence, reintroduced for exactly one shape. Now counted; pinned by `daemon.rs::replay_gap_drop_count_follows_live_routing` and mutation-probed in both directions.)* The resulting asymmetry is inherited from `apply_signal`'s check order, not invented here: a typed `start` is a boundary, a typed non-`start` is tolerated-unknown (§10, `record_unknown_signal`) and is not. Still NOT "every line the ingest branch skipped", which also holds memory-candidates whenever the `memory_enabled` kill-switch is off, plus any future typed non-`start` `kind`; counting that wider set would make the field overstate lost brackets the moment memory is disabled. The candidate exclusion is therefore **unconditional**, matching the live loop, which routes a candidate away kill-switch either way — a candidate carrying `event: "start"` is a candidate, never a boundary. **One unmodelled divergence, disclosed in the code comment and here rather than left for the next round to find, and it is LIVE rather than theoretical:** the live loop's `handle_emitter_turn_signal` pre-filter can also swallow an untyped start/stop (dedup resend, mismatched stop) before `apply_signal` sees it, so the gap count can OVERSTATE what live would have routed. The Codex hook this branch shipped emits `emitter_turn` on both signals (`hookcmds.rs`, `emitter_turn: Some(turn_id)`; Claude Code payloads still carry none — measured, not assumed: `git grep -ln emitter_turn` finds no other producer). Not modelled because the pre-filter's verdict is a function of `state.json`'s `last_emitter_turn_key` and the open bracket — exactly the engine state D7 refuses to reconstruct at boot. Over-count is the safe direction: the field breaks silence and is normatively a count of drops, never an activity record. **Zero means "counted nothing", never "lost nothing"**: every early bail-out in the scan returns 0 because it parsed no lines, and the `len < start` inbox-shrink branch is the sharp case — bytes were genuinely lost there and cannot be counted, because they were never read. That loss already has a louder, separate channel (`resync_shrunk_signal_offset`: stderr + `record_io_failure` → DEGRADED), and this field deliberately does not attempt to speak for it. **Wire compatibility:** omitted when 0, so every epoch line ever written is byte-identical (pinned at both the type layer and the `append_epoch` layer). `stop` epochs always pass 0 because a clean shutdown scans no gap — and that call site is **not test-pinned**, stated explicitly rather than implied: every daemon integration test SIGKILLs by design, so the clean-shutdown `stop` epoch is never written under test, and re-pointing that call at the replay count leaves the whole suite green (measured, not assumed). **Deliberately not built:** no reader consumes the field yet — `view::recording_gaps` and `GapKind` are untouched, and `blame`'s "attribution stale — recording gap" text is unchanged, so every human-form golden stays byte-identical under the P5 ratchet. A "(N tool signals dropped while offline)" clause on that message is the obvious next consumer and is a separate, non-freeze-critical change. |

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

**v0.4.1 amendment — skip-reason honesty (D45, 2026-07-25; corrects M+'s message text, adds a wire field):**

**SR1. Additive wire field:** `FileEntry.skipped_reason: Option<String>` — a record without the field deserializes fine (golden round-trip test against pre-field JSON), and a `None`-reason entry serializes byte-identical to the pre-D45 wire shape (no `skipped_reason` key at all).
**SR2. Three causes, five producer sites** *(count corrected 2026-07-25 — the original text said "three honest producers" while naming four sites in the same sentence, and omitted a fifth entirely: `Recorder::stage`'s symlink `read_link` failure arm, which silently left `skipped_reason: None` — "reason unrecorded" — for the same `unreadable` cause a regular file's `fs::read` failure already named honestly)*: `Recorder::stage` sets `skipped_reason` at five call sites collapsing to three genuinely different causes — `over_cap` (the pre-check `bytes.len() > MAX_SNAPSHOT_BYTES` branch, and `PutResult::OverCap`, kept even though currently unreachable given the pre-check), `io_failed` (`PutResult::IoError`), and `unreadable` (a regular file's `fs::read` failure, AND a symlink's `read_link` failure — the latter factored into the standalone `symlink_change` helper specifically so its TOCTOU-race failure arm is deterministically unit-testable). Asserted at the `stage`/`resolve` level with real filesystem fixtures (an actual >10 MiB file for `over_cap`, a chmod-0000 file for `unreadable`, a locked object-store parent dir for `io_failed`) for every cause except the symlink race, which cannot be reliably won against a real filesystem in a test and is instead asserted directly against `symlink_change` with a synthetic `read_link` error — not synthetic `FileEntry`/`ChangeObs` structs.
**SR3. `undo` names the real cause:** an `io_failed`-skipped entry's refusal says "write failed at record time", never "over size cap" — driven solely by the wire field, proven independent of `state.json`'s `io_failed` list (the test seeds a *different* path into that list).
**SR4. Honest read-side vocabulary, one shared helper (`fmt::skip_reason_text`):** `diff`'s per-file notice reads "(content not snapshotted — over size cap|write failed at record time|file unreadable at record time|reason unrecorded)" per the real cause; `undo`'s refusal renders the same fact in the same em-dash form, "content not snapshotted — &lt;reason&gt;" *(spelling unified 2026-07-25 at the done-gate — the two renderers initially disagreed, `diff` using an em-dash and `undo` parentheses; that is the D-PD6 drift class, so they are now one shared form fed by `fmt::skip_reason_text`)*. A present-but-unresolvable blob (hash recorded, `store.get` fails) reads "(snapshot unavailable)" with no asserted cause — the pre-D45 "purged or missing" suffix is gone because the true cause is genuinely unknown at that point. `undo`'s `StoreError::Missing` refusal on a `before` hash reads "prior snapshot unavailable — refusing to restore" (was conflated with the "before is absent entirely" case). **`diff`'s corrupt rendering DID change** *(corrected 2026-07-25 — an earlier draft of this line claimed the corrupt/hash-mismatch message was untouched, which was false for `diff`)*: `print_entry` previously collapsed `Missing` and `Corrupt` into one notice, leaving `diff` strictly less specific than `undo` about the same condition, and now renders "(snapshot corrupt — hash mismatch)" distinctly. `undo`'s own "prior snapshot corrupt (hash mismatch) — refusing to restore" is genuinely untouched.
**SR5. `after`-hash honesty gain:** when the recorder actually read the bytes (`over_cap`, `io_failed`) but didn't durably store them, `after` is now `Some(<hash>)` instead of `null` — proven RED-before-GREEN: an unmodified over-cap file previously satisfied modified-since forever (`after: null` vs. any real current hash); it now correctly does not.
**SR6. Skip gate ordering pinned:** a `skipped` entry whose `after` now happens to match the live file's hash (only reachable because of SR5) is still unconditionally REFUSED in `build_plan`, never routed into the revert path — regression-tested at the CLI level (file byte-identical after `undo --confirm`, no turn recorded).
**SR7. `state.json` independence (SR-E):** `io_failed`/`snapshot_failures` keep their D35 job unchanged (operational, aggregate, DEGRADED banner, `--ack-degraded`); `build_plan` no longer reads `state.json` at all for the per-entry message.
**Fixture debt, honestly owed:** N1's protocol conformance fixture suite does not exist yet (unbuilt, tracked below) — `skipped_reason` ships with a golden serde round-trip test (SR1) as the stand-in and is owed a real fixture entry at the N1 freeze, same as every other v1 wire field.

**v0.4.2 amendment — activity-window disclosure on `undo` (D49, 2026-07-31 / 2026-08-01; additive — no existing criterion changes):** every criterion below maps to a test in `cli/tests/misattribution.rs`, which drives the real binary (and, for AC-CAUTION-1, the real daemon).

**AC-CAUTION-1. Rich variant, both paths:** an end-to-end fold — `UserPromptSubmit` → agent write → human write to a *different* file → `Stop` — yields ONE rich turn attributed to the agent, and `undo` of it prints the rich CAUTION and its `revert`-scoped clause in the preview AND again under `--confirm`; the human's file is reverted with no `EXCLUDE` and no `REFUSE` (pinned as D6's documented data loss, which is why the line exists). Test: `intra_bracket_human_edit_folds_into_agent_turn_and_undo_reverts_it`.
**AC-CAUTION-2. Truth on a mixed plan:** on a plan carrying one `revert` and one `EXCLUDE`, the caution is present, the pre-fix overclaim ("every file listed above is reverted") is absent, and the `revert`-scoped clause is present — the assertion that makes "always true" checkable rather than asserted. Test: `mixed_plan_caution_scopes_itself_to_reverted_files_only`.
**AC-CAUTION-3. `agentrec` exclusion (negative direction, so AC-CAUTION-1 is not vacuous):** a `tool: "agentrec"` turn whose plan does contain a `revert` still prints its plan and prints NO caution. Test: `agentrec_own_turn_carries_no_window_caution`.
**AC-CAUTION-4. Bare variant, both paths, no variant bleed:** a `grade: "bare"` turn whose plan contains a real `revert` prints the bare caution and its scope clause in preview AND under `--confirm`, while the rich variant's recorded-tool/D6 clause is ABSENT in both — and the file really is reverted, so the confirm-path assertion is not vacuous. The reverse direction (bare text on a rich turn) is closed by construction — `window_caution` returns one branch or the other and never concatenates — not by a test. Test: `bare_turn_undo_carries_unattributed_window_caution` *(flipped 2026-08-01 from `bare_turn_undo_carries_no_window_caution`, which pinned the pre-decision exclusion; the founder decision reversed the behavior, so the pin reversed with it rather than being deleted)*.

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

**S. Service-unit leak: temp-root guard + orphan detection (D46)**

Measured trigger (2026-07-31, this machine): 41 `com.agentrec.*` launchd units
installed, **40 of them pointing at a root that no longer exists** — 37 under the
canonicalized `$TMPDIR` (`/private/var/folders/…/T/`), 3 under session scratchpads
(`/private/tmp/claude-501/…`). Every one carries `RunAtLoad` + `KeepAlive`.
`uninstall` reverses `init` correctly; the leak is entirely *init without a
matching uninstall*, i.e. any crashed or abandoned test run.

1. **Temp-root default.** `init` on a root whose canonicalized path is under a
   temp prefix (canonicalized `$TMPDIR`, `/tmp`, `/private/tmp`) skips the service
   install by default and prints a line naming both the reason and the `--service`
   override. Every other init step (scaffold, config, gitignore, perms, hooks) is
   unaffected.
2. **The decision is a pure function, separately testable.** `init`'s service
   branch consults one `service_decision(root, no_service, force)` returning
   `Install` | `SkipFlag` | `SkipTemp`; the effectful `service::install` is called
   only on `Install`. The full flag×path matrix is asserted against that function,
   never by executing a real `launchctl`/`systemctl` install.
3. **`--service` forces install** under a temp root; `--no-service` and `--service`
   together is a CLI-level conflict error, not a silent precedence rule.
4. **Non-temp roots are unaffected**: an ordinary repo path with no override yields
   `Install`. (Regression rail against an over-broad temp predicate.)
5. **Unit roots are recoverable.** `service::parse_unit_root` returns the exact
   `--root` from both generated unit forms, inverting `xml_escape` (launchd) and
   `systemd_escape` (systemd) — including roots containing `&`, spaces, `%`, and
   quotes. Required because `slug()` is a one-way hash: orphans cannot be found by
   inverting filenames, only by parsing unit contents.
6. **Scanning classifies into three disjoint buckets.** `service::scan_units(dir)`
   matches only agentrec-named units (`com.agentrec.*.plist` / `agentrec-*.service`),
   ignores unrelated files in the same directory, and reports each as *live*,
   *vanished-root*, or *unparseable*. An unparseable unit is never counted as an
   orphan.
7. **`doctor` reports vanished-root units as ADVISORY, never `fail`.** The check
   ("orphaned services") renders `pass` and carries a note with the count, one
   example unit, and the exact two-command removal pair. `fail` is forbidden here:
   `~/Library/LaunchAgents` is machine-global, so failing would break the exit-0
   deploy gate of every unrelated repo on the same machine. With no vanished-root
   units the check is a plain `pass` with no note. The note must not assert the
   directory was abandoned — an unmounted volume reads identically.
8. **Report shape is path-independent**: the new check name appears in
   `diagnose`'s uninitialized-repo `n/a` short-circuit list, so an uninitialized
   repo yields the same check set as an initialized one.
9. **Real-corpus evidence, not fixture-only** (ledger row, not a unit test): the
   parser is run read-only over this machine's 41 installed plists and must report
   41/41 parsed and 40 vanished — matching the independently recorded 40/41. A
   round-trip test against our own writer cannot establish this.

**Deferred deliberately — `service prune` is NOT built here**, for three facts,
not a preference: (a) it is the only piece of this work that would shell out to
`launchctl`/`systemctl`, which `cli/src/service.rs`'s module contract forbids
exercising from the automated suite; (b) it is destructive; (c) removing plists
from this machine is an open founder-pending item explicitly reserved to the
founder. `doctor`'s remedy string therefore prints the exact runnable pair
(`launchctl bootout gui/$(id -u)/<label>` then `rm <plist>`), closing the
user-facing problem with zero untestable code.

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
1. `agentrec init --codex` merges three hooks — `UserPromptSubmit`, `PostToolUse` (matcher `apply_patch`), and `Stop` — into Codex's hook config (`hooks.json` or `[hooks]` in `config.toml`, whichever exists; `hooks.json` created if neither), idempotently, preserving unrelated hooks; malformed existing config aborts untouched (mirror of A5).
2. `UserPromptSubmit` emits the standard start signal (`tool: "codex"`, rollout path as `transcript`). `PostToolUse` (`apply_patch`) emits no signal — it accumulates the turn's touched file paths, parsed from the patch-DSL `tool_input.command` (decision 17), into a per-turn scratch file keyed on Codex's own `turn_id`. `Stop` emits the stop signal, carrying the accumulated `files_written` list and `emitter_turn` (that same `turn_id` — confirmed live-stable across a `decision:"block"` continuation, Phase A spike).
3. Prompt comes directly from the `UserPromptSubmit` payload's `prompt` field (confirmed live — Codex furnishes it on the hook payload itself; no rollout-file parsing is needed on the live-hook path, unlike Claude Code's transcript-based extraction). This is D7's primary-signal case, not its transcript-extraction fallback; scrubbed.
4. `import codex` from `~/.codex/sessions/**/rollout-*.jsonl` with the full K-series AC applied (cwd mapping, idempotency, malformed-line tolerance, streaming, scrub).
5. Neutrality proof test: one live session each of Claude Code and Codex on the same repo yields one `log.jsonl` with both tools attributed correctly and zero cross-attribution. *(Met live 2026-08-05, both legs; evidence: `docs/verify/o5-two-tool-session.md`, `VERIFY-LEDGER.md` § "O5". Sequential sessions, not simultaneous — disclosed there.)*

### P. MCP server (`agentrec mcp`)
1. stdio transport (D24); `tools/list` exposes the five read tools — `agentrec_log`, `agentrec_diff`, `agentrec_blame`, `agentrec_recall`, `agentrec_status` — always, `agentrec_undo` only when `mcp_destructive != "off"`. *(Amended 2026-08-06 at plan exit: the original three-tool list predated Phase E shipping recall + status; §8's tool table is the normative surface.)*
2. Read tools return structured JSON; `diff`/`blame` payloads are byte-equal to their `--json` output (parity-pinned), while `log` returns the typed `Page<TurnSummary>` (P4b decision 12 — not `log --json`'s shape) and `recall` returns the whole `RecallPage` (delta decision 14 — `recall --json` stays a bare hit array). Results over 200 turns / 2,000 diff lines paginate with a cursor — no unbounded payloads into agent context. *(Amended 2026-08-06: "mirroring `--json` output" was written before the typed-page decisions; the parity tests in `cli/tests/mcp.rs` pin which tools are byte-equal and which are typed.)*
3. Works with the daemon stopped (reads are file-based); says so in a `status` field rather than erroring.
4. Mode matrix — `off`: undo absent from tools/list; calls to it error. `confirm`: undo call registers a pending request and returns `pending` + request id; `agentrec approve` lists/approves/denies; approval executes and returns the result on the agent's next poll; requests expire in 10 min (D23); denied and expired requests return distinct statuses. `auto`: two-phase — call 1 returns preview (per-file diff summary, warnings, `confirm_token`); call 2 with the token executes; token is single-use, 60 s TTL; wrong, reused, or expired tokens error without side effects; preview-to-execution drift (files changed between calls) aborts with a fresh-preview instruction.
5. In every mode: `skipped` and `withheld` files refused; modified-since files excluded unless `allow_modified: true` AND (mode `auto` or human approval) — the D30 predicate, never the display-level one; executed undos recorded as turns (H6 semantics).
6. Concurrency: two simultaneous undo requests on overlapping files — second is rejected with a conflict error; no partial interleaved reverts.
7. Prompt-injection stance tested: tool descriptions contain no instructions that could be construed as commands to the agent; tool outputs are data-only (no imperative text) — reviewed against the current MCP security guidance at build time. *(Stance unchanged. **Reviewed 2026-08-06** against the shipped 2.2 surface, Phase E exit; findings — including two the stance does not currently cover — recorded in `VERIFY-LEDGER.md` § "Task E4 … P.7 review". This line records the review date only; no clause here was rewritten.)*

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

## §attest — acceptance criteria

Acceptance criteria for the `attest` subsystem (spec:
`docs/superpowers/specs/2026-08-28-attest-design.md`; plan:
`docs/superpowers/plans/2026-08-28-attest-plan.md`). ACs are appended here **per
phase, before that phase's code lands** (house rule), each numbered
`AC-ATTEST-P<phase>-<n>` and each mapped to at least one automated test or, where
no runnable check exists, marked manual with the reason. Formats introduced by
this subsystem live in `ATTEST-FORMAT.md` marked unstable — `PROTOCOL.md` is
untouched (spec decision 9).

### Phase 1 — spike (exit criteria 1–7)

Evidence: `docs/verify/attest-coverage-spike.md` (Probe A) and
`docs/verify/attest-output-channel-spike.md` (Probe B). Suite baseline at the
branch fork, uninstrumented, `cargo test --workspace -- --test-threads=3`:
**1035 passed / 0 failed / 4 ignored, 207.84 s**.

**AC-ATTEST-P1-1.** Wall-time/bytes table for Probe A (a) suite-level and (b)
per-test, both measured, extrapolations labeled. — **MET**: (a) measured;
(b) 50-test sample measured (24 of its 25 integration samples executed; one is
`#[ignore]`d), full-suite projection labeled EXTRAPOLATION (Probe A §2–§4).
Riders that must travel with the figures: the instrumented-run overhead of
+0.5% (208.96 s vs 207.84 s) was measured without re-verifying machine
quietness immediately before the instrumented run; the per-test range of
~2–13 min is a median-based lower bound and a mean-based upper bound, the
latter dominated by a single 21.5 s test.
**AC-ATTEST-P1-2.** Probe A (d) answered as a plain yes/no with pasted evidence:
does a spawned `agentrec` child's `cli/src` execution appear in the spawning
test's map? — **MET**: YES for children exiting normally, NO for children killed
by SIGKILL (Probe A §5).
**AC-ATTEST-P1-3.** Written ruling on per-test vs suite-level coverage. —
**MET (founder-ruled 2026-09-01):** the Probe A §7 recommendation is adopted as
written — per-test coverage at FILE granularity; any test whose spawned child was
killed (or whose coverage is otherwise known-incomplete) is staled on any write
under `cli/src/**`; suite-level remains the sanctioned degraded fallback (spec
decision 5). Detection of the two under-attribution channels (SIGKILLed child,
untemplated-child profraw leak) is a Phase 4 contract obligation: until both are
detected, EVERY binary-spawning test is staled on any `cli/src/**` write (the
coarser rule Probe A §7 offers as the honest interim).
**AC-ATTEST-P1-4.** Probe B parser validated against real captured libtest
output, including a corrupted-fixture fail-closed proof. — **MET**:
`docs/verify/attest-output-channel-spike.md` § "Result" — the ratified parser
resolves all five staged-pipeline states across six fixtures with per-test
tallies matching each run's own summary line, and fails closed
(`parse_failed`, raw blob retained) on the deliberately corrupted fixture.
**AC-ATTEST-P1-5.** Schema sketch of the chosen coverage map for Phase 4's
consumer. — **MET** (Probe A §6): per-test identity → covered file set, file
granularity, with a child-killed scope flag.
**AC-ATTEST-P1-6.** This §attest section exists
(`grep -n '^#.*[Aa]ttest' IMPLEMENTATION.md` non-empty). — **MET**.
**AC-ATTEST-P1-7.** Founder sign-off on ruling 3 recorded before Phase 2
dispatch begins. — **MET:** founder ruled 2026-09-01 in session, adopting the
recommendation; the manual claim `clm_2D3J0S3FNM6VSQX367GGN1FTQZ` covering
Probe A was attested by the founder. Phase 2 dispatched the same day.

### Phase 2 — claim core: events, fold, derive (pure, no execution)

Scope: `agentrec-core/src/attest.rs` + `attest/{types,events,fold}.rs`,
`agentrec-core/src/lib.rs`, `ATTEST-FORMAT.md`. Pure: no I/O, no process
execution, no file paths in core. Tests are unit tests inside the module
(`cargo test -p agentrec-core attest::`).

**AC-ATTEST-P2-1.** Fold of `[derive, evidence, stale, verdict(confirmed)]` ends
`CONFIRMED` with the `STALE` overlay dropped. — `fold::tests::
ac1_derive_evidence_stale_confirmed_ends_confirmed_and_unstale`
**AC-ATTEST-P2-2.** `verdict(recipe-invalid)` then `verdict(confirmed)` ends
`CONFIRMED` (recipe-invalid proven retryable, not terminal). — `fold::tests::
ac2_recipe_invalid_then_confirmed_ends_confirmed`
**AC-ATTEST-P2-3.** An author-run `evidence` event alone can never reach
`CONFIRMED`; only a `verdict{confirmed}` can. — `fold::tests::
ac3_author_evidence_alone_never_reaches_confirmed`
**AC-ATTEST-P2-4.** A `Derive` carrying `renamed_from: Some(old)` plus the old
claim's `ClaimId` continues that claim under the SAME `ClaimId` — identity
remapped, zero prior evidence/verdicts lost. — `fold::tests::
ac4_rename_continues_same_claim_id_losing_no_history`
**AC-ATTEST-P2-5.** `claim-false` is permanent: a later `verdict{confirmed}`
does not move the state (it is counted in history only). — `fold::tests::
ac5_claim_false_is_permanent_against_a_later_confirmed_verdict`
**AC-ATTEST-P2-6.** The cargo target is part of `TestIdentity`'s key: two
identities differing only in the target component are distinct as map keys, so
two same-named tests in different targets cannot cross-attribute. — `fold::
tests::ac6_same_fn_name_in_two_targets_folds_to_two_claims` (the discriminating
half is the `BTreeSet` assertion; the two-claims half is illustrative, since
the fixture's two explicit `ClaimId`s would fold apart regardless)
**AC-ATTEST-P2-7.** Every event kind survives a JSONL round-trip value-equal,
and a line carrying an unknown `kind` is tolerated and counted (`ParsedAttestLine
::UnknownKind`), never fatal to the log. — `events::tests::
ac7_every_event_kind_round_trips_value_equal` and `events::tests::
ac7_unknown_kind_line_is_tolerated_and_counted`
**AC-ATTEST-P2-8.** The `FLAKY` state is produced only by a
`verdict{flaky-observation}`, and it is a non-blocking state flag (the state
carries `blocks_gate() == false`). — `fold::tests::
ac8_flaky_state_comes_only_from_flaky_observation_and_never_blocks`

**AC-ATTEST-P2-9.** `claim-false` permanence holds against EVERY later event
kind, not only later verdicts: a `human`, `manual-declare`, `evidence` or
re-`derive` after a `claim-false` verdict leaves the status `CLAIM_FALSE` and
`blocks_gate()` true, each counted in history. — `fold::tests::
ac9_claim_false_survives_a_later_human_answer_and_manual_declare`
**AC-ATTEST-P2-10.** A `manual-declare` landing on an already-established claim
records its text and severity without moving the status (a hand-authored event
never demotes a machine verdict); it sets `DECLARED` only on a fresh claim id. —
`fold::tests::ac10_manual_declare_on_an_existing_claim_records_without_moving_status`
**AC-ATTEST-P2-11.** The `test_identity → ClaimId` index is returned to callers
(`FoldResult::claim_for`, Phase 3's rename-detection lookup) and tracks the LIVE
identity: after a rename the old identity resolves to nothing and the new one to
the same `ClaimId`. — `fold::tests::
ac11_by_identity_tracks_the_live_identity_across_a_rename`
**AC-ATTEST-P2-12.** A later `derive` carrying `body_hash: None` does not erase a
hash a previous `derive` recorded. — `fold::tests::
ac12_a_later_derive_without_a_body_hash_does_not_erase_the_recorded_one`
**AC-ATTEST-P2-13.** An `evidence` event arriving before any `derive` populates
`test_identity` from its `StructuredResult` and indexes it; a claim seen only
through identity-less kinds keeps `test_identity: None`. — `fold::tests::
ac13_evidence_before_any_derive_still_carries_the_test_identity`
**AC-ATTEST-P2-14.** Every `AttestEvent` variant's serialized `kind` string is
present in `events::KNOWN_EVENT_KINDS`, so a seventh kind cannot drift into
being misreported as a newer schema. — `events::tests::
ac14_every_variants_kind_string_is_in_the_known_list`

**AC-ATTEST-P2-15.** A `derive` never regresses an established status: it sets
`DERIVED` only on first sight of the claim id, and updates identity, body hash
and the index on every later occurrence. Verified for `[evidence, derive]`,
`[verdict(confirmed), derive]` with and without a prior derive,
`[manual-declare, derive]`, and `[evidence(x), derive(y, renamed_from: x)]`. —
`fold::tests::ac15_a_later_derive_never_regresses_an_established_status`
**AC-ATTEST-P2-16.** A refuted claim is never left stale: `stale` on a
`CLAIM_FALSE` claim is counted but sets no overlay, and a claim that is stale
when it becomes `CLAIM_FALSE` has the overlay dropped — otherwise the overlay
could never clear and Phase 4 would re-verify a permanent refutation forever. —
`fold::tests::ac16_a_refuted_claim_is_never_left_stale`
**AC-ATTEST-P2-17.** When two claims declare one identity (a writer bug), the
index resolves it to the LATEST declarer and the earlier claim keeps the
identity it was given — the fold evicts nothing, having no basis to decide which
writer was wrong. — `fold::tests::
ac17_two_claims_declaring_one_identity_index_the_latest_writer`
**AC-ATTEST-P2-18.** A malformed `body_hash` or `claim_id` makes its LINE
`Unparsed` and never panics — including a 64-BYTE string holding multibyte
characters (byte-slicing a char boundary would take down every reader), a signed
`"+1"`×32, uppercase hex, and wrong lengths; only 64 lowercase hex characters
parse. — `events::tests::
b1_a_malformed_body_hash_makes_the_line_unparsed_and_never_panics`,
`events::tests::b1_a_malformed_claim_id_makes_the_line_unparsed`,
`types::tests::claim_ids_are_shape_checked_on_the_way_in`
**AC-ATTEST-P2-19.** A `flaky-observation` verdict leaves the `STALE` overlay
set (the mirror of the recipe-invalid case). — `fold::tests::
a_flaky_observation_leaves_the_stale_overlay_set`

**Signature note for Phase 3:** `fold_claims` returns `FoldResult { claims,
by_identity }`, not a bare `BTreeMap<ClaimId, ClaimState>` (AC-ATTEST-P2-11).
States are `result.claims`; the rename lookup is `result.claim_for(&identity)`.

The `STALE` overlay rule is stated once, in `ATTEST-FORMAT.md` § "The `STALE`
overlay" — it narrows the spec's Architecture line (amended in place there too)
and is what AC-ATTEST-P2-1, the recipe-invalid/flaky overlay tests and
AC-ATTEST-P2-16 verify.

### Phase 3A — attest append lock + `attest status` + fixture crate

Scope: `cli/src/attest.rs`, `cli/src/attest/{lock,statuscmd}.rs`,
`cli/src/main.rs` (hidden `attest status` subcommand), `Cargo.toml`
(`[workspace] exclude`), `.gitignore`, `cli/tests/fixtures/attest_sample_crate/`,
`cli/tests/attest_status.rs`.

**AC numbering, reserved:** `AC-ATTEST-P3-1..9` belong to chunk A (this block).
Chunk B (`adapter_cargo.rs`, `capture.rs`, `attest derive`/`attest run`) appends
from `AC-ATTEST-P3-10` onward.

**AC-ATTEST-P3-1.** `agentrec attest status` on a freshly `init`ed root (no
`attest.jsonl` at all) prints every count as zero and exits 0 — a missing log is
an empty log, never an error. — `attest_status::
ac_p3_1_status_on_a_fresh_root_is_all_zeros_and_exits_zero`
**AC-ATTEST-P3-2.** Claim counts are rendered per `ClaimStatus`, one status per
line in a fixed order, and match the fold of the log — including the `STALE`
overlay count, which is an overlay and not a status. — `attest_status::
ac_p3_2_counts_by_status_and_stale_overlay_match_the_folded_log`
**AC-ATTEST-P3-3.** `--json` emits one object carrying the same numbers as the
text form; every text figure has a JSON key. — `attest_status::
ac_p3_3_json_carries_the_same_numbers_as_the_text_form`
**AC-ATTEST-P3-4.** A malformed line and an unknown-`kind` line are each
surfaced with their own count and neither is fatal: the surrounding events still
fold and the exit code is 0. — `attest_status::
ac_p3_4_malformed_and_unknown_kind_lines_are_surfaced_and_not_fatal`
**AC-ATTEST-P3-5.** Concurrent appenders through `append_attest_locked`
(N threads × K events) leave exactly N×K parseable lines and zero unparsed
lines, and an append WAITS while another writer holds the lock — the Phase 4
concurrent-append AC, landed early with the lock. — `attest::lock::tests::
ac_p3_5_concurrent_appenders_leave_no_torn_lines_phase4_concurrent_append_ac`
and `attest::lock::tests::
an_append_blocks_while_another_writer_holds_the_lock`. **Rider, measured:** the
concurrent-append test is NOT discriminating on macOS/APFS — an
acquire-then-immediately-release neuter leaves it green at 256 KiB, 2 MiB and
32 MiB batches, because `write_all` to an `O_APPEND` fd did not short-write
and interleave at any size tried. The blocking test is the discriminating
half: an isolated neuter (`libc::flock` replaced by `let rc = 0`, lock file
still opened) reds it alone, 1 failed / 5 passed. Both tests are in-crate unit
tests, not `cli/tests/attest_status.rs`: `append_attest_locked` lives in the
`agentrec` binary crate, which has no lib target for an integration test to
link against.
**AC-ATTEST-P3-6.** Every `attest.jsonl` writer takes the append lock, the
daemon included — `loglock.rs`'s daemon exemption is deliberately NOT copied,
because for `attest.jsonl` the daemon is one routine writer among several
(it appends `stale`). — **Documented contract**, stated in `cli/src/attest/
lock.rs`'s module doc; `append_attest_locked` is the single choke point that
makes it structural. No automated check exists in chunk A because the daemon's
`stale` writer does not exist until Phase 4; Phase 4 wires it and owns the
enforcing test. Marked manual for that reason. **CLOSED by Phase 4B: the
daemon's `stale` writer now goes through `append_attest_locked` and is
exercised against concurrent CLI appenders by
`attest_stale::ac_p4b_7_concurrent_cli_appends_leave_no_torn_line`** — no
longer manual.
**AC-ATTEST-P3-7.** The fixture crate is excluded from the parent workspace:
`cargo metadata --no-deps` at the workspace root does not list the
`attest_sample_crate` package. — `attest_status::
ac_p3_7_fixture_crate_is_absent_from_the_parent_workspace`
**AC-ATTEST-P3-8.** `attest status` reports the dev-loop-only figure: the number
of claims whose LATEST `evidence` event carries `dirty: true`. Counted from the
event stream in append order, because `ClaimState` carries no dirty field. —
`attest_status::ac_p3_8_dirty_latest_evidence_is_counted_as_dev_loop_only`
**AC-ATTEST-P3-9.** The fixture crate exercises BOTH of Phase 2's
location-resolution shapes — a `tests/integration.rs` integration target and a
`#[cfg(test)] mod tests` inside `src/lib.rs`, each with ≥2 `#[test]` fns, plus
one `#[ignore]`d test — and builds standalone. — `attest_status::
ac_p3_9_fixture_crate_has_both_test_target_shapes`

### Phase 3B — cargo adapter + passive evidence capture

Scope: `cli/src/attest/{adapter_cargo,capture,derivecmd}.rs`, `cli/src/attest.rs`
(module list), `cli/src/main.rs` (`attest derive` / `attest run`), `cli/src/cmds.rs`
(the `hook claude` `PostToolUse` arm + the eviction protect set), `cli/src/
purgecmd.rs` (the `--orphans` / `--path` protect sets), `cli/src/initcmd.rs`
(`CLAUDE_HOOK_EVENTS`), `cli/src/uninstallcmd.rs`, `cli/src/doctorcmd.rs`,
`cli/Cargo.toml` (`syn`/`proc-macro2`/`quote`), `cli/tests/attest_capture.rs`.

**Phase 4 census hook:** every `std::process::Command::new` site this block adds
carries a `// attest: sanctioned spawn (Phase 4 census)` comment, so Phase 4's
`clippy.toml disallowed-methods` entry for `Command::new` has an enumerable
allow-set rather than a re-discovery problem.

**AC-ATTEST-P3-10.** `attest derive` on the fixture crate creates exactly one
claim per test libtest itself enumerates — including the `#[cfg(test)] mod tests`
unit tests inside `src/lib.rs`, the `tests/integration.rs` target's tests, and the
`#[ignore]`d test (an ignored test is a claim; it is `recipe-invalid` at verify
time, not absent at derive time). — `attest_capture::
ac_p3_10_derive_creates_one_claim_per_discovered_test`
**AC-ATTEST-P3-11.** A second `attest derive` with nothing changed appends
nothing: `attest.jsonl` is byte-identical before and after. — `attest_capture::
ac_p3_11_rederive_with_no_change_appends_nothing`
**AC-ATTEST-P3-12.** Renaming a test fn with its body unchanged makes the next
`attest derive` reuse the OLD `ClaimId` and write `renamed_from` = the old
identity; the claim count does not grow. — `attest_capture::
ac_p3_12_rename_with_an_unchanged_body_reuses_the_claim_id`
**AC-ATTEST-P3-13.** Renaming a test fn AND changing its body mints a NEW
`ClaimId` with no `renamed_from` — a body hash is the only rename evidence this
adapter has, so without it the two tests are two claims. — `attest_capture::
ac_p3_13_rename_with_a_changed_body_mints_a_new_claim`
**AC-ATTEST-P3-14.** Changing a known test's body without renaming it appends a
`derive` carrying the SAME `claim_id`, the new `body_hash`, and no
`renamed_from`. — `attest_capture::
ac_p3_14_a_body_change_without_a_rename_rehashes_in_place`
**AC-ATTEST-P3-15.** `attest run -- cargo test` inside a hook-bracketed session
writes one `evidence` event per known test, each carrying the open turn's id read
from `.agentrec/open.json`. — `attest_capture::
ac_p3_15_run_writes_evidence_joined_to_the_open_turn`
**AC-ATTEST-P3-16.** The same `cargo test` arriving through the hook path — a
`PostToolUse` `Bash` payload piped to `agentrec hook claude` — writes evidence of
the same shape, with no extra ceremony step. — `attest_capture::
ac_p3_16_hook_post_tool_use_bash_writes_the_same_evidence`
**AC-ATTEST-P3-17.** Evidence captured against a dirty working tree carries
`dirty: true` and is counted by `attest status` as dev-loop-only. — `attest_capture::
ac_p3_17_dirty_tree_evidence_renders_as_dev_loop_only`
**AC-ATTEST-P3-18.** A result for a test no `derive` knows is COUNTED on stderr
and no event is written for it — evidence is never attached to a claim that does
not exist. — `attest_capture::
ac_p3_18_undeclared_results_are_counted_on_stderr_and_not_written`
**AC-ATTEST-P3-19.** The ratified libtest parser reproduces, for every fixture in
`docs/fixtures/attest/`, the state `docs/verify/attest-output-channel-spike.md`
records for it — asserted as the full `(outcome, recipe_invalid, parse_failed)`
triple, not just the one field a state name mentions. — `attest::adapter_cargo::
tests::ac_p3_19_every_spike_fixture_parses_to_its_documented_state`
**AC-ATTEST-P3-20.** Unparseable output fails closed, in two shapes, and the
raw output survives both.
(a) A section that DOES name tests (per-test lines present, summary missing or
mismatched): no per-test outcome is trusted, one `evidence` event per named test
carries `parse_failed: true`, and the full raw output is retained in the CAS and
referenced by the event. — `attest_capture::
ac_p3_20_unparseable_output_writes_parse_failed_evidence_with_the_raw_blob`
(b) A section that names NOTHING — the harness died before libtest wrote a
single result line (`process::abort`, SIGABRT): no event is written, because no
test was named and attributing the run to a claim it never mentioned would be
fabrication. The section is COUNTED and the raw output is retained: stderr
carries `N section(s) unparseable (harness crash — no test named), raw output
retained at sha256:…`. — `attest_capture::
ac_p3_20_a_harness_crash_retains_its_raw_output_and_is_counted`
**Two consequences, stated rather than hidden.** When every section is
unattributable the retained blob is cited by no event, so `purge --orphans` may
later archive it — retention here is durable only until a reclaim runs.
And the count is gated on the capture looking like libtest at all (a `running N
tests` header), so `attest run -- <any non-test command>` still stores nothing;
a harness that crashes mid-run has already printed that header, which is why the
gate does not lose the case this exists for.
**AC-ATTEST-P3-21.** The test-runner command matcher recognizes a `cargo test`
invocation and rejects lookalikes (`cargo testfoo`, `cargo build`, a path
containing the word), so an arbitrary `Bash` tool call is not captured. —
`attest::capture::tests::ac_p3_21_the_test_runner_matcher_accepts_only_cargo_test`
**AC-ATTEST-P3-22.** `attest run` tees the child's stdout and stderr to its own
stdout and stderr, line by line as they arrive, and exits with the child's own
status — so the text a user reads and the bytes a downstream script reads are
the child's. **Not byte-identical to running the command bare:** the child's
stdio is a PIPE, not the terminal, so a child that adapts to a tty sees a
non-tty and renders its plain form (cargo's colour and progress output being the
one users meet). MEASURED, under a real pty via `script -q /dev/null`: `[ -t 1 ]`
reports TTY run bare and NOTTY run through `attest run --`. — `attest_capture::
ac_p3_22_run_tees_output_and_propagates_the_child_exit_code` (which covers the
tee and the exit code; the tty difference is the manual probe just described)
**AC-ATTEST-P3-24.** A CAS blob cited ONLY by `attest.jsonl` is protected by
BOTH reclaim paths: `purge --orphans` (manual) does not archive it, and the
daemon's automatic eviction tick does not evict it. Reproduced before the fix on
the manual path — a root whose only blob was cited by five `evidence` events
printed `reclaimed 1 orphaned (superseded) blob(s)`. Both protect sets now
harvest `attest.jsonl` alongside `log.jsonl` / `open.json` / `memory.jsonl`; the
two CAS refs any `AttestEvent` carries are `evidence.output_blob` and its
result's `raw_blob` (`derive.body_hash` is a bare hex token digest with no
`sha256:` prefix and no CAS object, so the raw scan correctly ignores it). —
`attest_capture::ac_p3_24_purge_orphans_protects_a_blob_cited_only_by_attest_jsonl`
and `cmds::tests::ac_p3_24_a_blob_cited_only_by_attest_jsonl_survives_the_eviction_tick`.
The eviction half drives the daemon's own harvest -> `plan_eviction` -> `execute`
sequence, not `status`'s dry run: `status` deletes nothing, so a survival
assertion behind it could not red. Its fixture also makes the attest-cited blob a
real eviction CANDIDATE (an older turn snapshots it, a newer turn snapshots
something else, budget 100), because a blob no turn references is never a
candidate and would survive whatever the protect set said. **Mutation-probed:**
removing `attest_path` from each protect set reds its own test.
**AC-ATTEST-P3-25.** `open.json` is a MIRROR of live daemon state, so a journal
left behind by a crashed or killed daemon names a turn that will never be
persisted. `open_turn_id` gates on the same liveness probe `status`/`doctor` use
(`daemon::daemon_is_running`): no live daemon → no open turn, and the capture is
recorded UNJOINED rather than attributed to an unresolvable id. — `attest::
capture::tests::ac_p3_25_a_journal_with_no_live_daemon_is_stale_and_yields_no_turn`
and `attest_capture::ac_p3_25_a_stale_open_json_yields_unjoined_evidence_on_a_clean_tree`
(which also carries the clean-tree half of the dirty bit: `dirty: false`,
`dev_loop_only: 0`). Mutation-probed: removing the gate reds both.
**Consequence, deliberate:** AC-ATTEST-P3-15/16 now run a REAL daemon and a real
hook bracket and assert against the id `daemon.rs::sync_journal` itself wrote —
which also retires the earlier "only the reader is proven" caveat.
**AC-ATTEST-P3-26.** The `--list` line parser is driven by the committed real
capture `docs/fixtures/attest/list.txt`: 37 names, every one from a `: test`
line, and a `: benchmark` line is never mistaken for a test. — `attest::
adapter_cargo::tests::ac_p3_26_the_list_parser_reads_the_real_list_fixture`
**AC-ATTEST-P3-27.** `init` installs Claude Code's `PostToolUse` hook scoped by
`matcher: "Bash"` (mirroring Codex's `apply_patch` scoping) so capture path 2
is wired for real and not merely reachable; a second `init` adds no duplicate;
`doctor`'s hook-presence check passes with it; `uninstall` removes it and drops
the emptied key. The event list is shared (`initcmd::CLAUDE_HOOK_EVENTS`) rather
than duplicated, so an installed-but-unenumerated event cannot leak past
uninstall. `doctor` requires only the BRACKETING pair
(`initcmd::CLAUDE_BRACKET_EVENTS`) — requiring `PostToolUse` would fail every
repo initialized before this hook existed, and turn recording does not depend on
it. — `attest_capture::
ac_p3_27_init_installs_doctor_accepts_and_uninstall_removes_the_capture_hook`
**AC-ATTEST-P3-28.** A REAL failing test (one assert flipped in a fixture copy)
is captured as `outcome: failed` with `parse_failed: false` — a failure is not a
parse error — its same-section siblings keep their own outcomes, and cargo's
`101` propagates. Measured and pinned rather than papered over: **on cargo's
DEFAULT invocation** it stops after the first target that fails, so the
`integration` target never runs and a failing bulk capture records 3 results
where a green one records 5. This is a property of the invocation, not of the
capture — the same fixture under `cargo test --no-fail-fast` runs all three
sections and yields 5 results, still exiting 101 (measured). —
`attest_capture::ac_p3_28_a_real_failing_test_is_captured_as_a_failed_outcome`
**AC-ATTEST-P3-29.** A `PostToolUse` event NEVER maps to a signal. `init`
installs `PostToolUse[Bash]` for every repo (AC-ATTEST-P3-27), and `cmds::hook`
maps every non-`UserPromptSubmit` event to `stop` — so before this fix an
ordinary Bash call (`ls`, `git status`) fell through and appended a stop signal,
closing the open bracket. Measured by the Fable skeptic gate on the `f1da261` extract, with
a real daemon: one prompt + ONE non-test Bash call (`ls -la`) + `Stop` wrote
start,stop,stop and produced TWO rich turns, the first with `files: []`, where
bracketing requires one (the same shape AC-ATTEST-P3-33's mutation reproduces
in-tree); the test below pins the mechanism (an unmatched `PostToolUse` appends a
signal), not that count. `cmds::hook` now returns for EVERY
`PostToolUse` regardless of capture outcome; unmatched shapes (a non-test Bash
command, a non-Bash tool) leave `signal.jsonl` byte-identical and write no
attest event, while `UserPromptSubmit`/`Stop` still map to start/stop. —
`attest_capture::ac_p3_29_no_post_tool_use_event_ever_emits_a_signal`
**AC-ATTEST-P3-30.** Rename donors are scoped to the TARGETS THIS DISCOVERY
ENUMERATED — which is **not** the same as "the same target", and the earlier
"target-scoped" wording overstated it. A claim whose target is outside the
enumerated set is never a donor (so `attest derive --crate <one crate>` cannot
hand its neighbours' ids away), while a test that MOVES between two enumerated
targets with its body unchanged carries its claim, which is what keeps history
across a file move. — `attest::derivecmd::tests::
a_vanished_identity_outside_this_discoverys_targets_is_never_a_donor` and
`attest::derivecmd::tests::a_move_between_two_enumerated_targets_is_a_rename`
**AC-ATTEST-P3-31.** Target attribution scans BOTH output streams for cargo's
`Running` / `Doc-tests` markers. `cargo test 2>&1 | tail -30` is an ordinary
agent shell shape and puts the markers on STDOUT; scanning only stderr found
none, emptied every identity's target, and reported "5 result(s) for undeclared
tests skipped" with zero evidence written — a wrong diagnosis on a healthy
capture. When markers still cannot be paired with the sections, the skip is
counted and reported as **unattributable section(s)**, a diagnosis distinct from
**undeclared** because the two prescribe different user actions. —
`attest_capture::ac_p3_31_markers_merged_into_stdout_are_still_attributed` and
`attest::adapter_cargo::tests::ac_p3_31_markers_are_found_on_either_stream`
**AC-ATTEST-P3-32.** The daemon-driven tests own their `agentrec record` child
through an RAII `DaemonGuard`, not an explicit kill after the asserts: a failing
assert unwinds past such a call and leaks a daemon watching a tempdir root for
the rest of the session. Verified by the post-suite check that
`pgrep -fl 'agentrec record'` lists only the dogfood daemon — there is no
in-suite assertion that can observe its own leak, so this AC is evidenced by
that external check, not by a test.
**AC-ATTEST-P3-33.** End-to-end against a REAL daemon: a non-test `PostToolUse`
in the middle of an open bracket does not split the turn. One prompt, an edit,
an `ls -la` `PostToolUse`, a second edit, `Stop` → exactly ONE rich turn carrying
both files. This is the shape the gate's own probe found broken (two rich turns,
the first with `files: []`), asserted end to end rather than only at the signal
layer. — `attest_capture::
ac_p3_33_a_non_test_post_tool_use_does_not_split_a_live_turn`. **Mutation-probed:**
restoring the `&& capture_from_hook_payload(..)` fall-through reds it with
`the single turn must carry b.rs: ["src/a.rs"]` — the split reproduced.

**Dependency note.** `check-versions.sh` does not track the three new direct
deps (`syn`, `proc-macro2`, `quote`) — it asserts the release version across six
files and has no per-dependency pin check — so a version bump on them is not
gated by CI. Recorded, not fixed. `syn` is taken with `default-features = false`
and only `full`, `parsing`, `printing`, `clone-impls`; `Cargo.lock` shows no
version movement from any of it.

**AC-ATTEST-P3-23.** The staged single-test pipeline keeps its four
`recipe-invalid` causes distinct. THREE are driven end to end against real
cargo output on the fixture crate — `missing` (name absent from `--list`),
`ignored` (`#[ignore]`d) and `build` (the target does not compile) — alongside a
real passing run, so a cause is never a pass. The fourth, `harness`, is driven for real
too: a `std::process::abort()` test is appended to the SCRATCH COPY of the
fixture crate (never the committed one, whose five-test count AC-ATTEST-P3-10
pins), and the scoped `--exact` run produces the real shape — libtest's `running
1 test` header, then SIGABRT, with no per-test line and no summary — yielding
`recipe_invalid: Harness, parse_failed: true, outcome: None`. The synthetic
string is kept as a second case, pinning the same mapping without a signal. —
`attest::adapter_cargo::tests::
ac_p3_23_the_staged_pipeline_keeps_its_recipe_invalid_causes_distinct`

### attest v1 — Phase 4A: `attest verify` (independent replay) + coverage capture

Verdict policy is stated ONCE, in `ATTEST-FORMAT.md` § "Verdict policy"; the
coverage map schema ONCE, in § "Coverage map"; the replay environment ONCE, in
§ "Replay environment". These ACs do not restate any of the three.

**AC-ATTEST-P4-1.** `attest verify` replays from a `git archive` extract of the
pinned commit, never a `git worktree add` (which would share `.git` with the
working tree). The extract goes to a STABLE directory
(`.agentrec/attest-target/extract`), not a fresh tempdir: cargo fingerprints
include the workspace path, so a new path per run would cold-build every
replay. Being shared is why concurrent verifies in one root are serialized by
`.agentrec/attest-verify.lock` (AC-ATTEST-P4C-10). A derive-then-verify round trip on the fixture crate yields
`confirmed`, and the appended `verdict` event's `replay_commit` equals the
repo's `git rev-parse HEAD`. —
`attest_verify::ac_p4_1_derive_then_verify_confirms_and_pins_the_replay_commit`

**AC-ATTEST-P4-2.** A failing test yields `claim-false` only after 3 failing
runs. The mutation probe breaks the tested function and COMMITS it (the extract
carries committed state only), then asserts the verdict is `claim-false` and the
stdout line reports `(3 runs)` — the run count has no wire slot, so stdout is
where it is asserted. —
`attest_verify::ac_p4_2_a_committed_mutation_yields_claim_false_after_three_runs`

**AC-ATTEST-P4-3.** A deleted test function, with the build still green, yields
`recipe-invalid` with cause **`missing`** — asserted positively AND asserted not
to be `build`. The two causes come from different stages of the pipeline
(`--list` membership vs. the build), and conflating them would let a broken
build masquerade as a deleted test. —
`attest_verify::ac_p4_3_a_deleted_test_is_missing_not_build`

**AC-ATTEST-P4-4.** A broken build yields `recipe-invalid` with cause `build`,
asserted not to be `missing`. Same test as AC-ATTEST-P4-3's second half, so the
two causes are discriminated against each other in one place. —
`attest_verify::ac_p4_4_a_broken_build_is_build_not_missing`

**AC-ATTEST-P4-5.** An `#[ignore]`d test yields `recipe-invalid` cause
`ignored`, and restoring the source and re-verifying yields `confirmed` — the
retry path proven end to end, not just the failure path. —
`attest_verify::ac_p4_5_ignored_then_restored_verifies_again`

**AC-ATTEST-P4-6.** A deterministically flaky test — fails on its first run,
passes on the rerun, keyed on a marker file outside the extract — yields
`flaky-observation` and NEVER `claim-false`. Together with AC-ATTEST-P4-2 this
pins both branches of the retry policy. —
`attest_verify::ac_p4_6_a_first_run_failure_that_passes_on_rerun_is_flaky`

**AC-ATTEST-P4-7.** A dirty working tree refuses verification and appends
NOTHING: verify replays committed state, and a verdict minted against
uncommitted bytes would be attributed to a commit that never contained them.
The test asserts both the refusal and that `attest.jsonl` is byte-identical
afterwards. —
`attest_verify::ac_p4_7_a_dirty_tree_refuses_and_appends_nothing`

**AC-ATTEST-P4-8.** `attest coverage` writes `.agentrec/attest-coverage.json`
per the schema, atomically (fresh `create_new` tmp + rename). On the fixture
crate both discovered tests carry `src/lib.rs` in their file set and
`over_stale: []` — the fixture crate declares no `[[bin]]`, so it builds no
binary for a test to spawn and the over-stale rule derives no scope. (The rule
keys on bin targets, not on the package name; AC-ATTEST-P4C-7 adds a `[[bin]]`
to a tempdir copy of the same fixture and gets `src/**`.) When `cargo llvm-cov` or the
LLVM tools are unavailable the test is skipped with a PRINTED reason, never
silently. —
`attest_verify::ac_p4_8_coverage_maps_the_fixture_crate_at_file_granularity`

**AC-ATTEST-P4-9.** The map the PRODUCER writes carries the two shapes the
daemon's matcher discriminates — an entry with measured `files` and no
`over_stale`, and an entry with an `over_stale` pattern and no `files` — under
distinct claim ids. **Scope, stated rather than implied:** `claims_touched_by`
and `match_kind` (which replaced `over_stale_hit` in chunk C,
AC-ATTEST-P4C-5) live in the `agentrec` BINARY crate, which an integration
test cannot link against, so the matching behavior itself is unit-tested in
`attest::coverage::tests` (Phase 4B's file) and this AC pins only that the
producer's wire shape is the one that matcher expects. Neither test alone closes
the pair. —
`attest_verify::ac_p4_9_the_producers_wire_shape_carries_both_matcher_inputs`

**Substitution, recorded rather than quietly satisfied (chunk C).** This AC's
test asserts a hand-written map LITERAL; it never runs the producer. When it was
written the producer could not be made to emit a non-empty `over_stale` in any
fixture, because `over_stale_for` keyed on the literal package name `agentrec`.
Chunk C removed that obstacle — the rule now derives the scope from the bin
target's own source directory, so
`ac_p4c_7_a_binary_building_package_gets_a_derived_over_stale_scope` exercises
the non-empty branch through a REAL producer run on a fixture with a `[[bin]]`.
**What is still not exercised end-to-end is the `agentrec` package's own
path**: no test captures coverage of this repo itself, so `cli/src/**` reaching
a real map is pinned by unit test
(`coveragecmd::tests::over_stale_fires_only_for_binary_spawning_targets_of_this_package`,
which asserts `spawn_scopes` derives exactly that constant from this repo's real
artifact shape) rather than observed. Stated, not closed.

**AC-ATTEST-P4-10.** Missing coverage tooling refuses with exit 2 and writes no
map, rather than writing an empty one that the daemon would read as "nothing is
covered". —
`attest_verify::ac_p4_10_absent_coverage_tooling_refuses_without_writing_a_map`

**AC-ATTEST-P4-11.** Before any capture there is no map file at all — the
normal pre-capture state, which the daemon reads as `Ok(None)` rather than as an
error — and a written map parses back to the same version and entry count. Same
crate-boundary scope note as AC-ATTEST-P4-9: `load_coverage_map`'s own
`Ok(None)`/error branches are unit-tested in `attest::coverage::tests`. —
`attest_verify::ac_p4_11_no_map_exists_before_capture_and_a_written_map_parses`

**AC-ATTEST-P4-12 (found by AC-ATTEST-P4-8, recorded because it failed
SILENTLY).** `cargo llvm-cov show-env --sh` emits BOTH single-quoted and bare
values in one block, and `parse_show_env` takes both. The first implementation
accepted only the quoted form and therefore dropped `RUSTC_WRAPPER` — the
instrumentation itself — so every build succeeded, every test ran, the exit
status was 0, and the map came out with an empty `files` array for every test.
Nothing in the exit code or the output said anything was wrong. The unit test
drives a verbatim capture of the real output, warning chatter included. —
`attest::coveragecmd::tests::show_env_parsing_takes_quoted_and_bare_values_alike`

Two invocations were MEASURED not to work and are recorded so nobody retries
them: `cargo llvm-cov test --no-run` is rejected (`--no-run is specific to
[nextest,...] and not supported for subcommand 'test'`), and `cargo llvm-cov
--no-run` rejects `--message-format`. The `show-env` + plain `cargo test
--no-run --message-format=json` path is what works.

**Probe hygiene, applied throughout.** Every mutation in `attest_verify.rs`
edits the fixture repo's SOURCE and commits it; the replayed binary is built
inside the extract from that commit, so the recorded stale-binary hazard
(editing a source and re-running against a stale `target/`) cannot apply — but
the replay build cache is keyed BY COMMIT for a measured reason: `git archive`
stamps every extracted file with the commit's timestamp, and cargo fingerprints
on mtime, so two commits made inside the same second extract byte-different
sources carrying identical mtimes. Measured on this machine — three successive
fixture commits all landed on one second, and a single shared cache made a
restored test still report `ignored`, which is what AC-ATTEST-P4-5 caught before
the fix. Per-commit keying keeps the cache across a claim's RETRIES by construction
(the retries reuse one extract at one commit, the case the 3-run policy needs)
and gives a different commit a cold build. The retry speed-up was not timed
separately — stated as construction, not as a measurement.


### Phase 4B — daemon stale-marking

Chunk B of Phase 4: the `record` daemon consults the coverage map on writes it
already receives and appends `stale`. **Security posture: the daemon never
executes a repo-authored command.** `cli/src/daemon.rs` production code
contains zero `std::process::Command` call sites (the `Command::new`
occurrences in that file are all inside its `#[cfg(test)]` module); chunk C's
`clippy.toml` census makes reintroduction a build failure.

Structural notes that these ACs rest on, stated once: the daemon appends at
the SETTLED-BATCH flush point (never per watch event), after the H7 undo-guard
`retain`, so undo's own writes stale nothing; it TAKES the attest append lock,
unlike `loglock.rs`'s daemon exemption; and idempotence belongs to
`fold.rs`, so the daemon dedupes per BATCH only and appends without asking
whether a claim is already stale. **Disclosed cost of that choice:** a hot
editing loop over a covered file appends one `stale` line per settled batch,
and `attest.jsonl` has no rewrite class, so those lines are not reclaimable.

**AC-ATTEST-P4B-1.** A write to a file in a claim's measured `files` appends
exactly one `stale` for exactly that claim, with cause `file-write` naming the
path. — `attest_stale::ac_p4b_1_2_3_exact_glob_and_unmapped_writes`

**AC-ATTEST-P4B-2.** A write matching a claim's coarse `over_stale` glob (and
no measured file) appends `stale` for that claim alone, with cause
`coverage-incomplete` naming the scope — not `file-write`. —
`attest_stale::ac_p4b_1_2_3_exact_glob_and_unmapped_writes`

**AC-ATTEST-P4B-3.** A write to an unmapped path appends nothing. Meaningful
only because the same daemon, in the same test, has already been shown to
stale twice. — `attest_stale::ac_p4b_1_2_3_exact_glob_and_unmapped_writes`

**AC-ATTEST-P4B-4.** With no coverage map present the daemon records normally
and never creates `attest.jsonl`. —
`attest_stale::ac_p4b_4_absent_coverage_map_appends_nothing`

**AC-ATTEST-P4B-5.** A coverage map rewritten while the daemon runs takes
effect without a restart. The fixture's rewrite is BYTE-LENGTH-PRESERVING
(`src/a.rs` -> `src/b.rs`), so an mtime- or length-keyed reload trigger fails
this AC; the daemon keys on a content hash. —
`attest_stale::ac_p4b_5_coverage_map_reloads_while_the_daemon_runs`

**AC-ATTEST-P4B-6.** `attest_stale = false` in `.agentrec/config.toml`
disables the append entirely, against a fixture otherwise identical to
AC-ATTEST-P4B-1's. Both new keys (`attest_stale`, `attest_coverage_path`) are
read, are listed in `KNOWN_KEYS` so neither warns as unknown, and degrade
per-key on a wrong-shaped value rather than failing the load — `mcp_destructive`
remains the only named-values hard error. —
`attest_stale::ac_p4b_6_config_off_switch_appends_nothing` +
`config::tests::attest_keys_are_read_and_degrade_per_key`

**AC-ATTEST-P4B-7.** The daemon appending `stale` concurrently with separate
CLI processes appending through `append_attest_locked` leaves no torn line:
`parse_log` reports 0 unparsed and 0 unknown-kind lines, and the event count
equals the seed plus what each CLI process reported writing. **Substitution,
recorded not quietly satisfied:** the plan's text names `verdict` lines, but
`cli` has no `[lib]` target (so a test binary cannot call
`append_attest_locked` directly) and `attest verify` is chunk A's. The
concurrent writer is `agentrec attest derive`, which appends `derive` events
through the SAME choke point; the property proven — two processes appending
concurrently tear no line — does not depend on the event kind. —
`attest_stale::ac_p4b_7_concurrent_cli_appends_leave_no_torn_line`

**Mutation probes, run live (`cargo build` before each, per the recorded
stale-binary hazard).** (a) Removing the `attest_stale.on_batch` call from the
flush point reds -1/-2/-3, -5 and -7 and leaves -4 and -6 GREEN — those two are
negative tests and cannot discriminate on their own, which is exactly why -3's
"nothing appended" leg shares a daemon with two proven-positive legs.
(b) Making the reload trigger fire only once reds -5 alone. (c) Forcing every
cause to `file-write` reds -2 with the two shapes printed side by side.

### attest v1 — Phase 4 chunk C (purity census, replay env scrub, match kinds)

**AC-ATTEST-P4C-1.** `clippy.toml`'s `disallowed-methods` carries a
`std::process::Command::new` entry, and every production spawn site in
`cli/src` carries a per-site `#[allow(clippy::disallowed_methods)]` with its
own one-line reason, so that `cargo clippy --workspace --all-targets
--all-features -- -D warnings` is CLEAN at HEAD on debug and release. This is
the baseline-green half of the two-direction probe: it proves the sweep found
every site the lint can see. `cli/src/daemon.rs` production code carries NO
allow, which is the property the lint exists to hold. —
`docs/verify/attest-purity-census.md` (the two clippy runs, pasted verbatim)

**AC-ATTEST-P4C-2 (manual, recorded in the census doc).** The refusing half:
one unannotated `std::process::Command::new("true")` planted inside an EXISTING
production fn of `cli/src/daemon.rs` makes the identical clippy command fail
with `clippy::disallowed_methods` naming the planted line. An existing fn, not
a new one, because an unreferenced new fn co-fires `dead_code` under
`-D warnings` and would let the probe pass with `clippy.toml` untouched. The
plant is reverted by an inverse edit and the file's SHA-256 is recorded before
and after. — manual; `docs/verify/attest-purity-census.md`

**AC-ATTEST-P4C-3.** `attest verify` runs the replay with a scrubbed
environment, per `ATTEST-FORMAT.md` § "Replay environment" — the allowlist is
stated there and is NOT restated here. Proven in both directions: a canary variable
set on the `attest verify` child is ABSENT inside the replayed test body, and
the same fixture asserts `PATH` IS present — absence alone would pass
vacuously if the parent never set the canary. The pure allowlist filter is
unit-tested separately from the spawn. —
`attest_verify::ac_p4c_3_the_replay_env_is_scrubbed_to_the_allowlist` +
`adapter_cargo::tests::scrubbed_pairs_keeps_the_allowlist_and_drops_everything_else`

**AC-ATTEST-P4C-4.** `attest derive --crate <RELATIVE path>` run from a parent
directory writes the crate's `target/` at `<crate>/target` and creates no
nested `<crate>/<crate>/target`. Closes the Phase 3 defect `5bd5c0f` recorded:
`build_targets` set `CARGO_TARGET_DIR` to a relative `crate_root.join("target")`
while also setting `current_dir(crate_root)`, so cargo resolved the relative
value against the child's cwd. Fixed by canonicalizing `crate_root` once at the
adapter's entry points. —
`attest_verify::ac_p4c_4_a_relative_crate_path_does_not_nest_the_target_dir`

**AC-ATTEST-P4C-5.** `coverage::claims_touched_by` returns the MATCH KIND
alongside each claim — `MatchKind::ExactFile` for a hit on the entry's measured
`files`, `MatchKind::OverStale(glob)` for a coarse-glob hit — so the daemon
consumes the kind instead of re-deriving it. `over_stale_hit` is deleted. The
`attest_stale` cause assertions stay green UNCHANGED, which is what proves the
refactor preserved the two `stale` cause shapes. —
`attest::coverage::tests::match_kind_discriminates_an_exact_file_from_an_over_stale_glob`
+ the unchanged `attest_stale::ac_p4b_1_*` / `ac_p4b_2_*`

**AC-ATTEST-P4C-6.** The producer and the daemon resolve the coverage map's
location through ONE function (`attest::coverage::resolve_coverage_path`), so
`attest_coverage_path` cannot be honoured by one and ignored by the other. It
was: the producer hard-coded the default while the daemon read the key, so with
the key set the daemon read a path nothing had written, got a legitimate-looking
"no map captured yet", and staled NOTHING — no error on any channel. Both halves
are tested against a DECOY map at the other path, so a reader of the wrong file
is visible rather than merely absent. —
`attest_verify::ac_p4c_6_the_producer_writes_the_configured_coverage_path` +
`attest_stale::ac_p4c_6_the_daemon_stales_from_the_configured_coverage_path` +
`attest::coverage::tests::resolve_coverage_path_honors_the_config_key_and_falls_back_to_the_default`

**Caveat on the key, disclosed not enforced:** an `attest_coverage_path`
outside `.agentrec/` (which `init` gitignores) leaves the written map as an
untracked file, so the working tree is DIRTY and the next `attest verify`
refuses outright — a verdict minted then would carry a commit that never held
those bytes. Measured with `attest_coverage_path = "custom/cov.json"`:
`git status --porcelain` reports `?? custom/`, `attest verify` exits 1 with
"working tree is dirty". Nothing rejects the configuration itself; the cost
lands on the next verify.

**AC-ATTEST-P4C-7.** `over_stale` scopes are DERIVED from cargo's artifact
stream — for each package that builds a binary, the bin target's source
directory made repo-relative — instead of keying on the literal package name
`agentrec`. This repo still yields exactly `cli/src/**` (pinned by unit test, so
the generalization cannot drift off the AC-ATTEST-P1-3 ruling), and a fixture
package with a `[[bin]]` now yields `src/**` through a real producer run, which
is what makes the non-empty branch testable at all. A bin whose source lies
outside the crate root yields NO scope rather than an absolute glob — an
absolute pattern would read as coverage while matching nothing, since
`coverage::match_kind` refuses absolute paths (that refusal is cited as a
mechanism, so it now carries its own test: removing it previously left every
coverage test green).

**The crate root is canonicalized on BOTH arms**, `--crate` and a bare
`--root`. `spawn_scopes` strips the crate root off cargo's `src_path`, which
cargo reports canonical, so a symlinked root failed every strip and produced
`over_stale: []` on every entry with no warning — and `/tmp` and `/var` are
symlinks on macOS, making that the ordinary case. Measured before the fix on
one fixture: canonical root or cwd yields `["src/**"]`, the same fixture
through a symlink yields `[]`. —
`attest_verify::ac_p4c_7_a_binary_building_package_gets_a_derived_over_stale_scope`
(three legs: the derived scope, the lib-target ALLOW half, and a symlinked
`--root`)
+ `coveragecmd::tests::over_stale_fires_only_for_binary_spawning_targets_of_this_package`
+ `coveragecmd::tests::a_bin_outside_the_crate_root_yields_no_scope_rather_than_an_absolute_glob`
+ `attest::coverage::tests::absolute_paths_and_absolute_globs_both_match_nothing`

**AC-ATTEST-P4C-8.** `attest verify --all-stale` verifies every stale claim: a
fixture with two stale claims yields two `verdict` events. Previously untested. —
`attest_verify::ac_p4c_8_all_stale_verifies_every_stale_claim`

**AC-ATTEST-P4C-9.** A verdict is appended PER CLAIM as it is decided, so an
error on a later claim cannot discard the work already done. The verdicts used
to accumulate in one batch appended after the loop, so a single adapter error
mid-`--all-stale` threw away every verdict computed before it. The error path is
real, not injected: the second claim names a cargo target absent from the
extract. Both halves asserted — the first claim's verdict IS on the wire and the
erroring claim's is NOT — or the test would pass on an empty log. —
`attest_verify::ac_p4c_9_an_error_on_a_later_claim_keeps_the_earlier_verdict`

**AC-ATTEST-P4C-10.** Concurrent `attest verify` runs in one root are serialized
by `.agentrec/attest-verify.lock`, held across extract + replay. The extract
directory is stable and shared (AC-ATTEST-P4-1), so without this a second verify
deletes and rewrites the first's tree mid-cargo. **This serializes; it does not
make concurrent verifies parallel-safe** — the second waits for the first. A
lock SEPARATE from `attest.lock`, which is held only for a single append:
holding that one across a multi-minute replay would block the daemon's `stale`
appends for the whole run. **No test, stated plainly rather than cited around:**
`attest::lock::tests::an_append_blocks_while_another_writer_holds_the_lock`
exercises `attest.lock` through `acquire_blocking`, NOT `lock_exclusive_at` and
not `attest-verify.lock`. What the two share is the `flock(LOCK_EX)` primitive;
the verify lock's own acquisition is unexercised.

**AC-ATTEST-P4C-11.** `attest verify`'s stdout prints the `recipe-invalid`
cause in the WIRE casing (`ignored`), not Rust's `Debug` casing (`Ignored`), so
the line and the appended event agree about the same field. Derived from serde
rather than a hand-written match, so a new variant cannot drift. —
`attest_verify::ac_p4_5_ignored_then_restored_verifies_again` (asserts the
stdout text alongside the event's `cause`)

**Mutation probes for chunk C, run live** (`cargo build` before each, per the
recorded stale-binary hazard). (a) Making `resolve_coverage_path` ignore the
config reds BOTH AC-P4C-6 halves, each on its decoy assertion. (b) Restoring the
batched append reds AC-P4C-9 on "the first claim's verdict must survive".
(c) Restoring the hard-coded `package == "agentrec"` rule reds AC-P4C-7 with the
whole map printed, `over_stale: []` on every entry.

### Phase 5 — review cards, advisory gate, attestation report

Four new verbs (`attest manual-declare` / `review` / `gate` / `report`), an
extension of `attest status`, and the unhide of `attest` itself. Tests:
`cli/tests/attest_gate.rs` unless another file is named.

**`fyi` follows the plan's Phase 5 AC line: "fyi items never appear in review or
gate output".** An `fyi` manual claim is not an `attest review` card (review
never prompts for one), and `attest gate` neither blocks on it nor lists it —
not even as an advisory line. It is visible in `attest report` and in `attest
status`'s counts, and nowhere else. That is `events.rs::ManualSeverity`'s
"`fyi` never nags" read literally. Founder ruling 2026-09-02: the plan wins over
the round's brief, which had asked for `fyi` cards and advisory lines; an
earlier version of this block recorded that as a deviation and it is now
reverted, code and text together.

**Correction to `8c917d2`'s commit message** (the message itself is immutable;
this is the durable record, and the next commit message states it too). It said
"Goldens unchanged (they hold no manual claims)". The parenthetical is false:
`golden.rs::ATTEST_LOG` carries a `manual-declare` with `severity: blocking`
(claim `c_00000000040W3GE1R70W3GE1R7`). The true reason the goldens did not move
is that the fixture holds no **`fyi`** claim, and the ruling changed the handling
of `fyi` only — a blocking manual claim renders in `attest report` exactly as it
did before.

**Correction to `bcccabc`'s commit message.** It said the gate got a "15-row
exit-code table"; the table it landed had **16** rows: derived, evidenced,
confirmed, claim-false, recipe-invalid, flaky, claim-false AND blocking manual,
manual blocking unanswered/yes/no/skip, manual fyi unanswered/yes/no/skip, and
stale-only. The count was wrong, and the table was wrong in a way the count
could not show — it was hand-listed, and the `claim-false × fyi` cell was simply
absent, which is the hole the Fable gate found (`B1`). AC-P5-2's table is now
GENERATED: **59 rows** (10 statuses × 3 severities × 2 stale = 60, less the one
cell that writes no events at all, `declared × severity=none × stale=false`).
The figure is asserted by the test rather than typed here — if the generator
stops generating, `p5_2_gate_exit_code_table` reds on its own row count.

**The gate's blocking rule is stated once**, in `ATTEST-FORMAT.md` § "Gate
blocking, precisely" (that section's "lands in Phase 5" future tense is replaced
by the implemented rule). `gatecmd.rs` and `reviewcmd.rs` point at it and do not
restate it.

**AC-ATTEST-P5-1.** `attest manual-declare --text <criterion> --severity
blocking|fyi` appends exactly one `manual-declare` event through
`attest::lock::append_attest_locked`, minting a fresh `ClaimId`, and prints that
id on stdout. The claim then folds to `DECLARED` and `attest status` counts it.
— `attest_gate::p5_1_manual_declare_appends_and_status_shows_it`

**AC-ATTEST-P5-2.** `attest gate` exits **1** iff at least one claim either
(a) has a status for which `ClaimStatus::blocks_gate()` is true (`CLAIM_FALSE`
only), or (b) carries `manual_severity == Blocking` with a status that is not
`Human { answer: Yes }` — `DECLARED`, `Human{No}` and `Human{Skip}` all block.
**(a) is evaluated for every claim before any severity handling**, so a claim
that is both `CLAIM_FALSE` and `fyi` still blocks; the precedence is stated once
in `ATTEST-FORMAT.md` § "Gate blocking, precisely". Past the blocking half, an
`fyi` claim reaches neither list. `recipe-invalid` (any cause), `flaky` and the
`STALE` overlay are advisory on a non-`fyi` claim and leave the exit code 0. A
claim satisfying both (a) and (b) is counted once.

The test enumerates the **full cross product**, generated rather than
hand-listed: 10 status recipes (`DECLARED`, `DERIVED`, `EVIDENCED`, `CONFIRMED`,
`CLAIM_FALSE`, `RECIPE_INVALID`, `FLAKY`, `HUMAN(yes)`, `HUMAN(no)`,
`HUMAN(skip)`) × manual severity {none, blocking, fyi} × `STALE` {no, yes} = 60
cells, of which one writes no events at all and is not a row — **59 rows**. Each
row seeds real events in its own root, folds them with `agentrec_core`'s own
fold to learn what the events MEAN, derives the expected exit code and section
by restating the rule over that folded state, and asserts the binary agrees on
both. Deriving expectations from the fold rather than from a hand-written list
is what makes a missing cell impossible: the previous 16-row hand-listed table
simply had no `CLAIM_FALSE × fyi` row, and the gate returned `PASS (0 blocking)`
on that shape while `attest status` reported `claim_false: 1`.
— `attest_gate::p5_2_gate_exit_code_table`

**AC-ATTEST-P5-3.** `recipe-invalid` and `flaky-observation` never produce a
nonzero `attest gate` exit, on their own or in combination, and the ids appear
in the advisory section with their cause.
— `attest_gate::p5_3_recipe_invalid_and_flaky_never_block`

**AC-ATTEST-P5-4.** `attest gate --json` emits `{"blocking": [...],
"advisory": [...], "pass": <bool>}`; `pass` is false exactly when the exit code
is 1, and the two arrays partition the ids the text form prints.
— `attest_gate::p5_4_gate_json_matches_the_text_verdict`

**AC-ATTEST-P5-5.** `attest review` renders one card per claim needing a human —
a `blocking` manual claim whose status is `DECLARED` or `Human{Skip}` — and the keys
`y`/`n`/`s` append `human` events with answer `yes`/`no`/`skip`. After `n` the
next stdin line is read as the note and stored VERBATIM. Each answer is appended
immediately, so an interrupt keeps the answers already given.
— `attest_gate::p5_5_review_scripted_stdin_appends_three_human_events`

**Structural gap in AC-P5-5's card, recorded and NOT redesigned (founder-owned).**
No sanctioned writer can put evidence on a review card. Cards are manual claims;
`attest run` joins `evidence` to a claim by TEST IDENTITY; a manual claim has no
test identity by construction. Every card a user sees today therefore reads
`evidence: none recorded` / `diff: no related turn`, and
`p5_5_review_scripted_stdin_appends_three_human_events` reaches the populated
rendering only by seeding an `evidence` event onto a manual claim id — **a shape
no writer emits**. That test pins the renderer, not a reachable state. The
rendering is kept (a future writer may attach evidence to manual claims) and
`reviewcmd.rs`'s module doc now describes what it renders today rather than
calling itself evidence-first. Deciding what evidence a manual criterion should
carry is a design question for the founder.

**AC-ATTEST-P5-6.** The card set is computed ONCE per invocation, so `s` does not
re-ask inside the same run; a skipped card reappears on the NEXT `attest review`.
`Human{No}` is deliberately NOT re-asked — it blocks the gate (AC-P5-2) and is a
recorded rejection, not an open question — so only `DECLARED` and `Human{Skip}`
`blocking` claims are cards. Stated as intent, not an omission.
— `attest_gate::p5_6_skip_reappears_next_run_and_no_does_not`

**AC-ATTEST-P5-7.** Non-interactive stdin (immediate EOF) lists the remaining
cards and exits **0** without appending anything; EOF arriving where a note was
expected stores an empty note rather than panicking. `attest review --json`
lists the cards and never prompts or appends.
— `attest_gate::p5_7_review_eof_and_json_never_append`

**AC-ATTEST-P5-8.** An `fyi` manual claim appears in NEITHER surface: not as an
`attest review` card (the card list is empty and stdin is never read), and not
in `attest gate`'s blocking OR advisory list. It IS present in `attest report`
and counted by `attest status`, so it is recorded rather than dropped.
— `attest_gate::p5_8_fyi_is_absent_from_review_and_gate_but_present_in_report`
and `attest_gate::p5_8_a_stale_fyi_claim_still_appears_nowhere_in_the_gate` (the
severity guard runs before EVERY gate branch, not just the manual one — guarding
only the manual note let a `stale` event on an `fyi` claim print an advisory
line; the test carries a stale non-`fyi` control so it cannot pass on a gate
that lost the stale note altogether)

**AC-ATTEST-P5-9.** `attest report` prints a markdown bundle and `--json` the
same data as one object: per claim its id, test identity or manual criterion
text, status, stale flag, verdict chain (history counters, the last verdict kind
and its `replay_commit`), evidence provenance (the last `evidence` event's
`turn_id`, `output_blob` and `dirty` bit), and the last human note. The report
carries no wall-clock header, so a fixture log renders byte-identically on every
run. — `golden::golden_attest_report` and `golden::golden_attest_report_json`

**AC-ATTEST-P5-10.** `attest report --range <base>..<head>` keeps only claims
with at least one event whose `ts` falls between the two commits' committer
timestamps inclusive. The filter is TIME-based, deliberately and disclosed in
the report's own header line: it does not prove an event was caused by a commit
in the range. `git log -1 --format=%ct` reports SECONDS and `AttestEvent::ts` is
unix MILLISECONDS, so the boundary multiplies by 1000. A `--range` whose base or
head is not a resolvable revision is an error (detected from git's exit status),
as is a range missing either side or written with `...`.
A **reversed** `head..base` is refused rather than normalized — sorting the pair
would answer a different question than the header names — and a root with no git
history reports the missing repository rather than blaming the revision (git's
stderr distinguishes them).
— `attest_gate::p5_10_range_filters_by_commit_time`,
`attest_gate::p5_10_range_rejects_bad_input`, and
`attest_gate::p5_10_range_refuses_a_reversed_pair_and_a_repo_less_root`

**AC-ATTEST-P5-11.** `attest` is no longer `#[command(hide = true)]`: it appears
in `agentrec --help`, and `agentrec attest --help` lists `status`, `derive`,
`run`, `verify`, `coverage`, `manual-declare`, `review`, `gate`, `report`.
— `attest_gate::p5_11_attest_is_visible_and_lists_every_verb`

**AC-ATTEST-P5-13.** `attest verify` prints the verdict it appended AND the
folded status when the two differ, which happens exactly when a permanently
refuted claim gets a later passing replay (spec decision 4). The status comes
from re-reading and re-folding the log after the append, so `attest verify` and
`attest status` cannot disagree about the same log. When they agree, the line is
unchanged. — `attest_verify::p5_13_a_passing_replay_on_a_refuted_claim_prints_both`
and `attest_verify::p5_13_an_agreeing_verdict_keeps_the_short_line` (the
agreeing case must NOT grow the second clause, or the first test would pass on
a line that always prints both). The permanence clause is printed only when the
folded status is `CLAIM_FALSE`, the one status decision 4 makes permanent; any
other divergence prints `claim status is <STATUS>` without asserting a
permanence the fold does not enforce. That branch is **unreachable through
sanctioned writers today**, and is retained as a guard rather than a live path.
Derivation: the divergence fires only when the folded status differs from the
one the appended verdict implies, and the fold overrides a verdict in exactly
one place — `CLAIM_FALSE` is permanent, so later verdicts are counted and move
nothing. Every other verdict assigns its own status unconditionally (a `flaky`
claim that later confirms becomes `CONFIRMED`), and the `STALE` overlay is not a
status, so it cannot produce a divergence either. Only `CLAIM_FALSE` can, and
that is the branch with the permanence clause.

**AC-ATTEST-P5-12.** `attest status --json` gains `recipe_invalid_causes` (a
per-cause object) and `manual` (blocking/fyi × answered/unanswered counts). Every
pre-existing key keeps its name and its value on the same fixture — the addition
is additive, asserted by comparing the whole key set and each old key's value
before and after.
— `attest_gate::p5_12_status_json_is_additive`

**Gate exit-code shape.** `attest gate` follows `doctor`'s precedent
(`main.rs`: `Ok(false)` → `std::process::exit(1)`), not an `Err`: a failing gate
is a verdict the command printed, not a command error, so no `agentrec: <msg>`
line is prefixed.

**One production spawn is added**, `git log -1 --format=%ct <rev>` in
`reportcmd.rs`, carrying the per-site `#[allow(clippy::disallowed_methods)]`
with its reason, in the same form `replaycmd.rs` uses. `cli/src/daemon.rs`
gains none.

**Golden normalization, deviation recorded.** The brief asked for
`golden.rs::NORMALIZE_TABLE` to be extended for claim ids and attest
timestamps. It is NOT extended: every `c_…` id and every `ts` reaching the attest
report goldens originates from a named constant in the fixture this harness
builds, so there is nothing dynamic to substitute, and adding a substitution for
a static literal would only weaken the byte pin. This is the same reasoning the
harness's module doc already gives for the table being empty. A pattern-based
normalizer stays rejected there.

**Mutation probes for Phase 5, run live** (`cargo build` before each and the
source restored after, per the recorded stale-binary hazard). (a) Making
`statuscmd::blocking_reason` treat `Human{Skip}` on a `blocking` manual claim as
non-blocking reds `p5_2_gate_exit_code_table` and nothing else (11 passed / 1
failed). (b) Dropping the `* 1000` seconds→milliseconds conversion in
`reportcmd::commit_time_ms` reds `p5_10_range_filters_by_commit_time` while
`p5_10_range_rejects_bad_input` stays green — so the range fixture's window
genuinely discriminates the unit, rather than being wide enough to pass either
way.
