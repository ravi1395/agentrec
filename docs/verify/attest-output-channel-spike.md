# Attest Phase 1 Probe B — output channel validation

Probe A (coverage economics) findings live in `docs/verify/attest-coverage-spike.md`.

## Scope

This document validates — not re-derives — the founder-ratified single parser
mechanism (spec decision 2, ratified 2026-08-31; see
`docs/superpowers/plans/2026-08-28-attest-plan.md` Phase 1 "Probe B") against
real captured `cargo test` output from this repo's own suite. Target used:
`cli/tests/import_claude.rs` (38 `#[test]` fns — a real cross-target name
collision with `cli/tests/import_codex.rs` is documented in the plan; that
collision is exactly why every fixture here is target-scoped, never a bare
unscoped `--exact`). Toolchain: `cargo 1.97.1` (no rustup override active),
darwin. Working tree: no `.rs`/`Cargo`/`IMPLEMENTATION.md` files touched;
fixtures and this doc are the only additions, on branch `feat/attest`.

Fixtures: `docs/fixtures/attest/` (commands + exit codes in that dir's
`README.md`).

## Parser source (throwaway, scratchpad-only, not committed)

Path during this spike:
`/private/tmp/claude-501/-Users-ravichandrasekhar-Projects-agentrec/a4344423-6308-452d-bf5b-21c7acda01bf/scratchpad/parse_libtest.py`

```python
#!/usr/bin/env python3
"""THROWAWAY validation script for the attest Phase 1 Probe B spike.
Not committed to the repo. Implements the founder-ratified parser mechanism:
  - per-test lines: 'test <name> ... ok|FAILED|ignored'
  - summary line:   'test result: ok|FAILED. N passed; M failed; K ignored; ... filtered out'
  - cross-check per-test tallies == summary counts
  - on mismatch or missing summary -> parse_failed: true, raw blob retained
  - derive a staged-pipeline state per the plan's single-test verify pipeline
"""
import re
import sys
import json

PER_TEST_RE = re.compile(r'^test (\S+) \.\.\. (ok|FAILED|ignored)\s*$')
SUMMARY_RE = re.compile(
    r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; '
    r'(\d+) measured; (\d+) filtered out; finished in ([\d.]+)s$'
)


def parse(raw: str) -> dict:
    lines = raw.splitlines()
    per_test = []
    summary = None
    for line in lines:
        m = PER_TEST_RE.match(line)
        if m:
            per_test.append({"name": m.group(1), "status": m.group(2)})
            continue
        m = SUMMARY_RE.match(line)
        if m:
            summary = {
                "result": m.group(1),
                "passed": int(m.group(2)),
                "failed": int(m.group(3)),
                "ignored": int(m.group(4)),
                "measured": int(m.group(5)),
                "filtered_out": int(m.group(6)),
                "finished_in_s": float(m.group(7)),
            }

    result = {
        "per_test": per_test,
        "summary": summary,
        "parse_failed": False,
        "mismatch_detail": None,
        "raw_kept": None,
    }

    if summary is None:
        result["parse_failed"] = True
        result["mismatch_detail"] = "no summary line found (harness crash / process::exit / SIGABRT)"
        result["raw_kept"] = raw
        result["state"] = "recipe-invalid (cause: harness)"
        return result

    tally = {"ok": 0, "FAILED": 0, "ignored": 0}
    for t in per_test:
        tally[t["status"]] += 1

    mismatch = (
        tally["ok"] != summary["passed"]
        or tally["FAILED"] != summary["failed"]
        or tally["ignored"] != summary["ignored"]
    )
    if mismatch:
        result["parse_failed"] = True
        result["mismatch_detail"] = (
            f"per-test tally {tally} != summary "
            f"{{'passed': {summary['passed']}, 'failed': {summary['failed']}, 'ignored': {summary['ignored']}}}"
        )
        result["raw_kept"] = raw
        result["state"] = "parse_failed"
        return result

    # Derive staged-pipeline state for single-test (--exact) fixtures only.
    total = summary["passed"] + summary["failed"] + summary["ignored"]
    if total == 0:
        result["state"] = "recipe-invalid (cause: missing)"
    elif summary["ignored"] == 1 and total == 1:
        result["state"] = "recipe-invalid (cause: ignored)"
    elif summary["passed"] == 1 and total == 1:
        result["state"] = "confirmed-candidate"
    elif summary["failed"] == 1 and total == 1:
        result["state"] = "claim-false-candidate"
    else:
        # whole-target run: not a single-test verdict, just evidence capture
        result["state"] = "bulk-evidence (whole-target run, per-test lines retained)"

    return result


def main():
    path = sys.argv[1]
    with open(path) as f:
        raw = f.read()
    result = parse(raw)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
```

## Results per fixture

### `whole-target.txt` — real run, `cargo test -p agentrec --test import_claude`, exit 0

- Per-test lines parsed: **37** (36 `ok` + 1 `ignored`).
- Summary line: `passed: 36, failed: 0, ignored: 1, measured: 0, filtered_out: 0, finished_in_s: 1.31`.
- Per-test tally `{ok: 36, FAILED: 0, ignored: 1}` **matches** the summary
  counts exactly.
- `parse_failed: false`.
- Derived state: `bulk-evidence (whole-target run, per-test lines retained)`
  — this is the Phase 3 bulk-run shape (many tests, one invocation); it does
  not resolve to a single-claim verdict, matching the plan's "bulk path
  writes EVIDENCE only, never verdicts".
- Note: 38 `#[test]` fns exist in the source (`grep -c '#\[test\]'`) but only
  37 appear in libtest's own enumeration/summary — confirmed cause:
  `cli/tests/import_claude.rs:1576` has `#[cfg(not(debug_assertions))]` on
  one test, so it compiles out of this debug build. The parser correctly
  counts what libtest itself reports (37), not the source-level grep count,
  and neither number is silently substituted for the other.

### `single-exact.txt` — real run, target-scoped `--exact ac3_zero_bytes_under_agentrec_in_tempdir`, exit 0

- Per-test lines: 1 (`ok`). Summary: `passed: 1, failed: 0, ignored: 0,
  filtered_out: 36`.
- Tally matches. `parse_failed: false`.
- Derived state: **`confirmed-candidate`** (total=1, passed=1) — matches the
  plan's staged pipeline step 3 exactly.

### `single-exact-ignored.txt` — real run, target-scoped `--exact
persist::ac5b_oracle_real_corpus_measurement` (no `--ignored` flag), exit 0

- Per-test lines: 1 (`ignored`). Summary: `passed: 0, failed: 0, ignored: 1,
  filtered_out: 36`.
- Tally matches. `parse_failed: false`.
- Derived state: **`recipe-invalid (cause: ignored)`** — matches the plan's
  "never confirmed-by-skip" rule.

### `single-exact-missing.txt` — real run, target-scoped `--exact
this_test_does_not_exist_xyz`, exit 0

- Per-test lines: 0 (`running 0 tests`). Summary: `passed: 0, failed: 0,
  ignored: 0, filtered_out: 37`.
- Tally matches (0 == 0). `parse_failed: false`.
- Derived state: **`recipe-invalid (cause: missing)`** — this is exactly
  the state the plan says exit-code-alone would mis-CONFIRM (exit is 0
  here, same as a real pass); the parser distinguishes it correctly by
  reading `total == 0`, not the exit code.

### `single-exact-failed.handcrafted.txt` — hand-crafted, NOT executed (see
label in the file itself: no test in this suite currently fails, and repo
source may not be modified to manufacture one for this spike)

- Per-test lines: 1 (`FAILED`). Summary: `passed: 0, failed: 1, ignored: 0,
  filtered_out: 36`.
- Tally matches. `parse_failed: false`.
- Derived state: **`claim-false-candidate`** — matches the plan's staged
  pipeline step 3. Shape (panic line, `note: run with RUST_BACKTRACE=1`,
  `failures:` list + `failures:` section, `error: test failed, to rerun
  pass ...`) is reproduced from memory of prior real cargo-test failure
  output in this repo, not observed in this session — flagged as such in
  the fixture file and here.

### `corrupted.txt` — synthetic corruption of `whole-target.txt`

Transform applied (real fixture as input, deterministic in-place edit): one
`ok` line changed to a nonsense status (`... CORRUPTED_STATUS`), and the
summary line truncated to `test result: ok. 36 pass` (no trailing counts).

- The mangled per-test line does not match `PER_TEST_RE` (status isn't
  `ok|FAILED|ignored`), so it silently drops out of `per_test` — 36 lines
  parsed instead of 37.
- The truncated summary line does not match `SUMMARY_RE` at all → `summary:
  None`.
- Because `summary is None`, the parser takes the **first** fail-closed
  branch (missing-summary), independent of the per-test mismatch:
  `parse_failed: true`, `mismatch_detail: "no summary line found (harness
  crash / process::exit / SIGABRT)"`, `raw_kept` = the full raw blob
  (verified non-null), `state: "recipe-invalid (cause: harness)"`.
- This demonstrates the fail-closed path holds even though the corruption
  in this fixture happened to hit the summary line rather than only a
  per-test line — the mismatch branch (tally != summary) is the second
  fail-closed gate, exercised whenever a summary line parses but disagrees
  with the per-test count; both branches set `parse_failed: true` and
  retain the raw blob, never silently drop data.

## Result

**Exit criterion 4 holds.** The founder-ratified parser, run unmodified
against six real (or explicitly labeled hand-crafted/synthetic) libtest
output captures from this repo's own suite:

- Correctly parses per-test lines and the summary line on both the
  whole-target run and the single-`--exact` run, with per-test tallies
  matching the summary line's counts exactly in every non-corrupted
  fixture (`whole-target.txt`, `single-exact.txt`,
  `single-exact-ignored.txt`, `single-exact-missing.txt`,
  `single-exact-failed.handcrafted.txt`).
- Correctly distinguishes all five staged-pipeline states named in the
  plan: `confirmed-candidate` (1 passed), `claim-false-candidate` (1
  failed), `recipe-invalid (cause: ignored)` (1 ignored), `recipe-invalid
  (cause: missing)` (0/0/0, exit 0 — the state exit-code-alone would
  mis-CONFIRM), and `bulk-evidence` for a whole-target/many-test run.
- On the deliberately corrupted fixture, fails closed: `parse_failed:
  true`, raw blob retained, never silently dropped or misreported as a
  passing state.

## Quirks encountered

- **`running N tests` header and blank lines are ignored by design** — the
  parser only matches the two line shapes it needs; no attempt to validate
  the header count against the per-test tally (a possible extra check, not
  implemented here since it wasn't required by exit criterion 4).
- **`--test-threads` / output ordering**: not exercised directly (default
  thread count used throughout), but the regex-per-line approach is
  order-independent — interleaved per-test lines from parallel test threads
  would still each match `PER_TEST_RE` on their own line; only truly
  interleaved *partial* lines (two threads' stdout writes racing mid-line)
  would break it, which is a real theoretical gap not probed here (this
  repo's own recorded lesson `cargo-multi-test-filters` / release-suite
  notes elsewhere flag `--test-threads=3` for flake avoidance in full-suite
  runs, not for single-target/single-test captures like these fixtures).
- **No doc-test target was probed.** All fixtures are from a `--test`
  integration binary; `cargo test --doc` output has a different per-test
  line prefix in some versions and was out of scope for this spike (the
  plan's target list is `--test <name> / --lib / --bin agentrec`).
- **`single-exact-ignored.txt` required NOT passing `--ignored`.** A first
  attempt at capturing this fixture used `--exact ... --ignored`, which
  actually *runs* the ignored test (real corpus permitting) and reports it
  as `ok`, not `ignored` — the wrong state for this fixture's purpose. Fixed
  by omitting `--ignored`, which reproduces libtest's own default
  skip-and-report-`ignored` behavior. Recorded here since it's a plausible
  trap for anyone re-deriving these fixtures.
- **38 `#[test]` (source grep) vs 37 (libtest's own summary)**: not
  investigated further — noted above under `whole-target.txt` — but worth
  flagging that the parser trusts libtest's own count, never a source-level
  grep, which is the correct trust boundary for this mechanism.
