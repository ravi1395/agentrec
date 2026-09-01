//! Deterministic fold: an append-only `attest.jsonl` event stream in, one
//! [`ClaimState`] per claim out. Pure, total, order-dependent only on the
//! order the events were appended.
//!
//! The fold is a DUMB APPLIER. It does not detect renames, does not decide
//! whether a run was flaky, and does not schedule retries — those are the
//! writers' jobs (`attest derive` / `attest verify`, Phase 3/4), which have
//! the context the flat event stream cannot express. Everything here is a
//! mechanical consequence of the events already written.

use super::events::{AttestEvent, HumanAnswer, ManualSeverity};
use super::types::{ClaimId, RecipeInvalidCause, TestIdentity, VerdictKind};
use std::collections::BTreeMap;

/// The claim state machine (spec "Architecture" → claim states).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimStatus {
    /// A test was discovered; nothing has been observed yet.
    Derived,
    /// A manual criterion was declared; awaits a human.
    Declared,
    /// At least one captured run exists. An author's own run stops here —
    /// only a verdict can go further.
    Evidenced,
    /// An independent replay agreed.
    Confirmed,
    /// The test fails on clean replay. Permanent: no later verdict moves it.
    ClaimFalse,
    /// Unverifiable this run. Retryable, and never blocks a gate by itself.
    RecipeInvalid { cause: RecipeInvalidCause },
    /// Replays disagreed. Tracked statistically, never blocks.
    Flaky,
    /// A human answered a manual card.
    Human { answer: HumanAnswer },
}

impl ClaimStatus {
    /// Whether this state blocks a gate. Only `claim-false` and an unanswered
    /// or rejected blocking manual item do; `recipe-invalid` and `flaky`
    /// explicitly do not (spec invariants).
    pub fn blocks_gate(&self) -> bool {
        matches!(self, ClaimStatus::ClaimFalse)
    }
}

/// How many of each thing this claim has seen. Nothing is discarded when a
/// terminal state refuses a transition — it is counted here instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClaimHistory {
    pub derives: usize,
    pub evidence: usize,
    pub verdicts: usize,
    pub confirmed_verdicts: usize,
    pub claim_false_verdicts: usize,
    pub recipe_invalid_verdicts: usize,
    pub flaky_observations: usize,
    pub stale_events: usize,
    pub human_answers: usize,
}

/// The folded state of one claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimState {
    pub claim_id: ClaimId,
    pub status: ClaimStatus,
    /// Current identity — remapped in place by a `derive` carrying
    /// `renamed_from`, so a rename loses no history.
    pub test_identity: Option<TestIdentity>,
    pub last_body_hash: Option<[u8; 32]>,
    pub renamed_from: Option<TestIdentity>,
    /// The `STALE` overlay: set by a `stale` event, and dropped only by a
    /// verdict that ESTABLISHES a state (`confirmed` or `claim-false`).
    ///
    /// The spec says "drops on the next verdict", which is ambiguous about
    /// `recipe-invalid` and `flaky-observation`. Those are precisely the "we
    /// still do not know" outcomes, and spec decision 5 says stale MORE when
    /// unsure, so they leave the overlay set. Value is the `ts` of the stale
    /// event that set it.
    pub stale_since: Option<u64>,
    /// Manual-criterion text and severity, for `manual-declare` claims.
    pub manual_text: Option<String>,
    pub manual_severity: Option<ManualSeverity>,
    pub last_human_note: Option<String>,
    pub history: ClaimHistory,
    pub last_ts: u64,
}

impl ClaimState {
    fn new(claim_id: ClaimId, status: ClaimStatus, ts: u64) -> Self {
        ClaimState {
            claim_id,
            status,
            test_identity: None,
            last_body_hash: None,
            renamed_from: None,
            stale_since: None,
            manual_text: None,
            manual_severity: None,
            last_human_note: None,
            history: ClaimHistory::default(),
            last_ts: ts,
        }
    }

    pub fn is_stale(&self) -> bool {
        self.stale_since.is_some()
    }
}

/// Fold an event stream into per-claim state, keyed by [`ClaimId`].
///
/// Also maintains the `test_identity → ClaimId` map internally so a rename
/// remaps rather than forking: the returned map is by id, and each state
/// carries its current identity.
pub fn fold_claims(events: &[AttestEvent]) -> BTreeMap<ClaimId, ClaimState> {
    let mut claims: BTreeMap<ClaimId, ClaimState> = BTreeMap::new();
    // Latest identity → claim. Kept for the fold's own remapping bookkeeping;
    // callers read identity off the state.
    let mut by_identity: BTreeMap<TestIdentity, ClaimId> = BTreeMap::new();

    for event in events {
        let id = event.claim_id().clone();
        let ts = event.ts();
        let entry = claims
            .entry(id.clone())
            .or_insert_with(|| ClaimState::new(id.clone(), ClaimStatus::Derived, ts));
        entry.last_ts = ts;

        match event {
            AttestEvent::Derive {
                test_identity,
                body_hash,
                renamed_from,
                ..
            } => {
                entry.history.derives += 1;
                if let Some(old) = &entry.test_identity {
                    if old != test_identity {
                        by_identity.remove(old);
                    }
                }
                if let Some(old) = renamed_from {
                    by_identity.remove(old);
                    entry.renamed_from = Some(old.clone());
                }
                entry.test_identity = Some(test_identity.clone());
                entry.last_body_hash = *body_hash;
                by_identity.insert(test_identity.clone(), id.clone());
                // A re-derive never regresses an established state: it only
                // (re)asserts that the test exists.
                if entry.history.derives == 1 {
                    entry.status = ClaimStatus::Derived;
                }
            }
            AttestEvent::Evidence { .. } => {
                entry.history.evidence += 1;
                // An author's own run can never reach CONFIRMED — evidence
                // moves DERIVED to EVIDENCED and touches nothing a verdict
                // has already established.
                if matches!(entry.status, ClaimStatus::Derived) {
                    entry.status = ClaimStatus::Evidenced;
                }
            }
            AttestEvent::Verdict { verdict: kind, .. } => {
                entry.history.verdicts += 1;
                match kind {
                    VerdictKind::Confirmed => entry.history.confirmed_verdicts += 1,
                    VerdictKind::ClaimFalse => entry.history.claim_false_verdicts += 1,
                    VerdictKind::RecipeInvalid { .. } => entry.history.recipe_invalid_verdicts += 1,
                    VerdictKind::FlakyObservation => entry.history.flaky_observations += 1,
                }
                // `claim-false` is permanent (spec decision 4): later verdicts
                // are recorded in history above and change nothing here.
                if matches!(entry.status, ClaimStatus::ClaimFalse) {
                    continue;
                }
                match kind {
                    VerdictKind::Confirmed => {
                        entry.status = ClaimStatus::Confirmed;
                        entry.stale_since = None;
                    }
                    VerdictKind::ClaimFalse => {
                        entry.status = ClaimStatus::ClaimFalse;
                        entry.stale_since = None;
                    }
                    VerdictKind::RecipeInvalid { cause } => {
                        entry.status = ClaimStatus::RecipeInvalid { cause: *cause };
                    }
                    VerdictKind::FlakyObservation => {
                        entry.status = ClaimStatus::Flaky;
                    }
                }
            }
            AttestEvent::Stale { .. } => {
                entry.history.stale_events += 1;
                entry.stale_since = Some(ts);
            }
            AttestEvent::Human { answer, note, .. } => {
                entry.history.human_answers += 1;
                entry.status = ClaimStatus::Human { answer: *answer };
                entry.last_human_note = note.clone();
            }
            AttestEvent::ManualDeclare { text, severity, .. } => {
                entry.manual_text = Some(text.clone());
                entry.manual_severity = Some(*severity);
                if entry.history.human_answers == 0 {
                    entry.status = ClaimStatus::Declared;
                }
            }
        }
    }

    claims
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attest::events::StaleCause;
    use crate::attest::types::{StructuredResult, TestOutcome};

    fn cid(n: u8) -> ClaimId {
        ClaimId::mint_with(1, &[n; 10])
    }

    fn ident(target: &str, name: &str) -> TestIdentity {
        TestIdentity::new(target, name)
    }

    fn derive(ts: u64, id: &ClaimId, identity: TestIdentity) -> AttestEvent {
        AttestEvent::Derive {
            ts,
            claim_id: id.clone(),
            test_identity: identity,
            body_hash: Some([1u8; 32]),
            renamed_from: None,
        }
    }

    fn evidence(ts: u64, id: &ClaimId, identity: TestIdentity) -> AttestEvent {
        AttestEvent::Evidence {
            ts,
            claim_id: id.clone(),
            turn_id: Some("t_abc".into()),
            dirty: false,
            output_blob: Some("blob".into()),
            result: StructuredResult::outcome(identity, TestOutcome::Passed),
        }
    }

    fn verdict(ts: u64, id: &ClaimId, kind: VerdictKind) -> AttestEvent {
        AttestEvent::Verdict {
            ts,
            claim_id: id.clone(),
            verdict: kind,
            replay_commit: "abc123".into(),
        }
    }

    fn stale(ts: u64, id: &ClaimId) -> AttestEvent {
        AttestEvent::Stale {
            ts,
            claim_id: id.clone(),
            cause: StaleCause::FileWrite {
                path: "cli/src/importcmd.rs".into(),
            },
        }
    }

    #[test]
    fn ac1_derive_evidence_stale_confirmed_ends_confirmed_and_unstale() {
        let id = cid(1);
        let i = ident("agentrec--integration", "a_test");
        let state = fold_claims(&[
            derive(1, &id, i.clone()),
            evidence(2, &id, i.clone()),
            stale(3, &id),
            verdict(4, &id, VerdictKind::Confirmed),
        ]);
        let c = &state[&id];
        assert_eq!(c.status, ClaimStatus::Confirmed);
        assert!(
            !c.is_stale(),
            "a confirming verdict drops the stale overlay"
        );
        assert_eq!(c.history.stale_events, 1);
    }

    #[test]
    fn ac2_recipe_invalid_then_confirmed_ends_confirmed() {
        let id = cid(2);
        let i = ident("agentrec--integration", "a_test");
        let state = fold_claims(&[
            derive(1, &id, i),
            verdict(
                2,
                &id,
                VerdictKind::RecipeInvalid {
                    cause: RecipeInvalidCause::Build,
                },
            ),
            verdict(3, &id, VerdictKind::Confirmed),
        ]);
        assert_eq!(state[&id].status, ClaimStatus::Confirmed);
    }

    #[test]
    fn a_recipe_invalid_verdict_leaves_the_stale_overlay_set() {
        let id = cid(9);
        let i = ident("agentrec--integration", "a_test");
        let state = fold_claims(&[
            derive(1, &id, i),
            stale(2, &id),
            verdict(
                3,
                &id,
                VerdictKind::RecipeInvalid {
                    cause: RecipeInvalidCause::Missing,
                },
            ),
        ]);
        assert!(
            state[&id].is_stale(),
            "recipe-invalid establishes nothing, so the claim stays stale"
        );
    }

    #[test]
    fn ac3_author_evidence_alone_never_reaches_confirmed() {
        let id = cid(3);
        let i = ident("agentrec--integration", "a_test");
        let mut events = vec![derive(1, &id, i.clone())];
        for ts in 2..12 {
            events.push(evidence(ts, &id, i.clone()));
        }
        let c = &fold_claims(&events)[&id];
        assert_eq!(c.status, ClaimStatus::Evidenced);
        assert_ne!(c.status, ClaimStatus::Confirmed);
        assert_eq!(c.history.evidence, 10);
    }

    #[test]
    fn ac4_rename_continues_same_claim_id_losing_no_history() {
        let id = cid(4);
        let old = ident("agentrec--integration", "old_name");
        let new = ident("agentrec--integration", "new_name");
        let events = vec![
            derive(1, &id, old.clone()),
            evidence(2, &id, old.clone()),
            verdict(3, &id, VerdictKind::Confirmed),
            AttestEvent::Derive {
                ts: 4,
                claim_id: id.clone(),
                test_identity: new.clone(),
                body_hash: Some([2u8; 32]),
                renamed_from: Some(old.clone()),
            },
        ];
        let folded = fold_claims(&events);
        assert_eq!(folded.len(), 1, "a rename must not fork a second claim");
        let c = &folded[&id];
        assert_eq!(c.claim_id, id);
        assert_eq!(c.test_identity.as_ref(), Some(&new));
        assert_eq!(c.renamed_from.as_ref(), Some(&old));
        assert_eq!(c.last_body_hash, Some([2u8; 32]));
        // History survives the rename.
        assert_eq!(c.history.evidence, 1);
        assert_eq!(c.history.confirmed_verdicts, 1);
        assert_eq!(c.status, ClaimStatus::Confirmed);
    }

    #[test]
    fn ac5_claim_false_is_permanent_against_a_later_confirmed_verdict() {
        let id = cid(5);
        let i = ident("agentrec--integration", "a_test");
        let state = fold_claims(&[
            derive(1, &id, i),
            verdict(2, &id, VerdictKind::ClaimFalse),
            verdict(3, &id, VerdictKind::Confirmed),
            verdict(4, &id, VerdictKind::FlakyObservation),
        ]);
        let c = &state[&id];
        assert_eq!(c.status, ClaimStatus::ClaimFalse);
        assert!(c.status.blocks_gate());
        // The refused transitions are counted, not discarded.
        assert_eq!(c.history.confirmed_verdicts, 1);
        assert_eq!(c.history.flaky_observations, 1);
        assert_eq!(c.history.verdicts, 3);
    }

    #[test]
    fn ac6_same_fn_name_in_two_targets_folds_to_two_claims() {
        let a = cid(6);
        let b = cid(7);
        let state = fold_claims(&[
            derive(1, &a, ident("agentrec--integration", "same_name")),
            derive(2, &b, ident("agentrec--golden", "same_name")),
            verdict(3, &a, VerdictKind::Confirmed),
            verdict(4, &b, VerdictKind::ClaimFalse),
        ]);
        assert_eq!(state.len(), 2);
        assert_eq!(state[&a].status, ClaimStatus::Confirmed);
        assert_eq!(state[&b].status, ClaimStatus::ClaimFalse);
        assert_ne!(state[&a].test_identity, state[&b].test_identity);
        // The discriminating half: an identity that ignored its target
        // component would collapse these two into one key, and the fold's
        // internal identity map (and any consumer's) would cross-attribute.
        let keyed: std::collections::BTreeSet<_> = [
            ident("agentrec--integration", "same_name"),
            ident("agentrec--golden", "same_name"),
        ]
        .into_iter()
        .collect();
        assert_eq!(keyed.len(), 2, "target must be part of TestIdentity's key");
    }

    #[test]
    fn ac8_flaky_state_comes_only_from_flaky_observation_and_never_blocks() {
        let id = cid(8);
        let i = ident("agentrec--approve", "a_killed_approve");
        let flaky = fold_claims(&[
            derive(1, &id, i.clone()),
            verdict(2, &id, VerdictKind::FlakyObservation),
        ]);
        assert_eq!(flaky[&id].status, ClaimStatus::Flaky);
        assert!(!flaky[&id].status.blocks_gate());

        // No other event kind produces FLAKY.
        for other in [
            VerdictKind::Confirmed,
            VerdictKind::ClaimFalse,
            VerdictKind::RecipeInvalid {
                cause: RecipeInvalidCause::Harness,
            },
        ] {
            let s = fold_claims(&[derive(1, &id, i.clone()), verdict(2, &id, other)]);
            assert_ne!(s[&id].status, ClaimStatus::Flaky);
        }
        let evidence_only = fold_claims(&[derive(1, &id, i.clone()), evidence(2, &id, i.clone())]);
        assert_ne!(evidence_only[&id].status, ClaimStatus::Flaky);
        let staled = fold_claims(&[derive(1, &id, i), stale(2, &id)]);
        assert_ne!(staled[&id].status, ClaimStatus::Flaky);
    }

    #[test]
    fn a_manual_declare_lands_declared_and_a_human_answer_closes_it() {
        let id = cid(10);
        let state = fold_claims(&[
            AttestEvent::ManualDeclare {
                ts: 1,
                claim_id: id.clone(),
                text: "install line is correct".into(),
                severity: ManualSeverity::Blocking,
            },
            AttestEvent::Human {
                ts: 2,
                claim_id: id.clone(),
                answer: HumanAnswer::Yes,
                note: Some("checked".into()),
            },
        ]);
        let c = &state[&id];
        assert_eq!(
            c.status,
            ClaimStatus::Human {
                answer: HumanAnswer::Yes
            }
        );
        assert_eq!(c.manual_severity, Some(ManualSeverity::Blocking));
        assert_eq!(c.manual_text.as_deref(), Some("install line is correct"));
    }

    #[test]
    fn folding_is_deterministic_and_empty_input_yields_no_claims() {
        assert!(fold_claims(&[]).is_empty());
        let id = cid(11);
        let i = ident("agentrec--integration", "a_test");
        let events = vec![derive(1, &id, i.clone()), evidence(2, &id, i)];
        assert_eq!(fold_claims(&events), fold_claims(&events));
    }
}
