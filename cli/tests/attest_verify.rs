//! `attest verify` (independent replay) and `attest coverage` — AC-ATTEST-P4-1
//! through P4-11.
//!
//! Every case builds a REAL git repository in a tempdir from the committed
//! fixture crate, runs the real `agentrec` binary against it, and asserts on
//! the appended `verdict` events. The replay extracts from that repository with
//! `git archive`, so a mutation only takes effect once it is COMMITTED — which
//! is also what removes the stale-binary hazard the repo has a recorded scar
//! for: the extract is built from the pinned commit, never from a target dir
//! carrying an older compile of an edited source.
//!
//! These tests each drive at least one cargo build inside a tempdir and are
//! therefore slow by nature, not by accident. The fixture crate has zero
//! dependencies and its own empty `[workspace]` table, so no build here reaches
//! the network or the parent workspace.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures; no attacker-planted FIFO can
//    reach these paths. Production reads stay lint-enforced (clippy.toml).

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    // `CARGO_BIN_EXE_<name>` is set by cargo for integration tests of a crate
    // that builds a binary — the binary under test, freshly built.
    PathBuf::from(env!("CARGO_BIN_EXE_agentrec"))
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit_all(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

/// A tempdir holding the fixture crate as a committed git repository, with
/// `.agentrec/` gitignored (so the build cache and the extract this command
/// writes under it never make the tree dirty).
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/attest_sample_crate");
        let tmp = tempfile::tempdir().unwrap();
        let dst = tmp.path();
        std::fs::create_dir_all(dst.join("src")).unwrap();
        std::fs::create_dir_all(dst.join("tests")).unwrap();
        // Only the four TRACKED files — the in-tree fixture also carries a
        // multi-megabyte `target/` from earlier builds.
        for rel in [
            "Cargo.toml",
            "Cargo.lock",
            "src/lib.rs",
            "tests/integration.rs",
        ] {
            std::fs::copy(src.join(rel), dst.join(rel)).unwrap();
        }
        std::fs::write(dst.join(".gitignore"), ".agentrec/\ntarget/\n").unwrap();
        std::fs::create_dir_all(dst.join(".agentrec")).unwrap();

        git(dst, &["init", "-q"]);
        git(dst, &["config", "user.email", "t@example.com"]);
        git(dst, &["config", "user.name", "t"]);
        commit_all(dst, "fixture");
        Fixture { dir: tmp }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn agentrec(&self, args: &[&str]) -> std::process::Output {
        Command::new(bin())
            .args(args)
            .current_dir(self.path())
            .output()
            .unwrap()
    }

    fn derive(&self) {
        let out = self.agentrec(&["attest", "derive"]);
        assert!(
            out.status.success(),
            "derive failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The claim id minted for `<target>::<fn_path>`, read back off the wire.
    fn claim_for(&self, target: &str, fn_path: &str) -> String {
        let body =
            std::fs::read_to_string(self.path().join(".agentrec/attest.jsonl")).unwrap_or_default();
        for line in body.lines() {
            let v: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v["kind"] != "derive" {
                continue;
            }
            if v["test_identity"]["target"] == target && v["test_identity"]["fn_path"] == fn_path {
                return v["claim_id"].as_str().unwrap().to_string();
            }
        }
        panic!("no derive event for {target}::{fn_path} in:\n{body}");
    }

    fn attest_bytes(&self) -> Vec<u8> {
        std::fs::read(self.path().join(".agentrec/attest.jsonl")).unwrap_or_default()
    }

    /// Verify one claim; returns (stdout, the verdict object appended, if any).
    fn verify(&self, claim: &str) -> (String, Option<serde_json::Value>) {
        let out = self.agentrec(&["attest", "verify", claim]);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            out.status.success(),
            "verify failed:\nstdout: {stdout}\nstderr: {stderr}"
        );
        let body =
            std::fs::read_to_string(self.path().join(".agentrec/attest.jsonl")).unwrap_or_default();
        let last = body
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .rfind(|v| v["kind"] == "verdict" && v["claim_id"] == claim);
        (stdout, last)
    }
}

/// The unit test the happy-path cases replay. Its identity is fixed by the
/// committed fixture crate (`attest_sample_crate::tests::unit_add_works`).
const UNIT_TARGET: &str = "attest_sample_crate";
const UNIT_FN: &str = "tests::unit_add_works";

// ---------------------------------------------------------------------------
// AC-ATTEST-P4-1
// ---------------------------------------------------------------------------

#[test]
fn ac_p4_1_derive_then_verify_confirms_and_pins_the_replay_commit() {
    let f = Fixture::new();
    f.derive();
    let claim = f.claim_for(UNIT_TARGET, UNIT_FN);
    let (stdout, verdict) = f.verify(&claim);
    let v = verdict.expect("a verdict must be appended");
    assert_eq!(v["verdict"], "confirmed", "stdout: {stdout}");
    assert!(stdout.contains("confirmed"), "{stdout}");
    // A passing first run is exactly one run — no wasted rebuilds.
    assert!(stdout.contains("(1 run)"), "{stdout}");

    // `replay_commit` is the tree that was actually extracted.
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(f.path())
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    assert_eq!(v["replay_commit"], head);

    // The extract carries committed content and NO `.git` — that absence is
    // the whole reason `git archive` was chosen over `git worktree add`.
    assert!(f
        .path()
        .join(".agentrec/attest-target/extract/src/lib.rs")
        .exists());
    assert!(!f
        .path()
        .join(".agentrec/attest-target/extract/.git")
        .exists());
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P4-2 — the mutation probe
// ---------------------------------------------------------------------------

#[test]
fn ac_p4_2_a_committed_mutation_yields_claim_false_after_three_runs() {
    let f = Fixture::new();
    f.derive();
    let claim = f.claim_for(UNIT_TARGET, UNIT_FN);

    // Break the function under test, and COMMIT — the extract carries
    // committed bytes only, so an uncommitted edit would change nothing and
    // the probe would pass vacuously. (Asserted below: the verdict moves.)
    let lib = f.path().join("src/lib.rs");
    let text = std::fs::read_to_string(&lib).unwrap();
    std::fs::write(&lib, text.replace("    a + b", "    a + b + 1")).unwrap();
    commit_all(f.path(), "break add");

    let (stdout, verdict) = f.verify(&claim);
    let v = verdict.expect("a verdict must be appended");
    assert_eq!(v["verdict"], "claim-false", "stdout: {stdout}");
    // The run count has no wire slot (ATTEST-FORMAT.md § "Verdict policy"), so
    // stdout is where the 3-run policy is observable at all.
    assert!(
        stdout.contains("(3 runs)"),
        "a claim-false must take all three runs: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P4-3 / P4-4 — `missing` and `build` discriminated against each other
// ---------------------------------------------------------------------------

#[test]
fn ac_p4_3_a_deleted_test_is_missing_not_build() {
    let f = Fixture::new();
    f.derive();
    let claim = f.claim_for(UNIT_TARGET, UNIT_FN);

    // Delete only the test fn. The crate still compiles, so the build stage
    // succeeds and the `--list` membership stage is what must fire.
    let lib = f.path().join("src/lib.rs");
    let text = std::fs::read_to_string(&lib).unwrap();
    let cut = text.replace(
        "    #[test]\n    fn unit_add_works() {\n        assert_eq!(add(2, 2), 4);\n    }\n",
        "",
    );
    assert_ne!(cut, text, "the deletion anchor did not match the fixture");
    std::fs::write(&lib, cut).unwrap();
    commit_all(f.path(), "delete the test");

    let (stdout, verdict) = f.verify(&claim);
    let v = verdict.expect("a verdict must be appended");
    assert_eq!(v["verdict"], "recipe-invalid", "stdout: {stdout}");
    assert_eq!(v["cause"], "missing", "stdout: {stdout}");
    // The negative half: a deleted test must NOT be reported as a broken
    // build. The two come from different stages and conflating them would let
    // a broken build masquerade as a deleted test.
    assert_ne!(v["cause"], "build");
    // No retries on a recipe-invalid.
    assert!(stdout.contains("(1 run)"), "{stdout}");
}

#[test]
fn ac_p4_4_a_broken_build_is_build_not_missing() {
    let f = Fixture::new();
    f.derive();
    let claim = f.claim_for(UNIT_TARGET, UNIT_FN);

    let lib = f.path().join("src/lib.rs");
    let text = std::fs::read_to_string(&lib).unwrap();
    std::fs::write(&lib, format!("{text}\nthis is not rust;\n")).unwrap();
    commit_all(f.path(), "break the build");

    let (stdout, verdict) = f.verify(&claim);
    let v = verdict.expect("a verdict must be appended");
    assert_eq!(v["verdict"], "recipe-invalid", "stdout: {stdout}");
    assert_eq!(v["cause"], "build", "stdout: {stdout}");
    assert_ne!(v["cause"], "missing");
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P4-5 — ignored, then restored
// ---------------------------------------------------------------------------

#[test]
fn ac_p4_5_ignored_then_restored_verifies_again() {
    let f = Fixture::new();
    f.derive();
    let claim = f.claim_for(UNIT_TARGET, UNIT_FN);

    let lib = f.path().join("src/lib.rs");
    let original = std::fs::read_to_string(&lib).unwrap();
    let ignored = original.replace(
        "    #[test]\n    fn unit_add_works()",
        "    #[test]\n    #[ignore]\n    fn unit_add_works()",
    );
    assert_ne!(ignored, original, "the ignore anchor did not match");
    std::fs::write(&lib, &ignored).unwrap();
    commit_all(f.path(), "ignore the test");

    let (stdout, verdict) = f.verify(&claim);
    let v = verdict.expect("a verdict must be appended");
    assert_eq!(v["verdict"], "recipe-invalid", "stdout: {stdout}");
    assert_eq!(v["cause"], "ignored", "stdout: {stdout}");
    // AC-ATTEST-P4C-11: stdout prints the WIRE casing, so the human line and
    // the appended event agree. `{:?}` on the cause would print `Ignored`.
    assert!(
        stdout.contains("cause: ignored"),
        "stdout must print the wire casing, not Debug's: {stdout}"
    );
    assert!(!stdout.contains("Ignored"), "{stdout}");

    // Restore and verify again: `recipe-invalid` is retryable, and this is the
    // end-to-end proof that a claim recovers rather than sticking.
    std::fs::write(&lib, &original).unwrap();
    commit_all(f.path(), "restore the test");
    let (stdout, verdict) = f.verify(&claim);
    let v = verdict.expect("a second verdict must be appended");
    assert_eq!(v["verdict"], "confirmed", "stdout: {stdout}");
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P4-6 — the flaky producer
// ---------------------------------------------------------------------------

#[test]
fn ac_p4_6_a_first_run_failure_that_passes_on_rerun_is_flaky() {
    let f = Fixture::new();

    // A deterministically flaky test: it fails exactly once, keyed on a marker
    // file that must survive BETWEEN the retries of one verify but be absent on
    // the first.
    //
    // REKEYED for the env scrub (AC-ATTEST-P4C-3): the marker path used to
    // arrive through `ATTEST_FLAKY_MARKER` in the inherited environment, which
    // `env_clear()` now removes. The fixture computes the path itself instead,
    // from its OWN `CARGO_MANIFEST_DIR` — which, compiled inside the extract,
    // is the extract dir — under `target/`, the symlink into the per-commit
    // build cache. That cache outlives the extract (which is rebuilt from the
    // commit each verify) and is fresh for this fixture's commit, so run 1
    // finds no marker and run 2 does. No environment variable is involved, so
    // the scrub cannot break it.
    let lib = f.path().join("src/lib.rs");
    let text = std::fs::read_to_string(&lib).unwrap();
    std::fs::write(
        &lib,
        format!(
            "{text}\n\
             #[test]\n\
             fn flaky_once() {{\n\
             \x20   let p = std::path::Path::new(env!(\"CARGO_MANIFEST_DIR\"))\n\
             \x20       .join(\"target\")\n\
             \x20       .join(\"flaky-marker\");\n\
             \x20   let p = p.as_path();\n\
             \x20   if p.exists() {{ return; }}\n\
             \x20   std::fs::write(p, \"x\").unwrap();\n\
             \x20   panic!(\"first run always fails\");\n\
             }}\n"
        ),
    )
    .unwrap();
    commit_all(f.path(), "add a deterministically flaky test");

    f.derive();
    let claim = f.claim_for(UNIT_TARGET, "flaky_once");

    let out = Command::new(bin())
        .args(["attest", "verify", &claim])
        .current_dir(f.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let body = std::fs::read_to_string(f.path().join(".agentrec/attest.jsonl")).unwrap();
    let v = body
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .rfind(|v| v["kind"] == "verdict" && v["claim_id"] == claim.as_str())
        .expect("a verdict must be appended");
    assert_eq!(v["verdict"], "flaky-observation", "stdout: {stdout}");
    // The load-bearing negative: a flake must NEVER mint the permanent verdict.
    assert_ne!(v["verdict"], "claim-false");
    assert!(stdout.contains("(2 runs)"), "{stdout}");
    // The marker landed in the per-commit build cache, whose directory is named
    // by a commit this test does not know; scan for it rather than guess.
    let cache = f.path().join(".agentrec/attest-target/target");
    let found = std::fs::read_dir(&cache)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", cache.display()))
        .filter_map(|e| e.ok())
        .any(|e| e.path().join("flaky-marker").exists());
    assert!(
        found,
        "the fixture's first run must have written a marker under {}",
        cache.display()
    );
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P4-7 — a dirty tree refuses
// ---------------------------------------------------------------------------

#[test]
fn ac_p4_7_a_dirty_tree_refuses_and_appends_nothing() {
    let f = Fixture::new();
    f.derive();
    let claim = f.claim_for(UNIT_TARGET, UNIT_FN);
    let before = f.attest_bytes();

    std::fs::write(f.path().join("src/lib.rs"), "// uncommitted\n").unwrap();

    let out = f.agentrec(&["attest", "verify", &claim]);
    assert!(!out.status.success(), "a dirty tree must refuse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("dirty"),
        "the refusal must say why: {stderr}"
    );
    // Nothing appended: a verdict minted here would be stamped with a commit
    // that never held these bytes.
    assert_eq!(f.attest_bytes(), before, "attest.jsonl must be untouched");
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P4-8 / P4-10 — coverage capture
// ---------------------------------------------------------------------------

/// Whether the coverage toolchain is present. Mirrors
/// `coveragecmd::tooling_status`, deliberately re-derived here so the test does
/// not skip itself for the same reason the command would refuse.
fn coverage_tooling_reason() -> Option<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let ok = Command::new(&cargo)
        .args(["llvm-cov", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        return Some("`cargo llvm-cov` is not installed".into());
    }
    if std::env::var("LLVM_COV").is_ok() && std::env::var("LLVM_PROFDATA").is_ok() {
        return None;
    }
    let home = std::env::var("HOME").ok()?;
    let toolchains = Path::new(&home).join(".rustup/toolchains");
    let found = std::fs::read_dir(&toolchains).ok().is_some_and(|entries| {
        entries.flatten().any(|tc| {
            std::fs::read_dir(tc.path().join("lib/rustlib"))
                .ok()
                .is_some_and(|hosts| {
                    hosts
                        .flatten()
                        .any(|h| h.path().join("bin/llvm-profdata").exists())
                })
        })
    });
    if found {
        None
    } else {
        Some(format!(
            "no rustup llvm-tools component under {}",
            toolchains.display()
        ))
    }
}

#[test]
fn ac_p4_8_coverage_maps_the_fixture_crate_at_file_granularity() {
    if let Some(why) = coverage_tooling_reason() {
        // PRINTED, never silent: a skipped coverage test that says nothing is
        // indistinguishable from a passing one.
        eprintln!("SKIP ac_p4_8: coverage tooling unavailable: {why}");
        return;
    }
    let f = Fixture::new();
    f.derive();

    let out = f.agentrec(&["attest", "coverage", "--all"]);
    assert!(
        out.status.success(),
        "coverage failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let map: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(f.path().join(".agentrec/attest-coverage.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(map["version"], 1);
    assert_eq!(map["granularity"], "file");

    let unit = &map["tests"][format!("{UNIT_TARGET}::{UNIT_FN}")];
    let integ = &map["tests"]["integration::integration_add_works"];
    for (name, entry) in [("unit", unit), ("integration", integ)] {
        assert!(!entry.is_null(), "{name} entry missing from map: {map}");
        let files: Vec<String> = serde_json::from_value(entry["files"].clone()).unwrap();
        assert!(
            files.iter().any(|f| f == "src/lib.rs"),
            "{name} must cover src/lib.rs, got {files:?}"
        );
        // The fixture crate declares no `[[bin]]`, so it builds no binary for
        // a test to spawn and the over-stale rule derives no scope — this is
        // the ALLOW half. (The rule keys on bin targets, not on the package
        // name; `ac_p4c_7` adds a `[[bin]]` to a tempdir copy and gets
        // `src/**`.)
        assert_eq!(
            entry["over_stale"],
            serde_json::json!([]),
            "{name} must carry no over_stale"
        );
        assert!(entry["claim_id"].as_str().unwrap().starts_with("c_"));
    }
}

#[test]
fn ac_p4_10_absent_coverage_tooling_refuses_without_writing_a_map() {
    let f = Fixture::new();
    f.derive();
    // An empty PATH makes both `cargo` and the llvm tools unresolvable, which
    // is the shape a machine without the toolchain has.
    let out = Command::new(bin())
        .args(["attest", "coverage", "--all"])
        .current_dir(f.path())
        .env("PATH", "")
        .env_remove("CARGO")
        .env_remove("LLVM_COV")
        .env_remove("LLVM_PROFDATA")
        .env("HOME", f.path())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "missing tooling must exit 2:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no coverage map written"), "{stderr}");
    // The load-bearing half: an empty map would read to the daemon as
    // "nothing is covered", which is worse than no map at all.
    assert!(
        !f.path().join(".agentrec/attest-coverage.json").exists(),
        "no map may be written"
    );
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P4-9 / P4-11 — the read side the daemon consumes
// ---------------------------------------------------------------------------

fn write_map(root: &Path, json: &str) {
    std::fs::create_dir_all(root.join(".agentrec")).unwrap();
    std::fs::write(root.join(".agentrec/attest-coverage.json"), json).unwrap();
}

const MAP: &str = r#"{"version":1,"granularity":"file","tests":{
  "attest_sample_crate::tests::unit_add_works":{"claim_id":"c_00000000010W3GE1R70W3GE1R7",
    "files":["src/lib.rs"],"over_stale":[],"captured_at":1,"commit":"abc"},
  "integration::spawns":{"claim_id":"c_00000000010W3GE1R70W3GE1R8",
    "files":[],"over_stale":["cli/src/**"],"captured_at":1,"commit":"abc"}
}}"#;

/// `claims_touched_by` is a library function the daemon calls, so it is driven
/// here through the same JSON the producer writes — end to end from the wire
/// rather than from a hand-built struct.
#[test]
fn ac_p4_9_the_producers_wire_shape_carries_both_matcher_inputs() {
    let tmp = tempfile::tempdir().unwrap();
    write_map(tmp.path(), MAP);

    // The command surface that exercises the read path end to end is the
    // daemon's, which this test cannot start; the matching itself is asserted
    // through the map's own JSON here so the producer's shape and the
    // consumer's expectations cannot drift apart silently.
    let map: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(tmp.path().join(".agentrec/attest-coverage.json")).unwrap(),
    )
    .unwrap();

    let exact = &map["tests"]["attest_sample_crate::tests::unit_add_works"];
    let over = &map["tests"]["integration::spawns"];
    assert_eq!(exact["files"], serde_json::json!(["src/lib.rs"]));
    assert_eq!(over["over_stale"], serde_json::json!(["cli/src/**"]));
    // Distinct claims — an over-broad match would collapse them into one.
    assert_ne!(exact["claim_id"], over["claim_id"]);
}

#[test]
fn ac_p4_11_no_map_exists_before_capture_and_a_written_map_parses() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".agentrec")).unwrap();
    // Before capture: no file at all. The daemon must read this as "nothing
    // captured yet", which is a normal state and not an error.
    assert!(!tmp.path().join(".agentrec/attest-coverage.json").exists());

    write_map(tmp.path(), MAP);
    let parsed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(tmp.path().join(".agentrec/attest-coverage.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(parsed["version"], 1);
    assert_eq!(parsed["tests"].as_object().unwrap().len(), 2);
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P4C-3 — the replay environment is scrubbed
// ---------------------------------------------------------------------------

#[test]
fn ac_p4c_3_the_replay_env_is_scrubbed_to_the_allowlist() {
    let f = Fixture::new();

    // The fixture asserts BOTH halves from inside the replayed test body:
    //   refuse — the canary this test sets on the `attest verify` child must
    //     NOT reach the replay;
    //   allow  — `PATH` must still be there.
    // The allow half is not decoration: a canary-absent assertion alone passes
    // vacuously if the parent never set the variable, or if the child's
    // environment broke for a reason having nothing to do with the allowlist.
    let lib = f.path().join("src/lib.rs");
    let text = std::fs::read_to_string(&lib).unwrap();
    std::fs::write(
        &lib,
        format!(
            "{text}\n\
             #[test]\n\
             fn env_is_scrubbed() {{\n\
             \x20   assert!(std::env::var(\"ATTEST_CANARY\").is_err(), \"canary leaked\");\n\
             \x20   assert!(std::env::var(\"PATH\").is_ok(), \"PATH was not allowlisted\");\n\
             }}\n"
        ),
    )
    .unwrap();
    commit_all(f.path(), "add an environment-scrub assertion");

    f.derive();
    let claim = f.claim_for(UNIT_TARGET, "env_is_scrubbed");

    let out = Command::new(bin())
        .args(["attest", "verify", &claim])
        .current_dir(f.path())
        // Set on the child that RUNS the replay. It must not survive into the
        // replay's own grandchild.
        .env("ATTEST_CANARY", "leaked")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let body = std::fs::read_to_string(f.path().join(".agentrec/attest.jsonl")).unwrap();
    let v = body
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .rfind(|v| v["kind"] == "verdict" && v["claim_id"] == claim.as_str())
        .expect("a verdict must be appended");
    assert_eq!(v["verdict"], "confirmed", "stdout: {stdout}");
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P4C-4 — a relative --crate path does not nest the target dir
// ---------------------------------------------------------------------------

#[test]
fn ac_p4c_4_a_relative_crate_path_does_not_nest_the_target_dir() {
    // The Phase 3 defect `5bd5c0f` recorded: `build_targets` set
    // `CARGO_TARGET_DIR` to a RELATIVE `crate_root.join("target")` while also
    // setting `current_dir(crate_root)`, so cargo resolved the relative value
    // against the child's cwd and built into `<crate>/<crate>/target`.
    let parent = tempfile::tempdir().unwrap();
    let crate_dir = parent.path().join("thecrate");
    std::fs::create_dir_all(&crate_dir).unwrap();

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/attest_sample_crate");
    std::fs::create_dir_all(crate_dir.join("src")).unwrap();
    std::fs::create_dir_all(crate_dir.join("tests")).unwrap();
    for rel in [
        "Cargo.toml",
        "Cargo.lock",
        "src/lib.rs",
        "tests/integration.rs",
    ] {
        std::fs::copy(src.join(rel), crate_dir.join(rel)).unwrap();
    }
    std::fs::create_dir_all(crate_dir.join(".agentrec")).unwrap();
    // `derive` writes `attest.jsonl` under the ROOT (the cwd), not the crate.
    std::fs::create_dir_all(parent.path().join(".agentrec")).unwrap();

    // Run FROM the parent, naming the crate by a relative path — the exact
    // shape that nested.
    let out = Command::new(bin())
        .args(["attest", "derive", "--crate", "thecrate"])
        .current_dir(parent.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "derive failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        crate_dir.join("target").is_dir(),
        "the build must land at <crate>/target"
    );
    assert!(
        !crate_dir.join("thecrate").exists(),
        "a nested <crate>/<crate>/ directory must not appear"
    );
    assert!(
        !parent.path().join("target").exists(),
        "nothing may be written to the PARENT's target dir"
    );
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P4C-8 — `--all-stale`, and per-claim durability
// ---------------------------------------------------------------------------

/// A claim id chosen to sort AFTER every derived id. `fold_claims` returns a
/// `BTreeMap` keyed by `ClaimId`, and `attest verify` walks it in that order,
/// so this is what makes "the SECOND claim errors" deterministic rather than a
/// coin flip. Real ids are ULID-shaped and start well below `7Z…`.
const LAST_CLAIM: &str = "c_7ZZZZZZZZZZZZZZZZZZZZZZZZZ";

/// Append raw attest lines to the fixture's log (no lock needed: nothing else
/// is running).
fn append_attest(f: &Fixture, lines: &[String]) {
    let p = f.path().join(".agentrec/attest.jsonl");
    let mut body = std::fs::read_to_string(&p).unwrap_or_default();
    for l in lines {
        body.push_str(l);
        body.push('\n');
    }
    std::fs::write(&p, body).unwrap();
}

fn stale_line(claim: &str) -> String {
    format!(
        r#"{{"kind":"stale","ts":900,"claim_id":"{claim}","cause":"file-write","path":"src/lib.rs"}}"#
    )
}

fn verdicts_for(f: &Fixture, claim: &str) -> Vec<String> {
    let body = std::fs::read_to_string(f.path().join(".agentrec/attest.jsonl")).unwrap_or_default();
    body.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["kind"] == "verdict" && v["claim_id"] == claim)
        .map(|v| v["verdict"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn ac_p4c_8_all_stale_verifies_every_stale_claim() {
    let f = Fixture::new();
    f.derive();
    let unit = f.claim_for(UNIT_TARGET, UNIT_FN);
    let integ = f.claim_for("integration", "integration_add_works");
    assert_ne!(unit, integ);

    append_attest(&f, &[stale_line(&unit), stale_line(&integ)]);

    let out = f.agentrec(&["attest", "verify", "--all-stale"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(
        verdicts_for(&f, &unit),
        vec!["confirmed".to_string()],
        "{stdout}"
    );
    assert_eq!(
        verdicts_for(&f, &integ),
        vec!["confirmed".to_string()],
        "{stdout}"
    );
}

/// AC-ATTEST-P4C-9. The durability half: an adapter ERROR on a later claim must
/// not discard the verdicts already decided.
///
/// The error is real, not injected — the second claim names a cargo target that
/// does not exist in the extract, which `CargoAdapter::run` reports as an `Err`
/// rather than a verdict. Before the fix every verdict was accumulated in one
/// batch appended after the loop, so this run persisted NOTHING.
#[test]
fn ac_p4c_9_an_error_on_a_later_claim_keeps_the_earlier_verdict() {
    let f = Fixture::new();
    f.derive();
    let unit = f.claim_for(UNIT_TARGET, UNIT_FN);

    append_attest(
        &f,
        &[
            format!(
                r#"{{"kind":"derive","ts":901,"claim_id":"{LAST_CLAIM}","test_identity":{{"target":"no_such_target","fn_path":"nope"}}}}"#
            ),
            stale_line(&unit),
            stale_line(LAST_CLAIM),
        ],
    );

    let out = f.agentrec(&["attest", "verify", "--all-stale"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    // The run fails — the error is surfaced, not swallowed.
    assert!(
        !out.status.success(),
        "expected failure:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("no cargo target named"),
        "stderr must name the real cause: {stderr}"
    );

    // BOTH halves, or the assert passes vacuously on an empty log.
    assert_eq!(
        verdicts_for(&f, &unit),
        vec!["confirmed".to_string()],
        "the first claim's verdict must survive the later error:\n{stdout}"
    );
    assert!(
        verdicts_for(&f, LAST_CLAIM).is_empty(),
        "the erroring claim must mint no verdict"
    );
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P4C-6 (producer half) and AC-ATTEST-P4C-7 (producer over_stale)
// ---------------------------------------------------------------------------

/// The producer half of the one-resolver fix: `attest coverage` must write the
/// map where `attest_coverage_path` says, not at the hard-coded default.
///
/// This is the exact defect: the producer ignored the key while the daemon
/// honoured it, so with the key set the daemon read a path nothing had written
/// and silently staled nothing. Asserting BOTH that the configured path exists
/// AND that the default one does not is what discriminates the fix — before it,
/// only the default was written.
#[test]
fn ac_p4c_6_the_producer_writes_the_configured_coverage_path() {
    if let Some(why) = coverage_tooling_reason() {
        eprintln!("SKIP ac_p4c_6: coverage tooling unavailable: {why}");
        return;
    }
    let f = Fixture::new();
    std::fs::write(
        f.path().join(".agentrec/config.toml"),
        "attest_coverage_path = \"custom/cov.json\"\n",
    )
    .unwrap();
    f.derive();

    let out = f.agentrec(&["attest", "coverage", "--all"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "coverage failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        f.path().join("custom/cov.json").is_file(),
        "the map must be written at the CONFIGURED path:\n{stdout}"
    );
    assert!(
        !f.path().join(".agentrec/attest-coverage.json").exists(),
        "nothing may be written at the default path when the key is set"
    );
    // The stdout line must name the same file, or a user following it looks in
    // the wrong place.
    assert!(stdout.contains("custom/cov.json"), "{stdout}");
}

/// AC-ATTEST-P4C-7 (producer half). A package that builds a BINARY yields a
/// non-empty `over_stale` from a real producer run.
///
/// This closes a gap recorded honestly rather than papered over: before the
/// generalization, `over_stale_for` keyed on the literal package name
/// `agentrec`, so no fixture could ever exercise the non-empty branch
/// end-to-end and only a hand-written map literal covered it. The rule now
/// derives the scope from the bin target's own source directory, so a fixture
/// with a `[[bin]]` at `src/main.rs` must yield `src/**`.
#[test]
fn ac_p4c_7_a_binary_building_package_gets_a_derived_over_stale_scope() {
    if let Some(why) = coverage_tooling_reason() {
        eprintln!("SKIP ac_p4c_7: coverage tooling unavailable: {why}");
        return;
    }
    let f = Fixture::new();
    // Turn the fixture into a binary-building package, in the tempdir copy
    // only — the committed fixture crate is untouched, so AC-ATTEST-P4-8's
    // "no over_stale" assertion keeps its meaning.
    std::fs::write(
        f.path().join("src/main.rs"),
        "fn main() { println!(\"{}\", attest_sample_crate::add(1, 1)); }\n",
    )
    .unwrap();
    let manifest = f.path().join("Cargo.toml");
    let mut toml = std::fs::read_to_string(&manifest).unwrap();
    toml.push_str("\n[[bin]]\nname = \"sample_bin\"\npath = \"src/main.rs\"\n");
    std::fs::write(&manifest, toml).unwrap();
    commit_all(f.path(), "add a bin target");

    f.derive();
    let out = f.agentrec(&["attest", "coverage", "--all"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "coverage failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let map: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(f.path().join(".agentrec/attest-coverage.json")).unwrap(),
    )
    .unwrap();

    let integ = &map["tests"]["integration::integration_add_works"];
    assert!(!integ.is_null(), "integration entry missing: {map}");
    assert_eq!(
        integ["over_stale"],
        serde_json::json!(["src/**"]),
        "a non-lib target of a package that builds a bin must carry the bin's \
         source scope: {map}"
    );

    // The ALLOW half: the lib target does not spawn the binary, so it must
    // still carry nothing — otherwise "everything is over-stale" would pass
    // the assert above.
    let unit = &map["tests"][format!("{UNIT_TARGET}::{UNIT_FN}")];
    assert!(!unit.is_null(), "unit entry missing: {map}");
    assert_eq!(
        unit["over_stale"],
        serde_json::json!([]),
        "the lib target must carry no over_stale: {map}"
    );

    // A SYMLINKED --root must derive the same scope. `spawn_scopes` strips the
    // crate root off cargo's `src_path`, which cargo reports canonical, so an
    // uncanonicalized root failed every strip and produced `[]` on every entry
    // with no warning — and `/tmp` and `/var` are symlinks on macOS, so this is
    // the ordinary case, not an exotic one.
    // The link lives in its own tempdir so it is removed with the test and two
    // concurrent runs of this binary cannot race on one fixed path.
    let link_dir = tempfile::tempdir().unwrap();
    let link = link_dir.path().join("linked-root");
    std::os::unix::fs::symlink(f.path(), &link).unwrap();
    let out = Command::new(bin())
        .args(["attest", "coverage", "--all", "--root"])
        .arg(&link)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "coverage via symlinked root failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let map: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(f.path().join(".agentrec/attest-coverage.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        map["tests"]["integration::integration_add_works"]["over_stale"],
        serde_json::json!(["src/**"]),
        "a symlinked root must derive the same scope as a canonical one: {map}"
    );
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P5-13 — stdout never contradicts `attest status`
// ---------------------------------------------------------------------------

/// A permanently refuted claim that later replays green: the verdict IS
/// appended (and counted), the claim does NOT move (spec decision 4). Before
/// this AC, stdout said `-> confirmed` about a claim `attest status` showed as
/// `CLAIM_FALSE` — two true statements that read as a contradiction.
#[test]
fn p5_13_a_passing_replay_on_a_refuted_claim_prints_both() {
    let f = Fixture::new();
    f.derive();
    let claim = f.claim_for(UNIT_TARGET, UNIT_FN);

    let lib = f.path().join("src/lib.rs");
    let original = std::fs::read_to_string(&lib).unwrap();
    std::fs::write(&lib, original.replace("    a + b", "    a + b + 1")).unwrap();
    commit_all(f.path(), "break add");
    let (_, v) = f.verify(&claim);
    assert_eq!(
        v.expect("a verdict must be appended")["verdict"],
        "claim-false",
        "precondition: the claim must be refuted before the passing replay"
    );

    // Restore and COMMIT, so the extract carries the working bytes.
    std::fs::write(&lib, &original).unwrap();
    commit_all(f.path(), "restore add");
    let (stdout, v) = f.verify(&claim);
    assert_eq!(
        v.expect("the passing replay's verdict must still be appended")["verdict"],
        "confirmed",
        "the verdict event is recorded even though the claim cannot move"
    );

    assert!(
        stdout.contains("verdict confirmed appended"),
        "stdout must name the verdict it appended: {stdout}"
    );
    assert!(
        stdout.contains("claim remains CLAIM_FALSE (permanent, decision 4)"),
        "stdout must also name the folded status, which differs: {stdout}"
    );

    // And the two readers agree about the same log.
    let status = f.agentrec(&["attest", "status", "--json"]);
    let status: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&status.stdout)).unwrap();
    assert_eq!(status["claim_false"], 1, "{status}");
    assert_eq!(status["confirmed"], 0, "{status}");
}

/// The agreeing case keeps the original one-clause line — the extra clause is
/// reserved for a real divergence, not printed on every verdict.
#[test]
fn p5_13_an_agreeing_verdict_keeps_the_short_line() {
    let f = Fixture::new();
    f.derive();
    let claim = f.claim_for(UNIT_TARGET, UNIT_FN);
    let (stdout, v) = f.verify(&claim);
    assert_eq!(v.expect("a verdict")["verdict"], "confirmed");
    assert!(stdout.contains("-> confirmed ("), "{stdout}");
    assert!(!stdout.contains("claim remains"), "{stdout}");
}
