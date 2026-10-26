//! Hostile pairs for M4.5's `distance` and `angle` (`silico_core::command::measure`), on real
//! solids rebuilt on kernel workers: carriers given in either order, axes parallel to the
//! last bit and crossing, a vertex far along a cylinder's axis, obtuse corners, an element
//! against itself, degenerated and seam edges, faces a nanodegree apart, and
//! the gate's kilometre. Every expected value by hand from the solids' dimensions.

use super::measuring::{
    along, angled, at, block, cylinder, engine, facing, measured, mm, model, named, point, polygon,
    revolve, the,
};

use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::command::elements::ElementType;
use silico_core::command::measure::{AngleAnswer, angle, distance};
use silico_core::document::{Angle, FeatureId, Length};
use silico_core::kernel::ElementKind;
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute};

fn sphere(id: &str, at: [f64; 3], radius: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "sphere", "v": 1, "name": "S{id}", "position": [{}, {}, {}], "radius": {radius}}}"#,
        at[0], at[1], at[2]
    )
}

fn pad(id: &str, sketch: &str, length: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "pad", "v": 1, "name": "P{id}", "sketch": "{sketch}", "extent": {{"type": "one_side", "length": {length}, "reversed": false}}}}"#
    )
}

fn deg(whole: i64) -> Angle {
    Angle::from_ndeg(whole * 1_000_000_000)
}

#[test]
fn an_edge_and_the_face_its_parallel_to_are_as_far_apart_normally_either_way_round() {
    // A 10 mm cube. Its bottom front edge, (0, 0, 0) to (10, 0, 0), is parallel to its top
    // (z = 10), 10 below it: the normal distance between the edge's line and the top's
    // plane is 10, whichever is named first (angle 0 either way). Its top front edge,
    // (0, 0, 10) to (10, 0, 10), lies in the top: 0 normal, either way round.
    let e = engine();
    let m = model(&e, &[block("bbbbbbbb", [0.0; 3], [10.0; 3])]);
    let top = named(&m, "bbbbbbbb", facing([0.0, 0.0, 1.0]));
    let below = named(&m, "bbbbbbbb", along([0.0; 3], [10.0, 0.0, 0.0]));
    let in_it = named(&m, "bbbbbbbb", along([0.0, 0.0, 10.0], [10.0, 0.0, 10.0]));
    assert_eq!(angled(&m, &top, &below).angle, deg(0));
    assert_eq!(angled(&m, &below, &top).angle, deg(0));
    // The face first: the edge's middle, (5, 0, 0), is 10 from the top's plane.
    assert_eq!(
        measured(&m, &e, &top, &below).normal_distance,
        Some(mm(10_000_000))
    );
    assert_eq!(measured(&m, &e, &top, &in_it).normal_distance, Some(mm(0)));
    // Fixed: a line and a plane are measured from the line's point either way; was: the edge first, normal_distance was the top's centre of area, (5, 5, 10),
    // from the edge's line (y = 0, z = 0): √(5² + 10²) = 11.180340, not the 10 between the
    // line and the plane; and 5, not 0, for the edge lying in the top. The rule "the
    // second's reference point to the first's carrier" measures a plane's centre from a
    // line, which isn't the normal distance of a line and a plane parallel to it: for a
    // line and a plane, it should be the line's point's distance from the plane whichever
    // comes first.
    assert_eq!(
        measured(&m, &e, &below, &top).normal_distance,
        Some(mm(10_000_000)),
        "the edge first"
    );
    assert_eq!(
        measured(&m, &e, &in_it, &top).normal_distance,
        Some(mm(0)),
        "the edge in the top, first"
    );
}

#[test]
fn cylinder_axes_parallel_but_for_the_last_bit_are_their_gap_apart_not_zero() {
    // A cylinder along z through the origin, and a solid one turned about a line of an xz
    // sketch from (20, 0) to (20 + 1 ulp, 10): its axis is parallel to the first's within
    // 4e-16 rad, which `angle` reads as 0 at the nanodegree, 20 from it along x. Turned
    // instead about a line from (20, 0) to (20, 10), exactly parallel, it's 20 (the
    // parallel rule); the axes, a last bit apart, should be 20 apart too.
    let e = engine();
    let ulp_past_20 = f64::from_bits(20f64.to_bits() + 1);
    let turned = |end_x: f64| {
        let m = model(
            &e,
            &[
                cylinder("cccccccc", [0.0; 3], 2.0, 10.0),
                polygon(
                    "s0000001",
                    'a',
                    "xz",
                    &[[20.0, 0.0], [end_x, 10.0], [23.0, 10.0], [23.0, 0.0]],
                ),
                revolve("rrrrrrrr", "s0000001", "na000001"),
            ],
        );
        let wall = named(&m, "cccccccc", |e| e.r#type == ElementType::Cylinder);
        let turned = the(&m, "rrrrrrrr", |e| e.r#type == ElementType::Cylinder);
        let d = measured(&m, &e, &wall, &turned.name.to_string());
        (d, turned)
    };
    let (exact, exact_wall) = turned(20.0);
    assert_eq!(
        exact.center_distance,
        Some(mm(20_000_000)),
        "{exact_wall:?}"
    );
    let (off, off_wall) = turned(ulp_past_20);
    // The turned cylinder's axis isn't exactly z: this is the case meant.
    let direction = off_wall.axis.as_ref().unwrap().direction;
    assert_ne!(direction[0], 0.0, "{off_wall:?}");
    assert!(direction[0].abs() < 1e-15, "{off_wall:?}");
    // Fixed: lines parallel at the nanodegree are parallel; was: center_distance was 0. Two lines whose cross product isn't exactly 0 were taken
    // as skew, and their distance along the cross product (here along y, across which the
    // axes don't differ, both in the xz plane) is 0: they cross 5e16 mm away. A pair of
    // holes parallel to the last bit reads 0 between their axes; the parallel test should
    // be the one normal_distance has (an angle of 0 at the nanodegree), or the skew formula
    // kept for lines that aren't parallel within it.
    assert_eq!(off.center_distance, Some(mm(20_000_000)));
}

#[test]
fn cylinder_axes_crossing_at_right_angles_are_their_common_perpendicular_apart() {
    // A cylinder of radius 1 along z through (0, 7), z from 0 to 10; and a tube turned about
    // x of an xy sketch's rectangle from y = 2 to 3, x from 0 to 10: its outer wall's and
    // inner wall's axis is the x axis. The axes, skew at right angles, are 7 apart (along
    // y); the tube's walls share their axis, so 0 apart. Least distance, the pin's wall to
    // the tube's outer wall: 7 - 1 - 3 = 3, met at (0, 6, 0) and (0, 3, 0).
    let e = engine();
    let m = model(
        &e,
        &[
            cylinder("cccccccc", [0.0, 7.0, 0.0], 1.0, 10.0),
            polygon(
                "s0000001",
                'a',
                "xy",
                &[[0.0, 2.0], [10.0, 2.0], [10.0, 3.0], [0.0, 3.0]],
            ),
            revolve("rrrrrrrr", "s0000001", "x_axis"),
        ],
    );
    let pin = named(&m, "cccccccc", |e| e.r#type == ElementType::Cylinder);
    let wall = |r: f64| {
        named(&m, "rrrrrrrr", move |e| {
            e.r#type == ElementType::Cylinder && e.radius == Length::rounded(r)
        })
    };
    let (outer, inner) = (wall(3.0), wall(2.0));
    let d = measured(&m, &e, &pin, &outer);
    assert_eq!(d.center_distance, Some(mm(7_000_000)));
    assert_eq!(d.distance, mm(3_000_000));
    assert_eq!(d.from, point([0.0, 6.0, 0.0]));
    assert_eq!(d.to, point([0.0, 3.0, 0.0]));
    assert_eq!(d.normal_distance, None);
    assert_eq!(
        measured(&m, &e, &inner, &pin).center_distance,
        Some(mm(7_000_000))
    );
    // Coaxial: one axis, 0 apart; and 1 apart as surfaces (3 - 2).
    let coaxial = measured(&m, &e, &outer, &inner);
    assert_eq!(coaxial.center_distance, Some(mm(0)));
    assert_eq!(coaxial.distance, mm(1_000_000));
}

#[test]
fn a_vertex_far_along_a_cylinders_axis_is_its_distance_from_the_axis_as_a_line() {
    // A cylinder of radius 1, z from 0 to 10, about the z axis; a box's lowest corner at
    // (3, 4, 50), 40 past the cylinder's top. Its axis, as an infinite line, is 5 from the
    // corner; the cylinder's wall, √(4² + 40²) = √1616 = 40.199502 from it, met at the top
    // rim's (0.6, 0.8, 10). Either way round.
    let e = engine();
    let m = model(
        &e,
        &[
            cylinder("cccccccc", [0.0; 3], 1.0, 10.0),
            block("bbbbbbbb", [3.0, 4.0, 50.0], [1.0; 3]),
        ],
    );
    let wall = named(&m, "cccccccc", |e| e.r#type == ElementType::Cylinder);
    let corner = named(&m, "bbbbbbbb", at([3.0, 4.0, 50.0]));
    let d = measured(&m, &e, &corner, &wall);
    assert_eq!(d.center_distance, Some(mm(5_000_000)));
    assert_eq!(d.distance, mm(40_199_502));
    assert_eq!(d.from, point([3.0, 4.0, 50.0]));
    assert_eq!(d.to, point([0.6, 0.8, 10.0]));
    assert_eq!(d.normal_distance, None);
    let back = measured(&m, &e, &wall, &corner);
    assert_eq!(back.center_distance, Some(mm(5_000_000)));
    assert_eq!((back.from, back.to), (d.to, d.from));
}

/// A prism of the triangle (0, 0), (20, 0), (30, 10), 5 up: at (20, 0) its sides meet at
/// 135 degrees.
fn obtuse(e: &Recompute<KernelPool>) -> Arc<Model> {
    model(
        e,
        &[
            polygon(
                "s0000001",
                'a',
                "xy",
                &[[0.0, 0.0], [20.0, 0.0], [30.0, 10.0]],
            ),
            pad("w5mb4c2n", "s0000001", 5.0),
        ],
    )
}

#[test]
fn edges_meeting_at_an_obtuse_corner_read_the_angle_at_it_past_90() {
    // At (20, 0, 0): the edge back to the origin, along -x, and the edge to (30, 10, 0),
    // along (1, 1): 135 at the corner, named, either way round. The same edges, one on the
    // top (z = 5), share no vertex: 45 between their lines.
    let e = engine();
    let m = obtuse(&e);
    let back = named(&m, "w5mb4c2n", along([0.0; 3], [20.0, 0.0, 0.0]));
    let up = named(&m, "w5mb4c2n", along([20.0, 0.0, 0.0], [30.0, 10.0, 0.0]));
    let corner = named(&m, "w5mb4c2n", at([20.0, 0.0, 0.0]));
    let expected = AngleAnswer {
        angle: deg(135),
        vertex: Some(corner),
    };
    assert_eq!(angled(&m, &back, &up), expected);
    assert_eq!(angled(&m, &up, &back), expected);
    let up_top = named(&m, "w5mb4c2n", along([20.0, 0.0, 5.0], [30.0, 10.0, 5.0]));
    assert_eq!(
        angled(&m, &back, &up_top),
        AngleAnswer {
            angle: deg(45),
            vertex: None
        }
    );
    // The vertical edge at the corner meets both at 90, the corner named.
    let vertical = named(&m, "w5mb4c2n", along([20.0, 0.0, 0.0], [20.0, 0.0, 5.0]));
    let corner = named(&m, "w5mb4c2n", at([20.0, 0.0, 0.0]));
    assert_eq!(
        angled(&m, &vertical, &up),
        AngleAnswer {
            angle: deg(90),
            vertex: Some(corner),
        }
    );
}

#[test]
fn an_element_against_itself_is_0_apart_a_face_reading_180() {
    // An edge with itself: both its ends shared, so no one vertex: 0 between its line and
    // itself, 0 apart, 0 normal. A planar face with itself: both normals the same way, 180
    // (the design: "180 facing the same way"), 0 apart, 0 normal.
    let e = engine();
    let m = obtuse(&e);
    let back = named(&m, "w5mb4c2n", along([0.0; 3], [20.0, 0.0, 0.0]));
    assert_eq!(
        angled(&m, &back, &back),
        AngleAnswer {
            angle: deg(0),
            vertex: None
        }
    );
    let d = measured(&m, &e, &back, &back);
    assert_eq!((d.distance, d.normal_distance), (mm(0), Some(mm(0))));
    let top = named(&m, "w5mb4c2n", facing([0.0, 0.0, 1.0]));
    assert_eq!(angled(&m, &top, &top).angle, deg(180));
    let d = measured(&m, &e, &top, &top);
    assert_eq!((d.distance, d.normal_distance), (mm(0), Some(mm(0))));
}

#[test]
fn a_spheres_degenerated_pole_edge_is_measured_and_refused_an_angle_as_what_it_is() {
    // A sphere of radius 5 about the origin, and a 2 mm box whose bottom is at z = 19,
    // centred over it: the sphere's pole edges are points at z = 5 and z = -5 (OCCT's
    // degenerated edges); the top pole is 14 below the box's bottom, met at (0, 0, 5) and
    // (0, 0, 19); no centre (a degenerated edge has no circle).
    let e = engine();
    let m = model(
        &e,
        &[
            sphere("pppppppp", [0.0; 3], 5.0),
            block("bbbbbbbb", [-1.0, -1.0, 19.0], [2.0; 3]),
        ],
    );
    let pole = named(&m, "pppppppp", |e| {
        e.kind == ElementKind::Edge && e.start == Some(point([0.0, 0.0, 5.0])) && e.end == e.start
    });
    let bottom = named(&m, "bbbbbbbb", facing([0.0, 0.0, -1.0]));
    let d = measured(&m, &e, &pole, &bottom);
    assert_eq!(d.distance, mm(14_000_000));
    assert_eq!(d.from, point([0.0, 0.0, 5.0]));
    assert_eq!(d.to, point([0.0, 0.0, 19.0]));
    assert_eq!((d.center_distance, d.normal_distance), (None, None));
    let refused = angle(&m, &[pole.clone(), bottom.clone()], None).unwrap_err();
    assert_eq!(refused.total(), 1);
    let error = &refused.errors()[0];
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.elements(), std::slice::from_ref(&pole));
    // A degenerated edge is said as one, a point with no curve and no length, as
    // `describe` has it (it was once "an edge on a curve" that "has a length and ends").
    assert_eq!(
        error.cause(),
        Some("elements.0 is a degenerated edge, a pole or an apex, with no curve")
    );
    assert_eq!(
        error.suggestion(),
        Some(
            "a degenerated edge is a point of no length (its entry in elements) with a \
             distance to other elements; an angle needs a straight edge"
        )
    );
}

#[test]
fn a_cylinders_seam_is_a_straight_edge_measured_and_angled_as_one() {
    // A cylinder of radius 3 about the z axis, z from 0 to 10: OCCT's seam runs from
    // (3, 0, 0) to (3, 0, 10). A box's edge from (10, 0, 0) to (10, 0, 10): parallel, 7
    // apart (least, and normal), 0 between them; the seam meets the cylinder's top at 90.
    let e = engine();
    let m = model(
        &e,
        &[
            cylinder("cccccccc", [0.0; 3], 3.0, 10.0),
            block("bbbbbbbb", [10.0, 0.0, 0.0], [1.0, 1.0, 10.0]),
        ],
    );
    let seam = named(&m, "cccccccc", along([3.0, 0.0, 0.0], [3.0, 0.0, 10.0]));
    let edge = named(&m, "bbbbbbbb", along([10.0, 0.0, 0.0], [10.0, 0.0, 10.0]));
    let top = named(&m, "cccccccc", facing([0.0, 0.0, 1.0]));
    let d = measured(&m, &e, &seam, &edge);
    assert_eq!(d.distance, mm(7_000_000));
    assert_eq!(d.normal_distance, Some(mm(7_000_000)));
    assert_eq!(d.center_distance, None);
    assert_eq!(
        angled(&m, &seam, &edge),
        AngleAnswer {
            angle: deg(0),
            vertex: None
        }
    );
    assert_eq!(angled(&m, &seam, &top).angle, deg(90));
}

#[test]
fn faces_a_few_nanodegrees_apart_read_it_and_have_no_normal_distance() {
    // A prism of the quadrilateral (0, 0), (20, 0), (20, 10 + δ), (0, 10), 5 up: its face on
    // the slanted side is tilted atan(δ / 20) from its bottom's plane, which it faces. With
    // δ = 1e-9, that's 2.8647890e-9 degrees, 3 at the nanodegree: not parallel, so no normal
    // distance. With δ = 1e-10, 0.29 nanodegree, 0: parallel, and the slanted face's centre
    // (y = 10 + δ/2) is 10 from the bottom's plane at the nanometre, either way round.
    let e = engine();
    let tilted = |delta: f64| {
        model(
            &e,
            &[
                polygon(
                    "s0000001",
                    'a',
                    "xy",
                    &[[0.0, 0.0], [20.0, 0.0], [20.0, 10.0 + delta], [0.0, 10.0]],
                ),
                pad("w5mb4c2n", "s0000001", 5.0),
            ],
        )
    };
    let faces = |m: &Model| {
        let bottom = named(m, "w5mb4c2n", facing([0.0, -1.0, 0.0]));
        let slanted = named(m, "w5mb4c2n", |e| {
            e.normal.is_some_and(|n| n[1] > 0.99 && n[0] != 0.0)
        });
        (bottom, slanted)
    };
    let m = tilted(1e-9);
    let (bottom, slanted) = faces(&m);
    assert_eq!(angled(&m, &bottom, &slanted).angle, Angle::from_ndeg(3));
    assert_eq!(measured(&m, &e, &bottom, &slanted).normal_distance, None);
    let m = tilted(1e-10);
    let (bottom, slanted) = faces(&m);
    assert_eq!(angled(&m, &bottom, &slanted).angle, Angle::from_ndeg(0));
    assert_eq!(
        measured(&m, &e, &bottom, &slanted).normal_distance,
        Some(mm(10_000_000))
    );
    assert_eq!(
        measured(&m, &e, &slanted, &bottom).normal_distance,
        Some(mm(10_000_000))
    );
}

#[test]
fn corners_a_kilometre_out_either_way_are_measured_without_overflow() {
    // Two 1 mm cubes, one with its lowest corner at (-1e6, -1e6, -1e6), the other's highest
    // at (1e6, 1e6, 1e6): their nearest corners, (-999999, ...) and (999999, ...), are
    // 1999998 √3 = 3464098.151036 apart, centre to centre too. The first's edges at its
    // farthest-out corner meet at 90; its bottom and the second's top, facing apart, read 0
    // (opposite normals), and are 2e6 apart normally.
    let e = engine();
    let m = model(
        &e,
        &[
            block("aaaaaaaa", [-1e6; 3], [1.0; 3]),
            block("bbbbbbbb", [999_999.0; 3], [1.0; 3]),
        ],
    );
    let near_a = named(&m, "aaaaaaaa", at([-999_999.0; 3]));
    let near_b = named(&m, "bbbbbbbb", at([999_999.0; 3]));
    let d = measured(&m, &e, &near_a, &near_b);
    assert_eq!(d.distance, mm(3_464_098_151_036));
    assert_eq!(d.center_distance, Some(mm(3_464_098_151_036)));
    let x = named(&m, "aaaaaaaa", along([-1e6; 3], [-999_999.0, -1e6, -1e6]));
    let y = named(&m, "aaaaaaaa", along([-1e6; 3], [-1e6, -999_999.0, -1e6]));
    assert_eq!(angled(&m, &x, &y).angle, deg(90));
    let bottom = named(&m, "aaaaaaaa", facing([0.0, 0.0, -1.0]));
    let top = named(&m, "bbbbbbbb", facing([0.0, 0.0, 1.0]));
    assert_eq!(angled(&m, &bottom, &top).angle, deg(0));
    assert_eq!(
        measured(&m, &e, &bottom, &top).normal_distance,
        Some(mm(2_000_000_000_000))
    );
}

#[test]
fn a_sketchs_own_line_names_no_element_of_a_solid_and_the_sketch_makes_none() {
    // The obtuse prism's sketch line na000001, (0, 0) to (20, 0), by the sketch's own name:
    // an element of no solid, refused at elements.0 as no name of a solid's, saying what to
    // name instead; the other name, a real edge, isn't refused. Named with the sketch as the
    // feature: refused once, naming feature, the names not looked for.
    let e = engine();
    let m = obtuse(&e);
    let line = "s0000001.sketch.Edge.na000001".to_owned();
    let back = named(&m, "w5mb4c2n", along([0.0; 3], [20.0, 0.0, 0.0]));
    for refused in [
        distance(&m, e.kernel(), &[line.clone(), back.clone()], None).unwrap_err(),
        angle(&m, &[line.clone(), back.clone()], None).unwrap_err(),
    ] {
        assert_eq!(refused.total(), 1, "{:?}", refused.errors());
        let error = &refused.errors()[0];
        assert_eq!(error.code(), ErrorCode::ReferenceBroken);
        assert_eq!(error.parameter(), Some("elements.0"));
        assert_eq!(error.elements(), std::slice::from_ref(&line));
        assert_eq!(
            error.message(),
            "elements.0, \"s0000001.sketch.Edge.na000001\", isn't a name: s0000001 is a \
             sketch: its elements are in no solid; name what a feature made of it, at column 1"
        );
    }
    let sketch = FeatureId::new("s0000001").unwrap();
    for refused in [
        distance(&m, e.kernel(), &[back.clone(), back.clone()], Some(&sketch)).unwrap_err(),
        angle(&m, &[back.clone(), back.clone()], Some(&sketch)).unwrap_err(),
    ] {
        assert_eq!(refused.total(), 1, "{:?}", refused.errors());
        let error = &refused.errors()[0];
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(error.parameter(), Some("feature"));
        assert_eq!(
            error.message(),
            "s0000001 is a sketch, which makes no solid"
        );
    }
    // Both wrong: both said, the feature first.
    let both = distance(&m, e.kernel(), &[line.clone(), back.clone()], Some(&sketch)).unwrap_err();
    let said: Vec<Option<&str>> = both.errors().iter().map(|e| e.parameter()).collect();
    assert_eq!(said, [Some("feature"), Some("elements.0")]);
}

#[test]
fn edges_of_two_bodies_meeting_at_a_point_have_no_vertex_and_read_between_their_lines() {
    // Two prisms of triangles that touch at (20, 0): (0, 0), (20, 0), (10, 10), and (20, 0),
    // (30, 10), (20, 10). The first's edge along x and the second's edge to (30, 10) meet
    // at (20, 0, 0) at 135, but no vertex is both solids': 45 between their lines (the
    // design's "of one solid"), no vertex; 0 apart there.
    let e = engine();
    let m = model(
        &e,
        &[
            polygon(
                "s0000001",
                'a',
                "xy",
                &[[0.0, 0.0], [20.0, 0.0], [10.0, 10.0]],
            ),
            pad("w5mb4c2n", "s0000001", 5.0),
            polygon(
                "s0000002",
                'b',
                "xy",
                &[[20.0, 0.0], [30.0, 10.0], [20.0, 10.0]],
            ),
            pad("w6mb4c2n", "s0000002", 5.0),
        ],
    );
    let first = named(&m, "w5mb4c2n", along([0.0; 3], [20.0, 0.0, 0.0]));
    let second = named(&m, "w6mb4c2n", along([20.0, 0.0, 0.0], [30.0, 10.0, 0.0]));
    assert_eq!(
        angled(&m, &first, &second),
        AngleAnswer {
            angle: deg(45),
            vertex: None
        }
    );
    let d = measured(&m, &e, &first, &second);
    assert_eq!(d.distance, mm(0));
    assert_eq!(
        (d.from, d.to),
        (point([20.0, 0.0, 0.0]), point([20.0, 0.0, 0.0]))
    );
}

fn torus(id: &str, at: [f64; 3], major: f64, minor: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "torus", "v": 1, "name": "T{id}", "position": [{}, {}, {}], "radius1": {major}, "radius2": {minor}}}"#,
        at[0], at[1], at[2]
    )
}

fn mm_of(length: Length) -> f64 {
    length.nm() as f64 * 1e-6
}

#[test]
fn curved_faces_measure_the_same_either_way_round_their_witness_on_each() {
    // Spheres of radii 5 and 3, centres 20 apart along x: 12 apart, met only at (5, 0, 0)
    // and (17, 0, 0). A torus about z centred at (0, 0, 40), radii 10 and 2: its tube's
    // centre circle is √(10² + 40²) = √1700 from the first sphere's centre, so the faces
    // are √1700 − 2 − 5 = 34.231056 apart, met on a whole ring of pairs: the witness is one
    // of them, a point of each face, whichever way round.
    let e = engine();
    let m = model(
        &e,
        &[
            sphere("aaaaaaaa", [0.0; 3], 5.0),
            sphere("bbbbbbbb", [20.0, 0.0, 0.0], 3.0),
            torus("tttttttt", [0.0, 0.0, 40.0], 10.0, 2.0),
        ],
    );
    let face = |id: &str| named(&m, id, |e| e.kind == ElementKind::Face);
    let (a, b, t) = (face("aaaaaaaa"), face("bbbbbbbb"), face("tttttttt"));
    let d = measured(&m, &e, &a, &b);
    assert_eq!(d.distance, mm(12_000_000));
    assert_eq!(
        (d.from, d.to),
        (point([5.0, 0.0, 0.0]), point([17.0, 0.0, 0.0]))
    );
    let back = measured(&m, &e, &b, &a);
    assert_eq!(
        (back.distance, back.from, back.to),
        (d.distance, d.to, d.from)
    );
    for (first, second) in [(&a, &t), (&t, &a)] {
        let d = measured(&m, &e, first, second);
        assert_eq!(d.distance, mm(34_231_056), "{first} to {second}");
        let (on_sphere, on_torus) = if first == &a {
            (d.from, d.to)
        } else {
            (d.to, d.from)
        };
        let [x, y, z] = on_sphere.map(mm_of);
        assert!(
            ((x * x + y * y + z * z).sqrt() - 5.0).abs() < 2e-6,
            "{on_sphere:?}"
        );
        let [x, y, z] = on_torus.map(mm_of);
        let (across, up) = ((x * x + y * y).sqrt() - 10.0, z - 40.0);
        let tube = (across * across + up * up).sqrt();
        assert!((tube - 2.0).abs() < 2e-6, "{on_torus:?}");
        assert_eq!((d.center_distance, d.normal_distance), (None, None));
    }
}

#[test]
fn tops_facing_the_same_way_read_180_and_their_heights_apart_normally() {
    // Two boxes side by side, x from 0 to 10 and from 20 to 30, tops at z = 10 and 15: both
    // tops face +z, 180 apart; their planes 5 apart either way round; the faces themselves
    // √(10² + 5²) = √125 = 11.180340 apart, across the gap.
    let e = engine();
    let m = model(
        &e,
        &[
            block("aaaaaaaa", [0.0; 3], [10.0, 10.0, 10.0]),
            block("bbbbbbbb", [20.0, 0.0, 0.0], [10.0, 10.0, 15.0]),
        ],
    );
    let up = |id: &str| named(&m, id, facing([0.0, 0.0, 1.0]));
    let (a, b) = (up("aaaaaaaa"), up("bbbbbbbb"));
    for (first, second) in [(&a, &b), (&b, &a)] {
        assert_eq!(angled(&m, first, second).angle, deg(180));
        let d = measured(&m, &e, first, second);
        assert_eq!(d.normal_distance, Some(mm(5_000_000)));
        assert_eq!(d.distance, mm(11_180_340));
    }
}

// ---- From the measure council (round 1): angles off 0 and 90, FreeCAD's cases ----

#[test]
fn parallel_edges_running_opposite_ways_read_0_and_slanted_lines_their_angle() {
    // A 10 × 5 rectangle padded 5: its bottom front edge, along +x, and its top back edge,
    // as the sketch runs it, along -x: parallel, 0 between their lines (never 180), and
    // √(5² + 5²) = 7.071068 apart normally, from the second's middle (5, 5, 5) to the first's
    // line.
    let e = engine();
    let m = model(
        &e,
        &[
            polygon(
                "s0000001",
                'a',
                "xy",
                &[[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]],
            ),
            pad("w5mb4c2n", "s0000001", 5.0),
        ],
    );
    let front = named(&m, "w5mb4c2n", along([0.0; 3], [10.0, 0.0, 0.0]));
    let back = named(&m, "w5mb4c2n", along([10.0, 5.0, 5.0], [0.0, 5.0, 5.0]));
    assert_eq!(angled(&m, &front, &back).angle, deg(0));
    assert_eq!(
        measured(&m, &e, &front, &back).normal_distance,
        Some(mm(7_071_068))
    );
    // The obtuse prism: its bottom back edge, along x, and its top long edge, (0, 0, 5) to
    // (30, 10, 5), share no vertex: atan(1/3) = 18.434948823 between their lines, either
    // way they run.
    let m = obtuse(&e);
    let back = named(&m, "w5mb4c2n", along([0.0; 3], [20.0, 0.0, 0.0]));
    let long_top = named(&m, "w5mb4c2n", along([0.0, 0.0, 5.0], [30.0, 10.0, 5.0]));
    assert_eq!(
        angled(&m, &back, &long_top),
        AngleAnswer {
            angle: Angle::from_ndeg(18_434_948_823),
            vertex: None,
        }
    );
    // Its bottom long edge against the face along x (outward normal -y): asin(10 / √1000) =
    // 18.434948823 too, either way round.
    let long = named(&m, "w5mb4c2n", along([0.0; 3], [30.0, 10.0, 0.0]));
    let side = named(&m, "w5mb4c2n", facing([0.0, -1.0, 0.0]));
    assert_eq!(
        angled(&m, &long, &side).angle,
        Angle::from_ndeg(18_434_948_823)
    );
    assert_eq!(
        angled(&m, &side, &long).angle,
        Angle::from_ndeg(18_434_948_823)
    );
}

#[test]
fn a_line_off_a_plane_reads_its_angle_as_freecads_tests_have_it() {
    // A 30° wedge's slanted bottom edge, (0, 0, 0) to (20 cos 30°, 10, 0), against its face
    // along x (normal -y): 30 (FreeCAD 1.1.3 reads 120, the angle to the normal, unfolded).
    let e = engine();
    let m = model(
        &e,
        &[
            polygon(
                "s0000001",
                'a',
                "xy",
                &[[0.0, 0.0], [20.0, 0.0], [17.320_508_075_688_775, 10.0]],
            ),
            pad("w5mb4c2n", "s0000001", 5.0),
        ],
    );
    let slanted = named(
        &m,
        "w5mb4c2n",
        along([0.0; 3], [17.320_508_075_688_775, 10.0, 0.0]),
    );
    let side = named(&m, "w5mb4c2n", facing([0.0, -1.0, 0.0]));
    assert_eq!(angled(&m, &slanted, &side).angle, deg(30));
    assert_eq!(angled(&m, &side, &slanted).angle, deg(30));
    // FreeCAD main's testFaceEdgeLineToPlane: a line at 45° to a plane, 45. A right
    // triangle's hypotenuse, (0, 0, 0) to (10, 10, 0), against its face along x.
    let m = model(
        &e,
        &[
            polygon(
                "s0000001",
                'a',
                "xy",
                &[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
            ),
            pad("w5mb4c2n", "s0000001", 5.0),
        ],
    );
    let hypotenuse = named(&m, "w5mb4c2n", along([0.0; 3], [10.0, 10.0, 0.0]));
    let side = named(&m, "w5mb4c2n", facing([0.0, -1.0, 0.0]));
    assert_eq!(angled(&m, &hypotenuse, &side).angle, deg(45));
}

#[test]
fn a_corner_between_edges_of_one_and_two_reads_its_10_degrees_with_the_vertex_named() {
    // Fable's case: two edges from one corner, 1 and 2 long, 10° apart: 10 at the corner
    // (FreeCAD 1.1.3 reads 170). The third point is (2 cos 10°, 2 sin 10°). Written as the
    // API writes it, the vertex by its name.
    let e = engine();
    let m = model(
        &e,
        &[
            polygon(
                "s0000001",
                'a',
                "xy",
                &[
                    [0.0, 0.0],
                    [1.0, 0.0],
                    [1.969_615_506_024_416, 0.347_296_355_333_860_7],
                ],
            ),
            pad("w5mb4c2n", "s0000001", 1.0),
        ],
    );
    let short = named(&m, "w5mb4c2n", along([0.0; 3], [1.0, 0.0, 0.0]));
    let long = named(
        &m,
        "w5mb4c2n",
        along(
            [0.0; 3],
            [1.969_615_506_024_416, 0.347_296_355_333_860_7, 0.0],
        ),
    );
    let corner = named(&m, "w5mb4c2n", at([0.0; 3]));
    let answer = angled(&m, &short, &long);
    assert_eq!(
        answer,
        AngleAnswer {
            angle: deg(10),
            vertex: Some(corner.clone()),
        }
    );
    let written = silico_core::command::wire::write(
        &silico_core::command::wire::ToWire::to_wire(&answer),
        silico_core::command::wire::Layout::Canonical,
    )
    .unwrap();
    assert_eq!(written, format!(r#"{{"angle": 10, "vertex": "{corner}"}}"#));
}

#[test]
fn a_spheres_face_is_15_from_a_box_face_20_out_and_discs_their_gap_apart() {
    // FreeCAD main's testCurvedFaceDistance: a sphere of radius 5 at the origin and a box's
    // face at x = 20: 15.
    let e = engine();
    let m = model(
        &e,
        &[
            sphere("ssssssss", [0.0; 3], 5.0),
            block("bbbbbbbb", [20.0, -5.0, -5.0], [10.0; 3]),
        ],
    );
    let ball = named(&m, "ssssssss", |e| e.kind == ElementKind::Face);
    let wall = named(&m, "bbbbbbbb", facing([-1.0, 0.0, 0.0]));
    assert_eq!(measured(&m, &e, &ball, &wall).distance, mm(15_000_000));
    // FreeCAD main's ParallelDiscsHaveNominalDistance: a disc of radius 5 at z = 2 (a
    // cylinder's top) and another at z = 9.5 (one's bottom), 3 apart in plan so overlapping:
    // 7.5 apart, and 7.5 normally.
    let m = model(
        &e,
        &[
            cylinder("cccccccc", [0.0; 3], 5.0, 2.0),
            cylinder("dddddddd", [3.0, 0.0, 9.5], 5.0, 1.0),
        ],
    );
    let top = named(&m, "cccccccc", facing([0.0, 0.0, 1.0]));
    let bottom = named(&m, "dddddddd", facing([0.0, 0.0, -1.0]));
    let discs = measured(&m, &e, &top, &bottom);
    assert_eq!(discs.distance, mm(7_500_000));
    assert_eq!(discs.normal_distance, Some(mm(7_500_000)));
    // ParallelDiscAndPlaneHaveNominalDistance: the disc and a plane 8 above it, 8 and 8.
    let m = model(
        &e,
        &[
            cylinder("cccccccc", [0.0; 3], 5.0, 2.0),
            block("bbbbbbbb", [-10.0, -10.0, 10.0], [20.0, 20.0, 1.0]),
        ],
    );
    let top = named(&m, "cccccccc", facing([0.0, 0.0, 1.0]));
    let plane = named(&m, "bbbbbbbb", facing([0.0, 0.0, -1.0]));
    let gap = measured(&m, &e, &top, &plane);
    assert_eq!(gap.distance, mm(8_000_000));
    assert_eq!(gap.normal_distance, Some(mm(8_000_000)));
}

#[test]
fn a_filleted_wedge_reads_30_still() {
    // The reason the face rule was chosen (Fable's decision): the angle of the wedge both
    // normals point into, whether or not its faces still share an edge. Its corner edge
    // rounded off, the two faces read 30 as before.
    let e = engine();
    let wedge = [
        polygon(
            "s0000001",
            'a',
            "xy",
            &[[0.0, 0.0], [20.0, 0.0], [17.320_508_075_688_775, 10.0]],
        ),
        pad("w5mb4c2n", "s0000001", 5.0),
    ];
    let m = model(&e, &wedge);
    let corner = named(&m, "w5mb4c2n", along([0.0; 3], [0.0, 0.0, 5.0]));
    let mut rounded = wedge.to_vec();
    rounded.push(format!(
        r#"{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "w5mb4c2n", "edges": [{corner:?}], "radius": 1}}"#
    ));
    let m = model(&e, &rounded);
    let along_x = named(&m, "ffffffff", facing([0.0, -1.0, 0.0]));
    let slanted = named(&m, "ffffffff", |e| {
        e.normal.is_some_and(|n| n[0] < 0.0 && n[1] > 0.0)
    });
    assert_eq!(angled(&m, &along_x, &slanted).angle, deg(30));
}
