//! `agentrec search` (Phase 3.0 T3) — CLI adapter over
//! `RepositoryView::search` (`agentrec-core/src/search.rs`). Numbers are
//! hand-derived from the fixture each test builds, per the plan's
//! self-checking-fixture rule.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures; no attacker-supplied FIFO to
//    block on (same rationale as stats.rs's identical allow).

use agentrec_core::record::{append_log, FileEntry, LogRecord, TurnRecord};
use agentrec_core::store::BlobStore;
use std::path::Path;
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

fn git_init(root: &Path) {
    let status = Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(root)
        .status()
        .expect("spawn git init");
    assert!(status.success(), "git init failed: {status:?}");
}

fn init(root: &Path) {
    git_init(root);
    let out = Command::new(bin())
        .args(["init", "--no-hook", "--no-service", "--root"])
        .arg(root)
        .output()
        .expect("agentrec init");
    assert!(out.status.success(), "init failed: {out:?}");
}

fn fe(path: &str, before: Option<String>, after: Option<String>, op: &str) -> FileEntry {
    FileEntry {
        path: path.to_string(),
        before,
        after,
        op: op.to_string(),
        skipped: false,
        withheld: false,
        baseline_unknown: false,
        skipped_reason: None,
        after_synthesized: None,
        link_kind: None,
        attribution: None,
    }
}

fn seed_turn(root: &Path, turn: TurnRecord) {
    append_log(&root.join(".agentrec/log.jsonl"), &LogRecord::Turn(turn)).expect("seed turn");
}

fn base_turn(id: &str, tool: Option<&str>, model: Option<&str>, root: &Path) -> TurnRecord {
    TurnRecord {
        v: 1,
        id: id.to_string(),
        grade: "rich".to_string(),
        truncated: false,
        started: "2020-06-01T00:00:00.000Z".to_string(),
        ended: "2020-06-01T00:00:05.000Z".to_string(),
        tool: tool.map(str::to_string),
        model: model.map(str::to_string),
        session: None,
        root: root.to_string_lossy().to_string(),
        prompt_ref: None,
        prompt_excerpt: None,
        merges: vec![],
        imported: None,
        files_complete: None,
        origin: None,
        files: vec![],
    }
}

// ---------------------------------------------------------------------------
// Substring match per field
// ---------------------------------------------------------------------------

#[test]
fn matches_the_prompt_field_via_cas() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));
    let hash = store.put(b"investigate the flaky bisect probe").unwrap();
    let mut t = base_turn("t_PROMPT0000000000000000001", Some("claude"), None, root);
    t.prompt_ref = Some(hash);
    seed_turn(root, t);

    let out = agentrec(root, &["search", "flaky"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("t_PROMPT0000000000000000001"), "{text}");
    assert!(text.contains("[prompt]"), "{text}");
    assert!(text.contains("flaky"), "{text}");
}

#[test]
fn matches_the_tool_field() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    seed_turn(
        root,
        base_turn("t_TOOL00000000000000000001", Some("codex"), None, root),
    );

    let out = agentrec(root, &["search", "codex"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("[tool]"), "{text}");
    assert!(text.contains("t_TOOL00000000000000000001"), "{text}");
}

#[test]
fn matches_the_model_field() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    seed_turn(
        root,
        base_turn(
            "t_MODEL0000000000000000001",
            Some("claude"),
            Some("opus-9"),
            root,
        ),
    );

    let out = agentrec(root, &["search", "opus-9"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("[model]"), "{text}");
}

#[test]
fn matches_a_file_path() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let mut t = base_turn("t_PATH00000000000000000001", Some("claude"), None, root);
    t.files = vec![fe(
        "agentrec-core/src/bisect.rs",
        None,
        Some("sha256:aa".to_string()),
        "create",
    )];
    seed_turn(root, t);

    let out = agentrec(root, &["search", "bisect.rs"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("[path]"), "{text}");
    assert!(text.contains("agentrec-core/src/bisect.rs"), "{text}");
}

// ---------------------------------------------------------------------------
// --regex invalid pattern
// ---------------------------------------------------------------------------

#[test]
fn invalid_regex_pattern_is_a_clean_error_not_a_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let out = agentrec(root, &["search", "(unclosed", "--regex"]);
    assert!(!out.status.success(), "{out:?}");
    assert!(
        out.status.code().is_some(),
        "must exit cleanly, not crash/signal: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid --regex pattern"), "{stderr}");
}

// ---------------------------------------------------------------------------
// Gate round-2 A1: dangling_prompt_refs renders (as 0) even in the
// no-matches branch — zeros-always-render precedent from T2's stats.
// ---------------------------------------------------------------------------

#[test]
fn no_matches_still_renders_dangling_prompt_refs_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    seed_turn(
        root,
        base_turn("t_NOMATCH000000000000000001", Some("claude"), None, root),
    );

    let out = agentrec(root, &["search", "no-such-term-anywhere"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("no matches"), "{text}");
    assert!(text.contains("dangling_prompt_refs=0"), "{text}");
}

// ---------------------------------------------------------------------------
// --content: reserved, errors rather than silently degrading
// ---------------------------------------------------------------------------

#[test]
fn content_flag_errors_content_search_not_implemented() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let out = agentrec(root, &["search", "anything", "--content"]);
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("content search not implemented"),
        "{stderr}"
    );
}

// ---------------------------------------------------------------------------
// --json parity — no clock field on this verb, so full equality holds.
// ---------------------------------------------------------------------------

#[test]
fn json_matches_search_page_field_for_field() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    seed_turn(
        root,
        base_turn(
            "t_JSON00000000000000000001",
            Some("needle-tool"),
            None,
            root,
        ),
    );

    let out = agentrec(root, &["search", "needle-tool", "--json"]);
    assert!(out.status.success(), "{out:?}");
    let cli_json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    let view = agentrec_core::view::RepositoryView::open(root).unwrap();
    let q = agentrec_core::search::SearchQuery {
        pattern: "needle-tool".to_string(),
        regex: false,
    };
    let direct = view.search(&q, None).unwrap();
    let direct_json = serde_json::to_value(&direct).unwrap();

    assert_eq!(
        cli_json, direct_json,
        "no clock field on SearchPage — full byte-for-byte value equality must hold"
    );
    // Top-level key-set proof, mirroring stats.rs's pattern.
    let obj = cli_json.as_object().expect("top-level object");
    assert_eq!(
        obj.keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        ["page", "dangling_prompt_refs"]
            .into_iter()
            .map(String::from)
            .collect(),
        "SearchPage's declared field set"
    );
}

// ---------------------------------------------------------------------------
// Zero-write invariant (mirrors stats.rs's pattern)
// ---------------------------------------------------------------------------

fn snapshot_agentrec_tree(root: &Path) -> std::collections::BTreeMap<String, u64> {
    fn walk(base: &Path, dir: &Path, out: &mut std::collections::BTreeMap<String, u64>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if let Ok(meta) = entry.metadata() {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, meta.len());
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(&root.join(".agentrec"), &root.join(".agentrec"), &mut out);
    out
}

#[test]
fn search_and_search_json_write_nothing_under_agentrec_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));
    let hash = store.put(b"the needle prompt").unwrap();
    let mut t = base_turn("t_ZW0000000000000000000001", Some("claude"), None, root);
    t.prompt_ref = Some(hash);
    seed_turn(root, t);

    let tree_before = snapshot_agentrec_tree(root);

    for args in [
        vec!["search", "needle"],
        vec!["search", "needle", "--json"],
        vec!["search", "needle", "--regex"],
    ] {
        let out = agentrec(root, &args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert_eq!(
            snapshot_agentrec_tree(root),
            tree_before,
            "{args:?} changed the .agentrec/ tree (new/removed/resized entry)"
        );
    }
}
