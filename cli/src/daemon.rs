//! `agentrec record`: the recorder daemon. Watches the worktree (gitignore +
//! built-in denylist), snapshots touched files into the object store, tails the
//! hook signal inbox, and drives `agentrec_core::engine::TurnEngine` to segment
//! and persist turns. Threads + `notify`, no async runtime.
//!
//! Clock discipline (PROTOCOL §"Clocks", D12): the engine runs on a MONOTONIC
//! millisecond clock so a wall-clock jump can never corrupt quiet-window math;
//! record timestamps are derived by adding a fixed startup offset, keeping them
//! sane (end >= start) regardless of clock changes.

use crate::state::{read_state, record_io_failure, write_state};
use crate::{log_path, objects_dir, open_path, signal_path};
use agentrec_core::engine::{ChangeObs, ClosedTurn, TurnEngine};
use agentrec_core::id::turn_id;
use agentrec_core::record::{
    append_log, parse_signals, EpochRecord, FileEntry, LogRecord, SignalEvent, TurnRecord,
};
use agentrec_core::scrub;
use agentrec_core::store::{BlobStore, PutResult};
use agentrec_core::time::rfc3339;
use agentrec_core::MAX_SNAPSHOT_BYTES;
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Debounce window before a mutation burst is staged (SPEC.md: 1.5 s).
const DEBOUNCE: Duration = Duration::from_millis(1_500);
/// Hard ceiling: a burst that keeps resetting the debounce is force-staged after
/// this long, so continuous churn on an unfiltered path can't starve turns.
const MAX_DEBOUNCE: Duration = Duration::from_secs(10);
/// Loop poll granularity — also the max latency of quiet-window/tick closure.
const POLL: Duration = Duration::from_millis(250);

pub fn run(root: &Path) -> Result<(), String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("cannot resolve root {}: {e}", root.display()))?;
    if !objects_dir(&root).exists() {
        return Err("not initialized — run `agentrec init` first".into());
    }
    acquire_lock(&root)?;

    // A journal left behind by an unclean shutdown (kill -9) is closed and
    // logged before this session opens its own epoch (AC B2).
    recover_orphan(&root)?;

    let clock = Clock::start();
    let store = BlobStore::new(objects_dir(&root));
    let mut engine = TurnEngine::new();
    let mut recorder = Recorder::scan(&root, store);
    let mut tailer = SignalTailer::open(&root)?;
    let mut ignore_set = IgnoreSet::build(&root);
    let mut journal_cache: Option<String> = None;

    append_epoch(&root, "start", clock.wall_ms(clock.now_ms()))?;

    // notify → channel of raw events; we debounce and filter here.
    let (tx, rx) = channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| format!("watcher init failed: {e}"))?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| watch_error(&e))?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_h = stop.clone();
    let _ = ctrlc::set_handler(move || stop_h.store(true, Ordering::SeqCst));

    println!(
        "recording {} (hooks: claude-code · git-aware · fallback: quiet-{}s)",
        root.display(),
        agentrec_core::QUIET_MS / 1000
    );

    let mut pending: HashSet<PathBuf> = HashSet::new();
    let mut last_event: Option<Instant> = None;
    let mut first_event: Option<Instant> = None;
    let mut git_hit = false;

    loop {
        match rx.recv_timeout(POLL) {
            Ok(Ok(event)) => {
                for path in event.paths {
                    match classify(&root, &path, &ignore_set) {
                        Class::GitRef => git_hit = true,
                        Class::Watch => {
                            pending.insert(path);
                            let at = Instant::now();
                            first_event.get_or_insert(at);
                            last_event = Some(at);
                        }
                        Class::Ignore => {}
                    }
                }
            }
            Ok(Err(_)) => {} // watcher-level error on one event; keep going
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        let now = clock.now_ms();

        // A ref transition classifies the surrounding burst as a git turn.
        if git_hit {
            engine.observe_git_change(now);
            git_hit = false;
        }

        // Flush the debounced burst once it settles, OR once it has been open
        // for MAX_DEBOUNCE (continuous churn would otherwise never settle).
        let settled = last_event.map(|t| t.elapsed() >= DEBOUNCE).unwrap_or(false);
        let capped = first_event
            .map(|t| t.elapsed() >= MAX_DEBOUNCE)
            .unwrap_or(false);
        if settled || capped {
            // A touched `.gitignore` changes the filter — rebuild after staging.
            let gitignore_touched = pending
                .iter()
                .any(|p| p.file_name().and_then(|n| n.to_str()) == Some(".gitignore"));
            // H7: exclude paths a concurrent `undo --confirm` is writing to —
            // those are undo's own mutation, not agent/human activity, and
            // must never mint a spurious bare turn.
            let guarded = undo_guard_paths(&root, clock.wall_ms(now));
            if !guarded.is_empty() {
                pending.retain(|p| {
                    p.strip_prefix(&root)
                        .map(|rel| !guarded.contains(rel))
                        .unwrap_or(true)
                });
            }
            let changes = recorder.stage(&pending);
            engine.observe_changes(now, &changes);
            drain_io_failures(&root, &mut recorder);
            pending.clear();
            last_event = None;
            first_event = None;
            if gitignore_touched {
                ignore_set = IgnoreSet::build(&root);
            }
        }

        // Consume any new hook signals (start/stop brackets). Fill missing
        // prompt/model from the transcript (Q+) before feeding the engine.
        for sig in tailer.poll(&root) {
            let (prompt, model) = signal_context(&sig);
            if let (Some(m), Some(s)) = (&model, &sig.session) {
                recorder.set_model(s.clone(), m.clone());
            }
            let closed = apply_signal(&mut engine, &sig, prompt, now);
            persist(&root, &recorder, &clock, closed)?;
        }

        // Quiet-window / settle / bracket-timeout closures.
        let closed = engine.tick(now);
        persist(&root, &recorder, &clock, closed)?;

        // Mirror the open turn to the crash journal (or clear it when idle).
        sync_journal(&root, &engine, &recorder, &clock, &mut journal_cache);

        if stop.load(Ordering::SeqCst) {
            break;
        }
    }

    // Clean shutdown: flush any final burst, force-close the open turn (any
    // source, at the real time), mark the epoch.
    let now = clock.now_ms();
    if !pending.is_empty() {
        let changes = recorder.stage(&pending);
        engine.observe_changes(now, &changes);
        drain_io_failures(&root, &mut recorder);
    }
    let closed = engine.force_close(now);
    persist(&root, &recorder, &clock, closed)?;
    // Clean shutdown: the turn is persisted, so drop the crash journal.
    sync_journal(&root, &engine, &recorder, &clock, &mut journal_cache);
    append_epoch(&root, "stop", clock.wall_ms(clock.now_ms()))?;
    release_lock(&root);
    println!("\nstopped recording {}", root.display());
    Ok(())
}

/// Monotonic engine clock with a fixed wall-clock offset for record stamps.
struct Clock {
    start_mono: Instant,
    start_wall_ms: u64,
}

impl Clock {
    fn start() -> Self {
        Clock {
            start_mono: Instant::now(),
            start_wall_ms: wall_now_ms(),
        }
    }
    fn now_ms(&self) -> u64 {
        self.start_mono.elapsed().as_millis() as u64
    }
    fn wall_ms(&self, mono_ms: u64) -> u64 {
        self.start_wall_ms + mono_ms
    }
}

fn wall_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---- change classification --------------------------------------------------

enum Class {
    /// A git ref/index/HEAD transition — drives git-turn classification.
    GitRef,
    /// A real worktree mutation to record.
    Watch,
    /// Filtered out (denylist, gitignore, or non-git dotfile under .git).
    Ignore,
}

fn classify(root: &Path, path: &Path, ignore_set: &IgnoreSet) -> Class {
    let Ok(rel) = path.strip_prefix(root) else {
        return Class::Ignore;
    };
    let comps: Vec<&str> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    if comps.is_empty() {
        return Class::Ignore;
    }
    // Built-in denylist — robust regardless of gitignore contents. `.agentrec`
    // suppression is load-bearing: it prevents the recorder's own writes from
    // opening turns (no feedback loop). `.git` HEAD/index/refs feed git-turn
    // classification; the rest of `.git` is noise.
    if comps[0] == ".git" {
        return match comps.get(1).copied() {
            Some("HEAD") | Some("ORIG_HEAD") | Some("index") | Some("refs") => Class::GitRef,
            _ => Class::Ignore,
        };
    }
    if comps
        .iter()
        .any(|c| matches!(*c, ".agentrec" | "node_modules" | "target" | "dist"))
    {
        return Class::Ignore;
    }
    // Everything else (`.next/`, `.venv/`, `__pycache__/`, `coverage/`, …) is
    // filtered by gitignore, honoring nested files and git precedence.
    if ignore_set.is_ignored(path, path.is_dir()) {
        return Class::Ignore;
    }
    Class::Watch
}

/// Gitignore matching that respects nested `.gitignore` files and git's
/// precedence rules (B+, D29). One matcher per directory (patterns relative to
/// that directory); a path is decided by its closest ancestor matcher, so a
/// deeper `!pattern` re-includes what a shallower file excluded.
struct IgnoreSet {
    /// (base dir, matcher), deepest base first.
    matchers: Vec<(PathBuf, ignore::gitignore::Gitignore)>,
}

impl IgnoreSet {
    fn build(root: &Path) -> Self {
        let mut matchers = vec![];
        // The walk itself prunes ignored dirs, so we never descend into (e.g.)
        // node_modules to collect a stray `.gitignore`.
        for entry in ignore::WalkBuilder::new(root)
            .hidden(false)
            .parents(false)
            .build()
            .flatten()
        {
            if entry.file_name() != std::ffi::OsStr::new(".gitignore") {
                continue;
            }
            let dir = entry.path().parent().unwrap_or(root).to_path_buf();
            let mut builder = ignore::gitignore::GitignoreBuilder::new(&dir);
            if builder.add(entry.path()).is_none() {
                if let Ok(gi) = builder.build() {
                    matchers.push((dir, gi));
                }
            }
        }
        // Deepest directory first → its rules take precedence.
        matchers.sort_by_key(|(dir, _)| std::cmp::Reverse(dir.components().count()));
        IgnoreSet { matchers }
    }

    fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        for (dir, gi) in &self.matchers {
            let Ok(rel) = path.strip_prefix(dir) else {
                continue; // matcher's dir is not an ancestor of this path
            };
            match gi.matched_path_or_any_parents(rel, is_dir) {
                ignore::Match::Ignore(_) => return true,
                ignore::Match::Whitelist(_) => return false,
                ignore::Match::None => {}
            }
        }
        false
    }
}

/// Repo-relative paths a concurrent `undo --confirm` is currently writing
/// (AC H7), if the guard exists and hasn't passed its `until_ms` deadline.
/// Tolerant: a missing, corrupt, or expired guard yields an empty set rather
/// than blocking normal recording — a crash-orphaned guard must not wedge
/// the daemon.
fn undo_guard_paths(root: &Path, now_wall_ms: u64) -> HashSet<PathBuf> {
    let Ok(text) = std::fs::read_to_string(crate::undo_guard_path(root)) else {
        return HashSet::new();
    };
    let Ok(guard) = serde_json::from_str::<crate::UndoGuard>(&text) else {
        return HashSet::new();
    };
    if guard.until_ms < now_wall_ms {
        return HashSet::new();
    }
    guard.paths.into_iter().map(PathBuf::from).collect()
}

// ---- snapshotting -----------------------------------------------------------

/// Owns the object store and the per-path baseline needed to derive `before`,
/// `op`, and the honest `baseline_unknown` flag.
struct Recorder {
    root: PathBuf,
    store: BlobStore,
    /// Paths that existed at daemon start (for create vs. baseline_unknown).
    known: HashSet<PathBuf>,
    /// Path → last content hash we snapshotted (this session).
    baseline: HashMap<PathBuf, String>,
    /// Path (relative, string) → latest `after` hash, resolved at turn close.
    after: HashMap<String, Option<String>>,
    /// Session id → model, extracted from transcripts; resolved at persist.
    models: HashMap<String, String>,
    /// (rel_path, cause) pairs for genuine snapshot I/O failures this batch,
    /// drained by the caller into `state.json` after each `stage()` call.
    io_failures: Vec<(String, String)>,
}

impl Recorder {
    fn scan(root: &Path, store: BlobStore) -> Self {
        // Record which paths exist now; we do NOT hash contents (that would
        // snapshot the whole repo). The first edit of an unhashed pre-existing
        // file therefore has an unrecoverable `before` → baseline_unknown.
        let mut known = HashSet::new();
        for entry in ignore::WalkBuilder::new(root)
            .hidden(false)
            .build()
            .flatten()
        {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Ok(rel) = entry.path().strip_prefix(root) {
                    known.insert(rel.to_path_buf());
                }
            }
        }
        Recorder {
            root: root.to_path_buf(),
            store,
            known,
            baseline: HashMap::new(),
            after: HashMap::new(),
            models: HashMap::new(),
            io_failures: Vec::new(),
        }
    }

    fn set_model(&mut self, session: String, model: String) {
        self.models.insert(session, model);
    }

    fn model_for(&self, session: Option<&str>) -> Option<String> {
        session.and_then(|s| self.models.get(s).cloned())
    }

    /// Snapshot each changed path and produce engine observations. Updates the
    /// baseline and the `after` map. Transient create+delete within one batch
    /// is dropped as noise.
    fn stage(&mut self, paths: &HashSet<PathBuf>) -> Vec<ChangeObs> {
        let mut out = vec![];
        for abs in paths {
            let Ok(rel) = abs.strip_prefix(&self.root) else {
                continue;
            };
            let rel = rel.to_path_buf();
            let rel_str = rel.to_string_lossy().to_string();
            let secret = scrub::is_secret_path(&rel_str);

            let meta = std::fs::symlink_metadata(abs);
            let deleted = matches!(&meta, Err(e) if e.kind() == std::io::ErrorKind::NotFound);
            let is_symlink = meta
                .as_ref()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            if is_dir {
                continue; // directory mtime churn is not a file change
            }

            // Compute the `after` snapshot.
            let (after, snapshotted, withheld) = if secret {
                (None, false, true) // never snapshotted (D31)
            } else if deleted {
                (None, true, false) // delete: no content, but not "skipped"
            } else if is_symlink {
                // Not followed (AC B5): snapshot the link *target string*, so the
                // symlink change is recorded without reading the pointed-to file.
                match std::fs::read_link(abs) {
                    Ok(target) => (
                        self.store.put(target.to_string_lossy().as_bytes()),
                        true,
                        false,
                    ),
                    Err(_) => (None, false, false),
                }
            } else {
                match std::fs::read(abs) {
                    Ok(bytes) if bytes.len() <= MAX_SNAPSHOT_BYTES => {
                        match self.store.put_result(&bytes) {
                            PutResult::Stored(h) => (Some(h), true, false),
                            PutResult::OverCap => (None, false, false), // over cap → skipped
                            PutResult::IoError(cause) => {
                                self.io_failures.push((rel_str.clone(), cause));
                                (None, false, false) // write failed → skipped
                            }
                        }
                    }
                    Ok(_) => (None, false, false), // over cap → skipped
                    Err(_) => (None, false, false), // unreadable → skipped
                }
            };

            // Derive `before` from the baseline; honest about unrecoverable ones.
            let (before, baseline_unknown) = match self.baseline.get(&rel) {
                Some(h) => (Some(h.clone()), false),
                None if self.known.contains(&rel) => (None, true),
                None => (None, false),
            };

            // Drop transient noise: a file created and gone with nothing captured.
            if before.is_none() && !baseline_unknown && after.is_none() && deleted {
                continue;
            }

            // Advance session state.
            match (&after, deleted) {
                (Some(h), _) => {
                    self.baseline.insert(rel.clone(), h.clone());
                    self.known.insert(rel.clone());
                }
                (None, true) => {
                    self.baseline.remove(&rel);
                    self.known.remove(&rel);
                }
                (None, false) => {}
            }
            self.after.insert(rel_str.clone(), after);

            out.push(ChangeObs {
                path: rel_str,
                before_hash: before,
                snapshotted,
                withheld,
                baseline_unknown,
                deleted,
            });
        }
        out
    }

    /// Turn a closed turn's observations into wire file entries.
    fn resolve(&self, obs: &ChangeObs) -> FileEntry {
        let after = self.after.get(&obs.path).cloned().flatten();
        let before = obs.before_hash.clone();
        // `op` keys on whether the file is actually gone, NOT on `after.is_none()`
        // — an over-cap or unreadable *modify* has no `after` snapshot yet still
        // exists, and must never be logged as a delete.
        let op = if obs.deleted {
            "delete"
        } else if before.is_none() && !obs.baseline_unknown {
            "create"
        } else {
            "modify"
        };
        FileEntry {
            path: obs.path.clone(),
            before,
            after,
            op: op.to_string(),
            skipped: !obs.snapshotted && !obs.withheld,
            withheld: obs.withheld,
            baseline_unknown: obs.baseline_unknown,
        }
    }
}

/// Drain `recorder`'s pending snapshot I/O failures (D35) into `state.json`:
/// bump the counter, track the path, and warn loudly on stderr. Mirrors the
/// SignalTailer offset pattern — read, mutate, write.
fn drain_io_failures(root: &Path, recorder: &mut Recorder) {
    if recorder.io_failures.is_empty() {
        return;
    }
    let mut state = read_state(root);
    for (path, cause) in recorder.io_failures.drain(..) {
        record_io_failure(&mut state, &path);
        eprintln!("agentrec: snapshot write failed for {path}: {cause}");
    }
    write_state(root, &state);
}

// ---- signals ----------------------------------------------------------------

/// Tails `.agentrec/signal.jsonl` from a persisted byte offset. A torn trailing
/// line (no newline yet) is left unconsumed; the offset never regresses.
struct SignalTailer {
    offset: u64,
}

impl SignalTailer {
    fn open(root: &Path) -> Result<Self, String> {
        // Start at the current end of the inbox, not the persisted offset. A
        // signal that landed while no daemon was recording refers to fs changes
        // we never observed; replaying it would mint an empty turn misdated to
        // daemon-boot (reviewer #5). The pre-recording interval is an honest gap.
        let offset = std::fs::metadata(signal_path(root))
            .map(|m| m.len())
            .unwrap_or(0);
        Ok(SignalTailer { offset })
    }

    fn poll(&mut self, root: &Path) -> Vec<SignalEvent> {
        let text = match std::fs::read(signal_path(root)) {
            Ok(bytes) => bytes,
            Err(_) => return vec![],
        };
        if (text.len() as u64) <= self.offset {
            return vec![];
        }
        let fresh = &text[self.offset as usize..];
        // Only consume up to the last complete line.
        let Some(nl) = fresh.iter().rposition(|b| *b == b'\n') else {
            return vec![]; // no complete line yet
        };
        let consumable = &fresh[..=nl];
        let events = parse_signals(&String::from_utf8_lossy(consumable));
        self.offset += (nl + 1) as u64;
        let mut state = read_state(root);
        state.signal_offset = self.offset;
        write_state(root, &state);
        events
    }
}

fn apply_signal(
    engine: &mut TurnEngine,
    sig: &SignalEvent,
    prompt: Option<String>,
    now: u64,
) -> Vec<ClosedTurn> {
    // `prompt` is already resolved (hook-provided or transcript-extracted);
    // persist() scrubs before anything reaches disk (idempotent, AC I4).
    if sig.is_start() {
        engine.observe_start(now, &sig.tool, prompt, sig.session.clone())
    } else {
        engine.observe_stop(now, &sig.tool, prompt, sig.session.clone())
    }
}

/// Resolve a signal's effective prompt + model. Prompt prefers the hook-provided
/// value, falling back to the transcript's last real user message (Q+). Model is
/// read from the transcript's assistant messages. Extraction is best-effort and
/// tolerant: a parse failure yields None rather than a wrong attribution.
fn signal_context(sig: &SignalEvent) -> (Option<String>, Option<String>) {
    let mut prompt = sig.prompt.clone();
    let mut model = None;
    if let Some(path) = &sig.transcript {
        if let Ok(text) = std::fs::read_to_string(path) {
            let (tp, tm) = parse_transcript(&text);
            if prompt.is_none() {
                prompt = tp;
            }
            model = tm;
        }
    }
    // Scrub HERE, before the prompt enters the engine: the transcript fallback
    // yields raw text, and the open turn is mirrored to `.agentrec/open.json`
    // (a disk path) every loop — no pre-scrub prompt may reach it. Re-scrubbing
    // the already-clean hook-provided prompt is idempotent.
    (prompt.map(|p| scrub::scrub(&p)), model)
}

/// Parse a Claude Code JSONL transcript for (last real user prompt, last model).
/// `user` entries that are tool results carry no text block, so they never
/// masquerade as a prompt.
fn parse_transcript(text: &str) -> (Option<String>, Option<String>) {
    let mut last_user = None;
    let mut last_model = None;
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        let message = value.get("message").unwrap_or(&value);
        let role = message
            .get("role")
            .and_then(|r| r.as_str())
            .or_else(|| value.get("type").and_then(|t| t.as_str()));
        match role {
            Some("user") => {
                if let Some(text) = transcript_text(message) {
                    if !text.trim().is_empty() {
                        last_user = Some(text);
                    }
                }
            }
            Some("assistant") => {
                if let Some(model) = message.get("model").and_then(|m| m.as_str()) {
                    last_model = Some(model.to_string());
                }
            }
            _ => {}
        }
    }
    (last_user, last_model)
}

/// Concatenate the `text` blocks of a message's content; string content is taken
/// verbatim. tool_use / tool_result blocks contribute nothing.
fn transcript_text(message: &serde_json::Value) -> Option<String> {
    match message.get("content") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(blocks)) => {
            let mut out = String::new();
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        out.push_str(t);
                    }
                }
            }
            (!out.is_empty()).then_some(out)
        }
        _ => None,
    }
}

// ---- persistence ------------------------------------------------------------

fn persist(
    root: &Path,
    recorder: &Recorder,
    clock: &Clock,
    closed: Vec<ClosedTurn>,
) -> Result<(), String> {
    for turn in closed {
        let files: Vec<FileEntry> = turn.files.iter().map(|o| recorder.resolve(o)).collect();
        // Scrub runs inside persistence (AC I4): no pre-scrub prompt text is
        // ever written to the log or the object store.
        let (prompt_ref, prompt_excerpt) = match &turn.prompt {
            Some(text) => {
                let excerpt = scrub::excerpt(text);
                let full = scrub::scrub(text);
                (recorder.store.put(full.as_bytes()), Some(excerpt))
            }
            None => (None, None),
        };
        let record = TurnRecord {
            v: 1,
            id: turn.id.clone(),
            grade: turn.grade.to_string(),
            truncated: turn.truncated,
            started: rfc3339(clock.wall_ms(turn.opened_at)),
            ended: rfc3339(clock.wall_ms(turn.closed_at)),
            tool: turn.tool.clone(),
            model: recorder.model_for(turn.session.as_deref()),
            session: turn.session.clone(),
            root: root.to_string_lossy().to_string(),
            prompt_ref,
            prompt_excerpt,
            merges: turn.merges.clone(),
            files,
        };
        append_log(&log_path(root), &LogRecord::Turn(record))?;
    }
    Ok(())
}

// ---- crash journal (AC B2) --------------------------------------------------

/// Disk mirror of the in-flight open turn. Files are already resolved (the
/// `after` hashes live only in memory, so raw observations would be
/// unrecoverable); times are wall-clock, decoupling recovery from the dead
/// process's monotonic clock.
#[derive(Serialize, Deserialize)]
struct OrphanJournal {
    source: String, // "bracket" | "git" | "quiet"
    tool: Option<String>,
    prompt: Option<String>,
    session: Option<String>,
    opened_wall_ms: u64,
    last_change_wall_ms: u64,
    root: String,
    files: Vec<FileEntry>,
}

/// Write the open-turn journal (atomic tmp+rename), or remove it when idle.
/// The cache suppresses redundant rewrites while the turn sits unchanged.
fn sync_journal(
    root: &Path,
    engine: &TurnEngine,
    recorder: &Recorder,
    clock: &Clock,
    cache: &mut Option<String>,
) {
    match engine.snapshot_open() {
        Some(snap) => {
            let files: Vec<FileEntry> = snap.files.iter().map(|o| recorder.resolve(o)).collect();
            let journal = OrphanJournal {
                source: snap.source,
                tool: snap.tool,
                prompt: snap.prompt,
                session: snap.session,
                opened_wall_ms: clock.wall_ms(snap.opened_at),
                last_change_wall_ms: clock.wall_ms(snap.last_change_at),
                root: root.to_string_lossy().to_string(),
                files,
            };
            let Ok(text) = serde_json::to_string(&journal) else {
                return;
            };
            if cache.as_deref() == Some(text.as_str()) {
                return; // unchanged since last write
            }
            let path = open_path(root);
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, &text).is_ok() {
                // D37: lock down before the rename makes it visible under
                // its final name.
                agentrec_core::perms::lock_file(&tmp);
                if std::fs::rename(&tmp, &path).is_ok() {
                    *cache = Some(text);
                }
            }
        }
        None => {
            let path = open_path(root);
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
            *cache = None;
        }
    }
}

/// On startup, close and log any turn a prior process left journaled. Recovery
/// is source-aware: an unattributed turn closes `bare` (AC B2), a bracket closes
/// `truncated rich` keeping attribution (crash rule C+/F1). Blobs referenced by
/// the journaled files already exist — they were snapshotted before the crash.
fn recover_orphan(root: &Path) -> Result<(), String> {
    let path = open_path(root);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let Ok(journal) = serde_json::from_str::<OrphanJournal>(&text) else {
        // Corrupt/torn journal: drop it rather than crash (no manifest damage).
        let _ = std::fs::remove_file(&path);
        eprintln!("agentrec: discarding unreadable crash journal");
        return Ok(());
    };

    let (grade, truncated) = match journal.source.as_str() {
        "bracket" => ("rich", true), // start-without-stop → truncated, keep attribution
        "git" => ("rich", false),
        _ => ("bare", false), // unattributed window → honest bare
    };

    // A bare recovery carries no attribution; a rich one keeps tool + prompt.
    let (tool, session, prompt_ref, prompt_excerpt) = if grade == "bare" {
        (None, None, None, None)
    } else {
        let store = BlobStore::new(objects_dir(root));
        let (prompt_ref, excerpt) = match &journal.prompt {
            Some(text) => (
                store.put(scrub::scrub(text).as_bytes()),
                Some(scrub::excerpt(text)),
            ),
            None => (None, None),
        };
        (
            journal.tool.clone(),
            journal.session.clone(),
            prompt_ref,
            excerpt,
        )
    };

    let ended = journal.last_change_wall_ms.max(journal.opened_wall_ms);
    let record = TurnRecord {
        v: 1,
        id: turn_id(),
        grade: grade.to_string(),
        truncated,
        started: rfc3339(journal.opened_wall_ms),
        ended: rfc3339(ended),
        tool,
        model: None,
        session,
        root: journal.root.clone(),
        prompt_ref,
        prompt_excerpt,
        merges: vec![],
        files: journal.files.clone(),
    };
    append_log(&log_path(root), &LogRecord::Turn(record))?;
    let _ = std::fs::remove_file(&path);
    eprintln!(
        "agentrec: recovered orphaned turn from an unclean shutdown ({} file(s), grade {grade})",
        journal.files.len()
    );
    Ok(())
}

fn append_epoch(root: &Path, event: &str, wall_ms: u64) -> Result<(), String> {
    append_log(
        &log_path(root),
        &LogRecord::Epoch(EpochRecord {
            v: 1,
            event: event.to_string(),
            ts: rfc3339(wall_ms),
        }),
    )
}

// ---- lock -------------------------------------------------------------------

/// Acquire the per-root recorder lock (AC B1). A live PID blocks with exit 1;
/// a stale PID (dead process) is broken with a logged notice.
fn acquire_lock(root: &Path) -> Result<(), String> {
    let mut state = read_state(root);
    if state.pid != 0 && pid_alive(state.pid) {
        return Err(format!(
            "already recording {} (pid {}) — stop it first",
            root.display(),
            state.pid
        ));
    }
    if state.pid != 0 {
        eprintln!("agentrec: breaking stale lock (pid {} is gone)", state.pid);
    }
    state.pid = std::process::id();
    write_state(root, &state);
    Ok(())
}

fn release_lock(root: &Path) {
    let mut state = read_state(root);
    if state.pid == std::process::id() {
        state.pid = 0;
        write_state(root, &state);
    }
}

/// `kill(pid, 0)`: true if the process exists (or we lack permission to signal
/// it — still "alive" for lock purposes).
pub(crate) fn pid_alive(pid: u32) -> bool {
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn watch_error(e: &notify::Error) -> String {
    let msg = e.to_string();
    if msg.contains("inotify") || msg.contains("watch") && msg.contains("limit") {
        return format!(
            "watch failed ({msg}) — raise the inotify limit: \
             sudo sysctl fs.inotify.max_user_watches=524288"
        );
    }
    format!("watch failed: {msg}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // B+ / D29: nested `.gitignore` precedence follows git's rules — deeper
    // files override shallower ones, `!` re-includes, and a matched directory
    // ignores everything beneath it.
    #[test]
    fn nested_gitignore_precedence() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join(".gitignore"), "*.log\nbuild/\n").unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/.gitignore"), "!keep.log\n*.tmp\n").unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();

        let set = IgnoreSet::build(root);
        let ig = |p: &str, is_dir: bool| set.is_ignored(&root.join(p), is_dir);

        assert!(ig("a.log", false), "root *.log ignored");
        assert!(!ig("main.rs", false), "source not ignored");
        assert!(
            ig("build/out.o", false),
            "matched parent dir ignores children"
        );
        assert!(ig("sub/x.tmp", false), "nested *.tmp ignored");
        // deeper `!keep.log` re-includes what the root `*.log` excluded
        assert!(!ig("sub/keep.log", false), "deeper whitelist wins");
        // a sibling .log still follows the root rule
        assert!(
            ig("sub/other.log", false),
            "root *.log still applies in sub"
        );
    }

    #[test]
    fn ignoreset_empty_when_no_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let set = IgnoreSet::build(tmp.path());
        assert!(!set.is_ignored(&tmp.path().join("anything.rs"), false));
    }

    // Q+ fixture: a pinned Claude Code transcript sample must yield the last real
    // user prompt and the model, while a tool_result user turn is NOT mistaken
    // for a prompt.
    #[test]
    fn transcript_extracts_prompt_and_model() {
        let sample = concat!(
            r#"{"type":"user","message":{"role":"user","content":"add rate limiting"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","model":"claude-fable-5","content":[{"type":"text","text":"on it"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"exit 0"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"now add tests"}]}}"#,
            "\n",
        );
        let (prompt, model) = parse_transcript(sample);
        // last real user text wins; the tool_result turn is skipped
        assert_eq!(prompt.as_deref(), Some("now add tests"));
        assert_eq!(model.as_deref(), Some("claude-fable-5"));
    }

    #[test]
    fn transcript_garbage_yields_none() {
        let (prompt, model) = parse_transcript("not json\n{}\n{\"type\":\"system\"}\n");
        assert!(prompt.is_none());
        assert!(model.is_none());
    }

    fn recorder_for(after: &[(&str, Option<&str>)]) -> Recorder {
        let tmp = tempfile::tempdir().unwrap();
        Recorder {
            root: tmp.path().to_path_buf(),
            store: BlobStore::new(tmp.path().join("obj")),
            known: HashSet::new(),
            baseline: HashMap::new(),
            after: after
                .iter()
                .map(|(p, h)| (p.to_string(), h.map(String::from)))
                .collect(),
            models: HashMap::new(),
            io_failures: Vec::new(),
        }
    }

    fn change(path: &str, before: Option<&str>, snapshotted: bool, deleted: bool) -> ChangeObs {
        ChangeObs {
            path: path.into(),
            before_hash: before.map(String::from),
            snapshotted,
            withheld: false,
            baseline_unknown: false,
            deleted,
        }
    }

    // Reviewer #1: an over-cap / unreadable *modify* (no `after` snapshot, file
    // still present) must be `op:modify skipped:true`, NEVER a false `delete`.
    #[test]
    fn over_cap_modify_is_not_a_delete() {
        let rec = recorder_for(&[("a.rs", None)]); // no after snapshot captured
        let entry = rec.resolve(&change("a.rs", Some("sha256:old"), false, false));
        assert_eq!(entry.op, "modify");
        assert!(entry.skipped);
        assert_eq!(entry.after, None);
        assert_eq!(entry.before.as_deref(), Some("sha256:old"));
    }

    #[test]
    fn genuine_delete_is_a_delete() {
        let rec = recorder_for(&[("b.rs", None)]);
        let entry = rec.resolve(&change("b.rs", Some("sha256:old"), true, true));
        assert_eq!(entry.op, "delete");
        assert!(!entry.skipped); // a delete is not "content over cap"
        assert_eq!(entry.after, None);
    }

    #[test]
    fn create_when_no_before_and_known_baseline() {
        let rec = recorder_for(&[("c.rs", Some("sha256:new"))]);
        let entry = rec.resolve(&change("c.rs", None, true, false));
        assert_eq!(entry.op, "create");
        assert_eq!(entry.after.as_deref(), Some("sha256:new"));
    }
}
