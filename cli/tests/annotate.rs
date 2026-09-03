//! `agentrec annotate` (Phase 3.0 T4) — integration tests driving the real
//! binary inside scripted git fixture repos. Every expected line number is
//! hand-derived from the fixture the test builds, per the plan's
//! self-checking-fixture rule.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads and writes its own tempdir fixtures; no attacker-supplied
//    FIFO to block on (same rationale as search.rs's and stats.rs's identical
//    allow). Scoped to tests — `cli/src` carries no new allow.

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

fn git(root: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("spawn git")
}

fn git_ok(root: &Path, args: &[&str]) -> String {
    let out = git(root, args);
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A git repo with a deterministic identity — a test must never depend on the
/// developer's global git config being set.
fn git_init(root: &Path) {
    let out = Command::new("git")
        .args(["init", "-q"])
        .arg(root)
        .output()
        .expect("git init");
    assert!(out.status.success(), "git init failed: {out:?}");
    git_ok(root, &["config", "user.email", "fixture@example.invalid"]);
    git_ok(root, &["config", "user.name", "Fixture"]);
}

fn commit_file(root: &Path, path: &str, contents: &str, message: &str) -> String {
    std::fs::write(root.join(path), contents).unwrap();
    git_ok(root, &["add", path]);
    git_ok(root, &["commit", "-q", "-m", message]);
    git_ok(root, &["rev-parse", "HEAD"])
}

/// `agentrec init` WITHOUT `git init` — for the not-a-git-repo case.
fn agentrec_init(root: &Path) {
    let out = Command::new(bin())
        .args(["init", "--no-hook", "--no-service", "--root"])
        .arg(root)
        .output()
        .expect("agentrec init");
    assert!(out.status.success(), "init failed: {out:?}");
}

fn init(root: &Path) {
    git_init(root);
    agentrec_init(root);
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

fn seed(root: &Path, r: LogRecord) {
    append_log(&root.join(".agentrec/log.jsonl"), &r).expect("seed record");
}

fn turn(id: &str, root: &Path, files: Vec<FileEntry>) -> TurnRecord {
    TurnRecord {
        v: 1,
        id: id.to_string(),
        grade: "rich".to_string(),
        truncated: false,
        started: "2020-06-01T00:00:00.000Z".to_string(),
        ended: "2020-06-01T00:00:05.000Z".to_string(),
        tool: Some("claude".to_string()),
        model: Some("opus-9".to_string()),
        session: None,
        root: root.to_string_lossy().to_string(),
        prompt_ref: None,
        prompt_excerpt: Some("append line three".to_string()),
        merges: vec![],
        imported: None,
        files_complete: None,
        origin: None,
        files,
    }
}

/// Task-local normalizer: commit hashes are minted by git at fixture time and
/// cannot be pinned. Scoped to this file — the shared golden NORMALIZE_TABLE
/// is untouched (T2's precedent for a query-time token).
fn normalize_shas(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find(|c: char| c.is_ascii_hexdigit()) {
        let (head, tail) = rest.split_at(pos);
        out.push_str(head);
        let run: String = tail.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if run.len() == 40 {
            out.push_str("<SHA>");
        } else {
            out.push_str(&run);
        }
        rest = &tail[run.len()..];
    }
    out.push_str(rest);
    out
}

/// Two commits: commit 1 is a human-written 2-line file, commit 2 appends a
/// third line that an agentrec turn claims. Hand-derived: blaming
/// `HEAD~1..HEAD` yields lines 1–2 on HEAD~1 and line 3 on HEAD;
/// the turn's before→after diff inserts exactly new-side line 3.
fn two_commit_fixture(root: &Path) {
    init(root);
    commit_file(root, "f.txt", "one\ntwo\n", "human writes two lines");
    let store = BlobStore::new(root.join(".agentrec/objects"));
    let before = store.put(b"one\ntwo\n").unwrap();
    let after = store.put(b"one\ntwo\nthree\n").unwrap();
    commit_file(root, "f.txt", "one\ntwo\nthree\n", "agent appends a line");
    seed(
        root,
        LogRecord::Turn(turn(
            "t_ANNOTATE00000000000001",
            root,
            vec![fe("f.txt", Some(before), Some(after), "modify")],
        )),
    );
}

const DISCLAIMER: &str = "line attribution is best-effort: turns record file-level snapshots, \
not per-line provenance — renames, reformats, and interleaved human+agent edits within one file \
can misattribute lines.";

// ---------------------------------------------------------------------------
// Disclaimer / line_precision in all three modes — golden-pinned
// ---------------------------------------------------------------------------

#[test]
fn text_output_is_golden_and_carries_the_disclaimer() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    two_commit_fixture(root);

    let out = agentrec(root, &["annotate", "HEAD~1..HEAD"]);
    assert!(out.status.success(), "{out:?}");
    let text = normalize_shas(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(
        text,
        format!(
            "annotate HEAD~1..HEAD\n\
             {DISCLAIMER}\n\
             f.txt\n\
             \x20 1-2 <SHA> human\n\
             \x20 3 <SHA> t_ANNOTATE00000000000001 (claude/opus-9, 2020-06-01T00:00:05.000Z) \"append line three\"\n\
             evicted_ranges=0\n"
        )
    );
}

#[test]
fn md_output_is_golden_and_carries_the_disclaimer() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    two_commit_fixture(root);

    let out = agentrec(root, &["annotate", "HEAD~1..HEAD", "--md"]);
    assert!(out.status.success(), "{out:?}");
    let text = normalize_shas(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(
        text,
        format!(
            "# annotate `HEAD~1..HEAD`\n\n\
             _{DISCLAIMER}_\n\n\
             ## `f.txt`\n\n\
             | lines | commit | attribution |\n\
             | --- | --- | --- |\n\
             | 1-2 | `<SHA>` | human |\n\
             | 3 | `<SHA>` | t_ANNOTATE00000000000001 (claude/opus-9, 2020-06-01T00:00:05.000Z) \"append line three\" |\n\n\
             evicted_ranges: 0\n"
        )
    );
}

#[test]
fn json_output_is_golden_and_carries_line_precision_best_effort() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    two_commit_fixture(root);

    let out = agentrec(root, &["annotate", "HEAD~1..HEAD", "--json"]);
    assert!(out.status.success(), "{out:?}");
    let json = normalize_shas(&String::from_utf8_lossy(&out.stdout));
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v["line_precision"], "best_effort",
        "the machine-readable form of the precision contract"
    );
    assert_eq!(
        v,
        serde_json::json!({
            "files": [{
                "path": "f.txt",
                "ranges": [
                    {"commit": "<SHA>", "start_line": 1, "line_count": 2,
                     "attribution": {"kind": "human"}},
                    {"commit": "<SHA>", "start_line": 3, "line_count": 1,
                     "attribution": {"kind": "turns", "turns": [{
                         "id": "t_ANNOTATE00000000000001",
                         "tool": "claude",
                         "model": "opus-9",
                         "ts": "2020-06-01T00:00:05.000Z",
                         "prompt_excerpt": "append line three"
                     }]}}
                ]
            }],
            "line_precision": "best_effort",
            "evicted_ranges": 0
        })
    );
}

#[test]
fn json_and_md_together_are_refused_by_clap() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    two_commit_fixture(root);
    let out = agentrec(root, &["annotate", "HEAD~1..HEAD", "--json", "--md"]);
    assert!(!out.status.success(), "{out:?}");
}

// ---------------------------------------------------------------------------
// Misattribution-shaped fixture (spec risk 2)
// ---------------------------------------------------------------------------

/// Human and agent edits interleaved in ONE file between two commits: the
/// human inserts at the top (shifting every later line) while the agent's
/// recorded snapshot pair describes a different intermediate state. Best-effort
/// promises only that this RENDERS and stays disclaimed — asserting the lines
/// come out correctly here would overclaim exactly what §3.0.3 forbids.
#[test]
fn interleaved_human_and_agent_edits_render_with_the_disclaimer() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    commit_file(root, "f.txt", "a\nb\nc\n", "base");

    let store = BlobStore::new(root.join(".agentrec/objects"));
    // The agent edited line 2 of the base file...
    let before = store.put(b"a\nb\nc\n").unwrap();
    let after = store.put(b"a\nB-agent\nc\n").unwrap();
    // ...but the human then prepended a line before the commit landed, so the
    // agent's content sits at line 3 in the committed file while the recorded
    // diff says new-side line 2.
    commit_file(
        root,
        "f.txt",
        "human-header\na\nB-agent\nc\n",
        "interleaved human + agent",
    );
    seed(
        root,
        LogRecord::Turn(turn(
            "t_INTERLEAVED0000000001",
            root,
            vec![fe("f.txt", Some(before), Some(after), "modify")],
        )),
    );

    let out = agentrec(root, &["annotate", "HEAD~1..HEAD"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(DISCLAIMER), "{text}");
    assert!(text.contains("f.txt"), "{text}");
    // Deliberately NO assertion about which line got which attribution.
}

// ---------------------------------------------------------------------------
// Gap
// ---------------------------------------------------------------------------

#[test]
fn a_range_in_a_gapped_history_renders_unattributable_gap() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    commit_file(root, "f.txt", "one\ntwo\n", "base");
    commit_file(
        root,
        "f.txt",
        "one\ntwo\nthree\n",
        "a change nothing recorded",
    );
    // stop → start with no turn covering the change = a Restart gap.
    for (event, ts) in [
        ("start", "2020-06-01T00:00:00.000Z"),
        ("stop", "2020-06-01T00:01:00.000Z"),
        ("start", "2020-06-01T00:02:00.000Z"),
    ] {
        seed(
            root,
            LogRecord::Epoch(agentrec_core::record::EpochRecord {
                v: 1,
                event: event.to_string(),
                ts: ts.to_string(),
                dropped_signals: 0,
            }),
        );
    }

    let out = agentrec(root, &["annotate", "HEAD~1..HEAD"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("unattributable (gap)"), "{text}");
    // The disclaimer itself contains "human+agent", so the check is scoped to
    // a rendered attribution cell, not the whole document.
    assert!(
        !text.contains(" human\n"),
        "a gapped history must never be guessed as human: {text}"
    );
}

// ---------------------------------------------------------------------------
// Evicted blob
// ---------------------------------------------------------------------------

#[test]
fn an_evicted_turn_blob_renders_unattributable_blob_evicted_and_is_counted() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    commit_file(root, "f.txt", "one\ntwo\n", "base");
    commit_file(root, "f.txt", "one\ntwo\nthree\n", "agent appends");
    // A turn touching f.txt whose snapshot blobs were purged (never written).
    let gone =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string();
    seed(
        root,
        LogRecord::Turn(turn(
            "t_EVICTED000000000000001",
            root,
            vec![fe("f.txt", Some(gone.clone()), Some(gone), "modify")],
        )),
    );

    let out = agentrec(root, &["annotate", "HEAD~1..HEAD"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("unattributable (blob evicted)"), "{text}");
    // Hand-derived: blame reports two ranges (lines 1–2 boundary, line 3 HEAD)
    // and BOTH fall to the evicted arm, since the only turn touching f.txt has
    // no derivable line set.
    assert!(text.contains("evicted_ranges=2"), "{text}");

    let out = agentrec(root, &["annotate", "HEAD~1..HEAD", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["evicted_ranges"], 2);
    assert_eq!(
        v["files"][0]["ranges"][0]["attribution"],
        serde_json::json!({"kind": "unattributable", "reason": "blob_evicted"})
    );
}

// ---------------------------------------------------------------------------
// Zero agentrec history
// ---------------------------------------------------------------------------

#[test]
fn a_range_with_zero_agentrec_history_is_all_human_and_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    commit_file(root, "f.txt", "one\ntwo\n", "base");
    commit_file(root, "f.txt", "one\ntwo\nthree\n", "another human change");

    let out = agentrec(root, &["annotate", "HEAD~1..HEAD", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let ranges = v["files"][0]["ranges"].as_array().unwrap();
    assert!(!ranges.is_empty(), "blame reported ranges: {v}");
    for r in ranges {
        assert_eq!(r["attribution"]["kind"], "human", "{v}");
    }
    assert_eq!(v["evicted_ranges"], 0);
}

// ---------------------------------------------------------------------------
// git spawn failures: clean typed errors, no panic
// ---------------------------------------------------------------------------

#[test]
fn a_root_that_is_not_a_git_repo_is_a_clean_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec_init(root); // deliberately no git init

    let out = agentrec(root, &["annotate", "HEAD~1..HEAD"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "clean exit, not a panic/signal: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("agentrec: git diff"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn a_bad_git_range_is_a_clean_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    two_commit_fixture(root);

    let out = agentrec(root, &["annotate", "no-such-ref..also-missing"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "clean exit, not a panic/signal: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("agentrec: git diff"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

// ---------------------------------------------------------------------------
// --json parity against an INDEPENDENTLY derived core result
// ---------------------------------------------------------------------------

/// `AnnotateResult` is a function of (ledger, git blame output), and git lives
/// in the CLI layer — so, unlike `search`'s parity test, there is no core call
/// that reproduces the CLI's input for free. The `BlameRange` vec here is
/// HAND-CONSTRUCTED from the fixture's known shape (a 3-line file, two
/// commits, lines 1–2 from HEAD~1 and line 3 from HEAD), with
/// only the commit hashes read back from git. That makes this an independent
/// derivation rather than the same parse run twice.
///
/// No clock field on `AnnotateResult`, so full value equality holds (the T3
/// shape), plus a top-level key-set proof.
#[test]
fn json_matches_a_hand_derived_core_annotate_result() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    two_commit_fixture(root);
    let head = git_ok(root, &["rev-parse", "HEAD"]);
    let base = git_ok(root, &["rev-parse", "HEAD~1"]);

    let view = agentrec_core::view::RepositoryView::open(root).unwrap();
    let direct = view.annotate(&[
        agentrec_core::annotate::BlameRange {
            path: "f.txt".to_string(),
            commit: base,
            start_line: 1,
            line_count: 2,
        },
        agentrec_core::annotate::BlameRange {
            path: "f.txt".to_string(),
            commit: head,
            start_line: 3,
            line_count: 1,
        },
    ]);
    let direct_json = serde_json::to_value(&direct).unwrap();

    let out = agentrec(root, &["annotate", "HEAD~1..HEAD", "--json"]);
    assert!(out.status.success(), "{out:?}");
    let cli_json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    assert_eq!(
        cli_json, direct_json,
        "no clock field on AnnotateResult — full value equality must hold"
    );
    let obj = cli_json.as_object().expect("top-level object");
    assert_eq!(
        obj.keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        ["files", "line_precision", "evicted_ranges"]
            .into_iter()
            .map(String::from)
            .collect(),
        "AnnotateResult's declared field set"
    );
}

// ---------------------------------------------------------------------------
// Zero-write invariant (mirrors search.rs's pattern)
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
fn annotate_writes_nothing_under_agentrec_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    two_commit_fixture(root);

    let tree_before = snapshot_agentrec_tree(root);
    assert!(!tree_before.is_empty(), "fixture has an .agentrec/ tree");

    for args in [
        vec!["annotate", "HEAD~1..HEAD"],
        vec!["annotate", "HEAD~1..HEAD", "--json"],
        vec!["annotate", "HEAD~1..HEAD", "--md"],
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
