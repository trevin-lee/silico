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
use silico_oracle::sketch::freecad_sketch;
use silico_oracle::sketch_edit::{
    self, Edit, EditCase, FcConstraint, FcGeometry, FreecadEdit, FreecadEdited, KNOWN, Plan,
    Prepared, Refusal, Rule, SilicoSide, Theirs, Verdict, freecad_outcomes, plan, prepare, rule,
    silico_side, verdict, without_known,
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
    // merge_holds_own_point_on_line, and `sketch-edit-sweep --seed 32 --count 500`'s case 490:
    // a merge after which an `on` would hold a line's own end on the line, which Silico
    // refuses as the gate's rule for the merged sketch (:31, :303). FreeCAD 1.1.3 makes the
    // merge, its solve finding the `on` redundant (-2, 10 of 10 runs), which its Coincident
    // command rolls back (:66): both refuse.
    let mut cases = cases(&["merge_holds_own_point_on_line"]);
    let mut sweep = silico_oracle::generate::sketch_edit_cases(32, 500).unwrap();
    cases.push(sweep.swap_remove(490));
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
        matches!(rule(&prepared[2]), Some(Rule::MergeHoldsOwn(id)) if id == "k0000032"),
        "{:?}",
        prepared[2].silico
    );
    let prepared = &prepared[..2];
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
            v.refused && v.modulo.contains(&31) && v.modulo.contains(&303),
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
            assert_eq!(p.name, "merge_holds_own_point_on_line");
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
        // Tangent (:523), and copies the horizontal (and the perpendicular and parallel) to
        // the second piece, which come off it (:160).
        assert_eq!(v.modulo, [160, 523], "{}", p.name);
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
fn only_a_split_construction_circles_pieces_are_excused_their_construction() {
    let (cases, prepared, theirs) = both(&[
        "split_construction_circle",
        "split_construction_line",
        "split_construction_arc",
        "split_construction_circle_beside_line",
        "split_circle_at_two_positions",
    ]);
    // 1.1.3's circle split drops construction from both arcs, where Silico's keep it (:177):
    // excused, each piece by name, and compared modulo that line, beside decision 3's
    // joining of 1.1.3's two arcs (:525).
    let excuse = |piece: &str| {
        format!(
            "Silico's {piece} is construction, FreeCAD's isn't: FreeCAD 1.1.3's split of a construction circle drops it, where Silico's pieces keep it (docs/ports/sketch-edit.md:177)"
        )
    };
    for i in [0, 3] {
        let v = verdict(&prepared[i], theirs[i].as_ref());
        agrees(&v);
        assert_eq!(v.excused, [excuse("c0000002"), excuse("w0000002")], "{v:?}");
        assert_eq!(v.modulo, [177, 525], "{v:?}");
    }
    // A line's and an arc's pieces keep it in 1.1.3 too: nothing to excuse.
    for i in [1, 2] {
        let v = verdict(&prepared[i], theirs[i].as_ref());
        agrees(&v);
        assert!(v.excused.is_empty(), "{v:?}");
        assert!(!v.modulo.contains(&177), "{v:?}");
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
    // reported, since only a circle's split drops it (:177).
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
    // other way round from :177, reported.
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
    // FreeCAD splits at the projection, and decision 4 (:526) says so.
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert_eq!(v.excused.len(), 1, "{v:?}");
    assert!(
        v.excused[0].starts_with("a split position 7.07106781")
            && v.excused[0].contains("decision 4, docs/ports/sketch-edit.md:526"),
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
    // (:248), and FreeCAD is given it there, so the results compare in full.
    let v = verdict(&prepared[2], theirs[2].as_ref());
    agrees(&v);
    assert!(v.excused.is_empty(), "{v:?}");
    assert!(v.modulo.contains(&248), "{v:?}");
}

#[test]
fn a_cutters_end_freecad_joins_by_tolerance_is_compared_as_decision_five_has_it() {
    let (_, prepared, theirs) = both(&["trim_line_end_as_drawn", "trim_line_end_held"]);
    // Drawn only: FreeCAD's 5e-5 mm test joins the new end to the cutter's start, which
    // becomes a point on the cutter, as Silico's (:527).
    let v = verdict(&prepared[0], theirs[0].as_ref());
    agrees(&v);
    assert_eq!(v.modulo, [527]);
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
    // A line's two ends (FreeCAD: a collapse, :65), and a horizontal_points between the
    // points (FreeCAD: a new redundancy, rolled back, :66): both refused on both sides.
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
    // stays with the point the others keep (:264); FreeCAD's vertex Delete leaves an end's
    // constraints on the curve it detaches (:259). The coincidences come first (:118).
    let Plan::Run { job, modulo } = planned(
        &three_way(),
        Edit::Unjoin {
            point: "p0000002".to_owned(),
            curve: "n0000001".to_owned(),
        },
    ) else {
        panic!("FreeCAD's plan refuses it");
    };
    assert_eq!(modulo, [264]);
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
    // Deleting a point curves end at: FreeCAD has no such edit (:136), so it's given its
    // Delete on that vertex, the point's first place, n0000001's end (:132-134).
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
    // itself, which its Coincident command refuses as already coincident (:64).
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
            coincidences: vec![[0, 1, 0, 1]]
        }
    );
    // Deleting a curve at the joint: geometry 1, the coincidences first (:118); the
    // distance_x sits on n0000001's end, which stays, so nothing moves.
    let Plan::Run { job, modulo } = planned(
        &s,
        Edit::Remove {
            members: vec!["n0000002".to_owned()],
        },
    ) else {
        panic!("FreeCAD's plan refuses it");
    };
    assert_eq!(modulo, [118]);
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
    // Every edit's result has to pass the gate (:34, :180): a gated result is Silico's bug,
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
    // line's end goes there and the line is 0 mm long: the gate refuses the merge (:303).
    // FreeCAD's Coincident command rolls back its solve, so both refuse.
    let (_, prepared, theirs) = both(&["merge_apart_onto_a_lines_other_end", "merge_line_ends"]);
    // A merge of a line's own ends is the collapse rule's (:300), not this one's, though it
    // names the line last too.
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
    assert!(v.refused && v.modulo == [303], "{v:?}");
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
    // is given the pick where Silico places it (:248), so the results compare, and this
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
    // 1.1.3 makes one arc in its place, its ends at one place, not joined (:148): excused.
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
        let changes: [(&str, Doctoring); 7] = [
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
                "a piece about another centre, or of another radius",
                Box::new(|fc| {
                    if let Some(c) = fc.edited[1].points.get_mut("3") {
                        c[0] += 1e-6;
                    }
                    if let Some(r) = &mut fc.edited[1].radius {
                        *r += 1e-6;
                    } else {
                        // A line has neither: move its start off the joint instead.
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
    // (:186).
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
    // the bottom line's far end, so the vertical n0000008 is left with no length (:307), and
    // Silico refuses it. FreeCAD 1.1.3's solve fails (-1), which its Coincident command rolls
    // back (10 of 10 runs, when this test was written): both refuse, as the note has it (:305).
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
    assert_eq!(v.modulo, [305, 306, 307], "{v:?}");
    // Its solve succeeding instead, the note's other outcome, the merged places (the job's
    // coincidence of geometry 4's end and geometry 2's end) at one place: Silico's refusal
    // excused.
    let Plan::Run { job, .. } = &p.plan else {
        panic!("FreeCAD isn't run");
    };
    assert_eq!(
        job.edit,
        FreecadEdit::Merge {
            coincidences: vec![[4, 2, 2, 2]]
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
    // too (:305, `edit_44_310`): agreeing, its geometry not checked.
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
        Theirs::Lost("FreeCAD crashed building it (signal: 11 (SIGSEGV))".to_owned()),
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
    // Splitting a point: 1.1.3 segfaults (:142), and Silico refuses it (:180).
    let p = &prepared[0];
    assert_eq!(rule(p), Some(Rule::SplitNotCurve));
    let Some(Theirs::Lost(why)) = &theirs[0] else {
        panic!("FreeCAD didn't crash: {:?}", theirs[0]);
    };
    assert!(why.starts_with("FreeCAD crashed"), "{why}");
    let v = verdict(p, theirs[0].as_ref());
    agrees(&v);
    assert!(v.crashed && !v.refused, "{v:?}");
    assert_eq!(v.modulo, [142, 180], "{v:?}");
    // FreeCAD hanging on it, raising, or making it: not the crash the note has.
    for other in [
        Theirs::Lost("FreeCAD took longer than 60 s building it".to_owned()),
        Theirs::Lost("FreeCAD crashed building it (exit status: 3)".to_owned()),
        Theirs::Lost("FreeCAD crashed building it (signal: 6 (SIGABRT))".to_owned()),
        Theirs::Failed("refused by hand".to_owned()),
    ] {
        let v = verdict(p, Some(&other));
        assert!(!v.crashed && !v.refused, "{other:?}: {v:?}");
        assert_eq!(v.how.len(), 1, "{other:?}: {v}");
    }
    // A crash where the note has FreeCAD refuse (a collapse, :65; a split past the end, in
    // OCCT, :144), or make the edit (a trim of a point, :186, a line's split): a difference,
    // never its refusal.
    let crash = Theirs::Lost("FreeCAD crashed building it (signal: 11 (SIGSEGV))".to_owned());
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
    assert!(v.modulo.contains(&528), "{v:?}");
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
    // the circle c0000008. FreeCAD has no edit deleting it (:136); its Delete on that vertex,
    // a0000004's centre, unjoins that centre from the others, deleting no curve (:132-134).
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
    // coincidence (:261): a difference.
    let raises = Theirs::Failed(sketch_edit::VERTEX_DELETE_RAISES.to_owned());
    for theirs in [&raises, &Theirs::Failed("refused by hand".to_owned())] {
        let v = verdict(p, Some(theirs));
        assert!(!v.refused && v.excused.is_empty(), "{v:?}");
        assert_eq!(v.how.len(), 1, "{v}");
    }
    // The job's vertex (the centre, on c0000008) moved to a0000004's start, p0000002, which
    // no other curve uses: an end with no coincidence in FreeCAD's model (the centre still
    // shared), where 1.1.3 raises, as the note gives it (:261), and both refuse; raising
    // otherwise is a difference.
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
    // "Not able to delete a constraint on point" (:261).
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
    // the points being one already (:64).
    let clean = doctored(&theirs[0], |fc| fc.after = fc.before.clone());
    let v = verdict(p, Some(&clean));
    agrees(&v);
    assert!(v.refused, "{v:?}");
}

#[test]
fn a_trim_at_a_rounds_centre_agrees_only_where_freecad_raises() {
    // 1.1.3 throws "Unknown C++ exception" (:249).
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
        Theirs::Lost("FreeCAD crashed building it (signal: 11 (SIGSEGV))".to_owned()),
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
    // 10 runs); Silico's point stays shared (:96), so FreeCAD's ends are joined again.
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
    assert!(modulo.contains(&96), "{modulo:?}");
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
    // again (:96), either pair. None removed, nothing is added.
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
        assert!(modulo.contains(&96), "{modulo:?}");
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
        rejoin.is_empty() && !modulo.contains(&96),
        "{rejoin:?} {modulo:?}"
    );
}

#[test]
fn a_merge_whose_solve_fails_from_the_kept_points_place_is_refused_and_named_the_other_way_solves()
{
    // `sketch-edit-sweep --seed 43 --count 500`'s case 229's sketch: p0000001 held at
    // (-29.382, -9.384) by distance_x and distance_y, p0000003 33 mm away at
    // (-11.093, 18.734). Merged with p0000003 kept, the solve starts from p0000003's place
    // and fails (:305, :307); named the other way round, the held point first, it solves.
    // Its freedoms by hand: 15 points (30 values) and 2 circle radii, 32; less the
    // horizontal, distance_x, distance_y, parallel, tangent_at, the arc's rule, tangent,
    // radius, and equal (one each) and the tangent_on (two): 21.
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
    let FreecadEdit::Merge { coincidences } = &job.edit else {
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
    // solve puts a symmetry's two points at one place, a result Silico refuses (:305).
    // FreeCAD 1.1.3's merge finds 13 constraints redundant (-2), which its Coincident command
    // rolls back, in 39 of 40 runs; in one it found one conflicting as well (-3), so the case
    // is known as FreeCAD varying, and FreeCAD's diagnosis here is set to the -2 it gives
    // nearly always, not taken as it came. Its geometry and constraints are FreeCAD's own.
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
    // both sides (:166, :524).
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
    // merge_holds_own_point_on_line, made known here as FreeCAD varying by its rule: shown
    // so while Silico refuses it by MergeHoldsOwn; with Silico's result gated, refused for a
    // reason no rule names, or the case's sketch not drawn as it solves, it's compared, and
    // the known case a problem.
    let (_, prepared, theirs) = both(&["merge_holds_own_point_on_line"]);
    let p = &prepared[0];
    let known = [sketch_edit::Known {
        case: "merge_holds_own_point_on_line",
        shows: sketch_edit::Shows::Varies(Rule::MergeHoldsOwn(String::new())),
        why: "varies here",
    }];
    let (differ, shown, stale) = without_known(vec![verdict(p, theirs[0].as_ref())], &known);
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
    // refuses (#290, :303). Naming another constraint isn't the rule.
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
    // the kept point's place, which Silico refuses (:305, :307). FreeCAD 1.1.3's merge finds
    // eight constraints conflicting (-3, 10 of 10 runs), which its Coincident command rolls
    // back.
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
