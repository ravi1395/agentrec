//! attest: the claim core — event types, deterministic fold, claim identity.
//!
//! Wire formats introduced by this subsystem are documented in
//! `ATTEST-FORMAT.md` at the repo root, marked DRAFT — UNSTABLE. `PROTOCOL.md`
//! is untouched by attest v1 (spec decision 9), so nothing here carries a
//! schema-major field or PROTOCOL's versioning guarantees.
//!
//! This module is pure — no process execution and no file paths — with one
//! disclosed exception: minting a `ClaimId` reads system entropy through
//! `crate::id::ulid`, the same sanctioned `/dev/urandom` read every turn id
//! already uses. Nothing here opens a file the caller named. Test execution,
//! replay, coverage and locking all live in `cli`, which depends on this crate
//! and never the reverse.

pub mod events;
pub mod fold;
pub mod types;
