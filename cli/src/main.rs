//! agentrec CLI: init, record (daemon), log, status, diff, blame, undo, hook
//! (called by agent lifecycle hooks), doctor.

mod cmds;
mod daemon;
mod doctorcmd;
mod fmt;
mod initcmd;
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
    },
    /// Store size, recording gaps, and rich-rate health.
    Status {
        /// Acknowledge and clear a DEGRADED snapshot-failure banner.
        #[arg(long)]
        ack_degraded: bool,
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
        } => cmds::log(&root, all, json, limit, utc, explain),
        Command::Status { ack_degraded } => cmds::status(&root, ack_degraded),
        Command::Diff { turn } => readcmds::diff(&root, &turn),
        Command::Blame { target } => readcmds::blame(&root, &target),
        Command::Show { turn, prompt } => readcmds::show(&root, &turn, prompt),
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
        } => purgecmd::run(&root, all_prompts, snapshots_before.as_deref()),
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
