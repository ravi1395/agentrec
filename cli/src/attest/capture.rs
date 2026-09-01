//! Passive evidence capture — the two paths the plan's Phase 3 defines, both
//! ending in [`write_evidence`] so they cannot drift apart.
//!
//! 1. **The explicit wrapper**, [`run_wrapped`] (`agentrec attest run -- <cmd>`):
//!    spawns the command with the user's own stdin, TEES its output to the
//!    terminal, and exits with the child's status. Wrapping a command changes
//!    nothing the user or a downstream script sees.
//! 2. **The hook path**, [`capture_from_hook_payload`]: a `PostToolUse` `Bash`
//!    firing whose command is a test runner. No new ceremony step — it rides
//!    the hook that already fires for every tool call.
//!
//! Evidence is only ever written for a test some `derive` already knows
//! ([`FoldResult::claim_for`]). A result for an unknown test is COUNTED and
//! reported on stderr, never attached to a claim that does not exist and never
//! silently dropped.

use crate::attest::adapter_cargo::{bulk_results, parse_libtest, section_targets};
use crate::attest::lock::{append_attest_locked, read_attest};
use agentrec_core::attest::events::AttestEvent;
use agentrec_core::attest::fold::fold_claims;
use agentrec_core::attest::types::StructuredResult;
use agentrec_core::store::{BlobStore, PutResult};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// What one capture did, for the caller's own reporting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureReport {
    pub written: usize,
    /// Results for tests no `derive` knows.
    pub skipped_undeclared: usize,
    /// The captured output exceeded the CAS snapshot cap, so no blob was kept.
    pub blob_over_cap: bool,
}

/// The single evidence writer both capture paths call.
///
/// `raw` (stdout + stderr) goes into the CAS; the hash is referenced by every
/// event this call writes, and is additionally set on the `StructuredResult` of
/// a `parse_failed` result — a fail-closed capture is exactly the case where
/// the raw output is the only trustworthy artifact.
///
/// An over-cap blob is a NOTE, never a failure: losing the raw text is worse
/// than losing nothing, but losing the evidence too is worse still.
pub fn write_evidence(
    root: &Path,
    results: &[StructuredResult],
    raw: &str,
    turn_id: Option<String>,
    dirty: bool,
) -> Result<CaptureReport, String> {
    let (events, _) = read_attest(root)?;
    let folded = fold_claims(&events);

    let mut report = CaptureReport::default();
    let blob = match BlobStore::new(crate::objects_dir(root)).put_result(raw.as_bytes()) {
        PutResult::Stored { hash, .. } => Some(hash),
        PutResult::OverCap => {
            report.blob_over_cap = true;
            None
        }
        PutResult::IoError(e) => {
            eprintln!("agentrec attest: could not retain captured output: {e}");
            None
        }
    };

    let ts = crate::cmds::wall_now_ms();
    let mut batch = Vec::new();
    for result in results {
        let Some(claim_id) = folded.claim_for(&result.identity) else {
            report.skipped_undeclared += 1;
            continue;
        };
        let mut result = result.clone();
        if result.parse_failed {
            result.raw_blob = blob.clone();
        }
        batch.push(AttestEvent::Evidence {
            ts,
            claim_id: claim_id.clone(),
            turn_id: turn_id.clone(),
            dirty,
            output_blob: blob.clone(),
            result,
        });
    }
    report.written = batch.len();
    append_attest_locked(root, &batch)?;
    Ok(report)
}

/// The open turn's id, read from the daemon's persisted mirror
/// `.agentrec/open.json`.
///
/// A short-lived CLI process cannot ask `TurnEngine` — that is live in-process
/// daemon state, and this repo's architecture invariant is that consumers never
/// need the daemon running. Only the `id` field is read, so `OrphanJournal`
/// stays private to `daemon.rs`. A missing or unreadable journal means "no open
/// turn", which is a normal state (the dev-loop-only path), not a failure.
pub fn open_turn_id(root: &Path) -> Option<String> {
    let text = agentrec_core::fsguard::read_regular_to_string(
        &crate::agentrec_dir(root).join("open.json"),
    )
    .ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("id")?.as_str().map(String::from)
}

/// Whether the working tree had uncommitted changes.
///
/// A tree that is not a git repo at all counts as DIRTY: "we cannot show this
/// ran against a known commit" is the property the dev-loop-only flag records,
/// and a non-repo satisfies it just as an edited tree does.
pub fn tree_is_dirty(root: &Path) -> bool {
    // attest: sanctioned spawn (Phase 4 census)
    let Ok(out) = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
    else {
        return true;
    };
    if !out.status.success() {
        return true;
    }
    !out.stdout.is_empty()
}

/// Does this shell command invoke a test runner?
///
/// Deliberately narrow (`cargo test` only — the adapter this phase ships) and
/// anchored at the start of a command SEGMENT, so `echo cargo test` and a path
/// merely containing the word are not captured. A `+toolchain` argument is
/// stepped over because `cargo +nightly test` is the same invocation.
pub fn is_test_runner_command(command: &str) -> bool {
    for segment in command.split(['\n', ';']).flat_map(|s| s.split("&&")) {
        let segment = segment.replace("||", " ; ");
        for part in segment.split(';') {
            let mut tokens = part.split_whitespace();
            let Some(first) = tokens.next() else { continue };
            let is_cargo = first == "cargo" || first.ends_with("/cargo");
            if !is_cargo {
                continue;
            }
            // Skip a `+toolchain` selector, then the next token must be `test`.
            let next = tokens.find(|t| !t.starts_with('+'));
            if next == Some("test") {
                return true;
            }
        }
    }
    false
}

/// `agentrec attest run -- <cmd...>`: run the command, show the user its output
/// unchanged, capture evidence, and exit with the child's own status.
///
/// Returns the exit code to propagate. A child killed by a signal has no code
/// (`status.code()` is `None`); that is reported as **1**, and the evidence is
/// written BEFORE the code is returned so a killed run still leaves a record.
pub fn run_wrapped(root: &Path, cmd: &[String]) -> Result<i32, String> {
    let Some((program, args)) = cmd.split_first() else {
        return Err("attest run: no command given (usage: attest run -- <cmd>)".to_string());
    };

    // attest: sanctioned spawn (Phase 4 census)
    let mut child = Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("attest run: cannot spawn {program}: {e}"))?;

    let out_pipe = child.stdout.take().expect("piped");
    let err_pipe = child.stderr.take().expect("piped");
    let out_thread = std::thread::spawn(move || tee(out_pipe, false));
    let err_thread = std::thread::spawn(move || tee(err_pipe, true));
    let status = child
        .wait()
        .map_err(|e| format!("attest run: waiting for {program}: {e}"))?;
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();

    let report = capture_output(root, &stdout, &stderr)?;
    report_to_stderr(&report);
    Ok(status.code().unwrap_or(1))
}

/// Copy a child pipe to our own, line by line, while keeping a copy. The user
/// sees the test output as it happens; the copy is what gets parsed and stored.
fn tee(pipe: impl std::io::Read, to_stderr: bool) -> String {
    let mut kept = String::new();
    let mut reader = BufReader::new(pipe);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if to_stderr {
            let _ = std::io::stderr().write_all(&buf);
            let _ = std::io::stderr().flush();
        } else {
            let _ = std::io::stdout().write_all(&buf);
            let _ = std::io::stdout().flush();
        }
        kept.push_str(&String::from_utf8_lossy(&buf));
    }
    kept
}

/// Parse a captured `cargo test` run and write its evidence. Shared by both
/// capture paths — the wrapper and the hook — so the two cannot produce
/// different event shapes for the same run.
pub fn capture_output(root: &Path, stdout: &str, stderr: &str) -> Result<CaptureReport, String> {
    let sections = parse_libtest(stdout);
    let targets = section_targets(stderr, sections.len());
    let results = bulk_results(stdout, &targets);
    write_evidence(
        root,
        &results,
        &format!("{stdout}{stderr}"),
        open_turn_id(root),
        tree_is_dirty(root),
    )
}

fn report_to_stderr(report: &CaptureReport) {
    if report.skipped_undeclared > 0 {
        eprintln!(
            "agentrec attest: {} result(s) for undeclared tests skipped — run attest derive",
            report.skipped_undeclared
        );
    }
    if report.blob_over_cap {
        eprintln!("agentrec attest: captured output exceeded the snapshot cap; no blob retained");
    }
}

/// Hook path (capture path 2): a `PostToolUse` `Bash` firing whose command is a
/// test runner. Returns `true` when this payload was captured, so the caller
/// knows not to fall through to the ordinary signal path.
///
/// **Field names.** `tool_input.command`, `tool_response.stdout` and
/// `tool_response.stderr` are Claude Code's documented `PostToolUse` `Bash`
/// shape. No captured payload of that shape exists under `docs/fixtures/` (the
/// committed `PostToolUse` fixtures are all Codex `apply_patch`), so unlike the
/// Codex adapter this is a DOCUMENTED-name assumption, not a live-probed one.
/// A payload missing those fields is simply not captured.
pub fn capture_from_hook_payload(root: &Path, payload: &serde_json::Value) -> bool {
    if payload.get("tool_name").and_then(|t| t.as_str()) != Some("Bash") {
        return false;
    }
    let Some(command) = payload
        .get("tool_input")
        .and_then(|i| i.get("command"))
        .and_then(|c| c.as_str())
    else {
        return false;
    };
    if !is_test_runner_command(command) {
        return false;
    }
    let response = payload.get("tool_response");
    let field = |name: &str| {
        response
            .and_then(|r| r.get(name))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let (stdout, stderr) = (field("stdout"), field("stderr"));
    match capture_output(root, &stdout, &stderr) {
        Ok(report) => report_to_stderr(&report),
        // Fail open, loudly: a hook must never break the agent's turn.
        Err(e) => eprintln!("agentrec attest: hook capture failed: {e}"),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC-ATTEST-P3-21.
    #[test]
    fn ac_p3_21_the_test_runner_matcher_accepts_only_cargo_test() {
        for yes in [
            "cargo test",
            "cargo test --workspace -- --test-threads=3",
            "cargo +nightly test",
            "/usr/bin/cargo test -p agentrec",
            "cd repo && cargo test",
            "make build; cargo test",
        ] {
            assert!(is_test_runner_command(yes), "must match: {yes}");
        }
        for no in [
            "cargo testfoo",
            "cargo build",
            "cargo test-runner",
            "echo cargo test",
            "ls /opt/cargo test",
            "npm test",
            "",
        ] {
            assert!(!is_test_runner_command(no), "must not match: {no}");
        }
    }

    #[test]
    fn a_missing_open_json_is_no_open_turn_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(open_turn_id(tmp.path()), None);
        std::fs::create_dir_all(tmp.path().join(".agentrec")).unwrap();
        std::fs::write(tmp.path().join(".agentrec/open.json"), "{ not json").unwrap();
        assert_eq!(open_turn_id(tmp.path()), None);
        std::fs::write(
            tmp.path().join(".agentrec/open.json"),
            r#"{"id":"t_01ARZ3NDEKTSV4RRFFQ69G5FAV","root":"/x"}"#,
        )
        .unwrap();
        assert_eq!(
            open_turn_id(tmp.path()).as_deref(),
            Some("t_01ARZ3NDEKTSV4RRFFQ69G5FAV")
        );
    }

    #[test]
    fn a_non_git_tree_counts_as_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(tree_is_dirty(tmp.path()));
    }
}
