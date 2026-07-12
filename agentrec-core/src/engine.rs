//! Pure turn-boundary engine (no fs access) — the v0.2 capture design:
//! start/stop bracketing with retroactive merge, git-turn classification,
//! quiet-window fallback for unattributed activity, crash rule, fold window.
//! Retroactive merge respects append-only logs: absorbed bare turns stay in
//! the log; the rich turn lists them in `merges` and consumers drop them.

use crate::{FOLD_WINDOW_MS, GIT_SETTLE_MS, MAX_BRACKET_MS, QUIET_MS};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// One observed file change, fingerprinted by the scanner (hashes, not bytes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChangeObs {
    pub path: String,
    pub before_hash: Option<String>,
    pub snapshotted: bool, // false = over cap / unreadable (content not captured)
    pub withheld: bool,    // secret-pattern file
    pub baseline_unknown: bool, // first seen post-change; original unrecoverable
    #[serde(default)]
    pub deleted: bool, // file absent at observation — a true delete, NOT
                           // merely "no snapshot captured" (distinguishes
                           // `op:delete` from an over-cap `op:modify`)
}

#[derive(Clone, Debug, PartialEq)]
enum Source {
    Bracket,
    Git,
    Quiet,
}

#[derive(Clone, Debug)]
struct OpenTurn {
    source: Source,
    tool: Option<String>,
    prompt: Option<String>,
    session: Option<String>,
    opened_at: u64,
    files: Vec<ChangeObs>,
    /// Reserved at open time rather than generated lazily at close (as every
    /// other turn id used to be) — this lets an external process learn the id
    /// a still-open turn WILL close under, before it closes. Motivating case:
    /// the daemon's memory-candidate ingestion (PROTOCOL.md memory-candidate
    /// design, "source_turns links to the enclosing turn") needs to stamp
    /// `source_turns` on a candidate that arrives mid-bracket. Safe because
    /// exactly one turn is ever open per root (strictly serialized
    /// open→close→open), so reserving here doesn't change ULID ordering
    /// across turns, only shifts one id's embedded timestamp a few seconds
    /// earlier; every record still carries explicit `started`/`ended`.
    id: String,
}

/// A closed turn, engine-level (unix-ms times; persistence converts).
#[derive(Clone, Debug)]
pub struct ClosedTurn {
    pub id: String,
    pub grade: &'static str,    // "rich" | "bare"
    pub boundary: &'static str, // "bracket" | "stop-only" | "git" | "quiet" | "timeout"
    pub truncated: bool,
    pub tool: Option<String>,
    pub prompt: Option<String>,
    pub session: Option<String>,
    pub opened_at: u64,
    pub closed_at: u64,
    pub files: Vec<ChangeObs>,
    /// Ids of previously-closed bare turns absorbed into this rich turn.
    pub merges: Vec<String>,
}

/// A daemon-persistable view of the open turn (AC B2 crash journal). Source is
/// stringified so the wire form is stable; times are engine ms.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenSnapshot {
    pub source: String, // "bracket" | "git" | "quiet"
    pub tool: Option<String>,
    pub prompt: Option<String>,
    pub session: Option<String>,
    pub opened_at: u64,
    pub last_change_at: u64,
    pub files: Vec<ChangeObs>,
    /// The id this turn will close under (reserved at open, per `OpenTurn`).
    /// `#[serde(default)]` with a fresh id keeps a crash journal written by a
    /// pre-this-change daemon binary still parseable across an upgrade.
    #[serde(default = "crate::id::turn_id")]
    pub id: String,
}

pub struct TurnEngine {
    open: Option<OpenTurn>,
    last_change_at: Option<u64>,
    git_window_until: u64,
    /// Bare turns closed since the last rich close, eligible for folding.
    recent_bare: Vec<ClosedTurn>,
    last_rich_closed_at: u64,
    /// Turns closed out-of-band from the normal `tick`/`observe_*` return
    /// path (currently only C2's git-transition close of a long-open Quiet
    /// turn). Drained by the very next `tick` call, which every caller
    /// already persists — this keeps every close returned through the one
    /// path callers already handle, instead of adding a second one they'd
    /// have to remember to wire up.
    pending_closed: Vec<ClosedTurn>,
    id_gen: fn() -> String,
}

impl Default for TurnEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnEngine {
    pub fn new() -> Self {
        TurnEngine {
            open: None,
            last_change_at: None,
            git_window_until: 0,
            recent_bare: vec![],
            last_rich_closed_at: 0,
            pending_closed: vec![],
            id_gen: crate::id::turn_id,
        }
    }

    pub fn has_open_turn(&self) -> bool {
        self.open.is_some()
    }

    /// The id the currently-open turn will close under, if any (reserved at
    /// open time — see `OpenTurn::id`). Lets an in-flight process — the
    /// daemon's memory-candidate ingestion is the motivating case — stamp
    /// `source_turns` on a candidate that arrives mid-bracket, before the
    /// turn actually closes.
    pub fn open_turn_id(&self) -> Option<&str> {
        self.open.as_ref().map(|o| o.id.as_str())
    }

    /// Serializable view of the currently-open turn, for the daemon's crash
    /// journal (AC B2). None when no turn is open. Times are engine ms; the
    /// daemon converts them to wall clock before persisting the journal.
    pub fn snapshot_open(&self) -> Option<OpenSnapshot> {
        let open = self.open.as_ref()?;
        Some(OpenSnapshot {
            source: match open.source {
                Source::Bracket => "bracket",
                Source::Git => "git",
                Source::Quiet => "quiet",
            }
            .to_string(),
            tool: open.tool.clone(),
            prompt: open.prompt.clone(),
            session: open.session.clone(),
            opened_at: open.opened_at,
            last_change_at: self.last_change_at.unwrap_or(open.opened_at),
            files: open.files.clone(),
            id: open.id.clone(),
        })
    }

    /// Scanner-observed file changes at `now`. Opens a turn when none is open:
    /// grade depends on whether a git ref-change window is active. Only the
    /// first observation of a path per turn records its `before`.
    pub fn observe_changes(&mut self, now: u64, changes: &[ChangeObs]) {
        if changes.is_empty() {
            return;
        }
        self.last_change_at = Some(now);
        if self.open.is_none() {
            // `git_window_until > 0` guards the uninitialized state: without it,
            // the first change at `now == 0` matches the default window and is
            // misclassified as a git turn (the monotonic daemon clock starts ~0).
            let (source, tool) = if self.git_window_until > 0 && now <= self.git_window_until {
                (Source::Git, Some("git".to_string()))
            } else {
                (Source::Quiet, None)
            };
            self.open = Some(OpenTurn {
                source,
                tool,
                prompt: None,
                session: None,
                opened_at: now,
                files: vec![],
                id: (self.id_gen)(),
            });
        }
        let open = self.open.as_mut().expect("just ensured");
        for change in changes {
            match open.files.iter_mut().find(|f| f.path == change.path) {
                None => open.files.push(change.clone()),
                Some(existing) => {
                    // `before_hash`/`baseline_unknown` describe the turn's
                    // true original state — captured once, from the first
                    // observation (C4). Every other field is terminal state
                    // (does the file exist now? was content captured?) and
                    // must track the LATEST observation, or a
                    // delete-then-recreate freezes `deleted:true` on a file
                    // that exists again (and modify-then-delete freezes
                    // `deleted:false` on a file that's gone).
                    existing.deleted = change.deleted;
                    existing.snapshotted = change.snapshotted;
                    existing.withheld = change.withheld;
                }
            }
        }
    }

    /// A `.git/HEAD`/index/ref transition at `now`: classify surrounding
    /// activity as a git operation, not agent/unknown work. A bracket in
    /// progress is left alone — an agent running git is agent work.
    ///
    /// A long-running open Quiet turn (continuous human saves keep the quiet
    /// window from ever elapsing, C2) is NOT converted wholesale into a git
    /// turn — that would fabricate attribution in the other direction (a
    /// 400-file checkout logged as a human bare turn, or a genuinely
    /// long-running human turn logged as `tool:"git"`). Instead it closes as
    /// bare at its last real mutation, and the git burst that follows opens
    /// its own fresh `tool:"git"` turn via the usual git-window check. The
    /// closed bare turn is queued in `pending_closed` and surfaces through
    /// the very next `tick` call rather than this function's own return
    /// value — every caller already persists whatever `tick` returns, so
    /// this can't be forgotten the way a new return value could be.
    pub fn observe_git_change(&mut self, now: u64) {
        self.git_window_until = now + GIT_SETTLE_MS;
        if matches!(&self.open, Some(o) if o.source == Source::Quiet) {
            let mut open = self.open.take().expect("checked above");
            if now.saturating_sub(open.opened_at) <= GIT_SETTLE_MS + QUIET_MS {
                open.source = Source::Git;
                open.tool = Some("git".to_string());
                self.open = Some(open);
            } else {
                let close_at = self.last_change_at.unwrap_or(open.opened_at);
                let turn = self.finish(open, close_at, "quiet", "bare", false);
                self.recent_bare.push(turn.clone());
                self.pending_closed.push(turn);
            }
        }
    }

    /// Start signal (e.g. Claude Code UserPromptSubmit): opens a bracket.
    /// Any open non-bracket turn closes first (pre-agent activity); an open
    /// bracket from any tool closes truncated (one open turn per root, D6).
    pub fn observe_start(
        &mut self,
        now: u64,
        tool: &str,
        prompt: Option<String>,
        session: Option<String>,
    ) -> Vec<ClosedTurn> {
        let mut closed = vec![];
        if let Some(open) = self.open.take() {
            let turn = match open.source {
                Source::Quiet => self.finish(open, now, "quiet", "bare", false),
                Source::Git => self.finish(open, now, "git", "rich", false),
                Source::Bracket => self.finish(open, now, "timeout", "rich", true),
            };
            closed.push(turn);
        }
        self.open = Some(OpenTurn {
            source: Source::Bracket,
            tool: Some(tool.to_string()),
            prompt,
            session,
            opened_at: now,
            files: vec![],
            id: (self.id_gen)(),
        });
        closed
    }

    /// Stop signal. Bracketed: closes the bracket rich, folding any bare turns
    /// that closed inside it (daemon restarts can produce those). Unbracketed
    /// (stop-only emitter): converts the open unattributed turn — or recently
    /// closed bare turns within the fold window — into the rich turn.
    pub fn observe_stop(
        &mut self,
        now: u64,
        tool: &str,
        prompt_fallback: Option<String>,
        session: Option<String>,
    ) -> Vec<ClosedTurn> {
        let mut closed = vec![];
        let open = self.open.take();
        match open {
            Some(open) if open.source == Source::Bracket && open.tool.as_deref() == Some(tool) => {
                let mut turn = self.finish(open, now, "bracket", "rich", false);
                if turn.prompt.is_none() {
                    turn.prompt = prompt_fallback;
                }
                if turn.session.is_none() {
                    turn.session = session;
                }
                self.fold_recent_bares(&mut turn);
                self.last_rich_closed_at = now;
                closed.push(turn);
            }
            other => {
                // Close whatever was open on its own terms first.
                if let Some(open) = other {
                    let turn = match open.source {
                        Source::Git => self.finish(open, now, "git", "rich", false),
                        Source::Bracket => self.finish(open, now, "timeout", "rich", true),
                        Source::Quiet => {
                            // Stop-only emitter: the open unattributed turn IS
                            // this tool's turn — attribute and close rich.
                            let mut turn = self.finish(open, now, "stop-only", "rich", false);
                            turn.tool = Some(tool.to_string());
                            turn.prompt = prompt_fallback.clone();
                            turn.session = session.clone();
                            self.fold_recent_bares(&mut turn);
                            self.last_rich_closed_at = now;
                            closed.push(turn);
                            return closed;
                        }
                    };
                    closed.push(turn);
                }
                // No open unattributed work: fold recent bares, or record an
                // empty rich turn (the agent turn happened; maybe it only read).
                let mut turn = ClosedTurn {
                    id: (self.id_gen)(),
                    grade: "rich",
                    boundary: "stop-only",
                    truncated: false,
                    tool: Some(tool.to_string()),
                    prompt: prompt_fallback,
                    session,
                    opened_at: now,
                    closed_at: now,
                    files: vec![],
                    merges: vec![],
                };
                self.fold_recent_bares(&mut turn);
                self.last_rich_closed_at = now;
                closed.push(turn);
            }
        }
        closed
    }

    /// Clock tick: quiet-window close for unattributed turns, settle close for
    /// git turns, crash-rule timeout for brackets. Brackets are never closed
    /// by the quiet window (PROTOCOL §4 suppression). Also drains any turn
    /// closed out-of-band since the last tick (C2's git-transition close),
    /// so callers persist it through the same path as everything else.
    pub fn tick(&mut self, now: u64) -> Vec<ClosedTurn> {
        self.recent_bare
            .retain(|t| now.saturating_sub(t.closed_at) <= FOLD_WINDOW_MS);
        let mut out: Vec<ClosedTurn> = std::mem::take(&mut self.pending_closed);
        let Some(open) = self.open.as_ref() else {
            return out;
        };
        let last_change = self.last_change_at.unwrap_or(open.opened_at);
        let close = match open.source {
            Source::Quiet => now.saturating_sub(last_change) > QUIET_MS,
            Source::Git => now.saturating_sub(last_change) > GIT_SETTLE_MS,
            Source::Bracket => now.saturating_sub(open.opened_at) > MAX_BRACKET_MS,
        };
        if !close {
            return out;
        }
        let open = self.open.take().expect("checked above");
        let turn = match open.source {
            Source::Quiet => {
                let turn = self.finish(open, now, "quiet", "bare", false);
                self.recent_bare.push(turn.clone());
                turn
            }
            Source::Git => self.finish(open, now, "git", "rich", false),
            // C5: close at the last real mutation, not the tick's `now` — a
            // crash-rule timeout fires long after the bracket went quiet
            // (MAX_BRACKET_MS+), and using `now` here would glue that entire
            // dead gap onto the turn's displayed span. `truncated: true`
            // already marks this as a synthetic, not-a-real-stop close.
            Source::Bracket => self.finish(open, last_change, "timeout", "rich", true),
        };
        out.push(turn);
        out
    }

    /// Force-close the open turn at `now`, regardless of source — used on clean
    /// daemon shutdown (an operator stopping mid-turn). Bracket → truncated rich
    /// (keep attribution), git → rich, quiet → bare. `now` is the real close
    /// time, so timestamps stay sane (unlike an inflated tick threshold).
    pub fn force_close(&mut self, now: u64) -> Vec<ClosedTurn> {
        let Some(open) = self.open.take() else {
            return vec![];
        };
        let turn = match open.source {
            Source::Quiet => self.finish(open, now, "quiet", "bare", false),
            Source::Git => self.finish(open, now, "git", "rich", false),
            Source::Bracket => self.finish(open, now, "timeout", "rich", true),
        };
        vec![turn]
    }

    fn fold_recent_bares(&mut self, turn: &mut ClosedTurn) {
        let cutoff = turn.closed_at.saturating_sub(FOLD_WINDOW_MS);
        let opened_at = turn.opened_at;
        // A bracket's `opened_at` comes from a real start signal, so activity
        // that closed bare BEFORE it is provably pre-agent and must never be
        // folded in (C1) — otherwise a pre-bracket human save gets fabricated
        // attribution and undo would revert the human's file. Stop-only turns
        // have no such boundary (their `opened_at` is just where the
        // fragment chain happened to break), so every bare within the fold
        // window stays eligible there, matching the retroactive-merge design.
        let is_bracket = turn.boundary == "bracket";
        let mut eligible: Vec<ClosedTurn> = self
            .recent_bare
            .drain(..)
            .filter(|b| {
                b.closed_at >= cutoff
                    && b.closed_at >= self.last_rich_closed_at
                    && (!is_bracket || b.closed_at >= opened_at)
            })
            .collect();
        // Fold oldest fragment first: for a path touched by more than one
        // fragment, the earliest fragment's before_hash is the true original
        // (C3) — a later occurrence of the same path (a later bare, or the
        // rich turn's own already-populated entry) must never clobber it.
        eligible.sort_by_key(|b| b.opened_at);
        let mut before_claimed: HashSet<String> = HashSet::new();
        for bare in eligible {
            turn.merges.push(bare.id.clone());
            if bare.opened_at < turn.opened_at {
                turn.opened_at = bare.opened_at;
            }
            for file in bare.files {
                if !before_claimed.insert(file.path.clone()) {
                    continue; // an earlier fragment already fixed this before
                }
                match turn.files.iter_mut().find(|f| f.path == file.path) {
                    None => turn.files.push(file),
                    Some(existing) => {
                        existing.before_hash = file.before_hash;
                        existing.baseline_unknown = file.baseline_unknown;
                    }
                }
            }
        }
    }

    fn finish(
        &mut self,
        open: OpenTurn,
        now: u64,
        boundary: &'static str,
        grade: &'static str,
        truncated: bool,
    ) -> ClosedTurn {
        self.last_change_at = None;
        ClosedTurn {
            id: open.id,
            grade,
            boundary,
            truncated,
            tool: open.tool,
            prompt: open.prompt,
            session: open.session,
            opened_at: open.opened_at,
            closed_at: now.max(open.opened_at),
            files: open.files,
            merges: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(path: &str) -> ChangeObs {
        ChangeObs {
            path: path.to_string(),
            before_hash: Some(format!("sha256:{path}")),
            snapshotted: true,
            withheld: false,
            baseline_unknown: false,
            deleted: false,
        }
    }

    // F1 fix: an open bracket suppresses the quiet window across long pauses
    // (thinking, test runs), so one agent turn stays one turn.
    #[test]
    fn bracket_suppresses_quiet_window() {
        let mut e = TurnEngine::new();
        e.observe_start(0, "claude-code", Some("fix auth".into()), None);
        e.observe_changes(1_000, &[obs("a.rs")]);
        assert!(e.tick(75_000).is_empty()); // 74s of silence: still open
        e.observe_changes(80_000, &[obs("b.rs")]);
        let closed = e.observe_stop(90_000, "claude-code", None, None);
        assert_eq!(closed.len(), 1);
        let t = &closed[0];
        assert_eq!(t.grade, "rich");
        assert_eq!(t.boundary, "bracket");
        assert_eq!(t.prompt.as_deref(), Some("fix auth"));
        assert_eq!(t.files.len(), 2);
        assert!(!t.truncated);
    }

    // F1 fix: human editor-save bursts outside any bracket close as bare —
    // unattributed activity windows, never fabricated agent turns.
    #[test]
    fn human_burst_outside_bracket_closes_bare() {
        let mut e = TurnEngine::new();
        e.observe_changes(0, &[obs("notes.md")]);
        assert!(e.tick(9_999).is_empty());
        let closed = e.tick(10_001);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].grade, "bare");
        assert_eq!(closed[0].tool, None);
    }

    // Stop-only emitters: the open unattributed turn is attributed on stop,
    // and recently-closed bare fragments fold in via `merges`.
    #[test]
    fn stop_only_attributes_open_turn_and_folds_bares() {
        let mut e = TurnEngine::new();
        // fragment 1: closed bare by quiet window (agent paused >10s mid-turn)
        e.observe_changes(0, &[obs("a.rs")]);
        let bare = e.tick(11_000);
        assert_eq!(bare[0].grade, "bare");
        let bare_id = bare[0].id.clone();
        // fragment 2: still open when the stop signal lands
        e.observe_changes(60_000, &[obs("b.rs")]);
        let closed = e.observe_stop(65_000, "codex", Some("refactor".into()), None);
        assert_eq!(closed.len(), 1);
        let t = &closed[0];
        assert_eq!(t.grade, "rich");
        assert_eq!(t.tool.as_deref(), Some("codex"));
        assert_eq!(t.merges, vec![bare_id]);
        let mut paths: Vec<&str> = t.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, vec!["a.rs", "b.rs"]);
        assert_eq!(t.opened_at, 0); // started at the earliest folded fragment
    }

    // F2 fix: a ref-change classifies the surrounding burst as a git turn.
    #[test]
    fn git_ref_change_converts_open_burst() {
        let mut e = TurnEngine::new();
        e.observe_changes(0, &[obs("x.rs"), obs("y.rs")]);
        e.observe_git_change(100); // checkout detected just after the burst began
        let closed = e.tick(100 + GIT_SETTLE_MS + 1);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].tool.as_deref(), Some("git"));
        assert_eq!(closed[0].grade, "rich");
        assert_eq!(closed[0].boundary, "git");
    }

    #[test]
    fn changes_inside_git_window_open_git_turn() {
        let mut e = TurnEngine::new();
        e.observe_git_change(1_000);
        e.observe_changes(1_500, &[obs("f1"), obs("f2")]);
        let closed = e.tick(1_500 + GIT_SETTLE_MS + 1);
        assert_eq!(closed[0].tool.as_deref(), Some("git"));
    }

    // Agent-run git stays agent work: brackets are not converted.
    #[test]
    fn git_change_leaves_bracket_alone() {
        let mut e = TurnEngine::new();
        e.observe_start(0, "claude-code", None, None);
        e.observe_changes(10, &[obs("a.rs")]);
        e.observe_git_change(20);
        let closed = e.observe_stop(1_000, "claude-code", None, None);
        assert_eq!(closed[0].tool.as_deref(), Some("claude-code"));
    }

    // Pre-agent activity: a start signal closes the open unattributed turn as
    // bare rather than absorbing pre-prompt human edits into the agent turn.
    #[test]
    fn start_closes_prior_quiet_as_bare() {
        let mut e = TurnEngine::new();
        e.observe_changes(0, &[obs("human.md")]);
        let closed = e.observe_start(5_000, "claude-code", Some("go".into()), None);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].grade, "bare");
        let stop = e.observe_stop(9_000, "claude-code", None, None);
        assert_eq!(stop[0].files.len(), 0); // human.md not attributed to agent
        assert!(stop[0].merges.is_empty()); // pre-start bare is not folded
    }

    // Crash rule: start with no stop closes truncated, keeping attribution.
    #[test]
    fn bracket_timeout_closes_truncated_rich() {
        let mut e = TurnEngine::new();
        e.observe_start(0, "claude-code", Some("long job".into()), None);
        e.observe_changes(10, &[obs("a.rs")]);
        assert!(e.tick(MAX_BRACKET_MS).is_empty());
        let closed = e.tick(MAX_BRACKET_MS + 1);
        assert_eq!(closed.len(), 1);
        assert!(closed[0].truncated);
        assert_eq!(closed[0].grade, "rich");
        assert_eq!(closed[0].tool.as_deref(), Some("claude-code"));
    }

    // D6: one open turn per root — a second start closes the first truncated.
    #[test]
    fn second_start_closes_first_bracket_truncated() {
        let mut e = TurnEngine::new();
        e.observe_start(0, "claude-code", None, None);
        e.observe_changes(10, &[obs("a.rs")]);
        let closed = e.observe_start(1_000, "codex", None, None);
        assert_eq!(closed.len(), 1);
        assert!(closed[0].truncated);
        assert_eq!(closed[0].tool.as_deref(), Some("claude-code"));
        let stop = e.observe_stop(2_000, "codex", None, None);
        assert_eq!(stop[0].tool.as_deref(), Some("codex"));
    }

    // A stop with nothing observed still records the (empty, rich) turn.
    #[test]
    fn empty_stop_records_prompt_only_turn() {
        let mut e = TurnEngine::new();
        let closed = e.observe_stop(1_000, "claude-code", Some("read the code".into()), None);
        assert_eq!(closed.len(), 1);
        assert!(closed[0].files.is_empty());
        assert_eq!(closed[0].grade, "rich");
    }

    #[test]
    fn before_captured_once_per_turn() {
        let mut e = TurnEngine::new();
        e.observe_start(0, "claude-code", None, None);
        let first = ChangeObs {
            before_hash: Some("sha256:v0".into()),
            ..obs("a.rs")
        };
        let second = ChangeObs {
            before_hash: Some("sha256:v1".into()),
            ..obs("a.rs")
        };
        e.observe_changes(1, &[first]);
        e.observe_changes(2, &[second]);
        let closed = e.observe_stop(3, "claude-code", None, None);
        assert_eq!(closed[0].files.len(), 1);
        assert_eq!(closed[0].files[0].before_hash.as_deref(), Some("sha256:v0"));
    }

    // B2: the open turn is snapshottable for the crash journal, carrying source,
    // attribution, times, and files; None once nothing is open.
    #[test]
    fn snapshot_open_captures_bracket_state() {
        let mut e = TurnEngine::new();
        assert!(e.snapshot_open().is_none());
        e.observe_start(
            1_000,
            "claude-code",
            Some("fix auth".into()),
            Some("s1".into()),
        );
        e.observe_changes(1_500, &[obs("a.rs")]);
        let snap = e.snapshot_open().expect("turn is open");
        assert_eq!(snap.source, "bracket");
        assert_eq!(snap.tool.as_deref(), Some("claude-code"));
        assert_eq!(snap.prompt.as_deref(), Some("fix auth"));
        assert_eq!(snap.opened_at, 1_000);
        assert_eq!(snap.last_change_at, 1_500);
        assert_eq!(snap.files.len(), 1);
        // round-trips through the wire form the daemon persists
        let json = serde_json::to_string(&snap).unwrap();
        let back: OpenSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.files[0].path, "a.rs");
        e.observe_stop(2_000, "claude-code", None, None);
        assert!(e.snapshot_open().is_none());
    }

    // The id `open_turn_id()` reports mid-bracket must be the SAME id the
    // turn actually closes under — a caller (the daemon's memory-candidate
    // ingestion) that reads it before the stop signal must not get a
    // dangling reference once the turn lands in the log.
    #[test]
    fn open_turn_id_matches_the_id_it_closes_under() {
        let mut e = TurnEngine::new();
        assert_eq!(e.open_turn_id(), None, "nothing open yet");

        e.observe_start(1_000, "claude-code", None, Some("s1".into()));
        let reserved = e.open_turn_id().expect("bracket is open").to_string();
        assert!(!reserved.is_empty());

        let closed = e.observe_stop(2_000, "claude-code", None, Some("s1".into()));
        assert_eq!(closed.len(), 1);
        assert_eq!(
            closed[0].id, reserved,
            "the id reserved at open must be the id the closed turn actually carries"
        );
        assert_eq!(e.open_turn_id(), None, "nothing open after close");
    }

    // Bare fragments older than the fold window are not absorbed.
    #[test]
    fn stale_bares_not_folded() {
        let mut e = TurnEngine::new();
        e.observe_changes(0, &[obs("old.rs")]);
        e.tick(11_000); // closed bare at 11s
        let much_later = FOLD_WINDOW_MS + 100_000;
        let closed = e.observe_stop(much_later, "codex", None, None);
        assert!(closed[0].merges.is_empty());
        assert!(closed[0].files.is_empty());
    }

    // C1: a bare turn that closed BEFORE a bracket ever opened is provably
    // pre-agent activity (the bracket's `opened_at` comes from a real start
    // signal) and must never fold into the bracket's rich turn.
    #[test]
    fn fold_excludes_bare_closed_before_bracket_opened() {
        let mut e = TurnEngine::new();
        // human vim save at t=10s, closes bare via quiet window at t=21s
        e.observe_changes(10_000, &[obs("human-notes.md")]);
        let bare = e.tick(21_000);
        assert_eq!(bare[0].grade, "bare");
        // agent bracket starts well after the bare closed
        e.observe_start(60_000, "claude-code", Some("fix auth".into()), None);
        e.observe_changes(70_000, &[obs("b.rs")]);
        let closed = e.observe_stop(120_000, "claude-code", None, None);
        let t = &closed[0];
        assert!(t.merges.is_empty());
        assert_eq!(
            t.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["b.rs"]
        );
        assert_eq!(t.opened_at, 60_000); // not pulled earlier by the pre-bracket bare
    }

    // C1: a bare fragment that closed INSIDE the bracket's own span (e.g. a
    // daemon-restart artifact recovered mid-bracket) still folds in.
    #[test]
    fn fold_includes_bare_closed_inside_bracket_span() {
        let mut e = TurnEngine::new();
        // bare closes at t=75_000 — inside what becomes the bracket's
        // [60_000, 80_000] span (simulating a restart-recovered fragment).
        e.observe_changes(64_000, &[obs("interim.rs")]);
        let bare = e.tick(75_000); // 75_000 - 64_000 = 11s > QUIET_MS
        assert_eq!(bare[0].grade, "bare");
        assert_eq!(bare[0].closed_at, 75_000);
        let bare_id = bare[0].id.clone();
        e.observe_start(60_000, "claude-code", Some("fix auth".into()), None);
        e.observe_changes(70_500, &[obs("b.rs")]);
        let closed = e.observe_stop(80_000, "claude-code", None, None);
        let t = &closed[0];
        assert_eq!(t.merges, vec![bare_id]);
        let mut paths: Vec<&str> = t.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, vec!["b.rs", "interim.rs"]);
    }

    // C2: a Quiet turn open well past the young-turn window must not absorb
    // a subsequent git checkout burst — it closes bare at its last real
    // mutation, and the burst opens its own fresh git turn.
    #[test]
    fn long_open_quiet_turn_closes_bare_before_git_burst() {
        let mut e = TurnEngine::new();
        // human editing continuously: saves every 5s keep the quiet turn open
        for i in 0..7u64 {
            e.observe_changes(i * 5_000, &[obs(&format!("edit{i}.md"))]);
        }
        // Closes the long-open quiet turn into the engine's internal pending
        // queue — NOT returned directly (observe_git_change returns `()`),
        // so it can only surface through the next `tick` call, same as
        // every other close the daemon already knows to persist.
        e.observe_git_change(31_000);

        let burst: Vec<ChangeObs> = (0..400).map(|i| obs(&format!("src/f{i}.rs"))).collect();
        e.observe_changes(31_500, &burst);
        let closed = e.tick(31_500 + QUIET_MS + GIT_SETTLE_MS + 1);
        // Both the pending pre-git bare AND the settled git turn surface
        // from this single tick call.
        assert_eq!(closed.len(), 2);
        let bare = closed
            .iter()
            .find(|t| t.grade == "bare")
            .expect("bare turn");
        assert_eq!(bare.files.len(), 7);
        assert_eq!(bare.closed_at, 30_000); // last real mutation, not 31_000
        let git = closed
            .iter()
            .find(|t| t.tool.as_deref() == Some("git"))
            .expect("git turn");
        assert_eq!(git.grade, "rich");
        assert_eq!(git.boundary, "git");
        assert_eq!(git.files.len(), 400);
    }

    // C2: a pending git-transition close surfaces on the very next tick even
    // when nothing else closes in that tick — the daemon calls tick() every
    // loop iteration and persists whatever it returns, so this is the only
    // guarantee the closed bare turn actually gets written.
    #[test]
    fn pending_git_close_surfaces_on_next_tick_even_when_nothing_else_closes() {
        let mut e = TurnEngine::new();
        for i in 0..7u64 {
            e.observe_changes(i * 5_000, &[obs(&format!("edit{i}.md"))]);
        }
        e.observe_git_change(31_000);
        let closed = e.tick(31_100); // far too soon for anything else to close
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].grade, "bare");
        assert_eq!(closed[0].files.len(), 7);
    }

    // C2: a young open Quiet turn still converts in place (existing
    // behavior preserved) — no bare turn is produced.
    #[test]
    fn young_open_quiet_turn_still_converts_to_git() {
        let mut e = TurnEngine::new();
        e.observe_changes(0, &[obs("x.rs")]);
        e.observe_git_change(1_000);
        let closed = e.tick(1_000 + GIT_SETTLE_MS + 1);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].tool.as_deref(), Some("git"));
    }

    // C3: a path touched by both an earlier bare fragment and the rich
    // turn's own (later) capture keeps the fragment's earliest before_hash —
    // the true original — not the later mid-turn state.
    #[test]
    fn fold_keeps_earliest_before_hash_for_shared_path() {
        let mut e = TurnEngine::new();
        // fragment 1: true original a.rs = v0, closes bare
        e.observe_changes(
            0,
            &[ChangeObs {
                before_hash: Some("sha256:v0".into()),
                ..obs("a.rs")
            }],
        );
        let bare = e.tick(11_000);
        assert_eq!(bare[0].grade, "bare");
        // fragment 2: agent re-edits a.rs; its own before is mid-turn state
        e.observe_changes(
            60_000,
            &[ChangeObs {
                before_hash: Some("sha256:v1".into()),
                ..obs("a.rs")
            }],
        );
        let closed = e.observe_stop(65_000, "codex", Some("refactor".into()), None);
        let t = &closed[0];
        assert_eq!(t.merges.len(), 1);
        let before = t
            .files
            .iter()
            .find(|f| f.path == "a.rs")
            .unwrap()
            .before_hash
            .clone();
        assert_eq!(before.as_deref(), Some("sha256:v0"));
    }

    // C4: a later observation's terminal flags (deleted/snapshotted/withheld)
    // must win over an earlier one, while before_hash/baseline_unknown stay
    // pinned to the first observation.
    #[test]
    fn dedup_updates_terminal_flags_from_latest_observation() {
        let mut e = TurnEngine::new();
        e.observe_start(0, "claude-code", None, None);
        let del = ChangeObs {
            path: "a.rs".into(),
            before_hash: Some("sha256:v0".into()),
            snapshotted: true,
            withheld: false,
            baseline_unknown: false,
            deleted: true,
        };
        e.observe_changes(1_000, &[del]);
        let recreate = ChangeObs {
            path: "a.rs".into(),
            before_hash: Some("sha256:different".into()), // must be ignored
            snapshotted: true,
            withheld: false,
            baseline_unknown: false,
            deleted: false,
        };
        e.observe_changes(2_000, &[recreate]);
        let closed = e.observe_stop(3_000, "claude-code", None, None);
        let f = &closed[0].files[0];
        assert!(!f.deleted); // latest observation wins
        assert_eq!(f.before_hash.as_deref(), Some("sha256:v0")); // first observation wins
    }

    // C5: a bracket-timeout close lands at the last real mutation, not the
    // tick's `now` — the crash rule fires MAX_BRACKET_MS+ after the bracket
    // went quiet, and gluing that whole dead gap onto the turn would report
    // hours of silence as agent activity.
    #[test]
    fn bracket_timeout_closes_at_last_mutation_not_tick_time() {
        let mut e = TurnEngine::new();
        e.observe_start(0, "claude-code", Some("long job".into()), None);
        e.observe_changes(500, &[obs("a.rs")]); // last real mutation at t=500
        assert!(e.tick(MAX_BRACKET_MS).is_empty());
        let closed = e.tick(MAX_BRACKET_MS + 1);
        assert_eq!(closed.len(), 1);
        let t = &closed[0];
        assert!(t.truncated);
        assert_eq!(t.grade, "rich");
        assert_eq!(t.boundary, "timeout");
        assert_eq!(t.closed_at, 500);
    }
}
