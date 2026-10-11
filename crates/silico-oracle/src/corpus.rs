//! The corpus of real parts (`pixi run corpus`, docs/roadmap/m1-kernel.md, M1.9): STEP
//! files read by FreeCAD and written as OCCT text B-reps, each then a case for each side
//! to read and measure, as it is and moved. Real geometry (splines, small features, loose
//! tolerances) is what the generated cases lack.

use std::path::{Path, PathBuf};

use silico_core::kernel::KernelWorker;

use crate::Difference;
use crate::case::{Case, Shape};
use crate::freecad;
use crate::silico;

/// The cases for every STEP file directly in `dir`, in name order, and the files FreeCAD
/// couldn't read (with why). Each file's B-rep is written beside it, in `dir/brep/`.
pub fn cases(dir: &Path) -> Result<(Vec<Case>, Vec<String>), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("can't read {}: {e}", dir.display()))?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "stp" | "step"))
        })
        .collect();
    files.sort();
    let out = dir.join("brep");
    std::fs::create_dir_all(&out).map_err(|e| format!("can't make {}: {e}", out.display()))?;
    let (mut cases, mut unread) = (Vec::new(), Vec::new());
    for file in files {
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("{} has no name", file.display()))?
            .to_owned();
        let brep = out.join(format!("{stem}.brep"));
        let jobs = serde_json::to_string(&[&file, &brep])
            .map_err(|e| format!("can't write the paths: {e}"))?;
        let script = format!(
            r#"
import json, Part
source, target = json.loads({jobs:?})
try:
    s = Part.Shape()
    s.read(source)
    if s.isNull() or not s.Solids:
        raise RuntimeError("it has no solid")
    s.exportBrep(target)
    out = None
except Exception as e:
    out = str(e) or type(e).__name__
print("SILICO_ORACLE " + json.dumps(out))
"#
        );
        match freecad::script(&script)? {
            serde_json::Value::Null => {
                let path = brep.to_string_lossy().into_owned();
                let part = Shape::Brep { path };
                cases.push(case(&stem, part.clone()));
                cases.push(case(
                    &format!("{stem}_moved"),
                    Shape::Translate {
                        shape: Box::new(part),
                        by: [13.7, -4.2, 9.1],
                    },
                ));
            }
            why => unread.push(format!("{stem}: FreeCAD can't read it: {why}")),
        }
    }
    Ok((cases, unread))
}

fn case(name: &str, shape: Shape) -> Case {
    Case {
        name: name.to_owned(),
        note: "a real part, from the corpus".to_owned(),
        boundary: None,
        shape,
    }
}

/// Every case Silico can't mesh (deflection a thousandth of the part's largest side), and
/// why: a mesh Silico refuses is worth a look, and a worker lost meshing is a bug.
pub fn meshes(cases: &[Case]) -> Result<Vec<String>, String> {
    let start = || KernelWorker::start().map_err(|e| e.to_string());
    let mut kernel = start()?;
    let mut found = Vec::new();
    for case in cases {
        let Ok(shape) = silico::build(&mut kernel, &case.shape) else {
            continue;
        };
        let size = kernel
            .measure_as_freecad(&shape)
            .ok()
            .and_then(|m| m.bounds)
            .map_or(1.0, |[low, high]| {
                (0..3).map(|k| high[k] - low[k]).fold(0.0, f64::max)
            });
        if let Err(e) = kernel.tessellate(&shape, size / 1000.0) {
            let lost = silico::lost(e.code());
            found.push(format!("{}: Silico can't mesh it: {e}", case.name));
            if lost {
                kernel = start()?;
            }
        }
    }
    Ok(found)
}

/// Differences the corpus shows that are understood, each with why: reported as known, not
/// failed. A known difference that changes, or goes away, is reported as it is.
pub const KNOWN: &[(&str, &str, &str)] = &[(
    "nist_stc_07_asme1_ap242-e3",
    "valid: Silico false, FreeCAD true",
    "OCCT's analyzer finds four faces' wires unorientable in Silico's build of OCCT 7.9.3, \
     and the part valid in FreeCAD's, reading the same B-rep: a borderline numeric check, \
     most likely the exact math Silico's build uses",
)];

/// `differences` without the [`KNOWN`] ones, and the known ones found, in words.
pub fn without_known(differences: Vec<Difference>) -> (Vec<Difference>, Vec<String>) {
    let mut known = Vec::new();
    let rest = differences
        .into_iter()
        .filter(|d| {
            let how = d.how.join("; ");
            match KNOWN
                .iter()
                .find(|(case, what, _)| *case == d.case && *what == how)
            {
                Some((_, _, why)) => {
                    known.push(format!("{d} (known: {why})"));
                    false
                }
                None => true,
            }
        })
        .collect();
    (rest, known)
}
