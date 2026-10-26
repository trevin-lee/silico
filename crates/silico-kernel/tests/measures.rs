//! M4.5: `distance` and `angle` between two elements by name, of real solids rebuilt on
//! kernel workers (`silico_core::command::measure`; docs/ports/measure.md, *Decided*, and
//! Fable's `m4-measure-fable.md`): the least distance as OCCT measures it, its witness, the
//! distance between centres and the normal distance; and the angle between faces, edges, and
//! one of each. Every expected value by hand from the solids' dimensions.

use std::path::Path;
use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::command::elements::{ElementType, Entry, entries};
use silico_core::command::measure::{AngleAnswer, DistanceAnswer, angle, distance};
use silico_core::document::{Angle, Checked, FeatureId, Length, read};
use silico_core::fake::FakeSketches;
use silico_core::kernel::ElementKind;
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute, RunOutcome};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

fn engine() -> Recompute<KernelPool> {
    Recompute::new(KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap())
        .with_sketches(FakeSketches::new())
}

/// A document of `features`, rebuilt.
fn model(engine: &Recompute<KernelPool>, features: &[String]) -> Arc<Model> {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    );
    let checked = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    match engine
        .start(checked)
        .wait_timeout(std::time::Duration::from_secs(60))
    {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the rebuild didn't finish within a minute"),
    }
}

fn block(id: &str, at: [f64; 3], size: [f64; 3]) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "B{id}", "position": [{}, {}, {}], "length": {}, "width": {}, "height": {}}}"#,
        at[0], at[1], at[2], size[0], size[1], size[2]
    )
}

fn cylinder(id: &str, at: [f64; 3], radius: f64, height: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "cylinder", "v": 1, "name": "C{id}", "position": [{}, {}, {}], "radius": {radius}, "height": {height}}}"#,
        at[0], at[1], at[2]
    )
}

/// The name of the feature `feature`'s element that `pick` picks, the one there is.
fn named(model: &Model, feature: &str, pick: impl Fn(&Entry) -> bool) -> String {
    let all = entries(model, &FeatureId::new(feature).unwrap()).unwrap();
    let picked: Vec<&Entry> = all.iter().filter(|e| pick(e)).collect();
    assert_eq!(picked.len(), 1, "{feature}: {picked:?}");
    picked[0].name.to_string()
}

fn mm(nm: i64) -> Length {
    Length::from_nm(nm)
}

fn point(p: [f64; 3]) -> [Length; 3] {
    p.map(|c| Length::rounded(c).unwrap())
}

fn measured(model: &Model, engine: &Recompute<KernelPool>, a: &str, b: &str) -> DistanceAnswer {
    distance(model, engine.kernel(), &[a.to_owned(), b.to_owned()], None).unwrap()
}

fn angled(model: &Model, a: &str, b: &str) -> AngleAnswer {
    angle(model, &[a.to_owned(), b.to_owned()], None).unwrap()
}

/// A face whose outward normal is `n`.
fn facing(n: [f64; 3]) -> impl Fn(&Entry) -> bool {
    move |e| e.normal == Some(n)
}

#[test]
fn a_boxs_bottom_and_top_are_its_height_apart_their_normal_distance_too() {
    // A 40 × 20 × 10 block: its bottom (z = 0) and top (z = 10) are 10 apart, first met at
    // (0, 0, 0) and (0, 0, 10); parallel, so 10 normal too; no centres. Swapped, the points
    // swap.
    let e = engine();
    let m = model(&e, &[block("bbbbbbbb", [0.0; 3], [40.0, 20.0, 10.0])]);
    let bottom = named(&m, "bbbbbbbb", facing([0.0, 0.0, -1.0]));
    let top = named(&m, "bbbbbbbb", facing([0.0, 0.0, 1.0]));
    let d = measured(&m, &e, &bottom, &top);
    assert_eq!(
        d,
        DistanceAnswer {
            distance: mm(10_000_000),
            from: point([0.0, 0.0, 0.0]),
            to: point([0.0, 0.0, 10.0]),
            center_distance: None,
            normal_distance: Some(mm(10_000_000)),
        }
    );
    let back = measured(&m, &e, &top, &bottom);
    assert_eq!((back.from, back.to), (d.to, d.from));
    // Adjacent faces meet: 0 apart, 90 degrees, so no normal distance.
    let side = named(&m, "bbbbbbbb", facing([-1.0, 0.0, 0.0]));
    let touching = measured(&m, &e, &side, &bottom);
    assert_eq!(touching.distance, mm(0));
    assert_eq!(touching.normal_distance, None);
    assert_eq!(
        angled(&m, &side, &bottom).angle,
        Angle::from_ndeg(90_000_000_000)
    );
    // The bottom and top: opposite-facing parallel faces read 0.
    assert_eq!(angled(&m, &bottom, &top).angle, Angle::from_ndeg(0));
}

#[test]
fn parallel_faces_that_dont_overlap_are_their_corners_apart_and_their_planes_normal() {
    // A 10 mm cube, and another whose bottom (z = 15) is 15 past the first along x and y:
    // the first's top (z = 10) is √(15² + 15² + 5²) = √475 = 21.794495 from it, met at
    // the near corners, (10, 10, 10) and (25, 25, 15); the planes are 5 apart.
    let e = engine();
    let m = model(
        &e,
        &[
            block("aaaaaaaa", [0.0; 3], [10.0; 3]),
            block("bbbbbbbb", [25.0, 25.0, 15.0], [10.0; 3]),
        ],
    );
    let top = named(&m, "aaaaaaaa", facing([0.0, 0.0, 1.0]));
    let bottom = named(&m, "bbbbbbbb", facing([0.0, 0.0, -1.0]));
    let d = measured(&m, &e, &top, &bottom);
    assert_eq!(d.distance, Length::rounded(475f64.sqrt()).unwrap());
    assert_eq!(d.distance, mm(21_794_495));
    assert_eq!(d.from, point([10.0, 10.0, 10.0]));
    assert_eq!(d.to, point([25.0, 25.0, 15.0]));
    assert_eq!(d.normal_distance, Some(mm(5_000_000)));
    assert_eq!(d.center_distance, None);
}

/// The bottom rim (z = 0) of the cylinder `id`.
fn bottom_rim(m: &Model, id: &str) -> String {
    named(m, id, |e| {
        e.r#type == ElementType::Circle && e.start.is_some_and(|p| p[2] == mm(0))
    })
}

#[test]
fn rims_are_their_gap_apart_and_their_centres_apart_too() {
    // Rims of radii 3 and 4, centres 20 apart along y: 20 − 3 − 4 = 13 apart, met at
    // (0, 3, 0) and (0, 16, 0); their centres 20.
    let e = engine();
    let m = model(
        &e,
        &[
            cylinder("cccccccc", [0.0; 3], 3.0, 5.0),
            cylinder("dddddddd", [0.0, 20.0, 0.0], 4.0, 5.0),
        ],
    );
    let (a, b) = (bottom_rim(&m, "cccccccc"), bottom_rim(&m, "dddddddd"));
    let d = measured(&m, &e, &a, &b);
    assert_eq!(d.distance, mm(13_000_000));
    assert_eq!(d.from, point([0.0, 3.0, 0.0]));
    assert_eq!(d.to, point([0.0, 16.0, 0.0]));
    assert_eq!(d.center_distance, Some(mm(20_000_000)));
    assert_eq!(d.normal_distance, None);
    // The walls, as infinite lines along z 20 apart: least 13, centres (axes) 20.
    let wall = |id: &str| named(&m, id, |e| e.r#type == ElementType::Cylinder);
    let walls = measured(&m, &e, &wall("cccccccc"), &wall("dddddddd"));
    assert_eq!(walls.distance, mm(13_000_000));
    assert_eq!(walls.center_distance, Some(mm(20_000_000)));
}

#[test]
fn crossing_and_concentric_rims_read_their_least_and_centre_distances() {
    // Radius 5 rims centred 3 apart cross: 0 apart, centres 3.
    let e = engine();
    let m = model(
        &e,
        &[
            cylinder("cccccccc", [0.0; 3], 5.0, 5.0),
            cylinder("dddddddd", [3.0, 0.0, 0.0], 5.0, 5.0),
        ],
    );
    let crossing = measured(
        &m,
        &e,
        &bottom_rim(&m, "cccccccc"),
        &bottom_rim(&m, "dddddddd"),
    );
    assert_eq!(crossing.distance, mm(0));
    assert_eq!(crossing.center_distance, Some(mm(3_000_000)));
    // Radius 5 and 2 about one centre: 3 apart, centres 0.
    let m = model(
        &e,
        &[
            cylinder("cccccccc", [0.0; 3], 5.0, 5.0),
            cylinder("dddddddd", [0.0, 0.0, 0.0], 2.0, 9.0),
        ],
    );
    let concentric = measured(
        &m,
        &e,
        &bottom_rim(&m, "cccccccc"),
        &bottom_rim(&m, "dddddddd"),
    );
    assert_eq!(concentric.distance, mm(3_000_000));
    assert_eq!(concentric.center_distance, Some(mm(0)));
}

#[test]
fn a_vertex_is_its_distance_from_a_rims_centre() {
    // A box's corner at (3, 4, 0), and a rim of radius 1 centred at the origin: the corner is
    // 5 from the centre, and 4 from the rim, met at (0.6, 0.8, 0).
    let e = engine();
    let m = model(
        &e,
        &[
            cylinder("cccccccc", [0.0; 3], 1.0, 2.0),
            block("bbbbbbbb", [3.0, 4.0, 0.0], [1.0; 3]),
        ],
    );
    let corner = named(&m, "bbbbbbbb", |e| {
        e.position == Some(point([3.0, 4.0, 0.0]))
    });
    let d = measured(&m, &e, &corner, &bottom_rim(&m, "cccccccc"));
    assert_eq!(d.center_distance, Some(mm(5_000_000)));
    assert_eq!(d.distance, mm(4_000_000));
    assert_eq!(d.from, point([3.0, 4.0, 0.0]));
    assert_eq!(d.to, point([0.6, 0.8, 0.0]));
}

#[test]
fn edges_meeting_at_a_corner_read_the_angle_at_it_and_name_it() {
    // A box's edges along x and along y from its origin corner: 90 at that corner, the
    // corner named; an edge along x and one along z from another corner don't meet the first:
    // still 90, between their lines, no vertex.
    let e = engine();
    let m = model(&e, &[block("bbbbbbbb", [0.0; 3], [10.0, 20.0, 30.0])]);
    let along = |from: [f64; 3], to: [f64; 3]| {
        let (from, to) = (point(from), point(to));
        named(&m, "bbbbbbbb", move |e| {
            e.kind == ElementKind::Edge
                && ((e.start, e.end) == (Some(from), Some(to))
                    || (e.start, e.end) == (Some(to), Some(from)))
        })
    };
    let x = along([0.0; 3], [10.0, 0.0, 0.0]);
    let y = along([0.0; 3], [0.0, 20.0, 0.0]);
    let corner = named(&m, "bbbbbbbb", |e| e.position == Some(point([0.0; 3])));
    let at = angled(&m, &x, &y);
    assert_eq!(
        at,
        AngleAnswer {
            angle: Angle::from_ndeg(90_000_000_000),
            vertex: Some(corner),
        }
    );
    let far_z = along([10.0, 20.0, 0.0], [10.0, 20.0, 30.0]);
    assert_eq!(
        angled(&m, &x, &far_z),
        AngleAnswer {
            angle: Angle::from_ndeg(90_000_000_000),
            vertex: None,
        }
    );
    // Opposite edges, parallel, read 0, and are 30 apart normally (from the second's middle
    // to the first's line).
    let top_x = along([0.0, 0.0, 30.0], [10.0, 0.0, 30.0]);
    assert_eq!(angled(&m, &x, &top_x).angle, Angle::from_ndeg(0));
    let d = measured(&m, &e, &x, &top_x);
    assert_eq!(d.normal_distance, Some(mm(30_000_000)));
    // An edge and a face: an edge along z against the bottom, 90; against a side it lies in,
    // 0.
    let bottom = named(&m, "bbbbbbbb", facing([0.0, 0.0, -1.0]));
    let side = named(&m, "bbbbbbbb", facing([1.0, 0.0, 0.0]));
    assert_eq!(
        angled(&m, &far_z, &bottom).angle,
        Angle::from_ndeg(90_000_000_000)
    );
    assert_eq!(angled(&m, &far_z, &side).angle, Angle::from_ndeg(0));
}

/// A wedge: a triangle in xy, (0, 0), (20, 0), and (20 cos 30°, 20 sin 30°), padded 5 up.
fn wedge() -> String {
    format!(
        r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Wedge", "plane": "xy", "points": [{{"id": "pa000000", "x": 0, "y": 0, "construction": false}}, {{"id": "pb000000", "x": 20, "y": 0, "construction": false}}, {{"id": "pc000000", "x": {}, "y": 10, "construction": false}}], "lines": [{{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}}, {{"id": "nb000000", "from": "pb000000", "to": "pc000000", "construction": false}}, {{"id": "nc000000", "from": "pc000000", "to": "pa000000", "construction": false}}], "circles": [], "arcs": [], "constraints": []}}, {{"id": "w5mb4c2n", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {{"type": "one_side", "length": 5, "reversed": false}}}}"#,
        // 20 cos 30°, to the nearest f64 (10√3).
        17.320_508_075_688_775_f64
    )
}

#[test]
fn a_30_degree_wedges_faces_read_30() {
    // Trevin's choice (2026-10-25, relayed by Main): the angle of the wedge both outward
    // normals point into, with no flip, so a 30° wedge's two faces read 30. The faces on the
    // triangle's sides from the origin: one along x (outward normal -y), one at 30° to it.
    let e = engine();
    let m = model(&e, &[wedge()]);
    let along_x = named(&m, "w5mb4c2n", facing([0.0, -1.0, 0.0]));
    let slanted = named(&m, "w5mb4c2n", |e| {
        e.normal.is_some_and(|n| n[0] < 0.0 && n[1] > 0.0)
    });
    assert_eq!(
        angled(&m, &along_x, &slanted),
        AngleAnswer {
            angle: Angle::from_ndeg(30_000_000_000),
            vertex: None,
        }
    );
    // And its top against its bottom: opposite-facing parallel faces, 0.
    let top = named(&m, "w5mb4c2n", facing([0.0, 0.0, 1.0]));
    let bottom = named(&m, "w5mb4c2n", facing([0.0, 0.0, -1.0]));
    assert_eq!(angled(&m, &top, &bottom).angle, Angle::from_ndeg(0));
    // A box's top and its side read 90; a box top against another's top, both facing up,
    // 180.
    let m = model(
        &e,
        &[
            block("aaaaaaaa", [0.0; 3], [10.0; 3]),
            block("bbbbbbbb", [20.0, 0.0, 0.0], [10.0; 3]),
        ],
    );
    let up = |id: &str| named(&m, id, facing([0.0, 0.0, 1.0]));
    assert_eq!(
        angled(&m, &up("aaaaaaaa"), &up("bbbbbbbb")).angle,
        Angle::from_ndeg(180_000_000_000)
    );
}

#[test]
fn a_circle_a_curved_face_or_a_vertex_has_no_angle_and_is_refused_naming_it() {
    let e = engine();
    let m = model(
        &e,
        &[
            cylinder("cccccccc", [0.0; 3], 3.0, 5.0),
            block("bbbbbbbb", [10.0, 0.0, 0.0], [1.0; 3]),
        ],
    );
    let top = named(&m, "bbbbbbbb", facing([0.0, 0.0, 1.0]));
    let rim = bottom_rim(&m, "cccccccc");
    let wall = named(&m, "cccccccc", |e| e.r#type == ElementType::Cylinder);
    let corner = named(&m, "bbbbbbbb", |e| {
        e.position == Some(point([10.0, 0.0, 0.0]))
    });
    let refused = |a: &str, b: &str| angle(&m, &[a.to_owned(), b.to_owned()], None).unwrap_err();
    let circle = refused(&rim, &top);
    let error = &circle.errors()[0];
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(
        error.message(),
        "an angle takes two straight edges, two planar faces, or one of each"
    );
    assert_eq!(error.parameter(), Some("elements"));
    assert_eq!(error.elements(), std::slice::from_ref(&rim));
    assert_eq!(error.cause(), Some("elements.0 is an edge on a circle"));
    assert_eq!(
        error.suggestion(),
        Some(
            "a circle has a radius and a centre (its entry in elements) and a distance to \
             other elements; an angle needs a straight edge"
        )
    );
    let curved = refused(&top, &wall);
    assert_eq!(
        curved.errors()[0].cause(),
        Some("elements.1 is a face on a cylinder, not a plane")
    );
    // Both wrong: both said.
    let both = refused(&corner, &wall);
    let causes: Vec<Option<&str>> = both.errors().iter().map(|e| e.cause()).collect();
    assert_eq!(
        causes,
        [
            Some("elements.0 is a vertex"),
            Some("elements.1 is a face on a cylinder, not a plane")
        ]
    );
}

/// The example `name` (`distance/rims-measured`), as committed in silico-core's contract.
fn example(name: &str) -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../silico-core/tests/contract/commands/examples")
        .join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn written<T: silico_core::command::wire::ToWire>(value: &T) -> String {
    silico_core::command::wire::write(
        &value.to_wire(),
        silico_core::command::wire::Layout::Canonical,
    )
    .unwrap()
        + "\n"
}

#[test]
fn the_designs_measures_are_written_byte_for_byte_from_real_solids() {
    let e = engine();
    // M2.1's example: its block's bottom and top, the hole cut through them.
    let example_part = [
        block("k7f2qx3m", [0.0; 3], [40.0, 20.0, 10.0]),
        cylinder("a91cw0de", [20.0, 10.0, -1.0], 2.5, 12.0),
        r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"}"#.to_owned(),
    ];
    let m = model(&e, &example_part);
    let d = measured(&m, &e, "k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6");
    assert_eq!(written(&d), example("distance/box-bottom-and-top-measured"));
    // Rims of radii 3 and 4, centres 20 apart.
    let rims = model(
        &e,
        &[
            cylinder("cccccccc", [0.0; 3], 3.0, 5.0),
            cylinder("dddddddd", [0.0, 20.0, 0.0], 4.0, 5.0),
        ],
    );
    let d = measured(
        &rims,
        &e,
        &bottom_rim(&rims, "cccccccc"),
        &bottom_rim(&rims, "dddddddd"),
    );
    assert_eq!(written(&d), example("distance/rims-measured"));
    // The design's wedge, its sketch's lines by the design's IDs: the faces from ra000001
    // (along x) and ra000002 (at 30 degrees to it).
    let wedge = wedge()
        .replace("s0000001", "s0000003")
        .replace("na000000", "ra000001")
        .replace("nc000000", "ra000002")
        .replace("nb000000", "ra000003");
    let w = model(&e, &[wedge]);
    let a = angled(
        &w,
        "w5mb4c2n.pad.Face(G s0000003.sketch.Edge.ra000001)",
        "w5mb4c2n.pad.Face(G s0000003.sketch.Edge.ra000002)",
    );
    assert_eq!(written(&a), example("angle/wedge-measured"));
    // The example's hole's top rim, which has no angle.
    let rim = "m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)";
    let refused = angle(&m, &[rim.to_owned(), "k7f2qx3m.box.Face5".to_owned()], None).unwrap_err();
    assert_eq!(refused.total(), 1);
    assert_eq!(
        written(&refused.errors()[0]),
        example("angle/refused-circle")
    );
}
