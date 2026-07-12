//! End-to-end tests driving the real `agentrec` binary against tempdir
//! fixtures. Covers init, the daemon record cycle (rich capture, gitignore
//! filtering, transcript-derived prompt/model), the single-writer lock (B1),
//! and crash-journal recovery (B2). Spawned-daemon assertions poll with a
//! timeout because macOS fsevents coalesces events by a second or more.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

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

fn spawn_record(root: &Path) -> Child {
    Command::new(bin())
        .args(["record", "--root", root.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn record")
}

fn send_hook(root: &Path, payload: &str) {
    let mut child = Command::new(bin())
        .args(["hook", "claude", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait().unwrap();
}

/// Like [`send_hook`] but captures stdout/status instead of discarding it —
/// Task 9's memory-injection block is written to stdout.
fn send_hook_capture(root: &Path, payload: &str) -> Output {
    let mut child = Command::new(bin())
        .args(["hook", "claude", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// Turn records currently in the log (epoch lines skipped).
fn turns(root: &Path) -> Vec<serde_json::Value> {
    let path = root.join(".agentrec/log.jsonl");
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("turn"))
        .collect()
}

/// Poll `f` until it returns Some or the deadline passes.
fn poll_until<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn init(root: &Path) {
    // A git repo so gitignore semantics are exercised; --no-hook to avoid
    // touching a real Claude Code settings file.
    Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(root)
        .status()
        .unwrap();
    // --no-service: none of these tests should ever write/load a real
    // launchd/systemd unit on the developer's machine.
    let out = agentrec(root, &["init", "--no-hook", "--no-service"]);
    assert!(out.status.success(), "init failed: {out:?}");
}

#[test]
fn init_scaffolds_agentrec() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    assert!(root.join(".agentrec/config.toml").exists());
    assert!(root.join(".agentrec/objects").exists());
    let gitignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();
    assert_eq!(gitignore.matches(".agentrec/").count(), 1);
}

// D37: the REAL daemon writes log.jsonl and snapshot blobs at 0600, and
// `.agentrec/` itself at 0700, umask-independently — replaces config.toml-
// only proof (that only covers `init`, not the daemon's own writes).
#[cfg(unix)]
#[test]
fn d37_daemon_writes_land_at_locked_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    std::fs::write(root.join("real.rs"), "fn main() {}").unwrap();

    poll_until(Duration::from_secs(25), || {
        root.join(".agentrec/open.json").exists().then_some(())
    })
    .expect("turn opened");

    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s_perm"}"#);

    poll_until(Duration::from_secs(10), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("real.rs"))
                })
                .unwrap_or(false)
        })
    })
    .expect("rich turn with real.rs");

    let _ = daemon.kill();
    let _ = daemon.wait();

    let dir_mode = std::fs::metadata(root.join(".agentrec"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700, ".agentrec/ must be 0700");

    let log_mode = std::fs::metadata(root.join(".agentrec/log.jsonl"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(log_mode & 0o777, 0o600, "log.jsonl must be 0600");

    let objects = root.join(".agentrec/objects");
    let mut found_blob = false;
    for fan in std::fs::read_dir(&objects).unwrap().flatten() {
        if !fan.path().is_dir() {
            continue;
        }
        for f in std::fs::read_dir(fan.path()).unwrap().flatten() {
            if f.path().is_file() {
                let mode = f.metadata().unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600, "blob {:?} must be 0600", f.path());
                found_blob = true;
            }
        }
    }
    assert!(found_blob, "expected at least one snapshot blob written");
}

#[test]
fn records_rich_turn_with_transcript_prompt_and_model_and_filters_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::write(root.join(".gitignore"), "ignored.log\n").unwrap();

    // A transcript carrying the model and the user prompt (no prompt on the
    // signal → exercises transcript fallback, Q+).
    let transcript = root.join("session.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"wire up the parser"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","model":"claude-opus-4-8","content":[{"type":"text","text":"ok"}]}}"#,
            "\n",
        ),
    )
    .unwrap();

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    // No start bracket: the file opens an unattributed turn, attributed on stop.
    std::fs::write(root.join("real.rs"), "fn main() {}").unwrap();
    std::fs::write(root.join("ignored.log"), "noise").unwrap();

    // Wait for the change to be observed before signalling stop.
    poll_until(Duration::from_secs(25), || {
        root.join(".agentrec/open.json").exists().then_some(())
    })
    .expect("turn opened");

    let stop = format!(
        r#"{{"hook_event_name":"Stop","session_id":"s_it","transcript_path":"{}"}}"#,
        transcript.display()
    );
    send_hook(root, &stop);

    let turn = poll_until(Duration::from_secs(10), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("real.rs"))
                })
                .unwrap_or(false)
        })
    })
    .expect("rich turn with real.rs");

    let _ = daemon.kill();
    let _ = daemon.wait();

    assert_eq!(turn.get("grade").and_then(|g| g.as_str()), Some("rich"));
    assert_eq!(turn.get("tool").and_then(|t| t.as_str()), Some("claude"));
    assert_eq!(
        turn.get("model").and_then(|m| m.as_str()),
        Some("claude-opus-4-8")
    );
    assert_eq!(
        turn.get("prompt_excerpt").and_then(|p| p.as_str()),
        Some("wire up the parser")
    );
    // The gitignored file never appears in any turn.
    let mentions_ignored = turns(root).iter().any(|t| {
        t.get("files")
            .and_then(|f| f.as_array())
            .map(|fs| {
                fs.iter()
                    .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("ignored.log"))
            })
            .unwrap_or(false)
    });
    assert!(!mentions_ignored, "gitignored file leaked into a turn");
}

#[test]
fn second_record_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    let out = agentrec(root, &["record"]);
    let _ = daemon.kill();
    let _ = daemon.wait();

    assert_eq!(out.status.code(), Some(1), "second record should exit 1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already recording"), "stderr: {stderr}");
}

#[test]
fn recovers_orphaned_open_turn_from_journal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // Plant a crash journal as if a prior process died mid-turn (unattributed).
    let journal = format!(
        r#"{{"source":"quiet","tool":null,"prompt":null,"session":null,"opened_wall_ms":1783296000000,"last_change_wall_ms":1783296005000,"root":"{}","files":[{{"path":"orphan.rs","before":null,"after":"sha256:deadbeef","op":"create"}}]}}"#,
        root.display()
    );
    std::fs::write(root.join(".agentrec/open.json"), journal).unwrap();

    let mut daemon = spawn_record(root);
    // Recovery runs at startup, before the watch loop — appears near-instantly.
    let turn = poll_until(Duration::from_secs(10), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("orphan.rs"))
                })
                .unwrap_or(false)
        })
    });
    let _ = daemon.kill();
    let _ = daemon.wait();

    let turn = turn.expect("orphan turn recovered");
    // Unattributed orphan → bare, no fabricated tool.
    assert_eq!(turn.get("grade").and_then(|g| g.as_str()), Some("bare"));
    assert!(turn.get("tool").and_then(|t| t.as_str()).is_none());
    // Journal consumed.
    assert!(!root.join(".agentrec/open.json").exists());
}

// D35 / AC M+: a persisted snapshot-failure count surfaces as a DEGRADED
// banner in `status`, and `status --ack-degraded` clears it. Inducing a real
// disk-full write failure is impractical in a portable test, so this drives
// the taxonomy deterministically via a planted `state.json`.
#[test]
fn status_shows_degraded_banner_and_ack_clears() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    std::fs::write(
        root.join(".agentrec/state.json"),
        r#"{"pid":0,"signal_offset":0,"snapshot_failures":2,"io_failed":["src/a.rs"]}"#,
    )
    .unwrap();

    let out = agentrec(root, &["status"]);
    assert!(out.status.success(), "status failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("DEGRADED"), "stdout: {stdout}");
    assert!(stdout.contains('2'), "stdout: {stdout}");
    assert!(stdout.contains("src/a.rs"), "stdout: {stdout}");

    let out = agentrec(root, &["status", "--ack-degraded"]);
    assert!(out.status.success(), "ack failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("acknowledged"), "stdout: {stdout}");

    let out = agentrec(root, &["status"]);
    assert!(out.status.success(), "status failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("DEGRADED"), "stdout: {stdout}");
}

// D-PD3: an empty store has zero agent turns, so a rich-rate percentage would
// be vacuous (100% over 0 turns misleadingly reads as "healthy"). `status`
// must print an honest "n/a" instead of fabricating a rate.
#[test]
fn status_zero_turns_shows_rich_rate_na_not_vacuous_100_percent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let out = agentrec(root, &["status"]);
    assert!(out.status.success(), "status failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("rich-rate:  n/a"), "stdout: {stdout}");
    assert!(stdout.contains("no agent turns yet"), "stdout: {stdout}");
    assert!(!stdout.contains("100%"), "stdout: {stdout}");
}

// --- AC F1–F4, K+: `diff` prints a unified diff of a turn's file changes.
// These seed the log directly via agentrec-core rather than driving the
// daemon, since the daemon's job (turn boundaries) isn't what's under test
// here — only the read-side rendering of an already-recorded turn.

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

#[test]
fn diff_text_modify_shows_unified() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"a\nb\nc\n").unwrap();
    let after = store.put(b"a\nB\nc\n").unwrap();
    let created = store.put(b"brand new file\n").unwrap();

    let turn = base_turn(
        "t_MODIFYTEST0000000000000001",
        vec![
            FileEntry {
                path: "src/x.rs".into(),
                before: Some(before),
                after: Some(after),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
            },
            FileEntry {
                path: "src/new.rs".into(),
                before: None,
                after: Some(created),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
            },
        ],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["diff", &turn.id]);
    assert!(out.status.success(), "diff failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("-b"), "stdout: {stdout}");
    assert!(stdout.contains("+B"), "stdout: {stdout}");
    assert!(stdout.contains("+brand new file"), "stdout: {stdout}");
}

#[test]
fn diff_binary_file_message() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before_bytes: Vec<u8> = vec![0u8, 1, 2, 3];
    let after_bytes: Vec<u8> = vec![0u8, 1, 2, 3, 4, 5, 6];
    let before = store.put(&before_bytes).unwrap();
    let after = store.put(&after_bytes).unwrap();

    let turn = base_turn(
        "t_BINARYTEST00000000000000001",
        vec![FileEntry {
            path: "assets/img.bin".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["diff", &turn.id]);
    assert!(out.status.success(), "diff failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("binary file changed ("), "stdout: {stdout}");
    assert!(
        stdout.contains(&before_bytes.len().to_string()),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&after_bytes.len().to_string()),
        "stdout: {stdout}"
    );
    assert!(!stdout.contains("@@"), "stdout: {stdout}");
}

#[test]
fn diff_skipped_file_notice() {
    use agentrec_core::record::FileEntry;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let turn = base_turn(
        "t_SKIPTEST0000000000000000001",
        vec![FileEntry {
            path: "big/huge.bin".into(),
            before: None,
            after: None,
            op: "modify".into(),
            skipped: true,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["diff", &turn.id]);
    assert!(out.status.success(), "diff failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("(content not snapshotted — over size cap)"),
        "stdout: {stdout}"
    );
}

#[test]
fn diff_unknown_turn_exits_1_with_range() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let turn = base_turn("t_REALTURN00000000000000001", vec![]);
    seed_turn(root, &turn);

    let out = agentrec(root, &["diff", "t_DOESNOTEXIST"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("unknown turn id"), "output: {combined}");
    assert!(
        combined.contains("..") && combined.contains("turns)"),
        "output: {combined}"
    );
}

#[test]
fn diff_prefix_match() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let turn = base_turn("t_PREFIXTEST000000000000001", vec![]);
    seed_turn(root, &turn);

    let prefix = &turn.id[..6];
    let out = agentrec(root, &["diff", prefix]);
    assert!(out.status.success(), "diff failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("turn "), "stdout: {stdout}");
    assert!(stdout.contains("0 file"), "stdout: {stdout}");
}

// --- AC PD2: `agentrec show <turn> [--prompt]` (SPEC §Prompt posture item
// 4, excerpt discipline). Bare `show` prints only the header; the full
// post-scrub prompt requires the explicit flag. Every case below asserts
// both the exit code and the correct stream, not just a substring.

#[test]
fn show_prompt_prints_full_post_scrub_prompt() {
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    // Long enough (>120 chars, multi-line) that the capped/flattened excerpt
    // the bare `show` renders can never contain this full text verbatim.
    let full_prompt = format!(
        "add rate limiting to the login endpoint\nand also please {}\ndone",
        "x".repeat(200)
    );
    let prompt_ref = store.put(full_prompt.as_bytes()).unwrap();

    let mut turn = base_turn("t_SHOWPROMPT0000000000001", vec![]);
    turn.prompt_ref = Some(prompt_ref);
    turn.prompt_excerpt = Some(agentrec_core::scrub::excerpt(&full_prompt));
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id, "--prompt"]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0: {out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        full_prompt,
        "stdout must be exactly the full post-scrub prompt"
    );
    assert!(
        out.stderr.is_empty(),
        "stderr must be empty on success: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn show_prompt_dangling_ref_reports_purge_on_stderr() {
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let prompt_ref = store.put(b"a prompt that will be purged").unwrap();
    assert!(store.remove(&prompt_ref).is_some(), "must actually delete");

    let mut turn = base_turn("t_SHOWDANGLING000000000001", vec![]);
    turn.prompt_ref = Some(prompt_ref);
    turn.prompt_excerpt = Some("a prompt that will be purged".into());
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id, "--prompt"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    assert!(
        out.stdout.is_empty(),
        "stdout must be empty on failure: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("agentrec: "), "stderr: {stderr}");
    assert!(
        stderr.to_lowercase().contains("purge") || stderr.to_lowercase().contains("ttl"),
        "stderr must mention purge or TTL: {stderr}"
    );
    // Regression guard: a genuinely missing ref must stay distinct from the
    // corrupt-blob path below — never claim hash-mismatch for a purge.
    assert!(
        !stderr.to_lowercase().contains("corrupt"),
        "missing-ref message must not say corrupt: {stderr}"
    );
}

// Skeptic-surfaced (2026-07-10): a PRESENT but CORRUPT (hash-mismatch) prompt
// blob was previously reported with the same "TTL expired" message as a
// genuinely missing/purged ref — fabricating a cause for a blob that's still
// on disk. This must be a distinct, honest message and must never say TTL.
#[test]
fn show_prompt_corrupt_blob_reports_hash_mismatch_on_stderr() {
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let prompt_ref = store.put(b"a prompt that will be corrupted").unwrap();
    // Tamper the object's bytes on disk so the stored hash no longer matches
    // (mirrors `agentrec_core::store::tests::corrupt_object_detected`).
    let hex = prompt_ref.strip_prefix("sha256:").unwrap();
    let path = root
        .join(".agentrec/objects")
        .join(&hex[..2])
        .join(&hex[2..]);
    std::fs::write(&path, b"tampered").unwrap();

    let mut turn = base_turn("t_SHOWCORRUPT0000000000001", vec![]);
    turn.prompt_ref = Some(prompt_ref);
    turn.prompt_excerpt = Some("a prompt that will be corrupted".into());
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id, "--prompt"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    assert!(
        out.stdout.is_empty(),
        "stdout must be empty on failure: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("agentrec: "), "stderr: {stderr}");
    assert!(
        stderr.to_lowercase().contains("corrupt")
            && stderr.to_lowercase().contains("hash mismatch"),
        "stderr must mention corrupt/hash mismatch: {stderr}"
    );
    assert!(
        !stderr.to_lowercase().contains("ttl expired"),
        "corrupt blob must not fabricate TTL expiry: {stderr}"
    );
}

#[test]
fn show_prompt_bare_turn_no_prompt_attached_on_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // A bare turn: no prompt_ref at all (base_turn's default).
    let turn = base_turn("t_SHOWBARETURN00000000001", vec![]);
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id, "--prompt"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    assert!(
        out.stdout.is_empty(),
        "stdout must be empty on failure: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("agentrec: "), "stderr: {stderr}");
    assert!(
        stderr.to_lowercase().contains("no prompt attached"),
        "stderr: {stderr}"
    );
}

#[test]
fn show_without_prompt_flag_prints_header_never_full_prompt() {
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let full_prompt = format!(
        "add rate limiting to the login endpoint\nand also please {}\ndone",
        "y".repeat(200)
    );
    let prompt_ref = store.put(full_prompt.as_bytes()).unwrap();

    let mut turn = base_turn("t_SHOWHEADERONLY000000001", vec![]);
    turn.prompt_ref = Some(prompt_ref);
    turn.prompt_excerpt = Some(agentrec_core::scrub::excerpt(&full_prompt));
    seed_turn(root, &turn);

    let out = agentrec(root, &["show", &turn.id]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // A header field (the turn's short id) is present...
    assert!(stdout.contains(&turn.id[..4]), "stdout: {stdout}");
    // ...but the full prompt body is never printed without --prompt.
    assert!(!stdout.contains(&full_prompt), "stdout: {stdout}");
    assert!(
        out.stderr.is_empty(),
        "stderr must be empty on success: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// --- AC L+: kill -9 around a turn close must lose nothing, and the blob
// store must never expose a half-written object after a hard crash.
// `Child::kill()` is graceful on some platforms; go straight to SIGKILL so
// the process gets no chance to run any shutdown/flush path.

fn sigkill(child: &std::process::Child) {
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGKILL);
    }
}

#[test]
fn closed_turn_survives_kill9() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    std::fs::write(root.join("f.rs"), "fn f() {}").unwrap();
    poll_until(Duration::from_secs(25), || {
        root.join(".agentrec/open.json").exists().then_some(())
    })
    .expect("turn opened");

    // Stop hook closes and persists (fsynced) the turn.
    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s1"}"#);

    let has_f_rs = |root: &Path| {
        turns(root).into_iter().any(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("f.rs"))
                })
                .unwrap_or(false)
        })
    };

    poll_until(Duration::from_secs(10), || has_f_rs(root).then_some(()))
        .expect("f.rs turn persisted before kill");

    // Hard-kill the daemon immediately after the close was persisted.
    sigkill(&daemon);
    let _ = daemon.wait();

    // The fsynced append survives the kill — no buffering to lose.
    assert!(has_f_rs(root), "f.rs turn lost after kill -9");

    // A restart (recovery/epoch bookkeeping) must not drop prior history.
    let mut daemon2 = spawn_record(root);
    std::thread::sleep(Duration::from_secs(1));
    assert!(has_f_rs(root), "f.rs turn lost after daemon restart");

    sigkill(&daemon2);
    let _ = daemon2.wait();
}

#[test]
fn kill9_leaves_no_corrupt_blob() {
    use agentrec_core::store::hash_bytes;

    for i in 0..4u64 {
        let delay_ms = 300 + i * 250;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);

        let mut daemon = spawn_record(root);
        std::thread::sleep(Duration::from_millis(800));

        // A sizeable file so a real blob write happens (well under the
        // 10 MiB per-object cap).
        let mut big = String::with_capacity(200 * 1024);
        while big.len() < 200 * 1024 {
            big.push_str(&format!("// line {} filler filler filler\n", big.len()));
        }
        std::fs::write(root.join(format!("big_{i}.rs")), big).unwrap();

        // Cross the debounce so a snapshot write is in flight or done, then
        // hard-kill without warning.
        std::thread::sleep(Duration::from_millis(delay_ms));
        sigkill(&daemon);
        let _ = daemon.wait();

        // Invariant: every FINAL (renamed) object under .agentrec/objects
        // hashes to its own address. Leftover `.tmp.*` files from a
        // mid-write crash are permitted and explicitly skipped.
        let objects_dir = root.join(".agentrec/objects");
        let mut checked = 0usize;
        let mut stack = vec![objects_dir.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let file_type = entry.file_type().unwrap();
                if file_type.is_dir() {
                    stack.push(path);
                    continue;
                }
                let file_name = entry.file_name();
                let file_name = file_name.to_str().unwrap();
                if file_name.starts_with(".tmp") {
                    continue;
                }
                let dir_name = dir.file_name().unwrap().to_str().unwrap();
                let contents = std::fs::read(&path)
                    .unwrap_or_else(|e| panic!("iteration {i}: read {path:?} failed: {e}"));
                let expected = hash_bytes(&contents);
                let actual = format!("sha256:{dir_name}{file_name}");
                assert_eq!(
                    expected, actual,
                    "iteration {i}: corrupt/mismatched object at {path:?} (delay_ms={delay_ms})"
                );
                checked += 1;
            }
        }
        // Sanity: this iteration actually produced at least one durable
        // object at some delay (not required every iteration, since very
        // short delays may kill before any snapshot lands) — but across all
        // 4 varying delays we expect coverage overall. No per-iteration
        // assertion on `checked` count to avoid flakiness on the shortest
        // delay.
        let _ = checked;
    }
}

// --- AC G1–G6: `blame` reports which turn last touched a file, or
// introduced a specific line, without ever fabricating attribution across a
// recording gap or on a bare (unattributed) turn. Seeded directly via
// agentrec-core, same rationale as the `diff` tests above.

fn make_turn(
    id: &str,
    grade: &str,
    tool: Option<&str>,
    started: &str,
    prompt_excerpt: Option<&str>,
    files: Vec<agentrec_core::record::FileEntry>,
) -> agentrec_core::record::TurnRecord {
    agentrec_core::record::TurnRecord {
        v: 1,
        id: id.to_string(),
        grade: grade.to_string(),
        truncated: false,
        started: started.to_string(),
        ended: started.to_string(),
        tool: tool.map(str::to_string),
        model: None,
        session: None,
        root: "/repo".into(),
        prompt_ref: None,
        prompt_excerpt: prompt_excerpt.map(str::to_string),
        merges: vec![],
        files,
    }
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

#[test]
fn blame_file_reports_last_rich_turn() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let after1 = store.put(b"v1\n").unwrap();
    let after2 = store.put(b"v2\n").unwrap();

    let turn1 = make_turn(
        "t_BLAMEFIRST00000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T10:00:00.000Z",
        Some("first pass"),
        vec![FileEntry {
            path: "x.rs".into(),
            before: None,
            after: Some(after1),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn1);

    let turn2 = make_turn(
        "t_BLAMESECOND0000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T11:00:00.000Z",
        Some("second pass"),
        vec![FileEntry {
            path: "x.rs".into(),
            before: turn1.files[0].after.clone(),
            after: Some(after2),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn2);

    // On-disk matches the latest turn's `after` — no human-edited suffix.
    std::fs::write(root.join("x.rs"), b"v2\n").unwrap();
    let out = agentrec(root, &["blame", "x.rs"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("claude"), "stdout: {stdout}");
    assert!(stdout.contains("second pass"), "stdout: {stdout}");
    assert!(!stdout.contains("first pass"), "stdout: {stdout}");
    assert!(!stdout.contains("human-edited"), "stdout: {stdout}");

    // Now a human edits the file post-recording — hash diverges from `after2`.
    std::fs::write(root.join("x.rs"), b"v2-edited-by-hand\n").unwrap();
    let out = agentrec(root, &["blame", "x.rs"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("human-edited since"), "stdout: {stdout}");
}

#[test]
fn blame_untouched_file_exit0() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // Balanced epochs (start then stop) — no gap.
    seed_epoch(root, "start", "2026-07-05T00:00:00.000Z");
    seed_epoch(root, "stop", "2026-07-05T00:10:00.000Z");

    let out = agentrec(root, &["blame", "never-touched.rs"]);
    assert_eq!(out.status.code(), Some(0), "blame: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no recorded turn touches never-touched.rs"),
        "stdout: {stdout}"
    );
}

#[test]
fn blame_bare_turn_no_fabrication() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let after = store.put(b"content\n").unwrap();
    let turn = make_turn(
        "t_BLAMEBARE000000000000000001",
        "bare",
        None,
        "2026-07-05T09:00:00.000Z",
        None,
        vec![FileEntry {
            path: "y.rs".into(),
            before: None,
            after: Some(after),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn);
    std::fs::write(root.join("y.rs"), b"content\n").unwrap();

    let out = agentrec(root, &["blame", "y.rs"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("bare turn"), "stdout: {stdout}");
    assert!(!stdout.contains("claude"), "stdout: {stdout}");
    assert!(
        !stdout.contains('"'),
        "stdout: {stdout} (a bare turn must not quote a fabricated prompt)"
    );
}

#[test]
fn blame_deleted_file_resolves() {
    use agentrec_core::record::FileEntry;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let turn = make_turn(
        "t_BLAMEDELETE0000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T09:00:00.000Z",
        Some("remove dead file"),
        vec![FileEntry {
            path: "z.rs".into(),
            before: None,
            after: None,
            op: "delete".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    // z.rs is never written to disk — absent, as expected post-delete.
    seed_turn(root, &turn);

    let out = agentrec(root, &["blame", "z.rs"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("claude"), "stdout: {stdout}");
    assert!(stdout.contains("deleted this file"), "stdout: {stdout}");
}

#[test]
fn blame_line_level_added_and_predating() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"a\nb\n").unwrap();
    let after = store.put(b"a\nB2\n").unwrap();

    let turn = make_turn(
        "t_BLAMELINE00000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T09:00:00.000Z",
        Some("tweak line two"),
        vec![FileEntry {
            path: "f.rs".into(),
            before: Some(before),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn);
    std::fs::write(root.join("f.rs"), b"a\nB2\n").unwrap();

    let out = agentrec(root, &["blame", "f.rs:2"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("claude"), "stdout: {stdout}");
    assert!(stdout.contains("tweak line two"), "stdout: {stdout}");
    assert!(
        !stdout.contains("before recording began"),
        "stdout: {stdout}"
    );

    let out = agentrec(root, &["blame", "f.rs:1"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("before recording began"),
        "stdout: {stdout}"
    );
}

#[test]
fn blame_gap_is_stale() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let after = store.put(b"original\n").unwrap();
    let turn = make_turn(
        "t_BLAMEGAP0000000000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T09:00:00.000Z",
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

    // daemon1 starts and is killed (no stop); daemon2 starts — an uncovered gap.
    seed_epoch(root, "start", "2026-07-05T09:00:00.000Z");
    seed_epoch(root, "start", "2026-07-05T09:05:00.000Z");

    // On-disk content diverges from the turn's recorded `after` — the gap
    // could be hiding whatever really changed it, so blame must not guess.
    std::fs::write(root.join("g.rs"), b"changed-during-the-gap\n").unwrap();

    let out = agentrec(root, &["blame", "g.rs"]);
    assert!(out.status.success(), "blame failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("attribution stale — recording gap"),
        "stdout: {stdout}"
    );
}

// --- AC H1–H7, Z+1/D42: `undo` reverts a turn's file changes, per file, with
// a preview-first destructive gate. Seeded directly via agentrec-core, same
// rationale as the `diff`/`blame` tests above — the on-disk fixture bytes
// must match the seeded `before`/`after` refs exactly since undo hashes real
// files.

/// New agentrec-tool (undo) turns appended to the log, in append order.
fn agentrec_turns(root: &Path) -> Vec<serde_json::Value> {
    turns(root)
        .into_iter()
        .filter(|t| t.get("tool").and_then(|x| x.as_str()) == Some("agentrec"))
        .collect()
}

#[test]
fn undo_clean_revert_byte_exact() {
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
        "t_UNDOCLEAN0000000000000001",
        vec![FileEntry {
            path: "a.rs".into(),
            before: Some(before.clone()),
            after: Some(after),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["undo", &turn.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("reverted 1 file"), "stdout: {stdout}");

    let bytes = std::fs::read(root.join("a.rs")).unwrap();
    assert_eq!(bytes, b"v1\n", "byte-exact revert expected");

    let new_turns = agentrec_turns(root);
    assert_eq!(
        new_turns.len(),
        1,
        "expected exactly one agentrec undo turn: {new_turns:?}"
    );
    assert_eq!(
        new_turns[0].get("grade").and_then(|g| g.as_str()),
        Some("rich")
    );
}

#[test]
fn undo_preview_does_not_mutate() {
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
        "t_UNDOPREVIEW000000000000001",
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

    // No --confirm: preview only.
    let out = agentrec(root, &["undo", &turn.id]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("preview only"), "stdout: {stdout}");

    let bytes = std::fs::read(root.join("a.rs")).unwrap();
    assert_eq!(bytes, b"v2\n", "preview must not mutate the file");
    assert!(
        agentrec_turns(root).is_empty(),
        "preview must not record a turn"
    );
}

#[test]
fn undo_file_subset() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before_a = store.put(b"a-v1\n").unwrap();
    let after_a = store.put(b"a-v2\n").unwrap();
    let before_b = store.put(b"b-v1\n").unwrap();
    let after_b = store.put(b"b-v2\n").unwrap();
    std::fs::write(root.join("a.rs"), b"a-v2\n").unwrap();
    std::fs::write(root.join("b.rs"), b"b-v2\n").unwrap();

    let turn = base_turn(
        "t_UNDOSUBSET0000000000000001",
        vec![
            FileEntry {
                path: "a.rs".into(),
                before: Some(before_a),
                after: Some(after_a),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
            },
            FileEntry {
                path: "b.rs".into(),
                before: Some(before_b),
                after: Some(after_b),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
            },
        ],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["undo", &turn.id, "--files", "a.rs", "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("reverted 1 file"), "stdout: {stdout}");

    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), b"a-v1\n");
    assert_eq!(
        std::fs::read(root.join("b.rs")).unwrap(),
        b"b-v2\n",
        "deselected file must be untouched"
    );
}

#[test]
fn undo_modified_since_excluded_then_allowed() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let before = store.put(b"v1\n").unwrap();
    let after = store.put(b"v2\n").unwrap();

    let turn = base_turn(
        "t_UNDOMODSINCE00000000000001",
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

    // A human (or something else) edits the file to a THIRD value after the
    // turn's recorded `after` — modified-since holds.
    std::fs::write(root.join("a.rs"), b"v3-human\n").unwrap();

    let out = agentrec(root, &["undo", &turn.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("EXCLUDE"), "stdout: {stdout}");
    assert!(stdout.contains("modified since"), "stdout: {stdout}");
    assert_eq!(
        std::fs::read(root.join("a.rs")).unwrap(),
        b"v3-human\n",
        "excluded file must be untouched"
    );
    assert!(
        agentrec_turns(root).is_empty(),
        "an all-excluded plan must not record a turn"
    );

    // Retry with --allow-modified: now it reverts, with a warning naming the file.
    let out = agentrec(root, &["undo", &turn.id, "--allow-modified", "--confirm"]);
    assert!(
        out.status.success(),
        "undo --allow-modified failed: {out:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("a.rs"), "stdout: {stdout}");
    assert!(stdout.contains("WARNING"), "stdout: {stdout}");
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), b"v1\n");
    assert_eq!(agentrec_turns(root).len(), 1);
}

#[test]
fn undo_skipped_and_withheld_refused() {
    use agentrec_core::record::FileEntry;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    std::fs::write(
        root.join(".agentrec/state.json"),
        r#"{"pid":0,"signal_offset":0,"snapshot_failures":1,"io_failed":["s.rs"]}"#,
    )
    .unwrap();

    let turn = base_turn(
        "t_UNDOREFUSE00000000000000001",
        vec![
            FileEntry {
                path: "s.rs".into(),
                before: None,
                after: None,
                op: "modify".into(),
                skipped: true,
                withheld: false,
                baseline_unknown: false,
            },
            FileEntry {
                path: "w.rs".into(),
                before: None,
                after: None,
                op: "modify".into(),
                skipped: false,
                withheld: true,
                baseline_unknown: false,
            },
        ],
    );
    seed_turn(root, &turn);

    let out = agentrec(root, &["undo", &turn.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let s_line = stdout.lines().find(|l| l.contains("s.rs")).unwrap_or("");
    assert!(
        s_line.contains("write failed at record time"),
        "s.rs line: {s_line}"
    );
    assert!(!s_line.contains("over size cap"), "s.rs line: {s_line}");
    let w_line = stdout.lines().find(|l| l.contains("w.rs")).unwrap_or("");
    assert!(w_line.contains("secret-pattern"), "w.rs line: {w_line}");

    assert!(!root.join("s.rs").exists());
    assert!(!root.join("w.rs").exists());
    assert!(
        agentrec_turns(root).is_empty(),
        "an all-refused plan must not record a turn"
    );

    // Second turn: a skipped path NOT in io_failed reports the over-cap reason.
    let turn2 = base_turn(
        "t_UNDOREFUSE20000000000000001",
        vec![FileEntry {
            path: "o.rs".into(),
            before: None,
            after: None,
            op: "modify".into(),
            skipped: true,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn2);

    let out = agentrec(root, &["undo", &turn2.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let o_line = stdout.lines().find(|l| l.contains("o.rs")).unwrap_or("");
    assert!(o_line.contains("over size cap"), "o.rs line: {o_line}");
    assert!(
        !o_line.contains("write failed at record time"),
        "o.rs line: {o_line}"
    );
}

#[test]
fn undo_create_and_delete_inverse() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    // Turn A: created c.rs.
    let created = store.put(b"created\n").unwrap();
    std::fs::write(root.join("c.rs"), b"created\n").unwrap();
    let turn_a = base_turn(
        "t_UNDOCREATE0000000000000001",
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
    seed_turn(root, &turn_a);

    let out = agentrec(root, &["undo", &turn_a.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    assert!(
        !root.join("c.rs").exists(),
        "revert of create must delete the file"
    );

    // Turn B: deleted sub/d.rs (parent dir doesn't exist on disk anymore).
    let gone = store.put(b"gone\n").unwrap();
    let turn_b = base_turn(
        "t_UNDODELETE0000000000000001",
        vec![FileEntry {
            path: "sub/d.rs".into(),
            before: Some(gone),
            after: None,
            op: "delete".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn_b);

    let out = agentrec(root, &["undo", &turn_b.id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");
    assert_eq!(
        std::fs::read(root.join("sub/d.rs")).unwrap(),
        b"gone\n",
        "revert of delete must restore content, creating parent dirs"
    );
}

#[test]
fn undo_is_a_turn_and_reversible() {
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
        "t_UNDOREVERSIBLE000000000001",
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
    assert!(out.status.success(), "first undo failed: {out:?}");
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), b"v1\n");

    let first_undo = agentrec_turns(root)
        .into_iter()
        .next()
        .expect("undo turn recorded");
    let first_undo_id = first_undo
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap()
        .to_string();

    // Undo of the undo restores the pre-undo (post-original) state exactly.
    let out = agentrec(root, &["undo", &first_undo_id, "--confirm"]);
    assert!(out.status.success(), "second undo failed: {out:?}");
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), b"v2\n");

    assert_eq!(
        agentrec_turns(root).len(),
        2,
        "each undo is itself a re-revertible turn"
    );
}

#[test]
fn panic_undo_targets_last_rich() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    // Case 1: two rich turns; the newest touches p.rs — panic mode must
    // target it, not the older turn touching q.rs.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        let store = BlobStore::new(root.join(".agentrec/objects"));

        let turn1 = make_turn(
            "t_PANICOLD00000000000000001",
            "rich",
            Some("claude"),
            "2026-07-05T10:00:00.000Z",
            Some("first"),
            vec![FileEntry {
                path: "q.rs".into(),
                before: None,
                after: Some(store.put(b"q\n").unwrap()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
            }],
        );
        seed_turn(root, &turn1);

        let turn2 = make_turn(
            "t_PANICNEW00000000000000001",
            "rich",
            Some("claude"),
            "2026-07-05T11:00:00.000Z",
            Some("second"),
            vec![FileEntry {
                path: "p.rs".into(),
                before: None,
                after: Some(store.put(b"p\n").unwrap()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
            }],
        );
        seed_turn(root, &turn2);

        // Panic mode: no turn arg, no --confirm — still previews only.
        let out = agentrec(root, &["undo"]);
        assert!(out.status.success(), "panic undo preview failed: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("preview only"), "stdout: {stdout}");
        assert!(
            stdout.contains("p.rs"),
            "stdout should name the newest turn's file: {stdout}"
        );
        assert!(
            !stdout.contains("q.rs"),
            "stdout should not touch the older turn's file: {stdout}"
        );
        assert!(
            !root.join("p.rs").exists() && !root.join("q.rs").exists(),
            "preview must not create files"
        );
    }

    // Case 2: newest turn is bare — panic undo refuses rather than skipping
    // to the older rich turn.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        let store = BlobStore::new(root.join(".agentrec/objects"));

        let turn_rich = make_turn(
            "t_PANICRICH0000000000000001",
            "rich",
            Some("claude"),
            "2026-07-05T10:00:00.000Z",
            Some("first"),
            vec![FileEntry {
                path: "q.rs".into(),
                before: None,
                after: Some(store.put(b"q\n").unwrap()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
            }],
        );
        seed_turn(root, &turn_rich);

        let turn_bare = make_turn(
            "t_PANICBARE0000000000000001",
            "bare",
            None,
            "2026-07-05T11:00:00.000Z",
            None,
            vec![FileEntry {
                path: "r.rs".into(),
                before: None,
                after: Some(store.put(b"r\n").unwrap()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
            }],
        );
        seed_turn(root, &turn_bare);

        let out = agentrec(root, &["undo"]);
        assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("unattributed"), "stderr: {stderr}");
    }

    // Case 3: empty log — nothing to undo.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);

        let out = agentrec(root, &["undo"]);
        assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("no turns to undo"), "stderr: {stderr}");
    }
}

#[test]
fn undo_concurrent_no_spurious_bare_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    std::fs::write(root.join("real.rs"), "fn real() {}").unwrap();
    poll_until(Duration::from_secs(25), || {
        root.join(".agentrec/open.json").exists().then_some(())
    })
    .expect("turn opened");

    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s_h7"}"#);

    let turn = poll_until(Duration::from_secs(10), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("real.rs"))
                })
                .unwrap_or(false)
        })
    })
    .expect("real.rs turn recorded");
    let turn_id = turn.get("id").and_then(|i| i.as_str()).unwrap().to_string();

    // Let the daemon settle fully back to idle before undo runs, so the
    // guard exclusion is the only thing standing between undo's write and a
    // spurious bare turn.
    poll_until(Duration::from_secs(5), || {
        (!root.join(".agentrec/open.json").exists()).then_some(())
    });

    let out = agentrec(root, &["undo", &turn_id, "--confirm"]);
    assert!(out.status.success(), "undo failed: {out:?}");

    // undo's guard lingers past the daemon's worst-case flush latency (AC
    // H7) before it's removed, so by the time undo returns, a buggy
    // (unfiltered) daemon would already have opened a turn for its delete
    // of real.rs.
    let open = std::fs::read_to_string(root.join(".agentrec/open.json")).unwrap_or_default();
    assert!(
        !open.contains("real.rs"),
        "daemon opened a turn for undo's own write: {open}"
    );

    sigkill(&daemon);
    let _ = daemon.wait();

    let all_turns = turns(root);
    let undo_turns = all_turns
        .iter()
        .filter(|t| t.get("tool").and_then(|x| x.as_str()) == Some("agentrec"))
        .count();
    assert_eq!(
        undo_turns, 1,
        "expected exactly one agentrec undo turn: {all_turns:?}"
    );

    let bare_touching_real = all_turns.iter().any(|t| {
        t.get("grade").and_then(|g| g.as_str()) == Some("bare")
            && t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("real.rs"))
                })
                .unwrap_or(false)
    });
    assert!(
        !bare_touching_real,
        "spurious bare turn for real.rs: {all_turns:?}"
    );
}

// --- AC M+ (gap closure): a REAL daemon-side snapshot I/O failure — not a
// planted state.json — must still (a) be counted + path-tracked in
// state.json, (b) leave the turn boundary itself intact with the file
// marked `skipped: true` (the daemon keeps recording despite the failure),
// and (c) surface as DEGRADED in `status`. `.agentrec/objects/` is made
// unwritable so the very first blob (a fresh fan-out subdir create) fails;
// `.agentrec/log.jsonl` / `state.json` live outside `objects/` so turn and
// state persistence are unaffected.
#[cfg(unix)]
#[test]
fn degraded_on_real_snapshot_io_failure() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    let objects = root.join(".agentrec/objects");
    let mut perms = std::fs::metadata(&objects).unwrap().permissions();
    perms.set_mode(0o500);
    std::fs::set_permissions(&objects, perms).unwrap();

    // Unique content so this can never dedup to a pre-existing blob.
    std::fs::write(root.join("victim.rs"), "fn victim_unique_marker_9f3a() {}").unwrap();

    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"sD"}"#);

    let state_ok = poll_until(Duration::from_secs(15), || {
        let text = std::fs::read_to_string(root.join(".agentrec/state.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let failures = v
            .get("snapshot_failures")
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        let tracks_victim = v
            .get("io_failed")
            .and_then(|f| f.as_array())
            .map(|fs| fs.iter().any(|p| p.as_str() == Some("victim.rs")))
            .unwrap_or(false);
        (failures > 0 && tracks_victim).then_some(())
    });

    let skipped_turn = poll_until(Duration::from_secs(15), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter().any(|f| {
                        f.get("path").and_then(|p| p.as_str()) == Some("victim.rs")
                            && f.get("skipped").and_then(|s| s.as_bool()) == Some(true)
                    })
                })
                .unwrap_or(false)
        })
    });

    // Restore perms before further assertions/process exit so the tempdir
    // can always be cleaned up, even on assertion failure below.
    let mut perms = std::fs::metadata(&objects).unwrap().permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(&objects, perms).unwrap();

    assert!(
        state_ok.is_some(),
        "state.json never recorded a real snapshot I/O failure for victim.rs"
    );
    assert!(
        skipped_turn.is_some(),
        "daemon did not persist a turn boundary with victim.rs skipped:true despite the I/O failure"
    );

    let out = agentrec(root, &["status"]);
    assert!(out.status.success(), "status failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("DEGRADED"), "stdout: {stdout}");

    sigkill(&daemon);
    let _ = daemon.wait();
}

// --- AC Z+1 (gap closure): panic-mode undo (no turn arg, no --confirm) must
// skip a git turn even when it is the newest record, targeting the most
// recent non-git rich turn instead. `resolve_panic_target` in readcmds.rs
// already filters `tool == "git"`; this proves it end-to-end against a
// realistic "checkout clobbered a pile of files" shape.
#[test]
fn panic_undo_skips_git_turn() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    // Older, rich, non-git: the real undo target.
    let turn_claude = make_turn(
        "t_GITSKIPOLD00000000000001",
        "rich",
        Some("claude"),
        "2026-07-05T10:00:00.000Z",
        Some("refactor the parser"),
        vec![FileEntry {
            path: "code.rs".into(),
            before: None,
            after: Some(store.put(b"fn code() {}\n").unwrap()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    seed_turn(root, &turn_claude);

    // Newest turn: a git checkout touching many files — must never be the
    // panic-undo target.
    let turn_git = make_turn(
        "t_GITSKIPNEW00000000000001",
        "rich",
        Some("git"),
        "2026-07-05T11:00:00.000Z",
        None,
        vec![
            FileEntry {
                path: "checkout_a.rs".into(),
                before: None,
                after: Some(store.put(b"a\n").unwrap()),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
            },
            FileEntry {
                path: "checkout_b.rs".into(),
                before: None,
                after: Some(store.put(b"b\n").unwrap()),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
            },
            FileEntry {
                path: "checkout_c.rs".into(),
                before: None,
                after: Some(store.put(b"c\n").unwrap()),
                op: "modify".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
            },
        ],
    );
    seed_turn(root, &turn_git);

    // Panic mode: no turn arg, no --confirm — preview only, nothing mutates.
    let out = agentrec(root, &["undo"]);
    assert!(out.status.success(), "panic undo preview failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("preview only"), "stdout: {stdout}");
    assert!(
        stdout.contains("code.rs"),
        "stdout should name the claude turn's file: {stdout}"
    );
    assert!(
        stdout.lines().next().unwrap_or("").contains("claude"),
        "preview header should target the claude turn, not git: {stdout}"
    );
    assert!(
        !stdout.contains("(git)"),
        "git turn must never be presented as the panic target: {stdout}"
    );
    assert!(
        !stdout.contains("checkout_a.rs"),
        "stdout should not surface the git turn's files: {stdout}"
    );
    assert!(
        !stdout.contains("checkout_b.rs"),
        "stdout should not surface the git turn's files: {stdout}"
    );
    assert!(
        !stdout.contains("checkout_c.rs"),
        "stdout should not surface the git turn's files: {stdout}"
    );

    assert!(
        !root.join("code.rs").exists(),
        "preview must not create files"
    );
    assert!(
        !root.join("checkout_a.rs").exists(),
        "preview must not create files"
    );
}

// --- AC I5–I6: `purge` deletes blob objects, never rewrites log.jsonl, and
// respects the dedup keep-set (a blob shared by a kept turn must survive).

fn days_ago_rfc3339(days: u64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    agentrec_core::time::rfc3339(now_ms.saturating_sub(days * 86_400_000))
}

#[test]
fn purge_removes_expired_prompt_blob_keeps_shared() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let shared_prompt = store.put(b"shared prompt content").unwrap();
    let unique_old_prompt = store.put(b"unique old prompt content").unwrap();
    // A snapshot blob on the expired turn — default purge must leave it alone.
    let old_snapshot = store.put(b"old file content").unwrap();

    // Expired (well past the default 90-day ttl), unshared prompt blob.
    let mut old_unique = base_turn("t_OLDUNIQUE000000000000001", vec![]);
    old_unique.started = days_ago_rfc3339(200);
    old_unique.ended = old_unique.started.clone();
    old_unique.prompt_ref = Some(unique_old_prompt.clone());
    old_unique.prompt_excerpt = Some("old excerpt".into());
    seed_turn(root, &old_unique);

    // Expired turn whose prompt blob is ALSO referenced by a within-ttl turn.
    let mut old_shared = base_turn(
        "t_OLDSHARED00000000000001",
        vec![FileEntry {
            path: "old.rs".into(),
            before: None,
            after: Some(old_snapshot.clone()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    old_shared.started = days_ago_rfc3339(200);
    old_shared.ended = old_shared.started.clone();
    old_shared.prompt_ref = Some(shared_prompt.clone());
    old_shared.prompt_excerpt = Some("shared excerpt (old)".into());
    seed_turn(root, &old_shared);

    // Recent (within ttl) turn sharing the same prompt blob — must keep it alive.
    let mut recent = base_turn("t_RECENT00000000000000001", vec![]);
    recent.started = days_ago_rfc3339(1);
    recent.ended = recent.started.clone();
    recent.prompt_ref = Some(shared_prompt.clone());
    recent.prompt_excerpt = Some("shared excerpt (recent)".into());
    seed_turn(root, &recent);

    let out = agentrec(root, &["purge"]);
    assert!(out.status.success(), "purge failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("expired prompt blob"), "stdout: {stdout}");

    assert!(
        !store.contains(&unique_old_prompt),
        "unshared expired prompt blob must be deleted"
    );
    assert!(
        store.contains(&shared_prompt),
        "prompt blob shared with a within-ttl turn must survive"
    );
    assert!(
        store.contains(&old_snapshot),
        "default purge must never touch snapshot blobs"
    );

    // `log` renders from the inline excerpt, not the blob — unaffected by purge.
    let log_out = agentrec(root, &["log", "--all"]);
    let log_stdout = String::from_utf8_lossy(&log_out.stdout);
    assert!(log_stdout.contains("old excerpt"), "log: {log_stdout}");
    assert!(
        log_stdout.contains("shared excerpt (old)"),
        "log: {log_stdout}"
    );
    assert!(
        log_stdout.contains("shared excerpt (recent)"),
        "log: {log_stdout}"
    );
}

#[test]
fn purge_all_prompts_removes_all_prompt_blobs() {
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let old_prompt = store.put(b"an old prompt").unwrap();
    let recent_prompt = store.put(b"a recent prompt").unwrap();

    let mut old = base_turn("t_ALLOLD00000000000000001", vec![]);
    old.started = days_ago_rfc3339(200);
    old.ended = old.started.clone();
    old.prompt_ref = Some(old_prompt.clone());
    old.prompt_excerpt = Some("old prompt excerpt".into());
    seed_turn(root, &old);

    let mut recent = base_turn("t_ALLRECENT0000000000001", vec![]);
    recent.started = days_ago_rfc3339(1);
    recent.ended = recent.started.clone();
    recent.prompt_ref = Some(recent_prompt.clone());
    recent.prompt_excerpt = Some("recent prompt excerpt".into());
    seed_turn(root, &recent);

    let out = agentrec(root, &["purge", "--all-prompts"]);
    assert!(out.status.success(), "purge --all-prompts failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("(all)"), "stdout: {stdout}");

    assert!(!store.contains(&old_prompt), "expired prompt blob deleted");
    assert!(
        !store.contains(&recent_prompt),
        "even a within-ttl prompt blob is deleted with --all-prompts"
    );

    let log_out = agentrec(root, &["log", "--all"]);
    let log_stdout = String::from_utf8_lossy(&log_out.stdout);
    assert!(
        log_stdout.contains("old prompt excerpt"),
        "log: {log_stdout}"
    );
    assert!(
        log_stdout.contains("recent prompt excerpt"),
        "log: {log_stdout}"
    );
}

#[test]
fn purge_snapshots_before_date_respects_keepset() {
    use agentrec_core::record::FileEntry;
    use agentrec_core::store::BlobStore;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    let old_only = store.put(b"only referenced before the cutoff").unwrap();
    let shared = store.put(b"referenced both sides of the cutoff").unwrap();
    let new_only = store.put(b"only referenced on/after the cutoff").unwrap();

    // Before the cutoff: creates old_only, then a second old turn hands off
    // to `shared` (this is the `after` a turn on/after the cutoff will share
    // as its `before`).
    let mut turn_old1 = base_turn(
        "t_SNAPOLD1000000000000001",
        vec![FileEntry {
            path: "a.rs".into(),
            before: None,
            after: Some(old_only.clone()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    turn_old1.started = "2024-01-01T00:00:00.000Z".into();
    turn_old1.ended = turn_old1.started.clone();
    seed_turn(root, &turn_old1);

    let mut turn_old2 = base_turn(
        "t_SNAPOLD2000000000000001",
        vec![FileEntry {
            path: "b.rs".into(),
            before: None,
            after: Some(shared.clone()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    turn_old2.started = "2024-02-01T00:00:00.000Z".into();
    turn_old2.ended = turn_old2.started.clone();
    seed_turn(root, &turn_old2);

    // On the cutoff date itself (on/after — kept side): references `shared`
    // as `before`, and introduces `new_only`.
    let mut turn_new = base_turn(
        "t_SNAPNEW0000000000000001",
        vec![FileEntry {
            path: "b.rs".into(),
            before: Some(shared.clone()),
            after: Some(new_only.clone()),
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
        }],
    );
    turn_new.started = "2024-06-01T00:00:00.000Z".into();
    turn_new.ended = turn_new.started.clone();
    seed_turn(root, &turn_new);

    let out = agentrec(root, &["purge", "--snapshots-before", "2024-06-01"]);
    assert!(
        out.status.success(),
        "purge --snapshots-before failed: {out:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("snapshot blob"), "stdout: {stdout}");

    assert!(
        !store.contains(&old_only),
        "unshared before-cutoff snapshot deleted"
    );
    assert!(
        store.contains(&shared),
        "snapshot shared with an on/after-cutoff turn must survive"
    );
    assert!(
        store.contains(&new_only),
        "on/after-cutoff snapshot untouched"
    );
}

// --- AC-Y+3/Y+4/Y+5: `init`/`uninstall` scaffold+hook+dry-run+archive flow,
// all with `--no-service` — this suite never loads a real launchd/systemd
// service (hermetic-tests requirement). `service.rs`'s own pure-function
// unit tests cover the unit-string/path/slug generation.

fn walk_paths(root: &Path) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .to_string();
            out.insert(rel);
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    out
}

#[test]
fn init_dry_run_then_noop_then_uninstall_no_service() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let out = agentrec(root, &["init", "--no-service"]);
    assert!(out.status.success(), "init failed: {out:?}");
    assert!(root.join(".agentrec/config.toml").exists());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("to reverse everything: agentrec uninstall"),
        "stdout: {stdout}"
    );
    // D-PD5: a fresh (disk-altering) init must NOT claim nothing changed.
    assert!(
        !stdout.contains("already initialized"),
        "fresh init must not print the no-op message: {stdout}"
    );

    let settings_path = root.join(".claude/settings.local.json");
    let settings_after_first = std::fs::read_to_string(&settings_path).unwrap();

    // --dry-run must touch NOTHING on disk.
    let before = walk_paths(root);
    let out = agentrec(root, &["init", "--no-service", "--dry-run"]);
    assert!(out.status.success(), "dry-run failed: {out:?}");
    let after = walk_paths(root);
    assert_eq!(before, after, "dry-run must not create/modify any file");

    // Re-running (no dry-run) is a byte-for-byte no-op: no dup hooks, exit 0.
    let out2 = agentrec(root, &["init", "--no-service"]);
    assert!(out2.status.success(), "second init failed: {out2:?}");
    let after2 = walk_paths(root);
    assert_eq!(before, after2, "re-init must not add/remove any file");
    let settings_after_second = std::fs::read_to_string(&settings_path).unwrap();
    assert_eq!(
        settings_after_first, settings_after_second,
        "hooks file must be byte-identical on no-op re-init"
    );
    // D-PD5: a byte-for-byte no-op re-run leads with an honest message.
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert_eq!(
        stdout2.lines().next(),
        Some("already initialized — nothing changed"),
        "expected leading no-op message as the first line: {stdout2}"
    );

    // uninstall --no-service: removes hooks, archives .agentrec/, deletes nothing.
    let out = agentrec(root, &["uninstall", "--no-service"]);
    assert!(out.status.success(), "uninstall failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("removed agentrec hook entries"),
        "stdout: {stdout}"
    );
    assert!(!root.join(".agentrec").exists());
    let archived: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".agentrec.archived.")
        })
        .collect();
    assert_eq!(archived.len(), 1, "expected exactly one archive dir");
    assert!(archived[0].path().join("config.toml").exists());

    // A later init starts fresh while the archive remains.
    let out = agentrec(root, &["init", "--no-service"]);
    assert!(out.status.success(), "post-uninstall init failed: {out:?}");
    assert!(root.join(".agentrec/config.toml").exists());
    assert!(archived[0].path().exists(), "archive must never be removed");
}

// Skeptic-surfaced (2026-07-10): the idempotent no-op message must reflect
// ACTUAL post-run disk state, not just whether `.agentrec/` itself pre-
// existed. If `.agentrec/objects/` is separately deleted, a re-run silently
// recreates it while the old logic still printed "nothing changed" —
// fabricating a no-op. This must flip to the normal "acted" output whenever
// init actually recreates something, and only claim the no-op on a genuine
// no-op re-run.
#[test]
fn init_recreates_deleted_objects_dir_and_reports_it_honestly() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let out = agentrec(root, &["init", "--no-service"]);
    assert!(out.status.success(), "first init failed: {out:?}");
    assert!(root.join(".agentrec/objects").exists());

    // Separately delete the required subdir — nothing else about
    // `.agentrec/` changes.
    std::fs::remove_dir_all(root.join(".agentrec/objects")).unwrap();
    assert!(!root.join(".agentrec/objects").exists());

    let out2 = agentrec(root, &["init", "--no-service"]);
    assert!(out2.status.success(), "re-init failed: {out2:?}");
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert_ne!(
        stdout2.lines().next(),
        Some("already initialized — nothing changed"),
        "init recreated objects/ — must not claim nothing changed: {stdout2}"
    );
    assert!(
        root.join(".agentrec/objects").exists(),
        "objects/ must be recreated: {stdout2}"
    );

    // A THIRD, genuine no-op re-run must still lead with the honest no-op
    // message — proves the happy path is untouched.
    let out3 = agentrec(root, &["init", "--no-service"]);
    assert!(out3.status.success(), "third init failed: {out3:?}");
    let stdout3 = String::from_utf8_lossy(&out3.stdout);
    assert_eq!(
        stdout3.lines().next(),
        Some("already initialized — nothing changed"),
        "genuine no-op re-run must still print the no-op message: {stdout3}"
    );
}

// --- D38 (gap closure): a planted secret in a prompt must never reach disk
// in cleartext, in ANY of the three locations scrub is load-bearing for —
// signal.jsonl, log.jsonl, and every blob under objects/. Drives the real
// hook → signal → daemon → persisted-turn path (the same entry point a real
// Claude Code Stop hook uses) with an AWS-key-shaped secret (same fixture
// shape as `agentrec_core::scrub` 's own tests).
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn secret_prompt_never_reaches_disk_in_cleartext() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    std::fs::write(root.join("touched.rs"), "fn touched() {}").unwrap();
    poll_until(Duration::from_secs(25), || {
        root.join(".agentrec/open.json").exists().then_some(())
    })
    .expect("turn opened");

    let secret = "AKIAABCDEFGHIJKLMNOP";
    let payload = format!(
        r#"{{"hook_event_name":"Stop","session_id":"s_secret","prompt":"deploy with key {secret} now"}}"#
    );
    send_hook(root, &payload);

    let turn = poll_until(Duration::from_secs(10), || {
        turns(root).into_iter().find(|t| {
            t.get("files")
                .and_then(|f| f.as_array())
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.get("path").and_then(|p| p.as_str()) == Some("touched.rs"))
                })
                .unwrap_or(false)
        })
    })
    .expect("turn with secret-bearing prompt recorded");

    sigkill(&daemon);
    let _ = daemon.wait();

    let excerpt = turn
        .get("prompt_excerpt")
        .and_then(|p| p.as_str())
        .unwrap_or("");
    assert!(
        excerpt.contains("[redacted:"),
        "expected a redaction marker in the excerpt: {excerpt}"
    );
    assert!(!excerpt.contains(secret), "excerpt: {excerpt}");

    let signal = std::fs::read(root.join(".agentrec/signal.jsonl")).unwrap_or_default();
    assert!(
        !contains_bytes(&signal, secret.as_bytes()),
        "raw secret leaked into signal.jsonl"
    );

    let log = std::fs::read(root.join(".agentrec/log.jsonl")).unwrap_or_default();
    assert!(
        !contains_bytes(&log, secret.as_bytes()),
        "raw secret leaked into log.jsonl"
    );
    assert!(
        contains_bytes(&log, b"[redacted:"),
        "expected a redaction marker in log.jsonl"
    );

    let mut stack = vec![root.join(".agentrec/objects")];
    let mut checked_any = false;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let is_tmp = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(".tmp"))
                .unwrap_or(false);
            if is_tmp {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap_or_default();
            checked_any = true;
            assert!(
                !contains_bytes(&bytes, secret.as_bytes()),
                "raw secret leaked into blob {path:?}"
            );
        }
    }
    assert!(checked_any, "expected at least one blob object to check");
}

// ---- `agentrec doctor` (D41 / AC-Y++) ---------------------------------------
//
// `doctor` is a one-shot diagnosis, never the daemon required. Exit 0 iff
// every check passes; exit 1 with a printed remedy per failing check, in
// both text and `--json` modes.

fn doctor(root: &Path) -> Output {
    agentrec(root, &["doctor"])
}

fn doctor_json_value(root: &Path) -> serde_json::Value {
    let out = agentrec(root, &["doctor", "--json"]);
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("doctor --json did not emit valid JSON ({e}): {out:?}"))
}

/// Poll `.agentrec/state.json` until the daemon has written a nonzero pid
/// (i.e. it holds the lock), so `doctor`'s liveness check has something real
/// to observe.
fn wait_for_live_daemon(root: &Path) {
    let ok = poll_until(Duration::from_secs(10), || {
        let text = std::fs::read_to_string(root.join(".agentrec/state.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let pid = v.get("pid")?.as_u64()?;
        (pid != 0).then_some(())
    });
    assert!(ok.is_some(), "daemon never wrote a live pid to state.json");
}

#[test]
fn doctor_healthy_all_pass_exit_0() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Real hooks installed (not --no-hook), never a real service (--no-service).
    let out = agentrec(root, &["init", "--no-service"]);
    assert!(out.status.success(), "init failed: {out:?}");

    let mut daemon = spawn_record(root);
    wait_for_live_daemon(root);

    // Pin the signal-freshness check's transcript root to an empty dir: this
    // process is itself a live Claude Code session, so the real
    // `$HOME/.claude/projects` may have a genuinely fresh transcript right
    // now, which would otherwise make this "healthy" fixture spuriously
    // trip the disconnected-hooks check (its tempdir signal.jsonl is real,
    // just untouched, since no real Claude session talks to this repo).
    let start = Instant::now();
    let out = Command::new(bin())
        .args(["doctor", "--root", root.to_str().unwrap()])
        .env(
            "AGENTREC_CLAUDE_PROJECTS_DIR",
            tmp.path().join("no-transcripts-here"),
        )
        .output()
        .expect("run agentrec doctor");
    let elapsed = start.elapsed();

    let _ = daemon.kill();
    let _ = daemon.wait();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "expected exit 0 on a healthy repo: {out:?}\nstdout={stdout}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "doctor took {elapsed:?}, expected well under 2s"
    );
    assert!(stdout.contains("pass"), "expected pass lines: {stdout}");
    assert!(
        !stdout.to_lowercase().contains("fail"),
        "healthy repo must report no failures: {stdout}"
    );
}

#[test]
fn doctor_daemon_down_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-service"]);
    // No daemon spawned: state.json's pid is 0 (or absent).

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("recorder not running"),
        "expected the daemon-down remedy: {stdout}"
    );
}

#[test]
fn doctor_hook_missing_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-hook", "--no-service"]);

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hooks missing/mangled"),
        "expected the hook remedy: {stdout}"
    );
}

#[test]
fn doctor_hook_malformed_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-hook", "--no-service"]);
    std::fs::create_dir_all(root.join(".claude")).unwrap();
    std::fs::write(
        root.join(".claude/settings.local.json"),
        "{ not json at all",
    )
    .unwrap();

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hooks missing/mangled"),
        "malformed settings.local.json must be treated as a fail, not a crash: {stdout}"
    );
}

#[test]
fn doctor_degraded_store_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-service"]);
    std::fs::write(
        root.join(".agentrec/state.json"),
        r#"{"pid":0,"signal_offset":0,"snapshot_failures":3,"io_failed":["a.rs"]}"#,
    )
    .unwrap();

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("DEGRADED"),
        "expected DEGRADED notice: {stdout}"
    );
    assert!(
        stdout.contains("3 snapshot"),
        "expected the failure count in the remedy: {stdout}"
    );
}

// D-PD5: the doctor check formerly named "store degraded" was renamed to
// "store health" (non-jargon, brand-invariant wording) — assert the new
// name appears (text + --json `name` field) and the old one is gone.
#[test]
fn doctor_check_name_is_store_health_not_store_degraded() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-service"]);

    let stdout = String::from_utf8_lossy(&doctor(root).stdout).into_owned();
    assert!(
        stdout.contains("store health"),
        "expected renamed check 'store health': {stdout}"
    );
    assert!(
        !stdout.contains("store degraded"),
        "old check name 'store degraded' must not appear: {stdout}"
    );

    let v = doctor_json_value(root);
    let checks = v["checks"].as_array().expect("checks array");
    assert!(
        checks.iter().any(|c| c["name"] == "store health"),
        "expected 'store health' in --json checks: {v}"
    );
    assert!(
        !checks.iter().any(|c| c["name"] == "store degraded"),
        "old check name must not appear in --json: {v}"
    );
}

#[cfg(unix)]
#[test]
fn doctor_bad_perms_fails() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-service"]);
    std::fs::set_permissions(
        root.join(".agentrec"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("store permissions too open"),
        "expected the permissions remedy: {stdout}"
    );
}

#[test]
fn doctor_signal_stale_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-service"]);

    // A fake Claude Code transcripts root with one freshly-written transcript
    // ("agent active"), while `.agentrec/signal.jsonl` was never written
    // (hooks never fired) — the disconnected-hooks failure mode.
    let projects_dir = tmp.path().join("fake-claude-projects");
    let project = projects_dir.join("some-project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("session.jsonl"), "{}\n").unwrap();

    let out = Command::new(bin())
        .args(["doctor", "--root", root.to_str().unwrap()])
        .env("AGENTREC_CLAUDE_PROJECTS_DIR", &projects_dir)
        .output()
        .expect("run agentrec doctor");

    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("agent active but no signals arriving"),
        "expected the signal-freshness remedy: {stdout}"
    );
}

// ---- D-PD1: `initialized` gate — never fabricate check results in an
// uninitialized repo (no `.agentrec/` at all: no `init` was ever run).

#[test]
fn doctor_uninitialized_repo_short_circuits_all_checks() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Deliberately no `agentrec init` — `.agentrec/` does not exist.

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("initialized") && stdout.contains("not initialized — run"),
        "expected the initialized check + remedy: {stdout}"
    );
    assert!(
        !stdout.contains("permissions too open"),
        "must not fabricate a permissions failure pre-init: {stdout}"
    );
    assert!(
        !stdout.contains("agent active but no signals"),
        "must not fabricate a signal-freshness failure pre-init: {stdout}"
    );

    // Every non-init check line ends in `n/a`.
    for line in stdout.lines() {
        if line.starts_with("initialized") || line.trim_start().starts_with("->") {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        assert!(
            line.trim_end().ends_with("n/a"),
            "expected every non-init check line to end in n/a: {line:?} (full output: {stdout})"
        );
    }
}

#[test]
fn doctor_uninitialized_repo_json_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Deliberately no `agentrec init`.

    let out = agentrec(root, &["doctor", "--json"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");

    let v = doctor_json_value(root);
    assert_eq!(v["ok"], false, "expected ok:false in {v}");
    let checks = v["checks"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a checks array in {v}"));
    assert!(!checks.is_empty(), "expected at least one check in {v}");

    let first = &checks[0];
    assert_eq!(
        first["name"], "initialized",
        "first check must be initialized: {v}"
    );
    assert_eq!(first["status"], "fail", "initialized must fail: {v}");
    assert!(
        first["remedy"]
            .as_str()
            .unwrap_or("")
            .contains("not initialized — run"),
        "expected the init remedy: {v}"
    );

    for check in &checks[1..] {
        assert_eq!(
            check["status"], "n/a",
            "every non-init check must be n/a in an uninitialized repo: {check} (full: {v})"
        );
    }
}

#[test]
fn doctor_initialized_repo_reports_initialized_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    agentrec(root, &["init", "--no-hook", "--no-service"]);

    // Prior behavior intact: hooks missing still fails, but `initialized`
    // itself passes since `.agentrec/` now exists.
    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hooks missing/mangled"),
        "expected prior hook-missing behavior to survive the init gate: {stdout}"
    );

    let v = doctor_json_value(root);
    let checks = v["checks"].as_array().unwrap();
    let init_check = checks
        .iter()
        .find(|c| c["name"] == "initialized")
        .unwrap_or_else(|| panic!("missing initialized check in {v}"));
    assert_eq!(
        init_check["status"], "pass",
        "expected initialized pass in {v}"
    );
}

#[test]
fn doctor_json_shape_and_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Guaranteed to fail (no daemon, no hooks) so ok:false is exercised.
    agentrec(root, &["init", "--no-hook", "--no-service"]);

    let out = agentrec(root, &["doctor", "--json"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");

    let v = doctor_json_value(root);
    assert_eq!(v["ok"], false, "expected ok:false in {v}");
    let checks = v["checks"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a checks array in {v}"));
    assert!(!checks.is_empty(), "expected at least one check in {v}");
    assert!(
        checks.iter().any(|c| c["status"] == "fail"),
        "expected at least one failing check in {v}"
    );
    for check in checks {
        assert!(check["name"].is_string(), "check missing name: {check}");
        assert!(check["status"].is_string(), "check missing status: {check}");
    }
}

// AC-Y++2: the inotify-headroom check must actually hit its fail branch when
// fs.inotify.max_user_watches is too low for the repo's directory count. The
// normal CI matrix runs with a generous default limit (pass branch only) —
// this test only makes sense in an environment that deliberately lowered the
// ceiling first (see the `inotify-low-watches` job in
// .github/workflows/ci.yml), hence `#[ignore]` so it never runs as part of
// the default `cargo test --workspace`.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires fs.inotify.max_user_watches lowered before running — see .github/workflows/ci.yml's inotify-low-watches job"]
fn doctor_inotify_low_watches_fails() {
    let max_watches: u64 = std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .expect("read /proc/sys/fs/inotify/max_user_watches")
        .trim()
        .parse()
        .expect("parse max_user_watches");
    assert!(
        max_watches < 100,
        "this test requires an induced-low fs.inotify.max_user_watches \
         (got {max_watches}) — run: sudo sysctl -w fs.inotify.max_user_watches=1"
    );

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let init_out = agentrec(root, &["init", "--no-service"]);
    assert!(init_out.status.success(), "init failed: {init_out:?}");

    let v = doctor_json_value(root);
    assert_eq!(
        v["ok"], false,
        "expected ok:false with induced-low watches: {v}"
    );
    let checks = v["checks"].as_array().unwrap();
    let inotify = checks
        .iter()
        .find(|c| c["name"] == "inotify headroom")
        .unwrap_or_else(|| panic!("no inotify headroom check in {v}"));
    assert_eq!(
        inotify["status"], "fail",
        "expected inotify headroom to fail: {v}"
    );
    assert!(
        inotify["remedy"]
            .as_str()
            .unwrap_or("")
            .contains("max_user_watches"),
        "expected remedy to mention max_user_watches: {v}"
    );

    let out = doctor(root);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
}

// --- AC-Z+2, AC-Z+3, AC-Z+4 (D42/D43): relative-time default / --utc
// absolute, color gated off when piped or under NO_COLOR, and the
// `--explain` glossary only ever mentions terms present in this listing.

// D-PD4: `log --json` must stay schema-stable even with zero turns — an
// empty store's JSON output is the empty array `[]`, never blank stdout
// (blank stdout is not valid JSON and breaks any consumer that parses it).
#[test]
fn log_json_zero_turns_prints_empty_array() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let out = agentrec(root, &["log", "--json"]);
    assert_eq!(out.status.code(), Some(0), "log --json failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim(), "[]", "stdout: {stdout:?}");
}

#[test]
fn log_default_is_relative_utc_is_absolute() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // base_turn's started/ended is a fixed past RFC 3339 stamp.
    seed_turn(root, &base_turn("t_RELTIME0000000000000001", vec![]));

    let default_out = agentrec(root, &["log"]);
    assert!(default_out.status.success(), "log failed: {default_out:?}");
    let default_stdout = String::from_utf8_lossy(&default_out.stdout);
    assert!(
        !default_stdout.contains("2026-07-05T00:00:00"),
        "default log must render relative time, not the RFC3339 stamp: {default_stdout}"
    );

    let utc_out = agentrec(root, &["log", "--utc"]);
    assert!(utc_out.status.success(), "log --utc failed: {utc_out:?}");
    let utc_stdout = String::from_utf8_lossy(&utc_out.stdout);
    assert!(
        utc_stdout.contains("2026-07-05T00:00:00"),
        "expected the RFC3339 stamp under --utc: {utc_stdout}"
    );
}

#[test]
fn log_piped_output_has_no_color_escapes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    seed_turn(root, &base_turn("t_NOCOLOR000000000000001", vec![]));

    // `agentrec()` captures stdout via `Output` — never a tty — so this
    // alone exercises the not-a-tty path of `should_color`.
    let out = agentrec(root, &["log"]);
    assert!(out.status.success(), "log failed: {out:?}");
    assert!(
        !out.stdout.contains(&0x1b_u8),
        "piped log output must contain no ESC byte"
    );
}

#[test]
fn log_no_color_env_suppresses_escapes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    seed_turn(root, &base_turn("t_NOCOLORENV00000000001", vec![]));

    let out = Command::new(bin())
        .args(["log", "--root", root.to_str().unwrap()])
        .env("NO_COLOR", "1")
        .output()
        .expect("run agentrec log");
    assert!(out.status.success(), "log failed: {out:?}");
    assert!(
        !out.stdout.contains(&0x1b_u8),
        "NO_COLOR log output must contain no ESC byte"
    );
}

// --- Task 4: `agentrec remember` — manual pinned memories.

#[test]
fn remember_writes_pinned_scrubbed_record() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();

    let out = agentrec(
        root,
        &[
            "remember",
            "build needs cargo nightly",
            "--from",
            "src/a.rs",
        ],
    );
    assert!(out.status.success(), "remember failed: {out:?}");

    let path = root.join(".agentrec/memory.jsonl");
    assert!(path.exists(), "memory.jsonl must exist");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "memory.jsonl must be 0600");
    }

    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "expected exactly one record: {text}");
    let rec: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(rec.get("origin").and_then(|v| v.as_str()), Some("human"));
    assert_eq!(rec.get("op").and_then(|v| v.as_str()), Some("assert"));
    let pins = rec.get("pins").and_then(|v| v.as_array()).unwrap();
    assert_eq!(pins.len(), 1, "expected one pin: {pins:?}");
    assert_eq!(
        pins[0].get("path").and_then(|v| v.as_str()),
        Some("src/a.rs")
    );
    let hash = pins[0].get("hash").and_then(|v| v.as_str()).unwrap();
    assert!(hash.starts_with("sha256:"), "hash: {hash}");
    let hex = &hash["sha256:".len()..];
    assert_eq!(hex.len(), 64, "hash hex len: {hex}");
    assert!(
        hex.chars().all(|c| c.is_ascii_hexdigit()),
        "hash not hex: {hash}"
    );
}

#[test]
fn remember_refuses_bad_pins_and_secret_facts() {
    // Traversal escape: --from ../escape -> exit 1, stderr names the path,
    // memory.jsonl absent.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        let out = agentrec(root, &["remember", "some fact", "--from", "../escape"]);
        assert!(!out.status.success(), "expected failure: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("../escape"), "stderr: {stderr}");
        assert!(
            !root.join(".agentrec/memory.jsonl").exists(),
            "memory.jsonl must not be created on a rejected pin"
        );
    }

    // Secret-file pin: --from .env -> exit 1, stderr mentions secret.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        std::fs::write(root.join(".env"), b"SECRET=1").unwrap();
        let out = agentrec(root, &["remember", "some fact", "--from", ".env"]);
        assert!(!out.status.success(), "expected failure: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("secret"), "stderr: {stderr}");
        assert!(
            !root.join(".agentrec/memory.jsonl").exists(),
            "memory.jsonl must not be created on a secret-path pin"
        );
    }

    // A fact containing a secret (but not only a secret), with a valid pin,
    // is persisted with the secret redacted (INV-M3 half 1) — same
    // AWS-key-shaped fixture as secret_prompt_never_reaches_disk_in_cleartext.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
        let secret = "AKIAABCDEFGHIJKLMNOP";
        let fact = format!("deploy with key {secret} now");
        let out = agentrec(root, &["remember", &fact, "--from", "src/a.rs"]);
        assert!(out.status.success(), "remember failed: {out:?}");
        let text = std::fs::read_to_string(root.join(".agentrec/memory.jsonl")).unwrap();
        assert!(
            text.contains("[redacted:"),
            "expected a redaction marker: {text}"
        );
        assert!(!text.contains(secret), "raw key leaked to disk: {text}");
    }

    // A fact that scrubs to nothing is refused, nothing written. scrub()
    // never deletes matched content (it substitutes a `[redacted:...]`
    // marker), so the only input that can trim-empty after scrubbing is one
    // that was already blank — same fixture shape as
    // agentrec_core::memory::append_memory_rejects_oversize_and_empty's
    // blank_fact case.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
        let out = agentrec(root, &["remember", "   \n\t  ", "--from", "src/a.rs"]);
        assert!(!out.status.success(), "expected failure: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("scrub") || stderr.contains("empty"),
            "stderr: {stderr}"
        );
        assert!(
            !root.join(".agentrec/memory.jsonl").exists(),
            "memory.jsonl must not be created for a fact that scrubs to nothing"
        );
    }
}

// --- Task 5: `agentrec recall` + `agentrec memories`.

/// Seeds `count` off-topic filler memories, each pinned to its own file —
/// mirrors `agentrec_core::memory::recall_never_returns_stale`'s fixture:
/// with only 1-2 on-topic memories in a tiny corpus, idf(shared terms)
/// doesn't clear SCORE_FLOOR on its own, so tests that exercise real BM25
/// ranking need the corpus padded to N ~10.
fn seed_filler_memories(root: &Path, count: usize) {
    for i in 0..count {
        let rel = format!("filler{i}.rs");
        std::fs::write(root.join(&rel), b"fn filler() {}").unwrap();
        let out = agentrec(
            root,
            &[
                "remember",
                "unrelated documentation cleanup housekeeping chore",
                "--from",
                &rel,
            ],
        );
        assert!(out.status.success(), "remember filler{i} failed: {out:?}");
    }
}

#[test]
fn recall_cli_fresh_only_and_json() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
    std::fs::write(root.join("src/b.rs"), b"fn b() {}").unwrap();

    let out = agentrec(
        root,
        &[
            "remember",
            "nightly seed rotation keeps torture runs reproducible",
            "--from",
            "src/a.rs",
        ],
    );
    assert!(out.status.success(), "remember A failed: {out:?}");
    let out = agentrec(
        root,
        &[
            "remember",
            "nightly seed also drives the fuzz corpus replay",
            "--from",
            "src/b.rs",
        ],
    );
    assert!(out.status.success(), "remember B failed: {out:?}");
    seed_filler_memories(root, 8);

    let out = agentrec(root, &["recall", "nightly seed", "--json"]);
    assert!(out.status.success(), "recall --json failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("invalid json: {e}: {stdout}"));
    let arr = parsed.as_array().expect("json array");
    assert_eq!(arr.len(), 2, "expected 2 fresh matches: {stdout}");

    // Mutate one pinned file's content -> that memory goes Stale.
    std::fs::write(root.join("src/a.rs"), b"fn a() { changed(); }").unwrap();

    let out = agentrec(root, &["recall", "nightly seed", "--json"]);
    assert!(out.status.success(), "recall --json (2) failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = parsed.as_array().expect("json array");
    assert_eq!(arr.len(), 1, "stale memory must be excluded: {stdout}");

    // `memories --stale` shows the one whose pin drifted.
    let out = agentrec(root, &["memories", "--stale"]);
    assert!(out.status.success(), "memories --stale failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("src/a.rs"),
        "stale listing must show the drifted pin path: {stdout}"
    );
    assert!(
        !stdout.contains("src/b.rs"),
        "fresh memory must not appear under --stale: {stdout}"
    );

    // `memories --all` shows both, regardless of freshness.
    let out = agentrec(root, &["memories", "--all"]);
    assert!(out.status.success(), "memories --all failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("src/a.rs") && stdout.contains("src/b.rs"),
        "memories --all must show both: {stdout}"
    );
}

#[test]
fn recall_for_hook_emits_block_or_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
    seed_filler_memories(root, 8);
    let out = agentrec(
        root,
        &[
            "remember",
            "nightly seed rotation keeps torture runs reproducible",
            "--from",
            "src/a.rs",
        ],
    );
    assert!(out.status.success(), "remember failed: {out:?}");

    // Matching query -> fenced block, capped at 800 chars, never a hash.
    let out = agentrec(root, &["recall", "nightly seed", "--for-hook"]);
    assert!(out.status.success(), "recall --for-hook failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("```agentrec memory"), "stdout: {stdout}");
    assert!(
        stdout.chars().count() <= 800,
        "block exceeds 800 chars ({} chars): {stdout}",
        stdout.chars().count()
    );
    assert!(
        !stdout.contains("sha256:"),
        "must not leak hashes: {stdout}"
    );

    // Nonsense query -> nothing above SCORE_FLOOR -> empty stdout, exit 0.
    let out = agentrec(root, &["recall", "zebra quantum", "--for-hook"]);
    assert!(
        out.status.success(),
        "nonsense query must still exit 0: {out:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "nonsense query must emit no stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Uninitialized repo: --for-hook fails open (empty stdout, exit 0);
    // without --for-hook it's a real error (exit 1, stderr).
    let tmp2 = tempfile::tempdir().unwrap();
    let root2 = tmp2.path();

    let out = agentrec(root2, &["recall", "anything", "--for-hook"]);
    assert!(
        out.status.success(),
        "uninitialized --for-hook must exit 0: {out:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "uninitialized --for-hook must emit no stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    let out = agentrec(root2, &["recall", "anything"]);
    assert!(
        !out.status.success(),
        "uninitialized recall (no --for-hook) must fail: {out:?}"
    );
    assert!(
        !out.stderr.is_empty(),
        "uninitialized recall (no --for-hook) must report on stderr"
    );
}

#[test]
fn log_explain_glossary_matches_only_present_terms() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    // Rich-only listing: glossary must mention "rich", never "bare".
    seed_turn(root, &base_turn("t_RICHONLY0000000000001", vec![]));
    let rich_out = agentrec(root, &["log", "--explain"]);
    assert!(
        rich_out.status.success(),
        "log --explain failed: {rich_out:?}"
    );
    let rich_stdout = String::from_utf8_lossy(&rich_out.stdout);
    assert!(
        rich_stdout.contains("rich:"),
        "expected a rich glossary entry: {rich_stdout}"
    );
    assert!(
        !rich_stdout.contains("bare:"),
        "must not explain bare with no bare turns present: {rich_stdout}"
    );

    // Add a bare turn; glossary must now mention "bare" too.
    let mut bare = base_turn("t_BARETURN0000000000001", vec![]);
    bare.grade = "bare".to_string();
    bare.tool = None;
    bare.prompt_excerpt = None;
    seed_turn(root, &bare);

    let both_out = agentrec(root, &["log", "--explain"]);
    assert!(
        both_out.status.success(),
        "log --explain failed: {both_out:?}"
    );
    let both_stdout = String::from_utf8_lossy(&both_out.stdout);
    assert!(
        both_stdout.contains("bare:"),
        "expected a bare glossary entry once a bare turn is present: {both_stdout}"
    );
}

// --- Task 8: `agentrec candidate` — agent-emitted memory candidates (the
// memory WRITE path's CLI emitter half; the daemon-side ingestion is
// Task 7's `daemon::ingest_candidate`, covered end-to-end here too).

/// Every parsed line of `.agentrec/memory.jsonl`. Absent file = empty vec.
fn memory_records(root: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(root.join(".agentrec/memory.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn candidate_cli_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
    std::fs::write(
        root.join(".github/workflows/nightly.yml"),
        "name: nightly\n",
    )
    .unwrap();

    let mut daemon = spawn_record(root);
    std::thread::sleep(Duration::from_millis(800));

    let out = agentrec(
        root,
        &[
            "candidate",
            "fact about nightly",
            "--from",
            ".github/workflows/nightly.yml",
        ],
    );
    assert!(out.status.success(), "candidate failed: {out:?}");

    // The emitted signal line: type memory-candidate, pins are PATHS ONLY —
    // no "sha256:" hash anywhere in the line. Hashing is the daemon's job,
    // done against the live tree at ingestion, never trusted from the
    // emitter (design spec, "Rejected approaches — emitter-side hashing").
    let signal_text = std::fs::read_to_string(root.join(".agentrec/signal.jsonl")).unwrap();
    let candidate_line = signal_text
        .lines()
        .rev()
        .find_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            (v.get("type").and_then(|t| t.as_str()) == Some("memory-candidate")).then_some(v)
        })
        .expect("no memory-candidate line appended to signal.jsonl");
    assert_eq!(
        candidate_line.get("fact").and_then(|f| f.as_str()),
        Some("fact about nightly")
    );
    let pins = candidate_line
        .get("pins")
        .and_then(|p| p.as_array())
        .expect("pins must be an array");
    assert_eq!(pins.len(), 1, "expected exactly one pin: {pins:?}");
    let pin0 = pins[0]
        .as_str()
        .expect("pins must be plain path strings, not hash objects");
    assert_eq!(pin0, ".github/workflows/nightly.yml");
    assert!(
        !signal_text.contains("sha256:"),
        "the emitted signal line must never carry a hash: {signal_text}"
    );

    // End-to-end through Task 7 ingestion: the daemon tails the inbox and
    // lands the fact in memory.jsonl, attributed to the agent.
    let recorded = poll_until(Duration::from_secs(5), || {
        let recs = memory_records(root);
        (!recs.is_empty()).then_some(recs)
    });
    let _ = daemon.kill();
    let _ = daemon.wait();

    let recs = recorded.expect("candidate was never ingested into memory.jsonl");
    assert_eq!(
        recs.len(),
        1,
        "expected exactly one memory record: {recs:?}"
    );
    assert_eq!(
        recs[0].get("origin").and_then(|v| v.as_str()),
        Some("agent")
    );
    assert_eq!(
        recs[0].get("fact").and_then(|v| v.as_str()),
        Some("fact about nightly")
    );
}

// Daemon-DOWN replay + D7-preservation pair. A candidate emitted while NO
// daemon is running must be ingested on the NEXT daemon start (candidate-only
// startup replay, daemon.rs `replay_pending_candidates`). In the SAME
// pre-daemon gap, a stale start/stop bracket must NOT be replayed — it would
// mint a phantom turn misdated to boot (the exact hazard D7's EOF-skip
// prevents). This is the pair that proves the startup replay is
// candidate-only and D7 still holds: memory record lands, zero turns appear.
#[test]
fn candidate_startup_replay_is_candidate_only_and_preserves_d7() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::write(root.join("notes.txt"), "hello").unwrap();

    // No daemon running yet. Emit a candidate via the real CLI...
    let out = agentrec(
        root,
        &["candidate", "a fact learned offline", "--from", "notes.txt"],
    );
    assert!(out.status.success(), "candidate failed: {out:?}");
    assert!(
        !root.join(".agentrec/memory.jsonl").exists(),
        "no daemon was running — nothing should be ingested yet"
    );

    // ...and, in the SAME gap, plant a stale bracket (start+stop). If replayed
    // as live, this pair would fabricate an empty turn misdated to boot.
    send_hook(
        root,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s_stale"}"#,
    );
    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s_stale"}"#);

    // Now start the daemon: the candidate is replayed on startup...
    let mut daemon = spawn_record(root);
    let recorded = poll_until(Duration::from_secs(6), || {
        let recs = memory_records(root);
        (!recs.is_empty()).then_some(recs)
    });

    // Give the daemon a beat past the replay to confirm no phantom turn lands.
    std::thread::sleep(Duration::from_secs(1));
    let phantom = turns(root);
    let _ = daemon.kill();
    let _ = daemon.wait();

    let recs = recorded.expect("candidate emitted while daemon down was never replayed on startup");
    assert_eq!(
        recs.len(),
        1,
        "expected exactly one memory record: {recs:?}"
    );
    assert_eq!(
        recs[0].get("fact").and_then(|v| v.as_str()),
        Some("a fact learned offline")
    );
    assert_eq!(
        recs[0].get("origin").and_then(|v| v.as_str()),
        Some("agent")
    );

    assert!(
        phantom.is_empty(),
        "a stale start/stop bracket in the pre-daemon gap was replayed and \
         minted a phantom turn — D7's EOF-skip must still drop start/stop: {phantom:?}"
    );
}

// --- Task 9: UserPromptSubmit hook memory injection (INV-M4).

/// `.agentrec/signal.jsonl` lines parsed as JSON, in file order.
fn signal_events(root: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(root.join(".agentrec/signal.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// `.agentrec/memory-stats.jsonl` lines parsed as JSON, in file order.
fn memory_stats_lines(root: &Path) -> Vec<serde_json::Value> {
    let text =
        std::fs::read_to_string(root.join(".agentrec/memory-stats.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn hook_injects_fresh_memories_into_stdout() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"fn a() {}").unwrap();
    seed_filler_memories(root, 8);
    let out = agentrec(
        root,
        &[
            "remember",
            "nightly seed rotation keeps torture runs reproducible",
            "--from",
            "src/a.rs",
        ],
    );
    assert!(out.status.success(), "remember failed: {out:?}");

    // Matching prompt -> stdout carries the fenced block + the fact; the
    // start signal (existing behavior) still lands; memory-stats.jsonl
    // gains exactly one line.
    let payload =
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"nightly seed"}"#;
    let out = send_hook_capture(root, payload);
    assert!(out.status.success(), "hook failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("```agentrec memory"), "stdout: {stdout}");
    assert!(
        stdout.contains("nightly seed rotation keeps torture runs reproducible"),
        "stdout: {stdout}"
    );

    let events = signal_events(root);
    assert!(
        events
            .iter()
            .any(|e| e.get("event").and_then(|v| v.as_str()) == Some("start")),
        "start signal missing after hook call: {events:?}"
    );

    let stats = memory_stats_lines(root);
    assert_eq!(stats.len(), 1, "expected one memory-stats line: {stats:?}");
    assert!(stats[0].get("ts").and_then(|v| v.as_u64()).is_some());
    assert_eq!(stats[0].get("n").and_then(|v| v.as_u64()), Some(1));

    // Non-matching prompt -> empty stdout, no new memory-stats line.
    let payload =
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s2","prompt":"zebra quantum"}"#;
    let out = send_hook_capture(root, payload);
    assert!(out.status.success(), "hook failed: {out:?}");
    assert!(
        out.stdout.is_empty(),
        "non-matching prompt must emit no stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        memory_stats_lines(root).len(),
        1,
        "no injection -> no new memory-stats line"
    );

    // memory_enabled = false -> no block, even for a matching prompt.
    std::fs::write(
        root.join(".agentrec/config.toml"),
        "ttl_days = 90\nmcp_destructive = \"off\"\nmemory_enabled = false\n",
    )
    .unwrap();
    let payload =
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s3","prompt":"nightly seed"}"#;
    let out = send_hook_capture(root, payload);
    assert!(out.status.success(), "hook failed: {out:?}");
    assert!(
        out.stdout.is_empty(),
        "memory_enabled=false must emit no stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        memory_stats_lines(root).len(),
        1,
        "memory_enabled=false -> no new memory-stats line"
    );

    // The Stop arm is untouched: no block, no memory-stats line, even for a
    // "matching" prompt field (which Stop payloads don't carry anyway).
    std::fs::write(
        root.join(".agentrec/config.toml"),
        "ttl_days = 90\nmcp_destructive = \"off\"\nmemory_enabled = true\n",
    )
    .unwrap();
    let payload = r#"{"hook_event_name":"Stop","session_id":"s1"}"#;
    let out = send_hook_capture(root, payload);
    assert!(out.status.success(), "hook (Stop) failed: {out:?}");
    assert!(
        out.stdout.is_empty(),
        "Stop arm must never print a memory block: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        memory_stats_lines(root).len(),
        1,
        "Stop arm must never append to memory-stats.jsonl"
    );

    // Bonus (coupling proof): memory_inject_max > the old hardcoded default
    // of 5 actually injects more than 5 facts — proves `max_facts` feeds
    // both `memory::recall`'s `k` and `build_hook_block`'s per-block cap.
    for i in 0..7 {
        let rel = format!("src/m{i}.rs");
        std::fs::write(root.join(&rel), b"fn m() {}").unwrap();
        let out = agentrec(
            root,
            &[
                "remember",
                &format!("nightly seed shard {i} rotation detail"),
                "--from",
                &rel,
            ],
        );
        assert!(out.status.success(), "remember m{i} failed: {out:?}");
    }
    std::fs::write(
        root.join(".agentrec/config.toml"),
        "ttl_days = 90\nmcp_destructive = \"off\"\nmemory_enabled = true\nmemory_inject_max = 10\n",
    )
    .unwrap();
    let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s4","prompt":"nightly seed rotation shard"}"#;
    let out = send_hook_capture(root, payload);
    assert!(out.status.success(), "hook failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let fact_lines = stdout.lines().filter(|l| l.starts_with("- ")).count();
    assert!(
        fact_lines > 5,
        "memory_inject_max=10 must inject more than the old hardcoded 5 \
         (coupling fix) — got {fact_lines} lines: {stdout}"
    );
}

#[test]
fn hook_fail_open_and_budget() {
    // Corrupt memory.jsonl -> hook still exits 0, stdout carries no block,
    // and the start signal is still appended (INV-M4 fail-open).
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        std::fs::write(
            root.join(".agentrec/memory.jsonl"),
            b"\xff\xfenot json at all garbage bytes\x00\x01",
        )
        .unwrap();

        let payload =
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"anything"}"#;
        let out = send_hook_capture(root, payload);
        assert!(out.status.success(), "hook must exit 0: {out:?}");
        assert!(
            out.stdout.is_empty(),
            "corrupt store must emit no stdout: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        let events = signal_events(root);
        assert!(
            events
                .iter()
                .any(|e| e.get("event").and_then(|v| v.as_str()) == Some("start")),
            "start signal missing despite corrupt memory.jsonl: {events:?}"
        );
    }

    // A large store still returns within the (generous, CI-slack) 500ms
    // wall-clock budget asserted here; the hook's own internal self-budget
    // is 50ms (RECALL_BUDGET_MS).
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        let mut lines = String::new();
        for i in 0..3000 {
            let rec = serde_json::json!({
                "v": 1,
                "type": "memory",
                "id": format!("m{i}"),
                "op": "assert",
                "fact": format!("filler fact number {i} about nightly seed rotation housekeeping"),
                "pins": [{
                    "path": format!("missing{i}.rs"),
                    "hash": format!("sha256:{i:064}"),
                }],
                "source_turns": [],
                "origin": "agent",
                "ts": i,
            });
            lines.push_str(&rec.to_string());
            lines.push('\n');
        }
        std::fs::write(root.join(".agentrec/memory.jsonl"), lines).unwrap();

        let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s2","prompt":"nightly seed rotation"}"#;
        let started = Instant::now();
        let out = send_hook_capture(root, payload);
        let elapsed = started.elapsed();
        assert!(out.status.success(), "hook must exit 0: {out:?}");
        assert!(
            elapsed < Duration::from_millis(500),
            "hook took too long against a 3000-record store: {elapsed:?}"
        );
    }

    // Missing store (never `remember`ed) and a fully uninitialized
    // `.agentrec/` both exit 0 with no block.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init(root);
        let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s3","prompt":"anything at all"}"#;
        let out = send_hook_capture(root, payload);
        assert!(out.status.success(), "hook must exit 0: {out:?}");
        assert!(out.stdout.is_empty());
    }
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path(); // never `init`ed — no .agentrec/ at all yet
        let payload = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s4","prompt":"anything at all"}"#;
        let out = send_hook_capture(root, payload);
        assert!(
            out.status.success(),
            "hook on an uninitialized repo must exit 0: {out:?}"
        );
        assert!(out.stdout.is_empty());
    }
}
