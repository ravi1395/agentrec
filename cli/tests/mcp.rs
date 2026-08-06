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

// `listed_but_unimplemented_tool_errors_cleanly` lived here through E1 and E2
// and is **RETIRED by E3**, which implemented the last two listed tools
// (`agentrec_recall`, `agentrec_status`). It asserted that a tool present in
// `tools/list` but not yet dispatched is refused by name; with all five read
// tools answering there is no such tool left to probe, and E2's own amendment
// note ("E3 moves it again or retires it") anticipated exactly this. The
// production arm it covered is deliberately KEPT (see `mcpcmd::tools_call`)
// because F1 re-opens the window when `agentrec_undo` joins the listed set;
// the test comes back with it, against `agentrec_undo`, rather than being
// re-pointed at a tool that now works. Unknown-tool refusal — the neighboring
// property — is still pinned, by `ac_e1_golden_transcript_init_list_unknown_tool`.

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
    pub(super) fn repo() -> PathBuf {
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

    pub(super) fn turn(id: &str, files: Vec<FileEntry>) -> TurnRecord {
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

    pub(super) fn entry(path: &str, before: Option<String>, after: Option<String>) -> FileEntry {
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

    pub(super) fn seed(root: &Path, t: &TurnRecord) {
        append_log(
            &root.join(".agentrec/log.jsonl"),
            &LogRecord::Turn(t.clone()),
        )
        .expect("append turn");
    }

    /// Twenty-eight characters after `t_`, matching the recorded id shape.
    pub(super) fn turn_id(n: usize) -> String {
        format!("t_{n:026}E2")
    }

    /// One `tools/call`, driven through a fresh server process. Returns the
    /// whole JSON-RPC response.
    pub(super) fn call(root: &Path, tool: &str, args: Value) -> Value {
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
    pub(super) fn text(root: &Path, tool: &str, args: Value) -> String {
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
    pub(super) fn error_code(root: &Path, tool: &str, args: Value) -> String {
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
    pub(super) fn cli_json(root: &Path, args: &[&str]) -> String {
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

// =========================================================================
// Task E3 — `agentrec_recall` / `agentrec_status`, plus E2's carried residual
// =========================================================================

/// `agentrec_recall` is the one 2.2 tool with NO `--json` byte-parity to pin
/// (delta decision 14: the CLI serializes `page.items` alone, the MCP tool
/// returns the whole `RecallPage`), and `agentrec_status` composes typed
/// values that no single CLI verb emits together. Both are driven here
/// against a real store through the real binary, same as E2.
mod e3 {
    use super::e2::{cli_json, entry, error_code, repo, seed, text, turn, turn_id};
    use super::*;
    use agentrec_core::record::{append_log, EpochRecord, LogRecord};
    use agentrec_core::store::BlobStore;
    use serde_json::Value;

    /// `agentrec <args> --root <root>`, asserted exit 0, stdout returned raw.
    fn cli(root: &Path, args: &[&str]) -> String {
        let out = Command::new(bin())
            .args(args)
            .args(["--root", root.to_str().unwrap()])
            .output()
            .expect("run agentrec");
        assert!(out.status.success(), "cli failed: {out:?}");
        String::from_utf8(out.stdout).expect("utf8")
    }

    /// Two memories whose BM25 scores are known to clear `SCORE_FLOOR` on a
    /// two-document corpus — the same shape `cli/tests/golden.rs` documents at
    /// length (a single-occurrence match cannot clear the floor at
    /// `dl ≈ avg_dl`; the doubled `throttle` is what carries it). Written by
    /// the real `remember` verb, so the store is one production wrote.
    ///
    /// Do NOT tidy the fact strings: rewording silently turns every recall in
    /// this module into an empty page, and an empty page passes a shape test
    /// that only checks keys.
    fn seed_memories(root: &Path) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/throttle.rs"), b"fn throttle() {}\n").unwrap();
        std::fs::write(root.join("src/changelog.rs"), b"fn changelog() {}\n").unwrap();
        cli(
            root,
            &[
                "remember",
                "throttle limiter guards the API from bursty traffic via throttle checks",
                "--from",
                "src/throttle.rs",
            ],
        );
        cli(
            root,
            &[
                "remember",
                "the release changelog script lives under scripts",
                "--from",
                "src/changelog.rs",
            ],
        );
    }

    /// AC-E3 (recall shape): the payload is the WHOLE `RecallPage` — the page
    /// plus all three honesty flags (delta decision 14) — asserted by exact
    /// key set at both levels, so a flag dropped or renamed reds here.
    ///
    /// The hits half is pinned by containment against `recall --json`: the
    /// CLI emits exactly the `page.items` array, so those bytes must appear
    /// verbatim inside the MCP payload. That is the discriminating half —
    /// without it an implementation returning `items: []` beside three
    /// correct flags would pass.
    #[test]
    fn ac_e3_recall_payload_is_the_whole_recall_page() {
        let root = repo();
        seed_memories(&root);

        let payload = text(
            &root,
            "agentrec_recall",
            serde_json::json!({"query": "throttle"}),
        );
        let v: Value = serde_json::from_str(&payload).expect("payload is JSON");
        let top: std::collections::BTreeSet<&str> =
            v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            top,
            ["page", "capped", "store_corrupt", "store_empty"]
                .into_iter()
                .collect(),
            "RecallPage shape — all three flags reach the agent: {payload}"
        );
        let page: std::collections::BTreeSet<&str> = v["page"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(page, ["items", "next"].into_iter().collect());
        assert_eq!(v["capped"], false);
        assert_eq!(v["store_corrupt"], false);
        assert_eq!(v["store_empty"], false);

        let items = v["page"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "one fresh match expected: {payload}");
        let hit: std::collections::BTreeSet<&str> = items[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            hit,
            [
                "id",
                "fact",
                "pins",
                "origin",
                "ts",
                "retracted",
                "freshness"
            ]
            .into_iter()
            .collect(),
            "MemoryHit shape: {payload}"
        );
        assert_eq!(items[0]["freshness"], "fresh");

        // The hits are the SAME bytes `recall --json` prints — the CLI's whole
        // output is `page.items`, so it must appear verbatim inside the page.
        let cli_items = cli_json(&root, &["recall", "throttle", "--json"]);
        assert!(
            payload.contains(&cli_items),
            "MCP hits must be the --json item bytes.\n  mcp: {payload}\n  cli: {cli_items}"
        );
    }

    /// AC-E3 (corrupt store): a malformed non-empty `memory.jsonl` reaches the
    /// agent as `store_corrupt: true` rather than as an innocent empty page.
    ///
    /// The corruption shape is copied from the producer, not invented: the
    /// non-UTF8 line below is what `memory::load_effective_checked` sets the
    /// flag on (it is also the fixture `integration.rs`'s
    /// `hook_corrupt_memory_store_is_counted` uses for AC-F10.2a).
    ///
    /// The second half is the one that matters: the SAME store answers
    /// `recall --json` with a bare `[]`, indistinguishable from "no memories
    /// match". That asymmetry is decision 14's entire justification, so it is
    /// asserted rather than described.
    #[test]
    fn ac_e3_corrupt_store_reaches_the_agent() {
        let root = repo();
        std::fs::write(
            root.join(".agentrec/memory.jsonl"),
            b"\xff\xfenot json at all garbage bytes\x00\x01\n",
        )
        .unwrap();

        let payload = text(
            &root,
            "agentrec_recall",
            serde_json::json!({"query": "throttle"}),
        );
        let v: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            v["store_corrupt"], true,
            "a corrupt store must not read to an agent as 'no matches': {payload}"
        );
        assert_eq!(v["page"]["items"].as_array().unwrap().len(), 0);

        assert_eq!(
            cli_json(&root, &["recall", "throttle", "--json"]),
            "[]",
            "the CLI channel cannot express this, which is why the MCP tool carries the flag"
        );

        // The CONTROL, and the reason this test is not one-sided: a store that
        // is merely empty must NOT set the flag. (`store_empty` is true in
        // both cases and is measured here rather than asserted away — a
        // wholly-corrupt store folds to zero effective memories under
        // `load_effective`'s tolerant read, which is what `store_empty` asks.
        // `store_corrupt` is the bit that separates them, which is exactly why
        // decision 14 puts it on the wire.)
        assert_eq!(
            v["store_empty"], true,
            "measured, not asserted-away: {payload}"
        );
        let empty = repo();
        let payload = text(
            &empty,
            "agentrec_recall",
            serde_json::json!({"query": "throttle"}),
        );
        let v: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            v["store_corrupt"], false,
            "an empty store is not a corrupt one: {payload}"
        );
        assert_eq!(v["store_empty"], true, "{payload}");
    }

    /// Malformed recall calls stay on the JSON-RPC channel; a bad cursor is a
    /// domain outcome the agent can act on.
    #[test]
    fn recall_argument_and_cursor_failures_use_the_right_channel() {
        let root = repo();
        seed_memories(&root);
        // `query` is required.
        let resp = super::e2::call(&root, "agentrec_recall", serde_json::json!({}));
        assert_eq!(resp["error"]["code"], -32602, "{resp}");
        // `k` honors the schema's `minimum: 1`.
        let resp = super::e2::call(
            &root,
            "agentrec_recall",
            serde_json::json!({"query": "throttle", "k": 0}),
        );
        assert_eq!(resp["error"]["code"], -32602, "{resp}");
        // A cursor minted against a different query is a domain refusal.
        let cursor = serde_json::json!({
            "after_id": "NOSUCHMEMORY00000000000000",
            "after_occurrence": 0,
            "query": "recall:query=something else",
        });
        assert_eq!(
            error_code(
                &root,
                "agentrec_recall",
                serde_json::json!({"query": "throttle", "cursor": cursor})
            ),
            "query_mismatch"
        );
    }

    /// AC-E3 (recall must not leak into the CLI): `recall --json` is still a
    /// BARE ARRAY of hits, never the page object.
    ///
    /// `RecallPage` was unserializable precisely so this could not regress;
    /// E3 removed that structural guard (delta decision 14 needs the derive).
    /// The four `recall_json_*.golden` files are the primary replacement — this
    /// test is the named, local one, so the reason a leak is forbidden sits
    /// next to the assertion instead of only in a captured byte-file.
    #[test]
    fn ac_e3_recall_json_is_still_a_bare_hit_array() {
        let root = repo();
        seed_memories(&root);
        let out = cli_json(&root, &["recall", "throttle", "--json"]);
        let v: Value = serde_json::from_str(&out).expect("recall --json is JSON");
        assert!(
            v.is_array(),
            "recall --json must stay `page.items`, never the RecallPage object: {out}"
        );
        for key in ["page", "capped", "store_corrupt", "store_empty"] {
            assert!(
                !out.contains(key),
                "decision 3: `{key}` must never reach the CLI --json surface: {out}"
            );
        }
    }

    /// AC-E3 (status): the payload composes the typed values, and a config
    /// change on disk is reported as RESTART-REQUIRED rather than hot-reloaded.
    ///
    /// Driven over ONE long-lived session with stdin held open, deliberately:
    /// the property under test is "changed since this process started", which
    /// a fresh process per call cannot express at all. The first status call
    /// (before any edit) is the control — without it, an implementation
    /// hardcoding `restart_required: true` would pass.
    ///
    /// The discriminating assertion on the second call is NOT `restart_required`
    /// alone — it is that `agent_undo_mode` is STILL `"off"` while the file on
    /// disk says `"auto"`. An implementation that reloaded config would report
    /// `"auto"` there and satisfy a one-sided test.
    #[test]
    fn ac_e3_status_reports_a_config_change_as_restart_required_and_never_reloads() {
        use std::io::{BufRead, BufReader};

        let root = repo();
        let config = root.join(".agentrec/config.toml");
        std::fs::write(&config, "mcp_destructive = \"off\"\n").unwrap();

        let mut child = Command::new(bin())
            .args(["mcp", "--root", root.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut send_and_read = |frame: &str, stdin: &mut std::process::ChildStdin| -> Value {
            writeln!(stdin, "{frame}").unwrap();
            stdin.flush().unwrap();
            let mut line = String::new();
            stdout.read_line(&mut line).expect("response line");
            serde_json::from_str(&line).expect("JSON response")
        };
        let status_frame = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"agentrec_status","arguments":{}}}"#;
        let payload = |resp: &Value| -> Value {
            assert_eq!(resp["result"]["isError"], false, "status errored: {resp}");
            serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap())
                .expect("status payload is JSON")
        };

        // Handshake first, so `protocol_revision` has something to report.
        let init = send_and_read(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
            &mut stdin,
        );
        let revision = init["result"]["protocolVersion"]
            .as_str()
            .unwrap()
            .to_string();

        // --- control: nothing has changed yet ---
        let before = payload(&send_and_read(status_frame, &mut stdin));
        let top: std::collections::BTreeSet<&str> = before
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            top,
            [
                "health",
                "daemon_running",
                "agent_undo_mode",
                "protocol_revision",
                "restart_required",
                "note",
            ]
            .into_iter()
            .collect(),
            "status payload shape: {before}"
        );
        assert_eq!(before["agent_undo_mode"], "off");
        assert_eq!(before["protocol_revision"], revision.as_str());
        assert_eq!(before["daemon_running"], false, "no daemon was started");
        assert_eq!(before["restart_required"], false, "nothing changed yet");
        assert!(before["note"].is_null());
        // Composed from `RepositoryHealth`, not re-derived here.
        for key in ["store_bytes", "budgeted_bytes", "budget", "turn_count"] {
            assert!(
                before["health"].get(key).is_some(),
                "health must be the RepositoryHealth struct, missing {key}: {before}"
            );
        }

        // --- the edit: agent-undo turned on, on disk, mid-session ---
        // Slept before rewriting so the new mtime is distinguishable from the
        // startup stamp on a filesystem with coarse timestamp granularity;
        // nothing about the property under test depends on the delay.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&config, "mcp_destructive = \"auto\"\n").unwrap();

        let after = payload(&send_and_read(status_frame, &mut stdin));
        assert_eq!(
            after["restart_required"], true,
            "a config change since startup must be reported: {after}"
        );
        assert_eq!(
            after["agent_undo_mode"], "off",
            "config MUST NOT hot-reload — the file says \"auto\" and the effective mode is \
             still the startup value: {after}"
        );
        assert!(
            after["note"]
                .as_str()
                .is_some_and(|n| n.contains("Restart")),
            "restart_required must carry a note saying so: {after}"
        );

        drop(stdin);
        assert!(child.wait().unwrap().success(), "EOF is a graceful exit 0");

        // A NEW process reads the new file, which is what "restart" means.
        let fresh = text(&root, "agentrec_status", serde_json::json!({}));
        let fresh: Value = serde_json::from_str(&fresh).unwrap();
        assert_eq!(fresh["agent_undo_mode"], "auto");
        assert_eq!(fresh["restart_required"], false);
    }

    /// `agentrec_status` is annotated `readOnlyHint: true`, so it must not
    /// touch the filesystem. `view::health()` is a pure read (P4), and the
    /// liveness probe deliberately opens `.agentrec/daemon.lock` WITHOUT
    /// `.create(true)` (`daemon::daemon_is_running`) — but "reads a lock file"
    /// is exactly the kind of claim this repo has been burned asserting from a
    /// comment, so it is measured: file set AND mtimes, before and after, on a
    /// repo where the daemon has never run (the only case in which a probe
    /// could create the lock file).
    #[test]
    fn ac_e3_status_call_writes_nothing() {
        let root = repo();
        let snapshot = |root: &Path| -> Vec<(PathBuf, std::time::SystemTime)> {
            let mut out = Vec::new();
            let mut stack = vec![root.join(".agentrec")];
            while let Some(dir) = stack.pop() {
                for e in std::fs::read_dir(&dir).unwrap() {
                    let e = e.unwrap();
                    let md = e.metadata().unwrap();
                    if md.is_dir() {
                        stack.push(e.path());
                    } else {
                        out.push((e.path(), md.modified().unwrap()));
                    }
                }
            }
            out.sort();
            out
        };
        let before = snapshot(&root);
        assert!(
            !root.join(".agentrec/daemon.lock").exists(),
            "precondition: the daemon has never run in this fixture"
        );

        let payload = text(&root, "agentrec_status", serde_json::json!({}));
        assert!(payload.contains("\"daemon_running\":false"), "{payload}");

        assert_eq!(
            snapshot(&root),
            before,
            "a readOnlyHint tool must leave .agentrec/ byte-for-byte alone"
        );
        assert!(
            !root.join(".agentrec/daemon.lock").exists(),
            "the liveness probe must not CREATE the lock file it probes"
        );
    }

    /// Wiring proof for the `cursor` argument on `agentrec_recall`, plus the
    /// recorded limitation that makes it awkward. E3 is the first caller
    /// anywhere in this repo to set `view::RecallQuery::after`.
    ///
    /// Half one, RECORDED NOT FIXED: a first page reports `next: null` even
    /// with hits left behind, because `view::recall` mints a continuation only
    /// on an already-cursored call. If a later round changes that, this
    /// assertion reds — and the tool metadata that currently tells clients so
    /// (`mcpcmd::READ_TOOLS`, `agentrec_recall`) must be updated with it.
    ///
    /// Half two: a caller-built cursor round-trips through the adapter and
    /// yields the NEXT hit, not the first one again. Without this the `after`
    /// wiring would be exercised by nothing but its rejection paths.
    ///
    /// Ten memories, deliberately: `bm25_rank`'s `SCORE_FLOOR` is 0.8 and the
    /// idf available on a small corpus is `ln(N/matches)`, so a 2- or 3-doc
    /// fixture scores every hit below the floor and recall returns `[]` — a
    /// green-looking test asserting nothing. Measured: at N=3 this fixture
    /// yields 0 hits, at N=10 it yields 2.
    #[test]
    fn recall_cursor_wiring_round_trips_a_caller_built_cursor() {
        let root = repo();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let remember = |fact: &str, path: &str| {
            std::fs::write(root.join(path), b"fn f() {}\n").unwrap();
            cli(&root, &["remember", fact, "--from", path]);
        };
        remember(
            "throttle limiter guards the API from bursty traffic via throttle checks",
            "src/f1.rs",
        );
        remember(
            "throttle budget resets hourly and the throttle counter is per-key",
            "src/f2.rs",
        );
        for i in 3..=10 {
            remember(
                &format!("unrelated note number {i} about changelog scripts and release notes"),
                &format!("src/f{i}.rs"),
            );
        }

        let page1: Value = serde_json::from_str(&text(
            &root,
            "agentrec_recall",
            serde_json::json!({"query": "throttle", "k": 1}),
        ))
        .unwrap();
        let items = page1["page"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "k=1 bounds the page: {page1}");
        let first = items[0]["id"].as_str().unwrap().to_string();
        assert!(
            page1["page"]["next"].is_null(),
            "RECORDED LIMITATION: a first recall page never mints a cursor, even with a              second hit waiting. If this now reds, `view::recall` gained first-page              continuations — update the agentrec_recall tool metadata, which tells clients              the opposite: {page1}"
        );

        // …and the second hit really is there, so the null above is a
        // limitation and not an empty corpus.
        let all: Value = serde_json::from_str(&text(
            &root,
            "agentrec_recall",
            serde_json::json!({"query": "throttle"}),
        ))
        .unwrap();
        assert_eq!(all["page"]["items"].as_array().unwrap().len(), 2);

        // A cursor the caller built: the fingerprint is `recall:query=<query>`
        // (`view::RecallQuery::fingerprint`), which a mismatching value would
        // reject with `query_mismatch` — already covered above.
        let page2: Value = serde_json::from_str(&text(
            &root,
            "agentrec_recall",
            serde_json::json!({
                "query": "throttle",
                "k": 1,
                "cursor": {
                    "after_id": first,
                    "after_occurrence": 0,
                    "query": "recall:query=throttle",
                },
            }),
        ))
        .unwrap();
        let items = page2["page"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{page2}");
        assert_ne!(
            items[0]["id"].as_str().unwrap(),
            first,
            "a cursor that resolved must advance past its own hit: {page2}"
        );
    }

    /// E2 residual 2: the `BlameState::File { gap_stale: true }` arm had no
    /// MCP-specific test. E2's blame test covers `no_turn_recording_gap` — a
    /// DIFFERENT arm, reached when no turn touches the path at all.
    ///
    /// `gap_stale` is `has_gap_after(records, turn.ended) && modified`
    /// (`view.rs::blame_file_state`), so the fixture must satisfy BOTH: the
    /// on-disk bytes deliberately differ from the turn's recorded `after`
    /// (otherwise `modified` is false and this asserts the wrong arm while
    /// looking green), and the two unmatched epoch starts are timestamped
    /// AFTER the turn ended.
    #[test]
    fn blame_file_gap_stale_reaches_mcp_byte_identically() {
        let root = repo();
        let store = BlobStore::new(root.join(".agentrec/objects"));
        let blob = store.put(b"recorded\n").unwrap();
        let t = turn(&turn_id(40), vec![entry("src/z.rs", None, Some(blob))]);
        seed(&root, &t);
        std::fs::create_dir_all(root.join("src")).unwrap();
        // Different from the recorded `after` → `modified` is true.
        std::fs::write(root.join("src/z.rs"), b"edited by someone\n").unwrap();
        // Two starts with no stop between them = an uncovered interval, and
        // both are after `t.ended` (2026-08-05T00:00:01.000Z).
        for ts in ["2026-08-05T01:00:00.000Z", "2026-08-05T02:00:00.000Z"] {
            append_log(
                &root.join(".agentrec/log.jsonl"),
                &LogRecord::Epoch(EpochRecord {
                    v: 1,
                    event: "start".into(),
                    ts: ts.into(),
                    dropped_signals: 0,
                }),
            )
            .unwrap();
        }

        let mcp_payload = text(
            &root,
            "agentrec_blame",
            serde_json::json!({"path": "src/z.rs"}),
        );
        let cli_payload = cli_json(&root, &["blame", "src/z.rs", "--json"]);
        assert_eq!(
            mcp_payload, cli_payload,
            "the gap-stale blame payload must be the --json bytes"
        );
        let v: Value = serde_json::from_str(&mcp_payload).unwrap();
        assert_eq!(v["state"]["type"], "file", "wrong blame arm: {mcp_payload}");
        assert_eq!(
            v["state"]["gap_stale"], true,
            "an uncovered interval after the attributing turn must reach the agent: {mcp_payload}"
        );
        assert_eq!(
            v["state"]["modified"], true,
            "gap_stale requires modified — if this is false the fixture, not the code, is wrong"
        );
        assert!(
            v["state"]["turn"].is_object(),
            "the File arm still names its attributor: {mcp_payload}"
        );
    }
}

// =========================================================================
// Task F1 — `agentrec_undo` mode gating + sub-action matrix
// =========================================================================

/// The mode matrix, and the one rail that fires before it.
///
/// **The matrix is read off the parent spec (:641-697), not off the plan's
/// one-line shorthand, and the two differ.** The plan summarises confirm as
/// "request/status path" and auto as "preview/execute path"; the normative
/// source is wider in two cells and is what these tests pin:
///
/// * `preview` — **both** modes. ":641-697" describes it per-mode rather than
///   per-permission: "In auto mode, successful preview atomically reserves
///   executable paths and issues the token. In confirm mode it does not
///   reserve until `request`." A confirm-mode preview is normative; it just
///   reserves nothing.
/// * `status` — **both** modes. It is the only sub-action carrying no mode
///   qualifier at all, where `request` says "confirm mode only" and `execute`
///   says "auto mode only". The confirm paragraph's "The agent only polls
///   `status`" settles confirm; nothing anywhere forbids it in auto.
///
/// So: `preview` both, `status` both, `request` confirm-only, `execute`
/// auto-only. F1 ships the matrix; F2-F4 ship the sub-actions, which is why
/// every legal cell here answers `not_implemented`. A build implementing the
/// shorthand instead reds `preview_and_status_are_legal_in_both_modes` on two
/// of its four cells — measured, not inferred; see that test's own comment.
mod f1 {
    use super::*;
    use serde_json::Value;

    /// A minimal read-consumer root pinned to one `mcp_destructive` mode.
    /// No store is seeded: F1's router refuses or defers before touching one.
    pub(super) fn root_with_mode(mode: &str) -> PathBuf {
        let root = init_root();
        std::fs::write(
            root.join(".agentrec/config.toml"),
            format!("mcp_destructive = \"{mode}\"\n"),
        )
        .expect("write config.toml");
        root
    }

    fn tool_names(root: &Path) -> Vec<String> {
        let responses = mcp(
            root,
            &format!("{}\n", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#),
        );
        responses[0]["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools array missing: {}", responses[0]))
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    fn listed_tool(root: &Path, name: &str) -> Value {
        let responses = mcp(
            root,
            &format!("{}\n", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#),
        );
        responses[0]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} not listed: {}", responses[0]))
            .clone()
    }

    /// One `tools/call` of `agentrec_undo` with the given arguments object.
    pub(super) fn undo(root: &Path, arguments: Value) -> Value {
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": {"name": "agentrec_undo", "arguments": arguments},
        });
        let responses = mcp(root, &format!("{frame}\n"));
        assert_eq!(responses.len(), 1, "one response per call: {responses:#?}");
        responses[0].clone()
    }

    /// The machine-readable `code` out of a domain refusal, asserting the
    /// domain channel was used at all (`isError: true` result, not `-32602`).
    pub(super) fn domain(resp: &Value) -> (String, String) {
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

    /// AC-F1 (a): `off` hides the tool.
    ///
    /// Two halves, because hiding a tool from `tools/list` while still
    /// answering it would be a gate that only looks closed. The call's error
    /// is asserted to be the UNKNOWN-tool refusal specifically — a build that
    /// listed undo in a shared array and gated only the dispatch would answer
    /// "registered but not implemented" here, which tells an agent the tool
    /// exists.
    #[test]
    fn ac_f1_off_hides_the_undo_tool() {
        let root = root_with_mode("off");
        let names = tool_names(&root);
        assert_eq!(names, READ_TOOLS, "off exposes exactly the five read tools");

        let resp = undo(&root, serde_json::json!({"action": "preview"}));
        assert!(
            resp.get("result").is_none(),
            "a hidden tool must not answer: {resp}"
        );
        assert_eq!(resp["error"]["code"], -32602);
        let msg = resp["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("unknown tool") && msg.contains("agentrec_undo"),
            "off must refuse undo as UNKNOWN, not as registered-but-unimplemented: {msg:?}"
        );
    }

    /// AC-F1 (b): `confirm` and `auto` both list six tools, and the sixth
    /// carries destructive annotations with the same key set the read tools
    /// declare.
    #[test]
    fn ac_f1_confirm_and_auto_list_six_tools_with_destructive_annotations() {
        for mode in ["confirm", "auto"] {
            let root = root_with_mode(mode);
            let names = tool_names(&root);
            assert_eq!(
                names.len(),
                6,
                "{mode}: five read tools + undo, got {names:?}"
            );
            assert_eq!(
                names[..5],
                READ_TOOLS[..],
                "{mode}: the read tools keep their §8 table order"
            );
            assert_eq!(
                names[5], "agentrec_undo",
                "{mode}: undo is last, as in the §8 table"
            );

            let undo_tool = listed_tool(&root, "agentrec_undo");
            let a = &undo_tool["annotations"];
            assert_eq!(a["readOnlyHint"], false, "{mode}: {undo_tool}");
            assert_eq!(a["destructiveHint"], true, "{mode}: {undo_tool}");
            assert_eq!(a["idempotentHint"], false, "{mode}: {undo_tool}");
            assert_eq!(a["openWorldHint"], false, "{mode}: {undo_tool}");

            // Same key set as a read tool's annotations — a host reading four
            // keys on five tools and two on the sixth is reading a bug.
            let log_tool = listed_tool(&root, "agentrec_log");
            let log_a = &log_tool["annotations"];
            let read_keys: Vec<&String> = log_a.as_object().unwrap().keys().collect();
            let undo_keys: Vec<&String> = a.as_object().unwrap().keys().collect();
            assert_eq!(read_keys, undo_keys, "{mode}: annotation key sets differ");

            // The read tools are unchanged by undo joining the list.
            assert_eq!(log_a["readOnlyHint"], true, "{mode}");
            assert_eq!(log_a["destructiveHint"], false, "{mode}");

            assert_eq!(undo_tool["inputSchema"]["type"], "object");
            assert!(undo_tool["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()));
        }
    }

    /// AC-F1 (c): `confirm` rejects `execute`.
    ///
    /// The positive control is in the same test and the same config: a build
    /// that refused every sub-action would satisfy the refusal assertion
    /// alone. `request` — confirm's own sub-action — must reach the
    /// not-yet-implemented answer instead.
    #[test]
    fn ac_f1_confirm_rejects_execute() {
        let root = root_with_mode("confirm");

        let (code, message) = domain(&undo(
            &root,
            serde_json::json!({"action": "execute", "token": "t"}),
        ));
        assert_eq!(code, "wrong_mode", "{message}");
        assert!(
            message.contains("confirm") && message.contains("execute"),
            "the refusal must name the effective mode and the sub-action: {message:?}"
        );
        assert!(
            message.contains("auto"),
            "the refusal must name the rail — which mode execute belongs to: {message:?}"
        );

        // Positive control: confirm's own path is legal, merely unbuilt.
        let (code, _) = domain(&undo(&root, serde_json::json!({"action": "request"})));
        assert_eq!(code, "not_implemented", "request is legal in confirm mode");
    }

    /// AC-F1 (d): `auto` rejects `request`, with the mirror-image control.
    #[test]
    fn ac_f1_auto_rejects_request() {
        let root = root_with_mode("auto");

        let (code, message) = domain(&undo(&root, serde_json::json!({"action": "request"})));
        assert_eq!(code, "wrong_mode", "{message}");
        assert!(
            message.contains("auto") && message.contains("request"),
            "the refusal must name the effective mode and the sub-action: {message:?}"
        );
        assert!(
            message.contains("confirm"),
            "the refusal must name the rail — which mode request belongs to: {message:?}"
        );

        let (code, _) = domain(&undo(
            &root,
            serde_json::json!({"action": "execute", "token": "t"}),
        ));
        assert_eq!(code, "not_implemented", "execute is legal in auto mode");
    }

    /// AC-F1 (e): auto + `allow_modified: true` is refused (decision 6 /
    /// PROTOCOL §8 as corrected by delta decision 15).
    ///
    /// Control: the byte-identical call WITHOUT the flag is legal. Without
    /// it, a build refusing every auto preview would pass.
    #[test]
    fn ac_f1_auto_refuses_allow_modified() {
        let root = root_with_mode("auto");

        let (code, message) = domain(&undo(
            &root,
            serde_json::json!({"action": "preview", "turn": "abc", "allow_modified": true}),
        ));
        assert_eq!(code, "allow_modified_refused", "{message}");
        assert!(
            message.contains("auto"),
            "the refusal must name the effective mode: {message:?}"
        );
        assert!(
            message.contains("confirm"),
            "the refusal must name the only override path: {message:?}"
        );

        // Controls. F2 built the preview body, so "legal" no longer reads as
        // `not_implemented`; it reads as the COORDINATOR answering about the
        // repository (this fixture has no turns). The assertion is not
        // weakened — it still fails for any build that refuses these calls
        // at the router, which is what the control exists to catch.
        let (code, _) = domain(&undo(
            &root,
            serde_json::json!({"action": "preview", "turn": "abc"}),
        ));
        assert_eq!(code, "no_turns", "preview without the flag is legal");

        // `false` is not "set" — an explicit false must behave as absence.
        let (code, _) = domain(&undo(
            &root,
            serde_json::json!({"action": "preview", "turn": "abc", "allow_modified": false}),
        ));
        assert_eq!(code, "no_turns", "allow_modified: false is not the rail");
    }

    /// The rail keys on `auto`, NOT on the flag: decision 6 keeps an override
    /// path open, and it is explicit human approval in confirm mode. Without
    /// this test, "refuse `allow_modified: true` always" satisfies AC-F1's
    /// four cases while deleting the only sanctioned override.
    #[test]
    fn confirm_does_not_refuse_allow_modified() {
        let root = root_with_mode("confirm");
        let (code, _) = domain(&undo(
            &root,
            serde_json::json!({"action": "request", "turn": "abc", "allow_modified": true}),
        ));
        assert_eq!(
            code, "not_implemented",
            "confirm mode is the sanctioned override path — the rail must not fire here"
        );
    }

    /// Ordering, pinned rather than left to accident: the `allow_modified`
    /// rail is evaluated BEFORE the mode matrix, so the invariant "no
    /// auto-mode call carrying `allow_modified: true` proceeds past argument
    /// validation, for any sub-action" holds without an exception for
    /// wrong-mode calls.
    #[test]
    fn the_allow_modified_rail_precedes_the_mode_matrix() {
        let root = root_with_mode("auto");
        let (code, _) = domain(&undo(
            &root,
            serde_json::json!({"action": "request", "allow_modified": true}),
        ));
        assert_eq!(
            code, "allow_modified_refused",
            "the rail fires first; `wrong_mode` here would leave the invariant holed"
        );
    }

    /// `preview` and `status` are legal in BOTH modes (see this module's doc
    /// comment for the derivation from parent :641-697).
    ///
    /// **All four cells are evaluated before anything is asserted, so the
    /// failure names every cell that broke rather than only the first.** A
    /// short-circuiting `assert_eq!` per cell would make any statement about
    /// *how many* cells a mutation breaks unmeasurable by this test —
    /// measured under the plan-shorthand mutation (`preview` auto-only,
    /// `status` confirm-only), this form reports exactly two: `confirm`/
    /// `preview` and `auto`/`status`.
    #[test]
    fn preview_and_status_are_legal_in_both_modes() {
        let mut refused = Vec::new();
        for mode in ["confirm", "auto"] {
            let root = root_with_mode(mode);
            for action in ["preview", "status"] {
                let (code, message) = domain(&undo(
                    &root,
                    serde_json::json!({"action": action, "turn": "abc"}),
                ));
                // "Legal" = not refused by the mode matrix or the rail. The
                // per-action legal answer differs now that F2 built preview:
                // `status` is still unbuilt, `preview` reaches the
                // coordinator and reports this empty fixture's `no_turns`.
                let legal = match action {
                    "preview" => "no_turns",
                    _ => "not_implemented",
                };
                if code != legal {
                    refused.push(format!("{mode}/{action}: {code} — {message}"));
                }
            }
        }
        assert!(
            refused.is_empty(),
            "preview and status must be legal in both modes; {} cell(s) refused:\n{}",
            refused.len(),
            refused.join("\n")
        );
    }

    /// A sub-action outside the four is a malformed call (`-32602`), not a
    /// domain refusal: the schema declares the enum, so the client's frame is
    /// wrong. Same channel as a missing `action`.
    #[test]
    fn an_unknown_or_missing_action_is_a_malformed_call() {
        let root = root_with_mode("auto");

        let resp = undo(&root, serde_json::json!({"action": "obliterate"}));
        assert_eq!(resp["error"]["code"], -32602, "{resp}");
        assert!(resp["error"]["message"]
            .as_str()
            .unwrap()
            .contains("obliterate"));

        let resp = undo(&root, serde_json::json!({}));
        assert_eq!(resp["error"]["code"], -32602, "{resp}");

        let resp = undo(&root, serde_json::json!({"action": 7}));
        assert_eq!(resp["error"]["code"], -32602, "{resp}");
    }
}

/// Task F2 — `agentrec_undo action:"preview"` over the real wire, driving the
/// real binary. The unit-level ACs live in
/// `agentrec_core::undo_coordinator::coordinator_tests`; what these add is the
/// part only a subprocess can show: the typed `UndoPreview` reaching a client
/// as JSON, and a reservation created by ONE process being seen by the NEXT
/// one — which is what makes the ledger a file rather than process memory.
mod f2 {
    use super::f1::{domain, root_with_mode, undo};
    use agentrec_core::record::{FileEntry, LogRecord, TurnRecord};
    use agentrec_core::store::{hash_bytes, BlobStore};
    use serde_json::Value;
    use std::path::Path;

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

    /// A file with its `before` blob stored and on-disk bytes matching
    /// `after` — the only shape that is actually executable.
    fn revertible(root: &Path, path: &str) -> FileEntry {
        let store = BlobStore::new(root.join(".agentrec/objects"));
        let before = store.put(format!("old {path}").as_bytes()).unwrap();
        let now = format!("new {path}");
        std::fs::write(root.join(path), now.as_bytes()).unwrap();
        let mut e = entry(path);
        e.before = Some(before);
        e.after = Some(hash_bytes(now.as_bytes()));
        e
    }

    const TURN: &str = "t_F20000000000000000000F2";

    fn seed(root: &Path, files: Vec<FileEntry>) {
        std::fs::create_dir_all(root.join(".agentrec/objects")).unwrap();
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
            files,
        };
        let line = serde_json::to_string(&LogRecord::Turn(t)).unwrap();
        let p = root.join(".agentrec/log.jsonl");
        let prev = std::fs::read_to_string(&p).unwrap_or_default();
        std::fs::write(&p, format!("{prev}{line}\n")).unwrap();
    }

    /// The success payload of a `tools/call`, asserting it was NOT a refusal.
    fn ok_payload(resp: &Value) -> Value {
        assert!(resp.get("error").is_none(), "JSON-RPC error: {resp}");
        assert_ne!(
            resp["result"]["isError"], true,
            "unexpected refusal: {resp}"
        );
        serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap())
            .expect("payload is JSON")
    }

    /// AC-F2: the typed preview crosses the wire, carries per-file rows and
    /// classed refusals, and writes nothing to the working tree.
    #[test]
    fn preview_crosses_the_wire_as_the_typed_payload_and_writes_nothing() {
        let root = root_with_mode("confirm");
        let mut withheld = entry(".env");
        withheld.withheld = true;
        let files = vec![revertible(&root, "a.rs"), withheld];
        seed(&root, files);
        let before = std::fs::read(root.join("a.rs")).unwrap();

        let payload = ok_payload(&undo(
            &root,
            serde_json::json!({"action": "preview", "turn": TURN}),
        ));
        assert_eq!(payload["mode"], "confirm");
        assert_eq!(payload["turn"], TURN);
        assert_eq!(payload["files"].as_array().unwrap().len(), 1, "{payload}");
        assert_eq!(payload["files"][0]["path"], "a.rs");
        assert_eq!(payload["refusals"][0]["path"], ".env");
        assert_eq!(payload["refusals"][0]["class"], "withheld");
        assert!(payload.get("token").is_none(), "confirm issues no token");
        assert_eq!(std::fs::read(root.join("a.rs")).unwrap(), before);
        assert!(
            !root.join(".agentrec/undo-requests.jsonl").exists(),
            "a confirm preview must not reserve"
        );
    }

    /// AC-F2, P6 across PROCESSES. Each `undo(...)` is a fresh
    /// `agentrec mcp` subprocess, so the second call can only see the first
    /// call's reservation by reading it off disk — the property that makes
    /// the ledger a file. Disjoint subsets of the same turn coexist;
    /// overlapping ones are `undo_conflict`.
    #[test]
    fn an_auto_reservation_survives_the_process_that_made_it() {
        let root = root_with_mode("auto");
        let files = vec![
            revertible(&root, "a.rs"),
            revertible(&root, "b.rs"),
            revertible(&root, "c.rs"),
        ];
        seed(&root, files);

        let first = ok_payload(&undo(
            &root,
            serde_json::json!({"action": "preview", "turn": TURN, "paths": ["a.rs"]}),
        ));
        let token = first["token"]
            .as_str()
            .expect("auto issues a token")
            .to_string();
        assert_eq!(token.len(), 40);
        assert!(first["token_expires_unix_ms"].is_u64());

        // Different process, disjoint subset: no conflict.
        let second = ok_payload(&undo(
            &root,
            serde_json::json!({"action": "preview", "turn": TURN, "paths": ["b.rs"]}),
        ));
        assert_ne!(second["token"], first["token"], "tokens must not repeat");

        // Different process, overlapping: refused at creation.
        let (code, message) = domain(&undo(
            &root,
            serde_json::json!({"action": "preview", "turn": TURN, "paths": ["b.rs", "c.rs"]}),
        ));
        assert_eq!(code, "undo_conflict", "{message}");
        assert!(message.contains("b.rs"), "{message}");

        // Neither raw token was persisted.
        let ledger = std::fs::read_to_string(root.join(".agentrec/undo-requests.jsonl")).unwrap();
        assert!(
            !ledger.contains(&token),
            "raw token in the ledger: {ledger}"
        );
        assert!(ledger.contains(&hash_bytes(token.as_bytes())));
    }
}
