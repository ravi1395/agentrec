# attest purity census — `std::process::Command::new`

**AC-ATTEST-P4C-1 (baseline green) and AC-ATTEST-P4C-2 (the plant probe).**
Branch `feat/attest`, on top of `5bd5c0f`. Everything below was run live;
commands and output are pasted, not paraphrased.

## What this establishes, and what it does not

`clippy.toml` disallows `std::process::Command::new`. Every legitimate
production spawn in `cli/src` carries a per-site
`#[allow(clippy::disallowed_methods)]` with its own reason, so the lint is a
complete, compiler-enforced census of the production spawn surface.
`cli/src/daemon.rs` production code carries **no** allow, which is the property
attest spec decision 6 asks for: the daemon must never execute repo-authored
commands, so a direct spawn added there is a build failure.

**Honest scope, stated once: this is a census of DIRECT call sites, not a
transitivity proof.** It does not establish that no function reachable from the
daemon can spawn through another module's allowed site. `coveragecmd.rs` and
`replaycmd.rs`, which spawn cargo, are exactly the uncovered shape. Real
call-graph analysis is out of v1 scope and is not claimed here.

Two further limits, both real:

- **Test code is out of scope by design.** All **19** spawning `cli/tests/*.rs`
  files carry a file-level `#![allow(clippy::disallowed_methods)]` (measured;
  in `golden.rs` and `torture.rs` it sits below a long module doc rather than at
  the top of the file, so a short head-of-file scan misses it). In `cli/src`, exactly
  three `#[cfg(test)] mod tests` blocks contain a spawn: `daemon.rs` and
  `service.rs` already carried a module-scoped allow, and **`doctorcmd.rs`
  carried none** — this round added one there, with a reason true of that mod
  (it spawns `git` to build its own fixtures) rather than a copy of the
  fsguard read-oriented wording used elsewhere.
- **Darwin-only lint census.** Clippy only sees code the target compiles.
  A `#[cfg(target_os = "linux")]` body left unannotated would red Linux CI and
  not this machine, so the sweep was done by reading source, not by chasing
  clippy output. This repo already carries the same residual from the PR #20
  fsguard round. **`service.rs` is not affected**: its launchctl/systemctl
  split uses the runtime `cfg!` macro, so both arms compile and clippy sees
  both on every platform (measured — the sweep annotated `load`/`unload` once
  each and both arms were covered).

## Per-site census — 30 production sites

Measured at task time by scanning the region of each file before its first
`#[cfg(test)]`, not copied from the plan. Each site carries a per-site
`#[allow]` except where noted.

| file (`cli/src/`) | fn | what it spawns, and why it is legitimate |
|---|---|---|
| `service.rs` | `load` ×4 | `launchctl` / `systemctl` — fixed program names; installing a service unit is inherently out-of-process. Allow is on the fn: two of the four are the block's tail expression, and expression-position attributes are unstable. |
| `service.rs` | `unload` ×2 | `launchctl` / `systemctl` to unload what `load` installed. Same, allow on the fn. |
| `importcmd.rs` | `load` ×2 | `git rev-parse --show-toplevel` and `git ls-files` — fixed subcommands. |
| `importcmd.rs` | `resolve_t2_before` ×2 | `git log` / `git show` to derive a committed `before` blob. |
| `doctorcmd.rs` | `probe_mcp_initialize` | spawns THIS binary (`exe` is our own resolved path) to probe the daemon. |
| `attest/adapter_cargo.rs` | `build_targets` | `cargo test --no-run` — the sanctioned test-runner boundary. |
| `attest/adapter_cargo.rs` | `list_tests` | the BUILT libtest binary with `--list`; the path came from cargo's artifact stream. |
| `attest/adapter_cargo.rs` | `run` | `cargo test` scoped to one target and one `--exact` name — the replay itself. |
| `attest/capture.rs` | `tree_is_dirty` | `git status` for capture provenance. |
| `attest/capture.rs` | `run_wrapped` | the user's own test command; the argv is what the user invoked. |
| `attest/coveragecmd.rs` | `head_commit` | `git rev-parse` for map provenance. Allow on the fn (tail expression). |
| `attest/coveragecmd.rs` | `tooling_status` | `cargo llvm-cov --version`, an availability check. |
| `attest/coveragecmd.rs` | `instrumented_build` ×2 | `cargo llvm-cov show-env`, then the instrumented `cargo test --no-run`. |
| `attest/coveragecmd.rs` | `capture_one` ×3 | a built libtest binary under instrumentation, then `llvm-profdata` and `llvm-cov` from the resolved rustup toolchain. |
| `attest/reportcmd.rs` | `commit_time_ms` | `git log -1 --format=%ct` on a user-supplied revision passed as one argv element (Phase 5; absent from this table until the `feat/phase-3-0` merge re-measured it). |
| `attest/replaycmd.rs` | `ensure_clean_tree` | `git status` — refuses a dirty tree before minting a verdict. |
| `attest/replaycmd.rs` | `head_commit` | `git rev-parse` to pin the replay commit. |
| `annotatecmd.rs` | `git` | `git log` / `git show` with fixed subcommands to read commit history (added at the `feat/phase-3-0` merge). |
| `bisectcmd.rs` | `run_test` | the user's own `--test` command, typed on the CLI by the invoker (added at the `feat/phase-3-0` merge). |
| `attest/replaycmd.rs` | `extract_commit` ×2 | `git archive` (never `git worktree add`, which writes into the production repo's `.git`), then `tar` to unpack it. |
| **`daemon.rs`** | — | **0 sites. No allow anywhere in its production code.** |

Per-file totals: `service.rs` 6, `importcmd.rs` 4, `doctorcmd.rs` 1,
`annotatecmd.rs` 1, `bisectcmd.rs` 1, `attest/reportcmd.rs` 1,
`attest/coveragecmd.rs` 7, `attest/replaycmd.rs` 4, `attest/adapter_cargo.rs` 3,
`attest/capture.rs` 2, `daemon.rs` 0 — **30** (re-measured at the `feat/phase-3-0`
merge by counting `Command::new` before each file's first `#[cfg(test)]`; the
earlier **27** predated Phase 5's `reportcmd.rs` site).

**This count differs from the plan's, and the difference is expected.** The
plan's Phase 4 section names 13 production sites over
`service.rs`/`importcmd.rs`/`doctorcmd.rs`/`bisectcmd.rs`/`annotatecmd.rs`,
measured in its round 5. Two reasons it no longer holds: Phase 3 and Phase 4A/B
landed 16 attest spawn sites after that measurement, and `bisectcmd.rs` /
`annotatecmd.rs` do not exist on this branch (they live on
`feat/phase-3-0`). The plan itself says to re-enumerate at task time.

**One correction to the task brief:** `service.rs`'s 6 production sites are
launchctl/systemctl only. Its `mkfifo` spawn lives in
`service.rs::tests::install_refuses_when_unit_path_is_a_fifo`, i.e. inside the
`#[cfg(test)] mod tests` — it is test-side and already covered by that mod's
blanket allow. (Anchors, not line numbers: the figures this sentence first
carried had already rotted by two commits, which is the rot this repo records
against itself.)

## Direction 1 — baseline green after the sweep

```
$ cargo clippy --workspace --all-targets --all-features -- -D warnings
    Checking agentrec-core v0.2.0 (/Users/ravichandrasekhar/Projects/agentrec/agentrec-core)
    Checking agentrec v0.2.0 (/Users/ravichandrasekhar/Projects/agentrec/cli)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.15s
```

```
$ cargo clippy --release --workspace --all-targets --all-features -- -D warnings
    Checking agentrec-core v0.2.0 (/Users/ravichandrasekhar/Projects/agentrec/agentrec-core)
    Checking agentrec v0.2.0 (/Users/ravichandrasekhar/Projects/agentrec/cli)
    Finished `release` profile [optimized] target(s) in 2.52s
```

Both clean on the first attempt, which is what says the sweep found every site
the lint can see. A one-directional green proves nothing on its own — hence
direction 2.

## Direction 2 — the plant probe

Planted inside `run_eviction_pass`, an **existing** production fn of
`cli/src/daemon.rs`. An unreferenced NEW fn would co-fire `dead_code` under
`-D warnings`, which also "names the line" and would let the probe pass with
`clippy.toml` untouched — so the plant had to go in an existing, plainly
compiled, non-`cfg`-gated body.

```
$ shasum -a 256 cli/src/daemon.rs        # before the plant
6349a265f769da9efb329dbcaa6d20d0247b5544e793409d7faa0dc54ab400c7  cli/src/daemon.rs
```

Planted at what became line 94:

```rust
fn run_eviction_pass(root: &Path) {
    let _probe = std::process::Command::new("true");
    let store = BlobStore::new(objects_dir(root));
```

```
$ cargo clippy --workspace --all-targets --all-features -- -D warnings
error: use of a disallowed method `std::process::Command::new`
  --> cli/src/daemon.rs:94:18
   |
94 |     let _probe = std::process::Command::new("true");
   |                  ^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: the daemon must never execute repo-authored commands (attest spec decision 6); every legitimate spawn carries a per-site #[allow(clippy::disallowed_methods)] with its reason
   = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.97.0/index.html#disallowed_methods
   = note: `-D clippy::disallowed-methods` implied by `-D warnings`
   = help: to override `-D warnings` add `#[allow(clippy::disallowed_methods)]`

error: could not compile `agentrec` (bin "agentrec") due to 1 previous error
warning: build failed, waiting for other jobs to finish...
error: could not compile `agentrec` (bin "agentrec" test) due to 1 previous error
```

`clippy::disallowed_methods` fired, named `cli/src/daemon.rs:94:18`, and was the
only diagnostic — no `dead_code` or other lint muddied which one refused.

The plant was reverted by an inverse edit (never `git checkout`: the working
tree carries uncommitted work), and the file is byte-identical to before:

```
$ shasum -a 256 cli/src/daemon.rs        # after the revert
6349a265f769da9efb329dbcaa6d20d0247b5544e793409d7faa0dc54ab400c7  cli/src/daemon.rs
$ git diff --stat -- cli/src/daemon.rs
(no output)
```

The probe ran BEFORE this round's `MatchKind` edit to `daemon.rs`
(AC-ATTEST-P4C-5), so the hash above is the pre-edit file on both sides of the
plant and the empty diffstat is a real check, not a vacuous one.

**`6349a265…` is therefore NOT reproducible at this round's final tree, by
design.** `daemon.rs` changes after the probe for AC-ATTEST-P4C-5, and `cargo
fmt` ran after that. The final-tree hash is:

```
$ shasum -a 256 cli/src/daemon.rs        # end of chunk C, post-MatchKind, post-fmt
7ef2b7a42b25a79e55f01dfe2281ef77c671464da8068e7a2a8107b71f8aa8f8  cli/src/daemon.rs
```

Both figures are recorded so neither reads as "the current one" by default. The
property the probe established is unaffected: `daemon.rs` production code
carries no `#[allow(clippy::disallowed_methods)]` at either hash, and the final
clippy runs (debug and release) are green with it that way.
