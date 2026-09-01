//! agentrec CLI: init, record (daemon), log, status, diff, blame, undo, hook
//! (called by agent lifecycle hooks), doctor.

mod approvecmd;
mod attest;
mod cmds;
mod config;
mod daemon;
mod doctorcmd;
mod fmt;
mod hookcmds;
mod importcmd;
mod initcmd;
mod loglock;
mod mcpcmd;
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
use std::path::{Path, PathBuf};

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
        /// Also install Codex's three lifecycle hooks (UserPromptSubmit,
        /// PostToolUse[apply_patch], Stop) into `.codex/hooks.json` or an
        /// existing inline `[hooks]` table in `.codex/config.toml`. Off by
        /// default — unlike Claude Code, Codex integration is opt-in
        /// (IMPLEMENTATION.md 454).
        #[arg(long)]
        codex: bool,
        /// Skip writing/loading the per-repo service unit (launchd/systemd).
        #[arg(long)]
        no_service: bool,
        /// Install the service unit even under a temporary directory, where
        /// it is skipped by default (D46: the unit outlives the directory).
        #[arg(long, conflicts_with = "no_service")]
        service: bool,
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
    /// Store size, recording gaps (all kinds), daemon liveness, and rich-rate health.
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
        /// Emit `serde_json` of the exact `DiffResult` `RepositoryView::diff`
        /// returned, instead of the unified-diff text.
        #[arg(long)]
        json: bool,
    },
    /// Which turn last touched a file or line.
    Blame {
        /// `<file>` or `<file>:<line>`.
        target: String,
        /// Emit `serde_json` of the exact `BlameResult` `RepositoryView::blame`
        /// returned, instead of the prose line.
        #[arg(long)]
        json: bool,
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
    /// Approve a pending agent undo request (confirm mode) and execute it.
    /// With no id, lists the requests still awaiting a decision (D23).
    Approve {
        /// Request id, full or an unambiguous prefix. Omit to list pending
        /// requests instead of approving one.
        id: Option<String>,
    },
    /// Deny a pending agent undo request. Nothing is written to the worktree.
    Deny {
        /// Request id, full or an unambiguous prefix.
        id: String,
    },
    /// Internal: invoked by agent lifecycle hooks; reads the hook payload on stdin.
    #[command(hide = true)]
    Hook {
        /// Tool identity, e.g. "claude" or "codex".
        tool: String,
    },
    /// Internal: the `attest` subsystem (claims derived from tests). Hidden
    /// until Phase 5 — the surface is not stable and `ATTEST-FORMAT.md` is
    /// explicitly outside `PROTOCOL.md` versioning.
    #[command(hide = true)]
    Attest {
        #[command(subcommand)]
        cmd: AttestCmd,
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
        /// Truncate the ALREADY-CONSUMED prefix of the hook inbox
        /// (`signal.jsonl`) — every byte before `state.json`'s
        /// `signal_offset`, whose prompts are already durable in `log.jsonl`
        /// and the object store. Archives (never deletes) that prefix to
        /// `.agentrec/signal.archived.<ts>.jsonl`, preserves the unconsumed
        /// tail byte-identically, and rebases `signal_offset` in the same
        /// operation. Refuses while the daemon is recording (D48).
        #[arg(long = "signals-consumed")]
        signals_consumed: bool,
        /// Forget the recorded content of files matching PATTERN (exact path,
        /// directory prefix, or a `*`/`**`/`?` glob): archives (never deletes)
        /// every snapshot blob referenced ONLY by matching file entries into
        /// `.agentrec/objects.archived.<ts>/`. A blob whose identical bytes are
        /// also referenced from outside the pattern is KEPT and reported as
        /// shared — the store is content-addressed, so those are the same
        /// object. `log.jsonl` is NOT rewritten: the matching paths and hashes
        /// stay in the record, so `diff` then prints `(snapshot unavailable)`
        /// for them and `undo` will not restore them — an entry whose file is
        /// otherwise unmodified is refused with `prior snapshot unavailable —
        /// refusing to restore`, and one that changed since the turn is
        /// excluded as modified-since exactly as it was before.
        /// Prompt blobs are never touched by this flag.
        /// Refuses while the daemon is recording; cannot be combined with the
        /// other purge flags.
        #[arg(
            long = "path",
            value_name = "PATTERN",
            conflicts_with_all = [
                "all_prompts",
                "snapshots_before",
                "memories_retracted",
                "log_duplicates",
                "orphans",
                "signals_consumed",
            ]
        )]
        path: Option<String>,
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
    /// Serve this repository's read tools to an MCP host over stdio
    /// (newline-delimited JSON-RPC 2.0). Long-lived: the host spawns it and
    /// speaks on stdin/stdout. Reads are file-based, so it works with the
    /// recorder daemon stopped. Config is read once at startup — changing
    /// `mcp_destructive` requires a restart.
    Mcp,
    /// Import history from another agent tool's transcript store (Claude
    /// Code or Codex). `--dry-run` classifies and reports without writing;
    /// omitting it persists imported turns into this repo's `log.jsonl`,
    /// scoped to sessions whose `cwd` resolves under `--root`.
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
    /// read-only classification only; omitting it persists imported turns
    /// into `log.jsonl` for sessions in scope of `--root` — idempotent
    /// (re-running appends nothing new) and safe to interrupt.
    Claude {
        /// Read-only classify-and-report mode — no writes. Omit to persist
        /// imported turns into this repo's `log.jsonl`.
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
    /// Classify (and, without `--dry-run`, persist) Codex's
    /// ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl transcript corpus
    /// (K-series, Phase 2 tail C4). `--dry-run` is read-only classification
    /// only; omitting it persists imported turns into `log.jsonl` for
    /// sessions in scope of `--root` — idempotent and safe to interrupt,
    /// same contract as `import claude`.
    Codex {
        /// Read-only classify-and-report mode — no writes. Omit to persist
        /// imported turns into this repo's `log.jsonl`.
        #[arg(long)]
        dry_run: bool,
        /// Corpus root containing a `sessions/` dir (mirrors the real
        /// `~/.codex` layout). Defaults to `~/.codex`.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Emit the machine-readable report instead of the text form.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum AttestCmd {
    /// Read-only summary of `.agentrec/attest.jsonl`: claim counts by status,
    /// the STALE overlay count, and the dev-loop-only (dirty-tree) figure.
    Status {
        /// Emit machine-readable JSON instead of the text report.
        #[arg(long)]
        json: bool,
    },
    /// Discover this crate's tests and append the `derive` events that mint
    /// (or carry forward, across a rename) their claims. Idempotent.
    Derive {
        /// The crate to discover, if not the repo root.
        #[arg(long = "crate", value_name = "PATH")]
        crate_path: Option<PathBuf>,
    },
    /// Run a test command, showing its output unchanged, and record one
    /// `evidence` event per known test. Exits with the command's own status.
    Run {
        /// The command to run, after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        cmd: Vec<String>,
    },
}

/// Walk from `start` up through ancestors looking for a directory containing
/// a `.agentrec/` subdirectory — mirrors git's `.git` discovery. Returns the
/// nearest such ancestor (closest to `start`, including `start` itself), or
/// `None` if the search reaches the filesystem root without finding one.
fn discover_agentrec_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        if dir.join(".agentrec").is_dir() {
            return Some(dir.to_path_buf());
        }
        // `?` on the None case: reaching the filesystem root without a
        // `.agentrec/` is discovery failure, handled by the caller.
        dir = dir.parent()?;
    }
}

/// Root resolution for the `hook` subcommand ONLY — every other verb keeps
/// using the shared `root` computed in `main` below, byte-identically to
/// before this function existed. Scoped this narrowly because O5
/// (`docs/verify/o5-two-tool-session.md`) measured LIVE that Codex (and,
/// latently, Claude) spawn their hook process with cwd = wherever the agent
/// itself was launched from, not necessarily the repo root: the installed
/// hook entry is the bare command `agentrec hook codex`/`agentrec hook
/// claude`, no `--root`, so a launch from a subdirectory silently minted a
/// second `.agentrec/` there — invisible to the daemon watching the true
/// root, with NO error, since `record.rs::open_append` creates parent dirs
/// unconditionally. Silent total data loss.
///
/// Precedence: an explicit `--root` always wins outright — discovery is the
/// DEFAULT for an *absent* root, never an override of one the user supplied.
/// When no explicit root is given, walk up from `cwd` for the nearest
/// ancestor already carrying a `.agentrec/` dir. Chosen specifically over
/// baking an absolute path into the installed hook command string, which is
/// this repo's own documented scar (39 orphaned LaunchAgents from a stale
/// baked `--root`; see CLAUDE.md's "Founder-pending" section) — walking up
/// survives repo moves/renames and handles launches from any subdirectory.
///
/// Discovery failure (no `.agentrec/` anywhere up to the filesystem root)
/// falls back to `cwd`, deliberately NOT an error: INV-M4 pins the hook path
/// as fail-open — it must always append the start/stop signal and exit 0,
/// even against a repo that was never `init`ed at all. Falling back to `cwd`
/// (which is exactly what happened unconditionally before this change) keeps
/// that contract intact for the no-`--root` case; erroring out here would
/// have silently broken `cli/tests/integration.rs::hook_fail_open_and_budget`'s
/// uninitialized-repo leg had it not pinned an explicit `--root` (which
/// bypasses discovery outright and so was never at risk) — the fallback
/// exists so a future no-`--root` variant of that same scenario keeps
/// failing open instead of hard-erroring.
fn resolve_hook_root(explicit: Option<&Path>, cwd: &Path) -> PathBuf {
    if let Some(explicit) = explicit {
        return explicit.to_path_buf();
    }
    discover_agentrec_root(cwd).unwrap_or_else(|| cwd.to_path_buf())
}

/// Root resolution for the `mcp` subcommand. Same discovery RULE as the hook
/// path — an explicit `--root` wins outright, otherwise walk up from cwd via
/// [`discover_agentrec_root`] — deliberately reusing that one function rather
/// than adding a second rule. An MCP host spawns the server with cwd set to
/// whatever it considers the workspace, which O5 measured is not reliably the
/// repo root.
///
/// The FAILURE POSTURE differs from [`resolve_hook_root`], which is why this
/// is its own wrapper rather than a shared one: the hook path is pinned
/// fail-open by INV-M4 and falls back to cwd, but a long-lived read server
/// that silently answers against a non-repo would report an empty history
/// forever. Discovery failure here resolves to cwd only so that
/// `mcpcmd::run`'s startup precondition can name the offending directory in
/// its hard error.
fn resolve_mcp_root(explicit: Option<&Path>, cwd: &Path) -> PathBuf {
    if let Some(explicit) = explicit {
        return explicit.to_path_buf();
    }
    discover_agentrec_root(cwd).unwrap_or_else(|| cwd.to_path_buf())
}

fn main() {
    let cli = Cli::parse();
    let explicit_root = cli.root.clone();
    let root = explicit_root
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .expect("cannot determine working directory");
    let result = match cli.command {
        Command::Init {
            no_hook,
            codex,
            no_service,
            service,
            dry_run,
        } => initcmd::run(&root, no_hook, codex, no_service, service, dry_run),
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
        Command::Diff { turn, json } => readcmds::diff(&root, &turn, json),
        Command::Blame { target, json } => readcmds::blame(&root, &target, json),
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
        Command::Approve { id } => approvecmd::approve(&root, id.as_deref()),
        Command::Deny { id } => approvecmd::deny(&root, &id),
        Command::Hook { tool } => {
            let hook_root = resolve_hook_root(explicit_root.as_deref(), &root);
            cmds::hook(&hook_root, &tool)
        }
        Command::Attest { cmd } => match cmd {
            AttestCmd::Status { json } => attest::statuscmd::run(&root, json),
            AttestCmd::Derive { crate_path } => {
                attest::derivecmd::run(&root, crate_path.as_deref())
            }
            // The wrapped command's own exit status is the user-visible one —
            // `attest run` must be transparent to a script downstream.
            AttestCmd::Run { cmd } => match attest::capture::run_wrapped(&root, &cmd) {
                Ok(0) => Ok(()),
                Ok(code) => std::process::exit(code),
                Err(e) => Err(e),
            },
        },
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
            signals_consumed,
            path,
        } => purgecmd::run(
            &root,
            all_prompts,
            snapshots_before.as_deref(),
            memories_retracted,
            log_duplicates,
            orphans,
            signals_consumed,
            path.as_deref(),
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
        Command::Mcp => {
            let mcp_root = resolve_mcp_root(explicit_root.as_deref(), &root);
            mcpcmd::run(&mcp_root)
        }
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

#[cfg(test)]
mod hook_root_discovery_tests {
    use super::{discover_agentrec_root, resolve_hook_root};

    /// Regression fixture: an `.agentrec/` at `tmp`, with no `--root` given
    /// and a cwd several levels below it.
    fn init_root() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".agentrec")).unwrap();
        tmp
    }

    #[test]
    fn discovers_root_from_a_subdirectory_several_levels_deep() {
        let tmp = init_root();
        let sub = tmp.path().join("a/b/c/d");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(discover_agentrec_root(&sub), Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn discovers_root_when_invoked_from_the_root_itself_no_regression() {
        let tmp = init_root();
        assert_eq!(
            discover_agentrec_root(tmp.path()),
            Some(tmp.path().to_path_buf())
        );
    }

    #[test]
    fn discovery_failure_returns_none_when_no_agentrec_exists_up_the_tree() {
        // A fresh tempdir, never `init`ed, with no `.agentrec/` at or above
        // it (system temp roots never carry one).
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(discover_agentrec_root(tmp.path()), None);
    }

    #[test]
    fn resolve_falls_back_to_cwd_on_discovery_failure_fail_open_inv_m4() {
        // INV-M4: the hook path must always append and exit 0, even against
        // a repo that was never `init`ed. No-`--root` + no `.agentrec/`
        // anywhere must resolve to cwd (which is exactly what happened
        // unconditionally before this change), not fail.
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_hook_root(None, tmp.path()),
            tmp.path().to_path_buf()
        );
    }

    #[test]
    fn explicit_root_wins_over_discovery() {
        // cwd itself carries a `.agentrec/` (discovery would find cwd), but
        // an explicit root pointing elsewhere must win outright.
        let cwd_tmp = init_root();
        let explicit_tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_hook_root(Some(explicit_tmp.path()), cwd_tmp.path()),
            explicit_tmp.path().to_path_buf()
        );
    }

    #[test]
    fn explicit_root_wins_even_when_it_has_no_agentrec_dir() {
        // An explicit --root is trusted outright; it is never validated
        // against discovery, matching every other subcommand's existing
        // (unchanged) contract.
        let explicit_tmp = tempfile::tempdir().unwrap();
        let cwd_tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_hook_root(Some(explicit_tmp.path()), cwd_tmp.path()),
            explicit_tmp.path().to_path_buf()
        );
    }
}
