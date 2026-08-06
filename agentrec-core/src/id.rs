//! ULID generation (D26): 48-bit unix-ms timestamp + 80 random bits, Crockford
//! base32, 26 chars. Machine-scoped, lexically time-ordered, merge-safe.

use std::fs::File;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// New turn id: `t_<ULID>`.
pub fn turn_id() -> String {
    format!("t_{}", ulid())
}

/// Raw 26-char ULID for the current time.
pub fn ulid() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    ulid_with(ms, &random_bytes())
}

/// Deterministic form for tests: encode `ms` (low 48 bits) + `rand` (10 bytes).
pub fn ulid_with(ms: u64, rand: &[u8; 10]) -> String {
    let mut out = [0u8; 26];
    let ts = ms & 0xFFFF_FFFF_FFFF;
    for (i, slot) in out.iter_mut().enumerate().take(10) {
        let shift = 45 - (i as u32) * 5;
        *slot = CROCKFORD[((ts >> shift) & 0x1F) as usize];
    }
    let mut acc: u128 = 0;
    for byte in rand {
        acc = (acc << 8) | *byte as u128;
    }
    for i in 0..16 {
        let shift = 75 - (i as u32) * 5;
        out[10 + i] = CROCKFORD[((acc >> shift) & 0x1F) as usize];
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 160 bits of system entropy, lowercase hex — the raw value of an undo
/// confirm token (`undo_coordinator`), handed to the caller once and stored
/// only as a sha256.
///
/// **Returns `None` rather than falling back**, unlike [`random_bytes`]. That
/// function's fallback is a time+pid xorshift: acceptable for a ULID, whose
/// job is uniqueness, and *predictable* for a credential that authorizes
/// writing to a user's working tree. An agent that can guess a token can
/// execute an undo the human never granted, so the honest degradation is to
/// issue no token at all and let the caller refuse the operation.
pub fn random_token() -> Option<String> {
    let mut buf = [0u8; 20];
    let mut f = File::open("/dev/urandom").ok()?;
    f.read_exact(&mut buf).ok()?;
    let mut out = String::with_capacity(40);
    for b in buf {
        out.push_str(&format!("{b:02x}"));
    }
    Some(out)
}

fn random_bytes() -> [u8; 10] {
    let mut buf = [0u8; 10];
    if let Ok(mut f) = File::open("/dev/urandom") {
        if f.read_exact(&mut buf).is_ok() {
            return buf;
        }
    }
    // Fallback: time+pid hash. Weak, but only reached on exotic platforms.
    let seed = std::process::id() as u64
        ^ SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    for slot in buf.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *slot = x as u8;
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_is_26_chars_and_time_ordered() {
        let a = ulid_with(1_000, &[0u8; 10]);
        let b = ulid_with(2_000, &[0u8; 10]);
        assert_eq!(a.len(), 26);
        assert!(a < b); // lexical order follows time
    }

    #[test]
    fn ulid_encodes_timestamp_prefix_stably() {
        let a = ulid_with(42, &[1u8; 10]);
        let b = ulid_with(42, &[2u8; 10]);
        assert_eq!(a[..10], b[..10]); // same ms → same time prefix
        assert_ne!(a[10..], b[10..]); // randomness differs
    }

    #[test]
    fn turn_ids_unique_in_burst() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1_000 {
            assert!(seen.insert(turn_id()));
        }
    }
}
