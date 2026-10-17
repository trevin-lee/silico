//! `silico-oracle`: compares Silico with FreeCAD, case by case.
//!
//! - `silico-oracle run [DIR]`: the cases in DIR (default `tests/oracle`).
//! - `silico-oracle sweep [--count N] [--seed S]`: N generated cases (default 500) from seed
//!   S (default 1).
//! - `silico-oracle corpus DIR`: the STEP files in DIR, as FreeCAD reads them, each as it is
//!   and moved; Silico also meshes each (`pixi run corpus` fetches them first).
//!
//! Prints each case that differs, and exits 1 if any does. Needs the kernel worker beside
//! it (`pixi run oracle` and `pixi run oracle-sweep` build it) and the FreeCAD oracle
//! (`pixi install -e oracle`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use silico_oracle::{Case, corpus, generate, load, run};

const USAGE: &str = "usage: silico-oracle run [DIR] | silico-oracle sweep [--count N] [--seed S] | silico-oracle corpus DIR";

/// The most cases a sweep runs: a few hours' worth (500 take about 10 s), a few hundred
/// megabytes of cases.
const MAX_SWEEP: usize = 1_000_000;

fn main() -> ExitCode {
    let mut out = std::io::stdout().lock();
    let ran = cases().and_then(|(cases, corpus)| {
        let mut report = run(&cases)?;
        let (mut problems, mut known) = (Vec::new(), Vec::new());
        if let Some((unread, breps)) = corpus {
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
            known.extend(breps);
        }
        Ok((cases.len(), report, problems, known))
    });
    match ran {
        Ok((count, report, problems, known)) => {
            for d in &report.differences {
                let _ = writeln!(out, "{d}");
            }
            for k in &known {
                let _ = writeln!(out, "{k}");
            }
            for p in &problems {
                let _ = writeln!(out, "{p}");
            }
            let found = report.differences.len();
            let _ = match report.refused.len() {
                0 => writeln!(out, "{found} of {count} cases differ from FreeCAD"),
                refused => writeln!(
                    out,
                    "{found} of {count} cases differ from FreeCAD; both refused {refused}: {}",
                    report.refused.join(", ")
                ),
            };
            if !problems.is_empty() {
                let _ = writeln!(out, "{} more problems with the corpus", problems.len());
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
type CorpusNotes = (Vec<String>, Vec<String>);

/// The cases the command line asks for, and for the corpus, its files FreeCAD couldn't
/// read and the B-reps it wrote.
fn cases() -> Result<(Vec<Case>, Option<CorpusNotes>), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => {
            if args.len() > 2 {
                return Err(format!("run takes one directory\n{USAGE}"));
            }
            let dir = args
                .get(1)
                .map_or_else(|| PathBuf::from("tests/oracle"), PathBuf::from);
            Ok((load(&dir)?, None))
        }
        Some("corpus") => match &args[1..] {
            [dir] => {
                let corpus = corpus::cases(Path::new(dir))?;
                Ok((corpus.cases, Some((corpus.unread, corpus.breps))))
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
            Ok((generate::cases(seed, count), None))
        }
        _ => Err(USAGE.to_owned()),
    }
}
