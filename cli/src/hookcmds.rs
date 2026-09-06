//! `agentrec hook codex`: emitter-side mapping from Codex's three lifecycle
//! hook events (`UserPromptSubmit` / `PostToolUse` / `Stop`) to agentrec's
//! signal wire format. Field shapes and the `apply_patch` path-extraction
//! rule are pinned by a live spike against Codex CLI 0.146.0 —
//! `docs/verify/codex-spike.md` is the reference; nothing here invents a
//! payload shape the spike didn't observe.
//!
//! Split out of `cmds.rs` (already the largest non-daemon CLI module, and
//! this is a fully self-contained feature — three event handlers, a private
//! `apply_patch` DSL parser, and an internal non-protocol scratch
//! accumulator) — the same convention `importcmd.rs`/`purgecmd.rs`/
//! `memorycmds.rs`/`initcmd.rs` already follow for `cmds.rs`.
//!
//! **Hard constraint (spike-confirmed, "`Stop` rejects plain-text stdout"):**
//! a `Stop` hook must emit valid JSON or NOTHING on stdout — non-empty
//! non-JSON stdout is reported as `Stop Failed`. This command therefore
//! writes NOTHING to stdout on any of the three events (no memory-injection
//! block, unlike `cmds::hook`'s Claude arm — wiring Codex into memory
//! injection is out of this task's scope, and doing so would risk exactly
//! the stdout output this constraint forbids). All diagnostics go to
//! stderr, via the `Result<(), String>` error path `main.rs` already prints
//! there on a nonzero exit.
//!
//! **Deliberately stricter than `cmds::hook`'s Claude arm.** Claude's hook
//! treats unparseable stdin as an empty JSON object and a missing
//! `hook_event_name` as an implicit `"Stop"` — silent degradation. Codex's
//! CI canary requirement calls for the opposite: every required field is
//! validated up front, and a violation is a loud `Err` before anything is
//! written to `signal.jsonl` or the scratch file. This is a considered
//! difference for this emitter, not an oversight or a claim that Claude's
//! arm should change.
//!
//! **`model` (C2 fix 2, founder-ratified): wired on `UserPromptSubmit`
//! only.** The spike's field inventory shows `model` present on all three
//! Codex events (`docs/verify/codex-spike.md`), but only
//! `UserPromptSubmit`'s copy is ever sent on the wire — mirroring
//! `prompt`'s existing start-only posture, not `emitter_turn`'s
//! carried-on-both-signals posture: `emitter_turn` is carried on both
//! because the DAEMON'S dedup/mismatch logic needs it at both ends (see
//! the fix-1 paragraph below); `model` is purely descriptive attribution
//! and needs no re-assertion at stop time. `daemon.rs::signal_context`
//! reads `sig.model` first, falling back to (Claude-only) transcript
//! parsing, and stamps `Recorder.models[session]` for whichever signal
//! resolves one — the same session-keyed, in-memory mechanism Claude's
//! transcript-derived model attribution already uses (Claude signals never
//! set `model`, so that path is unchanged). Residual, same class as an
//! existing accepted one: `Recorder.models` is in-memory, so a daemon
//! RESTART strictly between a session's `start` and `stop` loses the model
//! for that one turn — no different from a restart losing `prompt`.
//!
//! **C1 resend dedup and blocked continuations.** The spike's
//! block-continuation finding (`docs/verify/codex-spike.md`, "Continuation
//! semantics") measured `turn_id` staying IDENTICAL across a blocked
//! `Stop` firing twice for one turn (`stop_hook_active: false` then
//! `true`), and confirmed `UserPromptSubmit` never re-fires for the
//! synthetic continuation prompt. Both `Stop` firings therefore produce
//! the exact same C1 dedup key `(tool, event="stop", session,
//! emitter_turn)` in `daemon.rs::emitter_turn_dedup_key`. Every start/stop
//! invocation therefore also mints an `emitter_event` ULID: a separately-
//! fired continuation is distinguishable even when it lands in the same
//! millisecond with the same `files_written`, while an exact replay retains
//! the signal's original id. The daemon persists each Stop's declaration on
//! only the turn that Stop closes; a continuation appends its own turn and
//! never rewrites the first. See
//! `daemon.rs::tests::
//! handle_emitter_turn_signal_second_stop_with_new_files_written_is_applied_not_dropped`
//! and `handle_emitter_turn_signal_same_files_same_ms_new_event_is_a_continuation`.

use crate::cmds::wall_now_ms;
use crate::{agentrec_dir, signal_path};
use agentrec_core::record::{append_log_line, SignalEvent};
use agentrec_core::scrub;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// Internal per-turn accumulator entry. **NOT a wire format** — must never
/// appear in PROTOCOL.md, and nothing outside this module (in particular:
/// no daemon code) ever reads it. One entry per `PostToolUse` firing that
/// extracted at least a `tool_input.command` (even if extraction found zero
/// paths); `Stop` drains every entry matching its own `(session_id,
/// turn_id)` and unions their paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScratchEntry {
    ts_ms: u64,
    session_id: String,
    turn_id: String,
    paths: Vec<String>,
}

/// Entries older than this are dropped on every scratch rewrite (both an
/// accumulate and a drain trigger one — so garbage collection runs
/// regardless of which kind of event a still-live session happens to fire
/// next). Bounds growth from a Codex turn that fires `PostToolUse` and then
/// never reaches `Stop` — a crash, an interrupt (the spike confirmed
/// mid-turn `SIGINT` suppresses `Stop` entirely), or `/clear` all leave
/// scratch behind with no natural drain trigger.
const SCRATCH_TTL_MS: u64 = 24 * 60 * 60 * 1000;

fn scratch_path(root: &Path) -> PathBuf {
    agentrec_dir(root).join("codex-scratch.jsonl")
}

fn scratch_lock_path(root: &Path) -> PathBuf {
    agentrec_dir(root).join("codex-scratch.lock")
}

fn open_scratch_lock(root: &Path) -> Result<std::fs::File, String> {
    let path = scratch_lock_path(root);
    // Write-side fsguard mirror: a FIFO here blocks the open until a reader
    // appears, wedging the hook before the flock is even attempted.
    if agentrec_core::fsguard::is_nonregular(&path) {
        return Err(format!(
            "{} is not a regular file — refusing to lock",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        // Never truncate: this file's only role is to be `flock`'d, same
        // rationale as `loglock.rs`/`memlock.rs`.
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("cannot open lock file {}: {e}", path.display()))?;
    agentrec_core::perms::lock_file(&path);
    Ok(file)
}

/// Blocking `flock` around the whole read -> filter -> rewrite sequence, so
/// a `PostToolUse` accumulate can never race a `Stop` drain into losing or
/// duplicating an entry — same single-lock-domain posture as
/// `loglock.rs`/`memlock.rs`, scoped to this module because this scratch
/// file has exactly one writer type (this module; no other command touches
/// it), unlike `log.jsonl`/`memory.jsonl` which have several.
fn with_scratch_lock<T>(root: &Path, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let file = open_scratch_lock(root)?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        return Err(format!(
            "could not lock {}: {}",
            scratch_lock_path(root).display(),
            std::io::Error::last_os_error()
        ));
    }
    let result = f();
    drop(file); // releases the flock
    result
}

/// Best-effort read: a corrupt line is dropped, not fatal. This file is
/// internal state (not one of the three sanctioned append-only files), and
/// "scratch unreadable" already has a defined outcome downstream — a `Stop`
/// that finds nothing usable reports `files_written` absent, never an
/// empty-list fabrication.
fn read_scratch_entries(root: &Path) -> Vec<ScratchEntry> {
    let text =
        agentrec_core::fsguard::read_regular_to_string(&scratch_path(root)).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Atomically replace the scratch file's contents — tmp+lock_file+rename,
/// the same kill-9-safe pattern as `state.rs::write_state`: a crash
/// mid-write leaves either the untouched old file or the fully-written new
/// one, never a torn one, and the tmp name is unique per writing process so
/// two racing writers (already serialized by `with_scratch_lock` in this
/// module's own callers, but defensive regardless) can't clobber each
/// other's in-flight write.
fn write_scratch_entries(root: &Path, entries: &[ScratchEntry]) -> Result<(), String> {
    let path = scratch_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        agentrec_core::perms::lock_dir(parent);
    }
    let mut text = String::new();
    for entry in entries {
        let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        text.push_str(&line);
        text.push('\n');
    }
    let tmp = path.with_extension(format!("jsonl.tmp.{}", std::process::id()));
    // Write-side fsguard mirror: `fs::write` opens create+truncate, which
    // blocks forever on a FIFO pre-created at the tmp name.
    if agentrec_core::fsguard::is_nonregular(&tmp) {
        return Err(format!(
            "{} is not a regular file — refusing to write",
            tmp.display()
        ));
    }
    std::fs::write(&tmp, &text).map_err(|e| e.to_string())?;
    agentrec_core::perms::lock_file(&tmp);
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn is_expired(entry: &ScratchEntry, now_ms: u64) -> bool {
    now_ms.saturating_sub(entry.ts_ms) > SCRATCH_TTL_MS
}

/// `PostToolUse` (`apply_patch`): append one entry with this firing's
/// extracted paths (pruning expired entries in the same rewrite). Callers
/// (see `hook_codex`) must never call this with an empty `paths` — an
/// extraction that found nothing (e.g. a patch containing only the
/// unparsed `Move to:` op) must stay undeclared, not become a stored
/// `[]` that a later `Stop` would drain into an affirmative-but-false
/// "wrote nothing".
fn accumulate_scratch(
    root: &Path,
    session_id: &str,
    turn_id: &str,
    paths: Vec<String>,
    now_ms: u64,
) -> Result<(), String> {
    with_scratch_lock(root, || {
        let mut entries: Vec<ScratchEntry> = read_scratch_entries(root)
            .into_iter()
            .filter(|e| !is_expired(e, now_ms))
            .collect();
        entries.push(ScratchEntry {
            ts_ms: now_ms,
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            paths,
        });
        write_scratch_entries(root, &entries)
    })
}

/// `Stop`: drain every entry matching `(session_id, turn_id)` — removing
/// them from the file — and return the union of their paths in first-seen
/// order, deduped. `None` when zero entries matched: missing, already
/// drained (a prior `Stop` for this exact turn already ran), and
/// corrupt-and-dropped scratch are indistinguishable from each other by
/// design — all three mean "nothing to declare" to the caller. Because
/// `accumulate_scratch`'s only caller never stores an empty-`paths` entry
/// (see its doc comment), a non-empty `matching` set always yields a
/// non-empty union in practice; the `is_empty()` check below is kept as
/// defense-in-depth against that invariant, not because a real caller path
/// exercises it — the absent-vs-empty distinction on `files_written` must
/// hold even if a future caller breaks the invariant.
fn drain_scratch(
    root: &Path,
    session_id: &str,
    turn_id: &str,
    now_ms: u64,
) -> Result<Option<Vec<String>>, String> {
    with_scratch_lock(root, || {
        let entries = read_scratch_entries(root);
        let (matching, mut retained): (Vec<ScratchEntry>, Vec<ScratchEntry>) = entries
            .into_iter()
            .partition(|e| e.session_id == session_id && e.turn_id == turn_id);
        retained.retain(|e| !is_expired(e, now_ms));
        write_scratch_entries(root, &retained)?;
        if matching.is_empty() {
            return Ok(None);
        }
        let mut union = Vec::new();
        for entry in matching {
            for p in entry.paths {
                if !union.contains(&p) {
                    union.push(p);
                }
            }
        }
        if union.is_empty() {
            return Ok(None);
        }
        Ok(Some(union))
    })
}

/// Path-extraction rule (docs/verify/codex-spike.md, decision 17 —
/// confirmed live against Codex CLI 0.146.0): `tool_input.command` for an
/// `apply_patch` `PostToolUse` firing is raw `apply_patch` DSL text, not a
/// structured file list. Each line matching `*** (Add|Update|Delete) File:
/// <path>` contributes `<path>` (the rest of the line, verbatim). One
/// `PostToolUse` firing can bundle several ops (spike: one `Update` + two
/// `Add` in a single `tool_input.command`) — all of them are collected.
///
/// **Conservative on purpose:** `*** Move to: <path>` (renames) is
/// documented in the general `apply_patch` DSL elsewhere but was NOT
/// exercised live in the spike ("do not assume its exact syntax without a
/// run") and is deliberately NOT parsed here. A rename this misses is a
/// silent gap in `files_written` (undercount, never a wrong path), not a
/// correctness bug — closing it needs its own live probe first.
fn apply_patch_paths(command: &str) -> Vec<String> {
    const PREFIXES: [&str; 3] = ["*** Add File: ", "*** Update File: ", "*** Delete File: "];
    let mut paths = Vec::new();
    for line in command.lines() {
        for prefix in PREFIXES {
            if let Some(rest) = line.strip_prefix(prefix) {
                if !rest.is_empty() {
                    paths.push(rest.to_string());
                }
                break;
            }
        }
    }
    paths
}

/// `apply_patch` paths are relative to the payload's `cwd` (spike: every
/// captured `tool_input.command` path, e.g. `hello.txt`, has no leading
/// `/`). `SignalEvent::files_written` and the daemon's `resolve_declared` /
/// `normalize_declared` both document and require **absolute** paths
/// (`record.rs`: "absolute paths the emitting tool itself wrote";
/// `normalize_declared` lexically `strip_prefix`s each entry against the
/// repo root, which can never match a relative path — a relative entry
/// silently falls into its `out_of_root` bucket instead of ever reaching
/// `d.paths`). Joining against `cwd` here, before the path ever reaches
/// scratch or the wire, is what makes this producer's declarations
/// actually count instead of silently vanishing into `out_of_root`. A path
/// that already starts with `/` (not observed live, but not ruled out
/// either) is passed through unchanged rather than double-joined.
fn absolutize(cwd: &str, path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{}/{path}", cwd.trim_end_matches('/'))
    }
}

/// A required field, present and a non-empty string. Every violation here
/// is a loud `Err` — see the module doc comment for why this emitter does
/// NOT share Claude's tolerant-default posture.
fn require_str<'a>(payload: &'a serde_json::Value, field: &str) -> Result<&'a str, String> {
    payload
        .get(field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("codex hook: missing or empty required field {field:?}"))
}

fn append_signal(root: &Path, signal: &SignalEvent) -> Result<(), String> {
    let line = serde_json::to_string(signal).map_err(|e| e.to_string())?;
    append_log_line(&signal_path(root), &line)
}

/// `agentrec hook codex`: reads one hook payload from stdin, maps it to the
/// appropriate signal/scratch effect, and writes nothing to stdout. See the
/// module doc comment for the stdout and validation posture.
pub fn hook_codex(root: &Path) -> Result<(), String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| format!("codex hook: failed to read stdin: {e}"))?;
    let payload: serde_json::Value = serde_json::from_str(buf.trim())
        .map_err(|e| format!("codex hook: malformed JSON on stdin: {e}"))?;

    let event_name = require_str(&payload, "hook_event_name")?;
    let session_id = require_str(&payload, "session_id")?.to_string();
    let turn_id = require_str(&payload, "turn_id")?.to_string();
    let transcript = payload
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .map(String::from);
    let now_ms = wall_now_ms();

    match event_name {
        "UserPromptSubmit" => {
            let prompt_raw = require_str(&payload, "prompt")?;
            let prompt = scrub::scrub(prompt_raw);
            // C2 fix 2: `model` is present on every observed Codex event
            // (spike field inventory), but only the start signal carries it
            // on the wire — see the module doc's "model" paragraph for why
            // this mirrors `prompt`'s start-only posture rather than
            // `emitter_turn`'s carried-on-both one. Best-effort like
            // `transcript_path` above, not `require_str`: a payload missing
            // `model` should still open a bracket, just without model
            // attribution, not fail the whole hook.
            let model = payload
                .get("model")
                .and_then(|v| v.as_str())
                .map(String::from);
            let signal = SignalEvent {
                v: 1,
                ts: now_ms,
                tool: "codex".to_string(),
                event: Some("start".to_string()),
                session: Some(session_id),
                transcript,
                prompt: Some(prompt),
                files_written: None,
                emitter_turn: Some(turn_id),
                emitter_event: Some(agentrec_core::id::ulid()),
                model,
                kind: None,
                fact: None,
                pins: None,
            };
            append_signal(root, &signal)
        }
        "PostToolUse" => {
            let tool_name = require_str(&payload, "tool_name")?;
            if tool_name != "apply_patch" {
                // Forward-compatible no-op: only apply_patch has a
                // spike-confirmed extraction rule (decision 17). A future
                // Codex tool type firing PostToolUse is not a malformed
                // payload — it is simply not one this accumulator handles.
                return Ok(());
            }
            let command = payload
                .get("tool_input")
                .and_then(|v| v.get("command"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    "codex hook: PostToolUse(apply_patch) missing tool_input.command".to_string()
                })?;
            // Required (loud failure if absent), not best-effort: without
            // `cwd` the extracted paths can't be absolutized, and a
            // relative path silently fails to match in the daemon's
            // `normalize_declared` (see `absolutize`'s doc comment) —
            // shipping a signal we know will be silently ignored is worse
            // than refusing to emit it.
            let cwd = require_str(&payload, "cwd")?;
            let paths: Vec<String> = apply_patch_paths(command)
                .into_iter()
                .map(|p| absolutize(cwd, &p))
                .collect();
            if paths.is_empty() {
                // Extraction found no Add/Update/Delete headers (e.g. a
                // patch containing only the unparsed `Move to:` op).
                // Recording an empty entry here would let a later `Stop`
                // drain it into `files_written: []` — an affirmative
                // "wrote nothing" that would be false. Leaving no entry at
                // all keeps this turn's declaration status exactly what it
                // was before this firing.
                return Ok(());
            }
            accumulate_scratch(root, &session_id, &turn_id, paths, now_ms)
        }
        "Stop" => {
            let files_written = drain_scratch(root, &session_id, &turn_id, now_ms)?;
            let signal = SignalEvent {
                v: 1,
                ts: now_ms,
                tool: "codex".to_string(),
                event: Some("stop".to_string()),
                session: Some(session_id),
                transcript,
                prompt: None,
                files_written,
                emitter_turn: Some(turn_id),
                emitter_event: Some(agentrec_core::id::ulid()),
                // Not re-sent: the start signal already carried it (or
                // didn't) — see the module doc's "model" paragraph.
                model: None,
                kind: None,
                fact: None,
                pins: None,
            };
            append_signal(root, &signal)
        }
        other => Err(format!(
            "codex hook: unrecognized hook_event_name {other:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn apply_patch_paths_extracts_all_three_op_kinds() {
        let command = "*** Begin Patch\n*** Update File: hello.txt\n@@\n+line two\n*** Add File: second.txt\n+second file\n*** Add File: to_delete.txt\n+delete me\n*** End Patch";
        assert_eq!(
            apply_patch_paths(command),
            vec!["hello.txt", "second.txt", "to_delete.txt"]
        );
    }

    #[test]
    fn apply_patch_paths_delete_only() {
        let command = "*** Begin Patch\n*** Delete File: to_delete.txt\n*** End Patch";
        assert_eq!(apply_patch_paths(command), vec!["to_delete.txt"]);
    }

    #[test]
    fn apply_patch_paths_ignores_move_to_and_body_lines() {
        // "Move to:" (renames) is documented elsewhere but not spike-
        // confirmed here — deliberately not parsed. Body/context lines
        // (`@@`, `+...`) must never be mistaken for a header.
        let command = "*** Begin Patch\n*** Move to: new_name.txt\n@@\n+ *** Add File: not_really.txt\n*** End Patch";
        assert!(apply_patch_paths(command).is_empty());
    }

    #[test]
    fn absolutize_joins_relative_against_cwd() {
        assert_eq!(
            absolutize("/repo", "hello.txt"),
            "/repo/hello.txt".to_string()
        );
    }

    #[test]
    fn absolutize_tolerates_trailing_slash_on_cwd() {
        assert_eq!(
            absolutize("/repo/", "hello.txt"),
            "/repo/hello.txt".to_string()
        );
    }

    #[test]
    fn absolutize_passes_through_an_already_absolute_path() {
        assert_eq!(
            absolutize("/repo", "/elsewhere/hello.txt"),
            "/elsewhere/hello.txt".to_string()
        );
    }

    #[test]
    fn drain_scratch_none_when_nothing_accumulated() {
        let tmp = tmp_root();
        let out = drain_scratch(tmp.path(), "s1", "t1", 1_000).unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn accumulate_then_drain_unions_paths_and_removes_entries() {
        let tmp = tmp_root();
        accumulate_scratch(
            tmp.path(),
            "s1",
            "t1",
            vec!["a.txt".into(), "b.txt".into()],
            1_000,
        )
        .unwrap();
        accumulate_scratch(tmp.path(), "s1", "t1", vec!["b.txt".into()], 1_100).unwrap();
        // Different turn — must not be drained by t1's Stop.
        accumulate_scratch(tmp.path(), "s1", "t2", vec!["other.txt".into()], 1_050).unwrap();

        let drained = drain_scratch(tmp.path(), "s1", "t1", 2_000).unwrap();
        assert_eq!(
            drained,
            Some(vec!["a.txt".to_string(), "b.txt".to_string()])
        );

        // A second drain of the same turn finds nothing left.
        let second = drain_scratch(tmp.path(), "s1", "t1", 2_100).unwrap();
        assert_eq!(second, None);

        // The other turn's entry survived t1's drain untouched.
        let other = drain_scratch(tmp.path(), "s1", "t2", 2_200).unwrap();
        assert_eq!(other, Some(vec!["other.txt".to_string()]));
    }

    #[test]
    fn drain_scratch_gcs_expired_entries_of_other_turns() {
        let tmp = tmp_root();
        accumulate_scratch(tmp.path(), "stale", "old", vec!["x.txt".into()], 0).unwrap();
        // Draining an unrelated turn still prunes the expired sibling.
        let now = SCRATCH_TTL_MS + 2;
        let out = drain_scratch(tmp.path(), "s1", "t1", now).unwrap();
        assert_eq!(out, None);
        let stale = drain_scratch(tmp.path(), "stale", "old", now).unwrap();
        assert_eq!(
            stale, None,
            "expired entry must have been GC'd before this drain, not delivered late"
        );
    }

    #[cfg(unix)]
    fn mkfifo_at(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(
            unsafe { libc::mkfifo(c.as_ptr(), 0o600) },
            0,
            "fixture must actually create a fifo"
        );
    }

    /// `open_scratch_lock` opens `.agentrec/codex-scratch.lock` for WRITE
    /// before the flock is even attempted, so a FIFO there wedges the Codex
    /// hook process — and a hook that never returns stalls the whole agent
    /// turn, which is strictly worse than a lost signal.
    /// HANGS rather than fails on regression.
    #[test]
    #[cfg(unix)]
    fn open_scratch_lock_refuses_a_fifo_lock_instead_of_hanging() {
        let tmp = tmp_root();
        mkfifo_at(&scratch_lock_path(tmp.path()));

        let err = open_scratch_lock(tmp.path()).unwrap_err();
        assert!(
            err.contains("not a regular file"),
            "refusal must name the reason: {err}"
        );

        // ALLOW half: an ordinary lock path must still open.
        let ok = tmp_root();
        drop(open_scratch_lock(ok.path()).expect("an ordinary lock path must still open"));
    }

    /// `write_scratch_entries` stages through `codex-scratch.jsonl.tmp.<pid>`
    /// and `fs::write` opens create+truncate, which blocks on a FIFO planted
    /// at that name. Same wedged-hook consequence as the lock above.
    #[test]
    #[cfg(unix)]
    fn write_scratch_entries_refuses_a_fifo_tmp_instead_of_hanging() {
        let tmp = tmp_root();
        let tmp_path =
            scratch_path(tmp.path()).with_extension(format!("jsonl.tmp.{}", std::process::id()));
        mkfifo_at(&tmp_path);

        let err = write_scratch_entries(tmp.path(), &[]).unwrap_err();
        assert!(
            err.contains("not a regular file"),
            "refusal must name the reason: {err}"
        );
        assert!(
            !scratch_path(tmp.path()).exists(),
            "a refused write must not rename anything into place"
        );

        // ALLOW half: without the fifo the rewrite must still land.
        let ok = tmp_root();
        write_scratch_entries(ok.path(), &[]).expect("an ordinary tmp path must still write");
        assert!(scratch_path(ok.path()).exists());
    }
}
