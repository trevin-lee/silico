//! The sketch-edit oracle (tests/oracle/sketch-edits/, held for the maintainer): every case
//! agrees with FreeCAD 1.1.3 modulo docs/ports/sketch-edit.md, or is one of the differences
//! reported for a decision; and the harness catches a real difference, a Silico result
//! changed by hand being reported, while a divergence the note states is excused only where
//! the note says. Expected values are worked by hand from the cases' drawings.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use silico_core::document::sketch_edit::Edited;
use silico_core::document::{Checked, ElementId, Operation, Sketch, read};
use silico_core::sketch::{SketchWorker, Translation};
use silico_oracle::sketch::freecad_sketch;
use silico_oracle::sketch_edit::{
    self, Edit, EditCase, FreecadEdit, KNOWN, Plan, Prepared, SilicoSide, Theirs, Verdict,
    freecad_outcomes, plan, prepare, silico_side, verdict, without_known,
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
    let verdicts = sketch_edit::run(&all).unwrap();
    assert_eq!(verdicts.len(), all.len());
    let (differ, shown, stale) = without_known(verdicts, KNOWN);
    let differ: Vec<String> = differ.iter().map(ToString::to_string).collect();
    assert!(differ.is_empty(), "{differ:#?}");
    assert!(stale.is_empty(), "{stale:#?}");
    assert_eq!(shown.len(), KNOWN.len(), "{shown:#?}");
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
        // Tangent (:442), and copies the horizontal (and the perpendicular and parallel) to
        // the second piece, which come off it (:132).
        assert_eq!(v.modulo, [132, 442], "{}", p.name);
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
    // end and n0000006's start, Silico's p0000002 and p0000003 (D2, :25); five points (the
    // loose one at (6, 5) the merge_apart case uses too) less the horizontal and the vertical
    // is 8 freedoms, and FreeCAD's coincidence takes 2 more.
    let original = cases[0].sketch().unwrap().clone();
    let p = changed(&cases[0], &prepared[0], |s| *s = original);
    let v = verdict(&p, theirs[0].as_ref());
    assert_eq!(
        v.how,
        [
            "FreeCAD joins n0000004's end, n0000006's start, which are Silico's points p0000002, p0000003 apart (docs/ports/sketch-edit.md:25)",
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
fn a_refusal_is_excused_only_where_the_note_gives_it() {
    let (_, prepared, theirs) = both(&[
        "split_line_at_freecads_position",
        "split_line_at_projection",
        "trim_arc_pick_outside_sweep",
    ]);
    // (2, 3.1) is |3.1 - 3| / √2 = 0.0707 mm off the line y = x + 1: Silico refuses it,
    // FreeCAD splits at the projection, and decision 4 (:445) says so.
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    assert!(
        v.excused[0].starts_with("a split position 7.07106781")
            && v.excused[0].contains("decision 4, docs/ports/sketch-edit.md:445"),
        "{v:?}"
    );
    // The same refusal of a position on the line, (2.05, 3.05), far from its ends: nothing in
    // the note gives it, so it's a difference.
    let refused = Prepared {
        silico: SilicoSide::Refused("refused by hand".to_owned()),
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
    // A pick at 315 degrees, outside the arc's 0 to 270: the note says the two trim
    // otherwise (:217), so only the outcomes are compared.
    let v = verdict(&prepared[2], theirs[2].as_ref());
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    assert!(v.excused[0].contains(":217"), "{v:?}");
}

#[test]
fn a_cutters_end_freecad_joins_by_tolerance_is_compared_as_decision_five_has_it() {
    let (_, prepared, theirs) = both(&["trim_line_end_as_drawn", "trim_line_end_held"]);
    // Drawn only: FreeCAD's 5e-5 mm test joins the new end to the cutter's start, which
    // becomes a point on the cutter, as Silico's (:446).
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert_eq!(v.modulo, [446]);
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
    // A line's two ends (FreeCAD: a collapse, :52), and a horizontal_points between the
    // points (FreeCAD: a new redundancy, rolled back, :53): both refused on both sides.
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
    // stays with the point the others keep (:233); FreeCAD's vertex Delete leaves an end's
    // constraints on the curve it detaches (:228). The coincidences come first (:91).
    let Plan::Run { job, modulo } = planned(
        &three_way(),
        Edit::Unjoin {
            point: "p0000002".to_owned(),
            curve: "n0000001".to_owned(),
        },
    ) else {
        panic!("FreeCAD's plan refuses it");
    };
    assert_eq!(modulo, [233]);
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
fn edits_freecad_has_no_counterpart_for_are_its_refusals_citing_the_note() {
    let s = three_way();
    // Deleting a point curves end at: FreeCAD has no such edit (:107-108).
    let Plan::Refuses(why) = planned(
        &s,
        Edit::Remove {
            members: vec!["p0000002".to_owned()],
        },
    ) else {
        panic!("FreeCAD's plan runs it");
    };
    assert!(why.contains(":107-108"), "{why}");
    // A merge naming a point twice: FreeCAD's Coincident command refuses points already
    // coincident (:51).
    let Plan::Refuses(why) = planned(
        &s,
        Edit::Merge {
            points: vec!["p0000001".to_owned(), "p0000001".to_owned()],
        },
    ) else {
        panic!("FreeCAD's plan runs it");
    };
    assert!(why.contains(":51"), "{why}");
    // Deleting a curve at the joint: geometry 1, the coincidences first (:91); the
    // distance_x sits on n0000001's end, which stays, so nothing moves.
    let Plan::Run { job, modulo } = planned(
        &s,
        Edit::Remove {
            members: vec!["n0000002".to_owned()],
        },
    ) else {
        panic!("FreeCAD's plan refuses it");
    };
    assert_eq!(modulo, [91]);
    assert_eq!(
        job.edit,
        FreecadEdit::Delete {
            geometries: vec![1],
            constraints: Vec::new()
        }
    );
}

#[test]
fn a_known_difference_that_no_longer_shows_is_a_problem() {
    let known = [sketch_edit::Known {
        case: "c",
        difference: "it differs",
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
