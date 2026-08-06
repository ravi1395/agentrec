//! `agentrec approve [<id>]` / `agentrec deny <id>` — the human half of
//! confirm-mode agent undo (task F3; D23, parent spec :286-318 and :641-697).
//!
//! An agent in `confirm` mode cannot revert anything. It lodges a request
//! through `agentrec_undo`'s `request` sub-action and then only polls
//! `status`; the write happens here, in a process a human started, or it does
//! not happen at all.
//!
//! ## What this module does NOT own
//!
//! Every destructive decision — is the request still pending, has the working
//! tree drifted, which files are executable — belongs to
//! `agentrec_core::undo_coordinator`, the seam the MCP transport shares. The
//! revert itself runs through that crate's `execute_revert`, the same
//! primitive `undo --confirm` uses, and the plan is rendered by
//! [`crate::readcmds::render_plan`], the same renderer, so D49's caution
//! reaches an approver exactly as it reaches someone typing `undo`. This
//! module is the orchestration the CLI crate has to own because two pieces
//! live here: the `log.jsonl` writer (`loglock`) and the H7 recorder guard.
//!
//! ## The lock spans everything, including the parts core cannot see
//!
//! Parent :286-318: execution "takes an OS-backed exclusive `.agentrec/
//! undo.lock` before claiming a preview, then holds it across hash recheck,
//! all working-tree writes, undo-turn append/fsync, and terminal request-state
//! append". That is one critical section straddling both crates, which is why
//! `UndoCoordinator::lock` hands back a guard instead of taking the lock
//! internally per call. Lock ORDER is `undo.lock` then `log.lock`, never the
//! reverse — nothing else in the tree takes `log.lock` first.
//!
//! ## `mcp_destructive` gates the agent, not the human
//!
//! Neither verb reads `mcp_destructive`, deliberately. The mode governs what
//! an AGENT may ask for — whether `agentrec_undo` is listed at all, and which
//! sub-actions it may call — and a lodged request is a question already put to
//! a human. Flipping the config to `off` stops new requests being lodged; it
//! does not retroactively strip a human of the ability to answer one already
//! in front of them, any more than it gates `undo --confirm`, which is
//! likewise ungated. The reservation a pending request holds would otherwise
//! sit unreleasable until its TTL lapsed.
//!
//! ## Why the terminal row is appended last
//!
//! `execute` goes into the ledger only after the undo turn is in `log.jsonl`
//! and fsynced. A crash anywhere earlier leaves the request pending, which is
//! honest — nothing was approved — and a crash after leaves it executed with
//! its turn already on disk. There is no instant at which the ledger claims
//! an approval that has no evidence, which is what "restart cannot convert an
//! unapproved request into an approval" demands.

use std::path::Path;

use agentrec_core::undo_coordinator::{
    Claim, PendingStatus, PlanKind, UndoCoordinator, UndoLock, EVENT_EXECUTE, EVENT_FAIL,
};

use crate::fmt;
use crate::readcmds;

/// `agentrec approve [<id>]`. With no id this lists what is awaiting a
/// decision (D23: "`agentrec approve` lists pending undo requests"); with one
/// it claims, rechecks, and executes.
pub fn approve(root: &Path, request: Option<&str>) -> Result<(), String> {
    let Some(request_ref) = request else {
        return list_pending(root);
    };

    let co = UndoCoordinator::new(root);
    let lock = co.lock().map_err(|e| e.to_string())?;

    // Claim: pending-ness, expiry and drift are all decided in core, under
    // this lock, and every refusal it returns has already written whatever
    // terminal row it owed. Nothing has been written to the working tree yet.
    let Claim {
        request,
        target,
        plans,
    } = co.claim(&lock, request_ref).map_err(|e| e.to_string())?;

    // The same bytes `undo` prints before `--confirm`, D49 caution included.
    readcmds::print_plan(&target, &plans);

    let claim = Claim {
        request,
        target,
        plans,
    };
    let executed = execute_claim(root, &co, lock, &claim)?;

    println!(
        "approved request {}; reverted {} file(s); recorded as turn {}",
        claim.request.id,
        executed.reverted,
        fmt::short_id(&executed.undo_turn)
    );
    Ok(())
}

/// What a completed execution produced, for whichever transport asked for it.
///
/// Deliberately NOT `Serialize`: the MCP `execute` payload is not this struct
/// — it adds the reservation and target-turn ids — and a derive here would
/// advertise a wire contract that is not the wire contract.
pub struct Executed {
    /// The id of the undo turn appended to `log.jsonl` (P5).
    pub undo_turn: String,
    pub reverted: usize,
    /// The paths actually written, in plan order.
    pub files: Vec<String>,
}

/// Apply a claimed grant: guard, revert every file, append the undo turn, and
/// resolve the grant with its terminal ledger row.
///
/// **Shared by both flows that may execute an undo** — `agentrec approve`
/// (F3, a human's decision) and the MCP `execute` sub-action (F4, an
/// auto-mode token) — because everything from here down is identical, and a
/// second copy behind the token seam is exactly the drift the coordinator
/// exists to prevent. What differs between the two is only how the grant was
/// authorized, and that is entirely inside `UndoCoordinator::claim` /
/// `claim_token`. This function prints nothing: the MCP transport owns
/// stdout.
///
/// Takes the lock BY VALUE, because where it is released is part of the
/// contract: after the terminal row, before [`readcmds::finish_undo_guard`]'s
/// linger on the success path — the ordering `approve` already had, preserved
/// exactly.
pub fn execute_claim(
    root: &Path,
    co: &UndoCoordinator,
    lock: UndoLock,
    claim: &Claim,
) -> Result<Executed, String> {
    let Claim {
        request,
        target,
        plans,
    } = claim;

    let revertible: Vec<_> = plans
        .iter()
        .filter(|p| matches!(p.kind, PlanKind::Revert { .. }))
        .collect();
    if revertible.is_empty() {
        // Defensive only: both claim paths refuse any grant whose executable
        // set no longer equals the granted one, and the empty set is the
        // extreme case of that, so this is unreachable. It writes NO terminal
        // row — the claim already appended one for every divergence it
        // refused, and a second would be a duplicate resolution.
        return Err(format!(
            "undo request {} no longer has anything to revert — nothing was written",
            request.id
        ));
    }

    // H7/E8, exactly as `undo --confirm` does it. Without the guard a running
    // recorder attributes these writes to a bare turn — an unattributed
    // activity window over changes agentrec itself made.
    //
    // The auto path checks this BEFORE spending the token, so a collision
    // here does not burn a token the agent could legitimately re-present
    // seconds later; this second check is the one `approve` needs and is
    // redundant for that caller.
    if let Some(reason) = readcmds::live_undo_guard_reason(root) {
        return Err(reason);
    }
    let guarded: Vec<String> = revertible.iter().map(|p| p.entry.path.clone()).collect();
    readcmds::write_undo_guard(root, &guarded)?;

    let short_target = fmt::short_id(&target.id);
    let mut inverse = Vec::with_capacity(revertible.len());
    let mut failure: Option<String> = None;
    for plan in &revertible {
        match co.revert_file(&plan.entry) {
            Ok(entry) => inverse.push(entry),
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }

    if let Some(e) = failure {
        // Anything already reverted is a real write and must not go
        // unrecorded, so it gets an honestly-truncated undo turn before the
        // error surfaces — the E1 belt-and-braces `undo --confirm` applies.
        if !inverse.is_empty() {
            let _ = readcmds::append_undo_turn(root, &short_target, inverse, true);
        }
        let _ = co.append_terminal(&lock, request, EVENT_FAIL, None, Some(e.clone()));
        readcmds::finish_undo_guard(root);
        return Err(e);
    }

    let reverted = inverse.len();
    let undo_turn = readcmds::append_undo_turn(root, &short_target, inverse, false)?;
    co.append_terminal(&lock, request, EVENT_EXECUTE, Some(undo_turn.clone()), None)
        .map_err(|e| e.to_string())?;
    drop(lock);

    readcmds::finish_undo_guard(root);

    Ok(Executed {
        undo_turn,
        reverted,
        files: guarded,
    })
}

/// `agentrec deny <id>`: record the refusal. The worktree is not touched,
/// and the reservation the request held is released immediately.
pub fn deny(root: &Path, request_ref: &str) -> Result<(), String> {
    let status = UndoCoordinator::new(root)
        .deny(request_ref)
        .map_err(|e| e.to_string())?;
    println!(
        "denied request {} ({} file(s) left untouched)",
        status.request,
        status.paths.len()
    );
    Ok(())
}

fn list_pending(root: &Path) -> Result<(), String> {
    let pending = UndoCoordinator::new(root)
        .pending_requests()
        .map_err(|e| e.to_string())?;
    if pending.is_empty() {
        println!("no undo requests awaiting approval");
        return Ok(());
    }
    for p in &pending {
        println!("{}", pending_line(p));
    }
    println!("approve <id> to execute, deny <id> to refuse");
    Ok(())
}

/// One pending row. Every wire-sourced field is sanitized: a request's paths
/// come from a turn record, so they are attacker-controllable in exactly the
/// way `render_plan`'s F8 comment describes, and this list is read by a human
/// deciding whether to authorize a destructive op.
///
/// **The request id is printed WHOLE, and never through [`fmt::short_id`].**
/// That helper is a TURN-id formatter: it strips a `t_` prefix, elides the
/// middle, and re-adds `t_`. A request id is a bare ULID, so short_id renders
/// it as `t_01KZ…VEGY` — a string that is neither the id nor a prefix of it,
/// so feeding it back to `approve` cannot resolve, and which reads as a turn
/// id besides. D23's whole loop is "lists pending undo requests; approve/deny
/// by id", so what this prints has to be exactly what the next command takes.
/// Pinned by `approve.rs::the_listed_id_can_be_pasted_straight_back_into_approve`.
fn pending_line(p: &PendingStatus) -> String {
    let paths: Vec<String> = p.paths.iter().map(|s| fmt::sanitize_terminal(s)).collect();
    format!(
        "{}  undo of {}  {} file(s): {}{}",
        p.request,
        fmt::short_id(&p.turn),
        p.paths.len(),
        paths.join(", "),
        if p.allow_modified {
            "  [allow-modified]"
        } else {
            ""
        }
    )
}
