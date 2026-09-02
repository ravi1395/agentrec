//! `agentrec attest review [--json]` — evidence-first cards for the claims
//! that need a human.
//!
//! A card is a manual claim (`manual-declare` wrote its text and severity)
//! whose status is `DECLARED` (never answered) or `HUMAN(skip)` (deferred, so
//! ask again next time). `HUMAN(no)` is deliberately NOT re-asked: it is a
//! recorded rejection that blocks the gate, not an open question. `fyi` claims
//! ARE cards — they still need an answer; what `fyi` never does is block
//! (`ATTEST-FORMAT.md` § "Gate blocking, precisely", the single statement of
//! that rule).
//!
//! The card shows a DIFF POINTER, not a diff: the related turn id and the
//! `agentrec diff <turn>` command that renders it. Inlining a turn diff per
//! card would make review a `log.jsonl` reader for one line of text.
//!
//! Every answer is appended immediately through the shared append lock, so an
//! interrupt half-way keeps the answers already given.

use crate::attest::lock::{append_attest_locked, read_attest};
use crate::attest::statuscmd::{provenance, status_label, Provenance};
use agentrec_core::attest::events::{AttestEvent, HumanAnswer, ManualSeverity};
use agentrec_core::attest::fold::{fold_claims, ClaimState, ClaimStatus};
use agentrec_core::attest::types::ClaimId;
use std::io::BufRead;
use std::path::Path;

struct Card {
    claim_id: ClaimId,
    text: String,
    severity: ManualSeverity,
    status: String,
    prov: Provenance,
    evidence_runs: usize,
    verdicts: usize,
    stale_events: usize,
}

fn needs_a_human(claim: &ClaimState) -> bool {
    claim.manual_severity.is_some()
        && matches!(
            claim.status,
            ClaimStatus::Declared
                | ClaimStatus::Human {
                    answer: HumanAnswer::Skip
                }
        )
}

fn severity_label(severity: ManualSeverity) -> &'static str {
    match severity {
        ManualSeverity::Blocking => "blocking",
        ManualSeverity::Fyi => "fyi",
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn render(card: &Card, index: usize, total: usize) {
    println!(
        "[{}/{}] {} ({})",
        index + 1,
        total,
        card.claim_id,
        severity_label(card.severity)
    );
    println!("  criterion: {}", card.text);
    println!("  status: {}", card.status);
    match &card.prov.turn_id {
        Some(turn) => {
            println!("  evidence: turn {turn}{}", dirty_suffix(&card.prov));
            println!("  diff: agentrec diff {turn}");
        }
        None => {
            println!("  evidence: none recorded{}", dirty_suffix(&card.prov));
            println!("  diff: no related turn");
        }
    }
    if let Some(blob) = &card.prov.output_blob {
        println!("  output blob: {blob}");
    }
    println!(
        "  history: {} evidence, {} verdict(s), {} stale event(s)",
        card.evidence_runs, card.verdicts, card.stale_events
    );
}

fn dirty_suffix(prov: &Provenance) -> String {
    match prov.dirty {
        Some(true) => " (dirty tree)".to_string(),
        Some(false) => " (clean tree)".to_string(),
        None => String::new(),
    }
}

pub fn run(root: &Path, json: bool) -> Result<(), String> {
    let (events, _census) = read_attest(root)?;
    let folded = fold_claims(&events);
    let prov = provenance(&events);

    // The card set is computed ONCE. That is what makes `s` a skip rather than
    // an infinite re-ask: a `human` event appended below never re-enters this
    // list, and the skipped card returns only on the NEXT invocation.
    let cards: Vec<Card> = folded
        .claims
        .values()
        .filter(|c| needs_a_human(c))
        .map(|c| Card {
            claim_id: c.claim_id.clone(),
            text: c.manual_text.clone().unwrap_or_default(),
            severity: c.manual_severity.unwrap_or(ManualSeverity::Fyi),
            status: status_label(&c.status),
            prov: prov.get(&c.claim_id).cloned().unwrap_or_default(),
            evidence_runs: c.history.evidence,
            verdicts: c.history.verdicts,
            stale_events: c.history.stale_events,
        })
        .collect();

    if json {
        let payload: Vec<_> = cards
            .iter()
            .map(|c| {
                serde_json::json!({
                    "claim_id": c.claim_id.to_string(),
                    "text": c.text,
                    "severity": severity_label(c.severity),
                    "status": c.status,
                    "turn_id": c.prov.turn_id,
                    "output_blob": c.prov.output_blob,
                    "dirty": c.prov.dirty,
                    "evidence": c.evidence_runs,
                    "verdicts": c.verdicts,
                    "stale_events": c.stale_events,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return Ok(());
    }

    if cards.is_empty() {
        println!("no cards: nothing is waiting on a human");
        return Ok(());
    }

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let total = cards.len();
    for (i, card) in cards.iter().enumerate() {
        render(card, i, total);
        loop {
            println!("  [y]es / [n]o / [s]kip");
            // EOF (a pipe with nothing left, or a non-interactive caller) is
            // not an error: the remaining cards have been LISTED, which is
            // the useful thing a script can do with them, and nothing is
            // appended on a keypress nobody made.
            let Some(line) = lines.next() else {
                for (j, rest) in cards.iter().enumerate().skip(i + 1) {
                    render(rest, j, total);
                }
                println!("{} card(s) left unanswered (no input)", total - i);
                return Ok(());
            };
            let line = line.map_err(|e| format!("cannot read stdin: {e}"))?;
            let answer = match line.trim() {
                "y" => HumanAnswer::Yes,
                "n" => HumanAnswer::No,
                "s" => HumanAnswer::Skip,
                other => {
                    println!("  unrecognized answer {other:?}");
                    continue;
                }
            };
            // After `n`, the NEXT line is the note, stored verbatim. EOF here
            // means the note was never typed — an empty note, not a panic.
            let note = if answer == HumanAnswer::No {
                match lines.next() {
                    Some(line) => Some(line.map_err(|e| format!("cannot read stdin: {e}"))?),
                    None => Some(String::new()),
                }
            } else {
                None
            };
            append_attest_locked(
                root,
                &[AttestEvent::Human {
                    ts: now_ms(),
                    claim_id: card.claim_id.clone(),
                    answer,
                    note,
                }],
            )?;
            break;
        }
    }
    Ok(())
}
