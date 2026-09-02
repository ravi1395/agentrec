//! `agentrec attest coverage [--all | <claim_id>...]` — the PRODUCER of
//! `.agentrec/attest-coverage.json`.
//!
//! Schema and the `over_stale` rule: `ATTEST-FORMAT.md` § "Coverage map".
//! Stated once there, not restated here. The types and the READ half
//! (`load_coverage_map`, `claims_touched_by`) live in
//! [`crate::attest::coverage`], which the daemon consumes; this module only
//! writes.
//!
//! # Why this spawns `cargo llvm-cov` by hand rather than reading its report
//!
//! `cargo llvm-cov` produces a WHOLE-RUN report. What the daemon needs is
//! per-TEST attribution, which only exists if each test runs in its own
//! process with its own `LLVM_PROFILE_FILE`. So this uses `cargo llvm-cov test
//! --no-run` for the instrumented BUILD only, then drives each test binary
//! directly — the loop Phase 1 Probe A measured
//! (`docs/verify/attest-coverage-spike.md` Appendix A2).
//!
//! # The toolchain recipe is not optional on this machine
//!
//! Spike §1: stock `cargo llvm-cov` fails on Homebrew `rustc`, whose sysroot
//! ships no `llvm-profdata`. `LLVM_COV`/`LLVM_PROFDATA` are honored from the
//! environment when set, and otherwise resolved out of a rustup `llvm-tools`
//! component. When neither works capture REFUSES with exit 2 and writes NO map
//! — an empty map reads to the daemon as "nothing is covered", which is the
//! dangerous direction.
//!
//! # Two under-attribution channels, one coarse detector
//!
//! (i) A SIGKILLed child writes no profile at all. There is no detector; the
//! founder ruling (AC-ATTEST-P1-3) covers it structurally instead — every test
//! in a non-`lib` target of the `agentrec` package gets `over_stale`, whether
//! or not it actually spawned anything. (ii) Untemplated `default_*.profraw`
//! files leak into the crate root by an UNDETERMINED mechanism; they are moved
//! aside and COUNTED, which only says "we saw N leaks during this capture" and
//! not which test lost coverage. Disclosed as coarse, not solved.

use crate::attest::adapter_cargo::{parse_artifact_stream, TargetInfo};
use crate::attest::coverage::{CoverageEntry, CoverageMap, DEFAULT_COVERAGE_REL};
use crate::attest::lock::read_attest;
use agentrec_core::attest::fold::fold_claims;
use agentrec_core::attest::types::{ClaimId, TestIdentity};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The one `over_stale` pattern this producer ever emits.
pub const OVER_STALE_CLI_SRC: &str = "cli/src/**";

/// The per-root build cache. NOT part of any pinned tree: it is shared across
/// every capture and every `attest verify` replay in this root, which is the
/// point — a fresh target dir per run rebuilds the world each time.
pub fn attest_target_dir(root: &Path) -> PathBuf {
    crate::agentrec_dir(root).join("attest-target")
}

pub fn coverage_path(root: &Path) -> PathBuf {
    root.join(DEFAULT_COVERAGE_REL)
}

struct LlvmTools {
    cov: PathBuf,
    profdata: PathBuf,
}

/// `LLVM_COV`/`LLVM_PROFDATA` if set, else the highest-sorting rustup
/// `llvm-tools` component found. Never the active sysroot — spike §1 measured
/// that failing on this machine's Homebrew toolchain.
fn resolve_llvm_tools() -> Result<LlvmTools, String> {
    if let (Ok(cov), Ok(profdata)) = (std::env::var("LLVM_COV"), std::env::var("LLVM_PROFDATA")) {
        return Ok(LlvmTools {
            cov: PathBuf::from(cov),
            profdata: PathBuf::from(profdata),
        });
    }
    let home = std::env::var("HOME").map_err(|_| "HOME is unset".to_string())?;
    let toolchains = Path::new(&home).join(".rustup/toolchains");
    let entries = std::fs::read_dir(&toolchains)
        .map_err(|e| format!("cannot read {}: {e}", toolchains.display()))?;
    let mut found: Vec<PathBuf> = Vec::new();
    for tc in entries.flatten() {
        let Ok(hosts) = std::fs::read_dir(tc.path().join("lib/rustlib")) else {
            continue;
        };
        for host in hosts.flatten() {
            let bin = host.path().join("bin");
            if bin.join("llvm-profdata").exists() && bin.join("llvm-cov").exists() {
                found.push(bin);
            }
        }
    }
    found.sort();
    let bin = found.pop().ok_or_else(|| {
        format!(
            "no rustup llvm-tools component under {} — install it with \
             `rustup component add llvm-tools`, or set LLVM_COV and LLVM_PROFDATA",
            toolchains.display()
        )
    })?;
    Ok(LlvmTools {
        cov: bin.join("llvm-cov"),
        profdata: bin.join("llvm-profdata"),
    })
}

fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `git rev-parse HEAD`, or empty when the crate is not in a git repo — the
/// commit is provenance ON the map, not a precondition for capturing one.
fn head_commit(dir: &Path) -> String {
    // attest: sanctioned spawn (Phase 4 census)
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Tooling availability, checked BEFORE anything is written. Returns the tools
/// or the user-facing reason they are unavailable.
pub fn tooling_status() -> Result<(), String> {
    resolve_llvm_tools()?;
    // attest: sanctioned spawn (Phase 4 census)
    let ok = Command::new(cargo_bin())
        .args(["llvm-cov", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err("`cargo llvm-cov` is not installed (cargo install cargo-llvm-cov)".to_string())
    }
}

pub fn run(
    root: &Path,
    crate_path: Option<&Path>,
    all: bool,
    ids: &[String],
) -> Result<(), String> {
    if !all && ids.is_empty() {
        return Err("specify --all or one or more claim ids".to_string());
    }
    // CANONICALIZED, and this is load-bearing rather than tidiness.
    // `adapter_cargo::build_targets` sets `CARGO_TARGET_DIR` to
    // `crate_root.join("target")` while ALSO setting `current_dir(crate_root)`,
    // so a RELATIVE `--crate agentrec-core` yields the relative target dir
    // `agentrec-core/target`, which cargo then resolves against the child's cwd
    // — producing `agentrec-core/agentrec-core/target`. Measured live on a
    // clone of this repo: 284 MB written to the wrong place, and the stray
    // directory then made the tree dirty. An absolute path cannot compound.
    let canonical;
    let crate_root = match crate_path {
        Some(p) => {
            canonical = p
                .canonicalize()
                .map_err(|e| format!("cannot resolve --crate {}: {e}", p.display()))?;
            canonical.as_path()
        }
        None => root,
    };

    // Refuse before writing anything: a partial or empty map is worse than no
    // map, because the daemon cannot tell the two apart.
    if let Err(why) = tooling_status() {
        eprintln!("agentrec: coverage tooling unavailable: {why}");
        eprintln!("agentrec: no coverage map written");
        std::process::exit(2);
    }
    let tools = resolve_llvm_tools()?;

    let (events, _) = read_attest(root)?;
    let folded = fold_claims(&events);

    let mut wanted: BTreeMap<TestIdentity, ClaimId> = BTreeMap::new();
    for (identity, claim_id) in &folded.by_identity {
        if all || ids.iter().any(|i| i == claim_id.as_str()) {
            wanted.insert(identity.clone(), claim_id.clone());
        }
    }
    if wanted.is_empty() {
        return Err("no claims with a test identity matched".to_string());
    }

    let base = attest_target_dir(root);
    let target_dir = base.join("llvm-cov");
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| format!("cannot create {}: {e}", target_dir.display()))?;

    let targets = instrumented_build(crate_root, &target_dir, &tools)?;
    let objects: Vec<PathBuf> = targets.iter().map(|t| t.executable.clone()).collect();
    let source_roots = source_roots(crate_root);
    let commit = head_commit(crate_root);
    let captured_at = now_ms();

    let profraw_dir = base.join("profraw");
    let _ = std::fs::remove_dir_all(&profraw_dir);
    std::fs::create_dir_all(&profraw_dir)
        .map_err(|e| format!("cannot create {}: {e}", profraw_dir.display()))?;

    let mut map = CoverageMap {
        version: 1,
        granularity: "file".to_string(),
        tests: BTreeMap::new(),
    };
    let (mut captured, mut skipped) = (0usize, 0usize);

    for (identity, claim_id) in &wanted {
        let Some(target) = targets.iter().find(|t| t.name == identity.target) else {
            skipped += 1;
            continue;
        };
        let dir = profraw_dir.join(format!(
            "{}__{}",
            identity.target,
            sanitize(&identity.fn_path)
        ));
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let files = capture_one(
            &tools,
            target,
            &identity.fn_path,
            &dir,
            &objects,
            &source_roots,
        )?;
        map.tests.insert(
            format!("{}::{}", identity.target, identity.fn_path),
            CoverageEntry {
                claim_id: claim_id.clone(),
                files,
                over_stale: over_stale_for(target),
                captured_at,
                commit: commit.clone(),
            },
        );
        captured += 1;
    }

    let leaked = sweep_leaked_profraw(crate_root, &base.join("leaked"))?;
    write_map_atomically(root, &map)?;

    println!(
        "captured {captured} test(s), {skipped} skipped (no built target), \
         {leaked} leaked profraw file(s) moved aside"
    );
    println!("wrote {}", coverage_path(root).display());
    Ok(())
}

/// A profraw sub-directory name from a test path: `::` and `/` would nest.
fn sanitize(fn_path: &str) -> String {
    fn_path
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// The source directories whose files are kept in a test's file set: exactly
/// those that EXIST, from a fixed candidate list. One rule covering both
/// shapes — this repo yields the pinned `cli/src` + `agentrec-core/src`
/// restriction, and a single-crate fixture yields its own `src`.
fn source_roots(crate_root: &Path) -> Vec<String> {
    ["cli/src", "agentrec-core/src", "src"]
        .into_iter()
        .filter(|d| crate_root.join(d).is_dir())
        .map(str::to_string)
        .collect()
}

/// `["cli/src/**"]` for a test that may spawn the binary, `[]` otherwise. The
/// rule is structural, not measured — see this module's doc comment.
fn over_stale_for(target: &TargetInfo) -> Vec<String> {
    if target.kind != "lib" && target.package == "agentrec" {
        vec![OVER_STALE_CLI_SRC.to_string()]
    } else {
        Vec::new()
    }
}

/// The instrumented build, in two steps because `cargo llvm-cov` has no single
/// invocation that both instruments AND reports cargo's artifact JSON.
///
/// MEASURED, not assumed: `cargo llvm-cov test --no-run` is rejected outright
/// (`--no-run is specific to [nextest,...] and not supported for subcommand
/// 'test'`), and `cargo llvm-cov --no-run` rejects `--message-format`. The
/// supported path is `show-env`, which prints the instrumenting environment
/// (an `RUSTC_WRAPPER`, the profile-file template, the coverage target dir);
/// exporting it into a plain `cargo test --no-run --message-format=json` then
/// yields instrumented binaries AND the artifact stream the adapter parses.
fn instrumented_build(
    crate_root: &Path,
    target_dir: &Path,
    tools: &LlvmTools,
) -> Result<Vec<TargetInfo>, String> {
    // attest: sanctioned spawn (Phase 4 census)
    let env_out = Command::new(cargo_bin())
        .args(["llvm-cov", "show-env", "--sh"])
        .current_dir(crate_root)
        .env("LLVM_COV", &tools.cov)
        .env("LLVM_PROFDATA", &tools.profdata)
        .env("CARGO_TARGET_DIR", target_dir)
        .output()
        .map_err(|e| format!("cannot run cargo llvm-cov show-env: {e}"))?;
    if !env_out.status.success() {
        return Err(format!(
            "cargo llvm-cov show-env failed: {}",
            String::from_utf8_lossy(&env_out.stderr)
        ));
    }

    // attest: sanctioned spawn (Phase 4 census)
    let mut build = Command::new(cargo_bin());
    build
        .args(["test", "--no-run", "--message-format=json"])
        .current_dir(crate_root)
        .env("LLVM_COV", &tools.cov)
        .env("LLVM_PROFDATA", &tools.profdata)
        .env("CARGO_TARGET_DIR", target_dir);
    for (k, v) in parse_show_env(&String::from_utf8_lossy(&env_out.stdout)) {
        build.env(k, v);
    }
    let out = build
        .output()
        .map_err(|e| format!("cannot run cargo test --no-run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "instrumented build failed in {}:\n{}",
            crate_root.display(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(parse_artifact_stream(&String::from_utf8_lossy(&out.stdout)))
}

/// `export KEY=VALUE` lines from `cargo llvm-cov show-env --sh`.
///
/// **Values are quoted OR bare, and both must be taken.** The real capture
/// mixes them in one block: `LLVM_PROFILE_FILE` and the wrapper RUSTFLAGS come
/// single-quoted, while `RUSTC_WRAPPER`, `CARGO_LLVM_COV` and the
/// `__CARGO_LLVM_COV_*` markers come bare. An earlier version of this parser
/// accepted only the quoted form and silently dropped `RUSTC_WRAPPER` — which
/// IS the instrumentation — so every binary built fine, ran fine, and produced
/// no profile at all, yielding a map of empty file sets with a zero exit code.
/// Non-`export` lines (the tool's own `warning:`/`info:` chatter) are skipped.
fn parse_show_env(sh: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in sh.lines() {
        let Some(rest) = line.trim().strip_prefix("export ") else {
            continue;
        };
        let Some((key, value)) = rest.split_once('=') else {
            continue;
        };
        let value = match value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            Some(unquoted) => unquoted,
            None => value,
        };
        out.push((key.to_string(), value.to_string()));
    }
    out
}

/// One test, one process, one profile: run → merge → export → file set.
fn capture_one(
    tools: &LlvmTools,
    target: &TargetInfo,
    fn_path: &str,
    dir: &Path,
    objects: &[PathBuf],
    source_roots: &[String],
) -> Result<Vec<String>, String> {
    // attest: sanctioned spawn (Phase 4 census)
    let _ = Command::new(&target.executable)
        .args(["--exact", fn_path, "--test-threads=1"])
        .env("LLVM_PROFILE_FILE", dir.join("p-%p-%10m.profraw"))
        .output()
        .map_err(|e| format!("cannot run {}: {e}", target.executable.display()))?;

    let raws: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "profraw"))
        .collect();
    if raws.is_empty() {
        // No profile at all — the SIGKILL channel, or a test that never ran. An
        // EMPTY file set is the honest answer; `over_stale` is what covers such
        // a test, not a guessed set.
        return Ok(Vec::new());
    }

    let profdata = dir.join("t.profdata");
    // attest: sanctioned spawn (Phase 4 census)
    let merge = Command::new(&tools.profdata)
        .arg("merge")
        .arg("-sparse")
        .arg("-o")
        .arg(&profdata)
        .args(&raws)
        .output()
        .map_err(|e| format!("cannot run llvm-profdata: {e}"))?;
    if !merge.status.success() {
        return Err(format!(
            "llvm-profdata merge failed: {}",
            String::from_utf8_lossy(&merge.stderr)
        ));
    }

    let mut args: Vec<std::ffi::OsString> = vec![
        "export".into(),
        "-format=text".into(),
        format!("-instr-profile={}", profdata.display()).into(),
    ];
    for obj in objects {
        args.push("-object".into());
        args.push(obj.into());
    }
    // attest: sanctioned spawn (Phase 4 census)
    let export = Command::new(&tools.cov)
        .args(&args)
        .output()
        .map_err(|e| format!("cannot run llvm-cov: {e}"))?;
    if !export.status.success() {
        return Err(format!(
            "llvm-cov export failed: {}",
            String::from_utf8_lossy(&export.stderr)
        ));
    }
    Ok(project_files(
        &String::from_utf8_lossy(&export.stdout),
        source_roots,
    ))
}

/// llvm-cov's JSON export → the repo-relative files with EXECUTED lines.
///
/// `summary.lines.covered > 0` is the predicate (spike §6): a file merely
/// COMPILED into the binary appears in the export with zero covered lines, and
/// counting those as coverage would put every source file in every test's set.
fn project_files(export_json: &str, source_roots: &[String]) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(export_json) else {
        return Vec::new();
    };
    let empty = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for data in v.get("data").and_then(|d| d.as_array()).unwrap_or(&empty) {
        for file in data
            .get("files")
            .and_then(|f| f.as_array())
            .unwrap_or(&empty)
        {
            let covered = file
                .pointer("/summary/lines/covered")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            if covered == 0 {
                continue;
            }
            let Some(name) = file.get("filename").and_then(|n| n.as_str()) else {
                continue;
            };
            // The absolute path is the extract's or the crate's; keep the
            // suffix starting at a known source root, so the map is
            // repo-relative and comparable across trees.
            for src in source_roots {
                let needle = format!("/{src}/");
                if let Some(idx) = name.find(&needle) {
                    let rel = name[idx + 1..].to_string();
                    if !out.contains(&rel) {
                        out.push(rel);
                    }
                    break;
                }
            }
        }
    }
    out.sort();
    out
}

/// Move untemplated `default_*.profraw` leaks out of the crate root and count
/// them. Coarse by construction — see this module's doc comment.
fn sweep_leaked_profraw(crate_root: &Path, dest: &Path) -> Result<usize, String> {
    let Ok(entries) = std::fs::read_dir(crate_root) else {
        return Ok(0);
    };
    let mut moved = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !(name.starts_with("default_") && name.ends_with(".profraw")) {
            continue;
        }
        std::fs::create_dir_all(dest)
            .map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
        std::fs::rename(&path, dest.join(name))
            .map_err(|e| format!("cannot move {}: {e}", path.display()))?;
        moved += 1;
    }
    Ok(moved)
}

/// tmp + rename, so a reader never sees a half-written map. The tmp name is
/// fresh (pid + millis) and opened `create_new`, so the open cannot land on a
/// planted FIFO and cannot clobber a concurrent writer's tmp.
fn write_map_atomically(root: &Path, map: &CoverageMap) -> Result<(), String> {
    let path = coverage_path(root);
    let dir = path.parent().unwrap_or(root).to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let body =
        serde_json::to_string_pretty(map).map_err(|e| format!("cannot serialize map: {e}"))?;
    let tmp = dir.join(format!(
        ".attest-coverage.{}.{}.tmp",
        std::process::id(),
        now_ms()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
    file.write_all(body.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    file.sync_all()
        .map_err(|e| format!("cannot fsync {}: {e}", tmp.display()))?;
    drop(file);
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot rename into place: {e}"))?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn a_file_compiled_but_never_executed_is_not_coverage() {
        let json = r#"{"data":[{"files":[
            {"filename":"/r/cli/src/a.rs","summary":{"lines":{"covered":3}}},
            {"filename":"/r/cli/src/b.rs","summary":{"lines":{"covered":0}}}
        ]}]}"#;
        assert_eq!(
            project_files(json, &["cli/src".to_string()]),
            vec!["cli/src/a.rs".to_string()]
        );
        // A file outside every source root is dropped, not relativized wrongly.
        let outside = r#"{"data":[{"files":[
            {"filename":"/r/vendor/x.rs","summary":{"lines":{"covered":9}}}
        ]}]}"#;
        assert!(project_files(outside, &["cli/src".to_string()]).is_empty());
    }

    #[test]
    fn over_stale_fires_only_for_binary_spawning_targets_of_this_package() {
        let mk = |package: &str, kind: &str| TargetInfo {
            package: package.into(),
            name: "t".into(),
            kind: kind.into(),
            src_path: PathBuf::new(),
            executable: PathBuf::new(),
        };
        assert_eq!(
            over_stale_for(&mk("agentrec", "test")),
            vec![OVER_STALE_CLI_SRC]
        );
        // The lib target does not spawn the binary.
        assert!(over_stale_for(&mk("agentrec", "lib")).is_empty());
        // Another package's integration tests are not this binary's spawners.
        assert!(over_stale_for(&mk("attest_sample_crate", "test")).is_empty());
    }

    #[test]
    fn show_env_parsing_takes_quoted_and_bare_values_alike() {
        // Verbatim shape of a real `cargo llvm-cov show-env --sh` capture on
        // this machine, warning/info chatter and mixed quoting included.
        let sh = "warning: export-prefix has been renamed to --sh\n\
                  info: cargo-llvm-cov currently setting cfg(coverage)\n\
                  export LLVM_PROFILE_FILE='/t/target/p-%p-%10m.profraw'\n\
                  export __CARGO_LLVM_COV_RUSTC_WRAPPER=1\n\
                  export RUSTC_WRAPPER=/who/.cargo/bin/cargo-llvm-cov\n\
                  export CARGO_LLVM_COV_TARGET_DIR=/t/target\n";
        let got = parse_show_env(sh);
        let key = |k: &str| {
            got.iter()
                .find(|(a, _)| a == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("{k} missing from {got:?}"))
        };
        assert_eq!(key("LLVM_PROFILE_FILE"), "/t/target/p-%p-%10m.profraw");
        // The bare-value half. Dropping this one drops instrumentation
        // entirely, and the failure is SILENT: exit 0, empty file sets.
        assert_eq!(key("RUSTC_WRAPPER"), "/who/.cargo/bin/cargo-llvm-cov");
        assert_eq!(key("__CARGO_LLVM_COV_RUSTC_WRAPPER"), "1");
        assert_eq!(key("CARGO_LLVM_COV_TARGET_DIR"), "/t/target");
        // Chatter is not a variable.
        assert_eq!(got.len(), 4, "{got:?}");
    }

    #[test]
    fn source_roots_pick_the_dirs_that_exist() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        assert_eq!(source_roots(tmp.path()), vec!["src".to_string()]);
        std::fs::create_dir_all(tmp.path().join("cli/src")).unwrap();
        assert_eq!(
            source_roots(tmp.path()),
            vec!["cli/src".to_string(), "src".to_string()]
        );
    }
}
