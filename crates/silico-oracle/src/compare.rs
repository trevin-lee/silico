//! What each side measures of a case's shape, and how two measurements are compared. The
//! volume, area, and center compared are each side's shape measured the same way, FreeCAD's
//! (OCCT's default, fixed-order integration: `KernelWorker::measure_as_freecad` on Silico's
//! side), so a difference is a difference in the shapes, not in how precisely they were
//! measured
//! (FreeCAD's way can be off by 1% on faces trimmed by curves, where Silico's own volume is
//! precise to 1e-9; analytic tests check that). Both sides run OCCT 7.9.3, but separate
//! builds of it (FreeCAD's from conda-forge, Silico's from its own recipe), so measures are
//! compared to a tolerance that depends on what the shape is made of ([`Tolerance::of`]);
//! counts and validity exactly.

use serde::{Deserialize, Serialize};
use silico_core::kernel::{FreecadMeasures, ShapeSummary};

use crate::case::Shape;

/// How far apart the two sides' measures may be. Volume and area are relative to the
/// larger; the bounding box's corners and the center of mass are relative to the larger
/// box's size (its largest side, or 1 mm if that's smaller).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tolerance {
    pub relative: f64,
    pub bounds: f64,
    pub center: f64,
}

impl Tolerance {
    /// A shape made only of planes: the roadmap's 1e-9. Over the 2829 such cases of 8080
    /// (the 40 hand cases, the suite's 40 generated ones, and 1000 generated from each of
    /// seeds 1 to 8), every measure agreed to 7.5e-16.
    pub const PLANAR: Tolerance = Tolerance {
        relative: 1e-9,
        bounds: 1e-9,
        center: 1e-9,
    };

    /// A curved shape with no boolean of curved shapes in it: its surfaces are the same
    /// analytic ones on both sides, and over the 4168 such cases volume, area, and center agreed
    /// to 3.8e-10, the tight box to 6.9e-9 (its corners are found by optimization). Tighter
    /// than the roadmap's 1e-6, as it allows when OCCT is more precise.
    pub const CURVED: Tolerance = Tolerance {
        relative: 1e-8,
        bounds: 1e-7,
        center: 1e-8,
    };

    /// A boolean with a curved shape in it, or anything made from one: the intersection
    /// curves are approximations, which the two OCCT builds make a hair differently
    /// (FreeCAD runs its booleans in parallel, Silico in order). Over the 1083 such cases the
    /// largest differences were 1.2e-6 in volume, 5e-7 in area, 6.3e-7 in the center (a
    /// sphere common a torus), and 1.2e-8 in the box: past the roadmap's 1e-6 for curved
    /// shapes, so this is looser.
    pub const CURVED_BOOLEAN: Tolerance = Tolerance {
        relative: 1e-5,
        bounds: 1e-7,
        center: 1e-5,
    };

    /// The tolerance for a case's shape.
    pub fn of(shape: &Shape) -> Tolerance {
        if has_curved_boolean(shape) {
            Tolerance::CURVED_BOOLEAN
        } else if is_curved(shape) {
            Tolerance::CURVED
        } else {
            Tolerance::PLANAR
        }
    }
}

/// Whether `shape` has a face that isn't a plane: a chamfer of planes, and an extruded
/// polygon, are planes; a fillet and a revolve aren't.
fn is_curved(shape: &Shape) -> bool {
    match shape {
        Shape::Block { .. } | Shape::Extrude { .. } => false,
        // A real part: taken as curved, unless the corpus shows otherwise.
        Shape::Cylinder { .. }
        | Shape::Sphere { .. }
        | Shape::Cone { .. }
        | Shape::Torus { .. }
        | Shape::Fillet { .. }
        | Shape::Revolve { .. }
        | Shape::Brep { .. } => true,
        Shape::Translate { shape, .. } | Shape::Chamfer { shape, .. } => is_curved(shape),
        Shape::Fuse { base, tool } | Shape::Cut { base, tool } | Shape::Common { base, tool } => {
            is_curved(base) || is_curved(tool)
        }
    }
}

/// Whether `shape` is, or is made from, a boolean with a curved shape in it.
fn has_curved_boolean(shape: &Shape) -> bool {
    match shape {
        Shape::Fuse { base, tool } | Shape::Cut { base, tool } | Shape::Common { base, tool } => {
            is_curved(base) || is_curved(tool)
        }
        Shape::Translate { shape, .. }
        | Shape::Fillet { shape, .. }
        | Shape::Chamfer { shape, .. } => has_curved_boolean(shape),
        Shape::Block { .. }
        | Shape::Cylinder { .. }
        | Shape::Sphere { .. }
        | Shape::Cone { .. }
        | Shape::Torus { .. }
        | Shape::Extrude { .. }
        | Shape::Revolve { .. }
        | Shape::Brep { .. } => false,
    }
}

/// What one side measured of a shape it built: its volume and area by OCCT's default
/// integration, as FreeCAD measures them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measured {
    pub volume: f64,
    pub area: f64,
    /// The lowest and highest corners of the tight bounding box; none for an empty shape.
    pub bounds: Option<[[f64; 3]; 2]>,
    /// The center of mass, measured the same way as the volume; none for a shape with no
    /// solid. It tells apart shapes whose volume, area, and box agree (a chamfer flipped, a
    /// cone upside down, a fillet on the parallel edge).
    pub center: Option<[f64; 3]>,
    pub faces: u32,
    pub edges: u32,
    pub vertices: u32,
    pub valid: bool,
}

impl Measured {
    /// A shape's topology and validity from its summary, and the rest as FreeCAD measures
    /// it.
    pub fn new(summary: &ShapeSummary, measures: FreecadMeasures) -> Self {
        Measured {
            volume: measures.volume,
            area: measures.area,
            bounds: measures.bounds,
            center: Some(measures.center),
            faces: summary.faces,
            edges: summary.edges,
            vertices: summary.vertices,
            valid: summary.valid,
        }
    }
}

/// What came of building a case on one side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Built(Measured),
    /// Refused, or failed: why, in that side's words.
    Failed(String),
    /// That side couldn't finish: it crashed or hung (why, in its words). Never agreement,
    /// even with the other side's refusal.
    Lost(String),
}

/// Every way `ours` differs from `theirs` beyond `tolerance`, in words; none if they agree.
/// Both failing agrees, whatever each says.
pub fn differences(ours: &Outcome, theirs: &Outcome, tolerance: Tolerance) -> Vec<String> {
    let (ours, theirs) = match (ours, theirs) {
        (Outcome::Built(ours), Outcome::Built(theirs)) => (ours, theirs),
        (Outcome::Failed(_), Outcome::Failed(_)) => return Vec::new(),
        (Outcome::Built(_), Outcome::Failed(why) | Outcome::Lost(why)) => {
            return vec![format!("Silico built it; FreeCAD failed: {why}")];
        }
        (Outcome::Failed(why) | Outcome::Lost(why), Outcome::Built(_)) => {
            return vec![format!("FreeCAD built it; Silico failed: {why}")];
        }
        (
            Outcome::Failed(ours) | Outcome::Lost(ours),
            Outcome::Failed(theirs) | Outcome::Lost(theirs),
        ) => return vec![format!("Silico failed: {ours}; FreeCAD failed: {theirs}")],
    };
    let mut found = Vec::new();
    for (what, a, b) in [
        ("volume", ours.volume, theirs.volume),
        ("area", ours.area, theirs.area),
    ] {
        if !close(a, b, tolerance.relative) {
            found.push(format!("{what}: Silico {a}, FreeCAD {b}"));
        }
    }
    for (what, a, b) in [
        ("faces", ours.faces, theirs.faces),
        ("edges", ours.edges, theirs.edges),
        ("vertices", ours.vertices, theirs.vertices),
    ] {
        if a != b {
            found.push(format!("{what}: Silico {a}, FreeCAD {b}"));
        }
    }
    if ours.valid != theirs.valid {
        found.push(format!(
            "valid: Silico {}, FreeCAD {}",
            ours.valid, theirs.valid
        ));
    }
    match (ours.bounds, theirs.bounds) {
        (None, None) => {}
        (Some(a), Some(b)) => {
            if !boxes_close(a, b, tolerance.bounds) {
                found.push(format!("bounding box: Silico {a:?}, FreeCAD {b:?}"));
            }
        }
        (a, b) => found.push(format!("bounding box: Silico {a:?}, FreeCAD {b:?}")),
    }
    let size = ours
        .bounds
        .map_or(1.0, size)
        .max(theirs.bounds.map_or(1.0, size));
    match (ours.center, theirs.center) {
        (None, None) => {}
        (Some(a), Some(b)) if (0..3).all(|k| (a[k] - b[k]).abs() <= tolerance.center * size) => {}
        (a, b) => found.push(format!("center: Silico {a:?}, FreeCAD {b:?}")),
    }
    found
}

/// A box's largest side, or 1 mm if that's smaller.
fn size([low, high]: [[f64; 3]; 2]) -> f64 {
    (0..3).map(|k| high[k] - low[k]).fold(1.0, f64::max)
}

/// Whether `a` and `b` are within `relative` of the larger.
fn close(a: f64, b: f64, relative: f64) -> bool {
    (a - b).abs() <= relative * a.abs().max(b.abs())
}

/// Whether every corner coordinate of `a` is within `tolerance` of `b`'s, relative to the
/// larger box's largest side (at least 1 mm).
fn boxes_close(a: [[f64; 3]; 2], b: [[f64; 3]; 2], tolerance: f64) -> bool {
    let allowed = tolerance * size(a).max(size(b));
    (0..2).all(|corner| (0..3).all(|k| (a[corner][k] - b[corner][k]).abs() <= allowed))
}

#[cfg(test)]
mod tests {
    use super::{Measured, Outcome, Tolerance, differences};
    use crate::case::{Chamfer, Shape};

    fn block() -> Measured {
        Measured {
            volume: 6000.0,
            area: 2200.0,
            bounds: Some([[0.0; 3], [10.0, 20.0, 30.0]]),
            center: Some([5.0, 10.0, 15.0]),
            faces: 6,
            edges: 12,
            vertices: 8,
            valid: true,
        }
    }

    #[test]
    fn harness_catches_wrong_result() {
        // A 0.1% volume error, or one extra face, is a difference; agreement to within the
        // tolerance isn't.
        let theirs = Outcome::Built(block());
        let planar = Tolerance::PLANAR;
        assert!(differences(&Outcome::Built(block()), &theirs, planar).is_empty());
        let mut near = block();
        near.volume *= 1.0 + 5e-10;
        assert!(differences(&Outcome::Built(near), &theirs, planar).is_empty());
        let mut volume = block();
        volume.volume *= 1.001;
        let found = differences(&Outcome::Built(volume), &theirs, planar);
        assert!(
            found.len() == 1 && found[0].starts_with("volume"),
            "{found:?}"
        );
        let mut face = block();
        face.faces += 1;
        let found = differences(&Outcome::Built(face), &theirs, planar);
        assert!(
            found.len() == 1 && found[0].starts_with("faces"),
            "{found:?}"
        );
    }

    #[test]
    fn every_measure_is_compared() {
        let theirs = Outcome::Built(block());
        type Spoil = fn(&mut Measured);
        let spoils: [(&str, Spoil); 9] = [
            ("center", |m| m.center = Some([5.0, 10.0, 15.1])),
            ("center", |m| m.center = None),
            ("area", |m| m.area *= 1.0001),
            ("edges", |m| m.edges -= 1),
            ("vertices", |m| m.vertices += 2),
            ("valid", |m| m.valid = false),
            ("bounding box", |m| {
                m.bounds = Some([[0.0; 3], [10.0, 20.0, 30.01]])
            }),
            ("bounding box", |m| m.bounds = None),
            ("volume", |m| m.volume = -6000.0),
        ];
        for (what, spoil) in spoils {
            let mut ours = block();
            spoil(&mut ours);
            let found = differences(&Outcome::Built(ours), &theirs, Tolerance::CURVED_BOOLEAN);
            assert!(
                found.len() == 1 && found[0].starts_with(what),
                "{what}: {found:?}"
            );
        }
    }

    #[test]
    fn one_side_failing_is_a_difference_and_both_failing_isnt() {
        let built = Outcome::Built(block());
        let failed = |why: &str| Outcome::Failed(why.to_owned());
        let planar = Tolerance::PLANAR;
        assert_eq!(differences(&built, &failed("no"), planar).len(), 1);
        assert_eq!(differences(&failed("no"), &built, planar).len(), 1);
        assert!(differences(&failed("one reason"), &failed("another"), planar).is_empty());
        // A side that crashed or hung compared nothing: never agreement.
        let lost = Outcome::Lost("crashed".to_owned());
        assert_eq!(
            differences(&lost, &failed("refused"), planar),
            ["Silico failed: crashed; FreeCAD failed: refused"]
        );
        assert_eq!(differences(&failed("refused"), &lost, planar).len(), 1);
        assert_eq!(differences(&lost, &lost, planar).len(), 1);
        assert_eq!(differences(&built, &lost, planar).len(), 1);
    }

    #[test]
    fn each_tolerance_allows_what_it_says_and_no_more() {
        // A measure off by just under each tolerance agrees; just over, it differs.
        for tolerance in [
            Tolerance::PLANAR,
            Tolerance::CURVED,
            Tolerance::CURVED_BOOLEAN,
        ] {
            let theirs = Outcome::Built(block());
            // The box's size is 30: its corners and center are allowed 30 × the tolerance.
            type Spoil = fn(&mut Measured, f64);
            let spoils: [(&str, f64, Spoil); 4] = [
                ("volume", tolerance.relative, |m, e| m.volume *= 1.0 + e),
                ("area", tolerance.relative, |m, e| m.area *= 1.0 - e),
                ("bounding box", tolerance.bounds, |m, e| {
                    m.bounds = Some([[0.0; 3], [10.0, 20.0 + 30.0 * e, 30.0]]);
                }),
                ("center", tolerance.center, |m, e| {
                    m.center = Some([5.0 - 30.0 * e, 10.0, 15.0]);
                }),
            ];
            for (what, allowed, spoil) in spoils {
                let mut under = block();
                spoil(&mut under, allowed * 0.9);
                let found = differences(&Outcome::Built(under), &theirs, tolerance);
                assert!(found.is_empty(), "{tolerance:?} {what}: {found:?}");
                let mut over = block();
                spoil(&mut over, allowed * 1.1);
                let found = differences(&Outcome::Built(over), &theirs, tolerance);
                assert!(
                    found.len() == 1 && found[0].starts_with(what),
                    "{tolerance:?} {what}: {found:?}"
                );
            }
        }
    }

    #[test]
    fn a_shape_far_from_the_origin_is_allowed_its_size_not_its_distance() {
        // A 10 × 20 × 30 box moved 1e6 along x: its corners and center are allowed 30 × the
        // tolerance, not 1e6 × it.
        let far = |dx: f64, dz: f64| {
            let mut m = block();
            m.bounds = Some([[1e6, 0.0, 0.0], [1e6 + 10.0, 20.0, 30.0 + dz]]);
            m.center = Some([1e6 + 5.0 + dx, 10.0, 15.0]);
            m
        };
        let theirs = Outcome::Built(far(0.0, 0.0));
        let found = differences(&Outcome::Built(far(1e-4, 0.0)), &theirs, Tolerance::PLANAR);
        assert!(
            found.len() == 1 && found[0].starts_with("center"),
            "{found:?}"
        );
        let found = differences(&Outcome::Built(far(0.0, 1e-4)), &theirs, Tolerance::PLANAR);
        assert!(
            found.len() == 1 && found[0].starts_with("bounding box"),
            "{found:?}"
        );
    }

    #[test]
    fn a_shape_is_compared_as_what_its_made_of() {
        let block = || Shape::Block {
            dx: 1.0,
            dy: 2.0,
            dz: 3.0,
        };
        let sphere = || Shape::Sphere { radius: 1.0 };
        let boxed = |shape: Shape| Box::new(shape);
        let cases = [
            (block(), Tolerance::PLANAR),
            (
                Shape::Extrude {
                    profile: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    length: 1.0,
                    direction: None,
                },
                Tolerance::PLANAR,
            ),
            (
                Shape::Chamfer {
                    shape: boxed(Shape::Fuse {
                        base: boxed(block()),
                        tool: boxed(Shape::Translate {
                            shape: boxed(block()),
                            by: [0.5, 0.0, 0.0],
                        }),
                    }),
                    edges: vec![1],
                    size: Chamfer::Equal { distance: 0.1 },
                    flip: false,
                },
                Tolerance::PLANAR,
            ),
            (sphere(), Tolerance::CURVED),
            (
                Shape::Fillet {
                    shape: boxed(block()),
                    edges: vec![1],
                    radius: 0.1,
                },
                Tolerance::CURVED,
            ),
            (
                Shape::Revolve {
                    profile: vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 0.0, 1.0]],
                    origin: [0.0; 3],
                    direction: [0.0, 0.0, 1.0],
                    angle: 90.0,
                },
                Tolerance::CURVED,
            ),
            (
                Shape::Fillet {
                    shape: boxed(Shape::Cut {
                        base: boxed(block()),
                        tool: boxed(block()),
                    }),
                    edges: vec![1],
                    radius: 0.1,
                },
                Tolerance::CURVED,
            ),
            (
                Shape::Common {
                    base: boxed(block()),
                    tool: boxed(sphere()),
                },
                Tolerance::CURVED_BOOLEAN,
            ),
            (
                Shape::Translate {
                    shape: boxed(Shape::Chamfer {
                        shape: boxed(Shape::Fuse {
                            base: boxed(sphere()),
                            tool: boxed(block()),
                        }),
                        edges: vec![1],
                        size: Chamfer::Equal { distance: 0.1 },
                        flip: false,
                    }),
                    by: [1.0, 0.0, 0.0],
                },
                Tolerance::CURVED_BOOLEAN,
            ),
        ];
        let cylinder = || Shape::Cylinder {
            radius: 1.0,
            height: 2.0,
        };
        let moved = |shape| Shape::Translate {
            shape: boxed(shape),
            by: [1.0, 0.0, 0.0],
        };
        let more = [
            (moved(block()), Tolerance::PLANAR),
            (
                Shape::Cut {
                    base: boxed(block()),
                    tool: boxed(moved(block())),
                },
                Tolerance::PLANAR,
            ),
            (cylinder(), Tolerance::CURVED),
            (
                Shape::Cone {
                    radius1: 1.0,
                    radius2: 0.0,
                    height: 2.0,
                },
                Tolerance::CURVED,
            ),
            (
                Shape::Torus {
                    major_radius: 5.0,
                    minor_radius: 1.0,
                },
                Tolerance::CURVED,
            ),
            (moved(cylinder()), Tolerance::CURVED),
            (
                Shape::Chamfer {
                    shape: boxed(cylinder()),
                    edges: vec![1],
                    size: Chamfer::Equal { distance: 0.1 },
                    flip: false,
                },
                Tolerance::CURVED,
            ),
            (
                Shape::Fuse {
                    base: boxed(Shape::Fuse {
                        base: boxed(sphere()),
                        tool: boxed(block()),
                    }),
                    tool: boxed(block()),
                },
                Tolerance::CURVED_BOOLEAN,
            ),
            (
                Shape::Cut {
                    base: boxed(block()),
                    tool: boxed(moved(cylinder())),
                },
                Tolerance::CURVED_BOOLEAN,
            ),
            // A real part: compared as curved (its center and box to 1e-8 and 1e-7).
            (
                Shape::Brep {
                    path: "part.brep".to_owned(),
                },
                Tolerance::CURVED,
            ),
            (
                moved(Shape::Brep {
                    path: "part.brep".to_owned(),
                }),
                Tolerance::CURVED,
            ),
        ];
        for (shape, tolerance) in cases.into_iter().chain(more) {
            assert_eq!(Tolerance::of(&shape), tolerance, "{shape:?}");
        }
    }
}
