//! agentrec CLI: init, record (daemon), log, status, diff, blame, undo, hook
//! (called by agent lifecycle hooks), doctor.

mod cmds;
mod daemon;
mod doctorcmd;
mod fmt;
mod importcmd;
mod initcmd;
mod loglock;
mod memlock;
mod memorycmds;
mod noise;
mod purgecmd;
mod readcmds;
mod service;
mod state;
mod uninstallcmd;

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "agentrec",
    version,
    about = "Flight recorder for coding agents"
)]
struct Cli {
    /// Repository root (defaults to current directory).
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize .agentrec/, install agent hooks, and install+load a
    /// per-repo recorder service for this repo.
    Init {
        /// Skip agent hook installation (bare-turn capture only).
        #[arg(long)]
        no_hook: bool,
        /// Skip writing/loading the per-repo service unit (launchd/systemd).
        #[arg(long)]
        no_service: bool,
        /// Print the actions init would take without touching disk.
        #[arg(long)]
        dry_run: bool,
    },
    /// Run the recorder daemon for this repo (foreground).
    Record,
    /// List recorded turns, newest first (git turns hidden by default).
    Log {
        /// Include git-operation turns.
        #[arg(long)]
        all: bool,
        /// Emit protocol JSONL instead of the table.
        #[arg(long)]
        json: bool,
        /// Maximum turns to print.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Render absolute UTC timestamps instead of the relative default.
        #[arg(long)]
        utc: bool,
        /// Append a glossary of domain terms present in this output.
        #[arg(long)]
        explain: bool,
        /// Show file entries folded by `noise_globs` (config.toml) instead
        /// of collapsing them into a count line. Orthogonal to --all (which
        /// controls turn visibility, not file-entry visibility).
        #[arg(long = "all-files")]
        all_files: bool,
    },
    /// Store size, recording gaps, and rich-rate health.
    Status {
        /// Acknowledge and clear a DEGRADED snapshot-failure banner. Rejected
        /// together with `--json` (Phase 3, honesty-fixes round): the ack
        /// path prints prose ("acknowledged — DEGRADED cleared") on success,
        /// and prose on stdout under a `--json` flag would break any
        /// consumer piping to `jq`.
        #[arg(long, conflicts_with = "json")]
        ack_degraded: bool,
        /// Emit machine-readable operational fields (state.json data — NOT
        /// the PROTOCOL wire format) instead of the text report.
        #[arg(long)]
        json: bool,
    },
    /// Unified diff of a turn's changes.
    Diff {
        /// Turn id, full or an unambiguous prefix.
        turn: String,
    },
    /// Which turn last touched a file or line.
    Blame {
        /// `<file>` or `<file>:<line>`.
        target: String,
    },
    /// Print a turn's header; the full prompt requires the explicit --prompt flag.
    Show {
        /// Turn id, full or an unambiguous prefix.
        turn: String,
        /// Print the full post-scrub prompt text instead of the header.
        #[arg(long)]
        prompt: bool,
        /// Show file entries folded by `noise_globs` (config.toml) instead
        /// of collapsing them into a count line.
        #[arg(long = "all-files")]
        all_files: bool,
    },
    /// Revert a turn's changes, per file.
    Undo {
        /// Turn id, full or an unambiguous prefix. Omit for panic mode: the
        /// most recent non-git turn (must be rich).
        turn: Option<String>,
        /// Apply the revert. Without this flag, undo only previews.
        #[arg(long)]
        confirm: bool,
        /// Include files that were modified since the turn (excluded by default).
        #[arg(long = "allow-modified")]
        allow_modified: bool,
        /// Restrict the revert to these paths (comma-separated); others in
        /// the turn are left untouched.
        #[arg(long, value_delimiter = ',')]
        files: Vec<String>,
    },
    /// Internal: invoked by agent lifecycle hooks; reads the hook payload on stdin.
    #[command(hide = true)]
    Hook {
        /// Tool identity, e.g. "claude".
        tool: String,
    },
    /// Remove agentrec from this repo: hooks, service unit, and archive
    /// .agentrec/ to a sibling directory. Nothing is ever deleted.
    Uninstall {
        /// Skip unloading/removing the per-repo service unit.
        #[arg(long)]
        no_service: bool,
    },
    /// One-shot diagnosis of the whole recording chain: daemon liveness,
    /// hook presence, signal freshness, DEGRADED store, permissions, and
    /// (Linux) inotify headroom. A setup command, not a query verb.
    Doctor {
        /// Emit machine-readable JSON instead of the text report.
        #[arg(long)]
        json: bool,
    },
    /// Delete blob objects: expired prompts by default (TTL from config.toml).
    Purge {
        /// Delete ALL prompt blobs, regardless of age.
        #[arg(long = "all-prompts")]
        all_prompts: bool,
        /// Delete snapshot blobs for turns started before this date (YYYY-MM-DD).
        #[arg(long = "snapshots-before", value_name = "DATE")]
        snapshots_before: Option<String>,
        /// Archive (never delete) fully-retracted memory chains older than
        /// ttl_days into `.agentrec/memory.archived.<ts>.jsonl`. Rewrites
        /// memory.jsonl in place — refuses while the daemon is recording;
        /// concurrent remember/verify/forget are lock-serialized (they wait,
        /// never lost).
        #[arg(long = "memories-retracted")]
        memories_retracted: bool,
        /// Repair a log.jsonl that a pre-fix daemon (PR #2's kill-9 window)
        /// wrote a same-id duplicate turn into: archives the whole file
        /// (never delete) to `.agentrec/log.archived.<ts>.jsonl`, then
        /// rewrites log.jsonl dropping only lines that are exact
        /// `same_revert` duplicates of an earlier same-id record — a
        /// genuinely ambiguous same-id pair (different files) is left
        /// untouched. Refuses while the daemon is recording.
        #[arg(long = "log-duplicates")]
        log_duplicates: bool,
        /// Archive (never delete) orphaned CAS blobs — content snapshotted for
        /// crash recovery that no committed turn or in-flight open turn
        /// references (superseded intermediate states). Renames them into
        /// `.agentrec/objects.archived.<ts>/`. Refuses while the daemon is
        /// recording. This is the only path that reclaims that space.
        #[arg(long = "orphans")]
        orphans: bool,
    },
    /// Record a manual, human-authored pinned memory.
    Remember {
        /// The fact to remember (scrubbed of secrets before storage).
        fact: String,
        /// Comma-separated repo-relative paths this fact is pinned to.
        #[arg(long)]
        from: String,
    },
    /// Rank-then-verify search over pinned memories; only Fresh matches.
    Recall {
        /// Free-text query.
        query: String,
        /// Max results.
        #[arg(short, default_value = "5")]
        k: usize,
        /// Emit the full effective records (+ freshness) as JSON.
        #[arg(long)]
        json: bool,
        /// Internal: emit the fenced hook-injection block (or nothing).
        #[arg(long, hide = true)]
        for_hook: bool,
    },
    /// List recorded memories (audit view, not a query).
    Memories {
        /// Show only non-fresh (stale/orphaned) memories.
        #[arg(long)]
        stale: bool,
        /// Include retracted memories too.
        #[arg(long)]
        all: bool,
        /// Emit the full effective records (+ freshness) as JSON.
        #[arg(long)]
        json: bool,
        /// Summarize per-hook recall latency (elapsed_ms) from
        /// memory-stats.jsonl instead of listing memories. Reads that file
        /// directly — bypasses memory.jsonl entirely, so a corrupt store
        /// never blocks this readout. Rejected together with
        /// `--stale`/`--all`/`--json` (finding 4, review round; same
        /// precedent as `status --ack-degraded --json`): those flags shape
        /// the memory-listing branch this one bypasses entirely, and
        /// silently ignoring them let a monitoring script ask for
        /// `--stats --json` and get human prose instead of an error.
        #[arg(long, conflicts_with_all = ["stale", "all", "json"])]
        stats: bool,
    },
    /// Emit an agent-authored memory candidate (write path): appends a
    /// memory-candidate signal for the daemon to validate, hash, and
    /// ingest. See the companion `agentrec-memory` skill for usage guidance.
    Candidate {
        /// The candidate fact (scrubbed of secrets before it reaches disk).
        fact: String,
        /// Comma-separated repo-relative paths this fact is grounded in.
        #[arg(long)]
        from: String,
        /// Emitting tool identity.
        #[arg(long, default_value = "agent")]
        tool: String,
    },
    /// Re-pin a drifted memory. Without --confirm, previews the per-pin
    /// drift and changes nothing. Re-pinning a renamed/moved file to its
    /// successor is explicit only (--replace-pin) — there is no automatic
    /// rename detection.
    Verify {
        /// Memory id, full or an unambiguous prefix.
        id: String,
        /// Apply the reverify. Without this flag, verify only previews.
        #[arg(long)]
        confirm: bool,
        /// Explicitly drop an orphaned (deleted) pin; repeatable. Required
        /// for every orphaned pin (unless it's named by --replace-pin
        /// instead), or --confirm refuses.
        #[arg(long = "drop-pin")]
        drop_pin: Vec<String>,
        /// Re-point an existing pin (`old`, already on this memory) to a
        /// validated successor path (`new`) — repeatable as
        /// `--replace-pin old=new`. `new` must pass the same validation as
        /// `remember --from`: in-root, exists, not a secret path. No
        /// automatic rename/successor discovery — the mapping is always
        /// explicit.
        #[arg(long = "replace-pin", value_name = "OLD=NEW")]
        replace_pin: Vec<String>,
    },
    /// Retract a memory — quarantine is recoverable, but forgetting is
    /// explicit and reasoned.
    Forget {
        /// Memory id, full or an unambiguous prefix.
        id: String,
        /// Why this memory is being retracted (scrubbed of secrets).
        #[arg(long)]
        reason: Option<String>,
    },
    /// Import history from another agent tool's transcript store (Claude
    /// Code only). `--dry-run` classifies and reports without writing
    /// (P1); omitting it persists imported turns into this repo's
    /// `log.jsonl`, scoped to sessions whose `cwd` resolves under `--root`
    /// (P2).
    Import {
        #[command(subcommand)]
        source: ImportSource,
    },
}

#[derive(Subcommand)]
enum ImportSource {
    /// Classify (and, without `--dry-run`, persist) Claude Code's
    /// ~/.claude/projects transcript corpus: per-tier before-bytes
    /// reconstructability, fidelity figures, peak RSS. `--dry-run` is
    /// read-only classification only (P1); omitting it persists imported
    /// turns into `log.jsonl` for sessions in scope of `--root` (P2) —
    /// idempotent (re-running appends nothing new) and safe to interrupt.
    Claude {
        /// Read-only classify-and-report mode (P1) — no writes. Omit to
        /// persist (P2).
        #[arg(long)]
        dry_run: bool,
        /// Corpus root containing sibling `projects/` and `file-history/`
        /// dirs (mirrors the real `~/.claude` layout). Defaults to `~/.claude`.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Emit the machine-readable report instead of the text form.
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let root = cli
        .root
        .or_else(|| std::env::current_dir().ok())
        .expect("cannot determine working directory");
    let result = match cli.command {
        Command::Init {
            no_hook,
            no_service,
            dry_run,
        } => initcmd::run(&root, no_hook, no_service, dry_run),
        Command::Record => daemon::run(&root),
        Command::Log {
            all,
            json,
            limit,
            utc,
            explain,
            all_files,
        } => cmds::log(&root, all, json, limit, utc, explain, all_files),
        Command::Status { ack_degraded, json } => cmds::status(&root, ack_degraded, json),
        Command::Diff { turn } => readcmds::diff(&root, &turn),
        Command::Blame { target } => readcmds::blame(&root, &target),
        Command::Show {
            turn,
            prompt,
            all_files,
        } => readcmds::show(&root, &turn, prompt, all_files),
        Command::Undo {
            turn,
            confirm,
            allow_modified,
            files,
        } => readcmds::undo(&root, turn.as_deref(), confirm, allow_modified, &files),
        Command::Hook { tool } => cmds::hook(&root, &tool),
        Command::Doctor { json } => match doctorcmd::run(&root, json) {
            // Checks ran and printed their own report; a failing check is not
            // a command error (no "agentrec: <message>" line) — just a
            // nonzero exit for CI to key on.
            Ok(true) => Ok(()),
            Ok(false) => std::process::exit(1),
            Err(e) => Err(e),
        },
        Command::Uninstall { no_service } => uninstallcmd::run(&root, no_service),
        Command::Purge {
            all_prompts,
            snapshots_before,
            memories_retracted,
            log_duplicates,
            orphans,
        } => purgecmd::run(
            &root,
            all_prompts,
            snapshots_before.as_deref(),
            memories_retracted,
            log_duplicates,
            orphans,
        ),
        Command::Remember { fact, from } => memorycmds::remember(&root, &fact, &from),
        Command::Recall {
            query,
            k,
            json,
            for_hook,
        } => memorycmds::recall_cmd(&root, &query, k, json, for_hook),
        Command::Memories {
            stale,
            all,
            json,
            stats,
        } => {
            if stats {
                memorycmds::memories_stats(&root)
            } else {
                memorycmds::memories(&root, stale, all, json)
            }
        }
        Command::Candidate { fact, from, tool } => {
            memorycmds::candidate(&root, &fact, &from, &tool)
        }
        Command::Verify {
            id,
            confirm,
            drop_pin,
            replace_pin,
        } => memorycmds::verify(&root, &id, confirm, &drop_pin, &replace_pin),
        Command::Forget { id, reason } => memorycmds::forget(&root, &id, reason.as_deref()),
        Command::Import { source } => importcmd::run(&root, source),
    };
    if let Err(message) = result {
        eprintln!("agentrec: {message}");
        std::process::exit(1);
    }
}

/// Shared path helpers.
pub fn agentrec_dir(root: &std::path::Path) -> PathBuf {
    root.join(".agentrec")
}
pub fn log_path(root: &std::path::Path) -> PathBuf {
    agentrec_dir(root).join("log.jsonl")
}
pub fn signal_path(root: &std::path::Path) -> PathBuf {
    agentrec_dir(root).join("signal.jsonl")
}
pub fn objects_dir(root: &std::path::Path) -> PathBuf {
    agentrec_dir(root).join("objects")
}
pub fn state_path(root: &std::path::Path) -> PathBuf {
    agentrec_dir(root).join("state.json")
}
/// Crash journal for the in-flight open turn (AC B2); absent when idle.
pub fn open_path(root: &std::path::Path) -> PathBuf {
    agentrec_dir(root).join("open.json")
}
/// Task 9: hook-owned, append-only injection log — the daemon never reads or
/// writes this file (`state.json` stays daemon-exclusive; see `cmds::hook`).
pub fn memory_stats_path(root: &std::path::Path) -> PathBuf {
    agentrec_dir(root).join("memory-stats.jsonl")
}
/// Coordination file for an in-progress `undo --confirm` (AC H7): while it
/// exists (and hasn't expired), the daemon must not mint a turn for the
/// listed paths — those writes are undo's own, not agent/human activity.
pub fn undo_guard_path(root: &std::path::Path) -> PathBuf {
    agentrec_dir(root).join("undo-guard.json")
}

/// Repo-relative paths currently being rewritten by `undo`, plus a wall-clock
/// deadline bounding a crash-orphaned guard (best-effort cleanup otherwise).
#[derive(Serialize, Deserialize)]
pub struct UndoGuard {
    pub paths: Vec<String>,
    pub until_ms: u64,
}
