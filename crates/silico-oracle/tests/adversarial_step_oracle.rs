//! Hostile names through the STEP export, read back by FreeCAD 1.1.3 (break-it, M4.8 STEP).
//! Each name a document can hold is exported as a 10 × 20 × 30 box's product name (the
//! core's encoding, `step_encoded`), and the file read back: by `Part.read`, whose volume
//! must be the box's 6000 mm³, and by `Import.insert`, whose one object's label must be
//! the name exactly (docs/ports/step-export.md, *Strings*: "every reader reads back as the
//! text it encodes"). Expected values are by hand: the box's volume and the name given.
//! Needs the FreeCAD oracle (`pixi install -e oracle`) and the workers built.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::kernel::KernelWorker;
use silico_core::step_worker::StepWorker;
use silico_protocol::{ExportStep, Primitive, StepPart, step_encoded};

/// Each name, and the header's file name, exported and read back by FreeCAD: what came
/// back for each, as `(name, file name, read volume or error, labels)`.
fn read_back(cases: &[(&str, &str)]) -> Vec<(String, String, serde_json::Value)> {
    let temp = tempfile::Builder::new()
        .prefix("silico-step-hostile-")
        .tempdir()
        .unwrap();
    let dir = temp.path();
    let mut kernel = KernelWorker::start().unwrap();
    let mut step = StepWorker::start().unwrap();
    let block = kernel
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    for (n, &(name, file_name)) in cases.iter().enumerate() {
        let written = step
            .export_step(ExportStep {
                parts: vec![StepPart {
                    shape: block.clone(),
                    id: "f0000001".to_owned(),
                    name: step_encoded(name),
                }],
                assembly: "part".to_owned(),
                file_name: step_encoded(file_name),
            })
            .unwrap();
        assert!(
            written.unwritten.is_empty(),
            "{name:?}: {:?}",
            written.unwritten
        );
        // The file holds the name exactly, encoded, whatever a reader makes of it.
        let text = String::from_utf8(written.step.as_bytes().to_vec()).unwrap();
        assert!(
            text.contains(&format!("PRODUCT('f0000001','{}'", step_encoded(name))),
            "{name:?}"
        );
        std::fs::write(dir.join(format!("{n}.step")), written.step.as_bytes()).unwrap();
    }
    let value = silico_oracle::freecad::script(&format!(
        r#"
import json, os, FreeCAD, Part, Import
D = {dir:?}
out = []
for n in range({count}):
    path = os.path.join(D, "%d.step" % n)
    try:
        s = Part.read(path)
        volume = s.Volume
    except Exception as e:
        volume = repr(e)
    try:
        doc = FreeCAD.newDocument("rt%d" % n)
        Import.insert(path, doc.Name)
        labels = [o.Label for o in doc.Objects if o.TypeId == "Part::Feature"]
        FreeCAD.closeDocument(doc.Name)
    except Exception as e:
        labels = repr(e)
    out.append({{"volume": volume, "labels": labels}})
print("SILICO_ORACLE " + json.dumps(out))
"#,
        dir = dir.to_str().unwrap(),
        count = cases.len(),
    ))
    .unwrap();
    cases
        .iter()
        .zip(value.as_array().unwrap())
        .map(|(&(name, file_name), read)| (name.to_owned(), file_name.to_owned(), read.clone()))
        .collect()
}

/// The label FreeCAD gives the box named `name`: the name, but where OCCT's STEP reader
/// takes a product's name only if `TCollection_HAsciiString::UsefullLength` finds a byte
/// `std::isgraph` accepts (STEPCAFControl_Reader.cxx, `ReadNames`): in the C locale the
/// oracle runs FreeCAD in on every platform (`freecad.rs`), no byte past ASCII is graphic,
/// so a name with no ASCII graphic character comes back as the product's id
/// (docs/ports/step-export.md, *Round trips*). The file holds the name either way.
fn label_of(name: &str) -> String {
    let readable = name.bytes().any(|b| b.is_ascii_graphic());
    if !readable {
        "f0000001".to_owned()
    } else {
        name.to_owned()
    }
}

/// The cases whose file FreeCAD didn't read back as the box named `name` (as its reader
/// can: [`label_of`]).
fn misread(cases: &[(&str, &str)]) -> Vec<String> {
    read_back(cases)
        .into_iter()
        .filter(|(name, _, read)| {
            let volume_ok = read["volume"]
                .as_f64()
                .is_some_and(|v| (v - 6000.0).abs() < 1e-6);
            !volume_ok || read["labels"] != serde_json::json!([label_of(name)])
        })
        .map(|(name, file_name, read)| format!("{name:?} in {file_name:?}: {read}"))
        .collect()
}

#[test]
fn a_name_longer_than_a_step_line_reads_back_as_itself() {
    // OCCT's writer wraps a string past its 72-column line at the last space, else the
    // last backslash, else the last `_`, else at column 72 (StepData_StepWriter.cxx,
    // `Send(TCollection_AsciiString)`), and its reader (`cleanText`) decodes a `\X2\` run
    // with the line break still in it.
    // BREAKS: a long name whose encoding has no space is split inside a `\X2\` or `\X4\`
    // run, or between the two apostrophes of a doubled `'`, and FreeCAD reads it back as
    // something else; the export is `Ok`, so the file silently carries another name.
    let umlauts = "ü".repeat(40);
    let emoji = "\u{1F600}".repeat(12);
    let apostrophes = "'".repeat(100);
    let mixed = format!("{}ü{}", "a".repeat(66), "b".repeat(20));
    let plain = "a".repeat(200);
    let spaced = "word ".repeat(40);
    let underscored = "a_".repeat(100);
    let wrong = misread(&[
        (&umlauts, "part.silico"),
        (&emoji, "part.silico"),
        (&apostrophes, "part.silico"),
        (&mixed, "part.silico"),
        (&plain, "part.silico"),
        (spaced.trim_end(), "part.silico"),
        (&underscored, "part.silico"),
    ]);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_name_holding_an_apostrophe_before_a_comma_or_parenthesis_reads_back_as_itself() {
    // OCCT doubles `'` (`it's` is written `'it''s'`), and its lexer ends a string at an
    // apostrophe followed by `,` or `)` (step.lex, `<Text>[']/[" "\n\r]*[\)\,]`), so the
    // second apostrophe of `''` before one ends it.
    // BREAKS: `Bracket 'A', left` is written `'Bracket ''A'', left'` and FreeCAD can't read
    // the file back (or reads another name): every name passes `'` through raw.
    let wrong = misread(&[
        ("Bracket 'A', left", "part.silico"),
        ("x')", "part.silico"),
        ("a' ,b", "part.silico"),
        ("it's", "part.silico"),
    ]);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_document_file_name_that_is_hostile_still_gives_a_file_that_reads_back() {
    // The header's FILE_NAME is the document's file name, encoded as a name is: a long
    // one, one with an apostrophe before a comma, one of only non-ASCII characters.
    let long = format!("{}.silico", "ü".repeat(40));
    let wrong = misread(&[
        ("Block", &long),
        ("Block", "rev 'A', left.silico"),
        ("Block", "Kl\u{F6}tzchen \u{1F600}.silico"),
        ("Block", ""),
    ]);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_name_of_no_ascii_at_all_is_written_exactly_and_read_as_the_platforms_reader_can() {
    // Short names with no ASCII graphic character: written exactly (read_back checks the
    // file), read back as the product's id by OCCT's reader in the oracle's C locale, on
    // every platform (label_of): OCCT's bug, followed and said, not worked around.
    let wrong = misread(&[
        ("\u{652F}\u{67B6}", "part.silico"),
        ("\u{FC}", "part.silico"),
        ("\u{1F600}", "part.silico"),
    ]);
    assert!(wrong.is_empty(), "{wrong:#?}");
}
