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
    // Every known difference of the case file shows (the sweep's are known by their names).
    let of_the_file = KNOWN
        .iter()
        .filter(|k| all.iter().any(|c| c.name == k.case))
        .count();
    assert_eq!(shown.len(), of_the_file, "{shown:#?}");
}

#[test]
fn the_sweeps_known_case_still_differs_as_known() {
    // `sketch-edit-sweep --seed 32 --count 500`'s case 490: a merge Silico refuses, pending
    // decision 13b (#222), that FreeCAD makes.
    let mut cases = silico_oracle::generate::sketch_edit_cases(32, 500).unwrap();
    let case = cases.swap_remove(490);
    assert_eq!(case.name, "edit_32_490");
    let verdicts = sketch_edit::run(&[case]).unwrap();
    let (differ, shown, stale) = without_known(verdicts, KNOWN);
    assert!(
        differ.is_empty() && stale.is_empty(),
        "{differ:?} {stale:?}"
    );
    assert_eq!(shown.len(), 1);
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
        // Tangent (:457), and copies the horizontal (and the perpendicular and parallel) to
        // the second piece, which come off it (:133).
        assert_eq!(v.modulo, [133, 457], "{}", p.name);
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
    // FreeCAD splits at the projection, and decision 4 (:460) says so.
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    assert!(
        v.excused[0].starts_with("a split position 7.07106781")
            && v.excused[0].contains("decision 4, docs/ports/sketch-edit.md:460"),
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
    // (:221), and FreeCAD is given it there, so the results compare in full.
    let v = verdict(&prepared[2], theirs[2].as_ref());
    agrees(&v);
    assert!(v.excused.is_empty(), "{v:?}");
    assert!(v.modulo.contains(&221), "{v:?}");
}

#[test]
fn a_cutters_end_freecad_joins_by_tolerance_is_compared_as_decision_five_has_it() {
    let (_, prepared, theirs) = both(&["trim_line_end_as_drawn", "trim_line_end_held"]);
    // Drawn only: FreeCAD's 5e-5 mm test joins the new end to the cutter's start, which
    // becomes a point on the cutter, as Silico's (:461).
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert_eq!(v.modulo, [461]);
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
    // stays with the point the others keep (:237); FreeCAD's vertex Delete leaves an end's
    // constraints on the curve it detaches (:232). The coincidences come first (:91).
    let Plan::Run { job, modulo } = planned(
        &three_way(),
        Edit::Unjoin {
            point: "p0000002".to_owned(),
            curve: "n0000001".to_owned(),
        },
    ) else {
        panic!("FreeCAD's plan refuses it");
    };
    assert_eq!(modulo, [237]);
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
    // Deleting a point curves end at: FreeCAD has no such edit (:109).
    let Plan::Refuses(why) = planned(
        &s,
        Edit::Remove {
            members: vec!["p0000002".to_owned()],
        },
    ) else {
        panic!("FreeCAD's plan runs it");
    };
    assert!(why.contains(":109"), "{why}");
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

// ---- The port review's proofs (#251), each a way a difference was read as agreement ----

#[test]
fn a_result_the_gate_refuses_is_a_difference_whatever_freecad_does() {
    // Every edit's result has to pass the gate (:34, :153): a gated result is Silico's bug,
    // even where FreeCAD refuses too (it fails in OCCT, or throws).
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
    // The note excuses these refusals for a reason (decision 2's constraint, decision 8's
    // tangent): a refusal for any other is a difference, however FreeCAD ends.
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
    // is given the pick where Silico places it (:221), so the results compare, and this
    // shows.
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
