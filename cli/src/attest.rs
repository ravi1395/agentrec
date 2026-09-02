//! CLI side of the `attest` subsystem: the `attest.jsonl` append lock, the
//! derive/run/verify/coverage writers, and the read verbs
//! (`status`/`review`/`gate`/`report`).
//!
//! Wire format and state machine: `ATTEST-FORMAT.md`. Event types and the
//! deterministic fold live in `agentrec_core::attest` — this module only does
//! I/O and rendering, which core deliberately does not.
//!
//! Module-root file, not `attest/mod.rs`: this repo uses zero directory
//! modules (`find agentrec-core/src cli/src -name mod.rs` is empty).

pub mod adapter_cargo;
pub mod capture;
pub mod coverage;
pub mod coveragecmd;
pub mod derivecmd;
pub mod gatecmd;
pub mod lock;
pub mod manualcmd;
pub mod replaycmd;
pub mod reportcmd;
pub mod reviewcmd;
pub mod statuscmd;
