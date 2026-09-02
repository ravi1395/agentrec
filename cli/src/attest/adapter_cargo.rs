//! The cargo [`Adapter`]: test discovery, the ratified libtest output parser,
//! the staged single-test verify pipeline, and test-body hashing for rename
//! detection.
//!
//! # The parser is ratified, not invented here
//!
//! Spec decision 2 / plan Phase 1 "Probe B" pin the channel: per-test
//! `test <name> ... ok|FAILED|ignored` lines plus the ONE summary line
//! (`test result: ok. N passed; M failed; K ignored; …`), with the per-test
//! tallies CROSS-CHECKED against the summary. A mismatch, or a missing summary,
//! fails closed — `parse_failed: true`, raw output retained, no per-test result
//! trusted. [`parse_libtest`] is a faithful port of the Python validated in
//! `docs/verify/attest-output-channel-spike.md`, and
//! [`tests::ac_p3_19_every_spike_fixture_parses_to_its_documented_state`] runs
//! it against every fixture that doc records a state for.
//!
//! **One extension over the spike script, load-bearing:** a bulk
//! `cargo test` run emits SEVERAL libtest sections (one per target, plus
//! doc-tests), each with its own summary. The spike script parsed one. Here the
//! output is SPLIT INTO SECTIONS at each summary line and the ratified parser is
//! applied per section, so a whole-target capture of a multi-target crate does
//! not fail its own cross-check by tallying every section against the last
//! summary. A single-section capture — every fixture in `docs/fixtures/attest/`
//! — takes the identical path it did in the spike.
//!
//! # State mapping, stated once
//!
//! The spike doc records five derived states; [`StructuredResult`] has three
//! fields. This is the whole mapping — the per-fixture tests assert all three
//! fields, so the doc's wording and this code cannot drift apart silently:
//!
//! | spike state                     | `outcome`       | `recipe_invalid` | `parse_failed` |
//! |---------------------------------|-----------------|------------------|----------------|
//! | `confirmed-candidate`           | `Passed`        | `None`           | `false`        |
//! | `claim-false-candidate`         | `Failed`        | `None`           | `false`        |
//! | `recipe-invalid (ignored)`      | `Ignored`       | `Ignored`        | `false`        |
//! | `recipe-invalid (missing)`      | `None`          | `Missing`        | `false`        |
//! | `recipe-invalid (harness)`      | `None`          | `Harness`        | `true`         |
//! | tally != summary                | `None`          | `None`           | `true`         |
//! | `bulk-evidence` (per test line) | the line's word | `None`           | `false`        |
//!
//! The `ignored` row sets BOTH fields on purpose: `TestOutcome::Ignored` is what
//! libtest reported, `RecipeInvalidCause::Ignored` is the interpretation "this
//! claim is unverifiable" — `types.rs` keeps them separate types for exactly
//! this reason, and dropping either loses information the doc records.
//! The `harness` row is the only combination neither `StructuredResult`
//! constructor produces, so it is built field by field.

use agentrec_core::attest::types::{
    RecipeInvalidCause, StructuredResult, TestIdentity, TestOutcome,
};
use quote::ToTokens;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

// The [`Adapter`] surface below — `RunFilter`, `RunOutcome`, `CargoAdapter`,
// its `run`, and the staged single-test pipeline `single_test_result` feeds —
// is Phase 3's declared CONTRACT (plan Phase 3 "Contract (produces)"). Phase 4's
// `attest verify` (`replaycmd.rs`) is now its production caller, so the
// `#[allow(dead_code)]` markers Phase 3 carried on it are gone: the compiler,
// not a comment, is what keeps this surface live from here on.

/// What a caller asks [`Adapter::run`] to execute.
///
/// Never a bare unscoped `--exact`: a test fn name can collide across targets
/// in one workspace (measured, real — plan Phase 1), so every variant names its
/// target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunFilter {
    /// Every test in one target.
    ///
    /// Still allowed dead: Phase 4's `attest verify` uses only `Exact`, and
    /// the bulk path goes through `bulk_results` rather than this variant.
    #[allow(dead_code)]
    Target { target: String },
    /// The staged single-test pipeline: build → `--list` precheck → scoped
    /// `--exact` run.
    Exact { target: String, fn_path: String },
}

/// The environment a replay child runs with.
///
/// `attest verify` scrubs: an inherited variable can change what a test does,
/// and a verdict is supposed to be a property of the COMMITTED bytes, not of
/// whoever happened to run it. The allowlist is a pinned decision — widening
/// it is a founder call, not an implementer's.
#[derive(Clone, Debug, Default)]
pub struct RunEnv {
    /// `env_clear()` plus [`ENV_ALLOWLIST`] and every `LLVM_`-prefixed var.
    pub scrubbed: bool,
    /// Applied AFTER the scrub, so a caller can add what it needs.
    pub extra: Vec<(String, String)>,
}

/// Kept verbatim across the scrub. `CARGO_TARGET_DIR` is NOT here: the adapter
/// sets it explicitly after the clear, on every spawn, scrubbed or not.
pub const ENV_ALLOWLIST: [&str; 7] = [
    "PATH",
    "HOME",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "TMPDIR",
    "TERM",
];

/// The `LLVM_` prefix passes as a class, not name by name: coverage tooling is
/// located through `LLVM_COV`/`LLVM_PROFDATA` in this repo (stock
/// `cargo llvm-cov` fails on a Homebrew rustc without them), and enumerating
/// them here would rot the day another is added.
const ENV_ALLOW_PREFIX: &str = "LLVM_";

/// The pure half of the scrub: which of `vars` survive. Split from the spawn so
/// the allowlist is testable without running cargo.
pub fn scrubbed_pairs<I>(vars: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String)>,
{
    vars.into_iter()
        .filter(|(k, _)| ENV_ALLOWLIST.contains(&k.as_str()) || k.starts_with(ENV_ALLOW_PREFIX))
        .collect()
}

impl RunEnv {
    /// Inherit this process's environment — every caller but `attest verify`.
    pub fn inherited() -> Self {
        RunEnv::default()
    }

    /// `attest verify`'s replay environment.
    pub fn scrubbed() -> Self {
        RunEnv {
            scrubbed: true,
            extra: Vec::new(),
        }
    }

    /// Must run BEFORE any other `.env()` on `cmd`: `env_clear` drops the
    /// pairs set before it and keeps the ones set after.
    fn apply(&self, cmd: &mut Command) {
        if self.scrubbed {
            cmd.env_clear();
            for (k, v) in scrubbed_pairs(std::env::vars()) {
                cmd.env(k, v);
            }
        }
        for (k, v) in &self.extra {
            cmd.env(k, v);
        }
    }
}

/// Absolute, symlink-resolved crate root.
///
/// Load-bearing, not tidiness: [`build_targets`] pins `CARGO_TARGET_DIR` to
/// `crate_root.join("target")` while ALSO setting `current_dir(crate_root)`, so
/// a RELATIVE `--crate agentrec-core` yielded the relative value
/// `agentrec-core/target`, which cargo then resolved against the CHILD's cwd —
/// producing `agentrec-core/agentrec-core/target`. Measured live on a clone of
/// this repo: 284 MB written to the wrong place, and the stray directory then
/// made the tree dirty. An absolute path cannot compound.
pub fn canonical_crate_root(crate_root: &Path) -> Result<PathBuf, String> {
    crate_root
        .canonicalize()
        .map_err(|e| format!("cannot resolve crate root {}: {e}", crate_root.display()))
}

/// The result of one [`Adapter::run`], plus what it took to produce it.
#[derive(Clone, Debug)]
pub struct RunOutcome {
    pub results: Vec<StructuredResult>,
    /// Everything the child wrote, stdout and stderr, for the CAS blob.
    ///
    /// Allowed dead: `attest verify` reads only `results`; the CAS blob is
    /// Phase 5's consumer.
    #[allow(dead_code)]
    pub raw: String,
    /// `None` when the child was killed by a signal.
    #[allow(dead_code)]
    pub exit_code: Option<i32>,
}

/// A test-runner backend. One implementation today (cargo); the trait exists
/// because Phase 4's `attest verify` consumes it and the plan's contract names
/// it.
pub trait Adapter {
    /// Allowed dead through the trait: `derivecmd` calls the free function
    /// `discover_with_hashes` instead, so nothing dispatches this dynamically.
    #[allow(dead_code)]
    fn discover(&self, crate_root: &Path) -> Result<Vec<TestIdentity>, String>;
    fn run(
        &self,
        crate_root: &Path,
        filter: &RunFilter,
        env: &RunEnv,
    ) -> Result<RunOutcome, String>;
}

pub struct CargoAdapter;

// ---------------------------------------------------------------------------
// Target enumeration
// ---------------------------------------------------------------------------

/// One built test executable, as `cargo test --no-run --message-format=json`
/// reports it.
#[derive(Clone, Debug)]
pub struct TargetInfo {
    /// Package name, kept because the run-side invocation is `-p <pkg>
    /// --test <t>` / `--lib`: [`TestIdentity::target`] is the bare target name
    /// and loses the package, so package scope is RECOMPUTED here at run time
    /// rather than encoded into the identity (which is Phase 2's type and not
    /// this phase's to change).
    pub package: String,
    pub name: String,
    /// `lib` / `test` / `bin` — the target kind, which decides the scoping flag.
    pub kind: String,
    pub src_path: PathBuf,
    pub executable: PathBuf,
}

impl TargetInfo {
    /// The cargo flags that scope an invocation to exactly this target.
    fn scope_args(&self) -> Vec<String> {
        let mut args = vec!["-p".to_string(), self.package.clone()];
        match self.kind.as_str() {
            "lib" => args.push("--lib".to_string()),
            "bin" => {
                args.push("--bin".to_string());
                args.push(self.name.clone());
            }
            _ => {
                args.push("--test".to_string());
                args.push(self.name.clone());
            }
        }
        args
    }
}

fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

/// Build every test target and return the artifacts. `cargo test --no-run
/// --message-format=json` DOES carry per-artifact data on stable (the missing
/// piece is per-TEST results, which is why the output parser exists).
///
/// `CARGO_TARGET_DIR` is pinned to the crate's own `target/` so a nested build
/// can never land in a parent workspace's target dir — this repo has a recorded
/// scar from exactly that. `crate_root` reaches here ALREADY canonicalized by
/// the adapter's entry points; see [`canonical_crate_root`] for why a relative
/// path here nested the target dir.
fn build_targets(crate_root: &Path, env: &RunEnv) -> Result<Vec<TargetInfo>, String> {
    // Spawns `cargo` (from $CARGO or PATH) to build the crate's test targets.
    // The adapter is the sanctioned test-runner boundary; the daemon never
    // calls it.
    #[allow(clippy::disallowed_methods)]
    let mut cmd = Command::new(cargo_bin());
    // `apply` first: it may `env_clear`, which drops earlier `.env()` calls.
    env.apply(&mut cmd);
    let out = cmd
        .args(["test", "--no-run", "--message-format=json"])
        .current_dir(crate_root)
        .env("CARGO_TARGET_DIR", crate_root.join("target"))
        .output()
        .map_err(|e| format!("cannot run cargo in {}: {e}", crate_root.display()))?;
    if !out.status.success() {
        return Err(format!(
            "cargo test --no-run failed in {}:\n{}",
            crate_root.display(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(parse_artifact_stream(&String::from_utf8_lossy(&out.stdout)))
}

/// The pure half of [`build_targets`]: cargo's `--message-format=json`
/// compiler-artifact stream -> the built test executables.
///
/// Split out because Phase 4's coverage capture runs its OWN cargo invocation
/// (`cargo llvm-cov test --no-run`, which emits the identical stream) and must
/// not re-derive this parsing.
pub fn parse_artifact_stream(stdout: &str) -> Vec<TargetInfo> {
    let mut targets = Vec::new();
    for line in stdout.lines() {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if msg.get("reason").and_then(|r| r.as_str()) != Some("compiler-artifact") {
            continue;
        }
        let Some(exe) = msg.get("executable").and_then(|e| e.as_str()) else {
            continue;
        };
        let target = &msg["target"];
        let (Some(name), Some(src)) = (
            target.get("name").and_then(|n| n.as_str()),
            target.get("src_path").and_then(|p| p.as_str()),
        ) else {
            continue;
        };
        let kind = target
            .get("kind")
            .and_then(|k| k.as_array())
            .and_then(|k| k.first())
            .and_then(|k| k.as_str())
            .unwrap_or("test")
            .to_string();
        // `package_id` is `path+file:///...#name@version` or `registry+...#name@ver`;
        // the fragment's name half is what `-p` wants.
        let package = msg
            .get("package_id")
            .and_then(|p| p.as_str())
            .map(package_name_from_id)
            .unwrap_or_default();
        targets.push(TargetInfo {
            package,
            name: name.to_string(),
            kind,
            src_path: PathBuf::from(src),
            executable: PathBuf::from(exe),
        });
    }
    targets.sort_by(|a, b| (&a.name, &a.kind).cmp(&(&b.name, &b.kind)));
    targets
}

/// `path+file:///a/b#0.0.0` → `b`; `path+file:///a/b#name@1.2.3` → `name`.
fn package_name_from_id(id: &str) -> String {
    let (before, frag) = match id.split_once('#') {
        Some((b, f)) => (b, f),
        None => (id, ""),
    };
    if let Some((name, _ver)) = frag.split_once('@') {
        return name.to_string();
    }
    // A bare-version fragment (`#0.0.0`): the name is the last path segment.
    if !frag.is_empty() && frag.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return before
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
    }
    if frag.is_empty() {
        return before
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
    }
    frag.to_string()
}

/// The `<name>: test` lines of a libtest `--list` capture.
///
/// Split out from [`list_tests`] so a committed real capture
/// (`docs/fixtures/attest/list.txt`) can drive the exact line parser discovery
/// depends on, without spawning anything. The `: test` suffix is required —
/// libtest also prints `<name>: benchmark` lines and a trailing
/// `N tests, M benchmarks` summary, and neither is a test identity.
pub fn parse_list_output(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| l.strip_suffix(": test"))
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect()
}

/// `<exe> --list` → the `<name>: test` lines. No `--format=json` (unstable).
fn list_tests(exe: &Path) -> Result<Vec<String>, String> {
    // Spawns the BUILT libtest binary with `--list`. `exe` comes from cargo's own artifact stream, not from repo content.
    #[allow(clippy::disallowed_methods)]
    let out = Command::new(exe)
        .arg("--list")
        .output()
        .map_err(|e| format!("cannot run {} --list: {e}", exe.display()))?;
    Ok(parse_list_output(&String::from_utf8_lossy(&out.stdout)))
}

impl Adapter for CargoAdapter {
    fn discover(&self, crate_root: &Path) -> Result<Vec<TestIdentity>, String> {
        let crate_root = &canonical_crate_root(crate_root)?;
        let mut out = Vec::new();
        for target in build_targets(crate_root, &RunEnv::inherited())? {
            for name in list_tests(&target.executable)? {
                out.push(TestIdentity::new(target.name.clone(), name));
            }
        }
        // Sorted so derive order — and therefore the ids it mints — is
        // reproducible run to run.
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn run(
        &self,
        crate_root: &Path,
        filter: &RunFilter,
        env: &RunEnv,
    ) -> Result<RunOutcome, String> {
        let crate_root = &canonical_crate_root(crate_root)?;
        let targets = match build_targets(crate_root, env) {
            Ok(t) => t,
            Err(e) => {
                // Stage 1: the target did not build. Nothing runs.
                let identity = match filter {
                    RunFilter::Exact { target, fn_path } => TestIdentity::new(target, fn_path),
                    RunFilter::Target { target } => TestIdentity::new(target, ""),
                };
                return Ok(RunOutcome {
                    results: vec![StructuredResult::recipe_invalid(
                        identity,
                        RecipeInvalidCause::Build,
                    )],
                    raw: e,
                    exit_code: Some(101),
                });
            }
        };

        let want = match filter {
            RunFilter::Target { target } | RunFilter::Exact { target, .. } => target,
        };
        let target = targets
            .iter()
            .find(|t| &t.name == want)
            .ok_or_else(|| format!("no cargo target named {want} in {}", crate_root.display()))?;

        let mut args = vec!["test".to_string()];
        args.extend(target.scope_args());
        if let RunFilter::Exact { fn_path, .. } = filter {
            // Stage 2: `--list` membership precheck. A name absent from the
            // built binary is `missing`, and libtest would report that as a
            // 0/0/0 run with exit 0 — indistinguishable from success by exit
            // code alone.
            let listed = list_tests(&target.executable)?;
            if !listed.iter().any(|n| n == fn_path) {
                return Ok(RunOutcome {
                    results: vec![StructuredResult::recipe_invalid(
                        TestIdentity::new(&target.name, fn_path),
                        RecipeInvalidCause::Missing,
                    )],
                    raw: String::new(),
                    exit_code: Some(0),
                });
            }
            args.push("--".to_string());
            args.push("--exact".to_string());
            args.push(fn_path.clone());
        }

        // Spawns `cargo test` scoped to one target and one `--exact` name.
        // This is the replay itself — the whole point of the adapter.
        #[allow(clippy::disallowed_methods)]
        let mut cmd = Command::new(cargo_bin());
        // `apply` first: it may `env_clear`, which drops earlier `.env()` calls.
        env.apply(&mut cmd);
        let out = cmd
            .args(&args)
            .current_dir(crate_root)
            .env("CARGO_TARGET_DIR", crate_root.join("target"))
            .output()
            .map_err(|e| format!("cannot run cargo test: {e}"))?;

        // Stage 3: parse. stdout ONLY — libtest writes results there while
        // cargo writes progress to stderr, and merging them first lets a
        // `Compiling` line interleave into a per-test match. The CAS blob keeps
        // both.
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        let results = match filter {
            RunFilter::Exact { fn_path, .. } => {
                vec![single_test_result(
                    &TestIdentity::new(&target.name, fn_path),
                    &stdout,
                )]
            }
            RunFilter::Target { .. } => bulk_results(&stdout, std::slice::from_ref(&target.name)),
        };
        Ok(RunOutcome {
            results,
            raw: format!("{stdout}{stderr}"),
            exit_code: out.status.code(),
        })
    }
}

// ---------------------------------------------------------------------------
// The ratified libtest parser
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub passed: usize,
    pub failed: usize,
    pub ignored: usize,
}

/// One libtest run's worth of output: everything up to and including one
/// summary line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub per_test: Vec<(String, TestOutcome)>,
    pub summary: Option<Summary>,
    /// The ratified fail-closed flag: no summary, or per-test tallies that
    /// disagree with it.
    pub parse_failed: bool,
    pub detail: Option<String>,
}

fn parse_per_test(line: &str) -> Option<(String, TestOutcome)> {
    let rest = line.strip_prefix("test ")?;
    let (name, status) = rest.rsplit_once(" ... ")?;
    let outcome = match status.trim_end() {
        "ok" => TestOutcome::Passed,
        "FAILED" => TestOutcome::Failed,
        "ignored" => TestOutcome::Ignored,
        _ => return None,
    };
    if name.is_empty() || name.contains(' ') {
        return None;
    }
    Some((name.to_string(), outcome))
}

/// `test result: ok. 36 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.31s`
///
/// All six fields are required, in order — the truncated `test result: ok. 36
/// pass` in the corrupted fixture must NOT match, which is what puts that
/// capture on the fail-closed path.
fn parse_summary(line: &str) -> Option<Summary> {
    let rest = line.strip_prefix("test result: ")?;
    let rest = rest
        .strip_prefix("ok. ")
        .or_else(|| rest.strip_prefix("FAILED. "))?;
    let fields: Vec<&str> = rest.split("; ").collect();
    if fields.len() != 6 {
        return None;
    }
    let counted = |i: usize, label: &str| -> Option<usize> {
        let (n, l) = fields[i].split_once(' ')?;
        if l != label {
            return None;
        }
        n.parse::<usize>().ok()
    };
    let passed = counted(0, "passed")?;
    let failed = counted(1, "failed")?;
    let ignored = counted(2, "ignored")?;
    counted(3, "measured")?;
    counted(4, "filtered out")?;
    if !fields[5].starts_with("finished in ") {
        return None;
    }
    Some(Summary {
        passed,
        failed,
        ignored,
    })
}

/// Split raw stdout into libtest sections and apply the ratified cross-check to
/// each. Never errors: unusable output is a flagged section, never a lost one.
pub fn parse_libtest(raw: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut per_test: Vec<(String, TestOutcome)> = Vec::new();
    let mut saw_any = false;
    for line in raw.lines() {
        if let Some(entry) = parse_per_test(line) {
            saw_any = true;
            per_test.push(entry);
            continue;
        }
        if let Some(summary) = parse_summary(line) {
            saw_any = true;
            sections.push(finish_section(std::mem::take(&mut per_test), Some(summary)));
        }
    }
    if !per_test.is_empty() || !saw_any {
        sections.push(finish_section(per_test, None));
    }
    sections
}

fn finish_section(per_test: Vec<(String, TestOutcome)>, summary: Option<Summary>) -> Section {
    let Some(summary) = summary else {
        return Section {
            per_test,
            summary: None,
            parse_failed: true,
            detail: Some(
                "no summary line found (harness crash / process::exit / SIGABRT)".to_string(),
            ),
        };
    };
    let mut tally = Summary {
        passed: 0,
        failed: 0,
        ignored: 0,
    };
    for (_, outcome) in &per_test {
        match outcome {
            TestOutcome::Passed => tally.passed += 1,
            TestOutcome::Failed => tally.failed += 1,
            TestOutcome::Ignored => tally.ignored += 1,
        }
    }
    if tally != summary {
        return Section {
            per_test,
            summary: Some(summary),
            parse_failed: true,
            detail: Some(format!("per-test tally {tally:?} != summary {summary:?}")),
        };
    }
    Section {
        per_test,
        summary: Some(summary),
        parse_failed: false,
        detail: None,
    }
}

/// Stage 3 of the staged single-test pipeline: interpret a scoped `--exact`
/// capture for the one test it named. See this module's state-mapping table.
pub fn single_test_result(identity: &TestIdentity, stdout: &str) -> StructuredResult {
    let sections = parse_libtest(stdout);
    // A scoped `--exact` run produces exactly one libtest section. More than one
    // means the invocation was not scoped after all — refuse to interpret it.
    let Some(section) = sections.first() else {
        return harness(identity);
    };
    if sections.len() > 1 {
        return StructuredResult {
            identity: identity.clone(),
            outcome: None,
            recipe_invalid: None,
            parse_failed: true,
            raw_blob: None,
        };
    }
    if section.parse_failed {
        return match section.summary {
            None => harness(identity),
            Some(_) => StructuredResult {
                identity: identity.clone(),
                outcome: None,
                recipe_invalid: None,
                parse_failed: true,
                raw_blob: None,
            },
        };
    }
    let summary = section.summary.expect("checked parse_failed above");
    let total = summary.passed + summary.failed + summary.ignored;
    if total == 0 {
        return StructuredResult::recipe_invalid(identity.clone(), RecipeInvalidCause::Missing);
    }
    if total == 1 && summary.ignored == 1 {
        // Both fields: what libtest reported AND the interpretation.
        let mut r = StructuredResult::recipe_invalid(identity.clone(), RecipeInvalidCause::Ignored);
        r.outcome = Some(TestOutcome::Ignored);
        return r;
    }
    if total == 1 && summary.passed == 1 {
        return StructuredResult::outcome(identity.clone(), TestOutcome::Passed);
    }
    if total == 1 && summary.failed == 1 {
        return StructuredResult::outcome(identity.clone(), TestOutcome::Failed);
    }
    // More than one test ran under an `--exact` filter: not a single-test
    // verdict. Fail closed rather than pick one.
    StructuredResult {
        identity: identity.clone(),
        outcome: None,
        recipe_invalid: None,
        parse_failed: true,
        raw_blob: None,
    }
}

#[allow(dead_code)]
fn harness(identity: &TestIdentity) -> StructuredResult {
    StructuredResult {
        identity: identity.clone(),
        outcome: None,
        recipe_invalid: Some(RecipeInvalidCause::Harness),
        parse_failed: true,
        raw_blob: None,
    }
}

/// Bulk interpretation: one [`StructuredResult`] per per-test line, with the
/// section's target attached.
///
/// `targets` pairs positionally with the sections — see
/// [`section_targets`] for where the names come from and what happens when the
/// pairing does not hold. A section whose target is unknown yields identities
/// with an EMPTY target, which no claim can match, so such results are reported
/// as undeclared rather than mis-attributed.
pub fn bulk_results(stdout: &str, targets: &[String]) -> Vec<StructuredResult> {
    let sections = parse_libtest(stdout);
    let mut out = Vec::new();
    for (i, section) in sections.iter().enumerate() {
        let target = targets.get(i).cloned().unwrap_or_default();
        for (name, outcome) in &section.per_test {
            let identity = TestIdentity::new(target.clone(), name.clone());
            if section.parse_failed {
                // Fail closed: the identity is kept so the capture is
                // attributable, the OUTCOME is dropped because the section's
                // own cross-check says it cannot be trusted.
                out.push(StructuredResult {
                    identity,
                    outcome: None,
                    recipe_invalid: section
                        .summary
                        .is_none()
                        .then_some(RecipeInvalidCause::Harness),
                    parse_failed: true,
                    raw_blob: None,
                });
            } else {
                out.push(StructuredResult::outcome(identity, *outcome));
            }
        }
    }
    out
}

/// Strip ANSI SGR sequences (`ESC [` digits/`;` `m`) from one line.
///
/// Cargo colorizes its OWN status lines whenever color is forced. The trigger
/// is an explicit `CARGO_TERM_COLOR=always`: `.github/workflows/ci.yml` sets it
/// in a workflow-level `env:` block, and a user can export it in their shell.
/// It is NOT reached by attaching a terminal — [`crate::attest::capture::
/// run_wrapped`] spawns the child with `Stdio::piped()` on both streams, so
/// cargo's own TTY detection never fires under `attest run` (measured: a real
/// `attest run` driven inside a pty with the variable unset captured zero ESC
/// bytes). Forced color puts the escape BEFORE the leading whitespace and the
/// reset BETWEEN the word and its trailing space, so on a raw colorized line
/// `trim_start()` strips nothing and even `contains("Running ")` is false.
/// [`section_targets`] therefore paired no marker at all, every result landed
/// in an unattributable section, and the capture wrote zero evidence behind a
/// single stderr warning.
///
/// SGR is the only escape class cargo emits here — measured on a passing and on
/// a failing run, where the colorized lines were exactly `Compiling`,
/// `Finished`, `Running`, `Doc-tests` and `error:`, and libtest's own
/// `running N tests` / `test … ok` / `test result:` lines carried none. That
/// is why this scanner is narrow rather than a general-purpose ANSI stripper,
/// and why the libtest line parsers need no equivalent.
///
/// Anything that is not a complete SGR sequence — an unterminated `ESC [` from
/// a truncated capture, a bare ESC, or some other CSI — ends the scan and
/// returns what was accumulated. The remainder is not text this function can
/// read, and refusing to pair beats pairing on a half-read marker.
fn strip_sgr(line: &str) -> Cow<'_, str> {
    if !line.contains('\u{1b}') {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(esc) = rest.find('\u{1b}') {
        out.push_str(&rest[..esc]);
        let Some(body) = rest[esc..].strip_prefix("\u{1b}[") else {
            return Cow::Owned(out);
        };
        let Some(end) = body.find(|c: char| !c.is_ascii_digit() && c != ';') else {
            return Cow::Owned(out);
        };
        if body.as_bytes()[end] != b'm' {
            return Cow::Owned(out);
        }
        rest = &body[end + 1..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Target names for the sections of a bulk `cargo test`, read from cargo's own
/// `Running` / `Doc-tests` markers in execution order.
///
/// **BOTH streams are scanned, stdout first.** `cargo test 2>&1 | tail -30` is
/// an ordinary agent shell shape, and it puts the markers into STDOUT; scanning
/// only stderr found none, left every identity's target empty, and turned a
/// perfectly good capture into "5 result(s) for undeclared tests skipped" — a
/// wrong diagnosis and zero evidence written. The markers live in exactly one
/// stream per run, so concatenating the two scans cannot double-count.
///
/// A count mismatch still returns an empty vec, which is loudly counted as
/// UNATTRIBUTABLE — a distinct diagnosis from "undeclared"
/// (see [`crate::attest::capture::CaptureReport`]). A wrong target silently
/// cross-attributes evidence; an empty one cannot.
///
/// The marker shapes: `Running unittests src/lib.rs (target/debug/deps/
/// <name>-<hash>)`, `Running tests/<file>.rs (…)`, `Doc-tests <crate>`. cargo
/// emits one before each test binary, sequentially, so the Nth marker names the
/// Nth section of the results stream.
pub fn section_targets(stdout: &str, stderr: &str, sections: usize) -> Vec<String> {
    let mut names = Vec::new();
    for line in stdout.lines().chain(stderr.lines()) {
        let line = strip_sgr(line);
        let line = line.trim_start();
        if line.starts_with("Doc-tests ") {
            names.push("doc-tests".to_string());
            continue;
        }
        let Some(rest) = line.strip_prefix("Running ") else {
            continue;
        };
        // `… (target/debug/deps/<name>-<hash>)`
        let Some(open) = rest.rfind('(') else {
            continue;
        };
        let path = rest[open + 1..].trim_end_matches(')');
        let Some(file) = path.rsplit('/').next() else {
            continue;
        };
        let name = match file.rsplit_once('-') {
            Some((n, _hash)) => n,
            None => file,
        };
        names.push(name.to_string());
    }
    if names.len() != sections {
        return Vec::new();
    }
    names
}

// ---------------------------------------------------------------------------
// Body hashing (rename detection input)
// ---------------------------------------------------------------------------

/// sha256 of a test fn's BODY BLOCK as a token stream, so reformatting, comment
/// edits and whitespace do not read as a body change. Only a real edit to the
/// body's tokens moves the hash.
///
/// The body ALONE, deliberately: hashing the whole `ItemFn` would fold the fn's
/// own NAME into the hash, and then a rename — the exact event this hash exists
/// to detect — would always change it. The cost is that two tests with identical
/// bodies hash alike; [`crate::attest::derivecmd`] handles that by refusing to
/// adopt an ambiguous donor rather than guessing.
///
/// **What the module walk does NOT resolve — measured limitation, not a
/// promise:** `#[path = "…"]` module attributes, `include!`-ed sources, and
/// tests generated by a macro. In each case the fn is not found and the
/// identity simply gets no hash, which costs rename detection for that test
/// (a rename then mints a fresh claim) and costs nothing else. `#[cfg]`-gated
/// modules are walked by NAME only — a `#[cfg(test)] mod tests` and a
/// `#[cfg(not(test))] mod tests` in one file would collide, and the first wins.
pub fn body_hashes(targets: &[TargetInfo], identities: &[TestIdentity]) -> BodyHashes {
    let mut by_target: BTreeMap<&str, &TargetInfo> = BTreeMap::new();
    for t in targets {
        by_target.insert(t.name.as_str(), t);
    }
    let mut out = BTreeMap::new();
    for identity in identities {
        let Some(target) = by_target.get(identity.target.as_str()) else {
            continue;
        };
        if let Some(hash) = hash_test_fn(&target.src_path, &identity.fn_path) {
            out.insert(identity.clone(), hash);
        }
    }
    out
}

pub type BodyHashes = BTreeMap<TestIdentity, [u8; 32]>;

/// Resolve `mod::path::fn_name` from a target's root source file and hash it.
fn hash_test_fn(root_src: &Path, fn_path: &str) -> Option<[u8; 32]> {
    let mut segments: Vec<&str> = fn_path.split("::").collect();
    let fn_name = segments.pop()?;
    let file = agentrec_core::fsguard::read_regular_to_string(root_src).ok()?;
    let ast = syn::parse_file(&file).ok()?;
    let item = find_fn(&ast.items, root_src, &segments, fn_name)?;
    Some(sha256(item.block.to_token_stream().to_string().as_bytes()))
}

/// Descend `segments` through inline `mod x { … }` blocks AND file modules
/// (`mod x;` → `x.rs` or `x/mod.rs`, resolved relative to the CURRENT file's
/// directory the way rustc does), then find `fn <name>` in the reached module.
fn find_fn(
    items: &[syn::Item],
    current_file: &Path,
    segments: &[&str],
    fn_name: &str,
) -> Option<syn::ItemFn> {
    let Some((head, tail)) = segments.split_first() else {
        return items.iter().find_map(|item| match item {
            syn::Item::Fn(f) if f.sig.ident == fn_name => Some(f.clone()),
            _ => None,
        });
    };
    for item in items {
        let syn::Item::Mod(m) = item else { continue };
        if m.ident != *head {
            continue;
        }
        if let Some((_brace, inner)) = &m.content {
            return find_fn(inner, current_file, tail, fn_name);
        }
        // File module: `mod x;`
        let dir = module_dir(current_file);
        for candidate in [
            dir.join(format!("{head}.rs")),
            dir.join(head).join("mod.rs"),
        ] {
            let Ok(text) = agentrec_core::fsguard::read_regular_to_string(&candidate) else {
                continue;
            };
            let Ok(ast) = syn::parse_file(&text) else {
                continue;
            };
            return find_fn(&ast.items, &candidate, tail, fn_name);
        }
        return None;
    }
    None
}

/// The directory `mod x;` inside `file` resolves against: the file's own
/// directory for `lib.rs` / `main.rs` / `mod.rs`, otherwise a subdirectory
/// named after the file.
fn module_dir(file: &Path) -> PathBuf {
    let parent = file.parent().unwrap_or(Path::new(".")).to_path_buf();
    match file.file_name().and_then(|n| n.to_str()) {
        Some("lib.rs") | Some("main.rs") | Some("mod.rs") => parent,
        Some(name) => parent.join(name.trim_end_matches(".rs")),
        None => parent,
    }
}

/// sha256 as raw bytes, via core's `hash_bytes` (`"sha256:<hex>"`) so this
/// crate needs no `sha2` dependency of its own.
fn sha256(bytes: &[u8]) -> [u8; 32] {
    let hex = agentrec_core::store::hash_bytes(bytes);
    let hex = hex.strip_prefix("sha256:").unwrap_or(&hex);
    let raw = hex.as_bytes();
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        let hi = (raw[i * 2] as char).to_digit(16).unwrap_or(0);
        let lo = (raw[i * 2 + 1] as char).to_digit(16).unwrap_or(0);
        *slot = (hi * 16 + lo) as u8;
    }
    out
}

/// Discover, and hash every discovered test's body, in one build.
pub fn discover_with_hashes(crate_root: &Path) -> Result<(Vec<TestIdentity>, BodyHashes), String> {
    let crate_root = &canonical_crate_root(crate_root)?;
    let targets = build_targets(crate_root, &RunEnv::inherited())?;
    let mut identities = Vec::new();
    for target in &targets {
        for name in list_tests(&target.executable)? {
            identities.push(TestIdentity::new(target.name.clone(), name));
        }
    }
    identities.sort();
    identities.dedup();
    let hashes = body_hashes(&targets, &identities);
    Ok((identities, hashes))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
//  ^ Test code reads its own committed fixtures and tempdir copies.
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/fixtures/attest")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    fn ident() -> TestIdentity {
        TestIdentity::new("import_claude", "some_test")
    }

    /// AC-ATTEST-P3-19 — every state `docs/verify/attest-output-channel-spike.md`
    /// records, asserted as the full `(outcome, recipe_invalid, parse_failed)`
    /// triple this module's mapping table defines.
    #[test]
    fn ac_p3_19_every_spike_fixture_parses_to_its_documented_state() {
        // `whole-target.txt`: bulk-evidence, 36 ok + 1 ignored, tallies match.
        let sections = parse_libtest(&fixture("whole-target.txt"));
        assert_eq!(sections.len(), 1, "{sections:#?}");
        assert!(!sections[0].parse_failed);
        assert_eq!(sections[0].per_test.len(), 37);
        assert_eq!(
            sections[0].summary,
            Some(Summary {
                passed: 36,
                failed: 0,
                ignored: 1
            })
        );
        let bulk = bulk_results(&fixture("whole-target.txt"), &["import_claude".into()]);
        assert_eq!(bulk.len(), 37);
        assert_eq!(bulk[0].outcome, Some(TestOutcome::Passed));
        assert!(bulk
            .iter()
            .all(|r| !r.parse_failed && r.recipe_invalid.is_none()));
        assert_eq!(
            bulk.iter()
                .filter(|r| r.outcome == Some(TestOutcome::Ignored))
                .count(),
            1
        );

        // `single-exact.txt`: confirmed-candidate.
        let r = single_test_result(&ident(), &fixture("single-exact.txt"));
        assert_eq!(r.outcome, Some(TestOutcome::Passed));
        assert_eq!(r.recipe_invalid, None);
        assert!(!r.parse_failed);

        // `single-exact-ignored.txt`: recipe-invalid (ignored), both fields.
        let r = single_test_result(&ident(), &fixture("single-exact-ignored.txt"));
        assert_eq!(r.outcome, Some(TestOutcome::Ignored));
        assert_eq!(r.recipe_invalid, Some(RecipeInvalidCause::Ignored));
        assert!(!r.parse_failed);

        // `single-exact-missing.txt`: recipe-invalid (missing) — exit 0, 0/0/0.
        let r = single_test_result(&ident(), &fixture("single-exact-missing.txt"));
        assert_eq!(r.outcome, None);
        assert_eq!(r.recipe_invalid, Some(RecipeInvalidCause::Missing));
        assert!(!r.parse_failed);

        // `single-exact-failed.handcrafted.txt`: claim-false-candidate.
        let r = single_test_result(&ident(), &fixture("single-exact-failed.handcrafted.txt"));
        assert_eq!(r.outcome, Some(TestOutcome::Failed));
        assert_eq!(r.recipe_invalid, None);
        assert!(!r.parse_failed);

        // `corrupted.txt`: fails closed on the missing-summary branch.
        let raw = fixture("corrupted.txt");
        let sections = parse_libtest(&raw);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].parse_failed);
        assert_eq!(sections[0].summary, None);
        assert_eq!(sections[0].per_test.len(), 36, "the mangled line drops out");
        let r = single_test_result(&ident(), &raw);
        assert_eq!(r.outcome, None);
        assert_eq!(r.recipe_invalid, Some(RecipeInvalidCause::Harness));
        assert!(r.parse_failed);
    }

    /// A summary that PARSES but disagrees with the per-test lines is the
    /// second fail-closed gate — the spike's corrupted fixture only exercises
    /// the first.
    #[test]
    fn a_tally_that_disagrees_with_the_summary_fails_closed() {
        let raw = "\nrunning 2 tests\ntest a ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
        let sections = parse_libtest(raw);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].parse_failed);
        assert!(sections[0].summary.is_some());
        let r = single_test_result(&ident(), raw);
        assert!(r.parse_failed);
        assert_eq!(r.outcome, None);
        // Not `harness`: a summary WAS found, it just disagreed.
        assert_eq!(r.recipe_invalid, None);

        // The same branch through `bulk_results`, which is the function BOTH
        // capture paths call — a disagreeing summary is the likelier real-world
        // corruption, and the truncated-summary fixture does not exercise it.
        let bulk = bulk_results(raw, &["tgt".into()]);
        assert_eq!(bulk.len(), 1);
        assert!(bulk[0].parse_failed);
        assert_eq!(bulk[0].outcome, None, "no per-test outcome may be trusted");
        assert_eq!(bulk[0].recipe_invalid, None, "a summary was found");
        assert_eq!(bulk[0].identity, TestIdentity::new("tgt", "a"));
    }

    /// A multi-target `cargo test` is several sections; each is cross-checked
    /// on its own, which the spike's one-section script could not do.
    #[test]
    fn a_multi_target_run_is_cross_checked_per_section() {
        let raw = "\nrunning 1 test\ntest tests::a ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n\nrunning 1 test\ntest b ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
        let sections = parse_libtest(raw);
        assert_eq!(sections.len(), 2);
        assert!(sections.iter().all(|s| !s.parse_failed));
        let results = bulk_results(raw, &["mylib".into(), "integration".into()]);
        assert_eq!(results[0].identity, TestIdentity::new("mylib", "tests::a"));
        assert_eq!(results[1].identity, TestIdentity::new("integration", "b"));
    }

    /// AC-ATTEST-P3-31 (unit half) — markers arriving on STDOUT (the
    /// `cargo test 2>&1` shape) are found, and so are markers on stderr. Only
    /// one stream carries them per run, so scanning both cannot double-count.
    #[test]
    fn ac_p3_31_markers_are_found_on_either_stream() {
        let markers = "    Finished `test` profile\n     Running unittests src/lib.rs (target/debug/deps/attest_sample_crate-392ca9d12eda1cf9)\n     Running tests/integration.rs (target/debug/deps/integration-12fc9e26ddfc6535)\n   Doc-tests attest_sample_crate\n";
        let expected = vec![
            "attest_sample_crate".to_string(),
            "integration".to_string(),
            "doc-tests".to_string(),
        ];
        // Merged into stdout (`2>&1`) — the shape that previously found none.
        assert_eq!(section_targets(markers, "", 3), expected);
        // Separate streams — the plain shape.
        assert_eq!(section_targets("", markers, 3), expected);
        // Still refuses to mis-pair on a count mismatch, from either stream.
        assert!(section_targets(markers, "", 2).is_empty());
    }

    #[test]
    fn section_targets_pairs_markers_with_sections_and_refuses_a_mismatch() {
        let stderr = "    Finished `test` profile\n     Running unittests src/lib.rs (target/debug/deps/attest_sample_crate-392ca9d12eda1cf9)\n     Running tests/integration.rs (target/debug/deps/integration-12fc9e26ddfc6535)\n   Doc-tests attest_sample_crate\n";
        assert_eq!(
            section_targets("", stderr, 3),
            vec!["attest_sample_crate", "integration", "doc-tests"]
        );
        // Wrong section count: refuse rather than mis-pair.
        assert!(section_targets("", stderr, 2).is_empty());
    }

    /// `CARGO_TERM_COLOR=always` — which `.github/workflows/ci.yml` sets in a
    /// workflow-level `env:` block, and which a user may export in their own
    /// shell — makes cargo colorize its OWN status lines. An explicit variable
    /// is the only trigger; `attest run` pipes both of the child's streams, so
    /// a terminal alone does not produce this. The SGR sequence sits BEFORE the leading
    /// whitespace and the reset lands between the word and its trailing space —
    /// `\x1b[1m\x1b[92m     Running\x1b[0m unittests …` — so `trim_start()`
    /// strips nothing, `strip_prefix("Running ")` never matches, every result
    /// lands in an unattributable section, and the capture writes zero evidence
    /// behind one stderr warning. The fixture below is a byte-for-byte copy of
    /// real colorized cargo output, reset position included: a fix that merely
    /// searched for the token `Running` rather than stripping would pass a
    /// fixture without that reset, and still be broken on the real thing.
    ///
    /// Only cargo's own status lines are colorized — measured on both a passing
    /// and a failing run — so the libtest line parsers need no equivalent.
    #[test]
    fn section_targets_pairs_markers_that_cargo_colorized() {
        let stderr = "\u{1b}[1m\u{1b}[92m    Finished\u{1b}[0m `test` profile\n\u{1b}[1m\u{1b}[92m     Running\u{1b}[0m unittests src/lib.rs (target/debug/deps/attest_sample_crate-392ca9d12eda1cf9)\n\u{1b}[1m\u{1b}[92m     Running\u{1b}[0m tests/integration.rs (target/debug/deps/integration-12fc9e26ddfc6535)\n\u{1b}[1m\u{1b}[92m   Doc-tests\u{1b}[0m attest_sample_crate\n";
        assert_eq!(
            section_targets("", stderr, 3),
            vec!["attest_sample_crate", "integration", "doc-tests"]
        );
        // The names must be right, not merely three of them: a stripper that
        // ate the `(` would still count three and pair the wrong targets.
        assert!(section_targets("", stderr, 2).is_empty());
    }

    #[test]
    fn per_test_and_summary_line_parsers_reject_lookalikes() {
        assert!(parse_per_test("test a ... CORRUPTED_STATUS").is_none());
        assert!(parse_per_test("testing a ... ok").is_none());
        assert!(parse_per_test("test result: ok. 1 passed").is_none());
        assert_eq!(
            parse_per_test("test tests::x ... ok"),
            Some(("tests::x".to_string(), TestOutcome::Passed))
        );
        assert!(parse_summary("test result: ok. 36 pass").is_none());
        assert!(parse_summary("test result: weird. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.0s").is_none());
    }

    /// AC-ATTEST-P3-26 — the discovery line parser, against the committed real
    /// `--list` capture from this repo's own suite.
    #[test]
    fn ac_p3_26_the_list_parser_reads_the_real_list_fixture() {
        let raw = fixture("list.txt");
        let names = parse_list_output(&raw);
        assert_eq!(
            names.len(),
            37,
            "the fixture's own summary line says 37 tests"
        );
        assert!(names.contains(&"ac3_zero_bytes_under_agentrec_in_tempdir".to_string()));
        assert!(names.contains(&"persist::ac5b_oracle_real_corpus_measurement".to_string()));
        // Every returned name came from a `: test` line, and the trailer lines
        // (`37 tests, 0 benchmarks`, `EXIT: 0`) are not names.
        for name in &names {
            assert!(
                raw.contains(&format!("{name}: test")),
                "{name} is not a `: test` line"
            );
            assert!(!name.contains(' '), "{name}");
        }
        assert!(!names.iter().any(|n| n.contains("benchmark")));
        // A `: benchmark` line must not be mistaken for a test.
        assert!(parse_list_output("b: benchmark\nt: test\n") == vec!["t".to_string()]);
    }

    #[test]
    fn package_names_come_out_of_every_package_id_shape() {
        assert_eq!(
            package_name_from_id("path+file:///a/attest_sample_crate#0.0.0"),
            "attest_sample_crate"
        );
        assert_eq!(
            package_name_from_id("path+file:///a/b#agentrec@0.2.0"),
            "agentrec"
        );
    }

    // -- fixture-crate driven ------------------------------------------------

    fn copy_fixture_crate() -> tempfile::TempDir {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/attest_sample_crate");
        let tmp = tempfile::tempdir().unwrap();
        let dst = tmp.path();
        // Only the four TRACKED files — the in-tree fixture also carries a
        // multi-megabyte `target/` from earlier builds.
        std::fs::create_dir_all(dst.join("src")).unwrap();
        std::fs::create_dir_all(dst.join("tests")).unwrap();
        for rel in [
            "Cargo.toml",
            "Cargo.lock",
            "src/lib.rs",
            "tests/integration.rs",
        ] {
            std::fs::copy(src.join(rel), dst.join(rel)).unwrap();
        }
        tmp
    }

    #[test]
    fn discovery_enumerates_both_target_shapes_with_module_qualified_names() {
        let tmp = copy_fixture_crate();
        let found = CargoAdapter.discover(tmp.path()).unwrap();
        assert_eq!(
            found,
            vec![
                TestIdentity::new("attest_sample_crate", "tests::unit_add_works"),
                TestIdentity::new("attest_sample_crate", "tests::unit_double_works"),
                TestIdentity::new("attest_sample_crate", "tests::unit_ignored_test"),
                TestIdentity::new("integration", "integration_add_works"),
                TestIdentity::new("integration", "integration_double_works"),
            ],
            "discovery must be sorted and cover both shapes"
        );
    }

    #[test]
    fn body_hashes_resolve_inline_mod_tests_and_the_integration_target() {
        let tmp = copy_fixture_crate();
        let (ids, hashes) = discover_with_hashes(tmp.path()).unwrap();
        assert_eq!(hashes.len(), ids.len(), "every discovered test hashed");
        let unit = TestIdentity::new("attest_sample_crate", "tests::unit_add_works");
        let before = hashes[&unit];

        // A comment-only edit must NOT move the hash (token stream, not bytes).
        let lib = tmp.path().join("src/lib.rs");
        let text = std::fs::read_to_string(&lib).unwrap();
        std::fs::write(
            &lib,
            text.replace(
                "        assert_eq!(add(2, 2), 4);",
                "        // note\n        assert_eq!(add(2, 2), 4);",
            ),
        )
        .unwrap();
        let (_, after) = discover_with_hashes(tmp.path()).unwrap();
        assert_eq!(
            after[&unit], before,
            "a comment must not move the body hash"
        );

        // A real body edit must move it.
        let text = std::fs::read_to_string(&lib).unwrap();
        std::fs::write(&lib, text.replace("add(2, 2), 4", "add(3, 1), 4")).unwrap();
        let (_, changed) = discover_with_hashes(tmp.path()).unwrap();
        assert_ne!(changed[&unit], before);
    }

    /// AC-ATTEST-P3-23 — the four `recipe-invalid` causes stay distinct on real
    /// cargo output.
    #[test]
    fn ac_p3_23_the_staged_pipeline_keeps_its_recipe_invalid_causes_distinct() {
        let tmp = copy_fixture_crate();
        let root = tmp.path();

        // pass
        let r = CargoAdapter
            .run(
                root,
                &RunFilter::Exact {
                    target: "attest_sample_crate".into(),
                    fn_path: "tests::unit_add_works".into(),
                },
                &RunEnv::inherited(),
            )
            .unwrap();
        assert_eq!(r.results[0].outcome, Some(TestOutcome::Passed));
        assert_eq!(r.results[0].recipe_invalid, None);

        // ignored
        let r = CargoAdapter
            .run(
                root,
                &RunFilter::Exact {
                    target: "attest_sample_crate".into(),
                    fn_path: "tests::unit_ignored_test".into(),
                },
                &RunEnv::inherited(),
            )
            .unwrap();
        assert_eq!(
            r.results[0].recipe_invalid,
            Some(RecipeInvalidCause::Ignored)
        );

        // missing — stage 2 catches it, so nothing runs and exit 0 never
        // masquerades as a pass.
        let r = CargoAdapter
            .run(
                root,
                &RunFilter::Exact {
                    target: "attest_sample_crate".into(),
                    fn_path: "tests::no_such_test".into(),
                },
                &RunEnv::inherited(),
            )
            .unwrap();
        assert_eq!(
            r.results[0].recipe_invalid,
            Some(RecipeInvalidCause::Missing)
        );
        assert!(r.raw.is_empty(), "nothing ran");

        // harness — driven for real. A test calling `std::process::abort()` is
        // appended to THIS scratch copy of the fixture crate (never the
        // committed one, whose test count AC-ATTEST-P3-10 pins at 5), and the
        // scoped `--exact` run then produces the real shape: libtest prints its
        // `running 1 test` header, SIGABRT kills the binary, and no per-test
        // line and no summary are ever written.
        let lib = root.join("src/lib.rs");
        let text = std::fs::read_to_string(&lib).unwrap();
        std::fs::write(
            &lib,
            format!("{text}\n#[test]\nfn unit_aborts() {{ std::process::abort() }}\n"),
        )
        .unwrap();
        let r = CargoAdapter
            .run(
                root,
                &RunFilter::Exact {
                    target: "attest_sample_crate".into(),
                    fn_path: "unit_aborts".into(),
                },
                &RunEnv::inherited(),
            )
            .unwrap();
        assert_eq!(
            r.results[0].recipe_invalid,
            Some(RecipeInvalidCause::Harness),
            "raw: {}",
            r.raw
        );
        assert!(r.results[0].parse_failed, "{:?}", r.results[0]);
        assert!(r.results[0].outcome.is_none(), "{:?}", r.results[0]);
        assert!(
            r.raw.contains("running 1 test"),
            "the header libtest printed before the abort: {}",
            r.raw
        );
        // Second case, kept: the same cause from the parser's own branch on a
        // synthetic string, which pins the mapping independently of a signal.
        assert_eq!(
            single_test_result(&ident(), "running 1 test\ntest a ... ok\n").recipe_invalid,
            Some(RecipeInvalidCause::Harness)
        );
        // Restore the pre-abort source so the `build` case below breaks the
        // build for its OWN reason.
        std::fs::write(&lib, &text).unwrap();

        // build
        let lib = root.join("src/lib.rs");
        let text = std::fs::read_to_string(&lib).unwrap();
        std::fs::write(&lib, format!("{text}\nthis is not rust;\n")).unwrap();
        let r = CargoAdapter
            .run(
                root,
                &RunFilter::Exact {
                    target: "attest_sample_crate".into(),
                    fn_path: "tests::unit_add_works".into(),
                },
                &RunEnv::inherited(),
            )
            .unwrap();
        assert_eq!(r.results[0].recipe_invalid, Some(RecipeInvalidCause::Build));
    }

    /// AC-ATTEST-P4C-3, the pure half. The allowlist is a pinned decision, so
    /// it is tested by name rather than only through the e2e canary — an e2e
    /// pass cannot say WHICH names survived.
    #[test]
    fn scrubbed_pairs_keeps_the_allowlist_and_drops_everything_else() {
        let vars: Vec<(String, String)> = [
            ("PATH", "/bin"),
            ("HOME", "/h"),
            ("CARGO_HOME", "/ch"),
            ("RUSTUP_HOME", "/rh"),
            ("RUSTUP_TOOLCHAIN", "stable"),
            ("TMPDIR", "/tmp"),
            ("TERM", "xterm"),
            ("LLVM_COV", "/llvm-cov"),
            ("LLVM_PROFDATA", "/llvm-profdata"),
            ("ATTEST_CANARY", "leaked"),
            ("PATHOLOGICAL", "not PATH"),
            ("MY_LLVM_COV", "not a prefix match"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

        let kept: Vec<String> = scrubbed_pairs(vars).into_iter().map(|(k, _)| k).collect();

        // The ALLOW half: every pinned name, plus the `LLVM_` prefix class.
        for k in ENV_ALLOWLIST {
            assert!(kept.contains(&k.to_string()), "{k} must survive the scrub");
        }
        assert!(kept.contains(&"LLVM_COV".to_string()));
        assert!(kept.contains(&"LLVM_PROFDATA".to_string()));

        // The REFUSE half, including two near-misses a sloppy `contains`-style
        // match would wrongly keep.
        assert!(!kept.contains(&"ATTEST_CANARY".to_string()));
        assert!(!kept.contains(&"PATHOLOGICAL".to_string()));
        assert!(!kept.contains(&"MY_LLVM_COV".to_string()));
        assert_eq!(kept.len(), ENV_ALLOWLIST.len() + 2);
    }
}
