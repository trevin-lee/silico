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

/// The fuzzy value a boolean of two unit cubes side by side runs with, by hand: FreeCAD
/// 1.1.3's, 10 × Precision::Confusion() (1e-7) for each millimetre of the diagonal of the box
/// round both, about 2 × 1 × 1: √6 × 1e-6 = 2.449e-6 (docs/ports/booleans.md).
const SIDE_BY_SIDE_FUZZY: f64 = 2.449e-6;

#[test]
fn cubes_with_a_gap_at_and_around_the_tolerance() {
    // Unit cubes a gap apart along x. Up to 1e-6, under the fuzzy value, the fuzzy value
    // bridges the gap: one solid, its volume 2 and at most gap × 1 × 1 more (FreeCAD 1.1.3's
    // is 2 + gap / 1.5 at 1e-6); a gap of 1e-5, four times it, stays a gap: two solids, 2. The cut
    // removes nothing and the common is empty either way. FreeCAD 1.1.3's `fuse([b], -1.0)`:
    // one solid of 2.00000067 at 1e-6, two of 2 at 1e-5.
    let a = make(&cube(1.0)).unwrap();
    for gap in [1e-9, 5e-8, 1e-7, 2e-7, 1e-6, 1e-5] {
        let b = at(cube(1.0), [1.0 + gap, 0.0, 0.0]);
        let what = format!("gap {gap}");
        let fused = boolean(BooleanOp::Fuse, &a, &b).unwrap();
        let s = summarize(&fused.shape).unwrap();
        assert!(s.valid, "{what}: {s:?}");
        if gap < SIDE_BY_SIDE_FUZZY {
            assert_eq!(fused.history.result.solids, 1, "{what}");
            assert!(
                s.volume >= 2.0 - 1e-12 && s.volume <= 2.0 + gap + 1e-12,
                "{what}: volume {}",
                s.volume
            );
        } else {
            assert_eq!(fused.history.result.solids, 2, "{what}");
            assert!(close(s.volume, 2.0, 1e-12), "{what}: volume {}", s.volume);
        }
        sound(&what, BooleanOp::Cut, &a, &b, 1.0);
        refused_empty(&what, BooleanOp::Common, &a, &b);
    }
}

#[test]
fn cubes_overlapping_by_less_than_the_tolerance() {
    // Unit cubes overlapping along x. An overlap up to 1e-6, under the fuzzy value, is
    // contact: the common is empty (refused), the cut removes nothing (1), and the fuse is
    // the two cubes, between 2 − overlap and 2. FreeCAD 1.1.3's `-1.0` booleans give the
    // same (at 1e-6: an empty common, a cut of 1, a fuse of 1.99999933). Above it, 3e-6 and
    // 1e-5, the overlap is the sliver it is, on both sides of the band: the common the
    // sliver's volume, the cut 1 − overlap, the fuse 2 − overlap (FreeCAD 1.1.3: the 1e-5
    // sliver, 1e-5).
    let a = make(&cube(1.0)).unwrap();
    for overlap in [1e-9, 5e-8, 1e-7, 2e-7, 1e-6, 3e-6, 1e-5] {
        let b = at(cube(1.0), [1.0 - overlap, 0.0, 0.0]);
        let what = format!("overlap {overlap}");
        if overlap < SIDE_BY_SIDE_FUZZY {
            let fused = boolean(BooleanOp::Fuse, &a, &b).unwrap();
            let s = summarize(&fused.shape).unwrap();
            assert!(s.valid, "{what}: {s:?}");
            assert_eq!(fused.history.result.solids, 1, "{what}");
            assert!(
                s.volume >= 2.0 - overlap - 1e-12 && s.volume <= 2.0 + 1e-12,
                "{what}: volume {}",
                s.volume
            );
            sound(&what, BooleanOp::Cut, &a, &b, 1.0);
            refused_empty(&what, BooleanOp::Common, &a, &b);
        } else {
            sound(&what, BooleanOp::Fuse, &a, &b, 2.0 - overlap);
            sound(&what, BooleanOp::Cut, &a, &b, 1.0 - overlap);
            let common = boolean(BooleanOp::Common, &a, &b).unwrap();
            let s = summarize(&common.shape).unwrap();
            assert!(s.valid, "{what}: {s:?}");
            assert!(close(s.volume, overlap, 1e-6), "{what}: {s:?}");
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
fn boxes_with_flat_faces_a_hair_apart_are_right_to_the_hair() {
    // Two pairs of boxes with flat faces a hair d apart, past OCCT's tolerance, which OCCT
    // 7.9.3 got wrong without a fuzzy value (docs/ports/booleans.md): the fuse and common
    // empty or the first box alone, the cut removing nothing. Under the fuzzy value, 1e-6 of
    // the pair's box (3.3e-6 and 2.3e-6 here), the faces are taken as one, so each op is
    // right but for the slab between them: within d × the faces' shared area. A hair of
    // 1e-5, above the fuzzy value, is a gap OCCT keeps, and each op is right to the
    // measure's precision. FreeCAD 1.1.3's `-1.0` booleans of the first pair at 3e-7: fuse
    // 1.49999996, cut 0.50000005, common 0.250000025; of the second at 2e-7: 1.75000007,
    // 0.750000025, 0.25. Found by the seeded property test below (PROPTEST_RNG_SEED=20261021).
    for d in [3e-7, 2e-7, 1e-5] {
        // A 3 × 0.5 × 0.5 box, and a 1 × 1 × 1.0000001 box whose top is d above the first's:
        // they overlap in 1 × 0.5 × 0.5, the tops sharing 1 × 0.5.
        let a = make(&Primitive::Box {
            dx: 3.0,
            dy: 0.5,
            dz: 0.5,
        })
        .unwrap();
        let tall = 1.000_000_1;
        let b = at(
            Primitive::Box {
                dx: 1.0,
                dy: 1.0,
                dz: tall,
            },
            [0.0, 0.0, 0.5 + d - tall],
        );
        let rows = [(0.25, 0.75, tall, d * 0.5, a, b)];
        // A 1 × 1.0000001 × 1 box, and a unit cube at (0.5, −d, 0.5): their low y faces d
        // apart and their high ones 1e-7 + d, each pair sharing 0.5 × 0.5; they overlap in
        // 0.5 × (1 − d) × 0.5.
        let a = make(&Primitive::Box {
            dx: 1.0,
            dy: 1.000_000_1,
            dz: 1.0,
        })
        .unwrap();
        let b = at(cube(1.0), [0.5, -d, 0.5]);
        let both = 0.25 * (1.0 - d);
        let rows = rows
            .into_iter()
            .chain([(both, 1.000_000_1, 1.0, (d + 1e-7 + d) * 0.25, a, b)]);
        for (common, va, vb, slab, a, b) in rows {
            let what = format!("boxes {d:e} apart, {va} and {vb}");
            for (op, right) in [
                (BooleanOp::Fuse, va + vb - common),
                (BooleanOp::Cut, va - common),
                (BooleanOp::Common, common),
            ] {
                if d >= 1e-5 {
                    sound(&what, op, &a, &b, right);
                    continue;
                }
                let built =
                    boolean(op, &a, &b).unwrap_or_else(|e| panic!("{what} {op:?}: refused: {e}"));
                let s = summarize(&built.shape).unwrap();
                assert!(s.valid, "{what} {op:?}: result isn't valid: {s:?}");
                assert_eq!(built.history.validate(2), Ok(()), "{what} {op:?}");
                assert_eq!(
                    faces_without_a_source(&built.history),
                    Vec::<u32>::new(),
                    "{what} {op:?}: result faces with no source"
                );
                assert!(
                    (s.volume - right).abs() <= slab + 1e-12,
                    "{what} {op:?}: volume {}, want {right} within {slab:e}",
                    s.volume
                );
            }
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
    // A cube 1e5 on a side, and a cube centred on its corner. The fuzzy value is 1e-6 of the
    // box round both, its diagonal about √3 × 1e5: 0.173 mm. A 1e-5 cube, far under it,
    // merges away: the cut is the big cube, 6 faces, valid, and the common is empty,
    // refused, as FreeCAD 1.1.3's Part::Cut and Part::Common. A 1 mm cube, six times the
    // fuzzy value, notches it: the three faces it cuts and three of the notch, 9 faces, and
    // the common is the eighth of it inside, 0.125.
    let huge = make(&cube(1e5)).unwrap();
    let tiny = at(cube(1e-5), [-0.5e-5, -0.5e-5, -0.5e-5]);
    let built = boolean(BooleanOp::Cut, &huge, &tiny).unwrap();
    let s = summarize(&built.shape).unwrap();
    assert!(s.valid, "{s:?}");
    assert_eq!(s.faces, 6, "{s:?}");
    assert!(close(s.volume, 1e15, 1e-12), "{s:?}");
    refused_empty("the 1e-5 cube", BooleanOp::Common, &huge, &tiny);
    let small = at(cube(1.0), [-0.5, -0.5, -0.5]);
    let built = boolean(BooleanOp::Cut, &huge, &small).unwrap();
    let s = summarize(&built.shape).unwrap();
    assert!(s.valid, "{s:?}");
    assert_eq!(s.faces, 9, "{s:?}");
    let common = boolean(BooleanOp::Common, &huge, &small).unwrap();
    let s = summarize(&common.shape).unwrap();
    assert!(s.valid, "{s:?}");
    assert!(close(s.volume, 0.125, 1e-9), "{s:?}");
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
    /// coincident: there the fuzzy value takes them as one, or a contact as none, so the
    /// volume is off by the slab between them or the result is empty, as FreeCAD 1.1.3's is
    /// (docs/ports/booleans.md; pinned in
    /// `boxes_with_flat_faces_a_hair_apart_are_right_to_the_hair`), and only the result's
    /// soundness and the refusal's suggestion are checked.
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

// ---------------------------------------------------------------------------------------
// The fuzzy value at the edges of scale (break-it on FreeCAD 1.1.3's automatic fuzzy value,
// docs/ports/booleans.md, What the fuzzy value merges): f = 1e-6 × the diagonal of the box
// round both inputs, by hand; FreeCAD 1.1.3's `-1.0` booleans of each give the same.

#[test]
fn a_kilometre_bar_keeps_a_corner_cube_above_the_fuzzy_value_and_merges_one_under_it() {
    // A 1 km × 10 × 10 bar and a cube centred on its corner: f ≈ 1e-6 × 1e6 = 1 mm. A 5 mm
    // cube, above it, is right: an eighth inside, 15.625; the fuse 1e8 + 125 − 15.625, the
    // cut 1e8 − 15.625. A 0.1 mm cube, under it, merges away: the cut is the bar, 6 faces,
    // the common empty (refused), and the fuse two solids, the bar and the whole small cube
    // overlapping it, which every feature refuses as more than one solid (S5).
    let bar = make(&Primitive::Box {
        dx: 1e6,
        dy: 10.0,
        dz: 10.0,
    })
    .unwrap();
    let five = at(cube(5.0), [-2.5; 3]);
    sound("5 mm", BooleanOp::Fuse, &bar, &five, 1e8 + 125.0 - 15.625);
    sound("5 mm", BooleanOp::Cut, &bar, &five, 1e8 - 15.625);
    sound("5 mm", BooleanOp::Common, &bar, &five, 15.625);
    let tenth = at(cube(0.1), [-0.05; 3]);
    let cut = boolean(BooleanOp::Cut, &bar, &tenth).unwrap();
    let s = summarize(&cut.shape).unwrap();
    assert!(
        s.valid && s.faces == 6 && close(s.volume, 1e8, 1e-12),
        "{s:?}"
    );
    refused_empty("0.1 mm", BooleanOp::Common, &bar, &tenth);
    let fused = boolean(BooleanOp::Fuse, &bar, &tenth).unwrap();
    assert_eq!(fused.history.result.solids, 2);
}

#[test]
fn a_far_part_of_the_tool_grows_the_fuzzy_value_and_merges_a_sliver() {
    // 10 mm cubes overlapping by 1e-4: f = 2.45e-5, and the sliver is right, the common
    // 1e-2, the cut 1000 − 1e-2. The same tool fused first with a 1 mm cube a kilometre
    // off: the box round both is a kilometre, f about 1 mm, and the sliver is under it: the
    // common is empty (refused) and the cut removes nothing. The fuzzy value is the box's,
    // not each part's (FreeCAD 1.1.3: the same).
    let a = make(&cube(10.0)).unwrap();
    let near = at(cube(10.0), [9.9999, 0.0, 0.0]);
    sound("alone", BooleanOp::Common, &a, &near, 1e-2);
    sound("alone", BooleanOp::Cut, &a, &near, 1000.0 - 1e-2);
    let far = at(cube(1.0), [1e6, 0.0, 0.0]);
    let tool = boolean(BooleanOp::Fuse, &near, &far).unwrap().shape;
    refused_empty("with a far part", BooleanOp::Common, &a, &tool);
    sound("with a far part", BooleanOp::Cut, &a, &tool, 1000.0);
}

#[test]
fn a_foil_thinner_than_the_fuzzy_value_is_refused_and_a_thicker_one_is_right() {
    // A 10 × 10 foil t thick, half under a 10 mm cube standing across it: f = 1e-6 ×
    // √(15² + 10² + 10²) = 2.06e-5. At t = 1e-6 and 1e-5, under it, the foil's own faces
    // are within f of each other: the cut is refused as an invalid result (FreeCAD 1.1.3's
    // is invalid, 10 faces), the common as empty, and the fuse is two solids, which every
    // feature refuses (S5). At 1e-4 each is right: the half outside, 5e-3, the half inside,
    // 5e-3, and the fuse 1000 + 5e-3.
    let half = at(cube(10.0), [5.0, 0.0, -5.0]);
    let foil = |t: f64| {
        make(&Primitive::Box {
            dx: 10.0,
            dy: 10.0,
            dz: t,
        })
        .unwrap()
    };
    for t in [1e-6, 1e-5] {
        let f = foil(t);
        let what = format!("{t:e}");
        let e = boolean(BooleanOp::Cut, &f, &half).err().unwrap();
        assert_eq!(e.kind(), KernelErrorKind::InvalidResult, "{what}: {e}");
        assert!(e.suggestion().is_some(), "{what}");
        refused_empty(&what, BooleanOp::Common, &f, &half);
        let fused = boolean(BooleanOp::Fuse, &f, &half).unwrap();
        assert_eq!(fused.history.result.solids, 2, "{what}");
    }
    let f = foil(1e-4);
    sound("1e-4", BooleanOp::Cut, &f, &half, 5e-3);
    sound("1e-4", BooleanOp::Common, &f, &half, 5e-3);
    sound("1e-4", BooleanOp::Fuse, &f, &half, 1000.0 + 5e-3);
}
