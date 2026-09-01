# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working in this repository.

## What this is

**agentrec** — a local-first, tool-agnostic flight recorder for coding agents. It segments agent activity into *turns*, snapshots every touched file into a content-addressed store, and answers "who broke my repo — me or the agent?" via `log` / `diff` / `blame` / `undo`. The turn engine is an extraction of the production-proven `turns.rs` from Sutra (`~/Projects/sutra/src-tauri/src/turns.rs`) — when in doubt about engine semantics, that file is the reference implementation.

## Status (update after every delivery round — house rule)

**Slimmed 2026-07-29 at the founder's direction:** delivered-round narratives removed; full
history lives in `git log -- CLAUDE.md` (last full version at `204ff04`). This section now
carries only current state, what's next, and standing debts.

### Current state

- **attest v1 Phase 1 spike — Fable skeptic GATE PASS at `59fbbf7` (2026-09-01, branch
  `feat/attest`, cut from `main`; round 1 FAILED on one record blocker, narrow re-gate PASSED).**
  Findings only, no product code: `docs/verify/attest-coverage-spike.md` (Probe A),
  `docs/verify/attest-output-channel-spike.md` + `docs/fixtures/attest/` (Probe B),
  `IMPLEMENTATION.md` §attest (AC-ATTEST-P1-1..7). Baseline on this branch 1035/0/4.
  Measured: suite-level instrumented run +0.5% wall (quietness not re-verified before it);
  per-test isolated capture extrapolated from a 50-test stratified sample to a ~2–13 min
  range (median-based lower bound to a mean-based upper bound dominated by one 21.5 s test —
  never quote either end bare); spawned `agentrec` child coverage IS attributed to the
  spawning test (measured YES), but a SIGKILLed child writes no profraw and this repo
  SIGKILLs daemons routinely, plus an untemplated-child `.profraw` leak of UNDETERMINED
  mechanism — two under-attribution channels, detection mechanism for neither yet (disclosed
  in the schema sketch). Founder-ratified libtest parser validated on real captured output,
  fails closed on a corrupted fixture. **Round-1 blocker:** the pasted SIGKILL probe script had
  a relative binary path its own `cd` broke, so run as pasted both legs reported a profraw —
  the "reproduced identically" sentence was false of the pasted text (fixed by extracting the
  block out of the doc and running that). Environment facts worth knowing: stock
  `cargo llvm-cov` fails on Homebrew `rustc 1.97.1` (no `llvm-profdata` in the sysroot) —
  export `LLVM_COV`/`LLVM_PROFDATA` from the rustup component; the plan names
  `cli/tests/bisect.rs` as a spawn site but that file lives on `feat/phase-3-0`, not `main`.
  **Exit criterion 7 (coverage granularity ruling) is PENDING-FOUNDER — Phase 2 dispatch
  waits on it.** claimd: `.claims/` is untracked on `main` since `23a2e0d`; the 273-claim log
  lives only on `feat/phase-3-0`; three new claims here are local-only, and claimd's coverage
  lint does not count `manual` claims toward file coverage.
- **PR #20 fsguard remediation round — GATE PASS at `832ad63` (2026-08-07, rounds 8–11),
  pushed.** Round 7 had FAILED the PR on three blockers; this round closed them and survived
  four Fable gates. Code: purge liveness reads hard-error on non-regular files (refusal
  shrinking the protect set would let purge delete live blobs); init (12 sites) / uninstall
  (5) named-error on FIFO config paths, never treat-as-empty (init merges, so empty-on-refusal
  clobbers); importcmd ×4, service ×2, `record::open_append` (reviewer-found HIGH — `hook
  claude` hung on a FIFO `signal.jsonl`, the agent-facing surface) plus ten write-open guards
  (locks, tmp-writers, undo-guard, purge rewrites). `clippy.toml disallowed-methods`
  (fs::read/read_to_string/File::open) makes reintroduction a build failure — proven live by
  plant-probe (exit 101), partition of all 24 in-src allows enumerated in the ledger. Tests
  +9 incl. the daemon FIFO regression test (asserts skipped entry AND control-file survival —
  the neutered guard loses the whole recorder) — its fixture replaces a watched regular file
  with a FIFO for determinism. Suite 1019/0/4; clippy `--all-features -D warnings` + fmt clean.
  - **Rounds 8–10 each failed on PROSE only, never code — three of the false sentences were
    the orchestrator transcribing a skeptic's claims unverified** (the recorded gate-loop
    lesson, hit again): a `Create(_)` mechanism citing a cfg'd-out Linux arm, a "2/2
    byte-written" figure round 9 could not reproduce (0/3; 2/2 only with an explicit touch —
    stimulus UNDETERMINED, both measurements now stated without adjudication, probe shapes
    persisted in `scripts/probe-fresh-fifo-delivery.sh`), and a "(both rounds)" attribution
    for a shape round 9 never ran.
  - **Known-open, disclosed not fixed:** OpenOptions/fs::write/fs::copy lint gap (write-open
    class; store.rs:91 named); service.rs unit-file write outside the repo threat model;
    darwin-only lint census (Linux cfg bodies unlinted locally); nine of ten write-open guards
    evidenced only by the green suite; fresh-FIFO delivery stimulus undetermined (script
    settles it when someone runs the matrix); round-8/9/10 probe outputs not persisted to
    `docs/verify/` (founder option if gradeable figures wanted).
- **Phase 2 tail execution started (2026-08-05, branch `feat/phase-2-tail`, forked from
  `main`@`dd4238f`, worktree `~/Projects/agentrec-phase2-tail`) — orchestrated: Sonnet
  implementers per task, Fable skeptic gate per phase, no phase advances without GATE PASS.
  Plan: `docs/superpowers/plans/2026-08-05-phase-2-tail-plan.md` (lives on
  `docs/user-onboarding`, not yet merged to `main`). Baseline re-measured at fork: 751/0/3.**
  **Phase A (Codex hook spike) GATE PASS @ `64f2bcf`.** Live-probed pinned `codex-cli 0.146.0`
  (not inferred from docs): headline finding `turn_id` is STABLE across a `Stop`
  `decision:"block"` continuation (2/2 runs) — unblocks decision 17's drain-by-`turn_id`
  keying, no founder escalation needed. Trust hash-invalidates on both command AND
  non-command (`timeout`) field changes; `hooks.json`+inline `[hooks]` merge+warn confirmed;
  `PostToolUse apply_patch` `tool_input.command` is raw patch-DSL text (pins the path-
  extraction rule); `codex exec` silently skips untrusted hooks with **zero** warning (gap
  beyond the docs, recorded). Fixtures + field inventory + trust writeup:
  `docs/verify/codex-spike.md`. Gate found 2 non-blocking prose overclaims (fixed same round).
  **Phase B (toml config loader) GATE PASS @ `0c6c428`, after 2 fix rounds — closes the
  onboarding round's recorded debt above** (`mcp_destructive` now read; invalid TOML now a
  real hard error on CLI verbs + daemon startup). `cli/src/config.rs`: `McpDestructive`
  enum + `Config`/`load()` (D16 line-numbered hard error), all 5 legacy read sites migrated,
  scanner deleted. **Round 1 GATE FAIL:** implementer routed every production call site
  through a tolerant `load_or_default`, making the hard-error path unreachable — silent
  scope-narrowing of an explicit, pre-decided plan constraint, not founder-ratified. Fixed:
  CLI verbs now propagate real errors; daemon hard-fails once at startup, stays tolerant
  mid-tick (avoids the repo's own documented launchd `KeepAlive` respawn-loop scar).
  `memorycmds`' fail-open reads deliberately NOT migrated — collides with the pre-existing
  pinned INV-M4 "hook always appends signal" invariant; gate ratified this as sound,
  disclosed, non-blocking. **Round 2 GATE FAIL (narrow):** the fix's new mid-tick-survival
  test and its commit message overclaimed proving the *budget* path degrades gracefully;
  it actually only exercised a different read (`read_memory_enabled`) because of a debug
  env-override seam bypassing the budget path entirely — signature-defect-shaped (confident
  claim, no measurement behind it), caught by the gate rather than shipped. Round 3 fixed
  the false claim and added a real discriminating test (real on-disk `store_budget_bytes`,
  no env override) — the skeptic independently reproduced the mutation probe (old test
  blind to a forced hard-error regression, new test catches it). Suite 751→762/0/3;
  clippy+fmt clean debug+release; no test seams in release `strings`.
  **Phase C (Codex 2.1 adapter) DONE — task ACs C1–C4 GATE PASS, and Phase C EXIT REACHED
  @ `7212ba8`.** C1 `emitter_turn` + restart-safe dedup; C2 `agentrec hook codex` (three-event
  emitter + `files_written` accumulator); C3 init/doctor/uninstall codex hook management;
  C4 `import codex` (K-series, real-corpus derived: 616 rollout files, 100% importable).
  **Two founder-ratified mid-phase fixes:** content-aware dedup — Codex's `Stop` fires twice
  with an identical `turn_id` on a block-continuation and identity-only dedup was silently
  dropping the second WITH its new `files_written` (real attribution data loss) — and a
  `model` field on `SignalEvent`, without which Codex rich turns shipped with zero model
  attribution. **O5 met live** (real `claude -p` session + real interactive Codex session
  through the un-bypassed `/hooks` trust flow; sequential, not simultaneous; zero
  cross-attribution by jq-scoped queries). **O5 found a blocking product defect, fixed
  @ `3c4f598`:** Codex spawns hook processes with cwd = its LAUNCH directory, not the repo
  root, and the installed entry is bare — so a subdirectory launch silently minted a second
  `.agentrec/signal.jsonl` under `sub/`, invisible to the daemon, no error. Fixed by walking
  up to the nearest `.agentrec/` (git-style), **both emitters**, chosen over baking an
  absolute `--root` (that is this repo's 39-orphan scar). **`3c4f598`'s own message then
  carried an unmeasured claim** — that Claude Code pins hook cwd to the project root, making
  its defect "latent" — **measured FALSE** (`ecd09bc`): Claude inherits the launch dir too, so
  that defect was real and live. claimd gap **founder-waived** (`baeee84`): nothing in Phase C
  is claim-attested. Suite 762→866/0/3.
  **Phase D (Protocol 1.0 freeze) GATE PASS @ `8d8e1db` (Fable, 10/10 ACs).** D0 landed the
  D7 dropped-signal count on the epoch-start record (decision 51) first as a durable marker;
  the freeze commit then stamped `PROTOCOL.md` **1.0 (frozen 2026-08-06)** with 29 conformance
  fixtures (`docs/fixtures/conformance/{valid,tolerated,invalid}` + manifest test
  `cli/tests/conformance.rs`). Founder review of the freeze diff is asserted in `8d8e1db`'s
  commit message; **no ledger row records it** (provenance noted at plan exit — founder
  attestation welcome). Suite 866→876/0/4.
  **Phase E (MCP read 2.2) GATE PASS @ `d5b3deb` (E-commits `f48b519`/`75ae1de`/`d851408`/
  `f4a96c3`).** `agentrec mcp`: stdio JSON-RPC server, five read tools; `diff`/`blame`
  payloads byte-equal to `--json` (parity-pinned), `log` returns typed `Page<TurnSummary>`
  (P4b decision 12), `recall` returns `RecallPage` (delta 14). PROTOCOL delta across the
  whole phase = exactly the additive §8 `agentrec_status` row (first post-freeze additive
  change). Suite 876→926/0/4. Residuals: no real MCP host has spoken to the server yet;
  Codex repo-local `mcp_servers` honoring unconfirmed.
  **Phase F (MCP destructive 2.3) GATE PASS @ `670fba3` — 4 rounds; rounds 2–3 failed on
  RECORD defects only, never code.** F1 mode gating + matrix (`e264d7c` fixed §8
  `allow_modified` text first — delta 15); F2 `UndoCoordinator` preview + path reservations;
  F3 confirm-mode request ledger + `agentrec approve`/deny (D23 10-min expiry); F4 auto-mode
  two-phase tokened execute (spend-before-write, crash matrix); F5 `origin` cli/mcp
  discriminator (delta 11). Finding 11(b) fix `e32d247` (mid-revert failure-path persistence
  surfaced + fault injection). **Blocker closures:** self-healing E2E run LIVE on the release
  binary (`d8c69e3`, `docs/verify/f-exit-self-healing.md` — confirm + auto legs, token-hash
  cryptographic tie, teardown 5→5; agent identity simulated via hook emitter, disclosed);
  finding 13 (unbounded `undo-requests.jsonl`) recorded `78fbc36`. Gate-round-2/3 record
  fixes `64f6b68`/`670fba3`: false `reserve`+`approve` join corrected to `request`+`execute`;
  false flake name `hook_kill_9_closes_open_epoch` provenance-pinned (never existed — real
  recorded flake is `approve.rs::a_killed_approve_never_leaves_a_phantom_approval`, 1/26 and
  2/~14, disposition founder-owned); seam-sentence over-claim fixed. Suite **987/0/4**
  (measured 4×), clippy debug+release + fmt clean, release-seam grep zero hits.
  **Standing debts (recorded, founder-owned):** coordinator-layer `allow_modified` is
  downgrade-with-warning, not refusal — §8's refusal exists only at the transport rail;
  `undo-requests.jsonl` unbounded (reclaim needs a decision-register entry); Phase D founder
  review unledgered; kill-9 flake disposition.
  **Next: plan-exit checklist (in progress), then merge vehicle for `feat/phase-2-tail`.**
  **O5 Phase C exit criterion re-run LIVE (2026-08-05):** the prior round's Claude leg was
  direct hook-invocation, not a live process — closed by a genuine `claude -p` session (both
  legs re-driven live, one `log.jsonl`, zero jq-scoped cross-attribution). Along the way it
  found the prior round's `files:[]` diagnosis was mis-attributed (D6/`transcript_path` were
  both true facts but neither was the actual cause — a controlled repro pins it to a
  fs-watcher/bracket-timing race instead, a new undocumented defect surface) and falsified
  `3c4f598`'s "Claude Code sets cwd to the project root" assumption (measured: it doesn't; the
  walk-up fix earns its keep for claude for the measured reason, not the assumed one). Full
  writeup: `docs/verify/o5-two-tool-session.md`; ledger: `VERIFY-LEDGER.md` § "O5". Not fixed,
  not gated by a skeptic — verification only, recorded for founder disposition.
- **User-onboarding docs round (delivered 2026-08-04, branch `docs/user-onboarding`).** README
  restructured for new users: quickstart (`init` → `import claude` → `doctor` → read verbs),
  mermaid architecture diagram, How-it-works glossary (rich/bare, bracketing, epochs, git turns),
  config reference covering exactly the 5 keys the hand-rolled parser reads, troubleshooting,
  docs index + versioning note; the long skeptic-gated command-table cells were preserved
  verbatim inside `<details>` blocks, not rewritten. Install paths re-verified live before
  keeping them: npm 0.2.0, crates.io `agentrec`+`agentrec-core` (both published 2026-08-04),
  brew tap 0.2.0. `import` help de-jargoned ((P1)/(P2) removed; 36/0/1 import tests green, no
  golden asserts help text). ROADMAP got a status note (import/npm/crates/plugin shipped ahead
  of phase labels); SPEC's stale `agentrec.dev` install line and phantom `service` verb fixed;
  REVIEW.md gained a reader note (it stays at root — too widely cited to move);
  `HANDOFF.md`+`HANDOVER-PD-FIXES.md` → `docs/internal/`; SECURITY.md added.
  **Found+fixed: today's squash `23a2e0d` (via f6cadea's untrack of `docs/`) had removed
  `docs/blame-demo.gif` from the remote, breaking README's demo image on GitHub — the three
  demo files are re-tracked; the rest of `docs/` stays local per the untrack decision.**
  The J1/J2/I3 config debt recorded here (mcp_destructive read by nothing; invalid TOML
  silently tolerated) was CLOSED by Phase B's config loader (`0c6c428`, 2026-08-05) — see the
  Phase 2 tail bullet above. The user-supplied scrub-rule key still does not exist.
- **Release-version gate + `agentrec-release` skill (delivered 2026-08-04, UNCOMMITTED on
  `main`, not gated by a skeptic, never exercised by a real tag).** Closes the
  "4 files bumped by hand, no CI enforcement of match" gap recorded at v0.2.0. Three
  pieces: (1) `.github/scripts/check-versions.sh X.Y.Z` asserts **eight fields across six
  files** — both workspace crate versions, the `cli/Cargo.toml` → `agentrec-core` dep pin,
  both `Cargo.lock` entries, `npm/package.json`, `.claude-plugin/marketplace.json`
  **`.metadata.version`** (NOT `.plugins[0].version` — that is claude-mem's layout, not
  ours), and the plugin manifest; (2) a `verify-version` job that `build` now `needs`, so
  drift fails in ~30s before four matrix builds burn, plus `--locked` on the release build
  so a stale lockfile is a hard CI failure (verified clean at `v0.2.0` BEFORE being added);
  (3) `claude-setup/skills/agentrec-release/SKILL.md`, which the global `ship` skill's
  step-0 deferral hands off to. **Fields are read structurally (`cargo metadata` / `jq`),
  never by grepping the version string** — `cli/Cargo.toml` carries `regex = "1"` beside
  our pin, so a string grep false-matches the day a dep version collides with ours.
  **Founder decisions taken here:** CI verifies, the skill bumps (no CI commits, no CI
  write to `main`); registry publishing stays a human hand-off (agent holds no tokens).
  Evidence, all run: gate green at HEAD (8/8 `ok`); **per-site mutation probes on five
  sites** (`npm/package.json`, `marketplace.json`, `plugin.json`, the `cli/Cargo.toml`
  pin, `Cargo.lock`) each producing exactly ONE `MISMATCH` naming that site — an
  all-sites-fail probe alone cannot distinguish a wired gate from one that ignores files;
  wrong-version → exit 1, non-semver arg → exit 2; ref-shape guard rejects `main`, `v0.2`,
  and a branch name; `actionlint` clean. The count is measured repo-wide
  (`git grep -n '0\.2\.0' | grep -v CHANGELOG`), not from the scoped grep that found the
  sites — earlier prose saying "seven places" was imprecise and is corrected.
  **The script is invoked as `bash .github/scripts/check-versions.sh` deliberately**, in
  the workflow and the skill both, so the gate cannot depend on git recording mode
  `100755`. **Residuals, recorded:** never run against a real tag (first exercise is the
  next release, and the workflow edit + script MUST land in one commit — a commit carrying
  the workflow without the script fails `verify-version` on every tag); `.claude/` is
  gitignored, so the installed skill copy is machine-local and `claude-setup/skills/` is
  the tracked source; the skill relaxes `ship`'s VERIFY-LEDGER precondition to rows scoped
  to the version being cut, because this repo's long-lived recorded-not-blocking debts
  would otherwise block every release forever.
- **D6 attribution wedge, phases 1–2b (delivered 2026-08-01) — branch `feat/mvp-promise`, 15
  commits, unmerged; FULLY GATED after two skeptic engagements (first gate on 2a+2b: PASS
  9/9; scoped re-gate on the post-gate delta: 3 rounds, PASS at `94c3d59`).** Spike gate
  (phase 1) measured declared-write coverage on the real dogfood corpus: work-file coverage
  14.5–24.4%, precise-where-present (88% full-cover), hook-artifact false-match 0.1%, F4
  measured live (739/803 joined sessions are hook-only stubs — correlation must key off the
  CLOSING signal at Stop-time, never persisted `turn.session`). Founder ruled PROCEED as
  option 1: additive `files_written` on stop signals (PROTOCOL §4, landed — freeze-critical),
  emitter computes the list scoped to the last user prompt, daemon `resolve_declared` tier
  ladder (SignalField → TranscriptFallback → None) landed dark. Suite 677/0/3 (base 666),
  clippy debug+release + fmt clean. Value-set contract relayed to the periphery worktree
  (`D6-ATTRIBUTION-CONTRACT.md`: "declared"/"undeclared"/absent — their `attribution` field,
  our values). **The three-round re-gate found zero code defects; all three blockers were
  confidently-false PROSE — signature-defect instances 6–8, one of them the skeptic's own
  miscount copied verbatim (provenance recorded in docs/verify/d6-spike.md).** Phase 3
  (producer at persist) gated on: periphery merge → rebase; its ACs now include wiring-proof,
  out_of_root surfacing, and contract-pinned wire values. Open, recorded: >64 MiB cap path
  never executed (tier-shift is inspection-only), 14-transcript cutoff base, no Linux leg,
  `6c86e3a` AC-restatement awaiting founder ruling, 2b claims + 176-stale verify deferred at
  founder direction.
- **MVP periphery round (delivered 2026-08-01) — branch `fix/mvp-periphery`, merged to `main`
  via PR #13 squash (`47799f4`, 2026-08-02; CI 5/5 green). Fable skeptic GATE PASS at
  `7dd764e` (3 rounds).** All ten BRANCH-SCOPE items landed: F24 launchd stdio→`.agentrec/daemon.log`; F1
  README threat model (the redteam's §5-suppression mechanism claim was refuted in source and
  dropped); F8 undo-plan + turn-renderer sanitization; F11 purge liveness-before-delete +
  prompt archival; F27 schema-major enforcement (SCHEMA_MAJOR=1, every wire deser site, §10
  consumer clause); F13+F31 status all-gap-kinds + daemon liveness; F19 torture × imported
  turns (rides the nightly `--ignored` gate); F26+F28 budget keys on evictor-managed bytes +
  `config.toml store_budget_bytes`; F2 symlink undo refusal + **FileEntry gains `link_kind` AND
  `attribution` in one additive change** (attribution written by nothing here — it is
  `feat/mvp-promise`'s D6 producer field, per the cross-branch schema agreement; PROTOCOL §5 +
  FORMAT-CHANGELOG entries); F6+F7+F9 scrub shapes + dir-component denylist + `purge --path`
  (blob archival, NOT a fourth rewrite class). Register row D50. Suite **738 / 0 / 3**
  (baseline 666/0/3); clippy `-D warnings` + fmt clean; release seam check 0 hits; release-only
  budget test run explicitly. Gate rounds 1–2 caught 2 real blockers, both same-branch seam
  drift: F2's `link_kind` interpolated unsanitized into the REFUSE row (falsifying F8's
  fresh "probed" comment — signature-defect instance #7, now fixed + test-pinned) and F1's
  line-number citations rotted by later same-branch commits (now file:symbol). **Skeptic
  remaining risks, unverified by anyone:** nightly `--ignored` torture leg never run on this
  HEAD in CI; launchd honoring the new plist keys is manual-only (install on a scratch repo,
  confirm `daemon.log` receives daemon stderr). **Next: PR to `main`; then `feat/mvp-promise`
  rebases and builds its attribution producer.** Founder-pending here: `./`-prefix
  normalization for `purge --path` patterns; `--allow-modified` vs derived-`after` on imported
  turns (pinned-as-observed, founder ruling candidate); F11's `--snapshots-before` still
  unarchived (scope call, disclosed).
- **Redteam remediation round (delivered 2026-08-01, extended by founder acceptance of all
  recommendations; 7 skeptic rounds total) — branch `fix/redteam-immediate-actions`, 15
  commits, unmerged.** Extension added: PROTOCOL §3 append-only definition + D48 reclaim
  clause; D46→D48 renumber (collision with PR #10 resolved); D49 bare-turn caution; full
  stale-claims verify (73 confirmed). Claim-authorship lesson recorded: 7 replay refutations
  this round were all claim-cmd bugs (stale counts, overbroad greps, multi-filter cargo
  invocations match only one name), zero code defects — superseded with corrected replays. External redteam (technical +
  product) drove 4 immediate actions: (T1) README discloses D6 intra-bracket misattribution +
  silent-undo consequence; (T2) E2E test pins the D6 data-loss chain against the real daemon,
  `undo` gains a CAUTION activity-window line scoped to revert-marked files (mixed-plan-true;
  bare turns get their own unattributed-window variant — D49, founder-directed 2026-08-01); (T3) `purge --signals-consumed` — third sanctioned rewrite class
  (D48), inbox was 13.4 MB unbounded — plus `status` inbox accounting; (T4) memory 1-week
  dogfood ledger row **CLOSED FAILED** with per-conjunct evidence (candidate emitter never
  fired; hit-rate unfalsifiable as written). Gate found and fixed 2 REAL daemon defects: startup
  never detected a shrunk/missing inbox against a stale persisted offset (silent tail loss, now
  resync+persist+DEGRADED at both startup sites) — and 3 successive rounds of normative-text
  falsity around state.json deletion, killed only by probe-first writing (deletion mints NO
  duplicate turns — D7 drops start/stop in the gap; real loss is silent as-if-consumed drop).
  Test baseline was **465 / 0 / 1** pre-merge; after `origin/main` (PR #10's squash,
  `a90e9ff`) was merged into this branch the combined baseline is **666 / 0 / 3** —
  status goldens regenerated additive-only (+1 inbox text line; +`signal_bytes`/
  `signal_consumed_bytes` JSON keys, all pre-existing keys byte-identical), and this
  branch's test fixtures gained PR #10's new `TurnRecord`/`FileEntry` fields as `None`.
- **D46 SERVICE-LEAK FIX delivered (2026-07-31) on `feat/phase-2-0-view-completion`, after the
  Phase 2.0 plan exit. Not gated by a skeptic yet; not pushed; no PR.** Closes the *product-defect*
  half of the 40-orphaned-LaunchAgents founder-pending entry: `init` no longer installs a
  `RunAtLoad`+`KeepAlive` unit under a temp root (with `--service` to force), and `doctor` reports
  units whose `--root` vanished as advisory-`pass`. `service prune` deliberately NOT built (only
  piece that shells `launchctl`; destructive; plist removal is founder-reserved). ACs S1–S9 added
  to `IMPLEMENTATION.md` §4 block **S** + register row **D46**; 9 claimd claims (7 CONFIRMED, 1 REFUTED-and-fixed, S9
  DECLARED-manual by design — it pins a moving corpus; see Founder-pending for the refutation). Suite **635 / 0 / 3** (from 615/0/2);
  clippy+fmt clean debug **and** release; new `AGENTREC_TEST_SERVICE_DIR` seam absent from release
  `strings`. **The 40 existing plists are untouched** — still 41 installed, re-counted after the
  full test run. Evidence incl. real-corpus run and five neuter proofs: `VERIFY-LEDGER.md` § "D46".
- **PHASE 2.0 PLAN EXIT REACHED (2026-07-31) on `feat/phase-2-0-view-completion` at `88c7e9b`
  + this commit. P5 GATE-PASSED; all 9 plan-exit checkboxes verified. Pushed, no PR.**
  P5 shipped `--json` read contracts for `diff`/`blame`/`status` as adapters over P4b's typed
  values — `Serialize` on `RepositoryHealth`/`Page<T>`/`Cursor`/`DiffResult`/`FileDiff`/
  `FileDiffState`/`BlameResult`/`BlameState`; `status --json` now flattens the pre-existing
  operational payload with the exact `RepositoryHealth` the view returned. Test baseline
  **615 / 0 / 2** (was 602/0/2 at P4b), 13 new `json_contracts::` integration tests, **33
  goldens**. Fable skeptic in an isolated worktree: **GATE PASS, 12/12**. The **plan exit
  itself** then took its own binding Fable-skeptic round in an isolated worktree at `46e5bf0`
  — **GATE PASS, 9/9**, both substitutions judged defensible only after the skeptic verified
  their premises in source (not on the orchestrator's word), and the moving-corpus honesty
  note reproduced independently: its own real-corpus re-run measured 1937/1944 = 99.6%
  against this round's 1930/1937 = 99.6% — same ratio, different absolutes, hours apart.
  Its five findings are in the ledger; three were corrections to this repo's own record and
  are fixed, two are carried as debts (a claim whose replay is narrower than its text; three
  replays hardcoded to this worktree path). Plan-exit checklist
  verified by the orchestrator independently: hard gate retired at the final commit (99.6%
  importable, denominator re-measured to 1937), zero-bytes dry-run re-verified, gap logic
  proven unified (one `view.rs::has_gap_after`, zero copies in `cli/src`), suite/clippy/fmt
  green on debug **and** release, no test seam in release `strings`, scope honesty clean
  (`PROTOCOL.md` untouched, zero MCP/Sutra paths). Full evidence:
  `VERIFY-LEDGER.md` § "Phase 2.0 plan exit" and the plan's own Final-acceptance section,
  each checkbox now carrying its verdict inline.
  - **Two plan-exit ACs were founder-ratified substitutions, not passes-as-written. Both are
    recorded in the plan rather than quietly satisfied:**
    1. **Item 4 (and P5's own AC-2) named an impossible event.** "...while bare `status` in
       the same fixture **still evicts**" cannot hold — the perf-evidence round moved eviction
       to the daemon tick; `status` is read-only now and a test pins that. Substituted with a
       **parity** assertion: *both* `status` and `status --json` must be zero-write. Stronger,
       not weaker — a write reintroduced on either path still reds. Re-adding eviction to
       `status` to satisfy the original text is forbidden.
    2. **Item 2 collided with P5's AC-1.** "Every P3 golden byte-identical" could not coexist
       with "route `status --json` through `RepositoryHealth`", which necessarily adds that
       struct's fields. Scoped to **human-form** goldens (absolute), with `--json` goldens
       permitted to change **additively only**. Verified: exactly one modified golden
       (`status_json.golden`, a `--json` golden), all 11 pre-existing keys byte-identical,
       **+7 keys, 0 removed**; six recall goldens *added*, none modified.
  - **Open questions 1–3 are carried forward UNANSWERED, explicitly** (founder ruling: invent
    no answers). Q1 wave-2 cut, Q2 the memory plan's 12 unchecked boxes vs memory v1 recorded
    merged, Q3 store reclaim. Disposition table in the plan. All three founder-owned.
  - **Residual closed by P5:** `cli/src/readcmds.rs` was in P5's edit scope, but the `load_log`
    direct-call residual below was **not** part of P5's ACs and is **not** closed — it stands.
  - **Sequenced next (unchanged by this exit):** Codex 2.1 → Protocol 1.0 freeze → MCP read
    2.2, which mirrors these exact serializers. No MCP code exists in this plan by design.

- **P4b EXECUTED and GATE-PASSED (2026-07-30) on `feat/phase-2-0-view-completion`.** Closes
  `RepositoryView` against the parent spec's six-method list (`open`/`list`/`diff`/`blame`/
  `recall`/`health`) — P4 had left `diff`/`blame`/`recall` unimplemented by design; P4b lands
  them plus `DiffResult`/`DiffError`/`BlameError`/`RecallPage`/`RecallError` and a shared
  `select_turns` walk (`list_records`/`list_records_of`, with `list()` surviving as a
  delegating wrapper). Test baseline **602 / 0 / 2** (was 595/0/2 pre-P4b-4); **33 goldens**
  (not 32, not 21 — the stale `21` was corrected in `P3.md`/`P4.md`/`P5.md`, and a stale `32`,
  which P4b-5's gate caught surviving in `P4b-1.md` through `P4b-5.md`, was corrected there too;
  32 was already wrong from `9d30e51` onward, when P4b-1's own gate added a 6th golden mid-task).
  Manual E2E: all 9 steps pass, pre- vs post-P4b binaries byte-identical on both a frozen
  dogfood clone (steps 1/2/4/5/9) and the live dogfood repo read-only (steps 6/7/8) — **caveat:
  step 7 (`recall --json`) is weakly discriminating**, the dogfood store has 0 fresh/3 stale
  memories so both binaries print `[]`. **P5 is now executable** — its dependency (P4b's typed
  values) is satisfied and its `{"files":[]}` literal is corrected (see P5.md amendment,
  derived from the merged `DiffResult`, not observed via a live `--json` flag since `diff
  --json` doesn't exist yet — that's P5's own scope).
  - **Residual, out of P4b's scope, not fixed here:** `cli/src/readcmds.rs:202`, `:426`, and
    `cli/src/memorycmds.rs:985` still call `load_log` directly in production paths — AC17's
    `rg` was scoped to `cmds.rs` only. P5 or a dedicated cleanup task should close this.
- **P2 + P3 EXECUTED and GATE-PASSED (2026-07-30) — merged into `feat/phase-2-0-substrate` at
  `73d01b9`, ledger + hygiene follow-ups at `6fa0de1`. Nothing pushed, no PR.**
  Method: sonnet implementers in parallel worktrees → opus reviewers → a Fable skeptic in an
  isolated worktree as the binding done-gate. **Round 1 of the gate FAILED; round 2 PASSED all
  13 ACs** (8 P2 incl. an added AC5b, 5 P3). `cargo test --workspace -- --test-threads=3` →
  **504 / 0 / 2** (pre-P2/P3 baseline 460/0/1); clippy `-D warnings` + fmt clean on debug **and**
  release; both import debug seams absent from release `strings`. 12 of 13 claims `EVIDENCED`.
  Full evidence, caveats, and residuals: `VERIFY-LEDGER.md` § "Phase 2.0 P2 + P3".
  - **T2 resolution measured at last: 17 resolved / n=1133 = 1.50%.** Two caveats are mandatory
    and must travel with the figure: **never render it "897 → 17"** (P1's 897 is a different
    definition), and **the oracle and population channels are disjoint, overlap 0** (both
    coincidentally n=17), so **none of the 17 population resolutions is scorable for
    correctness**. A rate far below the 41.5% candidate share is the honest outcome the plan
    predicted — git holds committed states only.
  - **Founder decision: both T2 gates retained**, at a measured cost of **127 of 144 correct
    resolutions discarded to remove 7 fabricated** (gate 1 alone: 137 resolved / 5.1% fabricated;
    both: 17 / 0%). Zero fabrication chosen — a wrong `before` is a wrong-byte revert source in
    undo history. The cost is recorded, not silent.
  - **AC5b's ≤1% bar was set wrong (by the orchestrating agent) and is unmeetable: NEVER quote
    it.** n=17 gives a 95% upper bound of 16.2%; ≤1% needs ~299 clean samples against a channel
    of 228. Honest phrasing: *0 fabrications in 17 guard-admitted samples (95% UB 16.2%)*. Claim
    `clm_4KSWSEZXS894D2P0DC92ZHH8MA` stays DECLARED-unattested as the permanent honesty record.
  - **The gate's round-1 blocker was the third instance of this plan's signature defect:** a
    confidently-worded comment asserting a real-corpus fact that was false. `strip_prefix(cwd)
    .ok()?` silently dropped **507 of 2,170 (23.4%)** file-producing entries, uncounted, beside a
    comment claiming cwd "is lexically a prefix in every real transcript". Two implementers, two
    opus reviewers, and an integration audit all read that line without measuring it. Fixed by a
    `skipped_out_of_cwd` counter — **countability was required; recovery was not authorized.**
    The figure now has three independent agreeing derivations.
  - Import honesty semantics established: derived `after` bytes carry `after_synthesized` and are
    never presented as observed (before this, `undo` fabricated "human or external edit" on files
    nothing touched); `status`'s rich-rate **excludes** imported turns, so a bulk import can no
    longer mask a dead hook at 100% rich.
  - P1's figures re-measured and **NOT stale** after the secret-path parity fix
    (`skipped_secret_path: 0`).

- **P4 EXECUTED and GATE-PASSED (2026-07-30) on `feat/phase-2-0-p4`. Nothing pushed, no PR; NOT
  yet merged to `feat/phase-2-0-substrate` at time of writing.**
  `RepositoryView` extracted into `agentrec-core/src/view.rs`. **537 / 0 / 2** (baseline
  re-measured on clean substrate: 504 / 0 / 2 — P4.md's "410 tests" and "21 goldens" are BOTH
  stale, goldens are 27). Fable skeptic in an isolated worktree: **round 1 GATE FAIL, round 2
  GATE PASS on all 7 ACs**, both mutation proofs re-run by the skeptic itself.
  - Gap logic **unified, not relocated**: one `view::recording_gaps` returns every uncovered
    interval tagged `Crash`/`Restart`/`TrailingStop`. The three old callers are NOT equivalent —
    `status` and blame's no-turn fallback are crash-only, staleness is any-kind-after-a-timestamp
    — so a naive collapse to one boolean would have moved blame output.
  - `health()` is a pure read; `status` calls it and then `enforce_budget` explicitly. Proven both
    ways (re-inserting the eviction fails the purity test).
  - **AC6 failed round 1 for a reason worth keeping: a cursor keyed on a turn id assumed
    uniqueness the ledger itself documents as false.** Orphan-recovery double-emit and resumed
    import `sessionId`s both put two records under one id; resolving by first match re-delivered
    records after a **pure append**, silently. Cursors now carry an occurrence ordinal.
  - **The signature defect struck a fourth time, and this time the agent introduced it.**
    `parse_log_line` guarded on the `type` tag being a *string*, so `{"type": 5}` was coerced into
    a turn — beside a retained comment promising a line carrying a `type` is never coerced. The
    gate caught it by probing the comment rather than reading it. Pre-P4 behavior (presence-keyed)
    restored.
  - A **+54% `status` latency regression** (13.4 → 20.6 ms on the real 2143-line log) from parsing
    `log.jsonl` twice was found by measuring rather than shipped: `load_log` and `load_ledger` now
    share one per-line classifier. Flat at **6.4 ms vs 6.6 ms**.
  - **Deviations, recorded in VERIFY-LEDGER.md:** `health(&self, budget)` not the contract's no-arg
    form (core stays free of CLI config); **`diff`/`blame`/`recall` NOT implemented** — that is
    **P5's entry condition**, since a `--json` serializer reimplementing diff or blame
    interpretation in `cli/src` reopens the seam P4 closed.

- **`main`** — carries the perf-evidence round: [PR #8](https://github.com/ravi1395/agentrec/pull/8)
  **squash-merged** to `main` as `4e04438` on 2026-07-30 (per-phase history survives only in the
  PR, not on `main`), then reconciled with the local docs-only chain by merge, then absorbed into
  `feat/phase-2-0-substrate` (this branch) by a further merge. Test baseline on `main` **443 / 0
  / 1**; the substrate's own baseline is tracked separately above (P4: 537/0/2, pre-merge).
- **Perf-evidence round (delivered, GATE PASS, now on `main` and merged into substrate)** —
  10k/50ms recall envelope closed (p99 11–23 ms, AC1.3 founder-attested), dedup counters,
  retention plan/execute split, eviction moved to a daemon tick, 3 real defects fixed (symlink
  dedup loss, `--stats` uninit, tick gap). **Linux CI leg now real:** run
  [30552318400](https://github.com/ravi1395/agentrec/actions/runs/30552318400) — all 5 jobs
  green (`ubuntu-22.04`, `ubuntu-24.04`, `macos-14`, lint, induced-low-watches). The macOS leg
  went red on its first attempt and green on rerun; cause recorded under residuals, not
  hand-waved. **Collision with P4b-4:** this round split `enforce_budget` into
  `plan_eviction`/`execute` and moved eviction off the `status` read verb onto a daemon tick —
  P4b-4's AC16 fixture must target `plan_eviction`, not `enforce_budget` called from `status`;
  see `docs/superpowers/plans/2026-07-30-p4b-branch-merge-state.md` §3.
- **Phase 2 spec hardened + gated (2026-07-28, 6 skeptic rounds, commits
  `bd0b679`/`bde8146`/`90202f6`/`204ff04`):** founder decisions 5–10 in
  `docs/superpowers/specs/2026-07-18-agentrec-phase-2-design.md` — (5) protocol freeze behind
  Codex 2.1; (6) MCP destructive gated on ≥20 human-confirmed `undo --confirm` (today 0) and
  `allow_modified` never honored in auto mode; (7) CLI demand probe defined
  (transcript-sweep instrument) then (10) **waived as a gate by founder override — MCP read
  2.2 is unconditional Phase 2 scope**, probe survives as post-ship evaluation; (8) import
  gate carries a fidelity report + ledger row, parse-only pass insufficient; (9) **Sutra
  parked** — Phase 2 re-centered on accessibility (import, trailers, `--json`, distribution,
  setup, Codex). Corpus decay measured: import is a **≤30-day rolling backfill**
  (`cleanupPeriodDays` default 30); "durable archive" claim embargoed until a re-import
  mechanism ships; 4-tier before-ladder predicted this day (T1 42.5 / T1.5 25.3 / T2-cand 24.5
  / T3 7.7 → 67.9% honest, 92.3% banned) — **superseded, twice, by P1's real-corpus runs, see
  below: honest figures are 43.8% (counting `create` ops as reconstructible) / 31.7% (actual
  pre-edit bytes only); an intermediate 40.4% reading was also wrong and is retired.**
- **Phase 2.0 plan chunked:** `docs/superpowers/plans/tasks/P1..P5.md` (standalone,
  fresh-executor-ready); plan has a 9-checkbox "Final acceptance — plan exit" section.
- **P1 EXECUTED + GATE PASS (2026-07-29) — merged into `feat/phase-2-0-substrate` at `cc026c9`;
  nothing pushed, no PR.**
  `agentrec import claude --dry-run` built; **460 / 0 / 1** on `fix/p1-t15-path-normalization`
  (pre-P1 baseline 428; the 8/8 gate ran at 447, before the two T1.5 defect fixes added
  coverage); clippy+fmt clean debug & release; debug seam absent from release `strings`.
  Final Fable skeptic in an isolated worktree: **8/8 ACs PASS**, every figure independently
  reproduced (against figures since proven wrong twice over — see below). Real-corpus gate
  run, corrected 2026-07-29 evening (`docs/verify/p1-gate-run-t15fix2.txt`): **1631 sessions,
  1625 importable — 99.6%** (bar ≥90%), peak RSS **17.17 MB** (bar <500 MB). Fidelity row +
  anti-overclaim rider in `VERIFY-LEDGER.md`. **The Phase 2.0 hard stop is retired with a
  fidelity row, not a parse-only pass** (spec decision 8 satisfied).
  - **The plan's before-ladder prediction did NOT hold, and the correction itself was wrong
    once before landing.** Predicted T1 42.5 / T1.5 25.3 / T2-cand 24.5 / T3 7.7. A first
    real-corpus reading (`34d858c`) measured T1 39.9 / **T1.5 0.5** / T2-cand 44.8 / T3 15.0
    and claimed T1.5 was "genuinely near-empty, not under-detected" — **that claim was false.**
    T1.5 was under-detected by a bug (lookup compared an absolute `filePath` against
    `trackedFileBackups` keys that are relative to session `cwd`, so the raw compare almost
    never matched); fixing it raised the count, but then over-counted via stale file-history
    blobs (written at snapshot time, not per edit — ~30% of the fixed reading's resolved bytes
    were fabricated pre-*snapshot* states). Corrected figures, now measured over **2159**
    entries: T1 856 (39.6%, of which 595/27.6% inline `originalFile` + 261/12.1% `create` ops)
    / T1.5 90 (4.2%, ground-truth 101 correct / 1 fabricated) / T2-cand 897 (41.5%) / T3 316
    (14.6%). **Honest reconstructible is 43.8% (T1+T1.5 counting `create` ops) or 31.7%
    (actual-bytes only) — not 67.9%, and not the intermediate 40.4% either** — propagated
    across spec/plan/task/measurement docs/VERIFY-LEDGER. 92.3% stays banned, and the same ban
    now covers the **~85% ceiling** (43.8 + 41.5 T2-cand) — both are upper bounds by the
    identical argument (git holds committed states only).
  - **99.6% is an ingestion rate, never a recovery rate** — only ~184/1631 sessions (11%) carry
    any file mutation at all. Never quote it bare; the rider in VERIFY-LEDGER.md travels with
    it.
  - Defect worth remembering: first gate run reported `t1_5 = 0` because the classifier
    matched a `type: "snapshot"` literal the *synthetic fixture had invented*; the real corpus
    uses `file-history-snapshot`. Every test passed while nothing real resolved. Fixed to
    presence-based harvesting. **Fixture-only evidence cannot close a corpus-shape claim.**
  - 2 claimd claims REFUTED on malformed replay commands (multi-filter `cargo test`, which
    takes one TESTNAME) — the underlying tests pass under a corrected invocation, but REFUTED
    is not amendable and re-declaring an equivalent is forbidden as dodging. Founder call.

### Now / next (in order)

1. ~~**P4**~~ → ~~**P4b**~~ → ~~**P5**~~ → ~~**plan-exit checklist**~~ **all done and gated
   (2026-07-31).** Phase 2.0 is at plan exit; see Current state. The goldens were never
   weakened or regenerated to make extraction pass — every human-form golden is byte-identical
   from `10d235d` to exit, which was the whole ratchet.
   **Remaining on this branch: open a PR.** Nothing is merged to `main` yet.
   Wave 2 (aider import, trailers + shim, npm/mise) after or parallel per open question 1,
   which is still unanswered.
2. ~~import's `existing_ids` intra-run staleness~~ **DONE 2026-08-03 on
   `fix/import-existing-ids-staleness` (unmerged, no PR yet; NOT skeptic-gated).** The defect was
   real and reproduced before the fix: a two-project-dir fixture sharing one `sessionId` printed
   `appended: 2` and wrote two `log.jsonl` turns under one id (ids are
   `hash(session_id:turn_index)`), which is what makes `diff`/`show`/`undo <id>` ambiguous.
   Semantics chosen (founder-approved): **skip the second, count it** — no new id-derivation rule,
   so PROTOCOL turn-id semantics are untouched. `existing_ids` (prior runs) stays a SILENT skip —
   it is what `appended: 0` on a re-run means — and a separate `run_ids` set counts intra-run
   collisions as `skipped_duplicate_turn_id`, surfaced in BOTH the text and `--json` reports
   (mirrors `skipped_out_of_cwd`). The dropped turn's `files` are not imported; that loss is
   countable, not silent. Two new tests: the collision case, plus a guard that a single session
   file yielding two turns still appends both. Suite **751 / 0 / 3** (base 749/0/3); clippy
   `-D warnings` + fmt clean. Skeptic gate in an isolated worktree: **round 1 GATE FAIL on the
   RECORD, not the code** — AC1–AC7/AC8(b)/AC9 all PASS on evidence the skeptic generated itself
   (it neutered the guard and reproduced the two-same-id log plus live `ambiguous turn id` errors
   from `show`/`diff`/`undo`, and mutation-probed the guard test in both directions). **AC8(a) —
   "the new CLAUDE.md item text is true" — was the single blocking FAIL**; it is the only AC not
   listed as passing above. Its five findings, now fixed. **Round 2 (scoped to the record fix) also
   GATE FAILED and is fixed below too — the fix-commit itself shipped signature-defect instance
   #10.**
   - **Forward-only, and NO rewrite class repairs the damage.** A log already carrying a same-id
     pair from a pre-fix import stays ambiguous forever: `purge --log-duplicates` keys on
     `view::same_revert`, which requires equal `files`. Round 2 sharpened this: an equal-`files`
     same-id pair IS repairable (and never surfaces as ambiguous — `view::resolve_turn` collapses
     it); `same_revert` refuses exactly the harmful pair, the one whose copies touched different
     files. Skeptic-measured on such a log, against an equal-`files` control that repairs cleanly:
     `0 duplicate(s) removed`, `show <id>` still ambiguous. `import claude` is already on `main` (`a90e9ff`), so this is a live user-visible
     consequence. A repair needs a **fourth sanctioned rewrite class** (decision-register entry) —
     deliberately NOT built; also unmeasured: nobody knows how many repos already hold such a pair,
     and no verb detects it.
   - **Corpus grounding corrected — signature-defect instance #9, caught by the gate.** The
     replaced text carried a measured caveat ("inert today only because one copy is a 1-line
     cwd-less stub"); this round deleted it while re-asserting "the real corpus holds such a pair"
     in two NEW code comments. Caveat restored in both comments. Round 2 **re-measured this
     independently** rather than taking round 1's word (two channels, agreeing): ~2,134 distinct
     `sessionId`s over 2,136 depth-2 session files, exactly ONE spanning two project dirs, its
     second copy a 1-line `bridge-session` stub with no `cwd`, so the real pair mints **zero**
     colliding turns (`sessions_importable: 1 (50.0%)`, `appended: 0`, and
     `skipped_duplicate_turn_id: 0` at BOTH roots — the last figure is what actually establishes
     "zero colliding", since `appended` is root-dependent). Hedges checked and accurate: one
     machine; ≤30-day backfill confirmed (`-mtime +30` → 0 files, 30.03-day span). The file count
     drifted 2136→2146 within an hour of probing, so **the absolute is not a gradeable figure**.
     **The shape is real; the live corpus is not evidence the guard fires.**
   - **The skip leaves no durable wire trace — and round 2 falsified this round's first attempt at
     saying why (signature-defect instance #10, written into BOTH records, copied verbatim from
     round 1's report without checking source).** The false sentence was "`skipped_out_of_cwd` at
     least marks the surviving turn `files_complete: Some(false)`; this has no analogue".
     `files_complete: Some(false)` is set **unconditionally on every imported turn** (single
     assignment site in `importcmd.rs::persist_session_file`; `fmt.rs` renders it as the blanket
     AC7 "partial file list (imported)" marker), so it discriminates nothing — a repo with
     `skipped_out_of_cwd: 0` renders identically to one with `skipped_out_of_cwd: 2`, and the
     dup-skip survivor carries the same flag. Corrected statement: the counter is run-scoped stdout
     (re-importing the same colliding corpus prints `skipped_duplicate_turn_id: 0` while the
     collision persists — verified), and **neither** skip has a discriminating durable marker.
     Recorded, not built.
   - **Which copy survives is lexical** (`dirs.sort()` then `session_files.sort()`) — deterministic
     but unrelated to richness, so the kept copy may be the poorer one.
   - Commit-message inference trimmed: "every other persist test is one-turn-per-file" is true, but
     the skeptic's coarse `[..6]` key mutation also reds `ac2_resume_…`. Its own caveat was
     **probed and holds** in round 2: a realistic per-session over-eager skip
     (`run_ids.insert(t.session…)`) reds ONLY the new guard test (1 failure, not 2), so the guard
     earns its place.
   **Deliberately not done:** dry-run's report gains no equivalent counter — dry-run appends
   nothing and its classifier counts entries, not appended turns (its JSON has no turn-count key at
   all — key set enumerated by the gate — so "one fewer turn than dry-run predicts" is loose
   wording for a comparison dry-run never emits; the substance — no dedupe, no collide count — is
   skeptic-verified).
   **Round-2 residuals, recorded not fixed:** (a) `files_complete` cannot distinguish "import
   dropped entries from this turn" from "complete but imported" — pre-existing, and it is what
   falsified the text above; (b) the claim's `evidence` event is pinned to `2a60395` while later
   commits edit both its `stale_on` files — the re-evidence event at `306c26d` was left UNCOMMITTED
   for a round (the gate correctly read the claim STALE at HEAD from the committed ledger while the
   working tree read EVIDENCED), now committed; (c) **the repo's recorded "multi-filter `cargo
   test` matches only one TESTNAME" lesson does not hold on `cargo 1.97.1 (Homebrew, darwin)` —
   for EITHER flavor.** Positional: two test-name args → `2 passed; 35 filtered out` (measured
   three times). `--test`: `--test import_claude --test golden -- --list` runs BOTH binaries (37
   and 33 tests; each alone gives its own count, so the two-flag run is their union). An earlier
   version of this bullet asserted the restriction "holds for `--test`, not for test-name
   arguments" — false, and self-contradictory with its own first clause; it was written without a
   command behind it and the gate killed it (see the recurrence note below). Whether the P1-era
   REFUTED claims were correct on their contemporaneous cargo is untested and unrecoverable
   without that version; those claims are founder-owned and untouched here. (d) **Stale-build
   hazard the gate hit:** editing only
   `cli/src/importcmd.rs` and re-running `cargo test --test import_claude` can execute a STALE
   `target/debug/agentrec` and pass under a mutation that demonstrably breaks — `cargo build` (or
   touch the test source) before any mutation probe against the binary.
   **Unverified by anyone (belongs to the D46/brew item, not this fix):** the Linux
   `/proc/self/exe` residual in `service_exec_path`; the only test is `#[cfg(target_os = "macos")]`
   (`cli/tests/integration.rs`), so no Linux CI leg covers it either.
   **Every gate round up to the passing one FAILED, and every blocker was in the RECORD — never
   once in the code.** No round count is stated here on purpose: a tally embedded in the text being
   gated is undercounted by one on every next verdict, which is how the earlier "five" went stale
   the moment round 6 closed. Round 1: the item's own text (AC8(a); resting on round 1's per-AC
   table, which was never persisted — see the rider below). Round 2: the `files_complete` sentence
   (defect #10). Round 3: the `cargo
   --test` clause. Round 4: a paragraph, written here, generalizing rounds 1–3 into a lesson — *"the
   failure mode is asserting without a command behind it when the subject isn't code"*. **False,
   and DELETED rather than reworded:** #3 (`strip_prefix(cwd).ok()?` beside a comment claiming cwd
   "is lexically a prefix in every real transcript" — 507/2,170 entries dropped) and #4
   (`parse_log_line`'s string-tag guard beside a comment promising no coercion), both recorded
   above in this file, are code-subject assertions made without a command. (Bare `#n` here means a
   signature-defect instance; gate rounds are written "round n".) Round 5: three clauses inside
   the paragraph that deleted round 4's lesson — a misstated provenance, a count of "ten recorded
   instances" when only **8 are recorded anywhere** (#1 and #2 exist solely as an implication of
   the ordinal in "the third instance"; `git log -S` finds nothing), and a miscitation of this
   file's own "instances 6–8" bullet, which says **one of them** was a copied miscount, not all
   three.
   **No replacement lesson is offered, and none should be written from this sample.** The
   numbering is also not one series: the P2 gate bullet says "the third instance of this **plan's**
   signature defect", the D46 AC-S2 bullet says "fifth instance of this **repo's**". Reconciling or
   reconstructing #1–#2 is founder-owned; until then any sentence counting the series is
   unverifiable. **Anchors, not line numbers, deliberately** — the round-6 gate caught this very
   sentence citing "line 535" for text that this commit's own +6 shift had already moved to 541,
   which is the exact rot this file records against itself ("F1's line-number citations rotted by
   later same-branch commits — now file:symbol").
   **Provenance of round 4's blocker, corrected — it ran the other way from what this file said
   one round ago.** The tooling-vs-source partition was written by the round-3 SKEPTIC, unprompted,
   in that report's Remaining risk; this file then hardened it into the absolute "never about this
   repo's own source" and cited the gate as its authority. The prior wording ("supplied by the
   agent to its own gate") took blame the agent had not earned, and the round-5 gate refused it.
   What the agent did own is the hardening and the citation.
   The base figure is no longer a restatement: `769560d` re-measured after `cargo build` →
   **749 passed / 0 failed / 3 ignored**, twice and independently — once by the orchestrator in a
   detached worktree, once by the round-4 gate via `git archive` extraction (it is barred from
   `git worktree add`, which writes into the production repo's `.git`). Same figure on both
   channels, so the +2 delta is measured at both ends. Residual as the gate stated it: neither
   channel rules out a test whose behavior depends on being inside a git repo shifting BOTH
   endpoints equally; a `git clone --local` re-run would. The gate noted its own residual is
   conservative — the two channels differ precisely on the `.git` property and agree at 749, which
   is itself evidence against git-dependence.
   **Still unverifiable, and the rider must travel with the sentence:** "AC8(a) was round 1's
   single blocking FAIL" — round 1's per-AC verdict table was never persisted (gate confirmed:
   nothing in `docs/verify/`, no `VERIFY-LEDGER.md` row, `AC8(a)` appears only here), so restating
   it without this rider would let it rot into fact. Also unaudited: whether signature-defect
   instances 1, 2 and 5–8 share any category at all.
3. Then: Codex 2.1 → Protocol 1.0 freeze → MCP read 2.2 (unconditional, thin adapter over
   P5's serializer). 2.3 stays evidence-gated.

### Founder-pending (agent cannot or may not do these)

- ~~**40 orphaned `com.agentrec.*` LaunchAgents**~~ — **REAPED 2026-07-31 at the founder's explicit
  instruction.** The machine now carries **exactly one** agentrec unit,
  `com.agentrec.bfa6bde6eaa4` → `~/Projects/agentrec`, loaded and healthy (pid 865, launchd status
  **0**; before the reap 23 orphans were loaded and failing with status **78**, i.e. launchd was
  repeatedly respawning recorders whose `--root` was gone). Method: every plist archived first to
  `AGENTREC_LAUNCHAGENT_ARCHIVE` (below) — reversible, per the house never-delete rule — then the
  removal list built from agentrec's OWN classifier (`service::scan_units` `VanishedRoot` bucket),
  each root independently re-checked absent, the live unit asserted out of the list, and every
  entry asserted present in the archive before a single `rm`. Then `launchctl bootout
  gui/$(id -u)/<label>` followed by `rm` — 39 removed, 0 failures. `doctor`'s orphaned-services
  check is now a silent `pass` with no note, which is the D46 detection half confirming its own
  fix end-to-end on the real machine.
  - **Archive (delete when satisfied; nothing else references it):** `/Users/ravichandrasekhar/agentrec-launchagents-archive-20260731-152414` — 40 plists.
  - **Two of the 42 were removed before this pass and NOT by the agent.** `com.agentrec.039364bb7dfe`
    (the label `doctor` happened to print as its example) and `com.agentrec.c0bf764acce7` (the unit
    D46's own suite leaked, whose removal command was surfaced in a runnable block) both vanished
    between checks. Almost certainly the founder ran the two printed commands; that is inference
    from which labels disappeared, not proof, and it is recorded as inference.
  - **The product defect that produced all 40 is fixed (D46)** — see Current state. `init` under a
    temp root installs no unit, so this cannot silently re-accumulate; `doctor` reports any that do.
- ~~**`init` bakes a CANONICALIZED exec path, breaking every Homebrew user on upgrade**~~ **FIXED
  and this bullet was STALE — retired 2026-08-03 after re-reading the source, not the record.**
  `initcmd::service_exec_path()` is deliberately non-canonicalized and documents exactly why (brew
  symlink → Cellar version pin → status-78 respawn loop); both call sites use it. **Residual, stated
  in that doc comment and NOT fixed because it cannot be:** on Linux `std::env::current_exe()` reads
  `/proc/self/exe`, already kernel-resolved, so a symlinked Linux install still records the target.
- **The 40 orphans' root cause — this bullet was itself wrong once, and the correction of the
  correction is the load-bearing part.** The original recorded cause was "`init` in a temp dir
  without a matching uninstall". This bullet then "corrected" it to *build trees, **not** temp
  roots*, on the strength of the archived plists' **exec** paths. That inference was false: it read
  one of the unit's two baked paths and generalized to the other. Re-measured over all 40 archived
  plists, parsing each unit's `--root` (not its exec):
  - **39 of 40 recorded a `--root` under a temp prefix** — `/private/var/folders/…/T/tmp.*` or
    `/private/tmp/claude-501/…/scratchpad/*`. **0 orphans had a non-temp root.**
  - The 40th is `~/Projects/agentrec` on `~/.local/bin/agentrec` — both paths still present: the
    **live dogfood daemon, never an orphan at all**. So the orphan population is 39, not 40, and it
    is **100% temp-rooted**.
  - The build-tree observation is **substantively correct** and describes the **exec** path
    (`~/.gate3` 21, `~/.gate2` 6, `agentrec-phase2` 4, `~/.gate-linux` 4, plus scratchpad
    worktrees) — which is why those units fail with launchd status **78** (exec failure) rather
    than the daemon refusing a missing root. Only its *inference* ("not temp roots") was false.
    Exact split, since the round's own first count was off by one: **38 `target/debug` + 1
    `target/release`** (`com.agentrec.decf56034f2e`, `Projects/agentrec/target/release/agentrec`)
    + 1 `~/.local/bin` — so "39 `target/debug`" is wrong, "39 build-tree execs" is right.
  - **Therefore the temp-root guard is the correct predicate and would have prevented 39 of 39 —
    but only in its post-`58c517f` form, and that distinction is the point.** D46 **as first
    shipped** (`2c26fc5`) keyed on reading `$TMPDIR`, and demonstrably leaked a real launchd unit
    for a tempdir root when a test scrubbed the variable (`58c517f`: "the `$TMPDIR` read let the
    guard leak a unit"; `initcmd.rs:54-65`). TMPDIR-scrubbing harnesses are exactly the population
    that created these 39, so **"D46 would have prevented 39 of 39" is true of the current code
    and false of D46 v1.** The `--service` bypass is ruled out, not assumed: `git show
    2c26fc5^:cli/src/main.rs` has no `--service` flag at all, so no historical `init` could have
    passed it. The preceding "not temp roots" wording undercut the guard this repo had just shipped.
  - **Claim superseded at the founder's direction (2026-07-31):**
    `clm_50SC7TBN4GC281S1F8PZF0XTNF` overstated this by saying "D46's temp-root guard" without
    pinning the version. Replaced by `clm_4H1XM492AEQE4G2W9NTM822CER`, carrying the skeptic's
    wording. The founder directed the re-declaration explicitly — an agent softening its own
    reviewed claim unprompted would be indistinguishable from dodging, and is still forbidden.
    Both claims rest DECLARED-unattested; neither was self-attested.
  - **Fix design brainstormed, NOT built:**
    `docs/superpowers/specs/2026-07-31-service-unit-staleness-design.md` (fable, fresh context).
    Recommends declarative liveness conditions in the unit itself (launchd `KeepAlive`/`PathState`
    dict; systemd `ConditionPathExists`) plus an exec-aware `scan_units`, and keeps `service prune`
    deferred. Load-bearing finding: `daemon.rs::run` already exits cleanly on a vanished root, but
    a vanished **exec** fails at spawn (status 78) before any agentrec code runs — so **a
    daemon-side self-reap would have prevented none of the 39 measured leaks.** Three founder
    decisions at the tail of that doc.
  - **Not establishable, stated as a gap:** the archive copy clobbered every plist mtime to
    2026-07-31 15:24:14, so **no unit can be dated** — how many of the 39 pre- or post-date
    `2c26fc5`/`58c517f` is unrecoverable, as are the original `init` command lines.
  Replay (archive is local and disk-only, so this is a manual claim, `clm_50SC7TBN4GC281S1F8PZF0XTNF`):
  parse `ProgramArguments` in each `~/agentrec-launchagents-archive-20260731-152414/*.plist`, take
  the argument after `--root`, and bucket on the `/private/var/folders`, `/private/tmp`, `/tmp`,
  `/var/folders` prefixes → 39 temp / 1 other.
  Mechanism, unchanged and still the general lesson: `init` bakes a SNAPSHOT OF TWO PATHS (root +
  the running exec), neither validated at load time, and `KeepAlive` turns a stale snapshot into
  permanent noise rather than one clean failure. Either path going stale is sufficient.
- **Residual gap D46 does NOT close, and the one that matters for real users:** a unit whose
  recorded path — root **or** exec — vanishes on a **non-temp** path. Deleting, moving, or renaming
  an ordinary repo leaks an identical unit and the temp guard never fires. `doctor` reports it;
  nothing prevents or reaps it. This is also the new evidence bearing on **`service prune`**, whose
  deferral rationale (only piece that shells `launchctl`; destructive; founder-reserved) still
  holds — but whose stated mitigation was "the user reaps by hand", and the leak source is now
  known to be systemic rather than agent-only. Founder decision, not an agent reversal.
- **Re-declare AC5b's claim with honest wording** (founder decided the approach 2026-07-30; the
  agent must not run it — an agent re-declaring its own unmeetable claim with weaker text is
  indistinguishable from dodging a refutation). Text to use: *0 observed fabrications on the
  guard-admitted channel (n=17); 95% upper bound 16.2%; the whole verifiable channel is 228
  cases, so a bar below that bound is not establishable by this oracle.*
- **Pinned decision 14 conflicts with the corpus — needs a ruling.** P1.md pins `cwd` as
  session-level (first line carrying one wins) and marks it non-re-litigable, but **67 sessions
  carry more than one distinct `cwd`** (usually a subdirectory move). Resolving each backup key
  against the `cwd` in effect at its own snapshot line finds ~271 T1.5 candidates vs the current
  rule's 210-before-staleness-gating, at marginally *better* precision. Deliberately NOT changed —
  a pinned decision is not an executor's to overturn. Founder decides whether to amend it.
- **D46 AC-S2's claim REFUTED, correctly, and NOT re-declared** (`clm_4AFDDT3XCSFHKDZ06D926XJVNY`,
  exit 101). `claimd verify` replays committed state from a checkout under `$TMPDIR`; the test
  backing it used `env!("CARGO_MANIFEST_DIR")` as its non-temp control, beside a comment asserting
  that is "a real, non-temp path" — false in exactly that checkout, where the path IS temp and
  `Install` is the wrong expectation. Reproduced by cloning to `$TMPDIR`. **Production code was
  never wrong; the test's control path was.** Fixed (fixed non-temp paths covering both the
  canonicalize-succeeds and canonicalize-fails branches) and re-verified green from a `$TMPDIR`
  checkout — but REFUTED is not amendable and re-declaring an equivalent is forbidden as dodging,
  so AC-S2 now carries a passing test and no live claim. Founder decides whether a re-declaration
  is warranted. Same disposition as the two P1 claims below. Worth keeping: this is the
  fifth instance of this repo's signature defect — a confidently-worded comment asserting an
  environment fact nobody measured — and the first one caught by the claim protocol itself rather
  than by a skeptic.
- 2 claimd claims REFUTED on malformed replay commands (`clm_2DDM03JPR1N9JT4QHYHBTZC1SM`,
  `clm_07FVVKJGHZS8ZFSR2998HE7QRP`) — multi-filter `cargo test`. Underlying tests pass under a
  corrected invocation, but REFUTED is not amendable and re-declaring an equivalent is forbidden
  as dodging.
- ~~Open the PR for `fix/perf-evidence-round`~~ **done** — PR #8 merged to `main`
  (`4e04438`/`72c4b82`/`34bbb0c`) and absorbed into this branch by merge; Linux CI leg is real
  (run `30552318400`, 5/5 green — note this run predates `import`, which lives on
  `feat/phase-2-0-substrate`, not `main`; the Linux matrix has never run this branch's HEAD).
  **P1's AC7 residual is NOT closed by this** — checked before claiming it: `cli/tests/
  import_claude.rs` does execute `peak_rss_mb()` in-process (spawns the real binary, asserts
  `report["peak_rss_mb"].is_number()`), but `is_number()` passes under either the macOS or Linux
  `RSS_DIVISOR` — it can't discriminate which constant fired. `rss_raw_to_mb` itself is
  unit-tested with both divisors passed explicitly, which proves the arithmetic, not the
  `#[cfg(target_os = "linux")]` selection. Still open; needs a magnitude assertion on `import
  --dry-run`'s reported `peak_rss_mb` running under Linux CI on this branch's HEAD to close.
- `rm` retained archives `.agentrec/objects.archived.1784328469` (2.6 GiB) + `.1784934498`
  (15 MiB) — reversible-until-deleted, disk-only.
- 13 stale local branches (git-guardrails hook blocks agent `branch -D`; command was handed
  over 2026-07-28).
- **17 claimd claims DECLARED awaiting manual attestation** (never self-attested; both
  branches' queues combined by the merge union).
- ~~66 claimd claims STALE (PR #8 scope-drift)~~ **mechanically re-confirmed 2026-08-01**
  (full `verify --all-stale` batches during the redteam round). Post-merge-union state
  (`claimd status`, 2026-08-01 02:20): **211 claims — 163 confirmed, 5 stale
  (deferred/missing replay specs, cannot replay), 26 refuted, 17 declared.** Of the 26
  refuted: this round's 9 are all claim-cmd authoring bugs, each superseded by a CONFIRMED
  replacement (incl. 2 refuted only by the merge legitimately bringing PR #10's D46 comments
  into cli/src — superseded by `clm_7W9107CC`); the remaining ~17 are **PR #10's round
  claims broken by the squash + this branch's additive golden keys** (byte-identical-golden
  and worktree-pinned replays) — re-declaration of that round's claims is owed and was NOT
  done here (not this round's evidence to rewrite); same shape as the PR #8 debt above.
- Undecided claimd doc-scope rule: `PROTOCOL.md` is not lint-ignored — next normative-doc
  edit fires the Stop hook again.
- ~~PROTOCOL.md §3 amendment (D48 loose end)~~ **DONE 2026-08-01 (founder-directed):** §3 now
  defines append-only precisely with a MAY-reclaim clause for consumed `signal.jsonl` lines
  (D48). claimd doc-scope rule for PROTOCOL.md remains undecided (Stop hook fired, claim
  declared covering the edit).
- ~~CAUTION feature + bare-turn decision (re-gate N4)~~ **DONE 2026-08-01 (founder-directed):**
  bare turns get their own unattributed-window caution; D49 + AC-CAUTION-1..4 registered, each
  mapped to a named misattribution.rs test. Residual: a foreign L1+ producer emitting
  `grade:"bare"` WITH a tool would falsify the "no recorded tool" clause — becomes live when
  Phase 2 import lands (disclosed in code comment).
- ~~Merge decision~~ **RESOLVED by events (2026-08-01):** PR #10 squash-merged to `main` as
  `a90e9ff` first; `origin/main` then merged INTO this branch (conflicts resolved by union —
  register order D46/D48/D49, `status --json` carries both `RepositoryHealth` flatten and the
  D48 inbox fields). The D46 renumber to D48 predated the merge, so the register is
  collision-free. PR #11 remains the merge vehicle for this branch.
- Demand/launch gate (Show HN etc.) never run — ROADMAP Phase 0's 30-day kill criterion has
  no data; 2.2's post-ship evaluation row needs a probe repo picked + `agentrec init` there.
- Plan open questions 2–3: memory-plan stale checkboxes; store-churn reclaim.

### Standing debts & residuals (recorded, not blocking)

- `log.jsonl` churn history (9602 `.remember` entries) still renders as churn blasts in
  `log`/`show`; not byte-reclaimable without a fourth sanctioned rewrite class — deliberately
  not built.
- claimd coverage debt rows: `cli/src/cmds.rs` (P3 residuals round) and `cli/src/purgecmd.rs`
  (honesty round) — touched-uncovered, retroactive declaration refused by design.
- P1 probe verdict bounded: fixture aging can't reproduce weeks-old FSEvents journal history;
  the dogfood daemon is the observatory.
- Perf/timing margins are macOS+one-Colima-VM evidence; population-level claims close only
  over CI history. PR #8 contributed the first two GitHub-runner Linux data points for this
  round; one PR is not a population.
- **`hook_recall_hard_wall_deadline` is runner-coupled** (first observed PR #8, macos-14 job
  `90903831567`): it timed **204.1 ms** against a `< 200 ms` assert and passed on rerun. The
  invariant held both times — a 600 ms blocked pin read was abandoned, not waited on — but the
  assert measures *whole-process* wall including fork+exec, leaving ~4 ms of margin on a shared
  runner. Bound raised to 300 ms (founder decision 2026-07-30): still 2× under the 600 ms
  block, so the neuter that removes the wall still reds. The tighter fix (subtract a measured
  spawn baseline in-test) is **not** done and stays available if 300 ms also proves flaky.
- **`integration::daemon_counts_ignore_rebuilds` reported FSEvents-coupled** (`integration.rs::daemon_counts_ignore_rebuilds`) —
  **reported by the Task D0 review round 1, NOT reproduced here**, and the mechanism below is
  that review's, relayed with attribution rather than restated as measured fact: a `poll_until`
  on a monotonic counter waiting for exactly `== 2`, which an extra FSEvents rebuild pushes past,
  so it fails on full-suite runs and passes isolated or on rerun. This round's own full-suite run
  at the D0-review-fix commit passed it (873/0/3, one run). Pre-existing and unrelated to D51;
  recorded, not fixed. A `>= 2` bound is the obvious fix, unverified as such.
- Memory dogfood ladder: 1-week row **CLOSED FAILED 2026-07-31** (window expired dirty; 0
  agent-origin candidates in 1944 signal lines — the SKILL emitter never fired once; hit-rate
  unfalsifiable, stats log has no denominator). Rerun requires fresh T0, re-pinned baseline,
  and FIRST an end-to-end proof the candidate path fires at all.
- DEGRADED channel wording (re-gate N3): a signal-inbox shrink is counted via
  `record_io_failure`, so the banner reads "snapshot write(s) failed … undo on affected files
  has no snapshot" about a file that is neither a snapshot nor undoable. Pre-existing channel,
  deliberately deferred.
- `daemon.rs` resync helper doc: "len is the only offset that can't replay consumed signals as
  duplicate turns" is true at the `poll` site, over-general at the startup site (startup replay
  never feeds start/stop to the engine at any offset). Safe direction, text-only.

## Working method

Work runs as the `/ratchet` loop (`.claude/commands/ratchet.md`; staged in `claude-setup/` until installed — a ratchet only tightens, ACs are never loosened to pass): Opus orchestrates, Sonnet `implementer` agents build against named AC ids, an Opus `skeptic` agent with fresh context reviews per-AC (PASS/FAIL/UNTESTED — no PASS without a test), failures loop back, and every round ends by updating the Status section above. Never weaken an AC to pass; escalate ambiguity to the founder.

Read before building; do not invent semantics that contradict these docs:

| Doc | Contents |
|---|---|
| PROBLEM.md | Why this exists; who has the pain; why tool-neutral |
| SPEC.md | v1 reference implementation: daemon, 5 CLI verbs, prompt posture |
| PROTOCOL.md | **Normative.** Signal + turn-record schemas, conformance levels L0–L3, MCP surface, versioning rules |
| ROADMAP.md | v1 → v4+ phases, measurable gates, kill criteria |
| INTEGRATIONS.md | Four-ring integration thesis; v2 designs (Claude Code, Codex, VS Code) |
| IMPLEMENTATION.md | Decision register D1–D44 + exhaustive acceptance criteria per release (incl. v0.3 durability + v0.4 accessibility amendments) |
| REVIEW.md | Hostile review that forced the v0.2 capture redesign — read before touching capture semantics |

Conflict resolution order: PROTOCOL.md > IMPLEMENTATION.md decision register > SPEC.md > everything else.

## Architecture

```
EMITTERS                      RECORDER                      CONSUMERS
Claude Code Stop hook ─┐
Codex Stop hook (v2) ──┼─→ .agentrec/signal.jsonl ─┐
(any L1+ tool) ────────┘      (append-only inbox)  │
                                                   ▼
                              agentrec daemon ("record")
fs mutations ─→ watcher ─→    TurnEngine: signal boundary,
(notify, 1.5s debounce,       else 10s quiet window;        ─→ .agentrec/log.jsonl   ─→ CLI (log/diff/blame/undo)
 denylist filter)             one open turn per root            (canonical, JSONL)    ─→ MCP server (v2)
                                   │                        ─→ .agentrec/objects/     ─→ VS Code ext (v2, read-only)
transcripts (prompt) ─→ scrub ─────┘                            (sha256 CAS, 10MiB cap)─→ Sutra GUI (v3)
```

Two crates in one cargo workspace: `agentrec-core` (lib: TurnEngine, BlobStore, formats, scrub) and `agentrec` (bin: daemon + clap CLI). Threads + `notify`, no async runtime. Consumers never need the daemon running — all reads are file-based.

### Key semantics (do not violate)

- **Turn grades:** `rich` (hook signal → tool/model/prompt attached) vs `bare` (quiet-window inferred). A bare turn is an *unattributed activity window* — a human vim save produces the same fs signature — so bare turns are never rendered as agent activity and never fabricate attribution.
- **Bracketing (v0.2, load-bearing):** Claude Code integration installs `UserPromptSubmit` (start) + `Stop` (stop). Open bracket suppresses quiet-window closure; on stop, interim bare turns are retroactively merged into the rich turn. Start-without-stop = close at last mutation, `truncated: true`.
- **Git turns:** mutation bursts coinciding with `.git/HEAD`/index/ref transitions are rich turns with `tool: "git"`; hidden from `log` by default. Never let a `git checkout` become a 400-file bare turn.
- **Epochs & gaps:** daemon start/stop append `type:"epoch"` records; blame across an uncovered interval must say "attribution stale — recording gap", never guess.
- **Append-only everything:** `log.jsonl` and `signal.jsonl` are only ever appended to; no line is mutated, reordered, or rewritten in place. History is corrected by appending. Signal consumption tracked by byte offset in `state.json`. Turn ids are machine-scoped ULIDs. **Exactly three sanctioned rewrite classes exist, all manual `purge` sub-ops, all refusing while the daemon runs, all archive-before-touch + atomic tmp/fsync/rename:** (1) `--memories-retracted` — `memory.jsonl`, drops fully-retracted chains past TTL; (2) `--log-duplicates` — `log.jsonl`, drops same-id duplicate turns a pre-fix daemon wrote; (3) `--signals-consumed` (D48) — `signal.jsonl`, drops only WHOLE lines already consumed (strictly before `signal_offset`, whose prompts are already in `log.jsonl` + the CAS) and rebases `signal_offset` in the same operation. Nothing else may rewrite these files; adding a fourth class requires a decision-register entry.
- **Two predicates, never conflated:** `modified-since` (hash ≠ turn's after) gates every destructive op; `human-edited-since` (modified AND not covered by any *rich* turn) is blame display only. Bare turns never count as coverage.
- **Undo is a turn:** every revert (CLI or MCP) snapshots current state first and appends a new turn with `tool: "agentrec"`. Reverts are blame-able and re-revertible. `skipped` (over-cap) and `withheld` (secret-pattern) files are never revertible.
- **Prompts and snapshots both scrubbed:** prompt scrub (secret regexes + entropy) runs *inside* the persistence function; secret-file patterns (`.env*`, `*.pem`, credentials) are never snapshotted (`withheld: true`). Local-only; no network code exists in v1–v2; never claim "provably" safe.
- **Agent-driven undo** is gated by `config.toml: mcp_destructive = off|confirm|auto` (default off); `auto` requires the two-phase confirm-token flow; safety keys on `allow_modified` (PROTOCOL.md §8).
- **Watch filtering:** gitignore-derived by default, plus denylist `.git` contents (except HEAD/index/refs — needed for git-turn classification), `.agentrec` (self-write suppression — no feedback loops), `node_modules`, `target`, `dist`, and user globs. Linux inotify limit exhaustion = loud startup error.
- **Clocks:** UTC wall clock in records; monotonic clock for quiet-window measurement.

## Commands (once code lands)

```bash
cargo test                    # unit + engine table tests (agentrec-core)
cargo test --test integration # drives the real binary against tempdir fixtures
cargo clippy && cargo fmt     # CI-enforced
agentrec init && agentrec record   # dogfood in this repo itself
```

## Releases

**Use the `agentrec-release` skill** (`claude-setup/skills/agentrec-release/SKILL.md`;
install into `.claude/skills/`). It overrides the global `ship` skill for this repo.
Never hand-roll a release: the version lives in **six files / eight checked fields**
(measured repo-wide, not by a scoped grep: `git grep -n '0\.2\.0' | grep -v CHANGELOG`
at `d4b2576` surfaces exactly these and no eighth file), and two of them fail only
downstream — `cli/Cargo.toml`'s `agentrec-core` pin (breaks `cargo publish`, which
resolves it against the registry not the path) and `Cargo.lock` (breaks the `--locked`
release build). The release workflow's `verify-version` job asserts all eight fields
against the tag before anything is built; run the identical check locally first:

```bash
bash .github/scripts/check-versions.sh X.Y.Z
```

Registry publishing (crates.io, npm) is a **human hand-off** — the agent never holds
registry tokens. Runbook: `docs/launch/distribution-publish-runbook.md`.

## Conventions

- Every acceptance criterion in IMPLEMENTATION.md §3–§7 maps to at least one automated test; new features add their AC there first.
- Protocol changes: additive-only within a major `v`; update PROTOCOL.md + conformance fixtures in the same commit; consumers must tolerate unknown fields.
- Platforms: macOS + Linux. No Windows-specific code, but no hardcoded path separators either (D19).
- License: Apache-2.0 (open-core; all v1–v3 code). Hosted/team features v4+ commercial.
- Never delete user data: `purge` is the only deletion path and only for objects past TTL / by explicit flag; archives, never silent removal.
