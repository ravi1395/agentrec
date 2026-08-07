//! Black-box hardening tests for the pre-launch store/retention/purge audit
//! (findings A1–A7, `agentrec-core/src/store.rs`, `agentrec-core/src/
//! retention.rs`, `cli/src/purgecmd.rs`). Drives the real `agentrec` binary
//! against tempdir fixtures, mirroring `tests/integration.rs`'s helpers.
//!
//! Store/retention-internal invariants (A1 crafted-hash path validation, A3
//! dedup-hit mtime touch + eviction-pass grace window, A4 corrupt-dedup
//! healing, A5 never-evict-newest, A6 tmp-file create mode, A7 rename vs
//! dir-fsync split) have direct, deterministic unit tests inline in
//! `agentrec-core/src/store.rs` and `agentrec-core/src/retention.rs` — this
//! file covers what only the real CLI surfaces: A1's arbitrary-path-delete
//! reachable through `purge`, and A2's cross-type (prompt vs. snapshot)
//! keep-set union in both `purge` code paths.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures, which this harness created;
//    there is no attacker-supplied FIFO to block on, so the fsguard wrappers
//    buy nothing here. Production reads stay lint-enforced (clippy.toml).

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
    Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(root)
        .status()
        .unwrap();
    let out = agentrec(root, &["init", "--no-hook", "--no-service"]);
    assert!(out.status.success(), "init failed: {out:?}");
}

fn days_ago_rfc3339(days: u64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    agentrec_core::time::rfc3339(now_ms.saturating_sub(days * 86_400_000))
}

fn base_turn(
    id: &str,
    files: Vec<agentrec_core::record::FileEntry>,
) -> agentrec_core::record::TurnRecord {
    agentrec_core::record::TurnRecord {
        v: 1,
        id: id.to_string(),
        grade: "rich".into(),
        truncated: false,
        started: "2026-07-05T00:00:00.000Z".into(),
        ended: "2026-07-05T00:00:01.000Z".into(),
        tool: Some("claude".into()),
        model: None,
        session: None,
        root: "/repo".into(),
        prompt_ref: None,
        prompt_excerpt: None,
        merges: vec![],
        imported: None,
        files_complete: None,
        origin: None,
        files,
    }
}

fn seed_turn(root: &Path, turn: &agentrec_core::record::TurnRecord) {
    agentrec_core::record::append_log(
        &root.join(".agentrec/log.jsonl"),
        &agentrec_core::record::LogRecord::Turn(turn.clone()),
    )
    .expect("seed turn");
}

// --- A1: a crafted hash in the log must never let `purge` delete an
// arbitrary file outside the store. -----------------------------------

#[test]
fn purge_snapshots_before_ignores_planted_malformed_hash() {
    use agentrec_core::record::FileEntry;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // Plant a victim file completely outside the store/repo.
    let victim_dir = tempfile::tempdir().unwrap();
    let victim = victim_dir.path().join("planted-file.txt");
    std::fs::write(&victim, b"do not delete me").unwrap();

    // `object_path` used to `strip_prefix("sha256:")` then split at 2 hex
    // chars, so `"sha256:xx" + <abs path>` left an absolute `rest`
    // component that `PathBuf::join` silently substituted for the store
    // base — turning `store.remove` into an arbitrary-path delete.
    let evil_hash = format!("sha256:xx{}", victim.display());

    let mut old_turn = base_turn(
        "t_EVILHASH00000000000000001",
        vec![FileEntry {
            path: "x.rs".into(),
            before: None,
            after: Some(evil_hash),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
            link_kind: None,
            attribution: None,
        }],
    );
    old_turn.started = "2020-01-01T00:00:00.000Z".into();
    old_turn.ended = old_turn.started.clone();
    seed_turn(root, &old_turn);

    let out = agentrec(root, &["purge", "--snapshots-before", "2030-01-01"]);
    assert!(
        out.status.success(),
        "purge must not crash on a malformed hash: {out:?}"
    );

    assert!(
        victim.exists(),
        "planted file outside the store must survive purge"
    );
    assert_eq!(
        std::fs::read(&victim).unwrap(),
        b"do not delete me",
        "planted file content must be untouched"
    );
}

// --- A2: cross-type (prompt vs. snapshot) keep-set union. -------------

#[test]
fn default_purge_keeps_prompt_blob_also_referenced_as_a_snapshot() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    // Content shared between a prompt and a snapshot.
    let shared = store.put(b"shared prompt-and-snapshot content").unwrap();

    // An expired turn whose PROMPT blob is `shared` — default `purge`
    // (prompt-only, no `--snapshots-before`) would normally delete this.
    let mut old_prompt_turn = base_turn("t_A2PROMPT00000000000000001", vec![]);
    old_prompt_turn.started = days_ago_rfc3339(200);
    old_prompt_turn.ended = old_prompt_turn.started.clone();
    old_prompt_turn.prompt_ref = Some(shared.clone());
    old_prompt_turn.prompt_excerpt = Some("expired prompt".into());
    seed_turn(root, &old_prompt_turn);

    // A different (recent) turn whose SNAPSHOT `after` is the same content.
    let mut snapshot_turn = base_turn(
        "t_A2SNAPSHOT0000000000000001",
        vec![FileEntry {
            path: "shared.rs".into(),
            before: None,
            after: Some(shared.clone()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
            link_kind: None,
            attribution: None,
        }],
    );
    snapshot_turn.started = days_ago_rfc3339(1);
    snapshot_turn.ended = snapshot_turn.started.clone();
    seed_turn(root, &snapshot_turn);

    let out = agentrec(root, &["purge"]);
    assert!(out.status.success(), "purge failed: {out:?}");

    assert!(
        store.contains(&shared),
        "blob shared with a live snapshot ref must survive the prompt purge"
    );
}

#[test]
fn purge_snapshots_before_keeps_snapshot_blob_also_referenced_as_a_prompt() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let shared = store.put(b"shared snapshot-and-prompt content").unwrap();

    // An expired (before-cutoff) turn whose SNAPSHOT `after` is `shared` —
    // `--snapshots-before` would normally delete this.
    let mut old_snapshot_turn = base_turn(
        "t_A2SNAP0000000000000000001",
        vec![FileEntry {
            path: "shared.rs".into(),
            before: None,
            after: Some(shared.clone()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
            link_kind: None,
            attribution: None,
        }],
    );
    old_snapshot_turn.started = "2020-01-01T00:00:00.000Z".into();
    old_snapshot_turn.ended = old_snapshot_turn.started.clone();
    seed_turn(root, &old_snapshot_turn);

    // A turn (also before the cutoff — its own recency is irrelevant here)
    // whose PROMPT is the same content: A2's cross-type protection isn't
    // date-gated, so this alone must protect `shared` from the snapshot
    // purge below even though neither turn is "kept" by the normal
    // same-type (on/after-cutoff) keep-set.
    let mut prompt_turn = base_turn("t_A2PROMPT20000000000000001", vec![]);
    prompt_turn.started = "2020-06-01T00:00:00.000Z".into();
    prompt_turn.ended = prompt_turn.started.clone();
    prompt_turn.prompt_ref = Some(shared.clone());
    prompt_turn.prompt_excerpt = Some("still-needed prompt".into());
    seed_turn(root, &prompt_turn);

    let out = agentrec(root, &["purge", "--snapshots-before", "2021-01-01"]);
    // Cutoff is after BOTH turns' start dates, so neither turn is "kept" by
    // the normal same-type keep-set — only the A2 cross-type union
    // protects `shared`.
    assert!(
        out.status.success(),
        "purge --snapshots-before failed: {out:?}"
    );

    assert!(
        store.contains(&shared),
        "blob shared with a live prompt ref must survive the snapshot purge"
    );
}
