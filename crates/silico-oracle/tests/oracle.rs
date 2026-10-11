//! The oracle's cases (tests/oracle/, protected) match FreeCAD; every operation has its
//! cases; a sample of generated cases matches too (`pixi run oracle-sweep` runs 500); and
//! the harness sees a wrong result. Needs the FreeCAD oracle (`pixi install -e oracle`) and
//! the kernel worker built (`cargo nextest run --workspace` builds it).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::Path;

use silico_oracle::{
    Case, Difference, Outcome, Tolerance, differences, generate, load, outcomes, run,
};

const CASES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/oracle");

fn report(found: &[Difference]) -> String {
    found
        .iter()
        .map(Difference::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_case_matches_freecad() {
    let cases = load(Path::new(CASES)).unwrap();
    let found = run(&cases).unwrap().differences;
    assert!(
        found.is_empty(),
        "{} cases differ:\n{}",
        found.len(),
        report(&found)
    );
}

#[test]
fn every_operation_has_three_cases_one_near_a_tolerance_boundary() {
    let cases = load(Path::new(CASES)).unwrap();
    let mut by_operation: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for case in &cases {
        let entry = by_operation.entry(case.shape.operation()).or_default();
        entry.0 += 1;
        entry.1 += usize::from(case.boundary.is_some());
    }
    for operation in [
        "box",
        "cylinder",
        "sphere",
        "cone",
        "torus",
        "translate",
        "fuse",
        "cut",
        "common",
        "fillet",
        "chamfer",
        "extrude",
        "revolve",
    ] {
        let (all, near) = by_operation.get(operation).copied().unwrap_or_default();
        assert!(
            all >= 3 && near >= 1,
            "{operation}: {all} cases, {near} near a tolerance boundary"
        );
    }
}

#[test]
fn generated_cases_match_freecad() {
    // Each builds on both sides (both refusing would agree, and compare nothing), and
    // matches.
    let cases = generate::cases(20_261_010, 40);
    let found = run(&cases).unwrap();
    assert!(found.refused.is_empty(), "both refused {:?}", found.refused);
    assert!(
        found.differences.is_empty(),
        "{} cases differ:\n{}",
        found.differences.len(),
        report(&found.differences)
    );
}

#[test]
fn freecads_measures_are_compared_not_silicos_precise_ones() {
    // A cone fused with a cylinder (generated case 7 #782): OCCT's default integration,
    // FreeCAD's, is 3.6e-3 off the precise volume here. Compared as FreeCAD measures it,
    // Silico's matches; its precise volume wouldn't.
    let case = generate::cases(7, 783).remove(782);
    assert_eq!(case.shape.operation(), "fuse");
    let (ours, theirs) = outcomes(std::slice::from_ref(&case)).unwrap().remove(0);
    let found = differences(&ours, &theirs, Tolerance::of(&case.shape));
    assert!(found.is_empty(), "{found:?}");
    let mut kernel = silico_core::kernel::KernelWorker::start().unwrap();
    let shape = silico_oracle::silico::build(&mut kernel, &case.shape).unwrap();
    let precise = kernel.summarize(&shape).unwrap().volume;
    let Outcome::Built(theirs) = theirs else {
        panic!("{theirs:?}")
    };
    assert!((precise - theirs.volume).abs() > 1e-3 * theirs.volume);
}

#[test]
fn harness_catches_wrong_result_through_both_sides() {
    // Through both sides for real, then Silico's measure spoiled: a 0.1% volume error, or
    // one extra face, is a difference.
    let cases: Vec<Case> = load(Path::new(CASES))
        .unwrap()
        .into_iter()
        .filter(|c| c.name == "fillet_box_edge1")
        .collect();
    let tolerance = Tolerance::of(&cases[0].shape);
    let (ours, theirs) = outcomes(&cases).unwrap().remove(0);
    assert!(differences(&ours, &theirs, tolerance).is_empty());
    let Outcome::Built(measured) = ours else {
        panic!("{ours:?}")
    };
    let mut volume = measured.clone();
    volume.volume *= 1.001;
    assert!(!differences(&Outcome::Built(volume), &theirs, tolerance).is_empty());
    let mut face = measured;
    face.faces += 1;
    assert!(!differences(&Outcome::Built(face), &theirs, tolerance).is_empty());
}
