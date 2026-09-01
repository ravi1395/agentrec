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
    /// Whether this STATUS alone blocks a gate: only `CLAIM_FALSE` does.
    /// `RECIPE_INVALID` and `FLAKY` explicitly do not (spec invariants).
    ///
    /// The gate's other blocking condition — an unanswered or rejected
    /// `blocking` manual item — is not a status and is not decided here: it
    /// is the gate reading [`ClaimState::manual_severity`] against the
    /// claim's `human` answer (Phase 5). A status-only predicate cannot see
    /// severity, so this function deliberately does not pretend to.
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
    pub manual_declares: usize,
}

/// The folded state of one claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimState {
    pub claim_id: ClaimId,
    pub status: ClaimStatus,
    /// Current identity — remapped in place by a `derive` carrying
    /// `renamed_from`, so a rename loses no history. Also filled in from an
    /// `evidence` event's `StructuredResult` when no `derive` has been seen
    /// yet, so an out-of-order or partially-read log still attributes.
    ///
    /// Stays `None` only when this claim has been seen exclusively through
    /// events that carry no identity at all — a `verdict`/`stale`/`human`
    /// before its `derive` (torn tail, or two writers interleaving).
    pub test_identity: Option<TestIdentity>,
    /// Last body hash a `derive` reported. A later `derive` carrying `None`
    /// never erases it: absence means "this derive did not compute a hash",
    /// not "the test has no body".
    pub last_body_hash: Option<[u8; 32]>,
    pub renamed_from: Option<TestIdentity>,
    /// The `STALE` overlay — value is the `ts` of the `stale` event that set
    /// it. The rule for when it is set and dropped is stated once, in
    /// `ATTEST-FORMAT.md` § "The `STALE` overlay".
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

/// What [`fold_claims`] returns: per-claim state plus the
/// `test_identity → ClaimId` index the fold maintained on the way.
///
/// The index is part of the contract, not bookkeeping: `attest derive`
/// (Phase 3) needs exactly this lookup to resolve a rename — it compares the
/// new discovery set against the known identities and must find the OLD
/// claim's id to write into the `renamed_from` event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldResult {
    /// Folded state, keyed by claim id.
    pub claims: BTreeMap<ClaimId, ClaimState>,
    /// Identity → the LATEST claim that declared it.
    ///
    /// An identity a rename moved away from is removed. But two claims CAN
    /// declare the same identity (a writer bug: `derive(B, x)` then
    /// `derive(A, x)`), and then the index holds only `x → A` while `B` still
    /// carries `x` as its `test_identity`. The fold does not evict `B`'s
    /// identity — a dumb applier has no basis for deciding which writer was
    /// wrong — so the index answers "who owns this identity now", not "every
    /// claim carrying it".
    pub by_identity: BTreeMap<TestIdentity, ClaimId>,
}

impl FoldResult {
    /// The claim currently owning this test identity, if any.
    pub fn claim_for(&self, identity: &TestIdentity) -> Option<&ClaimId> {
        self.by_identity.get(identity)
    }

    /// The folded state for a claim id.
    pub fn claim(&self, id: &ClaimId) -> Option<&ClaimState> {
        self.claims.get(id)
    }
}

/// Fold an event stream into per-claim state plus the live identity index.
///
/// The fold is a DUMB APPLIER and this is where that bites in three places
/// worth naming, all deliberate:
///
/// - It never detects a rename. A `derive` whose `test_identity` differs from
///   the claim's current one, with NO `renamed_from`, still remaps that claim's
///   identity silently — the fold has no way to tell an unannounced rename from
///   a writer correcting itself, and guessing would be worse than applying what
///   was written. `attest derive` (Phase 3) is what decides.
/// - It never decides flakiness or schedules a retry.
/// - `CLAIM_FALSE` is permanent against EVERY later event kind, not just
///   verdicts (spec decision 4). Later events are counted in
///   [`ClaimHistory`] and move nothing.
pub fn fold_claims(events: &[AttestEvent]) -> FoldResult {
    let mut claims: BTreeMap<ClaimId, ClaimState> = BTreeMap::new();
    let mut by_identity: BTreeMap<TestIdentity, ClaimId> = BTreeMap::new();

    for event in events {
        let id = event.claim_id().clone();
        let ts = event.ts();
        let first_sight = !claims.contains_key(&id);
        let entry = claims
            .entry(id.clone())
            .or_insert_with(|| ClaimState::new(id.clone(), ClaimStatus::Derived, ts));
        entry.last_ts = ts;
        // A refuted claim stays refuted whatever arrives next. Computed
        // before the arm runs so each arm can still record its history.
        let refuted = matches!(entry.status, ClaimStatus::ClaimFalse);

        match event {
            AttestEvent::Derive {
                test_identity,
                body_hash,
                renamed_from,
                ..
            } => {
                entry.history.derives += 1;
                // The claim's own outgoing identity is the only mapping this
                // event may retire. `renamed_from` is recorded but never used
                // to remove: on a rename it names exactly this claim's current
                // identity (already handled here), and if it named a DIFFERENT
                // claim's live identity, honoring it would silently unmap that
                // claim — a writer bug the fold must not amplify.
                if let Some(old) = &entry.test_identity {
                    if old != test_identity {
                        by_identity.remove(old);
                    }
                }
                if let Some(old) = renamed_from {
                    entry.renamed_from = Some(old.clone());
                }
                entry.test_identity = Some(test_identity.clone());
                // Merge, never clobber: a derive that computed no hash says
                // nothing about the hash a previous derive did compute.
                if body_hash.is_some() {
                    entry.last_body_hash = *body_hash;
                }
                by_identity.insert(test_identity.clone(), id.clone());
                // No status assignment here, deliberately. The claim is born
                // DERIVED by `ClaimState::new` on first sight of its id; every
                // LATER derive updates identity, body hash and the index only.
                // Keying on "first derive" instead would regress an already
                // established claim whenever the derive is not the first event
                // for that id — `[evidence, derive]`, `[verdict, derive]` and a
                // rename after any history all take that path.
            }
            AttestEvent::Evidence { result, .. } => {
                entry.history.evidence += 1;
                // An evidence event carries the identity the run was for, so
                // an evidence-before-derive log still attributes.
                if entry.test_identity.is_none() {
                    entry.test_identity = Some(result.identity.clone());
                    by_identity.insert(result.identity.clone(), id.clone());
                }
                // An author's own run can never reach CONFIRMED — evidence
                // moves DERIVED to EVIDENCED and touches nothing a verdict
                // has already established.
                if matches!(entry.status, ClaimStatus::Derived) && !refuted {
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
                if refuted {
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
                // A permanently refuted claim is never re-queued: the Verdict
                // arm returns early on CLAIM_FALSE, so an overlay set here
                // could never be cleared again and Phase 4 would re-verify a
                // permanent refutation forever.
                if refuted {
                    continue;
                }
                entry.stale_since = Some(ts);
            }
            AttestEvent::Human { answer, note, .. } => {
                entry.history.human_answers += 1;
                if refuted {
                    continue;
                }
                entry.status = ClaimStatus::Human { answer: *answer };
                entry.last_human_note = note.clone();
            }
            AttestEvent::ManualDeclare { text, severity, .. } => {
                entry.history.manual_declares += 1;
                entry.manual_text = Some(text.clone());
                entry.manual_severity = Some(*severity);
                // A manual criterion is meaningful only on its own fresh id:
                // `manual-declare` is the hand-authored kind, and a claim
                // that already carries derives, evidence or a verdict is a
                // TEST's claim. Landing on one is a writer bug, so the text
                // is recorded and the status is left where the machine put
                // it rather than demoting a real verdict to DECLARED.
                if first_sight {
                    entry.status = ClaimStatus::Declared;
                }
            }
        }
    }

    FoldResult {
        claims,
        by_identity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attest::events::StaleCause;
    use crate::attest::types::{StructuredResult, TestOutcome};

    /// Most tests only care about the folded states.
    fn states(events: &[AttestEvent]) -> BTreeMap<ClaimId, ClaimState> {
        fold_claims(events).claims
    }

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
        let state = states(&[
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
        let state = states(&[
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
        let state = states(&[
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
        let c = &states(&events)[&id];
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
        let folded = states(&events);
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
        let state = states(&[
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
        let state = states(&[
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
        let flaky = states(&[
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
            let s = states(&[derive(1, &id, i.clone()), verdict(2, &id, other)]);
            assert_ne!(s[&id].status, ClaimStatus::Flaky);
        }
        let evidence_only = states(&[derive(1, &id, i.clone()), evidence(2, &id, i.clone())]);
        assert_ne!(evidence_only[&id].status, ClaimStatus::Flaky);
        let staled = states(&[derive(1, &id, i), stale(2, &id)]);
        assert_ne!(staled[&id].status, ClaimStatus::Flaky);
    }

    #[test]
    fn a_manual_declare_lands_declared_and_a_human_answer_closes_it() {
        let id = cid(10);
        let state = states(&[
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
    fn ac9_claim_false_survives_a_later_human_answer_and_manual_declare() {
        let id = cid(12);
        let i = ident("agentrec--integration", "a_test");
        let state = states(&[
            derive(1, &id, i.clone()),
            verdict(2, &id, VerdictKind::ClaimFalse),
            AttestEvent::Human {
                ts: 3,
                claim_id: id.clone(),
                answer: HumanAnswer::Yes,
                note: Some("looks fine to me".into()),
            },
            AttestEvent::ManualDeclare {
                ts: 4,
                claim_id: id.clone(),
                text: "trust me".into(),
                severity: ManualSeverity::Fyi,
            },
            evidence(5, &id, i),
        ]);
        let c = &state[&id];
        assert_eq!(
            c.status,
            ClaimStatus::ClaimFalse,
            "a permanent refutation must survive every later event kind"
        );
        assert!(c.status.blocks_gate());
        // The refused events are counted, never discarded.
        assert_eq!(c.history.human_answers, 1);
        assert_eq!(c.history.manual_declares, 1);
        assert_eq!(c.history.evidence, 1);
        // A refused human answer leaves no note behind either.
        assert_eq!(c.last_human_note, None);
    }

    #[test]
    fn ac10_manual_declare_on_an_existing_claim_records_without_moving_status() {
        let id = cid(13);
        let i = ident("agentrec--integration", "a_test");
        let state = states(&[
            derive(1, &id, i),
            verdict(2, &id, VerdictKind::Confirmed),
            AttestEvent::ManualDeclare {
                ts: 3,
                claim_id: id.clone(),
                text: "also check it by hand".into(),
                severity: ManualSeverity::Fyi,
            },
        ]);
        let c = &state[&id];
        assert_eq!(
            c.status,
            ClaimStatus::Confirmed,
            "a manual-declare must not demote an established verdict"
        );
        assert_eq!(c.manual_text.as_deref(), Some("also check it by hand"));
        assert_eq!(c.history.manual_declares, 1);
    }

    #[test]
    fn ac11_by_identity_tracks_the_live_identity_across_a_rename() {
        let id = cid(14);
        let old = ident("agentrec--integration", "old_name");
        let new = ident("agentrec--integration", "new_name");
        let folded = fold_claims(&[
            derive(1, &id, old.clone()),
            AttestEvent::Derive {
                ts: 2,
                claim_id: id.clone(),
                test_identity: new.clone(),
                body_hash: None,
                renamed_from: Some(old.clone()),
            },
        ]);
        assert_eq!(folded.claim_for(&new), Some(&id));
        assert_eq!(
            folded.claim_for(&old),
            None,
            "the identity a rename moved away from must not still resolve"
        );
        assert_eq!(folded.by_identity.len(), 1);
        // Two live identities resolve to their own claims.
        let a = cid(15);
        let b = cid(16);
        let two = fold_claims(&[
            derive(1, &a, ident("agentrec--integration", "same_name")),
            derive(2, &b, ident("agentrec--golden", "same_name")),
        ]);
        assert_eq!(
            two.claim_for(&ident("agentrec--integration", "same_name")),
            Some(&a)
        );
        assert_eq!(
            two.claim_for(&ident("agentrec--golden", "same_name")),
            Some(&b)
        );
    }

    #[test]
    fn ac12_a_later_derive_without_a_body_hash_does_not_erase_the_recorded_one() {
        let id = cid(17);
        let i = ident("agentrec--integration", "a_test");
        let state = states(&[
            derive(1, &id, i.clone()), // carries Some([1u8; 32])
            AttestEvent::Derive {
                ts: 2,
                claim_id: id.clone(),
                test_identity: i,
                body_hash: None,
                renamed_from: None,
            },
        ]);
        assert_eq!(
            state[&id].last_body_hash,
            Some([1u8; 32]),
            "None means 'this derive computed no hash', not 'the hash is gone'"
        );
    }

    #[test]
    fn ac13_evidence_before_any_derive_still_carries_the_test_identity() {
        let id = cid(18);
        let i = ident("agentrec--integration", "a_test");
        let folded = fold_claims(&[evidence(1, &id, i.clone())]);
        let c = &folded.claims[&id];
        assert_eq!(c.test_identity.as_ref(), Some(&i));
        assert_eq!(folded.claim_for(&i), Some(&id));
        assert_eq!(c.status, ClaimStatus::Evidenced);

        // A claim seen only through identity-less kinds stays None.
        let bare = fold_claims(&[stale(1, &id)]);
        assert_eq!(bare.claims[&id].test_identity, None);
        assert!(bare.by_identity.is_empty());
    }

    #[test]
    fn ac15_a_later_derive_never_regresses_an_established_status() {
        let i = ident("agentrec--integration", "a_test");
        let renamed = ident("agentrec--integration", "renamed");

        // [evidence, derive]
        let id = cid(19);
        let s = states(&[evidence(1, &id, i.clone()), derive(2, &id, i.clone())]);
        assert_eq!(s[&id].status, ClaimStatus::Evidenced);

        // [verdict(confirmed), derive]
        let id = cid(20);
        let s = states(&[
            derive(1, &id, i.clone()),
            verdict(2, &id, VerdictKind::Confirmed),
            derive(3, &id, i.clone()),
        ]);
        assert_eq!(s[&id].status, ClaimStatus::Confirmed);

        // [verdict(confirmed), derive] with no prior derive at all
        let id = cid(21);
        let s = states(&[
            verdict(1, &id, VerdictKind::Confirmed),
            derive(2, &id, i.clone()),
        ]);
        assert_eq!(s[&id].status, ClaimStatus::Confirmed);

        // [manual-declare, derive]
        let id = cid(22);
        let s = states(&[
            AttestEvent::ManualDeclare {
                ts: 1,
                claim_id: id.clone(),
                text: "by hand".into(),
                severity: ManualSeverity::Blocking,
            },
            derive(2, &id, i.clone()),
        ]);
        assert_eq!(s[&id].status, ClaimStatus::Declared);

        // [evidence(x), derive(y, renamed_from: x)]
        let id = cid(23);
        let folded = fold_claims(&[
            evidence(1, &id, i.clone()),
            AttestEvent::Derive {
                ts: 2,
                claim_id: id.clone(),
                test_identity: renamed.clone(),
                body_hash: None,
                renamed_from: Some(i.clone()),
            },
        ]);
        assert_eq!(folded.claims[&id].status, ClaimStatus::Evidenced);
        assert_eq!(folded.claims[&id].test_identity.as_ref(), Some(&renamed));

        // A first derive still mints DERIVED.
        let id = cid(24);
        assert_eq!(
            states(&[derive(1, &id, i)])[&id].status,
            ClaimStatus::Derived
        );
    }

    #[test]
    fn ac16_a_refuted_claim_is_never_left_stale() {
        let id = cid(25);
        let i = ident("agentrec--integration", "a_test");
        let state = states(&[
            derive(1, &id, i),
            stale(2, &id),
            verdict(3, &id, VerdictKind::ClaimFalse),
            stale(4, &id),
        ]);
        let c = &state[&id];
        assert_eq!(c.status, ClaimStatus::ClaimFalse);
        assert!(
            !c.is_stale(),
            "a permanent refutation is an established state; a stale overlay \
             on it could never be cleared and would re-verify forever"
        );
        // Both stale events are still counted.
        assert_eq!(c.history.stale_events, 2);
    }

    #[test]
    fn a_flaky_observation_leaves_the_stale_overlay_set() {
        let id = cid(26);
        let i = ident("agentrec--integration", "a_test");
        let state = states(&[
            derive(1, &id, i),
            stale(2, &id),
            verdict(3, &id, VerdictKind::FlakyObservation),
        ]);
        assert_eq!(state[&id].status, ClaimStatus::Flaky);
        assert!(
            state[&id].is_stale(),
            "replays that disagreed establish nothing, so the claim stays stale"
        );
    }

    #[test]
    fn ac17_two_claims_declaring_one_identity_index_the_latest_writer() {
        let a = cid(27);
        let b = cid(28);
        let x = ident("agentrec--integration", "contested");
        let folded = fold_claims(&[derive(1, &b, x.clone()), derive(2, &a, x.clone())]);
        // The index answers "who owns this identity now".
        assert_eq!(folded.claim_for(&x), Some(&a));
        // The earlier claim keeps the identity it was told it had — the fold
        // has no basis for deciding which writer was wrong.
        assert_eq!(folded.claims[&b].test_identity.as_ref(), Some(&x));
        assert_eq!(folded.by_identity.len(), 1);
    }

    #[test]
    fn folding_is_deterministic_and_empty_input_yields_no_claims() {
        assert!(states(&[]).is_empty());
        let id = cid(11);
        let i = ident("agentrec--integration", "a_test");
        let events = vec![derive(1, &id, i.clone()), evidence(2, &id, i)];
        assert_eq!(states(&events), states(&events));
    }
}
