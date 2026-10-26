//! break-it: `distance`'s `center_to_element` on real solids rebuilt on kernel workers
//! (`silico_core::command::measure::distance`): a cylinder's axis a fraction of a nanodegree
//! either side of parallel to a face and an edge (an axis turned off z by its sketch), a
//! fillet's partial cylinder and its arcs against every face of the box it rounds, a rim's
//! centre against another feature's faces and edge either way round (so the point is
//! measured on the other's shape, never its own), a sphere's seam, whose centre is the
//! sphere's, and a cone's apex, and a sphere's and cone's faces, which have no centre. Every
//! expected value is by hand from the solids' dimensions.

use std::path::Path;
use std::sync::Arc;

use silico_core::command::elements::{ElementType, Entry, entries};
use silico_core::command::measure::{DistanceAnswer, distance};
use silico_core::document::{Checked, FeatureId, Length, read};
use silico_core::fake::FakeSketches;
use silico_core::kernel::ElementKind;
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute, RunOutcome};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

fn engine() -> Recompute<KernelPool> {
    Recompute::new(KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap())
        .with_sketches(FakeSketches::new())
}

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

/// A sketch `id` on `plane` of one closed polygon through `points`, its points `p<tag>00000N`
/// and its lines `n<tag>00000N`, line N from point N to the next.
fn polygon(id: &str, tag: char, plane: &str, points: &[[f64; 2]]) -> String {
    let point = |i: usize| format!("p{tag}{:06}", i + 1);
    let line = |i: usize| format!("n{tag}{:06}", i + 1);
    let ps: Vec<String> = points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            format!(
                r#"{{"id": "{}", "x": {:?}, "y": {:?}, "construction": false}}"#,
                point(i),
                p[0],
                p[1]
            )
        })
        .collect();
    let ls: Vec<String> = (0..points.len())
        .map(|i| {
            format!(
                r#"{{"id": "{}", "from": "{}", "to": "{}", "construction": false}}"#,
                line(i),
                point(i),
                point((i + 1) % points.len())
            )
        })
        .collect();
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "Sk{id}", "plane": "{plane}", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
        ps.join(", "),
        ls.join(", ")
    )
}

fn revolve(id: &str, sketch: &str, axis: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "revolve", "v": 1, "name": "R{id}", "sketch": "{sketch}", "axis": "{axis}", "extent": {{"type": "one_side", "angle": 360, "reversed": false}}}}"#
    )
}

fn the(model: &Model, feature: &str, pick: impl Fn(&Entry) -> bool) -> Entry {
    let all = entries(model, &FeatureId::new(feature).unwrap()).unwrap();
    let picked: Vec<&Entry> = all.iter().filter(|e| pick(e)).collect();
    assert_eq!(picked.len(), 1, "{feature}: {picked:#?}");
    picked[0].clone()
}

fn named(model: &Model, feature: &str, pick: impl Fn(&Entry) -> bool) -> String {
    the(model, feature, pick).name.to_string()
}

fn mm(nm: i64) -> Length {
    Length::from_nm(nm)
}

fn point(p: [f64; 3]) -> [Length; 3] {
    p.map(|c| Length::rounded(c).unwrap())
}

fn measured(model: &Model, engine: &Recompute<KernelPool>, a: &str, b: &str) -> DistanceAnswer {
    distance(model, engine.kernel(), &[a.to_owned(), b.to_owned()], None)
        .unwrap_or_else(|e| panic!("{a} to {b}: {:?}", e.errors()))
}

/// `center_to_element` of `a` and `b`, the same either way round.
fn centre_to(model: &Model, engine: &Recompute<KernelPool>, a: &str, b: &str) -> Option<Length> {
    let ab = measured(model, engine, a, b).center_to_element;
    let ba = measured(model, engine, b, a).center_to_element;
    assert_eq!(ab, ba, "{a} and {b}, either way round");
    ab
}

fn facing(n: [f64; 3]) -> impl Fn(&Entry) -> bool {
    move |e| e.normal == Some(n)
}

fn along(from: [f64; 3], to: [f64; 3]) -> impl Fn(&Entry) -> bool {
    let (from, to) = (point(from), point(to));
    move |e| {
        e.kind == ElementKind::Edge
            && e.straight == Some(true)
            && ((e.start, e.end) == (Some(from), Some(to))
                || (e.start, e.end) == (Some(to), Some(from)))
    }
}

fn at(p: [f64; 3]) -> impl Fn(&Entry) -> bool {
    let p = point(p);
    move |e| e.kind == ElementKind::Vertex && e.position == Some(p)
}

/// A circle or arc edge whose centre is `c`.
fn centred(c: [f64; 3]) -> impl Fn(&Entry) -> bool {
    let c = point(c);
    move |e| e.r#type == ElementType::Circle && e.axis.as_ref().is_some_and(|a| a.point == c)
}

#[test]
fn an_axis_turned_a_fraction_of_a_nanodegree_off_parallel_is_parallel_below_half_and_not_above() {
    // A solid turned a whole turn about the first line of an xz sketch, from (20, 0) to
    // (20 + d, 10), the rectangle's far side, (23, 0) to (23 + d, 10), parallel to it: its
    // outer wall a cylinder of radius 3 whose axis is off z by atan(d / 10). A box from
    // (30, −5, 0), 10 on each side: its side x = 30 and its upright edge at (30, −5). The
    // angle between the axis and the side (and the edge) is the axis's tilt: at 0.45
    // nanodegrees, 0 at the nanodegree, parallel, the axis's point (20, 0, 0) 10 from the
    // side and √(10² + 5²) = √125 = 11.180340 from the edge; at 0.55, 1 nanodegree, not
    // parallel, so null. (The gap along the axis's 10 mm changes by 1e-10 mm, below the
    // nanometre: one answer either way.)
    let e = engine();
    let tilt = |ndeg: f64| 10.0 * libm::tan((ndeg * 1e-9).to_radians());
    let measure = |d: f64| {
        let m = model(
            &e,
            &[
                polygon(
                    "s0000001",
                    'a',
                    "xz",
                    &[[20.0, 0.0], [20.0 + d, 10.0], [23.0 + d, 10.0], [23.0, 0.0]],
                ),
                revolve("rrrrrrrr", "s0000001", "na000001"),
                block("bbbbbbbb", [30.0, -5.0, 0.0], [10.0; 3]),
            ],
        );
        let wall = the(&m, "rrrrrrrr", |e| e.r#type == ElementType::Cylinder);
        let side = named(&m, "bbbbbbbb", facing([-1.0, 0.0, 0.0]));
        let upright = named(&m, "bbbbbbbb", along([30.0, -5.0, 0.0], [30.0, -5.0, 10.0]));
        let wall_name = wall.name.to_string();
        (
            wall,
            centre_to(&m, &e, &wall_name, &side),
            centre_to(&m, &e, &wall_name, &upright),
        )
    };
    let (wall, side, edge) = measure(tilt(0.45));
    // The case meant: the axis isn't exactly z.
    let direction = wall.axis.as_ref().unwrap().direction;
    assert!(
        direction[0] != 0.0 && direction[0].abs() < 1e-11,
        "{wall:?}"
    );
    assert_eq!(side, Some(mm(10_000_000)), "{wall:?}");
    assert_eq!(edge, Some(mm(11_180_340)), "{wall:?}");
    let (wall, side, edge) = measure(tilt(0.55));
    assert!(direction[0].abs() < wall.axis.as_ref().unwrap().direction[0].abs());
    assert_eq!((side, edge), (None, None), "{wall:?}");
}

#[test]
fn a_fillets_partial_cylinder_and_its_arcs_are_measured_against_the_box_it_rounds() {
    // A 10 mm cube, its top front edge, (0, 0, 10) to (10, 0, 10), rounded by 2: the round
    // is a quarter cylinder whose axis runs along x through y = 2, z = 8. Beside it: the
    // front (y = 0) and the top 2 away, the back (y = 10) and the bottom 8; the back's
    // bottom edge, (0, 10, 0) to (10, 10, 0), √(8² + 8²) = √128 = 11.313708; the side x = 0,
    // which it crosses, null, as the upright edge at the origin, which is (0, 0, 0) to
    // (0, 0, 8) once the round is cut, is.
    let e = engine();
    let base = block("bbbbbbbb", [0.0; 3], [10.0; 3]);
    let m = model(&e, std::slice::from_ref(&base));
    let edge = named(&m, "bbbbbbbb", along([0.0, 0.0, 10.0], [10.0, 0.0, 10.0]));
    let round = format!(
        r#"{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "bbbbbbbb", "edges": [{edge:?}], "radius": 2}}"#
    );
    let m = model(&e, &[base, round]);
    let wall = named(&m, "ffffffff", |e| e.r#type == ElementType::Cylinder);
    let face = |n: [f64; 3]| named(&m, "ffffffff", facing(n));
    let gap = |other: &str| centre_to(&m, &e, &wall, other);
    assert_eq!(gap(&face([0.0, -1.0, 0.0])), Some(mm(2_000_000)), "front");
    assert_eq!(gap(&face([0.0, 0.0, 1.0])), Some(mm(2_000_000)), "top");
    assert_eq!(gap(&face([0.0, 1.0, 0.0])), Some(mm(8_000_000)), "back");
    assert_eq!(gap(&face([0.0, 0.0, -1.0])), Some(mm(8_000_000)), "bottom");
    assert_eq!(gap(&face([-1.0, 0.0, 0.0])), None, "the side x = 0");
    let back_bottom = named(&m, "ffffffff", along([0.0, 10.0, 0.0], [10.0, 10.0, 0.0]));
    assert_eq!(gap(&back_bottom), Some(mm(11_313_708)));
    let upright = named(&m, "ffffffff", along([0.0; 3], [0.0, 0.0, 8.0]));
    assert_eq!(gap(&upright), None);
    // The round's arc at x = 0, centred (0, 2, 8): 10 from x = 10, its foot (10, 2, 8)
    // inside it; 0 from x = 0, which it bounds, its centre inside that face; 8 from the
    // back and the bottom; 2 from the top and the front, whose edges the round's ends
    // are, its feet (0, 2, 10) and (0, 0, 8) on them.
    let arc = named(&m, "ffffffff", centred([0.0, 2.0, 8.0]));
    let from_arc = |other: &str| centre_to(&m, &e, &arc, other);
    assert_eq!(
        from_arc(&face([1.0, 0.0, 0.0])),
        Some(mm(10_000_000)),
        "x = 10"
    );
    assert_eq!(from_arc(&face([-1.0, 0.0, 0.0])), Some(mm(0)), "x = 0");
    assert_eq!(
        from_arc(&face([0.0, 1.0, 0.0])),
        Some(mm(8_000_000)),
        "back"
    );
    assert_eq!(
        from_arc(&face([0.0, 0.0, -1.0])),
        Some(mm(8_000_000)),
        "bottom"
    );
    assert_eq!(from_arc(&face([0.0, 0.0, 1.0])), Some(mm(2_000_000)), "top");
    assert_eq!(
        from_arc(&face([0.0, -1.0, 0.0])),
        Some(mm(2_000_000)),
        "front"
    );
    // To the back's bottom edge, its foot (0, 10, 0) the edge's end: √128.
    assert_eq!(from_arc(&back_bottom), Some(mm(11_313_708)));
}

#[test]
fn a_rims_centre_is_measured_on_the_other_features_shape_either_way_round() {
    // A cylinder of radius 2 about z, 10 high, and a 10 mm cube from (5, −5, 0), two
    // features. The top rim's centre (0, 0, 10): 5 from the cube's side x = 5 (its foot
    // (5, 0, 10) on the side's top edge) and its top (its nearest point (5, 0, 10));
    // √(5² + 10²) = √125 = 11.180340 from its bottom, whose nearest point is (5, 0, 0); the
    // upright edge at (5, −5), its top end (5, −5, 10), √50 = 7.071068. The bottom rim's
    // centre, the origin: 5 from the bottom. Were the point measured on its own feature's
    // shape, the cube's face numbers would name the cylinder's faces, or none.
    let e = engine();
    let m = model(
        &e,
        &[
            cylinder("cccccccc", [0.0; 3], 2.0, 10.0),
            block("bbbbbbbb", [5.0, -5.0, 0.0], [10.0; 3]),
        ],
    );
    let top_rim = named(&m, "cccccccc", centred([0.0, 0.0, 10.0]));
    let bottom_rim = named(&m, "cccccccc", centred([0.0; 3]));
    let face = |n: [f64; 3]| named(&m, "bbbbbbbb", facing(n));
    let upright = named(&m, "bbbbbbbb", along([5.0, -5.0, 0.0], [5.0, -5.0, 10.0]));
    for (what, rim, other, want) in [
        ("the side", &top_rim, face([-1.0, 0.0, 0.0]), 5_000_000),
        ("the top", &top_rim, face([0.0, 0.0, 1.0]), 5_000_000),
        ("the bottom", &top_rim, face([0.0, 0.0, -1.0]), 11_180_340),
        ("the upright", &top_rim, upright.clone(), 7_071_068),
        (
            "the bottom, low",
            &bottom_rim,
            face([0.0, 0.0, -1.0]),
            5_000_000,
        ),
    ] {
        assert_eq!(centre_to(&m, &e, rim, &other), Some(mm(want)), "{what}");
    }
    // A cube's corner as the centre, the cylinder's top (a disc, z = 10, radius 2): the
    // corner (5, −5, 10) is in its plane, √50 − 2 = 5.071068 from its rim.
    let corner = named(&m, "bbbbbbbb", at([5.0, -5.0, 10.0]));
    let disc = named(&m, "cccccccc", facing([0.0, 0.0, 1.0]));
    assert_eq!(
        centre_to(&m, &e, &corner, &disc),
        Length::rounded(50f64.sqrt() - 2.0)
    );
    // The cylinder's seam, the line (2, 0, 0) to (2, 0, 10), beside its own axis: 2.
    let wall = named(&m, "cccccccc", |e| e.r#type == ElementType::Cylinder);
    let seam = named(&m, "cccccccc", along([2.0, 0.0, 0.0], [2.0, 0.0, 10.0]));
    assert_eq!(centre_to(&m, &e, &wall, &seam), Some(mm(2_000_000)));
}

#[test]
fn a_spheres_seam_is_centred_on_it_and_curved_faces_have_no_centre_to_measure() {
    // A sphere of radius 5 at the origin, and a 10 mm cube from (10, −5, −5). The sphere's
    // seam is a half circle about its centre: that centre, (0, 0, 0), is 10 from the cube's
    // side x = 10. The sphere's face has no centre here (center_distance's rule), nor a
    // cone's: null against the side. A pointed cone's apex is a vertex: the apex (0, 0, 30)
    // of a cone from (0, 0, 20), 10 high, is measured to the side as bounded (z only to 5),
    // its nearest point (10, 0, 5): √(10² + 25²) = √725 = 26.925824.
    let e = engine();
    let m = model(
        &e,
        &[
            r#"{"id": "ssssssss", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}"#.to_owned(),
            block("bbbbbbbb", [10.0, -5.0, -5.0], [10.0; 3]),
            r#"{"id": "kkkkkkkk", "type": "cone", "v": 1, "name": "Cone", "position": [0, 0, 20], "radius1": 5, "radius2": 0, "height": 10}"#.to_owned(),
        ],
    );
    let side = named(&m, "bbbbbbbb", facing([-1.0, 0.0, 0.0]));
    let seam = named(&m, "ssssssss", |e| e.r#type == ElementType::Circle);
    assert_eq!(centre_to(&m, &e, &seam, &side), Some(mm(10_000_000)));
    let ball = named(&m, "ssssssss", |e| e.r#type == ElementType::Sphere);
    assert_eq!(centre_to(&m, &e, &ball, &side), None);
    let cone = named(&m, "kkkkkkkk", |e| e.r#type == ElementType::Cone);
    assert_eq!(centre_to(&m, &e, &cone, &side), None);
    let apex = named(&m, "kkkkkkkk", at([0.0, 0.0, 30.0]));
    assert_eq!(
        centre_to(&m, &e, &apex, &side),
        Length::rounded(725f64.sqrt())
    );
}
