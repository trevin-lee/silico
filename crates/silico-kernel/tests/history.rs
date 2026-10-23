//! M1.4: what an operation does to its inputs' faces, edges, and vertices, checked against
//! FreeCAD's element names, against the shape's geometry, over random fillets, and as the
//! app receives it through a worker (docs/ports/history.md).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::imports;
use super::oracle;

use std::path::Path;
use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::kernel::{KernelWorker, Primitive};
use silico_core::worker::Worker;
use silico_kernel::{ElementKind, ElementRef, History};
use silico_protocol::{DebugOp, Generation, Misbehavior, Op};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Relation {
    Kept,
    Modified,
    Generated,
}

/// One fact of a history: an input element (`Face3`) relates to a result element.
type Fact = (Relation, String, String);

fn name(r: ElementRef) -> String {
    format!("{:?}{}", r.kind, r.index)
}

/// Silico's history (of its first input) as facts.
fn facts(history: &History) -> Vec<Fact> {
    let mut out = Vec::new();
    let input = &history.inputs[0];
    let kinds = [
        (ElementKind::Face, &input.faces),
        (ElementKind::Edge, &input.edges),
        (ElementKind::Vertex, &input.vertices),
    ];
    for (kind, fates) in kinds {
        for (i, fate) in fates.iter().enumerate() {
            let from = format!("{kind:?}{}", i + 1);
            if let Some(kept) = fate.kept {
                out.push((Relation::Kept, from.clone(), format!("{kind:?}{kept}")));
            }
            for r in &fate.modified {
                out.push((Relation::Modified, from.clone(), name(r.element)));
            }
            for r in &fate.generated {
                out.push((Relation::Generated, from.clone(), name(r.element)));
            }
        }
    }
    out.sort();
    out
}

/// The facts FreeCAD's element names state directly, for a fillet of `edges` at `radius`
/// on a 10 × 20 × 30 box. A result element named `Face3;:M;FLT;...` was modified from
/// input `Face3`, `Edge1;:G;...` generated from input `Edge1`, and `Face2;:H...` (no
/// operation) is input `Face2` kept. Names FreeCAD derives from neighbours (`;:U`, `;:L`)
/// aren't history, and are left out.
fn freecad_fillet_facts(edges: &[u32], radius: f64) -> Vec<Fact> {
    let edge_list: Vec<String> = edges
        .iter()
        .map(|e| format!("({e}, {radius:?}, {radius:?})"))
        .collect();
    let script = format!(
        r#"
import json, FreeCAD
doc = FreeCAD.newDocument()
box = doc.addObject("Part::Box", "Box"); box.Length = 10.0; box.Width = 20.0; box.Height = 30.0
f = doc.addObject("Part::Fillet", "Fillet"); f.Base = box; f.Edges = [{}]
doc.recompute()
out = []
for result, name in f.Shape.ElementReverseMap.items():
    if ":U" in name or ":L" in name:
        continue
    parts = name.split(";")
    if parts[1] == ":M":
        out.append(["modified", parts[0], result])
    elif parts[1] == ":G":
        out.append(["generated", parts[0], result])
    elif parts[1].startswith(":H"):
        out.append(["kept", parts[0], result])
print("SILICO_ORACLE " + json.dumps(out))
"#,
        edge_list.join(", ")
    );
    let mut out: Vec<Fact> = oracle::freecad(&script)
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let s = |i: usize| r[i].as_str().unwrap().to_owned();
            let relation = match r[0].as_str().unwrap() {
                "kept" => Relation::Kept,
                "modified" => Relation::Modified,
                "generated" => Relation::Generated,
                other => panic!("unknown relation {other}"),
            };
            (relation, s(1), s(2))
        })
        .collect();
    out.sort();
    out
}

fn box_10_20_30() -> silico_kernel::Shape {
    silico_kernel::make_box(10.0, 20.0, 30.0).unwrap()
}

#[test]
fn fillet_history_matches_freecads_element_names() {
    // Every fact FreeCAD's names state, Silico's history has; and exactly the same elements
    // are kept. (Silico may record more: FreeCAD's names state one source per element.)
    let cases: [(&[u32], f64); 3] = [
        (&[1], 2.0),
        (&[1, 2], 1.5),
        (&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12], 1.0),
    ];
    for (edges, radius) in cases {
        let theirs = freecad_fillet_facts(edges, radius);
        let ours = facts(
            &silico_kernel::fillet(&box_10_20_30(), edges, radius)
                .unwrap()
                .history,
        );
        assert!(
            theirs.len() > 3,
            "{edges:?}: FreeCAD stated only {theirs:?}"
        );
        for fact in &theirs {
            assert!(
                ours.contains(fact),
                "{edges:?} r = {radius}: FreeCAD has {fact:?}; Silico has {ours:?}"
            );
        }
        let kept = |fs: &[Fact]| -> Vec<Fact> {
            fs.iter()
                .filter(|f| f.0 == Relation::Kept)
                .cloned()
                .collect()
        };
        assert_eq!(kept(&ours), kept(&theirs), "{edges:?} r = {radius}: kept");
    }
}

#[test]
fn filleting_one_edge_keeps_modifies_and_generates_what_the_geometry_says() {
    // The box's Face1 is x = 0, Face2 x = 10, Face3 y = 0, Face4 y = 20, Face5 z = 0, Face6
    // z = 30; Edge1 runs from (0, 0, 30) to (0, 0, 0), between Face1 and Face3. Rounding it
    // trims the two faces it joins and the two end caps whose outlines change, leaves the
    // other two faces as they were, and makes one new face: nothing else is generated.
    // The edges that meet Edge1's ends (2, 4, 9, 10) are shortened; the other seven are
    // untouched; Edge1's two end vertices go, and the other six stay.
    let h = silico_kernel::fillet(&box_10_20_30(), &[1], 2.0)
        .unwrap()
        .history;
    assert_eq!(h.validate(1), Ok(()));
    let input = &h.inputs[0];
    assert_eq!(
        (input.faces.len(), input.edges.len(), input.vertices.len()),
        (6, 12, 8)
    );
    assert_eq!(
        (
            h.result.faces,
            h.result.edges,
            h.result.vertices,
            h.result.solids
        ),
        (7, 15, 10, 1)
    );

    let kept: Vec<bool> = input.faces.iter().map(|f| f.kept.is_some()).collect();
    assert_eq!(kept, [false, true, false, true, false, false], "faces kept");
    for (i, face) in input.faces.iter().enumerate() {
        if face.kept.is_none() {
            assert_eq!(
                face.modified.len(),
                1,
                "Face{} modified into one face",
                i + 1
            );
            assert_eq!(face.modified[0].element.kind, ElementKind::Face);
        }
    }
    let edge1 = &input.edges[0];
    assert!(edge1.is_deleted(), "{edge1:?}");
    assert_eq!(edge1.generated.len(), 1, "{edge1:?}");
    let new_face = edge1.generated[0].element;
    assert_eq!(new_face.kind, ElementKind::Face);
    // The generated face is the one new face: no input face became it.
    assert!(input.faces.iter().all(
        |f| f.kept != Some(new_face.index) && f.modified.iter().all(|m| m.element != new_face)
    ));
    // And Edge1 is the only thing that generated anything.
    let generators: Vec<Fact> = facts(&h)
        .into_iter()
        .filter(|f| f.0 == Relation::Generated)
        .collect();
    assert_eq!(generators.len(), 1, "{generators:?}");
    assert_eq!(generators[0].1, "Edge1");

    for (i, edge) in input.edges.iter().enumerate().skip(1) {
        let n = i + 1;
        if [2, 4, 9, 10].contains(&n) {
            assert!(
                edge.kept.is_none() && edge.modified.len() == 1,
                "Edge{n}: {edge:?}"
            );
        } else {
            assert!(
                edge.kept.is_some() && edge.modified.is_empty(),
                "Edge{n}: {edge:?}"
            );
        }
    }
    assert_eq!(
        input.vertices.iter().filter(|v| v.kept.is_some()).count(),
        6
    );
    assert_eq!(input.vertices.iter().filter(|v| v.is_deleted()).count(), 2);
}

#[test]
fn a_fillets_history_is_the_same_every_time() {
    let fillet = || {
        silico_kernel::fillet(&box_10_20_30(), &[1, 2, 5], 1.5)
            .unwrap()
            .history
    };
    assert_eq!(fillet(), fillet());
}

#[test]
fn history_numbers_survive_the_trip_through_bytes() {
    // The worker sends the result as B-rep, and everything after numbers its elements from
    // those bytes: the history's numbers must be the same elements there. Filleting the
    // read-back result gives the same history as filleting the one in memory.
    let round_trip = |s: &silico_kernel::Shape| {
        silico_kernel::from_brep(&silico_kernel::to_brep(s).unwrap()).unwrap()
    };
    let built = silico_kernel::fillet(&box_10_20_30(), &[1, 2], 1.5).unwrap();
    let from_bytes = silico_kernel::fillet(&round_trip(&box_10_20_30()), &[1, 2], 1.5).unwrap();
    assert_eq!(built.history, from_bytes.history);
    let again = |s: &silico_kernel::Shape| silico_kernel::fillet(s, &[3], 0.5).unwrap().history;
    assert_eq!(again(&built.shape), again(&round_trip(&built.shape)));
}

#[test]
fn the_history_the_app_receives_is_the_kernels() {
    // Through a real worker and the core's typed layer, as the app gets it: the same history
    // as the kernel's, for one input, its counts those of the shape that came back.
    let mut k = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let shape = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let built = k.fillet(&shape, &[1], 2.0).unwrap();
    let here = silico_kernel::fillet(&box_10_20_30(), &[1], 2.0).unwrap();
    assert_eq!(built.history, here.history);
    assert_eq!(built.history.inputs.len(), 1);
    let s = k.summarize(&built.shape).unwrap();
    assert_eq!(
        (
            built.history.result.faces,
            built.history.result.edges,
            built.history.result.vertices
        ),
        (s.faces, s.edges, s.vertices)
    );
}

#[test]
fn a_history_that_names_what_the_result_doesnt_have_is_refused() {
    // A worker whose history names an element the result doesn't have can't be trusted:
    // a protocol error, with a suggestion, and the worker is stopped.
    let mut w = Worker::spawn(Path::new(TEST_WORKER)).unwrap();
    w.call(
        Generation(0),
        Op::Debug(DebugOp::Misbehave(Misbehavior::BadHistory)),
        Duration::from_secs(30),
    )
    .unwrap();
    let mut k = KernelWorker::from_worker(w);
    let shape = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let err = k.fillet(&shape, &[1], 2.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    assert!(err.cause().unwrap().contains("doesn't have"), "{err}");
    assert!(err.suggestion().is_some(), "{err}");
    let after = k.summarize(&shape).unwrap_err();
    assert_eq!(after.code(), ErrorCode::WorkerCrashed, "{after}");
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig { cases: 48, ..proptest::prelude::ProptestConfig::default() })]

    /// Every element a fillet's history names is in the result, every input element has a
    /// fate, and every face of the result comes from somewhere.
    #[test]
    fn history_indices_valid(
        edges in proptest::sample::subsequence((1u32..=12).collect::<Vec<_>>(), 1..=12),
        radius in 0.1f64..4.0,
    ) {
        let Ok(built) = silico_kernel::fillet(&box_10_20_30(), &edges, radius) else {
            // Refused (too large where edges meet): nothing to check.
            return Ok(());
        };
        let h = &built.history;
        proptest::prop_assert_eq!(h.validate(1), Ok(()));
        let s = silico_kernel::summarize(&built.shape).unwrap();
        proptest::prop_assert_eq!((h.result.faces, h.result.edges, h.result.vertices), (s.faces, s.edges, s.vertices));
        let input = &h.inputs[0];
        proptest::prop_assert_eq!((input.faces.len(), input.edges.len(), input.vertices.len()), (6, 12, 8));
        for face in 1..=h.result.faces {
            let face = ElementRef { kind: ElementKind::Face, index: face };
            let from_face = input.faces.iter().any(|f| f.kept == Some(face.index) || f.modified.iter().any(|m| m.element == face));
            let generated = input.edges.iter().chain(&input.vertices).any(|f| f.generated.iter().any(|g| g.element == face));
            proptest::prop_assert!(from_face || generated, "{:?} comes from nothing", face);
        }
        // Each filleted edge is gone and made a face.
        for &e in &edges {
            let edge = &input.edges[e as usize - 1];
            proptest::prop_assert!(edge.is_deleted() && !edge.generated.is_empty(), "Edge{}: {:?}", e, edge);
        }
    }
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}

#[test]
fn a_box_fillets_reported_elements_are_each_first_in_occts_list() {
    // A description of box fillets, not a check of the bridge's counting: each trimmed
    // face, shortened edge, and filleted edge becomes or makes exactly one element, so
    // OCCT's list for it has one entry, at position 1. (A bridge that always wrote 1 would
    // pass this; booleans, whose lists are longer, pin the counting in boolean.rs.)
    let h = silico_kernel::fillet(&box_10_20_30(), &[1, 2, 5], 1.5)
        .unwrap()
        .history;
    let input = &h.inputs[0];
    let mut seen = 0;
    for fate in input
        .faces
        .iter()
        .chain(&input.edges)
        .chain(&input.vertices)
    {
        for r in fate.modified.iter().chain(&fate.generated) {
            assert_eq!(r.position, 1, "{fate:?}");
            seen += 1;
        }
    }
    assert!(seen > 5, "only {seen} reported");
}

#[test]
fn a_history_for_the_wrong_number_of_inputs_is_refused() {
    // A fillet has one input; a worker that sends a history for none can't be trusted.
    // The test worker's fillets are good until it's told otherwise.
    let mut good = KernelWorker::start_worker(Path::new(TEST_WORKER)).unwrap();
    let b = good
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    assert_eq!(good.fillet(&b, &[1], 2.0).unwrap().history.inputs.len(), 1);
    let mut w = Worker::spawn(Path::new(TEST_WORKER)).unwrap();
    w.call(
        Generation(0),
        Op::Debug(DebugOp::Misbehave(Misbehavior::HistoryForNoInputs)),
        Duration::from_secs(30),
    )
    .unwrap();
    let mut k = KernelWorker::from_worker(w);
    let shape = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let err = k.fillet(&shape, &[1], 2.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    assert!(err.cause().unwrap().contains("input shapes"), "{err}");
    let after = k.summarize(&shape).unwrap_err();
    assert_eq!(after.code(), ErrorCode::WorkerCrashed, "{after}");
}
