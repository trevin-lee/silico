//! /break-it on decision 5 per part (silico-core's `Translation::read`): one window per
//! distinct part of the sketch, ε = max(1e-8, 1e-9 × E_part, 1e-9 / max(T_part, 1e-5)), and
//! the worst row reported (largest gap, not-a-number largest, lowest tag on a tie). The
//! worker's answers are made up here, so each gap is placed by hand; every expected
//! tolerance is a hand calculation from the part's own points.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/sketch.rs"]
mod fixture;

use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::document::{Operation, Sketch, read, validate};
use silico_core::sketch::{SketchSolution, Translation};
use silico_protocol::{
    Algorithm, PointValues, SketchGeometry, SolveStatus, Solved, SolverConstraint, Tag,
};

fn point(id: &str, x: f64, y: f64) -> String {
    format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
}

fn circle(id: &str, center: &str, radius: f64) -> String {
    format!(r#"{{"id": "{id}", "center": "{center}", "radius": {radius}, "construction": false}}"#)
}

fn row(id: &str, kind: &str, a: &str, b: &str, value: f64, driving: bool) -> String {
    format!(
        r#"{{"id": "{id}", "type": "{kind}", "a": "{a}", "b": "{b}", "value": {value}, "driving": {driving}}}"#
    )
}

fn text(points: &[String], circles: &[String], constraints: &[String]) -> String {
    fixture::document(&fixture::fields(
        &points.join(", "),
        "",
        &circles.join(", "),
        "",
        &constraints.join(", "),
    ))
}

/// The sketch, through the gate.
fn translate(text: &str) -> Translation {
    let document = read(text.as_bytes()).unwrap();
    if let Err(e) = validate(&document) {
        panic!("{:?}", e.errors());
    }
    translate_unchecked(text)
}

/// The sketch, not through the gate (as `Translation::new` is public).
fn translate_unchecked(text: &str) -> Translation {
    let sketch: Sketch = match &read(text.as_bytes()).unwrap().features[0].operation {
        Operation::Sketch(s) => s.clone(),
        other => panic!("{other:?}"),
    };
    Translation::new(&sketch).unwrap()
}

fn answer(t: &Translation, values: Vec<f64>, dropped: &[u32]) -> Solved {
    let mut tags: Vec<Tag> = t
        .system
        .constraints
        .iter()
        .filter_map(|c| c.tag())
        .collect();
    tags.sort_unstable();
    Solved {
        dofs: 0,
        conflicting: Vec::new(),
        redundant: dropped.iter().map(|&t| Tag(t)).collect(),
        partially_redundant: Vec::new(),
        free: Vec::new(),
        no_equations: false,
        status: SolveStatus::SolvedExceptRedundant,
        algorithm: Some(Algorithm::DogLeg),
        values,
        residuals: tags.into_iter().map(|t| (t, Some(0.0))).collect(),
    }
}

fn read_back(t: &Translation, solved: &Solved) -> Result<SketchSolution, silico_core::Error> {
    t.check_reply(solved)?;
    t.read(solved)
}

/// The values of `t`'s `i`th point, in ID order.
fn pv_of(t: &Translation, i: usize) -> PointValues {
    t.system
        .geometry
        .iter()
        .skip(3)
        .filter_map(|g| match *g {
            SketchGeometry::Point(p) => Some(p),
            _ => None,
        })
        .nth(i)
        .unwrap()
}

fn pid(i: usize) -> String {
    format!("p{i:07}")
}

fn kid(i: usize) -> String {
    format!("k{i:07}")
}

#[test]
fn two_hundred_parts_are_each_judged_by_their_own_window() {
    // p0 .. p199 at (10 i, 5 i), each held from the origin alone (its x twice, its y
    // once): 200 parts of one point each, E 0, ε the floor, 1e-8 for every one, though the
    // sketch spans (0, 0) to (1990, 995), E 2224.9, whose ε would be 2.2e-6. Each point's
    // second x is dropped, moved off by 0.99e-8 on even points and 1.01e-8 on odd ones,
    // and by 3e-8 on p151: the odd ones are refused by name, the worst p151's.
    let n = 200;
    let mut points = Vec::new();
    let mut constraints = Vec::new();
    for i in 0..n {
        let (x, y) = (10.0 * i as f64, 5.0 * i as f64);
        points.push(point(&pid(i), x, y));
        constraints.push(row(
            &kid(3 * i + 1),
            "distance_x",
            "origin",
            &pid(i),
            x,
            true,
        ));
        constraints.push(row(
            &kid(3 * i + 2),
            "distance_x",
            "origin",
            &pid(i),
            x,
            true,
        ));
        constraints.push(row(
            &kid(3 * i + 3),
            "distance_y",
            "origin",
            &pid(i),
            y,
            true,
        ));
    }
    let t = translate(&text(&points, &[], &constraints));
    let mut values = t.system.values.clone();
    for i in 0..n {
        let off = match i {
            151 => 3e-8,
            _ if i % 2 == 0 => 0.99e-8,
            _ => 1.01e-8,
        };
        values[pv_of(&t, i).x.0 as usize] += off;
    }
    let dropped: Vec<u32> = (0..n).map(|i| 3 * i as u32 + 2).collect();
    let e = read_back(&t, &answer(&t, values, &dropped)).unwrap_err();
    let odd: Vec<String> = (0..n)
        .filter(|i| i % 2 == 1)
        .map(|i| kid(3 * i + 2))
        .collect();
    assert_eq!(
        (e.code(), e.elements()),
        (ErrorCode::OverConstrained, &odd[..]),
        "{e}"
    );
    let m = e.message();
    // 3e-8 added to 1510 lands 2.99999...e-8 off in doubles.
    assert!(m.contains("by up to 2.99999"), "{m}");
    assert!(
        m.contains(&format!("mm ({}), past the 1e-8 mm", kid(3 * 151 + 2))),
        "{m}"
    );
    assert!(m.contains("of the part's 0e0 mm extent"), "{m}");
}

#[test]
fn a_reference_dimension_joins_nothing_and_a_driving_one_does() {
    // p1 (0, 0), its x given twice from the origin, the second dropped; p2 (1000, 0) held
    // from the origin. A reference distance from p1 to p2 isn't sent, so it joins nothing:
    // p1's part is p1, ε 1e-8, and 1.01e-8 is refused. Made driving, the distance is sent
    // and joins p1 to p2: E 1000, ε 1e-6, and 1.01e-8 is kept (decision 5's design: a kept
    // row joins as PlaneGCS solves the two together).
    for (driving, kept) in [(false, false), (true, true)] {
        let t = translate(&text(
            &[point(&pid(1), 0.0, 0.0), point(&pid(2), 1000.0, 0.0)],
            &[],
            &[
                row(&kid(1), "distance_x", "origin", &pid(1), 0.0, true),
                row(&kid(2), "distance_x", "origin", &pid(1), 0.0, true),
                row(&kid(3), "distance_y", "origin", &pid(1), 0.0, true),
                row(&kid(4), "distance_x", "origin", &pid(2), 1000.0, true),
                row(&kid(5), "distance_y", "origin", &pid(2), 0.0, true),
                row(&kid(6), "distance", &pid(1), &pid(2), 1000.0, driving),
            ],
        ));
        let mut values = t.system.values.clone();
        values[pv_of(&t, 0).x.0 as usize] += 1.01e-8;
        let got = read_back(&t, &answer(&t, values, &[2]));
        assert_eq!(got.is_ok(), kept, "driving {driving}: {got:?}");
        if let Err(e) = got {
            assert!(e.message().contains("past the 1e-8 mm"), "{e}");
        }
    }
}

#[test]
fn a_circles_radius_held_alone_is_a_part_of_no_extent() {
    // c1, radius 1000, about p1 held at the origin; its radius given (k3) and its diameter,
    // 2000 (k4), dropped. PlaneGCS reads a radius row's radius alone, so the radius is a
    // part of its own, no point in it: E 0, ε 1e-8, though the circle is 2 m across.
    // 0.99e-8 kept; 1.01e-8 refused naming k4.
    let t = translate(&text(
        &[point(&pid(1), 0.0, 0.0)],
        &[circle("c0000001", &pid(1), 1000.0)],
        &[
            row(&kid(1), "distance_x", "origin", &pid(1), 0.0, true),
            row(&kid(2), "distance_y", "origin", &pid(1), 0.0, true),
            r#"{"id": "k0000003", "type": "radius", "curve": "c0000001", "value": 1000, "driving": true}"#.to_owned(),
            r#"{"id": "k0000004", "type": "diameter", "curve": "c0000001", "value": 2000, "driving": true}"#.to_owned(),
        ],
    ));
    let r = t.system.values.iter().position(|&v| v == 1000.0).unwrap();
    let at = |off: f64| {
        let mut values = t.system.values.clone();
        values[r] += off;
        read_back(&t, &answer(&t, values, &[4]))
    };
    let s = at(0.99e-8).unwrap();
    assert!((s.redundant.values().next().unwrap() - 0.99e-8).abs() < 1e-13);
    let e = at(1.01e-8).unwrap_err();
    assert_eq!(e.elements(), &["k0000004".to_owned()][..], "{e}");
    assert!(e.message().contains("of the part's 0e0 mm extent"), "{e}");
}

#[test]
fn a_dropped_row_that_reaches_a_tangency_carries_its_window() {
    // Two circles tangent inside, c1 r 1 about p1 (0, 0), c2 r 1.03125 about p2 (0.03125,
    // 0) (T = 0.0625, ε 1.6e-8), and p3 (0.01, 0.01): its x held from the origin (k3, kept)
    // and given again from p1 (k4, dropped). The dropped row reads p1, whose x carries the
    // tangency's slack, so its part is the circles' and its ε 1.6e-8: 1.5e-8 kept. Given
    // from the origin both times, p3 is a part of its own: 1.5e-8 refused.
    for (from, kept) in [(pid(1), true), ("origin".to_owned(), false)] {
        let t = translate(&text(
            &[
                point(&pid(1), 0.0, 0.0),
                point(&pid(2), 0.03125, 0.0),
                point(&pid(3), 0.01, 0.01),
            ],
            &[circle("c0000001", &pid(1), 1.0), circle("c0000002", &pid(2), 1.03125)],
            &[
                r#"{"id": "k0000001", "type": "tangent", "a": "c0000001", "b": "c0000002", "side": "inside"}"#.to_owned(),
                row(&kid(3), "distance_x", "origin", &pid(3), 0.01, true),
                row(&kid(4), "distance_x", &from, &pid(3), 0.01, true),
            ],
        ));
        let mut values = t.system.values.clone();
        values[pv_of(&t, 2).x.0 as usize] += 1.5e-8;
        let got = read_back(&t, &answer(&t, values, &[3]));
        assert_eq!(got.is_ok(), kept, "from {from}: {got:?}");
    }
}

#[test]
fn a_tag_never_sent_or_tag_zero_is_a_protocol_error() {
    let t = translate(&text(
        &[point(&pid(1), 0.0, 0.0)],
        &[],
        &[
            row(&kid(1), "distance_x", "origin", &pid(1), 0.0, true),
            row(&kid(2), "distance_x", "origin", &pid(1), 0.0, true),
        ],
    ));
    for tag in [0, 3, u32::MAX] {
        let solved = answer(&t, t.system.values.clone(), &[tag]);
        let e = read_back(&t, &solved).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ProtocolError, "tag {tag}: {e}");
        // And read() alone, past the reply check, doesn't panic either.
        let e = t.read(&solved).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ProtocolError, "tag {tag}: {e}");
    }
}

#[test]
fn a_row_reading_only_fixed_values_is_judged_without_a_panic() {
    // Past the gate (which refuses a constraint naming only the origin and axes):
    // perpendicular x_axis and y_axis reads no unknown, so it's in no part. Its gap has no
    // lever (neither axis can move), so it isn't a number, and it's refused, the worst.
    // A finite row beside it, p1's x 5e-8 off, is named too.
    let t = translate_unchecked(&text(
        &[point(&pid(1), 0.0, 0.0)],
        &[],
        &[
            row(&kid(1), "distance_x", "origin", &pid(1), 0.0, true),
            row(&kid(2), "distance_x", "origin", &pid(1), 0.0, true),
            r#"{"id": "k0000003", "type": "perpendicular", "a": "x_axis", "b": "y_axis"}"#
                .to_owned(),
        ],
    ));
    assert!(
        t.system
            .constraints
            .iter()
            .any(|c| matches!(c, SolverConstraint::Perpendicular { .. }))
    );
    let mut values = t.system.values.clone();
    values[pv_of(&t, 0).x.0 as usize] += 5e-8;
    let e = read_back(&t, &answer(&t, values, &[2, 3])).unwrap_err();
    assert_eq!(
        (e.code(), e.elements()),
        (
            ErrorCode::OverConstrained,
            &["k0000002".to_owned(), "k0000003".to_owned()][..]
        ),
        "{e}"
    );
    assert!(e.message().contains("by up to NaN mm (k0000003)"), "{e}");
}

#[test]
fn the_largest_sketch_of_one_point_parts_reads_its_drops_quickly() {
    // 512 points (1,024 unknowns, the most), each held from the origin alone, its x given
    // 16 times and its y once (8,704 constraints, every value within the 10,000): 512 parts,
    // 7,680 rows dropped, every one met. Each window is worked out once per part; each
    // costs a pass over every constraint, so this is the most work decision 5 does.
    let n = 512;
    let mut points = Vec::new();
    let mut constraints = Vec::new();
    let mut id = 0;
    let mut dropped = Vec::new();
    for i in 0..n {
        let (x, y) = (i as f64, -(i as f64) - 1.0);
        points.push(point(&pid(i), x, y));
        for j in 0..16 {
            id += 1;
            constraints.push(row(&kid(id), "distance_x", "origin", &pid(i), x, true));
            if j > 0 {
                dropped.push(id as u32);
            }
        }
        id += 1;
        constraints.push(row(&kid(id), "distance_y", "origin", &pid(i), y, true));
    }
    let t = translate(&text(&points, &[], &constraints));
    let solved = answer(&t, t.system.values.clone(), &dropped);
    t.check_reply(&solved).unwrap();
    // Measured 3.3 s in a debug build and 0.39 s in release (one part, below: 0.24 s and
    // 21 ms): each part's window passes over every constraint.
    let start = Instant::now();
    let s = t.read(&solved).unwrap();
    let took = start.elapsed();
    assert_eq!(s.redundant.len(), dropped.len());
    assert!(took < Duration::from_secs(10), "{took:?}");
}

#[test]
fn the_largest_sketch_in_one_part_reads_its_drops_as_quickly() {
    // The sketch above with its points chained into one part by horizontal_points rows
    // (each pair drawn level for it: y = −1 for all): one window, worked out once. The
    // gap between this and the 512-part read is the per-part windows' cost.
    let n = 512;
    let mut points = Vec::new();
    let mut constraints = Vec::new();
    let mut id = 0;
    let mut dropped = Vec::new();
    for i in 0..n {
        let x = i as f64;
        points.push(point(&pid(i), x, -1.0));
        for j in 0..16 {
            id += 1;
            constraints.push(row(&kid(id), "distance_x", "origin", &pid(i), x, true));
            if j > 0 {
                dropped.push(id as u32);
            }
        }
        id += 1;
        constraints.push(row(&kid(id), "distance_y", "origin", &pid(i), -1.0, true));
        if i > 0 {
            id += 1;
            constraints.push(format!(
                r#"{{"id": "{}", "type": "horizontal_points", "a": "{}", "b": "{}"}}"#,
                kid(id),
                pid(i - 1),
                pid(i)
            ));
        }
    }
    let t = translate(&text(&points, &[], &constraints));
    let solved = answer(&t, t.system.values.clone(), &dropped);
    t.check_reply(&solved).unwrap();
    let start = Instant::now();
    let s = t.read(&solved).unwrap();
    let took = start.elapsed();
    assert_eq!(s.redundant.len(), dropped.len());
    assert!(took < Duration::from_secs(10), "{took:?}");
}
