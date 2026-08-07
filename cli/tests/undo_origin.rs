//! AC-F5 — the `origin` discriminator on undo turns (delta decision 11).
//!
//! `origin` names **the surface that executed the writes**, not the surface
//! that asked for them: `undo --confirm` and `approve` are both a human at a
//! keyboard and both write `"cli"`; only the MCP auto-mode `execute` — an
//! agent spending its own token, with no human in the loop — writes `"mcp"`.
//! Delta decision 11 is silent on `approve`, and the reason for that reading
//! is the row it exists to feed: decision 6's gate counts *human-confirmed*
//! undos, and an approve IS one.
//!
//! Every assertion here is made against the RAW `log.jsonl` bytes rather than
//! a parsed `TurnRecord`, because two of the three claims are about what is
//! on the wire — a parsed record cannot distinguish an absent `origin` from
//! an explicit `"cli"`, which is exactly the distinction the legacy-reading
//! test pins.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures, which this harness created;
//    there is no attacker-supplied FIFO to block on, so the fsguard wrappers
//    buy nothing here. Production reads stay lint-enforced (clippy.toml).

use agentrec_core::record::{FileEntry, LogRecord, TurnRecord};
use agentrec_core::store::{hash_bytes, BlobStore};
use agentrec_core::undo_coordinator::{McpDestructive, UndoCoordinator, UndoRequest};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

const TURN: &str = "t_F50000000000000000000F5";

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

fn seed(root: &Path) {
    let files = vec![revertible(root, "src.rs")];
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

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .args(["--root", root.to_str().unwrap()])
        .output()
        .expect("spawn agentrec")
}

fn mcp_call(root: &Path, arguments: Value) -> Value {
    let frame = json!({
        "jsonrpc": "2.0",
        "id": 5,
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
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "one response per call: {stdout}");
    let resp: Value = serde_json::from_str(lines[0]).expect("JSON response");
    assert!(resp.get("error").is_none(), "JSON-RPC error: {resp}");
    assert_ne!(
        resp["result"]["isError"], true,
        "unexpected refusal: {resp}"
    );
    resp
}

/// The raw `log.jsonl` lines whose `tool` is `agentrec` — the undo turns, as
/// bytes. Deliberately NOT parsed: absence of `origin` is a claim about the
/// wire that a `TurnRecord` erases.
fn undo_turn_lines(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join(".agentrec/log.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("log line is JSON"))
        .filter(|v| v["tool"] == "agentrec")
        .collect()
}

/// The one undo turn, asserting there is exactly one.
fn the_undo_turn(root: &Path) -> Value {
    let lines = undo_turn_lines(root);
    assert_eq!(lines.len(), 1, "expected exactly one undo turn: {lines:?}");
    lines.into_iter().next().unwrap()
}

// ---- AC-F5.1: a CLI `undo --confirm` writes origin "cli" --------------------

#[test]
fn ac_f5_cli_undo_confirm_records_origin_cli() {
    let root = root_with_mode("off");
    seed(&root);

    let out = cli(&root, &["undo", TURN, "--confirm"]);
    assert!(
        out.status.success(),
        "undo --confirm failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(
        the_undo_turn(&root)["origin"],
        json!("cli"),
        "a human's `undo --confirm` is the cli surface"
    );
}

// ---- AC-F5.2: an MCP auto-mode execute writes origin "mcp" -----------------

#[test]
fn ac_f5_mcp_execute_records_origin_mcp() {
    let root = root_with_mode("auto");
    seed(&root);

    let token = UndoCoordinator::new(&root)
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
        .expect("an executable auto preview issues a token");

    mcp_call(&root, json!({"action": "execute", "token": token}));

    assert_eq!(
        the_undo_turn(&root)["origin"],
        json!("mcp"),
        "an agent spending its own token is the mcp surface"
    );
}

// ---- AC-F5.3 (the approve call): a CLI approve of an MCP-REQUESTED undo ----

/// `approve` is the one ambiguous surface — the undo was *requested* over MCP
/// and *executed* by a human running a CLI verb. It records `"cli"`, because
/// `origin` names the executing surface and because decision 6's gate counts
/// human-confirmed undos. The requesting provenance is not lost: it is the
/// request ledger's `undo-requests.jsonl` row, which this test also pins so
/// the pairing is demonstrably recoverable rather than merely asserted.
#[test]
fn ac_f5_approve_of_an_mcp_request_records_origin_cli() {
    let root = root_with_mode("confirm");
    seed(&root);

    let request = UndoCoordinator::new(&root)
        .request_human(
            UndoRequest {
                turn: TURN.to_string(),
                paths: None,
                allow_modified: false,
            },
            McpDestructive::Confirm,
        )
        .expect("request lodged")
        .status
        .request;

    let out = cli(&root, &["approve", &request]);
    assert!(
        out.status.success(),
        "approve failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let turn = the_undo_turn(&root);
    assert_eq!(
        turn["origin"],
        json!("cli"),
        "approve executed the writes at a human's keyboard"
    );

    // The MCP provenance the `origin` field deliberately does not carry.
    let ledger = std::fs::read_to_string(root.join(".agentrec/undo-requests.jsonl")).unwrap();
    assert!(
        ledger.contains(turn["id"].as_str().unwrap()),
        "the request ledger must name the undo turn, or approve's MCP \
         provenance is unrecoverable: {ledger}"
    );
}

// ---- AC-F5.4: a pre-F5 log line has no `origin` and reads as cli ------------

/// The legacy shape: every undo turn written before this field existed. It
/// MUST parse, MUST read as `cli`, and MUST re-serialize byte-identically —
/// the last clause is what makes the field additive rather than a rewrite.
#[test]
fn ac_f5_a_pre_f5_undo_turn_without_origin_reads_as_cli() {
    // Verbatim from `docs/fixtures/conformance/valid/turn_undo.jsonl`'s shape:
    // a real undo turn with no `origin` key at all.
    let line = r#"{"type":"turn","v":1,"id":"t_01JXR9Y3Q3TN6J8W0Z2B4E6G8J","grade":"rich","started":"2026-08-05T14:22:01.412Z","ended":"2026-08-05T14:23:47.008Z","tool":"agentrec","session":"01JXSESSION0000000000000000","root":"/Users/dev/repo","files":[]}"#;

    let parsed = agentrec_core::record::parse_log_line(line);
    let agentrec_core::record::ParsedLine::Record(LogRecord::Turn(turn)) = parsed else {
        panic!("a pre-F5 undo turn must parse as a turn, got {parsed:?}");
    };

    assert!(turn.origin.is_none(), "the wire carried no origin");
    assert_eq!(turn.origin(), "cli", "absent reads as cli");

    assert_eq!(
        serde_json::to_string(&LogRecord::Turn(turn)).unwrap(),
        line,
        "an absent origin must round-trip byte-identically"
    );
}
