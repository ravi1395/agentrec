//! Fixture crate for the attest cargo adapter. Two test-location shapes on
//! purpose: this `#[cfg(test)] mod tests` (unit tests compiled into the lib
//! target) and `tests/integration.rs` (its own cargo target). Phase 2's
//! location resolution must handle both, and the integration-target shape
//! alone is the easier half.

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

pub fn double(a: i32) -> i32 {
    a * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_add_works() {
        assert_eq!(add(2, 2), 4);
    }

    #[test]
    fn unit_double_works() {
        assert_eq!(double(21), 42);
    }

    /// Deliberately ignored: the adapter must report `#[ignore]`d tests as a
    /// recipe-invalid cause, not as a pass.
    #[test]
    #[ignore]
    fn unit_ignored_test() {
        assert_eq!(add(1, 1), 3);
    }
}
