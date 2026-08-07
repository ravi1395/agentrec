#!/usr/bin/env bash
# agentrec MCP demand sweep — the D7/D10 post-ship instrument (delta spec gap 10).
#
# WHAT IT DOES: scans Claude Code transcripts (`~/.claude/projects/**/*.jsonl`)
# for tool-call names that look like invocations of agentrec's MCP tools, and
# prints the candidates it found. That is all it does.
#
# WHAT IT DOES NOT DO: it does not decide whether the MCP surface is earning
# its keep. Per the D7 clauses the audit is MANUAL — a name appearing in a
# transcript is a candidate, not a verified invocation (a name can appear in
# prose, in a tool *definition* echoed into context, or in a call the host
# rejected). Read the candidates, then judge.
#
# SAFETY: read-only (grep only, no writes anywhere), no network, and a missing
# transcript directory is reported and exits 0 rather than failing.
#
# CADENCE: first sweep is ship + 1 month; see the "MCP demand sweep" row in
# VERIFY-LEDGER.md. The date is load-bearing — Claude Code prunes transcripts
# on a ~30-day rolling window, so a skipped month is unrecoverable evidence.
#
# USAGE: scripts/mcp-demand-sweep.sh [projects-dir]     (default ~/.claude/projects)

set -euo pipefail

projects_dir="${1:-${HOME:-}/.claude/projects}"

echo "agentrec MCP demand sweep"
echo "scanned: $projects_dir"

if [ ! -d "$projects_dir" ]; then
  echo "no transcript directory at $projects_dir — nothing to sweep"
  echo "(this is not an error: Claude Code writes it only once it has run here)"
  exit 0
fi

# Real transcripts spell a host-side MCP call as
#   "name":"mcp__<server>__<tool>"
# (measured against this machine's own transcripts). The server segment is
# whatever the host config named us, so it is matched loosely; the bare
# `agentrec_<tool>` form is accepted too, for hosts that do not namespace.
pattern='"name"[[:space:]]*:[[:space:]]*"(mcp__[A-Za-z0-9_.-]+__)?agentrec_(log|diff|blame|recall|status|undo)"'

files_total=$(find "$projects_dir" -type f -name '*.jsonl' 2>/dev/null | wc -l | tr -d ' ')
echo "transcripts: $files_total"

matches=$(grep -REoh --include='*.jsonl' "$pattern" "$projects_dir" 2>/dev/null || true)

if [ -z "$matches" ]; then
  echo
  echo "candidate invocations: 0"
  echo "no candidate agentrec MCP tool invocations found"
  exit 0
fi

candidate_total=$(printf '%s\n' "$matches" | grep -c . || true)
echo
echo "candidate invocations: $candidate_total"

echo
echo "by tool:"
printf '%s\n' "$matches" \
  | sed -E 's/.*"(mcp__[A-Za-z0-9_.-]+__)?(agentrec_[a-z]+)"$/\2/' \
  | sort | uniq -c | sort -rn \
  | while read -r count tool; do printf '  %6s  %s\n' "$count" "$tool"; done

echo
echo "by transcript (candidates > 0):"
grep -REc --include='*.jsonl' "$pattern" "$projects_dir" 2>/dev/null \
  | awk -F: '$NF > 0 { n = $NF; sub(/:[0-9]+$/, "", $0); printf "  %6s  %s\n", n, $0 }' \
  | sort -rn || true

echo
echo "these are CANDIDATES — audit them by hand (D7); the script judges nothing"
