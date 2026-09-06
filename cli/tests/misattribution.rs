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

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures, which this harness created;
//    there is no attacker-supplied FIFO to block on, so the fsguard wrappers
//    buy nothing here. Production reads stay lint-enforced (clippy.toml).

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

/// The caution's *scope* clause. Deliberately a SECOND constant rather than an
/// extension of `CAUTION_SUBSTR`: the negatives below assert
/// `!contains(CAUTION_SUBSTR)`, and folding the two together would let a
/// regression that prints only the first clause slip past them.
const CAUTION_SCOPE_SUBSTR: &str =
    "every file marked `revert` above is reverted regardless of who wrote it";

/// The pre-fix wording, which claimed the revert covers every *listed* file.
/// False on any plan carrying an `EXCLUDE` or `REFUSE` line; pinned absent.
const CAUTION_OVERCLAIM_SUBSTR: &str = "every file listed above is reverted";

/// The BARE variant (D49, founder decision 2026-08-01). A separate sentence,
/// not a reworded `CAUTION_SUBSTR`: the rich text names "the recorded tool" and
/// D6, and a bare turn has no recorded tool — every bare producer opens on
/// `Source::Quiet`, which is constructed `tool: None` (agentrec-core
/// `engine.rs`), and crash recovery forces `None` for a bare grade
/// (`cli/src/daemon.rs`). Asserting the two constants against each other is how
/// "the variants must not bleed" stays checkable.
const BARE_CAUTION_SUBSTR: &str =
    "this is a bare turn — an unattributed activity window with no recorded tool; agentrec cannot \
     say who or what made these writes";

/// The bare caution's *scope* clause, a second constant for the same reason
/// `CAUTION_SCOPE_SUBSTR` is one: a regression printing only the first clause
/// must not pass. Deliberately worded "who or what" — the rich constant's
/// "who wrote it" must not match this string, or the negatives that separate
/// the two variants go vacuous.
const BARE_CAUTION_SCOPE_SUBSTR: &str =
    "every file marked `revert` above is reverted regardless of who or what wrote it";

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

fn spawn_record_with_stderr(root: &Path) -> Child {
    Command::new(bin())
        .args(["record", "--root", root.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
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
    // This plan is all-Revert — the one shape on which the pre-fix "every file
    // listed above" wording happened to be true. Pin the corrected clause here
    // too, so the reword is asserted on both the pure-revert and the mixed
    // shape (`mixed_plan_caution_scopes_itself_to_reverted_files_only`).
    assert!(
        preview.contains(CAUTION_SCOPE_SUBSTR),
        "the caution must scope its revert claim to files marked `revert`: {preview}"
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
        imported: None,
        files_complete: None,
        origin: None,
        files: vec![FileEntry {
            path: "restored.txt".into(),
            op: "create".into(),
            before: None,
            after: Some(after),
            skipped: false,
            skipped_reason: None,
            after_synthesized: None,
            link_kind: None,
            attribution: None,
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

// ---- the caution must be true for every plan shape it is printed on --------

/// A `modify` entry whose `before`/`after` are both real blobs, so the
/// integrity gate (`store.get`) above modified-since never fires and the entry
/// reaches the revert/exclude fork this file is about.
fn modify_entry(
    store: &agentrec_core::store::BlobStore,
    path: &str,
    before: &[u8],
    after: &[u8],
) -> agentrec_core::record::FileEntry {
    agentrec_core::record::FileEntry {
        path: path.into(),
        op: "modify".into(),
        before: Some(store.put(before).expect("snapshot before")),
        after: Some(store.put(after).expect("snapshot after")),
        skipped: false,
        skipped_reason: None,
        after_synthesized: None,
        link_kind: None,
        attribution: None,
        withheld: false,
        baseline_unknown: false,
    }
}

fn seed_turn(
    root: &Path,
    id: &str,
    grade: &str,
    tool: Option<&str>,
    files: Vec<agentrec_core::record::FileEntry>,
) {
    use agentrec_core::record::{LogRecord, TurnRecord};
    let turn = TurnRecord {
        v: 1,
        id: id.to_string(),
        grade: grade.into(),
        truncated: false,
        started: "2026-07-30T00:00:00.000Z".into(),
        ended: "2026-07-30T00:00:01.000Z".into(),
        tool: tool.map(str::to_string),
        model: None,
        session: None,
        root: root.to_string_lossy().to_string(),
        prompt_ref: None,
        prompt_excerpt: None,
        merges: vec![],
        imported: None,
        files_complete: None,
        origin: None,
        files,
    };
    agentrec_core::record::append_log(&root.join(".agentrec/log.jsonl"), &LogRecord::Turn(turn))
        .expect("seed turn");
}

/// D2: on a MIXED plan — one `revert`, one `EXCLUDE` — the caution must not
/// claim the revert covers everything printed. The excluded file IS "listed
/// above" and is NOT reverted, so the pre-fix sentence was false exactly where
/// a user is most likely to be reading it. The guard's own stated rationale is
/// "a warning that is always true beats a detector that is sometimes a lie";
/// this test is what makes "always true" checkable.
#[test]
fn mixed_plan_caution_scopes_itself_to_reverted_files_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = agentrec_core::store::BlobStore::new(root.join(".agentrec/objects"));

    // a.txt: on-disk content == the turn's `after` -> not modified -> revert.
    // b.txt: a post-turn edit to a THIRD value -> modified-since -> EXCLUDE.
    // (Same fixture shape as integration.rs's `undo_modified_since_*` tests.)
    let files = vec![
        modify_entry(&store, "a.txt", b"a-v1\n", b"a-v2\n"),
        modify_entry(&store, "b.txt", b"b-v1\n", b"b-v2\n"),
    ];
    seed_turn(
        root,
        "t_MIXEDPLANCAUTION0000000001",
        "rich",
        Some("claude"),
        files,
    );
    std::fs::write(root.join("a.txt"), b"a-v2\n").unwrap();
    std::fs::write(root.join("b.txt"), b"b-v3-human\n").unwrap();

    let out = agentrec(root, &["undo", "t_MIXEDPLANCAUTION0000000001"]);
    assert!(out.status.success(), "undo preview failed: {out:?}");
    let preview = String::from_utf8_lossy(&out.stdout).into_owned();

    // The plan really is mixed — otherwise the assertions below are vacuous.
    assert!(
        preview.contains("revert  a.txt"),
        "fixture must produce a revert: {preview}"
    );
    assert!(
        preview.contains("EXCLUDE b.txt"),
        "fixture must produce an exclusion: {preview}"
    );

    assert!(
        preview.contains(CAUTION_SUBSTR),
        "a rich non-agentrec turn still gets the caution: {preview}"
    );
    // D2 proper, asserted first so a regression reds on the overclaim itself.
    assert!(
        !preview.contains(CAUTION_OVERCLAIM_SUBSTR),
        "OVERCLAIM: b.txt is listed above and is NOT reverted, so the caution \
         must not say every listed file is reverted: {preview}"
    );
    assert!(
        preview.contains(CAUTION_SCOPE_SUBSTR),
        "the caution must scope its revert claim to the files actually marked \
         `revert`: {preview}"
    );

    assert_eq!(
        std::fs::read(root.join("b.txt")).unwrap(),
        b"b-v3-human\n",
        "preview must not mutate the worktree"
    );
}

/// D49 (founder decision, 2026-08-01, resolving the re-gate residual this test
/// previously pinned open): a bare turn's undo DOES carry a caution, but its
/// own — the rich sentence names "the recorded tool" and D6, and a bare turn
/// has no recorded tool at all, so reusing it would fabricate the very
/// attribution the grade exists to withhold.
///
/// The two variants must not bleed: this test asserts the bare text present AND
/// the rich text's tool/D6 clause absent. (The reverse direction — bare text
/// absent on rich turns — is closed by construction, `window_caution` returning
/// one branch or the other and never concatenating.)
///
/// Non-vacuity matters: the gate has two suppression paths, so the fixture
/// deliberately yields a plan with a real `revert` line. Without that, the test
/// would pass because nothing was revertible, pinning nothing about `grade`.
#[test]
fn bare_turn_undo_carries_unattributed_window_caution() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let store = agentrec_core::store::BlobStore::new(root.join(".agentrec/objects"));

    let files = vec![modify_entry(&store, "c.txt", b"c-v1\n", b"c-v2\n")];
    seed_turn(root, "t_BARETURNNOCAUTION000000001", "bare", None, files);
    std::fs::write(root.join("c.txt"), b"c-v2\n").unwrap();

    // --- preview ----------------------------------------------------------
    let out = agentrec(root, &["undo", "t_BARETURNNOCAUTION000000001"]);
    assert!(out.status.success(), "undo preview failed: {out:?}");
    let preview = String::from_utf8_lossy(&out.stdout).into_owned();

    assert!(
        preview.contains("revert  c.txt"),
        "the plan must contain a revert, or the caution gate is not even \
         reached and this test pins nothing: {preview}"
    );
    assert!(
        preview.contains(BARE_CAUTION_SUBSTR),
        "a bare turn's undo must disclose the unattributed window: {preview}"
    );
    assert!(
        preview.contains(BARE_CAUTION_SCOPE_SUBSTR),
        "the bare caution must scope its revert claim to files marked \
         `revert`: {preview}"
    );
    assert!(
        !preview.contains(CAUTION_SUBSTR),
        "VARIANT BLEED: the rich text names the recorded tool and D6, and a \
         bare turn has neither: {preview}"
    );

    // --- confirm: the same disclosure, on the path that actually writes ----
    let out = agentrec(root, &["undo", "t_BARETURNNOCAUTION000000001", "--confirm"]);
    assert!(out.status.success(), "undo --confirm failed: {out:?}");
    let confirmed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        confirmed.contains(BARE_CAUTION_SUBSTR),
        "confirm path must carry the bare caution too: {confirmed}"
    );
    assert!(
        confirmed.contains(BARE_CAUTION_SCOPE_SUBSTR),
        "confirm path must carry the scope clause too: {confirmed}"
    );
    assert!(
        !confirmed.contains(CAUTION_SUBSTR),
        "VARIANT BLEED on the confirm path: {confirmed}"
    );
    assert_eq!(
        std::fs::read(root.join("c.txt")).unwrap(),
        b"c-v1\n",
        "the revert the caution warns about really happened (non-vacuity: \
         the confirm path reached the writes)"
    );
}

// ---- D6 phase 2b: emitter-side files_written (PROTOCOL §4) ------------------

/// D6-ATTR: the daemon consumes, rather than merely parses, a Stop signal's
/// declaration. Only observed files are persisted: the declared intersection
/// is `declared`, other observed files are `undeclared`, and external
/// declarations are counted in the daemon's user-visible diagnostics.
#[test]
fn d6_attr_stop_declaration_persists_declared_and_undeclared_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root_buf = tmp.path().canonicalize().unwrap();
    let root = root_buf.as_path();
    init(root);

    let mut daemon = spawn_record_with_stderr(root);
    let started = poll_until(Duration::from_secs(10), || {
        epoch_events(root)
            .contains(&"start".to_string())
            .then_some(())
    });
    assert!(started.is_some(), "daemon never appended a start epoch");

    send_hook(
        root,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s_attr","prompt":"attribute writes"}"#,
    );
    let opened = poll_until(Duration::from_secs(5), || {
        root.join(".agentrec/open.json").exists().then_some(())
    });
    assert!(opened.is_some(), "daemon never opened the Codex bracket");
    std::thread::sleep(Duration::from_millis(500));

    let declared = root.join("declared.rs");
    let concurrent = root.join("concurrent.md");
    std::fs::write(&declared, "fn declared() {}\n").unwrap();
    std::thread::sleep(Duration::from_millis(800));
    std::fs::write(&concurrent, "human note\n").unwrap();
    let staged = poll_until(Duration::from_secs(10), || {
        let text = std::fs::read_to_string(root.join(".agentrec/open.json")).ok()?;
        let journal: serde_json::Value = serde_json::from_str(&text).ok()?;
        let paths: Vec<&str> = journal["files"]
            .as_array()?
            .iter()
            .filter_map(|file| file["path"].as_str())
            .collect();
        (paths.contains(&"declared.rs") && paths.contains(&"concurrent.md")).then_some(())
    });
    assert!(
        staged.is_some(),
        "daemon never staged both bracketed writes"
    );

    let transcript_dir = tempfile::tempdir().unwrap();
    let transcript = transcript_dir.path().join("turn.jsonl");
    std::fs::write(
        &transcript,
        format!(
            concat!(
                r#"{{"timestamp":"2026-09-06T10:00:00.000Z","message":{{"role":"user","content":"attribute writes"}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-09-06T10:00:01.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Edit","input":{{"file_path":"{}"}}}},{{"type":"tool_use","name":"Write","input":{{"file_path":"/outside-agentrec-root.rs"}}}}]}}}}"#,
                "\n"
            ),
            declared.display()
        ),
    )
    .unwrap();
    send_hook(
        root,
        &format!(
            r#"{{"hook_event_name":"Stop","session_id":"s_attr","transcript_path":"{}"}}"#,
            transcript.display()
        ),
    );

    let turn = poll_until(Duration::from_secs(15), || {
        live_rich_turns(root).into_iter().find(|turn| {
            let paths = turn_paths(turn);
            paths.iter().any(|p| p == "declared.rs") && paths.iter().any(|p| p == "concurrent.md")
        })
    })
    .expect("declared and concurrent writes did not land in one rich turn");

    let _ = daemon.kill();
    let output = daemon.wait_with_output().expect("collect daemon output");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ignored 1 declared write path outside recording root"),
        "out-of-root declarations must be visible in daemon diagnostics: {stderr}"
    );

    let files = turn["files"].as_array().expect("turn files");
    let attribution = |path: &str| {
        files
            .iter()
            .find(|file| file["path"] == path)
            .and_then(|file| file.get("attribution"))
            .and_then(|value| value.as_str())
    };
    assert_eq!(attribution("declared.rs"), Some("declared"));
    assert_eq!(attribution("concurrent.md"), Some("undeclared"));
    assert!(
        files
            .iter()
            .all(|file| file["path"] != "/outside-agentrec-root.rs"),
        "a declaration without an observed mutation must not invent a file entry: {turn}"
    );
}

/// The Stop hook is the emitter: given a transcript declaring writes for the
/// current turn, the signal line it appends must carry `files_written` with
/// exactly those absolute paths — earlier-turn writes scoped out, Read
/// results and Bash never declared. No daemon involved: this pins the
/// emitter half of the tier ladder at the signal.jsonl boundary.
#[test]
fn d6_signal_field_stop_hook_declares_current_turn_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = &tmp.path().canonicalize().unwrap();
    init(root);

    let transcript = root.join("transcript.jsonl");
    let a = root.join("declared_a.rs");
    let b = root.join("declared_b.rs");
    std::fs::write(
        &transcript,
        format!(
            concat!(
                // earlier turn's write: must NOT be declared on this Stop
                r#"{{"timestamp":"2026-08-01T10:00:00.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Write","input":{{"file_path":"{stale}"}}}}]}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-08-01T10:05:00.000Z","message":{{"role":"user","content":"do the thing"}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-08-01T10:05:10.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Edit","input":{{"file_path":"{a}"}}}}]}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-08-01T10:05:11.000Z","type":"user","toolUseResult":{{"filePath":"{b}","oldString":"x"}}}}"#,
                "\n",
                // Read-result shape: nested file.filePath, never harvested
                r#"{{"timestamp":"2026-08-01T10:05:12.000Z","type":"user","toolUseResult":{{"type":"text","file":{{"filePath":"{read}"}}}}}}"#,
                "\n",
            ),
            stale = root.join("stale_prior.rs").display(),
            a = a.display(),
            b = b.display(),
            read = root.join("only_read.rs").display(),
        ),
    )
    .unwrap();

    send_hook(
        root,
        &format!(
            r#"{{"hook_event_name":"Stop","session_id":"s_d6","transcript_path":"{}"}}"#,
            transcript.display()
        ),
    );

    let signals = std::fs::read_to_string(root.join(".agentrec/signal.jsonl")).unwrap();
    let stop = signals
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v.get("event").and_then(|e| e.as_str()) == Some("stop"))
        .expect("stop signal appended");
    let declared: Vec<&str> = stop
        .get("files_written")
        .and_then(|f| f.as_array())
        .expect("stop signal carries files_written")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        declared,
        vec![a.to_str().unwrap(), b.to_str().unwrap()],
        "current-turn Write/Edit declarations only — stale prior-turn write \
         scoped out, Read result never declared"
    );
}

/// A Stop with no transcript (L1 emitter) and a start signal both stay
/// silent: no `files_written` key at all — absence means "did not declare",
/// and a start must never carry a declaration.
#[test]
fn d6_signal_field_absent_without_transcript_and_on_start() {
    let tmp = tempfile::tempdir().unwrap();
    let root = &tmp.path().canonicalize().unwrap();
    init(root);

    send_hook(
        root,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s_d6","prompt":"go"}"#,
    );
    send_hook(root, r#"{"hook_event_name":"Stop","session_id":"s_d6"}"#);

    let signals = std::fs::read_to_string(root.join(".agentrec/signal.jsonl")).unwrap();
    for line in signals.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            !line.contains("files_written"),
            "no declaration may be minted without a transcript: {line}"
        );
    }
}

/// A turn whose transcript declares NO writes (agent only read) must emit a
/// stop signal with NO `files_written` key at all — not an empty list.
/// Pins two rules the skeptic gate found unpinned (gaps 1+2):
/// - the hook's empty-list guard: an empty parse is "did not declare"
///   (`None`), never an explicit empty declaration (`[]`) — the two tiers
///   must not disagree on emptiness with the daemon fallback, which applies
///   the same rule;
/// - the over-declaration allowlist at the E2E boundary: the Read tool_use
///   here carries a real in-root `file_path`, so widening DECLARING_TOOLS
///   to Read turns this red.
#[test]
fn d6_signal_field_all_read_turn_declares_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = &tmp.path().canonicalize().unwrap();
    init(root);

    let transcript = root.join("transcript.jsonl");
    std::fs::write(
        &transcript,
        format!(
            concat!(
                r#"{{"timestamp":"2026-08-01T11:00:00.000Z","message":{{"role":"user","content":"what does this do?"}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-08-01T11:00:05.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Read","input":{{"file_path":"{read}"}}}}]}}}}"#,
                "\n",
                // Read result: nested file.filePath shape, never harvested
                r#"{{"timestamp":"2026-08-01T11:00:06.000Z","type":"user","toolUseResult":{{"type":"text","file":{{"filePath":"{read}"}}}}}}"#,
                "\n",
            ),
            read = root.join("only_read.rs").display(),
        ),
    )
    .unwrap();

    send_hook(
        root,
        &format!(
            r#"{{"hook_event_name":"Stop","session_id":"s_d6r","transcript_path":"{}"}}"#,
            transcript.display()
        ),
    );

    let signals = std::fs::read_to_string(root.join(".agentrec/signal.jsonl")).unwrap();
    let stop = signals
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v.get("event").and_then(|e| e.as_str()) == Some("stop"))
        .expect("stop signal appended");
    assert!(
        stop.get("files_written").is_none(),
        "an all-read turn must not declare — no key, not an empty list: {stop}"
    );
}
