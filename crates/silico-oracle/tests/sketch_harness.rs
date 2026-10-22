//! The sketch oracle's harness (M3.3): the FreeCAD model it builds from a Silico sketch, and
//! that it catches a wrong answer from either side. The models expected are written out by
//! hand from FreeCAD's `Sketcher.Constraint` arguments.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f64::consts::{FRAC_PI_2, PI};

use silico_core::document::{Operation, Sketch, read};
use silico_core::sketch::{SketchWorker, Translation};
use silico_oracle::sketch::{
    FreecadConstraint, FreecadGeometry, FreecadSketch, SketchCase, compare, freecad_sketch,
    freecad_solve, silico_solve,
};

/// A document of one sketch with these fields (the JSON after the feature's name).
fn sketch(fields: &str) -> Sketch {
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"s0000001\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S\", \"plane\": \"xy\", {fields}}}]}}"
    );
    match &read(text.as_bytes()).unwrap().features[0].operation {
        Operation::Sketch(s) => s.clone(),
        other => panic!("{other:?}"),
    }
}

/// Two lines from p1 (0, 0): n1 to p2 (10, 0) and n2 to p3 (0, 8); n1 horizontal, 10 long,
/// p1 at the origin, and these constraints.
fn corner(more: &str) -> Sketch {
    sketch(&format!(
        r#""points": [{{"id": "p0000001", "x": 0, "y": 0, "construction": false}}, {{"id": "p0000002", "x": 10, "y": 0, "construction": false}}, {{"id": "p0000003", "x": 0, "y": 8, "construction": false}}], "lines": [{{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}}, {{"id": "n0000002", "from": "p0000001", "to": "p0000003", "construction": false}}], "circles": [], "arcs": [], "constraints": [{{"id": "k0000001", "type": "horizontal", "line": "n0000001"}}, {{"id": "k0000002", "type": "length", "line": "n0000001", "value": 10, "driving": true}}, {{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}}{more}]"#
    ))
}

fn model(s: &Sketch) -> FreecadSketch {
    freecad_sketch(s, &Translation::new(s).unwrap()).unwrap()
}

fn constraint(
    kind: &'static str,
    ints: Vec<i32>,
    value: Option<f64>,
    id: &str,
) -> FreecadConstraint {
    FreecadConstraint {
        kind,
        ints,
        value,
        driving: true,
        id: Some(id.to_owned()),
        locked: None,
    }
}

#[test]
fn a_shared_point_is_freecads_coincident_ends_after_silicos_constraints() {
    let m = model(&corner(""));
    assert_eq!(
        m.geometry,
        [
            FreecadGeometry::Line {
                start: [0.0, 0.0],
                end: [10.0, 0.0]
            },
            FreecadGeometry::Line {
                start: [0.0, 0.0],
                end: [0.0, 8.0]
            },
        ]
    );
    assert_eq!(
        m.constraints,
        [
            constraint("Horizontal", vec![0], None, "k0000001"),
            constraint("Distance", vec![0], Some(10.0), "k0000002"),
            // The origin is FreeCAD's root point, (−1, 1).
            constraint("DistanceX", vec![-1, 1, 0, 1], Some(0.0), "k0000003"),
            // p1 is both lines' start: one coincidence joins them.
            FreecadConstraint {
                kind: "Coincident",
                ints: vec![0, 1, 1, 1],
                value: None,
                driving: true,
                id: None,
                locked: None,
            },
        ]
    );
    assert_eq!(m.points["p0000001"], (0, 1));
    assert_eq!(m.points["p0000003"], (1, 2));
}

#[test]
fn a_tangency_where_curves_meet_is_freecads_own_coincidence() {
    // FreeCAD's endpoint-to-endpoint tangency joins the two ends itself: no coincidence is
    // added for them. Silico's θ of π (n1 starts at p1, n2 starts at p1: smooth means their
    // own directions opposite) is FreeCAD's locked value π − π/2.
    let m = model(&corner(
        r#", {"id": "k0000004", "type": "tangent_at", "a": "n0000001", "b": "n0000002", "at": "p0000001", "direction": "same"}"#,
    ));
    assert!(
        m.constraints.iter().all(|c| c.kind != "Coincident"),
        "{m:?}"
    );
    let tangent = m.constraints.iter().find(|c| c.kind == "Tangent").unwrap();
    assert_eq!(tangent.ints, [0, 1, 1, 1]);
    assert_eq!(tangent.locked, Some(PI - FRAC_PI_2));
}

#[test]
fn every_form_goes_to_freecads_constraint_of_it() {
    // A circle c1 about p4 and an arc a1 about p4 from p2 to p5 beside the corner, and one
    // constraint of each further form; FreeCAD's arguments by hand. Geometry: n1 (0), n2
    // (1), c1 (2), a1 (3).
    let s = sketch(
        r#""points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}, {"id": "p0000003", "x": 0, "y": 8, "construction": false}, {"id": "p0000004", "x": 10, "y": 8, "construction": false}, {"id": "p0000005", "x": 2, "y": 8, "construction": false}],
        "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n0000002", "from": "p0000001", "to": "p0000003", "construction": false}],
        "circles": [{"id": "c0000001", "center": "p0000004", "radius": 3, "construction": false}],
        "arcs": [{"id": "a0000001", "center": "p0000004", "start": "p0000002", "end": "p0000005", "construction": false}],
        "constraints": [
          {"id": "k0000001", "type": "vertical_points", "a": "p0000003", "b": "origin"},
          {"id": "k0000002", "type": "point_line_distance", "point": "p0000004", "line": "n0000001", "value": 8, "side": "left", "driving": true},
          {"id": "k0000003", "type": "on", "point": "p0000005", "curve": "c0000001"},
          {"id": "k0000004", "type": "angle", "a": "x_axis", "b": "n0000002", "value": 90, "driving": false},
          {"id": "k0000005", "type": "diameter", "curve": "c0000001", "value": 6, "driving": true},
          {"id": "k0000006", "type": "equal", "a": "c0000001", "b": "a0000001"},
          {"id": "k0000007", "type": "tangent_on", "a": "n0000001", "at": "p0000002", "b": "c0000001", "direction": "opposite"},
          {"id": "k0000008", "type": "symmetric", "a": "p0000003", "b": "p0000002", "about": "p0000001"},
          {"id": "k0000009", "type": "midpoint", "point": "p0000005", "line": "n0000002"}
        ]"#,
    );
    let m = model(&s);
    let ours: Vec<_> = m.constraints.iter().filter(|c| c.id.is_some()).collect();
    let want = [
        ("Vertical", vec![1, 2, -1, 1], None),
        ("Distance", vec![2, 3, 0], Some(8.0)),
        ("PointOnObject", vec![3, 2, 2], None),
        ("Angle", vec![-1, 1], Some(FRAC_PI_2)),
        ("Diameter", vec![2], Some(6.0)),
        ("Equal", vec![2, 3], None),
        ("Tangent", vec![0, 2, 2], None),
        ("Symmetric", vec![1, 2, 0, 2, 0, 1], None),
        ("Symmetric", vec![1, 1, 1, 2, 3, 2], None),
    ];
    assert_eq!(ours.len(), want.len());
    for (c, (kind, ints, value)) in ours.iter().zip(want) {
        assert_eq!((c.kind, &c.ints, c.value), (kind, &ints, value), "{c:?}");
    }
    // The reference angle isn't driving; the tangency at n1's end, opposite, is θ = 0,
    // locked at −π/2.
    assert!(!ours[3].driving);
    assert_eq!(ours[6].locked, Some(-FRAC_PI_2));
    // The arc: radius √68 from (10, 8) to (10, 0), from atan2(−8, 0) to atan2(0, −8) = π.
    match m.geometry[3] {
        FreecadGeometry::Arc {
            radius, from, to, ..
        } => {
            assert_eq!(radius, 8.0);
            assert_eq!((from, to), (-FRAC_PI_2, PI));
        }
        ref other => panic!("{other:?}"),
    }
}

fn case(s: Sketch) -> SketchCase {
    SketchCase {
        name: "corner".to_owned(),
        note: String::new(),
        sketch: s,
    }
}

#[test]
fn the_harness_catches_a_wrong_answer_from_either_side() {
    // The corner held fully, solved by both sides: they agree. FreeCAD given a length of 11
    // instead of 10 (a wrong model) puts p2 elsewhere, and the harness says so; FreeCAD's
    // lists made to name a constraint redundant that Silico's don't, and it says so.
    let s = corner(
        r#", {"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000005", "type": "vertical", "line": "n0000002"}, {"id": "k0000006", "type": "length", "line": "n0000002", "value": 8, "driving": true}"#,
    );
    // The sketch worker beside this test, as the oracle's run finds it.
    let mut worker = SketchWorker::start().unwrap();
    let ours = silico_solve(&case(s.clone()), &mut worker).unwrap();
    let good = model(&s);
    let mut wrong = good.clone();
    wrong.constraints[1].value = Some(11.0);
    let theirs = freecad_solve(&[good.clone(), wrong.clone()]).unwrap();
    assert!(
        compare(&good, &ours, &theirs[0]).is_empty(),
        "{:?}",
        compare(&good, &ours, &theirs[0])
    );
    let how = compare(&wrong, &ours, &theirs[1]);
    assert!(
        how.iter().any(|h| h.starts_with("point p0000002")),
        "{how:?}"
    );
    let mut listed = theirs[0].clone();
    listed.redundant = vec![2];
    let how = compare(&good, &ours, &listed);
    assert!(how.iter().any(|h| h.starts_with("redundant")), "{how:?}");
}

#[test]
fn every_sketch_case_matches_freecad() {
    // tests/oracle/sketches.silico (protected): the maintainer's approved cases, each solved
    // by Silico's sketch worker and by FreeCAD 1.1.3.
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/oracle"));
    let cases = silico_oracle::sketch::load(dir).unwrap();
    assert_eq!(cases.len(), 15);
    let differences = silico_oracle::sketch::run(&cases).unwrap();
    assert!(
        differences.is_empty(),
        "{}",
        differences
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_harness_catches_each_measure_freecad_gives_otherwise() {
    // The council's tests lens: the self-test above caught only points and lists. Two of
    // the approved cases solved by both sides, then FreeCAD's answer spoilt one measure at a
    // time: its degrees of freedom, a circle's radius, a tangency's locked value, a
    // reference value, and how it ended. Each is reported.
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/oracle"));
    let cases = silico_oracle::sketch::load(dir).unwrap();
    let pick = |name: &str| cases.iter().find(|c| c.name == name).unwrap().clone();
    let (tangent, diameter) = (pick("tangent_on_circle"), pick("reference_diameter"));
    let mut worker = SketchWorker::start().unwrap();
    let models: Vec<FreecadSketch> = [&tangent, &diameter]
        .iter()
        .map(|c| freecad_sketch(&c.sketch, &Translation::new(&c.sketch).unwrap()).unwrap())
        .collect();
    let theirs = freecad_solve(&models).unwrap();
    let ours = [
        silico_solve(&tangent, &mut worker).unwrap(),
        silico_solve(&diameter, &mut worker).unwrap(),
    ];
    for i in 0..2 {
        assert!(compare(&models[i], &ours[i], &theirs[i]).is_empty());
    }
    let reported =
        |i: usize, spoil: &dyn Fn(&mut silico_oracle::sketch::FreecadSolved), what: &str| {
            let mut t = theirs[i].clone();
            spoil(&mut t);
            let how = compare(&models[i], &ours[i], &t);
            assert!(how.iter().any(|h| h.contains(what)), "{what}: {how:?}");
        };
    reported(0, &|t| t.dof += 1, "degrees of freedom");
    reported(
        0,
        &|t| *t.radii.get_mut("c0000001").unwrap() += 1e-6,
        "radius",
    );
    let lock = models[0]
        .constraints
        .iter()
        .position(|c| c.locked.is_some())
        .unwrap();
    reported(0, &|t| t.values[lock] += PI, "locked");
    reported(0, &|t| t.code = -3, "FreeCAD's solve gave -3");
    let reference = models[1]
        .constraints
        .iter()
        .position(|c| !c.driving)
        .unwrap();
    reported(1, &|t| t.values[reference] += 1e-3, "reference");
}

#[test]
fn a_symmetry_about_the_origin_is_freecads_symmetric_about_its_root_point() {
    // Round 1's mutants: a symmetry about the sketch's origin wasn't built. In FreeCAD's
    // terms the origin is geometry −1's start (its root point): p2 (n1's end, 0 2) and p3
    // (n2's end, 1 2) about (−1, 1).
    let m = model(&corner(
        r#", {"id": "k0000004", "type": "symmetric", "a": "p0000002", "b": "p0000003", "about": "origin"}"#,
    ));
    let made = m
        .constraints
        .iter()
        .find(|c| c.id.as_deref() == Some("k0000004"))
        .unwrap();
    assert_eq!(
        (made.kind, made.ints.clone()),
        ("Symmetric", vec![0, 2, 1, 2, -1, 1])
    );
}

#[test]
fn a_case_file_that_cant_be_read_is_an_error_and_a_missing_one_no_cases() {
    // Round 1's mutants: any read error was taken as no file, so a case file the harness
    // couldn't read (here a directory where it goes) ran no cases, silently.
    let temp = tempfile::Builder::new()
        .prefix("silico-oracle-load-")
        .tempdir()
        .unwrap();
    let dir = temp.path();
    assert!(silico_oracle::sketch::load(dir).unwrap().is_empty());
    std::fs::create_dir(dir.join(silico_oracle::sketch::FILE)).unwrap();
    let e = silico_oracle::sketch::load(dir).unwrap_err();
    assert!(e.contains("can't read"), "{e}");
}

/// FreeCAD's answer ending in `code`, its lists empty and nothing solved; and Silico's
/// ending in `outcome`, its lists empty too: so only how each ended is compared.
fn ended(
    code: i32,
    outcome: Result<silico_core::sketch::SketchSolution, silico_core::Error>,
) -> (
    silico_oracle::sketch::FreecadSolved,
    silico_oracle::sketch::Ours,
) {
    let theirs = silico_oracle::sketch::FreecadSolved {
        code,
        dof: 0,
        conflicting: Vec::new(),
        redundant: Vec::new(),
        partially_redundant: Vec::new(),
        malformed: Vec::new(),
        points: Default::default(),
        radii: Default::default(),
        values: Vec::new(),
    };
    let ours = silico_oracle::sketch::Ours {
        dofs: 0,
        conflicting: Vec::new(),
        redundant: Vec::new(),
        partially_redundant: Vec::new(),
        outcome,
    };
    (theirs, ours)
}

#[test]
fn how_each_side_ended_is_compared_as_the_harness_says_they_agree() {
    // M3.3's round-1 mutants: the outcome arms that let two endings agree were tested only
    // where they agree. Each pair here, with the lists equal, either agrees or is a
    // difference, as compare's comment has it (and docs/ports/sketcher.md for −3 and −4).
    use silico_core::{Error, ErrorCode};
    let m = model(&corner(""));
    let failed = |code| Err(Error::new(code, "made up here"));
    // A solution of nothing: the (−1, solved) row never reaches compare_solutions, so only
    // that Silico solved matters.
    let solved = || {
        Ok(silico_core::sketch::SketchSolution {
            points: Default::default(),
            radii: Default::default(),
            references: Default::default(),
            dofs: 0,
            free: Default::default(),
            redundant: Default::default(),
            algorithm: silico_protocol::Algorithm::DogLeg,
        })
    };
    for (code, outcome, agrees, why) in [
        (
            -2,
            failed(ErrorCode::OverConstrained),
            true,
            "redundant, refused as over-constrained",
        ),
        (
            -2,
            failed(ErrorCode::SolveFailed),
            false,
            "redundant, but failed otherwise",
        ),
        (
            -1,
            solved(),
            false,
            "FreeCAD failed with nothing redundant, Silico solved",
        ),
        (
            -3,
            failed(ErrorCode::OverConstrained),
            true,
            "conflicting, over-constrained",
        ),
        (
            -3,
            failed(ErrorCode::SolveFailed),
            false,
            "conflicting, but failed otherwise",
        ),
        (
            -4,
            failed(ErrorCode::InvalidResult),
            false,
            "over-constrained, but an invalid result",
        ),
    ] {
        let (theirs, ours) = ended(code, outcome);
        let how = compare(&m, &ours, &theirs);
        assert_eq!(how.is_empty(), agrees, "{why}: {how:?}");
    }
}
