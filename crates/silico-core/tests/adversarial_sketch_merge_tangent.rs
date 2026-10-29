//! Hostile inputs for a merge's edge-tangency conversion (`document::sketch_edit::merge`:
//! an edge `tangent` between two curves that the merge makes share one end becomes a
//! `tangent_at` there, its `direction` keeping the tangent's `side`, whatever the merge
//! moved), from break-it and the review council.
//!
//! Every expected `direction` is worked by hand: from the geometry where the merged points
//! touch (there the point is the contact, and the drawing shows the side), and from the
//! side where they're merged from apart (the drawing can then show the other side). A side
//! fixes the angle between the curves' own directions (a line's from start to end, a
//! round's counter-clockwise) where they touch: `left` and `inside` 0, `right` and
//! `outside` π. The solve reads 0 as `same` where the point ends one curve and starts the
//! other, and as `opposite` where it starts or ends both. In the property test, the
//! touching merges are read independently from the drawing, by the path's chords a
//! thousandth of a radian (or the line's whole length) either side of the shared point;
//! the same configurations merged from apart must say what the touching merge said.

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use silico_core::document::sketch_edit::{Edited, merge};
use silico_core::document::{
    Direction, Document, ElementId, Form, Operation, Reference, Sketch, read, validate, write,
};

use crate::fixture::{arc, document, fields, line, point};

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn ids(texts: &[&str]) -> Vec<ElementId> {
    texts.iter().map(|t| id(t)).collect()
}

fn doc(points: &[String], lines: &[String], arcs: &[String], constraints: &[String]) -> Document {
    doc_with(points, lines, "", arcs, constraints)
}

fn doc_with(
    points: &[String],
    lines: &[String],
    circles: &str,
    arcs: &[String],
    constraints: &[String],
) -> Document {
    read(
        document(&fields(
            &points.join(", "),
            &lines.join(", "),
            circles,
            &arcs.join(", "),
            &constraints.join(", "),
        ))
        .as_bytes(),
    )
    .unwrap()
}

fn sketch_of(document: &Document) -> &Sketch {
    match &document.features[0].operation {
        Operation::Sketch(sketch) => sketch,
        other => panic!("{other:?}"),
    }
}

fn with_sketch(document: &Document, sketch: &Sketch) -> Document {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(sketch.clone());
    document
}

/// The gate's refusals of `sketch` in place of the document's, as text.
fn gate(document: &Document, sketch: &Sketch) -> Vec<String> {
    match validate(&with_sketch(document, sketch)) {
        Ok(_) => Vec::new(),
        Err(invalid) => invalid.errors().iter().map(|e| e.to_string()).collect(),
    }
}

fn tangent(k: &str, a: &str, b: &str, side: &str) -> String {
    format!(r#"{{"id": "{k}", "type": "tangent", "a": "{a}", "b": "{b}", "side": "{side}"}}"#)
}

/// The constraint `k`'s form in `sketch`.
fn form<'s>(sketch: &'s Sketch, k: &str) -> &'s Form {
    &sketch
        .constraints
        .iter()
        .find(|c| c.id.as_str() == k)
        .unwrap_or_else(|| panic!("no constraint {k}"))
        .form
}

/// `k` in `edited` is a tangent_at of `a` and `b` at `at` with `direction`.
#[track_caller]
fn tangent_at(edited: &Edited, k: &str, a: &str, b: &str, at: &str, direction: Direction) {
    let element = |t: &str| Reference::Element(id(t));
    let want = Form::TangentAt {
        a: element(a),
        b: element(b),
        at: element(at),
        direction,
    };
    assert_eq!(form(&edited.sketch, k), &want, "{k}");
}

/// The merge of `points` in `before`: Ok, and a sketch the gate takes.
#[track_caller]
fn merged(before: &Document, points: &[&str]) -> Edited {
    if let Err(invalid) = validate(before) {
        panic!("the gate refused the sketch before: {:?}", invalid.errors());
    }
    let edited = merge(sketch_of(before), &ids(points)).unwrap();
    assert_eq!(gate(before, &edited.sketch), Vec::<String>::new());
    edited
}

// ---------------------------------------------------------------- two arcs, by hand

/// Arc r0 about c0 (0, 0), radius 10, ending or starting at t0 (10, 0); arc r1 about c1
/// (20, 0), radius 10 (externally tangent at (10, 0)), or about c2 (5, 0), radius 5
/// (internally), ending or starting at t1 (10, 4e-8). Tangent k0 of `pair`.
fn two_arcs(r0: (&str, &str), r1: (&str, &str), center1: &str, pair: [&str; 2]) -> Document {
    let side = if center1 == "c0000001" {
        "outside"
    } else {
        "inside"
    };
    let points = [
        point("c0000000", 0.0, 0.0),
        point("c0000001", 20.0, 0.0),
        point("c0000002", 5.0, 0.0),
        point("t0000000", 10.0, 0.0),
        point("t0000001", 10.0, 0.00000004),
        // On r0's circle: 270° and 90°.
        point("g0000000", 0.0, -10.0),
        point("g0000001", 0.0, 10.0),
        // On r1's circle about c1: 90° and 270°.
        point("v0000000", 20.0, 10.0),
        point("v0000001", 20.0, -10.0),
        // On r1's circle about c2: 90° and 270°.
        point("w0000000", 5.0, 5.0),
        point("w0000001", 5.0, -5.0),
    ];
    doc(
        &points,
        &[],
        &[
            arc("r0000000", "c0000000", r0.0, r0.1),
            arc("r0000001", center1, r1.0, r1.1),
        ],
        &[tangent("k0000000", pair[0], pair[1], side)],
    )
}

#[test]
fn two_externally_tangent_arcs_every_way_round() {
    // About (0, 0), (10, 0) is at 0°: counter-clockwise there runs +y. About (20, 0) it's
    // at 180°: counter-clockwise runs −y. r0 ending at t0 (from 270°) arrives +y; starting
    // there (to 90°) it leaves +y, so along it backwards into t0 is −y. r1 starting at t1
    // (to 270°) leaves −y; ending there (from 90°) arrives −y, so on along it backwards is
    // +y. An S (on through t0, one arc above, one below) is `same`; a cusp, `opposite`.
    let r0_ends = ("g0000000", "t0000000");
    let r0_starts = ("t0000000", "g0000001");
    let r1_starts = ("t0000001", "v0000001");
    let r1_ends = ("v0000000", "t0000001");
    for (r0, r1, direction) in [
        (r0_ends, r1_starts, Direction::Opposite),
        (r0_ends, r1_ends, Direction::Same),
        (r0_starts, r1_starts, Direction::Same),
        (r0_starts, r1_ends, Direction::Opposite),
    ] {
        for pair in [["r0000000", "r0000001"], ["r0000001", "r0000000"]] {
            for points in [["t0000000", "t0000001"], ["t0000001", "t0000000"]] {
                let before = two_arcs(r0, r1, "c0000001", pair);
                let edited = merged(&before, &points);
                tangent_at(&edited, "k0000000", pair[0], pair[1], points[0], direction);
                // The curve whose end was merged away is the one modified.
                let moved = if points[1] == "t0000000" {
                    "r0000000"
                } else {
                    "r0000001"
                };
                assert_eq!(edited.modified, ids(&[moved, "k0000000"]));
                assert_eq!(edited.removed, ids(&[points[1]]));
            }
        }
    }
}

#[test]
fn two_internally_tangent_arcs_every_way_round() {
    // About (5, 0), radius 5, (10, 0) is at 0°: counter-clockwise runs +y, as on r0. r1
    // starting at t1 (to 90°) leaves +y; ending there (from 270°) arrives +y, so on along it
    // backwards is −y. r0 ending at t0 arrives +y; starting there, backwards into it is −y.
    let r0_ends = ("g0000000", "t0000000");
    let r0_starts = ("t0000000", "g0000001");
    let r1_starts = ("t0000001", "w0000000");
    let r1_ends = ("w0000001", "t0000001");
    for (r0, r1, direction) in [
        (r0_ends, r1_starts, Direction::Same),
        (r0_ends, r1_ends, Direction::Opposite),
        (r0_starts, r1_starts, Direction::Opposite),
        (r0_starts, r1_ends, Direction::Same),
    ] {
        for pair in [["r0000000", "r0000001"], ["r0000001", "r0000000"]] {
            let before = two_arcs(r0, r1, "c0000002", pair);
            let edited = merged(&before, &["t0000000", "t0000001"]);
            tangent_at(&edited, "k0000000", pair[0], pair[1], "t0000000", direction);
        }
    }
}

// ---------------------------------------------------- where the direction is read

/// Line a0 from p0 (0, 0) to p1 (10, 0); arc r0 about q0 (10, 10), radius 10, from p9
/// (10, 4e-8) at 270° to p8 (20, 10) at 0°; a lone point x0 at (10, 20), 90° on r0's circle.
/// Tangent k0 of a0 and r0.
fn line_and_arc_with_lone_point() -> Document {
    doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("p0000008", 20.0, 10.0),
            point("p0000009", 10.0, 0.00000004),
            point("q0000000", 10.0, 10.0),
            point("x0000000", 10.0, 20.0),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &[arc("r0000000", "q0000000", "p0000009", "p0000008")],
        &[tangent("k0000000", "a0000000", "r0000000", "left")],
    )
}

#[test]
fn a_lone_kept_point_drawn_off_the_contact_takes_its_direction_from_the_side() {
    // k0 is `left`: r0's centre q0 (10, 10) is on a0's left, facing from (0, 0) to (10, 0).
    // Where they touch, (10, 0), r0 at 270° runs counter-clockwise +x, the way a0 runs: a
    // round on a line's left turns with it, an angle of 0 between their own directions.
    // Kept x0, a0 ends at it and r0 starts there, where the solve reads 0 as `same`.
    // The drawing at x0 (10, 20) shows a turn back: a0 arrives along (1, 2)/√5, and r0, at
    // 90°, leaves −x, a dot product of −1/√5, `opposite` as drawn. The side decides.
    let before = line_and_arc_with_lone_point();
    let edited = merged(&before, &["x0000000", "p0000001", "p0000009"]);
    tangent_at(
        &edited,
        "k0000000",
        "a0000000",
        "r0000000",
        "x0000000",
        Direction::Same,
    );
    assert_eq!(edited.removed, ids(&["p0000001", "p0000009"]));
    assert_eq!(edited.modified, ids(&["a0000000", "r0000000", "k0000000"]));
    // Kept p1 (10, 0), x0 merged into it too: the same ends shared, the same side, and
    // here the drawing agrees (r0 leaves +x along a0's +x). `same`.
    let edited = merged(&before, &["p0000001", "x0000000", "p0000009"]);
    tangent_at(
        &edited,
        "k0000000",
        "a0000000",
        "r0000000",
        "p0000001",
        Direction::Same,
    );
}

#[test]
fn three_curves_merged_at_once_each_pair_its_own_direction() {
    // a0 into p1 (10, 0) along +x; r0 about (10, 10) from p9 at 270° leaves +x; r1 about
    // (10, −10), radius 10, ends at p7 (10, −4e-8), at 90°, arriving −x from s0 (20, −10) at
    // 0°, so on along it backwards is +x. a0 then r0: +x, +x, `same`. a0 then r1: +x, +x,
    // `same`. r0 (backwards into p1, −x) then r1 (+x): a cusp, `opposite`.
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("p0000007", 10.0, -0.00000004),
            point("p0000008", 20.0, 10.0),
            point("p0000009", 10.0, 0.00000004),
            point("q0000000", 10.0, 10.0),
            point("q0000001", 10.0, -10.0),
            point("s0000000", 20.0, -10.0),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &[
            arc("r0000000", "q0000000", "p0000009", "p0000008"),
            arc("r0000001", "q0000001", "s0000000", "p0000007"),
        ],
        &[
            tangent("k0000000", "a0000000", "r0000000", "left"),
            tangent("k0000001", "a0000000", "r0000001", "right"),
            tangent("k0000002", "r0000000", "r0000001", "outside"),
        ],
    );
    let edited = merged(&before, &["p0000001", "p0000009", "p0000007"]);
    let (s, o) = (Direction::Same, Direction::Opposite);
    tangent_at(&edited, "k0000000", "a0000000", "r0000000", "p0000001", s);
    tangent_at(&edited, "k0000001", "a0000000", "r0000001", "p0000001", s);
    tangent_at(&edited, "k0000002", "r0000000", "r0000001", "p0000001", o);
    assert_eq!(
        edited.modified,
        ids(&["r0000000", "r0000001", "k0000000", "k0000001", "k0000002"])
    );
    assert_eq!(edited.removed, ids(&["p0000007", "p0000009"]));
}

#[test]
fn the_same_tangency_listed_twice_either_way_round_reads_one_way() {
    // k0 (a0, r0) and k1 (r0, a0): the same S, read from either end, `same` both; and the
    // written sketch the same whichever order the file lists them.
    let make = |constraints: [String; 2]| {
        doc(
            &[
                point("p0000000", 0.0, 0.0),
                point("p0000001", 10.0, 0.0),
                point("p0000008", 20.0, 10.0),
                point("p0000009", 10.0, 0.00000004),
                point("q0000000", 10.0, 10.0),
            ],
            &[line("a0000000", "p0000000", "p0000001")],
            &[arc("r0000000", "q0000000", "p0000009", "p0000008")],
            &constraints,
        )
    };
    let k0 = tangent("k0000000", "a0000000", "r0000000", "left");
    let k1 = tangent("k0000001", "r0000000", "a0000000", "left");
    let forward = make([k0.clone(), k1.clone()]);
    let backward = make([k1, k0]);
    let one = merged(&forward, &["p0000001", "p0000009"]);
    let other = merged(&backward, &["p0000001", "p0000009"]);
    tangent_at(
        &one,
        "k0000000",
        "a0000000",
        "r0000000",
        "p0000001",
        Direction::Same,
    );
    tangent_at(
        &one,
        "k0000001",
        "r0000000",
        "a0000000",
        "p0000001",
        Direction::Same,
    );
    assert_eq!(
        write(&with_sketch(&forward, &one.sketch)).unwrap(),
        write(&with_sketch(&backward, &other.sketch)).unwrap()
    );
    assert_eq!(one.modified, other.modified);
}

#[test]
fn a_line_ending_where_it_starts_its_arc_and_an_edge_tangent_to_a_circle_beside_it() {
    // An edge tangent of a0 to a circle c0 about q0 (10, 10), radius 10 (no ends, so no
    // shared one), beside k0 to the arc r0: the circle's stays an edge tangent, untouched.
    let before = doc_with(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("p0000008", 20.0, 10.0),
            point("p0000009", 10.0, 0.00000004),
            point("q0000000", 10.0, 10.0),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &crate::fixture::circle("e0000000", "q0000000", 10.0),
        &[arc("r0000000", "q0000000", "p0000009", "p0000008")],
        &[
            tangent("k0000000", "a0000000", "r0000000", "left"),
            tangent("k0000001", "a0000000", "e0000000", "left"),
        ],
    );
    let edited = merged(&before, &["p0000001", "p0000009"]);
    tangent_at(
        &edited,
        "k0000000",
        "a0000000",
        "r0000000",
        "p0000001",
        Direction::Same,
    );
    assert_eq!(
        form(&edited.sketch, "k0000001"),
        form(sketch_of(&before), "k0000001")
    );
    assert_eq!(edited.modified, ids(&["r0000000", "k0000000"]));
}

#[test]
fn a_failed_merge_converts_nothing_and_the_next_one_still_does() {
    // A merge refused for another reason (a0's own ends made one) leaves the sketch it was
    // given as it was, k0 an edge tangent still; the valid merge after it converts k0.
    let before = line_and_arc_with_lone_point();
    let original = sketch_of(&before).clone();
    assert!(merge(&original, &ids(&["p0000000", "p0000001", "p0000009"])).is_err());
    assert!(sketch_of(&before).canonically_eq(&original));
    assert!(matches!(form(&original, "k0000000"), Form::Tangent { .. }));
    let edited = merged(&before, &["p0000001", "p0000009"]);
    tangent_at(
        &edited,
        "k0000000",
        "a0000000",
        "r0000000",
        "p0000001",
        Direction::Same,
    );
}

// ------------------------------------------- merged onto a place that has no direction

/// The merge's contract: a refusal naming what's at fault, or a sketch the gate takes.
#[track_caller]
fn refused_or_gated(before: &Document, points: &[&str]) -> Result<Edited, silico_core::Error> {
    if let Err(invalid) = validate(before) {
        panic!("the gate refused the sketch before: {:?}", invalid.errors());
    }
    let result = merge(sketch_of(before), &ids(points));
    if let Ok(edited) = &result {
        assert_eq!(
            gate(before, &edited.sketch),
            Vec::<String>::new(),
            "{edited:?}"
        );
    }
    result
}

/// Line a0 from p0 (0, 0) to p1 at `p1`; arc r0 about q0 (10, 10) from p9 (10, 4e-8) to
/// p8 (20, 10); tangent k0 of a0 and r0 if `tangent`.
fn line_to(p1: (f64, f64), tangent_too: bool) -> Document {
    doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", p1.0, p1.1),
            point("p0000008", 20.0, 10.0),
            point("p0000009", 10.0, 0.00000004),
            point("q0000000", 10.0, 10.0),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &[arc("r0000000", "q0000000", "p0000009", "p0000008")],
        &if tangent_too {
            vec![tangent("k0000000", "a0000000", "r0000000", "left")]
        } else {
            Vec::new()
        },
    )
}

#[test]
fn an_arcs_end_merged_onto_its_centres_place_beside_an_edge_tangent() {
    // a0's end p1 drawn at (10, 10), r0's centre's place (another point, which the gate
    // takes). Kept p1, r0 would start at its own centre's place: no radius, and no
    // direction there for the tangent_at either (a radius of (0, 0) turned is (0, 0), so
    // `drawn_at`'s dot product is 0 and it says `same` of nothing). Before the conversion
    // the merge refused this input (the tangent); now it reaches the gate's by-value rules,
    // which the merge doesn't repeat (a merge with no tangency here is Ok on it too).
    // Before its fix: merge returns Ok; the gate refuses "arc r0000000's start, point
    // p0000001 at (10, 10), is 0 mm from its center, point q0000000 at (10, 10)".
    // Its elements: the merged points as named, p1 and p9; then the gate's, the end and the
    // centre it measured (p1, already listed, and q0) and the arc (r0).
    let before = line_to((10.0, 10.0), true);
    let error = refused_or_gated(&before, &["p0000001", "p0000009"]).unwrap_err();
    assert_refused(
        &error,
        "arc r0000000's start, point p0000001 at (10, 10), is 0 mm from its center",
        &["p0000001", "p0000009", "q0000000", "r0000000"],
    );
}

/// The merge's refusal of what the gate would refuse: its words, then the gate's; its
/// elements exactly `elements`, the merged points as named, then the gate's not listed yet.
#[track_caller]
fn assert_refused(error: &silico_core::Error, gate_says: &str, elements: &[&str]) {
    let prefix = "merging them would leave a sketch the gate refuses: ";
    assert!(
        error
            .message()
            .strip_prefix(prefix)
            .is_some_and(|m| m.starts_with(gate_says)),
        "{error:?}"
    );
    assert_eq!(error.code(), silico_core::ErrorCode::InvalidInput);
    assert_eq!(error.parameter(), Some("points"));
    assert_eq!(error.elements(), elements, "{error:?}");
    assert_eq!(
        error.suggestion(),
        Some("merge other points, or change what the merge would collapse")
    );
}

#[test]
fn an_arcs_end_merged_onto_its_other_ends_place_beside_an_edge_tangent() {
    // a0's end p1 drawn at (20, 10), r0's other end's place: kept p1, r0 would run from
    // (20, 10) to (20, 10), and k0 would be a tangent_at on an arc of no sweep.
    // Before its fix: merge returns Ok; the gate refuses "arc r0000000's start and end,
    // points p0000001 and p0000008, are both drawn at (20, 10), so its sweep is 0 or a
    // whole turn". Its elements: the merged points as named, then the gate's, the arc.
    let before = line_to((20.0, 10.0), true);
    let error = refused_or_gated(&before, &["p0000001", "p0000009"]).unwrap_err();
    assert_refused(
        &error,
        "arc r0000000's start and end",
        &["p0000001", "p0000009", "r0000000"],
    );
    // With no tangency it's refused alike (main's merge, which places by ID, took it).
    let error =
        refused_or_gated(&line_to((20.0, 10.0), false), &["p0000001", "p0000009"]).unwrap_err();
    assert_refused(
        &error,
        "arc r0000000's start and end",
        &["p0000001", "p0000009", "r0000000"],
    );
}

#[test]
fn a_lines_end_merged_onto_its_other_ends_place_beside_an_edge_tangent() {
    // r0's start p9 drawn at (0, 0), a0's start p0's place (r0's ends off its circle, the
    // solve's to mend). Kept p9, a0 would run from (0, 0) to (0, 0): no length, and no
    // direction for the tangent_at.
    // Before its fix: merge returns Ok; the gate refuses "line a0000000 is 0 mm long, from
    // point p0000000 at (0, 0) to p0000009 at (0, 0)". Its elements: the merged points as
    // named, p9 and p1; then the gate's, the line's ends (p0, and p9, already listed) and
    // the line.
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("p0000008", 20.0, 10.0),
            point("p0000009", 0.0, 0.0),
            point("q0000000", 10.0, 10.0),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &[arc("r0000000", "q0000000", "p0000009", "p0000008")],
        &[tangent("k0000000", "a0000000", "r0000000", "left")],
    );
    let error = refused_or_gated(&before, &["p0000009", "p0000001"]).unwrap_err();
    assert_refused(
        &error,
        "line a0000000 is 0 mm long",
        &["p0000009", "p0000001", "p0000000", "a0000000"],
    );
}

// --------------------------------------------------- a sketch at the solver's limits

#[test]
fn a_merge_at_the_values_limit_whose_tangent_ats_carry_it_past() {
    // A tangent_at is a fixed angle for the solver, one value more than an edge tangent
    // (the gate's `size`: unknowns, driving dimensions, each tangent_on's and tangent_at's
    // angle, and 6, at most 10,000). Line a0 from p0 to p1, and three arcs starting at p9,
    // each edge tangent to a0: 9 points (18 unknowns) and 3 arcs (9) are 27, and 9,967
    // driving lengths of a0 make 27 + 9,967 + 6 = 10,000 values, the most the gate takes.
    // Merging p1 and p9 takes away one point's 2 and makes three tangent_ats, 3 angles:
    // 10,001, which the gate refuses ("points": the values). So the merge must refuse it,
    // naming the limit, or not be Ok.
    // Before its fix: merge returns Ok; the gate refuses the merged sketch's 10,001 values.
    // The gate names no element for the sketch's size: the elements are the merged points.
    let points = [
        point("p0000000", 0.0, 0.0),
        point("p0000001", 10.0, 0.0),
        point("p0000009", 10.0, 0.00000004),
        point("q0000000", 10.0, 10.0),
        point("q0000001", 10.0, -10.0),
        point("q0000002", 10.0, 20.0),
        point("e0000000", 20.0, 10.0),
        point("e0000001", 0.0, -10.0),
        point("e0000002", 30.0, 20.0),
    ];
    let arcs = [
        arc("r0000000", "q0000000", "p0000009", "e0000000"),
        arc("r0000001", "q0000001", "p0000009", "e0000001"),
        arc("r0000002", "q0000002", "p0000009", "e0000002"),
    ];
    let mut constraints = vec![
        tangent("k0000000", "a0000000", "r0000000", "left"),
        tangent("k0000001", "a0000000", "r0000001", "right"),
        tangent("k0000002", "a0000000", "r0000002", "left"),
    ];
    constraints.extend((0..9_967).map(|i| {
        format!(
            r#"{{"id": "d{i:07}", "type": "length", "line": "a0000000", "value": 10, "driving": true}}"#
        )
    }));
    let before = doc(
        &points,
        &[line("a0000000", "p0000000", "p0000001")],
        &arcs,
        &constraints,
    );
    let error = refused_or_gated(&before, &["p0000001", "p0000009"]).unwrap_err();
    assert_refused(
        &error,
        "the sketch has 10001 values, more than the 10000 the sketch solver takes",
        &["p0000001", "p0000009"],
    );
}

#[test]
fn square_curves_take_their_direction_from_the_side_at_every_end_either_way_round() {
    // a0 runs along x = 10 between p1 (10, 0) and p0 (10, −10), its own direction −y from
    // p1 or +y into it; r0 is about q0 (10, 10), from p9 (10, 4e-8) at 270° to p8 (20, 10),
    // or from p7 (0, 10) at 180° to p9, its own direction +x at p9 either way. Drawn
    // square, with q0 on a0's line: the drawing shows no side, so only k0's stored side
    // can say. `left` is an angle of 0 between their own directions, `right` π. The solve
    // reads 0 as `same` where p1 ends one and starts the other, as `opposite` where it's
    // both's start or both's end; π the other way. So, by hand:
    // - a0 and r0 both start at p1: `left` 0, `opposite`; `right` π, `same`.
    // - a0 starts, r0 ends there: `left` 0, `same`; `right` π, `opposite`.
    // - a0 ends, r0 starts there: `left` 0, `same`; `right` π, `opposite`.
    // - both end at p1: `left` 0, `opposite`; `right` π, `same`.
    // Which of a0 and r0 is the tangent's `a` changes none of them.
    // Before the fix: the left column at every end, `right` too (square read as 0, as
    // FreeCAD's AutoLockTangencyAndPerpty reads a drawing no more than a right angle apart).
    let (s, o) = (Direction::Same, Direction::Opposite);
    let a0_starts = ("p0000001", "p0000000");
    let a0_ends = ("p0000000", "p0000001");
    let r0_starts = ("p0000009", "p0000008");
    let r0_ends = ("p0000007", "p0000009");
    for (a0, r0, side, direction) in [
        (a0_starts, r0_starts, "left", o),
        (a0_starts, r0_starts, "right", s),
        (a0_starts, r0_ends, "left", s),
        (a0_starts, r0_ends, "right", o),
        (a0_ends, r0_starts, "left", s),
        (a0_ends, r0_starts, "right", o),
        (a0_ends, r0_ends, "left", o),
        (a0_ends, r0_ends, "right", s),
    ] {
        for pair in [["a0000000", "r0000000"], ["r0000000", "a0000000"]] {
            let before = doc(
                &[
                    point("p0000000", 10.0, -10.0),
                    point("p0000001", 10.0, 0.0),
                    point("p0000007", 0.0, 10.0),
                    point("p0000008", 20.0, 10.0),
                    point("p0000009", 10.0, 0.00000004),
                    point("q0000000", 10.0, 10.0),
                ],
                &[line("a0000000", a0.0, a0.1)],
                &[arc("r0000000", "q0000000", r0.0, r0.1)],
                &[tangent("k0000000", pair[0], pair[1], side)],
            );
            let edited = merged(&before, &["p0000001", "p0000009"]);
            assert_eq!(
                form(&edited.sketch, "k0000000"),
                &Form::TangentAt {
                    a: Reference::Element(id(pair[0])),
                    b: Reference::Element(id(pair[1])),
                    at: Reference::Element(id("p0000001")),
                    direction,
                },
                "a0 {a0:?}, r0 {r0:?}, {side}, a {}",
                pair[0]
            );
        }
    }
}

// --------------------------------------------- merged from apart: the side, not the drawing

#[test]
fn a_line_and_arc_merged_from_apart_keep_the_arc_on_the_lines_left() {
    // Robustness's first proof. a0 from p0 (0, 0) to p1 (10, 0); r0 about q0 (10, 10),
    // radius 10, from p7 (0, 10) at 180° to p9 (10, 4e-8) at 270°, touching a0 at (10, 0);
    // k0 `left`: q0 is on a0's left, facing +x. Kept p7, a0 runs from (0, 0) up to p7
    // (0, 10), ending there, and r0 starts there. A round on a line's left turns with it,
    // an angle of 0 between their own directions, which the solve reads as `same` where
    // the point ends one and starts the other.
    // The drawing the merge leaves shows the other side: a0 runs +y into p7, and r0, at
    // 180°, leaves it −y, a turn back, with q0 at x = 10 on a0's right.
    // Before the fix: `opposite`, as drawn, which the solve holds with r0 on a0's right.
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("p0000007", 0.0, 10.0),
            point("p0000009", 10.0, 0.00000004),
            point("q0000000", 10.0, 10.0),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &[arc("r0000000", "q0000000", "p0000007", "p0000009")],
        &[tangent("k0000000", "a0000000", "r0000000", "left")],
    );
    let edited = merged(&before, &["p0000007", "p0000001"]);
    tangent_at(
        &edited,
        "k0000000",
        "a0000000",
        "r0000000",
        "p0000007",
        Direction::Same,
    );
    assert_eq!(edited.modified, ids(&["a0000000", "k0000000"]));
    assert_eq!(edited.removed, ids(&["p0000001"]));
}

#[test]
fn two_arcs_merged_from_apart_stay_outside_each_other() {
    // Robustness's second proof. r0 about c0 (0, 0), radius 10, from t0 (10, 0) at 0° to e0
    // (−10, 0) at 180°; r1 about c1 (20, 0), radius 10, from s1 (30, 0) at 0° to t1 (10,
    // 4e-8) at 180°: touching at (10, 0), each outside the other, k0 `outside`. Kept e0, r0
    // ends there and r1 starts there. Two rounds outside each other turn opposite ways where
    // they touch (at (10, 0), r0 runs +y counter-clockwise, r1 −y): an angle of π between
    // their own directions, which the solve reads as `opposite` where the point ends one and
    // starts the other.
    // The drawing the merge leaves shows them one inside the other: at (−10, 0), r0 (180°
    // about (0, 0)) runs −y counter-clockwise, and so does r1, its start moved there (180°
    // about (20, 0)): the path along r0 into e0 and on along r1 runs on, `same` as drawn.
    // Before the fix: `same`.
    let before = doc(
        &[
            point("c0000000", 0.0, 0.0),
            point("c0000001", 20.0, 0.0),
            point("e0000000", -10.0, 0.0),
            point("s0000001", 30.0, 0.0),
            point("t0000000", 10.0, 0.0),
            point("t0000001", 10.0, 0.00000004),
        ],
        &[],
        &[
            arc("r0000000", "c0000000", "t0000000", "e0000000"),
            arc("r0000001", "c0000001", "s0000001", "t0000001"),
        ],
        &[tangent("k0000000", "r0000000", "r0000001", "outside")],
    );
    let edited = merged(&before, &["e0000000", "s0000001"]);
    tangent_at(
        &edited,
        "k0000000",
        "r0000000",
        "r0000001",
        "e0000000",
        Direction::Opposite,
    );
    assert_eq!(edited.modified, ids(&["r0000001", "k0000000"]));
    assert_eq!(edited.removed, ids(&["s0000001"]));
}

#[test]
fn a_line_merged_onto_an_arcs_far_end_keeps_the_side_freecad_would_flip() {
    // The port review's case, a documented departure from FreeCAD (docs/ports/sketch-edit.md,
    // §6). a0 from p0 (0, 0) to p1 (10, 0); r0 about q0 (10, 10), radius 10, from p8 (15,
    // 10 − 5√3) at 300° to p9 (10 + 5√3, 15) at 30°; k0 tangent of r0 (a) and a0 (b),
    // `left`: q0 is on a0's left, facing +x. Kept p9, a0 runs from (0, 0) to p9 and ends
    // there, and r0 ends there: both end at it. `left` is an angle of 0 between their own
    // directions, which the solve reads as `opposite` where the point ends both.
    // FreeCAD 1.1.3 locks the angle its unsolved drawing shows at the point
    // (AutoLockTangencyAndPerpty): a0 runs +x, and r0 at 30° runs (−1/2, √3/2)
    // counter-clockwise, more than a right angle apart, so π, which is `same` here: the
    // solve then holds r0 on a0's right, the side flipped. Silico keeps the stored side.
    let before = doc(
        &[
            point("p0000000", 0.0, 0.0),
            point("p0000001", 10.0, 0.0),
            point("p0000008", 15.0, 1.339_745_962_155_614_5),
            point("p0000009", 18.660_254_037_844_386, 15.0),
            point("q0000000", 10.0, 10.0),
        ],
        &[line("a0000000", "p0000000", "p0000001")],
        &[arc("r0000000", "q0000000", "p0000008", "p0000009")],
        &[tangent("k0000000", "r0000000", "a0000000", "left")],
    );
    let edited = merged(&before, &["p0000009", "p0000001"]);
    tangent_at(
        &edited,
        "k0000000",
        "r0000000",
        "a0000000",
        "p0000009",
        Direction::Opposite,
    );
    assert_eq!(edited.modified, ids(&["a0000000", "k0000000"]));
    assert_eq!(edited.removed, ids(&["p0000001"]));
}

// ------------------------------------------------------------------ property test

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: None,
        max_global_rejects: 100_000,
        rng_seed: RngSeed::Fixed(0x7a46_e171),
        ..ProptestConfig::default()
    }
}

/// An arc's shared end, and how far it sweeps from there to its other end, in
/// milliradians.
#[derive(Debug, Clone)]
struct ArcEnd {
    /// The arc starts at the shared end (its other end counter-clockwise from it), or ends
    /// there.
    starts: bool,
    sweep: i32,
}

fn arc_end() -> impl Strategy<Value = ArcEnd> {
    (any::<bool>(), 200i32..6000).prop_map(|(starts, sweep)| ArcEnd { starts, sweep })
}

/// Two curves that really are tangent, with their shared ends at the contact, the origin;
/// lengths in thousandths of a unit the case scales.
#[derive(Debug, Clone)]
enum Pair {
    /// A line along `along` (milliradians), starting or ending at the contact, and an arc
    /// of `radius` whose centre lies at that distance on the line's left or right.
    LineArc {
        along: i32,
        length: i32,
        line_starts: bool,
        radius: i32,
        left: bool,
        arc: ArcEnd,
        /// The line is the first curve (n1, its shared end s1), or the second.
        line_first: bool,
    },
    /// Two arcs whose circles touch at the contact, the first's centre at `radii.0` from it
    /// against `normal` (milliradians), the second's at `radii.1` along it (outside) or
    /// against it (inside).
    TwoArcs {
        normal: i32,
        radii: (i32, i32),
        inside: bool,
        ends: (ArcEnd, ArcEnd),
    },
}

fn length() -> impl Strategy<Value = i32> {
    prop_oneof![Just(1), Just(2), 1i32..50_000]
}

fn pair() -> impl Strategy<Value = Pair> {
    prop_oneof![
        (
            0i32..6283,
            length(),
            any::<bool>(),
            length(),
            any::<bool>(),
            arc_end(),
            any::<bool>(),
        )
            .prop_map(
                |(along, length, line_starts, radius, left, arc, line_first)| Pair::LineArc {
                    along,
                    length,
                    line_starts,
                    radius,
                    left,
                    arc,
                    line_first,
                }
            ),
        (
            0i32..6283,
            (length(), length()),
            any::<bool>(),
            (arc_end(), arc_end()),
        )
            .prop_map(|(normal, radii, inside, ends)| Pair::TwoArcs {
                normal,
                radii,
                inside,
                ends,
            }),
    ]
}

#[derive(Debug, Clone)]
struct Case {
    pair: Pair,
    /// The second curve's shared end's distance from the first's (up the y axis) in the
    /// touching merge: within the gate's 1e-7.
    gap: f64,
    /// Where the moved shared end is drawn in the merge from apart, in thousandths.
    apart: (i32, i32),
    /// The first curve's shared end is the one moved apart, or the second's.
    move_first: bool,
    scale: f64,
    /// Which shared end is kept: the first curve's, or the second's.
    keep_first: bool,
    /// The tangent's `a` is the first curve.
    first_is_a: bool,
}

fn case() -> impl Strategy<Value = Case> {
    (
        pair(),
        prop_oneof![Just(0.0), Just(0.00000004)],
        (-50_000i32..50_000, -50_000i32..50_000),
        any::<bool>(),
        prop_oneof![Just(0.001), Just(1.0), Just(10.0)],
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(pair, gap, apart, move_first, scale, keep_first, first_is_a)| Case {
                pair,
                gap,
                apart,
                move_first,
                scale,
                keep_first,
                first_is_a,
            },
        )
}

/// The arc `curve`'s JSON: radius `r`, the contact (the origin) at `contact_at` on its
/// circle, and `end` its shared end there (where it's drawn is the caller's); its centre
/// and other end pushed to `points`.
fn arc_json(
    curve: &str,
    end: &str,
    contact_at: f64,
    r: f64,
    shape: &ArcEnd,
    points: &mut Vec<String>,
) -> String {
    let (c, o) = (format!("c{}", &curve[1..]), format!("h{}", &curve[1..]));
    // The contact, the origin, at `contact_at` on the circle.
    let (cx, cy) = (-r * libm::cos(contact_at), -r * libm::sin(contact_at));
    let sweep = f64::from(shape.sweep) / 1000.0;
    let phi = contact_at + if shape.starts { sweep } else { -sweep };
    points.push(point(&c, cx, cy));
    points.push(point(&o, cx + r * libm::cos(phi), cy + r * libm::sin(phi)));
    if shape.starts {
        arc(curve, &c, end, &o)
    } else {
        arc(curve, &c, &o, end)
    }
}

/// The case's sketch, its shared ends s1 (the first curve's) and s2 drawn at `s1` and `s2`;
/// the tangent k0 of n1 and n2 with the side the configuration has.
fn pair_document(case: &Case, s1: [f64; 2], s2: [f64; 2]) -> Option<Document> {
    let mm = |v: i32| f64::from(v) / 1000.0 * case.scale;
    let mut points = vec![
        point("s0000001", s1[0], s1[1]),
        point("s0000002", s2[0], s2[1]),
    ];
    let (mut lines, mut arcs) = (Vec::new(), Vec::new());
    let side = match &case.pair {
        Pair::LineArc {
            along,
            length,
            line_starts,
            radius,
            left,
            arc: shape,
            line_first,
        } => {
            let (l, a) = if *line_first {
                (("n0000001", "s0000001"), ("n0000002", "s0000002"))
            } else {
                (("n0000002", "s0000002"), ("n0000001", "s0000001"))
            };
            // The line runs along `along` through the contact; its other end behind the
            // contact if it ends there, ahead if it starts there.
            let theta = f64::from(*along) / 1000.0;
            let reach = if *line_starts { 1.0 } else { -1.0 } * mm(*length);
            let o = format!("h{}", &l.0[1..]);
            points.push(point(
                &o,
                reach * libm::cos(theta),
                reach * libm::sin(theta),
            ));
            lines.push(if *line_starts {
                line(l.0, l.1, &o)
            } else {
                line(l.0, &o, l.1)
            });
            // The centre a radius off the line, square to it: on its left (facing along
            // it) the contact is a quarter turn clockwise of `along` on the circle, seen
            // from the centre; on its right, counter-clockwise.
            let quarter = std::f64::consts::FRAC_PI_2;
            let contact_at = if *left {
                theta - quarter
            } else {
                theta + quarter
            };
            arcs.push(arc_json(
                a.0,
                a.1,
                contact_at,
                mm(*radius),
                shape,
                &mut points,
            ));
            if *left { "left" } else { "right" }
        }
        Pair::TwoArcs {
            normal,
            radii,
            inside,
            ends,
        } => {
            let theta = f64::from(*normal) / 1000.0;
            arcs.push(arc_json(
                "n0000001",
                "s0000001",
                theta,
                mm(radii.0),
                &ends.0,
                &mut points,
            ));
            // Outside: the second centre beyond the contact, the contact half a turn round
            // its circle. Inside: on the first's side, the contact at the same angle.
            let contact_at = if *inside {
                theta
            } else {
                theta + std::f64::consts::PI
            };
            arcs.push(arc_json(
                "n0000002",
                "s0000002",
                contact_at,
                mm(radii.1),
                &ends.1,
                &mut points,
            ));
            if *inside { "inside" } else { "outside" }
        }
    };
    let (a, b) = names(case);
    read(
        document(&fields(
            &points.join(", "),
            &lines.join(", "),
            "",
            &arcs.join(", "),
            &tangent("k0000000", a, b, side),
        ))
        .as_bytes(),
    )
    .ok()
}

/// The tangent's `a` and `b`.
fn names(case: &Case) -> (&'static str, &'static str) {
    if case.first_is_a {
        ("n0000001", "n0000002")
    } else {
        ("n0000002", "n0000001")
    }
}

/// The merge of the shared ends, the case's one kept: Ok, the gate takes it, and k0 a
/// tangent_at of its a and b at the kept point, listed as modified, the same bytes merged
/// again; its direction, and the merged sketch.
fn merged_direction(case: &Case, before: &Document) -> Result<(Direction, Sketch), TestCaseError> {
    let named = if case.keep_first {
        ["s0000001", "s0000002"]
    } else {
        ["s0000002", "s0000001"]
    };
    let edited = match merge(sketch_of(before), &ids(&named)) {
        Ok(edited) => edited,
        Err(error) => return Err(TestCaseError::fail(format!("refused: {error:?}"))),
    };
    prop_assert_eq!(gate(before, &edited.sketch), Vec::<String>::new());
    let Form::TangentAt {
        a,
        b,
        at,
        direction,
    } = form(&edited.sketch, "k0000000").clone()
    else {
        return Err(TestCaseError::fail(format!(
            "not a tangent_at: {:?}",
            form(&edited.sketch, "k0000000")
        )));
    };
    let (want_a, want_b) = names(case);
    prop_assert_eq!(a, Reference::Element(id(want_a)));
    prop_assert_eq!(b, Reference::Element(id(want_b)));
    prop_assert_eq!(at, Reference::Element(id(named[0])));
    prop_assert!(edited.modified.contains(&id("k0000000")));
    let again = merge(sketch_of(before), &ids(&named)).unwrap();
    prop_assert_eq!(
        write(&with_sketch(before, &again.sketch)).unwrap(),
        write(&with_sketch(before, &edited.sketch)).unwrap()
    );
    Ok((direction, edited.sketch))
}

fn xy_of(sketch: &Sketch, point: &ElementId) -> [f64; 2] {
    let p = sketch.points.iter().find(|p| &p.id == point).unwrap();
    [p.x.mm(), p.y.mm()]
}

/// The way the path runs along `curve` into `at` (`into`), or on from it (not `into`), as a
/// chord of the merged drawing: a line's whole length; an arc's, a thousandth of a radian
/// along its circle through `at`, on the side of `at` the arc lies.
fn chord(sketch: &Sketch, curve: &ElementId, at: &ElementId, into: bool) -> [f64; 2] {
    let k = xy_of(sketch, at);
    let neighbour = if let Some(l) = sketch.lines.iter().find(|l| &l.id == curve) {
        let other = if &l.from == at { &l.to } else { &l.from };
        xy_of(sketch, other)
    } else {
        let a = sketch.arcs.iter().find(|a| &a.id == curve).unwrap();
        let c = xy_of(sketch, &a.center);
        let r = libm::hypot(k[0] - c[0], k[1] - c[1]);
        let theta = libm::atan2(k[1] - c[1], k[0] - c[0]);
        // The arc runs counter-clockwise from its start: inward from the start is ahead,
        // inward from the end behind.
        let inward = if &a.start == at { 0.001 } else { -0.001 };
        [
            c[0] + r * libm::cos(theta + inward),
            c[1] + r * libm::sin(theta + inward),
        ]
    };
    if into {
        [k[0] - neighbour[0], k[1] - neighbour[1]]
    } else {
        [neighbour[0] - k[0], neighbour[1] - k[1]]
    }
}

proptest! {
    #![proptest_config(config(3000))]

    /// Any line and arc, or two arcs, really tangent (the arc's centre a radius to the
    /// line's left or right, or two circles touching outside or inside), their shared ends
    /// at the contact, merged touching: the merge is Ok, the gate takes it, and the tangent
    /// is a tangent_at at the kept point, a and b as they were, its direction the drawing's
    /// chords' (`same` where the path runs on, the angle between the chords under 90°).
    /// Then the same configuration with one shared end moved anywhere and merged from
    /// apart: the same direction, the side alone deciding, whatever the drawing then shows.
    #[test]
    fn any_merged_edge_tangent_keeps_its_side_from_touching_or_apart(case in case()) {
        let Some(touching) = pair_document(&case, [0.0, 0.0], [0.0, case.gap]) else {
            return Err(TestCaseError::fail("unreadable"));
        };
        prop_assume!(validate(&touching).is_ok());
        let (direction, merged) = merged_direction(&case, &touching)?;
        let kept = id(if case.keep_first { "s0000001" } else { "s0000002" });
        let (a, b) = names(&case);
        let u = chord(&merged, &id(a), &kept, true);
        let v = chord(&merged, &id(b), &kept, false);
        let (nu, nv) = (libm::hypot(u[0], u[1]), libm::hypot(v[0], v[1]));
        let cos = (u[0] * v[0] + u[1] * v[1]) / (nu * nv);
        // Really tangent: the chords within a few degrees of one line.
        prop_assert!(cos.abs() > 0.99, "cos {}", cos);
        let want = if cos > 0.0 { Direction::Same } else { Direction::Opposite };
        prop_assert_eq!(direction, want, "touching, cos {}", cos);

        let moved = [
            f64::from(case.apart.0) / 1000.0 * case.scale,
            f64::from(case.apart.1) / 1000.0 * case.scale,
        ];
        let (s1, s2) = if case.move_first {
            (moved, [0.0, case.gap])
        } else {
            ([0.0, 0.0], moved)
        };
        let Some(apart) = pair_document(&case, s1, s2) else {
            return Err(TestCaseError::fail("unreadable"));
        };
        prop_assume!(validate(&apart).is_ok());
        prop_assert_eq!(merged_direction(&case, &apart)?.0, direction, "apart");
    }
}
