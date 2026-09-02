//! `.agentrec/config.toml` loader (D16, delta decision 13). Backed by the
//! `toml` crate — replaces the previous hand-rolled `key = value` line
//! scanner used across `cmds.rs`, `purgecmd.rs`, `noise.rs`, and
//! `memorycmds.rs`. `McpDestructive` is THE mode type: every later phase
//! that needs the destructive mode consumes this enum — no task defines a
//! second one.
//!
//! Two distinct failure modes, deliberately not conflated:
//! - **File-level**: `.agentrec/config.toml` exists but isn't valid TOML.
//!   [`load`] returns `Err` naming the line/column (D16's "hard error").
//! - **Value-level**: the file parses, but one key's value is the wrong
//!   shape (`store_budget_bytes = "lots"`, a stray decimal, etc). Every key
//!   except `mcp_destructive` stays tolerant here — falls back to that
//!   key's documented default — matching the original scanner's per-key
//!   behavior exactly, so pinned fallback tests survive unamended.
//!   `mcp_destructive` is the one exception (D16 explicitly requires a
//!   named-values validation error), and is a value-level `Err` too.

use std::path::Path;
use std::sync::Once;

/// Agent-driven-undo gating mode (PROTOCOL.md §8). Default `Off`.
///
/// **The definition MOVED to `agentrec_core::undo_coordinator` (task F2) and
/// is re-exported here.** `UndoCoordinator::preview` takes the mode as a
/// parameter and core cannot depend on the CLI crate, so the enum had to
/// live in core — and the F2 contract forbids a second mode type, which a
/// core-side behaviour enum plus a CLI mapping function would have been.
/// Re-exporting keeps ONE type under ONE name: every `config::McpDestructive`
/// path in this crate resolves unchanged, and there is no mapping to drift.
/// Parsing stays here — core never learns this key's name, its spelling, or
/// its default, exactly as `view::health` never learns the budget key's.
pub use agentrec_core::undo_coordinator::McpDestructive;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub ttl_days: u64,
    pub store_budget_bytes: u64,
    pub noise_globs: Vec<String>,
    pub memory_enabled: bool,
    pub memory_inject_max: usize,
    pub mcp_destructive: McpDestructive,
    /// Phase 4B: whether the `record` daemon appends `stale` events to
    /// `.agentrec/attest.jsonl` when a write lands in a claim's coverage
    /// scope. Default on — staleness is the attest subsystem's whole
    /// freshness signal, and a repo with no coverage map never reaches the
    /// append path anyway (the daemon does nothing when the map is absent).
    pub attest_stale: bool,
    /// Phase 4B: override for the coverage map's location. `None` means the
    /// default, `.agentrec/attest-coverage.json`.
    pub attest_coverage_path: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            // Sourced from the two pre-existing named constants (rather than
            // repeating the magic numbers 90 / 5 a third place) so
            // `purgecmd::DEFAULT_TTL_DAYS` and
            // `memorycmds::HOOK_MAX_FACTS_DEFAULT` stay the single source of
            // truth their own doc comments already claim to be.
            ttl_days: crate::purgecmd::DEFAULT_TTL_DAYS,
            store_budget_bytes: agentrec_core::MAX_STORE_BYTES,
            noise_globs: Vec::new(),
            memory_enabled: true,
            memory_inject_max: crate::memorycmds::HOOK_MAX_FACTS_DEFAULT,
            mcp_destructive: McpDestructive::Off,
            attest_stale: true,
            attest_coverage_path: None,
        }
    }
}

/// Carries a human-readable message; file-level errors include the line/col
/// `toml`'s own parser reports, value-level `mcp_destructive` errors name
/// the three valid values.
#[derive(Debug)]
pub struct ConfigError(String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ConfigError {}

const KNOWN_KEYS: &[&str] = &[
    "ttl_days",
    "store_budget_bytes",
    "noise_globs",
    "memory_enabled",
    "memory_inject_max",
    "mcp_destructive",
    "attest_stale",
    "attest_coverage_path",
];

/// Guards the unknown-key stderr warning to once per process — `load` is
/// called fresh (full re-read + re-parse) from every reader shim below, and
/// at least one of those (`memorycmds::read_memory_enabled`) is read once
/// per 250ms daemon poll tick. Without this, a config.toml with one unknown
/// key would spam `daemon.log` forever.
static UNKNOWN_KEY_WARNED: Once = Once::new();

/// Read and validate `.agentrec/config.toml`. A missing file yields
/// [`Config::default`]. An unparseable file is an `Err` naming the line/col
/// (D16); this is the ONLY hard-error path — value-level problems degrade
/// per-key (see module docs). Unknown top-level keys warn once to stderr
/// and are otherwise ignored — never an error.
pub fn load(root: &Path) -> Result<Config, ConfigError> {
    let path = crate::agentrec_dir(root).join("config.toml");
    // fsguard: `config.toml` is inside `.agentrec/`.
    let text = match agentrec_core::fsguard::read_regular_to_string(&path) {
        Ok(t) => t,
        // Missing file (or unreadable for any other reason, e.g.
        // permissions) — same tolerant posture the hand-rolled scanner had:
        // no config is not an error, it's the default configuration.
        Err(_) => return Ok(Config::default()),
    };
    let raw: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| ConfigError(e.to_string()))?;

    let unknown: Vec<&str> = raw
        .keys()
        .map(String::as_str)
        .filter(|k| !KNOWN_KEYS.contains(k))
        .collect();
    if !unknown.is_empty() {
        UNKNOWN_KEY_WARNED.call_once(|| {
            eprintln!(
                "agentrec: warning: unknown .agentrec/config.toml key(s): {} (ignored)",
                unknown.join(", ")
            );
        });
    }

    let mut cfg = Config::default();

    if let Some(v) = raw.get("ttl_days") {
        if let Some(n) = v.as_integer() {
            if n >= 0 {
                cfg.ttl_days = n as u64;
            }
        }
    }
    if let Some(v) = raw.get("store_budget_bytes") {
        if let Some(n) = v.as_integer() {
            // A zero budget makes every evictable snapshot a candidate on
            // the daemon's next tick — refuse it, keep the default, same
            // protective posture as the original `effective_store_budget`.
            if n > 0 {
                cfg.store_budget_bytes = n as u64;
            }
        }
    }
    if let Some(v) = raw.get("noise_globs") {
        if let Some(arr) = v.as_array() {
            cfg.noise_globs = arr
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
        }
    }
    if let Some(v) = raw.get("memory_enabled") {
        if let Some(b) = v.as_bool() {
            cfg.memory_enabled = b;
        }
    }
    if let Some(v) = raw.get("memory_inject_max") {
        if let Some(n) = v.as_integer() {
            if n >= 0 {
                cfg.memory_inject_max = n as usize;
            }
        }
    }
    if let Some(v) = raw.get("mcp_destructive") {
        let s = v.as_str().unwrap_or("");
        cfg.mcp_destructive = match s {
            "off" => McpDestructive::Off,
            "confirm" => McpDestructive::Confirm,
            "auto" => McpDestructive::Auto,
            other => {
                return Err(ConfigError(format!(
                    "invalid mcp_destructive value {other:?}: must be one of \"off\", \"confirm\", \"auto\""
                )));
            }
        };
    }

    // Value-level tolerant, like every key except `mcp_destructive`: a
    // wrong-shaped value falls back to this key's default rather than
    // failing the whole load. D16's named-values hard error is deliberately
    // NOT extended to a second key here.
    if let Some(v) = raw.get("attest_stale") {
        if let Some(b) = v.as_bool() {
            cfg.attest_stale = b;
        }
    }
    if let Some(v) = raw.get("attest_coverage_path") {
        if let Some(s) = v.as_str() {
            cfg.attest_coverage_path = Some(s.to_string());
        }
    }

    Ok(cfg)
}

/// Guards the file-level parse-error stderr warning to once per process —
/// same rationale as [`UNKNOWN_KEY_WARNED`].
static LOAD_ERROR_WARNED: Once = Once::new();

/// The tolerant shim for the two consumer classes that must never hard-fail
/// on a bad `config.toml`, because both are invoked automatically and
/// frequently rather than as a deliberate one-shot user command: the
/// `record` daemon's PER-TICK reads (`cmds::run_eviction_pass`'s budget
/// read, `daemon.rs`'s live-loop and startup-replay `memory_enabled` reads)
/// and the `hook` subcommand's memory-injection gate (Claude Code invokes it
/// on every prompt; `cmds::hook`'s own doc comment documents its fail-open
/// contract, and hard-failing there would exit nonzero *after* the signal
/// append it precedes already landed — noise with no protective value).
///
/// CLI verbs (`status`, `purge`, `log`, `show`) call [`load`] directly and
/// propagate its `Err` instead (gate finding, D16 remediation) — a file that
/// fails to parse as TOML at all is a hard, user-visible, nonzero-exit error
/// naming the line/col for those paths now. The daemon additionally hard-
/// fails once, at startup (`daemon::run`, before `recover_orphan`), via its
/// own direct `load` call — "daemon startup included" (D16) is satisfied
/// there, not by this function. Only a config edited to garbage AFTER a
/// clean boot reaches this tolerant path in the daemon, where it degrades to
/// [`Config::default`] for every key rather than crash-looping the process
/// under launchd `KeepAlive` (see CLAUDE.md's "40 orphaned LaunchAgents"
/// history) — and degrading is the safe direction specifically for
/// `store_budget_bytes`: the default is [`agentrec_core::MAX_STORE_BYTES`],
/// the LARGEST value in play, so a mid-tick parse failure means LESS
/// eviction, never a silent history wipe.
///
/// This still means a single syntax error anywhere in the file reverts keys
/// that parsed fine too — e.g. a stray `mcp_destructive = "yes"` line
/// silently reverts a correct, adjacent `ttl_days = 30` back to 90 — but
/// only for the two consumer classes above; every CLI verb that reads a
/// config key now surfaces that same file exactly once, loudly.
pub(crate) fn load_or_default(root: &Path) -> Config {
    match load(root) {
        Ok(cfg) => cfg,
        Err(e) => {
            LOAD_ERROR_WARNED.call_once(|| {
                eprintln!(
                    "agentrec: warning: .agentrec/config.toml failed to parse ({e}); using defaults for all keys until this is fixed"
                );
            });
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `content` as `.agentrec/config.toml` under a fresh tempdir and
    /// returns the root path. Leaks the tempdir (via `keep`) rather than
    /// deleting it on drop — the returned `PathBuf` must outlive the borrow
    /// `config::load(&root_with(...))` takes of it inline.
    fn root_with(content: &str) -> std::path::PathBuf {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.keep();
        std::fs::create_dir_all(root.join(".agentrec")).unwrap();
        std::fs::write(root.join(".agentrec/config.toml"), content).unwrap();
        root
    }

    #[test]
    fn missing_file_is_all_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(tmp.path()).unwrap(), Config::default());
    }

    #[test]
    fn invalid_toml_is_a_hard_error_naming_a_line() {
        let e = load(&root_with("ttl_days = [unclosed")).unwrap_err();
        assert!(
            e.to_string().contains("line"),
            "error must name a line, got: {e}"
        );
    }

    #[test]
    fn mcp_destructive_validation() {
        assert!(matches!(
            load(&root_with(r#"mcp_destructive = "confirm""#))
                .unwrap()
                .mcp_destructive,
            McpDestructive::Confirm
        ));
        assert!(matches!(
            load(&root_with(r#"mcp_destructive = "auto""#))
                .unwrap()
                .mcp_destructive,
            McpDestructive::Auto
        ));
        assert!(matches!(
            load(&root_with(r#"mcp_destructive = "off""#))
                .unwrap()
                .mcp_destructive,
            McpDestructive::Off
        ));

        let e = load(&root_with(r#"mcp_destructive = "yes""#)).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("off") && msg.contains("confirm") && msg.contains("auto"));
    }

    #[test]
    fn unknown_key_warns_but_does_not_error() {
        // Assert the "does not error" half directly; the "warns (stderr)"
        // half is implemented via `eprintln!` above and is not
        // stderr-captured here (no reliable in-process capture without
        // races against the other tests in this binary sharing the process
        // stderr handle, and this repo doesn't fabricate coverage claims it
        // can't back).
        let cfg = load(&root_with("totally_unknown_key = 1\nttl_days = 30\n")).unwrap();
        assert_eq!(cfg.ttl_days, 30);
    }

    #[test]
    fn per_key_defaults_are_all_documented() {
        let cfg = Config::default();
        assert_eq!(cfg.ttl_days, 90);
        assert_eq!(cfg.store_budget_bytes, agentrec_core::MAX_STORE_BYTES);
        assert_eq!(cfg.noise_globs, Vec::<String>::new());
        assert!(cfg.memory_enabled);
        assert_eq!(cfg.memory_inject_max, 5);
        assert_eq!(cfg.mcp_destructive, McpDestructive::Off);
        assert!(cfg.attest_stale);
        assert_eq!(cfg.attest_coverage_path, None);
    }

    /// AC-ATTEST-P4B-6 (loader half): both new keys read, and both degrade
    /// per-key on a wrong-shaped value instead of failing the load.
    #[test]
    fn attest_keys_are_read_and_degrade_per_key() {
        let cfg = load(&root_with(
            "attest_stale = false\nattest_coverage_path = \"cov.json\"\n",
        ))
        .unwrap();
        assert!(!cfg.attest_stale);
        assert_eq!(cfg.attest_coverage_path, Some("cov.json".to_string()));

        // Wrong shapes: valid TOML, so the file loads; each key falls back.
        let cfg = load(&root_with("attest_stale = 1\nattest_coverage_path = 7\n")).unwrap();
        assert!(cfg.attest_stale);
        assert_eq!(cfg.attest_coverage_path, None);
    }

    #[test]
    fn value_level_bad_values_default_per_key_not_hard_error() {
        // A wrong-shape value for a non-mcp_destructive key is valid TOML,
        // so the file parses; that key alone falls back to its default,
        // not an `Err` for the whole load.
        let cfg = load(&root_with("store_budget_bytes = \"lots\"\n")).unwrap();
        assert_eq!(cfg.store_budget_bytes, agentrec_core::MAX_STORE_BYTES);

        let cfg = load(&root_with("store_budget_bytes = 0\n")).unwrap();
        assert_eq!(
            cfg.store_budget_bytes,
            agentrec_core::MAX_STORE_BYTES,
            "a zero budget must be refused, not honored"
        );
    }

    #[test]
    fn noise_globs_non_string_elements_are_dropped_not_fatal() {
        // Replacement coverage for the deleted `parse_glob_array_malformed_
        // degrades_to_none` (B2 amendment): a non-string array element must
        // not fail the whole array, let alone the whole file — it's simply
        // filtered out, per-element, same posture as every other value-level
        // degrade in this module.
        let cfg = load(&root_with(r#"noise_globs = ["ok", 42]"#)).unwrap();
        assert_eq!(cfg.noise_globs, vec!["ok".to_string()]);
    }

    #[test]
    fn noise_globs_wrong_shape_value_degrades_to_empty_not_fatal() {
        // A `noise_globs` that isn't an array at all (wrong TOML type, not a
        // syntax error) is still valid TOML — `load` must return `Ok` with
        // the per-key default (empty), not an `Err` for the whole file.
        let cfg = load(&root_with(r#"noise_globs = "nope""#)).unwrap();
        assert_eq!(cfg.noise_globs, Vec::<String>::new());
    }

    #[test]
    fn load_or_default_defaults_every_key_on_file_level_parse_error() {
        // Even the ttl_days on the SAME line-adjacent well-formed key is
        // reverted — load_or_default's documented all-or-nothing behavior.
        let cfg = load_or_default(&root_with("ttl_days = 30\nstore_budget_bytes = [unclosed"));
        assert_eq!(cfg, Config::default());
    }
}
