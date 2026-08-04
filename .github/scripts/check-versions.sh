#!/usr/bin/env bash
# Assert every version-carrying manifest agrees with the release version.
#
# Usage: .github/scripts/check-versions.sh X.Y.Z        (no leading "v")
#
# Run by the release workflow's verify-version gate before any binary is built,
# and by the agentrec-release skill locally before the bump is committed — same
# script both places, so a green local run means the gate cannot surprise you.
#
# Fields are read structurally (cargo metadata / jq), never by grepping the
# version string: `cli/Cargo.toml` carries third-party pins like `regex = "1"`
# beside the `agentrec-core` pin, so a string grep false-matches the day a dep
# version collides with ours.
set -euo pipefail

EXPECTED="${1:-}"
if ! [[ "$EXPECTED" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 X.Y.Z   (got: '${EXPECTED}')" >&2
  exit 2
fi

cd "$(dirname "$0")/../.."

fail=0
check() { # check <label> <actual> <expected>
  if [[ "$2" == "$3" ]]; then
    printf 'ok       %-52s %s\n' "$1" "$2"
  else
    printf 'MISMATCH %-52s %s (expected %s)\n' "$1" "${2:-<missing>}" "$3"
    fail=1
  fi
}

meta="$(cargo metadata --no-deps --format-version 1)"

# 1+2. Both workspace crates (both inherit version.workspace from /Cargo.toml).
for crate in agentrec agentrec-core; do
  check "Cargo.toml (${crate})" \
    "$(jq -r --arg c "$crate" '.packages[]|select(.name==$c).version' <<<"$meta")" \
    "$EXPECTED"
done

# 3. The cli -> core path-dep pin. Missing this breaks `cargo publish -p agentrec`,
#    which resolves the pin against the registry rather than the path.
check "cli/Cargo.toml (agentrec-core dep pin)" \
  "$(jq -r '.packages[]|select(.name=="agentrec").dependencies[]|select(.name=="agentrec-core").req' <<<"$meta")" \
  "^$EXPECTED"

# 4. Cargo.lock — drifts silently whenever the bump skips `cargo update --workspace`.
for crate in agentrec agentrec-core; do
  check "Cargo.lock (${crate})" \
    "$(awk -v c="$crate" '/^name = /{n=$3} /^version = /{if (n=="\""c"\"") {gsub(/"/,"",$3); print $3; exit}}' Cargo.lock)" \
    "$EXPECTED"
done

# 5. npm wrapper.
check "npm/package.json" \
  "$(jq -r '.version' npm/package.json)" "$EXPECTED"

# 6. Plugin marketplace — the version lives under `metadata`, NOT `plugins[].version`.
check ".claude-plugin/marketplace.json (metadata.version)" \
  "$(jq -r '.metadata.version' .claude-plugin/marketplace.json)" "$EXPECTED"

# 7. Plugin manifest.
check "claude-plugin/agentrec/.claude-plugin/plugin.json" \
  "$(jq -r '.version' claude-plugin/agentrec/.claude-plugin/plugin.json)" "$EXPECTED"

if [[ $fail -ne 0 ]]; then
  echo >&2
  echo "version drift: the manifests above disagree with ${EXPECTED}." >&2
  echo "bump them in lockstep (agentrec-release skill), re-run \`cargo update --workspace\`, and retag." >&2
  exit 1
fi

echo
echo "all manifests agree at ${EXPECTED}"
