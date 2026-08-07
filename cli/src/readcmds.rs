//! `diff`: unified diff of one turn's file changes (AC F1–F4). `undo`: revert
//! a turn's changes, per file (AC H1–H7, Z+1). Both are read-mostly — `diff`
//! and undo's preview never need the daemon running; `undo --confirm` writes
//! to the worktree but not to the daemon's internal state.

use crate::cmds::wall_now_ms;
use crate::{fmt, log_path, objects_dir, undo_guard_path, UndoGuard};
use agentrec_core::diff;
use agentrec_core::record::{FileEntry, LogRecord, TurnRecord, UndoOrigin};
use agentrec_core::store::{BlobStore, StoreError};
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
            let noise_globs = crate::noise::read_noise_globs(root)?;
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
            let _ = append_undo_turn(root, &short_target, inverse_entries, true, UndoOrigin::Cli);
        }
        // Any entries already reverted are real writes a concurrent daemon
        // must still not misattribute, so this waits out the same linger as
        // the success path before cleaning up.
        finish_undo_guard(root);
        return Err(e);
    }

    let reverted_n = inverse_entries.len();
    let new_id = append_undo_turn(root, &short_target, inverse_entries, false, UndoOrigin::Cli)?;
    let new_short_id = fmt::short_id(&new_id);

    finish_undo_guard(root);

    println!("reverted {reverted_n} file(s); recorded as turn {new_short_id}");
    Ok(())
}

/// Build and append the undo turn for a completed (or, with `truncated`, an
/// aborted) revert, returning its id.
///
/// The ONE place this record's shape is decided. `agentrec approve` (task F3)
/// appends through it too, so "an approved undo is recorded exactly as
/// `undo --confirm` records one" is true by construction rather than by two
/// struct literals someone has to keep in agreement. F5's `origin`
/// discriminator (delta decision 11) lands here for the same reason: it is a
/// parameter of THIS function, so a new transport cannot record an undo
/// without stating which surface it is, and the partial/`truncated` append
/// on an aborted revert cannot disagree with its success sibling — each
/// caller passes one `origin` value to both of its call sites.
///
/// `origin` is the surface that EXECUTED the writes, never the one that
/// requested them: `agentrec approve` passes [`UndoOrigin::Cli`] even though
/// the request it is approving arrived over MCP (a human ran the verb), and
/// the request's own provenance stays in `.agentrec/undo-requests.jsonl`.
pub(crate) fn append_undo_turn(
    root: &Path,
    short_target: &str,
    files: Vec<FileEntry>,
    truncated: bool,
    origin: UndoOrigin,
) -> Result<String, String> {
    let now = wall_now_ms();
    let excerpt = if truncated {
        format!("undo of {short_target} (partial — aborted mid-revert)")
    } else {
        format!("undo of {short_target}")
    };
    let record = TurnRecord {
        v: 1,
        id: agentrec_core::id::turn_id(),
        grade: "rich".to_string(),
        truncated,
        started: agentrec_core::time::rfc3339(now),
        ended: agentrec_core::time::rfc3339(now),
        tool: Some("agentrec".to_string()),
        model: None,
        session: None,
        root: root.to_string_lossy().to_string(),
        prompt_ref: None,
        prompt_excerpt: Some(excerpt),
        merges: vec![],
        imported: None,
        files_complete: None,
        origin: Some(origin.as_str().to_string()),
        files,
    };
    let id = record.id.clone();
    crate::loglock::append_log_locked(&log_path(root), &LogRecord::Turn(record))?;
    Ok(id)
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

// `Plan`/`PlanKind`/`build_plan` and its helpers (`read_current_hash`,
// `is_symlink_on_disk`, `symlink_refusal`, `modified_cause`) plus
// `window_caution` MOVED to `agentrec_core::undo_coordinator` (task F2);
// `execute_revert`/`restore_from_before` followed in task F3, so `agentrec
// approve` reverts through THIS function rather than a second copy.
// The MCP `agentrec_undo` preview has to reach exactly the same refusal
// interpretation the CLI's preview does — D42 panic mode, D49 caution, F2
// symlink refusal, K2 imported-unreconstructible — and reimplementing any
// of it behind the MCP seam is what the core seam exists to prevent. The
// bodies moved VERBATIM (gate order is load-bearing: symlink, withheld,
// skipped/SR6, before-blob integrity, modified-since). Re-imported under
// their original names so this module's renderer and its whole test module
// resolve unchanged — an untouched test passing against a moved
// implementation is the behavior-preservation proof.
use agentrec_core::undo_coordinator::{build_plan, execute_revert, window_caution, Plan, PlanKind};

/// The exact bytes of the pre-`--confirm` undo plan, one `\n`-terminated line
/// per emitted row. Split out of [`print_plan`] so the text a user reads
/// before authorizing a destructive op is unit-testable as a value; the
/// printer is a thin wrapper and nothing else builds this text.
///
/// Every WIRE-SOURCED field interpolated here — `target.tool`, `entry.path`,
/// `entry.op` — passes through [`fmt::sanitize_terminal`] (redteam round 2,
/// F8). A filename is attacker-controllable (nothing stops an agent or a
/// postinstall script creating `"\x1b[1A\x1b[2Ksrc/decoy.rs"` — all legal
/// bytes), and cursor-up + erase-line reaching a real terminal would wipe the
/// `revert` line printed above it from the display while `--confirm` reverts
/// that file anyway, rewriting the one human checkpoint this destructive op
/// has. `sanitize_terminal` needed no extension for this: ESC is `0x1b`, so
/// its existing `cp >= 0x20` filter already dropped it — `fmt.rs`'s
/// `sanitize_terminal_strips_f8_cursor_up_erase_line` and
/// `render_plan_neutralizes_f8_erase_line_payload` below pin that rather
/// than assuming it.
///
/// `cause` and `reason` are NOT sanitized at this render site, and that was
/// probed rather than reasoned: every value able to reach them is either a
/// fixed literal built in [`build_plan`], [`modified_cause`], or
/// [`fmt::skip_reason_text`], or — the one wire interpolation, added by F2
/// after this comment first claimed there were none — `entry.link_kind` in
/// [`symlink_refusal`], which sanitizes it at the interpolation site.
/// Adding another wire interpolation requires sanitizing it where it is
/// built, or this render site stops being safe. `target.id` is
/// likewise left as-is: it is reachable only under a different (hostile
/// log-writer) threat model, and [`fmt::turn_list_line`] renders the same id
/// unsanitized, so treating it here alone would split the treatment without
/// closing anything.
pub(crate) fn render_plan(target: &TurnRecord, plans: &[Plan]) -> String {
    let mut out = String::new();
    let tool = fmt::sanitize_terminal(target.tool.as_deref().unwrap_or("—"));
    out.push_str(&format!("undo {} ({tool})\n", fmt::short_id(&target.id)));
    for p in plans {
        let path = fmt::sanitize_terminal(&p.entry.path);
        match &p.kind {
            PlanKind::Revert { warn } => {
                let op = fmt::sanitize_terminal(&p.entry.op);
                out.push_str(&format!("  revert  {path} ({op})\n"));
                if let Some(cause) = warn {
                    out.push_str(&format!(
                        "    WARNING: {path} modified since ({cause}) — reverting anyway (--allow-modified)\n"
                    ));
                }
            }
            PlanKind::Excluded { cause } => {
                out.push_str(&format!(
                    "  EXCLUDE {path} — modified since ({cause}); --allow-modified to include\n"
                ));
            }
            PlanKind::Refused { reason } => {
                out.push_str(&format!("  REFUSE  {path} — {reason}\n"));
            }
        }
    }
    // The caution text itself lives in core (one string for the CLI preview
    // and the MCP preview both); the two-space row indent is this renderer's
    // and is applied here. Byte-for-byte identical output to before the move
    // — `render_plan_*` goldens are the proof.
    if let Some(caution) = window_caution(target, plans) {
        out.push_str(&format!("  {caution}\n"));
    }
    out
}

pub(crate) fn print_plan(target: &TurnRecord, plans: &[Plan]) {
    print!("{}", render_plan(target, plans));
}

/// E8: `Some(reason)` when an unexpired H7 coordination guard already exists
/// — a second concurrent `undo --confirm` must refuse rather than clobber
/// it (clobbering would let the first undo's in-flight writes be mistaken
/// for a bare turn, and the two guards would delete each other on cleanup).
/// An absent, malformed, or expired guard is not live: `None`.
pub(crate) fn live_undo_guard_reason(root: &Path) -> Option<String> {
    // fsguard (round-7 blocker): this read is on all three undo legs — CLI
    // undo, `agentrec approve`, and MCP execute (agent-triggerable) — and a
    // fifo at this path hung every one of them. `daemon.rs:992` already
    // guards the SAME file; this was the mirror site the per-file pass
    // missed.
    let text = agentrec_core::fsguard::read_regular_to_string(&undo_guard_path(root)).ok()?;
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
pub(crate) fn write_undo_guard(root: &Path, paths: &[String]) -> Result<(), String> {
    let guard = UndoGuard {
        paths: paths.to_vec(),
        until_ms: wall_now_ms() + 30_000,
    };
    let text = serde_json::to_string(&guard).map_err(|e| e.to_string())?;
    let path = undo_guard_path(root);
    // Write-side fsguard mirror: `fs::write` opens create+truncate, which
    // blocks forever on a FIFO at this fixed in-repo path — and this runs
    // BEFORE any file mutation, so a hang here wedges the undo silently.
    if agentrec_core::fsguard::is_nonregular(&path) {
        return Err(format!(
            "{} is not a regular file — refusing to write",
            path.display()
        ));
    }
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
pub(crate) fn finish_undo_guard(root: &Path) {
    std::thread::sleep(GUARD_LINGER);
    let _ = std::fs::remove_file(undo_guard_path(root));
}

#[cfg(test)]
// Test code reads its own tempdir fixtures; no attacker-supplied FIFO can
// block these, so the fsguard wrappers buy nothing. Scoped to this module
// so production reads in this file stay lint-enforced (clippy.toml).
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    // Moved to core in F3 alongside `execute_revert`; used only by this
    // module's tests, which are the behavior-preservation proof for the move.
    use agentrec_core::undo_coordinator::restore_from_before;

    fn entry(path: &str, op: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            before: Some("a".repeat(64)),
            after: Some("b".repeat(64)),
            op: op.to_string(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
            link_kind: None,
            attribution: None,
        }
    }

    fn turn(grade: &str, tool: Option<&str>) -> TurnRecord {
        TurnRecord {
            v: 1,
            id: "t_ABCD00000000000000EFGH".to_string(),
            grade: grade.to_string(),
            truncated: false,
            started: "2026-01-01T00:00:00.000Z".into(),
            ended: "2026-01-01T00:00:01.000Z".into(),
            tool: tool.map(String::from),
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
        }
    }

    // Byte-exact shape of every plan row on clean input. There is no `undo`
    // golden file (checked: `cli/tests/fixtures/golden/` has none), so this
    // stands in for one — sanitizing an already-clean path/tool/op MUST be
    // the identity, and a stray extra or missing `\n` from the
    // `println!`-per-row → single-`String` refactor reds here. `tool:
    // "agentrec"` keeps `window_caution` out of the expected text (its own
    // tests own that line).
    #[test]
    fn render_plan_clean_input_is_byte_exact() {
        let t = turn("rich", Some("agentrec"));
        let plans = vec![
            Plan {
                entry: entry("src/app.rs", "modify"),
                kind: PlanKind::Revert { warn: None },
            },
            Plan {
                entry: entry("src/b.rs", "modify"),
                kind: PlanKind::Excluded {
                    cause: "later agent turn".to_string(),
                },
            },
            Plan {
                entry: entry("src/c.rs", "modify"),
                kind: PlanKind::Refused {
                    reason: "no prior snapshot to restore".to_string(),
                },
            },
        ];
        assert_eq!(
            render_plan(&t, &plans),
            "undo t_ABCD…EFGH (agentrec)\n\
             \x20 revert  src/app.rs (modify)\n\
             \x20 EXCLUDE src/b.rs — modified since (later agent turn); --allow-modified to include\n\
             \x20 REFUSE  src/c.rs — no prior snapshot to restore\n"
        );
    }

    // The caution ROW's exact bytes, indent included.
    //
    // Task F2 moved the caution text into `agentrec_core::undo_coordinator`
    // (one string for the CLI preview and the MCP preview both) and left the
    // two-space row indent here — a coordinated edit across a crate boundary
    // that NOTHING in the suite pinned: deleting the indent from
    // `render_plan` reds zero tests (measured). `misattribution.rs` asserts
    // the caution by SUBSTRING, so it cannot see the indent at all. This
    // pins it. `render_plan_clean_input_is_byte_exact` above cannot: its
    // turn is `tool: "agentrec"`, the one rich shape `window_caution`
    // deliberately excludes.
    #[test]
    fn render_plan_caution_row_is_byte_exact_including_its_indent() {
        let t = turn("rich", Some("claude"));
        let plans = vec![Plan {
            entry: entry("src/app.rs", "modify"),
            kind: PlanKind::Revert { warn: None },
        }];
        assert_eq!(
            render_plan(&t, &plans),
            "undo t_ABCD…EFGH (claude)\n\
             \x20 revert  src/app.rs (modify)\n\
             \x20 CAUTION: this turn's file list is an activity window, not an authorship \
             record — agentrec cannot distinguish the recorded tool's own writes from \
             concurrent human edits made in the same window (D6), and every file marked \
             `revert` above is reverted regardless of who wrote it. Review the list before \
             confirming.\n"
        );
    }

    // F8 (redteam round 2), the attack as reported: a second file named
    // `"\x1b[1A\x1b[2Ksrc/decoy.rs"` (cursor-up + erase-line) whose EXCLUDE
    // row, rendered raw, erases the `revert src/prod_config.rs` row above it
    // from the display — while `--confirm` reverts prod_config.rs anyway.
    // Asserting only "no 0x1b in output" is too weak (a fix that swapped ESC
    // for another active introducer would pass it), so this pins the
    // structural property the attack needs: both rows survive, on separate
    // lines, with the hostile filename still readable as inert literal text.
    // RED before the fix: `p.entry.path`/`target.tool` were interpolated raw.
    #[test]
    fn render_plan_neutralizes_f8_erase_line_payload() {
        let t = turn("rich", Some("\x1b[2Kclaude"));
        let plans = vec![
            Plan {
                entry: entry("src/prod_config.rs", "modify"),
                kind: PlanKind::Revert { warn: None },
            },
            Plan {
                entry: entry("\x1b[1A\x1b[2Ksrc/decoy.rs", "modify"),
                kind: PlanKind::Excluded {
                    cause: "later agent turn".to_string(),
                },
            },
        ];
        let out = render_plan(&t, &plans);
        assert!(!out.contains('\x1b'), "raw ESC reached the plan: {out:?}");
        assert!(!out.chars().any(|c| {
            let cp = c as u32;
            cp < 0x20 && c != '\n' || cp == 0x7f || (0x80..=0x9f).contains(&cp)
        }));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "undo t_ABCD…EFGH ([2Kclaude)");
        assert_eq!(lines[1], "  revert  src/prod_config.rs (modify)");
        assert_eq!(
            lines[2],
            "  EXCLUDE [1A[2Ksrc/decoy.rs — modified since (later agent turn); --allow-modified to include"
        );
    }

    // The same vector on the two remaining wire fields this row can carry:
    // `entry.op` (interpolated beside the path on a revert row) and the path
    // repeated in the `--allow-modified` WARNING row. Both are attacker-
    // reachable on an imported or foreign-written log, and the WARNING row is
    // the one that says a modified file is being clobbered anyway.
    #[test]
    fn render_plan_sanitizes_op_and_warning_row() {
        let t = turn("rich", None);
        let plans = vec![Plan {
            entry: entry("\u{9b}2Ksrc/evil.rs", "mod\x1b[1Aify"),
            kind: PlanKind::Revert {
                warn: Some("human or external edit".to_string()),
            },
        }];
        let out = render_plan(&t, &plans);
        assert!(!out.contains('\x1b'));
        assert!(!out.contains('\u{9b}'));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "undo t_ABCD…EFGH (—)");
        assert_eq!(lines[1], "  revert  2Ksrc/evil.rs (mod[1Aify)");
        assert_eq!(
            lines[2],
            "    WARNING: 2Ksrc/evil.rs modified since (human or external edit) — reverting anyway (--allow-modified)"
        );
    }

    // ---- F2 (red team round 2): symlink refusals -------------------------

    fn plan_for(
        root: &Path,
        entries: Vec<FileEntry>,
        allow_modified: bool,
    ) -> (TurnRecord, Vec<Plan>) {
        let store = BlobStore::new(objects_dir(root));
        let mut t = turn("rich", Some("claude"));
        t.files = entries;
        let turns = vec![&t];
        let plans = build_plan(root, &store, &t, 0, &turns, &[], &[], allow_modified);
        (t.clone(), plans)
    }

    fn refusal_reason(plans: &[Plan]) -> String {
        match &plans[0].kind {
            PlanKind::Refused { reason } => reason.clone(),
            PlanKind::Excluded { cause } => panic!("expected REFUSE, got EXCLUDE ({cause})"),
            PlanKind::Revert { .. } => panic!("expected REFUSE, got revert"),
        }
    }

    // Trigger 1, record side. An UNKNOWN `link_kind` value must refuse to
    // ACT while still having parsed fine (the parse half is
    // `record.rs::link_kind_unknown_value_round_trips`) — refuse-to-act,
    // never refuse-to-parse. If this ever becomes an `== "symlink"` equality
    // test, this reds.
    #[test]
    fn build_plan_refuses_unknown_link_kind_value() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut e = entry("mnt/j", "modify");
        e.link_kind = Some("junction".to_string());
        let (_, plans) = plan_for(root, vec![e], false);
        assert_eq!(
            refusal_reason(&plans),
            "recorded as a junction — its snapshot is the link target, not file content"
        );
    }

    // Skeptic-gate blocker (2026-08-01): `link_kind` is wire data on an open
    // enum, and its value is interpolated into the REFUSE row the user reads
    // before `--confirm` — F8's exact surface, reachable by a hostile log
    // writer or a future importer. The kind must render inert.
    #[test]
    fn refusal_reason_sanitizes_a_hostile_link_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut e = entry("mnt/evil", "modify");
        e.link_kind = Some("\x1b[1A\x1b[2Ksymlink".to_string());
        let (_, plans) = plan_for(root, vec![e], false);
        let reason = refusal_reason(&plans);
        assert!(!reason.contains('\x1b'), "reason leaked ESC: {reason:?}");
        assert_eq!(
            reason,
            "recorded as a [1A[2Ksymlink — its snapshot is the link target, not file content"
        );
        let rendered = render_plan(&turn("rich", Some("claude")), &plans);
        assert!(!rendered.contains('\x1b'), "plan leaked ESC: {rendered:?}");
    }

    // Trigger 2, the LEGACY-RECORD guard: the entry carries no `link_kind`
    // (it predates the field), so only the on-disk lstat can save it. The
    // distinct wording is what proves this branch fired rather than trigger
    // 1 — identical text would let this test pass for the wrong reason.
    #[test]
    #[cfg(unix)]
    fn build_plan_refuses_on_disk_symlink_for_legacy_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("real.yaml"), b"real\n").unwrap();
        std::os::unix::fs::symlink("real.yaml", root.join("cfg.yaml")).unwrap();

        let e = entry("cfg.yaml", "modify");
        assert_eq!(e.link_kind, None, "fixture must be a pre-link_kind entry");
        let (_, plans) = plan_for(root, vec![e], false);
        assert_eq!(
            refusal_reason(&plans),
            "path is a symlink on disk — reverting would write through the link"
        );
    }

    // `--allow-modified` overrides the modified-since EXCLUDE and nothing
    // else. Both triggers are asserted under the flag, because the flag is
    // exactly the path F2b weaponized.
    #[test]
    #[cfg(unix)]
    fn allow_modified_never_overrides_a_symlink_refusal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("real.yaml"), b"real\n").unwrap();
        std::os::unix::fs::symlink("real.yaml", root.join("cfg.yaml")).unwrap();

        let (_, plans) = plan_for(root, vec![entry("cfg.yaml", "modify")], true);
        assert_eq!(
            refusal_reason(&plans),
            "path is a symlink on disk — reverting would write through the link"
        );

        let mut e = entry("nowhere/link", "delete");
        e.link_kind = Some(agentrec_core::record::link_kind::SYMLINK.to_string());
        let (_, plans) = plan_for(root, vec![e], true);
        assert_eq!(
            refusal_reason(&plans),
            "recorded as a symlink — its snapshot is the link target, not file content"
        );
    }

    // Byte-exact REFUSE row, same standing-in-for-a-golden role as
    // `render_plan_clean_input_is_byte_exact` (there is still no `undo`
    // golden file).
    #[test]
    fn render_plan_symlink_refusal_row_is_byte_exact() {
        let t = turn("rich", Some("agentrec"));
        let plans = vec![Plan {
            entry: entry("cfg.yaml", "modify"),
            kind: PlanKind::Refused {
                reason: "path is a symlink on disk — reverting would write through the link"
                    .to_string(),
            },
        }];
        assert_eq!(
            render_plan(&t, &plans),
            "undo t_ABCD…EFGH (agentrec)\n\
             \x20 REFUSE  cfg.yaml — path is a symlink on disk — reverting would write through \
             the link\n"
        );
    }

    // F2b at the write primitive itself (the second gate). The pointed-to
    // file must survive byte-identical — that is the whole finding: today
    // `fs::write` truncates it and the read-back verification passes.
    #[test]
    #[cfg(unix)]
    fn restore_from_before_refuses_symlink_and_leaves_target_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let store = BlobStore::new(objects_dir(root));
        let target = root.join("prod.yaml");
        std::fs::write(&target, b"PRODUCTION CONFIG\n").unwrap();
        let link = root.join("cfg.yaml");
        std::os::unix::fs::symlink("prod.yaml", &link).unwrap();

        let mut e = entry("cfg.yaml", "modify");
        e.before = Some(store.put(b"dev.yaml").unwrap());

        let err = restore_from_before(&link, &store, &e).unwrap_err();
        assert!(err.contains("symlink — refusing to restore"), "err: {err}");
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"PRODUCTION CONFIG\n",
            "a file that was never in the plan must not be touched"
        );
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link itself must survive"
        );
    }

    // The one op path `restore_from_before`'s second gate does NOT cover:
    // reverting a `create` deletes rather than writes, so it never reaches
    // `restore_from_before` at all and the planner gate is the only thing
    // standing between an on-disk link and `remove_file`. That is exactly
    // what the comment on `restore_from_before` claims, so it is pinned here
    // rather than left as an assertion nobody measured. `remove_file`
    // unlinks the link (it does not follow it), so the pointed-to file would
    // survive — but the LINK would not, and undo never recorded it.
    #[test]
    #[cfg(unix)]
    fn build_plan_refuses_create_op_when_path_is_an_on_disk_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("real.txt"), b"real\n").unwrap();
        std::os::unix::fs::symlink("real.txt", root.join("made.txt")).unwrap();

        let (_, plans) = plan_for(root, vec![entry("made.txt", "create")], true);
        assert_eq!(
            refusal_reason(&plans),
            "path is a symlink on disk — reverting would write through the link",
            "a create-revert must not unlink a symlink undo never recorded"
        );
    }

    // ---- Root containment (branch review, PR #20 blockers 1+2) -----------
    //
    // `entry.path` is wire data, and before this gate `root.join(&path)` was
    // an arbitrary-write primitive reachable by an agent through
    // `agentrec_undo` in `auto` mode with no human in the loop. Each shape
    // below was proven exploitable end-to-end against the real binary before
    // the fix; the gate lives in the shared `build_plan`, so these cover the
    // MCP leg too.

    const ESCAPE: &str =
        "path escapes the repository root — refusing to write outside the recorded repo";

    #[test]
    fn build_plan_refuses_parent_dir_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(tmp.path().join("outside")).unwrap();
        std::fs::write(tmp.path().join("outside/victim.txt"), b"VICTIM\n").unwrap();

        let (_, plans) = plan_for(&root, vec![entry("../outside/victim.txt", "modify")], true);
        assert_eq!(refusal_reason(&plans), ESCAPE);
    }

    #[test]
    fn build_plan_refuses_absolute_path() {
        // `Path::join` with an absolute path DISCARDS the base, so a
        // `..`-component-only guard would not catch this shape at all.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let abs = tmp.path().join("elsewhere.txt");
        std::fs::write(&abs, b"VICTIM\n").unwrap();

        let (_, plans) = plan_for(
            root,
            vec![entry(&abs.display().to_string(), "modify")],
            true,
        );
        assert_eq!(refusal_reason(&plans), ESCAPE);
    }

    // The worse shape: lexically innocent, so neither a `..` check nor the
    // human reading the rendered plan would catch it. `is_symlink_on_disk`
    // lstats the FINAL component only, while `restore_from_before` runs
    // `create_dir_all(parent)` and writes THROUGH the intermediate link.
    #[test]
    #[cfg(unix)]
    fn build_plan_refuses_symlinked_parent_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("c.txt"), b"VICTIM\n").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linkdir")).unwrap();

        let (_, plans) = plan_for(&root, vec![entry("linkdir/c.txt", "modify")], true);
        assert_eq!(
            refusal_reason(&plans),
            ESCAPE,
            "an intermediate symlink escapes every lexical check and the final-component lstat"
        );
    }

    // Positive control, and it must DISCRIMINATE: an ordinary in-root path
    // has to reach the gates BELOW containment. Asserting the store-missing
    // refusal (rather than merely "not ESCAPE") proves the entry was still
    // being evaluated, so a containment gate that refused everything would
    // red here.
    #[test]
    fn build_plan_allows_an_ordinary_in_root_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}\n").unwrap();

        let (_, plans) = plan_for(root, vec![entry("src/main.rs", "modify")], true);
        assert_eq!(
            refusal_reason(&plans),
            "prior snapshot unavailable — refusing to restore",
            "containment must pass this through to the store check, not refuse it"
        );
    }

    // A revert whose target directory does not exist yet is legitimate
    // (`restore_from_before` calls `create_dir_all`). Containment resolves
    // the nearest EXISTING ancestor precisely so this is not refused.
    #[test]
    fn build_plan_allows_a_path_whose_directory_does_not_exist_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let (_, plans) = plan_for(root, vec![entry("not/here/yet.txt", "modify")], true);
        assert_eq!(
            refusal_reason(&plans),
            "prior snapshot unavailable — refusing to restore",
            "absent intermediate dirs cannot be links, so they must not trip containment"
        );
    }

    // Hardlink escape (branch review re-gate, PR #20 blocker 1). Path
    // containment CANNOT close this: `canonicalize` resolves symlinks, and a
    // hardlink leaves no path-level evidence that the inode has another name.
    // The repo-internal path is lexically ordinary, canonicalizes inside
    // root, and is not a symlink — yet `fs::write` truncates the shared inode
    // and the outside name sees the new bytes. Proven through MCP auto mode
    // before this gate existed.
    #[test]
    #[cfg(unix)]
    fn build_plan_refuses_a_hardlink_to_a_file_outside_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let victim = tmp.path().join("victim.txt");
        std::fs::write(&victim, b"VICTIM\n").unwrap();
        std::fs::hard_link(&victim, root.join("hard.txt")).unwrap();

        let (_, plans) = plan_for(&root, vec![entry("hard.txt", "modify")], true);
        assert_eq!(
            refusal_reason(&plans),
            "path is a hardlink — its inode has another name, which reverting would also rewrite"
        );
    }

    // The predicate is `nlink > 1`, deliberately not "the other name is
    // outside the root" — we cannot know where it is, and a second name
    // inside the repo is equally a file this turn's record does not describe.
    #[test]
    #[cfg(unix)]
    fn build_plan_refuses_a_hardlink_whose_other_name_is_inside_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("a.txt"), b"shared\n").unwrap();
        std::fs::hard_link(root.join("a.txt"), root.join("b.txt")).unwrap();

        let (_, plans) = plan_for(root, vec![entry("b.txt", "modify")], true);
        assert!(
            refusal_reason(&plans).starts_with("path is a hardlink"),
            "an in-root second name is still a file the record does not describe"
        );
    }

    // A FIFO target HUNG the process before this gate: `fs::read` on a fifo
    // blocks until a writer appears, which for the single-threaded stdio MCP
    // loop kills the whole agent-facing surface for the session, and hangs
    // `agentrec undo` for a human identically. `mkfifo` needs no privileges
    // and creating one is a write inside cwd — same sandboxed-agent
    // precondition as every escape shape above. This test would HANG, not
    // fail, if the gate regressed to skipping non-regular files.
    #[test]
    #[cfg(unix)]
    fn build_plan_refuses_a_fifo_target() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let fifo = root.join("pipe");
        let rc = unsafe {
            let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
            libc::mkfifo(c.as_ptr(), 0o600)
        };
        assert_eq!(rc, 0, "fixture must actually create a fifo");

        let (_, plans) = plan_for(root, vec![entry("pipe", "modify")], true);
        assert_eq!(
            refusal_reason(&plans),
            "path is not a regular file (directory, fifo, socket or device) — reverting cannot \
             read or write it as file content"
        );
    }

    // A directory recorded as a `modify` entry used to be rendered as a
    // performable `revert`, then failed at execution with `Is a directory` —
    // a plan promising an action it cannot take.
    #[test]
    #[cfg(unix)]
    fn build_plan_refuses_a_directory_target() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("adir")).unwrap();

        let (_, plans) = plan_for(root, vec![entry("adir", "modify")], true);
        assert!(
            refusal_reason(&plans).starts_with("path is not a regular file"),
            "a plan must not promise a revert it cannot perform"
        );
    }

    // The non-regular-file branch must NOT steal the symlink refusal: that is
    // `symlink_refusal`'s, with its own distinct wording, and stealing it
    // would make its tests pass for the wrong reason. (The symlink tests
    // above are the assertion; this one names the intent.)
    #[test]
    #[cfg(unix)]
    fn inode_gate_leaves_symlinks_to_the_symlink_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("real.txt"), b"real\n").unwrap();
        std::os::unix::fs::symlink("real.txt", root.join("link.txt")).unwrap();

        let (_, plans) = plan_for(root, vec![entry("link.txt", "modify")], true);
        assert_eq!(
            refusal_reason(&plans),
            "path is a symlink on disk — reverting would write through the link",
            "the inode gate must not shadow the symlink gate's distinct wording"
        );
    }

    // Discriminating control for the hardlink gate specifically: an ordinary
    // single-named file at nlink == 1 must pass it and reach the store gate.
    #[test]
    #[cfg(unix)]
    fn build_plan_allows_an_ordinary_single_linked_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("plain.txt"), b"plain\n").unwrap();

        let (_, plans) = plan_for(root, vec![entry("plain.txt", "modify")], true);
        assert_eq!(
            refusal_reason(&plans),
            "prior snapshot unavailable — refusing to restore",
            "nlink == 1 must not trip the hardlink gate"
        );
    }

    // A root that is ITSELF reached through a symlink (macOS `/tmp` →
    // `/private/tmp` is the everyday case) must not make every revert under
    // it look like an escape — which is what comparing a canonical child
    // against a non-canonical root would do.
    #[test]
    #[cfg(unix)]
    fn build_plan_allows_in_root_paths_under_a_symlinked_root() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real-repo");
        std::fs::create_dir_all(real.join("src")).unwrap();
        std::fs::write(real.join("src/main.rs"), b"fn main() {}\n").unwrap();
        let linked_root = tmp.path().join("linked-repo");
        std::os::unix::fs::symlink(&real, &linked_root).unwrap();

        let (_, plans) = plan_for(&linked_root, vec![entry("src/main.rs", "modify")], true);
        assert_eq!(
            refusal_reason(&plans),
            "prior snapshot unavailable — refusing to restore",
            "a symlinked root is ordinary, not an escape"
        );
    }
}
