#!/usr/bin/env bash
# Builds docs/fixtures/demo-repo: a throwaway git repo with one real
# agentrec turn recorded by the real daemon (not staged/faked text).
# Run this before docs/blame-demo.tape — the tape drives this repo live.
#
#   ./docs/generate-blame-fixture.sh
#   vhs docs/blame-demo.tape
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/release/agentrec"
FIXTURE="$ROOT/docs/fixtures/demo-repo"

if [ ! -x "$BIN" ]; then
  echo "building release binary..."
  (cd "$ROOT" && cargo build --release)
fi

rm -rf "$FIXTURE"
mkdir -p "$FIXTURE/src"
cd "$FIXTURE"
git init -q
git config user.email demo@example.com
git config user.name demo

"$BIN" init --no-hook --no-service --root "$FIXTURE" >/dev/null

"$BIN" record --root "$FIXTURE" >/tmp/agentrec-fixture-daemon.log 2>&1 &
DAEMON_PID=$!
trap 'kill "$DAEMON_PID" 2>/dev/null || true' EXIT
sleep 1

echo '{"hook_event_name":"UserPromptSubmit","session_id":"s_demo","prompt":"add rate limiting to login"}' \
  | "$BIN" hook claude-code --root "$FIXTURE"

cp "$ROOT/docs/fixtures/auth.ts.source" "$FIXTURE/src/auth.ts"
sleep 2

echo '{"hook_event_name":"Stop","session_id":"s_demo"}' \
  | "$BIN" hook claude-code --root "$FIXTURE"
sleep 2

kill "$DAEMON_PID" 2>/dev/null || true
wait "$DAEMON_PID" 2>/dev/null || true
trap - EXIT

echo "fixture ready at $FIXTURE"
"$BIN" blame src/auth.ts:42 --root "$FIXTURE"
