# Store-churn reduction: measured findings + deferred designs

**Date:** 2026-07-25 · **Status:** design record, not a plan · **Supersedes:** the informal
"four levers" enumeration from this session's first pass (two of those four are refuted below —
do not resurrect them without reading §4).

Produced by a 4-lens adversarial redteam (read-verb integrity / crash+concurrency / honesty+protocol
/ measurement+alternatives) against the real tree at `e8b597b`. Every number in §2 is measured from
`.agentrec/log.jsonl` and `.agentrec/objects/` on this machine, not estimated.

---

## 1. Problem restated

Two symptoms, **different fixes**, because `log.jsonl` is append-only:

- **(a) disk bytes** in `.agentrec/objects/` — reclaimable by GC.
- **(b) log line count** — a line, once appended, can never be removed without a third sanctioned
  append-only rewrite class. **Only prevention at record time, or suppression at render time,
  reduces it.** No reclaim path can ever help.

This split is the single most load-bearing fact in this document. Every candidate below is ranked
first by which symptom it actually addresses.

---

## 2. Measurements (2026-07-25, this repo)

From `log.jsonl`: 1290 lines, **0 unparseable** (1275 `turn` + 15 `epoch`).

| Quantity | Value |
|---|---|
| FileEntry records | **8346** across 3187 distinct paths |
| files/turn | mean 6.5 · median 3 · p90 9 · p99 72 · max 445 · 63 turns with 0 files |
| grades | bare 887 · rich/claude 307 · rich/git 84 |
| ops | create 3141 · modify 2767 · delete 2438 |
| flags | `baseline_unknown` 73 · `skipped` 45 · `withheld` 0 |
| blob-ref slots → distinct blobs | 10,636 → 2799 = **3.80:1** dedup |
| churn share of distinct blobs | **2451 / 2799 = 87.6%** |
| churn share of `log.jsonl` bytes | **75.0%** (1,382,199 of 1,842,299 B) |
| blobs shared between churn and source paths | **0** |

### 2.1 The mechanism: dedup is similarity-blind

Per-path dedup ratios on the churn class:

```
656  .remember/tmp/save-session.pid        591 distinct blobs   1.11x
642  .remember/logs/memory-2026-07-12.log  636                  1.01x
336  .remember/tmp/last-save-ts            329                  1.02x
219  .remember/logs/memory-2026-07-13.log  218                  1.00x
 68  .code-review-graph/graph.db            23                  2.96x
```

Append-only log files dedup at **1.00–1.02x** — every snapshot is genuinely new content, stored
whole. Cost is **O(N × final_size)**, i.e. quadratic in version count. The CAS never reports
distress because each individual `put` is correct. Dedup measures *identity*, not *similarity*.

### 2.2 Prefix structure confirmed live

`.remember/logs/memory-2026-07-25.log` sampled twice during the session:

```
snap1 107,897 B → snap3 110,720 B    strict prefix: True    appended 2,823 B
delta cost vs full re-store = 2.55%
```

Quadratic cost for the four such files still on disk (`N × final/2` vs `final + 64N`):

```
157 versions  final 603,477 B   naive ~47.37 MB   tailΔ ~0.614 MB
 85 versions  final  73,604 B   naive ~ 3.13 MB   tailΔ ~0.079 MB
 30 versions  final  96,841 B   naive ~ 1.45 MB   tailΔ ~0.099 MB
  9 versions  final 111,285 B   naive ~ 0.50 MB   tailΔ ~0.112 MB
TOTAL                           naive ~52.5 MB    tailΔ ~0.90 MB    ≈58x
```

**Understates** — the two largest churn files (642 and 219 versions) were rotated away before
measurement.

### 2.3 Compressibility (survivor sample — read the caveat)

285 blobs on disk, **263 of them prompt blobs**; 2790 referenced snapshot blobs were already purged.
So this is a prompt-biased sample, **not** a snapshot-size distribution. 285/285 text, 0 binary.

```
per-blob gzip -6 : 1,194,610 / 3,100,008 = 38.5%
concatenated     :   625,182 / 3,100,008 = 20.2%
```

The 18-point gap is cross-blob redundancy — headroom a delta scheme captures and per-blob
compression structurally cannot.

### 2.4 Reconciliations

- **The "9602" figure in CLAUDE.md is blob-ref slots, not file entries.** `.remember` entries =
  7659, slots (non-null `before`+`after`) = **9602**. Exact match. Correct the wording when next
  editing that note.
- **The D29 gitignore fix is holding.** Last churn entry `2026-07-24T23:06Z`; of the 8 turns since
  `2026-07-25`, **0 churn entries**.
- **Unenumerated by every prior note:** `signal.jsonl` = **2,448,003 B in 487 lines (~5 KB/line)**,
  fully consumed (`state.json signal_offset` == file size), never compacted. Larger than
  `log.jsonl` and larger than the entire live object store. Compaction would break byte-offset
  consumption semantics and create a third append-only rewrite class — poor cost/benefit, but it
  should stop being invisible.

---

## 3. Deferred designs (for the next session)

Ordered by measured value. None are started.

### 3.1 Tail-delta blobs — the real disk lever

**Shape.** CAS address stays `sha256(full content)` — unchanged, so every ref in `log.jsonl`,
`open.json` and `memory.jsonl` keeps working. Only the *on-disk representation* becomes either full
bytes or `{parent, tail}` where `parent` is a blob whose content is a strict prefix of this one.

**Measured payoff:** ≈58x on the append-only class (§2.2). ~0 on mid-file source edits — the prefix
test fails and it falls back to full store. Narrow but exactly aimed at the class that produced 87.6%
of distinct blobs.

**Three invariants, all load-bearing:**

1. **A parent MUST itself be a log-referenced hash.** `purge --orphans` finds garbage *by absence*
   from a raw, deliberately non-parsing `sha256:` byte-scan (torn-line safety — see
   `cli/src/purgecmd.rs` SAFETY note). It cannot see a parent hash that lives only inside a child
   blob's on-disk header, and would archive-rename the parent out from under its children.
2. **Retention MUST never evict a parent.** Same reasoning against `retention::enforce_budget`'s
   `store.remove` (which is a hard `fs::remove_file`, no archive).
3. **Bounded chain depth** — rebase to a full blob every K links.

With (1)+(3), the honesty vocabulary stays **binary**: a blob resolves or it doesn't. This is
precisely why tail-delta is acceptable where **FastCDC content-defined chunking is not** — chunking
introduces "chunk 7 of 40 missing", a partial-integrity state agentrec has no word for and whose
introduction would force new vocabulary into `undo`'s refusal ladder and `diff`'s notices.

**Protocol impact: none.** §6 addresses blobs by sha256 of *content*; encoding is implementation-local.
Files: `agentrec-core/src/store.rs`, `retention.rs`, `cli/src/purgecmd.rs`.

### 3.2 First-observation-only `put` + hash-guarded recovery

**Shape.** `Recorder::stage` currently `put`s on every debounced batch — load-bearing for kill-9
recovery, which is why intermediates exist at all. Reduce to O(1 per path per turn): journal the
*last-observation hash* rather than pushing bytes every batch.

**The mandatory guard.** The naive form ("re-read the worktree at turn close") **fabricates
attribution** — after a kill-9 the file may have been changed by someone else, and that edit would
be recorded as the crashed turn's `after`. At recovery, accept the worktree read **only if its hash
matches the journaled hash**; otherwise mark `after` unavailable. The cost of this design is one new
honest unavailable-state, and that cost is not optional.

**Ordering constraint.** If any staging step is introduced, promotion must be strictly ordered
before the journal/log append, or `recover_orphan` emits a turn citing a hash that never reached
`objects/` — and `diff` then prints `"purged or missing"` for a blob that was never purged.

Multiplies with §3.1: for append-only files every intermediate batch is genuinely-new content, which
is exactly where the quadratic came from. Files: `cli/src/daemon.rs` (+1). Protocol: none.

### 3.3 Rate budget for the unknown next churn directory

**Why this and not a better path rule.** The obvious declarative gate — *deny top-level dot-dirs
with no git-tracked files* — tests at 93.6% of entries / 87.4% of blobs suppressed with 0
git-tracked false positives... **retrospectively**. Its incremental value over the already-merged
D29 fix is ~0 on this corpus: every churn dir here self-matched its own `.gitignore`, and 0 churn
entries have appeared since the fix. And `.claims/` — the directory CLAUDE.md names as the next
instance of the same class — has 3 tracked files, so the rule would **keep** it.

A rate budget is the only lever blind to how the next churn directory announces itself: N snapshots
of one path within a window → demote to hash-only, emit a visible record.

**Non-negotiables:**

- Represent as `skipped: true` + `skipped_reason: "policy"` — **never a new never-revertible flag**.
  PROTOCOL.md §8 enumerates the never-revertible rail as exactly `skipped` and `withheld`; §10 tells
  consumers to ignore unknown fields, so a third flag is *additive in schema and breaking in the
  safety rail*. A conformant consumer would treat a demoted entry as restorable. (`baseline_unknown`
  is not a precedent: it self-enforces by riding on `before: null`, which `build_plan` refuses on
  directly. A demoted entry's hashes look valid.) **The `skipped_reason` field itself lands in this
  session's round — the producer is what remains.**
- Scope to untracked paths first. A legitimate hot file during a large refactor can trip the budget
  and lose undo on the file the user most cares about.
- Reversible by config, and **not** clearable by `--ack-degraded`: a standing policy is not an
  incident, and letting a user acknowledge it away is its own small lie.
- Minimum disclosure so no one is surprised at `undo` time days later: stderr warning + persisted
  counter at demotion time (follow the `drain_io_failures` pattern), a standing non-ack-clearable
  `status` line, a `doctor` check naming the active rule, and per-entry honest wording at read time.

**Companion, cheaper and safer:** make `doctor` a churn **advisor** — report high-churn paths with
the remedy "add to ignore globs". Recommendation, never silent action. And implement the
`config.toml` ignore globs that **PROTOCOL.md §3 already promises and no code has ever read**
(the only config keys read anywhere today are `ttl_days`, `memory_enabled`, `memory_inject_max`).

---

## 4. Refuted — do not resurrect without reading this

### 4.1 No-op suppression in `Recorder::stage` — REFUTED as a churn fix

- **Disk saving: 0.** `put_result` early-returns on `path.exists()` before writing
  (`agentrec-core/src/store.rs`). A no-op `put` already costs no bytes. Suppression saves a read and
  a sha256 — CPU and page cache, not store bytes.
- **Log saving: 0.37%** — 31 of 8346 entries have `before == after`, both non-null.
- The adjacent idea (collapse repeated per-path entries within a turn) has a measured population of
  **exactly 0 of 8346** — `TurnEngine::observe_changes` already keeps only the first observation per
  path per turn.
- **It reverses a founder-decided AC.** IMPLEMENTATION.md AC C8 requires that a zero-net-change
  mutation records the file with identical before/after hashes and `op: modify`.
- It also removes the A3 dedup mtime-touch, which is what spares a recurring-content blob from
  `enforce_budget`'s hard delete (eviction's keep-set does not include `open.json`, unlike purge's
  citer set — a real pre-existing asymmetry worth fixing on its own).

**Relabel, don't delete.** `observe_changes` bumps `last_change_at` and opens a turn unconditionally,
so metadata-only churn events start spurious bare turns and extend quiet windows. Lever 1 is a
**turn-boundary-hygiene** fix. Build it for that reason; do not credit its yield against churn.

### 4.2 zstd inside `BlobStore` — REFUTED at current priority

- **Wrong axis.** ~2.6x on text where the measured problem is 58x. It makes the O(N²) class cost
  20 MB instead of 52 MB.
- **Blocker without a store-format gate.** Mixed-binary operation is the *documented normal deploy
  state* here (launchd held the 07-12 binary for days while CLI verbs ran whatever was on PATH). The
  address is sha256 of plaintext, so an unaware reader reports `Corrupt`, not "unsupported" —
  `undo` refuses. Worse: each binary sees the other's format as corrupt and takes the A4 *heal*
  branch, rewriting the same address forever. Per-blob magic sniffing is unsafe (a plaintext blob
  can begin with the zstd frame magic).
- **Breaking at the protocol boundary.** §6 pins the `objects/<first2>/<rest>` layout and mandates
  that consumers handle dangling refs — its only sanctioned degradation is *absent*, never
  *present-but-undecodable*. §10's versioning is per-line/per-schema JSON; there is no object-byte
  counterpart, so this cannot be made additive by the protocol's own machinery.
- Would also require: decompress-before-hash in `put_result`'s intact-check and in `get`; decode
  failure mapped to `Corrupt` never `Missing`; rewrite of `kill9_leaves_no_corrupt_blob`; a decision
  on whether the 2 GiB budget and 10 MiB cap are logical or on-disk bytes (they would silently
  diverge).

If ever revisited: a separate `objects.z/` tree keeps `objects/` byte-faithful, and the §6 amendment
must land **before** the 1.0 freeze.

### 4.3 Reclaim-at-turn-close — REFUTED, unsound by construction

`put_result` returns `Stored(hash)` on **both** the fresh-write path and the dedup-hit path. The
daemon cannot tell whether it *created* a blob or *hit an existing one*. So any "reclaim what I put
this turn, minus the final after" list deletes blobs it never created:

> T1 records `X after=hA`. T2 edits X→hB→back to hA→hC. The `hA` re-put in T2 is a dedup hit on
> **T1's blob file** (one file per content). Reclaiming it destroys T1's `after` → `diff T1` prints
> "snapshot unavailable", `undo T1` refuses.

Adding a created/deduped discriminator does not fix it — a prior turn's blob can be legitimately
re-created after an intervening eviction. It also deletes by **attribution** rather than by absence,
which is the exact argument that killed `purge --paths`, and it must run while the daemon is live —
inverting the liveness refusal that makes `purge --orphans` safe. It would race `undo`, which `put`s
the pre-revert blob and appends its citing turn afterwards.

**The staging-dir variant is expensive-but-possible**, and must not be averaged with this one. It
needs ~12 read paths taught about staging (recovery, purge ref-set, `list_hashes`, `total_bytes`,
`enforce_budget`, `orphan_bytes`, `status`, `doctor`, `diff`/`show`, `build_plan`, `execute_revert`,
memory pin-diff), a TTL floor above `MAX_BRACKET_MS` (2 h), archive-not-delete on expiry, and care
that a staging dir placed *under* `objects/` is skipped by `list_hashes` yet counted by
`total_bytes` — inflating the over-budget trigger while pointing users at the one tool structurally
unable to reclaim it.

---

## 5. Cross-cutting items surfaced by the redteam

- **`doctor` is blind to all of this.** `check_degraded` keys only on `state.json` counters. A log
  citing never-stored hashes passes clean, exit 0. Any storage lever should land with a sampled
  ref→blob presence check in the same commit.
- **`enforce_budget`'s keep-set omits `open.json`**, unlike `purge`'s citer set. Pre-existing.
- **Torn-line safety is a `purge --orphans`-only property.** `enforce_budget`, `purge_prompts` and
  `purge_snapshots_before` all build keep-sets from *parsed* records and hard-delete. Anything
  routing reclaim through the eviction path inherits the weaker guarantee.
- **`status` rich-rate will shift** if any entry-suppressing lever lands — the turns it removes are
  predominantly bare, so the denominator moves. Rich-rate false alarms have already cost a
  diagnosis round in this repo; recalibrate the warning threshold in the same commit or the fix
  reads as a regression.
- **Protocol timing is a cost input.** Any `FileEntry`/turn field is materially cheaper before the
  Phase 2.0 protocol-1.0 freeze than after. Note the conformance fixture corpus **does not exist
  yet** (IMPLEMENTATION.md N1, unbuilt) — `docs/fixtures/` is the blame-GIF demo repo.
