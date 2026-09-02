//! `.agentrec/attest-coverage.json` — the per-test coverage map the daemon
//! consults to decide which claims a write stales (Phase 1's ruling: per-test,
//! FILE granularity, with binary-spawning tests carrying a coarse `over_stale`
//! scope until Phase 4 can detect the two under-attribution channels).
//!
//! The producer is `coveragecmd.rs` (`attest coverage`); this file is the
//! READER the daemon consults. The contracted interface:
//!
//! ```ignore
//! pub fn load_coverage_map(root: &Path) -> Result<Option<CoverageMap>, String>;
//! pub fn claims_touched_by(map: &CoverageMap, rel_path: &str) -> Vec<(ClaimId, MatchKind)>;
//! ```
//!
//! `claims_touched_by` returns the MATCH KIND with each claim so the daemon can
//! name the `stale` cause from what actually matched, rather than re-deriving
//! the discrimination from the entry's fields a second time and risking the two
//! copies drifting.

use agentrec_core::attest::types::ClaimId;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The map's default location, relative to the repo root. Overridable via
/// `config.toml`'s `attest_coverage_path`.
pub const DEFAULT_COVERAGE_REL: &str = ".agentrec/attest-coverage.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageMap {
    pub version: u32,
    pub granularity: String,
    /// Keyed by the test's identity string, as the producer writes it.
    pub tests: BTreeMap<String, CoverageEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageEntry {
    pub claim_id: ClaimId,
    /// Repo-relative, forward-slash paths this test's run actually covered.
    #[serde(default)]
    pub files: Vec<String>,
    /// Coarse gitignore-style globs that stale this claim because its measured
    /// coverage is known-incomplete (spawned-binary tests).
    #[serde(default)]
    pub over_stale: Vec<String>,
    #[serde(default)]
    pub captured_at: u64,
    #[serde(default)]
    pub commit: String,
}

/// Reads the map through fsguard. A missing file is `Ok(None)` — no coverage
/// captured yet is a normal state, not a failure. Malformed JSON IS an error:
/// silently treating a corrupt map as "no coverage" would turn every claim
/// permanently un-staleable with no signal anywhere.
///
/// Chunk B's daemon resolves the configurable `attest_coverage_path` and calls
/// [`load_coverage_map_at`] directly, so this contracted entry point has no
/// caller until chunk A lands.
#[allow(dead_code)]
pub fn load_coverage_map(root: &Path) -> Result<Option<CoverageMap>, String> {
    load_coverage_map_at(&root.join(DEFAULT_COVERAGE_REL))
}

/// Same, for an explicit path (`config.toml`'s `attest_coverage_path`).
pub fn load_coverage_map_at(path: &Path) -> Result<Option<CoverageMap>, String> {
    let body = match agentrec_core::fsguard::read_regular_to_string(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    serde_json::from_str(&body)
        .map(Some)
        .map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

/// WHY a write to a path stales a claim — and therefore which `stale` cause the
/// daemon writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatchKind {
    /// The path is one of the entry's MEASURED `files`.
    ExactFile,
    /// The path matched one of the entry's coarse `over_stale` globs, carried
    /// here so the cause can name the scope that fired.
    OverStale(String),
}

/// Every claim a write to `rel_path` stales, with the reason it matched: an
/// exact hit on an entry's measured `files`, or a hit on one of its coarse
/// `over_stale` globs. Deduped by claim, in map order — the FIRST matching
/// entry decides a claim's kind, which is the same first-wins rule the daemon
/// used when it re-derived this itself.
pub fn claims_touched_by(map: &CoverageMap, rel_path: &str) -> Vec<(ClaimId, MatchKind)> {
    let mut out: Vec<(ClaimId, MatchKind)> = Vec::new();
    for entry in map.tests.values() {
        let Some(kind) = match_kind(entry, rel_path) else {
            continue;
        };
        if out.iter().any(|(id, _)| id == &entry.claim_id) {
            continue;
        }
        out.push((entry.claim_id.clone(), kind));
    }
    out
}

/// How `rel_path` matches this entry, if at all. Exact files are checked first:
/// a path that is both measured AND inside a coarse scope is genuinely
/// measured, and reporting it as `coverage-incomplete` would understate what is
/// known.
///
/// Glob matching uses gitignore semantics — the same matcher
/// `noise.rs::NoiseMatcher` and `daemon::IgnoreSet` use — so `cli/src/**`
/// behaves the way a reader of the config expects.
pub fn match_kind(entry: &CoverageEntry, rel_path: &str) -> Option<MatchKind> {
    if entry.files.iter().any(|f| f == rel_path) {
        return Some(MatchKind::ExactFile);
    }
    let p = Path::new(rel_path);
    if p.is_absolute() {
        // The matcher asserts on rooted paths; an absolute path was never
        // going to match a repo-relative glob anyway.
        return None;
    }
    entry
        .over_stale
        .iter()
        .find(|g| {
            compile(g)
                .map(|m| {
                    matches!(
                        m.matched_path_or_any_parents(p, false),
                        ignore::Match::Ignore(_)
                    )
                })
                .unwrap_or(false)
        })
        .map(|g| MatchKind::OverStale(g.clone()))
}

fn compile(glob: &str) -> Option<Gitignore> {
    let mut b = GitignoreBuilder::new("");
    // A malformed individual pattern matches nothing rather than poisoning the
    // whole map — same per-pattern degrade as `NoiseMatcher::build`.
    b.add_line(None, glob).ok()?;
    b.build().ok()
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures; no attacker-supplied FIFO can
//    block these. Production reads stay lint-enforced (clippy.toml).
mod tests {
    use super::*;

    fn map_json() -> String {
        r#"{"version":1,"granularity":"file","tests":{
            "t::a":{"claim_id":"c_00000000010W3GE1R70W3GE1R7","files":["src/a.rs"],"over_stale":[],"captured_at":1,"commit":"abc"},
            "t::c":{"claim_id":"c_00000000010W3GE1R70W3GE1R8","files":[],"over_stale":["cli/src/**"],"captured_at":1,"commit":"abc"}
        }}"#
        .into()
    }

    #[test]
    fn a_missing_map_is_none_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load_coverage_map(tmp.path()).unwrap(), None);
    }

    #[test]
    fn a_malformed_map_is_an_error_not_a_silent_none() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".agentrec")).unwrap();
        std::fs::write(tmp.path().join(DEFAULT_COVERAGE_REL), "{not json").unwrap();
        assert!(load_coverage_map(tmp.path()).is_err());
    }

    #[test]
    fn exact_files_and_over_stale_globs_both_match_and_nothing_else_does() {
        let map: CoverageMap = serde_json::from_str(&map_json()).unwrap();
        let a = ClaimId::parse("c_00000000010W3GE1R70W3GE1R7").unwrap();
        let c = ClaimId::parse("c_00000000010W3GE1R70W3GE1R8").unwrap();

        assert_eq!(
            claims_touched_by(&map, "src/a.rs"),
            vec![(a, MatchKind::ExactFile)]
        );
        assert_eq!(
            claims_touched_by(&map, "cli/src/x.rs"),
            vec![(c, MatchKind::OverStale("cli/src/**".to_string()))]
        );
        // The ALLOW half: an unmapped path must match nothing, or a
        // match-everything matcher would pass the two asserts above.
        assert!(claims_touched_by(&map, "README.md").is_empty());
        assert!(claims_touched_by(&map, "src/a.rs.bak").is_empty());
    }

    /// AC-ATTEST-P4C-5. The kind is what the daemon writes its `stale` cause
    /// from, so the two arms must be distinguishable AND the glob that fired
    /// must travel with the `OverStale` arm — a bare bool would lose the scope
    /// the cause names.
    #[test]
    fn match_kind_discriminates_an_exact_file_from_an_over_stale_glob() {
        let map: CoverageMap = serde_json::from_str(&map_json()).unwrap();
        let exact = &map.tests["t::a"];
        let coarse = &map.tests["t::c"];

        assert_eq!(match_kind(exact, "src/a.rs"), Some(MatchKind::ExactFile));
        assert_eq!(
            match_kind(coarse, "cli/src/x.rs"),
            Some(MatchKind::OverStale("cli/src/**".to_string()))
        );
        // The ALLOW half: neither entry matches a path outside its own scope,
        // or the two asserts above would pass under a match-everything kind.
        assert_eq!(match_kind(exact, "cli/src/x.rs"), None);
        assert_eq!(match_kind(coarse, "src/a.rs"), None);
    }
}
