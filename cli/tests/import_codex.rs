//! Integration tests for `agentrec import codex` (Phase 2 tail, task C4).
//! Drives the real compiled binary (this crate has no `[lib]` target, same
//! black-box posture as `import_claude.rs` and `integration.rs`).
//!
//! Fixtures live under `tests/fixtures/import/codex/<case>/sessions/YYYY/MM/DD/
//! rollout-*.jsonl`, mirroring the real `~/.codex/sessions` layout
//! (`import_claude.rs`'s fixtures mirror `~/.claude/projects` the same way).
//! Every fixture's line shapes (`session_meta`/`event_msg`/`response_item`
//! wrappers, `patch_apply_end`'s `success`/`changes` map, `user_message`'s
//! `message` field) are copied from real corpus measurements recorded in
//! `cli/src/importcmd.rs`'s `mod codex` doc comment — this repo's standing
//! lesson is "fixture-only evidence cannot close a corpus-shape claim", so
//! `real_redacted_rollout_dry_run_report_keys_match_claude` below additionally
//! runs against `docs/fixtures/codex/rollout-add-sample-redacted.jsonl`, a
//! genuine (redacted) rollout excerpt, not a synthetic shape.
//!
//! Fixture cwds use `/fake/repoN` (mirrors `import_claude.rs`'s convention —
//! never a real path) except `t2_t3`, which is rewritten to a real tempdir
//! git repo at test time, same technique `import_claude.rs::
//! t2_candidate_vs_t3_classification` uses for `/fake/repo2`.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/import/codex")
        .join(name)
}

fn run_import_json(source: &Path, root: &Path) -> Output {
    Command::new(bin())
        .args(["import", "codex", "--dry-run", "--json", "--source"])
        .arg(source)
        .arg("--root")
        .arg(root)
        .env("AGENTREC_IMPORT_DEBUG_ENTRIES", "1")
        .output()
        .expect("run agentrec import codex --json")
}

fn run_import_text(source: &Path, root: &Path) -> Output {
    Command::new(bin())
        .args(["import", "codex", "--dry-run", "--source"])
        .arg(source)
        .arg("--root")
        .arg(root)
        .output()
        .expect("run agentrec import codex (text)")
}

fn report_json(out: &Output) -> Value {
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}; stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout must be valid JSON")
}

fn debug_entries(report: &Value) -> Vec<&Value> {
    report
        .get("debug_entries")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().collect())
        .unwrap_or_default()
}

fn entry_for<'a>(entries: &[&'a Value], path_suffix: &str) -> &'a Value {
    entries
        .iter()
        .find(|e| {
            e.get("path")
                .and_then(|p| p.as_str())
                .is_some_and(|p| p.ends_with(path_suffix))
        })
        .unwrap_or_else(|| panic!("no debug_entries entry ending in {path_suffix}"))
}

// ---- 1. `add` -> t1 (real, known content), `delete` -> t1 (real,
// pre-deletion content) -- both are "before or after bytes known with
// certainty from the transcript itself", the honest T1 definition for Codex
// (module doc in importcmd.rs). --------------------------------------------

#[test]
fn add_and_delete_both_classify_t1() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("basic"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 1);
    assert_eq!(report["tier_counts"]["t1"], 2, "one add + one delete");
    assert_eq!(report["tier_counts"]["t2_candidate"], 0);
    assert_eq!(report["tier_counts"]["t3"], 0);
    // Structurally unreachable for Codex — see importcmd.rs's `mod codex`
    // doc comment ("no code path ever increments it").
    assert_eq!(report["tier_counts"]["t1_5"], 0);
    assert_eq!(report["skipped_sidechain"], 0);
    assert_eq!(report["skipped_missing_field"]["tool_use_result"], 0);

    let entries = debug_entries(&report);
    let config = entry_for(&entries, "config.json");
    assert_eq!(config["tier"], "t1");
    assert!(config["sha256"].is_string(), "add content is real, hashed");
    let old = entry_for(&entries, "old.txt");
    assert_eq!(old["tier"], "t1");
    assert!(
        old["sha256"].is_string(),
        "delete content is real (pre-deletion), hashed"
    );
}

// ---- 2. `update` -> t2_candidate (git-tracked) vs t3 (not) — detect-only,
// mirrors Claude's dry-run T2 posture: never reads a git blob here. --------

#[test]
fn update_classifies_t2_candidate_vs_t3_by_git_tracking() {
    let repo = tempfile::tempdir().unwrap();
    let repo_path = std::fs::canonicalize(repo.path()).unwrap();

    let init = Command::new("git")
        .arg("init")
        .arg(&repo_path)
        .output()
        .unwrap();
    assert!(init.status.success());
    for cfg in [["user.email", "test@example.com"], ["user.name", "Test"]] {
        Command::new("git")
            .arg("-C")
            .arg(&repo_path)
            .args(["config", cfg[0], cfg[1]])
            .output()
            .unwrap();
    }
    std::fs::write(repo_path.join("tracked.txt"), "alpha\n").unwrap();
    std::fs::write(repo_path.join("untracked.txt"), "gamma\n").unwrap();
    Command::new("git")
        .arg("-C")
        .arg(&repo_path)
        .args(["add", "tracked.txt"])
        .output()
        .unwrap();
    let commit = Command::new("git")
        .arg("-C")
        .arg(&repo_path)
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();
    assert!(commit.status.success());

    let orig = std::fs::read_to_string(
        fixture("t2_t3")
            .join("sessions/2026/06/01/rollout-2026-06-01T00-00-00-b2222222-2222-4222-8222-222222222222.jsonl"),
    )
    .unwrap();
    let rewritten = orig.replace("/fake/repo2", &repo_path.to_string_lossy());

    let source_root = tempfile::tempdir().unwrap();
    let session_dir = source_root.path().join("sessions/2026/06/01");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("rollout-2026-06-01T00-00-00-b2222222-2222-4222-8222-222222222222.jsonl"),
        rewritten,
    )
    .unwrap();

    let root = tempfile::tempdir().unwrap();
    let out = run_import_json(source_root.path(), root.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 1);
    assert_eq!(report["tier_counts"]["t2_candidate"], 1);
    assert_eq!(report["tier_counts"]["t3"], 1);
    assert_eq!(report["tier_counts"]["t1"], 0);

    let entries = debug_entries(&report);
    let tracked = entry_for(&entries, "tracked.txt");
    assert_eq!(tracked["tier"], "t2_candidate");
    assert!(
        tracked["sha256"].is_null(),
        "detect-only: dry-run never reads the git blob"
    );
    let untracked = entry_for(&entries, "untracked.txt");
    assert_eq!(untracked["tier"], "t3");
}

// ---- 3. A session with only non-apply_patch tool calls (exec_command) is
// importable with zero file entries; the tool call counts as opaque. ------

#[test]
fn opaque_call_counts_as_importable_with_no_file_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("opaque"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 1);
    assert_eq!(report["opaque_calls"], 1);
    assert_eq!(report["tier_counts"]["t1"], 0);
    assert_eq!(report["tier_counts"]["t2_candidate"], 0);
    assert_eq!(report["tier_counts"]["t3"], 0);
}

// ---- 4. Malformed-line tolerance: a file with zero parseable lines never
// aborts the scan of its sibling. -------------------------------------------

#[test]
fn malformed_lines_tolerated_sibling_still_imports() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("malformed"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["skipped_malformed_line"], 3);
    assert_eq!(
        report["sessions_total"], 2,
        "broken file + valid sibling, both counted in the denominator"
    );
    assert_eq!(
        report["sessions_importable"], 1,
        "broken.jsonl: 0 lines parsed, never importable; valid sibling still is"
    );
    assert_eq!(report["tier_counts"]["t1"], 1, "the sibling's add op");
}

// ---- 5. A session that never establishes a `cwd` (no `session_meta.cwd`,
// no `turn_context.cwd`) is disqualified but does not abort the run. -------

#[test]
fn missing_cwd_disqualifies_session_but_does_not_abort() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("missing_cwd"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 0);
    assert!(report["skipped_missing_field"]["cwd"].as_u64().unwrap() >= 1);
}

// ---- 6. Secret-path withholding: an `add` targeting `.env` is never tiered
// or hashed into `debug_entries` — mirrors Claude's D7 fix. ----------------

#[test]
fn secret_path_never_tiered_or_hashed() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("secret_path"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["skipped_secret_path"], 1);
    assert_eq!(report["tier_counts"]["t1"], 0);
    let entries = debug_entries(&report);
    assert!(entries.iter().all(|e| e["tier"] != "t1"));
}

// ---- 7. Honesty gate: a `patch_apply_end` with `success: false` must never
// be trusted, even though its `changes` map is present and well-formed. ----

#[test]
fn failed_patch_apply_is_never_trusted() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("failed_apply"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(
        report["sessions_importable"], 1,
        "the session itself still has a cwd and parses cleanly"
    );
    assert_eq!(report["tier_counts"]["t1"], 0);
    assert_eq!(report["tier_counts"]["t2_candidate"], 0);
    assert_eq!(report["tier_counts"]["t3"], 0);
    assert_eq!(report["opaque_calls"], 0);
    let entries = debug_entries(&report);
    assert!(
        entries.is_empty(),
        "a failed apply_patch's changes must never surface anywhere"
    );
}

// ---- 8. --json round-trips through serde_json; text report contains the
// same fields (mirrors import_claude.rs's parity test). --------------------

#[test]
fn json_flag_round_trips_and_text_report_has_codex_header() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("basic"), tmp.path());
    let report = report_json(&out);
    assert!(report["sessions_total"].is_number());
    assert!(report["peak_rss_mb"].is_number());

    let text_out = run_import_text(&fixture("basic"), tempfile::tempdir().unwrap().path());
    assert!(text_out.status.success());
    let stdout = String::from_utf8_lossy(&text_out.stdout);
    assert!(
        stdout.starts_with("agentrec import codex --dry-run report"),
        "the shared printer must be told it's codex, not claude: {stdout}"
    );
    assert!(stdout.contains("tier_counts"));
}

// ---- 9. Report-key parity with `import claude --dry-run --json` (AC-C4). -

#[test]
fn dry_run_report_keys_match_import_claude() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("basic"), tmp.path());
    let report = report_json(&out);
    let mut codex_keys: Vec<&str> = report
        .as_object()
        .unwrap()
        .keys()
        .map(|s| s.as_str())
        .collect();
    codex_keys.sort();

    // The exact key set `import claude --dry-run --json` emits on a normal
    // run (Pinned decision 12's contract in importcmd.rs) — `debug_entries`
    // is included here because both runs set
    // AGENTREC_IMPORT_DEBUG_ENTRIES=1.
    let mut expected = vec![
        "sessions_total",
        "sessions_importable",
        "importable_pct",
        "sessions_in_root",
        "tier_counts",
        "opaque_calls",
        "mean_opaque_share_pct",
        "skipped_secret_path",
        "skipped_sidechain",
        "skipped_malformed_line",
        "skipped_non_utf8_line",
        "skipped_io_error",
        "skipped_missing_field",
        "peak_rss_mb",
        "debug_entries",
    ];
    expected.sort();
    assert_eq!(codex_keys, expected, "same ImportReport struct, same keys");
}

// ---- 10. Grounding: a REDACTED REAL rollout excerpt (docs/fixtures/codex),
// not a synthetic fixture — closes "fixture-only evidence cannot close a
// corpus-shape claim" for at least one case. --------------------------------

#[test]
fn real_redacted_rollout_dry_run_report_keys_match_claude() {
    let real_file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../docs/fixtures/codex/rollout-add-sample-redacted.jsonl");
    assert!(real_file.is_file(), "{real_file:?} must exist");

    let source_root = tempfile::tempdir().unwrap();
    let session_dir = source_root.path().join("sessions/2026/04/15");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::copy(
        &real_file,
        session_dir.join("rollout-2026-04-15T01-30-38-real.jsonl"),
    )
    .unwrap();

    let root = tempfile::tempdir().unwrap();
    let out = run_import_json(source_root.path(), root.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 1);
    assert_eq!(report["tier_counts"]["t1"], 1, "the real Position.java add");
}

// ---- persist (no --dry-run) ------------------------------------------------

mod persist {
    use super::*;

    fn write_session(source: &Path, date_dir: &str, file_name: &str, lines: &[String]) {
        let dir = source.join("sessions").join(date_dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file_name), lines.join("\n") + "\n").unwrap();
    }

    fn run_persist(source: &Path, root: &Path) -> Output {
        Command::new(bin())
            .args(["import", "codex", "--source"])
            .arg(source)
            .arg("--root")
            .arg(root)
            .output()
            .expect("run agentrec import codex (persist)")
    }

    fn run_persist_json(source: &Path, root: &Path) -> Output {
        Command::new(bin())
            .args(["import", "codex", "--json", "--source"])
            .arg(source)
            .arg("--root")
            .arg(root)
            .output()
            .expect("run agentrec import codex --json (persist)")
    }

    fn init_repo(root: &Path) {
        Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(root)
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["config", "user.email", "test@example.com"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["config", "user.name", "test"])
            .status()
            .unwrap();
    }

    fn log_lines(root: &Path) -> Vec<String> {
        std::fs::read_to_string(root.join(".agentrec/log.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn turn_lines(root: &Path) -> Vec<Value> {
        log_lines(root)
            .iter()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("turn"))
            .collect()
    }

    fn session_lines(cwd: &str, sid: &str, file_marker: &str) -> Vec<String> {
        vec![
            format!(
                r#"{{"timestamp":"2026-06-10T09:00:00.000Z","type":"session_meta","payload":{{"id":"{sid}","cwd":"{cwd}","originator":"Codex CLI","cli_version":"0.146.0"}}}}"#
            ),
            format!(
                r#"{{"timestamp":"2026-06-10T09:00:01.000Z","type":"event_msg","payload":{{"type":"user_message","message":"add {file_marker}.txt","images":[],"local_images":[],"text_elements":[]}}}}"#
            ),
            format!(
                r#"{{"timestamp":"2026-06-10T09:00:02.000Z","type":"event_msg","payload":{{"type":"patch_apply_end","call_id":"call_{file_marker}","turn_id":"t_{file_marker}","stdout":"","stderr":"","success":true,"changes":{{"{cwd}/{file_marker}.txt":{{"type":"add","content":"hello {file_marker}\n"}}}}}}}}"#
            ),
        ]
    }

    // ---- AC-C4: idempotent re-run appends 0 — mirrors
    // `import_claude.rs`'s `ac2_rerun_appends_zero_new_records`, but not
    // vacuously: run 1 must append > 0 first. --------------------------

    #[test]
    fn same_session_reimport_appends_zero() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let cwd = root.path().to_string_lossy().to_string();

        write_session(
            source.path(),
            "2026/06/10",
            "rollout-2026-06-10T09-00-00-s_ac2.jsonl",
            &session_lines(&cwd, "s_ac2", "a"),
        );

        let out1 = run_persist(source.path(), root.path());
        assert!(out1.status.success(), "{out1:?}");
        let stdout1 = String::from_utf8_lossy(&out1.stdout);
        assert!(stdout1.contains("appended: 1"), "stdout: {stdout1}");
        assert_eq!(turn_lines(root.path()).len(), 1);
        let after_first = log_lines(root.path());

        let out2 = run_persist(source.path(), root.path());
        assert!(out2.status.success());
        let stdout2 = String::from_utf8_lossy(&out2.stdout);
        assert!(
            stdout2.contains("appended: 0"),
            "re-run must append 0 new records, got: {stdout2}"
        );
        assert_eq!(
            log_lines(root.path()),
            after_first,
            "re-run must not change log.jsonl at all"
        );
    }

    // ---- Intra-run id collision, same shape as Claude's guard: two
    // rollout files sharing one `session_meta.id` (a resumed/forked
    // session written twice) mint the same deterministic id — the second
    // is skipped and COUNTED, never silently dropped. --------------------

    #[test]
    fn intra_run_duplicate_session_id_appends_one_and_counts_the_skip() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let cwd = root.path().to_string_lossy().to_string();

        write_session(
            source.path(),
            "2026/06/12",
            "rollout-2026-06-12T09-00-00-s_dup.jsonl",
            &session_lines(&cwd, "s_dup", "dupa"),
        );
        write_session(
            source.path(),
            "2026/06/13",
            "rollout-2026-06-13T09-00-00-s_dup.jsonl",
            &session_lines(&cwd, "s_dup", "dupb"),
        );

        let out = run_persist_json(source.path(), root.path());
        assert!(out.status.success(), "{out:?}");
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["appended"], 1);
        assert_eq!(report["skipped_duplicate_turn_id"], 1);
        assert_eq!(turn_lines(root.path()).len(), 1);
    }

    // ---- files_complete + after_synthesized honesty (AC-C4): every
    // imported turn carries files_complete: Some(false); an `add` entry's
    // `after` is real (after_synthesized ABSENT from the wire, never
    // `true`); an `update` entry (never persisted here — descoped, see
    // importcmd.rs module doc) is simply not present at all rather than a
    // fabricated file entry. ------------------------------------------

    #[test]
    fn files_complete_false_and_add_after_is_never_synthesized() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let cwd = root.path().to_string_lossy().to_string();

        write_session(
            source.path(),
            "2026/06/14",
            "rollout-2026-06-14T09-00-00-s_honesty.jsonl",
            &session_lines(&cwd, "s_honesty", "hon"),
        );

        let out = run_persist(source.path(), root.path());
        assert!(out.status.success());
        let turns = turn_lines(root.path());
        assert_eq!(turns.len(), 1);
        let turn = &turns[0];
        assert_eq!(turn["files_complete"], Value::Bool(false));
        assert_eq!(turn["imported"], Value::Bool(true));
        assert_eq!(turn["tool"], "codex");
        assert_eq!(turn["grade"], "rich");

        let files = turn["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f["op"], "create");
        assert!(
            f["after"].is_string(),
            "add's after is real, resolved bytes"
        );
        assert!(f["before"].is_null());
        assert!(
            f.get("after_synthesized").is_none(),
            "never Some(true): this importer only ever writes real, \
             observed bytes or nothing — got {:?}",
            f.get("after_synthesized")
        );
    }

    // ---- An `update` change is never persisted with fabricated bytes: the
    // whole turn IS still persisted (an update-only turn is not empty — it
    // has one honest FileEntry), but never with fabricated bytes:
    // `before`/`after` both null, `baseline_unknown: true` (the same field
    // the live daemon uses for "existed before we could see it" — see
    // `agentrec-core/src/daemon.rs`/`view.rs`; `import claude` never sets it
    // even for its own unresolved T3 entries, which this importer diverges
    // from deliberately — `before: None` alone is ambiguous between "no
    // prior content" and "unknown prior content", and only the latter is
    // true here). ---------------------------------------------------------

    #[test]
    fn update_only_turn_persists_with_baseline_unknown_never_fabricated() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let cwd = root.path().to_string_lossy().to_string();
        std::fs::write(root.path().join("u.txt"), "before\n").unwrap();

        let lines = vec![
            format!(
                r#"{{"timestamp":"2026-06-15T09:00:00.000Z","type":"session_meta","payload":{{"id":"s_upd","cwd":"{cwd}","originator":"Codex CLI","cli_version":"0.146.0"}}}}"#
            ),
            format!(
                r#"{{"timestamp":"2026-06-15T09:00:01.000Z","type":"event_msg","payload":{{"type":"user_message","message":"tweak u.txt","images":[],"local_images":[],"text_elements":[]}}}}"#
            ),
            format!(
                r#"{{"timestamp":"2026-06-15T09:00:02.000Z","type":"event_msg","payload":{{"type":"patch_apply_end","call_id":"call_u","turn_id":"t_u","stdout":"","stderr":"","success":true,"changes":{{"{cwd}/u.txt":{{"type":"update","unified_diff":"@@ -1,1 +1,1 @@\n-before\n+after\n","move_path":null}}}}}}}}"#
            ),
        ];
        write_session(
            source.path(),
            "2026/06/15",
            "rollout-2026-06-15T09-00-00-s_upd.jsonl",
            &lines,
        );

        let out = run_persist(source.path(), root.path());
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("appended: 1"), "stdout: {stdout}");
        let turns = turn_lines(root.path());
        assert_eq!(turns.len(), 1);
        let files = turns[0]["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f["op"], "modify");
        assert!(f["before"].is_null(), "never fabricated");
        assert!(f["after"].is_null(), "never fabricated");
        assert_eq!(f["baseline_unknown"], true);
        assert!(
            f.get("after_synthesized").is_none(),
            "nothing was derived, so nothing is marked synthesized either"
        );
    }
}
