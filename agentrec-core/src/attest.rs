//! attest: the claim core — event types, deterministic fold, claim identity.
//!
//! Wire formats introduced by this subsystem are documented in
//! `ATTEST-FORMAT.md` at the repo root, marked DRAFT — UNSTABLE. `PROTOCOL.md`
//! is untouched by attest v1 (spec decision 9), so nothing here carries a
//! schema-major field or PROTOCOL's versioning guarantees.
//!
//! This module is pure: no I/O, no process execution, no file paths. Test
//! execution, replay, coverage and locking all live in `cli`, which depends on
//! this crate and never the reverse.

pub mod events;
pub mod fold;
pub mod types;
