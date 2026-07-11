//! `diff`: unified diff of one turn's file changes (AC F1–F4). `undo`: revert
//! a turn's changes, per file (AC H1–H7, Z+1). Both are read-mostly — `diff`
//! and undo's preview never need the daemon running; `undo --confirm` writes
//! to the worktree but not to the daemon's internal state.

use crate::state::State;
use crate::{fmt, log_path, objects_dir, undo_guard_path, UndoGuard};
use agentrec_core::diff;
use agentrec_core::record::{FileEntry, LogRecord, TurnRecord};
use agentrec_core::store::{hash_bytes, BlobStore, StoreError};
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

/// `diff <turn>`: resolve `turn` (full id or unambiguous prefix, K+) and print
/// a unified diff per changed file. Binary files get a byte-count summary
/// instead of a textual diff (F2); skipped/withheld files get a notice (F3)
/// instead of content that was never snapshotted.
pub fn diff(root: &Path, turn_ref: &str) -> Result<(), String> {
    let records = agentrec_core::record::load_log(&log_path(root));
    let turns: Vec<&TurnRecord> = records
        .iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        })
        .collect();

    let turn = resolve_turn(&turns, turn_ref)?;

    let store = BlobStore::new(objects_dir(root));
    println!("{}", header_line(turn));
    for entry in &turn.files {
        print_entry(&store, entry);
    }
    Ok(())
}

/// `show <turn> [--prompt]` (D-PD2, SPEC §Prompt posture item 4): bare form
/// prints only the turn header, via the same renderer `blame` uses — that
/// renders `prompt_excerpt` (already post-scrub AND length-capped), never the
/// full prompt. `--prompt` is the one explicit path to the full post-scrub
/// prompt text, loaded through the integrity-checked blob store; prompt blobs
/// are post-scrub at rest, so printing the full text here is safe.
pub fn show(root: &Path, turn_ref: &str, prompt: bool) -> Result<(), String> {
    let records = agentrec_core::record::load_log(&log_path(root));
    let turns: Vec<&TurnRecord> = records
        .iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        })
        .collect();

    let turn = resolve_turn(&turns, turn_ref)?;

    if !prompt {
        println!("{}", render_turn(turn));
        return Ok(());
    }

    let Some(prompt_ref) = turn.prompt_ref.as_deref() else {
        return Err("no prompt attached — this turn has no recorded prompt".to_string());
    };

    let store = BlobStore::new(objects_dir(root));
    let bytes = store.get(prompt_ref).map_err(|e| match e {
        // Genuinely gone (retention purge or manual deletion) — the only
        // case that's actually "TTL expired".
        StoreError::Missing(_) => "prompt purged — TTL expired".to_string(),
        // Present but hash-mismatched: a distinct, honest cause — never
        // fabricate "TTL expired" for a blob that's still on disk.
        StoreError::Corrupt(_) => "prompt blob corrupt (hash mismatch)".to_string(),
    })?;
    std::io::stdout()
        .write_all(&bytes)
        .map_err(|e| format!("failed to write prompt to stdout: {e}"))?;
    Ok(())
}

/// Match `turn_ref` against recorded turn ids, exact or unambiguous prefix
/// (K+). Zero or multiple matches is an error naming the valid id range so
/// the caller can retry (F4).
fn resolve_turn<'a>(turns: &[&'a TurnRecord], turn_ref: &str) -> Result<&'a TurnRecord, String> {
    if turns.is_empty() {
        return Err("no turns recorded — is `agentrec record` running?".to_string());
    }
    let matches: Vec<&&TurnRecord> = turns
        .iter()
        .filter(|t| t.id == turn_ref || t.id.starts_with(turn_ref))
        .collect();
    match matches.len() {
        1 => Ok(*matches[0]),
        0 => Err(format!(
            "unknown turn id '{turn_ref}' — recorded turns: {}",
            turn_range(turns)
        )),
        n => Err(format!(
            "ambiguous turn id '{turn_ref}' — matches {n} turns; recorded turns: {}",
            turn_range(turns)
        )),
    }
}

/// `<oldest_short>..<newest_short> (N turns)` — turns are append order
/// (oldest first) in the log.
fn turn_range(turns: &[&TurnRecord]) -> String {
    let (Some(first), Some(last)) = (turns.first(), turns.last()) else {
        return "(no turns)".to_string();
    };
    format!(
        "{}..{} ({} turns)",
        short_id(&first.id),
        short_id(&last.id),
        turns.len()
    )
}

fn header_line(t: &TurnRecord) -> String {
    let id = short_id(&t.id);
    let tool = t.tool.as_deref().unwrap_or("—");
    let n = t.files.len();
    let files = if n == 1 {
        "1 file".to_string()
    } else {
        format!("{n} files")
    };
    format!("turn {id}  {tool}  {files}")
}

fn print_entry(store: &BlobStore, entry: &FileEntry) {
    if entry.withheld {
        println!(
            "  {}: (withheld — secret-pattern file, not snapshotted)",
            entry.path
        );
        return;
    }
    if entry.skipped {
        println!(
            "  {}: (content not snapshotted — over size cap)",
            entry.path
        );
        return;
    }

    let before = match load_blob(store, entry.before.as_deref()) {
        Ok(bytes) => bytes,
        Err(()) => {
            println!(
                "  {}: (snapshot unavailable — purged or missing)",
                entry.path
            );
            return;
        }
    };
    let after = match load_blob(store, entry.after.as_deref()) {
        Ok(bytes) => bytes,
        Err(()) => {
            println!(
                "  {}: (snapshot unavailable — purged or missing)",
                entry.path
            );
            return;
        }
    };

    if diff::is_binary(&before) || diff::is_binary(&after) {
        println!(
            "  {}: binary file changed ({} → {} bytes)",
            entry.path,
            before.len(),
            after.len()
        );
        return;
    }

    // Already checked valid UTF-8 by is_binary above.
    let before_str = std::str::from_utf8(&before).unwrap_or("");
    let after_str = std::str::from_utf8(&after).unwrap_or("");

    if entry.baseline_unknown && entry.op == "modify" && entry.before.is_none() {
        println!(
            "  {}: (baseline unknown — first seen mid-session; showing new content)",
            entry.path
        );
        print!("{}", diff::unified("", after_str, &entry.path));
        return;
    }

    let text = match entry.op.as_str() {
        "create" => diff::unified("", after_str, &entry.path),
        "delete" => diff::unified(before_str, "", &entry.path),
        _ => diff::unified(before_str, after_str, &entry.path),
    };
    print!("{text}");
}

/// Load a blob by its optional hash ref; `None` (e.g. a create's `before`)
/// yields empty content, not an error. A present hash that the store can't
/// serve (purged or corrupt) is the only error case.
fn load_blob(store: &BlobStore, hash: Option<&str>) -> Result<Vec<u8>, ()> {
    match hash {
        None => Ok(Vec::new()),
        Some(h) => store.get(h).map_err(|_| ()),
    }
}

/// `t_<ULID>` → keep the prefix + last 4 chars for readability (mirrors
/// `cmds::short_id`; kept separate since that one is private to its module).
fn short_id(id: &str) -> String {
    let body = id.strip_prefix("t_").unwrap_or(id);
    if body.len() <= 8 {
        return id.to_string();
    }
    format!("t_{}…{}", &body[..4], &body[body.len() - 4..])
}

/// "2026-07-05T14:03:11.000Z" → "14:03" (mirrors `cmds::hhmm`).
fn hhmm(rfc: &str) -> String {
    rfc.split('T')
        .nth(1)
        .and_then(|t| t.get(..5))
        .unwrap_or("--:--")
        .to_string()
}

/// `blame <file>` or `blame <file>:<line>` (PROTOCOL §5, AC G1–G6): which
/// turn last touched a file, or introduced one of its current lines. Read-only
/// — never needs the daemon running.
pub fn blame(root: &Path, target: &str) -> Result<(), String> {
    let (file, line_no) = parse_target(target);

    let records = agentrec_core::record::load_log(&log_path(root));
    // Global fallback for the "no touching turn at all" case, where there is
    // no turn to bound an interval-aware check against (unchanged semantics —
    // AC blame_untouched_file_exit0 depends on a balanced start/stop NOT
    // counting as a gap here).
    let no_turn_gap = has_recording_gap(&records);

    let turns: Vec<&TurnRecord> = records
        .iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        })
        .collect();

    // Append order (oldest first) in the log — preserved here.
    let touching: Vec<&TurnRecord> = turns
        .into_iter()
        .filter(|t| t.files.iter().any(|f| f.path == file))
        .collect();

    let disk_bytes = std::fs::read(root.join(&file)).ok();
    let current_hash = disk_bytes.as_deref().map(hash_bytes);

    match line_no {
        None => blame_file(&touching, &records, &file, &current_hash, no_turn_gap),
        Some(n) => {
            let store = BlobStore::new(objects_dir(root));
            blame_line(
                &store,
                &touching,
                &records,
                &file,
                n,
                &current_hash,
                disk_bytes.as_deref(),
                no_turn_gap,
            )
        }
    }
}

/// Splits `<file>` from an optional trailing `:<line>` — only when the
/// suffix after the LAST `:` parses as a positive integer, so paths with
/// colons elsewhere (not a POSIX concern here, but defensive) fall through
/// to the whole-file form.
fn parse_target(target: &str) -> (String, Option<usize>) {
    if let Some(idx) = target.rfind(':') {
        let (file, rest) = target.split_at(idx);
        if let Ok(line) = rest[1..].parse::<usize>() {
            if line > 0 {
                return (file.to_string(), Some(line));
            }
        }
    }
    (target.to_string(), None)
}

/// A recording gap is any `start` epoch that follows a prior `start` with no
/// intervening `stop` (mirrors `cmds::count_gaps`, kept boolean here since
/// blame only needs "does any gap exist").
fn has_recording_gap(records: &[LogRecord]) -> bool {
    let mut open = false;
    for r in records {
        if let LogRecord::Epoch(e) = r {
            match e.event.as_str() {
                "start" => {
                    if open {
                        return true;
                    }
                    open = true;
                }
                "stop" => open = false,
                _ => {}
            }
        }
    }
    false
}

/// True when the file's on-disk content differs from `turn`'s recorded
/// `after` for it (PROTOCOL §5 `modified-since`). Covers "went missing when
/// it shouldn't have" and "reappeared after a delete" the same way, since
/// both sides collapse to `None` when absent.
fn modified_since(turn: &TurnRecord, file: &str, current_hash: &Option<String>) -> bool {
    let entry_after = turn
        .files
        .iter()
        .find(|f| f.path == file)
        .and_then(|f| f.after.clone());
    current_hash.as_deref() != entry_after.as_deref()
}

/// `<short-id> · <tool> · "<prompt>" · <hh:mm>` for a rich turn, or
/// `<short-id> · bare turn · <hh:mm>` for a bare one — a bare turn never
/// fabricates a tool or prompt (AC G3).
fn render_turn(t: &TurnRecord) -> String {
    let id = short_id(&t.id);
    let when = hhmm(&t.started);
    if t.grade == "rich" {
        let tool = t.tool.as_deref().unwrap_or("—");
        let prompt = t
            .prompt_excerpt
            .as_deref()
            .map(fmt::sanitize_terminal)
            .unwrap_or_else(|| "—".to_string());
        format!("{id} · {tool} · \"{prompt}\" · {when}")
    } else {
        format!("{id} · bare turn · {when}")
    }
}

/// File-level blame (AC G1–G4, G6): report the last turn to touch `file`,
/// honestly noting deletion, human edits since, or a recording gap that
/// makes the answer uncertain.
fn blame_file(
    touching: &[&TurnRecord],
    records: &[LogRecord],
    file: &str,
    current_hash: &Option<String>,
    no_turn_gap: bool,
) -> Result<(), String> {
    let Some(t) = touching.last() else {
        if no_turn_gap {
            // A gap may hide the turn that actually touched this file.
            println!("attribution stale — recording gap");
        } else {
            println!("no recorded turn touches {file}");
        }
        return Ok(());
    };

    let deleted = t
        .files
        .iter()
        .find(|f| f.path == file)
        .map(|f| f.op.as_str())
        == Some("delete");
    let modified = modified_since(t, file, current_hash);
    // E2: interval-aware — a gap only makes THIS turn's attribution stale
    // when it occurs after the turn ended (a gap entirely before it is
    // irrelevant to whether the current on-disk state is explained).
    let gap_stale = has_gap_after(records, &t.ended) && modified;

    let mut line = render_turn(t);
    if deleted {
        line.push_str(" · deleted this file");
    }
    if gap_stale {
        line.push_str(" · attribution stale — recording gap");
    } else if modified {
        line.push_str(" · human-edited since");
    }
    println!("{line}");
    Ok(())
}

/// Line-level blame (AC G5, plus G6/G4 honesty carried through): which turn
/// introduced the current text of `file:line_no`, walking touching turns
/// oldest→newest so the LAST turn to introduce that exact line text wins.
///
/// Known v1 limitation: matching is by exact line text, not a tracked
/// identity — a line duplicated verbatim elsewhere in the file cannot be
/// told apart from its duplicate by this heuristic.
#[allow(clippy::too_many_arguments)]
fn blame_line(
    store: &BlobStore,
    touching: &[&TurnRecord],
    records: &[LogRecord],
    file: &str,
    line_no: usize,
    current_hash: &Option<String>,
    disk_bytes: Option<&[u8]>,
    no_turn_gap: bool,
) -> Result<(), String> {
    let last = touching.last().copied();

    // E2: interval-aware, bounded to the last touching turn's end — mirrors
    // blame_file's gap_stale check.
    let gap_stale = match last {
        Some(t) => has_gap_after(records, &t.ended) && modified_since(t, file, current_hash),
        None => no_turn_gap,
    };
    if gap_stale {
        println!("attribution stale — recording gap");
        return Ok(());
    }

    // A deleted-or-absent file resolves any line query to the last turn that
    // touched it — there is no current line N to walk toward otherwise.
    if let Some(t) = last {
        let deleted = t
            .files
            .iter()
            .find(|f| f.path == file)
            .map(|f| f.op.as_str())
            == Some("delete");
        if deleted || disk_bytes.is_none() {
            let mut line = render_turn(t);
            if deleted {
                line.push_str(" · deleted this file");
            }
            println!("{file}:{line_no}: {line}");
            return Ok(());
        }
    }

    let Some(bytes) = disk_bytes else {
        return Err(format!("{file}: not found"));
    };
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    if line_no == 0 || line_no > lines.len() {
        return Err(format!("{file} has only {} line(s)", lines.len()));
    }
    let target_text = lines[line_no - 1];

    let mut responsible: Option<&TurnRecord> = None;
    for t in touching {
        let Some(entry) = t.files.iter().find(|f| f.path == file) else {
            continue;
        };
        let before = load_text(store, entry.before.as_deref());
        let after = load_text(store, entry.after.as_deref());
        let intro = diff::added_or_changed_lines(&before, &after);
        if intro.iter().any(|l| l == target_text) {
            responsible = Some(t);
        }
    }

    match responsible {
        Some(t) => println!("{file}:{line_no}: {}", render_turn(t)),
        // E3: no turn's recorded diff introduces this exact line text. That
        // is only honestly "before recording began" when the whole history
        // is actually gap-free — a line that was silently added during an
        // uncovered interval (then folded into a later turn's unchanged
        // `before`) would otherwise be misreported as predating all
        // recording, when really its origin is just unknown.
        None if has_gap_after(records, "") => {
            println!("{file}:{line_no}: attribution stale — recording gap");
        }
        None => println!("{file}:{line_no}: before recording began"),
    }
    Ok(())
}

/// Loads blob text by optional hash, empty on any read error (purged,
/// corrupt) or absent hash — line-diffing degrades to "no lines introduced"
/// rather than failing the whole blame.
fn load_text(store: &BlobStore, hash: Option<&str>) -> String {
    match hash {
        None => String::new(),
        Some(h) => match store.get(h) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => String::new(),
        },
    }
}

// ---- undo (AC H1–H7, Z+1/D42) ------------------------------------------------

/// `undo [<turn>] [--confirm] [--allow-modified] [--files a,b]`: revert a
/// turn's file changes. Always previews first; only `--confirm` mutates the
/// worktree. `turn_ref` omitted selects panic mode: the most recent non-git
/// turn, which must be `rich` (a trailing unattributed window blocks it
/// rather than being silently skipped past — Z+1/D42).
pub fn undo(
    root: &Path,
    turn_ref: Option<&str>,
    confirm: bool,
    allow_modified: bool,
    files: &[String],
) -> Result<(), String> {
    let records = agentrec_core::record::load_log(&log_path(root));
    let superseded = crate::cmds::merged_ids(&records);
    let turns: Vec<&TurnRecord> = records
        .iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        })
        .collect();

    let target = match turn_ref {
        Some(r) => resolve_turn(&turns, r)?,
        None => resolve_panic_target(&turns, &superseded)?,
    };
    // `turns` borrows from `target`'s own source, so this position lookup
    // always succeeds; the fallback is only a defensive belt-and-braces.
    let target_idx = turns.iter().position(|t| t.id == target.id).unwrap_or(0);

    // E6: `--files` naming a path not in this turn is a user error, not a
    // silent no-op — refuse (no preview, no mutation) and name the typo(s)
    // rather than quietly printing "nothing to revert".
    if !files.is_empty() {
        let target_paths: HashSet<&str> = target.files.iter().map(|f| f.path.as_str()).collect();
        let unmatched: Vec<&str> = files
            .iter()
            .map(|s| s.as_str())
            .filter(|p| !target_paths.contains(p))
            .collect();
        if !unmatched.is_empty() {
            return Err(format!(
                "--files names path(s) not in turn {}: {}",
                short_id(&target.id),
                unmatched.join(", ")
            ));
        }
    }

    let store = BlobStore::new(objects_dir(root));
    let state = crate::state::read_state(root);
    let plans = build_plan(
        root,
        &store,
        &state,
        target,
        target_idx,
        &turns,
        &records,
        files,
        allow_modified,
    );

    print_plan(target, &plans);

    if !confirm {
        println!("preview only — re-run with --confirm to apply");
        return Ok(());
    }

    let revertible: Vec<&Plan> = plans
        .iter()
        .filter(|p| matches!(p.kind, PlanKind::Revert { .. }))
        .collect();
    if revertible.is_empty() {
        println!("nothing to revert");
        return Ok(());
    }

    // E8: refuse to START a concurrent undo while another one's guard is
    // still live — two undos racing on the same paths would otherwise
    // clobber each other's guard and mint a bogus bare turn for the loser's
    // in-flight writes. An expired (or absent/malformed) guard is not live.
    if let Some(reason) = live_undo_guard_reason(root) {
        return Err(reason);
    }

    let guarded_paths: Vec<String> = revertible.iter().map(|p| p.entry.path.clone()).collect();
    write_undo_guard(root, &guarded_paths)?;

    let short_target = short_id(&target.id);
    let mut inverse_entries = Vec::with_capacity(revertible.len());
    let mut mutation_err: Option<String> = None;
    for plan in &revertible {
        match execute_revert(root, &store, &plan.entry) {
            Ok(inverse) => inverse_entries.push(inverse),
            Err(e) => {
                mutation_err = Some(e);
                break;
            }
        }
    }

    if let Some(e) = mutation_err {
        // E1 belt-and-braces: any entries already reverted before the
        // failure are real writes — they must not go unrecorded (invisible
        // mutations), so append a partial, honestly-truncated undo turn
        // covering them before surfacing the error.
        if !inverse_entries.is_empty() {
            let now = wall_now_ms();
            let partial = TurnRecord {
                v: 1,
                id: agentrec_core::id::turn_id(),
                grade: "rich".to_string(),
                truncated: true,
                started: agentrec_core::time::rfc3339(now),
                ended: agentrec_core::time::rfc3339(now),
                tool: Some("agentrec".to_string()),
                model: None,
                session: None,
                root: root.to_string_lossy().to_string(),
                prompt_ref: None,
                prompt_excerpt: Some(format!(
                    "undo of {short_target} (partial — aborted mid-revert)"
                )),
                merges: vec![],
                files: inverse_entries,
            };
            let _ = agentrec_core::record::append_log(&log_path(root), &LogRecord::Turn(partial));
        }
        // Any entries already reverted are real writes a concurrent daemon
        // must still not misattribute, so this waits out the same linger as
        // the success path before cleaning up.
        finish_undo_guard(root);
        return Err(e);
    }

    let now = wall_now_ms();
    let undo_record = TurnRecord {
        v: 1,
        id: agentrec_core::id::turn_id(),
        grade: "rich".to_string(),
        truncated: false,
        started: agentrec_core::time::rfc3339(now),
        ended: agentrec_core::time::rfc3339(now),
        tool: Some("agentrec".to_string()),
        model: None,
        session: None,
        root: root.to_string_lossy().to_string(),
        prompt_ref: None,
        prompt_excerpt: Some(format!("undo of {short_target}")),
        merges: vec![],
        files: inverse_entries,
    };
    let reverted_n = undo_record.files.len();
    let new_short_id = short_id(&undo_record.id);
    agentrec_core::record::append_log(&log_path(root), &LogRecord::Turn(undo_record))?;

    finish_undo_guard(root);

    println!("reverted {reverted_n} file(s); recorded as turn {new_short_id}");
    Ok(())
}

/// Panic-mode target resolution (Z+1/D42): the most recent turn, excluding
/// git turns and ids superseded by a retroactive merge. A bare turn in that
/// position REFUSES rather than falling through to an older rich turn — a
/// trailing unattributed window must block panic undo, not be skipped.
fn resolve_panic_target<'a>(
    turns: &[&'a TurnRecord],
    superseded: &HashSet<String>,
) -> Result<&'a TurnRecord, String> {
    let newest = turns
        .iter()
        .rev()
        .find(|t| t.tool.as_deref() != Some("git") && !superseded.contains(&t.id));
    let Some(newest) = newest else {
        return Err("no turns to undo".to_string());
    };
    if newest.grade != "rich" {
        return Err(
            "last activity window is unattributed — pick a turn from `agentrec log`".to_string(),
        );
    }
    Ok(*newest)
}

/// Per-file disposition for an undo.
struct Plan {
    entry: FileEntry,
    kind: PlanKind,
}

enum PlanKind {
    /// Will be reverted; `warn` names the modified-since cause when included
    /// only because of `--allow-modified`.
    Revert { warn: Option<String> },
    /// Modified since the turn; skipped unless `--allow-modified`.
    Excluded { cause: String },
    /// Never revertible regardless of flags (no content, or none was ever
    /// snapshotted).
    Refused { reason: String },
}

/// Classify every file in `target` (filtered by `files_filter`, when
/// non-empty) into revert / exclude / refuse. Order matches `target.files`.
#[allow(clippy::too_many_arguments)]
fn build_plan(
    root: &Path,
    store: &BlobStore,
    state: &State,
    target: &TurnRecord,
    target_idx: usize,
    turns: &[&TurnRecord],
    records: &[LogRecord],
    files_filter: &[String],
    allow_modified: bool,
) -> Vec<Plan> {
    let filter_set: Option<HashSet<&str>> = if files_filter.is_empty() {
        None
    } else {
        Some(files_filter.iter().map(|s| s.as_str()).collect())
    };

    let mut plans = Vec::with_capacity(target.files.len());
    for entry in &target.files {
        if let Some(set) = &filter_set {
            if !set.contains(entry.path.as_str()) {
                continue; // deselected by --files, left untouched (AC H2)
            }
        }

        if entry.withheld {
            plans.push(Plan {
                entry: entry.clone(),
                kind: PlanKind::Refused {
                    reason: "secret-pattern file, never snapshotted".to_string(),
                },
            });
            continue;
        }
        if entry.skipped {
            let reason = if state.io_failed.iter().any(|p| p == &entry.path) {
                "no snapshot exists (write failed at record time)".to_string()
            } else {
                "content not snapshotted (over size cap)".to_string()
            };
            plans.push(Plan {
                entry: entry.clone(),
                kind: PlanKind::Refused { reason },
            });
            continue;
        }
        if entry.op == "modify" || entry.op == "delete" {
            // E1: an integrity READ (store.get), not a bare existence check —
            // build_plan runs entirely before any file mutation, so a corrupt
            // (hash-mismatched) before-blob is caught and refused here, never
            // discovered mid-revert after other files have already changed.
            let refuse_reason = match entry.before.as_deref() {
                None => Some("no prior snapshot to restore".to_string()),
                Some(h) => match store.get(h) {
                    Ok(_) => None,
                    Err(StoreError::Missing(_)) => Some("no prior snapshot to restore".to_string()),
                    Err(StoreError::Corrupt(_)) => Some(
                        "prior snapshot corrupt (hash mismatch) — refusing to restore".to_string(),
                    ),
                },
            };
            if let Some(reason) = refuse_reason {
                plans.push(Plan {
                    entry: entry.clone(),
                    kind: PlanKind::Refused { reason },
                });
                continue;
            }
        }

        // modified-since (PROTOCOL §5): current on-disk hash vs. the turn's
        // recorded `after` for this path. `None` on either side means absent.
        let current = read_current_hash(root, &entry.path);
        let is_modified = current.as_deref() != entry.after.as_deref();

        if is_modified && !allow_modified {
            let cause = modified_cause(target_idx, turns, records, target, &entry.path);
            plans.push(Plan {
                entry: entry.clone(),
                kind: PlanKind::Excluded { cause },
            });
            continue;
        }

        let warn =
            is_modified.then(|| modified_cause(target_idx, turns, records, target, &entry.path));
        plans.push(Plan {
            entry: entry.clone(),
            kind: PlanKind::Revert { warn },
        });
    }
    plans
}

/// On-disk content hash for `rel`, relative to `root`; `None` for an absent
/// file OR any read error — undo's safety gate treats both as "no content to
/// compare", which only ever makes the modified-since check MORE cautious
/// (a spurious `None` looks like a legitimate delete-target, not a bypass).
fn read_current_hash(root: &Path, rel: &str) -> Option<String> {
    std::fs::read(root.join(rel)).ok().map(|b| hash_bytes(&b))
}

/// Best-effort explanation for why a path is modified-since the target turn:
/// a later rich turn touching the same path outranks an uncovered recording
/// gap, which outranks the default "some edit we can't otherwise explain".
fn modified_cause(
    target_idx: usize,
    turns: &[&TurnRecord],
    records: &[LogRecord],
    target: &TurnRecord,
    path: &str,
) -> String {
    let later_touches = turns[target_idx + 1..]
        .iter()
        .any(|t| t.grade == "rich" && t.files.iter().any(|f| f.path == path));
    if later_touches {
        return "later agent turn".to_string();
    }
    if has_gap_after(records, &target.ended) {
        return "recording gap".to_string();
    }
    "human or external edit".to_string()
}

/// True when the daemon was NOT recording for some interval that falls after
/// `since` (RFC 3339 strings compare lexically in time order at fixed
/// width). Three shapes, all uncovered intervals (E2):
///   - crash-shaped: an unbalanced `start` (no intervening `stop`) — the
///     interval from the first `start` to the second is unaccounted for.
///   - clean restart: a `stop` followed later by a `start` — the daemon was
///     deliberately off for that interval, however short.
///   - trailing stop: the last epoch is a `stop` with nothing after it — the
///     daemon is (or was, as of the log) simply not running.
///
/// Only the *start* of the uncovered interval needs to be after `since` —
/// once recording has stopped, everything from there on is uncovered.
fn has_gap_after(records: &[LogRecord], since: &str) -> bool {
    let mut open = false;
    let mut pending_stop: Option<&str> = None;
    for r in records {
        if let LogRecord::Epoch(e) = r {
            match e.event.as_str() {
                "start" => {
                    if open && e.ts.as_str() > since {
                        return true; // crash-shaped
                    }
                    if pending_stop.is_some() && e.ts.as_str() > since {
                        return true; // clean restart: stop -> (later) start
                    }
                    open = true;
                    pending_stop = None;
                }
                "stop" => {
                    open = false;
                    pending_stop = Some(e.ts.as_str());
                }
                _ => {}
            }
        }
    }
    if let Some(stop_ts) = pending_stop {
        if stop_ts > since {
            return true; // trailing stop: daemon currently not running
        }
    }
    false
}

fn print_plan(target: &TurnRecord, plans: &[Plan]) {
    println!(
        "undo {} ({})",
        short_id(&target.id),
        target.tool.as_deref().unwrap_or("—")
    );
    for p in plans {
        match &p.kind {
            PlanKind::Revert { warn } => {
                println!("  revert  {} ({})", p.entry.path, p.entry.op);
                if let Some(cause) = warn {
                    println!(
                        "    WARNING: {} modified since ({cause}) — reverting anyway (--allow-modified)",
                        p.entry.path
                    );
                }
            }
            PlanKind::Excluded { cause } => {
                println!(
                    "  EXCLUDE {} — modified since ({cause}); --allow-modified to include",
                    p.entry.path
                );
            }
            PlanKind::Refused { reason } => {
                println!("  REFUSE  {} — {reason}", p.entry.path);
            }
        }
    }
}

/// Apply one file's revert and return the inverse `FileEntry` for the new
/// undo turn. Snapshots the CURRENT (pre-undo) bytes first — that becomes the
/// inverse entry's `before`, so the undo is itself re-revertible (AC H6).
/// Every write is verified by re-reading and re-hashing before returning Ok;
/// a mismatch is a hard error, never a silent partial revert.
fn execute_revert(root: &Path, store: &BlobStore, entry: &FileEntry) -> Result<FileEntry, String> {
    let path = root.join(&entry.path);
    let pre_bytes = match std::fs::read(&path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: cannot read before revert: {e}", entry.path)),
    };
    let new_before = match &pre_bytes {
        Some(b) => Some(store.put(b).ok_or_else(|| {
            format!(
                "{}: failed to snapshot current content before revert",
                entry.path
            )
        })?),
        None => None,
    };

    let (new_after, inverse_op) = match entry.op.as_str() {
        "create" => {
            // E1: idempotent — a file already absent (deleted by something
            // else since the turn) means the goal state ("file gone") is
            // already reached; NotFound is success, not an error.
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: failed to delete: {e}", entry.path)),
            }
            if path.exists() {
                return Err(format!(
                    "{}: still present after delete (revert of create)",
                    entry.path
                ));
            }
            (None, "delete")
        }
        "delete" => {
            let restored = restore_from_before(&path, store, entry)?;
            (Some(restored), "create")
        }
        _ => {
            // "modify"
            let restored = restore_from_before(&path, store, entry)?;
            (Some(restored), "modify")
        }
    };

    Ok(FileEntry {
        path: entry.path.clone(),
        before: new_before,
        after: new_after,
        op: inverse_op.to_string(),
        skipped: false,
        withheld: false,
        baseline_unknown: false,
    })
}

/// Write `entry.before`'s blob to `path` (creating parent dirs), then verify
/// by re-reading and re-hashing. Returns the (already-known) `before` hash on
/// success — the content is byte-identical by construction, verified.
fn restore_from_before(
    path: &Path,
    store: &BlobStore,
    entry: &FileEntry,
) -> Result<String, String> {
    let before_hash = entry
        .before
        .as_deref()
        .ok_or_else(|| format!("{}: no prior snapshot to restore", entry.path))?;
    let bytes = store
        .get(before_hash)
        .map_err(|e| format!("{}: {e}", entry.path))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("{}: failed to create parent dirs: {e}", entry.path))?;
    }
    std::fs::write(path, &bytes).map_err(|e| format!("{}: failed to write: {e}", entry.path))?;
    let readback = std::fs::read(path)
        .map_err(|e| format!("{}: failed to verify after write: {e}", entry.path))?;
    if hash_bytes(&readback) != before_hash {
        return Err(format!(
            "{}: verification failed after restore (byte mismatch)",
            entry.path
        ));
    }
    Ok(before_hash.to_string())
}

/// E8: `Some(reason)` when an unexpired H7 coordination guard already exists
/// — a second concurrent `undo --confirm` must refuse rather than clobber
/// it (clobbering would let the first undo's in-flight writes be mistaken
/// for a bare turn, and the two guards would delete each other on cleanup).
/// An absent, malformed, or expired guard is not live: `None`.
fn live_undo_guard_reason(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(undo_guard_path(root)).ok()?;
    let guard: UndoGuard = serde_json::from_str(&text).ok()?;
    if guard.until_ms > wall_now_ms() {
        Some(format!(
            "another undo is already in progress ({} file(s) guarded) — refusing to start a concurrent undo; retry once it finishes",
            guard.paths.len()
        ))
    } else {
        None
    }
}

/// Write the H7 coordination guard before any file mutation begins.
fn write_undo_guard(root: &Path, paths: &[String]) -> Result<(), String> {
    let guard = UndoGuard {
        paths: paths.to_vec(),
        until_ms: wall_now_ms() + 30_000,
    };
    let text = serde_json::to_string(&guard).map_err(|e| e.to_string())?;
    let path = undo_guard_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    agentrec_core::perms::lock_file(&path); // D37, umask-independent
    Ok(())
}

/// How long a completed undo keeps its guard alive before removing it: long
/// enough to outlast a concurrent daemon's worst-case flush latency (its
/// debounce window plus notify's own event-delivery lag, observed up to
/// ~1s on macOS FSEvents — see the kill9 tests below), so a running
/// `agentrec record` reliably filters undo's own writes out of its next
/// flush instead of mistaking them for a bare turn (AC H7). `until_ms`
/// (30s) is the separate backstop for a crash that skips this removal
/// entirely.
const GUARD_LINGER: std::time::Duration = std::time::Duration::from_millis(3_000);

/// Wait out [`GUARD_LINGER`], then best-effort remove the guard — the
/// `until_ms` deadline written alongside it is the backstop if this doesn't
/// run at all (e.g. the process is killed mid-revert).
fn finish_undo_guard(root: &Path) {
    std::thread::sleep(GUARD_LINGER);
    let _ = std::fs::remove_file(undo_guard_path(root));
}

/// Mirrors `cmds::wall_now_ms` (kept local — a one-line helper, not worth a
/// shared module for).
fn wall_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
