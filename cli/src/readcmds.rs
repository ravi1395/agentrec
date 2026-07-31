//! `diff`: unified diff of one turn's file changes (AC F1–F4). `undo`: revert
//! a turn's changes, per file (AC H1–H7, Z+1). Both are read-mostly — `diff`
//! and undo's preview never need the daemon running; `undo --confirm` writes
//! to the worktree but not to the daemon's internal state.

use crate::cmds::wall_now_ms;
use crate::{fmt, log_path, objects_dir, undo_guard_path, UndoGuard};
use agentrec_core::diff;
use agentrec_core::record::{FileEntry, LogRecord, TurnRecord};
use agentrec_core::store::{hash_bytes, BlobStore, StoreError};
use agentrec_core::view;
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

/// `diff <turn> [--json]`: resolve `turn` (full id or unambiguous prefix, K+)
/// and print a unified diff per changed file. Binary files get a byte-count
/// summary instead of a textual diff (F2); skipped/withheld files get a
/// notice (F3) instead of content that was never snapshotted.
///
/// `--json` (P5, AC-1): `serde_json::to_string` of the exact [`view::DiffResult`]
/// `RepositoryView::diff` returned — no adapter-owned field naming, so a
/// field added to `DiffResult` appears here with no edit. A lookup/cursor
/// failure is unaffected by the flag: still prose on stderr via
/// [`diff_error_text`], exit 1 — the plan does not ask for (and P5.md does
/// not pin) a JSON error envelope, so none is invented here.
pub fn diff(root: &Path, turn_ref: &str, json: bool) -> Result<(), String> {
    let view = view::RepositoryView::open(root).map_err(|e| e.to_string())?;
    let q = view::DiffQuery {
        turn: turn_ref.to_string(),
        ..Default::default()
    };
    let result = view.diff(&q).map_err(|e| diff_error_text(turn_ref, &e))?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    print!("{}", render_diff(&result));
    Ok(())
}

/// `diff`'s whole stdout, from the view's typed value alone.
///
/// The signature is the wiring gate: no `&Path`, no [`BlobStore`], no
/// records slice and no closure over any of them, so this cannot render from
/// a direct read even if someone wanted it to. Every fact it prints — the
/// header's id/tool/count, each entry's resolved content — rides on
/// [`view::DiffResult`].
fn render_diff(result: &view::DiffResult) -> String {
    let mut out = String::new();
    out.push_str(&header_line(result));
    out.push('\n');
    for file in &result.files.items {
        push_file_diff(&mut out, file);
    }
    out
}

/// `diff`'s one-line header: `turn <id> · <tool> · <files>`. A third
/// turn-rendering shape (distinct from [`fmt::turn_list_line`]/
/// [`fmt::turn_detail_header`] — `diff` wants the file count, not a
/// timestamp or prompt), but it shares the same canonical [`fmt::SEP`] so
/// the drift D-PD6 closed doesn't reopen here. The count is the turn's own,
/// never the page's, so a paginated query still names the whole turn.
fn header_line(result: &view::DiffResult) -> String {
    let id = fmt::short_id(&result.turn_id);
    let tool = result.tool.as_deref().unwrap_or("—");
    let n = result.total_files;
    let files = if n == 1 {
        "1 file".to_string()
    } else {
        format!("{n} files")
    };
    format!("turn {id}{sep}{tool}{sep}{files}", sep = fmt::SEP)
}

fn push_file_diff(out: &mut String, file: &view::FileDiff) {
    let path = &file.path;
    match &file.state {
        view::FileDiffState::Withheld => {
            out.push_str(&format!(
                "  {path}: (withheld — secret-pattern file, not snapshotted)\n"
            ));
        }
        view::FileDiffState::Skipped { reason } => {
            out.push_str(&format!(
                "  {path}: (content not snapshotted — {})\n",
                fmt::skip_reason_text(reason.as_deref())
            ));
        }
        // Finding #5(b): Missing and Corrupt stay distinct, restoring parity
        // with `build_plan`'s refusal messages. The *cause* of a missing blob
        // stays genuinely unknown (never "purged or missing" — the SR-D
        // defect); a hash mismatch is a distinct, honest fact.
        view::FileDiffState::Unresolvable { corrupt } => {
            let msg = if *corrupt {
                "(snapshot corrupt — hash mismatch)"
            } else {
                "(snapshot unavailable)"
            };
            out.push_str(&format!("  {path}: {msg}\n"));
        }
        view::FileDiffState::Binary {
            before_len,
            after_len,
        } => {
            out.push_str(&format!(
                "  {path}: binary file changed ({before_len} → {after_len} bytes)\n"
            ));
        }
        view::FileDiffState::BaselineUnknown { after } => {
            out.push_str(&format!(
                "  {path}: (baseline unknown — first seen mid-session; showing new content)\n"
            ));
            out.push_str(&diff::unified("", after, path));
        }
        view::FileDiffState::Text {
            before,
            after,
            op,
            after_synthesized,
        } => {
            // D1 (P2 fix round): a synthesized `after` is a DERIVED value (an
            // imported turn's oldString→newString substitution), not bytes
            // anyone actually observed post-edit — the diff below must not be
            // presented as recorded fact without saying so.
            if *after_synthesized {
                out.push_str(&format!(
                    "  {path}: (after-state DERIVED from imported oldString/newString substitution, \
                     not observed)\n"
                ));
            }
            let text = match op.as_str() {
                "create" => diff::unified("", after, path),
                "delete" => diff::unified(before, "", path),
                _ => diff::unified(before, after, path),
            };
            out.push_str(&text);
        }
    }
}

/// The human prose for a [`view::DiffError`]. Every string is built here:
/// the view ships the ledger range as data (F4 names the valid id range so
/// the caller can retry) and this adapter owns `fmt::short_id` and the
/// wording.
fn diff_error_text(turn_ref: &str, e: &view::DiffError) -> String {
    match e {
        view::DiffError::Lookup { err, ledger } => {
            lookup_error_text(turn_ref, err, &turn_range_text(ledger.as_ref()))
        }
        // Unreachable from the CLI, which never paginates `diff`; stated
        // rather than unwrapped so a future paging caller gets a real message
        // instead of a panic.
        view::DiffError::Cursor(c) => match c {
            view::CursorError::Stale => {
                "cursor is stale — the log was rewritten; restart the query".to_string()
            }
            view::CursorError::QueryMismatch => {
                "cursor came from a different query — restart the query".to_string()
            }
            view::CursorError::ZeroLimit => "a limit of 0 has no honest page".to_string(),
        },
        view::DiffError::Io(msg) => msg.clone(),
    }
}

/// The ONE spelling of a failed turn lookup (F4 names the valid id range so
/// the caller can retry). Shared by `diff`'s typed-error path and by
/// [`turn_by_ref`], which `show`/`undo` still use — two independently
/// maintained copies of golden-pinned prose is precisely the renderer drift
/// D-PD6 exists to stop.
fn lookup_error_text(turn_ref: &str, err: &view::LookupError, range: &str) -> String {
    match err {
        view::LookupError::NoTurns => {
            "no turns recorded — is `agentrec record` running?".to_string()
        }
        view::LookupError::Unknown => {
            format!("unknown turn id '{turn_ref}' — recorded turns: {range}")
        }
        view::LookupError::Ambiguous { matched } => format!(
            "ambiguous turn id '{turn_ref}' — matches {matched} turns; recorded turns: {range}"
        ),
    }
}

/// `<oldest_short>..<newest_short> (N turns)` — turns are append order
/// (oldest first) in the log.
fn turn_range_text(range: Option<&view::TurnRangeSummary>) -> String {
    let Some(r) = range else {
        return "(no turns)".to_string();
    };
    format!(
        "{}..{} ({} turns)",
        fmt::short_id(&r.oldest_id),
        fmt::short_id(&r.newest_id),
        r.count
    )
}

/// `show <turn> [--prompt] [--all-files]` (D-PD2, SPEC §Prompt posture item
/// 4): bare form prints only the turn header, via the same renderer `blame`
/// uses — that renders `prompt_excerpt` (already post-scrub AND
/// length-capped), never the full prompt. `--prompt` is the one explicit
/// path to the full post-scrub prompt text, loaded through the
/// integrity-checked blob store; prompt blobs are post-scrub at rest, so
/// printing the full text here is safe. `--all-files` is NF-C's file-class
/// reveal flag (NF-A/NF-B) — it only ever affects the bare-header branch;
/// `--prompt` prints raw prompt bytes to stdout and must never gain an
/// appended line regardless of `all_files` (a fold notice there would
/// silently corrupt the printed prompt).
pub fn show(root: &Path, turn_ref: &str, prompt: bool, all_files: bool) -> Result<(), String> {
    let records = agentrec_core::record::load_log(&log_path(root));
    let turns: Vec<&TurnRecord> = records
        .iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        })
        .collect();

    let turn = turn_by_ref(&turns, turn_ref)?;

    if !prompt {
        println!("{}", render_turn(turn));
        if !all_files {
            let noise_globs = crate::noise::read_noise_globs(root);
            if let Some(matcher) = crate::noise::NoiseMatcher::build(root, &noise_globs) {
                let n = turn
                    .files
                    .iter()
                    .filter(|f| matcher.is_noise(&f.path))
                    .count();
                if n > 0 {
                    println!("+{n} noise files (--all-files to show)");
                }
            }
        }
        return Ok(());
    }

    let Some(prompt_ref) = turn.prompt_ref.as_deref() else {
        // D35 gap closure: `prompt_ref: None` is ambiguous on the wire alone
        // — it's identical whether this turn never had a prompt, or had one
        // whose blob write failed at record time (`state.json`'s
        // `prompt_put_failures`, never a wire/protocol field). `prompt_excerpt`
        // presence is the discriminator — BUT only for turns that could have
        // carried a real prompt. Synthetic turns minted by agentrec itself
        // (`undo` → `tool: "agentrec"`) and git turns (`tool: "git"`) always
        // populate an excerpt ("undo of <id>", etc.) yet never had a prompt
        // blob, so keying on excerpt alone falsely reports "write failed" for
        // every undo turn and sends the user to a `status` that shows no such
        // failure. Exclude those; a real put-failure is a non-synthetic turn
        // with an excerpt but no ref.
        let synthetic = matches!(turn.tool.as_deref(), Some("agentrec") | Some("git"));
        return Err(if turn.prompt_excerpt.is_some() && !synthetic {
            // Hedged wording: the same ref-None + excerpt shape also results
            // from an over-cap (>10 MiB) prompt (`PutResult::OverCap`), which
            // is deliberately NOT counted as DEGRADED — so name both causes
            // rather than asserting a failure the `status` banner may not
            // corroborate.
            "prompt not stored — write failed at record time or over size cap (see `agentrec status`)"
                .to_string()
        } else {
            "no prompt attached — this turn has no recorded prompt".to_string()
        });
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

/// Prose shim over [`view::resolve_turn`]: the lookup itself lives in
/// `agentrec-core`, and this renders its typed failure into the human
/// message (F4 names the valid id range so the caller can retry). No
/// matching logic here — adding any would reintroduce the second
/// implementation the seam exists to prevent.
fn turn_by_ref<'a>(turns: &[&'a TurnRecord], turn_ref: &str) -> Result<&'a TurnRecord, String> {
    view::resolve_turn(turns, turn_ref)
        .map_err(|e| lookup_error_text(turn_ref, &e, &turn_range(turns)))
}

/// `<oldest_short>..<newest_short> (N turns)` — turns are append order
/// (oldest first) in the log. The `show`/`undo` side still holds the records
/// themselves rather than a [`view::TurnRangeSummary`], so it builds one to
/// reach the single range renderer.
fn turn_range(turns: &[&TurnRecord]) -> String {
    let (Some(first), Some(last)) = (turns.first(), turns.last()) else {
        return turn_range_text(None);
    };
    turn_range_text(Some(&view::TurnRangeSummary {
        oldest_id: first.id.clone(),
        newest_id: last.id.clone(),
        count: turns.len(),
    }))
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
///
/// `--json` (P5, AC-1): `serde_json::to_string` of the exact
/// [`view::BlameResult`] `RepositoryView::blame` returned. An uncovered path
/// serializes with a `state.type` tag naming the recording-gap arm and no
/// `turn` field at all (AC-3) — see [`view::BlameState`]'s doc for why no
/// separate `recording_gap` boolean is hand-built on top. Error path
/// unaffected by the flag, same rationale as [`diff`].
pub fn blame(root: &Path, target: &str, json: bool) -> Result<(), String> {
    let (file, line_no) = parse_target(target);
    let view = view::RepositoryView::open(root).map_err(|e| e.to_string())?;
    let q = view::BlameQuery {
        path: file,
        line: line_no,
    };
    let result = view.blame(&q).map_err(|e| blame_error_text(&q.path, &e))?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    print!("{}", render_blame(&result));
    Ok(())
}

/// `blame`'s whole stdout, from the view's typed value alone — the same
/// wiring gate as [`render_diff`]: no root, no store, no records, no
/// closure over any of them. The path and line ride on
/// [`view::BlameResult`] because the line-level renderings prefix them.
fn render_blame(result: &view::BlameResult) -> String {
    // Only the line-level states use this, and they exist only when the
    // query carried a line.
    let line_no = result.line.unwrap_or(0);
    let path = &result.path;
    match &result.state {
        // A gap may hide the turn that actually touched this file.
        view::BlameState::NoTurnRecordingGap => "attribution stale — recording gap\n".to_string(),
        view::BlameState::NoTurnTouches => format!("no recorded turn touches {path}\n"),
        view::BlameState::File {
            turn,
            deleted,
            gap_stale,
            modified,
        } => {
            let mut line = render_turn(turn);
            if *deleted {
                line.push_str(" · deleted this file");
            }
            if *gap_stale {
                line.push_str(" · attribution stale — recording gap");
            } else if *modified {
                line.push_str(" · human-edited since");
            }
            line.push('\n');
            line
        }
        // Deliberately unprefixed: this is an answer about recording
        // coverage, not about a line.
        view::BlameState::LineRecordingGap => "attribution stale — recording gap\n".to_string(),
        view::BlameState::LineDeletedOrAbsent { turn, deleted } => {
            let mut line = render_turn(turn);
            if *deleted {
                line.push_str(" · deleted this file");
            }
            format!("{path}:{line_no}: {line}\n")
        }
        view::BlameState::LineAttributed { turn } => {
            format!("{path}:{line_no}: {}\n", render_turn(turn))
        }
        view::BlameState::LineSnapshotUnavailable => {
            format!("{path}:{line_no}: attribution unavailable — snapshot unavailable\n")
        }
        view::BlameState::LineOriginGap => {
            format!("{path}:{line_no}: attribution stale — recording gap\n")
        }
        view::BlameState::LineBeforeRecording => {
            format!("{path}:{line_no}: before recording began\n")
        }
    }
}

/// The human prose for a [`view::BlameError`]. The view ships a bare
/// discriminant plus the line count; the path comes from the query, and
/// every word is built here.
fn blame_error_text(path: &str, e: &view::BlameError) -> String {
    match e {
        view::BlameError::FileNotFound => format!("{path}: not found"),
        view::BlameError::LineOutOfRange { lines } => format!("{path} has only {lines} line(s)"),
        view::BlameError::Io(msg) => format!("{path}: {msg}"),
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

/// Thin wrapper: computes `show`/`blame`'s caller-owned `hh:mm` field and
/// hands off to the shared [`fmt::turn_detail_header`] renderer (D-PD6 —
/// this used to be a fully independent implementation from `cmds::format_turn`).
fn render_turn(t: &TurnRecord) -> String {
    fmt::turn_detail_header(t, &hhmm(&t.started))
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
        Some(r) => turn_by_ref(&turns, r)?,
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
                fmt::short_id(&target.id),
                unmatched.join(", ")
            ));
        }
    }

    // AC3 (P2, clm_2GCNB5WPT0FKHH4NFBYB9Q2JJT): a turn imported from
    // external history whose selected files include any provenance-only
    // entry (`before: null` — NOT a `create` op, whose `before: null` is
    // legitimate rather than a reconstruction failure) refuses the WHOLE
    // undo BEFORE `build_plan`/`execute_revert` ever run — never a partial
    // revert of the other, genuinely-revertible entries in the same turn.
    // Distinct wording from `withheld` ("secret-pattern..."), `skipped`
    // ("content not snapshotted...") and modified-since ("later agent
    // turn"/"recording gap"/"human or external edit").
    if target.imported == Some(true) {
        let selected: Vec<&FileEntry> = target
            .files
            .iter()
            .filter(|f| files.is_empty() || files.contains(&f.path))
            .collect();
        let unreconstructable = selected
            .iter()
            .filter(|f| f.op != "create" && f.before.is_none())
            .count();
        if unreconstructable > 0 {
            return Err(format!(
                "undo refused: turn {} was imported from external history and has no \
                 recoverable pre-edit content for {unreconstructable} file(s) — \
                 provenance-only, never fabricated; nothing was written",
                fmt::short_id(&target.id)
            ));
        }
    }

    let store = BlobStore::new(objects_dir(root));
    let plans = build_plan(
        root,
        &store,
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

    let short_target = fmt::short_id(&target.id);
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
                imported: None,
                files_complete: None,
                files: inverse_entries,
            };
            let _ = crate::loglock::append_log_locked(&log_path(root), &LogRecord::Turn(partial));
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
        imported: None,
        files_complete: None,
        files: inverse_entries,
    };
    let reverted_n = undo_record.files.len();
    let new_short_id = fmt::short_id(&undo_record.id);
    crate::loglock::append_log_locked(&log_path(root), &LogRecord::Turn(undo_record))?;

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
        // SR6: the skipped gate MUST stay above modified-since (below). A
        // skipped entry is refused unconditionally here and `continue`s
        // before `entry.after` is ever compared against the current on-disk
        // hash — otherwise an unmodified skipped file (SR-C now gives it a
        // real `after` hash) could fall through into the revert path and
        // undo would try to restore a blob that was never stored.
        if entry.skipped {
            // SR-D: the wire field is the per-entry authoritative cause —
            // `state.json`'s `io_failed` is a separate, aggregate/operational
            // channel (drives the DEGRADED banner) and is deliberately never
            // consulted here, so the two can't be made to disagree.
            // Finding #5(a): unified on `print_entry`'s em-dash form (was
            // parenthesized here) — same fact, one spelling; D-PD6 is the
            // tracked debt item for exactly this renderer-drift class.
            let reason = format!(
                "content not snapshotted — {}",
                fmt::skip_reason_text(entry.skipped_reason.as_deref())
            );
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
                    Err(StoreError::Missing(_)) => {
                        Some("prior snapshot unavailable — refusing to restore".to_string())
                    }
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

        let after_synthesized = entry.after_synthesized == Some(true);

        if is_modified && !allow_modified {
            let cause = modified_cause(
                target_idx,
                turns,
                records,
                target,
                &entry.path,
                after_synthesized,
            );
            plans.push(Plan {
                entry: entry.clone(),
                kind: PlanKind::Excluded { cause },
            });
            continue;
        }

        let warn = is_modified.then(|| {
            modified_cause(
                target_idx,
                turns,
                records,
                target,
                &entry.path,
                after_synthesized,
            )
        });
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
    after_synthesized: bool,
) -> String {
    // D1 (P2 fix round, founder decision 2): a synthesized `after` is a
    // DERIVED value, not an observation of what the file actually looked
    // like post-edit — comparing the real on-disk hash against it and
    // reporting a mismatch as "human or external edit" would fabricate
    // attribution nobody earned. This must win over both signals below: a
    // later rich turn or a recording gap are real facts about *observed*
    // history, but neither makes an unobserved comparison point trustworthy.
    if after_synthesized {
        return "imported turn's after-state was derived (not observed) — cannot attribute this difference".to_string();
    }
    let later_touches = turns[target_idx + 1..]
        .iter()
        .any(|t| t.grade == "rich" && t.files.iter().any(|f| f.path == path));
    if later_touches {
        return "later agent turn".to_string();
    }
    if view::has_gap_after(records, &target.ended) {
        return "recording gap".to_string();
    }
    "human or external edit".to_string()
}

fn print_plan(target: &TurnRecord, plans: &[Plan]) {
    println!(
        "undo {} ({})",
        fmt::short_id(&target.id),
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
        skipped_reason: None,
        after_synthesized: None,
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
