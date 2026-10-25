//! M4.4: a model's solid described by a kernel worker and named by its element map
//! (`silico_core::elements`, docs/design/m4.4-describe.md): the design's example, a 40 ×
//! 20 × 10 block less a pin of radius 2.5 through it at (20, 10).

use super::example;

use std::collections::BTreeSet;
use std::f64::consts::PI;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use silico_core::document::{Checked, FeatureId, read};
use silico_core::elements::{Element, Geometry, elements};
use silico_core::kernel::{ElementKind, KernelWorker, Surface};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

fn model() -> Arc<Model> {
    let pool = KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap();
    let engine = Recompute::new(pool);
    let checked = Checked::new(read(example::EXAMPLE.as_bytes()).unwrap()).unwrap();
    match engine.start(checked).wait_timeout(Duration::from_secs(60)) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the rebuild didn't finish within a minute"),
    }
}

/// The hole's elements: the cut's solid as its rebuild described it, named by its map.
fn the_holes_elements() -> Vec<Element> {
    let model = model();
    let hole = FeatureId::new("m3tq8v0z").unwrap();
    let description = model.description(&hole).unwrap().clone();
    elements(model.element_map(&hole).unwrap(), description).unwrap()
}

#[test]
fn each_features_solid_is_described_by_its_rebuild_as_a_worker_describes_it() {
    // The rebuild describes each feature's solid before publishing (a batch at a time, on
    // its pool); a worker of its own asked for the same shape afterwards gives the same
    // description, value for value.
    let model = model();
    let mut worker = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    for feature in ["k7f2qx3m", "a91cw0de", "m3tq8v0z"] {
        let id = FeatureId::new(feature).unwrap();
        let Some(FeatureOutcome::Built { built, .. }) = model.outcome(&id) else {
            panic!("{feature} didn't build")
        };
        let described = model.description(&id).unwrap();
        assert_eq!(
            described,
            &worker.describe(&built.shape).unwrap(),
            "{feature}"
        );
        let summary = built.summary().unwrap();
        assert_eq!(
            [
                described.faces.len(),
                described.edges.len(),
                described.vertices.len()
            ],
            [summary.faces, summary.edges, summary.vertices].map(|n| n as usize),
            "{feature}"
        );
    }
    // The block: six planes, 200, 400, and 800 mm² twice each, by hand.
    let block = model
        .description(&FeatureId::new("k7f2qx3m").unwrap())
        .unwrap();
    let mut areas: Vec<f64> = block.faces.iter().map(|f| f.area).collect();
    areas.sort_by(f64::total_cmp);
    for (got, want) in areas.iter().zip([200.0, 200.0, 400.0, 400.0, 800.0, 800.0]) {
        assert!((got - want).abs() <= 1e-9 * want, "{areas:?}");
    }
    assert!(
        block
            .faces
            .iter()
            .all(|f| matches!(f.surface, Surface::Plane { .. }))
    );
}

fn named<'a>(listed: &'a [Element], name: &str) -> &'a Element {
    listed
        .iter()
        .find(|e| e.name.to_string() == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn every_element_of_a_cut_is_listed_once_by_its_maps_name_in_order() {
    let model = model();
    let hole = FeatureId::new("m3tq8v0z").unwrap();
    let map = model.element_map(&hole).unwrap();
    let listed = the_holes_elements();
    // Every element the map names, by its own name, once.
    let own: BTreeSet<String> = map.iter().map(|(_, names)| names[0].to_string()).collect();
    let got: BTreeSet<String> = listed.iter().map(|e| e.name.to_string()).collect();
    assert_eq!(got, own);
    assert_eq!(listed.len(), own.len());
    // Faces, then edges, then vertices; within each, by name byte by byte.
    let kinds: Vec<ElementKind> = listed.iter().map(|e| e.kind()).collect();
    let mut sorted = kinds.clone();
    sorted.sort_by_key(|k| match k {
        ElementKind::Face => 0,
        ElementKind::Edge => 1,
        _ => 2,
    });
    assert_eq!(kinds, sorted);
    for pair in listed.windows(2) {
        if pair[0].kind() == pair[1].kind() {
            assert!(
                pair[0].name.to_string().as_bytes() < pair[1].name.to_string().as_bytes(),
                "{} before {}",
                pair[0].name,
                pair[1].name
            );
        }
    }
}

#[test]
fn the_cuts_top_and_its_hole_measure_as_their_shapes_do() {
    let listed = the_holes_elements();
    // The block's top, z = 10, less the hole: 40 × 20 − π · 2.5² = 800 − 6.25π, facing
    // +z, its centre of area the hole's centre, (20, 10, 10), by symmetry.
    let top = named(&listed, "k7f2qx3m.box.Face6");
    let Geometry::Face(face) = &top.geometry else {
        panic!("{top:?}")
    };
    assert_eq!(
        face.surface,
        Surface::Plane {
            normal: [0.0, 0.0, 1.0]
        }
    );
    assert!((face.area - (800.0 - 6.25 * PI)).abs() < 1e-9, "{face:?}");
    for (got, want) in face.centroid.iter().zip([20.0, 10.0, 10.0]) {
        assert!((got - want).abs() < 1e-9, "{face:?}");
    }
    // The hole: the pin's side, a cylinder of radius 2.5 about x = 20, y = 10, through the
    // block's 10: area 2π · 2.5 · 10 = 50π, its centre of area (20, 10, 5).
    let wall = named(&listed, "m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)");
    let Geometry::Face(face) = &wall.geometry else {
        panic!("{wall:?}")
    };
    let Surface::Cylinder { axis, radius } = &face.surface else {
        panic!("{face:?}")
    };
    assert_eq!(*radius, 2.5);
    assert_eq!(&axis.origin[..2], &[20.0, 10.0]);
    assert_eq!(axis.direction.map(f64::abs), [0.0, 0.0, 1.0]);
    assert!((face.area - 50.0 * PI).abs() < 1e-9, "{face:?}");
    for (got, want) in face.centroid.iter().zip([20.0, 10.0, 5.0]) {
        assert!((got - want).abs() < 1e-9, "{face:?}");
    }
}

#[test]
fn a_solid_is_described_the_same_bytes_by_two_workers() {
    // The example's cut, described by two workers started apart: the same description,
    // every value to the bit (its JSON, each number written exactly, compared), as the
    // determinism sweep requires.
    let model = model();
    let hole = FeatureId::new("m3tq8v0z").unwrap();
    let Some(FeatureOutcome::Built { built, .. }) = model.outcome(&hole) else {
        panic!("the cut didn't build")
    };
    let describe = || {
        let mut worker = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
        serde_json::to_string(&worker.describe(&built.shape).unwrap()).unwrap()
    };
    assert_eq!(describe(), describe());
}

#[test]
fn the_holes_rims_are_circles_about_its_axis_where_the_block_meets_the_pin() {
    // Each rim named by the block's face and the pin's side that make it: the bottom's
    // (Face5, z = 0) and the top's (Face6, z = 10), circles of radius 2.5 about
    // (20, 10, z), 5π long.
    let listed = the_holes_elements();
    for (face, z) in [("Face5", 0.0), ("Face6", 10.0)] {
        let name = format!("m3tq8v0z.cut.Edge(G k7f2qx3m.box.{face}, G a91cw0de.cylinder.Face1)");
        let rim = named(&listed, &name);
        let Geometry::Edge(edge) = &rim.geometry else {
            panic!("{rim:?}")
        };
        let silico_core::kernel::EdgeCurve::Circle { axis, radius } = &edge.curve else {
            panic!("{name}: {edge:?}")
        };
        assert_eq!(*radius, 2.5, "{name}");
        assert_eq!(axis.origin, [20.0, 10.0, z], "{name}");
        assert_eq!(axis.direction.map(f64::abs), [0.0, 0.0, 1.0], "{name}");
        assert!((edge.length - 5.0 * PI).abs() < 1e-9, "{name}: {edge:?}");
    }
}
