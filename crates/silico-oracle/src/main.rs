//! `silico-oracle`: compares Silico with FreeCAD, case by case.
//!
//! - `silico-oracle run [DIR]`: the cases in DIR (default `tests/oracle`), its sketch
//!   cases (`sketches.silico`), each solved by Silico's sketch worker and by FreeCAD, and
//!   its sketch-feature cases (`sketch-features.silico`: pads, pockets, revolves, and
//!   grooves), the document rebuilt by Silico's recompute and each case built by FreeCAD's
//!   PartDesign, compared with each other and with the case's expected values; and its
//!   measure cases (`measures/`), pairs of elements of a document's solids measured by the
//!   core's `distance` and `angle` and by FreeCAD's Measure, compared by the decision's
//!   rules and with the case's values.
//! - `silico-oracle sweep [--count N] [--seed S]`: N generated cases (default 500) from seed
//!   S (default 1).
//! - `silico-oracle corpus DIR`: the STEP files in DIR, as FreeCAD reads them, each as it is
//!   and moved; Silico also meshes each (`pixi run corpus` fetches them first).
//!
//! Prints each case that differs, and exits 1 if any does. Needs the kernel worker beside
//! it (`pixi run oracle` and `pixi run oracle-sweep` build it), the sketch worker for sketch
//! cases (`pixi run oracle` builds it), and the FreeCAD oracle (`pixi install -e oracle`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use silico_oracle::{Case, corpus, generate, load, run, sketch};

const USAGE: &str = "usage: silico-oracle run [DIR] | silico-oracle sweep [--count N] [--seed S] | silico-oracle corpus DIR";

/// The most cases a sweep runs: a few hours' worth (500 take about 10 s), a few hundred
/// megabytes of cases.
const MAX_SWEEP: usize = 1_000_000;

fn main() -> ExitCode {
    let mut out = std::io::stdout().lock();
    let ran = cases().and_then(|(cases, corpus, sketches, features, measures)| {
        // A directory may hold sketch, sketch-feature, or measure cases alone.
        let others = !sketches.is_empty() || features.is_some() || measures.is_some();
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
        let feature_count = features.as_ref().map_or(0, |f| f.cases.len());
        if let Some(features) = &features {
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
        let (mut problems, mut notes) = (Vec::new(), Vec::new());
        let known;
        if let Some(CorpusNotes { unread, breps }) = corpus {
            problems = unread;
            problems.extend(corpus::meshes(&cases)?);
            let missing;
            (report.differences, known, missing) = corpus::without_known(
                report.differences,
                corpus::KNOWN,
                &corpus::platform(),
                &report.validity,
            );
            problems.extend(missing);
            // What each platform's FreeCAD wrote, so two platforms' runs can be compared.
            notes = breps;
        } else {
            // Generated cases' known differences, those of the cases this run made.
            let here: Vec<corpus::Known> = generate::KNOWN
                .iter()
                .filter(|k| cases.iter().any(|c| c.name == k.case))
                .copied()
                .collect();
            let missing;
            (report.differences, known, missing) = corpus::without_known(
                report.differences,
                &here,
                &corpus::platform(),
                &report.validity,
            );
            problems.extend(missing);
        }
        Ok((
            cases.len() + sketches.len() + feature_count + measure_count,
            report,
            problems,
            known,
            notes,
            sketch_differences,
        ))
    });
    match ran {
        Ok((count, report, problems, known, notes, sketch_differences)) => {
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

/// For the corpus: its files FreeCAD couldn't read, and the B-reps it wrote.
struct CorpusNotes {
    unread: Vec<String>,
    breps: Vec<String>,
}

/// The cases the command line asks for, for the corpus its files FreeCAD couldn't read and
/// the B-reps it wrote, the sketch cases, the sketch-feature cases, and the measure cases.
type Asked = (
    Vec<Case>,
    Option<CorpusNotes>,
    Vec<sketch::SketchCase>,
    Option<silico_oracle::features::Cases>,
    Option<silico_oracle::measures::Cases>,
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
                None,
                sketch::load(&dir)?,
                silico_oracle::features::load(&dir)?,
                silico_oracle::measures::load(&dir)?,
            ))
        }
        Some("corpus") => match &args[1..] {
            [dir] => {
                let corpus = corpus::cases(Path::new(dir))?;
                let notes = CorpusNotes {
                    unread: corpus.unread,
                    breps: corpus.breps,
                };
                Ok((corpus.cases, Some(notes), Vec::new(), None, None))
            }
            _ => Err(format!("corpus takes one directory\n{USAGE}")),
        },
        Some("sweep") => {
            let (mut count, mut seed) = (500usize, 1u64);
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
                    _ => return Err(format!("unknown argument {flag}\n{USAGE}")),
                }
            }
            if count > MAX_SWEEP {
                return Err(format!("--count {count}: at most {MAX_SWEEP}\n{USAGE}"));
            }
            Ok((generate::cases(seed, count), None, Vec::new(), None, None))
        }
        _ => Err(USAGE.to_owned()),
    }
}
