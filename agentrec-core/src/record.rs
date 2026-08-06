//! Protocol wire types (PROTOCOL §4–§5): signal lines, turn records, epoch
//! records, plus tolerant append-only log IO. Unknown fields are preserved by
//! never rewriting history and tolerated by serde defaults on read.

use crate::perms;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;

/// The schema major this binary implements, for both wire schemas the
/// protocol defines (PROTOCOL §4 signal `v`, §5 record `v`).
///
/// `v` is a bare integer with no minor component, so every distinct `v` IS a
/// distinct major: PROTOCOL §10 says "a major bump is a new schema", which
/// makes any other value a schema this binary does not implement rather than
/// an additive extension of this one. Additive change within the major is
/// carried by new *fields*, which serde's defaults tolerate — that half is
/// unaffected by this constant.
pub const SCHEMA_MAJOR: u32 = 1;

/// Deserializer for the `v` field of every wire type in this module. Refuses
/// any major other than [`SCHEMA_MAJOR`], so a record or signal from a
/// producer this binary predates can never be parsed *as if* it were v1 and
/// have its fields silently reinterpreted. These records gate `undo`, so a
/// misread future record is a wrong-bytes revert source.
///
/// Refusal is an ordinary serde error, deliberately: it lands in the same
/// channel every other unparseable line already uses, so no caller gains a
/// new error path to forget to handle. [`parse_log_line`] then separates the
/// two *causes* for the census (see [`ParsedLine::UnknownType`]).
fn de_schema_major<'de, D>(d: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = u32::deserialize(d)?;
    if v != SCHEMA_MAJOR {
        return Err(serde::de::Error::custom(format!(
            "unsupported schema major v={v} (this binary implements v={SCHEMA_MAJOR})"
        )));
    }
    Ok(v)
}

/// Emitter → recorder signal line (PROTOCOL §4). `ts` is unix milliseconds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignalEvent {
    /// Absent `v` defaults to [`SCHEMA_MAJOR`]: §4 makes `v` a writer MUST,
    /// but §10 constrains writers only and says nothing about a consumer
    /// meeting an absent one — §1's graceful degradation, and the legacy
    /// lines this default has always accepted, settle it as major 1.
    #[serde(default = "one", deserialize_with = "de_schema_major")]
    pub v: u32,
    pub ts: u64,
    pub tool: String,
    #[serde(default)]
    pub event: Option<String>, // "stop" (default) | "start"
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub transcript: Option<String>,
    /// Post-scrub prompt text, when the emitting hook has it (UserPromptSubmit).
    /// PROTOCOL §4 (row added at the 1.0 freeze, documenting shipped
    /// behavior). NOT gated on `event`: `cmds.rs::hook` reads the payload's
    /// `prompt` key whichever Claude Code hook fired, and `daemon.rs::
    /// signal_context` consumes it on `start` and `stop` alike — on a stop it
    /// becomes `observe_stop`'s `prompt_fallback` for a stop-only emitter.
    /// Unlike the additive fields below it has no `skip_serializing_if`, so
    /// it goes on the wire as an explicit `null` when absent; §4 pins
    /// explicit `null` as identical to absence.
    #[serde(default)]
    pub prompt: Option<String>,
    /// PROTOCOL §4 additive (D6 attribution): absolute paths the emitting tool
    /// itself wrote during the turn, populated by L2+ emitters on `stop`.
    /// `None` means "emitter did not declare", never "no files written" —
    /// consumers must not infer authorship for paths absent from the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_written: Option<Vec<String>>,
    /// PROTOCOL §4 additive (Phase 2 tail C1): a stable turn identity the
    /// EMITTER assigns, present on `start`/`stop` only. Motivating producer:
    /// Codex's own hook `turn_id` (docs/verify/codex-spike.md) — the daemon
    /// uses it to detect an emitter-resent start/stop (retry, or a daemon
    /// restart racing the emitter's own retry) by
    /// `(tool, event, session, emitter_turn)` instead of timing heuristics.
    /// Claude Code's hook emitter has no equivalent stable id today and
    /// omits this field entirely; `None` means "emitter did not declare",
    /// never "no upstream turn" — a recorder MUST fall back to today's
    /// existing start/stop matching whenever either side of a comparison
    /// lacks it (see `cli/src/daemon.rs`'s dedup + mismatch handling).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emitter_turn: Option<String>,
    /// PROTOCOL §4 additive (Phase 2 tail C2 fix 2): the model the emitting
    /// tool was running, when the emitter has it. Motivating producer:
    /// Codex's hook payload carries `model` on all three of its lifecycle
    /// events (`docs/verify/codex-spike.md`'s field inventory); `cli/src/
    /// hookcmds.rs::hook_codex` sets it on `UserPromptSubmit` only —
    /// mirroring `prompt`'s start-only posture, not `emitter_turn`'s
    /// carried-on-both one (see that module's doc for why). Claude Code's
    /// hook emitter has no such field and omits this entirely; its model
    /// attribution is a SEPARATE, pre-existing mechanism
    /// (`cli/src/daemon.rs::parse_transcript`, daemon-side transcript
    /// parsing) that this field does not replace. `None` means "emitter
    /// did not declare", never "no model" — a recorder falling back to
    /// transcript-derived attribution whenever this is absent is the
    /// existing, unchanged behavior (`cli/src/daemon.rs::signal_context`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Signal variant discriminator (PROTOCOL §4, additive). Absent/`None` on
    /// every existing turn-boundary signal (`start`/`stop`, keyed by `event`
    /// instead). Currently the only non-`None` value is `"memory-candidate"`
    /// (memory v1) — a fact-extraction hint that is NOT a turn boundary and
    /// MUST be routed away from `apply_signal`'s start/stop arms.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Memory-candidate payload: the extracted fact text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact: Option<String>,
    /// Memory-candidate payload: paths the fact should be pinned to. Paths
    /// only — the recorder hashes them at ingestion (PROTOCOL §4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pins: Option<Vec<String>>,
}

fn one() -> u32 {
    1
}

impl SignalEvent {
    pub fn is_start(&self) -> bool {
        self.event.as_deref() == Some("start")
    }

    /// True for a memory-candidate signal (PROTOCOL §4 additive). These carry
    /// no `event`, so callers MUST check this BEFORE treating a missing/non-
    /// "start" `event` as an implicit stop — a memory-candidate line is not a
    /// turn boundary at all and must never reach the stop arm.
    pub fn is_memory_candidate(&self) -> bool {
        self.kind.as_deref() == Some("memory-candidate")
    }
}

/// One file touched by a turn (PROTOCOL §5 file entry).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub op: String, // "create" | "modify" | "delete"
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skipped: bool, // over snapshot cap
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub withheld: bool, // secret-pattern file, never snapshotted
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub baseline_unknown: bool, // before unrecoverable (first seen post-change)
    /// Why `skipped` is set (PROTOCOL §5, additive, open string enum — unknown
    /// values MUST degrade to plain `skipped` behavior in consumers). `None`
    /// on every entry where `skipped` is false, and on logs written before
    /// this field existed. Permanent per-entry historical truth — see
    /// `skip_reason` in `agentrec-core::skip_reason` for the defined values
    /// and `cli/src/fmt.rs::skip_reason_text` for the one place that renders
    /// them. Distinct from (and must never be derived from, or derive)
    /// `state.json`'s `io_failed`/`snapshot_failures`, which stay operational
    /// and aggregate, driving the DEGRADED banner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
    /// PROTOCOL §5 additive, FROZEN at protocol 1.0 (2026-08-06) — the
    /// freeze added its §5 row (see the `after_synthesized` paragraph
    /// there); it landed in the Phase 2.0 P2 fix round under founder
    /// decision 2. `Some(true)` when `after` was DERIVED (e.g. applying an imported
    /// turn's `oldString`→`newString` substitution to a resolved-or-
    /// unresolved `before`) rather than observed directly from the source
    /// (a live daemon snapshot, or a transcript's own recorded `content`
    /// field). `None` on every entry where `after` is real observed bytes,
    /// including every live-recorded entry — so this stays byte-identical
    /// to the pre-this-field wire shape for every existing record.
    /// Consumers MUST NOT attribute a mismatch against a synthesized
    /// `after` to "human or external edit" — see `readcmds::modified_cause`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_synthesized: Option<bool>,
    /// Kind of the filesystem object at `path` AT SNAPSHOT TIME, when it is
    /// something other than an ordinary file (PROTOCOL §5, additive, open
    /// string enum — see [`link_kind`]). `None` on every ordinary-file entry
    /// and on every entry written before this field existed, so the wire
    /// shape of those stays byte-identical.
    ///
    /// The only value this implementation writes is [`link_kind::SYMLINK`]
    /// (writer: the daemon, `cli/src/daemon.rs`). It means the entry's
    /// `before`/`after` hashes address the **link target string**, not file
    /// content — the recorder deliberately does not follow symlinks
    /// (IMPLEMENTATION.md AC B5).
    ///
    /// Consumers MUST refuse to *act* on an entry carrying any non-`None`
    /// value, including an unknown future one — refuse-to-act, never
    /// refuse-to-parse. Treating a target string as content replaces the
    /// link with a text file, and writing to the path follows the link and
    /// truncates a file that was never in the plan (red team round 2, F2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_kind: Option<String>,
    /// Per-file attribution (PROTOCOL §5, additive, open string enum).
    /// **Writer-optional, and never written by this code** — reserved for
    /// the D6 transcript-correlation producer, which defines the value set.
    /// Every entry this workspace constructs leaves it `None`, so it is
    /// absent from the wire and existing records are unaffected. Consumers
    /// MUST tolerate any value (including unknown ones) and MUST NOT derive
    /// undo safety from it — undo safety keys on `modified-since`
    /// (PROTOCOL §5) and on the refusal gates, never on attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
}

/// Open string enum of [`FileEntry::link_kind`] values (PROTOCOL §5).
/// Unknown/future values MUST make a consumer refuse to ACT on the entry,
/// never refuse to parse the line — so never match exhaustively on these,
/// and never gate a destructive operation on recognizing one.
pub mod link_kind {
    /// The path was a symbolic link at snapshot time; the entry's hashes
    /// address the link target string, not file content.
    pub const SYMLINK: &str = "symlink";
}

/// Open string enum of [`FileEntry::skipped_reason`] values (PROTOCOL §5).
/// Unknown/absent values MUST degrade to plain `skipped` behavior in every
/// consumer — never match exhaustively on these.
pub mod skip_reason {
    /// Content exceeded the per-blob size cap (`MAX_SNAPSHOT_BYTES`).
    pub const OVER_CAP: &str = "over_cap";
    /// The snapshot write itself failed at record time (`PutResult::IoError`).
    pub const IO_FAILED: &str = "io_failed";
    /// The file could not be read at record time (`fs::read` / `read_link` error).
    pub const UNREADABLE: &str = "unreadable";
    /// RESERVED, no producer yet — a future rate/size-demotion feature will
    /// emit this. Reserved now so the frozen protocol has room (PROTOCOL §5).
    #[allow(dead_code)]
    pub const POLICY: &str = "policy";
}

/// A log line: turn or epoch (PROTOCOL §5). `type` defaults to "turn".
// Turns dominate the log and are always heap-allocated behind a Vec on read;
// the size gap to the tiny Epoch variant is immaterial here.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LogRecord {
    Turn(TurnRecord),
    Epoch(EpochRecord),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnRecord {
    /// Absent `v` defaults to [`SCHEMA_MAJOR`] — same reading as
    /// [`SignalEvent::v`], and the same legacy lines depend on it.
    #[serde(default = "one", deserialize_with = "de_schema_major")]
    pub v: u32,
    pub id: String,
    pub grade: String, // "rich" | "bare"
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    pub started: String,
    pub ended: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_excerpt: Option<String>,
    /// Ids of earlier bare turns absorbed by this rich turn (retroactive merge,
    /// PROTOCOL §4). Consumers must treat merged turns as superseded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merges: Vec<String>,
    /// Import provenance (Phase 2.0 P2). PROTOCOL §5 additive, FROZEN at
    /// protocol 1.0 (2026-08-06) — the freeze added the §5 rows for this
    /// field and for `files_complete` below, documenting both as already
    /// shipped. `None` on every live-recorded record, so existing lines
    /// stay byte-identical — neither this nor `files_complete` is emitted
    /// unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported: Option<bool>,
    /// `Some(false)` on imported turns: their file list is known-partial
    /// (opaque tool calls and non-file activity are not captured). Never
    /// `Some(true)` today — reserved for a future importer that can attest
    /// completeness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_complete: Option<bool>,
    /// Which surface EXECUTED an undo's writes (PROTOCOL §5 additive, delta
    /// decision 11): [`UndoOrigin::CLI`] or [`UndoOrigin::MCP`]. Set only on
    /// undo turns — the ones this implementation writes with
    /// `tool: "agentrec"` — and `None` on every other turn, so nothing else
    /// on the wire moves.
    ///
    /// Read through [`TurnRecord::origin`], never bare: an absent value means
    /// `cli`, which is what every undo turn written before this field existed
    /// is. Absent is therefore NOT "unknown" and NOT a third state.
    ///
    /// It names the surface that *wrote*, never the one that *asked*: an
    /// `agentrec approve` of an undo an agent requested over MCP is `cli`,
    /// because a human at a keyboard performed it. The requesting provenance
    /// lives in `.agentrec/undo-requests.jsonl`, not here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    pub files: Vec<FileEntry>,
}

/// The two values [`TurnRecord::origin`] may carry (PROTOCOL §5).
///
/// An enum rather than two `&str` constants at the call sites, because the
/// whole point of the field is that four `append_undo_turn` call sites across
/// two transports agree; a typo in a string literal is exactly the drift the
/// discriminator exists to measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UndoOrigin {
    /// `agentrec undo --confirm` or `agentrec approve` — a human executed it.
    Cli,
    /// The MCP `agentrec_undo` `execute` action — an agent spent a token.
    Mcp,
}

impl UndoOrigin {
    /// The wire value. `TurnRecord::CLI`/`MCP` name the same two strings for
    /// readers, which do not have this enum.
    pub fn as_str(self) -> &'static str {
        match self {
            UndoOrigin::Cli => TurnRecord::CLI,
            UndoOrigin::Mcp => TurnRecord::MCP,
        }
    }
}

impl TurnRecord {
    /// Wire value for [`UndoOrigin::Cli`].
    pub const CLI: &'static str = "cli";
    /// Wire value for [`UndoOrigin::Mcp`].
    pub const MCP: &'static str = "mcp";

    /// The effective `origin`, applying PROTOCOL §5's absent-means-`cli`
    /// default. The ONE place that default lives: reading `self.origin`
    /// directly and matching on `Some("cli")` would silently exclude every
    /// pre-F5 undo turn, which is the misreading the row this field feeds
    /// (delta decision 11's post-ship undo counts) would be destroyed by.
    ///
    /// Returns an unrecognized value verbatim rather than folding it into
    /// `cli` — §10 says consumers tolerate unknown values, and quietly
    /// relabelling a future surface as a human one would be worse than
    /// surfacing a string the caller does not know.
    pub fn origin(&self) -> &str {
        self.origin.as_deref().unwrap_or(Self::CLI)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpochRecord {
    /// Absent `v` defaults to [`SCHEMA_MAJOR`] — see [`SignalEvent::v`].
    #[serde(default = "one", deserialize_with = "de_schema_major")]
    pub v: u32,
    pub event: String, // "start" | "stop"
    pub ts: String,    // RFC 3339
    /// D51 (PROTOCOL §5, additive): how many turn-boundary signal lines the
    /// recorder found in the pre-startup gap and DROPPED without feeding them
    /// to the engine (D7 — replaying a stale start/stop would mint an empty
    /// turn misdated to daemon boot). A count, never the dropped content:
    /// nothing about the dropped signals is captured or persisted beyond how
    /// many there were.
    ///
    /// Meaningful on `event: "start"` only — a `stop` epoch is a clean
    /// shutdown, which has no gap to scan — and omitted from the wire
    /// whenever it is `0`, so every pre-D51 epoch line round-trips
    /// byte-identically (`epoch_dropped_signals_roundtrips_and_zero_stays_absent`).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub dropped_signals: u32,
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

/// Append one record; creates parents; never rewrites (append-only invariant).
/// Fsyncs before returning (D34): turn-close and epoch appends must survive a
/// kill-9 immediately after this call returns Ok.
pub fn append_log(path: &Path, record: &LogRecord) -> Result<(), String> {
    let line = serde_json::to_string(record).map_err(|e| e.to_string())?;
    append_line_synced(path, &line)
}

/// Append a pre-serialized, `\n`-terminated JSON line and fsync before
/// returning. Shared durability primitive (D34) for any append-only store in
/// the workspace that needs kill-9-safe closes — `append_log` is one caller;
/// `memory.rs`'s `append_memory` is another.
pub fn append_line_synced(path: &Path, line: &str) -> Result<(), String> {
    let file = open_append(path, line)?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}

/// Append a pre-serialized JSON line (also used for the signal inbox). Creates
/// parents; append-only, one `\n`-terminated line per call.
///
/// Deliberately NOT fsynced (D34): this is the hot path for the signal inbox,
/// and signals are reconstructible from re-emission, unlike turn closes.
pub fn append_log_line(path: &Path, line: &str) -> Result<(), String> {
    open_append(path, line)?;
    Ok(())
}

/// Shared open+write for both append paths. Creates parents; writes `line` as
/// a single `\n`-terminated buffer (one write syscall helps append atomicity
/// across processes). Returns the open file so callers may fsync it.
fn open_append(path: &Path, line: &str) -> Result<fs::File, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        perms::lock_dir(parent);
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    // umask-independent (D37): OpenOptions' create mode is masked by umask,
    // so lock down post-create every time; idempotent per append.
    perms::lock_file(path);
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    file.write_all(buf.as_bytes()).map_err(|e| e.to_string())?;
    Ok(file)
}

/// The `type` tags [`LogRecord`] knows. Used to tell "a record kind this
/// binary predates" apart from "a kind we know, written malformed" — the two
/// are indistinguishable by parse failure alone, and conflating them makes a
/// census that asserts a producer exists on evidence of corruption.
pub const KNOWN_RECORD_TYPES: [&str; 2] = ["turn", "epoch"];

/// What one `log.jsonl` line turned out to be.
// Same rationale as `LogRecord`: the record-bearing variant dominates real
// logs and is consumed straight into a Vec, so the gap to the unit variants
// is immaterial.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum ParsedLine {
    Record(LogRecord),
    /// Well-formed JSON declaring a schema this binary does not implement:
    /// either a `type` tag it does not know, or a `v` major other than
    /// [`SCHEMA_MAJOR`] (PROTOCOL §10). A newer producer is allowed to write
    /// both; consumers tolerate them. The variant name predates the version
    /// half and is kept because [`crate::view::Ledger`] surfaces its count as
    /// a public field.
    ///
    /// This bucket, not [`ParsedLine::Unparsed`], because the two counters
    /// prescribe different user actions: an unimplemented schema means
    /// "upgrade agentrec", torn JSON means "your log took damage".
    UnknownType,
    /// Not interpretable at all — a torn tail line after a crash, or a
    /// known record kind written malformed.
    Unparsed,
    Blank,
}

/// Classify one line. The single parser: both [`load_log`] and
/// `view::load_ledger` go through here, so the records a reader gets and the
/// census of what it skipped can never disagree.
pub fn parse_log_line(line: &str) -> ParsedLine {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ParsedLine::Blank;
    }
    // `type` defaults to turn for lines written before epochs existed.
    if let Ok(rec) = serde_json::from_str::<LogRecord>(trimmed) {
        return ParsedLine::Record(rec);
    }
    // The bare-TurnRecord fallback exists only for legacy lines that predate
    // the `type` tag entirely (C6). A line that DOES have a `type` field —
    // just one `LogRecord` doesn't recognize, e.g. a future additive record
    // kind — must never be coerced into a turn: serde ignores unknown fields
    // by default, so a `type:"future_thing"` line with turn-shaped fields
    // would otherwise silently misparse.
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return ParsedLine::Unparsed;
    };
    // A line declaring a schema major this binary does not implement was
    // already refused above by `de_schema_major` — it is not a Record and
    // never will be. All that is decided here is WHICH census it joins, and
    // it is the newer-producer one, not corruption (see `UnknownType`).
    //
    // Keyed on `as_u64` specifically. `v` is an int in both §4 and §5, so a
    // string/float/negative/null `v` declares no version at all; such a line
    // falls through to the `type` match below and lands in `Unparsed` — the
    // same rule, for the same reason, as the non-string `type` tag directly
    // below: a malformed field is evidence of corruption, never evidence
    // that a producer exists.
    if let Some(major) = value.get("v").and_then(|x| x.as_u64()) {
        if major != u64::from(SCHEMA_MAJOR) {
            return ParsedLine::UnknownType;
        }
    }
    // Keyed on PRESENCE of `type`, not on it being a string: a line tagged
    // `"type": 5` is still a line that carries a type, and coercing it into a
    // turn is exactly the misparse this guard exists to prevent.
    match value.get("type") {
        None => match serde_json::from_str::<TurnRecord>(trimmed) {
            Ok(turn) => ParsedLine::Record(LogRecord::Turn(turn)),
            Err(_) => ParsedLine::Unparsed,
        },
        Some(tag) => match tag.as_str() {
            // A kind this binary predates. Tolerated and counted.
            Some(t) if !KNOWN_RECORD_TYPES.contains(&t) => ParsedLine::UnknownType,
            // A kind we know, written malformed — corruption, not a newer
            // producer. A non-string tag is no kind at all, and lands here
            // for the same reason: it is not evidence a producer exists.
            _ => ParsedLine::Unparsed,
        },
    }
}

/// Load all parseable records; torn/corrupt lines are skipped, never fatal
/// (a bad line must not wipe history — lesson inherited from Sutra).
pub fn load_log(path: &Path) -> Vec<LogRecord> {
    let Ok(file) = fs::File::open(path) else {
        return vec![];
    };
    let reader = std::io::BufReader::new(file);
    let mut out = vec![];
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if let ParsedLine::Record(rec) = parse_log_line(&line) {
            out.push(rec);
        }
    }
    out
}

/// Parse signal-file text from an offset; skips garbage lines (PROTOCOL §4
/// tolerance). Returns events in file order.
///
/// A line declaring a `v` major other than [`SCHEMA_MAJOR`] is skipped here
/// too (`de_schema_major`). Skipping is the conservative direction: a signal
/// is a turn *boundary*, so misreading a future one mis-cuts a turn and
/// mis-attributes every file in it. The recorder falls back to its
/// quiet-window heuristic, which produces an honestly-unattributed `bare`
/// turn instead of a confidently wrong `rich` one.
pub fn parse_signals(text: &str) -> Vec<SignalEvent> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            serde_json::from_str::<SignalEvent>(line).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_parsing_tolerates_garbage_and_unknown_fields() {
        let text = concat!(
            "not json\n",
            "{\"v\":1,\"ts\":5000,\"tool\":\"claude-code\",\"event\":\"start\",\"prompt\":\"fix it\",\"future_field\":42}\n",
            "{\"ts\":6000,\"tool\":\"codex\"}\n",
        );
        let sigs = parse_signals(text);
        assert_eq!(sigs.len(), 2);
        assert!(sigs[0].is_start());
        assert_eq!(sigs[0].prompt.as_deref(), Some("fix it"));
        assert!(!sigs[1].is_start()); // event defaults to stop
        assert_eq!(sigs[1].v, 1); // v defaults
    }

    #[test]
    fn signal_files_written_roundtrips_and_absence_stays_absent() {
        // Pre-field line: parses with None, and re-serializing does NOT mint
        // the key (absence means "emitter did not declare" — PROTOCOL §4).
        let old = "{\"v\":1,\"ts\":5000,\"tool\":\"claude-code\",\"event\":\"stop\"}";
        let sig: SignalEvent = serde_json::from_str(old).unwrap();
        assert_eq!(sig.files_written, None);
        let re = serde_json::to_string(&sig).unwrap();
        assert!(!re.contains("files_written"));

        // Field-carrying line round-trips with the list intact.
        let new = concat!(
            "{\"v\":1,\"ts\":5000,\"tool\":\"claude-code\",\"event\":\"stop\",",
            "\"files_written\":[\"/repo/a.rs\",\"/repo/b.rs\"]}"
        );
        let sig: SignalEvent = serde_json::from_str(new).unwrap();
        assert_eq!(
            sig.files_written.as_deref(),
            Some(&["/repo/a.rs".to_string(), "/repo/b.rs".to_string()][..])
        );
        let re = serde_json::to_string(&sig).unwrap();
        let back: SignalEvent = serde_json::from_str(&re).unwrap();
        assert_eq!(back.files_written, sig.files_written);

        // An empty declared list is distinct from no declaration and survives.
        let empty = "{\"v\":1,\"ts\":5000,\"tool\":\"codex\",\"files_written\":[]}";
        let sig: SignalEvent = serde_json::from_str(empty).unwrap();
        assert_eq!(sig.files_written.as_deref(), Some(&[][..]));
    }

    #[test]
    fn signal_emitter_turn_roundtrips_and_absence_stays_absent() {
        // Pre-field line (every Claude Code hook payload today): parses with
        // None, and re-serializing does NOT mint the key — this is the wire
        // half of AC-C1's byte-identical claim (the daemon-side half is in
        // `cli/tests/emitter_turn.rs`).
        let old = "{\"v\":1,\"ts\":5000,\"tool\":\"claude-code\",\"event\":\"stop\"}";
        let sig: SignalEvent = serde_json::from_str(old).unwrap();
        assert_eq!(sig.emitter_turn, None);
        let re = serde_json::to_string(&sig).unwrap();
        assert!(!re.contains("emitter_turn"));

        // Field-carrying line round-trips intact.
        let new = concat!(
            "{\"v\":1,\"ts\":5000,\"tool\":\"codex\",\"event\":\"start\",",
            "\"emitter_turn\":\"turn_abc123\"}"
        );
        let sig: SignalEvent = serde_json::from_str(new).unwrap();
        assert_eq!(sig.emitter_turn.as_deref(), Some("turn_abc123"));
        let re = serde_json::to_string(&sig).unwrap();
        let back: SignalEvent = serde_json::from_str(&re).unwrap();
        assert_eq!(back.emitter_turn, sig.emitter_turn);
    }

    /// D51 wire half: the type-level byte-identity proof for every epoch line
    /// written before this field existed.
    #[test]
    fn epoch_dropped_signals_roundtrips_and_zero_stays_absent() {
        // Pre-D51 line: parses with 0, and re-serializing is byte-identical —
        // the key is never minted.
        let old = "{\"v\":1,\"event\":\"start\",\"ts\":\"2026-07-05T00:00:00.000Z\"}";
        let ep: EpochRecord = serde_json::from_str(old).unwrap();
        assert_eq!(ep.dropped_signals, 0);
        assert_eq!(serde_json::to_string(&ep).unwrap(), old);

        // An explicit zero on the wire is tolerated and normalizes away.
        let zero = "{\"v\":1,\"event\":\"start\",\"ts\":\"2026-07-05T00:00:00.000Z\",\"dropped_signals\":0}";
        let ep: EpochRecord = serde_json::from_str(zero).unwrap();
        assert_eq!(serde_json::to_string(&ep).unwrap(), old);

        // Nonzero round-trips intact.
        let new = "{\"v\":1,\"event\":\"start\",\"ts\":\"2026-07-05T00:00:00.000Z\",\"dropped_signals\":3}";
        let ep: EpochRecord = serde_json::from_str(new).unwrap();
        assert_eq!(ep.dropped_signals, 3);
        assert_eq!(serde_json::to_string(&ep).unwrap(), new);
    }

    #[test]
    fn signal_model_roundtrips_and_absence_stays_absent() {
        // Pre-field line (every existing signal, and every Claude Code hook
        // payload today): parses with None, and re-serializing does NOT mint
        // the key.
        let old = "{\"v\":1,\"ts\":5000,\"tool\":\"claude-code\",\"event\":\"stop\"}";
        let sig: SignalEvent = serde_json::from_str(old).unwrap();
        assert_eq!(sig.model, None);
        let re = serde_json::to_string(&sig).unwrap();
        assert!(!re.contains("\"model\""));

        // Field-carrying line round-trips intact.
        let new = concat!(
            "{\"v\":1,\"ts\":5000,\"tool\":\"codex\",\"event\":\"start\",",
            "\"model\":\"gpt-5.6-terra\"}"
        );
        let sig: SignalEvent = serde_json::from_str(new).unwrap();
        assert_eq!(sig.model.as_deref(), Some("gpt-5.6-terra"));
        let re = serde_json::to_string(&sig).unwrap();
        let back: SignalEvent = serde_json::from_str(&re).unwrap();
        assert_eq!(back.model, sig.model);
    }

    #[test]
    fn signal_stream_with_files_written_keeps_existing_routing() {
        // Conformance: a files_written-carrying stop interleaved with legacy
        // lines and a memory-candidate parses as before — the field changes
        // no variant discrimination and no line is dropped or re-routed.
        let text = concat!(
            "{\"v\":1,\"ts\":1000,\"tool\":\"claude-code\",\"event\":\"start\"}\n",
            "{\"v\":1,\"ts\":2000,\"tool\":\"claude-code\",\"event\":\"stop\",",
            "\"files_written\":[\"/repo/src/lib.rs\"]}\n",
            "{\"v\":1,\"ts\":3000,\"tool\":\"claude-code\",\"type\":\"memory-candidate\",",
            "\"fact\":\"uses pnpm\",\"pins\":[\"package.json\"]}\n",
        );
        let sigs = parse_signals(text);
        assert_eq!(sigs.len(), 3);
        assert!(sigs[0].is_start() && sigs[0].files_written.is_none());
        assert!(!sigs[1].is_start());
        assert_eq!(
            sigs[1].files_written.as_deref(),
            Some(&["/repo/src/lib.rs".to_string()][..])
        );
        assert!(sigs[2].is_memory_candidate());
        assert!(sigs[2].files_written.is_none());
    }

    #[test]
    fn log_roundtrip_with_epoch_and_corrupt_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("log.jsonl");
        let turn = TurnRecord {
            v: 1,
            id: "t_TEST".into(),
            grade: "rich".into(),
            truncated: false,
            started: "2026-07-05T00:00:00.000Z".into(),
            ended: "2026-07-05T00:00:01.000Z".into(),
            tool: Some("claude-code".into()),
            model: None,
            session: None,
            root: "/repo".into(),
            prompt_ref: None,
            prompt_excerpt: Some("add rate limiting".into()),
            merges: vec![],
            imported: None,
            files_complete: None,
            origin: None,
            files: vec![FileEntry {
                path: "src/a.rs".into(),
                before: None,
                after: Some("sha256:aa".into()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
                after_synthesized: None,
                link_kind: None,
                attribution: None,
            }],
        };
        append_log(&path, &LogRecord::Turn(turn)).unwrap();
        std::fs::write(
            &path,
            format!(
                "{}{}",
                std::fs::read_to_string(&path).unwrap(),
                "{\"type\":\"turn\",\"id\":\"torn\n"
            ),
        )
        .unwrap();
        append_log(
            &path,
            &LogRecord::Epoch(EpochRecord {
                v: 1,
                event: "start".into(),
                ts: "2026-07-05T00:00:02.000Z".into(),
                dropped_signals: 0,
            }),
        )
        .unwrap();
        let records = load_log(&path);
        assert_eq!(records.len(), 2); // torn line skipped
        assert!(matches!(records[0], LogRecord::Turn(_)));
        assert!(matches!(records[1], LogRecord::Epoch(_)));
    }

    #[test]
    fn optional_flags_omitted_when_false() {
        let entry = FileEntry {
            path: "a".into(),
            before: None,
            after: None,
            op: "delete".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
            link_kind: None,
            attribution: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("skipped"));
        assert!(!json.contains("withheld"));
    }

    // SR1: a record WITHOUT `skipped_reason` at all (pre-this-round wire
    // shape) deserializes fine, and a `None` entry serializes byte-identical
    // to the pre-change output — the additive-field promise PROTOCOL.md makes
    // for every wire type.
    #[test]
    fn skipped_reason_golden_roundtrip() {
        // Old-shape JSON, no `skipped_reason` key at all.
        let old_shape =
            r#"{"path":"a.rs","before":null,"after":null,"op":"modify","skipped":true}"#;
        let entry: FileEntry = serde_json::from_str(old_shape).unwrap();
        assert_eq!(
            entry.skipped_reason, None,
            "absent field must default to None"
        );
        assert!(entry.skipped);

        // A `None`-reason entry serializes byte-identical to the pre-change
        // shape: no `skipped_reason` key appears at all.
        let entry_none = FileEntry {
            path: "a.rs".into(),
            before: None,
            after: None,
            op: "modify".into(),
            skipped: true,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
            link_kind: None,
            attribution: None,
        };
        let json = serde_json::to_string(&entry_none).unwrap();
        assert_eq!(
            json, old_shape,
            "None skipped_reason must not appear on the wire"
        );

        // A `Some`-reason entry round-trips exactly.
        let entry_some = FileEntry {
            skipped_reason: Some(skip_reason::OVER_CAP.to_string()),
            ..entry_none
        };
        let json2 = serde_json::to_string(&entry_some).unwrap();
        assert!(
            json2.contains(r#""skipped_reason":"over_cap""#),
            "json: {json2}"
        );
        let back: FileEntry = serde_json::from_str(&json2).unwrap();
        assert_eq!(back.skipped_reason.as_deref(), Some(skip_reason::OVER_CAP));
    }

    // F2 wire shape, half 1: BOTH new fields absent from the wire when
    // `None`, so every record written before they existed is byte-identical
    // — and an old-shape line still parses with both defaulting to `None`.
    #[test]
    fn link_kind_and_attribution_absent_when_none() {
        let old_shape =
            r#"{"path":"a.rs","before":null,"after":null,"op":"modify","skipped":true}"#;
        let entry: FileEntry = serde_json::from_str(old_shape).unwrap();
        assert_eq!(entry.link_kind, None);
        assert_eq!(entry.attribution, None);

        let json = serde_json::to_string(&entry).unwrap();
        assert_eq!(
            json, old_shape,
            "None link_kind/attribution must not appear on the wire"
        );
        assert!(!json.contains("link_kind"));
        assert!(!json.contains("attribution"));
    }

    // Half 2: `Some` values survive a full round trip, including a value
    // this binary never writes and does not recognize. Parsing MUST succeed
    // — the refusal is the consumer's job (refuse-to-act, not
    // refuse-to-parse); the acting half is
    // `readcmds::build_plan_refuses_unknown_link_kind_value`.
    #[test]
    fn link_kind_and_attribution_round_trip_including_unknown_values() {
        let wire = r#"{"path":"a.rs","before":null,"after":null,"op":"modify","link_kind":"junction","attribution":"agent:claude/tool-call-7"}"#;
        let entry: FileEntry = serde_json::from_str(wire).unwrap();
        assert_eq!(entry.link_kind.as_deref(), Some("junction"));
        assert_eq!(
            entry.attribution.as_deref(),
            Some("agent:claude/tool-call-7")
        );
        assert_eq!(serde_json::to_string(&entry).unwrap(), wire);

        let sym = FileEntry {
            link_kind: Some(link_kind::SYMLINK.to_string()),
            ..entry
        };
        let json = serde_json::to_string(&sym).unwrap();
        assert!(json.contains(r#""link_kind":"symlink""#), "json: {json}");
        let back: FileEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.link_kind.as_deref(), Some(link_kind::SYMLINK));
    }

    #[test]
    fn append_log_fsyncs_and_coexists_with_signal_line() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("log.jsonl");
        let signal_path = tmp.path().join("signal.jsonl");

        let turn = TurnRecord {
            v: 1,
            id: "t_FSYNC".into(),
            grade: "rich".into(),
            truncated: false,
            started: "2026-07-05T00:00:00.000Z".into(),
            ended: "2026-07-05T00:00:01.000Z".into(),
            tool: Some("claude-code".into()),
            model: None,
            session: None,
            root: "/repo".into(),
            prompt_ref: None,
            prompt_excerpt: None,
            merges: vec![],
            imported: None,
            files_complete: None,
            origin: None,
            files: vec![],
        };
        append_log(&log_path, &LogRecord::Turn(turn)).unwrap();

        let signal_line = "{\"v\":1,\"ts\":7000,\"tool\":\"claude-code\",\"event\":\"stop\"}";
        append_log_line(&signal_path, signal_line).unwrap();

        let records = load_log(&log_path);
        assert_eq!(records.len(), 1);
        match &records[0] {
            LogRecord::Turn(t) => assert_eq!(t.id, "t_FSYNC"),
            LogRecord::Epoch(_) => panic!("expected turn record"),
        }

        let signal_text = std::fs::read_to_string(&signal_path).unwrap();
        assert!(signal_text.contains(signal_line));
    }

    // D37: append_log/append_log_line must land at mode 0600 regardless of
    // the process umask (the default test umask would otherwise yield 644).
    #[cfg(unix)]
    #[test]
    fn append_writes_land_at_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("log.jsonl");
        let signal_path = tmp.path().join("signal.jsonl");

        append_log(
            &log_path,
            &LogRecord::Epoch(EpochRecord {
                v: 1,
                event: "start".into(),
                ts: "2026-07-05T00:00:00.000Z".into(),
                dropped_signals: 0,
            }),
        )
        .unwrap();
        append_log_line(&signal_path, "{\"v\":1,\"ts\":1,\"tool\":\"claude-code\"}").unwrap();

        for path in [&log_path, &signal_path] {
            let mode = fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{}: expected 0600", path.display());
        }
    }

    // C6: a line with an unrecognized non-empty `type` (a future additive
    // record kind) must never be coerced into a turn via the legacy
    // bare-TurnRecord fallback, even when its other fields are turn-shaped.
    // A genuinely type-less legacy line (pre-epoch) still parses.
    #[test]
    fn unknown_type_line_not_coerced_into_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("log.jsonl");
        let future_line = concat!(
            "{\"type\":\"future_thing\",\"id\":\"t_FUTURE\",\"grade\":\"rich\",",
            "\"started\":\"2026-07-05T00:00:00.000Z\",\"ended\":\"2026-07-05T00:00:01.000Z\",",
            "\"root\":\"/repo\",\"files\":[]}"
        );
        let legacy_line = concat!(
            "{\"v\":1,\"id\":\"t_LEGACY\",\"grade\":\"rich\",",
            "\"started\":\"2026-07-05T00:00:00.000Z\",\"ended\":\"2026-07-05T00:00:01.000Z\",",
            "\"root\":\"/repo\",\"files\":[]}"
        );
        fs::write(&path, format!("{future_line}\n{legacy_line}\n")).unwrap();

        let records = load_log(&path);
        assert_eq!(records.len(), 1); // only the type-less legacy line parses
        match &records[0] {
            LogRecord::Turn(t) => assert_eq!(t.id, "t_LEGACY"),
            LogRecord::Epoch(_) => panic!("expected turn record"),
        }
    }

    // AC1 (clm_3V5KN3YSKEEWYVQ03NFRVCQJ6F, P2): a live-recorded turn
    // (`imported`/`files_complete` both `None`) serializes byte-identically
    // to its pre-P2 wire form — no new key appears at all. `old_shape` below
    // is not an invented literal: it is the exact field set/order
    // `TurnRecord` had at parent commit `5ea946b` (`git show
    // 5ea946b:agentrec-core/src/record.rs`), confirmed before this field was
    // added — this diff adds ONLY the two new fields between `merges` and
    // `files`, reordering nothing else, so `old_shape` is provably what this
    // exact struct used to emit for these values.
    #[test]
    fn imported_fields_absent_on_live_turn_keeps_pre_p2_wire_shape_byte_identical() {
        let old_shape = concat!(
            "{\"v\":1,\"id\":\"t_GOLD\",\"grade\":\"rich\",",
            "\"started\":\"2026-07-05T00:00:00.000Z\",\"ended\":\"2026-07-05T00:00:01.000Z\",",
            "\"tool\":\"claude-code\",\"session\":\"s1\",\"root\":\"/repo\",",
            "\"prompt_excerpt\":\"add rate limiting\",",
            "\"files\":[{\"path\":\"src/a.rs\",\"before\":null,\"after\":\"sha256:aa\",\"op\":\"create\"}]}"
        );

        // Old-shape JSON deserializes fine (additive-field tolerance).
        let turn: TurnRecord = serde_json::from_str(old_shape).unwrap();
        assert_eq!(turn.imported, None, "absent field must default to None");
        assert_eq!(
            turn.files_complete, None,
            "absent field must default to None"
        );

        // A live-recorded record (both fields None) re-serializes to
        // EXACTLY `old_shape` — the additive-field promise this whole file
        // makes for every wire type, now proven for `imported`/
        // `files_complete` specifically.
        let json = serde_json::to_string(&turn).unwrap();
        assert_eq!(
            json, old_shape,
            "a live (non-imported) turn must not gain any new wire key"
        );
        assert!(!json.contains("imported"));
        assert!(!json.contains("files_complete"));
    }

    // AC1's other half: an IMPORTED turn (P2) emits both new keys, and in
    // the frozen position (immediately after `merges`, before `files`).
    #[test]
    fn imported_turn_emits_both_new_keys_between_merges_and_files() {
        let turn = TurnRecord {
            v: 1,
            id: "t_imp_abc".into(),
            grade: "rich".into(),
            truncated: false,
            started: "2026-07-05T00:00:00.000Z".into(),
            ended: "2026-07-05T00:00:01.000Z".into(),
            tool: Some("claude".into()),
            model: None,
            session: Some("s1".into()),
            root: "/repo".into(),
            prompt_ref: None,
            prompt_excerpt: None,
            // Test nit (P2 fix round): `merges: vec![]` is
            // `skip_serializing_if`'d away, so its own wire position could
            // never be observed by this test — a non-empty `merges` is
            // required to actually prove `imported`/`files_complete` land
            // AFTER it, not merely before `files`.
            merges: vec!["t_MERGED".into()],
            imported: Some(true),
            files_complete: Some(false),
            origin: None,
            files: vec![],
        };
        let json = serde_json::to_string(&turn).unwrap();
        let merges_pos = json
            .find("\"merges\":[\"t_MERGED\"]")
            .expect("merges key present");
        let files_pos = json.find("\"files\":[]").unwrap();
        let imported_pos = json
            .find("\"imported\":true")
            .expect("imported key present");
        let complete_pos = json
            .find("\"files_complete\":false")
            .expect("files_complete key present");
        assert!(
            merges_pos < imported_pos && imported_pos < complete_pos && complete_pos < files_pos,
            "expected order ...merges, imported, files_complete, files...: {json}"
        );
    }

    // ---- F27 (redteam round 2): schema-major enforcement, PROTOCOL §10 ----
    //
    // These fixtures are wire literals, not struct literals, on purpose:
    // F27 is about what a line DECLARES, and a `TurnRecord { v: 2, .. }`
    // literal could not express a major this binary refuses to deserialize
    // in the first place.

    /// Turn-shaped JSON carrying `v_json` verbatim as its `v` value, and
    /// `type` only when `typed`. Every other field is a real required one.
    fn turn_wire(v_json: &str, typed: bool, id: &str) -> String {
        let tag = if typed { r#""type":"turn","# } else { "" };
        format!(
            concat!(
                "{{{}\"v\":{},\"id\":\"{}\",\"grade\":\"rich\",",
                "\"started\":\"2026-07-05T00:00:00.000Z\",",
                "\"ended\":\"2026-07-05T00:00:01.000Z\",",
                "\"root\":\"/repo\",\"files\":[]}}"
            ),
            tag, v_json, id
        )
    }

    // The load-bearing probe: `LogRecord` is an internally-tagged enum, so
    // the `v` field's `deserialize_with` runs against serde's buffered
    // `ContentDeserializer` rather than serde_json's own. If that path
    // silently accepted, the enforcement would have a hole exactly where the
    // finding lives — every real `log.jsonl` turn goes through it.
    #[test]
    fn future_major_is_refused_by_every_wire_type_including_the_tagged_enum() {
        assert!(
            serde_json::from_str::<LogRecord>(&turn_wire("2", true, "t_V2")).is_err(),
            "tagged LogRecord::Turn at v=2 must not deserialize"
        );
        assert!(
            serde_json::from_str::<TurnRecord>(&turn_wire("2", false, "t_V2")).is_err(),
            "bare TurnRecord at v=2 must not deserialize"
        );
        assert!(
            serde_json::from_str::<EpochRecord>(
                r#"{"v":2,"event":"start","ts":"2026-07-05T00:00:00.000Z"}"#
            )
            .is_err(),
            "EpochRecord at v=2 must not deserialize"
        );
        assert!(
            serde_json::from_str::<SignalEvent>(r#"{"v":2,"ts":5000,"tool":"claude-code"}"#)
                .is_err(),
            "SignalEvent at v=2 must not deserialize"
        );
        // v=0 is equally unimplemented: `v` has no minor component, so any
        // value other than SCHEMA_MAJOR is a different schema, not an older
        // point release of this one.
        assert!(serde_json::from_str::<LogRecord>(&turn_wire("0", true, "t_V0")).is_err());
    }

    // The classification half: a refused line is a NEWER PRODUCER, counted
    // as `UnknownType`, not corruption. `view::Ledger` surfaces the two as
    // separate counters and they prescribe different actions (upgrade vs.
    // check for a torn log).
    #[test]
    fn future_major_record_classifies_as_unknown_type_not_unparsed() {
        for typed in [true, false] {
            assert!(
                matches!(
                    parse_log_line(&turn_wire("2", typed, "t_V2")),
                    ParsedLine::UnknownType
                ),
                "typed={typed}: a v=2 turn must be counted as a newer producer"
            );
        }
        assert!(matches!(
            parse_log_line(r#"{"type":"epoch","v":7,"event":"start","ts":"2026-07-05T00:00:00Z"}"#),
            ParsedLine::UnknownType
        ));
    }

    // A non-integer `v` declares no version at all — it is a malformed field
    // on a known record kind, i.e. corruption. Same rule, same reason, as the
    // `"type": 5` guard: never assert a producer exists on evidence of
    // damage. (Probed, not assumed: `u32::deserialize` rejects each of these,
    // so none reaches `Record` either.)
    #[test]
    fn a_non_integer_v_is_corruption_not_a_version_declaration() {
        for bad_v in [r#""2""#, "2.5", "-1", "null"] {
            let line = turn_wire(bad_v, true, "t_BAD");
            assert!(
                matches!(parse_log_line(&line), ParsedLine::Unparsed),
                "v={bad_v} is a malformed field, not an unknown schema: {line}"
            );
        }
    }

    // The supported major, and the absent-`v` legacy shape, both still parse.
    // Absent `v` is read as SCHEMA_MAJOR: §4/§5 make `v` a writer MUST, but
    // §10 constrains writers only and is silent on a consumer meeting an
    // absent one, so §1 graceful degradation governs.
    #[test]
    fn supported_major_and_absent_v_both_still_parse() {
        assert!(matches!(
            parse_log_line(&turn_wire("1", true, "t_V1")),
            ParsedLine::Record(LogRecord::Turn(_))
        ));
        let no_v = concat!(
            "{\"type\":\"turn\",\"id\":\"t_NOV\",\"grade\":\"rich\",",
            "\"started\":\"2026-07-05T00:00:00.000Z\",",
            "\"ended\":\"2026-07-05T00:00:01.000Z\",\"root\":\"/repo\",\"files\":[]}"
        );
        match parse_log_line(no_v) {
            ParsedLine::Record(LogRecord::Turn(t)) => assert_eq!(t.v, SCHEMA_MAJOR),
            other => panic!("absent v must default to the supported major: {other:?}"),
        }
        // Unknown FIELDS within the supported major stay tolerated — that is
        // the additive half of §10 and this change must not touch it.
        let mut extra: serde_json::Value =
            serde_json::from_str(&turn_wire("1", true, "t_EXTRA")).unwrap();
        extra["field_from_a_later_v1_release"] = serde_json::json!("hello");
        assert!(matches!(
            parse_log_line(&extra.to_string()),
            ParsedLine::Record(LogRecord::Turn(_))
        ));
    }

    // End to end on the real read path: a future-major record never reaches
    // a consumer as a record. These records gate `undo`, so a misread future
    // record is a wrong-bytes revert source — that is why refusal, and not
    // best-effort interpretation, is the right default.
    #[test]
    fn future_major_records_never_reach_load_log() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("log.jsonl");
        let lines = [
            turn_wire("1", true, "t_OK"),
            turn_wire("2", true, "t_FUTURE_TAGGED"),
            turn_wire("2", false, "t_FUTURE_LEGACY"),
            r#"{"type":"epoch","v":2,"event":"start","ts":"2026-07-05T00:00:00Z"}"#.into(),
        ];
        fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();

        let records = load_log(&path);
        assert_eq!(records.len(), 1, "only the v=1 turn may be delivered");
        match &records[0] {
            LogRecord::Turn(t) => assert_eq!(t.id, "t_OK"),
            LogRecord::Epoch(_) => panic!("expected the v=1 turn"),
        }
    }

    // Signals are turn BOUNDARIES: misreading a future one mis-cuts a turn
    // and mis-attributes every file in it. Dropped like any other
    // uninterpretable line, leaving the quiet-window heuristic to produce an
    // honestly-unattributed bare turn.
    #[test]
    fn future_major_signal_is_dropped_while_supported_and_absent_v_survive() {
        let text = concat!(
            "{\"v\":2,\"ts\":5000,\"tool\":\"future-tool\",\"event\":\"start\"}\n",
            "{\"v\":1,\"ts\":6000,\"tool\":\"claude-code\",\"event\":\"start\"}\n",
            "{\"ts\":7000,\"tool\":\"codex\"}\n",
            "{\"v\":\"1\",\"ts\":8000,\"tool\":\"stringly-typed\"}\n",
        );
        let sigs = parse_signals(text);
        assert_eq!(sigs.len(), 2, "v=2 and a non-integer v are both dropped");
        assert_eq!(sigs[0].tool, "claude-code");
        assert_eq!(sigs[1].tool, "codex");
        assert_eq!(sigs[1].v, SCHEMA_MAJOR);
    }
}
