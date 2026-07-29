//! P3: golden byte-pinning harness. Captures today's exact `agentrec` CLI
//! bytes (stdout + stderr + exit code) against a deterministic, hand-authored
//! fixture repo, so P4's claimed byte-equivalence during the read-verb
//! extraction can actually be falsified instead of merely asserted.
//!
//! # Governing lesson (read before touching this file)
//!
//! P1 broke three times because a synthetic fixture and the implementation
//! agreed on a shape the real corpus didn't have. Every shape asserted here
//! is derived from reading the renderer that produces it
//! (`cli/src/fmt.rs`, `cli/src/cmds.rs`, `cli/src/readcmds.rs`,
//! `agentrec-core/src/record.rs`) — never invented from what a brief implies.
//!
//! # Why every byte here is deterministic
//!
//! Every command this harness drives is read-only over a fixture this file
//! builds by hand (real `git init` + hand-seeded `.agentrec/log.jsonl` +
//! real worktree files + real content-addressed blobs). No daemon ever runs
//! (AC5), so nothing here depends on FSEvents/inotify timing. The one
//! wall-clock-sensitive renderer — `fmt::relative_time` — is neutralized by
//! construction: every fixture turn's `started`/`ended` is dated in 2020,
//! far past the 7-day cutoff in `relative_time` (see `cli/src/fmt.rs`), so
//! it always takes the branch that prints the bare `YYYY-MM-DD` date
//! *parsed out of the turn's own `started` string* — never a value computed
//! from `now`. That branch is exercised here; the "just now"/"Nm ago"/
//! "yesterday" buckets are NOT (they're already pinned by
//! `fmt.rs::relative_time_golden_buckets` with an injected clock — this
//! harness doesn't re-cover them, and that's a deliberate coverage boundary,
//! not an oversight).
//!
//! # Normalization (ULIDs / timestamps / hashes) — required by the AC, and
//! why it is (mostly) a no-op here
//!
//! The AC requires ids/timestamps to be *substituted* by a documented stable
//! mapping, never deleted, so a golden can't silently swallow a wrong-but-
//! well-formed value. The mapping this harness uses is the fixture's own
//! `const` id/content literals below (`RICH_TURN_ID`, `BLOB_APP_BEFORE`,
//! etc.) — every id, timestamp, and content hash that reaches stdout
//! originates from exactly one of these named constants, never from
//! `agentrec_core::id::turn_id()` or `SystemTime::now()`. That IS the
//! "documented stable mapping": a Rust identifier bound once to a literal
//! wire value, used to build the fixture AND to grep-verify the captured
//! golden. [`normalize`] is the substitution mechanism the AC asks for; its
//! table is intentionally empty today because nothing dynamically-generated
//! ever reaches a captured golden in this harness. A pattern-based
//! normalizer (e.g. "redact anything ULID-shaped") is deliberately NOT used:
//! it would silently swallow a wrong-but-well-formed id introduced by a
//! future regression, defeating the whole point of a byte pin. If a future
//! golden needs to capture real system-generated content, add a literal
//! entry to [`normalize`]'s table — never delete the field from the golden.
//!
//! # Load-bearing edge case
//!
//! [`build_fixture`] always runs a real `git init` first. A fixture built in
//! a non-git tempdir silently disables ignore-rule application in every
//! gitignore-aware walker in this codebase (see `daemon.rs:2295`) — even
//! though none of the commands this harness drives (`log`/`diff`/`blame`/
//! `show`/`status`) walk the tree themselves, `agentrec init` behaves
//! differently outside a git repo, so this harness matches the same `init()`
//! shape `integration.rs` uses for every other end-to-end test.

use std::path::Path;
use std::process::{Command, Output};

use agentrec_core::record::{
    append_line_synced, append_log, EpochRecord, FileEntry, LogRecord, TurnRecord,
};
use agentrec_core::store::BlobStore;

// ---------------------------------------------------------------------------
// Fixture identity constants — the "documented stable mapping" (see module
// doc). Every one of these, and only these, may appear as an id/hash/
// timestamp in a captured golden.
// ---------------------------------------------------------------------------

const RICH_TURN_ID: &str = "t_RICH000000000000000000TUR1";
const BARE_TURN_ID: &str = "t_BARE000000000000000000TUR2";
const GIT_TURN_ID: &str = "t_GITT000000000000000000TUR3";
const UNDO_TURN_ID: &str = "t_UNDO000000000000000000TUR4";
const IMPORTED_TURN_ID: &str = "t_IMPT000000000000000000TUR7";
const DUP_TURN_ID: &str = "t_DUPA000000000000000000TUR8";

const EPOCH_GAP_START1: &str = "2020-01-01T00:00:00.000Z";
const EPOCH_GAP_START2: &str = "2020-01-01T00:01:00.000Z"; // no stop between -> 1 gap
const EPOCH_GAP_STOP: &str = "2020-01-01T00:02:00.000Z";

const RICH_STARTED: &str = "2020-01-02T10:00:00.000Z";
const RICH_ENDED: &str = "2020-01-02T10:00:05.000Z";
const BARE_STARTED: &str = "2020-01-02T11:00:00.000Z";
const BARE_ENDED: &str = "2020-01-02T11:00:03.000Z";
const GIT_STARTED: &str = "2020-01-02T12:00:00.000Z";
const GIT_ENDED: &str = "2020-01-02T12:00:01.000Z";
const UNDO_STARTED: &str = "2020-01-02T13:00:00.000Z";
const UNDO_ENDED: &str = "2020-01-02T13:00:01.000Z";
const IMPORTED_STARTED: &str = "2020-01-01T08:00:00.000Z"; // predates recording (backfill)
const IMPORTED_ENDED: &str = "2020-01-01T08:00:01.000Z";
const DUP_STARTED: &str = "2020-01-02T14:00:00.000Z";
const DUP_ENDED_A: &str = "2020-01-02T14:00:01.000Z";
const DUP_ENDED_B: &str = "2020-01-02T14:00:02.000Z"; // recovery-recomputed `ended` drift

// app.rs content: line2 changes, used for both diff-hunk and blame-line
// coverage (added_or_changed_lines(before, after) == ["LINE2"]).
const APP_BEFORE: &[u8] = b"line1\nline2\nline3\n";
const APP_AFTER: &[u8] = b"line1\nLINE2\nline3\n";
const NEW_FILE_CONTENT: &[u8] = b"brand new content\n";
const HUMAN_BEFORE: &[u8] = b"orig content\n";
const HUMAN_AFTER: &[u8] = b"clean content\n";
const HUMAN_DISK: &[u8] = b"hand edited content\n"; // diverges from HUMAN_AFTER on purpose
const BARE_BEFORE: &[u8] = b"bare before\n";
const BARE_AFTER: &[u8] = b"bare after\n";
const GIT_BEFORE: &[u8] = b"* text=auto\n";
const GIT_AFTER: &[u8] = b"* text=auto eol=lf\n";
const DUP_BEFORE: &[u8] = b"dup before\n";
const DUP_AFTER: &[u8] = b"dup after\n";
const IMPORTED_CONTENT: &[u8] = b"imported by backfill\n";
const PROMPT_TEXT: &[u8] =
    b"Please add rate limiting to the API and handle burst traffic gracefully.";

// ---------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

/// Run the real `agentrec` binary; never spawns `record` (AC5 — this whole
/// harness runs with the daemon stopped and starts none itself).
fn agentrec(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .args(["--root", root.to_str().unwrap()])
        .output()
        .expect("run agentrec")
}

fn git_init(root: &Path) {
    Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(root)
        .status()
        .expect("git init");
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

// ---------------------------------------------------------------------------
// Fixture construction
// ---------------------------------------------------------------------------

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
    }
}

fn write_file(root: &Path, rel: &str, content: &[u8]) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, content).unwrap();
}

fn seed_turn(root: &Path, turn: &TurnRecord) {
    append_log(
        &root.join(".agentrec/log.jsonl"),
        &LogRecord::Turn(turn.clone()),
    )
    .expect("seed turn");
}

fn seed_epoch(root: &Path, event: &str, ts: &str) {
    append_log(
        &root.join(".agentrec/log.jsonl"),
        &LogRecord::Epoch(EpochRecord {
            v: 1,
            event: event.to_string(),
            ts: ts.to_string(),
        }),
    )
    .expect("seed epoch");
}

/// Builds a complete, deterministic fixture repo: real `git init`, real
/// worktree files, a hand-seeded `.agentrec/log.jsonl` covering every shape
/// AC1 requires, and real content-addressed blobs backing every hash a
/// renderer might resolve. Call once per test — [`three_runs_are_byte_identical`]
/// (AC3) calls it three times, each into a fresh tempdir, specifically to
/// catch an absolute-tempdir-path leak that a single shared fixture across
/// three runs could never detect.
fn build_fixture(root: &Path) {
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));

    // ---- blobs -------------------------------------------------------
    let app_before = store.put(APP_BEFORE).unwrap();
    let app_after = store.put(APP_AFTER).unwrap();
    let new_after = store.put(NEW_FILE_CONTENT).unwrap();
    let human_before = store.put(HUMAN_BEFORE).unwrap();
    let human_after = store.put(HUMAN_AFTER).unwrap();
    let bare_before = store.put(BARE_BEFORE).unwrap();
    let bare_after = store.put(BARE_AFTER).unwrap();
    let git_before = store.put(GIT_BEFORE).unwrap();
    let git_after = store.put(GIT_AFTER).unwrap();
    let dup_before = store.put(DUP_BEFORE).unwrap();
    let dup_after = store.put(DUP_AFTER).unwrap();
    let imported_after = store.put(IMPORTED_CONTENT).unwrap();
    let prompt_ref = store.put(PROMPT_TEXT).unwrap();

    // ---- real worktree files (for blame's on-disk hash comparison) ---
    write_file(root, "src/app.rs", APP_AFTER); // matches recorded `after` -> clean
    write_file(root, "src/new.rs", NEW_FILE_CONTENT);
    write_file(root, "src/human.rs", HUMAN_DISK); // diverges -> human-edited since
    write_file(root, "src/bare.rs", BARE_AFTER);
    write_file(root, "src/dup.rs", DUP_AFTER);
    write_file(root, "src/imported.rs", IMPORTED_CONTENT);
    // src/big.bin and .env are `skipped`/`withheld` — never snapshotted, and
    // deliberately not written to the worktree either (this fixture only
    // needs their FileEntry shape for `diff`'s skip/withhold branches, which
    // never touch the worktree at all).
    // src/untouched.rs intentionally does not exist and is never referenced
    // by any turn — the `blame` "no recorded turn touches" / gap-poisoned
    // fallback case.

    // ---- epoch gap (before every turn's timestamp, so it never poisons
    // any *touched*-file `blame` via `has_gap_after`; it still poisons the
    // untouched-file case via the purely positional `has_recording_gap`) --
    seed_epoch(root, "start", EPOCH_GAP_START1);
    seed_epoch(root, "start", EPOCH_GAP_START2); // unmatched -> 1 gap
    seed_epoch(root, "stop", EPOCH_GAP_STOP);

    // ---- rich turn: modify + create + skipped + withheld + human-edited-
    // divergent modify, all in one turn so `diff`/`show` exercise every
    // `print_entry` branch in a single captured invocation. Also carries a
    // real `prompt_ref` so `show --prompt` has something to resolve.
    let rich = TurnRecord {
        v: 1,
        id: RICH_TURN_ID.to_string(),
        grade: "rich".to_string(),
        truncated: false,
        started: RICH_STARTED.to_string(),
        ended: RICH_ENDED.to_string(),
        tool: Some("claude".to_string()),
        model: Some("claude-opus".to_string()),
        session: Some("sess-rich-1".to_string()),
        root: "/repo".to_string(),
        prompt_ref: Some(prompt_ref),
        prompt_excerpt: Some("add rate limiting".to_string()),
        merges: vec![],
        files: vec![
            fe(
                "src/app.rs",
                Some(app_before.clone()),
                Some(app_after.clone()),
                "modify",
            ),
            fe("src/new.rs", None, Some(new_after), "create"),
            FileEntry {
                skipped: true,
                skipped_reason: Some(agentrec_core::record::skip_reason::OVER_CAP.to_string()),
                ..fe("src/big.bin", None, None, "modify")
            },
            FileEntry {
                withheld: true,
                ..fe(".env", None, None, "modify")
            },
            fe(
                "src/human.rs",
                Some(human_before.clone()),
                Some(human_after),
                "modify",
            ),
        ],
    };
    seed_turn(root, &rich);

    // ---- bare turn: unattributed activity window, never fabricates a
    // tool/prompt.
    let bare = TurnRecord {
        v: 1,
        id: BARE_TURN_ID.to_string(),
        grade: "bare".to_string(),
        truncated: false,
        started: BARE_STARTED.to_string(),
        ended: BARE_ENDED.to_string(),
        tool: None,
        model: None,
        session: None,
        root: "/repo".to_string(),
        prompt_ref: None,
        prompt_excerpt: None,
        merges: vec![],
        files: vec![fe(
            "src/bare.rs",
            Some(bare_before),
            Some(bare_after),
            "modify",
        )],
    };
    seed_turn(root, &bare);

    // ---- git turn: hidden from `log` unless --all.
    let git_turn = TurnRecord {
        v: 1,
        id: GIT_TURN_ID.to_string(),
        grade: "rich".to_string(),
        truncated: false,
        started: GIT_STARTED.to_string(),
        ended: GIT_ENDED.to_string(),
        tool: Some("git".to_string()),
        model: None,
        session: None,
        root: "/repo".to_string(),
        prompt_ref: None,
        prompt_excerpt: None,
        merges: vec![],
        files: vec![fe(
            ".gitattributes",
            Some(git_before),
            Some(git_after),
            "modify",
        )],
    };
    seed_turn(root, &git_turn);

    // ---- undo turn: hand-authored (never exercised via a real
    // `undo --confirm`, deliberately — a real run would embed this
    // tempdir's absolute path in `TurnRecord.root`, see `readcmds::undo`,
    // breaking determinism). Mirrors the exact shape `readcmds::undo`
    // constructs: `tool: Some("agentrec")`, `prompt_excerpt: Some(format!(
    // "undo of {short_id}"))`.
    let undo = TurnRecord {
        v: 1,
        id: UNDO_TURN_ID.to_string(),
        grade: "rich".to_string(),
        truncated: false,
        started: UNDO_STARTED.to_string(),
        ended: UNDO_ENDED.to_string(),
        tool: Some("agentrec".to_string()),
        model: None,
        session: None,
        root: "/repo".to_string(),
        prompt_ref: None,
        prompt_excerpt: Some(format!("undo of {}", short_id_of(RICH_TURN_ID))),
        merges: vec![],
        files: vec![fe(
            "src/app.rs",
            Some(app_after),
            Some(app_before),
            "modify",
        )],
    };
    seed_turn(root, &undo);

    // ---- imported turn: seeded as a RAW JSON line (P2's `imported`/
    // `files_complete` fields don't exist on this branch's `TurnRecord` yet
    // — see the frozen interface contract in the task brief). Field order
    // matches P2's frozen struct declaration order exactly:
    // v, id, grade, truncated?, started, ended, tool, model, session, root,
    // prompt_ref?, prompt_excerpt?, merges?, imported, files_complete, files.
    // Until P2 lands, `load_log` parses this via serde's tolerant-unknown-
    // field default and silently drops `imported`/`files_complete` — this
    // turn renders today as an ordinary bare turn with no visible marker.
    // The day P2 merges those fields onto `TurnRecord`, `log --json`'s
    // re-serialization of this exact line will start including
    // `"imported":true,"files_complete":false` — the ONE golden this
    // legitimately changes at that merge, not a regression.
    let imported_line = format!(
        concat!(
            r#"{{"type":"turn","v":1,"id":"{id}","grade":"bare","#,
            r#""started":"{started}","ended":"{ended}","tool":"claude-code","#,
            r#""root":"/repo","imported":true,"files_complete":false,"#,
            r#""files":[{{"path":"src/imported.rs","before":null,"after":"{after}","#,
            r#""op":"create"}}]}}"#
        ),
        id = IMPORTED_TURN_ID,
        started = IMPORTED_STARTED,
        ended = IMPORTED_ENDED,
        after = imported_after,
    );
    append_line_synced(&root.join(".agentrec/log.jsonl"), &imported_line).expect("seed imported");

    // ---- duplicate-id pair: PR #2's orphan-recovery double-emit shape —
    // same id, identical `files` set, but `ended`/`truncated` legitimately
    // drift between the steady-close and recovery-recomputed records
    // (`readcmds::same_revert`'s tolerated drift). `resolve_turn` collapses
    // these to one when queried by id.
    let dup_entry = fe("src/dup.rs", Some(dup_before), Some(dup_after), "modify");
    let dup_a = TurnRecord {
        v: 1,
        id: DUP_TURN_ID.to_string(),
        grade: "rich".to_string(),
        truncated: false,
        started: DUP_STARTED.to_string(),
        ended: DUP_ENDED_A.to_string(),
        tool: Some("claude".to_string()),
        model: Some("claude-opus".to_string()),
        session: None,
        root: "/repo".to_string(),
        prompt_ref: None,
        prompt_excerpt: Some("dup pass".to_string()),
        merges: vec![],
        files: vec![dup_entry.clone()],
    };
    let dup_b = TurnRecord {
        ended: DUP_ENDED_B.to_string(),
        truncated: true, // recovery forces truncated: true (see same_revert's doc)
        model: None,     // recovery forces model: None
        ..dup_a.clone()
    };
    seed_turn(root, &dup_a);
    seed_turn(root, &dup_b);
}

/// `t_<ULID>` -> `t_<first4>…<last4>`, mirroring `cli/src/fmt.rs::short_id`
/// exactly (that function is private to the `cli` crate's binary target, not
/// reachable from an integration test, so this is a deliberate, documented
/// duplication of one four-line pure function rather than a dependency
/// restructure — the golden capture itself is still the actual proof; this
/// helper only builds the fixture's own `prompt_excerpt` text).
fn short_id_of(id: &str) -> String {
    let body = id.strip_prefix("t_").unwrap_or(id);
    if body.len() <= 8 {
        return id.to_string();
    }
    format!("t_{}…{}", &body[..4], &body[body.len() - 4..])
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

/// Substitution table for [`normalize`]. Empty by design — see the module
/// doc's "Normalization" section for why. Add `(literal, placeholder)`
/// pairs here if a future golden captures dynamically-generated content;
/// never remove a field from the captured text to achieve determinism.
const NORMALIZE_TABLE: &[(&str, &str)] = &[];

/// Applies [`NORMALIZE_TABLE`] to captured output before comparison/storage.
fn normalize(mut s: String) -> String {
    for (literal, placeholder) in NORMALIZE_TABLE {
        s = s.replace(literal, placeholder);
    }
    s
}

// ---------------------------------------------------------------------------
// Golden capture/compare
// ---------------------------------------------------------------------------

fn golden_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden")
}

/// Captures `stdout\0stderr\0exit=<code>\n` (NUL-separated so a golden diff
/// distinguishes an empty-stderr-different-stdout case from the reverse) for
/// `out`, normalizes it, and compares against (or — under `UPDATE_GOLDEN=1`
/// — writes) `tests/fixtures/golden/<name>.golden`.
fn assert_golden(name: &str, out: &Output) {
    let captured = format!(
        "stdout:\n{}\n--stderr--\n{}\n--exit--\n{}\n",
        normalize(String::from_utf8_lossy(&out.stdout).into_owned()),
        normalize(String::from_utf8_lossy(&out.stderr).into_owned()),
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
        panic!(
            "missing golden {path:?}: {e}. Run with UPDATE_GOLDEN=1 to create it.\n\
             captured was:\n{captured}"
        )
    });
    if expected != captured {
        // Byte-level diff: first differing byte offset + surrounding context,
        // so a mutation's effect is visible without diffing two multi-KB
        // blobs by eye (AC4's RED proof needs this).
        let diff = first_diff(&expected, &captured);
        panic!(
            "golden mismatch for {name} ({path:?})\n{diff}\n\
             --- expected ---\n{expected}\n--- actual ---\n{captured}\n\
             (re-run with UPDATE_GOLDEN=1 only if this change is intentional \
             and reviewed — goldens are the byte-equivalence instrument for \
             the whole plan; never regenerate to silence a real regression)"
        );
    }
}

fn first_diff(expected: &str, actual: &str) -> String {
    let e = expected.as_bytes();
    let a = actual.as_bytes();
    let n = e.len().min(a.len());
    for i in 0..n {
        if e[i] != a[i] {
            let start = i.saturating_sub(20);
            let e_ctx = String::from_utf8_lossy(&e[start..(i + 20).min(e.len())]);
            let a_ctx = String::from_utf8_lossy(&a[start..(i + 20).min(a.len())]);
            return format!(
                "first differing byte at offset {i}:\n  expected…: {e_ctx:?}\n  actual…..: {a_ctx:?}"
            );
        }
    }
    format!(
        "one side is a prefix of the other: expected len={}, actual len={}",
        e.len(),
        a.len()
    )
}

// ---------------------------------------------------------------------------
// Tests: fixture shape (AC1)
// ---------------------------------------------------------------------------

#[test]
fn fixture_contains_every_required_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);

    let text = std::fs::read_to_string(root.join(".agentrec/log.jsonl")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let values: Vec<serde_json::Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let turns: Vec<&serde_json::Value> = values
        .iter()
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("turn"))
        .collect();

    assert!(
        turns
            .iter()
            .any(|t| t.get("grade") == Some(&serde_json::json!("rich"))
                && t.get("tool") == Some(&serde_json::json!("claude"))),
        "no rich turn"
    );
    assert!(
        turns
            .iter()
            .any(|t| t.get("grade") == Some(&serde_json::json!("bare")) && t.get("tool").is_none()),
        "no bare turn"
    );
    assert!(
        turns
            .iter()
            .any(|t| t.get("tool") == Some(&serde_json::json!("git"))),
        "no git turn"
    );
    assert!(
        turns
            .iter()
            .any(|t| t.get("tool") == Some(&serde_json::json!("agentrec"))),
        "no undo turn"
    );
    assert!(
        turns.iter().any(|t| t
            .get("files")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.get("skipped") == Some(&serde_json::json!(true)))),
        "no skipped entry"
    );
    assert!(
        turns.iter().any(|t| t
            .get("files")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.get("withheld") == Some(&serde_json::json!(true)))),
        "no withheld entry"
    );
    assert!(
        text.contains(r#""imported":true"#) && text.contains(r#""files_complete":false"#),
        "no imported turn"
    );
    let dup_count = turns
        .iter()
        .filter(|t| t.get("id") == Some(&serde_json::json!(DUP_TURN_ID)))
        .count();
    assert_eq!(dup_count, 2, "expected exactly one duplicate-id pair");

    let epochs: Vec<&serde_json::Value> = values
        .iter()
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("epoch"))
        .collect();
    assert_eq!(epochs.len(), 3, "expected the seeded start/start/stop gap");
}

// ---------------------------------------------------------------------------
// Tests: goldens per AC2's command list
// ---------------------------------------------------------------------------

#[test]
fn golden_log_default() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden("log_default", &agentrec(root, &["log"]));
}

#[test]
fn golden_log_all() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden("log_all", &agentrec(root, &["log", "--all"]));
}

#[test]
fn golden_log_json() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden("log_json", &agentrec(root, &["log", "--json"]));
}

#[test]
fn golden_log_explain() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden("log_explain", &agentrec(root, &["log", "--explain"]));
}

#[test]
fn golden_status() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden("status", &agentrec(root, &["status"]));
}

#[test]
fn golden_status_json() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden("status_json", &agentrec(root, &["status", "--json"]));
}

#[test]
fn golden_diff_rich_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden("diff_rich_turn", &agentrec(root, &["diff", RICH_TURN_ID]));
}

#[test]
fn golden_diff_unknown_id_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    let out = agentrec(root, &["diff", "t_DOESNOTEXIST0000000000001"]);
    assert!(!out.status.success(), "expected diff to fail: {out:?}");
    assert_golden("diff_unknown_id", &out);
}

#[test]
fn golden_blame_untouched_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    // Poisoned by the fixture's global epoch gap (see build_fixture's doc) —
    // "attribution stale — recording gap", not "no recorded turn touches".
    assert_golden(
        "blame_untouched_file",
        &agentrec(root, &["blame", "src/untouched.rs"]),
    );
}

#[test]
fn golden_blame_touched_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    // src/new.rs, not src/app.rs: the fixture's undo turn ALSO touches
    // src/app.rs (it's a revert of the rich turn's change to it) and is
    // later in append order, so `blame src/app.rs` attributes to the undo
    // turn and — since the worktree still holds the rich turn's `after`,
    // not the undo turn's — legitimately reports "human-edited since" (see
    // `golden_blame_human_edited`'s sibling case, which pins that exact
    // shape deliberately). src/new.rs is touched by exactly one turn (rich,
    // `create`) with disk content matching its recorded `after` byte for
    // byte, so this is the one golden that actually exercises blame_file's
    // no-divergence branch.
    assert_golden(
        "blame_touched_clean",
        &agentrec(root, &["blame", "src/new.rs"]),
    );
}

#[test]
fn golden_blame_human_edited() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden(
        "blame_human_edited",
        &agentrec(root, &["blame", "src/human.rs"]),
    );
}

#[test]
fn golden_blame_line_level() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden(
        "blame_line_level",
        &agentrec(root, &["blame", "src/app.rs:2"]),
    );
}

#[test]
fn golden_blame_line_out_of_range_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    let out = agentrec(root, &["blame", "src/app.rs:999"]);
    assert!(!out.status.success(), "expected blame to fail: {out:?}");
    assert_golden("blame_line_out_of_range", &out);
}

#[test]
fn golden_show_rich_header() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden("show_rich_header", &agentrec(root, &["show", RICH_TURN_ID]));
}

#[test]
fn golden_show_bare_header_no_fabrication() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden("show_bare_header", &agentrec(root, &["show", BARE_TURN_ID]));
}

#[test]
fn golden_show_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden(
        "show_prompt",
        &agentrec(root, &["show", RICH_TURN_ID, "--prompt"]),
    );
}

#[test]
fn golden_show_prompt_none_attached_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    let out = agentrec(root, &["show", BARE_TURN_ID, "--prompt"]);
    assert!(
        !out.status.success(),
        "expected show --prompt to fail: {out:?}"
    );
    assert_golden("show_prompt_none_attached", &out);
}

#[test]
fn golden_show_unknown_id_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    let out = agentrec(root, &["show", "t_DOESNOTEXIST0000000000001"]);
    assert!(!out.status.success(), "expected show to fail: {out:?}");
    assert_golden("show_unknown_id", &out);
}

// ---------------------------------------------------------------------------
// Test: determinism (AC3) — three FRESH fixture builds, each in its own
// tempdir. A single shared fixture reused across three runs would never
// catch an absolute-tempdir-path leak; three independent builds do.
// ---------------------------------------------------------------------------

#[test]
fn three_fresh_builds_are_byte_identical() {
    const COMMANDS: &[&[&str]] = &[
        &["log"],
        &["log", "--all"],
        &["log", "--json"],
        &["log", "--explain"],
        &["status"],
        &["status", "--json"],
    ];
    let mut runs: Vec<Vec<String>> = Vec::new();
    for _ in 0..3 {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        build_fixture(root);
        let mut captured = Vec::new();
        for args in COMMANDS {
            let out = agentrec(root, args);
            captured.push(format!(
                "{:?} => stdout={:?} stderr={:?} code={:?}",
                args,
                normalize(String::from_utf8_lossy(&out.stdout).into_owned()),
                normalize(String::from_utf8_lossy(&out.stderr).into_owned()),
                out.status.code()
            ));
        }
        runs.push(captured);
    }
    assert_eq!(runs[0], runs[1], "run 1 vs run 2 diverged");
    assert_eq!(runs[0], runs[2], "run 1 vs run 3 diverged");
}

// ---------------------------------------------------------------------------
// Test: no daemon spawned (AC5, empirical half). This test proves the
// negative *within* the harness: nothing in this file ever constructs a
// `record` subcommand invocation. The `ps aux` before/after capture that
// proves no LEAKED daemon survives the suite is done externally (reported,
// not assertable from inside the test binary — a test process cannot
// reliably enumerate its siblings' argv across platforms without shelling
// out, which would itself risk spawning something on a locked-down CI box).
// ---------------------------------------------------------------------------

#[test]
fn harness_never_spawns_the_daemon() {
    let src = include_str!("golden.rs");
    assert!(
        !src.contains("\"record\""),
        "golden.rs must never invoke `agentrec record` — this whole harness runs \
         with the daemon stopped and starts none (AC5)"
    );
}
