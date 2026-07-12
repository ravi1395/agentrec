//! Pre-launch hardening tests for Batch E (CLI read/undo/init/uninstall —
//! `cli/src/{readcmds,initcmd,uninstallcmd,fmt}.rs` plus the non-ack-degraded
//! parts of `cmds.rs`). Drives the real `agentrec` binary against tempdir
//! fixtures, mirroring the helper patterns in `cli/tests/integration.rs`
//! (kept separate per file-ownership rules for this hardening round).

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

// ---- Task 11: purge --memories-retracted (archive-never-delete) ----------

fn mem_rec(
    id: &str,
    op: agentrec_core::memory::MemoryOp,
    fact: &str,
    ts: u64,
) -> agentrec_core::memory::MemoryRecord {
    agentrec_core::memory::MemoryRecord {
        v: 1,
        kind: "memory".to_string(),
        id: id.to_string(),
        op,
        fact: fact.to_string(),
        pins: vec![agentrec_core::memory::Pin {
            path: "src/a.rs".to_string(),
            hash: format!("sha256:{}", "a".repeat(64)),
        }],
        source_turns: vec![],
        origin: "human".to_string(),
        ts,
        reason: None,
    }
}

fn mem_line_id(line: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string))
}

// Task 11 acceptance test: a LIVE fact, a STALE-but-unretracted fact (old
// assert, never retracted), a fact retracted YESTERDAY (inside the default
// 90-day TTL), and a fact retracted 100 DAYS AGO (past TTL) — only the
// 100-day chain's records (assert + retract) move to
// memory.archived.*.jsonl; the other three chains' lines survive in
// memory.jsonl byte-for-byte. A second run is a genuine no-op (no new
// archive file, memory.jsonl unchanged). Finally, the archive+survivor union
// covers exactly the original line set — the crash-safety property that a
// kill-9 between the archive fsync and the source rewrite can only ever
// leave memory.jsonl a superset, never drop a record.
#[test]
fn purge_archives_only_expired_retracted_chains() {
    use agentrec_core::memory::{append_memory, MemoryOp};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    const DAY_MS: u64 = 86_400_000;

    append_memory(
        root,
        &mem_rec(
            "live1",
            MemoryOp::Assert,
            "the daemon uses signal-tailer offsets",
            now_ms,
        ),
    )
    .unwrap();
    append_memory(
        root,
        &mem_rec(
            "stale1",
            MemoryOp::Assert,
            "legacy config path moved to config toml",
            now_ms - 200 * DAY_MS,
        ),
    )
    .unwrap();
    append_memory(
        root,
        &mem_rec(
            "retracted_recent",
            MemoryOp::Assert,
            "old build script used make",
            now_ms - 5 * DAY_MS,
        ),
    )
    .unwrap();
    append_memory(
        root,
        &mem_rec(
            "retracted_recent",
            MemoryOp::Retract,
            "old build script used make",
            now_ms - DAY_MS,
        ),
    )
    .unwrap();
    append_memory(
        root,
        &mem_rec(
            "retracted_old",
            MemoryOp::Assert,
            "prototype used sqlite for storage",
            now_ms - 150 * DAY_MS,
        ),
    )
    .unwrap();
    append_memory(
        root,
        &mem_rec(
            "retracted_old",
            MemoryOp::Retract,
            "prototype used sqlite for storage",
            now_ms - 100 * DAY_MS,
        ),
    )
    .unwrap();

    let mem_path = root.join(".agentrec/memory.jsonl");
    let before_lines: Vec<String> = std::fs::read_to_string(&mem_path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(before_lines.len(), 6, "6 records seeded");

    let out = agentrec(root, &["purge", "--memories-retracted"]);
    assert!(out.status.success(), "purge failed: {out:?}");

    let archive_files: Vec<_> = std::fs::read_dir(root.join(".agentrec"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("memory.archived.")
        })
        .collect();
    assert_eq!(
        archive_files.len(),
        1,
        "expected exactly one archive file: {archive_files:?}"
    );
    let archive_text = std::fs::read_to_string(archive_files[0].path()).unwrap();
    let archive_lines: Vec<&str> = archive_text.lines().collect();
    assert_eq!(archive_lines.len(), 2, "only retracted_old's 2 records");
    for line in &archive_lines {
        assert_eq!(mem_line_id(line).as_deref(), Some("retracted_old"));
    }

    // Archived lines are byte-identical to the originals (no reserialization).
    let orig_old_lines: Vec<&String> = before_lines
        .iter()
        .filter(|l| mem_line_id(l).as_deref() == Some("retracted_old"))
        .collect();
    assert_eq!(
        archive_lines,
        orig_old_lines
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
        "archived lines must be byte-for-byte the originals"
    );

    // memory.jsonl retains the other 4 lines byte-for-byte, original order.
    let after_lines: Vec<String> = std::fs::read_to_string(&mem_path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let expected_survivors: Vec<&String> = before_lines
        .iter()
        .filter(|l| mem_line_id(l).as_deref() != Some("retracted_old"))
        .collect();
    assert_eq!(
        after_lines.iter().collect::<Vec<_>>(),
        expected_survivors,
        "surviving lines must be byte-for-byte, in original order"
    );

    // Second run -> genuine no-op: no new archive file, memory.jsonl unchanged.
    let out2 = agentrec(root, &["purge", "--memories-retracted"]);
    assert!(out2.status.success(), "second purge failed: {out2:?}");
    let archive_files_2: Vec<_> = std::fs::read_dir(root.join(".agentrec"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("memory.archived.")
        })
        .collect();
    assert_eq!(
        archive_files_2.len(),
        1,
        "second run must not create a new archive file"
    );
    let after_lines_2: Vec<String> = std::fs::read_to_string(&mem_path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        after_lines_2, after_lines,
        "second run must not change memory.jsonl"
    );

    // Crash-safety proxy: the archive+source union equals the original full
    // set — no record is ever lost (worst case, a kill-9 mid-way leaves a
    // record in both places, never in neither).
    let mut union: Vec<String> = after_lines.clone();
    union.extend(archive_lines.iter().map(|s| s.to_string()));
    let mut union_sorted = union.clone();
    union_sorted.sort();
    let mut before_sorted = before_lines.clone();
    before_sorted.sort();
    assert_eq!(
        union_sorted, before_sorted,
        "archive+source union must equal the original full set"
    );
}

fn spawn_record(root: &Path) -> Child {
    Command::new(bin())
        .args(["record", "--root", root.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn record")
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

/// The `record` daemon appends memory.jsonl concurrently (Task 7); the ONLY
/// sanctioned rewrite (`purge --memories-retracted`) would otherwise clobber
/// an append that lands between its read and its rename. So while the daemon
/// holds its flock, purge must refuse and touch nothing — exit 1, a
/// running-daemon reason on stderr, memory.jsonl byte-for-byte unchanged, and
/// no archive file created.
#[test]
fn purge_memories_retracted_refuses_while_daemon_running() {
    use agentrec_core::memory::{append_memory, MemoryOp};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    const DAY_MS: u64 = 86_400_000;

    // An expired-retracted chain that WOULD be archived if the guard weren't
    // there — so a passing test proves the refusal, not an empty candidate set.
    append_memory(
        root,
        &mem_rec(
            "retracted_old",
            MemoryOp::Assert,
            "prototype used sqlite for storage",
            now_ms - 150 * DAY_MS,
        ),
    )
    .unwrap();
    append_memory(
        root,
        &mem_rec(
            "retracted_old",
            MemoryOp::Retract,
            "prototype used sqlite for storage",
            now_ms - 100 * DAY_MS,
        ),
    )
    .unwrap();

    let mem_path = root.join(".agentrec/memory.jsonl");
    let before = std::fs::read(&mem_path).unwrap();

    let mut daemon = spawn_record(root);
    // Wait until the daemon has actually taken its flock (start epoch appended).
    let up = poll_until(Duration::from_secs(5), || {
        let text = std::fs::read_to_string(root.join(".agentrec/log.jsonl")).ok()?;
        text.lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .any(|v| {
                v.get("type").and_then(|t| t.as_str()) == Some("epoch")
                    && v.get("event").and_then(|e| e.as_str()) == Some("start")
            })
            .then_some(())
    });
    assert!(up.is_some(), "daemon never came up");

    let out = agentrec(root, &["purge", "--memories-retracted"]);

    // Tear the daemon down before any assertion can early-return and leak it.
    let _ = daemon.kill();
    let _ = daemon.wait();

    assert!(
        !out.status.success(),
        "purge must exit non-zero while the daemon is recording: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("recording") || stderr.contains("running"),
        "stderr must name the running-daemon reason: {stderr}"
    );

    // memory.jsonl untouched byte-for-byte (nothing rewritten).
    let after = std::fs::read(&mem_path).unwrap();
    assert_eq!(before, after, "memory.jsonl must be untouched on refusal");

    // No archive file created.
    let archive_count = std::fs::read_dir(root.join(".agentrec"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("memory.archived.")
        })
        .count();
    assert_eq!(
        archive_count, 0,
        "no archive file may be created on refusal"
    );
}

// ---- PR #2 curative: read-side dedup of orphan-recovery same-id duplicates --
//
// The engine fix (cb5dcd1) is PREVENTIVE: it stops a post-fix daemon writing a
// duplicate turn id. A log.jsonl written by a PRE-fix daemon can still hold two
// TurnRecords under one id (the kill-9 window between persist and journal
// clear), which broke `undo <full_ulid>` with "ambiguous turn id — matches 2
// turns". `resolve_turn` must now be curative: collapse an exact re-emit while
// still surfacing a genuine id collision.

#[test]
fn undo_collapses_orphan_recovery_duplicate_same_id() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"BEFORE\n").unwrap();
    let after = store.put(b"AFTER\n").unwrap();
    std::fs::write(root.join("d.rs"), b"AFTER\n").unwrap();

    let id = "t_DUP00000000000000000000001";
    let mut turn = base_turn(
        id,
        vec![FileEntry {
            path: "d.rs".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    // The steady `persist` close of a bracket turn carries an attributed model
    // and (here) an untruncated close.
    turn.model = Some("claude-opus-4".into());
    turn.truncated = false;
    seed_turn(root, &turn);
    // Pre-fix orphan recovery re-appended the SAME turn under the SAME id from
    // the crash journal. Recovery drifts on MORE than the timestamp: it
    // recomputes `ended` from last-change, forces `model: None`, and forces
    // `truncated: true` for a bracket turn (see daemon::recover_orphan). The
    // file set — path + before/after hashes — is byte-identical, which is the
    // only thing the collapse may key on.
    turn.ended = "2026-07-06T00:00:09.000Z".into();
    turn.model = None;
    turn.truncated = true;
    seed_turn(root, &turn);

    // Pre-fix this errored "ambiguous turn id — matches 2 turns". The read side
    // must collapse the double-emit and revert cleanly to a single turn.
    let out = agentrec(root, &["undo", id, "--confirm"]);
    assert!(out.status.success(), "undo should succeed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("reverted 1 file"),
        "expected a clean revert, got: {stdout}"
    );
    assert_eq!(
        std::fs::read(root.join("d.rs")).unwrap(),
        b"BEFORE\n",
        "file must be reverted to its pre-turn content"
    );
    assert_eq!(
        agentrec_turns(root).len(),
        1,
        "the collapsed double-emit reverts as exactly one undo turn"
    );
}

#[test]
fn undo_still_errors_on_distinct_turns_sharing_id() {
    use agentrec_core::record::FileEntry;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let id = "t_COLLIDE0000000000000000001";
    // Two DIFFERENT turns (disjoint file sets) minted under the SAME id — a
    // genuine id-gen collision, NOT a recovery double-emit. Collapsing these
    // would silently undo one arbitrary turn, so resolution must still error.
    let a = base_turn(
        id,
        vec![FileEntry {
            path: "one.rs".into(),
            before: None,
            after: Some(
                "sha256:1111111111111111111111111111111111111111111111111111111111111111".into(),
            ),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    let b = base_turn(
        id,
        vec![FileEntry {
            path: "two.rs".into(),
            before: None,
            after: Some(
                "sha256:2222222222222222222222222222222222222222222222222222222222222222".into(),
            ),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &a);
    seed_turn(root, &b);

    let out = agentrec(root, &["undo", id]);
    assert!(
        !out.status.success(),
        "two distinct turns sharing an id must not resolve: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ambiguous turn id"),
        "must still surface the genuine collision: {stderr}"
    );
}
