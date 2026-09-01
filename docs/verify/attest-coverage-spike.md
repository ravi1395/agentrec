Probe B (output channel) findings live in `docs/verify/attest-output-channel-spike.md`

# attest Phase 1 — Probe A: coverage economics (findings)

Plan: `docs/superpowers/plans/2026-08-28-attest-plan.md` § "Phase 1".
Spec decision 5 (coverage-based scope, suite-level fallback sanctioned):
`docs/superpowers/specs/2026-08-28-attest-design.md`.

Everything below was measured in one session on 2026-09-01, on branch
`feat/attest`, on one machine (Apple Silicon macOS, Homebrew `rustc 1.97.1`).
Figures that are extrapolated are labeled **EXTRAPOLATION** inline. Nothing
here is estimated except where labeled.

Throwaway scripts (not part of the repo) live in the session scratchpad:
`sample.py` (deterministic stratified sampler, seed 20260901) and `pertest.sh`
(per-test capture harness).

---

## 0. Baseline (uninstrumented) — the figure for the first commit message

```
$ /usr/bin/time -p cargo test --workspace -- --test-threads=3
...
$ grep -E '^test result' baseline2.txt | awk '{p+=$4;f+=$6;i+=$8} END{print "passed="p" failed="f" ignored="i}'
passed=1035 failed=0 ignored=4

real 207.84
user 9.91
sys 9.73
```

**Baseline: 1035 passed / 0 failed / 4 ignored, 207.84 s wall.**

Contention caveat, recorded because it is unrecoverable later: an earlier
identical run of mine overlapped with a sibling agent running the same command
in the same tree. It produced the *same* counts and 208.82 s wall. The 207.84 s
figure above is the re-run on a quiet machine (verified with
`until ! pgrep -f "cargo test --workspace"; do sleep 10; done` before starting).
The two agree to 0.5%, which is expected: with `--test-threads=3` this suite is
dominated by daemon sleeps and filesystem waits, not CPU.

No test flaked in either run, so no per-binary re-run was needed.

## 1. Toolchain friction (a Phase 1 finding in its own right)

`cargo-llvm-cov 0.9.0` was already installed. The `llvm-tools` rustup component
is installed for the rustup toolchains. The repo's actual compiler is
**Homebrew** `rustc 1.97.1`, and cargo-llvm-cov derives the `llvm-profdata` path
from the *active* sysroot, which for Homebrew rust does not ship one:

```
$ cargo llvm-cov --lib -p agentrec-core --lcov --output-path smoke.lcov
test result: ok. 230 passed; 0 failed; 0 ignored; ...
error: failed to merge profile data: could not execute process
`/opt/homebrew/Cellar/rust/1.97.1/lib/rustlib/aarch64-apple-darwin/bin/llvm-profdata merge -sparse ...`
(never executed): No such file or directory (os error 2)
```

Two workarounds were tried; **both work**:

1. `rustup run stable cargo llvm-cov ...` → exit 0, report written. But this
   switches the compiler to `rustc 1.96.1` (the rustup stable), i.e. it measures
   a different toolchain than the repo builds with.
2. Point cargo-llvm-cov at the rustup component binaries while keeping the
   Homebrew compiler:
   ```
   B=~/.rustup/toolchains/stable-aarch64-apple-darwin/lib/rustlib/aarch64-apple-darwin/bin
   LLVM_COV=$B/llvm-cov LLVM_PROFDATA=$B/llvm-profdata cargo llvm-cov ...
   ```
   → exit 0, report written, `rustc 1.97.1` still the compiler.

**Every measurement in this document used workaround 2.** If attest ships a
coverage step it must set these two variables (or document the requirement),
because the out-of-the-box invocation fails on this machine.

## 2. (a) One full instrumented suite run — suite-level map

```
$ cargo llvm-cov clean --workspace
$ /usr/bin/time -p cargo llvm-cov --workspace --no-report --no-fail-fast -- --test-threads=3
passed=1035 failed=0 ignored=4
real 208.96
user 10.32
sys 9.53

$ find target/llvm-cov-target -name '*.profraw' | wc -l
    2306
$ find target/llvm-cov-target -name '*.profraw' -exec stat -f%z {} + | awk '{s+=$1} END{print s}'
324598424
```

Report export (separate, timed separately):

```
$ /usr/bin/time -p cargo llvm-cov report --lcov --output-path suite.lcov
real 0.64
$ /usr/bin/time -p cargo llvm-cov report --json --output-path suite.json
real 0.41

$ stat -f'%z %N' suite.lcov suite.json target/llvm-cov-target/agentrec.profdata
858635    suite.lcov
6023534   suite.json
743688    agentrec.profdata
```

| suite-level | value |
|---|---|
| wall, instrumented run | 208.96 s (vs 207.84 s uninstrumented — **+0.5%**) |
| profraw files | 2306 |
| profraw bytes (transient) | 324,598,424 (324.6 MB) |
| merged `.profdata` | 743,688 B |
| lcov export | 858,635 B (+0.64 s) |
| llvm-cov JSON export | 6,023,534 B (+0.41 s) |
| distinct source files with coverage | 35 (`grep -c '^SF:' suite.lcov`) |
| file-set projection (35 paths, one set) | ~1.5 KB |

Instrumentation is essentially free in wall time here for the same reason the
contention caveat above is small: this suite waits, it does not compute.
Qualification: machine quietness was verified before the uninstrumented re-run
and was **not** re-verified immediately before the instrumented run, so the
+0.5% delta carries unmeasured contention risk in an unknown direction.

**One aborted run, recorded rather than dropped.** The first `cargo llvm-cov
--workspace --no-report` invocation (immediately after `cargo llvm-cov clean
--workspace`) failed 3 `emitter_turn` tests with
`run agentrec: Os { code: 2, kind: NotFound }` — the `CARGO_BIN_EXE_agentrec`
path did not exist when those tests ran. `target/llvm-cov-target/debug/agentrec`
existed immediately afterwards, and the second run (no clean) had 0 failures.
I did not isolate the cause; it looks like a build-ordering race between the bin
target and the integration test binaries under a cold llvm-cov target dir, but
that is **unconfirmed** — one observation, not a diagnosis. It matters for
Phase 4 only as: a cold-cache coverage run may need the bin built first.

## 3. (b) Per-test profile capture — 50-test sample

Method: the instrumented test binaries built by step (a) were driven
**directly** (no `cargo` in the loop, which would otherwise dominate the
per-test number). Each sampled test ran in its own process with its own
`LLVM_PROFILE_FILE` template and its own output directory:

```
LLVM_PROFILE_FILE="$dir/p-%p-%10m.profraw" $D/$bin --exact "$test" --test-threads=1
llvm-profdata merge -sparse -o $dir/t.profdata $dir/*.profraw
llvm-cov export -format=text -instr-profile=$dir/t.profdata \
    -object $D/$bin -object target/llvm-cov-target/debug/agentrec \
    -sources cli/src agentrec-core/src
```

The `%p` (pid) in the template is what makes child processes write their own
files rather than clobbering the parent's — see §4. Per-test wall includes
**run + merge + export**, not just the run.

**Sampling:** deterministic, seed 20260901, stratified into two strata that
behave completely differently:

- *unit* — the two `--lib`/bin-unit binaries (`agentrec_core` 230 tests,
  `agentrec` bin unit tests 386) = **616** population; 25 sampled uniformly.
- *integration* — the 17 `cli/tests/*` binaries = **423** population; 25 sampled
  as one uniformly-chosen test per target (17) plus 8 more drawn uniformly from
  the remainder, so every integration target including the four spawn call sites
  is represented.

Populations come from each binary's own `--list` (`1039` names total = 1035
passed + 4 ignored, so the enumeration is complete). All 50 exited 0.

Total wall of the whole 50-test harness (including the sampler's own overhead):

```
$ /usr/bin/time -p ./pertest.sh
real 45.77
```

| per test | unit (n=25) | integration (n=25) |
|---|---|---|
| mean total (run+merge+export) | 0.112 s | 1.586 s |
| **median** total | 0.099 s | **0.125 s** |
| max total | 0.23 s | 21.58 s (`torture_smoke`) |
| mean profraw files written | 1.0 | 4.1 |
| mean profraw bytes | 135,164 | 469,753 |
| mean merged `.profdata` | 3,146 B | 14,680 B |
| mean full-region JSON export | 3,465,079 B | 2,066,464 B |
| mean **file-set projection** entry | 215 B | 355 B |

The integration mean is dominated by one test (`torture_smoke`, 21.5 s); the
median is 0.125 s. Both are reported because the extrapolation is sensitive to
which you use.

### EXTRAPOLATION to the full suite (labeled; weighted per stratum)

Weighted sum, mean-based: `616 × 0.112 s + 423 × 1.586 s = 69 s + 671 s =`
**~740 s (~12.3 min)**, sequential, one process per test.

Weighted sum, median-based: `616 × 0.099 + 423 × 0.125 =` **~114 s** — i.e. if
the handful of long-running daemon/torture tests are excluded or run in
parallel, per-test capture is *cheaper* than the suite run. The honest range is
therefore **~2 min to ~12 min** depending on the tail, against a 209 s suite
run. Neither figure was measured as a whole-suite per-test run; both are
extrapolations from the 50-test sample.

Bytes, EXTRAPOLATION: transient profraw `616 × 135 KB + 423 × 470 KB ≈ 282 MB`
(comparable to the suite run's measured 324.6 MB, which is a sanity check on the
extrapolation). Retained artifacts are §4's projection, not the profraw.

## 4. (c) Map sizes per approach

| artifact | suite-level (measured) | per-test (EXTRAPOLATION from sample means) |
|---|---|---|
| transient profraw | 324.6 MB | ~282 MB |
| merged profdata | 744 KB | ~10 MB (1039 × ~10 KB mean) |
| full-region llvm-cov JSON | 6.0 MB | **~3.0 GB** (1039 × ~2.9 MB) — prohibitive |
| **file-set projection** (what Phase 4 consumes) | ~1.5 KB (35 paths, one set) | **~283 KB** (616×215 + 423×355 = 282,605 B) |

Measured anchor for the projection column: the 50 sampled tests' projections
written as one JSON object = **13,519 bytes** on disk for 50 tests.

The decisive number is the last row. Keeping llvm-cov's region-level output
per test is not viable (~3 GB); reducing each test to *its set of covered source
files* costs **~283 KB for the whole suite** — smaller than a single golden
file. Line/region granularity buys nothing for the staleness question Phase 4
asks ("did any file this test executed change?") and costs four orders of
magnitude. See §6.

## 5. (d) THE KILLER QUESTION — does a spawned `agentrec` child's `cli/src` execution land in the test's map?

### **YES — for children that exit normally. NO — for children killed by SIGKILL.**

**How the tests locate the binary.** All four named call sites use
`env!("CARGO_BIN_EXE_agentrec")` (`cli/tests/approve.rs:25`,
`cli/tests/golden.rs:236`, `cli/tests/hardening_daemon.rs:24`, and
`cli/tests/emitter_turn.rs:25`; the plan's `cli/tests/bisect.rs` does not exist
in this tree). That macro resolves at compile time of the test crate, so under
`cargo llvm-cov` it resolves into `target/llvm-cov-target/debug/agentrec` — the
**instrumented** binary. Confirmed present and rebuilt under that dir
(`-rwxr-xr-x 24327288 target/llvm-cov-target/debug/agentrec`).

**Why the child writes its own profile.** `cargo llvm-cov show-env` exports:

```
LLVM_PROFILE_FILE='.../target/agentrec-%p-%10m.profraw'
```

The `%p` (pid) and `%10m` (module signature) expansions mean parent and child
write to *different* paths instead of clobbering. `golden.rs::agentrec` scrubs
every `AGENTREC_*` variable from the child env but leaves `LLVM_PROFILE_FILE`
untouched, so the inheritance survives.

**The linkage premise, measured not assumed.** `cli` has no library target
(`grep -n '\[lib\]' cli/Cargo.toml` → no match; `cli/src/lib.rs` does not
exist), so an integration test binary links none of `cli/src` — the only way
`cli/src/*` can enter one of their maps is a child process. Negative control
over the 50-test sample's own file sets:

```
core lib tests n=9,  with any cli/src: 0
bin unit tests n=16, with any cli/src: 16
integration   n=25,  with any cli/src: 24
integration with 0 cli/src: ['conformance::regenerate_conformance_fixtures']
```

`agentrec_core` lib tests neither link nor spawn `cli/src` and show **zero** —
the signal is not contamination from stale profdata or a `%10m` merge artifact.
The bin-unit tests show 16/16 because that binary *is* `cli/src` compiled as a
test target, which is the expected positive control.

**Direct evidence, three isolated per-test captures:**

```
clean_exit_child   (golden::fixture_contains_every_required_shape)       rc=0 profraw=2
   covered_files=  6  cli_src= 2   daemon.rs covered: False | main.rs: True
sigterm_child      (hardening_daemon::sigterm_triggers_clean_shutdown…)  rc=0 profraw=3
   covered_files= 16  cli_src= 7   daemon.rs covered: True  | main.rs: True
sigkill_child      (emitter_turn::duplicate_start_after_restart…)        rc=0 profraw=3
   covered_files= 16  cli_src= 7   daemon.rs covered: True  | main.rs: True
```

`cli/src/main.rs` (and for the daemon tests `cli/src/daemon.rs`) is covered in a
test binary that contains none of that code. **That is the yes.** Across the 25
sampled integration tests, **24 of 25 showed at least one `cli/src` file
covered** (1–11 files per test); the 25th,
`conformance::regenerate_conformance_fixtures`, is `#[ignore]`d and did not run
(`test result: ok. 0 passed; 0 failed; 1 ignored`).

**The qualification, measured directly.** The `sigkill_child` row above is *not*
evidence that a killed process is captured: its 3 profraw files exceed the 1 a
non-spawning test writes, so at least one child flushed, but which of that
test's children were killed and which exited cleanly was **not determined**. The
standalone probe below is what establishes the SIGKILL behaviour — it spawns the
instrumented binary directly and signals it:

```
SIGTERM: record_profraw=1 bytes=142360
SIGKILL: record_profraw=0 bytes=0
```

A process terminated by SIGKILL never runs its atexit flush, so **no profraw
exists and its execution is invisible to the map**. This repo kills daemons with
`Child::kill()` (SIGKILL on Unix) in many places —
`cli/tests/integration.rs:2536` goes to SIGKILL deliberately, and the RAII
daemon guard at `cli/tests/integration.rs:10111` SIGKILLs on `Drop`. So
daemon-lifecycle tests will have **systematically under-scoped** file sets: the
daemon code the child executed after the last clean exit is missing.

Per spec decision 5 ("when unsure, stale MORE"), under-scoping is the wrong
direction of error, and this is the one real risk the ruling has to price.

**Fixture-file reads: they do not affect the coverage map at all.** A coverage
map records executed *code*, not opened *data*. A test that reads
`docs/fixtures/…` and one that does not produce identical file sets if they run
the same code. Anything attest wants to stale on fixture edits must come from a
different mechanism (e.g. the recorder's own file-touch data), not from
llvm-cov. Stated as a finding because the plan asked; no measurement was needed
to establish it, and none was run.

## 6. Exit criterion 5 — schema sketch for Phase 4's consumer

Per-test identity → covered source files. Target-scoped identity (a bare test
name collides across targets, per the plan's own measured example):

```json
{
  "schema": "attest-coverage/1",
  "unstable": true,
  "captured_at": "2026-09-01T22:31:00Z",
  "commit": "<git rev at capture>",
  "granularity": "file",
  "tests": [
    {
      "target": { "kind": "test", "name": "golden" },
      "test": "fixture_contains_every_required_shape",
      "files": ["cli/src/main.rs", "cli/src/cmds.rs", "agentrec-core/src/view.rs"],
      "child_processes": 1,
      "child_killed": false
    }
  ]
}
```

`child_killed` (or an equivalent `scope_confidence` flag) exists because of §5:
a test whose child was SIGKILLed has a knowingly incomplete file set, and Phase
4 should treat such a test as staled by any `cli/src/**` write rather than
trusting its set. That is the "stale MORE" rule made mechanical.

**Line/region granularity is not worth keeping**, on the predicate first and
size second. The staleness predicate Phase 4 evaluates is "did a file in this
set change"; line ranges would only serve a same-file-different-lines
optimization that spec decision 5 explicitly does not want (it says over-stale
when unsure). Size corroborates: ~4 orders of magnitude larger (§4) — though
that figure comes from this harness exporting *full region data over all
sources* per test, and a tighter per-test export scope would be materially
smaller, so treat it as corroboration, not as the argument. Recommend file
granularity, with the raw `.profdata` discarded after projection.

## 7. Exit criterion 3 — RULING RECOMMENDATION (awaiting founder sign-off)

**Recommendation, awaiting founder sign-off — the founder rules, not the spike
author. Phase 2 dispatch is gated on this sign-off (plan exit criterion 7).**

**Recommend per-test coverage, at file granularity, with a declared
under-scope rule for SIGKILLed children.**

Grounded in the numbers above:

1. **Cost is not the obstacle.** Instrumentation costs +0.5% on the suite
   (208.96 s vs 207.84 s). Per-test capture extrapolates to ~2–12 min for the
   full suite depending on the long tail, against a 209 s suite run — a
   1×–3.5× multiplier on a run that already takes 3.5 minutes, and it is not on
   any interactive path (Phase 4 captures at explicit moments, not per keystroke).
2. **Storage is not the obstacle, once projected.** ~283 KB for a full per-test
   file-set map (EXTRAPOLATION; measured 13,519 B for 50 tests). Keeping
   llvm-cov's native per-test region output would be ~3 GB and is rejected.
3. **The quality question — the plan's named "likely killer" — came back YES.**
   Child-process `cli/src` execution *is* attributed to the spawning test. The
   fallback ruling ("unit-test claims only, integration claims stale on any src
   write") is therefore **not** required, and would throw away real measured
   scope for 423 integration tests.
4. **The one real defect is SIGKILLed children**, and it fails in the dangerous
   direction (under-scope). It is containable without abandoning per-test
   scope: mark tests whose spawned children were killed, and stale them on any
   write under `cli/src/**`. That is strictly more scope than the suite-level
   fallback gives, and strictly more honest than pretending their file set is
   complete.
5. **Suite-level remains the cheap degraded mode** and should stay implemented
   as the fallback path (spec decision 5 sanctions it): 209 s, 744 KB profdata,
   one 35-file set. It is what a `--fast` or CI-constrained capture should use.

If the founder prefers to avoid the SIGKILL complexity entirely, the honest
alternative is not the unit-only fallback but **per-test scope with every
daemon-spawning test permanently staled on `cli/src/**` writes** — same
mechanism, coarser flag, no per-test kill detection needed.

## 8. Not measured / caveats

- **Single machine, single OS.** Apple Silicon macOS; Homebrew `rustc 1.97.1`
  / `cargo 1.97.1`; `cargo-llvm-cov 0.9.0`. No Linux leg. The repo already
  records that its perf/timing evidence is macOS-only; this adds nothing there.
- **Toolchain friction is real** (§1) and was worked around by exporting
  `LLVM_COV`/`LLVM_PROFDATA` from the rustup component. The `rustup run stable`
  route also works but changes the compiler to 1.96.1 — no measurement here used
  it beyond the smoke test.
- **The 50-test sample is 4.8% of the suite** (50/1039), stratified into two
  strata whose behaviour differs by ~14× in mean wall time. The integration
  stratum's mean is dominated by one 21.5 s test; treat the mean-based
  extrapolation as an upper bound and the median-based one as a lower bound.
  No confidence interval is computed and none should be quoted.
- **Per-test capture was run sequentially with `--test-threads=1`.** A
  parallel per-test capture was not measured; profraw templating with `%p`
  suggests it would work, but that is untested.
- **The 3-failure aborted llvm-cov run (§2) was not diagnosed**, only recorded.
- **Attribution of a child to *which* test was not stress-tested under
  parallelism.** Every per-test capture here was one test per process with its
  own profile directory, which is what makes attribution unambiguous. If Phase 4
  ever captures several tests in one process, the `%p` templating alone does not
  tell you which test a child's profraw belongs to — unmeasured, and a real
  design constraint on Phase 4.
- **Exit criterion 4 (Probe B) is not in scope for this document** and is not
  claimed here; see the sibling doc named at the top.
- **An instrumented run leaks `.profraw` into the repo root.** 58 files named
  `default_<hash>_0_<pid>.profraw` appeared at the top of the working tree
  during the instrumented suite run (children that ran without the templated
  `LLVM_PROFILE_FILE` in scope). They were moved to the session scratchpad, not
  deleted. Phase 4 must either set the template explicitly for spawned children
  or gitignore the pattern, or a coverage run dirties the user's repo — which,
  in this repo, also means the recorder sees 58 file creations.
- No `.rs` or Cargo file was modified. `cargo llvm-cov` wrote only under
  `target/llvm-cov-target/` (1.0 GB); nothing was written inside `.agentrec/`.
