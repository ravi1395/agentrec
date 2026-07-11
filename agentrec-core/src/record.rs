//! Protocol wire types (PROTOCOL §4–§5): signal lines, turn records, epoch
//! records, plus tolerant append-only log IO. Unknown fields are preserved by
//! never rewriting history and tolerated by serde defaults on read.

use crate::perms;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;

/// Emitter → recorder signal line (PROTOCOL §4). `ts` is unix milliseconds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignalEvent {
    #[serde(default = "one")]
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
    #[serde(default)]
    pub prompt: Option<String>,
}

fn one() -> u32 {
    1
}

impl SignalEvent {
    pub fn is_start(&self) -> bool {
        self.event.as_deref() == Some("start")
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
    #[serde(default = "one")]
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
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpochRecord {
    #[serde(default = "one")]
    pub v: u32,
    pub event: String, // "start" | "stop"
    pub ts: String,    // RFC 3339
}

/// Append one record; creates parents; never rewrites (append-only invariant).
/// Fsyncs before returning (D34): turn-close and epoch appends must survive a
/// kill-9 immediately after this call returns Ok.
pub fn append_log(path: &Path, record: &LogRecord) -> Result<(), String> {
    let line = serde_json::to_string(record).map_err(|e| e.to_string())?;
    let file = open_append(path, &line)?;
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
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // `type` defaults to turn for lines written before epochs existed.
        if let Ok(rec) = serde_json::from_str::<LogRecord>(trimmed) {
            out.push(rec);
            continue;
        }
        // The bare-TurnRecord fallback exists only for legacy lines that
        // predate the `type` tag entirely (C6). A line that DOES have a
        // `type` field — just one `LogRecord` doesn't recognize, e.g. a
        // future additive record kind — must never be coerced into a turn:
        // serde ignores unknown fields by default, so a `type:"future_thing"`
        // line with turn-shaped fields would otherwise silently misparse.
        let has_type_field = serde_json::from_str::<serde_json::Value>(trimmed)
            .ok()
            .and_then(|v| v.as_object().map(|o| o.contains_key("type")))
            .unwrap_or(false);
        if !has_type_field {
            if let Ok(turn) = serde_json::from_str::<TurnRecord>(trimmed) {
                out.push(LogRecord::Turn(turn));
            }
        }
    }
    out
}

/// Parse signal-file text from an offset; skips garbage lines (PROTOCOL §4
/// tolerance). Returns events in file order.
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
            files: vec![FileEntry {
                path: "src/a.rs".into(),
                before: None,
                after: Some("sha256:aa".into()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
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
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("skipped"));
        assert!(!json.contains("withheld"));
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
}
