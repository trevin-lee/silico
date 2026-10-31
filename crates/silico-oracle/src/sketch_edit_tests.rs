//! The harness's own rules, on sketches and outcomes built by hand: each expected value is
//! worked out from the geometry, not from the code under test.

use super::*;

/// A member by hand: its ID, its kind, and its points by position.
type HandMember<'a> = (&'a str, &'static str, &'a [(i32, &'a str)]);

/// A shape of `points` (by ID), `members` (each its kind and its points by position), and
/// `radii`.
fn shape(points: &[(&str, [f64; 2])], members: &[HandMember<'_>], radii: &[(&str, f64)]) -> Shape {
    Shape {
        members: members
            .iter()
            .map(|(id, kind, at)| {
                (
                    (*id).to_owned(),
                    Member {
                        kind,
                        at: at.iter().map(|(pos, q)| (*pos, (*q).to_owned())).collect(),
                        construction: false,
                    },
                )
            })
            .collect(),
        points: points
            .iter()
            .map(|(id, at)| ((*id).to_owned(), *at))
            .collect(),
        radii: radii.iter().map(|(id, r)| ((*id).to_owned(), *r)).collect(),
    }
}

/// The line n1 from a = (1, 2) to b = (4, 6): length 5, along u = (0.6, 0.8), its normal
/// n = (-0.8, 0.6).
fn slanted_line() -> Shape {
    shape(
        &[("p1", [1.0, 2.0]), ("p2", [4.0, 6.0])],
        &[("n1", "line", &[(1, "p1"), (2, "p2")])],
        &[],
    )
}

/// `a` plus `along` of u and `off` of n, for [`slanted_line`].
fn on_slant(a: [f64; 2], along: f64, off: f64) -> [f64; 2] {
    [
        a[0] + 0.6 * along - 0.8 * off,
        a[1] + 0.8 * along + 0.6 * off,
    ]
}

/// The arc a1 about c = (1, 1), radius 2, from 30 degrees (p1) to 120 degrees (p2), its
/// centre p3.
fn arc_30_120() -> (Shape, [f64; 2], f64, f64) {
    let c = [1.0, 1.0];
    let at = |deg: f64| {
        let r = deg.to_radians();
        [c[0] + 2.0 * libm::cos(r), c[1] + 2.0 * libm::sin(r)]
    };
    let (s, e) = (at(30.0), at(120.0));
    let shape = shape(
        &[("p1", s), ("p2", e), ("p3", c)],
        &[("a1", "arc", &[(1, "p1"), (2, "p2"), (3, "p3")])],
        &[("a1", 2.0)],
    );
    // The angles as the arc's own ends give them.
    let from = libm::atan2(s[1] - c[1], s[0] - c[0]);
    let to = libm::atan2(e[1] - c[1], e[0] - c[0]);
    (shape, c, from, to)
}

/// The point at `angle` (radians) on the circle about `c` of radius 2.
fn on_round(c: [f64; 2], angle: f64) -> [f64; 2] {
    [c[0] + 2.0 * libm::cos(angle), c[1] + 2.0 * libm::sin(angle)]
}

#[test]
fn a_position_is_near_a_lines_end_within_1e_7_along_it_or_past_it() {
    let s = slanted_line();
    let (a, b) = ([1.0, 2.0], [4.0, 6.0]);
    // Along the line from the start, whatever the offset across it.
    assert!(near_end(&s, "n1", "p1", on_slant(a, 0.5e-7, 3.0)));
    assert!(!near_end(&s, "n1", "p1", on_slant(a, 2e-7, 3.0)));
    assert!(near_end(&s, "n1", "p1", on_slant(a, -1.0, -2.0)));
    assert!(!near_end(&s, "n1", "p1", on_slant(a, 2.5, 0.0)));
    // From the end, back along it.
    assert!(near_end(&s, "n1", "p2", on_slant(b, -0.5e-7, -2.0)));
    assert!(!near_end(&s, "n1", "p2", on_slant(b, -2e-7, -2.0)));
    assert!(near_end(&s, "n1", "p2", on_slant(b, 1.0, 1.0)));
    assert!(!near_end(&s, "n1", "p2", on_slant(b, -2.5, 0.0)));
    // Each end is only its own: the start isn't near the end, nor the end the start.
    assert!(!near_end(&s, "n1", "p2", a));
    assert!(!near_end(&s, "n1", "p1", b));
    // A point that isn't an end of the curve, or a curve that isn't there.
    assert!(!near_end(&s, "n1", "p9", a));
    assert!(!near_end(&s, "n9", "p1", a));
}

#[test]
fn a_position_is_near_an_arcs_end_within_1e_7_along_it_or_past_its_sweep() {
    let (s, c, from, to) = arc_30_120();
    // 2.5e-8 radians is 5e-8 mm on a radius of 2, 1e-7 radians 2e-7 mm.
    assert!(near_end(&s, "a1", "p1", on_round(c, from + 2.5e-8)));
    assert!(!near_end(&s, "a1", "p1", on_round(c, from + 1e-7)));
    assert!(near_end(&s, "a1", "p2", on_round(c, to - 2.5e-8)));
    assert!(!near_end(&s, "a1", "p2", on_round(c, to - 1e-7)));
    // Mid-sweep is near neither end.
    let mid = on_round(c, (from + to) / 2.0);
    assert!(!near_end(&s, "a1", "p1", mid));
    assert!(!near_end(&s, "a1", "p2", mid));
    // Near the start isn't near the end, nor near the end the start.
    assert!(!near_end(&s, "a1", "p2", on_round(c, from + 2.5e-8)));
    assert!(!near_end(&s, "a1", "p1", on_round(c, to - 2.5e-8)));
    // Each end itself, exactly: near its own end and not the other.
    let (start, end) = (s.points["p1"], s.points["p2"]);
    assert!(near_end(&s, "a1", "p1", start));
    assert!(!near_end(&s, "a1", "p2", start));
    assert!(near_end(&s, "a1", "p2", end));
    assert!(!near_end(&s, "a1", "p1", end));
    // Past the sweep, after its end (180 degrees) or before its start (0 degrees, and -90,
    // whose atan2 is below the start's): past an end, either end.
    for past in [std::f64::consts::PI, 0.0, -std::f64::consts::FRAC_PI_2] {
        let q = on_round(c, past);
        assert!(near_end(&s, "a1", "p1", q), "{past}");
        assert!(near_end(&s, "a1", "p2", q), "{past}");
    }
    // An arc without its centre, or a circle, has no ends here.
    let mut no_centre = arc_30_120().0;
    no_centre.points.remove("p3");
    assert!(!near_end(&no_centre, "a1", "p1", start));
    let circle = shape(
        &[("p1", [3.0, 1.0]), ("p3", [1.0, 1.0])],
        &[("c1", "circle", &[(1, "p1"), (3, "p3")])],
        &[("c1", 2.0)],
    );
    assert!(!near_end(&circle, "c1", "p1", [3.0, 1.0]));
}

#[test]
fn a_positions_distance_off_a_curve_and_its_projection_onto_it_are_the_geometrys() {
    let s = slanted_line();
    let a = [1.0, 2.0];
    // 3 across the line extended, wherever along it.
    for along in [2.0, -4.0, 9.0] {
        let q = on_slant(a, along, 3.0);
        let off = off_curve(&s, "n1", q).unwrap();
        assert!((off - 3.0).abs() <= 1e-12, "{along}: {off}");
        let foot = projection(&s, "n1", q).unwrap();
        let want = on_slant(a, along, 0.0);
        assert!(near(foot, want), "{along}: {foot:?} {want:?}");
    }
    // A circle about (1, 1), radius 2: (4, 5) is 5 from the centre along (0.6, 0.8), so 3
    // off it, its foot (2.2, 2.6); (1.6, 1.8), 1 from the centre, is 1 off, its foot the
    // same direction's.
    let round = shape(
        &[("p3", [1.0, 1.0])],
        &[("c1", "circle", &[(3, "p3")])],
        &[("c1", 2.0)],
    );
    for (q, off, foot) in [([4.0, 5.0], 3.0, [2.2, 2.6]), ([1.6, 1.8], 1.0, [2.2, 2.6])] {
        let got = off_curve(&round, "c1", q).unwrap();
        assert!((got - off).abs() <= 1e-12, "{q:?}: {got}");
        let at = projection(&round, "c1", q).unwrap();
        assert!(near(at, foot), "{q:?}: {at:?}");
    }
    // The centre itself projects nowhere; a point member isn't a curve.
    assert_eq!(projection(&round, "c1", [1.0, 1.0]), None);
    let lone = shape(&[("p1", [0.0, 0.0])], &[("p1", "point", &[(1, "p1")])], &[]);
    assert_eq!(off_curve(&lone, "p1", [1.0, 1.0]), None);
    assert_eq!(projection(&lone, "p1", [1.0, 1.0]), None);
}

/// FreeCAD geometry of `kind`, its points by position.
fn fc_geometry(kind: &str, points: &[(&str, [f64; 2])], radius: Option<f64>) -> FcGeometry {
    FcGeometry {
        id: 1,
        kind: kind.to_owned(),
        points: points.iter().map(|(k, q)| ((*k).to_owned(), *q)).collect(),
        radius,
        construction: false,
    }
}

#[test]
fn a_curves_length_is_its_lines_or_its_arcs() {
    let line = fc_geometry("line", &[("1", [1.0, 2.0]), ("2", [4.0, 6.0])], None);
    assert_eq!(curve_length(&line), Some(5.0));
    // From 30 to 120 degrees on a radius of 2: a quarter turn, pi long.
    let (s, ..) = arc_30_120();
    let arc = fc_geometry(
        "arc",
        &[
            ("1", s.points["p1"]),
            ("2", s.points["p2"]),
            ("3", s.points["p3"]),
        ],
        Some(2.0),
    );
    let length = curve_length(&arc).unwrap();
    assert!((length - std::f64::consts::PI).abs() <= 1e-12, "{length}");
    let circle = fc_geometry("circle", &[("3", [1.0, 1.0])], Some(2.0));
    assert_eq!(curve_length(&circle), None);
}

#[test]
fn only_the_kinds_whose_references_are_a_set_are_unordered() {
    for kind in [
        "Coincident",
        "Equal",
        "Parallel",
        "Perpendicular",
        "Tangent",
        "Distance",
        "Horizontal",
        "Vertical",
    ] {
        assert!(unordered(kind), "{kind}");
    }
    for kind in [
        "DistanceX",
        "DistanceY",
        "Symmetric",
        "PointOnObject",
        "Angle",
        "",
    ] {
        assert!(!unordered(kind), "{kind}");
    }
}

#[test]
fn a_freecad_place_is_named_as_silico_names_it() {
    // n1 a line from p1 to p2; p5 a lone point; p6 a point that's no member (a merged one's).
    let s = shape(
        &[
            ("p1", [0.0, 0.0]),
            ("p2", [1.0, 0.0]),
            ("p5", [2.0, 2.0]),
            ("p6", [3.0, 3.0]),
        ],
        &[
            ("n1", "line", &[(1, "p1"), (2, "p2")]),
            ("p5", "point", &[(1, "p5")]),
        ],
        &[],
    );
    let member = |g: i32| match g {
        0 => Some("n1".to_owned()),
        1 => Some("p5".to_owned()),
        2 => Some("p6".to_owned()),
        3 => Some("gone".to_owned()),
        _ => None,
    };
    let renamed = BTreeMap::new();
    let name = |g: i32, pos: i32, end: bool| name_place(&s, &member, &renamed, g, pos, end);
    assert_eq!(name(-1, 1, false), "origin");
    assert_eq!(name(-1, 0, false), "x_axis");
    assert_eq!(name(-2, 0, false), "y_axis");
    assert_eq!(name(-1, 2, false), "FreeCAD's geometry -1");
    assert_eq!(name(0, 0, false), "n1");
    assert_eq!(name(0, 1, false), "p1");
    assert_eq!(name(0, 2, true), "n1@p2");
    assert_eq!(name(1, 1, false), "p5");
    assert_eq!(name(1, 1, true), "p5");
    assert_eq!(name(2, 1, false), "p6");
    assert_eq!(name(2, 1, true), "p6");
    assert_eq!(name(3, 1, false), "gone.1");
    // Through a merge's renaming: p6 became p5.
    let renamed = BTreeMap::from([("p6".to_owned(), "p5".to_owned())]);
    assert_eq!(name_place(&s, &member, &renamed, 2, 1, true), "p5");
}

#[test]
fn a_keys_references_are_sorted_only_as_its_kind_has_them() {
    let names = ["z", "y", "a"];
    let name = |g: i32, _: i32, ends: bool| {
        let n = names[usize::try_from(g).unwrap()];
        if ends { format!("{n}@") } else { n.to_owned() }
    };
    let refs = [[0, 1], [1, 1], [2, 1]];
    let key_of = |kind: &str| key(kind, &refs, true, &name).refs;
    // A set: all sorted.
    assert_eq!(key_of("Coincident"), ["a", "y", "z"]);
    // A symmetric's two points are a set, its line not.
    assert_eq!(key_of("Symmetric"), ["y", "z", "a"]);
    // A sequence, three references or not, isn't sorted.
    assert_eq!(key_of("Block"), ["z", "y", "a"]);
    assert_eq!(key("Symmetric", &refs[..2], true, &name).refs, ["z", "y"]);
    // A tangency's or a perpendicular's references are ends.
    assert_eq!(key_of("Tangent"), ["a@", "y@", "z@"]);
    assert_eq!(key_of("Perpendicular"), ["a@", "y@", "z@"]);
    assert_eq!(key_of("Angle"), ["z", "y", "a"]);
    assert!(!key("Angle", &refs, false, &name).driving);
}

/// A sketch of `points` (ID, x, y), `lines` (ID, from, to), `circles` (ID, centre, radius),
/// `arcs` (ID, centre, start, end), and `constraints` (each as its JSON), through the gate.
fn sketch_of(
    points: &[(&str, f64, f64)],
    lines: &[(&str, &str, &str)],
    circles: &[(&str, &str, f64)],
    arcs: &[(&str, &str, &str, &str)],
    constraints: &[&str],
) -> Sketch {
    let points: Vec<String> = points
        .iter()
        .map(|(id, x, y)| format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#))
        .collect();
    let lines: Vec<String> = lines
        .iter()
        .map(|(id, a, b)| {
            format!(r#"{{"id": "{id}", "from": "{a}", "to": "{b}", "construction": false}}"#)
        })
        .collect();
    let circles: Vec<String> = circles
        .iter()
        .map(|(id, c, r)| {
            format!(r#"{{"id": "{id}", "center": "{c}", "radius": {r}, "construction": false}}"#)
        })
        .collect();
    let arcs: Vec<String> = arcs
        .iter()
        .map(|(id, c, s, e)| {
            format!(r#"{{"id": "{id}", "center": "{c}", "start": "{s}", "end": "{e}", "construction": false}}"#)
        })
        .collect();
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [{}], "circles": [{}], "arcs": [{}], "constraints": [{}]}}]}}"#,
        points.join(", "),
        lines.join(", "),
        circles.join(", "),
        arcs.join(", "),
        constraints.join(", ")
    );
    let document = Checked::new(read(text.as_bytes()).unwrap())
        .unwrap()
        .document()
        .clone();
    let Some(Operation::Sketch(sketch)) = document.features.first().map(|f| &f.operation) else {
        panic!("no sketch");
    };
    sketch.clone()
}

/// `sketch` prepared for `edit` as [`prepare`] prepares it, with Silico's side `silico`.
fn prepared_with(sketch: Sketch, edit: Edit, silico: SilicoSide) -> Prepared {
    let translation = Translation::new(&sketch).unwrap();
    let model = sketch::freecad_sketch(&sketch, &translation).unwrap();
    let plan = plan(&sketch, &model, &edit);
    Prepared {
        name: "by_hand".to_owned(),
        sketch,
        edit,
        model,
        silico,
        plan,
        problems: Vec::new(),
    }
}

/// Silico's refusal of an edit, as [`Refusal::of`] reads it.
fn refusal(message: &str, elements: &[&str], parameter: &str) -> SilicoSide {
    SilicoSide::Refused(Refusal {
        code: "invalid_input".to_owned(),
        message: message.to_owned(),
        elements: elements.iter().map(|e| (*e).to_owned()).collect(),
        parameter: Some(parameter.to_owned()),
        text: message.to_owned(),
    })
}

/// The line n0000003 from p0000001 (0, 0) to p0000002 (1, 0), the line n0000005 from
/// p0000002 to p0000004 (1, 1), and the lone point p0000006 (5, 5).
fn two_lines(constraints: &[&str]) -> Sketch {
    sketch_of(
        &[
            ("p0000001", 0.0, 0.0),
            ("p0000002", 1.0, 0.0),
            ("p0000004", 1.0, 1.0),
            ("p0000006", 5.0, 5.0),
        ],
        &[
            ("n0000003", "p0000001", "p0000002"),
            ("n0000005", "p0000002", "p0000004"),
        ],
        &[],
        &[],
        constraints,
    )
}

#[test]
fn a_splits_refusal_is_a_rule_only_for_what_it_names_and_where_it_is() {
    let split = |at: [f64; 2]| Edit::Split {
        curve: "n0000003".to_owned(),
        at: vec![at],
    };
    let rule_of = |at: [f64; 2], message: &str, elements: &[&str], parameter: &str| {
        rule(&prepared_with(
            two_lines(&[]),
            split(at),
            refusal(message, elements, parameter),
        ))
    };
    // A line's split refused as not a curve, or at one position, is neither rule: those are
    // a point's and a circle's.
    assert_eq!(rule_of([0.5, 0.0], "x", &["n0000003"], "curve"), None);
    assert_eq!(rule_of([0.5, 0.0], "x", &["n0000003"], "at"), None);
    // Off the curve: only as its message says, and only farther than 1e-7 (1e-7 exactly is
    // on it: the line is along x from the origin, so the offset is the position's y).
    let far = "position 0 is farther than 1e-7 mm from it";
    assert_eq!(
        rule_of([0.5, 5.0], "another reason", &["n0000003"], "at.0"),
        None
    );
    assert_eq!(rule_of([0.5, 1e-7], far, &["n0000003"], "at.0"), None);
    assert!(matches!(
        rule_of([0.5, 2e-7], far, &["n0000003"], "at.0"),
        Some(Rule::SplitOffCurve(off)) if (off - 2e-7).abs() <= 1e-22
    ));
    // At an end: only the split curve's end, and only a position there.
    assert_eq!(
        rule_of([1.0, 0.0], "x", &["n0000003", "p0000002"], "at.0"),
        Some(Rule::SplitAtEnd)
    );
    assert_eq!(
        rule_of([0.5, 0.0], "x", &["n0000003", "p0000002"], "at.0"),
        None
    );
    assert_eq!(
        rule_of([1.0, 0.0], "x", &["n0000005", "p0000002"], "at.0"),
        None
    );
}

#[test]
fn a_trims_refusal_is_a_rule_only_for_a_constraint_no_piece_can_take() {
    // The circle c0000008 about p0000007 (0.5, 1), radius 1, tangent to n0000003; the
    // circle c0000013 about p0000014 (5, 5), tangent to n0000005; and p0000009 and p0000010
    // symmetric about n0000005.
    let sketch = || {
        sketch_of(
            &[
                ("p0000001", 0.0, 0.0),
                ("p0000002", 1.0, 0.0),
                ("p0000004", 1.0, 1.0),
                ("p0000007", 0.5, 1.0),
                ("p0000009", 0.0, 0.5),
                ("p0000010", 2.0, 0.5),
                ("p0000014", 5.0, 5.0),
            ],
            &[
                ("n0000003", "p0000001", "p0000002"),
                ("n0000005", "p0000002", "p0000004"),
            ],
            &[("c0000008", "p0000007", 1.0), ("c0000013", "p0000014", 1.0)],
            &[],
            &[
                r#"{"id": "k0000011", "type": "tangent", "a": "n0000003", "b": "c0000008", "side": "left"}"#,
                r#"{"id": "k0000012", "type": "symmetric", "a": "p0000009", "b": "p0000010", "about": "n0000005"}"#,
                r#"{"id": "k0000015", "type": "tangent", "a": "n0000005", "b": "c0000013", "side": "left"}"#,
            ],
        )
    };
    let rule_of = |curve: &str, elements: &[&str], parameter: &str| {
        rule(&prepared_with(
            sketch(),
            Edit::Trim {
                curve: curve.to_owned(),
                pick: [0.25, 0.0],
            },
            refusal("x", elements, parameter),
        ))
    };
    // A line's trim refused as a point's is no rule.
    assert_eq!(rule_of("n0000003", &["n0000003"], "curve"), None);
    // A line keeps a tangency on a piece; a circle's has none to keep it on.
    assert_eq!(rule_of("n0000003", &["k0000011", "n0000003"], "pick"), None);
    assert_eq!(
        rule_of("c0000008", &["k0000011", "c0000008"], "pick"),
        Some(Rule::NoPieceTakes("k0000011".to_owned()))
    );
    // Another circle's tangency isn't the trimmed circle's.
    assert_eq!(rule_of("c0000008", &["k0000015", "c0000008"], "pick"), None);
    // A symmetric about the trimmed line has no piece to be about; one about another line
    // isn't this trim's.
    assert_eq!(
        rule_of("n0000005", &["k0000012", "n0000005"], "pick"),
        Some(Rule::NoPieceTakes("k0000012".to_owned()))
    );
    assert_eq!(rule_of("n0000003", &["k0000012", "n0000003"], "pick"), None);
}

#[test]
fn an_unjoins_refusal_is_the_rule_only_for_a_point_one_curve_uses() {
    let rule_of = |point: &str, curve: &str, parameter: &str| {
        rule(&prepared_with(
            two_lines(&[]),
            Edit::Unjoin {
                point: point.to_owned(),
                curve: curve.to_owned(),
            },
            refusal("x", &[point, curve], parameter),
        ))
    };
    assert_eq!(
        rule_of("p0000001", "n0000003", "point"),
        Some(Rule::UnjoinUnshared)
    );
    assert_eq!(rule_of("p0000001", "n0000003", "curve"), None);
    // p0000002 is both lines': shared, so not the rule.
    assert_eq!(rule_of("p0000002", "n0000003", "point"), None);
}

#[test]
fn a_merges_and_a_removals_refusals_are_rules_only_for_what_they_name() {
    let merge = |points: &[&str]| Edit::Merge {
        points: points.iter().map(|p| (*p).to_owned()).collect(),
    };
    // A point named twice: only the one named again.
    let named = |points: &[&str], elements: &[&str]| {
        rule(&prepared_with(
            two_lines(&[]),
            merge(points),
            refusal("x", elements, "points.1"),
        ))
    };
    assert_eq!(
        named(&["p0000001", "p0000001"], &["p0000001"]),
        Some(Rule::MergeNamedTwice)
    );
    assert_eq!(named(&["p0000001", "p0000006"], &["p0000006"]), None);
    // A removal of a point a curve that stays uses: only that point, and only while a curve
    // keeps it.
    let removal = |members: &[&str], elements: &[&str]| {
        rule(&prepared_with(
            two_lines(&[]),
            Edit::Remove {
                members: members.iter().map(|m| (*m).to_owned()).collect(),
            },
            refusal("x", elements, "members.0"),
        ))
    };
    assert_eq!(
        removal(&["p0000001"], &["p0000001"]),
        Some(Rule::RemovePointInUse)
    );
    assert_eq!(removal(&["p0000001", "n0000003"], &["p0000001"]), None);
    assert_eq!(removal(&["p0000001"], &["n0000003"]), None);
}

#[test]
fn a_merge_gated_for_a_collapsed_line_is_the_rule_only_where_the_placement_collapses_it() {
    // n0000003 from (3, 4) to (5, 4): its ends merged, both at (3, 4), it's 0 long.
    let sketch = || {
        sketch_of(
            &[
                ("p0000001", 3.0, 4.0),
                ("p0000002", 5.0, 4.0),
                ("p0000004", 9.0, 4.0),
            ],
            &[
                ("n0000003", "p0000001", "p0000002"),
                ("n0000005", "p0000002", "p0000004"),
            ],
            &[],
            &[],
            &[],
        )
    };
    let gated = "merging them would leave a sketch the gate refuses";
    let rule_of = |elements: &[&str]| {
        rule(&prepared_with(
            sketch(),
            Edit::Merge {
                points: vec!["p0000001".to_owned(), "p0000002".to_owned()],
            },
            refusal(gated, elements, "points"),
        ))
    };
    assert_eq!(
        rule_of(&["p0000001", "p0000002", "n0000003"]),
        Some(Rule::MergeGated("n0000003".to_owned()))
    );
    // n0000005, from (3, 4) once merged to (9, 4), isn't collapsed.
    assert_eq!(rule_of(&["p0000001", "p0000002", "n0000005"]), None);
    // The request's points not named first.
    assert_eq!(rule_of(&["p0000002", "p0000001", "n0000003"]), None);
}

#[test]
fn a_merge_that_would_make_a_constraint_hold_its_own_point_is_the_rule_only_then() {
    // p0000006 on n0000003; the arc a0000010 about p0000007 from p0000008 to p0000009, held
    // tangent at its start on n0000005.
    let sketch = || {
        sketch_of(
            &[
                ("p0000001", 0.0, 0.0),
                ("p0000002", 1.0, 0.0),
                ("p0000004", 1.0, 1.0),
                ("p0000006", 0.5, 0.0),
                ("p0000007", 3.0, 0.0),
                ("p0000008", 4.0, 0.0),
                ("p0000009", 3.0, 1.0),
            ],
            &[
                ("n0000003", "p0000001", "p0000002"),
                ("n0000005", "p0000002", "p0000004"),
            ],
            &[],
            &[("a0000010", "p0000007", "p0000008", "p0000009")],
            &[
                r#"{"id": "k0000011", "type": "on", "point": "p0000006", "curve": "n0000003"}"#,
                r#"{"id": "k0000012", "type": "tangent_on", "a": "a0000010", "at": "p0000008", "b": "n0000005", "direction": "same"}"#,
            ],
        )
    };
    let made = "merging them would make constraint hold a curve's own point";
    let rule_of = |points: &[&str], elements: &[&str]| {
        rule(&prepared_with(
            sketch(),
            Edit::Merge {
                points: points.iter().map(|p| (*p).to_owned()).collect(),
            },
            refusal(made, elements, "points"),
        ))
    };
    // p0000006 merged into n0000003's start: on its own curve.
    assert_eq!(
        rule_of(&["p0000001", "p0000006"], &["k0000011"]),
        Some(Rule::MergeHoldsOwn("k0000011".to_owned()))
    );
    // Merged with a point of another curve: not its own.
    assert_eq!(rule_of(&["p0000004", "p0000006"], &["k0000011"]), None);
    // The arc's start merged into n0000005's end: tangent on its own curve at its end.
    assert_eq!(
        rule_of(&["p0000004", "p0000008"], &["k0000012"]),
        Some(Rule::MergeHoldsOwn("k0000012".to_owned()))
    );
    assert_eq!(rule_of(&["p0000006", "p0000008"], &["k0000012"]), None);
}

#[test]
fn a_merge_naming_a_constraint_that_names_both_its_points_is_the_rule() {
    // p0000006 and p0000007 symmetric about n0000005, and a distance between them.
    let sketch = sketch_of(
        &[
            ("p0000001", 0.0, 0.0),
            ("p0000002", 1.0, 0.0),
            ("p0000004", 1.0, 1.0),
            ("p0000006", 0.0, 0.5),
            ("p0000007", 2.0, 0.5),
        ],
        &[
            ("n0000003", "p0000001", "p0000002"),
            ("n0000005", "p0000002", "p0000004"),
        ],
        &[],
        &[],
        &[
            r#"{"id": "k0000008", "type": "symmetric", "a": "p0000006", "b": "p0000007", "about": "n0000005"}"#,
            r#"{"id": "k0000009", "type": "distance", "a": "p0000006", "b": "p0000007", "value": 2, "driving": true}"#,
            r#"{"id": "k0000010", "type": "on", "point": "p0000006", "curve": "n0000003"}"#,
        ],
    );
    let rule_of = |k: &str| {
        rule(&prepared_with(
            sketch.clone(),
            Edit::Merge {
                points: vec!["p0000006".to_owned(), "p0000007".to_owned()],
            },
            refusal("x", &[k, "p0000006"], "points"),
        ))
    };
    for k in ["k0000008", "k0000009"] {
        assert_eq!(rule_of(k), Some(Rule::MergeNamesTwice(k.to_owned())), "{k}");
    }
    assert_eq!(rule_of("k0000010"), None);
}

/// A solve's diagnosis: `code`, and the constraints (from 1) redundant, conflicting, and
/// malformed.
fn diagnosis(
    code: i32,
    redundant: &[usize],
    conflicting: &[usize],
    malformed: &[usize],
) -> Diagnosis {
    Diagnosis {
        code,
        dof: 0,
        conflicting: conflicting.to_vec(),
        redundant: redundant.to_vec(),
        partially_redundant: Vec::new(),
        malformed: malformed.to_vec(),
    }
}

/// A FreeCAD geometry by hand: its kind, its points by position, and its radius.
type HandGeometry<'a> = (&'a str, Vec<(&'a str, [f64; 2])>, Option<f64>);

/// FreeCAD's geometry as it was given `geometry`, IDs from 1: each point by position (an
/// arc's ends at its angles).
fn fc_geometry_of(geometry: &[FreecadGeometry]) -> Vec<FcGeometry> {
    geometry
        .iter()
        .zip(1..)
        .map(|(g, id)| {
            let (kind, points, radius): HandGeometry = match g {
                FreecadGeometry::Point { at } => ("point", vec![("1", *at)], None),
                FreecadGeometry::Line { start, end } => {
                    ("line", vec![("1", *start), ("2", *end)], None)
                }
                FreecadGeometry::Circle { center, radius } => {
                    ("circle", vec![("3", *center)], Some(*radius))
                }
                FreecadGeometry::Arc {
                    center,
                    radius,
                    from,
                    to,
                } => (
                    "arc",
                    vec![
                        (
                            "1",
                            [
                                center[0] + radius * libm::cos(*from),
                                center[1] + radius * libm::sin(*from),
                            ],
                        ),
                        (
                            "2",
                            [
                                center[0] + radius * libm::cos(*to),
                                center[1] + radius * libm::sin(*to),
                            ],
                        ),
                        ("3", *center),
                    ],
                    Some(*radius),
                ),
            };
            FcGeometry {
                id,
                ..fc_geometry(kind, &points, radius)
            }
        })
        .collect()
}

/// FreeCAD's merge of `p` rolled back, as its plan gives FreeCAD the sketch: every geometry
/// where it was given (IDs from 1), the constraints it was given and the merge's
/// coincidences, and both solves clean.
fn rolled_back_by_hand(p: &Prepared) -> FreecadEdited {
    let Plan::Run { job, .. } = &p.plan else {
        panic!("{:?}", p.plan);
    };
    let FreecadEdit::Merge { coincidences, .. } = &job.edit else {
        panic!("{:?}", job.edit);
    };
    let geometry = fc_geometry_of(&job.geometry);
    let constraints = job
        .constraints
        .iter()
        .map(|c| FcConstraint {
            kind: c.kind.to_owned(),
            refs: pairs(c.kind, &c.ints),
            value: c.value.unwrap_or(0.0),
            driving: c.driving,
            name: c.name.clone().unwrap_or_default(),
        })
        .chain(coincidences.iter().map(|&[g1, p1, g2, p2]| FcConstraint {
            kind: "Coincident".to_owned(),
            refs: vec![[g1, p1], [g2, p2]],
            value: 0.0,
            driving: true,
            name: String::new(),
        }))
        .collect();
    FreecadEdited {
        before: diagnosis(0, &[], &[], &[]),
        ids_before: geometry.iter().map(|g| g.id).collect(),
        edited: geometry.clone(),
        geometry,
        constraints,
        after: diagnosis(0, &[], &[], &[]),
        applied: Vec::new(),
        placed: None,
    }
}

/// [`two_lines`]' p0000001 and p0000004 merged, Silico's side refused (unused here).
fn merge_of(points: &[&str], constraints: &[&str]) -> Prepared {
    prepared_with(
        two_lines(constraints),
        Edit::Merge {
            points: points.iter().map(|p| (*p).to_owned()).collect(),
        },
        refusal("x", &[], "points"),
    )
}

#[test]
fn points_are_merged_from_apart_only_farther_than_1e_8_wherever_they_are() {
    let merged = |at: [f64; 2], other: [f64; 2]| {
        let sketch = sketch_of(
            &[("p0000001", at[0], at[1]), ("p0000002", other[0], other[1])],
            &[],
            &[],
            &[],
            &[],
        );
        merged_apart(&prepared_with(
            sketch,
            Edit::Merge {
                points: vec!["p0000001".to_owned(), "p0000002".to_owned()],
            },
            refusal("x", &[], "points"),
        ))
    };
    // Two points at one place, away from the origin: not apart.
    assert_eq!(merged([3.0, 4.0], [3.0, 4.0]), None);
    // 1e-8 apart (exactly, from the origin) is together; 2e-8 apart isn't.
    assert_eq!(merged([0.0, 0.0], [1e-8, 0.0]), None);
    assert_eq!(
        merged([3.0, 4.0], [3.0, 5.0]),
        Some(BTreeSet::from(["p0000001".to_owned()]))
    );
}

#[test]
fn freecads_merge_refuses_points_already_joined_and_a_worse_solve_and_nothing_else() {
    // p0000001 (n0000003's start, geometry 0 position 1) and p0000004 (n0000005's end,
    // geometry 1 position 2), apart: FreeCAD's merge is clean.
    let p = merge_of(&["p0000001", "p0000004"], &[]);
    let Plan::Run { job, .. } = &p.plan else {
        panic!("{:?}", p.plan);
    };
    let FreecadEdit::Merge { coincidences, .. } = &job.edit else {
        panic!("{:?}", job.edit);
    };
    assert_eq!(coincidences, &[[0, 1, 1, 2]]);
    let clean = rolled_back_by_hand(&p);
    assert_eq!(merge_refusal(&p, &clean), None);
    // Both ends held on one curve's edge (position 0) aren't joined by that.
    let mut on_an_edge = p.clone();
    let Plan::Run { job, .. } = &mut on_an_edge.plan else {
        unreachable!()
    };
    for ints in [vec![0, 1, 1, 0], vec![1, 2, 1, 0]] {
        job.constraints.push(JobConstraint {
            kind: "Coincident",
            ints,
            value: None,
            driving: true,
            name: None,
        });
    }
    assert_eq!(merge_refusal(&on_an_edge, &clean), None);
    // Joined already by a coincidence of their places: refused (:64).
    let mut joined = p.clone();
    let Plan::Run { job, .. } = &mut joined.plan else {
        unreachable!()
    };
    job.constraints.push(JobConstraint {
        kind: "Coincident",
        ints: vec![0, 1, 1, 2],
        value: None,
        driving: true,
        name: None,
    });
    assert!(
        merge_refusal(&joined, &clean).is_some_and(|r| r.contains(":64")),
        "{:?}",
        merge_refusal(&joined, &clean)
    );
    // A solve worse than the one before: its code, or a constraint newly redundant,
    // conflicting, or malformed, each alone (:66); the same lists as before aren't worse.
    let worse = |before: Diagnosis, after: Diagnosis| {
        let fc = FreecadEdited {
            before,
            after,
            ..clean.clone()
        };
        (
            merge_refusal(&p, &fc),
            rolled_back_worse(&p, Some(&Theirs::Edited(Box::new(fc)))),
        )
    };
    let clean_solve = || diagnosis(0, &[], &[], &[]);
    for after in [
        diagnosis(-1, &[], &[], &[]),
        diagnosis(0, &[2], &[], &[]),
        diagnosis(0, &[], &[2], &[]),
        diagnosis(0, &[], &[], &[2]),
    ] {
        let (refused, rolled) = worse(clean_solve(), after.clone());
        assert!(refused.is_some_and(|r| r.contains(":66")), "{after:?}");
        assert!(rolled, "{after:?}");
    }
    let same = diagnosis(0, &[2], &[3], &[4]);
    assert_eq!(worse(same.clone(), same), (None, false));
    // Its geometry not as given (another curve made, or one gone): not rolled back.
    let mut made = clean.clone();
    made.after = diagnosis(0, &[2], &[], &[]);
    made.edited.push(FcGeometry {
        id: 99,
        ..fc_geometry("point", &[("1", [9.0, 9.0])], None)
    });
    assert!(!rolled_back_worse(
        &p,
        Some(&Theirs::Edited(Box::new(made.clone())))
    ));
    made.geometry
        .push(made.edited[made.edited.len() - 1].clone());
    made.edited.pop();
    assert!(!rolled_back_worse(
        &p,
        Some(&Theirs::Edited(Box::new(made)))
    ));
}

#[test]
fn a_merge_gated_for_a_collapsed_line_is_excused_only_where_freecads_line_is_longer_than_1e_7() {
    // n0000003 from (0, 0) to (1, 0) merged end to end: FreeCAD's solve leaves it 1e-7 long
    // (exactly: from the origin along x), or 2e-7.
    let p = merge_of(&["p0000001", "p0000002"], &[]);
    let rule = Rule::MergeGated("n0000003".to_owned());
    let mut fc = rolled_back_by_hand(&p);
    let at = |fc: &mut FreecadEdited, x: f64| {
        *fc.geometry[0].points.get_mut("2").unwrap() = [x, 0.0];
    };
    at(&mut fc, 1e-7);
    assert_eq!(excused(&p, &rule, &fc), None);
    at(&mut fc, 2e-7);
    assert!(excused(&p, &rule, &fc).is_some());
}

#[test]
fn a_merge_whose_solve_silico_refuses_as_unbuildable_is_the_rule_only_for_points_apart() {
    let solved_badly = |at: [f64; 2]| {
        let sketch = sketch_of(
            &[("p0000001", 0.0, 0.0), ("p0000002", at[0], at[1])],
            &[],
            &[],
            &[],
            &[],
        );
        let edit = Edit::Merge {
            points: vec!["p0000001".to_owned(), "p0000002".to_owned()],
        };
        let mut worker = silico_core::sketch::SketchWorker::start().unwrap();
        let Ok(Ok(edited)) = silico_edit(&sketch, &edit, &mut worker) else {
            panic!("not merged");
        };
        let translation = Translation::new(&edited.sketch).unwrap();
        let model = sketch::freecad_sketch(&edited.sketch, &translation).unwrap();
        let ours = Ours {
            dofs: 0,
            conflicting: Vec::new(),
            redundant: Vec::new(),
            partially_redundant: Vec::new(),
            outcome: Err(silico_core::Error::new(
                silico_core::ErrorCode::InvalidResult,
                "by hand",
            )),
        };
        prepared_with(
            sketch,
            edit,
            SilicoSide::Edited(Box::new(SilicoEdited {
                edited,
                model,
                ours,
            })),
        )
    };
    assert_eq!(rule(&solved_badly([0.0, 0.0])), None);
    let apart = solved_badly([3.0, 4.0]);
    assert_eq!(rule(&apart), Some(Rule::MergeSolveUnbuildable));
    // FreeCAD lost on it: a difference that says Silico's solve refused it.
    let v = verdict(
        &apart,
        Some(&Theirs::Lost(
            "FreeCAD took longer than 60 s building it".to_owned(),
        )),
    );
    assert!(
        v.how.len() == 1
            && v.how[0].ends_with("Silico's merge's solve refused it (invalid_result: by hand)"),
        "{v:?}"
    );
}

#[test]
fn a_removals_refusal_reads_its_first_problem_and_tells_them_all() {
    // p0000001 and p0000004 each used by a line that stays.
    let mut worker = silico_core::sketch::SketchWorker::start().unwrap();
    let Ok(Err(r)) = silico_edit(
        &two_lines(&[]),
        &Edit::Remove {
            members: vec!["p0000001".to_owned(), "p0000004".to_owned()],
        },
        &mut worker,
    ) else {
        panic!("removed");
    };
    assert_eq!(r.elements.first().map(String::as_str), Some("p0000001"));
    assert!(
        r.text.contains("p0000001") && r.text.contains("p0000004") && r.text.contains("; "),
        "{r:?}"
    );
}

#[test]
fn a_trims_contact_band_holds_the_curves_tangent_to_it_within_1e_7_with_their_gaps() {
    // The trimmed line n0000003 from a = (1, 2) to b = (4, 6); circles of radius 1 whose
    // centres are 1 + 5e-8, 1 + 2e-7, and 1 across it; and circles about p0000020 (7, 1),
    // radius 2: c0000021 (radius 1) whose centre is 3 + 5e-8 away, c0000023 1 + 5e-8 away
    // (inside it), and c0000025 2 + 1e-6 away.
    let a = [1.0, 2.0];
    let (near, far, touching) = (
        on_slant(a, 2.5, 1.0 + 5e-8),
        on_slant(a, 1.0, -(1.0 + 2e-7)),
        on_slant(a, 4.0, 1.0),
    );
    let along = |d: f64| [7.0 + 0.6 * d, 1.0 + 0.8 * d];
    let sketch = sketch_of(
        &[
            ("p0000001", 1.0, 2.0),
            ("p0000002", 4.0, 6.0),
            ("p0000004", near[0], near[1]),
            ("p0000006", far[0], far[1]),
            ("p0000008", touching[0], touching[1]),
            ("p0000020", 7.0, 1.0),
            ("p0000022", along(3.0 + 5e-8)[0], along(3.0 + 5e-8)[1]),
            ("p0000024", along(1.0 + 5e-8)[0], along(1.0 + 5e-8)[1]),
            ("p0000026", along(2.0 + 1e-6)[0], along(2.0 + 1e-6)[1]),
        ],
        &[("n0000003", "p0000001", "p0000002")],
        &[
            ("c0000005", "p0000004", 1.0),
            ("c0000007", "p0000006", 1.0),
            ("c0000009", "p0000008", 1.0),
            ("c0000019", "p0000020", 2.0),
            ("c0000021", "p0000022", 1.0),
            ("c0000023", "p0000024", 1.0),
            ("c0000025", "p0000026", 1.0),
        ],
        &[],
        &[],
    );
    let band = |curve: &str| {
        contact_band(&prepared_with(
            sketch.clone(),
            Edit::Trim {
                curve: curve.to_owned(),
                pick: [0.0, 0.0],
            },
            refusal("x", &[], "pick"),
        ))
    };
    let line = band("n0000003").unwrap();
    assert_eq!(line.trimmed, "n0000003");
    assert_eq!(line.near.keys().collect::<Vec<_>>(), ["c0000005"]);
    assert!((line.near["c0000005"] - 5e-8).abs() <= 1e-14, "{line:?}");
    let round = band("c0000019").unwrap();
    assert_eq!(
        round.near.keys().collect::<Vec<_>>(),
        ["c0000021", "c0000023"]
    );
    for gap in round.near.values() {
        assert!((gap - 5e-8).abs() <= 1e-14, "{round:?}");
    }
    // A curve tangent as drawn, or none near: no band.
    assert!(band("c0000009").is_none());
    assert!(band("c0000007").is_none());
}

#[test]
fn a_gap_of_exactly_1e_12_from_tangent_is_tangent_as_drawn_not_in_the_band() {
    // c0000002 about the origin, radius 2^-20; c0000004 about (1e-12 + 2^-73, 0), radius
    // 2^-20 - 2^-73: one inside the other, the centres apart by the radii's difference plus
    // exactly the f64 1e-12, so the gap from tangent is exactly that, in f64 as computed.
    // 2^-20 and 2^-73, exactly.
    let (r, step) = (9.5367431640625e-7, 1.058_791_184_067_875_4e-22);
    let q = r - step;
    let apart = 1e-12 + step;
    assert_eq!(apart - (r - q), 1e-12);
    let sketch = sketch_of(
        &[("p0000001", 0.0, 0.0), ("p0000003", apart, 0.0)],
        &[],
        &[("c0000002", "p0000001", r), ("c0000004", "p0000003", q)],
        &[],
        &[],
    );
    let trim = prepared_with(
        sketch,
        Edit::Trim {
            curve: "c0000002".to_owned(),
            pick: [r, 0.0],
        },
        refusal("x", &[], "pick"),
    );
    assert!(contact_band(&trim).is_none(), "{:?}", contact_band(&trim));
}

/// FreeCAD's geometry as `p`'s model gives it, IDs from 1, edited and solved alike, with
/// none of its constraints, and both solves clean.
fn as_modelled(p: &Prepared) -> FreecadEdited {
    let geometry = fc_geometry_of(&p.model.geometry);
    FreecadEdited {
        before: diagnosis(0, &[], &[], &[]),
        ids_before: geometry.iter().map(|g| g.id).collect(),
        edited: geometry.clone(),
        geometry,
        constraints: Vec::new(),
        after: diagnosis(0, &[], &[], &[]),
        applied: Vec::new(),
        placed: None,
    }
}

/// [`arc_30_120`] as a sketch: the arc a0000004 about p0000003 (1, 1), from p0000001 to
/// p0000002.
fn arc_sketch() -> Sketch {
    let (s, ..) = arc_30_120();
    let at = |id: &str| s.points[id];
    let (p1, p2, p3) = (at("p1"), at("p2"), at("p3"));
    sketch_of(
        &[
            ("p0000001", p1[0], p1[1]),
            ("p0000002", p2[0], p2[1]),
            ("p0000003", p3[0], p3[1]),
        ],
        &[],
        &[],
        &[("a0000004", "p0000003", "p0000001", "p0000002")],
        &[],
    )
}

fn extend(curve: &str, end: ExtendEnd, by: ExtendBy) -> Edit {
    Edit::Extend {
        curve: curve.to_owned(),
        end,
        by,
    }
}

#[test]
fn an_arcs_extend_refused_as_short_or_to_nothing_is_the_rule_by_its_sweep_and_turn() {
    // The arc sweeps 90 degrees on a radius of 2.
    let rule_of = |degrees: f64| {
        rule(&prepared_with(
            arc_sketch(),
            extend("a0000004", ExtendEnd::End, ExtendBy::Angle(degrees)),
            refusal(
                "the extend would leave it 1e-7 mm long or flipped",
                &["a0000004"],
                "by",
            ),
        ))
    };
    // Turned back its whole sweep: nothing left.
    assert_eq!(rule_of(-90.0), Some(Rule::ExtendArcToNothing));
    // Past it, or to 1e-6 degrees of it (2 x 1.75e-8 rad, 3.5e-8 mm): short or flipped.
    assert_eq!(rule_of(-100.0), Some(Rule::ExtendShortOrFlipped));
    assert_eq!(rule_of(-90.0 + 1e-6), Some(Rule::ExtendShortOrFlipped));
    // 10 degrees left: neither.
    assert_eq!(rule_of(-80.0), None);
}

/// The line n0000003 from `a` to `b`, extended at its end by `by` mm, Silico's side refused.
fn line_extend(a: [f64; 2], b: [f64; 2], by: f64) -> Prepared {
    prepared_with(
        sketch_of(
            &[("p0000001", a[0], a[1]), ("p0000002", b[0], b[1])],
            &[("n0000003", "p0000001", "p0000002")],
            &[],
            &[],
            &[],
        ),
        extend("n0000003", ExtendEnd::End, ExtendBy::Length(by)),
        refusal("long or flipped", &["n0000003"], "by"),
    )
}

/// FreeCAD's extend of `p`'s line making it run from `c` to `d`.
fn line_made(p: &Prepared, c: [f64; 2], d: [f64; 2]) -> FreecadEdited {
    let mut fc = as_modelled(p);
    for list in [&mut fc.edited, &mut fc.geometry] {
        *list[0].points.get_mut("1").unwrap() = c;
        *list[0].points.get_mut("2").unwrap() = d;
    }
    fc
}

#[test]
fn a_lines_extend_refused_as_short_or_flipped_is_excused_by_its_target_or_freecads_line() {
    let excuse = |p: &Prepared, fc: &FreecadEdited| excused(p, &Rule::ExtendShortOrFlipped, fc);
    let leaves =
        |e: Option<String>| e.is_some_and(|e| e.starts_with("an extend leaving the curve"));
    // Its own target 0 long (5 - 5): refused before any drag, whatever FreeCAD makes.
    let (a, b) = ([1.0, 2.0], [4.0, 6.0]);
    let short = line_extend(a, b, -5.0);
    assert!(
        excuse(&short, &line_made(&short, a, [7.0, 10.0]))
            .is_some_and(|e| e.starts_with("an extend whose own target")),
    );
    // A sound target (6 long), FreeCAD's line: flipped a little (-0.1 along it), at right
    // angles (0 along it exactly), or 5e-8 long its way: each the rule's; 3 long its way:
    // not.
    let p = line_extend(a, b, 1.0);
    assert!(leaves(excuse(&p, &line_made(&p, a, [0.94, 1.92]))));
    assert!(leaves(excuse(&p, &line_made(&p, a, [-3.0, 5.0]))));
    assert!(leaves(excuse(
        &p,
        &line_made(&p, [2.0, 3.0], [2.0 + 3e-8, 3.0 + 4e-8])
    )));
    assert_eq!(excuse(&p, &line_made(&p, a, [2.8, 4.4])), None);
    // From (1, 2) to (1.5, 6), along (0.5, 4): FreeCAD's along (1, -0.2) and (-1, 0.1), each
    // against it (dot products -0.3 and -0.1).
    let b = [1.5, 6.0];
    let p = line_extend(a, b, 1.0);
    assert!(leaves(excuse(&p, &line_made(&p, a, [2.0, 1.8]))));
    assert!(leaves(excuse(&p, &line_made(&p, a, [0.0, 2.1]))));
}

#[test]
fn an_extend_refused_for_what_it_reaches_is_excused_by_freecads_degenerate_curve_or_untouched_sketch()
 {
    // The line n0000003 and the circle c0000005 about p0000004, radius 1.
    let p = prepared_with(
        sketch_of(
            &[
                ("p0000001", 1.0, 2.0),
                ("p0000002", 4.0, 6.0),
                ("p0000004", 9.0, 9.0),
            ],
            &[("n0000003", "p0000001", "p0000002")],
            &[("c0000005", "p0000004", 1.0)],
            &[],
            &[],
        ),
        extend("n0000003", ExtendEnd::End, ExtendBy::Length(1.0)),
        refusal("x", &["n0000003"], "by"),
    );
    let untouched = as_modelled(&p);
    let circle = untouched
        .edited
        .iter()
        .position(|g| g.kind == "circle")
        .unwrap();
    let mut tiny = untouched.clone();
    tiny.edited[circle].radius = Some(1e-8);
    tiny.geometry[circle].radius = Some(1e-8);
    // The drag's frame: FreeCAD's drag making a circle 1e-8 across, or leaving the sketch.
    let frame = |fc| excused(&p, &Rule::ExtendFrame, fc).unwrap_or_default();
    assert!(frame(&tiny).ends_with("FreeCAD's drag makes it, a curve 1e-7 mm or less"));
    assert!(frame(&untouched).contains("FreeCAD 1.1.3's drag fails, leaves the sketch as it was"));
    // The gate's: FreeCAD's result with the circle 1e-8 across.
    assert!(
        excused(&p, &Rule::ExtendGated, &tiny)
            .is_some_and(|e| e.ends_with("FreeCAD makes it, a curve or radius 1e-7 mm or less"))
    );
}

#[test]
fn an_arc_extended_past_a_whole_turn_wraps_its_range_as_freecads_setrange_does() {
    // 370 degrees on at its end: FreeCAD's range wraps to a sweep of 100, its end at 130
    // degrees, its start, centre, and radius kept.
    let (s, c, from, _) = arc_30_120();
    let p = prepared_with(
        arc_sketch(),
        extend("a0000004", ExtendEnd::End, ExtendBy::Angle(370.0)),
        refusal("x", &["a0000004"], "by"),
    );
    let made = |end: [f64; 2], start: [f64; 2]| {
        let mut fc = as_modelled(&p);
        *fc.edited[0].points.get_mut("2").unwrap() = end;
        *fc.edited[0].points.get_mut("1").unwrap() = start;
        fc
    };
    let start = s.points["p1"];
    let at = |deg: f64| on_round(c, from + deg.to_radians());
    assert!(range_wrapped(&p, &made(at(100.0), start)));
    // Its end at 120 or 140 degrees, its start moved, or its end off the circle: not so.
    assert!(!range_wrapped(&p, &made(at(90.0), start)));
    assert!(!range_wrapped(&p, &made(at(110.0), start)));
    assert!(!range_wrapped(&p, &made(at(100.0), at(1.0))));
    let mut off = at(100.0);
    off[0] += 1e-6;
    assert!(!range_wrapped(&p, &made(off, start)));
    // 370 degrees on at its start: its start turned back to 20 degrees, its end kept.
    let p = prepared_with(
        arc_sketch(),
        extend("a0000004", ExtendEnd::Start, ExtendBy::Angle(370.0)),
        refusal("x", &["a0000004"], "by"),
    );
    let end = s.points["p2"];
    let made = |start: [f64; 2]| {
        let mut fc = as_modelled(&p);
        *fc.edited[0].points.get_mut("1").unwrap() = start;
        *fc.edited[0].points.get_mut("2").unwrap() = end;
        fc
    };
    assert!(range_wrapped(&p, &made(at(-10.0))));
    assert!(!range_wrapped(&p, &made(at(0.0))));
    assert!(!range_wrapped(&p, &made(at(-20.0))));
}

#[test]
fn a_place_is_shown_to_the_nanometre_and_never_as_minus_zero() {
    assert_eq!(
        shown([2.000_000_000_4, -1.000_000_000_6]),
        [2.0, -1.000_000_001]
    );
    let zero = shown([-1e-12, 0.0]);
    assert!(zero[0] == 0.0 && zero[0].is_sign_positive(), "{zero:?}");
}
