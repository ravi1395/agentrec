//! Phase 3B integration tests: `attest derive` / `attest run` and the hook
//! capture path, against the real binary and the real fixture crate.
//! ACs: `IMPLEMENTATION.md` § "Phase 3B".
//!
//! Each test gets its OWN copy of the fixture crate in a tempdir (only the four
//! tracked files — the in-tree fixture also carries a multi-megabyte `target/`
//! from earlier builds), `git init`ed and `agentrec init`ed, so a test that
//! renames a test fn cannot touch the repo.
//!
//! **Two of the eighteen tests here run a REAL daemon** — `ac_p3_15` and
//! `ac_p3_16`, the two that assert on a turn id, both via `open_bracketed_turn`
//! — so the id they compare against is the one `daemon.rs::sync_journal` itself
//! wrote, and writer and reader are both proven. `open.json` is written BY HAND
//! in exactly one test, `ac_p3_25_a_stale_open_json_yields_unjoined_evidence_on_a_clean_tree`,
//! whose whole subject is a journal left behind with no daemon running. The
//! remaining fifteen need no daemon at all.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures; production reads stay
//    lint-enforced (clippy.toml).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

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

fn signal_lines(root: &Path) -> usize {
    std::fs::read_to_string(root.join(".agentrec/signal.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
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

fn spawn_record(root: &Path) -> Child {
    Command::new(bin())
        .args(["record", "--root", root.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn record")
}

fn send_hook(root: &Path, payload: &str) -> Output {
    let mut child = Command::new(bin())
        .args(["hook", "claude", "--root", root.to_str().unwrap()])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook");
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn poll_until<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Start a real daemon and open a real hook bracket, returning the child and
/// the turn id the daemon itself minted and mirrored to `.agentrec/open.json`.
///
/// A REAL daemon, not a hand-written journal: `open_turn_id` now refuses a
/// journal with no live daemon (the staleness gate, AC-ATTEST-P3-25), so these
/// tests would be asserting on `None` otherwise. It also removes the earlier
/// caveat that only the reader was proven — the id asserted below is the one
/// `daemon.rs::sync_journal` wrote.
fn open_bracketed_turn(root: &Path) -> (Child, String) {
    let daemon = spawn_record(root);
    send_hook(
        root,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"go"}"#,
    );
    // The start signal alone is what opens the turn — measured: a daemon that
    // receives only `UserPromptSubmit` mirrors `{"source":"bracket", …,
    // "files":[]}` to `open.json` with no file mutation at all. This write is
    // kept anyway so the open turn also carries a file, which is the ordinary
    // shape the evidence below is joined to.
    std::fs::write(root.join("src/marker.rs"), "// bracket marker\n").unwrap();
    let id = poll_until(Duration::from_secs(20), || {
        let text = std::fs::read_to_string(root.join(".agentrec/open.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        v.get("id")?.as_str().map(String::from)
    });
    match id {
        Some(id) => (daemon, id),
        None => panic!("the daemon never opened a turn / mirrored open.json"),
    }
}

fn stop_daemon(mut daemon: Child) {
    let _ = daemon.kill();
    let _ = daemon.wait();
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

/// AC-ATTEST-P3-15.
#[test]
fn ac_p3_15_run_writes_evidence_joined_to_the_open_turn() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    let (daemon, turn) = open_bracketed_turn(root);

    ok(agentrec(root, &["attest", "run", "--", "cargo", "test"]));
    stop_daemon(daemon);

    let events = attest_events(root);
    let evidence = of_kind(&events, "evidence");
    assert_eq!(evidence.len(), 5, "one per known test: {evidence:#?}");
    assert!(
        evidence.iter().all(|e| e["turn_id"] == turn),
        "every capture joins the daemon's own open turn {turn}: {evidence:#?}"
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
    assert_eq!(status_json(root)["evidenced"], 5);
}

/// AC-ATTEST-P3-16 — the same capture with no wrapper, arriving through the
/// hook that already fires for every tool call.
#[test]
fn ac_p3_16_hook_post_tool_use_bash_writes_the_same_evidence() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    let (daemon, turn) = open_bracketed_turn(root);
    let signals_before = signal_lines(root);

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
    ok(send_hook(root, &payload.to_string()));
    stop_daemon(daemon);

    let events = attest_events(root);
    let evidence = of_kind(&events, "evidence");
    assert_eq!(evidence.len(), 5, "{events:#?}");
    assert!(evidence.iter().all(|e| e["turn_id"] == turn));

    // A captured test run is evidence, not a turn boundary: the arm must not
    // have fallen through and appended a stop signal, which would have closed
    // the very bracket this evidence is joined to.
    assert_eq!(
        signal_lines(root),
        signals_before,
        "a captured PostToolUse must append no signal"
    );
}

/// AC-ATTEST-P3-25 (integration half) — a journal with no live daemon is
/// stale, so evidence is recorded UNJOINED rather than attributed to a turn
/// that will never be persisted. Also the clean-tree half of the dirty bit.
#[test]
fn ac_p3_25_a_stale_open_json_yields_unjoined_evidence_on_a_clean_tree() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    // The shape `daemon.rs::sync_journal` writes — left behind by a daemon
    // that is not running.
    std::fs::write(
        root.join(".agentrec/open.json"),
        serde_json::json!({
            "source": "bracket", "tool": "claude", "prompt": null, "session": null,
            "opened_wall_ms": 1u64, "last_change_wall_ms": 2u64,
            "root": root.to_string_lossy(), "files": [],
            "id": "t_01ARZ3NDEKTSV4RRFFQ69G5FAV",
        })
        .to_string(),
    )
    .unwrap();

    ok(agentrec(root, &["attest", "run", "--", "cargo", "test"]));

    let events = attest_events(root);
    let evidence = of_kind(&events, "evidence");
    assert_eq!(evidence.len(), 5);
    assert!(
        evidence.iter().all(|e| e["turn_id"].is_null()),
        "a stale journal must not be joined: {evidence:#?}"
    );
    // The tree was committed clean and both `target/` and `.agentrec/` are
    // gitignored, so nothing here is dev-loop-only.
    assert!(
        evidence.iter().all(|e| e["dirty"] == false),
        "a clean tree must not be dev-loop-only: {evidence:#?}"
    );
    assert_eq!(status_json(root)["dev_loop_only"], 0);
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
    // ...and no CAS blob is left behind either. Storing the captured output
    // before knowing any event would cite it minted one orphan object per
    // undeclared run — a blob nothing references, which is exactly what
    // `purge --orphans` exists to clean up.
    let objects = root.join(".agentrec/objects");
    let blobs: Vec<_> = walk_files(&objects);
    assert!(
        blobs.is_empty(),
        "an all-undeclared run must store no blob, found {blobs:?}"
    );
}

/// Every regular file under `dir`, recursively. Used to count CAS objects.
fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_files(&path));
        } else {
            out.push(path);
        }
    }
    out
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

/// AC-ATTEST-P3-24 (purge half) — reproduced before the fix: a root whose only
/// CAS blob was cited by `evidence` events had that blob archived by
/// `purge --orphans`, because the protect set harvested `log.jsonl` +
/// `open.json` + `memory.jsonl` and not `attest.jsonl`.
#[test]
fn ac_p3_24_purge_orphans_protects_a_blob_cited_only_by_attest_jsonl() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    ok(agentrec(root, &["attest", "run", "--", "cargo", "test"]));

    let objects = root.join(".agentrec/objects");
    let before = walk_files(&objects);
    assert_eq!(
        before.len(),
        1,
        "fixture precondition: exactly one blob, cited only by attest.jsonl"
    );
    let evidence = attest_events(root);
    let evidence = of_kind(&evidence, "evidence");
    assert_eq!(evidence.len(), 5);
    let blob = evidence[0]["output_blob"].as_str().unwrap().to_string();
    // Nothing else in the repo cites it.
    let log = std::fs::read_to_string(root.join(".agentrec/log.jsonl")).unwrap_or_default();
    let hex = blob.strip_prefix("sha256:").unwrap();
    assert!(!log.contains(hex), "log.jsonl must not cite the blob");

    let out = ok(agentrec(root, &["purge", "--orphans"]));
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        text.contains("0 orphan"),
        "an attest-cited blob is not an orphan: {text}"
    );
    assert_eq!(
        walk_files(&objects),
        before,
        "purge --orphans reclaimed a blob that attest.jsonl still cites"
    );
}

/// AC-ATTEST-P3-27 — `init` installs the `PostToolUse[Bash]` capture hook,
/// `doctor` passes with it present, and `uninstall` removes it.
#[test]
fn ac_p3_27_init_installs_doctor_accepts_and_uninstall_removes_the_capture_hook() {
    let tmp = fixture_repo();
    let root = tmp.path();
    // `fixture_repo` inits with --no-hook; install the hooks for real here.
    ok(agentrec(root, &["init", "--no-service"]));

    let settings_path = root.join(".claude/settings.local.json");
    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    let post = settings["hooks"]["PostToolUse"]
        .as_array()
        .expect("PostToolUse installed");
    let ours = post
        .iter()
        .find(|e| {
            e["hooks"][0]["command"]
                .as_str()
                .is_some_and(|c| c.contains("agentrec hook claude"))
        })
        .expect("our PostToolUse entry");
    assert_eq!(ours["matcher"], "Bash", "scoped by matcher: {ours}");
    // The bracketing pair is unchanged and still matcher-less.
    for event in ["UserPromptSubmit", "Stop"] {
        assert!(settings["hooks"][event].as_array().is_some(), "{event}");
        assert!(
            settings["hooks"][event][0]["matcher"].is_null(),
            "{event} must stay unmatched"
        );
    }
    // Idempotent: a second init adds no duplicate.
    ok(agentrec(root, &["init", "--no-service"]));
    let again: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(again["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);

    // doctor's hook-presence check passes with the third hook present.
    let doctor = agentrec(root, &["doctor", "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let hook_check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "hook presence")
        .expect("hook presence check");
    assert_eq!(hook_check["status"], "pass", "{hook_check}");

    // uninstall removes all three, leaving no agentrec command behind.
    ok(agentrec(root, &["uninstall", "--no-service"]));
    let after = std::fs::read_to_string(&settings_path).unwrap_or_default();
    assert!(
        !after.contains("agentrec hook claude"),
        "uninstall left an agentrec hook behind: {after}"
    );
    let after_json: serde_json::Value =
        serde_json::from_str(&after).unwrap_or(serde_json::json!({}));
    assert!(
        after_json["hooks"].get("PostToolUse").is_none(),
        "the emptied PostToolUse key must be dropped: {after}"
    );
}

/// AC-ATTEST-P3-28 — a REAL failing test. Pins the `test result: FAILED.`
/// summary branch, that the trailing `failures:` block does not inflate the
/// per-test tally (its lines are bare names, not `test … ... ok` lines), and
/// that a failing run is ordinary evidence — `parse_failed` stays false.
#[test]
fn ac_p3_28_a_real_failing_test_is_captured_as_a_failed_outcome() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    // Flip one assert in the COPY so the test genuinely fails.
    edit(
        root,
        "src/lib.rs",
        "assert_eq!(add(2, 2), 4);",
        "assert_eq!(add(2, 2), 5);",
    );

    let out = agentrec(root, &["attest", "run", "--", "cargo", "test"]);
    assert_eq!(
        out.status.code(),
        Some(101),
        "cargo's own failure status must propagate: {out:?}"
    );

    let events = attest_events(root);
    let evidence = of_kind(&events, "evidence");
    let by_test: std::collections::BTreeMap<String, &serde_json::Value> = evidence
        .iter()
        .map(|e| (identity(&e["result"], "identity"), &e["result"]))
        .collect();

    let failed = by_test["attest_sample_crate::tests::unit_add_works"];
    assert_eq!(failed["outcome"], "failed", "{failed}");
    assert_eq!(
        failed["parse_failed"], false,
        "a failure is not a parse error"
    );
    assert!(failed["recipe_invalid"].is_null());

    // Its siblings IN THE SAME SECTION are unaffected — one failure must not
    // poison the target's other results, and the cross-check still balanced
    // (`1 passed; 1 failed; 1 ignored` against three per-test lines).
    assert_eq!(
        by_test["attest_sample_crate::tests::unit_double_works"]["outcome"],
        "passed"
    );
    assert_eq!(
        by_test["attest_sample_crate::tests::unit_ignored_test"]["outcome"],
        "ignored"
    );
    assert!(
        evidence
            .iter()
            .all(|e| e["result"]["parse_failed"] == false),
        "no section may fail closed on an ordinary test failure: {evidence:#?}"
    );
    // MEASURED, not assumed: cargo stops after the first target that fails, so
    // the `integration` target never runs and contributes NO evidence. This is
    // a real property of a bulk capture — a failing run records less than a
    // green one — pinned here rather than papered over.
    assert_eq!(by_test.len(), 3, "only the lib section ran: {by_test:#?}");
    assert!(
        !by_test.contains_key("integration::integration_add_works"),
        "cargo never ran the integration target: {by_test:#?}"
    );
}

/// AC-ATTEST-P3-20 (harness-crash half) — a run whose test binary dies before
/// libtest reports anything names no test, so nothing can be attributed. It
/// must still be visible: the raw output is retained in the CAS and the
/// unparseable section is counted on stderr. Before this fix the whole run
/// vanished — exit 101, zero events, no blob, no count.
///
/// The aborting test is appended to THIS tempdir copy only; the committed
/// fixture crate keeps its five tests (AC-ATTEST-P3-10 pins that count).
#[test]
fn ac_p3_20_a_harness_crash_retains_its_raw_output_and_is_counted() {
    let tmp = fixture_repo();
    let root = tmp.path();
    let lib = root.join("src/lib.rs");
    let text = std::fs::read_to_string(&lib).unwrap();
    std::fs::write(
        &lib,
        format!("{text}\n#[test]\nfn unit_aborts() {{ std::process::abort() }}\n"),
    )
    .unwrap();
    ok(agentrec(root, &["attest", "derive"]));

    // Scoped to the aborting test alone, which makes the shape deterministic:
    // libtest prints `running 1 test` and SIGABRT ends the process before any
    // per-test line or summary is written.
    let out = agentrec(
        root,
        &[
            "attest",
            "run",
            "--",
            "cargo",
            "test",
            "--lib",
            "unit_aborts",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(101),
        "cargo's own status still propagates: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        stderr.contains("1 section(s) unparseable"),
        "the crash must be counted, not silent: {stderr}"
    );

    // No event: the run named no test, and inventing an attribution would be
    // worse than the count.
    let events = attest_events(root);
    assert!(
        of_kind(&events, "evidence").is_empty(),
        "nothing may be attributed to a claim the run never named: {events:#?}"
    );

    // The raw output IS retained, and the reported hash resolves in the CAS.
    let blob = stderr
        .split("raw output retained at ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("stderr must name the retained blob: {stderr}"))
        .to_string();
    let hex = blob.strip_prefix("sha256:").expect("a sha256 ref");
    let path = root
        .join(".agentrec/objects")
        .join(&hex[..2])
        .join(&hex[2..]);
    let stored = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("blob {} missing: {e}", path.display()));
    assert!(
        stored.contains("running 1 test"),
        "the retained output must be the crashed run's own: {stored}"
    );
    assert!(
        stored.contains("SIGABRT") || stored.contains("signal: 6"),
        "stderr is retained alongside stdout: {stored}"
    );
}

/// AC-ATTEST-P3-29 — `PostToolUse` is never a turn boundary. `init` installs
/// `PostToolUse[Bash]` for every repo, so an ordinary Bash call that is not a
/// test runner reaches `cmds::hook`; before this fix it fell through to the
/// `_ => "stop"` arm and appended a stop signal, closing the open bracket.
/// Measured by the Fable skeptic gate at `74a0a2c` with a real daemon: one
/// prompt + two non-test Bash calls + `Stop` wrote start,stop,stop,stop and
/// produced three rich turns where bracketing requires one. This test pins the
/// MECHANISM (an unmatched `PostToolUse` appends a signal), not that count.
#[test]
fn ac_p3_29_no_post_tool_use_event_ever_emits_a_signal() {
    let tmp = fixture_repo();
    let root = tmp.path();
    ok(agentrec(root, &["attest", "derive"]));
    let signal = root.join(".agentrec/signal.jsonl");

    ok(send_hook(
        root,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"go"}"#,
    ));
    let after_start = std::fs::read(&signal).unwrap_or_default();
    assert_eq!(
        signal_lines(root),
        1,
        "precondition: the bracket is open with exactly one start signal"
    );

    // A Bash call that is not a test runner, and a non-Bash tool call.
    for payload in [
        r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"Bash",
            "tool_input":{"command":"git status"},
            "tool_response":{"stdout":"nothing to commit\n","stderr":""}}"#,
        r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"Bash",
            "tool_input":{"command":"ls"},"tool_response":{"stdout":"src\n","stderr":""}}"#,
        r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"Edit",
            "tool_input":{"file_path":"src/lib.rs"},"tool_response":{"stdout":"","stderr":""}}"#,
    ] {
        ok(send_hook(root, payload));
        assert_eq!(
            std::fs::read(&signal).unwrap_or_default(),
            after_start,
            "an unmatched PostToolUse must leave signal.jsonl byte-identical: {payload}"
        );
    }
    assert!(
        of_kind(&attest_events(root), "evidence").is_empty(),
        "an unmatched PostToolUse writes no attest evidence either"
    );

    // The two events that ARE turn boundaries still map as before.
    ok(send_hook(
        root,
        r#"{"hook_event_name":"Stop","session_id":"s1"}"#,
    ));
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&signal)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "exactly one start and one stop: {lines:#?}");
    assert_eq!(lines[0]["event"], "start");
    assert_eq!(lines[1]["event"], "stop");
}
