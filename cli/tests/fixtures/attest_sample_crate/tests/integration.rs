//! Integration-target half of the fixture (see `src/lib.rs`'s module doc).

use attest_sample_crate::{add, double};

#[test]
fn integration_add_works() {
    assert_eq!(add(40, 2), 42);
}

#[test]
fn integration_double_works() {
    assert_eq!(double(1), 2);
}
