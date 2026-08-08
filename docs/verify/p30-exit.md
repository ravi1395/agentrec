# Phase 3.0 plan-exit evidence

- **Date:** 2026-08-08
- **Branch:** `feat/phase-3-0`
- **HEAD at measurement:** `6db6bfa`
- **Normative source:** `docs/superpowers/plans/2026-08-07-phase-3-0-read-wedge.md`
  § "Plan exit" items 1–6 (item 7, the Fable skeptic gate, is a separate round and
  is NOT covered here) + § "Manual E2E script (tail)".
- **Baselines:** `docs/verify/p30-rework-spike.md` § Baselines — plan-start commit
  `e1efbe5`; `rg -c 'load_log' cli/src` total 36; suite 1035/0/4; T0 decision
  "rework stays headline".

Outputs below are quoted verbatim except where a line says the output was trimmed.
Long outputs (stats: 4276 lines; annotate: 16 lines but very long lines) are reduced
to their discriminating lines, and every trim is stated at the point it happens.

---

## Item 1 — full suite

```
cargo test --workspace --no-fail-fast -- --test-threads=3 > suite.txt 2>&1; echo "SUITE_EXIT=$?"
SUITE_EXIT=0
```

Tally (summed across every `test result:` line — never `head`/`tail`, which
poisons exit codes through SIGPIPE):

```
$ grep '^test result:' suite.txt | awk '{p+=$4; f+=$6; i+=$8} END {printf "PASSED=%d FAILED=%d IGNORED=%d binaries=%d\n", p,f,i,NR}'
PASSED=1150 FAILED=0 IGNORED=4 binaries=23

$ grep '^test result:' suite.txt | grep -v '^test result: ok'      # (no output)
$ grep -n '^failures:' -A 8 suite.txt                              # (no output)
```

**Result: 1150 passed / 0 failed / 4 ignored, over 23 test binaries.**

- Baseline was 1035/0/4 over **19** binaries. The binary count rose 19 → 23; the
  sub-phase added test binaries (stats/search/annotate/bisect), so the delta is
  +115 tests and +4 binaries, zero regression. The changed binary count is stated
  explicitly because it changes what the +115 delta means.
- **Flake disposition: no rerun was needed.** The two known runner-coupled flakes
  (`approve.rs` kill-9, `integration::daemon_counts_ignore_rebuilds`) both passed
  in this single foreground run — `FAILED=0` and no `failures:` block exists in
  the log. Nothing was rerun in isolation, because nothing failed.

## Item 2 — clippy (debug + release) and fmt

```
$ cargo clippy --workspace --all-features --all-targets -- -D warnings
CLIPPY_DEBUG_EXIT=0
$ cargo clippy --release --workspace --all-features --all-targets -- -D warnings
CLIPPY_RELEASE_EXIT=0
$ cargo fmt --all -- --check
FMT_EXIT=0
```

All three exit 0. **PASS.**

## Item 3 — four verbs against a FROZEN CLONE of the dogfood repo

### Setup and premise

The plan's premise — the live repo has a running writer, so hashing the live
`.agentrec/` false-fails on daemon ticks — was checked, not assumed:

```
$ pgrep -fl 'agentrec record'
39492 /Users/ravichandrasekhar/.local/bin/agentrec record --root /Users/ravichandrasekhar/Projects/agentrec
```

The live daemon (pid 39492) exists and was **not** touched; neither was the live
`.agentrec/`. Every verb below ran against the clone.

Clone:

```
$ time rsync -a --exclude 'target/' /Users/ravichandrasekhar/Projects/agentrec/ <scratch>/p30-frozen/
10.621 total     (wall seconds)
$ du -sh <scratch>/p30-frozen <scratch>/p30-frozen/.agentrec
1.7G  .../p30-frozen
1.5G  .../p30-frozen/.agentrec
$ find .agentrec -type f | wc -l
4495
```

**Deviation from the plan text, stated:** the plan says `cp -R`. This used
`rsync -a` with `target/` excluded. Reason: the live `target/` is 7.7 GB and was
being actively rewritten by the concurrent `cargo test` run, so copying it would
have captured a torn, meaningless snapshot at 5× the size. `.agentrec/`, `.git/`,
and the whole source tree ARE included — the corpus under test is complete. The
consequence is recorded under Caveats (it is load-bearing for 3d).

The clone has no daemon attached (nothing was started against it).

### The hash function, and proof it discriminates

`.agentrec/` is hashed as a **manifest hash**: sorted relative paths each with
their own sha256, then one sha256 over that whole listing. This covers content
changes AND file creation/deletion — a content-only hash cannot see a new file.

```sh
cd "$1"; find .agentrec -type f | LC_ALL=C sort | xargs shasum -a 256 | shasum -a 256 | cut -d' ' -f1
```

Four "byte-identical" verdicts are worthless from a hash that cannot change, so
the function was probed in both directions before any verb ran:

```
baseline      = b879ac1c2bf753b57c2750dce4704328f2a1980bbca130b50c99c0bfc1fa1cab
after-create  = 716f08f2f5301b318d5737a0e9d353576178497c4d89a2ca551eab8b2b7d6552   (touch .agentrec/zz-probe-newfile)
after-rm      = b879ac1c2bf753b57c2750dce4704328f2a1980bbca130b50c99c0bfc1fa1cab   (rm that file — restores)
after-append  = dfd214de9990b3f851e4bac8857415053aeeff615bdf1bcb9ad4204f28a5b27e   (append 1 byte to state.json)
after-restore = b879ac1c2bf753b57c2750dce4704328f2a1980bbca130b50c99c0bfc1fa1cab   (restore from backup)
```

The hash moves on creation and on a single appended byte, and returns to baseline
on exact restore. Whole-manifest hash takes ~4.2 s over 4495 files / 1.5 GB.

**Baseline hash for every run below:**
`b879ac1c2bf753b57c2750dce4704328f2a1980bbca130b50c99c0bfc1fa1cab`

### 3a — `stats`

Three invocations, each hashed before and after.

| invocation | exit | hash before | hash after | identical |
|---|---|---|---|---|
| `stats --since all` | 0 | `b879ac1c…1cab` | `b879ac1c…1cab` | yes |
| `stats` (default 30d) | 0 | `b879ac1c…1cab` | `b879ac1c…1cab` | yes |
| `stats --json` | 0 | `b879ac1c…1cab` | `b879ac1c…1cab` | yes |

Text output is 4276 lines (4262 of them the per-file churn table). Trimmed to the
section headers plus the complete rework block:

```
$ grep -n '^[a-z]' stats.txt
1:stats window: since 2026-07-09T14:49:26.353Z until 2026-08-08T14:49:26.353Z · rework-window 7d
3:turns: total 4509 (rich 3027, bare 1482) · imported 0
7:files (churn_bytes, dangling_refs):
4269:share (raw counts, not percentages — buckets use different units, see docs):
4272:rework (approx_lower):

4272:rework (approx_lower):
4273:  measurable=2581 reworked=1383 rate=0.5358 (approx_lower)
4274:  rate is an approximate lower bound: edits hidden in recording gaps are invisible (undercount); a dropped start signal can strip bracket coverage so agent activity reads as rework (overcount)
4275:  excluded: censored_recent=1938 excluded_imported=0 excluded_unknown_mtime=0
4276:  disclosed (not excluded): gap_overlapped=2581 undo_unevaluable_c=0 unparsed_ended=0
```

Other census lines (from the same run, trimmed of the churn table):

```
turns: total 4509 (rich 3027, bare 1482) · imported 0
  by tool: claude=2872, git=155
  by model: <synthetic>=15, claude-fable-5=385, claude-haiku-4-5-20251001=1253,
            claude-opus-4-8=50, claude-opus-5=247, claude-sonnet-5=88
```

`--json` carries the same figures structurally:

```
top keys: ['window', 'turns', 'files', 'share', 'rework']
"rework": {"measurable": 2581, "reworked": 1383, "rate": 0.5358388221619528,
           "rate_bound": "approx_lower", "censored_recent": 1938,
           "excluded_imported": 0, "gap_overlapped": 2581,
           "excluded_unknown_mtime": 0, "unparsed_ended": 0, "undo_unevaluable_c": 0}
```

Exclusion counts are printed beside every figure, as the verb's own help promises.
`--since all` and the default `--since 30d` produced identical figures — the
corpus is younger than 30 days, so the two windows cover the same records.

### 3b — `search "undo"`

| invocation | exit | hash before | hash after | identical |
|---|---|---|---|---|
| `search "undo"` | 0 | `b879ac1c…1cab` | `b879ac1c…1cab` | yes |
| `search "undo" --json` | 0 | `b879ac1c…1cab` | `b879ac1c…1cab` | yes |

71 lines of text output. First three hits and the tail (trimmed; snippets are
truncated by the renderer itself):

```
t_01KXB0M3ZAACD9KSM80MHEXMFY 2026-07-12T11:15:27.833Z [prompt] lations** (INV1/INV2 on every undo + INV-M1/INV-M2 on every memo
t_01KXB0MAVVMPVZAMWEW2CQXMGH 2026-07-12T11:15:34.867Z [prompt] lations** (INV1/INV2 on every undo + INV-M1/INV-M2 on every memo
t_01KXBAEEHE3GSYRJQPQTHPG752 2026-07-12T14:07:07.820Z [prompt] ture_smoke` (torture.rs:855, "undo ... failed"). My change only
...
t_01KYBGSJ1BBPBAVSN6D6SQ0225 2026-07-25T02:14:01.190Z [prompt] :hex-token])) --- 7be9e4d fix(undo,daemon,readcmds): close six d
dangling_prompt_refs=0
more results available — narrow the pattern, or consume page.next via --json
```

Hits carry ids, timestamps, a match-kind tag (`[prompt]`), and a snippet.
`dangling_prompt_refs=0` on this corpus.

`--json` exposes the cursor:

```
keys ['page', 'dangling_prompt_refs']
items 50
next {"after_id": "t_01KYBGSJ1BBPBAVSN6D6SQ0225", "after_occurrence": 0, "query": "search:pattern=undo;regex=false"}
dangling 0
```

**Paging was NOT demonstrated, and cannot be from the CLI.** `agentrec search
--help` lists no cursor argument, and `rg -n 'cursor' cli/src/main.rs` returns
nothing at all — no CLI verb accepts a cursor. There is also no MCP search tool
(`rg 'agentrec_search' cli/src` → no hits). The cursor is emitted for
programmatic consumers of `SearchPage` only, which is exactly what the verb's own
last line tells the user ("consume page.next via --json"). What is proven here is
that a well-formed, query-bound cursor IS produced and that a second page exists;
what is not proven is a second page being fetched. See Caveats.

### 3c — `annotate HEAD~5..HEAD`

| invocation | exit | hash before | hash after | identical |
|---|---|---|---|---|
| `annotate 'HEAD~5..HEAD'` | 0 | `b879ac1c…1cab` | `b879ac1c…1cab` | yes |

16 lines of output; each attribution line is very long, so lines are truncated to
180 chars below (truncation is this document's, not the tool's):

```
 1: annotate HEAD~5..HEAD
 2: line attribution is best-effort: turns record file-level snapshots, not per-line provenance — renames, reformats, and interleaved human+agent edits within one file can misattribu…
 3: .claims/claims.jsonl
 4:   1-2368 63e45da5… t_01KYB8PEF4GHDF0QCZV8KF572K (claude/claude-opus-5, 2026-07-25T00:01:39.511Z) "Please run phases on Phase 2 please, based off hand…
 5:   2369-2380 2c293f0a… t_01KZGRHE3Z165QBYWZ6NDKJ828 (unknown-tool/unknown-model, 2026-08-08T13:21:02.853Z) + t_01KZGRK0507ZRSCDYB7D5M4040 (unknown-too…
 6:   2381-2395 d6cfd2da… t_01KZGTD2T2JXN6C0CR80WX2VNV (claude/unknown-model, 2026-08-08T13:53:51.472Z) "Output raw markdown text only (no tool calls, no…
 7:   2396-2397 db1475a5… t_01KZGW3B0RFB1NSEVX3YR65VYZ (unknown-tool/unknown-model, 2026-08-08T14:23:15.054Z) + t_01KZGW3C3ZHTZWY550D1TDWDBK (claude/clau…
 8: CLAUDE.md
 9:   1-16 63e45da5… t_01KYBHRERZ8Q1F1YG267AK9X8E (claude/claude-opus-5, 2026-07-25T02:31:51.229Z) "Fix the blame over-attribution now, Noise folding bef…
10:   17-59 6db6bfa1… t_01KYBHRERZ8Q1F1YG267AK9X8E (claude/claude-opus-5, …)
11:   60-944 63e45da5… t_01KYBHRERZ8Q1F1YG267AK9X8E (claude/claude-opus-5, …)
12: cli/src/bisectcmd.rs
13:   1-308 63e45da5… t_01KZGMQ8KT6ERG2MTJHDQKN5B7 (git/unknown-model, 2026-08-08T12:14:19.429Z) + t_01KZGN2FV7SJMBJT004ZSBCTVJ (unknown-tool/unknown-mod…
14:   309-313 a7a5f9ae… t_01KZGN2FV7SJMBJT004ZSBCTVJ (unknown-tool/unknown-model, …) + t_01KZGPH0G0QH6TCJSFR8C9XTKM (unknown-tool/…
15:   314-587 63e45da5… t_01KZGN2FV7SJMBJT004ZSBCTVJ (unknown-tool/unknown-model, …) + t_01KZGN2XB2X56097PMKQSAZBPZ (unknown-tool/…
16: evicted_ranges=0
```

It renders on the real dogfood corpus, over three files, joining git blame commit
hashes to turn ids with tool/model/timestamp/prompt-snippet. **The best-effort
disclaimer is present**, on line 2, before any attribution
(`grep -i 'best-effort'` matches line 2). `evicted_ranges=0`.

This is the live-dogfood `annotate` evidence the handoff asked to be folded in.

### 3d — `bisect`

```
$ cd <clone> && time ./agentrec-rel bisect --test 'true' --keep > bisect-true.txt 2>&1
814.98s user 43.89s system 99% cpu 14:22.07 total
bisect exit=143            # 143 = SIGTERM: I killed it at 14:22, it did not exit on its own
$ wc -l bisect-true.txt
       0
```

| invocation | exit | hash before | hash after | identical |
|---|---|---|---|---|
| `bisect --test 'true' --keep` (killed at 14:22) | 143 (SIGTERM) | `b879ac1c…1cab` | `b879ac1c…1cab` | yes |

The zero-write property held even for a run terminated mid-flight: the clone's
`.agentrec/` manifest hash is unchanged after a `kill`, which is a stronger
read-only result than a clean exit would have given.

Three separate claims, kept apart deliberately:

**(i) E2E item 4 is NOT satisfied.** There is no hand-known first-bad turn here.
With `--test 'true'` every probe is good by construction, so there is no first-bad
for bisect to name. Seeding a real known-bad turn on the dogfood clone was not
attempted once (ii)/(iii) below showed a single probe costs ≥14 minutes; a seeded
run would need ~log2(4509) ≈ 12 of them.

**(ii) `--keep` inspectability is NOT demonstrated.** No probe scratch directory
ever materialized:

```
$ find $TMPDIR -maxdepth 1 -type d -name '*bisect*'     # (no output, twice)
$ ls -dl $TMPDIR/*bisect*
-rw-r--r--  ... Aug  8 13:47 .../agentrec-bisect-flaky-63002.count
-rw-r--r--  ... Aug  8 14:06 .../agentrec-bisect-reprobe-15805.count
-rw-r--r--  ... Aug  8 13:54 .../agentrec-bisect-reprobe-79570.count
```

Those three are **regular files, not directories, and are leftovers from earlier
test runs** (13:47–14:06) — this bisect run started at 15:51:13. So there was
nothing to inspect and nothing to `rm`. The task asked for `--keep` scratch-dir
existence to be recorded; the honest record is that the run never reached the
point of creating one.

**(iii) New finding — `bisect` did not complete its first probe on the real
corpus in 14:22 of ~100%-CPU work.** Found by profiling, not inferred. Two
`sample(1)` captures ~14 minutes apart give the same stable call graph:

```
$ sample 16234 3
  agentrec::bisectcmd::bisect
    RepositoryView::bisect_probe_state                       2516 / 2519 samples
      agentrec_core::bisect::probe_state
        BlobStore::get                                       2387
          sha2::sha256::compress256                          2386   (~95%)
```

Process provenance: pid 16234, `lstart` Sat Aug 8 15:51:13 2026, sampled at
16:05:05 (ELAPSED 13:52 / CPU 13:49) and again at 16:05:25; the shell's own
`time` gives the authoritative total, 14:22.07 wall against 858.9 s of CPU.

Mechanism: `BlobStore::get` (`agentrec-core/src/store.rs::get`) verifies **every**
blob it reads — `if hash_bytes(&bytes) != hash { return Err(Corrupt) }` — with no
opt-out. Probe-state materialization pulls blobs through that path, so the sha256
cost is structural to the CAS rather than something bisect opts into: every probe
pays full re-hashing of everything it restores from a 1.5 GB store. This is a
real-corpus performance surface, recorded as a residual, not fixed here.

**Per-probe cost residual, and why the measurement is a lower bound:** the clone
excludes `target/` (see setup), so this ≥14-minute figure is for a **1.7 GB**
tree. A user bisecting a repo with build output present carries the live repo's
additional **7.7 GB** of `target/`. The exclusion makes the recorded cost
conservative, not optimistic.

## Item 4 — T0 "rework stays headline" decision honored

The T0 criterion is quoted from `docs/verify/p30-rework-spike.md` § Findings:

> **T0 exit decision: rework stays headline** — 2420 measurable ≥ 20.
> Dogfood rate 55.2% (approximate lower bound, window 7d).

The criterion is `measurable ≥ 20`. Measured this round from 3a: **measurable =
2581 ≥ 20**, rate 0.5358 (approx_lower), against the spike's 2420 / 55.2% — same
shape, larger corpus, hours apart. **Honored: rework ships as a headline metric,
not demoted.** It is present in both the text report and `--json`, carrying its
`approx_lower` bound label and its full exclusion/disclosure counts.

Rendering observation, stated as such and not as a failed criterion: in the text
report `rework` is the **last** section (line 4272 of 4276), after a 4262-line
per-file churn table. "Headline" in the T0 decision means the metric survives as a
shipped headline figure, not that it renders first.

## Item 5 — no new `load_log` callers in `cli/src`

```
$ rg -c 'load_log' cli/src
cli/src/importcmd.rs:5
cli/src/purgecmd.rs:13
cli/src/readcmds.rs:2
cli/src/daemon.rs:5
cli/src/cmds.rs:10
cli/src/memorycmds.rs:1
$ rg -c 'load_log' cli/src | awk -F: '{s+=$2} END{print s}'
36
```

| file | baseline (`e1efbe5`) | HEAD (`6db6bfa`) | match |
|---|---|---|---|
| cmds.rs | 10 | 10 | yes |
| daemon.rs | 5 | 5 | yes |
| importcmd.rs | 5 | 5 | yes |
| memorycmds.rs | 1 | 1 | yes |
| purgecmd.rs | 13 | 13 | yes |
| readcmds.rs | 2 | 2 | yes |
| **total** | **36** | **36** | **yes** |

Every per-file count matches, not merely the total — a caller migrating between
files would hold the sum constant, so the per-file table is what actually proves
it. **PASS.**

## Item 6 — PROTOCOL.md untouched this sub-phase

```
$ git diff --stat e1efbe5..HEAD -- PROTOCOL.md
                                      (no output — empty diff)
```

**PASS.** `PROTOCOL.md` is byte-identical between the plan-start commit and HEAD.

---

## Caveats — everything weaker than the plan text demanded

1. **E2E item 4 (bisect names a known bad turn) is UNSATISFIED.** No first-bad
   turn was seeded and none was named. What 3d demonstrates is that the
   real-corpus probe path is entered and that bisect is zero-write even when
   killed mid-probe. It does not demonstrate bisection. See 3d(i).
2. **`--keep` scratch-dir existence and inspectability are UNDEMONSTRATED** — no
   probe directory was ever created, so none was inspected or removed. See 3d(ii).
3. **The frozen clone excludes `target/`**, against the plan's literal `cp -R`.
   Reason: 7.7 GB, and it was being rewritten by the concurrent test run.
   `.agentrec/`, `.git/` and all sources are present. Consequence: 3d's probe-cost
   figure is a lower bound for a user whose tree carries build output.
4. **E2E item 2's "page through with cursor" is not reachable from the CLI.** No
   agentrec verb accepts a cursor argument (`rg -n 'cursor' cli/src/main.rs` →
   no hits) and there is no MCP search tool. A valid, query-bound cursor IS
   emitted by `search --json` and the verb tells the user to consume it
   programmatically — so this is a plan-text/product-surface mismatch, not a verb
   defect, but a second page was not fetched and is not claimed.
5. **Test-binary count changed, 19 → 23**, so the suite delta (+115) spans a
   different set of binaries than the baseline. Stated rather than smoothed over.
6. **`stats --since all` and `--since 30d` are not independent evidence** — the
   corpus is younger than 30 days, so both windows cover the same records and
   returned identical figures.
7. **Single-machine, single-run evidence** (macOS, this laptop). No CI leg, no
   Linux leg, no repetition. The suite figure in particular is one run: the two
   known runner-coupled flakes passing here is one observation, not a disposition.
8. **Item 7 (Fable skeptic gate) is out of scope for this document** and has not
   been run.

## Live repo and cleanup

- The live daemon (pid 39492) was left running and untouched; the live
  `.agentrec/` was never read-modified by any verb here (all verbs ran with cwd
  inside the clone).
- The frozen clone and the copied release binary live under the session scratch
  directory and were removed after this evidence was captured.
</content>
