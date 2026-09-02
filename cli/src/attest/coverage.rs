//! `.agentrec/attest-coverage.json` — the per-test coverage map the daemon
//! consults to decide which claims a write stales (Phase 1's ruling: per-test,
//! FILE granularity, with binary-spawning tests carrying a coarse `over_stale`
//! scope until Phase 4 can detect the two under-attribution channels).
//!
//! **STUB, Phase 4B chunk.** The producer (`attest verify`'s coverage capture)
//! is chunk A's; this file exists so chunk B's daemon consumer can compile and
//! be tested against the agreed shape. Chunk A's version replaces it. The two
//! public functions below are the contracted interface and their signatures
//! must not drift:
//!
//! ```ignore
//! pub fn load_coverage_map(root: &Path) -> Result<Option<CoverageMap>, String>;
//! pub fn claims_touched_by(map: &CoverageMap, rel_path: &str) -> Vec<ClaimId>;
//! ```

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

/// Every claim a write to `rel_path` stales: an exact match against an entry's
/// measured `files`, OR a match against one of its coarse `over_stale` globs.
/// Deduped, in map order.
pub fn claims_touched_by(map: &CoverageMap, rel_path: &str) -> Vec<ClaimId> {
    let mut out: Vec<ClaimId> = Vec::new();
    for entry in map.tests.values() {
        if entry_matches(entry, rel_path) && !out.contains(&entry.claim_id) {
            out.push(entry.claim_id.clone());
        }
    }
    out
}

fn entry_matches(entry: &CoverageEntry, rel_path: &str) -> bool {
    entry.files.iter().any(|f| f == rel_path) || over_stale_hit(entry, rel_path).is_some()
}

/// The `over_stale` glob that matches `rel_path`, if any. Gitignore semantics,
/// the same matcher `noise.rs::NoiseMatcher` and `daemon::IgnoreSet` use, so
/// `cli/src/**` behaves the way a reader of the config expects.
pub fn over_stale_hit<'a>(entry: &'a CoverageEntry, rel_path: &str) -> Option<&'a str> {
    let p = Path::new(rel_path);
    if p.is_absolute() {
        // The matcher asserts on rooted paths; an absolute path was never
        // going to match a repo-relative glob anyway.
        return None;
    }
    entry.over_stale.iter().map(String::as_str).find(|g| {
        compile(g)
            .map(|m| {
                matches!(
                    m.matched_path_or_any_parents(p, false),
                    ignore::Match::Ignore(_)
                )
            })
            .unwrap_or(false)
    })
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

        assert_eq!(claims_touched_by(&map, "src/a.rs"), vec![a]);
        assert_eq!(claims_touched_by(&map, "cli/src/x.rs"), vec![c]);
        // The ALLOW half: an unmapped path must match nothing, or a
        // match-everything matcher would pass the two asserts above.
        assert!(claims_touched_by(&map, "README.md").is_empty());
        assert!(claims_touched_by(&map, "src/a.rs.bak").is_empty());
    }
}
