//! The measure oracle (M4.5; Fable's decision, `m4-measure-fable.md`, *Oracle*): the cases
//! of tests/oracle/measures/ (protected), each pair measured by the core's `distance` or
//! `angle` and by FreeCAD 1.1.3's Measure, compared by the decision's rules and with the
//! case's values by hand; that every rule and the case file see a wrong answer; and that
//! FreeCAD's side finds each element by its geometry or says it can't.
//! Needs the FreeCAD oracle (`pixi install -e oracle`) and the workers built
//! (`cargo nextest run --workspace` builds them).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use silico_core::command::measure::{AngleAnswer, DistanceAnswer};
use silico_core::command::rebuild;
use silico_core::document::{Angle, FeatureId, Length};
use silico_core::pool::KernelPool;
use silico_core::recompute::Recompute;
use silico_core::{Error, ErrorCode};
use silico_oracle::measures::{
    self, Answer, Cases, Element, Expected, ExpectedAngle, ExpectedDistance, Found, MeasureCase,
    Measured, Ours, Rule, Theirs, against_expected, compare,
};

const ORACLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/oracle");

fn cases() -> Cases {
    measures::load(Path::new(ORACLE)).unwrap().unwrap()
}

#[test]
fn every_measure_case_matches_freecad_and_its_values() {
    let found: Vec<String> = measures::run(&cases())
        .unwrap()
        .iter()
        .filter(|c| !c.how.is_empty())
        .map(ToString::to_string)
        .collect();
    assert!(
        found.is_empty(),
        "{} cases differ:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn the_case_file_holds_the_decisions_cases_and_every_rule_has_its_own() {
    // Fable's oracle cases and main's measure tests, as docs/ports/measure.md ports them, by
    // name; and each comparison rule, by what Silico finds the elements to be, has cases.
    let cases = cases();
    let names: BTreeSet<&str> = cases.cases.iter().map(|c| c.name.as_str()).collect();
    for name in [
        "box_bottom_and_top",
        "parallel_faces_apart",
        "adjacent_faces_touch",
        "rims_r3_r4",
        "walls_r3_r4",
        "crossing_rims",
        "concentric_rims",
        "hole_rims",
        "hole_wall_to_counterbore_wall",
        "vertex_to_rim",
        "opposite_quarter_arcs",
        "skew_edges",
        "crossing_edges",
        "parallel_edges",
        "collinear_edges_apart",
        "circle_circle",
        "circle_circle_placed",
        "two_boxes_moved",
        "discs_overlapping",
        "disc_and_plane",
        "wedge_30",
        "wedge_60",
        "wedge_90",
        "v_groove",
        "ramp_from_a_flat",
        "chamfer_to_top",
        "chamfer_to_front",
        "split_top_halves_angle",
        "step_tops_angle",
        "wedge_30_rounded",
        "hexagon_top_edges",
        "split_edge_halves",
        "two_bodies_edges_at_120",
        "ten_degrees_one_and_two",
        "line_30_off_a_plane",
        "line_45_to_a_plane",
        "line_along_a_normal",
        "line_in_a_plane",
        "box_adjacent_faces",
        "box_edges_at_a_corner",
        "sphere_to_a_wall",
        "vertex_over_a_top",
        "vertex_beyond_a_tops_corner",
        "vertex_inside_to_a_top",
        "crossing_faces",
        "a_face_and_itself",
        "circle_and_a_line",
        "segment_over_a_segment",
    ] {
        assert!(names.contains(name), "{name} is missing");
    }
    let ours = measures::silico(&cases).unwrap();
    let mut rules: BTreeMap<Rule, usize> = BTreeMap::new();
    for (case, ours) in cases.cases.iter().zip(&ours) {
        let rule = Rule::of(&case.expected, &ours.found[0], &ours.found[1]).unwrap();
        *rules.entry(rule).or_default() += 1;
    }
    for rule in [
        Rule::Least,
        Rule::Centres,
        Rule::Supplement,
        Rule::Folded,
        Rule::FromNormal,
    ] {
        let n = rules.get(&rule).copied().unwrap_or_default();
        assert!(n >= 3, "{rule:?}: {n} cases");
    }
}

// ---- The rules, on answers made up here ----

fn face(surface: &'static str) -> Element {
    Element::Face {
        surface,
        area: 1.0,
        centroid: [0.0; 3],
    }
}

fn edge(curve: &'static str) -> Element {
    Element::Edge {
        curve,
        length: 1.0,
        start: [0.0; 3],
        end: [1.0, 0.0, 0.0],
        middle: [0.5, 0.0, 0.0],
    }
}

fn found(element: Element) -> Found {
    found_in("bxa00001", element)
}

fn found_in(body: &str, element: Element) -> Found {
    Found {
        body: FeatureId::new(body).unwrap(),
        element,
    }
}

fn mm(v: f64) -> Length {
    Length::rounded(v).unwrap()
}

fn distance_case(distance: f64, center: Option<f64>, normal: Option<f64>) -> MeasureCase {
    MeasureCase {
        name: "made_up".to_owned(),
        note: String::new(),
        elements: ["a".to_owned(), "b".to_owned()],
        expected: Expected::Distance(ExpectedDistance {
            distance,
            center_distance: center,
            normal_distance: normal,
            witness: None,
        }),
    }
}

fn angle_case(angle: f64, vertex: Option<[f64; 3]>) -> MeasureCase {
    MeasureCase {
        name: "made_up".to_owned(),
        note: String::new(),
        elements: ["a".to_owned(), "b".to_owned()],
        expected: Expected::Angle(ExpectedAngle { angle, vertex }),
    }
}

/// Silico's distance answer between points `from` and `to`, with these centre and normal
/// distances.
fn distance_answer(
    a: Element,
    b: Element,
    from: [f64; 3],
    to: [f64; 3],
    center: Option<f64>,
    normal: Option<f64>,
) -> Ours {
    let gap = (0..3)
        .map(|k| (to[k] - from[k]) * (to[k] - from[k]))
        .sum::<f64>()
        .sqrt();
    Ours {
        found: [found(a), found(b)],
        answer: Ok(Answer::Distance(DistanceAnswer {
            distance: mm(gap),
            from: from.map(mm),
            to: to.map(mm),
            center_distance: center.map(mm),
            normal_distance: normal.map(mm),
        })),
        vertex_at: None,
    }
}

fn angle_answer(a: Element, b: Element, degrees: f64) -> Ours {
    Ours {
        found: [found(a), found(b)],
        answer: Ok(Answer::Angle(AngleAnswer {
            angle: Angle::rounded(degrees).unwrap(),
            vertex: None,
        })),
        vertex_at: None,
    }
}

fn freecad_distance(distance: f64, dist_to_shape: f64, witness: [f64; 2]) -> Theirs {
    Theirs::Measured(Measured {
        elements: ["Face1".to_owned(), "Face2".to_owned()],
        distance: Some(distance),
        from: Some([0.0; 3]),
        to: Some([0.0, 0.0, distance]),
        dist_to_shape: Some(dist_to_shape),
        witness: Some(witness),
        angle: None,
    })
}

fn freecad_angle(angle: f64) -> Theirs {
    Theirs::Measured(Measured {
        elements: ["Face1".to_owned(), "Face2".to_owned()],
        distance: None,
        from: None,
        to: None,
        dist_to_shape: None,
        witness: None,
        angle: Some(angle),
    })
}

#[test]
fn each_rule_is_the_decisions_and_finds_a_value_off_it() {
    let rule_of = |case: &MeasureCase, a: &Element, b: &Element| {
        Rule::of(&case.expected, &found(a.clone()), &found(b.clone()))
    };
    let d = distance_case(10.0, None, None);
    let a = angle_case(30.0, None);
    assert_eq!(
        rule_of(&d, &edge("circle"), &edge("circle")),
        Ok(Rule::Centres)
    );
    assert_eq!(rule_of(&d, &edge("circle"), &edge("line")), Ok(Rule::Least));
    assert_eq!(
        rule_of(&d, &face("cylinder"), &face("cylinder")),
        Ok(Rule::Least)
    );
    assert_eq!(
        rule_of(&a, &face("plane"), &face("plane")),
        Ok(Rule::Supplement)
    );
    assert_eq!(rule_of(&a, &edge("line"), &edge("line")), Ok(Rule::Folded));
    assert_eq!(
        rule_of(&a, &edge("line"), &face("plane")),
        Ok(Rule::FromNormal)
    );
    assert_eq!(
        rule_of(&a, &face("plane"), &edge("line")),
        Ok(Rule::FromNormal)
    );
    assert!(rule_of(&a, &edge("circle"), &face("plane")).is_err());
    assert!(rule_of(&a, &face("cylinder"), &face("plane")).is_err());
    // Planar faces of two bodies: FreeCAD's turn of their normals may flip one and not the
    // other, so Supplement can't compare them, and the case is refused, saying why. Lines of
    // two bodies, a line and another's plane, and two bodies' distances are compared still.
    let two = |rule: &MeasureCase, a: Element, b: Element| {
        Rule::of(
            &rule.expected,
            &found_in("bxa00001", a),
            &found_in("bxb00001", b),
        )
    };
    let refused = two(&a, face("plane"), face("plane")).unwrap_err();
    assert!(
        refused.starts_with("the angle of planar faces of two bodies has no rule")
            && refused.ends_with("name two faces of one body"),
        "{refused}"
    );
    assert_eq!(two(&a, edge("line"), edge("line")), Ok(Rule::Folded));
    assert_eq!(two(&a, edge("line"), face("plane")), Ok(Rule::FromNormal));
    assert_eq!(two(&d, face("plane"), face("plane")), Ok(Rule::Least));

    // Supplement: a 30° wedge, FreeCAD's 150; FreeCAD's 0 (parallel) folds both, so 180 and
    // 0 agree with it, 90 doesn't; and 30 against FreeCAD's 30 is a difference.
    let planes = |deg| angle_answer(face("plane"), face("plane"), deg);
    let agrees =
        |rule, ours: &Ours, theirs: f64| compare(&a, rule, ours, &freecad_angle(theirs)).is_empty();
    assert!(agrees(Rule::Supplement, &planes(30.0), 150.0));
    assert!(agrees(Rule::Supplement, &planes(150.0), 30.0));
    assert!(agrees(Rule::Supplement, &planes(180.0), 0.0));
    assert!(agrees(Rule::Supplement, &planes(0.0), 0.0));
    assert!(!agrees(Rule::Supplement, &planes(30.0), 30.0));
    assert!(!agrees(Rule::Supplement, &planes(90.0), 0.0));
    // Within the tolerance, and just past it.
    assert!(agrees(Rule::Supplement, &planes(30.0), 150.0 + 5e-9));
    assert!(!agrees(Rule::Supplement, &planes(30.0), 150.0 + 2e-8));
    // Folded: 10 at a corner against FreeCAD's 170 or 10; not 160.
    let lines = |deg| angle_answer(edge("line"), edge("line"), deg);
    assert!(agrees(Rule::Folded, &lines(10.0), 170.0));
    assert!(agrees(Rule::Folded, &lines(10.0), 10.0));
    assert!(agrees(Rule::Folded, &lines(180.0), 0.0));
    assert!(agrees(Rule::Folded, &lines(120.0), 60.0));
    assert!(!agrees(Rule::Folded, &lines(10.0), 160.0));
    // FromNormal: 30 off a plane, FreeCAD's 120 or 60; 90 along the normal, FreeCAD's 0; not
    // FreeCAD's 30 as it is.
    let mixed = |deg| angle_answer(edge("line"), face("plane"), deg);
    assert!(agrees(Rule::FromNormal, &mixed(30.0), 120.0));
    assert!(agrees(Rule::FromNormal, &mixed(30.0), 60.0));
    assert!(agrees(Rule::FromNormal, &mixed(90.0), 0.0));
    assert!(agrees(Rule::FromNormal, &mixed(0.0), 90.0));
    assert!(!agrees(Rule::FromNormal, &mixed(30.0), 30.0));
    // A distance rule is no rule for an angle.
    assert!(!agrees(Rule::Least, &planes(30.0), 30.0));

    // Least: Silico's distance is FreeCAD's MeasureDistance, to a nanometre.
    let faces = distance_answer(
        face("plane"),
        face("plane"),
        [0.0; 3],
        [0.0, 0.0, 10.0],
        None,
        Some(10.0),
    );
    let against = |rule, ours: &Ours, theirs: &Theirs| compare(&d, rule, ours, theirs);
    assert!(against(Rule::Least, &faces, &freecad_distance(10.0, 10.0, [0.0; 2])).is_empty());
    assert!(
        against(
            Rule::Least,
            &faces,
            &freecad_distance(10.0 + 5e-7, 99.0, [0.0; 2])
        )
        .is_empty()
    );
    assert_eq!(
        against(
            Rule::Least,
            &faces,
            &freecad_distance(10.0 + 2e-6, 10.0, [0.0; 2])
        )
        .len(),
        1
    );
    // Silico's witness off its elements, as FreeCAD has them.
    let off = against(
        Rule::Least,
        &faces,
        &freecad_distance(10.0, 10.0, [0.0, 3e-6]),
    );
    assert_eq!(
        off,
        ["Silico's to is 0.000003 from elements.1 as FreeCAD has it"]
    );
    // Centres: FreeCAD's MeasureDistance is Silico's centre distance, its distToShape
    // Silico's distance; the two the other way round is two differences.
    let rims = distance_answer(
        edge("circle"),
        edge("circle"),
        [0.0, 3.0, 0.0],
        [0.0, 16.0, 0.0],
        Some(20.0),
        None,
    );
    assert!(
        against(
            Rule::Centres,
            &rims,
            &freecad_distance(20.0, 13.0, [0.0; 2])
        )
        .is_empty()
    );
    assert_eq!(
        against(
            Rule::Centres,
            &rims,
            &freecad_distance(13.0, 20.0, [0.0; 2])
        )
        .len(),
        2
    );
    let no_centres = distance_answer(
        edge("circle"),
        edge("circle"),
        [0.0, 3.0, 0.0],
        [0.0, 16.0, 0.0],
        None,
        None,
    );
    assert_eq!(
        against(
            Rule::Centres,
            &no_centres,
            &freecad_distance(20.0, 13.0, [0.0; 2])
        )
        .len(),
        1
    );
    // Either side not measuring is a difference.
    assert!(!against(Rule::Least, &faces, &Theirs::Failed("no".to_owned())).is_empty());
    assert!(!against(Rule::Least, &faces, &Theirs::Lost("crashed".to_owned())).is_empty());
    let refused = Ours {
        answer: Err(vec![Error::new(ErrorCode::InvalidInput, "refused")]),
        ..faces.clone()
    };
    assert!(
        !against(
            Rule::Least,
            &refused,
            &freecad_distance(10.0, 10.0, [0.0; 2])
        )
        .is_empty()
    );
}

#[test]
fn every_expected_value_is_checked() {
    // The box's bottom and top, as Silico answers them; each value the case gives, off by
    // a nanometre or a nanodegree, or a field the case says is null and Silico has, is a
    // difference.
    let ours = distance_answer(
        face("plane"),
        face("plane"),
        [0.0; 3],
        [0.0, 0.0, 10.0],
        None,
        Some(10.0),
    );
    let mut case = distance_case(10.0, None, Some(10.0));
    assert!(against_expected(&case, &ours).is_empty());
    for spoiled in [
        distance_case(10.000001, None, Some(10.0)),
        distance_case(10.0, Some(10.0), Some(10.0)),
        distance_case(10.0, None, None),
        distance_case(10.0, None, Some(9.999999)),
    ] {
        assert_eq!(against_expected(&spoiled, &ours).len(), 1, "{spoiled:?}");
    }
    if let Expected::Distance(d) = &mut case.expected {
        d.witness = Some([[0.0; 3], [0.0, 0.0, 10.0]]);
    }
    assert!(against_expected(&case, &ours).is_empty());
    if let Expected::Distance(d) = &mut case.expected {
        d.witness = Some([[0.0, 10.0, 0.0], [0.0, 10.0, 10.0]]);
    }
    assert_eq!(against_expected(&case, &ours).len(), 1);
    // A witness that isn't its distance apart.
    let mut far = ours.clone();
    if let Ok(Answer::Distance(d)) = &mut far.answer {
        d.to = [0.0, 0.0, 10.00001].map(mm);
    }
    assert_eq!(
        against_expected(&distance_case(10.0, None, Some(10.0)), &far).len(),
        1
    );
    // An angle, and the vertex it names, where the case has it.
    let mut at = angle_answer(edge("line"), edge("line"), 120.0);
    assert!(against_expected(&angle_case(120.0, None), &at).is_empty());
    assert_eq!(
        against_expected(&angle_case(120.000000001, None), &at).len(),
        1
    );
    assert_eq!(
        against_expected(&angle_case(120.0, Some([5.0, 0.0, 5.0])), &at).len(),
        1
    );
    at.vertex_at = Some([5.0, 0.0, 5.0]);
    if let Ok(Answer::Angle(a)) = &mut at.answer {
        a.vertex = Some("hex00001.pad.Vertex(G shx00001.sketch.Vertex.p0000002)".to_owned());
    }
    assert!(against_expected(&angle_case(120.0, Some([5.0, 0.0, 5.0])), &at).is_empty());
    assert_eq!(
        against_expected(&angle_case(120.0, Some([5.0, 0.000001, 5.0])), &at).len(),
        1
    );
    assert_eq!(against_expected(&angle_case(120.0, None), &at).len(), 1);
    // The answer of the other measure.
    assert_eq!(against_expected(&angle_case(10.0, None), &ours).len(), 1);
}

/// A copy of the oracle's measure cases with `cases.json` replaced by `text`.
fn load(text: &str) -> Result<Option<Cases>, String> {
    let dir = tempfile::tempdir().unwrap();
    let measures = dir.path().join(measures::DIR);
    std::fs::create_dir(&measures).unwrap();
    std::fs::copy(
        Path::new(ORACLE).join(measures::DIR).join(measures::MODEL),
        measures.join(measures::MODEL),
    )
    .unwrap();
    std::fs::write(measures.join(measures::CASES), text).unwrap();
    measures::load(dir.path())
}

#[test]
fn a_case_file_that_would_check_less_than_it_seems_is_refused() {
    let case = |name: &str, expected: &str| {
        format!(
            r#"{{"name": "{name}", "note": "", "elements": ["bxa00001.box.Face5", "bxa00001.box.Face6"], "expected": {expected}}}"#
        )
    };
    let good = r#"{"distance": {"distance": 10, "center_distance": null, "normal_distance": 10}}"#;
    assert_eq!(
        load(&format!("[{}]", case("a", good)))
            .unwrap()
            .unwrap()
            .cases
            .len(),
        1
    );
    // A null left out, as if it weren't checked.
    let missing = r#"{"distance": {"distance": 10, "normal_distance": 10}}"#;
    let refused = load(&format!("[{}]", case("a", missing))).unwrap_err();
    assert!(refused.contains("center_distance"), "{refused}");
    let no_vertex = r#"{"angle": {"angle": 0}}"#;
    let refused = load(&format!("[{}]", case("a", no_vertex))).unwrap_err();
    assert!(refused.contains("vertex"), "{refused}");
    // A value it doesn't know.
    let unknown = r#"{"distance": {"distance": 10, "center_distance": null, "normal_distance": 10, "length": 3}}"#;
    assert!(load(&format!("[{}]", case("a", unknown))).is_err());
    // Two cases of one name, and none.
    let refused = load(&format!("[{}, {}]", case("a", good), case("a", good))).unwrap_err();
    assert!(
        refused.contains("two measure cases are named a"),
        "{refused}"
    );
    assert!(load("[]").unwrap_err().contains("has no cases"));
    // A directory without measure cases has none.
    assert!(
        measures::load(tempfile::tempdir().unwrap().path())
            .unwrap()
            .is_none()
    );
}

#[test]
fn an_angle_of_two_bodies_planar_faces_is_a_case_file_error() {
    // The cube's top and the bottom of the cube apart from it: no rule compares them, and
    // the run fails, naming the case and saying why.
    let cases = load(
        r#"[{"name": "two_bodies_faces", "note": "", "elements": ["bxa00001.box.Face6", "bxb00001.box.Face5"],
            "expected": {"angle": {"angle": 180, "vertex": null}}}]"#,
    )
    .unwrap()
    .unwrap();
    let refused = measures::run(&cases).unwrap_err();
    assert!(
        refused.starts_with("two_bodies_faces: the angle of planar faces of two bodies"),
        "{refused}"
    );
}

#[test]
fn freecad_finds_each_element_by_its_geometry_or_says_it_cant() {
    // The first case, the box's bottom and top, as Silico found them: FreeCAD measures it.
    // With the bottom's centre moved a micrometre, no face of FreeCAD's box fits it, and it
    // says so.
    let cases = cases();
    let engine = Recompute::new(KernelPool::start().unwrap());
    let model = rebuild(&engine, cases.document.clone()).unwrap().unwrap();
    let first = &cases.cases[..1];
    let ours = measures::answers(&model, engine.kernel(), first).unwrap();
    let theirs = measures::freecad(&model, first, &ours).unwrap();
    assert!(
        matches!(&theirs[0], Theirs::Measured(m) if m.elements == ["Face5", "Face6"]),
        "{theirs:?}"
    );
    let mut moved = ours.clone();
    if let Element::Face { centroid, .. } = &mut moved[0].found[0].element {
        centroid[0] += 0.001;
    }
    let theirs = measures::freecad(&model, first, &moved).unwrap();
    assert!(
        matches!(&theirs[0], Theirs::Failed(why) if why.starts_with("0 of its faces fit")),
        "{theirs:?}"
    );
    // And a spoiled Silico distance is a difference against what FreeCAD measured.
    let theirs = measures::freecad(&model, first, &ours).unwrap();
    let mut wrong = ours[0].clone();
    if let Ok(Answer::Distance(d)) = &mut wrong.answer {
        d.distance = mm(10.001);
    }
    let rule = Rule::of(&first[0].expected, &wrong.found[0], &wrong.found[1]).unwrap();
    assert_eq!(compare(&first[0], rule, &wrong, &theirs[0]).len(), 1);
}
