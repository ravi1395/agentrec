//! Protocol 1.0 conformance suite (PROTOCOL.md §4, §5, §10).
//!
//! `docs/fixtures/conformance/` is the frozen wire's example corpus: the lines
//! an external emitter or consumer can test itself against. This file asserts
//! that OUR reader agrees with the classification each fixture's manifest row
//! declares — every valid line parses, every wrong-major line is refused (§10),
//! every corrupt line is counted as corruption and not as a newer producer.
//!
//! Two properties this file deliberately buys, both learned from this repo's
//! own history:
//!
//! 1. **The manifest is exact, not a floor.** `MANIFEST` is compared to the
//!    directory listing by SET EQUALITY, so a deleted fixture reds, an added
//!    but undeclared fixture reds, and a typo'd filename reds. A
//!    `count >= N` guard would let a deletion pass.
//! 2. **Every valid fixture is serializer-derived.** They are produced by
//!    `regenerate_conformance_fixtures` (ignored by default, below) from the
//!    real `agentrec_core::record` types, not typed by hand — a hand-typed
//!    fixture that drifts from the wire is the failure mode that let a
//!    `type: "snapshot"` literal, invented by a synthetic fixture, pass an
//!    entire import gate while nothing real resolved.
//!
//! The corrupt/unknown-field fixtures are necessarily hand-written: they are
//! shapes no serializer can emit. Their provenance is recorded per row.

#![allow(clippy::disallowed_methods)]
//  ^ Test code reads its own tempdir fixtures, which this harness created;
//    there is no attacker-supplied FIFO to block on, so the fsguard wrappers
//    buy nothing here. Production reads stay lint-enforced (clippy.toml).

use agentrec_core::record::{
    parse_log_line, parse_signals, EpochRecord, FileEntry, LogRecord, ParsedLine, SignalEvent,
    TurnRecord,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What our reader MUST do with a fixture line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// `signal.jsonl` line: `parse_signals` yields exactly one event.
    SignalAccepted,
    /// `signal.jsonl` line: `parse_signals` yields nothing. §10 is explicit
    /// that the signal schema keeps NO census — an unsupported major is
    /// dropped indistinguishably from an unparseable line.
    SignalRefused,
    /// `log.jsonl` line: parses to `LogRecord::Turn`.
    LogTurn,
    /// `log.jsonl` line: parses to `LogRecord::Epoch`.
    LogEpoch,
    /// `log.jsonl` line: refused, and counted as "a schema this binary does
    /// not implement" — a newer producer, NOT damage (§10 third bullet).
    LogUnknownType,
    /// `log.jsonl` line: refused, and counted as corruption.
    LogUnparsed,
}

/// class dir, filename, expectation.
///
/// Set equality against the on-disk listing is asserted in
/// `fixture_manifest_matches_disk`, so this table is the single source of
/// truth for what the corpus contains.
const MANIFEST: &[(&str, &str, Expect)] = &[
    // ---- valid: signal schema (§4) ----
    ("valid", "signal_stop_minimal.jsonl", Expect::SignalAccepted),
    ("valid", "signal_stop_full.jsonl", Expect::SignalAccepted),
    (
        "valid",
        "signal_start_minimal.jsonl",
        Expect::SignalAccepted,
    ),
    (
        "valid",
        "signal_start_with_emitter_turn.jsonl",
        Expect::SignalAccepted,
    ),
    (
        "valid",
        "signal_start_with_prompt.jsonl",
        Expect::SignalAccepted,
    ),
    (
        "valid",
        "signal_memory_candidate.jsonl",
        Expect::SignalAccepted,
    ),
    ("valid", "signal_absent_v.jsonl", Expect::SignalAccepted),
    // Hand-written: PROTOCOL §4's own example line, verbatim. No serializer
    // emits it (ours always writes the optional keys as explicit `null`), but
    // it is the shape the spec shows an external emitter, so our reader had
    // better accept it.
    (
        "valid",
        "signal_stop_protocol_example.jsonl",
        Expect::SignalAccepted,
    ),
    // ---- valid: record schema (§5) ----
    ("valid", "turn_rich.jsonl", Expect::LogTurn),
    ("valid", "turn_bare.jsonl", Expect::LogTurn),
    ("valid", "turn_git.jsonl", Expect::LogTurn),
    ("valid", "turn_undo.jsonl", Expect::LogTurn),
    ("valid", "turn_undo_mcp_origin.jsonl", Expect::LogTurn),
    ("valid", "turn_imported_partial.jsonl", Expect::LogTurn),
    ("valid", "turn_absent_v.jsonl", Expect::LogTurn),
    ("valid", "epoch_start.jsonl", Expect::LogEpoch),
    (
        "valid",
        "epoch_start_dropped_signals.jsonl",
        Expect::LogEpoch,
    ),
    ("valid", "epoch_stop.jsonl", Expect::LogEpoch),
    // ---- tolerated: refused as a RECORD, but not corruption ----
    (
        "tolerated",
        "log_unknown_record_type.jsonl",
        Expect::LogUnknownType,
    ),
    (
        "tolerated",
        "turn_unknown_fields.jsonl",
        // Unknown FIELDS are additive change and MUST parse (§10 first para).
        Expect::LogTurn,
    ),
    (
        "tolerated",
        "signal_unknown_fields.jsonl",
        Expect::SignalAccepted,
    ),
    // ---- invalid: wrong major (§10 "MUST refuse the line") ----
    (
        "invalid",
        "turn_wrong_major_v99.jsonl",
        Expect::LogUnknownType,
    ),
    (
        "invalid",
        "epoch_wrong_major_v99.jsonl",
        Expect::LogUnknownType,
    ),
    (
        "invalid",
        "signal_wrong_major_v99.jsonl",
        Expect::SignalRefused,
    ),
    // ---- invalid: corruption ----
    ("invalid", "log_malformed_json.jsonl", Expect::LogUnparsed),
    ("invalid", "log_v_is_string.jsonl", Expect::LogUnparsed),
    ("invalid", "log_v_is_negative.jsonl", Expect::LogUnparsed),
    ("invalid", "log_v_is_float.jsonl", Expect::LogUnparsed),
    ("invalid", "log_type_is_number.jsonl", Expect::LogUnparsed),
    (
        "invalid",
        "signal_malformed_json.jsonl",
        Expect::SignalRefused,
    ),
];

const CLASSES: [&str; 3] = ["valid", "tolerated", "invalid"];

fn fixture_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `<repo>/cli` for `cli/tests/*`.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/fixtures/conformance")
}

fn read_fixture(class: &str, name: &str) -> String {
    let path = fixture_root().join(class).join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("conformance fixture {} unreadable: {e}", path.display()))
}

/// The corpus on disk is EXACTLY the manifest — no more, no less.
///
/// This is the guard that makes a deleted fixture a test failure instead of a
/// silently smaller run. It also catches the opposite drift: a fixture added
/// to the directory without a declared expectation would otherwise be checked
/// by nothing at all.
#[test]
fn fixture_manifest_matches_disk() {
    let root = fixture_root();
    assert!(
        root.is_dir(),
        "conformance fixture directory missing: {}",
        root.display()
    );

    let mut on_disk: BTreeSet<String> = BTreeSet::new();
    for class in CLASSES {
        let dir = root.join(class);
        assert!(
            dir.is_dir(),
            "conformance fixture class directory missing: {}",
            dir.display()
        );
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()));
        for entry in entries {
            let entry = entry.expect("dir entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            on_disk.insert(format!("{class}/{name}"));
        }
    }

    let declared: BTreeSet<String> = MANIFEST
        .iter()
        .map(|(class, name, _)| format!("{class}/{name}"))
        .collect();

    assert_eq!(
        declared, on_disk,
        "conformance corpus drifted from MANIFEST (left = declared, right = on disk)"
    );

    // A floor as well as the equality above: if someone ever relaxes the set
    // comparison, an emptied corpus still reds here.
    assert!(
        declared.len() >= 29,
        "conformance corpus shrank to {} entries",
        declared.len()
    );
}

/// Every fixture is dispatched to a reader by filename prefix; an unrecognized
/// prefix is a hard failure, never a silent skip.
fn reader_for(name: &str) -> &'static str {
    if name.starts_with("signal_") {
        "signal"
    } else if name.starts_with("turn_") || name.starts_with("epoch_") || name.starts_with("log_") {
        "log"
    } else {
        panic!(
            "fixture {name} has no known reader prefix \
             (signal_ / turn_ / epoch_ / log_) — it would exercise nothing"
        )
    }
}

/// The core assertion: our reader's verdict on every fixture equals the
/// manifest's declared expectation.
#[test]
fn reader_agrees_with_every_declared_expectation() {
    for (class, name, expect) in MANIFEST {
        let text = read_fixture(class, name);
        let reader = reader_for(name);
        let ctx = format!("{class}/{name}");

        match expect {
            Expect::SignalAccepted => {
                assert_eq!(
                    reader, "signal",
                    "{ctx}: signal expectation on a log reader"
                );
                let events = parse_signals(&text);
                assert_eq!(
                    events.len(),
                    1,
                    "{ctx}: expected exactly one accepted signal"
                );
            }
            Expect::SignalRefused => {
                assert_eq!(
                    reader, "signal",
                    "{ctx}: signal expectation on a log reader"
                );
                let events = parse_signals(&text);
                assert!(
                    events.is_empty(),
                    "{ctx}: expected refusal, reader accepted {} signal(s)",
                    events.len()
                );
            }
            _ => {
                assert_eq!(reader, "log", "{ctx}: log expectation on a signal reader");
                let line = text.lines().next().unwrap_or("");
                let parsed = parse_log_line(line);
                match (expect, &parsed) {
                    (Expect::LogTurn, ParsedLine::Record(LogRecord::Turn(_))) => {}
                    (Expect::LogEpoch, ParsedLine::Record(LogRecord::Epoch(_))) => {}
                    (Expect::LogUnknownType, ParsedLine::UnknownType) => {}
                    (Expect::LogUnparsed, ParsedLine::Unparsed) => {}
                    _ => panic!("{ctx}: expected {expect:?}, reader said {parsed:?}"),
                }
            }
        }
    }
}

/// Spot-check the SEMANTICS a few fixtures exist to pin, so the suite is not
/// merely "the bytes deserialize". Each of these would survive a pure
/// accept/refuse check while carrying the wrong meaning.
#[test]
fn named_fixtures_carry_the_semantics_they_are_named_for() {
    // §4: a memory-candidate is NOT a turn boundary.
    let cand = &parse_signals(&read_fixture("valid", "signal_memory_candidate.jsonl"))[0];
    assert!(
        cand.is_memory_candidate(),
        "memory-candidate not recognized"
    );
    assert!(!cand.is_start(), "memory-candidate must not be a start");
    assert_eq!(cand.event, None, "memory-candidate carries no `event`");

    // §4: start signals, and the optional emitter-assigned identity.
    let plain_start = &parse_signals(&read_fixture("valid", "signal_start_minimal.jsonl"))[0];
    assert!(plain_start.is_start());
    assert_eq!(
        plain_start.emitter_turn, None,
        "minimal start must omit emitter_turn (Claude Code's shape)"
    );
    assert_eq!(plain_start.emitter_event, None);
    let id_start = &parse_signals(&read_fixture(
        "valid",
        "signal_start_with_emitter_turn.jsonl",
    ))[0];
    assert!(id_start.is_start());
    assert!(id_start.emitter_turn.is_some());
    assert!(id_start.emitter_event.is_some());

    // §4: `files_written` present vs absent is a real distinction — absence
    // means "did not declare", never "wrote nothing".
    let minimal_stop = &parse_signals(&read_fixture("valid", "signal_stop_minimal.jsonl"))[0];
    assert_eq!(minimal_stop.event.as_deref(), Some("stop"));
    assert_eq!(minimal_stop.files_written, None);
    assert_eq!(minimal_stop.model, None);
    let full_stop = &parse_signals(&read_fixture("valid", "signal_stop_full.jsonl"))[0];
    assert!(full_stop
        .files_written
        .as_ref()
        .is_some_and(|f| !f.is_empty()));
    assert!(full_stop.model.is_some());
    assert!(full_stop.emitter_event.is_some());

    // §10: an absent `v` reads as major 1 on both schemas.
    let absent_v_sig = &parse_signals(&read_fixture("valid", "signal_absent_v.jsonl"))[0];
    assert_eq!(absent_v_sig.v, 1);
    assert!(
        !read_fixture("valid", "signal_absent_v.jsonl").contains("\"v\""),
        "signal_absent_v fixture must actually omit `v`"
    );
    let absent_v_turn = load_turn("valid", "turn_absent_v.jsonl");
    assert_eq!(absent_v_turn.v, 1);
    assert!(
        !read_fixture("valid", "turn_absent_v.jsonl").contains("\"v\""),
        "turn_absent_v fixture must actually omit `v`"
    );

    // §5: grades and the tool attribution rules around them.
    let rich = load_turn("valid", "turn_rich.jsonl");
    assert_eq!(rich.grade, "rich");
    assert!(rich.tool.is_some());
    let bare = load_turn("valid", "turn_bare.jsonl");
    assert_eq!(bare.grade, "bare");
    assert_eq!(bare.tool, None, "a bare turn must not name a tool (§5)");
    assert_eq!(
        load_turn("valid", "turn_git.jsonl").tool.as_deref(),
        Some("git")
    );
    assert_eq!(
        load_turn("valid", "turn_undo.jsonl").tool.as_deref(),
        Some("agentrec"),
        "an undo is itself a turn (§8)"
    );

    // §5 F5: the origin discriminator, both values plus the absent reading.
    assert_eq!(load_turn("valid", "turn_undo.jsonl").origin(), "cli");
    assert_eq!(
        load_turn("valid", "turn_undo_mcp_origin.jsonl").origin(),
        "mcp"
    );
    // Absent-means-cli is the reading every pre-F5 undo turn depends on, and
    // no fixture here can carry it — `turn_undo.jsonl` is emitted by the real
    // serializer, which always writes the field now. Pinned instead against a
    // line with no `origin` key at all, in the shape that fixture had before
    // F5, so the default is exercised on real §5 bytes rather than only in
    // `cli/tests/undo_origin.rs`.
    let pre_f5 = read_fixture("valid", "turn_undo.jsonl").replace(",\"origin\":\"cli\"", "");
    assert!(
        !pre_f5.contains("origin"),
        "the stripped line has no origin"
    );
    let ParsedLine::Record(LogRecord::Turn(legacy)) = parse_log_line(pre_f5.trim()) else {
        panic!("a pre-F5 undo turn must still parse");
    };
    assert_eq!(legacy.origin, None);
    assert_eq!(legacy.origin(), "cli", "absent means cli, not unknown");

    // §5 D51: the epoch field, present and absent.
    let plain_epoch = load_epoch("valid", "epoch_start.jsonl");
    assert_eq!(plain_epoch.event, "start");
    assert_eq!(plain_epoch.dropped_signals, 0);
    assert!(
        !read_fixture("valid", "epoch_start.jsonl").contains("dropped_signals"),
        "a zero count must stay OFF the wire (§5: pre-D51 lines byte-identical)"
    );
    let dropped = load_epoch("valid", "epoch_start_dropped_signals.jsonl");
    assert_eq!(dropped.dropped_signals, 3);
    assert_eq!(load_epoch("valid", "epoch_stop.jsonl").event, "stop");

    // §10: unknown FIELDS are tolerated on both schemas — this is the half of
    // §10 that must NOT refuse, and it is easy to break while tightening the
    // half that must.
    let tolerated_turn = load_turn("tolerated", "turn_unknown_fields.jsonl");
    assert_eq!(tolerated_turn.grade, "rich");
    assert!(
        read_fixture("tolerated", "turn_unknown_fields.jsonl")
            .contains("future_field_from_a_newer_producer"),
        "fixture must actually carry an unknown field"
    );
    let tolerated_sig =
        &parse_signals(&read_fixture("tolerated", "signal_unknown_fields.jsonl"))[0];
    assert_eq!(tolerated_sig.tool, "claude-code");
}

fn load_turn(class: &str, name: &str) -> TurnRecord {
    let text = read_fixture(class, name);
    match parse_log_line(text.lines().next().unwrap_or("")) {
        ParsedLine::Record(LogRecord::Turn(t)) => t,
        other => panic!("{class}/{name}: expected a turn record, got {other:?}"),
    }
}

fn load_epoch(class: &str, name: &str) -> EpochRecord {
    let text = read_fixture(class, name);
    match parse_log_line(text.lines().next().unwrap_or("")) {
        ParsedLine::Record(LogRecord::Epoch(e)) => e,
        other => panic!("{class}/{name}: expected an epoch record, got {other:?}"),
    }
}

/// Regenerate every serializer-derived fixture. Ignored by default — it
/// WRITES into `docs/fixtures/conformance/`.
///
/// ```text
/// cargo test --test conformance -- --ignored regenerate_conformance_fixtures
/// ```
///
/// Only the `valid/` corpus and the two wrong-major `turn_`/`epoch_` lines are
/// emitted here; the corrupt and unknown-field fixtures are shapes no
/// serializer can produce and are maintained by hand (see the corpus README).
#[test]
#[ignore = "writes fixtures; run explicitly after a wire-type change"]
fn regenerate_conformance_fixtures() {
    let valid = fixture_root().join("valid");
    std::fs::create_dir_all(&valid).expect("mkdir valid");
    let invalid = fixture_root().join("invalid");
    std::fs::create_dir_all(&invalid).expect("mkdir invalid");

    let write = |dir: &Path, name: &str, line: String| {
        std::fs::write(dir.join(name), format!("{line}\n")).expect("write fixture");
    };
    let sig = |e: &SignalEvent| serde_json::to_string(e).expect("serialize signal");
    let rec = |r: &LogRecord| serde_json::to_string(r).expect("serialize record");

    // --- signals (§4) ---
    let base_stop = SignalEvent {
        v: 1,
        ts: 1_751_724_242_183,
        tool: "claude-code".into(),
        event: Some("stop".into()),
        session: None,
        transcript: None,
        prompt: None,
        files_written: None,
        emitter_turn: None,
        emitter_event: None,
        model: None,
        kind: None,
        fact: None,
        pins: None,
    };
    write(&valid, "signal_stop_minimal.jsonl", sig(&base_stop));

    let full_stop = SignalEvent {
        session: Some("01JXSESSION0000000000000000".into()),
        transcript: Some("/Users/dev/.codex/sessions/2026/08/05/rollout.jsonl".into()),
        files_written: Some(vec![
            "/Users/dev/repo/src/auth.rs".into(),
            "/Users/dev/repo/src/lib.rs".into(),
        ]),
        emitter_turn: Some("turn_7f3a91".into()),
        emitter_event: Some("event_01JXSTOP0000000000000000".into()),
        model: Some("gpt-5-codex".into()),
        tool: "codex".into(),
        ..base_stop.clone()
    };
    write(&valid, "signal_stop_full.jsonl", sig(&full_stop));

    let start = SignalEvent {
        event: Some("start".into()),
        ..base_stop.clone()
    };
    write(&valid, "signal_start_minimal.jsonl", sig(&start));
    write(
        &valid,
        "signal_start_with_emitter_turn.jsonl",
        sig(&SignalEvent {
            tool: "codex".into(),
            session: Some("01JXSESSION0000000000000000".into()),
            emitter_turn: Some("turn_7f3a91".into()),
            emitter_event: Some("event_01JXSTART00000000000000".into()),
            model: Some("gpt-5-codex".into()),
            ..start.clone()
        }),
    );
    write(
        &valid,
        "signal_memory_candidate.jsonl",
        sig(&SignalEvent {
            event: None,
            kind: Some("memory-candidate".into()),
            fact: Some("repo uses pnpm, not npm".into()),
            pins: Some(vec!["package.json".into()]),
            ..base_stop.clone()
        }),
    );
    // The `prompt` field is a SHIPPED wire field that PROTOCOL.md §4's table
    // does not describe. BOTH hook emitters set it, post-scrub, on their
    // start event: `cli/src/cmds.rs::hook` (Claude, `UserPromptSubmit`) and
    // `cli/src/hookcmds.rs::hook_codex` (Codex, `UserPromptSubmit` arm).
    // See the corpus README.
    write(
        &valid,
        "signal_start_with_prompt.jsonl",
        sig(&SignalEvent {
            prompt: Some("fix the auth token refresh; the key is [redacted:entropy]".into()),
            ..start.clone()
        }),
    );
    write(&valid, "signal_absent_v.jsonl", strip_v(sig(&base_stop)));
    write(
        &invalid,
        "signal_wrong_major_v99.jsonl",
        sig(&SignalEvent {
            v: 99,
            ..base_stop.clone()
        }),
    );

    // --- turn records (§5) ---
    let modify = FileEntry {
        path: "src/auth.rs".into(),
        before: Some(
            "sha256:1c8b1f2e4a5d6c7b8a9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e".into(),
        ),
        after: Some(
            "sha256:9f8e7d6c5b4a39281706f5e4d3c2b1a09f8e7d6c5b4a39281706f5e4d3c2b1a0".into(),
        ),
        op: "modify".into(),
        skipped: false,
        withheld: false,
        baseline_unknown: false,
        skipped_reason: None,
        after_synthesized: None,
        link_kind: None,
        attribution: None,
    };
    let rich = TurnRecord {
        v: 1,
        id: "t_01JXR9V0M0QK3F5T7W9Y1B3D5F".into(),
        grade: "rich".into(),
        truncated: false,
        started: "2026-08-05T14:22:01.412Z".into(),
        ended: "2026-08-05T14:23:47.008Z".into(),
        tool: Some("claude-code".into()),
        model: Some("claude-opus-4".into()),
        session: Some("01JXSESSION0000000000000000".into()),
        root: "/Users/dev/repo".into(),
        prompt_ref: Some(
            "sha256:4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c".into(),
        ),
        prompt_excerpt: Some("fix the auth token refresh".into()),
        merges: vec!["t_01JXR9TZQ0QK3F5T7W9Y1B3D50".into()],
        imported: None,
        files_complete: None,
        origin: None,
        files: vec![
            modify.clone(),
            FileEntry {
                path: ".env.local".into(),
                before: None,
                after: None,
                op: "modify".into(),
                withheld: true,
                ..modify.clone()
            },
            FileEntry {
                path: "assets/blob.bin".into(),
                before: None,
                after: Some(
                    "sha256:0011223344556677889900112233445566778899001122334455667788990011"
                        .into(),
                ),
                op: "create".into(),
                skipped: true,
                skipped_reason: Some("over_cap".into()),
                ..modify.clone()
            },
            FileEntry {
                path: "node_modules/.bin/tsc".into(),
                before: Some(
                    "sha256:aabbccddeeff00112233445566778899aabbccddeeff001122334455667788".into(),
                ),
                after: None,
                op: "delete".into(),
                link_kind: Some("symlink".into()),
                ..modify.clone()
            },
        ],
    };
    write(
        &valid,
        "turn_rich.jsonl",
        rec(&LogRecord::Turn(rich.clone())),
    );

    let bare = TurnRecord {
        id: "t_01JXR9W1N1RL4G6U8X0Z2C4E6G".into(),
        grade: "bare".into(),
        tool: None,
        model: None,
        session: None,
        prompt_ref: None,
        prompt_excerpt: None,
        merges: vec![],
        files: vec![modify.clone()],
        ..rich.clone()
    };
    write(
        &valid,
        "turn_bare.jsonl",
        rec(&LogRecord::Turn(bare.clone())),
    );

    write(
        &valid,
        "turn_git.jsonl",
        rec(&LogRecord::Turn(TurnRecord {
            id: "t_01JXR9X2P2SM5H7V9Y1A3D5F7H".into(),
            grade: "rich".into(),
            tool: Some("git".into()),
            model: None,
            prompt_ref: None,
            prompt_excerpt: None,
            merges: vec![],
            files: vec![modify.clone()],
            ..rich.clone()
        })),
    );

    // An undo turn as `readcmds::append_undo_turn` writes one, `origin`
    // included (F5, delta decision 11). The two origins get one fixture each
    // rather than one fixture and a prose note: `origin` is the field a
    // consumer counting agent-initiated reverts keys on, and a corpus that
    // only ever shows `"cli"` lets a reader that hardcodes it pass.
    let undo = TurnRecord {
        id: "t_01JXR9Y3Q3TN6J8W0Z2B4E6G8J".into(),
        grade: "rich".into(),
        tool: Some("agentrec".into()),
        model: None,
        prompt_ref: None,
        prompt_excerpt: None,
        merges: vec![],
        origin: Some(TurnRecord::CLI.into()),
        files: vec![FileEntry {
            // The revert swaps the hashes of the turn being undone.
            before: modify.after.clone(),
            after: modify.before.clone(),
            ..modify.clone()
        }],
        ..rich.clone()
    };
    write(
        &valid,
        "turn_undo.jsonl",
        rec(&LogRecord::Turn(undo.clone())),
    );
    write(
        &valid,
        "turn_undo_mcp_origin.jsonl",
        rec(&LogRecord::Turn(TurnRecord {
            id: "t_01JXR9Y4S5VQ8M0Y2C4E6G8J0M".into(),
            origin: Some(TurnRecord::MCP.into()),
            ..undo.clone()
        })),
    );

    // Import provenance + a derived `after`. NOTE: `imported`,
    // `files_complete` and `after_synthesized` are SHIPPED wire fields that
    // PROTOCOL.md does not yet describe — see the corpus README.
    write(
        &valid,
        "turn_imported_partial.jsonl",
        rec(&LogRecord::Turn(TurnRecord {
            id: "t_01JXR9Z4R4UP7K9X1A3C5F7H9K".into(),
            grade: "rich".into(),
            tool: Some("claude-code".into()),
            merges: vec![],
            imported: Some(true),
            files_complete: Some(false),
            origin: None,
            files: vec![FileEntry {
                after_synthesized: Some(true),
                baseline_unknown: true,
                before: None,
                ..modify.clone()
            }],
            ..rich.clone()
        })),
    );

    write(
        &valid,
        "turn_absent_v.jsonl",
        strip_v(rec(&LogRecord::Turn(bare.clone()))),
    );
    write(
        &invalid,
        "turn_wrong_major_v99.jsonl",
        rec(&LogRecord::Turn(TurnRecord { v: 99, ..rich })),
    );

    // --- epoch records (§5) ---
    let epoch_start = EpochRecord {
        v: 1,
        event: "start".into(),
        ts: "2026-08-05T14:20:00.000Z".into(),
        dropped_signals: 0,
    };
    write(
        &valid,
        "epoch_start.jsonl",
        rec(&LogRecord::Epoch(epoch_start.clone())),
    );
    write(
        &valid,
        "epoch_start_dropped_signals.jsonl",
        rec(&LogRecord::Epoch(EpochRecord {
            dropped_signals: 3,
            ..epoch_start.clone()
        })),
    );
    write(
        &valid,
        "epoch_stop.jsonl",
        rec(&LogRecord::Epoch(EpochRecord {
            event: "stop".into(),
            ts: "2026-08-05T18:04:11.900Z".into(),
            ..epoch_start.clone()
        })),
    );
    write(
        &invalid,
        "epoch_wrong_major_v99.jsonl",
        rec(&LogRecord::Epoch(EpochRecord {
            v: 99,
            ..epoch_start
        })),
    );
}

/// Remove a top-level key from a serialized line, preserving everything else.
/// Used only to produce the two "absent `v`" fixtures (§10's last bullet):
/// the shape is legal on the wire but no serializer emits it, since `v` is a
/// writer MUST.
/// String surgery, not a `serde_json::Value` round-trip, deliberately: a
/// round-trip through `Value`'s `BTreeMap` re-sorts every key alphabetically,
/// which would make these two fixtures the only ones in the corpus whose field
/// order does not match what a real writer emits.
fn strip_v(line: String) -> String {
    let needle = "\"v\":1,";
    assert!(line.contains(needle), "no `{needle}` to strip from {line}");
    let out = line.replacen(needle, "", 1);
    assert!(!out.contains("\"v\""), "stripping left a `v` behind: {out}");
    out
}
