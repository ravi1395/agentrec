# Distribution publish runbook (founder-run; agent cannot hold tokens)

State at authoring (2026-08-04): npm name `agentrec` unclaimed (verified 404);
crates.io `agentrec`/`agentrec-core` unclaimed per `cargo search`. Claim early —
both registries are first-come.

Order matters. Do these after this branch merges (plugin marketplace works at
merge with no publish step; npm/cargo README sections go live when you publish).

## 1. crates.io — publish core BEFORE cli

```sh
cargo login            # crates.io token, once
cargo publish -p agentrec-core
# wait for the index to pick it up (~a minute), then:
cargo publish -p agentrec
```

`agentrec-core` dry-run already verified green. `cargo publish -p agentrec`
cannot even be dry-run until core is live on the index (its path dep carries
`version = "0.2.0"` and resolves against the registry at package time) — an
error before core is published is expected, not a defect.

## 2. npm

```sh
cd npm
npm publish            # needs npm login; name `agentrec` verified unclaimed 2026-08-04
```

Verified locally pre-publish: `node install.js` downloads + checksum-verifies
the v0.2.0 darwin-arm64 asset; `node bin/agentrec.js --version` → 0.2.0; the
lazy-download path (postinstall skipped) also verified.

## 3. Claude Code plugin — no publish step

Live once this branch is on `main`: the repo itself is the marketplace
(`.claude-plugin/marketplace.json`). Users run:

```
/plugin marketplace add ravi1395/agentrec
/plugin install agentrec@agentrec
```

Optional later: submission to any curated marketplace indexes.

## On every future release

- Tag `vX.Y.Z` (release workflow builds + verifies).
- Brew formula: bump version + 4 sha256s in `homebrew-agentrec`.
- npm: bump `npm/package.json` version to match the tag, `npm publish`.
- crates: bump happens via workspace version; `cargo publish -p agentrec-core`
  then `-p agentrec`.
- Plugin: bump `claude-plugin/agentrec/.claude-plugin/plugin.json` +
  `.claude-plugin/marketplace.json` versions in the same release PR.
  Consider folding npm/plugin version bumps into the release workflow later —
  manual for now, three files.
