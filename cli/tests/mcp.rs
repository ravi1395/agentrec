//! End-to-end tests for `agentrec mcp` — the stdio JSON-RPC 2.0 server
//! skeleton (Phase 2 tail, Task E1; delta decision 12: hand-rolled, no SDK,
//! no async runtime). Every test spawns the real binary, writes
//! newline-delimited frames to its stdin, closes stdin, and reads the
//! newline-delimited responses back off stdout — the same transport a host
//! uses, not an in-process shim.
//!
//! The daemon is never started in this file: reads are file-based (P3), and
//! the skeleton touches no daemon state at all.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

/// A tempdir carrying a `.agentrec/` directory — the minimum an `init`ed repo
/// presents to a read-only consumer. Leaked (`keep`) so the path outlives the
/// borrow taken inline by callers.
fn init_root() -> PathBuf {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.keep();
    std::fs::create_dir_all(root.join(".agentrec")).expect("mkdir .agentrec");
    root
}

/// Feed `frames` (each written verbatim, caller supplies newlines) to
/// `agentrec mcp --root <root>` and return the raw process output.
fn mcp_raw(root: &Path, frames: &str) -> Output {
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
        .write_all(frames.as_bytes())
        .expect("write frames");
    child.wait_with_output().expect("wait mcp")
}

/// Drive the server and return one parsed JSON value per stdout line.
/// Asserts a clean (exit 0) shutdown on stdin EOF — decision 12's "graceful
/// shutdown" leg; a server that errors out at EOF would make every other
/// assertion in this file ambiguous.
fn mcp(root: &Path, frames: &str) -> Vec<serde_json::Value> {
    let out = mcp_raw(root, frames);
    assert!(
        out.status.success(),
        "mcp exited {:?}; stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("non-JSON stdout line {l:?}: {e}"))
        })
        .collect()
}

fn initialize_frame(id: u32, revision: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"initialize","params":{{"protocolVersion":"{revision}","capabilities":{{}},"clientInfo":{{"name":"test","version":"0"}}}}}}"#
    )
}

/// The revision the server pins at build time is deliberately NOT hardcoded
/// here: it is a build-time constant that will be bumped as the MCP spec
/// moves, and an integration test cannot import it. Tests assert the *shape*
/// of a negotiated revision instead.
fn is_revision_shaped(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

const READ_TOOLS: [&str; 5] = [
    "agentrec_log",
    "agentrec_diff",
    "agentrec_blame",
    "agentrec_recall",
    "agentrec_status",
];

/// AC-E1's golden transcript: `initialize` → `tools/list` (the 5 read tools
/// under the default `mcp_destructive = off`) → `tools/call` naming a tool
/// that does not exist → a well-formed JSON-RPC error. One response per
/// request, in order, ids echoed.
#[test]
fn ac_e1_golden_transcript_init_list_unknown_tool() {
    let root = init_root();
    // The server must negotiate whatever revision it pins; ask for its own by
    // first reading it out of an initialize with no requested revision.
    let probe = mcp(
        &root,
        &format!(
            "{}\n",
            r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"capabilities":{},"clientInfo":{"name":"p","version":"0"}}}"#
        ),
    );
    let pinned = probe[0]["result"]["protocolVersion"]
        .as_str()
        .expect("negotiated protocolVersion")
        .to_string();
    assert!(
        is_revision_shaped(&pinned),
        "pinned revision must be YYYY-MM-DD shaped, got {pinned:?}"
    );

    let frames = format!(
        "{}\n{}\n{}\n",
        initialize_frame(1, &pinned),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"agentrec_nope","arguments":{}}}"#,
    );
    let responses = mcp(&root, &frames);
    assert_eq!(
        responses.len(),
        3,
        "one response per request, got: {responses:#?}"
    );

    // --- initialize ---
    let init = &responses[0];
    assert_eq!(init["jsonrpc"], "2.0");
    assert_eq!(init["id"], 1);
    assert!(init.get("error").is_none(), "initialize errored: {init}");
    assert_eq!(init["result"]["protocolVersion"], pinned.as_str());
    assert!(
        init["result"]["capabilities"]["tools"].is_object(),
        "server must advertise the tools capability: {init}"
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "agentrec");
    assert!(init["result"]["serverInfo"]["version"].is_string());

    // --- tools/list ---
    let list = &responses[1];
    assert_eq!(list["id"], 2);
    let tools = list["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools array missing: {list}"));
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names, READ_TOOLS,
        "the five PROTOCOL §8 / parent-spec read tools, in table order"
    );
    for tool in tools {
        assert!(
            tool["description"].as_str().is_some_and(|d| !d.is_empty()),
            "tool needs a description: {tool}"
        );
        assert_eq!(
            tool["inputSchema"]["type"], "object",
            "tool needs a JSON-Schema object inputSchema: {tool}"
        );
        // Read-only annotations (parent spec :636; Phase E exit verifies them
        // on every tool). `agentrec_undo` is F1's, and is not listed here.
        assert_eq!(
            tool["annotations"]["readOnlyHint"], true,
            "every 2.2 tool is read-only: {tool}"
        );
        assert_eq!(tool["annotations"]["destructiveHint"], false);
    }

    // --- tools/call, unknown tool ---
    let call = &responses[2];
    assert_eq!(call["id"], 3);
    assert!(
        call.get("result").is_none(),
        "unknown tool must not produce a result: {call}"
    );
    assert_eq!(call["error"]["code"], -32602);
    let msg = call["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("agentrec_nope"),
        "error must name the unknown tool, got {msg:?}"
    );
}

/// AC-E1's second leg: a malformed frame is answered with a parse error and
/// the loop SURVIVES — the follow-up valid frame is still answered.
///
/// The discriminating assertion is the response COUNT plus the second
/// response's id: an implementation that emits the parse error and then exits
/// (or stops reading) still emits line 1, so asserting only "-32700 appears"
/// would pass under exactly the mutation this test exists to catch.
#[test]
fn malformed_frame_errors_and_the_loop_survives() {
    let root = init_root();
    let frames = format!(
        "{}\n{}\n",
        "{ this is not json", r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#,
    );
    let responses = mcp(&root, &frames);
    assert_eq!(
        responses.len(),
        2,
        "parse error THEN the surviving loop's answer, got: {responses:#?}"
    );
    assert_eq!(responses[0]["error"]["code"], -32700);
    assert!(
        responses[0]["id"].is_null(),
        "a frame that never parsed has no id: {}",
        responses[0]
    );
    assert_eq!(responses[1]["id"], 7, "the follow-up request was answered");
    assert!(responses[1]["result"]["tools"].is_array());
}

/// The server answers each request BEFORE stdin closes — a real session, not
/// a batch replay.
///
/// Every other test in this file writes all frames, closes stdin, then reads.
/// A server that buffered its whole output and flushed only at exit passes
/// all of them, and deadlocks with every real MCP host: a host sends
/// `initialize` and waits for the response before sending anything else. This
/// test keeps stdin OPEN, reads one response line, sends a second request on
/// the strength of having received the first, and only then closes.
///
/// Reading happens on a worker thread behind a `recv_timeout`, deliberately:
/// the failure this test exists to catch is a server that never answers, and
/// a bare blocking `read_line` against one HANGS instead of failing — which
/// in CI is a job timeout with no attribution rather than a named red test.
/// Measured: under a buffer-until-exit mutation this assertion fires, while
/// all 13 other tests in this file stay green.
#[test]
fn responses_stream_while_stdin_stays_open() {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::time::Duration;

    /// Generous relative to a local answer (sub-millisecond) and to CI jitter
    /// both; this bound distinguishes "never" from "slow", not milliseconds.
    const ANSWER_TIMEOUT: Duration = Duration::from_secs(20);

    let root = init_root();
    let mut child = Command::new(bin())
        .args(["mcp", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");

    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let read_frame = |what: &str| -> serde_json::Value {
        let line = rx.recv_timeout(ANSWER_TIMEOUT).unwrap_or_else(|_| {
            panic!(
                "no response to {what} within {ANSWER_TIMEOUT:?} while stdin was still open — \
                 the server is not answering until EOF (buffered output?), which deadlocks \
                 every real MCP host"
            )
        });
        serde_json::from_str(&line).expect("JSON response")
    };

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"capabilities":{{}}}}}}"#
    )
    .unwrap();
    stdin.flush().unwrap();
    let init = read_frame("initialize");
    assert_eq!(init["id"], 1);
    assert!(is_revision_shaped(
        init["result"]["protocolVersion"].as_str().unwrap()
    ));

    // Only now — having received the answer, exactly as a host would.
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
    stdin.flush().unwrap();
    let list = read_frame("tools/list");
    assert_eq!(list["id"], 2);
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 5);

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "EOF must be a graceful exit 0");
    reader.join().unwrap();
}

/// A frame that parses but isn't an object (a JSON-RPC batch, a bare scalar)
/// is answered `-32600` rather than met with silence — silence hangs a client
/// that is waiting on a response.
#[test]
fn a_non_object_frame_is_answered_not_ignored() {
    let root = init_root();
    let responses = mcp(
        &root,
        "[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}]\n42\n",
    );
    assert_eq!(responses.len(), 2, "got: {responses:#?}");
    for r in &responses {
        assert_eq!(r["error"]["code"], -32600);
        assert!(r["id"].is_null());
    }
}

/// JSON-RPC notifications (no `id`) MUST NOT be answered. MCP hosts send
/// `notifications/initialized` immediately after the initialize response; a
/// loop that replies `-32601` to it breaks real clients.
#[test]
fn notifications_are_never_answered() {
    let root = init_root();
    let frames = format!(
        "{}\n{}\n{}\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/something_unknown"}"#,
        r#"{"jsonrpc":"2.0","id":9,"method":"tools/list"}"#,
    );
    let responses = mcp(&root, &frames);
    assert_eq!(
        responses.len(),
        1,
        "notifications get no response at all, got: {responses:#?}"
    );
    assert_eq!(responses[0]["id"], 9);
}

/// Blank lines are framing noise, not malformed frames.
#[test]
fn blank_lines_are_skipped_silently() {
    let root = init_root();
    let frames = format!(
        "\n   \n{}\n\n",
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#
    );
    let responses = mcp(&root, &frames);
    assert_eq!(responses.len(), 1, "got: {responses:#?}");
    assert_eq!(responses[0]["id"], 4);
}

#[test]
fn unknown_method_is_method_not_found() {
    let root = init_root();
    let responses = mcp(
        &root,
        &format!(
            "{}\n",
            r#"{"jsonrpc":"2.0","id":5,"method":"resources/list"}"#
        ),
    );
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["error"]["code"], -32601);
}

/// `tools/call` without a `name` is bad params, not a crash.
#[test]
fn tools_call_without_a_name_is_invalid_params() {
    let root = init_root();
    let responses = mcp(
        &root,
        &format!(
            "{}\n",
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"arguments":{}}}"#
        ),
    );
    assert_eq!(responses[0]["error"]["code"], -32602);
}

/// A listed tool whose implementation lands in E3 answers with a clean
/// JSON-RPC error naming it, never a panic or a silent empty result.
///
/// **Amended by E2**, which implemented `agentrec_log`/`agentrec_diff`/
/// `agentrec_blame`: the probe tool moved from `agentrec_log` to
/// `agentrec_recall`, the nearest still-unimplemented listed tool. The
/// assertion is unweakened — same code, same "names the tool" requirement —
/// and the property it guards (a listed-but-unimplemented tool is refused by
/// name, distinctly from one that does not exist) is unchanged. E3 moves it
/// again or retires it.
#[test]
fn listed_but_unimplemented_tool_errors_cleanly() {
    let root = init_root();
    let responses = mcp(
        &root,
        &format!(
            "{}\n",
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"agentrec_recall","arguments":{"query":"x"}}}"#
        ),
    );
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["error"]["code"], -32602);
    let msg = responses[0]["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("agentrec_recall"),
        "error must name the tool, got {msg:?}"
    );
}

/// An `initialize` naming a protocol revision the build does not support is
/// refused cleanly — error, not a silent downgrade — and the refusal names
/// what this build does support.
#[test]
fn initialize_with_an_unsupported_revision_is_refused() {
    let root = init_root();
    let responses = mcp(&root, &format!("{}\n", initialize_frame(1, "1999-01-01")));
    assert_eq!(responses.len(), 1);
    assert!(
        responses[0].get("result").is_none(),
        "an unsupported revision must not negotiate: {}",
        responses[0]
    );
    assert_eq!(responses[0]["error"]["code"], -32602);
    let supported = responses[0]["error"]["data"]["supported"]
        .as_array()
        .unwrap_or_else(|| panic!("refusal must name supported revisions: {}", responses[0]));
    assert!(!supported.is_empty());
    assert!(supported
        .iter()
        .all(|v| is_revision_shaped(v.as_str().unwrap())));
}

/// A non-date-shaped revision (a client speaking something else entirely) is
/// refused by the same path, not coerced.
#[test]
fn initialize_with_a_nonsense_revision_is_refused() {
    let root = init_root();
    let responses = mcp(&root, &format!("{}\n", initialize_frame(1, "2.0")));
    assert_eq!(responses[0]["error"]["code"], -32602);
}

/// Config is loaded ONCE at startup via `config::load`, so an unparseable
/// `config.toml` is the same hard, line-naming error every other CLI verb
/// gives — the process never reaches the request loop.
#[test]
fn invalid_config_toml_is_a_startup_hard_error() {
    let root = init_root();
    std::fs::write(root.join(".agentrec/config.toml"), "ttl_days = [unclosed\n").unwrap();
    let out = mcp_raw(
        &root,
        &format!("{}\n", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#),
    );
    assert_eq!(out.status.code(), Some(1), "must exit nonzero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("line"),
        "the hard error must name a line (D16), got: {stderr}"
    );
    assert!(
        out.stdout.is_empty(),
        "no frames may be answered when startup failed: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A directory that was never `init`ed is a hard startup error naming the
/// remedy, NOT a server that silently answers every read against an empty
/// repo (the O5 silent-wrong-root shape).
#[test]
fn uninitialized_root_is_a_startup_hard_error() {
    let tmp = tempfile::tempdir().unwrap();
    let out = mcp_raw(
        tmp.path(),
        &format!("{}\n", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#),
    );
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("agentrec init"),
        "the error must name the remedy, got: {stderr}"
    );
}

/// No `--root`: the server discovers the repo by walking up from cwd to the
/// nearest ancestor carrying `.agentrec/` — the same rule the emitters use
/// (`discover_agentrec_root`, the O5 fix), not a second discovery rule.
#[test]
fn root_is_discovered_by_walking_up_from_cwd() {
    let root = init_root();
    let deep = root.join("a/b/c");
    std::fs::create_dir_all(&deep).unwrap();
    let mut child = Command::new(bin())
        .arg("mcp")
        .current_dir(&deep)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "discovery from a subdirectory must find the root; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !deep.join(".agentrec").exists(),
        "discovery must not mint a nested .agentrec/ (the O5 defect shape)"
    );
}

// =========================================================================
// Task E2 — `agentrec_log` / `agentrec_diff` / `agentrec_blame`
// =========================================================================

/// The three E2 tools are adapters over `agentrec-core`'s typed views, so
/// these tests seed a REAL store (blobs + `log.jsonl`) and drive the real
/// binary — the same transport and the same on-disk state a host would meet.
mod e2 {
    use super::*;
    use agentrec_core::record::{append_log, FileEntry, LogRecord, TurnRecord};
    use agentrec_core::store::BlobStore;
    use serde_json::Value;

    /// A tempdir carrying a real `.agentrec/` (objects dir included), created
    /// by `agentrec init` itself rather than by hand — `--no-hook`/
    /// `--no-service` so no developer settings file or launchd unit is
    /// touched.
    fn repo() -> PathBuf {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.keep();
        let out = Command::new(bin())
            .args(["init", "--no-hook", "--no-service", "--root"])
            .arg(&root)
            .output()
            .expect("run agentrec init");
        assert!(out.status.success(), "init failed: {out:?}");
        root
    }

    fn turn(id: &str, files: Vec<FileEntry>) -> TurnRecord {
        TurnRecord {
            v: 1,
            id: id.to_string(),
            grade: "rich".into(),
            truncated: false,
            started: "2026-08-05T00:00:00.000Z".into(),
            ended: "2026-08-05T00:00:01.000Z".into(),
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

    fn entry(path: &str, before: Option<String>, after: Option<String>) -> FileEntry {
        FileEntry {
            path: path.into(),
            before,
            after,
            op: "modify".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
            link_kind: None,
            attribution: None,
        }
    }

    fn seed(root: &Path, t: &TurnRecord) {
        append_log(
            &root.join(".agentrec/log.jsonl"),
            &LogRecord::Turn(t.clone()),
        )
        .expect("append turn");
    }

    /// Twenty-eight characters after `t_`, matching the recorded id shape.
    fn turn_id(n: usize) -> String {
        format!("t_{n:026}E2")
    }

    /// One `tools/call`, driven through a fresh server process. Returns the
    /// whole JSON-RPC response.
    fn call(root: &Path, tool: &str, args: Value) -> Value {
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {"name": tool, "arguments": args},
        });
        let responses = mcp(root, &format!("{frame}\n"));
        assert_eq!(responses.len(), 1, "one response per call: {responses:#?}");
        responses.into_iter().next().unwrap()
    }

    /// The `text` content block of a successful call. Panics (loudly, with the
    /// payload) on `isError` — a test that silently accepted an error result
    /// would assert nothing.
    fn text(root: &Path, tool: &str, args: Value) -> String {
        let resp = call(root, tool, args);
        assert!(resp.get("error").is_none(), "JSON-RPC error: {resp}");
        assert_eq!(
            resp["result"]["isError"], false,
            "tool reported a domain failure: {resp}"
        );
        let content = resp["result"]["content"]
            .as_array()
            .unwrap_or_else(|| panic!("no content array: {resp}"));
        assert_eq!(content.len(), 1, "one text block: {resp}");
        assert_eq!(content[0]["type"], "text");
        content[0]["text"]
            .as_str()
            .expect("text string")
            .to_string()
    }

    /// The error code an `isError: true` result carries in its payload.
    fn error_code(root: &Path, tool: &str, args: Value) -> String {
        let resp = call(root, tool, args);
        assert_eq!(
            resp["result"]["isError"], true,
            "expected a domain failure: {resp}"
        );
        let payload: Value =
            serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap())
                .expect("error payload is JSON");
        payload["error"].as_str().expect("error code").to_string()
    }

    /// `agentrec <args> --root <root>` stdout, asserted exit 0. The CLI's
    /// `println!` adds a trailing newline that the MCP payload does not carry;
    /// it is stripped EXPLICITLY (not trimmed) so a payload that grows a stray
    /// newline of its own still reds.
    fn cli_json(root: &Path, args: &[&str]) -> String {
        let out = Command::new(bin())
            .args(args)
            .args(["--root", root.to_str().unwrap()])
            .output()
            .expect("run agentrec");
        assert!(out.status.success(), "cli failed: {out:?}");
        let stdout = String::from_utf8(out.stdout).expect("utf8");
        stdout
            .strip_suffix('\n')
            .expect("cli --json output ends in exactly one newline")
            .to_string()
    }

    /// AC-E2 parity (diff): the MCP `text` payload is byte-for-byte the
    /// `diff --json` stdout on the same store. Both sides serialize the SAME
    /// `view::DiffResult` with the same `serde_json::to_string`; nothing in
    /// `cli/src` re-renders it. The fixture is deliberately far under the
    /// payload bound, so nothing truncates and parity is the whole claim.
    #[test]
    fn ac_e2_diff_text_payload_byte_equals_diff_json() {
        let root = repo();
        let store = BlobStore::new(root.join(".agentrec/objects"));
        let before = store.put(b"a\nb\nc\n").unwrap();
        let after = store.put(b"a\nB\nc\n").unwrap();
        let t = turn(
            &turn_id(1),
            vec![entry("src/x.rs", Some(before), Some(after))],
        );
        seed(&root, &t);

        let mcp_payload = text(&root, "agentrec_diff", serde_json::json!({"turn": t.id}));
        let cli_payload = cli_json(&root, &["diff", &t.id, "--json"]);
        assert_eq!(
            mcp_payload, cli_payload,
            "MCP diff payload must be the --json bytes, not a second rendering"
        );
        // …and it really is the DiffResult shape, so parity is not two
        // matching empty strings.
        assert!(
            mcp_payload.contains("\"turn_id\"") && mcp_payload.contains("\"total_files\""),
            "payload is not a DiffResult: {mcp_payload}"
        );
    }

    /// AC-E2 parity (blame), including the gap-honesty leg: an uncovered path
    /// serializes with a `recording_gap` state tag and NO `turn` field, and
    /// those exact bytes are what the agent receives.
    #[test]
    fn ac_e2_blame_text_payload_byte_equals_blame_json() {
        let root = repo();
        let store = BlobStore::new(root.join(".agentrec/objects"));
        let blob = store.put(b"one\n").unwrap();
        let t = turn(&turn_id(2), vec![entry("src/y.rs", None, Some(blob))]);
        seed(&root, &t);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/y.rs"), b"one\n").unwrap();

        let mcp_payload = text(
            &root,
            "agentrec_blame",
            serde_json::json!({"path": "src/y.rs"}),
        );
        let cli_payload = cli_json(&root, &["blame", "src/y.rs", "--json"]);
        assert_eq!(
            mcp_payload, cli_payload,
            "MCP blame payload must be the --json bytes"
        );
        assert!(
            mcp_payload.contains("\"state\""),
            "payload is not a BlameResult: {mcp_payload}"
        );

        // Gap honesty reaches the client: a crash epoch plus an untouched
        // path is the `no_turn_recording_gap` arm, which carries no attributor
        // at all. Same bytes on both channels.
        append_log(
            &root.join(".agentrec/log.jsonl"),
            &LogRecord::Epoch(agentrec_core::record::EpochRecord {
                v: 1,
                event: "start".into(),
                ts: "2026-08-05T01:00:00.000Z".into(),
                dropped_signals: 0,
            }),
        )
        .unwrap();
        append_log(
            &root.join(".agentrec/log.jsonl"),
            &LogRecord::Epoch(agentrec_core::record::EpochRecord {
                v: 1,
                event: "start".into(),
                ts: "2026-08-05T02:00:00.000Z".into(),
                dropped_signals: 0,
            }),
        )
        .unwrap();
        let gap_mcp = text(
            &root,
            "agentrec_blame",
            serde_json::json!({"path": "src/untouched.rs"}),
        );
        let gap_cli = cli_json(&root, &["blame", "src/untouched.rs", "--json"]);
        assert_eq!(gap_mcp, gap_cli);
        let parsed: Value = serde_json::from_str(&gap_mcp).unwrap();
        assert_eq!(
            parsed["state"]["type"], "no_turn_recording_gap",
            "recording-gap honesty must reach the MCP client: {gap_mcp}"
        );
        assert!(
            parsed["state"].get("turn").is_none(),
            "a gap state must name no attributor: {gap_mcp}"
        );
    }

    /// AC-E2 shape: `agentrec_log` returns `Page<TurnSummary>` (P4b decision
    /// 12), NOT `log --json`'s protocol JSONL. Asserted by exact key set at
    /// both levels, so a field added to either struct shows up here.
    #[test]
    fn ac_e2_log_payload_is_page_of_turn_summary() {
        let root = repo();
        let t = turn(&turn_id(3), vec![]);
        seed(&root, &t);

        let payload = text(&root, "agentrec_log", serde_json::json!({}));
        let v: Value = serde_json::from_str(&payload).expect("payload is JSON");
        let top: std::collections::BTreeSet<&str> =
            v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            top,
            ["items", "next"].into_iter().collect(),
            "Page<T> shape: {payload}"
        );
        assert!(v["next"].is_null(), "one page, no continuation: {payload}");
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        let summary: std::collections::BTreeSet<&str> = items[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            summary,
            [
                "id",
                "grade",
                "tool",
                "started",
                "ended",
                "file_count",
                "imported"
            ]
            .into_iter()
            .collect(),
            "TurnSummary shape: {payload}"
        );
        assert_eq!(items[0]["id"], t.id);
        assert_eq!(items[0]["grade"], "rich");
        // Not the protocol record: a `TurnRecord` would carry these.
        assert!(items[0].get("v").is_none() && items[0].get("files").is_none());
    }

    /// AC-E2 cursor: a PURE APPEND between two pages re-delivers nothing.
    ///
    /// The ledger deliberately holds a DUPLICATE id (the pre-fix daemon's
    /// orphan-recovery shape `Cursor::after_occurrence` exists for), and the
    /// first page ends on the SECOND occurrence. A cursor round-trip that
    /// dropped `after_occurrence` would resolve to the first occurrence and
    /// re-deliver two already-seen turns — so this also pins the ordinal
    /// passing through the MCP boundary, not just the view's own semantics.
    ///
    /// Two separate server processes, with the append in between: the cursor
    /// must survive a restart, which is strictly stronger than surviving a
    /// session.
    #[test]
    fn ac_e2_cursor_pure_append_does_not_redeliver() {
        let root = repo();
        let dup = turn_id(10);
        seed(&root, &turn(&dup, vec![])); // occurrence 0
        seed(&root, &turn(&turn_id(11), vec![]));
        seed(&root, &turn(&dup, vec![])); // occurrence 1
        seed(&root, &turn(&turn_id(12), vec![]));

        let first: Value = serde_json::from_str(&text(
            &root,
            "agentrec_log",
            serde_json::json!({"limit": 3}),
        ))
        .unwrap();
        let ids: Vec<&str> = first["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec![dup.as_str(), turn_id(11).as_str(), dup.as_str()]);
        let cursor = first["next"].clone();
        assert!(!cursor.is_null(), "a bounded page must continue: {first}");
        assert_eq!(
            cursor["after_occurrence"], 1,
            "the page ended on the second occurrence of {dup}: {cursor}"
        );

        // Pure append — nothing rewritten.
        seed(&root, &turn(&turn_id(13), vec![]));

        let second: Value = serde_json::from_str(&text(
            &root,
            "agentrec_log",
            serde_json::json!({"cursor": cursor}),
        ))
        .unwrap();
        let ids: Vec<&str> = second["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![turn_id(12).as_str(), turn_id(13).as_str()],
            "only the unseen turns come back; a dropped `after_occurrence` would \
             resolve to occurrence 0 and re-deliver two already-delivered turns: {second}"
        );
        assert!(second["next"].is_null());
    }

    /// AC-E2 cursor: a LEDGER REWRITE (the `purge --log-duplicates` shape —
    /// the record the cursor named is gone) surfaces as `stale_cursor` the
    /// agent can read, never as a page with a silent hole.
    #[test]
    fn ac_e2_ledger_rewrite_yields_stale_cursor() {
        let root = repo();
        seed(&root, &turn(&turn_id(20), vec![]));
        seed(&root, &turn(&turn_id(21), vec![]));

        let first: Value = serde_json::from_str(&text(
            &root,
            "agentrec_log",
            serde_json::json!({"limit": 1}),
        ))
        .unwrap();
        let cursor = first["next"].clone();
        assert_eq!(cursor["after_id"], turn_id(20));

        // Rewrite: drop the record the cursor named.
        let log = root.join(".agentrec/log.jsonl");
        let kept: String = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .filter(|l| !l.contains(&turn_id(20)))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(&log, kept).unwrap();

        assert_eq!(
            error_code(&root, "agentrec_log", serde_json::json!({"cursor": cursor})),
            "stale_cursor"
        );
    }

    /// AC-E2 bounds: the 201st turn paginates. An unbounded request is capped
    /// at the server's 200 summaries and hands back a cursor that yields the
    /// remainder — the cap is the server's, not the client's to raise.
    #[test]
    fn ac_e2_201st_turn_paginates() {
        let root = repo();
        for n in 0..201 {
            seed(&root, &turn(&turn_id(100 + n), vec![]));
        }

        let first: Value =
            serde_json::from_str(&text(&root, "agentrec_log", serde_json::json!({}))).unwrap();
        assert_eq!(
            first["items"].as_array().unwrap().len(),
            200,
            "server bound is 200 summaries"
        );
        let cursor = first["next"].clone();
        assert!(!cursor.is_null(), "201 turns must continue past page 1");

        // A client asking for more than the bound still gets the bound.
        let greedy: Value = serde_json::from_str(&text(
            &root,
            "agentrec_log",
            serde_json::json!({"limit": 5000}),
        ))
        .unwrap();
        assert_eq!(greedy["items"].as_array().unwrap().len(), 200);

        let second: Value = serde_json::from_str(&text(
            &root,
            "agentrec_log",
            serde_json::json!({"cursor": cursor}),
        ))
        .unwrap();
        assert_eq!(second["items"].as_array().unwrap().len(), 1);
        assert_eq!(second["items"][0]["id"], turn_id(300));
        assert!(second["next"].is_null());
    }

    /// The `agentrec_diff` payload bound binds on a diff too big for one
    /// page, and the continuation is minted by the view's own walk (so it
    /// resolves) rather than by trimming the result here.
    #[test]
    fn diff_payload_bound_pages_a_large_turn() {
        let root = repo();
        let store = BlobStore::new(root.join(".agentrec/objects"));
        // Three files, ~1,200 payload lines each: two fit the 2,000-line
        // bound only one at a time.
        let body: String = std::iter::repeat_n("line\n", 1_200).collect();
        let files: Vec<FileEntry> = (0..3)
            .map(|i| {
                let blob = store.put(body.as_bytes()).unwrap();
                entry(&format!("f{i}.txt"), None, Some(blob))
            })
            .collect();
        let t = turn(&turn_id(30), files);
        seed(&root, &t);

        let page1: Value = serde_json::from_str(&text(
            &root,
            "agentrec_diff",
            serde_json::json!({"turn": t.id}),
        ))
        .unwrap();
        assert_eq!(page1["total_files"], 3, "the header reports the whole turn");
        assert_eq!(
            page1["files"]["items"].as_array().unwrap().len(),
            1,
            "one 1,200-line file at a time under the 2,000-line bound: {page1}"
        );
        let cursor = page1["files"]["next"].clone();
        assert!(!cursor.is_null(), "a bounded diff must continue: {page1}");

        let page2: Value = serde_json::from_str(&text(
            &root,
            "agentrec_diff",
            serde_json::json!({"turn": t.id, "cursor": cursor}),
        ))
        .unwrap();
        assert_eq!(page2["files"]["items"][0]["path"], "f1.txt");
    }

    /// Protocol-shape failures stay on the JSON-RPC channel (`-32602`),
    /// distinctly from domain outcomes, which ride `isError` — the split
    /// `tools_call` documents.
    #[test]
    fn malformed_arguments_are_invalid_params_not_tool_errors() {
        let root = repo();
        // Required argument missing.
        let resp = call(&root, "agentrec_diff", serde_json::json!({}));
        assert_eq!(resp["error"]["code"], -32602, "{resp}");
        // Wrong type.
        let resp = call(&root, "agentrec_blame", serde_json::json!({"path": 5}));
        assert_eq!(resp["error"]["code"], -32602, "{resp}");
        // Out-of-range limit (schema says minimum 1).
        let resp = call(&root, "agentrec_log", serde_json::json!({"limit": 0}));
        assert_eq!(resp["error"]["code"], -32602, "{resp}");
        // A cursor that is not one.
        let resp = call(&root, "agentrec_log", serde_json::json!({"cursor": "nope"}));
        assert_eq!(resp["error"]["code"], -32602, "{resp}");
        // A turn that does not exist is a DOMAIN outcome, not a bad frame.
        assert_eq!(
            error_code(
                &root,
                "agentrec_diff",
                serde_json::json!({"turn": "t_NOSUCHTURN"})
            ),
            "turn_not_found"
        );
    }
}
