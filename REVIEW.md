# agentrec — Hostile review (subagent, verbatim)

*Skeptical review of PROBLEM.md and SPEC.md, consistency-checked against PROTOCOL.md, CLAUDE.md, ROADMAP.md, IMPLEMENTATION.md, and Sutra's `turns.rs`. Preserved unedited below.*

## Disposition (v0.2 revision, applied across the suite)

| Finding | Status | Where |
|---|---|---|
| F1 human/agent conflation + turn splitting | Accepted | Bracketing + retroactive merge + crash rule in v1 (D25, PROTOCOL §4, SPEC capture, AC C+) |
| F2 git floods the ledger | Accepted | Git-turn classification, D3 revised (SPEC capture, AC C++) |
| F3 compliance built on unattainable attribution | Accepted | Phase 4 reframed to visibility/reporting; attestation gated on L3 vendor signing (PROBLEM, ROADMAP) |
| F4 gaps → confident false blame | Accepted | Epoch records, stale-blame output, service install in v1 (D27, AC E+/G6) |
| S1 snapshot secret channel | Accepted | Withheld files, TTL/budget defaults, "provably" banned (D31, AC I+) |
| S2 denylist inadequate | Accepted | Gitignore-derived filtering (D29, AC B+) |
| S3 two "human-touched" definitions | Accepted | Two named predicates; undo keys on modified-since (D30, PROTOCOL §5) |
| S4 committed-log id collisions | Accepted | ULIDs + per-writer sidecars for sharing (D26, PROTOCOL §3/§5) |
| S5 gif vs file-level blame | Accepted | Line-level blame in v1 (D14 reversed, AC G5) |
| S6 hook/transcript fragility | Accepted | Canary CI, defensive extraction, rich-rate in `status` (D7 revised, AC Q+) |
| S7 two-weekend fiction | Accepted | ~8 weekend-equivalents, M1–M3, M1 tripwire (D33) |
| S8 protocol moat overclaim | Accepted | "Upside, not the moat" reframe; git-hack paragraph rewritten (PROBLEM) |
| S9 inotify exhaustion | Accepted | Loud startup failure + AC (D29) |
| S10 unbounded growth | Accepted | TTL 90 d default, 2 GiB budget, `status` (D31/D32) |
| Minors | Accepted | Wording fixes across suite (ms timestamps, "battle-tested", "any snapshotted turn", brew at launch, etc.) |
| Steelman / verdict | Answered, not dismissed | M1-exit tripwire makes "fold back into Sutra" a cheap, explicit outcome (ROADMAP Phase 0) |

---

## FATAL

**F1. The quiet-window heuristic cannot tell a human from an agent, and stop-only signals split real agent turns. This breaks the flagship question in both directions.**

Targets: SPEC.md — *"a burst of file mutations followed by a **10-second quiet window** closes a turn (same mechanism proven in Sutra)"* and *"Between turns, edits not attributable to any signal or active burst are attributed to the human."*

Two independent failures, both unaddressed:

- **Human bursts become "agent" turns.** A human saving in vim/VS Code produces a mutation burst followed by quiet — indistinguishable from an agent burst at the fs level. Those edits get recorded as bare turns. Consequence: the human-touch derivation (PROTOCOL §5: *"file changed after `ended(N)` with no covering turn record"*) **never fires for editor-based human edits, because the human's edits ARE covered by a (bare) turn record.** The undo-safety warning — the thing PROBLEM.md sells as *"rollback warns when a human has edited a file since the turn, so recovery never silently destroys human work"* — is silently defeated by the tool's own segmentation.
- **Stop-only signals + 10s quiet chop real agent turns.** v1 installs only a Stop hook; the daemon learns a Claude turn existed *after it ends*. Claude Code routinely pauses >10s mid-turn (thinking, long tool calls, running tests with no file writes). Sequence: edits at t=0–5s, 60s test run, edits at t=70s, Stop at t=80s. The quiet window fires at t=15 and closes the first burst as a **bare/unknown** turn; the Stop signal attributes the prompt to only the trailing files. Blame on a first-burst file answers "unknown" — on the *most common* agent behavior pattern. Neither SPEC.md, PROTOCOL.md, nor IMPLEMENTATION.md's C-series ACs (hook-then-quiet, quiet-then-hook) covers mid-turn splitting or retroactive merge.

The *"same mechanism proven in Sutra"* claim is the tell. In Sutra it works because Sutra **is the editor**: it knows its own buffer saves (so human edits are ground truth, not inference), its agent tracker isolates external writes, and `turns.rs` explicitly *suppresses* the quiet window for hook-covered agent kinds (turns.rs:178–182) while the UI shows the open turn. The extraction keeps the state machine and discards every input signal that made the state machine's output correct. This is not an extraction of a solved problem; it's a new, harder problem wearing the old code.

Minimum credible fix: (a) pull `UserPromptSubmit` start signals from Phase 2 into v1 — start+stop bracketing eliminates both mid-turn splits and human-burst misattribution *for the hooked tool*; (b) on Stop, retroactively fold bare turns since the start signal into the rich turn; (c) rename/reframe bare turns as what they are — "unattributed activity windows," not turns — and never let them suppress human-touch warnings (i.e., undo-safety must use IMPLEMENTATION H3's hash-comparison, not the covering-turn derivation); (d) explicitly specify hooked-tool-crash behavior (Ctrl-C/kill with no Stop signal) — today the docs are silent and both options (suppress quiet vs. don't) are broken in different ways.

**F2. Ordinary git usage floods the ledger with garbage turns — and the design forbids the fix.**

Targets: SPEC.md — *"ignore `node_modules`, `target`, `dist`, build outputs, `.git` internals — Sutra's watcher filter rules carry over"*; IMPLEMENTATION D3 — *"No git dependency."*

`git checkout`, `git pull`, `git stash pop`, `git rebase` rewrite hundreds of worktree files in a burst. `.git` internals are denylisted, but the worktree changes are real mutations → every branch switch becomes a giant bare turn attributed to `unknown`. A normal developer's log will be dominated by these; `agentrec log` becomes unreadable and `blame` mostly answers "some 400-file unknown turn." Worse, the *"Sutra's watcher filter rules carry over"* claim is false by the project's own docs: Sutra's CLAUDE.md says its watcher deliberately **keeps** `.git/HEAD`/`index`/`refs/**` events precisely so it can react to commits/checkouts — the hard-won nuance the extraction throws away, and D3 ("no git dependency," pitched as a feature) prevents reintroducing. Minimum fix: watch `.git/HEAD`/index despite D3, and tag bursts coinciding with git ref changes as `tool: "git"` (or suppress them). Without this, v1 dies at first contact with a repo where someone uses branches.

**F3. The compliance second act is built on attribution the product cannot actually establish.**

Targets: PROBLEM.md — *"engineering orgs in regulated industries who will be *required* to show what AI wrote, under what instruction, and who reviewed it"*; *"the compliance/retention layer on top becomes the business."*

What the recorder actually establishes is *which time window a file mutation landed in*, correlated with a hook signal. D6 admits it: *"Interleaved multi-tool activity in a single root is attributed to the open turn — documented limitation."* A human editing while Claude runs is recorded as AI-authored; per F1, a human editing between turns is recorded as an unknown agent. An attestation product whose ground truth is timing correlation, captured by a **voluntarily run, locally killable, locally purgeable** daemon (with `purge` as a first-class verb and the signing key held by the very developer being audited — nothing prevents rewriting `log.jsonl` and re-signing) is not audit evidence; it's a diary. Ed25519-signing a heuristic guess (D20) produces a cryptographically verified guess. When orgs actually need AI provenance, they will demand capture at the point of ground truth — the agent vendor or gateway — not a timing-inference sidecar. Minimum fix: reframe Phase 4 as team *visibility/reporting* (honest, sellable, smaller), or make the compliance path depend on L3 vendor-native emission with vendor signatures — in which case admit the business is gated on adoption you don't control.

**F4. Recording gaps produce confident false blame, and v1's deployment model guarantees gaps.**

Targets: SPEC.md — *"`agentrec record` # daemon; a terminal tab in v1, launchd/systemd unit post-v1"*; PROBLEM.md — *"you can only record the past if you were recording at the time."*

A recorder that runs only while a terminal tab is open will be off half the time. During gaps: agent runs unrecorded, human edits unrecorded — and afterward `blame` confidently reports the last *recorded* turn as the file's provenance. There is no gap concept anywhere in the data model: no daemon start/stop epochs in `log.jsonl`, no staleness annotation in blame output, no AC covering it. PROBLEM.md's own sentence (quoted above) is the indictment of its own v1. A provenance tool that misattributes confidently after any gap torches exactly the trust it sells. Minimum fix: epoch records (`daemon-start`/`daemon-stop` lines) in the log; `blame` must print "recording gap since t_NNNN — attribution stale" whenever current content hash ≠ last recorded `after` hash or a gap covers the interval; launchd/systemd install moves into v1.

---

## SERIOUS

**S1. The object store is an unscrubbed secret-retention amplifier; the scrub pipeline covers the wrong channel.**
SPEC.md: *"No network code path exists in v1 — provably nothing to leak"*; scrub applies to *prompt text* only (PROTOCOL §9). But `objects/` stores **raw file contents** of every touched file. An agent that touches `.env`, `credentials.json`, or a key file immortalizes it in `.agentrec/objects/` — plaintext, outside git's protections, surviving rotation, default-retained (TTL/purge language centers on prompt objects). Additionally, the store creates a *new* attack surface: every local agent (the ones being recorded, with MCP/fs access) can read every other tool's past prompts and snapshots. "Provably nothing to leak" is false the moment Dropbox/Time Machine syncs `~/Projects`. Fix: default snapshot-denylist for known secret-file patterns, snapshot-side scrub or refusal, and delete the word "provably."

**S2. The denylist is five entries; real ecosystems will bury the log in bare-turn noise.**
SPEC.md/CLAUDE.md denylist: `.git`, `.agentrec`, `node_modules`, `target`, `dist`. Not covered: `.next/`, `.venv/`, `__pycache__/`, `coverage/`, `.pytest_cache/`, `.turbo/`, `*.tsbuildinfo`, `.idea/`, codegen into `src/`. A Next.js dev server alone writes continuously into `.next/` → perpetual bare turns. AC B3 tests only `node_modules`. SPEC's sample log (one tidy bare turn between two rich ones) is fiction; real ratios will be noise-dominated, which means the product's most common blame answer is "unknown" — barely better than `git status`. Fix is cheap and obvious: respect `.gitignore` semantics as the default filter. That it isn't in the spec suggests the daemon has not been run outside the Sutra repo.

**S3. Undo silently clobbers later turns' work; SPEC and IMPLEMENTATION define "human-touched" differently.**
SPEC.md undo *"warns on the human-edited file"* — but if **turn 49** (agent) also touched the file, undoing turn 47 restores 47's `before` blob, wiping 49's changes with **no warning**: PROTOCOL §5's derivation ("no covering turn record") says not-human-touched, since 49 covers it. IMPLEMENTATION H3 quietly uses a different definition (*"current content ≠ turn's `after` hash"*) that would warn — under a misleading name. Pick one: undo safety must key on any-change-since (hash), blame's "human-edited since" on the derivation, and the docs must stop using one term for two predicates. (Credit where due: the MCP two-phase token with drift-abort in PROTOCOL §8 is genuinely well designed.)

**S4. "Shared blame via committed log.jsonl" breaks the format's own id scheme.**
SPEC.md: *"Teams may opt in to committing `log.jsonl` for shared blame."* PROTOCOL §5: `id` is *"Unique in this log, ordered (`t_0047`)"*. Two developers on two branches both mint `t_0048`; merge produces duplicate ids and interleaved-out-of-order JSONL, plus guaranteed merge conflicts on an append-only file — and the file lives inside a gitignored directory, so committing it requires ignore-pattern gymnastics the spec never mentions. D21's sidecars solve multi-*tool* writers, not multi-*human*-via-git-merge. Fix: machine-scoped ULIDs or per-writer sidecars for the team path, before protocol 1.0 freeze — this is exactly the kind of thing you can't fix additively after freezing.

**S5. The launch demo demonstrates a capability v1 explicitly doesn't ship.**
PROBLEM.md's success definition IS the gif: *"`agentrec blame src/auth.ts:42` → `turn 47 · claude-code · ...`"* — line-level. IMPLEMENTATION D14: *"v1 `blame` is file-level; line-level lands in v1.x."* (SPEC hedges with "may ship file-level first," contradicting its own decision register.) File-level blame on a hot file returns whatever turn touched it last — almost never the turn that introduced the bug on line 42 — so the cut version doesn't answer the flagship question at all. Either line-level is v1 or the gif is vaporware on launch day.

**S6. Hook and transcript fragility is a single point of failure treated as solved.**
The entire "rich" tier hangs on: Claude Code's settings-merge surviving updates and enterprise managed-settings; the Stop hook firing (it doesn't on crash/Ctrl-C — see F1; SubagentStop is a separate event the spec never mentions); and D7's prompt extraction (*"last user message in the transcript at Stop time"*) against an **undocumented internal transcript format** that has changed before and will again — where "last user message" is frequently a tool-result entry, a queued second message, or stale after `--resume`. One vendor update quietly downgrades every turn to bare and nobody notices until blame comes back empty. Fix: a transcript-format canary test in CI, defensive prompt extraction (role+content-type filtering), and a visible "rich-rate" health stat in `agentrec log`.

**S7. The two-weekend plan is off by roughly an order of magnitude, which poisons trust in every other number.**
SPEC.md: *"Weekend 1: extract... Weekend 2: blame + undo + scrub pipeline; README...; PROTOCOL.md; publish."* The same project's IMPLEMENTATION.md v1 AC list spans ~50 criteria including a 24-hour soak test, a two-OS CI matrix, a kill‑9 harness, scrub positive/negative corpora, and perf smoke on a 100k-file repo. For one engineer on nights and weekends, that is 6–10 weekends of honest work. Either the ACs are decoration or the schedule is; an investor reads this gap as not knowing which.

**S8. The open-protocol moat has no forcing function.**
PROBLEM.md: *"the turn-log format becomes the standard other tools emit."* Protocols get adopted when a dominant emitter or consumer forces them (LSP had VS Code; MCP had Anthropic). A solo project with one hook integration has neither, and vendors have an active *disincentive* to emit a neutral format that commoditizes their lock-in — PROBLEM.md itself predicts *"each will build the single-tool, closed, session-scoped version."* Both can't be true: if vendors behave as predicted, L3 never happens and the "standard" is one repo's JSONL dialect. ROADMAP's kill criterion partially concedes this; PROBLEM.md should stop leaning on the standard as the long game and lean on the tool being useful alone. Related overclaim: git-per-turn *"collapses the moment two agents work in parallel"* — false; worktrees give each agent its own branch where per-turn commits work fine (agentrec's own model says *"parallel agents = distinct roots"*), and *"loses the prompt and model metadata entirely"* is contradicted by IMPLEMENTATION L1, which imports aider's commit trail as **rich** turns — impossible if the metadata were lost.

**S9. Linux watcher scalability is unexamined.** `notify` on Linux = inotify = one watch per directory; large monorepos exceed default `max_user_watches`, and the failure mode is **silent partial watching** — holes in a ledger whose whole value is completeness. AC B8 measures RSS, not watch exhaustion. Fix: detect watch-limit errors, fail loudly with the sysctl remedy, add an AC.

**S10. Unbounded growth with no default policy.** Full before/after copies per turn, no delta encoding, `ttl_days` with no stated default, snapshot purge manual. A heavy agent month on a mid-size repo is multiple GB in `.agentrec/objects/` before anyone looks. Needs: a shipped default TTL, a size budget with oldest-snapshot eviction, and `agentrec status` showing store size.

---

## MINOR

- PROBLEM.md: *"Coding agents now make most of the edits in an agent-assisted repo"* — circular (true by definition of "agent-assisted") and presented as market data.
- PROBLEM.md: *"The bottleneck on agent adoption is no longer model capability; it is accountability infrastructure"* — thesis stated as fact; one supporting anecdote would help.
- *"proven in production inside Sutra"* (PROBLEM.md, SPEC.md) — Sutra is the author's personal editor; "production" is one user. Say "battle-tested in daily personal use" and it stops being a claim a diligence call can puncture.
- SPEC Goals: *"Make any turn safely reversible"* — contradicted three sections later by `skipped: true` files being never revertible (10 MiB cap). Write "any snapshotted turn"; also note the cap means the *biggest* clobbered files are exactly the unrecoverable ones.
- SPEC says the signal carries *"timestamp, tool name, session id, and transcript path"*; PROTOCOL marks session/transcript MAY and its own example omits `session`.
- PROTOCOL `ts` is unix *seconds*, but AC C4 promises correct ordering of signals <1s apart — the schema can't express the ordering the test demands.
- *"no account, no server"* — `record` is a resident daemon; fine, but don't say "no server" in the same breath as shipping one.
- `cargo install agentrec` as the day-one funnel filters the Show HN audience to Rust-toolchain owners; brew from day one is cheap.
- Sutra migration (SPEC: `.sutra/turns` → `.agentrec/`) is asserted in one sentence; the house rule "never delete, archive" (CLAUDE.md, and the user's own global rules) at least appears in IMPLEMENTATION U1 — keep it in SPEC too.

Solid and worth one line each: bare-turn honesty (`unknown` stays unknown) is the right call; append-only-everything with byte-offset signal tailing is sound engineering; the MCP destructive-tier design (off/confirm/auto, two-phase token, undo-is-a-turn) is the best-designed section in the suite; ROADMAP's standing kill criteria are unusually honest.

---

## STEELMAN AGAINST

The turn engine's correctness in Sutra comes from signals only an editor has: it knows which writes are its own buffers (human ground truth), its agent tracker isolates external mutations before the engine ever sees them, and the quiet-window suppression works because the UI holds the open-turn state in front of a user who can see it. Extraction strips every one of those inputs and ships the naked state machine into the most hostile input environment possible — raw fs events polluted by editors, formatters, dev servers, and git — where its outputs (F1, F2) are wrong in exactly the ways that destroy trust in a trust product. Meanwhile the vendors sit at the point of ground truth: Claude Code checkpoints or Cursor history covering bash-tool edits is a release note away, and each incremental vendor improvement shrinks the standalone tool's territory to "bare turns from tools without hooks" — the segment where agentrec works worst. Kept inside Sutra, the same engineering effort compounds: turn tracking becomes the editor's differentiating feature, with correct attribution, a GUI that can actually show it, and one codebase for a nights-and-weekends solo dev instead of a daemon + CLI + protocol + MCP server + VS Code extension + PR bot federation that ROADMAP commits to maintaining alone.

---

## VERDICT

**Conditional as a build; no as an investment as specced.** Engineer hat: the extraction is worth attempting *only after* four design changes land in the spec — start-signal bracketing in v1 with retroactive bare-turn merge (F1), git-op awareness despite D3 (F2), gap/epoch honesty in the data model and blame output (F4), and gitignore-based filtering (S2); without those, the two-weekend dogfood will itself demonstrate the log is noise and the project self-kills on its own Phase 0 gate — cheaply, which is the one virtue of the current plan. Investor hat: no — the wedge is an episodic pain with insurance-product adoption dynamics, the moat ("the standard") has no forcing function and is contradicted by the pitch's own vendor-behavior prediction, and the monetization act (compliance attestation) requires attribution integrity and capture continuity the architecture cannot provide even in principle (F3); what remains fundable is not visible until the Phase 2 self-healing MCP loop proves out in strangers' repos, so the correct instrument is zero dollars and a calendar reminder — build it as an open-source experiment against the fixed spec, and come back when an unaffiliated team has both `claude-code` and `codex` turns in one committed log.
