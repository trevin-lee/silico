//! `silico-oracle`: compares Silico with FreeCAD, case by case.
//!
//! - `silico-oracle run [DIR]`: the cases in DIR (default `tests/oracle`).
//! - `silico-oracle sweep [--count N] [--seed S]`: N generated cases (default 500) from seed
//!   S (default 1).
//!
//! Prints each case that differs, and exits 1 if any does. Needs the kernel worker beside
//! it (`pixi run oracle` and `pixi run oracle-sweep` build it) and the FreeCAD oracle
//! (`pixi install -e oracle`).

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use silico_oracle::{Case, generate, load, run};

const USAGE: &str = "usage: silico-oracle run [DIR] | silico-oracle sweep [--count N] [--seed S]";

/// The most cases a sweep runs: a few hours' worth (500 take about 10 s), a few hundred
/// megabytes of cases.
const MAX_SWEEP: usize = 1_000_000;

fn main() -> ExitCode {
    let mut out = std::io::stdout().lock();
    match cases().and_then(|cases| Ok((cases.len(), run(&cases)?))) {
        Ok((count, report)) => {
            for d in &report.differences {
                let _ = writeln!(out, "{d}");
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
            if found == 0 {
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

/// The cases the command line asks for.
fn cases() -> Result<Vec<Case>, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => {
            if args.len() > 2 {
                return Err(format!("run takes one directory\n{USAGE}"));
            }
            let dir = args
                .get(1)
                .map_or_else(|| PathBuf::from("tests/oracle"), PathBuf::from);
            load(&dir)
        }
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
            Ok(generate::cases(seed, count))
        }
        _ => Err(USAGE.to_owned()),
    }
}
