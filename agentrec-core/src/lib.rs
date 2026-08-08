//! agentrec-core: pure engine + persistence primitives for the agentrec recorder.
//! Modules: id (ULID), time (RFC 3339), store (content-addressed blobs),
//! record (protocol types), scrub (secret redaction), engine (turn boundaries),
//! diff (unified text diff + binary detection for the `diff` verb), memory
//! (hash-pinned semantic memory records, fold, fsynced append), undo_coordinator
//! (the destructive-decision seam: undo plan classification + reservations),
//! text (rendering primitives shared with the CLI's formatters), pathenc
//! (OS path → wire-string conversion decision — never lossy), stats
//! (per-repository analytics fold), search (substring/regex search over
//! prompts and turn metadata).
//! Normative semantics live in PROTOCOL.md at the repo root; when code and doc
//! disagree, the doc wins and the code is a bug.

pub mod diff;
pub mod engine;
pub mod fsguard;
pub mod id;
pub mod memory;
pub mod pathenc;
pub mod perms;
pub mod record;
pub mod retention;
pub mod scrub;
pub mod search;
pub mod stats;
pub mod store;
pub mod text;
pub mod time;
pub mod undo_coordinator;
pub mod view;

/// Per-file snapshot cap (PROTOCOL §6).
pub const MAX_SNAPSHOT_BYTES: usize = 10 * 1024 * 1024;
/// Store-wide size budget (AC I+): once exceeded, the oldest snapshot blobs
/// (never prompt blobs — those have their own TTL path) are evicted.
pub const MAX_STORE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Quiet window closing unattributed activity (PROTOCOL §4; Sutra-proven).
pub const QUIET_MS: u64 = 10_000;
/// Settle window closing a git-classified turn after its last mutation.
pub const GIT_SETTLE_MS: u64 = 2_000;
/// Safety net: a bracket with no stop signal closes truncated after this.
pub const MAX_BRACKET_MS: u64 = 2 * 60 * 60 * 1000;
/// Stop-only emitters: bare turns closed within this window fold into the rich turn.
pub const FOLD_WINDOW_MS: u64 = 15 * 60 * 1000;
