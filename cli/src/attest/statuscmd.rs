//! `agentrec attest status [--json]` — the read-only view over
//! `.agentrec/attest.jsonl`: claim counts by status, the `STALE` overlay
//! count, and the dev-loop-only figure.
//!
//! Minimal by design (plan Phase 3). Richer rendering — surfacing
//! `recipe-invalid` causes and manual-card severity — needs Phase 4's verdict
//! writer and is an extension of this same file, not a new one.

use crate::attest::lock::read_attest;
use agentrec_core::attest::events::AttestEvent;
use agentrec_core::attest::fold::{fold_claims, ClaimStatus};
use agentrec_core::attest::types::ClaimId;
use std::collections::BTreeMap;
use std::path::Path;

/// Claim counts, one field per `ClaimStatus` variant in the order they are
/// rendered.
///
/// A hand-rolled struct rather than a map keyed on `ClaimStatus`, because that
/// type is `Clone + Copy + Debug + PartialEq + Eq` and deliberately carries no
/// `Ord`/`Hash` (it lives in `agentrec-core`, which this phase does not
/// change). The two parameterized variants are collapsed to one counter each:
/// `RecipeInvalid { cause }` and `Human { answer }` are counted by variant,
/// not by payload. That is the plain reading of "counts by status", and
/// breaking `recipe-invalid` out by cause is Phase 4/5 work that needs the
/// verdict writer to exist first.
#[derive(Debug, Default, PartialEq, Eq)]
struct StatusCounts {
    derived: usize,
    declared: usize,
    evidenced: usize,
    confirmed: usize,
    claim_false: usize,
    recipe_invalid: usize,
    flaky: usize,
    human: usize,
}

impl StatusCounts {
    fn tally(&mut self, status: &ClaimStatus) {
        match status {
            ClaimStatus::Derived => self.derived += 1,
            ClaimStatus::Declared => self.declared += 1,
            ClaimStatus::Evidenced => self.evidenced += 1,
            ClaimStatus::Confirmed => self.confirmed += 1,
            ClaimStatus::ClaimFalse => self.claim_false += 1,
            ClaimStatus::RecipeInvalid { .. } => self.recipe_invalid += 1,
            ClaimStatus::Flaky => self.flaky += 1,
            ClaimStatus::Human { .. } => self.human += 1,
        }
    }

    /// Rendering order, fixed so the output is stable across runs.
    fn rows(&self) -> [(&'static str, usize); 8] {
        [
            ("DERIVED", self.derived),
            ("DECLARED", self.declared),
            ("EVIDENCED", self.evidenced),
            ("CONFIRMED", self.confirmed),
            ("CLAIM_FALSE", self.claim_false),
            ("RECIPE_INVALID", self.recipe_invalid),
            ("FLAKY", self.flaky),
            ("HUMAN", self.human),
        ]
    }
}

/// How many claims' LATEST `evidence` event carried `dirty: true`.
///
/// Counted from the event stream, not from the fold: `ClaimState` carries no
/// dirty field (it holds status, identity, body hash, the stale overlay,
/// manual text/severity, history counters and `last_ts`), and this phase may
/// not add one — `agentrec-core` is out of scope here.
///
/// "Latest" is APPEND ORDER, not the largest `ts`, matching what the fold
/// itself does (`entry.last_ts = ts`, assigned unconditionally per event).
/// A log whose timestamps go backwards is a writer bug; both readers agree on
/// the same interpretation of it rather than diverging.
fn dirty_claims(events: &[AttestEvent]) -> usize {
    let mut latest: BTreeMap<&ClaimId, bool> = BTreeMap::new();
    for event in events {
        if let AttestEvent::Evidence {
            claim_id, dirty, ..
        } = event
        {
            latest.insert(claim_id, *dirty);
        }
    }
    latest.values().filter(|dirty| **dirty).count()
}

pub fn run(root: &Path, json: bool) -> Result<(), String> {
    let (events, census) = read_attest(root)?;
    let folded = fold_claims(&events);

    let mut counts = StatusCounts::default();
    let mut stale = 0usize;
    for claim in folded.claims.values() {
        counts.tally(&claim.status);
        if claim.is_stale() {
            stale += 1;
        }
    }
    let dirty = dirty_claims(&events);
    let total = folded.claims.len();

    if json {
        let payload = serde_json::json!({
            "claims": total,
            "derived": counts.derived,
            "declared": counts.declared,
            "evidenced": counts.evidenced,
            "confirmed": counts.confirmed,
            "claim_false": counts.claim_false,
            "recipe_invalid": counts.recipe_invalid,
            "flaky": counts.flaky,
            "human": counts.human,
            "stale": stale,
            "dev_loop_only": dirty,
            "unparsed_lines": census.unparsed_lines,
            "unknown_kind_lines": census.unknown_kind_lines,
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return Ok(());
    }

    println!("claims: {total}");
    for (label, n) in counts.rows() {
        println!("  {label}: {n}");
    }
    println!("stale (overlay): {stale}");
    println!("dev-loop-only (dirty tree): {dirty}");
    if census.unparsed_lines > 0 {
        println!("{} unparsed line(s)", census.unparsed_lines);
    }
    if census.unknown_kind_lines > 0 {
        println!("{} unknown-kind line(s)", census.unknown_kind_lines);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentrec_core::attest::types::{StructuredResult, TestIdentity, TestOutcome};

    fn id(n: u64) -> ClaimId {
        ClaimId::mint_with(n, &[0u8; 10])
    }

    fn evidence(claim_id: ClaimId, ts: u64, dirty: bool) -> AttestEvent {
        AttestEvent::Evidence {
            ts,
            claim_id,
            turn_id: None,
            dirty,
            output_blob: None,
            result: StructuredResult::outcome(TestIdentity::new("t", "f"), TestOutcome::Passed),
        }
    }

    /// The LATEST evidence wins, and it is the last one appended — a clean
    /// re-run after a dirty one takes the claim back out of the figure.
    #[test]
    fn dirty_count_takes_the_last_appended_evidence_per_claim() {
        let a = id(1);
        let b = id(2);
        let events = vec![
            evidence(a.clone(), 10, true),
            evidence(b.clone(), 11, true),
            // Later append, EARLIER ts: append order must still win.
            evidence(a, 1, false),
        ];
        assert_eq!(dirty_claims(&events), 1);
        assert_eq!(dirty_claims(&[]), 0);
        assert_eq!(dirty_claims(&[evidence(b, 12, false)]), 0);
    }
}
