//! `UndoCoordinator` — the seam every destructive decision goes through,
//! independent of CLI or MCP transport (parent spec :286-318).
//!
//! **Task F2 shipped `preview` + path reservations. Task F3 adds the
//! confirm-mode request ledger** — `request_human`, `status`, the terminal
//! writers (`deny`/`expire`/`execute`/`fail`) and the revert primitive
//! [`execute_revert`], moved here from `cli/src/readcmds.rs` so `agentrec
//! approve` and the CLI's own `undo --confirm` share one execution path. The
//! undo-*turn* append still belongs to the CLI (`cli/src/approvecmd.rs`, via
//! `loglock`); core owns the decision and the writes, not the log writer.
//! `execute` (the auto-mode tokened path) is still F4's.
//!
//! ## The five states, and why one of them is never written here
//!
//! Parent :641-697 wants `status` to distinguish pending / approved /
//! denied / expired / executed. F3 derives them from the append-only event
//! stream, plus a sixth — `failed` — because the spec also says "Deny,
//! expiry, failure, or successful execution appends the terminal transition
//! that releases the reservation", and a drift-aborted approval has to land
//! somewhere honest rather than being reported as one of the other five.
//!
//! **`approved` is recognized on read and never written by F3, on purpose.**
//! The durable approval event is appended only AFTER the working-tree writes
//! and the undo turn's `sync_all`, so at every instant a crash can occur the
//! ledger reads either "pending" (nothing was approved) or "executed" (the
//! whole thing landed). That is exactly the parent spec's "restart cannot
//! convert an unapproved request into an approval": nothing that is not a
//! durably-written human decision may ever read as approved. Writing
//! `approve` *before* executing would create the phantom the rule forbids.
//! The status stays in the vocabulary because a host-native approval UI
//! (D23's A4) may one day separate the decision from the execution, and a
//! reader that did not recognize the event would silently report such a
//! request as pending. F2 set this precedent: it recognized all six
//! transitions on read while writing exactly one.
//!
//! The two crash consequences are both correct, and neither is a bug: a
//! crash before any file was written leaves the request re-approvable, and a
//! re-approve legitimately executes; a crash part-way through the writes
//! makes the recorded per-path hashes disagree with the disk, so the next
//! approve aborts with `preview_stale` and requires a fresh request.
//!
//! ## Why the plan interpretation lives in this crate now
//!
//! `build_plan` and its helpers were `cli/src/readcmds.rs` private functions
//! until this task moved them here verbatim. The MCP `agentrec_undo` preview
//! must reach *the same* refusal interpretation the CLI preview reaches —
//! F2's symlink refusal, the SR6 skipped-above-modified-since gate order, the
//! before-blob integrity read, D49's caution, K2's imported-unreconstructible
//! rule — and a second implementation behind the MCP seam is exactly the
//! failure this seam exists to prevent. `readcmds` re-imports them under
//! their original names, so its renderer and its entire test module resolve
//! unchanged against the moved code.
//!
//! ## `McpDestructive` lives here, and there is still only one of it
//!
//! The binding interface says `preview(&self, req, mode: McpDestructive)` and
//! "core defines no second mode type". `McpDestructive` was `cli::config`'s,
//! and core cannot depend on the CLI crate, so the enum MOVED here and
//! `cli::config` re-exports it. That keeps exactly one type with one name:
//! every existing `config::McpDestructive` path still resolves, and no
//! mapping function exists to drift. The alternative — a core behaviour enum
//! the CLI maps onto — is the "second mode type" the contract forbids.
//!
//! Core stays config-*free* in the sense that matters here (the rule
//! `view::health(budget)` established): this module never reads
//! `config.toml`, never learns its key names, and never picks a default. It
//! receives the already-resolved mode as a parameter, exactly as `health`
//! receives an already-resolved budget. What moved is a domain type — the
//! destructive-consent tier — not config machinery.
//!
//! ## Reservations are file-backed, and the spec is not silent about it
//!
//! Parent :286-318: "Pending human requests use an append-only, 0600 event
//! ledger under `.agentrec/`... Creating a confirm-mode pending request or
//! issuing an auto-mode token takes `.agentrec/undo.lock`, rechecks the
//! preview, and atomically reserves its executable paths **in that ledger**.
//! Any overlap with a live reservation is rejected immediately with
//! `undo_conflict`." So reservation state is a file, not process memory: a
//! second agentrec process (an `agentrec approve` run against a repo whose
//! MCP server holds a live token) must see it. The plan's global constraints
//! pre-sanction the file — "the 2.3 request ledger is a NEW append-only file
//! under `.agentrec/`, not a rewrite class".
//!
//! ## F4: why the token is spent BEFORE the writes, not after them
//!
//! F3's ordering — terminal row last, states recognized on read — is right
//! for a human request and **insufficient for a token**. AC-F4 requires the
//! second presentation of a token to fail *even if the first presentation
//! crashed mid-execution*, and a crash between the last worktree write and
//! the `execute` row leaves the reservation live and the token unspent.
//! Content drift usually refuses the retry (the files now hold their `before`
//! bytes), but "usually" is not the guarantee: anything that restores those
//! files inside the window — an editor undo, a `git checkout`, a second agent
//! — makes the same token execute again and mint a second undo turn.
//!
//! So [`EVENT_CONSUME`] is appended and fsynced under the lock **before the
//! first working-tree write**, and it is not terminal, so the reservation
//! keeps holding its paths while the writes run. The ordering is:
//!
//! ```text
//! lock → resolve token → content drift → scope drift → concurrent-undo guard
//!      → CONSUME (fsync) → guard write → worktree writes → undo turn (fsync)
//!      → EXECUTE row
//! ```
//!
//! At every instant a crash can land:
//!
//! | crash point | ledger reads | worktree | retry with the same token |
//! |---|---|---|---|
//! | before `consume` | `reserve` only | untouched | executes — correct, nothing happened |
//! | after `consume`, before writes | `reserve`+`consume` | untouched | `token_consumed` — conservative, a fresh preview costs nothing |
//! | mid-writes | `reserve`+`consume` | partly reverted | `token_consumed` — **the case the row exists for** |
//! | after writes, before the turn | `reserve`+`consume` | reverted | `token_consumed` |
//! | after the turn, before `execute` | `reserve`+`consume` | reverted | `token_consumed` |
//!
//! There is no instant at which the token can execute twice, and none at
//! which the ledger shows an execution that did not happen — `execute` is
//! still written last, exactly as F3 writes it. The cost is one direction of
//! conservatism (a token burned by a crash that wrote nothing) and one
//! disclosed residual: a consumed-but-unresolved reservation goes on holding
//! its paths until its 60 s TTL lapses, because releasing it early is the
//! thing that would let a second grant write to a file mid-revert.
//!
//! In F2 the only thing that ended a reservation was TTL expiry. **F3 adds
//! the terminal writers** — `deny`, `expire` (appended lazily, by whichever
//! approve/deny first observes a lapsed request), `execute`, and `fail` —
//! so a resolved request releases its paths immediately instead of holding
//! them for the rest of its TTL. A confirm-mode `request` is a reservation
//! too, not only F2's auto `reserve`: an auto preview must not hand out a
//! token overlapping paths a human is being asked to approve.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::record::{FileEntry, LogRecord, TurnRecord};
use crate::store::{hash_bytes, BlobStore, StoreError};
use crate::view;

/// Agent-driven-undo gating mode (PROTOCOL.md §8). Default `Off`.
///
/// Moved here from `cli::config` by task F2 and re-exported there — see this
/// module's header for why that is one type rather than two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpDestructive {
    #[default]
    Off,
    Confirm,
    Auto,
}

impl McpDestructive {
    /// The wire spelling, identical to the `config.toml` value that selects
    /// it and to `agentrec_status`'s `agent_undo_mode`.
    pub fn as_str(self) -> &'static str {
        match self {
            McpDestructive::Off => "off",
            McpDestructive::Confirm => "confirm",
            McpDestructive::Auto => "auto",
        }
    }
}

/// A turn reference as the wire carries it: a full `t_<ULID>` or an
/// unambiguous prefix, resolved through [`view::resolve_turn`] — the single
/// lookup choke point. An alias rather than a newtype on purpose: a newtype
/// would be a new domain type to keep in agreement with every `&str` turn
/// reference already crossing this codebase, and it would buy no invariant
/// (an unresolved reference is not more valid for being wrapped).
pub type TurnId = String;

/// How long an auto-mode confirm token stays valid (parent :286-318, and
/// :641-697's "single-use 60 s token").
pub const TOKEN_TTL_MS: u64 = 60_000;

/// How long a confirm-mode pending request stays approvable.
///
/// **Ten minutes, from D23 itself** (IMPLEMENTATION.md decision register):
/// "`confirm` mode approval UX (v2): `agentrec approve` lists pending undo
/// requests; approve/deny by id; **pending requests expire after 10
/// minutes**." Deliberately its own constant rather than a reuse of
/// [`TOKEN_TTL_MS`]: an auto token and a human approval window are different
/// deadlines for different actors, and folding them into one would silently
/// expire every request in 60 seconds while an expiry test still passed.
pub const REQUEST_TTL_MS: u64 = 600_000;

/// The request/reservation ledger's schema version. Independent of
/// PROTOCOL.md's `v`: this file is agentrec-internal, never a wire surface,
/// and F5 owns the next protocol change.
const LEDGER_V: u32 = 1;

// ---- request / preview / error types --------------------------------------

/// Parent :302: turn reference + optional path subset + `allow_modified`.
#[derive(Debug, Clone)]
pub struct UndoRequest {
    pub turn: TurnId,
    /// `None` = the whole turn; `Some` = only these paths.
    pub paths: Option<Vec<PathBuf>>,
    pub allow_modified: bool,
}

/// Why one file cannot be reverted. The class is a discriminator the CLI's
/// prose refusal string does not carry — an agent needs to branch on
/// "modified since" (a human may override in confirm mode) versus "withheld"
/// (nothing can ever override it) without parsing English.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalClass {
    /// Content was never snapshotted (over cap, IO failure, unreadable).
    Skipped,
    /// Secret-pattern file; never snapshotted by design.
    Withheld,
    /// A link is involved on either the record side or on disk (F2 refusal).
    Symlink,
    /// The prior snapshot is absent or corrupt.
    NoSnapshot,
    /// Current bytes differ from the turn's recorded `after`.
    ModifiedSince,
    /// K2: imported from external history with no recoverable pre-edit
    /// content — provenance only, never fabricated.
    ImportedUnreconstructible,
}

#[derive(Debug, Clone, Serialize)]
pub struct Refusal {
    pub path: String,
    pub class: RefusalClass,
    /// The same prose the CLI preview prints for this entry, so the two
    /// surfaces cannot drift on the vocabulary.
    pub reason: String,
}

/// One executable file's diff summary. Hashes, not content: a preview is a
/// decision aid, and `agentrec_diff` already carries bytes.
#[derive(Debug, Clone, Serialize)]
pub struct PreviewFile {
    pub path: String,
    pub op: String,
    pub before: Option<String>,
    pub after: Option<String>,
    /// Sizes of the two snapshotted states, when the store still holds them.
    /// `before_bytes` is what an execute would WRITE; `after_bytes` is what
    /// it would replace. Both, not just one: "how big is this revert" is the
    /// question the row exists to answer, and it needs the delta.
    pub before_bytes: Option<u64>,
    pub after_bytes: Option<u64>,
    /// True only when the file is executable *despite* being modified since
    /// the turn — i.e. confirm mode with `allow_modified`. Always false in
    /// auto mode (see [`UndoPreview::allow_modified_effective`]).
    pub modified_since: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warn: Option<String>,
}

/// A bound preview: target record, selected paths, refusals, effective mode,
/// and — auto mode only — the one-time token and its expiry.
#[derive(Debug, Clone, Serialize)]
pub struct UndoPreview {
    /// The fully resolved turn id, never the caller's prefix.
    pub turn: String,
    pub tool: Option<String>,
    pub grade: String,
    /// Effective mode, which is the mode as passed — this type never renames
    /// or re-tiers it.
    pub mode: &'static str,
    /// What `allow_modified` actually did. In auto mode this is always
    /// `false` regardless of the request: parent :641-697's rail is
    /// "modified-since files are excluded unconditionally", and the
    /// coordinator is the seam, so it applies the rail itself rather than
    /// trusting a transport to have refused the flag first.
    pub allow_modified_effective: bool,
    /// The executable set — exactly the files an `execute` would write.
    pub files: Vec<PreviewFile>,
    pub refusals: Vec<Refusal>,
    pub warnings: Vec<String>,
    /// Auto mode only, and only when something is executable. The raw value
    /// is returned to the caller exactly once and never persisted; the
    /// ledger stores its sha256.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_expires_unix_ms: Option<u64>,
    /// The reservation this preview created, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reservation: Option<String>,
}

#[derive(Debug)]
pub enum UndoError {
    /// Defence in depth: the MCP router already hides the tool when the mode
    /// is `off`, but the coordinator refuses independently rather than
    /// trusting one caller.
    ModeOff,
    NoTurns,
    UnknownTurn(String),
    AmbiguousTurn {
        turn_ref: String,
        matched: usize,
    },
    /// Requested paths that are not in the target turn — a caller error, not
    /// a silent no-op (mirrors the CLI's `--files` refusal).
    UnknownPaths(Vec<String>),
    /// P6: another live reservation already holds one of these paths.
    Conflict {
        reservation: String,
        paths: Vec<String>,
    },
    /// No usable entropy source for a consent token — see [`crate::id::random_token`].
    TokenUnavailable,
    /// F3: `request_human` outside confirm mode. Defence in depth — the MCP
    /// matrix already refuses it with `wrong_mode`, and the coordinator
    /// refuses independently rather than trusting one caller.
    ConfirmModeOnly(&'static str),
    /// F3: nothing in the turn is executable, so there is nothing to lodge.
    /// A pending request granting zero writes is a decision a human would be
    /// asked to make for no effect — the same reasoning that keeps auto from
    /// issuing a token for an empty executable set.
    NothingExecutable,
    /// F3: no `request` event carries this id.
    UnknownRequest(String),
    /// F3: an id PREFIX matching more than one request.
    AmbiguousRequest {
        request_ref: String,
        matched: usize,
    },
    /// F3: the request exists but is no longer approvable. Carries the state
    /// it is actually in, so a second `approve` says "already executed"
    /// rather than a generic failure.
    NotPending {
        request: String,
        state: &'static str,
    },
    /// F3: the working tree moved between the request and the approval. No
    /// writes happened; a fresh request is required.
    ///
    /// `refusals` carries the plan's own reason for each dropped path that
    /// was REFUSED rather than merely changed. Both causes are
    /// `preview_stale` on the wire — the caller's remedy is identical — but
    /// they are different facts, and reporting a containment refusal as
    /// "changed since it was lodged" misdirects an operator investigating an
    /// attack toward a benign explanation (branch review, PR #20, MINOR 2:
    /// observed on a preview→swap-parent-to-symlink→execute race whose
    /// content hash was deliberately IDENTICAL across the swap, so drift
    /// provably was not what refused it). Empty for genuine content drift.
    PreviewStale {
        request: String,
        paths: Vec<String>,
        refusals: Vec<String>,
    },
    /// F4: no reservation, live or dead, was ever issued for this token. An
    /// agent seeing this has a bug (it invented or mangled a token), which is
    /// why it is distinct from [`UndoError::TokenConsumed`] — that one means
    /// "take a fresh preview", this one means "you are not holding what you
    /// think you are holding".
    BadToken,
    /// F4: the token matched a reservation that has already been spent. The
    /// spend is recorded BEFORE the writes, so this fires even when the run
    /// that spent it died part-way through them.
    TokenConsumed,
    /// F4: the token matched a reservation whose 60 s window has closed.
    TokenExpired,
    Io(String),
}

impl UndoError {
    /// Stable machine-readable code. `undo_conflict` is the name the parent
    /// spec gives P6's rejection and is not renamed here.
    pub fn code(&self) -> &'static str {
        match self {
            UndoError::ModeOff => "mode_off",
            UndoError::NoTurns => "no_turns",
            UndoError::UnknownTurn(_) => "unknown_turn",
            UndoError::AmbiguousTurn { .. } => "ambiguous_turn",
            UndoError::UnknownPaths(_) => "unknown_path",
            UndoError::Conflict { .. } => "undo_conflict",
            UndoError::TokenUnavailable => "token_unavailable",
            UndoError::ConfirmModeOnly(_) => "wrong_mode",
            UndoError::NothingExecutable => "nothing_to_revert",
            UndoError::UnknownRequest(_) => "unknown_request",
            UndoError::AmbiguousRequest { .. } => "ambiguous_request",
            UndoError::NotPending { .. } => "not_pending",
            // The name the parent spec gives approval drift, unchanged:
            // ":641-697 Approval drift records `preview_stale` and requires a
            // new request."
            UndoError::PreviewStale { .. } => "preview_stale",
            UndoError::BadToken => "bad_token",
            UndoError::TokenConsumed => "token_consumed",
            UndoError::TokenExpired => "token_expired",
            UndoError::Io(_) => "repo_error",
        }
    }
}

impl std::fmt::Display for UndoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UndoError::ModeOff => write!(
                f,
                "agent-driven undo is disabled (mcp_destructive = \"off\") for this repository"
            ),
            UndoError::NoTurns => write!(f, "no turns recorded in this repository"),
            UndoError::UnknownTurn(r) => write!(f, "no turn matches {r:?}"),
            UndoError::AmbiguousTurn { turn_ref, matched } => write!(
                f,
                "{turn_ref:?} matches {matched} distinct turns — use a longer prefix"
            ),
            UndoError::UnknownPaths(p) => write!(
                f,
                "path(s) not in this turn: {} — nothing was previewed",
                p.join(", ")
            ),
            UndoError::Conflict { reservation, paths } => write!(
                f,
                "another live undo reservation ({reservation}) already holds {} — \
                 retry once it is executed, denied, or expires",
                paths.join(", ")
            ),
            UndoError::TokenUnavailable => write!(
                f,
                "cannot issue a confirm token: no system entropy source available"
            ),
            UndoError::ConfirmModeOnly(mode) => write!(
                f,
                "agent-undo mode is {mode:?}: lodging a request for human approval requires \
                 \"confirm\" mode"
            ),
            UndoError::NothingExecutable => write!(
                f,
                "nothing in this turn is executable — no request was lodged"
            ),
            UndoError::UnknownRequest(r) => write!(f, "no undo request matches {r:?}"),
            UndoError::AmbiguousRequest {
                request_ref,
                matched,
            } => write!(
                f,
                "{request_ref:?} matches {matched} undo requests — use a longer prefix"
            ),
            UndoError::NotPending { request, state } => write!(
                f,
                "undo request {request} is {state}, not pending — nothing was written"
            ),
            UndoError::PreviewStale {
                request,
                paths,
                refusals,
            } if !refusals.is_empty() => write!(
                f,
                "undo request {request} is stale: {} can no longer be reverted ({}) — nothing \
                 was written; ask for a fresh preview and a new request",
                paths.join(", "),
                refusals.join("; ")
            ),
            UndoError::PreviewStale { request, paths, .. } => write!(
                f,
                "undo request {request} is stale: {} changed since it was lodged — nothing was \
                 written; ask for a fresh preview and a new request",
                paths.join(", ")
            ),
            UndoError::BadToken => write!(
                f,
                "no undo reservation matches that token — nothing was written; call `preview` \
                 to obtain one"
            ),
            UndoError::TokenConsumed => write!(
                f,
                "that undo token has already been spent — nothing was written; a token is \
                 single-use, so call `preview` again for a fresh one"
            ),
            UndoError::TokenExpired => write!(
                f,
                "that undo token has expired — nothing was written; tokens are valid for {} \
                 seconds, so call `preview` again for a fresh one",
                TOKEN_TTL_MS / 1000
            ),
            UndoError::Io(m) => write!(f, "{m}"),
        }
    }
}

// ---- the coordinator -------------------------------------------------------

/// Root + the `.agentrec/` paths derived from it. Holds no open handles: a
/// coordinator is cheap to construct per call and never caches log state,
/// because a preview must see the log as it is at preview time.
pub struct UndoCoordinator {
    root: PathBuf,
}

impl UndoCoordinator {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        UndoCoordinator { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn agentrec_dir(&self) -> PathBuf {
        self.root.join(".agentrec")
    }

    fn log_path(&self) -> PathBuf {
        self.agentrec_dir().join("log.jsonl")
    }

    fn objects_dir(&self) -> PathBuf {
        self.agentrec_dir().join("objects")
    }

    /// The append-only 0600 event ledger (parent :286-318).
    pub fn ledger_path(&self) -> PathBuf {
        self.agentrec_dir().join("undo-requests.jsonl")
    }

    /// The exclusive lock every reservation-creating operation takes.
    pub fn lock_path(&self) -> PathBuf {
        self.agentrec_dir().join("undo.lock")
    }

    /// Bind a preview: resolve the turn, classify every selected file, and —
    /// in auto mode, when something is actually executable — atomically
    /// reserve the executable paths and issue a one-time token.
    ///
    /// **No working-tree writes, in any mode.** In confirm mode nothing is
    /// written at all, `.agentrec/` included. In auto mode the only write is
    /// the ledger append (plus the lock file's creation), which is what a
    /// reservation *is*.
    pub fn preview(
        &self,
        req: UndoRequest,
        mode: McpDestructive,
    ) -> Result<UndoPreview, UndoError> {
        if mode == McpDestructive::Off {
            return Err(UndoError::ModeOff);
        }

        let records = crate::record::load_log(&self.log_path());
        let turns: Vec<&TurnRecord> = records
            .iter()
            .filter_map(|r| match r {
                LogRecord::Turn(t) => Some(t),
                LogRecord::Epoch(_) => None,
            })
            .collect();
        let target = view::resolve_turn(&turns, &req.turn).map_err(|e| match e {
            view::LookupError::NoTurns => UndoError::NoTurns,
            view::LookupError::Unknown => UndoError::UnknownTurn(req.turn.clone()),
            view::LookupError::Ambiguous { matched } => UndoError::AmbiguousTurn {
                turn_ref: req.turn.clone(),
                matched,
            },
        })?;
        let target_idx = turns.iter().position(|t| t.id == target.id).unwrap_or(0);

        // Requested subset → wire strings, validated against the turn. A
        // typo is refused rather than silently previewing nothing, which an
        // agent would read as "there is nothing to revert".
        let filter: Vec<String> = match &req.paths {
            None => Vec::new(),
            Some(paths) => {
                let selected: Vec<String> = paths
                    .iter()
                    .map(|p| {
                        crate::pathenc::utf8_path(p)
                            .map(str::to_string)
                            .unwrap_or_else(|| p.to_string_lossy().into_owned())
                    })
                    .collect();
                let known: HashSet<&str> = target.files.iter().map(|f| f.path.as_str()).collect();
                let unknown: Vec<String> = selected
                    .iter()
                    .filter(|p| !known.contains(p.as_str()))
                    .cloned()
                    .collect();
                if !unknown.is_empty() {
                    return Err(UndoError::UnknownPaths(unknown));
                }
                selected
            }
        };

        // The auto-mode rail (parent :641-697, founder decision 6): auto
        // excludes modified-since files unconditionally. Applied HERE, not
        // only at the transport, because the coordinator is the seam and
        // cannot assume a caller refused the flag first.
        let allow_modified = match mode {
            McpDestructive::Auto => false,
            _ => req.allow_modified,
        };

        let mut warnings: Vec<String> = Vec::new();
        if mode == McpDestructive::Auto && req.allow_modified {
            warnings.push(
                "allow_modified was requested but auto mode excludes modified-since files \
                 unconditionally; the override exists only through explicit human approval \
                 in confirm mode"
                    .to_string(),
            );
        }

        // K2, and the ONE place its whole-turn scope is decided.
        //
        // `readcmds::undo` refuses a K2 turn *before* `build_plan` runs, and
        // its pinned invariant (AC3) is "never a partial revert of the
        // other, genuinely-revertible entries in the same turn". AC-F2 wants
        // imported-unreconstructible enumerated as a refusal CLASS on the
        // preview. Both hold here: the K2 entries are enumerated per file,
        // and the executable set is empty, so auto reserves nothing, issues
        // no token, and F4 can never execute the turn. The remaining entries
        // are deliberately NOT classified — the CLI does not classify them
        // either (it returns before `build_plan`), and inventing rows for
        // them would let a reader believe a partial revert is on offer.
        let k2: Vec<&FileEntry> = if target.imported == Some(true) {
            target
                .files
                .iter()
                .filter(|f| filter.is_empty() || filter.contains(&f.path))
                .filter(|f| f.op != "create" && f.before.is_none())
                .collect()
        } else {
            Vec::new()
        };
        if !k2.is_empty() {
            let refusals: Vec<Refusal> = k2
                .iter()
                .map(|f| Refusal {
                    path: f.path.clone(),
                    class: RefusalClass::ImportedUnreconstructible,
                    reason: "imported from external history with no recoverable pre-edit \
                             content — provenance-only, never fabricated"
                        .to_string(),
                })
                .collect();
            warnings.push(format!(
                "turn {} was imported from external history and has no recoverable pre-edit \
                 content for {} file(s): the WHOLE undo is refused rather than partially \
                 applied, and no other file in this turn was classified",
                target.id,
                k2.len()
            ));
            return Ok(UndoPreview {
                turn: target.id.clone(),
                tool: target.tool.clone(),
                grade: target.grade.clone(),
                mode: mode.as_str(),
                allow_modified_effective: allow_modified,
                files: Vec::new(),
                refusals,
                warnings,
                token: None,
                token_expires_unix_ms: None,
                reservation: None,
            });
        }

        let store = BlobStore::new(self.objects_dir());
        let plans = build_plan(
            &self.root,
            &store,
            target,
            target_idx,
            &turns,
            &records,
            &filter,
            allow_modified,
        );

        let mut files = Vec::new();
        let mut refusals = Vec::new();
        for p in &plans {
            match &p.kind {
                PlanKind::Revert { warn } => files.push(PreviewFile {
                    path: p.entry.path.clone(),
                    op: p.entry.op.clone(),
                    before: p.entry.before.clone(),
                    after: p.entry.after.clone(),
                    before_bytes: p.entry.before.as_deref().and_then(|h| store.size(h)),
                    after_bytes: p.entry.after.as_deref().and_then(|h| store.size(h)),
                    modified_since: warn.is_some(),
                    warn: warn.clone(),
                }),
                PlanKind::Excluded { cause } => refusals.push(Refusal {
                    path: p.entry.path.clone(),
                    class: RefusalClass::ModifiedSince,
                    reason: cause.clone(),
                }),
                PlanKind::Refused { reason } => refusals.push(Refusal {
                    path: p.entry.path.clone(),
                    class: refusal_class(&self.root, &p.entry),
                    reason: reason.clone(),
                }),
            }
        }

        if let Some(caution) = window_caution(target, &plans) {
            warnings.push(caution);
        }

        // Auto mode reserves. Nothing executable means nothing to reserve —
        // and no token, because a token that grants zero writes is a
        // credential an agent could only misread.
        let (token, expires, reservation) = if mode == McpDestructive::Auto && !files.is_empty() {
            let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
            let r = self.reserve(&target.id, &paths)?;
            (Some(r.token), Some(r.expires_unix_ms), Some(r.id))
        } else {
            (None, None, None)
        };

        Ok(UndoPreview {
            turn: target.id.clone(),
            tool: target.tool.clone(),
            grade: target.grade.clone(),
            mode: mode.as_str(),
            allow_modified_effective: allow_modified,
            files,
            refusals,
            warnings,
            token,
            token_expires_unix_ms: expires,
            reservation,
        })
    }

    /// Every live (unreleased, unexpired) reservation, newest last.
    pub fn live_reservations(&self) -> Result<Vec<LedgerEvent>, UndoError> {
        let text = match std::fs::read_to_string(self.ledger_path()) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(UndoError::Io(format!(
                    "cannot read {}: {e}",
                    self.ledger_path().display()
                )))
            }
        };
        Ok(live_from_ledger(&text, now_ms()))
    }

    /// Take the lock, re-read the ledger under it, refuse any overlap, and
    /// append the reservation. Lock held across all three: a check outside
    /// the lock would let two previews both see "no conflict".
    fn reserve(&self, turn: &str, paths: &[String]) -> Result<Reservation, UndoError> {
        let dir = self.agentrec_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| UndoError::Io(format!("cannot create {}: {e}", dir.display())))?;
        crate::perms::lock_dir(&dir);

        let _guard = UndoLock::acquire(&self.lock_path())?;

        let existing = self.live_reservations()?;
        let wanted: HashSet<&str> = paths.iter().map(String::as_str).collect();
        for ev in &existing {
            let overlap: Vec<String> = ev
                .paths
                .iter()
                .filter(|p| wanted.contains(p.as_str()))
                .cloned()
                .collect();
            if !overlap.is_empty() {
                return Err(UndoError::Conflict {
                    reservation: ev.id.clone(),
                    paths: overlap,
                });
            }
        }

        // Raw token out, hash in. A ledger readable by anyone who can read
        // `.agentrec/` must not itself be the credential.
        let token = crate::id::random_token().ok_or(UndoError::TokenUnavailable)?;
        let now = now_ms();
        let event = LedgerEvent {
            v: LEDGER_V,
            event: EVENT_RESERVE.to_string(),
            id: crate::id::ulid(),
            turn: turn.to_string(),
            paths: paths.to_vec(),
            token_sha256: Some(hash_bytes(token.as_bytes())),
            expires_unix_ms: now + TOKEN_TTL_MS,
            at_unix_ms: now,
            // F4: the drift baseline, recorded here because P4's "hash
            // recheck under lock before writes" needs something to recheck
            // AGAINST — what each path held at preview time, which is what
            // the token was issued over. Same field, same shape and same
            // comparison as `request_human`'s, so one drift rule serves both
            // flows. A `reserve` row written by a pre-F4 binary carries an
            // EMPTY `hashes`, which reads as total drift and refuses: that
            // falls out of the `Option<Option<String>>` comparison rather
            // than being handled, and it fails closed, which is the safe
            // direction.
            hashes: paths
                .iter()
                .map(|p| read_current_hash(&self.root, p))
                .collect(),
            allow_modified: false,
            undo_turn: None,
            reason: None,
        };
        self.append_event(&event)?;
        Ok(Reservation {
            id: event.id,
            token,
            expires_unix_ms: event.expires_unix_ms,
        })
    }

    /// Append one line, fsync, and lock the file down to 0600. Caller must
    /// already hold [`Self::lock_path`].
    fn append_event(&self, event: &LedgerEvent) -> Result<(), UndoError> {
        let path = self.ledger_path();
        let line = serde_json::to_string(event)
            .map_err(|e| UndoError::Io(format!("cannot encode ledger event: {e}")))?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| UndoError::Io(format!("cannot open {}: {e}", path.display())))?;
        f.write_all(line.as_bytes())
            .and_then(|()| f.write_all(b"\n"))
            .and_then(|()| f.sync_all())
            .map_err(|e| UndoError::Io(format!("cannot append to {}: {e}", path.display())))?;
        crate::perms::lock_file(&path);
        Ok(())
    }

    // ---- F3: the confirm-mode request ledger ------------------------------

    /// Take `.agentrec/undo.lock`. Public because `agentrec approve` has to
    /// hold ONE lock across recheck → working-tree writes → undo-turn
    /// append/fsync → terminal append (parent :286-318), and the undo-turn
    /// append is the CLI's (`loglock`), not core's. Handing the caller the
    /// guard is what lets the two crates share a single critical section
    /// without core learning about the log writer.
    pub fn lock(&self) -> Result<UndoLock, UndoError> {
        let dir = self.agentrec_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| UndoError::Io(format!("cannot create {}: {e}", dir.display())))?;
        crate::perms::lock_dir(&dir);
        UndoLock::acquire(&self.lock_path())
    }

    /// Every ledger event, oldest first. Unparseable lines are skipped.
    pub fn events(&self) -> Result<Vec<LedgerEvent>, UndoError> {
        match std::fs::read_to_string(self.ledger_path()) {
            Ok(t) => Ok(parse_ledger(&t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(UndoError::Io(format!(
                "cannot read {}: {e}",
                self.ledger_path().display()
            ))),
        }
    }

    /// Lodge a pending request for a human to approve or deny out of band
    /// (parent :641-697 `request`; D23).
    ///
    /// The lock is taken FIRST and the preview computed under it, so the
    /// conflict check and the append cannot straddle a window in which
    /// another process reserved the same paths. That is safe only because a
    /// confirm-mode `preview` reserves nothing and therefore never re-enters
    /// [`Self::reserve`] — an auto-mode preview would deadlock on its own
    /// lock, which is why this method refuses any other mode outright.
    pub fn request_human(
        &self,
        req: UndoRequest,
        mode: McpDestructive,
    ) -> Result<PendingUndo, UndoError> {
        if mode != McpDestructive::Confirm {
            return Err(UndoError::ConfirmModeOnly(mode.as_str()));
        }
        let lock = self.lock()?;

        let preview = self.preview(req, mode)?;
        if preview.files.is_empty() {
            return Err(UndoError::NothingExecutable);
        }
        let paths: Vec<String> = preview.files.iter().map(|f| f.path.clone()).collect();

        // Conflict check under the lock, against BOTH reservation kinds.
        let live = self.live_reservations()?;
        let wanted: HashSet<&str> = paths.iter().map(String::as_str).collect();
        for ev in &live {
            let overlap: Vec<String> = ev
                .paths
                .iter()
                .filter(|p| wanted.contains(p.as_str()))
                .cloned()
                .collect();
            if !overlap.is_empty() {
                return Err(UndoError::Conflict {
                    reservation: ev.id.clone(),
                    paths: overlap,
                });
            }
        }

        // The drift baseline: what each path holds RIGHT NOW, which is what
        // the human is being shown. Recorded as `Option`, so absent → present
        // is drift and absent → absent is not.
        let hashes: Vec<Option<String>> = paths
            .iter()
            .map(|p| read_current_hash(&self.root, p))
            .collect();

        let now = now_ms();
        let event = LedgerEvent {
            v: LEDGER_V,
            event: EVENT_REQUEST.to_string(),
            id: crate::id::ulid(),
            turn: preview.turn.clone(),
            paths,
            token_sha256: None,
            expires_unix_ms: now + REQUEST_TTL_MS,
            at_unix_ms: now,
            hashes,
            allow_modified: preview.allow_modified_effective,
            undo_turn: None,
            reason: None,
        };
        self.append_event(&event)?;
        drop(lock);

        Ok(PendingUndo {
            status: PendingStatus {
                request: event.id.clone(),
                state: RequestState::Pending,
                turn: event.turn.clone(),
                paths: event.paths.clone(),
                allow_modified: event.allow_modified,
                requested_unix_ms: event.at_unix_ms,
                expires_unix_ms: event.expires_unix_ms,
                undo_turn: None,
                reason: None,
            },
            preview,
        })
    }

    /// Resolve a request id — full, or an unambiguous prefix — to its
    /// `request` event. A prefix matching two requests is refused rather
    /// than resolved to the first, exactly as [`view::resolve_turn`] refuses
    /// an ambiguous turn prefix.
    pub fn resolve_request(
        &self,
        events: &[LedgerEvent],
        request_ref: &str,
    ) -> Result<LedgerEvent, UndoError> {
        let matches: Vec<&LedgerEvent> = events
            .iter()
            .filter(|e| e.event == EVENT_REQUEST)
            .filter(|e| e.id == request_ref || e.id.starts_with(request_ref))
            .collect();
        match matches.len() {
            0 => Err(UndoError::UnknownRequest(request_ref.to_string())),
            1 => Ok(matches[0].clone()),
            n => Err(UndoError::AmbiguousRequest {
                request_ref: request_ref.to_string(),
                matched: n,
            }),
        }
    }

    /// One request's current state (parent :641-697 `status`).
    pub fn status(&self, request_ref: &str) -> Result<PendingStatus, UndoError> {
        let events = self.events()?;
        let request = self.resolve_request(&events, request_ref)?;
        Ok(self.status_of(&request, &events, now_ms()))
    }

    /// [`Self::status`] against an already-read event list — the form the
    /// approve path uses so it reports the state it decided on, not a state
    /// re-read after the fact.
    pub fn status_of(
        &self,
        request: &LedgerEvent,
        events: &[LedgerEvent],
        now: u64,
    ) -> PendingStatus {
        let state = state_from_events(request, events, now);
        let resolution = events
            .iter()
            .rev()
            .find(|e| e.id == request.id && TERMINAL_EVENTS.contains(&e.event.as_str()));
        PendingStatus {
            request: request.id.clone(),
            state,
            turn: request.turn.clone(),
            paths: request.paths.clone(),
            allow_modified: request.allow_modified,
            requested_unix_ms: request.at_unix_ms,
            expires_unix_ms: request.expires_unix_ms,
            undo_turn: resolution.and_then(|e| e.undo_turn.clone()),
            reason: resolution.and_then(|e| e.reason.clone()),
        }
    }

    /// Every request that is still awaiting a human, oldest first — what
    /// bare `agentrec approve` lists (D23: "`agentrec approve` lists pending
    /// undo requests").
    pub fn pending_requests(&self) -> Result<Vec<PendingStatus>, UndoError> {
        let events = self.events()?;
        let now = now_ms();
        Ok(events
            .iter()
            .filter(|e| e.event == EVENT_REQUEST)
            .map(|e| self.status_of(e, &events, now))
            .filter(|s| s.state == RequestState::Pending)
            .collect())
    }

    /// Claim a pending request for execution, under a lock the CALLER
    /// already holds.
    ///
    /// Returns the request event and the plan its approval would apply. Every
    /// refusal here is terminal-appending where the spec says it should be —
    /// a lapsed request gets its `expire` row, approval drift gets its `fail`
    /// row with reason `preview_stale` — and **none of them writes to the
    /// working tree**. The caller does the writes; this decides whether there
    /// are any.
    pub fn claim(&self, lock: &UndoLock, request_ref: &str) -> Result<Claim, UndoError> {
        let events = self.events()?;
        let request = self.resolve_request(&events, request_ref)?;
        let now = now_ms();
        match state_from_events(&request, &events, now) {
            RequestState::Pending => {}
            RequestState::Expired => {
                // Record the lapse the clock already decided, then refuse.
                self.append_terminal(lock, &request, EVENT_EXPIRE, None, None)?;
                return Err(UndoError::NotPending {
                    request: request.id.clone(),
                    state: RequestState::Expired.as_str(),
                });
            }
            other => {
                return Err(UndoError::NotPending {
                    request: request.id.clone(),
                    state: other.as_str(),
                })
            }
        }
        self.claim_grant(lock, request)
    }

    /// Resolve an auto-mode token to a claim, under a lock the CALLER already
    /// holds (F4; parent :641-697 `execute`).
    ///
    /// Validation is by HASH: the presented token is hashed and matched
    /// against `token_sha256` over EVERY `reserve` row, live or not. Matching
    /// only live rows would collapse "reused" and "never issued" into one
    /// answer — a spent reservation has been released by its `execute` row and
    /// is no longer live — and those two mean different things to an agent
    /// (take a fresh preview vs. fix your caller).
    ///
    /// **Nothing is appended on a bad, spent, or expired token.** That is a
    /// deliberate divergence from [`Self::claim`], which lazily records a
    /// lapsed request's `expire`: expiry here is already implicit in the
    /// reservation's own `expires_unix_ms` (the field [`live_from_ledger`]
    /// filters on), so no row is owed for correctness, and AC-F4 asks for
    /// zero side effects on exactly these three. Drift is different and DOES
    /// append its `fail` row — see [`Self::claim_grant`].
    ///
    /// **Measured, not assumed:** of [`Self::claim_grant`]'s two rechecks,
    /// only the SCOPE one is independently load-bearing on this path.
    /// `allow_modified` is always false in auto mode, so the reserve
    /// baseline equals the turn's `after` by construction, and any content
    /// drift turns the entry into a `build_plan` exclusion that the scope
    /// check then catches. Neutering the content recheck alone leaves every
    /// F4 test green (it reds three of F3's, where `allow_modified` makes it
    /// the only detector); neutering the scope check alone reds
    /// `ac_f4_an_executable_set_that_shrank_after_the_preview_is_preview_stale`;
    /// neutering both reds the drift test as well. It stays because the code
    /// is shared with the confirm path, not because this path needs it.
    pub fn claim_token(&self, lock: &UndoLock, token: &str) -> Result<Claim, UndoError> {
        let events = self.events()?;
        let presented = hash_bytes(token.as_bytes());
        let reservation = events
            .iter()
            .filter(|e| e.event == EVENT_RESERVE)
            .find(|e| e.token_sha256.as_deref() == Some(presented.as_str()))
            .cloned()
            .ok_or(UndoError::BadToken)?;

        // Spent-ness outranks expiry: a token spent at second 59 and
        // presented at second 61 is reused, not merely stale, and reporting
        // it as expired would invite the agent to blame the clock.
        if events
            .iter()
            .any(|e| e.id == reservation.id && e.event == EVENT_CONSUME)
        {
            return Err(UndoError::TokenConsumed);
        }
        // A terminal row without a `consume` means the grant was resolved
        // some other way (drift `fail`, or a `deny`): the token is dead too.
        if events
            .iter()
            .any(|e| e.id == reservation.id && TERMINAL_EVENTS.contains(&e.event.as_str()))
        {
            return Err(UndoError::TokenConsumed);
        }
        // ONE clock rule: the same comparison `live_from_ledger` applies,
        // against the same field. Re-deriving the deadline from
        // `at_unix_ms + TOKEN_TTL_MS` would be a second rule to keep in
        // agreement with the first.
        if reservation.expires_unix_ms <= now_ms() {
            return Err(UndoError::TokenExpired);
        }

        self.claim_grant(lock, reservation)
    }

    /// Spend the token: append and fsync [`EVENT_CONSUME`] for `reservation`.
    /// Caller must hold [`Self::lock`], and must call this **before the first
    /// working-tree write** — that ordering is the whole invariant. See the
    /// module header's crash matrix.
    pub fn consume_token(
        &self,
        _lock: &UndoLock,
        reservation: &LedgerEvent,
    ) -> Result<(), UndoError> {
        let ev = LedgerEvent::terminal(EVENT_CONSUME, reservation, now_ms());
        self.append_event(&ev)
    }

    /// The half of a claim that is identical for a human-approved request and
    /// an auto-mode token: content drift, then scope drift, then the plan.
    ///
    /// Extracted from F3's `claim` rather than copied, because the two flows
    /// differ ONLY in how the grant is authorized — and a second drift
    /// implementation behind the token seam is exactly the divergence this
    /// module exists to prevent. `grant` is a `request` row for `claim` and a
    /// `reserve` row for `claim_token`; both carry `paths`, `hashes`,
    /// `allow_modified` and `turn`, which is everything used here.
    ///
    /// **Every refusal appends the `fail` row it owes and writes nothing to
    /// the working tree.** The row is not bookkeeping: it is terminal, so it
    /// RELEASES the grant's path reservation — without it the fresh preview
    /// this error tells the caller to take would collide with the dead grant
    /// it is replacing.
    fn claim_grant(&self, lock: &UndoLock, grant: LedgerEvent) -> Result<Claim, UndoError> {
        let request = grant;

        // Drift: compare each path's CURRENT bytes against what it held when
        // the request was lodged. Not a re-run of the planner — with
        // `allow_modified` the planner admits an already-modified file, so it
        // cannot see this at all.
        let drifted: Vec<String> = request
            .paths
            .iter()
            .enumerate()
            .filter(|(i, p)| request.hashes.get(*i) != Some(&read_current_hash(&self.root, p)))
            .map(|(_, p)| p.clone())
            .collect();
        if !drifted.is_empty() {
            self.append_terminal(
                lock,
                &request,
                EVENT_FAIL,
                None,
                Some("preview_stale".to_string()),
            )?;
            return Err(UndoError::PreviewStale {
                request: request.id.clone(),
                paths: drifted,
                // Genuine content drift: the hash moved. No plan was built
                // here, so there is no refusal reason to carry.
                refusals: Vec::new(),
            });
        }

        let records = crate::record::load_log(&self.log_path());
        let turns: Vec<&TurnRecord> = records
            .iter()
            .filter_map(|r| match r {
                LogRecord::Turn(t) => Some(t),
                LogRecord::Epoch(_) => None,
            })
            .collect();
        let target = view::resolve_turn(&turns, &request.turn)
            .map_err(|_| UndoError::UnknownTurn(request.turn.clone()))?;
        let target_idx = turns.iter().position(|t| t.id == target.id).unwrap_or(0);
        let store = BlobStore::new(self.objects_dir());
        let plans = build_plan(
            &self.root,
            &store,
            target,
            target_idx,
            &turns,
            &records,
            &request.paths,
            request.allow_modified,
        );
        // The executable set must still be EXACTLY what was approved. Content
        // drift is caught above; this catches SCOPE drift — a `purge
        // --snapshots-before` between the request and the approval can turn
        // one entry into a refusal, and reverting the remainder would apply a
        // narrower change than the human agreed to while reporting success.
        // A preview "binds the target record, selected paths, current hashes"
        // (parent :286-318); a subset is not that preview, so it is
        // `preview_stale` like any other divergence, and the empty case falls
        // out of the same check rather than needing its own rule.
        let executable: HashSet<&str> = plans
            .iter()
            .filter(|p| matches!(p.kind, PlanKind::Revert { .. }))
            .map(|p| p.entry.path.as_str())
            .collect();
        let dropped: Vec<String> = request
            .paths
            .iter()
            .filter(|p| !executable.contains(p.as_str()))
            .cloned()
            .collect();
        if !dropped.is_empty() {
            // Name the plan's OWN reason for each dropped path that was
            // refused outright, so a containment refusal is not reported as
            // content drift (branch review MINOR 2). Paths dropped for any
            // other cause (excluded, absent from the plan entirely)
            // contribute nothing here and fall back to the drift wording.
            let refusals: Vec<String> = plans
                .iter()
                .filter(|p| dropped.iter().any(|d| d == &p.entry.path))
                .filter_map(|p| match &p.kind {
                    PlanKind::Refused { reason } => Some(reason.clone()),
                    _ => None,
                })
                .collect();
            self.append_terminal(
                lock,
                &request,
                EVENT_FAIL,
                None,
                Some("preview_stale".to_string()),
            )?;
            return Err(UndoError::PreviewStale {
                request: request.id.clone(),
                paths: dropped,
                refusals,
            });
        }

        Ok(Claim {
            target: target.clone(),
            plans,
            request,
        })
    }

    /// Deny a pending request: a terminal row, and not one byte written to
    /// the working tree (parent :641-697, D23).
    pub fn deny(&self, request_ref: &str) -> Result<PendingStatus, UndoError> {
        let lock = self.lock()?;
        let events = self.events()?;
        let request = self.resolve_request(&events, request_ref)?;
        let now = now_ms();
        match state_from_events(&request, &events, now) {
            RequestState::Pending => {}
            RequestState::Expired => {
                self.append_terminal(&lock, &request, EVENT_EXPIRE, None, None)?;
                return Err(UndoError::NotPending {
                    request: request.id.clone(),
                    state: RequestState::Expired.as_str(),
                });
            }
            other => {
                return Err(UndoError::NotPending {
                    request: request.id.clone(),
                    state: other.as_str(),
                })
            }
        }
        self.append_terminal(&lock, &request, EVENT_DENY, None, None)?;
        let events = self.events()?;
        Ok(self.status_of(&request, &events, now_ms()))
    }

    /// Append the terminal row that resolves `request`. Caller must hold
    /// [`Self::lock`] — the `&UndoLock` parameter is how that is stated in
    /// the type system rather than in a comment.
    pub fn append_terminal(
        &self,
        _lock: &UndoLock,
        request: &LedgerEvent,
        event: &str,
        undo_turn: Option<String>,
        reason: Option<String>,
    ) -> Result<(), UndoError> {
        let mut ev = LedgerEvent::terminal(event, request, now_ms());
        ev.undo_turn = undo_turn;
        ev.reason = reason;
        self.append_event(&ev)
    }

    /// Apply one file's revert, snapshotting its current bytes first. Thin
    /// re-export of [`execute_revert`] bound to this coordinator's root and
    /// store, so a caller that already has a coordinator does not have to
    /// rebuild either.
    pub fn revert_file(&self, entry: &FileEntry) -> Result<FileEntry, String> {
        let store = BlobStore::new(self.objects_dir());
        execute_revert(&self.root, &store, entry)
    }
}

/// What [`UndoCoordinator::claim`] hands back: the request it claimed, the
/// turn that request targets, and the per-file plan an approval applies. The
/// target travels with the plan because the caller has to RENDER it, and
/// re-resolving the turn on the CLI side would be a second lookup that could
/// disagree with the one the plan was built from.
pub struct Claim {
    pub request: LedgerEvent,
    pub target: TurnRecord,
    pub plans: Vec<Plan>,
}

/// What [`UndoCoordinator::reserve`] hands back: the ledger row's id, the
/// one-time raw token (never stored), and the expiry.
struct Reservation {
    id: String,
    token: String,
    expires_unix_ms: u64,
}

// ---- the event ledger ------------------------------------------------------

/// The auto-mode reservation event (F2): a token was issued for these paths.
pub const EVENT_RESERVE: &str = "reserve";

/// The confirm-mode pending-request event (F3): a human has been asked to
/// approve these paths.
pub const EVENT_REQUEST: &str = "request";

/// A human said yes. **Never written by F3** — see the module header: the
/// durable approval is [`EVENT_EXECUTE`], appended only after the writes and
/// the undo turn's fsync, so an interrupted approval can never read as one.
pub const EVENT_APPROVE: &str = "approve";
/// F4: the auto-mode token was SPENT. Appended and fsynced **before** the
/// first working-tree write, which is the whole point of the row — see
/// [`UndoCoordinator::consume_token`].
///
/// Deliberately NOT in [`TERMINAL_EVENTS`]: a consumed reservation still
/// intends to write to its paths (the writes are happening right now), and
/// releasing them mid-revert would let an overlapping grant exist against a
/// file being rewritten. It is not in [`RESERVING_EVENTS`] either — it does
/// not create a reservation, it annotates one.
pub const EVENT_CONSUME: &str = "consume";
/// A human said no (F3). Terminal; no working-tree write happened.
pub const EVENT_DENY: &str = "deny";
/// The approval window lapsed (F3). Appended lazily by whichever `approve`
/// or `deny` first observes it, so the ledger records the lapse rather than
/// leaving it implicit in a timestamp comparison.
pub const EVENT_EXPIRE: &str = "expire";
/// The revert ran to completion and its undo turn is in `log.jsonl` (F3).
pub const EVENT_EXECUTE: &str = "execute";
/// The approval was abandoned without completing (F3): today only approval
/// drift, whose `reason` is `preview_stale`.
pub const EVENT_FAIL: &str = "fail";

/// The two events that CREATE a reservation. Both, not just F2's `reserve`:
/// a confirm-mode pending request holds its paths exactly as an auto token
/// does, and if it did not, an auto preview could mint a token overlapping
/// the paths a human is being asked to approve — two live grants over one
/// file, which is the state P6 exists to make impossible.
pub const RESERVING_EVENTS: &[&str] = &[EVENT_RESERVE, EVENT_REQUEST];

/// The transitions that RELEASE a reservation. The parent spec's list is
/// "Deny, expiry, failure, or successful execution appends the terminal
/// transition that releases the reservation" — `approve` is deliberately
/// absent even though it is a state transition, because an approved-but-not-
/// yet-executed request still intends to write to its paths, and releasing
/// them would let an overlapping grant exist alongside it. F3 writes all four
/// of these.
pub const TERMINAL_EVENTS: &[&str] = &[EVENT_DENY, EVENT_EXPIRE, EVENT_EXECUTE, EVENT_FAIL];

/// One append-only line of `.agentrec/undo-requests.jsonl`. Unknown fields on
/// a known event are tolerated (the additive-only consumer contract), and an
/// unparseable line is skipped rather than failing the read — a corrupt tail
/// must not make every future reservation impossible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEvent {
    pub v: u32,
    pub event: String,
    pub id: String,
    pub turn: String,
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_sha256: Option<String>,
    pub expires_unix_ms: u64,
    pub at_unix_ms: u64,

    // ---- F3, additive; every one defaults so an F2-written line still
    // ---- deserializes unchanged.
    /// Parallel to [`Self::paths`]: the on-disk content hash each path had
    /// when the request was lodged, `None` for an absent file. This — not a
    /// re-run of `build_plan` — is what detects approval drift, because with
    /// `allow_modified: true` the planner deliberately admits a file whose
    /// content already differs from the turn's `after`, so the planner alone
    /// would see no drift at all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hashes: Vec<Option<String>>,
    /// The `allow_modified` the request was lodged with, so the approval
    /// rebuilds the SAME executable set a human was shown rather than a
    /// stricter or looser one.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_modified: bool,
    /// On [`EVENT_EXECUTE`]: the id of the undo turn appended to
    /// `log.jsonl`, so a resolved request points at its own evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_turn: Option<String>,
    /// On [`EVENT_FAIL`]: why. Today only `preview_stale`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl LedgerEvent {
    /// A terminal event resolving `request`. Carries the same `id` — F2's
    /// convention, on which [`live_from_ledger`] already depends ("a
    /// `reserve` event is live iff nothing terminal carries its id") — plus
    /// the turn, so a ledger line is readable without joining to its
    /// request.
    fn terminal(event: &str, request: &LedgerEvent, now: u64) -> Self {
        LedgerEvent {
            v: LEDGER_V,
            event: event.to_string(),
            id: request.id.clone(),
            turn: request.turn.clone(),
            paths: request.paths.clone(),
            token_sha256: None,
            expires_unix_ms: request.expires_unix_ms,
            at_unix_ms: now,
            hashes: Vec::new(),
            allow_modified: false,
            undo_turn: None,
            reason: None,
        }
    }
}

/// Parse the ledger, skipping unparseable lines. See [`LedgerEvent`] for why
/// a corrupt tail must not fail the whole read.
fn parse_ledger(text: &str) -> Vec<LedgerEvent> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<LedgerEvent>(l).ok())
        .collect()
}

/// Pure reader, split out so the liveness rule is testable without a
/// filesystem: a reservation-creating event ([`RESERVING_EVENTS`]) is live
/// iff nothing terminal carries its id and its expiry is still in the future.
fn live_from_ledger(text: &str, now: u64) -> Vec<LedgerEvent> {
    let events = parse_ledger(text);
    let released: HashSet<&str> = events
        .iter()
        .filter(|e| TERMINAL_EVENTS.contains(&e.event.as_str()))
        .map(|e| e.id.as_str())
        .collect();
    events
        .iter()
        .filter(|e| RESERVING_EVENTS.contains(&e.event.as_str()))
        .filter(|e| !released.contains(e.id.as_str()))
        .filter(|e| e.expires_unix_ms > now)
        .cloned()
        .collect()
}

// ---- F3: request states ----------------------------------------------------

/// What a lodged request currently is. The parent spec's five, plus
/// `Failed` — see the module header for why `Approved` is recognized here and
/// never written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    Pending,
    Approved,
    Denied,
    Expired,
    Executed,
    Failed,
}

impl RequestState {
    pub fn as_str(self) -> &'static str {
        match self {
            RequestState::Pending => "pending",
            RequestState::Approved => "approved",
            RequestState::Denied => "denied",
            RequestState::Expired => "expired",
            RequestState::Executed => "executed",
            RequestState::Failed => "failed",
        }
    }
}

/// Derive one request's state from every event carrying its id.
///
/// Precedence is by finality, not by file order: `execute` outranks
/// everything (the writes landed and the undo turn exists, whatever else was
/// appended), then the other terminals, then a bare `approve`. Only with no
/// terminal at all does the clock decide, so a request whose window lapsed
/// reads `expired` even before any `expire` event has been appended — the
/// event records the lapse, it does not cause it.
fn state_from_events(request: &LedgerEvent, events: &[LedgerEvent], now: u64) -> RequestState {
    let mine = || events.iter().filter(|e| e.id == request.id);
    if mine().any(|e| e.event == EVENT_EXECUTE) {
        return RequestState::Executed;
    }
    if mine().any(|e| e.event == EVENT_DENY) {
        return RequestState::Denied;
    }
    if mine().any(|e| e.event == EVENT_FAIL) {
        return RequestState::Failed;
    }
    if mine().any(|e| e.event == EVENT_EXPIRE) {
        return RequestState::Expired;
    }
    if mine().any(|e| e.event == EVENT_APPROVE) {
        return RequestState::Approved;
    }
    if request.expires_unix_ms <= now {
        return RequestState::Expired;
    }
    RequestState::Pending
}

/// What `request` returns to the agent and what `status` reports back.
///
/// `requested_unix_ms` is [`LedgerEvent::at_unix_ms`] and `turn` is
/// [`LedgerEvent::turn`] — both were written but unread by F2, and both are
/// what makes a pending row legible ("which turn, lodged when").
#[derive(Debug, Clone, Serialize)]
pub struct PendingStatus {
    /// The request id, which is also `agentrec approve`'s argument.
    pub request: String,
    pub state: RequestState,
    /// The resolved turn this request would revert.
    pub turn: String,
    /// Exactly the paths an approval would write.
    pub paths: Vec<String>,
    pub allow_modified: bool,
    pub requested_unix_ms: u64,
    pub expires_unix_ms: u64,
    /// Present once executed: the undo turn's id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_turn: Option<String>,
    /// Present on a failed request: why (today, `preview_stale`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What `request_human` hands back: the freshly lodged request's status,
/// with the bound preview alongside it so an agent does not have to call
/// `preview` again to learn what a human is being asked to allow.
#[derive(Debug, Clone, Serialize)]
pub struct PendingUndo {
    #[serde(flatten)]
    pub status: PendingStatus,
    pub preview: UndoPreview,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// An exclusive advisory lock on `.agentrec/undo.lock`, released on drop.
///
/// Public since F3: `agentrec approve` holds ONE of these across recheck,
/// working-tree writes, the undo-turn append, and the terminal ledger row,
/// and the undo-turn append lives in the CLI crate. Constructed only through
/// [`UndoCoordinator::lock`].
///
/// `std::fs::File::lock` (stabilized in std; `flock(LOCK_EX)` on unix) rather
/// than `libc` — `agentrec-core` has no `libc` dependency and adding one to a
/// published crate to reach a primitive std already exposes is the wrong
/// trade. The CLI's `loglock.rs`/`memlock.rs` predate the stable API and are
/// deliberately left alone; this is not a second locking convention so much
/// as the same `flock` reached without a dep.
pub struct UndoLock {
    file: std::fs::File,
}

impl UndoLock {
    fn acquire(path: &Path) -> Result<Self, UndoError> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            // Never truncate: this file exists only to be locked.
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| UndoError::Io(format!("cannot open {}: {e}", path.display())))?;
        crate::perms::lock_file(path);
        file.lock()
            .map_err(|e| UndoError::Io(format!("cannot lock {}: {e}", path.display())))?;
        Ok(UndoLock { file })
    }
}

impl Drop for UndoLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Map a [`PlanKind::Refused`] row onto its machine-readable class.
///
/// Re-evaluates the SAME predicates [`build_plan`] used, in the same order,
/// rather than pattern-matching the refusal prose. Matching the text would
/// make the class silently wrong the day a message is reworded, and the
/// record-side symlink trigger interpolates a wire value (`link_kind`) into
/// its reason, so there is no fixed substring to key on there at all.
fn refusal_class(root: &Path, entry: &FileEntry) -> RefusalClass {
    // Order mirrors build_plan's gates: symlink is FIRST there, and its two
    // triggers are independent (record says link / a link is there now).
    if entry.link_kind.is_some() || is_symlink_on_disk(&root.join(&entry.path)) {
        return RefusalClass::Symlink;
    }
    if entry.withheld {
        return RefusalClass::Withheld;
    }
    if entry.skipped {
        return RefusalClass::Skipped;
    }
    RefusalClass::NoSnapshot
}

// ---- moved verbatim from `cli/src/readcmds.rs` (task F3) ------------------
//
// The revert PRIMITIVE, moved for the same reason F2 moved the planner: the
// confirm-mode `agentrec approve` and the CLI's own `undo --confirm` must
// write through one implementation, and a second one behind the approval
// seam is exactly what this module exists to prevent. Bodies are
// byte-for-byte the originals apart from the visibility widening this crate
// boundary requires; `readcmds` re-imports `execute_revert` under its
// original name, so its untouched test module resolves against the moved
// code and its passing is the behavior-preservation proof.

/// Apply one file's revert and return the inverse `FileEntry` for the new
/// undo turn. Snapshots the CURRENT (pre-undo) bytes first — that becomes the
/// inverse entry's `before`, so the undo is itself re-revertible (AC H6).
/// Every write is verified by re-reading and re-hashing before returning Ok;
/// a mismatch is a hard error, never a silent partial revert.
pub fn execute_revert(
    root: &Path,
    store: &BlobStore,
    entry: &FileEntry,
) -> Result<FileEntry, String> {
    let path = root.join(&entry.path);
    let pre_bytes = match std::fs::read(&path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: cannot read before revert: {e}", entry.path)),
    };
    let new_before = match &pre_bytes {
        Some(b) => Some(store.put(b).ok_or_else(|| {
            format!(
                "{}: failed to snapshot current content before revert",
                entry.path
            )
        })?),
        None => None,
    };

    let (new_after, inverse_op) = match entry.op.as_str() {
        "create" => {
            // E1: idempotent — a file already absent (deleted by something
            // else since the turn) means the goal state ("file gone") is
            // already reached; NotFound is success, not an error.
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: failed to delete: {e}", entry.path)),
            }
            if path.exists() {
                return Err(format!(
                    "{}: still present after delete (revert of create)",
                    entry.path
                ));
            }
            (None, "delete")
        }
        "delete" => {
            let restored = restore_from_before(&path, store, entry)?;
            (Some(restored), "create")
        }
        _ => {
            // "modify"
            let restored = restore_from_before(&path, store, entry)?;
            (Some(restored), "modify")
        }
    };

    Ok(FileEntry {
        path: entry.path.clone(),
        before: new_before,
        after: new_after,
        op: inverse_op.to_string(),
        skipped: false,
        withheld: false,
        baseline_unknown: false,
        skipped_reason: None,
        after_synthesized: None,
        link_kind: None,
        attribution: None,
    })
}

/// Write `entry.before`'s blob to `path` (creating parent dirs), then verify
/// by re-reading and re-hashing. Returns the (already-known) `before` hash on
/// success — the content is byte-identical by construction, verified.
pub fn restore_from_before(
    path: &Path,
    store: &BlobStore,
    entry: &FileEntry,
) -> Result<String, String> {
    // F2, second gate. `build_plan::symlink_refusal` already keeps every
    // link-involved entry out of the revert set; this repeats the check at
    // the write primitive itself so no future caller of `restore_from_before`
    // can reach `fs::write` on a link by skipping the planner. The `create`
    // arm's `remove_file` is covered by the planner gate only — `remove_file`
    // unlinks the link rather than following it, so it destroys a link but
    // cannot truncate a file outside the plan.
    if entry.link_kind.is_some() || is_symlink_on_disk(path) {
        return Err(format!(
            "{}: symlink — refusing to restore (writing here would replace the link or \
             truncate its target)",
            entry.path
        ));
    }
    let before_hash = entry
        .before
        .as_deref()
        .ok_or_else(|| format!("{}: no prior snapshot to restore", entry.path))?;
    let bytes = store
        .get(before_hash)
        .map_err(|e| format!("{}: {e}", entry.path))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("{}: failed to create parent dirs: {e}", entry.path))?;
    }
    std::fs::write(path, &bytes).map_err(|e| format!("{}: failed to write: {e}", entry.path))?;
    let readback = std::fs::read(path)
        .map_err(|e| format!("{}: failed to verify after write: {e}", entry.path))?;
    if hash_bytes(&readback) != before_hash {
        return Err(format!(
            "{}: verification failed after restore (byte mismatch)",
            entry.path
        ));
    }
    Ok(before_hash.to_string())
}

// ---- moved verbatim from `cli/src/readcmds.rs` (task F2) ------------------
//
// Bodies are byte-for-byte the originals apart from `fmt::` -> `crate::text::`
// and `view::` -> `crate::view::` path rewrites and the visibility widening
// this crate boundary requires. The gate ORDER inside `build_plan` is
// load-bearing and comment-documented; do not tidy it.

/// Per-file disposition for an undo.
pub struct Plan {
    pub entry: FileEntry,
    pub kind: PlanKind,
}

pub enum PlanKind {
    /// Will be reverted; `warn` names the modified-since cause when included
    /// only because of `--allow-modified`.
    Revert { warn: Option<String> },
    /// Modified since the turn; skipped unless `--allow-modified`.
    Excluded { cause: String },
    /// Never revertible regardless of flags (no content, or none was ever
    /// snapshotted).
    Refused { reason: String },
}

/// Classify every file in `target` (filtered by `files_filter`, when
/// non-empty) into revert / exclude / refuse. Order matches `target.files`.
#[allow(clippy::too_many_arguments)]
pub fn build_plan(
    root: &Path,
    store: &BlobStore,
    target: &TurnRecord,
    target_idx: usize,
    turns: &[&TurnRecord],
    records: &[LogRecord],
    files_filter: &[String],
    allow_modified: bool,
) -> Vec<Plan> {
    let filter_set: Option<HashSet<&str>> = if files_filter.is_empty() {
        None
    } else {
        Some(files_filter.iter().map(|s| s.as_str()).collect())
    };

    let mut plans = Vec::with_capacity(target.files.len());
    for entry in &target.files {
        if let Some(set) = &filter_set {
            if !set.contains(entry.path.as_str()) {
                continue; // deselected by --files, left untouched (AC H2)
            }
        }

        // ROOT CONTAINMENT. This gate is first — above even the symlink
        // refusal — because it is the only one whose failure mode writes to a
        // path OUTSIDE the repository entirely, and because the symlink gate
        // below cannot catch the intermediate-component case (it lstats the
        // final component only). Unconditional w.r.t. `--allow-modified`.
        if let Some(kind) = escape_refusal(root, entry) {
            plans.push(Plan {
                entry: entry.clone(),
                kind,
            });
            continue;
        }
        // Inode containment, immediately after path containment because it is
        // the same harm through channels paths cannot express: a second name
        // for the target's inode (possibly outside the repo) that
        // `fs::write`'s in-place truncate would rewrite too, or a non-regular
        // file that cannot be read or written as file content at all — a FIFO
        // here BLOCKS the process forever.
        if let Some(kind) = inode_refusal(root, entry) {
            plans.push(Plan {
                entry: entry.clone(),
                kind,
            });
            continue;
        }
        // F2 (red team round 2). This gate is above every other refusal
        // except containment, because its failure mode writes to a
        // file that was never in the plan: `std::fs::write` follows a
        // symlink and truncates its target, and the post-write read-back
        // follows it too, so the corruption verifies clean and reports
        // success. It is also unconditional w.r.t. `--allow-modified` — it
        // sits above the modified-since gate below, so that flag never
        // reaches it.
        //
        // TWO INDEPENDENT triggers, each sufficient on its own:
        //   1. the record says the path was a link when it was snapshotted;
        //   2. the path IS a link on disk right now.
        // (1) does not cover records written before `link_kind` existed —
        // they carry no such field and never will, so (2) is the ONLY guard
        // for the entire pre-existing log. (2) does not cover a link that
        // has since been deleted (nothing to lstat), which is exactly the
        // F2a delete-restore case — so (1) is the only guard there. Neither
        // subsumes the other; both stay.
        if let Some(kind) = symlink_refusal(root, entry) {
            plans.push(Plan {
                entry: entry.clone(),
                kind,
            });
            continue;
        }
        if entry.withheld {
            plans.push(Plan {
                entry: entry.clone(),
                kind: PlanKind::Refused {
                    reason: "secret-pattern file, never snapshotted".to_string(),
                },
            });
            continue;
        }
        // SR6: the skipped gate MUST stay above modified-since (below). A
        // skipped entry is refused unconditionally here and `continue`s
        // before `entry.after` is ever compared against the current on-disk
        // hash — otherwise an unmodified skipped file (SR-C now gives it a
        // real `after` hash) could fall through into the revert path and
        // undo would try to restore a blob that was never stored.
        if entry.skipped {
            // SR-D: the wire field is the per-entry authoritative cause —
            // `state.json`'s `io_failed` is a separate, aggregate/operational
            // channel (drives the DEGRADED banner) and is deliberately never
            // consulted here, so the two can't be made to disagree.
            // Finding #5(a): unified on `print_entry`'s em-dash form (was
            // parenthesized here) — same fact, one spelling; D-PD6 is the
            // tracked debt item for exactly this renderer-drift class.
            let reason = format!(
                "content not snapshotted — {}",
                crate::text::skip_reason_text(entry.skipped_reason.as_deref())
            );
            plans.push(Plan {
                entry: entry.clone(),
                kind: PlanKind::Refused { reason },
            });
            continue;
        }
        if entry.op == "modify" || entry.op == "delete" {
            // E1: an integrity READ (store.get), not a bare existence check —
            // build_plan runs entirely before any file mutation, so a corrupt
            // (hash-mismatched) before-blob is caught and refused here, never
            // discovered mid-revert after other files have already changed.
            let refuse_reason = match entry.before.as_deref() {
                None => Some("no prior snapshot to restore".to_string()),
                Some(h) => match store.get(h) {
                    Ok(_) => None,
                    Err(StoreError::Missing(_)) => {
                        Some("prior snapshot unavailable — refusing to restore".to_string())
                    }
                    Err(StoreError::Corrupt(_)) => Some(
                        "prior snapshot corrupt (hash mismatch) — refusing to restore".to_string(),
                    ),
                },
            };
            if let Some(reason) = refuse_reason {
                plans.push(Plan {
                    entry: entry.clone(),
                    kind: PlanKind::Refused { reason },
                });
                continue;
            }
        }

        // modified-since (PROTOCOL §5): current on-disk hash vs. the turn's
        // recorded `after` for this path. `None` on either side means absent.
        let current = read_current_hash(root, &entry.path);
        let is_modified = current.as_deref() != entry.after.as_deref();

        let after_synthesized = entry.after_synthesized == Some(true);

        if is_modified && !allow_modified {
            let cause = modified_cause(
                target_idx,
                turns,
                records,
                target,
                &entry.path,
                after_synthesized,
            );
            plans.push(Plan {
                entry: entry.clone(),
                kind: PlanKind::Excluded { cause },
            });
            continue;
        }

        let warn = is_modified.then(|| {
            modified_cause(
                target_idx,
                turns,
                records,
                target,
                &entry.path,
                after_synthesized,
            )
        });
        plans.push(Plan {
            entry: entry.clone(),
            kind: PlanKind::Revert { warn },
        });
    }
    plans
}

/// On-disk content hash for `rel`, relative to `root`; `None` for an absent
/// file OR any read error — undo's safety gate treats both as "no content to
/// compare", which only ever makes the modified-since check MORE cautious
/// (a spurious `None` looks like a legitimate delete-target, not a bypass).
pub fn read_current_hash(root: &Path, rel: &str) -> Option<String> {
    std::fs::read(root.join(rel)).ok().map(|b| hash_bytes(&b))
}

/// True when `path` is itself a symbolic link. `symlink_metadata` is an
/// lstat: it describes the link, where `metadata`/`Path::exists` would
/// describe (and a write would hit) the pointed-to file. A metadata error —
/// absent path, permission denied — is `false`: this predicate answers only
/// "is there a link here", and the absent case is handled by the record-side
/// trigger instead.
pub fn is_symlink_on_disk(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Root containment: `Some(PlanKind::Refused)` when reverting `entry` could
/// write outside `root`, `None` when the write provably lands inside it.
///
/// `entry.path` is WIRE DATA. Every other gate in `build_plan` treats it as a
/// repo-relative name and asks questions *about* it; this one asks whether it
/// is a repo-relative name at all. Without it, `root.join(&entry.path)` is an
/// arbitrary-write primitive: `Path::join` with an absolute path silently
/// DISCARDS the base, and a `..` component walks straight out. Before Phase F
/// the only caller was a human reading the rendered plan before confirming;
/// `agentrec_undo` in `auto` mode removes that human, which is why this gate
/// exists in the shared planner rather than in either transport.
///
/// TWO CHECKS, and the second is not redundant:
///  1. LEXICAL — reject an absolute path or any `..`/root/prefix component.
///     Catches the plain `../outside/f` and `/etc/passwd` shapes.
///  2. RESOLVED — canonicalize the nearest EXISTING ancestor of the target and
///     require `root`'s canonical form to be a prefix. A path that is
///     lexically innocent (`linkdir/c.txt`) still escapes when `linkdir` is a
///     symlink to somewhere outside; check 1 cannot see that, and
///     [`is_symlink_on_disk`] cannot either — it lstats the FINAL component,
///     while `restore_from_before` runs `create_dir_all(parent)` and writes
///     THROUGH any intermediate link. That shape is strictly worse than the
///     `..` one: the rendered path looks ordinary, so human review does not
///     save the CLI leg either.
///
/// Both `root` and the ancestor are canonicalized before comparison because
/// `root` itself is commonly a symlink (macOS `/tmp` → `/private/tmp`);
/// comparing a canonical child against a non-canonical root would refuse
/// every legitimate revert under such a root. A canonicalize failure on
/// either side refuses — fail closed, since an unresolvable path is exactly
/// the case where containment cannot be established.
fn escape_refusal(root: &Path, entry: &FileEntry) -> Option<PlanKind> {
    const REASON: &str =
        "path escapes the repository root — refusing to write outside the recorded repo";
    let refuse = || {
        Some(PlanKind::Refused {
            reason: REASON.to_string(),
        })
    };

    let rel = Path::new(&entry.path);
    if entry.path.is_empty() {
        return refuse();
    }
    // 1. Lexical. `Prefix` is Windows-only in practice but is an absolute
    //    root there, so it is refused for the same reason as `RootDir`.
    for c in rel.components() {
        match c {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return refuse(),
            Component::Normal(_) | Component::CurDir => {}
        }
    }

    // 2. Resolved. Walk up from the target's parent to the nearest ancestor
    //    that exists on disk — the deeper components may legitimately be
    //    absent (a `create` revert, or a delete-restore into a directory
    //    `restore_from_before` will `create_dir_all`). Canonicalizing the
    //    nearest existing ancestor resolves every link ABOVE it, which is
    //    the whole escape surface: any component that does not exist yet
    //    cannot be a link to anywhere.
    // NOT `.ok()?` — in a `-> Option<PlanKind>` where `None` MEANS ALLOWED,
    // `?` would fail OPEN on an unresolvable root. Refuse explicitly.
    let root_canon = match root.canonicalize() {
        Ok(c) => c,
        Err(_) => return refuse(),
    };
    let target = root.join(rel);
    let mut probe = target.parent();
    while let Some(dir) = probe {
        match dir.canonicalize() {
            Ok(canon) => {
                return if canon.starts_with(&root_canon) {
                    None
                } else {
                    refuse()
                }
            }
            Err(_) => probe = dir.parent(),
        }
    }
    refuse()
}

/// Inode-level containment: `Some(PlanKind::Refused)` when the target is not
/// an ordinary, singly-named regular file, `None` otherwise.
///
/// TWO refusals, because path containment cannot express either one:
///
/// 1. **HARDLINK.** `canonicalize` resolves SYMLINKS; a hardlink is a second
///    directory entry for the same inode, and there is no path-level evidence
///    it exists. A repo-internal `hard.txt` hardlinked to a file outside the
///    repo is lexically ordinary, canonicalizes inside `root`, and is not a
///    symlink — so it passes every check in [`escape_refusal`] and
///    [`symlink_refusal`] — yet `fs::write` truncates the SHARED INODE in
///    place and the outside name sees the new bytes. Proven end-to-end
///    through `agentrec_undo` in `auto` mode (branch review, PR #20), the
///    same confused-deputy shape as the `..` and symlinked-parent escapes:
///    creating the link is a write INSIDE cwd, which a path-based agent
///    sandbox permits, while the victim is outside it.
///    `nlink > 1` is the whole predicate — it does not matter WHERE the other
///    name is, because we cannot know, and a second name inside the repo is
///    equally a file this turn's record does not describe.
///
/// 2. **NOT A REGULAR FILE.** This branch is written as "refuse unless it is
///    a regular file", NEVER as "skip unless it is a regular file". The
///    earlier form guarded the hardlink check on `is_file()` and thereby let
///    every OTHER inode type through untouched — and a FIFO target then
///    HUNG the process: `fs::read` on a fifo blocks until a writer appears,
///    which for the single-threaded stdio MCP loop kills the whole
///    agent-facing surface for that session, and hangs `agentrec undo` for a
///    human identically (branch review re-gate, MAJOR 1; `mkfifo` needs no
///    privileges and creating one is a write inside cwd, so the sandboxed
///    agent precondition is the same as every shape above). It also covers a
///    directory recorded as a `modify` entry, which the plan previously
///    rendered as a performable `revert` and then failed at execution with
///    `Is a directory (os error 21)` — a plan promising an action it cannot
///    take.
///
/// A symlink returns `None` here DELIBERATELY: it is [`symlink_refusal`]'s to
/// refuse, with its own distinct wording, and stealing it would make that
/// gate's tests pass for the wrong reason. An absent path also returns `None`
/// — `create` reverts and delete-restores legitimately target paths that do
/// not exist yet, and there is no inode to judge.
///
/// Unix-only: `nlink` needs [`std::os::unix::fs::MetadataExt`]. On a
/// non-unix target this returns `None` (no refusal) — stated rather than
/// silently implied. The repo targets macOS + Linux (D19).
fn inode_refusal(root: &Path, entry: &FileEntry) -> Option<PlanKind> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // lstat, not stat: a symlink is `symlink_refusal`'s to refuse, and
        // following it here would read the TARGET's link count and type.
        let meta = std::fs::symlink_metadata(root.join(&entry.path)).ok()?;
        let ft = meta.file_type();
        if ft.is_symlink() {
            return None;
        }
        if !ft.is_file() {
            return Some(PlanKind::Refused {
                reason: "path is not a regular file (directory, fifo, socket or device) — \
                         reverting cannot read or write it as file content"
                    .to_string(),
            });
        }
        if meta.nlink() > 1 {
            return Some(PlanKind::Refused {
                reason:
                    "path is a hardlink — its inode has another name, which reverting would also \
                     rewrite"
                        .to_string(),
            });
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = (root, entry);
        None
    }
}

/// The F2 symlink refusal: `Some(PlanKind::Refused)` when `entry` must never
/// be reverted because a link is involved, `None` otherwise.
///
/// The two triggers produce DELIBERATELY DIFFERENT text. They are different
/// facts — "the record says this was a link" vs "there is a link here now" —
/// and only distinct wording lets a reader (or a test) tell which one fired;
/// identical text would let the legacy-record path pass a test for the wrong
/// reason.
///
/// `entry.link_kind` is matched on `is_some()`, never against the known
/// value: an unrecognized future kind is still not an ordinary file, so
/// refusing to act on it is the correct degradation (PROTOCOL §5,
/// refuse-to-act-not-refuse-to-parse). Never make this an equality test
/// against [`link_kind::SYMLINK`].
fn symlink_refusal(root: &Path, entry: &FileEntry) -> Option<PlanKind> {
    if let Some(kind) = entry.link_kind.as_deref() {
        // `link_kind` is wire data on an OPEN enum — a foreign producer can
        // put any bytes here, and this string reaches the pre-confirm plan
        // the user reads (F8's exact surface). Sanitized at the one
        // interpolation site so `render_plan`'s every-reason-is-safe
        // invariant holds by construction.
        let kind = crate::text::sanitize_terminal(kind);
        return Some(PlanKind::Refused {
            reason: format!(
                "recorded as a {kind} — its snapshot is the link target, not file content"
            ),
        });
    }
    if is_symlink_on_disk(&root.join(&entry.path)) {
        return Some(PlanKind::Refused {
            reason: "path is a symlink on disk — reverting would write through the link"
                .to_string(),
        });
    }
    None
}

/// Best-effort explanation for why a path is modified-since the target turn:
/// a later rich turn touching the same path outranks an uncovered recording
/// gap, which outranks the default "some edit we can't otherwise explain".
fn modified_cause(
    target_idx: usize,
    turns: &[&TurnRecord],
    records: &[LogRecord],
    target: &TurnRecord,
    path: &str,
    after_synthesized: bool,
) -> String {
    // D1 (P2 fix round, founder decision 2): a synthesized `after` is a
    // DERIVED value, not an observation of what the file actually looked
    // like post-edit — comparing the real on-disk hash against it and
    // reporting a mismatch as "human or external edit" would fabricate
    // attribution nobody earned. This must win over both signals below: a
    // later rich turn or a recording gap are real facts about *observed*
    // history, but neither makes an unobserved comparison point trustworthy.
    if after_synthesized {
        return "imported turn's after-state was derived (not observed) — cannot attribute this difference".to_string();
    }
    let later_touches = turns[target_idx + 1..]
        .iter()
        .any(|t| t.grade == "rich" && t.files.iter().any(|f| f.path == path));
    if later_touches {
        return "later agent turn".to_string();
    }
    if crate::view::has_gap_after(records, &target.ended) {
        return "recording gap".to_string();
    }
    "human or external edit".to_string()
}

/// D6 honesty line, WITHOUT the renderer's two-space indent (task F2
/// moved this into core so the MCP preview and the CLI preview carry one
/// caution string, not two that can drift; `readcmds::render_plan` supplies
/// the indent it used to bake in here). A rich turn's file list is an *activity window*, not an
/// authorship record: while a bracket is open, every mutation in the root is
/// folded into that one turn (D6, "one open turn per root"), so a human edit
/// landing during the agent's bracket becomes one of the turn's files and the
/// recorded `after` hash for it IS the human's own content. That makes the
/// D30 modified-since rail structurally unable to fire for such a file — it
/// is not modified-since, it is *mis-attributed*, and no post-hoc heuristic
/// can separate the two. So undo states the limitation rather than guessing:
/// a warning that is always true beats a detector that is sometimes a lie.
/// "Always true" is load-bearing and was once violated: the sentence claimed
/// "every file listed above is reverted", which is false on a MIXED plan where
/// an `EXCLUDE`/`REFUSE` line is also listed. It now names only the files
/// marked `revert`, the one set that is reverted under every flag combination
/// (`--allow-modified` moves a file INTO that set, never out of it).
///
/// Deliberately NOT prefixed `WARNING:` — that token is already the per-file
/// modified-since marker above, and conflating the two would make each one
/// unreadable as evidence of the other. Turns with `tool: "agentrec"` are the
/// one rich shape excluded: their file list is built from a revert plan (what
/// this process itself wrote), not from a watch window. `tool: "git"` turns
/// are deliberately INCLUDED — a checkout burst is a watch window like any
/// other — which is why the wording says "the recorded tool's own writes"
/// rather than "the agent's": the sentence has to stay true for every turn
/// class the gate admits.
///
/// BARE turns get their own sentence (D49, founder decision 2026-08-01; this
/// was an open residual until then). They are cautioned — the writes are as
/// real and as irreversible-by-preview as a rich turn's — but not with the
/// rich text, which names "the recorded tool" and D6: a bare turn has no
/// recorded tool, so that sentence would fabricate the attribution the grade
/// exists to withhold. "No recorded tool" is an invariant, not an observation:
/// every bare close runs through `Source::Quiet`, whose `OpenTurn` is built
/// `tool: None` (agentrec-core `engine.rs`), and crash recovery hard-codes
/// `None` for a bare grade (`daemon.rs`) — a producer minting bare-with-tool
/// would make this sentence false and must change it. The gate stays on
/// `grade` alone (founder-specified), so that invariant is documented here
/// rather than defensively re-checked at the call site.
pub fn window_caution(target: &TurnRecord, plans: &[Plan]) -> Option<String> {
    let rich = target.grade == "rich" && target.tool.as_deref() != Some("agentrec");
    let bare = target.grade == "bare";
    if !rich && !bare {
        return None;
    }
    if !plans
        .iter()
        .any(|p| matches!(p.kind, PlanKind::Revert { .. }))
    {
        return None; // nothing will be written; no scope to caution about
    }
    // One branch or the other, never a concatenation: that is what makes
    // "the variants do not bleed" structural rather than test-enforced in
    // both directions (only the bare-shows-no-rich-text direction is
    // asserted; the reverse is closed here).
    if bare {
        return Some(
            "CAUTION: this is a bare turn — an unattributed activity window with no recorded \
             tool; agentrec cannot say who or what made these writes, and every file marked \
             `revert` above is reverted regardless of who or what wrote it. Review the list \
             before confirming."
                .to_string(),
        );
    }
    Some(
        "CAUTION: this turn's file list is an activity window, not an authorship record — \
         agentrec cannot distinguish the recorded tool's own writes from concurrent human \
         edits made in the same window (D6), and every file marked `revert` above is \
         reverted regardless of who wrote it. Review the list before confirming."
            .to_string(),
    )
}

#[cfg(test)]
mod coordinator_tests {
    use super::*;
    use crate::record::skip_reason;
    use std::collections::BTreeMap;

    fn entry(path: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            before: None,
            after: None,
            op: "modify".to_string(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
            link_kind: None,
            attribution: None,
        }
    }

    struct Fx {
        tmp: tempfile::TempDir,
    }

    impl Fx {
        fn new() -> Fx {
            let tmp = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(tmp.path().join(".agentrec").join("objects")).unwrap();
            Fx { tmp }
        }
        fn root(&self) -> &Path {
            self.tmp.path()
        }
        fn store(&self) -> BlobStore {
            BlobStore::new(self.root().join(".agentrec").join("objects"))
        }
        fn coord(&self) -> UndoCoordinator {
            UndoCoordinator::new(self.root())
        }
        /// A file that is cleanly revertible: `before` blob stored, on-disk
        /// bytes hash-equal to `after`.
        fn revertible(&self, path: &str) -> FileEntry {
            let before = self.store().put(format!("old {path}").as_bytes()).unwrap();
            let now = format!("new {path}");
            let full = self.root().join(path);
            if let Some(p) = full.parent() {
                std::fs::create_dir_all(p).unwrap();
            }
            std::fs::write(&full, now.as_bytes()).unwrap();
            let mut e = entry(path);
            e.before = Some(before);
            e.after = Some(hash_bytes(now.as_bytes()));
            e
        }
        fn write_turn(&self, id: &str, imported: bool, files: Vec<FileEntry>) -> String {
            let t = TurnRecord {
                v: 1,
                id: id.to_string(),
                grade: "rich".to_string(),
                truncated: false,
                started: "2026-01-01T00:00:00.000Z".into(),
                ended: "2026-01-01T00:00:01.000Z".into(),
                tool: Some("claude".into()),
                model: None,
                session: None,
                root: self.root().display().to_string(),
                prompt_ref: None,
                prompt_excerpt: None,
                merges: vec![],
                imported: imported.then_some(true),
                files_complete: None,
                origin: None,
                files,
            };
            let line = serde_json::to_string(&LogRecord::Turn(t)).unwrap();
            let p = self.root().join(".agentrec").join("log.jsonl");
            let prev = std::fs::read_to_string(&p).unwrap_or_default();
            std::fs::write(&p, format!("{prev}{line}\n")).unwrap();
            id.to_string()
        }
        /// path -> (len, mtime, content hash) for every file under `dir`.
        fn sweep(&self, dir: &Path) -> BTreeMap<String, String> {
            let mut out = BTreeMap::new();
            fn walk(d: &Path, base: &Path, out: &mut BTreeMap<String, String>) {
                let Ok(rd) = std::fs::read_dir(d) else { return };
                for e in rd.flatten() {
                    let p = e.path();
                    let md = std::fs::symlink_metadata(&p).unwrap();
                    let rel = p.strip_prefix(base).unwrap().display().to_string();
                    if md.is_dir() {
                        walk(&p, base, out);
                    } else if md.file_type().is_symlink() {
                        out.insert(
                            rel,
                            format!("link:{}", std::fs::read_link(&p).unwrap().display()),
                        );
                    } else {
                        let bytes = std::fs::read(&p).unwrap_or_default();
                        out.insert(
                            rel,
                            format!(
                                "{}:{:?}:{}",
                                bytes.len(),
                                md.modified().ok(),
                                hash_bytes(&bytes)
                            ),
                        );
                    }
                }
            }
            walk(dir, self.root(), &mut out);
            out
        }
        fn worktree_sweep(&self) -> BTreeMap<String, String> {
            let mut all = self.sweep(self.root());
            all.retain(|k, _| !k.starts_with(".agentrec"));
            all
        }
        fn agentrec_sweep(&self) -> BTreeMap<String, String> {
            self.sweep(&self.root().join(".agentrec"))
        }
    }

    fn req(turn: &str) -> UndoRequest {
        UndoRequest {
            turn: turn.to_string(),
            paths: None,
            allow_modified: false,
        }
    }

    // AC-F2 (1): preview never writes. Confirm mode is the absolute case —
    // NOTHING changes anywhere, `.agentrec/` included.
    #[test]
    fn confirm_preview_changes_no_byte_anywhere() {
        let fx = Fx::new();
        let e = fx.revertible("src/a.rs");
        let id = fx.write_turn("t_AAAA0000000000000000AAAA", false, vec![e]);
        let before_wt = fx.worktree_sweep();
        let before_ar = fx.agentrec_sweep();
        let p = fx
            .coord()
            .preview(req(&id), McpDestructive::Confirm)
            .unwrap();
        assert_eq!(p.files.len(), 1);
        assert_eq!(fx.worktree_sweep(), before_wt);
        assert_eq!(fx.agentrec_sweep(), before_ar);
    }

    // AC-F2 (1), auto half. A reservation IS a write, so the honest
    // assertion is scoped: the working tree is untouched, and `.agentrec/`
    // gains exactly the ledger and the lock file — nothing else moves.
    #[test]
    fn auto_preview_writes_only_the_ledger_and_lock_never_the_worktree() {
        let fx = Fx::new();
        let e = fx.revertible("src/a.rs");
        let id = fx.write_turn("t_AAAA0000000000000000AAAA", false, vec![e]);
        let before_wt = fx.worktree_sweep();
        let before_ar = fx.agentrec_sweep();
        fx.coord().preview(req(&id), McpDestructive::Auto).unwrap();
        assert_eq!(
            fx.worktree_sweep(),
            before_wt,
            "auto preview wrote the worktree"
        );
        let after_ar = fx.agentrec_sweep();
        let added: Vec<&String> = after_ar
            .keys()
            .filter(|k| !before_ar.contains_key(*k))
            .collect();
        assert_eq!(
            added,
            vec![".agentrec/undo-requests.jsonl", ".agentrec/undo.lock"],
            "auto preview touched more than the reservation ledger + lock"
        );
        for (k, v) in &before_ar {
            assert_eq!(
                after_ar.get(k),
                Some(v),
                "pre-existing .agentrec file {k} changed"
            );
        }
    }

    // AC-F2 (2): every refusal class, one fixture each.
    #[test]
    fn each_refusal_class_has_a_fixture() {
        let fx = Fx::new();
        let mut withheld = entry("secrets/.env");
        withheld.withheld = true;
        let mut skipped = entry("big.bin");
        skipped.skipped = true;
        skipped.skipped_reason = Some(skip_reason::OVER_CAP.to_string());
        let mut linked = entry("link.rs");
        linked.link_kind = Some("symlink".to_string());
        // The SECOND, independent trigger: no `link_kind` on the record (as
        // every pre-`link_kind` log line looks) but a link at the path right
        // now. Without its own fixture, `refusal_class` could drop the
        // `is_symlink_on_disk` call and every other assertion here would
        // still pass — and that call is the sole guard for the entire
        // pre-existing log.
        let legacy_link = entry("legacy-link.rs");
        std::fs::write(fx.root().join("link-target.rs"), b"target").unwrap();
        std::os::unix::fs::symlink(
            fx.root().join("link-target.rs"),
            fx.root().join("legacy-link.rs"),
        )
        .unwrap();
        let no_snapshot = entry("gone.rs"); // op modify, before None
        let modified = {
            let mut e = fx.revertible("drifted.rs");
            std::fs::write(fx.root().join("drifted.rs"), b"a human edited this").unwrap();
            e.op = "modify".into();
            e
        };
        let id = fx.write_turn(
            "t_BBBB0000000000000000BBBB",
            false,
            vec![
                withheld,
                skipped,
                linked,
                legacy_link,
                no_snapshot,
                modified,
            ],
        );
        let p = fx
            .coord()
            .preview(req(&id), McpDestructive::Confirm)
            .unwrap();
        let got: BTreeMap<&str, RefusalClass> = p
            .refusals
            .iter()
            .map(|r| (r.path.as_str(), r.class))
            .collect();
        assert_eq!(got.get("secrets/.env"), Some(&RefusalClass::Withheld));
        assert_eq!(got.get("big.bin"), Some(&RefusalClass::Skipped));
        assert_eq!(got.get("link.rs"), Some(&RefusalClass::Symlink));
        assert_eq!(got.get("legacy-link.rs"), Some(&RefusalClass::Symlink));
        assert_eq!(got.get("gone.rs"), Some(&RefusalClass::NoSnapshot));
        assert_eq!(got.get("drifted.rs"), Some(&RefusalClass::ModifiedSince));
        assert!(p.files.is_empty(), "no file in this fixture is executable");
    }

    // AC-F2 (2), K2. Enumerated per file AND whole-turn in effect: the
    // executable set is empty even though a perfectly revertible entry sits
    // in the same turn, so no partial revert is ever on offer (AC3).
    #[test]
    fn imported_unreconstructible_empties_the_executable_set() {
        let fx = Fx::new();
        let good = fx.revertible("ok.rs");
        let mut bad = entry("lost.rs");
        bad.op = "modify".into();
        bad.before = None;
        let id = fx.write_turn("t_CCCC0000000000000000CCCC", true, vec![good, bad]);
        let p = fx.coord().preview(req(&id), McpDestructive::Auto).unwrap();
        assert_eq!(p.refusals.len(), 1);
        assert_eq!(p.refusals[0].class, RefusalClass::ImportedUnreconstructible);
        assert!(p.files.is_empty(), "K2 must leave nothing executable");
        assert!(
            p.token.is_none(),
            "no token may be issued for a refused turn"
        );
        assert!(fx.coord().live_reservations().unwrap().is_empty());
    }

    // AC-F2 (3): a subset reserves only the subset; a disjoint subset of the
    // SAME turn still succeeds; an overlapping one is `undo_conflict`.
    #[test]
    fn path_subset_reserves_only_the_subset() {
        let fx = Fx::new();
        let a = fx.revertible("a.rs");
        let b = fx.revertible("b.rs");
        let c = fx.revertible("c.rs");
        let id = fx.write_turn("t_DDDD0000000000000000DDDD", false, vec![a, b, c]);
        let co = fx.coord();

        let first = co
            .preview(
                UndoRequest {
                    turn: id.clone(),
                    paths: Some(vec![PathBuf::from("a.rs")]),
                    allow_modified: false,
                },
                McpDestructive::Auto,
            )
            .unwrap();
        assert_eq!(
            first
                .files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.rs"]
        );
        let live = co.live_reservations().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].paths, vec!["a.rs".to_string()]);

        // Disjoint subset of the same turn: succeeds.
        co.preview(
            UndoRequest {
                turn: id.clone(),
                paths: Some(vec![PathBuf::from("b.rs")]),
                allow_modified: false,
            },
            McpDestructive::Auto,
        )
        .expect("a disjoint subset of the same turn must not conflict");

        // Overlapping: refused at creation (P6).
        let err = co
            .preview(
                UndoRequest {
                    turn: id.clone(),
                    paths: Some(vec![PathBuf::from("b.rs"), PathBuf::from("c.rs")]),
                    allow_modified: false,
                },
                McpDestructive::Auto,
            )
            .expect_err("an overlapping reservation must be refused");
        assert_eq!(err.code(), "undo_conflict");
        assert!(err.to_string().contains("b.rs"), "{err}");
        // The refused request reserved nothing — c.rs stays free.
        assert_eq!(co.live_reservations().unwrap().len(), 2);
    }

    // AC-F2 (4a): a token exists in auto mode and only in auto mode.
    #[test]
    fn token_is_issued_in_auto_only() {
        let fx = Fx::new();
        let e = fx.revertible("a.rs");
        let id = fx.write_turn("t_EEEE0000000000000000EEEE", false, vec![e]);
        let confirm = fx
            .coord()
            .preview(req(&id), McpDestructive::Confirm)
            .unwrap();
        assert!(confirm.token.is_none());
        assert!(confirm.reservation.is_none());
        assert!(
            !fx.coord().ledger_path().exists(),
            "confirm mode must not create the reservation ledger"
        );
        let auto = fx.coord().preview(req(&id), McpDestructive::Auto).unwrap();
        assert!(auto.token.is_some());
        let ttl = auto.token_expires_unix_ms.unwrap() - now_ms();
        assert!(
            ttl > TOKEN_TTL_MS - 5_000 && ttl <= TOKEN_TTL_MS,
            "ttl {ttl}ms"
        );
    }

    // AC-F2 (4b): what is STORED is the hash. The discriminating assertion
    // is that the raw token does not occur anywhere in the ledger bytes —
    // "a hash-shaped field exists" would still pass if the raw value were
    // written beside it.
    #[test]
    fn the_ledger_stores_the_token_hash_and_never_the_token() {
        let fx = Fx::new();
        let e = fx.revertible("a.rs");
        let id = fx.write_turn("t_FFFF0000000000000000FFFF", false, vec![e]);
        let p = fx.coord().preview(req(&id), McpDestructive::Auto).unwrap();
        let token = p.token.unwrap();
        assert_eq!(token.len(), 40, "160 bits of hex");
        let bytes = std::fs::read_to_string(fx.coord().ledger_path()).unwrap();
        assert!(
            !bytes.contains(&token),
            "the raw token appears in the ledger: {bytes}"
        );
        assert!(
            bytes.contains(&hash_bytes(token.as_bytes())),
            "the token's sha256 is not in the ledger: {bytes}"
        );
    }

    // Parent :641-697 rail: auto excludes modified-since unconditionally, and
    // the COORDINATOR applies it — not only the MCP router.
    #[test]
    fn auto_mode_ignores_a_requested_allow_modified() {
        let fx = Fx::new();
        let e = fx.revertible("drift.rs");
        std::fs::write(fx.root().join("drift.rs"), b"changed underneath").unwrap();
        let id = fx.write_turn("t_GGGG0000000000000000GGGG", false, vec![e]);
        let auto = fx
            .coord()
            .preview(
                UndoRequest {
                    turn: id.clone(),
                    paths: None,
                    allow_modified: true,
                },
                McpDestructive::Auto,
            )
            .unwrap();
        assert!(!auto.allow_modified_effective);
        assert!(auto.files.is_empty());
        assert_eq!(auto.refusals[0].class, RefusalClass::ModifiedSince);
        assert!(
            auto.token.is_none(),
            "nothing executable, so nothing to grant"
        );
        // Confirm mode honors it — proving the exclusion above is the rail,
        // not an inert flag.
        let confirm = fx
            .coord()
            .preview(
                UndoRequest {
                    turn: id,
                    paths: None,
                    allow_modified: true,
                },
                McpDestructive::Confirm,
            )
            .unwrap();
        assert!(confirm.allow_modified_effective);
        assert_eq!(confirm.files.len(), 1);
    }

    #[test]
    fn off_mode_is_refused_by_the_coordinator_itself() {
        let fx = Fx::new();
        let e = fx.revertible("a.rs");
        let id = fx.write_turn("t_HHHH0000000000000000HHHH", false, vec![e]);
        let err = fx
            .coord()
            .preview(req(&id), McpDestructive::Off)
            .expect_err("off must refuse");
        assert_eq!(err.code(), "mode_off");
    }

    #[test]
    fn an_unknown_path_is_refused_not_silently_previewed_as_empty() {
        let fx = Fx::new();
        let e = fx.revertible("a.rs");
        let id = fx.write_turn("t_IIII0000000000000000IIII", false, vec![e]);
        let err = fx
            .coord()
            .preview(
                UndoRequest {
                    turn: id,
                    paths: Some(vec![PathBuf::from("typo.rs")]),
                    allow_modified: false,
                },
                McpDestructive::Confirm,
            )
            .expect_err("a path not in the turn is a caller error");
        assert_eq!(err.code(), "unknown_path");
    }

    #[test]
    fn an_expired_reservation_is_not_live_and_a_terminal_event_releases_one() {
        let mk = |id: &str, event: &str, expires: u64| {
            serde_json::to_string(&LedgerEvent {
                v: LEDGER_V,
                event: event.to_string(),
                id: id.to_string(),
                turn: "t_X".into(),
                paths: vec!["a.rs".into()],
                token_sha256: None,
                expires_unix_ms: expires,
                at_unix_ms: 0,
                hashes: Vec::new(),
                allow_modified: false,
                undo_turn: None,
                reason: None,
            })
            .unwrap()
        };
        let text = format!(
            "{}\n{}\n{}\n{}\nnot json at all\n",
            mk("r1", EVENT_RESERVE, 10_000), // live
            mk("r2", EVENT_RESERVE, 1_000),  // expired
            mk("r3", EVENT_RESERVE, 10_000), // released below
            mk("r3", "execute", 10_000),
        );
        let live = live_from_ledger(&text, 5_000);
        assert_eq!(
            live.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["r1"]
        );
    }

    // ---- F4 ---------------------------------------------------------------

    /// The refusal code out of a claim attempt. `unwrap_err` is unavailable
    /// here — `Claim` is not `Debug`, deliberately (it carries a whole turn
    /// record) — and this reads better than a `matches!` at each site.
    fn refusal(r: Result<Claim, UndoError>) -> &'static str {
        match r {
            Ok(_) => panic!("expected a refusal, got a claim"),
            Err(e) => e.code(),
        }
    }

    /// The reservation records the drift baseline P4 rechecks against, and
    /// the RAW token appears nowhere on disk — only its sha256. The ledger is
    /// 0600 but a credential stored in a file readable by anyone who can read
    /// `.agentrec/` would make the token pointless.
    #[test]
    fn a_reservation_stores_the_hash_and_the_baseline_never_the_raw_token() {
        let fx = Fx::new();
        let e = fx.revertible("src/a.rs");
        let id = fx.write_turn("t_AAAA0000000000000000AAAA", false, vec![e]);
        let p = fx.coord().preview(req(&id), McpDestructive::Auto).unwrap();
        let token = p.token.expect("auto issues a token");

        let raw = std::fs::read_to_string(fx.root().join(".agentrec/undo-requests.jsonl")).unwrap();
        assert!(
            !raw.contains(&token),
            "the raw token must never be persisted"
        );
        let rows = fx.coord().events().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event, EVENT_RESERVE);
        assert_eq!(rows[0].token_sha256, Some(hash_bytes(token.as_bytes())));
        assert_eq!(
            rows[0].hashes,
            vec![read_current_hash(fx.root(), "src/a.rs")],
            "the reserve row must carry the per-path drift baseline"
        );
        assert_eq!(
            rows[0].expires_unix_ms - rows[0].at_unix_ms,
            TOKEN_TTL_MS,
            "the granted window is the spec's 60s"
        );
    }

    /// A `reserve` row written by a PRE-F4 binary carries no `hashes`. It
    /// reads as total drift and refuses — stated as what happens, not as a
    /// case that cannot arise: the ledger is append-only and an older
    /// agentrec may have written into it.
    #[test]
    fn a_reserve_row_without_a_baseline_refuses_rather_than_executing() {
        let fx = Fx::new();
        let e = fx.revertible("src/a.rs");
        let id = fx.write_turn("t_AAAA0000000000000000AAAA", false, vec![e]);
        let p = fx.coord().preview(req(&id), McpDestructive::Auto).unwrap();
        let token = p.token.unwrap();

        // Rewrite the row the way F2 wrote it: no `hashes` at all.
        let path = fx.root().join(".agentrec/undo-requests.jsonl");
        let mut row: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(&path).unwrap().trim()).unwrap();
        row.as_object_mut().unwrap().remove("hashes");
        std::fs::write(&path, format!("{row}\n")).unwrap();

        let co = fx.coord();
        let lock = co.lock().unwrap();
        assert_eq!(
            refusal(co.claim_token(&lock, &token)),
            "preview_stale",
            "a baseline-less grant must not execute"
        );
    }

    /// The three token refusals are three distinct codes, and none of them is
    /// reachable by a caller that merely guessed a well-formed string.
    #[test]
    fn bad_spent_and_expired_tokens_are_three_different_answers() {
        let fx = Fx::new();
        let e = fx.revertible("src/a.rs");
        let id = fx.write_turn("t_AAAA0000000000000000AAAA", false, vec![e]);
        let token = fx
            .coord()
            .preview(req(&id), McpDestructive::Auto)
            .unwrap()
            .token
            .unwrap();
        let co = fx.coord();

        let lock = co.lock().unwrap();
        assert_eq!(refusal(co.claim_token(&lock, "deadbeef")), "bad_token");
        // Spend it WITHOUT executing — the crash-between-consume-and-writes
        // state, in its smallest form.
        let reservation = co.events().unwrap().into_iter().next().unwrap();
        co.consume_token(&lock, &reservation).unwrap();
        assert_eq!(refusal(co.claim_token(&lock, &token)), "token_consumed");
        drop(lock);

        // A second, independent reservation, expired rather than spent.
        let fx2 = Fx::new();
        let e2 = fx2.revertible("src/a.rs");
        let id2 = fx2.write_turn("t_BBBB0000000000000000BBBB", false, vec![e2]);
        let token2 = fx2
            .coord()
            .preview(req(&id2), McpDestructive::Auto)
            .unwrap()
            .token
            .unwrap();
        let path = fx2.root().join(".agentrec/undo-requests.jsonl");
        let mut row: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(&path).unwrap().trim()).unwrap();
        row["expires_unix_ms"] = serde_json::json!(1_000u64);
        std::fs::write(&path, format!("{row}\n")).unwrap();
        let co2 = fx2.coord();
        let lock2 = co2.lock().unwrap();
        assert_eq!(refusal(co2.claim_token(&lock2, &token2)), "token_expired");
    }
}
