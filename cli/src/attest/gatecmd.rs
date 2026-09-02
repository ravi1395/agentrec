//! `agentrec attest gate [--json]` — the release gate over `attest.jsonl`.
//!
//! The blocking rule is stated ONCE, in `ATTEST-FORMAT.md` § "Gate blocking,
//! precisely", and implemented once, in
//! `crate::attest::statuscmd::blocking_reason`. Nothing here restates it.
//!
//! Everything that is not blocking is rendered the way `doctorcmd::Check::
//! advisory` renders a non-blocking finding: printed with its reason, never
//! contributing to the exit code. `attest gate` returns a bool for `main` to
//! turn into `exit(1)`, following `doctor`'s precedent — a failing gate is a
//! verdict this command printed, not a command error, so no
//! `agentrec: <message>` line is prefixed.

use crate::attest::lock::read_attest;
use crate::attest::statuscmd::{blocking_reason, cause_label};
use agentrec_core::attest::events::ManualSeverity;
use agentrec_core::attest::fold::{fold_claims, ClaimStatus};
use std::path::Path;

struct Item {
    claim_id: String,
    reason: String,
    text: Option<String>,
}

/// Returns `true` when the gate passes.
pub fn run(root: &Path, json: bool) -> Result<bool, String> {
    let (events, _census) = read_attest(root)?;
    let folded = fold_claims(&events);

    let mut blocking: Vec<Item> = Vec::new();
    let mut advisory: Vec<Item> = Vec::new();

    for claim in folded.claims.values() {
        // `fyi` manual claims are skipped BEFORE any branch below, so they
        // reach neither list by any route. Guarding only the manual note
        // was not enough: a `stale` event on an `fyi` claim still produced an
        // advisory line, which contradicts `ATTEST-FORMAT.md`'s "visible in
        // `attest report` and in `attest status`'s counts, and nowhere else".
        if claim.manual_severity == Some(ManualSeverity::Fyi) {
            continue;
        }
        let id = claim.claim_id.to_string();
        if let Some(reason) = blocking_reason(claim) {
            blocking.push(Item {
                claim_id: id,
                reason,
                text: claim.manual_text.clone(),
            });
            continue;
        }
        // Advisory conditions. A claim can carry more than one (a stale
        // recipe-invalid claim), and each is surfaced on its own line — the
        // one-per-claim rule applies to the BLOCKING count, not to notes.
        let mut notes: Vec<String> = Vec::new();
        match &claim.status {
            ClaimStatus::RecipeInvalid { cause } => notes.push(format!(
                "recipe-invalid({}): unverifiable this run, never blocks",
                cause_label(cause)
            )),
            ClaimStatus::Flaky => {
                notes.push("flaky: replays disagreed, tracked but never blocks".to_string())
            }
            _ => {}
        }
        if claim.is_stale() {
            notes.push("stale: a write landed in this claim's coverage scope".to_string());
        }
        for reason in notes {
            advisory.push(Item {
                claim_id: id.clone(),
                reason,
                text: claim.manual_text.clone(),
            });
        }
    }

    let pass = blocking.is_empty();

    if json {
        let render = |items: &[Item]| {
            items
                .iter()
                .map(|i| {
                    serde_json::json!({
                        "claim_id": i.claim_id,
                        "reason": i.reason,
                        "text": i.text,
                    })
                })
                .collect::<Vec<_>>()
        };
        let payload = serde_json::json!({
            "blocking": render(&blocking),
            "advisory": render(&advisory),
            "pass": pass,
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return Ok(pass);
    }

    if !blocking.is_empty() {
        println!("blocking:");
        for item in &blocking {
            match &item.text {
                Some(text) => println!("  {} {} — {text}", item.claim_id, item.reason),
                None => println!("  {} {}", item.claim_id, item.reason),
            }
        }
    }
    if !advisory.is_empty() {
        println!("advisory (never blocks):");
        for item in &advisory {
            println!("  {} {}", item.claim_id, item.reason);
        }
    }
    println!(
        "attest gate: {} ({} blocking)",
        if pass { "PASS" } else { "FAIL" },
        blocking.len()
    );
    Ok(pass)
}
