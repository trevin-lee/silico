//! `/break-it` on the join (docs/design/m5.0-app.md, `sketch_join`): hostile sketches the
//! core's `join` meets. Each expected value by hand: a circle's other split point a third
//! curve still uses, arcs whose sweeps meet a whole turn exactly or a hair past it, and a
//! sketch as large as the gate takes whose constraints on both pieces are each a repeat once
//! re-pointed.

use silico_core::document::sketch_edit::join;
use silico_core::document::{Document, ElementId, Operation, Sketch, read, validate};

use crate::fixture::{arc, document, fields, line, point};

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn doc(points: &str, lines: &str, circles: &str, arcs: &str, constraints: &str) -> Document {
    let document =
        read(document(&fields(points, lines, circles, arcs, constraints)).as_bytes()).unwrap();
    if let Err(invalid) = validate(&document) {
        panic!("the gate refused the fixture: {:?}", invalid.errors());
    }
    document
}

fn sketch_of(document: &Document) -> &Sketch {
    match &document.features[0].operation {
        Operation::Sketch(sketch) => sketch,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_split_circles_other_point_a_third_curve_uses_refuses_the_join() {
    // Found by break-it: the circle's two arcs, n0 to n1 and n1 to n0, and a line from n0
    // out to (10, 2). Joined at n1, the circle would drop n0, the line's start: refused,
    // naming it, where the join first removed the point from under the line.
    let before = doc(
        &[
            point("z0000000", 1.0, 2.0),
            point("n0000000", 4.0, 2.0),
            point("n0000001", -2.0, 2.0),
            point("e0000001", 10.0, 2.0),
        ]
        .join(", "),
        &line("e0000000", "n0000000", "e0000001"),
        "",
        &[
            arc("c0000000", "z0000000", "n0000000", "n0000001"),
            arc("c1000000", "z0000000", "n0000001", "n0000000"),
        ]
        .join(", "),
        "",
    );
    let error = join(sketch_of(&before), &id("n0000001")).unwrap_err();
    assert_eq!(
        error.message(),
        "joining arcs c0000000 and c1000000 at point n0000001 makes them a circle, which would leave line e0000000 without point n0000000, their other shared point"
    );
    assert_eq!(
        error.elements(),
        ["n0000001", "c0000000", "c1000000", "n0000000", "e0000000"]
    );
    assert_eq!(error.parameter(), Some("point"));
    assert_eq!(
        error.suggestion(),
        Some("unjoin the other curves from that point first")
    );
}

/// Arcs about the origin: a0 from (5, 0) to n0 (−5, 0), half a turn, and b0 from n0 on to
/// `end`, another point than a0's start.
fn halves(end: [f64; 2]) -> Document {
    doc(
        &[
            point("c0000000", 0.0, 0.0),
            point("r0000000", 5.0, 0.0),
            point("n0000000", -5.0, 0.0),
            point("r0000001", end[0], end[1]),
        ]
        .join(", "),
        "",
        "",
        &[
            arc("a0000000", "c0000000", "r0000000", "n0000000"),
            arc("b0000000", "c0000000", "n0000000", "r0000001"),
        ]
        .join(", "),
        "",
    )
}

#[test]
fn arcs_whose_sweeps_meet_a_turn_exactly_or_a_hair_past_it_are_refused_and_short_of_it_join() {
    let over = "arcs a0000000 and b0000000 sweep a whole turn or more between them, so one arc can't join them at point n0000000";
    // b0's end at (5, 0), a point of its own where a0 starts: exactly a turn between them.
    let error = join(sketch_of(&halves([5.0, 0.0])), &id("n0000000")).unwrap_err();
    assert_eq!(error.message(), over);
    // At (5, 1e-9), a hair past a0's start: more than a turn.
    let error = join(sketch_of(&halves([5.0, 1e-9])), &id("n0000000")).unwrap_err();
    assert_eq!(error.message(), over);
    // At (5, −1e-6), short of it: one arc from (5, 0) round to there, nearly a turn.
    let before = halves([5.0, -1e-6]);
    let joined = join(sketch_of(&before), &id("n0000000")).unwrap();
    let arcs = &joined.edited.sketch.arcs;
    assert_eq!(arcs.len(), 1);
    assert_eq!(
        (
            arcs[0].id.as_str(),
            arcs[0].start.as_str(),
            arcs[0].end.as_str()
        ),
        ("a0000000", "r0000000", "r0000001")
    );
}

#[test]
fn many_constraints_on_both_pieces_each_a_repeat_once_re_pointed_are_each_dropped() {
    // 500 points on the line's carrier past its end (with the line's three, 1,006 of the
    // solver's 1,024 unknowns, the gate's bound), each held on both pieces: once b1's are
    // re-pointed to a0 each repeats a0's own, so all 500 of b1's go and a0's stay.
    const N: usize = 500;
    let name = |prefix: char, i: usize| format!("{prefix}{i:07}");
    let mut points = vec![
        point("p0000000", 0.0, 0.0),
        point("n0000000", 8.0, 0.0),
        point("p0000004", 32.0, 0.0),
    ];
    let mut constraints = vec![
        r#"{"id": "j0000000", "type": "tangent_at", "a": "a0000000", "b": "b1000000", "at": "n0000000", "direction": "same"}"#.to_owned(),
    ];
    for i in 0..N {
        let p = name('q', i);
        points.push(point(&p, 40.0 + i as f64, 0.0));
        constraints.push(format!(
            r#"{{"id": "{}", "type": "on", "point": "{p}", "curve": "a0000000"}}"#,
            name('k', i)
        ));
        constraints.push(format!(
            r#"{{"id": "{}", "type": "on", "point": "{p}", "curve": "b1000000"}}"#,
            name('m', i)
        ));
    }
    let before = doc(
        &points.join(", "),
        &[
            line("a0000000", "p0000000", "n0000000"),
            line("b1000000", "n0000000", "p0000004"),
        ]
        .join(", "),
        "",
        "",
        &constraints.join(", "),
    );
    let joined = join(sketch_of(&before), &id("n0000000")).unwrap();
    let sketch = &joined.edited.sketch;
    assert_eq!(sketch.constraints.len(), N);
    assert!(
        sketch
            .constraints
            .iter()
            .all(|c| c.id.as_str().starts_with('k'))
    );
    // The point, the piece, the join, and every one of b1's.
    assert_eq!(joined.edited.removed.len(), 3 + N);
    assert_eq!(joined.edited.modified, [id("a0000000")]);
}
