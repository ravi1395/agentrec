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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpDestructive {
    #[default]
    Off,
    Confirm,
    Auto,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub ttl_days: u64,
    pub store_budget_bytes: u64,
    pub noise_globs: Vec<String>,
    pub memory_enabled: bool,
    pub memory_inject_max: usize,
    pub mcp_destructive: McpDestructive,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ttl_days: 90,
            store_budget_bytes: agentrec_core::MAX_STORE_BYTES,
            noise_globs: Vec::new(),
            memory_enabled: true,
            memory_inject_max: 5,
            mcp_destructive: McpDestructive::Off,
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
    let text = match std::fs::read_to_string(&path) {
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

    Ok(cfg)
}

/// Guards the file-level parse-error stderr warning to once per process —
/// same rationale as [`UNKNOWN_KEY_WARNED`].
static LOAD_ERROR_WARNED: Once = Once::new();

/// The tolerant shim every per-key CLI reader (`effective_store_budget`,
/// `read_ttl_days`, `read_noise_globs`, `read_memory_enabled`,
/// `read_memory_inject_max`) goes through instead of calling [`load`]
/// directly. [`load`]'s `Err` — a file that fails to parse as TOML at all —
/// is where D16's hard error actually lives (its message names the
/// line/col); none of today's reader call sites take a `Result`, and
/// widening all of their callers (which fan out through `cmds.rs`,
/// `daemon.rs`, `main.rs`, ...) is out of this task's scope. So on `Err`
/// this degrades to [`Config::default`] for EVERY key — not just the
/// offending one — which matches the pre-toml-loader scanner's behavior
/// exactly (it also silently defaulted the whole read on a line it
/// couldn't make sense of), so every existing pinned-fallback test survives
/// unamended.
///
/// NOTE this means a single syntax error anywhere in the file reverts keys
/// that parsed fine too — e.g. a stray `mcp_destructive = "yes"` line
/// silently reverts a correct, adjacent `ttl_days = 30` back to 90. Wiring
/// a loud top-level failure (D16's "daemon startup included" language) is a
/// deliberate residual of this task, not an oversight — see CLAUDE.md.
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
    fn load_or_default_defaults_every_key_on_file_level_parse_error() {
        // Even the ttl_days on the SAME line-adjacent well-formed key is
        // reverted — load_or_default's documented all-or-nothing behavior.
        let cfg = load_or_default(&root_with("ttl_days = 30\nstore_budget_bytes = [unclosed"));
        assert_eq!(cfg, Config::default());
    }
}
