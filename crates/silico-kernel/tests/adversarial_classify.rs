//! Hostile input for `classify` (M4.6: a camera's eye inside a body) and
//! `tessellate_at_least_the_floor`: shapes that aren't one closed solid (a bare face, a
//! compound of curves and surfaces), solids with a cavity, an INTERNAL or EXTERNAL face, or
//! mirrored geometry, points at the ends of the float range, a real part against FreeCAD's
//! `isInside`, and the floor asked again of a shape already meshed. Expected values are by
//! hand, or FreeCAD 1.1.3's (named where they're used).

use std::path::Path;

use silico_core::ErrorCode;
use silico_core::kernel::{KernelRequest, KernelWorker};
use silico_kernel::{
    BooleanOp, Curve, FaceRegions, LENGTH_TOLERANCE, Primitive, Region, Shape, SketchPlane,
    boolean, classify, from_brep, from_text_brep, make, make_box, make_faces, tessellate,
    tessellate_at_least_the_floor, to_brep, translate,
};
use silico_protocol::{Blob, ShapeData};

/// The fixture `name` (tests/fixtures/brep), with Silico's checksum.
fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/brep")
        .join(name);
    let mut body = std::fs::read(path).unwrap();
    body.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
    body
}

fn line(start: [f64; 2], end: [f64; 2]) -> Curve {
    Curve::Line { start, end }
}

/// The 10 × 10 square face from the origin in z = 0.
fn square_face() -> Shape {
    let c = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
    let outer = (0..4).map(|i| line(c[i], c[(i + 1) % 4])).collect();
    make_faces(&FaceRegions {
        plane: SketchPlane::Xy,
        regions: vec![Region {
            outer,
            holes: vec![],
        }],
    })
    .unwrap()
    .shape
}

fn worker() -> KernelWorker {
    KernelWorker::start_worker(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"))).unwrap()
}

fn data(shape: &Shape) -> ShapeData {
    ShapeData::unchecked(Blob::Inline(to_brep(shape).unwrap()))
}

// ---- Shapes that aren't one closed solid ----

#[test]
fn a_bare_square_face_doesnt_hold_points_off_its_plane() {
    // FreeCAD doesn't classify a vertex, edge, wire, or face with OCCT's solid classifier:
    // it measures the distance to it (639ff927de, "Part: fix TopoShape.isInside() for
    // faces"). Silico classifies whatever it's given; on a bare face OCCT's classifier
    // answers as the distance would.
    let face = square_face();
    for (point, inside) in [
        // On the face: on its boundary, as FreeCAD's distance ≤ tolerance says.
        ([5.0, 5.0, 0.0], true),
        ([0.0, 0.0, 0.0], true),
        // Off it, either side, and beside it in its plane: nothing holds these.
        ([5.0, 5.0, 5.0], false),
        ([5.0, 5.0, -5.0], false),
        ([50.0, 50.0, 0.0], false),
        ([5.0, 5.0, -1e-3], false),
    ] {
        let got = classify(&face, point);
        assert!(
            matches!(got, Ok(g) if g == inside),
            "{point:?}: {got:?}, expected {inside}"
        );
    }
}

#[test]
fn a_compound_of_curves_and_surfaces_doesnt_hold_a_far_point() {
    // curves_and_surfaces.brep: edges and open faces, no solid. A point a kilometre off holds
    // nothing; and the answer is an answer, not a failure (or, if refused, one that says it
    // isn't a solid).
    let shape = from_text_brep(&fixture("curves_and_surfaces.brep")).unwrap();
    for point in [[1000.0, 1000.0, 1000.0], [-1000.0, 0.0, 0.0]] {
        match classify(&shape, point) {
            Ok(inside) => assert!(!inside, "{point:?} inside a compound of no solid"),
            Err(e) => panic!("{point:?}: {e:?}"),
        }
    }
}

// ---- Solids the classifier might misread ----

#[test]
fn a_point_in_a_boxs_cavity_isnt_inside() {
    // A 10 mm cube from the origin, a 2 mm cube from (4, 4, 4) cut out of its middle: a
    // void shell. The void is outside, its wall on the boundary, the material inside.
    let outer = make_box(10.0, 10.0, 10.0).unwrap();
    let inner = translate(&make_box(2.0, 2.0, 2.0).unwrap(), [4.0, 4.0, 4.0])
        .unwrap()
        .shape;
    let hollow = boolean(BooleanOp::Cut, &outer, &inner).unwrap().shape;
    for (point, inside) in [
        ([5.0, 5.0, 5.0], false),
        ([4.5, 5.5, 4.2], false),
        ([4.0, 5.0, 5.0], true),
        ([2.0, 2.0, 2.0], true),
        ([3.9, 5.0, 5.0], true),
        ([11.0, 5.0, 5.0], false),
    ] {
        assert_eq!(classify(&hollow, point).unwrap(), inside, "{point:?}");
    }
}

#[test]
fn a_box_with_an_internal_or_external_face_holds_its_inside() {
    // The 10 × 20 × 30 box whose face at x = 0 is INTERNAL, or EXTERNAL, in its shell.
    for name in ["box_internal_face1.bin", "box_external_face1.bin"] {
        let shape = from_brep(&fixture(name)).unwrap();
        for (point, inside) in [
            ([5.0, 10.0, 15.0], true),
            ([9.0, 1.0, 29.0], true),
            ([10.0, 10.0, 15.0], true),
            ([11.0, 10.0, 15.0], false),
            ([5.0, 25.0, 15.0], false),
        ] {
            let got = classify(&shape, point);
            assert!(
                matches!(got, Ok(g) if g == inside),
                "{name} {point:?}: {got:?}, expected {inside}"
            );
        }
    }
}

#[test]
fn a_box_mirrored_in_its_geometry_holds_its_inside() {
    // FreeCAD's 10 × 20 × 30 box mirrored in x = 0: from x = -10 to 0, each plane's axes
    // left-handed.
    let shape = from_text_brep(&fixture("mirrored_box.brep")).unwrap();
    for (point, inside) in [
        ([-5.0, 10.0, 15.0], true),
        ([-0.5, 0.5, 0.5], true),
        ([0.0, 10.0, 15.0], true),
        ([5.0, 10.0, 15.0], false),
        ([-11.0, 10.0, 15.0], false),
    ] {
        assert_eq!(classify(&shape, point).unwrap(), inside, "{point:?}");
    }
}

#[test]
fn a_thin_slab_holds_its_middle_and_not_a_hair_above() {
    // 100 × 100 × 1e-6: its middle (5e-7 up) is inside, 1e-3 above isn't.
    let slab = make_box(100.0, 100.0, 1e-6).unwrap();
    assert!(classify(&slab, [50.0, 50.0, 5e-7]).unwrap());
    assert!(!classify(&slab, [50.0, 50.0, 1e-3]).unwrap());
    assert!(!classify(&slab, [50.0, 50.0, -1e-3]).unwrap());
}

#[test]
fn a_box_a_kilometre_out_holds_its_middle() {
    let far = translate(&make_box(10.0, 10.0, 10.0).unwrap(), [1e6, -1e6, 1e6])
        .unwrap()
        .shape;
    assert!(classify(&far, [1e6 + 5.0, -1e6 + 5.0, 1e6 + 5.0]).unwrap());
    assert!(!classify(&far, [5.0, 5.0, 5.0]).unwrap());
    assert!(!classify(&far, [1e6 + 15.0, -1e6 + 5.0, 1e6 + 5.0]).unwrap());
}

#[test]
fn a_cones_apex_and_a_spheres_poles_are_on_them() {
    // A cone of base radius 5, pointed, 10 tall: its apex at (0, 0, 10); a sphere of radius
    // 10's poles at z = ±10 and the seam at (10, 0, 0). On the boundary each, so inside;
    // a millimetre past each, outside.
    let cone = make(&Primitive::Cone {
        radius1: 5.0,
        radius2: 0.0,
        height: 10.0,
    })
    .unwrap();
    assert!(classify(&cone, [0.0, 0.0, 10.0]).unwrap(), "the apex");
    assert!(!classify(&cone, [0.0, 0.0, 11.0]).unwrap());
    assert!(
        classify(&cone, [0.0, 0.0, 0.0]).unwrap(),
        "the base's centre"
    );
    assert!(
        classify(&cone, [5.0, 0.0, 0.0]).unwrap(),
        "the base's seam vertex"
    );
    let ball = make(&Primitive::Sphere { radius: 10.0 }).unwrap();
    for pole in [[0.0, 0.0, 10.0], [0.0, 0.0, -10.0], [10.0, 0.0, 0.0]] {
        assert!(classify(&ball, pole).unwrap(), "{pole:?}");
    }
    assert!(!classify(&ball, [0.0, 0.0, 11.0]).unwrap());
}

#[test]
fn a_real_part_is_classified_as_freecad_classifies_it() {
    // NIST CTC 01, at the 729 centres of a 9 × 9 × 9 grid over its tight box, against
    // FreeCAD 1.1.3's `shape.isInside(p, tol, True)` with tol the part's largest vertex,
    // edge, or face tolerance (0.00335), as Silico takes it (/tmp script: Part.read, then
    // the same grid in the same order of operations). 234 of the 729 inside.
    const FREECAD: &str = "111111000111111000000111000000111000000111000000111000000111000111111000111111000111111000111111000000111000000111000000000000000111000000111000111111000111111000000000000000000000000111000000111000000111000000111000000111000000000000000000000000000000000000000000111000000111000000111000000111000000111000000000000000000000000000000000000000000111000000111111000111111000111111000111000000000000000000000000000000000000000000111000000111000000111000000111000000111000000000000000000000000000000000000000000111000000111000000111000000111000000111000000000000000000000111111000111111000000111000000111000000000000000111000000111000111111000111111000111111000111111000000111000000111000000111000000111000000111000111111000111111000";
    let part = from_text_brep(&fixture("nist_ctc_01_asme1_ap242-e1.brep")).unwrap();
    let (low, high) = (
        [-400.000_000_1, -225.000_000_000_000_2, -100.0],
        [
            400.000_000_1,
            225.000_000_000_000_14,
            50.000_000_000_000_036,
        ],
    );
    let n = 9;
    let mut got = String::new();
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                let at = |c: usize, m: i32| {
                    low[c] + (f64::from(m) + 0.5) / f64::from(n) * (high[c] - low[c])
                };
                let p = [at(0, i), at(1, j), at(2, k)];
                got.push(if classify(&part, p).unwrap() {
                    '1'
                } else {
                    '0'
                });
            }
        }
    }
    let differ: Vec<usize> = got
        .chars()
        .zip(FREECAD.chars())
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .collect();
    assert!(differ.is_empty(), "differs from FreeCAD at {differ:?}");
}

// ---- Points at the ends of the range, through the worker (its traps armed) ----

/// Whether the worker's answer is right for a point far outside: `false`, or the point
/// refused as out of range, naming it.
fn outside_or_refused(got: &Result<bool, silico_core::Error>) -> bool {
    match got {
        Ok(inside) => !inside,
        Err(e) => e.code() == ErrorCode::InvalidInput && e.parameter() == Some("point"),
    }
}

#[test]
fn points_far_out_along_an_axis_arent_inside_a_box() {
    // A box from the origin to (40, 20, 10). A finite point however far out is outside it,
    // or refused as out of range, naming `point`.
    //
    // BREAKS (spec question: FreeCAD 1.1.3's `isInside(p, 1e-7, True)` answers the same, on
    // the same OCCT): from 1e18 out along +x, or straight above the box along +z, OCCT's
    // classifier says the point is inside (Ok(true)), up to about 1e150; past about 1e154
    // (where a coordinate's square overflows), it throws `gp_Dir() - input vector has zero
    // norm`, and the core reports `kernel_failed`, naming no parameter, "change the
    // operation's values: the kernel can't build it from these". Along -x and along the
    // diagonal it's right up to 1e150.
    let mut w = worker();
    let local = make_box(40.0, 20.0, 10.0).unwrap();
    let b = data(&local);
    let mut wrong = Vec::new();
    for point in [
        [1e10, 1e10, 1e10],
        [1e17, 0.0, 0.0],
        [1e18, 0.0, 0.0],
        [20.0, 10.0, 1e18],
        [1e100, 0.0, 0.0],
        [1e150, 1e150, 1e150],
        [1e155, 0.0, 0.0],
        [1e300, 1e300, 1e300],
        [f64::MAX, 0.0, 0.0],
        [-f64::MAX, -f64::MAX, -f64::MAX],
        [20.0, 10.0, f64::MAX],
    ] {
        let got = w.run(KernelRequest::classify(&b, point));
        if !outside_or_refused(&got) {
            wrong.push(format!("{point:?}: {got:?}"));
        }
    }
    // And the worker still answers after them.
    assert!(w.run(KernelRequest::classify(&b, [1.0, 1.0, 1.0])).unwrap());
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_least_and_signed_zero_points_are_on_a_boxs_corner() {
    // The least subnormal off the box's corner at the origin, and -0, are on it.
    let mut w = worker();
    let b = data(&make_box(40.0, 20.0, 10.0).unwrap());
    for point in [
        [5e-324, 5e-324, 5e-324],
        [-0.0, -0.0, -0.0],
        [-5e-324, 0.0, 0.0],
    ] {
        assert!(
            w.run(KernelRequest::classify(&b, point)).unwrap(),
            "{point:?}"
        );
    }
}

#[test]
fn a_point_that_isnt_finite_is_refused_through_the_worker_and_it_keeps_answering() {
    let mut w = worker();
    let b = data(&make_box(1.0, 1.0, 1.0).unwrap());
    for point in [
        [f64::NAN, 0.0, 0.0],
        [0.0, f64::INFINITY, 0.0],
        [0.0, 0.0, f64::NEG_INFINITY],
    ] {
        let e = w.run(KernelRequest::classify(&b, point)).unwrap_err();
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{point:?}: {e:?}");
        assert_eq!(e.parameter(), Some("point"), "{point:?}: {e:?}");
        assert!(e.suggestion().is_some(), "{point:?}: no suggestion: {e:?}");
    }
    assert!(w.run(KernelRequest::classify(&b, [0.5, 0.5, 0.5])).unwrap());
}

#[test]
fn bytes_that_arent_a_shape_are_refused_naming_the_shape() {
    let mut w = worker();
    for brep in [vec![], vec![0u8; 16], b"garbage that is no B-rep".to_vec()] {
        let shape = ShapeData::unchecked(Blob::Inline(brep));
        let e = w
            .run(KernelRequest::classify(&shape, [0.0, 0.0, 0.0]))
            .unwrap_err();
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
        let e = w
            .run(KernelRequest::tessellate_at_least_the_floor(&shape, 0.1))
            .unwrap_err();
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
    }
    let b = data(&make_box(1.0, 1.0, 1.0).unwrap());
    assert!(w.run(KernelRequest::classify(&b, [0.5, 0.5, 0.5])).unwrap());
}

// ---- The floor ----

#[test]
fn the_floor_is_the_same_asked_again_of_a_shape_already_meshed() {
    // The floor is 1e-4 of the shape's box's diagonal, and OCCT's box takes a meshed
    // shape's triangulation into account: the same shape, meshed and asked again, must have
    // the same floor (the core's cache keys a mesh by the deflection used).
    let cube = make_box(100.0, 100.0, 100.0).unwrap();
    let (first, used) = tessellate_at_least_the_floor(&cube, 1e-9).unwrap();
    for asked in [1e-9, 1.0 / 128.0, used] {
        let (again, again_used) = tessellate_at_least_the_floor(&cube, asked).unwrap();
        assert_eq!(again_used.to_bits(), used.to_bits(), "asked {asked}");
        assert_eq!(again, first, "asked {asked}");
    }
    // A fresh copy of it, and through the bytes, agree.
    let copy = from_brep(&to_brep(&cube).unwrap()).unwrap();
    let (_, copy_used) = tessellate_at_least_the_floor(&copy, 1e-9).unwrap();
    assert_eq!(copy_used.to_bits(), used.to_bits());
    // A sphere, whose box OCCT enlarges by its curved faces, the same way.
    let ball = make(&Primitive::Sphere { radius: 7.0 }).unwrap();
    let (_, a) = tessellate_at_least_the_floor(&ball, 1e-9).unwrap();
    let (_, b) = tessellate_at_least_the_floor(&ball, 1e-9).unwrap();
    let _ = tessellate(&ball, 0.5).unwrap();
    let (_, c) = tessellate_at_least_the_floor(&ball, 1e-9).unwrap();
    assert_eq!((a.to_bits(), b.to_bits()), (c.to_bits(), c.to_bits()));
}

#[test]
fn the_floor_asked_exactly_and_an_ulp_either_side() {
    // At the floor, the floor; an ulp under it, the floor (not refused, not the ulp); an ulp
    // over it, the ulp over.
    let cube = make_box(100.0, 100.0, 100.0).unwrap();
    let (_, floor) = tessellate_at_least_the_floor(&cube, 1e-9).unwrap();
    for (asked, used) in [
        (floor, floor),
        (floor.next_down(), floor),
        (floor.next_up(), floor.next_up()),
    ] {
        let (_, got) = tessellate_at_least_the_floor(&cube, asked).unwrap();
        assert_eq!(got.to_bits(), used.to_bits(), "asked {asked:e}");
    }
    // The least deflection, a subnormal, is the floor too.
    let (_, got) = tessellate_at_least_the_floor(&cube, 5e-324).unwrap();
    assert_eq!(got.to_bits(), floor.to_bits());
}

#[test]
fn a_huge_deflection_is_used_as_asked_through_the_worker() {
    // Far coarser than the part: meshed at it, as asked (never finer, the core checks), a
    // valid mesh, and said.
    let mut w = worker();
    let b = data(&make_box(10.0, 10.0, 10.0).unwrap());
    for asked in [1e3, 1e100, f64::MAX] {
        let got = w.run(KernelRequest::tessellate_at_least_the_floor(&b, asked));
        match got {
            Ok((mesh, used)) => {
                assert_eq!(used.to_bits(), asked.to_bits(), "{asked:e}");
                assert_eq!(mesh.face_triangles.len(), 6, "{asked:e}");
            }
            Err(e) => panic!("{asked:e}: {e:?}"),
        }
    }
}

#[test]
fn a_tiny_shape_at_the_length_tolerance_floor_through_the_worker() {
    // 1e-4 on a side: its floor is LENGTH_TOLERANCE, and the core's check of the reply (used
    // ≥ asked) holds for every deflection under it.
    let mut w = worker();
    let speck = data(&make_box(1e-4, 1e-4, 1e-4).unwrap());
    for asked in [5e-324, 1e-300, 1e-12, LENGTH_TOLERANCE.next_down()] {
        let (_, used) = w
            .run(KernelRequest::tessellate_at_least_the_floor(&speck, asked))
            .unwrap();
        assert_eq!(used.to_bits(), LENGTH_TOLERANCE.to_bits(), "{asked:e}");
    }
}

// ---- Points on the planes and lines a box's faces and edges lie on ----

mod on_extensions {
    use super::*;
    use proptest::prelude::*;

    /// A coordinate for the box from the origin to `size`: on a bound exactly, an ulp or a
    /// micrometre either side of one, its middle, or well past either end, biased toward
    /// the bounds (where a classifier's rays graze faces and run along edges).
    fn coordinate(size: f64) -> impl Strategy<Value = f64> {
        prop_oneof![
            Just(0.0),
            Just(size),
            Just(size / 2.0),
            Just(-1e-3),
            Just(size + 1e-3),
            Just(0.0f64.next_down()),
            Just(size.next_up()),
            Just(-5.0),
            Just(size + 5.0),
            Just(-1e6),
            Just(size + 1e6),
            -10.0..size + 10.0,
        ]
    }

    /// By hand: inside with the boundary for a point within the box's bounds, outside one a
    /// micrometre or more past any; None for a point within OCCT's tolerance band of a
    /// bound, an ulp past it (the band, 1e-7, holds both answers).
    fn expected(size: [f64; 3], p: [f64; 3]) -> Option<bool> {
        let mut inside = true;
        for k in 0..3 {
            let past = (-p[k]).max(p[k] - size[k]);
            if past > 0.0 && past < 1e-6 {
                return None;
            }
            if past > 0.0 {
                inside = false;
            }
        }
        Some(inside)
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 1500, ..ProptestConfig::default() })]

        #[test]
        fn a_box_holds_exactly_the_points_within_its_bounds(
            x in coordinate(40.0),
            y in coordinate(20.0),
            z in coordinate(10.0),
        ) {
            thread_local! {
                static BOX: Shape = make_box(40.0, 20.0, 10.0).unwrap();
            }
            let p = [x, y, z];
            if let Some(inside) = expected([40.0, 20.0, 10.0], p) {
                let got = BOX.with(|b| classify(b, p));
                prop_assert!(matches!(got, Ok(g) if g == inside), "{:?}: {:?}, expected {}", p, got, inside);
            }
        }

        #[test]
        fn a_cylinder_holds_exactly_the_points_within_its_radius_and_height(
            x in prop_oneof![Just(0.0), Just(5.0), Just(-5.0), Just(5.0f64.next_up()), Just(3.0), Just(-4.0), -8.0..8.0f64],
            y in prop_oneof![Just(0.0), Just(5.0), Just(-5.0), Just(3.0), Just(4.0), -8.0..8.0f64],
            z in coordinate(10.0),
        ) {
            // Radius 5, from z = 0 to 10, its seam in y = 0 at x = 5: by hand, inside within
            // radius and height, a micrometre band left unpinned.
            thread_local! {
                static CYLINDER: Shape =
                    make(&Primitive::Cylinder { radius: 5.0, height: 10.0 }).unwrap();
            }
            let p = [x, y, z];
            let radial = (x * x + y * y).sqrt() - 5.0;
            let height = (-z).max(z - 10.0);
            let past = radial.max(height);
            if past.abs() >= 1e-6 {
                let inside = past < 0.0;
                let got = CYLINDER.with(|c| classify(c, p));
                prop_assert!(matches!(got, Ok(g) if g == inside), "{:?}: {:?}, expected {}", p, got, inside);
            }
        }
    }
}

// ---- The fake kernel against the real one ----

mod fake_against_real {
    use super::*;
    use silico_core::fake::FakeKernel;
    use silico_core::kernel::Kernel;
    use silico_core::pool::Outcome;

    fn on_fake<T: Send + std::fmt::Debug + 'static>(
        fake: &FakeKernel,
        request: KernelRequest<T>,
    ) -> Result<T, silico_core::Error> {
        match fake.submit(fake.generation(), request).wait() {
            Outcome::Done(result) => result,
            Outcome::Superseded => panic!("superseded"),
        }
    }

    /// What an agent reads of an error: its code, parameter, message, and suggestion.
    fn said(e: &silico_core::Error) -> (ErrorCode, Option<String>, String, Option<String>) {
        (
            e.code(),
            e.parameter().map(str::to_owned),
            e.message().to_owned(),
            e.suggestion().map(str::to_owned),
        )
    }

    #[test]
    fn a_point_that_isnt_finite_is_refused_alike() {
        let fake = FakeKernel::new(1).unwrap();
        let mut real = worker();
        let box_ = Primitive::Box {
            dx: 40.0,
            dy: 20.0,
            dz: 10.0,
        };
        let on_fake_box = on_fake(&fake, KernelRequest::make(box_).unwrap()).unwrap();
        let on_real_box = real.make(box_).unwrap();
        for point in [[f64::NAN, 0.0, 0.0], [0.0, f64::NEG_INFINITY, 0.0]] {
            let f = on_fake(&fake, KernelRequest::classify(&on_fake_box, point)).unwrap_err();
            let r = real
                .run(KernelRequest::classify(&on_real_box, point))
                .unwrap_err();
            assert_eq!(said(&f), said(&r), "{point:?}");
        }
    }

    #[test]
    fn a_deflection_that_isnt_one_is_refused_alike_at_least_the_floor() {
        let fake = FakeKernel::new(1).unwrap();
        let mut real = worker();
        let box_ = Primitive::Box {
            dx: 40.0,
            dy: 20.0,
            dz: 10.0,
        };
        let on_fake_box = on_fake(&fake, KernelRequest::make(box_).unwrap()).unwrap();
        let on_real_box = real.make(box_).unwrap();
        for deflection in [0.0, -0.0, -1.0, f64::NAN, f64::INFINITY] {
            let f = on_fake(
                &fake,
                KernelRequest::tessellate_at_least_the_floor(&on_fake_box, deflection),
            )
            .unwrap_err();
            let r = real
                .run(KernelRequest::tessellate_at_least_the_floor(
                    &on_real_box,
                    deflection,
                ))
                .unwrap_err();
            assert_eq!(
                (f.code(), f.parameter()),
                (r.code(), r.parameter()),
                "{deflection}: fake {f:?}, real {r:?}"
            );
        }
    }

    #[test]
    fn a_boxs_boundary_and_a_hair_past_it_are_classified_alike() {
        // The fake's solids are their boxes, "a point on its boundary counts as inside, as
        // OCCT's classifier counts it": on the box's faces, edges, and corners, and a
        // micrometre past and inside them, the two agree.
        let fake = FakeKernel::new(1).unwrap();
        let mut real = worker();
        let box_ = Primitive::Box {
            dx: 40.0,
            dy: 20.0,
            dz: 10.0,
        };
        let on_fake_box = on_fake(&fake, KernelRequest::make(box_).unwrap()).unwrap();
        let on_real_box = real.make(box_).unwrap();
        let mut differ = Vec::new();
        for x in [-1e-6, 0.0, 1e-6, 20.0, 40.0 - 1e-6, 40.0, 40.0 + 1e-6] {
            for y in [-1e-6, 0.0, 10.0, 20.0, 20.0 + 1e-6] {
                for z in [-1e-6, 0.0, 5.0, 10.0, 10.0 + 1e-6] {
                    let p = [x, y, z];
                    let f = on_fake(&fake, KernelRequest::classify(&on_fake_box, p)).unwrap();
                    let r = real.run(KernelRequest::classify(&on_real_box, p)).unwrap();
                    if f != r {
                        differ.push(format!("{p:?}: fake {f}, real {r}"));
                    }
                }
            }
        }
        assert!(differ.is_empty(), "{}", differ.join("\n"));
    }

    #[test]
    fn bytes_that_arent_a_shape_are_refused_alike() {
        let fake = FakeKernel::new(1).unwrap();
        let mut real = worker();
        for brep in [vec![], b"garbage that is no B-rep".to_vec()] {
            let shape = ShapeData::unchecked(Blob::Inline(brep));
            let f = on_fake(&fake, KernelRequest::classify(&shape, [0.0; 3])).unwrap_err();
            let r = real
                .run(KernelRequest::classify(&shape, [0.0; 3]))
                .unwrap_err();
            assert_eq!(f.code(), r.code(), "fake {f:?}, real {r:?}");
            let f = on_fake(
                &fake,
                KernelRequest::tessellate_at_least_the_floor(&shape, 0.1),
            )
            .unwrap_err();
            let r = real
                .run(KernelRequest::tessellate_at_least_the_floor(&shape, 0.1))
                .unwrap_err();
            assert_eq!(f.code(), r.code(), "fake {f:?}, real {r:?}");
        }
    }
}
