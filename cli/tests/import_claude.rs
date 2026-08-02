//! Integration tests for `agentrec import claude --dry-run` (P1). Drives the
//! real compiled binary (this crate has no `[lib]` target, so — matching
//! `integration.rs`'s existing pattern — internals are exercised only through
//! process stdout/stderr/exit code, never a direct function call).
//!
//! `AGENTREC_IMPORT_DEBUG_ENTRIES=1` (debug builds only, see
//! `importcmd.rs::debug_dump_entries_enabled`) makes `--json` output include a
//! `debug_entries` array with per-entry `{session_file, path, tier, sha256}` —
//! the only way this black-box test can assert tier + resolved-bytes hashes
//! without loosening the production JSON contract (Pinned decision 12), which
//! omits that field entirely whenever it's empty (i.e. every normal run).

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/import/claude")
        .join(name)
}

/// Runs `agentrec import claude --dry-run --source <fixture> --root <root>`,
/// with debug per-entry dump enabled so tests can inspect tier/hash detail.
fn run_import_json(source: &Path, root: &Path) -> Output {
    Command::new(bin())
        .args(["import", "claude", "--dry-run", "--json", "--source"])
        .arg(source)
        .arg("--root")
        .arg(root)
        .env("AGENTREC_IMPORT_DEBUG_ENTRIES", "1")
        .output()
        .expect("run agentrec import claude --json")
}

fn run_import_text(source: &Path, root: &Path) -> Output {
    Command::new(bin())
        .args(["import", "claude", "--dry-run", "--source"])
        .arg(source)
        .arg("--root")
        .arg(root)
        .output()
        .expect("run agentrec import claude (text)")
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

// ---- 1. AC4: T1 and T1.5 resolve to the known-good, independently-hashed
// pre-edit content (fixture `t1_and_t15/`). --------------------------------

#[test]
fn t1_and_t15_resolve_with_matching_sha256() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("t1_and_t15"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 1);
    assert_eq!(report["tier_counts"]["t1"], 1);
    assert_eq!(report["tier_counts"]["t1_5"], 1);
    assert_eq!(report["tier_counts"]["t2_candidate"], 0);
    assert_eq!(report["tier_counts"]["t3"], 0);

    let entries = debug_entries(&report);
    let config = entry_for(&entries, "config.json");
    assert_eq!(config["tier"], "t1");
    assert_eq!(
        config["sha256"],
        "366a38a17f288f5fce4ed5d4f2e5a06c42152d29417473a91666d8ac1cac6b43"
    );

    let notes = entry_for(&entries, "notes.txt");
    assert_eq!(notes["tier"], "t1_5");
    assert_eq!(
        notes["sha256"],
        "e9024f1a07d29d52ad3aa5e1a18e94db1f3a9fd32b89e39d47c472cd99071e13"
    );
}

// ---- 2. T2-candidate vs T3 (fixture `t2_t3_candidate/`) — requires a real
// git repo standing in for `/fake/repo2`; the fixture's literal `/fake/repo2`
// paths are rewritten to the tempdir's real path in a temp copy of the
// transcript (FIXTURES.md's explicit instruction — the fixture itself is
// never modified). ---------------------------------------------------------

#[test]
fn t2_candidate_vs_t3_classification() {
    let repo = tempfile::tempdir().unwrap();
    let repo_path = std::fs::canonicalize(repo.path()).unwrap();

    let init = Command::new("git")
        .arg("init")
        .arg(&repo_path)
        .output()
        .expect("git init");
    assert!(init.status.success(), "git init must succeed");
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
    assert!(commit.status.success(), "git commit must succeed");

    // Rewrite the fixture's `/fake/repo2` to the real tempdir path in a
    // fresh source root mirroring the original `projects/proj-a/...` layout.
    let orig = std::fs::read_to_string(
        fixture("t2_t3_candidate")
            .join("projects/proj-a/b2222222-2222-4222-8222-222222222222.jsonl"),
    )
    .unwrap();
    let rewritten = orig.replace("/fake/repo2", &repo_path.to_string_lossy());

    let source_root = tempfile::tempdir().unwrap();
    let proj_dir = source_root.path().join("projects/proj-a");
    std::fs::create_dir_all(&proj_dir).unwrap();
    std::fs::write(
        proj_dir.join("b2222222-2222-4222-8222-222222222222.jsonl"),
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
    assert_eq!(report["tier_counts"]["t1_5"], 0);

    let entries = debug_entries(&report);
    let tracked = entry_for(&entries, "tracked.txt");
    assert_eq!(tracked["tier"], "t2_candidate");
    assert!(
        tracked["sha256"].is_null(),
        "T2-candidate must never report resolved bytes"
    );
    let untracked = entry_for(&entries, "untracked.txt");
    assert_eq!(untracked["tier"], "t3");
    assert!(untracked["sha256"].is_null());
}

// ---- 3. Opaque tool call (Bash), zero file entries, still importable -----

#[test]
fn opaque_call_counts_as_importable_with_no_file_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("opaque"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 1);
    assert_eq!(report["opaque_calls"], 1);
    assert_eq!(report["tier_counts"]["t1"], 0);
    assert_eq!(report["tier_counts"]["t1_5"], 0);
    assert_eq!(report["tier_counts"]["t2_candidate"], 0);
    assert_eq!(report["tier_counts"]["t3"], 0);
}

// ---- 4. AC5: dual sidechain exclusion (field in the top-level file, path
// segment in the nested subagents file) — both yield zero turns. -----------

#[test]
fn sidechain_excluded_by_field_and_path_yields_zero_turns() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("sidechain"), tmp.path());
    let report = report_json(&out);

    // Only the top-level file is in the AC1 denominator; the nested
    // subagents/agent-x1.jsonl is two levels deep and never counted here.
    assert_eq!(report["sessions_total"], 1);
    assert_eq!(
        report["sessions_importable"], 1,
        "completes cleanly, still importable"
    );
    assert_eq!(
        report["tier_counts"]["t1"], 0,
        "sidechain-excluded, never tiered"
    );
    assert_eq!(report["tier_counts"]["t1_5"], 0);
    assert_eq!(report["tier_counts"]["t2_candidate"], 0);
    assert_eq!(report["tier_counts"]["t3"], 0);
    // 1 field-excluded entry (top-level README.md) + 1 path-excluded entry
    // (subagents/agent-x1.jsonl has 2 lines but only one is file-producing —
    // its first line is a plain subagent prompt, no toolUseResult) = 2.
    assert_eq!(report["skipped_sidechain"], 2);
}

// ---- 5. AC6: every line malformed — one skip per line, exit 0, session not
// importable (zero lines parsed). A sibling valid file in the same project
// dir (added after review, item 8) proves the malformed file doesn't abort
// processing of the rest of the corpus. -------------------------------------

#[test]
fn malformed_lines_all_skip_exit_zero_and_session_not_importable() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("malformed"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["skipped_malformed_line"], 5);
    assert_eq!(
        report["sessions_total"], 2,
        "broken.jsonl + valid-sibling.jsonl"
    );
    assert_eq!(
        report["sessions_importable"], 1,
        "broken.jsonl: zero lines parsed, decision 3(b)'s carve-out doesn't apply; \
         valid-sibling.jsonl: parses cleanly and must still be counted — proving \
         the malformed file didn't abort the scan of its sibling"
    );
    assert_eq!(
        report["tier_counts"]["t1"], 1,
        "valid-sibling.jsonl's Edit entry must have actually been classified, \
         not merely counted importable"
    );
}

// ---- 6. AC8 (cwd branch): missing required field disqualifies the session
// but does not abort the run. Judgment call (P1.md leaves the exit code
// open): this repo exits 0 for the whole batch even when one session hits
// the AC8 path — schema drift on a single session is not a fatal error for
// the corpus scan (mirrors AC6's "doesn't abort" posture); the loud signal is
// the named counter (asserted below) + an stderr line, not a nonzero exit. --

#[test]
fn missing_cwd_disqualifies_session_but_does_not_abort() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("missing_cwd"), tmp.path());
    assert!(out.status.success(), "exit 0 per this repo's judgment call");
    let report = report_json(&out);

    assert!(report["skipped_missing_field"]["cwd"].as_u64().unwrap() >= 1);
    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 0);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("cwd"),
        "the missing field name must be named loudly, not silently dropped"
    );
}

// ---- 7. AC8 (toolUseResult branch): schema drift on the OTHER required
// field, distinct counter from #6, never conflated with AC6. ---------------

#[test]
fn missing_tool_use_result_disqualifies_session() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("missing_touluseresult"), tmp.path());
    assert!(out.status.success());
    let report = report_json(&out);

    assert!(
        report["skipped_missing_field"]["tool_use_result"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 0);
    assert_eq!(
        report["skipped_missing_field"]["cwd"], 0,
        "must not conflate the two AC8 branches"
    );
}

// ---- 8. --json round-trips cleanly through serde_json, and every field in
// Pinned decision 12's contract is present with the right shape. -----------

#[test]
fn json_flag_round_trips_through_serde_json() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(bin())
        .args(["import", "claude", "--dry-run", "--json", "--source"])
        .arg(fixture("t1_and_t15"))
        .arg("--root")
        .arg(tmp.path())
        .output()
        .expect("run agentrec import claude --json");
    let report = report_json(&out);

    assert!(report["sessions_total"].is_u64());
    assert!(report["sessions_importable"].is_u64());
    assert!(report["importable_pct"].is_number());
    assert!(report["sessions_in_root"].is_u64());
    assert!(report["tier_counts"]["t1"].is_u64());
    assert!(report["tier_counts"]["t1_5"].is_u64());
    assert!(report["tier_counts"]["t15_unverified"].is_u64());
    assert!(report["tier_counts"]["t15_rejected_unverifiable"].is_u64());
    assert!(report["tier_counts"]["t15_blob_missing"].is_u64());
    assert!(report["tier_counts"]["t2_candidate"].is_u64());
    assert!(report["tier_counts"]["t3"].is_u64());
    assert!(report["opaque_calls"].is_u64());
    assert!(report["mean_opaque_share_pct"].is_number());
    assert!(report["skipped_sidechain"].is_u64());
    assert!(report["skipped_malformed_line"].is_u64());
    assert!(report["skipped_non_utf8_line"].is_u64());
    assert!(report["skipped_io_error"].is_u64());
    assert!(report["skipped_missing_field"]["cwd"].is_u64());
    assert!(report["skipped_missing_field"]["tool_use_result"].is_u64());
    assert!(report["peak_rss_mb"].is_number());
    // The debug seam wasn't enabled for this call — the extra field must be
    // fully absent, matching Pinned decision 12's exact shape.
    assert!(report.get("debug_entries").is_none());
}

// ---- 9. AC3: zero bytes created/modified under .agentrec/ across a
// dry-run — digested before/after against a tempdir root (Pinned decision 6:
// never this repo's own working tree, where a live daemon writes). ---------

/// Recursive digest of every file under `<root>/.agentrec` (path + content),
/// or a sentinel string if the directory is absent. Test-local (this crate
/// has no `[lib]` target for `cli/tests/*.rs` to import internals from).
fn agentrec_dir_digest(root: &Path) -> String {
    let dir = root.join(".agentrec");
    if !dir.exists() {
        return "absent".to_string();
    }
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in walkdir::WalkDir::new(&dir).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel = entry
                .path()
                .strip_prefix(&dir)
                .unwrap()
                .to_string_lossy()
                .to_string();
            let bytes = std::fs::read(entry.path()).unwrap_or_default();
            entries.push((rel, bytes));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut buf = Vec::new();
    for (rel, bytes) in entries {
        buf.extend_from_slice(rel.as_bytes());
        buf.push(0);
        buf.extend_from_slice(&bytes);
        buf.push(0);
    }
    agentrec_core::store::hash_bytes(&buf)
}

#[test]
fn ac3_zero_bytes_under_agentrec_in_tempdir() {
    let root = tempfile::tempdir().unwrap();

    // Item 3: `agentrec init` FIRST, so `.agentrec/` genuinely exists before
    // the digest is ever taken — otherwise this test compares two "absent"
    // sentinels and never exercises the hashing path at all. Mirrors
    // `integration.rs::init`'s preamble (git init, then `agentrec init
    // --no-hook --no-service`, both against this tempdir root).
    Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(root.path())
        .status()
        .unwrap();
    let init_out = Command::new(bin())
        .args(["init", "--no-hook", "--no-service"])
        .arg("--root")
        .arg(root.path())
        .output()
        .expect("run agentrec init");
    assert!(
        init_out.status.success(),
        "agentrec init failed: {init_out:?}"
    );

    let before = agentrec_dir_digest(root.path());
    assert_ne!(
        before, "absent",
        "agentrec init must have populated .agentrec/ before the dry-run import \
         runs — otherwise this test never exercises a populated tree"
    );

    let out = run_import_json(&fixture("t1_and_t15"), root.path());
    assert!(out.status.success());

    let after = agentrec_dir_digest(root.path());
    assert_eq!(
        before, after,
        "import claude --dry-run must create or modify zero bytes under .agentrec/"
    );
}

// ---- 10. `--source` defaults to `$HOME/.claude` when omitted — exercised
// cheaply via a fake HOME pointed at a throwaway tempdir (never the CI
// machine's real ~/.claude). ------------------------------------------------

#[test]
fn default_source_resolves_via_home_env_when_source_omitted() {
    let fake_home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(fake_home.path().join(".claude/projects")).unwrap();
    let root = tempfile::tempdir().unwrap();

    let out = Command::new(bin())
        .args(["import", "claude", "--dry-run", "--json"])
        .arg("--root")
        .arg(root.path())
        .env("HOME", fake_home.path())
        .output()
        .expect("run agentrec import claude with fake HOME");
    let report = report_json(&out);
    assert_eq!(
        report["sessions_total"], 0,
        "empty fake ~/.claude/projects — zero sessions, but must not error"
    );
}

// ---- 11. `--dry-run` omitted is a loud, nonzero-exit refusal, never a
// silent no-op. --------------------------------------------------------

#[test]
fn dry_run_false_persists_instead_of_refusing_now_that_p2_has_landed() {
    // Superseded by P2: P1's refusal existed only because persistence
    // didn't exist yet ("not implemented until P2"). Now that it has,
    // omitting `--dry-run` performs real persistence, scoped to `--root`.
    // The "opaque" fixture's session has no file-producing entries at all
    // (only an opaque Bash call), so nothing is in scope to persist
    // regardless of root, and the command must still exit 0.
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(bin())
        .args(["import", "claude", "--source"])
        .arg(fixture("opaque"))
        .arg("--root")
        .arg(tmp.path())
        .output()
        .expect("run agentrec import claude without --dry-run");

    assert!(
        out.status.success(),
        "P2: omitting --dry-run must persist, not refuse: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("appended: 0"),
        "opaque-only fixture has nothing to persist, got: {stdout}"
    );
}

// ---- 12. The text (non-JSON) report prints every field from Pinned
// decision 12's contract in prose form. -------------------------------

#[test]
fn text_report_contains_every_json_field_in_prose() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_text(&fixture("t1_and_t15"), tmp.path());
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);

    for needle in [
        "sessions_total",
        "sessions_importable",
        "sessions_in_root",
        "tier_counts",
        "t1=",
        "t1_5=",
        "t15_unverified=",
        "t15_rejected_unverifiable=",
        "t15_blob_missing=",
        "t2_candidate=",
        "t3=",
        "opaque_calls",
        "mean_opaque_share_pct",
        "skipped_secret_path",
        "skipped_sidechain",
        "skipped_malformed_line",
        "skipped_non_utf8_line",
        "skipped_io_error",
        "skipped_missing_field",
        "cwd=",
        "tool_use_result=",
        "peak_rss_mb",
    ] {
        assert!(
            stdout.contains(needle),
            "text report missing '{needle}':\n{stdout}"
        );
    }
}

// ---- 13. Pinned decision 15 (item 1): a non-UTF8 line does not silently
// truncate the rest of the file — every valid line before AND after it must
// still classify, and the bad line is counted, not dropped with zero signal.
// A fresh temp fixture (not one of the committed corpus scenarios — this
// scenario is authored inline, matching the `t2_t3_candidate` test's own
// precedent of writing a source root under a tempdir). --------------------

#[test]
fn non_utf8_line_mid_file_does_not_truncate_remaining_lines() {
    fn edit_line(session_id: &str, cwd: &str, uuid_suffix: &str, path: &str) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"e00000{uuid_suffix}-0000-4000-8000-0000000000{uuid_suffix}","parentUuid":null,"timestamp":"2026-06-10T09:00:00.000Z","sessionId":"{session_id}","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_{uuid_suffix}","name":"Edit","input":{{"file_path":"{path}","old_string":"a","new_string":"b"}}}}]}},"toolUseResult":{{"type":"update","filePath":"{path}","oldString":"a","newString":"b","originalFile":"a\n","structuredPatch":[],"success":true,"userModified":false}}}}"#
        )
    }

    let session_id = "a1010101-1010-4101-8101-101010101010";
    let cwd = "/fake/repo10a";

    let mut bytes: Vec<u8> = Vec::new();
    bytes.extend_from_slice(edit_line(session_id, cwd, "10", "/fake/repo10a/a.txt").as_bytes());
    bytes.push(b'\n');
    // Line 2: not valid UTF-8 at all (a lone continuation byte followed by
    // an overlong-encoding lead byte — never a valid UTF-8 sequence).
    bytes.extend_from_slice(&[0xC0, 0xAF, 0xFF, 0xFE]);
    bytes.push(b'\n');
    bytes.extend_from_slice(edit_line(session_id, cwd, "11", "/fake/repo10a/b.txt").as_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(edit_line(session_id, cwd, "12", "/fake/repo10a/c.txt").as_bytes());
    bytes.push(b'\n');

    let source_root = tempfile::tempdir().unwrap();
    let proj_dir = source_root.path().join("projects/proj-nonutf8");
    std::fs::create_dir_all(&proj_dir).unwrap();
    std::fs::write(proj_dir.join(format!("{session_id}.jsonl")), bytes).unwrap();

    let root = tempfile::tempdir().unwrap();
    let out = run_import_json(source_root.path(), root.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(
        report["sessions_importable"], 1,
        "3 of 4 lines are valid and classify cleanly; the session must still count"
    );
    assert_eq!(
        report["skipped_non_utf8_line"], 1,
        "exactly one bad line, counted, not silently dropped"
    );
    assert_eq!(
        report["tier_counts"]["t1"], 3,
        "all 3 valid lines (not just the first) must have classified — this is \
         the assertion that fails if the non-UTF8 line silently truncates the rest"
    );
}

// ---- 14. Pinned decision 14 (item 2): `cwd` is session-level. A `type:
// "summary"` head line with no `cwd` must not disqualify a session that
// establishes `cwd` from a later line. -------------------------------------

#[test]
fn session_level_cwd_survives_a_headless_summary_line() {
    let session_id = "a2020202-2020-4202-8202-202020202020";
    let cwd = "/fake/repo10b";

    let lines = [
        r#"{"type":"summary","summary":"resumed session","leafUuid":"b0000000-0000-4000-8000-000000000000"}"#.to_string(),
        format!(
            r#"{{"type":"user","uuid":"e0000020-0000-4000-8000-000000000020","parentUuid":null,"timestamp":"2026-06-10T09:01:00.000Z","sessionId":"{session_id}","cwd":"{cwd}","message":{{"role":"user","content":[{{"type":"text","text":"continue"}}]}}}}"#
        ),
        format!(
            r#"{{"type":"assistant","uuid":"e0000021-0000-4000-8000-000000000021","parentUuid":"e0000020-0000-4000-8000-000000000020","timestamp":"2026-06-10T09:02:00.000Z","sessionId":"{session_id}","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_0000000021","name":"Edit","input":{{"file_path":"{cwd}/resumed.txt","old_string":"a","new_string":"b"}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/resumed.txt","oldString":"a","newString":"b","originalFile":"a\n","structuredPatch":[],"success":true,"userModified":false}}}}"#
        ),
    ];

    let source_root = tempfile::tempdir().unwrap();
    let proj_dir = source_root.path().join("projects/proj-summary");
    std::fs::create_dir_all(&proj_dir).unwrap();
    std::fs::write(
        proj_dir.join(format!("{session_id}.jsonl")),
        lines.join("\n") + "\n",
    )
    .unwrap();

    let root = tempfile::tempdir().unwrap();
    let out = run_import_json(source_root.path(), root.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(
        report["sessions_importable"], 1,
        "the headless summary line must not disqualify a session that later \
         establishes cwd"
    );
    assert_eq!(
        report["skipped_missing_field"]["cwd"], 0,
        "cwd is session-level: it was established by line 2, so this must not fire"
    );
    assert_eq!(report["tier_counts"]["t1"], 1);

    let entries = debug_entries(&report);
    let resumed = entry_for(&entries, "resumed.txt");
    assert_eq!(resumed["tier"], "t1");
}

// ---- 15. Item 5: a sidechain-flagged line whose toolUseResult is opaque
// (Bash-shaped, no `filePath`) must count under `skipped_sidechain`, never
// `opaque_calls` — the sidechain exclusion must run before opaque
// classification. -----------------------------------------------------------

#[test]
fn sidechain_flagged_opaque_call_counts_as_sidechain_not_opaque() {
    let session_id = "a3030303-3030-4303-8303-303030303030";
    let line = format!(
        r#"{{"type":"assistant","uuid":"e0000030-0000-4000-8000-000000000030","parentUuid":null,"timestamp":"2026-06-10T09:03:00.000Z","sessionId":"{session_id}","cwd":"/fake/repo10c","isSidechain":true,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_0000000030","name":"Bash","input":{{"command":"ls -la"}}}}]}},"toolUseResult":{{"stdout":"total 0\n","stderr":"","interrupted":false}}}}"#
    );

    let source_root = tempfile::tempdir().unwrap();
    let proj_dir = source_root.path().join("projects/proj-sidechain-opaque");
    std::fs::create_dir_all(&proj_dir).unwrap();
    std::fs::write(proj_dir.join(format!("{session_id}.jsonl")), line + "\n").unwrap();

    let root = tempfile::tempdir().unwrap();
    let out = run_import_json(source_root.path(), root.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(
        report["skipped_sidechain"], 1,
        "sidechain exclusion must run before opaque-call classification"
    );
    assert_eq!(
        report["opaque_calls"], 0,
        "must not double-count the sidechain-excluded line as an opaque call"
    );
}

// ---- 16. Item 6: a nonexistent `--source` is a hard, nonzero-exit error —
// never a fake `sessions_total: 0` indistinguishable from a legitimately
// empty corpus. --------------------------------------------------------

#[test]
fn nonexistent_source_is_a_loud_error_not_a_fake_empty_corpus() {
    let tmp = tempfile::tempdir().unwrap();
    let bogus = tmp.path().join("this-does-not-exist-at-all");
    let root = tempfile::tempdir().unwrap();

    let out = Command::new(bin())
        .args(["import", "claude", "--dry-run", "--source"])
        .arg(&bogus)
        .arg("--root")
        .arg(root.path())
        .output()
        .expect("run agentrec import claude with a nonexistent --source");

    assert!(
        !out.status.success(),
        "a nonexistent --source must exit nonzero, not report a fake empty corpus"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--source") || stderr.to_lowercase().contains("directory"),
        "stderr must name the problem, got: {stderr}"
    );
}

// ---- 17. Item 6: an EXISTING `--source` whose `projects/` subdirectory is
// present but empty is a legitimate zero-session corpus — exit 0. ----------

#[test]
fn empty_projects_dir_is_a_legitimate_zero_session_corpus() {
    let source_root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(source_root.path().join("projects")).unwrap();
    let root = tempfile::tempdir().unwrap();

    let out = run_import_json(source_root.path(), root.path());
    let report = report_json(&out);

    assert_eq!(
        report["sessions_total"], 0,
        "an existing but empty projects/ dir is a legitimate empty corpus"
    );
}

// ---- 18. T1.5 path-normalization fix: a `trackedFileBackups` key that is
// RELATIVE to the session's `cwd` (measured on the real corpus: 1606 of 1883
// keys) must still resolve — this is the RED->GREEN test for the reported
// defect (fixture `t15_relative_key/`; see FIXTURES.md scenario 8). ---------

#[test]
fn t15_relative_backup_key_resolves_against_session_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("t15_relative_key"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 1);
    assert_eq!(
        report["tier_counts"]["t1_5"], 1,
        "a relative trackedFileBackups key (\"src/notes.txt\") joined against \
         the session's cwd must resolve to T1.5, not fall through to T2/T3; \
         full report: {report}"
    );
    assert_eq!(report["tier_counts"]["t1"], 0);
    assert_eq!(report["tier_counts"]["t2_candidate"], 0);
    assert_eq!(report["tier_counts"]["t3"], 0);
    assert_eq!(
        report["tier_counts"]["t15_unverified"], 0,
        "oldString is present and matches the blob — this must be content-proven"
    );
    assert_eq!(report["tier_counts"]["t15_rejected_unverifiable"], 0);

    let entries = debug_entries(&report);
    let notes = entry_for(&entries, "notes.txt");
    assert_eq!(notes["tier"], "t1_5");
    assert_eq!(
        notes["sha256"], "678cd3ef69c16e338a717c91a0ecdec9c394b4992360d2680539fbaf028b8396",
        "resolved bytes must match the independently-computed shasum of the \
         file-history blob (see FIXTURES.md scenario 8)"
    );
}

// ---- 19. T1.5 "never fabricate" guard: a backup blob resolves and is
// readable, but its content does NOT contain the edit's `oldString` — the
// blob is not this edit's pre-state, so the entry must be refused as T1.5
// and fall through to T2/T3, counted in `t15_rejected_unverifiable`
// (fixture `t15_stale_blob/`; see FIXTURES.md scenario 9). ------------------

#[test]
fn stale_backup_blob_is_rejected_not_classified_t15() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("t15_stale_blob"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 1);
    assert_eq!(
        report["tier_counts"]["t1_5"], 0,
        "the resolved blob does not contain the edit's oldString — must NOT \
         classify T1.5 (would fabricate a plausible-but-wrong pre-state); \
         full report: {report}"
    );
    assert_eq!(
        report["tier_counts"]["t15_rejected_unverifiable"], 1,
        "the rejection must be counted so the loss stays visible"
    );
    // No git repo backs `/fake/repo16` in this fixture, so the fallthrough
    // lands in T3, not T2-candidate.
    assert_eq!(report["tier_counts"]["t3"], 1);
    assert_eq!(report["tier_counts"]["t2_candidate"], 0);
    assert_eq!(report["tier_counts"]["t1"], 0);

    let entries = debug_entries(&report);
    let stale = entry_for(&entries, "stale.txt");
    assert_eq!(stale["tier"], "t3");
    assert!(
        stale["sha256"].is_null(),
        "a rejected/unverifiable entry must never report resolved bytes"
    );
}

// ---- 20. T1.5 ordering trap: the `snapshot` line carrying
// `trackedFileBackups` appears BEFORE any line in the session carries `cwd`.
// The relative key harvested on that first line must still resolve once
// `cwd` becomes known on a later line (fixture `t15_cwd_after_snapshot/`;
// see FIXTURES.md scenario 10). ---------------------------------------------

#[test]
fn relative_key_harvested_before_cwd_is_known_still_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("t15_cwd_after_snapshot"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 1);
    assert_eq!(
        report["tier_counts"]["t1_5"], 1,
        "a trackedFileBackups key harvested on a line BEFORE cwd is \
         established must still resolve once a later line sets cwd; \
         full report: {report}"
    );
    assert_eq!(report["tier_counts"]["t2_candidate"], 0);
    assert_eq!(report["tier_counts"]["t3"], 0);

    let entries = debug_entries(&report);
    let readme = entry_for(&entries, "readme.txt");
    assert_eq!(readme["tier"], "t1_5");
    assert_eq!(
        readme["sha256"], "59c14830f5913e2fb18b82eb129dfb6189bf39b80a7a858c0b9b86758e1ae963",
        "resolved bytes must match the independently-computed shasum of the \
         file-history blob (see FIXTURES.md scenario 10)"
    );
}

// ---- 21. T1.5 unverified branch: `oldString` is absent from the entry
// entirely (some ops don't carry one), so the containment check cannot run.
// The blob reference is still trusted (best evidence available), but the
// entry must be counted as `t15_unverified`, not silently folded into a
// content-proven T1.5 (fixture `t15_unverified_no_oldstring/`; see
// FIXTURES.md scenario 11). This is the corpus-untested defensive branch: a
// full real-corpus gate run (2026-07-29) found `oldString` present on every
// single T1.5 candidate (243/243), so `t15_unverified` measures 0 there —
// this fixture is the only coverage this branch has. --------------------

#[test]
fn missing_old_string_classifies_t15_unverified_not_content_proven() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("t15_unverified_no_oldstring"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 1);
    assert_eq!(
        report["tier_counts"]["t1_5"], 1,
        "no oldString to verify against — the blob reference is still the \
         best evidence available, so this must classify T1.5; full report: \
         {report}"
    );
    assert_eq!(
        report["tier_counts"]["t15_unverified"], 1,
        "must be counted as unverified/assumed, not content-proven"
    );
    assert_eq!(report["tier_counts"]["t15_rejected_unverifiable"], 0);
    assert_eq!(report["tier_counts"]["t2_candidate"], 0);
    assert_eq!(report["tier_counts"]["t3"], 0);

    let entries = debug_entries(&report);
    let data = entry_for(&entries, "data.bin");
    assert_eq!(data["tier"], "t1_5");
    assert_eq!(
        data["sha256"], "03e693d9f2f687e0f40e36a8df7fcb4d1c22974012b7c2a55c000eb30f305824",
        "resolved bytes must match the independently-computed shasum of the \
         file-history blob (see FIXTURES.md scenario 11)"
    );
}

// ---- 22. T1.5 fabrication round, Fix 1: a snapshot followed by TWO edits to
// the same path with no intervening snapshot. The blob is edit #1's true
// pre-state (must classify T1.5, content-proven) but is stale for edit #2 —
// and edit #2's `oldString` ("shared line\n") happens to still be present in
// the untouched region of the stale blob, so the pre-fix `oldString`-only
// guard would have incorrectly accepted it. The intervening-edit check must
// reject edit #2 regardless (fixture `t15_intervening_edit/`; see
// FIXTURES.md scenario 12). This is the fixture shape (snapshot -> edit ->
// edit against the same path) that every prior T1.5 fixture lacked, which is
// why this exact defect class survived three review rounds. --------------

#[test]
fn intervening_edit_between_snapshot_and_second_edit_is_rejected_as_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("t15_intervening_edit"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 1);
    assert_eq!(
        report["tier_counts"]["t1_5"], 1,
        "exactly one of the two edits (#1) has a true pre-state blob; \
         full report: {report}"
    );
    assert_eq!(
        report["tier_counts"]["t15_rejected_stale"], 1,
        "edit #2's backup is stale by one intervening edit (#1) and must be \
         rejected BEFORE any oldString check, even though edit #2's \
         oldString (\"shared line\\n\") is still coincidentally present in \
         the stale blob's untouched region — full report: {report}"
    );
    assert_eq!(
        report["tier_counts"]["t15_rejected_unverifiable"], 0,
        "this must be caught by the staleness check, not the content check"
    );
    assert_eq!(report["tier_counts"]["t2_candidate"], 0);
    assert_eq!(
        report["tier_counts"]["t3"], 1,
        "edit #2 falls through to T3 (no git repo backs /fake/repo19 here)"
    );

    let entries = debug_entries(&report);
    let multi_entries: Vec<&Value> = entries
        .iter()
        .filter(|e| {
            e.get("path")
                .and_then(|p| p.as_str())
                .is_some_and(|p| p.ends_with("multi.txt"))
        })
        .copied()
        .collect();
    assert_eq!(
        multi_entries.len(),
        2,
        "expected one debug_entries row per edit; full report: {report}"
    );
    assert_eq!(multi_entries[0]["tier"], "t1_5");
    assert_eq!(
        multi_entries[0]["sha256"],
        "99931c6269c8f9e306b7ce0ac5fe986ab1121e83df93153eb3f66cf298384c56",
        "resolved bytes for edit #1 must match the independently-computed \
         shasum of the file-history blob (see FIXTURES.md scenario 12)"
    );
    assert_eq!(
        multi_entries[1]["tier"], "t3",
        "edit #2 must NOT resolve T1.5 despite the coincidental oldString match"
    );
    assert!(
        multi_entries[1]["sha256"].is_null(),
        "a stale-rejected entry must never report resolved bytes"
    );
}

// ---- 23. T1.5 fabrication round, ground-truth-discovered refinement: a
// `trackedFileBackups` snapshot line re-announcing the SAME `backupFileName`
// (a manifest-style re-list; confirmed on the real corpus — identical
// backupFileName AND identical backupTime repeated across consecutive
// snapshot lines) must NOT reset the intervening-edit staleness counter.
// Only a genuinely NEW backup identity (a different `backupFileName`) may
// reset it. Real-corpus ground-truth validation of the Fix-1 predicate
// (comparing resolved T1.5 blobs against known-true `originalFile` bytes on
// entries that also happen to carry one) surfaced this as the dominant
// remaining source of fabricated T1.5 classifications after Fix 1 alone: a
// redundant re-announcement of an already-stale backup was wrongly treated
// as evidence of a fresh backup, clearing staleness that should have stayed
// set (fixture `t15_redundant_backup_announcement/`). ------------------------

#[test]
fn redundant_reannouncement_of_same_backup_name_does_not_clear_staleness() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("t15_redundant_backup_announcement"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["sessions_total"], 1);
    assert_eq!(report["sessions_importable"], 1);
    assert_eq!(
        report["tier_counts"]["t1_5"], 1,
        "edit #1 has a true pre-state blob and must still resolve T1.5; \
         full report: {report}"
    );
    assert_eq!(
        report["tier_counts"]["t15_rejected_stale"], 1,
        "edit #2's backup is stale by edit #1, and the SAME backupFileName \
         being re-announced between them (a manifest re-list, not a fresh \
         backup) must NOT clear that staleness — even though edit #2's \
         oldString (\"line B\\n\") is still present in the stale blob's \
         untouched region; full report: {report}"
    );
    assert_eq!(report["tier_counts"]["t15_rejected_unverifiable"], 0);
    assert_eq!(report["tier_counts"]["t2_candidate"], 0);
    assert_eq!(report["tier_counts"]["t3"], 1);

    let entries = debug_entries(&report);
    let redundant_entries: Vec<&Value> = entries
        .iter()
        .filter(|e| {
            e.get("path")
                .and_then(|p| p.as_str())
                .is_some_and(|p| p.ends_with("redundant.txt"))
        })
        .copied()
        .collect();
    assert_eq!(redundant_entries.len(), 2);
    assert_eq!(redundant_entries[0]["tier"], "t1_5");
    assert_eq!(
        redundant_entries[1]["tier"], "t3",
        "must NOT resolve T1.5 despite the redundant same-name re-announcement"
    );
    assert!(redundant_entries[1]["sha256"].is_null());
}

// =====================================================================
// P2: persistence (`agentrec import claude`, no `--dry-run`)
// =====================================================================
//
// P1's fixtures above use fake `cwd` values ("/fake/repo") since dry-run
// classification never touches the filesystem at `cwd`. Persistence DOES
// (root-scoping, T2 git resolution), so every test below builds its own
// transcript dynamically, pointed at a real tempdir it controls — a static
// fixture file can't know its own tempdir path at authoring time.

mod persist {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write_session(source: &Path, project: &str, session_id: &str, lines: &[String]) {
        let dir = source.join("projects").join(project);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{session_id}.jsonl")),
            lines.join("\n") + "\n",
        )
        .unwrap();
    }

    fn run_persist(source: &Path, root: &Path) -> Output {
        Command::new(bin())
            .args(["import", "claude", "--source"])
            .arg(source)
            .arg("--root")
            .arg(root)
            .output()
            .expect("run agentrec import claude (persist)")
    }

    fn run_persist_json(source: &Path, root: &Path) -> Output {
        Command::new(bin())
            .args(["import", "claude", "--json", "--source"])
            .arg(source)
            .arg("--root")
            .arg(root)
            .output()
            .expect("run agentrec import claude --json (persist)")
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

    /// Commits `rel_path` with `content`, at an explicit author/committer
    /// date, so a test can control exactly which commits `git log --before`
    /// sees — real commit dates, not the arbitrary fictional-future
    /// timestamps this fixture format otherwise uses.
    fn git_commit_at(root: &Path, rel_path: &str, content: &str, date_no_tz: &str) {
        // An explicit `+00:00` offset is load-bearing: the transcript
        // timestamps this is compared against (`--before=...Z`) are UTC,
        // and `GIT_AUTHOR_DATE` without an explicit offset is interpreted
        // in the LOCAL timezone — on a machine east of UTC that silently
        // shifts which side of `--before` a commit lands on.
        let date_rfc3339 = format!("{date_no_tz}+00:00");
        std::fs::write(root.join(rel_path), content).unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", rel_path])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .env("GIT_AUTHOR_DATE", &date_rfc3339)
            .env("GIT_COMMITTER_DATE", &date_rfc3339)
            .args(["commit", "-q", "-m", "commit"])
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

    fn turn_lines(root: &Path) -> Vec<serde_json::Value> {
        log_lines(root)
            .iter()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("turn"))
            .collect()
    }

    // ---- AC2 (clm_6NQ448DW5PD4X4XW5EKX9CVPKM): idempotent re-run appends
    // 0; kill-9-then-resume is byte-identical (after sort) to an
    // uninterrupted run. -----------------------------------------------

    #[test]
    fn ac2_rerun_appends_zero_new_records() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());

        let cwd = root.path().to_string_lossy().to_string();
        let lines = vec![
            format!(
                r#"{{"type":"user","uuid":"u1","timestamp":"2026-06-10T09:00:00.000Z","sessionId":"s_ac2","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":"please fix a.txt"}}}}"#
            ),
            format!(
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-06-10T09:00:05.000Z","sessionId":"s_ac2","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/a.txt","oldString":"one","newString":"ONE","originalFile":"one\ntwo\n"}}}}"#
            ),
        ];
        write_session(source.path(), "proj", "s_ac2", &lines);

        let out1 = run_persist(source.path(), root.path());
        assert!(out1.status.success(), "{out1:?}");
        let stdout1 = String::from_utf8_lossy(&out1.stdout);
        assert!(stdout1.contains("appended: 1"), "stdout: {stdout1}");

        let after_first = log_lines(root.path());
        assert_eq!(turn_lines(root.path()).len(), 1);

        let out2 = run_persist(source.path(), root.path());
        assert!(out2.status.success(), "{out2:?}");
        let stdout2 = String::from_utf8_lossy(&out2.stdout);
        assert!(
            stdout2.contains("appended: 0"),
            "AC2: re-run must append 0 new records, got: {stdout2}"
        );
        assert_eq!(
            log_lines(root.path()),
            after_first,
            "AC2: re-run must not change log.jsonl at all"
        );
    }

    // AC2's kill-9 half: a real mid-process SIGKILL is nondeterministic to
    // time reliably in a unit test, so this proves the mechanism that makes
    // resumability true — deterministic ids + idempotent skip — by
    // constructing the "killed after session 1, before session 2" state
    // directly (pre-seeding only session 1's already-appended turn) and
    // asserting the resumed run's final log is byte-identical (after
    // sorting lines, which sorts by `id` here since every line shares the
    // same `v`/`type` prefix) to an uninterrupted two-session run.
    #[test]
    fn ac2_resume_after_simulated_interruption_matches_uninterrupted_run() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let cwd = root.path().to_string_lossy().to_string();

        for sid in ["s1_ac2r", "s2_ac2r"] {
            let lines = vec![
                format!(
                    r#"{{"type":"user","uuid":"u_{sid}","timestamp":"2026-06-11T09:00:00.000Z","sessionId":"{sid}","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":"edit {sid}.txt"}}}}"#
                ),
                format!(
                    r#"{{"type":"assistant","uuid":"a_{sid}","timestamp":"2026-06-11T09:00:05.000Z","sessionId":"{sid}","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t_{sid}","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/{sid}.txt","oldString":"one","newString":"ONE","originalFile":"one\ntwo\n"}}}}"#
                ),
            ];
            write_session(source.path(), "proj", sid, &lines);
        }

        // Reference: one uninterrupted run over both sessions, same root.
        let out_ref = run_persist(source.path(), root.path());
        assert!(out_ref.status.success());
        assert_eq!(turn_lines(root.path()).len(), 2);
        let mut reference: Vec<String> = log_lines(root.path());
        reference.sort();

        // Simulate "killed after session 1, before session 2": wipe this
        // SAME root back to empty (log + objects), then persist only
        // session 1's file, then resume with both files present. Same
        // root throughout means `root`/paths/hashes are directly
        // comparable to `reference` — the earlier version of this test
        // compared two different roots and never actually asserted byte
        // equality; this version does.
        std::fs::remove_file(root.path().join(".agentrec/log.jsonl")).unwrap();
        let _ = std::fs::remove_dir_all(root.path().join(".agentrec/objects"));

        let source_partial = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(source_partial.path().join("projects/proj")).unwrap();
        std::fs::copy(
            source.path().join("projects/proj/s1_ac2r.jsonl"),
            source_partial.path().join("projects/proj/s1_ac2r.jsonl"),
        )
        .unwrap();
        let out_partial = run_persist(source_partial.path(), root.path());
        assert!(out_partial.status.success());
        assert_eq!(
            turn_lines(root.path()).len(),
            1,
            "partial run should have appended exactly session 1's turn"
        );

        // Resume: same root, now with BOTH session files available (mirrors
        // a process that crashed after session 1 and was restarted against
        // the full, unchanged source corpus).
        let out_resume = run_persist(source.path(), root.path());
        assert!(out_resume.status.success());
        let stdout_resume = String::from_utf8_lossy(&out_resume.stdout);
        assert!(
            stdout_resume.contains("appended: 1"),
            "resume must append only the NEW session's turn, session 1 already present: {stdout_resume}"
        );

        let mut resumed: Vec<String> = log_lines(root.path());
        resumed.sort();
        assert_eq!(
            resumed, reference,
            "AC2: a kill-9'd-then-resumed run must be byte-identical (after \
             sorting) to an uninterrupted run"
        );
    }

    // ---- Intra-run id collision (clm_7VYKN2SDTGENZEMXMT4BNXJACP): turn ids
    // are `hash(session_id:turn_index)`, so two session files carrying the
    // SAME `sessionId` (a worktree-resumed session lands one copy per
    // project dir) mint identical ids. These fixtures are SYNTHETIC and the
    // live corpus does not currently exercise this — see the measurement in
    // `importcmd.rs`'s `run_ids` comment: the one real cross-dir `sessionId`
    // on the measured machine has a 1-line stub as its second copy and mints
    // zero colliding turns. Guard, not a reproduction of observed damage.
    // `existing_ids` is built once before the loop and never learned ids
    // appended during the run, so both used to append: two `log.jsonl`
    // turns under one id, which makes `diff`/`show`/`undo <id>` ambiguous.
    // The second is now skipped and COUNTED (never silently dropped).

    fn dup_session_lines(cwd: &str, sid: &str, file: &str) -> Vec<String> {
        vec![
            format!(
                r#"{{"type":"user","uuid":"u_{file}","timestamp":"2026-06-12T09:00:00.000Z","sessionId":"{sid}","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":"edit {file}"}}}}"#
            ),
            format!(
                r#"{{"type":"assistant","uuid":"a_{file}","timestamp":"2026-06-12T09:00:05.000Z","sessionId":"{sid}","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"tu_{file}","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/{file}","oldString":"one","newString":"ONE","originalFile":"one\ntwo\n"}}}}"#
            ),
        ]
    }

    #[test]
    fn intra_run_duplicate_session_id_appends_one_turn_and_counts_the_skip() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let cwd = root.path().to_string_lossy().to_string();

        // Same `sessionId`, two project dirs, DIFFERENT touched files — so a
        // duplicate append would be two same-id turns disagreeing on `files`.
        write_session(
            source.path(),
            "projA",
            "s_dup",
            &dup_session_lines(&cwd, "s_dup", "a.txt"),
        );
        write_session(
            source.path(),
            "projB",
            "s_dup",
            &dup_session_lines(&cwd, "s_dup", "b.txt"),
        );

        let out = run_persist(source.path(), root.path());
        assert!(out.status.success(), "{out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("appended: 1"),
            "one id must yield one turn, got: {stdout}"
        );
        assert!(
            stdout.contains("skipped_duplicate_turn_id: 1"),
            "the dropped turn must be counted in text output, got: {stdout}"
        );

        let turns = turn_lines(root.path());
        assert_eq!(turns.len(), 1, "exactly one turn on the wire: {turns:?}");

        // JSON report carries the same counter under the same key.
        let root2 = tempfile::tempdir().unwrap();
        init_repo(root2.path());
        let cwd2 = root2.path().to_string_lossy().to_string();
        let source2 = tempfile::tempdir().unwrap();
        write_session(
            source2.path(),
            "projA",
            "s_dup2",
            &dup_session_lines(&cwd2, "s_dup2", "a.txt"),
        );
        write_session(
            source2.path(),
            "projB",
            "s_dup2",
            &dup_session_lines(&cwd2, "s_dup2", "b.txt"),
        );
        let out_json = run_persist_json(source2.path(), root2.path());
        assert!(out_json.status.success(), "{out_json:?}");
        let report: Value = serde_json::from_slice(&out_json.stdout).unwrap();
        assert_eq!(report["appended"], 1, "report: {report}");
        assert_eq!(report["skipped_duplicate_turn_id"], 1, "report: {report}");
    }

    /// Guard: the intra-run set must only ever collide across DISTINCT
    /// logical turns. A single session file yielding several turns has a
    /// distinct `turn_index` per turn, so all of them must still append —
    /// every other persist test in this file is one-turn-per-file and so
    /// cannot catch an over-eager skip.
    #[test]
    fn multi_turn_session_file_still_appends_every_turn() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let cwd = root.path().to_string_lossy().to_string();

        let mut lines = dup_session_lines(&cwd, "s_multi", "a.txt");
        lines.extend(vec![
            format!(
                r#"{{"type":"user","uuid":"u2_multi","timestamp":"2026-06-12T10:00:00.000Z","sessionId":"s_multi","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":"now edit b.txt"}}}}"#
            ),
            format!(
                r#"{{"type":"assistant","uuid":"a2_multi","timestamp":"2026-06-12T10:00:05.000Z","sessionId":"s_multi","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"tu2_multi","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/b.txt","oldString":"one","newString":"ONE","originalFile":"one\ntwo\n"}}}}"#
            ),
        ]);
        write_session(source.path(), "proj", "s_multi", &lines);

        let out = run_persist(source.path(), root.path());
        assert!(out.status.success(), "{out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("appended: 2"),
            "two prompts in one file are two distinct turns: {stdout}"
        );
        assert!(
            stdout.contains("skipped_duplicate_turn_id: 0"),
            "no collision here: {stdout}"
        );
        assert_eq!(turn_lines(root.path()).len(), 2);
    }

    // ---- AC4 (clm_6TEDAJ0DE6S93FAQ7BYQPRE1WE): import-missing-`before`
    // entries have `baseline_unknown == false`, asserted on the wire. ------

    #[test]
    fn ac4_import_missing_before_never_sets_baseline_unknown() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path()); // no commit at all — T2 has nothing to find

        let cwd = root.path().to_string_lossy().to_string();
        let lines = vec![
            format!(
                r#"{{"type":"user","uuid":"u1","timestamp":"2026-06-12T09:00:00.000Z","sessionId":"s_ac4","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":"edit orphan.txt"}}}}"#
            ),
            format!(
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-06-12T09:00:05.000Z","sessionId":"s_ac4","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/orphan.txt","oldString":"one","newString":"ONE"}}}}"#
            ),
        ];
        write_session(source.path(), "proj", "s_ac4", &lines);

        let out = run_persist(source.path(), root.path());
        assert!(out.status.success(), "{out:?}");
        let turns = turn_lines(root.path());
        assert_eq!(turns.len(), 1);
        let files = turns[0]["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0]["before"].is_null(), "T3: before must be null");
        assert!(
            files[0].get("baseline_unknown").is_none(),
            "AC4: baseline_unknown must be absent (false) on an import-missing-before \
             entry, not reused from the live first-observation meaning; got: {:?}",
            files[0]
        );
    }

    // ---- AC5 (clm_6V537ZT8N3Z11MXVEWZE1P65SA): T2 resolution — the latest
    // commit at-or-before the entry's own timestamp, byte-equal to
    // known-good content; a candidate whose exact bytes were never
    // committed falls through to T3 `before: null`, never a near-miss
    // commit. -------------------------------------------------------------

    #[test]
    fn ac5_t2_candidate_resolves_exact_committed_blob() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());

        // Commit BEFORE the edit's own timestamp: this is the state T2 must
        // resolve to.
        git_commit_at(root.path(), "tracked.txt", "alpha\n", "2026-06-13T08:00:00");
        // A LATER commit, after the edit — must never be preferred.
        git_commit_at(
            root.path(),
            "tracked.txt",
            "alpha-newer\n",
            "2026-06-13T10:00:00",
        );

        let cwd = root.path().to_string_lossy().to_string();
        let lines = vec![
            format!(
                r#"{{"type":"user","uuid":"u1","timestamp":"2026-06-13T09:00:00.000Z","sessionId":"s_ac5","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":"edit tracked.txt"}}}}"#
            ),
            format!(
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-06-13T09:00:05.000Z","sessionId":"s_ac5","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/tracked.txt","oldString":"alpha","newString":"beta"}}}}"#
            ),
        ];
        write_session(source.path(), "proj", "s_ac5", &lines);

        let out = run_persist_json(source.path(), root.path());
        assert!(out.status.success(), "{out:?}");
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["t2_resolution"]["resolved"], 1, "report: {report}");

        let turns = turn_lines(root.path());
        assert_eq!(turns.len(), 1);
        let files = turns[0]["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        let before_ref = files[0]["before"].as_str().expect("before must resolve");
        // Byte-equal to the known-good pre-edit commit, not the later one.
        assert_eq!(
            before_ref,
            agentrec_core::store::hash_bytes(b"alpha\n"),
            "must resolve the commit at-or-before the edit's OWN timestamp, never a later one"
        );
    }

    // Never a near-miss: bytes that were never committed at all (the edit's
    // `oldString` doesn't match ANY committed state, e.g. a mid-session
    // intermediate edit that never got its own commit) must fall through to
    // T3 `before: null`, never fall back to whatever commit happens to exist.
    #[test]
    fn ac5_t2_never_falls_back_to_a_near_miss_commit() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());

        // The only commit's content does NOT contain the edit's `oldString`
        // ("alpha") at all — this is not this edit's pre-state.
        git_commit_at(
            root.path(),
            "tracked.txt",
            "totally-unrelated-content\n",
            "2026-06-14T08:00:00",
        );

        let cwd = root.path().to_string_lossy().to_string();
        let lines = vec![
            format!(
                r#"{{"type":"user","uuid":"u1","timestamp":"2026-06-14T09:00:00.000Z","sessionId":"s_ac5b_neg","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":"edit tracked.txt"}}}}"#
            ),
            format!(
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-06-14T09:00:05.000Z","sessionId":"s_ac5b_neg","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/tracked.txt","oldString":"alpha","newString":"beta"}}}}"#
            ),
        ];
        write_session(source.path(), "proj", "s_ac5b_neg", &lines);

        let out = run_persist_json(source.path(), root.path());
        assert!(out.status.success(), "{out:?}");
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["t2_resolution"]["resolved"], 0);
        assert_eq!(report["t2_resolution"]["rejected_unverifiable"], 1);

        let turns = turn_lines(root.path());
        let files = turns[0]["files"].as_array().unwrap();
        assert!(
            files[0]["before"].is_null(),
            "must fall through to T3 before:null, never the near-miss commit's bytes"
        );
    }

    // ---- AC5b (added): fabrication-rate oracle. Two fixture legs
    // (positive: git blob matches; negative: it doesn't) plus a real-corpus
    // measurement (separate #[ignore]d test below, run manually and
    // reported — never asserted against, since the corpus is this
    // developer's real, uncommitted-to-the-repo data). --------------------

    #[test]
    fn ac5b_oracle_fixture_positive_git_blob_matches_true_before() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        git_commit_at(root.path(), "f.txt", "one\ntwo\n", "2026-06-15T08:00:00");

        let cwd = root.path().to_string_lossy().to_string();
        // originalFile ("one\ntwo\n") matches exactly what was committed —
        // the oracle must find 0 mismatches over a denominator of 1.
        let lines = vec![
            format!(
                r#"{{"type":"user","uuid":"u1","timestamp":"2026-06-15T09:00:00.000Z","sessionId":"s_oracle_pos","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":"edit f.txt"}}}}"#
            ),
            format!(
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-06-15T09:00:05.000Z","sessionId":"s_oracle_pos","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/f.txt","oldString":"one","newString":"ONE","originalFile":"one\ntwo\n"}}}}"#
            ),
        ];
        write_session(source.path(), "proj", "s_oracle_pos", &lines);

        let out = Command::new(bin())
            .args(["import", "claude", "--json", "--source"])
            .arg(source.path())
            .arg("--root")
            .arg(root.path())
            .env("AGENTREC_IMPORT_T2_ORACLE", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["t2_oracle"]["denominator"], 1, "report: {report}");
        assert_eq!(report["t2_oracle"]["mismatches"], 0);
    }

    #[test]
    fn ac5b_oracle_fixture_negative_git_blob_disagrees_with_true_before() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        // Committed content DOES contain the edit's `oldString` ("one") —
        // so the T1.5-style content guard `resolve_t2_before` applies would
        // pass it — but the rest of the blob disagrees with the true
        // `originalFile` below. This is the exact fabrication shape that
        // survived an `oldString`-only guard once already (T1.5 round): a
        // stale/wrong blob whose unrelated region happens to contain the
        // needle.
        git_commit_at(
            root.path(),
            "f.txt",
            "one\nDIFFERENT-STALE-LINE\n",
            "2026-06-16T08:00:00",
        );

        let cwd = root.path().to_string_lossy().to_string();
        let lines = vec![
            format!(
                r#"{{"type":"user","uuid":"u1","timestamp":"2026-06-16T09:00:00.000Z","sessionId":"s_oracle_neg","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":"edit f.txt"}}}}"#
            ),
            format!(
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-06-16T09:00:05.000Z","sessionId":"s_oracle_neg","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/f.txt","oldString":"one","newString":"ONE","originalFile":"one\ntwo\n"}}}}"#
            ),
        ];
        write_session(source.path(), "proj", "s_oracle_neg", &lines);

        let out = Command::new(bin())
            .args(["import", "claude", "--json", "--source"])
            .arg(source.path())
            .arg("--root")
            .arg(root.path())
            .env("AGENTREC_IMPORT_T2_ORACLE", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["t2_oracle"]["denominator"], 1, "report: {report}");
        assert_eq!(
            report["t2_oracle"]["mismatches"], 1,
            "oracle must detect that the git blob does NOT match the true pre-edit bytes"
        );
    }

    // The debug seam must not exist in a release binary (Pinned decision 9
    // posture, extended to this new seam).
    #[test]
    #[cfg(not(debug_assertions))]
    fn ac5b_oracle_seam_disabled_in_release_even_with_env_set() {
        std::env::set_var("AGENTREC_IMPORT_T2_ORACLE", "1");
        // In a release build the seam is compiled out; nothing to assert
        // beyond "this compiles and the env var is inert" — the release
        // `strings` check (run manually, see report) is the real proof.
        std::env::remove_var("AGENTREC_IMPORT_T2_ORACLE");
    }

    // ---- AC6 (unlabeled in P2.md's list, 5th bullet): a planted secret in
    // a transcript prompt appears in neither the CAS prompt blob nor
    // `prompt_excerpt`. Scrub runs INSIDE persistence (house invariant). ---

    #[test]
    fn ac6_planted_secret_in_prompt_never_appears_in_blob_or_excerpt() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());

        let cwd = root.path().to_string_lossy().to_string();
        // D5 (P2 fix round): AC6 names THREE distinct secret classes, not
        // one — an earlier version of this test planted only the `sk-...`
        // shape. All three literals below are the exact ones
        // `agentrec-core/src/scrub.rs`'s own test suite already proves
        // redact (`known_secret_shapes_redacted`,
        // `entropy_catches_hex_and_decimal_tokens`,
        // `quoted_multiword_secret_fully_redacted`) — reused here rather
        // than invented, so this test can't silently drift from what scrub
        // actually catches.
        let sk_secret = "sk-abcdefghijklmnopqrstuvwxyz012345";
        let hex64_secret = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let quoted_secret = r#"password = "correct horse battery staple""#;
        let prompt_text = format!(
            "use this key {sk_secret} and blob {hex64_secret} and {quoted_secret} to fix a.txt"
        );
        let lines = vec![
            format!(
                r#"{{"type":"user","uuid":"u1","timestamp":"2026-06-17T09:00:00.000Z","sessionId":"s_ac6","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":{prompt_json}}}}}"#,
                prompt_json = serde_json::to_string(&prompt_text).unwrap()
            ),
            format!(
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-06-17T09:00:05.000Z","sessionId":"s_ac6","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/a.txt","oldString":"one","newString":"ONE","originalFile":"one\ntwo\n"}}}}"#
            ),
        ];
        write_session(source.path(), "proj", "s_ac6", &lines);

        let out = run_persist(source.path(), root.path());
        assert!(out.status.success(), "{out:?}");

        let turns = turn_lines(root.path());
        assert_eq!(turns.len(), 1);
        let excerpt = turns[0]["prompt_excerpt"].as_str().unwrap_or("");

        let prompt_ref = turns[0]["prompt_ref"]
            .as_str()
            .expect("prompt must have been stored");
        let hex = prompt_ref.strip_prefix("sha256:").unwrap();
        let (fan, rest) = hex.split_at(2);
        let blob_path = root.path().join(".agentrec/objects").join(fan).join(rest);
        let blob = std::fs::read_to_string(&blob_path).unwrap();
        assert!(
            blob.contains("[redacted:"),
            "blob must show a redaction marker: {blob}"
        );

        for (label, secret) in [
            ("sk-...", sk_secret),
            ("64-hex", hex64_secret),
            ("quoted multi-word credential", quoted_secret),
        ] {
            assert!(
                !excerpt.contains(secret),
                "AC6: {label} secret leaked into prompt_excerpt: {excerpt}"
            );
            assert!(
                !blob.contains(secret),
                "AC6: {label} secret leaked into the CAS prompt blob: {blob}"
            );
        }
        // The quoted credential's individual words must not leak either
        // (mirrors scrub.rs's own `quoted_multiword_secret_fully_redacted`
        // assertion) — containment-of-the-whole-string alone could miss a
        // partial leak if scrub redacted only part of the quoted value.
        for word in ["correct", "horse", "battery", "staple"] {
            assert!(
                !blob.contains(word),
                "AC6: quoted secret word leaked: {word} in {blob}"
            );
        }
    }

    // ---- D7 (P2 fix round): dry-run and persist must agree on a secret
    // file — dry-run counts it under `skipped_secret_path` (never
    // tier-classified as if it were reconstructible), persist withholds it
    // (never snapshotted). Same transcript, both modes, one shared fixture
    // — exactly the cross-check the P1 fabrication defects show is needed
    // whenever two paths measure "the same" thing independently.
    #[test]
    fn d7_secret_path_parity_between_dry_run_and_persist() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());

        let cwd = root.path().to_string_lossy().to_string();
        let lines = vec![
            format!(
                r#"{{"type":"user","uuid":"u1","timestamp":"2026-06-19T09:00:00.000Z","sessionId":"s_d7","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":"edit .env"}}}}"#
            ),
            format!(
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-06-19T09:00:05.000Z","sessionId":"s_d7","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/.env","oldString":"KEY=old","newString":"KEY=new","originalFile":"KEY=old\n"}}}}"#
            ),
        ];
        write_session(source.path(), "proj", "s_d7", &lines);

        // Dry-run leg: the secret file must be counted under
        // `skipped_secret_path`, NOT tier-classified.
        let dry_out = Command::new(bin())
            .args(["import", "claude", "--dry-run", "--json", "--source"])
            .arg(source.path())
            .arg("--root")
            .arg(root.path())
            .output()
            .unwrap();
        assert!(dry_out.status.success(), "{dry_out:?}");
        let dry_report: Value = serde_json::from_slice(&dry_out.stdout).unwrap();
        assert_eq!(dry_report["skipped_secret_path"], 1, "report: {dry_report}");
        assert_eq!(dry_report["tier_counts"]["t1"], 0, "report: {dry_report}");

        // Persist leg: withheld, never snapshotted, never stored as an
        // over-cap/regular blob.
        let persist_out = run_persist(source.path(), root.path());
        assert!(persist_out.status.success(), "{persist_out:?}");
        let turns = turn_lines(root.path());
        assert_eq!(turns.len(), 1);
        let files = turns[0]["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["withheld"], true, "entry: {:?}", files[0]);
        assert!(files[0]["before"].is_null());
        assert!(files[0]["after"].is_null());
    }

    // ---- BLOCKER 1 (P2 integration-gate fix round): a file-producing
    // entry whose `filePath` is NOT lexically under its session's `cwd`
    // must be COUNTED (`skipped_out_of_cwd`), not silently vanish. Mirrors
    // the skeptic's proven failure scenario exactly: `cwd = <root>/sub`,
    // a fully-recoverable T1 entry (real `originalFile` bytes) sits at
    // `<root>/outside-cwd-inside-root.txt` — outside cwd, but still inside
    // root. Recovery is NOT authorized this round: the entry must still be
    // refused, only now visibly.
    #[test]
    fn blocker1_out_of_cwd_entry_is_counted_not_silently_dropped() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        std::fs::create_dir_all(root.path().join("sub")).unwrap();

        let cwd = root.path().join("sub").to_string_lossy().to_string();
        let outside_path = root.path().join("outside-cwd-inside-root.txt");
        let outside_path_str = outside_path.to_string_lossy().to_string();
        let lines = vec![
            format!(
                r#"{{"type":"user","uuid":"u1","timestamp":"2026-06-20T09:00:00.000Z","sessionId":"s_b1","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":"touch both files"}}}}"#
            ),
            // In-cwd entry: must persist normally (proves the fix doesn't
            // over-refuse — only the genuinely out-of-cwd entry is dropped).
            format!(
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-06-20T09:00:05.000Z","sessionId":"s_b1","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/inside.txt","oldString":"one","newString":"ONE","originalFile":"one\ntwo\n"}}}}"#
            ),
            // Out-of-cwd (but in-root) entry: fully recoverable (real
            // originalFile bytes) yet must be refused-and-counted, not
            // silently dropped.
            format!(
                r#"{{"type":"assistant","uuid":"a2","timestamp":"2026-06-20T09:00:06.000Z","sessionId":"s_b1","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t2","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{outside_path_str}","oldString":"alpha","newString":"beta","originalFile":"alpha\nkeep\n"}}}}"#
            ),
        ];
        write_session(source.path(), "proj", "s_b1", &lines);

        let out = run_persist_json(source.path(), root.path());
        assert!(out.status.success(), "{out:?}");
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            report["skipped_out_of_cwd"], 1,
            "the out-of-cwd entry must be counted: report={report}"
        );

        let turns = turn_lines(root.path());
        assert_eq!(turns.len(), 1);
        let files = turns[0]["files"].as_array().unwrap();
        assert_eq!(
            files.len(),
            1,
            "only the in-cwd entry persists — the out-of-cwd one is refused, not recovered: {files:?}"
        );
        assert_eq!(files[0]["path"], "sub/inside.txt");
        assert!(
            !files.iter().any(|f| f["path"]
                .as_str()
                .unwrap_or("")
                .contains("outside-cwd-inside-root")),
            "the out-of-cwd entry must never appear in files, even though its bytes were \
             fully recoverable: {files:?}"
        );
    }

    // ---- AC7 (last P2.md bullet): `log` renders imported turns with a
    // "partial file list (imported)" marker; a bare turn is never relabeled.

    #[test]
    fn ac7_log_marks_imported_turns_partial_and_never_relabels_bare() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());

        let cwd = root.path().to_string_lossy().to_string();
        let lines = vec![
            format!(
                r#"{{"type":"user","uuid":"u1","timestamp":"2026-06-18T09:00:00.000Z","sessionId":"s_ac7","cwd":"{cwd}","isSidechain":false,"message":{{"role":"user","content":"edit a.txt"}}}}"#
            ),
            format!(
                r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-06-18T09:00:05.000Z","sessionId":"s_ac7","cwd":"{cwd}","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{}}}}]}},"toolUseResult":{{"type":"update","filePath":"{cwd}/a.txt","oldString":"one","newString":"ONE","originalFile":"one\ntwo\n"}}}}"#
            ),
        ];
        write_session(source.path(), "proj", "s_ac7", &lines);

        let out = run_persist(source.path(), root.path());
        assert!(out.status.success(), "{out:?}");

        // Also seed a plain BARE turn directly (never imported, never
        // rich) — the second half of this test's name ("never relabels
        // bare") is unproven without one actually present in the log.
        agentrec_core::record::append_log(
            &root.path().join(".agentrec/log.jsonl"),
            &agentrec_core::record::LogRecord::Turn(agentrec_core::record::TurnRecord {
                v: 1,
                id: "t_bareturn0000000000000001".into(),
                grade: "bare".into(),
                truncated: false,
                started: "2026-06-18T10:00:00.000Z".into(),
                ended: "2026-06-18T10:00:01.000Z".into(),
                tool: None,
                model: None,
                session: None,
                root: root.path().to_string_lossy().to_string(),
                prompt_ref: None,
                prompt_excerpt: None,
                merges: vec![],
                imported: None,
                files_complete: None,
                files: vec![agentrec_core::record::FileEntry {
                    path: "bare.txt".into(),
                    before: None,
                    after: Some(agentrec_core::store::hash_bytes(b"x")),
                    op: "create".into(),
                    skipped: false,
                    withheld: false,
                    baseline_unknown: true,
                    skipped_reason: None,
                    after_synthesized: None,
                    link_kind: None,
                    attribution: None,
                }],
            }),
        )
        .unwrap();

        let log_out = Command::new(bin())
            .args(["log"])
            .arg("--root")
            .arg(root.path())
            .output()
            .unwrap();
        assert!(log_out.status.success(), "{log_out:?}");
        let stdout = String::from_utf8_lossy(&log_out.stdout);

        let imported_line = stdout
            .lines()
            .find(|l| l.contains("claude"))
            .expect("imported turn's line must be present");
        assert!(
            imported_line.contains("partial file list (imported)"),
            "log must mark the imported turn's partial file list, got: {imported_line}"
        );

        let bare_line = stdout
            .lines()
            .find(|l| l.contains("bare"))
            .expect("bare turn's line must be present");
        assert!(
            !bare_line.contains("partial file list (imported)"),
            "AC7: a bare (live, unattributed) turn must never be relabeled as imported, \
             got: {bare_line}"
        );
    }

    // ---- Real-corpus leg (AC5b requires this — a fixture alone cannot
    // close the negative case). Runs `agentrec import claude` in
    // --dry-run-equivalent oracle mode against THIS machine's real
    // ~/.claude/projects corpus for measurement only; nothing from that
    // corpus is committed to the repo. `#[ignore]`d because it depends on
    // developer-machine state unavailable in CI — run manually and the
    // measured rate is captured in the round's report.
    #[test]
    #[ignore]
    fn ac5b_oracle_real_corpus_measurement() {
        let home = std::env::var("HOME").expect("HOME must be set");
        let source = PathBuf::from(&home).join(".claude");
        if !source.join("projects").is_dir() {
            eprintln!("no ~/.claude/projects on this machine — skipping");
            return;
        }
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let out = Command::new(bin())
            .args(["import", "claude", "--json", "--source"])
            .arg(&source)
            .arg("--root")
            .arg(root.path())
            .env("AGENTREC_IMPORT_T2_ORACLE", "1")
            .output()
            .expect("run real-corpus oracle pass");
        let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap() - start;
        assert!(out.status.success(), "{out:?}");
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();

        let denominator = report["t2_oracle"]["denominator"].as_u64().unwrap_or(0);
        let mismatches = report["t2_oracle"]["mismatches"].as_u64().unwrap_or(0);

        // D2 (P2 fix round, founder-corrected bar): "PASSES <=1%" is not a
        // claim this oracle can ever establish — the whole guard-admitted
        // channel on this corpus is on the order of a few hundred cases at
        // best, nowhere near the ~299 clean samples a genuine <=1% claim
        // needs. The defensible claim is a one-sided Clopper-Pearson 95%
        // upper bound on the observed rate, computed exactly (for 0
        // observed mismatches the closed form is `1 - 0.05^(1/n)`).
        let upper_bound_pct = if denominator > 0 && mismatches == 0 {
            (1.0 - 0.05f64.powf(1.0 / denominator as f64)) * 100.0
        } else {
            f64::NAN
        };
        eprintln!(
            "AC5b real-corpus report ({elapsed:?}): {report}\n\
             {mismatches} fabrications in {denominator} guard-admitted real-corpus samples \
             (95% upper bound {upper_bound_pct:.1}%). The <=1% bar is not establishable on \
             this corpus by this oracle; the whole verifiable channel is on this order of \
             magnitude, not thousands."
        );

        // The actual ratchet: mismatches must stay at 0 on whatever the
        // guard admits. A regression to the pre-hardening ~37% fabrication
        // rate (or any nonzero rate) must fail this test — an eprintln!
        // with no assertion, which is what this test used to be, passes on
        // a silent regression.
        assert_eq!(
            mismatches, 0,
            "AC5b ratchet: 0 fabrications required on the guard-admitted channel \
             (denominator={denominator}); a regression here is a correctness defect, \
             not a recall trade-off"
        );
    }
}
