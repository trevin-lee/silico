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

/// The cases for every STEP file directly in `dir`, in name order, the files FreeCAD
/// couldn't read (with why), and each B-rep FreeCAD wrote, with its SHA-256 (a part's
/// B-rep is this platform's FreeCAD's reading of it, so two platforms' can differ). Each
/// file's B-rep is written beside it, in `dir/brep/`.
pub fn cases(dir: &Path) -> Result<Corpus, String> {
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
    let (mut cases, mut unread, mut breps) = (Vec::new(), Vec::new(), Vec::new());
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
                let bytes = std::fs::read(&brep)
                    .map_err(|e| format!("can't read {}: {e}", brep.display()))?;
                breps.push(format!("{stem}: B-rep SHA-256 {}", sha256(&bytes)));
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
    Ok(Corpus {
        cases,
        unread,
        breps,
    })
}

/// What [`cases`] found in a directory of STEP files.
pub struct Corpus {
    pub cases: Vec<Case>,
    /// The files FreeCAD couldn't read, with why.
    pub unread: Vec<String>,
    /// Each B-rep FreeCAD wrote, `name: B-rep SHA-256 hex`.
    pub breps: Vec<String>,
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The platform this runs on, as [`Known::platforms`] names it: the architecture and the
/// operating system (`aarch64-macos`, `x86_64-linux`).
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// A difference the corpus shows that's understood: the case, the difference as it's
/// written, why, and the platforms it shows on (a part's B-rep is the platform's FreeCAD's
/// reading of its STEP file, so a borderline check can fall either way by platform).
pub struct Known {
    pub case: &'static str,
    pub difference: &'static str,
    pub why: &'static str,
    pub platforms: &'static [&'static str],
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
    let mut kernel = KernelWorker::start().map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    for case in cases {
        // A worker lost building or measuring a part (the comparison reports that) is
        // replaced, so the next part isn't blamed for it.
        let shape = match silico::build(&mut kernel, &case.shape) {
            Ok(shape) => shape,
            Err(e) => {
                replace_if_lost(&mut kernel, &e)?;
                continue;
            }
        };
        let bounds = match kernel.measure_as_freecad(&shape) {
            Ok(m) => m.bounds,
            Err(e) => {
                replace_if_lost(&mut kernel, &e)?;
                continue;
            }
        };
        let size = bounds.map_or(1.0, |[low, high]| {
            (0..3).map(|k| high[k] - low[k]).fold(0.0, f64::max)
        });
        let deflection = size / 1000.0;
        if let Err(e) = kernel.tessellate(&shape, deflection) {
            found.push(format!(
                "{}: Silico can't mesh it at a deflection of {deflection}: {e}",
                case.name
            ));
            replace_if_lost(&mut kernel, &e)?;
        }
    }
    Ok(found)
}

/// A new worker in place of `kernel`, if `error` says it was lost.
fn replace_if_lost(kernel: &mut KernelWorker, error: &silico_core::Error) -> Result<(), String> {
    if silico::lost(error.code()) {
        *kernel = KernelWorker::start().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Differences the corpus shows that are understood, each with why and where: reported as
/// known, not failed, on the platforms they show on. A known difference that changes, goes
/// away, or shows on another platform, is reported as it is.
pub const KNOWN: &[Known] = &[Known {
    case: "nist_stc_07_asme1_ap242-e3.stp",
    difference: "valid: Silico false, FreeCAD true",
    why: "OCCT's analyzer finds four faces' wires unorientable in Silico's build of OCCT \
          7.9.3, and the part valid in FreeCAD's, reading the same B-rep: a borderline \
          numeric check, most likely the exact math Silico's build uses. On the Mac only: on \
          the x64 Linux runner, each reading that platform's FreeCAD's B-rep, they agree",
    platforms: &["aarch64-macos"],
}];

/// `differences` without those `known` lists on `platform`, the known ones found (in
/// words), and the known ones not found there, with what each side said of the case's
/// validity (`validity`, by case): a problem, since the list should say what the corpus
/// shows.
pub fn without_known(
    differences: Vec<Difference>,
    known: &[Known],
    platform: &str,
    validity: &[(String, Option<bool>, Option<bool>)],
) -> (Vec<Difference>, Vec<String>, Vec<String>) {
    let here: Vec<&Known> = known
        .iter()
        .filter(|k| k.platforms.contains(&platform))
        .collect();
    let mut found = Vec::new();
    let mut seen = vec![false; here.len()];
    let rest = differences
        .into_iter()
        .filter(|d| {
            let how = d.how.join("; ");
            match here
                .iter()
                .position(|k| k.case == d.case && k.difference == how)
            {
                Some(i) => {
                    seen[i] = true;
                    found.push(format!("{d} (known: {})", here[i].why));
                    false
                }
                None => true,
            }
        })
        .collect();
    let said =
        |valid: Option<bool>| valid.map_or("didn't build it".to_owned(), |v| format!("valid {v}"));
    let missing = here
        .iter()
        .zip(seen)
        .filter(|(_, seen)| !seen)
        .map(|(k, _)| {
            let verdicts = validity
                .iter()
                .find(|(case, _, _)| case == k.case)
                .map_or_else(
                    || "no such case".to_owned(),
                    |(_, ours, theirs)| {
                        format!("Silico {}, FreeCAD {}", said(*ours), said(*theirs))
                    },
                );
            format!(
                "{}: a known difference didn't show on {platform}: {} ({verdicts})",
                k.case, k.difference
            )
        })
        .collect();
    (rest, found, missing)
}

#[cfg(test)]
mod tests {
    use super::{Known, without_known};
    use crate::{Difference, Outcome};

    const WHY: &str = "why";

    fn known(platforms: &'static [&'static str]) -> [Known; 1] {
        [Known {
            case: "a",
            difference: "valid: Silico false, FreeCAD true",
            why: WHY,
            platforms,
        }]
    }

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
        let (rest, found, missing) = without_known(
            vec![
                difference("a", "valid: Silico false, FreeCAD true"),
                difference("a", "valid: Silico true, FreeCAD false"),
                difference("b", "valid: Silico false, FreeCAD true"),
            ],
            &known(&["here"]),
            "here",
            &[],
        );
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].how, ["valid: Silico true, FreeCAD false"]);
        assert_eq!(rest[1].case, "b");
        assert_eq!(
            found,
            [format!(
                "a: valid: Silico false, FreeCAD true (known: {WHY})"
            )]
        );
        assert!(missing.is_empty());
    }

    #[test]
    fn a_known_difference_that_didnt_show_is_a_problem() {
        let validity = [("a".to_owned(), Some(true), Some(true))];
        let (rest, found, missing) = without_known(
            vec![difference("b", "x")],
            &known(&["here"]),
            "here",
            &validity,
        );
        assert_eq!(rest.len(), 1);
        assert!(found.is_empty());
        assert_eq!(
            missing,
            [
                "a: a known difference didn't show on here: valid: Silico false, FreeCAD true (Silico valid true, FreeCAD valid true)"
            ]
        );
    }

    #[test]
    fn a_known_difference_is_expected_only_where_it_shows() {
        // Elsewhere, it's not looked for: absent, nothing's missing; shown, it's a difference.
        let (rest, found, missing) = without_known(Vec::new(), &known(&["there"]), "here", &[]);
        assert!(rest.is_empty() && found.is_empty() && missing.is_empty());
        let (rest, found, missing) = without_known(
            vec![difference("a", "valid: Silico false, FreeCAD true")],
            &known(&["there"]),
            "here",
            &[],
        );
        assert_eq!(rest.len(), 1);
        assert!(found.is_empty() && missing.is_empty());
    }

    #[test]
    fn a_known_difference_on_a_case_that_didnt_build_says_so() {
        let validity = [("a".to_owned(), None, Some(true))];
        let (_, _, missing) = without_known(Vec::new(), &known(&["here"]), "here", &validity);
        assert_eq!(
            missing,
            [
                "a: a known difference didn't show on here: valid: Silico false, FreeCAD true (Silico didn't build it, FreeCAD valid true)"
            ]
        );
    }

    #[test]
    fn this_platform_is_named_as_known_differences_name_it() {
        assert_eq!(
            super::platform(),
            format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
        );
        #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
        assert_eq!(super::platform(), "aarch64-macos");
    }
}
