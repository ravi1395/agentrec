//! Phase 5 acceptance tests: `attest manual-declare` / `review` / `gate` /
//! `report`, the unhidden subcommand, and `attest status`'s additive keys.
//!
//! Fixtures are literal `attest.jsonl` lines, written by hand from the shapes
//! `ATTEST-FORMAT.md` documents, so a serializer regression cannot make a test
//! agree with itself.

#![allow(clippy::disallowed_methods)]
//  ^ Test code spawns the binary under test and `git`, and reads its own
//    tempdir fixtures. Production spawns/reads stay lint-enforced.

use std::path::Path;
use std::process::{Command, Output};

use agentrec_core::attest::events::{AttestEvent, HumanAnswer, ManualSeverity, StaleCause};
use agentrec_core::attest::types::{
    ClaimId, RecipeInvalidCause, StructuredResult, TestIdentity, TestOutcome, VerdictKind,
};

fn bin() -> std::path::PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("agentrec")
}

fn agentrec(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .args(["--root", root.to_str().unwrap()])
        .output()
        .expect("run agentrec")
}

fn agentrec_stdin(root: &Path, args: &[&str], stdin: &str) -> Output {
    use std::io::Write;
    let mut child = Command::new(bin())
        .args(args)
        .args(["--root", root.to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn agentrec");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().expect("agentrec output")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn cid(n: u8) -> ClaimId {
    ClaimId::mint_with(u64::from(n), &[n; 10])
}

fn ident(n: u8) -> TestIdentity {
    TestIdentity::new("agentrec--attest_gate", format!("t{n}"))
}

/// Seed `.agentrec/attest.jsonl` with literal serialized events.
fn seed(root: &Path, events: &[AttestEvent]) {
    let dir = root.join(".agentrec");
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::new();
    for ev in events {
        body.push_str(&serde_json::to_string(ev).unwrap());
        body.push('\n');
    }
    std::fs::write(dir.join("attest.jsonl"), body).unwrap();
}

fn declare(n: u8, severity: ManualSeverity) -> AttestEvent {
    AttestEvent::ManualDeclare {
        ts: 100 + u64::from(n),
        claim_id: cid(n),
        text: format!("manual criterion {n}"),
        severity,
    }
}

fn human(n: u8, answer: HumanAnswer) -> AttestEvent {
    AttestEvent::Human {
        ts: 200 + u64::from(n),
        claim_id: cid(n),
        answer,
        note: None,
    }
}

fn verdict(n: u8, kind: VerdictKind) -> AttestEvent {
    AttestEvent::Verdict {
        ts: 300 + u64::from(n),
        claim_id: cid(n),
        verdict: kind,
        replay_commit: format!("commit{n}"),
    }
}

fn derive(n: u8) -> AttestEvent {
    AttestEvent::Derive {
        ts: u64::from(n),
        claim_id: cid(n),
        test_identity: ident(n),
        body_hash: None,
        renamed_from: None,
    }
}

fn evidence(n: u8, turn: Option<&str>, dirty: bool) -> AttestEvent {
    AttestEvent::Evidence {
        ts: 50 + u64::from(n),
        claim_id: cid(n),
        turn_id: turn.map(str::to_string),
        dirty,
        output_blob: Some(format!("blob{n}")),
        result: StructuredResult::outcome(ident(n), TestOutcome::Passed),
    }
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P5-1
// ---------------------------------------------------------------------------

#[test]
fn p5_1_manual_declare_appends_and_status_shows_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".agentrec")).unwrap();

    let o = agentrec(
        root,
        &[
            "attest",
            "manual-declare",
            "--text",
            "the README install line is correct",
            "--severity",
            "blocking",
        ],
    );
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    let printed = out(&o).trim().to_string();
    assert!(
        ClaimId::parse(&printed).is_some(),
        "must print a well-formed claim id, got {printed:?}"
    );

    // Exactly one event landed, and it is the declare with our text.
    let body = std::fs::read_to_string(root.join(".agentrec/attest.jsonl")).unwrap();
    assert_eq!(body.lines().count(), 1, "{body}");
    let v: serde_json::Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
    assert_eq!(v["kind"], "manual-declare");
    assert_eq!(v["claim_id"], printed);
    assert_eq!(v["text"], "the README install line is correct");
    assert_eq!(v["severity"], "blocking");

    let status: serde_json::Value =
        serde_json::from_str(&out(&agentrec(root, &["attest", "status", "--json"]))).unwrap();
    assert_eq!(status["declared"], 1);
    assert_eq!(status["manual"]["blocking"]["unanswered"], 1);

    // An empty criterion is refused rather than declared unreadably.
    let bad = agentrec(
        root,
        &[
            "attest",
            "manual-declare",
            "--text",
            "   ",
            "--severity",
            "fyi",
        ],
    );
    assert_ne!(code(&bad), 0);
    assert_eq!(
        std::fs::read_to_string(root.join(".agentrec/attest.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1,
        "a refused declare must append nothing"
    );
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P5-2, P5-3, P5-8
// ---------------------------------------------------------------------------

/// Every verdict kind × every manual severity/answer combination, each in its
/// own root so the exit code is attributable to exactly one condition.
#[test]
fn p5_2_gate_exit_code_table() {
    // (label, events for one claim, expected exit code, expected section)
    let cases: Vec<(&str, Vec<AttestEvent>, i32, &str)> = vec![
        ("derived", vec![derive(1)], 0, "absent"),
        (
            "evidenced",
            vec![derive(1), evidence(1, None, false)],
            0,
            "absent",
        ),
        (
            "confirmed",
            vec![derive(1), verdict(1, VerdictKind::Confirmed)],
            0,
            "absent",
        ),
        (
            "claim-false",
            vec![derive(1), verdict(1, VerdictKind::ClaimFalse)],
            1,
            "blocking",
        ),
        (
            "recipe-invalid",
            vec![
                derive(1),
                verdict(
                    1,
                    VerdictKind::RecipeInvalid {
                        cause: RecipeInvalidCause::Missing,
                    },
                ),
            ],
            0,
            "advisory",
        ),
        (
            "flaky",
            vec![derive(1), verdict(1, VerdictKind::FlakyObservation)],
            0,
            "advisory",
        ),
        (
            "manual blocking unanswered",
            vec![declare(1, ManualSeverity::Blocking)],
            1,
            "blocking",
        ),
        (
            "manual blocking yes",
            vec![
                declare(1, ManualSeverity::Blocking),
                human(1, HumanAnswer::Yes),
            ],
            0,
            "absent",
        ),
        (
            "manual blocking no",
            vec![
                declare(1, ManualSeverity::Blocking),
                human(1, HumanAnswer::No),
            ],
            1,
            "blocking",
        ),
        (
            "manual blocking skip",
            vec![
                declare(1, ManualSeverity::Blocking),
                human(1, HumanAnswer::Skip),
            ],
            1,
            "blocking",
        ),
        (
            "manual fyi unanswered",
            vec![declare(1, ManualSeverity::Fyi)],
            0,
            "absent",
        ),
        (
            "manual fyi yes",
            vec![declare(1, ManualSeverity::Fyi), human(1, HumanAnswer::Yes)],
            0,
            "absent",
        ),
        (
            "manual fyi no",
            vec![declare(1, ManualSeverity::Fyi), human(1, HumanAnswer::No)],
            0,
            "absent",
        ),
        (
            "manual fyi skip",
            vec![declare(1, ManualSeverity::Fyi), human(1, HumanAnswer::Skip)],
            0,
            "absent",
        ),
        // Both halves of the rule on ONE claim: the AC says it is counted
        // once, and only this row can falsify that.
        (
            "claim-false AND blocking manual",
            vec![
                declare(1, ManualSeverity::Blocking),
                verdict(1, VerdictKind::ClaimFalse),
            ],
            1,
            "blocking",
        ),
        (
            "stale only",
            vec![
                derive(1),
                AttestEvent::Stale {
                    ts: 400,
                    claim_id: cid(1),
                    cause: StaleCause::FileWrite {
                        path: "cli/src/x.rs".into(),
                    },
                },
            ],
            0,
            "advisory",
        ),
    ];

    let id = cid(1).to_string();
    for (label, events, expected_code, section) in cases {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        seed(root, &events);
        let o = agentrec(root, &["attest", "gate"]);
        let text = out(&o);
        assert_eq!(code(&o), expected_code, "{label}: gate said\n{text}");
        // The tally never double-counts a claim, whichever half caught it.
        let want_count = if expected_code == 1 { 1 } else { 0 };
        assert!(
            text.contains(&format!("({want_count} blocking)")),
            "{label}: expected exactly {want_count} blocking in\n{text}"
        );

        let json: serde_json::Value =
            serde_json::from_str(&out(&agentrec(root, &["attest", "gate", "--json"]))).unwrap();
        let in_blocking = json["blocking"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["claim_id"] == id);
        let in_advisory = json["advisory"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["claim_id"] == id);
        match section {
            "blocking" => assert!(in_blocking && !in_advisory, "{label}: {json}"),
            "advisory" => assert!(in_advisory && !in_blocking, "{label}: {json}"),
            _ => assert!(!in_blocking && !in_advisory, "{label}: {json}"),
        }
    }
}

#[test]
fn p5_3_recipe_invalid_and_flaky_never_block() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Every recipe-invalid cause plus a flaky claim, all at once.
    let mut events = vec![derive(1), derive(2), derive(3), derive(4), derive(5)];
    for (n, cause) in [
        (1u8, RecipeInvalidCause::Build),
        (2, RecipeInvalidCause::Missing),
        (3, RecipeInvalidCause::Ignored),
        (4, RecipeInvalidCause::Harness),
    ] {
        events.push(verdict(n, VerdictKind::RecipeInvalid { cause }));
    }
    events.push(verdict(5, VerdictKind::FlakyObservation));
    seed(root, &events);

    let o = agentrec(root, &["attest", "gate"]);
    assert_eq!(code(&o), 0, "{}", out(&o));
    let text = out(&o);
    assert!(text.contains("attest gate: PASS (0 blocking)"), "{text}");
    for cause in ["build", "missing", "ignored", "harness"] {
        assert!(
            text.contains(&format!("recipe-invalid({cause})")),
            "cause {cause} must be surfaced: {text}"
        );
    }
    assert!(text.contains("flaky:"), "{text}");
}

#[test]
fn p5_4_gate_json_matches_the_text_verdict() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed(
        root,
        &[
            derive(1),
            verdict(1, VerdictKind::ClaimFalse),
            derive(2),
            verdict(2, VerdictKind::FlakyObservation),
        ],
    );
    let text = agentrec(root, &["attest", "gate"]);
    let json_out = agentrec(root, &["attest", "gate", "--json"]);
    assert_eq!(code(&text), 1);
    assert_eq!(code(&json_out), 1, "--json must exit the same way");
    let json: serde_json::Value = serde_json::from_str(&out(&json_out)).unwrap();
    assert_eq!(json["pass"], false);
    assert_eq!(json["blocking"].as_array().unwrap().len(), 1);
    assert_eq!(json["advisory"].as_array().unwrap().len(), 1);
    let body = out(&text);
    for id in [cid(1).to_string(), cid(2).to_string()] {
        assert!(body.contains(&id), "{id} missing from text form: {body}");
    }
}

#[test]
fn p5_8_fyi_is_absent_from_review_and_gate_but_present_in_report() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed(root, &[declare(1, ManualSeverity::Fyi)]);
    let id = cid(1).to_string();

    // The gate lists it in NEITHER section, and passes.
    let gate_json: serde_json::Value =
        serde_json::from_str(&out(&agentrec(root, &["attest", "gate", "--json"]))).unwrap();
    assert_eq!(gate_json["pass"], true);
    assert!(gate_json["blocking"].as_array().unwrap().is_empty());
    assert!(
        gate_json["advisory"].as_array().unwrap().is_empty(),
        "fyi must not be an advisory line either: {gate_json}"
    );
    let gate_text = agentrec(root, &["attest", "gate"]);
    assert_eq!(code(&gate_text), 0);
    assert!(
        !out(&gate_text).contains(&id),
        "fyi must not appear in gate output at all: {}",
        out(&gate_text)
    );

    // Review offers no card and never reads stdin: piping an answer must
    // append nothing, which is what proves it did not prompt.
    let cards: serde_json::Value =
        serde_json::from_str(&out(&agentrec(root, &["attest", "review", "--json"]))).unwrap();
    assert!(
        cards.as_array().unwrap().is_empty(),
        "fyi must not be a card: {cards}"
    );
    let before = std::fs::read_to_string(root.join(".agentrec/attest.jsonl")).unwrap();
    let review = agentrec_stdin(root, &["attest", "review"], "y\n");
    assert_eq!(code(&review), 0);
    assert!(!out(&review).contains(&id), "{}", out(&review));
    assert_eq!(
        std::fs::read_to_string(root.join(".agentrec/attest.jsonl")).unwrap(),
        before,
        "review must not append an answer for an fyi claim"
    );

    // Recorded, not dropped: report and status still carry it.
    let report: serde_json::Value =
        serde_json::from_str(&out(&agentrec(root, &["attest", "report", "--json"]))).unwrap();
    assert_eq!(report["claims"][0]["claim_id"], id);
    assert_eq!(report["claims"][0]["severity"], "fyi");
    let status: serde_json::Value =
        serde_json::from_str(&out(&agentrec(root, &["attest", "status", "--json"]))).unwrap();
    assert_eq!(status["manual"]["fyi"]["unanswered"], 1);
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P5-5, P5-6, P5-7
// ---------------------------------------------------------------------------

fn human_events(root: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.join(".agentrec/attest.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["kind"] == "human")
        .collect()
}

#[test]
fn p5_5_review_scripted_stdin_appends_three_human_events() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed(
        root,
        &[
            declare(1, ManualSeverity::Blocking),
            declare(2, ManualSeverity::Blocking),
            declare(3, ManualSeverity::Blocking),
            // Card 1 has a joined turn, so its diff pointer is real.
            evidence(1, Some("t_01ARZ3NDEKTSV4RRFFQ69G5FAV"), true),
        ],
    );

    let note = "the install line still says 0.1.0  # trailing spaces kept  ";
    let o = agentrec_stdin(root, &["attest", "review"], &format!("y\nn\n{note}\ns\n"));
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));

    let text = out(&o);
    assert!(
        text.contains("diff: agentrec diff t_01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        "the card must carry a diff pointer, not a diff: {text}"
    );
    assert!(text.contains("(dirty tree)"), "{text}");

    let humans = human_events(root);
    assert_eq!(humans.len(), 3, "{humans:?}");
    assert_eq!(humans[0]["claim_id"], cid(1).to_string());
    assert_eq!(humans[0]["answer"], "yes");
    assert_eq!(humans[1]["claim_id"], cid(2).to_string());
    assert_eq!(humans[1]["answer"], "no");
    assert_eq!(
        humans[1]["note"], note,
        "the note must be stored VERBATIM, trailing spaces included"
    );
    assert_eq!(humans[2]["claim_id"], cid(3).to_string());
    assert_eq!(humans[2]["answer"], "skip");
}

#[test]
fn p5_6_skip_reappears_next_run_and_no_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed(
        root,
        &[
            declare(1, ManualSeverity::Blocking),
            declare(2, ManualSeverity::Blocking),
            declare(3, ManualSeverity::Blocking),
        ],
    );

    // `s` on card 1 must not re-ask inside this run: three answers consume
    // three cards and the command ends.
    let o = agentrec_stdin(root, &["attest", "review"], "s\ny\nn\nrejected\n");
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(human_events(root).len(), 3);

    // Next run: only the skipped one is a card again. `yes` is settled and
    // `no` is a recorded rejection, deliberately not re-asked.
    let cards: serde_json::Value =
        serde_json::from_str(&out(&agentrec(root, &["attest", "review", "--json"]))).unwrap();
    let ids: Vec<String> = cards
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["claim_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec![cid(1).to_string()], "{cards}");

    // ...and the rejected one still blocks the gate.
    let gate = agentrec(root, &["attest", "gate"]);
    assert_eq!(code(&gate), 1, "{}", out(&gate));
    assert!(
        out(&gate).contains("blocking manual criterion rejected"),
        "{}",
        out(&gate)
    );
}

#[test]
fn p5_7_review_eof_and_json_never_append() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let seeded = [
        declare(1, ManualSeverity::Blocking),
        declare(2, ManualSeverity::Blocking),
    ];
    seed(root, &seeded);
    let before = std::fs::read_to_string(root.join(".agentrec/attest.jsonl")).unwrap();

    // Immediate EOF: both cards LISTED, exit 0, nothing appended.
    let o = agentrec_stdin(root, &["attest", "review"], "");
    assert_eq!(code(&o), 0);
    let text = out(&o);
    assert!(
        text.contains(&cid(1).to_string()) && text.contains(&cid(2).to_string()),
        "{text}"
    );
    assert!(text.contains("2 card(s) left unanswered"), "{text}");
    assert_eq!(
        std::fs::read_to_string(root.join(".agentrec/attest.jsonl")).unwrap(),
        before
    );

    // `--json` never prompts and never appends.
    let j = agentrec(root, &["attest", "review", "--json"]);
    assert_eq!(code(&j), 0);
    assert_eq!(
        std::fs::read_to_string(root.join(".agentrec/attest.jsonl")).unwrap(),
        before
    );

    // EOF where the note was expected is an EMPTY note, not a panic.
    let o = agentrec_stdin(root, &["attest", "review"], "n\n");
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    let humans = human_events(root);
    assert_eq!(humans.len(), 1, "{humans:?}");
    assert_eq!(humans[0]["answer"], "no");
    assert_eq!(humans[0]["note"], "");
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P5-10 — `--range`
// ---------------------------------------------------------------------------

fn git(root: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git")
}

#[test]
fn p5_10_range_filters_by_commit_time() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "."]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "t"]);
    std::fs::write(root.join("a.txt"), "one").unwrap();
    git(root, &["add", "-A"]);
    // Fixed committer dates, so the window is exact rather than racing the
    // clock: base at 1_000_000 s, head at 2_000_000 s (unix).
    let mut c = Command::new("git");
    c.args(["commit", "-qm", "base"])
        .current_dir(root)
        .env("GIT_COMMITTER_DATE", "@1000000 +0000")
        .env("GIT_AUTHOR_DATE", "@1000000 +0000");
    assert!(c.output().unwrap().status.success());
    let base = String::from_utf8_lossy(&git(root, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    std::fs::write(root.join("a.txt"), "two").unwrap();
    git(root, &["add", "-A"]);
    let mut c = Command::new("git");
    c.args(["commit", "-qm", "head"])
        .current_dir(root)
        .env("GIT_COMMITTER_DATE", "@2000000 +0000")
        .env("GIT_AUTHOR_DATE", "@2000000 +0000");
    assert!(c.output().unwrap().status.success());
    let head = String::from_utf8_lossy(&git(root, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();

    // Three claims: before the window, inside it, after it (ms, not s).
    seed(
        root,
        &[
            AttestEvent::ManualDeclare {
                ts: 999_000 * 1000,
                claim_id: cid(1),
                text: "before".into(),
                severity: ManualSeverity::Fyi,
            },
            AttestEvent::ManualDeclare {
                ts: 1_500_000 * 1000,
                claim_id: cid(2),
                text: "inside".into(),
                severity: ManualSeverity::Fyi,
            },
            AttestEvent::ManualDeclare {
                ts: 2_500_000 * 1000,
                claim_id: cid(3),
                text: "after".into(),
                severity: ManualSeverity::Fyi,
            },
        ],
    );

    let all: serde_json::Value =
        serde_json::from_str(&out(&agentrec(root, &["attest", "report", "--json"]))).unwrap();
    assert_eq!(all["claims"].as_array().unwrap().len(), 3);
    assert!(all["range"].is_null());

    let range = format!("{base}..{head}");
    let filtered: serde_json::Value = serde_json::from_str(&out(&agentrec(
        root,
        &["attest", "report", "--json", "--range", &range],
    )))
    .unwrap();
    let ids: Vec<&str> = filtered["claims"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["claim_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![cid(2).to_string()], "{filtered}");
    assert_eq!(filtered["range"], range);

    // The markdown form discloses that the filter is a time window.
    let md = out(&agentrec(root, &["attest", "report", "--range", &range]));
    assert!(md.contains("A time window, not a causal one."), "{md}");
}

#[test]
fn p5_10_range_rejects_bad_input() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "."]);
    seed(root, &[declare(1, ManualSeverity::Fyi)]);

    for bad in [
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef..HEAD",
        "HEAD..",
        "..HEAD",
        "HEAD...HEAD",
        "HEAD",
    ] {
        let o = agentrec(root, &["attest", "report", "--range", bad]);
        assert_ne!(code(&o), 0, "--range {bad:?} must be an error");
    }
}

// ---------------------------------------------------------------------------
// AC-ATTEST-P5-11, P5-12
// ---------------------------------------------------------------------------

#[test]
fn p5_11_attest_is_visible_and_lists_every_verb() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let top = out(&agentrec(root, &["--help"]));
    assert!(
        top.contains("attest"),
        "attest must no longer be hidden: {top}"
    );
    // A discriminating assertion, not a negative one: clap shows only the
    // FIRST line of a doc comment in the subcommand list, and the old
    // "Hidden until Phase 5" text was on line 2 — so asserting its absence
    // would have passed before the unhide too. The new first line is only
    // present after it.
    assert!(
        top.contains("Claims derived from tests"),
        "the unhidden summary line must be in --help: {top}"
    );

    let sub = out(&agentrec(root, &["attest", "--help"]));
    for verb in [
        "status",
        "derive",
        "run",
        "verify",
        "coverage",
        "manual-declare",
        "review",
        "gate",
        "report",
    ] {
        assert!(sub.contains(verb), "attest --help omits {verb}: {sub}");
    }
}

#[test]
fn p5_12_status_json_is_additive() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed(
        root,
        &[
            derive(1),
            verdict(
                1,
                VerdictKind::RecipeInvalid {
                    cause: RecipeInvalidCause::Ignored,
                },
            ),
            declare(2, ManualSeverity::Blocking),
            declare(3, ManualSeverity::Fyi),
            human(3, HumanAnswer::Yes),
        ],
    );
    let v: serde_json::Value =
        serde_json::from_str(&out(&agentrec(root, &["attest", "status", "--json"]))).unwrap();

    // Every key Phase 3 shipped, with the value this fixture implies. A
    // repurposed name or a changed meaning reds here.
    let expected_pre_existing = [
        ("claims", serde_json::json!(3)),
        ("derived", serde_json::json!(0)),
        ("declared", serde_json::json!(1)),
        ("evidenced", serde_json::json!(0)),
        ("confirmed", serde_json::json!(0)),
        ("claim_false", serde_json::json!(0)),
        ("recipe_invalid", serde_json::json!(1)),
        ("flaky", serde_json::json!(0)),
        ("human", serde_json::json!(1)),
        ("stale", serde_json::json!(0)),
        ("dev_loop_only", serde_json::json!(0)),
        ("unparsed_lines", serde_json::json!(0)),
        ("unknown_kind_lines", serde_json::json!(0)),
    ];
    for (key, want) in &expected_pre_existing {
        assert_eq!(v[key], *want, "pre-existing key {key} changed: {v}");
    }

    // The additions, and nothing else.
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    let mut expected: Vec<&str> = expected_pre_existing.iter().map(|(k, _)| *k).collect();
    expected.push("recipe_invalid_causes");
    expected.push("manual");
    expected.sort_unstable();
    let mut keys = keys;
    keys.sort_unstable();
    assert_eq!(keys, expected, "status --json key set drifted");

    assert_eq!(v["recipe_invalid_causes"]["ignored"], 1);
    assert_eq!(v["manual"]["blocking"]["unanswered"], 1);
    assert_eq!(v["manual"]["blocking"]["answered"], 0);
    assert_eq!(v["manual"]["fyi"]["answered"], 1);
}
