# agentrec — red team round 2 (2026-08-01)

**Provenance — this is an analysis of `main`.** Run against the working tree of
`fix/redteam-immediate-actions` @ `9ff1b19`, which is **content-identical to `origin/main`
@ `08cf0d9`** (PR #11's squash). Verified: `git diff origin/main HEAD` with *no* exclusions is
empty — every `.rs` file, test, golden, README, `PROTOCOL.md` and `IMPLEMENTATION.md` read by the
agents is byte-identical to main. The two branches differ in commit topology only. The sole
working-tree divergence was `.claims/claims.jsonl` and a 16-line claimd bookkeeping edit in
CLAUDE.md's standing-debts section, neither of which is product code nor underlies any finding
(checked specifically against F20, whose cited text is unchanged from main).
**No re-run against `main` is required.** Re-verification becomes necessary only once the fix
branches change code — that is the per-round skeptic gate's job, not a repeat of this analysis.

Six adversarial
passes: attribution soundness, data loss / destructive ops, security & privacy, threat model
& evidence integrity, operational & product risk, and a divergence audit against the Sutra
reference implementation (`~/Projects/sutra/src-tauri/src/turns.rs`) that had never been run.

Every finding below was re-verified against source by the orchestrator before promotion.
Findings whose stated mechanism failed verification were corrected or dropped — one is
recorded as corrected in F3.

**Dedup discipline.** Findings are labelled NEW, KNOWN-UNDISCLOSED (the repo knows it
internally; a user installing from README would not — the disclosure gap is itself the
defect), or KNOWN-DISCLOSED (dropped). D6 intra-bracket misattribution, the undo/human-edit
chain, the D49 CAUTION, the unbounded signal inbox, and the orphaned-unit class are all
KNOWN-DISCLOSED and are not reported.

---

## Ranking rule

severity = breaks the "who broke my repo" promise × **silent** (the user cannot tell it
happened) × undisclosed. A quiet wrong attribution outranks a loud failure with a larger
blast radius.

---

# Tier 1 — silent, core-promise, undisclosed

## F1 — CRITICAL — The record is not evidence against the actor it records: a forged `signal.jsonl` line mints a fully-attributed `rich` turn
**NEW (mechanism) / KNOWN-UNDISCLOSED (threat model)**

`agentrec-core/src/record.rs:17` declares `pub tool: String` — free-form, no allowlist, no
provenance check. `parse_signals` (`record.rs:307-317`) is a bare
`serde_json::from_str::<SignalEvent>(line).ok()`. `apply_signal` passes the fields straight
into the engine (`cli/src/daemon.rs:1656`), which stamps `grade: "rich"`
(`agentrec-core/src/engine.rs:344`).

```sh
echo '{"v":1,"ts":<now>,"tool":"vim","event":"stop","prompt":"manual cleanup"}' \
  >> .agentrec/signal.jsonl
```

That produces a turn `blame` renders in the README's headline format as authoritative. Because
it is `rich`, it also counts as coverage for PROTOCOL §5 predicate 2, **suppressing the
`human-edited since` marker**. The inverse works identically: an agent can attribute its own
edits to a human or to a tool that never ran.

Three structural facts make this unrecoverable rather than a missing check:

1. **No tamper-evidence on `log.jsonl`.** `rg` for `prev_hash|chain|hmac|checksum|crc` across
   both crates returns nothing. `record::append_log` (`record.rs:177`) is a plain append.
   "Append-only" is a convention the writer follows, not an enforced property. The CAS
   integrity check is self-referential — it proves a blob matches its own address, so an
   attacker rewriting a line just writes a correctly-hashed blob and points at it.
2. **`.agentrec` is in the watcher denylist** (`cli/src/daemon.rs:766-770`), so tampering with
   the record is the one class of filesystem mutation the recorder structurally cannot witness.
3. **`init` gitignores the record** (`cli/src/initcmd.rs:353`) — no second copy, no VCS
   cross-check.

**Disclosure gap.** `rg -i "threat|tamper|adversar|malicious|forge"` over README.md matches
exactly one line — 71, *"Trust in a flight recorder comes from adversarial testing, not from
reading the source"* — which heads **"How we try to break it"**, the precise section a reader
checks for the trust boundary, and which then discusses only `undo` correctness. README:6
claims agentrec *"is the independent observability and recovery layer."* The honest framing
already exists in this repo — `REVIEW.md:53` says plainly *"nothing prevents rewriting
`log.jsonl`"* and calls it *"not audit evidence; it's a diary"* — but lives only in documents
users don't read. README:180's *"out of scope **for a local single-user tool**"* reveals the
implicit model: single-user therefore trusted, while the stated purpose is adjudicating between
two actors sharing that uid. Deleting `log.jsonl` outright renders as *"no turns recorded — is
`agentrec record` running?"* (`cli/src/cmds.rs:77`) — indistinguishable from a fresh repo.

**Recommendation.** Do not build signing to fix this (that is v3/L3 and won't help a local
adversary anyway). Fix the *claim*: state the threat model in README — defends against
honest-but-buggy agents, careless humans, crashes and bit-rot; does not defend against a
malicious or prompt-injected agent, because both run as your uid.

---

## F2 — HIGH — `undo` destroys symlinks, and in one variant truncates a file that was never in the plan
**NEW**

`restore_from_before` (`cli/src/readcmds.rs:1016-1042`) is the only restore primitive and has
no symlink awareness at all. The recorder deliberately does *not* follow symlinks — for a link
it stores the **link target string** as the blob (`daemon.rs:1249-1259`, `symlink_change` →
`store.put_result(target.to_string_lossy().as_bytes())`). Restore then treats those bytes as
file content. `FileEntry` (`agentrec-core/src/record.rs:65-97`) has no link-kind field, so the
loss is unrecoverable by wire format, not merely unimplemented.

**F2a — deleted symlink is "restored" as a regular file. Default path, no flag.** The
`"delete"` arm (`readcmds.rs:989-992`) calls `restore_from_before` unconditionally. Undoing the
deletion of `node_modules/.bin/tsc -> ../typescript/bin/tsc` writes a 21-byte *text file*
containing `../typescript/bin/tsc`. The symlink is permanently gone; undo-of-undo deletes the
stub and leaves the path absent. `undo` reports success.

**F2b — `--allow-modified` writes through the link and truncates the target.**
`std::fs::write(path, &bytes)` (`readcmds.rs:1032`) opens `O_TRUNC` on the *resolved* path, so
it overwrites the pointed-to file. The post-write verification (`readcmds.rs:1033-1035`) does
`std::fs::read(path)`, which also resolves the link, reads back what was just written, and
**passes** — `undo` prints `reverted 1 file(s)`. Given `config.yaml -> configs/prod.yaml`,
`configs/prod.yaml` is truncated to the bytes `configs/dev.yaml`; a file never named in the
plan is destroyed.

**Why F2b is high rather than critical:** the default path is accidentally gated.
`read_current_hash` (`readcmds.rs:809`) also follows the link, so `current` is the hash of the
*target's contents* while `entry.after` is the hash of the *target string*. They never match, so
a symlink is always `EXCLUDE`d as modified-since. The exclusion message the user reads before
reaching for `--allow-modified` is `modified since (human or external edit)` — a fabricated
cause; nothing edited the link.

`IMPLEMENTATION.md:101` states the invariant ("symlinks are not followed; the symlink itself is
recorded") — enforced on the record side, never on the restore side. Zero tests exercise revert
against a symlink.

---

## F3 — HIGH — Edit, then `git add`/`git commit` within 12 s, and your edit burst is relabelled `tool:"git"` and hidden from `log`
**NEW — mechanism corrected during verification**

`classify` returns `Class::GitRef` for any event on `.git/index` (`daemon.rs:759-765`). That
arms a 2 s window and calls `observe_git_change`, which converts an **already-open `Source::Quiet`
turn** younger than `GIT_SETTLE_MS + QUIET_MS` (12 s) *wholesale, in place*, to `Source::Git` /
`tool: Some("git")` (`agentrec-core/src/engine.rs:230-245`). `log` then filters it out by
default (`agentrec-core/src/view.rs:872`: `t.tool.as_deref() != Some("git")`).

**Correction, recorded because it matters.** The originating agent claimed the index write is
*caused by the edit being recorded* ("edit file, then `git status` → index changed = YES"). I
could not reproduce that, and it is false. Measured on git 2.50.1 (Apple Git-155), sub-second
mtime + inode:

| action | `.git/index` written |
|---|---|
| edit a tracked file | **no** |
| `git status` after an edit | **no** |
| `git diff` after an edit | **no** |
| `git add` | **yes** |
| `git commit` | **yes** |
| `git checkout -b` | **yes** |

The finding survives via a different and more common path: **you edit, then you stage or
commit.** The edit opens a Quiet turn; `git add`/`git commit` seconds later writes `.git/index`;
the open turn is ≤ 12 s old and is converted whole. Consequences:

- `agentrec log` — the turn is **not printed**. Your work is absent from the log you consult.
- `agentrec blame src/auth.rs` → `t_01JX…A4 · git · "—" · 14:03` — a confidently named actor
  that made no change (git turns carry no prompt, so the excerpt renders `—`).
- `agentrec undo` (panic mode) — `resolve_panic_target` skips git turns
  (`readcmds.rs:648`), so it silently targets an **older** turn and reverts the wrong changes.

This also hits agents: an agent that edits and then commits (common) has its work relabelled
`git` and hidden. D3's rationale for git turns is checkout floods, and the classifier cannot
distinguish "files changed *because of* git" from "files changed *by someone*, then git ran" —
the conversion path is precisely the latter. Brackets are exempt (`engine.rs:232` only converts
`Source::Quiet`, pinned by `git_change_leaves_bracket_alone`), so the exposed population is
exactly unbracketed activity: **the human's own edits, and stop-only (L1) emitters.**

---

## F4 — HIGH — Two same-tool sessions in one root: one Stop closes the other's bracket as a clean turn carrying the wrong prompt and session
**NEW**

`observe_stop` matches an open bracket on `open.source == Source::Bracket && open.tool.as_deref()
== Some(tool)` (`engine.rs:292`) — **tool only**. `session` is threaded through `OpenTurn` and
`ClosedTurn` but is never compared. So session A's Stop satisfies the match against session B's
open bracket and closes it via `finish(open, now, "bracket", "rich", false)` — `boundary:
"bracket"`, **`truncated: false`**.

The honesty marker is suppressed exactly where it is most needed. Two *different* tools
(claude-code + codex) fall to the `other` arm and close `truncated: true` (`engine.rs:309`),
flagging the stolen bracket. Two instances of the *same* tool — far the more common setup — get
`truncated: false`: a record asserting a clean start/stop boundary that never occurred.

Claude Code in a terminal (prompt "fix the auth bug") plus Claude Code in the IDE on the same
repo (prompt "update the README") yields:

```
agentrec blame src/auth.rs
t_01JX…B7 · claude-code · "update the README" · 14:12
```

`rich`, `truncated: false`, session B, prompt B — on session A's files. `show` prints B's prompt
as the authorship record for A's code; `undo` reverts A's work under B's name.

README:88-107 discloses the **agent-vs-human** case and prescribes "separation by root". It does
not disclose that the recorded prompt and session can belong to a *different agent session*, and
the `truncated` flag that would expose it is off. The pinning test
`second_start_closes_first_bracket_truncated` (`engine.rs:648`) deliberately uses two different
tools, so the same-tool case is untested.

---

## F5 — HIGH — The bracket deadline is lifetime-anchored, not idle-anchored: a divergence from the "production-proven" reference that truncates live agent turns
**NEW — from the never-run Sutra divergence audit**

CLAUDE.md names `sutra/src-tauri/src/turns.rs` the reference implementation for engine
semantics. Nobody had checked the extraction against it. The audit found the extraction largely
faithful (see the clean list below), with one consequential divergence:

| | Sutra | agentrec |
|---|---|---|
| constant | `HOOK_FALLBACK_MS = 15 min` (`turns.rs:17-23`) | `MAX_BRACKET_MS = 2 h` (`lib.rs:33`) |
| anchor | `now - last_change` — **idle** (`turns.rs:212`) | `now - open.opened_at` — **lifetime** (`engine.rs:379`) |

Sutra's own comment states the intent: *"Long enough to never pre-empt a live agent between tool
calls."* Idle-anchoring means an actively-writing turn is never force-closed. agentrec's
lifetime anchor pre-empts a live agent by construction. Two effects:

1. **Live-turn truncation.** Any agent run exceeding 2 h of continuous activity — an autonomous
   loop, an overnight job — has its bracket force-closed `truncated: true` mid-flight. `finish`
   leaves `self.open = None`, so **every subsequent write by that still-running agent opens a
   `Source::Quiet` turn and logs `grade: "bare"`** — an unattributed activity window, never
   rendered as agent activity. It also breaks panic undo, which refuses on a trailing bare turn.
   The second half of a long agent run is silently de-attributed.
2. **The abandonment window is 8× Sutra's.** After a SIGKILL or closed terminal, agentrec keeps
   the dead agent's bracket open up to 2 h, folding the human's own edits into it
   (`engine.rs:194-212`) and making them undo-able as "the agent's". Sutra bounds this at 15 min
   of idleness.

## F5b — MEDIUM, LATENT — Sutra's "a foreign tool's Stop is a no-op" early return has no counterpart
**NEW — corroborated by two independent passes**

Sutra's `observe_signal` hard-returns `None` when the signal's tool does not own the open turn
(`turns.rs:172-182`) — a signal from agent A is a complete no-op against agent B's turn. agentrec
writes the same tool check but does not use it for the same decision: on mismatch control falls to
the `other` arm (`engine.rs:304-358`), which closes the foreign bracket `truncated: true` **and**
mints a zero-file `stop-only` rich turn for the stopping tool. That phantom then runs
`fold_recent_bares` with `is_bracket = false`, so the pre-turn guard `b.closed_at >= opened_at`
(`engine.rs:459`) is skipped entirely and every bare fragment from the preceding 15 minutes is
`merges`'d into a turn that observed nothing.

This is the same code region as F4 and was reached independently by both the attribution pass and
the divergence pass; the orchestrator confirmed both arms of `engine.rs:288-315` directly. F4 is
the *happy* arm (same tool, session ignored, `truncated: false`); this is the *other* arm.

**Explicitly latent:** only `claude-code` emits signals today, so the two-tool case cannot fire.
Codex 2.1 — already sequenced next in the roadmap — makes it live. A duplicated Stop is a milder
present-tense variant: it mints a phantom, sets `last_rich_closed_at`, permanently disqualifies
earlier bare fragments from folding, and becomes the newest non-git turn, so `agentrec undo` in
panic mode selects it and reports nothing to revert instead of undoing the real work.

---

# Tier 2 — security & privacy

## F6 — HIGH — `scrub()` passes most real-world secret shapes verbatim, and covers 2 of ~8 persisted string fields
**NEW**

Seven regexes total (`agentrec-core/src/scrub.rs:8-46`): AWS **key ID**, GitHub, Slack, Stripe
`sk_live_`/`sk_test_`, PEM blocks, `word=value` assignment, 40+ hex. The entropy gate needs ≥20
chars **and** `shannon > 0.75·log2(alphabet)`; punctuation-bearing tokens get alphabet 95 → a
4.93 bits/char bar real credentials rarely clear. Tokenization is whitespace-delimited, so
nothing spanning a space is evaluated as a unit.

Six of seven planted secrets survived into persisted store files, proven by running the shipped
binary:

```
SURVIVED  wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY   <- AWS *secret* key; only AKIA has a rule
SURVIVED  postgres://admin:Hunter2Hunter2@db.internal:5432/prod
SURVIVED  Authorization: Bearer sk-proj-{40+ alnum}   [literal defanged — see note]
SURVIVED  mongodb+srv://root:P%40ssw0rd123@cluster0.mongodb.net/admin
SURVIVED  sk-proj-{T3Blb''kFJ + alnum}                <- OpenAI; only Stripe's sk_ is covered
SURVIVED  pаssword = hunter2secretvalue               <- Cyrillic а defeats \bpassword\b
REDACTED  eyJhbGci…                                   <- only the JWT was caught
```

Reproduced on the live prompt path — `agentrec hook claude` with an AWS secret key and a
postgres URL wrote both verbatim into `.agentrec/signal.jsonl`.

> **Note on the two defanged literals above.** The OpenAI examples were tested as real-shaped
> tokens but are written here in broken form, because **GitHub push protection rejected the
> commit containing them** (`GH013`, detector "OpenAI API Key", this file line 273). That is
> itself a data point for this finding: *GitHub's scanner detects the shape and agentrec's
> `scrub()` does not.* When building the F6 regression fixture, **synthesize the literals in
> test code** rather than committing them — a public repo should not carry key-shaped strings,
> and the unblock-and-allow path would be the wrong resolution for a project whose own pitch is
> secret hygiene.

`cli/src/cmds.rs:838-839` asserts *"no pre-scrub prompt may ever reach it (AC I4)"*. That is
literally true — scrub ran. **AC I4 is satisfied vacuously**: the invariant is "scrub was
called", not "secrets were removed", and the wording invites the stronger reading.

**Field coverage.** scrub touches only `prompt` and memory `fact`. `TurnRecord.tool` / `.model`
/ `.session` / `.root` and `FileEntry.path` are persisted raw. **File contents are never
content-scrubbed at all** — a hardcoded key in `src/config.ts` goes to the CAS verbatim.
README:171 says facts are scrubbed *"matching the discipline the rest of the store already
applies to prompts and snapshots"*, which reads as a snapshot-content claim that does not hold.

## F7 — HIGH — The secret-file denylist misses most common credential filenames
**NEW**

`is_secret_path` (`scrub.rs:143-174`) matches basename only, and its `credential_word`
(`secret`/`credential`/`key`) fires **only** in combination with a `config_ext`
(`.json .yaml .yml .toml .txt .ini .cfg .conf`). Directory names are never consulted.

Proven by running the binary — accepted (i.e. would be snapshotted in full):

```
.netrc   .git-credentials   .npmrc   .pgpass   .pypirc   .htpasswd
terraform.tfstate   kubeconfig   id_dsa   id_ecdsa   auth.json
AuthKey_ABC123.p8   keystore.jks   secrets/prod.yaml   dump.sql
```

withheld correctly: `.env`, `.envrc`, `.aws/credentials`, `server.pem`.

Sharpest misses: `.git-credentials` (contains "credential" but has no config extension),
`id_dsa`/`id_ecdsa` (only `id_rsa`/`id_ed25519` are listed), and `secrets/prod.yaml` — the
credential word is in the *directory*, and the basename `prod.yaml` alone never matches.

Partial mitigation: the watch filter is gitignore-derived, so a gitignored secret is never
staged. That does not cover the untracked-but-not-gitignored case, which is exactly where
`kubeconfig`, `.pgpass`, `terraform.tfstate` and `secrets/` typically live.

## F8 — HIGH — Terminal escapes in `path` and `tool` can erase the `revert` line from the `undo` confirmation the user reads
**NEW**

`fmt::sanitize_terminal` (`cli/src/fmt.rs:206-214`) strips C0/C1/DEL and is applied to
`prompt_excerpt` and memory facts. It is **not** applied to `t.tool` or `p.entry.path`, which
`print_plan` interpolates raw (`readcmds.rs:854, 858, 865, 869`). `print_plan` runs at
`readcmds.rs:532`, *before* the `if !confirm` branch — so the poisoned plan is exactly what the
user reviews, in both preview and the applying run.

A second file named `"\x1b[1A\x1b[2Ksrc/decoy.rs"` (cursor-up + erase-line; legal bytes in a
filename, creatable by any agent or postinstall script) renders as:

```
raw:        revert  src/prod_config.rs (modify)
            EXCLUDE ^[[1A^[[2Ksrc/decoy.rs — modified since ...
as shown:   src/decoy.rs — modified since ...
```

The `revert src/prod_config.rs` line is gone from the display. `--confirm` reverts it anyway.
Same vector via `tool` in `agentrec log`, reaching stdout raw under `NO_COLOR=1` and when piped.

The existing test `e7_prompt_escape_sequence_never_reaches_stdout_raw`
(`cli/tests/hardening_cli.rs:629-687`) seeds the evil bytes **only** in `evil_prompt`, with
`path: "e.rs"` and a clean `tool` — scope stated in its own section header, "at excerpt render
sites".

## F9 — MEDIUM — A secret captured into the CAS is permanent: no path-targeted purge exists
**NEW**

Every removal path is date-, budget-, or class-based (`purgecmd.rs`, `retention.rs`). There is
no "forget every blob of file X" — blobs are content-addressed and nothing indexes them by path
for removal. Rotate the credential, `git rm` the file, add it to `.gitignore`: the pre-rotation
bytes stay in `.agentrec/objects/` indefinitely and remain readable via `diff`/`show`/`undo`.
The only remediation is a blanket date purge that also destroys unrelated recovery history —
i.e. the product's whole value. This is the persistence multiplier on F6 and F7.

## F10 — MEDIUM — Recalled memory is injected into agent context with no semantic trust boundary
**NEW**

`build_hook_block` (`cli/src/memorycmds.rs:1186-1215`) renders stored facts into a fenced block
on the hook's stdout, appended to the model's context. The only filter is `sanitize_terminal` —
control bytes. `scrub()` redacts secret *shapes*, not instructions. Writers include `agentrec
candidate`, which **the agent itself emits**. An agent prompt-injected once can emit a durable,
file-grounded, validation-passing fact that is re-injected into **every future session** — the
injection outlives the session that created it.

Bounded, and this is why it is medium: `sanitize_terminal` drops `\n`, so a fact cannot break
out of the fence or forge list items. It is content poisoning inside a delimited, labelled
region, not context forgery.

---

# Tier 3 — data loss & operational

## F11 — MEDIUM/HIGH — `purge`'s default prompt deletion is unconditional, unarchived, and runs *before* the daemon-liveness refusals every flag advertises
**NEW**

`purgecmd::run` calls `purge_prompts` at `cli/src/purgecmd.rs:41` — **before any flag dispatch**
(lines 42-56). It has no daemon-liveness check and no archive step; it calls `store.remove`
(unlink). Every daemon refusal lives inside the flag handlers, which run after.

So `agentrec purge --orphans` with the daemon running: unlinks every prompt blob of every turn
older than 90 days, no archive, prints `purged N expired prompt blob(s)` — **then** errors with
`stop recording (agentrec is running) before reclaiming orphans`. The user reads a refusal and
concludes nothing happened. Lost: full scrubbed prompt text for every turn >90d; `show --prompt`
degrades to the inline excerpt. Unrecoverable.

This contradicts CLAUDE.md's house rule — *"`purge` is the only deletion path… archives, never
silent removal"* — and README:123's *"all archive first and all refuse to run while the recorder
daemon is live."*

## F12 — MEDIUM — Budget eviction hard-deletes snapshots on an automatic tick, with no archive and no post-hoc marker
**NEW**

`retention.rs:185` unlinks via `store.remove` — unlike `purge --orphans`, which archives. Per
README:121 the daemon is the sole evictor on a 10-minute tick: fully automatic, no confirmation.
`log.jsonl` is append-only and never truncated, so evicted turns remain listed forever with
their blobs gone, rendered identically to live ones (`format_turn`/`render_log` never touch the
store). The user discovers it at `diff`/`undo` time.

The reachability computation itself is careful and I could not break it — shared blobs, prompt
refs, the newest turn, post-`pass_start` touches and `extra_protected_refs` are all spared, and
`build_plan` correctly *refuses* on a missing blob rather than writing wrong bytes. The finding
is delete-without-archive plus the absent signal, not a reachability bug.

## F13 — HIGH — `status` reports "N recording gap(s)" but counts *crash* gaps only: a deliberately stopped daemon reads zero
**NEW**

`cmds.rs:431` takes `health.crash_gaps`; `crash_gap_count` (`view.rs:89-96`) filters to
`GapKind::Crash`, excluding `Restart` ("the daemon was deliberately off for that interval") and
`TrailingStop`. The daemon writes a clean `stop` epoch on SIGTERM/SIGINT
(`daemon.rs:170`→`:414`). So `kill <daemon>` → damage → restart yields a `Restart` gap that
`status` counts as **0**, with no liveness line in the text report.

`status` computes `daemon_is_running` but uses it only for a JSON field, the ignore-reload line,
and the over-budget branch. The shipped golden proves it: it carries
`"epoch_ignore_rebuilds_stale":true` (daemon not running) while the text reads `gaps: 1 recording
gap(s)` and says nothing about liveness. `unparsed_lines`/`unknown_type_lines` are `--json`-only,
so a truncated log is invisible to the human report.

Scoped: `blame` compensates honestly — `has_gap_after` is any-kind and both blame paths poison on
it — and `doctor`'s `check_daemon` fails on a stopped daemon. The defect is that the one verb
README:121 advertises as reporting recording gaps is the one that under-reports them. Silent
non-recording is the worst failure mode for a flight recorder.

## F14 — MEDIUM — `blame` is the only read path with no superseded-turn filter, and can fabricate "human-edited since" on agent-written files
**NEW**

`select_turns` (`view.rs:846-872`) and `undo`'s `resolve_panic_target` (`readcmds.rs:643-648`)
both exclude superseded ids. `blame` collects touching turns straight from `ledger.records` with
no `merges` exclusion and takes `touching.last()` (`view.rs:1042-1049`). A reachable append order
— git-transition close queues a bare into `pending_closed`, a Stop in the same 250 ms poll folds
it into a rich turn persisted first, then `tick` persists the bare — puts the superseded record
**after** its superseder. Because `fold_recent_bares` copies only `before_hash` for a re-observed
path (`engine.rs:473-484`), `bare.after ≠ rich.after`, so `modified_since` against the bare
returns true. Output: `t_01JX…C2 · bare turn · 14:20 · human-edited since` — both clauses false,
on a file the agent wrote inside a recorded rich turn. Over-attribution direction on PROTOCOL §5
predicate 2.

## F15 — MEDIUM — A backward clock step across a daemon restart makes real recording gaps invisible
**NEW**

`has_gap_after` positions a gap relative to a turn by **lexical string comparison**
(`view.rs:102-106`), valid only if recorded timestamps are non-decreasing in file order.
`Clock::max_wall_ms` clamps that — but it is initialised from `wall_now_ms()` in `Clock::start`
(`daemon.rs:438-444`), **per-process state reset on every restart**. After an NTP step back or a
resume with a bad RTC, a crash-restart appends a `start` epoch stamped *earlier* than the last
turn's `ended`. `recording_gaps` still detects the gap correctly, but `"…13:05:00Z" > "…14:00:00Z"`
is false, so `blame` prints a confident attribution with no `· attribution stale — recording gap`
suffix — the exact thing PROTOCOL §5 requires. `modified_cause` misses it identically, reporting
`"human or external edit"` instead of `"recording gap"`.

## F16 — MEDIUM — `write_state` never fsyncs, widening the crash window `--signals-consumed` names as its safety argument
**NEW**

`cli/src/state.rs:270-280` does `fs::write` → `rename` with **no `sync_all` and no parent-dir
fsync**, unlike every other durability-sensitive write in the codebase (`store.rs:132`,
`purgecmd.rs:434/732/1048/698`). `purge_signals_consumed`'s documented ordering argument treats
the rename→rebase gap as instruction-width; but the `signal.jsonl` rename *is* fsynced while the
`signal_offset` rebase is not, so the durable-vs-lost gap is a page-cache writeback interval.
On power loss in that window the unconsumed tail — real hook signals whose prompts were never
transcribed — is skipped forever and is not in the archive either. Those turns degrade to `bare`.

## F17 — LOW/MEDIUM — Unbounded re-allocation on a newline-less `signal.jsonl` tail
**NEW**

`daemon.rs:1357-1364` reads the whole tail past `offset` with no length cap; if it contains no
`\n` the daemon allocates it, scans, discards, and **does not advance `offset`** — repeating
every poll, forever. Any local process appending a large newline-free blob pins the recorder at
multi-GB RSS and 100% CPU. The shrink/truncation case *is* handled; a grown newline-less tail is
not.

---

# Tier 3b — resource exhaustion & operational failure

All figures below were re-measured by the orchestrator on this repo's live dogfood store.

## F24 — HIGH — The macOS service unit routes the daemon's entire diagnostic channel to nowhere
**NEW**

`launchd_plist` (`cli/src/service.rs:99-121`) emits exactly `Label`, `ProgramArguments`,
`RunAtLoad`, `KeepAlive`. `rg` for `StandardErrorPath|StandardOutPath` across all of `cli/src`
returns **zero hits**, and no `~/Library/Logs/agentrec*` exists on this machine. `daemon.rs`
contains 24 `eprintln!` calls that therefore go nowhere under launchd.

Eviction is the sharpest case, because the code says so itself (`daemon.rs:88-91`): *"Prints one
stderr line when anything is actually evicted; … no persistent eviction-history counter this
phase — stderr plus `status`'s dry-run report are the observables."* Eviction permanently deletes
snapshot blobs — the user's undo history — so on macOS **permanent deletion of recorded history
happens with no durable record of any kind.**

Composed with F12: `log.jsonl` never rotates, so `agentrec log` keeps listing a turn forever after
its blobs are evicted. Retention *is* bounded, but it is bounded by silently converting recorded
history into unrevertible history, with no notification and no way to tell from `log` which turns
are still actionable.

Scope is honest: systemd defaults `StandardError=journal`, so Linux captures this. This is
macOS-only — and macOS is the primary documented install path (`brew install`). Watcher errors and
snapshot failures *are* persisted to `state.json` and surfaced by DEGRADED; eviction is not.

## F25 — HIGH — Whole-file CAS over append-only files is quadratic in disk. Measured: 9× the git history it records
**NEW**

Every touched file version is stored whole; there is no delta encoding. For a monotonically
appended file, snapshot *i* costs ≈ *i·k* and the total is ≈ *k·n²/2*.

Measured on this repo, 21 days of recording:

| | |
|---|---|
| `.agentrec` | **317 MB** |
| `.git` (entire history) | **35 MB** |
| `objects/` | 298 MB across 1884 blobs |
| largest blob | **2,876,199 bytes** |
| `.claims/claims.jsonl` | **2,876,199 bytes** — exact match |

151 snapshots of that one append-only file account for ~120 MiB, ~41% of the store, for a file
whose per-append delta is ~10 KB. The mechanism is path-agnostic and generalises to
`package-lock.json`, `pnpm-lock.yaml`, `*.sqlite`, `.ipynb` with outputs, `terraform.tfstate`,
`compile_commands.json` — none of which is typically gitignored.

`noise_globs` does **not** help: `cli/src/noise.rs` is explicit that matches are folded out of
`log`/`show`'s *human-rendered output only* — never `--json`, never `diff`/`blame`/`undo` — and it
defaults to empty. **There is no configuration knob anywhere that reduces disk for a noisy path**
short of editing `.gitignore`.

## F26 — HIGH — The budget and the evictor measure different byte sets, and the budget is not user-configurable
**NEW**

`RepositoryHealth.store_bytes` is `store.total_bytes()` — a `read_dir` walk of `objects/`, **disk
bytes including orphans** — and `over_budget` compares that to the budget. But `plan_eviction`
(`agentrec-core/src/retention.rs:59`) accumulates and selects victims only over hashes reachable
from turn entries. **Orphaned blobs count toward the budget and are simultaneously ineligible as
victims.**

Measured here, and reproduced independently by the orchestrator by walking `objects/` and
diffing against every `before`/`after`/`prompt_ref` hash in `log.jsonl`: **127.8 MiB orphaned of
294.4 MiB on disk = 43.4%** (1887 blobs, 1475 referenced). A subagent's independent derivation gave
130.6 / 293.8 MiB = 44% — same figure by a different route. Worth flagging that an orphan-vs-churn
fraction is exactly the kind of number this repo has already been burned by (see F18), which is why
it was derived twice.

A store whose orphan fraction alone exceeds the budget enters a permanent state where every
600 s tick evicts *every evictable snapshot blob* — destroying all revertible history — while
`status` still reports over-budget forever, because the bytes actually responsible are invisible
to the evictor. The remedy is a manual `purge --orphans`, and nothing tells the user which of the
two problems they have.

Compounding: the budget is **not user-configurable**. `effective_store_budget`
(`cli/src/cmds.rs:910-918`) reads only `AGENTREC_TEST_STORE_BUDGET_BYTES`, gated
`#[cfg(debug_assertions)]` and compiled out of release. No `config.toml` key exists. Ten repos is
a non-negotiable 20 GiB ceiling — and "put concurrent work in a separate worktree", the README's
own remedy for D6, means more roots.

## F27 — HIGH — The schema version field `v` is never compared anywhere in production code
**NEW**

`rg` across `agentrec-core/src` and `cli/src` for any comparison of `.v`, any `match` on it, or a
named `PROTOCOL_V`/`SCHEMA_V` constant returns **zero hits** outside tests. Every writer emits the
literal `v: 1`.

PROTOCOL's "additive-only within a major `v`" rule therefore has **zero runtime enforcement**. An
old binary reading a `v: 2` record does not refuse it — it parses it as v1 and silently
misinterprets the fields it recognises. Because these records gate `undo`, a misinterpreted future
record is a wrong-bytes revert source.

Unknown-*field* tolerance is genuinely real and correctly done (`deny_unknown_fields` appears
nowhere; every optional field has `#[serde(default)]`). The hard half — detecting a major bump and
refusing — is absent. The `unknown_type_lines`/`unparsed_lines` census cannot catch a *known* type
at an *unknown* version. This matters most right now, with a protocol 1.0 freeze sequenced next.

## F28 — HIGH — `log.jsonl` is unbounded with no rotation class, and `signal.jsonl` has already regrown past its own documented figure
**NEW**

Exactly three sanctioned rewrite classes exist and **none rotates**. `memory-stats.jsonl` has no
class at all. Read verbs `read_to_string` the whole log. Current: `log.jsonl` 2.6 MB / 2685 lines;
CLAUDE.md records `status` at 13.4 ms on a 2143-line log. Eviction reclaims blob bytes but never
touches `log.jsonl`, so records outlive their own blobs and growth is monotonic by design.

Measured now: **`signal.jsonl` is 15.8 MB** — D48's reclaim is a manual `purge --signals-consumed`,
so the inbox has already regrown past the 13.4 MB that motivated building D48 in the first place. A
manual reclaim for a monotonically growing file is a treadmill, not a fix. `log.jsonl` does not
even have that.

## F29 — MEDIUM/HIGH — Service units are baked at `init` and never migrated; `doctor` has no template-version check
**NEW**

`service::install` does rewrite a unit when content differs (`service.rs:531-541`), but it is
reachable only from `init`/`install` — nothing invokes it on upgrade, and `install.sh` replaces the
binary only. `scan_units` classifies on exactly two axes, root-existence and exec-existence; there
is **no unit-template-version check**, so a unit generated by an older binary is reported by
`doctor` as healthy — an active false negative, not merely a gap.

Structural class: a unit is a snapshot of two absolute paths *and* a template, taken once at
`init`, validated at neither load nor upgrade. Every unit-template fix reaches only users who
happen to re-run `init`, and `doctor`'s clean pass affirmatively signals there is nothing to re-run.
Same class as the Homebrew exec-path defect in F20.

## F30 — MEDIUM — The gitignore-derived watch filter is the wrong predicate for disk
**NEW**

The builtin denylist is exactly `.agentrec | node_modules | target | dist` plus the gitignore set.
Anything untracked-but-not-ignored is recorded in full; anything tracked is recorded in full
regardless of churn.

Measured here by the orchestrator directly from `log.jsonl`: `.remember/` — agent tooling,
untracked, absent from this repo's `.gitignore` — accounts for **7659 of 9179 total file entries,
83.4% of all recorded file activity**. (A subagent's independent count gave 7496 of ~9500 = 79%;
the figure above is the reproduced one.)

`cargo build` and `npm install` are safe only because `target` and `node_modules` are hardcoded.
The equivalents for other ecosystems are not: `.venv`, `vendor/`, `build/`, `.next`, `Pods/`,
`__pycache__`, `.gradle`, `.tox`, `.terraform`, `DerivedData`. Most projects do gitignore these,
which caps this at medium — but when it misses it misses silently and without bound.

## F31 — MEDIUM — Hook and daemon are decoupled, so a dead recorder produces a normal-looking session with zero recording
**NEW**

The Claude Code hook appends to `signal.jsonl` and succeeds whether or not the daemon is alive.
Nothing in the agent session surfaces the recorder's absence. If the watch fails to arm
(`daemon.rs:158-161`), `run()` returns `Err` and the process exits; under `KeepAlive` launchd
respawns it in a loop while `signal.jsonl` keeps growing from hook writes — so **the inbox looks
healthier the longer the outage lasts**, and per F24 the startup error is unroutable on macOS.

Discovery requires voluntarily running `status` or `doctor` — commands one runs only when already
suspicious. For a flight recorder this inverts the value proposition: you learn you were not
recording exactly when you needed the recording. `status` and `doctor` do report it accurately and
the flock-based liveness probe correctly refuses to be fooled by pid recycling. The gap is purely
discoverability: **there is no push channel of any kind.**

---

# Tier 4 — evidence integrity of the project's own claims

## F18 — HIGH — The sixth signature-defect instance, and it is shipped user-facing advice
**NEW**

`cli/src/cmds.rs:616-618`:

> *"Most store bloat is usually ORPHANED blobs — superseded intermediate snapshots the daemon
> `put` for crash recovery that no committed turn references — which eviction can't touch."*

Doubly quantified ("Most… usually") over an unstated population of real stores, and load-bearing:
the next statement computes `orphan_bytes` and points the over-budget user at `purge --orphans`.

The repo measured the opposite. `docs/superpowers/specs/2026-07-25-store-churn-designs.md:39`
records **churn share of distinct blobs 2451 / 2799 = 87.6%** — turn-*referenced* snapshots,
exactly what `--orphans` cannot reclaim. `daemon.rs:2323-2324` independently records *"7419 file
entries / 764 MiB — 98% of referenced store bytes."* On such a store the shipped advice reclaims
~0 while asserting it is the fix. The claim's basis (`VERIFY-LEDGER.md:21`) is one store on one
machine on one day, generalised to "usually" — and the same store measured the other way 8 days
later. The project already recorded this exact lesson in its own memory index ("measure first").

## F19 — MEDIUM — The torture launch gate structurally cannot reach the code Phase 2.0 added
**NEW — orchestrator-verified**

README:78 makes the nightly torture harness a launch gate requiring 7 consecutive green nights.
The harness is real and healthy: `.github/workflows/nightly.yml` is cron-scheduled `17 7 * * *`
and `gh run list` shows **15/15 green, daily since 2026-07-17**.

**The durable half.** `cli/tests/torture.rs` contains **zero** references to `import`, `imported`,
`synthesized`, or `baseline_unknown`. The harness cannot reach imported turns — the one class where
`before` bytes are **derived** (git-resolved T2, `after_synthesized`) rather than observed, i.e.
precisely where "undo writes the wrong bytes" is most likely. Its two invariants are asserted only
over turns the daemon itself recorded. Running the gate more often does not fix this; extending it
does.

**The timing half, which expires on its own.** PR #10 (Phase 2.0 substrate, `a90e9ff`) merged to
`main` at 2026-07-31T21:39Z; the most recent nightly at the time of writing ran 2026-07-31T08:15Z,
13 hours earlier, so no nightly had yet covered the merged substrate. The next scheduled run closes
that specific gap — but it closes it by running a harness that, per the paragraph above, does not
exercise the new code. Recorded so the distinction is explicit rather than implied.

## F20 — MEDIUM — Founder-pending lists a public-launch blocker as "NOT BUILT" that is built and merged
**KNOWN-UNDISCLOSED (stale record)**

CLAUDE.md:295/307 asserts *"`init` bakes a CANONICALIZED exec path, which breaks every Homebrew
user on upgrade… **NOT BUILT**."* `cli/src/initcmd.rs:246-266` (`service_exec_path`) reads
*"Deliberately NOT `canonicalize`d, and this is the whole point of the function"* with the
identical Homebrew/Cellar reasoning. There is no `current_exe().canonicalize()` in the file;
canonicalize appears only on the root and in tests. Claim `clm_4BWNKY43VCD30YVP8G0ZXB4DXA` is
CONFIRMED for the macOS behaviour.

The genuinely open residual, declared by the function itself and **not** carried into any
user-facing doc: **Linux cannot be fixed this way** — `std::env::current_exe()` reads
`/proc/self/exe`, already kernel-resolved, so a symlinked install records the target regardless.
That collides with D39's planned npm-wrapper and mise/asdf distribution, which are shim/symlink
based by construction.

## F21 — MEDIUM — Untested load-bearing acceptance clauses, and one AC pinned to a corpus that no longer exists
**NEW**

- **AC-S7** requires doctor's note carry *"the exact two-command removal pair."* The test
  (`cli/src/doctorcmd.rs:843-871`) asserts only prefix, label, path and "unmounted volume" — all
  satisfied by an earlier fragment of the same string. **Deleting the entire `remove each with:
  <cmd>` tail passes every assertion.** Verified: a `manual_remove_command` returning
  `format!("{label} && rm {path}")` — no `launchctl bootout`, not a pair — passes both tests. The
  command is correct today, so this is a coverage gap, not a live bug; but README:122 promises the
  pair to users, and D46's rationale for *not* building `service prune` is that this string closes
  the problem.
- **AC-S9** is stated in the present tense — *"the parser is run over this machine's 41 installed
  plists and must report 41/41 parsed and 40 vanished."* 39 units were reaped 2026-07-31; the
  machine carries one. No one can satisfy it.
- **IMPLEMENTATION.md:521** — *"Every AC in this document maps to at least one automated test
  except B4/B8 and H++ (nightly) and launch items (WS7)"* — is false by AC-S9's own parenthetical
  ("ledger row, not a unit test"), which is not in that exception list.

Sampling context, and it is favourable: 9 other criteria were checked (AC-CAUTION-1..4, D48's four
refusal gates, AC-S1/S3/S5/S6, SR6) and **all discriminate properly** — several are exemplary,
notably `mixed_plan_caution_scopes_itself_to_reverted_files_only`, which asserts fixture
non-vacuity before its real assertions. The discipline is strong; the blanket sentence overreaches.

## F22 — MEDIUM — A corpus-shape absolute licenses a string-splitting parser over files agentrec did not write
**NEW**

`cli/src/service.rs:426-427`: *"the only documents this ever sees are the ones `launchd_plist`
writes."* But `scan_units` reads the user's machine-global `~/Library/LaunchAgents` — a population
agentrec does not control. launchd equally accepts **binary** plists (`plutil -convert binary1`
makes the splitter return `None`), plus older-version and hand-edited units. `VERIFY-LEDGER.md:871`
records the repo's own rule that a unit we could not read must never be reported as an orphan. The
real-corpus evidence (41 parsed, 0 unparseable) establishes "every plist on one machine, all
written by current agentrec, parses" — not the quoted absolute.

## F23 — MEDIUM — PROBLEM.md states a rollback absolute that README's own Known Limitations refutes
**KNOWN-UNDISCLOSED (cross-document)**

PROBLEM.md:43 — *"Rollback refuses, by default, to touch any file whose content changed since the
turn… **so recovery never silently destroys later work.**"* README:95-100 documents the opposite
for intra-bracket edits, in bold. The absolute is technically rescued by "since the turn" (the
human edit lands *during*), but no reader parses it that way, and the D6 window is the product's
single most likely data-loss path. PROBLEM.md is the positioning document.

---

# Product risk

**The README concedes the headline question, and the concession is fatal for the default setup.**
agentrec is named for *"Who broke my repo — me or the agent?"* Known limitations then states that a
human edit made while an agent's turn is open **is attributed to the agent**, that `undo` of such a
turn **discards those human edits with no modified-since warning**, and that the remedy is
*"separation by root."* For the single most common configuration — one developer, one repo, an
editor open while the agent works — the headline answer can be wrong and the most dangerous verb
can silently destroy work. A skeptic does not have to find this; it is disclosed on the page. F3
and F4 above widen it into the unbracketed and multi-session cases, which are *not* disclosed.

**The disclosed remedy multiplies the operational cost, and this is the tightest argument against
adoption in this report.** "Separation by root" means another root. Per F26 and F29 each root is
another launchd unit baked once and never migrated, and another **non-configurable 2 GiB** store
budget. The fix for the attribution problem is the amplifier for the resource problem — and the
whole argument is assembled from the product's own text plus measurements of its own store.

**The "first" claim is a three-predicate carve-out that will not survive its first hostile
comment.** README:6 — *"the first local, tool-agnostic flight recorder for coding agents."* Every
qualifier is load-bearing, and a reader counting them reaches the conclusion the sentence exists to
prevent. Two pieces of prior art land hardest, for opposite reasons:

- **JetBrains Local History** — roughly two decades of prior art for the *mechanism*: local,
  automatic, zero-config, per-file timeline with revert, no commit required, already installed on
  millions of machines.
- **aider's commit-per-edit** — answers the *headline question* directly, git-natively, with no
  daemon, no store, no budget.

Behind them: VS Code Timeline / Local History, [ckpt](https://pypi.org/project/ckpt-cli/) (a local
tool-agnostic CLI explicitly covering Claude Code, Codex, Cursor, Copilot, Windsurf, Kiro and
Aider), the `gitwatch`/`git-wip`/`etckeeper` family, and now vendor-native rollback in
[Claude Code](https://code.claude.com/docs/en/checkpointing),
[VS Code](https://code.visualstudio.com/learn/foundations/reviewing-and-controlling-agent-changes),
[Kiro](https://kiro.dev/blog/introducing-checkpointing/) and
[Replit](https://docs.replit.com/features/version-control/checkpoints-and-rollbacks). The carve-out
is defensible if litigated word by word, but "first" buys nothing the next sentence doesn't already
earn — *"the independent observability and recovery layer"* is the stronger claim and is not
contestable.

**Strategically: the recovery half is being commoditised from both directions** — by the platforms
agentrec integrates with, and by simpler git-backed CLIs. What none of them do is **line-level
attribution across a recording gap with an explicit rich/bare honesty grade**. That is the
defensible wedge; the README leads with disaster-recovery framing (D43, deliberately) instead.

**"Just use git" is answered only in a subordinate clause, and the rebuttal is stronger than the
answer.** `git add -A && git commit -m wip` before an agent run takes two seconds, needs no daemon,
no disk budget, no launchd unit, and is already muscle memory. It answers "what changed" and "can I
undo it" completely. What it does not answer is line-level attribution *within one uncommitted
working state* — real, genuinely unserved, and much narrower than the pitch implies. The project's
own P1 corpus measurement agrees: only ~11% of 1631 real sessions carried any file mutation at all.

**The always-on cost is real, measured, and disclosed nowhere a first-time user will look.** 317 MB
in 21 days against a 35 MB git history; a signal inbox already regrown to 15.8 MB; a 2 GiB per-repo
budget the user cannot lower; two separate manual reclaim commands. The README's Install section
says nothing about any of it. The first-time user's discovery mechanism for the cost of this
product is running low on disk.

**The trust asymmetry is the quiet one.** A flight recorder's value is believing what it says.
CLAUDE.md catalogs five prior instances of a confidently-worded comment asserting a real-corpus
fact that was false, two real daemon defects caught only at a final gate, and a memory dogfood row
closed FAILED because the emitter never fired once in a week. That this is all disclosed is
genuinely to the project's credit. But the skeptic's read is: the recorder has repeatedly been
wrong about its own state, and every time what caught it was adversarial human review — not the
product's own instrumentation. F18 is that pattern in the present tense, and F1 is its security
analogue.

**What the README already does better than the category, honestly noted.** The torture harness as
an explicit launch gate — randomized interleavings, kill-9s mid-write, seven green nights required,
runnable by the reader in one command — is a real trust artifact, not a badge. The refusal to guess
across a recording gap (`attribution stale — recording gap`) is the right decision and the most
credible line on the page. Known limitations is unusually honest, and publishing it is why the rest
of the page is believable. And no cloud / no telemetry / no sudo / one-line install / Apache-2.0
removes the category's largest adoption blocker. **The product's problem is not candor. It is that
the candor documents a headline limitation whose only remedy compounds an undisclosed operational
cost.**

---

# Verified clean — reported so the negatives are on the record

- **Test baseline honest.** `cargo test --workspace -- --test-threads=3` → **666 passed / 0 failed
  / 3 ignored**, exactly matching CLAUDE.md's stated figure.
- **Nightly torture gate real.** Cron-scheduled, 15/15 green daily since 2026-07-17 (see F19 for
  the coverage caveat).
- **No network code.** `rg` for `reqwest|hyper|curl|TcpStream|UdpSocket|std::net|tokio::net|ureq`
  across both crates matches only prose in comments. Dependencies are serde, serde_json, sha2,
  regex, similar, clap, ignore, notify, walkdir, libc, ctrlc. The local-only claim holds.
- **No JSONL injection.** Every write goes through `serde_json::to_string`; zero manual JSON string
  construction anywhere in either crate. No prompt, filename, tool or transcript field can append a
  forged record. (F1's forgery uses the *documented emitter interface*, not injection.)
- **Path traversal genuinely hardened**, in three independent places: `store.rs:277-283` requires
  64 lowercase hex before any join; `memory.rs:123-155` rejects absolute paths, any `..`, symlink
  escapes and secret paths; `importcmd.rs:938/999` guards transcript paths lexically.
- **Permissions**, verified empirically on the live store, not from the doc comment: `.agentrec`
  and every `objects/*` dir are `drwx------`; `config.toml`, `log.jsonl`, `signal.jsonl`,
  `memory.jsonl`, `state.json` all `-rw-------`.
- **Blob store sound.** tmp+fsync+rename+dir-fsync, per-writer-unique tmp names, verify-before-dedup
  with self-heal, re-hash on every read, malformed-hash rejection so a crafted ref cannot turn
  `remove` into an arbitrary-path delete.
- **`uninstall`/`init` delete no user data** — `uninstallcmd.rs` contains zero `remove_*` calls; it
  archives. `init` backs up `settings.local.json` before writing, and adds `.agentrec/` to
  `.gitignore` idempotently in git repos (tested).
- **Service-unit generation is injection-free** — the label is a sha256 prefix, not user text, and
  both interpolated values are XML-escaped.
- **`skipped`/`withheld` refusals are correctly sequenced** above the modified-since compare (SR6),
  and `baseline_unknown` has no hole — it always pairs with `before: None`, which `build_plan`
  refuses.
- **No banned figure leaked into README.** The 99.6% ingestion rate, the banned 92.3%, the ~85%
  ceiling, "897 → 17" and the 43.8%/31.7% pair appear only in internal docs, each carrying its
  rider. README contains no recovery percentage at all.
- **The Sutra extraction is otherwise faithful** — quiet window, idle anchor for Quiet turns,
  close-time clamp, empty-change early return, snapshot cap, never-delete-an-unrecoverable-file,
  blob integrity, torn-tail newline discipline, signal-shrink handling, newest-turn GC protection
  and corrupt-line tolerance are all present, several strengthened. Divergences in the safe
  direction: monotonic engine clock, and terminal-flag updates on re-observation that fix a
  delete-then-recreate freeze Sutra still has.

---

# Skeptic gate — independent Fable pass, 2026-08-01

A fresh-context Fable skeptic was run against this report with instructions to refute it. It
examined every CRITICAL/HIGH finding and sampled the MEDIUMs, reproducing measurements with its own
scripts. **26 findings SURVIVE; 4 are severity-inflated; 1 is UNVERIFIED; 0 were refuted outright.**
It reproduced F26 (43.4% orphaned), F30 (83.4% `.remember`) and F25's disk figures independently —
F26 now has three derivations, two of them using agentrec's own broader `referenced_hashes` set.

**Three corrections accepted, two of which land on the orchestrator's own work:**

1. **F3's correction was itself wrong, in the direction that understates the finding.** This report
   asserted as an absolute that `git status` never writes `.git/index`. That is false, and the
   orchestrator's own raw data showed it: in the first test run, the first `git status` after a
   commit **did** rewrite the index (mtime + inode both changed); only the *second* status, and
   status-after-an-edit, left it alone. The skeptic independently observed the same thing in 2 of 7
   trials, both on the first status following a commit. Neither party isolated the trigger, and
   neither asserts a mechanism — consistent with git's racily-clean index refresh, but unproven, and
   this repo's signature defect is exactly the confident unmeasured mechanism claim.
   **What is established:** the absolute is wrong, so F3's exposed population is *wider* than the
   corrected text claims. A `git status` fired by a shell prompt (starship, p10k), an IDE poll, or
   lazygit can convert an open Quiet turn into a hidden `tool:"git"` turn **with no human git
   command at all.** F3 is the report's most under-stated finding, not its most-corrected one.
2. **F18 and F26 contradict each other, and F26 wins.** F18's consequence clause — that the shipped
   `status` advice pointing at `purge --orphans` "reclaims ~0" — is refuted by this report's own
   measurement of the same store: orphans are 43.4%, so `--orphans` reclaims 43% here. F18's valid
   core survives and is unchanged: `cmds.rs:616-618` generalises "Most… usually" from one store on
   one day. **Fix the wording; do not remove the advice.**
3. **F19's timing half has expired.** A nightly ran 2026-08-01T08:01Z, after the `a90e9ff` merge.
   The durable half — `torture.rs` has zero `import`/`synthesized`/`baseline_unknown` references —
   is unaffected, which is why F19 was already rewritten around it.

**Severity downgrades accepted:** F5 effect 1 (after force-close, the eventual Stop recovers via
stop-only attribution within `FOLD_WINDOW_MS`, so de-attribution needs a >15 min tail — effect 2,
the 2 h abandonment window, survives at full strength); F8 (HIGH→MEDIUM: precondition is an
adversary who can already write arbitrary filenames into your repo, i.e. F1's model, where worse is
available); F22 (`scan_units` has a third `Unparseable` state, so a binary plist is never reported
as an orphan — the repo's rule is honored in code, consequence near-nil); F24's key list omitted
that `KeepAlive` is a `PathState` dict, an existing mitigation.
**F14 is downgraded to UNVERIFIED** — the blame half is real, but neither party traced the daemon
poll ordering that makes a superseded record land after its superseder.

**The skeptic's most important structural criticism of this report, accepted:** D6 was deduped out
as KNOWN-DISCLOSED, correct by this report's own rule and wrong by consequence. D6 *is* the
product's answer to its headline question, and in the default configuration the answer is wrong.
See the MVP section below.

---

# MVP assessment

**"MVP" is ambiguous here and the ambiguity changes the answer.** agentrec is already shipped —
v0.1.0, Apache-2.0, brew tap, `curl|sh` installer, public repo. Three readings:

| Reading | Status |
|---|---|
| **Safe-to-install** — a stranger uses it two weeks without losing data or leaking secrets | **Not yet.** ~6 items, mostly days of work. |
| **Delivers-the-promise** — actually answers "who broke my repo" in the default configuration | **No — and fixing all 31 findings does not achieve it.** |
| **Launch-ready** — ROADMAP Phase 0's demand gate | Never run; no data against its 30-day kill criterion. |

## The MVP-blocking set, ranked by cost rather than severity

**Hours:** F1 (README threat-model paragraph — the honest text already exists at `REVIEW.md:53`);
F24 (`StandardErrorPath`/`StandardOutPath`, two lines, and it is the precondition for every other
operational finding being *observable* on the primary platform); F11 (move `purge_prompts` behind
the liveness check and give it the archive its four siblings have); F27 (compare `v`, refuse an
unknown major — cheap now, expensive after the protocol 1.0 freeze).

**Days:** F13 + F31 (report all gap kinds and daemon liveness in the text report; a dead recorder
currently produces a *healthier-looking* inbox the longer the outage runs); F26 (count only what the
evictor can reclaim, and add a `config.toml` budget key — the 2 GiB ceiling is currently
unsettable in release builds).

**Schedule-coupled, not merely severity-coupled:** **F2** needs a link-kind field on `FileEntry`,
which wants doing *before* the protocol 1.0 freeze. That makes it a sequencing decision, not just a
bug fix.

**Coupled pair:** F6/F7 are ten-line table edits, but F9 (no path-targeted purge) makes every miss
permanent — so they need the path-scoped reclaim to land with them or they are only half-fixed.

**Weeks, and it is the actual product:** F3 plus D6 — see below.

## The load-bearing answer: no, fixing the findings does not deliver the promise

Two independent analyses — this report's orchestrator and the fresh-context skeptic — converged on
the same conclusion by different routes.

**The promise closes from both sides.** With **D6**, the "agent" answer is wrong whenever you type
during a bracket: a human save inside an open bracket folds into the agent's turn, `blame` names the
agent's tool and prompt for your lines, and `undo` discards your edit with no modified-since warning
because D30's rail compares against a post-turn `after` and has nothing to fire on. With **F3**, the
"me" answer disappears from `log` whenever git runs afterward — or, per the correction above,
whenever *anything* runs `git status`. **The product currently answers its headline question
reliably only when the human is idle and does not touch git — which is the configuration in which
nobody needed to ask.**

The documented remedy for D6, "separation by root", is also the one remedy that makes the disk
findings worse: another root is another non-configurable 2 GiB budget and another baked launchd unit.

## What would fix it — assembly, not research

The per-file authorship data the live daemon discards **already exists in this codebase.**

- `ChangeObs` (`agentrec-core/src/engine.rs:13-29`) carries path, hashes and flags — **no
  timestamp, no ordering index, no signal-window marker.** `observe_changes` receives `now` and
  discards it per file, keeping only a turn-level `last_change_at`. So today a turn cannot even say
  *when* within the bracket a file was written.
- But per-file timestamps alone would not attribute anyway — a human save and an agent write are
  indistinguishable at the filesystem layer. **The distinguishing signal is the agent's own
  transcript**, and agentrec already has it: `SignalEvent` carries `transcript: Option<String>`
  (`agentrec-core/src/record.rs:23`), so the daemon receives the transcript path at Stop, and P1's
  importer already extracts per-edit `toolUseResult.filePath` from exactly those files
  (`cli/src/importcmd.rs:607-608`, `:845`).

**The fix:** intersect the transcript-declared writes for a bracket against the fs mutations
observed in that bracket. Paths the agent declared are attributable to the agent; paths it did not
declare are marked unattributed **at the file level**, excluded from `undo`'s default plan, and
rendered as such by `blame`. That converts D6 from "cannot distinguish" into "distinguishes for
every hook-emitting tool" — which is every tool supported today. It is built from parts already
shipped, and it is the highest-value item on the board.

**Until it exists,** the honest move is to narrow the README's claim from *"who broke my repo — me
or the agent?"* to what is actually provable: **what changed, when, inside which agent turn, and can
I put it back** — attribution scoped to the turn, not the author.

## Suggested cuts

The skeptic's cut list, with the evidence for each:

- **The memory subsystem** (`remember`/`recall`/`candidate`/`verify`/`forget`). The repo's own
  ledger closed the one-week dogfood row **FAILED** — the candidate emitter never fired once in 1944
  signal lines and the hit rate was unfalsifiable as written. It is a second product sharing the
  first one's binary, it is why `.remember/` is 83.4% of recorded file activity, and F10 is a
  durable prompt-injection surface that exists only because of it. Default `memory_enabled = false`.
- **`import`.** No stranger has a backlog on day one; its `before` bytes are *derived* rather than
  observed — the class where a wrong revert source is most likely — and per F19 the torture harness
  cannot reach imported turns at all. Defer until the harness covers it.
- **Git-turn *hiding*** (keep the classification; checkout floods are real). The default-hide and
  the panic-undo skip are what convert F3 from a mislabel into lost work.
- **Service units.** 39 orphaned LaunchAgents on the author's own machine, baked once and never
  migrated (F29), and on macOS they swallow every diagnostic (F24). Ship `agentrec record` plus a
  documented one-liner for MVP.
- **Keep:** `log`/`diff`/`blame`/`show`/`undo`, `status`, `doctor` (it is the discovery channel for
  silent non-recording), and `purge --orphans` (it reclaims 43% on the only store anyone has
  measured).

---

# Suggested order of work

1. **F1 disclosure** — write the threat model into README. Cheapest, highest trust impact; the
   honest text already exists in `REVIEW.md`.
2. **F2** — symlink handling in `restore_from_before`. Real data destruction on a default path.
   Needs a wire-format field for link kind, so it wants doing before the protocol 1.0 freeze.
3. **F6/F7** — scrub shapes and the secret-path denylist. Both are additive table edits;
   F9 makes every miss permanent.
4. **F8** — route `path` and `tool` through `sanitize_terminal`. One-line fix, defeats a
   confirmation-prompt spoof.
5. **F11** — move `purge_prompts` behind the daemon check and give it the archive every sibling has.
6. **F24** — add `StandardErrorPath`/`StandardOutPath` to the launchd plist. Two lines, and it is
   the precondition for every other operational finding being *observable* on the primary platform.
7. **F27** — compare `v` and refuse an unknown major. Cheap now, and the protocol 1.0 freeze is the
   moment it stops being cheap.
8. **F3/F4/F5** — attribution correctness. These are the wedge; F5 also wants a decision on whether
   the Sutra idle anchor was dropped deliberately or by accident.
9. **F13/F31** — `status` should report all gap kinds and daemon liveness; a dead recorder needs a
   channel that reaches the user without them suspecting anything first.
10. **F19** — re-run the nightly gate against the merged substrate, and extend the torture harness
    to imported turns before claiming the launch gate is green.
11. **F25/F26/F28** — the resource story needs a decision, not a patch: delta encoding or a
    per-path size policy, a budget that counts what the evictor can actually reclaim, a
    user-configurable budget key, and a rotation story for `log.jsonl`. Until then the honest move
    is to disclose the growth rate in README's Install section.
