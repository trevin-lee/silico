//! The pattern oracle (M6.4, docs/design/m6.0-patterns.md, section 11; docs/roadmap/
//! m6-features.md, M6.4): the cases of tests/oracle/patterns.silico (protected), each linear
//! pattern, polar pattern, and mirror built by FreeCAD 1.1.3's PartDesign `LinearPattern`,
//! `PolarPattern`, and `Mirrored` (`Refine = False`) and checked against its expected values,
//! worked by hand in its note; that the harness sees a FreeCAD pattern built otherwise than
//! the document says, a face's direction FreeCAD's surface axis among them (decision 3); what
//! the harness refuses to build; and Silico's rebuild, compared with FreeCAD's and the
//! expected values.
//! Needs the FreeCAD oracle (`pixi install -e oracle`) and the workers built
//! (`cargo nextest run --workspace` builds them).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::Path;

use silico_core::document::FeatureId;
use silico_oracle::features::{
    self, Cases, FeatureCase, LinearPattern, Mirrored, Originals, PartDesignFeature,
    PatternDirection, PolarPattern, Reference, against_expected,
};
use silico_oracle::measures::Element;

const CASES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/oracle");

fn cases() -> Cases {
    features::load_patterns(Path::new(CASES)).unwrap().unwrap()
}

fn report(found: &[String]) -> String {
    format!("{} cases:\n{}", found.len(), found.join("\n"))
}

/// The pattern cases, by name, in the file's order: each FreeCAD test M6.3 ports that 1.1.3
/// builds as Silico does, patterns.md's probes, and a case for each kind of reference and
/// each form of the document.
const PATTERNS: [&str; 27] = [
    "linear_x",
    "linear_y",
    "linear_z",
    "polar_z",
    "polar_x",
    "polar_y",
    "mirror_xy",
    "mirror_whole_xy",
    "three_cylinders",
    "three_holes",
    "mirror_off_axis",
    "bottom_face",
    "side_face",
    "edge_axis",
    "edge_direction",
    "rim_axis",
    "half_turn",
    "reversed_quarter",
    "spacing_45",
    "one_instance",
    "grid",
    "reversed_x",
    "mirror_notch",
    "face_plane",
    "nested_pattern",
    "nested",
    "groove_copies",
];

#[test]
fn the_pattern_file_holds_every_case_of_the_step() {
    // Every pattern case, and nothing else is a pattern: the 27 patterns and the 41 pads,
    // pockets, and the groove they're built on, each a case of its own.
    let cases = cases();
    let patterns: Vec<&str> = cases
        .cases
        .iter()
        .filter(|c| {
            matches!(
                c.model.last(),
                Some(
                    PartDesignFeature::LinearPattern(_)
                        | PartDesignFeature::PolarPattern(_)
                        | PartDesignFeature::Mirrored(_)
                )
            )
        })
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(patterns, PATTERNS);
    assert_eq!(cases.cases.len(), 27 + 41);
    let names: BTreeSet<&str> = cases.cases.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names.len(), cases.cases.len());
}

/// The case named `name`.
fn case(cases: &[FeatureCase], name: &str) -> FeatureCase {
    cases.iter().find(|c| c.name == name).unwrap().clone()
}

#[test]
fn freecad_builds_every_pattern_case_to_its_expected_values() {
    // Each case built by FreeCAD 1.1.3's PartDesign, unrefined, its references found by
    // their geometry as Silico's rebuild describes them: its volume, and its counts, box,
    // and center where the case gives them, are the ones worked by hand in its note.
    let described = features::described(&cases()).unwrap();
    let theirs = features::freecad(&described).unwrap();
    assert_eq!(theirs.len(), described.len());
    let mut wrong = Vec::new();
    for (case, outcome) in described.iter().zip(&theirs) {
        let how = against_expected(case, outcome);
        if !how.is_empty() {
            wrong.push(format!("{}: {}", case.name, how.join("; ")));
        }
    }
    assert!(wrong.is_empty(), "{}", report(&wrong));
}

#[test]
fn every_pattern_case_matches_freecad() {
    // Silico's rebuild of the document through the core's commands, each case compared with
    // FreeCAD's and with its expected values: every one of them.
    let differ: Vec<String> = features::run(&cases())
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(differ.is_empty(), "{}", report(&differ));
}

/// The last feature of `case`'s model, changed by `spoil`.
fn spoiled(mut case: FeatureCase, spoil: impl FnOnce(&mut PartDesignFeature)) -> FeatureCase {
    spoil(case.model.last_mut().unwrap());
    case
}

fn linear(f: &mut PartDesignFeature) -> &mut LinearPattern {
    match f {
        PartDesignFeature::LinearPattern(p) => p,
        other => panic!("{other:?}"),
    }
}

fn polar(f: &mut PartDesignFeature) -> &mut PolarPattern {
    match f {
        PartDesignFeature::PolarPattern(p) => p,
        other => panic!("{other:?}"),
    }
}

fn mirrored(f: &mut PartDesignFeature) -> &mut Mirrored {
    match f {
        PartDesignFeature::Mirrored(m) => m,
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_harness_catches_a_freecad_pattern_built_otherwise_than_the_document_says() {
    // Each of the translation's choices done wrong on a case FreeCAD builds to its expected
    // values: the case's expected values say so. A direction reversed; extent taken as
    // spacing; an axis swapped; a polar extent as spacing (copies past a turn); a reversed
    // polar not reversed; a mirror in the wrong plane; a feature's copies as the whole
    // shape's; a grid's second direction dropped; both ways for decision 3, a face's
    // direction left as FreeCAD's surface axis, which points into the block; and the block
    // turned about its hole's rim the other way.
    let described = features::described(&cases()).unwrap();
    let wrong = [
        spoiled(case(&described, "linear_x"), |f| {
            linear(f).directions[0].reversed = true;
        }),
        spoiled(case(&described, "linear_y"), |f| {
            linear(f).directions[0].mode = "Spacing";
        }),
        spoiled(case(&described, "polar_z"), |f| {
            polar(f).axis = Reference::Origin { role: "X_Axis" };
        }),
        spoiled(case(&described, "half_turn"), |f| {
            polar(f).mode = "Spacing";
        }),
        spoiled(case(&described, "reversed_quarter"), |f| {
            polar(f).reversed = false;
        }),
        spoiled(case(&described, "mirror_xy"), |f| {
            mirrored(f).plane = Reference::Origin { role: "XZ_Plane" };
        }),
        spoiled(case(&described, "three_cylinders"), |f| {
            linear(f).originals = Originals::WholeShape;
        }),
        spoiled(case(&described, "grid"), |f| {
            linear(f).directions.pop();
        }),
        spoiled(case(&described, "bottom_face"), |f| {
            linear(f).directions[0].outward = false;
        }),
        spoiled(case(&described, "rim_axis"), |f| {
            polar(f).reversed = true;
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
    // The face's direction left as FreeCAD's: the copy goes up, not down (decision 3): the
    // same volume, the box from z = 0 to 20, as patterns.md §1 has a box's Face5.
    let bottom = wrong.iter().position(|c| c.name == "bottom_face").unwrap();
    let silico_oracle::Outcome::Built(measured) = &theirs[bottom] else {
        panic!("{:?}", theirs[bottom]);
    };
    assert!((measured.volume - 16_000.0).abs() < 1e-6, "{measured:?}");
    let [low, high] = measured.bounds.unwrap();
    assert!(
        low[2].abs() < 1e-9 && (high[2] - 20.0).abs() < 1e-9,
        "{measured:?}"
    );
}

/// A document of `features` (JSON, after the header), under scheme 6.
fn document(features: &str) -> Vec<u8> {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 6, \"features\": [{features}]}}"
    )
    .into_bytes()
}

fn load(text: &str) -> Result<Option<Cases>, String> {
    // A directory of each call's own (tests run at once), removed however the call ends.
    let temp = tempfile::Builder::new()
        .prefix("silico-pattern-cases-")
        .tempdir()
        .unwrap();
    std::fs::write(temp.path().join(features::PATTERNS), document(text)).unwrap();
    features::load_patterns(temp.path())
}

/// A square sketched on xy, `s0000001`, and its pad `p0000001`, 10 tall; then a hole,
/// `q0000002`, and a boss, `p0000002`, each from a circle of their own.
const SEED: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy",
  "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 40, "y": 0, "construction": false}, {"id": "pa000003", "x": 40, "y": 20, "construction": false}, {"id": "pa000004", "x": 0, "y": 20, "construction": false}],
  "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}],
  "circles": [], "arcs": [], "constraints": []},
  {"id": "p0000001", "type": "pad", "v": 1, "name": "pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 10, "reversed": false}, "note": "Expected: volume 8000."},
  {"id": "s0000002", "type": "sketch", "v": 1, "name": "H", "plane": "xy", "points": [{"id": "pa000001", "x": 5, "y": 5, "construction": false}], "lines": [], "circles": [{"id": "ca000001", "center": "pa000001", "radius": 1, "construction": false}], "arcs": [], "constraints": []},
  {"id": "q0000002", "type": "pocket", "v": 1, "name": "hole", "base": "p0000001", "sketch": "s0000002", "extent": {"type": "through_all", "reversed": true}, "note": "Expected: volume 1."},
  {"id": "s0000003", "type": "sketch", "v": 1, "name": "B", "plane": "xy", "points": [{"id": "pa000001", "x": 15, "y": 5, "construction": false}], "lines": [], "circles": [{"id": "ca000001", "center": "pa000001", "radius": 1, "construction": false}], "arcs": [], "constraints": []},
  {"id": "p0000002", "type": "pad", "v": 1, "name": "boss", "base": "q0000002", "sketch": "s0000003", "extent": {"type": "one_side", "length": 12, "reversed": false}, "note": "Expected: volume 2."}"#;

#[test]
fn each_pattern_is_freecads_as_the_port_note_maps_it() {
    // Written out by hand from docs/ports/patterns.md §1 and §7: a linear pattern's
    // directions as Direction and Direction2, an origin axis as the body's X_Axis, a name as
    // an element of the base's solid, extent as Extent's Length and spacing as Spacing's
    // Offset, a face's direction corrected to its outward normal; the originals in timeline
    // order (the boss p0000002 after the hole q0000002), whatever their IDs' order; a polar
    // pattern's spacing as Spacing's Offset in degrees, reversed as Reversed, about an edge;
    // a mirror's origin plane as XZ_Plane, and the whole solid as Whole shape.
    let features = format!(
        r#"{SEED},
        {{"id": "a0000001", "type": "linear_pattern", "v": 1, "name": "linear", "base": "p0000002", "copies": {{"type": "features", "features": ["p0000002", "q0000002"]}}, "directions": [{{"along": "x", "reversed": true, "distribution": {{"type": "extent", "length": 20}}, "count": 3}}, {{"along": "p0000001.pad.Face(M s0000001.sketch.Face1)", "reversed": false, "distribution": {{"type": "spacing", "length": 2.5}}, "count": 2}}], "skip": [], "note": "Expected: volume 3."}},
        {{"id": "a0000002", "type": "polar_pattern", "v": 1, "name": "polar", "base": "a0000001", "copies": {{"type": "features", "features": ["q0000002"]}}, "axis": "p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000002)", "reversed": true, "distribution": {{"type": "spacing", "angle": 30.5}}, "count": 4, "skip": [], "note": "Expected: volume 4."}},
        {{"id": "a0000003", "type": "mirror", "v": 1, "name": "mirror", "base": "a0000002", "copies": {{"type": "solid"}}, "plane": "xz", "note": "Expected: volume 5."}}"#
    );
    let loaded = load(&features).unwrap().unwrap();
    assert_eq!(loaded.cases.len(), 6);
    let model = &loaded.cases[5].model;
    // The body: the three sketches and their three features, then the three patterns.
    assert_eq!(model.len(), 9);
    let element = |of: &str, name: &str| Reference::Element {
        of: of.to_owned(),
        name: name.to_owned(),
        element: None,
    };
    assert_eq!(
        model[6],
        PartDesignFeature::LinearPattern(LinearPattern {
            id: "a0000001".to_owned(),
            originals: Originals::Features(vec!["q0000002".to_owned(), "p0000002".to_owned()]),
            directions: vec![
                PatternDirection {
                    reference: Reference::Origin { role: "X_Axis" },
                    reversed: true,
                    mode: "Extent",
                    length: 20.0,
                    occurrences: 3,
                    outward: true,
                },
                PatternDirection {
                    reference: element("p0000002", "p0000001.pad.Face(M s0000001.sketch.Face1)"),
                    reversed: false,
                    mode: "Spacing",
                    length: 2.5,
                    occurrences: 2,
                    outward: true,
                },
            ],
        })
    );
    assert_eq!(
        model[7],
        PartDesignFeature::PolarPattern(PolarPattern {
            id: "a0000002".to_owned(),
            originals: Originals::Features(vec!["q0000002".to_owned()]),
            axis: element(
                "a0000001",
                "p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000002)"
            ),
            reversed: true,
            mode: "Spacing",
            angle: 30.5,
            occurrences: 4,
        })
    );
    assert_eq!(
        model[8],
        PartDesignFeature::Mirrored(Mirrored {
            id: "a0000003".to_owned(),
            originals: Originals::WholeShape,
            plane: Reference::Origin { role: "XZ_Plane" },
        })
    );
    // Each pattern case's body is its chain: the polar pattern's holds the linear one.
    assert_eq!(loaded.cases[4].model[..], model[..8]);
    // A pattern adds no curve of its own: copies of a hole's are a curved boolean, and
    // copies of a cube's are planar.
    assert_eq!(
        loaded.cases[3].tolerance,
        silico_oracle::Tolerance::CURVED_BOOLEAN
    );
    assert_eq!(
        case(&cases().cases, "linear_x").tolerance,
        silico_oracle::Tolerance::PLANAR
    );
    // FreeCAD is never asked to find a reference Silico's rebuild hasn't described.
    let e = features::freecad(&loaded.cases[3..4]).unwrap_err();
    assert!(
        e.contains("linear's reference p0000001.pad.Face(M s0000001.sketch.Face1) isn't described"),
        "{e}"
    );
}

#[test]
fn a_pattern_freecad_1_1_3_builds_otherwise_is_refused_naming_why() {
    // FreeCAD 1.1.3 has no instances to skip, and takes a polar extent within 1e-7 degrees of
    // 360 as a whole turn, which Silico doesn't (decision 5): the harness builds neither. A
    // skip past a count takes nothing out, and exactly 360 is a whole turn to both.
    let linear = |skip: &str| {
        format!(
            r#"{SEED}, {{"id": "a0000001", "type": "linear_pattern", "v": 1, "name": "linear", "base": "p0000002", "copies": {{"type": "features", "features": ["p0000002"]}}, "directions": [{{"along": "x", "reversed": false, "distribution": {{"type": "spacing", "length": 5}}, "count": 3}}], "skip": {skip}, "note": "Expected: volume 3."}}"#
        )
    };
    let e = load(&linear("[[1]]")).unwrap_err();
    assert!(
        e.contains("a0000001 skips instance [1], which FreeCAD 1.1.3's patterns can't"),
        "{e}"
    );
    assert!(load(&linear("[[3]]")).unwrap().is_some());
    let polar = |angle: &str| {
        format!(
            r#"{SEED}, {{"id": "a0000001", "type": "polar_pattern", "v": 1, "name": "polar", "base": "p0000002", "copies": {{"type": "features", "features": ["p0000002"]}}, "axis": "z", "reversed": false, "distribution": {{"type": "extent", "angle": {angle}}}, "count": 3, "skip": [], "note": "Expected: volume 3."}}"#
        )
    };
    let e = load(&polar("359.99999999")).unwrap_err();
    assert!(
        e.contains("a0000001 turns 359.99999999 degrees, which FreeCAD 1.1.3 takes as a whole turn and Silico doesn't"),
        "{e}"
    );
    // Exactly 360 is a whole turn to both, and 359.99 to neither.
    for angle in ["360", "359.99"] {
        assert!(load(&polar(angle)).unwrap().is_some(), "{angle}");
    }
}

#[test]
fn a_pattern_case_and_a_measure_case_find_a_reference_by_silicos_description_of_it() {
    // The bottom face of bottom_face's 40 x 20 block, as FreeCAD looks for it: a plane of
    // area 800, its centre (20, 10, 0), by hand. The same for a measure case's body that's a
    // pattern (silico-oracle's measures.rs).
    let cases = cases();
    let described = features::described(&cases).unwrap();
    let mut bottom = case(&described, "bottom_face");
    let PartDesignFeature::LinearPattern(p) = bottom.model.pop().unwrap() else {
        panic!()
    };
    let Reference::Element {
        element:
            Some(Element::Face {
                surface,
                area,
                centroid,
            }),
        ..
    } = &p.directions[0].reference
    else {
        panic!("{:?}", p.directions[0].reference)
    };
    assert_eq!(*surface, "plane");
    assert!((area - 800.0).abs() < 1e-9, "{area}");
    for (got, want) in centroid.iter().zip([20.0, 10.0, 0.0]) {
        assert!((got - want).abs() < 1e-9, "{centroid:?}");
    }
    let model = features::rebuilt(&cases).unwrap();
    let body = silico_oracle::measures::body(&model, &FeatureId::new("t0000009").unwrap()).unwrap();
    let last = body["part_design"]["features"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["kind"], "linear_pattern");
    let area = last["directions"][0]["reference"]["element"]["face"]["area"]
        .as_f64()
        .unwrap();
    assert!((area - 800.0).abs() < 1e-9, "{last}");
}
