#!/usr/bin/env bash
#
# check-write-opens.sh — the WRITE half of the fsguard blocking-I/O gate.
#
# WHY THIS EXISTS. Opening a path that resolves to a FIFO/socket/device blocks
# the calling thread forever. `clippy.toml`'s `disallowed-methods` closes the
# READ half (`fs::read`, `fs::read_to_string`, `File::open`) but CANNOT close
# the write half: clippy matches a method, and `OpenOptions::open` is the same
# method for a guarded read and an unguarded write, so listing it would fire
# on every lock-file open in the tree. `agentrec-core/src/fsguard.rs`'s module
# doc therefore carries a hand-maintained registry of which write sites call
# `fsguard::is_nonregular` inline — a convention, which a newly added site can
# silently skip. This script turns that convention into a CI gate.
#
# DESIGN — allowlist FILE keyed on `path::fn`, not marker comments, not lines.
#   * Not line numbers: this repo has repeatedly been burned by cited line
#     numbers rotting out from under later commits in the same branch.
#   * Not marker comments: a marker travels with a copy-pasted call site, so
#     copying a guarded write into an unguarded context carries its own
#     absolution. An external allowlist cannot be copy-pasted by accident, and
#     it makes the full set of write-opens reviewable in one diff.
#   * `path::fn` survives every edit inside the function; a rename breaks it
#     LOUDLY (stale entry, below) rather than silently.
#
# The allowlist is EXHAUSTIVE AND BIDIRECTIONAL. Each entry carries the number
# of call sites expected in that function, and:
#   * a candidate whose key is absent      -> violation (new unguarded write)
#   * an entry matching zero candidates    -> violation (stale entry)
#   * a count mismatch                     -> violation (site added/removed
#                                             inside an allowlisted function)
# The stale-entry direction is load-bearing, not tidiness: this script's whole
# correctness rests on excluding `#[cfg(test)]` regions, and an over-greedy
# region would make production sites VANISH. A vanished site turns its entry
# stale and reds the build, instead of passing silently.
#
# SCOPE. Production source under agentrec-core/src and cli/src. Test-ONLY
# regions and cli/tests/ are exempt: test code writes its own tempdir
# fixtures. "Test-only" is decided by classifying the cfg attribute, not by a
# substring match — `#[cfg(test)]` and `#[cfg(all(test, ...))]` (containing
# no `any(` and no `not(`) open an exempt region; `#[cfg(any(..., test))]`
# and `#[cfg(not(test))]` gate PRODUCTION code (a disjunction with `test`
# still compiles into real binaries — `doctorcmd.rs::estimate_watch_count`
# is the live example, caught by a gate round when a substring match
# silently exempted it) and are scanned; any other col-0 cfg form carrying a
# `test` token — including `all(` forms nesting `any(` or `not(`, which a
# second gate round measured reopening the same hole one level down — is an
# INTEGRITY abort rather than a guess. An INDENTED unrecognized form arms no
# channel and is scanned: an inner attribute cannot open a col-0 region, so
# the failure direction is over-scan (a false VIOLATION that fails loud),
# never a silent exemption.
#
# KNOWN COLLAPSE, disclosed: allowlist keys are `path::fn`, so two same-named
# functions in one file (e.g. the cfg-paired `create_tmp_file` arms) share
# one entry and one count. A write-open MOVING between same-named twins is
# invisible to the count check; adding or removing one still trips it.
#
# PORTABILITY. bash 3.2 (the macOS floor: no associative arrays, no mapfile)
# and POSIX awk/grep only — CI's lint job runs on ubuntu, so a bash-4-ism or a
# GNU-grep-ism would only ever break on a developer's Mac, never in CI.
# Invoked as `bash scripts/check-write-opens.sh` in CI by repo convention, so
# the gate cannot depend on git recording mode 100755.
#
# EXIT CODES: 0 clean, 1 violation (each named on stderr), 2 usage/environment.

set -u

usage() {
    printf 'usage: bash scripts/check-write-opens.sh\n' >&2
    printf '  no arguments; run from the repository root\n' >&2
    exit 2
}

[ "$#" -eq 0 ] || usage

ROOTS="agentrec-core/src cli/src"
ALLOWLIST="scripts/write-open-allowlist.txt"

for d in $ROOTS; do
    if [ ! -d "$d" ]; then
        printf 'check-write-opens: %s not found — run from the repository root\n' "$d" >&2
        exit 2
    fi
done
if [ ! -f "$ALLOWLIST" ]; then
    printf 'check-write-opens: allowlist %s not found\n' "$ALLOWLIST" >&2
    exit 2
fi

tmpdir=$(mktemp -d) || exit 2
trap 'rm -rf "$tmpdir"' EXIT
candidates="$tmpdir/candidates"

# Pass 1: enumerate production write-open call sites.
#
# Emits TAB-separated: path <TAB> fn <TAB> lineno <TAB> source text.
# Also emits `!!INTEGRITY <path> <what>` lines for any file whose test-region
# detection did not behave, which pass 2 escalates.
find $ROOTS -name '*.rs' -type f | LC_ALL=C sort | while IFS= read -r f; do
    awk -v F="$f" '
    FNR == 1 { in_test = 0; saw_cfg_test = 0; cfgtest_present = 0; curfn = "<toplevel>" }

    # Classify a cfg attribute line carrying a standalone `test` token.
    #   "testonly"   — compiled ONLY under cfg(test): `#[cfg(test)]`, or an
    #                  `all(...)` containing the token with NO `any(` and NO
    #                  `not(` anywhere inside. Conjunction nesting preserves
    #                  test-only-ness (`all(unix, all(test))` still requires
    #                  `test`), so with disjunction and negation excluded,
    #                  any `test` token inside the `all` is a conjunct the
    #                  whole attribute depends on. `any(` breaks that (a gate
    #                  round measured `all(unix, any(test, feature = "x"))`
    #                  classified test-only while compiling into a real
    #                  binary under `--features x`); `not(` can invert it.
    #   "production" — carries the token but still compiles into real
    #                  binaries: `#[cfg(any(...))]` (disjunction) and
    #                  `#[cfg(not(...))]` (negation) at top level.
    #   "unknown"    — anything else, including `all(` forms carrying `any(`
    #                  or `not(`; the scanner must not guess — col-0 unknown
    #                  is an INTEGRITY abort.
    # A substring match here is exactly the defect the first gate round
    # caught: `#[cfg(any(target_os = "linux", test))]` was treated as a test
    # region and a production function silently vanished from the scan.
    function cfg_class(line,  c) {
        c = line
        sub(/^[ \t]*#\[cfg\(/, "", c)
        sub(/\)\][ \t]*$/, "", c)
        if (c == "test") return "testonly"
        if (c ~ /^all\(/ && c !~ /not\(/ && c !~ /any\(/) return "testonly"
        if (c ~ /^any\(/ || c ~ /^not\(/) return "production"
        return "unknown"
    }
    # `test` as a standalone token (not `latest`, not `test_util` — those are
    # different cfgs and none of this scanner s business).
    function has_test_token(line) {
        return (line ~ /[^A-Za-z0-9_]test[^A-Za-z0-9_]/ || line ~ /\(test\)/)
    }

    # Independent channel for the integrity check, evaluated BEFORE any rule
    # that can `next`: does the file contain a TEST-ONLY cfg attribute at any
    # column? If one is present but the anchored col-0 region opener below
    # never fired, the scanner has stopped understanding this codebase and
    # must say so rather than scan on. (Production-class forms — any/not —
    # deliberately do not arm this channel: they open no region.)
    /#\[cfg\(/ && $0 !~ /^[ \t]*\/\// && has_test_token($0) {
        cls = cfg_class($0)
        if (cls == "testonly") cfgtest_present = 1
        if (cls == "unknown" && $0 ~ /^#\[cfg\(/) {
            printf "!!INTEGRITY\t%s\tunrecognized-test-cfg-form:FNR=%d\n", F, FNR
        }
    }

    # A col-0 TEST-ONLY cfg opens a test region. rustfmt keeps every line
    # inside `mod tests { ... }` indented, so the region ends at the next
    # col-0 `}` — which is why the closing brace below is anchored.
    /^#\[cfg\(/ && has_test_token($0) && cfg_class($0) == "testonly" {
        in_test = 1; saw_cfg_test = 1
    }
    in_test == 1 { if ($0 ~ /^\}$/) { in_test = 0 } ; next }

    # Track the enclosing function. Declaration forms only (optional
    # visibility / const / async / unsafe / extern), never a call.
    /^[ \t]*(pub[ \t]+)?(pub\([a-z:]+\)[ \t]+)?(default[ \t]+)?(const[ \t]+)?(async[ \t]+)?(unsafe[ \t]+)?(extern[ \t]+"[A-Za-z-]+"[ \t]+)?fn[ \t]+[A-Za-z_]/ {
        if (match($0, /fn[ \t]+[A-Za-z_][A-Za-z0-9_]*/)) {
            nm = substr($0, RSTART, RLENGTH)
            sub(/^fn[ \t]+/, "", nm)
            curfn = nm
        }
    }

    # Line comments are documentation ABOUT these calls, not calls.
    /^[ \t]*\/\// { next }

    # The write-open surface. `fs::write`/`fs::copy`/`File::create` block on a
    # FIFO directly; the OpenOptions builder flags below are how every other
    # write-open in this tree is spelled. `fs::copy` blocks on its SOURCE.
    /fs::write\(|fs::copy\(|File::create\(|\.write\(true\)|\.append\(true\)|\.create\(true\)|\.create_new\(true\)|\.truncate\(true\)/ {
        printf "%s\t%s\t%d\t%s\n", F, curfn, FNR, $0
    }

    END {
        if (in_test == 1) printf "!!INTEGRITY\t%s\tunclosed-test-region\n", F
        # A file carrying a test attribute that never opened a region means
        # the anchored patterns above stopped matching this codebase.
        if (saw_cfg_test == 0 && cfgtest_present == 1) printf "!!INTEGRITY\t%s\tcfg-test-not-detected\n", F
    }
    ' "$f"
done > "$candidates" || exit 2

# Pass 2: join against the allowlist, both directions.
awk -F '\t' -v AL="$ALLOWLIST" '
BEGIN {
    n_allow = 0
    while ((getline line < AL) > 0) {
        sub(/#.*$/, "", line)
        gsub(/^[ \t]+|[ \t]+$/, "", line)
        if (line == "") continue
        split(line, p, /[ \t]+/)
        if (p[1] == "" || p[2] !~ /^[0-9]+$/) {
            printf "check-write-opens: malformed allowlist entry: %s\n", line > "/dev/stderr"
            bad_allowlist = 1
            continue
        }
        n_allow++
        akey[n_allow] = p[1]
        acount[n_allow] = p[2] + 0
        seen[p[1]] = n_allow
    }
    close(AL)
}

$1 == "!!INTEGRITY" {
    printf "check-write-opens: INTEGRITY %s: %s — test-region detection is unreliable; the gate cannot be trusted\n", $2, $3 > "/dev/stderr"
    integrity = 1
    next
}

{
    key = $1 "::" $2
    observed[key]++
    # Remember one representative site per key for the violation message.
    if (!(key in firstline)) { firstline[key] = $3; firsttext[key] = $4 }
    if (nsites[key] < 8) { nsites[key]++; sitelines[key] = sitelines[key] " " $3 }
    total++
}

END {
    if (bad_allowlist) exit 2
    if (integrity) exit 1

    viol = 0

    # Direction 1: an observed site with no allowlist entry is a new,
    # unreviewed write-open.
    for (key in observed) {
        if (!(key in seen)) {
            txt = firsttext[key]
            gsub(/^[ \t]+/, "", txt)
            printf "check-write-opens: VIOLATION unguarded write-open at %s (line%s)\n", key, sitelines[key] > "/dev/stderr"
            printf "    %s\n", txt > "/dev/stderr"
            printf "    Guard it with agentrec_core::fsguard::is_nonregular on the same path\n" > "/dev/stderr"
            printf "    before the open, then add it to %s with the rationale.\n", AL > "/dev/stderr"
            viol = 1
        }
    }

    # Direction 2: an entry matching nothing, or matching a different number
    # of sites than it was reviewed for.
    for (i = 1; i <= n_allow; i++) {
        k = akey[i]
        if (!(k in observed)) {
            printf "check-write-opens: VIOLATION stale allowlist entry %s matches no call site\n", k > "/dev/stderr"
            printf "    Either the function was renamed/removed (update %s), or the\n", AL > "/dev/stderr"
            printf "    scanner stopped seeing it — which would silently disable this gate.\n" > "/dev/stderr"
            viol = 1
        } else if (observed[k] != acount[i]) {
            printf "check-write-opens: VIOLATION %s has %d write-open line(s), allowlist expects %d (lines%s)\n", k, observed[k], acount[i], sitelines[k] > "/dev/stderr"
            printf "    A write-open was added to or removed from an already-allowlisted\n" > "/dev/stderr"
            printf "    function. Re-verify the guard covers every site, then update the count.\n" > "/dev/stderr"
            viol = 1
        }
    }

    if (viol) exit 1
    printf "check-write-opens: ok — %d production write-open line(s) across %d allowlisted function(s)\n", total, n_allow
}
' "$candidates"
