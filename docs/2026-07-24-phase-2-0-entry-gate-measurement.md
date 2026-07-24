# Phase 2.0 entry-gate measurement + store audit — 2026-07-24

Measured on the real corpus (`~/.claude/projects`, 1860 files / 515 MB) and this
repo's live `.agentrec` store. Scripts: `corpus_gate.py` (v1), `corpus_gate2.py` (v2).

## 1. Import gate: **PASS, 99.1%** (spec floor: 90%)

| denominator | result |
|---|---|
| all 1860 `.jsonl` files | 1264 = 68.0% — **wrong denominator** |
| 1277 top-level sessions | **1266 = 99.1% PASS** |

583 of the 1860 files are `subagents/agent-*.jsonl`. Those are sidechain
transcripts the spec already excludes ("Sidechain (subagent) transcript lines are
skipped, not imported as top-level turns"). They carry no `cwd`, so they cannot
be mapped to a repo — correctly unimportable, and correctly out of the denominator.

**Note for the spec:** sidechains are no longer only inline `isSidechain` lines —
they are now separate files under `subagents/`. The importer must exclude by path,
not only by field. Spec assumes the inline form.

11 genuine top-level failures: 7 no `cwd`, 4 with no prompts and no file-naming
calls (session-metadata-only transcripts). Both classes are honest skips.

Parse robustness: **0 unparseable lines out of 106,311.** 37 MB peak RSS over the
515 MB corpus — the spec's "500 MB under 500 MB RSS" target is met with 13x margin.

## 2. Before-bytes ladder — the spec's 3 tiers are missing a source

| tier | source | entries | share |
|---|---|---|---|
| T1 | transcript `originalFile` or `create` op | 869 | 42.5% |
| **T1.5** | **`~/.claude/file-history/<session>/<hash>@<vN>`** | **518** | **25.3%** |
| T2 | git-tracked (UPPER BOUND, see caveat) | 500 | 24.5% |
| T3 | provenance-only, honest `null` | 157 | 7.7% |

**Reconstructible without touching git: 67.9%.** With the git upper bound: 92.3%.

### T1.5 is a real, undocumented source
`~/.claude/file-history/<sessionId>/<hash>@<version>` holds **verbatim pre-edit
file bytes**, referenced by `file-history-snapshot` records
(`snapshot.trackedFileBackups[path].backupFileName`). 594 backups, 102 sessions,
11 MB, spanning 2026-06-25 → today. Every one of the 508 referenced backups
resolved on disk (100%).

Caveat: **retention-limited to ~30 days / 102 of 1277 sessions.** Treat as
opportunistic like T1, never as a guarantee. It raises the honest reconstructible
floor; it does not change the honesty model.

### T2 is an upper bound, not a measurement
"cwd is a live git repo and the path is tracked" ≠ "the pre-edit bytes at that
turn's timestamp are recoverable". Git holds **committed** states only, so
mid-session intermediate edits (edit → edit → edit → one commit) were never in git
at all. The true T2 rate is below 24.5%; measuring it requires replaying each
turn's timestamp against `git log --until`, which was out of scope here.
**Do not quote 92.3% as the reconstructible rate.** Quote 67.9% + "git recovers
some unknown share of the remaining 24.5%".

## 3. Structural incompleteness — confirmed, slightly worse than spec

Opaque:naming ratio **2.49:1** (spec said ~2:1). 5268 Bash/Task calls vs 2117
Edit/Write. **71% of tool calls can mutate files while naming none.**
`files_complete: false` is mandatory, exactly as specced.

## 4. Store audit — the store is not inefficient, it is recording the wrong files

Live store: 2963 blobs, 789.6 MiB. Size distribution is extremely skewed:
p50 = 14 KiB, mean = 273 KiB, 156 blobs > 1 MiB = 366 MiB.

Top snapshotted paths by count:

```
1282  .remember/logs/memory-2026-07-12.log
1142  .remember/tmp/save-session.pid          <- a PID file, 1142 times
 621  .remember/tmp/last-save-ts
 437  .remember/logs/memory-2026-07-13.log
  64  .code-review-graph/graph.db             <- ~9 MiB SQLite x64 = ~576 MiB
  67  cli/tests/integration.rs                <- actual source code
```

Attribution of referenced store bytes:

| source | blobs | bytes |
|---|---|---|
| `.remember/` + `.code-review-graph/` | 2228 | **764.6 MiB** |
| everything else (real repo content) | 397 | **14.2 MiB** |

**98.2% of the referenced store is churn from two directories that are gitignored.**

### Root cause: self-ignoring `.gitignore` (live bug)

`IgnoreSet::build` (`cli/src/daemon.rs:466-491`) collects `.gitignore` files using
`ignore::WalkBuilder`, which **respects gitignore rules while walking**. Both
offending directories contain a `.gitignore` whose sole rule is `*`:

```
.remember/.gitignore          -> "*"
.code-review-graph/.gitignore -> "# ...comment...\n*"
```

`*` matches `.gitignore` itself. Verified with git:

```
$ git check-ignore -v .remember/.gitignore
.remember/.gitignore:1:*    .remember/.gitignore
```

So the walk never yields those `.gitignore` files → `build` never constructs a
matcher for those directories → `is_ignored` has no rule for anything beneath them
→ every file under them is watched, snapshotted, and CAS-stored forever.

**Verified by executable repro**, not by reading. A standalone probe copies
`IgnoreSet` verbatim and rebuilds the real directory shape:

```
matchers built for:  normal, <root>          <- .remember absent
  .remember/logs/memory.log   ignored=false expected=true  <<< LEAK
  .remember/tmp/save.pid      ignored=false expected=true  <<< LEAK
  .remember/now.md            ignored=false expected=true  <<< LEAK
  normal/a.log                ignored=true  expected=true  ok   (control:
                                     .gitignore = "*.log", does not self-match)
  src.rs                      ignored=false expected=false ok
```

**Fixture gotcha that inverted the result once.** The first probe run REFUTED this
hypothesis — it yielded both `.gitignore` files. Cause: the tempdir was not a git
repo, and `ignore::WalkBuilder::require_git` defaults to true, so no ignore rules
were applied at all and the fixture silently tested nothing. Adding `git init`
flipped it to the LEAK result above. The repo's own comment at `daemon.rs:2295`
already records this trap ("A real `git init` is required so the `.gitignore` is
honored by a gitignore-aware walker"). Any regression test for this MUST `git init`.

Supporting evidence:
1. Both `.gitignore` files are self-ignored (`git check-ignore -v` confirms).
2. 7419 file entries under those paths were snapshotted anyway — most recent
   **2026-07-24T21:41Z, four minutes into this session**, while the ignore files
   date from 07-06 and 07-11. The leak is live, not historical.
3. SPEC.md:51 and IMPLEMENTATION.md D29 both state gitignored paths are excluded
   from watching, so this is a defect against documented behavior, not a design
   choice. (An auto-generated memory observation asserting the opposite —
   "deliberately records gitignored paths — intentional design" — is wrong and
   should not be trusted.)

The generalization is broader than these two dirs: **any directory ignored solely
by its own `.gitignore`, or by a rule that also matches that `.gitignore`, is
invisible to the filter.** Tool-generated caches (`.remember`, `.code-review-graph`,
many others) commonly ship exactly this self-ignoring pattern.

## 5. Efficiency options, in the order they actually pay

1. **Fix the ignore bug.** ~98% of store bytes. Everything below is rounding error
   until this lands. Fix direction: collect `.gitignore` files with a walk that does
   NOT apply ignore rules to `.gitignore` files themselves (keep pruning `.git`/
   `.agentrec`/`node_modules` by name for cost), so a self-ignoring file is still
   read. Needs a RED test: nested dir whose only `.gitignore` is `*`, assert
   contents are filtered.
2. **Per-blob compression.** Measured on 300 real blobs: **19.5x** with gzip -6.
   **Do not act on that number.** It was measured on the churn — SQLite pages and
   repeated log text, which compress absurdly well. After the ignore fix the store
   is ~14 MiB of source code, where 19.5x will not hold. Re-measure on a post-fix
   store before treating this as live. If pursued, blob identity must stay the
   sha256 of *plaintext*, compression purely a storage detail, or every existing
   hash reference breaks.
3. **Never store what cannot be undone.** A 9 MiB SQLite snapshot is not a useful
   undo unit. A size/type policy (binary over N MiB → record hash + `skipped`,
   as the existing over-cap path already does) caps worst-case growth.
4. **Delta/CDC chunking** (restic/borg-style) for the intermediate-snapshot
   problem — consecutive debounced snapshots of one file differ by a few lines.
   This would make `purge --orphans` largely unnecessary rather than necessary.
   Real work; only justified after 1-3, and only with dogfood evidence.

## 5b. Second, independent hole in the same function

`IgnoreSet::build` runs **once at daemon startup** and never refreshes
(`daemon.rs:466`). A `.gitignore` created or edited after the daemon starts is not
honored until restart. Not the cause of this leak (both ignore files predate the
daemon), so it needs its own fix and its own test — but it is the same class of
defect in the same function.

## 5c. Reframing last round's `purge --orphans`

The 2.55 GiB of orphaned blobs reclaimed on 2026-07-17 were almost certainly this
same churn — same root cause, different symptom. That makes `purge --orphans` a
workaround that masked the real bug for a week, not "the durable fix" it was
recorded as. The feature is still correct and worth having (intermediate snapshots
genuinely orphan), but the store will stop needing it at this volume once the
filter is fixed.

## 6. Side finding: memory-dogfood ladder row

- The `agentrec memory` block **injected into a live Claude Code prompt this
  session** — the "real Claude Code session shows injected block" ladder row now
  has its evidence.
- Skill-emitted candidates: `grep -c '"type":"memory-candidate"' .agentrec/signal.jsonl`
  = **0**, and `memory.jsonl` holds 6 records, all `origin: human`. Six days into
  the dogfood with the SKILL installed, **no agent has ever emitted a candidate**.
  The 1-week row cannot close on elapsed time.
