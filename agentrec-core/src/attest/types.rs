//! Shared attest value types: claim identity, test identity, per-test results,
//! verdict kinds. Owned here (not in `cli`) because the fold and the adapters
//! both speak them and the dependency direction is `cli` → `agentrec-core`.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable surrogate claim id, `c_<ULID>` (spec decision 10).
///
/// Minted once at first derive and never changed for the life of the claim —
/// deliberately NOT a hash of [`TestIdentity`], because identity includes the
/// fn name and an identity-derived key cannot survive a rename. The `c_`
/// prefix distinguishes it from a turn id (`t_`), since an `evidence` event
/// carries both.
///
/// The fold never mints: it only reads ids off `derive` events.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClaimId(String);

impl ClaimId {
    /// Mint a fresh id for a newly derived claim.
    pub fn mint() -> Self {
        ClaimId(format!("c_{}", crate::id::ulid()))
    }

    /// Deterministic form for tests, mirroring [`crate::id::ulid_with`].
    pub fn mint_with(ms: u64, rand: &[u8; 10]) -> Self {
        ClaimId(format!("c_{}", crate::id::ulid_with(ms, rand)))
    }

    /// Adopt an id read off the wire.
    pub fn from_wire(s: impl Into<String>) -> Self {
        ClaimId(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ClaimId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A test, identified the way the cargo adapter enumerates it.
///
/// `target` is the cargo target/binary the test lives in; `fn_path` is the
/// module path of the test fn as libtest's `--list` prints it. Both components
/// are load-bearing: two identically-named fns in different targets are two
/// tests, and keying on `fn_path` alone silently cross-attributes their
/// evidence and verdicts.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TestIdentity {
    pub target: String,
    pub fn_path: String,
}

impl TestIdentity {
    pub fn new(target: impl Into<String>, fn_path: impl Into<String>) -> Self {
        TestIdentity {
            target: target.into(),
            fn_path: fn_path.into(),
        }
    }
}

impl fmt::Display for TestIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.target, self.fn_path)
    }
}

/// What libtest reported for one test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TestOutcome {
    Passed,
    Failed,
    Ignored,
}

/// Why a claim could not be verified at all — distinct from a test *result*.
///
/// [`RecipeInvalidCause::Ignored`] overlaps [`TestOutcome::Ignored`] on
/// purpose and they are kept as separate types: the outcome is what libtest
/// reported, the cause is the interpretation "this claim is unverifiable
/// because its test is `#[ignore]`d". Collapsing them would make a verdict
/// indistinguishable from an observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecipeInvalidCause {
    /// The target did not build.
    Build,
    /// The named test is not in the target's `--list` (renamed, deleted,
    /// cfg'd out) — a 0/0/0 run that a bare exit code would read as success.
    Missing,
    /// The test exists but is `#[ignore]`d, so no result was produced.
    Ignored,
    /// The harness produced no summary line — crash, `process::exit`, SIGABRT.
    Harness,
}

/// One test's captured result, as the adapter's libtest-output parser emits it
/// (Phase 1 Probe B, `docs/verify/attest-output-channel-spike.md`).
///
/// Deliberately carries no verdict candidacy: promoting a result to a verdict
/// is Phase 3/4's decision (a single failing run must never mint `claim-false`
/// — spec review finding 2), so no variant here says "confirmed" or
/// "claim-false".
///
/// `raw_blob` is a CAS hash string, never a path — core holds no file paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredResult {
    pub identity: TestIdentity,
    /// Absent when the run yielded no per-test result at all.
    #[serde(default)]
    pub outcome: Option<TestOutcome>,
    /// Set when the run was unusable rather than informative.
    #[serde(default)]
    pub recipe_invalid: Option<RecipeInvalidCause>,
    /// The parser failed closed: per-test lines disagreed with the summary
    /// line, or no summary line was found. The raw output is retained.
    #[serde(default)]
    pub parse_failed: bool,
    /// CAS hash of the retained raw output blob, when one was kept.
    #[serde(default)]
    pub raw_blob: Option<String>,
}

impl StructuredResult {
    /// A clean per-test outcome.
    pub fn outcome(identity: TestIdentity, outcome: TestOutcome) -> Self {
        StructuredResult {
            identity,
            outcome: Some(outcome),
            recipe_invalid: None,
            parse_failed: false,
            raw_blob: None,
        }
    }

    /// An unusable run.
    pub fn recipe_invalid(identity: TestIdentity, cause: RecipeInvalidCause) -> Self {
        StructuredResult {
            identity,
            outcome: None,
            recipe_invalid: Some(cause),
            parse_failed: false,
            raw_blob: None,
        }
    }
}

/// A replay verdict (spec decision 4).
///
/// Naming, normalized once: the wire kind is `flaky-observation`, the fold
/// state it produces is `FLAKY`, and decision 4's word "flaky" names that
/// state. One concept, three casings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "verdict")]
pub enum VerdictKind {
    /// Independent replay agreed. The only route to `CONFIRMED`.
    Confirmed,
    /// The test fails on clean replay. Permanent.
    ClaimFalse,
    /// Unverifiable this run. Retryable, never blocks a gate by itself.
    RecipeInvalid { cause: RecipeInvalidCause },
    /// Replays disagreed. Tracked statistically, never blocks.
    FlakyObservation,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_ids_are_prefixed_and_distinct_from_turn_ids() {
        let id = ClaimId::mint_with(1_000, &[0u8; 10]);
        assert!(id.as_str().starts_with("c_"));
        assert_eq!(id.as_str().len(), 28);
        assert!(!id.as_str().starts_with("t_"));
    }

    #[test]
    fn verdict_kinds_use_the_flaky_observation_wire_spelling() {
        let json = serde_json::to_string(&VerdictKind::FlakyObservation).unwrap();
        assert_eq!(json, r#"{"verdict":"flaky-observation"}"#);
        let json = serde_json::to_string(&VerdictKind::ClaimFalse).unwrap();
        assert_eq!(json, r#"{"verdict":"claim-false"}"#);
    }
}
