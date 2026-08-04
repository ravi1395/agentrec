<!-- Staged, not installed: cp -r claude-setup/skills/agentrec-release .claude/skills/ (project-level, so the global `ship` skill's step-0 deferral finds it), same as claude-setup/skills/agentrec-memory. -->
---
name: agentrec-release
description: Use when cutting an agentrec release — "ship agentrec", "cut vX.Y.Z", "bump the version", "tag a release", or when the release workflow's verify-version gate fails. Bumps every version site in lockstep, commits, tags, pushes, then hands off registry publishing. Overrides the global `ship` skill in this repo.
argument-hint: "[X.Y.Z | major|minor|patch]"
---

# agentrec-release

Repo-specific release. Takes precedence over the global `ship` skill (whose step 0
defers to exactly this).

**What this skill does NOT do:** publish to crates.io or npm. Those need registry
tokens the agent must never hold — see *Hand-off* at the tail. It stops after the
tag is pushed.

## Why it exists

agentrec carries the version in **six files (eight checked fields)** across three ecosystems, and two of
them fail in ways nothing catches locally:

| Site | Field | Failure if missed |
|---|---|---|
| `Cargo.toml` | `workspace.package.version` | both crates ship the old version |
| `cli/Cargo.toml` | `agentrec-core` dep `version` pin | `cargo publish -p agentrec` fails — the pin resolves against the **registry**, not the path |
| `Cargo.lock` | `agentrec`, `agentrec-core` entries | `--locked` release build fails in CI |
| `npm/package.json` | `version` | `npm publish` ships a wrapper that downloads a mismatched asset |
| `.claude-plugin/marketplace.json` | `metadata.version` | plugin users see a stale version |
| `claude-plugin/agentrec/.claude-plugin/plugin.json` | `version` | same |

The release workflow's `verify-version` job asserts all eight fields against the tag before
building anything. **The gate reads structurally (`cargo metadata` / `jq`), never by
grepping the version string** — `cli/Cargo.toml` carries third-party pins like
`regex = "1"` beside ours.

## Preconditions — gate, stop if any fails

1. On `main`, clean tree, up to date with `origin/main`.
2. `cargo test --workspace -- --test-threads=3` green — quote pass/fail/ignored counts.
   Red or unrunnable → stop and report. (`--test-threads=3` is not optional: FSEvents
   contention flakes ~6 daemon tests under full parallelism.)
3. `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` clean.
4. `VERIFY-LEDGER.md`: **only rows scoped to this version's acceptance criteria block.**
   This repo deliberately carries long-lived recorded-not-blocking debts and open
   founder-pending rows; treating every one as a blocker would block every release
   forever. Rows for other versions get *mentioned*, not enforced.

## Steps

**1. Resolve version.** Explicit `X.Y.Z` from the argument; `major|minor|patch` →
compute from `Cargo.toml`'s current `workspace.package.version`. Absent or ambiguous →
ask. Never guess a major.

**2. Bump the five declared sites** (`Cargo.lock`, the sixth file, is generated next):

- `Cargo.toml` → `[workspace.package] version`
- `cli/Cargo.toml` → `agentrec-core = { path = "../agentrec-core", version = "X.Y.Z" }`
- `npm/package.json` → `.version`
- `.claude-plugin/marketplace.json` → `.metadata.version` — **not** `.plugins[0].version`;
  agentrec keys it under `metadata`
- `claude-plugin/agentrec/.claude-plugin/plugin.json` → `.version`

**3. Regenerate the lockfile:** `cargo update --workspace`.

**4. Run the gate locally — same script CI runs:**

```bash
bash .github/scripts/check-versions.sh X.Y.Z
```

Every line must read `ok`. A green run here means `verify-version` cannot surprise you
after the tag is public. Do not proceed on a MISMATCH.

**5. Update the record.**
- `CLAUDE.md` § Status — house rule: updated every delivery round. State what shipped.
- `README.md` / `docs/launch/distribution-publish-runbook.md` if either names a current
  version.
- A claimd Stop hook fires on these doc edits and will flag paths without claim
  coverage. Either declare a claim for the release commit, or cite the repo's
  **undecided claimd doc-scope rule for normative docs** (recorded in CLAUDE.md
  § Founder-pending) and move on. Do not silently ignore it.

**6. Build check:** `cargo build --release --locked -p agentrec` — this is the exact
invocation CI runs, so a stale lockfile surfaces here rather than in the release.

**7. One commit, all release files:**

```
chore(release): vX.Y.Z — <one-line summary>
```

The bump commit **must be the commit that gets tagged** — the gate reads the manifests
at the tagged tree.

**8. Tag:** `git tag vX.Y.Z` (this repo's pattern is `v`-prefixed — `v0.1.0`, `v0.2.0`).

**9. Push — CONFIRM WITH THE USER FIRST.**

Pushing the tag triggers the release workflow, which creates a **public GitHub Release**
with four binary assets. That is irreversible in practice: the tag and release are
visible immediately and `install.sh` consumers can fetch them within seconds. State the
version and what it will publish, and wait for a clear yes.

Then, in this order — tag only if the branch push succeeded:

```bash
git push origin main
git push origin vX.Y.Z
```

**10. Watch the run.** `gh run watch` (or `gh run list --workflow=Release`). Jobs:
`verify-version` → `build` ×4 → `publish` → `verify-install` ×4. The `verify-install`
leg does a real network `curl | sh` against the just-published release, so a green run
is end-to-end proof, not a build-passed claim.

## Hand-off — registry publishing is the user's to run

The agent must not run these; they need registry tokens and are irreversible.
Print them and stop.

**crates.io — core BEFORE cli, order is load-bearing:**

```bash
cargo publish -p agentrec-core
```

Wait ~1 min for the index, then:

```bash
cargo publish -p agentrec
```

`-p agentrec` cannot even be dry-run before core is live: its path dep carries a
`version` pin that resolves against the registry at package time. An error before core
publishes is expected, not a defect.

**npm:**

```bash
cd npm && npm publish
```

**Homebrew:** bump the version and all four sha256s in the `homebrew-agentrec` tap
formula (the sha256 files ride along as release assets).

**Claude Code plugin:** no publish step — the repo itself is the marketplace, so
step 8's push makes the new version live.

Full context: `docs/launch/distribution-publish-runbook.md`.

## Rollback

Before the tag is pushed: `git tag -d vX.Y.Z` and amend or reset the commit.

After: the tag is public. Do not force-delete a pushed tag or a published release
(consumers may already have fetched it, and the repo's safety rules forbid force-pushing
shared refs) — cut a patch release instead.
