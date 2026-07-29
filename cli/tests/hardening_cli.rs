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
        imported: None,
        files_complete: None,
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
        imported: None,
        files_complete: None,
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

// ---- P2 AC3 (clm_2GCNB5WPT0FKHH4NFBYB9Q2JJT): undo of an imported turn
// with a provenance-only (`before: null`) entry refuses the WHOLE turn,
// before any working-tree write — even when another entry in the same turn
// has real, revertible content. ---------------------------------------------

#[test]
fn ac3_imported_turn_with_provenance_only_entry_refuses_before_any_write() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"BEFORE\n").unwrap();
    let after = store.put(b"AFTER\n").unwrap();
    // The on-disk file matches the turn's recorded `after` exactly, so this
    // entry is NOT modified-since — if the AC3 preflight were absent, this
    // entry alone would be perfectly revertible.
    std::fs::write(root.join("known.rs"), b"AFTER\n").unwrap();

    let mut turn = base_turn(
        "t_imp_AC3TEST00000000000000001",
        vec![
            FileEntry {
                path: "known.rs".into(),
                before: Some(before),
                after: Some(after),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
                after_synthesized: None,
            },
            FileEntry {
                path: "unknown.rs".into(),
                before: None, // provenance-only: import could not reconstruct this
                after: None,
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false, // AC4: import-missing-before is NOT baseline_unknown
                skipped_reason: None,
                after_synthesized: None,
            },
        ],
    );
    turn.imported = Some(true);
    turn.files_complete = Some(false);
    seed_turn(root, &turn);

    let out = agentrec(root, &["undo", &turn.id, "--confirm"]);

    assert!(
        !out.status.success(),
        "AC3: undo of an imported turn with a provenance-only entry must exit nonzero: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("imported"),
        "AC3: refusal must name imported history, got: {stderr}"
    );
    // Textually distinct from the other three refusal classes.
    assert!(!stderr.contains("secret-pattern"), "stderr: {stderr}");
    assert!(
        !stderr.contains("content not snapshotted"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains("later agent turn")
            && !stderr.contains("recording gap")
            && !stderr.contains("human or external edit"),
        "stderr: {stderr}"
    );

    // The load-bearing assertion: `known.rs` — which WOULD have been
    // revertible on its own — must be byte-unchanged. The refusal must land
    // before build_plan/execute_revert ever runs, not just before the
    // *other* file.
    assert_eq!(
        std::fs::read(root.join("known.rs")).unwrap(),
        b"AFTER\n",
        "AC3: refusal must happen before ANY working-tree write, even to an \
         otherwise-revertible file in the same turn"
    );
    // No undo turn was appended either (nothing was reverted, so nothing to record).
    assert!(
        agentrec_turns(root).is_empty(),
        "AC3: a refused undo must not append an agentrec undo turn"
    );
}

// ---- D1 (P2 fix round, founder decision 2): a synthesized `after` must
// never be reported as "human or external edit" when it disagrees with the
// real on-disk file — the disagreement is the synthesis being approximate,
// not a human touching the file. -----------------------------------------

#[test]
fn d1_synthesized_after_mismatch_is_never_attributed_to_a_human_edit() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    // `before` is real (e.g. a T2 git blob); `after` is a DERIVED
    // oldString->newString substitution that was never independently
    // observed. The file on disk is genuinely untouched since import —
    // still exactly the `before` content — so it disagrees with the
    // synthesized `after`, but NOT because anyone edited it.
    let before = store.put(b"alpha\nkeep\n").unwrap();
    let after = store.put(b"beta\nkeep\n").unwrap(); // synthesized, never real
    std::fs::write(root.join("tracked.txt"), b"alpha\nkeep\n").unwrap(); // untouched

    let mut turn = base_turn(
        "t_imp_D1TEST0000000000000000001",
        vec![FileEntry {
            path: "tracked.txt".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: Some(true),
        }],
    );
    turn.imported = Some(true);
    turn.files_complete = Some(false);
    seed_turn(root, &turn);

    // Preview only (no --confirm) is enough to exercise build_plan's
    // modified-since cause without mutating anything.
    let out = agentrec(root, &["undo", &turn.id]);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains("human or external edit"),
        "D1: a synthesized-after mismatch must never be blamed on a human edit, got: {stdout}"
    );
    assert!(
        stdout.contains("derived") || stdout.contains("not observed"),
        "D1: the cause must name the bytes as derived/not-observed, got: {stdout}"
    );

    // File genuinely untouched — belt-and-braces given this was preview-only.
    assert_eq!(
        std::fs::read(root.join("tracked.txt")).unwrap(),
        b"alpha\nkeep\n"
    );
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
            skipped_reason: None,
            after_synthesized: None,
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
            skipped_reason: None,
            after_synthesized: None,
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
            skipped_reason: None,
            after_synthesized: None,
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
            skipped_reason: None,
            after_synthesized: None,
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
            skipped_reason: None,
            after_synthesized: None,
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
            skipped_reason: None,
            after_synthesized: None,
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
            skipped_reason: None,
            after_synthesized: None,
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
            skipped_reason: None,
            after_synthesized: None,
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
            skipped_reason: None,
            after_synthesized: None,
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

/// Like `agentrec`/`spawn_record`, but with extra env vars set on the child —
/// used by the F4 tests below to drive `AGENTREC_TEST_PAUSE_BEFORE_MEMORY_REWRITE_MS`.
fn spawn_agentrec_with_env(root: &Path, args: &[&str], envs: &[(&str, &str)]) -> Child {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .args(["--root", root.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.spawn().expect("spawn agentrec")
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

// ---- F4: dedicated memory.lock closes the purge/append probe->act race ----
//
// The daemon-liveness refusal above only ever protects against the daemon.
// It does nothing for a MANUAL writer (`remember`/`verify`/`forget`) racing a
// concurrent `purge --memories-retracted`: purge reads memory.jsonl, computes
// its survivor set, then (pre-F4) rewrites the file with no lock held at all
// — a manual append landing in that window was silently clobbered by the
// rewrite. F4 adds a dedicated `.agentrec/memory.lock` that purge holds
// (non-blocking) across its whole read->archive->rewrite->rename sequence,
// and every writer now blocks on that same lock rather than racing it.
//
// `AGENTREC_TEST_PAUSE_BEFORE_MEMORY_REWRITE_MS` (purgecmd.rs) widens the
// window between purge's archive-fsync and its atomic rewrite long enough for
// these tests to land a concurrent writer deterministically inside it, using
// the archive file's appearance on disk as the observable "purge has read
// and archived, is now paused right before the rewrite" marker — no sleeps
// guessing at timing.

/// Core F4 acceptance test: a `remember` that starts while purge is paused
/// mid-rewrite must still have its record survive the rewrite.
///
/// RED (pre-F4, no `memory.lock`): `remember`'s unlocked `append_memory`
/// call succeeds immediately, appending its line to `memory.jsonl` on disk
/// while purge is paused — but purge already captured `survivor_lines` from
/// its EARLIER read, before that append happened. When purge's pause ends
/// and its atomic rewrite lands, it overwrites `memory.jsonl` with only the
/// old survivors, silently erasing the concurrent `remember`. Confirmed by
/// running this exact test against the code as it stood before this commit
/// (writers calling `agentrec_core::memory::append_memory` directly, no
/// lock anywhere): it fails — the new fact is absent from the post-purge
/// store, and `remember` returns near-instantly (it was never blocked).
/// GREEN (post-F4): `remember` blocks on `memory.lock` for (most of) the
/// pause — proven by wall-clock elapsed time, not just eventual presence —
/// and only appends after purge's rename has landed, so the final store has
/// BOTH the rewritten survivors AND the new fact, and the archive is
/// untouched.
#[test]
fn purge_rewrite_never_loses_concurrent_append() {
    use agentrec_core::memory::{append_memory, MemoryOp};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::write(root.join("a.rs"), b"fn a() {}").unwrap();

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    const DAY_MS: u64 = 86_400_000;

    // A live memory that must survive the rewrite untouched — part of the
    // "old survivors" set purge captures before it pauses.
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
    // A retracted-and-expired chain, so purge actually reaches the
    // archive+rewrite (an empty candidate set would take the fast "0
    // chains" path and never pause at all).
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

    let mut purge = spawn_agentrec_with_env(
        root,
        &["purge", "--memories-retracted"],
        &[("AGENTREC_TEST_PAUSE_BEFORE_MEMORY_REWRITE_MS", "3000")],
    );

    // Observable proof purge has read + archived and is now paused
    // immediately before the rewrite that would otherwise clobber a
    // concurrent append.
    let archive_path = poll_until(Duration::from_secs(5), || {
        std::fs::read_dir(root.join(".agentrec"))
            .ok()?
            .filter_map(|e| e.ok())
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("memory.archived.")
            })
            .map(|e| e.path())
    })
    .expect("purge never archived — never reached the pause");

    // While purge is paused, a concurrent writer appends a brand-new fact.
    let write_started = Instant::now();
    let out = agentrec(
        root,
        &["remember", "a fresh fact pinned to a.rs", "--from", "a.rs"],
    );
    let write_elapsed = write_started.elapsed();
    assert!(out.status.success(), "remember failed: {out:?}");

    let purge_status = purge.wait().expect("purge exited");
    assert!(purge_status.success(), "purge itself must succeed");

    // Proof the writer genuinely BLOCKED on memory.lock for (most of) the
    // pause, rather than racing in and getting lucky.
    assert!(
        write_elapsed >= Duration::from_millis(1500),
        "remember returned in {write_elapsed:?} — too fast to have waited \
         for purge's lock; the writer is not actually blocking on memory.lock"
    );

    let effective = agentrec_core::memory::load_effective(root).unwrap();
    assert!(
        effective.iter().any(|m| m.fact.contains("fresh fact")),
        "the concurrent remember's fact must survive purge's rewrite: {effective:?}"
    );
    assert!(
        effective.iter().any(|m| m.id == "live1"),
        "pre-existing live memory must also survive: {effective:?}"
    );
    assert!(
        !effective.iter().any(|m| m.id == "retracted_old"),
        "retracted_old was archived by this same purge and must be gone \
         from memory.jsonl: {effective:?}"
    );

    // Archive is intact and correct — the concurrent append never touched
    // it (archive-then-rewrite ordering, and the lock, are both undisturbed
    // by a writer that landed after the rewrite).
    let archive_text = std::fs::read_to_string(&archive_path).unwrap();
    let archive_ids: Vec<Option<String>> = archive_text.lines().map(mem_line_id).collect();
    assert_eq!(
        archive_ids.len(),
        2,
        "archive must still hold exactly retracted_old's 2 records: {archive_ids:?}"
    );
    assert!(
        archive_ids
            .iter()
            .all(|id| id.as_deref() == Some("retracted_old")),
        "archive must contain only retracted_old's records: {archive_ids:?}"
    );
}

/// Companion to the above: `verify --confirm` and `forget` (not just
/// `remember`) also route through `memory.lock`. Two DIFFERENT manual
/// writers, both started while purge is paused mid-rewrite, must both
/// eventually land — serialized by the lock, neither silently dropped —
/// rather than either racing purge's rewrite or failing outright on
/// contention.
///
/// RED (pre-F4): both `verify --confirm` and `forget` call unlocked
/// `append_memory` directly; whichever of them (or purge's rewrite) lands
/// last during the pause wins, silently discarding the others' appends —
/// this test's presence/absence assertions fail non-deterministically
/// depending on interleaving, and reliably fail under the widened pause
/// window used here.
/// GREEN (post-F4): both block on `memory.lock`, are serialized by the OS
/// (one after the other, after purge's rename), and both end up recorded.
#[test]
fn remember_waits_or_fails_cleanly_during_purge() {
    use agentrec_core::memory::{append_memory, MemoryOp};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::write(root.join("c.rs"), b"fn c() {}").unwrap();

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    const DAY_MS: u64 = 86_400_000;

    // `live1` will be retracted (via `forget`) by one of the two concurrent
    // writers below — its pin path doesn't need to exist on disk since
    // `forget` never re-hashes pins, only carries them forward.
    append_memory(
        root,
        &mem_rec(
            "live1",
            MemoryOp::Assert,
            "old build script used make",
            now_ms,
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

    let mut purge = spawn_agentrec_with_env(
        root,
        &["purge", "--memories-retracted"],
        &[("AGENTREC_TEST_PAUSE_BEFORE_MEMORY_REWRITE_MS", "3000")],
    );

    poll_until(Duration::from_secs(5), || {
        std::fs::read_dir(root.join(".agentrec"))
            .ok()?
            .filter_map(|e| e.ok())
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("memory.archived.")
            })
            .map(|_| ())
    })
    .expect("purge never archived — never reached the pause");

    // Two independent writers, spawned concurrently (both real, separate
    // processes), both attempting to touch memory.jsonl while purge holds
    // the lock paused.
    let spawn_started = Instant::now();
    let mut forget_child = spawn_agentrec_with_env(root, &["forget", "live1"], &[]);
    let mut remember_child = spawn_agentrec_with_env(
        root,
        &["remember", "a second concurrent fact", "--from", "c.rs"],
        &[],
    );

    let forget_status = forget_child.wait().expect("forget exited");
    let remember_status = remember_child.wait().expect("remember exited");
    let spawn_elapsed = spawn_started.elapsed();

    let purge_status = purge.wait().expect("purge exited");
    assert!(purge_status.success(), "purge itself must succeed");

    // Both concurrent writers must land cleanly — waiting, never a silent
    // drop or a contention error.
    assert!(
        forget_status.success(),
        "forget must succeed (wait, not fail)"
    );
    assert!(
        remember_status.success(),
        "remember must succeed (wait, not fail)"
    );
    assert!(
        spawn_elapsed >= Duration::from_millis(1500),
        "both writers returned in {spawn_elapsed:?} combined — too fast to \
         have waited for purge's lock"
    );

    let effective = agentrec_core::memory::load_effective(root).unwrap();
    let live1 = effective
        .iter()
        .find(|m| m.id == "live1")
        .expect("live1 must still be present (retracted, not deleted)");
    assert!(
        live1.retracted,
        "forget's retraction must have landed: {live1:?}"
    );
    assert!(
        effective
            .iter()
            .any(|m| m.fact.contains("second concurrent fact")),
        "remember's fact must have landed: {effective:?}"
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
            skipped_reason: None,
            after_synthesized: None,
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
            skipped_reason: None,
            after_synthesized: None,
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
            skipped_reason: None,
            after_synthesized: None,
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

// ---- purge --log-duplicates: store-level repair (the deferred "Option 2" --
// read-side migration/repair chip from the PR #2 follow-ups). The engine fix
// stops NEW same-id duplicates; `readcmds::same_revert` CURES the read path
// (diff/show/undo collapse on the fly) — but `log.jsonl` itself still carries
// the dup forever until this repair rewrites it.

fn append_raw_line(root: &Path, line: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(".agentrec/log.jsonl"))
        .expect("open log.jsonl for raw append");
    writeln!(f, "{line}").expect("append raw line");
}

fn count_log_archives(root: &Path) -> usize {
    std::fs::read_dir(root.join(".agentrec"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("log.archived."))
        .count()
}

#[test]
fn purge_log_duplicates_collapses_dedup_preserves_ambiguous_and_other_lines() {
    use agentrec_core::record::FileEntry;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // (a) a TRUE same_revert duplicate: same id, identical FileEntry set,
    // but drifted `ended`/`model`/`truncated` — exactly the shape a pre-fix
    // daemon's orphan recovery re-emitted (see readcmds.rs's own dup test).
    let dup_id = "t_LOGDUP0000000000000000001";
    let dup_files = vec![FileEntry {
        path: "a.rs".into(),
        before: Some(
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        ),
        after: Some(
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        ),
        op: "modify".into(),
        skipped: false,
        withheld: false,
        baseline_unknown: false,
        skipped_reason: None,
        after_synthesized: None,
    }];
    let mut turn1 = base_turn(dup_id, dup_files);
    turn1.model = Some("claude-opus-4".into());
    turn1.truncated = false;
    seed_turn(root, &turn1);
    let mut turn2 = turn1.clone();
    turn2.ended = "2026-07-06T00:00:09.000Z".into();
    turn2.model = None;
    turn2.truncated = true;
    seed_turn(root, &turn2);

    // (b) a same-id pair with DIFFERENT file sets — a genuine id collision,
    // not a recovery double-emit. Must stay untouched (never silently pick
    // one side).
    let collide_id = "t_LOGCOLLIDE000000000000001";
    let turn_c1 = base_turn(
        collide_id,
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
            skipped_reason: None,
            after_synthesized: None,
        }],
    );
    let turn_c2 = base_turn(
        collide_id,
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
            skipped_reason: None,
            after_synthesized: None,
        }],
    );
    seed_turn(root, &turn_c1);
    seed_turn(root, &turn_c2);

    // (c) an epoch line, plus a turn-SHAPED unknown-`type` line appended
    // TWICE with byte-identical content — the classification trap: this
    // repair must recognize the `type` field and never treat these as
    // removable turn duplicates just because a naive `TurnRecord` parse
    // (which ignores unrecognized `type`) would find them "identical".
    seed_epoch(root, "start", "2026-07-06T00:00:00.000Z");
    let future_line = concat!(
        "{\"type\":\"future_thing\",\"id\":\"t_FUTURE0000000000000000001\",\"grade\":\"rich\",",
        "\"started\":\"2026-07-06T00:00:00.000Z\",\"ended\":\"2026-07-06T00:00:01.000Z\",",
        "\"root\":\"/repo\",\"files\":[]}"
    );
    append_raw_line(root, future_line);
    append_raw_line(root, future_line);

    let log_path = root.join(".agentrec/log.jsonl");
    let before_bytes = std::fs::read(&log_path).unwrap();

    let out = agentrec(root, &["purge", "--log-duplicates"]);
    assert!(out.status.success(), "purge should succeed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("removed 1 duplicate"),
        "expected exactly 1 dup removed: {stdout}"
    );

    let after_bytes = std::fs::read(&log_path).unwrap();
    let after_text = String::from_utf8_lossy(&after_bytes).into_owned();
    let after_lines: Vec<&str> = after_text.lines().collect();

    // (a) collapsed to exactly one line for dup_id.
    assert_eq!(
        after_lines.iter().filter(|l| l.contains(dup_id)).count(),
        1,
        "true duplicate must collapse to one line: {after_text}"
    );

    // (b) both collide_id lines survive untouched.
    assert_eq!(
        after_lines
            .iter()
            .filter(|l| l.contains(collide_id))
            .count(),
        2,
        "distinct same-id turns must NOT be collapsed: {after_text}"
    );

    // (c) epoch + BOTH unknown-type lines preserved byte-identical.
    assert_eq!(
        after_lines
            .iter()
            .filter(|l| l.contains("\"type\":\"epoch\""))
            .count(),
        1,
        "epoch line must be preserved: {after_text}"
    );
    assert_eq!(
        after_lines
            .iter()
            .filter(|l| l.contains("future_thing"))
            .count(),
        2,
        "unknown-type turn-shaped lines must never be treated as turn duplicates: {after_text}"
    );
    assert!(
        after_text.contains(future_line),
        "unknown-type line must be byte-preserved: {after_text}"
    );

    // Archive exists and holds the exact pre-repair original.
    let archive = std::fs::read_dir(root.join(".agentrec"))
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().starts_with("log.archived."))
        .expect("archive file must exist");
    let archived_bytes = std::fs::read(archive.path()).unwrap();
    assert_eq!(
        archived_bytes, before_bytes,
        "archive must hold the exact pre-repair original"
    );

    // Second run: no-op, no new archive, log.jsonl byte-identical.
    let archive_count_before = count_log_archives(root);
    let out2 = agentrec(root, &["purge", "--log-duplicates"]);
    assert!(out2.status.success(), "second run should succeed: {out2:?}");
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        stdout2.contains("0 duplicate"),
        "second run must be a no-op: {stdout2}"
    );
    assert_eq!(
        std::fs::read(&log_path).unwrap(),
        after_bytes,
        "second run must not touch log.jsonl"
    );
    assert_eq!(
        count_log_archives(root),
        archive_count_before,
        "second run must not create another archive"
    );
}

#[test]
fn purge_log_duplicates_refuses_while_daemon_running() {
    use agentrec_core::record::FileEntry;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let dup_id = "t_LOGDUPRUN0000000000000001";
    let files = vec![FileEntry {
        path: "r.rs".into(),
        before: None,
        after: Some("sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into()),
        op: "create".into(),
        skipped: false,
        withheld: false,
        baseline_unknown: false,
        skipped_reason: None,
        after_synthesized: None,
    }];
    let turn = base_turn(dup_id, files);
    // Exact identical dup — WOULD be removed if the daemon-liveness guard
    // weren't there, so a passing test proves the refusal, not an empty
    // candidate set.
    seed_turn(root, &turn);
    seed_turn(root, &turn);

    let log_path = root.join(".agentrec/log.jsonl");

    let mut daemon = spawn_record(root);
    // Wait until the daemon has actually taken its flock (start epoch appended).
    let up = poll_until(Duration::from_secs(5), || {
        let text = std::fs::read_to_string(&log_path).ok()?;
        text.lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .any(|v| {
                v.get("type").and_then(|t| t.as_str()) == Some("epoch")
                    && v.get("event").and_then(|e| e.as_str()) == Some("start")
            })
            .then_some(())
    });
    assert!(up.is_some(), "daemon never came up");

    let out = agentrec(root, &["purge", "--log-duplicates"]);

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

    // Both duplicate lines must remain — refusal happened before any read of
    // the file for repair purposes.
    let after = std::fs::read_to_string(&log_path).unwrap();
    let dup_count = after.lines().filter(|l| l.contains(dup_id)).count();
    assert_eq!(
        dup_count, 2,
        "daemon refusal must leave both duplicate lines untouched: {after}"
    );

    // No archive file created.
    assert_eq!(
        count_log_archives(root),
        0,
        "no archive file must be created on refusal"
    );
}

/// `log.jsonl`'s only non-daemon writer is `undo --confirm` (no dedicated
/// lock guards it — see purgecmd.rs's `purge_log_duplicates` doc comment for
/// why not). The length-recheck-before-rename guard is what actually closes
/// that TOCTOU window: `AGENTREC_TEST_PAUSE_BEFORE_LOG_REWRITE_MS` widens the
/// gap between purge's archive-fsync and its rewrite long enough to land a
/// real concurrent append inside it, using the archive file's appearance as
/// the observable "paused right before rewrite" marker (same technique as
/// `purge_rewrite_never_loses_concurrent_append` above).
#[test]
fn purge_log_duplicates_aborts_on_concurrent_growth() {
    use agentrec_core::record::FileEntry;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let dup_id = "t_LOGGROW0000000000000000001";
    let files = vec![FileEntry {
        path: "g.rs".into(),
        before: None,
        after: Some("sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".into()),
        op: "create".into(),
        skipped: false,
        withheld: false,
        baseline_unknown: false,
        skipped_reason: None,
        after_synthesized: None,
    }];
    let turn = base_turn(dup_id, files);
    seed_turn(root, &turn);
    seed_turn(root, &turn); // real duplicate, so purge actually reaches the pause

    let log_path = root.join(".agentrec/log.jsonl");
    let original_bytes = std::fs::read(&log_path).unwrap();

    let mut purge = spawn_agentrec_with_env(
        root,
        &["purge", "--log-duplicates"],
        &[("AGENTREC_TEST_PAUSE_BEFORE_LOG_REWRITE_MS", "3000")],
    );

    // Observable proof purge has read + archived and is now paused
    // immediately before the length recheck + rewrite.
    poll_until(Duration::from_secs(5), || {
        std::fs::read_dir(root.join(".agentrec"))
            .ok()?
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().starts_with("log.archived."))
            .map(|e| e.path())
    })
    .expect("purge never archived — never reached the pause");

    // While purge is paused, a concurrent writer (standing in for a racing
    // `undo --confirm` or a daemon that starts mid-repair) appends a new
    // line directly.
    let extra_id = "t_EXTRAWRITE0000000000000001";
    let extra_turn = base_turn(
        extra_id,
        vec![FileEntry {
            path: "extra.rs".into(),
            before: None,
            after: Some(
                "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".into(),
            ),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
        }],
    );
    seed_turn(root, &extra_turn);
    let grown_bytes = std::fs::read(&log_path).unwrap();
    assert!(
        grown_bytes.len() > original_bytes.len(),
        "concurrent append must have actually grown the file before purge resumes"
    );

    let status = purge.wait().expect("purge exited");
    assert!(
        !status.success(),
        "purge must abort when the file grew underneath it"
    );

    // log.jsonl must be untouched — still exactly the pre-rewrite content
    // plus the concurrent writer's append; the duplicate is STILL present
    // because the rewrite never happened.
    let after_bytes = std::fs::read(&log_path).unwrap();
    assert_eq!(
        after_bytes, grown_bytes,
        "aborted purge must leave log.jsonl exactly as the concurrent writer left it"
    );
    let after_text = String::from_utf8_lossy(&after_bytes);
    assert_eq!(
        after_text.lines().filter(|l| l.contains(dup_id)).count(),
        2,
        "duplicate must still be present — the rewrite was aborted, not silently lost: {after_text}"
    );
    assert!(
        after_text.contains(extra_id),
        "the concurrent writer's own append must never be lost: {after_text}"
    );
}

/// Finding #3: a REAL `undo --confirm` racing `purge --log-duplicates` must
/// lose nothing. `undo` takes `log.lock` (loglock.rs) blocking around its
/// append and `purge` holds the same lock across its rewrite, so the undo
/// waits for purge's rename and then lands on the rewritten file. Post-fix
/// purge therefore SUCCEEDS (the undo never grew the file mid-window) and both
/// the collapsed duplicate and the undo's own turn survive. Pre-fix (no lock)
/// the undo appended during purge's pause, growing the file, and the length
/// recheck ABORTED the purge — so `purge succeeds` is the RED assertion this
/// test is built around (distinct from `aborts_on_concurrent_growth` above,
/// whose raw-`append_log` writer stands in for a lock-less daemon and still
/// trips the recheck).
#[test]
fn purge_log_duplicates_and_concurrent_undo_lose_nothing() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    // A real duplicate, so purge reaches the pause + rewrite.
    let dup_id = "t_LOGUNDODUP00000000000000001";
    let dup = base_turn(
        dup_id,
        vec![FileEntry {
            path: "dup.rs".into(),
            before: None,
            after: Some(
                "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".into(),
            ),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
        }],
    );
    seed_turn(root, &dup);
    seed_turn(root, &dup);

    // A separate, genuinely revertible turn for `undo --confirm` to act on.
    let before = store.put(b"BEFORE\n").unwrap();
    let after = store.put(b"AFTER\n").unwrap();
    std::fs::write(root.join("u.rs"), b"AFTER\n").unwrap();
    let undo_target_id = "t_LOGUNDOTGT00000000000000001";
    let target = base_turn(
        undo_target_id,
        vec![FileEntry {
            path: "u.rs".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
        }],
    );
    seed_turn(root, &target);

    // Start purge paused right before its rewrite (holding log.lock).
    let mut purge = spawn_agentrec_with_env(
        root,
        &["purge", "--log-duplicates"],
        &[("AGENTREC_TEST_PAUSE_BEFORE_LOG_REWRITE_MS", "3000")],
    );

    // Observable proof purge archived and is now paused, holding the lock.
    poll_until(Duration::from_secs(5), || {
        std::fs::read_dir(root.join(".agentrec"))
            .ok()?
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().starts_with("log.archived."))
            .map(|e| e.path())
    })
    .expect("purge never archived — never reached the pause");

    // Concurrent real undo: with the lock it BLOCKS on its append until
    // purge's rename completes.
    let mut undo = spawn_agentrec_with_env(root, &["undo", undo_target_id, "--confirm"], &[]);

    let purge_status = purge.wait().expect("purge exited");
    let undo_status = undo.wait().expect("undo exited");

    assert!(
        purge_status.success(),
        "purge must SUCCEED — the racing undo blocks on log.lock instead of \
         growing the file inside the recheck window (pre-fix this aborted)"
    );
    assert!(
        undo_status.success(),
        "undo must succeed once purge releases the lock"
    );

    let text = std::fs::read_to_string(root.join(".agentrec/log.jsonl")).unwrap();
    assert_eq!(
        text.lines().filter(|l| l.contains(dup_id)).count(),
        1,
        "the duplicate must be collapsed to exactly one copy: {text}"
    );
    assert!(
        text.contains("\"tool\":\"agentrec\""),
        "the undo's own turn (tool agentrec) must survive in the rewritten log: {text}"
    );
    assert_eq!(
        std::fs::read(root.join("u.rs")).unwrap(),
        b"BEFORE\n",
        "u.rs must be reverted to its pre-turn content"
    );
}

// ---- purge --orphans (superseded-snapshot GC) -------------------------------

/// End-to-end: a blob referenced by no turn (a superseded intermediate
/// snapshot) is archived out of `objects/` and preserved under
/// `objects.archived.<ts>/`, while a blob a turn DOES reference is left in
/// place. Exit 0, a "reclaimed" line on stdout.
#[test]
fn purge_orphans_archives_unreferenced_and_keeps_referenced() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let store = agentrec_core::store::BlobStore::new(root.join(".agentrec/objects"));
    let referenced = store.put(b"content a committed turn keeps").unwrap();
    let orphan = store.put(b"a superseded intermediate state").unwrap();

    // A committed turn that references only `referenced`.
    let turn = base_turn(
        "t_ORPHANTEST0000000000000001",
        vec![agentrec_core::record::FileEntry {
            path: "kept.rs".into(),
            before: None,
            after: Some(referenced.clone()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["purge", "--orphans"]);
    assert!(out.status.success(), "purge --orphans must exit 0: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("reclaimed 1 orphaned"),
        "expected a reclaim line: {stdout}"
    );

    assert!(
        object_path(root, &referenced).exists(),
        "the turn-referenced blob must remain in objects/"
    );
    assert!(
        !object_path(root, &orphan).exists(),
        "the orphan blob must be moved out of objects/"
    );

    // The orphan is preserved in the archive dir, never deleted.
    let hex = orphan.strip_prefix("sha256:").unwrap();
    let archive_dir = std::fs::read_dir(root.join(".agentrec"))
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("objects.archived.")
        })
        .expect("archive dir created");
    assert!(
        archive_dir.path().join(&hex[..2]).join(&hex[2..]).exists(),
        "orphan preserved under objects.archived/ (never deleted)"
    );
}

/// The daemon `put`s new snapshot blobs and appends turns continuously, so an
/// orphan reclaim racing it could archive a blob a turn is about to reference.
/// While the daemon holds its flock, `purge --orphans` must refuse (exit 1)
/// and archive nothing.
#[test]
fn purge_orphans_refuses_while_daemon_running() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // An orphan that WOULD be reclaimed if the guard weren't there.
    let store = agentrec_core::store::BlobStore::new(root.join(".agentrec/objects"));
    let orphan = store.put(b"would-be reclaimed if unguarded").unwrap();

    let mut daemon = spawn_record(root);
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

    let out = agentrec(root, &["purge", "--orphans"]);

    let _ = daemon.kill();
    let _ = daemon.wait();

    assert!(
        !out.status.success(),
        "purge --orphans must exit non-zero while the daemon records: {out:?}"
    );
    assert!(
        object_path(root, &orphan).exists(),
        "the orphan must be untouched while the daemon is up"
    );
    assert!(
        std::fs::read_dir(root.join(".agentrec"))
            .unwrap()
            .filter_map(|e| e.ok())
            .all(|e| !e
                .file_name()
                .to_string_lossy()
                .starts_with("objects.archived.")),
        "no archive dir may be created on refusal"
    );
}
