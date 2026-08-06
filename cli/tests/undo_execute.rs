//! AC-F4 — the auto-mode tokened two-phase execute (parent spec :641-697 P4/P5).
//!
//! Every test drives the REAL `agentrec mcp` binary as a subprocess. The
//! properties F4 is accountable for are cross-process: a token is issued by
//! one `tools/call` and presented by another, and the only thing joining them
//! is `.agentrec/undo-requests.jsonl`. A token that "works" only inside one
//! process would pass an in-process test and be useless.
//!
//! The re-undo leg additionally shells the CLI `undo`, because "the executed
//! undo is itself a turn" (P5) only means something if the ordinary human verb
//! can revert it.

use agentrec_core::record::{FileEntry, LogRecord, TurnRecord};
use agentrec_core::store::{hash_bytes, BlobStore};
use agentrec_core::undo_coordinator::{
    McpDestructive, UndoCoordinator, UndoRequest, EVENT_CONSUME, EVENT_EXECUTE, EVENT_RESERVE,
};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

const TURN: &str = "t_F40000000000000000000F4";

fn root_with_mode(mode: &str) -> PathBuf {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.keep();
    std::fs::create_dir_all(root.join(".agentrec/objects")).expect("mkdir");
    std::fs::write(
        root.join(".agentrec/config.toml"),
        format!("mcp_destructive = \"{mode}\"\n"),
    )
    .expect("config.toml");
    root
}

fn entry(path: &str) -> FileEntry {
    FileEntry {
        path: path.to_string(),
        before: None,
        after: None,
        op: "modify".to_string(),
        skipped: false,
        withheld: false,
        baseline_unknown: false,
        skipped_reason: None,
        after_synthesized: None,
        link_kind: None,
        attribution: None,
    }
}

/// A file whose `before` blob is stored and whose on-disk bytes equal
/// `after` — the only shape that is actually executable.
fn revertible(root: &Path, path: &str) -> FileEntry {
    let store = BlobStore::new(root.join(".agentrec/objects"));
    let before = store.put(format!("old {path}\n").as_bytes()).unwrap();
    let now = format!("new {path}\n");
    std::fs::write(root.join(path), now.as_bytes()).unwrap();
    let mut e = entry(path);
    e.before = Some(before);
    e.after = Some(hash_bytes(now.as_bytes()));
    e
}

fn seed(root: &Path, files: Vec<FileEntry>) {
    let t = TurnRecord {
        v: 1,
        id: TURN.to_string(),
        grade: "rich".into(),
        truncated: false,
        started: "2026-01-01T00:00:00.000Z".into(),
        ended: "2026-01-01T00:00:01.000Z".into(),
        tool: Some("claude".into()),
        model: None,
        session: None,
        root: root.display().to_string(),
        prompt_ref: None,
        prompt_excerpt: None,
        merges: vec![],
        imported: None,
        files_complete: None,
        origin: None,
        files,
    };
    let line = serde_json::to_string(&LogRecord::Turn(t)).unwrap();
    let p = root.join(".agentrec/log.jsonl");
    let prev = std::fs::read_to_string(&p).unwrap_or_default();
    std::fs::write(&p, format!("{prev}{line}\n")).unwrap();
}

/// Issue an auto-mode token in-process. The wire path for `preview` is F2's
/// and already pinned there; every test here wants a raw token in its arrange
/// step without a second subprocess.
fn issue_token(root: &Path) -> String {
    UndoCoordinator::new(root)
        .preview(
            UndoRequest {
                turn: TURN.to_string(),
                paths: None,
                allow_modified: false,
            },
            McpDestructive::Auto,
        )
        .expect("auto preview")
        .token
        .expect("an executable auto preview issues a token")
}

fn mcp_call(root: &Path, arguments: Value) -> Value {
    let frame = json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {"name": "agentrec_undo", "arguments": arguments},
    });
    let mut child = Command::new(bin())
        .args(["mcp", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentrec mcp");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(format!("{frame}\n").as_bytes())
        .expect("write frame");
    let out = child.wait_with_output().expect("wait mcp");
    assert!(
        out.status.success(),
        "mcp exited {:?}; stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "one response per call: {stdout}");
    serde_json::from_str(lines[0]).expect("JSON response")
}

fn execute(root: &Path, token: &str) -> Value {
    mcp_call(root, json!({"action": "execute", "token": token}))
}

/// The success payload, asserting it was NOT a refusal.
fn ok_payload(resp: &Value) -> Value {
    assert!(resp.get("error").is_none(), "JSON-RPC error: {resp}");
    assert_ne!(
        resp["result"]["isError"], true,
        "unexpected refusal: {resp}"
    );
    serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap())
        .expect("payload is JSON")
}

/// `(code, message)` out of a domain refusal, asserting the domain channel
/// was used at all (`isError: true` result, not `-32602`).
fn domain(resp: &Value) -> (String, String) {
    assert!(
        resp.get("error").is_none(),
        "expected a domain refusal, got a JSON-RPC error: {resp}"
    );
    assert_eq!(
        resp["result"]["isError"], true,
        "a refusal must set isError: {resp}"
    );
    let payload: Value =
        serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap())
            .expect("refusal payload is JSON");
    (
        payload["error"].as_str().unwrap().to_string(),
        payload["message"].as_str().unwrap().to_string(),
    )
}

fn ledger(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join(".agentrec/undo-requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn rows_of(root: &Path, event: &str) -> usize {
    ledger(root).iter().filter(|e| e["event"] == event).count()
}

/// Every turn in `log.jsonl` whose tool is `agentrec` — i.e. the undo turns.
fn undo_turns(root: &Path) -> Vec<TurnRecord> {
    agentrec_core::record::load_log(&root.join(".agentrec/log.jsonl"))
        .into_iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) if t.tool.as_deref() == Some("agentrec") => Some(t),
            _ => None,
        })
        .collect()
}

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .args(["--root", root.to_str().unwrap()])
        .output()
        .expect("spawn agentrec")
}

/// Rewrite every ledger row through `f` — the deterministic time machine the
/// expiry test needs, and the crash-state constructor.
fn rewrite_rows(root: &Path, mut f: impl FnMut(&mut Value)) {
    let mut out = String::new();
    for mut row in ledger(root) {
        f(&mut row);
        out.push_str(&serde_json::to_string(&row).unwrap());
        out.push('\n');
    }
    std::fs::write(root.join(".agentrec/undo-requests.jsonl"), out).unwrap();
}

// ---- AC-F4 (success): revert + turn + re-undoable ---------------------------

/// AC-F4 (successful execute): the revert lands byte-exact, a
/// `tool: "agentrec"` turn is appended, the reservation resolves `executed`
/// with a `consume` row ahead of it — and the undo is itself undoable by the
/// ordinary human verb, returning the tree to its pre-revert bytes.
#[test]
fn ac_f4_execute_reverts_records_a_turn_and_the_undo_is_re_undoable() {
    let root = root_with_mode("auto");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let pre_revert = std::fs::read(root.join("a.rs")).unwrap();
    assert_eq!(pre_revert, b"new a.rs\n");
    let token = issue_token(&root);

    let payload = ok_payload(&execute(&root, &token));
    assert_eq!(payload["turn"], TURN);
    assert_eq!(payload["reverted"], 1);
    assert_eq!(payload["files"][0], "a.rs");
    let undo_turn_id = payload["undo_turn"]
        .as_str()
        .expect("undo_turn id")
        .to_string();

    // The revert is byte-exact against the stored `before` blob.
    assert_eq!(
        std::fs::read(root.join("a.rs")).unwrap(),
        b"old a.rs\n",
        "the revert must have landed byte-exact"
    );

    // P5: recorded as a turn, in the same shape `undo --confirm` records one.
    let turns = undo_turns(&root);
    assert_eq!(turns.len(), 1, "exactly one undo turn");
    let t = &turns[0];
    assert_eq!(t.id, undo_turn_id, "the payload names the turn it wrote");
    assert_eq!(t.tool.as_deref(), Some("agentrec"));
    assert_eq!(t.grade, "rich");
    assert!(!t.truncated);
    assert_eq!(t.files.len(), 1);
    assert_eq!(t.files[0].path, "a.rs");
    // Re-revertible (AC H6): the inverse entry's `before` is the PRE-undo
    // content, which is what makes the next assertion possible at all.
    assert_eq!(
        t.files[0].before.as_deref(),
        Some(hash_bytes(&pre_revert).as_str())
    );
    let raw = std::fs::read_to_string(root.join(".agentrec/log.jsonl")).unwrap();
    let line = raw
        .lines()
        .find(|l| l.contains("\"agentrec\""))
        .expect("undo turn line");
    let v: Value = serde_json::from_str(line).unwrap();
    // Inverted BY F5, in the open: F4 wrote this as `origin` MUST be absent
    // ("F5 owns `origin`; F4 must not add it early"), and F5 flipped it to
    // the value it specifies rather than deleting the assertion. This is the
    // ONLY surface that writes `"mcp"` — an agent spending its own token,
    // with no human in the loop — which is what makes delta decision 11's
    // post-ship counts separable from `approve`'s human-executed `"cli"`.
    assert_eq!(
        v["origin"],
        Value::String("mcp".into()),
        "an auto-mode token execute is the mcp surface: {line}"
    );

    // The ledger: consume BEFORE execute, and exactly one of each.
    let rows = ledger(&root);
    let consume_at = rows.iter().position(|e| e["event"] == EVENT_CONSUME);
    let execute_at = rows.iter().position(|e| e["event"] == EVENT_EXECUTE);
    assert!(
        consume_at.is_some() && execute_at.is_some() && consume_at < execute_at,
        "the token must be consumed before it is executed: {rows:?}"
    );
    assert_eq!(rows_of(&root, EVENT_CONSUME), 1);
    assert_eq!(rows_of(&root, EVENT_EXECUTE), 1);
    let exec = rows.iter().find(|e| e["event"] == EVENT_EXECUTE).unwrap();
    assert_eq!(exec["undo_turn"], Value::String(undo_turn_id.clone()));
    assert!(
        exec["token_sha256"].is_null(),
        "a terminal row must not re-carry the token hash: {exec}"
    );

    // P5, the part that matters: the undo is a turn like any other, so the
    // human verb can revert it and the tree returns to its pre-revert bytes.
    let out = cli(&root, &["undo", &undo_turn_id, "--confirm"]);
    assert!(
        out.status.success(),
        "undo of the undo failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(root.join("a.rs")).unwrap(),
        pre_revert,
        "undoing the undo must restore the exact pre-revert bytes"
    );
    assert_eq!(undo_turns(&root).len(), 2, "the re-undo is a turn too");
}

// ---- AC-F4 (reuse) ----------------------------------------------------------

/// AC-F4 (token reuse): the second presentation of a spent token is refused
/// with a distinct code, the tree is untouched, and no second undo turn is
/// minted.
#[test]
fn ac_f4_a_reused_token_is_refused_and_the_tree_is_untouched() {
    let root = root_with_mode("auto");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let token = issue_token(&root);
    ok_payload(&execute(&root, &token));

    let after_bytes = std::fs::read(root.join("a.rs")).unwrap();
    let after_log = std::fs::read(root.join(".agentrec/log.jsonl")).unwrap();

    let (code, msg) = domain(&execute(&root, &token));
    assert_eq!(code, "token_consumed", "{msg}");
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), after_bytes);
    assert_eq!(
        std::fs::read(root.join(".agentrec/log.jsonl")).unwrap(),
        after_log,
        "a refused execute must not append a second undo turn"
    );
    assert_eq!(undo_turns(&root).len(), 1);
    assert_eq!(rows_of(&root, EVENT_EXECUTE), 1);
}

/// AC-F4 (crash matrix), the ORDERING half: the `consume` row is on disk
/// **before the first working-tree write**, not merely before the `execute`
/// row.
///
/// This is the assertion the constructed-state test below cannot make. That
/// one rebuilds the ledger after the fact, so an implementation that spent
/// the token last — F3's ordering, and the obvious thing to write — leaves an
/// indistinguishable file. The only way to tell the two apart is to look at
/// the ledger *while the writes are happening*, so this test does exactly
/// that: it reverts a turn wide enough (300 files) to make the write window
/// milliseconds rather than microseconds, polls until the worktree is
/// visibly HALF reverted, and reads the ledger at that instant.
///
/// The half-reverted requirement is not decoration. Without it a poll that
/// arrived late would read a completed run's ledger — which carries a
/// `consume` row under either ordering — and the test would be green while
/// never once looking inside the window it exists to probe. The sweep fails
/// loudly rather than passing vacuously if it never lands there.
#[test]
fn ac_f4_the_token_is_spent_before_the_first_worktree_write() {
    const N: usize = 300;
    let root = root_with_mode("auto");
    let files: Vec<FileEntry> = (0..N)
        .map(|i| revertible(&root, &format!("f{i:03}.rs")))
        .collect();
    seed(&root, files);
    let token = issue_token(&root);
    let first = root.join("f000.rs");
    let last = root.join(format!("f{:03}.rs", N - 1));
    let reverted = |p: &Path| std::fs::read(p).unwrap_or_default().starts_with(b"old ");

    let root_for_child = root.clone();
    let token_for_child = token.clone();
    let observed = std::thread::scope(|s| {
        let child = s.spawn(move || execute(&root_for_child, &token_for_child));
        let mut snapshot: Option<Vec<Value>> = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            // Half reverted: the first file has flipped, the last has not.
            // Read the LEDGER first, so a row appearing between the two reads
            // can only make this test more likely to fail, never less.
            let rows = ledger(&root);
            if reverted(&first) && !reverted(&last) {
                snapshot = Some(rows);
                break;
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        ok_payload(&child.join().unwrap());
        snapshot
    });

    let rows = observed.expect(
        "the poll never caught the revert half-done, so it never looked inside the write \
         window this test exists to probe",
    );
    assert!(
        rows.iter().any(|e| e["event"] == EVENT_CONSUME),
        "the token was still unspent while the worktree was being rewritten — a crash here \
         would leave a live token over already-reverted files: {rows:?}"
    );
}

/// AC-F4 (crash matrix, the hard case): a token consumed by a run that died
/// before its `execute` row — and before, or during, its writes — is still
/// dead on the next presentation.
///
/// **Constructed directly rather than by killing a process.** The state a
/// crash leaves is `reserve` + `consume` and nothing else, with the working
/// tree in whatever state the interrupted writes left it; building that
/// exactly is deterministic, where a kill sweep against a stdio server would
/// have to land inside a window measured in microseconds to probe it at all.
/// The worktree is restored to its PRE-revert bytes, which is the most
/// dangerous variant: content drift alone would not refuse the retry, so only
/// the consumption record can.
///
/// What this does NOT establish is *when* the row was written — that is
/// `ac_f4_the_token_is_spent_before_the_first_worktree_write`'s job, and the
/// two are only jointly sufficient.
#[test]
fn ac_f4_a_consumed_token_stays_dead_after_a_crash_before_the_execute_row() {
    let root = root_with_mode("auto");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let pre_revert = std::fs::read(root.join("a.rs")).unwrap();
    let token = issue_token(&root);
    ok_payload(&execute(&root, &token));
    let undo_turn = undo_turns(&root)[0].id.clone();

    // Rewind to the crash state: drop every row after `consume`, put the file
    // back the way it was before the revert, and drop the undo turn the
    // crashed run would never have written.
    let kept: Vec<Value> = ledger(&root)
        .into_iter()
        .filter(|e| e["event"] == EVENT_RESERVE || e["event"] == EVENT_CONSUME)
        .collect();
    assert_eq!(kept.len(), 2, "reserve + consume is the crash state");
    let mut out = String::new();
    for row in &kept {
        out.push_str(&serde_json::to_string(row).unwrap());
        out.push('\n');
    }
    std::fs::write(root.join(".agentrec/undo-requests.jsonl"), out).unwrap();
    std::fs::write(root.join("a.rs"), &pre_revert).unwrap();
    let log = std::fs::read_to_string(root.join(".agentrec/log.jsonl")).unwrap();
    let trimmed: String = log
        .lines()
        .filter(|l| !l.contains(&undo_turn))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(root.join(".agentrec/log.jsonl"), &trimmed).unwrap();
    assert!(undo_turns(&root).is_empty(), "crash state has no undo turn");

    // The tree is bit-for-bit what the preview saw, so nothing but the
    // consumption record can refuse this.
    let (code, msg) = domain(&execute(&root, &token));
    assert_eq!(
        code, "token_consumed",
        "a token consumed by a crashed run must not execute a second time: {msg}"
    );
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), pre_revert);
    assert!(
        undo_turns(&root).is_empty(),
        "the refused retry must mint no turn"
    );
    assert_eq!(rows_of(&root, EVENT_EXECUTE), 0);
}

// ---- AC-F4 (expiry / wrong token): zero side effects ------------------------

/// AC-F4 (expired token): past its 60 s window the token is refused, the tree
/// is untouched, and NOTHING is appended — expiry is already implicit in the
/// reservation's own deadline, so no bookkeeping row is owed.
///
/// Time-travelled by rewriting `expires_unix_ms`, the field liveness actually
/// keys on. Backdating `at_unix_ms` instead would expire nothing and the test
/// would pass only if the execute failed for some other reason.
#[test]
fn ac_f4_an_expired_token_is_refused_with_zero_side_effects() {
    let root = root_with_mode("auto");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let token = issue_token(&root);

    // The window that was actually granted is the spec's 60 seconds.
    let reserve = &ledger(&root)[0];
    assert_eq!(
        reserve["expires_unix_ms"].as_u64().unwrap() - reserve["at_unix_ms"].as_u64().unwrap(),
        60_000,
        "an auto token's TTL is 60s"
    );
    rewrite_rows(&root, |r| r["expires_unix_ms"] = json!(1_000u64));

    let before_wt = std::fs::read(root.join("a.rs")).unwrap();
    let before_ledger = std::fs::read(root.join(".agentrec/undo-requests.jsonl")).unwrap();
    let before_log = std::fs::read(root.join(".agentrec/log.jsonl")).unwrap();

    let (code, msg) = domain(&execute(&root, &token));
    assert_eq!(code, "token_expired", "{msg}");
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), before_wt);
    assert_eq!(
        std::fs::read(root.join(".agentrec/undo-requests.jsonl")).unwrap(),
        before_ledger,
        "an expired token must append no ledger row"
    );
    assert_eq!(
        std::fs::read(root.join(".agentrec/log.jsonl")).unwrap(),
        before_log
    );
    assert!(undo_turns(&root).is_empty());
}

/// AC-F4 (wrong token): a well-formed token nobody issued is refused with its
/// own code and zero side effects — distinguishable from a spent one, because
/// the two mean different things to an agent (re-preview vs. fix your bug).
#[test]
fn ac_f4_a_wrong_token_is_refused_with_zero_side_effects() {
    let root = root_with_mode("auto");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let real = issue_token(&root);
    let wrong = "f".repeat(real.len());
    assert_ne!(wrong, real);

    let before_wt = std::fs::read(root.join("a.rs")).unwrap();
    let before_ledger = std::fs::read(root.join(".agentrec/undo-requests.jsonl")).unwrap();

    let (code, msg) = domain(&execute(&root, &wrong));
    assert_eq!(code, "bad_token", "{msg}");
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), before_wt);
    assert_eq!(
        std::fs::read(root.join(".agentrec/undo-requests.jsonl")).unwrap(),
        before_ledger,
        "a wrong token must append no ledger row"
    );
    assert!(undo_turns(&root).is_empty());

    // …and the real token still works afterwards: a wrong guess must not
    // invalidate the live reservation it failed to match.
    ok_payload(&execute(&root, &real));
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), b"old a.rs\n");
}

// ---- AC-F4 (P4 drift) -------------------------------------------------------

/// P4: the working tree moving between preview and execute aborts, writes
/// nothing, and the refusal tells the agent to take a FRESH preview. The
/// reservation is released in the same breath, so that fresh preview is
/// actually available rather than colliding with its own dead grant.
#[test]
fn ac_f4_drift_between_preview_and_execute_aborts_and_demands_a_fresh_preview() {
    let root = root_with_mode("auto");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let token = issue_token(&root);

    std::fs::write(root.join("a.rs"), b"drifted\n").unwrap();

    let (code, msg) = domain(&execute(&root, &token));
    assert_eq!(code, "preview_stale", "{msg}");
    assert!(
        msg.contains("fresh preview"),
        "the refusal must instruct a fresh preview (P4): {msg}"
    );
    assert!(msg.contains("a.rs"), "and name what drifted: {msg}");
    assert_eq!(
        std::fs::read(root.join("a.rs")).unwrap(),
        b"drifted\n",
        "a drift abort writes nothing"
    );
    assert!(undo_turns(&root).is_empty());
    assert_eq!(rows_of(&root, EVENT_CONSUME), 0, "no write, no consumption");

    // The dead reservation released its paths, so a fresh preview succeeds
    // instead of answering `undo_conflict` against the caller's own grant.
    let fresh = UndoCoordinator::new(&root).preview(
        UndoRequest {
            turn: TURN.to_string(),
            paths: None,
            allow_modified: false,
        },
        McpDestructive::Auto,
    );
    let fresh = fresh.expect("a fresh preview must not collide with the aborted reservation");
    // The file is modified-since now, so it is EXCLUDED — which is the honest
    // answer, and means no token is issued for it.
    assert!(fresh.files.is_empty(), "{:?}", fresh.files);
    assert!(fresh.token.is_none());
}

/// The second half of the drift rail: a token is refused when the executable
/// set SHRANK, not only when content moved. Deleting the before-blob turns
/// the entry into a refusal, and reverting the remainder would apply less
/// than the preview promised.
#[test]
fn ac_f4_an_executable_set_that_shrank_after_the_preview_is_preview_stale() {
    let root = root_with_mode("auto");
    let a = revertible(&root, "a.rs");
    let b = revertible(&root, "b.rs");
    let b_before = b.before.clone().unwrap();
    seed(&root, vec![a, b]);
    let token = issue_token(&root);

    // A `purge --snapshots-before` between preview and execute, in effect:
    // every on-disk hash is untouched, so the CONTENT recheck still passes
    // and only the scope check can catch this.
    let blob = BlobStore::new(root.join(".agentrec/objects"));
    assert!(blob.remove(&b_before).is_some(), "the blob must exist");

    let (code, msg) = domain(&execute(&root, &token));
    assert_eq!(code, "preview_stale", "{msg}");
    assert_eq!(
        std::fs::read(root.join("a.rs")).unwrap(),
        b"new a.rs\n",
        "the still-executable file must NOT be partially reverted"
    );
    assert!(undo_turns(&root).is_empty());
    assert_eq!(rows_of(&root, EVENT_CONSUME), 0);
}

// ---- the guard is checked BEFORE the spend ----------------------------------

/// A concurrent undo refuses the call — and, the load-bearing half, does NOT
/// burn the token: the H7 guard is checked ahead of the spend, so the agent
/// can re-present the same token once the other undo finishes, inside its TTL.
///
/// The re-present leg is what makes this discriminating. Asserting only the
/// refusal would pass against a build that consumed the token first and then
/// refused, which is precisely the behavior the ordering exists to avoid.
#[test]
fn a_concurrent_undo_refuses_without_spending_the_token() {
    let root = root_with_mode("auto");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let token = issue_token(&root);

    // The H7 coordination guard another `undo --confirm` would have written.
    let guard = json!({
        "paths": ["a.rs"],
        "until_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 30_000,
    });
    let guard_path = root.join(".agentrec/undo-guard.json");
    std::fs::write(&guard_path, guard.to_string()).unwrap();

    let (code, msg) = domain(&execute(&root, &token));
    assert_eq!(code, "undo_in_progress", "{msg}");
    assert_eq!(
        rows_of(&root, EVENT_CONSUME),
        0,
        "a collision must not spend the token"
    );
    assert!(undo_turns(&root).is_empty());
    assert_eq!(
        std::fs::read(root.join("a.rs")).unwrap(),
        b"new a.rs\n",
        "nothing was written"
    );

    // The other undo finishes; the SAME token still works.
    std::fs::remove_file(&guard_path).unwrap();
    let payload = ok_payload(&execute(&root, &token));
    assert_eq!(payload["reverted"], 1);
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), b"old a.rs\n");
}

// ---- matrix + argument channel ----------------------------------------------

/// `execute` stays confirm-mode-forbidden (F1's matrix precedes the body),
/// and a missing token is a malformed call (`-32602`), not a domain refusal —
/// the schema declares the argument.
#[test]
fn execute_keeps_its_mode_gate_and_its_argument_channel() {
    let confirm = root_with_mode("confirm");
    seed(&confirm, vec![revertible(&confirm, "a.rs")]);
    let (code, _) = domain(&mcp_call(
        &confirm,
        json!({"action": "execute", "token": "0".repeat(40)}),
    ));
    assert_eq!(code, "wrong_mode");

    let auto = root_with_mode("auto");
    seed(&auto, vec![revertible(&auto, "a.rs")]);
    let resp = mcp_call(&auto, json!({"action": "execute"}));
    assert_eq!(
        resp["error"]["code"], -32602,
        "a missing token is a malformed call: {resp}"
    );
    assert!(!auto.join(".agentrec/undo-requests.jsonl").exists());
}

/// Re-gate round 4 blocker: the FIFO hang was only HALF fixed by the
/// planner-side `inode_refusal`, because `claim_grant` runs its drift loop
/// through `read_current_hash` BEFORE it ever calls `build_plan`. A target
/// swapped for a fifo between preview and execute therefore blocked in
/// `std::fs::read` with the gate never reached — killing the single-threaded
/// stdio MCP loop for that whole session. `agentrec approve` shares
/// `claim_grant` and was exposed identically.
///
/// The fix guards the PRIMITIVE (`read_current_hash` lstats first), so this
/// test pins the leg the planner cannot protect. Like the unit-level fifo
/// test, a regression makes this HANG rather than fail — which is why the
/// harness-level timeout matters more here than the assertion.
#[test]
#[cfg(unix)]
fn a_fifo_swapped_in_after_the_preview_refuses_instead_of_hanging() {
    let root = root_with_mode("auto");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let token = issue_token(&root);

    // Swap the previewed regular file for a fifo — the shape that hung.
    std::fs::remove_file(root.join("a.rs")).unwrap();
    let c = std::ffi::CString::new(root.join("a.rs").as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(
        unsafe { libc::mkfifo(c.as_ptr(), 0o600) },
        0,
        "fixture must actually create a fifo"
    );

    let (code, msg) = domain(&execute(&root, &token));
    assert_eq!(
        code, "preview_stale",
        "a fifo must read as drift, not block: {msg}"
    );
    assert!(undo_turns(&root).is_empty(), "nothing may be written");
    assert_eq!(
        rows_of(&root, EVENT_CONSUME),
        0,
        "an aborted execute must not spend the token"
    );
}
