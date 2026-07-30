//! `agentrec purge` (AC I5–I6): default deletes prompt-blob objects for turns
//! older than `ttl_days` (config.toml, default 90). `--all-prompts` deletes
//! every prompt blob regardless of age; `--snapshots-before <DATE>` deletes
//! snapshot blobs (`before`/`after`) for turns started before that date. Both
//! keep a dedup keep-set so a blob still referenced by a kept turn — content
//! is shared across turns — is never deleted. `log`/`blame` are unaffected
//! (they render `prompt_excerpt`, never the blob); `diff`/`undo` are
//! unaffected by the default (prompt-only) purge, since snapshot blobs are a
//! disjoint set only touched by `--snapshots-before`. `--memories-retracted`
//! archives+rewrites `memory.jsonl`; `--log-duplicates` archives+rewrites
//! `log.jsonl` to repair a pre-fix daemon's same-id duplicate `TurnRecord`s
//! (the read-side migration/repair deferred after PR #2's curative
//! `readcmds::same_revert` dedup — see that module for the underlying bug).

use crate::cmds::wall_now_ms;
use crate::{agentrec_dir, log_path, objects_dir};
use agentrec_core::memory::{memory_path, MemoryRecord};
use agentrec_core::record::{LogRecord, TurnRecord};
use agentrec_core::store::BlobStore;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_TTL_DAYS: u64 = 90;
const DAY_MS: u64 = 86_400_000;

pub fn run(
    root: &Path,
    all_prompts: bool,
    snapshots_before: Option<&str>,
    memories_retracted: bool,
    log_duplicates: bool,
    orphans: bool,
) -> Result<(), String> {
    let records = agentrec_core::record::load_log(&log_path(root));
    let turns: Vec<&TurnRecord> = owned_turns(&records);
    let store = BlobStore::new(objects_dir(root));

    purge_prompts(&store, &turns, root, all_prompts);
    if let Some(date) = snapshots_before {
        purge_snapshots_before(&store, &turns, date, root)?;
    }
    if memories_retracted {
        purge_memories_retracted(root)?;
    }
    if log_duplicates {
        purge_log_duplicates(root)?;
    }
    if orphans {
        purge_orphans(root)?;
    }
    Ok(())
}

fn owned_turns(records: &[LogRecord]) -> Vec<&TurnRecord> {
    records
        .iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        })
        .collect()
}

/// Every snapshot-blob hash (`before`/`after`) referenced by any of `turns`.
fn snapshot_hashes<'a>(turns: &[&'a TurnRecord]) -> HashSet<&'a str> {
    turns
        .iter()
        .flat_map(|t| t.files.iter())
        .flat_map(|f| [f.before.as_deref(), f.after.as_deref()])
        .flatten()
        .collect()
}

/// Every prompt-blob hash (`prompt_ref`) referenced by any of `turns`.
fn prompt_hashes<'a>(turns: &[&'a TurnRecord]) -> HashSet<&'a str> {
    turns
        .iter()
        .filter_map(|t| t.prompt_ref.as_deref())
        .collect()
}

/// Delete prompt-blob objects: all of them (`all_prompts`) or just those
/// belonging to turns older than `ttl_days`, keeping any blob still shared
/// with a turn inside the TTL window.
fn purge_prompts(store: &BlobStore, turns: &[&TurnRecord], root: &Path, all_prompts: bool) {
    let ttl_days = read_ttl_days(root);
    let cutoff = ttl_cutoff(ttl_days);

    let mut keep: HashSet<&str> = if all_prompts {
        HashSet::new()
    } else {
        turns
            .iter()
            .filter(|t| t.started.as_str() >= cutoff.as_str())
            .filter_map(|t| t.prompt_ref.as_deref())
            .collect()
    };
    // A2: a blob content-shared with any turn's snapshot (before/after) must
    // never be deleted here, regardless of prompt TTL or `--all-prompts` —
    // snapshot blobs are only purged via `--snapshots-before`, its own
    // disjoint retention policy.
    keep.extend(snapshot_hashes(turns));

    let mut candidates: HashSet<&str> = HashSet::new();
    for t in turns {
        let Some(pref) = t.prompt_ref.as_deref() else {
            continue;
        };
        let expired = all_prompts || t.started.as_str() < cutoff.as_str();
        if expired && !keep.contains(pref) {
            candidates.insert(pref);
        }
    }

    // A3(b): re-load the log immediately before the destructive step and
    // re-union the keep-set, narrowing the TOCTOU window between this
    // function's initial log read (in `run`) and the delete below — a live
    // daemon may have appended a turn referencing a candidate in between.
    let fresh_records = agentrec_core::record::load_log(&log_path(root));
    let fresh_turns = owned_turns(&fresh_records);
    let mut fresh_keep: HashSet<&str> = if all_prompts {
        HashSet::new()
    } else {
        fresh_turns
            .iter()
            .filter(|t| t.started.as_str() >= cutoff.as_str())
            .filter_map(|t| t.prompt_ref.as_deref())
            .collect()
    };
    fresh_keep.extend(snapshot_hashes(&fresh_turns));
    candidates.retain(|h| !fresh_keep.contains(h));

    let (count, bytes) = delete_all(store, &candidates);
    if all_prompts {
        println!(
            "purged {count} prompt blob(s) (all), {} freed",
            human_bytes(bytes)
        );
    } else {
        println!(
            "purged {count} expired prompt blob(s) (ttl {ttl_days}d), {} freed",
            human_bytes(bytes)
        );
    }
}

/// Delete snapshot-blob objects (`before`/`after`) for turns started before
/// `date` (`YYYY-MM-DD`), keeping any blob still shared with a turn on/after
/// that date.
fn purge_snapshots_before(
    store: &BlobStore,
    turns: &[&TurnRecord],
    date: &str,
    root: &Path,
) -> Result<(), String> {
    let cutoff = parse_date_cutoff(date)?;

    let mut keep: HashSet<&str> = turns
        .iter()
        .filter(|t| t.started.as_str() >= cutoff.as_str())
        .flat_map(|t| t.files.iter())
        .flat_map(|f| [f.before.as_deref(), f.after.as_deref()])
        .flatten()
        .collect();
    // A2: a blob content-shared with any turn's prompt must never be
    // deleted here — prompt blobs are only purged via the default TTL path.
    keep.extend(prompt_hashes(turns));

    let mut candidates: HashSet<&str> = HashSet::new();
    for t in turns {
        if t.started.as_str() >= cutoff.as_str() {
            continue;
        }
        for f in &t.files {
            for h in [f.before.as_deref(), f.after.as_deref()]
                .into_iter()
                .flatten()
            {
                if !keep.contains(h) {
                    candidates.insert(h);
                }
            }
        }
    }

    // A3(b): same reload-before-delete narrowing as `purge_prompts`.
    let fresh_records = agentrec_core::record::load_log(&log_path(root));
    let fresh_turns = owned_turns(&fresh_records);
    let mut fresh_keep: HashSet<&str> = fresh_turns
        .iter()
        .filter(|t| t.started.as_str() >= cutoff.as_str())
        .flat_map(|t| t.files.iter())
        .flat_map(|f| [f.before.as_deref(), f.after.as_deref()])
        .flatten()
        .collect();
    fresh_keep.extend(prompt_hashes(&fresh_turns));
    candidates.retain(|h| !fresh_keep.contains(h));

    let (count, bytes) = delete_all(store, &candidates);
    println!(
        "purged {count} snapshot blob(s) before {date}, {} freed",
        human_bytes(bytes)
    );
    Ok(())
}

fn delete_all(store: &BlobStore, hashes: &HashSet<&str>) -> (usize, u64) {
    let mut count = 0;
    let mut bytes = 0u64;
    for h in hashes {
        if let Some(size) = store.remove(h) {
            count += 1;
            bytes += size;
        }
    }
    (count, bytes)
}

/// `purge --memories-retracted` (the ONLY sanctioned rewrite of
/// `memory.jsonl` — handle with care): chains whose folded effective state
/// is `retracted` AND whose retract `ts` is older than `ttl_days` move to
/// `.agentrec/memory.archived.<unix_ts>.jsonl`. Live memories and
/// stale-but-unretracted memories (any chain not currently `retracted`) are
/// never touched, regardless of pin freshness — freshness is orthogonal to
/// this retention policy.
///
/// Crash-safety ordering (load-bearing, do not reorder): (a) determine the
/// expired-retracted ids from the folded state; (b) gather every raw line
/// belonging to those ids (assert + reverify + retract, verbatim bytes) and
/// append+fsync them to the archive file; only once that fsync has returned
/// (c) rewrite `memory.jsonl` without those lines, atomically (tmp + fsync +
/// rename + dir-fsync, mirroring `store.rs`'s blob-write pattern). A kill-9
/// between (b) and (c) leaves `memory.jsonl` a strict superset of what it
/// should be — the archived records are also still in the (unreplaced)
/// source — never a subset, so no record is ever lost.
fn purge_memories_retracted(root: &Path) -> Result<(), String> {
    // The `record` daemon (a long-running launchd/systemd service) appends
    // memory candidates to `memory.jsonl` continuously (Task 7). Belt and
    // suspenders: refuse outright while the daemon holds its own flock,
    // before ever touching `memory.lock` below. Reuse the existing
    // non-blocking flock probe — never reimplement liveness detection.
    if crate::daemon::daemon_is_running(root) {
        return Err(
            "stop recording (agentrec is running) before purging memories — \
             the daemon appends memory.jsonl concurrently"
                .to_string(),
        );
    }

    // F4: hold the dedicated `memory.lock` (non-blocking) across the ENTIRE
    // read -> archive -> rewrite -> rename sequence below. This is what
    // actually closes the probe/act race the daemon-liveness check alone
    // can't: a manual `remember`/`verify`/`forget` (or a daemon starting up
    // right after the check above) landing between our read and our rename
    // would otherwise be silently clobbered by the atomic rewrite. Every
    // writer (`memlock::append_memory_locked`) takes this same lock
    // BLOCKING, so from here until the lock drops at the end of this
    // function, any concurrent writer simply waits its turn and its append
    // lands intact right after — never lost. If we can't acquire it (another
    // purge or a writer already holds it), refuse loudly rather than
    // proceeding unlocked.
    let _lock = crate::memlock::try_acquire(root)?;

    let ttl_days = read_ttl_days(root);
    let cutoff_ms = wall_now_ms().saturating_sub(ttl_days.saturating_mul(DAY_MS));

    let mem_path = memory_path(root);
    let Ok(text) = std::fs::read_to_string(&mem_path) else {
        println!("purged 0 retracted memory chain(s) (ttl {ttl_days}d) — no memory.jsonl yet");
        return Ok(());
    };

    // Raw (line, id) pairs — the id is parsed only to decide which lines
    // move, never to reserialize: survivors and archived lines both keep
    // their exact original bytes.
    let raw_lines: Vec<(&str, Option<String>)> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let id = serde_json::from_str::<MemoryRecord>(l).ok().map(|r| r.id);
            (l, id)
        })
        .collect();

    let effective = agentrec_core::memory::load_effective(root)?;
    let expired_ids: HashSet<String> = effective
        .iter()
        .filter(|m| m.retracted && m.ts < cutoff_ms)
        .map(|m| m.id.clone())
        .collect();

    if expired_ids.is_empty() {
        println!("purged 0 retracted memory chain(s) (ttl {ttl_days}d)");
        return Ok(());
    }

    let is_expired = |id: &Option<String>| id.as_deref().is_some_and(|i| expired_ids.contains(i));
    let archived_lines: Vec<&str> = raw_lines
        .iter()
        .filter(|(_, id)| is_expired(id))
        .map(|(l, _)| *l)
        .collect();
    let survivor_lines: Vec<&str> = raw_lines
        .iter()
        .filter(|(_, id)| !is_expired(id))
        .map(|(l, _)| *l)
        .collect();

    // (b) archive first, fsynced, BEFORE the source is touched at all.
    let archive_path = memory_archive_path(root);
    append_lines_synced(&archive_path, &archived_lines)?;
    agentrec_core::perms::lock_file(&archive_path);

    test_pause_before_memory_rewrite();

    // (c) atomic rewrite of memory.jsonl — the only sanctioned rewrite site.
    rewrite_memory_atomic(&mem_path, &survivor_lines)?;
    agentrec_core::perms::lock_file(&mem_path);

    println!(
        "purged {} retracted memory chain(s) (ttl {ttl_days}d) — archived to {}",
        expired_ids.len(),
        archive_path.display()
    );
    Ok(())
}

/// Test-only race-window widener (F4, mirrors `daemon.rs`'s
/// `test_pause_after_candidate_persist`): when
/// `AGENTREC_TEST_PAUSE_BEFORE_MEMORY_REWRITE_MS` is set (only ever done by
/// `cli/tests/hardening_cli.rs`'s F4 tests), sleeps for the given duration
/// right after the archive fsync and right before the atomic rewrite of
/// `memory.jsonl` — i.e. inside the exact window a concurrent writer's
/// append must survive. With `memory.lock` held across this whole function
/// (see the call site above), a concurrent writer blocks for the entire
/// pause and lands only after this function's lock drops; on unfixed
/// (unlocked) code the same pause instead gives a concurrent writer a wide,
/// reliable opening to land its append here — where it is invisible to the
/// already-computed `survivor_lines` and gets silently clobbered by the
/// rewrite that follows. The env read is compiled out entirely in release
/// builds (`#[cfg(not(debug_assertions))]` arm always returns `None` without
/// touching the environment) — same fail-safe class as
/// `agentrec_core::memory::test_slow_pin_read_delay`: a release/production
/// binary can never have an arbitrary sleep injected into its purge path.
/// It stays active under `cfg(debug_assertions)`, which `cargo test` sets
/// and which `cli/tests/hardening_cli.rs`'s spawned `CARGO_BIN_EXE_agentrec`
/// (always a debug build) inherits, so the seam still bites in the test
/// suite.
fn test_pause_before_memory_rewrite_delay() -> Option<std::time::Duration> {
    #[cfg(debug_assertions)]
    {
        std::env::var("AGENTREC_TEST_PAUSE_BEFORE_MEMORY_REWRITE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(std::time::Duration::from_millis)
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

fn test_pause_before_memory_rewrite() {
    if let Some(delay) = test_pause_before_memory_rewrite_delay() {
        std::thread::sleep(delay);
    }
}

/// `.agentrec/memory.archived.<unix_ts>.jsonl` — mirrors `uninstallcmd`'s
/// `.agentrec.archived.<unix_ts>` naming (whole-seconds unix timestamp).
fn memory_archive_path(root: &Path) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    agentrec_dir(root).join(format!("memory.archived.{ts}.jsonl"))
}

/// Append `lines` (each written verbatim + a trailing `\n`) to `path`,
/// creating it if absent, then fsync the file handle before returning —
/// callers must be able to trust the archive is durable the moment this
/// returns `Ok`, since the source rewrite is only safe to start afterward.
fn append_lines_synced(path: &Path, lines: &[&str]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    for line in lines {
        writeln!(file, "{line}").map_err(|e| e.to_string())?;
    }
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}

/// Rewrite `mem_path` to contain exactly `lines` (verbatim bytes, one per
/// line), atomically: write a per-process-unique tmp file in the same
/// directory, fsync it, rename over `mem_path`, then fsync the parent dir —
/// the same tmp+fsync+rename+dir-fsync shape as `store.rs`'s blob writes, so
/// a crash mid-rewrite either leaves the old file intact or the new one
/// fully written, never a truncated/partial file.
///
/// F6: evaluated reusing `BlobStore::put`/`put_result` directly instead of
/// this hand-rolled shape — skipped, not just duplicated for its own sake.
/// `put_result` writes to a path it *derives from the content hash* (the
/// object store's fan-out layout); this function must overwrite one
/// specific, already-named path (`memory.jsonl`), which is a different unit
/// of operation `put_result`'s signature can't express. Its `create_tmp_file`
/// helper is also private to `agentrec-core::store` and not part of that
/// crate's public surface — exporting it across the crate boundary just for
/// this call site would be a real API change to durability-sensitive code,
/// out of scope for a behavior-neutral polish pass.
fn rewrite_memory_atomic(mem_path: &Path, lines: &[&str]) -> Result<(), String> {
    let parent = mem_path
        .parent()
        .ok_or_else(|| "memory.jsonl has no parent directory".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let tmp = parent.join(format!(
        ".memory.jsonl.tmp.{}.{}",
        std::process::id(),
        wall_now_ms()
    ));

    let write = (|| -> std::io::Result<()> {
        let mut file = create_tmp_file(&tmp)?;
        for line in lines {
            writeln!(file, "{line}")?;
        }
        file.sync_all()?;
        std::fs::rename(&tmp, mem_path)?;
        Ok(())
    })();

    match write {
        Ok(()) => {
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(format!("failed to rewrite memory.jsonl: {e}"))
        }
    }
}

/// Create the rewrite tmp file at mode 0600 (unix) in the same syscall that
/// creates it — mirrors `store.rs::create_tmp_file`'s set-at-create posture
/// (no window where the tmp file is briefly reachable at the umask default).
#[cfg(unix)]
fn create_tmp_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_tmp_file(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

// ---- purge --log-duplicates (read-side migration/repair, Option 2) --------
//
// A pre-fix daemon (PR #2's kill-9 window between `persist` and the journal
// clear) could write two `TurnRecord`s sharing one turn id into `log.jsonl`.
// The engine fix stops NEW duplicates; `readcmds::resolve_turn`/`same_revert`
// CURES the read path (diff/show/undo collapse the pair on the fly) — but
// the file itself still carries the dup forever, and any future consumer
// that reads `log.jsonl` directly (not through `resolve_turn`) still sees
// two records. This is the store-level repair: archive the file, then
// rewrite it dropping only lines that are exact `same_revert` duplicates of
// an earlier same-id record.

/// `purge --log-duplicates`: mirrors `purge_memories_retracted`'s shape (the
/// only other sanctioned rewrite in this codebase) — daemon-liveness
/// refusal, archive-before-touch, atomic tmp+fsync+rename+dir-fsync. Two
/// differences, both load-bearing:
///
/// (a) TWO overlapping guards cover TWO different concurrent writers of
///     `log.jsonl`:
///       * `undo --confirm` (the only non-daemon writer) takes `log.lock`
///         (`loglock.rs`) BLOCKING around its append; this function takes the
///         same lock NONBLOCKING and holds it across the whole
///         archive+recheck+rewrite+rename sequence. An undo racing the repair
///         therefore waits until the rename completes and then appends to the
///         rewritten file — never lost. This closes the microsecond
///         recheck->rename window that a length check alone cannot (finding
///         #3).
///       * a daemon that STARTS after the liveness refusal above (and so does
///         NOT hold `log.lock` — the daemon's per-turn append is a hot path
///         kept off this lock) is still caught by the length recheck: this
///         function re-reads the file's byte length immediately before the
///         destructive rename, and `log.jsonl` being append-only, any such
///         writer can only grow it. Growth since the initial read aborts the
///         rewrite entirely (tmp discarded, original untouched) rather than
///         risk clobbering it. Between the two, the only residual is a daemon
///         that both starts mid-repair AND lands its write in the
///         sub-recheck-to-rename window — vanishingly narrow and requires
///         defeating the liveness guard, documented rather than fully closed.
/// (b) the archive holds the WHOLE original file, not just the removed
///     lines — simpler to verify ("never delete user data" trivially holds:
///     the archive alone reconstructs the pre-repair state) and cheap, since
///     duplicates are a rare one-shot artifact here, unlike memory's routine
///     TTL churn.
fn purge_log_duplicates(root: &Path) -> Result<(), String> {
    // Same belt-and-suspenders posture as purge_memories_retracted: refuse
    // outright while the daemon holds its own flock, before reading anything.
    if crate::daemon::daemon_is_running(root) {
        return Err(
            "stop recording (agentrec is running) before repairing log.jsonl — \
             the daemon appends turns concurrently"
                .to_string(),
        );
    }

    // Finding #3: hold `log.lock` (NONBLOCKING) for the whole read -> archive
    // -> recheck -> rewrite -> rename sequence. `undo --confirm` takes the
    // same lock BLOCKING around its append, so an undo racing this repair
    // waits until the rename completes and then lands on the rewritten file —
    // closing the microsecond recheck->rename window that the length recheck
    // below cannot. Refuse loudly if an undo is mid-append rather than proceed
    // unlocked. `_log_lock` must live to the end of this function.
    let _log_lock = crate::loglock::try_acquire(root)?;

    let path = log_path(root);
    let Ok(original) = std::fs::read_to_string(&path) else {
        println!("scanned 0 turn record(s), 0 duplicate(s) removed — no log.jsonl yet");
        return Ok(());
    };

    // Tolerant classify-and-fold over RAW lines (never `load_log`, which
    // silently drops torn lines and any record type it doesn't recognize —
    // this repair must preserve exactly what's on disk except the specific
    // duplicates it's authorized to remove).
    let mut survivors: Vec<&str> = Vec::new();
    let mut kept_turns: Vec<TurnRecord> = Vec::new();
    let mut scanned = 0usize;
    let mut removed = 0usize;

    for line in original.lines() {
        if line.trim().is_empty() {
            survivors.push(line);
            continue;
        }
        match classify_turn_line(line) {
            Some(t) => {
                scanned += 1;
                let is_duplicate = kept_turns
                    .iter()
                    .any(|kept| agentrec_core::view::same_revert(kept, &t));
                if is_duplicate {
                    removed += 1;
                } else {
                    kept_turns.push(t);
                    survivors.push(line);
                }
            }
            None => survivors.push(line), // epoch, unknown type, or garbage — always kept
        }
    }

    if removed == 0 {
        println!("scanned {scanned} turn record(s), 0 duplicate(s) removed — log.jsonl unchanged");
        return Ok(());
    }

    // Archive the WHOLE original file, fsynced, BEFORE the source is touched.
    let archive_path = log_archive_path(root);
    write_full_file_synced(&archive_path, &original)?;
    agentrec_core::perms::lock_file(&archive_path);

    test_pause_before_log_rewrite();

    // (a) above: re-check length right before the destructive rewrite.
    let current_len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if current_len as usize > original.len() {
        return Err(
            "log.jsonl changed during repair (a concurrent writer landed) — \
             no changes made; rerun `agentrec purge --log-duplicates`"
                .to_string(),
        );
    }

    rewrite_log_atomic(&path, &survivors)?;
    agentrec_core::perms::lock_file(&path);

    println!(
        "scanned {scanned} turn record(s), removed {removed} duplicate(s) — archived original to {}",
        archive_path.display()
    );
    Ok(())
}

/// Test-only race-window widener, mirroring
/// `test_pause_before_memory_rewrite`: when
/// `AGENTREC_TEST_PAUSE_BEFORE_LOG_REWRITE_MS` is set (only ever done by
/// `cli/tests/hardening_cli.rs`), sleeps right after the archive fsync and
/// right before the length recheck / atomic rewrite — the exact window a
/// concurrent writer's append must be detected in. The env read is compiled
/// out entirely in release builds (`#[cfg(not(debug_assertions))]` arm always
/// returns `None` without touching the environment) — same fail-safe class
/// as `test_pause_before_memory_rewrite_delay` above.
fn test_pause_before_log_rewrite_delay() -> Option<std::time::Duration> {
    #[cfg(debug_assertions)]
    {
        std::env::var("AGENTREC_TEST_PAUSE_BEFORE_LOG_REWRITE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(std::time::Duration::from_millis)
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

fn test_pause_before_log_rewrite() {
    if let Some(delay) = test_pause_before_log_rewrite_delay() {
        std::thread::sleep(delay);
    }
}

/// Classify one raw `log.jsonl` line as a turn (`Some`) or not (`None`),
/// mirroring `agentrec_core::record::load_log`'s exact turn-recognition rule
/// — this MUST stay in sync with that function, or this repair can
/// misclassify data:
///
/// a line is a turn iff it parses as a tagged `LogRecord::Turn`, OR it has
/// NO `type` field at all and parses as a bare legacy `TurnRecord`. A line
/// like `{"type":"future_thing", ...turn-shaped fields...}` must NOT be
/// treated as a turn just because `TurnRecord`'s deserializer would ignore
/// the unrecognized `type` field if asked naively — that would let an
/// unknown-type record be candidate-matched and silently removed as a "dup"
/// by the fold above. Anything not positively recognized as a turn (epoch,
/// unknown type, torn/garbage JSON) returns `None` and is always preserved
/// byte-exact by the caller.
fn classify_turn_line(line: &str) -> Option<TurnRecord> {
    if let Ok(rec) = serde_json::from_str::<LogRecord>(line) {
        return match rec {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        };
    }
    let has_type_field = serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| v.as_object().map(|o| o.contains_key("type")))
        .unwrap_or(false);
    if has_type_field {
        return None;
    }
    serde_json::from_str::<TurnRecord>(line).ok()
}

/// `.agentrec/log.archived.<unix_ts>.jsonl` — mirrors `memory_archive_path`'s
/// naming (whole-seconds unix timestamp).
fn log_archive_path(root: &Path) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    agentrec_dir(root).join(format!("log.archived.{ts}.jsonl"))
}

/// Write `content` verbatim to `path` (creating it, truncating any stale
/// content at that exact path), then fsync the file handle before
/// returning — callers must be able to trust the archive is durable before
/// the source rewrite starts. `path` is always a freshly-minted, per-run
/// timestamped name, so truncate-on-open never discards a previous archive.
fn write_full_file_synced(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(content.as_bytes())
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}

/// Rewrite `log.jsonl` to contain exactly `lines` (verbatim bytes, one per
/// line), atomically: per-process-unique tmp file in the same directory,
/// fsync it, rename over `log_path`, then fsync the parent dir — the same
/// tmp+fsync+rename+dir-fsync shape as `rewrite_memory_atomic` above.
///
/// Deliberately NOT unified with `rewrite_memory_atomic` into one shared
/// helper: `log.jsonl` is the durability-critical append-only ledger
/// (fsynced turn/epoch closes, D34) with its own hardened test suite: a
/// shared helper would mean any future change to the memory-purge rewrite
/// shape risks the log-repair path (and vice versa) without either call
/// site's tests naming the coupling. Kept as a deliberate near-duplicate,
/// same rationale F6 already used for skipping `put_result` reuse above.
fn rewrite_log_atomic(log_path: &Path, lines: &[&str]) -> Result<(), String> {
    let parent = log_path
        .parent()
        .ok_or_else(|| "log.jsonl has no parent directory".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let tmp = parent.join(format!(
        ".log.jsonl.tmp.{}.{}",
        std::process::id(),
        wall_now_ms()
    ));

    let write = (|| -> std::io::Result<()> {
        let mut file = create_tmp_file(&tmp)?;
        for line in lines {
            writeln!(file, "{line}")?;
        }
        file.sync_all()?;
        std::fs::rename(&tmp, log_path)?;
        Ok(())
    })();

    match write {
        Ok(()) => {
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(format!("failed to rewrite log.jsonl: {e}"))
        }
    }
}

// ---- purge --orphans (superseded-snapshot GC) ------------------------------
//
// The daemon `put`s a file's current content into the CAS on EVERY debounced
// change (`daemon::Recorder::stage`) — load-bearing for crash recovery: a
// kill-9 mid-turn must find the open turn's current content already durable in
// the store so `recover_orphan` can reconstruct a valid `after` hash. But the
// committed `TurnRecord` is coarse — it keeps only the turn's FIRST `before`
// and LAST `after` per file. Every intermediate content state a file passed
// through therefore leaves a blob no record references the instant the file
// advances again. These "orphans" are the inevitable byproduct of continuous
// crash-safe snapshotting — not a bug, not crash residue — and over a heavy
// dogfood week they can dwarf the referenced set. Nothing else reclaims them
// (budget eviction walks only turn-referenced snapshot blobs; TTL purge only
// touches prompt blobs), so this is their sole reclaim path.
//
// SAFETY — deleting "by absence" is correct iff the reference set is COMPLETE:
// a missed reference means a live blob wrongly reclaimed. Two guards keep the
// ref-set complete in the safe (over-keep) direction:
//
//  (1) It is harvested by a raw `sha256:<64hex>` byte-scan of every file that
//      can cite a CAS blob — ALL THREE: `log.jsonl` (snapshot `before`/`after`
//      + `prompt_ref`), `open.json` (the in-flight crash-journal turn a future
//      `recover_orphan` will resurrect), and `memory.jsonl` (a memory pin's
//      `hash` is a file-content sha256 that `verify`'s `print_pin_diff` looks
//      up as a CAS blob to render old content — a pinned version that was
//      snapshotted then superseded would otherwise look orphaned). NEVER via
//      `load_log`, which silently drops torn/unknown-type lines; a blob cited
//      only by such a line would then look orphaned. The raw scan yields the
//      hash whether or not the line parses, so it can only ever KEEP more than
//      a structured read would, never less.
//  (2) `daemon_is_running` refusal (the daemon appends blobs + turns
//      continuously) plus a `pass_start` mtime guard skipping any blob written
//      after the scan began — covering the narrow race where a manual `undo`
//      (the only other blob writer) `put`s a blob an instant before appending
//      the turn that references it. The residual TOCTOU is real but bounded:
//      an `undo` whose `put` lands BEFORE `pass_start` yet whose turn append
//      lands AFTER the log scan would archive that blob. It is archive-only
//      (recoverable, never a delete) and the same honest "narrowed, not
//      closed" posture as `purge_log_duplicates`' length-recheck — running an
//      `undo` concurrently with a manual `purge --orphans` is the only way to
//      hit it, and the fix is to move the archive dir back.
//
// Reclaimed blobs are archive-*renamed* into `.agentrec/objects.archived.<ts>/`
// (same-fs, cheap, fan-out preserved) — never deleted — so "never delete user
// data" holds trivially: moving the archive dir back under `objects/` fully
// restores the pre-reclaim store.

/// `purge --orphans`: archive every CAS blob referenced by no turn/prompt in
/// `log.jsonl`, no in-flight turn in `open.json`, and no pin in `memory.jsonl`.
/// See the module comment above for the completeness/safety argument.
fn purge_orphans(root: &Path) -> Result<(), String> {
    if crate::daemon::daemon_is_running(root) {
        return Err(
            "stop recording (agentrec is running) before reclaiming orphans — \
             the daemon appends new snapshot blobs and turns concurrently"
                .to_string(),
        );
    }

    let pass_start = SystemTime::now();
    let store = BlobStore::new(objects_dir(root));
    let referenced = referenced_hashes(root);

    let all = store.list_hashes();
    let scanned = all.len();
    let archive_dir = objects_archive_path(root);

    let mut count = 0usize;
    let mut bytes = 0u64;
    for hash in all {
        if referenced.contains(&hash) {
            continue;
        }
        // (2) skip a blob written after this pass began — a racing `undo`
        // may have `put` it just before appending its referencing turn.
        if store.mtime(&hash).is_some_and(|m| m > pass_start) {
            continue;
        }
        if let Some(size) = store.archive(&hash, &archive_dir) {
            count += 1;
            bytes += size;
        }
    }

    if count == 0 {
        println!("scanned {scanned} blob(s), 0 orphaned (superseded) — nothing to reclaim");
    } else {
        agentrec_core::perms::lock_dir(&archive_dir);
        println!(
            "reclaimed {count} orphaned (superseded) blob(s) of {scanned} scanned, {} freed — archived to {}",
            human_bytes(bytes),
            archive_dir.display()
        );
    }
    Ok(())
}

/// Every `sha256:<64hex>` ref cited by any file that can reference a CAS blob:
/// `log.jsonl` (snapshots + prompts), `open.json` (the in-flight crash-journal
/// turn), and `memory.jsonl` (pin hashes `verify`'s pin-diff resolves as
/// blobs). A RAW byte-scan (never `load_log`) so a hash on a torn/unknown line
/// still counts — see the module SAFETY note. Returns refs in `sha256:` form,
/// matching `BlobStore::list_hashes`.
pub(crate) fn referenced_hashes(root: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    for path in [log_path(root), crate::open_path(root), memory_path(root)] {
        if let Ok(text) = std::fs::read_to_string(&path) {
            harvest_refs(&text, &mut out);
        }
    }
    out
}

/// Bytes currently held by orphaned (unreferenced) blobs — what
/// `purge --orphans` would reclaim. Shared with `status`' over-budget notice
/// so the trigger (total store size) and the honest remedy attribution use one
/// definition of "orphan". Applies the same `pass_start` mtime guard as the
/// command so a just-written (racing) blob isn't counted reclaimable.
pub(crate) fn orphan_bytes(root: &Path, store: &BlobStore) -> u64 {
    let pass_start = SystemTime::now();
    let referenced = referenced_hashes(root);
    store
        .list_hashes()
        .into_iter()
        .filter(|h| !referenced.contains(h))
        .filter(|h| store.mtime(h).is_none_or(|m| m <= pass_start))
        .filter_map(|h| store.size(&h))
        .sum()
}

/// Scan `text` for every `sha256:` followed by exactly 64 lowercase-hex
/// chars, inserting each as a `sha256:<hex>` ref. Anchored on the literal
/// `sha256:` prefix (not a bare 64-hex match) so an unrelated hex run can't
/// fool it, yet oblivious to JSON structure so a torn line still yields its
/// hashes. `pub(crate)` (mech, Phase 1): `cmds::status_report` reuses this
/// same primitive to build its OWN narrower protect-set — unlike
/// `referenced_hashes` above (which deliberately folds in every VALID
/// log.jsonl reference too, correct for `--orphans`' absence test), budget
/// eviction's own age-based walk already decides a validly-referenced
/// blob's fate; only refs invisible to that walk (open.json, memory.jsonl,
/// torn log.jsonl lines) need harvesting separately.
pub(crate) fn harvest_refs(text: &str, out: &mut HashSet<String>) {
    const PREFIX: &[u8] = b"sha256:";
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + PREFIX.len() + 64 <= bytes.len() {
        if &bytes[i..i + PREFIX.len()] == PREFIX {
            let hex = &bytes[i + PREFIX.len()..i + PREFIX.len() + 64];
            if hex.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                out.insert(format!("sha256:{}", std::str::from_utf8(hex).unwrap()));
                i += PREFIX.len() + 64;
                continue;
            }
        }
        i += 1;
    }
}

/// `.agentrec/objects.archived.<unix_ts>/` — mirrors `memory_archive_path`'s
/// naming; a directory (fan-out preserved) rather than a `.jsonl` file.
fn objects_archive_path(root: &Path) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    agentrec_dir(root).join(format!("objects.archived.{ts}"))
}

/// Read `ttl_days` from `.agentrec/config.toml` via the shared
/// [`crate::cmds::config_values`] scanner. Missing file, missing key, or an
/// unparseable value all fall back to the documented default of 90.
fn read_ttl_days(root: &Path) -> u64 {
    let Some(text) = crate::cmds::read_config_text(root) else {
        return DEFAULT_TTL_DAYS;
    };
    for value in crate::cmds::config_values(&text, "ttl_days") {
        if let Ok(n) = value.parse::<u64>() {
            return n;
        }
    }
    DEFAULT_TTL_DAYS
}

/// RFC 3339 cutoff `ttl_days` before now — turns started earlier than this
/// are expired. RFC 3339's fixed-width fields compare lexically in time
/// order (same trick used elsewhere in this codebase, e.g. `readcmds::has_gap_after`).
fn ttl_cutoff(ttl_days: u64) -> String {
    let now_ms = wall_now_ms();
    let cutoff_ms = now_ms.saturating_sub(ttl_days.saturating_mul(DAY_MS));
    agentrec_core::time::rfc3339(cutoff_ms)
}

/// `YYYY-MM-DD` → the RFC 3339 midnight-UTC instant of that date, in the same
/// fixed-width format `TurnRecord.started` uses, so lexical comparison works.
fn parse_date_cutoff(date: &str) -> Result<String, String> {
    let parts: Vec<&str> = date.split('-').collect();
    let invalid = || format!("invalid date '{date}' — expected YYYY-MM-DD");
    if parts.len() != 3 {
        return Err(invalid());
    }
    let y: u32 = parts[0].parse().map_err(|_| invalid())?;
    let m: u32 = parts[1].parse().map_err(|_| invalid())?;
    let d: u32 = parts[2].parse().map_err(|_| invalid())?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(invalid());
    }
    Ok(format!("{y:04}-{m:02}-{d:02}T00:00:00.000Z"))
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors `memory::slow_pin_read_delay_is_none_in_release_even_with_env_set`:
    /// in a release build (`debug_assertions` off), the F4 pause seam must
    /// return `None` even when the env var IS set — proves the
    /// `#[cfg(not(debug_assertions))]` arm actually compiles out the env
    /// read rather than merely being unreachable dead code. Only runs under
    /// `cargo test --release` (the `debug_assertions`-on debug test build
    /// never exercises this arm at all).
    #[test]
    #[cfg(not(debug_assertions))]
    fn pause_before_memory_rewrite_delay_is_none_in_release_even_with_env_set() {
        std::env::set_var("AGENTREC_TEST_PAUSE_BEFORE_MEMORY_REWRITE_MS", "5000");
        assert_eq!(
            test_pause_before_memory_rewrite_delay(),
            None,
            "release builds must never honor AGENTREC_TEST_PAUSE_BEFORE_MEMORY_REWRITE_MS"
        );
        std::env::remove_var("AGENTREC_TEST_PAUSE_BEFORE_MEMORY_REWRITE_MS");
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn pause_before_log_rewrite_delay_is_none_in_release_even_with_env_set() {
        std::env::set_var("AGENTREC_TEST_PAUSE_BEFORE_LOG_REWRITE_MS", "5000");
        assert_eq!(
            test_pause_before_log_rewrite_delay(),
            None,
            "release builds must never honor AGENTREC_TEST_PAUSE_BEFORE_LOG_REWRITE_MS"
        );
        std::env::remove_var("AGENTREC_TEST_PAUSE_BEFORE_LOG_REWRITE_MS");
    }

    #[test]
    fn ttl_days_defaults_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_ttl_days(tmp.path()), DEFAULT_TTL_DAYS);
    }

    #[test]
    fn ttl_days_parses_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(agentrec_dir(tmp.path())).unwrap();
        std::fs::write(
            agentrec_dir(tmp.path()).join("config.toml"),
            "ttl_days = 30\nmcp_destructive = \"off\"\n",
        )
        .unwrap();
        assert_eq!(read_ttl_days(tmp.path()), 30);
    }

    #[test]
    fn date_cutoff_parses_and_rejects() {
        assert_eq!(
            parse_date_cutoff("2024-06-01").unwrap(),
            "2024-06-01T00:00:00.000Z"
        );
        assert!(parse_date_cutoff("not-a-date").is_err());
        assert!(parse_date_cutoff("2024-13-01").is_err());
    }

    #[test]
    fn harvest_refs_yields_hashes_from_torn_and_valid_lines() {
        let a = format!("sha256:{}", "a".repeat(64));
        let b = format!("sha256:{}", "b".repeat(64));
        // second line is deliberately truncated (torn) JSON — its hash must
        // still be harvested (raw scan is oblivious to parseability).
        let text = format!("valid {{\"after\":\"{a}\"}}\n<torn json {b}");
        let mut out = HashSet::new();
        harvest_refs(&text, &mut out);
        assert!(out.contains(&a) && out.contains(&b));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn harvest_refs_ignores_short_or_uppercase_hex() {
        let short = format!("sha256:{}", "a".repeat(63));
        let upper = format!("sha256:{}", "A".repeat(64));
        let mut out = HashSet::new();
        harvest_refs(
            &format!("{short} {upper} tail padding padding padding"),
            &mut out,
        );
        assert!(out.is_empty(), "63-hex and uppercase must not match");
    }

    // The load-bearing safety test: `purge --orphans` archives ONLY blobs no
    // record references, and — critically — a blob referenced only by a TORN
    // (unparseable) `log.jsonl` line is NOT reclaimed, because the ref-set is
    // a raw byte-scan, not `load_log` (which would silently drop that line).
    #[test]
    fn purge_orphans_archives_only_unreferenced_and_spares_torn_line_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let ref_snap = store.put(b"snapshot content").unwrap();
        let ref_prompt = store.put(b"prompt content").unwrap();
        let pending = store.put(b"in-flight open turn content").unwrap();
        let torn = store.put(b"referenced only by a torn line").unwrap();
        let orphan = store.put(b"superseded intermediate state").unwrap();

        // log.jsonl: one well-formed turn line citing ref_snap + ref_prompt,
        // then a truncated (torn) line still citing `torn`.
        let log = format!(
            "{{\"v\":1,\"id\":\"t_X\",\"files\":[{{\"after\":\"{ref_snap}\"}}],\"prompt_ref\":\"{ref_prompt}\"}}\n\
             {{\"v\":1,\"id\":\"t_TORN\",\"files\":[{{\"after\":\"{torn}\"\n"
        );
        std::fs::write(log_path(root), log).unwrap();
        // open.json crash journal citing the pending blob.
        std::fs::write(
            crate::open_path(root),
            format!("{{\"files\":[{{\"before_hash\":\"{pending}\"}}]}}"),
        )
        .unwrap();

        purge_orphans(root).unwrap();

        assert!(store.contains(&ref_snap), "snapshot-referenced blob kept");
        assert!(store.contains(&ref_prompt), "prompt-referenced blob kept");
        assert!(store.contains(&pending), "open.json pending blob kept");
        assert!(
            store.contains(&torn),
            "blob cited only by a torn line kept (raw scan, not load_log)"
        );
        assert!(!store.contains(&orphan), "orphan archived out of the store");

        // The orphan is preserved in the archive dir, never deleted.
        let hex = orphan.strip_prefix("sha256:").unwrap();
        let archived = std::fs::read_dir(agentrec_dir(root))
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("objects.archived.")
            })
            .expect("archive dir created");
        let arch_blob = archived.path().join(&hex[..2]).join(&hex[2..]);
        assert!(
            arch_blob.exists(),
            "orphan preserved in archive (never deleted)"
        );
    }

    // A memory pin's `hash` is a file-content sha256 that `verify`'s pin-diff
    // resolves as a CAS blob. If that pinned version was snapshotted then
    // superseded, it looks orphaned to a log-only ref-set — but memory.jsonl
    // is in the ref-set, so it must be kept.
    #[test]
    fn purge_orphans_keeps_a_memory_pinned_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        // A blob referenced by NO turn — only by a memory pin.
        let pinned = store
            .put(b"a pinned file version, later superseded")
            .unwrap();
        std::fs::write(log_path(root), "").unwrap(); // no turns reference it
        std::fs::write(
            memory_path(root),
            format!(
                "{{\"v\":1,\"type\":\"memory\",\"id\":\"01AAA\",\"op\":\"assert\",\"fact\":\"x\",\"pins\":[{{\"path\":\"f.rs\",\"hash\":\"{pinned}\"}}],\"source_turns\":[],\"origin\":\"human\",\"ts\":1}}\n"
            ),
        )
        .unwrap();

        purge_orphans(root).unwrap();

        assert!(
            store.contains(&pinned),
            "a memory-pinned blob must never be archived as an orphan"
        );
    }

    #[test]
    fn orphan_bytes_counts_only_unreferenced() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let kept = store.put(&[0xAAu8; 40]).unwrap();
        let _orphan = store.put(&[0xBBu8; 25]).unwrap();
        std::fs::write(
            log_path(root),
            format!("{{\"files\":[{{\"after\":\"{kept}\"}}]}}\n"),
        )
        .unwrap();

        assert_eq!(orphan_bytes(root, &store), 25, "only the 25-byte orphan");
    }
}
