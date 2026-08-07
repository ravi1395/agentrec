#!/usr/bin/env bash
# Probe: does a FRESH fifo (mkfifo, never a regular file) generate an FSEvents
# delivery that reaches the daemon's snapshot read?
#
# Context: two 2026-08-07 gate rounds measured this and disagreed on the
# byte-written row (round 8: 2/2 delivered; round 9: 0/3, delivering 2/2 only
# after an explicit `touch`). Round 8's probe commands were not persisted, so
# the governing stimulus is undetermined. This script persists the shapes so a
# future run can settle it. See the comment above
# `daemon_snapshots_a_fifo_as_skipped_without_losing_the_turn` in
# cli/tests/integration.rs.
#
# Usage: scripts/probe-fresh-fifo-delivery.sh <shape> [wait-secs]
#   shape: bare | rw-open | byte | byte-touch
# Prints the resulting log.jsonl and a DELIVERED/NOT-DELIVERED verdict.
# Runs entirely in a mktemp fixture; kills its own daemon on exit.

set -euo pipefail

SHAPE="${1:?usage: $0 bare|rw-open|byte|byte-touch [wait-secs]}"
WAIT="${2:-35}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$REPO_ROOT/target/debug/agentrec"
[ -x "$BIN" ] || { echo "build first: cargo build" >&2; exit 2; }

FX="$(mktemp -d)"
DAEMON_PID=""
cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        # Wait for the shutdown flush: without this the daemon's final writes
        # race the rm -rf and can resurrect $FX/.agentrec (seen live, round 10).
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    exec 3>&- 2>/dev/null || true
    rm -rf "$FX"
}
trap cleanup EXIT

cd "$FX"
git init -q .
"$BIN" init --no-hook --no-service >/dev/null
"$BIN" record --root "$FX" &
DAEMON_PID=$!
sleep 12   # pipeline warm-up measured ~11.5s in round 9

mkfifo "$FX/data.pipe"
case "$SHAPE" in
    bare) ;;
    rw-open)
        exec 3<> "$FX/data.pipe" ;;
    byte)
        exec 3<> "$FX/data.pipe"
        printf 'x' >&3 ;;
    byte-touch)
        exec 3<> "$FX/data.pipe"
        printf 'x' >&3
        touch "$FX/data.pipe" ;;
    *) echo "unknown shape: $SHAPE" >&2; exit 2 ;;
esac

sleep "$WAIT"

LOG="$FX/.agentrec/log.jsonl"
echo "--- log.jsonl ---"
cat "$LOG" 2>/dev/null || echo "(missing)"
echo "-----------------"
if grep -q '"data.pipe"' "$LOG" 2>/dev/null; then
    echo "DELIVERED (shape=$SHAPE wait=${WAIT}s)"
else
    echo "NOT-DELIVERED (shape=$SHAPE wait=${WAIT}s)"
fi
