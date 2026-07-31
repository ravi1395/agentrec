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
use crate::{agentrec_dir, log_path, objects_dir, signal_path};
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
    signals_consumed: bool,
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
    if signals_consumed {
        purge_signals_consumed(root)?;
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

    // (c) atomic rewrite of memory.jsonl — the only sanctioned rewrite site
    // for THIS file (the other two classes rewrite log.jsonl/signal.jsonl).
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

/// `purge --log-duplicates`: mirrors `purge_memories_retracted`'s shape — one
/// of the three sanctioned rewrite classes in this codebase, in the order they
/// landed: `--memories-retracted` (first), `--log-duplicates` (second, here),
/// `--signals-consumed` (third, D46) — daemon-liveness
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
                    .any(|kept| crate::readcmds::same_revert(kept, &t));
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
    write_full_file_synced(&archive_path, original.as_bytes())?;
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
///
/// Takes raw BYTES, not `&str`: `purge --signals-consumed` archives a byte
/// slice of `signal.jsonl` that must round-trip verbatim, and a prefix cut at
/// a line boundary is still not guaranteed to be valid UTF-8 (a hook could
/// have written invalid bytes; `String::from_utf8_lossy` would silently
/// substitute replacement characters into the archive).
fn write_full_file_synced(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(content).map_err(|e| e.to_string())?;
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

// ---- purge --signals-consumed (hook-inbox prefix truncation, D46) ----------
//
// `signal.jsonl` is the emitter -> recorder inbox. Every hook fire appends a
// line carrying the SCRUBBED PROMPT TEXT, and nothing has ever removed one:
// on the dogfood store the inbox reached 13.4 MB / 1932 lines against a 2.5 MB
// `log.jsonl`. Retention (`purge` TTL, budget eviction, `--orphans`) covers
// the object store only, so the inbox grew without bound.
//
// The consumed prefix is genuinely redundant, not merely old: the daemon
// persists a signal's prompt text into the CAS (`daemon.rs`'s `persist`) and
// cites it from the closed turn's `prompt_ref` at consumption time. So every
// byte before `state.json`'s `signal_offset` has already been transcribed into
// the two durable surfaces (`log.jsonl` + `objects/`) that every read verb
// actually reads. Nothing in the codebase re-reads the inbox below
// `signal_offset` — `SignalTailer::poll` and `replay_pending_candidates` both
// start AT that offset and never read behind it. The offset itself moves
// forward on every consumption; the ONE exception is a detected shrink, where
// both sites resync it DOWN to the file's real EOF
// (`daemon::resync_shrunk_signal_offset`). That is not a re-read of consumed
// bytes — the bytes below it are gone from the file — so the redundancy
// argument above is unaffected.
//
// This is the THIRD sanctioned rewrite class (D46). The full set, in landing
// order: `purge --memories-retracted`, `purge --log-duplicates`, and this one.
// What survives of "append-only": emitters only ever
// append, no line is ever mutated, reordered, or rewritten in place, and only
// WHOLE already-consumed lines leave the file. What changes: the file's byte
// offsets are rebased, so `signal_offset` must be rebased in the same
// operation or the daemon would either replay (offset too low) or resync-and-
// drop (offset too high).
//
// CONCURRENCY — deliberately weaker than `purge --log-duplicates`, do not
// copy that comment's confidence here. `log.jsonl` has exactly one non-daemon
// writer (`undo --confirm`) and it takes `log.lock` blocking, so the
// recheck->rename window is backstopped by a lock. `signal.jsonl` has TWO
// non-daemon writers — `cmds::hook` (every prompt/stop, whether or not a
// daemon is running) and `memorycmds`' candidate emitter — and NEITHER takes
// any lock. A `signal.lock` was considered and rejected for now: the hook path
// is the latency-critical surface guarded by the `hook_recall_hard_wall_
// deadline` test (a blocking flock there could park a hook behind a
// maintenance command), and covering only one of the two writers would buy a
// false sense of closure. So the length recheck below is the ONLY guard, and
// its residual is real and unbacked: an append landing in the microseconds
// between the recheck and the rename is lost from the rewritten file, and the
// archive holds only the consumed prefix, so it is not recoverable from there
// either. It is a manual, human-invoked maintenance command on a stopped
// daemon; the honest mitigation is that any growth detected before the rename
// aborts the whole operation with a rerun instruction.

/// `purge --signals-consumed` (D46): truncate `signal.jsonl` to its unconsumed
/// tail. Mirrors `purge_log_duplicates`' shape — daemon-liveness refusal,
/// archive-before-touch, length recheck, atomic tmp+fsync+rename+dir-fsync —
/// with three differences, each load-bearing:
///
/// (a) It operates on BYTES, never lines. The unconsumed tail routinely ends
///     mid-line (a hook is appending as we read), and both existing rewrite
///     helpers take `&[&str]` and `writeln!` each element — which would append
///     a newline to that torn final line, promoting a partial signal into one
///     `SignalTailer::poll`'s `rposition(b'\n')` scan treats as complete and
///     hands to `parse_signals`. The tail is copied verbatim.
/// (b) It refuses rather than guesses on any inconsistency: a missing
///     `state.json` (the only record of what was consumed), an offset past
///     EOF, or an offset that does not land just after a `\n`. A consumed
///     offset is always a line boundary by construction (both readers advance
///     only to `rposition(b'\n') + 1`), so a mid-line offset means something
///     already went wrong — truncating there would decapitate a signal line.
/// (c) Ordering is rename-THEN-rebase, and that direction is load-bearing.
///     A crash between them leaves a large `signal_offset` against a short
///     file, which the daemon detects AT STARTUP OR MID-RUN and reconciles
///     identically via `daemon::resync_shrunk_signal_offset`: it logs loudly,
///     counts a DEGRADED I/O failure, and resyncs the offset to the new EOF
///     *and persists it* — the unconsumed tail's bytes survive on disk, they
///     are just never processed. Both sites matter here and the startup one
///     is the one this command's own refusal text sends users to: red-team D1
///     found only the mid-run site (`SignalTailer::poll`, `len < self.offset`)
///     existed, so a crash in this window was never reconciled by restarting
///     the daemon and this command refused forever with no escape.
///     The reverse order (rebase first) would leave offset 0
///     against the full file and REPLAY every consumed start/stop signal as
///     duplicate turns, corrupting the ledger. Losing an unprocessed signal is
///     recoverable by re-prompting; a fabricated turn is not.
fn purge_signals_consumed(root: &Path) -> Result<(), String> {
    purge_signals_consumed_inner(root, None)
}

/// The body of [`purge_signals_consumed`], with the recheck-window widener as
/// a PARAMETER rather than an env-var seam.
///
/// Deliberately NOT the `AGENTREC_TEST_PAUSE_BEFORE_*_MS` shape the memory and
/// log rewrites use. Those are read by a spawned `CARGO_BIN_EXE_agentrec`
/// subprocess (`cli/tests/hardening_cli.rs`), so the `set_var` happens in a
/// parent that isn't itself multi-threaded over that variable. This function's
/// race test lives in-process, where a debug-path `std::env::set_var` would
/// run concurrently (`--test-threads=3`) with every other test calling
/// `std::env::var` — the classic unsound set_var/var data race, for a seam no
/// acceptance criterion requires to be externally injectable. A parameter also
/// makes the fail-safe property STRONGER than the env seam's: a production
/// binary has no code path that can sleep here at all, rather than one that is
/// merely compiled out.
fn purge_signals_consumed_inner(
    root: &Path,
    pause_before_recheck: Option<std::time::Duration>,
) -> Result<(), String> {
    // Same belt-and-suspenders posture as the other rewrites: the daemon both
    // appends nothing here and advances `signal_offset` continuously, so a
    // live daemon would race the rebase as well as the rewrite.
    if crate::daemon::daemon_is_running(root) {
        return Err(
            "stop recording (agentrec is running) before truncating signal.jsonl — \
             the daemon consumes the inbox and advances signal_offset concurrently"
                .to_string(),
        );
    }

    // `state.json` is the ONLY record of how much of the inbox was consumed.
    // Missing means either "never recorded here" or "operational state was
    // deleted"; both are cases where any offset we picked would be a guess,
    // and guessing low replays signals while guessing high destroys them.
    // Deliberately NOT gated on `state_parse_failures`: that is a persisted
    // LIFETIME counter, so gating on it would refuse forever after a single
    // historical corruption. A `signal_offset` that fails to parse degrades to
    // 0 (state.rs' per-field fallback), which lands on the no-op path below —
    // safe by construction.
    if !crate::state_path(root).exists() {
        return Err(
            "no .agentrec/state.json — cannot tell which signal lines were already \
             consumed; refusing to guess (start `agentrec record` once, then retry)"
                .to_string(),
        );
    }
    let offset = crate::state::read_state(root).signal_offset;

    let path = signal_path(root);
    let Ok(bytes) = std::fs::read(&path) else {
        println!("scanned 0 B of signal inbox — no signal.jsonl yet");
        return Ok(());
    };
    let len = bytes.len() as u64;

    if offset == 0 {
        println!(
            "signal inbox {} — 0 B consumed, nothing to reclaim",
            human_bytes(len)
        );
        return Ok(());
    }
    if offset > len {
        return Err(format!(
            "state.json's signal_offset ({offset}) is past the end of signal.jsonl \
             ({len} B) — the inbox was already truncated or rewritten externally; \
             refusing to truncate against an inconsistent offset (start `agentrec \
             record` once: the daemon detects the shrink at startup, resyncs the \
             offset to the file's end and reports DEGRADED — any signals in the \
             gap are already lost — then retry; if the file's last line is torn \
             mid-write the retry refuses once more on the line boundary until one \
             more hook fire completes that line)"
        ));
    }
    // `offset` is a COUNT of consumed bytes, so `offset - 1` is the last
    // consumed byte and MUST be the newline ending the last consumed line.
    if bytes[offset as usize - 1] != b'\n' {
        return Err(format!(
            "state.json's signal_offset ({offset}) does not land on a line boundary \
             — refusing to truncate through a partial signal line (a consumed offset \
             is always a line boundary by construction; this state.json is corrupt)"
        ));
    }
    let (consumed, tail) = bytes.split_at(offset as usize);

    // Archive the consumed prefix, fsynced, BEFORE the source is touched at
    // all — "never delete user data" (house rule). Unlike
    // `purge --log-duplicates` the archive holds the removed bytes rather than
    // the whole file: the removed prefix here is the bulk of a multi-megabyte
    // file, and the retained tail is untouched on disk, so prefix + live file
    // still reconstructs the original exactly.
    let archive_path = signal_archive_path(root);
    write_full_file_synced(&archive_path, consumed)?;
    agentrec_core::perms::lock_file(&archive_path);

    if let Some(delay) = pause_before_recheck {
        std::thread::sleep(delay);
    }

    // The only guard against the two unlocked appenders (see the section
    // comment): `signal.jsonl` is append-only from every writer, so any
    // concurrent write can only grow it. Any change in length since the read
    // above means our captured `tail` is already stale — abort entirely rather
    // than rewrite a file we no longer have the whole of.
    //
    // The archive written moments ago is REMOVED on this path, and that is not
    // a "never delete user data" violation: nothing has been taken out of
    // `signal.jsonl` yet, so at this instant the archive is a pure redundant
    // copy of bytes still fully present in the live inbox. Leaving it would
    // mean each aborted run mints another multi-megabyte
    // `signal.archived.<ts>.jsonl` inside `.agentrec/` — the command whose
    // whole purpose is bounding `.agentrec/` growth would grow it on failure,
    // and the founder-pending manual-`rm` archive burden with it. Best-effort:
    // a failed removal is not worth failing the (already failing) command over,
    // and the leftover copy is harmless.
    let current_len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if current_len != len {
        let _ = std::fs::remove_file(&archive_path);
        return Err(
            "signal.jsonl changed during truncation (a hook appended) — no changes \
             made; rerun `agentrec purge --signals-consumed`"
                .to_string(),
        );
    }

    rewrite_signal_atomic(&path, tail)?;
    agentrec_core::perms::lock_file(&path);

    // (c) above: rebase AFTER the rename. The tail now starts at byte 0, so
    // every consumed byte is gone and the new consumed count is exactly 0.
    let mut state = crate::state::read_state(root);
    state.signal_offset = 0;
    crate::state::write_state(root, &state).map_err(|e| {
        format!(
            "signal.jsonl was truncated but rebasing signal_offset failed: {e} — \
             start `agentrec record` to resync (the daemon detects the shrink at \
             startup, resumes at the new end, and reports DEGRADED); the {} unconsumed \
             byte(s) still in the inbox will be skipped",
            tail.len()
        )
    })?;

    println!(
        "reclaimed {} of consumed signal inbox ({} unconsumed retained) — archived to {}",
        human_bytes(offset),
        human_bytes(tail.len() as u64),
        archive_path.display()
    );
    Ok(())
}

/// `.agentrec/signal.archived.<unix_ts>.jsonl` — mirrors `log_archive_path`'s
/// naming (whole-seconds unix timestamp).
fn signal_archive_path(root: &Path) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    agentrec_dir(root).join(format!("signal.archived.{ts}.jsonl"))
}

/// Rewrite `signal.jsonl` to contain exactly `tail`'s bytes, atomically:
/// per-process-unique tmp file in the same directory, fsync it, rename over
/// `sig_path`, then fsync the parent dir — the same tmp+fsync+rename+dir-fsync
/// shape as `rewrite_log_atomic`.
///
/// Byte-verbatim by signature (`&[u8]`, one `write_all`), never line-oriented:
/// see difference (a) on `purge_signals_consumed`. Kept as a deliberate
/// near-duplicate of `rewrite_log_atomic` for the same reason that one is not
/// unified with `rewrite_memory_atomic` — three files with three different
/// crash/concurrency stories, none of which should be able to change the
/// others' rewrite shape without their own tests naming the coupling.
///
/// An empty `tail` (everything consumed — the steady state on a live store)
/// writes a 0-byte file rather than removing it: `doctorcmd`'s signal-freshness
/// check reads a MISSING `signal.jsonl` as the hooks-disconnected failure mode,
/// so deleting it here would make a successful reclaim look like a broken
/// install.
fn rewrite_signal_atomic(sig_path: &Path, tail: &[u8]) -> Result<(), String> {
    let parent = sig_path
        .parent()
        .ok_or_else(|| "signal.jsonl has no parent directory".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let tmp = parent.join(format!(
        ".signal.jsonl.tmp.{}.{}",
        std::process::id(),
        wall_now_ms()
    ));

    let write = (|| -> std::io::Result<()> {
        let mut file = create_tmp_file(&tmp)?;
        file.write_all(tail)?;
        file.sync_all()?;
        std::fs::rename(&tmp, sig_path)?;
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
            Err(format!("failed to rewrite signal.jsonl: {e}"))
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

    // ---- purge --signals-consumed (D46) -----------------------------------

    /// A signal inbox whose first `consumed_lines` lines the daemon has
    /// already eaten, plus a deliberately TORN final line (no trailing
    /// newline) — the normal on-disk shape while a hook is mid-append, and
    /// the shape a line-oriented rewrite would silently "repair" by adding a
    /// newline. Writes `state.json` with the matching `signal_offset`.
    /// Returns (root-owned tempdir, whole original bytes, consumed offset).
    fn signal_fixture(consumed_lines: usize) -> (tempfile::TempDir, Vec<u8>, u64) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();

        let mut bytes: Vec<u8> = Vec::new();
        for i in 0..4 {
            bytes.extend_from_slice(
                format!(
                    "{{\"v\":1,\"ts\":{i},\"event\":\"stop\",\"prompt\":\"secret-ish {i}\"}}\n"
                )
                .as_bytes(),
            );
        }
        // Torn tail: a partial line with NO trailing newline.
        bytes.extend_from_slice(b"{\"v\":1,\"ts\":9,\"even");
        std::fs::write(signal_path(root), &bytes).unwrap();

        // Offset = end of the Nth complete line, exactly how the daemon
        // advances it (`rposition(b'\n') + 1`).
        let offset = bytes
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i as u64 + 1)
            .nth(consumed_lines.saturating_sub(1))
            .unwrap_or(0);

        let state = crate::state::State {
            signal_offset: offset,
            ..Default::default()
        };
        crate::state::write_state(root, &state).unwrap();
        (tmp, bytes, offset)
    }

    // AC3.1 core: exactly the consumed prefix leaves the file, the unconsumed
    // tail survives BYTE-IDENTICALLY (including its torn, newline-less final
    // line), and the archive holds the removed bytes verbatim. Neuter: rewrite
    // the tail line-oriented (`for l in tail.lines() { writeln!(..) }`) → RED
    // (the torn line gains a newline).
    #[test]
    fn signals_consumed_drops_exactly_the_consumed_prefix_and_keeps_the_tail_byte_identical() {
        let (tmp, original, offset) = signal_fixture(3);
        let root = tmp.path();
        let expected_tail = original[offset as usize..].to_vec();

        purge_signals_consumed(root).unwrap();

        let after = std::fs::read(signal_path(root)).unwrap();
        assert_eq!(
            after, expected_tail,
            "unconsumed tail must survive byte-identically (no added newline on the torn line)"
        );
        assert!(
            !after.ends_with(b"\n"),
            "the torn final line must NOT gain a trailing newline"
        );

        // The removed prefix is archived verbatim, never deleted.
        let archive = std::fs::read_dir(agentrec_dir(root))
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("signal.archived.")
            })
            .expect("archive file created");
        assert_eq!(
            std::fs::read(archive.path()).unwrap(),
            &original[..offset as usize],
            "archive must hold the removed prefix verbatim"
        );
    }

    // AC3.1: the offset is rebased in the SAME operation — the tail now starts
    // at byte 0, so a stale non-zero offset would make the next daemon start
    // either skip live signals or (if past the new EOF) resync-and-drop them.
    // Neuter: delete the `state.signal_offset = 0` write → RED.
    #[test]
    fn signals_consumed_rebases_the_offset_to_zero() {
        let (tmp, _original, offset) = signal_fixture(3);
        let root = tmp.path();
        assert!(offset > 0, "fixture must have a consumed prefix");

        purge_signals_consumed(root).unwrap();

        let state = crate::state::read_state(root);
        assert_eq!(
            state.signal_offset, 0,
            "offset must be rebased to the new file's start"
        );
        assert_eq!(
            state.state_parse_failures, 0,
            "the rebase must not corrupt state.json"
        );
    }

    // Red-team D8: `signal_archive_path` stamps a WHOLE-SECOND timestamp, so
    // two archives created inside the same second would collide on one name
    // and the second would clobber the first — a silent loss of the very
    // bytes the archive exists to preserve. The collision is argued
    // unreachable (a second back-to-back run rebases to offset 0 first, and
    // offset 0 short-circuits to the no-op BEFORE any archive is written),
    // but "reasoned unreachable" was untested. This pins the reasoning.
    //
    // Asserts the archive COUNT, not just the first archive's bytes: a
    // content-only check would pass even if a second archive file appeared
    // alongside the first.
    #[test]
    fn signals_consumed_run_twice_in_one_second_no_ops_without_clobbering_the_archive() {
        let (tmp, original, offset) = signal_fixture(3);
        let root = tmp.path();
        let expected_archive = original[..offset as usize].to_vec();
        let expected_tail = original[offset as usize..].to_vec();

        let archives = |root: &Path| -> Vec<PathBuf> {
            let mut v: Vec<PathBuf> = std::fs::read_dir(agentrec_dir(root))
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().starts_with("signal.archived."))
                        .unwrap_or(false)
                })
                .collect();
            v.sort();
            v
        };

        purge_signals_consumed(root).unwrap();
        let first = archives(root);
        assert_eq!(first.len(), 1, "first run archives once: {first:?}");

        // Immediately again — same wall-clock second by construction (no
        // sleep between them), which is exactly the collision window.
        purge_signals_consumed(root).unwrap();

        let second = archives(root);
        assert_eq!(
            second, first,
            "the second run must not create, rename, or replace any archive — \
             it short-circuits on offset 0 before reaching the archive step"
        );
        assert_eq!(
            std::fs::read(&first[0]).unwrap(),
            expected_archive,
            "the first run's archive bytes must survive the second run untouched \
             (a same-second name collision would have clobbered them)"
        );
        assert_eq!(
            std::fs::read(signal_path(root)).unwrap(),
            expected_tail,
            "the second run is a no-op: the tail is unchanged and never re-truncated"
        );
        assert_eq!(
            crate::state::read_state(root).signal_offset,
            0,
            "offset stays rebased at 0 after the no-op"
        );
    }

    // AC3.1: everything consumed (the live 13.4 MB shape) is a SUCCESS that
    // leaves a 0-byte file — not a refusal, and not a deleted file
    // (`doctorcmd` reads a missing signal.jsonl as hooks-disconnected).
    #[test]
    fn signals_consumed_fully_consumed_inbox_leaves_an_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let body = b"{\"v\":1,\"ts\":1,\"event\":\"stop\"}\n";
        std::fs::write(signal_path(root), body).unwrap();
        crate::state::write_state(
            root,
            &crate::state::State {
                signal_offset: body.len() as u64,
                ..Default::default()
            },
        )
        .unwrap();

        purge_signals_consumed(root).unwrap();

        assert!(
            signal_path(root).exists(),
            "the inbox file must remain (a missing one reads as hooks-disconnected)"
        );
        assert_eq!(std::fs::read(signal_path(root)).unwrap(), Vec::<u8>::new());
        assert_eq!(crate::state::read_state(root).signal_offset, 0);
    }

    // AC3.1: refuses while the daemon holds `daemon.lock`. The lock is taken
    // here with the same raw non-blocking flock `daemon::acquire_lock` uses,
    // rather than a pid or a mock, so this exercises the real
    // `daemon_is_running` probe. Neuter: drop the liveness check → RED.
    #[test]
    fn signals_consumed_refuses_while_the_daemon_holds_the_lock() {
        use std::os::unix::io::AsRawFd;
        let (tmp, original, _offset) = signal_fixture(3);
        let root = tmp.path();

        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(agentrec_dir(root).join("daemon.lock"))
            .unwrap();
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );

        let err = purge_signals_consumed(root).unwrap_err();
        assert!(
            err.contains("stop recording"),
            "must refuse while recording: {err}"
        );
        assert_eq!(
            std::fs::read(signal_path(root)).unwrap(),
            original,
            "a refused run must leave signal.jsonl untouched"
        );
        drop(lock);
    }

    // AC3.1: an offset that does not land just after a newline can only come
    // from a corrupt state.json (both readers advance to `rposition('\n')+1`).
    // Truncating there would decapitate a signal line, so it refuses and
    // changes nothing. Neuter: drop the boundary check → RED.
    #[test]
    fn signals_consumed_refuses_a_mid_line_offset() {
        let (tmp, original, offset) = signal_fixture(3);
        let root = tmp.path();
        crate::state::write_state(
            root,
            &crate::state::State {
                signal_offset: offset - 5, // mid-line by construction
                ..Default::default()
            },
        )
        .unwrap();

        let err = purge_signals_consumed(root).unwrap_err();
        assert!(
            err.contains("line boundary"),
            "must name the inconsistency: {err}"
        );
        assert_eq!(
            std::fs::read(signal_path(root)).unwrap(),
            original,
            "a refused run must leave signal.jsonl untouched"
        );
        assert_eq!(
            crate::state::read_state(root).signal_offset,
            offset - 5,
            "a refused run must not rebase the offset either"
        );
    }

    // AC3.1: an offset PAST the end of the file means the inbox was already
    // truncated/rewritten externally — refuse rather than slice out of bounds
    // (the old `text[offset..]` shape panicked on exactly this).
    #[test]
    fn signals_consumed_refuses_an_offset_past_eof() {
        let (tmp, original, _offset) = signal_fixture(3);
        let root = tmp.path();
        crate::state::write_state(
            root,
            &crate::state::State {
                signal_offset: original.len() as u64 + 1,
                ..Default::default()
            },
        )
        .unwrap();

        let err = purge_signals_consumed(root).unwrap_err();
        assert!(err.contains("past the end"), "must name the cause: {err}");
        assert_eq!(std::fs::read(signal_path(root)).unwrap(), original);
    }

    // AC3.1: `state.json` is the only record of what was consumed. Missing
    // means any offset would be a guess — guessing low replays consumed
    // signals as duplicate turns, guessing high destroys unconsumed ones.
    #[test]
    fn signals_consumed_refuses_without_state_json() {
        let (tmp, original, _offset) = signal_fixture(3);
        let root = tmp.path();
        std::fs::remove_file(crate::state_path(root)).unwrap();

        let err = purge_signals_consumed(root).unwrap_err();
        assert!(err.contains("state.json"), "must name the cause: {err}");
        assert_eq!(std::fs::read(signal_path(root)).unwrap(), original);
    }

    // AC3.1: offset 0 (nothing consumed) is a no-op success — never an empty
    // rewrite, never an archive file.
    #[test]
    fn signals_consumed_is_a_noop_at_offset_zero() {
        let (tmp, original, _offset) = signal_fixture(3);
        let root = tmp.path();
        crate::state::write_state(root, &crate::state::State::default()).unwrap();

        purge_signals_consumed(root).unwrap();

        assert_eq!(std::fs::read(signal_path(root)).unwrap(), original);
        let archived = std::fs::read_dir(agentrec_dir(root))
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("signal.archived.")
            });
        assert!(!archived, "a no-op must not write an archive");
    }

    // The unlocked-appender guard (see the section comment): `cmds::hook`
    // appends with no lock even while the daemon is stopped. If one lands
    // between our read and the rename, our captured tail is stale — the
    // operation must ABORT with a rerun instruction, never rewrite a file it
    // no longer has the whole of. The window is widened through the `_inner`
    // PAUSE PARAMETER (never an env var — see `purge_signals_consumed_inner`'s
    // doc for why a debug-path `set_var` would be unsound here). Neuter: drop
    // the length recheck → RED (the append is silently clobbered).
    #[test]
    fn signals_consumed_aborts_when_a_hook_appends_during_the_rewrite() {
        let (tmp, original, _offset) = signal_fixture(3);
        let root = tmp.path().to_path_buf();
        let appended = b"{\"v\":1,\"ts\":99,\"event\":\"stop\"}\n";

        let sig = signal_path(&root);
        let appender = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let mut f = std::fs::OpenOptions::new().append(true).open(&sig).unwrap();
            f.write_all(appended).unwrap();
        });

        let err = purge_signals_consumed_inner(&root, Some(std::time::Duration::from_millis(400)))
            .unwrap_err();
        appender.join().unwrap();

        assert!(
            err.contains("changed during truncation"),
            "must abort and say so: {err}"
        );
        let mut expected = original.clone();
        expected.extend_from_slice(appended);
        assert_eq!(
            std::fs::read(signal_path(&root)).unwrap(),
            expected,
            "the racing append must survive intact — nothing rewritten"
        );
        // The abort path removes its own archive: at that instant nothing had
        // been taken out of the inbox, so the archive was a redundant copy —
        // and an aborted reclaim must not GROW `.agentrec/`.
        let archived = std::fs::read_dir(agentrec_dir(&root))
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("signal.archived.")
            });
        assert!(!archived, "an aborted run must not leave a stray archive");
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
