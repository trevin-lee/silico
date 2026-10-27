//! STEP export on OCCT (docs/ports/step-export.md): each part a product named by its
//! feature, written by OCCT's XCAF writer with every setting given, the header written from
//! the document, so the bytes are the same every run, after any other export, in any worker,
//! and on every platform (the golden hashes, the same on macOS and Linux in CI). Expected
//! values are by hand: a box is one MANIFOLD_SOLID_BREP of six ADVANCED_FACEs; the header's
//! fields are the note's recommendations; the names are the encoder's, read as written.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::exported::data;
use super::kernel::{kernel, step};

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
    let file = step()
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
    let mut worker = step();
    let first = worker.export_step(request()).unwrap();
    worker
        .export_step(export(&[
            (&cylinder(), "c0000001", "Other"),
            (&block(), "c0000002", "Again"),
        ]))
        .unwrap();
    let again = worker.export_step(request()).unwrap();
    let elsewhere = step().export_step(request()).unwrap();
    assert_eq!(again.step, first.step);
    assert_eq!(elsewhere.step, first.step);
    // The golden: the same on every platform CI runs (macOS arm64 and Linux x86-64).
    assert_eq!(hex(first.step.as_bytes()), BOX_STEP_SHA256);
}

/// The box's STEP file's SHA-256, as this build writes it on every platform.
const BOX_STEP_SHA256: &str = "67a46f6e348ae45329be29ccc4d516213a2c2665782230da0ca9924afd5ddec8";

#[test]
fn two_parts_are_an_assembly_named_by_the_documents_stem_each_a_product_in_order() {
    let file = step()
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
    // Each link's id its place among them, from 1 (OCCT's counter is the process's).
    assert!(
        text.contains("NEXT_ASSEMBLY_USAGE_OCCURRENCE('1',"),
        "{text}"
    );
    assert!(
        text.contains("NEXT_ASSEMBLY_USAGE_OCCURRENCE('2',"),
        "{text}"
    );
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
        let file = step()
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
        let file = step()
            .export_step(export(&[(&shape, "k7f2qx3m", "Block")]))
            .unwrap();
        assert_eq!(file.left_out.len(), 1, "{name}");
        assert_eq!(file.left_out[0].len(), 1, "{name}: {:?}", file.left_out);
        assert!(file.unwritten.is_empty(), "{name}: {:?}", file.unwritten);
        assert_eq!(entities(&text(&file), "ADVANCED_FACE"), 5, "{name}");
        // Counted without it: five faces, five bounds, all written.
        let five = silico_protocol::StepCount {
            expected: 5,
            written: 5,
        };
        assert_eq!((file.faces, file.bounds), (five, five), "{name}");
    }
}

#[test]
fn the_kernel_worker_writes_no_step_and_links_none_of_steps_toolkits() {
    // STEP has a worker of its own: the kernel worker refuses it as another worker's, and
    // carries none of its toolkits (nor what TKService loads), so its startup, memory, and
    // identity are the kernel's alone. The STEP worker links them.
    let refused = kernel()
        .run(
            silico_core::kernel::KernelRequest::export_step(export(&[(
                &block(),
                "k7f2qx3m",
                "Block",
            )]))
            .unwrap(),
        )
        .unwrap_err();
    assert!(refused.message().contains("ExportStep"), "{refused:?}");
    let bytes = |exe: &str| std::fs::read(exe).unwrap();
    let names = |bytes: &[u8], lib: &str| bytes.windows(lib.len()).any(|w| w == lib.as_bytes());
    let kernel_worker = bytes(env!("CARGO_BIN_EXE_silico-kernel-worker"));
    let step_worker = bytes(env!("CARGO_BIN_EXE_silico-step-worker"));
    for lib in [
        "libTKDESTEP",
        "libTKXCAF",
        "libTKXSBase",
        "libTKService",
        "libTKV3d",
    ] {
        assert!(!names(&kernel_worker, lib), "the kernel worker links {lib}");
    }
    for lib in ["libTKDESTEP", "libTKXCAF"] {
        assert!(
            names(&step_worker, lib),
            "the STEP worker doesn't link {lib}"
        );
    }
}

#[test]
fn the_step_worker_imports_no_platform_math() {
    // As every executable linking OCCT: check-math-imports.sh on the STEP worker itself.
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../third_party/occt/scripts/check-math-imports.sh"
    );
    let out = std::process::Command::new("bash")
        .arg(script)
        .arg(env!("CARGO_BIN_EXE_silico-step-worker"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_face_the_writer_drops_is_said_by_the_count_from_a_real_worker() {
    // A box and a face with no wires (Face7): OCCT's writer drops the face with only a
    // warning (" No Wires of this Face were mapped"); the worker reports seven faces that
    // bound material, six written, for the core to refuse. Nothing is on the check list.
    let shape = silico_kernel::test_ops::box_with_wireless_face().unwrap();
    let file = step()
        .export_step(export(&[(&shape, "k7f2qx3m", "Block")]))
        .unwrap();
    assert_eq!(
        file.faces,
        silico_protocol::StepCount {
            expected: 7,
            written: 6
        }
    );
    // The box's six faces, one bound each, all written.
    assert_eq!(
        file.bounds,
        silico_protocol::StepCount {
            expected: 6,
            written: 6
        }
    );
    assert!(file.unwritten.is_empty(), "{:?}", file.unwritten);
    // A box alone: all written.
    let file = step()
        .export_step(export(&[(&block(), "k7f2qx3m", "Block")]))
        .unwrap();
    assert_eq!(
        (file.faces, file.bounds),
        (
            silico_protocol::StepCount {
                expected: 6,
                written: 6
            },
            silico_protocol::StepCount {
                expected: 6,
                written: 6
            }
        )
    );
}

#[test]
fn a_part_placed_by_a_location_is_one_product_named_by_its_feature() {
    // XCAF makes a located shape a reference to a prototype: two products for one body, the
    // inner one OCCT's "SOLID". Given in a compound of its own, it's one product, named.
    let moved = silico_kernel::translate(&block(), [5.0, 0.0, 0.0])
        .unwrap()
        .shape;
    let file = step()
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

#[test]
fn a_wire_the_writer_drops_is_said_by_the_bounds_count_and_one_it_skips_on_purpose_isnt() {
    // A square face beside the box with a second, empty wire: the writer drops that wire
    // (" a Wire not mapped") with only a warning, so the file holds 7 of the 8 bounds. With an
    // INTERNAL wire (a slit) instead, which the writer skips on purpose
    // (TopoDSToStep_MakeStepWire), it isn't counted: 7 of 7, whole.
    let count = |expected, written| silico_protocol::StepCount { expected, written };
    let dropped = silico_kernel::test_ops::box_with_empty_wire_face().unwrap();
    let file = step()
        .export_step(export(&[(&dropped, "k7f2qx3m", "Block")]))
        .unwrap();
    assert_eq!((file.faces, file.bounds), (count(7, 7), count(8, 7)));
    let skipped = silico_kernel::test_ops::box_with_internal_wire_face().unwrap();
    let file = step()
        .export_step(export(&[(&skipped, "k7f2qx3m", "Block")]))
        .unwrap();
    assert_eq!((file.faces, file.bounds), (count(7, 7), count(7, 7)));
}
