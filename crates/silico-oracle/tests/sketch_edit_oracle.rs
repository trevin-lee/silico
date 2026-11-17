//! The sketch-edit oracle (tests/oracle/sketch-edits/, held for the maintainer): every case
//! agrees with FreeCAD 1.1.3 modulo docs/ports/sketch-edit.md, or is one of the differences
//! reported for a decision; and the harness catches a real difference, a Silico result
//! changed by hand being reported, while a divergence the note states is excused only where
//! the note says. Expected values are worked by hand from the cases' drawings.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use silico_core::document::sketch_edit::Edited;
use silico_core::document::{
    Checked, ElementId, Form, Length, LineSide, Operation, Reference, Sketch, SketchConstraint,
    read,
};
use silico_core::sketch::{SketchWorker, Translation};
use silico_oracle::sketch::{FreecadGeometry, freecad_sketch};
use silico_oracle::sketch_edit::{
    self, Edit, EditCase, ExtendBy, FcConstraint, FcGeometry, FreecadEdit, FreecadEdited, KNOWN,
    Plan, Prepared, Refusal, Rule, SilicoSide, Theirs, Verdict, freecad_outcomes, plan, prepare,
    rule, silico_side, verdict, without_known,
};

const CASES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/oracle");

fn cases(names: &[&str]) -> Vec<EditCase> {
    let all = sketch_edit::load(Path::new(CASES)).unwrap();
    names
        .iter()
        .map(|n| {
            all.iter()
                .find(|c| c.name == *n)
                .unwrap_or_else(|| panic!("no case {n}"))
                .clone()
        })
        .collect()
}

/// The cases named, prepared, with FreeCAD's outcome of each (one run of FreeCAD).
fn both(names: &[&str]) -> (Vec<EditCase>, Vec<Prepared>, Vec<Option<Theirs>>) {
    let cases = cases(names);
    let mut worker = SketchWorker::start().unwrap();
    let prepared: Vec<Prepared> = cases
        .iter()
        .map(|c| prepare(c, &mut worker).unwrap())
        .collect();
    let theirs = freecad_outcomes(&prepared).unwrap();
    (cases, prepared, theirs)
}

/// `p` with Silico's edited sketch replaced by `change` of it, through the gate and solved
/// again.
fn changed(case: &EditCase, p: &Prepared, change: impl FnOnce(&mut Sketch)) -> Prepared {
    let SilicoSide::Edited(s) = &p.silico else {
        panic!("{}: Silico didn't edit it: {:?}", p.name, p.silico);
    };
    let mut edited: Edited = s.edited.clone();
    change(&mut edited.sketch);
    let mut worker = SketchWorker::start().unwrap();
    let silico = silico_side(&case.document, case.feature, edited, &mut worker).unwrap();
    assert!(
        matches!(silico, SilicoSide::Edited(_)),
        "the change made a sketch the gate refuses: {silico:?}"
    );
    Prepared {
        silico,
        ..p.clone()
    }
}

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn agrees(v: &Verdict) {
    assert!(v.how.is_empty(), "{v}");
}

#[test]
fn every_case_agrees_with_freecad_modulo_the_note_or_is_a_reported_difference() {
    let all = sketch_edit::load(Path::new(CASES)).unwrap();
    // Every case runs FreeCAD (#251's condition b): no plan is FreeCAD's refusal before it.
    let mut worker = SketchWorker::start().unwrap();
    for case in &all {
        let p = prepare(case, &mut worker).unwrap();
        assert!(
            matches!(p.plan, Plan::Run { .. }),
            "{}: {:?}",
            p.name,
            p.plan
        );
    }
    let verdicts = sketch_edit::run(&all).unwrap();
    assert_eq!(verdicts.len(), all.len());
    let (differ, shown, stale) = without_known(verdicts, KNOWN);
    let differ: Vec<String> = differ.iter().map(ToString::to_string).collect();
    assert!(differ.is_empty(), "{differ:#?}");
    assert!(stale.is_empty(), "{stale:#?}");
    // Every known difference of the case file shows (the sweep's are known by their names).
    let of_the_file = KNOWN
        .iter()
        .filter(|k| all.iter().any(|c| c.name == k.case))
        .count();
    assert_eq!(shown.len(), of_the_file, "{shown:#?}");
}

#[test]
fn a_merge_that_would_make_a_constraint_hold_its_own_curve_agrees_with_freecads_rollback() {
    // A merge after which an `on` would hold an arc's own centre on the arc, which Silico
    // refuses as the gate's rule for the merged sketch (#se-a-curve-s-own-point-constrained,
    // #se-the-merged-sketch-is-gated). FreeCAD 1.1.3 makes the merge, its solve failing (-1), which
    // its Coincident command rolls back (#se-any-result-whose-solve-is-worse): both refuse. (An
    // `on` held on its own curve's end is dropped as implied instead,
    // #se-an-on-the-merge-makes-hold.)
    let mut cases = vec![holds_own_centre()];
    // `--seed 43`'s case 6, a symmetric that would mirror an end of its line about it: the
    // rule too, though FreeCAD's answer there varies run to run (KNOWN).
    let mut sweep = silico_oracle::generate::sketch_edit_cases(43, 500).unwrap();
    cases.push(sweep.swap_remove(6));
    let mut worker = SketchWorker::start().unwrap();
    let prepared: Vec<Prepared> = cases
        .iter()
        .map(|c| prepare(c, &mut worker).unwrap())
        .collect();
    assert!(
        matches!(rule(&prepared[1]), Some(Rule::MergeHoldsOwn(id)) if id == "k0000032"),
        "{:?}",
        prepared[1].silico
    );
    let prepared = &prepared[..1];
    // A merge that would make a constraint name a point twice is its own rule, not this one.
    let (_, twice, _) = both(&["merge_with_constraint_between"]);
    assert!(
        matches!(rule(&twice[0]), Some(Rule::MergeNamesTwice(_))),
        "{:?}",
        twice[0].silico
    );
    let theirs = freecad_outcomes(prepared).unwrap();
    for (p, fc) in prepared.iter().zip(&theirs) {
        assert!(
            matches!(rule(p), Some(Rule::MergeHoldsOwn(_))),
            "{}: {:?}",
            p.name,
            p.silico
        );
        assert!(
            sketch_edit::rolled_back_worse(p, fc.as_ref()),
            "{}: {fc:?}",
            p.name
        );
        let v = verdict(p, fc.as_ref());
        agrees(&v);
        assert!(
            v.refused
                && v.modulo
                    .iter()
                    .any(|c| c == "se-a-curve-s-own-point-constrained")
                && v.modulo
                    .iter()
                    .any(|c| c == "se-the-merged-sketch-is-gated"),
            "{v:?}"
        );
        // FreeCAD otherwise: its solve no worse than before, its edit raising, a constraint
        // of the case's changed, its merge's coincidence missing, a geometry made: a
        // difference each.
        let changes: Vec<(&str, Doctoring)> = vec![
            (
                "its solve no worse",
                Box::new(|fc| fc.after = fc.before.clone()),
            ),
            (
                "a constraint of the case's made a reference",
                Box::new(|fc| {
                    let named = fc
                        .constraints
                        .iter_mut()
                        .find(|c| !c.name.is_empty())
                        .unwrap();
                    named.driving = false;
                }),
            ),
            (
                "the merge's coincidence missing",
                Box::new(|fc| {
                    fc.constraints.pop();
                }),
            ),
            (
                "a geometry made",
                Box::new(|fc| {
                    let mut made = fc.geometry[0].clone();
                    made.id = 999;
                    fc.geometry.push(made.clone());
                    fc.edited.push(made);
                }),
            ),
        ];
        for (what, change) in changes {
            let made = doctored(fc, |fc| change(fc));
            assert!(
                !sketch_edit::rolled_back_worse(p, Some(&made)),
                "{}: {what}",
                p.name
            );
            let v = verdict(p, Some(&made));
            assert!(!v.refused && !v.how.is_empty(), "{}: {what}: {v:?}", p.name);
        }
        let v = verdict(p, Some(&Theirs::Failed("refused by hand".to_owned())));
        assert!(!v.refused && v.how.len() == 1, "{}: {v:?}", p.name);
        // The same refusal naming a constraint that holds nothing of its own curve: not the
        // rule.
        let SilicoSide::Refused(r) = &p.silico else {
            panic!("{}: {:?}", p.name, p.silico);
        };
        let mut r = r.clone();
        r.elements[0] = "k0099999".to_owned();
        let none = Prepared {
            silico: SilicoSide::Refused(r.clone()),
            ..p.clone()
        };
        assert!(rule(&none).is_none(), "{}", p.name);
        // Naming a constraint of the sketch that holds nothing of its own curve (490's
        // horizontal, k0000009): not the rule.
        if let Some(other) = p.sketch.constraints.iter().find(|k| k.id == id("k0000009")) {
            r.elements[0] = other.id.to_string();
            let wrong = Prepared {
                silico: SilicoSide::Refused(r),
                ..p.clone()
            };
            assert!(rule(&wrong).is_none(), "{}", p.name);
        } else {
            assert_eq!(p.name, "merge_holds_own_centre");
        }
    }
}

#[test]
fn a_split_line_agrees_and_freecads_form_is_brought_to_decision_ones() {
    let (_, prepared, theirs) =
        both(&["split_level_line", "split_line_with_direction_constraints"]);
    for (p, fc) in prepared.iter().zip(&theirs) {
        let v = verdict(p, fc.as_ref());
        agrees(&v);
        assert!(v.excused.is_empty(), "{v:?}");
        // 1.1.3 joins line pieces with a Coincident, which becomes main's end-to-end
        // Tangent (#se-a-split-line-s-two-pieces), and copies the horizontal (and the perpendicular
        // and parallel) to the second piece, which come off it (#se-split-direction-constraints).
        assert_eq!(
            v.modulo,
            [
                "se-split-direction-constraints",
                "se-a-split-line-s-two-pieces"
            ],
            "{}",
            p.name
        );
    }
}

#[test]
fn a_dropped_constraint_in_silicos_result_is_reported_with_the_freedom_it_frees() {
    let (cases, prepared, theirs) = both(&["split_level_line"]);
    // The level line (0,0)-(10,0) split at x 4: FreeCAD's pieces n0000003 and the new one,
    // joined by an end-to-end Tangent (3 equations), the horizontal on n0000003 (1), the
    // length a distance of the outer ends (1), and the start's lock (2): 8 coordinates less
    // 7, 1 freedom. Without the horizontal, Silico's has 2.
    let p = changed(&cases[0], &prepared[0], |s| {
        s.constraints.retain(|c| c.id != id("k0000004"));
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert_eq!(
        v.how,
        [
            "FreeCAD has Horizontal(n0000003) (from k0000004), Silico hasn't",
            "degrees of freedom 2 against FreeCAD's 1",
        ],
        "{v}"
    );
}

#[test]
fn a_point_silico_puts_elsewhere_is_reported_before_and_after_the_solve() {
    let (cases, prepared, theirs) = both(&["split_level_line"]);
    // The split point, w0000000, moved from x 4 to 4.5: it's free to slide along the line,
    // so the solve leaves it there, and FreeCAD's at 4.
    let p = changed(&cases[0], &prepared[0], |s| {
        let point = s
            .points
            .iter_mut()
            .find(|p| p.id == id("w0000000"))
            .unwrap();
        point.x = silico_core::document::Coordinate::new(4.5).unwrap();
    });
    let v = verdict(&p, theirs[0].as_ref());
    let starts = |prefix: &str| v.how.iter().any(|h| h.starts_with(prefix));
    assert!(
        starts(
            "after the edit, before the solve, Silico's point w0000000 is at [4.5, 0.0], FreeCAD's n0000003's end at [4"
        ),
        "{v}"
    );
    assert!(
        starts(
            "after the edit, before the solve, Silico's point w0000000 is at [4.5, 0.0], FreeCAD's w0000001's start at [4"
        ),
        "{v}"
    );
    assert!(
        starts("point w0000000 at [4.5, 0.0], FreeCAD's at Some([4"),
        "{v}"
    );
}

#[test]
fn ends_freecad_joins_and_silico_keeps_apart_are_reported() {
    let (cases, prepared, theirs) = both(&["merge_touching_ends"]);
    agrees(&verdict(&prepared[0], theirs[0].as_ref()));
    // Silico's result replaced by the sketch unmerged: FreeCAD's coincidence joins n0000004's
    // end and n0000006's start, Silico's p0000002 and p0000003 (D2, #se-share-a-point); five points
    // (the loose one at (6, 5) the merge_apart case uses too) less the horizontal and the vertical
    // is 8 freedoms, and FreeCAD's coincidence takes 2 more.
    let original = cases[0].sketch().unwrap().clone();
    let p = changed(&cases[0], &prepared[0], |s| *s = original);
    let v = verdict(&p, theirs[0].as_ref());
    assert_eq!(
        v.how,
        [
            "FreeCAD joins n0000004's end, n0000006's start, which are Silico's points p0000002, p0000003 apart (docs/ports/sketch-edit.md#se-share-a-point)",
            "degrees of freedom 8 against FreeCAD's 6",
        ],
        "{v}"
    );
}

#[test]
fn a_piece_of_another_construction_is_reported() {
    let (cases, prepared, theirs) = both(&["trim_arc_between_cuts"]);
    agrees(&verdict(&prepared[0], theirs[0].as_ref()));
    // The arc a0000004 (0 to 270 degrees) trimmed between its cuts at 90 and 180: its
    // first piece made construction in Silico's result.
    let p = changed(&cases[0], &prepared[0], |s| {
        s.arcs
            .iter_mut()
            .find(|a| a.id == id("a0000004"))
            .unwrap()
            .construction = true;
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert_eq!(
        v.how,
        ["Silico's a0000004 is construction, FreeCAD's isn't"],
        "{v}"
    );
}

#[test]
fn only_a_split_construction_circles_pieces_are_excused_their_construction() {
    let (cases, prepared, theirs) = both(&[
        "split_construction_circle",
        "split_construction_line",
        "split_construction_arc",
        "split_construction_circle_beside_line",
        "split_circle_at_two_positions",
    ]);
    // 1.1.3's circle split drops construction from both arcs, where Silico's keep it
    // (#se-the-start-side-piece-keeps-the): excused, each piece by name, and compared modulo that
    // line, beside decision 3's joining of 1.1.3's two arcs (#se-splitting-a-circle).
    let excuse = |piece: &str| {
        format!(
            "Silico's {piece} is construction, FreeCAD's isn't: FreeCAD 1.1.3's split of a construction circle drops it, where Silico's pieces keep it (docs/ports/sketch-edit.md#se-the-start-side-piece-keeps-the)"
        )
    };
    for i in [0, 3] {
        let v = verdict(&prepared[i], theirs[i].as_ref());
        agrees(&v);
        assert_eq!(v.excused, [excuse("c0000002"), excuse("w0000002")], "{v:?}");
        assert_eq!(
            v.modulo,
            ["se-the-start-side-piece-keeps-the", "se-splitting-a-circle"],
            "{v:?}"
        );
    }
    // A line's and an arc's pieces keep it in 1.1.3 too: nothing to excuse.
    for i in [1, 2] {
        let v = verdict(&prepared[i], theirs[i].as_ref());
        agrees(&v);
        assert!(v.excused.is_empty(), "{v:?}");
        assert!(
            !v.modulo
                .iter()
                .any(|c| c == "se-the-start-side-piece-keeps-the"),
            "{v:?}"
        );
    }
    // Beside the circle, a normal line Silico's result made construction: not a piece of
    // the split, so reported, while the pieces stay excused.
    let p = changed(&cases[3], &prepared[3], |s| {
        s.lines
            .iter_mut()
            .find(|l| l.id == id("n0000005"))
            .unwrap()
            .construction = true;
    });
    let v = verdict(&p, theirs[3].as_ref());
    assert_eq!(
        v.how,
        ["Silico's n0000005 is construction, FreeCAD's isn't"],
        "{v}"
    );
    assert_eq!(v.excused, [excuse("c0000002"), excuse("w0000002")], "{v:?}");
    // FreeCAD's pieces of a construction line or arc made normal, as its circle's are:
    // reported, since only a circle's split drops it (#se-the-start-side-piece-keeps-the).
    for i in [1, 2] {
        let Some(Theirs::Edited(fc)) = &theirs[i] else {
            panic!("FreeCAD didn't split it: {:?}", theirs[i]);
        };
        let mut fc = fc.clone();
        for g in &mut fc.geometry {
            g.construction = false;
        }
        let v = verdict(&prepared[i], Some(&Theirs::Edited(fc)));
        assert_eq!(v.how.len(), 2, "{v}");
        assert!(
            v.how
                .iter()
                .all(|h| h.ends_with("is construction, FreeCAD's isn't")),
            "{v}"
        );
        assert!(v.excused.is_empty(), "{v:?}");
    }
    // The circle's pieces normal in Silico's result where FreeCAD's are construction: the
    // other way round from #se-the-start-side-piece-keeps-the, reported.
    let Some(Theirs::Edited(fc)) = &theirs[0] else {
        panic!("FreeCAD didn't split it: {:?}", theirs[0]);
    };
    let mut fc = fc.clone();
    for g in &mut fc.geometry {
        g.construction = true;
    }
    let p = changed(&cases[0], &prepared[0], |s| {
        for a in &mut s.arcs {
            a.construction = false;
        }
    });
    let v = verdict(&p, Some(&Theirs::Edited(fc)));
    assert!(v.excused.is_empty(), "{v:?}");
    assert!(
        !v.how.is_empty()
            && v.how
                .iter()
                .all(|h| h.ends_with("is not construction, FreeCAD's is")),
        "{v}"
    );
    // A normal circle's split, a piece made construction in Silico's result: the circle
    // wasn't construction, so nothing excuses it.
    let p = changed(&cases[4], &prepared[4], |s| {
        s.arcs[0].construction = true;
    });
    let v = verdict(&p, theirs[4].as_ref());
    assert!(v.excused.is_empty(), "{v:?}");
    assert_eq!(v.how.len(), 1, "{v}");
    assert!(
        v.how[0].ends_with("is construction, FreeCAD's isn't"),
        "{v}"
    );
    // A split arc's pieces made normal in Silico's result where FreeCAD's are construction:
    // the other way round, never excused.
    let p = changed(&cases[2], &prepared[2], |s| {
        for a in &mut s.arcs {
            a.construction = false;
        }
    });
    let v = verdict(&p, theirs[2].as_ref());
    assert_eq!(v.how.len(), 2, "{v}");
    assert!(
        v.how
            .iter()
            .all(|h| h.ends_with("is not construction, FreeCAD's is")),
        "{v}"
    );
    assert!(v.excused.is_empty(), "{v:?}");
}

#[test]
fn a_refusal_is_excused_only_where_the_note_gives_it() {
    let (_, prepared, theirs) = both(&[
        "split_line_at_freecads_position",
        "split_line_at_projection",
        "trim_arc_pick_outside_sweep",
    ]);
    // (2, 3.1) is |3.1 - 3| / √2 = 0.0707 mm off the line y = x + 1: Silico refuses it,
    // FreeCAD splits at the projection, and decision 4 (#se-decision-split-position) says so.
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    assert!(
        v.excused[0].starts_with("a split position 7.07106781")
            && v.excused[0]
                .contains("decision 4, docs/ports/sketch-edit.md#se-decision-split-position"),
        "{v:?}"
    );
    // The same refusal of a position on the line, (2.05, 3.05), far from its ends: nothing in
    // the note gives it, so it's a difference.
    let refused = Prepared {
        silico: SilicoSide::Refused("refused by hand".into()),
        ..prepared[1].clone()
    };
    let v = verdict(&refused, theirs[1].as_ref());
    assert!(v.excused.is_empty(), "{v:?}");
    assert_eq!(v.how.len(), 1, "{v}");
    assert!(
        v.how[0].starts_with(
            "Silico refused (its edit refused: refused by hand); FreeCAD made the edit (2 line,"
        ),
        "{v}"
    );
    // A pick at 315 degrees, outside the arc's 0 to 270: Silico places it at the nearer end
    // (#se-an-arc-s-pick-outside-its), and FreeCAD is given it there, so the results compare in
    // full.
    let v = verdict(&prepared[2], theirs[2].as_ref());
    agrees(&v);
    assert!(v.excused.is_empty(), "{v:?}");
    assert!(
        v.modulo.iter().any(|c| c == "se-an-arc-s-pick-outside-its"),
        "{v:?}"
    );
}

#[test]
fn a_cutters_end_freecad_joins_by_tolerance_is_compared_as_decision_five_has_it() {
    let (_, prepared, theirs) = both(&["trim_line_end_as_drawn", "trim_line_end_held"]);
    // Drawn only: FreeCAD's 5e-5 mm test joins the new end to the cutter's start, which
    // becomes a point on the cutter, as Silico's (#se-trim-s-the-cut-is-the).
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert_eq!(v.modulo, ["se-trim-s-the-cut-is-the"]);
    // Held by an on: FreeCAD's coincidence is Silico's shared point, as it is.
    let v = verdict(&prepared[1], theirs[1].as_ref());
    agrees(&v);
    assert!(v.modulo.is_empty(), "{v:?}");
}

#[test]
fn merges_freecads_coincident_command_would_refuse_agree_with_silicos_refusals() {
    let (_, prepared, theirs) = both(&[
        "merge_line_ends",
        "merge_with_constraint_between",
        "merge_touching_ends",
    ]);
    // A line's two ends (FreeCAD: a collapse, #se-a-coincidence-that-would-bring-two), and a
    // horizontal_points between the points (FreeCAD: a new redundancy, rolled back,
    // #se-any-result-whose-solve-is-worse): both refused on both sides.
    for (p, fc) in prepared.iter().zip(&theirs).take(2) {
        let v = verdict(p, fc.as_ref());
        assert!(v.refused && v.how.is_empty(), "{}: {v:?}", p.name);
    }
    // FreeCAD's merge refused where Silico's made it: a difference.
    let v = verdict(
        &prepared[2],
        Some(&Theirs::Failed("refused by hand".to_owned())),
    );
    assert_eq!(
        v.how,
        ["FreeCAD refused (its edit raised: refused by hand); Silico made the edit"],
        "{v}"
    );
}

/// The sketch of a document's one feature, written as `fields`.
fn sketch(fields: &str) -> Sketch {
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"s0000001\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S\", \"plane\": \"xy\", {fields}}}]}}"
    );
    let checked = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    match &checked.document().features[0].operation {
        Operation::Sketch(s) => s.clone(),
        other => panic!("{other:?}"),
    }
}

/// Lines n0000001 (0,0)-(5,0), n0000002 (5,0)-(5,5), n0000003 (5,0)-(10,0), meeting at
/// p0000002, which a distance_x from the origin holds.
fn three_way() -> Sketch {
    sketch(
        r#""points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 5, "y": 0, "construction": false}, {"id": "p0000003", "x": 5, "y": 5, "construction": false}, {"id": "p0000004", "x": 10, "y": 0, "construction": false}],
        "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false}, {"id": "n0000003", "from": "p0000002", "to": "p0000004", "construction": false}],
        "circles": [], "arcs": [],
        "constraints": [{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 5, "driving": true}]"#,
    )
}

fn planned(s: &Sketch, edit: Edit) -> Plan {
    let model = freecad_sketch(s, &Translation::new(s).unwrap()).unwrap();
    plan(s, &model, &edit)
}

#[test]
fn an_unjoins_shared_point_keeps_its_constraints_on_a_curve_that_keeps_it() {
    // FreeCAD's model has the lines as geometry 0, 1, 2; p0000002 is n0000001's end (0, 2),
    // n0000002's start (1, 1), and n0000003's start (2, 1), its distance_x on the first.
    // Unjoining n0000001 there, the distance_x sits on n0000002's start instead, as Silico's
    // stays with the point the others keep (#se-the-other-curves-keep-the-point); FreeCAD's vertex
    // Delete leaves an end's constraints on the curve it detaches
    // (#se-the-end-s-other-constraints-stay). The coincidences come first
    // (#se-the-result-depends-on-the-order).
    let Plan::Run { job, modulo } = planned(
        &three_way(),
        Edit::Unjoin {
            point: "p0000002".to_owned(),
            curve: "n0000001".to_owned(),
        },
    ) else {
        panic!("FreeCAD's plan refuses it");
    };
    assert_eq!(modulo, ["se-the-other-curves-keep-the-point"]);
    assert_eq!(
        job.edit,
        FreecadEdit::Unjoin {
            geometry: 0,
            position: 2
        }
    );
    let made: Vec<(&str, Vec<i32>, Option<&str>)> = job
        .constraints
        .iter()
        .map(|c| (c.kind, c.ints.clone(), c.name.as_deref()))
        .collect();
    assert_eq!(
        made,
        [
            ("Coincident", vec![0, 2, 1, 1], None),
            ("Coincident", vec![0, 2, 2, 1], None),
            ("DistanceX", vec![-1, 1, 1, 1], Some("s_k0000001")),
        ]
    );
}

#[test]
fn edits_freecad_has_no_counterpart_for_still_run_freecad() {
    let s = three_way();
    // Deleting a point curves end at: FreeCAD has no such edit (#se-freecad-has-no-delete-a-point),
    // so it's given its Delete on that vertex, the point's first place, n0000001's end
    // (#se-freecad-s-gui-delete-on-a to #se-on-any-curve-s-end-it).
    let Plan::Run { job, .. } = planned(
        &s,
        Edit::Remove {
            members: vec!["p0000002".to_owned()],
        },
    ) else {
        panic!("FreeCAD isn't run");
    };
    assert_eq!(
        job.edit,
        FreecadEdit::VertexDelete {
            geometry: 0,
            position: 2
        }
    );
    // A merge naming a point twice: FreeCAD is given the coincidence of n0000001's start with
    // itself, which its Coincident command refuses as already coincident
    // (#se-points-already-coincident-even-through-other).
    let Plan::Run { job, .. } = planned(
        &s,
        Edit::Merge {
            points: vec!["p0000001".to_owned(), "p0000001".to_owned()],
        },
    ) else {
        panic!("FreeCAD isn't run");
    };
    assert_eq!(
        job.edit,
        FreecadEdit::Merge {
            coincidences: vec![[0, 1, 0, 1]],
            placed: None,
        }
    );
    // Deleting a curve at the joint: geometry 1, the coincidences first
    // (#se-the-result-depends-on-the-order); the distance_x sits on n0000001's end, which stays, so
    // nothing moves.
    let Plan::Run { job, modulo } = planned(
        &s,
        Edit::Remove {
            members: vec!["n0000002".to_owned()],
        },
    ) else {
        panic!("FreeCAD's plan refuses it");
    };
    assert_eq!(modulo, ["se-the-result-depends-on-the-order"]);
    assert_eq!(
        job.edit,
        FreecadEdit::Delete {
            geometries: vec![1],
            constraints: Vec::new(),
            rejoin: Vec::new()
        }
    );
}

#[test]
fn a_known_difference_that_no_longer_shows_is_a_problem() {
    let known = [sketch_edit::Known {
        case: "c",
        shows: sketch_edit::Shows::Difference("it differs"),
        why: "because",
    }];
    let agreeing = Verdict {
        case: "c".to_owned(),
        ..Verdict::default()
    };
    let (differ, shown, stale) = without_known(vec![agreeing], &known);
    assert!(differ.is_empty() && shown.is_empty());
    assert_eq!(stale.len(), 1);
    // Another difference in a known case shows, as itself.
    let other = Verdict {
        case: "c".to_owned(),
        how: vec!["it differs".to_owned(), "and more".to_owned()],
        ..Verdict::default()
    };
    let (differ, shown, stale) = without_known(vec![other], &known);
    assert_eq!(differ.len(), 1);
    assert!(shown.is_empty());
    assert_eq!(stale.len(), 1);
}

#[test]
fn a_case_freecad_answers_otherwise_run_to_run_is_known_only_by_its_rule_on_a_sound_case() {
    let known = [sketch_edit::Known {
        case: "c",
        shows: sketch_edit::Shows::Varies(Rule::MergeSolveUnbuildable),
        why: "FreeCAD varies",
    }];
    // Agreeing or differing, Silico's side the rule's, it's shown as known, never a
    // difference, and never stale.
    for how in [Vec::new(), vec!["it differs".to_owned()]] {
        let v = Verdict {
            case: "c".to_owned(),
            how: how.clone(),
            rule: Some(Rule::MergeSolveUnbuildable),
            ..Verdict::default()
        };
        let (differ, shown, stale) = without_known(vec![v], &known);
        assert!(differ.is_empty() && stale.is_empty(), "{how:?}");
        assert_eq!(shown.len(), 1, "{how:?}");
        assert!(shown[0].contains("varies run to run"), "{shown:?}");
    }
    // Silico's side otherwise (a regression: it made the merge, or refused it for another
    // rule): compared, and the known case a problem.
    for rule in [None, Some(Rule::MergeSolveConflicts)] {
        let v = Verdict {
            case: "c".to_owned(),
            how: vec!["it differs".to_owned()],
            rule: rule.clone(),
            ..Verdict::default()
        };
        let (differ, shown, stale) = without_known(vec![v], &known);
        assert_eq!(differ.len(), 1, "{rule:?}");
        assert!(shown.is_empty(), "{rule:?}");
        assert_eq!(stale.len(), 1, "{rule:?}");
    }
    // The rule's, but the case itself with a problem: compared, and a problem.
    let v = Verdict {
        case: "c".to_owned(),
        how: vec!["it differs".to_owned()],
        rule: Some(Rule::MergeSolveUnbuildable),
        problem: true,
        ..Verdict::default()
    };
    let (differ, shown, stale) = without_known(vec![v], &known);
    assert!(differ.len() == 1 && shown.is_empty() && stale.len() == 1);
    // Another case is compared as ever.
    let other = Verdict {
        case: "d".to_owned(),
        how: vec!["it differs".to_owned()],
        ..Verdict::default()
    };
    let (differ, _, _) = without_known(vec![other], &known);
    assert_eq!(differ.len(), 1);
}

#[test]
fn a_merge_silico_makes_where_freecads_varies_hides_only_freecads_rollback() {
    let known = [sketch_edit::Known {
        case: "c",
        shows: sketch_edit::Shows::VariesMade,
        why: "FreeCAD varies",
    }];
    let rolled = "FreeCAD refused (its solve failed); Silico made the edit".to_owned();
    // FreeCAD's merge failing and rolled back, Silico making it: shown as known, never a
    // difference, and never stale.
    let v = Verdict {
        case: "c".to_owned(),
        how: vec![rolled.clone()],
        made: true,
        rolled_back: true,
        ..Verdict::default()
    };
    let (differ, shown, stale) = without_known(vec![v], &known);
    assert!(
        differ.is_empty() && stale.is_empty(),
        "{differ:?} {stale:?}"
    );
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(shown[0].contains("varies run to run"), "{shown:?}");
    // FreeCAD making the merge too: compared as any case. Agreeing, nothing shows; differing
    // (another degree of freedom, a constraint re-pointed), it's a difference, and the entry
    // isn't stale, as FreeCAD's answer still varies.
    let v = Verdict {
        case: "c".to_owned(),
        made: true,
        ..Verdict::default()
    };
    let (differ, shown, stale) = without_known(vec![v], &known);
    assert!(differ.is_empty() && shown.is_empty() && stale.is_empty());
    let v = Verdict {
        case: "c".to_owned(),
        how: vec!["degrees of freedom: Silico 7, FreeCAD 8".to_owned()],
        made: true,
        ..Verdict::default()
    };
    let (differ, shown, stale) = without_known(vec![v], &known);
    assert_eq!(differ.len(), 1, "{differ:?}");
    assert!(shown.is_empty() && stale.is_empty(), "{shown:?} {stale:?}");
    // Silico refusing it (by a rule or none), or the case with a problem of its own though
    // Silico made the edit, FreeCAD's merge rolled back or not: compared, and the known case
    // a problem.
    for rolled_back in [false, true] {
        for (rule, made, problem) in [
            (None, false, false),
            (Some(Rule::MergeSolveFails), false, false),
            (None, true, true),
        ] {
            let what = format!("{rule:?} made {made} problem {problem} rolled back {rolled_back}");
            let v = Verdict {
                case: "c".to_owned(),
                how: vec![rolled.clone()],
                rule: rule.clone(),
                made,
                problem,
                rolled_back,
                ..Verdict::default()
            };
            let (differ, shown, stale) = without_known(vec![v], &known);
            assert_eq!(differ.len(), 1, "{what}");
            assert!(shown.is_empty(), "{what}");
            assert_eq!(stale.len(), 1, "{what}");
            assert!(stale[0].contains("no longer makes the edit"), "{stale:?}");
        }
    }
    // A refusal agreeing with FreeCAD's, Silico not making the edit: still a problem.
    let v = Verdict {
        case: "c".to_owned(),
        refused: true,
        rolled_back: true,
        ..Verdict::default()
    };
    let (differ, shown, stale) = without_known(vec![v], &known);
    assert!(differ.is_empty() && shown.is_empty() && stale.len() == 1);
}

#[test]
fn the_known_merge_freecad_varies_on_is_the_one_its_entry_says_and_a_crash_there_is_compared() {
    // KNOWN's edit_78_52, as seed 78 makes it, on Silico's side alone: if the generator or the
    // merge changes, its entry is about another case and must be redone. The arc a0000016
    // (centre p0000014 at (31.807, -12.943), start p0000011 at (31.807, -16.418): radius
    // 3.475) has its end p0000015 merged onto the circle c0000026's centre p0000025, 31 mm
    // away; Silico places the end there, 27.75 mm from the arc's centre, and its solve from
    // there is clean, the arc's radius 22.05.
    let mut sweep = silico_oracle::generate::sketch_edit_cases(78, 500).unwrap();
    let case = sweep.swap_remove(52);
    assert_eq!(case.name, "edit_78_52");
    assert_eq!(
        case.edit,
        Edit::Merge {
            points: vec!["p0000025".to_owned(), "p0000015".to_owned()],
        }
    );
    let mut worker = SketchWorker::start().unwrap();
    let p = prepare(&case, &mut worker).unwrap();
    assert!(p.problems.is_empty(), "{:?}", p.problems);
    assert_eq!(rule(&p), None, "{:?}", p.silico);
    let SilicoSide::Edited(s) = &p.silico else {
        panic!("Silico didn't make the merge: {:?}", p.silico);
    };
    assert!(
        s.ours.conflicting.is_empty()
            && s.ours.redundant.is_empty()
            && s.ours.partially_redundant.is_empty(),
        "{:?}",
        s.ours
    );
    let solved = s.ours.outcome.as_ref().unwrap();
    let at = |point: &str| solved.point(&id(point)).unwrap();
    let from = |a: [f64; 2], b: [f64; 2]| libm::hypot(b[0] - a[0], b[1] - a[1]);
    let (centre, start, end) = (at("p0000014"), at("p0000011"), at("p0000025"));
    assert!(
        (from(centre, start) - 22.05).abs() < 0.005,
        "{centre:?} {start:?}"
    );
    assert!(
        (from(centre, end) - from(centre, start)).abs() < 1e-6,
        "{centre:?} {start:?} {end:?}"
    );
    // A FreeCAD crash, hang, or raise there is no rollback: compared, a difference, never
    // shown as known, and the entry not stale, as Silico still makes the merge.
    for theirs in [
        Theirs::Lost("FreeCAD crashed".to_owned()),
        Theirs::Failed("RuntimeError: doctored".to_owned()),
    ] {
        let v = verdict(&p, Some(&theirs));
        assert!(v.made && !v.rolled_back, "{v:?}");
        let (differ, shown, stale) = without_known(vec![v], KNOWN);
        assert_eq!(differ.len(), 1, "{theirs:?}");
        assert!(shown.is_empty() && stale.is_empty(), "{shown:?} {stale:?}");
    }
    // Its merge's solve failing: Silico refuses it, so it made no edit, and the entry is stale.
    let mut failed = s.clone();
    failed.ours.outcome = Err(silico_core::Error::new(
        silico_core::ErrorCode::SolveFailed,
        "doctored",
    ));
    let failed = Prepared {
        silico: SilicoSide::Edited(failed),
        ..p.clone()
    };
    let v = verdict(&failed, Some(&Theirs::Lost("FreeCAD crashed".to_owned())));
    assert!(!v.made, "{v:?}");
    let (_, shown, stale) = without_known(vec![v], KNOWN);
    assert!(shown.is_empty(), "{shown:?}");
    assert!(
        stale.len() == 1 && stale[0].starts_with("edit_78_52: "),
        "{stale:?}"
    );
}

#[test]
fn the_known_merge_silico_refuses_as_holding_its_own_curve_is_the_one_its_entry_says() {
    // KNOWN's edit_58_356, as seed 58 makes it: the symmetric k0000019 would mirror an end of
    // n0000005 about n0000005 itself, which Silico refuses by MergeHoldsOwn.
    let mut sweep = silico_oracle::generate::sketch_edit_cases(58, 500).unwrap();
    let case = sweep.swap_remove(356);
    assert_eq!(case.name, "edit_58_356");
    let mut worker = SketchWorker::start().unwrap();
    let p = prepare(&case, &mut worker).unwrap();
    assert!(p.problems.is_empty(), "{:?}", p.problems);
    assert!(
        matches!(rule(&p), Some(Rule::MergeHoldsOwn(id)) if id == "k0000019"),
        "{:?}",
        p.silico
    );
}

#[test]
fn an_arc_extend_whose_solve_fails_is_no_edit_made() {
    // `sketch-extend-sweep --seed 43 --count 500`'s case 212, whose solve fails from the
    // written end (ExtendSolveFails, #se-an-arc-swept-a-whole-turn): Silico refuses it, so a
    // VariesMade entry for it is stale whatever FreeCAD answers.
    let mut sweep = silico_oracle::generate::sketch_extend_cases(43, 500).unwrap();
    let case = sweep.swap_remove(212);
    assert_eq!(case.name, "extend_43_212");
    let mut worker = SketchWorker::start().unwrap();
    let p = prepare(&case, &mut worker).unwrap();
    assert_eq!(rule(&p), Some(Rule::ExtendSolveFails), "{:?}", p.silico);
    assert!(matches!(&p.silico, SilicoSide::Edited(s) if s.ours.outcome.is_err()));
    let known = [sketch_edit::Known {
        case: "extend_43_212",
        shows: sketch_edit::Shows::VariesMade,
        why: "doctored",
    }];
    for theirs in [None, Some(Theirs::Lost("FreeCAD crashed".to_owned()))] {
        let v = verdict(&p, theirs.as_ref());
        assert!(!v.made, "{v:?}");
        let (_, shown, stale) = without_known(vec![v], &known);
        assert!(shown.is_empty() && stale.len() == 1, "{shown:?} {stale:?}");
    }
}

#[test]
fn a_generated_sweep_is_the_same_for_a_seed_and_every_sketch_is_drawn_as_it_solves() {
    let first = silico_oracle::generate::sketch_edit_cases(7, 30).unwrap();
    let again = silico_oracle::generate::sketch_edit_cases(7, 30).unwrap();
    for (a, b) in first.iter().zip(&again) {
        assert_eq!(a.edit, b.edit);
        assert!(a.sketch().unwrap().canonically_eq(b.sketch().unwrap()));
    }
    let mut worker = SketchWorker::start().unwrap();
    for case in &first {
        let p = prepare(case, &mut worker).unwrap();
        assert!(p.problems.is_empty(), "{}: {:?}", case.name, p.problems);
    }
}

// ---- The port review's proofs (#251), each a way a difference was read as agreement ----

#[test]
fn a_result_the_gate_refuses_is_a_difference_whatever_freecad_does() {
    // Every edit's result has to pass the gate (#se-every-edit-s-result-has-to,
    // #se-split-refusals-as-data): a gated result is Silico's bug, even where FreeCAD refuses too
    // (it fails in OCCT, or throws).
    let (_, prepared, theirs) = both(&[
        "split_line_at_end",
        "split_level_line_within_1e-8_of_start",
        "split_line_beyond_end",
        "trim_at_centre",
    ]);
    for (p, fc) in prepared.iter().zip(&theirs) {
        let gated = Prepared {
            silico: SilicoSide::Gated(vec!["a line of length 0 (the gate)".to_owned()]),
            ..p.clone()
        };
        let v = verdict(&gated, fc.as_ref());
        assert!(
            v.how.iter().any(|h| h.contains("the gate refuses")),
            "{}: {v:?}",
            p.name
        );
        assert!(!v.refused, "{v:?}");
    }
}

#[test]
fn a_refusal_for_another_reason_than_the_notes_is_a_difference() {
    // The note excuses these refusals for a reason (decision 2's constraint, a split's or a
    // merge's own): a refusal for any other is a difference, however FreeCAD ends; and so is
    // one of an edit Silico makes (the merge that makes an edge tangent a tangent_at).
    // Every excusal path, and every refusal both sides make.
    let (_, prepared, theirs) = both(&[
        "trim_symmetric_about_line_into_two",
        "trim_arc_tangent_no_piece_touches",
        "merge_leaves_edge_tangent",
        "split_line_at_end",
        "split_line_at_freecads_position",
        "split_circle_at_one_position",
        "split_level_line_within_1e-8_of_start",
        "split_line_beyond_end",
        "split_point",
        "trim_point",
        "trim_at_centre",
        "unjoin_unshared_point",
        "remove_point_curves_use",
        "merge_line_ends",
        "merge_named_twice",
        "merge_collapses_through_chain",
        "merge_with_constraint_between",
    ]);
    for (p, fc) in prepared.iter().zip(&theirs) {
        let wrong = Prepared {
            silico: SilicoSide::Refused("internal: an unrelated bug's refusal".into()),
            ..p.clone()
        };
        let v = verdict(&wrong, fc.as_ref());
        assert!(!v.how.is_empty(), "{}: {v:?}", p.name);
        assert!(v.excused.is_empty(), "{}: {v:?}", p.name);
    }
}

/// FreeCAD's result of `theirs` with `change` made to it.
fn doctored(theirs: &Option<Theirs>, change: impl FnOnce(&mut FreecadEdited)) -> Theirs {
    let Some(Theirs::Edited(fc)) = theirs else {
        panic!("FreeCAD didn't make the edit: {theirs:?}");
    };
    let mut fc = fc.clone();
    change(&mut fc);
    Theirs::Edited(fc)
}

#[test]
fn a_merge_the_gate_refuses_is_the_notes_where_its_placement_collapses_a_line() {
    // merge_apart_onto_a_lines_other_end: p0000004, at the line's start, named first, so the
    // line's end goes there and the line is 0 mm long: the gate refuses the merge
    // (#se-the-merged-sketch-is-gated). FreeCAD's Coincident command rolls back its solve, so both
    // refuse.
    let (_, prepared, theirs) = both(&["merge_apart_onto_a_lines_other_end", "merge_line_ends"]);
    // A merge of a line's own ends is the collapse rule's (#se-a-merge-after-which-a-curve), not
    // this one's, though it names the line last too.
    assert_eq!(
        rule(&prepared[1]),
        Some(Rule::MergeCollapses),
        "{:?}",
        prepared[1].silico
    );
    let p = &prepared[0];
    assert!(
        matches!(rule(p), Some(Rule::MergeGated(c)) if c == "n0000003"),
        "{:?}",
        p.silico
    );
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert!(
        v.refused && v.modulo == ["se-the-merged-sketch-is-gated"],
        "{v:?}"
    );
    let SilicoSide::Refused(r) = &p.silico else {
        panic!("Silico made the merge: {:?}", p.silico);
    };
    // Named in another order, or naming a curve the placement doesn't collapse: not the rule.
    for change in [
        |r: &mut Refusal| r.elements.swap(0, 1),
        |r: &mut Refusal| *r.elements.last_mut().unwrap() = "p0000001".to_owned(),
    ] {
        let mut r = r.clone();
        change(&mut r);
        let wrong = Prepared {
            silico: SilicoSide::Refused(r),
            ..p.clone()
        };
        assert!(rule(&wrong).is_none(), "{:?}", wrong.silico);
    }
    // The line's end named first, so the merged point stays at (10, 0) and the line keeps its
    // length: the same refusal isn't the note's.
    let mut r = r.clone();
    r.elements.swap(0, 1);
    let kept = Prepared {
        edit: Edit::Merge {
            points: vec!["p0000002".to_owned(), "p0000004".to_owned()],
        },
        silico: SilicoSide::Refused(r),
        ..p.clone()
    };
    assert!(rule(&kept).is_none(), "{:?}", kept.silico);
    // FreeCAD's coincidences solving, its line kept 5 mm long: Silico's refusal excused; its
    // line also 0 mm long: a difference.
    for (end, excused) in [([5.0, 0.0], true), ([0.0, 0.0], false)] {
        let made = doctored(&theirs[0], |fc| {
            fc.after.code = 0;
            fc.after.redundant.clear();
            let i = p
                .model
                .elements
                .iter()
                .position(|e| e == "n0000003")
                .unwrap();
            let id = fc.ids_before[i];
            let line = fc.geometry.iter_mut().find(|g| g.id == id).unwrap();
            *line.points.get_mut("1").unwrap() = [0.0, 0.0];
            *line.points.get_mut("2").unwrap() = end;
        });
        let v = verdict(p, Some(&made));
        assert_eq!(!v.excused.is_empty(), excused, "{end:?}: {v:?}");
        assert_eq!(v.how.is_empty(), excused, "{end:?}: {v:?}");
    }
}

#[test]
fn a_merge_from_apart_still_compares_the_points_it_doesnt_merge() {
    // merge_apart, Silico's result with p0000001 (the first line's start, which the merge
    // doesn't touch) moved 1 mm: reported.
    let (cases, prepared, theirs) = both(&["merge_apart"]);
    let p = changed(&cases[0], &prepared[0], |s| {
        let pt = s
            .points
            .iter_mut()
            .find(|p| p.id == id("p0000001"))
            .unwrap();
        pt.x = silico_core::document::Coordinate::new(-1.0).unwrap();
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert!(v.how.iter().any(|h| h.contains("p0000001")), "{v:?}");
}

#[test]
fn a_pick_outside_the_sweep_compares_the_result() {
    // trim_arc_pick_outside_sweep, Silico's result with its arcs made construction: FreeCAD
    // is given the pick where Silico places it (#se-an-arc-s-pick-outside-its), so the results
    // compare, and this shows.
    let (cases, prepared, theirs) = both(&["trim_arc_pick_outside_sweep"]);
    let p = changed(&cases[0], &prepared[0], |s| {
        for a in &mut s.arcs {
            a.construction = true;
        }
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert!(v.how.iter().any(|h| h.contains("construction")), "{v:?}");
}

#[test]
fn a_refusal_naming_another_constraint_than_the_one_no_piece_takes_is_a_difference() {
    // trim_symmetric_about_line_into_two: Silico's refusal as the rule's in all but the
    // constraint it names (k0000012 is the symmetric; k0000099 is none of the sketch's).
    let (_, prepared, theirs) = both(&["trim_symmetric_about_line_into_two"]);
    let SilicoSide::Refused(right) = &prepared[0].silico else {
        panic!("{:?}", prepared[0].silico);
    };
    assert_eq!(right.elements[0], "k0000012");
    agrees(&verdict(&prepared[0], theirs[0].as_ref()));
    let mut wrong = right.clone();
    wrong.elements[0] = "k0000099".to_owned();
    let v = verdict(
        &Prepared {
            silico: SilicoSide::Refused(wrong),
            ..prepared[0].clone()
        },
        theirs[0].as_ref(),
    );
    assert!(v.excused.is_empty() && v.how.len() == 1, "{v:?}");
}

#[test]
fn a_merge_not_from_apart_reports_a_point_it_doesnt_merge_moved() {
    // merge_touching_ends, Silico's result with p0000001 (the first line's start) moved:
    // the points merged are at one place, so every point is compared.
    let (cases, prepared, theirs) = both(&["merge_touching_ends"]);
    let p = changed(&cases[0], &prepared[0], |s| {
        let pt = s
            .points
            .iter_mut()
            .find(|p| p.id == id("p0000001"))
            .unwrap();
        pt.x = silico_core::document::Coordinate::new(-1.0).unwrap();
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert!(v.excused.is_empty(), "{v:?}");
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "after the edit, before the solve, Silico's point p0000001 is at [-1.0, 0.0]"
        )),
        "{v:?}"
    );
}

#[test]
fn the_harnesss_own_failure_is_never_freecads_refusal() {
    let (_, prepared, _) = both(&["split_line_at_end"]);
    let v = verdict(
        &prepared[0],
        Some(&Theirs::Harness("KeyError: 'geometry'".to_owned())),
    );
    assert!(!v.refused, "{v:?}");
    assert_eq!(
        v.how,
        ["the harness's FreeCAD side failed, not FreeCAD's edit: KeyError: 'geometry'"]
    );
}

// ---- Every case pins FreeCAD's outcome (#251's condition b) ----

/// A change made to FreeCAD's result of a case, given the case.
type CaseDoctor = fn(&Prepared, &mut FreecadEdited);
/// A change to FreeCAD's result.
type Doctor = fn(&mut FreecadEdited);
/// A change made to FreeCAD's result that captures what it sets.
type Doctoring = Box<dyn Fn(&mut FreecadEdited)>;

/// `v` reports Silico's refusal against FreeCAD's edit (`what` made of it), and excuses
/// nothing.
fn refused_against_freecads_edit(v: &Verdict, what: &str) {
    assert!(v.excused.is_empty(), "{what}: {v:?}");
    assert_eq!(v.how.len(), 1, "{what}: {v}");
    assert!(
        v.how[0].starts_with("Silico refused (") && v.how[0].contains("; FreeCAD made the edit ("),
        "{what}: {v}"
    );
}

/// The FreeCAD geometry of `p`'s element `element`, after the edit (before the last solve).
fn edited_geometry<'a>(
    p: &Prepared,
    fc: &'a mut FreecadEdited,
    element: &str,
) -> &'a mut FcGeometry {
    let i = p.model.elements.iter().position(|e| e == element).unwrap();
    let id = fc.ids_before[i];
    fc.edited.iter_mut().find(|g| g.id == id).unwrap()
}

#[test]
fn a_circle_split_at_one_position_is_excused_only_where_freecad_opens_it_into_one_arc() {
    // split_circle_at_one_position: the circle about (1, 2), radius 3, split at (2, 3.1).
    // 1.1.3 makes one arc in its place, its ends at one place, not joined
    // (#se-a-circle-gives-one-arc-from): excused.
    let (_, prepared, theirs) = both(&["split_circle_at_one_position"]);
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::SplitCircleAtOne));
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    // FreeCAD's arc otherwise than the note says: each a difference.
    let changes: [(&str, CaseDoctor); 7] = [
        ("opened 10 degrees on", |p, fc| {
            let arc = edited_geometry(p, fc, "c0000002");
            let c = arc.points["3"];
            let (sin, cos) = (libm::sin(10f64.to_radians()), libm::cos(10f64.to_radians()));
            for k in ["1", "2"] {
                let e = arc.points.get_mut(k).unwrap();
                let (dx, dy) = (e[0] - c[0], e[1] - c[1]);
                *e = [c[0] + dx * cos - dy * sin, c[1] + dx * sin + dy * cos];
            }
        }),
        ("its ends joined", |_, fc| {
            fc.constraints.push(FcConstraint {
                kind: "Coincident".to_owned(),
                refs: vec![[0, 1], [0, 2]],
                value: 0.0,
                driving: true,
                name: String::new(),
            });
        }),
        ("its end 1 mm on", |p, fc| {
            let arc = edited_geometry(p, fc, "c0000002");
            arc.points.get_mut("2").unwrap()[0] += 1.0;
        }),
        ("its centre moved", |p, fc| {
            let arc = edited_geometry(p, fc, "c0000002");
            arc.points.get_mut("3").unwrap()[1] += 1.0;
        }),
        ("its radius 4", |p, fc| {
            edited_geometry(p, fc, "c0000002").radius = Some(4.0);
        }),
        ("still a circle", |p, fc| {
            edited_geometry(p, fc, "c0000002").kind = "circle".to_owned();
        }),
        ("a second arc made", |p, fc| {
            let mut arc = edited_geometry(p, fc, "c0000002").clone();
            arc.id = 99;
            fc.edited.push(arc.clone());
            fc.geometry.push(arc);
        }),
    ];
    for (what, change) in changes {
        let made = doctored(&theirs[0], |fc| change(p, fc));
        refused_against_freecads_edit(&verdict(p, Some(&made)), what);
    }
}

#[test]
fn a_split_off_the_curve_is_excused_only_where_freecads_pieces_meet_at_its_projection() {
    // (2, 3.1) is off the line y = x + 1, from (1, 2) to (3, 4): its projection is
    // (2.05, 3.05). (-2, 3.1) is off the arc about (1, 2), radius 3: its projection is
    // (1, 2) + 3 (-3, 1.1) / |(-3, 1.1)| = (-1.81663, 3.03276).
    let (_, prepared, theirs) = both(&[
        "split_line_at_freecads_position",
        "split_arc_at_freecads_position",
    ]);
    let given = [[2.0, 3.1], [-2.0, 3.1]];
    let projected = [
        [2.05, 3.05],
        [1.0 - 9.0 / 10.21_f64.sqrt(), 2.0 + 3.3 / 10.21_f64.sqrt()],
    ];
    for (((p, fc), at), projected) in prepared.iter().zip(&theirs).zip(given).zip(projected) {
        assert!(
            matches!(rule(p), Some(Rule::SplitOffCurve(_))),
            "{}",
            p.name
        );
        let v = verdict(p, fc.as_ref());
        agrees(&v);
        assert_eq!(v.excused.len(), 1, "{v:?}");
        // The joint where each piece meets the other: at the projection, by hand.
        let Some(Theirs::Edited(edited)) = fc else {
            panic!("FreeCAD didn't split {}", p.name);
        };
        let kept = edited.edited[0].points["2"];
        assert!(
            (kept[0] - projected[0]).abs() < 1e-12 && (kept[1] - projected[1]).abs() < 1e-12,
            "{}: {kept:?}",
            p.name
        );
        // FreeCAD's pieces meeting at the position as given, not its projection; meeting
        // nowhere; one piece only; or pieces of another kind: each a difference.
        let changes: [(&str, Doctoring); 8] = [
            (
                "the kept piece's start moved",
                Box::new(|fc| {
                    fc.edited[0].points.get_mut("1").unwrap()[1] += 1e-6;
                }),
            ),
            (
                "the new piece's end moved",
                Box::new(|fc| {
                    fc.edited[1].points.get_mut("2").unwrap()[0] -= 1e-6;
                }),
            ),
            (
                "a piece about another centre",
                Box::new(|fc| {
                    if let Some(c) = fc.edited[1].points.get_mut("3") {
                        c[0] += 1e-6;
                    } else {
                        // A line has none: move its start off the joint instead.
                        fc.edited[1].points.get_mut("1").unwrap()[1] += 1e-6;
                    }
                }),
            ),
            (
                "a piece of another radius",
                Box::new(|fc| {
                    if let Some(r) = &mut fc.edited[1].radius {
                        *r += 1e-6;
                    } else {
                        // A line has none: move its start off the joint instead.
                        fc.edited[1].points.get_mut("1").unwrap()[1] += 1e-6;
                    }
                }),
            ),
            (
                "split at the position given",
                Box::new(move |fc| {
                    *fc.edited[0].points.get_mut("2").unwrap() = at;
                    *fc.edited[1].points.get_mut("1").unwrap() = at;
                }),
            ),
            (
                "the new piece's start moved",
                Box::new(|fc| {
                    fc.edited[1].points.get_mut("1").unwrap()[0] += 1e-6;
                }),
            ),
            (
                "no new piece",
                Box::new(|fc| {
                    fc.edited.truncate(1);
                    fc.geometry.truncate(1);
                }),
            ),
            (
                "pieces of another kind",
                Box::new(|fc| {
                    for g in &mut fc.edited {
                        g.kind = "bspline".to_owned();
                    }
                }),
            ),
        ];
        for (what, change) in changes {
            let made = doctored(fc, |fc| change(fc));
            refused_against_freecads_edit(&verdict(p, Some(&made)), what);
        }
    }
}

#[test]
fn a_trim_of_a_point_is_excused_only_where_freecad_deletes_it_and_nothing_else() {
    // point_alone: the point p0000001 and the line n0000004; FreeCAD 1.1.3 deletes the point
    // (#se-what-may-be-trimmed).
    let (_, prepared, theirs) = both(&["trim_point"]);
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::TrimPoint));
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    let point_id = fc_id(p, &theirs[0], "p0000001");
    let changes: [(&str, Doctoring); 4] = [
        (
            "the line moved",
            Box::new(|fc| {
                fc.edited[0].points.get_mut("2").unwrap()[0] += 1e-6;
            }),
        ),
        (
            "the point kept",
            Box::new(move |fc| {
                let mut point = fc.edited[0].clone();
                point.id = point_id;
                point.kind = "point".to_owned();
                point.points = [("1".to_owned(), [2.0, 2.0])].into();
                fc.edited.push(point.clone());
                fc.geometry.push(point);
            }),
        ),
        (
            "the line deleted too",
            Box::new(|fc| {
                fc.edited.clear();
                fc.geometry.clear();
            }),
        ),
        (
            "something made",
            Box::new(|fc| {
                let mut line = fc.edited[0].clone();
                line.id = 99;
                fc.edited.push(line.clone());
                fc.geometry.push(line);
            }),
        ),
    ];
    for (what, change) in changes {
        let made = doctored(&theirs[0], |fc| change(fc));
        refused_against_freecads_edit(&verdict(p, Some(&made)), what);
    }
}

/// FreeCAD's geometry ID, before the edit, of `member` of `p`'s sketch.
fn fc_id(p: &Prepared, theirs: &Option<Theirs>, member: &str) -> i64 {
    let Some(Theirs::Edited(fc)) = theirs else {
        panic!("FreeCAD didn't make the edit: {theirs:?}");
    };
    fc.ids_before[p.model.elements.iter().position(|e| e == member).unwrap()]
}

#[test]
fn a_merge_whose_solve_silico_cant_take_agrees_where_freecads_solves_or_rolls_back() {
    // `sketch-edit-sweep --seed 32 --count 500`'s case 184: the top-left corner merged onto
    // the bottom line's far end, so the vertical n0000008 is left with no length
    // (#se-a-line-the-solve-shortens-to), and Silico refuses it. FreeCAD 1.1.3's solve fails (-1),
    // which its Coincident command rolls back (10 of 10 runs, when this test was written): both
    // refuse, as the note has it (#se-so-does-any-refusal-of-the).
    let mut cases = silico_oracle::generate::sketch_edit_cases(32, 500).unwrap();
    let case = cases.swap_remove(184);
    assert_eq!(case.name, "edit_32_184");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let theirs = freecad_outcomes(&prepared).unwrap();
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::MergeSolveUnbuildable), "{:?}", p.silico);
    assert!(
        sketch_edit::rolled_back(p, theirs[0].as_ref()),
        "{:?}",
        theirs[0]
    );
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert!(v.refused && v.excused.is_empty(), "{v:?}");
    // The verdict records the rollback, as a VariesMade case reads it.
    assert!(v.rolled_back, "{v:?}");
    assert_eq!(
        v.modulo,
        [
            "se-so-does-any-refusal-of-the",
            "se-a-tangent-at-or-tangent-on",
            "se-a-line-the-solve-shortens-to"
        ],
        "{v:?}"
    );
    // Its solve succeeding instead, the note's other outcome, the merged places (the job's
    // coincidence of geometry 4's end and geometry 2's end) at one place: Silico's refusal
    // excused.
    let Plan::Run { job, .. } = &p.plan else {
        panic!("FreeCAD isn't run");
    };
    // FreeCAD's geometry placed as Silico places the merge, by hand: p0000004, the end of
    // n0000007 and the start of n0000008, at the kept p0000013's place, (19.483, -17.098);
    // the rest as drawn.
    let line = |start: [f64; 2], end: [f64; 2]| FreecadGeometry::Line { start, end };
    let kept = [19.482999999999997, -17.098];
    assert_eq!(
        job.edit,
        FreecadEdit::Merge {
            coincidences: vec![[4, 2, 2, 2]],
            placed: Some(vec![
                line([-10.829, -17.098], [15.479, -17.098]),
                line([15.479, -17.098], [15.479, 11.804000000000002]),
                line([15.479, 11.804000000000002], kept),
                line(kept, [-10.829, -17.098]),
                line([15.479, -17.098], kept),
                line([7.245, -20.288], [7.245, 13.893000000000002]),
                line([-4.831, -21.785], [22.217, 5.262999999999998]),
                FreecadGeometry::Circle {
                    center: [-0.99, -17.711],
                    radius: 6.522
                },
                FreecadGeometry::Circle {
                    center: [15.479, -3.109],
                    radius: 6.522
                },
                FreecadGeometry::Point {
                    at: [-1.279, -17.098]
                },
            ]),
        }
    );
    let solved = |fc: &mut FreecadEdited| {
        fc.after.code = 0;
        let to = fc.geometry[4].points["2"];
        *fc.geometry[2].points.get_mut("2").unwrap() = to;
    };
    let v = verdict(p, Some(&doctored(&theirs[0], solved)));
    agrees(&v);
    assert!(!v.refused, "{v:?}");
    assert_eq!(v.excused.len(), 1, "{v:?}");
    // Its solve succeeding with the places apart, or without the coincidence: not merged, a
    // difference.
    let unmerged: [(&str, Doctor); 2] = [
        ("code 0, the places apart", |fc| fc.after.code = 0),
        ("code 0, no coincidence", |fc| {
            let to = fc.geometry[4].points["2"];
            *fc.geometry[2].points.get_mut("2").unwrap() = to;
            fc.after.code = 0;
            fc.constraints.pop();
        }),
    ];
    for (what, change) in unmerged {
        refused_against_freecads_edit(&verdict(p, Some(&doctored(&theirs[0], change))), what);
    }
    // Anything else FreeCAD does is a difference: a failure that moved a point, a partial
    // merge (its coincidence missing), a constraint of the case's gone or one more, a
    // geometry made or gone, a failure where the sketch failed before the merge too (nothing
    // to roll back), its edit raising, a crash, or a hang.
    let failed: Vec<(&str, Doctoring)> = vec![
        (
            "a dimension's value another",
            Box::new(|fc| {
                let length = fc
                    .constraints
                    .iter_mut()
                    .find(|c| c.kind == "Distance")
                    .unwrap();
                length.value += 1e-6;
            }),
        ),
        (
            "a point moved",
            Box::new(|fc| {
                fc.geometry[0].points.get_mut("1").unwrap()[0] += 1e-6;
            }),
        ),
        (
            "the merge's coincidence missing",
            Box::new(|fc| {
                fc.constraints.pop();
            }),
        ),
        (
            "a named constraint gone",
            Box::new(|fc| {
                let named = fc
                    .constraints
                    .iter()
                    .position(|c| !c.name.is_empty())
                    .unwrap();
                fc.constraints.remove(named);
            }),
        ),
        (
            "a constraint more",
            Box::new(|fc| {
                let mut extra = fc.constraints[0].clone();
                extra.name = "s_k9999999".to_owned();
                fc.constraints.push(extra);
            }),
        ),
        (
            "a geometry made",
            Box::new(|fc| {
                let mut made = fc.geometry[0].clone();
                made.id = 999;
                fc.geometry.push(made.clone());
                fc.edited.push(made);
            }),
        ),
        (
            "a geometry gone",
            Box::new(|fc| {
                fc.geometry.pop();
                fc.edited.pop();
            }),
        ),
        (
            "failing before the merge too",
            Box::new(|fc| {
                fc.before.code = -1;
            }),
        ),
    ];
    // Its solve finding a constraint redundant (-2) instead, which the command rolls back
    // too (#se-so-does-any-refusal-of-the, `edit_44_310`): agreeing, its geometry not checked.
    let redundant = doctored(&theirs[0], |fc| {
        fc.after.code = -2;
        fc.after.redundant = vec![1];
    });
    let v = verdict(p, Some(&redundant));
    assert!(v.refused && v.how.is_empty(), "{v:?}");
    for (what, change) in failed {
        let made = doctored(&theirs[0], |fc| change(fc));
        assert!(!sketch_edit::rolled_back(p, Some(&made)), "{what}");
        let v = verdict(p, Some(&made));
        assert!(!v.refused && v.excused.is_empty(), "{what}: {v:?}");
        assert_eq!(v.how.len(), 1, "{what}: {v}");
    }
    for other in [
        Theirs::Failed("refused by hand".to_owned()),
        Theirs::Lost(
            "FreeCAD crashed building it by a segmentation fault (signal: 11 (SIGSEGV))".to_owned(),
        ),
        Theirs::Lost("FreeCAD took longer than 60 s building it".to_owned()),
    ] {
        let v = verdict(p, Some(&other));
        assert!(
            !v.refused && !v.crashed && v.excused.is_empty(),
            "{other:?}: {v:?}"
        );
        assert_eq!(v.how.len(), 1, "{other:?}: {v}");
    }
}

#[test]
fn freecad_refusing_an_edit_the_note_has_it_make_is_a_difference() {
    // The rules whose edit the note has FreeCAD make: its refusal of one isn't the note's.
    let (_, prepared, _) = both(&[
        "split_circle_at_one_position",
        "split_line_at_freecads_position",
        "trim_point",
        "trim_symmetric_about_line_into_two",
        "trim_arc_tangent_no_piece_touches",
    ]);
    for p in &prepared {
        assert_eq!(
            rule(p).and_then(|r| r.pinned()),
            Some(sketch_edit::Pinned::Edits),
            "{}",
            p.name
        );
        let v = verdict(p, Some(&Theirs::Failed("refused by hand".to_owned())));
        assert!(!v.refused && v.excused.is_empty(), "{}: {v:?}", p.name);
        assert_eq!(v.how.len(), 1, "{}: {v}", p.name);
        assert!(
            v.how[0].starts_with("both refused, but the note has FreeCAD make the edit"),
            "{}: {v}",
            p.name
        );
    }
}

#[test]
fn a_freecad_crash_is_its_own_outcome_agreeing_only_where_the_note_has_it_crash() {
    let (_, prepared, theirs) = both(&[
        "split_point",
        "merge_line_ends",
        "split_line_beyond_end",
        "trim_point",
        "split_level_line",
    ]);
    // Splitting a point: 1.1.3 segfaults (#se-split-index-refusals), and Silico refuses it
    // (#se-split-refusals-as-data).
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::SplitNotCurve));
    let Some(Theirs::Lost(why)) = &theirs[0] else {
        panic!("FreeCAD didn't crash: {:?}", theirs[0]);
    };
    assert!(why.starts_with("FreeCAD crashed"), "{why}");
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert!(v.crashed && !v.refused, "{v:?}");
    assert_eq!(
        v.modulo,
        ["se-split-index-refusals", "se-split-refusals-as-data"],
        "{v:?}"
    );
    // On Linux, FreeCAD 1.1.3's SIGSEGV handler prints its line and exits with status 1
    // (src/App/Application.cpp:1815-1825), which the harness tells as a segmentation fault
    // too: the crash the note has.
    let linux = Theirs::Lost(
        "FreeCAD crashed building it by a segmentation fault (exit status: 1, after its SIGSEGV handler's \"Program received signal SIGSEGV, Segmentation fault.\"); its last output:\n#20  0x0 in main".to_owned(),
    );
    let v = verdict(p, Some(&linux));
    agrees(&v);
    assert!(v.crashed && !v.refused, "{v:?}");
    assert_eq!(
        v.modulo,
        ["se-split-index-refusals", "se-split-refusals-as-data"],
        "{v:?}"
    );
    // FreeCAD hanging on it, raising, or making it: not the crash the note has; nor exiting
    // with status 1 without the handler's line, even with the line in its output, which the
    // harness doesn't read (it tells how FreeCAD ended); nor an abort or another status.
    for other in [
        Theirs::Lost("FreeCAD took longer than 60 s building it".to_owned()),
        Theirs::Lost("FreeCAD crashed building it (exit status: 1)".to_owned()),
        Theirs::Lost(
            "FreeCAD crashed building it (exit status: 1); its last output:\nProgram received signal SIGSEGV, Segmentation fault.\nFreeCAD crashed building it by a segmentation fault".to_owned(),
        ),
        Theirs::Lost("FreeCAD crashed building it (exit status: 3)".to_owned()),
        Theirs::Lost("FreeCAD crashed building it (signal: 6 (SIGABRT))".to_owned()),
        Theirs::Failed("refused by hand".to_owned()),
    ] {
        let v = verdict(p, Some(&other));
        assert!(!v.crashed && !v.refused, "{other:?}: {v:?}");
        assert_eq!(v.how.len(), 1, "{other:?}: {v}");
    }
    // A crash where the note has FreeCAD refuse (a collapse,
    // #se-a-coincidence-that-would-bring-two; a split past the end, in OCCT,
    // #se-at-or-beyond-an-end-it), or make the edit (a trim of a point, #se-what-may-be-trimmed, a
    // line's split): a difference, never its refusal.
    let crash = Theirs::Lost(
        "FreeCAD crashed building it by a segmentation fault (signal: 11 (SIGSEGV))".to_owned(),
    );
    for p in &prepared[1..] {
        let v = verdict(p, Some(&crash));
        assert!(!v.crashed && !v.refused, "{}: {v:?}", p.name);
        assert!(v.excused.is_empty(), "{}: {v:?}", p.name);
        assert_eq!(v.how.len(), 1, "{}: {v}", p.name);
        assert!(
            v.how[0].starts_with("FreeCAD crashed or hung (FreeCAD crashed building it"),
            "{}: {v}",
            p.name
        );
    }
}

// ---- The tangency band compares all but the curves it touches (#251's condition c) ----

#[test]
fn a_trim_in_the_tangency_band_compares_every_curve_it_doesnt_touch() {
    // tangent_gap_1e-10: the circle c0000002 about the origin, radius 5, trimmed; the line
    // n0000005 1e-10 mm above its top, in the band; the upright line n0000008 at x 3,
    // crossing it, which neither side's trim touches but to cut.
    let (cases, prepared, theirs) = both(&["trim_circle_line_1e-10_off"]);
    let p = &prepared[0];
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    assert!(v.excused[0].starts_with("n0000005 is 1.0000000"), "{v:?}");
    assert!(
        v.modulo
            .iter()
            .any(|c| c == "se-where-trim-finds-intersections"),
        "{v:?}"
    );
    // The upright line made construction in Silico's result: reported, as in any case.
    let construction = |line: &'static str| {
        move |s: &mut Sketch| {
            s.lines
                .iter_mut()
                .find(|l| l.id == id(line))
                .unwrap()
                .construction = true;
        }
    };
    let v = verdict(
        &changed(&cases[0], p, construction("n0000008")),
        theirs[0].as_ref(),
    );
    assert_eq!(
        v.how,
        ["Silico's n0000008 is construction, FreeCAD's isn't"],
        "{v}"
    );
    // A `vertical` on the upright line on Silico's side only: reported, and the freedom it
    // takes. A distance from the trimmed piece's centre to the upright line, 3 as drawn, on
    // Silico's side only: it names the touched piece, so isn't compared, but the freedom it
    // takes still is (11 less 1).
    let constrained = |form: Form| {
        move |s: &mut Sketch| {
            s.constraints.push(SketchConstraint {
                id: id("k0000099"),
                form,
            });
        }
    };
    let vertical = Form::Vertical {
        line: Reference::Element(id("n0000008")),
    };
    let v = verdict(
        &changed(&cases[0], p, constrained(vertical)),
        theirs[0].as_ref(),
    );
    assert_eq!(
        v.how,
        [
            "Silico has Vertical(n0000008) (k0000099), FreeCAD hasn't",
            "degrees of freedom 10 against FreeCAD's 11",
        ],
        "{v}"
    );
    let distance = Form::PointLineDistance {
        point: Reference::Element(id("p0000001")),
        line: Reference::Element(id("n0000008")),
        value: Length::from_nm(3_000_000),
        side: LineSide::Left,
        driving: true,
    };
    let v = verdict(
        &changed(&cases[0], p, constrained(distance)),
        theirs[0].as_ref(),
    );
    assert_eq!(v.how, ["degrees of freedom 10 against FreeCAD's 11"], "{v}");
    // Its top end moved 1 mm right: before and after the solve, reported.
    let v = verdict(
        &changed(&cases[0], p, |s| {
            let top = s
                .points
                .iter_mut()
                .find(|q| q.id == id("p0000007"))
                .unwrap();
            top.x = silico_core::document::Coordinate::new(4.0).unwrap();
        }),
        theirs[0].as_ref(),
    );
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "after the edit, before the solve, Silico's point p0000007 is at [4.0, 8.0]"
        )),
        "{v}"
    );
    assert!(
        v.how.iter().any(|h| h.starts_with("point p0000007 at [")),
        "{v}"
    );
    // The touched curves: the line in the band, and the trimmed circle's piece, made
    // construction: not compared, so not reported.
    let v = verdict(
        &changed(&cases[0], p, construction("n0000005")),
        theirs[0].as_ref(),
    );
    agrees(&v);
    let v = verdict(
        &changed(&cases[0], p, |s| {
            for a in &mut s.arcs {
                a.construction = true;
            }
        }),
        theirs[0].as_ref(),
    );
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    // A curve missing from FreeCAD's result: the line in the band isn't compared, the
    // upright line is. FreeCAD's geometry is n0000005, n0000008, and the arc, in that order.
    let without = |member: &str| {
        doctored(&theirs[0], |fc| {
            let i = p.model.elements.iter().position(|e| e == member).unwrap();
            let gone = fc.ids_before[i];
            let at = fc.geometry.iter().position(|g| g.id == gone).unwrap();
            fc.edited.remove(at);
            fc.geometry.remove(at);
            let at = i32::try_from(at).unwrap();
            fc.constraints.retain(|c| c.refs.iter().all(|r| r[0] != at));
            for c in &mut fc.constraints {
                for r in &mut c.refs {
                    if r[0] > at {
                        r[0] -= 1;
                    }
                }
            }
        })
    };
    let v = verdict(p, Some(&without("n0000005")));
    agrees(&v);
    // A constraint of FreeCAD's on the line in the band, last, found redundant: touched, so
    // not named as FreeCAD's own.
    let listed = doctored(&theirs[0], |fc| {
        fc.constraints.push(FcConstraint {
            kind: "Horizontal".to_owned(),
            refs: vec![[0, 0]],
            value: 0.0,
            driving: true,
            name: String::new(),
        });
        fc.after.redundant.push(fc.constraints.len());
    });
    agrees(&verdict(p, Some(&listed)));
    let v = verdict(p, Some(&without("n0000008")));
    assert!(
        v.how
            .iter()
            .any(|h| h == "Silico has line n0000008, which FreeCAD hasn't"),
        "{v}"
    );
}

// ---- Edits FreeCAD has no counterpart for still run FreeCAD (#251's condition b) ----

#[test]
fn removing_a_point_curves_use_is_excused_only_where_freecads_vertex_delete_unjoins_it() {
    // arcs_through_circle_centre: the centre p0000001 of the arcs a0000004 and a0000007 and
    // the circle c0000008. FreeCAD has no edit deleting it (#se-freecad-has-no-delete-a-point); its
    // Delete on that vertex, a0000004's centre, unjoins that centre from the others, deleting no
    // curve (#se-freecad-s-gui-delete-on-a to #se-on-any-curve-s-end-it).
    let (_, prepared, theirs) = both(&["remove_point_curves_use"]);
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::RemovePointInUse));
    assert!(
        matches!(theirs[0], Some(Theirs::Edited(_))),
        "{:?}",
        theirs[0]
    );
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    // A curve deleted, a curve moved, or the end still joined: not the vertex Delete, a
    // difference.
    let changes: [(&str, Doctor); 3] = [
        ("a curve deleted", |fc| {
            fc.edited.pop();
            fc.geometry.pop();
        }),
        ("a curve moved", |fc| {
            fc.edited[1].points.get_mut("3").unwrap()[0] += 1e-6;
        }),
        ("the end still joined", |fc| {
            fc.constraints.push(FcConstraint {
                kind: "Coincident".to_owned(),
                refs: vec![[0, 3], [1, 3]],
                value: 0.0,
                driving: true,
                name: String::new(),
            });
        }),
    ];
    for (what, change) in changes {
        refused_against_freecads_edit(&verdict(p, Some(&doctored(&theirs[0], change))), what);
    }
    // Its Delete raising on a shared point, where 1.1.3 raises only on an end with no
    // coincidence (#se-on-a-point-with-no-coincidence): a difference.
    let raises = Theirs::Failed(sketch_edit::VERTEX_DELETE_RAISES.to_owned());
    for theirs in [&raises, &Theirs::Failed("refused by hand".to_owned())] {
        let v = verdict(p, Some(theirs));
        assert!(!v.refused && v.excused.is_empty(), "{v:?}");
        assert_eq!(v.how.len(), 1, "{v}");
    }
    // The job's vertex (the centre, on c0000008) moved to a0000004's start, p0000002, which
    // no other curve uses: an end with no coincidence in FreeCAD's model (the centre still
    // shared), where 1.1.3 raises, as the note gives it (#se-on-a-point-with-no-coincidence), and
    // both refuse; raising otherwise is a difference.
    let Plan::Run { job, modulo } = &p.plan else {
        panic!("FreeCAD isn't run");
    };
    assert!(
        matches!(job.edit, FreecadEdit::VertexDelete { position: 3, .. }),
        "{:?}",
        job.edit
    );
    let geometry = i32::try_from(
        p.model
            .elements
            .iter()
            .position(|e| e == "a0000004")
            .unwrap(),
    )
    .unwrap();
    let mut moved = job.clone();
    moved.edit = FreecadEdit::VertexDelete {
        geometry,
        position: 1,
    };
    let unshared = Prepared {
        plan: Plan::Run {
            job: moved,
            modulo: modulo.clone(),
        },
        ..p.clone()
    };
    let v = verdict(&unshared, Some(&raises));
    agrees(&v);
    assert!(v.refused, "{v:?}");
    let v = verdict(
        &unshared,
        Some(&Theirs::Failed("refused by hand".to_owned())),
    );
    assert!(!v.refused && v.how.len() == 1, "{v:?}");
}

#[test]
fn an_unjoin_of_an_unshared_point_agrees_only_where_freecad_raises_as_the_note_says() {
    // chain's p0000001, n0000005's start, no other curve's: 1.1.3's vertex Delete raises
    // "Not able to delete a constraint on point" (#se-on-a-point-with-no-coincidence).
    let (_, prepared, theirs) = both(&["unjoin_unshared_point"]);
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::UnjoinUnshared));
    assert!(
        matches!(&theirs[0], Some(Theirs::Failed(why)) if why.contains(sketch_edit::VERTEX_DELETE_RAISES)),
        "{:?}",
        theirs[0]
    );
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert!(v.refused, "{v:?}");
    // Raising otherwise: a difference.
    let v = verdict(p, Some(&Theirs::Failed("refused by hand".to_owned())));
    assert!(!v.refused && v.how.len() == 1, "{v:?}");
}

#[test]
fn a_point_merged_with_itself_runs_freecad_whose_command_refuses_it() {
    let (_, prepared, theirs) = both(&["merge_named_twice"]);
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::MergeNamedTwice));
    assert!(
        matches!(theirs[0], Some(Theirs::Edited(_))),
        "{:?}",
        theirs[0]
    );
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert!(v.refused, "{v:?}");
    // Its solve after the coincidence no worse than before: the command still refuses it,
    // the points being one already (#se-points-already-coincident-even-through-other).
    let clean = doctored(&theirs[0], |fc| fc.after = fc.before.clone());
    let v = verdict(p, Some(&clean));
    agrees(&v);
    assert!(v.refused, "{v:?}");
}

#[test]
fn a_trim_at_a_rounds_centre_agrees_only_where_freecad_raises() {
    // 1.1.3 throws "Unknown C++ exception" (#se-a-pick-at-a-round-s).
    let (_, prepared, theirs) = both(&["trim_at_centre"]);
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::TrimAtCentre));
    assert!(
        matches!(theirs[0], Some(Theirs::Failed(_))),
        "{:?}",
        theirs[0]
    );
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert!(v.refused, "{v:?}");
    // Its edit raising otherwise: a difference.
    let v = verdict(p, Some(&Theirs::Failed("refused by hand".to_owned())));
    assert!(!v.refused && v.how.len() == 1, "{v:?}");
    // FreeCAD refusing it otherwise (its plan, never run): a difference.
    let planned = Prepared {
        plan: Plan::Refuses("refused by hand".to_owned()),
        ..p.clone()
    };
    let v = verdict(&planned, None);
    assert!(!v.refused, "{v:?}");
    assert_eq!(v.how.len(), 1, "{v}");
    // A crash or a hang instead: a difference.
    for other in [
        Theirs::Lost(
            "FreeCAD crashed building it by a segmentation fault (signal: 11 (SIGSEGV))".to_owned(),
        ),
        Theirs::Lost("FreeCAD took longer than 60 s building it".to_owned()),
    ] {
        let v = verdict(p, Some(&other));
        assert!(!v.refused && !v.crashed, "{other:?}: {v:?}");
        assert_eq!(v.how.len(), 1, "{other:?}: {v}");
    }
}

#[test]
fn a_case_freecad_was_run_on_and_gave_no_outcome_is_a_difference() {
    let (_, prepared, _) = both(&["split_point", "split_level_line"]);
    for p in &prepared {
        let v = verdict(p, None);
        assert!(!v.refused && !v.crashed, "{}: {v:?}", p.name);
        assert_eq!(
            v.how,
            ["FreeCAD was run on it and gave no outcome"],
            "{}",
            p.name
        );
    }
}

#[test]
fn a_removed_tangent_at_leaves_its_point_shared_where_freecads_delete_unjoins_it() {
    // `sketch-edit-sweep --seed 42 --count 500`'s case 214: the tangent_at k0000016 of the
    // line n0000012 and the arc a0000015 at their shared point p0000011, removed. FreeCAD's
    // endpoint Tangent is all that joins the two ends, so its delete leaves them apart (10 of
    // 10 runs); Silico's point stays shared (#se-in-silico-a-constraint-is-removed), so FreeCAD's
    // ends are joined again.
    let mut sweep = silico_oracle::generate::sketch_edit_cases(42, 500).unwrap();
    let case = sweep.swap_remove(214);
    assert_eq!(case.name, "edit_42_214");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let Plan::Run { job, modulo } = &p.plan else {
        panic!("FreeCAD isn't run");
    };
    let FreecadEdit::Delete { rejoin, .. } = &job.edit else {
        panic!("{:?}", job.edit);
    };
    let at = |member: &str, pos: i32| {
        let g = p.model.elements.iter().position(|e| e == member).unwrap();
        (i32::try_from(g).unwrap(), pos)
    };
    let ((g1, p1), (g2, p2)) = (at("n0000012", 2), at("a0000015", 1));
    assert!(
        rejoin == &vec![[g1, p1, g2, p2]] || rejoin == &vec![[g2, p2, g1, p1]],
        "{rejoin:?}"
    );
    assert!(
        modulo
            .iter()
            .any(|c| c == "se-in-silico-a-constraint-is-removed"),
        "{modulo:?}"
    );
    let theirs = freecad_outcomes(&prepared).unwrap();
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    // FreeCAD's ends left apart (the rejoining coincidence taken off): a difference, the
    // point Silico keeps shared reported.
    let apart = doctored(&theirs[0], |fc| {
        fc.constraints.retain(|c| {
            !(c.kind == "Coincident"
                && (c.refs == [[g1, p1], [g2, p2]] || c.refs == [[g2, p2], [g1, p1]]))
        });
    });
    let v = verdict(p, Some(&apart));
    assert!(
        v.how
            .iter()
            .any(|h| h.starts_with("Silico's point p0000011 is one point, which FreeCAD has apart")),
        "{v}"
    );
}

#[test]
fn a_removed_tangent_at_is_rejoined_in_freecads_job_only_where_nothing_else_joins_its_ends() {
    // Three lines at p0000002, n0000001's end, n0000002's start and n0000003's start. In
    // FreeCAD's model a tangent_at there is the endpoint Tangent that joins its two ends in
    // place of a Coincident, the third end joined by a Coincident: its removal joins the two
    // again (#se-in-silico-a-constraint-is-removed), either pair. None removed, nothing is added.
    let three = |a: &str, b: &str| {
        sketch(&format!(
            r#""points": [{{"id": "p0000001", "x": 0, "y": 0, "construction": false}}, {{"id": "p0000002", "x": 5, "y": 0, "construction": false}}, {{"id": "p0000003", "x": 5, "y": 5, "construction": false}}, {{"id": "p0000004", "x": 10, "y": 0, "construction": false}}],
        "lines": [{{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}}, {{"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false}}, {{"id": "n0000003", "from": "p0000002", "to": "p0000004", "construction": false}}],
        "circles": [], "arcs": [],
        "constraints": [{{"id": "k0000001", "type": "tangent_at", "a": "{a}", "b": "{b}", "at": "p0000002", "direction": "same"}}]"#
        ))
    };
    let removed = || Edit::Remove {
        members: vec!["k0000001".to_owned()],
    };
    for (a, b, want) in [
        ("n0000001", "n0000003", vec![[0, 2, 2, 1]]),
        ("n0000002", "n0000003", vec![[1, 1, 2, 1]]),
    ] {
        let Plan::Run { job, modulo } = planned(&three(a, b), removed()) else {
            panic!("FreeCAD isn't run");
        };
        let FreecadEdit::Delete { rejoin, .. } = &job.edit else {
            panic!("{:?}", job.edit);
        };
        assert_eq!(rejoin, &want, "{a}, {b}: {:?}", job.constraints);
        assert!(
            modulo
                .iter()
                .any(|c| c == "se-in-silico-a-constraint-is-removed"),
            "{modulo:?}"
        );
    }
    let Plan::Run { job, modulo } = planned(
        &three("n0000001", "n0000003"),
        Edit::Remove {
            members: vec!["n0000002".to_owned()],
        },
    ) else {
        panic!("FreeCAD isn't run");
    };
    let FreecadEdit::Delete { rejoin, .. } = &job.edit else {
        panic!("{:?}", job.edit);
    };
    assert!(
        rejoin.is_empty()
            && !modulo
                .iter()
                .any(|c| c == "se-in-silico-a-constraint-is-removed"),
        "{rejoin:?} {modulo:?}"
    );
}

#[test]
fn a_merge_whose_solve_fails_from_the_kept_points_place_is_refused_and_named_the_other_way_solves()
{
    // `sketch-edit-sweep --seed 43 --count 500`'s case 229's sketch: p0000001 held at
    // (-29.382, -9.384) by distance_x and distance_y, p0000003 33 mm away at
    // (-11.093, 18.734). Merged with p0000003 kept, the solve starts from p0000003's place
    // and fails (#se-so-does-any-refusal-of-the, #se-a-solve-that-fails-or-conflicts); named the
    // other way round, the held point first, it solves. Its freedoms by hand: 15 points (30 values)
    // and 2 circle radii, 32; less the horizontal, distance_x, distance_y, parallel, tangent_at,
    // the arc's rule, tangent, radius, and equal (one each) and the tangent_on (two): 21.
    let mut sweep = silico_oracle::generate::sketch_edit_cases(43, 500).unwrap();
    let case = sweep.swap_remove(229);
    assert_eq!(case.name, "edit_43_229");
    let sketch = case.sketch().unwrap().clone();
    let mut worker = SketchWorker::start().unwrap();
    for (points, solves) in [
        (["p0000003", "p0000001"], false),
        (["p0000001", "p0000003"], true),
    ] {
        let edited =
            silico_core::document::sketch_edit::merge(&sketch, &[id(points[0]), id(points[1])])
                .unwrap();
        let SilicoSide::Edited(s) =
            silico_side(&case.document, case.feature, edited, &mut worker).unwrap()
        else {
            panic!("{points:?}: the gate refuses the merge");
        };
        match (&s.ours.outcome, solves) {
            (Err(e), false) => assert_eq!(e.code(), silico_core::ErrorCode::SolveFailed, "{e}"),
            (Ok(solution), true) => {
                assert_eq!(solution.dofs, 21);
                let held = solution.points[&id("p0000001")];
                assert_eq!([held[0].mm(), held[1].mm()], [-29.382, -9.384]);
            }
            (outcome, _) => panic!("{points:?}: {outcome:?}"),
        }
    }
}

#[test]
fn a_merge_whose_solve_fails_is_excused_only_where_freecads_merge_solves_and_merges() {
    let mut sweep = silico_oracle::generate::sketch_edit_cases(43, 500).unwrap();
    let case = sweep.swap_remove(229);
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::MergeSolveFails), "{:?}", p.silico);
    let theirs = freecad_outcomes(&prepared).unwrap();
    let Plan::Run { job, .. } = &p.plan else {
        panic!("FreeCAD isn't run");
    };
    let FreecadEdit::Merge { coincidences, .. } = &job.edit else {
        panic!("{:?}", job.edit);
    };
    let [g1, p1, g2, p2] = coincidences[0];
    let (i1, i2) = (usize::try_from(g1).unwrap(), usize::try_from(g2).unwrap());
    let together = move |fc: &mut FreecadEdited| {
        let to = fc.geometry[i1].points[&p1.to_string()];
        *fc.geometry[i2].points.get_mut(&p2.to_string()).unwrap() = to;
    };
    // FreeCAD's merge solving (0), its places together: Silico's refusal excused.
    let merged = doctored(&theirs[0], |fc| {
        fc.after.code = 0;
        together(fc);
    });
    let v = verdict(p, Some(&merged));
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    // The case's points drawn together: both solvers start alike, so a failure Silico's
    // alone finds is no seed's, a difference.
    let Edit::Merge { points } = &p.edit else {
        panic!("{:?}", p.edit);
    };
    let mut drawn_together = p.clone();
    let kept = drawn_together
        .sketch
        .points
        .iter()
        .find(|q| q.id == id(&points[0]))
        .map(|q| (q.x, q.y))
        .unwrap();
    for q in &mut drawn_together.sketch.points {
        if points[1..].iter().any(|other| q.id == id(other)) {
            (q.x, q.y) = kept;
        }
    }
    let v = verdict(&drawn_together, Some(&merged));
    assert!(v.excused.is_empty() && !v.how.is_empty(), "{v:?}");
    // Reporting success with its places apart (as one run of FreeCAD's did), or without its
    // coincidence: a difference.
    let apart = doctored(&theirs[0], |fc| {
        fc.after.code = 0;
        together(fc);
        fc.geometry[i2].points.get_mut(&p2.to_string()).unwrap()[0] += 1.0;
    });
    let unmade = doctored(&theirs[0], |fc| {
        fc.after.code = 0;
        together(fc);
        fc.constraints.pop();
    });
    for (what, made) in [("apart", apart), ("no coincidence", unmade)] {
        let v = verdict(p, Some(&made));
        assert!(v.excused.is_empty() && !v.how.is_empty(), "{what}: {v:?}");
    }
}

#[test]
fn a_merge_freecad_rolls_back_after_finding_constraints_redundant_agrees() {
    // `sketch-edit-sweep --seed 44 --count 500`'s case 310: points merged from apart, whose
    // solve puts a symmetry's two points at one place, a result Silico refuses
    // (#se-so-does-any-refusal-of-the). FreeCAD 1.1.3's merge finds 13 constraints redundant (-2),
    // which its Coincident command rolls back, in 39 of 40 runs; in one it found one conflicting as
    // well (-3), so the case is known as FreeCAD varying, and FreeCAD's diagnosis here is set to
    // the -2 it gives nearly always, not taken as it came. Its geometry and constraints are
    // FreeCAD's own.
    let mut sweep = silico_oracle::generate::sketch_edit_cases(44, 500).unwrap();
    let case = sweep.swap_remove(310);
    assert_eq!(case.name, "edit_44_310");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::MergeSolveUnbuildable), "{:?}", p.silico);
    let live = freecad_outcomes(&prepared).unwrap();
    let theirs = [Some(doctored(&live[0], |fc| {
        fc.after.code = -2;
        fc.after.conflicting.clear();
        fc.after.redundant = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14];
        fc.after.malformed.clear();
    }))];
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert!(v.refused, "{v:?}");
    // FreeCAD's merge's coincidence missing, a case's constraint gone, or its solve finding a
    // conflict (-3, or a conflicting constraint beside the redundancy) rather than only
    // redundancy: a difference.
    let changes: [(&str, Doctor); 6] = [
        ("a conflict", |fc| {
            fc.after.code = -3;
            fc.after.conflicting = vec![1];
        }),
        ("a conflict's code alone", |fc| fc.after.code = -3),
        ("redundant with a conflict", |fc| {
            fc.after.conflicting = vec![1]
        }),
        ("-2 with no redundancy new", |fc| fc.after.redundant.clear()),
        ("the coincidence missing", |fc| {
            fc.constraints.pop();
        }),
        ("a constraint gone", |fc| {
            let named = fc
                .constraints
                .iter()
                .position(|c| !c.name.is_empty())
                .unwrap();
            fc.constraints.remove(named);
        }),
    ];
    for (what, change) in changes {
        let v = verdict(p, Some(&doctored(&theirs[0], change)));
        assert!(!v.refused && !v.how.is_empty(), "{what}: {v:?}");
    }
}

#[test]
fn a_split_circles_edge_tangent_goes_to_the_arc_whose_sweep_holds_the_contact() {
    // `sketch-edit-sweep --seed 45 --count 500`'s case 170: the circle c0000025 about
    // (-0.262, -17.213), radius 2.557, tangent to the line y = -19.77 at its bottom (270
    // degrees), split at about -63.7 and 97.6 degrees. The kept arc runs -63.7 to 97.6, the
    // new one, w0000002, 97.6 to 296.3, which holds 270: the tangent goes to w0000002 on
    // both sides (#se-split-on-dropped, #se-a-constraint-that-no-piece-can).
    let mut sweep = silico_oracle::generate::sketch_edit_cases(45, 500).unwrap();
    let case = sweep.swap_remove(170);
    assert_eq!(case.name, "edit_45_170");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let SilicoSide::Edited(s) = &p.silico else {
        panic!("{:?}", p.silico);
    };
    assert!(s.edited.sketch.constraints.iter().any(|k| k.id == id("k0000026")
        && matches!(&k.form, Form::Tangent { b, .. } if b.element() == Some(&id("w0000002")))));
    let theirs = freecad_outcomes(&prepared).unwrap();
    agrees(&verdict(p, theirs[0].as_ref()));
}

#[test]
fn a_varying_case_is_shown_known_only_while_silicos_side_is_its_rules_refusal() {
    // merge_holds_own_centre, made known here as FreeCAD varying by its rule: shown so while
    // Silico refuses it by MergeHoldsOwn; with Silico's result gated, refused for a reason no
    // rule names, or the case's sketch not drawn as it solves, it's compared, and the known
    // case a problem.
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&holds_own_centre(), &mut worker).unwrap()];
    let theirs = freecad_outcomes(&prepared).unwrap();
    let p = &prepared[0];
    let known = [sketch_edit::Known {
        case: "merge_holds_own_centre",
        shows: sketch_edit::Shows::Varies(Rule::MergeHoldsOwn(String::new())),
        why: "varies here",
    }];
    let v = verdict(p, theirs[0].as_ref());
    // Refused: Silico didn't make the edit.
    assert!(!v.made, "{v:?}");
    let (differ, shown, stale) = without_known(vec![v], &known);
    assert!(differ.is_empty() && stale.is_empty() && shown.len() == 1);
    let mut problem = p.clone();
    problem
        .problems
        .push("the case's sketch isn't drawn as it solves".to_owned());
    for (what, other) in [
        (
            "gated",
            Prepared {
                silico: SilicoSide::Gated(vec!["a line of length 0".to_owned()]),
                ..p.clone()
            },
        ),
        (
            "refused for no rule",
            Prepared {
                silico: SilicoSide::Refused("refused by hand".into()),
                ..p.clone()
            },
        ),
        ("a problem of its own", problem),
    ] {
        let v = verdict(&other, theirs[0].as_ref());
        // Gated or refused, Silico made no edit.
        assert!(!v.made, "{what}: {v:?}");
        let (differ, shown, stale) = without_known(vec![v], &known);
        assert!(
            differ.len() == 1 && shown.is_empty() && stale.len() == 1,
            "{what}: {differ:?} {shown:?} {stale:?}"
        );
    }
    // FreeCAD's model of the case malformed before the edit, or the harness's own code
    // failing around FreeCAD's edit: not hidden as known either.
    let malformed = doctored(&theirs[0], |fc| fc.before.malformed = vec![1]);
    let harness = Theirs::Harness("KeyError: 'geometry'".to_owned());
    for (what, other) in [("malformed before", malformed), ("the harness", harness)] {
        let v = verdict(p, Some(&other));
        assert!(v.problem, "{what}: {v:?}");
        let (differ, shown, stale) = without_known(vec![v], &known);
        assert!(
            differ.len() == 1 && shown.is_empty() && stale.len() == 1,
            "{what}: {differ:?} {shown:?} {stale:?}"
        );
    }
}

#[test]
fn a_merge_left_with_an_edge_tangent_beside_a_tangent_on_is_the_gates_rule() {
    // `sketch-edit-sweep --seed 45 --count 500`'s case 93: the merge would leave the edge
    // tangent k0000020 beside the tangent_on k0000024 of the same pair, which the gate
    // refuses (#290, #se-the-merged-sketch-is-gated). Naming another constraint isn't the rule.
    let mut sweep = silico_oracle::generate::sketch_edit_cases(45, 500).unwrap();
    let case = sweep.swap_remove(93);
    assert_eq!(case.name, "edit_45_93");
    let mut worker = SketchWorker::start().unwrap();
    let p = prepare(&case, &mut worker).unwrap();
    assert!(
        matches!(rule(&p), Some(Rule::MergeGatedTangentOn(id)) if id == "k0000024"),
        "{:?}",
        p.silico
    );
    let SilicoSide::Refused(r) = &p.silico else {
        panic!("{:?}", p.silico);
    };
    for change in [
        |r: &mut Refusal| *r.elements.last_mut().unwrap() = "k0000020".to_owned(),
        |r: &mut Refusal| {
            r.message = r
                .message
                .replace("constraint k0000020", "constraint k0000099")
        },
        |r: &mut Refusal| {
            r.message = r
                .message
                .replace("a tangent_on puts point", "a constraint puts point")
        },
        |r: &mut Refusal| r.elements.swap(0, 1),
    ] {
        let mut r = r.clone();
        change(&mut r);
        let wrong = Prepared {
            silico: SilicoSide::Refused(r),
            ..p.clone()
        };
        assert!(rule(&wrong).is_none(), "{:?}", wrong.silico);
    }
    // The tangent_on's at moved to a point that's no curve's end: not the rule.
    let mut moved = p.clone();
    let elsewhere = p
        .sketch
        .points
        .iter()
        .map(|q| q.id.clone())
        .find(|q| {
            !p.sketch.lines.iter().any(|l| l.from == *q || l.to == *q)
                && !p.sketch.arcs.iter().any(|a| a.start == *q || a.end == *q)
        })
        .unwrap();
    let tangent_on = moved
        .sketch
        .constraints
        .iter_mut()
        .find(|k| k.id == id("k0000024"))
        .unwrap();
    let Form::TangentOn { at, .. } = &mut tangent_on.form else {
        panic!("{:?}", tangent_on.form);
    };
    *at = Reference::Element(elsewhere);
    assert!(rule(&moved).is_none(), "{:?}", moved.sketch.constraints);
}

#[test]
fn a_split_circles_edge_tangent_in_the_kept_arc_across_its_wrap_stays_on_it() {
    // A circle about the origin, radius 5, tangent to the upright line x = 5 at (5, 0),
    // 0 degrees, split at 300 and 60 degrees: the kept arc runs from 300 to 60 across 0,
    // which holds the contact, so the tangent stays on it, c0000002, on both sides.
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000003", "x": 5, "y": -8, "construction": false}, {"id": "p0000004", "x": 5, "y": 8, "construction": false}], "lines": [{"id": "n0000005", "from": "p0000003", "to": "p0000004", "construction": false}], "circles": [{"id": "c0000002", "center": "p0000001", "radius": 5, "construction": false}], "arcs": [], "constraints": [{"id": "k0000006", "type": "tangent", "a": "n0000005", "b": "c0000002", "side": "left"}]}]}"#;
    let document = Checked::new(read(text.as_bytes()).unwrap())
        .unwrap()
        .document()
        .clone();
    let at = |deg: f64| {
        let r = deg.to_radians();
        [5.0 * libm::cos(r), 5.0 * libm::sin(r)]
    };
    let case = EditCase {
        name: "split_circle_contact_across_the_wrap".to_owned(),
        note: String::new(),
        document,
        feature: 0,
        edit: Edit::Split {
            curve: "c0000002".to_owned(),
            at: vec![at(300.0), at(60.0)],
        },
    };
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let SilicoSide::Edited(s) = &p.silico else {
        panic!("{:?}", p.silico);
    };
    assert!(s.edited.sketch.constraints.iter().any(|k| k.id == id("k0000006")
        && matches!(&k.form, Form::Tangent { b, .. } if b.element() == Some(&id("c0000002")))));
    let theirs = freecad_outcomes(&prepared).unwrap();
    agrees(&verdict(p, theirs[0].as_ref()));
}

#[test]
fn a_merge_freecad_rolls_back_after_finding_constraints_conflicting_agrees() {
    // `sketch-edit-sweep --seed 45 --count 500`'s case 243: a merge whose solve fails from
    // the kept point's place, which Silico refuses (#se-so-does-any-refusal-of-the,
    // #se-a-solve-that-fails-or-conflicts). FreeCAD 1.1.3's merge finds eight constraints
    // conflicting (-3, 10 of 10 runs), which its Coincident command rolls back.
    let mut sweep = silico_oracle::generate::sketch_edit_cases(45, 500).unwrap();
    let case = sweep.swap_remove(243);
    assert_eq!(case.name, "edit_45_243");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::MergeSolveFails), "{:?}", p.silico);
    // FreeCAD's diagnosis set to the one it gave in 10 of 10 runs, not taken as it came.
    let live = freecad_outcomes(&prepared).unwrap();
    let theirs = [Some(doctored(&live[0], |fc| {
        fc.after.code = -3;
        fc.after.conflicting = vec![3, 6, 7, 11, 12, 13, 14, 15];
        fc.after.redundant.clear();
        fc.after.malformed.clear();
    }))];
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert!(v.refused, "{v:?}");
    // Its conflict beside a redundancy, or a malformed constraint: not what was measured.
    let changes: [(&str, Doctor); 3] = [
        ("-3 with no conflict new", |fc| fc.after.conflicting.clear()),
        ("a redundancy too", |fc| fc.after.redundant = vec![1]),
        ("a malformed constraint too", |fc| {
            fc.after.malformed = vec![1]
        }),
    ];
    for (what, change) in changes {
        let v = verdict(p, Some(&doctored(&theirs[0], change)));
        assert!(!v.refused && !v.how.is_empty(), "{what}: {v:?}");
    }
}

/// A case of the sketch of `fields` (as [`sketch`] takes them) and `edit`.
fn case_of(name: &str, fields: &str, edit: Edit) -> EditCase {
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"s0000001\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S\", \"plane\": \"xy\", {fields}}}]}}"
    );
    let document = Checked::new(read(text.as_bytes()).unwrap())
        .unwrap()
        .document()
        .clone();
    EditCase {
        name: name.to_owned(),
        note: String::new(),
        document,
        feature: 0,
        edit,
    }
}

/// `p`'s FreeCAD geometry index, after the edit, of its element `element`.
fn solved_index(p: &Prepared, fc: &FreecadEdited, element: &str) -> i32 {
    let i = p.model.elements.iter().position(|e| e == element).unwrap();
    let id = fc.ids_before[i];
    i32::try_from(fc.geometry.iter().position(|g| g.id == id).unwrap()).unwrap()
}

#[test]
fn freecad_keeping_what_silicos_removal_removed_is_a_difference() {
    // remove_point_alone removes the lone point p0000001 at (2, 2); remove_middle_of_chain
    // the line n0000006 from (5, 0) to (10, 0). FreeCAD keeping either: a difference that
    // says so.
    let (_, prepared, theirs) = both(&["remove_point_alone", "remove_middle_of_chain"]);
    for ((p, fc), (member, kind, points)) in prepared.iter().zip(&theirs).zip([
        ("p0000001", "point", vec![("1", [2.0, 2.0])]),
        (
            "n0000006",
            "line",
            vec![("1", [5.0, 0.0]), ("2", [10.0, 0.0])],
        ),
    ]) {
        agrees(&verdict(p, fc.as_ref()));
        let kept = doctored(fc, |fc| {
            let i = p.model.elements.iter().position(|e| e == member).unwrap();
            fc.geometry.push(FcGeometry {
                id: fc.ids_before[i],
                kind: kind.to_owned(),
                points: points.iter().map(|(k, q)| ((*k).to_owned(), *q)).collect(),
                radius: None,
                construction: false,
            });
        });
        let v = verdict(p, Some(&kept));
        let want = format!("FreeCAD keeps {kind} {member}");
        assert!(v.how.iter().any(|h| h.starts_with(&want)), "{want}: {v}");
    }
}

#[test]
fn freecad_joining_what_silico_has_apart_names_each_place_and_point() {
    // remove_constraint leaves chain's lines, p0000001 at the origin, and its distance_x
    // k0000008 from the origin to p0000002, 5. FreeCAD joining the origin (its geometry -1's
    // start) with n0000005's start, which Silico has as p0000001, apart from the origin: a
    // difference naming FreeCAD's -1 and both of Silico's points.
    let (_, prepared, theirs) = both(&["remove_constraint", "remove_circle_sharing_centre"]);
    let p = &prepared[0];
    agrees(&verdict(p, theirs[0].as_ref()));
    let joined = doctored(&theirs[0], |fc| {
        let n5 = solved_index(p, fc, "n0000005");
        fc.constraints.push(FcConstraint {
            kind: "Coincident".to_owned(),
            refs: vec![[-1, 1], [n5, 1]],
            value: 0.0,
            driving: true,
            name: String::new(),
        });
    });
    let v = verdict(p, Some(&joined));
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "FreeCAD joins FreeCAD's -1, n0000005's start, which are Silico's points origin, p0000001 apart"
        )),
        "{v}"
    );
    // A line of FreeCAD's own, first, its start joined with n0000005's start and n0000006's
    // end: named as FreeCAD's geometry 0.
    let own = doctored(&theirs[0], |fc| {
        for c in &mut fc.constraints {
            for r in &mut c.refs {
                if r[0] >= 0 {
                    r[0] += 1;
                }
            }
        }
        let line = FcGeometry {
            id: 999,
            kind: "line".to_owned(),
            points: [("1", [0.0, 0.0]), ("2", [5.0, 0.0])]
                .into_iter()
                .map(|(k, q)| (k.to_owned(), q))
                .collect(),
            radius: None,
            construction: false,
        };
        fc.edited.insert(0, line.clone());
        fc.geometry.insert(0, line);
        let (n5, n6) = (
            solved_index(p, fc, "n0000005"),
            solved_index(p, fc, "n0000006"),
        );
        for other in [[n5, 1], [n6, 2]] {
            fc.constraints.push(FcConstraint {
                kind: "Coincident".to_owned(),
                refs: vec![[0, 1], other],
                value: 0.0,
                driving: true,
                name: String::new(),
            });
        }
    });
    let v = verdict(p, Some(&own));
    assert!(
        v.how
            .iter()
            .any(|h| h.starts_with("FreeCAD joins FreeCAD's geometry 0's start, ")),
        "{v}"
    );
    // FreeCAD's distance_y k0000009 1e-9 rather than 0, exactly the values' tolerance: the
    // same value.
    let near = doctored(&theirs[0], |fc| {
        let c = fc
            .constraints
            .iter_mut()
            .find(|c| c.kind == "DistanceY")
            .unwrap();
        c.value = 1e-9;
    });
    agrees(&verdict(p, Some(&near)));
    // FreeCAD's distance_x 5.001: the dimension's value differs.
    let longer = doctored(&theirs[0], |fc| {
        let c = fc
            .constraints
            .iter_mut()
            .find(|c| c.kind == "DistanceX")
            .unwrap();
        c.value += 1e-3;
    });
    let v = verdict(p, Some(&longer));
    assert!(
        v.how
            .iter()
            .any(|h| h.contains("is 5 in Silico's (k0000008), 5.001 in FreeCAD's")),
        "{v}"
    );
    // remove_circle_sharing_centre leaves the arcs a0000004 (about p0000001, from
    // p0000002) and a0000007, about the same p0000001. FreeCAD joining a0000004's centre with
    // its start: each place named by its role.
    let p = &prepared[1];
    agrees(&verdict(p, theirs[1].as_ref()));
    let joined = doctored(&theirs[1], |fc| {
        let a4 = solved_index(p, fc, "a0000004");
        fc.constraints.push(FcConstraint {
            kind: "Coincident".to_owned(),
            refs: vec![[a4, 3], [a4, 1]],
            value: 0.0,
            driving: true,
            name: String::new(),
        });
    });
    let v = verdict(p, Some(&joined));
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "FreeCAD joins a0000004's start, a0000004's centre, a0000007's centre, which are Silico's points p0000001, p0000002 apart"
        )),
        "{v}"
    );
}

#[test]
fn a_redundancy_both_sides_find_agrees_and_one_only_freecads_is_named() {
    // The line n0000003 along y = 0, held horizontal and both ends at y = 0: one constraint
    // redundant, on both sides alike. Removing the lone point p0000008 leaves it so.
    let case = case_of(
        "redundancy_kept",
        r#""points": [{"id": "p0000001", "x": 1, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}, {"id": "p0000008", "x": 6, "y": 7, "construction": false}],
        "lines": [{"id": "n0000003", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [],
        "constraints": [{"id": "k0000005", "type": "horizontal", "line": "n0000003"}, {"id": "k0000006", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000007", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true}]"#,
        Edit::Remove {
            members: vec!["p0000008".to_owned()],
        },
    );
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let theirs = freecad_outcomes(&prepared).unwrap();
    let SilicoSide::Edited(s) = &p.silico else {
        panic!("{:?}", p.silico);
    };
    assert_eq!(s.ours.redundant.len(), 1, "{:?}", s.ours);
    let Some(Theirs::Edited(fc)) = &theirs[0] else {
        panic!("{:?}", theirs[0]);
    };
    assert_eq!(fc.after.redundant.len(), 1, "{fc:?}");
    agrees(&verdict(p, theirs[0].as_ref()));
    // A constraint of FreeCAD's own, which Silico hasn't, redundant too: named as FreeCAD's.
    let extra = doctored(&theirs[0], |fc| {
        let n3 = solved_index(p, fc, "n0000003");
        fc.constraints.push(FcConstraint {
            kind: "DistanceX".to_owned(),
            refs: vec![[-1, 1], [n3, 2]],
            value: 10.0,
            driving: true,
            name: String::new(),
        });
        fc.after.redundant.push(fc.constraints.len());
    });
    let v = verdict(p, Some(&extra));
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "FreeCAD finds its DistanceX(origin, p0000002), which Silico hasn't, redundant"
        )),
        "{v}"
    );
}

#[test]
fn a_merge_from_apart_agrees_and_silicos_other_points_moved_before_the_solve_are_named() {
    // p0000006 (3, 4), a lone point, merged into the line n0000003's end p0000002 (10, 3);
    // the lone points p0000007 (6, 7) and p0000008 (0, 0) stay where they are.
    let case = case_of(
        "merge_lone_point_from_apart",
        r#""points": [{"id": "p0000001", "x": 1, "y": 2, "construction": false}, {"id": "p0000002", "x": 10, "y": 3, "construction": false}, {"id": "p0000006", "x": 3, "y": 4, "construction": false}, {"id": "p0000007", "x": 6, "y": 7, "construction": false}, {"id": "p0000008", "x": 0, "y": 0, "construction": false}],
        "lines": [{"id": "n0000003", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [], "constraints": []"#,
        Edit::Merge {
            points: vec!["p0000002".to_owned(), "p0000006".to_owned()],
        },
    );
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let theirs = freecad_outcomes(&prepared).unwrap();
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    // Silico made the edit, as its side says.
    assert!(v.made, "{v:?}");
    // The lone p0000007, which neither solve moves: Silico's solved place 0.9e-8 mm one way
    // and FreeCAD's 0.9e-8 the other, each within 1e-8 of where it started, agree (1.8e-8
    // apart, within twice that); 1.1e-8 each way, each solve has moved it, so it isn't
    // compared by place.
    let nudged = |by: f64| {
        let SilicoSide::Edited(s) = &p.silico else {
            panic!("{:?}", p.silico);
        };
        let mut s = s.clone();
        let solution = s.ours.outcome.as_mut().unwrap();
        let q = solution.points.get_mut(&id("p0000007")).unwrap();
        q[0] = silico_core::document::Coordinate::new(q[0].mm() + by).unwrap();
        let ours = Prepared {
            silico: SilicoSide::Edited(s),
            ..p.clone()
        };
        let i = p
            .model
            .elements
            .iter()
            .position(|e| e == "p0000007")
            .unwrap();
        let theirs = doctored(&theirs[0], |fc| {
            let gid = fc.ids_before[i];
            fc.geometry
                .iter_mut()
                .find(|g| g.id == gid)
                .unwrap()
                .points
                .get_mut("1")
                .unwrap()[0] -= by;
        });
        verdict(&ours, Some(&theirs))
    };
    agrees(&nudged(0.9e-8));
    agrees(&nudged(1.1e-8));
    // Silico's p0000008 moved before the solve by exactly 1e-8, across or up: within the
    // comparison's 1e-8, so not named.
    for axis in [0, 1] {
        let moved = changed(&case, p, |sketch| {
            let q = sketch
                .points
                .iter_mut()
                .find(|q| q.id == id("p0000008"))
                .unwrap();
            let at = if axis == 0 { &mut q.x } else { &mut q.y };
            *at = silico_core::document::Coordinate::new(1e-8).unwrap();
        });
        agrees(&verdict(&moved, theirs[0].as_ref()));
    }
    // Silico's p0000007 moved before the solve, across or up: each named.
    for (axis, by) in [(0, 1e-6), (1, -1e-6)] {
        let moved = changed(&case, p, |sketch| {
            let q = sketch
                .points
                .iter_mut()
                .find(|q| q.id == id("p0000007"))
                .unwrap();
            let at = if axis == 0 { &mut q.x } else { &mut q.y };
            *at = silico_core::document::Coordinate::new(at.mm() + by).unwrap();
        });
        let v = verdict(&moved, theirs[0].as_ref());
        assert!(
            v.how.iter().any(|h| h
                .starts_with("after the edit, before the solve, Silico's point p0000007 is at")),
            "{axis}: {v}"
        );
        // Neither solve moves it from where it started, so after the solve it's compared,
        // 1e-6 apart, and reported.
        assert!(
            v.how.iter().any(|h| h.starts_with(
                "after the solve, Silico's point p0000007, which neither solve moved, is at"
            )),
            "{axis}: {v}"
        );
    }
}

/// The arc a0000004 about p0000001 (0, 0), radius 3, and the point p0000005 (-3, 0) held on
/// it by k0000006; the centre and that point merged, the centre kept, so the `on` would hold
/// the arc's own centre on it.
fn holds_own_centre() -> EditCase {
    case_of(
        "merge_holds_own_centre",
        r#""points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 3, "y": 0, "construction": false}, {"id": "p0000003", "x": 0, "y": 3, "construction": false}, {"id": "p0000005", "x": -3, "y": 0, "construction": false}],
        "lines": [], "circles": [], "arcs": [{"id": "a0000004", "center": "p0000001", "start": "p0000002", "end": "p0000003", "construction": false}],
        "constraints": [{"id": "k0000006", "type": "on", "point": "p0000005", "curve": "a0000004"}]"#,
        Edit::Merge {
            points: vec!["p0000001".to_owned(), "p0000005".to_owned()],
        },
    )
}

#[test]
fn a_merge_dropping_an_implied_on_is_excused_only_where_freecad_rolls_back_just_its_redundancy() {
    // merge_holds_own_point_on_line, and `sketch-edit-sweep --seed 32 --count 500`'s case 490:
    // a merge after which an `on` would hold a line's own end on the line, which Silico drops
    // as implied, listing it in `removed` (decision 13b, #se-an-on-the-merge-makes-hold). FreeCAD
    // 1.1.3 keeps the PointOnObject, its solve finding just it redundant (-2), which its Coincident
    // command rolls back (#se-any-result-whose-solve-is-worse).
    let mut cases = cases(&["merge_holds_own_point_on_line"]);
    let mut sweep = silico_oracle::generate::sketch_edit_cases(32, 500).unwrap();
    cases.push(sweep.swap_remove(490));
    assert_eq!(cases[1].name, "edit_32_490");
    let mut worker = SketchWorker::start().unwrap();
    let prepared: Vec<Prepared> = cases
        .iter()
        .map(|c| prepare(c, &mut worker).unwrap())
        .collect();
    let theirs = freecad_outcomes(&prepared).unwrap();
    for (p, fc) in prepared.iter().zip(&theirs) {
        let v = verdict(p, fc.as_ref());
        agrees(&v);
        assert!(
            v.excused.len() == 1
                && v.excused[0].starts_with("a merge that drops the `on`")
                && v.modulo
                    == [
                        "se-any-result-whose-solve-is-worse",
                        "se-an-on-the-merge-makes-hold"
                    ],
            "{v:?}"
        );
    }
    let p = &prepared[0];
    let SilicoSide::Edited(s) = &p.silico else {
        panic!("{:?}", p.silico);
    };
    assert!(s.edited.removed.contains(&id("k0000005")), "{:?}", s.edited);
    let Some(Theirs::Edited(fc)) = &theirs[0] else {
        panic!("{:?}", theirs[0]);
    };
    let on = fc
        .constraints
        .iter()
        .position(|c| c.name == "s_k0000005")
        .unwrap()
        + 1;
    assert_eq!((fc.after.code, &fc.after.redundant[..]), (-2, &[on][..]));
    // FreeCAD otherwise: another constraint redundant too, one conflicting, its solve
    // succeeding or failing, the sketch not clean before the merge, or the sketch not as it
    // was (a geometry made): a difference each.
    let dropped = |v: &Verdict| {
        v.excused
            .iter()
            .any(|e| e.starts_with("a merge that drops"))
    };
    let other = if on == 1 { 2 } else { 1 };
    let changes: Vec<(&str, Doctoring)> = vec![
        (
            "another constraint redundant too",
            Box::new(move |fc| fc.after.redundant.push(other)),
        ),
        (
            "a constraint conflicting",
            Box::new(move |fc| fc.after.conflicting.push(other)),
        ),
        (
            "its solve succeeding",
            Box::new(|fc| fc.after = fc.before.clone()),
        ),
        (
            "its solve failing (-1), the `on` redundant still",
            Box::new(|fc| fc.after.code = -1),
        ),
        (
            "the sketch not solving cleanly before the merge",
            Box::new(|fc| fc.before.code = -1),
        ),
        (
            "a geometry made",
            Box::new(|fc| {
                let mut made = fc.geometry[0].clone();
                made.id = 999;
                fc.geometry.push(made.clone());
                fc.edited.push(made);
            }),
        ),
    ];
    for (what, change) in changes {
        let v = verdict(p, Some(&doctored(&theirs[0], |fc| change(fc))));
        assert!(!dropped(&v) && !v.how.is_empty(), "{what}: {v:?}");
    }
    // Silico's merge not listing the `on` as removed: a difference.
    let mut kept = s.clone();
    kept.edited.removed.retain(|r| *r != id("k0000005"));
    let kept = Prepared {
        silico: SilicoSide::Edited(kept),
        ..p.clone()
    };
    let v = verdict(&kept, theirs[0].as_ref());
    assert!(!dropped(&v) && !v.how.is_empty(), "{v:?}");
}

// ---- extend (section 7) ----

#[test]
fn the_notes_extend_probe_slides_the_line_to_5_15_on_both_sides() {
    // A horizontal 10 mm line with its length, its end extended by 5: the drag pulls the
    // end toward (15, 0); the length holds 10 and horizontal holds y, so the start follows
    // to (5, 0) (section 7's 1.1.3 probe).
    let (_, prepared, theirs) = both(&["extend_probe_line_end_by_5"]);
    let SilicoSide::Edited(s) = &prepared[0].silico else {
        panic!("{:?}", prepared[0].silico);
    };
    let at = |name: &str| {
        let p = s
            .edited
            .sketch
            .points
            .iter()
            .find(|p| p.id == id(name))
            .unwrap();
        [p.x.mm(), p.y.mm()]
    };
    for (got, want) in [(at("p0000001"), [5.0, 0.0]), (at("p0000002"), [15.0, 0.0])] {
        assert!(
            (got[0] - want[0]).abs() <= 1e-9 && (got[1] - want[1]).abs() <= 1e-9,
            "{got:?} against {want:?}"
        );
    }
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert!(v.excused.is_empty(), "{v:?}");
}

/// `p` with Silico's refusal changed by `change`.
fn refused_otherwise(p: &Prepared, change: impl FnOnce(&mut Refusal)) -> Prepared {
    let SilicoSide::Refused(r) = &p.silico else {
        panic!("{}: Silico didn't refuse it: {:?}", p.name, p.silico);
    };
    let mut r = r.clone();
    change(&mut r);
    Prepared {
        silico: SilicoSide::Refused(r),
        ..p.clone()
    }
}

/// A change to a refusal, named.
type Change = Box<dyn Fn(&mut Refusal)>;

fn reported(v: &Verdict, why: &str) {
    assert!(!v.how.is_empty() && v.excused.is_empty(), "{why}: {v:?}");
}

#[test]
fn an_extend_refused_for_another_reason_than_the_notes_is_a_difference() {
    // Each extend refusal the note gives, as Silico makes it, with one thing about it changed:
    // its code, its field, the curve it names, or its words. None is the note's any more, but
    // a held end in other words, which is matched by what it names.
    let names = [
        "extend_held_end",
        "extend_probe_line_by_0",
        "extend_free_line_flipped",
        "extend_free_line_to_nothing",
        "extend_arc_inverted",
        "extend_arc_whole_turn",
        "extend_arc_past_a_whole_turn",
        "extend_arc_turn_too_small",
        "extend_collapses_neighbour",
        "extend_arc_collapses_neighbour",
        "extend_circle",
    ];
    let (_, prepared, theirs) = both(&names);
    for (p, fc) in prepared.iter().zip(&theirs) {
        let v = verdict(p, fc.as_ref());
        assert!(rule(p).is_some(), "{}: {:?}", p.name, p.silico);
        agrees(&v);
        let changes: [(&str, Change); 4] = [
            ("its code", Box::new(|r| r.code = "internal".to_owned())),
            (
                "its field",
                Box::new(|r| r.parameter = Some("elsewhere".to_owned())),
            ),
            (
                "the curve",
                Box::new(|r| r.elements[0] = "n0009999".to_owned()),
            ),
            (
                "its words",
                Box::new(|r| r.message = "an unrelated refusal".to_owned()),
            ),
        ];
        for (what, change) in &changes {
            let wrong = refused_otherwise(p, change);
            // A held end is matched by its code and the line and end it names, not its
            // words, so a reword can't lose it: the same rule, in other words.
            if p.name == "extend_held_end" && *what == "its words" {
                assert_eq!(rule(&wrong), Some(Rule::ExtendHeld), "{} by {what}", p.name);
                continue;
            }
            assert!(rule(&wrong).is_none(), "{} by {what}", p.name);
            reported(
                &verdict(&wrong, fc.as_ref()),
                &format!("{} by {what}", p.name),
            );
        }
    }
    // A held end or a frame that can't be built names the moving end: another point isn't.
    for i in [0, 8] {
        let wrong = refused_otherwise(&prepared[i], |r| r.elements[1] = "p0009999".to_owned());
        assert!(rule(&wrong).is_none(), "{}", prepared[i].name);
    }
    // Within 1e-7 mm of the bounds, on the arc of radius 5: 1e-6 degrees short of a whole
    // turn, 8.7e-8 mm, and 1e-6 degrees of sweep left, each still the note's.
    for (i, by) in [
        (5, ExtendBy::Angle(269.999999)),
        (4, ExtendBy::Angle(-89.999999)),
    ] {
        let Edit::Extend { curve, end, .. } = &prepared[i].edit else {
            panic!("{}", prepared[i].name);
        };
        let near = Prepared {
            edit: Edit::Extend {
                curve: curve.clone(),
                end: *end,
                by,
            },
            ..prepared[i].clone()
        };
        assert!(rule(&near).is_some(), "{} by {by:?}", prepared[i].name);
    }
    // Just outside: 1e-5 degrees short of a whole turn, 8.7e-7 mm, and 1e-5 degrees of sweep
    // left: neither is the note's.
    for (i, by) in [
        (5, ExtendBy::Angle(269.99999)),
        (4, ExtendBy::Angle(-89.99999)),
    ] {
        let Edit::Extend { curve, end, .. } = &prepared[i].edit else {
            panic!("{}", prepared[i].name);
        };
        let outside = Prepared {
            edit: Edit::Extend {
                curve: curve.clone(),
                end: *end,
                by,
            },
            ..prepared[i].clone()
        };
        assert!(rule(&outside).is_none(), "{} by {by:?}", prepared[i].name);
    }
    // The increment: by 1, not 0; a turn of 1e-6 degrees on a radius of 1, 1.7e-8 mm, more
    // than the 1e-9 the note's "too small" is; by -60 degrees, which leaves a 30 degree arc;
    // by 200, a 290 degree one. Each refusal then isn't the note's.
    for (i, by) in [
        (1, ExtendBy::Length(1.0)),
        (7, ExtendBy::Angle(1e-6)),
        (4, ExtendBy::Angle(-60.0)),
        (5, ExtendBy::Angle(200.0)),
    ] {
        let Edit::Extend { curve, end, .. } = &prepared[i].edit else {
            panic!("{}", prepared[i].name);
        };
        let wrong = Prepared {
            edit: Edit::Extend {
                curve: curve.clone(),
                end: *end,
                by,
            },
            ..prepared[i].clone()
        };
        assert!(rule(&wrong).is_none(), "{} by {by:?}", prepared[i].name);
    }
}

#[test]
fn an_extend_excused_only_where_freecads_result_is_the_notes() {
    let (_, prepared, theirs) = both(&[
        "extend_held_end",
        "extend_probe_line_by_0",
        "extend_arc_turn_too_small",
        "extend_free_line_flipped",
        "extend_free_line_to_nothing",
        "extend_collapses_neighbour",
        "extend_arc_collapses_neighbour",
    ]);
    // The flipped line's and the line taken to nothing's refusals asked by -5, a target the
    // request leaves 5 mm long: so a drag's result, not the request's own target, is what's
    // refused (#se-refuse-a-result-of-size-1e), and FreeCAD's result decides.
    let by_minus_5 = |p: &Prepared| {
        let Edit::Extend { curve, end, .. } = &p.edit else {
            panic!("not an extend");
        };
        Prepared {
            edit: Edit::Extend {
                curve: curve.clone(),
                end: *end,
                by: ExtendBy::Length(-5.0),
            },
            ..p.clone()
        }
    };
    let (flipped, to_nothing) = (by_minus_5(&prepared[3]), by_minus_5(&prepared[4]));
    agrees(&verdict(&flipped, theirs[3].as_ref()));
    agrees(&verdict(&to_nothing, theirs[4].as_ref()));
    // "Nothing moved" and "leaves the sketch as it was" hold of the whole sketch: FreeCAD's
    // line start, not the extended end, moved 1e-6 mm, and each is a difference.
    for i in [0, 1, 4, 5] {
        let p = if i == 4 { &to_nothing } else { &prepared[i] };
        let moved = doctored(&theirs[i], |fc| {
            let curve = match &p.edit {
                Edit::Extend { curve, .. } => curve.clone(),
                _ => unreachable!(),
            };
            let g = edited_geometry(p, fc, &curve);
            g.points.get_mut("1").unwrap()[0] += 1e-6;
        });
        reported(&verdict(p, Some(&moved)), &p.name);
    }
    // The arc's turn too small, its radius changed by FreeCAD.
    let p = &prepared[2];
    let grown = doctored(&theirs[2], |fc| {
        *edited_geometry(p, fc, "a0000004").radius.as_mut().unwrap() += 1e-6;
    });
    reported(&verdict(p, Some(&grown)), &p.name);
    // The arc's turn too small, with its centre moved by FreeCAD.
    let p = &prepared[2];
    let moved = doctored(&theirs[2], |fc| {
        edited_geometry(p, fc, "a0000004")
            .points
            .get_mut("3")
            .unwrap()[1] += 1e-6;
    });
    reported(&verdict(p, Some(&moved)), &p.name);
    // A sound target, FreeCAD's line left 1e-8 mm long (the note's), or 1e-6 (not).
    for (left, excused) in [(1e-8, true), (1e-6, false)] {
        let made = doctored(&theirs[4], |fc| {
            let line = edited_geometry(&to_nothing, fc, "n0000003");
            let start = line.points["1"];
            *line.points.get_mut("2").unwrap() = [start[0] + left, start[1]];
        });
        let v = verdict(&to_nothing, Some(&made));
        assert_eq!(v.how.is_empty(), excused, "{left} mm: {v:?}");
    }
    // A line FreeCAD left running the same way, 5 mm long, where the note's is flipped.
    let p = &flipped;
    let same_way = doctored(&theirs[3], |fc| {
        *edited_geometry(p, fc, "n0000003")
            .points
            .get_mut("2")
            .unwrap() = [5.0, 0.0];
    });
    reported(&verdict(p, Some(&same_way)), &p.name);
    // A frame that can't be built: FreeCAD's drag leaving B 1e-6 mm long is no collapse (and
    // moved the sketch), where B 1e-8 mm long is the note's.
    let p = &prepared[5];
    for (left, excused) in [(1e-6, false), (1e-8, true)] {
        let made = doctored(&theirs[5], |fc| {
            let b = edited_geometry(p, fc, "n0000005");
            let start = b.points["1"];
            *b.points.get_mut("2").unwrap() = [start[0] + left, start[1]];
        });
        let v = verdict(p, Some(&made));
        assert_eq!(v.how.is_empty(), excused, "B {left} mm long: {v:?}");
    }
    // A gated arc's extend at a shared end names the shared point; one that doesn't isn't
    // the note's. FreeCAD 1.1.3's solve there collapses the line in some runs and not in
    // others, so its result is given the line as drawn, no curve 1e-7 mm or less, and only
    // the refusal decides.
    let p = &prepared[6];
    let sound = doctored(&theirs[6], |fc| {
        let i = p
            .model
            .elements
            .iter()
            .position(|e| e == "n0000006")
            .unwrap();
        let id = fc.ids_before[i];
        let line = fc.geometry.iter_mut().find(|g| g.id == id).unwrap();
        *line.points.get_mut("1").unwrap() = [0.0, 5.0];
        *line.points.get_mut("2").unwrap() = [-5.0, 0.0];
    });
    agrees(&verdict(p, Some(&sound)));
    let wrong = refused_otherwise(p, |r| r.elements.retain(|e| e != "p0000003"));
    reported(&verdict(&wrong, Some(&sound)), &p.name);
    // Nor one naming the shared point but not the neighbour that shares it.
    let wrong = refused_otherwise(p, |r| r.elements.retain(|e| e != "n0000006"));
    reported(&verdict(&wrong, Some(&sound)), &p.name);
}

#[test]
fn a_request_whose_own_target_flips_the_line_is_refused_before_the_drag() {
    // The probe's line, 10 long and held so, its end by -15: the request's own target flips
    // it, so Silico refuses it before any drag (#se-refuse-a-result-of-size-1e,
    // #se-an-increment-whose-target-is-1e, Worker 1's #270), where FreeCAD drags toward it and
    // keeps what the constraints leave, unflipped.
    let (_, prepared, theirs) = both(&["extend_probe_line_end_by_minus_15"]);
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    // FreeCAD's line slid to (-15, 0)-(-5, 0), unflipped.
    let Some(Theirs::Edited(fc)) = &theirs[0] else {
        panic!("FreeCAD didn't make the edit: {:?}", theirs[0]);
    };
    let mut fc = fc.clone();
    let line = edited_geometry(&prepared[0], &mut fc, "n0000003");
    for (pos, want) in [("1", [-15.0, 0.0]), ("2", [-5.0, 0.0])] {
        let at = line.points[pos];
        assert!(
            libm::hypot(at[0] - want[0], at[1] - want[1]) <= 1e-8,
            "{pos}: {at:?}"
        );
    }
    assert!(
        v.excused[0].starts_with("an extend whose own target leaves the line"),
        "{v:?}"
    );
    // The same refusal of an increment of -5, whose target leaves 5 mm: not the note's.
    let Edit::Extend { curve, end, .. } = &prepared[0].edit else {
        panic!("not an extend");
    };
    let wrong = Prepared {
        edit: Edit::Extend {
            curve: curve.clone(),
            end: *end,
            by: ExtendBy::Length(-5.0),
        },
        ..prepared[0].clone()
    };
    reported(&verdict(&wrong, theirs[0].as_ref()), "by -5");
}

#[test]
fn a_drag_a_reference_reaches_compares_all_it_can() {
    // extend_line_with_reference_length: FreeCAD's drag settles otherwise each run, so no
    // place before the solve, and after it only points neither solve moved, are compared; but
    // everything else is.
    let (cases, prepared, theirs) = both(&["extend_line_with_reference_length"]);
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert!(
        v.excused
            .iter()
            .any(|e| e.starts_with("harness limit: FreeCAD 1.1.3's drag of line n0000003")),
        "{v:?}"
    );
    // Silico's result without the horizontal: reported.
    let p = changed(&cases[0], &prepared[0], |s| {
        s.constraints.retain(|c| c.id != id("k0000004"));
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert!(
        v.how
            .iter()
            .any(|h| h == "FreeCAD has Horizontal(n0000003) (from k0000004), Silico hasn't"),
        "{v:?}"
    );
    // The lone point, which neither solve moves, elsewhere: reported after the solve.
    let p = changed(&cases[0], &prepared[0], |s| {
        let lone = s
            .points
            .iter_mut()
            .find(|q| q.id == id("p0000006"))
            .unwrap();
        lone.x = silico_core::document::Coordinate::new(21.0).unwrap();
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "after the solve, Silico's point p0000006, which neither solve moved, is at [21.0, 20.0]"
        )),
        "{v:?}"
    );
    // The dragged end elsewhere before the solve: FreeCAD's isn't reproducible there, so not
    // compared; but Silico's own end off the request's target is Silico's, and reported
    // (#se-extend-built), nothing else before the solve.
    let p = changed(&cases[0], &prepared[0], |s| {
        let end = s
            .points
            .iter_mut()
            .find(|q| q.id == id("p0000002"))
            .unwrap();
        end.x = silico_core::document::Coordinate::new(14.5).unwrap();
    });
    let v = verdict(&p, theirs[0].as_ref());
    let before: Vec<&String> = v
        .how
        .iter()
        .filter(|h| h.starts_with("after the edit, before the solve"))
        .collect();
    assert!(
        before.len() == 1
            && before[0].starts_with(
                "after the edit, before the solve, Silico's p0000002, the line's moving end, is at [14.5, 0.0], not the request's target [14.0, 0.0] exactly"
            ),
        "{v:?}"
    );
}

#[test]
fn a_reference_that_doesnt_reach_the_line_leaves_its_drag_compared_in_full() {
    // extend_line_beside_a_reference: the reference length is on another line, joined to
    // the extended one by nothing; the drag is compared as any, its end before the solve.
    let (cases, prepared, theirs) = both(&["extend_line_beside_a_reference"]);
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert!(v.excused.is_empty(), "{v:?}");
    let p = changed(&cases[0], &prepared[0], |s| {
        let end = s
            .points
            .iter_mut()
            .find(|q| q.id == id("p0000002"))
            .unwrap();
        end.x = silico_core::document::Coordinate::new(14.5).unwrap();
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "after the edit, before the solve, Silico's p0000002, the line's moving end, is at [14.5, 0.0], not the request's target"
        )),
        "{v:?}"
    );
}

#[test]
fn a_reference_reaching_the_line_through_a_shared_point_is_a_harness_limit() {
    // extend_line_joined_to_a_reference: the reference length is on a line that shares the
    // extended one's start, so it reaches it through that point.
    let (_, prepared, theirs) = both(&["extend_line_joined_to_a_reference"]);
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert!(
        v.excused
            .iter()
            .any(|e| e.starts_with("harness limit: FreeCAD 1.1.3's drag of line n0000003")),
        "{v:?}"
    );
}

#[test]
fn a_shared_arc_ends_places_are_checked_where_both_settle_them() {
    // The note's arc-order probe (#se-a-line-s-end-goes-through): the arc's end, which the line
    // shares, turned 30 degrees on. Silico's shared point is compared with FreeCAD's arc end before
    // the solve, and FreeCAD's line end, which its setRange leaves, with where the case drew it.
    let (cases, prepared, theirs) = both(&["extend_arc_order_probe"]);
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    // Silico's shared point elsewhere: reported.
    let p = changed(&cases[0], &prepared[0], |s| {
        let end = s
            .points
            .iter_mut()
            .find(|q| q.id == id("p0000003"))
            .unwrap();
        end.x = silico_core::document::Coordinate::new(-6.0).unwrap();
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert!(
        v.how.iter().any(|h| h
            .starts_with("after the edit, before the solve, Silico's point p0000003 is at [-6.0,")),
        "{v:?}"
    );
    // The line's far end, which neither solve moves, elsewhere: reported after the solve.
    let p = changed(&cases[0], &prepared[0], |s| {
        let end = s
            .points
            .iter_mut()
            .find(|q| q.id == id("p0000005"))
            .unwrap();
        end.x = silico_core::document::Coordinate::new(-11.0).unwrap();
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "after the solve, Silico's point p0000005, which neither solve moved, is at [-11.0, 10.0]"
        )),
        "{v:?}"
    );
    // FreeCAD's solve failing there (-1): it solved in 10 of 10 runs, so not a case that
    // varies (only extend_32_115 is), and a difference.
    let v = verdict(
        &prepared[0],
        Some(&doctored(&theirs[0], |fc| fc.after.code = -1)),
    );
    assert!(!v.how.is_empty(), "{v:?}");
    assert!(
        !v.excused
            .iter()
            .any(|e| e.starts_with("harness limit: FreeCAD 1.1.3's solve after setRange")),
        "{v:?}"
    );
    // But only the failure: a conflict FreeCAD finds (-3), or its degrees of freedom
    // otherwise, each found before FreeCAD solves, is reported.
    let changes: [(&str, Doctor); 3] = [
        ("a conflict's code alone", |fc| fc.after.code = -3),
        ("a conflict", |fc| {
            fc.after.code = -3;
            fc.after.conflicting = vec![1];
        }),
        ("another dof", |fc| {
            fc.after.code = -1;
            fc.after.dof += 1;
        }),
    ];
    for (what, change) in changes {
        let v = verdict(&prepared[0], Some(&doctored(&theirs[0], change)));
        assert!(!v.how.is_empty(), "{what}: {v:?}");
    }
    // Nor where Silico's solve failed too: then FreeCAD's failure is no harness limit's.
    let SilicoSide::Edited(edited) = &prepared[0].silico else {
        panic!("Silico didn't extend it: {:?}", prepared[0].silico);
    };
    let mut edited = edited.clone();
    edited.ours.outcome = Err(silico_core::Error::new(
        silico_core::ErrorCode::SolveFailed,
        "doctored",
    ));
    let both_failed = Prepared {
        silico: SilicoSide::Edited(edited),
        ..prepared[0].clone()
    };
    let v = verdict(
        &both_failed,
        Some(&doctored(&theirs[0], |fc| fc.after.code = -1)),
    );
    assert!(
        !v.excused
            .iter()
            .any(|e| e.starts_with("harness limit: FreeCAD 1.1.3's solve after setRange")),
        "{v:?}"
    );
    // FreeCAD's line end moved by its edit: not what setRange does, reported.
    let moved = doctored(&theirs[0], |fc| {
        edited_geometry(&prepared[0], fc, "n0000006")
            .points
            .get_mut("1")
            .unwrap()[0] += 1e-3;
    });
    let v = verdict(&prepared[0], Some(&moved));
    assert!(
        v.how
            .iter()
            .any(|h| h.contains("not where the case drew it")),
        "{v:?}"
    );
}

#[test]
fn freecads_failed_solve_is_excused_only_at_a_shared_arc_end() {
    // extend_arc_end_by_30 shares no end: FreeCAD's solve failing there is a difference.
    let (_, prepared, theirs) = both(&["extend_arc_end_by_30"]);
    let failed = doctored(&theirs[0], |fc| fc.after.code = -1);
    reported(
        &verdict(&prepared[0], Some(&failed)),
        "extend_arc_end_by_30",
    );
}

#[test]
fn an_extended_arc_moved_elsewhere_by_silico_is_reported() {
    // extend_arc_end_by_30: the end turned to 120 degrees, (-2.5, 4.330127); Silico's result
    // with it at 125 instead is reported before and after the solve.
    let (cases, prepared, theirs) = both(&["extend_arc_end_by_30"]);
    agrees(&verdict(&prepared[0], theirs[0].as_ref()));
    let p = changed(&cases[0], &prepared[0], |s| {
        let a = 125f64.to_radians();
        let end = s
            .points
            .iter_mut()
            .find(|p| p.id == id("p0000003"))
            .unwrap();
        end.x = silico_core::document::Coordinate::new(5.0 * libm::cos(a)).unwrap();
        end.y = silico_core::document::Coordinate::new(5.0 * libm::sin(a)).unwrap();
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "after the edit, before the solve, Silico's point p0000003 is at [-2.867882182, 4.095760221], FreeCAD's a0000004's end at [-2.5, 4.330127019]"
        )),
        "{v:?}"
    );
}

#[test]
fn a_shared_arc_end_excuses_only_what_follows_from_it() {
    // extend_arc_start_with_tangent: the arc a0000006's start, p0000001, is the line
    // n0000003's end too. Excused by its pattern (#se-a-line-s-end-goes-through): the line's end
    // moving before the solve, and places after it that a solve moved.
    let (cases, prepared, theirs) = both(&["extend_arc_start_with_tangent"]);
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert!(
        v.modulo.iter().any(|c| c == "se-a-line-s-end-goes-through"),
        "{v:?}"
    );
    assert!(
        v.excused
            .iter()
            .any(|e| e.contains("#se-a-line-s-end-goes-through")),
        "{v:?}"
    );
    // Silico's result without the line's horizontal: another difference, reported.
    let p = changed(&cases[0], &prepared[0], |s| {
        s.constraints.retain(|c| c.id != id("k0000008"));
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert!(
        v.how
            .iter()
            .any(|h| h == "FreeCAD has Horizontal(n0000003) (from k0000008), Silico hasn't"),
        "{v:?}"
    );
    // And the arc's own end put elsewhere: reported before the solve.
    let p = changed(&cases[0], &prepared[0], |s| {
        let end = s
            .points
            .iter_mut()
            .find(|q| q.id == id("p0000005"))
            .unwrap();
        end.x = silico_core::document::Coordinate::new(4.0).unwrap();
    });
    let v = verdict(&p, theirs[0].as_ref());
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "after the edit, before the solve, Silico's point p0000005 is at [4.0, 3.0]"
        )),
        "{v:?}"
    );
}

#[test]
fn a_whole_turn_is_excused_only_where_freecads_range_wraps() {
    // extend_arc: its arc a0000004 extended at its end by 270 and by 300 degrees, a whole
    // turn and past one, which Silico refuses (#se-an-arc-swept-a-whole-turn). FreeCAD's range
    // wraps (#se-extend-unchecked): its arc after the edit sweeps the sweep plus the turn, less a
    // whole turn, about the same centre with the same radius.
    let (_, prepared, theirs) = both(&["extend_arc_whole_turn", "extend_arc_past_a_whole_turn"]);
    for (p, fc) in prepared.iter().zip(&theirs) {
        assert_eq!(rule(p), Some(Rule::ExtendWholeTurn), "{}", p.name);
        let v = verdict(p, fc.as_ref());
        agrees(&v);
        assert_eq!(v.excused.len(), 1, "{}: {v:?}", p.name);
        // FreeCAD's arc otherwise: its end 10 degrees on from the wrap, its centre moved, its
        // radius another: each a difference.
        let changes: [(&str, CaseDoctor); 4] = [
            (
                "the whole arc turned, its start, not extended, with it",
                |p, fc| {
                    let arc = edited_geometry(p, fc, "a0000004");
                    let c = arc.points["3"];
                    let (sin, cos) = (libm::sin(1e-6), libm::cos(1e-6));
                    for k in ["1", "2"] {
                        let q = arc.points.get_mut(k).unwrap();
                        let (dx, dy) = (q[0] - c[0], q[1] - c[1]);
                        *q = [c[0] + dx * cos - dy * sin, c[1] + dx * sin + dy * cos];
                    }
                },
            ),
            ("its end 10 degrees on", |p, fc| {
                let arc = edited_geometry(p, fc, "a0000004");
                let c = arc.points["3"];
                let e = arc.points.get_mut("2").unwrap();
                let (dx, dy) = (e[0] - c[0], e[1] - c[1]);
                let (sin, cos) = (libm::sin(10f64.to_radians()), libm::cos(10f64.to_radians()));
                *e = [c[0] + dx * cos - dy * sin, c[1] + dx * sin + dy * cos];
            }),
            ("its centre moved", |p, fc| {
                let arc = edited_geometry(p, fc, "a0000004");
                for q in arc.points.values_mut() {
                    q[0] += 1e-6;
                }
            }),
            ("its radius another", |p, fc| {
                let arc = edited_geometry(p, fc, "a0000004");
                arc.radius = arc.radius.map(|r| r + 1e-6);
            }),
        ];
        for (what, change) in changes {
            let made = doctored(fc, |fc| change(p, fc));
            refused_against_freecads_edit(&verdict(p, Some(&made)), what);
        }
        // FreeCAD refusing it: the note gives it the edit, so a difference.
        let v = verdict(p, Some(&Theirs::Failed("refused by hand".to_owned())));
        assert!(!v.refused && v.excused.is_empty(), "{v:?}");
        assert!(
            v.how[0].starts_with("both refused, but the note has FreeCAD make the edit"),
            "{v}"
        );
    }
}

#[test]
fn a_failed_solve_at_a_shared_arc_end_is_excused_only_where_freecad_varies() {
    // `sketch-extend-sweep --seed 32 --count 500`'s case 115: the arc a0000018's start,
    // p0000013, is the line n0000014's end too. FreeCAD 1.1.3's solve after setRange failed in
    // 5 of 10 runs (#se-a-line-s-end-goes-through). Its failure (-1), its geometry where setRange
    // left it: a harness limit, excused, and the points neither solve moved still compared after
    // it.
    let mut cases = silico_oracle::generate::sketch_extend_cases(32, 500).unwrap();
    let case = cases.swap_remove(115);
    assert_eq!(case.name, "extend_32_115");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let theirs = freecad_outcomes(&prepared).unwrap();
    let p = &prepared[0];
    let failed = Some(doctored(&theirs[0], |fc| {
        fc.after.code = -1;
        fc.geometry = fc.edited.clone();
    }));
    let v = verdict(p, failed.as_ref());
    agrees(&v);
    assert!(
        v.excused
            .iter()
            .any(|e| e.starts_with("harness limit: FreeCAD 1.1.3's solve after setRange")),
        "{v:?}"
    );
    // The free line n0000027's start moved 1 mm in Silico's result: no solve moves it, so
    // it's reported after the solve, FreeCAD's failed one included.
    let p = changed(&case, p, |s| {
        let start = s
            .points
            .iter_mut()
            .find(|q| q.id == id("p0000025"))
            .unwrap();
        start.x = silico_core::document::Coordinate::new(-21.777).unwrap();
    });
    let v = verdict(&p, failed.as_ref());
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "after the solve, Silico's point p0000025, which neither solve moved, is at [-21.777, -9.332]"
        )),
        "{v:?}"
    );
}

#[test]
fn freecad_refusing_an_extend_the_note_has_it_make_is_a_difference() {
    // Every extend of the case file Silico refuses: FreeCAD 1.1.3 makes each, reporting
    // success (#se-extend-unchecked, #se-an-increment-of-0-since-it to
    // #se-each-result-is-checked-by-the), but for a kind 1.1 refuses
    // (#se-other-kinds-and-the-mid-position) and an arc turned to no sweep, which OCCT raises
    // (#se-extend-unchecked), so its refusal of one isn't the note's; those two agree only with its
    // raising.
    let names = [
        "extend_probe_line_by_0",
        "extend_arc_by_0",
        "extend_arc_turn_too_small",
        "extend_held_end",
        "extend_collapses_neighbour",
        "extend_arc_collapses_neighbour",
        "extend_free_line_flipped",
        "extend_free_line_to_nothing",
        "extend_probe_line_end_by_minus_15",
        "extend_arc_to_nothing",
        "extend_arc_inverted",
        "extend_arc_whole_turn",
        "extend_arc_past_a_whole_turn",
        "extend_circle",
        "extend_point",
    ];
    let (_, prepared, theirs) = both(&names);
    let (_, _, made) = both(&["extend_arc_end_by_30"]);
    let made = made.into_iter().next().flatten();
    assert!(matches!(made, Some(Theirs::Edited(_))), "{made:?}");
    let mut seen = std::collections::BTreeSet::new();
    for (p, fc) in prepared.iter().zip(&theirs) {
        let Some(r) = rule(p) else {
            panic!("{}: not a rule's refusal: {:?}", p.name, p.silico);
        };
        agrees(&verdict(p, fc.as_ref()));
        seen.insert(format!("{r:?}"));
        let refused = verdict(p, Some(&Theirs::Failed("refused by hand".to_owned())));
        if matches!(r, Rule::ExtendKind | Rule::ExtendArcToNothing) {
            assert!(
                matches!(r.pinned(), Some(sketch_edit::Pinned::Raises(_))),
                "{}",
                p.name
            );
            // Raising with another text than the note's: a difference.
            assert!(
                !refused.refused && refused.how.len() == 1,
                "{}: {refused:?}",
                p.name
            );
            // Its hanging instead: a difference.
            assert!(
                !verdict(
                    p,
                    Some(&Theirs::Lost(
                        "FreeCAD took longer than 60 s building it".to_owned()
                    ))
                )
                .how
                .is_empty(),
                "{}",
                p.name
            );
            // Its making the edit instead (another case's edit, extend_arc_end_by_30's,
            // standing in): a difference, never excused.
            let v = verdict(p, made.as_ref());
            assert!(!v.refused && v.excused.is_empty(), "{}: {v:?}", p.name);
            assert_eq!(v.how.len(), 1, "{}: {v}", p.name);
            assert!(
                v.how[0].starts_with("Silico refused (")
                    && v.how[0].contains("; FreeCAD made the edit ("),
                "{}: {v}",
                p.name
            );
        } else if matches!(r, Rule::ExtendFrame | Rule::ExtendShortOrFlipped) {
            // Its drag failing as FreeCAD's `extend` raises it: the note's other outcome.
            assert_eq!(
                r.pinned(),
                Some(sketch_edit::Pinned::EditsOrDragFails),
                "{}",
                p.name
            );
            let drag = Theirs::Failed(
                "Not able to extend geometry with id : (0) for increment (-15) and point position (2)"
                    .to_owned(),
            );
            let v = verdict(p, Some(&drag));
            // A line's drag; an arc's extend never drags (#se-extend-lines), so its raising so
            // differs.
            let line = !p.name.contains("arc");
            assert_eq!(v.refused && v.how.is_empty(), line, "{}: {v:?}", p.name);
            assert!(
                !refused.refused && refused.how.len() == 1,
                "{}: {refused:?}",
                p.name
            );
            assert!(
                refused.how[0]
                    .starts_with("both refused, but the note has FreeCAD make the edit, or raise"),
                "{}: {refused}",
                p.name
            );
        } else {
            assert_eq!(r.pinned(), Some(sketch_edit::Pinned::Edits), "{}", p.name);
            assert!(
                !refused.refused && refused.excused.is_empty(),
                "{}: {refused:?}",
                p.name
            );
            assert_eq!(refused.how.len(), 1, "{}: {refused}", p.name);
            assert!(
                refused.how[0].starts_with("both refused, but the note has FreeCAD make the edit"),
                "{}: {refused}",
                p.name
            );
        }
    }
    // Every extend rule is among them.
    for want in [
        "ExtendKind",
        "ExtendArcToNothing",
        "ExtendZero",
        "ExtendTurnTooSmall",
        "ExtendHeld",
        "ExtendFrame",
        "ExtendShortOrFlipped",
        "ExtendGated",
        "ExtendWholeTurn",
    ] {
        assert!(seen.contains(want), "{want} not seen: {seen:?}");
    }
}

#[test]
fn an_extend_whose_solve_fails_from_the_written_end_is_refused_and_excused_where_freecads_solves() {
    // `sketch-extend-sweep --seed 43 --count 500`'s case 212: the arc a0000020's start, which
    // the line n0000016 shares and holds tangent and parallel to the horizontal n0000005,
    // turned 130.869 degrees on. Silico's solve fails from the written end
    // (#se-an-arc-swept-a-whole-turn), which refuses the extend; FreeCAD's solve succeeds, at a
    // different place each run (KNOWN). A hand-built sketch of the kind (an arc's start a line
    // shares, held tangent there and parallel to a fixed line, its centre held or free) solved at
    // every turn tried, 3 to 179 degrees, so the refusal is shown on the sweep's case.
    let mut sweep = silico_oracle::generate::sketch_extend_cases(43, 500).unwrap();
    let case = sweep.swap_remove(212);
    assert_eq!(case.name, "extend_43_212");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::ExtendSolveFails), "{:?}", p.silico);
    let theirs = freecad_outcomes(&prepared).unwrap();
    // FreeCAD's solve succeeding: excused; failing, or its edit raising: a difference.
    let solved = doctored(&theirs[0], |fc| fc.after.code = 0);
    let v = verdict(p, Some(&solved));
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    let failed = doctored(&theirs[0], |fc| fc.after.code = -1);
    // FreeCAD's solve succeeding, but its arc's other end, the end a0000020's, turned instead
    // of its start: not the request, a difference.
    let wrong_end = doctored(&theirs[0], |fc| {
        fc.after.code = 0;
        let arc = edited_geometry(p, fc, "a0000020");
        let (start, end) = (arc.points["1"], arc.points["2"]);
        let drawn_start = p
            .sketch
            .points
            .iter()
            .find(|q| q.id == id("p0000015"))
            .unwrap();
        *arc.points.get_mut("1").unwrap() = [drawn_start.x.mm(), drawn_start.y.mm()];
        *arc.points.get_mut("2").unwrap() = [end[0] + (start[0] - drawn_start.x.mm()), end[1]];
    });
    // FreeCAD's arc turned as requested, but its centre moved, or its radius changed, by
    // 1e-6 (past the comparison's 1e-8): not the request, a difference.
    let moved_centre = doctored(&theirs[0], |fc| {
        fc.after.code = 0;
        edited_geometry(p, fc, "a0000020")
            .points
            .get_mut("3")
            .unwrap()[0] += 1e-6;
    });
    let changed_radius = doctored(&theirs[0], |fc| {
        fc.after.code = 0;
        let arc = edited_geometry(p, fc, "a0000020");
        arc.radius = arc.radius.map(|r| r + 1e-6);
    });
    // Its start turned as requested, but its end moved 1e-6 too: not the request.
    let end_moved = doctored(&theirs[0], |fc| {
        fc.after.code = 0;
        edited_geometry(p, fc, "a0000020")
            .points
            .get_mut("2")
            .unwrap()[0] += 1e-6;
    });
    for other in [
        failed,
        wrong_end,
        moved_centre,
        changed_radius,
        end_moved,
        Theirs::Failed("refused by hand".to_owned()),
    ] {
        let v = verdict(p, Some(&other));
        assert!(v.excused.is_empty() && !v.how.is_empty(), "{v:?}");
    }
    // The same refusal for the arc's end: FreeCAD's arc with its end turned on by the
    // request's angle about the drawn centre, its start where it was drawn, is excused; its
    // start turned instead (FreeCAD's own edit of the start) is a difference.
    let mut at_end = p.clone();
    let Edit::Extend { end, by, .. } = &mut at_end.edit else {
        panic!("{:?}", at_end.edit)
    };
    *end = sketch_edit::ExtendEnd::End;
    let ExtendBy::Angle(degrees) = *by else {
        panic!("{by:?}")
    };
    let arc = p
        .sketch
        .arcs
        .iter()
        .find(|a| a.id == id("a0000020"))
        .unwrap();
    let place = |point: &ElementId| {
        let q = p.sketch.points.iter().find(|q| q.id == *point).unwrap();
        [q.x.mm(), q.y.mm()]
    };
    let (centre, start, end) = (place(&arc.center), place(&arc.start), place(&arc.end));
    let radius = libm::hypot(start[0] - centre[0], start[1] - centre[1]);
    let angle = libm::atan2(end[1] - centre[1], end[0] - centre[0]) + degrees.to_radians();
    let end_turned = doctored(&theirs[0], |fc| {
        fc.after.code = 0;
        let g = edited_geometry(&at_end, fc, "a0000020");
        *g.points.get_mut("1").unwrap() = start;
        *g.points.get_mut("2").unwrap() = [
            centre[0] + radius * libm::cos(angle),
            centre[1] + radius * libm::sin(angle),
        ];
        *g.points.get_mut("3").unwrap() = centre;
        g.radius = Some(radius);
    });
    let v = verdict(&at_end, Some(&end_turned));
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    // Its end turned as requested, but its start moved 1e-6 too: a difference.
    let Theirs::Edited(start_moved) = &end_turned else {
        unreachable!()
    };
    let mut start_moved = start_moved.clone();
    edited_geometry(&at_end, &mut start_moved, "a0000020")
        .points
        .get_mut("1")
        .unwrap()[1] += 1e-6;
    let v = verdict(&at_end, Some(&Theirs::Edited(start_moved)));
    assert!(v.excused.is_empty() && !v.how.is_empty(), "{v:?}");
    let v = verdict(&at_end, Some(&solved));
    assert!(v.excused.is_empty() && !v.how.is_empty(), "{v:?}");
}

#[test]
fn an_arcs_shared_start_turned_a_few_degrees_is_written_at_the_turned_angle_and_solves() {
    // The arc a0000009 about (10, 5), radius 5, from (10, 0) (270 degrees), the end of the
    // line n0000003, which is held tangent there (tangent_at) and parallel to the fixed
    // horizontal n0000006. Its start turned back 3 degrees is written at 267 degrees,
    // (10 + 5 cos 267, 5 + 5 sin 267), by hand (9.738320218785281, 0.006852326227130945),
    // and the solve succeeds, taking it back to the arc's bottom, where the line is level.
    let s = sketch(
        r#""points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}, {"id": "p0000004", "x": 0, "y": -5, "construction": false}, {"id": "p0000005", "x": 10, "y": -5, "construction": false}, {"id": "p0000007", "x": 10, "y": 5, "construction": false}, {"id": "p0000008", "x": 15, "y": 5, "construction": false}],
        "lines": [{"id": "n0000003", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n0000006", "from": "p0000004", "to": "p0000005", "construction": false}],
        "circles": [], "arcs": [{"id": "a0000009", "center": "p0000007", "start": "p0000002", "end": "p0000008", "construction": false}],
        "constraints": [{"id": "k0000010", "type": "horizontal", "line": "n0000006"}, {"id": "k0000011", "type": "parallel", "a": "n0000003", "b": "n0000006"}, {"id": "k0000012", "type": "tangent_at", "a": "n0000003", "b": "a0000009", "at": "p0000002", "direction": "same"}, {"id": "k0000013", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000014", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000015", "type": "distance_x", "a": "origin", "b": "p0000007", "value": 10, "driving": true}, {"id": "k0000016", "type": "distance_y", "a": "origin", "b": "p0000007", "value": 5, "driving": true}]"#,
    );
    let mut worker = SketchWorker::start().unwrap();
    let edit = Edit::Extend {
        curve: "a0000009".to_owned(),
        end: sketch_edit::ExtendEnd::Start,
        by: ExtendBy::Angle(3.0),
    };
    let edited = sketch_edit::silico_edit(&s, &edit, &mut worker)
        .unwrap()
        .unwrap();
    let start = edited
        .sketch
        .points
        .iter()
        .find(|q| q.id == id("p0000002"))
        .unwrap();
    let want = [9.738320218785281, 0.006852326227130945];
    assert!(
        (start.x.mm() - want[0]).abs() <= 1e-9 && (start.y.mm() - want[1]).abs() <= 1e-9,
        "({}, {})",
        start.x.mm(),
        start.y.mm()
    );
    let case = silico_oracle::sketch::SketchCase {
        name: "turned".into(),
        note: String::new(),
        sketch: edited.sketch.clone(),
    };
    let ours = silico_oracle::sketch::silico_solve(&case, &mut worker).unwrap();
    let solved = ours.outcome.unwrap().points[&id("p0000002")];
    assert!(
        (solved[0].mm() - 10.0).abs() <= 1e-9 && solved[1].mm().abs() <= 1e-9,
        "({}, {})",
        solved[0].mm(),
        solved[1].mm()
    );
}

#[test]
fn a_lines_drag_compares_the_points_it_moved_within_its_band_and_the_rest_exactly() {
    // `sketch-extend-sweep --seed 43 --count 500`'s case 425: the line n0000005's end
    // extended by 21.587 mm through the drag. FreeCAD's drag lands short of the target
    // otherwise each run, up to 1.6e-7 mm from Silico's (sketcher.md:51): within the band
    // (5e-7 mm) for the points the drag moved, the rest compared as ever.
    let mut sweep = silico_oracle::generate::sketch_extend_cases(43, 500).unwrap();
    let case = sweep.swap_remove(425);
    assert_eq!(case.name, "extend_43_425");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let theirs = freecad_outcomes(&prepared).unwrap();
    agrees(&verdict(p, theirs[0].as_ref()));
    // A line's extend whose solve failed is no ExtendSolveFails: that rule is an arc's,
    // whose end is written, not dragged (#se-an-arc-swept-a-whole-turn).
    let SilicoSide::Edited(s) = &p.silico else {
        panic!("{:?}", p.silico);
    };
    let mut failed = s.clone();
    failed.ours.outcome = Err(silico_core::Error::new(
        silico_core::ErrorCode::SolveFailed,
        "doctored",
    ));
    let failed = Prepared {
        silico: SilicoSide::Edited(failed),
        ..p.clone()
    };
    assert_eq!(rule(&failed), None);
    // Compared as the edit it is, not as the arc rule's refusal.
    let v = verdict(&failed, theirs[0].as_ref());
    assert!(
        !v.how.is_empty() && !v.how.iter().any(|h| h.contains("extend's solve refused")),
        "{v}"
    );
    // FreeCAD's dragged end (n0000005's end, p0000002), which the harness writes at the
    // target as decision (3) writes Silico's (#se-extend-built), 1e-6 mm further short, before the
    // solve and after: past the band, reported both times.
    let short = doctored(&theirs[0], |fc| {
        let g = &mut edited_geometry(p, fc, "n0000005").points;
        g.get_mut("2").unwrap()[0] -= 1e-6;
        let id = fc.ids_before[p
            .model
            .elements
            .iter()
            .position(|e| e == "n0000005")
            .unwrap()];
        let solved = fc.geometry.iter_mut().find(|g| g.id == id).unwrap();
        solved.points.get_mut("2").unwrap()[0] -= 1e-6;
    });
    let v = verdict(p, Some(&short));
    assert!(
        v.how
            .iter()
            .any(|h| h.starts_with("after the edit, before the solve, Silico's point p0000002")),
        "{v}"
    );
    assert!(
        v.how.iter().any(
            |h| h.starts_with("after the solve, Silico's point p0000002, which the drag moved")
        ),
        "{v}"
    );
    // A circle the drag doesn't move (c0000033, joined to nothing it moves) its radius 2e-8
    // mm off in FreeCAD's: past the comparison's tolerance, reported.
    let off = doctored(&theirs[0], |fc| {
        let circle = edited_geometry(p, fc, "c0000033");
        circle.radius = circle.radius.map(|r| r + 2e-8);
    });
    let v = verdict(p, Some(&off));
    assert!(
        v.how.iter().any(
            |h| h.starts_with("after the edit, before the solve, Silico's c0000033 has radius")
        ),
        "{v}"
    );
    // A point the drag doesn't move (the upright line n0000020's start, p0000018, joined to
    // nothing) 2e-8 mm off in FreeCAD's: past the comparison's tolerance, reported.
    let off = doctored(&theirs[0], |fc| {
        edited_geometry(p, fc, "n0000020")
            .points
            .get_mut("1")
            .unwrap()[0] += 2e-8;
    });
    let v = verdict(p, Some(&off));
    assert!(
        v.how
            .iter()
            .any(|h| h.starts_with("after the edit, before the solve, Silico's point p0000018")),
        "{v}"
    );
}

#[test]
fn a_lines_drag_that_moves_a_circles_centre_compares_its_solved_radius_and_misses_none() {
    // The line n0000002 from (0, 0) to (10, 0), its end p0000003 the centre of the circle
    // c0000004, radius 2: the line's end extended 5 mm drags the circle's centre to (15, 0),
    // its radius 2 still. FreeCAD's solved radius 1e-6 mm off, past the drag's band, is
    // reported after the solve, and 2e-7 off, within it, isn't; so is the circle, or the
    // line's end, missing from FreeCAD's solve, or its end 1e-6 off across the line. The
    // circle c0000005 about p0000006 (20, 20), radius 1, which the drag doesn't move, is
    // compared exactly.
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000003", "x": 10, "y": 0, "construction": false}, {"id": "p0000006", "x": 20, "y": 20, "construction": false}], "lines": [{"id": "n0000002", "from": "p0000001", "to": "p0000003", "construction": false}], "circles": [{"id": "c0000004", "center": "p0000003", "radius": 2, "construction": false}, {"id": "c0000005", "center": "p0000006", "radius": 1, "construction": false}], "arcs": [], "constraints": []}]}"#;
    let document = Checked::new(read(text.as_bytes()).unwrap())
        .unwrap()
        .document()
        .clone();
    let case = EditCase {
        name: "extend_drags_a_circles_centre".to_owned(),
        note: String::new(),
        document,
        feature: 0,
        edit: Edit::Extend {
            curve: "n0000002".to_owned(),
            end: sketch_edit::ExtendEnd::End,
            by: ExtendBy::Length(5.0),
        },
    };
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let SilicoSide::Edited(s) = &p.silico else {
        panic!("{:?}", p.silico)
    };
    let centre = s
        .edited
        .sketch
        .points
        .iter()
        .find(|q| q.id == id("p0000003"))
        .unwrap();
    assert_eq!([centre.x.mm(), centre.y.mm()], [15.0, 0.0]);
    let theirs = freecad_outcomes(&prepared).unwrap();
    agrees(&verdict(p, theirs[0].as_ref()));
    let solved = |fc: &FreecadEdited, element: &str| {
        let i = p.model.elements.iter().position(|e| e == element).unwrap();
        fc.ids_before[i]
    };
    let off = doctored(&theirs[0], |fc| {
        let circle = solved(fc, "c0000004");
        let g = fc.geometry.iter_mut().find(|g| g.id == circle).unwrap();
        g.radius = g.radius.map(|r| r + 1e-6);
    });
    let v = verdict(p, Some(&off));
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "after the solve, Silico's c0000004, whose points the drag moved, has radius"
        )),
        "{v}"
    );
    let within = doctored(&theirs[0], |fc| {
        let circle = solved(fc, "c0000004");
        let g = fc.geometry.iter_mut().find(|g| g.id == circle).unwrap();
        g.radius = g.radius.map(|r| r + 2e-7);
    });
    agrees(&verdict(p, Some(&within)));
    let still = doctored(&theirs[0], |fc| {
        let circle = solved(fc, "c0000005");
        let g = fc.geometry.iter_mut().find(|g| g.id == circle).unwrap();
        g.radius = g.radius.map(|r| r + 1e-6);
    });
    let v = verdict(p, Some(&still));
    assert!(v.how.iter().any(|h| h.contains("c0000005")), "{v}");
    // The dragged end, the line's end and the circle's centre, 1e-6 off across the line in
    // FreeCAD's, before the solve and after.
    let across = doctored(&theirs[0], |fc| {
        let (line, circle) = (solved(fc, "n0000002"), solved(fc, "c0000004"));
        for g in fc.edited.iter_mut().chain(fc.geometry.iter_mut()) {
            if g.id == line {
                g.points.get_mut("2").unwrap()[1] += 1e-6;
            } else if g.id == circle {
                g.points.get_mut("3").unwrap()[1] += 1e-6;
            }
        }
    });
    // FreeCAD's dragged end exactly the band (5e-7) across the line, before the solve or
    // after it, and the undragged start p0000001 exactly 1e-8 across before it: within, so
    // agreeing.
    let edge = |change: fn(&mut FcGeometry, bool)| {
        doctored(&theirs[0], |fc| {
            let line = solved(fc, "n0000002");
            let circle = solved(fc, "c0000004");
            for g in &mut fc.edited {
                if g.id == line || g.id == circle {
                    change(g, false);
                }
            }
            for g in &mut fc.geometry {
                if g.id == line || g.id == circle {
                    change(g, true);
                }
            }
        })
    };
    let at_the_band = |after: bool| -> fn(&mut FcGeometry, bool) {
        if after {
            |g, solved| {
                if solved {
                    let k = if g.kind == "line" { "2" } else { "3" };
                    g.points.get_mut(k).unwrap()[1] = 5e-7;
                }
            }
        } else {
            |g, solved| {
                if !solved {
                    let k = if g.kind == "line" { "2" } else { "3" };
                    g.points.get_mut(k).unwrap()[1] = 5e-7;
                }
            }
        }
    };
    for after in [false, true] {
        agrees(&verdict(p, Some(&edge(at_the_band(after)))));
    }
    agrees(&verdict(
        p,
        Some(&edge(|g, solved| {
            if !solved && g.kind == "line" {
                g.points.get_mut("1").unwrap()[0] = 1e-8;
            }
        })),
    ));
    let v = verdict(p, Some(&across));
    for want in [
        "after the edit, before the solve, Silico's point p0000003 is at",
        "after the solve, Silico's point p0000003, which the drag moved, is at",
    ] {
        assert!(v.how.iter().any(|h| h.starts_with(want)), "{want}: {v}");
    }
    // FreeCAD's end, which its drag left where it stopped, written at the request's target
    // by the harness, as decision (3) writes Silico's (#se-extend-built), at each of its places
    // (the line's end and the circle's centre); 4e-7 off it before the solve, within the band,
    // agrees.
    let Some(Theirs::Edited(fc)) = &theirs[0] else {
        panic!("{:?}", theirs[0]);
    };
    assert!(fc.applied.iter().any(|c| c == "se-extend-built"), "{fc:?}");
    let (line, circle) = (solved(fc, "n0000002"), solved(fc, "c0000004"));
    for g in &fc.edited {
        if g.id == line {
            assert_eq!(g.points["2"], [15.0, 0.0]);
        } else if g.id == circle {
            assert_eq!(g.points["3"], [15.0, 0.0]);
        }
    }
    let near = doctored(&theirs[0], |fc| {
        for g in &mut fc.edited {
            if g.id == line {
                g.points.get_mut("2").unwrap()[0] += 4e-7;
            } else if g.id == circle {
                g.points.get_mut("3").unwrap()[0] += 4e-7;
            }
        }
    });
    agrees(&verdict(p, Some(&near)));
    // Silico's own end 1e-11 mm off the target, well inside any tolerance: reported.
    let off = changed(&case, p, |sketch| {
        let q = sketch
            .points
            .iter_mut()
            .find(|q| q.id == id("p0000003"))
            .unwrap();
        q.x = silico_core::document::Coordinate::new(15.000_000_000_01).unwrap();
    });
    let v = verdict(&off, theirs[0].as_ref());
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "after the edit, before the solve, Silico's p0000003, the line's moving end, is at [15.00000000001, 0.0], not the request's target [15.0, 0.0] exactly"
        )),
        "{v}"
    );
    let no_circle = doctored(&theirs[0], |fc| {
        let circle = solved(fc, "c0000004");
        fc.geometry.retain(|g| g.id != circle);
    });
    let v = verdict(p, Some(&no_circle));
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "after the solve, Silico's c0000004, whose points the drag moved, has no radius"
        )),
        "{v}"
    );
    // The line and the circle both missing, p0000003 has no place in FreeCAD's solve.
    let no_end = doctored(&theirs[0], |fc| {
        let (line, circle) = (solved(fc, "n0000002"), solved(fc, "c0000004"));
        fc.geometry.retain(|g| g.id != line && g.id != circle);
    });
    let v = verdict(p, Some(&no_end));
    assert!(
        v.how.iter().any(|h| h.starts_with(
            "after the solve, Silico's point p0000003, which the drag moved, has no place"
        )),
        "{v}"
    );
}

#[test]
fn a_lines_drag_that_moves_an_arc_compares_its_radius_within_the_band() {
    // `sketch-extend-sweep --seed 45 --count 500`'s case 412: the drag moves the arc
    // a0000017's points, and FreeCAD's leaves its radius up to 3e-7 mm from Silico's
    // (sketcher.md:51): within the band, before the solve and after it. Its radius 1e-6 mm
    // off in FreeCAD's, past the band, is reported.
    let mut sweep = silico_oracle::generate::sketch_extend_cases(45, 500).unwrap();
    let case = sweep.swap_remove(412);
    assert_eq!(case.name, "extend_45_412");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let theirs = freecad_outcomes(&prepared).unwrap();
    agrees(&verdict(p, theirs[0].as_ref()));
    let off = doctored(&theirs[0], |fc| {
        let arc = edited_geometry(p, fc, "a0000017");
        arc.radius = arc.radius.map(|r| r + 1e-6);
    });
    let v = verdict(p, Some(&off));
    assert!(
        v.how.iter().any(
            |h| h.starts_with("after the edit, before the solve, Silico's a0000017 has radius")
        ),
        "{v}"
    );
}

#[test]
fn an_extends_band_and_tolerances_hold_at_their_edges_exactly() {
    // The line n0000002 from (-10, 0) to (-5, 0), its end p0000003 the centre of c0000004,
    // radius 1e-6; the lone point p0000005 at (0, 7). The end extended 5 mm lands at (0, 0)
    // exactly. Each difference below is exactly its tolerance in f64 (from 0, or 1e-6 less
    // 5e-7): within it, so agreeing.
    let case = case_of(
        "extend_to_the_origin",
        r#""points": [{"id": "p0000001", "x": -10, "y": 0, "construction": false}, {"id": "p0000003", "x": -5, "y": 0, "construction": false}, {"id": "p0000005", "x": 0, "y": 7, "construction": false}],
        "lines": [{"id": "n0000002", "from": "p0000001", "to": "p0000003", "construction": false}], "circles": [{"id": "c0000004", "center": "p0000003", "radius": 0.000001, "construction": false}], "arcs": [], "constraints": []"#,
        Edit::Extend {
            curve: "n0000002".to_owned(),
            end: sketch_edit::ExtendEnd::End,
            by: ExtendBy::Length(5.0),
        },
    );
    assert_eq!(1e-6 - 5e-7, 5e-7);
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let SilicoSide::Edited(s) = &p.silico else {
        panic!("{:?}", p.silico)
    };
    let end = s
        .edited
        .sketch
        .points
        .iter()
        .find(|q| q.id == id("p0000003"))
        .unwrap();
    assert_eq!([end.x.mm(), end.y.mm()], [0.0, 0.0]);
    let theirs = freecad_outcomes(&prepared).unwrap();
    agrees(&verdict(p, theirs[0].as_ref()));
    let index = |fc: &FreecadEdited, element: &str| {
        let i = p.model.elements.iter().position(|e| e == element).unwrap();
        fc.ids_before[i]
    };
    // FreeCAD's dragged end 5e-7 across after the solve; its dragged circle's radius 5e-7,
    // before the solve or after it.
    let end_across = doctored(&theirs[0], |fc| {
        let (line, circle) = (index(fc, "n0000002"), index(fc, "c0000004"));
        for g in &mut fc.geometry {
            if g.id == line {
                g.points.get_mut("2").unwrap()[0] = 5e-7;
            } else if g.id == circle {
                g.points.get_mut("3").unwrap()[0] = 5e-7;
            }
        }
    });
    agrees(&verdict(p, Some(&end_across)));
    for solved in [false, true] {
        let smaller = doctored(&theirs[0], |fc| {
            let circle = index(fc, "c0000004");
            let list = if solved {
                &mut fc.geometry
            } else {
                &mut fc.edited
            };
            list.iter_mut().find(|g| g.id == circle).unwrap().radius = Some(5e-7);
        });
        agrees(&verdict(p, Some(&smaller)));
    }
    // A line held horizontal, or vertical, at its length 10, its end extended 5: the drag
    // takes its start along (section 7's probe), on y = 0 (x = 0) exactly. FreeCAD's start
    // exactly the band across it before the solve is within; 6e-7, past it, is reported.
    for (name, held, end, axis) in [
        ("frame_horizontal", "horizontal", [10, 0], 1),
        ("frame_vertical", "vertical", [0, 10], 0),
    ] {
        let frame = case_of(
            name,
            &format!(
                r#""points": [{{"id": "p0000001", "x": 0, "y": 0, "construction": false}}, {{"id": "p0000002", "x": {}, "y": {}, "construction": false}}],
        "lines": [{{"id": "n0000003", "from": "p0000001", "to": "p0000002", "construction": false}}], "circles": [], "arcs": [],
        "constraints": [{{"id": "k0000004", "type": "{held}", "line": "n0000003"}}, {{"id": "k0000005", "type": "length", "line": "n0000003", "value": 10, "driving": true}}]"#,
                end[0], end[1]
            ),
            Edit::Extend {
                curve: "n0000003".to_owned(),
                end: sketch_edit::ExtendEnd::End,
                by: ExtendBy::Length(5.0),
            },
        );
        let prepared = vec![prepare(&frame, &mut worker).unwrap()];
        let p = &prepared[0];
        let SilicoSide::Edited(s) = &p.silico else {
            panic!("{:?}", p.silico)
        };
        let start = s
            .edited
            .sketch
            .points
            .iter()
            .find(|q| q.id == id("p0000001"))
            .unwrap();
        let start = [start.x.mm(), start.y.mm()];
        assert!(
            start[axis] == 0.0 && start[1 - axis] > 1.0,
            "{name}: {start:?}"
        );
        let theirs = freecad_outcomes(&prepared).unwrap();
        agrees(&verdict(p, theirs[0].as_ref()));
        let across = |by: f64| {
            doctored(&theirs[0], |fc| {
                edited_geometry(p, fc, "n0000003")
                    .points
                    .get_mut("1")
                    .unwrap()[axis] = by;
            })
        };
        agrees(&verdict(p, Some(&across(5e-7))));
        let v = verdict(p, Some(&across(6e-7)));
        assert!(
            v.how.iter().any(|h| h
                .starts_with("after the edit, before the solve, Silico's point p0000001 is at")),
            "{name}: {v}"
        );
    }
    // Silico's lone p0000005, or the line's start, moved by exactly 1e-8 (from 0): not moved
    // by the drag, so FreeCAD's 2e-7 there, before the solve, is past the comparison's 1e-8.
    for (point, axis) in [("p0000005", 0), ("p0000001", 1)] {
        let moved = changed(&case, p, |sketch| {
            let q = sketch
                .points
                .iter_mut()
                .find(|q| q.id == id(point))
                .unwrap();
            let at = if axis == 0 { &mut q.x } else { &mut q.y };
            *at = silico_core::document::Coordinate::new(1e-8).unwrap();
        });
        let theirs_moved = doctored(&theirs[0], |fc| {
            let (member, k) = if point == "p0000005" {
                ("p0000005", "1")
            } else {
                ("n0000002", "1")
            };
            let id = index(fc, member);
            fc.edited
                .iter_mut()
                .find(|g| g.id == id)
                .unwrap()
                .points
                .get_mut(k)
                .unwrap()[axis] = 2e-7;
        });
        let v = verdict(&moved, Some(&theirs_moved));
        let want = format!("after the edit, before the solve, Silico's point {point} is at");
        assert!(v.how.iter().any(|h| h.starts_with(&want)), "{want}: {v}");
    }
}

#[test]
fn a_shared_arc_ends_neighbour_is_where_the_case_drew_it_within_1e_8_exactly() {
    // The arc a0000003 about (0, 5) from p0000001 (0, 0) to (5, 5), the line n0000006 from
    // (-10, 0) ending at p0000001; the arc's start turned 10 degrees back. FreeCAD's
    // setRange leaves the line's end where the case drew it; 1e-8 off it (exactly, from 0) is
    // within the comparison, 2e-8 isn't.
    let case = case_of(
        "extend_shared_start",
        r#""points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 0, "y": 5, "construction": false}, {"id": "p0000004", "x": 5, "y": 5, "construction": false}, {"id": "p0000005", "x": -10, "y": 0, "construction": false}],
        "lines": [{"id": "n0000006", "from": "p0000005", "to": "p0000001", "construction": false}], "circles": [], "arcs": [{"id": "a0000003", "center": "p0000002", "start": "p0000001", "end": "p0000004", "construction": false}], "constraints": []"#,
        Edit::Extend {
            curve: "a0000003".to_owned(),
            end: sketch_edit::ExtendEnd::Start,
            by: ExtendBy::Angle(10.0),
        },
    );
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let theirs = freecad_outcomes(&prepared).unwrap();
    let neighbour = |x: f64| {
        doctored(&theirs[0], |fc| {
            let i = p
                .model
                .elements
                .iter()
                .position(|e| e == "n0000006")
                .unwrap();
            let id = fc.ids_before[i];
            let g = fc.edited.iter_mut().find(|g| g.id == id).unwrap();
            *g.points.get_mut("2").unwrap() = [x, 0.0];
        })
    };
    let drawn = |v: &Verdict| {
        v.how
            .iter()
            .any(|h| h.contains("not where the case drew it"))
    };
    assert!(!drawn(&verdict(p, theirs[0].as_ref())));
    assert!(!drawn(&verdict(p, Some(&neighbour(1e-8)))));
    assert!(drawn(&verdict(p, Some(&neighbour(2e-8)))));
}

#[test]
fn a_line_extend_freecad_settles_otherwise_compares_only_what_both_settle_alike() {
    // `sketch-extend-sweep --seed 44 --count 500`'s case 305: n0000006's start extended
    // 6.715 mm, written at the target (#se-extend-built), off where n0000005's length lets it be,
    // so both solves have far to go, and FreeCAD 1.1.3 settles the perpendicular n0000016's free
    // places otherwise run to run (SETTLES_OTHERWISE). After the solve only the points
    // neither solve moved are compared by place, and the rest in full.
    let mut sweep = silico_oracle::generate::sketch_extend_cases(44, 500).unwrap();
    let case = sweep.swap_remove(305);
    assert_eq!(case.name, "extend_44_305");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let theirs = freecad_outcomes(&prepared).unwrap();
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert!(
        v.excused
            .iter()
            .any(|e| e.starts_with("harness limit: FreeCAD 1.1.3's solve after its extend")),
        "{v:?}"
    );
    let solved = |fc: &mut FreecadEdited, element: &str| {
        let i = p.model.elements.iter().position(|e| e == element).unwrap();
        let id = fc.ids_before[i];
        fc.geometry.iter_mut().position(|g| g.id == id).unwrap()
    };
    // FreeCAD's n0000016's start (p0000014, which the solves move) 1e-3 off after the solve:
    // not compared here; in a case not on the list, past the band, reported.
    let moved = doctored(&theirs[0], |fc| {
        let i = solved(fc, "n0000016");
        fc.geometry[i].points.get_mut("1").unwrap()[0] += 1e-3;
    });
    agrees(&verdict(p, Some(&moved)));
    let unlisted = Prepared {
        name: "extend_44_305_unlisted".to_owned(),
        ..p.clone()
    };
    let v = verdict(&unlisted, Some(&moved));
    assert!(
        v.how.iter().any(|h| h
            .starts_with("after the solve, Silico's point p0000014, which the drag moved, is at")),
        "{v}"
    );
    // FreeCAD's perpendicular k0000017 gone, or its degrees of freedom one fewer: compared
    // still, reported.
    let dropped = doctored(&theirs[0], |fc| {
        fc.constraints.retain(|c| c.name != "s_k0000017");
    });
    let v = verdict(p, Some(&dropped));
    assert!(
        v.how
            .iter()
            .any(|h| h.starts_with("Silico has Perpendicular") && h.contains("k0000017")),
        "{v}"
    );
    let fewer = doctored(&theirs[0], |fc| fc.after.dof -= 1);
    let v = verdict(p, Some(&fewer));
    assert!(
        v.how.iter().any(|h| h.starts_with("degrees of freedom")),
        "{v}"
    );
}

#[test]
fn a_line_extends_end_an_arc_shares_is_written_at_the_target_in_freecads_arc_too() {
    // The line n0000005 from (0, 0) to p0000002 (10, 0), which starts the arc a0000006 about
    // (10, 5) to (15, 5); the line's end extended 2 mm. Silico's write puts p0000002 at
    // (12, 0); the harness writes FreeCAD's arc as Silico's model takes one from its points,
    // its start there too (#se-extend-built), so both solves start alike, and they agree.
    let case = case_of(
        "extend_line_into_an_arc",
        r#""points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}, {"id": "p0000003", "x": 10, "y": 5, "construction": false}, {"id": "p0000004", "x": 15, "y": 5, "construction": false}],
        "lines": [{"id": "n0000005", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [{"id": "a0000006", "center": "p0000003", "start": "p0000002", "end": "p0000004", "construction": false}], "constraints": []"#,
        Edit::Extend {
            curve: "n0000005".to_owned(),
            end: sketch_edit::ExtendEnd::End,
            by: ExtendBy::Length(2.0),
        },
    );
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let theirs = freecad_outcomes(&prepared).unwrap();
    let Some(Theirs::Edited(fc)) = &theirs[0] else {
        panic!("{:?}", theirs[0]);
    };
    assert!(fc.applied.iter().any(|c| c == "se-extend-built"), "{fc:?}");
    let start = edited_geometry(p, &mut fc.as_ref().clone(), "a0000006").points["1"];
    assert!(
        (start[0] - 12.0).abs() <= 1e-12 && start[1].abs() <= 1e-12,
        "{start:?}"
    );
    agrees(&verdict(p, theirs[0].as_ref()));
}

#[test]
fn a_held_ends_every_place_is_written_at_the_target_in_freecads_sketch() {
    // The line n0000002 from (0, 0) to p0000003 (10, 0), the centre of c0000004, radius 2,
    // and held on the slanted n0000007 from (5, -5) to (15, 5): the end extended 5 mm, both
    // drags stop on n0000007, short of (15, 0). Silico writes p0000003 there (#se-extend-built);
    // the harness writes FreeCAD's line end and circle centre there too, so before the solve
    // nothing of p0000003 differs.
    let case = case_of(
        "extend_end_held_on_a_slant",
        r#""points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000003", "x": 10, "y": 0, "construction": false}, {"id": "p0000005", "x": 5, "y": -5, "construction": false}, {"id": "p0000006", "x": 15, "y": 5, "construction": false}],
        "lines": [{"id": "n0000002", "from": "p0000001", "to": "p0000003", "construction": false}, {"id": "n0000007", "from": "p0000005", "to": "p0000006", "construction": false}], "circles": [{"id": "c0000004", "center": "p0000003", "radius": 2, "construction": false}], "arcs": [],
        "constraints": [{"id": "k0000008", "type": "on", "point": "p0000003", "curve": "n0000007"}]"#,
        Edit::Extend {
            curve: "n0000002".to_owned(),
            end: sketch_edit::ExtendEnd::End,
            by: ExtendBy::Length(5.0),
        },
    );
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let theirs = freecad_outcomes(&prepared).unwrap();
    let Some(Theirs::Edited(fc)) = &theirs[0] else {
        panic!("{:?}", theirs[0]);
    };
    let mut edited = fc.as_ref().clone();
    assert_eq!(
        edited_geometry(p, &mut edited, "n0000002").points["2"],
        [15.0, 0.0]
    );
    assert_eq!(
        edited_geometry(p, &mut edited, "c0000004").points["3"],
        [15.0, 0.0]
    );
    let v = verdict(p, theirs[0].as_ref());
    assert!(
        !v.how
            .iter()
            .any(|h| h.starts_with("after the edit, before the solve") && h.contains("p0000003")),
        "{v}"
    );
}

#[test]
fn a_merge_its_command_judges_worse_is_refused_and_agrees_with_freecads_rollback() {
    // `sketch-edit-sweep` seed 84's case 479 and 90's 254: Silico's merge solves, but finds
    // constraints redundant that the sketch's solve before it didn't, so its command refuses
    // it (silico_core::sketch_judge, #se-no-automatic-fix-up, #se-merge-built); FreeCAD's merge
    // finds them redundant too (-2, 8 of 8 runs each), which its Coincident command rolls back
    // (#se-any-result-whose-solve-is-worse). (Seed 52's case 215 is the same, but FreeCAD reports
    // it as -2 or -3 run to run: KNOWN.)
    let mut worker = SketchWorker::start().unwrap();
    let prepared: Vec<Prepared> = [(84u64, 479usize), (90, 254)]
        .into_iter()
        .map(|(seed, n)| {
            let mut sweep = silico_oracle::generate::sketch_edit_cases(seed, 500).unwrap();
            prepare(&sweep.swap_remove(n), &mut worker).unwrap()
        })
        .collect();
    let theirs = freecad_outcomes(&prepared).unwrap();
    for (p, fc) in prepared.iter().zip(&theirs) {
        assert_eq!(
            rule(p),
            Some(Rule::MergeSolveWorse),
            "{}: {:?}",
            p.name,
            p.silico
        );
        let v = verdict(p, fc.as_ref());
        agrees(&v);
        assert!(
            v.refused && v.modulo.iter().any(|c| c == "se-merge-built"),
            "{v:?}"
        );
        // FreeCAD's merge solving cleanly instead: a difference.
        let clean = doctored(fc, |fc| fc.after = fc.before.clone());
        let v = verdict(p, Some(&clean));
        assert!(!v.refused && !v.how.is_empty(), "{}: {v:?}", p.name);
        // FreeCAD refusing otherwise than measured: its sketch not clean before the merge, a
        // constraint of its own after the rollback, its solve a conflict (-3), or redundancy
        // of constraints other than those Silico names: both refuse, but not as the note has
        // it, a difference each.
        let Some(Theirs::Edited(edited)) = fc else {
            panic!("{fc:?}");
        };
        let named: Vec<usize> = edited
            .after
            .redundant
            .iter()
            .copied()
            .filter(|i| edited.constraints[i - 1].name.starts_with("s_"))
            .collect();
        let unnamed = (1..=edited.constraints.len())
            .find(|i| !edited.after.redundant.contains(i) && !edited.before.redundant.contains(i))
            .unwrap();
        let changes: Vec<(&str, Doctoring)> = vec![
            ("not clean before", Box::new(|fc| fc.before.code = -2)),
            (
                "a constraint of its own after the rollback",
                Box::new(|fc| {
                    let mut extra = fc.constraints[0].clone();
                    extra.name = "s_k9999999".to_owned();
                    fc.constraints.push(extra);
                }),
            ),
            ("a conflict", Box::new(|fc| fc.after.code = -3)),
            (
                "a constraint conflicting too",
                Box::new(move |fc| fc.after.conflicting.push(unnamed)),
            ),
            (
                "other constraints redundant",
                Box::new(move |fc| fc.after.redundant = vec![unnamed]),
            ),
        ];
        assert!(!named.is_empty(), "{}: {edited:?}", p.name);
        for (what, change) in changes {
            let v = verdict(p, Some(&doctored(fc, |fc| change(fc))));
            assert!(!v.how.is_empty(), "{}: {what}: {v:?}", p.name);
        }
    }
    // The refusal naming a constraint the sketch hasn't, or another edit's words: no rule.
    let p = &prepared[0];
    let SilicoSide::Refused(r) = &p.silico else {
        panic!("{:?}", p.silico);
    };
    for change in [
        Box::new(|r: &mut Refusal| r.elements = vec!["k9999999".to_owned()])
            as Box<dyn Fn(&mut Refusal)>,
        Box::new(|r: &mut Refusal| {
            r.message = r.message.replace("after the merge", "after the split")
        }),
        Box::new(|r: &mut Refusal| r.code = "solve_failed".to_owned()),
    ] {
        let mut other = r.clone();
        change(&mut other);
        let p = Prepared {
            silico: SilicoSide::Refused(other),
            ..p.clone()
        };
        assert!(rule(&p).is_none(), "{:?}", p.silico);
    }
}

#[test]
fn a_merge_its_command_judges_worse_agrees_with_freecads_rollback_as_a_conflict_too() {
    // The nightly sweeps' edit_169_486, edit_175_299, and edit_158_347: Silico's merge solve,
    // from the kept point's place, finds constraints redundant that the sketch's before didn't,
    // so its command refuses it (MergeSolveWorse); FreeCAD's, from the points apart, calls the
    // dependent constraints conflicting (-3: 169_486 and 175_299 in 3 of 3 runs alone and their
    // nights' sweeps, 158_347 in its sweep) or redundant (-2: 158_347 in a run alone), which its
    // Coincident command rolls back. Both refuse the merge as worse: agreement either way,
    // where each constraint Silico names is among those FreeCAD newly finds redundant or
    // conflicting.
    let mut worker = SketchWorker::start().unwrap();
    let prepared: Vec<Prepared> = [(169u64, 486usize), (175, 299), (158, 347)]
        .into_iter()
        .map(|(seed, n)| {
            let mut sweep = silico_oracle::generate::sketch_edit_cases(seed, 500).unwrap();
            prepare(&sweep.swap_remove(n), &mut worker).unwrap()
        })
        .collect();
    let theirs = freecad_outcomes(&prepared).unwrap();
    for (p, fc) in prepared.iter().zip(&theirs) {
        assert_eq!(
            rule(p),
            Some(Rule::MergeSolveWorse),
            "{}: {:?}",
            p.name,
            p.silico
        );
        let Some(Theirs::Edited(edited)) = fc else {
            panic!("{}: {fc:?}", p.name);
        };
        assert!(
            [-2, -3].contains(&edited.after.code),
            "{}: {edited:?}",
            p.name
        );
        let v = verdict(p, fc.as_ref());
        agrees(&v);
        assert!(
            v.refused && v.modulo.iter().any(|c| c == "se-merge-built"),
            "{v:?}"
        );
        // FreeCAD's rollback as a conflict, whatever this run gave: the constraints Silico
        // names conflicting (-3), nothing else redundant. Agrees.
        let SilicoSide::Refused(r) = &p.silico else {
            panic!("{:?}", p.silico);
        };
        let named: Vec<usize> = (1..=edited.constraints.len())
            .filter(|i| {
                r.elements
                    .iter()
                    .any(|id| edited.constraints[i - 1].name == format!("s_{id}"))
            })
            .collect();
        assert_eq!(named.len(), r.elements.len(), "{}: {r:?}", p.name);
        let as_conflict = {
            let named = named.clone();
            doctored(fc, move |fc| {
                fc.after.code = -3;
                fc.after.conflicting = named;
                fc.after.redundant = fc.before.redundant.clone();
            })
        };
        let v = verdict(p, Some(&as_conflict));
        agrees(&v);
        assert!(v.refused, "{}: {v:?}", p.name);
        // From there: Silico's named constraints in neither list, nothing newly conflicting,
        // or a constraint newly malformed: a difference each.
        let as_conflict = Some(as_conflict);
        let unnamed = (1..=edited.constraints.len())
            .find(|i| !named.contains(i) && !edited.before.conflicting.contains(i))
            .unwrap();
        let changes: Vec<(&str, Doctoring)> = vec![
            (
                "other constraints conflicting",
                Box::new(move |fc| fc.after.conflicting = vec![unnamed]),
            ),
            (
                "nothing newly conflicting",
                Box::new(|fc| fc.after.conflicting = fc.before.conflicting.clone()),
            ),
            (
                "a constraint malformed",
                Box::new(move |fc| fc.after.malformed.push(unnamed)),
            ),
        ];
        for (what, change) in changes {
            let v = verdict(p, Some(&doctored(&as_conflict, |fc| change(fc))));
            assert!(!v.how.is_empty(), "{}: {what}: {v:?}", p.name);
        }
    }
}

#[test]
fn a_merge_whose_solve_conflicts_from_the_kept_points_place_is_excused_where_freecads_solves() {
    // `sketch-edit-sweep --seed 90 --count 500`'s case 370: p0000002 merged into p0000004
    // leaves n0000005 upright where it starts from the kept point's place, and Silico's solve
    // from there finds its horizontal and its length conflicting, refusing the merge (the
    // request's order is the seed, #se-a-solve-that-fails-or-conflicts); FreeCAD's coincidences,
    // from the points apart, solve and merge.
    let mut sweep = silico_oracle::generate::sketch_edit_cases(90, 500).unwrap();
    let case = sweep.swap_remove(370);
    assert_eq!(case.name, "edit_90_370");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::MergeSolveConflicts), "{:?}", p.silico);
    // FreeCAD's coincidences solve here in 9 of 10 runs, and its merge is otherwise rolled
    // back (#se-any-result-whose-solve-is-worse), a refusal too: either agrees. Its solving run is
    // taken for the excusal's checks (one is all but certain in eight).
    let mut solved = None;
    for _ in 0..8 {
        let theirs = freecad_outcomes(&prepared).unwrap();
        let v = verdict(p, theirs[0].as_ref());
        agrees(&v);
        if v.excused.is_empty() {
            assert!(v.refused, "{v:?}");
        } else {
            assert!(
                v.excused.len() == 1
                    && v.excused[0]
                        .starts_with("a merge whose solve conflicts from the kept point's place"),
                "{v:?}"
            );
            solved = Some(theirs);
            break;
        }
    }
    let theirs = solved.expect("FreeCAD's coincidences solved in none of eight runs");
    // The dropped point drawn onto the kept one: points together start both solvers alike,
    // so the seed's excusal doesn't hold, a difference.
    let mut together = p.clone();
    let kept = together
        .sketch
        .points
        .iter()
        .find(|q| q.id == id("p0000004"))
        .map(|q| (q.x, q.y))
        .unwrap();
    for q in &mut together.sketch.points {
        if q.id == id("p0000002") {
            (q.x, q.y) = kept;
        }
    }
    let v = verdict(&together, theirs[0].as_ref());
    assert!(v.excused.is_empty() && !v.how.is_empty(), "{v:?}");
    // FreeCAD's solve succeeding with its merge's coincidence not made, or its merged places
    // left apart: no merge, a difference.
    let unmerged = doctored(&theirs[0], |fc| {
        fc.constraints.pop();
    });
    let Plan::Run { job, .. } = &p.plan else {
        panic!("{:?}", p.plan);
    };
    let FreecadEdit::Merge { coincidences, .. } = &job.edit else {
        panic!("{:?}", job.edit);
    };
    let [g, pos, ..] = coincidences[0];
    let apart = doctored(&theirs[0], |fc| {
        fc.geometry[usize::try_from(g).unwrap()]
            .points
            .get_mut(&pos.to_string())
            .unwrap()[0] += 1e-3;
    });
    for other in [unmerged, apart] {
        let v = verdict(p, Some(&other));
        assert!(v.excused.is_empty() && !v.how.is_empty(), "{v:?}");
    }
}

#[test]
fn a_merge_freecad_conflicts_on_only_from_its_own_start_is_excused_where_its_placed_solve_is_clean()
{
    // `sketch-edit-sweep --seed 90 --count 500`'s case 328: p0000033 (n0000005's midpoint)
    // and p0000024 (n0000026's start, held tangent on c0000018) merged from apart, at the
    // circle's contact with n0000005, a double root. Silico's solve, from the kept point's
    // place (#se-the-first-point-the-request-names), is clean; FreeCAD's, from the points apart,
    // finds six constraints conflicting (-3), which its Coincident command rolls back, while its
    // solve of the same merge placed as Silico places it is clean (#se-so-does-any-refusal-of-the).
    let mut sweep = silico_oracle::generate::sketch_edit_cases(90, 500).unwrap();
    let case = sweep.swap_remove(328);
    assert_eq!(case.name, "edit_90_328");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let theirs = freecad_outcomes(&prepared).unwrap();
    let Some(Theirs::Edited(fc)) = &theirs[0] else {
        panic!("{:?}", theirs[0]);
    };
    assert_eq!(fc.after.code, -3, "{:?}", fc.after);
    assert_eq!(
        fc.placed.as_ref().map(|d| d.solve.code),
        Some(0),
        "{:?}",
        fc.placed
    );
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert!(
        v.excused
            .iter()
            .any(|e| e.starts_with("points merged from apart, whose solve Silico takes clean")),
        "{v:?}"
    );
    // FreeCAD's placed solve not run, conflicting too, failing, or one degree of freedom off
    // Silico's; its own solve -2 rather than a conflict: a difference each.
    let changes: Vec<(&str, Doctoring)> = vec![
        ("no placed solve", Box::new(|fc| fc.placed = None)),
        (
            "the placed solve conflicting",
            Box::new(|fc| {
                if let Some(d) = &mut fc.placed {
                    d.solve.code = -3;
                    d.solve.conflicting = vec![1];
                }
            }),
        ),
        (
            "the placed solve failing",
            Box::new(|fc| {
                if let Some(d) = &mut fc.placed {
                    d.solve.code = -1;
                }
            }),
        ),
        (
            "the placed solve's degrees of freedom one fewer",
            Box::new(|fc| {
                if let Some(d) = &mut fc.placed {
                    d.solve.dof -= 1;
                }
            }),
        ),
        ("its own solve -2", Box::new(|fc| fc.after.code = -2)),
    ];
    for (what, change) in changes {
        let v = verdict(p, Some(&doctored(&theirs[0], |fc| change(fc))));
        assert!(v.excused.is_empty() && !v.how.is_empty(), "{what}: {v:?}");
    }
}

#[test]
fn a_trimmed_end_two_cutters_cut_at_one_place_is_excused_only_for_freecads_other_join() {
    // `sketch-edit-sweep --seed 84 --count 500`'s case 466: n0000005 trimmed where the
    // upright n0000013 crosses it, at c0000019's tangent contact. Silico holds the new end
    // on the circle, its tangency the end's; FreeCAD 1.1.3 holds it on n0000013, the
    // tangency kept edge to edge (Fable, 2026-10-30, #se-of-cuts-within-1e-7-mm).
    let mut sweep = silico_oracle::generate::sketch_edit_cases(84, 500).unwrap();
    let case = sweep.swap_remove(466);
    assert_eq!(case.name, "edit_84_466");
    let mut worker = SketchWorker::start().unwrap();
    let prepared = vec![prepare(&case, &mut worker).unwrap()];
    let p = &prepared[0];
    let theirs = freecad_outcomes(&prepared).unwrap();
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert!(
        v.excused
            .iter()
            .any(|e| e.starts_with("a trimmed end that two cutters cut at one place"))
            && v.modulo.iter().any(|c| c == "se-of-cuts-within-1e-7-mm"),
        "{v:?}"
    );
    let index = |fc: &FreecadEdited, element: &str| {
        let i = p.model.elements.iter().position(|e| e == element).unwrap();
        let id = fc.ids_before[i];
        i32::try_from(fc.geometry.iter().position(|g| g.id == id).unwrap()).unwrap()
    };
    let joined = "FreeCAD has PointOnObject(w0000000, n0000013), Silico hasn't";
    // FreeCAD joining the circle, the cutter Silico takes: no tie, reported.
    let same = doctored(&theirs[0], |fc| {
        let (upright, circle) = (index(fc, "n0000013"), index(fc, "c0000019"));
        let c = fc
            .constraints
            .iter_mut()
            .find(|c| c.kind == "PointOnObject" && c.refs[1] == [upright, 0])
            .unwrap();
        c.refs[1] = [circle, 0];
    });
    let v = verdict(p, Some(&same));
    assert!(v.excused.is_empty() && !v.how.is_empty(), "{v:?}");
    // The upright drawn 1e-6 mm off the cut: no longer cutting at one place, reported.
    let mut off = p.clone();
    for q in &mut off.sketch.points {
        if q.id == id("p0000011") || q.id == id("p0000012") {
            q.x = silico_core::document::Coordinate::new(q.x.mm() + 1e-6).unwrap();
        }
    }
    let v = verdict(&off, theirs[0].as_ref());
    assert!(v.how.iter().any(|h| h.contains(joined)), "{v}");
    // FreeCAD's edge tangency gone, so nothing of the tie's form to set aside; or the circle
    // drawn 1e-6 mm off the cut (its centre, above it, raised): no tie, the join reported.
    let edgeless = doctored(&theirs[0], |fc| {
        fc.constraints.retain(|c| c.name != "s_k0000020");
    });
    let v = verdict(p, Some(&edgeless));
    assert!(v.how.iter().any(|h| h.contains(joined)), "{v}");
    let mut moved = p.clone();
    for q in &mut moved.sketch.points {
        if q.id == id("p0000018") {
            q.y = silico_core::document::Coordinate::new(q.y.mm() + 1e-6).unwrap();
        }
    }
    let v = verdict(&moved, theirs[0].as_ref());
    assert!(v.how.iter().any(|h| h.contains(joined)), "{v}");
    // FreeCAD making the end tangency too (beside its edge one): the forms agree, so no tie,
    // the join reported.
    let both_forms = doctored(&theirs[0], |fc| {
        let (upright, circle) = (index(fc, "n0000013"), index(fc, "c0000019"));
        let end = fc
            .constraints
            .iter()
            .find(|c| c.kind == "PointOnObject" && c.refs[1] == [upright, 0])
            .unwrap()
            .refs[0];
        fc.constraints.push(FcConstraint {
            kind: "Tangent".to_owned(),
            refs: vec![end, [circle, 0]],
            value: 0.0,
            driving: true,
            name: "s_k0000020".to_owned(),
        });
    });
    let v = verdict(p, Some(&both_forms));
    assert!(
        v.excused.iter().all(|e| !e.starts_with("a trimmed end"))
            && v.how.iter().any(|h| h.contains(joined)),
        "{v}"
    );
    // Silico's end held on the upright too: FreeCAD's join is Silico's, no tie.
    let held = changed(&case, p, |sketch| {
        sketch.constraints.push(SketchConstraint {
            id: id("k0000099"),
            form: Form::On {
                point: Reference::Element(id("w0000000")),
                curve: Reference::Element(id("n0000013")),
            },
        });
    });
    let v = verdict(&held, theirs[0].as_ref());
    assert!(
        v.excused.iter().all(|e| !e.starts_with("a trimmed end")),
        "{v}"
    );
    // Another difference beside the join: FreeCAD's horizontal k0000009 gone, reported
    // still, the tie excused.
    let other = doctored(&theirs[0], |fc| {
        fc.constraints.retain(|c| c.name != "s_k0000009");
    });
    let v = verdict(p, Some(&other));
    assert!(
        !v.excused.is_empty()
            && v.how.iter().any(|h| h.contains("k0000009"))
            && !v.how.iter().any(|h| h.contains(joined)),
        "{v}"
    );
}
