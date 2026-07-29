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

use agentrec_core::store::hash_bytes;
use serde::Serialize;
use serde_json::Value;
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

    let src = source.unwrap_or_else(default_claude_source);
    let mut counters = Counters::default();
    let mut debug_entries: Vec<DebugEntry> = Vec::new();

    let projects_dir = src.join("projects");
    if let Ok(project_dirs) = fs::read_dir(&projects_dir) {
        let mut dirs: Vec<PathBuf> = project_dirs
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for project_dir in dirs {
            scan_project_dir(&project_dir, &src, root, &mut counters, &mut debug_entries);
        }
    }

    let peak_rss_mb = peak_rss_mb();
    let report = build_report(&counters, peak_rss_mb, debug_entries);

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

// ---- report shape (Pinned decision 12 — exact object when the debug seam is
// unused; do not add fields outside `debug_entries`, which serializes away
// entirely when empty) ---------------------------------------------------

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
    skipped_sidechain: usize,
    skipped_malformed_line: usize,
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
        skipped_sidechain: counters.skipped_sidechain,
        skipped_malformed_line: counters.skipped_malformed_line,
        skipped_missing_field: MissingFieldCounts {
            cwd: counters.skipped_missing_cwd,
            tool_use_result: counters.skipped_missing_tool_use_result,
        },
        peak_rss_mb,
        debug_entries: if debug_dump_entries_enabled() {
            debug_entries
        } else {
            Vec::new()
        },
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
    println!("  skipped_sidechain: {}", report.skipped_sidechain);
    println!(
        "  skipped_malformed_line: {}",
        report.skipped_malformed_line
    );
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

// ---- default `--source` ------------------------------------------------

/// `~/.claude` — split out as a pure function of `home` so the join logic is
/// directly unit-testable without touching the real environment (only the
/// thin `default_claude_source` wrapper reads `$HOME`).
fn default_source_for_home(home: &str) -> PathBuf {
    PathBuf::from(home).join(".claude")
}

fn default_claude_source() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    default_source_for_home(&home)
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
    debug: &mut Vec<DebugEntry>,
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
        process_session_file(file, source, root, counters, debug);
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
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let is_file_producing = value
            .get("toolUseResult")
            .and_then(|t| t.get("filePath"))
            .and_then(|v| v.as_str())
            .is_some();
        if is_file_producing {
            counters.skipped_sidechain += 1;
        }
    }
}

/// Stream one top-level transcript file line by line (never loaded whole).
fn process_session_file(
    path: &Path,
    source: &Path,
    root: &Path,
    counters: &mut Counters,
    debug: &mut Vec<DebugEntry>,
) {
    let Ok(file) = File::open(path) else {
        return;
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
    let mut backups: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                counters.skipped_malformed_line += 1;
                continue;
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

        let cwd = value.get("cwd").and_then(|v| v.as_str());
        let Some(cwd) = cwd else {
            counters.skipped_missing_cwd += 1;
            hit_ac8 = true;
            continue;
        };
        if session_cwd.is_none() {
            session_cwd = Some(PathBuf::from(cwd));
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
            continue;
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
            continue;
        }

        let Some(tur) = top_level_tur else {
            // Not a tool-result-bearing line at all (plain chat, a bare
            // tool_use invocation with no attached result yet, etc).
            continue;
        };

        let Some(file_path) = tur.get("filePath").and_then(|v| v.as_str()) else {
            // Opaque call (e.g. Bash/Task) — legitimate, not a failure.
            counters.opaque_calls += 1;
            continue;
        };

        if is_sidechain_field {
            counters.skipped_sidechain += 1;
            continue;
        }

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
        );
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
}

/// Before-bytes ladder, first hit wins (P1.md Changes / Pinned decision 1).
#[allow(clippy::too_many_arguments)]
fn classify_file_entry(
    tur: &Value,
    file_path: &str,
    source: &Path,
    session_id: Option<&str>,
    session_cwd: Option<&Path>,
    backups: &std::collections::HashMap<String, String>,
    session_file_label: &str,
    counters: &mut Counters,
    debug: &mut Vec<DebugEntry>,
) {
    let original_file = tur.get("originalFile").and_then(|v| v.as_str());
    // Judgment call (no fixture exercises this path — P1.md's "op is a
    // create" text names no confirmed field/value; this is the best-guess
    // signal, additive-only on top of the primary `originalFile` check):
    // a `toolUseResult.type == "create"` marks a brand-new file, so there is
    // no pre-edit content to reconstruct — still T1, just with `before` empty
    // rather than resolved.
    let op_is_create = tur.get("type").and_then(|v| v.as_str()) == Some("create");

    if let Some(orig) = original_file {
        counters.t1 += 1;
        let hash = hash_bytes(orig.as_bytes());
        push_debug(
            debug,
            session_file_label,
            file_path,
            "t1",
            Some(strip_hash_prefix(&hash)),
        );
        return;
    }
    if op_is_create {
        counters.t1 += 1;
        push_debug(debug, session_file_label, file_path, "t1", None);
        return;
    }
    if let Some(backup_name) = backups.get(file_path) {
        if let Some(sid) = session_id {
            let blob_path = source.join("file-history").join(sid).join(backup_name);
            if let Ok(bytes) = fs::read(&blob_path) {
                counters.t1_5 += 1;
                let hash = hash_bytes(&bytes);
                push_debug(
                    debug,
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
    classify_t2_or_t3(file_path, session_cwd, session_file_label, counters, debug);
}

fn classify_t2_or_t3(
    file_path: &str,
    session_cwd: Option<&Path>,
    session_file_label: &str,
    counters: &mut Counters,
    debug: &mut Vec<DebugEntry>,
) {
    let tracked = session_cwd.is_some_and(|cwd| git_path_is_tracked(cwd, file_path));
    if tracked {
        counters.t2_candidate += 1;
        push_debug(debug, session_file_label, file_path, "t2_candidate", None);
    } else {
        counters.t3 += 1;
        push_debug(debug, session_file_label, file_path, "t3", None);
    }
}

/// T2 is detected, not resolved, in this phase (P1.md): shells out to the
/// `git` CLI rather than adding a git-object-reading dependency. This
/// deviates from the originating task brief's assumption that `git2` was
/// already a workspace dependency — it is not (checked `Cargo.toml`/
/// `Cargo.lock` for both crates; no such dependency exists anywhere in this
/// repo). The existing codebase's own git-interaction idiom is exactly this
/// shape (`std::process::Command::new("git")`, see `daemon.rs`/
/// `doctorcmd.rs`), so this keeps the "no new crates without a stated
/// reason" constraint rather than adding one to match a brief that doesn't
/// match this repo's actual dependency graph.
fn git_path_is_tracked(cwd: &Path, file_path: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("ls-files")
        .arg("--error-unmatch")
        .arg("--")
        .arg(file_path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn push_debug(
    debug: &mut Vec<DebugEntry>,
    session_file: &str,
    path: &str,
    tier: &str,
    sha256: Option<String>,
) {
    debug.push(DebugEntry {
        session_file: session_file.to_string(),
        path: path.to_string(),
        tier: tier.to_string(),
        sha256,
    });
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

fn peak_rss_mb() -> f64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    if ret != 0 {
        return 0.0;
    }
    rss_raw_to_mb(usage.ru_maxrss as i64)
}

/// `ru_maxrss` is bytes on macOS, KiB on Linux (Pinned decision 5) — a wrong
/// platform constant would pass or fail AC7 for the wrong reason on CI's
/// Linux leg, so the divisor is picked by `cfg(target_os)` and this
/// conversion is unit-tested directly (below), not just the printed value.
fn rss_raw_to_mb(raw: i64) -> f64 {
    #[cfg(target_os = "macos")]
    {
        raw as f64 / (1024.0 * 1024.0)
    }
    #[cfg(target_os = "linux")]
    {
        raw as f64 / 1024.0
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        raw as f64 / (1024.0 * 1024.0)
    }
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
    #[cfg(target_os = "macos")]
    fn rss_conversion_macos_is_bytes_to_mb() {
        assert_eq!(rss_raw_to_mb(10 * 1024 * 1024), 10.0);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn rss_conversion_linux_is_kib_to_mb() {
        assert_eq!(rss_raw_to_mb(10 * 1024), 10.0);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn debug_dump_entries_is_disabled_in_release_even_with_env_set() {
        std::env::set_var("AGENTREC_IMPORT_DEBUG_ENTRIES", "1");
        assert!(!debug_dump_entries_enabled());
        std::env::remove_var("AGENTREC_IMPORT_DEBUG_ENTRIES");
    }
}
