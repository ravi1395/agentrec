# HANDOFF — 2026-08-01 — agentrec MVP delivery (two branches)

Next session's job: deliver the MVP fix set as **two branches**, via ultracode.
This session ran red team round 2 + a Fable skeptic gate + the MVP assessment. **No code changed.**

> Supersedes the auto-generated handoff written mid-session at 12:15. That version is stale —
> it said the skeptic was still pending and PR #11 was unmerged. Both are now resolved (below).

---

## Provenance — the analysis IS an analysis of `main`

Run on the working tree of `fix/redteam-immediate-actions` @ `9ff1b19`, which is
**content-identical to `origin/main` @ `08cf0d9`**. Verified with `git diff origin/main HEAD`,
**no exclusions → empty**: every `.rs` file, test, golden, README, `PROTOCOL.md` and
`IMPLEMENTATION.md` the agents read is byte-identical to main. The branches differ in commit
topology only (PR #11 was squash-merged). Sole working-tree divergence: `.claims/claims.jsonl`
and a 16-line claimd bookkeeping edit in CLAUDE.md's standing-debts section — neither is product
code, and neither underlies a finding (checked against F20 specifically; its cited text is
unchanged from main). **Do not re-run the red team against `main` — it would read identical bytes.**

## Done (verified)

- **Red team round 2 complete.** 6 parallel adversarial passes (attribution, data-loss,
  security/privacy, threat-model/evidence, operational/product) **+ a divergence audit against the
  Sutra reference `~/Projects/sutra/src-tauri/src/turns.rs` that had never been run before.**
  31 findings; every promoted one re-verified against source by the orchestrator.
  — evidence: `docs/redteam/2026-08-01-redteam-round-2.md` — **UNTRACKED. Commit it first.**
- **Fable skeptic gate returned.** Fresh context, instructed to refute.
  **26 SURVIVE / 4 severity-inflated / 1 UNVERIFIED / 0 refuted outright.** Independently
  reproduced the orphan fraction (43.4%), `.remember` share (83.4%) and disk figures.
  — evidence: "Skeptic gate" section of the report.
- **Test baseline verified honest: 666 passed / 0 failed / 3 ignored**, matching CLAUDE.md exactly.
  — evidence: `cargo test --workspace -- --test-threads=3`.
- **PR #11 is MERGED to main as `08cf0d9`.** — evidence: `gh pr view 11` → `MERGED`
  2026-08-01T01:15:24Z, and `git diff --stat origin/main HEAD -- . ':(exclude).claims'` → **empty**.
  The squash captured everything; `fix/redteam-immediate-actions` can be abandoned.
- **Nightly torture gate real and green** — 16+ consecutive daily runs, latest
  `2026-08-01T08:01:25Z` success (after the PR #10 merge).

## In progress

Nothing half-edited. No source file was modified this session.

`git status --short` (verbatim):

```
 M .claims/claims.jsonl
 M CLAUDE.md
?? .claims/objects/77/
?? docs/redteam/
?? f.txt
```

- `.claims/claims.jsonl`, `.claims/objects/77/` — dirty from this round's verify runs. Pre-existing.
- `CLAUDE.md` — pre-existing modification from the prior round, not made by this session.
- `docs/redteam/` — **this session's deliverable. Commit it.**
- `f.txt` — 2-byte stray (`y\n`) at repo root. Scratch; safe to delete.

**Exact next step:** `git fetch origin && git switch -c fix/mvp-periphery origin/main`

---

## Branch plan

**Base BOTH branches on `origin/main` (`08cf0d9`)** — not on `fix/redteam-immediate-actions`,
which is content-identical to main.

### Branch 1 — `fix/mvp-periphery` — "safe to install". Ships first, independent of branch 2.

| # | Finding | Change | Where |
|---|---|---|---|
| 1 | F1 | README threat-model section: defends against buggy agent / careless human / crash / bit-rot; does **not** defend against a malicious or prompt-injected agent or repo — both run as your uid. **Doc only.** Honest text already exists at `REVIEW.md:53`. | `README.md` |
| 2 | F24 | Add `StandardErrorPath` + `StandardOutPath` to the launchd plist. 24 `eprintln!`s go nowhere on macOS today — including the only record that eviction deleted history. **Precondition for every other operational fix being observable.** | `cli/src/service.rs:99-121` |
| 3 | F8 | Route `entry.path` and `tool` through `fmt::sanitize_terminal`. A crafted filename can erase the `revert` line from the `undo` confirmation you review before `--confirm`. | `cli/src/readcmds.rs:854-869`, `cli/src/fmt.rs:228-262` |
| 4 | F11 | Move `purge_prompts` behind the daemon-liveness check; give it the archive its four siblings have. Today `purge --orphans` with the daemon live **deletes prompt blobs, then refuses** — the user reads the refusal and believes nothing happened. Unarchived, unrecoverable. | `cli/src/purgecmd.rs:41`, `:217` |
| 5 | F27 | Compare record/signal `v`; refuse an unknown major. Zero runtime enforcement today — an old binary parses a `v:2` record as v1. **Cheap now, expensive after the protocol 1.0 freeze.** | `agentrec-core/src/record.rs` |
| 6 | F13 + F31 | `status` must report **all** gap kinds (not just `Crash`) plus daemon liveness in the *text* report. Today `kill` → damage → restart reads `gaps: 0`, and a dead recorder makes the inbox look *healthier* the longer it's dead. | `cli/src/cmds.rs:431`, `:492`; `agentrec-core/src/view.rs:89-96` |
| 7 | F26 + F28 | Budget must count only what the evictor can reclaim (43.4% of the live store is orphaned — counted toward budget, ineligible as victim), **and** add a `config.toml` budget key. Unsettable in release today (`#[cfg(debug_assertions)]` env var only). | `cli/src/cmds.rs:910-918`, `agentrec-core/src/retention.rs:59`, `view.rs:750-762` |
| 8 | **F2** | **Symlink undo. PROTOCOL-COUPLED — land before the 1.0 freeze.** Add a link-kind field to `FileEntry`; make `restore_from_before` refuse when the entry is a link or the path is currently a symlink. Today undoing a deleted symlink writes a *text file* containing the target path (unrecoverable — no link field in the wire format), and `--allow-modified` truncates the *pointed-to* file, which was never in the plan, while readback verification passes and prints success. | `agentrec-core/src/record.rs:65-97`, `cli/src/readcmds.rs:1016-1042`, `:989-992`, `cli/src/daemon.rs:1249-1259` |
| 9 | F6 + F7 + F9 | Secrets, as **one coupled unit** — F9 (no path-targeted purge) makes every miss permanent, so the path-scoped reclaim must land with the table edits. Scrub passes AWS *secret* keys, `postgres://user:pass@`, `sk-proj-`, Bearer tokens. Denylist misses `.netrc`, `.git-credentials`, `.pgpass`, `.npmrc`, `kubeconfig`, `terraform.tfstate`, `id_dsa`/`id_ecdsa`, `secrets/*.yaml` (directory names never consulted). | `agentrec-core/src/scrub.rs:8-46`, `:143-174`; `cli/src/purgecmd.rs` |
| 10 | F19 | Extend the torture harness to imported turns. `rg 'import\|synthesized\|baseline_unknown' cli/tests/torture.rs` → **0 hits**, so the stated launch gate cannot reach the one class where `before` bytes are *derived* rather than observed. Gates any "undo is safe" claim. | `cli/tests/torture.rs` |

### Branch 2 — `feat/mvp-promise` — the attribution wedge. Merge **after** periphery.

| # | Finding | Change | Where |
|---|---|---|---|
| 1 | F3 | Git classification. Stop **hiding** converted turns by default; stop panic-undo skipping them; only classify as git when the burst *begins after* the ref transition. Today edit-then-`git add`/`commit` — or merely a `git status` from a shell prompt / IDE poll (see gotchas) — relabels your burst `tool:"git"`, hides it from `log`, makes `blame` say `git · "—"`, and makes panic `undo` silently target an older turn. Keep the classification; checkout floods are real. | `agentrec-core/src/engine.rs:230-245`, `view.rs:872`, `cli/src/readcmds.rs:648` |
| 2 | F4 | Session-aware bracket matching. `observe_stop` matches on tool only, never compares `session`, so one Claude Code session's Stop closes another's bracket with `truncated: false`, carrying the wrong prompt and session onto the other session's files. | `agentrec-core/src/engine.rs:292` |
| 3 | F5 | Bracket deadline idle-anchored, not lifetime-anchored. Sutra: `now - last_change` @ 15 min, commented *"never pre-empt a live agent"*. agentrec: `now - opened_at` @ 2 h — so a dead agent's bracket absorbs up to 2 h of the human's edits. | `agentrec-core/src/engine.rs:379`, `agentrec-core/src/lib.rs:33` |
| 4 | **D6** | **The product. Transcript correlation.** Intersect transcript-declared writes for a bracket against fs mutations observed in that bracket: declared → attributable to the agent; undeclared → unattributed **at the file level**, excluded from `undo`'s default plan, rendered as such by `blame`. **Built from parts already shipped:** `SignalEvent.transcript` already reaches the daemon at Stop (`record.rs:23`), and P1's importer already extracts per-edit `toolUseResult.filePath` from exactly those files (`importcmd.rs:607-608`, `:845`). Note `ChangeObs` (`engine.rs:13-29`) has **no timestamp or ordering index** — `observe_changes` takes `now` and discards it per file. | `agentrec-core/src/engine.rs`, `cli/src/daemon.rs`, reuse `cli/src/importcmd.rs` |

---

## Decisions + why

- **Two branches, periphery first.** Periphery is independent and mostly hours-to-days; promise is
  weeks and touches the engine. Periphery makes the tool safe to install while the wedge is built.
  **F2 forces the sequencing** — it needs a wire-format field, so it must land *before the protocol
  1.0 freeze*, which is why it sits in periphery despite being the most invasive item there.
- **"MVP" was ambiguous and the ambiguity changed the answer.** Three readings: *safe-to-install*
  (not yet, ~6 items), *delivers-the-promise* (**no — fixing all 31 findings does not achieve it**),
  *launch-ready* (Phase 0 demand gate never run). The founder is asking the second.
- **D6 was wrongly excluded from the red team report** as KNOWN-DISCLOSED — correct by the report's
  own dedup rule, wrong by consequence. It *is* the answer to the headline question. The skeptic
  caught this independently; it is now the report's MVP section. **This resolves the "D6 scope
  deferred" question in the superseded handoff: it is a code fix, not disclosure-only**, and the
  repair path is item 4 above.
- **The promise closes from both sides.** With D6 the "agent" answer is wrong whenever you type
  during a bracket; with F3 the "me" answer disappears from `log` whenever git runs. The product
  answers reliably only when the human is idle and doesn't touch git — the case where nobody needed
  to ask. **Until D6 correlation ships, narrow the README claim** from *"who broke my repo"* to
  *what changed, when, inside which agent turn, and can I put it back*.
- **Alternatives rejected:** signing / hash-chaining for F1 — it is v3/L3 and does not help against a
  local adversary running as your uid. F1 is a **disclosure** fix; do not queue it behind code work.
  Per-file timestamps alone for D6 — a human save and an agent write are indistinguishable at the fs
  layer; the transcript is the only distinguishing signal.
- **The engine core is sound.** The skeptic tried to break the store, eviction reachability, path
  handling and append discipline, and could not. What is broken is the periphery and the promise.

### Cuts recommended (founder decision, NOT taken)

- **Memory subsystem default-off** (`memory_enabled = false`). The repo's own ledger closed the
  1-week dogfood row **FAILED** — candidate emitter never fired once in 1944 signal lines. It is why
  `.remember/` is **83.4% of all recorded file activity**, and F10 (durable prompt-injection
  surface) exists only because of it.
- **Defer `import`** until the torture harness reaches it (branch 1, item 10).
- **Drop git-turn *hiding*** (keep classification) — branch 2, item 1.
- **Drop service units for MVP** — 39 orphaned LaunchAgents on this machine, baked once and never
  migrated (F29), and they swallow every diagnostic on macOS (F24). Ship `agentrec record` plus a
  documented one-liner.
- **Keep:** `log`/`diff`/`blame`/`show`/`undo`, `status`, `doctor`, `purge --orphans`.

---

## Test state

```
cargo test --workspace -- --test-threads=3
→ 666 passed; 0 failed; 3 ignored
```

No red tests. Nightly torture green, latest `2026-08-01T08:01:25Z`.

---

## Gotchas

- **`origin/main` was stale in this working copy — `git fetch` first.** Before fetching it looked
  like PR #11 was unmerged and the branch 17 commits ahead. It is merged; content diff is empty.
- **The red team report is untracked.** Commit `docs/redteam/` before anything else or a stray
  `git clean` destroys the entire input to this work.
- **claimd Stop hook fires on normative-doc edits.** `PROTOCOL.md` is *not* lint-ignored (recorded
  in CLAUDE.md as an undecided rule). Branch 1 items 5 and 8 both touch protocol surface — expect
  the hook and budget for declaring claims.
- **CLAUDE.md's Founder-pending list has a STALE entry** (F20): it claims the Homebrew
  canonicalized-exec-path bug is "NOT BUILT". It **is** built — `initcmd.rs:246-266` deliberately
  does not canonicalize. Don't re-fix it. The real open residual is Linux, where `/proc/self/exe`
  makes it unfixable this way, colliding with D39's planned npm/mise shim distribution.
- **Two of the report's own claims were corrected by the skeptic; both accepted and already written
  into the report.** (1) F3's mechanism table asserts `git status` never writes `.git/index` —
  **false**; a `git status` following a commit does, in both parties' measurements. Neither isolated
  the trigger; neither asserts a mechanism. This makes F3's exposed population *wider*. (2) F18's
  "reclaims ~0" clause is refuted by the report's own F26 (43.4% orphaned → `--orphans` reclaims
  43%); F18's valid core is the unquantified "Most… usually" generalized from one store on one day.
- **This repo has a documented signature defect** — a confidently-worded comment asserting a
  real-corpus fact nobody measured. Six instances so far. Probe before writing normative text.
- `.claims/claims.jsonl` + `.claims/objects/77/` dirty from verify runs; normal, not your change.
- Daemon integration tests flake under full-parallel `cargo test` (FSEvents contention) — keep
  `--test-threads=3`.
- **Memory v1 rerun is blocked** on: fresh T0 window + re-pinned baseline, and first an end-to-end
  proof the candidate path fires at all (real agent → `agentrec candidate` → daemon ingests an
  `origin:"agent"` record). The hit-rate criterion is unfalsifiable as written.

---

## Suggested skills

- `/brief` — vault context before planning.
- `/phases`, then `/pchunker` + `/pexec` — branch 1's ten items are mostly file-disjoint and
  parallelize well. Branch 2 item 4 (D6) wants `/phases` with a **spike gate on correlation
  accuracy** before any rendering work.
- **ultracode** — the founder explicitly wants this delivered via multi-agent orchestration. Good
  shape: spec-per-fix → adversarial verify-per-spec → synthesis. Branch 1 is the natural fan-out;
  branch 2 item 4 is not (one coherent design — wants depth, not width).
- `claimd:declare-claims` — declare per AC before implementing.
- `skeptical-reviewer` (Opus, fresh context) — binding done-gate before any "done" claim, per the
  `/ratchet` house rule. Never weaken an AC to pass.
- `/verify-ledger` — VERIFY-LEDGER.md rows before any release claim.

---

## Not started

Everything above. No branch created, no code changed, no fix begun.

Unrelated open plans, untouched by this round:
- Memory v1 ladder — 12 tasks, all open: `docs/superpowers/plans/2026-07-12-agentrec-memory.md`
- Churn honesty round — open: `docs/superpowers/plans/2026-07-25-agentrec-churn-honesty-round.md`

**Full finding detail** — mechanisms, failure scenarios, refutation attempts, verified-clean list,
skeptic verdict table, MVP assessment, cuts:
**`docs/redteam/2026-08-01-redteam-round-2.md`** (F1–F31).
