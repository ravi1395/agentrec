//! Pre-launch hardening tests for Batch E (CLI read/undo/init/uninstall —
//! `cli/src/{readcmds,initcmd,uninstallcmd,fmt}.rs` plus the non-ack-degraded
//! parts of `cmds.rs`). Drives the real `agentrec` binary against tempdir
//! fixtures, mirroring the helper patterns in `cli/tests/integration.rs`
//! (kept separate per file-ownership rules for this hardening round).

use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

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

fn base_turn(
    id: &str,
    files: Vec<agentrec_core::record::FileEntry>,
) -> agentrec_core::record::TurnRecord {
    agentrec_core::record::TurnRecord {
        v: 1,
        id: id.to_string(),
        grade: "rich".into(),
        truncated: false,
        started: "2026-07-06T00:00:00.000Z".into(),
        ended: "2026-07-06T00:00:01.000Z".into(),
        tool: Some("claude".into()),
        model: None,
        session: None,
        root: "/repo".into(),
        prompt_ref: None,
        prompt_excerpt: None,
        merges: vec![],
        files,
    }
}

#[allow(clippy::too_many_arguments)]
fn make_turn(
    id: &str,
    started: &str,
    ended: &str,
    prompt_excerpt: Option<&str>,
    files: Vec<agentrec_core::record::FileEntry>,
) -> agentrec_core::record::TurnRecord {
    agentrec_core::record::TurnRecord {
        v: 1,
        id: id.to_string(),
        grade: "rich".into(),
        truncated: false,
        started: started.to_string(),
        ended: ended.to_string(),
        tool: Some("claude".into()),
        model: None,
        session: None,
        root: "/repo".into(),
        prompt_ref: None,
        prompt_excerpt: prompt_excerpt.map(str::to_string),
        merges: vec![],
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

fn seed_epoch(root: &Path, event: &str, ts: &str) {
    agentrec_core::record::append_log(
        &root.join(".agentrec/log.jsonl"),
        &agentrec_core::record::LogRecord::Epoch(agentrec_core::record::EpochRecord {
            v: 1,
            event: event.to_string(),
            ts: ts.to_string(),
        }),
    )
    .expect("seed epoch");
}

/// New agentrec-tool (undo) turns appended to the log.
fn agentrec_turns(root: &Path) -> Vec<serde_json::Value> {
    let path = root.join(".agentrec/log.jsonl");
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| {
            v.get("type").and_then(|t| t.as_str()) == Some("turn")
                && v.get("tool").and_then(|t| t.as_str()) == Some("agentrec")
        })
        .collect()
}

/// Object-store path for a `sha256:<hex>` ref (mirrors
/// `agentrec_core::store::BlobStore`'s private fan-out layout) — used only to
/// corrupt a blob on disk for the E1 integrity test.
fn object_path(root: &Path, hash: &str) -> std::path::PathBuf {
    let hex = hash.strip_prefix("sha256:").unwrap();
    let (fan, rest) = hex.split_at(2);
    root.join(".agentrec/objects").join(fan).join(rest)
}

fn wall_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ---- E1: pre-flight integrity read + idempotent create-inverse ------------

#[test]
fn e1_corrupt_before_blob_refuses_and_mutates_nothing() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"before content\n").unwrap();
    let after = store.put(b"after content\n").unwrap();
    std::fs::write(root.join("a.rs"), b"after content\n").unwrap();

    // Corrupt the before-blob on disk directly — its bytes no longer hash
    // to its own address.
    std::fs::write(object_path(root, &before), b"TAMPERED\n").unwrap();

    let turn = base_turn(
        "t_E1CORRUPT00000000000000001",
        vec![FileEntry {
            path: "a.rs".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["undo", &turn.id, "--confirm"]);
    assert!(out.status.success(), "undo should not hard-error: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("REFUSE"), "stdout: {stdout}");
    assert!(
        stdout.contains("corrupt") || stdout.contains("hash mismatch"),
        "expected an honest corruption reason: {stdout}"
    );

    // Pre-flight caught it BEFORE any mutation — the file is untouched.
    assert_eq!(
        std::fs::read(root.join("a.rs")).unwrap(),
        b"after content\n",
        "no file mutation may occur when the before-blob is corrupt"
    );
    assert!(
        agentrec_turns(root).is_empty(),
        "an all-refused plan must not record an undo turn"
    );
}

#[test]
fn e1_create_inverse_with_already_deleted_file_succeeds() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let created = store.put(b"created\n").unwrap();
    // c.rs is NOT written to disk — something else already deleted it since
    // the turn; revert-of-create's goal state ("file gone") already holds.
    // (Absent-vs-`after` also trips the separate, correctly-behaving
    // modified-since gate — CLAUDE.md's two-predicates rule — so
    // --allow-modified is used here to reach execute_revert's create-inverse
    // branch directly, which is what E1(b) is actually about: idempotent
    // handling of an already-absent target, not bypassing modified-since.)
    let turn = base_turn(
        "t_E1IDEMPOTENT000000000000001",
        vec![FileEntry {
            path: "c.rs".into(),
            before: None,
            after: Some(created),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn);
    assert!(!root.join("c.rs").exists());

    let out = agentrec(root, &["undo", &turn.id, "--allow-modified", "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("reverted 1 file"),
        "expected a successful revert: {stdout}"
    );
    assert!(!root.join("c.rs").exists());
    assert_eq!(agentrec_turns(root).len(), 1, "undo is still a real turn");
}

// ---- E2/E3: interval-aware recording-gap staleness -------------------------

#[test]
fn e2_clean_stop_start_gap_after_turn_marks_blame_stale() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let after = store.put(b"original\n").unwrap();
    let turn = make_turn(
        "t_E2CLEANGAP0000000000000001",
        "2026-07-06T09:00:00.000Z",
        "2026-07-06T09:00:00.000Z",
        Some("write g.rs"),
        vec![FileEntry {
            path: "g.rs".into(),
            before: None,
            after: Some(after),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn);

    // A CLEAN restart (stop, then later start) after the turn ended — no
    // crash, both epochs balanced, but still an uncovered interval.
    seed_epoch(root, "stop", "2026-07-06T09:01:00.000Z");
    seed_epoch(root, "start", "2026-07-06T09:02:00.000Z");

    // On-disk content diverges from the turn's recorded `after`.
    std::fs::write(root.join("g.rs"), b"changed-somewhere\n").unwrap();

    let out = agentrec(root, &["blame", "g.rs"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("attribution stale — recording gap"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("human-edited since"),
        "must not guess across an uncovered restart: {stdout}"
    );
}

#[test]
fn e3_line_added_during_gap_is_reported_stale_not_predating() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    // Turn A: created f.rs with two lines.
    let a_after = store.put(b"a\nb\n").unwrap();
    let turn_a = make_turn(
        "t_E3TURNA0000000000000000001",
        "2026-07-06T09:00:00.000Z",
        "2026-07-06T09:00:00.000Z",
        Some("create f.rs"),
        vec![FileEntry {
            path: "f.rs".into(),
            before: None,
            after: Some(a_after),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn_a);

    // Gap: a clean restart between turn A and turn C. During this window,
    // something outside recording added a third line ("c") — invisible to
    // any turn.
    seed_epoch(root, "stop", "2026-07-06T09:01:00.000Z");
    seed_epoch(root, "start", "2026-07-06T09:02:00.000Z");

    // Turn C: touches f.rs again, but its recorded `before` ALREADY includes
    // the externally-added "c" line (it started after the gap) — its diff
    // only introduces the b->B change, never "c".
    let c_before = store.put(b"a\nb\nc\n").unwrap();
    let c_after = store.put(b"a\nB\nc\n").unwrap();
    let turn_c = make_turn(
        "t_E3TURNC0000000000000000001",
        "2026-07-06T09:05:00.000Z",
        "2026-07-06T09:05:00.000Z",
        Some("tweak line two"),
        vec![FileEntry {
            path: "f.rs".into(),
            before: Some(c_before),
            after: Some(c_after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn_c);

    // On-disk matches turn C's `after` exactly — unmodified since the later
    // turn, so the file-level modified-since gate does NOT fire here; this
    // isolates the line-level fallback under test.
    std::fs::write(root.join("f.rs"), b"a\nB\nc\n").unwrap();

    let out = agentrec(root, &["blame", "f.rs:3"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("attribution stale — recording gap"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("before recording began"),
        "a gap-hidden origin must not be reported as predating all recording: {stdout}"
    );
}

// ---- E6: undo --files naming an unmatched path -----------------------------

#[test]
fn e6_undo_files_unmatched_path_refuses_and_mutates_nothing() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let after = store.put(b"content\n").unwrap();
    std::fs::write(root.join("a.rs"), b"content\n").unwrap();
    let turn = base_turn(
        "t_E6FILTER00000000000000001",
        vec![FileEntry {
            path: "a.rs".into(),
            before: None,
            after: Some(after),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["undo", &turn.id, "--files", "typo.rs", "--confirm"]);
    assert_eq!(out.status.code(), Some(1), "out: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("typo.rs"), "stderr: {stderr}");

    assert_eq!(
        std::fs::read(root.join("a.rs")).unwrap(),
        b"content\n",
        "nothing may be mutated when --files names an unmatched path"
    );
    assert!(agentrec_turns(root).is_empty());
}

// ---- E7: terminal-escape sanitization at excerpt render sites --------------

#[test]
fn e7_prompt_escape_sequence_never_reaches_stdout_raw() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let after = store.put(b"content\n").unwrap();
    let evil_prompt = "hello \x1b]0;evil\x07 world";
    let turn = make_turn(
        "t_E7ESCAPE0000000000000000001",
        "2026-07-06T09:00:00.000Z",
        "2026-07-06T09:00:00.000Z",
        Some(evil_prompt),
        vec![FileEntry {
            path: "e.rs".into(),
            before: None,
            after: Some(after),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn);
    std::fs::write(root.join("e.rs"), b"content\n").unwrap();

    // `log` (cmds.rs render site).
    let out = agentrec(root, &["log"]);
    assert!(out.status.success(), "log failed: {out:?}");
    assert!(
        !out.stdout.contains(&0x1bu8),
        "log stdout carried a raw ESC"
    );
    assert!(
        !out.stdout.contains(&0x07u8),
        "log stdout carried a raw BEL"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("hello"),
        "plain text must survive sanitization"
    );

    // `blame <file>` (readcmds.rs render_turn site).
    let out = agentrec(root, &["blame", "e.rs"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    assert!(!out.stdout.contains(&0x1bu8));
    assert!(!out.stdout.contains(&0x07u8));

    // `show <turn>` bare header (also render_turn).
    let out = agentrec(root, &["show", &turn.id]);
    assert!(out.status.success(), "show failed: {out:?}");
    assert!(!out.stdout.contains(&0x1bu8));
    assert!(!out.stdout.contains(&0x07u8));
}

// ---- E8: concurrent-undo guard refusal --------------------------------------

fn write_guard(root: &Path, paths: &[&str], until_ms: u64) {
    let dir = root.join(".agentrec");
    std::fs::create_dir_all(&dir).unwrap();
    let json = serde_json::json!({ "paths": paths, "until_ms": until_ms });
    std::fs::write(
        dir.join("undo-guard.json"),
        serde_json::to_string(&json).unwrap(),
    )
    .unwrap();
}

#[test]
fn e8_live_guard_refuses_concurrent_undo() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"v1\n").unwrap();
    let after = store.put(b"v2\n").unwrap();
    std::fs::write(root.join("a.rs"), b"v2\n").unwrap();
    let turn = base_turn(
        "t_E8LIVEGUARD00000000000001",
        vec![FileEntry {
            path: "a.rs".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn);

    write_guard(root, &["a.rs"], wall_now_ms() + 60_000);

    let out = agentrec(root, &["undo", &turn.id, "--confirm"]);
    assert_eq!(out.status.code(), Some(1), "out: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already in progress") || stderr.contains("concurrent"),
        "stderr: {stderr}"
    );
    assert_eq!(
        std::fs::read(root.join("a.rs")).unwrap(),
        b"v2\n",
        "a live guard must block the mutation entirely"
    );
    assert!(agentrec_turns(root).is_empty());
}

#[test]
fn e8_expired_guard_does_not_block_undo() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"v1\n").unwrap();
    let after = store.put(b"v2\n").unwrap();
    std::fs::write(root.join("a.rs"), b"v2\n").unwrap();
    let turn = base_turn(
        "t_E8EXPIREDGUARD0000000001",
        vec![FileEntry {
            path: "a.rs".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn);

    // Long-expired guard (well in the past).
    write_guard(root, &["a.rs"], 1_000);

    let out = agentrec(root, &["undo", &turn.id, "--confirm"]);
    assert!(out.status.success(), "undo should proceed: {out:?}");
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), b"v1\n");
    assert_eq!(agentrec_turns(root).len(), 1);
}

// ---- E9: perms repair is reported, not silently folded into "nothing changed"

#[cfg(unix)]
#[test]
fn e9_perms_drift_repair_is_not_reported_as_nothing_changed() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // Loosen config.toml's mode below the 0600 init locks it to.
    let config = root.join(".agentrec/config.toml");
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o644)).unwrap();

    let out = agentrec(root, &["init", "--no-hook", "--no-service"]);
    assert!(out.status.success(), "re-init failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("already initialized — nothing changed"),
        "a genuine perms repair must not be reported as a no-op: {stdout}"
    );

    let mode = std::fs::metadata(&config).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "the drifted mode must be repaired");
}

#[cfg(unix)]
#[test]
fn e9_second_rerun_with_no_drift_is_still_a_real_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // No perms drift this time — genuinely nothing to do.
    let out = agentrec(root, &["init", "--no-hook", "--no-service"]);
    assert!(out.status.success(), "re-init failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("already initialized — nothing changed"),
        "a genuine no-op re-run must still say so: {stdout}"
    );
}
