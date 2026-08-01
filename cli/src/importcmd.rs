//! `agentrec import claude` streams Claude Code's
//! `~/.claude/projects/**/*.jsonl` transcript corpus. `--dry-run` (P1,
//! read-only) reports per-tier before-bytes reconstructability without
//! writing anything. Omitting `--dry-run` (P2, `mod persist` below) persists
//! classified turns into this repo's `log.jsonl`, scoped to sessions whose
//! `cwd` resolves under `--root` — idempotently (a deterministic id per
//! `(session_id, turn_index)` means re-running, or resuming after an
//! interruption, appends only what's missing) and with T2 candidates
//! actually resolved against a git blob (P1 only detected candidacy).
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
    // P2: `--dry-run` selects the P1 read-only classify-and-report path
    // (unchanged below, still what the P1 gate measures). Its absence now
    // means real persistence (P2), not the P1-era refusal — a bare
    // `agentrec import claude` classifies AND writes turns into this repo's
    // `log.jsonl`, scoped to sessions whose `cwd` resolves under `root`.
    if !dry_run {
        return persist::run(root, source, json);
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

// ---- report shape (Pinned decision 12, extended by decisions 13/15, item
// 6's I/O counter, and the T1.5 path-normalization fix's three extra
// `tier_counts` sub-counters below — the whole point of a fidelity report is
// that losses stay countable, so these are deliberately visible in the
// contract rather than folded silently into `debug_entries`) ---------------

#[derive(Serialize)]
struct TierCounts {
    t1: usize,
    t1_5: usize,
    /// Subset of `t1_5`: no `oldString` was available to verify the backup
    /// blob against, so this entry is classified on the blob reference alone
    /// (assumed, not content-proven). `t1_5 - t15_unverified` is the
    /// content-proven count.
    t15_unverified: usize,
    /// NOT part of `t1_5`: a backup blob resolved and was readable, but its
    /// content did not contain the edit's `oldString`, so it was refused as
    /// T1.5 (would-be-fabricated pre-state) and fell through to T2/T3.
    t15_rejected_unverifiable: usize,
    /// NOT part of `t1_5`: at least one OTHER edit to the same absolute path
    /// occurred between the snapshot that recorded this backup and the edit
    /// being classified, so the blob is a pre-*snapshot* state, not this
    /// edit's pre-*edit* state (Fix 1, T1.5 fabrication round). Distinct from
    /// `t15_rejected_unverifiable` — that one has content proof the blob is
    /// wrong; this one is refused purely on ordering, before any content
    /// check runs, because content correctness by itself is not sufficient
    /// (a stale blob's unrelated regions can still happen to contain a later
    /// edit's `oldString`, which is exactly how the fabrication defect
    /// survived the `oldString`-only guard). Falls through to T2/T3.
    t15_rejected_stale: usize,
    /// NOT part of `t1_5`: a `trackedFileBackups` key resolved to `file_path`
    /// but the referenced blob file could not be read (e.g. reaped by
    /// retention) — no content to even check. Falls through to T2/T3.
    t15_blob_missing: usize,
    /// NOT part of `t1_5`: the resolved `sessionId`/`backupFileName` pair
    /// failed path-component validation (absolute, empty, or containing a
    /// separator/`..`) before being joined into a filesystem path (Fix 3,
    /// latent-traversal hardening) — refused rather than read. Falls through
    /// to T2/T3.
    t15_rejected_unsafe_path: usize,
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
    /// D7 (P2 fix round): see `Counters::skipped_secret_path`.
    skipped_secret_path: usize,
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
            t15_unverified: counters.t15_unverified,
            t15_rejected_unverifiable: counters.t15_rejected_unverifiable,
            t15_rejected_stale: counters.t15_rejected_stale,
            t15_blob_missing: counters.t15_blob_missing,
            t15_rejected_unsafe_path: counters.t15_rejected_unsafe_path,
            t2_candidate: counters.t2_candidate,
            t3: counters.t3,
        },
        opaque_calls: counters.opaque_calls,
        mean_opaque_share_pct,
        skipped_secret_path: counters.skipped_secret_path,
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
        "  tier_counts: t1={} t1_5={} (of which t15_unverified={}) t2_candidate={} t3={}",
        report.tier_counts.t1,
        report.tier_counts.t1_5,
        report.tier_counts.t15_unverified,
        report.tier_counts.t2_candidate,
        report.tier_counts.t3
    );
    println!(
        "  t1_5 fallthroughs (NOT part of t1_5, counted toward t2_candidate/t3): \
         t15_rejected_unverifiable={} t15_rejected_stale={} t15_blob_missing={} \
         t15_rejected_unsafe_path={}",
        report.tier_counts.t15_rejected_unverifiable,
        report.tier_counts.t15_rejected_stale,
        report.tier_counts.t15_blob_missing,
        report.tier_counts.t15_rejected_unsafe_path
    );
    println!("  opaque_calls: {}", report.opaque_calls);
    println!(
        "  mean_opaque_share_pct: {:.2}",
        report.mean_opaque_share_pct
    );
    println!("  skipped_secret_path: {}", report.skipped_secret_path);
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
    /// Subset of `t1_5` (i.e. `t1_5 - t15_unverified` is the content-proven
    /// count): `oldString` was absent/empty on the entry, so the
    /// containment check could not run — classified T1.5 on the blob
    /// reference alone (best evidence available, but NOT content-proven).
    /// Also double-counted into `t1_5`; kept distinct so the fidelity report
    /// never lets an assumed match masquerade as a proven one.
    t15_unverified: usize,
    /// NOT part of `t1_5`: a backup blob resolved and was readable, but its
    /// content does not contain the edit's `oldString` — the blob is not
    /// this edit's pre-state. Resolving it anyway would fabricate a
    /// plausible-but-wrong `before`, which the "never fabricate" invariant
    /// forbids, so the entry falls through to T2/T3 instead and is counted
    /// here so the loss stays visible rather than silently folding into T3.
    t15_rejected_unverifiable: usize,
    /// NOT part of `t1_5`: at least one other edit to this same absolute
    /// path was classified since the backup currently held for it was
    /// recorded, so the blob predates that intervening edit and is not this
    /// edit's true pre-state (Fix 1). Checked — and, if true, short-circuits
    /// straight to this counter — BEFORE any `oldString` content check runs,
    /// because a stale blob's untouched regions can still coincidentally
    /// contain a later edit's `oldString` (the exact shape that let the
    /// fabrication defect pass the content-only guard). Tracked via
    /// `edits_since_backup` (per-session state alongside `backups`).
    t15_rejected_stale: usize,
    /// NOT part of `t1_5`: a `trackedFileBackups` key resolved (matched
    /// `file_path`, verbatim or after `cwd`-normalization), but
    /// `fs::read(blob_path)` failed — e.g. retention already reaped the
    /// blob (~30-day window). Distinct from `t15_rejected_unverifiable`
    /// (blob present but content doesn't match): here there is no content
    /// to even check. Falls through to T2/T3, same as before this counter
    /// existed; added so this loss is visible instead of silently absorbed.
    t15_blob_missing: usize,
    /// NOT part of `t1_5`: the `sessionId`/`backupFileName` pair that would
    /// be joined into a filesystem path failed validation (Fix 3) — absolute,
    /// empty, or containing a path separator / `..` component. Corpus-clean
    /// today (all real values match a narrow confirmed shape), but a
    /// transcript-supplied string must never be trusted unvalidated in a
    /// path join (`Path::join` silently replaces on an absolute component).
    /// Refused rather than attempted; falls through to T2/T3.
    t15_rejected_unsafe_path: usize,
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
    /// D7 (P2 fix round): a file-producing entry whose path matches
    /// `scrub::is_secret_path` — never snapshotted by the live daemon, and
    /// (as of this fix round) never persisted by `import claude` either
    /// (`withheld: true`). Checked FIRST, before any tier ladder, and
    /// excluded from T1/T1.5/T2/T3 entirely: prior to this fix, dry-run
    /// tier-counted secret files as if they were ordinary reconstructible
    /// content (and hashed their bytes into `debug_entries`), while
    /// persist withheld them — the two paths were measuring different
    /// things for the same input. Still counted toward `session_entries`
    /// (a real file-touching entry, just not tier-classified), same as
    /// every T1-T3 entry.
    skipped_secret_path: usize,
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
    // Absolute-path key -> backupFileName, harvested from `snapshot` lines
    // as we walk. `trackedFileBackups` keys are relative to the session's
    // `cwd` far more often than absolute — corpus is a live, rolling ≤30-day
    // window so the exact ratio drifts run to run (measured 2026-07-29 on
    // this machine: 1196 of 1401 keys relative at re-verification time; an
    // earlier same-day measurement on a larger pre-decay corpus snapshot saw
    // 1606 of 1883) — every key is normalized to an absolute path before
    // landing here (`normalize_backup_key`, below) so the T1.5 lookup in
    // `classify_file_entry` can stay a plain `HashMap::get(file_path)`
    // (`file_path` there is always absolute).
    //
    // Repeated-key `insert` overwriting is deliberate, not accidental: real
    // transcripts are chronological, so the LAST snapshot recorded before an
    // edit is the pre-edit version to use for that edit — corpus-validated
    // (2026-07-29): of every T1.5 candidate, the latest backup recorded
    // before the edit line was correct in 100% of cases where any recorded
    // version was correct (zero cases where an earlier version matched but
    // the latest one didn't). Do not add `@vN`-suffix parsing to pick a
    // version some other way — record order is the authority, and
    // `backupFileName` stays an opaque string, read verbatim (Pinned
    // decision 1).
    let mut backups: HashMap<String, String> = HashMap::new();
    // Fix 1 (T1.5 fabrication round): per absolute path, how many OTHER
    // file-edit entries for that path have been classified since the backup
    // currently held in `backups` for it was recorded. Reset to 0 at every
    // point `backups` gains or replaces an entry for a path (a fresh
    // snapshot is, by definition, zero edits stale); incremented once per
    // classified file-entry for a path already present here (see
    // `classify_file_entry`'s `mark_edit`). A nonzero count at
    // classification time means the blob predates an intervening edit and
    // must not be trusted as this edit's pre-state, regardless of what the
    // `oldString` containment check would say.
    let mut edits_since_backup: HashMap<String, usize> = HashMap::new();
    // Relative keys harvested before `cwd` is known yet (Pinned decision 14:
    // `cwd` is session-level and a snapshot line can legitimately precede
    // the first line that carries `cwd`). Drained into `backups` the moment
    // `cwd` becomes known (see the `cwd`-detection block below), in original
    // (chronological) order so the latest-wins rule above still holds.
    let mut pending_relative_backups: Vec<(String, String)> = Vec::new();
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
                let cwd_path = PathBuf::from(cwd);
                // Now that `cwd` is known, resolve every relative key that
                // was harvested before we could resolve it — draining in
                // order preserves "latest recorded before the edit wins".
                for (key, backup_name) in pending_relative_backups.drain(..) {
                    let normalized = normalize_backup_key(&key, &cwd_path);
                    record_backup(
                        &mut backups,
                        &mut edits_since_backup,
                        normalized,
                        backup_name,
                    );
                }
                session_cwd = Some(cwd_path);
            }
        }

        // Backup bookkeeping is harvested on the PRESENCE of
        // `snapshot.trackedFileBackups`, never gated on the line's `type`.
        // The real corpus emits these on `type: "file-history-snapshot"`
        // lines (measured 2026-07-29: 884 such lines, 0 carrying any other
        // type). Keying off a `type` literal is what silently produced
        // `t1_5 = 0` over the whole corpus on the first gate run — the
        // synthetic fixture had invented `type: "snapshot"` and the
        // classifier matched the fixture, so every test passed while no
        // real T1.5 entry ever resolved. Presence-based harvesting is
        // correct for both shapes and cannot drift with a type rename.
        if let Some(map) = value
            .pointer("/snapshot/trackedFileBackups")
            .and_then(|v| v.as_object())
        {
            for (file_path, info) in map {
                if let Some(backup_name) = info.get("backupFileName").and_then(|v| v.as_str()) {
                    // An absolute key needs no `cwd` to resolve — normalize
                    // and insert immediately regardless of whether `cwd` is
                    // known yet. A relative key needs `cwd`: normalize now
                    // if we have it, otherwise defer until the `cwd`-
                    // detection block above drains it.
                    if file_path.starts_with('/') {
                        let normalized = lexical_normalize(Path::new(file_path))
                            .to_string_lossy()
                            .into_owned();
                        record_backup(
                            &mut backups,
                            &mut edits_since_backup,
                            normalized,
                            backup_name.to_string(),
                        );
                    } else if let Some(cwd) = &session_cwd {
                        let normalized = normalize_backup_key(file_path, cwd);
                        record_backup(
                            &mut backups,
                            &mut edits_since_backup,
                            normalized,
                            backup_name.to_string(),
                        );
                    } else {
                        pending_relative_backups.push((file_path.clone(), backup_name.to_string()));
                    }
                }
            }
            // These lines carry bookkeeping only, never a tool result.
            if value.get("toolUseResult").is_none() {
                return;
            }
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
            &mut edits_since_backup,
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

/// Resolves a raw `trackedFileBackups` key to the absolute path it names.
/// An already-absolute key (`/`-prefixed) is lexically normalized as-is. A
/// relative key is joined lexically against `cwd`, then normalized —
/// deliberately never `fs::canonicalize`d, since transcript paths frequently
/// name a file that no longer exists on disk (canonicalize would fail on
/// exactly the paths this needs to resolve). A leading `./` is stripped
/// before joining so the resulting string never carries a literal `./`
/// component (`Path::join` does not collapse one on its own).
///
/// Fix 2 (T1.5 fabrication round): a bare `./`-strip does not collapse `..`
/// — a key like `../beta/src/composer.ts` joined against `cwd`
/// produces a path string that can never equal any real (already-absolute,
/// `..`-free) `filePath`, so every such key silently failed to match,
/// looking like "no `..` keys ever resolve" when the real cause was that the
/// join was never lexically collapsed. `lexical_normalize` fixes that.
fn normalize_backup_key(key: &str, cwd: &Path) -> String {
    if key.starts_with('/') {
        return lexical_normalize(Path::new(key))
            .to_string_lossy()
            .into_owned();
    }
    let stripped = key.strip_prefix("./").unwrap_or(key);
    lexical_normalize(&cwd.join(stripped))
        .to_string_lossy()
        .into_owned()
}

/// Lexically normalizes `.` and `..` path components without touching the
/// filesystem (unlike `fs::canonicalize`, which requires every component to
/// actually exist — fails on exactly the transcript-referenced paths this
/// importer needs to resolve, since those files are routinely stale,
/// renamed, or deleted by the time import runs). Mirrors the walk-and-pop
/// behavior of Python's `os.path.normpath`: a `..` pops the preceding
/// `Normal` component; a `..` with nothing poppable (already at the root, or
/// a leading `..` on a relative path) is preserved rather than allowed to
/// escape — the degenerate "climb above root" case is the one place this
/// deliberately does NOT mirror `..`'s filesystem meaning, since there is no
/// path above `/` to represent.
fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) => {
                    // Guard the degenerate case: `..` above the filesystem
                    // root has nowhere to go. Drop it rather than escape.
                }
                _ => out.push(component),
            },
            other => out.push(other),
        }
    }
    out.into_iter().collect()
}

/// Records a resolved backup for `path`, resetting `edits_since_backup` to 0
/// ONLY when `backup_name` is genuinely different from whatever is already
/// stored for `path` — never unconditionally on every `trackedFileBackups`
/// sighting.
///
/// Ground-truth corpus validation (T1.5 fabrication round, post-Fix-1) found
/// real transcripts commonly emit several manifest-style `file-history-
/// snapshot` lines in a row that all re-list the SAME `backupFileName` for a
/// path whose backup hasn't actually changed since the last one (confirmed
/// on-disk: identical `backupFileName` AND identical `backupTime` repeated
/// across consecutive snapshot lines). Resetting the staleness counter on
/// every such re-announcement — as an earlier version of this fix did —
/// wrongly cleared "an edit already happened since this backup" for a
/// redundant re-announcement of the exact same (already-stale) blob,
/// re-opening the fabrication window Fix 1 exists to close. A change in
/// `backup_name` (a real new version) still resets normally.
fn record_backup(
    backups: &mut HashMap<String, String>,
    edits_since_backup: &mut HashMap<String, usize>,
    path: String,
    backup_name: String,
) {
    let is_new_backup = backups.get(&path) != Some(&backup_name);
    backups.insert(path.clone(), backup_name);
    if is_new_backup {
        edits_since_backup.insert(path, 0);
    }
}

/// Fix 3 (T1.5 fabrication round, latent-traversal hardening): rejects a
/// transcript-supplied `sessionId`/`backupFileName` string before it is
/// joined into a filesystem path. `Path::join` silently REPLACES the whole
/// accumulated path when the joined component is itself absolute, and does
/// not reject a `..` component — so an unvalidated value could otherwise
/// escape `<source>/file-history/` entirely. The corpus is clean today (see
/// module-level notes), so this is a hardening guard against a shape that
/// hasn't been observed live, not a fix for an observed failure — same
/// posture as `agentrec_core::store`'s
/// `malformed_hash_rejects_traversal_components`.
fn is_safe_path_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !Path::new(s).is_absolute()
}

/// True iff `needle` occurs as a contiguous byte sequence anywhere in
/// `haystack`. Used to verify a resolved T1.5 blob actually contains an
/// edit's `oldString` before trusting it as that edit's pre-state — plain
/// byte search rather than `str`-based, since a resolved blob is not
/// guaranteed to be valid UTF-8 (binary files go through the same backup
/// path).
fn bytes_contain(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Before-bytes ladder, first hit wins (P1.md Changes / Pinned decision 1).
///
/// Fix 1 (T1.5 fabrication round): `edits_since_backup` tracks, per absolute
/// path, how many file-edit entries for that path have already been
/// classified since the backup currently held for it (in `backups`) was
/// recorded. Every call into this function for a given `file_path` reads
/// that count BEFORE deciding anything (a nonzero count means the blob
/// predates an intervening edit and cannot be this edit's true pre-state,
/// full stop — checked ahead of and independent of the `oldString`
/// containment check, since a stale blob can still coincidentally contain a
/// later edit's `oldString` in a region an earlier edit didn't touch), then
/// increments it exactly once via `mark_edit` right before returning or
/// falling through — so the NEXT entry for this path sees this edit
/// reflected. `mark_edit` is a no-op for a path with no tracked backup.
#[allow(clippy::too_many_arguments)]
fn classify_file_entry(
    tur: &Value,
    file_path: &str,
    source: &Path,
    session_id: Option<&str>,
    session_cwd: Option<&Path>,
    backups: &HashMap<String, String>,
    edits_since_backup: &mut HashMap<String, usize>,
    session_file_label: &str,
    counters: &mut Counters,
    debug: &mut DebugSink,
    git_cache: &mut GitTrackCache,
    session_entries: &mut usize,
) {
    let mark_edit = |edits_since_backup: &mut HashMap<String, usize>| {
        if let Some(count) = edits_since_backup.get_mut(file_path) {
            *count += 1;
        }
    };

    // D7 (P2 fix round): checked FIRST, before any tier ladder — a secret
    // file is never tier-counted (previously it was silently classified
    // T1/T1.5/T2/T3 like any other file, and its bytes hashed into
    // `debug_entries`) or persisted (`classify_and_resolve` withholds it).
    // This is real corpus-shape measurement drift between the two paths,
    // fixed here so the fidelity report actually describes what
    // persistence does.
    if agentrec_core::scrub::is_secret_path(file_path) {
        counters.skipped_secret_path += 1;
        *session_entries += 1;
        debug.push(session_file_label, file_path, "secret_path", None);
        mark_edit(edits_since_backup);
        return;
    }

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
        mark_edit(edits_since_backup);
        return;
    }
    if op_is_create {
        counters.t1 += 1;
        *session_entries += 1;
        debug.push(session_file_label, file_path, "t1", None);
        mark_edit(edits_since_backup);
        return;
    }
    if let Some(backup_name) = backups.get(file_path) {
        if let Some(sid) = session_id {
            let stale_by_intervening_edit =
                edits_since_backup.get(file_path).copied().unwrap_or(0) > 0;
            if stale_by_intervening_edit {
                // At least one other edit to this same path was classified
                // since this backup was recorded — the blob is a
                // pre-*snapshot* state, not this edit's pre-*edit* state.
                // Refused before any content check runs; falls through to
                // T2/T3 exactly as if no backup existed.
                counters.t15_rejected_stale += 1;
            } else if !is_safe_path_component(sid) || !is_safe_path_component(backup_name) {
                // Fix 3: refuse to join an unvalidated transcript-supplied
                // component into a filesystem path. Corpus-clean today, but
                // latent — never errors the run, just counted and treated
                // as if the blob could not be resolved.
                counters.t15_rejected_unsafe_path += 1;
            } else {
                let blob_path = source.join("file-history").join(sid).join(backup_name);
                if let Ok(bytes) = fs::read(&blob_path) {
                    // "Never fabricate" invariant (PROTOCOL.md /
                    // IMPLEMENTATION.md): resolving `before` bytes from a
                    // blob that is NOT actually this edit's pre-state would
                    // fabricate a plausible-but-wrong pre-state. When
                    // `oldString` is present and non-empty, the blob must
                    // also be verified to contain it before it can be
                    // trusted as T1.5 — this check runs in addition to (not
                    // instead of) the staleness check above; both must hold.
                    let old_string = tur.get("oldString").and_then(|v| v.as_str());
                    match old_string {
                        Some(old) if !old.is_empty() => {
                            if bytes_contain(&bytes, old.as_bytes()) {
                                counters.t1_5 += 1;
                                *session_entries += 1;
                                let hash = hash_bytes(&bytes);
                                debug.push(
                                    session_file_label,
                                    file_path,
                                    "t1_5",
                                    Some(strip_hash_prefix(&hash)),
                                );
                                mark_edit(edits_since_backup);
                                return;
                            }
                            // The blob does not contain the edit's
                            // `oldString` — it is not this edit's pre-state.
                            // Fall through to T2/T3 exactly as if no backup
                            // existed, and count the rejection so the loss
                            // stays visible rather than silently folding
                            // into T3.
                            counters.t15_rejected_unverifiable += 1;
                        }
                        _ => {
                            // No `oldString` on this entry (some ops don't
                            // carry one), so the containment check can't
                            // run. The staleness check above already holds
                            // (no intervening edit), which is the
                            // structural argument that this blob IS the
                            // pre-edit state even without content proof —
                            // classify T1.5, but count it as
                            // unverified/assumed rather than content-proven.
                            counters.t1_5 += 1;
                            counters.t15_unverified += 1;
                            *session_entries += 1;
                            let hash = hash_bytes(&bytes);
                            debug.push(
                                session_file_label,
                                file_path,
                                "t1_5",
                                Some(strip_hash_prefix(&hash)),
                            );
                            mark_edit(edits_since_backup);
                            return;
                        }
                    }
                } else {
                    // Backup referenced but missing (e.g. retention already
                    // reaped it, ~30-day window) — falls through to T2/T3,
                    // never errors, but the loss is counted so it stays
                    // visible.
                    counters.t15_blob_missing += 1;
                }
            }
        }
    }
    mark_edit(edits_since_backup);
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
    /// Repo root for `cwd` (P2's T2 git-blob resolution needs only this,
    /// not the tracked-file set `is_tracked` also loads) — shares the same
    /// cache and `load` so a `cwd` already resolved by one caller is never
    /// re-shelled-out-to-git by the other.
    fn repo_root(&mut self, cwd: &Path) -> Option<PathBuf> {
        let key = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let entry = self
            .by_cwd
            .entry(key.clone())
            .or_insert_with(|| Self::load(&key));
        entry.as_ref().map(|(root, _)| root.clone())
    }

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

// ---- P2: persistence (`agentrec import claude`, no `--dry-run`) -----------
//
// Reuses the P1 ladder's low-level, previously-hardened primitives
// (`bytes_contain`, `is_safe_path_component`, `record_backup`,
// `normalize_backup_key`, `lexical_normalize`, `GitTrackCache`) verbatim —
// those are exactly where the T1.5 fabrication bugs lived, so this
// deliberately does NOT re-derive them. The tier *decision* ladder itself is
// intentionally re-expressed here (not shared with `classify_file_entry`)
// because P1's dry-run path must stay byte-for-byte unchanged (its gate
// already passed and its RSS/perf figures are cited elsewhere) — dry-run
// never reads a T2 git blob at all (module doc: "Detected only in this
// phase"), while persistence must.
mod persist {
    use super::*;
    use agentrec_core::record::{skip_reason, FileEntry, LogRecord, TurnRecord};
    use agentrec_core::store::{BlobStore, PutResult};
    use std::process::Command;

    pub fn run(root: &Path, source: Option<PathBuf>, json: bool) -> Result<(), String> {
        let src = match source {
            Some(s) => s,
            None => default_claude_source()?,
        };
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

        let root_canon = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let log_path = crate::log_path(root);
        // D8 (P2 fix round, documented not fixed): `load_log` silently
        // skips any line it can't parse — including a torn LAST line, its
        // documented tolerant-parsing contract (`record.rs::load_log`'s own
        // doc comment: "a bad line must not wipe history"). If that torn
        // line was this importer's own most-recently-appended turn (only
        // reachable via power loss mid-write, NOT a `kill -9`: `append_log`
        // fsyncs after a single `write_all` of the whole line, so a
        // `kill -9` can only ever land before or after that atomic write,
        // never mid-line), `existing_ids` would miss it and a resume would
        // re-append a duplicate turn for that one id. Accepted risk, not
        // fixed this round — `load_log`'s tolerance is shared, load-bearing
        // infrastructure (every consumer of `log.jsonl` depends on it) and
        // changing its contract is out of scope here.
        let existing_ids: HashSet<String> = agentrec_core::record::load_log(&log_path)
            .into_iter()
            .filter_map(|r| match r {
                LogRecord::Turn(t) => Some(t.id),
                LogRecord::Epoch(_) => None,
            })
            .collect();

        let store = BlobStore::new(crate::objects_dir(root));
        let mut git_cache = GitTrackCache::default();
        let mut t2 = T2Counters::default();
        let mut t15 = T15Counters::default();
        let mut scope = ScopeCounters::default();
        let mut oracle = OracleCounters::default();
        let oracle_on = t2_oracle_enabled();

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

        let mut appended = 0usize;
        for project_dir in dirs {
            let mut session_files: Vec<PathBuf> = fs::read_dir(&project_dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "jsonl"))
                .collect();
            session_files.sort();

            // D10/D11 (P2 fix round, documented not fixed — perf/posture
            // only, correctness is intact): `persist_session_file` calls
            // `store.put_result` for every resolved before/after blob
            // BEFORE the `existing_ids` idempotency check below runs, so an
            // `appended: 0` re-run still rewrites the CAS object set
            // (dedup-safe, same hash, wasted work) and re-spawns every
            // `git log`/`git show` for T2 candidates. And each turn is
            // appended through its own separate `append_log_locked` call
            // (lock acquired and released per record, not held across the
            // whole batch), so `purge --log-duplicates` could in principle
            // interleave a rewrite between two of this run's appends. The
            // append-only invariant itself is never at risk either way —
            // `log.lock` IS taken for every append, and this path only ever
            // appends — so this is a performance/posture item, not a
            // correctness one; not fixed this round.
            for file in session_files {
                let records = persist_session_file(
                    &file,
                    &src,
                    root,
                    &root_canon,
                    &store,
                    &mut git_cache,
                    &mut t2,
                    &mut t15,
                    &mut scope,
                    oracle_on.then_some(&mut oracle),
                );
                for record in records {
                    if let LogRecord::Turn(t) = &record {
                        if existing_ids.contains(&t.id) {
                            continue; // idempotent resume/re-run
                        }
                    }
                    crate::loglock::append_log_locked(&log_path, &record)?;
                    appended += 1;
                }
            }
        }

        if json {
            let mut obj = serde_json::json!({
                "appended": appended,
                "t2_resolution": {
                    "resolved": t2.resolved,
                    "no_commit": t2.no_commit,
                    "blob_missing": t2.blob_missing,
                    "rejected_unverifiable": t2.rejected_unverifiable,
                    "no_oldstring": t2.no_oldstring,
                    "rejected_prior_edit_in_session": t2.rejected_prior_edit_in_session,
                },
                // D7: parity with dry-run's `t15_rejected_stale`/
                // `t15_rejected_unsafe_path`/`t15_blob_missing`/
                // `t15_rejected_unverifiable` — see `T15Counters`' doc.
                "t15_diagnostics": {
                    "rejected_stale": t15.rejected_stale,
                    "rejected_unsafe_path": t15.rejected_unsafe_path,
                    "blob_missing": t15.blob_missing,
                    "rejected_unverifiable": t15.rejected_unverifiable,
                },
                // BLOCKER 1: see `ScopeCounters`' doc — a file-producing
                // entry not lexically under its session's `cwd` used to
                // silently vanish; now counted (never recovered/imported).
                "skipped_out_of_cwd": scope.skipped_out_of_cwd,
            });
            if oracle_on {
                obj["t2_oracle"] = serde_json::json!({
                    "denominator": oracle.denominator,
                    "mismatches": oracle.mismatches,
                });
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&obj).map_err(|e| e.to_string())?
            );
        } else {
            println!("appended: {appended}");
            let candidates = t2.resolved
                + t2.no_commit
                + t2.blob_missing
                + t2.rejected_unverifiable
                + t2.no_oldstring
                + t2.rejected_prior_edit_in_session;
            println!(
                "t2_resolution: resolved={} of {} candidate(s) attempted \
                 (no_commit={} blob_missing={} rejected_unverifiable={} no_oldstring={} \
                 rejected_prior_edit_in_session={})",
                t2.resolved,
                candidates,
                t2.no_commit,
                t2.blob_missing,
                t2.rejected_unverifiable,
                t2.no_oldstring,
                t2.rejected_prior_edit_in_session
            );
            println!(
                "t15_diagnostics: rejected_stale={} rejected_unsafe_path={} blob_missing={} \
                 rejected_unverifiable={}",
                t15.rejected_stale,
                t15.rejected_unsafe_path,
                t15.blob_missing,
                t15.rejected_unverifiable
            );
            println!("skipped_out_of_cwd: {}", scope.skipped_out_of_cwd);
            if oracle_on {
                println!(
                    "t2_oracle (AC5b, T1 entries only): mismatches={} of {} both-resolved",
                    oracle.mismatches, oracle.denominator
                );
            }
        }
        Ok(())
    }

    #[derive(Default)]
    struct T2Counters {
        resolved: usize,
        no_commit: usize,
        blob_missing: usize,
        rejected_unverifiable: usize,
        no_oldstring: usize,
        /// AC5b gate 1: this session already edited the same path earlier —
        /// refused before any git process even runs.
        rejected_prior_edit_in_session: usize,
    }

    /// D7 (P2 fix round): the persist-time T1.5 branch used to fall
    /// through to T2/T3 on `stale`/unsafe-path/blob-missing without
    /// counting WHY — the dry-run classifier's equivalent
    /// `t15_rejected_stale`/`t15_rejected_unsafe_path`/`t15_blob_missing`/
    /// `t15_rejected_unverifiable` counters had no persist-side
    /// counterpart, so the two paths' fidelity reports were not directly
    /// comparable. Restored here, named identically to the dry-run report
    /// contract (Pinned decision 12's amendment).
    #[derive(Default)]
    struct T15Counters {
        rejected_stale: usize,
        rejected_unsafe_path: usize,
        blob_missing: usize,
        rejected_unverifiable: usize,
    }

    /// BLOCKER 1 (P2 integration-gate fix round): counts file-producing
    /// entries whose `filePath` is NOT lexically under their session's
    /// `session_cwd` (Pinned decision 14: `cwd` is session-level, first-
    /// line-wins — untouched by this fix). Real-corpus measurement
    /// (read-only, 1,660 top-level session files): **507 of 2,170 (23.4%)**
    /// non-sidechain file-producing entries fall here — 24 are mid-session
    /// `cwd` moves, 196 sit under the first `cwd`'s PARENT (would be
    /// importable under a repo-root `--root`, but that is a founder-scope
    /// recall decision not made here), the remainder are genuinely
    /// out-of-tree. An earlier version of this code's doc comment claimed
    /// `cwd` "is lexically a prefix of `file_path` in every real
    /// transcript" — that was never measured and is false; the entry used
    /// to simply vanish (no `FileEntry`, no counter, no stderr line) when
    /// it wasn't. This struct exists so that loss is countable, per this
    /// round's own repeated invariant. Recovery (importing these entries
    /// under a wider scope) is explicitly NOT authorized this round.
    #[derive(Default)]
    struct ScopeCounters {
        skipped_out_of_cwd: usize,
    }

    #[derive(Default)]
    struct OracleCounters {
        denominator: usize,
        mismatches: usize,
    }

    /// AC5b's fabrication oracle (governing-lesson channel): when enabled
    /// (debug builds only), every T1 entry (true pre-edit bytes known via
    /// `originalFile`) ALSO attempts git-blob resolution — bypassing T1's
    /// normal short-circuit — purely to compare, never to change what gets
    /// persisted. `strings target/release/agentrec | grep
    /// AGENTREC_IMPORT_T2_ORACLE` must print nothing.
    fn t2_oracle_enabled() -> bool {
        #[cfg(debug_assertions)]
        {
            std::env::var("AGENTREC_IMPORT_T2_ORACLE").as_deref() == Ok("1")
        }
        #[cfg(not(debug_assertions))]
        {
            false
        }
    }

    struct PendingTurn {
        turn_index: usize,
        prompt_raw: Option<String>,
        started: Option<String>,
        ended: Option<String>,
        files: Vec<FileEntry>,
    }

    impl PendingTurn {
        fn new(turn_index: usize) -> Self {
            PendingTurn {
                turn_index,
                prompt_raw: None,
                started: None,
                ended: None,
                files: Vec::new(),
            }
        }

        // RFC 3339 timestamps compare lexically in time order at this
        // corpus's fixed-width format (mirrors `readcmds.rs::has_gap_after`,
        // an existing convention in this codebase).
        fn touch_ts(&mut self, ts: &str) {
            if self.started.as_deref().is_none_or(|s| ts < s) {
                self.started = Some(ts.to_string());
            }
            if self.ended.as_deref().is_none_or(|e| ts > e) {
                self.ended = Some(ts.to_string());
            }
        }
    }

    /// A line is a genuine user-authored turn boundary iff `message.role ==
    /// "user"` and its content is NOT a `tool_result` block (the same
    /// structural test the dry-run path uses, Pinned decision 4). Judgment
    /// call (not specified by P2.md): real transcripts mix true user prompts
    /// with skill-injected / slash-command-expanded "user"-role lines: this
    /// over-segments turns in that case, but never mis-resolves file bytes —
    /// every file entry still resolves `before`/`after` from its own line's
    /// content and timestamp, never the turn's aggregate. Turn granularity
    /// is cosmetic; per-entry byte resolution is not.
    fn is_genuine_user_prompt(value: &Value) -> bool {
        let Some(msg) = value.get("message").and_then(|m| m.as_object()) else {
            return false;
        };
        if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
            return false;
        }
        let is_tool_result = msg
            .get("content")
            .and_then(|c| c.as_array())
            .map(|items| {
                items
                    .iter()
                    .any(|item| item.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
            })
            .unwrap_or(false);
        !is_tool_result
    }

    fn extract_user_text(value: &Value) -> Option<String> {
        let content = value.get("message")?.get("content")?;
        if let Some(s) = content.as_str() {
            return (!s.is_empty()).then(|| s.to_string());
        }
        let arr = content.as_array()?;
        let mut out = String::new();
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
        }
        (!out.is_empty()).then_some(out)
    }

    /// Deterministic id (idempotency key, Pinned P2.md contract): a function
    /// of `(session_id, turn_index)` only — never wall-clock, never a
    /// process-local counter — so a kill-9'd-and-resumed run and an
    /// uninterrupted run mint byte-identical ids for the same logical turn
    /// (AC2).
    fn deterministic_turn_id(session_id: &str, turn_index: usize) -> String {
        let hash =
            agentrec_core::store::hash_bytes(format!("{session_id}:{turn_index}").as_bytes());
        let hex = hash.strip_prefix("sha256:").unwrap_or(&hash);
        format!("t_imp_{}", &hex[..26.min(hex.len())])
    }

    /// One session file end to end: segments it into candidate turns, then
    /// (only for the ones scoped to `root` and not already in
    /// `existing_ids`, checked by the caller) returns fully-formed
    /// `LogRecord::Turn`s ready to append, in chronological order.
    #[allow(clippy::too_many_arguments)]
    fn persist_session_file(
        path: &Path,
        source: &Path,
        root: &Path,
        root_canon: &Path,
        store: &BlobStore,
        git_cache: &mut GitTrackCache,
        t2: &mut T2Counters,
        t15: &mut T15Counters,
        scope: &mut ScopeCounters,
        mut oracle: Option<&mut OracleCounters>,
    ) -> Vec<LogRecord> {
        let Ok(file) = File::open(path) else {
            return vec![];
        };

        let mut session_cwd: Option<PathBuf> = None;
        let mut session_id: Option<String> = None;
        let mut backups: HashMap<String, String> = HashMap::new();
        let mut edits_since_backup: HashMap<String, usize> = HashMap::new();
        let mut pending_relative_backups: Vec<(String, String)> = Vec::new();
        // AC5b hardening (advisor-directed after the real-corpus oracle
        // measured ~37% fabrication with `oldString` containment alone):
        // per absolute path, how many EARLIER file-edit entries this
        // session already classified for it — unlike `edits_since_backup`
        // (which only has keys for paths that ever had a tracked backup),
        // this has a key for every path any entry touches, so it also
        // catches T2 candidates (no backup at all). A nonzero count means
        // an uncommitted intervening edit exists by construction (this
        // session already edited the path earlier), so the latest commit
        // cannot be THIS edit's pre-state regardless of what content
        // matching says — mirrors Fix 1's staleness argument, generalized
        // to paths with no tracked backup.
        let mut session_edit_counts: HashMap<String, usize> = HashMap::new();

        let mut turn_index = 0usize;
        let mut cur = PendingTurn::new(0);
        let mut finished: Vec<PendingTurn> = Vec::new();

        each_raw_line(file, |outcome| {
            let line = match outcome {
                LineOutcome::Line(s) => s,
                LineOutcome::NonUtf8 => return,
            };
            if line.trim().is_empty() {
                return;
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                return;
            };

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
            let line_ts = value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .map(str::to_string);

            if session_cwd.is_none() {
                if let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()) {
                    let cwd_path = PathBuf::from(cwd);
                    for (key, backup_name) in pending_relative_backups.drain(..) {
                        let normalized = normalize_backup_key(&key, &cwd_path);
                        record_backup(
                            &mut backups,
                            &mut edits_since_backup,
                            normalized,
                            backup_name,
                        );
                    }
                    session_cwd = Some(cwd_path);
                }
            }

            if let Some(map) = value
                .pointer("/snapshot/trackedFileBackups")
                .and_then(|v| v.as_object())
            {
                for (file_path, info) in map {
                    if let Some(backup_name) = info.get("backupFileName").and_then(|v| v.as_str()) {
                        if file_path.starts_with('/') {
                            let normalized = lexical_normalize(Path::new(file_path))
                                .to_string_lossy()
                                .into_owned();
                            record_backup(
                                &mut backups,
                                &mut edits_since_backup,
                                normalized,
                                backup_name.to_string(),
                            );
                        } else if let Some(cwd) = &session_cwd {
                            let normalized = normalize_backup_key(file_path, cwd);
                            record_backup(
                                &mut backups,
                                &mut edits_since_backup,
                                normalized,
                                backup_name.to_string(),
                            );
                        } else {
                            pending_relative_backups
                                .push((file_path.clone(), backup_name.to_string()));
                        }
                    }
                }
                if value.get("toolUseResult").is_none() {
                    return;
                }
            }

            // Fix 2 (P2 integration-gate fix round): sidechain check moved
            // ahead of the turn-boundary check — an `isSidechain:true`
            // "user"-role line must never seed a turn boundary or have its
            // text persisted as a prompt. Real-corpus measurement: 0 such
            // lines across all 1,660 top-level session files (latent, not
            // yet observed), but a top-level file COULD carry one and the
            // prior ordering would have silently promoted sidechain text
            // into a persisted prompt blob.
            if !is_sidechain_field && is_genuine_user_prompt(&value) {
                if let Some(text) = extract_user_text(&value) {
                    if cur.prompt_raw.is_some() || !cur.files.is_empty() {
                        turn_index += 1;
                        finished.push(std::mem::replace(&mut cur, PendingTurn::new(turn_index)));
                    }
                    cur.prompt_raw = Some(text);
                    if let Some(ts) = &line_ts {
                        cur.touch_ts(ts);
                    }
                    return;
                }
            }

            let is_tool_result_shaped = value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
                .map(|items| {
                    items.iter().any(|item| {
                        item.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                    })
                })
                .unwrap_or(false);
            let top_level_tur = value.get("toolUseResult");
            if is_tool_result_shaped && top_level_tur.is_none() {
                return;
            }
            let Some(tur) = top_level_tur else {
                return;
            };
            if is_sidechain_field {
                return;
            }
            let Some(file_path) = tur.get("filePath").and_then(|v| v.as_str()) else {
                return; // opaque call
            };

            if let Some(ts) = &line_ts {
                cur.touch_ts(ts);
            }

            if let Some(entry) = classify_and_resolve(
                tur,
                file_path,
                source,
                session_id.as_deref(),
                session_cwd.as_deref(),
                &backups,
                &mut edits_since_backup,
                &mut session_edit_counts,
                line_ts.as_deref(),
                root_canon,
                store,
                git_cache,
                t2,
                t15,
                scope,
                oracle.as_deref_mut(),
            ) {
                cur.files.push(entry);
            }
        });

        if cur.prompt_raw.is_some() || !cur.files.is_empty() {
            finished.push(cur);
        }

        let Some(cwd) = session_cwd else {
            return vec![];
        };
        let Ok(cwd_canon) = fs::canonicalize(&cwd) else {
            return vec![];
        };
        if !cwd_canon.starts_with(root_canon) {
            return vec![]; // out of scope for this repo (Pinned decision 10)
        }
        let Some(sid) = session_id else {
            return vec![];
        };

        let mut out = Vec::new();
        for t in finished {
            // Judgment call (not specified by P2.md): a turn with zero
            // touched files carries nothing for agentrec's file-recovery
            // purpose and is never persisted — only granularity is lost,
            // never a file-entry's byte resolution.
            if t.files.is_empty() {
                continue;
            }
            let id = deterministic_turn_id(&sid, t.turn_index);
            let (prompt_ref, prompt_excerpt) = match &t.prompt_raw {
                Some(text) => {
                    let excerpt = agentrec_core::scrub::excerpt(text);
                    let full = agentrec_core::scrub::scrub(text);
                    let prompt_ref = match store.put_result(full.as_bytes()) {
                        PutResult::Stored { hash, .. } => Some(hash),
                        PutResult::OverCap | PutResult::IoError(_) => None,
                    };
                    (prompt_ref, Some(excerpt))
                }
                None => (None, None),
            };
            let started = t
                .started
                .clone()
                .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string());
            let ended = t.ended.clone().unwrap_or_else(|| started.clone());
            out.push(LogRecord::Turn(TurnRecord {
                v: 1,
                id,
                grade: "rich".to_string(),
                truncated: false,
                started,
                ended,
                tool: Some("claude".to_string()),
                model: None,
                session: Some(sid.clone()),
                root: root.to_string_lossy().to_string(),
                prompt_ref,
                prompt_excerpt,
                merges: vec![],
                imported: Some(true),
                files_complete: Some(false),
                files: t.files,
            }));
        }
        out
    }

    /// The persist-time before/after ladder. Shares the low-level primitives
    /// with the dry-run classifier (see module doc above) but is a distinct
    /// function because it (a) returns real bytes, not just a tier label,
    /// and (b) is the only place T2 candidates are actually resolved against
    /// a git blob (P1 only detects candidacy).
    #[allow(clippy::too_many_arguments)]
    fn classify_and_resolve(
        tur: &Value,
        file_path: &str,
        source: &Path,
        session_id: Option<&str>,
        session_cwd: Option<&Path>,
        backups: &HashMap<String, String>,
        edits_since_backup: &mut HashMap<String, usize>,
        session_edit_counts: &mut HashMap<String, usize>,
        line_ts: Option<&str>,
        root_canon: &Path,
        store: &BlobStore,
        git_cache: &mut GitTrackCache,
        t2: &mut T2Counters,
        t15: &mut T15Counters,
        scope: &mut ScopeCounters,
        oracle: Option<&mut OracleCounters>,
    ) -> Option<FileEntry> {
        // `file_path` (from the transcript) and `root` (from `--root`) are
        // both un-canonicalized strings, but `root_canon` IS canonicalized
        // (symlink-resolved, e.g. macOS `/tmp` -> `/private/tmp`) — comparing
        // a raw `file_path` against a canonicalized root directly fails
        // whenever the root sits behind a symlinked path segment, even
        // though the two names the SAME directory. Route `file_path` through
        // `session_cwd` and canonicalize THAT, so both sides of the final
        // `strip_prefix` went through the same resolution.
        //
        // BLOCKER 1 (P2 integration-gate fix round): this used to assume
        // `session_cwd` is lexically a prefix of `file_path` "in every real
        // transcript" — that was never measured and is false. Real-corpus
        // measurement (read-only, 1,660 top-level session files): 507 of
        // 2,170 (23.4%) non-sidechain file-producing entries are NOT under
        // their session's first `cwd` (24 mid-session `cwd` moves, 196
        // under the first `cwd`'s PARENT — would be importable under a
        // repo-root `--root`, a founder-scope recall decision not made
        // here — the remainder genuinely out-of-tree). Previously this
        // entry simply vanished: no `FileEntry`, no counter, no stderr
        // line. Now it is counted (`ScopeCounters::skipped_out_of_cwd`)
        // and still refused — countability, not recovery; recovering these
        // by widening scope is explicitly NOT authorized this round.
        let cwd = session_cwd?;
        let rel_to_cwd = match Path::new(file_path).strip_prefix(cwd) {
            Ok(rel) => rel,
            Err(_) => {
                scope.skipped_out_of_cwd += 1;
                return None;
            }
        };
        let cwd_canon = fs::canonicalize(cwd).ok()?;
        let abs = lexical_normalize(&cwd_canon.join(rel_to_cwd));

        let old_string = tur.get("oldString").and_then(|v| v.as_str());
        let new_string = tur.get("newString").and_then(|v| v.as_str());
        let content_field = tur.get("content").and_then(|v| v.as_str());
        let original_file = tur.get("originalFile").and_then(|v| v.as_str());
        let op_is_create = tur.get("type").and_then(|v| v.as_str()) == Some("create");
        let structured_patch = tur.get("structuredPatch");

        // AC5b hardening: count of earlier entries THIS SESSION already
        // classified for this exact path, read BEFORE this entry is marked
        // (so entry N sees the count contributed by entries 0..N-1). A
        // nonzero count means an uncommitted intervening edit exists by
        // construction, which the real-corpus oracle measurement (~37%
        // fabrication with `oldString` containment alone) showed content
        // matching cannot substitute for.
        let abs_key = abs.to_string_lossy().into_owned();
        let prior_edits_this_path = session_edit_counts.get(&abs_key).copied().unwrap_or(0);
        *session_edit_counts.entry(abs_key).or_insert(0) += 1;

        // AC5b oracle: measured independent of `--root` scope (below) — the
        // fabrication-rate question ("does the guarded T2 path ever still
        // disagree with a known-true T1 pre-edit state") is corpus-wide by
        // nature; gating it on root scope would silently zero out the
        // measurement for a `--root` that happens to match no real session
        // (which is every synthetic-fixture `--root`, and most single-repo
        // developer machines run against ONE `--root` per invocation).
        if let (Some(oracle), Some(orig)) = (oracle, original_file) {
            let mut scratch = T2Counters::default();
            if let Some(git_bytes) = resolve_t2_before(
                &abs,
                line_ts,
                old_string,
                structured_patch,
                prior_edits_this_path,
                git_cache,
                &mut scratch,
            ) {
                oracle.denominator += 1;
                if git_bytes != orig.as_bytes() {
                    oracle.mismatches += 1;
                }
            }
        }

        let rel = abs
            .strip_prefix(root_canon)
            .ok()?
            .to_string_lossy()
            .into_owned();
        if rel.is_empty() {
            return None;
        }

        let mark_edit = |edits_since_backup: &mut HashMap<String, usize>| {
            if let Some(count) = edits_since_backup.get_mut(file_path) {
                *count += 1;
            }
        };

        if agentrec_core::scrub::is_secret_path(&rel) {
            mark_edit(edits_since_backup);
            return Some(FileEntry {
                path: rel,
                before: None,
                after: None,
                op: if op_is_create { "create" } else { "modify" }.to_string(),
                skipped: false,
                withheld: true,
                baseline_unknown: false,
                skipped_reason: None,
                after_synthesized: None,
                link_kind: None,
                attribution: None,
            });
        }

        let before_bytes: Option<Vec<u8>>;
        let op: String;

        if let Some(orig) = original_file {
            before_bytes = Some(orig.as_bytes().to_vec());
            op = "modify".to_string();
            mark_edit(edits_since_backup);
        } else if op_is_create {
            before_bytes = None;
            op = "create".to_string();
            mark_edit(edits_since_backup);
        } else if let Some(backup_name) = backups.get(file_path) {
            let stale = edits_since_backup.get(file_path).copied().unwrap_or(0) > 0;
            let mut resolved: Option<Vec<u8>> = None;
            // D7 (P2 fix round): these three counters restore parity with
            // the dry-run classifier's `t15_rejected_stale`/
            // `t15_rejected_unsafe_path`/`t15_blob_missing` — the persist
            // path used to fall through to T2/T3 on each of these without
            // recording why, so the two fidelity reports weren't directly
            // comparable (the exact measurement-drift shape this repo's
            // governing lesson warns about).
            if stale {
                t15.rejected_stale += 1;
            } else if let Some(sid) = session_id {
                if !is_safe_path_component(sid) || !is_safe_path_component(backup_name) {
                    t15.rejected_unsafe_path += 1;
                } else {
                    let blob_path = source.join("file-history").join(sid).join(backup_name);
                    match fs::read(&blob_path) {
                        Ok(bytes) => match old_string {
                            Some(old) if !old.is_empty() => {
                                if bytes_contain(&bytes, old.as_bytes()) {
                                    resolved = Some(bytes);
                                } else {
                                    t15.rejected_unverifiable += 1;
                                }
                            }
                            _ => resolved = Some(bytes),
                        },
                        Err(_) => t15.blob_missing += 1,
                    }
                }
            }
            mark_edit(edits_since_backup);
            before_bytes = match resolved {
                Some(b) => Some(b),
                None => resolve_t2_before(
                    &abs,
                    line_ts,
                    old_string,
                    structured_patch,
                    prior_edits_this_path,
                    git_cache,
                    t2,
                ),
            };
            op = "modify".to_string();
        } else {
            mark_edit(edits_since_backup);
            before_bytes = resolve_t2_before(
                &abs,
                line_ts,
                old_string,
                structured_patch,
                prior_edits_this_path,
                git_cache,
                t2,
            );
            op = "modify".to_string();
        }

        // D1 (P2 fix round, founder decision 2): `content` is the tool's
        // OWN recorded post-edit bytes — real observed data. When it's
        // absent, `after` below is DERIVED by applying `oldString`→
        // `newString` to a `before` that may itself be approximate (a T2
        // git blob, or nothing at all) — this can disagree with the real
        // on-disk file for reasons that have nothing to do with a human or
        // external edit. Recorded so `undo`'s modified-since predicate and
        // `diff` can say so honestly instead of fabricating "human or
        // external edit" attribution.
        let after_will_be_synthesized = content_field.is_none();

        let after_bytes: Option<Vec<u8>> = if let Some(c) = content_field {
            Some(c.as_bytes().to_vec())
        } else if let (Some(old), Some(new)) = (old_string, new_string) {
            let base = original_file.map(str::to_string).or_else(|| {
                before_bytes
                    .as_ref()
                    .and_then(|b| String::from_utf8(b.clone()).ok())
            });
            base.map(|base_text| {
                let replace_all = tur
                    .get("replaceAll")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if replace_all {
                    base_text.replace(old, new)
                } else {
                    base_text.replacen(old, new, 1)
                }
                .into_bytes()
            })
        } else {
            None
        };

        let mut skipped = false;
        let mut skipped_reason: Option<String> = None;
        let mut before_ref: Option<String> = None;
        let mut after_ref: Option<String> = None;

        if let Some(b) = &before_bytes {
            match store.put_result(b) {
                PutResult::Stored { hash, .. } => before_ref = Some(hash),
                PutResult::OverCap => {
                    skipped = true;
                    skipped_reason = Some(skip_reason::OVER_CAP.to_string());
                }
                PutResult::IoError(_) => {
                    skipped = true;
                    skipped_reason = Some(skip_reason::IO_FAILED.to_string());
                }
            }
        }
        if !skipped {
            if let Some(a) = &after_bytes {
                match store.put_result(a) {
                    PutResult::Stored { hash, .. } => after_ref = Some(hash),
                    PutResult::OverCap => {
                        skipped = true;
                        skipped_reason = Some(skip_reason::OVER_CAP.to_string());
                    }
                    PutResult::IoError(_) => {
                        skipped = true;
                        skipped_reason = Some(skip_reason::IO_FAILED.to_string());
                    }
                }
            }
        }
        if skipped {
            before_ref = None;
            after_ref = None;
        }
        // Only meaningful when there's an `after` ref actually on the wire
        // to be misread as observed fact — moot (and left `None`, matching
        // the additive-field discipline) if `skipped` nulled it out, or if
        // no `after` bytes resolved at all.
        let after_synthesized = (after_will_be_synthesized && after_ref.is_some()).then_some(true);

        Some(FileEntry {
            path: rel,
            before: before_ref,
            after: after_ref,
            op,
            skipped,
            withheld: false,
            baseline_unknown: false,
            skipped_reason,
            after_synthesized,
            link_kind: None,
            attribution: None,
        })
    }

    /// Byte offset of the FIRST occurrence of `needle` in `haystack`, or
    /// `None` if absent. Companion to `bytes_count` (AC5b gate 2).
    fn bytes_find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() || needle.len() > haystack.len() {
            return None;
        }
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    /// Non-overlapping occurrence count of `needle` in `haystack`. AC5b gate
    /// 2 requires exactly 1.
    ///
    /// MEASURED REALITY (P2 fix round, real-corpus reproduction, n=228
    /// guard-admitted candidates): this uniqueness check is a **no-op** on
    /// this corpus — `uniq_pass = 228/228`, it never rejects anything,
    /// because Claude's Edit tool already requires `oldString` to be
    /// unique within the target file before it will apply the edit at all.
    /// An earlier version of this comment claimed uniqueness was
    /// "load-bearing... in a way it isn't" for T1.5 — that claim was never
    /// measured and was false. The `expected_old_start_line` check below
    /// (a coarse whole-prefix-equality proxy for "blob == originalFile",
    /// NOT a fabrication test by itself) is what actually does the work:
    /// on the same 228, it fails 204 of them. Kept anyway (uniqueness is
    /// cheap, and a corpus without the tool's own uniqueness guarantee —
    /// a different tool, a future Claude Code version — could still need
    /// it), but do not describe it as load-bearing without re-measuring.
    fn bytes_count(haystack: &[u8], needle: &[u8]) -> usize {
        if needle.is_empty() {
            return 0;
        }
        let mut count = 0;
        let mut start = 0;
        while start + needle.len() <= haystack.len() {
            if haystack[start..start + needle.len()] == *needle {
                count += 1;
                start += needle.len();
            } else {
                start += 1;
            }
        }
        count
    }

    /// T2 resolution (deferred from P1): the latest commit at-or-before the
    /// entry's OWN line timestamp (never the turn's or session's aggregate
    /// timestamp — a session can span hours, and resolving against a
    /// turn-level timestamp would land on a commit that postdates the
    /// specific edit, exactly the fabrication shape T1.5 already fell into
    /// once). Accepted only if `oldString` is present AND the resolved blob
    /// contains it — unlike T1.5, there is no staleness argument available
    /// here to grant an "unverified" tier, so a missing `oldString` refuses
    /// outright rather than trusting the blob on structure alone.
    #[allow(clippy::too_many_arguments)]
    fn resolve_t2_before(
        abs_file: &Path,
        line_ts: Option<&str>,
        old_string: Option<&str>,
        structured_patch: Option<&Value>,
        prior_edits_this_path: usize,
        git_cache: &mut GitTrackCache,
        t2: &mut T2Counters,
    ) -> Option<Vec<u8>> {
        // AC5b hardening, gate 1 (advisor-directed): if THIS session
        // already classified an earlier edit to this exact path, an
        // uncommitted intervening edit exists by construction — the latest
        // commit before this line's timestamp cannot be this edit's
        // pre-state, full stop, regardless of what content matching below
        // would say. Checked before any git process is even spawned.
        if prior_edits_this_path > 0 {
            t2.rejected_prior_edit_in_session += 1;
            return None;
        }
        let ts = line_ts?;
        // `abs_file` is already canonicalized (via `cwd_canon` in the
        // caller), so its parent is a real directory `git rev-parse` can
        // resolve without hitting the same symlink mismatch `classify_and_
        // resolve`'s doc comment explains.
        let dir = abs_file.parent()?;
        let repo_root = git_cache.repo_root(dir)?;
        let rel = abs_file.strip_prefix(&repo_root).ok()?;

        let before_arg = format!("--before={ts}");
        let log_out = Command::new("git")
            .arg("-C")
            .arg(&repo_root)
            .args(["log", &before_arg, "-1", "--format=%H", "--"])
            .arg(rel)
            .output()
            .ok()?;
        if !log_out.status.success() {
            t2.no_commit += 1;
            return None;
        }
        let hash = String::from_utf8_lossy(&log_out.stdout).trim().to_string();
        if hash.is_empty() {
            t2.no_commit += 1;
            return None;
        }

        let show_out = Command::new("git")
            .arg("-C")
            .arg(&repo_root)
            .arg("show")
            .arg(format!("{hash}:{}", rel.display()))
            .output()
            .ok()?;
        if !show_out.status.success() {
            t2.blob_missing += 1;
            return None;
        }
        let bytes = show_out.stdout;

        // Never accept a near-miss commit blob (P2.md prohibition): the
        // commit found above is the latest one AT OR BEFORE this edit's own
        // timestamp that touched this exact path, so it's temporally
        // correct by construction — but it can still be a pre-*commit*
        // state that predates THIS edit within an uncommitted working tree
        // (the T1.5 fabrication shape). Bare `oldString` containment ALONE
        // was measured (real-corpus AC5b oracle, before gate 1 existed) to
        // fabricate ~37% of the time (84/226) — a needle check proves the
        // needle is present, not that the rest of the haystack didn't
        // drift.
        //
        // FOUNDER DECISION 1 (P2 fix round, measured trade-off, real
        // corpus, n=1133 population this guard actually serves —
        // `originalFile` absent, not a `create`): gate 1 (the
        // prior-edit-in-session refusal above) ALONE gives 137 resolved /
        // 5.1% fabricated (7 wrong) / 130 correct. Adding gate 2 (below)
        // gives 17 resolved / 0% fabricated / 17 correct — it costs 113
        // correct resolutions (7.6x recall) to remove the last 7
        // fabrications. The founder ruled: keep both gates. Zero observed
        // fabrication is the only claim that survives a skeptic, and there
        // is no per-entry "unverified" marker (unlike T1.5's
        // `t15_unverified`) that would make shipping a small known-wrong
        // fraction acceptable here. **Do not re-tune this trade-off** and
        // do not attempt to recover the 113 — that decision is closed.
        //
        // What gate 2 actually rejects (measured, NOT "the fabrication-
        // prone cases" as an earlier version of this comment claimed): of
        // 144 resolutions that gate 1 alone got RIGHT, gate 2 refuses 127
        // of them (only 17 survive) — i.e. 60% of what gate 2 discards was
        // already correct. Gate 2 buys the last 7-fabrication reduction at
        // a steep recall cost; see `bytes_count`'s doc comment for exactly
        // which half of gate 2 (uniqueness vs. line-position) does that
        // work. Checked: exactly ONE occurrence of `oldString` in the
        // blob, AND, when the transcript's `structuredPatch` names the
        // hunk's starting line, that occurrence must land on that exact
        // line — a coarse whole-prefix-equality proxy, not a fabrication
        // test by itself (see `bytes_count`).
        match old_string {
            Some(old) if !old.is_empty() => {
                if bytes_count(&bytes, old.as_bytes()) != 1 {
                    t2.rejected_unverifiable += 1;
                    return None;
                }
                let offset = bytes_find(&bytes, old.as_bytes()).expect("count==1 implies a match");
                let line_matches = match expected_old_start_line(structured_patch) {
                    Some(expected) => {
                        let line_no = bytes[..offset].iter().filter(|&&b| b == b'\n').count() + 1;
                        line_no == expected
                    }
                    // No hunk-position info in this transcript entry —
                    // uniqueness above is the only signal available.
                    None => true,
                };
                if line_matches {
                    t2.resolved += 1;
                    Some(bytes)
                } else {
                    t2.rejected_unverifiable += 1;
                    None
                }
            }
            _ => {
                t2.no_oldstring += 1;
                None
            }
        }
    }

    /// `toolUseResult.structuredPatch[0].oldStart` — the 1-based line
    /// number the edit's hunk starts at in the PRE-edit file, when the
    /// transcript provides one. `None` if absent/unparseable (older
    /// transcript shapes, or a tool that doesn't emit one) — callers must
    /// treat that as "no positional signal available", never as a match.
    fn expected_old_start_line(structured_patch: Option<&Value>) -> Option<usize> {
        structured_patch?
            .as_array()?
            .first()?
            .get("oldStart")?
            .as_u64()
            .map(|n| n as usize)
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
    fn lexical_normalize_collapses_parent_dir_components() {
        assert_eq!(
            lexical_normalize(Path::new("/Users/x/../y")),
            PathBuf::from("/Users/y")
        );
        assert_eq!(
            lexical_normalize(Path::new("/a/b/../../c")),
            PathBuf::from("/c")
        );
        assert_eq!(
            lexical_normalize(Path::new("/home/dev/Projects/beta/../beta/src/composer.ts")),
            PathBuf::from("/home/dev/Projects/beta/src/composer.ts")
        );
    }

    #[test]
    fn lexical_normalize_drops_dot_components() {
        assert_eq!(
            lexical_normalize(Path::new("/a/./b/./c")),
            PathBuf::from("/a/b/c")
        );
    }

    #[test]
    fn lexical_normalize_does_not_escape_above_root() {
        // Guard the degenerate case (Fix 2): more `..`s than there are
        // components above them must not climb above `/`.
        assert_eq!(
            lexical_normalize(Path::new("/a/../../b")),
            PathBuf::from("/b")
        );
        assert_eq!(
            lexical_normalize(Path::new("/../../b")),
            PathBuf::from("/b")
        );
    }

    #[test]
    fn normalize_backup_key_resolves_relative_dotdot_key_against_cwd() {
        // Reproduces the real-corpus shape from Fix 2's brief: a
        // `trackedFileBackups` key that climbs one directory above `cwd`.
        let cwd = Path::new("/home/dev/Projects/alpha");
        assert_eq!(
            normalize_backup_key("../beta/src/composer.ts", cwd),
            "/home/dev/Projects/beta/src/composer.ts"
        );
    }

    #[test]
    fn record_backup_resets_staleness_only_on_a_genuinely_new_backup_name() {
        let mut backups = HashMap::new();
        let mut edits_since_backup = HashMap::new();
        let path = "/fake/redundant.txt".to_string();

        record_backup(
            &mut backups,
            &mut edits_since_backup,
            path.clone(),
            "v1".to_string(),
        );
        assert_eq!(edits_since_backup.get(&path), Some(&0));

        // Simulate one edit having happened since.
        *edits_since_backup.get_mut(&path).unwrap() += 1;
        assert_eq!(edits_since_backup.get(&path), Some(&1));

        // A re-announcement of the SAME backup name (the real-corpus
        // manifest-re-list shape) must NOT reset the staleness count.
        record_backup(
            &mut backups,
            &mut edits_since_backup,
            path.clone(),
            "v1".to_string(),
        );
        assert_eq!(
            edits_since_backup.get(&path),
            Some(&1),
            "re-announcing the same backup name must not clear staleness"
        );

        // A genuinely different backup name DOES reset it.
        record_backup(
            &mut backups,
            &mut edits_since_backup,
            path.clone(),
            "v2".to_string(),
        );
        assert_eq!(
            edits_since_backup.get(&path),
            Some(&0),
            "a real new backup version must reset staleness"
        );
    }

    #[test]
    fn is_safe_path_component_rejects_hostile_values() {
        assert!(!is_safe_path_component(""));
        assert!(!is_safe_path_component(".."));
        assert!(!is_safe_path_component("."));
        assert!(!is_safe_path_component("../../etc/passwd"));
        assert!(!is_safe_path_component("/etc/passwd"));
        assert!(!is_safe_path_component("a/b"));
        assert!(!is_safe_path_component("a\\b"));
    }

    #[test]
    fn is_safe_path_component_accepts_the_real_confirmed_shape() {
        assert!(is_safe_path_component("bb6db0439755b9fa@v1"));
        assert!(is_safe_path_component(
            "a1111111-1111-4111-8111-111111111111"
        ));
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
