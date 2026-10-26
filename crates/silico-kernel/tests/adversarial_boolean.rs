//! break-it: hostile and degenerate inputs for booleans and translation
//! (docs/ports/booleans.md). Every expected value is analytic or a hand calculation. The
//! wrong results OCCT still gives, which nothing here catches yet, are listed in the port
//! note with their inputs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::numbers;

use std::f64::consts::PI;
use std::path::Path;

use numbers::relative_error;
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::kernel::{KernelWorker, ShapeData};
use silico_kernel::{
    Axis, BooleanOp, Built, History, KernelErrorKind, MAX_LENGTH, Primitive, Profile, Refusal,
    Shape, ShapeSummary, boolean, from_brep, make, revolve, summarize, to_brep, translate,
};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const OPS: [BooleanOp; 3] = [BooleanOp::Fuse, BooleanOp::Cut, BooleanOp::Common];

fn cube(size: f64) -> Primitive {
    Primitive::Box {
        dx: size,
        dy: size,
        dz: size,
    }
}

fn at(primitive: Primitive, offset: [f64; 3]) -> Shape {
    translate(&make(&primitive).unwrap(), offset).unwrap().shape
}

fn copy(shape: &Shape) -> Shape {
    from_brep(&to_brep(shape).unwrap()).unwrap()
}

fn close(got: f64, want: f64, tolerance: f64) -> bool {
    if want == 0.0 {
        got.abs() <= tolerance
    } else {
        relative_error(got, want) <= tolerance
    }
}

/// Every face of a boolean's result comes from a face of one of its inputs (kept or
/// modified): a boolean makes no face from nothing. A face with no source can't be named.
fn faces_without_a_source(h: &History) -> Vec<u32> {
    (1..=h.result.faces)
        .filter(|&n| {
            !h.inputs.iter().any(|input| {
                input
                    .faces
                    .iter()
                    .any(|f| f.kept == Some(n) || f.modified.iter().any(|r| r.element.index == n))
            })
        })
        .collect()
}

/// The boolean's result, checked: a valid shape, a well-formed history for two inputs in
/// which every result face has a source, and the volume `want`.
fn sound(what: &str, op: BooleanOp, base: &Shape, tool: &Shape, want: f64) -> ShapeSummary {
    let built: Built = boolean(op, base, tool)
        .unwrap_or_else(|e| panic!("{what} {op:?}: refused: {e} ({:?})", e.suggestion()));
    let s = summarize(&built.shape).unwrap();
    assert!(s.valid, "{what} {op:?}: result isn't valid: {s:?}");
    assert_eq!(built.history.validate(2), Ok(()), "{what} {op:?}");
    assert_eq!(
        faces_without_a_source(&built.history),
        Vec::<u32>::new(),
        "{what} {op:?}: result faces with no source"
    );
    // A boundary within OCCT's tolerance (1e-7, so faces within 2e-7 coincide) of another
    // may land on either side.
    assert!(
        close(s.volume, want, 3e-7),
        "{what} {op:?}: volume {}, want {want}",
        s.volume
    );
    s
}

/// The boolean is refused as having an empty result, with the op's own message.
fn refused_empty(what: &str, op: BooleanOp, base: &Shape, tool: &Shape) {
    match boolean(op, base, tool) {
        Ok(built) => {
            let s = summarize(&built.shape).unwrap();
            panic!("{what} {op:?}: want an empty-result refusal, got Ok {s:?}");
        }
        Err(e) => {
            assert_eq!(e.kind(), KernelErrorKind::Failed, "{what} {op:?}: {e}");
            let name = op.name().to_lowercase();
            assert_eq!(
                e.message(),
                format!("the {name}'s result is empty"),
                "{what} {op:?}"
            );
            assert!(e.suggestion().is_some(), "{what} {op:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------
// Identical and coincident operands

#[test]
fn boolean_of_a_shape_with_itself_the_same_object() {
    let a = make(&cube(10.0)).unwrap();
    let s = sound("a ∪ a", BooleanOp::Fuse, &a, &a, 1000.0);
    assert_eq!((s.faces, s.edges, s.vertices), (6, 12, 8));
    let s = sound("a ∩ a", BooleanOp::Common, &a, &a, 1000.0);
    assert_eq!((s.faces, s.edges, s.vertices), (6, 12, 8));
    refused_empty("a − a", BooleanOp::Cut, &a, &a);
}

#[test]
fn boolean_of_identical_primitives_made_separately() {
    let cases = [
        (cube(10.0), 1000.0),
        (Primitive::Sphere { radius: 5.0 }, 4.0 / 3.0 * PI * 125.0),
        (
            Primitive::Cylinder {
                radius: 3.0,
                height: 7.0,
            },
            PI * 9.0 * 7.0,
        ),
        (
            Primitive::Cone {
                radius1: 4.0,
                radius2: 1.0,
                height: 6.0,
            },
            PI * 6.0 / 3.0 * (16.0 + 4.0 + 1.0),
        ),
        (
            Primitive::Cone {
                radius1: 4.0,
                radius2: 0.0,
                height: 6.0,
            },
            PI * 6.0 / 3.0 * 16.0,
        ),
        (
            Primitive::Torus {
                major_radius: 10.0,
                minor_radius: 2.0,
            },
            2.0 * PI * PI * 10.0 * 4.0,
        ),
        // A horn torus: the tube touches the axis.
        (
            Primitive::Torus {
                major_radius: 3.0,
                minor_radius: 3.0,
            },
            2.0 * PI * PI * 3.0 * 9.0,
        ),
    ];
    for (p, volume) in cases {
        let a = make(&p).unwrap();
        let b = make(&p).unwrap();
        let what = format!("{p:?}");
        sound(&what, BooleanOp::Fuse, &a, &b, volume);
        sound(&what, BooleanOp::Common, &a, &b, volume);
        refused_empty(&what, BooleanOp::Cut, &a, &b);
    }
}

#[test]
fn coaxial_cylinders_sharing_their_side_surface() {
    let c = Primitive::Cylinder {
        radius: 2.0,
        height: 10.0,
    };
    let a = make(&c).unwrap();
    let b = at(c, [0.0, 0.0, 5.0]);
    let area = PI * 4.0;
    sound("coaxial", BooleanOp::Fuse, &a, &b, area * 15.0);
    sound("coaxial", BooleanOp::Cut, &a, &b, area * 5.0);
    sound("coaxial", BooleanOp::Common, &a, &b, area * 5.0);
}

#[test]
fn concentric_spheres_one_inside_the_other() {
    let big = make(&Primitive::Sphere { radius: 6.0 }).unwrap();
    let small = make(&Primitive::Sphere { radius: 5.0 }).unwrap();
    let v = |r: f64| 4.0 / 3.0 * PI * r * r * r;
    sound("spheres", BooleanOp::Fuse, &big, &small, v(6.0));
    sound("spheres", BooleanOp::Cut, &big, &small, v(6.0) - v(5.0));
    sound("spheres", BooleanOp::Common, &big, &small, v(5.0));
    refused_empty("spheres", BooleanOp::Cut, &small, &big);
}

#[test]
fn shapes_moved_by_float_sums_that_differ_in_the_last_bit_still_coincide() {
    // 0.1 + 0.2 != 0.3 in floating point; the two cubes differ by 5.6e-17, far inside
    // OCCT's tolerance, so they're the same cube.
    let a = translate(&at(cube(1.0), [0.1, 0.0, 0.0]), [0.2, 0.0, 0.0])
        .unwrap()
        .shape;
    let b = at(cube(1.0), [0.3, 0.0, 0.0]);
    sound("0.1+0.2 vs 0.3", BooleanOp::Fuse, &a, &b, 1.0);
    sound("0.1+0.2 vs 0.3", BooleanOp::Common, &a, &b, 1.0);
    refused_empty("0.1+0.2 vs 0.3", BooleanOp::Cut, &a, &b);
}

// ---------------------------------------------------------------------------------------
// Contact: faces, edges, vertices, tangency

#[test]
fn cubes_touching_on_a_face() {
    let a = make(&cube(1.0)).unwrap();
    let b = at(cube(1.0), [1.0, 0.0, 0.0]);
    let s = sound("face contact", BooleanOp::Fuse, &a, &b, 2.0);
    assert_eq!(
        s.faces, 10,
        "the shared face is gone, nothing merged without refine"
    );
    let s = sound("face contact", BooleanOp::Cut, &a, &b, 1.0);
    assert_eq!(s.faces, 6);
    refused_empty("face contact", BooleanOp::Common, &a, &b);
}

#[test]
fn cubes_touching_along_an_edge() {
    let a = make(&cube(1.0)).unwrap();
    let b = at(cube(1.0), [1.0, 1.0, 0.0]);
    sound("edge contact", BooleanOp::Fuse, &a, &b, 2.0);
    sound("edge contact", BooleanOp::Cut, &a, &b, 1.0);
    refused_empty("edge contact", BooleanOp::Common, &a, &b);
}

#[test]
fn cubes_touching_at_a_vertex() {
    let a = make(&cube(1.0)).unwrap();
    let b = at(cube(1.0), [1.0, 1.0, 1.0]);
    sound("vertex contact", BooleanOp::Fuse, &a, &b, 2.0);
    sound("vertex contact", BooleanOp::Cut, &a, &b, 1.0);
    refused_empty("vertex contact", BooleanOp::Common, &a, &b);
}

#[test]
fn cubes_with_a_gap_at_and_around_the_tolerance() {
    let a = make(&cube(1.0)).unwrap();
    for gap in [1e-9, 5e-8, 1e-7, 2e-7, 1e-6] {
        let b = at(cube(1.0), [1.0 + gap, 0.0, 0.0]);
        let what = format!("gap {gap}");
        sound(&what, BooleanOp::Fuse, &a, &b, 2.0);
        sound(&what, BooleanOp::Cut, &a, &b, 1.0);
        refused_empty(&what, BooleanOp::Common, &a, &b);
    }
}

#[test]
fn cubes_overlapping_by_less_than_the_tolerance() {
    // An overlap thinner than OCCT's tolerance is contact: the common is empty (refused),
    // or, if OCCT keeps the sliver, a valid solid of the sliver's volume. Never a "solid"
    // whose volume is nothing like the overlap.
    let a = make(&cube(1.0)).unwrap();
    for overlap in [1e-9, 5e-8, 1e-7, 2e-7, 1e-6, 1e-5] {
        let b = at(cube(1.0), [1.0 - overlap, 0.0, 0.0]);
        let what = format!("overlap {overlap}");
        sound(&what, BooleanOp::Fuse, &a, &b, 2.0 - overlap);
        sound(&what, BooleanOp::Cut, &a, &b, 1.0 - overlap);
        match boolean(BooleanOp::Common, &a, &b) {
            Err(e) => {
                // Faces within twice the tolerance of each other coincide.
                assert!(overlap <= 2e-7, "{what}: common refused: {e}");
                assert_eq!(e.message(), "the common's result is empty", "{what}");
            }
            Ok(built) => {
                let s = summarize(&built.shape).unwrap();
                assert!(s.valid, "{what}: {s:?}");
                assert!(close(s.volume, overlap, 1e-6), "{what}: {s:?}");
            }
        }
    }
}

#[test]
fn a_boolean_result_refused_as_invalid_says_what_to_try() {
    // When OCCT's result fails the check, the error says what to try, as fillet's does.
    // A cylinder (r 1, h 2) fused with a 2 mm cube at (-1.0000001, 1, 0.4999999).
    let cylinder = make(&Primitive::Cylinder {
        radius: 1.0,
        height: 2.0,
    })
    .unwrap();
    let b = at(cube(2.0), [-1.000_000_1, 1.0, 0.499_999_9]);
    match boolean(BooleanOp::Fuse, &cylinder, &b) {
        Ok(built) => {
            // A right answer is fine too: 2π + 8 − the quarter of the cylinder's top
            // half (|x| ≤ 1, y ≥ 1 is outside the cylinder): the cube touches it along
            // the line y = 1 only, so the fuse is the sum.
            let v = summarize(&built.shape).unwrap().volume;
            assert!(close(v, 2.0 * PI + 8.0, 1e-5), "volume {v}");
        }
        Err(e) => {
            assert!(
                e.suggestion().is_some(),
                "{:?}: {e} has no suggestion",
                e.kind()
            );
        }
    }
}

#[test]
fn boxes_with_flat_faces_3e7_apart_are_right_or_occts_known_wrong_answer() {
    // A 3 × 0.5 × 0.5 box, and a 1 × 1 × 1.0000001 box at (0, 0, −0.4999998): the second's
    // top is 3e-7 above the first's, past OCCT's tolerance but within a few of it. They
    // overlap in 1 × 0.5 × 0.5, so the common is 0.25, the cut 0.75 − 0.25 = 0.5, and the
    // fuse 0.75 + 1.0000001 − 0.25 = 1.5000001. OCCT 7.9.3 gets all three wrong, as
    // FreeCAD 1.1.3's `Shape.fuse`, `cut`, and `common` do on it: the fuse and common
    // empty, and the cut a valid solid that removes nothing (0.75), with the warnings
    // BOPAlgo_AlertBadPositioning and BOPAlgo_AlertAcquiredSelfIntersection. Silico takes
    // OCCT's answer, as FreeCAD does (docs/ports/booleans.md, Known wrong results): each op
    // is the right answer or that wrong one, so OCCT giving any other answer fails here.
    // Found by the seeded property test below (PROPTEST_RNG_SEED=20261021).
    let a = make(&Primitive::Box {
        dx: 3.0,
        dy: 0.5,
        dz: 0.5,
    })
    .unwrap();
    let b = at(
        Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.000_000_1,
        },
        [0.0, 0.0, -0.499_999_8],
    );
    let what = "boxes 3e-7 apart";
    // Each op, its right volume, and OCCT's known wrong one: a volume, or (None) refused
    // as empty.
    for (op, right, wrong) in [
        (BooleanOp::Fuse, 1.500_000_1, None),
        (BooleanOp::Cut, 0.5, Some(0.75)),
        (BooleanOp::Common, 0.25, None),
    ] {
        match boolean(op, &a, &b) {
            Ok(built) => {
                let s = summarize(&built.shape).unwrap();
                assert!(s.valid, "{what} {op:?}: result isn't valid: {s:?}");
                assert_eq!(built.history.validate(2), Ok(()), "{what} {op:?}");
                assert_eq!(
                    faces_without_a_source(&built.history),
                    Vec::<u32>::new(),
                    "{what} {op:?}: result faces with no source"
                );
                let is_right = close(s.volume, right, 3e-7);
                let is_known_wrong = wrong.is_some_and(|w| close(s.volume, w, 1e-12));
                assert!(
                    is_right || is_known_wrong,
                    "{what} {op:?}: volume {}, want {right} (or OCCT's known {wrong:?})",
                    s.volume
                );
            }
            // OCCT's known wrong answer for the fuse and common: refused as empty.
            Err(e) if wrong.is_none() => {
                assert_eq!(e.kind(), KernelErrorKind::Failed, "{what} {op:?}: {e}");
                let name = op.name().to_lowercase();
                assert_eq!(
                    e.message(),
                    format!("the {name}'s result is empty"),
                    "{what} {op:?}"
                );
                assert!(e.suggestion().is_some(), "{what} {op:?}");
            }
            Err(e) => panic!("{what} {op:?}: refused: {e} ({:?})", e.suggestion()),
        }
    }
}

#[test]
fn cylinder_inscribed_in_a_box_tangent_to_four_faces() {
    // A cylinder of radius 5 centered in a 10 × 10 box touches four of its faces along
    // lines. The cut leaves four corner pieces meeting the tangent lines.
    let b = make(&cube(10.0)).unwrap();
    let c = at(
        Primitive::Cylinder {
            radius: 5.0,
            height: 12.0,
        },
        [5.0, 5.0, -1.0],
    );
    let cyl = PI * 25.0 * 10.0;
    sound("inscribed cylinder", BooleanOp::Cut, &b, &c, 1000.0 - cyl);
    sound("inscribed cylinder", BooleanOp::Common, &b, &c, cyl);
    sound(
        "inscribed cylinder",
        BooleanOp::Fuse,
        &b,
        &c,
        1000.0 + PI * 25.0 * 2.0,
    );
}

#[test]
fn sphere_inscribed_in_a_cube_tangent_to_all_six_faces() {
    let b = make(&cube(10.0)).unwrap();
    let s = at(Primitive::Sphere { radius: 5.0 }, [5.0, 5.0, 5.0]);
    let ball = 4.0 / 3.0 * PI * 125.0;
    sound("inscribed sphere", BooleanOp::Cut, &b, &s, 1000.0 - ball);
    sound("inscribed sphere", BooleanOp::Common, &b, &s, ball);
    sound("inscribed sphere", BooleanOp::Fuse, &b, &s, 1000.0);
}

#[test]
fn spheres_touching_at_one_point() {
    let a = make(&Primitive::Sphere { radius: 1.0 }).unwrap();
    let b = at(Primitive::Sphere { radius: 1.0 }, [2.0, 0.0, 0.0]);
    let ball = 4.0 / 3.0 * PI;
    sound("kissing spheres", BooleanOp::Fuse, &a, &b, 2.0 * ball);
    sound("kissing spheres", BooleanOp::Cut, &a, &b, ball);
    refused_empty("kissing spheres", BooleanOp::Common, &a, &b);
}

#[test]
fn cylinder_filling_a_torus_hole_tangent_along_a_circle() {
    // A torus of radii 10 and 2 has a hole of radius 8; a cylinder of radius 8 touches the
    // tube's inside along the circle z = 0.
    let t = make(&Primitive::Torus {
        major_radius: 10.0,
        minor_radius: 2.0,
    })
    .unwrap();
    let c = at(
        Primitive::Cylinder {
            radius: 8.0,
            height: 10.0,
        },
        [0.0, 0.0, -5.0],
    );
    let torus = 2.0 * PI * PI * 10.0 * 4.0;
    let cyl = PI * 64.0 * 10.0;
    sound("torus hole", BooleanOp::Fuse, &t, &c, torus + cyl);
    sound("torus hole", BooleanOp::Cut, &t, &c, torus);
    refused_empty("torus hole", BooleanOp::Common, &t, &c);
}

#[test]
fn cone_with_its_apex_on_a_sphere() {
    // A cone of base radius 3, height 4 standing on z = 0 with its apex at (0, 0, 4); a
    // sphere of radius 1 centered at (0, 0, 5) has its south pole on the apex.
    let cone = make(&Primitive::Cone {
        radius1: 3.0,
        radius2: 0.0,
        height: 4.0,
    })
    .unwrap();
    let ball = at(Primitive::Sphere { radius: 1.0 }, [0.0, 0.0, 5.0]);
    let vc = PI * 9.0 * 4.0 / 3.0;
    let vs = 4.0 / 3.0 * PI;
    sound("apex on sphere", BooleanOp::Fuse, &cone, &ball, vc + vs);
    sound("apex on sphere", BooleanOp::Cut, &cone, &ball, vc);
    refused_empty("apex on sphere", BooleanOp::Common, &cone, &ball);
}

// ---------------------------------------------------------------------------------------
// Containment, splitting, compounds

#[test]
fn tool_entirely_inside_the_base() {
    let base = make(&cube(10.0)).unwrap();
    let tool = at(cube(2.0), [4.0, 4.0, 4.0]);
    let s = sound("void", BooleanOp::Cut, &base, &tool, 992.0);
    assert_eq!(s.faces, 12, "a box with a cubic void");
    let solids = boolean(BooleanOp::Cut, &base, &tool)
        .unwrap()
        .history
        .result
        .solids;
    assert_eq!(solids, 1, "one solid with two shells");
    sound("void", BooleanOp::Common, &base, &tool, 8.0);
    let s = sound("void", BooleanOp::Fuse, &base, &tool, 1000.0);
    assert_eq!(s.faces, 6, "the tool vanishes into the base");
}

#[test]
fn base_entirely_inside_the_tool() {
    let base = at(cube(2.0), [4.0, 4.0, 4.0]);
    let tool = make(&cube(10.0)).unwrap();
    refused_empty("swallowed", BooleanOp::Cut, &base, &tool);
    sound("swallowed", BooleanOp::Common, &base, &tool, 8.0);
    sound("swallowed", BooleanOp::Fuse, &base, &tool, 1000.0);
}

#[test]
fn cut_that_splits_the_base_in_two_then_used_again() {
    let bar = make(&Primitive::Box {
        dx: 10.0,
        dy: 1.0,
        dz: 1.0,
    })
    .unwrap();
    let slab = at(
        Primitive::Box {
            dx: 2.0,
            dy: 3.0,
            dz: 3.0,
        },
        [4.0, -1.0, -1.0],
    );
    let halves = boolean(BooleanOp::Cut, &bar, &slab).unwrap();
    assert_eq!(halves.history.result.solids, 2);
    let s = summarize(&halves.shape).unwrap();
    assert!(close(s.volume, 8.0, 1e-12), "{s:?}");
    // The two halves, as one compound input: cut a cube from each end, fuse a bridge.
    let ends = at(
        Primitive::Box {
            dx: 12.0,
            dy: 3.0,
            dz: 0.5,
        },
        [-1.0, -1.0, 0.5],
    );
    sound("halves", BooleanOp::Cut, &halves.shape, &ends, 4.0);
    sound("halves", BooleanOp::Common, &halves.shape, &ends, 4.0);
    sound("halves", BooleanOp::Fuse, &halves.shape, &slab, 8.0 + 18.0);
    let bridge = at(
        Primitive::Box {
            dx: 4.0,
            dy: 1.0,
            dz: 1.0,
        },
        [3.0, 0.0, 0.0],
    );
    let joined = boolean(BooleanOp::Fuse, &halves.shape, &bridge).unwrap();
    assert_eq!(
        joined.history.result.solids, 1,
        "the bridge joins the halves"
    );
    assert!(close(summarize(&joined.shape).unwrap().volume, 10.0, 1e-12));
}

#[test]
fn a_fuse_of_shapes_apart_used_as_a_tool() {
    let a = make(&cube(1.0)).unwrap();
    let b = at(cube(1.0), [3.0, 0.0, 0.0]);
    let pair = boolean(BooleanOp::Fuse, &a, &b).unwrap().shape;
    let bar = at(
        Primitive::Box {
            dx: 6.0,
            dy: 0.5,
            dz: 0.5,
        },
        [-1.0, 0.25, 0.25],
    );
    sound("pair tool", BooleanOp::Cut, &bar, &pair, 1.5 - 0.5);
    sound("pair tool", BooleanOp::Common, &bar, &pair, 0.5);
    sound("pair tool", BooleanOp::Fuse, &bar, &pair, 1.5 + 2.0 - 0.5);
}

// ---------------------------------------------------------------------------------------
// Scale and position

#[test]
fn tiny_cube_at_the_corner_of_a_huge_one() {
    // A ratio of 1e10 between the two operands' sizes.
    let huge = make(&cube(1e5)).unwrap();
    let tiny = at(cube(1e-5), [-0.5e-5, -0.5e-5, -0.5e-5]);
    let built = boolean(BooleanOp::Cut, &huge, &tiny).unwrap();
    let s = summarize(&built.shape).unwrap();
    assert!(s.valid, "{s:?}");
    // A notch at the corner: the three faces it cuts, and three faces of the notch.
    assert_eq!(s.faces, 9, "{s:?}");
    let common = boolean(BooleanOp::Common, &huge, &tiny).unwrap();
    let s = summarize(&common.shape).unwrap();
    assert!(s.valid, "{s:?}");
    assert!(close(s.volume, 1.25e-16, 1e-6), "{s:?}");
}

#[test]
fn booleans_at_the_farthest_one_move_can_reach() {
    let far = [MAX_LENGTH, -MAX_LENGTH, MAX_LENGTH];
    let a = at(cube(10.0), far);
    let b = translate(&a, [5.0, 5.0, 5.0]).unwrap().shape;
    for (op, volume) in OPS.into_iter().zip([1875.0, 875.0, 125.0]) {
        let s = sound("far", op, &a, &b, volume);
        assert!(
            s.center.iter().zip(far).all(|(c, f)| (c - f).abs() < 20.0),
            "{op:?}: {s:?}"
        );
    }
}

#[test]
fn booleans_far_beyond_one_move_by_chaining_moves() {
    // One move is limited to MAX_LENGTH each way, but nothing limits a chain of them: a
    // hundred moves put a 1 mm cube 1e8 mm from the origin, where a double's spacing is
    // 1.5e-8 mm, within an order of magnitude of OCCT's tolerance.
    let mut a = make(&cube(1.0)).unwrap();
    for _ in 0..100 {
        a = translate(&a, [MAX_LENGTH, 0.0, 0.0]).unwrap().shape;
    }
    let b = translate(&a, [0.5, 0.5, 0.5]).unwrap().shape;
    for (op, volume) in OPS.into_iter().zip([1.875, 0.875, 0.125]) {
        match boolean(op, &a, &b) {
            Ok(built) => {
                let s = summarize(&built.shape).unwrap();
                assert!(s.valid, "{op:?}: {s:?}");
                assert!(close(s.volume, volume, 1e-6), "{op:?}: {s:?}");
            }
            Err(e) => {
                assert!(e.suggestion().is_some(), "{op:?}: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Translation

#[test]
fn a_move_by_negative_zero_is_the_shape_itself() {
    let a = make(&cube(10.0)).unwrap();
    let moved = translate(&a, [-0.0, -0.0, -0.0]).unwrap();
    assert_eq!(to_brep(&moved.shape).unwrap(), to_brep(&a).unwrap());
    assert!(
        moved.history.inputs[0]
            .faces
            .iter()
            .all(|f| f.kept.is_some())
    );
}

#[test]
fn a_move_by_a_subnormal_is_a_move() {
    let a = make(&cube(10.0)).unwrap();
    let moved = translate(&a, [f64::from_bits(1), 0.0, -f64::MIN_POSITIVE]).unwrap();
    let s = summarize(&moved.shape).unwrap();
    assert!(s.valid);
    assert!(close(s.volume, 1000.0, 1e-12));
    assert_eq!(moved.history.validate(1), Ok(()));
    // Coincident with the original within any tolerance: the cut is empty.
    refused_empty("subnormal move", BooleanOp::Cut, &a, &moved.shape);
}

#[test]
fn a_move_of_exactly_max_length_is_allowed_and_one_ulp_more_is_not() {
    let a = make(&cube(1.0)).unwrap();
    for v in [MAX_LENGTH, -MAX_LENGTH] {
        let s = summarize(&translate(&a, [v, v, v]).unwrap().shape).unwrap();
        assert!(close(s.center[0], v + 0.5, 1e-15), "{s:?}");
    }
    for v in [MAX_LENGTH.next_up(), (-MAX_LENGTH).next_down(), f64::MAX] {
        let e = translate(&a, [0.0, 0.0, v]).err().unwrap();
        assert_eq!(e.kind(), KernelErrorKind::InvalidInput, "{v}");
        assert!(e.message().contains("offset's z"), "{e}");
        assert!(e.suggestion().is_some());
    }
}

#[test]
fn a_moved_empty_shape_is_still_refused_by_a_boolean() {
    let empty = silico_kernel::test_ops::empty_shape().unwrap();
    let moved = translate(&empty, [1.0, 2.0, 3.0]).unwrap();
    assert_eq!(moved.history.validate(1), Ok(()));
    let a = make(&cube(1.0)).unwrap();
    for op in OPS {
        let e = boolean(op, &a, &moved.shape).err().unwrap();
        assert_eq!(e.kind(), KernelErrorKind::InvalidInput, "{op:?}: {e}");
    }
}

#[test]
fn moving_back_and_forth_doesnt_grow_the_shape() {
    // A move puts a new location on the same geometry. If each move adds to a chain of
    // locations rather than replacing it, a shape moved back and forth grows without
    // bound, and so does every operation on it.
    let a = make(&cube(10.0)).unwrap();
    let size = to_brep(&a).unwrap().len();
    let mut b = translate(&a, [1.0, 0.0, 0.0]).unwrap().shape;
    let once = to_brep(&b).unwrap().len();
    for _ in 0..500 {
        b = translate(&b, [-1.0, 0.0, 0.0]).unwrap().shape;
        b = translate(&b, [1.0, 0.0, 0.0]).unwrap().shape;
    }
    let after = to_brep(&b).unwrap().len();
    assert_eq!(
        after, once,
        "a cube's B-rep is {size} bytes, {once} moved once, {after} after 1000 more moves"
    );
    // And moved back to where it started, it's the shape it was, byte for byte.
    let back = translate(&b, [-1.0, 0.0, 0.0]).unwrap().shape;
    assert_eq!(to_brep(&back).unwrap(), to_brep(&a).unwrap());
}

#[test]
fn a_moved_shapes_validity_is_what_a_fresh_check_says() {
    // translate passes the input's validity on without checking. Far from the origin,
    // check that a fresh check (the shape read back from its bytes) agrees.
    let mut a = make(&Primitive::Torus {
        major_radius: 10.0,
        minor_radius: 2.0,
    })
    .unwrap();
    for _ in 0..1000 {
        a = translate(&a, [MAX_LENGTH, MAX_LENGTH, MAX_LENGTH])
            .unwrap()
            .shape;
    }
    let claimed = a.is_valid().unwrap();
    let fresh = copy(&a).is_valid().unwrap();
    assert_eq!(claimed, fresh, "1e9 from the origin");
}

// ---------------------------------------------------------------------------------------
// Invalid inputs

#[test]
fn the_open_box_cut_across_its_missing_face() {
    // The open box (10 × 20 × 30, Face1 at x = 0 missing) is invalid. A tool through the
    // missing face makes OCCT work on the hole: whatever comes back must be a refusal with
    // a reason, or a result no worse than the input; never a crash.
    let open = silico_kernel::test_ops::open_box().unwrap();
    let tool = at(cube(4.0), [-2.0, 8.0, 13.0]);
    for op in OPS {
        for (base, tool, order) in [(&open, &tool, "open base"), (&tool, &open, "open tool")] {
            match boolean(op, base, tool) {
                Ok(built) => {
                    assert_eq!(built.history.validate(2), Ok(()), "{op:?} {order}");
                    let s = summarize(&built.shape).unwrap();
                    assert!(s.faces > 0, "{op:?} {order}: {s:?}");
                    assert!(s.volume.is_finite(), "{op:?} {order}: {s:?}");
                }
                Err(e) => {
                    assert_ne!(e.kind(), KernelErrorKind::Internal, "{op:?} {order}: {e}");
                    assert!(e.suggestion().is_some(), "{op:?} {order}: {e}");
                }
            }
        }
    }
    let open2 = silico_kernel::test_ops::open_box().unwrap();
    for op in OPS {
        let _ = boolean(op, &open, &open2);
        let _ = boolean(op, &open, &open);
    }
}

// ---------------------------------------------------------------------------------------
// Through the worker

#[test]
fn worker_boolean_of_a_shape_with_itself_and_then_goes_on() {
    let mut k = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let a = k.make(cube(10.0)).unwrap();
    let fused = k.boolean(BooleanOp::Fuse, &a, &a).unwrap();
    assert_eq!(fused.shape.valid(), Some(true));
    assert!(close(
        k.summarize(&fused.shape).unwrap().volume,
        1000.0,
        1e-12
    ));
    let e = k.boolean(BooleanOp::Cut, &a, &a).unwrap_err();
    assert_eq!(e.code(), ErrorCode::KernelFailed, "{e}");
    assert!(e.suggestion().is_some(), "{e}");
    // The open box through the worker, every op, with a tool across its missing face.
    let open = ShapeData::unchecked(silico_protocol::Blob::inline(
        to_brep(&silico_kernel::test_ops::open_box().unwrap()).unwrap(),
    ));
    let small = k.make(cube(4.0)).unwrap();
    let tool = k.translate(&small, [-2.0, 8.0, 13.0]).unwrap().shape;
    let inside = k.translate(&small, [3.0, 3.0, 3.0]).unwrap().shape;
    for op in OPS {
        if let Err(e) = k.boolean(op, &open, &tool) {
            assert!(
                matches!(e.code(), ErrorCode::KernelFailed | ErrorCode::InvalidResult),
                "{op:?}: {e}"
            );
        }
        let after = k.boolean(BooleanOp::Common, &a, &inside).unwrap();
        assert!(close(
            k.summarize(&after.shape).unwrap().volume,
            64.0,
            1e-12
        ));
    }
}

#[test]
fn worker_refuses_a_move_too_far_before_or_at_the_worker() {
    let mut k = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let a = k.make(cube(1.0)).unwrap();
    for offset in [
        [f64::NAN, 0.0, 0.0],
        [0.0, f64::INFINITY, 0.0],
        [0.0, 0.0, MAX_LENGTH.next_up()],
        [-f64::MAX, 0.0, 0.0],
    ] {
        let e = k.translate(&a, offset).unwrap_err();
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{offset:?}: {e}");
        assert!(e.suggestion().is_some());
    }
    let ok = k.translate(&a, [1.0, 1.0, 1.0]).unwrap();
    assert_eq!(ok.shape.valid(), Some(true));
}

// ---------------------------------------------------------------------------------------
// Property tests

/// The overlap of two intervals' lengths.
fn overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// Offsets biased to the places where booleans go wrong: touching exactly, just apart or
/// just overlapping by about the tolerance, and coincident.
fn offset() -> impl Strategy<Value = f64> {
    let whole = prop_oneof![
        Just(0.0),
        Just(0.5),
        Just(1.0),
        Just(-1.0),
        Just(2.0),
        Just(-0.5)
    ];
    let nudge = prop_oneof![
        4 => Just(0.0),
        1 => Just(1e-7),
        1 => Just(-1e-7),
        1 => Just(2e-7),
        1 => Just(-2e-7),
        1 => Just(1e-9),
        1 => Just(-1e-9),
        1 => Just(1e-4),
        1 => Just(-1e-4),
    ];
    (whole, nudge).prop_map(|(w, n)| w + n)
}

fn size() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(1.0),
        Just(2.0),
        Just(0.5),
        Just(1.0 + 1e-7),
        Just(3.0),
        0.1f64..4.0
    ]
}

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 1000,
        failure_persistence: None,
        // Fixed, so a red run is red every time.
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x5111_c0b0),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// Axis-aligned boxes: each boolean's volume is a product of interval overlaps. A
    /// result must be valid, with a history in which every face has a source, and must
    /// have that volume; a refusal must say what to try, and be right that the result is
    /// empty. Except where two parallel faces are within a micron of each other and not
    /// coincident: there OCCT can give an empty result or a valid wrong one, and Silico
    /// takes it, as FreeCAD does (docs/ports/booleans.md, Known wrong results; pinned in
    /// `boxes_with_flat_faces_3e7_apart_are_right_or_occts_known_wrong_answer`), so only
    /// the result's soundness and the refusal's suggestion are checked.
    #[test]
    fn box_booleans_match_interval_arithmetic(
        a in (size(), size(), size()),
        b in (size(), size(), size()),
        o in (offset(), offset(), offset()),
    ) {
        let base = make(&Primitive::Box { dx: a.0, dy: a.1, dz: a.2 }).unwrap();
        let tool = at(Primitive::Box { dx: b.0, dy: b.1, dz: b.2 }, [o.0, o.1, o.2]);
        let (a, b, o) = ([a.0, a.1, a.2], [b.0, b.1, b.2], [o.0, o.1, o.2]);
        let va = a.iter().product::<f64>();
        let vb = b.iter().product::<f64>();
        let dims: Vec<f64> = (0..3).map(|i| overlap(0.0, a[i], o[i], o[i] + b[i])).collect();
        let common = dims.iter().product::<f64>();
        // Two parallel faces closer than a micron but not coincident: their planes that
        // close, and the faces themselves meeting, or within a micron, along the other axes.
        let near = (0..3).any(|i| {
            let faces_meet = (0..3)
                .filter(|&j| j != i)
                .all(|j| o[j] <= a[j] + 1e-6 && o[j] + b[j] >= -1e-6);
            faces_meet
                && [0.0, a[i]].iter().any(|&p| {
                    [o[i], o[i] + b[i]].iter().any(|&q| {
                        let d = (p - q).abs();
                        d > 0.0 && d < 1e-6
                    })
                })
        });
        // A boundary moved by twice the tolerance, over the largest face.
        let slack = 1e-9 * (va + vb) + 2e-7 * 16.0;
        for (op, want) in [
            (BooleanOp::Fuse, va + vb - common),
            (BooleanOp::Cut, va - common),
            (BooleanOp::Common, common),
        ] {
            let what = format!("{op:?} a={a:?} b={b:?} at {o:?}");
            match boolean(op, &base, &tool) {
                Ok(built) => {
                    let s = summarize(&built.shape).unwrap();
                    prop_assert!(s.valid, "{what}: invalid {s:?}");
                    prop_assert_eq!(built.history.validate(2), Ok(()), "{}", what);
                    prop_assert_eq!(
                        faces_without_a_source(&built.history), Vec::<u32>::new(), "{}", what
                    );
                    prop_assert!(
                        near || (s.volume - want).abs() <= slack,
                        "{what}: volume {} want {want}", s.volume
                    );
                }
                Err(e) => {
                    prop_assert!(e.suggestion().is_some(), "{}", what);
                    prop_assert_ne!(e.kind(), KernelErrorKind::Internal, "{}", what);
                    prop_assert!(
                        near || (e.message().contains("empty") && want <= slack),
                        "{what}: refused: {e} ({:?}), want volume {want}", e.suggestion()
                    );
                }
            }
        }
    }
}

/// A cylinder of `radius` along x from `base`, `length` long: a rectangle in the plane
/// z = base's revolved a full turn about the line through `base` along x.
fn cylinder_along_x(base: [f64; 3], radius: f64, length: f64) -> Shape {
    let [x, y, z] = base;
    let points = vec![
        [x, y, z],
        [x + length, y, z],
        [x + length, y + radius, z],
        [x, y + radius, z],
    ];
    let axis = Axis {
        origin: base,
        direction: [1.0, 0.0, 0.0],
    };
    revolve(&Profile { points }, &axis, 360.0).unwrap().shape
}

#[test]
fn a_cylinder_tangent_to_a_boxs_face_along_a_line_never_fuses_wrong() {
    // SolidCraft's random boolean identity (its kernel's fuzz generator, seeds 424, 753,
    // 768, 773, 817), in process: each a cylinder touching a box's face along a line from
    // outside, a cylinder along x built as a rectangle revolved about it. OCCT 7.9.3's fuse
    // of each is invalid, one shell of volume A − B or below zero, in FreeCAD 1.1.3 too
    // (docs/ports/booleans.md, known wrong results): Silico refuses it as an invalid
    // result, never returning it, and never calls it empty, however far below zero its
    // volume (seed 768's is -5968: a solid inside out). Should OCCT get one right, it's
    // accepted only as A + B. The common is empty, a line, refused as empty; the cut is A,
    // whole, sound. Volumes by hand.
    let cases: [(&str, Shape, Shape, f64, f64); 5] = [
        (
            "424",
            at(
                Primitive::Box {
                    dx: 30.0,
                    dy: 12.5,
                    dz: 15.0,
                },
                [2.5, 5.0, 7.5],
            ),
            cylinder_along_x([27.5, 12.5, 30.0], 7.5, 20.0),
            30.0 * 12.5 * 15.0,
            PI * 7.5 * 7.5 * 20.0,
        ),
        (
            "753",
            at(
                Primitive::Box {
                    dx: 30.0,
                    dy: 10.0,
                    dz: 10.0,
                },
                [0.0, 7.5, 17.5],
            ),
            cylinder_along_x([17.5, 20.0, 22.5], 2.5, 20.0),
            30.0 * 10.0 * 10.0,
            PI * 2.5 * 2.5 * 20.0,
        ),
        (
            "768",
            cylinder_along_x([27.5, 17.5, 20.0], 7.5, 27.5),
            at(
                Primitive::Box {
                    dx: 22.5,
                    dy: 27.5,
                    dz: 17.5,
                },
                [10.0, 10.0, 27.5],
            ),
            PI * 7.5 * 7.5 * 27.5,
            22.5 * 27.5 * 17.5,
        ),
        (
            "773",
            at(
                Primitive::Box {
                    dx: 15.0,
                    dy: 27.5,
                    dz: 27.5,
                },
                [22.5, 12.5, 0.0],
            ),
            at(
                Primitive::Cylinder {
                    radius: 5.0,
                    height: 25.0,
                },
                [27.5, 7.5, 10.0],
            ),
            15.0 * 27.5 * 27.5,
            PI * 5.0 * 5.0 * 25.0,
        ),
        (
            "817",
            at(
                Primitive::Box {
                    dx: 17.5,
                    dy: 22.5,
                    dz: 12.5,
                },
                [7.5, 30.0, 17.5],
            ),
            at(
                Primitive::Cylinder {
                    radius: 7.5,
                    height: 7.5,
                },
                [22.5, 22.5, 15.0],
            ),
            17.5 * 22.5 * 12.5,
            PI * 7.5 * 7.5 * 7.5,
        ),
    ];
    for (seed, a, b, va, vb) in cases {
        let what = format!("seed {seed}");
        match boolean(BooleanOp::Fuse, &a, &b) {
            Ok(built) => {
                let v = summarize(&built.shape).unwrap().volume;
                assert!(
                    close(v, va + vb, 1e-6),
                    "{what}: fuse {v}, A + B {}",
                    va + vb
                );
            }
            Err(e) => {
                assert_eq!(e.kind(), KernelErrorKind::InvalidResult, "{what}: {e}");
                assert!(
                    matches!(e.refusal(), Refusal::ResultInvalid { .. }),
                    "{what}: {e}"
                );
                assert!(e.suggestion().is_some(), "{what}: {e}");
            }
        }
        refused_empty(&what, BooleanOp::Common, &a, &b);
        sound(&what, BooleanOp::Cut, &a, &b, va);
    }
}

#[test]
fn a_torus_cut_by_a_larger_torus_occt_turns_inside_out_is_right_or_refused() {
    // The oracle sweep's generated_15_96: a torus (R 12.531, r 9.057) less a torus
    // (R 23.399, r 12.985) moved (−3.965, 0.164, 1.576). Their axes are parallel and
    // 3.968 apart, and the tool's tube comes no nearer its own axis than
    // 23.399 − 12.985 = 10.414, so all of the base within 10.414 − 3.968 = 6.446 of its
    // axis is outside the tool: at least ∫ 2πρ · 2√(r² − (ρ − R)²) dρ from 3.474 to 6.446
    // = 908 mm³ is left (about 5854 ± 22, by 400 000 points). OCCT finds no intersection
    // and takes the tool as inside the base: FreeCAD 1.1.3's cut is one solid turned inside
    // out, −57587.21, the base less the whole tool, which its checks pass; Silico refuses
    // it. Either a refusal or the right solid; never inside out, or empty.
    let base = make(&Primitive::Torus {
        major_radius: 12.531,
        minor_radius: 9.057,
    })
    .unwrap();
    let tool = make(&Primitive::Torus {
        major_radius: 23.399,
        minor_radius: 12.985,
    })
    .unwrap();
    let tool = translate(&tool, [-3.965, 0.164, 1.576]).unwrap().shape;
    match boolean(BooleanOp::Cut, &base, &tool) {
        Ok(built) => {
            let s = summarize(&built.shape).unwrap();
            assert!(s.valid, "{s:?}");
            assert!((s.volume - 5854.0).abs() < 100.0, "{s:?}");
            assert_eq!(built.history.validate(2), Ok(()));
            assert_eq!(faces_without_a_source(&built.history), Vec::<u32>::new());
        }
        Err(e) => {
            assert_eq!(e.kind(), KernelErrorKind::InvalidResult, "{e}");
            assert!(matches!(e.refusal(), Refusal::ResultInvalid { .. }), "{e}");
        }
    }
}
