//! M5.9, hostile face frames for the kernel's faces (`/break-it`): frames at the edge of
//! `FaceFrame::check`'s tolerances with sketches as far out as it allows (a kilometre square,
//! a kilometre half disc), and frames just past them refused, rotated frames a kilometre
//! out, arcs (clockwise and counter-clockwise) on reversed and tilted frames, frames the
//! protocol refuses, and `describe`'s plane location on every kind of planar face the kernel
//! makes. Each area and point is worked by hand or by a rotation's properties.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::close;

use std::f64::consts::{FRAC_PI_2, PI};

use close::assert_close;
use proptest::prelude::*;
use silico_kernel::{
    Axis, Built, ChamferSize, CopiesOp, Curve, FaceFrame, FaceRegions, KernelErrorKind, Placement,
    Region, SketchPlane, chamfer, copies, describe_whole, make_box, make_faces, prism, revolution,
    summarize, to_brep,
};
use silico_protocol::{Description, FACE_FRAME_SQUARENESS, Refusal, Surface};

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let l = libm::sqrt(dot(v, v));
    v.map(|c| c / l)
}

/// The rotation by `angle` about the unit `k` (Rodrigues), as a frame from `origin` (its
/// first two columns), and its normal, its third column.
fn rotation(k: [f64; 3], angle: f64, origin: [f64; 3]) -> (FaceFrame, [f64; 3]) {
    let (s, c) = (libm::sin(angle), libm::cos(angle));
    let column = |e: [f64; 3]| -> [f64; 3] {
        let kxe = cross(k, e);
        let ke = dot(k, e);
        [0, 1, 2].map(|i| e[i] * c + kxe[i] * s + k[i] * ke * (1.0 - c))
    };
    let frame = FaceFrame {
        origin,
        x: column([1.0, 0.0, 0.0]),
        y: column([0.0, 1.0, 0.0]),
    };
    (frame, column([0.0, 0.0, 1.0]))
}

fn line(start: [f64; 2], end: [f64; 2]) -> Curve {
    Curve::Line { start, end }
}

fn rectangle(x: f64, y: f64, w: f64, h: f64) -> Vec<Curve> {
    let p = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    (0..4).map(|i| line(p[i], p[(i + 1) % 4])).collect()
}

fn on(frame: FaceFrame, outer: Vec<Curve>, holes: Vec<Vec<Curve>>) -> FaceRegions {
    FaceRegions {
        plane: SketchPlane::Face(frame),
        regions: vec![Region { outer, holes }],
    }
}

fn point_off(got: [f64; 3], want: [f64; 3]) -> f64 {
    (0..3).map(|k| (got[k] - want[k]).abs()).fold(0.0, f64::max)
}

/// Made, valid, and described.
fn made(regions: &FaceRegions) -> (Built, Description) {
    let built = make_faces(regions).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(summarize(&built.shape).unwrap().valid, "invalid faces");
    let d = describe_whole(&built.shape).unwrap();
    d.validate(1).unwrap();
    (built, d)
}

/// The plane of each planar face: its normal and its own location, and its centroid.
fn planes(d: &Description) -> Vec<([f64; 3], [f64; 3], [f64; 3])> {
    d.faces
        .iter()
        .filter_map(|f| match f.surface {
            Surface::Plane { normal, location } => Some((normal, location, f.centroid)),
            _ => None,
        })
        .collect()
}

/// Axis-aligned frames at the edge of `FaceFrame::check`'s tolerances: x and y each 4ε
/// (`UNIT_SLACK`) long or short of a unit, and y leaning 1e-14 (`FACE_FRAME_SQUARENESS`)
/// towards x or away, from (0, 0, 0) and from (0, 0, 5).
fn frames_at_the_edge() -> Vec<FaceFrame> {
    let slack = 4.0 * f64::EPSILON;
    let lean = FACE_FRAME_SQUARENESS;
    let up = |x: [f64; 3], y: [f64; 3], z: f64| FaceFrame {
        origin: [0.0, 0.0, z],
        x,
        y,
    };
    vec![
        up([1.0 + slack, 0.0, 0.0], [0.0, 1.0, 0.0], 0.0),
        up([1.0 - slack, 0.0, 0.0], [0.0, 1.0 + slack, 0.0], 5.0),
        up([1.0, 0.0, 0.0], [lean, 1.0, 0.0], 0.0),
        // Both at once: y leaning a hair under 1e-14, as x's 4ε stretch multiplies it.
        up(
            [1.0 + slack, 0.0, 0.0],
            [-0.99 * lean, 1.0 - slack, 0.0],
            5.0,
        ),
    ]
}

#[test]
fn a_frame_at_the_edge_of_its_tolerances_with_a_kilometre_sketch_makes_valid_faces() {
    // Broke (break-it), before the fix: FaceFrame::check admitted a normal 0.9e-12 off
    // x × y (its tolerance was 1e-12), and FaceRegions::check a sketch point 1e6 from the
    // sketch's origin (MAX_LENGTH). The kernel placed the points along x and y but built the
    // face's Geom_Plane from the normal, so the far corners were 1e6 · 0.9e-12 = 9e-7 mm off
    // the plane, nine times OCCT's 1e-7: the request the protocol accepted came back
    // ResultInvalid. Now a frame has no normal of its own (it's x × y), and its axes are
    // unit to 4ε and square to 1e-14: a kilometre square is made, valid, on every frame at
    // the edge of those.
    for frame in frames_at_the_edge() {
        assert_eq!(frame.check(), Ok(()), "{frame:?}");
        let built = make_faces(&on(frame, rectangle(0.0, 0.0, 1e6, 1e6), vec![]))
            .unwrap_or_else(|e| panic!("{frame:?}: {e:?}"));
        let s = summarize(&built.shape).unwrap();
        assert!(s.valid, "{frame:?}");
        assert_close("a kilometre square's area", s.area, 1e12, 1e-9);
    }
}

/// A half disc of `r` about the sketch's origin: an arc from angle `from` to `from` + π,
/// `from` 0 (from (r, 0) to (−r, 0)) or −π/2 (from (0, −r) to (0, r)), closed by its
/// diameter.
fn half_disc(r: f64, from: f64) -> Vec<Curve> {
    let (start, end) = if from == 0.0 {
        ([r, 0.0], [-r, 0.0])
    } else {
        ([0.0, -r], [0.0, r])
    };
    vec![
        Curve::Arc {
            center: [0.0, 0.0],
            start,
            end,
            radius: r,
            start_angle: from,
            end_angle: from + PI,
        },
        line(end, start),
    ]
}

#[test]
fn a_kilometre_half_disc_on_a_frame_at_the_edge_of_its_tolerances_makes_valid_faces() {
    // The robustness review's case: a half disc of radius 9.99e5 (an arc from 0 to π,
    // closed by a line), and the same turned a quarter (−π/2 to π/2), so its ends lie along
    // the frame's x and then its y. OCCT builds the arc on its own frame, x normalized and
    // y = n × x, and its ends where that frame puts them; the vertices are where the
    // frame's own x and y put them. At the edge of the tolerances the two are within
    // 9.99e5 · 4ε ≈ 9e-10 (a stretched axis) and 9.99e5 · 1e-14 ≈ 1e-8 (a lean): inside
    // OCCT's 1e-7, so valid faces of area π r² / 2.
    let r = 9.99e5;
    for frame in frames_at_the_edge() {
        for from in [0.0, -FRAC_PI_2] {
            let regions = on(frame, half_disc(r, from), vec![]);
            let built = make_faces(&regions).unwrap_or_else(|e| panic!("{frame:?}: {e:?}"));
            let s = summarize(&built.shape).unwrap();
            assert!(s.valid, "{frame:?} from {from}");
            assert_close("a half disc's area", s.area, PI * r * r / 2.0, 1e-9);
        }
    }
    // Within the old rule (each axis unit and the pair square to 1e-12) but past the new:
    // x 0.9e-12 long, or y leaning 0.9e-12 towards x. Placed 9.99e5 out, the arc's ends are
    // 9e-7 from OCCT's, past its 1e-7: the old rule took them, and OCCT made faces BRepCheck
    // found invalid, a ResultInvalid naming no frame. Now each is refused first, naming the
    // plane.
    for frame in [
        FaceFrame {
            origin: [0.0; 3],
            x: [1.0 + 0.9e-12, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        },
        FaceFrame {
            origin: [0.0; 3],
            x: [1.0, 0.0, 0.0],
            y: [0.9e-12, 1.0, 0.0],
        },
    ] {
        for from in [0.0, -FRAC_PI_2] {
            let Err(e) = make_faces(&on(frame, half_disc(r, from), vec![])) else {
                panic!("{frame:?} from {from} was taken")
            };
            assert_eq!(e.kind(), KernelErrorKind::InvalidInput, "{frame:?}: {e:?}");
            assert!(
                matches!(e.refusal(), Refusal::InvalidParameter { parameter, .. } if parameter == "plane"),
                "{frame:?}: {e:?}"
            );
        }
    }
}

#[test]
fn a_rotated_frame_a_kilometre_out_makes_a_kilometre_square_on_its_plane() {
    // An exact rotation (37° about (1, 2, 3)), its origin a kilometre out on each axis's
    // reach, a 2 km square about the sketch's origin: 4e12 mm², its centre the frame's
    // origin, the plane located exactly there.
    let (frame, n) = rotation(unit([1.0, 2.0, 3.0]), 37f64.to_radians(), [1e6, -1e6, 5e5]);
    assert_eq!(frame.check(), Ok(()));
    let (_, d) = made(&on(frame, rectangle(-1e6, -1e6, 2e6, 2e6), vec![]));
    let face = &d.faces[0];
    assert_close("the square's area", face.area, 4e12, 1e-9);
    assert!(
        point_off(face.centroid, frame.origin) <= 1e-3,
        "{:?}",
        face.centroid
    );
    let Surface::Plane { normal, location } = face.surface else {
        panic!("{:?}", face.surface)
    };
    assert!(point_off(normal, n) <= 1e-15, "{normal:?}");
    assert_eq!(location, frame.origin);
    // Each corner where the frame puts it.
    for p in [[-1e6, -1e6], [1e6, -1e6], [1e6, 1e6], [-1e6, 1e6]] {
        let want = SketchPlane::Face(frame).place(p);
        assert!(
            d.vertices.iter().any(|v| point_off(*v, want) <= 1e-6),
            "{want:?} not among {:?}",
            d.vertices
        );
    }
}

/// A slot (two lines and two counter-clockwise half-circle arcs of `r` about (0, 0) and
/// (`l`, 0)), less a hole of two clockwise half-circle arcs of `h` about (`l` / 2, 0):
/// 2·r·l + π r² − π h².
fn slot_with_a_round_hole(l: f64, r: f64, h: f64) -> (Vec<Curve>, Vec<Curve>, f64) {
    let outer = vec![
        line([0.0, -r], [l, -r]),
        Curve::Arc {
            center: [l, 0.0],
            start: [l, -r],
            end: [l, r],
            radius: r,
            start_angle: -FRAC_PI_2,
            end_angle: FRAC_PI_2,
        },
        line([l, r], [0.0, r]),
        Curve::Arc {
            center: [0.0, 0.0],
            start: [0.0, r],
            end: [0.0, -r],
            radius: r,
            start_angle: FRAC_PI_2,
            end_angle: 3.0 * FRAC_PI_2,
        },
    ];
    let c = l / 2.0;
    let hole = vec![
        Curve::Arc {
            center: [c, 0.0],
            start: [c + h, 0.0],
            end: [c - h, 0.0],
            radius: h,
            start_angle: 0.0,
            end_angle: -PI,
        },
        Curve::Arc {
            center: [c, 0.0],
            start: [c - h, 0.0],
            end: [c + h, 0.0],
            radius: h,
            start_angle: PI,
            end_angle: 0.0,
        },
    ];
    (outer, hole, 2.0 * r * l + PI * r * r - PI * h * h)
}

#[test]
fn arcs_both_ways_on_reversed_and_tilted_frames_bound_the_slot_they_say() {
    let (outer, hole, area) = slot_with_a_round_hole(10.0, 3.0, 2.0);
    let frames = [
        // A box's top reversed, at z = 7: n −Z.
        (
            FaceFrame {
                origin: [0.0, 0.0, 7.0],
                x: [-1.0, 0.0, 0.0],
                y: [0.0, 1.0, 0.0],
            },
            [0.0, 0.0, -1.0],
        ),
        // Tilted every way, half a kilometre out.
        rotation(unit([-2.0, 1.0, 0.5]), 2.3, [5e5, -3e5, 2e5]),
        // Upside down about X: y −Y, n −Z.
        rotation([1.0, 0.0, 0.0], PI, [1.0, 2.0, 3.0]),
    ];
    for (frame, n) in frames {
        assert_eq!(frame.check(), Ok(()));
        let regions = on(frame, outer.clone(), vec![hole.clone()]);
        let (built, d) = made(&regions);
        assert_eq!(d.faces.len(), 1);
        assert_close("the slot's area", d.faces[0].area, area, 1e-9);
        // The face faces along the frame's normal.
        let Surface::Plane { normal, .. } = d.faces[0].surface else {
            panic!("{:?}", d.faces[0].surface)
        };
        assert!(point_off(normal, n) <= 1e-12, "{frame:?}: {normal:?}");
        // Every vertex where the frame places a sketch point: the slot's four corners and
        // the hole's two ends.
        let plane = SketchPlane::Face(frame);
        for p in [
            [0.0, -3.0],
            [10.0, -3.0],
            [10.0, 3.0],
            [0.0, 3.0],
            [7.0, 0.0],
            [3.0, 0.0],
        ] {
            let want = plane.place(p);
            assert!(
                d.vertices.iter().any(|v| point_off(*v, want) <= 1e-9),
                "{frame:?}: {want:?} not among {:?}",
                d.vertices
            );
        }
        // Its centre of area: the slot's and the hole's are both sketch (5, 0), so the
        // face's is (5, 0) placed. (An arc bulging the wrong way would move it, or change
        // the area.)
        assert!(
            point_off(d.faces[0].centroid, plane.place([5.0, 0.0])) <= 1e-6,
            "{frame:?}: {:?}",
            d.faces[0].centroid
        );
        // Padded 2 along the normal: 2 × area.
        let pad = prism(&built.shape, [0.0; 3], n.map(|c| 2.0 * c)).unwrap();
        let s = summarize(&pad.shape).unwrap();
        assert!(s.valid, "{frame:?}");
        assert_close("the pad's volume", s.volume, 2.0 * area, 1e-9);
    }
}

#[test]
fn a_circle_on_a_tilted_frame_has_the_box_of_a_tilted_circle() {
    // A circle of r 5 about the frame's origin, the frame tilted 30° about X: the circle's
    // box reaches r·√(1 − n_k²) along each axis k (n = (0, −½, √3/2)): 5 along X, 5·√3/2
    // along Y, 5·½ along Z.
    let (frame, n) = rotation([1.0, 0.0, 0.0], 30f64.to_radians(), [0.0; 3]);
    let (built, _) = made(&on(
        frame,
        vec![Curve::Circle {
            center: [0.0, 0.0],
            radius: 5.0,
        }],
        vec![],
    ));
    let [low, high] = summarize(&built.shape).unwrap().bounds.unwrap();
    let reach = n.map(|n| 5.0 * libm::sqrt(1.0 - n * n));
    assert!(point_off(high, reach) <= 1e-6, "{high:?} vs {reach:?}");
    assert!(point_off(low, reach.map(|c| -c)) <= 1e-6, "{low:?}");
}

#[test]
fn frames_the_protocol_refuses_are_refused_before_occt_naming_the_plane() {
    let (good, _) = rotation([0.0, 0.0, 1.0], 0.5, [1.0, 2.0, 3.0]);
    let bad = [
        FaceFrame {
            origin: [f64::NAN, 0.0, 0.0],
            ..good
        },
        FaceFrame {
            x: [f64::INFINITY, 0.0, 0.0],
            ..good
        },
        FaceFrame {
            y: good.y.map(|c| c * 2.0),
            ..good
        },
        FaceFrame {
            origin: [0.0, 0.0, 2e6],
            ..good
        },
        FaceFrame { x: good.y, ..good },
        // x and y each a unit, 2e-14 off square.
        FaceFrame {
            x: [1.0, 0.0, 0.0],
            y: [2e-14, 1.0, 0.0],
            ..good
        },
    ];
    for frame in bad {
        let Err(e) = make_faces(&on(frame, rectangle(0.0, 0.0, 1.0, 1.0), vec![])) else {
            panic!("{frame:?} was taken")
        };
        assert_eq!(e.kind(), KernelErrorKind::InvalidInput, "{frame:?}: {e:?}");
        assert!(
            matches!(e.refusal(), Refusal::InvalidParameter { parameter, .. } if parameter == "plane"),
            "{frame:?}: {e:?}"
        );
    }
    // And the next request, a sound one, is made.
    made(&on(good, rectangle(0.0, 0.0, 1.0, 1.0), vec![]));
}

#[test]
fn every_planar_faces_own_location_lies_on_its_plane() {
    // The plane's location as describe reports it is a point of the plane: along its
    // normal, the same offset as its centre of area, on every kind of planar face: a box
    // chamfered (a new slanted plane), a rotated copy fused on (planes moved by a
    // location), a mirrored one (planes rebuilt), the caps of a pad and of a part turn of
    // faces on a tilted frame.
    let (frame, n) = rotation(unit([1.0, 1.0, 0.0]), 0.7, [40.0, -20.0, 15.0]);
    let faces = make_faces(&on(frame, rectangle(1.0, 1.0, 4.0, 3.0), vec![]))
        .unwrap()
        .shape;
    let block = make_box(10.0, 20.0, 30.0).unwrap();
    let mut shapes = vec![
        (
            "chamfered box",
            chamfer(&block, &[1], ChamferSize::Equal { distance: 2.0 }, false)
                .unwrap()
                .shape,
        ),
        (
            "pad on a tilted frame",
            prism(&faces, [0.0; 3], n.map(|c| 3.0 * c)).unwrap().shape,
        ),
        (
            "part turn on a tilted frame",
            revolution(
                &faces,
                &Axis {
                    origin: frame.origin,
                    direction: frame.y,
                },
                0.0,
                1.1,
            )
            .unwrap()
            .shape,
        ),
    ];
    let far = Axis {
        origin: [100.0, 50.0, 0.0],
        direction: unit([0.3, -0.2, 1.0]),
    };
    for (what, placement) in [
        (
            "rotated copy",
            Placement::Rotation {
                axis: far,
                angle: 0.9,
            },
        ),
        (
            "mirrored copy",
            Placement::Mirror {
                origin: [60.0, 0.0, 0.0],
                normal: unit([1.0, 0.2, 0.1]),
            },
        ),
    ] {
        let fused = copies(CopiesOp::Fuse, &block, &block, &[placement])
            .unwrap_or_else(|e| panic!("{what}: {e:?}"));
        shapes.push((what, fused.shape));
    }
    for (what, shape) in shapes {
        let d = describe_whole(&shape).unwrap();
        let found = planes(&d);
        assert!(!found.is_empty(), "{what}");
        for (i, (normal, location, centroid)) in found.into_iter().enumerate() {
            let gap = dot(normal, location) - dot(normal, centroid);
            assert!(
                gap.abs() <= 1e-9,
                "{what}, plane {i}: location {location:?} is {gap} off the plane through {centroid:?}"
            );
        }
    }
}

#[test]
fn the_same_frame_request_makes_the_same_bytes() {
    let (frame, _) = rotation(unit([3.0, -1.0, 2.0]), 1.234, [12.5, -7.25, 1e5]);
    let (outer, hole, _) = slot_with_a_round_hole(10.0, 3.0, 2.0);
    let regions = on(frame, outer, vec![hole]);
    let a = to_brep(&make_faces(&regions).unwrap().shape).unwrap();
    let b = to_brep(&make_faces(&regions).unwrap().shape).unwrap();
    assert_eq!(a, b);
}

#[test]
fn a_frame_a_kilometre_out_with_a_kilometre_sketch_places_points_past_the_reach() {
    // The frame's origin a kilometre out on each axis (√3 · 1e6 from the model's origin,
    // the most `FaceFrame::check` allows), and the sketch's points within a kilometre of
    // it: placed, a point is up to 1e6 · (√3 + √2) from the model's origin, past the reach
    // every other request keeps to. The kernel makes it, valid: the core refuses a profile
    // that reaches past a kilometre (docs/ports/attacher.md, the frame), not the kernel.
    let (frame, _) = rotation(unit([1.0, 1.0, 1.0]), 0.6, [1e6, 1e6, 1e6]);
    assert_eq!(frame.check(), Ok(()));
    let regions = on(frame, rectangle(0.0, 0.0, 1e6, 1e6), vec![]);
    let built = make_faces(&regions).unwrap_or_else(|e| panic!("{e:?}"));
    let s = summarize(&built.shape).unwrap();
    assert!(s.valid);
    assert_close("the far square's area", s.area, 1e12, 1e-9);
    let [_, high] = s.bounds.unwrap();
    // Past a kilometre in some coordinate.
    assert!(high.iter().any(|c| *c > 1e6), "{high:?}");
}

fn axis() -> impl Strategy<Value = [f64; 3]> {
    prop_oneof![
        Just([1.0, 0.0, 0.0]),
        Just([0.0, 1.0, 0.0]),
        Just([0.0, 0.0, 1.0]),
        Just(unit([1.0, 1.0, 1.0])),
        (-1.0..1.0f64, -1.0..1.0f64, -1.0..1.0f64)
            .prop_filter("not tiny", |(a, b, c)| a * a + b * b + c * c > 1e-6)
            .prop_map(|(a, b, c)| unit([a, b, c])),
    ]
}

fn angle() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(1e-12),
        Just(FRAC_PI_2),
        Just(PI),
        (-1e-9..1e-9f64).prop_map(|d| FRAC_PI_2 + d),
        -PI..PI,
    ]
}

fn coordinate() -> impl Strategy<Value = f64> {
    prop_oneof![Just(0.0), Just(1e6), Just(-1e6), Just(1e-3), -1e6..1e6f64]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Any rotation's frame, anywhere within a kilometre, a rectangle and a circle hole on
    /// it, sketched anywhere within a kilometre less their size: valid faces of the
    /// rectangle's area less the circle's, centred where the frame places the sketch's
    /// centre of area, on a plane through the frame's origin.
    #[test]
    fn faces_on_any_rotated_frame_are_valid_and_where_the_frame_puts_them(
        k in axis(), a in angle(),
        o in [coordinate(), coordinate(), coordinate()],
        at in [-9.9e5..9.9e5f64, -9.9e5..9.9e5f64],
        size in prop_oneof![Just(1.0), Just(1e-3), 1e-2..1e4f64],
    ) {
        let (frame, n) = rotation(k, a, o);
        prop_assert_eq!(frame.check(), Ok(()));
        let (w, h) = (size * 2.0, size);
        let mut outer = rectangle(at[0], at[1], w, h);
        if at[0] + w > 1e6 || at[1] + h > 1e6 {
            outer = rectangle(at[0] - w, at[1] - h, w, h);
        }
        let Curve::Line { start: corner, .. } = outer[0] else { unreachable!() };
        let middle = [corner[0] + w / 2.0, corner[1] + h / 2.0];
        let hole = vec![Curve::Circle { center: middle, radius: h / 4.0 }];
        let regions = on(frame, outer, vec![hole]);
        let built = make_faces(&regions);
        let built = match built {
            Ok(built) => built,
            Err(e) => return Err(TestCaseError::fail(format!("{frame:?}: {e:?}"))),
        };
        let s = summarize(&built.shape).unwrap();
        prop_assert!(s.valid, "{:?}", frame);
        let want = w * h - PI * h * h / 16.0;
        // Placing rounds each corner to the grid of up to 2.4e6 mm (an ulp of 4.7e-10), so each
        // side may be off by about 1e-9: a millimetre square far out is off by 1e-7 of its
        // area from the sketch's, a rounding an origin plane, placing exactly, doesn't have.
        let rounding = 2.0 * (w + h) * 1e-9;
        prop_assert!((s.area - want).abs() <= 1e-9 * want + rounding, "{} vs {}", s.area, want);
        let d = describe_whole(&built.shape).unwrap();
        let Surface::Plane { normal, location } = d.faces[0].surface else {
            panic!("{:?}", d.faces[0].surface)
        };
        prop_assert_eq!(location, frame.origin.map(|c| c + 0.0));
        prop_assert!(point_off(normal, n) <= 1e-12, "{:?}", normal);
        // Its centre of area: the rectangle's and the hole's are both its middle, so the
        // face's is the middle, o + u·x + v·y, to the measure's precision (1e-9 of the
        // coordinates it integrates, up to 2.4e6 out).
        let want = [0, 1, 2].map(|k| o[k] + middle[0] * frame.x[k] + middle[1] * frame.y[k]);
        let scale = libm::sqrt(dot(want, want)).max(1.0);
        prop_assert!(
            point_off(d.faces[0].centroid, want) <= 1e-9 * scale,
            "{:?} vs {:?}", d.faces[0].centroid, want
        );
        // Each corner where the frame places it, to the rounding of a kilometre's reach.
        let plane = SketchPlane::Face(frame);
        for p in [corner, [corner[0] + w, corner[1]], [corner[0] + w, corner[1] + h], [corner[0], corner[1] + h]] {
            let want = plane.place(p);
            prop_assert!(
                d.vertices.iter().any(|v| point_off(*v, want) <= 1e-8),
                "{:?} not among {:?}", want, d.vertices
            );
        }
    }
}
