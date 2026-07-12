//! `agentrec purge` (AC I5–I6): default deletes prompt-blob objects for turns
//! older than `ttl_days` (config.toml, default 90). `--all-prompts` deletes
//! every prompt blob regardless of age; `--snapshots-before <DATE>` deletes
//! snapshot blobs (`before`/`after`) for turns started before that date. Both
//! keep a dedup keep-set so a blob still referenced by a kept turn — content
//! is shared across turns — is never deleted. `log`/`blame` are unaffected
//! (they render `prompt_excerpt`, never the blob); `diff`/`undo` are
//! unaffected by the default (prompt-only) purge, since snapshot blobs are a
//! disjoint set only touched by `--snapshots-before`.

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
/// rewrite that follows. A single env var read (no-op) when unset — no
/// effect on production behavior.
fn test_pause_before_memory_rewrite() {
    if let Ok(ms) = std::env::var("AGENTREC_TEST_PAUSE_BEFORE_MEMORY_REWRITE_MS") {
        if let Ok(ms) = ms.parse::<u64>() {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
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
}
