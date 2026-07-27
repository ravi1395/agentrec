#!/usr/bin/env bash
# scripts/linux-leg.sh — the manual Colima Linux leg, codified.
#
# Why this exists (residuals round, Phase 5): the directory-admission
# machinery this round shipped (macOS rename-in admission alongside the
# pre-existing Linux Create(_)|Modify(Name(_)) arm, cli/src/daemon.rs) is
# now testable ONLY on Linux, and ci.yml has no automatic branch-push
# trigger (see the workflow_dispatch comment in .github/workflows/ci.yml —
# GitHub only dispatches workflows already on the default branch, so this
# manual leg stays load-bearing until this file lands on `main`). Until
# now the procedure lived only in prose in this repo's CLAUDE.md; this
# script makes it reproducible instead of tribal knowledge.
#
# Load-bearing details, every one of them learned the hard way in prior
# rounds — do not "simplify" any of these away:
#
#   1. rust:1-bookworm, arm64. Colima on Apple Silicon defaults to an
#      arm64 VM; running the amd64 image would silently emulate under
#      Rosetta/qemu and the timing-sensitive daemon tests are not proven
#      under emulation. This script hard-checks the container's actual
#      `uname -m` rather than trusting configuration.
#
#   2. MUST run non-root inside the container. As root, `chmod 000`
#      fixtures pass VACUOUSLY — root bypasses Unix permission bits
#      entirely — which is exactly what made the two #[cfg(unix)]
#      permission tests (lock_file_sets_0600, lock_dir_sets_0700 and
#      friends) fail spuriously on the first real attempt at this leg.
#      This script creates a real unprivileged user and runs the suite
#      as that user, then asserts `id -u` is non-zero before proceeding
#      as a hard gate, not a hope.
#
#   3. Fixtures must land in the container's OWN /tmp (its overlayfs
#      layer, native to the VM), never inside a virtiofs-backed bind
#      mount — Colima shares the host filesystem into the VM over
#      virtiofs, and the daemon under test must watch a real Linux
#      filesystem via real inotify, not virtiofs's translation layer.
#      `tempfile::tempdir()` (what the test suite uses for fixtures)
#      resolves via `$TMPDIR`, defaulting to `/tmp` — so this script
#      deliberately does NOT set TMPDIR into the bind-mounted workspace
#      and does NOT bind-mount /tmp itself. Only the repo source is
#      bind-mounted; test fixtures fall through to the container-native
#      /tmp untouched.
#
#   4. Real inotify, `fs.inotify.max_user_watches` raised well above a
#      fresh `agentrec init`'s directory count.
#
#      CAVEAT — the MECHANISM here is an inference, not recovered history.
#      What the round actually recorded is the VALUE (1048576), never how it
#      was applied. `fs.inotify.max_user_watches` is a VM-wide kernel knob
#      rather than a per-container-namespaced one, so `docker run --sysctl`
#      cannot set it; this script therefore sets it on the Colima VM itself
#      via `colima ssh`. That reasoning was originally an UNVERIFIED inference
#      against the round's manual session; the 2026-07-27 first run VERIFIED
#      the mechanism itself — the script echoes the value from inside the
#      container (see below), and propagation was independently probed by
#      lowering the VM knob and watching the container report the lowered
#      value. Note the default on that Colima version was already 1048576,
#      so a green run alone never proves the sysctl call did anything; the
#      in-container echo is the load-bearing evidence.
#
# STATUS: first executed 2026-07-27 (Colima 4-CPU arm64 VM, rust:1-bookworm).
# Suite green: 431 passed / 0 failed / 2 ignored; host repo untouched (0
# ownership diffs, macOS target/ mtimes identical) — the two defects the
# residuals-round gate had found by reading alone (a `chown -R` through the
# rw bind mount, container `target/` clobbering the host's) stayed fixed at
# runtime. The first run found two more only execution could surface: the
# missing `--show-output` (libtest captured the rebuild-count print from a
# passing test — evidence-on-green silently defeated) and a silent-truncation
# false pass (inner block truncated -> container exit 0, zero tests run,
# "linux leg complete" printed), now guarded by the `.suite-ran` sentinel.
# History kept because it is this repo's sharpest demonstration that
# `shellcheck` clean is not a run.
#
# Fails loudly and early on any unmet precondition (no colima, colima not
# running, wrong arch, no docker) rather than silently running a degraded
# leg — a script that quietly falls back to root or amd64 would reintroduce
# exactly the false-pass classes items 1-2 above exist to prevent.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="rust:1-bookworm"
PLATFORM="linux/arm64"
CONTAINER_NAME="agentrec-linux-leg"
MAX_USER_WATCHES=1048576
# Named docker volume (lives in the Colima VM on ext4 — container-native, NOT
# the virtiofs mount, so item 3's fixture-locality rule is unaffected). This is
# a build CACHE only. Measured on a 4-CPU arm64 Colima VM: a full cold leg
# (compile + suite) is ~144s, of which the compile is ~40s — so this saves less
# than it might seem, and a clean-room run is cheap. `docker volume rm
# agentrec-linux-leg-target` to force one.
TARGET_VOLUME="agentrec-linux-leg-target"

log() { printf '[linux-leg] %s\n' "$*" >&2; }
fail() { printf '[linux-leg] FATAL: %s\n' "$*" >&2; exit 1; }

# --- Preconditions: fail loudly, not silently-degraded. ---

command -v colima >/dev/null 2>&1 \
  || fail "colima not found on PATH — install with 'brew install colima' first"
command -v docker >/dev/null 2>&1 \
  || fail "docker not found on PATH"

colima status >/dev/null 2>&1 \
  || fail "colima is not running — start it first, e.g.: colima start --arch aarch64 --cpu 4 --memory 4"

log "pulling ${IMAGE} (${PLATFORM})"
docker pull --platform "$PLATFORM" "$IMAGE" >/dev/null

ACTUAL_ARCH="$(docker run --rm --platform "$PLATFORM" "$IMAGE" uname -m)"
case "$ACTUAL_ARCH" in
  aarch64|arm64) ;;
  *) fail "container arch is '${ACTUAL_ARCH}', expected aarch64/arm64 — the admission machinery has only been measured on arm64 Colima; do not trust an amd64/emulated run for this leg" ;;
esac
log "arch confirmed: ${ACTUAL_ARCH}"

log "raising fs.inotify.max_user_watches on the Colima VM to ${MAX_USER_WATCHES}"
colima ssh -- sudo sysctl -w "fs.inotify.max_user_watches=${MAX_USER_WATCHES}" \
  || fail "could not raise fs.inotify.max_user_watches on the Colima VM"

log "running the suite non-root, fixtures in container-native /tmp"
# The repo is mounted READ-ONLY and copied to a container-native working
# directory before anything builds. Two host-mutation bugs this avoids, both
# caught at the round's gate before this script had ever been run:
#   - `chown -R builder:builder /workspace` on an rw bind mount rewrites the
#     HOST repo's ownership metadata through virtiofs. The container needs a
#     writable tree; the host does not need to pay for it.
#   - an rw mount also puts the container's `target/` at the same non-triple
#     `target/debug` path the host's macOS build uses, so a Linux run
#     clobbers the host's build cache. `CARGO_TARGET_DIR` now points at
#     container-native storage, off the mount entirely.
# `target/` is excluded from the copy: it is the host's macOS artifacts, is
# large, and is exactly what must not travel.
#
# QUOTING WARNING — the inner block below is doubly nested: a single-quoted
# argument to `bash -euxc`, containing a double-quoted argument to `su -c`.
# Inside it, an apostrophe closes the outer string, and an unescaped double
# quote or a backtick closes/substitutes inside the inner one. All three were
# hit for real on the first execution of this script; the failure mode is NOT a
# loud syntax error but a silently TRUNCATED script that exits 0 having run no
# tests. Keep the inner block free of prose — explain things out here instead.
#
# Two flags on the cargo line, both load-bearing:
#   - `--show-output`, not `--nocapture`: libtest captures `eprintln!` from a
#     PASSING test, so `rebuild_count_is_bounded_by_writes` prints its observed
#     count into a void without it — the residuals round added that print to
#     make the bound evidenced on green, and this script silently defeated it.
#     `--nocapture` would work too but interleaves across `--test-threads=3`
#     and can garble the `test result:` summary lines.
#   - the `.suite-ran` sentinel + its post-run check: proof the suite was
#     actually reached, rather than inferring it from a 0 exit code.
#
# The sentinel CLEAR runs here, host-side, in its own container — NOT inside
# the fragile block below. The first shape of this guard cleared it inside the
# block, which defeats itself: a truncation landing before the clear leaves a
# prior green run's sentinel in the persistent volume, and the check then
# passes with zero tests run (demonstrated live at the post-run skeptic gate:
# injected truncation + stale sentinel -> "linux leg complete", exit 0). After
# the first green run a stale sentinel is the volume's steady state, so an
# in-block clear guards only truncations that land after itself.
docker run --rm --platform "$PLATFORM" \
  -v "${TARGET_VOLUME}:/work-target" "$IMAGE" \
  rm -f /work-target/.suite-ran

docker run --rm --name "$CONTAINER_NAME" \
  --platform "$PLATFORM" \
  -v "${REPO_ROOT}:/src:ro" \
  -v "${TARGET_VOLUME}:/work-target" \
  -w /work \
  -e CARGO_TARGET_DIR=/work-target \
  "$IMAGE" \
  bash -euxc '
    set -euo pipefail

    # Real unprivileged user — item 2 above. No chmod-000 fixture may pass
    # while this script runs as root.
    id -u builder >/dev/null 2>&1 || useradd -m -s /bin/bash builder

    mkdir -p /work /work-target
    # `.agentrec` is this repo dogfooding itself — a ~75 MiB content-addressed
    # blob store the suite never reads (every test builds its own tempdir
    # root), so copying it in is pure cost. `target` is the host`s macOS
    # artifacts. `.git` IS kept: cheap (~13 MiB) and `ignore::WalkBuilder`
    # honors `require_git`, so removing it could silently change ignore
    # behavior — the exact class of vacuous-fixture bug this repo has already
    # shipped once.
    tar -C /src --exclude=./target --exclude=./.agentrec -cf - . | tar -C /work -xf -
    chown -R builder:builder /work /work-target /usr/local/cargo /usr/local/rustup

    su builder -s /bin/bash -c "
      set -euo pipefail
      cd /work

      # Hard gate, not a hope: refuse to continue if somehow still root.
      test \"\$(id -u)\" -ne 0 || { echo FATAL: still running as root >&2; exit 1; }
      echo \"running as uid=\$(id -u) (\$(whoami))\"

      # Deliberately NOT setting TMPDIR — item 3 above. Fixtures must
      # resolve to the container-native /tmp, not the bind-mounted
      # /workspace (which sits on Colima virtiofs).
      unset TMPDIR || true
      echo \"TMPDIR unset; tempfile fixtures will land under: \$(df -T /tmp | tail -1)\"

      echo \"container fs.inotify.max_user_watches = \$(cat /proc/sys/fs/inotify/max_user_watches)\"

      cargo test --workspace --all-features --no-fail-fast -- --test-threads=3 --show-output

      touch /work-target/.suite-ran
    "
  '

# The inner block ran to completion only if the suite actually executed. Without
# this, a mid-script syntax break (see the quoting warning above) lets the
# container exit 0 having run ZERO tests while this script still prints
# "linux leg complete" — a false PASS observed for real on the first run.
docker run --rm --platform "$PLATFORM" \
  -v "${TARGET_VOLUME}:/work-target" "$IMAGE" \
  test -f /work-target/.suite-ran \
  || fail "the container exited without reaching the end of the test suite — do NOT read this as a pass"

log "linux leg complete"
