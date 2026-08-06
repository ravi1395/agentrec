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
//! E1 shipped the *skeleton* (framing, handshake, `tools/list` metadata and
//! read-only annotations); E2 implemented `agentrec_log`/`agentrec_diff`/
//! `agentrec_blame`; **E3 implements the last two, `agentrec_recall` and
//! `agentrec_status`, so all five 2.2 read tools now answer.** The
//! listed-but-unimplemented refusal path survives for F1's `agentrec_undo`
//! (see [`tools_call`]) — a listed tool must never answer a silent empty
//! result, which an agent would read as "no history".
//!
//! Reads are file-based (P3), so nothing here touches the daemon: the server
//! is fully functional with `agentrec record` stopped.

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{json, Map, Value};

use agentrec_core::view;

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
pub(crate) const PINNED_REVISION: &str = SUPPORTED_REVISIONS[0];

// JSON-RPC 2.0 error codes (§5.1 of the spec).
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// `agentrec_log`'s server bound (parent spec §8: "≤200 turn summaries +
/// cursor"). A client `limit` narrows it; nothing widens it.
const MAX_LOG_SUMMARIES: usize = 200;

/// `agentrec_diff`'s server bound (parent spec §8: "≤2,000 diff lines").
///
/// **Unit recorded, because the spec's unit does not exist on this wire.**
/// The §8 row was written against a rendered unified diff; what this tool
/// emits is [`view::DiffResult`], whose `FileDiffState::Text` carries the raw
/// `before`/`after` blobs, not hunks. There are therefore no "diff lines" in
/// the payload to count. This build counts **payload lines** — the newline
/// count of the content each admitted file entry carries — which is the same
/// order of magnitude and is the figure that actually bounds the bytes an
/// agent receives. It deliberately does NOT equal `agentrec diff`'s rendered
/// unified-diff line count, and no attempt is made to make it: computing
/// rendered hunks here would put diff interpretation back in `cli/src`, which
/// is precisely the seam P4/P5 closed.
///
/// The bound is applied by choosing how many whole FILE ENTRIES fit (the unit
/// [`view::DiffQuery::limit`] pages in), always admitting at least one so a
/// single oversized file cannot stall pagination. When the whole diff fits,
/// the unpaginated result is returned untouched — which is what keeps the
/// payload byte-identical to `diff --json`.
const MAX_DIFF_PAYLOAD_LINES: usize = 2_000;

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
                      summaries with a pagination cursor. Turns hidden from the default \
                      listing — git-operation turns, and turns superseded by a retroactive \
                      merge — are excluded unless requested.",
        properties: || {
            json!({
                "include_hidden": {"type": "boolean", "description": "Also return turns the default listing hides: git-operation turns AND turns superseded by a retroactive merge."},
                "limit": {"type": "integer", "minimum": 1, "description": "Maximum summaries to return (server-bounded)."},
                "cursor": {"type": "string", "description": "Opaque cursor from a previous page."}
            })
        },
        required: &[],
    },
    ToolSpec {
        name: "agentrec_diff",
        description: "One recorded turn's file changes as raw before/after content per file \
                      — not rendered diff hunks — with per-file metadata and a pagination \
                      cursor when the payload exceeds the content-line bound.",
        properties: || {
            json!({
                "turn": {"type": "string", "description": "Turn id, full or an unambiguous prefix."},
                "paths": {"type": "array", "items": {"type": "string"}, "description": "Restrict the diff to these repo-relative paths."},
                "limit": {"type": "integer", "minimum": 1, "description": "Payload budget in content lines — the before/after blob lines each admitted file entry carries, NOT rendered unified-diff lines. Spent by admitting whole file entries; one entry is always admitted, so a single oversized file can exceed it. Server-bounded."},
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
                      capped/corrupt/empty state. Freshness is derived from the live \
                      worktree, so a later page can miss a fact that became fresh after the \
                      cursor was minted. A recall page never carries a continuation cursor \
                      of its own; raise `k` to see more hits.",
        properties: || {
            json!({
                "query": {"type": "string", "description": "Free-text query."},
                "k": {"type": "integer", "minimum": 1, "description": "Maximum hits to return (server-bounded)."},
                "cursor": {"type": "string", "description": "Opaque cursor identifying the hit to resume after. A recall page's own `next` is always null today, so this cannot be obtained from a previous page — see the tool description."}
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
    root: std::path::PathBuf,
    config: Config,
    negotiated_revision: Option<String>,
    /// `.agentrec/config.toml`'s modification time as of startup, or `None`
    /// when the file did not exist then. Compared against the live value by
    /// `agentrec_status` — see [`config_mtime`].
    config_stamp: Option<std::time::SystemTime>,
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
    // Stamped AFTER the load, deliberately: a stamp taken first could be
    // older than the bytes actually parsed (an edit landing between the two
    // reads), which would make `agentrec_status` report restart-required for
    // a change this process already picked up.
    let config_stamp = config_mtime(root);
    let mut server = Server {
        root: root.to_path_buf(),
        config,
        negotiated_revision: None,
        config_stamp,
    };

    // Startup banner on STDERR — stdout is reserved for JSON-RPC frames and a
    // stray line there corrupts the stream. Hosts capture a stdio server's
    // stderr as its log, and the effective mode is the one startup fact a
    // user debugging "why won't it undo" needs; E3's `agentrec_status`
    // reports the same value over the wire.
    eprintln!(
        "agentrec mcp: root {} — agent-undo mode {}",
        root.display(),
        mode_str(server.destructive_mode())
    );

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        // A read error on stdin ends the session the same way EOF does:
        // graceful shutdown, exit 0. Two causes reach here, not one — the
        // host went away mid-frame, OR a live host sent bytes that are not
        // valid UTF-8 (`BufRead::lines` errors on those). Neither is
        // recoverable framing: the stream position is no longer known.
        //
        // There is deliberately NO line-length bound: an unterminated frame
        // allocates without limit. Accepted, not overlooked — this is a
        // stdio server with exactly one client, the host process that spawned
        // it, and that host also controls the input; a bound here would only
        // constrain a party already able to exhaust us by other means.
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
        "tools/call" => tools_call(server, id, params),
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

/// `tools/call` dispatch.
///
/// **Two error channels, kept apart on purpose** (they are read by different
/// things and mean different things):
///
/// * A malformed *call* — no `name`, a missing required argument, an argument
///   of the wrong type — is a JSON-RPC `-32602`, exactly as E1 already
///   answers an unknown tool. The client's frame was wrong; there is no
///   result.
/// * A *domain outcome* — the turn ref did not resolve, the cursor went stale
///   under a ledger rewrite — is a normal `tools/call` result carrying
///   `isError: true` and a machine-readable code in the text payload
///   (`stale_cursor`, `query_mismatch`, `not_found`, …). This is the shape
///   the parent spec's "rewrite/truncation returns `stale_cursor`" needs: the
///   agent must be able to *read* the code and restart its query, which a
///   transport-level error does not reliably surface to a model.
fn tools_call(server: &Server, id: Value, params: Option<&Value>) -> Value {
    let name = params.and_then(|p| p.get("name")).and_then(Value::as_str);
    let Some(name) = name else {
        return error_response(id, INVALID_PARAMS, "tools/call requires \"name\"", None);
    };
    // Absent `arguments` is the same as `{}` — a no-argument tool is called
    // that way by real hosts.
    let empty = Value::Object(Map::new());
    let args = params
        .and_then(|p| p.get("arguments"))
        .cloned()
        .unwrap_or(empty);
    if !args.is_object() {
        return error_response(id, INVALID_PARAMS, "\"arguments\" must be an object", None);
    }
    let outcome = match name {
        "agentrec_log" => log_tool(&server.root, &args),
        "agentrec_diff" => diff_tool(&server.root, &args),
        "agentrec_blame" => blame_tool(&server.root, &args),
        "agentrec_recall" => recall_tool(&server.root, &args),
        "agentrec_status" => status_tool(server),
        // **Kept, though E3 leaves it unreachable from `READ_TOOLS` alone.**
        // All five listed read tools now dispatch above, so nothing in
        // `READ_TOOLS` reaches this arm today. It is retained rather than
        // deleted because F1 adds `agentrec_undo` to the listed set gated on
        // `mcp_destructive`, and the window in which a tool is listed but not
        // yet dispatched is exactly when this arm matters: without it that
        // call falls through to "unknown tool", which tells an agent the tool
        // does not exist when `tools/list` just said it does. The two
        // refusals are deliberately distinct messages.
        other if READ_TOOLS.iter().any(|t| t.name == other) => {
            return error_response(
                id,
                INVALID_PARAMS,
                &format!("tool {other:?} is registered but not implemented in this build"),
                None,
            )
        }
        other => {
            return error_response(id, INVALID_PARAMS, &format!("unknown tool {other:?}"), None)
        }
    };
    match outcome {
        Ok(payload) => tool_result(id, &payload, false),
        Err(ToolFailure::BadParams(message)) => error_response(id, INVALID_PARAMS, &message, None),
        Err(ToolFailure::Domain { code, message }) => tool_result(
            id,
            &json!({"error": code, "message": message}).to_string(),
            true,
        ),
    }
}

/// How a read tool can fail. See [`tools_call`] for why the two channels are
/// not collapsed.
enum ToolFailure {
    /// The call frame was wrong → JSON-RPC `-32602`.
    BadParams(String),
    /// The repository answered, and the answer is a refusal the agent can act
    /// on → `isError: true` result carrying `code`.
    Domain { code: &'static str, message: String },
}

/// A `tools/call` result. The payload is delivered as a single `text` content
/// block holding compact JSON — the SAME bytes `--json` prints (see
/// [`diff_tool`]/[`blame_tool`]), never a re-rendering. `structuredContent`
/// is deliberately not also emitted: it would duplicate the payload in every
/// response for hosts that already read `text`, and a second copy is a second
/// thing that can drift.
fn tool_result(id: Value, payload: &str, is_error: bool) -> Value {
    success(
        id,
        json!({
            "content": [{"type": "text", "text": payload}],
            "isError": is_error,
        }),
    )
}

fn open_view(root: &Path) -> Result<view::RepositoryView, ToolFailure> {
    view::RepositoryView::open(root).map_err(|e| ToolFailure::Domain {
        code: "repo_error",
        message: e.to_string(),
    })
}

/// `CursorError` → the wire codes the parent spec names.
fn cursor_failure(e: &view::CursorError) -> ToolFailure {
    match e {
        view::CursorError::Stale => ToolFailure::Domain {
            code: "stale_cursor",
            message: "the record this cursor names is no longer in the ledger — restart the query"
                .into(),
        },
        view::CursorError::QueryMismatch => ToolFailure::Domain {
            code: "query_mismatch",
            message: "this cursor was minted against a different query".into(),
        },
        // Unreachable from here: a zero `limit` is refused as `-32602` by
        // [`bounded_limit`] before any query is built. Mapped anyway so the
        // arm cannot silently become a panic if that guard ever moves.
        view::CursorError::ZeroLimit => ToolFailure::Domain {
            code: "zero_limit",
            message: "limit must be at least 1".into(),
        },
    }
}

// ---- argument reading ---------------------------------------------------

fn str_arg(args: &Value, key: &str) -> Result<Option<String>, ToolFailure> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(ToolFailure::BadParams(format!("{key:?} must be a string"))),
    }
}

fn required_str_arg(args: &Value, key: &str) -> Result<String, ToolFailure> {
    str_arg(args, key)?.ok_or_else(|| ToolFailure::BadParams(format!("{key:?} is required")))
}

fn bool_arg(args: &Value, key: &str) -> Result<bool, ToolFailure> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(ToolFailure::BadParams(format!("{key:?} must be a boolean"))),
    }
}

/// A positive-integer argument. Zero and negatives are `-32602` rather than a
/// clamp: the schema says `minimum: 1`, and silently turning `limit: 0` into
/// a full page would answer a question the client did not ask.
fn positive_usize_arg(args: &Value, key: &str) -> Result<Option<usize>, ToolFailure> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if n >= 1 => Ok(Some(usize::try_from(n).unwrap_or(usize::MAX))),
            _ => Err(ToolFailure::BadParams(format!(
                "{key:?} must be an integer >= 1"
            ))),
        },
    }
}

/// The client's `limit`, narrowed to the server bound. Absent = the bound.
fn bounded_limit(args: &Value, cap: usize) -> Result<usize, ToolFailure> {
    Ok(positive_usize_arg(args, "limit")?.unwrap_or(cap).min(cap))
}

/// A `cursor` argument, accepted in EITHER shape a client can plausibly hold:
/// the JSON text of a [`view::Cursor`], or the `next` object lifted verbatim
/// out of a previous payload. The second form matters because the payload the
/// agent just read carries `next` as an object (it is the serialized
/// `Page::next`, and parity with `--json` forbids rewriting it into a
/// string), so demanding a string would make the obvious move fail.
fn cursor_arg(args: &Value) -> Result<Option<view::Cursor>, ToolFailure> {
    let raw = match args.get("cursor") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) => serde_json::from_str::<view::Cursor>(s).map_err(|e| {
            ToolFailure::BadParams(format!("\"cursor\" is not a valid cursor: {e}"))
        })?,
        Some(v @ Value::Object(_)) => {
            serde_json::from_value::<view::Cursor>(v.clone()).map_err(|e| {
                ToolFailure::BadParams(format!("\"cursor\" is not a valid cursor: {e}"))
            })?
        }
        Some(_) => {
            return Err(ToolFailure::BadParams(
                "\"cursor\" must be the string or object a previous page returned".into(),
            ))
        }
    };
    Ok(Some(raw))
}

fn string_list_arg(args: &Value, key: &str) -> Result<Option<Vec<String>>, ToolFailure> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| {
                i.as_str().map(str::to_string).ok_or_else(|| {
                    ToolFailure::BadParams(format!("{key:?} must be an array of strings"))
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(ToolFailure::BadParams(format!(
            "{key:?} must be an array of strings"
        ))),
    }
}

fn encode<T: serde::Serialize>(value: &T) -> Result<String, ToolFailure> {
    serde_json::to_string(value).map_err(|e| ToolFailure::Domain {
        code: "serialize_failed",
        message: e.to_string(),
    })
}

// ---- the three read tools ------------------------------------------------

/// `agentrec_log` → `Page<TurnSummary>` (P4b decision 12).
///
/// Deliberately NOT `log --json`, which emits protocol JSONL records; there
/// is no `--json` byte-parity to pin here and none is claimed. The payload is
/// the serialized [`view::Page`] the view returned, so a field added to
/// [`view::TurnSummary`] reaches the agent with no edit here.
///
/// `include_hidden` maps to [`view::TurnQuery::include_all`], the one knob
/// the view offers: it unhides git-operation turns AND turns superseded by a
/// retroactive merge.
///
/// **Renamed from `include_git` in E3** (E2 flagged the mismatch as its own
/// residual). E2 shipped the parameter as `include_git` with the description
/// "Include git-operation turns", which named half of what the knob does; the
/// widening was documented here but not on the wire, where the agent reads.
/// Of the two honest fixes — rename, or keep the name and correct the
/// description — the rename was chosen because a model keys on the parameter
/// NAME at least as much as on its description, so a name that under-promises
/// would keep misleading readers of `tools/list` who never read past it.
/// Renaming is free at this point: 2.2 has never shipped, the identifier was
/// confined to this file (E1's golden transcript asserts only that
/// descriptions are non-empty, so no test pinned the old text and none needed
/// amending), and no host holds a saved call. The alternative rejected in E2
/// stands rejected: a second git-only filter implemented here would put
/// ledger interpretation back in `cli/src`.
fn log_tool(root: &Path, args: &Value) -> Result<String, ToolFailure> {
    let q = view::TurnQuery {
        include_all: bool_arg(args, "include_hidden")?,
        limit: Some(bounded_limit(args, MAX_LOG_SUMMARIES)?),
        after: cursor_arg(args)?,
    };
    let page = open_view(root)?.list(&q).map_err(|e| cursor_failure(&e))?;
    encode(&page)
}

/// `agentrec_diff` → [`view::DiffResult`], byte-identical to `diff --json`
/// whenever the payload bound does not bind (`readcmds::diff` serializes the
/// same typed value with the same `serde_json::to_string`; neither side
/// renders).
///
/// The bound is applied by re-asking the view for a limited page rather than
/// by trimming the result here — the cursor must be minted by the same walk
/// that would mint it for any other paging caller, or it would not resolve.
fn diff_tool(root: &Path, args: &Value) -> Result<String, ToolFailure> {
    let budget = bounded_limit(args, MAX_DIFF_PAYLOAD_LINES)?;
    let mut q = view::DiffQuery {
        turn: required_str_arg(args, "turn")?,
        paths: string_list_arg(args, "paths")?,
        limit: None,
        after: cursor_arg(args)?,
    };
    let view = open_view(root)?;
    let full = view.diff(&q).map_err(|e| diff_failure(&e))?;

    // How many whole entries fit the payload bound. At least one always does
    // — a single file bigger than the budget must still be delivered, or a
    // pager sits on it forever.
    let mut used = 0usize;
    let mut admitted = 0usize;
    for file in &full.files.items {
        let cost = payload_lines(&file.state);
        if admitted > 0 && used + cost > budget {
            break;
        }
        used += cost;
        admitted += 1;
    }
    if admitted >= full.files.items.len() {
        // Everything fits: the unpaginated value, untouched. This is the
        // `--json` parity path.
        return encode(&full);
    }
    q.limit = Some(admitted);
    let bounded = view.diff(&q).map_err(|e| diff_failure(&e))?;
    encode(&bounded)
}

/// One file entry's contribution to the payload bound — see
/// [`MAX_DIFF_PAYLOAD_LINES`] for why this counts content lines and not
/// rendered unified-diff lines. States that carry no content still cost 1:
/// they occupy a row in the answer, and costing them 0 would let an unbounded
/// number of them into one page.
fn payload_lines(state: &view::FileDiffState) -> usize {
    match state {
        view::FileDiffState::Text { before, after, .. } => {
            before.lines().count() + after.lines().count()
        }
        view::FileDiffState::BaselineUnknown { after } => after.lines().count(),
        view::FileDiffState::Withheld
        | view::FileDiffState::Skipped { .. }
        | view::FileDiffState::Unresolvable { .. }
        | view::FileDiffState::Binary { .. } => 1,
    }
    .max(1)
}

fn diff_failure(e: &view::DiffError) -> ToolFailure {
    match e {
        view::DiffError::Cursor(c) => cursor_failure(c),
        view::DiffError::Lookup { err, .. } => ToolFailure::Domain {
            code: "turn_not_found",
            message: match err {
                view::LookupError::NoTurns => "no turns are recorded in this repository".into(),
                view::LookupError::Ambiguous { .. } => {
                    "that turn ref matches more than one recorded turn".into()
                }
                view::LookupError::Unknown => "no recorded turn matches that ref".into(),
            },
        },
        view::DiffError::Io(message) => ToolFailure::Domain {
            code: "io_error",
            message: message.clone(),
        },
    }
}

/// `agentrec_blame` → [`view::BlameResult`], byte-identical to
/// `blame --json`.
///
/// Gap honesty rides on the value itself and needs nothing added here: the
/// `no_turn_recording_gap` / `line_recording_gap` / `line_origin_gap` arms of
/// [`view::BlameState`] are internally tagged and carry NO `turn` field, so
/// "attribution stale — recording gap" reaches the agent as structure rather
/// than as prose it would have to parse (and a guessed attributor is
/// impossible to emit, not merely unlikely).
fn blame_tool(root: &Path, args: &Value) -> Result<String, ToolFailure> {
    let q = view::BlameQuery {
        path: required_str_arg(args, "path")?,
        line: positive_usize_arg(args, "line")?,
    };
    let result = open_view(root)?.blame(&q).map_err(|e| match e {
        view::BlameError::FileNotFound => ToolFailure::Domain {
            code: "file_not_found",
            message: "that path is not on disk and no recorded turn touches it".into(),
        },
        view::BlameError::LineOutOfRange { lines } => ToolFailure::Domain {
            code: "line_out_of_range",
            message: format!("that file has {lines} line(s)"),
        },
        view::BlameError::Io(message) => ToolFailure::Domain {
            code: "io_error",
            message,
        },
    })?;
    encode(&result)
}

// ---- E3: recall + status --------------------------------------------------

/// `agentrec_recall`'s server bound on `k`.
///
/// Not an arbitrary page size: [`view::RECALL_VERIFY_CAP`] is where the
/// rank-then-verify walk stops examining candidates, so a call asking for
/// more hits than this can never receive them — the hits are a subset of the
/// candidates walked. Bounding at the cap therefore removes no reachable
/// result, and it is the same figure the cursored path inside `view::recall`
/// already fetches at.
const MAX_RECALL_HITS: usize = view::RECALL_VERIFY_CAP;

fn mode_str(mode: config::McpDestructive) -> &'static str {
    match mode {
        config::McpDestructive::Off => "off",
        config::McpDestructive::Confirm => "confirm",
        config::McpDestructive::Auto => "auto",
    }
}

/// `.agentrec/config.toml`'s mtime, or `None` if it is absent or unstattable.
///
/// **Single-component on purpose.** Pairing mtime with a size or a hash would
/// make the comparison in [`status_tool`] pass for the wrong reason under a
/// mutation probe of either half, so the check stays exactly the one the plan
/// names. Its two error directions, both stated rather than papered over:
///
/// * *False negative* — a filesystem whose mtime granularity is coarser than
///   the interval between startup and the edit can report an unchanged stamp
///   for a real change. The effective mode is still the startup one either
///   way (nothing here ever reloads), so the failure is a missing note, never
///   a wrong mode.
/// * *False positive* — `touch`ing the file, or rewriting it with identical
///   bytes, reports restart-required. Conservative in the safe direction: the
///   note says "restart to pick up config changes", not "your mode changed".
fn config_mtime(root: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(crate::agentrec_dir(root).join("config.toml"))
        .and_then(|m| m.modified())
        .ok()
}

/// `agentrec_recall` → the FULL [`view::RecallPage`], honesty flags included
/// (delta decision 14).
///
/// The one 2.2 tool with no `--json` byte-parity to pin, and deliberately so:
/// `recall --json` serializes `page.items` alone (decision 3 of the memory
/// round), so there is no CLI contract carrying `capped`/`store_corrupt`/
/// `store_empty` for this to mirror. Withholding them from an agent would be
/// the same dishonesty the CLI's human renderer refuses — a corrupt store
/// would read to the agent as "no memories match". The CLI surface is
/// untouched by this; see [`view::RecallPage`]'s doc comment for the golden
/// pin that now stands in for the type's former unserializability.
///
/// Hits are hash-verified-fresh by construction — that filtering is
/// `memory::recall`'s, not re-implemented here.
///
/// **Two pagination facts, both measured here rather than assumed, because
/// E3 is the first caller in the repo's history to set
/// [`view::RecallQuery::after`] (its doc comment has been dead text until
/// now).**
///
/// 1. *A recall page never mints its own `next`.* `view::recall` mints a
///    continuation only on a call that ALREADY carried a cursor
///    (`fetch.from_cursor && end < hits.len()`), so a client's first page
///    always reports `next: null` no matter how many hits it left behind. The
///    `cursor` parameter is therefore reachable only by a caller that
///    constructs a `Cursor` itself. That is left as the view's behavior, not
///    worked around here — synthesizing a first cursor in this adapter would
///    be adapter-owned pagination, the seam P4/P5 closed — and the tool
///    metadata says so plainly instead of advertising a round-trip that does
///    not exist. Both halves are pinned by
///    `mcp.rs::e3::recall_cursor_wiring_round_trips_a_caller_built_cursor`.
/// 2. *The `after` path's staleness check is one-sided.* A memory that
///    becomes Fresh between two calls can sort ahead of the cursor and be
///    skipped silently — no error, no gap. That is documented on
///    `view::RecallQuery::after` and, from here, on the wire.
fn recall_tool(root: &Path, args: &Value) -> Result<String, ToolFailure> {
    let q = view::RecallQuery {
        query: required_str_arg(args, "query")?,
        k: positive_usize_arg(args, "k")?
            .unwrap_or(MAX_RECALL_HITS)
            .min(MAX_RECALL_HITS),
        after: cursor_arg(args)?,
    };
    let page = open_view(root)?.recall(&q).map_err(|e| match e {
        view::RecallError::Cursor(c) => cursor_failure(&c),
        view::RecallError::Io(message) => ToolFailure::Domain {
            code: "io_error",
            message,
        },
    })?;
    encode(&page)
}

/// The `agentrec_status` payload. Composed from typed values the read layer
/// already produces — no interpretation is re-implemented here.
#[derive(serde::Serialize)]
struct StatusPayload<'a> {
    /// [`view::RepositoryHealth`] verbatim, the same struct `status --json`
    /// flattens (P5). Serialized nested rather than flattened so the
    /// operational fields below cannot collide with a field health gains
    /// later.
    health: view::RepositoryHealth,
    /// `daemon::daemon_is_running` — the non-blocking `flock` probe on
    /// `.agentrec/daemon.lock` (D2) that `status`, `doctor` and `purge`
    /// already share. Not a second liveness notion; a pid check false-passes
    /// on pid recycling.
    daemon_running: bool,
    /// The effective agent-undo mode for THIS process: the startup value, not
    /// what is on disk now. See `restart_required`.
    agent_undo_mode: &'a str,
    /// The MCP revision negotiated by `initialize`, or `null` when no
    /// handshake has happened — `handle_line` deliberately does not enforce
    /// initialize-first ordering, so `null` here is a real, reachable state
    /// meaning "unnegotiated", never "unknown".
    protocol_revision: Option<&'a str>,
    /// `.agentrec/config.toml` changed since this process read it.
    restart_required: bool,
    /// Human-readable companion to `restart_required`; `null` when false.
    /// Emitted unconditionally (no `skip_serializing_if`) so an absent key
    /// can never be confused with a suppressed one.
    note: Option<&'a str>,
}

/// `agentrec_status` → [`view::RepositoryHealth`] plus this process's own
/// operational facts.
///
/// **The budget comes from `server.config`, not from a fresh
/// `config::load`.** `cmds::effective_store_budget_checked` re-reads the file
/// at call time; using it here would let `store_budget_bytes` hot-reload in
/// the very payload that reports the mode as needing a restart — two config
/// keys with two different reload semantics, in one response. Everything this
/// tool reports about configuration is the startup value.
///
/// `health()` is a pure read (P4): it never evicts, so a `tools/call` cannot
/// mutate the store.
fn status_tool(server: &Server) -> Result<String, ToolFailure> {
    let health = open_view(&server.root)?
        .health(server.config.store_budget_bytes)
        .map_err(|e| ToolFailure::Domain {
            code: "repo_error",
            message: e.to_string(),
        })?;
    let restart_required = config_mtime(&server.root) != server.config_stamp;
    encode(&StatusPayload {
        health,
        daemon_running: crate::daemon::daemon_is_running(&server.root),
        agent_undo_mode: mode_str(server.destructive_mode()),
        protocol_revision: server.negotiated_revision.as_deref(),
        restart_required,
        note: restart_required.then_some(
            ".agentrec/config.toml changed since this server started; the reported \
             agent-undo mode is the one loaded at startup. Restart the server to apply \
             the file's current contents.",
        ),
    })
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
            config_stamp: None,
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
