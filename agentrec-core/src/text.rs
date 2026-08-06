//! Shared text rendering primitives used by BOTH the CLI's formatters and
//! `undo_coordinator`'s refusal strings.
//!
//! These two functions lived in `cli/src/fmt.rs` until task F2 moved
//! `build_plan` into this crate. `build_plan` *builds* the strings a plan row
//! renders (`skip_reason_text` for the skipped-entry refusal,
//! `sanitize_terminal` for the wire-sourced `link_kind` interpolated into the
//! symlink refusal), so they had to come with it — core cannot depend on the
//! CLI crate. `cli::fmt` re-exports both, so every call site there is
//! unchanged and `fmt.rs`'s own tests still exercise this implementation.

use crate::record::skip_reason;

/// Strip terminal control characters — C0 controls `0x00`-`0x1F`, DEL
/// `0x7F`, and C1 controls `U+0080`-`U+009F` — from text about to be
/// printed to a real terminal (E7): a prompt excerpt is user-authored text
/// that reaches stdout verbatim, so an embedded escape sequence (e.g. an
/// OSC "set terminal title" or a cursor move) must never survive to the
/// terminal. C1 is included because some terminals honor its single-byte
/// forms as escape introducers in their own right — CSI (U+009B) and OSC
/// (U+009D) chief among them — not just the ESC-prefixed 7-bit equivalents
/// C0 already covers. Printable text — including non-ASCII UTF-8 outside
/// the C1 range — passes through unchanged; this is display-only and never
/// touches what's persisted (the scrub/excerpt pipeline in
/// `agentrec_core::scrub` already ran before this text ever reaches here).
///
/// Prompt excerpts were the first caller and are no longer the only class:
/// `readcmds::render_plan` routes `path`/`tool`/`op` through this before the
/// `undo` confirmation the user reads (redteam round 2, F8) — a FILENAME is
/// equally attacker-authored text reaching stdout verbatim. That attack
/// needed no extension here; the `sanitize_terminal_strips_f8_*` tests below
/// are the probe for that, not an assumption.
pub fn sanitize_terminal(s: &str) -> String {
    s.chars()
        .filter(|c| {
            let cp = *c as u32;
            cp >= 0x20 && cp != 0x7f && !(0x80..=0x9f).contains(&cp)
        })
        .collect()
}

/// Renders [`agentrec_core::record::FileEntry::skipped_reason`] as the
/// human-readable clause `readcmds::build_plan`'s refusal and `readcmds::
/// print_entry`'s notice both embed (SR-D). The ONE place that maps the wire
/// enum to text — `undo` and `diff` must never drift on this vocabulary the
/// way the pre-SR-D messages did. Any value outside the defined enum
/// (including `skip_reason::POLICY`, which has no producer yet) degrades to
/// `reason unrecorded`, per PROTOCOL.md's additive-enum consumer contract —
/// never match exhaustively on this open set.
pub fn skip_reason_text(reason: Option<&str>) -> &'static str {
    match reason {
        Some(r) if r == skip_reason::OVER_CAP => "over size cap",
        Some(r) if r == skip_reason::IO_FAILED => "write failed at record time",
        Some(r) if r == skip_reason::UNREADABLE => "file unreadable at record time",
        _ => "reason unrecorded",
    }
}
