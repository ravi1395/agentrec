//! `agentrec stats` (Phase 3.0 T2) — CLI adapter over `RepositoryView::stats`
//! (T1, `agentrec-core/src/stats.rs`). Every number asserted below is
//! HAND-COMPUTED in the comments beside it, against the fixture built by
//! [`build_fixture`] — never re-derived by calling the same fold twice (the
//! plan's self-checking-fixture rule, defending the repo's recurring
//! signature-defect class: a confidently-worded comment asserting a fact
//! nobody measured).
//!
//! `StatsResult.window.until` is `SystemTime::now()` read inside the fold —
//! genuinely non-deterministic across two invocations, even two calls a few
//! milliseconds apart in the same test. The golden below normalizes exactly
//! that one token (and `since` when it is a resolved timestamp rather than
//! the literal `"all"`) to a fixed placeholder before comparing bytes; every
//! other line is a real byte pin. This is a local, narrowly-scoped
//! normalization — it does not touch `golden.rs`'s shared harness or its
//! deliberately-empty `NORMALIZE_TABLE`.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures; no attacker-supplied FIFO to
//    block on, so the fsguard wrappers buy nothing here (same rationale as
//    golden.rs's identical allow).

use agentrec_core::record::{append_log, FileEntry, LogRecord, TurnRecord};
use agentrec_core::store::BlobStore;
use std::path::Path;
use std::process::{Command, Output};

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

fn git_init(root: &Path) {
    let status = Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(root)
        .status()
        .expect("spawn git init");
    assert!(status.success(), "git init failed: {status:?}");
}

fn init(root: &Path) {
    git_init(root);
    let out = Command::new(bin())
        .args(["init", "--no-hook", "--no-service", "--root"])
        .arg(root)
        .output()
        .expect("agentrec init");
    assert!(out.status.success(), "init failed: {out:?}");
}

fn fe(path: &str, before: Option<String>, after: Option<String>, op: &str) -> FileEntry {
    FileEntry {
        path: path.to_string(),
        before,
        after,
        op: op.to_string(),
        skipped: false,
        withheld: false,
        baseline_unknown: false,
        skipped_reason: None,
        after_synthesized: None,
        link_kind: None,
        attribution: None,
    }
}

fn seed_turn(root: &Path, turn: TurnRecord) {
    append_log(&root.join(".agentrec/log.jsonl"), &LogRecord::Turn(turn)).expect("seed turn");
}

const T1_ID: &str = "t_STATST1000000000000000TUR1";
const T1_STARTED: &str = "2020-06-01T00:00:00.000Z";
const T1_ENDED: &str = "2020-06-01T00:00:05.000Z";
const A_TXT_CONTENT: &[u8] = b"hello\n"; // 6 bytes

/// One rich `claude`/`m1` turn touching three files, dated 2020 (far outside
/// any real-time censoring window, and far outside the default 30-day
/// `--since`):
///   - `a.txt`: create, no `before`, `after` = hash(A_TXT_CONTENT). The real
///     content is ALSO written to the worktree below matching that hash, so
///     it never diverges (no `excluded_unknown_mtime`, no `share.human`).
///   - `b.bin`: delete, `skipped: true` (over-cap), `after: None`.
///   - `c.env`: delete, `withheld: true` (secret pattern), `after: None`.
///
/// `b.bin`/`c.env` use `op: "delete"` deliberately — `is_write_op` in
/// `stats.rs` only admits `create`/`modify`, so they are excluded from the
/// rework fold entirely (kept out of `measurable` by construction) while
/// still counting in the churn footnote and in `share.agent` (which counts
/// ALL of a turn's entries, not just writes).
fn build_fixture(root: &Path) {
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));
    let a_hash = store.put(A_TXT_CONTENT).unwrap();
    std::fs::write(root.join("a.txt"), A_TXT_CONTENT).unwrap();

    let turn = TurnRecord {
        v: 1,
        id: T1_ID.to_string(),
        grade: "rich".to_string(),
        truncated: false,
        started: T1_STARTED.to_string(),
        ended: T1_ENDED.to_string(),
        tool: Some("claude".to_string()),
        model: Some("m1".to_string()),
        session: None,
        root: root.to_string_lossy().to_string(),
        prompt_ref: None,
        prompt_excerpt: None,
        merges: vec![],
        imported: None,
        files_complete: None,
        origin: None,
        files: vec![
            fe("a.txt", None, Some(a_hash), "create"),
            FileEntry {
                skipped: true,
                ..fe("b.bin", Some("sha256:deadbeef".to_string()), None, "delete")
            },
            FileEntry {
                withheld: true,
                ..fe("c.env", Some("sha256:cafef00d".to_string()), None, "delete")
            },
        ],
    };
    seed_turn(root, turn);
}

// ---------------------------------------------------------------------------
// Normalization for the one non-deterministic pair (`since`/`until` when
// resolved to a real timestamp) in `stats`'s window line. See module doc.
// ---------------------------------------------------------------------------

fn normalize_window_line(s: &str) -> String {
    let mut out = String::new();
    for line in s.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if let Some(rest) = trimmed.strip_prefix("stats window: since ") {
            let (since_tok, rest) = rest.split_once(" until ").expect("well-formed window line");
            let (_until_tok, tail) = rest
                .split_once(" \u{b7} rework-window ")
                .expect("well-formed window line");
            let since_norm = if since_tok == "all" { "all" } else { "<TS>" };
            out.push_str(&format!(
                "stats window: since {since_norm} until <TS> \u{b7} rework-window {tail}\n"
            ));
        } else {
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out
}

/// Parses `YYYY-MM-DDTHH:MM:SS.fffZ` to unix ms. Only needed here to bound
/// `window.until` in the parity test below; deliberately a local, minimal
/// copy rather than a new public export off `agentrec-core::stats` for one
/// test's sake.
fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut dp = date.splitn(3, '-');
    let y: i64 = dp.next()?.parse().ok()?;
    let mo: u32 = dp.next()?.parse().ok()?;
    let d: u32 = dp.next()?.parse().ok()?;
    let (hms, frac) = match time.split_once('.') {
        Some((h, f)) => (h, Some(f)),
        None => (time, None),
    };
    let mut tp = hms.splitn(3, ':');
    let h: i64 = tp.next()?.parse().ok()?;
    let mi: i64 = tp.next()?.parse().ok()?;
    let se: i64 = tp.next()?.parse().ok()?;
    let millis: u64 = match frac {
        Some(f) => {
            let mut digits: String = f.chars().take(3).collect();
            while digits.len() < 3 {
                digits.push('0');
            }
            digits.parse().ok()?
        }
        None => 0,
    };
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = if mo > 2 { mo - 3 } else { mo + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3_600 + mi * 60 + se;
    if secs < 0 {
        return None;
    }
    Some(secs as u64 * 1000 + millis)
}

fn golden_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stats")
}

fn assert_golden(name: &str, out: &Output) {
    let stdout = normalize_window_line(&String::from_utf8_lossy(&out.stdout));
    let captured = format!(
        "stdout:\n{stdout}\n--stderr--\n{}\n--exit--\n{}\n",
        String::from_utf8_lossy(&out.stderr),
        out.status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string()),
    );
    let path = golden_dir().join(format!("{name}.golden"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        std::fs::write(&path, &captured).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("missing golden {path:?}: {e}. Run with UPDATE_GOLDEN=1 to create it.\ncaptured:\n{captured}")
    });
    assert_eq!(expected, captured, "golden mismatch for {name} ({path:?})");
}

// ---------------------------------------------------------------------------
// --since / bad duration / clap-level error
// ---------------------------------------------------------------------------

#[test]
fn since_all_covers_full_history() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    let out = agentrec(root, &["stats", "--since", "all", "--json"]);
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["window"]["since"].is_null());
    assert_eq!(v["turns"]["total"], 1);
}

#[test]
fn default_since_excludes_the_2020_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    let out = agentrec(root, &["stats", "--json"]);
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["window"]["since"].is_string());
    assert_eq!(
        v["turns"]["total"], 0,
        "2020 turn must be outside default 30d window"
    );
}

#[test]
fn bad_duration_is_a_clap_level_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let out = agentrec(root, &["stats", "--since", "bogus"]);
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2), "clap usage-error exit code");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("invalid duration"),
        "{out:?}"
    );
}

/// B2 (gate round 2): a multibyte trailing char in `--since` must be a
/// clap-level error, not a panic. `SinceArg::from_str` used to split the
/// unit off with a byte-index slice (`s.len() - 1`), which panics on any
/// non-ASCII final char instead of producing a well-formed `Err`.
#[test]
fn multibyte_bad_duration_is_a_clap_level_error_not_a_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let out = agentrec(root, &["stats", "--since", "3\u{b5}"]); // "3µ"
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected clap usage-error exit code, not a panic: {out:?}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("invalid duration"),
        "{out:?}"
    );
}

#[test]
fn rework_window_flag_maps_to_stats_options() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    let out = agentrec(
        root,
        &["stats", "--since", "all", "--rework-window", "3", "--json"],
    );
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["window"]["rework_window_days"], 3);
}

// ---------------------------------------------------------------------------
// Honesty rendering (spec §3.0.1): every figure beside its exclusion counts
// ---------------------------------------------------------------------------

#[test]
fn text_output_carries_every_disclosure_field_including_zeros() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    let out = agentrec(root, &["stats", "--since", "all"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    // ReworkRate: all nine fields present, including the ones that are 0 on
    // this fixture (unparsed_ended, undo_unevaluable_c, gap_overlapped).
    for needle in [
        "measurable=1",
        "reworked=0",
        "censored_recent=0",
        "excluded_imported=0",
        "excluded_unknown_mtime=0",
        "gap_overlapped=0",
        "undo_unevaluable_c=0",
        "unparsed_ended=0",
        "approx_lower",
        // B1 (gate round 2): the OUTPUT, not just the machine `approx_lower`
        // token, must say the rate is an approximate lower bound and name
        // both disclosed bias channels.
        "approximate lower bound",
        "undercount",
        "overcount",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
}

#[test]
fn zero_measurable_renders_the_literal_no_measurable_events() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root); // no turns at all
    let out = agentrec(root, &["stats"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("no measurable events"),
        "expected the literal phrase: {text}"
    );
}

#[test]
fn change_share_never_renders_a_percentage() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    let out = agentrec(root, &["stats", "--since", "all"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains('%'),
        "share must never render as a percentage:\n{text}"
    );
    // Hand-computed: t.files.len() == 3 (a.txt/b.bin/c.env), turn is rich,
    // non-git, non-imported -> all three land in `agent`.
    assert!(text.contains("agent=3"), "{text}");
    assert!(text.contains("human=0"), "{text}");
}

#[test]
fn file_churn_footnotes_skipped_and_withheld_only_when_nonzero() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    let out = agentrec(root, &["stats", "--since", "all"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    // a.txt: churn_bytes = |6 - 0| = 6 (before absent = 0, after = 6 bytes).
    assert!(
        text.contains("a.txt  churn_bytes=6 dangling_refs=0"),
        "{text}"
    );
    // b.bin / c.env: 0 churn bytes, each its own undercount footnote.
    assert!(
        text.contains("b.bin  churn_bytes=0 dangling_refs=0"),
        "{text}"
    );
    assert!(
        text.contains("undercounted: 1 skipped, 0 withheld"),
        "expected b.bin's skipped footnote:\n{text}"
    );
    assert!(
        text.contains("c.env  churn_bytes=0 dangling_refs=0"),
        "{text}"
    );
    assert!(
        text.contains("undercounted: 0 skipped, 1 withheld"),
        "expected c.env's withheld footnote:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// --json parity (AC-1 pattern) — with the wall-clock caveat stated up top.
// ---------------------------------------------------------------------------

/// `--json`'s top-level key set is EXACTLY `StatsResult`'s declared fields
/// (proof by key set, same style as `json_contracts::diff_json_key_set_...`
/// in `cli/tests/integration.rs`), and every field's VALUE matches an
/// independently-computed `StatsResult` for the same fixture and options —
/// except `window.until`/`window.since`, which are wall-clock reads that
/// cannot be pinned byte-for-byte across two process invocations a few
/// milliseconds apart. Those two are checked for a bounded skew instead.
#[test]
fn json_matches_stats_result_field_for_field_except_the_clock() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);

    let before_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let out = agentrec(root, &["stats", "--since", "all", "--json"]);
    assert!(out.status.success(), "{out:?}");
    let after_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();

    let cli_json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let obj = cli_json.as_object().expect("top-level object");
    assert_eq!(
        obj.keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        ["window", "turns", "files", "share", "rework"]
            .into_iter()
            .map(String::from)
            .collect(),
        "StatsResult's declared field set"
    );

    let view = agentrec_core::view::RepositoryView::open(root).unwrap();
    let opts = agentrec_core::stats::StatsOptions {
        since: None,
        rework_window_days: agentrec_core::stats::DEFAULT_REWORK_WINDOW_DAYS,
    };
    let direct = view.stats(&opts).unwrap();
    let direct_json = serde_json::to_value(&direct).unwrap();

    for field in ["turns", "files", "share", "rework"] {
        assert_eq!(
            cli_json[field], direct_json[field],
            "field {field} diverged"
        );
    }
    assert_eq!(cli_json["window"]["since"], direct_json["window"]["since"]);
    assert_eq!(
        cli_json["window"]["rework_window_days"],
        direct_json["window"]["rework_window_days"]
    );
    // The clock field: both reads must fall inside the wall-clock window this
    // test bracketed the subprocess call with — a real bound, not a literal.
    let until_str = cli_json["window"]["until"].as_str().unwrap();
    let until_ms =
        parse_rfc3339_ms(until_str).unwrap_or_else(|| panic!("unparseable until: {until_str}"));
    assert!(
        until_ms as u128 >= before_ms && until_ms as u128 <= after_ms,
        "until={until_ms} not within [{before_ms}, {after_ms}]"
    );
}

// ---------------------------------------------------------------------------
// Zero-write invariant (mirrors integration.rs's status zero-write pattern)
// ---------------------------------------------------------------------------

fn total_store_bytes(root: &Path) -> u64 {
    fn walk(dir: &Path, total: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, total);
            } else if let Ok(meta) = entry.metadata() {
                *total += meta.len();
            }
        }
    }
    let mut total = 0u64;
    walk(&root.join(".agentrec/objects"), &mut total);
    total
}

fn log_bytes(root: &Path) -> u64 {
    std::fs::metadata(root.join(".agentrec/log.jsonl"))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// A3 (gate round 2): every path under `.agentrec/`, relative, mapped to its
/// byte length. Enumerating the whole tree (rather than checking only the
/// specific files/dirs the fixture already knows about) catches a NEW stray
/// file or directory `stats` might create — a mutation the earlier,
/// named-path-only version of this test could not see.
fn snapshot_agentrec_tree(root: &Path) -> std::collections::BTreeMap<String, u64> {
    fn walk(base: &Path, dir: &Path, out: &mut std::collections::BTreeMap<String, u64>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if let Ok(meta) = entry.metadata() {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, meta.len());
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(&root.join(".agentrec"), &root.join(".agentrec"), &mut out);
    out
}

#[test]
fn stats_and_stats_json_write_nothing_under_agentrec_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);

    let state_path = root.join(".agentrec/state.json");
    std::fs::write(&state_path, "{}").unwrap();
    let state_before = std::fs::read_to_string(&state_path).unwrap();
    let store_before = total_store_bytes(root);
    let log_before = log_bytes(root);
    let tree_before = snapshot_agentrec_tree(root);

    for args in [
        vec!["stats", "--since", "all"],
        vec!["stats", "--since", "all", "--json"],
    ] {
        let out = agentrec(root, &args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert_eq!(
            std::fs::read_to_string(&state_path).unwrap(),
            state_before,
            "{args:?} touched state.json"
        );
        assert_eq!(
            total_store_bytes(root),
            store_before,
            "{args:?} wrote to objects/"
        );
        assert_eq!(
            log_bytes(root),
            log_before,
            "{args:?} appended to log.jsonl"
        );
        assert_eq!(
            snapshot_agentrec_tree(root),
            tree_before,
            "{args:?} changed the .agentrec/ tree (new/removed/resized entry)"
        );
    }
}

// ---------------------------------------------------------------------------
// Text golden on the hand-computed fixture. `--json` goldens (none added
// here — the parity test above is this task's json instrument) are additive
// only per the plan's global constraint: a future field on any of
// `StatsResult`/`TurnCounts`/`FileChurn`/`ChangeShare`/`ReworkRate` may
// widen this text output, but no existing line here may be removed or
// reworded without a reviewed, intentional `UPDATE_GOLDEN=1` regeneration.
// ---------------------------------------------------------------------------

#[test]
fn golden_stats_since_all() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden(
        "stats_since_all",
        &agentrec(root, &["stats", "--since", "all"]),
    );
}

#[test]
fn golden_stats_zero_measurable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    assert_golden("stats_zero_measurable", &agentrec(root, &["stats"]));
}
