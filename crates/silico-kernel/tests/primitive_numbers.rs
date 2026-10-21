//! Trevin's fullest-form numbering (2026-10-17, docs/ports/naming.md): a primitive's
//! elements are named by their numbers as in its fullest form, so whatever its values,
//! one number names one element. Checked on the real kernel by where each element is: a
//! cone's Face2 is always its top cap (flat, at its height), Face3 its base cap (flat, at
//! 0), Edge1 the top circle, Edge3 the base circle, Edge4 and Edge5 its apex at the top or
//! the base, each vertex numbered as the end it's at; and an edit that takes an element away breaks a reference to it, never giving
//! its number, and so its name, to another.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use silico_core::ErrorCode;
use silico_core::document::{Checked, FeatureId, read};
use silico_core::kernel::{ElementKind, KernelWorker, Primitive, primitive_numbers};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

fn worker() -> KernelWorker {
    KernelWorker::start_worker(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"))).unwrap()
}

/// Elements by the number each is named by, with their points.
type Placed = Vec<(u32, Vec<[f64; 3]>)>;

/// Each of `primitive`'s faces' and edges' points, by the number it's named by.
fn placed(kernel: &mut KernelWorker, primitive: Primitive) -> (Placed, Placed) {
    let shape = kernel.make(primitive).unwrap();
    let mesh = kernel.tessellate(&shape, 0.05).unwrap();
    let mut faces = Vec::new();
    let mut triangles = mesh.triangles.iter();
    for (count, number) in mesh
        .face_triangles
        .iter()
        .zip(primitive_numbers(&primitive, ElementKind::Face))
    {
        let points = triangles
            .by_ref()
            .take(*count as usize)
            .flat_map(|t| t.iter().map(|&i| mesh.positions[i as usize]))
            .collect();
        faces.push((number, points));
    }
    let mut edges = Vec::new();
    let mut points = mesh.edge_points.iter();
    for (count, number) in mesh
        .edge_point_counts
        .iter()
        .zip(primitive_numbers(&primitive, ElementKind::Edge))
    {
        edges.push((
            number,
            points.by_ref().take(*count as usize).copied().collect(),
        ));
    }
    (faces, edges)
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * (1.0 + b.abs())
}

fn radius(p: &[f64; 3]) -> f64 {
    (p[0] * p[0] + p[1] * p[1]).sqrt()
}

/// Whether the cone's element `number` of `kind` is where its number says, given its
/// points (none, for a degenerate edge OCCT draws no polyline for).
fn where_its_number_says(
    kind: ElementKind,
    number: u32,
    points: &[[f64; 3]],
    (r1, r2, h): (f64, f64, f64),
) -> bool {
    let at = |z: f64| points.iter().all(|p| near(p[2], z));
    match (kind, number) {
        // The side spans the height.
        (ElementKind::Face, 1) | (ElementKind::Edge, 2) => {
            points.iter().any(|p| near(p[2], 0.0)) && points.iter().any(|p| near(p[2], h))
        }
        (ElementKind::Face, 2) => at(h) && !points.is_empty(),
        (ElementKind::Face, 3) => at(0.0) && !points.is_empty(),
        // The circles, drawn (a degenerate edge isn't).
        (ElementKind::Edge, 1) => {
            !points.is_empty() && at(h) && points.iter().all(|p| near(radius(p), r2))
        }
        (ElementKind::Edge, 3) => {
            !points.is_empty() && at(0.0) && points.iter().all(|p| near(radius(p), r1))
        }
        // An apex: degenerate (OCCT draws no polyline for it), so where it is is told by
        // which end of the cone is a point (found by the review: with no points, a check of
        // where they are holds for any edge).
        (ElementKind::Edge, 4) => points.is_empty() && r2 == 0.0,
        (ElementKind::Edge, 5) => points.is_empty() && r1 == 0.0,
        _ => false,
    }
}

#[test]
fn a_cones_element_numbers_name_the_same_elements_whatever_its_radii() {
    // Either radius 0 (a point at the top or the base), or both above it, at many sizes:
    // each face and edge is where its number says.
    let kernel = std::cell::RefCell::new(worker());
    let radius = prop_oneof![Just(0.0), 0.1f64..20.0];
    let cones = (radius.clone(), radius, 0.1f64..20.0)
        .prop_filter("a cone has a radius", |(r1, r2, _)| *r1 > 0.0 || *r2 > 0.0);
    let mut runner = TestRunner::new(Config {
        cases: 300,
        failure_persistence: None,
        ..Config::default()
    });
    runner
        .run(&cones, |(r1, r2, h)| {
            let cone = Primitive::Cone {
                radius1: r1,
                radius2: r2,
                height: h,
            };
            let (faces, edges) = placed(&mut kernel.borrow_mut(), cone);
            for (kind, elements) in [(ElementKind::Face, &faces), (ElementKind::Edge, &edges)] {
                for (number, points) in elements {
                    prop_assert!(
                        where_its_number_says(kind, *number, points, (r1, r2, h)),
                        "{cone:?}: {kind:?}{number} is elsewhere: {:?}",
                        points.iter().take(4).collect::<Vec<_>>()
                    );
                }
            }
            // Each number once, of each kind.
            for numbered in [&faces, &edges] {
                let mut numbers: Vec<u32> = numbered.iter().map(|(n, _)| *n).collect();
                numbers.sort_unstable();
                numbers.dedup();
                prop_assert_eq!(numbers.len(), numbered.len());
            }
            // Each vertex is numbered as the end it's at: Vertex1 the top circle's (Edge1),
            // Vertex2 the base circle's (Edge3), Vertex3 the top apex (Edge4), Vertex4 the
            // base apex (Edge5), by the edges each ends, which a move reports as OCCT's.
            let mut kernel = kernel.borrow_mut();
            let shape = kernel.make(cone).unwrap();
            let moved = kernel.translate(&shape, [1.0, 0.0, 0.0]).unwrap();
            let edge_numbers = primitive_numbers(&cone, ElementKind::Edge);
            let vertex_numbers = primitive_numbers(&cone, ElementKind::Vertex);
            for (edge, ends) in edge_numbers
                .iter()
                .zip(&moved.history.topology.edge_vertices)
            {
                let at: Vec<u32> = ends
                    .iter()
                    .map(|&v| vertex_numbers[v as usize - 1])
                    .collect();
                let ok = match edge {
                    1 => at.iter().all(|&v| v == 1),
                    3 => at.iter().all(|&v| v == 2),
                    4 => at.iter().all(|&v| v == 3),
                    5 => at.iter().all(|&v| v == 4),
                    // The seam, from the base's end to the top's.
                    2 => {
                        let base = if r1 > 0.0 { 2 } else { 4 };
                        let top = if r2 > 0.0 { 1 } else { 3 };
                        let mut ends = at.clone();
                        ends.sort_unstable();
                        ends == [base.min(top), base.max(top)]
                    }
                    _ => false,
                };
                prop_assert!(ok, "{:?}: Edge{} ends at {:?}", cone, edge, at);
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn every_other_primitive_is_numbered_as_occt_numbers_it_whatever_its_values() {
    // A box, cylinder, sphere, and torus (a horn torus too: the tube touching the axis)
    // have the same elements at every size, so OCCT's numbers are theirs, as FreeCAD's are.
    let mut kernel = worker();
    for primitive in [
        Primitive::Box {
            dx: 1.0,
            dy: 30.0,
            dz: 0.2,
        },
        Primitive::Cylinder {
            radius: 0.1,
            height: 40.0,
        },
        Primitive::Sphere { radius: 7.0 },
        Primitive::Torus {
            major_radius: 10.0,
            minor_radius: 1.0,
        },
        Primitive::Torus {
            major_radius: 2.0,
            minor_radius: 2.0,
        },
    ] {
        let shape = kernel.make(primitive).unwrap();
        let summary = kernel.summarize(&shape).unwrap();
        for (kind, count) in [
            (ElementKind::Face, summary.faces),
            (ElementKind::Edge, summary.edges),
            (ElementKind::Vertex, summary.vertices),
        ] {
            let own: Vec<u32> = (1..=count).collect();
            assert_eq!(
                primitive_numbers(&primitive, kind),
                own,
                "{primitive:?} {kind:?}"
            );
        }
    }
}

fn publish(engine: &Recompute<KernelPool>, text: &str) -> Arc<Model> {
    match engine
        .start(Checked::new(read(text.as_bytes()).unwrap()).unwrap())
        .wait()
        .unwrap()
    {
        RunOutcome::Published(model) => model,
        other => panic!("{other:?}"),
    }
}

/// A cone (radii `r1`, `r2`, height 10) with a pin (r 1) cut down its axis, and a fillet
/// of radius 0.2 of `edge` of the cut.
fn drilled_cone(r1: &str, r2: &str, edge: &str) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [
{{"id": "kkkkkkkk", "type": "cone", "v": 1, "name": "Cone", "position": [0, 0, 0], "radius1": {r1}, "radius2": {r2}, "height": 10}},
{{"id": "pppppppp", "type": "cylinder", "v": 1, "name": "Pin", "position": [0, 0, -1], "radius": 1, "height": 12}},
{{"id": "hhhhhhhh", "type": "cut", "v": 1, "name": "Hole", "base": "kkkkkkkk", "tool": "pppppppp"}},
{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "hhhhhhhh", "edges": [{edge:?}], "radius": 0.2}}
]}}"#
    )
}

#[test]
fn a_cone_brought_to_a_point_breaks_a_reference_to_the_rim_it_lost_and_keeps_the_other() {
    // The hole's rims: at the top cap (Face2), and at the base cap (Face3). Bring the top to
    // a point (r2 → 0) and the cone has no top cap: the top rim's fillet breaks, offering
    // the base rim, never taking it; the base rim's still builds. The same at the base.
    let engine = Recompute::new(
        KernelPool::start_on_demand(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2)
            .unwrap(),
    );
    let top = "hhhhhhhh.cut.Edge(G kkkkkkkk.cone.Face2, G pppppppp.cylinder.Face1)";
    let base = "hhhhhhhh.cut.Edge(G kkkkkkkk.cone.Face3, G pppppppp.cylinder.Face1)";
    for edge in [top, base] {
        let model = publish(&engine, &drilled_cone("5", "3", edge));
        assert!(model.errors().next().is_none(), "{edge}");
    }
    for (r1, r2, lost, kept) in [("5", "0", top, base), ("0", "5", base, top)] {
        let model = publish(&engine, &drilled_cone(r1, r2, lost));
        let Some(FeatureOutcome::Broken { errors, .. }) =
            model.outcome(&FeatureId::new("ffffffff").unwrap())
        else {
            panic!(
                "r1 {r1}, r2 {r2}: {:?}",
                model.outcome(&FeatureId::new("ffffffff").unwrap())
            );
        };
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
        assert!(
            errors[0].candidates().iter().any(|c| c == kept),
            "r1 {r1}, r2 {r2}: {:?}",
            errors[0].candidates()
        );
        let model = publish(&engine, &drilled_cone(r1, r2, kept));
        assert!(
            model.errors().next().is_none(),
            "r1 {r1}, r2 {r2}: {:?}",
            model.errors().collect::<Vec<_>>()
        );
    }
}
