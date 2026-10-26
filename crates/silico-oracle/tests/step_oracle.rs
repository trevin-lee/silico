//! STEP export against FreeCAD 1.1.3 (docs/ports/step-export.md, *Round trips*): every case
//! of tests/oracle (protected) built through a kernel worker, written as STEP by it, and read
//! back by FreeCAD's `Part.read` (`TopoShape::importStep`): the same volume and area as the
//! solid it was, measured as FreeCAD measures (both through OCCT 7.9.3's integration, so
//! the only difference is the file's digits: within [`BOUND`]), the same faces, valid if it
//! was; and, through `Import.insert` (ImportOCAF2), the product's name read back as its
//! object's label.
//! Needs the FreeCAD oracle (`pixi install -e oracle`) and the workers built.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use silico_core::kernel::KernelWorker;
use silico_oracle::case::load;
use silico_protocol::{ExportStep, StepPart, step_encoded};

const CASES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/oracle");

/// How far a solid read back from its STEP file may measure from what it was, relative: the
/// file's 13 significant digits, through intersection curves OCCT approximates as B-splines
/// (two spheres fused, the worst of the cases, read back 4.5e-11 off in volume with pcurves
/// written; 1.1e-6 without them, the reader rebuilding them).
const BOUND: f64 = 1e-10;

/// A solid as written, measured: its volume and area as FreeCAD measures them, its faces,
/// and whether it's valid.
struct Written {
    name: String,
    volume: f64,
    area: f64,
    faces: u64,
    valid: bool,
}

#[test]
fn every_oracle_case_written_as_step_reads_back_in_freecad_as_the_solid_it_was() {
    let temp = tempfile::Builder::new()
        .prefix("silico-step-oracle-")
        .tempdir()
        .unwrap();
    let dir = temp.path();
    let mut kernel = KernelWorker::start().unwrap();
    let mut written = Vec::new();
    for case in load(Path::new(CASES)).unwrap() {
        let Ok(shape) = silico_oracle::silico::build(&mut kernel, &case.shape) else {
            // A case the kernel refuses is the oracle's to compare, not this test's.
            continue;
        };
        let file = kernel
            .export_step(ExportStep {
                parts: vec![StepPart {
                    shape: shape.clone(),
                    id: "f0000001".to_owned(),
                    name: step_encoded(&case.name),
                }],
                assembly: "part".to_owned(),
                file_name: "part.silico".to_owned(),
            })
            .unwrap();
        assert!(
            file.unwritten.is_empty(),
            "{}: {:?}",
            case.name,
            file.unwritten
        );
        std::fs::write(
            dir.join(format!("{}.step", case.name)),
            file.step.as_bytes(),
        )
        .unwrap();
        let measured = kernel.measure_as_freecad(&shape).unwrap();
        let summary = kernel.summarize(&shape).unwrap();
        written.push(Written {
            name: case.name.clone(),
            volume: measured.volume,
            area: measured.area,
            faces: u64::from(summary.faces),
            valid: summary.valid,
        });
    }
    assert!(written.len() > 20, "{}", written.len());
    let names: Vec<&str> = written.iter().map(|w| w.name.as_str()).collect();
    let value = silico_oracle::freecad::script(&format!(
        r#"
import json, os, FreeCAD, Part, Import
D = {dir:?}
out = {{}}
for name in {names:?}:
    path = os.path.join(D, name + ".step")
    try:
        s = Part.read(path)
        read = [s.Volume, s.Area, len(s.Faces), s.isValid()]
    except Exception as e:
        out[name] = {{"error": repr(e)}}
        continue
    doc = FreeCAD.newDocument("rt")
    Import.insert(path, doc.Name)
    labels = [o.Label for o in doc.Objects if o.TypeId == "Part::Feature"]
    FreeCAD.closeDocument(doc.Name)
    out[name] = {{"read": read, "labels": labels}}
print("SILICO_ORACLE " + json.dumps(out))
"#,
        dir = dir.to_str().unwrap(),
    ))
    .unwrap();
    let mut wrong = Vec::new();
    for w in &written {
        let case = &value[w.name.as_str()];
        if let Some(error) = case.get("error") {
            wrong.push(format!("{}: {error}", w.name));
            continue;
        }
        let read = &case["read"];
        let relative = |ours: f64, theirs: f64| (ours - theirs).abs() / ours.abs().max(1e-300);
        let (volume, area) = (read[0].as_f64().unwrap(), read[1].as_f64().unwrap());
        if relative(w.volume, volume) > BOUND
            || relative(w.area, area) > BOUND
            || read[2].as_u64() != Some(w.faces)
            || (w.valid && read[3] != true)
        {
            wrong.push(format!(
                "{}: volume {} area {} faces {} valid {} read back as {read}",
                w.name, w.volume, w.area, w.faces, w.valid
            ));
        }
        if case["labels"] != serde_json::json!([w.name]) {
            wrong.push(format!("{}: labelled {}", w.name, case["labels"]));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
