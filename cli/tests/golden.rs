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
//!
//! # Documented residual coverage gaps (D5, review round)
//!
//! P4 is a `RepositoryView` extraction over the read verbs; the review
//! round asked for goldens covering the arms inside that extraction's
//! plausible blast radius (`print_entry`'s binary/baseline-unknown/
//! missing/corrupt branches, the noise-fold line) and explicitly
//! deprioritized the rest as a stated residual rather than silently
//! skipped. The following production-reachable renderer arms are NOT
//! pinned by any golden in this file:
//!
//! - `blame_file`'s deleted-file suffix (`" · deleted this file"`) and its
//!   gap-stale suffix (`" · attribution stale — recording gap"` appended to
//!   an otherwise-touched-file line, as opposed to the whole-line untouched
//!   forms both `blame_untouched_file*` goldens already pin).
//! - `blame_line`'s "attribution unavailable — snapshot unavailable" arm
//!   and its "before recording began" / gap-poisoned "attribution stale"
//!   arms.
//! - The git-turn glossary entry, the "human-edited since" glossary term,
//!   and the "noise files" glossary term under `log --explain` (only
//!   "rich"/"bare"/"truncated" are exercised — see `golden_log_explain`).
//! - `log --utc` (absolute-timestamp rendering instead of the relative
//!   default).
//! - `show --all-files` (the noise-fold-suppression flag on `show`, as
//!   opposed to `log --all-files`, which IS covered —
//!   `golden_log_noise_all_files`).
//!
//! P4b-1 added `recall` goldens and extends the same residual list rather
//! than implying `recall` is now fully pinned. Still NOT pinned:
//!
//! - The human `recall` HITS line (`memorycmds::format_memory_line` — short
//!   id, relative time, pin rendering, color). Only the two human EMPTY
//!   states are captured; `recall_json_hits` pins the machine shape.
//! - The F3 capped notice (`"verification capped at N candidates"`, stderr)
//!   emitted when the verify walk stops at `memory::RECALL_VERIFY_CAP`.
//! - `recall --for-hook` (the memory-injection path, INV-M4).
//! - The `memories` verb in its entirety.
//!
//! Within `recall_json_hits` itself, three of `EffectiveJson`'s eight fields
//! are pinned at a structurally single value, so the golden proves they are
//! PRESENT but not that they VARY correctly: `origin` (`seed_memory`
//! hardcodes `"human"`), `freshness` (`recall_cmd` passes a literal
//! `Freshness::Fresh`), and `retracted` (always false by INV-M2 — not
//! reachable from this seam at all).
//!
//! If a future round needs these, they follow the same pattern already
//! established here: derive the exact shape from the renderer, add the
//! minimal fixture data to reach it, capture, review the bytes once before
//! committing.

use std::path::Path;
use std::process::{Command, Output};

use agentrec_core::memory::{memory_path, MemoryOp, MemoryRecord, Pin};
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
const EDGE_TURN_ID: &str = "t_EDGE000000000000000000TUR9";

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
const EDGE_STARTED: &str = "2020-01-02T15:00:00.000Z";
const EDGE_ENDED: &str = "2020-01-02T15:00:01.000Z";

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
const BINARY_BEFORE: &[u8] = &[0u8, 1, 2, 3];
const BINARY_AFTER: &[u8] = &[0u8, 1, 2, 3, 4, 5, 6];
const BASELINE_UNKNOWN_AFTER: &[u8] = b"first seen mid-session\n";
const MISSING_BLOB_GHOST: &[u8] = b"never actually stored\n";
const CORRUPT_BLOB_REAL: &[u8] = b"real content at write time\n";
const CORRUPT_BLOB_TAMPERED: &[u8] = b"tampered after the fact\n";

// ---------------------------------------------------------------------------
// Memory recall fixture constants (P4b-1) — same discipline as the turn
// fixture above: every id/timestamp/hash reaching a captured `recall` golden
// originates from exactly one of these named consts. Pin hashes are not
// literal strings (a memory's pin hash is content-addressed, computed from
// the pinned file's real bytes — `memory::hash_pin` reads the working-tree
// file directly, it never consults the CAS) — they are derived at fixture-
// build time via `store::hash_bytes` over the named `MEMORY_*_PIN_CONTENT`
// byte const, mirroring exactly how `build_fixture` above derives blob
// hashes from `APP_BEFORE` et al. via `store.put`. Never `memory::remember`'s
// real `id::ulid()` generator, never `SystemTime::now()`.
//
// Id SHAPE is faithful to what production can emit, same as the `t_`-prefixed
// turn consts above: `memorycmds::remember` mints a memory id with bare
// `id::ulid()` (`cli/src/memorycmds.rs`), so a memory id is 26 Crockford
// characters with NO prefix — unlike `id::turn_id()`, which does prefix `t_`.
// The Crockford alphabet (`agentrec-core/src/id.rs`) omits I, L, O and U, so
// no literal below may contain them.
// ---------------------------------------------------------------------------

const MEMORY_MATCH_ID: &str = "MATCH0000000000000000MEM1A";
const MEMORY_OTHER_ID: &str = "THER0000000000000000MEM2AB";
const MEMORY_MATCH_TS: u64 = 1_577_923_200_000; // 2020-01-02T00:00:00.000Z
const MEMORY_OTHER_TS: u64 = 1_577_923_260_000; // 2020-01-02T00:01:00.000Z

// The doubled `throttle` is load-bearing — do NOT reword. `bm25_rank` filters
// on `SCORE_FLOOR` = 0.8, and with a 2-document corpus the largest reachable
// idf is ln(2) ≈ 0.693 — below the floor. A single-occurrence match therefore
// cannot clear it at dl ≈ avg_dl; this fact scores ≈1.05 only because
// `throttle` occurs three times across `doc_text` (twice here, once in
// `MEMORY_MATCH_PIN_PATH`). Tidying the sentence silently turns
// `recall_json_hits` into `[]`.
const MEMORY_MATCH_FACT: &str =
    "throttle limiter guards the API from bursty traffic via throttle checks";
const MEMORY_OTHER_FACT: &str = "the release changelog script lives under scripts";
const MEMORY_MATCH_PIN_PATH: &str = "src/throttle.rs";
const MEMORY_OTHER_PIN_PATH: &str = "src/changelog.rs";
const MEMORY_MATCH_PIN_CONTENT: &[u8] = b"fn throttle() {}\n";
const MEMORY_OTHER_PIN_CONTENT: &[u8] = b"fn changelog() {}\n";
/// Replaces `MEMORY_MATCH_PIN_CONTENT` on disk after its hash is recorded, so
/// the pin verifies Stale. See `build_stale_recall_fixture`.
const MEMORY_STALE_PIN_CONTENT: &[u8] = b"fn throttle() { /* edited */ }\n";
// Free-text query terms — not ids/timestamps/hashes, so AC-1c's "traces to a
// named const" requirement doesn't bind them, but named here anyway for the
// same readability the id/fact/path consts above give.
const MEMORY_RECALL_QUERY_MATCH: &str = "throttle";
const MEMORY_RECALL_QUERY_NO_MATCH: &str = "zzzznomatch";

// ---------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

/// Run the real `agentrec` binary; never spawns `record` (AC5 — this whole
/// harness runs with the daemon stopped and starts none itself).
///
/// D10 (review fix): scrubs every `AGENTREC_*` variable from the child's
/// environment before running. Without this, a developer's shell exporting
/// e.g. `AGENTREC_TEST_STORE_BUDGET_BYTES` (read by `cmds.rs` under
/// `#[cfg(debug_assertions)]` and folded into `status_report`'s eviction
/// path) would get different `status` bytes than CI — a golden mismatch
/// that has nothing to do with the code under test.
fn agentrec(root: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(bin());
    for (key, _) in std::env::vars_os() {
        if let Some(k) = key.to_str() {
            if k.starts_with("AGENTREC_") {
                cmd.env_remove(k);
            }
        }
    }
    cmd.args(args)
        .args(["--root", root.to_str().unwrap()])
        .output()
        .expect("run agentrec")
}

/// D8 (review fix): `.status()` alone only catches spawn failure, not a
/// non-zero exit — a `git` that runs and fails would leave the fixture in a
/// non-git tempdir, silently disabling every ignore rule (the exact trap
/// this module's doc comment and `P3.md` both call out). Assert success.
fn git_init(root: &Path) {
    let status = Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(root)
        .status()
        .expect("spawn git init");
    assert!(status.success(), "git init failed for {root:?}: {status:?}");
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
        after_synthesized: None,
    }
}

fn write_file(root: &Path, rel: &str, content: &[u8]) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, content).unwrap();
}

/// Overwrites an already-`put` blob's on-disk bytes directly (bypassing
/// `BlobStore`'s own API, which has no "corrupt an object" method by
/// design) so `store.get` later sees a hash mismatch — `StoreError::Corrupt`
/// — instead of `Missing`. Mirrors `BlobStore`'s private two-char fan-out
/// layout (`objects/<hash[..2]>/<hash[2..]>`) rather than reaching into the
/// crate; that layout is PROTOCOL §6, stable, and this is a test-only
/// tamper, not a load-bearing dependency on store internals.
fn corrupt_blob(objects_dir: &Path, hash: &str, tampered: &[u8]) {
    let hex = hash.strip_prefix("sha256:").expect("well-formed hash ref");
    let (fan, rest) = hex.split_at(2);
    std::fs::write(objects_dir.join(fan).join(rest), tampered).expect("tamper blob");
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

/// Hand-seeds one `assert` line into `.agentrec/memory.jsonl` — never via
/// `memorycmds::remember`/`memory::append_memory` (which would scrub the fact
/// and fsync through the real write path unnecessarily for a fixture) and
/// never via `id::ulid()`/`SystemTime::now()`. Mirrors `seed_turn`'s
/// hand-construct-then-serialize style, just against `MemoryRecord` instead
/// of `TurnRecord`.
fn seed_memory(root: &Path, id: &str, fact: &str, pin_path: &str, pin_hash: &str, ts: u64) {
    let rec = MemoryRecord {
        v: 1,
        kind: "memory".to_string(),
        id: id.to_string(),
        op: MemoryOp::Assert,
        fact: fact.to_string(),
        pins: vec![Pin {
            path: pin_path.to_string(),
            hash: pin_hash.to_string(),
        }],
        source_turns: vec![],
        origin: "human".to_string(),
        ts,
        reason: None,
    };
    let line = serde_json::to_string(&rec).expect("serialize memory record");
    append_line_synced(&memory_path(root), &line).expect("seed memory");
}

/// Builds a minimal, deterministic `recall` fixture: an initialized repo with
/// exactly two Fresh, non-retracted, pinned memories — `MEMORY_MATCH_*`
/// (matches `MEMORY_RECALL_QUERY_MATCH` via BM25) and `MEMORY_OTHER_*`
/// (carries neither query term, so `df(throttle)` stays 1 and it never
/// appears in either query's results — it exists purely to make the store
/// non-empty for
/// `recall_json_no_match`/`recall_human_no_match`, which need "nothing fresh
/// matched" to be distinguishable from "no memories were ever recorded").
/// Both pins' real worktree bytes are written first so `memory::hash_pin`
/// verifies them Fresh (INV-M2) when `recall` walks the ranking.
fn build_recall_fixture(root: &Path) {
    init(root);
    write_file(root, MEMORY_MATCH_PIN_PATH, MEMORY_MATCH_PIN_CONTENT);
    write_file(root, MEMORY_OTHER_PIN_PATH, MEMORY_OTHER_PIN_CONTENT);
    let match_hash = agentrec_core::store::hash_bytes(MEMORY_MATCH_PIN_CONTENT);
    let other_hash = agentrec_core::store::hash_bytes(MEMORY_OTHER_PIN_CONTENT);
    seed_memory(
        root,
        MEMORY_MATCH_ID,
        MEMORY_MATCH_FACT,
        MEMORY_MATCH_PIN_PATH,
        &match_hash,
        MEMORY_MATCH_TS,
    );
    seed_memory(
        root,
        MEMORY_OTHER_ID,
        MEMORY_OTHER_FACT,
        MEMORY_OTHER_PIN_PATH,
        &other_hash,
        MEMORY_OTHER_TS,
    );
}

/// Same store as [`build_recall_fixture`]'s matching memory, except the pin's
/// worktree bytes are REWRITTEN after the hash is recorded, so
/// `memory::hash_pin` reads content that no longer matches and the memory
/// verifies Stale. `recall` returns only Fresh entries (INV-M2), so the
/// correct output is the empty array.
///
/// This fixture exists to close a one-sided blind spot the P4b-1 skeptic gate
/// found by experiment: breaking freshness in the STRICT direction (treat
/// everything as stale) reds `recall_json_hits`, but breaking it in the
/// PERMISSIVE direction (treat everything as fresh) left all goldens green,
/// because no fixture held a stale memory for a too-permissive filter to
/// wrongly admit. A silently dropped verify walk is a permissive break — and
/// that is exactly the failure mode available to P4b-3, which reimplements
/// `recall` behind `view.rs`. With this fixture, dropping the walk makes the
/// golden emit the stale memory instead of `[]`, and it reds.
fn build_stale_recall_fixture(root: &Path) {
    // Reuse the two-memory corpus verbatim. This is load-bearing, not laziness:
    // a one-memory store gives n=1, so idf = ln(0.5/1.5 + 1) ≈ 0.288, far under
    // SCORE_FLOOR 0.8 — the memory would be dropped by BM25 before freshness was
    // ever consulted, and the golden would capture `[]` for the wrong reason. It
    // was written that way first, and the permissive-freshness neuter below
    // stayed green, which is exactly how the vacuity surfaced.
    build_recall_fixture(root);
    // Drift the pinned file AFTER the hash is bound — this is what makes it stale.
    write_file(root, MEMORY_MATCH_PIN_PATH, MEMORY_STALE_PIN_CONTENT);
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
        imported: None,
        files_complete: None,
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
        imported: None,
        files_complete: None,
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
        imported: None,
        files_complete: None,
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
        imported: None,
        files_complete: None,
        files: vec![fe(
            "src/app.rs",
            Some(app_after),
            Some(app_before),
            "modify",
        )],
    };
    seed_turn(root, &undo);

    // ---- imported turn: seeded as a RAW JSON line (rather than a
    // `TurnRecord` struct literal) so this fixture pins the exact wire
    // bytes independent of whatever field order the struct happens to
    // declare — the byte contract, not the Rust type, is what a golden
    // test is supposed to nail down. Field order matches P2's frozen
    // struct declaration order exactly:
    // v, id, grade, truncated?, started, ended, tool, model, session, root,
    // prompt_ref?, prompt_excerpt?, merges?, imported, files_complete, files.
    //
    // `grade: "rich"`, `tool: "claude"` — NOT `"bare"`/`"claude-code"` (fixed
    // in the review round; the first cut of this fixture got this wrong).
    // `daemon.rs`'s `grade == "bare"` arm categorically forces
    // `(tool, session, prompt_ref, prompt_excerpt)` to `None` — a bare turn
    // carrying a tool is a shape the real system can never produce, and
    // rendering one would fabricate agent attribution onto an unattributed
    // window, directly contradicting the "bare turns never fabricate
    // attribution" invariant (CLAUDE.md; P2.md decision 6). P2.md:53's own
    // AC is explicit: imported turns land as `grade: "rich"`, `tool:
    // "claude"`. This is the shape a bare fixture-and-implementation
    // agreement would have hidden — exactly the P1 failure mode.
    //
    // On the merged tree (P2 + P3 integrated), `imported`/`files_complete`
    // are real `TurnRecord` fields and AC7's "partial file list (imported)"
    // `log` marker is live — both are exercised and pinned by this fixture:
    // the marker renders in `log_default.golden` (and friends) for this
    // turn. This RAW-JSON seeding approach still stands (see above), not
    // because the fields don't exist, but because it is the more precise
    // golden-testing technique regardless.
    let imported_line = format!(
        concat!(
            r#"{{"type":"turn","v":1,"id":"{id}","grade":"rich","#,
            r#""started":"{started}","ended":"{ended}","tool":"claude","#,
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
        imported: None,
        files_complete: None,
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

    // ---- edge turn (D5, review fix): pins `print_entry`'s remaining
    // branches that a plausible P4 `RepositoryView` extraction could change
    // without any golden noticing — binary, baseline-unknown, missing-blob,
    // and corrupt-blob, all captured by one `diff` invocation
    // (`golden_diff_edge_cases`).
    let binary_before = store.put(BINARY_BEFORE).unwrap();
    let binary_after = store.put(BINARY_AFTER).unwrap();
    let baseline_unknown_after = store.put(BASELINE_UNKNOWN_AFTER).unwrap();
    // Never `store.put` — a well-formed hash ref pointing at an object that
    // was never written, so `store.get` returns `StoreError::Missing`.
    let missing_hash = agentrec_core::store::hash_bytes(MISSING_BLOB_GHOST);
    let corrupt_hash = store.put(CORRUPT_BLOB_REAL).unwrap();
    corrupt_blob(
        &root.join(".agentrec/objects"),
        &corrupt_hash,
        CORRUPT_BLOB_TAMPERED,
    );

    let edge = TurnRecord {
        v: 1,
        id: EDGE_TURN_ID.to_string(),
        grade: "rich".to_string(),
        truncated: false,
        started: EDGE_STARTED.to_string(),
        ended: EDGE_ENDED.to_string(),
        tool: Some("claude".to_string()),
        model: None,
        session: None,
        root: "/repo".to_string(),
        prompt_ref: None,
        prompt_excerpt: Some("edge cases".to_string()),
        merges: vec![],
        imported: None,
        files_complete: None,
        files: vec![
            fe(
                "assets/img.bin",
                Some(binary_before),
                Some(binary_after),
                "modify",
            ),
            FileEntry {
                baseline_unknown: true,
                ..fe(
                    "src/baseline_unknown.rs",
                    None,
                    Some(baseline_unknown_after),
                    "modify",
                )
            },
            fe("src/missing.rs", None, Some(missing_hash), "modify"),
            fe("src/corrupt.rs", None, Some(corrupt_hash.clone()), "modify"),
        ],
    };
    seed_turn(root, &edge);
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

/// Captures `out` as `stdout:\n<stdout>\n--stderr--\n<stderr>\n--exit--\n<code>\n`
/// (text delimiters, not NUL-separated — see caveat below), normalizes it,
/// and compares against (or — under `UPDATE_GOLDEN=1` — writes)
/// `tests/fixtures/golden/<name>.golden`.
///
/// Caveat (review fix — this doc previously claimed a NUL-separated format
/// that the code below never implemented): if a captured `stdout` payload
/// ever contained the literal line `--stderr--` or `--exit--`, this format
/// would misparse on a manual read (though `assert_golden`'s own comparison
/// is a whole-string equality check, so a golden MISMATCH is still caught
/// correctly either way — the ambiguity only affects a human eyeballing the
/// `.golden` file, not correctness of the pass/fail). None of this
/// harness's captured commands can ever emit those literal lines (no
/// renderer in `cmds.rs`/`readcmds.rs`/`memorycmds.rs` prints either
/// string — `memorycmds.rs` joined this list when P4b-1 added the `recall`
/// goldens), so this is a documented risk, not a live bug.
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

/// `status`'s own failing invocation (AC2's "one failing invocation per
/// command"): `--ack-degraded` and `--json` are `conflicts_with` in clap
/// (`main.rs`'s `Status` variant) — clap itself rejects this before
/// `cmds::status` ever runs, exit code 2, clap's own usage text on stderr.
/// Pinned because that text is exactly the kind of byte that silently
/// drifts on a clap version bump, with nothing else in this suite watching
/// it.
#[test]
fn golden_status_ack_degraded_json_conflict_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    let out = agentrec(root, &["status", "--ack-degraded", "--json"]);
    assert!(!out.status.success(), "expected clap to reject: {out:?}");
    assert_golden("status_ack_degraded_json_conflict", &out);
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

/// D4 (review fix): nothing previously invoked any verb against
/// `DUP_TURN_ID`, so `resolve_turn`'s same-revert collapse arm
/// (`readcmds.rs:152`) was unpinned — the fixture's "duplicate-id pair
/// collapsible by `same_revert`" existed in `log.jsonl` but its actual
/// purpose (that querying it by id succeeds instead of erroring
/// "ambiguous") was never exercised. `diff` on the (single, collapsed)
/// duplicate id proves the collapse.
#[test]
fn golden_diff_dup_turn_collapses() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    let out = agentrec(root, &["diff", DUP_TURN_ID]);
    assert!(
        out.status.success(),
        "expected same_revert to collapse the duplicate pair, not error ambiguous: {out:?}"
    );
    assert_golden("diff_dup_turn_collapses", &out);
}

/// D5 (review fix, narrow slice): pins `print_entry`'s binary,
/// baseline-unknown, missing-blob (`StoreError::Missing`), and
/// corrupt-blob (`StoreError::Corrupt`) branches — all reachable in
/// production, all inside a plausible P4 `RepositoryView` extraction's
/// blast radius, none previously covered by any golden.
#[test]
fn golden_diff_edge_cases() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    assert_golden("diff_edge_cases", &agentrec(root, &["diff", EDGE_TURN_ID]));
}

#[test]
fn golden_blame_untouched_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    // Poisoned by the fixture's global epoch gap (see build_fixture's doc) —
    // "attribution stale — recording gap", not "no recorded turn touches".
    // The genuinely-untouched, no-gap branch is a SEPARATE golden below
    // (`golden_blame_untouched_file_no_gap`) against a second, minimal
    // fixture — without it, `blame_file`'s `no recorded turn touches <file>`
    // arm would be unreachable by any golden in this suite and could be
    // deleted with the whole suite still green (exactly the failure shape
    // the governing lesson warns about).
    assert_golden(
        "blame_untouched_file",
        &agentrec(root, &["blame", "src/untouched.rs"]),
    );
}

/// Minimal second fixture: balanced epochs (start/stop, no gap), one rich
/// turn that does NOT touch `src/untouched.rs`. Exists solely to reach
/// `blame_file`'s `no recorded turn touches {file}` arm — the main
/// `build_fixture` fixture can never reach it because its epoch gap
/// unconditionally poisons that branch via `has_recording_gap` (positional,
/// not time-scoped — see `build_fixture`'s doc comment).
fn build_fixture_no_gap(root: &Path) {
    init(root);
    let store = BlobStore::new(root.join(".agentrec/objects"));
    let before = store.put(b"a\n").unwrap();
    let after = store.put(b"b\n").unwrap();
    write_file(root, "src/touched.rs", b"b\n");

    seed_epoch(root, "start", "2020-01-01T00:00:00.000Z");
    seed_epoch(root, "stop", "2020-01-01T00:01:00.000Z");

    let turn = TurnRecord {
        v: 1,
        id: "t_NOGAP00000000000000000TUR1".to_string(),
        grade: "rich".to_string(),
        truncated: false,
        started: "2020-01-02T00:00:00.000Z".to_string(),
        ended: "2020-01-02T00:00:01.000Z".to_string(),
        tool: Some("claude".to_string()),
        model: None,
        session: None,
        root: "/repo".to_string(),
        prompt_ref: None,
        prompt_excerpt: Some("touch a file".to_string()),
        merges: vec![],
        imported: None,
        files_complete: None,
        files: vec![fe("src/touched.rs", Some(before), Some(after), "modify")],
    };
    seed_turn(root, &turn);
}

#[test]
fn golden_blame_untouched_file_no_gap() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture_no_gap(root);
    assert_golden(
        "blame_untouched_file_no_gap",
        &agentrec(root, &["blame", "src/untouched.rs"]),
    );
}

/// D5 (review fix, narrow slice): pins `log`'s `noise_globs` fold
/// (`cmds.rs:124` / NF-A/NF-B) — a third, minimal fixture, since
/// `noise_globs` is config-driven and adding a `config.toml` to the main
/// fixture would perturb every other golden's `status`/`log` byte count for
/// no reason. One turn with one noise-matching entry and one ordinary
/// entry, so the fold line's count (`+1 noise files`) and the surviving
/// visible entry are both pinned in a single capture.
fn build_fixture_noise(root: &Path) {
    init(root);
    std::fs::write(
        root.join(".agentrec/config.toml"),
        "noise_globs = [\"*.log\"]\n",
    )
    .unwrap();
    let store = BlobStore::new(root.join(".agentrec/objects"));
    let log_after = store.put(b"noisy\n").unwrap();
    let src_after = store.put(b"real change\n").unwrap();

    let turn = TurnRecord {
        v: 1,
        id: "t_NOISE00000000000000000TUR1".to_string(),
        grade: "rich".to_string(),
        truncated: false,
        started: "2020-01-02T00:00:00.000Z".to_string(),
        ended: "2020-01-02T00:00:01.000Z".to_string(),
        tool: Some("claude".to_string()),
        model: None,
        session: None,
        root: "/repo".to_string(),
        prompt_ref: None,
        prompt_excerpt: Some("touch a log and a source file".to_string()),
        merges: vec![],
        imported: None,
        files_complete: None,
        files: vec![
            fe("debug.log", None, Some(log_after), "create"),
            fe("src/real.rs", None, Some(src_after), "create"),
        ],
    };
    seed_turn(root, &turn);
}

#[test]
fn golden_log_noise_fold() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture_noise(root);
    assert_golden("log_noise_fold", &agentrec(root, &["log"]));
}

#[test]
fn golden_log_noise_all_files_reveals() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture_noise(root);
    assert_golden(
        "log_noise_all_files",
        &agentrec(root, &["log", "--all-files"]),
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
// Tests: goldens for `recall` (P4b-1) — pre-refactor byte pins for the three
// `recall --json` states and the human empty-state pair. Zero production
// changes accompany these; they exist so P4b-3's `recall` → `view.rs`
// extraction (design decision 3: `recall --json` output stays byte-
// identical) is falsifiable rather than merely asserted, the same
// instrument role P3's goldens played for P4.
// ---------------------------------------------------------------------------

#[test]
fn golden_recall_json_hits() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_recall_fixture(root);
    assert_golden(
        "recall_json_hits",
        &agentrec(root, &["recall", MEMORY_RECALL_QUERY_MATCH, "--json"]),
    );
}

/// Same query and same single memory as [`golden_recall_json_hits`], but the
/// pin has drifted, so freshness verification must exclude it and the output
/// must be `[]`. Pairs with that test as a two-sided pin on the verify walk:
/// `recall_json_hits` reds if freshness is broken STRICTLY (a fresh memory
/// wrongly excluded), this one reds if it is broken PERMISSIVELY (a stale
/// memory wrongly admitted). Only the strict direction was covered before —
/// see `build_stale_recall_fixture` for why that gap mattered.
#[test]
fn golden_recall_json_stale_pin() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_stale_recall_fixture(root);
    assert_golden(
        "recall_json_stale_pin",
        &agentrec(root, &["recall", MEMORY_RECALL_QUERY_MATCH, "--json"]),
    );
}

/// `recall_json_no_match` and [`golden_recall_json_empty_store`] both
/// capture the literal `[]` — byte-identical stdout, from two DIFFERENT
/// fixtures (a non-empty store with no fresh match here, vs. a store that
/// never had a memory recorded at all in the sibling test). This is expected
/// duplication, not a bug to "fix" by merging or deleting one of them:
/// `memorycmds::recall_cmd`'s `--json` branch returns the (possibly-empty)
/// `arr` before the human branch's `hits.is_empty()` check ever runs, so
/// `--json` mode cannot distinguish "nothing recorded" from "nothing fresh
/// matched" by construction (design decision 3, P4b-1 plan) — the human pair
/// below (`recall_human_no_memories` / `recall_human_no_match`) is the ONLY
/// place that distinction is actually observable. A future reader should not
/// deduplicate these two `--json` goldens on the theory that identical bytes
/// mean redundant tests.
#[test]
fn golden_recall_json_no_match() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_recall_fixture(root);
    assert_golden(
        "recall_json_no_match",
        &agentrec(root, &["recall", MEMORY_RECALL_QUERY_NO_MATCH, "--json"]),
    );
}

/// See [`golden_recall_json_no_match`]'s doc comment: this and that test
/// capture byte-identical `[]` stdout from different fixtures, deliberately.
#[test]
fn golden_recall_json_empty_store() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root); // initialized repo, no memory.jsonl ever written
    assert_golden(
        "recall_json_empty_store",
        &agentrec(root, &["recall", MEMORY_RECALL_QUERY_NO_MATCH, "--json"]),
    );
}

/// A store with NO memories ever recorded: `memorycmds::recall_cmd`'s human
/// branch loads the full effective set, finds it empty, and prints the PD3
/// zero-state message to **stderr** (never stdout) at exit 0 — matching
/// `log`/`status`'s convention that an honest "nothing here yet" notice is
/// not a failure.
#[test]
fn golden_recall_human_no_memories() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    assert_golden(
        "recall_human_no_memories",
        &agentrec(root, &["recall", MEMORY_RECALL_QUERY_NO_MATCH]),
    );
}

/// A non-empty store whose one query term matches nothing fresh: the human
/// branch's effective set is non-empty, so this takes the OTHER half of the
/// same `if all.is_empty() { .. } else { .. }` — the plain notice on
/// **stdout** (never stderr), also exit 0. This and
/// [`golden_recall_human_no_memories`] are the pair `RecallPage::store_empty`
/// exists to preserve; see that test's doc comment for why the `--json`
/// twins of these two cannot show the same distinction.
#[test]
fn golden_recall_human_no_match() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_recall_fixture(root);
    assert_golden(
        "recall_human_no_match",
        &agentrec(root, &["recall", MEMORY_RECALL_QUERY_NO_MATCH]),
    );
}

// ---------------------------------------------------------------------------
// Test: determinism (AC3) — three FRESH fixture builds, each in its own
// tempdir. A single shared fixture reused across three runs would never
// catch an absolute-tempdir-path leak; three independent builds do.
// ---------------------------------------------------------------------------

#[test]
fn three_fresh_builds_are_byte_identical() {
    // D6 (review fix): the original list covered only `log`/`status`, but
    // those never touch the worktree or the blob store by absolute path.
    // `blame` reads `root.join(&file)` directly (`readcmds.rs:361`) and
    // `diff`/`show --prompt` resolve blobs from `objects_dir(root)` — those
    // are the three verbs that could actually leak a tempdir path, and
    // they were outside this loop.
    const COMMANDS: &[&[&str]] = &[
        &["log"],
        &["log", "--all"],
        &["log", "--json"],
        &["log", "--explain"],
        &["status"],
        &["status", "--json"],
        &["diff", RICH_TURN_ID],
        &["blame", "src/app.rs"],
        &["blame", "src/app.rs:2"],
        &["show", RICH_TURN_ID, "--prompt"],
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
