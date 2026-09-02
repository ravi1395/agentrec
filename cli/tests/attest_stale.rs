//! Phase 4B integration tests: the REAL `agentrec record` daemon appending
//! `stale` events to `.agentrec/attest.jsonl` when a write lands in a claim's
//! coverage scope. ACs: `IMPLEMENTATION.md` § "Phase 4B — daemon
//! stale-marking".
//!
//! Every test here drives a real spawned daemon against a tempdir root. The
//! production dogfood daemon (this repo's own `~/Projects/agentrec` recorder)
//! is never touched: `init` runs with `--no-service` so no launchd unit is
//! written, and every root is a fresh `tempfile::tempdir()`.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures; there is no attacker-supplied
//    FIFO to block on. Production reads stay lint-enforced (clippy.toml).

use agentrec_core::attest::events::{parse_log, AttestEvent, StaleCause};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

// Claim ids are validated by `ClaimId`'s deserializer, so these are real,
// well-formed ids rather than hand-invented strings.
const CLAIM_A: &str = "c_00000000010W3GE1R70W3GE1R7";
const CLAIM_B: &str = "c_00000000010W3GE1R70W3GE1R8";
const CLAIM_C: &str = "c_00000000010W3GE1R70W3GE1R9";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

fn agentrec(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .args(["--root", root.to_str().unwrap()])
        .output()
        .expect("run agentrec")
}

fn init(root: &Path) {
    Command::new("git")
        .args(["init", "-q"])
        .arg(root)
        .status()
        .unwrap();
    // --no-service: never write a real launchd/systemd unit (the D46 scar).
    let out = agentrec(root, &["init", "--no-hook", "--no-service"]);
    assert!(out.status.success(), "init failed: {out:?}");
}

fn spawn_record(root: &Path) -> Child {
    Command::new(bin())
        .args(["record", "--root", root.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn record")
}

fn poll_until<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn wait_for_live_daemon(root: &Path) {
    let ok = poll_until(Duration::from_secs(15), || {
        let text = std::fs::read_to_string(root.join(".agentrec/state.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let pid = v.get("pid")?.as_u64()?;
        let epoch_nonce = v.get("epoch_nonce")?.as_str()?;
        let armed_nonce = v.get("watcher_armed_nonce")?.as_str()?;
        (pid != 0 && !epoch_nonce.is_empty() && armed_nonce == epoch_nonce).then_some(())
    });
    assert!(
        ok.is_some(),
        "daemon never reached watcher-armed state in state.json"
    );
}

/// RAII: a RED run must not leak a live daemon per run (`Child::drop` does not
/// reap). Mirrors `integration.rs::SingleDaemonGuard`, which lives in another
/// test binary and cannot be imported.
struct DaemonGuard(Option<Child>);

impl DaemonGuard {
    fn spawn(root: &Path) -> Self {
        let child = spawn_record(root);
        wait_for_live_daemon(root);
        DaemonGuard(Some(child))
    }
    fn kill(&mut self) {
        if let Some(mut c) = self.0.take() {
            unsafe { libc::kill(c.id() as libc::pid_t, libc::SIGKILL) };
            let _ = c.wait();
        }
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

fn write_coverage_map(root: &Path, entries: &[(&str, &str, &[&str], &[&str])]) {
    write_coverage_map_at(root, ".agentrec/attest-coverage.json", entries)
}

fn write_coverage_map_at(root: &Path, rel: &str, entries: &[(&str, &str, &[&str], &[&str])]) {
    let tests: Vec<String> = entries
        .iter()
        .map(|(name, claim, files, over)| {
            let files = files
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect::<Vec<_>>()
                .join(",");
            let over = over
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                r#""{name}":{{"claim_id":"{claim}","files":[{files}],"over_stale":[{over}],"captured_at":1,"commit":"deadbeef"}}"#
            )
        })
        .collect();
    let body = format!(
        r#"{{"version":1,"granularity":"file","tests":{{{}}}}}"#,
        tests.join(",")
    );
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn write_derives(root: &Path, claims: &[&str]) {
    let mut body = String::new();
    for (i, c) in claims.iter().enumerate() {
        body.push_str(&format!(
            r#"{{"kind":"derive","ts":{},"claim_id":"{c}","test_identity":{{"target":"tgt","fn_path":"t{i}"}}}}"#,
            i + 1
        ));
        body.push('\n');
    }
    std::fs::write(root.join(".agentrec/attest.jsonl"), body).unwrap();
}

fn attest_events(root: &Path) -> Vec<AttestEvent> {
    match std::fs::read_to_string(root.join(".agentrec/attest.jsonl")) {
        Ok(b) => parse_log(&b).0,
        Err(_) => Vec::new(),
    }
}

fn stales(root: &Path) -> Vec<(String, StaleCause)> {
    attest_events(root)
        .into_iter()
        .filter_map(|e| match e {
            AttestEvent::Stale {
                claim_id, cause, ..
            } => Some((claim_id.as_str().to_string(), cause)),
            _ => None,
        })
        .collect()
}

/// Writes `path` (creating parent dirs) and waits for the daemon to have
/// recorded it, so the settled-batch flush that carries the attest hook has
/// demonstrably run. Returns the observed stale list at that point.
fn write_and_settle(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
    let seen = poll_until(Duration::from_secs(25), || {
        // `open.json` is the crash journal for the OPEN turn — it names the
        // files staged by the flush, which is the same flush the attest hook
        // rides. Waiting on it is what makes these assertions deterministic
        // rather than sleep-timed.
        let text = std::fs::read_to_string(root.join(".agentrec/open.json")).ok()?;
        text.contains(rel).then_some(())
    });
    assert!(
        seen.is_some(),
        "daemon never staged {rel} — the flush the attest hook rides never ran"
    );
}

/// AC-ATTEST-P4B-1 / -2 / -3: an exact coverage hit stales exactly its claim
/// with a `file-write` cause; an `over_stale` glob hit stales exactly its claim
/// with a `coverage-incomplete` cause naming the scope; an unmapped path stales
/// nothing.
///
/// All three legs share one daemon deliberately: the "unmapped path stales
/// nothing" leg is only meaningful against a daemon that has been PROVEN to
/// stale in this same process (the two legs before it), or a daemon that was
/// simply dead would pass it.
#[test]
fn ac_p4b_1_2_3_exact_glob_and_unmapped_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    write_coverage_map(
        root,
        &[
            ("tgt::t0", CLAIM_A, &["src/a.rs"], &[]),
            ("tgt::t1", CLAIM_B, &["src/b.rs"], &[]),
            ("tgt::t2", CLAIM_C, &[], &["cli/src/**"]),
        ],
    );
    write_derives(root, &[CLAIM_A, CLAIM_B, CLAIM_C]);

    let mut daemon = DaemonGuard::spawn(root);

    write_and_settle(root, "src/a.rs", "fn a() {}");
    let after_a = stales(root);
    assert_eq!(
        after_a,
        vec![(
            CLAIM_A.to_string(),
            StaleCause::FileWrite {
                path: "src/a.rs".into()
            }
        )],
        "a write to a covered file must stale exactly its claim, once"
    );

    write_and_settle(root, "cli/src/x.rs", "fn x() {}");
    let after_c: Vec<_> = stales(root).into_iter().skip(after_a.len()).collect();
    assert_eq!(
        after_c,
        vec![(
            CLAIM_C.to_string(),
            StaleCause::CoverageIncomplete {
                scope: "cli/src/**".into()
            }
        )],
        "an over_stale glob hit must stale only the coarse claim, as coverage-incomplete"
    );

    let before_readme = stales(root).len();
    write_and_settle(root, "README.md", "# hi");
    daemon.kill();
    assert_eq!(
        stales(root).len(),
        before_readme,
        "a write to an unmapped path must append nothing"
    );
}

/// AC-ATTEST-P4B-4: no coverage map at all — the daemon records normally and
/// never creates `attest.jsonl`.
#[test]
fn ac_p4b_4_absent_coverage_map_appends_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    let mut daemon = DaemonGuard::spawn(root);
    write_and_settle(root, "src/a.rs", "fn a() {}");
    daemon.kill();

    assert!(
        !root.join(".agentrec/attest.jsonl").exists(),
        "no coverage map must mean no attest.jsonl is created at all"
    );
}

/// AC-ATTEST-P4B-5: the map is re-read while the daemon runs. The rewrite is
/// BYTE-LENGTH-PRESERVING (`src/a.rs` -> `src/b.rs`) on purpose: an mtime- or
/// length-based reload trigger would miss it, so this test discriminates the
/// content-hash trigger the daemon actually uses.
#[test]
fn ac_p4b_5_coverage_map_reloads_while_the_daemon_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    write_coverage_map(
        root,
        &[
            ("tgt::t0", CLAIM_A, &["src/a.rs"], &[]),
            ("tgt::t1", CLAIM_B, &["src/b.rs"], &[]),
        ],
    );
    write_derives(root, &[CLAIM_A, CLAIM_B]);

    let mut daemon = DaemonGuard::spawn(root);

    // Positive control first: the ORIGINAL map is live.
    write_and_settle(root, "src/a.rs", "fn a() {}");
    assert_eq!(
        stales(root).into_iter().map(|(c, _)| c).collect::<Vec<_>>(),
        vec![CLAIM_A.to_string()]
    );

    // Remap A onto src/b.rs, same file length.
    write_coverage_map(
        root,
        &[
            ("tgt::t0", CLAIM_A, &["src/b.rs"], &[]),
            ("tgt::t1", CLAIM_B, &["src/b.rs"], &[]),
        ],
    );

    write_and_settle(root, "src/b.rs", "fn b() {}");
    daemon.kill();

    let after: Vec<String> = stales(root).into_iter().skip(1).map(|(c, _)| c).collect();
    assert_eq!(after.len(), 2, "expected both claims staled, got {after:?}");
    assert!(after.contains(&CLAIM_A.to_string()), "{after:?}");
    assert!(after.contains(&CLAIM_B.to_string()), "{after:?}");
}

/// AC-ATTEST-P4B-6: `attest_stale = false` in `config.toml` disables the
/// append entirely. The fixture is otherwise IDENTICAL to
/// `ac_p4b_1_2_3_exact_glob_and_unmapped_writes`'s first leg, which is what
/// makes this a discriminating off-switch test rather than a test of a broken
/// fixture.
#[test]
fn ac_p4b_6_config_off_switch_appends_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let cfg = root.join(".agentrec/config.toml");
    let mut body = std::fs::read_to_string(&cfg).unwrap_or_default();
    body.push_str("\nattest_stale = false\n");
    std::fs::write(&cfg, body).unwrap();
    write_coverage_map(root, &[("tgt::t0", CLAIM_A, &["src/a.rs"], &[])]);
    write_derives(root, &[CLAIM_A]);

    let mut daemon = DaemonGuard::spawn(root);
    write_and_settle(root, "src/a.rs", "fn a() {}");
    daemon.kill();

    assert!(
        stales(root).is_empty(),
        "attest_stale = false must append nothing: {:?}",
        stales(root)
    );
}

/// AC-ATTEST-P4B-7 — the plan's concurrent-writer AC: while the daemon appends
/// `stale` lines, separate CLI PROCESSES append through the same lock and no
/// line in `attest.jsonl` is torn.
///
/// **SUBSTITUTION, recorded rather than quietly satisfied.** The AC text names
/// `verdict` lines appended via `append_attest_locked`. Two constraints move
/// it: `cli` has no `[lib]` target, so a test binary cannot call
/// `append_attest_locked` at all (everything goes through the built binary),
/// and no `attest verify` verb exists at HEAD — that is chunk A's. The
/// concurrent writer here is therefore `agentrec attest derive`, which appends
/// `derive` events through the SAME `append_attest_locked` choke point. The
/// property under test — two independent processes appending concurrently
/// leave no torn line — does not depend on the event kind; the substitution is
/// in which kind, not in what is proven.
///
/// The three derive processes race deliberately: each reads `attest.jsonl`
/// before any of them has appended, so each mints and appends its own batch.
/// Counting is exact rather than approximate — each process prints its own
/// `N new` and the sum is what must appear in the file.
#[test]
fn ac_p4b_7_concurrent_cli_appends_leave_no_torn_line() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    write_coverage_map(root, &[("tgt::t0", CLAIM_A, &["src/a.rs"], &[])]);
    write_derives(root, &[CLAIM_A]);

    // A private copy of the committed sample crate, in its OWN tempdir — never
    // under the daemon's root. A cargo build inside the watched tree would
    // bury the one write this test cares about under thousands of `target/`
    // events and make the assertion below meaningless.
    let crate_home = tempfile::tempdir().unwrap();
    let crate_src =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/attest_sample_crate");
    let crate_dst = crate_home.path().join("sample_crate");
    copy_dir(&crate_src, &crate_dst);

    // Warm the crate's build against a THROWAWAY root, so the three racing
    // processes below contend on the attest lock rather than on cargo's.
    let warm = tempfile::tempdir().unwrap();
    init(warm.path());
    let out = agentrec(
        warm.path(),
        &["attest", "derive", "--crate", crate_dst.to_str().unwrap()],
    );
    assert!(out.status.success(), "warm-up derive failed: {out:?}");

    let mut daemon = DaemonGuard::spawn(root);

    // The daemon's side of the race: a burst of writes to the covered file.
    let writer_root = root.to_path_buf();
    let writer = std::thread::spawn(move || {
        for i in 0..40 {
            let p = writer_root.join("src/a.rs");
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, format!("fn a() {{ let _ = {i}; }}")).unwrap();
            std::thread::sleep(Duration::from_millis(75));
        }
    });

    let handles: Vec<_> = (0..3)
        .map(|_| {
            let (root, crate_dst) = (root.to_path_buf(), crate_dst.clone());
            std::thread::spawn(move || {
                agentrec(
                    &root,
                    &["attest", "derive", "--crate", crate_dst.to_str().unwrap()],
                )
            })
        })
        .collect();

    let mut expected_new = 0usize;
    for h in handles {
        let out = h.join().unwrap();
        assert!(out.status.success(), "derive failed: {out:?}");
        let text = String::from_utf8_lossy(&out.stdout);
        expected_new += parse_new_count(&text);
    }
    writer.join().unwrap();
    // The burst is continuous, so the daemon's flush lands on the debounce
    // settle AFTER the last write — wait for it rather than killing mid-burst.
    let saw_stale = poll_until(Duration::from_secs(25), || {
        (!stales(root).is_empty()).then_some(())
    });
    daemon.kill();
    assert!(
        saw_stale.is_some(),
        "the daemon never appended a stale event — the race never happened"
    );

    let body = std::fs::read_to_string(root.join(".agentrec/attest.jsonl")).unwrap();
    let (events, census) = parse_log(&body);
    assert_eq!(census.unparsed_lines, 0, "torn line(s) in attest.jsonl");
    assert_eq!(census.unknown_kind_lines, 0);

    let derives = events
        .iter()
        .filter(|e| matches!(e, AttestEvent::Derive { .. }))
        .count();
    // 1 hand-written seed derive + every `new` the three processes reported.
    assert_eq!(derives, 1 + expected_new, "derive lines lost or duplicated");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AttestEvent::Stale { .. })),
        "stale events vanished from the final read"
    );
}

fn parse_new_count(stdout: &str) -> usize {
    // "discovered N test(s): X new, ..."
    stdout
        .split(':')
        .nth(1)
        .and_then(|tail| tail.split(" new").next())
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("cannot parse derive stdout: {stdout}"))
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P4C-6 — producer and daemon resolve ONE coverage-map path
// ---------------------------------------------------------------------------

/// The daemon half. With `attest_coverage_path` set, the daemon must stale from
/// the map at THAT path — and must NOT be rescued by a map sitting at the
/// default location.
///
/// The decoy at the default path is the discriminating part. Before the fix the
/// producer wrote the default path while the daemon read the configured one, so
/// a test that only placed the map at the configured path could not tell a
/// correct daemon from one reading the default. Here the decoy names a
/// DIFFERENT claim, so whichever file the daemon actually read is visible in
/// the appended event.
#[test]
fn ac_p4c_6_the_daemon_stales_from_the_configured_coverage_path() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);

    std::fs::write(
        root.join(".agentrec/config.toml"),
        "attest_coverage_path = \"custom/cov.json\"\n",
    )
    .unwrap();

    // The real map, at the CONFIGURED path.
    write_coverage_map_at(
        root,
        "custom/cov.json",
        &[("tgt::t0", CLAIM_A, &["src/a.rs"], &[])],
    );
    // The decoy, at the DEFAULT path, mapping the same file to another claim.
    write_coverage_map_at(
        root,
        ".agentrec/attest-coverage.json",
        &[("tgt::t0", CLAIM_B, &["src/a.rs"], &[])],
    );
    write_derives(root, &[CLAIM_A, CLAIM_B]);

    let mut daemon = DaemonGuard::spawn(root);
    write_and_settle(root, "src/a.rs", "fn a() {}");
    daemon.kill();

    let got: Vec<String> = stales(root).into_iter().map(|(c, _)| c).collect();
    assert_eq!(
        got,
        vec![CLAIM_A.to_string()],
        "the daemon must read the configured map (CLAIM_A), never the decoy at \
         the default path (CLAIM_B)"
    );
}
