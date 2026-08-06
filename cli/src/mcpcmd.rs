//! `agentrec mcp` — stdio JSON-RPC 2.0 server (Phase 2.2, Task E1).
//!
//! **Hand-rolled by decision** (delta decision 12): the official `rmcp` SDK
//! requires an async runtime, and this codebase's "threads + `notify`, no
//! async runtime" invariant outweighs SDK convenience for a stdio server with
//! exactly one client. Scope owned here: newline-delimited JSON-RPC 2.0
//! framing over stdin/stdout, `initialize` + protocol-revision negotiation,
//! `tools/list`, `tools/call` dispatch, graceful shutdown on stdin EOF. No
//! new dependency: `serde_json` was already present.
//!
//! E1 ships the *skeleton*: the five 2.2 read tools appear in `tools/list`
//! with their metadata and read-only annotations, but their implementations
//! land in E2 (`agentrec_log`/`agentrec_diff`/`agentrec_blame`) and E3
//! (`agentrec_recall`/`agentrec_status`). A `tools/call` naming one of them
//! answers with a clean JSON-RPC error until then — never a silent empty
//! result, which an agent would read as "no history".
//!
//! Reads are file-based (P3), so nothing here touches the daemon: the server
//! is fully functional with `agentrec record` stopped.

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{json, Map, Value};

use crate::config::{self, Config};

/// MCP protocol revisions this build speaks, pinned at build time (decision
/// 12). The negotiated revision is echoed back in the `initialize` result and
/// — from E3 — in `agentrec_status`.
///
/// **Reading recorded, because the plan's wording does not map onto MCP.**
/// The plan says "refuse unknown-major client revision", mirroring PROTOCOL
/// §10's integer-`v` major stance. MCP revisions are `YYYY-MM-DD` date
/// strings with *no major component at all*, so there is no major to compare
/// and no non-arbitrary way to derive one from a date. Rather than invent a
/// major-from-date rule, this build treats the revision string atomically: a
/// member of this set negotiates, anything else is refused with `-32602`
/// naming the set. That is strictly *stronger* than "refuse unknown majors"
/// (it also refuses same-series revisions this build has not been tested
/// against), and it is the honest reading — the alternative silently
/// downgrades a client to a revision it never asked for.
///
/// **Provenance of these values, since a stale pin means a real host gets
/// `-32602` and no session at all.** Both are read off artifacts on this
/// machine, not recalled: `@modelcontextprotocol/sdk` 1.27.1 (vendored under
/// `~/.claude/plugins/cache/.../telegram/0.0.6/node_modules`) declares
/// `LATEST_PROTOCOL_VERSION = '2025-11-25'` and lists `'2025-06-18'` next in
/// `SUPPORTED_PROTOCOL_VERSIONS`; `2025-06-18` is also the revision observed
/// in initialize frames in the official plugin docs on disk. Older entries in
/// that SDK list (`2025-03-26` and earlier) are deliberately NOT claimed —
/// this server has been exercised against neither, and claiming a revision is
/// a wire promise. **Neither value is verified against a live host in this
/// environment**; E4's doctor probe ("server answers `initialize`") is where
/// that gets exercised.
const SUPPORTED_REVISIONS: &[&str] = &["2025-11-25", "2025-06-18"];

/// The revision reported when a client's `initialize` omits `protocolVersion`
/// entirely (permitted by the framing here; the field is how a client *opts
/// into* a revision, and its absence is not a conflict to refuse).
const PINNED_REVISION: &str = SUPPORTED_REVISIONS[0];

// JSON-RPC 2.0 error codes (§5.1 of the spec).
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// One `tools/list` entry. Descriptions state capability and data shape only
/// — never instructions to the model (P7: tool output and metadata are data,
/// not directives).
struct ToolSpec {
    name: &'static str,
    description: &'static str,
    /// JSON-Schema `properties` object, rendered into `inputSchema`.
    properties: fn() -> Value,
    required: &'static [&'static str],
}

/// The five Phase 2.2 read tools, in the parent spec's §8 table order.
/// `agentrec_undo` is Phase 2.3's (Task F1) and is deliberately absent: it
/// joins this list only when `mcp_destructive != off`.
const READ_TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "agentrec_log",
        description: "Recorded turns for this repository, newest first, as bounded turn \
                      summaries with a pagination cursor. Git-operation turns are excluded \
                      unless requested.",
        properties: || {
            json!({
                "include_git": {"type": "boolean", "description": "Include git-operation turns."},
                "limit": {"type": "integer", "minimum": 1, "description": "Maximum summaries to return (server-bounded)."},
                "cursor": {"type": "string", "description": "Opaque cursor from a previous page."}
            })
        },
        required: &[],
    },
    ToolSpec {
        name: "agentrec_diff",
        description: "Unified diff of one recorded turn's file changes, with per-file \
                      metadata and a pagination cursor when the diff exceeds the line bound.",
        properties: || {
            json!({
                "turn": {"type": "string", "description": "Turn id, full or an unambiguous prefix."},
                "paths": {"type": "array", "items": {"type": "string"}, "description": "Restrict the diff to these repo-relative paths."},
                "limit": {"type": "integer", "minimum": 1, "description": "Maximum diff lines to return (server-bounded)."},
                "cursor": {"type": "string", "description": "Opaque cursor from a previous page."}
            })
        },
        required: &["turn"],
    },
    ToolSpec {
        name: "agentrec_blame",
        description: "Which recorded turn last touched a file or line: attribution, \
                      human-edited-since state, and recording-gap honesty when the interval \
                      is uncovered.",
        properties: || {
            json!({
                "path": {"type": "string", "description": "Repo-relative file path."},
                "line": {"type": "integer", "minimum": 1, "description": "1-based line number; omit for whole-file attribution."}
            })
        },
        required: &["path"],
    },
    ToolSpec {
        name: "agentrec_recall",
        description: "Rank-then-verify search over this repository's recorded memory facts. \
                      Returns hash-verified fresh hits only, with the store's own \
                      capped/corrupt/empty state.",
        properties: || {
            json!({
                "query": {"type": "string", "description": "Free-text query."},
                "k": {"type": "integer", "minimum": 1, "description": "Maximum hits to return (server-bounded)."},
                "cursor": {"type": "string", "description": "Opaque cursor from a previous page."}
            })
        },
        required: &["query"],
    },
    ToolSpec {
        name: "agentrec_status",
        description: "Repository recording health: store size, recording gaps, daemon \
                      liveness, effective agent-undo mode, and the negotiated protocol \
                      revision.",
        properties: || json!({}),
        required: &[],
    },
];

/// Long-lived server state. Config is read ONCE, at startup (parent spec:
/// "config loads once at startup; changing destructive mode requires restart
/// and is reported in `agentrec_status`") — there is deliberately no reload
/// path, and E3's `agentrec_status` is what surfaces a since-changed file as
/// restart-required.
struct Server {
    #[allow(
        dead_code,
        reason = "read by agentrec_status (E3) and undo gating (F1)"
    )]
    root: std::path::PathBuf,
    config: Config,
    negotiated_revision: Option<String>,
}

impl Server {
    /// The effective agent-undo mode for this process, fixed at startup.
    /// `McpDestructive` (B1) is THE mode type — no second enum exists.
    fn destructive_mode(&self) -> config::McpDestructive {
        self.config.mcp_destructive
    }

    /// Tools visible to `tools/list`. E1 exposes the five read tools in every
    /// mode; `agentrec_undo` joins the list in F1, gated on
    /// [`Self::destructive_mode`] being other than `Off`.
    fn visible_tools(&self) -> Vec<Value> {
        READ_TOOLS
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": {
                        "type": "object",
                        "properties": (t.properties)(),
                        "required": t.required,
                    },
                    // Read-only annotations (parent spec :636). Every tool in
                    // 2.2 reads; none writes, none reaches the network.
                    "annotations": {
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": false,
                    },
                })
            })
            .collect()
    }
}

/// Entry point for the `mcp` subcommand.
///
/// Two startup preconditions, both hard errors before a single frame is read
/// — a long-lived server answering reads against the wrong (or an
/// uninitialized) root is precisely the silent-wrong-root failure O5 measured
/// on the hook path, and here there is no fail-open contract to honor:
///
/// 1. `<root>/.agentrec/` must exist. Without it every read would return an
///    honest-looking empty result forever.
/// 2. `.agentrec/config.toml`, if present, must parse. [`config::load`]'s
///    `Err` propagates exactly as it does for `status`/`purge`/`log`/`show`
///    (D16) — the tolerant [`config::load_or_default`] shim is for the daemon
///    tick and the hook path, not for a verb the user launched.
pub fn run(root: &Path) -> Result<(), String> {
    if !crate::agentrec_dir(root).is_dir() {
        return Err(format!(
            "no .agentrec/ at {} — run `agentrec init` in the repository first",
            root.display()
        ));
    }
    let config = config::load(root).map_err(|e| e.to_string())?;
    let mut server = Server {
        root: root.to_path_buf(),
        config,
        negotiated_revision: None,
    };

    // Startup banner on STDERR — stdout is reserved for JSON-RPC frames and a
    // stray line there corrupts the stream. Hosts capture a stdio server's
    // stderr as its log, and the effective mode is the one startup fact a
    // user debugging "why won't it undo" needs; E3's `agentrec_status`
    // reports the same value over the wire.
    eprintln!(
        "agentrec mcp: root {} — agent-undo mode {}",
        root.display(),
        match server.destructive_mode() {
            config::McpDestructive::Off => "off",
            config::McpDestructive::Confirm => "confirm",
            config::McpDestructive::Auto => "auto",
        }
    );

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        // A read error on stdin (host went away mid-frame) ends the session
        // the same way EOF does: graceful shutdown, exit 0.
        let Ok(line) = line else { break };
        if let Some(response) = handle_line(&mut server, &line) {
            // Each response is written AND FLUSHED before the next line is
            // read: a host sends `initialize` and waits for the answer before
            // saying anything else, so a server that buffered until exit
            // would deadlock every real session. A write failure (host
            // vanished between our read and our write) ends the session the
            // same graceful way EOF does — a broken pipe is not this
            // process's error to report.
            if writeln!(out, "{response}").is_err() || out.flush().is_err() {
                break;
            }
        }
    }
    Ok(())
}

/// Process one input line. `None` = write nothing, which covers both framing
/// noise (blank lines) and JSON-RPC notifications — a frame with no `id` MUST
/// NOT be answered, and MCP hosts send `notifications/initialized` right
/// after initialize.
///
/// **`initialize` ordering is deliberately NOT enforced.** A `tools/list`
/// before any handshake is answered normally rather than refused: the
/// metadata is static, refusing buys no safety, and a strict ordering check
/// would make this server the only thing in the repo that fails a read
/// because of session bookkeeping. The consequence for later tasks is that
/// [`Server::negotiated_revision`] can legitimately be `None` when E3's
/// `agentrec_status` renders it.
fn handle_line(server: &mut Server, line: &str) -> Option<Value> {
    if line.trim().is_empty() {
        return None;
    }
    let frame: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        // A frame that never parsed carries no id, so the error carries
        // `id: null` (JSON-RPC 2.0 §5).
        Err(e) => {
            return Some(error_response(
                Value::Null,
                PARSE_ERROR,
                &format!("parse error: {e}"),
                None,
            ))
        }
    };
    // A frame that parsed but isn't a JSON object — a batch array, a bare
    // scalar — has no `id` to key on, so falling through to the notification
    // path would answer it with total silence and hang a client that is
    // waiting. Batching was removed in MCP 2025-06-18, so a conforming client
    // never sends one; an explicit `-32600` still beats silence.
    if !frame.is_object() {
        return Some(error_response(
            Value::Null,
            INVALID_REQUEST,
            "a JSON-RPC frame must be an object (batches are not supported)",
            None,
        ));
    }
    let id = frame.get("id").cloned();
    let method = frame.get("method").and_then(Value::as_str);
    let Some(method) = method else {
        return id.map(|id| error_response(id, INVALID_REQUEST, "missing \"method\"", None));
    };
    // Notification: no id, no response — even for an unknown method.
    let id = id?;
    let params = frame.get("params");
    Some(match method {
        "initialize" => initialize(server, id, params),
        "tools/list" => success(id, json!({"tools": server.visible_tools()})),
        "tools/call" => tools_call(id, params),
        // `ping` is part of MCP's base protocol and costs nothing to answer.
        "ping" => success(id, json!({})),
        other => error_response(
            id,
            METHOD_NOT_FOUND,
            &format!("unknown method {other:?}"),
            None,
        ),
    })
}

fn initialize(server: &mut Server, id: Value, params: Option<&Value>) -> Value {
    let requested = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str);
    let negotiated = match requested {
        None => PINNED_REVISION,
        Some(r) if SUPPORTED_REVISIONS.contains(&r) => r,
        Some(r) => {
            return error_response(
                id,
                INVALID_PARAMS,
                &format!("unsupported MCP protocol revision {r:?}"),
                Some(json!({"supported": SUPPORTED_REVISIONS})),
            );
        }
    };
    server.negotiated_revision = Some(negotiated.to_string());
    success(
        id,
        json!({
            "protocolVersion": negotiated,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "agentrec", "version": env!("CARGO_PKG_VERSION")},
        }),
    )
}

/// `tools/call` dispatch. Every read tool's implementation lands in E2/E3;
/// until then a call to a listed tool is refused by name, distinctly from a
/// call to a tool that does not exist at all.
fn tools_call(id: Value, params: Option<&Value>) -> Value {
    let name = params.and_then(|p| p.get("name")).and_then(Value::as_str);
    let Some(name) = name else {
        return error_response(id, INVALID_PARAMS, "tools/call requires \"name\"", None);
    };
    if READ_TOOLS.iter().any(|t| t.name == name) {
        return error_response(
            id,
            INVALID_PARAMS,
            &format!("tool {name:?} is registered but not implemented in this build"),
            None,
        );
    }
    error_response(id, INVALID_PARAMS, &format!("unknown tool {name:?}"), None)
}

fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = Map::new();
    error.insert("code".into(), json!(code));
    error.insert("message".into(), json!(message));
    if let Some(data) = data {
        error.insert("data".into(), data);
    }
    json!({"jsonrpc": "2.0", "id": id, "error": Value::Object(error)})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server {
            root: std::path::PathBuf::from("/nonexistent"),
            config: Config::default(),
            negotiated_revision: None,
        }
    }

    #[test]
    fn default_config_means_undo_stays_off() {
        // B1's `McpDestructive` is the only mode type in play; F1 gates
        // `agentrec_undo` on it. Today the read-tool list is mode-independent
        // and carries no destructive tool at all.
        assert_eq!(server().destructive_mode(), config::McpDestructive::Off);
        let names: Vec<String> = server()
            .visible_tools()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.iter().any(|n| n == "agentrec_undo"));
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn pinned_revision_is_date_shaped_and_supported() {
        assert!(SUPPORTED_REVISIONS.contains(&PINNED_REVISION));
        for r in SUPPORTED_REVISIONS {
            assert_eq!(r.len(), 10, "revision must be YYYY-MM-DD: {r}");
        }
    }

    #[test]
    fn notifications_and_blank_lines_produce_no_frame() {
        let mut s = server();
        assert!(handle_line(&mut s, "").is_none());
        assert!(handle_line(&mut s, "   ").is_none());
        assert!(handle_line(
            &mut s,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
        )
        .is_none());
        // …including for a method that does not exist.
        assert!(handle_line(&mut s, r#"{"jsonrpc":"2.0","method":"nope"}"#).is_none());
    }

    #[test]
    fn initialize_records_the_negotiated_revision() {
        let mut s = server();
        let frame = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{PINNED_REVISION}"}}}}"#
        );
        let resp = handle_line(&mut s, &frame).unwrap();
        assert_eq!(resp["result"]["protocolVersion"], PINNED_REVISION);
        assert_eq!(s.negotiated_revision.as_deref(), Some(PINNED_REVISION));
    }

    #[test]
    fn an_unsupported_revision_leaves_the_session_unnegotiated() {
        let mut s = server();
        let resp = handle_line(
            &mut s,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        )
        .unwrap();
        assert_eq!(resp["error"]["code"], INVALID_PARAMS);
        assert!(s.negotiated_revision.is_none());
    }
}
