//! The sketch-feature oracle (M3.6, docs/design/m3.6-sketch-features.md, S9): the cases of
//! tests/oracle/sketch-features.silico (protected), each built by FreeCAD 1.1.3's PartDesign
//! and checked against its analytic expected values; that the harness sees a FreeCAD model
//! built otherwise than the document says; and Silico's rebuild of them, compared with
//! FreeCAD's and the expected values.
//! Needs the FreeCAD oracle (`pixi install -e oracle`) and the workers built
//! (`cargo nextest run --workspace` builds them).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::Path;

use silico_oracle::features::{
    self, Cases, Extrusion, FeatureCase, PartDesignFeature, Revolution, RevolutionAxis,
    SketchGeometry, against_expected, compare,
};
use silico_oracle::{Outcome, Tolerance};

const CASES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/oracle");

fn cases() -> Cases {
    features::load(Path::new(CASES)).unwrap().unwrap()
}

fn report(found: &[String]) -> String {
    format!("{} cases:\n{}", found.len(), found.join("\n"))
}

#[test]
fn the_case_file_holds_every_case_of_the_design() {
    // Design S9's cases: those ported from FreeCAD by name, and Silico's own by how many of
    // each kind (each pad and pocket form on each plane, the torus segments, the grooves).
    let cases = cases();
    let names: BTreeSet<&str> = cases.cases.iter().map(|c| c.name.as_str()).collect();
    for name in [
        "pocket_dimension",
        "pocket_through_all",
        "pad_two_dimensions",
        "pad_two_lengths_circle",
        "slot_plate",
        "unit_square_pad",
        "pocket_in_box",
        "groove_control",
        "two_holes",
        "flush_pad",
        "through_rim",
        "sphere",
        "rectangle_about_its_side_360_xy",
        "rectangle_about_its_side_90_yz",
        "groove_both_sides_360",
        "groove_both_sides_181",
        "groove_both_sides_180",
        "groove_both_sides_near_270",
        "groove_both_sides_mirrored_360",
        "cylinder_revolved_onto_a_box",
    ] {
        assert!(names.contains(name), "{name} is missing");
    }
    let count = |prefix: &str| names.iter().filter(|n| n.starts_with(prefix)).count();
    assert_eq!(count("pad_one_side_") + count("pad_symmetric_"), 9);
    assert_eq!(count("pad_two_sides_"), 3);
    assert_eq!(
        count("pocket_"),
        4 + 18,
        "the ported pockets and each pocket form"
    );
    assert_eq!(
        count("torus_"),
        42 + 3,
        "the design's torus segments, and the M3 close's three about the x axis"
    );
    assert_eq!(
        count("groove_"),
        2 + 24 + 10,
        "the ported groove, each groove and its cylinder, and the five grooves of regions either side of their axis (council round 2) and their cylinders"
    );
    // And the M3 close's four revolves (three tori about the x axis, a cylinder onto a box).
    assert_eq!(cases.cases.len(), 124 + 4);
}

#[test]
fn freecad_builds_every_sketch_feature_case_to_its_expected_values() {
    // Each case built by FreeCAD 1.1.3's Pad, Pocket, Revolution, and Groove, unrefined,
    // from the same sketches: its volume, and its counts, box, and center where the case
    // gives them, are the formulas' (docs/ports/sketch-features.md has each FreeCAD test's).
    let cases = cases();
    let theirs = features::freecad(&cases.cases).unwrap();
    assert_eq!(theirs.len(), cases.cases.len());
    let mut wrong = Vec::new();
    for (case, outcome) in cases.cases.iter().zip(&theirs) {
        let how = against_expected(case, outcome);
        if !how.is_empty() {
            wrong.push(format!("{}: {}", case.name, how.join("; ")));
        }
    }
    assert!(wrong.is_empty(), "{}", report(&wrong));
}

/// The case named `name`.
fn case(cases: &Cases, name: &str) -> FeatureCase {
    cases.cases.iter().find(|c| c.name == name).unwrap().clone()
}

/// The last feature of `case`'s model, changed by `spoil`.
fn spoiled(mut case: FeatureCase, spoil: impl FnOnce(&mut PartDesignFeature)) -> FeatureCase {
    spoil(case.model.last_mut().unwrap());
    case
}

#[test]
fn the_harness_catches_a_freecad_model_built_otherwise_than_the_document_says() {
    // Each of the harness's translations to FreeCAD, done wrong on a case FreeCAD builds to
    // its expected values: the case's expected values say so. A pad not reversed (it goes
    // up, not down), a symmetric pad as one side, two sides' lengths swapped, through all
    // reversed, a revolve the other way, about the sketch's other axis, a groove symmetric
    // as one side, and a revolve about a line as its sketch's y axis.
    fn extrusion(f: &mut PartDesignFeature) -> Option<&mut Extrusion> {
        match f {
            PartDesignFeature::Pad(e) | PartDesignFeature::Pocket(e) => Some(e),
            _ => None,
        }
    }
    fn revolution(f: &mut PartDesignFeature) -> Option<&mut Revolution> {
        match f {
            PartDesignFeature::Revolution(r) | PartDesignFeature::Groove(r) => Some(r),
            _ => None,
        }
    }
    let cases = cases();
    let wrong = [
        spoiled(case(&cases, "pocket_dimension_plate"), |f| {
            extrusion(f).unwrap().reversed = false;
        }),
        spoiled(case(&cases, "pad_symmetric_xz"), |f| {
            extrusion(f).unwrap().side_type = "One side";
        }),
        spoiled(case(&cases, "pad_two_sides_yz"), |f| {
            let e = extrusion(f).unwrap();
            (e.length, e.length2) = (e.length2, e.length);
        }),
        spoiled(case(&cases, "pocket_through_all_xy"), |f| {
            extrusion(f).unwrap().reversed = true;
        }),
        spoiled(case(&cases, "torus_90_one_side_xz"), |f| {
            revolution(f).unwrap().reversed = true;
        }),
        spoiled(case(&cases, "torus_180_one_side_yz"), |f| {
            revolution(f).unwrap().axis = RevolutionAxis::Sketch("H_Axis");
        }),
        spoiled(case(&cases, "groove_90_symmetric_xy"), |f| {
            revolution(f).unwrap().midplane = false;
        }),
        spoiled(case(&cases, "torus_90_one_side_xy_about_a_line"), |f| {
            revolution(f).unwrap().axis = RevolutionAxis::Sketch("V_Axis");
        }),
    ];
    let theirs = features::freecad(&wrong).unwrap();
    for (case, outcome) in wrong.iter().zip(&theirs) {
        assert!(
            !against_expected(case, outcome).is_empty(),
            "{} built wrong passes: {outcome:?}",
            case.name
        );
    }
}

/// A document of `features` (JSON, after the header).
fn document(features: &str) -> Vec<u8> {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{features}]}}"
    )
    .into_bytes()
}

const SQUARE: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xz",
  "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 4, "y": 0, "construction": false}, {"id": "p0000003", "x": 4, "y": 4, "construction": false}, {"id": "p0000004", "x": 0, "y": 4, "construction": false}, {"id": "p0000005", "x": 2, "y": 2, "construction": false}],
  "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false}, {"id": "n0000003", "from": "p0000003", "to": "p0000004", "construction": false}, {"id": "n0000004", "from": "p0000004", "to": "p0000001", "construction": false}, {"id": "n0000005", "from": "p0000001", "to": "p0000003", "construction": true}],
  "circles": [{"id": "c0000001", "center": "p0000005", "radius": 1, "construction": false}],
  "arcs": [], "constraints": []}"#;

fn load(text: &str) -> Result<Option<Cases>, String> {
    // A directory of each call's own (tests run at once), removed however the call ends.
    let temp = tempfile::Builder::new()
        .prefix("silico-feature-cases-")
        .tempdir()
        .unwrap();
    std::fs::write(temp.path().join(features::FILE), document(text)).unwrap();
    features::load(temp.path())
}

#[test]
fn each_feature_is_freecads_as_the_port_note_maps_it() {
    // docs/ports/sketch-features.md, *What M3.6 builds*, written out by hand: one side with
    // Reversed, symmetric as Symmetric, two sides as Two sides with both lengths, through
    // all as ThroughAll; a revolve's angle with Midplane for symmetric; its axis the
    // sketch's V_Axis for y_axis, or a line by its ID; a box at its position.
    let features = format!(
        r#"{{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [-1, -2, -3], "length": 10, "width": 20, "height": 30}},
        {SQUARE},
        {{"id": "p0000001", "type": "pad", "v": 1, "name": "pad", "base": "b0000001", "sketch": "s0000001", "extent": {{"type": "two_sides", "length": 3, "length2": 0.5}}, "note": "Expected: volume 1."}},
        {{"id": "q0000001", "type": "pocket", "v": 1, "name": "pocket", "base": "p0000001", "sketch": "s0000001", "extent": {{"type": "through_all", "reversed": true}}, "note": "Expected: volume 2."}},
        {{"id": "r0000001", "type": "revolve", "v": 1, "name": "revolve", "base": "q0000001", "sketch": "s0000001", "axis": "y_axis", "extent": {{"type": "symmetric", "angle": 90}}, "note": "Expected: volume 3."}},
        {{"id": "g0000001", "type": "groove", "v": 1, "name": "groove", "base": "r0000001", "sketch": "s0000001", "axis": "n0000005", "extent": {{"type": "one_side", "angle": 45.5, "reversed": true}}, "note": "Expected: volume 4; faces 5; edges 6; vertices 7; box 0 1 2 3 4 5; center 1 2 3."}}"#
    );
    let loaded = load(&features).unwrap().unwrap();
    let groove = loaded.cases.last().unwrap();
    assert_eq!(loaded.cases.len(), 4);
    assert_eq!(
        (
            groove.expected.faces,
            groove.expected.edges,
            groove.expected.vertices
        ),
        (Some(5), Some(6), Some(7))
    );
    assert_eq!(
        groove.expected.bounds,
        Some([[0.0, 1.0, 2.0], [3.0, 4.0, 5.0]])
    );
    assert_eq!(groove.expected.center, Some([1.0, 2.0, 3.0]));
    let sketch = PartDesignFeature::Sketch {
        id: "s0000001".to_owned(),
        plane: "XZ_Plane",
        geometry: vec![
            SketchGeometry::Line {
                id: "n0000001".to_owned(),
                start: [0.0, 0.0],
                end: [4.0, 0.0],
                construction: false,
            },
            SketchGeometry::Line {
                id: "n0000002".to_owned(),
                start: [4.0, 0.0],
                end: [4.0, 4.0],
                construction: false,
            },
            SketchGeometry::Line {
                id: "n0000003".to_owned(),
                start: [4.0, 4.0],
                end: [0.0, 4.0],
                construction: false,
            },
            SketchGeometry::Line {
                id: "n0000004".to_owned(),
                start: [0.0, 4.0],
                end: [0.0, 0.0],
                construction: false,
            },
            SketchGeometry::Line {
                id: "n0000005".to_owned(),
                start: [0.0, 0.0],
                end: [4.0, 4.0],
                construction: true,
            },
            SketchGeometry::Circle {
                id: "c0000001".to_owned(),
                center: [2.0, 2.0],
                radius: 1.0,
                construction: false,
            },
        ],
    };
    let want = vec![
        PartDesignFeature::Primitive {
            id: "b0000001".to_owned(),
            shape: "box",
            position: [-1.0, -2.0, -3.0],
            values: vec![10.0, 20.0, 30.0],
        },
        sketch,
        PartDesignFeature::Pad(Extrusion {
            id: "p0000001".to_owned(),
            sketch: "s0000001".to_owned(),
            side_type: "Two sides",
            method: "Length",
            length: 3.0,
            length2: 0.5,
            reversed: false,
        }),
        PartDesignFeature::Pocket(Extrusion {
            id: "q0000001".to_owned(),
            sketch: "s0000001".to_owned(),
            side_type: "One side",
            method: "ThroughAll",
            length: 0.0,
            length2: 0.0,
            reversed: true,
        }),
        PartDesignFeature::Revolution(Revolution {
            id: "r0000001".to_owned(),
            sketch: "s0000001".to_owned(),
            axis: RevolutionAxis::Sketch("V_Axis"),
            angle: 90.0,
            midplane: true,
            reversed: false,
        }),
        PartDesignFeature::Groove(Revolution {
            id: "g0000001".to_owned(),
            sketch: "s0000001".to_owned(),
            axis: RevolutionAxis::Line("n0000005".to_owned()),
            angle: 45.5,
            midplane: false,
            reversed: true,
        }),
    ];
    assert_eq!(groove.model, want);
    // The pad's case is built from the box, the sketch, and the pad: no later feature.
    assert_eq!(loaded.cases[0].model, want[..3]);
    // A sketch of a circle onto a box is a curved boolean; the grooves likewise.
    assert_eq!(
        loaded.cases[0].tolerance,
        silico_oracle::Tolerance::CURVED_BOOLEAN
    );
}

#[test]
fn a_case_file_that_would_check_less_than_it_seems_is_refused() {
    let pad = |id: &str, base: &str, note: &str| {
        let base = if base.is_empty() {
            String::new()
        } else {
            format!(r#""base": "{base}", "#)
        };
        format!(
            r#"{{"id": "{id}", "type": "pad", "v": 1, "name": "{id}", {base}"sketch": "s0000001", "extent": {{"type": "one_side", "length": 1, "reversed": false}}, "note": "{note}"}}"#
        )
    };
    let box_ = |note: &str| {
        format!(
            r#"{{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1, "note": "{note}"}}"#
        )
    };
    for (features, why) in [
        (
            format!("{SQUARE}, {}", pad("p0000001", "", "no values")),
            "doesn't say what it's expected",
        ),
        (
            format!("{SQUARE}, {}", pad("p0000001", "", "Expected: faces 6.")),
            "don't give its volume",
        ),
        (
            format!("{SQUARE}, {}", pad("p0000001", "", "Expected: volume 1")),
            "full stop",
        ),
        (
            format!(
                "{SQUARE}, {}",
                pad("p0000001", "", "Expected: volume 1; area 2.")
            ),
            "\"area\" isn't something",
        ),
        (
            format!(
                "{SQUARE}, {}",
                pad("p0000001", "", "Expected: volume 1; volume 2.")
            ),
            "volume is expected twice",
        ),
        (
            format!("{SQUARE}, {}", pad("p0000001", "", "Expected: volume NaN.")),
            "isn't a finite number",
        ),
        (
            format!(
                "{SQUARE}, {}",
                pad("p0000001", "", "Expected: volume 1; box 1 2 3.")
            ),
            "box takes 6 numbers",
        ),
        (
            format!(
                "{SQUARE}, {}",
                pad("p0000001", "", "Expected: volume 1; faces 6.5.")
            ),
            "isn't a count",
        ),
        (
            format!(
                "{SQUARE}, {}, {}",
                pad("p0000001", "", "Expected: volume 1."),
                pad("p0000002", "", "Expected: volume 1.")
                    .replace(r#""name": "p0000002""#, r#""name": "p0000001""#)
            ),
            "two cases are named p0000001",
        ),
        (
            format!("{}, {SQUARE}", box_("a box")),
            "makes a solid that isn't a case",
        ),
        (
            format!(
                "{}, {SQUARE}, {}",
                box_("Expected: volume 1."),
                pad("p0000001", "b0000001", "Expected: volume 2.")
            ),
            "isn't a pad, pocket, revolve, or groove",
        ),
    ] {
        let e = load(&features).unwrap_err();
        assert!(e.contains(why), "{why}: {e}");
    }
    // A boolean between a case and its base: FreeCAD's body has none to build.
    let fused = format!(
        r#"{}, {{"id": "b0000002", "type": "box", "v": 1, "name": "B2", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}},
        {{"id": "f0000001", "type": "fuse", "v": 1, "name": "F", "base": "b0000001", "tool": "b0000002"}}, {SQUARE}, {}"#,
        box_("a box"),
        pad("p0000001", "f0000001", "Expected: volume 1.")
    );
    let e = load(&fused).unwrap_err();
    assert!(e.contains("f0000001 is a fuse"), "{e}");
    // No file: no cases.
    let temp = tempfile::Builder::new()
        .prefix("silico-feature-none-")
        .tempdir()
        .unwrap();
    assert!(features::load(temp.path()).unwrap().is_none());
}

fn measured(case: &FeatureCase) -> silico_oracle::Measured {
    let want = &case.expected;
    silico_oracle::Measured {
        volume: want.volume,
        area: 1.0,
        bounds: want.bounds,
        center: want.center,
        faces: want.faces.unwrap(),
        edges: want.edges.unwrap(),
        vertices: want.vertices.unwrap(),
        valid: true,
    }
}

#[test]
fn every_expected_value_is_checked() {
    // A case with every value given, measured as expected, agrees; each value spoiled
    // past the tolerance (planar: 1e-9) is reported, and a failure is never agreement.
    let cases = cases();
    let case = case(&cases, "pocket_in_box");
    assert_eq!(case.tolerance, silico_oracle::Tolerance::PLANAR);
    let good = measured(&case);
    assert!(against_expected(&case, &Outcome::Built(good.clone())).is_empty());
    let mut near = good.clone();
    near.volume *= 1.0 + 5e-10;
    assert!(against_expected(&case, &Outcome::Built(near)).is_empty());
    // The center is compared relative to the box's largest side, 10 mm here: 5e-9 off is
    // within the 1e-9 × 10 allowed, though past 1e-9 itself.
    let mut near = good.clone();
    near.center.as_mut().unwrap()[2] += 5e-9;
    assert!(against_expected(&case, &Outcome::Built(near)).is_empty());
    type Spoil = fn(&mut silico_oracle::Measured);
    let spoils: [(&str, Spoil); 10] = [
        ("volume", |m| m.volume *= 1.0 + 2e-9),
        ("faces", |m| m.faces += 1),
        ("edges", |m| m.edges -= 1),
        ("vertices", |m| m.vertices += 2),
        ("not valid", |m| m.valid = false),
        ("box", |m| m.bounds.as_mut().unwrap()[1][2] += 1e-6),
        ("box", |m| m.bounds = None),
        ("center", |m| m.center.as_mut().unwrap()[0] += 1e-6),
        ("center", |m| m.center = None),
        ("volume", |m| m.volume = -m.volume),
    ];
    for (what, spoil) in spoils {
        let mut m = good.clone();
        spoil(&mut m);
        let how = against_expected(&case, &Outcome::Built(m));
        assert!(
            how.len() == 1 && how[0].starts_with(what),
            "{what}: {how:?}"
        );
    }
    for outcome in [
        Outcome::Failed("refused".to_owned()),
        Outcome::Lost("crashed".to_owned()),
    ] {
        assert!(!against_expected(&case, &outcome).is_empty());
    }
}

#[test]
fn silicos_kernel_faults_are_lost_and_its_refusals_and_blocks_failed() {
    // A worker that crashed, hung, or couldn't be had, or a fault of the kernel's own, never
    // finished the case: lost, which never agrees with FreeCAD, even with its refusal. A
    // refusal of the model (a value out of range, the kernel failing on it) is a failure, as
    // FreeCAD's own refusals are; so is a feature not built for one that failed before it.
    use silico_core::{Error, ErrorCode};
    use silico_oracle::features::Ours;
    for code in [
        ErrorCode::WorkerCrashed,
        ErrorCode::WorkerTimeout,
        ErrorCode::ProtocolError,
        ErrorCode::ResourceExhausted,
        ErrorCode::Internal,
        ErrorCode::WorkerUnavailable,
    ] {
        let outcome = Ours::Failed(Error::new(code, "faulted")).outcome();
        assert!(matches!(outcome, Outcome::Lost(_)), "{code:?}: {outcome:?}");
        let refused = Outcome::Failed("refused".to_owned());
        assert!(!silico_oracle::differences(&outcome, &refused, Tolerance::PLANAR).is_empty());
    }
    for code in [
        ErrorCode::InvalidInput,
        ErrorCode::KernelFailed,
        ErrorCode::InvalidResult,
    ] {
        let outcome = Ours::Failed(Error::new(code, "refused")).outcome();
        assert!(
            matches!(outcome, Outcome::Failed(_)),
            "{code:?}: {outcome:?}"
        );
    }
    let on = silico_core::document::FeatureId::new("p0000001").unwrap();
    let outcome = Ours::Blocked(on).outcome();
    assert!(
        matches!(&outcome, Outcome::Failed(why) if why.contains("p0000001")),
        "{outcome:?}"
    );
}

#[test]
fn every_sketch_feature_case_matches_freecad() {
    // Silico's rebuild of the document through the core's commands (M3.6, step iv), each
    // case compared with FreeCAD's and with its expected values: every one of them.
    let cases = cases();
    let differ: Vec<String> = features::run(&cases)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(differ.is_empty(), "{}", report(&differ));
}

#[test]
fn a_silico_rebuild_that_differs_from_freecad_or_its_values_is_reported() {
    // The comparison can fail: a case Silico refuses where FreeCAD builds it differs, and
    // so does one whose volume isn't the expected.
    let cases = cases();
    let case = case(&cases, "pocket_in_box");
    let theirs = Outcome::Built(measured(&case));
    let refused = Outcome::Failed("refused".to_owned());
    assert!(!compare(&case, &refused, &theirs).is_empty());
    let mut off = measured(&case);
    off.volume += 1.0;
    assert!(!against_expected(&case, &Outcome::Built(off)).is_empty());
}
