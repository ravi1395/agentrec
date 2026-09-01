//! `agentrec attest derive [--crate <path>]` — discover tests and append the
//! `derive` events that mint (or carry forward) their claims.
//!
//! **Rename detection lives HERE, not in the fold.** `fold.rs` is a dumb
//! applier: it reads `renamed_from` off an event and remaps, detecting nothing.
//! This is the writer that has the context to detect — the previous discovery
//! set, the new one, and each test's body hash.
//!
//! The rule, and why it refuses rather than guesses:
//!
//! - A discovered identity already in the fold's `by_identity` index: append
//!   nothing, unless its body hash changed — then one `derive` with the SAME
//!   `claim_id`, the new hash, and no `renamed_from`.
//! - A NEW identity whose body hash matches exactly one VANISHED identity —
//!   and which is itself the only new identity with that hash — adopts that
//!   claim's id and records `renamed_from`.
//! - Anything ambiguous in either direction (two vanished tests share a body,
//!   or two new tests do) mints a FRESH id. A wrong adoption silently welds one
//!   test's history onto another; a fresh id costs only the history.
//!
//! The vanished set is scoped to the TARGETS this discovery enumerated. Without
//! that scope, running `attest derive --crate <one crate>` would see every other
//! crate's tests as vanished and hand their claim ids to unrelated new tests.

use crate::attest::adapter_cargo::{discover_with_hashes, BodyHashes};
use crate::attest::lock::{append_attest_locked, read_attest};
use agentrec_core::attest::events::AttestEvent;
use agentrec_core::attest::fold::fold_claims;
use agentrec_core::attest::types::{ClaimId, TestIdentity};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DeriveCounts {
    pub new: usize,
    pub renamed: usize,
    pub rehashed: usize,
    pub unchanged: usize,
}

pub fn run(root: &Path, crate_path: Option<&Path>) -> Result<(), String> {
    let crate_root = crate_path.unwrap_or(root);
    let (identities, hashes) = discover_with_hashes(crate_root)?;

    let (events, _) = read_attest(root)?;
    let folded = fold_claims(&events);

    let (batch, counts) = plan_derives(&folded, &identities, &hashes);
    append_attest_locked(root, &batch)?;

    println!(
        "discovered {} test(s): {} new, {} renamed, {} rehashed, {} unchanged",
        identities.len(),
        counts.new,
        counts.renamed,
        counts.rehashed,
        counts.unchanged
    );
    Ok(())
}

/// The pure half, so the rename rule is testable without spawning cargo.
fn plan_derives(
    folded: &agentrec_core::attest::fold::FoldResult,
    identities: &[TestIdentity],
    hashes: &BodyHashes,
) -> (Vec<AttestEvent>, DeriveCounts) {
    let discovered: BTreeSet<&TestIdentity> = identities.iter().collect();
    let targets: BTreeSet<&str> = identities.iter().map(|i| i.target.as_str()).collect();

    // Identities the fold knows for the targets we just enumerated, that this
    // discovery no longer sees.
    let mut vanished_by_hash: BTreeMap<[u8; 32], Vec<(&TestIdentity, &ClaimId)>> = BTreeMap::new();
    for (identity, claim_id) in &folded.by_identity {
        if !targets.contains(identity.target.as_str()) || discovered.contains(identity) {
            continue;
        }
        let Some(state) = folded.claim(claim_id) else {
            continue;
        };
        let Some(hash) = state.last_body_hash else {
            continue;
        };
        vanished_by_hash
            .entry(hash)
            .or_default()
            .push((identity, claim_id));
    }

    // New identities bucketed by hash, so an ambiguous NEW side is visible too.
    let mut new_by_hash: BTreeMap<[u8; 32], usize> = BTreeMap::new();
    for identity in identities {
        if folded.claim_for(identity).is_some() {
            continue;
        }
        if let Some(hash) = hashes.get(identity) {
            *new_by_hash.entry(*hash).or_default() += 1;
        }
    }

    let ts = crate::cmds::wall_now_ms();
    let mut batch = Vec::new();
    let mut counts = DeriveCounts::default();
    let mut consumed: BTreeSet<&ClaimId> = BTreeSet::new();

    for identity in identities {
        let hash = hashes.get(identity).copied();
        match folded.claim_for(identity) {
            Some(claim_id) => {
                let known = folded.claim(claim_id).and_then(|s| s.last_body_hash);
                // A hash we could not compute must never erase one we have —
                // `None` means "this derive did not hash", not "no body".
                if hash.is_some() && hash != known {
                    counts.rehashed += 1;
                    batch.push(AttestEvent::Derive {
                        ts,
                        claim_id: claim_id.clone(),
                        test_identity: identity.clone(),
                        body_hash: hash,
                        renamed_from: None,
                    });
                } else {
                    counts.unchanged += 1;
                }
            }
            None => {
                let donor = hash.and_then(|h| {
                    if new_by_hash.get(&h) != Some(&1) {
                        return None;
                    }
                    let candidates = vanished_by_hash.get(&h)?;
                    let [(old_identity, old_id)] = candidates.as_slice() else {
                        return None;
                    };
                    consumed.insert(old_id).then_some((*old_identity, *old_id))
                });
                match donor {
                    Some((old_identity, old_id)) => {
                        counts.renamed += 1;
                        batch.push(AttestEvent::Derive {
                            ts,
                            claim_id: old_id.clone(),
                            test_identity: identity.clone(),
                            body_hash: hash,
                            renamed_from: Some(old_identity.clone()),
                        });
                    }
                    None => {
                        counts.new += 1;
                        batch.push(AttestEvent::Derive {
                            ts,
                            claim_id: ClaimId::mint(),
                            test_identity: identity.clone(),
                            body_hash: hash,
                            renamed_from: None,
                        });
                    }
                }
            }
        }
    }
    (batch, counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(name: &str) -> TestIdentity {
        TestIdentity::new("t", name)
    }

    fn hash(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn fold_of(events: &[AttestEvent]) -> agentrec_core::attest::fold::FoldResult {
        fold_claims(events)
    }

    fn derive(id: &ClaimId, identity: &TestIdentity, h: u8) -> AttestEvent {
        AttestEvent::Derive {
            ts: 1,
            claim_id: id.clone(),
            test_identity: identity.clone(),
            body_hash: Some(hash(h)),
            renamed_from: None,
        }
    }

    #[test]
    fn a_first_derive_mints_one_claim_per_test_and_a_rerun_appends_nothing() {
        let ids = vec![ident("a"), ident("b")];
        let hashes: BodyHashes = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), hash(i as u8)))
            .collect();
        let (batch, counts) = plan_derives(&fold_of(&[]), &ids, &hashes);
        assert_eq!(batch.len(), 2);
        assert_eq!(counts.new, 2);

        let (again, counts) = plan_derives(&fold_of(&batch), &ids, &hashes);
        assert!(again.is_empty(), "{again:?}");
        assert_eq!(counts.unchanged, 2);
    }

    #[test]
    fn a_rename_with_an_unchanged_body_reuses_the_claim_id() {
        let old = ident("old_name");
        let id = ClaimId::mint();
        let prior = vec![derive(&id, &old, 7)];

        let new = ident("new_name");
        let hashes: BodyHashes = [(new.clone(), hash(7))].into_iter().collect();
        let (batch, counts) = plan_derives(&fold_of(&prior), std::slice::from_ref(&new), &hashes);
        assert_eq!(counts.renamed, 1);
        assert_eq!(counts.new, 0);
        match &batch[0] {
            AttestEvent::Derive {
                claim_id,
                renamed_from,
                test_identity,
                ..
            } => {
                assert_eq!(claim_id, &id);
                assert_eq!(renamed_from.as_ref(), Some(&old));
                assert_eq!(test_identity, &new);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_rename_with_a_changed_body_mints_a_fresh_claim() {
        let old = ident("old_name");
        let id = ClaimId::mint();
        let prior = vec![derive(&id, &old, 7)];
        let new = ident("new_name");
        let hashes: BodyHashes = [(new.clone(), hash(9))].into_iter().collect();
        let (batch, counts) = plan_derives(&fold_of(&prior), &[new], &hashes);
        assert_eq!(counts.new, 1);
        assert_eq!(counts.renamed, 0);
        assert!(matches!(
            &batch[0],
            AttestEvent::Derive { claim_id, renamed_from: None, .. } if claim_id != &id
        ));
    }

    #[test]
    fn a_body_change_without_a_rename_rehashes_the_same_claim() {
        let a = ident("a");
        let id = ClaimId::mint();
        let prior = vec![derive(&id, &a, 1)];
        let hashes: BodyHashes = [(a.clone(), hash(2))].into_iter().collect();
        let (batch, counts) = plan_derives(&fold_of(&prior), std::slice::from_ref(&a), &hashes);
        assert_eq!(counts.rehashed, 1);
        assert!(matches!(
            &batch[0],
            AttestEvent::Derive { claim_id, body_hash: Some(h), renamed_from: None, .. }
                if claim_id == &id && h == &hash(2)
        ));
    }

    /// Ambiguity in EITHER direction mints fresh rather than guessing.
    #[test]
    fn ambiguous_rename_candidates_mint_fresh_ids() {
        // Two vanished tests share one body.
        let (o1, o2) = (ident("old_1"), ident("old_2"));
        let (i1, i2) = (ClaimId::mint(), ClaimId::mint());
        let prior = vec![derive(&i1, &o1, 5), derive(&i2, &o2, 5)];
        let new = ident("new_1");
        let hashes: BodyHashes = [(new.clone(), hash(5))].into_iter().collect();
        let (_, counts) = plan_derives(&fold_of(&prior), &[new], &hashes);
        assert_eq!(counts.renamed, 0);
        assert_eq!(counts.new, 1);

        // One vanished test, TWO new tests with its body.
        let prior = vec![derive(&i1, &o1, 5)];
        let (n1, n2) = (ident("new_1"), ident("new_2"));
        let hashes: BodyHashes = [(n1.clone(), hash(5)), (n2.clone(), hash(5))]
            .into_iter()
            .collect();
        let (_, counts) = plan_derives(&fold_of(&prior), &[n1, n2], &hashes);
        assert_eq!(counts.renamed, 0);
        assert_eq!(counts.new, 2);
    }

    /// A claim in another TARGET is not a rename donor — otherwise deriving one
    /// crate would hand its neighbours' ids away.
    #[test]
    fn a_vanished_identity_in_another_target_is_never_a_donor() {
        let other = TestIdentity::new("other_target", "gone");
        let id = ClaimId::mint();
        let prior = vec![derive(&id, &other, 3)];
        let new = ident("fresh");
        let hashes: BodyHashes = [(new.clone(), hash(3))].into_iter().collect();
        let (_, counts) = plan_derives(&fold_of(&prior), &[new], &hashes);
        assert_eq!(counts.renamed, 0);
        assert_eq!(counts.new, 1);
    }
}
