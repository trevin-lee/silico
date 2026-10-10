//! Comparing a measured value with an expected one, to a relative tolerance.

use super::numbers::relative_error;

/// Fails, saying `what` and both values, unless `got` is within `tolerance` of `want`,
/// relative to `want`.
pub fn assert_close(what: &str, got: f64, want: f64, tolerance: f64) {
    assert!(
        relative_error(got, want) <= tolerance,
        "{what}: got {got}, want {want} (relative error {:e})",
        relative_error(got, want)
    );
}
