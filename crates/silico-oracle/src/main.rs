//! `silico-oracle`: compares Silico with FreeCAD, case by case.
//!
//! - `silico-oracle run [DIR]`: the cases in DIR (default `tests/oracle`), its sketch
//!   cases (`sketches.silico`), each solved by Silico's sketch worker and by FreeCAD, and
//!   its sketch-feature cases (`sketch-features.silico`: pads, pockets, revolves, and
//!   grooves) and pattern cases (`patterns.silico`: linear patterns, polar patterns, and
//!   mirrors), each document rebuilt by Silico's recompute and each case built by FreeCAD's
//!   PartDesign, compared with each other and with the case's expected values; and its
//!   measure cases (`measures/`), pairs of elements of a document's solids measured by the
//!   core's `distance` and `angle` and by FreeCAD's Measure, compared by the decision's
//!   rules and with the case's values.
//!   And its sketch-edit cases (`sketch-edits/`), each a sketch and one edit, made by
//!   Silico's `sketch_edit` and by FreeCAD's Sketcher, solved by each, and compared modulo
//!   what docs/ports/sketch-edit.md says Silico does otherwise.
//! - `silico-oracle sweep [--count N] [--seed S]`: N generated cases (default 500) from seed
//!   S (default 1).
//! - `silico-oracle sketch-edit-sweep [--count N] [--seed S]`: N generated sketch-edit cases
//!   (default 500) from seed S (default 1): random sketches, each with a random edit.
//!   `--only INDEX` takes the one case of them at INDEX (from 0); `--dump DIR` writes each
//!   case taken to DIR as `<name>.json`: its sketch, its edit, and Silico's edited sketch
//!   ([`sketch_edit::dump`]).
//! - `silico-oracle sketch-extend-sweep`, with the same options: the same random sketches,
//!   each with one of its lines or arcs extended (`silico_core::extend`).
//!
//!   In either sketch sweep, a case that differs is run through FreeCAD twice more, each
//!   alone in a FreeCAD of its own ([`sketch_edit::rerun`]): where FreeCAD's runs disagree,
//!   only the differences every run of it gives are reported, the rest listed (`varies:`)
//!   with what of FreeCAD's answer varied, and the cases it varied on counted.
//! - `silico-oracle corpus DIR`: the STEP files in DIR, as FreeCAD reads them, each as it is
//!   and moved; Silico also meshes each (`pixi run corpus` fetches them first).
//!
//! Prints each case that differs, and exits 1 if any does. Needs the kernel worker beside
//! it (`pixi run oracle` and `pixi run oracle-sweep` build it), the sketch worker for sketch
//! and sketch-edit cases (`pixi run oracle` and `pixi run oracle-sketch-edit-sweep` build it),
//! and the FreeCAD oracle (`pixi install -e oracle`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use silico_oracle::{Case, corpus, generate, load, run, sketch, sketch_edit};

const USAGE: &str = "usage: silico-oracle run [DIR] | silico-oracle sweep [--count N] [--seed S] | silico-oracle sketch-edit-sweep [--count N] [--seed S] [--only INDEX] [--dump DIR] | silico-oracle sketch-extend-sweep [the same] | silico-oracle corpus DIR";

/// The most cases a sweep runs: a few hours' worth (500 take about 10 s), a few hundred
/// megabytes of cases.
const MAX_SWEEP: usize = 1_000_000;

fn main() -> ExitCode {
    let mut out = std::io::stdout().lock();
    let ran = cases().and_then(|(cases, source, sketches, features, measures, edits)| {
        // A directory may hold sketch, sketch-feature, measure, or sketch-edit cases alone.
        let others =
            !sketches.is_empty() || !features.is_empty() || measures.is_some() || !edits.is_empty();
        let mut report = if cases.is_empty() && others {
            silico_oracle::Report::default()
        } else {
            run(&cases)?
        };
        let mut sketch_differences = if sketches.is_empty() {
            Vec::new()
        } else {
            sketch::run(&sketches)?
                .into_iter()
                .map(|d| d.to_string())
                .collect()
        };
        let feature_count: usize = features.iter().map(|f| f.cases.len()).sum();
        for features in &features {
            sketch_differences.extend(
                silico_oracle::features::run(features)?
                    .into_iter()
                    .map(|d| d.to_string()),
            );
        }
        let measure_count = measures.as_ref().map_or(0, |m| m.cases.len());
        if let Some(measures) = &measures {
            sketch_differences.extend(
                silico_oracle::measures::run(measures)?
                    .into_iter()
                    .filter(|c| !c.how.is_empty())
                    .map(|c| c.to_string()),
            );
        }
        // A sweep's differing cases are run through FreeCAD again, alone, and judged against
        // every run (`sketch_edit::rerun`); the hand cases aren't.
        let (verdicts, reruns) = if matches!(source, Source::Sweep) {
            sketch_edit::rerun::run_rerunning(&edits)?
        } else {
            (sketch_edit::run(&edits)?, Vec::new())
        };
        let mut excused: Vec<String> = verdicts
            .iter()
            .flat_map(|v| {
                v.excused
                    .iter()
                    .map(move |why| format!("excused: {}: {why}", v.case))
            })
            .collect();
        let (differ, shown, stale) = sketch_edit::without_known(verdicts, sketch_edit::KNOWN);
        let varying = varying(&reruns, &differ);
        sketch_differences.extend(differ.iter().map(ToString::to_string));
        excused.extend(shown);
        let (mut problems, mut notes, known) = without_known(source, &cases, &mut report)?;
        notes.extend(excused);
        notes.extend(reruns.iter().map(ToString::to_string));
        problems.extend(stale);
        Ok((
            cases.len() + sketches.len() + feature_count + measure_count + edits.len(),
            report,
            problems,
            known,
            notes,
            sketch_differences,
            varying,
        ))
    });
    match ran {
        Ok((count, report, problems, known, notes, sketch_differences, varying)) => {
            for d in &report.differences {
                let _ = writeln!(out, "{d}");
            }
            for d in &sketch_differences {
                let _ = writeln!(out, "{d}");
            }
            for k in &known {
                let _ = writeln!(out, "{k}");
            }
            for n in &notes {
                let _ = writeln!(out, "{n}");
            }
            for p in &problems {
                let _ = writeln!(out, "{p}");
            }
            let found = report.differences.len() + sketch_differences.len();
            let _ = match report.refused.len() {
                0 => writeln!(out, "{found} of {count} cases differ from FreeCAD"),
                refused => writeln!(
                    out,
                    "{found} of {count} cases differ from FreeCAD; both refused {refused}: {}",
                    report.refused.join(", ")
                ),
            };
            if let Some(said) = varying_said(&varying) {
                let _ = writeln!(out, "{said}");
            }
            if !problems.is_empty() {
                let _ = writeln!(out, "{} more problems with the cases", problems.len());
            }
            if found == 0 && problems.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr().lock(), "silico-oracle: {e}");
            ExitCode::from(2)
        }
    }
}

/// The cases whose runs of FreeCAD varied, by name, in the sweep's order: each marked
/// "(still differs)" where it's among the cases that `differ`, a difference every run
/// reproduces left and no known case's.
fn varying(reruns: &[sketch_edit::rerun::Rerun], differ: &[sketch_edit::Verdict]) -> Vec<String> {
    reruns
        .iter()
        .filter(|r| matches!(r.reran, sketch_edit::rerun::Reran::Varied { .. }))
        .map(|r| {
            let differs = differ.iter().any(|v| v.case == r.case);
            if differs {
                format!("{} (still differs)", r.case)
            } else {
                r.case.clone()
            }
        })
        .collect()
}

/// The summary's line of the cases FreeCAD's runs varied on, if any did.
fn varying_said(varying: &[String]) -> Option<String> {
    (!varying.is_empty()).then(|| {
        format!(
            "FreeCAD's answer varied run to run on {}, which differ only where every run of it reproduces the difference: {}",
            varying.len(),
            varying.join(", ")
        )
    })
}

/// What's said of a run's differences beside them: its problems, its notes, and the known
/// differences that showed.
type Said = (Vec<String>, Vec<String>, Vec<String>);

/// `report` less the differences known for cases from `source`, and what's said of them:
/// the problems (a known difference that didn't show, and the corpus's files FreeCAD
/// couldn't read and meshes Silico couldn't make), the notes (the B-reps the corpus's
/// FreeCAD wrote), and each known difference that showed.
fn without_known(
    source: Source,
    cases: &[Case],
    report: &mut silico_oracle::Report,
) -> Result<Said, String> {
    let differences = std::mem::take(&mut report.differences);
    match source {
        Source::Corpus(CorpusNotes { unread, breps }) => {
            let mut problems = unread;
            problems.extend(corpus::meshes(cases)?);
            let (rest, known, missing) = corpus::without_known(
                differences,
                corpus::KNOWN,
                &corpus::platform(),
                &report.validity,
            );
            report.differences = rest;
            problems.extend(missing);
            // What each platform's FreeCAD wrote, so two platforms' runs can be compared.
            Ok((problems, breps, known))
        }
        // Generated cases' known differences, those of the cases this run made, and
        // FreeCAD's no solid where Silico refuses it.
        Source::Sweep => {
            let (rest, known, missing) =
                silico_oracle::sweep_without_known(differences, cases, &report.validity);
            report.differences = rest;
            Ok((missing, Vec::new(), known))
        }
        // The hand-written cases: every difference reported as it is.
        Source::Hand => {
            report.differences = differences;
            Ok((Vec::new(), Vec::new(), Vec::new()))
        }
    }
}

/// For the corpus: its files FreeCAD couldn't read, and the B-reps it wrote.
struct CorpusNotes {
    unread: Vec<String>,
    breps: Vec<String>,
}

/// Where a run's cases come from, which says what of their differences is known.
enum Source {
    /// A directory's hand-written cases (`run`).
    Hand,
    /// A generated sweep (`sweep`, and the sketch sweeps, which have no shape cases).
    Sweep,
    /// The corpus's STEP files.
    Corpus(CorpusNotes),
}

/// The cases the command line asks for, where they come from (for the corpus its files
/// FreeCAD couldn't read and the B-reps it wrote), the sketch cases, the sketch-feature and
/// pattern cases, the measure cases, and the sketch-edit cases.
type Asked = (
    Vec<Case>,
    Source,
    Vec<sketch::SketchCase>,
    Vec<silico_oracle::features::Cases>,
    Option<silico_oracle::measures::Cases>,
    Vec<sketch_edit::EditCase>,
);

fn cases() -> Result<Asked, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => {
            if args.len() > 2 {
                return Err(format!("run takes one directory\n{USAGE}"));
            }
            let dir = args
                .get(1)
                .map_or_else(|| PathBuf::from("tests/oracle"), PathBuf::from);
            Ok((
                load(&dir)?,
                Source::Hand,
                sketch::load(&dir)?,
                silico_oracle::features::load(&dir)?
                    .into_iter()
                    .chain(silico_oracle::features::load_patterns(&dir)?)
                    .collect(),
                silico_oracle::measures::load(&dir)?,
                sketch_edit::load(&dir)?,
            ))
        }
        Some("corpus") => match &args[1..] {
            [dir] => {
                let corpus = corpus::cases(Path::new(dir))?;
                let notes = CorpusNotes {
                    unread: corpus.unread,
                    breps: corpus.breps,
                };
                Ok((
                    corpus.cases,
                    Source::Corpus(notes),
                    Vec::new(),
                    Vec::new(),
                    None,
                    Vec::new(),
                ))
            }
            _ => Err(format!("corpus takes one directory\n{USAGE}")),
        },
        Some(sweep @ ("sweep" | "sketch-edit-sweep" | "sketch-extend-sweep")) => {
            let extends = sweep == "sketch-extend-sweep";
            let edits = sweep == "sketch-edit-sweep" || extends;
            let (mut count, mut seed) = (500usize, 1u64);
            let (mut dump, mut only): (Option<PathBuf>, Option<usize>) = (None, None);
            let mut rest = args[1..].iter();
            while let Some(flag) = rest.next() {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?;
                match flag.as_str() {
                    "--count" => {
                        count = value
                            .parse()
                            .map_err(|e| format!("--count {value}: {e}\n{USAGE}"))?;
                    }
                    "--seed" => {
                        seed = value
                            .parse()
                            .map_err(|e| format!("--seed {value}: {e}\n{USAGE}"))?;
                    }
                    "--dump" if edits => dump = Some(PathBuf::from(value)),
                    "--only" if edits => {
                        only = Some(
                            value
                                .parse()
                                .map_err(|e| format!("--only {value}: {e}\n{USAGE}"))?,
                        );
                    }
                    _ => return Err(format!("unknown argument {flag}\n{USAGE}")),
                }
            }
            if count > MAX_SWEEP {
                return Err(format!("--count {count}: at most {MAX_SWEEP}\n{USAGE}"));
            }
            if edits {
                let mut cases = if extends {
                    generate::sketch_extend_cases(seed, count)?
                } else {
                    generate::sketch_edit_cases(seed, count)?
                };
                if let Some(index) = only {
                    if index >= cases.len() {
                        return Err(format!(
                            "--only {index}: the sweep makes cases 0 to {}\n{USAGE}",
                            cases.len().saturating_sub(1)
                        ));
                    }
                    cases = vec![cases.swap_remove(index)];
                }
                if let Some(dir) = &dump {
                    let mut worker =
                        silico_core::sketch::SketchWorker::start().map_err(|e| e.to_string())?;
                    for case in &cases {
                        sketch_edit::dump(case, dir, &mut worker)?;
                    }
                }
                return Ok((
                    Vec::new(),
                    Source::Sweep,
                    Vec::new(),
                    Vec::new(),
                    None,
                    cases,
                ));
            }
            Ok((
                generate::cases(seed, count),
                Source::Sweep,
                Vec::new(),
                Vec::new(),
                None,
                Vec::new(),
            ))
        }
        _ => Err(USAGE.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use silico_oracle::{Difference, Measured, Outcome, Report, Shape, Validity};

    use silico_oracle::sketch_edit::Verdict;
    use silico_oracle::sketch_edit::rerun::{Reran, Rerun};

    use super::{Case, Source, varying, varying_said, without_known};

    #[test]
    fn the_cases_freecad_varied_on_are_named_and_counted_each_still_differing_marked() {
        // Four rerun cases: two varied, one of them among the cases still differing; one
        // alike, though it differs; one not run again.
        let rerun = |case: &str, reran: Reran| Rerun {
            case: case.to_owned(),
            runs: 3,
            reran,
        };
        let varied = || Reran::Varied {
            varied: vec!["its constraints".to_owned()],
            uncompared: Vec::new(),
        };
        let reruns = [
            rerun("edit_1_2", varied()),
            rerun("edit_1_3", Reran::Alike),
            rerun("edit_1_4", varied()),
            rerun("edit_1_5", Reran::Unjudged("not run again".to_owned())),
        ];
        let differ = |case: &str| Verdict {
            case: case.to_owned(),
            how: vec!["a difference".to_owned()],
            ..Verdict::default()
        };
        let named = varying(&reruns, &[differ("edit_1_3"), differ("edit_1_4")]);
        assert_eq!(named, ["edit_1_2", "edit_1_4 (still differs)"]);
        assert_eq!(
            varying_said(&named).as_deref(),
            Some(
                "FreeCAD's answer varied run to run on 2, which differ only where every run of it reproduces the difference: edit_1_2, edit_1_4 (still differs)"
            )
        );
        assert_eq!(varying_said(&[]), None);
    }

    /// generated_111_374 as the seed-111 sweep found it (FreeCAD's cut turned inside out,
    /// Silico's refusal of it as invalid), in a report of its own.
    fn inside_out() -> (Vec<Case>, Report) {
        let torus = |major_radius, minor_radius| Shape::Torus {
            major_radius,
            minor_radius,
        };
        let case = Case {
            name: "generated_111_374".to_owned(),
            note: "generated".to_owned(),
            boundary: None,
            shape: Shape::Cut {
                base: Box::new(torus(20.0, 9.0)),
                tool: Box::new(Shape::Translate {
                    shape: Box::new(torus(18.0, 8.0)),
                    by: [1.0, 2.0, 3.0],
                }),
            },
        };
        let difference = Difference {
            case: case.name.clone(),
            ours: Outcome::Failed(
                "invalid_result: Cut gave an invalid result, which was refused: the cut's \
                 result isn't a valid solid."
                    .to_owned(),
            ),
            theirs: Outcome::Built(Measured {
                volume: -6480.640578694009,
                area: 8873.33,
                bounds: Some([[-30.0; 3], [30.0; 3]]),
                center: Some([0.0; 3]),
                faces: 4,
                edges: 6,
                vertices: 2,
                valid: true,
            }),
            how: vec!["FreeCAD built it; Silico failed: ...".to_owned()],
        };
        let report = Report {
            differences: vec![difference],
            refused: Vec::new(),
            validity: vec![Validity {
                case: case.name.clone(),
                silico: None,
                freecad: Some(true),
            }],
        };
        (vec![case], report)
    }

    #[test]
    fn freecads_no_solid_is_known_in_a_generated_sweep_alone() {
        // A sweep's: known, said as such, no problem.
        let (cases, mut report) = inside_out();
        let (problems, notes, known) = without_known(Source::Sweep, &cases, &mut report).unwrap();
        assert!(report.differences.is_empty(), "{:?}", report.differences);
        assert!(
            problems.is_empty() && notes.is_empty(),
            "{problems:?} {notes:?}"
        );
        assert_eq!(known.len(), 1, "{known:?}");
        assert!(
            known[0].starts_with("generated_111_374: ")
                && known[0].contains("FreeCAD's result is no solid"),
            "{known:?}"
        );
        // The same case among the hand-written ones: a difference, reported as it is.
        let (cases, mut report) = inside_out();
        let before = report.differences.clone();
        let (problems, notes, known) = without_known(Source::Hand, &cases, &mut report).unwrap();
        assert_eq!(report.differences, before);
        assert!(
            problems.is_empty() && notes.is_empty() && known.is_empty(),
            "{problems:?} {notes:?} {known:?}"
        );
    }
}
