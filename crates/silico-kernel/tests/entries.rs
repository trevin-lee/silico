//! M4.4: `elements`' entries of real solids, rebuilt on kernel workers (docs/design/m4.1-command-api.md,
//! section 2's *Element entries*): each primitive's faces, edges, and vertices, their types,
//! measures, axes, and defaults, every expected value by hand from the primitive's
//! dimensions, or FreeCAD 1.1.3's where named.

use super::model::{checked, published};

use std::f64::consts::PI;
use std::path::Path;
use std::sync::Arc;

use silico_core::command::Revision;
use silico_core::command::elements::{
    AxisToken, ElementType, Entry, Facing, MeasureName, Order, Quantity, Query, Range, entries,
    list, query,
};
use silico_core::document::{Angle, FeatureId, Length};
use silico_core::kernel::ElementKind;
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// A box, a cylinder, a cone to a point, a sphere, and a torus, apart, each a body.
const PRIMITIVES: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 6,
  "features": [
    {"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 10, "width": 20, "height": 30},
    {"id": "cccccccc", "type": "cylinder", "v": 1, "name": "Cylinder", "position": [100, 0, 0], "radius": 2.5, "height": 10},
    {"id": "kkkkkkkk", "type": "cone", "v": 1, "name": "Cone", "position": [200, 0, 0], "radius1": 4, "radius2": 0, "height": 6},
    {"id": "ssssssss", "type": "sphere", "v": 1, "name": "Sphere", "position": [300, 0, 0], "radius": 3},
    {"id": "tttttttt", "type": "torus", "v": 1, "name": "Torus", "position": [400, 0, 0], "radius1": 10, "radius2": 2}
  ]
}
"#;

fn model() -> Arc<Model> {
    let pool = KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap();
    published(&Recompute::new(pool), checked(PRIMITIVES))
}

fn of(model: &Model, feature: &str) -> Vec<Entry> {
    entries(model, &FeatureId::new(feature).unwrap())
        .unwrap()
        .to_vec()
}

/// `got` is `want` to the measure's precision (a relative 1e-9) and its rounding (half a
/// step of 6 decimals).
fn close(what: &str, got: f64, want: f64) {
    assert!(
        (got - want).abs() <= 1e-9 * want.abs() + 0.5e-6,
        "{what}: {got}, not {want}"
    );
}

fn mm(l: Option<Length>) -> f64 {
    l.unwrap().mm()
}

fn point(p: Option<[Length; 3]>) -> [f64; 3] {
    p.unwrap().map(Length::mm)
}

fn area(e: &Entry) -> f64 {
    e.area.unwrap().millionths() as f64 / 1e6
}

fn close_point(what: &str, got: [f64; 3], want: [f64; 3]) {
    for i in 0..3 {
        close(what, got[i], want[i]);
    }
}

#[test]
fn a_boxs_faces_are_planes_facing_out_with_their_areas_centres_and_default_x() {
    let model = model();
    let entries = of(&model, "bbbbbbbb");
    let faces: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.kind == ElementKind::Face)
        .collect();
    assert_eq!(faces.len(), 6);
    for face in &faces {
        assert_eq!(face.r#type, ElementType::Plane);
        assert_eq!(face.feature.as_str(), "bbbbbbbb");
        assert!(face.stable);
        let n = face.normal.unwrap();
        // By hand, for a 10 × 20 × 30 box at the origin: each face's area, centre, and the
        // default x F1's table gives its normal.
        let (want_area, want_centre, want_x) = match n {
            [1.0, 0.0, 0.0] => (600.0, [10.0, 10.0, 15.0], AxisToken::PlusY),
            [-1.0, 0.0, 0.0] => (600.0, [0.0, 10.0, 15.0], AxisToken::MinusY),
            [0.0, 1.0, 0.0] => (300.0, [5.0, 20.0, 15.0], AxisToken::MinusX),
            [0.0, -1.0, 0.0] => (300.0, [5.0, 0.0, 15.0], AxisToken::PlusX),
            [0.0, 0.0, 1.0] => (200.0, [5.0, 10.0, 30.0], AxisToken::PlusX),
            [0.0, 0.0, -1.0] => (200.0, [5.0, 10.0, 0.0], AxisToken::PlusX),
            other => panic!("{}: normal {other:?}", face.name),
        };
        close("a face's area", area(face), want_area);
        close_point("a face's centre", point(face.centroid), want_centre);
        assert_eq!(face.default_x, Some(want_x), "{}", face.name);
        // A plane has nothing of a curve's.
        assert_eq!(
            (face.axis.clone(), face.radius, face.apex),
            (None, None, None)
        );
    }
    // Twelve straight edges, four each of 10, 20, and 30, each with its middle.
    let edges: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.kind == ElementKind::Edge)
        .collect();
    let mut lengths: Vec<f64> = edges.iter().map(|e| mm(e.length)).collect();
    lengths.sort_by(f64::total_cmp);
    let want: Vec<f64> = [10.0, 20.0, 30.0]
        .into_iter()
        .flat_map(|l| std::iter::repeat_n(l, 4))
        .collect();
    assert_eq!(lengths, want);
    for edge in &edges {
        assert_eq!(edge.r#type, ElementType::Line);
        assert_eq!(edge.straight, Some(true));
        let (s, e) = (point(edge.start), point(edge.end));
        let mid = [0, 1, 2].map(|i| (s[i] + e[i]) / 2.0);
        close_point("a line's middle", point(edge.midpoint), mid);
        assert_eq!(edge.diameter, None);
    }
    // Its eight corners, each once, by hand.
    let mut vertices: Vec<[i64; 3]> = entries
        .iter()
        .filter(|e| e.kind == ElementKind::Vertex)
        .map(|e| e.position.unwrap().map(Length::nm))
        .collect();
    vertices.sort();
    let mut corners = Vec::new();
    for x in [0, 10_000_000] {
        for y in [0, 20_000_000] {
            for z in [0, 30_000_000] {
                corners.push([x, y, z]);
            }
        }
    }
    assert_eq!(vertices, corners);
}

#[test]
fn a_boxs_edges_run_from_their_first_vertex_to_their_last_as_freecad_gives_them() {
    // FreeCAD 1.1.3's Part::Box 10 × 20 × 30: each edge's firstVertex() and lastVertex()
    // (its curve's own order), by its number. Face1's outer wire uses Edge3 and Edge4
    // reversed, Face2's Edge5 and Edge6, and so on: an entry's ends are the curve's, not the
    // order a face takes it in.
    let model = model();
    let entries = of(&model, "bbbbbbbb");
    for (n, start, end) in [
        (1, [0, 0, 0], [0, 0, 30]),
        (2, [0, 0, 30], [0, 20, 30]),
        (3, [0, 20, 0], [0, 20, 30]),
        (4, [0, 0, 0], [0, 20, 0]),
        (5, [10, 0, 0], [10, 0, 30]),
        (6, [10, 0, 30], [10, 20, 30]),
        (7, [10, 20, 0], [10, 20, 30]),
        (8, [10, 0, 0], [10, 20, 0]),
        (9, [0, 0, 0], [10, 0, 0]),
        (10, [0, 0, 30], [10, 0, 30]),
        (11, [0, 20, 0], [10, 20, 0]),
        (12, [0, 20, 30], [10, 20, 30]),
    ] {
        let name = format!("bbbbbbbb.box.Edge{n}");
        let edge = entries
            .iter()
            .find(|e| e.name.to_string() == name)
            .unwrap_or_else(|| panic!("no {name}"));
        let mm = |p: [i64; 3]| Some(p.map(|c| Length::from_nm(c * 1_000_000)));
        assert_eq!((edge.start, edge.end), (mm(start), mm(end)), "{name}");
    }
}

#[test]
fn the_largest_planar_face_facing_up_is_the_boxs_top_by_name() {
    // The design's query (section 7): planar faces within 1° of +z, largest first, one.
    // The box's top, 10 × 20 = 200 mm², beats the cylinder's top cap (6.25π); the box's
    // 600 mm² sides don't face up. FreeCAD 1.1.3's Part::Box has its top as Face6, at
    // (5, 10, 30).
    let model = model();
    let mut q = Query::of(ElementKind::Face, 1);
    q.r#type = Some(ElementType::Plane);
    q.stable = Some(true);
    q.facing = Some(Facing {
        direction: [0.0, 0.0, 1.0],
        within: Angle::from_ndeg(1_000_000_000),
    });
    q.order = Some(Order {
        by: MeasureName::Area,
        descending: true,
    });
    let got = query(&model, &q).unwrap();
    assert_eq!(got.tied, None);
    let [top] = got.entries.as_slice() else {
        panic!("{:?}", got.entries)
    };
    assert_eq!(top.name.to_string(), "bbbbbbbb.box.Face6");
    assert_eq!(top.normal, Some([0.0, 0.0, 1.0]));
    close_point("its centre", point(top.centroid), [5.0, 10.0, 30.0]);
    close("its area", area(top), 200.0);
    // Facing up: the box's top, the cylinder's top cap, and nothing of the cone (its base
    // faces down) or the round solids.
    assert_eq!(got.matched, 2);
}

#[test]
fn a_radius_range_keeps_only_faces_that_have_a_radius_in_it() {
    // Faces of radius 2 to 2.9 mm: the cylinder's side (2.5) alone. The sphere's 3 is past
    // it, and a plane, a cone, and a torus have no radius, so no range holds them.
    let model = model();
    let mut q = Query::of(ElementKind::Face, 1000);
    q.measures = vec![Range {
        measure: MeasureName::Radius,
        at_least: Some(Quantity::Length(Length::from_nm(2_000_000))),
        at_most: Some(Quantity::Length(Length::from_nm(2_900_000))),
    }];
    let got = query(&model, &q).unwrap();
    let found: Vec<(&str, ElementType)> = got
        .entries
        .iter()
        .map(|e| (e.feature.as_str(), e.r#type))
        .collect();
    assert_eq!(found, [("cccccccc", ElementType::Cylinder)]);
    // Open above: the sphere too, after the cylinder in timeline order.
    q.measures[0].at_most = None;
    let got = query(&model, &q).unwrap();
    let found: Vec<&str> = got.entries.iter().map(|e| e.feature.as_str()).collect();
    assert_eq!(found, ["cccccccc", "ssssssss"]);
}

#[test]
fn a_cylinders_side_has_its_axis_radius_and_diameter_and_its_rims_are_circles() {
    let model = model();
    let entries = of(&model, "cccccccc");
    let side: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.r#type == ElementType::Cylinder)
        .collect();
    assert_eq!(side.len(), 1);
    let side = side[0];
    // 2π r h = 50π; its axis through (100, 0), along −z, as FreeCAD 1.1.3's Part::Cylinder
    // has it (a reversed face).
    close("the side's area", area(side), 50.0 * PI);
    let axis = side.axis.clone().unwrap();
    assert_eq!(axis.direction, [0.0, 0.0, -1.0]);
    close("the axis's x", axis.point[0].mm(), 100.0);
    close("the axis's y", axis.point[1].mm(), 0.0);
    assert_eq!(side.radius, Some(Length::from_nm(2_500_000)));
    assert_eq!(side.diameter, Some(Length::from_nm(5_000_000)));
    assert_eq!(side.normal, None);
    assert_eq!(side.default_x, None);
    // Two caps of π r² = 6.25π.
    let caps: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.r#type == ElementType::Plane)
        .collect();
    assert_eq!(caps.len(), 2);
    for cap in caps {
        close("a cap's area", area(cap), 6.25 * PI);
    }
    // Two rims, circles of radius 2.5, 5π long, about the axis; a seam line 10 long.
    let rims: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.r#type == ElementType::Circle)
        .collect();
    assert_eq!(rims.len(), 2);
    for rim in rims {
        assert_eq!(rim.radius, Some(Length::from_nm(2_500_000)));
        assert_eq!(rim.diameter, Some(Length::from_nm(5_000_000)));
        close("a rim's length", mm(rim.length), 5.0 * PI);
        assert_eq!(rim.straight, Some(false));
        assert_eq!(rim.midpoint, None);
        let axis = rim.axis.clone().unwrap();
        close("a rim's centre x", axis.point[0].mm(), 100.0);
    }
    let seams: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.r#type == ElementType::Line)
        .collect();
    assert_eq!(seams.len(), 1);
    close("the seam", mm(seams[0].length), 10.0);
}

#[test]
fn a_cone_to_a_point_has_its_signed_half_angle_and_apex_and_no_radius() {
    let model = model();
    let entries = of(&model, "kkkkkkkk");
    let cone: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.r#type == ElementType::Cone)
        .collect();
    assert_eq!(cone.len(), 1);
    let cone = cone[0];
    // r 4 to 0 over 6: half-angle atan(−4 / 6), narrowing up its axis, −33.690067526° as
    // FreeCAD 1.1.3's Part::Cone gives it; its apex at (200, 0, 6).
    // atan(2 / 3) is 33.69006752597979°, to 9 places 33.690067526.
    assert_eq!(cone.half_angle, Some(Angle::from_ndeg(-33_690_067_526)));
    close_point("the apex", point(cone.apex), [200.0, 0.0, 6.0]);
    // Its radius is OCCT's reference radius, which isn't an entry's.
    assert_eq!((cone.radius, cone.diameter), (None, None));
    // Its side's area, π r √(r² + h²) = 4π √52.
    close("the side's area", area(cone), 4.0 * PI * 52.0f64.sqrt());
    // The apex edge is degenerated: of type other, 0 long, at the apex, not straight.
    let apex_edges: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.kind == ElementKind::Edge && e.r#type == ElementType::Other)
        .collect();
    assert_eq!(apex_edges.len(), 1);
    let apex = apex_edges[0];
    assert_eq!(apex.length, Some(Length::from_nm(0)));
    assert_eq!(apex.straight, Some(false));
    assert_eq!(apex.start, apex.end);
    close_point("the apex edge", point(apex.start), [200.0, 0.0, 6.0]);
}

#[test]
fn a_sphere_has_its_centre_and_radius_and_its_poles_are_degenerated_edges() {
    let model = model();
    let entries = of(&model, "ssssssss");
    let ball: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.kind == ElementKind::Face)
        .collect();
    assert_eq!(ball.len(), 1);
    let ball = ball[0];
    assert_eq!(ball.r#type, ElementType::Sphere);
    close("the sphere's area", area(ball), 36.0 * PI);
    assert_eq!(ball.radius, Some(Length::from_nm(3_000_000)));
    // A sphere's diameter isn't an entry's (only a cylinder's and a circle's are).
    assert_eq!(ball.diameter, None);
    let axis = ball.axis.clone().unwrap();
    close_point("its centre", axis.point.map(Length::mm), [300.0, 0.0, 0.0]);
    // +z, as FreeCAD 1.1.3's Part::Sphere has it (a forward face).
    assert_eq!(axis.direction, [0.0, 0.0, 1.0]);
    let poles: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.kind == ElementKind::Edge && e.length == Some(Length::from_nm(0)))
        .collect();
    assert_eq!(poles.len(), 2);
    for pole in poles {
        assert_eq!(pole.r#type, ElementType::Other);
        assert_eq!(pole.straight, Some(false));
    }
}

#[test]
fn a_torus_has_its_two_radii_and_no_single_radius() {
    let model = model();
    let entries = of(&model, "tttttttt");
    let ring: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.kind == ElementKind::Face)
        .collect();
    assert_eq!(ring.len(), 1);
    let ring = ring[0];
    assert_eq!(ring.r#type, ElementType::Torus);
    // 4π² R r = 80π².
    close("the torus's area", area(ring), 80.0 * PI * PI);
    assert_eq!(ring.major_radius, Some(Length::from_nm(10_000_000)));
    assert_eq!(ring.minor_radius, Some(Length::from_nm(2_000_000)));
    assert_eq!((ring.radius, ring.diameter), (None, None));
    let axis = ring.axis.clone().unwrap();
    close_point("its centre", axis.point.map(Length::mm), [400.0, 0.0, 0.0]);
    // −z, as FreeCAD 1.1.3's Part::Torus has it ((−0, −0, −1) on a reversed face), its
    // zeros written 0.
    assert_eq!(
        axis.direction.map(f64::to_bits),
        [0.0, 0.0, -1.0].map(f64::to_bits)
    );
}

#[test]
fn every_body_lists_in_timeline_order() {
    let model = model();
    let listed = list(&model, Revision::OPENED, None, None, 1000).unwrap();
    let mut bodies: Vec<&str> = listed.entries.iter().map(|e| e.feature.as_str()).collect();
    bodies.dedup();
    assert_eq!(
        bodies,
        ["bbbbbbbb", "cccccccc", "kkkkkkkk", "ssssssss", "tttttttt"]
    );
    // Faces, edges, and vertices, as FreeCAD 1.1.3 counts its primitives': the box's 26
    // (6, 12, 8), the cylinder's 8 (3, 3, 2), the cone's 7 (2, 3, 2), the sphere's 6 (1, 3,
    // 2), and the torus's 4 (1, 2, 1): 51, each once.
    assert_eq!(listed.total, 51);
    assert_eq!(listed.entries.len(), 51);
    let mut names: Vec<String> = listed.entries.iter().map(|e| e.name.to_string()).collect();
    names.dedup();
    assert_eq!(names.len(), 51);
}
