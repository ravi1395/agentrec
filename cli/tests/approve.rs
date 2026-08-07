//! AC-F3 — the confirm-mode request ledger and `agentrec approve|deny`.
//!
//! Every test here drives the REAL binary as a subprocess, because the
//! properties F3 is accountable for are cross-process ones: a request lodged
//! by an MCP server is approved by a human running a different `agentrec`
//! minutes later, and the only thing joining them is
//! `.agentrec/undo-requests.jsonl`. The lock-contention and kill-9 cases have
//! no in-process form at all.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures, which this harness created;
//    there is no attacker-supplied FIFO to block on, so the fsguard wrappers
//    buy nothing here. Production reads stay lint-enforced (clippy.toml).

use agentrec_core::record::{FileEntry, LogRecord, TurnRecord};
use agentrec_core::store::{hash_bytes, BlobStore};
use agentrec_core::undo_coordinator::{
    McpDestructive, UndoCoordinator, UndoRequest, EVENT_APPROVE, EVENT_EXECUTE, EVENT_REQUEST,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

const TURN: &str = "t_F30000000000000000000F3";

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

/// Lodge a request in-process. The wire path is proven separately by
/// [`request_and_status_cross_the_mcp_wire`]; every other test wants a
/// request id without a subprocess round trip in the arrange step.
fn lodge(root: &Path, paths: Option<Vec<&str>>, allow_modified: bool) -> String {
    UndoCoordinator::new(root)
        .request_human(
            UndoRequest {
                turn: TURN.to_string(),
                paths: paths.map(|v| v.into_iter().map(PathBuf::from).collect()),
                allow_modified,
            },
            McpDestructive::Confirm,
        )
        .expect("request lodged")
        .status
        .request
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .args(["--root", root.to_str().unwrap()])
        .output()
        .expect("spawn agentrec")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn ledger(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join(".agentrec/undo-requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn state(root: &Path, id: &str) -> String {
    UndoCoordinator::new(root)
        .status(id)
        .expect("status")
        .state
        .as_str()
        .to_string()
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

// ---- AC: approve-executes-once ---------------------------------------------

/// AC-F3 (approve-executes-once): the first approve reverts and records; the
/// second is a clean error naming the state, writes nothing, and does not
/// mint a second undo turn.
#[test]
fn approve_executes_once_and_a_second_approve_is_a_clean_error() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let id = lodge(&root, None, false);

    let first = run(&root, &["approve", &id]);
    assert!(
        first.status.success(),
        "approve failed: {}{}",
        stdout(&first),
        stderr(&first)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("a.rs")).unwrap(),
        "old a.rs\n",
        "the revert must have landed"
    );
    assert_eq!(state(&root, &id), "executed");
    assert_eq!(undo_turns(&root).len(), 1);

    // Freeze the exact post-approve worktree + log, then approve again.
    let after_bytes = std::fs::read(root.join("a.rs")).unwrap();
    let after_log = std::fs::read(root.join(".agentrec/log.jsonl")).unwrap();

    let second = run(&root, &["approve", &id]);
    assert!(!second.status.success(), "a second approve must fail");
    let msg = stderr(&second);
    assert!(
        msg.contains("executed"),
        "the error must name the state it is actually in: {msg}"
    );
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), after_bytes);
    assert_eq!(
        std::fs::read(root.join(".agentrec/log.jsonl")).unwrap(),
        after_log,
        "a refused approve must not append a second undo turn"
    );
    assert_eq!(undo_turns(&root).len(), 1);
}

/// The undo turn an approval writes is the SAME shape `undo --confirm`
/// writes: `tool: "agentrec"`, rich, an inverse entry per reverted file, and
/// — since F5 landed — `origin: "cli"`, the value `undo --confirm` itself
/// writes.
///
/// This assertion was inverted BY F5, deliberately and in the open: F3 wrote
/// it as `origin` MUST be absent ("F5 owns `origin`; F3 must not add it
/// early"), and F5 flipped it to the value F5 specifies rather than deleting
/// it. Either way the property under test is unchanged and is the point of
/// the whole shared-`execute_claim` seam — **approve and `undo --confirm`
/// agree**. What makes `"cli"` the right value here is that `origin` names
/// the surface that EXECUTED the writes, and a human ran this verb; that the
/// undo was REQUESTED over MCP is the request ledger's business, asserted
/// below and in `cli/tests/undo_origin.rs`.
#[test]
fn the_approved_undo_turn_has_the_shape_cli_undo_produces() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let id = lodge(&root, None, false);
    assert!(run(&root, &["approve", &id]).status.success());

    let turns = undo_turns(&root);
    assert_eq!(turns.len(), 1);
    let t = &turns[0];
    assert_eq!(t.tool.as_deref(), Some("agentrec"));
    assert_eq!(t.grade, "rich");
    assert!(!t.truncated);
    assert_eq!(t.files.len(), 1);
    assert_eq!(t.files[0].path, "a.rs");
    // Re-revertible (AC H6): the inverse entry's `before` is the pre-undo
    // content, so the undo can itself be undone.
    assert!(t.files[0].before.is_some());

    let raw = std::fs::read_to_string(root.join(".agentrec/log.jsonl")).unwrap();
    let line = raw
        .lines()
        .find(|l| l.contains("\"agentrec\""))
        .expect("undo turn line");
    let v: Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        v["origin"],
        Value::String("cli".into()),
        "an approve executes at a human's keyboard, so it is the cli \
         surface — not `mcp`, and not absent: {line}"
    );

    // The terminal ledger row points at its own evidence.
    let exec = ledger(&root)
        .into_iter()
        .find(|e| e["event"] == EVENT_EXECUTE)
        .expect("an execute row");
    assert_eq!(exec["undo_turn"], Value::String(t.id.clone()));
}

// ---- AC: expiry -------------------------------------------------------------

/// AC-F3 (expiry): a request past its 10-minute window is refused, records
/// its lapse, and writes nothing.
///
/// Time-travelled by rewriting the lodged row's `expires_unix_ms` — the field
/// liveness actually keys on. Backdating `at_unix_ms` instead would expire
/// nothing and the test would pass only because the approve failed for some
/// other reason.
#[test]
fn an_expired_request_is_refused_and_its_lapse_is_recorded() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let id = lodge(&root, None, false);

    // The window that was actually granted is D23's ten minutes.
    let lodged = &ledger(&root)[0];
    let granted =
        lodged["expires_unix_ms"].as_u64().unwrap() - lodged["at_unix_ms"].as_u64().unwrap();
    assert_eq!(
        granted, 600_000,
        "D23: pending requests expire after 10 minutes"
    );

    let path = root.join(".agentrec/undo-requests.jsonl");
    let mut row: Value =
        serde_json::from_str(std::fs::read_to_string(&path).unwrap().trim()).unwrap();
    row["expires_unix_ms"] = Value::from(row["at_unix_ms"].as_u64().unwrap() - 1);
    std::fs::write(&path, format!("{row}\n")).unwrap();

    assert_eq!(state(&root, &id), "expired", "derived from the clock alone");

    let before = std::fs::read(root.join("a.rs")).unwrap();
    let out = run(&root, &["approve", &id]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("expired"), "{}", stderr(&out));
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), before);
    assert!(undo_turns(&root).is_empty());
    assert!(
        ledger(&root).iter().any(|e| e["event"] == "expire"),
        "the lapse must be recorded, not left implicit: {:?}",
        ledger(&root)
    );
}

// ---- AC: drift --------------------------------------------------------------

/// AC-F3 (drift): a target file mutated between request and approve aborts
/// with `preview_stale` and writes nothing.
///
/// Lodged with `allow_modified: true` ON PURPOSE. At `false` the planner
/// alone would exclude the mutated file and the approve would fail as
/// "nothing to revert" — which passes a drift test for the wrong reason. With
/// the flag on, the planner deliberately admits the modified file, so ONLY
/// the recorded-hash recheck can catch this.
#[test]
fn drift_after_a_request_is_preview_stale_and_writes_nothing() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let id = lodge(&root, None, true);

    std::fs::write(root.join("a.rs"), b"a human typed this\n").unwrap();

    let out = run(&root, &["approve", &id]);
    assert!(!out.status.success());
    let msg = stderr(&out);
    assert!(msg.contains("stale"), "{msg}");
    assert!(
        msg.contains("a.rs"),
        "the drifted path must be named: {msg}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("a.rs")).unwrap(),
        "a human typed this\n",
        "drift must not revert the human's edit"
    );
    assert!(undo_turns(&root).is_empty());
    assert_eq!(state(&root, &id), "failed");
    let fail = ledger(&root)
        .into_iter()
        .find(|e| e["event"] == "fail")
        .expect("a fail row");
    assert_eq!(fail["reason"], "preview_stale");
}

/// The ordinary case: at `allow_modified: false` a drifted path is caught
/// too. Pinned separately so the `true` case above cannot be the only
/// coverage of the recheck.
#[test]
fn drift_is_caught_at_allow_modified_false_as_well() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let id = lodge(&root, None, false);
    std::fs::write(root.join("a.rs"), b"drifted\n").unwrap();

    let out = run(&root, &["approve", &id]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("stale"), "{}", stderr(&out));
    assert_eq!(
        std::fs::read_to_string(root.join("a.rs")).unwrap(),
        "drifted\n"
    );
    assert!(undo_turns(&root).is_empty());
}

/// Drift is compared as `Option`: a target DELETED after the request is
/// drift, and the absent→absent case is not confused with it.
#[test]
fn deleting_a_target_after_the_request_is_drift() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let id = lodge(&root, None, true);
    std::fs::remove_file(root.join("a.rs")).unwrap();

    let out = run(&root, &["approve", &id]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("stale"), "{}", stderr(&out));
    assert!(!root.join("a.rs").exists());
    assert!(undo_turns(&root).is_empty());
}

// ---- AC: no phantom approval across a crash --------------------------------

/// AC-F3 (kill -9 between claim and execute): a killed approve leaves no
/// phantom approval.
///
/// The assertion is deliberately WINDOW-INDEPENDENT. The durable approval row
/// is appended only after the working-tree writes AND the undo turn's fsync,
/// so at every instant the process can die the ledger is in one of exactly
/// two states. Asserting that invariant — rather than trying to land the kill
/// inside a microsecond-wide window — is what makes this test honest instead
/// of flaky: wherever it lands, the property must hold.
/// SWEPT across kill delays rather than aimed at one. A single delay would
/// either miss the pre-write window entirely (the whole claim→execute path
/// runs in a few ms, after which the process spends 3s in the H7 guard
/// linger) or be tuned to a machine. The sweep asserts the invariant at every
/// delay AND requires that at least one run died before the writes landed —
/// otherwise the test would be green while never once entering the window it
/// exists to probe.
#[test]
fn a_killed_approve_never_leaves_a_phantom_approval() {
    let mut observed: Vec<(u64, String)> = Vec::new();
    for delay_ms in [0u64, 1, 2, 3, 5, 8, 13, 21, 34, 60, 100, 150, 250, 400] {
        let root = root_with_mode("confirm");
        seed(&root, vec![revertible(&root, "a.rs")]);
        let id = lodge(&root, None, false);

        let mut child = Command::new(bin())
            .args(["approve", &id, "--root", root.to_str().unwrap()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn approve");
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        let _ = child.kill();
        let _ = child.wait();

        let rows = ledger(&root);
        assert!(
            !rows.iter().any(|e| e["event"] == EVENT_APPROVE),
            "F3 never writes an `approve` row (delay {delay_ms}ms): {rows:?}"
        );
        let st = state(&root, &id);
        assert_ne!(
            st, "approved",
            "a killed approve must never read as approved (delay {delay_ms}ms)"
        );
        assert!(
            st == "pending" || st == "executed",
            "at every kill point the ledger is pending or executed, got {st} (delay {delay_ms}ms)"
        );
        // The strong form: an executed request must have its undo turn on
        // disk, and a pending one must have no undo turn at all. Either way
        // the ledger and the log agree — no approval exists without its
        // evidence, and no evidence exists without its approval.
        if st == "executed" {
            assert_eq!(
                undo_turns(&root).len(),
                1,
                "executed without an undo turn (delay {delay_ms}ms)"
            );
            assert_eq!(
                std::fs::read_to_string(root.join("a.rs")).unwrap(),
                "old a.rs\n"
            );
        } else {
            assert!(
                undo_turns(&root).is_empty(),
                "a still-pending request must not have written a turn (delay {delay_ms}ms)"
            );
        }
        observed.push((delay_ms, st));
    }

    assert!(
        observed.iter().any(|(_, s)| s == "pending"),
        "the sweep never once died before the writes landed, so it never entered the \
         claim-to-execute window this test exists to probe: {observed:?}"
    );
    // Not asserted in the other direction: whether any delay lands AFTER the
    // execute row is machine-dependent, and the invariant is what matters.
    eprintln!("kill-9 sweep (delay_ms, state): {observed:?}");
}

/// The deterministic companion: a ledger holding a `request` and nothing else
/// — the exact on-disk state a crash mid-approve can leave — reads as
/// `pending`, never `approved`. Constructed directly, so it does not depend
/// on where a real kill happened to land.
#[test]
fn a_request_with_no_terminal_row_reads_as_pending_not_approved() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let id = lodge(&root, None, false);

    let rows = ledger(&root);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["event"], EVENT_REQUEST);
    assert_eq!(state(&root, &id), "pending");
}

// ---- AC: lock contention ----------------------------------------------------

/// AC-F3 (lock contention): two approves of the same request racing — one
/// wins and executes, the other gets a clean error, and exactly one undo turn
/// exists. The loser cannot half-execute: it blocks on `.agentrec/undo.lock`,
/// and by the time it reads the ledger the winner's terminal row is there.
#[test]
fn two_racing_approves_produce_one_execution_and_one_clean_error() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let id = lodge(&root, None, false);

    let spawn = || {
        Command::new(bin())
            .args(["approve", &id, "--root", root.to_str().unwrap()])
            .output()
            .expect("spawn approve")
    };
    let (a, b) = std::thread::scope(|s| {
        let ha = s.spawn(spawn);
        let hb = s.spawn(spawn);
        (ha.join().unwrap(), hb.join().unwrap())
    });

    let wins = [&a, &b].iter().filter(|o| o.status.success()).count();
    assert_eq!(
        wins,
        1,
        "exactly one approve may win; stdout/stderr: {} | {} || {} | {}",
        stdout(&a),
        stderr(&a),
        stdout(&b),
        stderr(&b)
    );
    let loser = if a.status.success() { &b } else { &a };
    assert!(
        stderr(loser).contains("executed"),
        "the loser needs a clean state-naming error, got: {}",
        stderr(loser)
    );
    assert_eq!(undo_turns(&root).len(), 1, "exactly one undo turn");
    assert_eq!(state(&root, &id), "executed");
    assert_eq!(
        ledger(&root)
            .iter()
            .filter(|e| e["event"] == EVENT_EXECUTE)
            .count(),
        1
    );
}

// ---- deny -------------------------------------------------------------------

/// `agentrec deny <id>` records the decision and touches nothing.
#[test]
fn deny_records_the_decision_without_writing() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let id = lodge(&root, None, false);
    let before = std::fs::read(root.join("a.rs")).unwrap();

    let out = run(&root, &["deny", &id]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), before);
    assert!(undo_turns(&root).is_empty());
    assert_eq!(state(&root, &id), "denied");

    // And a denied request cannot then be approved.
    let after = run(&root, &["approve", &id]);
    assert!(!after.status.success());
    assert!(stderr(&after).contains("denied"), "{}", stderr(&after));
    assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), before);
}

// ---- reservations ------------------------------------------------------------

/// A pending confirm request holds its paths against an auto-mode preview in
/// another process. Without `request` being a reservation-creating event,
/// an auto token would be minted over paths a human is being asked to
/// approve — two live grants on one file.
#[test]
fn a_pending_request_reserves_its_paths_against_a_later_auto_preview() {
    let root = root_with_mode("confirm");
    seed(
        &root,
        vec![revertible(&root, "a.rs"), revertible(&root, "b.rs")],
    );
    let _id = lodge(&root, Some(vec!["a.rs"]), false);

    let co = UndoCoordinator::new(&root);
    let overlap = co.preview(
        UndoRequest {
            turn: TURN.to_string(),
            paths: Some(vec![PathBuf::from("a.rs")]),
            allow_modified: false,
        },
        McpDestructive::Auto,
    );
    let err = overlap.expect_err("overlap must be refused");
    assert_eq!(err.code(), "undo_conflict", "{err}");

    // Disjoint paths still work.
    co.preview(
        UndoRequest {
            turn: TURN.to_string(),
            paths: Some(vec![PathBuf::from("b.rs")]),
            allow_modified: false,
        },
        McpDestructive::Auto,
    )
    .expect("a disjoint auto preview must still reserve");
}

/// A resolved request releases its paths immediately rather than holding them
/// for the rest of its ten minutes — that is what the terminal writers buy.
#[test]
fn denying_a_request_releases_its_reservation_at_once() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let id = lodge(&root, Some(vec!["a.rs"]), false);
    assert!(run(&root, &["deny", &id]).status.success());

    let second = lodge(&root, Some(vec!["a.rs"]), false);
    assert_ne!(second, id);
    assert_eq!(state(&root, &second), "pending");
}

// ---- status + listing --------------------------------------------------------

/// `status` distinguishes every state the parent spec names. `approved` is
/// the one F3 never writes (see the coordinator's module header), so it is
/// proven by planting the row a future host-native approval UI would write —
/// which is precisely the reader path that keeps such a request from being
/// misreported as pending.
#[test]
fn status_distinguishes_pending_denied_expired_executed_failed_and_approved() {
    let root = root_with_mode("confirm");
    seed(
        &root,
        vec![
            revertible(&root, "p.rs"),
            revertible(&root, "d.rs"),
            revertible(&root, "x.rs"),
            revertible(&root, "e.rs"),
            revertible(&root, "f.rs"),
            revertible(&root, "v.rs"),
        ],
    );

    let pending = lodge(&root, Some(vec!["p.rs"]), false);
    assert_eq!(state(&root, &pending), "pending");

    let denied = lodge(&root, Some(vec!["d.rs"]), false);
    assert!(run(&root, &["deny", &denied]).status.success());
    assert_eq!(state(&root, &denied), "denied");

    let executed = lodge(&root, Some(vec!["e.rs"]), false);
    assert!(run(&root, &["approve", &executed]).status.success());
    assert_eq!(state(&root, &executed), "executed");

    let failed = lodge(&root, Some(vec!["f.rs"]), true);
    std::fs::write(root.join("f.rs"), b"drift\n").unwrap();
    assert!(!run(&root, &["approve", &failed]).status.success());
    assert_eq!(state(&root, &failed), "failed");

    // Expired: backdate the granted window.
    let expired = lodge(&root, Some(vec!["x.rs"]), false);
    backdate(&root, &expired);
    assert_eq!(state(&root, &expired), "expired");

    // Approved: plant the row F3 itself never writes.
    let approved = lodge(&root, Some(vec!["v.rs"]), false);
    plant_approve(&root, &approved);
    assert_eq!(
        state(&root, &approved),
        "approved",
        "a decision-only row must not be misreported as pending"
    );

    // Bare `approve` lists exactly the requests still awaiting a human.
    // Keyed on each request's unique path rather than its id: the listing
    // renders ids in the elided `short_id` form every other verb uses, so a
    // raw-id substring match would fail for a reason that has nothing to do
    // with which requests are pending.
    let listed = run(&root, &["approve"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let text = stdout(&listed);
    assert!(
        text.contains("p.rs"),
        "the pending request must be listed: {text}"
    );
    for (resolved, path) in [
        (&denied, "d.rs"),
        (&executed, "e.rs"),
        (&failed, "f.rs"),
        (&expired, "x.rs"),
        (&approved, "v.rs"),
    ] {
        assert!(
            !text.contains(path),
            "{resolved} is {} but was listed as pending: {text}",
            state(&root, resolved)
        );
    }
    assert_eq!(
        text.lines().filter(|l| l.contains("undo of")).count(),
        1,
        "exactly one pending row: {text}"
    );
}

fn backdate(root: &Path, id: &str) {
    rewrite_rows(root, |row| {
        if row["id"] == id && row["event"] == EVENT_REQUEST {
            row["expires_unix_ms"] = Value::from(row["at_unix_ms"].as_u64().unwrap() - 1);
        }
    });
}

/// Append the `approve` row F3 never writes, exactly as a future host-native
/// approval UI would.
fn plant_approve(root: &Path, id: &str) {
    let request = ledger(root)
        .into_iter()
        .find(|e| e["id"] == id && e["event"] == EVENT_REQUEST)
        .expect("the request row");
    let mut row = request.clone();
    row["event"] = Value::from(EVENT_APPROVE);
    let path = root.join(".agentrec/undo-requests.jsonl");
    let prev = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{prev}{row}\n")).unwrap();
}

fn rewrite_rows(root: &Path, mut f: impl FnMut(&mut Value)) {
    let path = root.join(".agentrec/undo-requests.jsonl");
    let mut out = String::new();
    for line in std::fs::read_to_string(&path).unwrap().lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut row: Value = serde_json::from_str(line).unwrap();
        f(&mut row);
        out.push_str(&format!("{row}\n"));
    }
    std::fs::write(&path, out).unwrap();
}

// ---- the MCP wire ------------------------------------------------------------

/// `request` and `status` over the real `agentrec mcp` server: the sub-actions
/// F1 routed and left `not_implemented`. Two separate server processes, so the
/// second can only see the first's request by reading the ledger off disk.
#[test]
fn request_and_status_cross_the_mcp_wire() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);

    let lodged = undo_tool(
        &root,
        serde_json::json!({"action": "request", "turn": TURN}),
    );
    assert_eq!(lodged["state"], "pending");
    assert_eq!(lodged["turn"], TURN);
    assert_eq!(lodged["paths"][0], "a.rs");
    assert_eq!(lodged["preview"]["files"][0]["path"], "a.rs");
    let id = lodged["request"]
        .as_str()
        .expect("a request id")
        .to_string();

    let polled = undo_tool(
        &root,
        serde_json::json!({"action": "status", "request_id": id}),
    );
    assert_eq!(polled["state"], "pending");
    assert_eq!(polled["request"], id);

    // The human approves out of band; the agent's next poll sees it.
    assert!(run(&root, &["approve", &id]).status.success());
    let after = undo_tool(
        &root,
        serde_json::json!({"action": "status", "request_id": id}),
    );
    assert_eq!(after["state"], "executed");
    assert!(after["undo_turn"].is_string(), "{after}");
}

/// One `tools/call agentrec_undo`, asserting success and returning the
/// decoded payload.
fn undo_tool(root: &Path, arguments: Value) -> Value {
    let frame = serde_json::json!({
        "jsonrpc": "2.0", "id": 9, "method": "tools/call",
        "params": {"name": "agentrec_undo", "arguments": arguments},
    });
    let out = Command::new(bin())
        .args(["mcp", "--root", root.to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin
                .as_mut()
                .unwrap()
                .write_all(format!("{frame}\n").as_bytes())?;
            c.wait_with_output()
        })
        .expect("mcp server");
    let line = stdout(&out)
        .lines()
        .find(|l| l.contains("\"result\"") || l.contains("\"error\""))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no response: {}{}", stdout(&out), stderr(&out)));
    let resp: Value = serde_json::from_str(&line).unwrap();
    assert!(resp.get("error").is_none(), "JSON-RPC error: {resp}");
    assert_ne!(resp["result"]["isError"], true, "refusal: {resp}");
    serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

// ---- the id a human actually reads ------------------------------------------

/// D23's loop end to end: `agentrec approve` lists pending requests, and the
/// id it prints is approved by pasting it straight back. Every other test here
/// takes the id from `lodge`'s return value, so none of them can see a
/// listing that renders an id a user cannot use — which is exactly the defect
/// this pins. The id is PARSED OUT OF STDOUT on purpose; asserting the
/// rendered string against a locally-computed expectation would re-implement
/// the renderer and pass with it.
#[test]
fn the_listed_id_can_be_pasted_straight_back_into_approve() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let expected = lodge(&root, None, false);

    let listed = run(&root, &["approve"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let text = stdout(&listed);
    let printed = text
        .lines()
        .find(|l| l.contains("undo of"))
        .and_then(|l| l.split_whitespace().next())
        .expect("a pending row naming a request id")
        .to_string();
    assert_eq!(
        printed, expected,
        "the listed id must BE the request id, not a re-formatting of it: {text}"
    );

    let out = run(&root, &["approve", &printed]);
    assert!(
        out.status.success(),
        "the listed id must resolve: {}{}",
        stdout(&out),
        stderr(&out)
    );
    assert_eq!(state(&root, &expected), "executed");
    // And the success line names the same id, so a scripted caller can join
    // the two.
    assert!(stdout(&out).contains(&expected), "{}", stdout(&out));
}

/// Same round trip for `deny`, which shares the renderer.
#[test]
fn the_listed_id_can_be_pasted_straight_back_into_deny() {
    let root = root_with_mode("confirm");
    seed(&root, vec![revertible(&root, "a.rs")]);
    let expected = lodge(&root, None, false);
    let text = stdout(&run(&root, &["approve"]));
    let printed = text
        .lines()
        .find(|l| l.contains("undo of"))
        .and_then(|l| l.split_whitespace().next())
        .expect("a pending row")
        .to_string();

    let out = run(&root, &["deny", &printed]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(state(&root, &expected), "denied");
    assert!(stdout(&out).contains(&expected), "{}", stdout(&out));
}

// ---- scope drift -------------------------------------------------------------

/// A request binds the SELECTED PATHS, not just their content. If one entry
/// stops being executable between the request and the approval — here, its
/// `before` blob is purged out of the CAS, which leaves every on-disk hash
/// untouched so the content recheck passes — approving the remainder would
/// apply a narrower revert than the human agreed to, and report success. That
/// is `preview_stale` too, and nothing is written.
#[test]
fn an_executable_set_that_shrank_after_the_request_is_preview_stale() {
    let root = root_with_mode("confirm");
    let a = revertible(&root, "a.rs");
    let b = revertible(&root, "b.rs");
    let doomed = b.before.clone().unwrap();
    seed(&root, vec![a, b]);
    let id = lodge(&root, None, false);

    // Remove b.rs's prior snapshot; a.rs is untouched and still executable.
    let blob = BlobStore::new(root.join(".agentrec/objects"));
    assert!(
        blob.remove(&doomed).is_some(),
        "the before blob must exist to remove"
    );
    assert!(!blob.contains(&doomed));

    let before_a = std::fs::read(root.join("a.rs")).unwrap();
    let out = run(&root, &["approve", &id]);
    assert!(
        !out.status.success(),
        "a shrunk set must not silently apply"
    );
    assert!(stderr(&out).contains("stale"), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("b.rs"),
        "name what dropped out: {}",
        stderr(&out)
    );
    assert_eq!(
        std::fs::read(root.join("a.rs")).unwrap(),
        before_a,
        "the still-executable file must NOT be reverted on its own"
    );
    assert!(undo_turns(&root).is_empty());
    assert_eq!(state(&root, &id), "failed");
}

// ---- AC: mid-revert failure (Phase F review finding 11) ---------------------

/// A revert that fails PARTWAY — the branch in `approvecmd::execute_claim`
/// that both of the review's swallowed `let _ =` writes live in.
///
/// **Injection: the second file is made read-only (0o444) after the request
/// is lodged.** Every cheaper-looking injection is refused before this branch
/// is reachable, which is the point: removing the before-blob turns the entry
/// into a `build_plan` refusal and `claim_grant` answers `preview_stale`
/// (pinned by `an_executable_set_that_shrank_after_the_request_is_preview_stale`
/// directly above), and anything that changes the file's BYTES trips the
/// content recheck. A mode bit changes neither the plan nor any hash — the
/// claim admits the file, and `restore_from_before`'s `fs::write` is the
/// first thing that sees it. That is exactly the shape this branch exists
/// for: a failure only the working tree can report, discovered after other
/// files have already been written.
#[cfg(unix)]
#[test]
fn a_mid_revert_failure_records_what_landed_and_releases_the_reservation() {
    use std::os::unix::fs::PermissionsExt;

    let root = root_with_mode("confirm");
    seed(
        &root,
        vec![revertible(&root, "a.rs"), revertible(&root, "b.rs")],
    );
    let id = lodge(&root, None, false);

    let b = root.join("b.rs");
    let mut perms = std::fs::metadata(&b).unwrap().permissions();
    perms.set_mode(0o444);
    std::fs::set_permissions(&b, perms).unwrap();

    let out = run(&root, &["approve", &id]);
    assert!(
        !out.status.success(),
        "a partial revert must not report success: {}",
        stdout(&out)
    );
    let msg = stderr(&out);
    assert!(
        msg.contains("b.rs") && msg.contains("failed to write"),
        "the error must carry the REVERT failure, not the bookkeeping: {msg}"
    );

    // a.rs was written before b.rs failed; b.rs was not.
    assert_eq!(
        std::fs::read_to_string(root.join("a.rs")).unwrap(),
        "old a.rs\n",
        "the first file's revert really landed"
    );
    assert_eq!(
        std::fs::read_to_string(&b).unwrap(),
        "new b.rs\n",
        "the failed file must be untouched"
    );

    // The write that landed is RECORDED, truncated, and names only a.rs.
    let turns = undo_turns(&root);
    assert_eq!(
        turns.len(),
        1,
        "the partial revert must mint exactly one turn"
    );
    assert!(turns[0].truncated, "a partial undo turn must say so");
    let recorded: Vec<&str> = turns[0].files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        recorded,
        vec!["a.rs"],
        "the truncated turn must record only what was actually written"
    );

    // The reservation is resolved, not left to lapse at TTL.
    assert_eq!(state(&root, &id), "failed");
    let fail_rows: Vec<Value> = ledger(&root)
        .into_iter()
        .filter(|e| e["event"] == "fail" && e["id"] == id.as_str())
        .collect();
    assert_eq!(fail_rows.len(), 1, "exactly one terminal fail row");
    assert!(
        fail_rows[0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("b.rs"),
        "the fail row must carry the cause: {}",
        fail_rows[0]
    );
}
