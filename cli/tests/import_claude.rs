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
fn dry_run_false_is_a_loud_error_not_a_silent_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(bin())
        .args(["import", "claude", "--source"])
        .arg(fixture("opaque"))
        .arg("--root")
        .arg(tmp.path())
        .output()
        .expect("run agentrec import claude without --dry-run");

    assert!(!out.status.success(), "must exit nonzero without --dry-run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--dry-run") && stderr.contains("P2"),
        "stderr must name the reason, got: {stderr}"
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
