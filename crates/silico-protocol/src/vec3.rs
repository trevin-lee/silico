//! The arithmetic of 3-vectors in `f64` that the core, the render worker, and the protocol's
//! own checks share: each written once, so each gives the same bits wherever it's used (no
//! fused multiply-add: Rust contracts none). Lengths aren't here: a profile's check scales
//! before it squares (`profile.rs`), so a huge vector's length doesn't overflow, where the
//! rest take the plain root of a vector's dot with itself.

/// `a - b`.
pub fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `a · b`, summed in order: x, then y, then z.
pub fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `a × b`.
pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::{cross, dot, sub};

    #[test]
    fn each_is_the_textbook_formula() {
        // By hand: (1, 2, 3) − (4, 6, 9) = (−3, −4, −6); (1, 2, 3) · (4, 5, 6) = 4 + 10 + 18
        // = 32; x × y = z, y × x = −z, and (1, 2, 3) × (4, 5, 6) = (12 − 15, 12 − 6, 5 − 8).
        assert_eq!(sub([1.0, 2.0, 3.0], [4.0, 6.0, 9.0]), [-3.0, -4.0, -6.0]);
        assert_eq!(dot([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]), 32.0);
        assert_eq!(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(cross([0.0, 1.0, 0.0], [1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]);
        assert_eq!(cross([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]), [-3.0, 6.0, -3.0]);
    }

    #[test]
    fn a_dot_sums_x_then_y_then_z() {
        // The spacing of doubles at 1e16 is 2, so 1e16 + 1 and 1 − 1e16 round to ±1e16
        // (ties to even). Each sum below is 0 only added x, then y, then z (or y and x, then
        // z, the same); every other order gives 1: x and z first, in the first; y and z
        // first (x + (y + z), or z, y, x), in the second. The order is the bits.
        assert_eq!(dot([1e16, 1.0, -1e16], [1.0, 1.0, 1.0]), 0.0);
        assert_eq!(dot([1.0, 1e16, -1e16], [1.0, 1.0, 1.0]), 0.0);
    }
}
