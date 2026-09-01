//! CLI side of the `attest` subsystem: the `attest.jsonl` append lock and the
//! read-only `attest status` verb.
//!
//! Wire format and state machine: `ATTEST-FORMAT.md`. Event types and the
//! deterministic fold live in `agentrec_core::attest` — this module only does
//! I/O and rendering, which core deliberately does not.
//!
//! Module-root file, not `attest/mod.rs`: this repo uses zero directory
//! modules (`find agentrec-core/src cli/src -name mod.rs` is empty).

pub mod lock;
pub mod statuscmd;
