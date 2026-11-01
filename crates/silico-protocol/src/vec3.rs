//! The arithmetic of 3-vectors in `f64` that the core, the render worker, and the protocol's
//! own checks share: each written once, so each gives the same bits wherever it's used (no
//! fused multiply-add: Rust contracts none). A length ([`norm`]) is the plain root of a
//! vector's dot with itself, which overflows past about 1e154: a profile's check, which
//! must measure such vectors, scales before it squares instead (`profile.rs`).

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

/// `|a|`: the root of `a · a` (correctly rounded, so the same on every platform).
pub fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// `a / |a|`, each component divided by [`norm`]'s length: a unit vector to rounding, for
/// any `a` neither zero nor too long or short to square.
pub fn normalize(a: [f64; 3]) -> [f64; 3] {
    let length = norm(a);
    a.map(|c| c / length)
}

#[cfg(test)]
mod tests {
    use super::{cross, dot, norm, normalize, sub};

    #[test]
    fn each_is_the_textbook_formula() {
        // By hand: (1, 2, 3) − (4, 6, 9) = (−3, −4, −6); (1, 2, 3) · (4, 5, 6) = 4 + 10 + 18
        // = 32; x × y = z, y × x = −z, and (1, 2, 3) × (4, 5, 6) = (12 − 15, 12 − 6, 5 − 8).
        assert_eq!(sub([1.0, 2.0, 3.0], [4.0, 6.0, 9.0]), [-3.0, -4.0, -6.0]);
        assert_eq!(dot([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]), 32.0);
        assert_eq!(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(cross([0.0, 1.0, 0.0], [1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]);
        assert_eq!(cross([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]), [-3.0, 6.0, -3.0]);
        // |(2, 3, 6)| = √49 = 7, and (2, 3, 6) / 7; a 3-4-5 triangle's hypotenuse.
        assert_eq!(norm([2.0, 3.0, 6.0]), 7.0);
        assert_eq!(norm([0.0, -3.0, 4.0]), 5.0);
        assert_eq!(
            normalize([2.0, 3.0, 6.0]),
            [2.0 / 7.0, 3.0 / 7.0, 6.0 / 7.0]
        );
        assert_eq!(normalize([0.0, 0.0, -5.0]), [0.0, 0.0, -1.0]);
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
