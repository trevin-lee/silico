//! break-it: M2.7's element map and the topology it reads, on the real kernel. The fake's
//! hand-written primitive topologies against OCCT's (the real kernel is the oracle), and
//! every element map a real recompute publishes for hostile boolean layouts (coincident
//! solids that share a step, disjoint, face-, edge-, and vertex-touching operands, a cut
//! that removes nothing or splits the body, nested booleans, curved operands): each map
//! names every element of its solid, each name finds its element, and nothing changes
//! between two recomputes, a cached one, or an edit that moves no element.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    BuiltData, ElementKind, ElementRef, Kernel, KernelRequest, Primitive, ShapeData,
};
use silico_core::naming::{ElementMap, Name};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

fn pool() -> KernelPool {
    KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2).unwrap()
}

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn done<T>(pending: silico_core::pool::Pending<T>) -> T {
    pending
        .wait_timeout(Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("no outcome within a minute"))
        .done()
        .expect("superseded")
        .unwrap()
}

fn make<K: Kernel>(kernel: &K, primitive: Primitive) -> ShapeData {
    done(kernel.submit(kernel.generation(), KernelRequest::make(primitive).unwrap()))
}

fn translate<K: Kernel>(kernel: &K, shape: &ShapeData, offset: [f64; 3]) -> BuiltData {
    done(kernel.submit(
        kernel.generation(),
        KernelRequest::translate(shape, offset).unwrap(),
    ))
}

const ABOVE_TOLERANCE: f64 = 1.0000000000000002e-7;

// --- The fake's primitive topology against OCCT's --------------------------------------

/// Each primitive's topology as a move reports it (a move keeps every element's number),
/// on the fake and on the real kernel: the fake's tables say they're OCCT's, so a core test
/// naming elements from above or below on the fake names them as OCCT's would be named.
fn fake_and_real_topology(primitive: Primitive) -> (String, String) {
    let real = pool();
    let fake = FakeKernel::new(1).unwrap();
    let of = |history: &silico_core::kernel::History| format!("{:?}", history.topology);
    let r = translate(&real, &make(&real, primitive), [1.0, 0.0, 0.0]);
    let f = translate(&fake, &make(&fake, primitive), [1.0, 0.0, 0.0]);
    assert_eq!(r.history.result, f.history.result, "{primitive:?}: counts");
    (of(&f.history), of(&r.history))
}

#[test]
fn the_fakes_box_topology_is_occts() {
    let (fake, real) = fake_and_real_topology(Primitive::Box {
        dx: 1.0,
        dy: 2.0,
        dz: 3.0,
    });
    assert_eq!(fake, real);
}

#[test]
fn the_fakes_cylinder_topology_is_occts() {
    let (fake, real) = fake_and_real_topology(Primitive::Cylinder {
        radius: 2.0,
        height: 5.0,
    });
    assert_eq!(fake, real);
}

#[test]
fn the_fakes_sphere_topology_is_occts() {
    let (fake, real) = fake_and_real_topology(Primitive::Sphere { radius: 3.0 });
    assert_eq!(fake, real);
}

#[test]
fn the_fakes_torus_topology_is_occts() {
    let (fake, real) = fake_and_real_topology(Primitive::Torus {
        major_radius: 10.0,
        minor_radius: 2.0,
    });
    assert_eq!(fake, real);
}

#[test]
fn the_fakes_horn_torus_topology_is_occts() {
    // Minor radius equal to the major: the tube meets itself at the axis.
    let (fake, real) = fake_and_real_topology(Primitive::Torus {
        major_radius: 2.0,
        minor_radius: 2.0,
    });
    assert_eq!(fake, real);
}

#[test]
fn the_fakes_frustum_topology_is_occts() {
    let (fake, real) = fake_and_real_topology(Primitive::Cone {
        radius1: 2.0,
        radius2: 4.0,
        height: 10.0,
    });
    assert_eq!(fake, real);
}

#[test]
fn the_fakes_cone_with_its_apex_on_top_has_occts_topology() {
    let (fake, real) = fake_and_real_topology(Primitive::Cone {
        radius1: 3.0,
        radius2: 0.0,
        height: 4.0,
    });
    assert_eq!(fake, real);
}

#[test]
fn the_fakes_cone_with_its_apex_at_the_base_has_occts_topology() {
    let (fake, real) = fake_and_real_topology(Primitive::Cone {
        radius1: 0.0,
        radius2: 3.0,
        height: 4.0,
    });
    assert_eq!(fake, real);
}

#[test]
fn the_fakes_cone_with_a_top_radius_a_hair_above_tolerance_has_occts_topology() {
    let (fake, real) = fake_and_real_topology(Primitive::Cone {
        radius1: 1.0,
        radius2: ABOVE_TOLERANCE,
        height: 1.0,
    });
    assert_eq!(fake, real);
}

#[test]
fn the_fakes_cone_with_a_base_radius_a_hair_above_tolerance_has_occts_topology() {
    let (fake, real) = fake_and_real_topology(Primitive::Cone {
        radius1: ABOVE_TOLERANCE,
        radius2: 1.0,
        height: 1.0,
    });
    assert_eq!(fake, real);
}

// --- Every map a real recompute publishes ------------------------------------------------

const HEAD: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": ["#;

fn boxed(id: &str, at: [f64; 3], size: [f64; 3]) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "{id}", "position": [{}, {}, {}], "length": {}, "width": {}, "height": {}}}"#,
        at[0], at[1], at[2], size[0], size[1], size[2]
    )
}

fn cylinder(id: &str, at: [f64; 3], radius: f64, height: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "cylinder", "v": 1, "name": "{id}", "position": [{}, {}, {}], "radius": {radius}, "height": {height}}}"#,
        at[0], at[1], at[2]
    )
}

fn sphere(id: &str, at: [f64; 3], radius: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "sphere", "v": 1, "name": "{id}", "position": [{}, {}, {}], "radius": {radius}}}"#,
        at[0], at[1], at[2]
    )
}

fn torus(id: &str, at: [f64; 3], major: f64, minor: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "torus", "v": 1, "name": "{id}", "position": [{}, {}, {}], "radius1": {major}, "radius2": {minor}}}"#,
        at[0], at[1], at[2]
    )
}

fn op(id: &str, kind: &str, base: &str, tool: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "{kind}", "v": 1, "name": "{id}", "base": "{base}", "tool": "{tool}"}}"#
    )
}

fn document(features: &[String]) -> Checked {
    let text = format!("{HEAD}{}]}}", features.join(",\n"));
    Checked::new(read(text.as_bytes()).unwrap_or_else(|e| panic!("{e}\n{text}")))
        .unwrap_or_else(|e| panic!("{e:?}\n{text}"))
}

fn publish<K: Kernel + 'static>(engine: &Recompute<K>, document: Checked) -> Arc<Model> {
    match engine
        .start(document)
        .wait_timeout(Duration::from_secs(120))
    {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish in two minutes"),
    }
}

/// Every map in `model` holds together against its solid's counts: each element of the
/// solid named, nothing named the solid hasn't got, each name finding its own element, no
/// name on two elements, and no two names written alike. Returns every built feature's
/// names in Silico's form, by element, for comparing recomputes.
fn check_maps(model: &Model, case: &str) -> BTreeMap<String, Vec<(ElementRef, Vec<String>)>> {
    let mut all = BTreeMap::new();
    for (feature, outcome) in model.features() {
        let FeatureOutcome::Built { built, .. } = outcome else {
            assert!(
                model.element_map(feature).is_none(),
                "{case}: {feature} didn't build but has names"
            );
            continue;
        };
        let map = model
            .element_map(feature)
            .unwrap_or_else(|| panic!("{case}: {feature} built but has no names"));
        let summary = built
            .summary()
            .unwrap_or_else(|| panic!("{case}: {feature} unmeasured"));
        let counts = [
            (ElementKind::Face, summary.faces),
            (ElementKind::Edge, summary.edges),
            (ElementKind::Vertex, summary.vertices),
        ];
        let mut texts = BTreeSet::new();
        let mut listed = Vec::new();
        for (kind, count) in counts {
            for index in 1..=count {
                let element = ElementRef { kind, index };
                let names = map.names(element);
                assert!(
                    !names.is_empty(),
                    "{case}: {feature}'s {element} has no name"
                );
                assert_eq!(
                    map.name(element).as_ref(),
                    names.first(),
                    "{case}: {feature}'s {element}"
                );
                for name in &names {
                    assert_eq!(
                        map.element(name),
                        Some(element),
                        "{case}: {feature}'s {element}'s name {name} finds another element"
                    );
                    assert!(
                        texts.insert(name.to_string()),
                        "{case}: {feature}: two names written {name}"
                    );
                }
                listed.push((element, names.iter().map(|n| n.to_string()).collect()));
            }
        }
        // Nothing beyond the solid's counts is named.
        for (element, _) in map.iter() {
            let count = counts
                .iter()
                .find(|(k, _)| *k == element.kind)
                .map_or(0, |(_, c)| *c);
            assert!(
                (1..=count).contains(&element.index),
                "{case}: {feature} names {element}, which its solid hasn't got"
            );
        }
        all.insert(feature.to_string(), listed);
    }
    all
}

/// The hostile layouts, each a document whose last feature is the boolean of interest.
fn layouts() -> Vec<(&'static str, Vec<String>)> {
    let unit = [10.0, 10.0, 10.0];
    vec![
        (
            // Two identical boxes in the same place: one step key for both, so the kernel
            // is handed the same shape's bytes as base and tool.
            "coincident boxes fused (one step for both)",
            vec![
                boxed("aaaaaaaa", [0.0, 0.0, 0.0], unit),
                boxed("bbbbbbbb", [0.0, 0.0, 0.0], unit),
                op("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "coincident boxes common",
            vec![
                boxed("aaaaaaaa", [0.0, 0.0, 0.0], unit),
                boxed("bbbbbbbb", [0.0, 0.0, 0.0], unit),
                op("rrrrrrrr", "common", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "disjoint boxes fused (a compound of two solids)",
            vec![
                boxed("aaaaaaaa", [0.0, 0.0, 0.0], unit),
                boxed("bbbbbbbb", [50.0, 0.0, 0.0], unit),
                op("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "boxes sharing a whole face, fused",
            vec![
                boxed("aaaaaaaa", [0.0, 0.0, 0.0], unit),
                boxed("bbbbbbbb", [10.0, 0.0, 0.0], unit),
                op("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "boxes sharing part of a face, fused",
            vec![
                boxed("aaaaaaaa", [0.0, 0.0, 0.0], unit),
                boxed("bbbbbbbb", [10.0, 5.0, 5.0], unit),
                op("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "boxes touching along an edge, fused",
            vec![
                boxed("aaaaaaaa", [0.0, 0.0, 0.0], unit),
                boxed("bbbbbbbb", [10.0, 10.0, 0.0], unit),
                op("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "boxes touching at a corner, fused",
            vec![
                boxed("aaaaaaaa", [0.0, 0.0, 0.0], unit),
                boxed("bbbbbbbb", [10.0, 10.0, 10.0], unit),
                op("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "a cut that removes nothing",
            vec![
                boxed("aaaaaaaa", [0.0, 0.0, 0.0], unit),
                boxed("bbbbbbbb", [50.0, 0.0, 0.0], unit),
                op("rrrrrrrr", "cut", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "a cut that splits the body in two",
            vec![
                boxed("aaaaaaaa", [0.0, 0.0, 0.0], [30.0, 10.0, 10.0]),
                boxed("bbbbbbbb", [10.0, -5.0, -5.0], [10.0, 20.0, 20.0]),
                op("rrrrrrrr", "cut", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "a blind hole cut from a top face",
            vec![
                boxed("aaaaaaaa", [0.0, 0.0, 0.0], [40.0, 20.0, 10.0]),
                cylinder("bbbbbbbb", [20.0, 10.0, 5.0], 2.5, 10.0),
                op("rrrrrrrr", "cut", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "a sphere cut by a cylinder through its poles",
            vec![
                sphere("aaaaaaaa", [0.0, 0.0, 0.0], 5.0),
                cylinder("bbbbbbbb", [0.0, 0.0, -10.0], 1.0, 20.0),
                op("rrrrrrrr", "cut", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "a torus fused with a box through its tube",
            vec![
                torus("aaaaaaaa", [0.0, 0.0, 0.0], 10.0, 2.0),
                boxed("bbbbbbbb", [8.0, -1.0, -5.0], [4.0, 2.0, 10.0]),
                op("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "a cylinder in a cylinder, the same radius and axis (coincident walls)",
            vec![
                cylinder("aaaaaaaa", [0.0, 0.0, 0.0], 3.0, 10.0),
                cylinder("bbbbbbbb", [0.0, 0.0, 5.0], 3.0, 10.0),
                op("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            // Names told apart only by rank: four loops `G1/4` to `G4/4`, the seam's
            // three pieces (tests/name_collisions.rs).
            "a torus cut by a pin through its tube",
            vec![
                torus("aaaaaaaa", [0.0, 0.0, 0.0], 10.0, 3.0),
                cylinder("bbbbbbbb", [10.0, 0.0, -5.0], 1.0, 10.0),
                op("rrrrrrrr", "cut", "aaaaaaaa", "bbbbbbbb"),
            ],
        ),
        (
            "a groove across a U-shaped face (three pieces ranked)",
            vec![
                boxed("aaaaaaaa", [0.0, 0.0, 0.0], [40.0, 20.0, 5.0]),
                boxed("bbbbbbbb", [15.0, -1.0, -1.0], [10.0, 12.0, 7.0]),
                op("cccccccc", "cut", "aaaaaaaa", "bbbbbbbb"),
                boxed("dddddddd", [-1.0, 3.0, 4.0], [42.0, 2.0, 2.0]),
                op("rrrrrrrr", "cut", "cccccccc", "dddddddd"),
            ],
        ),
        (
            "nested: a cut of a fuse, then a common",
            vec![
                boxed("aaaaaaaa", [0.0, 0.0, 0.0], unit),
                boxed("bbbbbbbb", [5.0, 5.0, 0.0], unit),
                op("cccccccc", "fuse", "aaaaaaaa", "bbbbbbbb"),
                cylinder("dddddddd", [7.5, 7.5, -1.0], 2.0, 12.0),
                op("eeeeeeee", "cut", "cccccccc", "dddddddd"),
                sphere("ffffffff", [7.5, 7.5, 5.0], 8.0),
                op("rrrrrrrr", "common", "eeeeeeee", "ffffffff"),
            ],
        ),
    ]
}

#[test]
fn every_map_a_recompute_publishes_names_each_element_once_for_hostile_layouts() {
    let engine = Recompute::new(pool());
    let mut failures = Vec::new();
    for (case, features) in layouts() {
        let model = publish(&engine, document(&features));
        // Every layout here builds (none is empty), so a map check can't pass vacuously.
        if !matches!(
            model.outcome(&id("rrrrrrrr")),
            Some(FeatureOutcome::Built { .. })
        ) {
            failures.push(format!(
                "{case}: didn't build: {:?}",
                model.outcome(&id("rrrrrrrr"))
            ));
            continue;
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check_maps(&model, case);
        }));
        if let Err(e) = outcome {
            let message = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default();
            failures.push(message);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn names_are_the_same_from_two_pools_and_from_the_cache() {
    // The same documents recomputed on fresh pools, and from a cache another recompute
    // filled: names can't depend on which worker ran a step or whether it was rebuilt.
    let dir = std::env::temp_dir().join(format!("silico-break-naming-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let secret = dir.join("secret");
    std::fs::create_dir_all(&dir).unwrap();
    let cache = || silico_core::cache::Cache::with_secret(dir.join("cache"), &secret);
    let first = Recompute::with_cache(pool(), cache());
    let mut expected = Vec::new();
    for (case, features) in layouts() {
        expected.push(check_maps(&publish(&first, document(&features)), case));
    }
    drop(first);
    let second = Recompute::new(pool());
    let cached = Recompute::with_cache(pool(), cache());
    for ((case, features), expected) in layouts().into_iter().zip(&expected) {
        let again = check_maps(&publish(&second, document(&features)), case);
        assert_eq!(&again, expected, "{case}: a second pool");
        let from_cache = check_maps(&publish(&cached, document(&features)), case);
        assert_eq!(&from_cache, expected, "{case}: from the cache");
    }
}

/// The primary names of a feature's map, as a set: what a reference could hold.
fn name_set(model: &Model, feature: &str) -> BTreeSet<String> {
    model
        .element_map(&id(feature))
        .unwrap()
        .iter()
        .map(|(_, names)| names[0].to_string())
        .collect()
}

fn primaries(map: &ElementMap) -> Vec<Arc<Name>> {
    map.iter().map(|(_, names)| Arc::clone(&names[0])).collect()
}

#[test]
fn a_hole_moved_within_its_face_keeps_every_name() {
    // The design's example: the pin moved along the block's top (still well inside it).
    // No element appears or goes, so every name a reference could hold must survive.
    let at = |x: f64| {
        document(&[
            boxed("k7f2qx3m", [0.0, 0.0, 0.0], [40.0, 20.0, 10.0]),
            cylinder("a91cw0de", [x, 10.0, -1.0], 2.5, 12.0),
            op("m3tq8v0z", "cut", "k7f2qx3m", "a91cw0de"),
        ])
    };
    let engine = Recompute::new(pool());
    let before = name_set(&publish(&engine, at(20.0)), "m3tq8v0z");
    for x in [5.0, 12.25, 30.0, 34.0] {
        let after = name_set(&publish(&engine, at(x)), "m3tq8v0z");
        assert_eq!(after, before, "pin at x = {x}");
    }
}

#[test]
fn a_block_made_longer_keeps_every_name_of_its_hole() {
    let long = |length: f64| {
        document(&[
            boxed("k7f2qx3m", [0.0, 0.0, 0.0], [length, 20.0, 10.0]),
            cylinder("a91cw0de", [10.0, 10.0, -1.0], 2.5, 12.0),
            op("m3tq8v0z", "cut", "k7f2qx3m", "a91cw0de"),
        ])
    };
    let engine = Recompute::new(pool());
    let before = name_set(&publish(&engine, long(40.0)), "m3tq8v0z");
    for length in [20.0, 41.0, 1000.0] {
        let after = name_set(&publish(&engine, long(length)), "m3tq8v0z");
        assert_eq!(after, before, "block {length} long");
    }
}

#[test]
fn no_name_in_a_hostile_layout_is_a_duplicate_or_a_bare_number() {
    // Names that are `~n` duplicates or bare numbers can't follow an element through an
    // edit (N3, and FreeCAD's unmapped names): for these simple booleans OCCT reports every
    // element, so every element should have a name from history or its neighbours. This
    // lists which layouts fall back, if any, rather than failing on the first.
    let engine = Recompute::new(pool());
    let mut fallbacks = Vec::new();
    for (case, features) in layouts() {
        let model = publish(&engine, document(&features));
        let Some(map) = model.element_map(&id("rrrrrrrr")) else {
            continue;
        };
        for name in primaries(map) {
            let bad = match &*name {
                Name::Duplicate { .. } => true,
                Name::Element { feature, .. } => *feature == id("rrrrrrrr"),
                _ => false,
            };
            if bad {
                fallbacks.push(format!("{case}: {name}"));
            }
        }
    }
    assert!(fallbacks.is_empty(), "{fallbacks:#?}");
}

#[test]
fn a_boolean_with_nothing_left_has_no_names_and_what_follows_still_names() {
    // A common of disjoint boxes and a cut by a tool that swallows the base leave nothing:
    // the feature fails (no map, no topology of an empty shape), what uses it is blocked,
    // and an unrelated boolean after it still gets its names.
    let unit = [10.0, 10.0, 10.0];
    let engine = Recompute::new(pool());
    let model = publish(
        &engine,
        document(&[
            boxed("aaaaaaaa", [0.0, 0.0, 0.0], unit),
            boxed("bbbbbbbb", [50.0, 0.0, 0.0], unit),
            op("cccccccc", "common", "aaaaaaaa", "bbbbbbbb"),
            boxed("dddddddd", [0.0, 0.0, 0.0], unit),
            boxed("eeeeeeee", [-5.0, -5.0, -5.0], [20.0, 20.0, 20.0]),
            op("ffffffff", "cut", "dddddddd", "eeeeeeee"),
            boxed("gggggggg", [0.0, 0.0, 0.0], unit),
            op("hhhhhhhh", "fuse", "cccccccc", "gggggggg"),
            boxed("jjjjjjjj", [0.0, 0.0, 0.0], unit),
            boxed("kkkkkkkk", [5.0, 0.0, 0.0], unit),
            op("rrrrrrrr", "cut", "jjjjjjjj", "kkkkkkkk"),
        ]),
    );
    for empty in ["cccccccc", "ffffffff"] {
        let outcome = model.outcome(&id(empty)).unwrap();
        let FeatureOutcome::Failed(error) = outcome else {
            panic!("{empty}: {outcome:?}");
        };
        assert_eq!(error.feature(), Some(empty), "{error:?}");
        assert!(model.element_map(&id(empty)).is_none());
    }
    assert!(matches!(
        model.outcome(&id("hhhhhhhh")),
        Some(FeatureOutcome::Blocked { .. })
    ));
    check_maps(&model, "after empty results");
    assert!(model.element_map(&id("rrrrrrrr")).is_some());
}
