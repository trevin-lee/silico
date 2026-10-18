//! The FreeCAD oracle (ARCHITECTURE.md, Testing). A [`Case`] says how to build a shape from
//! Silico's operations. The harness builds it through Silico's kernel workers, and through
//! headless FreeCAD (1.1.3, on the same OCCT release) with the calls Silico's port notes
//! name, and compares what each measures: volume, area, bounding box, center of mass,
//! topology counts, and validity (see [`compare`]). A case that one side refuses and the other builds is a
//! difference too. The hand-written cases live in `tests/oracle/`, which is protected;
//! [`generate`] makes random valid ones.

pub mod case;
pub mod compare;
pub mod corpus;
pub mod freecad;
pub mod generate;
pub mod silico;
pub mod sketch;

pub use case::{Case, Chamfer, Shape, load};
pub use compare::{Measured, Outcome, Tolerance, differences};

/// A case whose two outcomes differ, and how.
#[derive(Debug, Clone, PartialEq)]
pub struct Difference {
    pub case: String,
    pub ours: Outcome,
    pub theirs: Outcome,
    pub how: Vec<String>,
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.case, self.how.join("; "))
    }
}

/// What Silico (a kernel worker, started here) and FreeCAD each made of every case, in
/// order: (Silico's, FreeCAD's).
pub fn outcomes(cases: &[Case]) -> Result<Vec<(Outcome, Outcome)>, String> {
    if cases.is_empty() {
        return Err("there are no cases to compare".to_owned());
    }
    // Silico's worker first: a missing one is found before FreeCAD's run, not after.
    let start = || silico_core::kernel::KernelWorker::start().map_err(|e| e.to_string());
    let mut kernel = start()?;
    let theirs = freecad::run(cases)?;
    if theirs.len() != cases.len() {
        return Err(format!(
            "FreeCAD's side gave {} outcomes for {} cases",
            theirs.len(),
            cases.len()
        ));
    }
    let mut found = Vec::with_capacity(cases.len());
    for (case, theirs) in cases.iter().zip(theirs) {
        let ours = silico::measure(&mut kernel, &case.shape);
        if matches!(ours, Outcome::Lost(_)) {
            kernel = start()?;
        }
        found.push((ours, theirs));
    }
    Ok(found)
}

/// What a run of cases found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Report {
    /// The cases whose outcomes differ.
    pub differences: Vec<Difference>,
    /// The cases both sides refused: they agree, but nothing about the shape was compared.
    pub refused: Vec<String>,
    /// Whether each side found each case's shape valid, in case order: what a known
    /// difference that didn't show is reported with.
    pub validity: Vec<Validity>,
}

/// Whether each side found a case's shape valid: none for a side that didn't build it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validity {
    pub case: String,
    pub silico: Option<bool>,
    pub freecad: Option<bool>,
}

/// Runs `cases` through Silico and FreeCAD, each compared to its shape's [`Tolerance`].
pub fn run(cases: &[Case]) -> Result<Report, String> {
    let mut report = Report::default();
    for (case, (ours, theirs)) in cases.iter().zip(outcomes(cases)?) {
        if matches!((&ours, &theirs), (Outcome::Failed(_), Outcome::Failed(_))) {
            report.refused.push(case.name.clone());
        }
        let valid = |outcome: &Outcome| match outcome {
            Outcome::Built(measured) => Some(measured.valid),
            Outcome::Failed(_) | Outcome::Lost(_) => None,
        };
        report.validity.push(Validity {
            case: case.name.clone(),
            silico: valid(&ours),
            freecad: valid(&theirs),
        });
        let how = differences(&ours, &theirs, Tolerance::of(&case.shape));
        if !how.is_empty() {
            report.differences.push(Difference {
                case: case.name.clone(),
                ours,
                theirs,
                how,
            });
        }
    }
    Ok(report)
}
