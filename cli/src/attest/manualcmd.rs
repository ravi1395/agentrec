//! `agentrec attest manual-declare --text <criterion> --severity blocking|fyi`
//! — the only hand-authored `attest.jsonl` event.
//!
//! A criterion no test covers (a README line, a live-app check) gets a claim
//! of its own so `attest review` can put it in front of a human and `attest
//! gate` can hold a release on it. The id is minted here and printed, because
//! nothing else can find the claim again: a manual claim has no test identity
//! to look it up by.

use crate::attest::lock::append_attest_locked;
use agentrec_core::attest::events::{AttestEvent, ManualSeverity};
use agentrec_core::attest::types::ClaimId;
use std::path::Path;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn run(root: &Path, text: &str, severity: ManualSeverity) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(
            "--text must not be empty: a criterion nobody can read is not reviewable".into(),
        );
    }
    let claim_id = ClaimId::mint();
    append_attest_locked(
        root,
        &[AttestEvent::ManualDeclare {
            ts: now_ms(),
            claim_id: claim_id.clone(),
            text: text.to_string(),
            severity,
        }],
    )?;
    println!("{claim_id}");
    Ok(())
}
