//! The FreeCAD oracle (ARCHITECTURE.md, Testing). A [`Case`] says how to build a shape from
//! Silico's operations. The harness builds it through Silico's kernel workers, and through
//! headless FreeCAD (1.1.3, on the same OCCT release) with the calls Silico's port notes
//! name, and compares what each measures: volume, area, bounding box, center of mass,
//! topology counts, and validity (see [`compare`]). A case that one side refuses and the other builds is a
//! difference too. The hand-written cases live in `tests/oracle/`, which is protected;
//! [`generate`] makes random valid ones. Sketches ([`sketch`]) and sketch features
//! ([`features`]: pad, pocket, revolve, groove) have cases of their own there, as Silico
//! documents, and so do measures ([`measures`]: the core's `distance` and `angle` against
//! FreeCAD's Measure, pairs of elements of a document's solids), and so do sketch edits
//! ([`sketch_edit`]: a sketch and one edit, through Silico's `sketch_edit` and FreeCAD's
//! Sketcher, compared modulo what docs/ports/sketch-edit.md says Silico does otherwise).

pub mod case;
pub mod compare;
pub mod corpus;
pub mod features;
pub mod freecad;
pub mod generate;
pub mod measures;
pub mod silico;
pub mod sketch;
pub mod sketch_edit;

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

/// A generated sweep's `differences` (of `cases`, with each side's `validity`) less those
/// understood: [`generate::KNOWN`]'s, then [`without_freecads_no_solid`]'s, so a case listed
/// there keeps its own entry. The second list says each, as known; the third each known
/// difference of these cases that didn't show. A sweep's alone: the hand-written cases and
/// the corpus have their differences reported as they are.
pub fn sweep_without_known(
    differences: Vec<Difference>,
    cases: &[Case],
    validity: &[Validity],
) -> (Vec<Difference>, Vec<String>, Vec<String>) {
    let here: Vec<corpus::Known> = generate::KNOWN
        .iter()
        .filter(|k| cases.iter().any(|c| c.name == k.case))
        .copied()
        .collect();
    let (rest, mut known, missing) =
        corpus::without_known(differences, &here, &corpus::platform(), validity);
    let (rest, no_solid) = without_freecads_no_solid(rest, cases);
    known.extend(no_solid);
    (rest, known, missing)
}

/// `differences` (of `cases`) less those where FreeCAD's result is no solid and Silico
/// refuses it as such: FreeCAD built a shape whose volume is below zero (a solid turned
/// inside out) or that its own `isValid` finds invalid, and Silico refused the case's last
/// operation as an invalid result (the kernel's "{op} gave an invalid result, which was
/// refused", `{op}` the case's own top-level operation, not one beneath it or a measure of
/// the result). The second list says each, as known. It's `generate::KNOWN`'s generated_15_96
/// as a rule: OCCT 7.9.3 turns some torus booleans inside out, FreeCAD 1.1.3 keeps them, and
/// Silico refuses them (docs/ports/booleans.md, Known wrong results). Each case the sweeps
/// found, rebuilt as the oracle builds it, against the volume integrated by hand, slice by
/// slice (the exact area of the annuli's intersection at each height, adaptive Gauss-Kronrod,
/// which gives a torus's 2π²Rr² and two spheres' lens to 1e-13): generated_111_374, a torus
/// cut a torus, FreeCAD's -6480.64 where it's 5902.990; generated_161_99, a fuse, -50386.75
/// and invalid where it's 53261.001; generated_19_400 -146172.65 where it's 6576.118;
/// generated_24_431 -77962.89 where it's 2737.074; generated_41_99 -78796.32 where it's
/// 7379.488. Nothing else is excused: Silico refusing for another reason or another
/// operation, a case not among `cases`, or FreeCAD's result a sound solid, is still a
/// difference. For generated sweeps only ([`sweep_without_known`]).
pub fn without_freecads_no_solid(
    differences: Vec<Difference>,
    cases: &[Case],
) -> (Vec<Difference>, Vec<String>) {
    let mut known = Vec::new();
    let rest = differences
        .into_iter()
        .filter(|d| {
            let (Outcome::Failed(ours), Outcome::Built(theirs)) = (&d.ours, &d.theirs) else {
                return true;
            };
            let Some(case) = cases.iter().find(|c| c.name == d.case) else {
                return true;
            };
            let refused_invalid = ours
                .strip_prefix("invalid_result: ")
                .and_then(|m| m.split_once(" gave an invalid result, which was refused"))
                .is_some_and(|(op, _)| op.eq_ignore_ascii_case(case.shape.operation()));
            let no_solid = theirs.volume < 0.0 || !theirs.valid;
            if refused_invalid && no_solid {
                known.push(format!(
                    "{d} (known: FreeCAD's result is no solid, volume {} and valid {}, which Silico refuses as invalid)",
                    theirs.volume, theirs.valid
                ));
                false
            } else {
                true
            }
        })
        .collect();
    (rest, known)
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

#[cfg(test)]
mod tests {
    use super::{
        Case, Difference, Measured, Outcome, Shape, Validity, sweep_without_known,
        without_freecads_no_solid,
    };

    /// A generated case named `name` whose shape is `shape`.
    fn generated(name: &str, shape: Shape) -> Case {
        Case {
            name: name.to_owned(),
            note: "generated".to_owned(),
            boundary: None,
            shape,
        }
    }

    /// A torus cut a torus moved: generated_111_374's kind of shape (its values made up, as
    /// the rule doesn't build it).
    fn torus_cut() -> Shape {
        let torus = |major_radius, minor_radius| Shape::Torus {
            major_radius,
            minor_radius,
        };
        Shape::Cut {
            base: Box::new(torus(20.0, 9.0)),
            tool: Box::new(Shape::Translate {
                shape: Box::new(torus(18.0, 8.0)),
                by: [1.0, 2.0, 3.0],
            }),
        }
    }

    /// generated_111_374 as the case the differences are of.
    fn cases() -> Vec<Case> {
        vec![generated("generated_111_374", torus_cut())]
    }

    /// generated_111_374's two sides, as the sweep of seed 111 found them: Silico's refusal
    /// word for word, and FreeCAD's cut turned inside out (its volume and validity as
    /// FreeCAD 1.1.3 measured them, the rest made up).
    fn case(ours: Outcome, volume: f64, valid: bool) -> Difference {
        let theirs = Outcome::Built(Measured {
            volume,
            area: 8873.33,
            bounds: Some([[-20.0; 3], [20.0; 3]]),
            center: Some([0.0; 3]),
            faces: 8,
            edges: 12,
            vertices: 6,
            valid,
        });
        Difference {
            case: "generated_111_374".to_owned(),
            how: vec!["FreeCAD built it; Silico failed: ...".to_owned()],
            ours,
            theirs,
        }
    }

    fn refused_invalid() -> Outcome {
        Outcome::Failed(
            "invalid_result: Cut gave an invalid result, which was refused: the cut's result \
             isn't a valid solid. Suggestion: faces or edges of the two shapes nearly meet: move \
             one so they clearly overlap or stay clearly apart."
                .to_owned(),
        )
    }

    #[test]
    fn freecads_inside_out_or_invalid_solid_that_silico_refuses_as_invalid_is_known() {
        // Inside out (111_374, -6480.64, valid), and inside out and invalid (161_99).
        for (volume, valid) in [(-6480.640578694009, true), (-50386.75037071653, false)] {
            let (rest, known) =
                without_freecads_no_solid(vec![case(refused_invalid(), volume, valid)], &cases());
            assert!(rest.is_empty(), "{volume} {valid}: {rest:?}");
            assert_eq!(known.len(), 1);
            assert!(
                known[0].starts_with("generated_111_374: FreeCAD built it")
                    && known[0].contains(&format!("volume {volume} and valid {valid}")),
                "{known:?}"
            );
        }
        // A positive volume FreeCAD itself finds invalid.
        let (rest, known) =
            without_freecads_no_solid(vec![case(refused_invalid(), 5902.99, false)], &cases());
        assert!(rest.is_empty() && known.len() == 1, "{rest:?}");
    }

    #[test]
    fn a_sound_freecad_solid_or_another_refusal_of_silicos_is_still_a_difference() {
        let kept = |d: Difference| {
            let (rest, known) = without_freecads_no_solid(vec![d], &cases());
            assert!(known.is_empty(), "{known:?}");
            assert_eq!(rest.len(), 1);
        };
        // FreeCAD's solid sound: Silico's refusal is a difference, however it's worded.
        kept(case(refused_invalid(), 5902.987769501616, true));
        kept(case(refused_invalid(), 0.0, true));
        // FreeCAD's no solid, but Silico refused for another reason, or was lost.
        kept(case(
            Outcome::Failed("kernel_failed: Cut failed. Suggestion: ...".to_owned()),
            -6480.64,
            true,
        ));
        kept(case(
            Outcome::Failed(
                "invalid_result: Cut gave an empty result, which was refused".to_owned(),
            ),
            -6480.64,
            false,
        ));
        kept(case(
            Outcome::Lost("the worker crashed".to_owned()),
            -6480.64,
            true,
        ));
        // Silico built it: a difference whatever FreeCAD's is.
        let mut built = case(refused_invalid(), -6480.64, true);
        built.ours = built.theirs.clone();
        kept(built);
    }

    #[test]
    fn a_refusal_of_another_operation_or_under_another_code_is_still_a_difference() {
        let kept = |d: Difference, cases: &[Case]| {
            let (rest, known) = without_freecads_no_solid(vec![d], cases);
            assert!(known.is_empty(), "{known:?}");
            assert_eq!(rest.len(), 1);
        };
        let failed = |message: &str| Outcome::Failed(message.to_owned());
        // The phrase word for word, but under another code than invalid_result.
        kept(
            case(
                failed("kernel_failed: Cut gave an invalid result, which was refused: ..."),
                -6480.64,
                true,
            ),
            &cases(),
        );
        // invalid_result, but of an operation that isn't the case's last: a measure of the
        // result, a fuse where the case is a cut, the cut beneath a case's move.
        for op in ["Summarize", "Fuse", "Translate"] {
            kept(
                case(
                    failed(&format!(
                        "invalid_result: {op} gave an invalid result, which was refused: ..."
                    )),
                    -6480.64,
                    true,
                ),
                &cases(),
            );
        }
        let moved = generated(
            "generated_111_374",
            Shape::Translate {
                shape: Box::new(torus_cut()),
                by: [0.0, 0.0, 1.0],
            },
        );
        kept(
            case(refused_invalid(), -6480.64, true),
            std::slice::from_ref(&moved),
        );
        // The case's own operation, named for the move: known.
        let (rest, known) = without_freecads_no_solid(
            vec![case(
                failed("invalid_result: Translate gave an invalid result, which was refused"),
                -6480.64,
                true,
            )],
            &[moved],
        );
        assert!(rest.is_empty() && known.len() == 1, "{rest:?}");
        // A case the run didn't make: nothing to name its operation, a difference.
        kept(case(refused_invalid(), -6480.64, true), &[]);
        kept(
            case(refused_invalid(), -6480.64, true),
            &[generated("generated_111_375", torus_cut())],
        );
    }

    #[test]
    fn a_sweep_takes_its_listed_case_by_its_entry_and_then_freecads_no_solid() {
        // generated_15_96, listed in generate::KNOWN, is that entry's (its own words), not
        // the rule's, though the rule would take it too; generated_111_374, listed nowhere,
        // the rule's.
        let listed = &crate::generate::KNOWN[0];
        let mut d15 = case(refused_invalid(), -57587.21, true);
        d15.case = listed.case.to_owned();
        d15.how = vec![listed.difference.to_owned()];
        let cases = vec![
            generated(listed.case, torus_cut()),
            generated("generated_111_374", torus_cut()),
        ];
        let validity: Vec<Validity> = cases
            .iter()
            .map(|c| Validity {
                case: c.name.clone(),
                silico: None,
                freecad: Some(true),
            })
            .collect();
        let (rest, known, missing) = sweep_without_known(
            vec![d15, case(refused_invalid(), -6480.64, true)],
            &cases,
            &validity,
        );
        assert!(
            rest.is_empty() && missing.is_empty(),
            "{rest:?} {missing:?}"
        );
        assert_eq!(known.len(), 2, "{known:?}");
        assert!(
            known[0].starts_with("generated_15_96: ") && known[0].contains(listed.why),
            "{known:?}"
        );
        assert!(
            known[1].starts_with("generated_111_374: ")
                && known[1].contains("FreeCAD's result is no solid"),
            "{known:?}"
        );
    }
}
