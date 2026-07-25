//! `agentrec record`: the recorder daemon. Watches the worktree (gitignore +
//! built-in denylist), snapshots touched files into the object store, tails the
//! hook signal inbox, and drives `agentrec_core::engine::TurnEngine` to segment
//! and persist turns. Threads + `notify`, no async runtime.
//!
//! Clock discipline (PROTOCOL §"Clocks", D12): the engine runs on a MONOTONIC
//! millisecond clock so a wall-clock jump can never corrupt quiet-window math;
//! record timestamps are derived by adding a fixed startup offset, keeping them
//! sane (end >= start) regardless of clock changes.

use crate::cmds::wall_now_ms;
use crate::state::{
    read_state, record_ignore_rebuild, record_io_failure, record_non_utf8_path_skip,
    record_prompt_put_failure, write_state, State,
};
use crate::{log_path, memorycmds, objects_dir, open_path, signal_path};
use agentrec_core::engine::{ChangeObs, ClosedTurn, TurnEngine};
use agentrec_core::id::turn_id;
use agentrec_core::record::{
    append_log, parse_signals, skip_reason, EpochRecord, FileEntry, LogRecord, SignalEvent,
    TurnRecord,
};
use agentrec_core::scrub;
use agentrec_core::store::{hash_bytes, BlobStore, PutResult};
use agentrec_core::time::rfc3339;
use agentrec_core::MAX_SNAPSHOT_BYTES;
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Seek, SeekFrom};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    // D2: a real OS-level advisory lock, held for the daemon's entire
    // lifetime via this binding. Must be a NAMED local, never `let _ =` — the
    // latter drops the `File` immediately, closing the fd and releasing the
    // flock right away, silently destroying mutual exclusion.
    let _lock = acquire_lock(&root)?;

    // A journal left behind by an unclean shutdown (kill -9) is closed and
    // logged before this session opens its own epoch (AC B2).
    recover_orphan(&root)?;

    let mut clock = Clock::start();
    let store = BlobStore::new(objects_dir(&root));
    let mut engine = TurnEngine::new();
    let mut recorder = Recorder::scan(&root, store);
    // Candidate-only startup replay: memory-candidate lines that landed in the
    // inbox while no daemon was running are ingested now, and the live tailer
    // starts exactly where this scan stopped so nothing is read twice.
    let replay_to = replay_pending_candidates(&root, engine.open_turn_id());
    let mut tailer = SignalTailer { offset: replay_to };
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
    // Set whenever a `.gitignore` event is observed, regardless of its own
    // ignore verdict; cleared after the ignore set is rebuilt.
    let mut gitignore_dirty = false;

    loop {
        // A touched `.gitignore` changes the filter, and is consumed HERE —
        // at the top of the tick, before this iteration's
        // `drain_watch_events` runs `classify` on anything — not inside the
        // `settled || capped` flush block below. The rebuild used to live in
        // that block, implicitly gated on `pending` becoming non-empty; but
        // `pending` only ever receives `Class::Watch` paths, and a path the
        // STALE ignore set still classifies `Ignore` (e.g. one a
        // `.gitignore` edit just re-included) never arms the debounce — so
        // the flush block, and the rebuild inside it, could go unreached
        // forever while the flag sat `true`. Moving the rebuild here means
        // it runs on the very next tick regardless of whether anything else
        // is pending.
        //
        // Residual window (stated, not eliminated): classification happens
        // inside `drain_watch_events`, which runs AFTER this. An event for
        // the same path arriving in the SAME drain batch as the `.gitignore`
        // edit is still classified against the pre-edit set. After this fix
        // the hole is "the next mutation of that path is honored" (≤1 POLL
        // tick + notify latency), not "never, until unrelated watched
        // activity." Closing it fully would mean reclassifying `pending` at
        // flush time — a larger change, deliberately not taken here.
        //
        // Consequence of losing the triggering event itself: harmless.
        // `Recorder::stage` reads *current* file bytes at flush time, so a
        // dropped intermediate event costs an intermediate snapshot, never
        // the file's content — the next mutation snapshots it as it then
        // stands.
        //
        // Walk-rate cost of this move (Phase 2 of the rebuild-gate fix):
        // pre-fix, a repo where only `.gitignore` churns did ZERO
        // `IgnoreSet::build` full-repo walks (the flush block never ran);
        // post-fix it can do up to one per `POLL` tick (250ms) indefinitely —
        // `state.json`'s `ignore_rebuilds`/`last_ignore_rebuild_ms` (surfaced
        // by `status`) is what makes that rate observable rather than
        // theoretical.
        if let Some(fresh) = maybe_rebuild(&mut gitignore_dirty, &root) {
            log_ignore_rebuild(&root, fresh.matchers.len(), clock.wall_ms(clock.now_ms()));
            ignore_set = fresh;
        }

        // D9: block for the first message, then drain everything already
        // queued via `try_recv` before moving on to flush logic. The old
        // one-event-per-250ms-tick shape let a large burst dribble in over
        // many ticks, which both delays debounce settlement and (worse) can
        // make MAX_DEBOUNCE fire before the burst has actually finished
        // arriving.
        if drain_watch_events(
            &rx,
            &root,
            &ignore_set,
            &mut pending,
            &mut last_event,
            &mut first_event,
            &mut git_hit,
            &mut gitignore_dirty,
        ) {
            break; // channel disconnected — the watcher thread is gone
        }

        // D4: re-anchor the wall-clock offset once per loop iteration so a
        // system sleep/wake (which halts the monotonic clock's elapsed()
        // progress relative to wall time) can't leave every subsequent
        // record timestamped hours early.
        clock.reanchor();
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
        }

        // Consume any new hook signals (start/stop brackets). Fill missing
        // prompt/model from the transcript (Q+) before feeding the engine.
        // Kill-switch (design spec line 184, binding): `memory_enabled =
        // false` disables injection AND candidate ingestion. Read once per
        // poll batch rather than per-candidate — candidates are rare
        // (occasional agent signals, not a hot loop) so a per-batch disk
        // read is cheap, and it still picks up a runtime config edit within
        // one POLL tick (250ms).
        let memory_enabled = memorycmds::read_memory_enabled(&root);
        for sig in tailer.poll(&root) {
            // Memory-candidate lines (PROTOCOL §4, additive) carry no `event`
            // field, so `is_start()` is false — routed here BEFORE
            // `apply_signal` ever sees them, or they'd fall through to the
            // stop arm and fabricate a turn closure (the hazard this guard
            // exists to close; see cli/tests/hardening_daemon.rs).
            if sig.is_memory_candidate() {
                if !memory_enabled {
                    // Disabled feature, not an invalid candidate: the line is
                    // still consumed from the inbox (offset already advanced
                    // by `tailer.poll` above) but nothing is ingested and
                    // nothing is counted as a reject.
                    continue;
                }
                let mut state = read_state(&root);
                // D-M6: if a turn is open, its id was only RESERVED in
                // memory (`TurnEngine::open_turn_id`) — the crash journal
                // that makes it recoverable after a kill-9 is normally
                // written once per loop iteration, AFTER this whole `for
                // sig` loop finishes. When a `start` and a `memory-candidate`
                // land in the SAME polled batch, `ingest_candidate` below
                // fsyncs a memory record whose `source_turns` names that
                // reserved id BEFORE the post-loop `sync_journal` call ever
                // runs. A kill-9 in that window leaves the id durably
                // referenced in memory.jsonl but recoverable nowhere — the
                // reference dangles. Force the journal write here, before
                // the candidate is persisted, so the open turn is always
                // recoverable before anything references its id.
                if engine.open_turn_id().is_some() {
                    sync_journal(&root, &engine, &recorder, &clock, &mut journal_cache);
                }
                ingest_candidate(&root, &mut state, &sig, engine.open_turn_id());
                continue;
            }
            let (prompt, model) = signal_context(&sig);
            if let (Some(m), Some(s)) = (&model, &sig.session) {
                recorder.set_model(s.clone(), m.clone());
            }
            let closed = apply_signal(&root, &mut engine, &sig, prompt, now);
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

/// D4: a system sleep/wake halts `Instant::now()`'s progress relative to wall
/// time (the monotonic clock doesn't tick while asleep, the wall clock keeps
/// going) — beyond this much disagreement between the predicted and the live
/// wall clock, the offset is stale enough to re-anchor.
const DRIFT_REANCHOR_MS: u64 = 2_000;

/// Monotonic engine clock with a wall-clock offset for record stamps. The
/// offset is re-anchored on drift (D4) so a laptop sleep doesn't leave every
/// record after it misdated by the sleep duration; `max_wall_ms` is a floor
/// so a correction can never make a derived timestamp go backwards relative
/// to one already handed out.
struct Clock {
    start_mono: Instant,
    start_wall_ms: u64,
    max_wall_ms: Cell<u64>,
}

impl Clock {
    fn start() -> Self {
        let start_wall_ms = wall_now_ms();
        Clock {
            start_mono: Instant::now(),
            start_wall_ms,
            max_wall_ms: Cell::new(start_wall_ms),
        }
    }
    fn now_ms(&self) -> u64 {
        self.start_mono.elapsed().as_millis() as u64
    }
    /// Compare the live wall clock against what the current offset predicts;
    /// beyond `DRIFT_REANCHOR_MS` disagreement, recompute the offset so
    /// forthcoming `wall_ms` calls track real time again. Cheap (one
    /// `SystemTime::now()`), safe to call every loop iteration.
    fn reanchor(&mut self) {
        let elapsed = self.now_ms();
        let predicted = self.start_wall_ms + elapsed;
        let actual = wall_now_ms();
        if actual.abs_diff(predicted) > DRIFT_REANCHOR_MS {
            self.start_wall_ms = actual.saturating_sub(elapsed);
        }
    }
    fn wall_ms(&self, mono_ms: u64) -> u64 {
        let derived = self.start_wall_ms + mono_ms;
        let clamped = derived.max(self.max_wall_ms.get());
        self.max_wall_ms.set(clamped);
        clamped
    }
}

// ---- watch event draining (D9) ----------------------------------------------

/// Blocks up to `POLL` for the first watcher message, then drains everything
/// already queued behind it via non-blocking `try_recv` — a burst of many
/// `notify` events (or a genuine watcher error) is applied in full within one
/// loop iteration instead of trickling in one message per 250ms tick.
/// Returns `true` if the channel is confirmed disconnected (the watcher
/// thread is gone; the caller should stop recording).
#[allow(clippy::too_many_arguments)]
fn drain_watch_events(
    rx: &Receiver<Result<notify::Event, notify::Error>>,
    root: &Path,
    ignore_set: &IgnoreSet,
    pending: &mut HashSet<PathBuf>,
    last_event: &mut Option<Instant>,
    first_event: &mut Option<Instant>,
    git_hit: &mut bool,
    gitignore_dirty: &mut bool,
) -> bool {
    match rx.recv_timeout(POLL) {
        Ok(res) => apply_watch_result(
            res,
            root,
            ignore_set,
            pending,
            last_event,
            first_event,
            git_hit,
            gitignore_dirty,
        ),
        Err(RecvTimeoutError::Timeout) => {}
        Err(RecvTimeoutError::Disconnected) => return true,
    }
    loop {
        match rx.try_recv() {
            Ok(res) => apply_watch_result(
                res,
                root,
                ignore_set,
                pending,
                last_event,
                first_event,
                git_hit,
                gitignore_dirty,
            ),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => return true,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_watch_result(
    res: Result<notify::Event, notify::Error>,
    root: &Path,
    ignore_set: &IgnoreSet,
    pending: &mut HashSet<PathBuf>,
    last_event: &mut Option<Instant>,
    first_event: &mut Option<Instant>,
    git_hit: &mut bool,
    gitignore_dirty: &mut bool,
) {
    match res {
        Ok(event) => {
            for path in event.paths {
                // A `.gitignore` is filter *configuration*, not watched content,
                // so the rebuild trigger must not depend on its own ignore
                // verdict. Deriving it from `pending` (Watch-only) meant a
                // self-matching `.gitignore` — which correctly classifies
                // `Ignore` — could never announce its own edit, so changing it
                // (e.g. adding `!keep.log`) went unhonored until a restart.
                if path.file_name().and_then(|n| n.to_str()) == Some(".gitignore") {
                    *gitignore_dirty = true;
                }
                match classify(root, &path, ignore_set) {
                    Class::GitRef => *git_hit = true,
                    Class::Watch => {
                        pending.insert(path);
                        let at = Instant::now();
                        first_event.get_or_insert(at);
                        *last_event = Some(at);
                    }
                    Class::Ignore => {}
                }
            }
        }
        // D3: was `Ok(Err(_)) => {}` — a mid-run watcher error (inotify queue
        // overflow, watch-limit exhaustion on a newly created directory) was
        // silently dropped, leaving `doctor` reporting healthy through a real
        // recording gap. Route it through the same persisted-counter/DEGRADED
        // banner the snapshot-I/O taxonomy (D35) already surfaces.
        Err(e) => record_watch_error(root, &e.to_string()),
    }
}

/// Persist a watcher-level error into the DEGRADED taxonomy (D3) and warn
/// loudly on stderr. Reuses `record_io_failure`'s counter/path-list rather
/// than inventing a parallel one — `status`/`doctor` already surface it.
fn record_watch_error(root: &Path, cause: &str) {
    eprintln!("agentrec: watcher error: {cause}");
    let mut state = read_state(root);
    record_io_failure(&mut state, "<watcher>");
    if let Err(e) = write_state(root, &state) {
        eprintln!("agentrec: warning: failed to persist watcher-error state: {e}");
    }
}

// ---- change classification --------------------------------------------------

#[derive(Debug, PartialEq)]
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
    // opening turns (no feedback loop). `.git` HEAD/index/refs/packed-refs
    // feed git-turn classification; the rest of `.git` is noise. Item 3:
    // `packed-refs` sits alongside `refs/` — `git pack-refs`/`git gc`/some
    // clones move ref state there instead of (or in addition to) loose files
    // under `.git/refs/`, so a ref transition can be invisible without it,
    // misclassifying a git operation as a bare mutation burst.
    if comps[0] == ".git" {
        return match comps.get(1).copied() {
            Some("HEAD") | Some("ORIG_HEAD") | Some("index") | Some("refs")
            | Some("packed-refs") => Class::GitRef,
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

/// D9: both `IgnoreSet::build`'s and `Recorder::scan`'s walks previously
/// descended into `.git` and `.agentrec` (`.hidden(false)` with no override
/// left them unpruned) — on a repo with real history / a grown object store
/// that's a full walk of every loose git/agentrec blob for no reason, and for
/// `Recorder::scan` it meant agentrec's own object files ended up in `known`
/// as if they were ordinary repo content. `filter_entry` returns `false` to
/// prune a subtree before the walker descends into it (event-time denylisting
/// in `classify` already excludes them from *watched* changes; this excludes
/// them from these one-shot scans too).
fn prune_git_and_agentrec(entry: &ignore::DirEntry) -> bool {
    !matches!(entry.file_name().to_str(), Some(".git") | Some(".agentrec"))
}

impl IgnoreSet {
    fn build(root: &Path) -> Self {
        let mut matchers = vec![];
        // Probe each directory the walk reaches for its own `.gitignore`, rather
        // than waiting for the walk to *yield* that file. The walk applies
        // gitignore rules to its own results, so a `.gitignore` whose rules match
        // itself (`*` — what tool-generated cache dirs ship) was filtered out of
        // the walk, no matcher was built for its directory, and nothing beneath
        // it was ever filtered.
        //
        // Directories the walk prunes are already excluded by an ancestor rule or
        // the `.git`/`.agentrec` denylist, so probing only reached directories
        // loses no coverage — and still never descends into (e.g.) node_modules
        // to collect a stray `.gitignore`.
        for entry in ignore::WalkBuilder::new(root)
            .hidden(false)
            .parents(false)
            .filter_entry(prune_git_and_agentrec)
            .build()
            .flatten()
        {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let dir = entry.path();
            let gitignore = dir.join(".gitignore");
            if !gitignore.is_file() {
                continue;
            }
            let mut builder = ignore::gitignore::GitignoreBuilder::new(dir);
            if builder.add(&gitignore).is_none() {
                if let Ok(gi) = builder.build() {
                    matchers.push((dir.to_path_buf(), gi));
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

/// Rebuild the `IgnoreSet` iff `dirty` is set, clearing it in the same call.
/// Exists so the loop tick's rate rule — at most one full-repo walk per
/// `POLL` — has a unit-reachable target: `run`'s loop body itself is not
/// unit-testable, which is exactly how the unconsumed-flag bug survived a
/// prior round undetected.
fn maybe_rebuild(dirty: &mut bool, root: &Path) -> Option<IgnoreSet> {
    if !*dirty {
        return None;
    }
    *dirty = false;
    Some(IgnoreSet::build(root))
}

/// Persist + log a completed ignore-set rebuild (Phase 2 of the
/// rebuild-gate fix — this class shipped twice partly because nothing
/// reported whether a reload ever happened). Bumps `state.json`'s
/// `ignore_rebuilds`/`last_ignore_rebuild_ms` and prints one stderr line
/// naming the matcher count, mirroring `drain_io_failures`'s
/// read-mutate-log-write shape. Called only from the `Some(fresh)` arm of
/// `maybe_rebuild`'s caller, so the increment tracks REBUILDS, never events.
fn log_ignore_rebuild(root: &Path, matcher_count: usize, wall_ms: u64) {
    let mut state = read_state(root);
    record_ignore_rebuild(&mut state, wall_ms);
    eprintln!("agentrec: ignore rules reloaded ({matcher_count} matchers)");
    if let Err(e) = write_state(root, &state) {
        eprintln!("agentrec: warning: failed to persist ignore-rebuild state: {e}");
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
    /// Count of file-change events skipped this batch because the OS path
    /// is not valid UTF-8 (item 2, non-UTF8 path handling) — no path list
    /// (there is no valid `String` form to store), drained into
    /// `state.json`'s `non_utf8_path_skips` the same way `io_failures` is.
    non_utf8_skips: u64,
}

impl Recorder {
    fn scan(root: &Path, store: BlobStore) -> Self {
        // Record which paths exist now; we do NOT hash contents (that would
        // snapshot the whole repo). The first edit of an unhashed pre-existing
        // file therefore has an unrecoverable `before` → baseline_unknown.
        let mut known = HashSet::new();
        for entry in ignore::WalkBuilder::new(root)
            .hidden(false)
            .filter_entry(prune_git_and_agentrec)
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
            non_utf8_skips: 0,
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
            // Item 2 (non-UTF8 path handling): a path whose raw OS bytes
            // aren't valid UTF-8 has no valid String form for the wire
            // record (paths serialize as JSON strings). Skip it outright —
            // `to_string_lossy()` would substitute U+FFFD and silently
            // record a DIFFERENT path than the one that actually changed,
            // breaking every later hash lookup keyed on that string (undo,
            // blame). Never snapshot, never emit a ChangeObs for it; just
            // count the skip so `status`'s DEGRADED surfacing sees it.
            let Some(rel_str) = agentrec_core::pathenc::utf8_path(&rel) else {
                self.non_utf8_skips += 1;
                continue;
            };
            let rel_str = rel_str.to_string();
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

            // Compute the `after` snapshot. The 4th element is
            // `FileEntry::skipped_reason` (SR-B): which of the three genuinely
            // different causes made `snapshotted` false. SR-C: whenever the
            // content bytes were actually read (over-cap, I/O-failed-write),
            // a hash IS computable even though nothing was stored — record it
            // as `after` so `modified-since` doesn't false-positive forever on
            // an unmodified skipped file. `unreadable` never has bytes, so its
            // `after` honestly stays `None` — never fabricate a hash we don't
            // have.
            let (after, snapshotted, withheld, skip_cause) = if secret {
                (None, false, true, None) // never snapshotted (D31)
            } else if deleted {
                (None, true, false, None) // delete: no content, but not "skipped"
            } else if is_symlink {
                // Not followed (AC B5): snapshot the link *target string*, so the
                // symlink change is recorded without reading the pointed-to file.
                symlink_change(&self.store, std::fs::read_link(abs))
            } else {
                match std::fs::read(abs) {
                    Ok(bytes) if bytes.len() <= MAX_SNAPSHOT_BYTES => {
                        match self.store.put_result(&bytes) {
                            PutResult::Stored(h) => (Some(h), true, false, None),
                            // Unreachable given the `<= MAX_SNAPSHOT_BYTES` guard
                            // above (store.put_result's own over-cap check can
                            // never trip here) — handled anyway, symmetrically
                            // with the pre-check arm below, per SR-B/C.
                            PutResult::OverCap => (
                                Some(hash_bytes(&bytes)),
                                false,
                                false,
                                Some(skip_reason::OVER_CAP.to_string()),
                            ),
                            PutResult::IoError(cause) => {
                                self.io_failures.push((rel_str.clone(), cause));
                                // The write failed, but the bytes were read
                                // successfully — the content hash is still
                                // honestly knowable (SR-C).
                                (
                                    Some(hash_bytes(&bytes)),
                                    false,
                                    false,
                                    Some(skip_reason::IO_FAILED.to_string()),
                                )
                            }
                        }
                    }
                    // Over cap: bytes were read (hash computable) but never
                    // stored (SR-C).
                    Ok(bytes) => (
                        Some(hash_bytes(&bytes)),
                        false,
                        false,
                        Some(skip_reason::OVER_CAP.to_string()),
                    ),
                    // Unreadable: no bytes were ever obtained, so no hash can
                    // be honestly recorded (SR-C — never fabricate one).
                    Err(_) => (
                        None,
                        false,
                        false,
                        Some(skip_reason::UNREADABLE.to_string()),
                    ),
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
                skip_reason: skip_cause,
            });
        }
        out
    }

    /// Turn a closed turn's observations into wire file entries.
    fn resolve(&self, obs: &ChangeObs) -> FileEntry {
        let after = self.after.get(&obs.path).cloned().flatten();
        let before = obs.before_hash.clone();
        // `op` keys on whether the file is actually gone, NOT on `after.is_none()`
        // — an unreadable *modify* has no `after` snapshot yet still exists
        // (an over-cap modify DOES have an `after` hash since SR-C — content was
        // read even though it wasn't stored), and neither must ever be logged
        // as a delete.
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
            skipped_reason: obs.skip_reason.clone(),
        }
    }
}

/// Classify a symlink's `read_link` outcome into `stage`'s
/// `(after, snapshotted, withheld, skip_reason)` shape. Pulled out to a
/// standalone, deterministically-testable function rather than inlined:
/// the failure arm is a TOCTOU race (the dirent can vanish, or change kind,
/// in the gap between the `symlink_metadata` check above and this
/// `read_link` call) that real threads can't be made to reliably win in a
/// test, but the classification logic itself — a `read_link` error is the
/// `unreadable` cause, exactly like a regular file's unreadable `fs::read`
/// (finding #2: previously left `skip_reason: None`, rendering as the
/// unclassified "reason unrecorded" instead of naming the plainly-known
/// cause) — is ordinary pure logic that doesn't need the race to verify.
fn symlink_change(
    store: &BlobStore,
    read_result: std::io::Result<std::path::PathBuf>,
) -> (Option<String>, bool, bool, Option<String>) {
    match read_result {
        Ok(target) => (
            store.put(target.to_string_lossy().as_bytes()),
            true,
            false,
            None,
        ),
        Err(_) => (
            None,
            false,
            false,
            Some(skip_reason::UNREADABLE.to_string()),
        ),
    }
}

/// Drain `recorder`'s pending snapshot failures (D35 I/O failures, item 2
/// non-UTF8 path skips) into `state.json`: bump the counters, track I/O
/// failure paths, and warn loudly on stderr. One read-mutate-write cycle for
/// both (mirrors the SignalTailer offset pattern) rather than two, so a
/// batch that hits both kinds doesn't race itself across two separate
/// state.json writes.
fn drain_io_failures(root: &Path, recorder: &mut Recorder) {
    if recorder.io_failures.is_empty() && recorder.non_utf8_skips == 0 {
        return;
    }
    let mut state = read_state(root);
    for (path, cause) in recorder.io_failures.drain(..) {
        record_io_failure(&mut state, &path);
        eprintln!("agentrec: snapshot write failed for {path}: {cause}");
    }
    if recorder.non_utf8_skips > 0 {
        for _ in 0..recorder.non_utf8_skips {
            record_non_utf8_path_skip(&mut state);
        }
        eprintln!(
            "agentrec: skipped {} file change(s) with a non-UTF8 path (undo/blame cannot track them)",
            recorder.non_utf8_skips
        );
        recorder.non_utf8_skips = 0;
    }
    if let Err(e) = write_state(root, &state) {
        eprintln!("agentrec: warning: failed to persist snapshot-failure state: {e}");
    }
}

// ---- signals ----------------------------------------------------------------

/// Tails `.agentrec/signal.jsonl` from a persisted byte offset. A torn trailing
/// line (no newline yet) is left unconsumed; the offset never regresses.
struct SignalTailer {
    offset: u64,
}

impl SignalTailer {
    /// D7: seek to the persisted offset and read only the fresh tail, instead
    /// of re-reading the whole inbox every ~250ms (O(file) per poll, on a
    /// file that only grows). Also detects external truncation (`len <
    /// offset` — someone rewrote or corrupted signal.jsonl out from under
    /// us): the old code sliced `text[offset..]` on a file shorter than
    /// `offset`, which panics; resuming from a reset offset=0 would instead
    /// *replay* already-consumed signals as duplicate turns. Neither is
    /// acceptable — we resync to the new end, log loudly, and count it as an
    /// I/O failure (DEGRADED-visible), accepting that whatever was written
    /// during the truncated window is lost.
    fn poll(&mut self, root: &Path) -> Vec<SignalEvent> {
        let path = signal_path(root);
        let Ok(mut file) = std::fs::File::open(&path) else {
            return vec![];
        };
        let Ok(len) = file.metadata().map(|m| m.len()) else {
            return vec![];
        };
        if len < self.offset {
            eprintln!(
                "agentrec: signal.jsonl truncated externally (was {} bytes, now {len}) — \
                 resuming from the new end; any signals written during the gap are lost",
                self.offset
            );
            self.offset = len;
            let mut state = read_state(root);
            state.signal_offset = self.offset;
            record_io_failure(&mut state, "signal.jsonl (truncated)");
            if let Err(e) = write_state(root, &state) {
                eprintln!("agentrec: warning: failed to persist signal offset: {e}");
            }
            return vec![];
        }
        if len == self.offset {
            return vec![];
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return vec![];
        }
        let mut fresh = Vec::new();
        if file.read_to_end(&mut fresh).is_err() {
            return vec![];
        }
        // Only consume up to the last complete line.
        let Some(nl) = fresh.iter().rposition(|b| *b == b'\n') else {
            return vec![]; // no complete line yet
        };
        let consumable = &fresh[..=nl];
        let events = parse_signals(&String::from_utf8_lossy(consumable));
        self.offset += (nl + 1) as u64;
        let mut state = read_state(root);
        state.signal_offset = self.offset;
        if let Err(e) = write_state(root, &state) {
            eprintln!("agentrec: warning: failed to persist signal offset: {e}");
        }
        events
    }
}

/// Memory-candidate ingestion (design spec §Write path/Agent-emitted;
/// PROTOCOL §4). Runs on the daemon thread — the only writer of both
/// `memory.jsonl` (via `append_memory`) and `state.json`'s `memory_rejects`
/// counter, so no lock/TOCTOU concerns beyond what already applies elsewhere
/// in this file.
///
/// Trust boundary (design spec, "Rejected approaches — emitter-side
/// hashing"): the candidate carries pin PATHS only; hashing happens HERE,
/// now, against the current working tree — never trust a hash the emitter
/// computed, since the file may have changed between emit and ingest.
///
/// Order: validate + hash every pin (any failure -> reject, nothing
/// persisted, INV-M1) -> scrub the fact (scrub-empty -> reject) -> dedup
/// against live (non-retracted) memories (an exact normalized-fact +
/// pin-path-set match is dropped silently — neither persisted nor counted
/// as a reject, a duplicate is not an error) -> append with `source_turns`
/// pointing at the enclosing open turn, if any. `append_memory` is the final
/// gate for the length/pins-count bounds (PINS_MAX, FACT_MAX_CHARS) and
/// re-scrubs before writing (idempotent on already-scrubbed text, matching
/// the prompt-persistence discipline elsewhere in this file); any of its
/// refusals also counts as a reject here.
/// Candidate-only startup replay. Scans `signal.jsonl` from the persisted
/// `signal_offset` (end of what the previous daemon session consumed) up to the
/// current EOF — the window of signals that arrived while no daemon was running
/// — and routes ONLY memory-candidate lines through `ingest_candidate`. Every
/// start/stop signal in that same pre-daemon window is intentionally dropped.
///
/// This is the one narrow relaxation of D7 (`SignalTailer::open`'s blanket
/// EOF-skip), and it is safe precisely where D7's rationale does not apply:
/// D7 skips the gap because replaying a stale start/stop "would mint an empty
/// turn misdated to daemon-boot". A memory-candidate mints no turn at all —
/// `ingest_candidate` never opens/closes a turn, stamps the record with the
/// signal's own `ts` (not boot time), and dedups an already-ingested fact to a
/// silent no-op — so replaying candidates is turn-neutral and idempotent across
/// restarts, while start/stop lines here still never reach the engine.
///
/// Returns the offset consumed up to (the last complete line). The live tailer
/// adopts this as its starting offset, so within a single boot the startup scan
/// and the live tailer never read the same line twice. The advanced offset is
/// also persisted, so an immediate restart doesn't re-scan the same window
/// (dedup would no-op it, but advancing avoids the repeated work).
fn replay_pending_candidates(root: &Path, current_turn: Option<&str>) -> u64 {
    let Ok(mut file) = std::fs::File::open(signal_path(root)) else {
        // No inbox yet — same starting point as the historical open() path.
        return 0;
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return 0;
    };
    let mut state = read_state(root);
    let start = state.signal_offset;
    if len <= start {
        // Nothing appended since the last consumed offset. (len < start is an
        // external shrink we can't recover — sit at the new EOF, matching the
        // historical open() behaviour of starting at the current end.)
        return len;
    }
    if file.seek(SeekFrom::Start(start)).is_err() {
        return len;
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return len;
    }
    // Only replay complete lines; a torn final line is left for the live tailer
    // to complete and process (it re-reads from `start` in that case).
    let Some(nl) = buf.iter().rposition(|b| *b == b'\n') else {
        return start;
    };
    // Kill-switch (design spec line 184, binding): `memory_enabled = false`
    // disables injection AND candidate ingestion, including this pre-daemon
    // replay window. One read for the whole replay batch — same rationale as
    // the live loop above.
    let memory_enabled = memorycmds::read_memory_enabled(root);
    for sig in parse_signals(&String::from_utf8_lossy(&buf[..=nl])) {
        // D7 preserved: ONLY candidate lines are acted on; start/stop (and any
        // other) signals in the pre-daemon gap are dropped, never fed to the
        // engine, so no phantom turn can be minted here.
        if sig.is_memory_candidate() && memory_enabled {
            ingest_candidate(root, &mut state, &sig, current_turn);
        }
    }
    let consumed = start + (nl as u64) + 1;
    state.signal_offset = consumed;
    if let Err(e) = write_state(root, &state) {
        eprintln!("agentrec: warning: failed to persist replayed signal offset: {e}");
    }
    consumed
}

fn ingest_candidate(root: &Path, state: &mut State, sig: &SignalEvent, current_turn: Option<&str>) {
    let raw_pins: &[String] = sig.pins.as_deref().unwrap_or(&[]);
    let mut pins = Vec::with_capacity(raw_pins.len());
    for raw in raw_pins {
        let validated = match agentrec_core::memory::validate_pin_path(root, raw) {
            Ok(v) => v,
            Err(_) => return reject_candidate(root, state),
        };
        let hash = match agentrec_core::memory::hash_pin(root, &validated) {
            Ok(h) => h,
            Err(_) => return reject_candidate(root, state),
        };
        pins.push(agentrec_core::memory::Pin {
            path: validated,
            hash,
        });
    }

    let fact = sig.fact.clone().unwrap_or_default();
    let scrubbed = scrub::scrub(&fact);
    if scrubbed.trim().is_empty() {
        return reject_candidate(root, state);
    }

    let norm_fact = normalize_fact(&scrubbed);
    let pin_set: HashSet<&str> = pins.iter().map(|p| p.path.as_str()).collect();
    if let Ok(existing) = agentrec_core::memory::load_effective(root) {
        let is_dup = existing.iter().any(|m| {
            !m.retracted
                && normalize_fact(&m.fact) == norm_fact
                && m.pins
                    .iter()
                    .map(|p| p.path.as_str())
                    .collect::<HashSet<_>>()
                    == pin_set
        });
        if is_dup {
            return; // silent drop — a duplicate is not an error (not a reject)
        }
    }

    let rec = agentrec_core::memory::MemoryRecord {
        v: 1,
        kind: "memory".to_string(),
        id: agentrec_core::id::ulid(),
        op: agentrec_core::memory::MemoryOp::Assert,
        fact,
        pins,
        source_turns: current_turn
            .map(|t| vec![t.to_string()])
            .unwrap_or_default(),
        origin: "agent".to_string(),
        ts: sig.ts,
        reason: None,
    };

    // F4: route through the shared memory.lock choke point like every other
    // writer — the daemon holds `daemon.lock` for its whole lifetime, so
    // reusing it here would deadlock this call against a live `purge
    // --memories-retracted` forever instead of just making it wait.
    match crate::memlock::append_memory_locked(root, &rec) {
        Ok(()) => test_pause_after_candidate_persist(),
        Err(_) => reject_candidate(root, state),
    }
}

/// Test-only crash-window widener (`cli/tests/hardening_daemon.rs`,
/// `dangling_source_turns_closed_by_pre_persist_journal_sync`). The real
/// D-M6 race — a candidate's `source_turns` fsynced before the open turn it
/// names is journaled — is a same-iteration, sub-millisecond window; no
/// external test process can land a SIGKILL inside it reliably. When
/// `AGENTREC_TEST_PAUSE_AFTER_CANDIDATE_MS` is set (only ever done by that
/// test), this sleeps for the given duration immediately after a candidate
/// is durably persisted, holding the daemon inside the exact window the fix
/// closes long enough for a deterministic external kill. The env read is
/// compiled out entirely in release builds (`#[cfg(not(debug_assertions))]`
/// arm always returns `None` without touching the environment) — same
/// fail-safe class as `agentrec_core::memory::test_slow_pin_read_delay`: a
/// release/production daemon can never have an arbitrary sleep injected
/// into its candidate-ingestion path. It stays active under
/// `cfg(debug_assertions)`, which `cargo test` sets and which
/// `cli/tests/hardening_daemon.rs`'s spawned `CARGO_BIN_EXE_agentrec`
/// (always a debug build) inherits, so the seam still bites in the test
/// suite.
fn test_pause_after_candidate_persist_delay() -> Option<Duration> {
    #[cfg(debug_assertions)]
    {
        std::env::var("AGENTREC_TEST_PAUSE_AFTER_CANDIDATE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

fn test_pause_after_candidate_persist() {
    if let Some(delay) = test_pause_after_candidate_persist_delay() {
        std::thread::sleep(delay);
    }
}

/// Lowercase + whitespace-collapse, for order-independent-of-spacing dedup
/// key comparison (design spec's dedup normalization).
fn normalize_fact(fact: &str) -> String {
    fact.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn reject_candidate(root: &Path, state: &mut State) {
    state.memory_rejects += 1;
    if let Err(e) = write_state(root, state) {
        eprintln!("agentrec: warning: failed to persist memory-reject state: {e}");
    }
}

fn apply_signal(
    root: &Path,
    engine: &mut TurnEngine,
    sig: &SignalEvent,
    prompt: Option<String>,
    now: u64,
) -> Vec<ClosedTurn> {
    // `prompt` is already resolved (hook-provided or transcript-extracted);
    // persist() scrubs before anything reaches disk (idempotent, AC I4).
    if sig.is_start() {
        return engine.observe_start(now, &sig.tool, prompt, sig.session.clone());
    }
    // F5 / PROTOCOL §10 additive-versioning: a signal carrying a `type` this
    // consumer doesn't recognize MUST be tolerated, never reinterpreted as a
    // stop. `memory-candidate` is already routed away before this function is
    // ever called (see the poll loop above), so any `kind` still present here
    // is, by construction, unknown — but this check does not lean on that
    // routing: it re-derives "unknown" locally (`Some(kind)` where
    // `kind != "memory-candidate"`) so `apply_signal` stays correct even if a
    // future caller stops pre-filtering. A signal with NO `type` at all
    // (every legacy start/stop producer, and the real Stop hook payload) is
    // the only shape that may fall through to the stop arm — that path is
    // unchanged.
    if let Some(kind) = sig.kind.as_deref() {
        if kind != "memory-candidate" {
            record_unknown_signal(root);
            return Vec::new();
        }
    }
    engine.observe_stop(now, &sig.tool, prompt, sig.session.clone())
}

/// Persist a forward-compat "unknown signal type" drop (F5, PROTOCOL §10
/// additive-versioning). Mirrors `reject_candidate`'s counter mechanism — a
/// dropped signal leaves no trace in `log.jsonl`/`memory.jsonl`, so this
/// counter is the only visible evidence it happened. Not a DEGRADED
/// condition: an unrecognized-but-tolerated signal is expected forward
/// compatibility, not a daemon health problem.
fn record_unknown_signal(root: &Path) {
    let mut state = read_state(root);
    state.unknown_signal_ignored += 1;
    if let Err(e) = write_state(root, &state) {
        eprintln!("agentrec: warning: failed to persist unknown-signal state: {e}");
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
                // D35 gap closure: distinguish a genuine I/O failure from an
                // over-cap prompt (both leave prompt_ref: None on the wire,
                // same as `BlobStore::put`) so it can be counted separately
                // from file-snapshot failures — see `prompt_put_failures`.
                let prompt_ref = match recorder.store.put_result(full.as_bytes()) {
                    PutResult::Stored(hash) => Some(hash),
                    PutResult::OverCap => None,
                    PutResult::IoError(cause) => {
                        let mut state = read_state(root);
                        record_prompt_put_failure(&mut state);
                        if let Err(e) = write_state(root, &state) {
                            eprintln!(
                                "agentrec: warning: failed to persist prompt-failure state: {e}"
                            );
                        }
                        eprintln!(
                            "agentrec: prompt write failed for turn {}: {cause}",
                            turn.id
                        );
                        None
                    }
                };
                (prompt_ref, Some(excerpt))
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
    /// The id this turn was reserved under at open (`TurnEngine::open_turn_id`)
    /// — recovery must reuse it, not mint a fresh one, or an in-flight
    /// memory-candidate ingested against this turn (`source_turns`) would be
    /// left pointing at an id that never appears in `log.jsonl`.
    /// `#[serde(default = "turn_id")]` keeps a journal from a pre-this-change
    /// daemon binary still parseable across an upgrade.
    #[serde(default = "turn_id")]
    id: String,
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
                id: snap.id,
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

    let ended_ms = journal.last_change_wall_ms.max(journal.opened_wall_ms);
    let started = rfc3339(journal.opened_wall_ms);
    let ended = rfc3339(ended_ms);

    // D8: a kill-9 landing between this function's own `append_log` and its
    // `remove_file(&path)` a few lines down (or the equivalent window in the
    // steady-state journal-close path) leaves the journal on disk describing
    // a turn that's ALREADY in log.jsonl. Recovering it again on the next
    // startup would double-log the same turn — and since the id was reserved
    // at OPEN, that duplicate carries the SAME id, breaking `undo` with an
    // "ambiguous turn id". Idempotent by construction: skip if a turn with
    // this reserved id (or, for legacy pre-reserved-id journals, this exact
    // root/start/end/file set) already sits in the log tail. See
    // `already_logged` for why the id key is load-bearing (`ended` drifts
    // between the persist path and recovery).
    if already_logged(root, &journal, &started, &ended) {
        let _ = std::fs::remove_file(&path);
        eprintln!(
            "agentrec: crash journal matches an already-logged turn — skipping duplicate recovery"
        );
        return Ok(());
    }

    // A bare recovery carries no attribution; a rich one keeps tool + prompt.
    let (tool, session, prompt_ref, prompt_excerpt) = if grade == "bare" {
        (None, None, None, None)
    } else {
        let store = BlobStore::new(objects_dir(root));
        // D35 gap closure: same IoError-vs-OverCap distinction as the
        // steady-state `persist` path — a crash-recovery prompt write can
        // fail for the same real-world reasons (disk full, permissions).
        let (prompt_ref, excerpt) = match &journal.prompt {
            Some(text) => {
                let scrubbed = scrub::scrub(text);
                let prompt_ref = match store.put_result(scrubbed.as_bytes()) {
                    PutResult::Stored(hash) => Some(hash),
                    PutResult::OverCap => None,
                    PutResult::IoError(cause) => {
                        let mut state = read_state(root);
                        record_prompt_put_failure(&mut state);
                        if let Err(e) = write_state(root, &state) {
                            eprintln!(
                                "agentrec: warning: failed to persist prompt-failure state: {e}"
                            );
                        }
                        eprintln!(
                            "agentrec: prompt write failed recovering orphaned turn {}: {cause}",
                            journal.id
                        );
                        None
                    }
                };
                (prompt_ref, Some(scrub::excerpt(text)))
            }
            None => (None, None),
        };
        (
            journal.tool.clone(),
            journal.session.clone(),
            prompt_ref,
            excerpt,
        )
    };

    let record = TurnRecord {
        v: 1,
        id: journal.id.clone(),
        grade: grade.to_string(),
        truncated,
        started,
        ended,
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

/// D8: does the log tail already contain the turn this journal describes?
/// Bounded to the most recent 50 records — a duplicate-recovery replay is
/// always near the tail (it can only happen across back-to-back crashes),
/// so there's no reason to scan the whole history.
///
/// Idempotent by turn `id` first (the canonical unique key, stable across
/// open -> journal -> recovery now that ids are RESERVED AT OPEN): if a turn
/// with this reserved id is already logged, this journal is a replay of a
/// close/recovery that already landed — even when the logged turn's `ended`
/// drifted from the journal's recomputed one (a turn closed by an incoming
/// start signal is logged at the start time, not `last_change`). The
/// root+times+files match is retained as a fallback for journals written by
/// a pre-reserved-id daemon binary across an upgrade: their `id` deserializes
/// to a fresh ULID (`#[serde(default)]`) that can't match anything logged, so
/// the original content-based D8 guarantee still holds for them.
fn already_logged(root: &Path, journal: &OrphanJournal, started: &str, ended: &str) -> bool {
    let records = agentrec_core::record::load_log(&log_path(root));
    records.iter().rev().take(50).any(|r| match r {
        LogRecord::Turn(t) => {
            t.id == journal.id
                || (t.root == journal.root
                    && t.started == started
                    && t.ended == ended
                    && files_match(&t.files, &journal.files))
        }
        LogRecord::Epoch(_) => false,
    })
}

/// Same set of paths, order-independent (the journal's file order need not
/// match the logged turn's).
fn files_match(a: &[FileEntry], b: &[FileEntry]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let set_a: HashSet<&str> = a.iter().map(|f| f.path.as_str()).collect();
    let set_b: HashSet<&str> = b.iter().map(|f| f.path.as_str()).collect();
    set_a == set_b
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

/// Per-root advisory lock file (D2): `flock(LOCK_EX | LOCK_NB)` on this path
/// is the actual mutual-exclusion mechanism, replacing the old
/// check-state-then-write-state pid dance — which raced two daemons starting
/// concurrently (both could observe "no live pid" before either wrote its
/// own) and, separately, treated a *recycled* pid (a new, unrelated process
/// that happened to reuse a dead daemon's pid) as "still alive". The OS
/// arbitrates `flock` atomically; there is no window for two winners.
fn lock_path(root: &Path) -> PathBuf {
    crate::agentrec_dir(root).join("daemon.lock")
}

/// Acquire the per-root recorder lock (AC B1, hardened D2). Returns the open
/// `File` — the caller MUST bind it to a named local held for the daemon's
/// entire lifetime (`let _lock = acquire_lock(...)?;`); dropping it early
/// closes the fd and releases the flock immediately. `state.json`'s `pid`
/// field is still set here, but purely for the refusal message and `status`
/// display — it is never consulted to decide whether the lock is held.
fn acquire_lock(root: &Path) -> Result<std::fs::File, String> {
    let path = lock_path(root);
    let file = std::fs::OpenOptions::new()
        .create(true)
        // Never truncate: this file's only role is to be `flock`'d — clearing
        // its (currently unused) content on every open has no benefit and
        // needlessly risks racing a concurrent reader of it.
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("cannot open lock file {}: {e}", path.display()))?;
    agentrec_core::perms::lock_file(&path);
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let pid = read_state(root).pid;
        let who = if pid != 0 {
            format!(" (pid {pid})")
        } else {
            String::new()
        };
        return Err(format!(
            "already recording {}{who} — stop it first",
            root.display()
        ));
    }
    let mut state = read_state(root);
    state.pid = std::process::id();
    // D2: persistence failure here is fatal — an "acquired" lock the daemon
    // can't record its own pid into would still function (the flock IS the
    // gate), but silently leaving a stale/wrong pid on disk would mislead
    // every later refusal message and `doctor`/`status` display.
    write_state(root, &state)
        .map_err(|e| format!("acquired recorder lock but failed to write state.json: {e}"))?;
    Ok(file)
}

fn release_lock(root: &Path) {
    let mut state = read_state(root);
    if state.pid == std::process::id() {
        state.pid = 0;
        if let Err(e) = write_state(root, &state) {
            eprintln!("agentrec: warning: failed to clear pid in state.json: {e}");
        }
    }
    // The flock itself releases when `_lock`'s `File` drops at the end of
    // `run()` (fd close) — nothing to do here for the actual mutex.
}

/// Non-blocking liveness probe (D2) for `doctor`: if we can acquire the lock
/// ourselves, nothing else holds it, so no daemon is recording. Used instead
/// of the old pid-liveness check, which false-passed when a *different*,
/// unrelated process recycled a dead daemon's pid. Fails safe in every
/// can't-determine case (missing/unopenable lock file, syscall error) by
/// reporting "not running" — a false healthy pass is the direction that
/// actually hides a problem from the user; a false failure is merely
/// annoying.
pub(crate) fn daemon_is_running(root: &Path) -> bool {
    let path = lock_path(root);
    // No `.create(true)`: a lock file that was never written means the
    // daemon has never run here — nothing to probe, and `doctor` must not
    // mutate disk as a side effect of a liveness check.
    let Ok(file) = std::fs::OpenOptions::new().write(true).open(&path) else {
        return false;
    };
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        // We just took the lock ourselves — release it immediately; we're
        // only probing, not claiming it.
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        false
    } else {
        true
    }
}

fn watch_error(e: &notify::Error) -> String {
    let msg = e.to_string();
    // D10: was `a || b && c`, which — `&&` binding tighter than `||` — parses
    // as `a || (b && c)`: any message merely mentioning "inotify" (with no
    // "limit" in sight, e.g. a permission error) wrongly got the raise-the-
    // limit remedy. Intent is `(a || b) && c`: only messages that actually
    // mention a limit get that specific remedy.
    if (msg.contains("inotify") || msg.contains("watch")) && msg.contains("limit") {
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

    /// Mirrors `memory::slow_pin_read_delay_is_none_in_release_even_with_env_set`:
    /// in a release build (`debug_assertions` off), the D-M6 pause seam must
    /// return `None` even when the env var IS set — proves the
    /// `#[cfg(not(debug_assertions))]` arm actually compiles out the env
    /// read rather than merely being unreachable dead code. Only runs under
    /// `cargo test --release` (the `debug_assertions`-on debug test build
    /// never exercises this arm at all).
    #[test]
    #[cfg(not(debug_assertions))]
    fn pause_after_candidate_persist_delay_is_none_in_release_even_with_env_set() {
        std::env::set_var("AGENTREC_TEST_PAUSE_AFTER_CANDIDATE_MS", "5000");
        assert_eq!(
            test_pause_after_candidate_persist_delay(),
            None,
            "release builds must never honor AGENTREC_TEST_PAUSE_AFTER_CANDIDATE_MS"
        );
        std::env::remove_var("AGENTREC_TEST_PAUSE_AFTER_CANDIDATE_MS");
    }

    // B+ / D29: nested `.gitignore` precedence follows git's rules — deeper
    // files override shallower ones, `!` re-includes, and a matched directory
    // ignores everything beneath it.
    //
    // `git init` here is hygiene, not a vacuity fix (Phase 3 of the
    // rebuild-gate plan): pre-D29, `IgnoreSet::build` relied on a
    // gitignore-aware walk to *yield* each `.gitignore`, so a non-git tempdir
    // (where `ignore::WalkBuilder::require_git` applies no rules at all)
    // still yielded both files and this test passed vacuously. Post-D29,
    // `build` probes `dir.join(".gitignore")` for every directory the walk
    // reaches instead, and a non-git walk still yields directories — so the
    // matchers were collected and these assertions were already real. `git
    // init` just makes the fixture match how the code runs in production
    // (where `require_git` also governs traversal pruning). The precondition
    // assert states what the test depends on.
    #[test]
    fn nested_gitignore_precedence() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let initialized = std::process::Command::new("git")
            .arg("init")
            .arg(root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(initialized, "git must be available to run this test");

        std::fs::write(root.join(".gitignore"), "*.log\nbuild/\n").unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/.gitignore"), "!keep.log\n*.tmp\n").unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();

        let set = IgnoreSet::build(root);
        eprintln!(
            "nested_gitignore_precedence: matchers.len() = {}",
            set.matchers.len()
        );
        assert!(
            set.matchers.len() >= 2,
            "expected at least the root and sub/ matchers, got {}",
            set.matchers.len()
        );
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

    // D29 regression: a `.gitignore` whose own rules match itself (`*`, the
    // pattern every tool-generated cache dir ships — `.remember/`,
    // `.code-review-graph/`, …) must still filter its directory.
    //
    // `IgnoreSet::build` collects `.gitignore` files with a gitignore-aware
    // walk, so a self-matching file hid itself from the walk: no matcher was
    // built for that directory and NOTHING beneath it was ever filtered. In
    // this repo's own live store that leaked 7419 file entries / 764 MiB —
    // 98% of referenced store bytes — including 64 snapshots of a 9 MiB SQLite.
    //
    // `git init` is load-bearing: `ignore::WalkBuilder::require_git` defaults to
    // true, so without a real repo NO ignore rules apply during the walk, every
    // `.gitignore` is yielded, and this test passes vacuously against the bug.
    #[test]
    fn self_matching_gitignore_still_filters_its_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let initialized = std::process::Command::new("git")
            .arg("init")
            .arg(root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(initialized, "git must be available to run this test");

        // Self-matching: `*` matches `.gitignore` itself.
        std::fs::create_dir_all(root.join("cache/logs")).unwrap();
        std::fs::write(root.join("cache/.gitignore"), "*\n").unwrap();
        // Control: a nested ignore that does NOT match itself.
        std::fs::create_dir_all(root.join("normal")).unwrap();
        std::fs::write(root.join("normal/.gitignore"), "*.log\n").unwrap();

        let set = IgnoreSet::build(root);
        let ig = |p: &str, is_dir: bool| set.is_ignored(&root.join(p), is_dir);

        assert!(
            ig("cache/logs/memory.log", false),
            "self-matching `*` must filter nested files"
        );
        assert!(
            ig("cache/session.pid", false),
            "self-matching `*` must filter direct children"
        );
        assert!(
            ig("normal/a.log", false),
            "control: non-self-matching nested ignore still works"
        );
        assert!(!ig("src.rs", false), "unignored source must stay watched");
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
            non_utf8_skips: 0,
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
            skip_reason: None,
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

    // SR2 (over_cap producer) + SR-C (after-hash honesty gain): a real
    // over-cap file, staged through the actual `Recorder::stage` production
    // path (not a synthetic `ChangeObs`) — bytes ARE read here, so the
    // resulting `FileEntry` must carry `skipped_reason: over_cap` AND a real
    // `after` hash matching the content, even though the blob itself was
    // never stored (the store never saw it — RED-relevant: pre-SR-C this
    // asserted `entry.after == None`, which made `modified_since` a
    // permanent false positive for every never-modified over-cap file).
    #[test]
    fn stage_over_cap_sets_reason_and_recoverable_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let store = BlobStore::new(root.join(".agentrec/objects"));
        let mut recorder = Recorder::scan(root, store);

        let content = vec![b'x'; MAX_SNAPSHOT_BYTES + 1];
        let file = root.join("huge.bin");
        std::fs::write(&file, &content).unwrap();

        let mut paths = HashSet::new();
        paths.insert(file);
        let changes = recorder.stage(&paths);
        assert_eq!(
            changes.len(),
            1,
            "must still produce an observation: {changes:?}"
        );
        assert!(!changes[0].snapshotted, "over cap must not be snapshotted");
        assert_eq!(
            changes[0].skip_reason.as_deref(),
            Some(skip_reason::OVER_CAP),
            "changes: {:?}",
            changes[0]
        );

        let entry = recorder.resolve(&changes[0]);
        assert!(entry.skipped);
        assert_eq!(entry.skipped_reason.as_deref(), Some(skip_reason::OVER_CAP));
        let expected_hash = hash_bytes(&content);
        assert_eq!(
            entry.after.as_deref(),
            Some(expected_hash.as_str()),
            "a computable hash must be recorded even though nothing was stored"
        );
        // Precondition this test depends on: the blob genuinely never landed
        // in the store (the honesty gain is recording a KNOWN hash, not
        // claiming content was captured).
        let store2 = BlobStore::new(root.join(".agentrec/objects"));
        assert!(
            store2.get(&expected_hash).is_err(),
            "over-cap content must never actually be stored"
        );
    }

    // SR2 (unreadable producer): no bytes were ever obtained, so `after`
    // must honestly stay `None` — never fabricate a hash for content that
    // was never read (SR-C's other half).
    #[cfg(unix)]
    #[test]
    fn stage_unreadable_sets_reason_and_no_hash() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let store = BlobStore::new(root.join(".agentrec/objects"));
        let mut recorder = Recorder::scan(root, store);

        let file = root.join("secret.rs");
        std::fs::write(&file, b"can't read me").unwrap();
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&file, perms).unwrap();

        let mut paths = HashSet::new();
        paths.insert(file.clone());
        let changes = recorder.stage(&paths);

        // restore perms so tempdir cleanup can remove it
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&file, perms).unwrap();

        assert_eq!(changes.len(), 1, "changes: {changes:?}");
        assert!(!changes[0].snapshotted);
        assert_eq!(
            changes[0].skip_reason.as_deref(),
            Some(skip_reason::UNREADABLE),
            "changes: {:?}",
            changes[0]
        );
        let entry = recorder.resolve(&changes[0]);
        assert!(entry.skipped);
        assert_eq!(
            entry.skipped_reason.as_deref(),
            Some(skip_reason::UNREADABLE)
        );
        assert_eq!(
            entry.after, None,
            "no bytes were ever read — a hash must never be fabricated"
        );
    }

    // Finding #2 (symlink producer, previously unclassified): a symlink
    // whose `read_link` fails (a TOCTOU race against the `symlink_metadata`
    // check that decided `is_symlink` — the dirent can vanish or change
    // kind in the gap between the two calls) must set `unreadable`, not
    // leave `skipped_reason: None` (which rendered as "reason unrecorded"
    // even though the cause — no bytes obtained — is exactly the same as
    // the regular-file unreadable case above). The race itself can't be
    // deterministically won against a real filesystem in a test, so this
    // exercises `symlink_change` — the actual production classification
    // function `stage` calls — directly with a synthetic `read_link`
    // error, rather than asserting on a flaky real race.
    #[test]
    fn stage_symlink_unreadable_sets_unreadable_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path().join("objects"));

        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "race: link vanished");
        let (after, snapshotted, withheld, skip_cause) = symlink_change(&store, Err(err));

        assert_eq!(
            after, None,
            "no bytes were ever read for a failed read_link — a hash must never be fabricated"
        );
        assert!(!snapshotted);
        assert!(!withheld);
        assert_eq!(skip_cause.as_deref(), Some(skip_reason::UNREADABLE));
    }

    // SR2 (io_failed producer): the write itself fails, but the bytes WERE
    // read successfully first — so a hash is knowable here too, same as the
    // over-cap case (a judgment call: the task text only worked through
    // over_cap/unreadable explicitly, but io_failed's bytes are equally in
    // hand at the point `put_result` returns `IoError`).
    #[cfg(unix)]
    #[test]
    fn stage_io_failed_sets_reason_and_recoverable_hash() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("work");
        std::fs::create_dir_all(&root).unwrap();
        let locked = tmp.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        // store dir does not exist yet; `locked` will be made read-only so
        // `put_result`'s `create_dir_all` of the fan-out dir must fail
        // (mirrors `store::tests::io_error_is_typed`).
        let store_dir = locked.join("store");
        let store = BlobStore::new(&store_dir);
        let mut recorder = Recorder::scan(&root, store);

        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o500);
        std::fs::set_permissions(&locked, perms).unwrap();

        let content = b"some real content".to_vec();
        let file = root.join("a.rs");
        std::fs::write(&file, &content).unwrap();
        let mut paths = HashSet::new();
        paths.insert(file);
        let changes = recorder.stage(&paths);

        // restore perms first so tempdir cleanup can remove `locked`
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&locked, perms).unwrap();

        assert_eq!(changes.len(), 1, "changes: {changes:?}");
        assert!(!changes[0].snapshotted);
        assert_eq!(
            changes[0].skip_reason.as_deref(),
            Some(skip_reason::IO_FAILED),
            "changes: {:?}",
            changes[0]
        );
        assert_eq!(
            recorder.io_failures.len(),
            1,
            "must record the D35 operational failure too"
        );

        let entry = recorder.resolve(&changes[0]);
        assert!(entry.skipped);
        assert_eq!(
            entry.skipped_reason.as_deref(),
            Some(skip_reason::IO_FAILED)
        );
        assert_eq!(
            entry.after.as_deref(),
            Some(hash_bytes(&content).as_str()),
            "the write failed, but the bytes were read — a hash is still knowable"
        );
    }

    // SR5: the false-positive fix, end to end at the producer level — an
    // over-cap file that is NEVER modified after being recorded must not be
    // reported as `modified_since` (the exact bug: `after` used to be `None`
    // forever, so `current_hash != None` was always true). This exercises
    // the real producer's output against `modified_since`'s actual
    // comparison rule (mirrored inline — `readcmds::modified_since` is
    // private to a different module — its rule is a one-line hash
    // comparison, asserted identically to its real implementation).
    #[test]
    fn sr5_over_cap_after_hash_makes_modified_since_false_for_unmodified_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let store = BlobStore::new(root.join(".agentrec/objects"));
        let mut recorder = Recorder::scan(root, store);

        let content = vec![b'z'; MAX_SNAPSHOT_BYTES + 1];
        let file = root.join("huge.bin");
        std::fs::write(&file, &content).unwrap();
        let mut paths = HashSet::new();
        paths.insert(file);
        let changes = recorder.stage(&paths);
        let entry = recorder.resolve(&changes[0]);
        assert!(
            entry.skipped,
            "precondition: this entry is over-cap-skipped"
        );

        // The file on disk is untouched since recording — re-hash it exactly
        // as `readcmds::modified_since` does for the live worktree.
        let current_hash = Some(hash_bytes(&content));
        let modified_since = current_hash.as_deref() != entry.after.as_deref();
        assert!(
            !modified_since,
            "an unmodified over-cap file must not be reported as modified: after={:?} current={:?}",
            entry.after, current_hash
        );
    }

    // Item 2 (non-UTF8 path handling): a path whose raw OS bytes are not
    // valid UTF-8 can never round-trip through a wire record (paths
    // serialize as JSON strings). `stage()` must skip it — never fall back
    // to `to_string_lossy()`, which would silently record a *different*
    // path than the one that actually changed and break undo/blame hash
    // lookups keyed on that string — and count the skip so `status`'s
    // DEGRADED surfacing sees it (the `snapshot_failures` honesty pattern:
    // a skip leaves no trace in `log.jsonl`). Linux-only: this needs to
    // actually create a file with invalid-UTF8 bytes in its name, which
    // macOS/APFS refuses at the syscall level ("Illegal byte sequence") —
    // the pure conversion decision itself (`pathenc::utf8_path`) is tested
    // cross-platform in agentrec-core without touching a real filesystem.
    #[cfg(target_os = "linux")]
    #[test]
    fn stage_skips_non_utf8_path_never_lossily_recorded() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let store = BlobStore::new(root.join("obj"));
        let mut recorder = Recorder::scan(root, store);

        // "fo\xFFo" — 0xFF is not valid UTF-8 in any position.
        let bytes = [0x66, 0x6f, 0xff, 0x6f];
        let bad_name = OsStr::from_bytes(&bytes);
        let bad_path = root.join(bad_name);
        std::fs::write(&bad_path, b"hello").unwrap();

        let mut paths = HashSet::new();
        paths.insert(bad_path);
        let changes = recorder.stage(&paths);

        assert!(
            changes.is_empty(),
            "a non-UTF8 path must never produce a ChangeObs: {changes:?}"
        );
        assert_eq!(recorder.non_utf8_skips, 1);
        assert!(
            recorder.io_failures.is_empty(),
            "distinct skip reason, not an I/O write failure"
        );
    }

    // A UTF-8 path containing non-ASCII (but valid) bytes is unaffected —
    // the bound is on VALID UTF-8, not on ASCII-only.
    #[test]
    fn stage_records_valid_non_ascii_utf8_path_normally() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let store = BlobStore::new(root.join("obj"));
        let mut recorder = Recorder::scan(root, store);

        let path = root.join("café.rs");
        std::fs::write(&path, b"fn main() {}").unwrap();

        let mut paths = HashSet::new();
        paths.insert(path);
        let changes = recorder.stage(&paths);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "café.rs");
        assert_eq!(recorder.non_utf8_skips, 0);
    }

    // ---- D2: flock-based lock -----------------------------------------------

    #[test]
    fn acquire_lock_refuses_a_second_holder_via_flock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();

        let _first = acquire_lock(root).expect("first acquire succeeds");
        let second = acquire_lock(root);
        assert!(
            second.is_err(),
            "a second acquire must be refused while the first holds the lock"
        );
        assert!(second.unwrap_err().contains("already recording"));
    }

    #[test]
    fn acquire_lock_succeeds_again_after_the_first_holder_drops() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();

        {
            let _first = acquire_lock(root).expect("first acquire succeeds");
        } // `_first` drops here — the flock is released with the fd close.
        let second = acquire_lock(root);
        assert!(
            second.is_ok(),
            "a fresh acquire after the holder released must succeed"
        );
    }

    #[test]
    fn daemon_is_running_false_when_lock_never_taken() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();
        assert!(!daemon_is_running(root));
    }

    #[test]
    fn daemon_is_running_true_while_held_false_after_release() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();

        let held = acquire_lock(root).unwrap();
        assert!(
            daemon_is_running(root),
            "lock is held — must report running"
        );
        drop(held);
        assert!(
            !daemon_is_running(root),
            "lock released — must report not running"
        );
    }

    // D2/D6: the old check was `state.pid != 0 && pid_alive(state.pid)` — a
    // *recycled* pid (an unrelated, live process that happens to reuse a dead
    // daemon's pid number) would false-pass. The flock probe must not be
    // fooled by a live-but-unrelated pid sitting in state.json.
    #[test]
    fn daemon_is_running_false_despite_a_live_recycled_pid_in_state() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();

        let mut state = read_state(root);
        state.pid = std::process::id(); // this test process is definitely alive
        write_state(root, &state).unwrap();

        assert!(
            !daemon_is_running(root),
            "a live-but-unrelated pid in state.json must not fake liveness"
        );
    }

    // ---- D4: wall-clock re-anchor on drift -----------------------------------

    #[test]
    fn reanchor_corrects_large_stale_offset() {
        // Simulate a Clock whose offset is wildly stale (as if the process had
        // slept for years) — `reanchor` must snap it back close to real wall
        // time instead of leaving every subsequent record misdated.
        let mut clock = Clock {
            start_mono: Instant::now(),
            start_wall_ms: 1_000, // ~1970, absurdly far in the past
            max_wall_ms: Cell::new(1_000),
        };
        let before = clock.wall_ms(0);
        clock.reanchor();
        let after = clock.wall_ms(0);
        assert!(
            after > before + 1_000,
            "reanchor should have corrected a multi-decade drift: before={before} after={after}"
        );
    }

    #[test]
    fn wall_ms_never_regresses_even_after_a_backward_correction() {
        let mut clock = Clock::start();
        let high = clock.wall_ms(10_000);
        // Simulate a backward-jumped reanchor (e.g. NTP correction) by
        // directly rewinding the offset, as `reanchor` itself would on a
        // backward system-clock jump.
        clock.start_wall_ms = clock.start_wall_ms.saturating_sub(50_000);
        let low = clock.wall_ms(10_000);
        assert_eq!(
            low, high,
            "a derived wall time must never regress below one already handed out"
        );
    }

    // ---- D10: watch_error precedence -----------------------------------------

    #[test]
    fn watch_error_requires_limit_keyword_not_just_inotify_mention() {
        let e = notify::Error::generic("inotify_add_watch failed: Permission denied");
        let msg = watch_error(&e);
        assert!(
            !msg.contains("raise the inotify limit"),
            "message has no 'limit' — must not claim a limit fix: {msg}"
        );
    }

    #[test]
    fn watch_error_flags_true_limit_exhaustion() {
        let e = notify::Error::generic("inotify_add_watch failed: limit reached");
        let msg = watch_error(&e);
        assert!(
            msg.contains("raise the inotify limit"),
            "message mentions both inotify and limit: {msg}"
        );
    }

    #[test]
    fn watch_error_flags_watch_plus_limit_without_inotify_word() {
        let e = notify::Error::generic("could not add watch: limit exceeded");
        let msg = watch_error(&e);
        assert!(msg.contains("raise the inotify limit"), "message: {msg}");
    }

    // ---- D3: watcher errors feed the persisted DEGRADED counter -------------

    #[test]
    fn record_watch_error_bumps_the_persisted_counter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();

        let before = read_state(root).snapshot_failures;
        record_watch_error(root, "inotify queue overflow");
        let after = read_state(root);
        assert_eq!(after.snapshot_failures, before + 1);
        assert!(after.io_failed.iter().any(|p| p == "<watcher>"));
    }

    // ---- D7: seek-based signal tail + truncation recovery --------------------

    #[test]
    fn signal_tailer_consumes_appended_signals_via_seek() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();
        std::fs::write(signal_path(root), b"").unwrap();

        let mut tailer = SignalTailer { offset: 0 };

        std::fs::write(
            signal_path(root),
            b"{\"v\":1,\"ts\":1,\"tool\":\"claude\",\"event\":\"start\"}\n",
        )
        .unwrap();
        let events = tailer.poll(root);
        assert_eq!(events.len(), 1);
        assert!(tailer.offset > 0);
    }

    // D7: external truncation must never replay from offset 0 (that would
    // double-log already-applied signals) — it resyncs to the new EOF,
    // warns loudly, and bumps the I/O-failure counter so
    // `doctor`/`status` surface it.
    #[test]
    fn signal_tailer_recovers_from_external_truncation_without_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();
        std::fs::write(signal_path(root), b"012345678901234567890").unwrap(); // 21 bytes

        let mut tailer = SignalTailer { offset: 21 };
        std::fs::write(signal_path(root), b"tiny\n").unwrap(); // 5 bytes — truncated

        let events = tailer.poll(root);
        assert!(
            events.is_empty(),
            "the truncation batch itself yields no events"
        );
        assert_eq!(tailer.offset, 5, "offset resets to the new EOF, never to 0");
        assert!(
            read_state(root).snapshot_failures >= 1,
            "truncation must bump the I/O-failure counter"
        );

        // A signal appended after the truncation is consumed cleanly from the
        // reset offset — recovery, not just detection.
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(signal_path(root))
            .unwrap();
        writeln!(
            f,
            "{{\"v\":1,\"ts\":2,\"tool\":\"claude\",\"event\":\"start\"}}"
        )
        .unwrap();
        let events2 = tailer.poll(root);
        assert_eq!(events2.len(), 1);
    }

    // Candidate-only startup replay: the scan ingests ONLY memory-candidate
    // lines from the pre-daemon gap (start/stop are skipped — D7 preserved for
    // turn boundaries), reconciles the offset to the scanned EOF so the live
    // tailer never re-reads within one boot, persists that offset, and is
    // idempotent across restarts (a re-scanned candidate dedups to a no-op).
    #[test]
    fn replay_pending_candidates_is_candidate_only_and_offset_reconciled() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();
        std::fs::write(root.join("notes.txt"), b"hello").unwrap();

        let start_line = r#"{"v":1,"ts":1,"tool":"claude","event":"start"}"#;
        let candidate = serde_json::json!({
            "v": 1, "ts": 1_700_000_000_000u64, "tool": "claude-code",
            "type": "memory-candidate", "fact": "a gap fact", "pins": ["notes.txt"],
        })
        .to_string();
        let stop_line = r#"{"v":1,"ts":2,"tool":"claude"}"#;
        let contents = format!("{start_line}\n{candidate}\n{stop_line}\n");
        std::fs::write(signal_path(root), contents.as_bytes()).unwrap();
        let eof = contents.len() as u64;

        // No state.json -> persisted offset 0: the whole file is the gap.
        let consumed = replay_pending_candidates(root, None);
        assert_eq!(
            consumed, eof,
            "scan consumes up to the last complete line (EOF)"
        );
        assert_eq!(
            read_state(root).signal_offset,
            eof,
            "advanced offset persisted so the live tailer resumes at EOF"
        );

        let mems = agentrec_core::memory::load_effective(root).unwrap();
        assert_eq!(
            mems.len(),
            1,
            "only the candidate is ingested; start/stop are skipped: {mems:?}"
        );
        assert_eq!(mems[0].fact, "a gap fact");
        assert_eq!(mems[0].origin, "agent");

        // Restart idempotency: force a full re-scan of a gap that now holds the
        // same candidate twice — dedup keeps memory.jsonl at one record.
        std::fs::write(
            signal_path(root),
            format!("{contents}{candidate}\n").as_bytes(),
        )
        .unwrap();
        let mut st = read_state(root);
        st.signal_offset = 0;
        write_state(root, &st).unwrap();
        replay_pending_candidates(root, None);
        let mems2 = agentrec_core::memory::load_effective(root).unwrap();
        assert_eq!(
            mems2.len(),
            1,
            "a re-scanned/duplicate candidate dedups to a no-op: {mems2:?}"
        );
    }

    // ---- Item 3: .git/packed-refs classification ------------------------------

    // `git pack-refs` / `git gc` / some clones move ref state into
    // `.git/packed-refs` instead of (or alongside) loose files under
    // `.git/refs/` — a ref transition that lands there must classify as a
    // git signal exactly like HEAD/index/refs, or a `git gc`/repack burst
    // gets misclassified as a bare human mutation burst (never let a git
    // operation become a fabricated bare turn). Sibling `.git` content
    // (logs, objects) must stay denied.
    #[test]
    fn classify_admits_packed_refs_as_git_signal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ignore_set = IgnoreSet::build(root);
        assert_eq!(
            classify(root, &root.join(".git/packed-refs"), &ignore_set),
            Class::GitRef
        );
    }

    #[test]
    fn classify_still_denies_other_git_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ignore_set = IgnoreSet::build(root);
        assert_eq!(
            classify(root, &root.join(".git/logs/HEAD"), &ignore_set),
            Class::Ignore
        );
        assert_eq!(
            classify(root, &root.join(".git/objects/ab/cdef"), &ignore_set),
            Class::Ignore
        );
        assert_eq!(
            classify(root, &root.join(".git/config"), &ignore_set),
            Class::Ignore
        );
    }

    // End to end: a mutation burst coinciding with a packed-refs transition
    // must classify as a `tool:"git"` rich turn, not a bare human burst —
    // chains the daemon's `classify()` decision straight into the real
    // engine (`agentrec_core::engine::TurnEngine`), the same way the main
    // loop does (`Class::GitRef` -> `observe_git_change`, `Class::Watch` ->
    // `observe_changes`).
    #[test]
    fn packed_refs_burst_classifies_as_git_rich_turn_via_engine() {
        use agentrec_core::engine::{ChangeObs, TurnEngine};

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ignore_set = IgnoreSet::build(root);
        let mut engine = TurnEngine::new();

        // A `git pack-refs`-shaped burst: many files touched by a checkout,
        // plus the packed-refs transition itself.
        let burst = [
            root.join("src/a.rs"),
            root.join("src/b.rs"),
            root.join(".git/packed-refs"),
        ];
        let mut git_hit = false;
        for p in &burst {
            match classify(root, p, &ignore_set) {
                Class::GitRef => git_hit = true,
                Class::Watch => { /* fed to observe_changes below */ }
                Class::Ignore => panic!("unexpected Ignore for {p:?}"),
            }
        }
        assert!(git_hit, "packed-refs must have set git_hit");

        engine.observe_git_change(0);
        let changes = vec![
            ChangeObs {
                path: "src/a.rs".into(),
                before_hash: Some("sha256:a".into()),
                snapshotted: true,
                withheld: false,
                baseline_unknown: false,
                deleted: false,
                skip_reason: None,
            },
            ChangeObs {
                path: "src/b.rs".into(),
                before_hash: Some("sha256:b".into()),
                snapshotted: true,
                withheld: false,
                baseline_unknown: false,
                deleted: false,
                skip_reason: None,
            },
        ];
        engine.observe_changes(500, &changes);
        let closed = engine.tick(500 + agentrec_core::GIT_SETTLE_MS + 1);

        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].tool.as_deref(), Some("git"));
        assert_eq!(closed[0].grade, "rich");
        assert_eq!(closed[0].boundary, "git");
        assert_eq!(closed[0].files.len(), 2);
    }

    // The watch-event path (not just the pure classifier) must flip
    // `git_hit` for a packed-refs event, the same signal HEAD/index/refs
    // already produce — this is the whole path a real notify event takes.
    #[test]
    fn drain_watch_events_sets_git_hit_on_packed_refs_transition() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let ignore_set = IgnoreSet::build(&root);
        let (tx, rx) = channel::<Result<notify::Event, notify::Error>>();

        tx.send(Ok(
            notify::Event::default().add_path(root.join(".git/packed-refs"))
        ))
        .unwrap();

        let mut pending = HashSet::new();
        let mut last_event = None;
        let mut first_event = None;
        let mut git_hit = false;
        drain_watch_events(
            &rx,
            &root,
            &ignore_set,
            &mut pending,
            &mut last_event,
            &mut first_event,
            &mut git_hit,
            &mut false,
        );

        assert!(git_hit, "a packed-refs transition must set git_hit");
        assert!(
            pending.is_empty(),
            "a GitRef signal is not itself a watched mutation"
        );
    }

    // A `.gitignore` is filter *configuration*, not watched content. The
    // rebuild trigger used to be derived from `pending`, which holds watched
    // content only — so a `.gitignore` whose own rules match it (`*`) classified
    // `Ignore`, never entered `pending`, and could never announce its own edit.
    // Editing it (adding `!keep.log`) went unhonored until a daemon restart.
    // `e453e86` fixed that trigger — this test used to assert only that the
    // flag got SET, which left the *consumption* half (nothing ever read the
    // flag unless something else was also pending) entirely uncaught; that's
    // the bug this plan's Phase 1 fixes. Now also drives `maybe_rebuild`, the
    // loop tick's consumer, and checks the rebuilt set actually reflects the
    // edit — not just that a rebuild happened.
    #[test]
    fn self_matching_gitignore_edit_flags_and_rebuild_consumes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let initialized = std::process::Command::new("git")
            .arg("init")
            .arg(&root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(initialized, "git must be available to run this test");

        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::write(root.join("cache/.gitignore"), "*\n").unwrap();
        let ignore_set = IgnoreSet::build(&root);

        // Precondition: this is exactly the file that classifies `Ignore`, so a
        // `pending`-derived trigger cannot see it — and `keep.log` is ignored
        // under this STALE set too, the fact the rebuild below must flip.
        let gi = root.join("cache/.gitignore");
        assert!(
            matches!(classify(&root, &gi, &ignore_set), Class::Ignore),
            "a self-matching .gitignore must classify Ignore (else this test proves nothing)"
        );
        assert!(
            ignore_set.is_ignored(&root.join("cache/keep.log"), false),
            "keep.log must be ignored under the pre-edit set (else the rebuild proves nothing)"
        );

        // The edit under test: negate one file.
        std::fs::write(&gi, "*\n!keep.log\n").unwrap();

        let (tx, rx) = channel::<Result<notify::Event, notify::Error>>();
        tx.send(Ok(notify::Event::default().add_path(gi))).unwrap();

        let mut pending = HashSet::new();
        let mut last_event = None;
        let mut first_event = None;
        let mut git_hit = false;
        let mut gitignore_dirty = false;
        drain_watch_events(
            &rx,
            &root,
            &ignore_set,
            &mut pending,
            &mut last_event,
            &mut first_event,
            &mut git_hit,
            &mut gitignore_dirty,
        );

        assert!(
            gitignore_dirty,
            "editing a self-matching .gitignore must flag an ignore-set rebuild"
        );
        assert!(
            pending.is_empty(),
            "the .gitignore itself is still not recorded as watched content"
        );

        // Consumption: `maybe_rebuild` is the loop tick's rate-limited reader
        // of the flag. This is the part the old test never reached.
        let rebuilt = maybe_rebuild(&mut gitignore_dirty, &root);
        assert!(
            rebuilt.is_some(),
            "a set dirty flag must produce a rebuilt IgnoreSet"
        );
        assert!(
            !gitignore_dirty,
            "maybe_rebuild must clear the flag it just consumed"
        );
        assert!(
            !rebuilt
                .unwrap()
                .is_ignored(&root.join("cache/keep.log"), false),
            "the rebuilt IgnoreSet must reflect the edited rules (keep.log re-included)"
        );
    }

    // `maybe_rebuild` is the unit-reachable target for the loop tick's rate
    // rule (at most one full-repo walk per dirty flag) — `run`'s loop body
    // itself cannot be driven from a unit test.
    #[test]
    fn maybe_rebuild_runs_once_per_dirty_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let initialized = std::process::Command::new("git")
            .arg("init")
            .arg(&root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(initialized, "git must be available to run this test");

        let mut dirty = false;
        assert!(
            maybe_rebuild(&mut dirty, &root).is_none(),
            "a clear flag must never rebuild"
        );

        // First dirty setting: rebuilds exactly once, then goes quiet.
        dirty = true;
        assert!(maybe_rebuild(&mut dirty, &root).is_some());
        assert!(
            !dirty,
            "the flag must be cleared by the call that consumed it"
        );
        assert!(
            maybe_rebuild(&mut dirty, &root).is_none(),
            "an immediate second call on a now-clean flag must not rebuild again"
        );

        // A second, independent dirty setting: still exactly one `Some`.
        dirty = true;
        assert!(maybe_rebuild(&mut dirty, &root).is_some());
        assert!(!dirty);
        assert!(maybe_rebuild(&mut dirty, &root).is_none());
    }

    // ---- D9: event-channel draining + walk exclusion -------------------------

    #[test]
    fn drain_watch_events_applies_every_queued_message_in_one_call() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let ignore_set = IgnoreSet::build(&root);
        let (tx, rx) = channel::<Result<notify::Event, notify::Error>>();

        let p1 = root.join("a.rs");
        let p2 = root.join("b.rs");
        let p3 = root.join("c.rs");
        tx.send(Ok(notify::Event::default().add_path(p1.clone())))
            .unwrap();
        tx.send(Ok(notify::Event::default().add_path(p2.clone())))
            .unwrap();
        tx.send(Ok(notify::Event::default().add_path(p3.clone())))
            .unwrap();

        let mut pending = HashSet::new();
        let mut last_event = None;
        let mut first_event = None;
        let mut git_hit = false;
        let disconnected = drain_watch_events(
            &rx,
            &root,
            &ignore_set,
            &mut pending,
            &mut last_event,
            &mut first_event,
            &mut git_hit,
            &mut false,
        );

        assert!(!disconnected);
        assert_eq!(
            pending.len(),
            3,
            "one call must drain every already-queued event, not just the first"
        );
        assert!(pending.contains(&p1));
        assert!(pending.contains(&p2));
        assert!(pending.contains(&p3));
    }

    #[test]
    fn drain_watch_events_reports_disconnected_when_sender_drops() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let ignore_set = IgnoreSet::build(&root);
        let (tx, rx) = channel::<Result<notify::Event, notify::Error>>();
        drop(tx);

        let mut pending = HashSet::new();
        let mut last_event = None;
        let mut first_event = None;
        let mut git_hit = false;
        let disconnected = drain_watch_events(
            &rx,
            &root,
            &ignore_set,
            &mut pending,
            &mut last_event,
            &mut first_event,
            &mut git_hit,
            &mut false,
        );
        assert!(disconnected);
    }

    #[test]
    fn recorder_scan_excludes_git_and_agentrec_subtrees() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
        std::fs::write(root.join(".git/objects/ab/deadbeef"), b"x").unwrap();
        std::fs::create_dir_all(root.join(".agentrec/objects/cd")).unwrap();
        std::fs::write(root.join(".agentrec/objects/cd/feedface"), b"x").unwrap();
        std::fs::write(root.join("real.rs"), b"fn main(){}").unwrap();

        let store = BlobStore::new(root.join(".agentrec/objects"));
        let recorder = Recorder::scan(root, store);
        assert!(recorder.known.contains(Path::new("real.rs")));
        assert!(
            !recorder.known.iter().any(|p| p.starts_with(".git")),
            "known set: {:?}",
            recorder.known
        );
        assert!(
            !recorder.known.iter().any(|p| p.starts_with(".agentrec")),
            "known set: {:?}",
            recorder.known
        );
    }

    #[test]
    fn ignoreset_build_does_not_descend_into_git_or_agentrec() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        // A `.gitignore` buried inside `.git`/`.agentrec` must never be
        // collected as a real ignore rule for the repo.
        std::fs::write(root.join(".git/.gitignore"), "*.rs\n").unwrap();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        std::fs::write(root.join(".agentrec/.gitignore"), "*.rs\n").unwrap();

        let set = IgnoreSet::build(root);
        assert!(
            !set.is_ignored(&root.join("real.rs"), false),
            "a buried .gitignore under .git/.agentrec must not leak into real rules"
        );
    }

    // ---- D8: idempotent orphan recovery ---------------------------------------

    // A kill-9 landing between `append_log` and the journal's `remove_file`
    // leaves the journal describing a turn that's already logged. Recovering
    // it again on the next startup must not double-log it.
    #[test]
    fn recover_orphan_skips_duplicate_when_already_logged() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();

        let files = vec![FileEntry {
            path: "a.rs".into(),
            before: None,
            after: Some("sha256:aaa".into()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }];

        let existing = TurnRecord {
            v: 1,
            id: turn_id(),
            grade: "bare".to_string(),
            truncated: false,
            started: rfc3339(1_000),
            ended: rfc3339(2_000),
            tool: None,
            model: None,
            session: None,
            root: root.to_string_lossy().to_string(),
            prompt_ref: None,
            prompt_excerpt: None,
            merges: vec![],
            files: files.clone(),
        };
        append_log(&log_path(root), &LogRecord::Turn(existing)).unwrap();

        // A journal describing the SAME turn, as if a prior recovery already
        // appended it and crashed before removing the journal file.
        let journal = OrphanJournal {
            source: "quiet".to_string(),
            tool: None,
            prompt: None,
            session: None,
            opened_wall_ms: 1_000,
            last_change_wall_ms: 2_000,
            root: root.to_string_lossy().to_string(),
            files,
            id: turn_id(),
        };
        std::fs::write(open_path(root), serde_json::to_string(&journal).unwrap()).unwrap();

        recover_orphan(root).unwrap();

        let turns: Vec<_> = agentrec_core::record::load_log(&log_path(root))
            .into_iter()
            .filter(|r| matches!(r, LogRecord::Turn(_)))
            .collect();
        assert_eq!(
            turns.len(),
            1,
            "must not double-log the same recovered turn"
        );
        assert!(
            !open_path(root).exists(),
            "the stale journal is still cleaned up on the duplicate path"
        );
    }

    // Regression: a turn closed by an incoming *start* signal is logged with
    // `ended` = the start time (engine `observe_start` closes at `now`, not
    // `last_change_at`), but its crash journal was written earlier carrying
    // `last_change_wall_ms` < that. A kill-9 in the persist->sync_journal
    // window leaves that stale journal; on restart `recover_orphan`
    // recomputes `ended` from `last_change` and it no longer matches the
    // logged turn. Because turn ids are now RESERVED AT OPEN, the journal and
    // the already-logged turn share the SAME id — a content-only idempotency
    // check misses the drift and re-appends that id, producing an "ambiguous
    // turn id" that breaks `undo`. Dedup must key on the (unique, stable)
    // turn id, not just root+times+files.
    #[test]
    fn recover_orphan_skips_duplicate_by_id_even_when_ended_drifted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();

        let files = vec![FileEntry {
            path: "a.rs".into(),
            before: None,
            after: Some("sha256:aaa".into()),
            op: "create".into(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
        }];

        let id = turn_id();
        // Logged turn: closed at now=3_000 (a later start signal), NOT at
        // last_change=2_000.
        let existing = TurnRecord {
            v: 1,
            id: id.clone(),
            grade: "bare".to_string(),
            truncated: false,
            started: rfc3339(1_000),
            ended: rfc3339(3_000),
            tool: None,
            model: None,
            session: None,
            root: root.to_string_lossy().to_string(),
            prompt_ref: None,
            prompt_excerpt: None,
            merges: vec![],
            files: files.clone(),
        };
        append_log(&log_path(root), &LogRecord::Turn(existing)).unwrap();

        // Journal written before the close, same reserved id, last_change
        // 2_000 -> recover_orphan recomputes ended = rfc3339(2_000) != 3_000.
        let journal = OrphanJournal {
            source: "quiet".to_string(),
            tool: None,
            prompt: None,
            session: None,
            opened_wall_ms: 1_000,
            last_change_wall_ms: 2_000,
            root: root.to_string_lossy().to_string(),
            files,
            id: id.clone(),
        };
        std::fs::write(open_path(root), serde_json::to_string(&journal).unwrap()).unwrap();

        recover_orphan(root).unwrap();

        let turns: Vec<_> = agentrec_core::record::load_log(&log_path(root))
            .into_iter()
            .filter_map(|r| match r {
                LogRecord::Turn(t) => Some(t),
                _ => None,
            })
            .collect();
        let with_id = turns.iter().filter(|t| t.id == id).count();
        assert_eq!(
            with_id, 1,
            "the reserved id must appear exactly once — a drifted `ended` must \
             not defeat idempotency and re-log the same id (ambiguous `undo`)"
        );
        assert_eq!(turns.len(), 1, "no duplicate turn appended");
        assert!(!open_path(root).exists(), "stale journal cleaned up");
    }

    // A genuinely new orphaned turn (different times/files than anything
    // logged) must still be recovered normally — the idempotency check must
    // not swallow real recoveries.
    #[test]
    fn recover_orphan_still_recovers_a_genuinely_new_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(crate::agentrec_dir(root)).unwrap();

        let journal = OrphanJournal {
            source: "quiet".to_string(),
            tool: None,
            prompt: None,
            session: None,
            opened_wall_ms: 5_000,
            last_change_wall_ms: 6_000,
            root: root.to_string_lossy().to_string(),
            files: vec![FileEntry {
                path: "b.rs".into(),
                before: None,
                after: Some("sha256:bbb".into()),
                op: "create".into(),
                skipped: false,
                withheld: false,
                baseline_unknown: false,
                skipped_reason: None,
            }],
            id: turn_id(),
        };
        std::fs::write(open_path(root), serde_json::to_string(&journal).unwrap()).unwrap();

        recover_orphan(root).unwrap();

        let turns: Vec<_> = agentrec_core::record::load_log(&log_path(root))
            .into_iter()
            .filter(|r| matches!(r, LogRecord::Turn(_)))
            .collect();
        assert_eq!(
            turns.len(),
            1,
            "a genuinely new orphan must still be recovered"
        );
        assert!(!open_path(root).exists());
    }
}
