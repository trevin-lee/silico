//! break-it: M4.8's STEP export on OCCT (`silico_kernel::export_step`, the worker's
//! `Op::ExportStep`, `KernelRequest::export_step`'s reply checks) with hostile parts: the
//! same `TShape` placed twice in one body, located compounds (a moved multi-solid body),
//! several located parts, faces shared by two solids, a membrane, an empty compound, free
//! faces and edges, many parts, duplicate shapes and names, very long names, and the worker
//! after a refusal. Expected values are by hand: a box is six faces; a product per part,
//! named and id'd as asked; the faces that bound material all in the file.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::exported::data;
use super::kernel::{kernel, step};
use super::oracle::freecad_shape;

use silico_core::kernel::ShapeData;
use silico_kernel::{BooleanOp, Primitive, boolean, make, make_box, translate};
use silico_protocol::{ExportStep, StepFile, StepPart, step_encoded};

/// An export of `parts` (each a shape, an ID, and a name, encoded here), of `bracket.silico`.
fn export(parts: &[(&ShapeData, &str, &str)]) -> ExportStep {
    ExportStep {
        parts: parts
            .iter()
            .map(|&(shape, id, name)| StepPart {
                shape: shape.clone(),
                id: step_encoded(id),
                name: step_encoded(name),
            })
            .collect(),
        assembly: "bracket".to_owned(),
        file_name: "bracket.silico".to_owned(),
    }
}

fn text(file: &StepFile) -> String {
    String::from_utf8(file.step.as_bytes().to_vec()).unwrap()
}

/// How many times `entity(` begins an entity in `text` (`#12 = ADVANCED_FACE(`).
fn entities(text: &str, entity: &str) -> usize {
    text.matches(&format!("= {entity}(")).count()
}

/// The 10 × 20 × 30 box.
fn block() -> silico_kernel::Shape {
    make_box(10.0, 20.0, 30.0).unwrap()
}

/// The box and the same box moved 100 along x (a location: the same `TShape`), fused: two
/// solids apart, as a body of two solids is.
fn two_apart() -> silico_kernel::Shape {
    let b = block();
    let moved = translate(&b, [100.0, 0.0, 0.0]).unwrap().shape;
    boolean(BooleanOp::Fuse, &b, &moved).unwrap().shape
}

#[test]
fn a_body_of_two_solids_apart_writes_all_twelve_faces_in_one_product() {
    // A fuse of a box and itself moved apart: one body, two solids, twelve faces that bound
    // material. One product, every face in the file, nothing unwritten.
    let body = data(&two_apart());
    let summary = kernel().summarize(&body).unwrap();
    assert_eq!(summary.faces, 12);
    assert!(
        (summary.volume - 12000.0).abs() < 1e-9,
        "{}",
        summary.volume
    );
    let file = step()
        .export_step(export(&[(&body, "f0000001", "Pair")]))
        .unwrap();
    let text = text(&file);
    assert!(file.unwritten.is_empty(), "{:?}", file.unwritten);
    assert_eq!(file.left_out, [Vec::<u32>::new()]);
    assert_eq!(entities(&text, "ADVANCED_FACE"), 12, "{text}");
    assert_eq!(entities(&text, "PRODUCT"), 1, "{text}");
    assert!(text.contains("PRODUCT('f0000001','Pair'"), "{text}");
}

#[test]
fn a_moved_body_of_two_solids_is_one_product_named_by_its_feature() {
    // The pair, moved: a located compound. XCAF and the writer's assembly mode (Auto) could
    // make its located compound an assembly of its own, products OCCT names: one product,
    // named and id'd by its feature, and every face in the file.
    let body = data(&translate(&two_apart(), [0.0, 5.0, 0.0]).unwrap().shape);
    let file = step()
        .export_step(export(&[(&body, "f0000002", "Moved pair")]))
        .unwrap();
    let text = text(&file);
    assert!(file.unwritten.is_empty(), "{:?}", file.unwritten);
    assert_eq!(entities(&text, "ADVANCED_FACE"), 12, "{text}");
    assert_eq!(entities(&text, "PRODUCT"), 1, "{text}");
    assert!(text.contains("PRODUCT('f0000002','Moved pair'"), "{text}");
    assert_eq!(
        entities(&text, "NEXT_ASSEMBLY_USAGE_OCCURRENCE"),
        0,
        "{text}"
    );
}

#[test]
fn two_located_parts_are_two_products_under_the_assembly_and_no_other() {
    // Two parts, each placed by a location: three products (the assembly and the two),
    // each part's own id and name, none of OCCT's ("SOLID").
    let a = data(&translate(&block(), [0.0, 0.0, 50.0]).unwrap().shape);
    let b = data(&translate(&block(), [0.0, 70.0, 0.0]).unwrap().shape);
    let file = step()
        .export_step(export(&[
            (&a, "a0000001", "Upper"),
            (&b, "b0000002", "Side"),
        ]))
        .unwrap();
    let text = text(&file);
    assert!(file.unwritten.is_empty(), "{:?}", file.unwritten);
    assert_eq!(entities(&text, "PRODUCT"), 3, "{text}");
    assert!(text.contains("PRODUCT('','bracket'"), "{text}");
    assert!(text.contains("PRODUCT('a0000001','Upper'"), "{text}");
    assert!(text.contains("PRODUCT('b0000002','Side'"), "{text}");
    assert!(!text.contains("'SOLID'"), "{text}");
    assert_eq!(entities(&text, "ADVANCED_FACE"), 12, "{text}");
}

#[test]
fn the_same_shape_twice_is_two_products_each_with_its_own_id() {
    // The same B-rep bytes as two parts (two features that built the same box), and the same
    // name: two products, never one shared, each with its own id, in order.
    let b = data(&block());
    let file = step()
        .export_step(export(&[(&b, "f0000001", "Box"), (&b, "f0000002", "Box")]))
        .unwrap();
    let text = text(&file);
    assert!(file.unwritten.is_empty(), "{:?}", file.unwritten);
    assert_eq!(entities(&text, "PRODUCT"), 3, "{text}");
    let first = text.find("PRODUCT('f0000001','Box'").unwrap();
    let second = text.find("PRODUCT('f0000002','Box'").unwrap();
    assert!(first < second, "{text}");
    assert_eq!(entities(&text, "ADVANCED_FACE"), 12, "{text}");
    assert_eq!(file.left_out, [Vec::<u32>::new(), Vec::new()]);
}

#[test]
fn forty_parts_each_get_their_own_id_and_name_in_order() {
    // Forty boxes of different sizes: each product's id and name are the part's own, in
    // the request's order; forty-one products; 240 faces.
    let shapes: Vec<ShapeData> = (1..=40)
        .map(|n| data(&make_box(f64::from(n), 2.0, 3.0).unwrap()))
        .collect();
    let ids: Vec<String> = (1..=40).map(|n| format!("p{n:07}")).collect();
    let names: Vec<String> = (1..=40).map(|n| format!("Part {n}")).collect();
    let parts: Vec<(&ShapeData, &str, &str)> = shapes
        .iter()
        .zip(&ids)
        .zip(&names)
        .map(|((s, i), n)| (s, i.as_str(), n.as_str()))
        .collect();
    let file = step().export_step(export(&parts)).unwrap();
    let text = text(&file);
    assert!(file.unwritten.is_empty(), "{:?}", file.unwritten);
    assert_eq!(entities(&text, "PRODUCT"), 41);
    assert_eq!(entities(&text, "ADVANCED_FACE"), 240);
    let mut at = 0;
    for (id, name) in ids.iter().zip(&names) {
        let found = text[at..]
            .find(&format!("PRODUCT('{id}','{name}'"))
            .unwrap_or_else(|| panic!("{id} {name}: {text}"));
        at += found + 1;
    }
    assert_eq!(file.left_out.len(), 40);
}

#[test]
fn a_membrane_inside_a_box_is_left_out_and_the_box_written_whole() {
    // FreeCAD 1.1.3's generalFuse of a box and a square inside it at x = 5: a valid solid
    // whose seventh face is INTERNAL. Left out (the writer drops it); the box's six faces
    // all in the file; nothing unwritten, though the membrane's edges and vertices are
    // inside the solid.
    let shape = freecad_shape(
        "m = Part.Face(Part.makePolygon([V(5, 5, 5), V(5, 15, 5), V(5, 15, 25), V(5, 5, 25), \
         V(5, 5, 5)]))\nfused, _ = Part.makeBox(10, 20, 30).generalFuse([m])\nshape = \
         fused.Solids[0]\nassert [f.Orientation for f in shape.Faces].count('Internal') == 1",
    );
    let file = step()
        .export_step(export(&[(&shape, "f0000001", "Membrane")]))
        .unwrap();
    assert_eq!(file.left_out, [vec![7]]);
    assert!(file.unwritten.is_empty(), "{:?}", file.unwritten);
    assert_eq!(entities(&text(&file), "ADVANCED_FACE"), 6);
}

#[test]
fn two_solids_sharing_a_face_are_written_whole_and_nothing_said() {
    // Two boxes side by side, split by generalFuse into a compound of two solids sharing
    // the face at x = 10: eleven faces in the body, as TopExp::MapShapes counts them. Each
    // solid's closed shell holds the shared face, so the writer writes it once a solid:
    // twelve face surfaces, every face of the body in the file, none dropped.
    // BREAKS: the worker's count check (`written != expected`, bridge.cpp) takes any count
    // but eleven as faces missing and reports "the file holds 12 faces of the 11 the parts
    // have that bound material" as unwritten, which the core refuses as "the STEP writer
    // couldn't write all of the file": a whole file refused, with a message that says the
    // opposite of what happened.
    let shape = freecad_shape(
        "a = Part.makeBox(10, 20, 30)\nb = Part.makeBox(10, 20, 30, V(10, 0, 0))\nfused, _ = \
         a.generalFuse([b])\nshape = Part.Compound(fused.Solids)\nassert len(shape.Solids) == 2",
    );
    let summary = kernel().summarize(&shape).unwrap();
    assert_eq!(summary.faces, 11);
    let file = step()
        .export_step(export(&[(&shape, "f0000001", "Split")]))
        .unwrap();
    let written = entities(&text(&file), "ADVANCED_FACE");
    assert!(written >= 11, "{written}");
    assert!(file.unwritten.is_empty(), "{:?}", file.unwritten);
}

#[test]
fn an_empty_compound_is_refused_or_written_empty_but_never_crashes_the_worker() {
    // No faces, no solid: the core never sends one; the worker answers it (a refusal or a
    // file) and goes on to export a box.
    let empty = freecad_shape("shape = Part.Compound([])");
    let mut worker = step();
    match worker.export_step(export(&[(&empty, "f0000001", "Nothing")])) {
        Ok(file) => {
            assert_eq!(entities(&text(&file), "ADVANCED_FACE"), 0);
            assert_eq!(file.left_out, [Vec::<u32>::new()]);
        }
        Err(e) => assert!(!e.code().is_kernel_fault(), "{e:?}"),
    }
    let b = data(&block());
    let file = worker
        .export_step(export(&[(&b, "f0000002", "Box")]))
        .unwrap();
    assert_eq!(entities(&text(&file), "ADVANCED_FACE"), 6);
}

#[test]
fn a_lone_face_and_a_lone_edge_beside_a_box_write_every_face() {
    // A box, a square face apart from it, and a free edge, in one compound: seven faces
    // that bound material (OCCT's writer gives the face a surface model); all in the file,
    // or what wasn't, said.
    let shape = freecad_shape(
        "f = Part.Face(Part.makePolygon([V(50, 0, 0), V(60, 0, 0), V(60, 10, 0), V(50, 10, 0), \
         V(50, 0, 0)]))\ne = Part.makeLine(V(-20, 0, 0), V(-10, 0, 0))\nshape = \
         Part.Compound([Part.makeBox(10, 20, 30), f, e])",
    );
    let file = step()
        .export_step(export(&[(&shape, "f0000001", "Mixed")]))
        .unwrap();
    let written = entities(&text(&file), "ADVANCED_FACE");
    assert!(
        written == 7 || !file.unwritten.is_empty(),
        "{written} faces written of 7, unwritten {:?}",
        file.unwritten
    );
}

#[test]
fn a_refused_export_leaves_the_worker_writing_the_same_bytes() {
    // An export refused for a string not encoded, then the box: the same bytes as a fresh
    // worker's.
    let b = data(&block());
    let mut worker = step();
    let mut bad = export(&[(&b, "f0000001", "Box")]);
    bad.assembly = "a\\N\\b".to_owned();
    assert!(worker.export_step(bad).is_err());
    let mut none = export(&[]);
    none.parts.clear();
    assert!(worker.export_step(none).is_err());
    let after = worker
        .export_step(export(&[(&b, "f0000001", "Box")]))
        .unwrap();
    let fresh = step()
        .export_step(export(&[(&b, "f0000001", "Box")]))
        .unwrap();
    assert_eq!(after.step, fresh.step);
}

#[test]
fn two_parts_are_the_same_bytes_in_any_worker_after_any_other_export() {
    // The existing determinism test compares one-part files, which have no assembly.
    // BREAKS: each NEXT_ASSEMBLY_USAGE_OCCURRENCE's id is OCCT's process-wide counter
    // (`static Standard_Integer id` in STEPConstruct_Assembly::MakeRelationship), so the
    // same two-part request gives `NEXT_ASSEMBLY_USAGE_OCCURRENCE('1',...)` and `('2',...)`
    // in a fresh worker and `('3',...)`, `('4',...)` in one that wrote an assembly before:
    // different bytes for the same model, depending on which pool worker served it.
    let a = data(&translate(&block(), [0.0, 0.0, 50.0]).unwrap().shape);
    let c = data(
        &make(&Primitive::Cylinder {
            radius: 3.0,
            height: 7.0,
        })
        .unwrap(),
    );
    let request = || export(&[(&a, "a0000001", "Upper"), (&c, "c0000002", "Pin ü")]);
    let mut worker = step();
    let first = worker.export_step(request()).unwrap();
    worker
        .export_step(export(&[(&c, "z0000009", "Other")]))
        .unwrap();
    let again = worker.export_step(request()).unwrap();
    let elsewhere = step().export_step(request()).unwrap();
    assert_eq!(again.step, first.step);
    assert_eq!(elsewhere.step, first.step);
}

#[test]
fn a_long_name_is_written_whole_on_its_line_as_encoded() {
    // OCCT wraps a string past 72 columns: at the last space, else the last backslash, else
    // the last `_`, else at column 72. ISO 10303-21 ignores line ends, but OCCT's own reader
    // decodes a `\X2\` run with the line end in it (tests/adversarial_step_oracle.rs in
    // silico-oracle reads the same names back through FreeCAD).
    // BREAKS: the encoded name is split across lines inside its `\X2\` run (and FreeCAD
    // reads `?`): the file doesn't hold the name as encoded.
    let b = data(&block());
    for name in [
        "ü".repeat(40),
        format!("{}ü{}", "a".repeat(66), "b".repeat(20)),
        "\u{1F600}".repeat(12),
    ] {
        let file = step()
            .export_step(export(&[(&b, "f0000001", &name)]))
            .unwrap();
        let text = text(&file);
        assert!(
            text.contains(&format!("'{}'", step_encoded(&name))),
            "{name:?}: {text}"
        );
    }
}

#[test]
fn an_eight_megabyte_name_is_written_within_the_kernels_time() {
    // A document may hold a name of megabytes (its bound is the document's 16 MiB, and the
    // gate bounds no name). OCCT wraps a long string 72 columns at a time, copying the rest
    // of it each time (StepData_StepWriter::Send, `aVal.Split(aStop)`): quadratic. Measured
    // here: a 4 MiB name (no space) took 36 s.
    // BREAKS: an 8 MiB name runs past the kernel request's 60 s: the worker is killed and
    // the export ends as a worker timeout, for a file of 8 MiB that is linear to write.
    let b = data(&block());
    let name = "a".repeat(8 << 20);
    let start = std::time::Instant::now();
    let result = step().export_step(export(&[(&b, "f0000001", &name)]));
    let took = start.elapsed();
    let file = result.unwrap_or_else(|e| panic!("after {took:?}: {e:?}"));
    assert!(file.step.as_bytes().len() > 8 << 20);
}
