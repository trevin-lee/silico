//! break-it: hostile and degenerate profiles, lengths, directions, axes, and angles for
//! extrude and revolve (docs/ports/extrude-revolve.md). Every expected volume is a hand
//! calculation: the shoelace area times the rise for an extrusion, and Pappus's theorem
//! (the angle times the profile's first moment about the axis) for a revolve.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::numbers;

use std::f64::consts::{PI, TAU};
use std::path::Path;
use std::time::{Duration, Instant};

use numbers::relative_error;
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::worker::Worker;
use silico_kernel::{
    Axis, BooleanOp, Built, KernelError, KernelErrorKind, Primitive, Profile, ShapeSummary,
    boolean, extrude, make, revolve, summarize, tessellate, translate,
};
use silico_protocol::{Generation, Op};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

fn profile(points: &[[f64; 3]]) -> Profile {
    Profile {
        points: points.to_vec(),
    }
}

fn z(x: f64, y: f64) -> [f64; 3] {
    [x, y, 0.0]
}

fn axis(origin: [f64; 3], direction: [f64; 3]) -> Axis {
    Axis { origin, direction }
}

const Y: [f64; 3] = [0.0, 1.0, 0.0];

/// The `w` × `h` rectangle in the XY plane from (x0, y0), counterclockwise from above.
fn rect(x0: f64, y0: f64, w: f64, h: f64) -> Profile {
    profile(&[z(x0, y0), z(x0 + w, y0), z(x0 + w, y0 + h), z(x0, y0 + h)])
}

/// The result's summary, after checking it's a valid solid with a well-formed history.
fn sound(what: &str, result: Result<Built, KernelError>) -> (Built, ShapeSummary) {
    let built = result.unwrap_or_else(|e| panic!("{what}: refused: {e} ({:?})", e.kind()));
    let s = summarize(&built.shape).unwrap();
    assert!(s.valid, "{what}: not valid: {s:?}");
    assert!(s.volume > 0.0, "{what}: volume {}", s.volume);
    assert_eq!(built.history.validate(1), Ok(()), "{what}");
    (built, s)
}

fn assert_volume(what: &str, got: f64, want: f64, tolerance: f64) {
    assert!(
        relative_error(got, want) <= tolerance,
        "{what}: volume {got}, want {want} (relative error {:e})",
        relative_error(got, want)
    );
}

/// Twice the signed area of a polygon in the XY plane, and its first moment about the y
/// axis (∫ x dA), both by the shoelace formulas.
fn area_and_moment(points: &[[f64; 3]]) -> (f64, f64) {
    let n = points.len();
    let (mut twice_area, mut moment) = (0.0, 0.0);
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        let c = a[0] * b[1] - b[0] * a[1];
        twice_area += c;
        moment += (a[0] + b[0]) * c;
    }
    (twice_area / 2.0, moment / 6.0)
}

/// The volume a shape's mesh encloses (exact for flat faces, whose triangles lie on them),
/// as signed tetrahedra from its first point, so far-off coordinates don't cancel. None
/// when the shape can't be meshed.
fn mesh_volume(shape: &silico_kernel::Shape) -> Option<f64> {
    let mesh = tessellate(shape, 10.0).ok()?;
    let o = mesh.positions[0];
    mesh.triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| {
                let q = mesh.positions[i as usize];
                [q[0] - o[0], q[1] - o[1], q[2] - o[2]]
            });
            (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0
        })
        .sum::<f64>()
        .into()
}

// ---- Breaks ----

#[test]
fn a_bow_tie_crossing_at_a_shallow_angle_is_refused_as_crossing_itself() {
    // (0, 0) → (100, 1e-6) → (100, 0) → (0, 5e-7) crosses itself at x = 100/3, at
    // 1.5e-8 rad. Profile::check misses it (cancellation in segment_distance; see
    // silico-protocol's adversarial_profile.rs), OCCT builds an invalid face, and the user
    // gets InvalidResult "the extrusion's result isn't a valid solid", which names neither
    // the profile nor its sides. Expected: InvalidInput, "crosses or touches itself".
    let bow = profile(&[z(0.0, 0.0), z(100.0, 1e-6), z(100.0, 0.0), z(0.0, 5e-7)]);
    let err = extrude(&bow, 1.0, None).err().unwrap();
    assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{err}");
    assert!(err.message().contains("crosses or touches itself"), "{err}");
}

#[test]
fn a_revolve_by_less_than_occts_angular_precision_is_refused_naming_the_angle() {
    // 9e5 to 1e6 from the axis, by 5e-11° (8.7e-13 rad, under OCCT's
    // Precision::Angular() of 1e-12): the check passes it (the far corner moves 8.7e-7 mm),
    // and OCCT throws: Failed, "Standard_ConstructionError: BRepSweep_Rotation::Constructor",
    // no suggestion. FreeCAD's PartDesign refuses it first: "Angle of revolution too small"
    // (FeatureRevolved.cpp:251). Expected: InvalidInput naming the angle.
    let far = rect(9e5, 0.0, 1e5, 1.0);
    for angle in [5e-11, 3e-11] {
        let err = revolve(&far, &axis([0.0; 3], Y), angle).err().unwrap();
        assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{angle}: {err}");
        assert!(err.message().contains("angle"), "{angle}: {err}");
    }
}

// ---- Tolerance edges that hold ----

#[test]
fn a_profile_within_the_tolerance_of_the_axis_revolves_as_if_touching_it() {
    // A unit square from x = x0, about y. Crossing the axis by less than the tolerance counts as
    // touching: a full turn is a solid cylinder, π. Just past the tolerance on the far
    // side it's a tube with a hole of radius x0: π(1 − x0²). A quarter turn is π/4 (1 − x0²)
    // either way, within what OCCT's tolerance moves.
    for x0 in [-9.99e-8, -5e-8, 0.0, 5e-8, 9.99e-8, 1.01e-7, 3e-7, 1e-6] {
        let square = rect(x0, 0.0, 1.0 - x0, 1.0);
        let hole = if x0 > 1e-7 { x0 * x0 } else { 0.0 };
        let (_, full) = sound(
            &format!("{x0} full"),
            revolve(&square, &axis([0.0; 3], Y), 360.0),
        );
        assert_volume(&format!("{x0} full"), full.volume, PI * (1.0 - hole), 1e-9);
        let (_, quarter) = sound(
            &format!("{x0} quarter"),
            revolve(&square, &axis([0.0; 3], Y), 90.0),
        );
        assert_volume(
            &format!("{x0} quarter"),
            quarter.volume,
            PI / 4.0 * (1.0 - x0 * x0),
            1e-7,
        );
    }
}

/// A revolve refused for its axis, or failed in OCCT with a suggestion about the axis.
fn assert_names_the_axis(what: &str, e: &silico_kernel::KernelError) {
    let says = format!("{} {}", e.message(), e.suggestion().unwrap_or(""));
    assert!(
        matches!(
            e.kind(),
            KernelErrorKind::InvalidInput | KernelErrorKind::Failed
        ),
        "{what}: {e}"
    );
    assert!(says.contains("axis"), "{what}: {says}");
}

#[test]
fn a_side_exactly_the_tolerance_from_the_axis_revolves_or_fails_naming_the_axis() {
    // A unit square whose side parallel to the axis is exactly 1e-7 from it, on either side
    // (both pass check_revolve: crossing by the tolerance counts as touching). OCCT throws
    // at every angle ("Standard_OutOfRange: NCollection_Sequence::Value"), though 0.999e-7
    // and 1.01e-7 both work; FreeCAD's Part.revolve throws the same: an OCCT bug at exactly
    // Precision::Confusion(). Expected: the solid (π(1 − x0²) for a turn, as either
    // neighbour gives), or OCCT's failure with a suggestion naming the axis.
    // And a corner across the axis by 5e-8, within the tolerance: OCCT's MakeRevol isn't
    // done ("Standard_Failure: the revolve couldn't be built"; FreeCAD: "BRep_API: command
    // not done"). Expected: Pappus, θ ∫x dA (θ · 0.046232), to 1e-6.
    let corner = profile(&[
        [0.513_752_008_619_868_8, 0.154_925_381_989_353_33, 0.0],
        [0.101_925_622_320_581_16, 0.256_901_004_309_934_43, 0.0],
        [-5e-8, -0.154_925_381_989_353_27, 0.0],
        [0.411_776_386_299_287_84, -0.256_901_004_309_934_4, 0.0],
    ]);
    let (_, moment) = area_and_moment(&corner.points);
    for angle in [360.0, 90.0] {
        match revolve(&corner, &axis([0.0; 3], Y), angle) {
            Ok(built) => {
                let s = summarize(&built.shape).unwrap();
                assert!(s.valid, "corner by {angle}");
                assert_volume(
                    &format!("corner by {angle}"),
                    s.volume,
                    angle.to_radians() * moment.abs(),
                    1e-6,
                );
            }
            Err(e) => assert_names_the_axis(&format!("corner by {angle}"), &e),
        }
    }
    for x0 in [-1e-7, 1e-7] {
        for angle in [360.0, 90.0] {
            let square = rect(x0, 0.0, 1.0 - x0, 1.0);
            match revolve(&square, &axis([0.0; 3], Y), angle) {
                Ok(built) => {
                    let s = summarize(&built.shape).unwrap();
                    assert!(s.valid, "{x0} by {angle}");
                    assert_volume(
                        &format!("{x0} by {angle}"),
                        s.volume,
                        angle.to_radians() / 2.0,
                        1e-7,
                    );
                }
                Err(e) => assert_names_the_axis(&format!("{x0} by {angle}"), &e),
            }
        }
    }
}

#[test]
fn a_profile_touching_the_axis_at_one_corner_revolves_into_a_pinched_solid() {
    // A diamond with a corner on the axis: area 2, centroid 1 from it. Pappus: 4π for a
    // turn, π for a quarter.
    let diamond = profile(&[z(0.0, 0.0), z(1.0, -1.0), z(2.0, 0.0), z(1.0, 1.0)]);
    let (_, s) = sound("full", revolve(&diamond, &axis([0.0; 3], Y), 360.0));
    assert_volume("full", s.volume, 4.0 * PI, 1e-9);
    let (b, s) = sound("quarter", revolve(&diamond, &axis([0.0; 3], Y), 90.0));
    assert_volume("quarter", s.volume, PI, 1e-9);
    assert_eq!(
        (b.history.ends.start.len(), b.history.ends.end.len()),
        (1, 1)
    );
}

#[test]
fn a_notch_open_to_the_axis_revolves_into_a_solid_with_a_closed_void() {
    // A 2 × 3 rectangle on the axis, less a 1 × 1 notch that opens onto the axis: turned
    // fully, the notch is a void inside, 4·3·π − 1·1·π = 11π; a quarter turn is 11π/4.
    let c = profile(&[
        z(0.0, 0.0),
        z(2.0, 0.0),
        z(2.0, 3.0),
        z(0.0, 3.0),
        z(0.0, 2.0),
        z(1.0, 2.0),
        z(1.0, 1.0),
        z(0.0, 1.0),
    ]);
    let (_, s) = sound("full", revolve(&c, &axis([0.0; 3], Y), 360.0));
    assert_volume("full", s.volume, 11.0 * PI, 1e-9);
    let (_, s) = sound("quarter", revolve(&c, &axis([0.0; 3], Y), 90.0));
    assert_volume("quarter", s.volume, 11.0 * PI / 4.0, 1e-9);
}

#[test]
fn a_corner_just_past_the_tolerance_from_another_side_extrudes_and_booleans_soundly() {
    // A 2 × 2 square less the triangle from its top side down to (1, d): area 2 + d, its
    // waist d wide, within 2e-7 (two tolerances) for the smaller d. Extruded 1, cut from and
    // fused with a 4-cube around it, and intersected with that cube.
    for d in [1.1e-7, 1.5e-7, 2e-7, 1e-6] {
        let pinch = profile(&[
            z(0.0, 0.0),
            z(2.0, 0.0),
            z(2.0, 2.0),
            z(1.0, d),
            z(0.0, 2.0),
        ]);
        let (prism, s) = sound(&format!("{d}"), extrude(&pinch, 1.0, None));
        assert_volume(&format!("{d}"), s.volume, 2.0 + d, 1e-12);
        let cube = translate(
            &make(&Primitive::Box {
                dx: 4.0,
                dy: 4.0,
                dz: 4.0,
            })
            .unwrap(),
            [-1.0, -1.0, -1.0],
        )
        .unwrap()
        .shape;
        for (op, base, tool, want) in [
            (BooleanOp::Common, &prism.shape, &cube, 2.0 + d),
            (BooleanOp::Fuse, &prism.shape, &cube, 64.0),
            (BooleanOp::Cut, &cube, &prism.shape, 62.0 - d),
        ] {
            let built = boolean(op, base, tool).unwrap();
            let s = summarize(&built.shape).unwrap();
            assert!(s.valid, "{d} {op:?}");
            assert_volume(&format!("{d} {op:?}"), s.volume, want, 1e-12);
        }
    }
}

#[test]
fn slivers_spikes_and_short_sides_at_the_tolerance_extrude_exactly() {
    // A unit square with a spike 10 long and d wide at its root (area 1 + 5d); with a side
    // e long (area 1); and the unit square extruded e (volume e).
    for d in [1.1e-7, 2e-7, 5e-7] {
        let spike = profile(&[
            z(0.0, 0.0),
            z(1.0, 0.0),
            z(1.0, 0.5),
            z(11.0, 0.5),
            z(1.0, 0.5 + d),
            z(1.0, 1.0),
            z(0.0, 1.0),
        ]);
        let (_, s) = sound(&format!("spike {d}"), extrude(&spike, 1.0, None));
        assert_volume(&format!("spike {d}"), s.volume, 1.0 + 5.0 * d, 1e-12);
    }
    for e in [1.1e-7, 1.5e-7, 2e-7] {
        let short = profile(&[
            z(0.0, 0.0),
            z(1.0, 0.0),
            z(1.0, 1.0),
            z(e, 1.0),
            z(0.0, 1.0),
        ]);
        let (_, s) = sound(&format!("side {e}"), extrude(&short, 1.0, None));
        assert_volume(&format!("side {e}"), s.volume, 1.0, 1e-12);
        let (_, s) = sound(
            &format!("height {e}"),
            extrude(&rect(0.0, 0.0, 1.0, 1.0), e, None),
        );
        assert_volume(&format!("height {e}"), s.volume, e, 1e-9);
    }
}

#[test]
fn a_profile_bent_off_its_plane_by_up_to_the_tolerance_extrudes() {
    // One corner up to 1e-7 off the plane of the rest, on a 1 mm and a 1 m square.
    for off in [5e-8, 9e-8, 1e-7] {
        for size in [1.0, 1000.0] {
            let bent = profile(&[z(0.0, 0.0), z(size, 0.0), [size, size, off], z(0.0, size)]);
            let (_, s) = sound(&format!("{off} {size}"), extrude(&bent, 1.0, None));
            assert_volume(&format!("{off} {size}"), s.volume, size * size, 1e-9);
        }
    }
}

#[test]
fn collinear_corners_each_give_their_side_its_own_face() {
    // A 3 × 1 rectangle with two extra corners along its bottom: 6 sides, so 6 side faces
    // (three coplanar) and a top and bottom: 8 faces, volume 3.
    let p = profile(&[
        z(0.0, 0.0),
        z(1.0, 0.0),
        z(2.0, 0.0),
        z(3.0, 0.0),
        z(3.0, 1.0),
        z(0.0, 1.0),
    ]);
    let (b, s) = sound("collinear", extrude(&p, 1.0, None));
    assert_volume("collinear", s.volume, 3.0, 1e-12);
    assert_eq!(s.faces, 8);
    for (n, edge) in b.history.inputs[0].edges.iter().enumerate() {
        assert_eq!(edge.generated.len(), 1, "Edge{}", n + 1);
    }
}

#[test]
fn extreme_scales_measure_as_their_corners_say() {
    // 2 km square, 1.5e-7 thick and a kilometre tall.
    let huge = rect(-1e6, -1e6, 2e6, 2e6);
    let (_, s) = sound("thin", extrude(&huge, 1.5e-7, None));
    assert_volume("thin", s.volume, 4e12 * 1.5e-7, 1e-9);
    let (_, s) = sound("tall", extrude(&huge, 1e6, None));
    assert_volume("tall", s.volume, 4e18, 1e-9);
    // A micron square a kilometre out in every direction: its sides as stored (1e6 − 1e-6
    // rounds) are sx and sy. Extruded a kilometre, and turned about a line 2 km away.
    let lo = 1e6 - 1e-6;
    let (sx, sy) = (1e6 - lo, 1e6 - lo);
    let tiny = profile(&[
        [1e6, 1e6, 1e6],
        [lo, 1e6, 1e6],
        [lo, lo, 1e6],
        [1e6, lo, 1e6],
    ]);
    let (_, s) = sound("tiny tall", extrude(&tiny, 1e6, None));
    assert_volume("tiny tall", s.volume, sx * sy * 1e6, 1e-6);
}

#[test]
fn either_corner_order_and_either_axis_direction_give_the_same_positive_volume() {
    // A unit square from x = 1 about y: by Pappus, θ · 1.5.
    let ccw = rect(1.0, 0.0, 1.0, 1.0);
    let mut cw = ccw.clone();
    cw.points.reverse();
    for p in [&ccw, &cw] {
        for d in [Y, [0.0, -1.0, 0.0], [0.0, 7.0, 0.0]] {
            for angle in [90.0, 270.0, 360.0, 1e-3] {
                let what = format!("{:?} about {d:?} by {angle}", p.points[1]);
                let (_, s) = sound(&what, revolve(p, &axis([0.0; 3], d), angle));
                assert_volume(&what, s.volume, angle.to_radians() * 1.5, 1e-9);
            }
        }
    }
}

#[test]
fn a_side_nearly_parallel_to_the_axis_is_swept_into_a_cylinder_as_occt_does() {
    // OCCT revolves a line into a cylinder when its length exceeds its length along the
    // axis by 1e-7 or less (GeomAdaptor_SurfaceOfRevolution), which is second order in the
    // lean: this 100 mm wall leaning 3e-3 mm comes out 1.1e-4 of its volume too small, a
    // valid solid (docs/ports/extrude-revolve.md; FreeCAD's OCCT does the same). Accepted
    // as FreeCAD accepts it; pinned here so a change either way shows: worse than 2e-4,
    // or right (to 1e-6), means the note is out of date.
    let wall = profile(&[z(40.0, 0.0), z(50.0, 0.0), z(50.003, 100.0), z(40.0, 100.0)]);
    let (_, moment) = area_and_moment(&wall.points);
    let (_, s) = sound("wall", revolve(&wall, &axis([0.0; 3], Y), 360.0));
    let error = relative_error(s.volume, TAU * moment);
    assert!((1e-6..=2e-4).contains(&error), "relative error {error:e}");
}

#[test]
fn a_long_slanted_slender_prism_has_the_shape_its_rise_gives() {
    // A 1e-3 mm square rising 1000 along (0, 4.6, 1): its mesh encloses the square's area
    // times its rise, so the shape is right. (The summary measures it 6.1e-5 off, where
    // its faces' integrals cancel: carried over in docs/roadmap/m1-kernel.md.)
    let tiny = rect(0.0, 0.0, 1e-3, 1e-3);
    let (built, _) = sound("slender", extrude(&tiny, 1000.0, Some([0.0, 4.6, 1.0])));
    let enclosed = mesh_volume(&built.shape).unwrap();
    assert_volume("slender mesh", enclosed, 1e-6 * 1000.0, 1e-6);
}

#[test]
fn an_axis_leaning_off_the_plane_where_it_passes_the_profile_is_refused() {
    // The axis leans 1e-9 out of the profile's plane, and its point is 2 km from the
    // profile, so where it passes the profile it's 2e-3 mm off the plane: OCCT revolved
    // that into a tube with a 2e-3 hole, not the solid cylinder a side on the axis makes
    // (4 faces, not 3). Refused, as an axis not in the plane. The same axis without the
    // lean makes the cylinder.
    let far = rect(0.0, 1e6 - 1.0, 1.0, 1.0);
    let err = revolve(&far, &axis([0.0, -1e6, 0.0], [0.0, 1.0, 1e-9]), 360.0)
        .err()
        .unwrap();
    assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{err}");
    assert!(
        err.message().contains("must lie in the profile's plane"),
        "{err}"
    );
    let (_, s) = sound("straight", revolve(&far, &axis([0.0, -1e6, 0.0], Y), 360.0));
    assert_eq!(s.faces, 3);
    assert_volume("straight", s.volume, PI, 1e-9);
}

#[test]
fn a_thousand_corner_profile_extrudes_and_revolves_in_reasonable_time() {
    // A 1000-gon on the unit circle about (5, 0) (its corners evenly spaced by diamond
    // angle): a prism of its shoelace area, and a solid of revolution of 2π ∫x dA.
    let n = 1000u32;
    let points: Vec<[f64; 3]> = (0..n)
        .map(|i| {
            let (c, s) = direction(4.0 * f64::from(i) / f64::from(n));
            [5.0 + c, s, 0.0]
        })
        .collect();
    let p = profile(&points);
    let (area, moment) = area_and_moment(&points);
    let start = Instant::now();
    let (b, s) = sound("prism", extrude(&p, 1.0, None));
    assert_volume("prism", s.volume, area, 1e-9);
    assert_eq!(s.faces, n + 2);
    assert_eq!(b.history.inputs[0].edges.len(), n as usize);
    let (_, s) = sound("torus", revolve(&p, &axis([0.0; 3], Y), 360.0));
    assert_volume("torus", s.volume, TAU * moment, 1e-9);
    assert!(
        start.elapsed() < Duration::from_secs(60),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn a_revolve_a_hair_short_of_a_turn_is_the_turn_it_says() {
    // A unit square from x = 1 about y by 360 − ε: a partial turn with a slit between its
    // ends, as FreeCAD's is, and its volume θ · 1.5, once the slit where it's narrowest, at
    // its near corners, 1 off the axis, is wider than 1e-7 (ε above 1e-7 · 180 / π =
    // 5.730e-6 degrees: 6e-6 leaves 1.047e-7). Within OCCT's angular tolerance of a turn
    // (1e-12 radians, 5.73e-11 degrees), OCCT makes it a full one, without ends.
    for (angle, ends) in [
        (360.0 - 1e-5, (1, 1)),
        (360.0 - 6e-6, (1, 1)),
        (360.0 - 1e-12, (0, 0)),
        (359.999_999_999_999_94, (0, 0)),
    ] {
        let (b, s) = sound(
            &format!("{angle}"),
            revolve(&rect(1.0, 0.0, 1.0, 1.0), &axis([0.0; 3], Y), angle),
        );
        assert_volume(
            &format!("{angle}"),
            s.volume,
            angle.to_radians() * 1.5,
            1e-9,
        );
        let got = (b.history.ends.start.len(), b.history.ends.end.len());
        assert_eq!(got, ends, "{angle}");
    }
    // Short by less, the slit at the near corners is within the tolerance (5.7e-6 degrees
    // leaves 9.95e-8, 3e-6 5.2e-8, though 1.047e-7 at the far corners, 1e-6 1.7e-8): its
    // ends are one place there, which OCCT would build as two, so it's refused.
    for angle in [360.0 - 5.7e-6, 360.0 - 3e-6, 360.0 - 1e-6, 360.0 - 1e-10] {
        let err = revolve(&rect(1.0, 0.0, 1.0, 1.0), &axis([0.0; 3], Y), angle)
            .err()
            .unwrap_or_else(|| panic!("{angle}: built"));
        assert!(
            err.message().contains("its ends would be one place"),
            "{angle}: {err}"
        );
    }
}

#[test]
fn a_cut_across_where_a_nearly_full_turn_would_leave_its_gap_removes_what_it_covers() {
    // A ring from a unit square at x = 1..2 about y by 360 − 6e-6 leaves a slit of
    // 1.047e-7 at its near corners, about the narrowest the rules take (one within 1e-7 is
    // refused); a box (0.5 × 0.5 × 1 = 0.25) across it lies wholly inside the ring, so the
    // cut removes exactly 0.25. (OCCT 8.0.1 removed nothing, a regression from the 7.9.3
    // that Silico and FreeCAD's oracle run.)
    let ring = revolve(&rect(1.0, 0.0, 1.0, 1.0), &axis([0.0; 3], Y), 360.0 - 6e-6).unwrap();
    let block = silico_kernel::make(&silico_kernel::Primitive::Box {
        dx: 0.5,
        dy: 0.5,
        dz: 1.0,
    })
    .unwrap();
    let block = silico_kernel::translate(&block, [1.2, 0.2, -0.5])
        .unwrap()
        .shape;
    let before = summarize(&ring.shape).unwrap().volume;
    let cut = silico_kernel::boolean(silico_kernel::BooleanOp::Cut, &ring.shape, &block).unwrap();
    let s = summarize(&cut.shape).unwrap();
    assert!(s.valid);
    assert!(
        (before - s.volume - 0.25).abs() < 1e-6,
        "removed {}",
        before - s.volume
    );
}

#[test]
fn merged_corners_are_named_off_the_origin_and_off_the_xy_plane() {
    // In the plane z = y, the merged pair at (0, 1, 1): points OCCT measures as 1e-7 or
    // closer that the rules measure as over it. The pair is named, wherever it is.
    let a = [0.0, 1.0, 1.0];
    for b in [
        [
            9.999_948_325_612_022e-8,
            1.000_000_000_227_319_7,
            1.000_000_000_227_319_7,
        ],
        [
            9.999_940_022_129_681e-8,
            1.000_000_000_244_903_4,
            1.000_000_000_244_903_4,
        ],
        [
            9.999_934_743_215_577e-8,
            1.000_000_000_255_453_7,
            1.000_000_000_255_453_7,
        ],
    ] {
        let p = profile(&[a, b, [2.0, 0.2, 0.2], [2.0, -0.2, -0.2], [-1.0, -0.2, -0.2]]);
        assert!(p.check().is_ok(), "the rules pass it");
        let err = extrude(&p, 1.0, None).err().unwrap();
        assert!(err.message().contains("5 corners into 4 sides"), "{err}");
        assert!(err.message().contains("corners 1 and 2"), "{err}");
        // The profile's corners, as data; not a shape's elements.
        assert_eq!(
            err.refusal(),
            &silico_kernel::Refusal::CornersMerged {
                corners: 5,
                sides: 4,
                first: 1,
                second: 2
            }
        );
        assert!(err.elements().is_empty(), "{err}");
    }
}

#[test]
fn a_cut_across_a_large_nearly_full_ring_removes_what_it_covers() {
    // Rings of other sizes and angles where OCCT 8.0.1's cut removed nothing across the
    // slit (a stock build too, and FreeCAD 26.3 on it); OCCT 7.9.3, which Silico and
    // FreeCAD's oracle run, cuts them right. A 0.5 × 0.5 × 1 box across the slit, wholly
    // inside the ring: 0.25.
    for (x0, width, short, at) in [
        // At 1e-6 short the slit at x = 1 is 1.7e-8, one place, and refused (RR2).
        (1.0, 9.0, 6e-6, 1.2),
        (100.0, 100.0, 1e-7, 120.0),
        (100.0, 100.0, 3e-7, 120.0),
        (1000.0, 1000.0, 1e-8, 1200.0),
    ] {
        let ring = revolve(
            &rect(x0, 0.0, width, 1.0),
            &axis([0.0; 3], Y),
            360.0 - short,
        )
        .unwrap();
        let block = silico_kernel::make(&silico_kernel::Primitive::Box {
            dx: 0.5,
            dy: 0.5,
            dz: 1.0,
        })
        .unwrap();
        let block = silico_kernel::translate(&block, [at, 0.2, -0.5])
            .unwrap()
            .shape;
        let before = summarize(&ring.shape).unwrap().volume;
        let cut =
            silico_kernel::boolean(silico_kernel::BooleanOp::Cut, &ring.shape, &block).unwrap();
        let s = summarize(&cut.shape).unwrap();
        assert!(s.valid);
        let removed = before - s.volume;
        assert!(
            (removed - 0.25).abs() < 1e-5,
            "{x0}..{} by 360 − {short}: removed {removed}",
            x0 + width
        );
    }
}

#[test]
fn corners_occt_measures_as_one_are_refused_rather_than_misnumbered() {
    // The side from (0, 0) to (9.415e-8, 3.369e-8) measures a
    // hair over 1e-7 by the rules' arithmetic and exactly 1e-7 by OCCT's, which then
    // drops the corner: a four-corner profile made three edges, every name after the
    // short side shifted. Refused, naming what happened; first and as the closing side.
    let v = [9.415_276_395_598_258e-8, 3.369_357_564_060_285_6e-8, 0.0];
    let first = profile(&[[0.0; 3], v, [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]]);
    let closing = profile(&[v, [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0], [0.0; 3]]);
    for (p, corners, merged) in [
        (first, "corners 1 and 2", (1, 2)),
        (closing, "corners 4 and 1", (4, 1)),
    ] {
        assert!(p.check().is_ok(), "the rules pass it");
        let err = extrude(&p, 1.0, None).err().unwrap();
        assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{err}");
        assert!(err.message().contains("4 corners into 3 sides"), "{err}");
        assert!(
            err.message().contains("within its tolerance (1e-7 mm)"),
            "{err}"
        );
        assert!(err.message().contains(corners), "{err}");
        assert_eq!(
            err.refusal(),
            &silico_kernel::Refusal::CornersMerged {
                corners: 4,
                sides: 3,
                first: merged.0,
                second: merged.1
            }
        );
        assert!(err.elements().is_empty(), "{err}");
        assert!(err.suggestion().is_some());
    }
}

#[test]
fn an_axis_direction_with_the_largest_finite_component_is_just_a_direction() {
    // Its length overflowed once (f64::MAX squared), and the axis was refused for it; it's
    // the y axis. A unit square from x = 1 to 2, a quarter turn: π/2 × 1.5 (Pappus).
    let s = summarize(
        &revolve(
            &rect(1.0, 0.0, 1.0, 1.0),
            &axis([0.0; 3], [0.0, f64::MAX, 0.0]),
            90.0,
        )
        .unwrap()
        .shape,
    )
    .unwrap();
    assert_volume("f64::MAX", s.volume, 90f64.to_radians() * 1.5, 1e-9);
}

// ---- The worker survives what OCCT refuses ----

#[test]
fn the_worker_goes_on_after_occt_fails_or_its_result_is_refused() {
    // Each of these passes the worker's checks and is refused by OCCT or by the result
    // check; the worker must say so and keep working.
    let mut w = Worker::spawn(Path::new(WORKER)).unwrap();
    let timeout = Duration::from_secs(60);
    let bad = [
        Op::Extrude {
            profile: profile(&[z(0.0, 0.0), z(100.0, 1e-6), z(100.0, 0.0), z(0.0, 5e-7)]),
            length: 1.0,
            direction: None,
        },
        Op::Revolve {
            profile: rect(9e5, 0.0, 1e5, 1.0),
            axis: axis([0.0; 3], Y),
            angle: 5e-11,
        },
        Op::Revolve {
            profile: rect(9e5, 0.0, 1e5, 1.0),
            axis: axis([0.0; 3], Y),
            angle: 1e-10,
        },
        Op::Extrude {
            profile: rect(0.0, 0.0, 1.0, 1.0),
            length: f64::NAN,
            direction: None,
        },
    ];
    for op in bad {
        let name = format!("{op:?}");
        let err = w.call(Generation(1), op, timeout).unwrap_err();
        assert!(
            matches!(
                err.code(),
                ErrorCode::InvalidInput | ErrorCode::KernelFailed | ErrorCode::InvalidResult
            ),
            "{name}: {err}"
        );
        let good = Op::Extrude {
            profile: rect(0.0, 0.0, 1.0, 1.0),
            length: 1.0,
            direction: None,
        };
        assert!(w.call(Generation(1), good, timeout).is_ok(), "after {name}");
    }
}

// ---- Properties ----

/// The unit vector at diamond angle `u`: 0 to 4 for a turn counterclockwise from +x, a
/// quarter turn per unit, increasing with the angle (the point at `u` on the diamond
/// |x| + |y| = 1, scaled onto the circle). Silico's lints keep trigonometry out.
fn direction(u: f64) -> (f64, f64) {
    let u = u.rem_euclid(4.0);
    let (x, y) = if u < 1.0 {
        (1.0 - u, u)
    } else if u < 2.0 {
        (1.0 - u, 2.0 - u)
    } else if u < 3.0 {
        (u - 3.0, 2.0 - u)
    } else {
        (u - 3.0, u - 4.0)
    };
    let r = (x * x + y * y).sqrt();
    (x / r, y / r)
}

/// A star-shaped polygon around (0, 0): `n` corners at increasing angles (each step under
/// a half turn, so it's simple), at radii from 0.3 to 1.
fn star() -> impl Strategy<Value = Vec<(f64, f64)>> {
    (4usize..=12).prop_flat_map(|n| {
        (
            prop::collection::vec(0.5f64..1.0, n),
            prop::collection::vec(prop_oneof![Just(0.3), Just(1.0), 0.3f64..1.0], n),
            0.0f64..4.0,
        )
            .prop_map(|(steps, radii, start)| {
                let total: f64 = steps.iter().sum();
                let mut angle = start;
                steps
                    .iter()
                    .zip(radii)
                    .map(|(step, r)| {
                        let (c, s) = direction(angle);
                        angle += step / total * 4.0;
                        (r * c, r * s)
                    })
                    .collect()
            })
    })
}

/// The rotation a quaternion of any length but zero gives, applied to `p`.
fn rotate(p: [f64; 3], q: [f64; 4]) -> [f64; 3] {
    let n = q.iter().map(|c| c * c).sum::<f64>().sqrt();
    let [w, x, y, z] = q.map(|c| c / n);
    let row = |a: f64, b: f64, c: f64| a * p[0] + b * p[1] + c * p[2];
    [
        row(
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ),
        row(
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ),
        row(
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ),
    ]
}

/// No rotation, or a general one.
fn turn() -> impl Strategy<Value = Option<[f64; 4]>> {
    prop::option::of(
        prop::array::uniform4(-1.0f64..1.0).prop_filter("a quaternion of some length", |q| {
            q.iter().map(|c| c * c).sum::<f64>() > 0.01
        }),
    )
}

fn scale() -> impl Strategy<Value = f64> {
    prop::sample::select(vec![1e-3, 1.0, 37.5, 1e3])
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1000,
        failure_persistence: None,
        // Fixed, so a red run is red every time.
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x5111_c0b0),
        ..ProptestConfig::default()
    })]

    #[test]
    fn an_extrusion_has_the_profiles_area_times_its_rise(
        corners in star(),
        s in scale(),
        length in prop_oneof![Just(1.1e-7), Just(1.0), Just(1e3), 1e-4f64..1e2],
        slant in prop::option::of(prop::array::uniform2(prop_oneof![Just(0.0), -5.0f64..5.0])),
        angles in turn(),
        offset in prop::array::uniform3(prop_oneof![Just(0.0), -1e4f64..1e4]),
    ) {
        let flat: Vec<[f64; 3]> = corners.iter().map(|&(x, y)| [x * s, y * s, 0.0]).collect();
        let (area, _) = area_and_moment(&flat);
        let place = |p: [f64; 3]| {
            let p = angles.map_or(p, |a| rotate(p, a));
            [p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]]
        };
        let p = profile(&flat.iter().map(|&q| place(q)).collect::<Vec<_>>());
        let direction = slant.map(|[a, b]| {
            let d = [a, b, 1.0];
            angles.map_or(d, |r| rotate(d, r))
        });
        let what = format!("{p:?} by {length} along {direction:?}");
        // Prisms that travel more than 1e4 times their width are measured wrongly by
        // summarize (a_long_slanted_slender_prism_has_the_shape_its_rise_gives checks the
        // shape; the measuring is carried over): leave them out.
        let rise_factor = slant.map_or(1.0, |[a, b]| (1.0 + a * a + b * b).sqrt());
        prop_assume!(length * rise_factor / (0.3 * s) < 1e4);
        let built = extrude(&p, length, direction);
        let built = built.map_err(|e| format!("{what}: {e}")).unwrap();
        let summary = summarize(&built.shape).unwrap();
        prop_assert!(summary.valid, "{}", what);
        // Rotated corners far out are rounded to about 1e-16 of their distance, which moves
        // a thin or small prism's volume by that over its height and width.
        let far = offset.iter().fold(1.0f64, |m, c| m.max(c.abs())) + s;
        let rounding = 8.0 * f64::EPSILON * far * (1.0 / length + 1.0 / (0.3 * s));
        let close = |v: f64| relative_error(v, area * length) <= 1e-8 + rounding;
        // Each measure is wrong somewhere the other is right (summarize for slender slanted
        // prisms, the mesh for thin ones: see the tests above); a wrong solid fails both.
        let meshed = mesh_volume(&built.shape);
        prop_assert!(close(summary.volume) || meshed.is_some_and(close),
            "{}: summary {} mesh {:?} want {}", what, summary.volume, meshed, area * length);
        prop_assert_eq!(summary.faces as usize, corners.len() + 2, "{}", what);
        prop_assert_eq!(built.history.validate(1), Ok(()));
        prop_assert_eq!((built.history.ends.start.len(), built.history.ends.end.len()), (1, 1));
    }

    #[test]
    fn a_revolve_has_the_volume_pappus_gives(
        corners in star(),
        s in scale(),
        gap in prop_oneof![Just(0.0), Just(5e-8), Just(2e-7), Just(1e-6)],
        far in prop_oneof![Just(false), Just(true)],
        angle in prop_oneof![Just(360.0), Just(90.0), Just(180.0), Just(359.0), Just(1e-3), 1e-2f64..360.0],
        angles in turn(),
        offset in prop::array::uniform3(prop_oneof![Just(0.0), -1e3f64..1e3]),
    ) {
        // The star moved so its leftmost corner is `gap` right of the y axis (within the
        // tolerance of it, or just past), or, when `far`, its own size plus that.
        let left = corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
        let shift = if far { s } else { 0.0 } + gap;
        let flat: Vec<[f64; 3]> = corners
            .iter()
            .map(|&(x, y)| [(x - left) * s + shift, y * s, 0.0])
            .collect();
        let (area, moment) = area_and_moment(&flat);
        let want = angle.to_radians() * moment.abs();
        prop_assert!(area.abs() > 0.0);
        // A side OCCT would wrongly sweep into a cylinder (its length within 1e-7 of its
        // length along the axis, though not parallel to it) has its own test above
        // (a_side_nearly_parallel_to_the_axis_is_swept_into_a_cylinder_as_occt_does); leave
        // those, with a margin, to it.
        let n = flat.len();
        prop_assume!((0..n).all(|i| {
            let (a, b) = (flat[i], flat[(i + 1) % n]);
            let along = (b[1] - a[1]).abs();
            let length = ((b[0] - a[0]) * (b[0] - a[0]) + (b[1] - a[1]) * (b[1] - a[1])).sqrt();
            b[0] == a[0] || length - along > 1e-6
        }));
        // Turns that move the profile less than (ten times) the tolerance are refused, rightly.
        let reach = flat.iter().map(|p| p[0].abs()).fold(0.0, f64::max);
        prop_assume!(reach * angle.to_radians() > 1e-6);
        // So are turns short of a whole one whose slit is within the tolerance where it's
        // narrowest: at the nearest corner off the axis, or the farthest if none is
        // (a_revolves_slit_is_measured_at_the_profiles_nearest_corner).
        let at = flat
            .iter()
            .map(|p| p[0].abs())
            .filter(|&d| d > 1e-7)
            .fold(reach, f64::min);
        prop_assume!(angle == 360.0 || at * (360.0 - angle).to_radians() > 1e-7);
        let place = |p: [f64; 3]| {
            let p = angles.map_or(p, |a| rotate(p, a));
            [p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]]
        };
        let p = profile(&flat.iter().map(|&q| place(q)).collect::<Vec<_>>());
        let a = axis(place([0.0; 3]), angles.map_or(Y, |r| rotate(Y, r)));
        let what = format!("{p:?} about {a:?} by {angle}");
        let built = revolve(&p, &a, angle).map_err(|e| format!("{what}: {e}")).unwrap();
        let summary = summarize(&built.shape).unwrap();
        prop_assert!(summary.valid, "{}", what);
        prop_assert!(relative_error(summary.volume, want) <= 1e-7,
            "{}: volume {} want {}", what, summary.volume, want);
        prop_assert_eq!(built.history.validate(1), Ok(()));
        let ends = (built.history.ends.start.len(), built.history.ends.end.len());
        prop_assert_eq!(ends, if angle == 360.0 { (0, 0) } else { (1, 1) }, "{}", what);
    }
}
