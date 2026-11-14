//! The ten constraint tools (`relate`) against the study's §7 sketch, `s0000005`, with
//! Fable's additions (the decisions C1 to C9, 2026-11-14, *Tests that must exist*). Each
//! test gives the sketch, picks in order, and a source of IDs (`kt000101` on), and puts the
//! edit `relate` returns through the gate as its command would (`sketch_edit::add`, `merge`,
//! then `remove`). Every expected value is worked by hand in the test's comment from the
//! decisions, the forms' definitions, and plain geometry: never from the tool.

use super::*;
use crate::document::{Direction, Length, SketchArc, SketchCircle, TangentSide};

/// IDs `kt000101`, `kt000102`, ... in turn, skipping any taken.
struct Kt(u32);

impl NewIds for Kt {
    fn next(&mut self, taken: &dyn Fn(&ElementId) -> bool) -> Result<ElementId, Error> {
        loop {
            self.0 += 1;
            let id = id(&format!("kt{:06}", self.0));
            if !taken(&id) {
                return Ok(id);
            }
        }
    }
}

fn circle(name: &str, center: &str, radius: f64) -> SketchCircle {
    SketchCircle {
        id: id(name),
        center: id(center),
        radius: Coordinate::new(radius).unwrap(),
        construction: false,
    }
}

fn arc(name: &str, center: &str, start: &str, end: &str) -> SketchArc {
    SketchArc {
        id: id(name),
        center: id(center),
        start: id(start),
        end: id(end),
        construction: false,
    }
}

fn kt(name: &str, form: Form) -> SketchConstraint {
    SketchConstraint { id: id(name), form }
}

/// The study's §7 sketch, `s0000005` on `xy`, with Fable's additions:
/// - the quadrilateral pe000001 (0, 0), pe000002 (40, 1), pe000003 (40, 21), pe000004
///   (2, 20), its sides ne000001 to ne000004 in turn, ne000002 held `vertical` (kt000001);
/// - circle ce000001 about pe000005 (20, 10), radius 5; circle ce000002 about pe000006
///   (60, 10), radius 4; lone points pe000007 (20, 30) and pe000008 (−10, 30);
/// - arc ae000001 about pe000009 (50, 21), counter-clockwise from pe00000a (50, 31), at 90°,
///   to pe000003 (40, 21), at 180°, radius 10 at both ends;
/// - the construction centreline ne000005, pe00000c (30, −10) to pe00000d (30, 50); line
///   ne000006, pe00000e (50, 0) to pe00000f (55, 5);
/// - Fable's D: line ne000007, pe000010 (70, 0) to pe000011 (70, 20), and arc ae000002 about
///   pe000012 (70, 10), counter-clockwise from pe000010 (−90°) to pe000011 (90°), radius 10,
///   sharing both ends;
/// - line ne000008, pe000014 (0, 15) to pe000013 (20, 15), its end held on ce000001 by
///   kt000002 (`on`: (20, 15) is 5 above the centre); and kt000003, pe000008 `on` ne000001.
fn s5() -> Sketch {
    let mut s = empty();
    for (name, x, y) in [
        ("pe000001", 0.0, 0.0),
        ("pe000002", 40.0, 1.0),
        ("pe000003", 40.0, 21.0),
        ("pe000004", 2.0, 20.0),
        ("pe000005", 20.0, 10.0),
        ("pe000006", 60.0, 10.0),
        ("pe000007", 20.0, 30.0),
        ("pe000008", -10.0, 30.0),
        ("pe000009", 50.0, 21.0),
        ("pe00000a", 50.0, 31.0),
        ("pe00000c", 30.0, -10.0),
        ("pe00000d", 30.0, 50.0),
        ("pe00000e", 50.0, 0.0),
        ("pe00000f", 55.0, 5.0),
        ("pe000010", 70.0, 0.0),
        ("pe000011", 70.0, 20.0),
        ("pe000012", 70.0, 10.0),
        ("pe000013", 20.0, 15.0),
        ("pe000014", 0.0, 15.0),
    ] {
        s.points.push(point(name, x, y));
    }
    for (name, from, to) in [
        ("ne000001", "pe000001", "pe000002"),
        ("ne000002", "pe000002", "pe000003"),
        ("ne000003", "pe000003", "pe000004"),
        ("ne000004", "pe000004", "pe000001"),
        ("ne000005", "pe00000c", "pe00000d"),
        ("ne000006", "pe00000e", "pe00000f"),
        ("ne000007", "pe000010", "pe000011"),
        ("ne000008", "pe000014", "pe000013"),
    ] {
        s.lines.push(line(name, from, to));
    }
    s.lines[4].construction = true;
    s.circles.push(circle("ce000001", "pe000005", 5.0));
    s.circles.push(circle("ce000002", "pe000006", 4.0));
    s.arcs
        .push(arc("ae000001", "pe000009", "pe00000a", "pe000003"));
    s.arcs
        .push(arc("ae000002", "pe000012", "pe000010", "pe000011"));
    s.constraints.push(kt(
        "kt000001",
        Form::Vertical {
            line: el("ne000002"),
        },
    ));
    s.constraints.push(kt(
        "kt000002",
        Form::On {
            point: el("pe000013"),
            curve: el("ce000001"),
        },
    ));
    s.constraints.push(kt(
        "kt000003",
        Form::On {
            point: el("pe000008"),
            curve: el("ne000001"),
        },
    ));
    s
}

/// `text` as a reference: an ID, or `origin`, `x_axis`, `y_axis`.
fn r(text: &str) -> Reference {
    Reference::new(text).unwrap()
}

/// `tool`'s click on `picks`, in order, in `sketch`, IDs from `kt000101`.
fn click(sketch: &Sketch, tool: RelationTool, picks: &[&str]) -> Result<Related, Error> {
    let picks: Vec<Reference> = picks.iter().map(|p| r(p)).collect();
    relate(sketch, tool, &picks, &mut Kt(100))
}

/// `tool`'s click, which must make an edit and leave nothing out.
fn edit(sketch: &Sketch, tool: RelationTool, picks: &[&str]) -> RelateEdit {
    let related =
        click(sketch, tool, picks).unwrap_or_else(|e| panic!("{tool:?} {picks:?}: {e:?}"));
    assert_eq!(related.already, Vec::<ElementId>::new(), "{related:?}");
    assert_eq!(related.message, None, "{related:?}");
    related.edit.unwrap()
}

/// The constraints `tool`'s click adds, which must be a plain `sketch_add`.
fn added(sketch: &Sketch, tool: RelationTool, picks: &[&str]) -> Vec<SketchConstraint> {
    match edit(sketch, tool, picks) {
        RelateEdit::Add(members) => only_constraints(members),
        other => panic!("{tool:?} {picks:?} made {other:?}"),
    }
}

fn only_constraints(members: Members) -> Vec<SketchConstraint> {
    assert!(
        members.points.is_empty()
            && members.lines.is_empty()
            && members.circles.is_empty()
            && members.arcs.is_empty(),
        "{members:?}"
    );
    members.constraints
}

/// `tool`'s refusal of `picks`: its message and elements.
fn refused(sketch: &Sketch, tool: RelationTool, picks: &[&str]) -> (String, Vec<String>) {
    match click(sketch, tool, picks) {
        Err(e) => {
            assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
            (e.message().to_owned(), e.elements().to_vec())
        }
        Ok(related) => panic!("{tool:?} {picks:?} made {related:?}"),
    }
}

/// `picks` as the error's elements are written.
fn named(picks: &[&str]) -> Vec<String> {
    picks.iter().map(|p| (*p).to_owned()).collect()
}

/// `edit` put through the gate as its commands would be: the add, the merge, or the add and
/// then the removal. The sketch it leaves, or the first refusal's errors' messages.
fn gated(sketch: &Sketch, edit: &RelateEdit) -> Result<Sketch, Vec<String>> {
    let messages = |errors: Vec<Error>| errors.iter().map(|e| e.message().to_owned()).collect();
    match edit {
        RelateEdit::Add(members) => sketch_edit::add(sketch, members)
            .map(|e| e.sketch)
            .map_err(|r| messages(r.errors)),
        RelateEdit::Merge(points) => sketch_edit::merge(sketch, points)
            .map(|e| e.sketch)
            .map_err(|e| messages(vec![e])),
        RelateEdit::AddThenRemove { add, remove } => {
            let added = sketch_edit::add(sketch, add).map_err(|r| messages(r.errors))?;
            sketch_edit::remove(&added.sketch, remove)
                .map(|e| e.sketch)
                .map_err(|r| messages(r.errors))
        }
    }
}

/// `edit` through the gate, which must take it.
fn taken(sketch: &Sketch, edit: &RelateEdit) -> Sketch {
    gated(sketch, edit).unwrap_or_else(|e| panic!("the gate refused {edit:?}: {e:?}"))
}

fn constraint_of(sketch: &Sketch, name: &str) -> Option<Form> {
    sketch
        .constraints
        .iter()
        .find(|c| c.id == id(name))
        .map(|c| c.form.clone())
}

use RelationTool::*;

// --- The gate's assumptions Fable didn't run (Residual risk), first --------------------

#[test]
fn the_gate_takes_a_tangent_on_beside_the_on_of_its_point_and_curve_then_the_ons_removal() {
    // kt000002 holds pe000013, ne000008's end, on ce000001. A tangent_on of that end on that
    // curve beside it is taken: the gate's tangent rule pairs an edge `tangent` with an `on`
    // or a `tangent_on`, never a `tangent_on` with an `on`. Then the `on`'s removal, by ID.
    let s = s5();
    let add = Members {
        constraints: vec![kt(
            "kt000101",
            Form::TangentOn {
                a: el("ne000008"),
                at: el("pe000013"),
                b: el("ce000001"),
                direction: Direction::Same,
            },
        )],
        ..Members::default()
    };
    let beside = sketch_edit::add(&s, &add).unwrap_or_else(|r| panic!("{r:?}"));
    assert!(constraint_of(&beside.sketch, "kt000002").is_some());
    let removed =
        sketch_edit::remove(&beside.sketch, &[id("kt000002")]).unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(removed.sketch.constraints.len(), 3);
    assert_eq!(constraint_of(&removed.sketch, "kt000002"), None);
}

#[test]
fn the_removal_of_any_constraint_by_its_id_is_never_refused() {
    // sketch_edit::remove refuses a member the sketch hasn't, a fixed reference, and a
    // point a curve still uses: a constraint by ID is none of them.
    let s = s5();
    for c in &s.constraints {
        let removed = sketch_edit::remove(&s, std::slice::from_ref(&c.id))
            .unwrap_or_else(|r| panic!("{r:?}"));
        assert_eq!(removed.removed, vec![c.id.clone()]);
    }
}

// --- The study's §7, kept as written ------------------------------------------------------

#[test]
fn horizontal_of_two_lines_is_one_horizontal_each_in_pick_order_and_lines_with_a_point_are_refused()
{
    // §7 1.
    let s = s5();
    let want = vec![
        kt(
            "kt000101",
            Form::Horizontal {
                line: el("ne000001"),
            },
        ),
        kt(
            "kt000102",
            Form::Horizontal {
                line: el("ne000003"),
            },
        ),
    ];
    let edit = edit(&s, Horizontal, &["ne000001", "ne000003"]);
    assert_eq!(
        edit,
        RelateEdit::Add(Members {
            constraints: want,
            ..Members::default()
        })
    );
    taken(&s, &edit);
    // Lines and points together: Horizontal's sentence, the selection as its elements.
    let picks = ["ne000001", "ne000003", "pe000007"];
    assert_eq!(
        refused(&s, Horizontal, &picks),
        (
            "Horizontal takes lines, or two or more points".to_owned(),
            named(&picks)
        )
    );
}

#[test]
fn vertical_of_two_points_is_vertical_points_and_a_circle_is_refused_by_verticals_sentence() {
    // §7 2.
    let s = s5();
    let constraints = added(&s, Vertical, &["pe000007", "pe000005"]);
    assert_eq!(
        constraints,
        vec![kt(
            "kt000101",
            Form::VerticalPoints {
                a: el("pe000007"),
                b: el("pe000005"),
            },
        )]
    );
    taken(&s, &edit(&s, Vertical, &["pe000007", "pe000005"]));
    assert_eq!(
        refused(&s, Vertical, &["ce000001"]),
        (
            "Vertical takes lines, or two or more points".to_owned(),
            named(&["ce000001"])
        )
    );
}

#[test]
fn coincident_of_a_point_and_a_line_is_an_on() {
    // §7 3, the first.
    let s = s5();
    let edit = edit(&s, Coincident, &["pe000007", "ne000003"]);
    assert_eq!(
        edit,
        RelateEdit::Add(Members {
            constraints: vec![kt(
                "kt000101",
                Form::On {
                    point: el("pe000007"),
                    curve: el("ne000003"),
                },
            )],
            ..Members::default()
        })
    );
    taken(&s, &edit);
}

#[test]
fn coincident_of_two_points_merges_them_the_first_picked_surviving() {
    // §7 3, the second: pe000008 survives at (−10, 30); ne000003 and ne000004 then end
    // there, and pe000004 is removed.
    let s = s5();
    let edit = edit(&s, Coincident, &["pe000008", "pe000004"]);
    assert_eq!(
        edit,
        RelateEdit::Merge(vec![id("pe000008"), id("pe000004")])
    );
    let merged = taken(&s, &edit);
    assert!(!merged.points.iter().any(|p| p.id == id("pe000004")));
    let kept = merged
        .points
        .iter()
        .find(|p| p.id == id("pe000008"))
        .unwrap();
    assert_eq!((kept.x.mm(), kept.y.mm()), (-10.0, 30.0));
    let ne3 = merged
        .lines
        .iter()
        .find(|l| l.id == id("ne000003"))
        .unwrap();
    let ne4 = merged
        .lines
        .iter()
        .find(|l| l.id == id("ne000004"))
        .unwrap();
    assert_eq!((&ne3.from, &ne3.to), (&id("pe000003"), &id("pe000008")));
    assert_eq!((&ne4.from, &ne4.to), (&id("pe000008"), &id("pe000001")));
}

/// #278's A pair for `point` and the origin, as the drawing tools write it.
fn at_the_origin(point: &str) -> Vec<SketchConstraint> {
    vec![
        kt(
            "kt000101",
            Form::HorizontalPoints {
                a: el(point),
                b: Reference::Origin,
            },
        ),
        kt(
            "kt000102",
            Form::VerticalPoints {
                a: el(point),
                b: Reference::Origin,
            },
        ),
    ]
}

#[test]
fn coincident_of_a_point_and_the_origin_is_the_pair_the_drawing_tools_write() {
    // §7 3, the third.
    let s = s5();
    assert_eq!(
        added(&s, Coincident, &["pe000007", "origin"]),
        at_the_origin("pe000007")
    );
    taken(&s, &edit(&s, Coincident, &["pe000007", "origin"]));
}

#[test]
fn tangent_of_a_line_and_a_circle_apart_is_an_edge_tangent_on_the_side_the_centre_is_drawn() {
    // §7 4, the first: ne000002's direction is (0, 20), the centre less its `from`
    // (60 − 40, 10 − 1) = (20, 9); the cross product 0 × 9 − 20 × 20 = −400 < 0: right.
    let s = s5();
    let edit = edit(&s, Tangent, &["ne000002", "ce000002"]);
    assert_eq!(
        edit,
        RelateEdit::Add(Members {
            constraints: vec![kt(
                "kt000101",
                Form::Tangent {
                    a: el("ne000002"),
                    b: el("ce000002"),
                    side: TangentSide::Right,
                },
            )],
            ..Members::default()
        })
    );
    taken(&s, &edit);
}

#[test]
fn tangent_of_a_line_and_an_arc_sharing_an_end_is_a_tangent_at_there() {
    // §7 4, the second: they share pe000003, ne000002's `to` and ae000001's `end`. Arriving
    // along ne000002 is (0, 20). The arc's own direction at (40, 21) is (−(21 − 21),
    // 40 − 50) = (0, −10); leaving it from its end, reversed, (0, 10). The dot product is
    // 200 ≥ 0: same.
    let s = s5();
    let constraints = added(&s, Tangent, &["ne000002", "ae000001"]);
    assert_eq!(
        constraints,
        vec![kt(
            "kt000101",
            Form::TangentAt {
                a: el("ne000002"),
                b: el("ae000001"),
                at: el("pe000003"),
                direction: Direction::Same,
            },
        )]
    );
    taken(&s, &edit(&s, Tangent, &["ne000002", "ae000001"]));
}

#[test]
fn tangent_of_two_lines_is_refused_in_the_gates_words() {
    // §7 4's refusal, in the gate's own words (C3).
    let s = s5();
    let picks = ["ne000001", "ne000003"];
    assert_eq!(
        refused(&s, Tangent, &picks),
        (
            "a tangent takes a line and a circle or arc, or two circles or arcs: two lines can't be tangent"
                .to_owned(),
            named(&picks)
        )
    );
    // A line and an axis are two lines too, and both axes.
    assert_eq!(
        refused(&s, Tangent, &["x_axis", "ne000001"]).0,
        refused(&s, Tangent, &picks).0
    );
    assert_eq!(
        refused(&s, Tangent, &["x_axis", "y_axis"]).0,
        refused(&s, Tangent, &picks).0
    );
}

#[test]
fn equal_of_three_rounds_is_a_star_from_the_first_picked() {
    // §7 5 (C8): each after the first paired with it, `a` the first.
    let s = s5();
    let constraints = added(&s, Equal, &["ce000001", "ce000002", "ae000001"]);
    assert_eq!(
        constraints,
        vec![
            kt(
                "kt000101",
                Form::Equal {
                    a: el("ce000001"),
                    b: el("ce000002"),
                },
            ),
            kt(
                "kt000102",
                Form::Equal {
                    a: el("ce000001"),
                    b: el("ae000001"),
                },
            ),
        ]
    );
    taken(&s, &edit(&s, Equal, &["ce000001", "ce000002", "ae000001"]));
}

#[test]
fn equal_of_three_lines_in_an_order_not_by_id_pairs_each_with_the_first_clicked() {
    // C8: [ne000004, ne000001, ne000003], each after the first with it, never a chain
    // (ne000001 with ne000003), never by ID.
    let s = s5();
    let constraints = added(&s, Equal, &["ne000004", "ne000001", "ne000003"]);
    assert_eq!(
        constraints,
        vec![
            kt(
                "kt000101",
                Form::Equal {
                    a: el("ne000004"),
                    b: el("ne000001"),
                },
            ),
            kt(
                "kt000102",
                Form::Equal {
                    a: el("ne000004"),
                    b: el("ne000003"),
                },
            ),
        ]
    );
}

#[test]
fn parallel_of_two_lines_is_one_parallel_and_a_circle_is_refused() {
    // §7 6.
    let s = s5();
    let constraints = added(&s, Parallel, &["ne000001", "ne000003"]);
    assert_eq!(
        constraints,
        vec![kt(
            "kt000101",
            Form::Parallel {
                a: el("ne000001"),
                b: el("ne000003"),
            },
        )]
    );
    taken(&s, &edit(&s, Parallel, &["ne000001", "ne000003"]));
    assert_eq!(
        refused(&s, Parallel, &["ne000001", "ce000001"]),
        (
            "Parallel takes two or more lines".to_owned(),
            named(&["ne000001", "ce000001"])
        )
    );
}

#[test]
fn perpendicular_of_two_lines_and_three_refused() {
    // §7 7.
    let s = s5();
    let constraints = added(&s, Perpendicular, &["ne000001", "ne000004"]);
    assert_eq!(
        constraints,
        vec![kt(
            "kt000101",
            Form::Perpendicular {
                a: el("ne000001"),
                b: el("ne000004"),
            },
        )]
    );
    taken(&s, &edit(&s, Perpendicular, &["ne000001", "ne000004"]));
    let picks = ["ne000001", "ne000003", "ne000004"];
    assert_eq!(
        refused(&s, Perpendicular, &picks),
        ("Perpendicular takes two lines".to_owned(), named(&picks))
    );
}

#[test]
fn midpoint_of_a_point_and_a_line_either_way_round() {
    // §7 8, the first.
    let s = s5();
    let want = vec![kt(
        "kt000101",
        Form::Midpoint {
            point: el("pe000007"),
            line: el("ne000001"),
        },
    )];
    assert_eq!(added(&s, Midpoint, &["pe000007", "ne000001"]), want);
    assert_eq!(added(&s, Midpoint, &["ne000001", "pe000007"]), want);
    taken(&s, &edit(&s, Midpoint, &["pe000007", "ne000001"]));
    // A point and an arc: Midpoint's sentence.
    assert_eq!(
        refused(&s, Midpoint, &["pe000007", "ae000001"]),
        (
            "Midpoint takes a point and a line".to_owned(),
            named(&["pe000007", "ae000001"])
        )
    );
}

#[test]
fn symmetric_of_two_points_about_a_line() {
    // §7 9, the first.
    let s = s5();
    let constraints = added(&s, Symmetric, &["pe000007", "pe000008", "ne000005"]);
    assert_eq!(
        constraints,
        vec![kt(
            "kt000101",
            Form::Symmetric {
                a: el("pe000007"),
                b: el("pe000008"),
                about: el("ne000005"),
            },
        )]
    );
    taken(
        &s,
        &edit(&s, Symmetric, &["pe000007", "pe000008", "ne000005"]),
    );
}

#[test]
fn vertical_on_a_line_held_vertical_adds_nothing_and_says_so() {
    // §7 11 (C6): kt000001 is that constraint.
    let s = s5();
    let related = click(&s, Vertical, &["ne000002"]).unwrap();
    assert_eq!(
        related,
        Related {
            edit: None,
            already: vec![id("kt000001")],
            message: Some("line ne000002 is already vertical: nothing was added".to_owned()),
        }
    );
}

// --- Fable's changed and added cases ------------------------------------------------------

#[test]
fn f1_coincident_of_a_curves_own_point_is_the_on_the_gate_refuses() {
    // pe000003 is ne000002's `to`: the tool writes the `on`, and the gate refuses it in its
    // words (FreeCAD skips it; Silico doesn't).
    let s = s5();
    let edit = edit(&s, Coincident, &["pe000003", "ne000002"]);
    assert_eq!(
        edit,
        RelateEdit::Add(Members {
            constraints: vec![kt(
                "kt000101",
                Form::On {
                    point: el("pe000003"),
                    curve: el("ne000002"),
                },
            )],
            ..Members::default()
        })
    );
    assert_eq!(
        gated(&s, &edit),
        Err(vec![
            "a on's point, pe000003, is part of ne000002 itself".to_owned()
        ])
    );
}

#[test]
fn f2_coincident_of_the_origin_and_a_point_is_the_same_pair_either_way() {
    let s = s5();
    assert_eq!(
        added(&s, Coincident, &["origin", "pe000007"]),
        at_the_origin("pe000007")
    );
    assert_eq!(
        added(&s, Coincident, &["pe000007", "origin"]),
        at_the_origin("pe000007")
    );
}

#[test]
fn f3_coincident_of_the_origin_and_two_points_is_refused() {
    let s = s5();
    let picks = ["pe000007", "pe000008", "origin"];
    assert_eq!(
        refused(&s, Coincident, &picks),
        (
            "Coincident takes points and the curves they lie on, or two or more points to make one"
                .to_owned(),
            named(&picks)
        )
    );
}

#[test]
fn f4_equal_of_a_line_and_a_circle_is_the_equal_the_gate_refuses() {
    let s = s5();
    let edit = edit(&s, Equal, &["ne000001", "ce000001"]);
    assert_eq!(
        edit,
        RelateEdit::Add(Members {
            constraints: vec![kt(
                "kt000101",
                Form::Equal {
                    a: el("ne000001"),
                    b: el("ce000001"),
                },
            )],
            ..Members::default()
        })
    );
    assert_eq!(
        gated(&s, &edit),
        Err(vec![
            "an equal takes two lines, or two circles or arcs".to_owned()
        ])
    );
}

#[test]
fn f5_tangent_of_curves_sharing_both_ends_is_refused_naming_both() {
    // ne000007 and ae000002 share pe000010 and pe000011, the D's two corners.
    let s = s5();
    let picks = ["ne000007", "ae000002"];
    let (message, elements) = refused(&s, Tangent, &picks);
    assert_eq!(
        message,
        "line ne000007 and arc ae000002 could be tangent at more than one point, pe000010 and pe000011: select the one they're tangent at too"
    );
    assert_eq!(
        elements,
        named(&["ne000007", "ae000002", "pe000010", "pe000011"])
    );
}

#[test]
fn f6_tangent_with_the_place_picked_is_a_tangent_at_there() {
    // The line arrives at pe000011 along (0, 20). The arc's own direction at (70, 20) is
    // (−(20 − 10), 70 − 70) = (−10, 0); pe000011 is its end, so leaving it is (10, 0). The
    // dot product is 0, and the tie goes to same.
    let s = s5();
    let edit = edit(&s, Tangent, &["ne000007", "ae000002", "pe000011"]);
    assert_eq!(
        edit,
        RelateEdit::Add(Members {
            constraints: vec![kt(
                "kt000101",
                Form::TangentAt {
                    a: el("ne000007"),
                    b: el("ae000002"),
                    at: el("pe000011"),
                    direction: Direction::Same,
                },
            )],
            ..Members::default()
        })
    );
    taken(&s, &edit);
    // At the other corner, pe000010, ne000007's `from`: arriving is the reverse of leaving,
    // −(0, 20) = (0, −20). The arc's own direction at (70, 0) is (−(0 − 10), 70 − 70) =
    // (10, 0), its start, so leaving it is (10, 0): the dot product 0 again, same.
    let edit = edit_at(&s, "pe000010");
    assert_eq!(
        edit,
        vec![kt(
            "kt000101",
            Form::TangentAt {
                a: el("ne000007"),
                b: el("ae000002"),
                at: el("pe000010"),
                direction: Direction::Same,
            },
        )]
    );
    // A point that's neither place: Tangent's sentence.
    let picks = ["ne000007", "ae000002", "pe000012"];
    assert_eq!(
        refused(&s, Tangent, &picks),
        (
            "Tangent takes two curves, at least one a circle or an arc".to_owned(),
            named(&picks)
        )
    );
}

/// Tangent on the D, ne000007 and ae000002, at `at`.
fn edit_at(s: &Sketch, at: &str) -> Vec<SketchConstraint> {
    added(s, Tangent, &["ne000007", "ae000002", at])
}

#[test]
fn a_tangent_at_where_the_drawing_turns_back_is_opposite() {
    // ae000002 reversed in its drawing: the line ne000009 from pe000011 (70, 20) to
    // pe000015 (80, 20), and Tangent of it with ae000002 at pe000011, its only place. The line
    // leaves pe000011 along (10, 0), so it arrives along (−10, 0); the arc leaves its end
    // along (10, 0). The dot product is −100 < 0: opposite.
    let mut s = s5();
    s.points.push(point("pe000015", 80.0, 20.0));
    s.lines.push(line("ne000009", "pe000011", "pe000015"));
    assert_eq!(
        added(&s, Tangent, &["ne000009", "ae000002"]),
        vec![kt(
            "kt000101",
            Form::TangentAt {
                a: el("ne000009"),
                b: el("ae000002"),
                at: el("pe000011"),
                direction: Direction::Opposite,
            },
        )]
    );
}

#[test]
fn f7_tangent_at_an_end_held_on_is_a_tangent_on_replacing_the_on() {
    // One place, pe000013, ne000008's `to`, which kt000002 holds on ce000001. Leaving it
    // along the line is (0 − 20, 15 − 15) = (−20, 0). The circle's own direction at
    // (20, 15) is (−(15 − 10), 20 − 20) = (−5, 0). The dot product is 100: same. The edit is
    // the add of the tangent_on, then the removal of kt000002.
    let s = s5();
    let want = RelateEdit::AddThenRemove {
        add: Members {
            constraints: vec![kt(
                "kt000101",
                Form::TangentOn {
                    a: el("ne000008"),
                    at: el("pe000013"),
                    b: el("ce000001"),
                    direction: Direction::Same,
                },
            )],
            ..Members::default()
        },
        remove: vec![id("kt000002")],
    };
    assert_eq!(edit(&s, Tangent, &["ne000008", "ce000001"]), want);
    // The structure fixes `a`: the other order gives the same edit.
    assert_eq!(edit(&s, Tangent, &["ce000001", "ne000008"]), want);
    // Through the gate in that order, both are taken.
    let after = taken(&s, &want);
    assert_eq!(constraint_of(&after, "kt000002"), None);
    assert!(constraint_of(&after, "kt000101").is_some());
}

#[test]
fn tangent_again_where_a_tangent_on_holds_the_end_adds_nothing_and_replaces_nothing() {
    // f7's flow, then Tangent again on the same selection, as the selection stays. The
    // tangent_on kt000101 now holds pe000013 on ce000001, so it's the one place (C3), and the
    // tangent_on the click would write, same as drawn, is kt000101 itself: left out (C6).
    let s = s5();
    let after = taken(&s, &edit(&s, Tangent, &["ne000008", "ce000001"]));
    // The message names the two as picked.
    for (picks, named) in [
        (
            ["ne000008", "ce000001"],
            "line ne000008 and circle ce000001",
        ),
        (
            ["ce000001", "ne000008"],
            "circle ce000001 and line ne000008",
        ),
    ] {
        assert_eq!(
            click(&after, Tangent, &picks).unwrap(),
            Related {
                edit: None,
                already: vec![id("kt000101")],
                message: Some(format!("{named} are already tangent: nothing was added")),
            },
            "{picks:?}"
        );
    }
    // A tangent_on held the other way round (opposite, where the drawing reads same): the
    // click's isn't its repeat, and is added, to conflict, by a plain add: with no `on` there,
    // nothing is removed (C5).
    let mut opposite = after.clone();
    if let Some(Form::TangentOn { direction, .. }) = opposite
        .constraints
        .iter_mut()
        .find(|c| c.id == id("kt000101"))
        .map(|c| &mut c.form)
    {
        *direction = Direction::Opposite;
    }
    let made = edit(&opposite, Tangent, &["ne000008", "ce000001"]);
    assert_eq!(
        made,
        RelateEdit::Add(Members {
            constraints: vec![kt(
                "kt000102",
                Form::TangentOn {
                    a: el("ne000008"),
                    at: el("pe000013"),
                    b: el("ce000001"),
                    direction: Direction::Same,
                },
            )],
            ..Members::default()
        })
    );
    taken(&opposite, &made);
}

#[test]
fn a_tangent_on_whose_curve_leaves_against_the_circle_is_opposite() {
    // ne000008 drawn the other way, from pe000013 (20, 15) to pe000014 (0, 15): leaving
    // pe000013, its start, along (−20, 0) still; the circle's own direction there (−5, 0):
    // same. Held at its `from` instead, by a line from pe000013 to pe000016 (40, 15): it
    // leaves along (20, 0), against (−5, 0), the dot product −100: opposite.
    let mut s = s5();
    s.lines[7] = line("ne000008", "pe000013", "pe000014");
    assert_eq!(
        added_on(&s, "ne000008"),
        Form::TangentOn {
            a: el("ne000008"),
            at: el("pe000013"),
            b: el("ce000001"),
            direction: Direction::Same,
        }
    );
    s.points.push(point("pe000016", 40.0, 15.0));
    s.lines[7] = line("ne000008", "pe000013", "pe000016");
    assert_eq!(
        added_on(&s, "ne000008"),
        Form::TangentOn {
            a: el("ne000008"),
            at: el("pe000013"),
            b: el("ce000001"),
            direction: Direction::Opposite,
        }
    );
}

/// The tangent_on Tangent adds of `line` and ce000001, replacing kt000002.
fn added_on(s: &Sketch, line: &str) -> Form {
    match edit(s, Tangent, &[line, "ce000001"]) {
        RelateEdit::AddThenRemove { add, remove } => {
            assert_eq!(remove, vec![id("kt000002")]);
            only_constraints(add).remove(0).form
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_tangent_on_an_axis_reads_the_axis_running_toward_plus_x() {
    // The arc ae000003 about pe000017 (0, 10), counter-clockwise from pe000018 (0, 0), at
    // −90°, to pe000019 (10, 10), its start held on x_axis. Leaving its start, the arc's own
    // direction there, (−(0 − 10), 0 − 0) = (10, 0); the axis runs (1, 0): same. Drawn the
    // other way, about the same centre from pe000019 to pe000018, held at its end: leaving it
    // is the reverse of (10, 0), against the axis: opposite.
    let mut s = s5();
    s.points.push(point("pe000017", 0.0, 10.0));
    s.points.push(point("pe000018", 0.0, 0.0));
    s.points.push(point("pe000019", 10.0, 10.0));
    s.arcs
        .push(arc("ae000003", "pe000017", "pe000018", "pe000019"));
    s.constraints.push(kt(
        "kt000004",
        Form::On {
            point: el("pe000018"),
            curve: Reference::XAxis,
        },
    ));
    let want = |direction| RelateEdit::AddThenRemove {
        add: Members {
            constraints: vec![kt(
                "kt000101",
                Form::TangentOn {
                    a: el("ae000003"),
                    at: el("pe000018"),
                    b: Reference::XAxis,
                    direction,
                },
            )],
            ..Members::default()
        },
        remove: vec![id("kt000004")],
    };
    let made = edit(&s, Tangent, &["x_axis", "ae000003"]);
    assert_eq!(made, want(Direction::Same));
    taken(&s, &made);
    s.arcs[2] = arc("ae000003", "pe000017", "pe000019", "pe000018");
    assert_eq!(
        edit(&s, Tangent, &["x_axis", "ae000003"]),
        want(Direction::Opposite)
    );
}

#[test]
fn f8_tangent_of_a_circle_whose_centre_is_on_the_line_is_refused() {
    // ce000003 about pe000015 (40, 11), on ne000002: the cross product
    // (0, 20) × (40 − 40, 11 − 1) = 0 × 10 − 20 × 0 = 0.
    let mut s = s5();
    s.points.push(point("pe000015", 40.0, 11.0));
    s.circles.push(circle("ce000003", "pe000015", 3.0));
    let picks = ["ne000002", "ce000003"];
    assert_eq!(
        refused(&s, Tangent, &picks),
        (
            "the centre of circle ce000003 is on line ne000002: which side it's tangent on is unclear; move one off the other first"
                .to_owned(),
            named(&picks)
        )
    );
    // Its centre 1e-9 mm to the left, (40 − 1e-9, 11): (0, 20) × (−1e-9, 10) = 2e-8 > 0, the
    // left; to the right, right. Exactly zero is refused, nothing near it.
    s.points.last_mut().unwrap().x = Coordinate::new(40.0 - 1e-9).unwrap();
    assert_eq!(side_of(&s, "ne000002", "ce000003"), TangentSide::Left);
    s.points.last_mut().unwrap().x = Coordinate::new(40.0 + 1e-9).unwrap();
    assert_eq!(side_of(&s, "ne000002", "ce000003"), TangentSide::Right);
}

/// The side of the edge tangent Tangent adds of `a` and `b`.
fn side_of(s: &Sketch, a: &str, b: &str) -> TangentSide {
    match added(s, Tangent, &[a, b]).remove(0).form {
        Form::Tangent { side, .. } => side,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_round_on_the_left_of_a_line_or_an_axis_is_left() {
    // ce000001's centre (20, 10) from ne000001 (0, 0)-(40, 1): (40, 1) × (20, 10) =
    // 40 × 10 − 1 × 20 = 380 > 0, left. x_axis runs (1, 0): (1, 0) × (20, 10) = 10, left;
    // y_axis runs (0, 1): (0, 1) × (20, 10) = 0 × 10 − 1 × 20 = −20, right.
    let s = s5();
    assert_eq!(side_of(&s, "ne000001", "ce000001"), TangentSide::Left);
    assert_eq!(side_of(&s, "ce000001", "x_axis"), TangentSide::Left);
    assert_eq!(side_of(&s, "y_axis", "ce000001"), TangentSide::Right);
}

#[test]
fn f9_tangent_of_two_rounds_is_outside_at_least_the_larger_radius_apart_else_inside() {
    // ce000001 and ce000002: centres (20, 10) and (60, 10), 40 apart, the larger radius 5:
    // outside.
    let mut s = s5();
    assert_eq!(side_of(&s, "ce000001", "ce000002"), TangentSide::Outside);
    // Radii 10 and 4, centres 7 apart: inside. 10 apart, the larger radius exactly: outside
    // (FreeCAD's d ≥ r).
    s.points.push(point("pe000015", 100.0, 0.0));
    s.points.push(point("pe000016", 107.0, 0.0));
    s.points.push(point("pe000017", 110.0, 0.0));
    s.circles.push(circle("ce000003", "pe000015", 10.0));
    s.circles.push(circle("ce000004", "pe000016", 4.0));
    s.circles.push(circle("ce000005", "pe000017", 4.0));
    assert_eq!(side_of(&s, "ce000003", "ce000004"), TangentSide::Inside);
    assert_eq!(side_of(&s, "ce000004", "ce000003"), TangentSide::Inside);
    assert_eq!(side_of(&s, "ce000003", "ce000005"), TangentSide::Outside);
    // An arc's radius is its centre to its start: ae000001's 10, its centre (50, 21) from
    // ce000002's (60, 10) √(100 + 121) = 14.87 ≥ 10: outside.
    assert_eq!(side_of(&s, "ae000001", "ce000002"), TangentSide::Outside);
    taken(&s, &edit(&s, Tangent, &["ce000003", "ce000004"]));
}

#[test]
fn two_rounds_about_one_centre_are_written_inside_for_the_solve_to_judge() {
    // C3: not refused by the tool. ce000003 about pe000005, ce000001's centre: 0 apart,
    // less than 7: inside.
    let mut s = s5();
    s.circles.push(circle("ce000003", "pe000005", 7.0));
    assert_eq!(side_of(&s, "ce000001", "ce000003"), TangentSide::Inside);
}

#[test]
fn f10_midpoint_over_an_on_replaces_it() {
    // kt000003 holds pe000008 on ne000001: the midpoint, then its removal.
    let s = s5();
    let edit = edit(&s, Midpoint, &["pe000008", "ne000001"]);
    assert_eq!(
        edit,
        RelateEdit::AddThenRemove {
            add: Members {
                constraints: vec![kt(
                    "kt000101",
                    Form::Midpoint {
                        point: el("pe000008"),
                        line: el("ne000001"),
                    },
                )],
                ..Members::default()
            },
            remove: vec![id("kt000003")],
        }
    );
    let after = taken(&s, &edit);
    assert_eq!(constraint_of(&after, "kt000003"), None);
    // An `on` of the point on another curve, or of another point, is never replaced.
    assert!(matches!(
        self::edit(&s, Midpoint, &["pe000008", "ne000003"]),
        RelateEdit::Add(_)
    ));
}

#[test]
fn f11_symmetric_of_three_points_mirrors_about_the_last_picked() {
    let s = s5();
    assert_eq!(
        added(&s, Symmetric, &["pe000007", "pe000008", "pe000005"]),
        vec![kt(
            "kt000101",
            Form::Symmetric {
                a: el("pe000007"),
                b: el("pe000008"),
                about: el("pe000005"),
            },
        )]
    );
    assert_eq!(
        added(&s, Symmetric, &["pe000005", "pe000007", "pe000008"]),
        vec![kt(
            "kt000101",
            Form::Symmetric {
                a: el("pe000005"),
                b: el("pe000007"),
                about: el("pe000008"),
            },
        )]
    );
    taken(
        &s,
        &edit(&s, Symmetric, &["pe000007", "pe000008", "pe000005"]),
    );
}

#[test]
fn f12_symmetric_with_the_line_picked_first_mirrors_about_it() {
    let s = s5();
    assert_eq!(
        added(&s, Symmetric, &["ne000005", "pe000007", "pe000008"]),
        vec![kt(
            "kt000101",
            Form::Symmetric {
                a: el("pe000007"),
                b: el("pe000008"),
                about: el("ne000005"),
            },
        )]
    );
    // An axis is a line here, and the origin a point.
    assert_eq!(
        added(&s, Symmetric, &["pe000007", "y_axis", "origin"]),
        vec![kt(
            "kt000101",
            Form::Symmetric {
                a: el("pe000007"),
                b: Reference::Origin,
                about: Reference::YAxis,
            },
        )]
    );
}

#[test]
fn f13_symmetric_of_lines_or_of_other_counts_is_refused() {
    let s = s5();
    let sentence =
        "Symmetric takes two points and the line or point they mirror about, the point picked last";
    for picks in [
        &["ne000001", "ne000006", "ne000005"][..],
        &["pe000007", "pe000008"],
        &["pe000007", "pe000008", "pe000005", "pe000006"],
        &["pe000007", "ce000001", "ne000005"],
        &["pe000007", "ne000001", "ne000005"],
    ] {
        assert_eq!(
            refused(&s, Symmetric, picks),
            (sentence.to_owned(), named(picks)),
            "{picks:?}"
        );
    }
}

/// Fix's two dimensions of a point, as written: `distance_x` then `distance_y`.
fn fixed(dx: (&str, &str, &str), dy: (&str, &str, &str)) -> Vec<SketchConstraint> {
    let length = |v: &str| Length::parse(v).unwrap();
    vec![
        kt(
            "kt000101",
            Form::DistanceX {
                a: r(dx.0),
                b: r(dx.1),
                value: length(dx.2),
                driving: true,
            },
        ),
        kt(
            "kt000102",
            Form::DistanceY {
                a: r(dy.0),
                b: r(dy.1),
                value: length(dy.2),
                driving: true,
            },
        ),
    ]
}

#[test]
fn f14_fix_is_a_driving_distance_x_and_distance_y_from_the_origin() {
    // pe000007 at (20, 30).
    let s = s5();
    assert_eq!(
        added(&s, Fix, &["pe000007"]),
        fixed(("origin", "pe000007", "20"), ("origin", "pe000007", "30"))
    );
    taken(&s, &edit(&s, Fix, &["pe000007"]));
}

#[test]
fn f15_fix_left_of_the_origin_puts_the_point_first_so_the_value_is_positive() {
    // pe000008 at (−10, 30): it's to the left, so it's `a`, the value 10.
    let s = s5();
    assert_eq!(
        added(&s, Fix, &["pe000008"]),
        fixed(("pe000008", "origin", "10"), ("origin", "pe000008", "30"))
    );
    taken(&s, &edit(&s, Fix, &["pe000008"]));
}

#[test]
fn f16_fix_at_the_origins_place_is_two_zeros_from_the_origin() {
    // pe000001 at (0, 0): level and plumb with the origin, the order picked, (origin, point).
    let s = s5();
    assert_eq!(
        added(&s, Fix, &["pe000001"]),
        fixed(("origin", "pe000001", "0"), ("origin", "pe000001", "0"))
    );
    taken(&s, &edit(&s, Fix, &["pe000001"]));
}

#[test]
fn f17_fix_rounds_to_the_nanometre() {
    // (20.00000049, 30): 20 mm and 0.49 nm, rounded to 20. (20.0000006, 30): 0.6 nm, to
    // 20.000001.
    let mut s = s5();
    s.points[6].x = Coordinate::new(20.00000049).unwrap();
    assert_eq!(
        added(&s, Fix, &["pe000007"]),
        fixed(("origin", "pe000007", "20"), ("origin", "pe000007", "30"))
    );
    s.points[6].x = Coordinate::new(20.0000006).unwrap();
    assert_eq!(
        added(&s, Fix, &["pe000007"]),
        fixed(
            ("origin", "pe000007", "20.000001"),
            ("origin", "pe000007", "30")
        )
    );
}

#[test]
fn f18_fix_twice_adds_nothing_and_with_one_made_driven_adds_only_its_driving_twin() {
    // 14 applied, then Fix of pe000007 again: both its dimensions are there, at these values.
    let s = taken(&s5(), &edit(&s5(), Fix, &["pe000007"]));
    assert_eq!(
        click(&s, Fix, &["pe000007"]).unwrap(),
        Related {
            edit: None,
            already: vec![id("kt000101"), id("kt000102")],
            message: Some("point pe000007 is already fixed: nothing was added".to_owned()),
        }
    );
    // The distance_x made driven: a driving one isn't its repeat, and is added, only it.
    let mut s = s;
    if let Some(Form::DistanceX { driving, .. }) = s
        .constraints
        .iter_mut()
        .find(|c| c.id == id("kt000101"))
        .map(|c| &mut c.form)
    {
        *driving = false;
    }
    let related = click(&s, Fix, &["pe000007"]).unwrap();
    assert_eq!(
        related,
        Related {
            edit: Some(RelateEdit::Add(Members {
                constraints: vec![kt(
                    "kt000103",
                    Form::DistanceX {
                        a: Reference::Origin,
                        b: el("pe000007"),
                        value: Length::parse("20").unwrap(),
                        driving: true,
                    },
                )],
                ..Members::default()
            })),
            already: vec![id("kt000102")],
            message: Some(
                "1 of these constraints was already there: it wasn't added again".to_owned()
            ),
        }
    );
}

#[test]
fn f19_fix_of_a_line_or_with_a_curve_is_refused() {
    let s = s5();
    for picks in [&["ne000001"][..], &["pe000007", "ce000001"], &["x_axis"]] {
        assert_eq!(
            refused(&s, Fix, picks),
            ("Fix takes points".to_owned(), named(picks)),
            "{picks:?}"
        );
    }
}

#[test]
fn fix_of_two_points_is_each_points_pair_in_pick_order() {
    // pe000008 (−10, 30), then pe000007 (20, 30).
    let s = s5();
    let length = |v: &str| Length::parse(v).unwrap();
    assert_eq!(
        added(&s, Fix, &["pe000008", "pe000007"])
            .into_iter()
            .map(|c| c.form)
            .collect::<Vec<_>>(),
        vec![
            Form::DistanceX {
                a: el("pe000008"),
                b: Reference::Origin,
                value: length("10"),
                driving: true,
            },
            Form::DistanceY {
                a: Reference::Origin,
                b: el("pe000008"),
                value: length("30"),
                driving: true,
            },
            Form::DistanceX {
                a: Reference::Origin,
                b: el("pe000007"),
                value: length("20"),
                driving: true,
            },
            Form::DistanceY {
                a: Reference::Origin,
                b: el("pe000007"),
                value: length("30"),
                driving: true,
            },
        ]
    );
}

#[test]
fn fix_of_the_origin_alone_is_the_gates_to_refuse() {
    // Two dimensions of the origin from itself: the gate refuses a constraint naming only
    // the fixed references.
    let s = s5();
    let edit = edit(&s, Fix, &["origin"]);
    let refusals = gated(&s, &edit).unwrap_err();
    assert!(
        refusals.contains(
            &"this distance_x names only the sketch's fixed references, so it constrains nothing in the sketch"
                .to_owned()
        ),
        "{refusals:?}"
    );
}

#[test]
fn f20_a_repeat_either_way_round_adds_nothing() {
    // parallel {a: ne000003, b: ne000001} in the sketch, then Parallel on [ne000001,
    // ne000003].
    let mut s = s5();
    s.constraints.push(kt(
        "kt000004",
        Form::Parallel {
            a: el("ne000003"),
            b: el("ne000001"),
        },
    ));
    assert_eq!(
        click(&s, Parallel, &["ne000001", "ne000003"]).unwrap(),
        Related {
            edit: None,
            already: vec![id("kt000004")],
            message: Some(
                "lines ne000001 and ne000003 are already parallel: nothing was added".to_owned()
            ),
        }
    );
}

#[test]
fn either_way_round_holds_for_the_interchangeable_forms_only() {
    // horizontal_points, vertical_points, perpendicular, equal, and symmetric's a and b
    // swapped are repeats; a tangent_at's a and b swapped isn't (its direction is read along
    // a into b), nor a distance_x's.
    let mut s = s5();
    let held = [
        Form::HorizontalPoints {
            a: el("pe000005"),
            b: el("pe000007"),
        },
        Form::VerticalPoints {
            a: el("pe000005"),
            b: el("pe000007"),
        },
        Form::Perpendicular {
            a: el("ne000004"),
            b: el("ne000001"),
        },
        Form::Equal {
            a: el("ce000002"),
            b: el("ce000001"),
        },
        Form::Symmetric {
            a: el("pe000008"),
            b: el("pe000007"),
            about: el("ne000005"),
        },
        Form::TangentAt {
            a: el("ae000001"),
            b: el("ne000002"),
            at: el("pe000003"),
            direction: Direction::Same,
        },
    ];
    for (k, form) in held.into_iter().enumerate() {
        s.constraints.push(kt(&format!("kt00020{k}"), form));
    }
    for (tool, picks, held) in [
        (Horizontal, &["pe000007", "pe000005"][..], "kt000200"),
        (Vertical, &["pe000007", "pe000005"], "kt000201"),
        (Perpendicular, &["ne000001", "ne000004"], "kt000202"),
        (Equal, &["ce000001", "ce000002"], "kt000203"),
        (Symmetric, &["pe000007", "pe000008", "ne000005"], "kt000204"),
    ] {
        let related = click(&s, tool, picks).unwrap();
        assert_eq!(related.edit, None, "{tool:?}");
        assert_eq!(related.already, vec![id(held)], "{tool:?}");
    }
    // Tangent of [ne000002, ae000001] writes a: ne000002, b: ae000001: not the one held.
    let related = click(&s, Tangent, &["ne000002", "ae000001"]).unwrap();
    assert!(
        related.edit.is_some() && related.already.is_empty(),
        "{related:?}"
    );
}

#[test]
fn f21_a_partial_repeat_adds_the_rest_and_counts_one_left_out() {
    // horizontal {line: ne000001} in the sketch, then Horizontal on [ne000001, ne000003].
    let mut s = s5();
    s.constraints.push(kt(
        "kt000004",
        Form::Horizontal {
            line: el("ne000001"),
        },
    ));
    let related = click(&s, Horizontal, &["ne000001", "ne000003"]).unwrap();
    assert_eq!(
        related,
        Related {
            edit: Some(RelateEdit::Add(Members {
                constraints: vec![kt(
                    "kt000101",
                    Form::Horizontal {
                        line: el("ne000003"),
                    },
                )],
                ..Members::default()
            })),
            already: vec![id("kt000004")],
            message: Some(
                "1 of these constraints was already there: it wasn't added again".to_owned()
            ),
        }
    );
    // Two of three left out: counted.
    s.constraints.push(kt(
        "kt000005",
        Form::Horizontal {
            line: el("ne000003"),
        },
    ));
    let related = click(&s, Horizontal, &["ne000001", "ne000003", "ne000006"]).unwrap();
    assert_eq!(related.already, vec![id("kt000004"), id("kt000005")]);
    assert_eq!(
        related.message.as_deref(),
        Some("2 of these constraints were already there: they weren't added again")
    );
}

#[test]
fn f22_a_tangent_on_the_other_side_is_not_a_repeat() {
    // tangent {a: ne000002, b: ce000002, side: left} in the sketch; the drawing says right
    // (§7 4): the right one is added, to conflict.
    let mut s = s5();
    s.constraints.push(kt(
        "kt000004",
        Form::Tangent {
            a: el("ne000002"),
            b: el("ce000002"),
            side: TangentSide::Left,
        },
    ));
    assert_eq!(
        added(&s, Tangent, &["ne000002", "ce000002"]),
        vec![kt(
            "kt000101",
            Form::Tangent {
                a: el("ne000002"),
                b: el("ce000002"),
                side: TangentSide::Right,
            },
        )]
    );
}

#[test]
fn f23_a_constraint_selected_is_refused_by_each_tools_sentence() {
    let s = s5();
    for tool in RelationTool::ALL {
        for picks in [
            &["kt000001"][..],
            &["ne000001", "kt000001"],
            &["pe000007", "pe000008", "kt000002"],
        ] {
            assert_eq!(
                refused(&s, tool, picks),
                (tool.takes().to_owned(), named(picks)),
                "{tool:?} {picks:?}"
            );
        }
    }
}

#[test]
fn each_tools_sentence_is_the_decisions() {
    // The table in "The interaction, as decided".
    let want = [
        (Horizontal, "Horizontal takes lines, or two or more points"),
        (Vertical, "Vertical takes lines, or two or more points"),
        (
            Coincident,
            "Coincident takes points and the curves they lie on, or two or more points to make one",
        ),
        (
            Tangent,
            "Tangent takes two curves, at least one a circle or an arc",
        ),
        (
            Equal,
            "Equal takes two or more lines, or two or more circles and arcs",
        ),
        (Parallel, "Parallel takes two or more lines"),
        (Perpendicular, "Perpendicular takes two lines"),
        (Midpoint, "Midpoint takes a point and a line"),
        (
            Symmetric,
            "Symmetric takes two points and the line or point they mirror about, the point picked last",
        ),
        (Fix, "Fix takes points"),
    ];
    assert_eq!(want.map(|(tool, _)| tool), RelationTool::ALL);
    for (tool, sentence) in want {
        assert_eq!(tool.takes(), sentence);
    }
}

#[test]
fn nothing_selected_is_the_prototypes_words_for_every_tool() {
    let s = s5();
    for tool in RelationTool::ALL {
        let e = click(&s, tool, &[]).unwrap_err();
        assert_eq!(
            e.message(),
            "Select the curves or points first; the relation applies to them."
        );
        assert!(e.elements().is_empty());
    }
}

#[test]
fn horizontal_and_vertical_of_points_are_a_star_from_the_first_picked_the_origin_as_any() {
    // C8 and "written as picked": [origin, pe000007, pe000008] pairs each with the origin,
    // `a` the origin.
    let s = s5();
    assert_eq!(
        added(&s, Horizontal, &["origin", "pe000007", "pe000008"]),
        vec![
            kt(
                "kt000101",
                Form::HorizontalPoints {
                    a: Reference::Origin,
                    b: el("pe000007"),
                },
            ),
            kt(
                "kt000102",
                Form::HorizontalPoints {
                    a: Reference::Origin,
                    b: el("pe000008"),
                },
            ),
        ]
    );
    // One point alone: the sentence.
    assert_eq!(
        refused(&s, Vertical, &["pe000007"]).0,
        "Vertical takes lines, or two or more points"
    );
}

#[test]
fn coincident_of_points_and_curves_is_each_point_on_each_curve_points_first() {
    // [ne000003, pe000007, ce000001, origin]: points pe000007, origin; curves ne000003,
    // ce000001; each point (in pick order) on each curve (in pick order).
    let s = s5();
    let on = |p: &str, c: &str| Form::On {
        point: r(p),
        curve: r(c),
    };
    assert_eq!(
        added(
            &s,
            Coincident,
            &["ne000003", "pe000007", "ce000001", "origin"]
        )
        .into_iter()
        .map(|c| c.form)
        .collect::<Vec<_>>(),
        vec![
            on("pe000007", "ne000003"),
            on("pe000007", "ce000001"),
            on("origin", "ne000003"),
            on("origin", "ce000001"),
        ]
    );
    // Curves alone, or one point: the sentence.
    for picks in [&["ne000001", "ce000001"][..], &["pe000007"]] {
        assert_eq!(
            refused(&s, Coincident, picks).0,
            Coincident.takes(),
            "{picks:?}"
        );
    }
}

#[test]
fn a_merge_of_three_points_names_them_in_pick_order() {
    let s = s5();
    assert_eq!(
        edit(&s, Coincident, &["pe000007", "pe000008", "pe000005"]),
        RelateEdit::Merge(vec![id("pe000007"), id("pe000008"), id("pe000005")])
    );
}

#[test]
fn the_other_refusals_of_equal_parallel_and_perpendicular() {
    let s = s5();
    for (tool, picks) in [
        (Equal, &["ne000001"][..]),
        (Equal, &["ne000001", "pe000007"]),
        (Parallel, &["ne000001"]),
        (Perpendicular, &["ne000001"]),
        (Perpendicular, &["ne000001", "ce000001"]),
        (Midpoint, &["pe000007", "pe000008", "ne000001"]),
        (Midpoint, &["ne000001", "ne000003"]),
    ] {
        assert_eq!(
            refused(&s, tool, picks),
            (tool.takes().to_owned(), named(picks)),
            "{tool:?} {picks:?}"
        );
    }
    // Axes are lines to the tool: parallel to x_axis is written.
    assert_eq!(
        added(&s, Parallel, &["x_axis", "ne000001"]),
        vec![kt(
            "kt000101",
            Form::Parallel {
                a: Reference::XAxis,
                b: el("ne000001"),
            },
        )]
    );
}

#[test]
fn every_message_for_a_click_left_out_whole() {
    // Each tool's words when the sketch holds all it would write (C6).
    let mut s = s5();
    let held = [
        Form::On {
            point: el("pe000007"),
            curve: el("ne000003"),
        },
        Form::HorizontalPoints {
            a: el("pe000005"),
            b: Reference::Origin,
        },
        Form::VerticalPoints {
            a: el("pe000005"),
            b: Reference::Origin,
        },
        Form::Tangent {
            a: el("ne000002"),
            b: el("ce000002"),
            side: TangentSide::Right,
        },
        Form::Equal {
            a: el("ce000001"),
            b: el("ce000002"),
        },
        Form::Perpendicular {
            a: el("ne000001"),
            b: el("ne000004"),
        },
        Form::Midpoint {
            point: el("pe000007"),
            line: el("ne000006"),
        },
        Form::Symmetric {
            a: el("pe000007"),
            b: el("pe000008"),
            about: el("ne000005"),
        },
        Form::Horizontal {
            line: el("ne000003"),
        },
    ];
    for (k, form) in held.into_iter().enumerate() {
        s.constraints.push(kt(&format!("kt00030{k}"), form));
    }
    for (tool, picks, said) in [
        (
            Coincident,
            &["pe000007", "ne000003"][..],
            "point pe000007 is already on line ne000003",
        ),
        (
            Coincident,
            &["origin", "pe000005"],
            "point pe000005 is already at the origin",
        ),
        (
            Tangent,
            &["ne000002", "ce000002"],
            "line ne000002 and circle ce000002 are already tangent",
        ),
        (
            Equal,
            &["ce000002", "ce000001"],
            "circles ce000002 and ce000001 are already equal",
        ),
        (
            Perpendicular,
            &["ne000004", "ne000001"],
            "lines ne000004 and ne000001 are already perpendicular",
        ),
        (
            Midpoint,
            &["ne000006", "pe000007"],
            "point pe000007 is already the midpoint of line ne000006",
        ),
        (
            Symmetric,
            &["pe000008", "pe000007", "ne000005"],
            "point pe000008 and point pe000007 are already symmetric about line ne000005",
        ),
        (
            Horizontal,
            &["ne000003"],
            "line ne000003 is already horizontal",
        ),
    ] {
        let related = click(&s, tool, picks).unwrap();
        assert_eq!(related.edit, None, "{tool:?}");
        assert_eq!(
            related.message,
            Some(format!("{said}: nothing was added")),
            "{tool:?}"
        );
    }
}

#[test]
fn ids_come_from_the_source_none_the_sketch_holds() {
    // kt000101 held already: the next, kt000102, is taken for the first new constraint.
    let mut s = s5();
    s.constraints.push(kt(
        "kt000101",
        Form::Horizontal {
            line: el("ne000006"),
        },
    ));
    let constraints = added(&s, Parallel, &["ne000001", "ne000003"]);
    assert_eq!(constraints[0].id, id("kt000102"));
}

#[test]
fn a_member_the_read_doesnt_hold_is_refused_by_the_tools_sentence() {
    // A stale selection: no member zz000001.
    let s = s5();
    for tool in RelationTool::ALL {
        assert_eq!(
            refused(&s, tool, &["zz000001", "ne000001"]),
            (tool.takes().to_owned(), named(&["zz000001", "ne000001"])),
            "{tool:?}"
        );
    }
}

// --- Any tool, any selection ----------------------------------------------------------

mod property {
    use proptest::prelude::*;
    use proptest::test_runner::RngSeed;

    use super::*;

    /// The sketch, with a circle about a line's end and two arcs sharing a centre added, and
    /// every member of it, the origin, and the axes, to pick from.
    fn busy() -> (Sketch, Vec<Reference>) {
        let mut s = s5();
        s.circles.push(circle("ce000003", "pe000002", 3.0));
        s.points.push(point("pe000015", 80.0, 10.0));
        s.arcs
            .push(arc("ae000003", "pe000012", "pe000011", "pe000015"));
        s.constraints.push(kt(
            "kt000004",
            Form::On {
                point: el("pe000015"),
                curve: el("ne000007"),
            },
        ));
        let mut members: Vec<Reference> = s
            .elements()
            .map(|(id, _)| Reference::Element(id.clone()))
            .collect();
        members.extend([Reference::Origin, Reference::XAxis, Reference::YAxis]);
        (s, members)
    }

    /// The IDs `form` names.
    fn names(form: &Form) -> Vec<Reference> {
        form.references()
            .into_iter()
            .map(|(_, r, _)| r.clone())
            .collect()
    }

    /// Whether `id` is an end of a line or arc among `picks`.
    fn an_end_of(s: &Sketch, picks: &[Reference], id: &ElementId) -> bool {
        picks.iter().filter_map(Reference::element).any(|curve| {
            s.lines
                .iter()
                .any(|l| &l.id == curve && (&l.from == id || &l.to == id))
                || s.arcs
                    .iter()
                    .any(|a| &a.id == curve && (&a.start == id || &a.end == id))
        })
    }

    /// Whether every reference `form` holds is picked, or a picked curve's end it's
    /// tangent at, or Fix's origin.
    fn names_only_the_picks(
        s: &Sketch,
        tool: RelationTool,
        picks: &[Reference],
        form: &Form,
    ) -> bool {
        let at = match form {
            Form::TangentOn { at, .. } | Form::TangentAt { at, .. } => Some(at),
            _ => None,
        };
        names(form).iter().all(|r| {
            picks.contains(r)
                || (tool == RelationTool::Fix && *r == Reference::Origin)
                || (Some(r) == at && r.element().is_some_and(|id| an_end_of(s, picks, id)))
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 2000,
            failure_persistence: None,
            rng_seed: RngSeed::Fixed(0x5e1a7e),
            ..ProptestConfig::default()
        })]

        /// For every tool and any selection of up to three members, the origin and the axes
        /// among them, in any order: an edit or a refusal, never a panic; a refusal says
        /// something; nothing left out comes with no message; and an edit the gate takes names
        /// only the picked members (and a picked curve's end it's tangent at, and Fix's
        /// origin), removes only `on`s of what it adds, and merges only points picked.
        #[test]
        fn any_tool_on_any_selection_is_an_edit_or_a_refusal_naming_only_whats_picked(
            picks in {
                let (_, members) = busy();
                prop::sample::subsequence(members, 0..=3).prop_shuffle()
            },
        ) {
            let (s, _) = busy();
            for tool in RelationTool::ALL {
                let related = match relate(&s, tool, &picks, &mut Kt(100)) {
                    Err(e) => {
                        prop_assert!(!e.message().is_empty());
                        continue;
                    }
                    Ok(related) => related,
                };
                let Some(edit) = &related.edit else {
                    prop_assert!(!related.already.is_empty());
                    prop_assert!(related.message.is_some());
                    continue;
                };
                if gated(&s, edit).is_err() {
                    continue;
                }
                match edit {
                    RelateEdit::Merge(points) => {
                        prop_assert!(points.len() >= 2);
                        for p in points {
                            prop_assert!(picks.contains(&Reference::Element(p.clone())));
                        }
                    }
                    RelateEdit::Add(add) | RelateEdit::AddThenRemove { add, .. } => {
                        prop_assert!(!add.constraints.is_empty());
                        for c in &add.constraints {
                            prop_assert!(
                                names_only_the_picks(&s, tool, &picks, &c.form),
                                "{:?} {:?}: {:?}", tool, picks, c.form
                            );
                        }
                        if let RelateEdit::AddThenRemove { remove, .. } = edit {
                            let [made] = add.constraints.as_slice() else {
                                prop_assert!(false, "{:?}", add);
                                continue;
                            };
                            let implied = match &made.form {
                                Form::TangentOn { at, b, .. } => (at, b),
                                Form::Midpoint { point, line } => (point, line),
                                other => {
                                    prop_assert!(false, "{:?}", other);
                                    continue;
                                }
                            };
                            prop_assert!(!remove.is_empty());
                            for id in remove {
                                prop_assert_eq!(
                                    constraint_of(&s, id.as_str()),
                                    Some(Form::On {
                                        point: implied.0.clone(),
                                        curve: implied.1.clone(),
                                    })
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
