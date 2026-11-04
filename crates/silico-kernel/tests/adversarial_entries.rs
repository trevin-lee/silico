//! break-it: M4.4's entries of real solids, rebuilt on kernel workers
//! (`silico_core::command::elements`): cones at their limits (narrowing either way, from a
//! point, all but a cylinder, and with an apex far past the kernel's lengths but within a
//! `Length`), every chamfer face of a box on F1's 45° boundaries, `query`'s `stable` against
//! the element map on booleans of boxes, and orders by a measure only some bodies' faces
//! have. Every expected value by hand from the features' dimensions.

use super::model::{checked, published};

use std::f64::consts::{FRAC_1_SQRT_2, PI};
use std::path::Path;
use std::sync::Arc;

use silico_core::command::elements::{
    AxisToken, ElementType, Entry, Facing, MeasureName, Order, Quantity, Query, Range, entries,
    query,
};
use silico_core::document::{Angle, FeatureId, Length};
use silico_core::kernel::ElementKind;
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

const HEAD: &str =
    r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": ["#;

fn engine() -> Recompute<KernelPool> {
    Recompute::new(KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap())
}

fn rebuilt(engine: &Recompute<KernelPool>, features: &[String]) -> Arc<Model> {
    published(
        engine,
        checked(&format!("{HEAD}{}]}}", features.join(", "))),
    )
}

fn of(model: &Model, feature: &str) -> Vec<Entry> {
    entries(model, &FeatureId::new(feature).unwrap())
        .unwrap()
        .to_vec()
}

fn cone_doc(id: &str, x: f64, r1: f64, r2: f64, h: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "cone", "v": 1, "name": "Cone", "position": [{x}, 0, 0], "radius1": {r1}, "radius2": {r2}, "height": {h}}}"#
    )
}

fn box_doc(id: &str, at: [f64; 3], size: [f64; 3]) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Box", "position": [{}, {}, {}], "length": {}, "width": {}, "height": {}}}"#,
        at[0], at[1], at[2], size[0], size[1], size[2]
    )
}

fn the_cone(entries: &[Entry]) -> &Entry {
    let cones: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.r#type == ElementType::Cone)
        .collect();
    assert_eq!(cones.len(), 1);
    cones[0]
}

fn degrees(a: Option<Angle>) -> f64 {
    a.unwrap().ndeg() as f64 / 1e9
}

#[test]
fn cones_narrowing_either_way_from_a_point_or_all_but_a_cylinder() {
    let engine = engine();
    let model = rebuilt(
        &engine,
        &[
            // Widening up: r 2 at z 0 to 4 at z 6, so its apex is 6 below, at z -6, and
            // its half-angle atan(2 / 6) = 18.434948822922010°.
            cone_doc("aaaaaaaa", 0.0, 2.0, 4.0, 6.0),
            // From a point: r 0 at z 0 to 4 at z 6, its apex at its base point, its
            // half-angle atan(4 / 6) = 33.690067525979785°.
            cone_doc("bbbbbbbb", 100.0, 0.0, 4.0, 6.0),
            // All but a cylinder: r 1e6 to 1e6 - 1e-6 over 1e5, valid from lengths a
            // primitive takes. Its apex is 1e6 · 1e5 / 1e-6 = 1e17 mm up, past what a
            // Length holds: none. Its half-angle -atan(1e-11) = -5.7e-10°, -1 nanodegree.
            cone_doc("cccccccc", 0.0, 1e6, 999_999.999_999, 1e5),
            // r 1e6 to 1e6 - 1 over 1e6: its apex 1e6 · 1e6 / 1 = 1e12 mm up, far past the
            // kernel's lengths but within a Length's 9.2e12 mm.
            cone_doc("dddddddd", 0.0, 1e6, 999_999.0, 1e6),
        ],
    );
    let widening = of(&model, "aaaaaaaa");
    let cone = the_cone(&widening);
    assert!(
        (degrees(cone.half_angle) - 18.434_948_822_922_01).abs() <= 1e-9,
        "{:?}",
        cone.half_angle
    );
    let apex = cone.apex.unwrap().map(Length::mm);
    for (got, want) in apex.iter().zip([0.0, 0.0, -6.0]) {
        assert!((got - want).abs() <= 1e-9, "apex {apex:?}");
    }
    let pointed = of(&model, "bbbbbbbb");
    let cone = the_cone(&pointed);
    assert!(
        (degrees(cone.half_angle) - 33.690_067_525_979_785).abs() <= 1e-9,
        "{:?}",
        cone.half_angle
    );
    let apex = cone.apex.unwrap().map(Length::mm);
    for (got, want) in apex.iter().zip([100.0, 0.0, 0.0]) {
        assert!((got - want).abs() <= 1e-9, "apex {apex:?}");
    }
    let nearly = of(&model, "cccccccc");
    let cone = the_cone(&nearly);
    assert_eq!(cone.apex, None);
    assert!(
        cone.half_angle.unwrap().ndeg().abs() <= 1,
        "{:?}",
        cone.half_angle
    );
    // Its axis and area stand: 2π r slant ≈ 2π · 1e6 · 1e5.
    assert!(cone.axis.is_some());
    let area = cone.area.unwrap().millionths() as f64 / 1e6;
    let want = PI * (1e6 + 999_999.999_999) * (1e10f64 + 1e-12).sqrt();
    assert!(
        (area - want).abs() <= 1e-9 * want + 1e-6,
        "{area} not {want}"
    );
    let far = of(&model, "dddddddd");
    let cone = the_cone(&far);
    let apex = cone.apex.unwrap().map(Length::mm);
    assert!(apex[0].abs() <= 1e-6 && apex[1].abs() <= 1e-6, "{apex:?}");
    assert!(
        (apex[2] - 1e12).abs() <= 1.0,
        "apex {apex:?}, not at z = 1e12"
    );
}

#[test]
fn every_chamfer_face_of_a_box_takes_f1s_default_from_its_normal_as_written() {
    // A 10 mm cube with each of its edges chamfered 1 mm in turn: each chamfer face is a
    // plane whose normal is two axes' bisector, (±1, ±1, 0)/√2 or the like, so it sits on
    // one of F1's 45° ties. Its default_x is the table's on its normal as written, ties or
    // not; and it faces each of those two axes 45° off, as written.
    let engine = engine();
    let mut seen = Vec::new();
    for edge in 1..=12 {
        let model = rebuilt(
            &engine,
            &[
                box_doc("aaaaaaaa", [0.0; 3], [10.0; 3]),
                format!(
                    r#"{{"id": "cccccccc", "type": "chamfer", "v": 1, "name": "Chamfer", "base": "aaaaaaaa", "edges": ["aaaaaaaa.box.Edge{edge}"], "size": {{"distance": 1}}}}"#
                ),
            ],
        );
        let entries = of(&model, "cccccccc");
        let slanted: Vec<&Entry> = entries
            .iter()
            .filter(|e| {
                e.normal
                    .is_some_and(|n| n.iter().filter(|c| c.abs() > 1e-9).count() == 2)
            })
            .collect();
        assert_eq!(slanted.len(), 1, "Edge{edge}");
        let face = slanted[0];
        let n = face.normal.unwrap();
        // √2 · 10 · 1 mm of chamfer face: 10 √2.
        let area = face.area.unwrap().millionths() as f64 / 1e6;
        assert!(
            (area - 10.0 * 2f64.sqrt()).abs() <= 1e-6,
            "Edge{edge}: {area}"
        );
        let mut axes = Vec::new();
        for (i, c) in n.iter().enumerate() {
            if c.abs() > 1e-9 {
                assert!(
                    (c.abs() - FRAC_1_SQRT_2).abs() <= 1e-15,
                    "Edge{edge}: {n:?}"
                );
                let mut axis = [0.0; 3];
                axis[i] = c.signum();
                axes.push(axis);
            } else {
                assert!(c.abs() <= 1e-15, "Edge{edge}: {n:?}");
            }
        }
        assert_eq!(face.default_x, Some(AxisToken::default_x(n)), "Edge{edge}");
        // OCCT's chamfer normals tie exactly, in bits (break-it's run: ±0.7071067811865476
        // twice on each), so the table's tie rule decides, by hand: x over y and z gives
        // ±y by x's sign; y over z gives -x for +y, +x for -y.
        let big: Vec<u64> = n
            .iter()
            .filter(|c| c.abs() > 1e-9)
            .map(|c| c.abs().to_bits())
            .collect();
        assert_eq!(big[0], big[1], "Edge{edge}: {n:?} doesn't tie in bits");
        let by_hand = if n[0].abs() > 1e-9 {
            if n[0] > 0.0 {
                AxisToken::PlusY
            } else {
                AxisToken::MinusY
            }
        } else if n[1] > 0.0 {
            AxisToken::MinusX
        } else {
            AxisToken::PlusX
        };
        assert_eq!(face.default_x, Some(by_hand), "Edge{edge}: {n:?}");
        for axis in axes {
            for (within, faces) in [(45_000_000_000, true), (44_999_999_999, false)] {
                let q = Query {
                    feature: Some(FeatureId::new("cccccccc").unwrap()),
                    kind: ElementKind::Face,
                    r#type: None,
                    stable: None,
                    facing: Some(Facing {
                        direction: axis,
                        within: Angle::from_ndeg(within),
                    }),
                    measures: Vec::new(),
                    order: None,
                    limit: 1000,
                };
                let got = query(&model, &q).unwrap();
                assert_eq!(
                    got.entries.iter().any(|e| e.name == face.name),
                    faces,
                    "Edge{edge}'s chamfer face {n:?} against {axis:?} within {within} ndeg"
                );
            }
        }
        seen.push((edge, n.map(f64::to_bits), face.default_x));
    }
    // Twelve chamfer faces, each on one of the cube's twelve bisectors.
    let mut normals: Vec<[u64; 3]> = seen.iter().map(|(_, n, _)| *n).collect();
    normals.sort();
    normals.dedup();
    assert_eq!(normals.len(), 12, "{seen:?}");
}

/// Booleans of boxes coincident but for one face, sharing a face, cut by one sharing a side, slotted, and
/// overlapping, each a document whose last feature is the boolean: layouts that might give
/// an element an alias or a duplicate (`~n`), though none does on OCCT's histories (see
/// the test below; core's tests/adversarial_entries.rs makes an unstable name on the fake,
/// and command::elements's unit test an alias).
fn layouts() -> Vec<(&'static str, Vec<String>)> {
    let unit = [10.0; 3];
    let op = |id: &str, op: &str, base: &str, tool: &str| {
        format!(
            r#"{{"id": "{id}", "type": "{op}", "v": 1, "name": "Op", "base": "{base}", "tool": "{tool}"}}"#
        )
    };
    vec![
        (
            // Coincident but for the tool's top, 1 mm higher, so the fuse adds to its base
            // (S7's rule refuses one that adds nothing): every other face shared.
            "coincident boxes, the tool a millimetre taller, fused",
            vec![
                box_doc("aaaaaaaa", [0.0; 3], unit),
                box_doc("bbbbbbbb", [0.0, 0.0, 0.0], [10.0, 10.0, 11.0]),
                op("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "boxes sharing a face fused",
            vec![
                box_doc("aaaaaaaa", [0.0; 3], unit),
                box_doc("bbbbbbbb", [10.0, 0.0, 0.0], unit),
                op("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "a box cut by one sharing its top",
            vec![
                box_doc("aaaaaaaa", [0.0; 3], unit),
                box_doc("bbbbbbbb", [2.0, 2.0, 5.0], [6.0, 6.0, 5.0]),
                op("rrrrrrrr", "cut", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "a box with a slot cut across its bottom",
            vec![
                box_doc("aaaaaaaa", [0.0; 3], unit),
                box_doc("bbbbbbbb", [4.0, -1.0, -1.0], [2.0, 12.0, 6.0]),
                op("rrrrrrrr", "cut", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "boxes overlapping, common",
            vec![
                box_doc("aaaaaaaa", [0.0; 3], unit),
                box_doc("bbbbbbbb", [5.0, 5.0, 0.0], unit),
                op("rrrrrrrr", "common", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
    ]
}

#[test]
fn stable_keeps_elements_with_any_stable_name_by_the_element_map() {
    // Booleans of coincident, touching, and overlapping boxes: `stable: true` keeps exactly
    // the elements one of whose names (its own or an alias) the element map holds stable,
    // and `false` the rest; an entry's own `stable` and its aliases' are the map's.
    let engine = engine();
    for (layout, features) in layouts() {
        let model = rebuilt(&engine, &features);
        let id = FeatureId::new("rrrrrrrr").unwrap();
        let map = model
            .element_map(&id)
            .unwrap_or_else(|| panic!("{layout}: {:?}", model.outcome(&id)));
        for kind in [ElementKind::Face, ElementKind::Edge, ElementKind::Vertex] {
            let q = |stable: Option<bool>| Query {
                feature: Some(id.clone()),
                kind,
                r#type: None,
                stable,
                facing: None,
                measures: Vec::new(),
                order: None,
                limit: 1000,
            };
            let all = query(&model, &q(None)).unwrap().entries;
            let kept = query(&model, &q(Some(true))).unwrap().entries;
            let rest = query(&model, &q(Some(false))).unwrap().entries;
            assert_eq!(kept.len() + rest.len(), all.len(), "{layout} {kind:?}");
            for entry in &all {
                let element = map.element(&entry.name).unwrap();
                let names = map.names(element);
                assert_eq!(*names[0], *entry.name, "{layout}");
                assert_eq!(
                    entry
                        .aliases
                        .iter()
                        .map(|a| a.name.to_string())
                        .collect::<Vec<_>>(),
                    names[1..].iter().map(|n| n.to_string()).collect::<Vec<_>>(),
                    "{layout}"
                );
                for alias in &entry.aliases {
                    assert_eq!(alias.stable, !map.is_unstable(&alias.name), "{layout}");
                }
                assert_eq!(entry.stable, !map.is_unstable(&entry.name), "{layout}");
                let any = names.iter().any(|n| !map.is_unstable(n));
                assert_eq!(kept.contains(entry), any, "{layout}: {}", entry.name);
                assert_eq!(rest.contains(entry), !any, "{layout}: {}", entry.name);
            }
        }
    }
    // None of these layouts reaches an alias or an unstable own name: every name here is
    // stable, so `stable: false` keeps nothing; checked against the map either way.
}

#[test]
fn an_order_by_radius_across_bodies_keeps_only_the_faces_that_have_one() {
    // A box, a cylinder r 2.5, a cone, a sphere r 3, and a torus 10 and 2: of their faces,
    // only the cylinder's side and the sphere have a radius (a cone's and a torus's aren't
    // an entry's), so by radius, largest first: the sphere's 3, then the side's 2.5; by
    // diameter only the side, 5.
    let engine = engine();
    let model = rebuilt(
        &engine,
        &[
            box_doc("bbbbbbbb", [0.0; 3], [10.0, 20.0, 30.0]),
            r#"{"id": "cccccccc", "type": "cylinder", "v": 1, "name": "Cylinder", "position": [100, 0, 0], "radius": 2.5, "height": 10}"#.to_owned(),
            cone_doc("kkkkkkkk", 200.0, 4.0, 0.0, 6.0),
            r#"{"id": "ssssssss", "type": "sphere", "v": 1, "name": "Sphere", "position": [300, 0, 0], "radius": 3}"#.to_owned(),
            r#"{"id": "tttttttt", "type": "torus", "v": 1, "name": "Torus", "position": [400, 0, 0], "radius1": 10, "radius2": 2}"#.to_owned(),
        ],
    );
    let by = |measure: MeasureName, limit: u32| {
        query(
            &model,
            &Query {
                feature: None,
                kind: ElementKind::Face,
                r#type: None,
                stable: None,
                facing: None,
                measures: Vec::new(),
                order: Some(Order {
                    by: measure,
                    descending: true,
                }),
                limit,
            },
        )
        .unwrap()
    };
    let radii = by(MeasureName::Radius, 1000);
    assert_eq!(radii.matched, 2);
    let got: Vec<(&str, i64)> = radii
        .entries
        .iter()
        .map(|e| (e.feature.as_str(), e.radius.unwrap().nm()))
        .collect();
    assert_eq!(got, [("ssssssss", 3_000_000), ("cccccccc", 2_500_000)]);
    let diameters = by(MeasureName::Diameter, 1);
    assert_eq!(diameters.matched, 1);
    assert_eq!(
        diameters.entries[0].diameter,
        Some(Length::from_nm(5_000_000))
    );
    assert_eq!(diameters.tied, None);
    // By area, largest first: the torus's 80π² ≈ 789.6 alone; then 2: the box's two
    // 600 mm² faces tie at the cut, both returned.
    let largest = by(MeasureName::Area, 1);
    assert_eq!(largest.entries.len(), 1);
    assert_eq!(largest.entries[0].feature.as_str(), "tttttttt");
    assert_eq!(largest.tied, None);
    let two = by(MeasureName::Area, 2);
    assert_eq!(two.entries.len(), 3);
    assert!(
        two.entries[1..].iter().all(
            |e| e.feature.as_str() == "bbbbbbbb" && e.area.unwrap().millionths() == 600_000_000
        )
    );
    let tied = two.tied.unwrap();
    assert_eq!((tied.count, tied.included), (2, true));
    // The tie keeps `elements`' order: by name, Face1 (x = 0) before Face2 (x = 10).
    let names: Vec<String> = two.entries[1..]
        .iter()
        .map(|e| e.name.to_string())
        .collect();
    assert_eq!(names, ["bbbbbbbb.box.Face1", "bbbbbbbb.box.Face2"]);
}

#[test]
fn tori_are_ordered_and_kept_by_their_major_and_minor_radii_each() {
    // A torus 10 and 2 and a torus 6 and 3: by major radius, largest first, 10 then 6; by
    // minor radius, 3 then 2, the other way round; a major radius from 9 to 11 keeps the
    // first alone, a minor radius from 2.5 to 4 the second.
    let engine = engine();
    let model = rebuilt(
        &engine,
        &[
            r#"{"id": "tttttttt", "type": "torus", "v": 1, "name": "Torus", "position": [0, 0, 0], "radius1": 10, "radius2": 2}"#.to_owned(),
            r#"{"id": "vvvvvvvv", "type": "torus", "v": 1, "name": "Torus001", "position": [100, 0, 0], "radius1": 6, "radius2": 3}"#.to_owned(),
        ],
    );
    let mm = |v: f64| Some(Quantity::Length(Length::from_nm((v * 1e6) as i64)));
    let ask = |measures: Vec<Range>, order: Option<MeasureName>| {
        let found = query(
            &model,
            &Query {
                measures,
                order: order.map(|by| Order {
                    by,
                    descending: true,
                }),
                ..Query::of(ElementKind::Face, 1000)
            },
        )
        .unwrap();
        found
            .entries
            .iter()
            .map(|e| {
                (
                    e.feature.as_str().to_owned(),
                    e.major_radius.unwrap().nm(),
                    e.minor_radius.unwrap().nm(),
                )
            })
            .collect::<Vec<_>>()
    };
    let first = ("tttttttt".to_owned(), 10_000_000, 2_000_000);
    let second = ("vvvvvvvv".to_owned(), 6_000_000, 3_000_000);
    assert_eq!(
        ask(Vec::new(), Some(MeasureName::MajorRadius)),
        [first.clone(), second.clone()]
    );
    assert_eq!(
        ask(Vec::new(), Some(MeasureName::MinorRadius)),
        [second.clone(), first.clone()]
    );
    let range = |measure, at_least, at_most| Range {
        measure,
        at_least,
        at_most,
    };
    assert_eq!(
        ask(
            vec![range(MeasureName::MajorRadius, mm(9.0), mm(11.0))],
            None
        ),
        [first]
    );
    assert_eq!(
        ask(
            vec![range(MeasureName::MinorRadius, mm(2.5), mm(4.0))],
            None
        ),
        [second]
    );
}
