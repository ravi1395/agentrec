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
| D51 | *(Phase 2 tail, Task D0, 2026-08-05 — founder-ratified as a precondition of the Protocol 1.0 freeze, which is a one-way door)* **D7's offline-drop posture is RATIFIED and unchanged; the drop stops being silent.** D7 says a `start`/`stop` signal that arrived while no recorder was running is never fed to the engine at startup — replaying one would mint an empty turn misdated to daemon boot, i.e. fabricate attribution. That stays exactly as it was; `replay_pending_candidates` still routes only memory-candidate lines to `ingest_candidate`, and nothing in this decision mints, replays, or reconstructs a turn. **What the decision fixes is the measured cost of that posture**, recorded in `VERIFY-LEDGER.md` § "RE-RECORDED 2026-08-05" and § "SUPERSEDED 2026-08-05": a tool session whose ENTIRE bracket elapses while the daemon is down leaves `signal_offset` fully consumed, zero turn records, and no trace of any kind — which reads identically to no activity at all. A gate round reproduced this deterministically and (correctly) found no code defect: the mechanism is this documented decision doing its job. Silence, not the drop, was the defect. **Mechanism: a field on the existing epoch record, not a new record type** — `EpochRecord.dropped_signals: u32` (PROTOCOL §5, FORMAT-CHANGELOG "Phase 2 tail D0"), the count of turn-boundary lines the gap scan dropped, threaded from `replay_pending_candidates`'s new `ReplayOutcome` into the `start` epoch `daemon::run` appends immediately afterwards. A new top-level record type was rejected: this lands days before the 1.0 freeze, and one additive integer on a record consumers already parse is a far smaller surface to be stuck with forever than a record type every consumer must learn. **Count only, and that boundary is load-bearing** — no `scrub_prompt`, no `BlobStore` write, no excerpt, nothing derived from the dropped line's payload. Capturing the prompt would re-import exactly the phantom-data problem D7 exists to prevent, one level down: a prompt with no turn, no files, and no `after` hashes is unanchored text presented beside real records. **Predicate is `kind.is_none()`**, the set `apply_signal` would route to its start/stop arms — NOT "every line the ingest branch skipped", which also holds memory-candidates whenever the `memory_enabled` kill-switch is off, plus any future typed `kind`. Counting the wider set would make the field overstate lost brackets the moment memory is disabled. **Zero means "counted nothing", never "lost nothing"**: every early bail-out in the scan returns 0 because it parsed no lines, and the `len < start` inbox-shrink branch is the sharp case — bytes were genuinely lost there and cannot be counted, because they were never read. That loss already has a louder, separate channel (`resync_shrunk_signal_offset`: stderr + `record_io_failure` → DEGRADED), and this field deliberately does not attempt to speak for it. **Wire compatibility:** omitted when 0, so every epoch line ever written is byte-identical (pinned at both the type layer and the `append_epoch` layer). `stop` epochs always pass 0 because a clean shutdown scans no gap — and that call site is **not test-pinned**, stated explicitly rather than implied: every daemon integration test SIGKILLs by design, so the clean-shutdown `stop` epoch is never written under test, and re-pointing that call at the replay count leaves the whole suite green (measured, not assumed). **Deliberately not built:** no reader consumes the field yet — `view::recording_gaps` and `GapKind` are untouched, and `blame`'s "attribution stale — recording gap" text is unchanged, so every human-form golden stays byte-identical under the P5 ratchet. A "(N tool signals dropped while offline)" clause on that message is the obvious next consumer and is a separate, non-freeze-critical change. |

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
