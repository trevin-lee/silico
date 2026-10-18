//! M3.3: a document's sketch translated into the sketch worker's terms, and the worker's
//! answers read back (docs/design/m3.3-solving.md). The systems expected are written out by
//! hand from the design's rules; the answers read are made up here, standing in for the
//! worker, so each outcome is reached on purpose. Solving itself, through the worker, is
//! silico-sketch's tests/document.rs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/sketch.rs"]
mod fixture;

use std::f64::consts::PI;

use silico_core::ErrorCode;
use silico_core::document::{Checked, FeatureId, LineSide, Operation, Sketch, read};
use silico_core::sketch::{ALGORITHMS, Measured, SketchSolution, SketchSolver, Translation, solve};
use silico_protocol::{
    Algorithm, Geometry, PointOf, PointValues, SketchGeometry, SketchSystem, SolveStatus, Solved,
    SolverConstraint, Tag, Value,
};

fn point(id: &str, x: f64, y: f64) -> String {
    format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
}

fn line(id: &str, from: &str, to: &str) -> String {
    format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": false}}"#)
}

fn circle(id: &str, center: &str, radius: f64) -> String {
    format!(r#"{{"id": "{id}", "center": "{center}", "radius": {radius}, "construction": false}}"#)
}

fn arc(id: &str, center: &str, start: &str, end: &str) -> String {
    format!(
        r#"{{"id": "{id}", "center": "{center}", "start": "{start}", "end": "{end}", "construction": false}}"#
    )
}

/// A document of one sketch with these lists.
fn text(
    points: &[String],
    lines: &[String],
    circles: &[String],
    arcs: &[String],
    constraints: &[String],
) -> String {
    fixture::document(&fixture::fields(
        &points.join(", "),
        &lines.join(", "),
        &circles.join(", "),
        &arcs.join(", "),
        &constraints.join(", "),
    ))
}

fn sketch_of(text: &str) -> Sketch {
    match &read(text.as_bytes()).unwrap().features[0].operation {
        Operation::Sketch(s) => s.clone(),
        other => panic!("{other:?}"),
    }
}

fn translate(text: &str) -> Translation {
    Translation::new(&sketch_of(text)).unwrap()
}

/// The constraints sent, without the arcs' rules.
fn sent(t: &Translation) -> Vec<SolverConstraint> {
    t.system
        .constraints
        .iter()
        .copied()
        .filter(|c| c.tag().is_some())
        .collect()
}

fn pv(x: u32) -> PointValues {
    PointValues {
        x: Value(x),
        y: Value(x + 1),
    }
}

#[test]
fn the_designs_example_translates_constraint_by_constraint_tagged_by_place() {
    // docs/design/m3.2-sketch-format.md's rectangle, as its file holds it. Values: the
    // origin and the axes' far ends (0..6), the four points sorted by ID (6..14, unknowns),
    // then each driving dimension's value as its constraint comes (14..18). Geometry: the
    // origin and the axes (0..3), the points (3..7), the lines sorted by ID (7..12).
    let points = [
        point("0a1b2c3d", 0.0, 0.0),
        point("2p3q4r5s", 0.0, 20.0),
        point("4e5f6g7h", 40.0, 0.0),
        point("8j9k0m1n", 40.0, 20.0),
    ];
    let lines = [
        line("6t7v8w9x", "0a1b2c3d", "4e5f6g7h"),
        line("a0b1c2d3", "4e5f6g7h", "8j9k0m1n"),
        line("e4f5g6h7", "0a1b2c3d", "8j9k0m1n"),
        line("j8k9m0n1", "8j9k0m1n", "2p3q4r5s"),
        line("p2q3r4s5", "2p3q4r5s", "0a1b2c3d"),
    ];
    let constraints = [
        r#"{"id": "c0d1e2f3", "type": "horizontal", "line": "6t7v8w9x"}"#,
        r#"{"id": "g4h5j6k7", "type": "length", "line": "6t7v8w9x", "value": 40, "driving": true}"#,
        r#"{"id": "m8n9p0q1", "type": "distance_x", "a": "origin", "b": "0a1b2c3d", "value": 0, "driving": true}"#,
        r#"{"id": "r2s3t4v5", "type": "vertical", "line": "a0b1c2d3"}"#,
        r#"{"id": "w6x7y8z9", "type": "length", "line": "a0b1c2d3", "value": 20, "driving": true}"#,
        r#"{"id": "x0y1z2a3", "type": "horizontal", "line": "j8k9m0n1"}"#,
        r#"{"id": "y4z5a6b7", "type": "distance_y", "a": "origin", "b": "0a1b2c3d", "value": 0, "driving": true}"#,
        r#"{"id": "z8a9b0c1", "type": "vertical", "line": "p2q3r4s5"}"#,
    ]
    .map(str::to_owned);
    let t = translate(&text(&points, &lines, &[], &[], &constraints));
    let s = &t.system;
    assert_eq!(
        s.values,
        [
            0.0, 0.0, 1.0, 0.0, 0.0, 1.0, // origin, the axes' ends
            0.0, 0.0, 0.0, 20.0, 40.0, 0.0, 40.0, 20.0, // the points
            40.0, 0.0, 20.0, 0.0, // the dimensions
        ]
    );
    assert_eq!(s.unknowns, (6..14).map(Value).collect::<Vec<_>>());
    assert_eq!(s.algorithms, ALGORITHMS);
    assert_eq!(s.geometry.len(), 12);
    assert_eq!(s.geometry[3], SketchGeometry::Point(pv(6)));
    assert_eq!(
        s.geometry[7],
        SketchGeometry::Line {
            start: pv(6),
            end: pv(10)
        }
    );
    let tag = Tag;
    assert_eq!(
        s.constraints,
        [
            SolverConstraint::Horizontal {
                tag: tag(1),
                line: Geometry(7)
            },
            SolverConstraint::Distance {
                tag: tag(2),
                a: PointOf::Start(Geometry(7)),
                b: PointOf::End(Geometry(7)),
                value: Value(14)
            },
            SolverConstraint::DistanceX {
                tag: tag(3),
                a: PointOf::Point(Geometry(0)),
                b: PointOf::Point(Geometry(3)),
                value: Value(15)
            },
            SolverConstraint::Vertical {
                tag: tag(4),
                line: Geometry(8)
            },
            SolverConstraint::Distance {
                tag: tag(5),
                a: PointOf::Start(Geometry(8)),
                b: PointOf::End(Geometry(8)),
                value: Value(16)
            },
            SolverConstraint::Horizontal {
                tag: tag(6),
                line: Geometry(10)
            },
            SolverConstraint::DistanceY {
                tag: tag(7),
                a: PointOf::Point(Geometry(0)),
                b: PointOf::Point(Geometry(3)),
                value: Value(17)
            },
            SolverConstraint::Vertical {
                tag: tag(8),
                line: Geometry(11)
            },
        ]
    );
    s.check().unwrap();
}

#[test]
fn the_files_order_doesnt_change_what_is_sent() {
    // The same sketch with every list reversed: sorted by ID, the same system.
    let points = [
        point("p0000001", 0.0, 0.0),
        point("p0000002", 10.0, 1.0),
        point("p0000003", 3.0, 7.0),
    ];
    let lines = [
        line("n0000001", "p0000001", "p0000002"),
        line("n0000002", "p0000002", "p0000003"),
    ];
    let constraints = [
        r#"{"id": "k0000001", "type": "horizontal", "line": "n0000001"}"#.to_owned(),
        r#"{"id": "k0000002", "type": "perpendicular", "a": "n0000001", "b": "n0000002"}"#
            .to_owned(),
    ];
    let rev = |v: &[String]| v.iter().rev().cloned().collect::<Vec<_>>();
    let forward = translate(&text(&points, &lines, &[], &[], &constraints));
    let backward = translate(&text(
        &rev(&points),
        &rev(&lines),
        &[],
        &[],
        &rev(&constraints),
    ));
    assert_eq!(forward.system, backward.system);
}

/// A line n1 from p1 (0, 0) to p2 (10, 0), a circle c1 about p3 (5, 10) of radius 10, and
/// an arc a1 about p3 from p2 to p4 (−5, 10): with these constraints.
fn shapes(constraints: &[&str]) -> String {
    text(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 5.0, 10.0),
            point("p0000004", -5.0, 10.0),
        ],
        &[line("n0000001", "p0000001", "p0000002")],
        &[circle("c0000001", "p0000003", 10.0)],
        &[arc("a0000001", "p0000003", "p0000002", "p0000004")],
        &constraints
            .iter()
            .map(|c| (*c).to_owned())
            .collect::<Vec<_>>(),
    )
}

/// The angle value the one sent constraint holds (a tangency at an end or a point).
fn angle_sent(t: &Translation) -> f64 {
    match sent(t)[..] {
        [
            SolverConstraint::TangentOn { angle, .. }
            | SolverConstraint::AngleViaPoint { angle, .. },
        ] => t.system.values[angle.0 as usize],
        ref other => panic!("{other:?}"),
    }
}

#[test]
fn a_tangency_at_an_end_holds_the_angle_its_direction_and_end_give() {
    // The line's direction leaving `at` against the circle's own (counter-clockwise):
    // leaving its start (p1) it runs as drawn, so `same` is θ = 0; leaving its end (p2) it
    // runs back, so `same` is π. `opposite` is the other.
    for (at, direction, want) in [
        ("p0000001", "same", 0.0),
        ("p0000001", "opposite", PI),
        ("p0000002", "same", PI),
        ("p0000002", "opposite", 0.0),
    ] {
        let c = format!(
            r#"{{"id": "k0000001", "type": "tangent_on", "a": "n0000001", "at": "{at}", "b": "c0000001", "direction": "{direction}"}}"#
        );
        let t = translate(&shapes(&[&c]));
        assert_eq!(angle_sent(&t), want, "{at} {direction}");
        let SolverConstraint::TangentOn { a, point, b, .. } = sent(&t)[0] else {
            panic!()
        };
        // Geometry: origin, axes (0..3), the four points (3..7), the line (7), the circle
        // (8), the arc (9).
        assert_eq!((a, b), (Geometry(7), Geometry(8)));
        let end = if at == "p0000001" {
            PointOf::Start(a)
        } else {
            PointOf::End(a)
        };
        assert_eq!(point, end);
    }
}

#[test]
fn a_tangency_where_two_curves_meet_holds_the_angle_their_ends_give() {
    // Lines n1 from p1 to p2 and, each case, n2 between p2 and p3 one way or the other, or
    // n2 from p1: smooth (`same`) is θ = 0 when the shared point is one's end and the
    // other's start, π when both start or both end there.
    let points = [
        point("p0000001", 0.0, 0.0),
        point("p0000002", 10.0, 0.0),
        point("p0000003", 20.0, 1.0),
    ];
    for (second, at, direction, want) in [
        (("p0000002", "p0000003"), "p0000002", "same", 0.0),
        (("p0000003", "p0000002"), "p0000002", "same", PI),
        (("p0000002", "p0000003"), "p0000002", "opposite", PI),
        (("p0000003", "p0000002"), "p0000002", "opposite", 0.0),
        (("p0000001", "p0000003"), "p0000001", "same", PI),
        (("p0000003", "p0000001"), "p0000001", "same", 0.0),
    ] {
        let c = format!(
            r#"{{"id": "k0000001", "type": "tangent_at", "a": "n0000001", "b": "n0000002", "at": "{at}", "direction": "{direction}"}}"#
        );
        let t = translate(&text(
            &points,
            &[
                line("n0000001", "p0000001", "p0000002"),
                line("n0000002", second.0, second.1),
            ],
            &[],
            &[],
            &[c],
        ));
        assert_eq!(angle_sent(&t), want, "{second:?} at {at}, {direction}");
    }
}

#[test]
fn an_arcs_own_ends_mirror_about_a_line_its_centre_is_held_on_by_one_equation() {
    // FreeCAD's 365af33fc6, decided from the references: the arc a1 about p3 from p2 to p4,
    // its ends mirrored about a line through p3 (its end) or one an `on` holds p3 on, is one
    // equation; about a line nothing holds its centre on, two.
    let points = [
        point("p0000002", 10.0, 0.0),
        point("p0000003", 5.0, 10.0),
        point("p0000004", 0.0, 0.0),
        point("p0000005", 5.0, -10.0),
        point("p0000006", 5.1, 30.0),
    ];
    let arcs = [arc("a0000001", "p0000003", "p0000002", "p0000004")];
    let symmetric = r#"{"id": "k0000001", "type": "symmetric", "a": "p0000002", "b": "p0000004", "about": "n0000001"}"#;
    let on = r#"{"id": "k0000002", "type": "on", "point": "p0000003", "curve": "n0000001"}"#;
    for (from, constraints, one) in [
        ("p0000003", vec![symmetric], true),
        ("p0000006", vec![symmetric, on], true),
        ("p0000006", vec![symmetric], false),
    ] {
        let t = translate(&text(
            &points,
            &[line("n0000001", from, "p0000005")],
            &[],
            &arcs,
            &constraints
                .iter()
                .map(|c| (*c).to_owned())
                .collect::<Vec<_>>(),
        ));
        let first = sent(&t)[0];
        assert_eq!(
            matches!(first, SolverConstraint::MidpointOnLine { .. }),
            one,
            "{from} {constraints:?}: {first:?}"
        );
        assert_eq!(
            matches!(first, SolverConstraint::SymmetricLine { .. }),
            !one,
            "{first:?}"
        );
    }
    // The same ends about a point: two equations, whatever holds the centre.
    let t = translate(&text(
        &points,
        &[],
        &[],
        &arcs,
        &[r#"{"id": "k0000001", "type": "symmetric", "a": "p0000002", "b": "p0000004", "about": "p0000003"}"#.to_owned()],
    ));
    assert!(matches!(
        sent(&t)[0],
        SolverConstraint::SymmetricPoint { .. }
    ));
}

#[test]
fn a_diameter_goes_as_half_and_reference_dimensions_not_at_all() {
    let t = translate(&shapes(&[
        r#"{"id": "k0000001", "type": "diameter", "curve": "c0000001", "value": 20.000001, "driving": true}"#,
        r#"{"id": "k0000002", "type": "length", "line": "n0000001", "value": 10, "driving": false}"#,
        r#"{"id": "k0000003", "type": "radius", "curve": "a0000001", "value": 3, "driving": true}"#,
    ]));
    match sent(&t)[..] {
        [
            SolverConstraint::Radius {
                tag: Tag(1),
                circle: Geometry(8),
                value: half,
            },
            SolverConstraint::Radius {
                tag: Tag(3),
                circle: Geometry(9),
                value: three,
            },
        ] => {
            assert_eq!(t.system.values[half.0 as usize], 20_000_001.0 / 1e6 / 2.0);
            assert_eq!(t.system.values[three.0 as usize], 3.0);
        }
        ref other => panic!("{other:?}"),
    }
}

#[test]
fn an_arcs_radius_and_angles_are_worked_out_from_its_points() {
    // The arc a1 about (5, 10) from (10, 0) to (−5, 10): radius √125, from atan2(−10, 5) to
    // atan2(0, −10) = π.
    let t = translate(&shapes(&[]));
    let SketchGeometry::Arc {
        radius,
        start_angle,
        end_angle,
        ..
    } = t.system.geometry[9]
    else {
        panic!()
    };
    let v = |x: Value| t.system.values[x.0 as usize];
    assert_eq!(v(radius), 125f64.sqrt());
    assert_eq!(v(start_angle), libm::atan2(-10.0, 5.0));
    assert_eq!(v(end_angle), PI);
    assert!(t.system.unknowns.contains(&radius) && t.system.unknowns.contains(&end_angle));
    assert!(
        t.system
            .constraints
            .contains(&SolverConstraint::ArcRules { arc: Geometry(9) })
    );
}

#[test]
fn an_arc_whose_ends_are_at_one_angle_is_refused_naming_it() {
    // From (10, 0) to (15, 0) about (5, 0): both at angle 0.
    let e = Translation::new(&sketch_of(&text(
        &[
            point("p0000001", 5.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 15.0, 0.0),
        ],
        &[],
        &[],
        &[arc("a0000001", "p0000001", "p0000002", "p0000003")],
        &[],
    )))
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput);
    assert_eq!(e.elements(), ["a0000001"]);
    assert_eq!(
        e.message(),
        "arc a0000001's start and end are at one angle from its centre, so it sweeps nothing, or a whole turn"
    );
}

#[test]
fn a_symmetry_of_two_points_drawn_at_one_place_is_refused_naming_it() {
    let e = Translation::new(&sketch_of(&text(
        &[
            point("p0000001", 3.0, 4.0),
            point("p0000002", 3.0, 4.0),
            point("p0000003", 0.0, 0.0),
        ],
        &[],
        &[],
        &[],
        &[r#"{"id": "k0000001", "type": "symmetric", "a": "p0000001", "b": "p0000002", "about": "p0000003"}"#.to_owned()],
    )))
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput);
    assert_eq!(e.elements(), ["k0000001"]);
    assert_eq!(
        e.message(),
        "a symmetry of two points drawn at one place, (3, 4): the solver measures it by the line between them"
    );
}

// ---------------------------------------------------------------------------------------
// Reading what the worker found.

/// What a worker that solved `t` to `values` would answer.
fn answer(t: &Translation, values: Vec<f64>, status: SolveStatus) -> Solved {
    let mut tags: Vec<Tag> = sent(t).iter().filter_map(|c| c.tag()).collect();
    tags.sort_unstable();
    let solution = matches!(
        status,
        SolveStatus::Success | SolveStatus::SolvedExceptRedundant | SolveStatus::SolvedInvalid
    );
    Solved {
        dofs: 0,
        conflicting: Vec::new(),
        redundant: Vec::new(),
        partially_redundant: Vec::new(),
        free: Vec::new(),
        no_equations: false,
        status,
        algorithm: solution.then_some(Algorithm::DogLeg),
        values,
        residuals: tags.into_iter().map(|t| (t, Some(0.0))).collect(),
    }
}

fn read_back(t: &Translation, solved: &Solved) -> Result<SketchSolution, silico_core::Error> {
    t.check_reply(solved)?;
    t.read(solved)
}

/// The line n1 from p1 (0, 0) to p2 (10, 0), horizontal and 10 long, with these
/// constraints after.
fn ten(constraints: &[&str]) -> String {
    let mut all = vec![
        r#"{"id": "k0000001", "type": "horizontal", "line": "n0000001"}"#,
        r#"{"id": "k0000002", "type": "length", "line": "n0000001", "value": 10, "driving": true}"#,
    ];
    all.extend(constraints);
    text(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 4.0, 3.0),
        ],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        &all.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>(),
    )
}

#[test]
fn a_solution_is_read_back_by_id() {
    let t = translate(&ten(&[]));
    let mut values = t.system.values.clone();
    values[11] = 4.5; // p3's y, moved by the solve
    let solution = read_back(&t, &answer(&t, values, SolveStatus::Success)).unwrap();
    let id = |s: &str| silico_core::document::ElementId::new(s).unwrap();
    assert_eq!(solution.points[&id("p0000003")], [4.0, 4.5]);
    assert_eq!(solution.points[&id("p0000002")], [10.0, 0.0]);
    assert_eq!(solution.algorithm, Algorithm::DogLeg);
    assert!(solution.redundant.is_empty() && solution.references.is_empty());
}

#[test]
fn reference_dimensions_are_measured_on_the_solution() {
    // A reference length and horizontal distance of the line, p3's distance from it (on its
    // left, 3 away), and the angle from it to the y axis, stored on several turns: 1.1.3
    // keeps a reference angle on its stored value's turn, then reduces it with fmod.
    let t = translate(&ten(&[
        r#"{"id": "r0000001", "type": "length", "line": "n0000001", "value": 1, "driving": false}"#,
        r#"{"id": "r0000002", "type": "distance_x", "a": "p0000002", "b": "p0000001", "value": 0, "driving": false}"#,
        r#"{"id": "r0000003", "type": "point_line_distance", "point": "p0000003", "line": "n0000001", "value": 0, "side": "right", "driving": false}"#,
        r#"{"id": "r0000004", "type": "angle", "a": "n0000001", "b": "y_axis", "value": 90, "driving": false}"#,
        r#"{"id": "r0000005", "type": "angle", "a": "n0000001", "b": "y_axis", "value": -270, "driving": false}"#,
        r#"{"id": "r0000006", "type": "angle", "a": "n0000001", "b": "y_axis", "value": 359, "driving": false}"#,
        r#"{"id": "r0000007", "type": "angle", "a": "y_axis", "b": "n0000001", "value": 10, "driving": false}"#,
    ]));
    // Nothing reference is sent: the two driving constraints only.
    assert_eq!(sent(&t).len(), 2);
    let solution = read_back(
        &t,
        &answer(&t, t.system.values.clone(), SolveStatus::Success),
    )
    .unwrap();
    let r = |s: &str| solution.references[&silico_core::document::ElementId::new(s).unwrap()];
    assert_eq!(r("r0000001"), Measured::Length(10.0));
    assert_eq!(r("r0000002"), Measured::Length(-10.0));
    assert_eq!(
        r("r0000003"),
        Measured::FromLine {
            distance: 3.0,
            side: LineSide::Left
        }
    );
    // 90° measured: stored 90 → 90; stored −270, a turn below → −270; stored 359, nearest
    // 90 + 360 = 450 → reduced to 90; from the y axis to the line, −90, near 10 → −90.
    let angle = |s: &str| match r(s) {
        Measured::Angle(a) => a,
        other => panic!("{other:?}"),
    };
    for (id, want) in [
        ("r0000004", 90.0),
        ("r0000005", -270.0),
        ("r0000006", 90.0),
        ("r0000007", -90.0),
    ] {
        assert!((angle(id) - want).abs() < 1e-12, "{id}: {}", angle(id));
    }
}

#[test]
fn conflicting_constraints_are_over_constrained_naming_them() {
    let t = translate(&ten(&[
        r#"{"id": "k0000003", "type": "length", "line": "n0000001", "value": 11, "driving": true}"#,
    ]));
    let mut solved = answer(&t, t.system.values.clone(), SolveStatus::Success);
    solved.conflicting = vec![Tag(2), Tag(3)];
    let e = read_back(&t, &solved).unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained);
    assert_eq!(e.elements(), ["k0000002", "k0000003"]);
}

#[test]
fn a_solve_that_fails_is_solve_failed_and_one_on_unbuildable_geometry_names_it() {
    let t = translate(&ten(&[]));
    for status in [SolveStatus::Failed, SolveStatus::Converged] {
        let e = read_back(&t, &answer(&t, t.system.values.clone(), status)).unwrap_err();
        assert_eq!(e.code(), ErrorCode::SolveFailed, "{status:?}");
    }
    // The line collapsed onto its start.
    let mut values = t.system.values.clone();
    values[8] = 0.0;
    let e = read_back(&t, &answer(&t, values, SolveStatus::SolvedInvalid)).unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidResult);
    assert_eq!(e.elements(), ["n0000001"]);
}

#[test]
fn redundant_constraints_that_agree_are_kept_and_named_and_ones_a_nanometre_off_conflict() {
    // A second length of 10 (agrees), or of 10.000001 (a nanometre off): PlaneGCS drops
    // either as redundant (its own tolerance is 1e-5); decision 5 keeps only the first.
    for (value, agrees) in [("10", true), ("10.000001", false)] {
        let c = format!(
            r#"{{"id": "k0000003", "type": "length", "line": "n0000001", "value": {value}, "driving": true}}"#
        );
        let t = translate(&ten(&[&c]));
        let mut solved = answer(
            &t,
            t.system.values.clone(),
            SolveStatus::SolvedExceptRedundant,
        );
        solved.redundant = vec![Tag(3)];
        let read = read_back(&t, &solved);
        if agrees {
            let id = silico_core::document::ElementId::new("k0000003").unwrap();
            assert_eq!(
                read.unwrap().redundant.into_iter().collect::<Vec<_>>(),
                [id]
            );
        } else {
            let e = read.unwrap_err();
            assert_eq!(e.code(), ErrorCode::OverConstrained);
            assert_eq!(e.elements(), ["k0000003"]);
        }
    }
}

#[test]
fn decision_fives_tolerances_are_a_hundredth_of_a_nanometre_and_a_nanoradian() {
    // A redundant horizontal distance of 10, on a solution with the line's end just within,
    // and just past, 1e-8 mm of it; a redundant horizontal on one with the line turned just
    // within, and past, 1e-9 rad.
    for (off, agrees) in [(0.9e-8, true), (1.1e-8, false)] {
        let t = translate(&ten(&[
            r#"{"id": "k0000003", "type": "distance_x", "a": "p0000001", "b": "p0000002", "value": 10, "driving": true}"#,
        ]));
        let mut values = t.system.values.clone();
        values[8] = 10.0 + off;
        let mut solved = answer(&t, values, SolveStatus::SolvedExceptRedundant);
        solved.partially_redundant = vec![Tag(3)];
        assert_eq!(read_back(&t, &solved).is_ok(), agrees, "{off}");
    }
    for (off, agrees) in [(0.9e-9, true), (1.1e-9, false)] {
        // The line turned off horizontal by `off` radians at its end; the horizontal is the
        // redundant one.
        let t = translate(&ten(&[]));
        let mut values = t.system.values.clone();
        values[8] = 10.0 * libm::cos(off);
        values[9] = 10.0 * libm::sin(off);
        let mut solved = answer(&t, values, SolveStatus::SolvedExceptRedundant);
        solved.redundant = vec![Tag(1)];
        assert_eq!(read_back(&t, &solved).is_ok(), agrees, "{off}");
    }
}

#[test]
fn a_solution_on_the_wrong_side_of_a_line_is_an_invalid_result_naming_the_constraint() {
    // p3 is 3 left of the line; held 3 to its right, a solve that keeps it left is refused
    // (1.1.3's distance has no side), and one that moves it right isn't.
    let t = translate(&ten(&[
        r#"{"id": "k0000003", "type": "point_line_distance", "point": "p0000003", "line": "n0000001", "value": 3, "side": "right", "driving": true}"#,
    ]));
    let e = read_back(
        &t,
        &answer(&t, t.system.values.clone(), SolveStatus::Success),
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidResult);
    assert_eq!(e.elements(), ["k0000003"]);
    let mut values = t.system.values.clone();
    values[11] = -3.0;
    read_back(&t, &answer(&t, values, SolveStatus::Success)).unwrap();
}

#[test]
fn a_reply_that_doesnt_answer_the_request_is_a_protocol_error() {
    let t = translate(&ten(&[]));
    let good = answer(&t, t.system.values.clone(), SolveStatus::Success);
    type Spoil = Box<dyn Fn(&mut Solved)>;
    let spoilt: [(&str, Spoil); 6] = [
        (
            "short",
            Box::new(|s| {
                s.values.pop();
            }),
        ),
        ("a fixed value moved", Box::new(|s| s.values[12] = 11.0)),
        ("a tag not sent", Box::new(|s| s.conflicting = vec![Tag(9)])),
        ("free fixed value", Box::new(|s| s.free = vec![Value(0)])),
        (
            "residuals missing",
            Box::new(|s| {
                s.residuals.pop();
            }),
        ),
        ("no algorithm", Box::new(|s| s.algorithm = None)),
    ];
    t.check_reply(&good).unwrap();
    for (what, spoil) in spoilt {
        let mut s = good.clone();
        spoil(&mut s);
        assert_eq!(
            t.check_reply(&s).unwrap_err().code(),
            ErrorCode::ProtocolError,
            "{what}"
        );
    }
}

/// A stand-in for the worker: answers with `answer`, made from the system it's sent.
struct Standing(Box<dyn Fn(&SketchSystem) -> Solved>);

impl SketchSolver for Standing {
    fn solve(&mut self, system: SketchSystem) -> Result<Solved, silico_core::Error> {
        Ok((self.0)(&system))
    }
}

#[test]
fn solving_a_documents_sketch_names_the_feature_in_its_errors() {
    let text = ten(&[]);
    let checked = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    let feature = FeatureId::new(fixture::FEATURE).unwrap();
    let mut fails = Standing(Box::new(|s: &SketchSystem| {
        let t = Translation::new(&sketch_of(&ten(&[]))).unwrap();
        assert_eq!(&t.system, s);
        answer(&t, s.values.clone(), SolveStatus::Failed)
    }));
    let e = solve(&checked, &feature, &mut fails).unwrap_err();
    assert_eq!(
        (e.code(), e.feature()),
        (ErrorCode::SolveFailed, Some(fixture::FEATURE))
    );
    let mut solves = Standing(Box::new(|s: &SketchSystem| {
        let t = Translation::new(&sketch_of(&ten(&[]))).unwrap();
        answer(&t, s.values.clone(), SolveStatus::Success)
    }));
    assert_eq!(
        solve(&checked, &feature, &mut solves).unwrap().points.len(),
        3
    );
    let missing = FeatureId::new("zzzzzzzz").unwrap();
    assert_eq!(
        solve(&checked, &missing, &mut solves).unwrap_err().code(),
        ErrorCode::InvalidInput
    );
}
