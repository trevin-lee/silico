//! STEP export on OCCT (docs/ports/step-export.md): each part a product named by its
//! feature, written by OCCT's XCAF writer with every setting given, the header written from
//! the document, so the bytes are the same every run, after any other export, in any worker,
//! and on every platform (the golden hashes, the same on macOS and Linux in CI). Expected
//! values are by hand: a box is one MANIFOLD_SOLID_BREP of six ADVANCED_FACEs; the header's
//! fields are the note's recommendations; the names are the encoder's, read as written.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::exported::data;
use super::kernel::kernel;

use sha2::{Digest, Sha256};
use silico_kernel::{Primitive, Shape, make, make_box};
use silico_protocol::{ExportStep, StepFile, StepPart, step_encoded};

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// An export of `parts` (each a shape, an ID, and a name, encoded here), of `bracket.silico`.
fn export(parts: &[(&Shape, &str, &str)]) -> ExportStep {
    ExportStep {
        parts: parts
            .iter()
            .map(|&(shape, id, name)| StepPart {
                shape: data(shape),
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

/// A cylinder of radius 3 and height 7.
fn cylinder() -> Shape {
    make(&Primitive::Cylinder {
        radius: 3.0,
        height: 7.0,
    })
    .unwrap()
}

/// The 10 × 20 × 30 box.
fn block() -> Shape {
    make_box(10.0, 20.0, 30.0).unwrap()
}

#[test]
fn a_box_is_one_product_its_header_written_from_the_document() {
    let file = kernel()
        .export_step(export(&[(&block(), "k7f2qx3m", "Block")]))
        .unwrap();
    let text = text(&file);
    assert!(text.starts_with("ISO-10303-21;\nHEADER;\n"), "{text}");
    let header = &text[..text.find("ENDSEC;").unwrap()];
    assert_eq!(
        header,
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('bracket.silico',\
         '1970-01-01T00:00:00',(''),(''),\n  'Open CASCADE STEP processor 7.9','Silico','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\n"
    );
    assert_eq!(
        text.matches("PRODUCT('k7f2qx3m','Block'").count(),
        1,
        "{text}"
    );
    assert_eq!(entities(&text, "PRODUCT"), 1);
    assert_eq!(entities(&text, "MANIFOLD_SOLID_BREP"), 1);
    assert_eq!(entities(&text, "ADVANCED_FACE"), 6);
    // In millimetres; a pcurve for each of the 12 edges on each of its two faces.
    assert!(text.contains("SI_UNIT(.MILLI.,.METRE.)"), "{text}");
    assert_eq!(entities(&text, "PCURVE"), 24);
    assert_eq!(file.left_out, [Vec::<u32>::new()]);
    assert!(file.unwritten.is_empty(), "{:?}", file.unwritten);
}

#[test]
fn a_step_file_is_the_same_every_run_after_any_other_and_in_any_worker() {
    // OCCT's plain writer numbers its products by a counter that runs for the life of the
    // process, and its statics outlive a request: the same request gives the same bytes in
    // a new worker, again in the same one, and after other exports there.
    let request = || export(&[(&block(), "k7f2qx3m", "Block")]);
    let mut worker = kernel();
    let first = worker.export_step(request()).unwrap();
    worker
        .export_step(export(&[
            (&cylinder(), "c0000001", "Other"),
            (&block(), "c0000002", "Again"),
        ]))
        .unwrap();
    let again = worker.export_step(request()).unwrap();
    let elsewhere = kernel().export_step(request()).unwrap();
    assert_eq!(again.step, first.step);
    assert_eq!(elsewhere.step, first.step);
    // The golden: the same on every platform CI runs (macOS arm64 and Linux x86-64).
    assert_eq!(hex(first.step.as_bytes()), BOX_STEP_SHA256);
}

/// The box's STEP file's SHA-256, as this build writes it on every platform.
const BOX_STEP_SHA256: &str = "67a46f6e348ae45329be29ccc4d516213a2c2665782230da0ca9924afd5ddec8";

#[test]
fn two_parts_are_an_assembly_named_by_the_documents_stem_each_a_product_in_order() {
    let file = kernel()
        .export_step(export(&[
            (&cylinder(), "p0000001", "Pin"),
            (&block(), "k7f2qx3m", "Block"),
        ]))
        .unwrap();
    let text = text(&file);
    assert_eq!(entities(&text, "PRODUCT"), 3, "{text}");
    assert!(text.contains("PRODUCT('','bracket'"), "{text}");
    let pin = text.find("PRODUCT('p0000001','Pin'").unwrap();
    let block = text.find("PRODUCT('k7f2qx3m','Block'").unwrap();
    assert!(pin < block, "{text}");
    assert_eq!(entities(&text, "NEXT_ASSEMBLY_USAGE_OCCURRENCE"), 2);
    assert_eq!(entities(&text, "MANIFOLD_SOLID_BREP"), 2);
    assert_eq!(file.left_out, [Vec::<u32>::new(), Vec::new()]);
    assert!(file.unwritten.is_empty(), "{:?}", file.unwritten);
}

#[test]
fn every_name_is_written_encoded_as_iso_10303_21_reads_it() {
    // The note's table: names OCCT would read back as something else, encoded so they
    // can't be (an umlaut, a backslash that looks like a directive, an emoji, an apostrophe).
    for (name, written) in [
        ("Klötzchen", "Kl\\X2\\00F6\\X0\\tzchen"),
        ("a\\N\\b", "a\\X2\\005C\\X0\\N\\X2\\005C\\X0\\b"),
        ("smile \u{1F600} end", "smile \\X4\\0001F600\\X0\\ end"),
        ("it's", "it\\X2\\0027\\X0\\s"),
    ] {
        let file = kernel()
            .export_step(export(&[(&block(), "k7f2qx3m", name)]))
            .unwrap();
        let text = text(&file);
        assert!(
            text.contains(&format!("PRODUCT('k7f2qx3m','{written}'")),
            "{name}: {text}"
        );
    }
}

#[test]
fn a_face_that_bounds_no_material_is_left_out_and_said_and_nothing_else() {
    // The 10 × 20 × 30 box whose face at x = 0 is INTERNAL, or EXTERNAL (the fixtures'
    // README): OCCT's writer drops it (six faces, five written), and it's said, as the 3MF
    // export says it, and not as unwritten: the five faces that bound material are all in
    // the file.
    for name in ["box_internal_face1.bin", "box_external_face1.bin"] {
        let mut bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/brep")
                .join(name),
        )
        .unwrap();
        // The checksum Silico appends to a B-rep (the fixtures' README).
        bytes.extend_from_slice(&crc32fast::hash(&bytes).to_le_bytes());
        let shape = silico_kernel::from_brep(&bytes).unwrap();
        let file = kernel()
            .export_step(export(&[(&shape, "k7f2qx3m", "Block")]))
            .unwrap();
        assert_eq!(file.left_out.len(), 1, "{name}");
        assert_eq!(file.left_out[0].len(), 1, "{name}: {:?}", file.left_out);
        assert!(file.unwritten.is_empty(), "{name}: {:?}", file.unwritten);
        assert_eq!(entities(&text(&file), "ADVANCED_FACE"), 5, "{name}");
    }
}

#[test]
fn a_string_not_encoded_is_refused_by_the_worker_naming_its_field() {
    // The core never sends one; the worker refuses it before OCCT reads it as a directive.
    let mut asked = export(&[(&block(), "k7f2qx3m", "Block")]);
    asked.parts[0].name = "a\\N\\b".to_owned();
    let failure = silico_kernel::worker::handle(silico_protocol::Op::ExportStep(Box::new(asked)))
        .unwrap_err();
    assert!(
        matches!(
            &failure.refusal,
            silico_protocol::Refusal::InvalidParameter { parameter, .. } if parameter == "name"
        ),
        "{failure:?}"
    );
}

#[test]
fn a_part_placed_by_a_location_is_one_product_named_by_its_feature() {
    // XCAF makes a located shape a reference to a prototype: two products for one body, the
    // inner one OCCT's "SOLID". Given in a compound of its own, it's one product, named.
    let moved = silico_kernel::translate(&block(), [5.0, 0.0, 0.0])
        .unwrap()
        .shape;
    let file = kernel()
        .export_step(export(&[(&moved, "k7f2qx3m", "Block")]))
        .unwrap();
    let text = text(&file);
    assert_eq!(entities(&text, "PRODUCT"), 1, "{text}");
    assert!(text.contains("PRODUCT('k7f2qx3m','Block'"), "{text}");
    assert_eq!(
        entities(&text, "NEXT_ASSEMBLY_USAGE_OCCURRENCE"),
        0,
        "{text}"
    );
    assert_eq!(entities(&text, "ADVANCED_FACE"), 6);
    assert!(file.unwritten.is_empty(), "{:?}", file.unwritten);
}
