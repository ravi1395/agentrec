//! `agentrec attest status [--json]` — the read-only view over
//! `.agentrec/attest.jsonl`: claim counts by status, the `STALE` overlay
//! count, and the dev-loop-only figure.
//!
//! Minimal by design (plan Phase 3). Richer rendering — surfacing
//! `recipe-invalid` causes and manual-card severity — needed Phase 4's verdict
//! writer and is an extension of this same file, not a new one; Phase 5 added
//! it, along with the shared read helpers the review/gate/report verbs use
//! ([`status_label`], [`Provenance`]).
//!
//! Why those helpers live here and not in the fold: `ClaimState` carries
//! status, identity, body hash, the stale overlay, manual text/severity, the
//! last human note, history counters and `last_ts` — and nothing about which
//! turn an `evidence` event was joined to, which blob it captured, or which
//! commit a verdict replayed. `agentrec-core` is frozen for Phase 5, so
//! provenance is a SECOND pass over the same event slice, exactly as
//! [`dirty_claims`] already does, and on the same convention: "latest" means
//! APPEND ORDER, not the largest `ts`.

use crate::attest::lock::read_attest;
use agentrec_core::attest::events::{AttestEvent, HumanAnswer, ManualSeverity};
use agentrec_core::attest::fold::{fold_claims, ClaimState, ClaimStatus};
use agentrec_core::attest::types::{ClaimId, RecipeInvalidCause, VerdictKind};
use std::collections::BTreeMap;
use std::path::Path;

/// The wire spelling of a `recipe-invalid` cause, derived from serde so a new
/// variant cannot drift out of sync with the events it is rendered beside
/// (the AC-ATTEST-P4C-11 rule, reused).
pub(crate) fn cause_label(cause: &RecipeInvalidCause) -> String {
    serde_json::to_value(cause)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{cause:?}"))
}

/// The uppercase state name every attest verb prints for a status.
pub(crate) fn status_label(status: &ClaimStatus) -> String {
    match status {
        ClaimStatus::Derived => "DERIVED".to_string(),
        ClaimStatus::Declared => "DECLARED".to_string(),
        ClaimStatus::Evidenced => "EVIDENCED".to_string(),
        ClaimStatus::Confirmed => "CONFIRMED".to_string(),
        ClaimStatus::ClaimFalse => "CLAIM_FALSE".to_string(),
        ClaimStatus::RecipeInvalid { cause } => {
            format!("RECIPE_INVALID({})", cause_label(cause))
        }
        ClaimStatus::Flaky => "FLAKY".to_string(),
        ClaimStatus::Human { answer } => match answer {
            HumanAnswer::Yes => "HUMAN(yes)".to_string(),
            HumanAnswer::No => "HUMAN(no)".to_string(),
            HumanAnswer::Skip => "HUMAN(skip)".to_string(),
        },
    }
}

/// Whether this claim blocks `attest gate`, and why.
///
/// The rule itself is stated ONCE, in `ATTEST-FORMAT.md` § "Gate blocking,
/// precisely". This function is that section's implementation and adds no
/// policy of its own. A claim meeting both halves reports only the first, so
/// the gate's count is one per claim.
pub(crate) fn blocking_reason(claim: &ClaimState) -> Option<String> {
    if claim.status.blocks_gate() {
        return Some("claim-false: independent replay refuted this claim".to_string());
    }
    if claim.manual_severity == Some(ManualSeverity::Blocking) {
        return match claim.status {
            ClaimStatus::Human {
                answer: HumanAnswer::Yes,
            } => None,
            ClaimStatus::Human {
                answer: HumanAnswer::No,
            } => Some("blocking manual criterion rejected".to_string()),
            ClaimStatus::Human {
                answer: HumanAnswer::Skip,
            } => Some("blocking manual criterion skipped".to_string()),
            _ => Some("blocking manual criterion unanswered".to_string()),
        };
    }
    None
}

/// The last `evidence` and last `verdict` a claim saw, in append order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Provenance {
    pub(crate) turn_id: Option<String>,
    pub(crate) output_blob: Option<String>,
    pub(crate) dirty: Option<bool>,
    pub(crate) last_verdict: Option<String>,
    pub(crate) replay_commit: Option<String>,
}

/// Walk the event slice once, collecting per-claim provenance the fold does
/// not retain. Append order wins, matching [`dirty_claims`].
pub(crate) fn provenance(events: &[AttestEvent]) -> BTreeMap<ClaimId, Provenance> {
    let mut out: BTreeMap<ClaimId, Provenance> = BTreeMap::new();
    for event in events {
        match event {
            AttestEvent::Evidence {
                claim_id,
                turn_id,
                dirty,
                output_blob,
                ..
            } => {
                let entry = out.entry(claim_id.clone()).or_default();
                entry.turn_id = turn_id.clone();
                entry.output_blob = output_blob.clone();
                entry.dirty = Some(*dirty);
            }
            AttestEvent::Verdict {
                claim_id,
                verdict,
                replay_commit,
                ..
            } => {
                let entry = out.entry(claim_id.clone()).or_default();
                entry.last_verdict = Some(match verdict {
                    VerdictKind::Confirmed => "confirmed".to_string(),
                    VerdictKind::ClaimFalse => "claim-false".to_string(),
                    VerdictKind::RecipeInvalid { cause } => {
                        format!("recipe-invalid({})", cause_label(cause))
                    }
                    VerdictKind::FlakyObservation => "flaky-observation".to_string(),
                });
                entry.replay_commit = Some(replay_commit.clone());
            }
            _ => {}
        }
    }
    out
}

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

    // Phase 5 additions, both purely additive to the payload below.
    //
    // `recipe_invalid_causes` breaks the existing flat `recipe_invalid` count
    // out by cause; the flat key keeps its name and value.
    //
    // `manual` counts manual claims by severity. "answered" means a `human`
    // event has landed on the claim, whatever the answer — a `skip` is an
    // answer that was given, and the gate (not this counter) is what decides
    // whether it is good enough.
    let mut causes: BTreeMap<String, usize> = BTreeMap::new();
    let mut manual = [[0usize; 2]; 2]; // [severity: blocking, fyi][answered]
    for claim in folded.claims.values() {
        if let ClaimStatus::RecipeInvalid { cause } = &claim.status {
            *causes.entry(cause_label(cause)).or_default() += 1;
        }
        if let Some(severity) = claim.manual_severity {
            let sev = usize::from(severity == ManualSeverity::Fyi);
            let answered = usize::from(matches!(claim.status, ClaimStatus::Human { .. }));
            manual[sev][answered] += 1;
        }
    }

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
            "recipe_invalid_causes": causes,
            "manual": {
                "blocking": {"unanswered": manual[0][0], "answered": manual[0][1]},
                "fyi": {"unanswered": manual[1][0], "answered": manual[1][1]},
            },
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return Ok(());
    }

    println!("claims: {total}");
    for (label, n) in counts.rows() {
        println!("  {label}: {n}");
    }
    for (cause, n) in &causes {
        println!("  recipe-invalid/{cause}: {n}");
    }
    println!(
        "manual: blocking {} unanswered / {} answered, fyi {} unanswered / {} answered",
        manual[0][0], manual[0][1], manual[1][0], manual[1][1]
    );
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
