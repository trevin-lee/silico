//! M3.2's gate, rule by rule: each test fails on a wrong version of a rule that the other
//! sketch tests pass (found by the review's mutations), and on each rule added by the review.
//! Counts and limits are worked out by hand from the solver's limits.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/sketch.rs"]
mod fixture;

use fixture::{ARC, CIRCLE, FEATURE, LINE, POINTS, constrained, document, fields, problems};
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
    only(&text, "arcs.0.start", "more than the 1000000 mm");
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
            &format!("arcs.0.{key}"),
            "is too close to its center at (0, 0) for its radius to be anything but 0",
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
        "constraints.0.at",
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
        "constraints.0.about",
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
        "constraints.0.about",
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
            "constraints.0.point",
            "is part of aa000000 itself",
        );
    }
}

// E9: each arc is three unknowns.
#[test]
fn arcs_count_three_unknowns_each() {
    // 3 points (6) and 681 arcs (2043): 2049, past 2048.
    let arcs: Vec<String> = (0..681).map(|i| format!(r#"{{"id": "a{i:07}", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}}"#)).collect();
    only(
        &document(&fields(POINTS, "", "", &arcs.join(","), "")),
        "points",
        "the sketch has 2049 unknowns",
    );
}

// E9: each arc's rules count as a constraint.
#[test]
fn arcs_count_as_constraints() {
    // 680 arcs and 9,321 constraints: 10,001.
    let arcs: Vec<String> = (0..680).map(|i| format!(r#"{{"id": "a{i:07}", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}}"#)).collect();
    let cs: Vec<String> = (0..9321)
        .map(|i| format!(r#"{{"id": "k{i:07}", "type": "horizontal", "line": "na000000"}}"#))
        .collect();
    only(
        &document(&fields(POINTS, LINE, "", &arcs.join(","), &cs.join(","))),
        "constraints",
        "the sketch has 10001 constraints",
    );
}

// E9: each arc is four rows; each midpoint two.
#[test]
fn arcs_and_midpoints_count_their_rows_in_the_jacobian() {
    // 680 arcs (2046 unknowns), 8,000 symmetric (16,000 rows) and the arcs' 2,720:
    // 18,720 × 2046 = 38,301,120, past 2^25; without the arcs' rows, 32,736,000.
    let arcs: Vec<String> = (0..680).map(|i| format!(r#"{{"id": "a{i:07}", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}}"#)).collect();
    let cs: Vec<String> = (0..8000).map(|i| format!(r#"{{"id": "k{i:07}", "type": "symmetric", "a": "pb000000", "b": "pc000000", "about": "pa000000"}}"#)).collect();
    only(
        &document(&fields(POINTS, "", "", &arcs.join(","), &cs.join(","))),
        "constraints",
        "38301120",
    );
    // 1024 points (2048 unknowns) and 8,193 midpoints, two rows each: 33,558,528.
    let points: Vec<String> = (0..1024)
        .map(|i| format!(r#"{{"id": "p{i:07}", "x": {i}, "y": 0, "construction": false}}"#))
        .collect();
    let line = r#"{"id": "n0000000", "from": "p0000000", "to": "p0000001", "construction": false}"#;
    let cs: Vec<String> = (0..8193).map(|i| format!(r#"{{"id": "k{i:07}", "type": "midpoint", "point": "p0000002", "line": "n0000000"}}"#)).collect();
    only(
        &document(&fields(&points.join(","), line, "", "", &cs.join(","))),
        "constraints",
        "33558528",
    );
}

#[test]
fn the_schema_refuses_sketches_the_reader_refuses_by_shape() {
    let schema: serde_json::Value =
        serde_json::from_str(&silico_core::document::schema().unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let base = with_two_lines(
        r#"{"id": "k0000000", "type": "horizontal", "line": "na000000"}, {"id": "k0000001", "type": "length", "line": "na000000", "value": 3, "driving": true}"#,
    );
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
        "constraints.0.b",
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
        "constraints.0.point",
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
            Some("a reference length's value is 3000000.000001 mm, but it's from 0 to 3000000 mm"),
        ),
        (
            r#"{"id": "k0000000", "type": "radius", "curve": "ca000000", "value": -0.000001, "driving": false}"#,
            Some("a reference radius's value is -0.000001 mm, but it's from 0 to 3000000 mm"),
        ),
        (
            r#"{"id": "k0000000", "type": "distance_x", "a": "pa000000", "b": "pb000000", "value": -3000000, "driving": false}"#,
            None,
        ),
        (
            r#"{"id": "k0000000", "type": "distance_x", "a": "pa000000", "b": "pb000000", "value": -3000000.000001, "driving": false}"#,
            Some(
                "a reference distance_x's value is -3000000.000001 mm, but it's at most 3000000 mm either way",
            ),
        ),
        (
            r#"{"id": "k0000000", "type": "distance", "a": "pa000000", "b": "pb000000", "value": 0, "driving": true}"#,
            Some("a distance's value is 0 mm, but it's above 0 and at most 1000000 mm"),
        ),
    ] {
        match problem {
            None => none(&with_two_lines(constraint)),
            Some(message) => only(&with_two_lines(constraint), "constraints.0.value", message),
        }
    }
}

#[test]
fn geometry_the_solver_measures_as_no_size_is_refused() {
    // 1e-200 squared underflows to 0: to the solver, a line of no length and an arc of no
    // radius, though the points are at different places.
    let tiny = r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 0, "y": 1e-200, "construction": false}, {"id": "pc000000", "x": 1e-200, "y": 0, "construction": false}, {"id": "pd000000", "x": 0, "y": 10, "construction": false}"#;
    only(
        &document(&fields(
            tiny,
            r#"{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}"#,
            "",
            "",
            "",
        )),
        "lines.0.to",
        "line na000000's ends, points pa000000 at (0, 0) and pb000000 at (0, 1e-200), are too close",
    );
    only(
        &document(&fields(
            tiny,
            "",
            "",
            r#"{"id": "aa000000", "center": "pa000000", "start": "pc000000", "end": "pd000000", "construction": false}"#,
            "",
        )),
        "arcs.0.start",
        "is too close to its center at (0, 0) for its radius to be anything but 0 to the solver",
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
        "lines.0.to",
        "too close for its length to be anything but 0",
    );
}

#[test]
fn an_arc_whose_start_and_end_are_drawn_at_one_place_is_refused() {
    let points = r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pc000000", "x": 10, "y": 0, "construction": false}"#;
    only(
        &document(&fields(points, "", "", ARC, "")),
        "arcs.0.end",
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
            "constraints.0.b",
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
        "constraints.0.b",
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
fn geometry_a_hair_apart_has_size_to_the_solver() {
    // 1e-150 squared is 1e-300, not 0: a line and an arc radius the solver measures.
    let tiny = r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 0.000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 1e-150, "construction": false}"#;
    none(&document(&fields(tiny, LINE, "", ARC, "")));
    // The arc's end, 1e-200 from its center, has no angle to it.
    let end = r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pc000000", "x": 1e-200, "y": 0, "construction": false}"#;
    only(
        &document(&fields(end, "", "", ARC, "")),
        "arcs.0.end",
        "arc aa000000's end, point pc000000 at (1e-200, 0), is too close to its center at (0, 0)",
    );
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
        "constraints.0.value",
        "a reference diameter's value is 3000000.000001 mm, but it's from 0 to 3000000 mm",
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
        "constraints.1.b",
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
        "constraints.1.b",
        "a tangent's nz000000 ends at point pt000000",
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
        only(&constrained(constraint), "constraints.0.at", message);
    }
    only(
        &constrained(
            r#"{"id": "k0000000", "type": "tangent_on", "a": "na000000", "at": "pb000000", "b": "x_axis", "direction": "same"}"#,
        ),
        "constraints.0.b",
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
fn a_tangent_on_is_two_equations_and_a_fixed_angle() {
    // 1,023 points (2,046 unknowns), a circle about one (2,047), a line between two, and
    // 8,197 tangent_ons: 16,394 equations × 2,047 unknowns = 33,558,518 entries, past 2^25
    // (33,554,432); and 2,047 + 8,197 angles + 6 (the origin, the axes' ends) = 10,250
    // values, past 10,000.
    let points: Vec<String> = (0..1023)
        .map(|i| format!(r#"{{"id": "p{i:07}", "x": {i}, "y": 0, "construction": false}}"#))
        .collect();
    let line = r#"{"id": "n0000000", "from": "p0000000", "to": "p0000001", "construction": false}"#;
    let circle = r#"{"id": "c0000000", "center": "p0000005", "radius": 3, "construction": false}"#;
    let tangents: Vec<String> = (0..8197)
        .map(|i| format!(r#"{{"id": "k{i:07}", "type": "tangent_on", "a": "n0000000", "at": "p0000001", "b": "c0000000", "direction": "same"}}"#))
        .collect();
    let all = problems(&document(&fields(
        &points.join(", "),
        line,
        circle,
        "",
        &tangents.join(", "),
    )));
    let messages: Vec<&str> = all.iter().map(|e| e.message()).collect();
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("the sketch has 33558518 entries")),
        "{messages:#?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("the sketch has 10250 values")),
        "{messages:#?}"
    );
    assert_eq!(all.len(), 2, "{messages:#?}");
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
        "constraints.0.at",
        "a tangent_on's at, point pa000000, is part of its b, ca000000, itself",
    );
    // The arc's centre the same.
    only(
        &constrained(
            r#"{"id": "k0000000", "type": "tangent_on", "a": "na000000", "at": "pa000000", "b": "aa000000", "direction": "same"}"#,
        ),
        "constraints.0.at",
        "a tangent_on's at, point pa000000, is part of its b, aa000000, itself",
    );
    let text = constrained(
        r#"{"id": "k0000000", "type": "tangent_on", "a": "aa000000", "at": "pb000000", "b": "aa000000", "direction": "same"}"#,
    );
    only(
        &text,
        "constraints.0.b",
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
        "constraints.0.at",
        "constraints.0.at ca000000 is a circle, but a tangent_on's at is a point of this sketch where a ends, on b",
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
