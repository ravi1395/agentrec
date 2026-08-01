//! F9 (red team round 2) — `agentrec purge --path <PATTERN>`, driven through
//! the real binary.
//!
//! The unit tests in `cli/src/purgecmd.rs` pin which blobs move. These pin the
//! things only the shipped CLI can show: the clap-level refusal of flag
//! combinations, and the reporting the user actually reads — in particular the
//! shared-blob count, which is the honest half of a path-targeted purge over a
//! content-addressed store (identical bytes are one object, so "forget file X"
//! cannot always be granted in full).

use std::path::Path;
use std::process::{Command, Output};

use agentrec_core::store::BlobStore;

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

fn file_entry(path: &str, before: Option<&str>, after: Option<&str>) -> String {
    let j = |v: Option<&str>| match v {
        Some(h) => format!("\"{h}\""),
        None => "null".to_string(),
    };
    format!(
        "{{\"path\":\"{path}\",\"before\":{},\"after\":{},\"op\":\"modify\"}}",
        j(before),
        j(after)
    )
}

fn turn_line(id: &str, files: &str) -> String {
    format!(
        "{{\"type\":\"turn\",\"v\":1,\"id\":\"{id}\",\"grade\":\"rich\",\
         \"started\":\"2026-01-01T00:00:00.000Z\",\
         \"ended\":\"2026-01-01T00:00:01.000Z\",\"root\":\"/x\",\
         \"files\":{files}}}\n"
    )
}

/// A store + `log.jsonl` where `secrets/prod.yaml` has one blob of its own and
/// one whose identical bytes are also recorded under `docs/example.yaml`.
fn shared_blob_fixture() -> (tempfile::TempDir, BlobStore, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let dir = root.join(".agentrec");
    std::fs::create_dir_all(&dir).unwrap();
    let store = BlobStore::new(dir.join("objects"));

    let exclusive = store.put(b"bytes only the leaked file ever had").unwrap();
    let shared = store
        .put(b"bytes two different paths both recorded")
        .unwrap();
    let files = format!(
        "[{},{}]",
        file_entry("secrets/prod.yaml", Some(&shared), Some(&exclusive)),
        file_entry("docs/example.yaml", None, Some(&shared))
    );
    std::fs::write(dir.join("log.jsonl"), turn_line("t_A", &files)).unwrap();
    (tmp, store, exclusive, shared)
}

#[test]
fn purge_path_reports_what_it_archived_and_what_it_kept_as_shared() {
    let (tmp, store, exclusive, shared) = shared_blob_fixture();
    let root = tmp.path();

    let out = agentrec(root, &["purge", "--path", "secrets/prod.yaml"]);
    assert!(out.status.success(), "purge --path failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("purged 1 snapshot blob(s)") && stdout.contains("secrets/prod.yaml"),
        "must report the reclaim and name the pattern: {stdout}"
    );
    assert!(
        stdout.contains("1 blob(s) KEPT"),
        "the shared blob must be reported, not silently skipped: {stdout}"
    );
    // The honesty line: log.jsonl still cites these hashes, so the user must
    // be told what `diff`/`undo` will now do.
    assert!(
        stdout.contains("log.jsonl is unchanged")
            && stdout.contains("(snapshot unavailable)")
            && stdout.contains("refusing to restore"),
        "must disclose the read-side consequence: {stdout}"
    );

    assert!(!store.contains(&exclusive), "exclusive blob reclaimed");
    assert!(store.contains(&shared), "shared blob kept");
}

// The read-side consequence the command promises, measured against the real
// binary rather than asserted from the source.
#[test]
fn purge_path_leaves_diff_reporting_the_snapshot_unavailable() {
    let (tmp, _store, _exclusive, _shared) = shared_blob_fixture();
    let root = tmp.path();

    let before = agentrec(root, &["diff", "t_A"]);
    assert!(
        String::from_utf8_lossy(&before.stdout).contains("secrets/prod.yaml"),
        "precondition: diff renders the file before the purge"
    );

    assert!(agentrec(root, &["purge", "--path", "secrets/prod.yaml"])
        .status
        .success());

    let after = agentrec(root, &["diff", "t_A"]);
    let stdout = String::from_utf8_lossy(&after.stdout);
    assert!(
        stdout.contains("(snapshot unavailable)"),
        "diff must report the blob as unavailable, never fabricate content: {stdout}"
    );
}

// The other half of the read-side consequence the `--path` help text promises.
// `undo`'s preview is the honest place to check it: `build_plan` does an
// integrity READ of every `before` blob up front, so a purged entry must come
// back REFUSED before anything is written, never half-reverted.
//
// THE FIXTURE IS THE FINDING. A first version of this test reused
// `shared_blob_fixture`, whose file does not exist on disk — so `undo` printed
// only `undo t_A (—)`: the entry was EXCLUDEd as modified-since long before
// the before-blob check ran, and the refusal string never appeared. The
// wording of `--path`'s help text was corrected against that measurement
// rather than the other way round. Reaching the refusal requires the file to
// be otherwise unmodified, which is what this fixture arranges.
#[test]
fn purge_path_leaves_undo_refusing_the_entry_before_it_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let dir = root.join(".agentrec");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(root.join("secrets")).unwrap();
    let store = BlobStore::new(dir.join("objects"));

    let before_bytes = b"the pre-rotation credential".as_slice();
    let after_bytes = b"the post-rotation credential".as_slice();
    let before = store.put(before_bytes).unwrap();
    let after = store.put(after_bytes).unwrap();
    // On-disk content == the turn's `after`, so the entry is NOT modified-since
    // and `build_plan` reaches its integrity read of `before`.
    std::fs::write(root.join("secrets/prod.yaml"), after_bytes).unwrap();
    std::fs::write(
        dir.join("log.jsonl"),
        turn_line(
            "t_A",
            &format!(
                "[{}]",
                file_entry("secrets/prod.yaml", Some(&before), Some(&after))
            ),
        ),
    )
    .unwrap();

    assert!(agentrec(root, &["purge", "--path", "secrets/prod.yaml"])
        .status
        .success());

    // Preview only (no --confirm): nothing is written either way.
    let out = agentrec(root, &["undo", "t_A"]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("prior snapshot unavailable") && text.contains("refusing to restore"),
        "undo must refuse the purged entry, not attempt it: {text}"
    );
    // And the file on disk is untouched by the preview.
    assert_eq!(
        std::fs::read(root.join("secrets/prod.yaml")).unwrap(),
        after_bytes
    );
}

// `--path` is surgical; combining it with the class/date-based ops is refused
// at the argument layer rather than silently doing both.
#[test]
fn purge_path_refuses_to_combine_with_the_other_purge_flags() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".agentrec")).unwrap();

    for other in [
        "--orphans",
        "--all-prompts",
        "--memories-retracted",
        "--log-duplicates",
        "--signals-consumed",
    ] {
        let out = agentrec(root, &["purge", "--path", "secrets", other]);
        assert!(
            !out.status.success(),
            "purge --path {other} must be refused, not run"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("cannot be used with"),
            "clap must name the conflict for {other}: {stderr}"
        );
    }
}

// The pattern language, end to end: a directory prefix and a glob both select,
// and neither reaches outside what it names.
#[test]
fn purge_path_accepts_a_directory_prefix_and_a_glob() {
    for pattern in ["secrets", "secrets/*.yaml", "**/prod.yaml"] {
        let (tmp, store, exclusive, _shared) = shared_blob_fixture();
        let root = tmp.path();
        let out = agentrec(root, &["purge", "--path", pattern]);
        assert!(out.status.success(), "purge --path {pattern} failed");
        assert!(
            !store.contains(&exclusive),
            "pattern {pattern} must select secrets/prod.yaml"
        );
    }
}

#[test]
fn purge_path_on_a_pattern_that_matches_nothing_says_so() {
    let (tmp, store, exclusive, shared) = shared_blob_fixture();
    let root = tmp.path();

    let out = agentrec(root, &["purge", "--path", "nowhere/at/all.txt"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no recorded file entry matches"),
        "must say the pattern matched nothing: {stdout}"
    );
    assert!(store.contains(&exclusive) && store.contains(&shared));
}
