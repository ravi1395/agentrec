//! `agentrec attest report [--json] [--range <base>..<head>]` — the
//! attestation bundle: every claim with its status, verdict chain, evidence
//! provenance and human notes.
//!
//! No wall-clock header is emitted, deliberately: the report over a fixed log
//! must render byte-identically on every run so a golden can pin it.
//!
//! `--range` is a TIME filter and says so in its own header line. It resolves
//! both revisions' committer timestamps and keeps claims with at least one
//! event inside that window. It does NOT establish that an event was caused by
//! a commit in the range — nothing in `attest.jsonl` records which commit an
//! `evidence` event's run was against, so a stronger claim would be a
//! fabrication.

use crate::attest::lock::read_attest;
use crate::attest::statuscmd::{provenance, status_label};
use agentrec_core::attest::events::ManualSeverity;
use agentrec_core::attest::fold::fold_claims;
use agentrec_core::attest::types::ClaimId;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// Committer timestamp of `rev`, in unix MILLISECONDS.
///
/// `git log -1 --format=%ct` reports SECONDS; `AttestEvent::ts` is
/// milliseconds, so the conversion happens here, at the boundary, once.
/// An unresolvable revision is detected from git's EXIT STATUS — an empty or
/// unparseable stdout would also be a failure, but exit status is what git
/// actually promises.
fn commit_time_ms(root: &Path, rev: &str) -> Result<u64, String> {
    // Per-site allow: the same form `replaycmd.rs` uses. This spawns `git`
    // from a CLI verb, never from the daemon (attest spec decision 6), and
    // the argument is a user-supplied revision passed as one argv element,
    // never through a shell.
    #[allow(clippy::disallowed_methods)]
    let out = Command::new("git")
        .args(["log", "-1", "--format=%ct", rev])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git in {}: {e}", root.display()))?;
    if !out.status.success() {
        // Two different user errors, two different fixes: a typo'd sha in a
        // real repo, versus running the verb somewhere that has no git
        // history at all. Reporting the second as "not a revision in this
        // repo" sends the reader hunting for a sha when the repo is what is
        // missing. git names the condition on stderr; nothing else in its
        // failure output says "not a git repository".
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("not a git repository") {
            return Err(format!(
                "--range needs git history, and {} is not a git repository",
                root.display()
            ));
        }
        return Err(format!("--range: not a revision in this repo: {rev}"));
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .map(|secs| secs * 1000)
        .map_err(|_| format!("--range: git gave no commit time for {rev}"))
}

/// Parse `base..head` strictly. A missing side, or the `...` symmetric form
/// (which means something else in git and would silently be read as a base of
/// `base.`), is an error rather than a guess.
fn parse_range(range: &str) -> Result<(&str, &str), String> {
    if range.contains("...") {
        return Err("--range takes <base>..<head>; the `...` form is not supported".into());
    }
    let Some((base, head)) = range.split_once("..") else {
        return Err(format!("--range must be <base>..<head>, got {range:?}"));
    };
    if base.is_empty() || head.is_empty() {
        return Err(format!("--range must name both ends, got {range:?}"));
    }
    Ok((base, head))
}

pub fn run(root: &Path, json: bool, range: Option<&str>) -> Result<(), String> {
    let (events, _census) = read_attest(root)?;

    let window = match range {
        Some(range) => {
            let (base, head) = parse_range(range)?;
            let lo = commit_time_ms(root, base)?;
            let hi = commit_time_ms(root, head)?;
            // A reversed range is refused, not silently normalized. Sorting
            // the pair would make `head..base` and `base..head` print the
            // same report under a header naming the order the user asked
            // for — a wrong question answered without complaint.
            if lo > hi {
                return Err(format!(
                    "--range <base>..<head>: base must not be later than head, but {base} commits after {head}"
                ));
            }
            Some((range.to_string(), lo, hi))
        }
        None => None,
    };

    let mut in_window: BTreeMap<ClaimId, bool> = BTreeMap::new();
    for event in &events {
        let keep = match &window {
            None => true,
            Some((_, lo, hi)) => event.ts() >= *lo && event.ts() <= *hi,
        };
        let entry = in_window.entry(event.claim_id().clone()).or_insert(false);
        *entry = *entry || keep;
    }

    let folded = fold_claims(&events);
    let prov = provenance(&events);

    let mut rows = Vec::new();
    for claim in folded.claims.values() {
        if !in_window.get(&claim.claim_id).copied().unwrap_or(false) {
            continue;
        }
        let p = prov.get(&claim.claim_id).cloned().unwrap_or_default();
        rows.push(serde_json::json!({
            "claim_id": claim.claim_id.to_string(),
            "identity": claim.test_identity.as_ref().map(|i| i.to_string()),
            "criterion": claim.manual_text,
            "severity": claim.manual_severity.map(|s| match s {
                ManualSeverity::Blocking => "blocking",
                ManualSeverity::Fyi => "fyi",
            }),
            "status": status_label(&claim.status),
            "stale": claim.is_stale(),
            "history": {
                "derives": claim.history.derives,
                "evidence": claim.history.evidence,
                "verdicts": claim.history.verdicts,
                "confirmed": claim.history.confirmed_verdicts,
                "claim_false": claim.history.claim_false_verdicts,
                "recipe_invalid": claim.history.recipe_invalid_verdicts,
                "flaky_observations": claim.history.flaky_observations,
                "stale_events": claim.history.stale_events,
                "human_answers": claim.history.human_answers,
            },
            "last_verdict": p.last_verdict,
            "replay_commit": p.replay_commit,
            "evidence_turn": p.turn_id,
            "output_blob": p.output_blob,
            "dirty": p.dirty,
            "human_note": claim.last_human_note,
        }));
    }

    if json {
        let payload = serde_json::json!({
            "range": window.as_ref().map(|(r, _, _)| r.clone()),
            "claims": rows,
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return Ok(());
    }

    println!("# attest report");
    println!();
    match &window {
        Some((range, _, _)) => println!(
            "Range `{range}`: claims with at least one event between the two commits' \
             committer timestamps. A time window, not a causal one."
        ),
        None => println!("Every claim in `.agentrec/attest.jsonl`."),
    }
    println!();
    println!("claims: {}", rows.len());
    for row in &rows {
        println!();
        println!("## {}", row["claim_id"].as_str().unwrap_or("?"));
        if let Some(identity) = row["identity"].as_str() {
            println!("- identity: {identity}");
        }
        if let Some(text) = row["criterion"].as_str() {
            println!(
                "- criterion ({}): {text}",
                row["severity"].as_str().unwrap_or("?")
            );
        }
        println!("- status: {}", row["status"].as_str().unwrap_or("?"));
        println!("- stale: {}", row["stale"]);
        let h = &row["history"];
        println!(
            "- history: {} derive(s), {} evidence, {} verdict(s) \
             (confirmed {}, claim-false {}, recipe-invalid {}, flaky {}), \
             {} stale event(s), {} human answer(s)",
            h["derives"],
            h["evidence"],
            h["verdicts"],
            h["confirmed"],
            h["claim_false"],
            h["recipe_invalid"],
            h["flaky_observations"],
            h["stale_events"],
            h["human_answers"]
        );
        // "appended", not "current": `CLAIM_FALSE` is permanent, so a later
        // `confirmed` verdict is recorded and counted but moves nothing. The
        // status line above is what the claim IS; this line is what was last
        // written, and the two legitimately disagree on a refuted claim.
        match (row["last_verdict"].as_str(), row["replay_commit"].as_str()) {
            (Some(v), Some(c)) => println!("- last verdict appended: {v} (replay {c})"),
            (Some(v), None) => println!("- last verdict appended: {v}"),
            _ => println!("- last verdict appended: none"),
        }
        match row["evidence_turn"].as_str() {
            Some(turn) => println!(
                "- evidence: turn {turn}, blob {}, dirty {}",
                row["output_blob"].as_str().unwrap_or("none"),
                row["dirty"]
            ),
            None => println!("- evidence: no turn joined"),
        }
        if let Some(note) = row["human_note"].as_str() {
            println!("- human note: {note}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn range_parsing_refuses_every_ambiguous_form() {
        assert_eq!(parse_range("a..b").unwrap(), ("a", "b"));
        for bad in ["a...b", "a..", "..b", "abc", "..", ""] {
            assert!(parse_range(bad).is_err(), "{bad:?} must be refused");
        }
    }
}
