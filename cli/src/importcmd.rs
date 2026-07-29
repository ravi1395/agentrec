//! `agentrec import claude --dry-run` (P1, read-only phase): streams Claude
//! Code's `~/.claude/projects/**/*.jsonl` transcript corpus and reports
//! per-tier before-bytes reconstructability without writing anything. This
//! phase never persists — `--dry-run` is the only supported mode; passing
//! `--dry-run=false` (i.e. omitting the flag) is a loud, nonzero-exit error
//! rather than a silent no-op, since real persistence lands in P2.
//!
//! Classification ladder (first hit wins), see P1.md "Changes" + Pinned
//! decisions 1-4 (binding, ground-truthed against real `~/.claude` data):
//!   T1       — `toolUseResult.originalFile` present, or the op is a create.
//!   T1.5     — no `originalFile`, but an earlier `snapshot` line in the same
//!              session recorded `trackedFileBackups[path].backupFileName`;
//!              resolved by reading `<source>/file-history/<sessionId>/<name>`
//!              **verbatim** — the field IS the filename, never recomputed.
//!   T2       — no `originalFile`, no tracked backup, but the path is
//!              git-tracked in a repo opened at the session's `cwd`.
//!              **Detected only** in this phase — no blob bytes are read.
//!   T3       — none of the above; `before` stays unknown, provenance-only.
//!
//! Two distinct skip counters that must never be conflated (Pinned decision
//! 4): `skipped_malformed_line` (a line that isn't even valid JSON — AC6,
//! tolerated, doesn't abort the file) vs `skipped_missing_field` (a line that
//! parses but lacks a field its shape requires — AC8, schema drift, named by
//! field so it's visible). A session hitting the AC8 path never counts
//! importable; a session with only AC6 hits still counts importable as long
//! as at least one line parsed (Pinned decision 3).
//!
//! `cwd` is tracked at session granularity, not per-line (Pinned decision
//! 14): a `type: "summary"` head line commonly carries no `cwd` on a
//! resumed/compacted transcript, and must not disqualify a session that
//! establishes `cwd` from a later line.

use agentrec_core::store::hash_bytes;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Mirrors `main::ImportSource` — kept private (this module is a descendant
/// of the crate root, so the root's private `ImportSource` is already
/// visible here; no re-export needed).
pub fn run(root: &Path, source: crate::ImportSource) -> Result<(), String> {
    let crate::ImportSource::Claude {
        dry_run,
        source,
        json,
    } = source;
    run_claude(root, dry_run, source, json)
}

fn run_claude(
    root: &Path,
    dry_run: bool,
    source: Option<PathBuf>,
    json: bool,
) -> Result<(), String> {
    // Judgment call (P1.md's CLI-shape section leaves this open): a plain
    // `#[arg(long)]` bool flag can't distinguish "explicitly passed false"
    // from "omitted" — clap flags are presence-only. Both collapse to the
    // same `dry_run == false` case here, and both get the same loud refusal:
    // P2 hasn't landed persistence yet, so a bare `agentrec import claude`
    // must never silently do nothing.
    if !dry_run {
        return Err(
            "import claude requires --dry-run — persistence is not implemented \
             until P2; this command only ever classifies and reports, never writes"
                .to_string(),
        );
    }

    let src = match source {
        Some(s) => s,
        None => default_claude_source()?,
    };

    // Item 6 (silent-failure hardening): a missing or malformed --source is
    // a hard, nonzero-exit error — never a fake `sessions_total: 0`, which
    // is indistinguishable from a legitimately empty corpus and would
    // silently produce a bogus "0% but valid" AC1 result for a typo'd path.
    // An EXISTING `projects/` with zero session files inside is legitimate
    // (handled below: the scan loop simply doesn't iterate) and must still
    // exit 0.
    if !src.is_dir() {
        return Err(format!(
            "import claude: --source '{}' is not a directory (does it exist?)",
            src.display()
        ));
    }
    let projects_dir = src.join("projects");
    if !projects_dir.is_dir() {
        return Err(format!(
            "import claude: --source '{}' has no 'projects' subdirectory — expected \
             Claude Code's transcript layout (<source>/projects/<project>/*.jsonl, \
             Pinned decision 1)",
            src.display()
        ));
    }

    let mut counters = Counters::default();
    let mut debug = DebugSink::new(debug_dump_entries_enabled());
    let mut git_cache = GitTrackCache::default();

    let mut dirs: Vec<PathBuf> = fs::read_dir(&projects_dir)
        .map_err(|e| {
            format!(
                "import claude: failed to read '{}': {e}",
                projects_dir.display()
            )
        })?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for project_dir in dirs {
        scan_project_dir(
            &project_dir,
            &src,
            root,
            &mut counters,
            &mut debug,
            &mut git_cache,
        );
    }

    let peak_rss_mb = peak_rss_mb();
    let report = build_report(&counters, peak_rss_mb, debug.entries);

    if counters.skipped_missing_cwd > 0 {
        eprintln!(
            "agentrec: import claude: {} line(s) missing required field 'cwd' \
             (schema drift) — affected session(s) not counted importable",
            counters.skipped_missing_cwd
        );
    }
    if counters.skipped_missing_tool_use_result > 0 {
        eprintln!(
            "agentrec: import claude: {} line(s) structurally a tool result but \
             missing 'toolUseResult' (schema drift) — affected session(s) not \
             counted importable",
            counters.skipped_missing_tool_use_result
        );
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else {
        print_text_report(&report);
    }

    // AC8: schema drift is surfaced loudly (stderr line above + named,
    // present-in-json counters) but does not fail the whole batch run — one
    // session's drift is not a fatal error for the corpus scan (AC6's sibling
    // "exits 0" posture extends to this case too; see module doc comment).
    Ok(())
}

// ---- report shape (Pinned decision 12, extended by decisions 13/15 and
// item 6's I/O counter — do not add fields outside `debug_entries`, which
// serializes away entirely when empty) --------------------------------------

#[derive(Serialize)]
struct TierCounts {
    t1: usize,
    t1_5: usize,
    t2_candidate: usize,
    t3: usize,
}

#[derive(Serialize)]
struct MissingFieldCounts {
    cwd: usize,
    tool_use_result: usize,
}

#[derive(Serialize)]
struct ImportReport {
    sessions_total: usize,
    sessions_importable: usize,
    importable_pct: f64,
    sessions_in_root: usize,
    tier_counts: TierCounts,
    opaque_calls: usize,
    /// Pinned decision 13: mean, across every denominator session, of that
    /// session's `opaque_calls_in_session / max(1, tool_result_entries)`
    /// fraction, expressed as a 0-100 percentage (same scale as
    /// `importable_pct`).
    mean_opaque_share_pct: f64,
    skipped_sidechain: usize,
    skipped_malformed_line: usize,
    /// Pinned decision 15: a line that read but was not valid UTF-8.
    skipped_non_utf8_line: usize,
    /// Item 6: a denominator session file that failed `File::open` (e.g.
    /// permissions) — counted in `sessions_total` but never importable.
    skipped_io_error: usize,
    skipped_missing_field: MissingFieldCounts,
    peak_rss_mb: f64,
    /// Test-only (Pinned decision 9): per-entry classification detail,
    /// populated only when `AGENTREC_IMPORT_DEBUG_ENTRIES=1` is set in a
    /// debug build (`debug_dump_entries_enabled`, below). Empty in every
    /// normal run, in which case this field is omitted from the JSON
    /// entirely — the on-the-wire shape then matches Pinned decision 12
    /// exactly. `cli/tests/import_claude.rs` is a black-box integration test
    /// against the compiled binary (no `[lib]` target to import internals
    /// from), so this is the only seam available for asserting per-entry
    /// tier + resolved-bytes hashes without weakening the aggregate contract.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    debug_entries: Vec<DebugEntry>,
}

#[derive(Serialize, Clone)]
struct DebugEntry {
    session_file: String,
    path: String,
    tier: String,
    sha256: Option<String>,
}

fn build_report(
    counters: &Counters,
    peak_rss_mb: f64,
    debug_entries: Vec<DebugEntry>,
) -> ImportReport {
    let importable_pct = if counters.sessions_total == 0 {
        0.0
    } else {
        counters.sessions_importable as f64 / counters.sessions_total as f64 * 100.0
    };
    let mean_opaque_share_pct = if counters.sessions_total == 0 {
        0.0
    } else {
        counters.opaque_share_sum / counters.sessions_total as f64 * 100.0
    };
    ImportReport {
        sessions_total: counters.sessions_total,
        sessions_importable: counters.sessions_importable,
        importable_pct,
        sessions_in_root: counters.sessions_in_root,
        tier_counts: TierCounts {
            t1: counters.t1,
            t1_5: counters.t1_5,
            t2_candidate: counters.t2_candidate,
            t3: counters.t3,
        },
        opaque_calls: counters.opaque_calls,
        mean_opaque_share_pct,
        skipped_sidechain: counters.skipped_sidechain,
        skipped_malformed_line: counters.skipped_malformed_line,
        skipped_non_utf8_line: counters.skipped_non_utf8_line,
        skipped_io_error: counters.skipped_io_error,
        skipped_missing_field: MissingFieldCounts {
            cwd: counters.skipped_missing_cwd,
            tool_use_result: counters.skipped_missing_tool_use_result,
        },
        peak_rss_mb,
        // Already filtered by `DebugSink::push` (item 11) — no run-condition
        // re-check needed here, this Vec is empty on every normal run.
        debug_entries,
    }
}

fn print_text_report(report: &ImportReport) {
    println!("agentrec import claude --dry-run report");
    println!("  sessions_total: {}", report.sessions_total);
    println!(
        "  sessions_importable: {} ({:.1}%)",
        report.sessions_importable, report.importable_pct
    );
    println!("  sessions_in_root: {}", report.sessions_in_root);
    println!(
        "  tier_counts: t1={} t1_5={} t2_candidate={} t3={}",
        report.tier_counts.t1,
        report.tier_counts.t1_5,
        report.tier_counts.t2_candidate,
        report.tier_counts.t3
    );
    println!("  opaque_calls: {}", report.opaque_calls);
    println!(
        "  mean_opaque_share_pct: {:.2}",
        report.mean_opaque_share_pct
    );
    println!("  skipped_sidechain: {}", report.skipped_sidechain);
    println!(
        "  skipped_malformed_line: {}",
        report.skipped_malformed_line
    );
    println!("  skipped_non_utf8_line: {}", report.skipped_non_utf8_line);
    println!("  skipped_io_error: {}", report.skipped_io_error);
    println!(
        "  skipped_missing_field: cwd={} tool_use_result={}",
        report.skipped_missing_field.cwd, report.skipped_missing_field.tool_use_result
    );
    println!("  peak_rss_mb: {:.2}", report.peak_rss_mb);
}

// ---- test-only seam (Pinned decision 9) ------------------------------------

/// When set (debug builds only), `ImportReport.debug_entries` is populated —
/// the only way `cli/tests/import_claude.rs` (a black-box integration test,
/// no crate internals to import) can assert per-entry tier + resolved-bytes
/// hashes. The env read is compiled out entirely in release builds (the
/// `#[cfg(not(debug_assertions))]` arm never touches the environment), same
/// fail-safe class as `purgecmd.rs`'s `test_pause_before_memory_rewrite_delay`
/// — `strings target/release/agentrec | grep AGENTREC_IMPORT_DEBUG_ENTRIES`
/// must print nothing.
fn debug_dump_entries_enabled() -> bool {
    #[cfg(debug_assertions)]
    {
        std::env::var("AGENTREC_IMPORT_DEBUG_ENTRIES").as_deref() == Ok("1")
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

/// Sink for `DebugEntry` records. Item 11: gates the *construction/push*,
/// not just JSON serialization — when the seam above is off, `push` is a
/// no-op, so this Vec never grows with corpus size (previously it was
/// populated unconditionally and only hidden from the wire format via
/// `#[serde(skip_serializing_if)]`, which still cost memory linear in
/// corpus size against the very RSS budget AC7 measures).
struct DebugSink {
    enabled: bool,
    entries: Vec<DebugEntry>,
}

impl DebugSink {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            entries: Vec::new(),
        }
    }

    fn push(&mut self, session_file: &str, path: &str, tier: &str, sha256: Option<String>) {
        if !self.enabled {
            return;
        }
        self.entries.push(DebugEntry {
            session_file: session_file.to_string(),
            path: path.to_string(),
            tier: tier.to_string(),
            sha256,
        });
    }
}

// ---- default `--source` ------------------------------------------------

/// `~/.claude` — split out as a pure function of `home` so the join logic is
/// directly unit-testable without touching the real environment (only the
/// thin `default_claude_source` wrapper reads `$HOME`).
fn default_source_for_home(home: &str) -> PathBuf {
    PathBuf::from(home).join(".claude")
}

/// Item 6: if `--source` is omitted and `$HOME` cannot be resolved, error
/// loudly rather than silently falling back to a relative `./.claude/projects`
/// path that will almost certainly resolve to nothing.
fn default_claude_source() -> Result<PathBuf, String> {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => Ok(default_source_for_home(&home)),
        _ => Err(
            "import claude: cannot resolve default --source (~/.claude) — $HOME \
             is not set; pass --source explicitly"
                .to_string(),
        ),
    }
}

// ---- counters ---------------------------------------------------------

#[derive(Default)]
struct Counters {
    sessions_total: usize,
    sessions_importable: usize,
    sessions_in_root: usize,
    t1: usize,
    t1_5: usize,
    t2_candidate: usize,
    t3: usize,
    opaque_calls: usize,
    skipped_sidechain: usize,
    skipped_malformed_line: usize,
    skipped_missing_cwd: usize,
    skipped_missing_tool_use_result: usize,
    skipped_non_utf8_line: usize,
    skipped_io_error: usize,
    /// Sum, across every denominator session processed so far, of that
    /// session's `opaque_calls_in_session / max(1, tool_result_entries)`
    /// fraction (Pinned decision 13). Divided by `sessions_total` and
    /// scaled to a percentage in `build_report`.
    opaque_share_sum: f64,
}

// ---- line reading, tolerant of invalid UTF-8 (Pinned decision 15) --------

enum LineOutcome {
    Line(String),
    NonUtf8,
}

/// Streams `file` line-by-line, tolerant of invalid UTF-8. Unlike
/// `BufRead::lines()` — which yields `Err` on a non-UTF8 line, and combined
/// with an early-stop combinator like `.map_while(Result::ok)` silently
/// truncates every remaining line in the file with zero signal — a bad line
/// here yields `LineOutcome::NonUtf8` to the callback and iteration
/// continues with the next line. Trailing `\n` (and a preceding `\r`, for
/// CRLF-authored fixtures) is stripped; a final line with no trailing
/// newline is still delivered.
fn each_raw_line(file: File, mut on_line: impl FnMut(LineOutcome)) {
    let mut reader = BufReader::new(file);
    let mut buf: Vec<u8> = Vec::new();
    while let Ok(n) = reader.read_until(b'\n', &mut buf) {
        if n == 0 {
            break;
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
        }
        let bytes = std::mem::take(&mut buf);
        match String::from_utf8(bytes) {
            Ok(s) => on_line(LineOutcome::Line(s)),
            Err(_) => on_line(LineOutcome::NonUtf8),
        }
    }
}

// ---- corpus walk --------------------------------------------------------

/// One project directory (`<source>/projects/<project>/`): top-level
/// `*.jsonl` files are the AC1 denominator (Pinned decision 3); anything
/// nested under a `subagents/` path segment is walked ONLY to harvest
/// `skipped_sidechain` counts for file-producing entries (Pinned decision 2)
/// — never added to `sessions_total`.
fn scan_project_dir(
    project_dir: &Path,
    source: &Path,
    root: &Path,
    counters: &mut Counters,
    debug: &mut DebugSink,
    git_cache: &mut GitTrackCache,
) {
    let mut top_level: Vec<PathBuf> = fs::read_dir(project_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    top_level.sort();

    for file in &top_level {
        counters.sessions_total += 1;
        process_session_file(file, source, root, counters, debug, git_cache);
    }

    for entry in walkdir::WalkDir::new(project_dir)
        .min_depth(2)
        .into_iter()
        .flatten()
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_subagent_jsonl = path.extension().is_some_and(|e| e == "jsonl")
            && path.components().any(|c| c.as_os_str() == "subagents");
        if is_subagent_jsonl {
            harvest_sidechain_only(path, counters);
        }
    }
}

/// A `subagents/*.jsonl` file is outside the AC1 denominator entirely (not a
/// "session that fails" — not counted at all). It still needs walking so its
/// file-producing lines land in `skipped_sidechain` (AC5's path-signal half).
fn harvest_sidechain_only(path: &Path, counters: &mut Counters) {
    let Ok(file) = File::open(path) else {
        return;
    };
    each_raw_line(file, |outcome| {
        let line = match outcome {
            LineOutcome::Line(s) => s,
            LineOutcome::NonUtf8 => {
                counters.skipped_non_utf8_line += 1;
                return;
            }
        };
        if line.trim().is_empty() {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            return;
        };
        let is_file_producing = value
            .get("toolUseResult")
            .and_then(|t| t.get("filePath"))
            .and_then(|v| v.as_str())
            .is_some();
        if is_file_producing {
            counters.skipped_sidechain += 1;
        }
    });
}

/// Stream one top-level transcript file line by line (never loaded whole).
fn process_session_file(
    path: &Path,
    source: &Path,
    root: &Path,
    counters: &mut Counters,
    debug: &mut DebugSink,
    git_cache: &mut GitTrackCache,
) {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            // Item 6: a session file that fails to open after already being
            // counted in the AC1 denominator must not be silently skipped
            // with zero signal.
            counters.skipped_io_error += 1;
            eprintln!(
                "agentrec: import claude: warning: failed to open session file \
                 '{}': {e} (counted in sessions_total, never importable)",
                path.display()
            );
            return;
        }
    };
    let session_file_label = path.display().to_string();

    let mut any_line_parsed = false;
    let mut hit_ac8 = false;
    let mut total_parsed_lines = 0usize;
    let mut sidechain_flagged_lines = 0usize;
    let mut session_cwd: Option<PathBuf> = None;
    let mut session_id: Option<String> = None;
    // path -> backupFileName, harvested from `snapshot` lines as we walk;
    // real transcripts are chronological, so a snapshot always precedes the
    // edit it backs up (assumption, undisturbed by any fixture here).
    let mut backups: HashMap<String, String> = HashMap::new();
    // Pinned decision 13 bookkeeping, this session only.
    let mut session_opaque = 0usize;
    let mut session_entries = 0usize;

    each_raw_line(file, |outcome| {
        let line = match outcome {
            LineOutcome::Line(s) => s,
            LineOutcome::NonUtf8 => {
                counters.skipped_non_utf8_line += 1;
                return;
            }
        };
        if line.trim().is_empty() {
            return;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                counters.skipped_malformed_line += 1;
                return;
            }
        };
        any_line_parsed = true;
        total_parsed_lines += 1;

        if session_id.is_none() {
            session_id = value
                .get("sessionId")
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }

        let is_sidechain_field = value
            .get("isSidechain")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if is_sidechain_field {
            sidechain_flagged_lines += 1;
        }

        // Pinned decision 14: `cwd` is session-level, not line-level. A line
        // lacking `cwd` (e.g. a `type: "summary"` head line on a
        // resumed/compacted transcript) does not disqualify anything by
        // itself; only the first line that DOES carry `cwd` sets it for the
        // rest of the session. Whether the session ever saw one at all is
        // only knowable at EOF (checked after this closure returns).
        if session_cwd.is_none() {
            if let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()) {
                session_cwd = Some(PathBuf::from(cwd));
            }
        }

        // `snapshot` lines carry no tool-result, only backup bookkeeping.
        if value.get("type").and_then(|v| v.as_str()) == Some("snapshot") {
            if let Some(map) = value
                .pointer("/snapshot/trackedFileBackups")
                .and_then(|v| v.as_object())
            {
                for (file_path, info) in map {
                    if let Some(backup_name) = info.get("backupFileName").and_then(|v| v.as_str()) {
                        backups.insert(file_path.clone(), backup_name.to_string());
                    }
                }
            }
            return;
        }

        // Structural check (Pinned decision 4 / FIXTURES.md scenario 7): a
        // line is a tool-result line iff `message.content[]` has a nested
        // `type == "tool_result"` block — NOT keyed off a top-level
        // `toolUseID` convenience field.
        let is_tool_result_shaped = value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
            .map(|items| {
                items
                    .iter()
                    .any(|item| item.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
            })
            .unwrap_or(false);

        let top_level_tur = value.get("toolUseResult");

        if is_tool_result_shaped && top_level_tur.is_none() {
            counters.skipped_missing_tool_use_result += 1;
            hit_ac8 = true;
            return;
        }

        let Some(tur) = top_level_tur else {
            // Not a tool-result-bearing line at all (plain chat, a bare
            // tool_use invocation with no attached result yet, etc).
            return;
        };

        // Item 5: sidechain exclusion (Pinned decision 2) runs BEFORE any
        // tier/opaque classification — a sidechain-flagged line whose
        // toolUseResult happens to be opaque-shaped (e.g. Bash, no
        // `filePath`) must land in `skipped_sidechain`, never
        // `opaque_calls`. Placed here (after confirming the line actually
        // carries a toolUseResult) rather than at the top of the loop, so a
        // plain sidechain-flagged chat line with no toolUseResult still
        // falls through the `continue` above and is never double-counted.
        if is_sidechain_field {
            counters.skipped_sidechain += 1;
            return;
        }

        let Some(file_path) = tur.get("filePath").and_then(|v| v.as_str()) else {
            // Opaque call (e.g. Bash/Task) — legitimate, not a failure.
            counters.opaque_calls += 1;
            session_opaque += 1;
            session_entries += 1;
            return;
        };

        classify_file_entry(
            tur,
            file_path,
            source,
            session_id.as_deref(),
            session_cwd.as_deref(),
            &backups,
            &session_file_label,
            counters,
            debug,
            git_cache,
            &mut session_entries,
        );
    });

    // EOF: only now do we know whether the session ever established a `cwd`
    // at all (Pinned decision 14).
    if any_line_parsed && session_cwd.is_none() {
        counters.skipped_missing_cwd += 1;
        hit_ac8 = true;
    }

    let entirely_sidechain =
        total_parsed_lines > 0 && sidechain_flagged_lines == total_parsed_lines;
    if any_line_parsed && !hit_ac8 && !entirely_sidechain {
        counters.sessions_importable += 1;
    }

    if let Some(cwd) = &session_cwd {
        if path_is_under_root(cwd, root) {
            counters.sessions_in_root += 1;
        }
    }

    // Pinned decision 13: fold this session's opaque-call share into the
    // corpus-wide mean computed in `build_report`. A session with zero
    // tool-result entries (e.g. all lines malformed, or a plain-chat-only
    // session) contributes 0.0, never a divide-by-zero.
    let session_fraction = if session_entries == 0 {
        0.0
    } else {
        session_opaque as f64 / session_entries as f64
    };
    counters.opaque_share_sum += session_fraction;
}

/// Before-bytes ladder, first hit wins (P1.md Changes / Pinned decision 1).
#[allow(clippy::too_many_arguments)]
fn classify_file_entry(
    tur: &Value,
    file_path: &str,
    source: &Path,
    session_id: Option<&str>,
    session_cwd: Option<&Path>,
    backups: &HashMap<String, String>,
    session_file_label: &str,
    counters: &mut Counters,
    debug: &mut DebugSink,
    git_cache: &mut GitTrackCache,
    session_entries: &mut usize,
) {
    let original_file = tur.get("originalFile").and_then(|v| v.as_str());
    // Item 9 (real-corpus-confirmed shape, not a guess): a create op has
    // `toolUseResult.type == "create"` AND `originalFile` explicitly JSON
    // `null` (not merely absent from the object) — this legitimately means
    // "no pre-edit content exists because the file was new", a valid T1
    // classification with no bytes to hash, distinct from T3 ("content
    // exists but couldn't be found"). `Value::Null.as_str()` already yields
    // `None`, so `original_file` above is `None` for this shape too, and
    // control falls through to this check. `content` on a create line holds
    // the NEW file's bytes, not relevant to T1's pre-edit resolution.
    let op_is_create = tur.get("type").and_then(|v| v.as_str()) == Some("create");

    if let Some(orig) = original_file {
        counters.t1 += 1;
        *session_entries += 1;
        let hash = hash_bytes(orig.as_bytes());
        debug.push(
            session_file_label,
            file_path,
            "t1",
            Some(strip_hash_prefix(&hash)),
        );
        return;
    }
    if op_is_create {
        counters.t1 += 1;
        *session_entries += 1;
        debug.push(session_file_label, file_path, "t1", None);
        return;
    }
    if let Some(backup_name) = backups.get(file_path) {
        if let Some(sid) = session_id {
            let blob_path = source.join("file-history").join(sid).join(backup_name);
            if let Ok(bytes) = fs::read(&blob_path) {
                counters.t1_5 += 1;
                *session_entries += 1;
                let hash = hash_bytes(&bytes);
                debug.push(
                    session_file_label,
                    file_path,
                    "t1_5",
                    Some(strip_hash_prefix(&hash)),
                );
                return;
            }
            // Backup referenced but missing (e.g. retention already reaped
            // it, ~30-day window) — falls through to T2/T3, never errors.
        }
    }
    classify_t2_or_t3(
        file_path,
        session_cwd,
        session_file_label,
        counters,
        debug,
        git_cache,
        session_entries,
    );
}

#[allow(clippy::too_many_arguments)]
fn classify_t2_or_t3(
    file_path: &str,
    session_cwd: Option<&Path>,
    session_file_label: &str,
    counters: &mut Counters,
    debug: &mut DebugSink,
    git_cache: &mut GitTrackCache,
    session_entries: &mut usize,
) {
    let tracked = session_cwd.is_some_and(|cwd| git_cache.is_tracked(cwd, file_path));
    *session_entries += 1;
    if tracked {
        counters.t2_candidate += 1;
        debug.push(session_file_label, file_path, "t2_candidate", None);
    } else {
        counters.t3 += 1;
        debug.push(session_file_label, file_path, "t3", None);
    }
}

/// Item 10: per-`cwd` cache of "is this repo, and if so which paths does it
/// track" — a real corpus has thousands of T2/T3-candidate entries, and the
/// prior implementation shelled out to `git ls-files --error-unmatch` once
/// per entry (thousands of process forks). This resolves the repo root and
/// its full tracked-file set once per unique `cwd`, reusing it for every
/// later entry that shares that `cwd`/repo.
#[derive(Default)]
struct GitTrackCache {
    /// Canonicalized `cwd` -> `Some((canonicalized repo root, tracked
    /// relative paths))`, or `None` if `cwd` isn't inside a git repo (or the
    /// lookup failed) — cached either way so a bad `cwd` isn't retried.
    by_cwd: HashMap<PathBuf, Option<(PathBuf, HashSet<PathBuf>)>>,
}

impl GitTrackCache {
    fn is_tracked(&mut self, cwd: &Path, file_path: &str) -> bool {
        let key = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let entry = self
            .by_cwd
            .entry(key.clone())
            .or_insert_with(|| Self::load(&key));
        let Some((repo_root, tracked)) = entry else {
            return false;
        };
        match Path::new(file_path).strip_prefix(&repo_root) {
            Ok(rel) => tracked.contains(rel),
            Err(_) => false,
        }
    }

    /// One `git rev-parse --show-toplevel` + one `git ls-files -z` per
    /// unique `cwd`, instead of a `git ls-files --error-unmatch` fork per
    /// file entry.
    fn load(cwd: &Path) -> Option<(PathBuf, HashSet<PathBuf>)> {
        let toplevel_out = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()?;
        if !toplevel_out.status.success() {
            return None;
        }
        let toplevel_str = String::from_utf8_lossy(&toplevel_out.stdout)
            .trim()
            .to_string();
        if toplevel_str.is_empty() {
            return None;
        }
        let repo_root =
            fs::canonicalize(&toplevel_str).unwrap_or_else(|_| PathBuf::from(&toplevel_str));

        let ls_out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo_root)
            .args(["ls-files", "-z"])
            .output()
            .ok()?;
        if !ls_out.status.success() {
            return None;
        }
        let tracked: HashSet<PathBuf> = ls_out
            .stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| PathBuf::from(String::from_utf8_lossy(s).into_owned()))
            .collect();
        Some((repo_root, tracked))
    }
}

fn strip_hash_prefix(hash: &str) -> String {
    hash.strip_prefix("sha256:").unwrap_or(hash).to_string()
}

fn path_is_under_root(cwd: &Path, root: &Path) -> bool {
    let (Ok(c), Ok(r)) = (fs::canonicalize(cwd), fs::canonicalize(root)) else {
        return false;
    };
    c.starts_with(r)
}

// ---- AC7: peak RSS ------------------------------------------------------

/// `ru_maxrss` is bytes on macOS, KiB on Linux (Pinned decision 5) — the
/// `#[cfg(target_os = ...)]` gate is confined to this constant, never to
/// `rss_raw_to_mb` itself, so the conversion function stays plain and
/// testable with both divisors on whichever OS CI happens to run on (item
/// 7).
#[cfg(target_os = "macos")]
const RSS_DIVISOR: f64 = 1024.0 * 1024.0; // bytes -> MB
#[cfg(target_os = "linux")]
const RSS_DIVISOR: f64 = 1024.0; // KiB -> MB
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const RSS_DIVISOR: f64 = 1024.0 * 1024.0;

fn peak_rss_mb() -> f64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    if ret != 0 {
        return 0.0;
    }
    rss_raw_to_mb(usage.ru_maxrss as u64, RSS_DIVISOR)
}

/// Pure conversion, divisor passed in explicitly — not `#[cfg]`'d itself —
/// so a single, unconditional unit test can exercise both platforms'
/// arithmetic regardless of which OS actually compiles it (item 7).
fn rss_raw_to_mb(raw: u64, divisor: f64) -> f64 {
    raw as f64 / divisor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_source_joins_dot_claude_onto_home() {
        assert_eq!(
            default_source_for_home("/Users/x"),
            PathBuf::from("/Users/x/.claude")
        );
    }

    #[test]
    fn rss_conversion_covers_both_platform_divisors() {
        const MACOS_DIVISOR: f64 = 1024.0 * 1024.0; // bytes -> MB
        const LINUX_DIVISOR: f64 = 1024.0; // KiB -> MB
        assert_eq!(rss_raw_to_mb(10 * 1024 * 1024, MACOS_DIVISOR), 10.0);
        assert_eq!(rss_raw_to_mb(10 * 1024, LINUX_DIVISOR), 10.0);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn debug_dump_entries_is_disabled_in_release_even_with_env_set() {
        std::env::set_var("AGENTREC_IMPORT_DEBUG_ENTRIES", "1");
        assert!(!debug_dump_entries_enabled());
        std::env::remove_var("AGENTREC_IMPORT_DEBUG_ENTRIES");
    }
}
