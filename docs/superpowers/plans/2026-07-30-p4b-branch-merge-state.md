# Branch + merge state for Phase 2.0 (P1–P4 shipped, P4b planned)

**Captured 2026-07-30** at `feat/phase-2-0-substrate` = `664060a`. Verified with `git
for-each-ref` / `git branch --merged` / `git rev-list --left-right`, not from memory. Re-verify
before acting — this is a snapshot, and every count below rots the moment anything is committed.

Purpose: so the eventual merge to `main` misses nothing. **There is one real hazard** (§3) — read
that section before merging anything.

## 1. Where the work lives

`feat/phase-2-0-substrate` (local only, **never pushed**, `664060a`) is the integration branch and
**contains all of P1–P4**. It is **52 commits ahead of `main`, 0 behind**.

Every P-phase branch is fully contained in it — `git rev-list --count substrate..<branch>` = **0**
for all seven, meaning they carry nothing the substrate lacks:

| Branch | Tip | Unmerged commits | Content |
|---|---|---|---|
| `feat/p1-import-classifier` | `b80a3a5` | 0 | P1 — `import claude --dry-run` classifier, 4-tier ladder |
| `fix/p1-t15-path-normalization` | `7f1c8eb` | 0 | P1 follow-up — the path-norm defect + T1.5 correction |
| `feat/p2-import-persist` | `5f8f0bf` | 0 | P2 — persist imported turns, T2 git-blob resolution, undo refusal |
| `fix/p2-out-of-cwd-countability` | `32ee9b7` | 0 | P2 follow-up — out-of-cwd counter, rich-rate fix |
| `feat/p3-golden-harness` | `3c51de4` | 0 | P3 — golden byte-pinning harness (27 tests / 24 files) |
| `integration/phase-2-0-p2p3` | `32ee9b7` | 0 | P2+P3 integration branch (superseded by substrate) |
| `feat/phase-2-0-p4` | `39a1770` | 0 | P4 — `RepositoryView` extraction, gate PASS |

**Consequence:** all seven are safe to delete once the substrate merges. They are recorded here so
deleting them loses no information. (Deletion is founder-run — the git-guardrails hook blocks agent
`branch -D`.)

`main` itself is **8 commits ahead of `origin/main`, unpushed** — all docs (spec hardening,
plan chunking, the founder-directed CLAUDE.md slim). Those must reach `origin` too; they are not
in any PR.

## 2. Merge order to `main`

```
origin/main
  └── main                      (+8 unpushed docs commits)
        └── feat/phase-2-0-substrate   (+52: P1, P2, P3, P4, and the P4b design+plan)
              └── feat/phase-2-0-view-completion   (P4b — not yet created)
```

Substrate is **0 behind `main`**, so this is a fast-forward with no rebase needed *as of this
snapshot*. Push `main` first, then open the substrate PR.

## 3. ⚠ THE HAZARD — `fix/perf-evidence-round` collides with P4b-4

`fix/perf-evidence-round` (`5cfbc82`, **pushed to `origin`, no PR opened**) is **not** in the
substrate and diverges hard: **57 commits on substrate / 24 on perf-evidence** since their
merge-base `1ece033`.

It touches 8 files the substrate also touches:

```
.claims/claims.jsonl   CLAUDE.md   VERIFY-LEDGER.md
agentrec-core/src/retention.rs     cli/src/cmds.rs
cli/src/daemon.rs      cli/src/main.rs      cli/tests/integration.rs
```

`retention.rs` and `cmds.rs` are **exactly the two files P4b-4 rewrites**, and the collision is
semantic, not textual:

| On perf-evidence | Effect on P4b-4 |
|---|---|
| `aac459b` splits `enforce_budget` into `plan_eviction` + `execute` (behavior-identical). Public API becomes `plan_eviction` / `execute` / `enforce_budget` (the last retained). | **AC16's fixture is written against `enforce_budget` called from `status`.** Post-merge it must target `plan_eviction`, or the daemon tick. |
| `98bd074` moves eviction **out of the read verb**: `cmds.rs` now calls `plan_eviction` (read-only) and `status` "renders what a tick would do" — eviction happens on the daemon tick instead. | **P4b-4's AC "`status` still evicts" becomes FALSE** — and it is false *by design* there, not by regression. That AC must be restated, not enforced. |

**What survives the collision, and why AC16 still matters:** `cmds.rs` on perf-evidence still
builds `owned_turns` from the **unfiltered** `all_turns` and still passes that slice to
`plan_eviction`. So the data-loss concern is unchanged — a filtered protect-set still loses
excluded turns' `prompt_ref`s and blob hashes. AC16's *substance* is intact; only its *call target*
and its "after status evicts" *wording* need rewriting.

P4.md already anticipated this in its Notes ("if that branch has merged by the time you start,
re-read `cmds.rs`/`retention.rs` … part of the pure-read split may already exist"). It is now
concrete rather than hypothetical.

### Decision required before P4b-4 starts

Pick one, and record it in the P4b plan's decisions log:

- **(a) Merge `fix/perf-evidence-round` first** (open its PR, get the Linux CI leg, merge to
  `main`), then rebase the substrate, then execute P4b **against the post-merge shape**. P4b-4's
  AC16 and its "status still evicts" AC get rewritten in the task file *before* an executor sees
  them. Cleanest; costs a rebase of 52 commits.
- **(b) Execute P4b first**, merge the substrate, then rebase perf-evidence's 24 commits onto it
  and resolve `retention.rs`/`cmds.rs` by hand. Risks a hand-merge in the exact file that carries
  the data-loss gate.
- **(c) Keep perf-evidence parked indefinitely.** Only honest if someone decides its 15 tests and
  3 real defect fixes are not shipping — CLAUDE.md currently lists opening that PR as
  founder-pending, so this is the status-quo-by-inaction option, not a decision.

**Recommendation: (a).** The AC16 fixture is easier to write once against the final shape than to
write, gate, and then rewrite. And perf-evidence's PR is what unlocks the Linux CI timing leg that
CLAUDE.md already lists as founder-pending.

## 4. Other unmerged branches — triage

Not in the substrate, and none block P4b:

| Branch | Tip | Status |
|---|---|---|
| `feat/memory-v1` | `dca8cab` | upstream `[gone]` — merged via PR #2, local leftover |
| `fix/gitignore-self-match` | `b2990ac` | upstream `[gone]` — merged, leftover |
| `fix/honesty-round` | `d78d328` | upstream `[gone]` — merged, leftover |
| `fix/purge-orphans-gc` | `444a8f0` | upstream `[gone]` — merged, leftover |
| `fix/residuals-round` | `ff76b58` | upstream `[gone]` — merged via PR #7, leftover |
| `fix/review-findings-043c749` | `4c67243` | upstream `[gone]` — merged, leftover |
| `worktree-fix-doctor-inotify` | `7560659` | upstream `[gone]` — merged, leftover |
| `fix/blame-attribution-and-noise-folding` | `bdb911c` | local-only; content believed folded into later rounds — **verify before deleting** |
| `fix/ignore-rebuild-gate` | `897f584` | local-only; ditto — **verify before deleting** |
| `gate-review-orphans` | `45a5628` | local-only review scratch |
| `claude/magical-mendeleev-723a61`, `claude/xenodochial-matsumoto-10ff1b` | `d5aecf2` | agent scratch branches, 2026-07-12 |
| `claude/strange-gagarin-bbd4af` | `39b59f9` | agent scratch, has a worktree attached |
| `old-history-local` | `067337a` | **do not push** — pre-rewrite history (recorded in memory as a known trap) |

The seven `[gone]`-upstream branches are the 13-stale-branch cleanup CLAUDE.md already lists as
founder-pending. The two marked **verify** are the only ones whose deletion could lose work.

## 5. Live worktrees

| Path | At | Note |
|---|---|---|
| `~/Projects/agentrec` | `2ade38a` `main` | primary; a **production daemon watches this tree** — never build or test here from an agent |
| `~/Projects/agentrec-phase2` | `664060a` substrate | where P4b work happens |
| `.claude/worktrees/perf-evidence` | `5cfbc82` | the hazard branch above |
| `.claude/worktrees/fix-doctor-inotify` | `7560659` | stale |
| `.claude/worktrees/strange-gagarin-bbd4af` | `39b59f9` | stale |
| `/private/tmp/.../scratchpad/p4b-skeptic` | detached | this session's skeptic worktree; disposable |
| `/private/var/folders/.../claimd-wt-*` | detached | claimd's own worktree |

## 6. Pre-merge checklist

- [ ] Answer §3 — the perf-evidence ordering decision. **Blocking for P4b-4**, not for P4b-1..3.
- [ ] Push `main`'s 8 unpushed docs commits to `origin`.
- [ ] Decide whether the substrate merges as one PR or splits (P1–P4 shipped code vs the P4b
      design/plan docs) — 52 commits is a large review surface.
- [ ] Confirm the substrate is still 0 behind `main` at merge time (it is now; that can change).
- [ ] After merge: delete the seven contained P-branches (§1) — recorded here, so nothing is lost.
- [ ] Verify the two "verify before deleting" branches in §4 before the stale-branch sweep.
- [ ] Re-run `cargo test --workspace -- --test-threads=3` on the merge result; the substrate's
      baseline is 537/0/2 and perf-evidence's was 443/0/1 on its own base — neither number
      survives a merge, so measure, don't assume.
