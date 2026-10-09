//! Comparing numbers in tests.

/// Relative error between `actual` and a nonzero `expected`.
pub fn relative_error(actual: f64, expected: f64) -> f64 {
    ((actual - expected) / expected).abs()
}
