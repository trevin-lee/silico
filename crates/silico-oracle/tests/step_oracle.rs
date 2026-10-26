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
use silico_core::step_worker::StepWorker;
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
    /// Where it is: its tight bounding box and its center, as FreeCAD measures them, which a
    /// rigid motion lost on the way would move (volume, area, and faces wouldn't).
    bounds: Option<[[f64; 3]; 2]>,
    center: [f64; 3],
}

/// How far a solid read back may be from where it was, as a fraction of its box's diagonal.
const PLACED: f64 = 1e-9;

#[test]
fn every_oracle_case_written_as_step_reads_back_in_freecad_as_the_solid_it_was() {
    let temp = tempfile::Builder::new()
        .prefix("silico-step-oracle-")
        .tempdir()
        .unwrap();
    let dir = temp.path();
    let mut kernel = KernelWorker::start().unwrap();
    let mut step = StepWorker::start().unwrap();
    let mut written = Vec::new();
    for case in load(Path::new(CASES)).unwrap() {
        let Ok(shape) = silico_oracle::silico::build(&mut kernel, &case.shape) else {
            // A case the kernel refuses is the oracle's to compare, not this test's.
            continue;
        };
        let file = step
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
            bounds: measured.bounds,
            center: measured.center,
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
        b = s.optimalBoundingBox(False, False)
        read = [s.Volume, s.Area, len(s.Faces), s.isValid(),
                [[b.XMin, b.YMin, b.ZMin], [b.XMax, b.YMax, b.ZMax]],
                list(s.CenterOfGravity) if s.Solids else None]
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
        if let Some([low, high]) = w.bounds {
            let diagonal = (0..3)
                .map(|i| (high[i] - low[i]) * (high[i] - low[i]))
                .sum::<f64>()
                .sqrt()
                .max(1.0);
            let off = |ours: f64, theirs: &serde_json::Value| {
                (ours - theirs.as_f64().unwrap()).abs() > PLACED * diagonal
            };
            let box_off =
                (0..3).any(|i| off(low[i], &read[4][0][i]) || off(high[i], &read[4][1][i]));
            let center_off = !read[5].is_null() && (0..3).any(|i| off(w.center[i], &read[5][i]));
            if box_off || center_off {
                wrong.push(format!(
                    "{}: at {low:?} to {high:?}, center {:?}, read back at {} center {}",
                    w.name, w.center, read[4], read[5]
                ));
            }
        }
        if case["labels"] != serde_json::json!([w.name]) {
            wrong.push(format!("{}: labelled {}", w.name, case["labels"]));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn two_parts_placed_apart_read_back_in_freecad_each_where_it_was() {
    // A box moved to (100, 0, 0) and a cylinder moved to (0, 50, 0), one assembly: each
    // solid read back where it was, its box by hand (the box from 100 to 110 in x, 0 to 20
    // in y, 0 to 30 in z; the cylinder of radius 3 and height 7 centered on (0, 50)), and
    // each product's name its object's label.
    let mut kernel = KernelWorker::start().unwrap();
    let mut step = StepWorker::start().unwrap();
    use silico_oracle::case::Shape;
    let moved = |shape: Shape, by: [f64; 3]| Shape::Translate {
        shape: Box::new(shape),
        by,
    };
    let block = silico_oracle::silico::build(
        &mut kernel,
        &moved(
            Shape::Block {
                dx: 10.0,
                dy: 20.0,
                dz: 30.0,
            },
            [100.0, 0.0, 0.0],
        ),
    )
    .unwrap();
    let pin = silico_oracle::silico::build(
        &mut kernel,
        &moved(
            Shape::Cylinder {
                radius: 3.0,
                height: 7.0,
            },
            [0.0, 50.0, 0.0],
        ),
    )
    .unwrap();
    let file = step
        .export_step(ExportStep {
            parts: vec![
                StepPart {
                    shape: block,
                    id: "k7f2qx3m".to_owned(),
                    name: "Block".to_owned(),
                },
                StepPart {
                    shape: pin,
                    id: "p0000001".to_owned(),
                    name: "Pin".to_owned(),
                },
            ],
            assembly: "part".to_owned(),
            file_name: "part.silico".to_owned(),
        })
        .unwrap();
    let temp = tempfile::Builder::new()
        .prefix("silico-step-placed-")
        .tempdir()
        .unwrap();
    let path = temp.path().join("two.step");
    std::fs::write(&path, file.step.as_bytes()).unwrap();
    let value = silico_oracle::freecad::script(&format!(
        r#"
import json, FreeCAD, Part, Import
s = Part.read({path:?})
boxes = []
for solid in s.Solids:
    b = solid.optimalBoundingBox(False, False)
    boxes.append([b.XMin, b.YMin, b.ZMin, b.XMax, b.YMax, b.ZMax])
doc = FreeCAD.newDocument("rt")
Import.insert({path:?}, doc.Name)
labels = sorted(o.Label for o in doc.Objects if o.TypeId == "Part::Feature")
print("SILICO_ORACLE " + json.dumps({{"boxes": sorted(boxes), "labels": labels}}))
"#,
        path = path.to_str().unwrap(),
    ))
    .unwrap();
    let boxes: Vec<Vec<f64>> = serde_json::from_value(value["boxes"].clone()).unwrap();
    let want = [
        [-3.0, 47.0, 0.0, 3.0, 53.0, 7.0],
        [100.0, 0.0, 0.0, 110.0, 20.0, 30.0],
    ];
    assert_eq!(boxes.len(), 2, "{value}");
    for (got, want) in boxes.iter().zip(want) {
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 1e-9, "{got:?} against {want:?}");
        }
    }
    assert_eq!(value["labels"], serde_json::json!(["Block", "Pin"]));
}
