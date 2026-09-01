//! Phase 3A integration tests: `agentrec attest status` against the real
//! binary, plus the concurrent-append proof for `cli/src/attest/lock.rs`.
//! ACs: `IMPLEMENTATION.md` § "Phase 3A".

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures; there is no attacker-supplied
//    FIFO to block on. Production reads stay lint-enforced (clippy.toml).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

fn agentrec(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .args(["--root", root.to_str().unwrap()])
        .output()
        .expect("run agentrec")
}

fn init(root: &Path) {
    let out = agentrec(root, &["init", "--no-hook", "--no-service"]);
    assert!(out.status.success(), "init failed: {out:?}");
}

fn status_text(root: &Path) -> String {
    let out = agentrec(root, &["attest", "status"]);
    assert!(out.status.success(), "attest status failed: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

fn status_json(root: &Path) -> serde_json::Value {
    let out = agentrec(root, &["attest", "status", "--json"]);
    assert!(out.status.success(), "attest status --json failed: {out:?}");
    serde_json::from_slice(&out.stdout).expect("stdout must be one JSON object")
}

fn write_log(root: &Path, lines: &[&str]) {
    let path = root.join(".agentrec").join("attest.jsonl");
    std::fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
}

/// Two claim ids, valid `c_` + 26 Crockford chars.
const A: &str = "c_00000000010W3GE1R70W3GE1R7";
const B: &str = "c_00000000010W3GE1R70W3GE1R8";
const C: &str = "c_00000000010W3GE1R70W3GE1R9";

fn derive(id: &str, fn_path: &str) -> String {
    format!(
        r#"{{"kind":"derive","ts":1,"claim_id":"{id}","test_identity":{{"target":"tgt","fn_path":"{fn_path}"}}}}"#
    )
}

fn evidence(id: &str, ts: u64, dirty: bool, fn_path: &str) -> String {
    format!(
        r#"{{"kind":"evidence","ts":{ts},"claim_id":"{id}","turn_id":null,"dirty":{dirty},"output_blob":null,"result":{{"identity":{{"target":"tgt","fn_path":"{fn_path}"}},"outcome":"passed","recipe_invalid":null,"parse_failed":false,"raw_blob":null}}}}"#
    )
}

/// AC-ATTEST-P3-1.
#[test]
fn ac_p3_1_status_on_a_fresh_root_is_all_zeros_and_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    init(tmp.path());
    assert!(
        !tmp.path().join(".agentrec/attest.jsonl").exists(),
        "fixture precondition: init must not create attest.jsonl"
    );

    let text = status_text(tmp.path());
    for line in [
        "claims: 0",
        "  DERIVED: 0",
        "  CONFIRMED: 0",
        "stale (overlay): 0",
        "dev-loop-only (dirty tree): 0",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
    // Nothing to tolerate, so neither tolerance line is printed.
    assert!(!text.contains("unparsed"), "{text}");
    assert!(!text.contains("unknown-kind"), "{text}");

    let json = status_json(tmp.path());
    for (key, _) in json.as_object().unwrap() {
        assert_eq!(json[key], 0, "{key} must be zero on a fresh root");
    }
}

/// AC-ATTEST-P3-2 and AC-ATTEST-P3-3.
#[test]
fn ac_p3_2_counts_by_status_and_stale_overlay_match_the_folded_log() {
    let tmp = tempfile::tempdir().unwrap();
    init(tmp.path());
    write_log(
        tmp.path(),
        &[
            // A: derive -> evidence -> stale -> verdict(confirmed).
            // The confirmed verdict drops the overlay (ATTEST-FORMAT.md).
            &derive(A, "a"),
            &evidence(A, 2, false, "a"),
            &format!(
                r#"{{"kind":"stale","ts":3,"claim_id":"{A}","cause":"file-write","path":"cli/src/x.rs"}}"#
            ),
            &format!(
                r#"{{"kind":"verdict","ts":4,"claim_id":"{A}","verdict":"confirmed","replay_commit":"abc"}}"#
            ),
            // B: derived, then staled and left stale.
            &derive(B, "b"),
            &format!(
                r#"{{"kind":"stale","ts":6,"claim_id":"{B}","cause":"coverage-incomplete","scope":"cli/src/**"}}"#
            ),
            // C: a manual criterion, never answered.
            &format!(
                r#"{{"kind":"manual-declare","ts":7,"claim_id":"{C}","text":"README is right","severity":"blocking"}}"#
            ),
        ],
    );

    let text = status_text(tmp.path());
    for line in [
        "claims: 3",
        "  DERIVED: 1",
        "  DECLARED: 1",
        "  EVIDENCED: 0",
        "  CONFIRMED: 1",
        "  CLAIM_FALSE: 0",
        "  RECIPE_INVALID: 0",
        "  FLAKY: 0",
        "  HUMAN: 0",
        "stale (overlay): 1",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }

    // AC-ATTEST-P3-3: every text figure has a matching JSON key.
    let json = status_json(tmp.path());
    assert_eq!(json["claims"], 3);
    assert_eq!(json["derived"], 1);
    assert_eq!(json["declared"], 1);
    assert_eq!(json["evidenced"], 0);
    assert_eq!(json["confirmed"], 1);
    assert_eq!(json["claim_false"], 0);
    assert_eq!(json["recipe_invalid"], 0);
    assert_eq!(json["flaky"], 0);
    assert_eq!(json["human"], 0);
    assert_eq!(json["stale"], 1);
    assert_eq!(json["dev_loop_only"], 0);
    assert_eq!(json["unparsed_lines"], 0);
    assert_eq!(json["unknown_kind_lines"], 0);
}

/// AC-ATTEST-P3-4.
#[test]
fn ac_p3_4_malformed_and_unknown_kind_lines_are_surfaced_and_not_fatal() {
    let tmp = tempfile::tempdir().unwrap();
    init(tmp.path());
    write_log(
        tmp.path(),
        &[
            &derive(A, "a"),
            // A known kind written malformed: damage, not a newer writer.
            r#"{"kind":"verdict","ts":9}"#,
            // A kind this binary does not know: tolerated, counted.
            r#"{"kind":"future-kind","ts":9,"claim_id":"c_00000000010W3GE1R70W3GE1R7"}"#,
            &derive(B, "b"),
        ],
    );

    let out = agentrec(tmp.path(), &["attest", "status"]);
    assert!(out.status.success(), "must exit 0: {out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("1 unparsed line(s)"), "{text}");
    assert!(text.contains("1 unknown-kind line(s)"), "{text}");
    // The surrounding events still folded.
    assert!(text.contains("claims: 2"), "{text}");

    let json = status_json(tmp.path());
    assert_eq!(json["unparsed_lines"], 1);
    assert_eq!(json["unknown_kind_lines"], 1);
    assert_eq!(json["claims"], 2);
}

/// AC-ATTEST-P3-8.
#[test]
fn ac_p3_8_dirty_latest_evidence_is_counted_as_dev_loop_only() {
    let tmp = tempfile::tempdir().unwrap();
    init(tmp.path());
    write_log(
        tmp.path(),
        &[
            &derive(A, "a"),
            &evidence(A, 2, true, "a"),
            &derive(B, "b"),
            &evidence(B, 3, true, "b"),
            // B's later clean run takes it back out of the figure.
            &evidence(B, 4, false, "b"),
        ],
    );

    let text = status_text(tmp.path());
    assert!(text.contains("dev-loop-only (dirty tree): 1"), "{text}");
    assert_eq!(status_json(tmp.path())["dev_loop_only"], 1);
    assert_eq!(status_json(tmp.path())["evidenced"], 2);
}

// AC-ATTEST-P3-5 (concurrent appenders) lives in
// `cli/src/attest/lock.rs::tests::
// ac_p3_5_concurrent_appenders_leave_no_torn_lines_phase4_concurrent_append_ac`
// and NOT here: `append_attest_locked` is in the `agentrec` binary crate,
// which has no lib target for an integration test to link against, and no CLI
// verb appends to `attest.jsonl` yet (chunk B's `attest derive`/`run` are the
// first). An in-crate unit test is the only place that can call the function
// under test directly.

/// AC-ATTEST-P3-7.
#[test]
fn ac_p3_7_fixture_crate_is_absent_from_the_parent_workspace() {
    let ws = workspace_root();
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(&ws)
        .output()
        .expect("cargo metadata");
    assert!(out.status.success(), "{out:?}");
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = meta["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        !names.contains(&"attest_sample_crate"),
        "fixture crate must be excluded from the parent workspace, got {names:?}"
    );
    assert!(names.contains(&"agentrec"), "sanity: {names:?}");
}

/// AC-ATTEST-P3-9.
#[test]
fn ac_p3_9_fixture_crate_has_both_test_target_shapes() {
    let dir = fixture_crate();
    let lib = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();
    assert!(lib.contains("#[cfg(test)]\nmod tests {"), "unit-test shape");
    assert_eq!(lib.matches("#[test]").count(), 3);
    assert!(lib.contains("#[ignore]"));

    let it = std::fs::read_to_string(dir.join("tests/integration.rs")).unwrap();
    assert_eq!(it.matches("#[test]").count(), 2);

    // Standalone: its own `[workspace]` table, so cargo does not walk up.
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("\n[workspace]\n"), "{manifest}");
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `cli/`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn fixture_crate() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/attest_sample_crate")
}
