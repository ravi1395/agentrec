//! `agentrec bisect` (Phase 3.0 T5) — integration tests driving the real
//! binary against scripted fixture repositories. Every expected verdict is
//! hand-derived from the fixture the test builds, per the plan's
//! self-checking-fixture rule; nothing here is computed by running the code
//! path under test a second time.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads and writes its own tempdir fixtures; no attacker-supplied
//    FIFO to block on (same rationale as annotate.rs's, search.rs's and
//    stats.rs's identical allow). Scoped to tests — `cli/src` carries no new
//    allow.

use agentrec_core::record::{append_log, FileEntry, LogRecord, TurnRecord};
use agentrec_core::store::{hash_bytes, BlobStore};
use std::collections::BTreeMap;
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

fn init(root: &Path) {
    let out = Command::new(bin())
        .args(["init", "--no-hook", "--no-service", "--root"])
        .arg(root)
        .output()
        .expect("agentrec init");
    assert!(out.status.success(), "init failed: {out:?}");
}

fn fe(path: &str, before: Option<String>, op: &str) -> FileEntry {
    FileEntry {
        path: path.to_string(),
        before,
        after: Some("after-hash".to_string()),
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

fn turn(id: &str, root: &Path, grade: &str, ended: &str, files: Vec<FileEntry>) -> TurnRecord {
    TurnRecord {
        v: 1,
        id: id.to_string(),
        grade: grade.to_string(),
        truncated: false,
        started: ended.to_string(),
        ended: ended.to_string(),
        tool: Some("claude".to_string()),
        model: Some("opus-9".to_string()),
        session: None,
        root: root.to_string_lossy().to_string(),
        prompt_ref: None,
        prompt_excerpt: None,
        merges: vec![],
        imported: None,
        files_complete: None,
        origin: None,
        files,
    }
}

fn seed(root: &Path, r: LogRecord) {
    append_log(&root.join(".agentrec/log.jsonl"), &r).expect("seed record");
}

fn put(root: &Path, body: &str) -> String {
    BlobStore::new(root.join(".agentrec/objects"))
        .put(body.as_bytes())
        .expect("store blob")
}

/// A content hash of a whole directory tree: sorted relative paths joined with
/// their bytes' hashes. Used to MEASURE the read-only claim rather than assert
/// it — a probe that wrote into the working tree or `.agentrec/` changes this.
fn tree_hash(dir: &Path) -> String {
    let mut entries: BTreeMap<String, String> = BTreeMap::new();
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let meta = std::fs::symlink_metadata(&p).unwrap();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().to_string();
            if meta.file_type().is_dir() {
                out.insert(format!("{rel}/"), String::new());
                walk(base, &p, out);
            } else if meta.file_type().is_file() {
                out.insert(rel, hash_bytes(&std::fs::read(&p).unwrap()));
            } else {
                out.insert(rel, "<non-regular>".to_string());
            }
        }
    }
    walk(dir, dir, &mut entries);
    let joined: String = entries
        .iter()
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect::<Vec<_>>()
        .concat();
    hash_bytes(joined.as_bytes())
}

/// The fixture every search test uses.
///
/// HAND-COMPUTED. `f.txt` starts at "ok0" and is rewritten by four turns:
///   t1: before "ok0" → after "ok1"
///   t2: before "ok1" → after "ok2"
///   t3: before "ok2" → after "BAD"   <- the first bad turn
///   t4: before "BAD" → after "BAD2"
/// The working tree is left at "BAD2". The test command greps for `BAD`, so:
///   probe after t1 → f.txt = t2's before = "ok1"  → good
///   probe after t2 → f.txt = t3's before = "ok2"  → good
///   probe after t3 → f.txt = t4's before = "BAD"  → bad
///   probe after t4 → no subtraction → "BAD2"      → bad
/// The first turn whose post-state is bad is therefore **t3**.
fn four_turn_fixture(root: &Path) {
    init(root);
    std::fs::write(root.join("f.txt"), "BAD2").unwrap();
    let h0 = put(root, "ok0");
    let h1 = put(root, "ok1");
    let h2 = put(root, "ok2");
    let h3 = put(root, "BAD");
    for (id, before, ts) in [
        ("t1", h0, "2026-01-01T00:00:01.000Z"),
        ("t2", h1, "2026-01-01T00:00:02.000Z"),
        ("t3", h2, "2026-01-01T00:00:03.000Z"),
        ("t4", h3, "2026-01-01T00:00:04.000Z"),
    ] {
        seed(
            root,
            LogRecord::Turn(turn(
                id,
                root,
                "rich",
                ts,
                vec![fe("f.txt", Some(before), "modify")],
            )),
        );
    }
}

/// Exit 0 (good) while `f.txt` does NOT contain BAD.
const TEST_CMD: &str = "! grep -q BAD f.txt";

#[test]
fn binary_search_lands_on_the_hand_known_first_bad_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    four_turn_fixture(root);

    let before_tree = tree_hash(root);
    let out = agentrec(root, &["bisect", "--test", TEST_CMD]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "bisect failed: {out:?}");
    assert!(
        stdout.contains("first bad turn: t3"),
        "expected t3 (hand-derived above), got:\n{stdout}"
    );
    // Every render carries the fidelity sentence.
    assert!(
        stdout.contains("working tree minus later agent turns; not a historical snapshot"),
        "{stdout}"
    );
    // MEASURED, not asserted: the repository is byte-identical afterwards.
    assert_eq!(
        before_tree,
        tree_hash(root),
        "bisect must not write to the working tree or .agentrec/"
    );
}

#[test]
fn json_report_carries_the_verdict_and_the_fidelity_key() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    four_turn_fixture(root);
    let out = agentrec(root, &["bisect", "--test", TEST_CMD, "--json"]);
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(v["verdict"]["kind"], "first_bad");
    assert_eq!(v["verdict"]["turn_id"], "t3");
    // Probe points at which the command actually ran. Hand-derived: endpoints
    // are lo=-1 (baseline, trusted good) and hi=3 (t4, trusted bad), so the
    // candidates are indices 0..=2. Probe 1 at index 1 (t2) is good → lo=1;
    // probe 2 at index 2 (t3) is bad → hi=2; now hi-lo == 1 → answer t3.
    assert_eq!(v["probes"], 2, "{v}");
    assert!(v["fidelity"]
        .as_str()
        .unwrap()
        .contains("not a historical snapshot"));
    assert_eq!(v["unanswerable"].as_array().unwrap().len(), 0);
}

/// A `--good` that names a turn narrows the span; the answer is unchanged.
#[test]
fn explicit_good_and_bad_endpoints_are_honored() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    four_turn_fixture(root);
    let out = agentrec(
        root,
        &["bisect", "--test", TEST_CMD, "--good", "t1", "--bad", "t4"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{out:?}");
    assert!(stdout.contains("first bad turn: t3"), "{stdout}");
}

/// A turn whose `before` blob is missing blocks every probe that subtracts it,
/// and the search must then report the narrowest span it DID establish rather
/// than guess inside it.
///
/// HAND-DERIVED. t2's `before` hash is never stored. Endpoints are lo=-1
/// (baseline, trusted good) and hi=2 (t3, trusted bad); candidates are indices
/// 0 and 1.
///   - candidate 0 subtracts t2 and t3 → unanswerable (t2's blob is dangling);
///   - candidate 1 subtracts t3 only → answerable, leaving f.txt at t3's
///     before, "BAD" → the test fails → hi = 1. One probe has run.
///
/// The remaining candidate span is index 0 alone, which is still
/// unanswerable, so the verdict is the span seq[0..=1] = t1, t2 — narrower
/// than the starting t1,t2,t3, and never a guess between them.
#[test]
fn an_unanswerable_span_is_reported_not_guessed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::write(root.join("f.txt"), "BAD2").unwrap();
    let h0 = put(root, "ok0");
    let dangling = hash_bytes(b"never stored");
    let h2 = put(root, "BAD");
    for (id, before, ts) in [
        ("t1", h0, "2026-01-01T00:00:01.000Z"),
        ("t2", dangling, "2026-01-01T00:00:02.000Z"),
        ("t3", h2, "2026-01-01T00:00:03.000Z"),
    ] {
        seed(
            root,
            LogRecord::Turn(turn(
                id,
                root,
                "rich",
                ts,
                vec![fe("f.txt", Some(before), "modify")],
            )),
        );
    }

    let out = agentrec(root, &["bisect", "--test", TEST_CMD, "--json"]);
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(v["verdict"]["kind"], "ambiguous_span", "{v}");
    let ids: Vec<&str> = v["verdict"]["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["t1", "t2"], "{v}");
    assert_eq!(
        v["probes"], 1,
        "only candidate 1 had a materializable state"
    );
    let un = v["unanswerable"].as_array().unwrap();
    assert!(!un.is_empty(), "{v}");
    assert_eq!(un[0]["turn_id"], "t2");
    assert_eq!(un[0]["reason"], "dangling_blob");
}

/// `--flaky-retries 1` against a command whose two runs disagree: the probe
/// gets no verdict. The counter file lives OUTSIDE the scratch dir (which is
/// recreated per probe) so the alternation is deterministic.
#[test]
fn a_flaky_command_makes_the_probe_unanswerable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    four_turn_fixture(root);
    let counter = tmp.path().parent().unwrap().join(format!(
        "agentrec-bisect-flaky-{}.count",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&counter);
    // Alternates exit 0 / exit 1 across invocations, so the two runs of any
    // single probe always disagree.
    let cmd = format!(
        "n=$(cat {c} 2>/dev/null || echo 0); echo $((n+1)) > {c}; [ $((n % 2)) -eq 0 ]",
        c = counter.display()
    );

    let out = agentrec(
        root,
        &["bisect", "--test", &cmd, "--flaky-retries", "1", "--json"],
    );
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    let un = v["unanswerable"].as_array().unwrap();
    assert!(
        un.iter().any(|u| u["reason"] == "flaky_disagreement"),
        "expected a flaky_disagreement report, got {v}"
    );
    assert_eq!(v["verdict"]["kind"], "ambiguous_span", "{v}");
    let _ = std::fs::remove_file(&counter);
}

/// Two records under one id whose `files` differ are a genuine collision (the
/// `same_revert` collapse deliberately does NOT cover them), so resolving
/// `--good dup` must hard-error.
#[test]
fn an_ambiguous_good_id_is_a_hard_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    four_turn_fixture(root);
    let h = put(root, "other");
    // Same id as t1, different file entries → not collapsible.
    seed(
        root,
        LogRecord::Turn(turn(
            "t1",
            root,
            "rich",
            "2026-01-01T00:00:09.000Z",
            vec![fe("other.txt", Some(h), "modify")],
        )),
    );

    let out = agentrec(root, &["bisect", "--test", TEST_CMD, "--good", "t1"]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "must exit non-zero: {out:?}");
    assert!(stderr.contains("ambiguous turn id"), "{stderr}");
}

/// Bare turns are outside the default sequence; `--include-bare` admits them
/// and prints the misattribution warning.
#[test]
fn include_bare_admits_bare_turns_and_warns() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::write(root.join("f.txt"), "BAD2").unwrap();
    let h0 = put(root, "ok0");
    let h1 = put(root, "BAD");
    seed(
        root,
        LogRecord::Turn(turn(
            "b1",
            root,
            "bare",
            "2026-01-01T00:00:01.000Z",
            vec![fe("f.txt", Some(h0), "modify")],
        )),
    );
    seed(
        root,
        LogRecord::Turn(turn(
            "b2",
            root,
            "bare",
            "2026-01-01T00:00:02.000Z",
            vec![fe("f.txt", Some(h1), "modify")],
        )),
    );

    let out = agentrec(root, &["bisect", "--test", TEST_CMD]);
    assert!(!out.status.success(), "no eligible turns: {out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no turns to bisect"),
        "{out:?}"
    );

    let out = agentrec(root, &["bisect", "--test", TEST_CMD, "--include-bare"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{out:?}");
    // Endpoints lo=-1 / hi=1 (b2) with hi-lo == 2 → one candidate, index 0.
    // Probing after b1 leaves f.txt at b2's before, "BAD" → bad → hi=0, and
    // b1 is named. Bare turns are therefore probe candidates, not just
    // subtractions.
    assert!(stdout.contains("first bad turn: b1"), "{stdout}");
    assert!(
        stdout.contains("warning: --include-bare"),
        "the misattribution warning is mandatory: {stdout}"
    );
}

/// A mid-span daemon restart mints a recording gap. Bisect must still answer,
/// and must list the gap as a caveat (spec round-2 correction).
#[test]
fn a_recording_gap_is_listed_but_never_fatal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    four_turn_fixture(root);
    // Splice epochs around the turns: stop at :02.5, start at :02.9.
    seed(
        root,
        LogRecord::Epoch(agentrec_core::record::EpochRecord {
            v: 1,
            event: "stop".to_string(),
            ts: "2026-01-01T00:00:02.500Z".to_string(),
            dropped_signals: 0,
        }),
    );
    seed(
        root,
        LogRecord::Epoch(agentrec_core::record::EpochRecord {
            v: 1,
            event: "start".to_string(),
            ts: "2026-01-01T00:00:02.900Z".to_string(),
            dropped_signals: 0,
        }),
    );

    let out = agentrec(root, &["bisect", "--test", TEST_CMD, "--json"]);
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(
        v["verdict"]["turn_id"], "t3",
        "gaps must not change the answer"
    );
    let gaps = v["gap_windows"].as_array().unwrap();
    assert!(
        gaps.iter().any(|g| g["kind"] == "restart"),
        "the restart gap must be reported: {v}"
    );
}

/// `--help` is the only place a user reads the fidelity posture and the
/// no-sandbox warning before running an arbitrary command. Both are asserted
/// literally; the fidelity wording is kept byte-identical to
/// `agentrec_core::bisect::FIDELITY` (cross-referenced from main.rs's doc
/// comment, which clap cannot interpolate a constant into).
#[test]
fn help_carries_the_fidelity_and_no_sandbox_statements() {
    let out = Command::new(bin())
        .args(["bisect", "--help"])
        .output()
        .expect("run help");
    assert!(out.status.success(), "{out:?}");
    // Whitespace-normalized before matching: clap re-wraps the doc comment to
    // the terminal width, so a raw `contains` would be asserting where the
    // wrap happens to land rather than that the sentence is present.
    let help = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        help.contains("working tree minus later agent turns"),
        "{help}"
    );
    assert!(help.contains("not a historical snapshot"), "{help}");
    assert!(help.contains("NOT sandboxed"), "{help}");
    assert_eq!(
        agentrec_core::bisect::FIDELITY,
        "a probe state is the working tree minus later agent turns; not a historical snapshot",
        "if this constant changes, main.rs's Bisect doc comment must change with it"
    );
}
