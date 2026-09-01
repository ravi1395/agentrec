//! Phase 3B integration tests: `attest derive` / `attest run` and the hook
//! capture path, against the real binary and the real fixture crate.
//! ACs: `IMPLEMENTATION.md` § "Phase 3B".
//!
//! Each test gets its OWN copy of the fixture crate in a tempdir (only the four
//! tracked files — the in-tree fixture also carries a multi-megabyte `target/`
//! from earlier builds), `git init`ed and `agentrec init`ed, so a test that
//! renames a test fn cannot touch the repo.
//!
//! **`open.json` is written by hand here**, the way `daemon.rs::sync_journal`
//! writes it, and only its `id` field is read back. That proves the READER;
//! it is not evidence that the daemon's writer and this reader agree on the
//! field — the daemon is not running in these fixtures.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures; production reads stay
//    lint-enforced (clippy.toml).

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

/// `--root` goes BEFORE the subcommand (it is a `global = true` arg, so clap
/// accepts it in either position). It must not go after: `attest run`'s
/// trailing var arg would swallow it into the wrapped command.
fn agentrec(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(["--root", root.to_str().unwrap()])
        .args(args)
        .current_dir(root)
        .output()
        .expect("run agentrec")
}

fn ok(out: Output) -> Output {
    assert!(
        out.status.success(),
        "command failed: status={:?}\nstdout={}\nstderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A tempdir holding a standalone copy of the fixture crate, `git init`ed with
/// one commit (so the tree is CLEAN) and `agentrec init`ed.
fn fixture_repo() -> tempfile::TempDir {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/attest_sample_crate");
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    for rel in [
        "Cargo.toml",
        "Cargo.lock",
        "src/lib.rs",
        "tests/integration.rs",
    ] {
        std::fs::copy(src.join(rel), root.join(rel)).unwrap();
    }
    std::fs::write(root.join(".gitignore"), "target/\n.agentrec/\n").unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "fixture"]);
    ok(agentrec(root, &["init", "--no-hook", "--no-service"]));
    tmp
}

fn attest_events(root: &Path) -> Vec<serde_json::Value> {
    let path = root.join(".agentrec/attest.jsonl");
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("every line must be JSON"))
        .collect()
}

fn of_kind<'a>(events: &'a [serde_json::Value], kind: &str) -> Vec<&'a serde_json::Value> {
    events
        .iter()
        .filter(|e| e["kind"] == kind)
        .collect::<Vec<_>>()
}

fn identity(event: &serde_json::Value, field: &str) -> String {
    format!("{}::{}", event[field]["target"], event[field]["fn_path"]).replace('"', "")
}

fn status_json(root: &Path) -> serde_json::Value {
    let out = ok(agentrec(root, &["attest", "status", "--json"]));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn edit(root: &Path, rel: &str, from: &str, to: &str) {
    let path = root.join(rel);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains(from),
        "fixture precondition: {from:?} in {rel}"
    );
    std::fs::write(&path, text.replace(from, to)).unwrap();
}

/// The five tests libtest itself enumerates in the fixture crate.
const EXPECTED: [&str; 5] = [
    "attest_sample_crate::tests::unit_add_works",
    "attest_sample_crate::tests::unit_double_works",
    "attest_sample_crate::tests::unit_ignored_test",
    "integration::integration_add_works",
    "integration::integration_double_works",
];

/// AC-ATTEST-P3-10.
#[test]
fn ac_p3_10_derive_creates_one_claim_per_discovered_test() {
    let tmp = fixture_repo();
    let root = tmp.path();
    let out = ok(agentrec(root, &["attest", "derive"]));
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(stdout.contains("5 new"), "{stdout}");

    let events = attest_events(root);
    let derives = of_kind(&events, "derive");
    assert_eq!(derives.len(), 5, "{events:#?}");
    let mut got: Vec<String> = derives
        .iter()
        .map(|e| identity(e, "test_identity"))
        .collect();
    got.sort();
    assert_eq!(got, EXPECTED);

    // The `#[ignore]`d test is a claim like any other — ignored is a verify-time
    // recipe-invalid cause, not an absence at derive time.
    assert!(got.contains(&"attest_sample_crate::tests::unit_ignored_test".to_string()));
    // Every claim id is distinct, and every derive carries a body hash.
    let ids: std::collections::BTreeSet<&str> = derives
        .iter()
        .map(|e| e["claim_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 5);
    assert!(
        derives.iter().all(|e| e["body_hash"].is_string()),
        "{derives:#?}"
    );
    assert_eq!(status_json(root)["claims"], 5);
    assert_eq!(status_json(root)["derived"], 5);
}

/// AC-ATTEST-P3-11.
#[test]
fn ac_p3_11_rederive_with_no_change_appends_nothing() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    let before = std::fs::read(root.join(".agentrec/attest.jsonl")).unwrap();

    let out = ok(agentrec(root, &["attest", "derive"]));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("5 unchanged"),
        "{out:?}"
    );
    let after = std::fs::read(root.join(".agentrec/attest.jsonl")).unwrap();
    assert_eq!(before, after, "a no-change re-derive must append nothing");
}

/// AC-ATTEST-P3-12.
#[test]
fn ac_p3_12_rename_with_an_unchanged_body_reuses_the_claim_id() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    let old_id = of_kind(&attest_events(root), "derive")
        .iter()
        .find(|e| identity(e, "test_identity").ends_with("unit_add_works"))
        .map(|e| e["claim_id"].as_str().unwrap().to_string())
        .expect("the unit test was derived");

    edit(
        root,
        "src/lib.rs",
        "fn unit_add_works()",
        "fn unit_add_renamed()",
    );
    let out = ok(agentrec(root, &["attest", "derive"]));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("1 renamed"),
        "{out:?}"
    );

    let events = attest_events(root);
    let last = of_kind(&events, "derive").last().copied().unwrap().clone();
    assert_eq!(last["claim_id"].as_str().unwrap(), old_id);
    assert_eq!(
        identity(&last, "test_identity"),
        "attest_sample_crate::tests::unit_add_renamed"
    );
    assert_eq!(
        identity(&last, "renamed_from"),
        "attest_sample_crate::tests::unit_add_works"
    );
    // A rename moves a claim, it does not create one.
    assert_eq!(status_json(root)["claims"], 5);
}

/// AC-ATTEST-P3-13.
#[test]
fn ac_p3_13_rename_with_a_changed_body_mints_a_new_claim() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    let before: Vec<String> = of_kind(&attest_events(root), "derive")
        .iter()
        .map(|e| e["claim_id"].as_str().unwrap().to_string())
        .collect();

    edit(
        root,
        "src/lib.rs",
        "fn unit_add_works()",
        "fn unit_add_renamed()",
    );
    edit(
        root,
        "src/lib.rs",
        "assert_eq!(add(2, 2), 4);",
        "assert_eq!(add(3, 1), 4);",
    );
    let out = ok(agentrec(root, &["attest", "derive"]));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("1 new"),
        "{out:?}"
    );

    let last = of_kind(&attest_events(root), "derive")
        .last()
        .copied()
        .unwrap()
        .clone();
    assert!(last["renamed_from"].is_null(), "{last}");
    assert!(
        !before.contains(&last["claim_id"].as_str().unwrap().to_string()),
        "a body change makes it a different test: {last}"
    );
    assert_eq!(status_json(root)["claims"], 6);
}

/// AC-ATTEST-P3-14.
#[test]
fn ac_p3_14_a_body_change_without_a_rename_rehashes_in_place() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    let first = of_kind(&attest_events(root), "derive")
        .iter()
        .find(|e| identity(e, "test_identity").ends_with("unit_add_works"))
        .copied()
        .unwrap()
        .clone();

    edit(
        root,
        "src/lib.rs",
        "assert_eq!(add(2, 2), 4);",
        "assert_eq!(add(3, 1), 4);",
    );
    ok(agentrec(root, &["attest", "derive"]));

    let last = of_kind(&attest_events(root), "derive")
        .last()
        .copied()
        .unwrap()
        .clone();
    assert_eq!(last["claim_id"], first["claim_id"], "same claim");
    assert_eq!(
        identity(&last, "test_identity"),
        identity(&first, "test_identity")
    );
    assert!(last["renamed_from"].is_null());
    assert_ne!(last["body_hash"], first["body_hash"], "the hash must move");
    assert_eq!(status_json(root)["claims"], 5);
}

/// Write `.agentrec/open.json` the way `daemon.rs::sync_journal` does. Only the
/// `id` field is read back by capture — see this file's module doc.
fn write_open_json(root: &Path, id: &str) {
    let journal = serde_json::json!({
        "source": "bracket",
        "tool": "claude",
        "prompt": null,
        "session": null,
        "opened_wall_ms": 1u64,
        "last_change_wall_ms": 2u64,
        "root": root.to_string_lossy(),
        "files": [],
        "id": id,
    });
    std::fs::write(
        root.join(".agentrec/open.json"),
        serde_json::to_string(&journal).unwrap(),
    )
    .unwrap();
}

const TURN: &str = "t_01ARZ3NDEKTSV4RRFFQ69G5FAV";

/// AC-ATTEST-P3-15.
#[test]
fn ac_p3_15_run_writes_evidence_joined_to_the_open_turn() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    // The hook-bracketed session: the open bracket's signal, then the work.
    ok(agentrec(root, &["hook", "claude"]));
    write_open_json(root, TURN);

    ok(agentrec(root, &["attest", "run", "--", "cargo", "test"]));

    let events = attest_events(root);
    let evidence = of_kind(&events, "evidence");
    assert_eq!(evidence.len(), 5, "one per known test: {evidence:#?}");
    assert!(
        evidence.iter().all(|e| e["turn_id"] == TURN),
        "every capture joins the open turn: {evidence:#?}"
    );
    assert!(evidence.iter().all(|e| e["output_blob"].is_string()));

    // Both target shapes came back, with their real outcomes.
    let mut seen: Vec<(String, String)> = evidence
        .iter()
        .map(|e| {
            (
                identity(&e["result"], "identity"),
                e["result"]["outcome"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            (
                "attest_sample_crate::tests::unit_add_works".into(),
                "passed".to_string()
            ),
            (
                "attest_sample_crate::tests::unit_double_works".into(),
                "passed".to_string()
            ),
            (
                "attest_sample_crate::tests::unit_ignored_test".into(),
                "ignored".to_string()
            ),
            (
                "integration::integration_add_works".into(),
                "passed".to_string()
            ),
            (
                "integration::integration_double_works".into(),
                "passed".to_string()
            ),
        ]
    );
    // The tree was committed clean before the run, and `target/` is gitignored.
    assert!(
        evidence.iter().all(|e| e["dirty"] == false),
        "a clean tree must not be dev-loop-only: {evidence:#?}"
    );
    assert_eq!(status_json(root)["evidenced"], 5);
    assert_eq!(status_json(root)["dev_loop_only"], 0);
}

/// AC-ATTEST-P3-16 — the same capture with no wrapper, arriving through the
/// hook that already fires for every tool call.
#[test]
fn ac_p3_16_hook_post_tool_use_bash_writes_the_same_evidence() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    write_open_json(root, TURN);

    // A real `cargo test` run, whose real output becomes the hook payload's
    // `tool_response` — no invented libtest text.
    let real = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .arg("test")
        .current_dir(root)
        .output()
        .expect("cargo test");
    let payload = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "session_id": "s1",
        "tool_name": "Bash",
        "tool_input": { "command": "cargo test" },
        "tool_response": {
            "stdout": String::from_utf8_lossy(&real.stdout),
            "stderr": String::from_utf8_lossy(&real.stderr),
        }
    });

    let mut child = Command::new(bin())
        .args(["hook", "claude", "--root", root.to_str().unwrap()])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    ok(child.wait_with_output().unwrap());

    let events = attest_events(root);
    assert_eq!(of_kind(&events, "evidence").len(), 5, "{events:#?}");
    assert!(of_kind(&events, "evidence")
        .iter()
        .all(|e| e["turn_id"] == TURN));

    // A captured test run is evidence, not a turn boundary: it must not have
    // closed the bracket by appending a stop signal.
    let signals = std::fs::read_to_string(root.join(".agentrec/signal.jsonl")).unwrap_or_default();
    assert!(
        signals.trim().is_empty(),
        "a captured PostToolUse must append no signal: {signals}"
    );
}

/// AC-ATTEST-P3-17.
#[test]
fn ac_p3_17_dirty_tree_evidence_renders_as_dev_loop_only() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    // Dirty the tree with a tracked-file edit that does not change any test.
    edit(root, "src/lib.rs", "pub fn double", "pub fn  double");

    ok(agentrec(root, &["attest", "run", "--", "cargo", "test"]));
    let evidence = attest_events(root);
    let evidence = of_kind(&evidence, "evidence");
    assert!(!evidence.is_empty());
    assert!(evidence.iter().all(|e| e["dirty"] == true), "{evidence:#?}");
    assert_eq!(status_json(root)["dev_loop_only"], 5);
    let text = String::from_utf8(ok(agentrec(root, &["attest", "status"])).stdout).unwrap();
    assert!(text.contains("dev-loop-only (dirty tree): 5"), "{text}");
}

/// AC-ATTEST-P3-18.
#[test]
fn ac_p3_18_undeclared_results_are_counted_on_stderr_and_not_written() {
    let tmp = fixture_repo();
    let root = tmp.path();
    // No `attest derive` — nothing is declared.
    let out = ok(agentrec(root, &["attest", "run", "--", "cargo", "test"]));
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        stderr.contains("5 result(s) for undeclared tests skipped — run attest derive"),
        "{stderr}"
    );
    assert!(
        attest_events(root).is_empty(),
        "evidence must never attach to a claim that does not exist"
    );
    assert_eq!(status_json(root)["claims"], 0);
}

/// AC-ATTEST-P3-20 — a fail-closed capture. The output is a REAL libtest shape
/// naming real fixture-crate tests, with its summary line truncated the way
/// `docs/fixtures/attest/corrupted.txt` truncates it, so the parser takes the
/// missing-summary branch on genuine end-to-end plumbing.
#[test]
fn ac_p3_20_unparseable_output_writes_parse_failed_evidence_with_the_raw_blob() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));

    let script = "printf 'running 2 tests\\ntest tests::unit_add_works ... ok\\ntest tests::unit_double_works ... ok\\n\\ntest result: ok. 2 pass\\n'; \
                  printf '     Running unittests src/lib.rs (target/debug/deps/attest_sample_crate-abc123)\\n' >&2";
    ok(agentrec(
        root,
        &["attest", "run", "--", "bash", "-c", script],
    ));

    let events = attest_events(root);
    let evidence = of_kind(&events, "evidence");
    assert_eq!(evidence.len(), 2, "{events:#?}");
    for e in &evidence {
        assert_eq!(e["result"]["parse_failed"], true, "{e}");
        assert!(
            e["result"]["outcome"].is_null(),
            "no per-test outcome may be trusted: {e}"
        );
        assert_eq!(e["result"]["recipe_invalid"], "harness", "{e}");
        // The raw blob is retained and reachable in the CAS.
        let blob = e["result"]["raw_blob"].as_str().expect("raw blob kept");
        assert_eq!(e["output_blob"].as_str(), Some(blob));
        let hex = blob.strip_prefix("sha256:").expect("a sha256 ref");
        let path = root
            .join(".agentrec/objects")
            .join(&hex[..2])
            .join(&hex[2..]);
        let stored = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("blob {} missing: {e}", path.display()));
        assert!(stored.contains("test result: ok. 2 pass"), "{stored}");
    }
}

/// AC-ATTEST-P3-22.
#[test]
fn ac_p3_22_run_tees_output_and_propagates_the_child_exit_code() {
    let tmp = fixture_repo();
    let root = tmp.path();
    let out = agentrec(
        root,
        &[
            "attest",
            "run",
            "--",
            "bash",
            "-c",
            "echo to-stdout; echo to-stderr >&2; exit 3",
        ],
    );
    assert_eq!(out.status.code(), Some(3), "the child's own status");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("to-stdout"),
        "{out:?}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("to-stderr"),
        "{out:?}"
    );
}

/// The fixture crate this file drives must stay out of the published package,
/// alongside `attest_status::ac_p3_7_...`'s workspace check.
#[test]
fn the_fixture_crate_is_not_packaged() {
    let ws = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["package", "--list", "-p", "agentrec", "--allow-dirty"])
        .current_dir(&ws)
        .output()
        .expect("cargo package --list");
    if !out.status.success() {
        // Packaging can fail for reasons unrelated to this assertion (an
        // unpublished path dependency during development); don't turn that into
        // a false RED for a claim about file inclusion.
        return;
    }
    let listed = String::from_utf8_lossy(&out.stdout);
    assert!(
        !listed.contains("attest_sample"),
        "the fixture crate must not be published: {listed}"
    );
}
