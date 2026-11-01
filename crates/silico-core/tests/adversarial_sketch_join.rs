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

/// Arcs about the origin, a0 from `r0` to `n0` and b0 from `n0` on to `r1`, with their
/// sweeps and the joined one's, each as the document measures an arc: its ends' angles
/// about the centre by `libm::atan2`, a turn added to the end's where it's below the
/// start's (none, for the joined one, where its ends are at one angle).
fn about_the_origin(r0: [f64; 2], n0: [f64; 2], r1: [f64; 2]) -> (Document, [Option<f64>; 3]) {
    let document = doc(
        &[
            crate::fixture::point_debug("c0000000", 0.0, 0.0),
            crate::fixture::point_debug("r0000000", r0[0], r0[1]),
            crate::fixture::point_debug("n0000000", n0[0], n0[1]),
            crate::fixture::point_debug("r0000001", r1[0], r1[1]),
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
    );
    let sweep = |from: [f64; 2], to: [f64; 2]| {
        let (start, end) = (libm::atan2(from[1], from[0]), libm::atan2(to[1], to[0]));
        let end = if end < start {
            end + std::f64::consts::TAU
        } else {
            end
        };
        (end > start).then_some(end - start)
    };
    (document, [sweep(r0, n0), sweep(n0, r1), sweep(r0, r1)])
}

#[test]
fn arcs_an_ulp_past_a_turn_by_libm_are_refused_and_just_short_of_it_join() {
    // The review's case: r1 a few ulps of x past r0, which libm puts one ulp of angle past
    // r0's, so the two pieces sweep a whole turn and a hair between them, and the joined
    // arc, from r0 to r1, the hair alone, less than either piece: refused.
    let r0 = [0.229_607_899_042_674_23, -4.994_725_238_959_317];
    let n0 = [3.222_034_576_183_325_3, -3.823_413_813_580_625];
    let past = [0.229_607_899_042_675_34, -4.994_725_238_959_317];
    let (before, [a, b, joined]) = about_the_origin(r0, n0, past);
    let (a, b, joined) = (a.unwrap(), b.unwrap(), joined.unwrap());
    let ulp = |v: f64| f64::from_bits(v.abs().to_bits() + 1) - v.abs();
    let start = libm::atan2(r0[1], r0[0]);
    assert_eq!(libm::atan2(past[1], past[0]) - start, ulp(start));
    assert_eq!(joined, ulp(start));
    assert!(joined < a && joined < b, "{a} {b} {joined}");
    let error = join(sketch_of(&before), &id("n0000000")).unwrap_err();
    assert_eq!(
        error.message(),
        "arcs a0000000 and b0000000 sweep a whole turn or more between them, so one arc can't join them at point n0000000"
    );
    assert_eq!(error.elements(), ["n0000000", "a0000000", "b0000000"]);
    // r1 2.5e-7 mm short of r0 in x, as near as the gate lets an arc's two ends be (more
    // than 1e-7 mm apart), some 5e-8 of angle: the joined arc sweeps all but that, less
    // than a turn, more than either piece, and it's one arc from r0 to r1.
    let short = [r0[0] - 2.5e-7, r0[1]];
    let (before, [a, b, joined]) = about_the_origin(r0, n0, short);
    let (a, b, joined) = (a.unwrap(), b.unwrap(), joined.unwrap());
    assert!(joined < std::f64::consts::TAU, "{joined}");
    assert!(joined > a && joined > b, "{a} {b} {joined}");
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
