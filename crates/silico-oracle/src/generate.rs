//! Random valid cases, for broad coverage: each seed gives the same cases every time.
//! Shapes are valid by construction (sizes well above tolerance, fillets and chamfers that
//! fit their box, booleans of shapes that overlap, profiles that are simple polygons), so a
//! case that fails on either side is worth a look. Values are rounded to a thousandth of a
//! millimetre (or degree), so a case reads as it runs.

use crate::case::{Case, Chamfer, Shape};

/// SplitMix64: small, fast, and the same everywhere.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `[low, high)`, rounded to a thousandth.
    fn between(&mut self, low: f64, high: f64) -> f64 {
        // The top 53 bits, a fraction in [0, 1) exactly.
        let fraction = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        thousandths(low + (high - low) * fraction)
    }

    /// An index below `n` (which must be above 0).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

fn thousandths(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// `count` cases from `seed`, named `generated_<seed>_<n>`.
pub fn cases(seed: u64, count: usize) -> Vec<Case> {
    let mut random = Random(seed);
    (0..count)
        .map(|n| {
            let shape = match random.below(7) {
                0 => primitive(&mut random),
                1 => Shape::Translate {
                    shape: Box::new(primitive(&mut random)),
                    by: [
                        random.between(-50.0, 50.0),
                        random.between(-50.0, 50.0),
                        random.between(-50.0, 50.0),
                    ],
                },
                2 => boolean(&mut random),
                3 => fillet(&mut random),
                4 => chamfer(&mut random),
                5 => extrude(&mut random),
                _ => revolve(&mut random),
            };
            Case {
                name: format!("generated_{seed}_{n}"),
                note: "generated".to_owned(),
                boundary: None,
                shape,
            }
        })
        .collect()
}

fn primitive(random: &mut Random) -> Shape {
    match random.below(5) {
        0 => Shape::Block {
            dx: random.between(1.0, 40.0),
            dy: random.between(1.0, 40.0),
            dz: random.between(1.0, 40.0),
        },
        1 => Shape::Cylinder {
            radius: random.between(1.0, 20.0),
            height: random.between(1.0, 40.0),
        },
        2 => Shape::Sphere {
            radius: random.between(1.0, 20.0),
        },
        3 => {
            let radius1 = random.between(1.0, 20.0);
            // A point, or a radius at least a millimetre from the other.
            let radius2 = if random.coin() {
                0.0
            } else {
                thousandths(radius1 + random.between(1.0, 15.0))
            };
            Shape::Cone {
                radius1,
                radius2,
                height: random.between(1.0, 40.0),
            }
        }
        _ => {
            let major_radius = random.between(5.0, 30.0);
            Shape::Torus {
                major_radius,
                minor_radius: random.between(1.0, major_radius * 0.8),
            }
        }
    }
}

/// A boolean of two primitives, the tool moved by less than the size of either, so they
/// mostly overlap.
fn boolean(random: &mut Random) -> Shape {
    let base = Box::new(primitive(random));
    let tool = Box::new(Shape::Translate {
        shape: Box::new(primitive(random)),
        by: [
            random.between(-5.0, 5.0),
            random.between(-5.0, 5.0),
            random.between(-5.0, 5.0),
        ],
    });
    match random.below(3) {
        0 => Shape::Fuse { base, tool },
        1 => Shape::Cut { base, tool },
        _ => Shape::Common { base, tool },
    }
}

/// A box, and one to three of its twelve edges, distinct and in increasing order.
fn box_and_edges(random: &mut Random) -> (Shape, f64, Vec<u32>) {
    let (dx, dy, dz) = (
        random.between(5.0, 40.0),
        random.between(5.0, 40.0),
        random.between(5.0, 40.0),
    );
    let mut edges: Vec<u32> = (0..=random.below(3))
        .map(|_| u32::try_from(random.below(12)).unwrap_or(0) + 1)
        .collect();
    edges.sort_unstable();
    edges.dedup();
    (Shape::Block { dx, dy, dz }, dx.min(dy).min(dz), edges)
}

/// A fillet of a quarter of the box's smallest side at most, which always fits.
fn fillet(random: &mut Random) -> Shape {
    let (block, smallest, edges) = box_and_edges(random);
    Shape::Fillet {
        shape: Box::new(block),
        edges,
        radius: random.between(0.05 * smallest, 0.25 * smallest),
    }
}

/// A chamfer whose sizes reach at most a quarter of the box's smallest side.
fn chamfer(random: &mut Random) -> Shape {
    let (block, smallest, edges) = box_and_edges(random);
    let distance = random.between(0.05 * smallest, 0.25 * smallest);
    let size = match random.below(3) {
        0 => Chamfer::Equal { distance },
        1 => Chamfer::TwoDistances {
            distance,
            other: random.between(0.05 * smallest, 0.25 * smallest),
        },
        // tan 45° is 1, so the cut reaches at most the distance along the other face.
        _ => Chamfer::DistanceAngle {
            distance,
            angle: random.between(20.0, 45.0),
        },
    };
    Shape::Chamfer {
        shape: Box::new(block),
        edges,
        size,
        flip: random.coin(),
    }
}

/// A rectangle, a triangle, or an L in the plane z = 0, counterclockwise from above.
fn profile(random: &mut Random) -> Vec<[f64; 3]> {
    let (w, h) = (random.between(2.0, 30.0), random.between(2.0, 30.0));
    match random.below(3) {
        0 => vec![[0.0, 0.0, 0.0], [w, 0.0, 0.0], [w, h, 0.0], [0.0, h, 0.0]],
        1 => {
            let apex = random.between(0.0, w);
            vec![[0.0, 0.0, 0.0], [w, 0.0, 0.0], [apex, h, 0.0]]
        }
        _ => {
            // The arm, from a third to two thirds of each side.
            let (a, b) = (
                thousandths(w * random.between(0.33, 0.66)),
                thousandths(h * random.between(0.33, 0.66)),
            );
            vec![
                [0.0, 0.0, 0.0],
                [w, 0.0, 0.0],
                [w, b, 0.0],
                [a, b, 0.0],
                [a, h, 0.0],
                [0.0, h, 0.0],
            ]
        }
    }
}

/// A profile swept up its normal, or up a direction less than 45° from it (x² + y² at most
/// 0.98, under z² = 1).
fn extrude(random: &mut Random) -> Shape {
    let profile = profile(random);
    let direction = random
        .coin()
        .then(|| [random.between(-0.7, 0.7), random.between(-0.7, 0.7), 1.0]);
    Shape::Extrude {
        profile,
        length: random.between(1.0, 30.0),
        direction,
    }
}

/// A rectangle in the plane y = 0, off the z axis or touching it, turned about it.
fn revolve(random: &mut Random) -> Shape {
    let inner = if random.coin() {
        0.0
    } else {
        random.between(1.0, 20.0)
    };
    let outer = thousandths(inner + random.between(1.0, 15.0));
    let (bottom, top) = (0.0, random.between(1.0, 30.0));
    Shape::Revolve {
        profile: vec![
            [inner, 0.0, bottom],
            [outer, 0.0, bottom],
            [outer, 0.0, top],
            [inner, 0.0, top],
        ],
        origin: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
        angle: random.between(5.0, 360.0),
    }
}

#[cfg(test)]
mod tests {
    use super::cases;

    #[test]
    fn a_seed_gives_the_same_cases_every_time() {
        assert_eq!(cases(7, 50), cases(7, 50));
        assert_ne!(cases(7, 50), cases(8, 50));
    }

    #[test]
    fn every_kind_of_case_comes_up() {
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let generated = cases(1, 300);
        for case in &generated {
            seen.insert(case.shape.operation());
        }
        for operation in [
            "box",
            "cylinder",
            "sphere",
            "cone",
            "torus",
            "translate",
            "fuse",
            "cut",
            "common",
            "fillet",
            "chamfer",
            "extrude",
            "revolve",
        ] {
            assert!(seen.contains(operation), "no {operation} in 300 cases");
        }
    }
}
