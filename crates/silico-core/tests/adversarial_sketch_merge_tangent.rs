//! Hostile inputs for a merge's edge-tangency conversion (`document::sketch_edit::merge`:
//! an edge `tangent` between two curves that the merge makes share one end becomes a
//! `tangent_at` there, its `direction` as the merged drawing shows it), from break-it.
//!
//! Every expected `direction` is worked by hand from the geometry, or, in the property
//! test, by an independent reading of the drawing: the path's chords a thousandth of a
//! radian (or the line's whole length) either side of the shared point, rather than the
//! curves' own directions and which end is which that the code reads.

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
fn a_lone_kept_point_is_where_the_direction_is_read() {
    // Kept x0 (10, 20): a0 runs from (0, 0) to it, arriving along (1, 2); r0 starts there,
    // at 90°, counter-clockwise −x. Their dot product is −1: `opposite`. Read at p1's or
    // p9's place (10, 0), where r0 would leave +x, it'd be `same`.
    let before = line_and_arc_with_lone_point();
    let edited = merged(&before, &["x0000000", "p0000001", "p0000009"]);
    tangent_at(
        &edited,
        "k0000000",
        "a0000000",
        "r0000000",
        "x0000000",
        Direction::Opposite,
    );
    assert_eq!(edited.removed, ids(&["p0000001", "p0000009"]));
    assert_eq!(edited.modified, ids(&["a0000000", "r0000000", "k0000000"]));
    // Kept p1 (10, 0), x0 merged into it too: there r0 leaves +x along a0's +x. `same`.
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
    let before = line_to((10.0, 10.0), true);
    let _ = refused_or_gated(&before, &["p0000001", "p0000009"]);
}

#[test]
fn an_arcs_end_merged_onto_its_other_ends_place_beside_an_edge_tangent() {
    // a0's end p1 drawn at (20, 10), r0's other end's place: kept p1, r0 would run from
    // (20, 10) to (20, 10), and k0 would be a tangent_at on an arc of no sweep.
    // Before its fix: merge returns Ok; the gate refuses "arc r0000000's start and end,
    // points p0000001 and p0000008, are both drawn at (20, 10), so its sweep is 0 or a
    // whole turn".
    let before = line_to((20.0, 10.0), true);
    let _ = refused_or_gated(&before, &["p0000001", "p0000009"]);
}

#[test]
fn a_lines_end_merged_onto_its_other_ends_place_beside_an_edge_tangent() {
    // r0's start p9 drawn at (0, 0), a0's start p0's place (r0's ends off its circle, the
    // solve's to mend). Kept p9, a0 would run from (0, 0) to (0, 0): no length, and no
    // direction for the tangent_at.
    // Before its fix: merge returns Ok; the gate refuses "line a0000000 is 0 mm long, from
    // point p0000000 at (0, 0) to p0000009 at (0, 0)".
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
    let _ = refused_or_gated(&before, &["p0000009", "p0000001"]);
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
    let edited = refused_or_gated(&before, &["p0000001", "p0000009"]);
    if let Err(error) = edited {
        assert_eq!(error.parameter(), Some("points"), "{error:?}");
    }
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

/// One curve of the pair: a line from (or to) the shared end, to another point; or an arc
/// about a centre, the shared end at one angle and its other end at another.
#[derive(Debug, Clone)]
enum Piece {
    Line {
        other: (i32, i32),
        starts: bool,
    },
    Arc {
        radius: i32,
        /// The shared end's angle, and the other end's, in milliradians.
        at: i32,
        sweep: i32,
        starts: bool,
    },
}

fn piece(line: bool) -> BoxedStrategy<Piece> {
    let xy = || (-50_000i32..50_000, -50_000i32..50_000);
    if line {
        (xy(), any::<bool>())
            .prop_map(|(other, starts)| Piece::Line { other, starts })
            .boxed()
    } else {
        (
            prop_oneof![Just(1), Just(2), 1i32..50_000],
            0i32..6283,
            200i32..6000,
            any::<bool>(),
        )
            .prop_map(|(radius, at, sweep, starts)| Piece::Arc {
                radius,
                at,
                sweep,
                starts,
            })
            .boxed()
    }
}

/// Where the second curve's shared end lies against the first's: touching (within the
/// gate's 1e-7, or a little more), or anywhere, a merge from apart.
#[derive(Debug, Clone)]
enum Gap {
    Touching(f64),
    Apart((i32, i32)),
}

fn gap() -> impl Strategy<Value = Gap> {
    prop_oneof![
        prop_oneof![Just(0.0), Just(0.00000004), Just(0.0000001), Just(0.001)]
            .prop_map(Gap::Touching),
        (-50_000i32..50_000, -50_000i32..50_000).prop_map(Gap::Apart),
    ]
}

#[derive(Debug, Clone)]
struct Case {
    first: Piece,
    second: Piece,
    gap: Gap,
    scale: f64,
    /// Which shared end is kept: the first curve's, or the second's.
    keep_first: bool,
    /// The tangent's `a` is the first curve.
    first_is_a: bool,
}

fn case() -> impl Strategy<Value = Case> {
    (any::<bool>(), any::<bool>())
        .prop_flat_map(|(first_line, second_line)| {
            // Two lines can't be tangent: one at least is an arc.
            let second_line = second_line && !first_line;
            (
                piece(first_line),
                piece(second_line),
                gap(),
                prop_oneof![Just(0.001), Just(1.0), Just(10.0)],
                any::<bool>(),
                any::<bool>(),
            )
        })
        .prop_map(|(first, second, gap, scale, keep_first, first_is_a)| Case {
            first,
            second,
            gap,
            scale,
            keep_first,
            first_is_a,
        })
}

/// Whether the curve is a line, and its JSON, its shared end `end` at `xy`; its other
/// points' JSON pushed to `points`.
fn curve_json(
    piece: &Piece,
    curve: &str,
    end: &str,
    xy: [f64; 2],
    scale: f64,
    points: &mut Vec<String>,
) -> (bool, String) {
    let mm = |v: i32| f64::from(v) / 1000.0 * scale;
    match piece {
        Piece::Line { other, starts } => {
            let o = format!("h{}", &curve[1..]);
            points.push(point(&o, mm(other.0), mm(other.1)));
            let json = if *starts {
                line(curve, end, &o)
            } else {
                line(curve, &o, end)
            };
            (true, json)
        }
        Piece::Arc {
            radius,
            at,
            sweep,
            starts,
        } => {
            let c = format!("c{}", &curve[1..]);
            let o = format!("h{}", &curve[1..]);
            // The centre placed so the shared end lies on the circle.
            let (r, theta) = (mm(*radius), f64::from(*at) / 1000.0);
            let cx = xy[0] - r * libm::cos(theta);
            let cy = xy[1] - r * libm::sin(theta);
            let phi = theta + if *starts { 1.0 } else { -1.0 } * f64::from(*sweep) / 1000.0;
            points.push(point(&c, cx, cy));
            points.push(point(&o, cx + r * libm::cos(phi), cy + r * libm::sin(phi)));
            let json = if *starts {
                arc(curve, &c, end, &o)
            } else {
                arc(curve, &c, &o, end)
            };
            (false, json)
        }
    }
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

    /// Any line and arc, or two arcs, edge tangent, whose ends a merge joins (touching, or
    /// from apart): the merge is Ok, the gate takes it, and the tangent is a tangent_at at
    /// the kept point, a and b as they were, its direction the chords' (`same` where the
    /// path runs on, the angle between the chords under 90°), wherever that isn't within
    /// a thousandth of a right angle.
    #[test]
    fn any_merged_edge_tangent_reads_the_drawing(case in case()) {
        let mut points = Vec::new();
        let s1 = [0.0, 0.0];
        let s2 = match case.gap {
            Gap::Touching(d) => [0.0, d],
            Gap::Apart((x, y)) => [f64::from(x) / 1000.0 * case.scale, f64::from(y) / 1000.0 * case.scale],
        };
        points.push(point("s0000001", s1[0], s1[1]));
        points.push(point("s0000002", s2[0], s2[1]));
        let (first_line, first) =
            curve_json(&case.first, "n0000001", "s0000001", s1, case.scale, &mut points);
        let (second_line, second) =
            curve_json(&case.second, "n0000002", "s0000002", s2, case.scale, &mut points);
        let (lines, arcs): (Vec<_>, Vec<_>) = [(first_line, first), (second_line, second)]
            .into_iter()
            .partition(|(is_line, _)| *is_line);
        let side = if first_line || second_line { "left" } else { "outside" };
        let (a, b) = if case.first_is_a {
            ("n0000001", "n0000002")
        } else {
            ("n0000002", "n0000001")
        };
        let Ok(before) = read(
            document(&fields(
                &points.join(", "),
                &lines.into_iter().map(|(_, j)| j).collect::<Vec<_>>().join(", "),
                "",
                &arcs.into_iter().map(|(_, j)| j).collect::<Vec<_>>().join(", "),
                &tangent("k0000000", a, b, side),
            ))
            .as_bytes(),
        ) else {
            return Err(TestCaseError::fail("unreadable"));
        };
        prop_assume!(validate(&before).is_ok());
        let named = if case.keep_first {
            ["s0000001", "s0000002"]
        } else {
            ["s0000002", "s0000001"]
        };
        let edited = merge(sketch_of(&before), &ids(&named));
        let edited = match edited {
            Ok(edited) => edited,
            Err(error) => {
                return Err(TestCaseError::fail(format!("refused: {error:?}")));
            }
        };
        prop_assert_eq!(gate(&before, &edited.sketch), Vec::<String>::new());
        let Form::TangentAt { a: ga, b: gb, at, direction } = form(&edited.sketch, "k0000000").clone() else {
            return Err(TestCaseError::fail(format!("not a tangent_at: {:?}", form(&edited.sketch, "k0000000"))));
        };
        prop_assert_eq!(ga, Reference::Element(id(a)));
        prop_assert_eq!(gb, Reference::Element(id(b)));
        prop_assert_eq!(&at, &Reference::Element(id(named[0])));
        prop_assert!(edited.modified.contains(&id("k0000000")));
        let kept = id(named[0]);
        let u = chord(&edited.sketch, &id(a), &kept, true);
        let v = chord(&edited.sketch, &id(b), &kept, false);
        let (nu, nv) = (libm::hypot(u[0], u[1]), libm::hypot(v[0], v[1]));
        prop_assume!(nu > 0.0 && nv > 0.0);
        let cos = (u[0] * v[0] + u[1] * v[1]) / (nu * nv);
        prop_assume!(cos.abs() > 1e-3);
        let want = if cos > 0.0 { Direction::Same } else { Direction::Opposite };
        prop_assert_eq!(direction, want, "cos {}", cos);
        // The same merge again: the same bytes.
        let again = merge(sketch_of(&before), &ids(&named)).unwrap();
        prop_assert_eq!(
            write(&with_sketch(&before, &again.sketch)).unwrap(),
            write(&with_sketch(&before, &edited.sketch)).unwrap()
        );
    }
}
