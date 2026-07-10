# VERIFY-LEDGER — M3 "Ship"

Done-definition for this round (founder-confirmed 2026-07-10): **M3 code-complete + laddered.**
Every AC verifiable on this macOS box is PASSed with pasted evidence in the SDD final gate.
The rows below are ACs whose *only* proof requires an environment absent here (Linux, containers,
Homebrew, a 7-night clock, a GIF pipeline). They are **OPEN** — never counted as PASS, never
allowing a "M3 shipped" claim. Each names the exact command/environment that closes it.

## Open (gated) rows

| AC | Gate | Closes when |
|---|---|---|
| H++ / D36 | 7-consecutive-green-nights torture run is a **launch gate**. Harness built + one local ≥1000-op run proven here; the multi-night streak cannot be produced tonight. | Nightly CI runs the harness 7 nights with 0 invariant violations. |
| Y+1 (Ubuntu leg) | `curl \| sh` installer proven on a **clean Ubuntu container**. macOS leg provable here. | Installer run in an Ubuntu 22.04/24.04 container yields `agentrec` on PATH, checksum-verified. |
| Y+1 (macOS clean box) | Clean-box guarantee (no prior toolchain) needs a fresh macOS VM/container. | Installer run on a pristine macOS image. |
| Y+2 | `brew install` via own tap. Formula authored; tap publish + install is a network/launch step. | Formula published to tap and `brew install` succeeds. |
| Y++2 (inotify mode) | `doctor` Linux-inotify-headroom failure mode. Code path present; only asserts on Linux. | Integration test run on Linux CI induces low `fs.inotify.max_user_watches`. |
| I++1 (Linux perms) | 0700/0600 assertion on **Linux CI** (macOS leg provable here). | Perms test green on Ubuntu CI. |
| README blame GIF | The line-level-blame demo GIF leading the README. | GIF recorded from a real session and embedded. |
| CI matrix (J3) | macOS-14 + Ubuntu-22.04/24.04 green per-commit; soak/perf nightly. | GitHub Actions matrix green. |

## Closed here

| AC | Evidence |
|---|---|
| Y+1 (macOS local-binary leg) | `install.sh` supports `AGENTREC_LOCAL_BINARY`/`--local` + `--sha256`. Ran: `AGENTREC_LOCAL_BINARY=./target/debug/agentrec sh install.sh --sha256 $(shasum -a 256 ./target/debug/agentrec \| cut -d' ' -f1)` → `checksum verified: ceebf69bc654e3f57601d1c04aee5986a481b7a01e152a463a7b216dd7c2f2a1`, installed to `~/.local/bin/agentrec`, `~/.local/bin/agentrec --version` → `agentrec 0.1.0`. Mismatch path also verified: `--sha256 deadbeef` → `error: checksum mismatch: expected deadbeef, got ceebf6…` and exit 1. |
| Y+1 (install.sh shell hygiene) | `shellcheck install.sh` → no output (clean), shellcheck 0.11.0. |
| PROTOCOL v0.2 published | Version marker + changelog line at top of PROTOCOL.md; added previously-undocumented wire fields `merges` (turn) and `baseline_unknown` (file entry) — both already emitted by `agentrec-core/src/record.rs` and `cli/src/daemon.rs` — additive within v0.2, cross-checked field-for-field against `record.rs`. DEGRADED/snapshot-failure state confirmed local-only (`cli/src/state.rs`) and explicitly excluded from the wire protocol rather than invented as a field. |
| README sections | `## Line-level blame` (leads the doc, GIF placeholder + real CLI-output examples matching `readcmds.rs` line/whole-file formats), `## How we try to break it` (D36 torture harness framing + local run command), `## Commands` (cross-checked against `cli/src/main.rs` subcommand/flag list — no invented flags), install section pointing at `install.sh` + `agentrec init`, Apache-2.0 license line. |

## Closed at the M3 final gate (opus-4.8-high, commands executed, output pasted)

Verdict: **SHIP** — all ACs verifiable on macOS PASS. 142 tests, 0 failed; clippy clean; zero orphan daemons.

| AC | Evidence |
|---|---|
| I5 / I6 (purge) | `purge_removes_expired_prompt_blob_keeps_shared`, `purge_all_prompts_*`, `purge_snapshots_before_date_respects_keepset` — keep-set dedup real (shared blob survives), `log` still renders excerpts after purge. |
| I+ (TTL / eviction) | `enforce_budget_evicts_oldest_snapshots_only` → `Evicted{count:1,bytes:8}`; oldest evicted, shared+newest kept; `status` over-budget notice. |
| I++ perms (D37) | Live daemon write path: `.agentrec`=0700; `log.jsonl`/`signal.jsonl`/`state.json`/blob all 0600. (Caught+fixed blocker: daemon files had been 0644.) |
| I++ scrub (D38) | `secret_prompt_never_reaches_disk_in_cleartext` greps signal.jsonl + log.jsonl + every blob; cleartext absent, `[redacted:` present. |
| Y+3 / Y+4 / Y+5 | Live: init prints `to reverse everything: agentrec uninstall`, re-run no-op; uninstall archives to `.agentrec.archived.<ts>` (nothing deleted); `--dry-run` touches nothing. Unit: `launchd_plist_shape`/`systemd_unit_shape`. |
| Y++ doctor | 8 doctor tests (daemon-down, hook missing/malformed, signal-stale, degraded, bad-perms, json, healthy); live `--json` `{"ok":false}` exit 1, healthy exit 0. |
| Z+2 / Z+3 / Z+4 | Golden relative-time buckets; `should_color` truth table + no ESC when piped/NO_COLOR; `--explain` glossary only for present terms. |
| H++ (one run) | 1200 ops, INV1+INV2 asserted every undo, 0 violations, 0 orphans; deterministic via `AGENTREC_TORTURE_SEED`. |

## Open (still gated — environment/time absent here)
_(see the Open table near the top; unchanged this round.)_ The 7-night D36 streak, Ubuntu/clean-macOS-VM/brew installer legs, Linux inotify + Linux-perms modes, README GIF, and CI matrix J3 remain the only un-run items.

## Follow-ups from the final gate (accepted, non-blocking)
- **CONCERN #2 — FIXED:** the torture default seed was a fixed constant, so a nightly D36 run with an unset seed would repeat one interleaving 7× and never broaden INV2 coverage. `env_seed()` now derives from the wall clock when `AGENTREC_TORTURE_SEED` is unset (explicit seed still honored + printed for reproducibility). Verified: two unset-seed runs print different seeds.
- **CONCERN #1 (accepted):** the 1200-op run exercised INV2 (undo-of-undo byte-exact) on only 2/21 checkpoints; INV2 also has dedicated integration coverage. With the varying nightly seed (above), the 7-night streak will accumulate broader INV2 coverage — folded into the D36 launch-gate ladder row.
- **CONCERN #3 (accepted, cosmetic):** idempotent `init` re-run reprints "scaffolded"/"set 0700" lines though it redid no work (operation is genuinely idempotent — no hook dup). Message-only nicety, deferred.
