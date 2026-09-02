# attest Phase 4A — manual E2E for `attest verify`

Run once, by hand, on 2026-09-02, branch `feat/attest`, one machine
(Apple Silicon macOS, Homebrew `rustc 1.97.1`). Everything below is pasted
output, not a summary of output.

Neither leg ran against the production working tree, and neither started
`agentrec record` anywhere. `pgrep -fl 'agentrec record'` was re-checked after
both legs (see the tail of this file).

---

## Leg 1 — verify ONE real claim of this repo

The production tree is dirty (uncommitted Phase 4 work), and `attest verify`
refuses a dirty tree by design. So the leg ran against a `git clone --local` of
the repo, which is a real git repository at the repo's committed HEAD with a
clean tree. Claim chosen for speed: an `agentrec-core` unit test.

```
$ git clone --local -q /Users/ravichandrasekhar/Projects/agentrec $SP/e2e/clone
$ cd $SP/e2e/clone && git rev-parse HEAD
e12dc160d5aa295e27b27adfb3d73e7f0894bf12
$ git status --porcelain      # (no output — clean)

$ mkdir -p .agentrec
$ agentrec attest derive --crate agentrec-core
discovered 259 test(s): 259 new, 0 renamed, 0 rehashed, 0 unchanged

$ agentrec attest verify c_01M1GTEYG7Q7PB6FMRQTGEN2RZ
c_01M1GTEYG7Q7PB6FMRQTGEN2RZ agentrec_core::attest::types::tests::claim_ids_are_prefixed_and_distinct_from_turn_ids -> confirmed (1 run)

$ grep '"kind":"verdict"' .agentrec/attest.jsonl
{"kind":"verdict","ts":1788344823105,"claim_id":"c_01M1GTEYG7Q7PB6FMRQTGEN2RZ","verdict":"confirmed","replay_commit":"e12dc160d5aa295e27b27adfb3d73e7f0894bf12"}
```

`replay_commit` equals the clone's HEAD, and a passing first run took exactly
one run — the retry policy costs nothing on the happy path.

### The dirty-tree refusal fired for real, before this leg could run

The first attempt refused, and the reason was a genuine defect rather than a
stale checkout:

```
$ agentrec attest verify c_01M1GTEYG7Q7PB6FMRQTGEN2RZ
agentrec: working tree is dirty — verify replays COMMITTED state only, so a
verdict minted now would be stamped with a commit that never held these bytes.
Commit or stash first:
?? agentrec-core/agentrec-core/
```

**`agentrec-core/agentrec-core/target` was 284 MB, and `attest derive --crate
agentrec-core` had just created it.** Mechanism:
`adapter_cargo::build_targets` sets `CARGO_TARGET_DIR` to
`crate_root.join("target")` while ALSO setting `current_dir(crate_root)`, so a
RELATIVE `--crate agentrec-core` produces the relative target dir
`agentrec-core/target`, which cargo then resolves against the CHILD's cwd —
landing at `agentrec-core/agentrec-core/target`.

- This is Phase 3 code (`adapter_cargo.rs`), reached through Phase 3's
  `attest derive`. It is **not fixed here** — the plan names that module as
  Phase 3's frozen contract. Reported to the orchestrator; `attest derive
  --crate <relative-path>` is affected on any repo.
- `attest verify` was never affected: `replaycmd` passes an absolute extract
  path.
- `attest coverage` WAS affected, since it forwards `--crate`. Fixed inside
  this chunk's own file by canonicalizing the path before use
  (`coveragecmd::run`), with the mechanism recorded in the code comment.

## Leg 2 — a real repo test binary inside a `git archive` extract

The plan requires this because `git archive` strips `.git` and `claim-false` is
PERMANENT: a test that needs to be inside a git repository would fail in the
extract for a reason unrelated to its claim.

```
$ git archive --format=tar -o $SP/e2e/head.tar HEAD
$ tar -xf $SP/e2e/head.tar -C $SP/e2e/extract
$ ls -a $SP/e2e/extract | head -8
.
..
.claims
.claude-plugin
.github
.gitignore
AGENTS.md
ATTEST-FORMAT.md
```

No `.git` — which is the whole reason `git archive` was chosen over
`git worktree add`.

```
$ cd $SP/e2e/extract && cargo test --test golden
...
test three_fresh_builds_are_byte_identical ... ok

test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.93s
```

**33 passed / 0 failed.** `golden.rs` `git_init`s its own tempdirs, so it does
not depend on the checkout being a repository — confirmed by running it, not by
reading it.

**Scope of this result, stated so it is not over-read:** one test binary, one
run, on one machine. It does NOT establish that every `cli/tests/*.rs` binary is
git-independent. The other binaries were spot-checked by reading for
`git_init`, and a spot check is not a proof. Until every binary has been run in
an extract, a `claim-false` on an integration claim carries a residual risk of
being an artifact of the missing `.git` — and `claim-false` is permanent.

## Daemon check

```
$ pgrep -fl 'agentrec record'
39492 /Users/ravichandrasekhar/.local/bin/agentrec record --root /Users/ravichandrasekhar/Projects/agentrec
```

One process, the pre-existing dogfood daemon (pid 39492). No daemon was started
by either leg.

## Environment deviation carried by both legs — SINCE CLOSED

**Superseded by Phase 4 chunk C (AC-ATTEST-P4C-3); kept because it describes
the environment the two legs below actually ran under.** At the time of these
runs the replay inherited this process's environment: the orchestrator's pinned
`env_clear()` plus allowlist was NOT implemented, on the reasoning that the
spawn happens inside `adapter_cargo::CargoAdapter::run`, named as Phase 3's
frozen contract. Chunk C implemented it additively instead — `RunEnv` is a
parameter on `Adapter::run`, so the contract extended rather than broke, and
`attest verify` now scrubs. The allowlist is stated once in `ATTEST-FORMAT.md`
§ "Replay environment".

Independent of the scrub, and true then and now: the extract's `target/` is a
symlink into a per-root, **per-commit** cache under `.agentrec/attest-target/`.
