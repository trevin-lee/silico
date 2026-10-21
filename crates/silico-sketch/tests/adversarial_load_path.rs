//! /break-it on `Translation::solve`'s check by load through the real sketch worker (as
//! first written, only a solve that dropped constraints or was called over-constrained was
//! checked by a load): a save's answer must be its load's (M3.4: every open solves the
//! sketch again from the coordinates its file stores). Each case is solved (the save); a
//! solution is written through the file as a save writes it, read back, gated, and solved
//! again (the load); a refusal saves nothing, so its load solves the drawing again. The two
//! answers must agree whole: Ok on both with the same bits, degrees of freedom, free
//! geometry, and redundant constraints, or the same refusal. Shapes beyond the earlier
//! break-it's rectangles, slots, and triangles: circles (whose radii a save writes),
//! tangencies, a point held where two curves touch (a rank that drops only at the
//! solution), and under-constrained sketches with a constraint given twice.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/saves.rs"]
mod saves;

use proptest::prelude::*;
use saves::{arc, checked, circle, k, line, nm, point, said, solved, text, worker, written_back};
use silico_core::ErrorCode;
use silico_core::document::{Checked, write};
use silico_core::sketch::{SketchSolution, SketchSolver, SketchWorker};

/// Constraints of these forms, numbered k0000001 on.
fn numbered(forms: &[String]) -> Vec<String> {
    forms.iter().enumerate().map(|(i, f)| k(i + 1, f)).collect()
}

/// What a solve says, whole: Ok with where it put the sketch ([`said`]) and the redundant
/// constraints it names, with their gaps; or the refusal's code, message, and elements.
fn says(r: &Result<SketchSolution, silico_core::Error>) -> Result<String, String> {
    match r {
        Ok(s) => Ok(format!("{} redundant {:?}", said(s), s.redundant)),
        Err(e) => Err(format!(
            "{:?}: {} {:?}",
            e.code(),
            e.message(),
            e.elements()
        )),
    }
}

/// The save's answer, and its load's.
fn save_and_load(
    doc: &Checked,
    w: &mut dyn SketchSolver,
) -> (
    Result<SketchSolution, silico_core::Error>,
    Result<SketchSolution, silico_core::Error>,
) {
    let save = solved(doc, w);
    let load = match &save {
        Ok(s) => solved(&written_back(doc, s), w),
        Err(_) => solved(doc, w),
    };
    (save, load)
}

/// How a save and its load went, where they agree.
#[derive(Debug, PartialEq)]
enum Agreed {
    /// Both Ok, alike.
    Saved,
    /// Both refused alike, as this says.
    Refused(String),
}

/// That the save and its load agree, and how they went; or what they said.
fn agree(doc: &Checked, w: &mut dyn SketchSolver, what: &str) -> Result<Agreed, String> {
    let (save, load) = save_and_load(doc, w);
    let (a, b) = (says(&save), says(&load));
    if a != b {
        return Err(format!("{what}\n  saved  {a:?}\n  loaded {b:?}"));
    }
    Ok(match a {
        Ok(_) => Agreed::Saved,
        Err(refusal) => Agreed::Refused(refusal),
    })
}

/// That a consistent sketch's save is Ok and its load agrees; or what went wrong.
fn kept(doc: &Checked, w: &mut dyn SketchSolver, what: &str) -> Result<(), String> {
    match agree(doc, w, what)? {
        Agreed::Saved => Ok(()),
        Agreed::Refused(refusal) => Err(format!("{what}: consistent, but refused: {refusal}")),
    }
}

// --- Shapes ---------------------------------------------------------------------------------

/// A circle of radius r about p1, held at the origin; a horizontal line n1 from p2 to p3,
/// 2L long, its start held at (-L, r): tangent to the circle at (0, r). p4 is held on both
/// the circle and the line, drawn at (r · dx, r · (1 + dy)): its only solution is (0, r),
/// where the two constraints' gradients are parallel, so the Jacobian's rank drops there and
/// only there. `twice` gives the line's horizontal twice, so the drawing's diagnosis drops
/// one and the solve again runs.
fn touching(r: f64, l: f64, dx: f64, dy: f64, twice: bool) -> String {
    let points = [
        point("p0000001", 0.0, 0.0),
        point("p0000002", -l, r),
        point("p0000003", l, r),
        point("p0000004", nm(r * dx), nm(r * (1.0 + dy))),
    ];
    let lines = [line("n0000001", "p0000002", "p0000003")];
    let circles = [circle("c0000001", "p0000001", r)];
    let mut forms = vec![
        r#""type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true"#
            .to_owned(),
        r#""type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true"#
            .to_owned(),
        format!(r#""type": "radius", "curve": "c0000001", "value": {r}, "driving": true"#),
        format!(
            r#""type": "distance_x", "a": "origin", "b": "p0000002", "value": {}, "driving": true"#,
            -l
        ),
        format!(
            r#""type": "distance_y", "a": "origin", "b": "p0000002", "value": {r}, "driving": true"#
        ),
        r#""type": "horizontal", "line": "n0000001""#.to_owned(),
        format!(
            r#""type": "length", "line": "n0000001", "value": {}, "driving": true"#,
            2.0 * l
        ),
        r#""type": "on", "point": "p0000004", "curve": "c0000001""#.to_owned(),
        r#""type": "on", "point": "p0000004", "curve": "n0000001""#.to_owned(),
    ];
    if twice {
        forms.push(r#""type": "horizontal", "line": "n0000001""#.to_owned());
    }
    text(&points, &lines, &circles, &[], &numbered(&forms))
}

/// Two circles: c1 about p1 at the origin, radius a, held; c2 about p2, radius b, tangent
/// to c1 outside, p2 on the x axis, and also held at a + b from p1 (which the tangency
/// already gives): redundant. Drawn off by `jitter` (fractions of b).
fn kissing(a: f64, b: f64, jitter: [f64; 3]) -> String {
    let points = [
        point("p0000001", 0.0, 0.0),
        point("p0000002", nm(a + b + jitter[0] * b), nm(jitter[1] * b)),
    ];
    let circles = [
        circle("c0000001", "p0000001", a),
        circle("c0000002", "p0000002", nm(b * (1.0 + jitter[2]))),
    ];
    let forms = [
        r#""type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true"#
            .to_owned(),
        r#""type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true"#
            .to_owned(),
        format!(r#""type": "radius", "curve": "c0000001", "value": {a}, "driving": true"#),
        format!(
            r#""type": "diameter", "curve": "c0000002", "value": {}, "driving": true"#,
            nm(2.0 * b)
        ),
        r#""type": "on", "point": "p0000002", "curve": "x_axis""#.to_owned(),
        r#""type": "tangent", "a": "c0000001", "b": "c0000002", "side": "outside""#.to_owned(),
        format!(
            r#""type": "distance", "a": "p0000001", "b": "p0000002", "value": {}, "driving": true"#,
            nm(a + b)
        ),
        format!(r#""type": "radius", "curve": "c0000002", "value": {b}, "driving": true"#),
    ];
    text(&points, &[], &circles, &[], &numbered(&forms))
}

/// A line from p1 to p2, horizontal twice and nothing else: under-constrained (three
/// degrees of freedom), drawn at a slope.
fn loose(x: f64, y: f64, len: f64, slope: f64) -> String {
    let points = [
        point("p0000001", x, y),
        point("p0000002", nm(x + len), nm(y + slope * len)),
    ];
    let lines = [line("n0000001", "p0000001", "p0000002")];
    let forms = [
        r#""type": "horizontal", "line": "n0000001""#.to_owned(),
        r#""type": "horizontal", "line": "n0000001""#.to_owned(),
    ];
    text(&points, &lines, &[], &[], &numbered(&forms))
}

/// A quarter arc about p1, held at (x0, y0), radius r, from p2 (level with p1, to its
/// right) to p3 (above p1): fully constrained, nothing repeated, unless `twice` gives the
/// radius twice. Drawn off by `jitter` (fractions of r).
fn quarter(x0: f64, y0: f64, r: f64, jitter: [f64; 4], twice: bool) -> String {
    let j = |k: usize| jitter[k] * r;
    let points = [
        point("p0000001", x0, y0),
        point("p0000002", nm(x0 + r + j(0)), nm(y0 + j(1))),
        point("p0000003", nm(x0 + j(2)), nm(y0 + r + j(3))),
    ];
    let arcs = [arc("a0000001", "p0000001", "p0000002", "p0000003")];
    let mut forms = vec![
        format!(
            r#""type": "distance_x", "a": "origin", "b": "p0000001", "value": {x0}, "driving": true"#
        ),
        format!(
            r#""type": "distance_y", "a": "origin", "b": "p0000001", "value": {y0}, "driving": true"#
        ),
        format!(r#""type": "radius", "curve": "a0000001", "value": {r}, "driving": true"#),
        r#""type": "horizontal_points", "a": "p0000001", "b": "p0000002""#.to_owned(),
        r#""type": "vertical_points", "a": "p0000001", "b": "p0000003""#.to_owned(),
    ];
    if twice {
        forms.push(format!(
            r#""type": "radius", "curve": "a0000001", "value": {r}, "driving": true"#
        ));
    }
    text(&points, &[], &[], &arcs, &numbered(&forms))
}

/// An arc about p1, held at (x0, y0), radius r, from p2 to p3 (above p1); p2 held at
/// `deg` degrees from the x axis by a construction-free line n1 from p1 to p2: its
/// coordinates irrational, so a load's arc radius and angles, worked out from the stored
/// points, aren't the solve's own. Nothing repeated unless `twice` gives the radius twice.
fn slanted(x0: f64, y0: f64, r: f64, deg: f64, jitter: [f64; 4], twice: bool) -> String {
    let j = |k: usize| jitter[k] * r;
    let (c, s) = (libm::cos(deg.to_radians()), libm::sin(deg.to_radians()));
    let points = [
        point("p0000001", x0, y0),
        point("p0000002", nm(x0 + r * c + j(0)), nm(y0 + r * s + j(1))),
        point("p0000003", nm(x0 - r * 0.3 + j(2)), nm(y0 + r + j(3))),
    ];
    let lines = [line("n0000001", "p0000001", "p0000002")];
    let arcs = [arc("a0000001", "p0000001", "p0000002", "p0000003")];
    let mut forms = vec![
        format!(
            r#""type": "distance_x", "a": "origin", "b": "p0000001", "value": {x0}, "driving": true"#
        ),
        format!(
            r#""type": "distance_y", "a": "origin", "b": "p0000001", "value": {y0}, "driving": true"#
        ),
        format!(r#""type": "radius", "curve": "a0000001", "value": {r}, "driving": true"#),
        format!(
            r#""type": "angle", "a": "x_axis", "b": "n0000001", "value": {deg}, "driving": true"#
        ),
        r#""type": "vertical_points", "a": "p0000001", "b": "p0000003""#.to_owned(),
    ];
    if twice {
        forms.push(format!(
            r#""type": "radius", "curve": "a0000001", "value": {r}, "driving": true"#
        ));
    }
    text(&points, &lines, &[], &arcs, &numbered(&forms))
}

// --- Targeted ---------------------------------------------------------------------------------

#[test]
fn a_point_on_a_circle_and_on_its_tangent_line_loads_as_it_saved() {
    let mut w = worker();
    let mut broke = Vec::new();
    for (dx, dy) in [
        (0.2, 0.2),
        (0.5, 0.1),
        (-0.3, 0.4),
        (1e-3, 1e-3),
        (0.0, 0.3),
        (0.9, -0.5),
    ] {
        for twice in [false, true] {
            let doc = checked(&touching(5.0, 10.0, dx, dy, twice)).unwrap();
            if let Err(m) = kept(
                &doc,
                &mut w,
                &format!("touching dx {dx} dy {dy} twice {twice}"),
            ) {
                broke.push(m);
            }
        }
    }
    assert!(broke.is_empty(), "{}", broke.join("\n"));
}

#[test]
fn a_redundant_circle_tangency_loads_as_it_saved() {
    let mut w = worker();
    let mut broke = Vec::new();
    for jitter in [
        [0.1, 0.1, 0.1],
        [-0.2, 0.3, -0.1],
        [0.0, 0.0, 0.0],
        [0.4, -0.4, 0.2],
    ] {
        let doc = checked(&kissing(10.0, 3.0, jitter)).unwrap();
        if let Err(m) = kept(&doc, &mut w, &format!("kissing jitter {jitter:?}")) {
            broke.push(m);
        }
    }
    assert!(broke.is_empty(), "{}", broke.join("\n"));
}

#[test]
fn an_under_constrained_line_horizontal_twice_loads_as_it_saved() {
    let mut w = worker();
    let mut broke = Vec::new();
    for (x, y, len, slope) in [
        (0.0, 0.0, 10.0, 0.1),
        (3.0, -7.0, 0.001, 0.3),
        (999_000.0, -999_000.0, 400.0, 0.2),
        (-12.5, 4.25, 1000.0, -0.7),
    ] {
        let doc = checked(&loose(x, y, len, slope)).unwrap();
        if let Err(m) = kept(
            &doc,
            &mut w,
            &format!("loose ({x}, {y}) len {len} slope {slope}"),
        ) {
            broke.push(m);
        }
    }
    assert!(broke.is_empty(), "{}", broke.join("\n"));
}

#[test]
fn the_same_sketch_saved_twice_gives_the_same_bytes() {
    // Determinism across workers: two fresh workers, the same drawing, the same file.
    let doc = checked(&kissing(10.0, 3.0, [0.2, 0.1, -0.1])).unwrap();
    let file = |w: &mut SketchWorker| {
        let s = solved(&doc, w).unwrap();
        write(written_back(&doc, &s).document()).unwrap()
    };
    assert_eq!(file(&mut worker()), file(&mut worker()));
}

#[test]
fn a_45_degree_arc_of_radius_100_saved_clean_loads_where_it_saved() {
    // Found by break-it on that first check by load, fixed: an arc of radius 100 about the
    // origin, its start held at 45 degrees by a line from the centre, its end above the
    // centre, drawn a little off. Nothing is repeated, so the first solve (dofs 0, no
    // redundant constraint) stood without the load-path loop, and the save wrote p2 at
    // (70.71067811847684, 70.71067811899326), 5.2e-10 mm apart (the angle met to 5e-12 rad,
    // under PlaneGCS's 1e-10). A load works the arc's radius and angles out of those
    // points, and its solve moved p2 to (70.71067811865476, 70.71067811865474): `silico
    // fmt` on the drawing, then `silico check`, exited 1 with drift. Now every solve that
    // reaches a solution is checked by loads, and the answer kept is one its file gives
    // back.
    let doc = checked(&slanted(0.0, 0.0, 100.0, 45.0, [0.0, 0.1, 0.1, 0.1], false)).unwrap();
    let (save, load) = save_and_load(&doc, &mut worker());
    let save = save.unwrap();
    assert!(save.redundant.is_empty() && save.dofs == 0, "{save:?}");
    assert_eq!(says(&Ok(save)), says(&load));
}

#[test]
fn an_implied_perpendicular_drawn_off_its_solution_saves_what_its_load_says() {
    // Found by the council (port) on that first check by load, fixed: n1 horizontal, drawn
    // (0, 0) to (10, 3), n2 vertical, drawn (12, 1) to (14, 9), and the two perpendicular,
    // which the first two imply. Diagnosed on the drawing, where the lines aren't yet level
    // and plumb, PlaneGCS finds no redundancy and 5 degrees of freedom; every load,
    // diagnosed on the solution, finds k0000003 redundant and 6 (FreeCAD 1.1.3's PlaneGCS
    // does the same; fc82d71c15 documents the mechanism). The save said the drawing's; now
    // it says the load's.
    let doc = checked(&text(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 3.0),
            point("p0000003", 12.0, 1.0),
            point("p0000004", 14.0, 9.0),
        ],
        &[
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000003", "p0000004"),
        ],
        &[],
        &[],
        &numbered(&[
            r#""type": "horizontal", "line": "n0000001""#.to_owned(),
            r#""type": "vertical", "line": "n0000002""#.to_owned(),
            r#""type": "perpendicular", "a": "n0000001", "b": "n0000002""#.to_owned(),
        ]),
    ))
    .unwrap();
    let (save, load) = save_and_load(&doc, &mut worker());
    let s = save.as_ref().unwrap();
    assert_eq!(s.dofs, 6, "{s:?}");
    assert_eq!(
        s.redundant.keys().map(|id| id.as_str()).collect::<Vec<_>>(),
        ["k0000003"]
    );
    assert_eq!(says(&save), says(&load));
}

#[test]
fn a_slanted_arc_with_a_repeated_radius_loads_as_it_saved() {
    // Survives: the same arc with its radius given twice is solved again as a load solves
    // it, and the load that settles stands.
    let doc = checked(&slanted(0.0, 0.0, 100.0, 45.0, [0.0, 0.1, 0.1, 0.1], true)).unwrap();
    let mut w = worker();
    let (save, load) = save_and_load(&doc, &mut w);
    assert!(save.is_ok());
    assert_eq!(says(&save), says(&load));
}

#[test]
fn quarter_arcs_far_out_load_as_they_saved() {
    // Survives: an arc whose ends are level with and above its centre solves to exact
    // coordinates, so a load's arc values are the solver's; 4,000 such far out and near
    // the origin, with and without a repeated radius, all loaded as they saved.
    let mut w = worker();
    let mut broke = Vec::new();
    for (x0, y0) in [
        (0.0, 0.0),
        (998_000.0, -998_000.0),
        (-123_456.789, 654_321.5),
    ] {
        for r in [0.001, 1.5, 997.25] {
            for twice in [false, true] {
                let doc = checked(&quarter(x0, y0, r, [0.1, -0.1, -0.15, 0.05], twice)).unwrap();
                if let Err(m) = kept(
                    &doc,
                    &mut w,
                    &format!("quarter ({x0}, {y0}) r {r} twice {twice}"),
                ) {
                    broke.push(m);
                }
            }
        }
    }
    assert!(broke.is_empty(), "{}", broke.join("\n"));
}

// --- Properties -------------------------------------------------------------------------------

thread_local!(static W: std::cell::RefCell<SketchWorker> = std::cell::RefCell::new(worker()));

fn report(r: Result<(), String>) -> Result<(), TestCaseError> {
    r.map_err(TestCaseError::fail)
}

/// A jitter biased toward the edges: zero, tiny, and the range's ends.
fn jitter(range: f64) -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(range),
        Just(-range),
        -1e-6f64..1e-6,
        -range..range,
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, .. ProptestConfig::default() })]

    /// A point held on a circle and on the line tangent to it: whatever the scale and
    /// wherever it's drawn, the save's answer is its load's.
    #[test]
    fn a_point_where_a_line_touches_a_circle_loads_as_it_saved(
        log_r in -3.0f64..5.0,
        dx in jitter(0.9),
        dy in jitter(0.9),
        twice in any::<bool>(),
    ) {
        let r = nm(libm::pow(10.0, log_r));
        prop_assume!(r > 0.0 && 2.0 * r < 9e5);
        let Ok(doc) = checked(&touching(r, nm(2.0 * r), dx, dy, twice)) else {
            return Ok(());
        };
        let what = format!("touching r {r} dx {dx} dy {dy} twice {twice}");
        report(W.with(|w| kept(&doc, &mut *w.borrow_mut(), &what)))?;
    }

    // Found by break-it on that first check by load, fixed: about 1 in 250 of these arcs,
    // nothing repeated, at ordinary places and sizes (a sweep of 10,000 near the origin:
    // 38), saved Ok and loaded moved by ulps
    // (a_45_degree_arc_of_radius_100_saved_clean_loads_where_it_saved). Now none does, and
    // every one is kept: a refusal fails the property (of 20,000, none).
    /// An arc whose start is held at an angle, so its points are irrational and a load's
    /// arc values, worked out from them, aren't the solver's.
    #[test]
    fn a_slanted_arc_loads_as_it_saved(
        log_r in -2.0f64..4.0,
        deg in 1.0f64..89.0,
        place in prop_oneof![Just(0.0), -1e4f64..1e4],
        j0 in jitter(0.15),
        j1 in jitter(0.15),
        j2 in jitter(0.15),
        j3 in jitter(0.15),
        twice in any::<bool>(),
    ) {
        let r = nm(libm::pow(10.0, log_r));
        let (x0, y0, deg) = (nm(place), nm(-place), nm(deg));
        prop_assume!(r > 0.0);
        let Ok(doc) = checked(&slanted(x0, y0, r, deg, [j0, j1, j2, j3], twice)) else {
            return Ok(());
        };
        let what = format!("slanted ({x0}, {y0}) r {r} deg {deg} jitter {:?} twice {twice}", [j0, j1, j2, j3]);
        report(W.with(|w| kept(&doc, &mut *w.borrow_mut(), &what)))?;
    }

    /// An under-constrained line horizontal twice.
    #[test]
    fn a_loose_line_horizontal_twice_loads_as_it_saved(
        x in prop_oneof![Just(0.0), Just(999_000.0), -999_000.0f64..999_000.0],
        y in prop_oneof![Just(0.0), Just(-999_000.0), -999_000.0f64..999_000.0],
        log_len in -3.0f64..5.0,
        slope in jitter(2.0),
    ) {
        let len = nm(libm::pow(10.0, log_len));
        let (x, y) = (nm(x.clamp(-9e5, 9e5)), nm(y.clamp(-9e5, 9e5)));
        let Ok(doc) = checked(&loose(x, y, len, slope)) else {
            return Ok(());
        };
        let what = format!("loose ({x}, {y}) len {len} slope {slope}");
        report(W.with(|w| kept(&doc, &mut *w.borrow_mut(), &what)))?;
    }
}

#[test]
fn a_refused_sketch_is_refused_with_its_code_on_every_solve() {
    // Survives as a sanity check of the harness: a sketch whose constraints conflict
    // outright (a line 10 long and 20 long) is refused over_constrained, naming both, on
    // save and on load.
    let points = [point("p0000001", 0.0, 0.0), point("p0000002", 10.0, 0.0)];
    let lines = [line("n0000001", "p0000001", "p0000002")];
    let forms = [
        r#""type": "length", "line": "n0000001", "value": 10, "driving": true"#.to_owned(),
        r#""type": "length", "line": "n0000001", "value": 20, "driving": true"#.to_owned(),
    ];
    let doc = checked(&text(&points, &lines, &[], &[], &numbered(&forms))).unwrap();
    let mut w = worker();
    let (save, load) = save_and_load(&doc, &mut w);
    let e = save.as_ref().unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained);
    assert_eq!(says(&save), says(&load));
}

#[test]
fn redundant_kissing_circles_load_as_they_saved() {
    // Two circles tangent, their centres' distance given too, the second's size given as a
    // radius and a diameter: consistent, but PlaneGCS, diagnosing on the drawing, calls about
    // 1 in 30 drawn this far off conflicting (9 of 300 in a sweep; a spec question, as for
    // the redundant shapes in adversarial_redundant_resolve.rs, not a break). So a refusal
    // must be that diagnosis, the save's and its load's alike, and at least nine in ten of
    // the cases saved: a solve that refused every case alike would agree with its load and
    // pass on agreement alone.
    let strategy = (
        -3.0f64..5.0,
        -3.0f64..5.0,
        jitter(0.4),
        jitter(0.4),
        jitter(0.4),
    );
    let (cases, saved) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 1000,
        ..ProptestConfig::default()
    });
    let run = runner.run(&strategy, |(log_a, log_b, j0, j1, j2)| {
        let (a, b) = (nm(libm::pow(10.0, log_a)), nm(libm::pow(10.0, log_b)));
        prop_assume!(a > 0.0 && b > 0.0 && a + 3.0 * b < 9e5);
        let Ok(doc) = checked(&kissing(a, b, [j0, j1, j2])) else {
            return Ok(());
        };
        let what = format!("kissing a {a} b {b} jitter {:?}", [j0, j1, j2]);
        cases.set(cases.get() + 1);
        match W.with(|w| agree(&doc, &mut *w.borrow_mut(), &what)) {
            Ok(Agreed::Saved) => saved.set(saved.get() + 1),
            Ok(Agreed::Refused(refusal)) => prop_assert!(
                refusal.starts_with("OverConstrained: the sketch's constraints conflict"),
                "{what}: refused otherwise: {refusal}"
            ),
            Err(m) => return Err(TestCaseError::fail(m)),
        }
        Ok(())
    });
    if let Err(e) = run {
        panic!("{e}");
    }
    let (cases, saved) = (cases.get(), saved.get());
    assert!(
        cases >= 500 && saved * 10 >= cases * 9,
        "{saved} of {cases} saved"
    );
}
