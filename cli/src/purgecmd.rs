//! `agentrec purge` (AC I5–I6): default archives prompt-blob objects for turns
//! older than `ttl_days` (config.toml, default 90) into
//! `.agentrec/objects.archived.<ts>/` (F11 — never unlinks them).
//! `--all-prompts` archives
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
//!
//! The verb as a whole refuses while the recorder daemon is live: `run`
//! probes `daemon_is_running` before any step, and each sub-op probes again
//! for itself (F11 — before that, the prompt purge ran ahead of every
//! handler's refusal and destroyed blobs the refusal implied were untouched).

use crate::cmds::wall_now_ms;
use crate::{agentrec_dir, log_path, objects_dir, signal_path};
use agentrec_core::memory::{memory_path, MemoryRecord};
use agentrec_core::record::{LogRecord, TurnRecord};
use agentrec_core::store::BlobStore;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const DEFAULT_TTL_DAYS: u64 = 90;
const DAY_MS: u64 = 86_400_000;

// F9 pushed this to 8 parameters (clippy's threshold is 7). Kept as a flat
// signature rather than folded into a flags struct: this is a 1:1 mirror of
// clap's `Command::Purge` variant, and the mirror is what makes it obvious at
// the call site that every flag is forwarded. A struct is the right shape once
// a ninth flag lands; recorded here rather than done as drive-by churn across
// this module's test call sites.
#[allow(clippy::too_many_arguments)]
pub fn run(
    root: &Path,
    all_prompts: bool,
    snapshots_before: Option<&str>,
    memories_retracted: bool,
    log_duplicates: bool,
    orphans: bool,
    signals_consumed: bool,
    path: Option<&str>,
) -> Result<(), String> {
    // F11 (red team round 2): the verb-level daemon-liveness refusal sits
    // HERE, ahead of every destructive step in this file, rather than only
    // inside the individual flag handlers. It used to live only in the
    // handlers, while `purge_prompts` ran unconditionally as the FIRST step
    // of every invocation with no liveness check of its own — so
    // `agentrec purge --orphans` against a live daemon unlinked every expired
    // prompt blob and only THEN refused, and the user read the refusal as
    // "nothing happened". Each op keeps its own check too (they are called
    // directly by this module's unit tests and must refuse on their own, and
    // defence in depth costs one flock probe): this one makes the ordering
    // property hold for the whole verb no matter what order the ops are
    // dispatched in, or what a future op forgets.
    if crate::daemon::daemon_is_running(root) {
        return Err(
            "stop recording (agentrec is running) before purging — the daemon appends \
             turns, blobs and signals concurrently"
                .to_string(),
        );
    }

    // F9: `--path` is a SURGICAL op ("forget what you recorded about this
    // file"), and it returns here rather than falling through to the shared
    // steps below. That is a deliberate exception to this verb's structure,
    // for one reason: `purge_prompts` runs UNCONDITIONALLY on every other
    // invocation, so without this early return `agentrec purge --path
    // secrets/prod.yaml` would also archive every prompt blob older than
    // `ttl_days` — 90 days of prompt text reclaimed as a side effect of a
    // request to forget one file. That is the same class of surprise F11 just
    // fixed (a step the user did not ask for, running ahead of the step they
    // did), and shipping the flag with it intact would re-open it. The clap
    // layer additionally refuses `--path` combined with any other purge flag,
    // so this return can never skip work the user asked for.
    if let Some(pattern) = path {
        return purge_path(root, pattern);
    }

    let records = agentrec_core::record::load_log(&log_path(root));
    let turns: Vec<&TurnRecord> = owned_turns(&records);
    let store = BlobStore::new(objects_dir(root));

    purge_prompts(&store, &turns, root, all_prompts)?;
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

/// Reclaim prompt-blob objects: all of them (`all_prompts`) or just those
/// belonging to turns older than `ttl_days`, keeping any blob still shared
/// with a turn inside the TTL window.
///
/// F11 (red team round 2) changed two things about this function and nothing
/// else about which blobs it selects:
///  (1) it refuses while the daemon is live, like its four sibling ops — it
///      is the only step `run` performs unconditionally, so before the fix a
///      refusal raised by a LATER op (e.g. `--orphans`) was printed after
///      this function had already destroyed blobs, reading to the user as
///      "nothing happened";
///  (2) it ARCHIVES rather than unlinks. Blobs are archive-*renamed* into
///      `.agentrec/objects.archived.<ts>/` by `BlobStore::archive` — the
///      same-fs, fan-out-preserving move `purge --orphans` uses — so moving
///      that directory back under `objects/` fully restores them, and the
///      house rule ("`purge` archives, never silent removal") holds on this
///      path too. `--all-prompts` on a large store therefore frees nothing
///      until the archive directory is removed by hand, exactly as
///      `--orphans` already behaved.
fn purge_prompts(
    store: &BlobStore,
    turns: &[&TurnRecord],
    root: &Path,
    all_prompts: bool,
) -> Result<(), String> {
    // Mirrors each sibling op's own probe (`purge_memories_retracted`,
    // `purge_log_duplicates`, `purge_orphans`, `purge_signals_consumed`).
    // `run` checks this first as well; this one keeps the guarantee local to
    // the function, which is also what this module's unit tests drive.
    if crate::daemon::daemon_is_running(root) {
        return Err(
            "stop recording (agentrec is running) before purging prompt blobs — \
             the daemon appends new turns and prompt blobs concurrently"
                .to_string(),
        );
    }

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
    // function's initial log read (in `run`) and the archive below.
    //
    // The liveness check above does NOT close this window, and the writers it
    // does not cover are the reason the reload stays: `undo`
    // (`readcmds.rs:629`) and `import` (`importcmd.rs:1477`) append turns
    // through `loglock::append_log_locked`, which takes `log.lock` — not the
    // `daemon.lock` `daemon_is_running` probes — so a concurrent manual undo
    // or import is invisible to that check; and a daemon started between the
    // probe and here is likewise unseen. Narrowed, not closed — same honest
    // posture as `purge_log_duplicates`' length recheck.
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

    let archive_dir = objects_archive_path(root);
    let (count, bytes) = archive_all(store, &candidates, &archive_dir);
    // `BlobStore::archive` creates the fan-out directories lazily, so a
    // zero-count pass leaves no empty archive directory behind and there is
    // nothing to lock down — mirrors `purge_orphans`' two-branch shape.
    let suffix = if count > 0 {
        agentrec_core::perms::lock_dir(&archive_dir);
        format!(" — archived to {}", archive_dir.display())
    } else {
        String::new()
    };
    if all_prompts {
        println!(
            "purged {count} prompt blob(s) (all), {} freed{suffix}",
            human_bytes(bytes)
        );
    } else {
        println!(
            "purged {count} expired prompt blob(s) (ttl {ttl_days}d), {} freed{suffix}",
            human_bytes(bytes)
        );
    }
    Ok(())
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

/// Archive-rename every blob in `hashes` out of the store into
/// `archive_dir`, returning `(count, bytes)` — the never-delete counterpart
/// of `delete_all`, and the same `BlobStore::archive` call `purge_orphans`
/// reclaims with (same-fs rename, fan-out layout preserved, so moving
/// `archive_dir` back under `objects/` restores the store).
fn archive_all(store: &BlobStore, hashes: &HashSet<&str>, archive_dir: &Path) -> (usize, u64) {
    let mut count = 0;
    let mut bytes = 0u64;
    for h in hashes {
        if let Some(size) = store.archive(h, archive_dir) {
            count += 1;
            bytes += size;
        }
    }
    (count, bytes)
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
/// `--signals-consumed` (third, D48) — daemon-liveness
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

// ---- purge --signals-consumed (hook-inbox prefix truncation, D48) ----------
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
// This is the THIRD sanctioned rewrite class (D48). The full set, in landing
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

/// `purge --signals-consumed` (D48): truncate `signal.jsonl` to its unconsumed
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
///     EOF, or an offset that does not land just after a `\n`. Ordinary
///     consumption always leaves a line-boundary offset (both readers advance
///     only to `rposition(b'\n') + 1`) — but the shrink resync in (c) is a
///     legitimate producer of a mid-line offset: it lands at the file's real
///     EOF, which sits mid-line whenever the last line was torn mid-write.
///     That state self-resolves after the next hook fire (hooks append whole
///     lines, so consumption advances back onto a boundary). The refusal is
///     still right either way — truncating there would decapitate a signal
///     line — but it is a wait-and-retry, not proof of corruption.
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
///     The reverse order (rebase first) would leave offset 0 against the
///     full file — NOT duplicate turns (D7's replay discipline drops
///     start/stop in the gap at any offset; probe-validated 2026-08-01):
///     the daemon would silently advance the offset to EOF across the
///     UNCONSUMED tail, and the next run of this command would archive-and-
///     drop those never-processed signals as if consumed. Rename-first makes
///     a crash announce itself (DEGRADED); rebase-first makes the same
///     crash lose the tail silently. Announced loss over silent loss.
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
             — refusing to truncate through a partial signal line. Ordinary \
             consumption always leaves a line-boundary offset; the usual cause here \
             is a detected inbox shrink whose resync landed on a torn final line — \
             that resolves itself after the next hook fire and one `agentrec record` \
             cycle, then retry. Do NOT delete state.json — it is the only record \
             of what was consumed. A daemon starting without it mints no turns \
             from the backlog (start/stop signals in the gap are dropped by \
             design) and silently advances the offset to end-of-file, after \
             which this command archives and drops signals that were never \
             processed as if they had been consumed. Only an offset that stays \
             mid-line across new hook activity indicates a hand-edited or \
             corrupt state.json"
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

// ---- purge --path (F9: path-targeted forget) -------------------------------
//
// F9 (red team round 2): every pre-existing removal path is date-, budget- or
// class-based, so a credential that was snapshotted into the CAS was permanent.
// Rotating the key, `git rm`-ing the file and gitignoring it left the
// pre-rotation bytes readable through `diff`/`show`/`undo` forever, and the
// only remediation was a blanket date purge that also destroyed unrelated
// recovery history. This op is the path-scoped remediation, and it is the
// reason F6/F7 land in the same change: without it, every shape those two
// tables still miss stays permanent.
//
// THE SEMANTICS ARE CONSTRAINED BY THE STORE MODEL, not chosen freely. Blobs
// are content-addressed: one object per distinct byte-string, shared by every
// entry whose content is identical. "Delete the blobs of file X" is therefore
// not always expressible — if `secrets/prod.yaml` and `config/example.yaml`
// ever held identical bytes, they ARE one object, and removing it would blow a
// hole in the second file's history. So the honest contract is:
//
//   archive every blob referenced ONLY by matching file entries;
//   KEEP every blob also referenced from outside the pattern, and report the
//   count loudly rather than silently doing half a job.
//
// "Outside the pattern" is deliberately over-inclusive, the same safe
// direction `purge_orphans` takes. The protect-set unions:
//   * `before`/`after` of every NON-matching file entry;
//   * EVERY `prompt_ref`, matching turn or not — a prompt blob is not
//     path-scoped, and a short file's snapshot can be byte-identical to a
//     prompt, which would make them one object;
//   * every `sha256:` ref on a log line that does NOT parse as a turn record —
//     torn lines, epochs, future record types. `load_log` drops those, so a
//     structured read alone would treat a blob cited only by a torn line as
//     unreferenced-outside-the-match and archive it;
//   * every ref in `open.json` (in-flight crash journal) and `memory.jsonl`
//     (pin hashes), via the same `referenced_hashes` primitive `--orphans` uses.
//
// `log.jsonl` IS NOT REWRITTEN. The matching paths and hashes stay in the
// record — dropping them would be a fourth sanctioned rewrite class, which
// needs a decision-register entry this op does not have. Measured consequences
// (driven through the real binary in `cli/tests/purge_path.rs`, not read off
// the source): `diff` prints `  <path>: (snapshot unavailable)` via
// `FileDiffState::Unresolvable`, and `undo`'s `build_plan` refuses the
// entry with `prior snapshot unavailable — refusing to restore` before
// mutating anything. Both are the pre-existing missing-blob paths; this op
// adds no new rendering.
//
// One qualifier, learned by measuring rather than assumed: `undo` only reaches
// that refusal when the file is otherwise UNMODIFIED. An entry whose file has
// changed (or vanished) since the turn is EXCLUDEd as modified-since first, and
// the user sees that instead — the first version of the integration test used a
// fixture with no file on disk and got `undo t_A (—)` with no refusal at all.
// Either way `undo` does not restore the purged bytes, which is the property
// that matters; the command's own output is worded to that, not to the refusal
// string alone.

/// `purge --path <PATTERN>`: archive the snapshot blobs referenced only by
/// file entries whose path matches `PATTERN`. See the module comment above for
/// the shared-blob contract and the protect-set completeness argument.
fn purge_path(root: &Path, pattern: &str) -> Result<(), String> {
    // Sibling parity: `run` already refused above, but every op in this module
    // also refuses on its own (they are driven directly by this module's unit
    // tests, and a future dispatch reorder must not be able to strip the
    // guarantee). Same posture as `purge_prompts`/`purge_orphans`.
    if crate::daemon::daemon_is_running(root) {
        return Err(
            "stop recording (agentrec is running) before purging by path — \
             the daemon appends new snapshot blobs and turns concurrently"
                .to_string(),
        );
    }
    if pattern.trim().is_empty() {
        return Err("--path requires a non-empty pattern".to_string());
    }

    let store = BlobStore::new(objects_dir(root));
    let Scan {
        candidates,
        protect,
        matched_entries,
    } = scan_for_path(root, pattern);

    if matched_entries == 0 {
        println!("no recorded file entry matches {pattern} — nothing to reclaim");
        return Ok(());
    }

    // Same A3(b) narrowing as `purge_prompts`/`purge_snapshots_before`:
    // recompute the protect-set from a FRESH read immediately before the
    // destructive step, so a turn appended by `undo`/`import` (which take
    // `log.lock`, not `daemon.lock`, and are therefore invisible to the
    // liveness probe above) between the scan and here still protects its
    // blobs. Narrowed, not closed — and archive-only, so the residual is
    // recoverable by moving the archive directory back.
    let fresh = scan_for_path(root, pattern);
    let mut archivable: Vec<&String> = candidates
        .iter()
        .filter(|h| !protect.contains(*h) && !fresh.protect.contains(*h))
        .collect();
    archivable.sort();
    let shared = candidates.len() - archivable.len();

    let archive_dir = objects_archive_path(root);
    let mut count = 0usize;
    let mut bytes = 0u64;
    for hash in &archivable {
        if let Some(size) = store.archive(hash, &archive_dir) {
            count += 1;
            bytes += size;
        }
    }

    if count > 0 {
        agentrec_core::perms::lock_dir(&archive_dir);
        println!(
            "purged {count} snapshot blob(s) of {matched_entries} file entrie(s) matching \
             {pattern}, {} freed — archived to {}",
            human_bytes(bytes),
            archive_dir.display()
        );
    } else {
        println!(
            "purged 0 snapshot blob(s) of {matched_entries} file entrie(s) matching {pattern} \
             — nothing to reclaim"
        );
    }
    if shared > 0 {
        println!(
            "  {shared} blob(s) KEPT — identical content is also referenced outside {pattern} \
             (content-addressed store: it is the same object)"
        );
    }
    if count > 0 {
        println!(
            "  log.jsonl is unchanged: those paths and hashes remain recorded. `diff` now \
             prints `(snapshot unavailable)` for them, and `undo` will not restore them — \
             an entry whose file is otherwise unmodified is refused with `prior snapshot \
             unavailable — refusing to restore`."
        );
    }
    Ok(())
}

/// One pass over everything that can reference a CAS blob, split into the
/// blobs a `--path` pattern selects and the blobs anything else protects.
struct Scan {
    /// Hashes referenced by at least one MATCHING file entry.
    candidates: HashSet<String>,
    /// Hashes referenced by anything that is not a matching file entry.
    protect: HashSet<String>,
    /// How many file entries matched — distinguishes "pattern matched nothing"
    /// from "matched, but every blob is shared".
    matched_entries: usize,
}

fn scan_for_path(root: &Path, pattern: &str) -> Scan {
    let mut candidates: HashSet<String> = HashSet::new();
    let mut protect: HashSet<String> = HashSet::new();
    let mut matched_entries = 0usize;

    let text = std::fs::read_to_string(log_path(root)).unwrap_or_default();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some(turn) = classify_turn_line(line) else {
            // Epoch, unknown record type, or a torn line. `load_log` would
            // drop it; harvest its refs raw so a blob cited only here is
            // protected. Epochs carry no refs, so this costs nothing there.
            harvest_refs(line, &mut protect);
            continue;
        };
        if let Some(pref) = turn.prompt_ref.as_deref() {
            protect.insert(pref.to_string());
        }
        for entry in &turn.files {
            let matched = path_matches(pattern, &entry.path);
            if matched {
                matched_entries += 1;
            }
            for hash in [entry.before.as_deref(), entry.after.as_deref()]
                .into_iter()
                .flatten()
            {
                if matched {
                    candidates.insert(hash.to_string());
                } else {
                    protect.insert(hash.to_string());
                }
            }
        }
    }

    // `open.json` + `memory.jsonl` (and `log.jsonl` again, harmlessly — this
    // primitive scans all three). Anything it finds is protected: an in-flight
    // turn or a memory pin is not a matching file entry.
    for hash in referenced_hashes_outside_log(root) {
        protect.insert(hash);
    }

    Scan {
        candidates,
        protect,
        matched_entries,
    }
}

/// The `open.json` + `memory.jsonl` half of [`referenced_hashes`] — the refs
/// that exist OUTSIDE `log.jsonl`. `purge_path` needs these separately because
/// it derives its own per-entry view of `log.jsonl`; folding in the whole-log
/// scan would protect every blob and make the op a no-op.
fn referenced_hashes_outside_log(root: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    for path in [crate::open_path(root), memory_path(root)] {
        if let Ok(text) = std::fs::read_to_string(&path) {
            harvest_refs(&text, &mut out);
        }
    }
    out
}

/// Does `pattern` select the recorded (root-relative, `/`-separated) path
/// `path`? Three forms, checked in order:
///   * exact equality — `src/config.ts`;
///   * directory prefix — `secrets` or `secrets/` selects `secrets/prod.yaml`
///     at any depth beneath it (only when the pattern has no glob character,
///     so a glob is never silently widened into a prefix match);
///   * glob — `*` matches within one path segment, `**` crosses separators,
///     `?` is one non-separator character.
fn path_matches(pattern: &str, path: &str) -> bool {
    if pattern == path {
        return true;
    }
    let has_glob = pattern.contains(['*', '?']);
    if !has_glob {
        let dir = pattern.trim_end_matches('/');
        if !dir.is_empty()
            && path.len() > dir.len()
            && path.starts_with(dir)
            && path.as_bytes()[dir.len()] == b'/'
        {
            return true;
        }
        return false;
    }
    glob_rec(pattern.as_bytes(), path.as_bytes())
}

/// Backtracking glob matcher. Deliberately hand-rolled rather than pulling in
/// `globset`: `agentrec-core` and the CLI both stay dependency-light, and the
/// inputs are recorded repo-relative paths (short, bounded), so the
/// backtracking cost this shape can reach on adversarial patterns is not
/// reachable from a real `log.jsonl`.
fn glob_rec(p: &[u8], t: &[u8]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    if p[0] == b'*' {
        if p.len() > 1 && p[1] == b'*' {
            // `**/x` must also match `x` at depth zero, otherwise the most
            // natural "this file anywhere" pattern misses the repo root.
            if p.len() > 2 && p[2] == b'/' && glob_rec(&p[3..], t) {
                return true;
            }
            return (0..=t.len()).any(|i| glob_rec(&p[2..], &t[i..]));
        }
        for i in 0..=t.len() {
            if t[..i].contains(&b'/') {
                break;
            }
            if glob_rec(&p[1..], &t[i..]) {
                return true;
            }
        }
        return false;
    }
    if p[0] == b'?' {
        return !t.is_empty() && t[0] != b'/' && glob_rec(&p[1..], &t[1..]);
    }
    !t.is_empty() && p[0] == t[0] && glob_rec(&p[1..], &t[1..])
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

/// Read `ttl_days` from `.agentrec/config.toml` via
/// [`crate::config::load_or_default`]. Missing file, missing key, an
/// unparseable value, or a file-level TOML parse error all fall back to the
/// documented default of 90 ([`DEFAULT_TTL_DAYS`]).
fn read_ttl_days(root: &Path) -> u64 {
    crate::config::load_or_default(root).ttl_days
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

    // ---- F11: prompt purge is behind the liveness check, and archives -----

    /// Every `.agentrec/objects.archived.<ts>/` directory, sorted. Used both
    /// to prove one was created and to prove none was.
    fn objects_archive_dirs(root: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(agentrec_dir(root)) else {
            return Vec::new();
        };
        let mut out: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("objects.archived.")
            })
            .map(|e| e.path())
            .collect();
        out.sort();
        out
    }

    const EXPIRED_PROMPT_BODY: &[u8] = b"the full scrubbed prompt text of an old turn";

    /// A store holding one prompt blob, and a `log.jsonl` whose single turn
    /// started in 2000 — well past any plausible `ttl_days`, so the default
    /// purge selects that blob. `"type":"turn"` is explicit rather than
    /// relying on the tag default, so the line's classification is not part
    /// of what these tests are trusting.
    fn expired_prompt_fixture() -> (tempfile::TempDir, BlobStore, String) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));
        let prompt = store.put(EXPIRED_PROMPT_BODY).unwrap();
        std::fs::write(
            log_path(root),
            format!(
                "{{\"type\":\"turn\",\"v\":1,\"id\":\"t_EXPIRED00000000000000001\",\
                 \"grade\":\"rich\",\"started\":\"2000-01-01T00:00:00.000Z\",\
                 \"ended\":\"2000-01-01T00:00:01.000Z\",\"root\":\"/x\",\
                 \"prompt_ref\":\"{prompt}\",\"files\":[]}}\n"
            ),
        )
        .unwrap();
        (tmp, store, prompt)
    }

    // F11 (a): with the daemon live, `purge --orphans` must destroy NOTHING.
    // Before the fix this exact call unlinked the expired prompt blob inside
    // `purge_prompts` — the unconditional first step — and only then hit the
    // orphan handler's refusal, so the user read "stop recording ... before
    // reclaiming orphans" and concluded nothing had happened.
    //
    // The lock is taken with the same raw non-blocking flock
    // `daemon::acquire_lock` uses (not a pid, not a mock), so this exercises
    // the real `daemon_is_running` probe. Neuter: drop either liveness check
    // -> RED.
    #[test]
    fn prompt_purge_destroys_nothing_while_the_daemon_holds_the_lock() {
        use std::os::unix::io::AsRawFd;
        let (tmp, store, prompt) = expired_prompt_fixture();
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

        // orphans = true: the F11 scenario verbatim.
        let err = run(root, false, None, false, false, true, false, None).unwrap_err();
        assert!(
            err.contains("stop recording"),
            "the verb must refuse while recording: {err}"
        );

        // The distinguishing assertion: archiving ALSO makes `contains`
        // false, so only "still in objects/" separates "nothing happened"
        // from "moved out, then refused".
        assert!(
            store.contains(&prompt),
            "a refused purge must leave every prompt blob in the store"
        );
        assert!(
            objects_archive_dirs(root).is_empty(),
            "a refused purge must not create an archive dir either"
        );

        // Same guarantee when the step is driven directly rather than
        // through `run`'s ordering — including under `--all-prompts`, the
        // widest selection this function offers.
        let records = agentrec_core::record::load_log(&log_path(root));
        let turns = owned_turns(&records);
        let direct = purge_prompts(&store, &turns, root, true).unwrap_err();
        assert!(
            direct.contains("stop recording"),
            "purge_prompts must refuse on its own: {direct}"
        );
        assert!(store.contains(&prompt), "direct call must destroy nothing");
        assert!(objects_archive_dirs(root).is_empty());

        drop(lock);
    }

    // F11 (b): a successful prompt purge ARCHIVES the expired blob — it
    // leaves `objects/` but survives byte-for-byte under
    // `objects.archived.<ts>/`, so moving that directory back restores it.
    // Neuter: swap `archive_all` back to `delete_all` -> RED.
    #[test]
    fn prompt_purge_archives_the_expired_blob_rather_than_unlinking_it() {
        let (tmp, store, prompt) = expired_prompt_fixture();
        let root = tmp.path();

        run(root, false, None, false, false, false, false, None).unwrap();

        assert!(
            !store.contains(&prompt),
            "the expired prompt blob must leave objects/"
        );

        let dirs = objects_archive_dirs(root);
        assert_eq!(dirs.len(), 1, "exactly one archive dir expected: {dirs:?}");
        let hex = prompt.strip_prefix("sha256:").unwrap();
        let archived = dirs[0].join(&hex[..2]).join(&hex[2..]);
        assert!(
            archived.exists(),
            "expired prompt preserved in the archive (never deleted)"
        );
        assert_eq!(
            std::fs::read(&archived).unwrap(),
            EXPIRED_PROMPT_BODY,
            "the archived blob must hold the original bytes verbatim"
        );
    }

    // ---- purge --signals-consumed (D48) -----------------------------------

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

    // AC3.1: an offset that does not land just after a newline is refused —
    // truncating there would decapitate a signal line. It is NOT proof of
    // corruption: ordinary consumption always lands on `rposition('\n')+1`,
    // but a shrink resync legitimately lands mid-line when the last line was
    // torn (self-resolves on the next hook fire). Refuses and changes
    // nothing either way. Neuter: drop the boundary check → RED.
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
    // means any offset would be a guess — a daemon rebuilding it starts at 0
    // and silently advances to EOF over never-processed signals (D7 drops
    // start/stop in the gap; probe-validated 2026-08-01), after which this
    // command would archive-and-drop them as if consumed; guessing high
    // destroys unconsumed ones directly.
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

    // ---- F9: purge --path (path-targeted forget) -------------------------

    /// One `log.jsonl` turn line. `files` is a pre-rendered JSON array so each
    /// test can shape entries exactly; `prompt` is an optional `prompt_ref`.
    fn turn_line(id: &str, files: &str, prompt: Option<&str>) -> String {
        let pref = match prompt {
            Some(p) => format!("\"prompt_ref\":\"{p}\","),
            None => String::new(),
        };
        format!(
            "{{\"type\":\"turn\",\"v\":1,\"id\":\"{id}\",\"grade\":\"rich\",\
             \"started\":\"2026-01-01T00:00:00.000Z\",\
             \"ended\":\"2026-01-01T00:00:01.000Z\",\"root\":\"/x\",{pref}\
             \"files\":{files}}}\n"
        )
    }

    fn file_entry(path: &str, before: Option<&str>, after: Option<&str>) -> String {
        let j = |v: Option<&str>| match v {
            Some(h) => format!("\"{h}\""),
            None => "null".to_string(),
        };
        format!(
            "{{\"path\":\"{path}\",\"before\":{},\"after\":{},\"op\":\"modify\"}}",
            j(before),
            j(after)
        )
    }

    /// Absolute path a blob would occupy inside `archive_dir` (fan-out layout).
    fn archived_blob(archive_dir: &Path, hash: &str) -> PathBuf {
        let hex = hash.strip_prefix("sha256:").unwrap();
        archive_dir.join(&hex[..2]).join(&hex[2..])
    }

    // The core F9 contract: blobs referenced ONLY by matching entries leave
    // the store (into the archive, never deleted); everything else stays.
    // Neuter: make `path_matches` return true unconditionally -> the
    // non-matching blobs are archived too -> RED on the `contains` asserts.
    #[test]
    fn path_purge_archives_only_blobs_exclusive_to_the_matching_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let leaked_before = store.put(b"the pre-rotation credential bytes").unwrap();
        let leaked_after = store.put(b"the post-rotation credential bytes").unwrap();
        let source_before = store.put(b"fn main() { old }").unwrap();
        let source_after = store.put(b"fn main() { new }").unwrap();

        let files = format!(
            "[{},{}]",
            file_entry(
                "secrets/prod.yaml",
                Some(&leaked_before),
                Some(&leaked_after)
            ),
            file_entry("src/main.rs", Some(&source_before), Some(&source_after))
        );
        std::fs::write(log_path(root), turn_line("t_A", &files, None)).unwrap();

        purge_path(root, "secrets/prod.yaml").unwrap();

        assert!(
            !store.contains(&leaked_before) && !store.contains(&leaked_after),
            "both blobs of the matching path must leave objects/"
        );
        assert!(
            store.contains(&source_before) && store.contains(&source_after),
            "a non-matching file's blobs must be untouched"
        );

        // Archived, not deleted — and in the same fan-out layout the sibling
        // ops use, so moving the directory back restores the store.
        let archive = objects_archive_dirs(root);
        assert_eq!(archive.len(), 1, "exactly one archive dir");
        for h in [&leaked_before, &leaked_after] {
            assert!(
                archived_blob(&archive[0], h).exists(),
                "{h} preserved in the archive (never deleted)"
            );
        }
    }

    // log.jsonl is NOT rewritten — dropping those lines would be a fourth
    // sanctioned rewrite class, which this op does not have.
    #[test]
    fn path_purge_leaves_log_jsonl_byte_identical() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));
        let blob = store.put(b"leaked").unwrap();
        let log = turn_line(
            "t_A",
            &format!("[{}]", file_entry("secrets/prod.yaml", None, Some(&blob))),
            None,
        );
        std::fs::write(log_path(root), &log).unwrap();

        purge_path(root, "secrets/prod.yaml").unwrap();

        assert!(!store.contains(&blob), "the blob is gone from objects/");
        assert_eq!(
            std::fs::read_to_string(log_path(root)).unwrap(),
            log,
            "log.jsonl must be byte-identical: paths and hashes stay recorded"
        );
    }

    // The content-addressing consequence: identical bytes are ONE object. A
    // blob a non-matching entry also references must be kept, not silently
    // half-removed. Neuter: drop the `else { protect.insert(..) }` arm in
    // `scan_for_path` -> the shared blob is archived -> RED.
    #[test]
    fn path_purge_keeps_a_blob_shared_with_a_non_matching_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        // Same bytes recorded under two paths => one CAS object.
        let shared = store.put(b"identical content under two paths").unwrap();
        let exclusive = store.put(b"content only the matching path has").unwrap();
        let files = format!(
            "[{},{}]",
            file_entry("secrets/prod.yaml", Some(&shared), Some(&exclusive)),
            file_entry("docs/example.yaml", None, Some(&shared))
        );
        std::fs::write(log_path(root), turn_line("t_A", &files, None)).unwrap();

        purge_path(root, "secrets/prod.yaml").unwrap();

        assert!(
            store.contains(&shared),
            "a blob shared with a non-matching entry must be KEPT"
        );
        assert!(
            !store.contains(&exclusive),
            "the exclusive blob is still reclaimed"
        );
    }

    // A prompt blob is not path-scoped, and a small file's snapshot can be
    // byte-identical to a prompt — which makes them the same object. Every
    // prompt_ref is protected regardless of which turn it belongs to.
    #[test]
    fn path_purge_never_archives_a_prompt_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let same = store.put(b"fix the yaml").unwrap();
        let files = format!("[{}]", file_entry("secrets/prod.yaml", None, Some(&same)));
        // The matching entry's `after` IS the other turn's prompt blob.
        let log = format!(
            "{}{}",
            turn_line("t_A", &files, None),
            turn_line("t_B", "[]", Some(&same))
        );
        std::fs::write(log_path(root), log).unwrap();

        purge_path(root, "secrets/prod.yaml").unwrap();

        assert!(
            store.contains(&same),
            "a blob that is also a prompt_ref must be kept"
        );
    }

    // `load_log` silently drops torn lines, so a structured-only read would
    // treat a blob cited ONLY by a torn line as unreferenced-outside-the-match
    // and archive it. Mirrors `purge_orphans`' torn-line test.
    // Neuter: delete the `harvest_refs(line, &mut protect)` fallback -> RED.
    #[test]
    fn path_purge_protects_a_blob_cited_only_by_a_torn_log_line() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let torn_ref = store.put(b"cited by a torn line and by the match").unwrap();
        let clean = store.put(b"cited only by the matching entry").unwrap();
        let files = format!(
            "[{}]",
            file_entry("secrets/prod.yaml", Some(&torn_ref), Some(&clean)),
        );
        let log = format!(
            "{}{{\"v\":1,\"id\":\"t_TORN\",\"files\":[{{\"after\":\"{torn_ref}\"\n",
            turn_line("t_A", &files, None)
        );
        std::fs::write(log_path(root), log).unwrap();

        purge_path(root, "secrets/prod.yaml").unwrap();

        assert!(
            store.contains(&torn_ref),
            "a blob cited by a torn (unparseable) line must be kept"
        );
        assert!(!store.contains(&clean), "the exclusive blob is reclaimed");
    }

    // `open.json` (in-flight crash journal) and `memory.jsonl` (pin hashes)
    // are outside log.jsonl entirely; neither is a matching file entry.
    #[test]
    fn path_purge_protects_open_json_and_memory_pin_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));

        let pending = store.put(b"in-flight open turn content").unwrap();
        let pinned = store.put(b"a memory-pinned version").unwrap();
        let files = format!(
            "[{},{}]",
            file_entry("secrets/prod.yaml", Some(&pending), None),
            file_entry("secrets/other.yaml", None, Some(&pinned))
        );
        std::fs::write(log_path(root), turn_line("t_A", &files, None)).unwrap();
        std::fs::write(
            crate::open_path(root),
            format!("{{\"files\":[{{\"before_hash\":\"{pending}\"}}]}}"),
        )
        .unwrap();
        std::fs::write(
            memory_path(root),
            format!(
                "{{\"v\":1,\"type\":\"memory\",\"id\":\"01AAA\",\"op\":\"assert\",\
                 \"fact\":\"x\",\"pins\":[{{\"path\":\"f.rs\",\"hash\":\"{pinned}\"}}],\
                 \"source_turns\":[],\"origin\":\"human\",\"ts\":1}}\n"
            ),
        )
        .unwrap();

        purge_path(root, "secrets").unwrap();

        assert!(store.contains(&pending), "open.json ref must be kept");
        assert!(store.contains(&pinned), "memory pin ref must be kept");
        assert!(
            objects_archive_dirs(root).is_empty(),
            "nothing archived => no archive dir created"
        );
    }

    // Sibling parity with every other op in this module (F11's ordering
    // property): the step refuses on its own, not only via `run`.
    #[test]
    fn path_purge_refuses_while_the_daemon_holds_the_lock() {
        use std::os::unix::io::AsRawFd;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));
        let blob = store.put(b"leaked credential bytes").unwrap();
        std::fs::write(
            log_path(root),
            turn_line(
                "t_A",
                &format!("[{}]", file_entry("secrets/prod.yaml", None, Some(&blob))),
                None,
            ),
        )
        .unwrap();

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

        // Through the verb...
        let err = run(
            root,
            false,
            None,
            false,
            false,
            false,
            false,
            Some("secrets/prod.yaml"),
        )
        .unwrap_err();
        assert!(err.contains("stop recording"), "verb must refuse: {err}");
        // ...and driven directly.
        let direct = purge_path(root, "secrets/prod.yaml").unwrap_err();
        assert!(
            direct.contains("stop recording"),
            "the step must refuse on its own: {direct}"
        );

        assert!(store.contains(&blob), "a refused purge destroys nothing");
        assert!(
            objects_archive_dirs(root).is_empty(),
            "a refused purge creates no archive dir either"
        );
    }

    // `--path` must NOT drag the unconditional prompt purge along with it:
    // "forget one file" may not also reclaim 90 days of prompt text.
    // Neuter: remove the early `return purge_path(..)` in `run` -> the expired
    // prompt blob leaves the store -> RED.
    #[test]
    fn path_purge_does_not_run_the_default_prompt_purge() {
        let (tmp, store, prompt) = expired_prompt_fixture();
        let root = tmp.path();

        run(
            root,
            false,
            None,
            false,
            false,
            false,
            false,
            Some("no/such/file"),
        )
        .unwrap();

        assert!(
            store.contains(&prompt),
            "purge --path must leave expired prompt blobs alone"
        );
        assert!(
            objects_archive_dirs(root).is_empty(),
            "and must not create an archive dir for them"
        );
    }

    #[test]
    fn path_purge_reports_no_match_without_touching_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let store = BlobStore::new(objects_dir(root));
        let blob = store.put(b"unrelated content").unwrap();
        std::fs::write(
            log_path(root),
            turn_line(
                "t_A",
                &format!("[{}]", file_entry("src/main.rs", None, Some(&blob))),
                None,
            ),
        )
        .unwrap();

        purge_path(root, "secrets/prod.yaml").unwrap();

        assert!(store.contains(&blob), "no match => nothing touched");
        assert!(objects_archive_dirs(root).is_empty());
    }

    #[test]
    fn path_purge_refuses_an_empty_pattern() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(agentrec_dir(root)).unwrap();
        let err = purge_path(root, "   ").unwrap_err();
        assert!(err.contains("non-empty"), "must name the cause: {err}");
    }

    #[test]
    fn path_matches_exact_prefix_and_glob_forms() {
        // exact
        assert!(path_matches("src/config.ts", "src/config.ts"));
        assert!(!path_matches("src/config.ts", "src/config.tsx"));
        // directory prefix, at any depth
        assert!(path_matches("secrets", "secrets/prod.yaml"));
        assert!(path_matches("secrets/", "secrets/prod.yaml"));
        assert!(path_matches("secrets", "secrets/a/b/c.yaml"));
        // a prefix must end on a separator, never mid-name
        assert!(!path_matches("secret", "secrets/prod.yaml"));
        assert!(!path_matches("secrets", "secretsx/prod.yaml"));
        // `*` stays inside one segment
        assert!(path_matches("secrets/*.yaml", "secrets/prod.yaml"));
        assert!(!path_matches("secrets/*.yaml", "secrets/sub/prod.yaml"));
        // `**` crosses separators, and `**/x` also matches at depth zero
        assert!(path_matches("**/*.pem", "certs/inner/server.pem"));
        assert!(path_matches("**/id_rsa", "id_rsa"));
        assert!(path_matches("secrets/**", "secrets/a/b.yaml"));
        // `?` is exactly one non-separator character
        assert!(path_matches("src/config.t?", "src/config.ts"));
        assert!(!path_matches("src/config.t?", "src/config.tsx"));
        assert!(!path_matches("src?config.ts", "src/config.ts"));
    }

    // A glob pattern must never be silently widened into a directory-prefix
    // match — `secrets/*` selects the files directly under `secrets/`, not the
    // whole subtree.
    #[test]
    fn path_matches_does_not_widen_a_glob_into_a_prefix() {
        assert!(path_matches("secrets/*", "secrets/prod.yaml"));
        assert!(!path_matches("secrets/*", "secrets/sub/prod.yaml"));
    }
}
