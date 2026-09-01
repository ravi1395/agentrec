# Attest Phase 1 Probe B fixtures

Real captured libtest output from this repo's own suite (`cargo 1.97.1`,
target: `cli/tests/import_claude.rs`, 38 `#[test]` fns, 36 run + 1 `#[ignore]`d
+ 1 filtered by the exact-match target-scoping). All captured on this branch
(`feat/attest`), working tree otherwise unmodified — no `.rs`/`Cargo`/
`IMPLEMENTATION.md` files touched.

| File | Command | Exit |
|---|---|---|
| `whole-target.txt` | `cargo test -p agentrec --test import_claude` | 0 |
| `single-exact.txt` | `cargo test -p agentrec --test import_claude -- --exact ac3_zero_bytes_under_agentrec_in_tempdir` | 0 |
| `single-exact-ignored.txt` | `cargo test -p agentrec --test import_claude -- --exact persist::ac5b_oracle_real_corpus_measurement` (no `--ignored`, so it's skipped) | 0 |
| `single-exact-missing.txt` | `cargo test -p agentrec --test import_claude -- --exact this_test_does_not_exist_xyz` | 0 |
| `list.txt` | `target/debug/deps/import_claude-<hash> --list` (binary path resolved from `find target/debug/deps -name 'import_claude-*' -type f -perm +111`) | 0 |
| `corrupted.txt` | `whole-target.txt` deliberately corrupted: one `ok` line changed to `CORRUPTED_STATUS`, the summary line truncated to `test result: ok. 36 pass` (script: scratchpad `corrupt` inline python, not retained — see findings doc for the exact transform) | n/a (synthetic) |
| `single-exact-failed.handcrafted.txt` | NOT executed — hand-crafted `1 failed` shape, since no real test in this suite currently fails and repo source may not be modified to manufacture one. Labeled and explained in-file. | n/a (synthetic, 101 shown) |

`EXIT: N` is appended as a trailing line in each real-run fixture (captured
via `$?` immediately after the command, before any other command ran).
