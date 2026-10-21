//! M1.5: booleans and translation (docs/ports/booleans.md), checked against hand
//! calculations, FreeCAD's own tests, FreeCAD's element names, and through a worker.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/close.rs"]
mod close;
#[path = "common/imports.rs"]
mod imports;
#[path = "common/numbers.rs"]
mod numbers;
#[path = "common/oracle.rs"]
mod oracle;

use std::path::Path;
use std::time::Duration;

use close::assert_close;
use silico_core::ErrorCode;
use silico_core::kernel::KernelWorker;
use silico_core::worker::Worker;
use silico_kernel::{
    BooleanOp, ElementKind, History, KernelErrorKind, Primitive, Shape, ShapeSummary, boolean,
    make, measure_as_freecad, summarize, translate,
};
use silico_protocol::{DebugOp, Generation, Misbehavior, Op};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");

const OPS: [BooleanOp; 3] = [BooleanOp::Fuse, BooleanOp::Cut, BooleanOp::Common];

fn cube(size: f64) -> Primitive {
    Primitive::Box {
        dx: size,
        dy: size,
        dz: size,
    }
}

/// `primitive` moved by `offset`.
fn at(primitive: Primitive, offset: [f64; 3]) -> Shape {
    translate(&make(&primitive).unwrap(), offset).unwrap().shape
}

fn apply(op: BooleanOp, base: &Shape, tool: &Shape) -> ShapeSummary {
    summarize(&boolean(op, base, tool).unwrap().shape).unwrap()
}

#[test]
fn a_translated_shape_has_every_element_moved_and_numbered_the_same() {
    let b = make(&Primitive::Box {
        dx: 10.0,
        dy: 20.0,
        dz: 30.0,
    })
    .unwrap();
    let moved = translate(&b, [1.0, -2.0, 3.5]).unwrap();
    let s = summarize(&moved.shape).unwrap();
    assert_close("volume", s.volume, 6000.0, 1e-12);
    assert_close("area", s.area, 2200.0, 1e-12);
    // Integrated adaptively, so to the last bit or two, not exactly.
    for (got, want) in s.center.into_iter().zip([6.0, 8.0, 18.5]) {
        assert_close("center", got, want, 1e-12);
    }
    assert_eq!((s.faces, s.edges, s.vertices), (6, 12, 8));
    assert!(s.valid);

    let h = &moved.history;
    assert_eq!(h.validate(1), Ok(()));
    let input = &h.inputs[0];
    let kinds = [
        (ElementKind::Face, &input.faces, 6),
        (ElementKind::Edge, &input.edges, 12),
        (ElementKind::Vertex, &input.vertices, 8),
    ];
    for (kind, fates, count) in kinds {
        assert_eq!(fates.len(), count, "{kind:?}");
        for (i, fate) in fates.iter().enumerate() {
            // A moved element isn't the same sub-shape (its location differs), so it's
            // modified, into the element with its own number.
            assert_eq!(fate.kept, None, "{kind:?}{}", i + 1);
            assert!(fate.generated.is_empty(), "{kind:?}{}", i + 1);
            assert_eq!(fate.modified.len(), 1, "{kind:?}{}", i + 1);
            assert_eq!(fate.modified[0].element.kind, kind);
            assert_eq!(fate.modified[0].element.index as usize, i + 1);
            assert_eq!(fate.modified[0].position, 1);
        }
    }
}

#[test]
fn a_translation_by_nothing_keeps_every_element() {
    let b = make(&cube(10.0)).unwrap();
    let h = translate(&b, [0.0, 0.0, 0.0]).unwrap().history;
    let input = &h.inputs[0];
    for fates in [&input.faces, &input.edges, &input.vertices] {
        for (n, fate) in fates.iter().enumerate() {
            assert_eq!(fate.kept, Some(n as u32 + 1), "{fate:?}");
            assert!(
                fate.modified.is_empty() && fate.generated.is_empty(),
                "{fate:?}"
            );
        }
    }
}

#[test]
fn an_offset_that_isnt_finite_or_is_too_far_is_refused() {
    let b = make(&cube(10.0)).unwrap();
    for (offset, axis) in [
        ([f64::NAN, 0.0, 0.0], "x"),
        ([0.0, f64::INFINITY, 0.0], "y"),
        ([0.0, 0.0, f64::NEG_INFINITY], "z"),
        ([0.0, -1.000_001e6, 0.0], "y"),
        ([0.0, 0.0, 2e6], "z"),
    ] {
        let err = translate(&b, offset).err().unwrap();
        assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{offset:?}");
        assert!(
            err.message().contains(&format!("offset's {axis} ")),
            "{offset:?}: {err}"
        );
    }
    // A kilometre is the largest length a primitive may have, and the farthest a move goes.
    let far = translate(&b, [-1e6, 1e6, 0.0]).unwrap();
    assert_eq!(
        summarize(&far.shape).unwrap().center,
        [-1e6 + 5.0, 1e6 + 5.0, 5.0]
    );
}

#[test]
fn overlapping_cubes_match_a_hand_calculation() {
    // Two 10 mm cubes, the second at (5, 5, 5): they share a 5 mm cube.
    // Fuse: 2 × 1000 − 125. Each cube loses three 5 × 5 squares of its surface inside the
    // other, so the area is 2 × (600 − 75). Each cube keeps its six faces (the three that
    // meet the other become L-shapes), so 12 faces; 7 corners of each, plus the 6 points
    // where an edge of one passes through a face of the other, are 20 vertices; Euler
    // (V − E + F = 2) gives 30 edges.
    // Cut: 1000 − 125; the base's surface loses three squares and gains the three inside
    // it, so 600; 9 faces, 14 vertices (7 + the tool's corner (5, 5, 5) + 6), 21 edges.
    // Common: the 5 mm cube, centered at (7.5, 7.5, 7.5).
    let a = make(&cube(10.0)).unwrap();
    let b = at(cube(10.0), [5.0, 5.0, 5.0]);
    let cut_center = (1000.0 * 5.0 - 125.0 * 7.5) / 875.0;
    let cases = [
        (BooleanOp::Fuse, 1875.0, 1050.0, 7.5, (12, 30, 20)),
        (BooleanOp::Cut, 875.0, 600.0, cut_center, (9, 21, 14)),
        (BooleanOp::Common, 125.0, 150.0, 7.5, (6, 12, 8)),
    ];
    for (op, volume, area, center, counts) in cases {
        let s = apply(op, &a, &b);
        assert_close(&format!("{op:?} volume"), s.volume, volume, 1e-12);
        assert_close(&format!("{op:?} area"), s.area, area, 1e-12);
        for c in s.center {
            assert_close(&format!("{op:?} center"), c, center, 1e-12);
        }
        assert_eq!((s.faces, s.edges, s.vertices), counts, "{op:?}");
        assert!(s.valid, "{op:?}");
    }
}

#[test]
fn freecads_part_boolean_tests() {
    // tests/src/Mod/Part/App/FeaturePart{Fuse,Cut,Common}.cpp: 1 × 2 × 3 boxes, the first
    // at the origin, the second overlapping it, apart from it, touching it, 1e-7 (OCCT's
    // tolerance) from touching, and 1e-4 into it. FreeCAD expects an empty common (volume
    // 0) where they don't overlap; Silico refuses it (docs/ports/booleans.md).
    let block = Primitive::Box {
        dx: 1.0,
        dy: 2.0,
        dz: 3.0,
    };
    let first = make(&block).unwrap();
    let barely = 3.0 * 1e-4;
    let cases = [
        (1.0, 9.0, 3.0, Some(3.0)),
        (3.0, 12.0, 6.0, None),
        (2.0, 12.0, 6.0, None),
        (2.0 + 1e-7, 12.0, 6.0, None),
        (2.0 - 1e-4, 12.0 - barely, 6.0 - barely, Some(barely)),
    ];
    for (y, fuse, cut, common) in cases {
        let second = at(block, [0.0, y, 0.0]);
        // FreeCAD compares these to 1e-7 relative (EXPECT_FLOAT_EQ) where 1e-4 is involved.
        assert_close(
            &format!("fuse at {y}"),
            apply(BooleanOp::Fuse, &first, &second).volume,
            fuse,
            1e-7,
        );
        assert_close(
            &format!("cut at {y}"),
            apply(BooleanOp::Cut, &first, &second).volume,
            cut,
            1e-7,
        );
        match common {
            Some(volume) => assert_close(
                &format!("common at {y}"),
                apply(BooleanOp::Common, &first, &second).volume,
                volume,
                1e-7,
            ),
            None => {
                let err = boolean(BooleanOp::Common, &first, &second).err().unwrap();
                assert_eq!(err.kind(), KernelErrorKind::Failed, "{y}: {err}");
                assert_eq!(err.message(), "the common's result is empty");
                assert!(
                    err.suggestion().unwrap().contains("OCCT found no overlap"),
                    "{err}"
                );
            }
        }
    }
    // FeaturePartCut.cpp testMapping: the cut of the overlapping pair has 26 elements.
    let cut = boolean(BooleanOp::Cut, &first, &at(block, [0.0, 1.0, 0.0])).unwrap();
    let s = summarize(&cut.shape).unwrap();
    assert_eq!(s.faces + s.edges + s.vertices, 26);
}

#[test]
fn freecads_common_history_test() {
    // tests/src/Mod/Part/App/FeaturePartCommon.cpp testHistory, whose assertions were
    // dropped in ecf7e51ab3 (recovered from its parent): each input face's result faces,
    // numbered from 0.
    let block = Primitive::Box {
        dx: 1.0,
        dy: 2.0,
        dz: 3.0,
    };
    let common = boolean(
        BooleanOp::Common,
        &make(&block).unwrap(),
        &at(block, [0.0, 1.0, 0.0]),
    )
    .unwrap();
    let h = &common.history;
    assert_eq!(h.validate(2), Ok(()));
    let base: [&[u32]; 6] = [&[0], &[5], &[], &[2], &[3], &[1]];
    let tool: [&[u32]; 6] = [&[0], &[5], &[4], &[], &[3], &[1]];
    for (input, want) in [base, tool].iter().enumerate() {
        for (face, want) in want.iter().enumerate() {
            let fate = &h.inputs[input].faces[face];
            let got: Vec<u32> = fate
                .kept
                .into_iter()
                .chain(fate.modified.iter().map(|r| r.element.index))
                .map(|n| n - 1)
                .collect();
            assert_eq!(&got, want, "input {input}, face {face}: {fate:?}");
        }
    }
}

#[test]
fn freecads_partdesign_boolean_and_topo_shape_tests() {
    // src/Mod/PartDesign/PartDesignTests/TestBoolean.py: 10 mm cubes, one 5 mm along x.
    let a = make(&cube(10.0)).unwrap();
    let b = at(cube(10.0), [-5.0, 0.0, 0.0]);
    for (op, volume) in OPS.into_iter().zip([1500.0, 500.0, 500.0]) {
        assert_close(&format!("{op:?}"), apply(op, &a, &b).volume, volume, 1e-12);
    }
    // tests/src/Mod/Part/App/TopoShapeExpansion.cpp makeElementFuse, makeElementCut,
    // makeElementBooleanCommon: unit cubes, one at (0.5, 0.5, 0).
    let a = make(&cube(1.0)).unwrap();
    let b = at(cube(1.0), [0.5, 0.5, 0.0]);
    for (op, volume) in OPS.into_iter().zip([1.75, 0.75, 0.25]) {
        assert_close(&format!("{op:?}"), apply(op, &a, &b).volume, volume, 1e-12);
    }
}

#[test]
fn subtracting_a_primitive_from_a_larger_one_sharing_its_origin() {
    // src/Mod/PartDesign/PartDesignTests/TestPrimitive.py: faces that coincide, an apex
    // shared, a solid hollowed out.
    use std::f64::consts::PI;
    let cases = [
        (cube(11.0), cube(10.0), 1331.0 - 1000.0),
        (
            Primitive::Cylinder {
                radius: 11.0,
                height: 10.0,
            },
            Primitive::Cylinder {
                radius: 10.0,
                height: 10.0,
            },
            PI * 10.0 * (121.0 - 100.0),
        ),
        (
            Primitive::Sphere { radius: 6.0 },
            Primitive::Sphere { radius: 5.0 },
            4.0 / 3.0 * PI * (216.0 - 125.0),
        ),
        (
            Primitive::Cone {
                radius1: 0.0,
                radius2: 4.0,
                height: 10.0,
            },
            Primitive::Cone {
                radius1: 0.0,
                radius2: 3.0,
                height: 10.0,
            },
            PI * 10.0 * (16.0 - 9.0) / 3.0,
        ),
        (
            Primitive::Torus {
                major_radius: 10.0,
                minor_radius: 4.0,
            },
            Primitive::Torus {
                major_radius: 10.0,
                minor_radius: 3.0,
            },
            2.0 * PI * PI * 10.0 * (16.0 - 9.0),
        ),
    ];
    for (outer, inner, volume) in cases {
        let s = apply(
            BooleanOp::Cut,
            &make(&outer).unwrap(),
            &make(&inner).unwrap(),
        );
        assert_close(outer.name(), s.volume, volume, 1e-9);
        assert!(s.valid, "{outer:?}");
    }
}

#[test]
fn a_cylinder_cut_through_a_box_leaves_a_hole_with_the_history_the_geometry_says() {
    // A 10 mm cube less a cylinder of radius 2 through its middle: 1000 − 40π. The cube's
    // Face5 (z = 0) and Face6 (z = 10) are pierced: each is modified, and makes the hole's
    // circle edge and that circle's one vertex; its four side faces are untouched. The
    // cylinder's side face (Face1) becomes the hole's wall and makes the two circles; its
    // caps (Face2, Face3), the circles they had, and its vertices are gone.
    let bx = make(&cube(10.0)).unwrap();
    let cylinder = at(
        Primitive::Cylinder {
            radius: 2.0,
            height: 20.0,
        },
        [5.0, 5.0, -5.0],
    );
    let cut = boolean(BooleanOp::Cut, &bx, &cylinder).unwrap();
    let s = summarize(&cut.shape).unwrap();
    assert_close(
        "volume",
        s.volume,
        1000.0 - 40.0 * std::f64::consts::PI,
        1e-9,
    );
    // Six faces and the wall; 12 edges, 2 circles, and the wall's seam; 8 corners and
    // the 2 circles' vertices.
    assert_eq!((s.faces, s.edges, s.vertices), (7, 15, 10));
    let h = &cut.history;
    assert_eq!(h.validate(2), Ok(()));

    let cube_faces = &h.inputs[0].faces;
    for (i, face) in cube_faces[..4].iter().enumerate() {
        assert!(face.kept.is_some(), "Face{}: {face:?}", i + 1);
    }
    let mut circles = Vec::new();
    for face in &cube_faces[4..6] {
        assert_eq!(face.kept, None, "{face:?}");
        assert_eq!(face.modified.len(), 1, "{face:?}");
        let kinds: Vec<_> = face.generated.iter().map(|r| r.element.kind).collect();
        assert_eq!(kinds, [ElementKind::Vertex, ElementKind::Edge], "{face:?}");
        // OCCT lists two elements for each, so one of them is second: the bridge counts.
        let mut positions: Vec<u32> = face.generated.iter().map(|r| r.position).collect();
        positions.sort();
        assert_eq!(positions, [1, 2], "{face:?}");
        circles.push(face.generated[1].element);
    }

    let tool = &h.inputs[1];
    let wall = &tool.faces[0];
    assert_eq!(wall.kept, None);
    assert_eq!(wall.modified.len(), 1, "{wall:?}");
    let mut made: Vec<_> = wall.generated.iter().map(|r| r.element).collect();
    made.sort();
    circles.sort();
    assert_eq!(made, circles, "the wall makes the same two circles");
    for cap in &tool.faces[1..] {
        assert!(cap.is_deleted() && cap.generated.is_empty(), "{cap:?}");
    }
    // The cylinder's edges: its seam is cut down to the hole's; its circles go.
    let seam_kept_or_modified = tool.edges.iter().filter(|e| !e.is_deleted()).count();
    assert_eq!(seam_kept_or_modified, 1, "{:?}", tool.edges);
    assert!(
        tool.vertices.iter().all(|v| v.is_deleted()),
        "{:?}",
        tool.vertices
    );
}

#[test]
fn an_empty_result_is_refused_with_what_to_do() {
    let small = make(&cube(1.0)).unwrap();
    let big = at(cube(3.0), [-1.0, -1.0, -1.0]);
    let err = boolean(BooleanOp::Cut, &small, &big).err().unwrap();
    assert_eq!(err.kind(), KernelErrorKind::Failed);
    assert_eq!(err.message(), "the cut's result is empty");
    assert!(
        err.suggestion()
            .unwrap()
            .contains("removes all of the base"),
        "{err}"
    );

    let apart = at(cube(1.0), [5.0, 0.0, 0.0]);
    let err = boolean(BooleanOp::Common, &small, &apart).err().unwrap();
    assert_eq!(err.message(), "the common's result is empty");
    // A fuse of shapes apart is both of them.
    let s = apply(BooleanOp::Fuse, &small, &apart);
    assert_close("fuse apart", s.volume, 2.0, 1e-12);
    assert_eq!(s.faces, 12);
}

#[test]
fn an_empty_input_is_refused_naming_which() {
    // OCCT would fuse an empty shape with another, or cut nothing from one, as if nothing
    // were wrong: an empty input means an earlier operation went wrong.
    let empty = silico_kernel::test_ops::empty_shape().unwrap();
    let b = make(&cube(1.0)).unwrap();
    for op in OPS {
        for (base, tool, role) in [
            (&empty, &b, "base"),
            (&b, &empty, "tool"),
            (&empty, &empty, "base"),
        ] {
            let err = boolean(op, base, tool).err().unwrap();
            assert_eq!(
                err.kind(),
                KernelErrorKind::InvalidInput,
                "{op:?} {role}: {err}"
            );
            let name = op.name().to_lowercase();
            assert_eq!(err.message(), format!("the {name}'s {role} is empty"));
            assert!(err.suggestion().unwrap().contains("made nothing"), "{err}");
        }
    }
}

#[test]
fn after_a_refused_boolean_the_worker_goes_on() {
    // Each refusal (an empty input, an empty result, an offset that isn't a number) is
    // followed by an operation that works, on the same worker.
    let mut k = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let a = box_data(&mut k, 10.0);
    let empty = silico_core::kernel::ShapeData::unchecked(silico_protocol::Blob::inline(
        silico_kernel::to_brep(&silico_kernel::test_ops::empty_shape().unwrap()).unwrap(),
    ));
    let apart = k.translate(&a, [20.0, 0.0, 0.0]).unwrap().shape;
    type Refusal<'a> = &'a dyn Fn(&mut KernelWorker) -> silico_core::Error;
    let refusals: [(Refusal, ErrorCode); 4] = [
        (
            &|k| k.boolean(BooleanOp::Cut, &a, &empty).unwrap_err(),
            ErrorCode::InvalidInput,
        ),
        (
            &|k| k.boolean(BooleanOp::Fuse, &empty, &a).unwrap_err(),
            ErrorCode::InvalidInput,
        ),
        (
            &|k| k.boolean(BooleanOp::Common, &a, &apart).unwrap_err(),
            ErrorCode::KernelFailed,
        ),
        (
            &|k| k.translate(&a, [0.0, f64::NAN, 0.0]).unwrap_err(),
            ErrorCode::InvalidInput,
        ),
    ];
    for (refuse, code) in refusals {
        let err = refuse(&mut k);
        assert_eq!(err.code(), code, "{err}");
        assert!(err.suggestion().is_some(), "{err}");
        let fused = k.boolean(BooleanOp::Fuse, &a, &apart).unwrap();
        assert_close(
            "after",
            k.summarize(&fused.shape).unwrap().volume,
            2000.0,
            1e-12,
        );
    }
}

#[test]
fn a_problem_an_input_already_had_isnt_blamed_on_the_boolean() {
    // The open box (10 × 20 × 30, Face1 at x = 0 missing) is invalid. Cutting a notch
    // from its far side, nowhere near the missing face, changes only valid faces, so it's
    // accepted (FreeCAD warns about such inputs but goes on, 3f5a2ae6b8), and the result
    // is as invalid as the input was.
    let open = silico_kernel::test_ops::open_box().unwrap();
    let notch = at(cube(4.0), [8.0, 8.0, 13.0]);
    for (base, tool) in [(&open, &notch), (&notch, &open)] {
        let op = if std::ptr::eq(base, &open) {
            BooleanOp::Cut
        } else {
            BooleanOp::Common
        };
        let built = boolean(op, base, tool).unwrap_or_else(|e| panic!("{op:?} refused: {e}"));
        let s = summarize(&built.shape).unwrap();
        assert!(s.faces > 0);
        if op == BooleanOp::Cut {
            assert!(!s.valid, "the open box's hole is still there");
        }
    }
}

#[test]
fn the_same_boolean_twice_gives_the_same_bytes_and_history() {
    let a = make(&Primitive::Sphere { radius: 6.0 }).unwrap();
    let b = at(
        Primitive::Cylinder {
            radius: 3.0,
            height: 20.0,
        },
        [1.0, 0.5, -10.0],
    );
    for op in OPS {
        let first = boolean(op, &a, &b).unwrap();
        let second = boolean(op, &a, &b).unwrap();
        assert_eq!(
            silico_kernel::to_brep(&first.shape).unwrap(),
            silico_kernel::to_brep(&second.shape).unwrap(),
            "{op:?}"
        );
        assert_eq!(first.history, second.history, "{op:?}");
    }
}

/// One fact of a history: input `0` or `1`'s element (`Face3`) is kept as, modified into,
/// or generates a result element, at a position in OCCT's list (0 when kept).
type Fact = (usize, String, String, String, u32);

/// Silico's history as facts.
fn facts(history: &History) -> Vec<Fact> {
    let mut out = Vec::new();
    for (n, input) in history.inputs.iter().enumerate() {
        let kinds = [
            (ElementKind::Face, &input.faces),
            (ElementKind::Edge, &input.edges),
            (ElementKind::Vertex, &input.vertices),
        ];
        for (kind, fates) in kinds {
            for (i, fate) in fates.iter().enumerate() {
                let from = format!("{kind:?}{}", i + 1);
                if let Some(kept) = fate.kept {
                    out.push((n, "kept".into(), from.clone(), format!("{kind:?}{kept}"), 0));
                }
                let lists = [("modified", &fate.modified), ("generated", &fate.generated)];
                for (relation, list) in lists {
                    for r in list {
                        let to = format!("{:?}{}", r.element.kind, r.element.index);
                        out.push((n, relation.into(), from.clone(), to, r.position));
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// FreeCAD's description of a primitive as a Part feature, as the oracle script builds it.
fn freecad_solid(primitive: Primitive) -> String {
    match primitive {
        Primitive::Box { dx, dy, dz } => {
            format!(r#"("Box", {{"Length": {dx:?}, "Width": {dy:?}, "Height": {dz:?}}})"#)
        }
        Primitive::Cylinder { radius, height } => {
            format!(r#"("Cylinder", {{"Radius": {radius:?}, "Height": {height:?}}})"#)
        }
        Primitive::Sphere { radius } => format!(r#"("Sphere", {{"Radius": {radius:?}}})"#),
        Primitive::Cone {
            radius1,
            radius2,
            height,
        } => format!(
            r#"("Cone", {{"Radius1": {radius1:?}, "Radius2": {radius2:?}, "Height": {height:?}}})"#
        ),
        Primitive::Torus {
            major_radius,
            minor_radius,
        } => format!(r#"("Torus", {{"Radius1": {major_radius:?}, "Radius2": {minor_radius:?}}})"#),
    }
}

/// What FreeCAD makes of a boolean of two placed primitives, with Python's booleans (OCCT's
/// own fuzzy value, as Silico's): volume, area, counts, validity, and the history its
/// element names state. A result element named `Face6;:M;CUT;:H<tag>...` was modified from
/// `Face6` of the input with that tag; `Face6;:G2(Face3;K-3;:H<tag2>...);CUT;:H<tag>...`
/// was generated second in the list of the first input's `Face6`, and third (`K-3`, minus
/// for generated) in the list of the second input's `Face3`; without `K`, an extra source
/// was modified at position 1. `Face2;:H<tag>,F` is kept. Names FreeCAD derives from
/// neighbours (`;:U`, `;:L`) aren't history, and are left out. A name the script can't
/// read is returned, so it fails the test rather than be skipped.
fn freecad_boolean(
    op: BooleanOp,
    base: (Primitive, [f64; 3]),
    tool: (Primitive, [f64; 3]),
) -> serde_json::Value {
    let method = match op {
        BooleanOp::Fuse => "fuse",
        BooleanOp::Cut => "cut",
        BooleanOp::Common => "common",
    };
    let script = format!(
        r#"
import json, re, FreeCAD
doc = FreeCAD.newDocument()
def solid(spec, at):
    kind, props = spec
    o = doc.addObject("Part::" + kind, kind)
    for k, v in props.items():
        setattr(o, k, v)
    o.Placement.Base = FreeCAD.Vector(*at)
    return o
a = solid({base}, {base_at:?})
b = solid({tool}, {tool_at:?})
doc.recompute()
r = a.Shape.{method}(b.Shape)
tags = {{format(a.Shape.Tag, "x"): 0, format(b.Shape.Tag, "x"): 1}}
EL = r"([A-Za-z]+[0-9]+)"
kept = re.compile(EL + r";:H([0-9a-f]+),[FEV]")
made = re.compile(EL + r";:([MG])([0-9]*)(?:\((.*)\))?;[A-Z]+;:H([0-9a-f]+):[0-9a-f]+,[FEV]")
other = re.compile(EL + r"(?:;K(-?[0-9]+))?;:H([0-9a-f]+)(?::[0-9a-f]+)?,[FEV]")
facts, unread = [], []
for result, name in r.ElementReverseMap.items():
    if ";:U" in name or ";:L" in name:
        continue
    m = kept.fullmatch(name)
    if m:
        facts.append([tags[m[2]], "kept", m[1], result, 0])
        continue
    m = made.fullmatch(name)
    if not m:
        unread.append(name)
        continue
    facts.append([tags[m[5]], "modified" if m[2] == "M" else "generated", m[1], result, int(m[3] or 1)])
    for o in (m[4].split("|") if m[4] else []):
        n = other.fullmatch(o)
        if not n:
            unread.append(name)
            continue
        k = int(n[2]) if n[2] else 1
        facts.append([tags[n[3]], "modified" if k > 0 else "generated", n[1], result, abs(k)])
out = {{"volume": r.Volume, "area": r.Area, "faces": len(r.Faces), "edges": len(r.Edges),
        "vertices": len(r.Vertexes), "valid": r.isValid(), "facts": facts, "unread": unread}}
print("SILICO_ORACLE " + json.dumps(out))
"#,
        base = freecad_solid(base.0),
        base_at = base.1,
        tool = freecad_solid(tool.0),
        tool_at = tool.1,
    );
    oracle::freecad(&script)
}

#[test]
fn a_boolean_measured_as_freecad_measures_matches_freecad_where_the_precise_measure_doesnt() {
    // A cone fused with a cylinder (the oracle's generated case 7 #782). OCCT's default
    // integration, which FreeCAD measures with, misjudges the faces the intersection trims:
    // 3.6e-3 off the precise volume, and 0.012 mm off its center. Measured as FreeCAD
    // measures it, Silico's fuse is FreeCAD's to the oracle's tolerance for booleans of
    // curved shapes; its precise measures aren't.
    let cone = Primitive::Cone {
        radius1: 8.917,
        radius2: 0.0,
        height: 12.397,
    };
    let cylinder = Primitive::Cylinder {
        radius: 4.203,
        height: 21.611,
    };
    let offset = [-0.474, 4.27, -0.853];
    let fused = boolean(
        BooleanOp::Fuse,
        &make(&cone).unwrap(),
        &at(cylinder, offset),
    )
    .unwrap()
    .shape;
    let ours = measure_as_freecad(&fused).unwrap();
    let precise = summarize(&fused).unwrap();
    let script = format!(
        r#"
import json, FreeCAD
doc = FreeCAD.newDocument()
def solid(spec, at):
    kind, props = spec
    o = doc.addObject("Part::" + kind, kind)
    for k, v in props.items():
        setattr(o, k, v)
    o.Placement.Base = FreeCAD.Vector(*at)
    return o
a = solid({cone}, [0.0, 0.0, 0.0])
b = solid({cylinder}, {offset:?})
doc.recompute()
r = a.Shape.fuse(b.Shape)
box = r.optimalBoundingBox(False, False)
out = {{"volume": r.Volume, "area": r.Area, "solids": len(r.Solids),
        "center": list(r.Solids[0].CenterOfMass),
        "bounds": [[box.XMin, box.YMin, box.ZMin], [box.XMax, box.YMax, box.ZMax]]}}
print("SILICO_ORACLE " + json.dumps(out))
"#,
        cone = freecad_solid(cone),
        cylinder = freecad_solid(cylinder),
    );
    let theirs = oracle::freecad(&script);
    let num = |v: &serde_json::Value| v.as_f64().unwrap();
    let point = |v: &serde_json::Value| -> [f64; 3] {
        std::array::from_fn(|k| num(&v.as_array().unwrap()[k]))
    };
    let tolerance = silico_oracle::Tolerance::CURVED_BOOLEAN;
    assert_eq!(theirs["solids"], 1);
    let volume = num(&theirs["volume"]);
    assert_close("volume", ours.volume, volume, tolerance.relative);
    assert_close("area", ours.area, num(&theirs["area"]), tolerance.relative);
    assert!(numbers::relative_error(precise.volume, volume) > 1e-3);
    let our_box = ours.bounds.unwrap();
    let their_box = [point(&theirs["bounds"][0]), point(&theirs["bounds"][1])];
    let their_center = point(&theirs["center"]);
    let size = (0..3)
        .map(|k| our_box[1][k] - our_box[0][k])
        .fold(0.0, f64::max);
    for k in 0..3 {
        let off = (ours.center[k] - their_center[k]).abs();
        assert!(off <= tolerance.center * size, "{ours:?} {theirs}");
        for corner in 0..2 {
            let off = (our_box[corner][k] - their_box[corner][k]).abs();
            assert!(off <= tolerance.bounds * size, "{ours:?} {theirs}");
        }
    }
    let off = (0..3).map(|k| (precise.center[k] - their_center[k]).abs());
    assert!(off.fold(0.0, f64::max) > 1e-3, "{precise:?} {theirs}");
}

#[test]
fn booleans_match_freecad_with_the_history_its_names_state() {
    let cylinder = |radius, height| Primitive::Cylinder { radius, height };
    let block = Primitive::Box {
        dx: 1.0,
        dy: 2.0,
        dz: 3.0,
    };
    let cases = [
        (
            OPS.to_vec(),
            (cube(10.0), [0.0; 3]),
            (cube(10.0), [5.0, 5.0, 5.0]),
        ),
        (OPS.to_vec(), (block, [0.0; 3]), (block, [0.0, 1.0, 0.0])),
        (
            vec![BooleanOp::Cut],
            (cube(10.0), [0.0; 3]),
            (cylinder(2.0, 20.0), [5.0, 5.0, -5.0]),
        ),
        (
            OPS.to_vec(),
            (Primitive::Sphere { radius: 6.0 }, [0.0; 3]),
            (cylinder(3.0, 20.0), [1.0, 0.5, -10.0]),
        ),
        (
            vec![BooleanOp::Fuse, BooleanOp::Cut],
            (cylinder(5.0, 10.0), [0.0; 3]),
            (
                Primitive::Box {
                    dx: 4.0,
                    dy: 4.0,
                    dz: 20.0,
                },
                [3.0, -2.0, -5.0],
            ),
        ),
        (
            vec![BooleanOp::Common],
            (
                Primitive::Torus {
                    major_radius: 10.0,
                    minor_radius: 3.0,
                },
                [0.0; 3],
            ),
            (
                Primitive::Box {
                    dx: 30.0,
                    dy: 30.0,
                    dz: 2.0,
                },
                [-15.0, -15.0, -1.0],
            ),
        ),
        (
            vec![BooleanOp::Fuse],
            (
                Primitive::Cone {
                    radius1: 5.0,
                    radius2: 2.0,
                    height: 10.0,
                },
                [0.0; 3],
            ),
            (Primitive::Sphere { radius: 4.0 }, [0.0, 0.0, 10.0]),
        ),
    ];
    let mut longer_lists = 0;
    for (ops, base, tool) in cases {
        let a = at(base.0, base.1);
        let b = at(tool.0, tool.1);
        for op in ops {
            let case = format!("{op:?} of {base:?} and {tool:?}");
            let theirs = freecad_boolean(op, base, tool);
            assert_eq!(theirs["unread"], serde_json::json!([]), "{case}");
            let built = boolean(op, &a, &b).unwrap();
            let s = summarize(&built.shape).unwrap();
            // FreeCAD measures with OCCT's fixed-order integration, Silico adaptively to
            // 1e-9; on these shapes they differ by about 2e-8.
            assert_close(&case, s.volume, theirs["volume"].as_f64().unwrap(), 1e-7);
            assert_close(&case, s.area, theirs["area"].as_f64().unwrap(), 1e-7);
            let count = |key: &str| theirs[key].as_u64().unwrap() as u32;
            assert_eq!(
                (s.faces, s.edges, s.vertices),
                (count("faces"), count("edges"), count("vertices")),
                "{case}"
            );
            assert_eq!(s.valid, theirs["valid"].as_bool().unwrap(), "{case}");

            let ours = facts(&built.history);
            let mut stated: Vec<Fact> = theirs["facts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| {
                    let s = |i: usize| f[i].as_str().unwrap().to_owned();
                    (
                        f[0].as_u64().unwrap() as usize,
                        s(1),
                        s(2),
                        s(3),
                        f[4].as_u64().unwrap() as u32,
                    )
                })
                .collect();
            stated.sort();
            assert!(stated.len() >= 6, "{case}: FreeCAD stated only {stated:?}");
            for fact in &stated {
                assert!(
                    ours.contains(fact),
                    "{case}: FreeCAD has {fact:?}; Silico has {ours:?}"
                );
                if fact.4 > 1 {
                    longer_lists += 1;
                }
            }
            let kept = |fs: &[Fact]| -> Vec<Fact> {
                fs.iter().filter(|f| f.1 == "kept").cloned().collect()
            };
            assert_eq!(kept(&ours), kept(&stated), "{case}: kept");
        }
    }
    // The cases include elements OCCT lists second or later, so positions are compared.
    assert!(
        longer_lists >= 4,
        "only {longer_lists} facts past position 1"
    );
}

fn box_data(k: &mut KernelWorker, size: f64) -> silico_core::kernel::ShapeData {
    k.make(cube(size)).unwrap()
}

#[test]
fn booleans_and_moves_through_the_worker_match_in_process() {
    let mut k = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let a = box_data(&mut k, 10.0);
    let moved = k.translate(&a, [5.0, 5.0, 5.0]).unwrap();
    assert_eq!(moved.history.inputs.len(), 1);
    assert_eq!(moved.shape.valid(), Some(true));
    for (op, volume) in OPS.into_iter().zip([1875.0, 875.0, 125.0]) {
        let built = k.boolean(op, &a, &moved.shape).unwrap();
        assert_eq!(built.history.inputs.len(), 2, "{op:?}");
        assert_eq!(built.shape.valid(), Some(true), "{op:?}");
        let s = k.summarize(&built.shape).unwrap();
        assert_close(&format!("{op:?}"), s.volume, volume, 1e-12);
        // The same bytes and history as in this process, from the same inputs.
        let read = |data: &silico_core::kernel::ShapeData| {
            silico_kernel::from_brep(data.brep().as_bytes()).unwrap()
        };
        let here = boolean(op, &read(&a), &read(&moved.shape)).unwrap();
        assert_eq!(
            built.shape.brep().as_bytes(),
            silico_kernel::to_brep(&here.shape).unwrap().as_slice(),
            "{op:?}"
        );
        assert_eq!(built.history, here.history, "{op:?}");
    }
    let apart = k.translate(&a, [20.0, 0.0, 0.0]).unwrap().shape;
    let err = k.boolean(BooleanOp::Common, &a, &apart).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert!(
        err.suggestion().unwrap().contains("OCCT found no overlap"),
        "{err}"
    );
    let err = k.translate(&a, [f64::NAN, 0.0, 0.0]).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
}

#[test]
fn a_boolean_history_for_the_wrong_number_of_inputs_is_refused() {
    // A boolean has two inputs; a worker that sends a history for none can't be trusted.
    let mut w = Worker::spawn(Path::new(TEST_WORKER)).unwrap();
    w.call(
        Generation(0),
        Op::Debug(DebugOp::Misbehave(Misbehavior::HistoryForNoInputs)),
        Duration::from_secs(30),
    )
    .unwrap();
    let mut k = KernelWorker::from_worker(w);
    let a = box_data(&mut k, 10.0);
    let err = k.boolean(BooleanOp::Fuse, &a, &a).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    assert!(err.cause().unwrap().contains("input shapes"), "{err}");
    let after = k.summarize(&a).unwrap_err();
    assert_eq!(after.code(), ErrorCode::WorkerCrashed, "{after}");
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}

#[test]
fn a_result_with_no_volume_is_refused_as_empty() {
    // OCCT cuts a torus by a copy of itself moved 0.5 across its seam into a "solid" of
    // volume -8e-11 and two whole torus surfaces (the cut is really 7.919), does much the
    // same to the smaller torus, and makes the common of the smaller torus with a copy
    // moved 0.3 across it a solid of 4e-13 with its center 5e12 away. None of them is
    // anything: each is refused as empty.
    // And the same at a hundredth and a hundred times the size, where the quick measure's
    // error on such shapes is larger or smaller; but for the first cut at a hundred times,
    // which isn't nothing (`a_result_turned_inside_out_is_refused_as_invalid`).
    let mut cases = Vec::new();
    for scale in [0.01, 1.0, 100.0] {
        for (major, minor, op, y) in [
            (2.0, 1.0, BooleanOp::Cut, 0.5),
            (2.0, 1.0, BooleanOp::Cut, 0.3),
            (1.5, 0.5, BooleanOp::Cut, 0.5),
            (1.5, 0.5, BooleanOp::Common, 0.3),
        ] {
            if scale == 100.0 && (major, op, y) == (2.0, BooleanOp::Cut, 0.5) {
                continue;
            }
            cases.push((major * scale, minor * scale, op, [0.0, y * scale, 0.0]));
        }
    }
    for (major, minor, op, offset) in cases {
        let t = Primitive::Torus {
            major_radius: major,
            minor_radius: minor,
        };
        let case = format!("{t:?} moved {offset:?}: {op:?}");
        match boolean(op, &make(&t).unwrap(), &at(t, offset)) {
            Err(e) => {
                assert_eq!(e.kind(), KernelErrorKind::Failed, "{case}: {e}");
                assert!(e.message().ends_with("result is empty"), "{case}: {e}");
            }
            // Should OCCT get it right one day, it has a volume (at least a tenth of a
            // millionth of the torus's, which these overlaps are far above).
            Ok(built) => {
                let v = summarize(&built.shape).unwrap().volume;
                let torus =
                    2.0 * std::f64::consts::PI * std::f64::consts::PI * major * minor * minor;
                assert!(v > 1e-7 * torus, "{case}: accepted with volume {v}");
            }
        }
    }
}

#[test]
fn a_broken_result_of_a_kilometre_torus_is_measured_precisely_not_cleared_by_the_quick_measure() {
    // Found by the milestone's mutation run: the quick measure clears a result against the
    // cube of its bounding diagonal (1e-5 d³), and a threshold that grew only as d² went
    // unnoticed. A torus of radii 1e6 and 2.5e5, the largest the kernel takes, cut by a copy
    // moved 175,000 along y (the family the port note lists: OCCT gets these wrong): OCCT's
    // result reads 3.0e13 by the quick measure, under 1e-5 d³ (6.3e14, d 4.0e6) but over d²
    // (1.6e13), and 32 precisely, with an area of 2e13: no volume to speak of. Measured
    // precisely, it's refused as empty, never accepted. Should OCCT get it right one day,
    // it has a volume, at least a tenth of a millionth of the torus's.
    let t = Primitive::Torus {
        major_radius: 1e6,
        minor_radius: 2.5e5,
    };
    match boolean(
        BooleanOp::Cut,
        &make(&t).unwrap(),
        &at(t, [0.0, 175_000.0, 0.0]),
    ) {
        Err(e) => {
            assert_eq!(e.kind(), KernelErrorKind::Failed, "{e}");
            assert!(e.message().ends_with("result is empty"), "{e}");
        }
        Ok(built) => {
            let v = summarize(&built.shape).unwrap().volume;
            let torus = 2.0 * std::f64::consts::PI * std::f64::consts::PI * 1e6 * 2.5e5 * 2.5e5;
            assert!(v > 1e-7 * torus, "accepted with volume {v}");
        }
    }
}

#[test]
fn a_result_turned_inside_out_is_refused_as_invalid() {
    // A torus of radii 200 and 100 cut by a copy of itself moved 50 across its seam: OCCT's
    // cut is a solid turned inside out, its volume -3.15e6 for an area of 7.9e5, which
    // OCCT's own check passes. Not empty (a volume within OCCT's tolerance of none, either
    // side, is), and not a solid: refused as an invalid result.
    let t = Primitive::Torus {
        major_radius: 200.0,
        minor_radius: 100.0,
    };
    match boolean(BooleanOp::Cut, &make(&t).unwrap(), &at(t, [0.0, 50.0, 0.0])) {
        Err(e) => {
            assert_eq!(e.kind(), KernelErrorKind::InvalidResult, "{e}");
            assert!(e.suggestion().is_some(), "{e}");
        }
        // Should OCCT get it right one day, it has a volume, as the scaled cases above.
        Ok(built) => {
            let v = summarize(&built.shape).unwrap().volume;
            let torus = 2.0 * std::f64::consts::PI * std::f64::consts::PI * 200.0 * 100.0 * 100.0;
            assert!(v > 1e-7 * torus, "accepted with volume {v}");
        }
    }
}

#[test]
fn a_result_that_fills_little_of_its_box_is_kept() {
    // Valid results with little volume for their size: pieces far apart, a long bar cut
    // down to its end tabs, a thin wire. Their mean thickness is far above OCCT's
    // tolerance, so none is taken for a result with no volume.
    let cylinder = |radius, height| Primitive::Cylinder { radius, height };
    let slab = |dx, dy, dz| Primitive::Box { dx, dy, dz };
    let cases = [
        (
            "unit cubes 2 m apart",
            BooleanOp::Fuse,
            make(&cube(1.0)).unwrap(),
            at(cube(1.0), [2000.0, 0.0, 0.0]),
            2.0,
        ),
        (
            "0.05 mm cubes 100 mm apart",
            BooleanOp::Fuse,
            make(&cube(0.05)).unwrap(),
            at(cube(0.05), [100.0, 0.0, 0.0]),
            2.5e-4,
        ),
        (
            "a 1 m bar's two 0.4 mm end tabs",
            BooleanOp::Cut,
            make(&slab(1000.0, 1.0, 1.0)).unwrap(),
            at(slab(999.2, 3.0, 3.0), [0.4, -1.0, -1.0]),
            0.8,
        ),
        (
            "a 1 m wire of radius 0.01",
            BooleanOp::Common,
            make(&cylinder(0.01, 1000.0)).unwrap(),
            at(slab(2.0, 2.0, 1002.0), [-1.0, -1.0, -1.0]),
            std::f64::consts::PI * 0.1,
        ),
    ];
    for (case, op, base, tool, volume) in cases {
        let built = boolean(op, &base, &tool)
            .unwrap_or_else(|e| panic!("{case}: refused: {e} ({:?})", e.suggestion()));
        assert_close(case, summarize(&built.shape).unwrap().volume, volume, 1e-9);
    }
}

#[test]
fn an_empty_result_doesnt_claim_more_than_occt_found() {
    // OCCT returns nothing for the common of two tori that overlap (moved 1 along y,
    // across the seam: docs/ports/booleans.md, Known wrong results). The refusal says
    // what OCCT found, and what to do if the shapes do overlap.
    let t = Primitive::Torus {
        major_radius: 2.0,
        minor_radius: 1.0,
    };
    let e = boolean(
        BooleanOp::Common,
        &make(&t).unwrap(),
        &at(t, [0.0, 1.0, 0.0]),
    )
    .err()
    .unwrap();
    assert_eq!(e.kind(), KernelErrorKind::Failed, "{e}");
    assert_eq!(e.message(), "the common's result is empty");
    let suggestion = e.suggestion().unwrap();
    assert!(
        suggestion.starts_with("OCCT found no overlap"),
        "{suggestion}"
    );
    assert!(
        suggestion.contains("if they already do, move one slightly"),
        "{suggestion}"
    );
}

/// The volume two tori share, both about z through the origin's xy plane, the second
/// moved `d` along x: twice the overlap of their heights over the xy plane, integrated
/// by the midpoint rule on a grid of `n` × `n` cells (by symmetry, over y ≥ 0).
fn tori_common(a: (f64, f64), b: (f64, f64), d: f64, n: usize) -> f64 {
    let height = |(major, minor): (f64, f64), x: f64, y: f64| {
        let off = (x * x + y * y).sqrt() - major;
        if off.abs() < minor {
            (minor * minor - off * off).sqrt()
        } else {
            0.0
        }
    };
    let reach = (a.0 + a.1).max(b.0 + b.1 + d.abs());
    let (x0, x1, y1) = (-reach, reach, reach);
    let (dx, dy) = ((x1 - x0) / n as f64, y1 / n as f64);
    let mut sum = 0.0;
    for i in 0..n {
        let x = x0 + (i as f64 + 0.5) * dx;
        for j in 0..n {
            let y = (j as f64 + 0.5) * dy;
            sum += height(a, x, y).min(height(b, x - d, y));
        }
    }
    2.0 * 2.0 * sum * dx * dy
}

#[test]
fn torus_booleans_occt_gets_right_are_accepted_and_right() {
    // Two equal tori moved apart sideways, where OCCT gets every boolean right: accepted,
    // with volumes (measured to 1e-9) matching ones integrated independently of OCCT.
    // OCCT gets others wrong (most along y, across the seam, and the smaller torus 0.3
    // along x: docs/ports/booleans.md, Known wrong results); Silico accepts those as
    // FreeCAD does, or refuses them as empty when they have no volume, so they aren't
    // here.
    use std::f64::consts::PI;
    let cases: [(f64, f64, &[[f64; 3]]); 2] = [
        (
            2.0,
            1.0,
            &[
                [0.5, 0.0, 0.0],
                [0.7, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.5, 0.0, 0.0],
                [2.2, 0.0, 0.0],
                [0.0, 1.5, 0.0],
                [0.0, 2.2, 0.0],
            ],
        ),
        (1.5, 0.5, &[[0.5, 0.0, 0.0], [0.8, 0.0, 0.0]]),
    ];
    for (major, minor, offsets) in cases {
        let t = Primitive::Torus {
            major_radius: major,
            minor_radius: minor,
        };
        let whole = 2.0 * PI * PI * major * minor * minor;
        let a = make(&t).unwrap();
        for &offset in offsets {
            let d = offset[0] + offset[1];
            let common = tori_common((major, minor), (major, minor), d, 3000);
            let wants = [2.0 * whole - common, whole - common, common];
            let b = at(t, offset);
            for (op, want) in OPS.into_iter().zip(wants) {
                let case = format!("{t:?} moved {offset:?}: {op:?}");
                let built = boolean(op, &a, &b).unwrap_or_else(|e| panic!("{case}: {e}"));
                assert_close(&case, summarize(&built.shape).unwrap().volume, want, 1e-4);
            }
        }
    }
}

#[test]
fn a_move_by_nothing_of_a_moved_shape_keeps_every_element() {
    // A shape with a location of its own, as every moved shape has, in this process and
    // read back from its bytes.
    let b = make(&Primitive::Box {
        dx: 1.0,
        dy: 2.0,
        dz: 3.0,
    })
    .unwrap();
    let moved = translate(&b, [1.0, 0.0, 0.0]).unwrap().shape;
    let read = silico_kernel::from_brep(&silico_kernel::to_brep(&moved).unwrap()).unwrap();
    for shape in [&moved, &read] {
        let h = translate(shape, [0.0, 0.0, 0.0]).unwrap().history;
        let i = &h.inputs[0];
        let kinds = [(&i.faces, 6), (&i.edges, 12), (&i.vertices, 8)];
        for (fates, count) in kinds {
            assert_eq!(fates.len(), count);
            for (n, fate) in fates.iter().enumerate() {
                assert_eq!(fate.kept, Some(n as u32 + 1), "{fate:?}");
                assert!(
                    fate.modified.is_empty() && fate.generated.is_empty(),
                    "{fate:?}"
                );
            }
        }
    }
}

#[test]
fn a_move_through_the_worker_goes_where_it_was_told() {
    let mut k = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let a = k.make(cube(10.0)).unwrap();
    let moved = k.translate(&a, [1.0, -2.0, 3.5]).unwrap();
    assert_eq!(k.summarize(&moved.shape).unwrap().center, [6.0, 3.0, 8.5]);
}

#[test]
fn a_moved_invalid_shape_is_still_invalid() {
    let open = silico_kernel::test_ops::open_box().unwrap();
    let moved = translate(&open, [1.0, 2.0, 3.0]).unwrap().shape;
    assert!(!moved.is_valid().unwrap());
    // And through the worker, which says so with the shape.
    let mut k = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let data = silico_core::kernel::ShapeData::unchecked(silico_protocol::Blob::inline(
        silico_kernel::to_brep(&open).unwrap(),
    ));
    let moved = k.translate(&data, [1.0, 2.0, 3.0]).unwrap();
    assert_eq!(moved.shape.valid(), Some(false));
}

#[test]
fn a_bad_offset_never_reaches_the_worker() {
    let mut k = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let a = k.make(cube(1.0)).unwrap();
    let sent = k.requests_sent();
    for offset in [
        [f64::NAN, 0.0, 0.0],
        [0.0, 2e6, 0.0],
        [0.0, 0.0, f64::NEG_INFINITY],
    ] {
        let err = k.translate(&a, offset).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
        assert!(err.message().contains("offset's"), "{err}");
        assert!(err.suggestion().is_some(), "{err}");
    }
    assert_eq!(k.requests_sent(), sent);
}
