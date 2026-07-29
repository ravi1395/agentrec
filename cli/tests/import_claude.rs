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
// importable (zero lines parsed). ------------------------------------------

#[test]
fn malformed_lines_all_skip_exit_zero_and_session_not_importable() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_import_json(&fixture("malformed"), tmp.path());
    let report = report_json(&out);

    assert_eq!(report["skipped_malformed_line"], 5);
    assert_eq!(report["sessions_total"], 1);
    assert_eq!(
        report["sessions_importable"], 0,
        "zero lines parsed — decision 3(b)'s carve-out doesn't apply"
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
    assert!(report["tier_counts"]["t2_candidate"].is_u64());
    assert!(report["tier_counts"]["t3"].is_u64());
    assert!(report["opaque_calls"].is_u64());
    assert!(report["skipped_sidechain"].is_u64());
    assert!(report["skipped_malformed_line"].is_u64());
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
    let before = agentrec_dir_digest(root.path());

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
        "t2_candidate=",
        "t3=",
        "opaque_calls",
        "skipped_sidechain",
        "skipped_malformed_line",
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
