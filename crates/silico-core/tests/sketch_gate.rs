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
use silico_core::document::{Operation, OriginPlane, Plane, read, write};

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
    for (text, plane) in [
        ("xy", OriginPlane::Xy),
        ("xz", OriginPlane::Xz),
        ("yz", OriginPlane::Yz),
    ] {
        let doc = document(&fields("", "", "", "", "")).replace("\"xy\"", &format!("\"{text}\""));
        let d = read(doc.as_bytes()).unwrap();
        match &d.features[0].operation {
            Operation::Sketch(s) => assert_eq!(s.plane, Plane::Origin(plane), "{text}"),
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

/// #258's case: a line nq from pq (0, 0) to pm (32, 0), and an arc aa about pc (10, 10)
/// from ps (10, 0), on the line, to pe (20, 10), so the arc ends on the line and runs along
/// it there; and these constraints.
fn arc_ending_on_the_line(constraints: &str) -> String {
    document(&fields(
        r#"{"id": "pq000000", "x": 0, "y": 0, "construction": false}, {"id": "pm000000", "x": 32, "y": 0, "construction": false}, {"id": "pc000000", "x": 10, "y": 10, "construction": false}, {"id": "ps000000", "x": 10, "y": 0, "construction": false}, {"id": "pe000000", "x": 20, "y": 10, "construction": false}"#,
        r#"{"id": "nq000000", "from": "pq000000", "to": "pm000000", "construction": false}"#,
        "",
        r#"{"id": "aa000000", "center": "pc000000", "start": "ps000000", "end": "pe000000", "construction": false}"#,
        constraints,
    ))
}

const ARC_TANGENT_ON: &str = r#"{"id": "k0000000", "type": "tangent_on", "a": "aa000000", "at": "ps000000", "b": "nq000000", "direction": "same"}"#;

#[test]
fn an_edge_tangent_beside_a_tangent_on_of_the_pair_is_refused_at_the_later_of_the_two() {
    // A tangent_on holds its a's end on its b as an `on` does, and makes them tangent there,
    // so an edge tangent of the same two curves says nothing more: FreeCAD 1.1.3 diagnoses the
    // pair redundant (solve -2, redundant [1], #258). The tangent_on alone is taken.
    none(&arc_ending_on_the_line(ARC_TANGENT_ON));
    let remove = "remove the tangent: the tangent_on already makes them tangent at point ps000000";
    // The tangent after the tangent_on, either way round: refused at the tangent.
    for tangent in [
        r#"{"id": "k0000001", "type": "tangent", "a": "aa000000", "b": "nq000000", "side": "left"}"#,
        r#"{"id": "k0000001", "type": "tangent", "a": "nq000000", "b": "aa000000", "side": "left"}"#,
    ] {
        let text = arc_ending_on_the_line(&format!("{ARC_TANGENT_ON}, {tangent}"));
        only(
            &text,
            "constraints.k0000001.b",
            "a tangent's aa000000 ends at point ps000000, which constraint k0000000 puts on nq000000: a curve ending on another is tangent where it ends",
        );
        let all = problems(&text);
        assert_eq!(all[0].elements(), ["k0000001"]);
        assert_eq!(all[0].suggestion(), Some(remove));
    }
    // The tangent_on after the tangent, either way round: refused at the tangent_on, so an
    // edit that appends it is refused at its own member, naming the tangent.
    for tangent in [
        r#"{"id": "k0000000", "type": "tangent", "a": "aa000000", "b": "nq000000", "side": "left"}"#,
        r#"{"id": "k0000000", "type": "tangent", "a": "nq000000", "b": "aa000000", "side": "left"}"#,
    ] {
        let text = arc_ending_on_the_line(&format!(
            r#"{tangent}, {{"id": "k0000001", "type": "tangent_on", "a": "aa000000", "at": "ps000000", "b": "nq000000", "direction": "same"}}"#
        ));
        only(
            &text,
            "constraints.k0000001.at",
            "a tangent_on puts point ps000000, where aa000000 ends, on nq000000, which constraint k0000000 makes aa000000 edge tangent to: a curve ending on another is tangent where it ends",
        );
        let all = problems(&text);
        assert_eq!(all[0].elements(), ["k0000001"]);
        assert_eq!(all[0].suggestion(), Some(remove));
    }
}

/// Lines nz from pz (-20, 5) and ny to pw (20, 5) both ending at pt (0, 5) on the top of
/// circle ca, radius 5 about pa; and these constraints.
fn lines_on_the_circle(constraints: &str) -> String {
    document(&fields(
        r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pt000000", "x": 0, "y": 5, "construction": false}, {"id": "pz000000", "x": -20, "y": 5, "construction": false}, {"id": "pw000000", "x": 20, "y": 5, "construction": false}"#,
        r#"{"id": "nz000000", "from": "pz000000", "to": "pt000000", "construction": false}, {"id": "ny000000", "from": "pt000000", "to": "pw000000", "construction": false}"#,
        CIRCLE,
        "",
        constraints,
    ))
}

#[test]
fn a_tangent_on_holds_its_point_for_another_curve_ending_there_as_an_on_does() {
    // Lines nz and ny both end at pt, on the circle; the tangent_on makes nz tangent there,
    // and the edge tangent is ny's. pt is on the circle all the same, so ny ends on it and is
    // tangent where it ends: the tangent becomes a tangent_on of ny, and nz's stays (both
    // ways round, after the tangent_on and before it).
    let keep = "make the tangent a tangent_on, with a ny000000, at pt000000, and b ca000000, keeping nz000000's";
    let text = lines_on_the_circle(
        r#"{"id": "k0000000", "type": "tangent_on", "a": "nz000000", "at": "pt000000", "b": "ca000000", "direction": "same"}, {"id": "k0000001", "type": "tangent", "a": "ny000000", "b": "ca000000", "side": "left"}"#,
    );
    only(
        &text,
        "constraints.k0000001.b",
        "a tangent's ny000000 ends at point pt000000, which constraint k0000000 puts on ca000000",
    );
    assert_eq!(problems(&text)[0].suggestion(), Some(keep));
    let text = lines_on_the_circle(
        r#"{"id": "k0000000", "type": "tangent", "a": "ny000000", "b": "ca000000", "side": "left"}, {"id": "k0000001", "type": "tangent_on", "a": "nz000000", "at": "pt000000", "b": "ca000000", "direction": "same"}"#,
    );
    only(
        &text,
        "constraints.k0000001.at",
        "a tangent_on puts point pt000000, where ny000000 ends, on ca000000, which constraint k0000000 makes ny000000 edge tangent to",
    );
    assert_eq!(problems(&text)[0].suggestion(), Some(keep));
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
fn an_on_after_two_tangents_it_pairs_with_names_the_first() {
    // Two edge tangents of nz and ca, the circle named first in the first, then the `on`
    // holding nz's end on ca: refused once, at the `on`, naming the earlier tangent.
    only(
        &ending_on_the_circle(
            r#"{"id": "k0000000", "type": "tangent", "a": "ca000000", "b": "nz000000", "side": "left"}, {"id": "k0000001", "type": "tangent", "a": "nz000000", "b": "ca000000", "side": "right"}, {"id": "k0000002", "type": "on", "point": "pt000000", "curve": "ca000000"}"#,
        ),
        "constraints.k0000002.point",
        "which constraint k0000000 makes nz000000 edge tangent to",
    );
}

#[test]
fn a_tangent_after_both_of_its_lines_ends_held_on_the_curve_names_the_earliest_on() {
    // nz's end pt held on ca (k0000000), then its start pz (k0000001), then the tangent:
    // refused at the tangent, naming the earlier `on` and its point, though pz comes first
    // among nz's ends.
    only(
        &ending_on_the_circle(
            r#"{"id": "k0000000", "type": "on", "point": "pt000000", "curve": "ca000000"}, {"id": "k0000001", "type": "on", "point": "pz000000", "curve": "ca000000"}, {"id": "k0000002", "type": "tangent", "a": "nz000000", "b": "ca000000", "side": "right"}"#,
        ),
        "constraints.k0000002.b",
        "a tangent's nz000000 ends at point pt000000, which constraint k0000000 puts on ca000000",
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

/// Fable's P1 sketch (#478's port review): the arc ae000003 about pe000020 (100, 10),
/// counter-clockwise from pe000021 (100, 0) to pe000022 (110, 10), radius 10; the line
/// ne000010 from pe000023 (90, 0) to pe000024 (110, 0), whose middle is (100, 0), the arc's
/// start; beside the base sketch's points and its line na000000; and these constraints.
fn arc_ending_at_the_lines_middle(constraints: &str) -> String {
    let points = [
        fixture::point("pe000020", 100.0, 10.0),
        fixture::point("pe000021", 100.0, 0.0),
        fixture::point("pe000022", 110.0, 10.0),
        fixture::point("pe000023", 90.0, 0.0),
        fixture::point("pe000024", 110.0, 0.0),
    ];
    document(&fields(
        &format!("{POINTS}, {}", points.join(", ")),
        &format!(
            "{LINE}, {}",
            fixture::line("ne000010", "pe000023", "pe000024")
        ),
        "",
        &fixture::arc("ae000003", "pe000020", "pe000021", "pe000022"),
        constraints,
    ))
}

/// A `midpoint` of `point` on `line`, as `id`.
fn midpoint(id: &str, point: &str, line: &str) -> String {
    format!(r#"{{"id": "{id}", "type": "midpoint", "point": "{point}", "line": "{line}"}}"#)
}

/// An edge `tangent` of `a` and `b`, the round on the line's left, as `id`.
fn edge_tangent(id: &str, a: &str, b: &str) -> String {
    format!(r#"{{"id": "{id}", "type": "tangent", "a": "{a}", "b": "{b}", "side": "left"}}"#)
}

/// The arc's start held on the line by a `tangent_on` that makes the arc tangent there, as
/// `id`: the form Tangent writes beside the midpoint (Fable's P1, test 1).
fn arc_tangent_on(id: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "tangent_on", "a": "ae000003", "at": "pe000021", "b": "ne000010", "direction": "same"}}"#
    )
}

#[test]
fn an_edge_tangent_beside_a_midpoint_holding_its_curves_end_is_refused_at_the_later_of_the_two() {
    // Fable's P1, the gate's half: a `midpoint` holds its point on its line as an `on` does
    // (C3 as amended: a hold is a hold). The arc ends at pe000021, which kt000004 holds at
    // ne000010's middle, so an edge tangent of the arc and the line is redundant where it
    // holds, as beside an `on`, and refused at whichever comes later. The suggestion keeps
    // the midpoint, which holds the end at the middle, and makes the tangent a tangent_on of
    // the arc's start on the line.
    let keep = |midpoint: &str| {
        format!(
            "make the tangent a tangent_on, with a ae000003, at pe000021, and b ne000010, keeping midpoint {midpoint}"
        )
    };
    // The midpoint, then the tangent, either way round: refused at the tangent's b.
    for (a, b) in [("ae000003", "ne000010"), ("ne000010", "ae000003")] {
        let text = arc_ending_at_the_lines_middle(&format!(
            "{}, {}",
            midpoint("kt000004", "pe000021", "ne000010"),
            edge_tangent("kt000101", a, b)
        ));
        only(&text, "constraints.kt000101.b", "kt000004");
        let all = problems(&text);
        assert_eq!(
            all[0].message(),
            "a tangent's ae000003 ends at point pe000021, which constraint kt000004 puts on ne000010: a curve ending on another is tangent where it ends",
            "{a} {b}"
        );
        assert_eq!(all[0].elements(), ["kt000101"]);
        assert_eq!(all[0].suggestion(), Some(keep("kt000004").as_str()));
    }
    // The tangent, then the midpoint, either way round: refused at the midpoint's point,
    // naming the tangent, so an edit that appends the midpoint is refused at its own member.
    for (a, b) in [("ae000003", "ne000010"), ("ne000010", "ae000003")] {
        let text = arc_ending_at_the_lines_middle(&format!(
            "{}, {}",
            edge_tangent("kt000004", a, b),
            midpoint("kt000101", "pe000021", "ne000010")
        ));
        only(&text, "constraints.kt000101.point", "kt000004");
        let all = problems(&text);
        assert_eq!(
            all[0].message(),
            "a midpoint puts point pe000021, where ae000003 ends, on ne000010, which constraint kt000004 makes ae000003 edge tangent to: a curve ending on another is tangent where it ends",
            "{a} {b}"
        );
        assert_eq!(all[0].elements(), ["kt000101"]);
        assert_eq!(all[0].suggestion(), Some(keep("kt000101").as_str()));
    }
}

#[test]
fn a_tangent_on_beside_the_midpoint_holding_its_end_stands() {
    // Fable's P1 (a): the tangent_on Tangent writes beside the midpoint, in either order. Its
    // point-on-line repeats the midpoint's, which the solve flags redundant, the plain kind;
    // neither is an edge tangent, so the tangent rule has no pair.
    none(&arc_ending_at_the_lines_middle(&format!(
        "{}, {}",
        midpoint("kt000004", "pe000021", "ne000010"),
        arc_tangent_on("kt000101")
    )));
    none(&arc_ending_at_the_lines_middle(&format!(
        "{}, {}",
        arc_tangent_on("kt000004"),
        midpoint("kt000101", "pe000021", "ne000010")
    )));
}

#[test]
fn a_midpoint_of_a_point_that_isnt_an_end_of_the_tangents_curve_stands() {
    // The arc's centre pe000020 at ne000010's middle: no end of the arc or the line is held
    // on the other, so the edge tangent has no pair, in either order.
    let held = midpoint("kt000004", "pe000020", "ne000010");
    let tangent = edge_tangent("kt000101", "ae000003", "ne000010");
    none(&arc_ending_at_the_lines_middle(&format!(
        "{held}, {tangent}"
    )));
    let held = midpoint("kt000101", "pe000020", "ne000010");
    let tangent = edge_tangent("kt000004", "ae000003", "ne000010");
    none(&arc_ending_at_the_lines_middle(&format!(
        "{tangent}, {held}"
    )));
}

#[test]
fn a_midpoint_on_another_line_than_the_tangents_stands() {
    // The arc's start pe000021 held at na000000's middle, not ne000010's: the arc's end is on
    // a line the tangent doesn't name, so the edge tangent has no pair, in either order.
    let held = midpoint("kt000004", "pe000021", "na000000");
    let tangent = edge_tangent("kt000101", "ae000003", "ne000010");
    none(&arc_ending_at_the_lines_middle(&format!(
        "{held}, {tangent}"
    )));
    let held = midpoint("kt000101", "pe000021", "na000000");
    let tangent = edge_tangent("kt000004", "ae000003", "ne000010");
    none(&arc_ending_at_the_lines_middle(&format!(
        "{tangent}, {held}"
    )));
}

#[test]
fn an_on_and_a_tangent_on_beside_a_midpoint_keep_their_own_suggestions() {
    // On Fable's P1 sketch, each hold of the arc's start on ne000010 and an edge tangent
    // after it: the refusal names the first hold in the list, with that hold's own
    // suggestion. An `on` and the edge tangent become one tangent_on; a tangent_on of the
    // arc already makes it tangent there; a midpoint stays (above).
    let on = |id: &str| {
        format!(r#"{{"id": "{id}", "type": "on", "point": "pe000021", "curve": "ne000010"}}"#)
    };
    let one = "make the two one tangent_on, with a ae000003, at pe000021, and b ne000010";
    let remove = "remove the tangent: the tangent_on already makes them tangent at point pe000021";
    let tangent = edge_tangent("kt000101", "ae000003", "ne000010");
    for (holds, holder, suggestion) in [
        (on("kt000004"), "kt000004", one.to_owned()),
        (arc_tangent_on("kt000004"), "kt000004", remove.to_owned()),
        // An `on` before a midpoint of the same point and line, and after one.
        (
            format!(
                "{}, {}",
                on("kt000004"),
                midpoint("kt000005", "pe000021", "ne000010")
            ),
            "kt000004",
            one.to_owned(),
        ),
        (
            format!(
                "{}, {}",
                midpoint("kt000004", "pe000021", "ne000010"),
                on("kt000005")
            ),
            "kt000004",
            "make the tangent a tangent_on, with a ae000003, at pe000021, and b ne000010, keeping midpoint kt000004".to_owned(),
        ),
    ] {
        let text = arc_ending_at_the_lines_middle(&format!("{holds}, {tangent}"));
        only(&text, "constraints.kt000101.b", "kt000004");
        let all = problems(&text);
        assert_eq!(
            all[0].message(),
            format!(
                "a tangent's ae000003 ends at point pe000021, which constraint {holder} puts on ne000010: a curve ending on another is tangent where it ends"
            ),
            "{holds}"
        );
        assert_eq!(all[0].suggestion(), Some(suggestion.as_str()), "{holds}");
    }
    // An `on` after the tangent keeps its own words too.
    let text = arc_ending_at_the_lines_middle(&format!(
        "{}, {}",
        edge_tangent("kt000004", "ae000003", "ne000010"),
        on("kt000101")
    ));
    only(
        &text,
        "constraints.kt000101.point",
        "an on puts point pe000021, where ae000003 ends, on ne000010, which constraint kt000004 makes ae000003 edge tangent to: a curve ending on another is tangent where it ends",
    );
    assert_eq!(problems(&text)[0].suggestion(), Some(one));
}

/// A refusal: its parameter, its elements, its message, and its suggestion.
type Refusal = (String, Vec<String>, String, String);

/// Each of the gate's refusals of `text`, in the order it gives them.
fn refusals(text: &str) -> Vec<Refusal> {
    problems(text)
        .iter()
        .map(|e| {
            (
                e.parameter().unwrap_or_default().to_owned(),
                e.elements().to_vec(),
                e.message().to_owned(),
                e.suggestion().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// What every refusal of the pair says beside the arc's own tangent_on of pe000021 on
/// ne000010 (#498's round 1).
const REMOVE_THE_TANGENT: &str =
    "remove the tangent: the tangent_on already makes them tangent at point pe000021";

/// On Fable's P1 sketch, the arc's edge tangent to ne000010, `tangent`, refused at its b,
/// naming `holder`, the hold before it.
fn the_tangent_refused(tangent: &str, holder: &str) -> Refusal {
    (
        format!("constraints.{tangent}.b"),
        vec![tangent.to_owned()],
        format!(
            "a tangent's ae000003 ends at point pe000021, which constraint {holder} puts on ne000010: a curve ending on another is tangent where it ends"
        ),
        REMOVE_THE_TANGENT.to_owned(),
    )
}

/// On Fable's P1 sketch, `hold`, `noun` ("an on", "a midpoint", "a tangent_on"), holding the
/// arc's start on ne000010 after the edge tangent `tangent`, refused at its field `field`.
fn the_hold_refused(noun: &str, hold: &str, field: &str, tangent: &str) -> Refusal {
    (
        format!("constraints.{hold}.{field}"),
        vec![hold.to_owned()],
        format!(
            "{noun} puts point pe000021, where ae000003 ends, on ne000010, which constraint {tangent} makes ae000003 edge tangent to: a curve ending on another is tangent where it ends"
        ),
        REMOVE_THE_TANGENT.to_owned(),
    )
}

/// The tangent's two spellings, its arc first and its line first: the pair's search runs one
/// way round, then the other, so each order is checked both ways.
const BOTH_WAYS: [(&str, &str); 2] = [("ae000003", "ne000010"), ("ne000010", "ae000003")];

/// Each order of Fable's P1 sketch's constraints that the gate doesn't refuse as `expected`
/// says, or whose refusals ask for a tangent_on: each named by `tangent`, the tangent's
/// spelling, and its number from 1, so a failure names every one.
fn wrong_orders<const N: usize>(
    tangent: (&str, &str),
    orders: impl IntoIterator<Item = ([String; N], Vec<Refusal>)>,
) -> Vec<String> {
    orders
        .into_iter()
        .enumerate()
        .filter_map(|(n, (constraints, expected))| {
            let said = refusals(&arc_ending_at_the_lines_middle(&constraints.join(", ")));
            let asks = said.iter().any(|(.., advice)| advice.starts_with("make"));
            (said != expected || asks).then(|| {
                format!(
                    "tangent {tangent:?}, order {}: {constraints:#?}\nsaid {said:#?}\nnot {expected:#?}",
                    n + 1
                )
            })
        })
        .collect()
}

#[test]
fn beside_the_arcs_own_tangent_on_every_refusal_of_a_midpoints_pair_says_to_remove_the_tangent() {
    // A midpoint and the arc's tangent_on of pe000021 on ne000010 are what Tangent writes
    // (#478). An edge tangent of the arc and the line beside them says nothing the tangent_on
    // doesn't, so every refusal of the pair, whichever hold it names and wherever the
    // tangent_on is in the list, says to remove the tangent: none asks for a tangent_on the
    // sketch has (#498's round 1). In each order the IDs are kt000004, kt000005, kt000101 as
    // listed. The tangent is refused at its b, naming the tangent_on if it comes before the
    // tangent, else the first hold before it; each hold after the tangent is refused at its
    // own member, naming the tangent. The midpoint and the tangent_on never pair.
    // The tangent either way round (#498's round 2).
    let m = |id: &str| midpoint(id, "pe000021", "ne000010");
    let o = arc_tangent_on;
    let midpoint_refused =
        |hold: &str, tangent: &str| the_hold_refused("a midpoint", hold, "point", tangent);
    let tangent_on_refused =
        |hold: &str, tangent: &str| the_hold_refused("a tangent_on", hold, "at", tangent);
    let mut wrong = Vec::new();
    for (a, b) in BOTH_WAYS {
        let t = |id: &str| edge_tangent(id, a, b);
        wrong.extend(wrong_orders(
            (a, b),
            [
                (
                    [m("kt000004"), o("kt000005"), t("kt000101")],
                    vec![the_tangent_refused("kt000101", "kt000005")],
                ),
                (
                    [o("kt000004"), m("kt000005"), t("kt000101")],
                    vec![the_tangent_refused("kt000101", "kt000004")],
                ),
                (
                    [t("kt000004"), m("kt000005"), o("kt000101")],
                    vec![
                        midpoint_refused("kt000005", "kt000004"),
                        tangent_on_refused("kt000101", "kt000004"),
                    ],
                ),
                (
                    [t("kt000004"), o("kt000005"), m("kt000101")],
                    vec![
                        tangent_on_refused("kt000005", "kt000004"),
                        midpoint_refused("kt000101", "kt000004"),
                    ],
                ),
                (
                    [m("kt000004"), t("kt000005"), o("kt000101")],
                    vec![
                        the_tangent_refused("kt000005", "kt000004"),
                        tangent_on_refused("kt000101", "kt000005"),
                    ],
                ),
                (
                    [o("kt000004"), t("kt000005"), m("kt000101")],
                    vec![
                        the_tangent_refused("kt000005", "kt000004"),
                        midpoint_refused("kt000101", "kt000005"),
                    ],
                ),
            ],
        ));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn beside_the_arcs_own_tangent_on_an_ons_refusal_says_to_remove_the_tangent_too() {
    // One rule for every hold: an `on` of the arc's start on ne000010 beside the arc's
    // tangent_on and an edge tangent. After the tangent, the `on` is refused at its point
    // saying to remove the tangent, not "make the two one tangent_on", which would ask for a
    // second copy of the tangent_on; before it, the tangent names the tangent_on, though the
    // `on` comes first. The tangent either way round.
    let on = |id: &str| {
        format!(r#"{{"id": "{id}", "type": "on", "point": "pe000021", "curve": "ne000010"}}"#)
    };
    let mut wrong = Vec::new();
    for (a, b) in BOTH_WAYS {
        let t = |id: &str| edge_tangent(id, a, b);
        wrong.extend(wrong_orders(
            (a, b),
            [
                (
                    [t("kt000004"), on("kt000005"), arc_tangent_on("kt000101")],
                    vec![
                        the_hold_refused("an on", "kt000005", "point", "kt000004"),
                        the_hold_refused("a tangent_on", "kt000101", "at", "kt000004"),
                    ],
                ),
                (
                    [on("kt000004"), arc_tangent_on("kt000005"), t("kt000101")],
                    vec![the_tangent_refused("kt000101", "kt000005")],
                ),
            ],
        ));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The line's end pe000024 held on the arc by a `tangent_on` that makes the line tangent
/// there, as `id`: a tangent_on of the arc and the line the other way round from the arc's own
/// (format review F5 on #498). The gate reads which points and curves a constraint names, not
/// where they lie.
fn line_tangent_on(id: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "tangent_on", "a": "ne000010", "at": "pe000024", "b": "ae000003", "direction": "same"}}"#
    )
}

/// What every refusal of the pair says beside the line's tangent_on of pe000024 on ae000003,
/// where the arc has none of its own: its own point, pe000024, which it makes them tangent at.
const REMOVE_AT_THE_LINES_END: &str =
    "remove the tangent: the tangent_on already makes them tangent at point pe000024";

/// `refusal` with `suggestion` in place of its own.
fn suggesting(refusal: Refusal, suggestion: &str) -> Refusal {
    let (parameter, elements, message, _) = refusal;
    (parameter, elements, message, suggestion.to_owned())
}

/// On Fable's P1 sketch, the edge tangent `tangent` refused at its b, naming `holder`, the
/// line's tangent_on before it, in its words: the line ends on the arc.
fn the_tangent_refused_for_the_lines_end(tangent: &str, holder: &str) -> Refusal {
    (
        format!("constraints.{tangent}.b"),
        vec![tangent.to_owned()],
        format!(
            "a tangent's ne000010 ends at point pe000024, which constraint {holder} puts on ae000003: a curve ending on another is tangent where it ends"
        ),
        REMOVE_AT_THE_LINES_END.to_owned(),
    )
}

/// On Fable's P1 sketch, the line's tangent_on `hold` after the edge tangent `tangent`,
/// refused at its at.
fn the_lines_tangent_on_refused(hold: &str, tangent: &str) -> Refusal {
    (
        format!("constraints.{hold}.at"),
        vec![hold.to_owned()],
        format!(
            "a tangent_on puts point pe000024, where ne000010 ends, on ae000003, which constraint {tangent} makes ne000010 edge tangent to: a curve ending on another is tangent where it ends"
        ),
        REMOVE_AT_THE_LINES_END.to_owned(),
    )
}

/// Every order of a hold of the arc's start on ne000010 (`hold`, an `on` or a `midpoint`,
/// `noun` its words, `field` the member it's refused at), the line's tangent_on, and an edge
/// tangent, either way round, that the gate doesn't refuse as F5's rule says: each says to
/// remove the tangent, at pe000024, where the line's tangent_on makes the two tangent, so
/// none asks for a second tangency of the pair. The tangent is refused at its b, naming the
/// line's tangent_on if it comes before the tangent, else the hold, in the arc's words; each
/// constraint after the tangent is refused at its own member, naming the tangent. The hold and
/// the tangent_on never pair.
fn wrong_orders_beside_the_lines_tangent_on(
    hold: &dyn Fn(&str) -> String,
    noun: &str,
    field: &str,
) -> Vec<String> {
    let h = hold;
    let o = line_tangent_on;
    let hold_refused = |id: &str, tangent: &str| {
        suggesting(
            the_hold_refused(noun, id, field, tangent),
            REMOVE_AT_THE_LINES_END,
        )
    };
    let mut wrong = Vec::new();
    for (a, b) in BOTH_WAYS {
        let t = |id: &str| edge_tangent(id, a, b);
        wrong.extend(wrong_orders(
            (a, b),
            [
                (
                    [h("kt000004"), o("kt000005"), t("kt000101")],
                    vec![the_tangent_refused_for_the_lines_end(
                        "kt000101", "kt000005",
                    )],
                ),
                (
                    [o("kt000004"), h("kt000005"), t("kt000101")],
                    vec![the_tangent_refused_for_the_lines_end(
                        "kt000101", "kt000004",
                    )],
                ),
                (
                    [t("kt000004"), h("kt000005"), o("kt000101")],
                    vec![
                        hold_refused("kt000005", "kt000004"),
                        the_lines_tangent_on_refused("kt000101", "kt000004"),
                    ],
                ),
                (
                    [t("kt000004"), o("kt000005"), h("kt000101")],
                    vec![
                        the_lines_tangent_on_refused("kt000005", "kt000004"),
                        hold_refused("kt000101", "kt000004"),
                    ],
                ),
                (
                    [h("kt000004"), t("kt000005"), o("kt000101")],
                    vec![
                        suggesting(
                            the_tangent_refused("kt000005", "kt000004"),
                            REMOVE_AT_THE_LINES_END,
                        ),
                        the_lines_tangent_on_refused("kt000101", "kt000005"),
                    ],
                ),
                (
                    [o("kt000004"), t("kt000005"), h("kt000101")],
                    vec![
                        the_tangent_refused_for_the_lines_end("kt000005", "kt000004"),
                        hold_refused("kt000101", "kt000005"),
                    ],
                ),
            ],
        ));
    }
    wrong
}

#[test]
fn beside_the_lines_tangent_on_every_refusal_of_an_ons_pair_says_to_remove_the_tangent() {
    // F5 (format review on #498): an `on` of the arc's start on ne000010, and the line's
    // tangent_on of its end pe000024 on the arc, which makes the two tangent already. Before,
    // the tangent's refusal named the `on` and said to make the two one tangent_on, a second
    // tangency of the pair, while the tangent_on's refusal said to remove the tangent.
    let on = |id: &str| {
        format!(r#"{{"id": "{id}", "type": "on", "point": "pe000021", "curve": "ne000010"}}"#)
    };
    let wrong = wrong_orders_beside_the_lines_tangent_on(&on, "an on", "point");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn beside_the_lines_tangent_on_every_refusal_of_a_midpoints_pair_says_to_remove_the_tangent() {
    // F5 with a midpoint of the arc's start on ne000010 in place of the `on`: before, the
    // tangent's refusal said to make the tangent a tangent_on keeping the midpoint.
    let held = |id: &str| midpoint(id, "pe000021", "ne000010");
    let wrong = wrong_orders_beside_the_lines_tangent_on(&held, "a midpoint", "point");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn beside_both_tangent_ons_of_the_pair_a_refusal_names_the_ending_curves_own_point() {
    // The arc's own tangent_on of pe000021 on ne000010 and the line's of pe000024 on the arc.
    // A hold of an end refused after the tangent names the point of the ending curve's own
    // tangent_on holding that end, wherever either tangent_on is in the list, so #498's words
    // stand; the tangent, refused at its b, names the first tangent_on before it, either way
    // round, in its own words. The tangent either way round.
    let on = |id: &str| {
        format!(r#"{{"id": "{id}", "type": "on", "point": "pe000021", "curve": "ne000010"}}"#)
    };
    let (ao, lo) = (arc_tangent_on, line_tangent_on);
    let mut wrong = Vec::new();
    for (a, b) in BOTH_WAYS {
        let t = |id: &str| edge_tangent(id, a, b);
        wrong.extend(wrong_orders(
            (a, b),
            [
                (
                    [
                        t("kt000004"),
                        on("kt000005"),
                        lo("kt000006"),
                        ao("kt000101"),
                    ],
                    vec![
                        the_hold_refused("an on", "kt000005", "point", "kt000004"),
                        the_lines_tangent_on_refused("kt000006", "kt000004"),
                        the_hold_refused("a tangent_on", "kt000101", "at", "kt000004"),
                    ],
                ),
                (
                    [
                        lo("kt000004"),
                        t("kt000005"),
                        on("kt000006"),
                        ao("kt000101"),
                    ],
                    vec![
                        the_tangent_refused_for_the_lines_end("kt000005", "kt000004"),
                        the_hold_refused("an on", "kt000006", "point", "kt000005"),
                        the_hold_refused("a tangent_on", "kt000101", "at", "kt000005"),
                    ],
                ),
                (
                    [
                        ao("kt000004"),
                        lo("kt000005"),
                        on("kt000006"),
                        t("kt000101"),
                    ],
                    vec![the_tangent_refused("kt000101", "kt000004")],
                ),
                (
                    [
                        lo("kt000004"),
                        ao("kt000005"),
                        on("kt000006"),
                        t("kt000101"),
                    ],
                    vec![the_tangent_refused_for_the_lines_end(
                        "kt000101", "kt000004",
                    )],
                ),
            ],
        ));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_tangent_on_of_the_arc_and_a_third_curve_leaves_the_ons_words() {
    // A tangent_on of na000000's end pb000000 on the arc, or of the arc's end pe000022 on
    // na000000: neither is of the pair, the arc and ne000010, so neither makes them tangent,
    // and the `on` of the arc's start on ne000010 keeps its words, one tangent_on, whether the
    // tangent comes after it (refused at its b) or before (the `on` refused at its point).
    // The tangent either way round.
    let on = |id: &str| {
        format!(r#"{{"id": "{id}", "type": "on", "point": "pe000021", "curve": "ne000010"}}"#)
    };
    let one = "make the two one tangent_on, with a ae000003, at pe000021, and b ne000010";
    for third in [
        r#"{"id": "kt000005", "type": "tangent_on", "a": "na000000", "at": "pb000000", "b": "ae000003", "direction": "same"}"#,
        r#"{"id": "kt000005", "type": "tangent_on", "a": "ae000003", "at": "pe000022", "b": "na000000", "direction": "same"}"#,
    ] {
        for (a, b) in BOTH_WAYS {
            let said = refusals(&arc_ending_at_the_lines_middle(&format!(
                "{}, {third}, {}",
                on("kt000004"),
                edge_tangent("kt000101", a, b)
            )));
            assert_eq!(
                said,
                [suggesting(the_tangent_refused("kt000101", "kt000004"), one)],
                "{third} {a} {b}"
            );
            let said = refusals(&arc_ending_at_the_lines_middle(&format!(
                "{}, {third}, {}",
                edge_tangent("kt000004", a, b),
                on("kt000101")
            )));
            assert_eq!(
                said,
                [suggesting(
                    the_hold_refused("an on", "kt000101", "point", "kt000004"),
                    one
                )],
                "{third} {a} {b}"
            );
        }
    }
}

/// A `tangent_on` of `a` at `at` on `b`, as `id`.
fn tangent_on(id: &str, a: &str, at: &str, b: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "tangent_on", "a": "{a}", "at": "{at}", "b": "{b}", "direction": "same"}}"#
    )
}

/// An `on` of the arc's start pe000021 on ne000010, as `id`.
fn arc_start_on_the_line(id: &str) -> String {
    format!(r#"{{"id": "{id}", "type": "on", "point": "pe000021", "curve": "ne000010"}}"#)
}

/// The `on`'s words beside the edge tangent: one tangent_on of the arc's start on the line.
const ONE_TANGENT_ON: &str =
    "make the two one tangent_on, with a ae000003, at pe000021, and b ne000010";

/// On Fable's P1 sketch, the tangent_on `hold` of `ending`'s end `point` on `other`, after the
/// edge tangent `tangent`, refused at its at, saying to remove the tangent at `at`.
fn a_tangent_on_refused(
    hold: &str,
    point: &str,
    ending: &str,
    other: &str,
    tangent: &str,
    at: &str,
) -> Refusal {
    (
        format!("constraints.{hold}.at"),
        vec![hold.to_owned()],
        format!(
            "a tangent_on puts point {point}, where {ending} ends, on {other}, which constraint {tangent} makes {ending} edge tangent to: a curve ending on another is tangent where it ends"
        ),
        format!("remove the tangent: the tangent_on already makes them tangent at point {at}"),
    )
}

#[test]
fn a_tangent_on_of_the_pair_refused_for_its_at_holds_nothing_for_the_rule() {
    // Round 1's F1: the line's tangent_on kt000005 of a point that isn't an end of the line,
    // the arc's centre pe000020 or the free point pc000000, or of an ID no element has,
    // pz999999, is refused on its own, so it holds nothing for the rule, and the `on` of the arc's
    // start keeps its words, one tangent_on, whether the tangent comes after it (refused at
    // its b) or before (the `on` refused at its point). The tangent_on's own refusal is as it
    // is without the tangent. The tangent either way round.
    let not_an_end = |at: &str| -> Refusal {
        (
            "constraints.kt000005.at".to_owned(),
            vec!["kt000005".to_owned()],
            format!("a tangent_on's at, point {at}, isn't an end of its a, ne000010"),
            "name the end of ne000010 that lies on ae000003".to_owned(),
        )
    };
    let unknown: Refusal = (
        "constraints.kt000005.at".to_owned(),
        vec!["kt000005".to_owned()],
        "constraints.kt000005.at pz999999 isn't the ID of any element of this sketch".to_owned(),
        "name a point of this sketch where a ends, on b in constraints.kt000005.at".to_owned(),
    );
    for (at, own) in [
        ("pe000020", not_an_end("pe000020")),
        ("pc000000", not_an_end("pc000000")),
        ("pz999999", unknown),
    ] {
        let refused = tangent_on("kt000005", "ne000010", at, "ae000003");
        for (a, b) in BOTH_WAYS {
            let said = refusals(&arc_ending_at_the_lines_middle(&format!(
                "{}, {refused}, {}",
                arc_start_on_the_line("kt000004"),
                edge_tangent("kt000101", a, b)
            )));
            assert_eq!(
                said,
                [
                    own.clone(),
                    suggesting(the_tangent_refused("kt000101", "kt000004"), ONE_TANGENT_ON)
                ],
                "{at} {a} {b}"
            );
            let said = refusals(&arc_ending_at_the_lines_middle(&format!(
                "{}, {refused}, {}",
                edge_tangent("kt000004", a, b),
                arc_start_on_the_line("kt000101")
            )));
            assert_eq!(
                said,
                [
                    own.clone(),
                    suggesting(
                        the_hold_refused("an on", "kt000101", "point", "kt000004"),
                        ONE_TANGENT_ON
                    )
                ],
                "{at} {a} {b}"
            );
        }
    }
}

#[test]
fn a_tangent_on_of_the_pair_refused_as_at_bs_own_point_holds_nothing_for_the_rule() {
    // Round 1's F1 as the review gave it, and round 2's rule: the line ending at the arc's
    // centre pe000020 instead of pe000024, and its tangent_on of pe000020 on the arc, refused
    // as the arc's own point. A tangent_on refused on its own holds nothing for the tangent
    // rule, so it never pairs with the edge tangent: alone with the tangent, in either order,
    // only its own refusal is said; beside the `on` of the arc's start, the `on` pairs with
    // the tangent in the on's words, one tangent_on. Nothing says to remove the tangent. The
    // tangent either way round, first and last.
    let sketch = |constraints: &[String]| {
        arc_ending_at_the_lines_middle(&constraints.join(", ")).replace(
            r#""from": "pe000023", "to": "pe000024""#,
            r#""from": "pe000023", "to": "pe000020""#,
        )
    };
    let refused = |id: &str| tangent_on(id, "ne000010", "pe000020", "ae000003");
    let own = |id: &str| -> Refusal {
        (
            format!("constraints.{id}.at"),
            vec![id.to_owned()],
            "a tangent_on's at, point pe000020, is part of its b, ae000003, itself".to_owned(),
            "name the end of ne000010 that lies on ae000003, not a point of ae000003's own"
                .to_owned(),
        )
    };
    let on = arc_start_on_the_line;
    let mut wrong = Vec::new();
    for (a, b) in BOTH_WAYS {
        let t = |id: &str| edge_tangent(id, a, b);
        for (constraints, expected) in [
            (
                vec![refused("kt000004"), t("kt000101")],
                vec![own("kt000004")],
            ),
            (
                vec![t("kt000004"), refused("kt000101")],
                vec![own("kt000101")],
            ),
            (
                vec![on("kt000004"), refused("kt000005"), t("kt000101")],
                vec![
                    own("kt000005"),
                    suggesting(the_tangent_refused("kt000101", "kt000004"), ONE_TANGENT_ON),
                ],
            ),
            (
                vec![t("kt000004"), refused("kt000005"), on("kt000101")],
                vec![
                    own("kt000005"),
                    suggesting(
                        the_hold_refused("an on", "kt000101", "point", "kt000004"),
                        ONE_TANGENT_ON,
                    ),
                ],
            ),
        ] {
            let said = refusals(&sketch(&constraints));
            if said != expected || said.iter().any(|(.., s)| s.starts_with("remove")) {
                wrong.push(format!(
                    "{a} {b}: {constraints:#?}\nsaid {said:#?}\nnot {expected:#?}"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_tangent_on_of_two_lines_holds_nothing_for_the_rule() {
    // The two-lines refusal counts as the `at` checks do: ne000010's tangent_on kt000005 of
    // its start pe000023 on na000000 is refused, two lines being never tangent, so it holds
    // nothing for the tangent rule. Beside an `on` of na000000's start pa000000 on ne000010
    // and an edge tangent of the two lines, the `on` pairs with the tangent in the on's
    // words, whichever comes later, and the tangent is refused as two lines too. The tangent
    // either way round.
    let on = r#"{"id": "ON", "type": "on", "point": "pa000000", "curve": "ne000010"}"#;
    let refused = tangent_on("kt000005", "ne000010", "pe000023", "na000000");
    let own: Refusal = (
        "constraints.kt000005.b".to_owned(),
        vec!["kt000005".to_owned()],
        "a tangent_on takes a curve ending on a circle or arc, or an arc ending on a line: two lines can't be tangent".to_owned(),
        "make one of them a circle or arc, or use parallel and on".to_owned(),
    );
    let two_lines = |id: &str| -> Refusal {
        (
            format!("constraints.{id}.b"),
            vec![id.to_owned()],
            "a tangent takes a line and a circle or arc, or two circles or arcs: two lines can't be tangent".to_owned(),
            "make one of them a circle or arc, or use parallel".to_owned(),
        )
    };
    let one = "make the two one tangent_on, with a na000000, at pa000000, and b ne000010";
    for (a, b) in [("na000000", "ne000010"), ("ne000010", "na000000")] {
        let said = refusals(&arc_ending_at_the_lines_middle(&format!(
            "{}, {refused}, {}",
            on.replace("ON", "kt000004"),
            edge_tangent("kt000101", a, b)
        )));
        assert_eq!(
            said,
            [
                own.clone(),
                (
                    "constraints.kt000101.b".to_owned(),
                    vec!["kt000101".to_owned()],
                    "a tangent's na000000 ends at point pa000000, which constraint kt000004 puts on ne000010: a curve ending on another is tangent where it ends".to_owned(),
                    one.to_owned(),
                ),
                two_lines("kt000101"),
            ],
            "{a} {b}"
        );
        let said = refusals(&arc_ending_at_the_lines_middle(&format!(
            "{}, {refused}, {}",
            edge_tangent("kt000004", a, b),
            on.replace("ON", "kt000101")
        )));
        assert_eq!(
            said,
            [
                two_lines("kt000004"),
                own.clone(),
                (
                    "constraints.kt000101.point".to_owned(),
                    vec!["kt000101".to_owned()],
                    "an on puts point pa000000, where na000000 ends, on ne000010, which constraint kt000004 makes na000000 edge tangent to: a curve ending on another is tangent where it ends".to_owned(),
                    one.to_owned(),
                ),
            ],
            "{a} {b}"
        );
    }
}

#[test]
fn the_arcs_own_tangent_on_at_its_other_end_makes_the_pair_tangent() {
    // Round 1's T1: the arc's tangent_on kt of its end pe000022 on ne000010, not the end the
    // `on` holds, pe000021. It makes the two tangent all the same, at pe000022, so the `on`
    // after the tangent says to remove the tangent there; the tangent_on's own refusal names
    // its own point. Before the tangent, the tangent_on is the hold the tangent names, in its
    // own words. The tangent either way round.
    let other_end = |id: &str| tangent_on(id, "ae000003", "pe000022", "ne000010");
    let remove = "remove the tangent: the tangent_on already makes them tangent at point pe000022";
    let mut wrong = Vec::new();
    for (a, b) in BOTH_WAYS {
        let t = |id: &str| edge_tangent(id, a, b);
        wrong.extend(wrong_orders(
            (a, b),
            [
                (
                    [
                        t("kt000004"),
                        arc_start_on_the_line("kt000005"),
                        other_end("kt000101"),
                    ],
                    vec![
                        suggesting(
                            the_hold_refused("an on", "kt000005", "point", "kt000004"),
                            remove,
                        ),
                        a_tangent_on_refused(
                            "kt000101", "pe000022", "ae000003", "ne000010", "kt000004",
                            "pe000022",
                        ),
                    ],
                ),
                (
                    [
                        arc_start_on_the_line("kt000004"),
                        other_end("kt000005"),
                        t("kt000101"),
                    ],
                    vec![(
                        "constraints.kt000101.b".to_owned(),
                        vec!["kt000101".to_owned()],
                        "a tangent's ae000003 ends at point pe000022, which constraint kt000005 puts on ne000010: a curve ending on another is tangent where it ends".to_owned(),
                        remove.to_owned(),
                    )],
                ),
            ],
        ));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn of_two_tangent_ons_of_the_pair_the_first_in_the_list_names_the_point() {
    // Round 1's T2: beside the `on` of the arc's start after the tangent, two tangent_ons of
    // the pair, neither the arc's own of pe000021: the first in the list names the point the
    // `on`'s refusal says the two are tangent at, whichever way round each is; and of two
    // tangent_ons the same way round, the line's of pe000024 and of pe000023, the first.
    // Each tangent_on's own refusal names its own point. The tangent either way round.
    let other_end = |id: &str| tangent_on(id, "ae000003", "pe000022", "ne000010");
    let line_start = |id: &str| tangent_on(id, "ne000010", "pe000023", "ae000003");
    let on_refused = |at: &str| {
        suggesting(
            the_hold_refused("an on", "kt000101", "point", "kt000004"),
            &format!("remove the tangent: the tangent_on already makes them tangent at point {at}"),
        )
    };
    let arcs = |hold: &str| {
        a_tangent_on_refused(
            hold, "pe000022", "ae000003", "ne000010", "kt000004", "pe000022",
        )
    };
    let lines = |hold: &str, point: &str| {
        a_tangent_on_refused(hold, point, "ne000010", "ae000003", "kt000004", point)
    };
    let mut wrong = Vec::new();
    for (a, b) in BOTH_WAYS {
        let t = |id: &str| edge_tangent(id, a, b);
        let on = arc_start_on_the_line;
        wrong.extend(wrong_orders(
            (a, b),
            [
                (
                    [
                        t("kt000004"),
                        line_tangent_on("kt000005"),
                        other_end("kt000006"),
                        on("kt000101"),
                    ],
                    vec![
                        lines("kt000005", "pe000024"),
                        arcs("kt000006"),
                        on_refused("pe000024"),
                    ],
                ),
                (
                    [
                        t("kt000004"),
                        other_end("kt000005"),
                        line_tangent_on("kt000006"),
                        on("kt000101"),
                    ],
                    vec![
                        arcs("kt000005"),
                        lines("kt000006", "pe000024"),
                        on_refused("pe000022"),
                    ],
                ),
                (
                    [
                        t("kt000004"),
                        line_tangent_on("kt000005"),
                        line_start("kt000006"),
                        on("kt000101"),
                    ],
                    vec![
                        lines("kt000005", "pe000024"),
                        lines("kt000006", "pe000023"),
                        on_refused("pe000024"),
                    ],
                ),
                (
                    [
                        t("kt000004"),
                        line_start("kt000005"),
                        line_tangent_on("kt000006"),
                        on("kt000101"),
                    ],
                    vec![
                        lines("kt000005", "pe000023"),
                        lines("kt000006", "pe000024"),
                        on_refused("pe000023"),
                    ],
                ),
            ],
        ));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_tangent_on_of_the_line_and_a_third_curve_leaves_the_ons_words() {
    // Round 1's T3: a tangent_on of ne000010's start pe000023 on na000000, or of na000000's
    // end pb000000 on ne000010, is of neither the arc nor the pair, so the `on` of the arc's
    // start keeps its words, one tangent_on, the tangent after it or before. Each tangent_on
    // is refused on its own as two lines. The tangent either way round.
    let two_lines: Refusal = (
        "constraints.kt000005.b".to_owned(),
        vec!["kt000005".to_owned()],
        "a tangent_on takes a curve ending on a circle or arc, or an arc ending on a line: two lines can't be tangent".to_owned(),
        "make one of them a circle or arc, or use parallel and on".to_owned(),
    );
    for third in [
        tangent_on("kt000005", "ne000010", "pe000023", "na000000"),
        tangent_on("kt000005", "na000000", "pb000000", "ne000010"),
    ] {
        for (a, b) in BOTH_WAYS {
            let said = refusals(&arc_ending_at_the_lines_middle(&format!(
                "{}, {third}, {}",
                arc_start_on_the_line("kt000004"),
                edge_tangent("kt000101", a, b)
            )));
            assert_eq!(
                said,
                [
                    two_lines.clone(),
                    suggesting(the_tangent_refused("kt000101", "kt000004"), ONE_TANGENT_ON)
                ],
                "{third} {a} {b}"
            );
            let said = refusals(&arc_ending_at_the_lines_middle(&format!(
                "{}, {third}, {}",
                edge_tangent("kt000004", a, b),
                arc_start_on_the_line("kt000101")
            )));
            assert_eq!(
                said,
                [
                    two_lines.clone(),
                    suggesting(
                        the_hold_refused("an on", "kt000101", "point", "kt000004"),
                        ONE_TANGENT_ON
                    )
                ],
                "{third} {a} {b}"
            );
        }
    }
}

#[test]
fn a_tangent_names_the_on_before_another_curves_tangent_on_of_its_end_in_the_ons_words() {
    // Lines nz and ny both end at pt on the circle: k0000004 holds pt on ca, k0000005 makes
    // nz tangent there, and k0000101 is ny's edge tangent, either way round. The tangent_on
    // is nz's, not the ending ny's, so it settles nothing for ny and isn't preferred: the
    // refusal names the first hold before the tangent, the `on`, and the on's suggestion, a
    // tangent_on of ny (#498's round 2).
    for (a, b) in [("ny000000", "ca000000"), ("ca000000", "ny000000")] {
        let said = refusals(&lines_on_the_circle(&format!(
            r#"{{"id": "k0000004", "type": "on", "point": "pt000000", "curve": "ca000000"}}, {{"id": "k0000005", "type": "tangent_on", "a": "nz000000", "at": "pt000000", "b": "ca000000", "direction": "same"}}, {{"id": "k0000101", "type": "tangent", "a": "{a}", "b": "{b}", "side": "left"}}"#
        )));
        assert_eq!(
            said,
            [(
                "constraints.k0000101.b".to_owned(),
                vec!["k0000101".to_owned()],
                "a tangent's ny000000 ends at point pt000000, which constraint k0000004 puts on ca000000: a curve ending on another is tangent where it ends".to_owned(),
                "make the two one tangent_on, with a ny000000, at pt000000, and b ca000000".to_owned(),
            )],
            "{a} {b}"
        );
    }
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
