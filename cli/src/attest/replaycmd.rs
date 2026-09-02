//! `agentrec attest verify [--all-stale | <claim_id>...]` — the INDEPENDENT
//! replay, and the only writer of `verdict` events.
//!
//! The verdict policy is stated ONCE in `ATTEST-FORMAT.md` § "Verdict policy".
//! This module implements it ([`decide`], [`replay`]) and does not restate it.
//!
//! # Why `git archive`, never `git worktree add`
//!
//! Plan "Infeasible/rejected": `git worktree add` writes metadata INTO the
//! production repo's `.git`, so a verify would mutate the tree it is supposed
//! to be independent of, and a killed verify would leave a registered worktree
//! behind. `git archive <commit> | tar -x` produces a plain directory carrying
//! only committed content and no `.git` at all.
//!
//! That missing `.git` is a real constraint on what can be replayed: a test
//! that expects to run inside a git repository will fail in the extract for a
//! reason that has nothing to do with the claim, and `claim-false` is
//! PERMANENT. This repo's fixtures were spot-checked as `git_init`-ing their
//! own tempdirs, and one real repo test binary was run inside an extract by
//! hand (`docs/verify/attest-verify-e2e.md`) — a spot check plus one binary,
//! not an exhaustive proof.
//!
//! # A dirty tree refuses
//!
//! The extract carries COMMITTED bytes. Minting a verdict while the working
//! tree holds uncommitted changes would stamp `replay_commit` with a commit
//! that never contained what the author was actually looking at.
//!
//! # Environment: scrubbed
//!
//! The replay runs with `env_clear()` plus the pinned allowlist — see
//! `adapter_cargo::ENV_ALLOWLIST` for the names and `ATTEST-FORMAT.md`
//! § "Replay environment" for why. An inherited variable can change what a
//! test does, and a verdict is meant to be a property of the committed bytes.
//! `adapter_cargo::RunEnv` carries this as an explicit parameter, so every
//! other adapter caller keeps inheriting.
//!
//! Scoped precisely: the scrub covers the cargo invocations that BUILD and RUN
//! the test. The `--list` membership precheck (`adapter_cargo::list_tests`)
//! spawns the built libtest binary with the inherited environment — it
//! enumerates test names and runs no test body, so it cannot carry an
//! inherited variable into a verdict.
//!
//! Independent of the scrub, the extract's `target/` is a SYMLINK to a
//! per-commit cache under `.agentrec/attest-target/`, so
//! `crate_root.join("target")` resolves into a shared build cache and replays
//! do not rebuild the world each time. That cache is shared across replays and
//! is NOT part of the pinned tree.

use crate::attest::adapter_cargo::{Adapter, CargoAdapter, RunEnv, RunFilter};
use crate::attest::coveragecmd::attest_target_dir;
use crate::attest::lock::{append_attest_locked, read_attest};
use agentrec_core::attest::events::AttestEvent;
use agentrec_core::attest::fold::fold_claims;
use agentrec_core::attest::types::{
    ClaimId, StructuredResult, TestIdentity, TestOutcome, VerdictKind,
};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Total runs allowed for a claim whose first run FAILED. See the format doc.
const MAX_RUNS: usize = 3;

pub fn run(root: &Path, all_stale: bool, ids: &[String]) -> Result<(), String> {
    if !all_stale && ids.is_empty() {
        return Err("specify --all-stale or one or more claim ids".to_string());
    }
    ensure_clean_tree(root)?;
    let commit = head_commit(root)?;

    let (events, _) = read_attest(root)?;
    let folded = fold_claims(&events);

    let mut selected: Vec<(ClaimId, TestIdentity)> = Vec::new();
    for (claim_id, state) in &folded.claims {
        let wanted = if all_stale {
            state.is_stale()
        } else {
            ids.iter().any(|i| i == claim_id.as_str())
        };
        if !wanted {
            continue;
        }
        match &state.test_identity {
            Some(identity) => selected.push((claim_id.clone(), identity.clone())),
            // A manual claim has no test to replay. Named, not silently
            // dropped — a user who asked for an id deserves to know why
            // nothing happened.
            None => println!("{claim_id} skipped: no test identity (manual claim)"),
        }
    }
    if selected.is_empty() {
        println!("no claims to verify");
        return Ok(());
    }

    // Held for extract + every replay: the extract directory is SHARED and
    // stable per root, so a second concurrent verify would delete this one's
    // tree out from under a running cargo. This serializes verifies in a root;
    // it does not make them concurrent.
    let _verify_lock = crate::attest::lock::lock_exclusive_at(&verify_lock_path(root))?;

    let extract = extract_commit(root, &commit)?;

    // Appended PER CLAIM, not batched at the end. A batch discards every
    // verdict already computed when a later claim errors — and an adapter
    // error is reachable (a claim naming a target absent from the extract),
    // so a long `--all-stale` run could do all the work and persist none of
    // it. One append per decided verdict costs one lock acquisition each and
    // makes partial progress durable.
    for (claim_id, identity) in &selected {
        let (verdict, runs) = replay(&extract, identity)?;
        println!(
            "{claim_id} {}::{} -> {} ({runs} run{})",
            identity.target,
            identity.fn_path,
            describe(&verdict),
            if runs == 1 { "" } else { "s" }
        );
        append_attest_locked(
            root,
            &[AttestEvent::Verdict {
                ts: now_ms(),
                claim_id: claim_id.clone(),
                verdict,
                replay_commit: commit.clone(),
            }],
        )?;
    }
    Ok(())
}

/// Serializes concurrent `attest verify` runs in one root. Separate from
/// `attest.lock`, which is held only for the duration of a single append —
/// holding that one across a multi-minute replay would block the daemon's
/// `stale` appends for the whole run.
fn verify_lock_path(root: &Path) -> PathBuf {
    crate::agentrec_dir(root).join("attest-verify.lock")
}

fn describe(v: &VerdictKind) -> String {
    match v {
        VerdictKind::Confirmed => "confirmed".to_string(),
        VerdictKind::ClaimFalse => "claim-false".to_string(),
        VerdictKind::FlakyObservation => "flaky-observation".to_string(),
        // The WIRE casing, not Rust's `{:?}`. `RecipeInvalidCause` is
        // `#[serde(rename_all = "kebab-case")]`, so the event says `ignored`
        // while Debug says `Ignored`; printing Debug made stdout and the
        // appended line disagree about the same field. Derived from serde
        // rather than hand-matched, so a new variant cannot drift.
        VerdictKind::RecipeInvalid { cause } => {
            let wire = serde_json::to_value(cause)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| format!("{cause:?}"));
            format!("recipe-invalid (cause: {wire})")
        }
    }
}

/// Run the staged pipeline under the retry policy. Returns the verdict and the
/// number of runs it took — the count has NO wire slot, so it reaches the user
/// only through the caller's stdout line.
fn replay(extract: &Path, identity: &TestIdentity) -> Result<(VerdictKind, usize), String> {
    let filter = RunFilter::Exact {
        target: identity.target.clone(),
        fn_path: identity.fn_path.clone(),
    };
    let mut runs = 0usize;
    let mut any_failed = false;
    loop {
        let outcome = CargoAdapter.run(extract, &filter, &RunEnv::scrubbed())?;
        runs += 1;
        let result = outcome
            .results
            .first()
            .ok_or_else(|| "adapter returned no result".to_string())?;
        match decide(result) {
            // `reconcile` is what turns a pass that FOLLOWS a failure into
            // `flaky-observation` instead of `confirmed`.
            Decision::Verdict(v) => return Ok((reconcile(any_failed, v), runs)),
            Decision::Retry => {
                any_failed = true;
                if runs >= MAX_RUNS {
                    return Ok((VerdictKind::ClaimFalse, runs));
                }
            }
        }
    }
}

enum Decision {
    Verdict(VerdictKind),
    /// The run FAILED. Only this outcome is retried.
    Retry,
}

/// One run's structured result → what to do about it. Pure, so the policy is
/// testable without spawning cargo.
///
/// `recipe_invalid` is checked BEFORE `outcome` deliberately: the staged
/// pipeline sets both for an `#[ignore]`d test (`outcome: Ignored`,
/// `recipe_invalid: Ignored`), and reading `outcome` first would have to invent
/// a verdict for `Ignored` that the taxonomy does not have.
fn decide(result: &StructuredResult) -> Decision {
    if let Some(cause) = result.recipe_invalid {
        return Decision::Verdict(VerdictKind::RecipeInvalid { cause });
    }
    match result.outcome {
        Some(TestOutcome::Passed) => Decision::Verdict(VerdictKind::Confirmed),
        Some(TestOutcome::Failed) => Decision::Retry,
        // Neither an outcome nor a cause: the parser failed closed. Unverifiable
        // this run, and unverifiable is `recipe-invalid`, never `claim-false`.
        _ => Decision::Verdict(VerdictKind::RecipeInvalid {
            cause: agentrec_core::attest::types::RecipeInvalidCause::Harness,
        }),
    }
}

/// A previous failing run is followed by a passing one → `flaky-observation`.
/// Expressed as a fold over the loop above rather than a second code path: the
/// loop returns `Confirmed` from [`decide`] on a pass, and this rewrites it
/// when an earlier run had failed.
fn reconcile(first_failed: bool, verdict: VerdictKind) -> VerdictKind {
    if first_failed && verdict == VerdictKind::Confirmed {
        VerdictKind::FlakyObservation
    } else {
        verdict
    }
}

fn ensure_clean_tree(root: &Path) -> Result<(), String> {
    // Spawns `git status` to refuse a dirty tree before minting a verdict.
    #[allow(clippy::disallowed_methods)]
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git in {}: {e}", root.display()))?;
    if !out.status.success() {
        return Err(format!(
            "git status failed in {}: {}",
            root.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let body = String::from_utf8_lossy(&out.stdout);
    if !body.trim().is_empty() {
        return Err(format!(
            "working tree is dirty — verify replays COMMITTED state only, so a \
             verdict minted now would be stamped with a commit that never held \
             these bytes. Commit or stash first:\n{}",
            body.trim()
        ));
    }
    Ok(())
}

fn head_commit(root: &Path) -> Result<String, String> {
    // Spawns `git rev-parse` to pin the replay commit.
    #[allow(clippy::disallowed_methods)]
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git in {}: {e}", root.display()))?;
    if !out.status.success() {
        return Err(format!(
            "cannot resolve HEAD in {}: {}",
            root.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `git archive <commit>` into a STABLE directory under the per-root cache.
///
/// Stable, not a fresh tempdir per run, on purpose: cargo's fingerprints
/// include the workspace path, so a new extract path invalidates the whole
/// shared build cache and every replay pays a cold build. The directory is
/// removed and recreated each time, so it never carries a previous commit's
/// files.
fn extract_commit(root: &Path, commit: &str) -> Result<PathBuf, String> {
    let base = attest_target_dir(root);
    let extract = base.join("extract");
    let _ = std::fs::remove_dir_all(&extract);
    std::fs::create_dir_all(&extract)
        .map_err(|e| format!("cannot create {}: {e}", extract.display()))?;

    let tar = base.join("extract.tar");
    let _ = std::fs::remove_file(&tar);
    // Spawns `git archive` to extract committed bytes; never `git worktree add`, which would write into the production repo's .git.
    #[allow(clippy::disallowed_methods)]
    let out = Command::new("git")
        .args(["archive", "--format=tar", "-o"])
        .arg(&tar)
        .arg(commit)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git archive: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git archive {commit} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    // Spawns `tar` to unpack the archive `git archive` just wrote.
    #[allow(clippy::disallowed_methods)]
    let out = Command::new("tar")
        .arg("-xf")
        .arg(&tar)
        .arg("-C")
        .arg(&extract)
        .output()
        .map_err(|e| format!("cannot run tar: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "tar extract failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let _ = std::fs::remove_file(&tar);

    // The build-cache seam. The adapter pins `CARGO_TARGET_DIR` to
    // `<crate_root>/target`; symlinking that at the cache redirects it without
    // touching Phase 3's contract. See this module's doc comment.
    //
    // KEYED BY COMMIT, and that is a correctness requirement, not tidiness.
    // `git archive` stamps every extracted file with the COMMIT's timestamp,
    // and cargo fingerprints on mtime — so two commits made within the same
    // second extract to byte-different sources carrying IDENTICAL mtimes, and
    // one shared cache serves the earlier commit's build for the later one.
    // Measured: three successive commits in this repo's fixture all landed on
    // the same second, and a shared cache made a restored test still report
    // `ignored`. Per-commit keying keeps the cache across a claim's RETRIES
    // (same commit, which is the case that matters for the 3-run policy) and
    // gives a different commit a cold build, which is the only correct answer.
    let key: String = commit.chars().take(12).collect();
    let cache = base.join("target").join(&key);
    std::fs::create_dir_all(&cache)
        .map_err(|e| format!("cannot create {}: {e}", cache.display()))?;
    let link = extract.join("target");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&cache, &link)
        .map_err(|e| format!("cannot link {} -> {}: {e}", link.display(), cache.display()))?;

    Ok(extract)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use agentrec_core::attest::types::RecipeInvalidCause;

    fn ident() -> TestIdentity {
        TestIdentity::new("t", "f")
    }

    #[test]
    fn an_ignored_test_reads_as_its_cause_not_as_its_outcome() {
        // The staged pipeline sets BOTH fields for `#[ignore]`. Reading
        // `outcome` first would have to invent a verdict for `Ignored`.
        let mut r = StructuredResult::recipe_invalid(ident(), RecipeInvalidCause::Ignored);
        r.outcome = Some(TestOutcome::Ignored);
        assert!(matches!(
            decide(&r),
            Decision::Verdict(VerdictKind::RecipeInvalid {
                cause: RecipeInvalidCause::Ignored
            })
        ));
    }

    #[test]
    fn only_a_failure_is_retried() {
        assert!(matches!(
            decide(&StructuredResult::outcome(ident(), TestOutcome::Failed)),
            Decision::Retry
        ));
        assert!(matches!(
            decide(&StructuredResult::outcome(ident(), TestOutcome::Passed)),
            Decision::Verdict(VerdictKind::Confirmed)
        ));
        // A recipe-invalid is never retried — a retry cannot fix a broken
        // build, it only costs a second build.
        assert!(matches!(
            decide(&StructuredResult::recipe_invalid(
                ident(),
                RecipeInvalidCause::Build
            )),
            Decision::Verdict(VerdictKind::RecipeInvalid {
                cause: RecipeInvalidCause::Build
            })
        ));
    }

    #[test]
    fn a_parse_failure_is_unverifiable_never_claim_false() {
        let r = StructuredResult {
            identity: ident(),
            outcome: None,
            recipe_invalid: None,
            parse_failed: true,
            raw_blob: None,
        };
        assert!(matches!(
            decide(&r),
            Decision::Verdict(VerdictKind::RecipeInvalid { .. })
        ));
    }

    #[test]
    fn a_pass_after_a_failure_is_flaky_not_confirmed() {
        assert_eq!(
            reconcile(true, VerdictKind::Confirmed),
            VerdictKind::FlakyObservation
        );
        assert_eq!(
            reconcile(false, VerdictKind::Confirmed),
            VerdictKind::Confirmed
        );
        // A recipe-invalid after a failure stays what it is.
        let ri = VerdictKind::RecipeInvalid {
            cause: RecipeInvalidCause::Build,
        };
        assert_eq!(reconcile(true, ri), ri);
    }
}
