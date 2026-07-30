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

    let turn = turn_by_ref(&turns, turn_ref)?;

    let store = BlobStore::new(objects_dir(root));
    println!("{}", header_line(turn));
    for entry in &turn.files {
        print_entry(&store, entry);
    }
    Ok(())
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
    view::resolve_turn(turns, turn_ref).map_err(|e| match e {
        view::LookupError::NoTurns => {
            "no turns recorded — is `agentrec record` running?".to_string()
        }
        view::LookupError::Unknown => format!(
            "unknown turn id '{turn_ref}' — recorded turns: {}",
            turn_range(turns)
        ),
        view::LookupError::Ambiguous { matched } => format!(
            "ambiguous turn id '{turn_ref}' — matches {matched} turns; recorded turns: {}",
            turn_range(turns)
        ),
    })
}

/// `<oldest_short>..<newest_short> (N turns)` — turns are append order
/// (oldest first) in the log.
fn turn_range(turns: &[&TurnRecord]) -> String {
    let (Some(first), Some(last)) = (turns.first(), turns.last()) else {
        return "(no turns)".to_string();
    };
    format!(
        "{}..{} ({} turns)",
        fmt::short_id(&first.id),
        fmt::short_id(&last.id),
        turns.len()
    )
}

/// `diff`'s one-line header: `turn <id> · <tool> · <files>`. A third
/// turn-rendering shape (distinct from [`fmt::turn_list_line`]/
/// [`fmt::turn_detail_header`] — `diff` wants the file count, not a
/// timestamp or prompt), but it shares the same canonical [`fmt::SEP`] so
/// the drift D-PD6 closed doesn't reopen here.
fn header_line(t: &TurnRecord) -> String {
    let id = fmt::short_id(&t.id);
    let tool = t.tool.as_deref().unwrap_or("—");
    let n = t.files.len();
    let files = if n == 1 {
        "1 file".to_string()
    } else {
        format!("{n} files")
    };
    format!("turn {id}{sep}{tool}{sep}{files}", sep = fmt::SEP)
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
            "  {}: (content not snapshotted — {})",
            entry.path,
            fmt::skip_reason_text(entry.skipped_reason.as_deref())
        );
        return;
    }

    let before = match load_blob(store, entry.before.as_deref()) {
        Ok(bytes) => bytes,
        Err(e) => {
            // Finding #5(b): distinguish Missing from Corrupt, restoring
            // parity with `build_plan`'s refusal messages — `diff` was
            // strictly less specific than `undo` about the exact same
            // condition. The *cause* of Missing stays genuinely unknown
            // (never "purged or missing" — that's the SR-D defect); Corrupt
            // is a distinct, honest fact (a hash mismatch), not folded into
            // the same generic message.
            println!("  {}: {}", entry.path, unresolvable_msg(&e));
            return;
        }
    };
    let after = match load_blob(store, entry.after.as_deref()) {
        Ok(bytes) => bytes,
        Err(e) => {
            println!("  {}: {}", entry.path, unresolvable_msg(&e));
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

    // D1 (P2 fix round): `after` here is a DERIVED value (an imported
    // turn's oldString→newString substitution), not bytes anyone actually
    // observed post-edit — the diff below must not be presented as
    // recorded fact without saying so.
    if entry.after_synthesized == Some(true) {
        println!(
            "  {}: (after-state DERIVED from imported oldString/newString substitution, \
             not observed)",
            entry.path
        );
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
/// serve (purged or corrupt) is the only error case — the real
/// [`StoreError`] is preserved (not collapsed) so the caller can render
/// Missing and Corrupt distinctly (finding #5(b)).
fn load_blob(store: &BlobStore, hash: Option<&str>) -> Result<Vec<u8>, StoreError> {
    match hash {
        None => Ok(Vec::new()),
        Some(h) => store.get(h),
    }
}

/// `diff`'s rendering of an unresolvable blob, mirroring `undo`'s
/// `build_plan` distinction (finding #5(b)) rather than `diff` collapsing
/// both into one generic message: `Missing`'s cause stays genuinely
/// unknown (never "purged or missing"), `Corrupt` is a distinct, honest
/// fact. `undo`'s established `StoreError::Corrupt` wording is
/// "prior snapshot corrupt (hash mismatch) — refusing to restore" — kept
/// as-is there; this is `diff`'s own (shorter, no verb) rendering of the
/// same fact.
fn unresolvable_msg(e: &StoreError) -> String {
    match e {
        StoreError::Missing(_) => "(snapshot unavailable)".to_string(),
        StoreError::Corrupt(_) => "(snapshot corrupt — hash mismatch)".to_string(),
    }
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
    let no_turn_gap = view::has_crash_gap(&records);

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

/// Thin wrapper: computes `show`/`blame`'s caller-owned `hh:mm` field and
/// hands off to the shared [`fmt::turn_detail_header`] renderer (D-PD6 —
/// this used to be a fully independent implementation from `cmds::format_turn`).
fn render_turn(t: &TurnRecord) -> String {
    fmt::turn_detail_header(t, &hhmm(&t.started))
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
    let gap_stale = view::has_gap_after(records, &t.ended) && modified;

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
        Some(t) => view::has_gap_after(records, &t.ended) && modified_since(t, file, current_hash),
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

    // `touching` is oldest→newest (append order — see the doc comment
    // above). `responsible` tracks the newest turn whose recorded diff
    // introduces `target_text`; `newest_unresolvable_idx` tracks the newest
    // candidate whose `before`/`after` snapshot didn't resolve, so a turn
    // can't be silently skipped past — mirrors the `has_gap_after`
    // poisoning idiom below, but for missing snapshots instead of missing
    // recording coverage.
    let mut responsible: Option<(usize, &TurnRecord)> = None;
    let mut newest_unresolvable_idx: Option<usize> = None;
    for (idx, t) in touching.iter().enumerate() {
        let Some(entry) = t.files.iter().find(|f| f.path == file) else {
            continue;
        };
        let before = load_text(store, entry.before.as_deref());
        let after = load_text(store, entry.after.as_deref());
        let (Some(before), Some(after)) = (before, after) else {
            // Can't compute this turn's diff at all — it might have
            // introduced or removed `target_text`; treat it as poisoning
            // rather than silently skipping it (that would either wrongly
            // credit an older turn or wrongly fall through to "before
            // recording began").
            newest_unresolvable_idx = Some(idx);
            continue;
        };
        let intro = diff::added_or_changed_lines(&before, &after);
        if intro.iter().any(|l| l == target_text) {
            responsible = Some((idx, t));
        }
    }

    // A responsible turn is only honestly reportable when no unresolvable
    // candidate is NEWER than it — a newer unresolvable turn could have
    // overwritten the line, so naming the older turn would be a guess.
    let responsible_poisoned = matches!(
        (responsible, newest_unresolvable_idx),
        (Some((r_idx, _)), Some(u_idx)) if u_idx > r_idx
    );

    match responsible {
        Some((_, t)) if !responsible_poisoned => {
            println!("{file}:{line_no}: {}", render_turn(t));
        }
        // Some candidate's snapshot didn't resolve (whether or not a
        // now-poisoned `responsible` was also found) — the CRITICAL SCOPE
        // RULE: this branches only on `store.get` failing, never on why.
        _ if newest_unresolvable_idx.is_some() => {
            println!("{file}:{line_no}: attribution unavailable — snapshot unavailable");
        }
        // E3: no turn's recorded diff introduces this exact line text, and
        // every candidate's snapshot resolved cleanly. That is only
        // honestly "before recording began" when the whole history is
        // actually gap-free — a line that was silently added during an
        // uncovered interval (then folded into a later turn's unchanged
        // `before`) would otherwise be misreported as predating all
        // recording, when really its origin is just unknown.
        None if view::has_gap_after(records, "") => {
            println!("{file}:{line_no}: attribution stale — recording gap");
        }
        _ => println!("{file}:{line_no}: before recording began"),
    }
    Ok(())
}

/// Loads blob text by optional hash, distinguishing two shapes callers must
/// not conflate: `None` hash is legitimately-empty text (a `create` op has
/// no `before` — every line of its `after` really was introduced by that
/// turn, and collapsing that to "unresolvable" would wrongly deny credit
/// for every file-creating turn). `Some(hash)` that fails to resolve (any
/// error — the caller must not, and does not, care which) is UNRESOLVABLE
/// and returned as `None`, never silently coerced to empty text: an
/// unresolvable snapshot is not "no text there", it's "no idea what text
/// was there", and must poison the comparison rather than let it pass
/// through as if nothing changed.
fn load_text(store: &BlobStore, hash: Option<&str>) -> Option<String> {
    match hash {
        None => Some(String::new()),
        Some(h) => store
            .get(h)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
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

#[cfg(test)]
mod tests {
    use super::*;

    // BL1: `load_text` must tell "legitimately empty" (no hash at all, e.g.
    // a `create` op's `before`) apart from "unresolvable" (a hash is
    // recorded but the blob won't load) — collapsing both to `""` is
    // exactly the bug (blame credits any turn for any line once one
    // candidate's `before` goes missing).
    #[test]
    fn load_text_distinguishes_absent_resolvable_and_unresolvable() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path().join("objects"));

        // Absent hash (e.g. a `create` op's `before`) — legitimately empty.
        assert_eq!(load_text(&store, None), Some(String::new()));

        // Resolvable hash — real content comes back.
        let hash = store.put(b"hello\n").unwrap();
        assert_eq!(load_text(&store, Some(&hash)), Some("hello\n".to_string()));

        // Present-but-unresolvable hash: well-formed, genuinely never
        // stored — proves the store really can't resolve it, rather than
        // assuming so.
        let ghost = agentrec_core::store::hash_bytes(b"never-actually-stored");
        assert!(
            store.get(&ghost).is_err(),
            "precondition: ghost must be genuinely unresolvable"
        );
        assert_eq!(load_text(&store, Some(&ghost)), None);
    }
}
