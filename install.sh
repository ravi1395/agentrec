#!/bin/sh
# agentrec installer — POSIX sh, curl|sh style.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/ravi1395/agentrec/main/install.sh | sh
#
# Env overrides:
#   AGENTREC_RELEASE_BASE   base URL releases are fetched from (default: GitHub
#                           releases "latest" asset directory)
#   AGENTREC_INSTALL_DIR    install directory (default: "$HOME/.local/bin")
#   AGENTREC_LOCAL_BINARY   path to an already-built local binary; skips the
#                           network download entirely (local verification leg)
#
# Flags:
#   --local <path>          same as AGENTREC_LOCAL_BINARY
#   --sha256 <sum>          expected sha256 of the binary being installed;
#                           required to actually verify in local mode (without
#                           it the checksum is computed and reported, not
#                           checked against anything)
#   -h, --help               print this usage and exit
#
# No sudo is ever used. Uninstall: rm "$AGENTREC_INSTALL_DIR/agentrec" (default
# rm ~/.local/bin/agentrec).

set -eu

bin_name="agentrec"
release_base="${AGENTREC_RELEASE_BASE:-https://github.com/ravi1395/agentrec/releases/latest/download}"
install_dir="${AGENTREC_INSTALL_DIR:-$HOME/.local/bin}"
local_binary="${AGENTREC_LOCAL_BINARY:-}"
expected_sha256=""

usage() {
    sed -n '2,20p' "$0"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --local)
            local_binary="$2"
            shift 2
            ;;
        --sha256)
            expected_sha256="$2"
            shift 2
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "install.sh: unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

fail() {
    echo "install.sh: error: $1" >&2
    exit 1
}

# --- OS / arch detection -----------------------------------------------

detect_os() {
    uname_s=$(uname -s)
    case "$uname_s" in
        Darwin) echo "darwin" ;;
        Linux) echo "linux" ;;
        *) fail "unsupported OS: $uname_s (agentrec ships macOS + Linux binaries only)" ;;
    esac
}

detect_arch() {
    uname_m=$(uname -m)
    case "$uname_m" in
        arm64 | aarch64) echo "arm64" ;;
        x86_64 | amd64) echo "x86_64" ;;
        *) fail "unsupported architecture: $uname_m" ;;
    esac
}

# --- sha256 -------------------------------------------------------------

sha256_of() {
    target="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$target" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$target" | cut -d ' ' -f 1
    else
        fail "no sha256 tool found (need sha256sum or shasum)"
    fi
}

# --- download -------------------------------------------------------------

download() {
    src_url="$1"
    dest_path="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$dest_path" "$src_url" || fail "download failed: $src_url"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$dest_path" "$src_url" || fail "download failed: $src_url"
    else
        fail "neither curl nor wget found"
    fi
}

# --- install ---------------------------------------------------------------

mkdir -p "$install_dir"
dest="$install_dir/$bin_name"

if [ -n "$local_binary" ]; then
    # Local-binary leg: install from an already-built binary, no network.
    [ -f "$local_binary" ] || fail "AGENTREC_LOCAL_BINARY/--local path not found: $local_binary"

    actual_sha256=$(sha256_of "$local_binary")
    if [ -n "$expected_sha256" ]; then
        if [ "$actual_sha256" != "$expected_sha256" ]; then
            fail "checksum mismatch: expected $expected_sha256, got $actual_sha256"
        fi
        echo "checksum verified: $actual_sha256"
    else
        echo "no --sha256 given; computed sha256 (unverified): $actual_sha256"
    fi

    cp "$local_binary" "$dest"
    chmod +x "$dest"
else
    # Network leg: download a release asset + its checksum, verify, install.
    # LADDERED — cannot be exercised end-to-end without a published release.
    os=$(detect_os)
    arch=$(detect_arch)
    asset="${bin_name}-${os}-${arch}"
    asset_url="$release_base/$asset"
    sum_url="$asset_url.sha256"

    tmp_dir=$(mktemp -d)
    trap 'rm -rf "$tmp_dir"' EXIT

    echo "downloading $asset_url"
    download "$asset_url" "$tmp_dir/$bin_name"
    download "$sum_url" "$tmp_dir/$bin_name.sha256"

    expected_sha256=$(cut -d ' ' -f 1 <"$tmp_dir/$bin_name.sha256")
    actual_sha256=$(sha256_of "$tmp_dir/$bin_name")
    if [ "$actual_sha256" != "$expected_sha256" ]; then
        fail "checksum mismatch for $asset: expected $expected_sha256, got $actual_sha256"
    fi
    echo "checksum verified: $actual_sha256"

    cp "$tmp_dir/$bin_name" "$dest"
    chmod +x "$dest"
fi

echo "installed $dest"

case ":$PATH:" in
    *":$install_dir:"*) ;;
    *)
        echo ""
        echo "$install_dir is not on your PATH. Add this to your shell profile:"
        echo "  export PATH=\"$install_dir:\$PATH\""
        ;;
esac

echo ""
echo "verify: $dest --version"
"$dest" --version || true

echo ""
echo "uninstall: rm $dest"
