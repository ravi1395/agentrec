//! NF-A/NF-B (`noise_globs` fold): a declarative, config-driven set of
//! gitignore-style glob patterns whose matching `FileEntry` paths are folded
//! out of `log`/`show`'s HUMAN-rendered output — never `--json`, never
//! `diff`/`blame`/`undo` (those stay attribution/revert paths and must never
//! consult this file; see `cli/src/cmds.rs::log` and
//! `cli/src/readcmds.rs::show` for the two call sites, both display-only).
//!
//! Precedent: `log` already hides an entire class of turns by default — git
//! turns (`cmds::log`'s `t.tool.as_deref() != Some("git")` filter,
//! `--all` reveals them). This is the same idea one level down: from turns to
//! individual file entries within a turn, with `--all-files` as the reveal
//! flag. Declarative-only, never inferred from write rate, size, or
//! tracked-status — a heuristic here would silently hide real activity.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::Path;

/// Read `.agentrec/config.toml`'s `noise_globs` array via
/// [`crate::config::load_or_default`] — B2 amendment: this used to be a
/// hand-rolled single-line-array scanner limited to `noise_globs = ["a",
/// "b"]` on one line; the real `toml` parser now backing this also handles a
/// multi-line array, which is a strict widening (nothing that parsed before
/// stops parsing). Absent file, absent key, `noise_globs = []`, or a
/// file-level TOML parse error all yield an empty vec, the "identical to
/// today" baseline (NF1).
pub(crate) fn read_noise_globs(root: &Path) -> Vec<String> {
    crate::config::load_or_default(root).noise_globs
}

/// Compiled matcher over a repo's `noise_globs`. `NoiseMatcher::build`
/// returns `None` for an empty list so callers skip matching entirely in the
/// common (unconfigured) case, rather than build and consult a no-op
/// matcher on every turn/file.
pub(crate) struct NoiseMatcher(Gitignore);

impl NoiseMatcher {
    pub(crate) fn build(root: &Path, globs: &[String]) -> Option<Self> {
        if globs.is_empty() {
            return None;
        }
        let mut builder = GitignoreBuilder::new(root);
        for g in globs {
            // A malformed individual pattern is skipped, not fatal — a
            // hand-edited config should degrade per-line, never crash the
            // whole read command over one bad glob.
            let _ = builder.add_line(None, g);
        }
        builder.build().ok().map(NoiseMatcher)
    }

    /// `path` is a `FileEntry.path` — normally repo-relative, forward-slash.
    /// Uses `matched_path_or_any_parents` (not `matched`) so a
    /// directory-style pattern like `.remember/` or bare `.remember` folds
    /// every file beneath it, not only paths that literally repeat the
    /// pattern text — mirrors the idiom `daemon::IgnoreSet::is_ignored`
    /// already uses for the same reason.
    ///
    /// `log.jsonl` is a trust boundary, not a schema-enforced one — nothing
    /// stops a hand-edited or foreign-tool-written line from carrying an
    /// absolute `FileEntry.path`. `Gitignore::matched_path_or_any_parents`
    /// panics (`assert!(!path.has_root())`) on a path that shares no common
    /// prefix with the matcher's root, which an absolute path never will
    /// against a repo root — so that case is rejected up front rather than
    /// handed to the matcher. Folding nothing is the safe direction (same
    /// posture as `build`'s per-line degrade): a foreign
    /// absolute path was never going to match a repo-relative noise glob
    /// anyway, and `log`/`show` must never crash over a `noise_globs`
    /// config that has nothing to do with this entry.
    pub(crate) fn is_noise(&self, path: &str) -> bool {
        let p = Path::new(path);
        if p.is_absolute() {
            return false;
        }
        matches!(
            self.0.matched_path_or_any_parents(p, false),
            ignore::Match::Ignore(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_noise_globs_missing_config_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_noise_globs(tmp.path()).is_empty());
    }

    #[test]
    fn read_noise_globs_parses_config_line() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".agentrec")).unwrap();
        std::fs::write(
            tmp.path().join(".agentrec/config.toml"),
            "ttl_days = 90\nnoise_globs = [\".remember/**\", \".code-review-graph/**\"]\n",
        )
        .unwrap();
        assert_eq!(
            read_noise_globs(tmp.path()),
            vec![
                ".remember/**".to_string(),
                ".code-review-graph/**".to_string()
            ]
        );
    }

    // NF2 precondition (per the round's TDD instructions: this test must
    // assert the glob genuinely matches the fixture path before any renderer
    // test relies on it) — proves the matcher actually fires for all three
    // natural spellings a user might reasonably write for "everything under
    // .remember/", not only the one the implementation happens to prefer.
    #[test]
    fn is_noise_matches_directory_pattern_spellings() {
        let tmp = tempfile::tempdir().unwrap();
        for pattern in [".remember/**", ".remember/", ".remember"] {
            let matcher = NoiseMatcher::build(tmp.path(), &[pattern.to_string()])
                .unwrap_or_else(|| panic!("pattern {pattern:?} must build a matcher"));
            assert!(
                matcher.is_noise(".remember/session.log"),
                "pattern {pattern:?} must match .remember/session.log"
            );
        }
    }

    #[test]
    fn is_noise_does_not_match_unrelated_path() {
        let tmp = tempfile::tempdir().unwrap();
        let matcher = NoiseMatcher::build(tmp.path(), &[".remember/**".to_string()]).unwrap();
        assert!(!matcher.is_noise("src/main.rs"));
    }

    // Advisor-surfaced (log.jsonl is a trust boundary, not schema-enforced):
    // `matched_path_or_any_parents` panics on a path that shares no common
    // prefix with the matcher root, which an absolute path never will —
    // `is_noise` must degrade to "not noise" instead of crashing `log`/`show`
    // over a foreign or hand-edited log line.
    #[test]
    fn is_noise_does_not_panic_on_absolute_path() {
        let tmp = tempfile::tempdir().unwrap();
        let matcher = NoiseMatcher::build(tmp.path(), &[".remember/**".to_string()]).unwrap();
        assert!(!matcher.is_noise("/etc/passwd"));
    }

    #[test]
    fn build_returns_none_for_empty_globs() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(NoiseMatcher::build(tmp.path(), &[]).is_none());
    }
}
