//! Red-team finding T2: the intra-turn misattribution chain.
//!
//! D6 is "one open turn per root". While a Claude Code bracket is open
//! (`UserPromptSubmit` -> `Stop`), EVERY filesystem mutation in the root is
//! folded into that one rich turn — including a human's own edit to a file
//! the agent never touched. The turn's recorded `after` hash for that file is
//! therefore the *human's* content, so at undo time
//! `is_modified = current != after` is **false**, the D30 modified-since rail
//! (`is_modified && !allow_modified`) never fires, and the human's work is
//! reverted with no refusal and no exclusion.
//!
//! `human_burst_outside_bracket_closes_bare` (agentrec-core/src/engine.rs)
//! covers the *outside*-the-bracket case only. This file covers the inside
//! case end-to-end against the real binary + real daemon, and pins the
//! honesty line undo now prints instead of pretending it can tell the two
//! apart. No source-detection heuristic exists or should: a bare statement
//! that is always true beats a guess that is sometimes a lie.
//!
//! Helpers are deliberately duplicated from `cli/tests/hardening_daemon.rs`
//! rather than shared — same file-ownership convention as the rest of
//! `cli/tests/`.

use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// The exact substring `undo` must print for any window-derived rich turn.
/// Load-bearing: the whole guard is this sentence being unmissable and
/// unconditionally true.
const CAUTION_SUBSTR: &str =
    "cannot distinguish the recorded tool's own writes from concurrent human edits made in the \
     same window";

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

/// `event` values of every epoch line, in order.
fn epoch_events(root: &Path) -> Vec<String> {
    let path = root.join(".agentrec/log.jsonl");
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("epoch"))
        .filter_map(|v| v.get("event").and_then(|e| e.as_str()).map(str::to_string))
        .collect()
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
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Paths recorded on a turn record.
fn turn_paths(t: &serde_json::Value) -> Vec<String> {
    t.get("files")
        .and_then(|f| f.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|f| f.get("path").and_then(|p| p.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The live (non-superseded) rich turns. `log.jsonl` is append-only and a
/// retroactive fold supersedes rather than removes the bare fragments it
/// absorbed, so a raw `turns().len()` is timing-dependent — the fold's own
/// `merges` list is the authority on what is still live.
fn live_rich_turns(root: &Path) -> Vec<serde_json::Value> {
    let all = turns(root);
    let superseded: HashSet<String> = all
        .iter()
        .filter_map(|t| t.get("merges").and_then(|m| m.as_array()))
        .flatten()
        .filter_map(|m| m.as_str().map(str::to_string))
        .collect();
    all.into_iter()
        .filter(|t| t.get("grade").and_then(|g| g.as_str()) == Some("rich"))
        .filter(|t| {
            t.get("id")
                .and_then(|i| i.as_str())
                .is_some_and(|i| !superseded.contains(i))
        })
        .collect()
}

// ---- AC2.1 + AC2.2 ---------------------------------------------------------

/// The full chain, end to end, against the real daemon and the real binary:
///
///   `UserPromptSubmit` -> agent writes `agent_feature.rs`
///                      -> "human" writes `human_notes.md` (different file)
///                      -> `Stop`
///
/// Both files land in ONE rich turn attributed to `claude` (D6). Undoing that
/// turn deletes the human's file — no `EXCLUDE`, no `REFUSE`, because the
/// recorded `after` hash for `human_notes.md` IS the human's content, so the
/// D30 modified-since rail cannot fire. That data loss is pinned here as
/// current, intended-by-D6 behavior; what the guard adds is that undo now
/// says so out loud (`CAUTION_SUBSTR`) in both preview and confirm.
#[test]
fn intra_bracket_human_edit_folds_into_agent_turn_and_undo_reverts_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = spawn_record(root);
    let started = poll_until(Duration::from_secs(10), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(started.is_some(), "daemon never appended a start epoch");

    // Open the agent's bracket.
    send_hook(
        root,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s_t2","prompt":"add the feature"}"#,
    );
    std::thread::sleep(Duration::from_millis(500));

    // The agent's write.
    std::fs::write(root.join("agent_feature.rs"), "fn feature() {}\n").unwrap();
    std::thread::sleep(Duration::from_millis(800));

    // The human's write, to a DIFFERENT file, while the bracket is still
    // open. At the filesystem level this is indistinguishable from the line
    // above — that is the whole finding.
    std::fs::write(
        root.join("human_notes.md"),
        "my own notes, not the agent's\n",
    )
    .unwrap();

    // Past the 1.5s mutation debounce, well short of the 10s quiet window
    // (which the open bracket suppresses anyway).
    std::thread::sleep(Duration::from_secs(3));

    // Close the bracket.
    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s_t2"}"#);

    let folded = poll_until(Duration::from_secs(15), || {
        let live = live_rich_turns(root);
        live.into_iter().find(|t| {
            let p = turn_paths(t);
            p.iter().any(|x| x == "agent_feature.rs") && p.iter().any(|x| x == "human_notes.md")
        })
    });

    // Stop watching BEFORE undo runs: undo's own writes would otherwise be
    // observed by the daemon and mint a turn mid-assertion.
    let _ = daemon.kill();
    let _ = daemon.wait();

    let turn = folded.expect(
        "the agent write and the human write did not land in one rich turn — \
         D6's one-open-turn-per-root fold did not happen",
    );
    assert_eq!(
        turn.get("tool").and_then(|t| t.as_str()),
        Some("claude"),
        "the folded turn is attributed to the agent, human file and all: {turn:?}"
    );
    let turn_id = turn
        .get("id")
        .and_then(|i| i.as_str())
        .expect("turn id")
        .to_string();

    // --- preview: the caution must be visible BEFORE anything is mutated ---
    let out = agentrec(root, &["undo", &turn_id]);
    assert!(out.status.success(), "undo preview failed: {out:?}");
    let preview = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        preview.contains(CAUTION_SUBSTR),
        "preview must carry the intra-window caution: {preview}"
    );
    assert!(
        preview.contains("preview only"),
        "preview must still require --confirm: {preview}"
    );
    assert!(
        root.join("human_notes.md").exists(),
        "preview must not mutate the worktree"
    );

    // --- confirm: the human's file is reverted, with no refusal -----------
    let out = agentrec(root, &["undo", &turn_id, "--confirm"]);
    assert!(out.status.success(), "undo --confirm failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains(CAUTION_SUBSTR),
        "confirm path must carry the intra-window caution too: {stdout}"
    );
    assert!(
        stdout.contains("revert  human_notes.md"),
        "the human's file must be planned for revert: {stdout}"
    );
    assert!(
        !stdout.contains("EXCLUDE human_notes.md") && !stdout.contains("REFUSE  human_notes.md"),
        "the D30 modified-since rail cannot fire here — the turn's `after` IS \
         the human's content, so there is no refusal to be had: {stdout}"
    );
    assert!(
        !root.join("human_notes.md").exists(),
        "PINNED: the human's file is silently reverted (deleted) by an undo of \
         the agent's turn — this is D6's documented limitation, and the reason \
         the caution line exists"
    );
    assert!(
        !root.join("agent_feature.rs").exists(),
        "the agent's own file is reverted too (sanity: the undo really ran)"
    );
}

// ---- the caution is gated, not blanket ------------------------------------

/// Negative direction, so the assertion above is not vacuous: agentrec's own
/// undo turns are the one rich shape whose file list is NOT a watch window —
/// it is built from the revert plan, i.e. exactly what the process itself
/// wrote. Those must NOT carry the caution, or the line becomes noise printed
/// on every command and stops being read.
#[test]
fn agentrec_own_turn_carries_no_window_caution() {
    use agentrec_core::record::{FileEntry, LogRecord, TurnRecord};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    std::fs::write(root.join("restored.txt"), "content\n").unwrap();
    let store = agentrec_core::store::BlobStore::new(root.join(".agentrec/objects"));
    let after = store.put(b"content\n").expect("snapshot fixture content");

    let turn = TurnRecord {
        v: 1,
        id: "t_AGENTRECOWNTURN00000000001".to_string(),
        grade: "rich".into(),
        truncated: false,
        started: "2026-07-30T00:00:00.000Z".into(),
        ended: "2026-07-30T00:00:01.000Z".into(),
        tool: Some("agentrec".into()),
        model: None,
        session: None,
        root: root.to_string_lossy().to_string(),
        prompt_ref: None,
        prompt_excerpt: Some("undo of t_EARLIER".into()),
        merges: vec![],
        files: vec![FileEntry {
            path: "restored.txt".into(),
            op: "create".into(),
            before: None,
            after: Some(after),
            skipped: false,
            skipped_reason: None,
            withheld: false,
            baseline_unknown: false,
        }],
    };
    agentrec_core::record::append_log(
        &root.join(".agentrec/log.jsonl"),
        &LogRecord::Turn(turn.clone()),
    )
    .expect("seed turn");

    let out = agentrec(root, &["undo", &turn.id]);
    assert!(out.status.success(), "undo preview failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("revert  restored.txt"),
        "the plan must still be printed: {stdout}"
    );
    assert!(
        !stdout.contains(CAUTION_SUBSTR),
        "agentrec's own turn is not a watch window — no caution: {stdout}"
    );
}
