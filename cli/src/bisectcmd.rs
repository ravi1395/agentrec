//! `bisect`: binary-search the turn sequence for the first turn whose probe
//! state fails a user-supplied command (Phase 3.0 T5, spec §3.0.4).
//!
//! All attribution logic lives in `agentrec_core::bisect`; this layer owns the
//! three things core deliberately does not: materializing a probe state into a
//! scratch directory, spawning the test command, and driving the search.
//!
//! **Read-only by construction.** The repository is opened, `log.jsonl` is
//! parsed, and CAS blobs are read. Nothing under the working tree or
//! `.agentrec/` is opened for writing anywhere in this file — every write goes
//! to a fresh directory under the system temp root, which is removed again
//! unless `--keep`. The integration test measures this by hashing both trees
//! before and after a full run rather than asserting it.
//!
//! **The test command is NOT sandboxed.** It runs through the user's shell
//! with the scratch directory as its cwd and full ambient authority — it can
//! reach the network, the real repository, and anything else the user can.
//! `--help` says so; see `main.rs`'s `Bisect` doc comment.

use agentrec_core::bisect::{
    self, BisectOptions, BisectResult, FileAction, ProbeState, Unanswerable, UnanswerableReason,
    Verdict, FIDELITY, INCLUDE_BARE_WARNING,
};
use agentrec_core::record::TurnRecord;
use agentrec_core::view::{self, LookupError, RepositoryView};
use std::path::{Path, PathBuf};

/// `agentrec bisect --test <cmd> [...]`.
#[allow(clippy::too_many_arguments)]
pub fn bisect(
    root: &Path,
    test_cmd: String,
    good: Option<String>,
    bad: Option<String>,
    include_bare: bool,
    flaky_retries: u32,
    keep: bool,
    json: bool,
) -> Result<(), String> {
    let view = RepositoryView::open(root).map_err(|e| e.to_string())?;
    let ledger = view.ledger();
    let opts = BisectOptions { include_bare };
    let seq = bisect::sequence(&ledger, &opts);
    if seq.is_empty() {
        return Err(
            "no turns to bisect (bare and imported turns are excluded; see --include-bare)"
                .to_string(),
        );
    }

    // Resolved against ALL turns, not against the sequence: that keeps
    // "unknown id" and "ambiguous id" distinguishable, and lets a ref that
    // resolves to a turn the sequence excluded say so instead of silently
    // snapping to a neighbour.
    let all_turns: Vec<&TurnRecord> = ledger
        .records
        .iter()
        .filter_map(|r| match r {
            agentrec_core::record::LogRecord::Turn(t) => Some(t),
            agentrec_core::record::LogRecord::Epoch(_) => None,
        })
        .collect();
    let good_idx = match good.as_deref() {
        None => None,
        Some(r) => Some(resolve_index(&all_turns, &seq, r, "--good")?),
    };
    let bad_idx = match bad.as_deref() {
        None => seq.len() - 1,
        Some(r) => resolve_index(&all_turns, &seq, r, "--bad")?,
    };
    let lo0: i64 = good_idx.map(|i| i as i64).unwrap_or(-1);
    if lo0 >= bad_idx as i64 {
        return Err(format!(
            "--good must name a turn older than --bad ({} is not before {})",
            seq[good_idx.unwrap_or(0)].id,
            seq[bad_idx].id
        ));
    }

    let mut driver = Driver {
        root,
        view: &view,
        seq: &seq,
        test_cmd: &test_cmd,
        flaky_retries,
        keep,
        unanswerable: Vec::new(),
        probes: 0,
        copy_skipped: 0,
    };
    let verdict = driver.search(lo0, bad_idx)?;

    let span_from = good_idx.map(|i| seq[i].ended.clone()).unwrap_or_default();
    let span_to = seq[bad_idx].ended.clone();
    let result = BisectResult {
        verdict,
        unanswerable: driver.unanswerable,
        gap_windows: bisect::gap_windows(&ledger.records, &span_from, &span_to),
        probes: driver.probes,
        copy_skipped: driver.copy_skipped,
        include_bare_warning: include_bare.then(|| INCLUDE_BARE_WARNING.to_string()),
        fidelity: FIDELITY.to_string(),
    };

    if json {
        println!(
            "{}",
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        );
    } else {
        print!("{}", render_text(&result));
    }
    Ok(())
}

/// Resolve a `--good`/`--bad` reference to a position in the searched
/// sequence, through the same ambiguity path `show`/`diff` use.
fn resolve_index(
    all_turns: &[&TurnRecord],
    seq: &[&TurnRecord],
    turn_ref: &str,
    flag: &str,
) -> Result<usize, String> {
    let turn = view::resolve_turn(all_turns, turn_ref).map_err(|e| match e {
        LookupError::NoTurns => format!("{flag} {turn_ref}: no turns recorded"),
        LookupError::Unknown => format!("{flag} {turn_ref}: unknown turn id"),
        LookupError::Ambiguous { matched } => {
            format!("{flag} {turn_ref}: ambiguous turn id — {matched} turns match")
        }
    })?;
    seq.iter().position(|t| t.id == turn.id).ok_or_else(|| {
        format!(
            "{flag} {turn_ref}: turn {} is not in the searched sequence \
(bare and imported turns are excluded; see --include-bare)",
            turn.id
        )
    })
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

struct Driver<'a> {
    root: &'a Path,
    view: &'a RepositoryView,
    seq: &'a [&'a TurnRecord],
    test_cmd: &'a str,
    flaky_retries: u32,
    keep: bool,
    unanswerable: Vec<Unanswerable>,
    probes: u64,
    copy_skipped: u64,
}

/// One probe's outcome.
enum Outcome {
    Good,
    Bad,
    /// The command was run and its runs disagreed.
    Flaky,
}

impl Driver<'_> {
    /// Binary search over probe points. `lo` is the last index known good
    /// (`-1` = the baseline, before the first turn); `hi` is an index known
    /// bad. Both endpoints are taken ON TRUST — bisect never verifies that the
    /// good state passes or the bad state fails, exactly as `git bisect`
    /// trusts the marks a user gives it. `--help` states this.
    fn search(&mut self, mut lo: i64, mut hi: usize) -> Result<Verdict, String> {
        // Hoisted out of the narrowing loop deliberately: whether a probe
        // point is answerable does not change as the endpoints move, so a
        // point already rejected must not be re-probed after a narrowing —
        // that would run the command twice at one point (inflating `probes`,
        // whose contract is "points at which the command was executed") and
        // push a second `FlakyDisagreement` entry for the same turn.
        let mut tried: Vec<usize> = Vec::new();
        loop {
            if hi as i64 - lo <= 1 {
                return Ok(Verdict::FirstBad {
                    turn_id: self.seq[hi].id.clone(),
                });
            }
            // Candidates are strictly between the endpoints.
            let verdict_at = loop {
                let Some(mid) = self.pick_candidate(lo, hi, &tried) else {
                    // Nothing between the endpoints can be probed: report the
                    // narrowest span the search did establish rather than
                    // guessing which of its turns is to blame.
                    return Ok(Verdict::AmbiguousSpan {
                        ids: self.seq[(lo + 1) as usize..=hi]
                            .iter()
                            .map(|t| t.id.clone())
                            .collect(),
                        reason: "every probe point inside this span was unanswerable".to_string(),
                    });
                };
                tried.push(mid);
                let state = match self.view.bisect_probe_state(self.seq, Some(mid)) {
                    Ok(s) => s,
                    Err(blocked) => {
                        self.record_unanswerable(blocked);
                        continue;
                    }
                };
                match self.run_probe(&state)? {
                    Outcome::Flaky => {
                        self.unanswerable.push(Unanswerable {
                            turn_id: self.seq[mid].id.clone(),
                            path: None,
                            reason: UnanswerableReason::FlakyDisagreement,
                        });
                        continue;
                    }
                    o => break (mid, o),
                }
            };
            match verdict_at {
                (mid, Outcome::Good) => lo = mid as i64,
                (mid, _) => hi = mid,
            }
        }
    }

    /// The untried candidate closest to the midpoint of `(lo, hi)`, or `None`
    /// when every candidate has been tried. Probing outward from the midpoint
    /// keeps the search logarithmic when states are answerable, and degrades
    /// to a scan only over the region that is not.
    fn pick_candidate(&self, lo: i64, hi: usize, tried: &[usize]) -> Option<usize> {
        let first = (lo + 1) as usize;
        let last = hi - 1;
        let mid = first + (last - first) / 2;
        (0..=(last - first)).find_map(|d| {
            [mid.checked_add(d), mid.checked_sub(d)]
                .into_iter()
                .flatten()
                .find(|c| (first..=last).contains(c) && !tried.contains(c))
        })
    }

    /// Record obstructions once each: the same evicted blob blocks every probe
    /// that subtracts its turn, and repeating it would bury the report.
    fn record_unanswerable(&mut self, blocked: Vec<Unanswerable>) {
        for u in blocked {
            if !self.unanswerable.contains(&u) {
                self.unanswerable.push(u);
            }
        }
    }

    /// Materialize `state` into a fresh scratch directory, run the test
    /// command there, and clean up.
    fn run_probe(&mut self, state: &ProbeState) -> Result<Outcome, String> {
        let scratch = scratch_dir(self.probes)?;
        let skipped = materialize(self.root, &scratch, state)?;
        self.copy_skipped = self.copy_skipped.max(skipped);
        self.probes += 1;

        let mut verdicts = Vec::new();
        for _ in 0..=self.flaky_retries {
            verdicts.push(run_test(self.test_cmd, &scratch)?);
        }
        if self.keep {
            eprintln!("agentrec: kept probe scratch dir {}", scratch.display());
        } else {
            let _ = std::fs::remove_dir_all(&scratch);
        }
        let first = verdicts[0];
        if verdicts.iter().any(|&v| v != first) {
            return Ok(Outcome::Flaky);
        }
        Ok(if first { Outcome::Good } else { Outcome::Bad })
    }
}

/// Run the command with cwd = `dir`. `true` = exit 0 = good.
///
/// Spawned through `sh -c` so `--test 'cargo test && ./check.sh'` behaves the
/// way a user typing it into a terminal expects. This is the unsandboxed
/// surface `--help` discloses.
fn run_test(cmd: &str, dir: &Path) -> Result<bool, String> {
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("could not run --test command: {e}"))?;
    Ok(status.success())
}

/// A fresh, private directory under the system temp root.
fn scratch_dir(n: u64) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!(
        "agentrec-bisect-{}-{}-{}",
        std::process::id(),
        n,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create scratch dir: {e}"))?;
    Ok(dir)
}

/// Copy the working tree into `scratch`, then apply the probe state's edits.
/// Returns how many working-tree entries could not be reproduced.
fn materialize(root: &Path, scratch: &Path, state: &ProbeState) -> Result<u64, String> {
    let skipped = copy_tree(root, scratch)?;
    for (path, action) in &state.files {
        // `path` passed core's lexical containment check, so this join
        // provably stays inside `scratch`.
        let target = scratch.join(path);
        match action {
            FileAction::Delete => {
                let _ = std::fs::remove_file(&target);
            }
            FileAction::Write(bytes) => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("scratch mkdir {}: {e}", parent.display()))?;
                }
                std::fs::write(&target, bytes)
                    .map_err(|e| format!("scratch write {}: {e}", target.display()))?;
            }
        }
    }
    Ok(skipped)
}

/// Recursive copy of ordinary files and directories.
///
/// `.agentrec` is deliberately NOT copied: it is the recorder's own store
/// (multi-GiB on a long-lived repo), never part of the product under test, and
/// copying it would make every probe pay for the whole history. `.git` IS
/// copied — a test command may legitimately depend on being in a repository.
///
/// Anything that is not a regular file or directory (symlink, socket, fifo) is
/// skipped and COUNTED, never silently dropped: a probe run against a tree
/// missing them may fail for reasons that have nothing to do with any turn.
/// The materializer never recreating symlinks is also what makes core's
/// lexical path check sufficient.
fn copy_tree(src: &Path, dst: &Path) -> Result<u64, String> {
    let mut skipped = 0u64;
    let entries = std::fs::read_dir(src).map_err(|e| format!("read dir {}: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read dir {}: {e}", src.display()))?;
        let name = entry.file_name();
        if name == ".agentrec" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let meta = std::fs::symlink_metadata(&from)
            .map_err(|e| format!("stat {}: {e}", from.display()))?;
        if meta.file_type().is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| format!("mkdir {}: {e}", to.display()))?;
            skipped += copy_tree(&from, &to)?;
        } else if meta.file_type().is_file() {
            // fsguard: the working tree is user-controlled, so the read goes
            // through the guarded helper rather than `fs::read`.
            let bytes = agentrec_core::fsguard::read_regular(&from)
                .map_err(|e| format!("read {}: {e}", from.display()))?;
            std::fs::write(&to, bytes).map_err(|e| format!("write {}: {e}", to.display()))?;
        } else {
            skipped += 1;
        }
    }
    Ok(skipped)
}

// ---------------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------------

fn reason_text(r: UnanswerableReason) -> &'static str {
    match r {
        UnanswerableReason::SkippedEntry => "snapshot skipped (over cap)",
        UnanswerableReason::WithheldEntry => "withheld (secret-pattern file)",
        UnanswerableReason::MissingBefore => "no recorded before state",
        UnanswerableReason::DanglingBlob => "before snapshot no longer in the store",
        UnanswerableReason::SynthesizedAfter => "derived (imported) bytes, not observed",
        UnanswerableReason::NonRegularEntry => "not an ordinary file",
        UnanswerableReason::UnsafePath => "recorded path is not repo-relative",
        UnanswerableReason::FlakyDisagreement => "test command disagreed with itself",
    }
}

fn render_text(r: &BisectResult) -> String {
    let mut out = String::new();
    if let Some(w) = &r.include_bare_warning {
        out.push_str(&format!("warning: {w}\n"));
    }
    match &r.verdict {
        Verdict::FirstBad { turn_id } => {
            out.push_str(&format!("first bad turn: {turn_id}\n"));
        }
        Verdict::AmbiguousSpan { ids, reason } => {
            out.push_str(&format!("ambiguous span ({reason}):\n"));
            for id in ids {
                out.push_str(&format!("  {id}\n"));
            }
        }
    }
    out.push_str(&format!("probes: {}\n", r.probes));
    if r.copy_skipped > 0 {
        out.push_str(&format!(
            "{} working-tree entr{} could not be copied into the probe state (not ordinary files)\n",
            r.copy_skipped,
            if r.copy_skipped == 1 { "y" } else { "ies" }
        ));
    }
    for u in &r.unanswerable {
        out.push_str(&format!(
            "unanswerable: {} {}{}\n",
            u.turn_id,
            reason_text(u.reason),
            u.path
                .as_deref()
                .map(|p| format!(" ({p})"))
                .unwrap_or_default()
        ));
    }
    for g in &r.gap_windows {
        out.push_str(&format!(
            "recording gap ({}) since {} — edits in that window are unrecorded\n",
            g.kind, g.since
        ));
    }
    out.push_str(&format!("{FIDELITY}\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentrec_core::bisect::GapWindow;

    fn r(verdict: Verdict) -> BisectResult {
        BisectResult {
            verdict,
            unanswerable: vec![],
            gap_windows: vec![],
            probes: 3,
            copy_skipped: 0,
            include_bare_warning: None,
            fidelity: FIDELITY.to_string(),
        }
    }

    /// The fidelity sentence is present in EVERY text render — the spec makes
    /// any output that lets a probe state read as a snapshot a defect.
    #[test]
    fn every_text_render_carries_the_fidelity_sentence() {
        let first_bad = render_text(&r(Verdict::FirstBad {
            turn_id: "t2".to_string(),
        }));
        assert!(first_bad.contains("first bad turn: t2"), "{first_bad}");
        assert!(first_bad.contains(FIDELITY), "{first_bad}");

        let ambiguous = render_text(&r(Verdict::AmbiguousSpan {
            ids: vec!["t2".to_string(), "t3".to_string()],
            reason: "why".to_string(),
        }));
        assert!(ambiguous.contains("ambiguous span (why)"), "{ambiguous}");
        assert!(ambiguous.contains("  t2\n  t3\n"), "{ambiguous}");
        assert!(ambiguous.contains(FIDELITY), "{ambiguous}");
    }

    #[test]
    fn gap_and_unanswerable_lines_render() {
        let mut res = r(Verdict::FirstBad {
            turn_id: "t2".to_string(),
        });
        res.gap_windows = vec![GapWindow {
            since: "2026-01-01T00:04:00.000Z".to_string(),
            kind: "restart".to_string(),
        }];
        res.unanswerable = vec![Unanswerable {
            turn_id: "t9".to_string(),
            path: Some("a.txt".to_string()),
            reason: UnanswerableReason::DanglingBlob,
        }];
        let text = render_text(&res);
        assert!(
            text.contains("recording gap (restart) since 2026-01-01T00:04:00.000Z"),
            "{text}"
        );
        assert!(
            text.contains("unanswerable: t9 before snapshot no longer in the store (a.txt)"),
            "{text}"
        );
    }
}
