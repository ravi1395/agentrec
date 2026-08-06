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

/// A listed tool whose implementation lands in E2/E3 answers with a clean
/// JSON-RPC error naming it, never a panic or a silent empty result.
#[test]
fn listed_but_unimplemented_tool_errors_cleanly() {
    let root = init_root();
    let responses = mcp(
        &root,
        &format!(
            "{}\n",
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"agentrec_log","arguments":{}}}"#
        ),
    );
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["error"]["code"], -32602);
    let msg = responses[0]["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("agentrec_log"),
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
