//! `attest.jsonl` event kinds and tolerant line parsing.
//!
//! One JSON object per line, append-only, multiple writers. Every event
//! carries `kind` (the serde tag) and `ts` (unix milliseconds). Consumers
//! tolerate unknown fields (serde defaults) and unknown kinds (counted, not
//! fatal) — the same posture `record.rs` takes for `log.jsonl`.

use super::types::{ClaimId, StructuredResult, TestIdentity, VerdictKind};
use serde::{Deserialize, Serialize};

/// Hex serde for the 32-byte body hash: a JSON string, not a 32-element array
/// of numbers, so an `attest.jsonl` line stays readable and compact.
mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<[u8; 32]>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(bytes) => {
                let mut out = String::with_capacity(64);
                for b in bytes {
                    out.push_str(&format!("{b:02x}"));
                }
                s.serialize_some(&out)
            }
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[u8; 32]>, D::Error> {
        let opt = Option::<String>::deserialize(d)?;
        let Some(text) = opt else { return Ok(None) };
        if text.len() != 64 {
            return Err(serde::de::Error::custom("body_hash must be 64 hex chars"));
        }
        let mut out = [0u8; 32];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
                .map_err(|e| serde::de::Error::custom(format!("body_hash: {e}")))?;
        }
        Ok(Some(out))
    }
}

/// Why a claim was staled (`stale` event cause).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "cause")]
pub enum StaleCause {
    /// A file inside the claim's measured coverage set was written.
    FileWrite { path: String },
    /// The claim's coverage map is known-incomplete, so any write in the
    /// coarse scope stales it (Phase 1 AC-ATTEST-P1-3's interim rule).
    CoverageIncomplete { scope: String },
}

/// A human's answer on a manual-attestation card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HumanAnswer {
    /// `y` — attested.
    Yes,
    /// `n` — rejected.
    No,
    /// `s` — skipped/waived for now.
    Skip,
}

/// Whether a manual criterion blocks the gate or is informational only
/// (spec decision 8(e); `fyi` never nags).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManualSeverity {
    Blocking,
    Fyi,
}

/// One line of `attest.jsonl`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AttestEvent {
    /// A test was discovered; a claim is created, or an existing claim's
    /// identity is remapped when `renamed_from` is set. The writer
    /// (`attest derive`, Phase 3) resolves renames and writes the OLD claim's
    /// id here — the fold is a dumb applier and detects nothing.
    Derive {
        ts: u64,
        claim_id: ClaimId,
        test_identity: TestIdentity,
        #[serde(default, with = "hex32", skip_serializing_if = "Option::is_none")]
        body_hash: Option<[u8; 32]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        renamed_from: Option<TestIdentity>,
    },
    /// A captured run, joined to the turn it happened inside. An author's own
    /// run is evidence and never a verdict — the two kinds are disjoint.
    Evidence {
        ts: u64,
        claim_id: ClaimId,
        /// The open turn this run happened inside, when there was one.
        #[serde(default)]
        turn_id: Option<String>,
        /// The working tree had uncommitted changes during the run.
        #[serde(default)]
        dirty: bool,
        /// CAS hash of the captured output blob.
        #[serde(default)]
        output_blob: Option<String>,
        result: StructuredResult,
    },
    /// An independent replay result — the only kind that can move a claim to
    /// `CONFIRMED`.
    Verdict {
        ts: u64,
        claim_id: ClaimId,
        #[serde(flatten)]
        verdict: VerdictKind,
        /// The commit the replay tree was extracted from.
        replay_commit: String,
    },
    /// The daemon's note-taking: a write landed in this claim's scope.
    Stale {
        ts: u64,
        claim_id: ClaimId,
        #[serde(flatten)]
        cause: StaleCause,
    },
    /// A manual card review outcome.
    Human {
        ts: u64,
        claim_id: ClaimId,
        answer: HumanAnswer,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// The only hand-authored event: an un-testable criterion's text.
    ManualDeclare {
        ts: u64,
        claim_id: ClaimId,
        text: String,
        severity: ManualSeverity,
    },
}

impl AttestEvent {
    /// The claim every event kind refers to.
    pub fn claim_id(&self) -> &ClaimId {
        match self {
            AttestEvent::Derive { claim_id, .. }
            | AttestEvent::Evidence { claim_id, .. }
            | AttestEvent::Verdict { claim_id, .. }
            | AttestEvent::Stale { claim_id, .. }
            | AttestEvent::Human { claim_id, .. }
            | AttestEvent::ManualDeclare { claim_id, .. } => claim_id,
        }
    }

    pub fn ts(&self) -> u64 {
        match self {
            AttestEvent::Derive { ts, .. }
            | AttestEvent::Evidence { ts, .. }
            | AttestEvent::Verdict { ts, .. }
            | AttestEvent::Stale { ts, .. }
            | AttestEvent::Human { ts, .. }
            | AttestEvent::ManualDeclare { ts, .. } => *ts,
        }
    }
}

/// One parsed line of `attest.jsonl`, mirroring `record::ParsedLine`: the two
/// failure buckets prescribe different user actions — an unknown kind means
/// "a newer agentrec wrote this, upgrade", torn JSON means "your log took
/// damage" — so they are counted separately rather than merged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedAttestLine {
    Event(Box<AttestEvent>),
    /// Well-formed JSON whose `kind` this binary does not implement.
    UnknownKind,
    /// Not interpretable at all.
    Unparsed,
    Blank,
}

/// Every `kind` string this binary implements — the source of truth
/// [`parse_line`] uses to tell "a newer agentrec wrote this" from "this line
/// is damaged". Kept in sync with [`AttestEvent`] by
/// `tests::ac14_every_variants_kind_string_is_in_the_known_list`, which reds
/// the day a seventh kind lands without an entry here.
pub const KNOWN_EVENT_KINDS: [&str; 6] = [
    "derive",
    "evidence",
    "verdict",
    "stale",
    "human",
    "manual-declare",
];

/// Parse one line, tolerating both unknown kinds and torn JSON. Never returns
/// an error: a single bad line must not cost the caller the whole log.
pub fn parse_line(line: &str) -> ParsedAttestLine {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ParsedAttestLine::Blank;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return ParsedAttestLine::Unparsed;
    };
    match serde_json::from_value::<AttestEvent>(value.clone()) {
        Ok(ev) => ParsedAttestLine::Event(Box::new(ev)),
        Err(_) => {
            // A `kind` this binary does not know is tolerable; anything else
            // (a known kind missing required fields) is damage.
            let known = matches!(
                value.get("kind").and_then(|k| k.as_str()),
                Some("derive" | "evidence" | "verdict" | "stale" | "human" | "manual-declare")
            );
            if known {
                ParsedAttestLine::Unparsed
            } else {
                ParsedAttestLine::UnknownKind
            }
        }
    }
}

/// Counts from a tolerant parse of a whole log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AttestParseCensus {
    pub events: usize,
    pub unknown_kind_lines: usize,
    pub unparsed_lines: usize,
}

/// Parse a whole log body, returning the events plus the census of what was
/// tolerated. Unknown kinds and torn lines are counted, never dropped
/// silently.
pub fn parse_log(body: &str) -> (Vec<AttestEvent>, AttestParseCensus) {
    let mut events = Vec::new();
    let mut census = AttestParseCensus::default();
    for line in body.lines() {
        match parse_line(line) {
            ParsedAttestLine::Event(ev) => {
                census.events += 1;
                events.push(*ev);
            }
            ParsedAttestLine::UnknownKind => census.unknown_kind_lines += 1,
            ParsedAttestLine::Unparsed => census.unparsed_lines += 1,
            ParsedAttestLine::Blank => {}
        }
    }
    (events, census)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attest::types::{RecipeInvalidCause, TestOutcome};

    fn ident() -> TestIdentity {
        TestIdentity::new("agentrec--import_claude", "ac3_zero_bytes")
    }

    fn one_of_every_kind() -> Vec<AttestEvent> {
        let claim_id = ClaimId::mint_with(1, &[7u8; 10]);
        vec![
            AttestEvent::Derive {
                ts: 1,
                claim_id: claim_id.clone(),
                test_identity: ident(),
                body_hash: Some([0xabu8; 32]),
                renamed_from: Some(TestIdentity::new("agentrec--import_claude", "old_name")),
            },
            AttestEvent::Derive {
                ts: 2,
                claim_id: claim_id.clone(),
                test_identity: ident(),
                body_hash: None,
                renamed_from: None,
            },
            AttestEvent::Evidence {
                ts: 3,
                claim_id: claim_id.clone(),
                turn_id: Some("t_01ARZ3NDEKTSV4RRFFQ69G5FAV".into()),
                dirty: true,
                output_blob: Some("deadbeef".into()),
                result: StructuredResult::outcome(ident(), TestOutcome::Passed),
            },
            AttestEvent::Evidence {
                ts: 4,
                claim_id: claim_id.clone(),
                turn_id: None,
                dirty: false,
                output_blob: None,
                result: StructuredResult::recipe_invalid(ident(), RecipeInvalidCause::Harness),
            },
            AttestEvent::Verdict {
                ts: 5,
                claim_id: claim_id.clone(),
                verdict: VerdictKind::Confirmed,
                replay_commit: "abc123".into(),
            },
            AttestEvent::Verdict {
                ts: 6,
                claim_id: claim_id.clone(),
                verdict: VerdictKind::RecipeInvalid {
                    cause: RecipeInvalidCause::Missing,
                },
                replay_commit: "abc123".into(),
            },
            AttestEvent::Verdict {
                ts: 7,
                claim_id: claim_id.clone(),
                verdict: VerdictKind::FlakyObservation,
                replay_commit: "abc123".into(),
            },
            AttestEvent::Verdict {
                ts: 8,
                claim_id: claim_id.clone(),
                verdict: VerdictKind::ClaimFalse,
                replay_commit: "abc123".into(),
            },
            AttestEvent::Stale {
                ts: 9,
                claim_id: claim_id.clone(),
                cause: StaleCause::FileWrite {
                    path: "cli/src/importcmd.rs".into(),
                },
            },
            AttestEvent::Stale {
                ts: 10,
                claim_id: claim_id.clone(),
                cause: StaleCause::CoverageIncomplete {
                    scope: "cli/src/**".into(),
                },
            },
            AttestEvent::Human {
                ts: 11,
                claim_id: claim_id.clone(),
                answer: HumanAnswer::Yes,
                note: Some("checked by hand".into()),
            },
            AttestEvent::ManualDeclare {
                ts: 12,
                claim_id,
                text: "the README install line is correct".into(),
                severity: ManualSeverity::Blocking,
            },
        ]
    }

    #[test]
    fn ac7_every_event_kind_round_trips_value_equal() {
        for ev in one_of_every_kind() {
            let line = serde_json::to_string(&ev).unwrap();
            assert!(!line.contains('\n'), "an event must be one JSONL line");
            match parse_line(&line) {
                ParsedAttestLine::Event(back) => assert_eq!(*back, ev, "round-trip: {line}"),
                other => panic!("expected an event for {line}, got {other:?}"),
            }
            // Re-serializing the parsed value reproduces the same bytes.
            let again = serde_json::to_string(&ev).unwrap();
            assert_eq!(again, line);
        }
    }

    #[test]
    fn ac7_unknown_kind_line_is_tolerated_and_counted() {
        let known = serde_json::to_string(&one_of_every_kind()[1]).unwrap();
        let body = format!(
            "{known}\n{{\"kind\":\"from-the-future\",\"ts\":9,\"claim_id\":\"c_X\"}}\n\nnot json\n"
        );
        let (events, census) = parse_log(&body);
        assert_eq!(events.len(), 1);
        assert_eq!(census.events, 1);
        assert_eq!(census.unknown_kind_lines, 1);
        assert_eq!(census.unparsed_lines, 1);
    }

    #[test]
    fn ac14_every_variants_kind_string_is_in_the_known_list() {
        let mut seen = std::collections::BTreeSet::new();
        for ev in one_of_every_kind() {
            let value: serde_json::Value = serde_json::to_value(&ev).unwrap();
            let kind = value
                .get("kind")
                .and_then(|k| k.as_str())
                .expect("every event serializes with a kind tag")
                .to_string();
            assert!(
                KNOWN_EVENT_KINDS.contains(&kind.as_str()),
                "{kind} is written by AttestEvent but missing from KNOWN_EVENT_KINDS, \
                 so a malformed line of that kind would be misreported as a newer schema"
            );
            seen.insert(kind);
        }
        assert_eq!(
            seen.len(),
            KNOWN_EVENT_KINDS.len(),
            "the fixture must cover every kind in the known list: saw {seen:?}"
        );
    }

    #[test]
    fn a_known_kind_written_malformed_is_damage_not_an_unknown_kind() {
        let (_, census) = parse_log("{\"kind\":\"verdict\",\"ts\":1}\n");
        assert_eq!(census.unparsed_lines, 1);
        assert_eq!(census.unknown_kind_lines, 0);
    }

    #[test]
    fn unknown_fields_on_a_known_kind_are_tolerated() {
        let line = "{\"kind\":\"stale\",\"ts\":1,\"claim_id\":\"c_X\",\"cause\":\"file-write\",\"path\":\"a.rs\",\"future_field\":7}";
        assert!(matches!(parse_line(line), ParsedAttestLine::Event(_)));
    }
}
