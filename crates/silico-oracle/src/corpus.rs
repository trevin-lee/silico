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
        // Named for the whole file name, so `a.stp` and `a.step` don't share a case.
        let stem = file
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("{} has no name", file.display()))?
            .to_owned();
        let brep = out.join(format!("{stem}.brep"));
        // The paths reach Python as JSON in a string literal (whose escapes JSON's are):
        // any name is passed as it is.
        let jobs = serde_json::to_string(&[&file, &brep])
            .and_then(|json| serde_json::to_string(&json))
            .map_err(|e| format!("can't write the paths: {e}"))?;
        let script = format!(
            r#"
import json, Part
source, target = json.loads({jobs})
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
        // A worker lost building or measuring a part (the comparison reports that) is
        // replaced, so the next part isn't blamed for it.
        let shape = match silico::build(&mut kernel, &case.shape) {
            Ok(shape) => shape,
            Err(e) => {
                if silico::lost(e.code()) {
                    kernel = start()?;
                }
                continue;
            }
        };
        let bounds = match kernel.measure_as_freecad(&shape) {
            Ok(m) => m.bounds,
            Err(e) => {
                if silico::lost(e.code()) {
                    kernel = start()?;
                }
                continue;
            }
        };
        let size = bounds.map_or(1.0, |[low, high]| {
            (0..3).map(|k| high[k] - low[k]).fold(0.0, f64::max)
        });
        let deflection = size / 1000.0;
        if let Err(e) = kernel.tessellate(&shape, deflection) {
            let lost = silico::lost(e.code());
            found.push(format!(
                "{}: Silico can't mesh it at a deflection of {deflection}: {e}",
                case.name
            ));
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
    "nist_stc_07_asme1_ap242-e3.stp",
    "valid: Silico false, FreeCAD true",
    "OCCT's analyzer finds four faces' wires unorientable in Silico's build of OCCT 7.9.3, \
     and the part valid in FreeCAD's, reading the same B-rep: a borderline numeric check, \
     most likely the exact math Silico's build uses",
)];

/// `differences` without those `known` lists, the known ones found (in words), and the
/// known ones not found: a problem, since the list should say what the corpus shows.
pub fn without_known(
    differences: Vec<Difference>,
    known: &[(&str, &str, &str)],
) -> (Vec<Difference>, Vec<String>, Vec<String>) {
    let mut found = Vec::new();
    let mut seen = vec![false; known.len()];
    let rest = differences
        .into_iter()
        .filter(|d| {
            let how = d.how.join("; ");
            match known
                .iter()
                .position(|(case, what, _)| *case == d.case && *what == how)
            {
                Some(i) => {
                    seen[i] = true;
                    found.push(format!("{d} (known: {})", known[i].2));
                    false
                }
                None => true,
            }
        })
        .collect();
    let missing = known
        .iter()
        .zip(seen)
        .filter(|(_, seen)| !seen)
        .map(|((case, what, _), _)| format!("{case}: a known difference didn't show: {what}"))
        .collect();
    (rest, found, missing)
}

#[cfg(test)]
mod tests {
    use super::without_known;
    use crate::{Difference, Outcome};

    fn difference(case: &str, how: &str) -> Difference {
        Difference {
            case: case.to_owned(),
            ours: Outcome::Failed(String::new()),
            theirs: Outcome::Failed(String::new()),
            how: vec![how.to_owned()],
        }
    }

    #[test]
    fn only_a_known_difference_exactly_as_listed_is_set_aside() {
        let known = [("a", "valid: Silico false, FreeCAD true", "why")];
        let (rest, found, missing) = without_known(
            vec![
                difference("a", "valid: Silico false, FreeCAD true"),
                difference("a", "valid: Silico true, FreeCAD false"),
                difference("b", "valid: Silico false, FreeCAD true"),
            ],
            &known,
        );
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].how, ["valid: Silico true, FreeCAD false"]);
        assert_eq!(rest[1].case, "b");
        assert_eq!(found, ["a: valid: Silico false, FreeCAD true (known: why)"]);
        assert!(missing.is_empty());
    }

    #[test]
    fn a_known_difference_that_didnt_show_is_a_problem() {
        let known = [("a", "valid: Silico false, FreeCAD true", "why")];
        let (rest, found, missing) = without_known(vec![difference("b", "x")], &known);
        assert_eq!(rest.len(), 1);
        assert!(found.is_empty());
        assert_eq!(
            missing,
            ["a: a known difference didn't show: valid: Silico false, FreeCAD true"]
        );
    }
}
