//! M3.2's gate, rule by rule: each test fails on a wrong version of a rule that the other
//! sketch tests pass (found by the review's mutations), and on each rule added by the review.
//! Counts and limits are worked out by hand from the solver's limits.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture;

use fixture::{
    ARC, CIRCLE, FEATURE, LINE, POINTS, constrained, document, fields, line_as, point_debug,
    problems,
};
use silico_core::ErrorCode;
use silico_core::document::{Operation, Plane, read, write};

/// A second line, nb from pb to pc, beside the base sketch's.
const LINE2: &str =
    r#"{"id": "nb000000", "from": "pb000000", "to": "pc000000", "construction": false}"#;

/// The base sketch with a second line, and these constraints.
fn with_two_lines(c: &str) -> String {
    document(&fields(POINTS, &format!("{LINE}, {LINE2}"), CIRCLE, ARC, c))
}
#[track_caller]
fn only(text: &str, parameter: &str, message: &str) {
    let all = problems(text);
    assert_eq!(all.len(), 1, "{all:#?}");
    assert_eq!(all[0].code(), ErrorCode::InvalidInput);
    assert_eq!(all[0].feature(), Some(FEATURE), "{all:#?}");
    assert_eq!(all[0].parameter(), Some(parameter), "{all:#?}");
    assert!(all[0].message().contains(message), "{all:#?}");
}
#[track_caller]
fn none(text: &str) {
    let all = problems(text);
    assert!(all.is_empty(), "{all:#?}");
}

#[test]
fn each_plane_reads_as_its_own_value() {
    for (text, plane) in [("xy", Plane::Xy), ("xz", Plane::Xz), ("yz", Plane::Yz)] {
        let doc = document(&fields("", "", "", "", "")).replace("\"xy\"", &format!("\"{text}\""));
        let d = read(doc.as_bytes()).unwrap();
        match &d.features[0].operation {
            Operation::Sketch(s) => assert_eq!(s.plane, plane, "{text}"),
            other => panic!("{other:?}"),
        }
        assert!(
            write(&d)
                .unwrap()
                .contains(&format!("\"plane\": \"{text}\""))
        );
    }
}

#[test]
fn an_arcs_radius_past_a_kilometre_along_y_is_refused() {
    let text = document(&fields(
        r#"{"id": "pa000000", "x": 0, "y": -1000000, "construction": false}, {"id": "pb000000", "x": 0, "y": 1000000, "construction": false}, {"id": "pc000000", "x": 1000000, "y": -1000000, "construction": false}"#,
        "",
        "",
        ARC,
        "",
    ));
    only(&text, "arcs.aa000000.start", "more than the 1000000 mm");
}

#[test]
fn an_arcs_start_or_end_drawn_at_its_center_is_refused() {
    let pd = r#"{"id": "pd000000", "x": 0, "y": 0, "construction": false}"#;
    for (key, arc) in [
        (
            "start",
            r#"{"id": "aa000000", "center": "pa000000", "start": "pd000000", "end": "pc000000", "construction": false}"#,
        ),
        (
            "end",
            r#"{"id": "aa000000", "center": "pa000000", "start": "pb000000", "end": "pd000000", "construction": false}"#,
        ),
    ] {
        let text = document(&fields(&format!("{POINTS}, {pd}"), "", "", arc, ""));
        only(
            &text,
            &format!("arcs.aa000000.{key}"),
            &format!(
                "arc aa000000's {key}, point pd000000 at (0, 0), is 0 mm from its center, point pa000000 at (0, 0): arcs and circles have a radius above 1e-7 mm"
            ),
        );
    }
}

#[test]
fn tangent_at_an_end_of_a_alone_is_refused_naming_b() {
    // pa is na's start; aa runs from pb to pc (pa is its center, not an end).
    only(
        &with_two_lines(
            r#"{"id": "k0000000", "type": "tangent_at", "a": "na000000", "b": "aa000000", "at": "pa000000", "direction": "same"}"#,
        ),
        "constraints.k0000000.at",
        "isn't an end of its b, aa000000",
    );
}

#[test]
fn two_rounds_tangent_outside_or_inside_are_admitted() {
    for side in ["outside", "inside"] {
        none(&with_two_lines(&format!(
            r#"{{"id": "k0000000", "type": "tangent", "a": "aa000000", "b": "ca000000", "side": "{side}"}}"#
        )));
    }
}

#[test]
fn two_lines_equal_are_admitted() {
    none(&with_two_lines(
        r#"{"id": "k0000000", "type": "equal", "a": "na000000", "b": "nb000000"}"#,
    ));
}

#[test]
fn symmetric_about_its_b_or_a_line_through_one_of_them_is_refused() {
    only(
        &with_two_lines(
            r#"{"id": "k0000000", "type": "symmetric", "a": "pb000000", "b": "pc000000", "about": "pc000000"}"#,
        ),
        "constraints.k0000000.about",
        "is one of the points it mirrors",
    );
    // FreeCAD refuses only both ends of the line (CommandConstraints.cpp, GeoId1 == GeoId2 ==
    // GeoId3), and solves one end's mirror onto that end, two points at one place. With
    // shared points that's a joint the sketch doesn't have, which the next solve refuses
    // (M3.3's break-it): refused here, naming the end.
    only(
        &with_two_lines(
            r#"{"id": "k0000000", "type": "symmetric", "a": "pa000000", "b": "pc000000", "about": "na000000"}"#,
        ),
        "constraints.k0000000.about",
        "a symmetric's pa000000 is an end of na000000, so it mirrors onto itself",
    );
}

#[test]
fn an_arcs_own_end_on_the_arc_is_refused() {
    for p in ["pb000000", "pc000000"] {
        only(
            &with_two_lines(&format!(
                r#"{{"id": "k0000000", "type": "on", "point": "{p}", "curve": "aa000000"}}"#
            )),
            "constraints.k0000000.point",
            "is part of aa000000 itself",
        );
    }
}

// E9: each arc is three unknowns.
#[test]
fn arcs_count_three_unknowns_each() {
    // 3 points (6) and 340 arcs (1,020): 1,026, past 1,024; 339 arcs, 1,023, admitted.
    let arcs = |n: usize| -> String {
        (0..n).map(|i| format!(r#"{{"id": "a{i:07}", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}}"#)).collect::<Vec<_>>().join(",")
    };
    only(
        &document(&fields(POINTS, "", "", &arcs(340), "")),
        "points",
        "the sketch has 1026 unknowns",
    );
    none(&document(&fields(POINTS, "", "", &arcs(339), "")));
}

// E9: each arc's rules count as a constraint.
#[test]
fn arcs_count_as_constraints() {
    // 339 arcs and 9,662 constraints: 10,001.
    let arcs: Vec<String> = (0..339).map(|i| format!(r#"{{"id": "a{i:07}", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}}"#)).collect();
    let cs: Vec<String> = (0..9662)
        .map(|i| format!(r#"{{"id": "k{i:07}", "type": "horizontal", "line": "na000000"}}"#))
        .collect();
    only(
        &document(&fields(POINTS, LINE, "", &arcs.join(","), &cs.join(","))),
        "constraints",
        "the sketch has 10001 constraints",
    );
}

#[test]
fn the_schema_refuses_sketches_the_reader_refuses_by_shape() {
    let schema: serde_json::Value =
        serde_json::from_str(&silico_core::document::schema().unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    // With the header the schema requires (`naming`, M4.0), which the reader also takes
    // away, as written before it.
    let base = with_two_lines(
        r#"{"id": "k0000000", "type": "horizontal", "line": "na000000"}, {"id": "k0000001", "type": "length", "line": "na000000", "value": 3, "driving": true}"#,
    )
    .replacen(r#""units": "#, r#""naming": 5, "units": "#, 1);
    assert!(base.contains(r#""naming": 5"#));
    assert!(read(base.as_bytes()).is_ok());
    assert!(validator.is_valid(&serde_json::from_str(&base).unwrap()));
    let mut admitted = Vec::new();
    for (from, to) in [
        (
            r#""line": "na000000"}"#,
            r#""line": "na000000", "value": 3}"#,
        ),
        (r#", "driving": true}"#, "}"),
        (r#""type": "horizontal""#, r#""type": "coincident""#),
        (r#""plane": "xy""#, r#""plane": "XY""#),
        (r#""x": 10, "#, r#""x": 10, "z": 0, "#),
        (r#""x": 10, "#, ""),
        (r#""id": "pa000000""#, r#""id": "PA000000""#),
        (r#""from": "pa000000""#, r#""from": "origin""#),
        (
            r#""construction": false}, {"id": "pb"#,
            r#""construction": 0}, {"id": "pb"#,
        ),
        (r#""value": 3"#, r#""value": "3""#),
        (r#""line": "na000000"}"#, r#""line": "x-axis"}"#),
        (r#""radius": 5, "#, ""),
        (r#", "arcs": ["#, r#", "arcz": ["#),
    ] {
        let text = base.replacen(from, to, 1);
        assert_ne!(text, base, "{from}");
        assert!(read(text.as_bytes()).is_err(), "reader takes {to}");
        if validator.is_valid(&serde_json::from_str(&text).unwrap()) {
            admitted.push(to);
        }
    }
    assert!(
        admitted.is_empty(),
        "the schema takes what the reader refuses: {admitted:#?}"
    );
}

#[test]
fn a_tangent_between_curves_sharing_an_end_is_a_tangent_at() {
    // The line ends at pb, where the arc starts: FreeCAD makes that a tangency at the point
    // (180d722a53); an edge tangency there is redundant where it holds.
    only(
        &with_two_lines(
            r#"{"id": "k0000000", "type": "tangent", "a": "na000000", "b": "aa000000", "side": "left"}"#,
        ),
        "constraints.k0000000.b",
        "a tangent's na000000 and aa000000 share an end, point pb000000",
    );
    let error = &problems(&with_two_lines(
        r#"{"id": "k0000000", "type": "tangent", "a": "na000000", "b": "aa000000", "side": "left"}"#,
    ))[0];
    assert_eq!(
        error.suggestion(),
        Some("make it a tangent_at, at point pb000000")
    );
    // A circle has no ends, so a line through its center can be tangent to it.
    none(&with_two_lines(
        r#"{"id": "k0000000", "type": "tangent", "a": "na000000", "b": "ca000000", "side": "left"}"#,
    ));
}

#[test]
fn a_point_line_distance_from_the_lines_own_end_is_refused() {
    only(
        &with_two_lines(
            r#"{"id": "k0000000", "type": "point_line_distance", "point": "pb000000", "line": "na000000", "value": 3, "side": "left", "driving": true}"#,
        ),
        "constraints.k0000000.point",
        "a point_line_distance's point, pb000000, is part of na000000 itself",
    );
}

#[test]
fn reference_dimensions_take_what_a_solve_can_measure() {
    // Driving: above 0 and at most a kilometre. Reference: from 0, and up to 3 km, past the
    // 2√2 km (2828427.124746... mm) two points within a kilometre can be apart.
    for (constraint, problem) in [
        (
            r#"{"id": "k0000000", "type": "distance", "a": "pa000000", "b": "pb000000", "value": 0, "driving": false}"#,
            None,
        ),
        (
            r#"{"id": "k0000000", "type": "point_line_distance", "point": "pc000000", "line": "na000000", "value": 0, "side": "left", "driving": false}"#,
            None,
        ),
        (
            r#"{"id": "k0000000", "type": "distance", "a": "pa000000", "b": "pb000000", "value": 2828427.124747, "driving": false}"#,
            None,
        ),
        (
            r#"{"id": "k0000000", "type": "length", "line": "na000000", "value": 3000000, "driving": false}"#,
            None,
        ),
        (
            r#"{"id": "k0000000", "type": "length", "line": "na000000", "value": 3000000.000001, "driving": false}"#,
            Some(
                "a reference length's value is 3000000.000001 mm, but it must be from 0 to 3000000 mm",
            ),
        ),
        (
            r#"{"id": "k0000000", "type": "radius", "curve": "ca000000", "value": -0.000001, "driving": false}"#,
            Some("a reference radius's value is -0.000001 mm, but it must be from 0 to 3000000 mm"),
        ),
        (
            r#"{"id": "k0000000", "type": "distance_x", "a": "pa000000", "b": "pb000000", "value": -3000000, "driving": false}"#,
            None,
        ),
        (
            r#"{"id": "k0000000", "type": "distance_x", "a": "pa000000", "b": "pb000000", "value": -3000000.000001, "driving": false}"#,
            Some(
                "a reference distance_x's value is -3000000.000001 mm, but it must be at most 3000000 mm either way",
            ),
        ),
        (
            r#"{"id": "k0000000", "type": "distance", "a": "pa000000", "b": "pb000000", "value": 0, "driving": true}"#,
            Some("a distance's value is 0 mm, but it must be above 0 and at most 1000000 mm"),
        ),
    ] {
        match problem {
            None => none(&with_two_lines(constraint)),
            Some(message) => only(
                &with_two_lines(constraint),
                "constraints.k0000000.value",
                message,
            ),
        }
    }
}

#[test]
fn geometry_the_solver_measures_as_no_size_is_refused() {
    // 1e-200 squared underflows to 0: to the solver, a line of no length and an arc of no
    // radius, though the points are at different places. Far inside the kernel's 1e-7 mm,
    // they're refused by its rule.
    let tiny = r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 0, "y": 1e-200, "construction": false}, {"id": "pc000000", "x": 1e-200, "y": 0, "construction": false}, {"id": "pd000000", "x": 0, "y": 10, "construction": false}"#;
    only(
        &document(&fields(
            tiny,
            r#"{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}"#,
            "",
            "",
            "",
        )),
        "lines.na000000.to",
        "line na000000 is 1e-200 mm long, from point pa000000 at (0, 0) to pb000000 at (0, 1e-200): lines and arcs are longer than 1e-7 mm between their ends",
    );
    only(
        &document(&fields(
            tiny,
            "",
            "",
            r#"{"id": "aa000000", "center": "pa000000", "start": "pc000000", "end": "pd000000", "construction": false}"#,
            "",
        )),
        "arcs.aa000000.start",
        "arc aa000000's start, point pc000000 at (1e-200, 0), is 1e-200 mm from its center, point pa000000 at (0, 0)",
    );
    // A line from 0 to -0 is the same place: no length.
    only(
        &document(&fields(
            r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": -0, "y": 0, "construction": false}"#,
            r#"{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}"#,
            "",
            "",
            "",
        )),
        "lines.na000000.to",
        "line na000000 is 0 mm long, from point pa000000 at (0, 0) to pb000000 at (-0, 0)",
    );
}

#[test]
fn an_arc_whose_start_and_end_are_drawn_at_one_place_is_refused() {
    let points = r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pc000000", "x": 10, "y": 0, "construction": false}"#;
    only(
        &document(&fields(points, "", "", ARC, "")),
        "arcs.aa000000.end",
        "arc aa000000's start and end, points pb000000 and pc000000, are both drawn at (10, 0)",
    );
}

/// pd (-20, 20), pe (20, 20), pf (-10, 20), pg (-20, 30), beside the base sketch's points.
const MORE_POINTS: &str = r#"{"id": "pd000000", "x": -20, "y": 20, "construction": false}, {"id": "pe000000", "x": 20, "y": 20, "construction": false}, {"id": "pf000000", "x": -10, "y": 20, "construction": false}, {"id": "pg000000", "x": -20, "y": 30, "construction": false}"#;

/// The base points and these, with these lines, the base arc and these arcs, and these
/// constraints.
fn more(lines: &str, arcs: &str, constraints: &str) -> String {
    let arcs = if arcs.is_empty() {
        ARC.to_owned()
    } else {
        format!("{ARC}, {arcs}")
    };
    document(&fields(
        &format!("{POINTS}, {MORE_POINTS}"),
        lines,
        "",
        &arcs,
        constraints,
    ))
}

#[test]
fn a_tangent_between_curves_that_dont_meet_is_admitted() {
    // A line from pd to pe and the base arc: no end in common.
    none(&more(
        r#"{"id": "nd000000", "from": "pd000000", "to": "pe000000", "construction": false}"#,
        "",
        r#"{"id": "k0000000", "type": "tangent", "a": "nd000000", "b": "aa000000", "side": "right"}"#,
    ));
    // Two arcs, about pa and about pd, apart.
    none(&more(
        "",
        r#"{"id": "ab000000", "center": "pd000000", "start": "pf000000", "end": "pg000000", "construction": false}"#,
        r#"{"id": "k0000000", "type": "tangent", "a": "aa000000", "b": "ab000000", "side": "outside"}"#,
    ));
}

#[test]
fn a_tangent_names_the_end_its_curves_share_whichever_ends_they_are() {
    // The arc runs pb to pc. A line starting at the arc's end, then one ending at its start.
    for (line, point) in [
        (
            r#"{"id": "nd000000", "from": "pc000000", "to": "pd000000", "construction": false}"#,
            "pc000000",
        ),
        (
            r#"{"id": "nd000000", "from": "pd000000", "to": "pb000000", "construction": false}"#,
            "pb000000",
        ),
    ] {
        only(
            &more(
                line,
                "",
                r#"{"id": "k0000000", "type": "tangent", "a": "nd000000", "b": "aa000000", "side": "left"}"#,
            ),
            "constraints.k0000000.b",
            &format!("a tangent's nd000000 and aa000000 share an end, point {point}"),
        );
    }
}

#[test]
fn a_curve_tangent_to_itself_is_told_it_needs_two() {
    only(
        &constrained(
            r#"{"id": "k0000000", "type": "tangent", "a": "aa000000", "b": "aa000000", "side": "outside"}"#,
        ),
        "constraints.k0000000.b",
        "a tangent's a and b are both aa000000: it needs two different elements",
    );
}

#[test]
fn each_tangent_side_reads_as_its_own_value_and_writes_back() {
    use silico_core::document::{Form, TangentSide};
    for (word, side) in [
        ("outside", TangentSide::Outside),
        ("inside", TangentSide::Inside),
    ] {
        let text = more(
            "",
            r#"{"id": "ab000000", "center": "pd000000", "start": "pf000000", "end": "pg000000", "construction": false}"#,
            &format!(
                r#"{{"id": "k0000000", "type": "tangent", "a": "aa000000", "b": "ab000000", "side": "{word}"}}"#
            ),
        );
        let d = read(text.as_bytes()).unwrap();
        let Operation::Sketch(sketch) = &d.features[0].operation else {
            panic!()
        };
        assert!(
            matches!(sketch.constraints[0].form, Form::Tangent { side: s, .. } if s == side),
            "{word}"
        );
        assert!(
            write(&d)
                .unwrap()
                .contains(&format!("\"side\": \"{word}\""))
        );
    }
}

#[test]
fn geometry_a_hair_apart_has_no_size_to_the_kernel() {
    // 1e-150 squared is 1e-300, not 0, so the solver would measure a line and an arc's
    // radius; but they're far inside the kernel's 1e-7 mm, one place to it, and refused:
    // the line (lines.na000000.to), and the arc's start and end, each 1e-150 from its center.
    let tiny = three_points((1e-150, 0.0), (0.0, 1e-150));
    let all = problems(&document(&fields(&tiny, LINE, "", ARC, "")));
    let refused: Vec<Option<&str>> = all.iter().map(|e| e.parameter()).collect();
    assert_eq!(
        refused,
        [
            Some("lines.na000000.to"),
            Some("arcs.aa000000.start"),
            Some("arcs.aa000000.end")
        ],
        "{all:#?}"
    );
    // The arc's end, 1e-200 from its center, is refused by the same rule.
    let end = r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pc000000", "x": 1e-200, "y": 0, "construction": false}"#;
    only(
        &document(&fields(end, "", "", ARC, "")),
        "arcs.aa000000.end",
        "arc aa000000's end, point pc000000 at (1e-200, 0), is 1e-200 mm from its center, point pa000000 at (0, 0): arcs and circles have a radius above 1e-7 mm, as the kernel measures them",
    );
}

/// pa at the origin, pb and pc where given.
fn three_points(b: (f64, f64), c: (f64, f64)) -> String {
    [
        point_debug("pa000000", 0.0, 0.0),
        point_debug("pb000000", b.0, b.1),
        point_debug("pc000000", c.0, c.1),
    ]
    .join(", ")
}

#[test]
fn a_drawn_size_at_most_the_kernels_tolerance_is_refused_and_above_it_admitted() {
    // The kernel's 1e-7 mm (OCCT's Precision::Confusion), inclusive, measured as the
    // profile measures a curve it sends (`Curve::size_problem`, libm's hypot).
    let far = (0.0, 10.0);
    let line = |b: (f64, f64), construction: bool| {
        document(&fields(
            &three_points(b, far),
            &line_as("na000000", "pa000000", "pb000000", construction),
            "",
            "",
            "",
        ))
    };
    // A line along x: 1e-7 long refused, the next float up admitted.
    only(
        &line((1e-7, 0.0), false),
        "lines.na000000.to",
        "line na000000 is 1e-7 mm long, from point pa000000 at (0, 0) to pb000000 at (1e-7, 0): lines and arcs are longer than 1e-7 mm between their ends, as the kernel measures them",
    );
    let error = &problems(&line((1e-7, 0.0), false))[0];
    assert_eq!(error.elements(), ["pa000000", "pb000000", "na000000"]);
    assert_eq!(
        error.suggestion(),
        Some("draw the line's ends further apart")
    );
    none(&line((1e-7f64.next_up(), 0.0), false));
    // On a slant, 3-4-5: (0.594e-7, 0.792e-7) is 0.99e-7 long; (0.606e-7, 0.808e-7)
    // 1.01e-7.
    only(
        &line((0.594e-7, 0.792e-7), false),
        "lines.na000000.to",
        "mm long",
    );
    none(&line((0.606e-7, 0.808e-7), false));
    // Construction lines alike.
    only(&line((1e-7, 0.0), true), "lines.na000000.to", "mm long");

    // A circle's radius: 1e-7 refused, 1e-6 admitted.
    let circle = |radius: f64| {
        document(&fields(
            POINTS,
            "",
            &fixture::circle("ca000000", "pa000000", radius),
            "",
            "",
        ))
    };
    only(
        &circle(1e-7),
        "circles.ca000000.radius",
        "circle ca000000's radius is 1e-7 mm: arcs and circles have a radius above 1e-7 mm, as the kernel measures them",
    );
    assert_eq!(problems(&circle(1e-7))[0].elements(), ["ca000000"]);
    none(&circle(0.000001));

    // An arc about pa from pb to pc: its start 1e-7 from its center refused, 1.01e-7
    // admitted; its end likewise.
    let arc =
        |b: (f64, f64), c: (f64, f64)| document(&fields(&three_points(b, c), "", "", ARC, ""));
    only(
        &arc((1e-7, 0.0), far),
        "arcs.aa000000.start",
        "arc aa000000's start, point pb000000 at (1e-7, 0), is 1e-7 mm from its center",
    );
    // Each names the point, the center, and the arc.
    assert_eq!(
        problems(&arc((1e-7, 0.0), far))[0].elements(),
        ["pb000000", "pa000000", "aa000000"]
    );
    none(&arc((1.01e-7, 0.0), (0.0, 1.01e-7)));
    only(
        &arc((10.0, 0.0), (0.0, 1e-7)),
        "arcs.aa000000.end",
        "arc aa000000's end, point pc000000 at (0, 1e-7), is 1e-7 mm from its center",
    );
    assert_eq!(
        problems(&arc((10.0, 0.0), (0.0, 1e-7)))[0].elements(),
        ["pc000000", "pa000000", "aa000000"]
    );
    none(&arc((10.0, 0.0), (0.0, 1.01e-7)));
    // Its chord: from (10, 0) to (10, 1e-8), 1e-8 apart, refused as a circle; to
    // (10, 1.01e-7) admitted.
    only(
        &arc((10.0, 0.0), (10.0, 1e-8)),
        "arcs.aa000000.end",
        "arc aa000000's ends, points pb000000 and pc000000, are 1e-8 mm apart: lines and arcs are longer than 1e-7 mm between their ends, as the kernel measures them",
    );
    let chord = &problems(&arc((10.0, 0.0), (10.0, 1e-8)))[0];
    assert_eq!(
        chord.suggestion(),
        Some("draw a circle instead, or open the arc")
    );
    assert_eq!(chord.elements(), ["pb000000", "pc000000", "aa000000"]);
    none(&arc((10.0, 0.0), (10.0, 1.01e-7)));
}

#[test]
fn a_reference_diameter_takes_up_to_three_kilometres_too() {
    none(&constrained(
        r#"{"id": "k0000000", "type": "diameter", "curve": "ca000000", "value": 3000000, "driving": false}"#,
    ));
    only(
        &constrained(
            r#"{"id": "k0000000", "type": "diameter", "curve": "ca000000", "value": 3000000.000001, "driving": false}"#,
        ),
        "constraints.k0000000.value",
        "a reference diameter's value is 3000000.000001 mm, but it must be from 0 to 3000000 mm",
    );
}

/// A circle ca of radius 5 about pa, a point pt (0, 5) on its top, and a line nz ending
/// there from pz (-20, 5).
fn ending_on_the_circle(constraints: &str) -> String {
    document(&fields(
        r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pt000000", "x": 0, "y": 5, "construction": false}, {"id": "pz000000", "x": -20, "y": 5, "construction": false}"#,
        r#"{"id": "nz000000", "from": "pz000000", "to": "pt000000", "construction": false}"#,
        CIRCLE,
        "",
        constraints,
    ))
}

#[test]
fn a_curve_ending_on_another_and_tangent_there_is_one_tangent_on() {
    // FreeCAD's endpoint-to-edge tangency (3a656d8597): the line ends on the circle and runs
    // along it there.
    none(&ending_on_the_circle(
        r#"{"id": "k0000000", "type": "tangent_on", "a": "nz000000", "at": "pt000000", "b": "ca000000", "direction": "same"}"#,
    ));
    // The same as an `on` and a `tangent`: redundant where it holds, so refused, pointing at
    // tangent_on.
    let text = ending_on_the_circle(
        r#"{"id": "k0000000", "type": "on", "point": "pt000000", "curve": "ca000000"}, {"id": "k0000001", "type": "tangent", "a": "nz000000", "b": "ca000000", "side": "right"}"#,
    );
    only(
        &text,
        "constraints.k0000001.b",
        "a tangent's nz000000 ends at point pt000000, which constraint k0000000 puts on ca000000",
    );
    assert_eq!(
        problems(&text)[0].suggestion(),
        Some("make the two one tangent_on, with a nz000000, at pt000000, and b ca000000")
    );
    // Either way round.
    only(
        &ending_on_the_circle(
            r#"{"id": "k0000000", "type": "on", "point": "pt000000", "curve": "ca000000"}, {"id": "k0000001", "type": "tangent", "a": "ca000000", "b": "nz000000", "side": "left"}"#,
        ),
        "constraints.k0000001.b",
        "a tangent's nz000000 ends at point pt000000",
    );
}

#[test]
fn a_curve_ending_on_another_and_tangent_there_is_refused_at_the_later_of_the_two() {
    // The pair is refused once, at whichever comes later in the constraint list: here the
    // `on`, after the tangent, so an edit that appends the `on` (sketch_add) is refused at
    // its own member. The message names the tangent, and the suggestion is the same.
    let text = ending_on_the_circle(
        r#"{"id": "k0000000", "type": "tangent", "a": "nz000000", "b": "ca000000", "side": "right"}, {"id": "k0000001", "type": "on", "point": "pt000000", "curve": "ca000000"}"#,
    );
    only(
        &text,
        "constraints.k0000001.point",
        "an on puts point pt000000, where nz000000 ends, on ca000000, which constraint k0000000 makes nz000000 edge tangent to: a curve ending on another is tangent where it ends",
    );
    let all = problems(&text);
    assert_eq!(all[0].elements(), ["k0000001"]);
    assert_eq!(
        all[0].suggestion(),
        Some("make the two one tangent_on, with a nz000000, at pt000000, and b ca000000")
    );
    // The tangent naming the circle first, its b the line that ends on it.
    only(
        &ending_on_the_circle(
            r#"{"id": "k0000000", "type": "tangent", "a": "ca000000", "b": "nz000000", "side": "left"}, {"id": "k0000001", "type": "on", "point": "pt000000", "curve": "ca000000"}"#,
        ),
        "constraints.k0000001.point",
        "which constraint k0000000 makes nz000000 edge tangent to",
    );
}

#[test]
fn a_curve_tangent_to_itself_makes_no_pair_with_an_on_of_its_end_on_it() {
    // The tangent names nz000000 twice and the on holds nz000000's own end on it: each is
    // refused once, for itself, and the on gets no pair error besides.
    let all = problems(&ending_on_the_circle(
        r#"{"id": "k0000000", "type": "tangent", "a": "nz000000", "b": "nz000000", "side": "right"}, {"id": "k0000001", "type": "on", "point": "pt000000", "curve": "nz000000"}"#,
    ));
    let at = |parameter: &str| {
        all.iter()
            .filter(|e| e.parameter() == Some(parameter))
            .count()
    };
    assert_eq!(at("constraints.k0000001.point"), 1, "{all:?}");
    assert!(
        all.iter().all(|e| !e.message().contains("edge tangent to")),
        "{all:?}"
    );
}

#[test]
fn the_tangent_rule_answers_each_pair_among_thousands_of_tangents_and_ons() {
    // 2,000 lines n_i from p_i (-20, i) to q_i (0, i), each edge tangent to circle ca (k_i)
    // and with its start held on circle cb (m_i): no pair, since each `on` holds the line's
    // end on the other circle. Then line 0's end held on ca before its tangent, and line 1's
    // after: two pairs, refused at the later member of each, k0000000 and m1000001. The
    // sketch is past the solver's unknowns too (4,001 points and two circles: 8,004, over
    // 1,024), refused at `points`; its values (8,010) and the rest are within its limits.
    let n = 2_000;
    let mut points =
        vec![r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}"#.to_owned()];
    let mut lines = Vec::new();
    let mut constraints = Vec::new();
    for i in 0..n {
        points.push(format!(
            r#"{{"id": "p{i:07}", "x": -20, "y": {i}, "construction": false}}"#
        ));
        points.push(format!(
            r#"{{"id": "q{i:07}", "x": 0, "y": {i}, "construction": false}}"#
        ));
        lines.push(format!(
            r#"{{"id": "n{i:07}", "from": "p{i:07}", "to": "q{i:07}", "construction": false}}"#
        ));
        let tangent = format!(
            r#"{{"id": "k{i:07}", "type": "tangent", "a": "n{i:07}", "b": "ca000000", "side": "right"}}"#
        );
        let on = format!(
            r#"{{"id": "m{i:07}", "type": "on", "point": "p{i:07}", "curve": "cb000000"}}"#
        );
        let pair = format!(
            r#"{{"id": "m1{i:06}", "type": "on", "point": "q{i:07}", "curve": "ca000000"}}"#
        );
        match i {
            0 => constraints.extend([pair, tangent, on]),
            1 => constraints.extend([tangent, on, pair]),
            _ => constraints.extend([tangent, on]),
        }
    }
    let circles = format!(
        r#"{CIRCLE}, {{"id": "cb000000", "center": "pa000000", "radius": 7, "construction": false}}"#
    );
    let text = document(&fields(
        &points.join(", "),
        &lines.join(", "),
        &circles,
        "",
        &constraints.join(", "),
    ));
    let all = problems(&text);
    let said: Vec<&str> = all.iter().filter_map(|e| e.parameter()).collect();
    assert_eq!(
        said,
        [
            "constraints.k0000000.b",
            "constraints.m1000001.point",
            "points"
        ]
    );
    assert!(
        all[0]
            .message()
            .contains("which constraint m1000000 puts on ca000000")
    );
    assert!(
        all[1]
            .message()
            .contains("which constraint k0000001 makes n0000001 edge tangent to")
    );
}

#[test]
fn a_tangent_on_takes_an_end_of_a_that_isnt_an_end_of_b() {
    for (constraint, message) in [
        (
            r#"{"id": "k0000000", "type": "tangent_on", "a": "na000000", "at": "pc000000", "b": "ca000000", "direction": "same"}"#,
            "a tangent_on's at, point pc000000, isn't an end of its a, na000000",
        ),
        (
            r#"{"id": "k0000000", "type": "tangent_on", "a": "na000000", "at": "pb000000", "b": "aa000000", "direction": "same"}"#,
            "a tangent_on's at, point pb000000, is an end of its b, aa000000, too",
        ),
    ] {
        only(&constrained(constraint), "constraints.k0000000.at", message);
    }
    only(
        &constrained(
            r#"{"id": "k0000000", "type": "tangent_on", "a": "na000000", "at": "pb000000", "b": "x_axis", "direction": "same"}"#,
        ),
        "constraints.k0000000.b",
        "a tangent_on takes a curve ending on a circle or arc, or an arc ending on a line",
    );
    // An arc ending on a line.
    none(&constrained(
        r#"{"id": "k0000000", "type": "tangent_on", "a": "aa000000", "at": "pc000000", "b": "x_axis", "direction": "opposite"}"#,
    ));
}

#[test]
fn each_tangent_on_direction_reads_as_its_own_value_and_writes_back() {
    use silico_core::document::{Direction, Form};
    for (word, direction) in [("same", Direction::Same), ("opposite", Direction::Opposite)] {
        let text = constrained(&format!(
            r#"{{"id": "k0000000", "type": "tangent_on", "a": "aa000000", "at": "pc000000", "b": "x_axis", "direction": "{word}"}}"#
        ));
        let d = read(text.as_bytes()).unwrap();
        let Operation::Sketch(sketch) = &d.features[0].operation else {
            panic!()
        };
        assert!(
            matches!(sketch.constraints[0].form, Form::TangentOn { direction: d, .. } if d == direction),
            "{word}"
        );
        assert!(
            write(&d)
                .unwrap()
                .contains(&format!("\"direction\": \"{word}\""))
        );
    }
}

#[test]
fn reference_dimensions_arent_the_solvers_values() {
    // The base points (6 unknowns), and the origin's and the axes' ends 6: 9,988 values to
    // spare. As many reference lengths as that, and one more, are admitted; one more driving
    // one, refused.
    let lengths = |n: usize, driving: bool| {
        let all: Vec<String> = (0..n)
            .map(|i| format!(r#"{{"id": "k{i:07}", "type": "length", "line": "na000000", "value": 10, "driving": {driving}}}"#))
            .collect();
        document(&fields(POINTS, LINE, "", "", &all.join(", ")))
    };
    none(&lengths(9989, false));
    none(&lengths(9988, true));
    only(
        &lengths(9989, true),
        "points",
        "the sketch has 10001 values, more than the 10000 the sketch solver takes",
    );
}

#[test]
fn a_tangent_on_is_a_fixed_angle() {
    // POINTS (6 unknowns) and CIRCLE (1), and 9,988 tangent_ons of the line's end on the
    // circle, each with its fixed angle: 7 + 9,988 + 6 (the origin, the axes' ends) =
    // 10,001 values, past 10,000; one fewer, 10,000, admitted.
    let tangents = |n: usize| {
        let all: Vec<String> = (0..n)
            .map(|i| format!(r#"{{"id": "k{i:07}", "type": "tangent_on", "a": "na000000", "at": "pb000000", "b": "ca000000", "direction": "same"}}"#))
            .collect();
        document(&fields(POINTS, LINE, CIRCLE, "", &all.join(", ")))
    };
    none(&tangents(9987));
    only(
        &tangents(9988),
        "points",
        "the sketch has 10001 values, more than the 10000 the sketch solver takes",
    );
}

#[test]
fn an_on_and_a_tangent_that_dont_meet_at_an_end_stand() {
    // The line's end on an axis, and the line tangent to the circle: no end on the circle.
    none(&constrained(
        r#"{"id": "k0000000", "type": "on", "point": "pb000000", "curve": "x_axis"}, {"id": "k0000001", "type": "tangent", "a": "na000000", "b": "ca000000", "side": "left"}"#,
    ));
    // The arc's end on the circle, and the line tangent to it: not the line's end.
    none(&constrained(
        r#"{"id": "k0000000", "type": "on", "point": "pc000000", "curve": "ca000000"}, {"id": "k0000001", "type": "tangent", "a": "na000000", "b": "ca000000", "side": "left"}"#,
    ));
}

#[test]
fn a_tangent_on_whose_end_is_bs_own_point_or_whose_curves_are_one_is_refused() {
    // The line's start is the circle's centre: on the circle, it would need no radius.
    only(
        &constrained(
            r#"{"id": "k0000000", "type": "tangent_on", "a": "na000000", "at": "pa000000", "b": "ca000000", "direction": "same"}"#,
        ),
        "constraints.k0000000.at",
        "a tangent_on's at, point pa000000, is part of its b, ca000000, itself",
    );
    // The arc's centre the same.
    only(
        &constrained(
            r#"{"id": "k0000000", "type": "tangent_on", "a": "na000000", "at": "pa000000", "b": "aa000000", "direction": "same"}"#,
        ),
        "constraints.k0000000.at",
        "a tangent_on's at, point pa000000, is part of its b, aa000000, itself",
    );
    let text = constrained(
        r#"{"id": "k0000000", "type": "tangent_on", "a": "aa000000", "at": "pb000000", "b": "aa000000", "direction": "same"}"#,
    );
    only(
        &text,
        "constraints.k0000000.b",
        "a tangent_on's a and b are both aa000000: it needs two different curves",
    );
    assert!(
        problems(&text)
            .iter()
            .all(|e| !e.suggestion().unwrap_or("").contains("tangent_at"))
    );
    // A circle where its at should be: told what at is.
    only(
        &constrained(
            r#"{"id": "k0000000", "type": "tangent_on", "a": "na000000", "at": "ca000000", "b": "aa000000", "direction": "same"}"#,
        ),
        "constraints.k0000000.at",
        "constraints.k0000000.at ca000000 is a circle, but a tangent_on's at is a point of this sketch where a ends, on b",
    );
}

#[test]
fn a_tangent_at_is_a_fixed_angle_too() {
    // POINTS (6 unknowns) and ARC (3): 9 unknowns, and the origin's and the axes' ends 6:
    // 9,985 values to spare. As many tangent_ats, each with its fixed angle, are admitted;
    // one more, refused.
    let tangents = |n: usize| {
        let all: Vec<String> = (0..n)
            .map(|i| format!(r#"{{"id": "k{i:07}", "type": "tangent_at", "a": "na000000", "b": "aa000000", "at": "pb000000", "direction": "same"}}"#))
            .collect();
        document(&fields(POINTS, LINE, "", ARC, &all.join(", ")))
    };
    none(&tangents(9985));
    only(
        &tangents(9986),
        "points",
        "the sketch has 10001 values, more than the 10000 the sketch solver takes",
    );
}

#[test]
fn a_tangent_on_at_another_circles_centre_stands() {
    // pa is na's start and ca's centre, and lies on cb (about pc, radius 10): not cb's own.
    let cb = r#"{"id": "cb000000", "center": "pc000000", "radius": 10, "construction": false}"#;
    none(&document(&fields(
        POINTS,
        LINE,
        &format!("{CIRCLE}, {cb}"),
        ARC,
        r#"{"id": "k0000000", "type": "tangent_on", "a": "na000000", "at": "pa000000", "b": "cb000000", "direction": "same"}"#,
    )));
}

#[test]
fn an_arc_whose_sent_radius_is_the_tolerance_is_refused_at_its_start_though_hypot_puts_it_beyond() {
    // The start's own check measures center to start with libm's hypot; the request sends
    // the radius as the square root of the squared distance. They differ by a rounding:
    // this start is a float above 1e-7 to hypot and exactly 1e-7 to the request, so the
    // arc's size as the kernel measures it (`Curve::size_problem`) refuses it, and the gate
    // refuses it as it refuses any start at its center, naming the start, not the chord.
    let start = (9.832072323405422e-8, -1.824925704604195e-8);
    let p = [start.0, start.1];
    assert!(silico_protocol::distance([0.0, 0.0], p) > 1e-7);
    assert_eq!(
        silico_protocol::squared_distance([0.0, 0.0], p).sqrt(),
        1e-7
    );
    // Its end 1.5e-7 up the y axis: 1.5e-7 from the center and 1.95e-7 from the start
    // (√((0.983e-7)² + (1.682e-7)²)), both above the tolerance.
    let text = document(&fields(
        &three_points(start, (0.0, 1.5e-7)),
        "",
        "",
        ARC,
        "",
    ));
    only(
        &text,
        "arcs.aa000000.start",
        "arc aa000000's start, point pb000000 at (9.832072323405422e-8, -1.824925704604195e-8), is 1e-7 mm from its center, point pa000000 at (0, 0): arcs and circles have a radius above 1e-7 mm, as the kernel measures them",
    );
    let error = &problems(&text)[0];
    assert_eq!(error.elements(), ["pb000000", "pa000000", "aa000000"]);
    assert_eq!(
        error.suggestion(),
        Some("draw the arc's start away from its center")
    );
    // A start clearly beyond, 1.01e-7 along x, is admitted with that end.
    none(&document(&fields(
        &three_points((1.01e-7, 0.0), (0.0, 1.5e-7)),
        "",
        "",
        ARC,
        "",
    )));
}

#[test]
fn a_circles_radius_past_a_kilometre_is_refused_in_the_arcs_words() {
    let circle = |radius: f64| {
        document(&fields(
            POINTS,
            "",
            &fixture::circle("ca000000", "pa000000", radius),
            "",
            "",
        ))
    };
    only(
        &circle(1000001.0),
        "circles.ca000000.radius",
        "circle ca000000's radius is 1000001 mm, more than the 1000000 mm (a kilometre) the solver takes",
    );
    assert_eq!(problems(&circle(1000001.0))[0].elements(), ["ca000000"]);
    none(&circle(1000000.0));
}

/// The refusal of a reference to an ID no element holds: one error, at `parameter`.
#[track_caller]
fn unknown(text: &str, parameter: &str) -> silico_core::Error {
    let all = problems(text);
    assert_eq!(all.len(), 1, "{all:#?}");
    assert_eq!(all[0].parameter(), Some(parameter), "{all:#?}");
    assert!(
        all[0]
            .message()
            .ends_with("isn't the ID of any element of this sketch"),
        "{all:#?}"
    );
    all[0].clone()
}

#[test]
fn a_curves_point_mistyped_has_the_points_a_character_or_two_off_as_candidates() {
    // pb000001 is a character from pb000000 and two from pa000000 and pc000000: nearest
    // first, then by ID. The line na000000 is three off, and isn't a point.
    let text = document(&fields(
        POINTS,
        &fixture::line("na000000", "pa000000", "pb000001"),
        "",
        "",
        "",
    ));
    let error = unknown(&text, "lines.na000000.to");
    assert_eq!(error.candidates(), ["pb000000", "pa000000", "pc000000"]);
    assert_eq!(
        error.suggestion(),
        Some(
            "name one of this sketch's points in lines.na000000.to: the candidates are this sketch's IDs a character or two from pb000001, none taken for it"
        )
    );
}

#[test]
fn a_constraints_reference_mistyped_has_only_elements_its_place_takes_as_candidates() {
    // na000001 is a character from the line na000000, and two from nb000000 (a line),
    // ca000000 (a circle), aa000000 (an arc), and pa000000 (a point). A horizontal takes a
    // line, so the lines are its candidates.
    let error = unknown(
        &with_two_lines(r#"{"id": "k0000000", "type": "horizontal", "line": "na000001"}"#),
        "constraints.k0000000.line",
    );
    assert_eq!(error.candidates(), ["na000000", "nb000000"]);
    // An on's curve takes any curve: the circle and the arc too, the nearest first.
    let error = unknown(
        &with_two_lines(
            r#"{"id": "k0000000", "type": "on", "point": "pc000000", "curve": "na000001"}"#,
        ),
        "constraints.k0000000.curve",
    );
    assert_eq!(
        error.candidates(),
        ["na000000", "aa000000", "ca000000", "nb000000"]
    );
}

#[test]
fn a_reference_far_from_every_id_has_no_candidates_and_its_advice_says_none() {
    let text = document(&fields(
        POINTS,
        &fixture::line("na000000", "pa000000", "zz999999"),
        "",
        "",
        "",
    ));
    let error = unknown(&text, "lines.na000000.to");
    assert!(error.candidates().is_empty(), "{error:#?}");
    assert_eq!(
        error.suggestion(),
        Some("name one of this sketch's points in lines.na000000.to")
    );
}

#[test]
fn two_characters_swapped_are_a_candidate_and_at_most_five_are_given() {
    // a0b1c2d3 with its last two swapped is a0b1c23d: two places off.
    let swapped = document(&fields(
        &[
            fixture::point("a0b1c2d3", 0.0, 0.0),
            fixture::point("q7w4zr8b", 10.0, 0.0),
        ]
        .join(", "),
        &fixture::line("na000000", "q7w4zr8b", "a0b1c23d"),
        "",
        "",
        "",
    ));
    assert_eq!(
        unknown(&swapped, "lines.na000000.to").candidates(),
        ["a0b1c2d3"]
    );
    // p0000001 to p0000007 are each a character from p0000000, and p00000aa two: the
    // first five by ID.
    let points: Vec<String> = (1..=7)
        .map(|k| fixture::point(&format!("p000000{k}"), f64::from(k), 0.0))
        .chain([fixture::point("p00000aa", 0.0, 5.0)])
        .collect();
    let many = document(&fields(
        &points.join(", "),
        &fixture::line("na000000", "p0000001", "p0000000"),
        "",
        "",
        "",
    ));
    assert_eq!(
        unknown(&many, "lines.na000000.to").candidates(),
        ["p0000001", "p0000002", "p0000003", "p0000004", "p0000005"]
    );
}

#[test]
fn a_constraint_naming_geometry_that_isnt_there_is_refused_not_dropped() {
    // FreeCAD's testValidateConstraintsCase (TestSketchValidateCoincidents.py:110): two
    // lines meeting at v1, and a constraint whose point is moved to geometry that doesn't
    // exist. FreeCAD's validateConstraints deletes the constraint; the gate refuses it,
    // naming it, and leaves the document as it is.
    let points = [
        fixture::point("pa000000", -47.680691, 18.824165000000004),
        fixture::point("pb000000", -47.680691, -27.346279),
        fixture::point("pc000000", 51.132679, -27.346279),
    ]
    .join(", ");
    let lines = [
        fixture::line("na000000", "pa000000", "pb000000"),
        fixture::line("nb000000", "pb000000", "pc000000"),
    ]
    .join(", ");
    let text = document(&fields(
        &points,
        &lines,
        "",
        "",
        r#"{"id": "k0000000", "type": "on", "point": "pd000000", "curve": "nb000000"}"#,
    ));
    let error = unknown(&text, "constraints.k0000000.point");
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert!(
        error.elements().contains(&"k0000000".to_owned()),
        "{error:#?}"
    );
    assert_eq!(error.candidates(), ["pa000000", "pb000000", "pc000000"]);
}

#[test]
fn geometry_deleted_with_its_constraints_leaves_the_rest_as_they_were() {
    // FreeCAD's testIssue3245 (TestSketcherSolver.py:298): three horizontal lines, two
    // with their start on the y axis, each with a horizontal distance (60, 65, 70 by
    // expression). Deleting line 2 deletes its constraints (3, 4, and 8) and renumbers
    // the rest, whose expressions were left on the old numbers until FreeCAD fixed it:
    // Constraints[4] and [5] must still be 60 and 65. That bug can't arise here: a
    // reference is an ID, and a constraint holds its own value (testIssue3245_2's deleted
    // constraint with an expression likewise has nothing to leave behind). So the first
    // half only shows the deletion leaves a document the gate admits, its constraints as
    // they were; the second is the gate's own part: a deletion that leaves constraints
    // naming what's gone is refused, each with candidates, never dropped as FreeCAD's
    // delGeometry drops them.
    let points = [
        fixture::point("p0a00000", -1.195999, 56.041161),
        fixture::point("p0b00000", 60.654316, 56.382877),
        fixture::point("p1a00000", 0.512583, 32.121155),
        fixture::point("p1b00000", 60.654316, 31.779440),
        fixture::point("p2a00000", 0.170867, 13.326859),
        fixture::point("p2b00000", 61.679455, 13.326859),
    ];
    let lines = [
        fixture::line("n0000000", "p0a00000", "p0b00000"),
        fixture::line("n1000000", "p1a00000", "p1b00000"),
        fixture::line("n2000000", "p2a00000", "p2b00000"),
    ];
    let constraints = [
        r#"{"id": "c0000000", "type": "on", "point": "p0a00000", "curve": "y_axis"}"#,
        r#"{"id": "c0000001", "type": "horizontal", "line": "n0000000"}"#,
        r#"{"id": "c0000002", "type": "horizontal", "line": "n1000000"}"#,
        r#"{"id": "c0000003", "type": "on", "point": "p2a00000", "curve": "y_axis"}"#,
        r#"{"id": "c0000004", "type": "horizontal", "line": "n2000000"}"#,
        r#"{"id": "c0000005", "type": "on", "point": "p1a00000", "curve": "y_axis"}"#,
        r#"{"id": "c0000006", "type": "distance_x", "a": "p0a00000", "b": "p0b00000", "value": 60, "driving": true}"#,
        r#"{"id": "c0000007", "type": "distance_x", "a": "p1a00000", "b": "p1b00000", "value": 65, "driving": true}"#,
        r#"{"id": "c0000008", "type": "distance_x", "a": "p2a00000", "b": "p2b00000", "value": 70, "driving": true}"#,
    ];
    let sketch = |points: &[String], lines: &[String], constraints: &[&str]| {
        document(&fields(
            &points.join(", "),
            &lines.join(", "),
            "",
            "",
            &constraints.join(", "),
        ))
    };
    let constraints_of = |text: &str| match &read(text.as_bytes()).unwrap().features[0].operation {
        Operation::Sketch(s) => s.constraints.clone(),
        other => panic!("{other:?}"),
    };
    let before = sketch(&points, &lines, &constraints);
    none(&before);
    let kept = [0, 1, 2, 5, 6, 7].map(|k| constraints[k]);
    let after = sketch(&points[..4], &lines[..2], &kept);
    none(&after);
    let mut expected = constraints_of(&before);
    expected.retain(|c| !["c0000003", "c0000004", "c0000008"].contains(&c.id.as_str()));
    assert_eq!(constraints_of(&after), expected);
    // 60 and 65 by hand, under their own IDs.
    let written = write(&read(after.as_bytes()).unwrap()).unwrap();
    for (id, value) in [("c0000006", "60"), ("c0000007", "65")] {
        let at = written.find(id).unwrap();
        let line = written[at..].lines().next().unwrap();
        assert!(line.contains(&format!("\"value\": {value},")), "{line}");
    }

    // The line and its points deleted but not its constraints: each reference to them is
    // refused, with the elements a character or two off as candidates.
    let dangling = problems(&sketch(&points[..4], &lines[..2], &constraints));
    let found: Vec<(Option<&str>, Vec<String>)> = dangling
        .iter()
        .map(|e| (e.parameter(), e.candidates().to_vec()))
        .collect();
    let near_p2a = ["p0a00000", "p1a00000", "p0b00000", "p1b00000"].map(String::from);
    let near_p2b = ["p0b00000", "p1b00000", "p0a00000", "p1a00000"].map(String::from);
    assert_eq!(
        found,
        [
            (Some("constraints.c0000003.point"), near_p2a.to_vec()),
            (
                Some("constraints.c0000004.line"),
                ["n0000000", "n1000000"].map(String::from).to_vec()
            ),
            (Some("constraints.c0000008.a"), near_p2a.to_vec()),
            (Some("constraints.c0000008.b"), near_p2b.to_vec()),
        ]
    );
}

#[test]
fn geometry_counts_every_point_line_circle_and_arc_and_the_origin_and_axes() {
    // POINTS (3), n lines, one circle, no arc, and the origin and two axes (3): 3 + n + 1 + 3.
    // 9,994 lines is 10,001, past MAX_SKETCH_GEOMETRY's 10,000; 9,993 is at it. With one
    // circle and no arc, a count multiplying lines by circles, or circles by arcs, is 10,000
    // (found by the mutation run).
    let lines = |n: usize| -> String {
        (0..n)
            .map(|i| {
                format!(
                    r#"{{"id": "n{i:07}", "from": "pa000000", "to": "pb000000", "construction": false}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    only(
        &document(&fields(POINTS, &lines(9994), CIRCLE, "", "")),
        "lines",
        "the sketch has 10001 points, lines, circles, and arcs, more than the 10000 the sketch solver takes",
    );
    none(&document(&fields(POINTS, &lines(9993), CIRCLE, "", "")));
}

#[test]
fn symmetric_of_a_lines_two_ends_about_the_line_says_both_are_its_ends() {
    // FreeCAD's own refusal (CommandConstraints.cpp, GeoId1 == GeoId2 == GeoId3): "Cannot
    // add a symmetry constraint between a line and its end points!", not one end's (found by
    // the mutation run).
    only(
        &with_two_lines(
            r#"{"id": "k0000000", "type": "symmetric", "a": "pa000000", "b": "pb000000", "about": "na000000"}"#,
        ),
        "constraints.k0000000.about",
        "a symmetric's a and b, pa000000 and pb000000, are the ends of na000000 itself",
    );
}
