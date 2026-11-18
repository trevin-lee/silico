//! M6.2: the linear pattern, polar pattern, and mirror in the document
//! (docs/design/m6.0-patterns.md), held to the design's own text: the plate of section 2.2
//! and each form of section 2.3 read and written back byte for byte; their outline lines,
//! section 2.5's; each gate refusal of section 4.1 made by its edit and equal to its example
//! byte for byte; and section 6's edits, a pattern a child of each original. Every example
//! is read from the design, not copied here, so the two can't drift apart.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use silico_core::command::wire::{Layout, ToWire, write as wire};
use silico_core::document::{Checked, Document, FeatureId, read, write};

use super::within::within;

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

/// The design's text between the headings `from` and `to`.
fn section(from: &str, to: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m6.0-patterns.md");
    let doc = std::fs::read_to_string(&path).unwrap();
    let start = doc.find(from).unwrap_or_else(|| panic!("no {from}"));
    let end = start + doc[start..].find(to).unwrap_or_else(|| panic!("no {to}"));
    doc[start..end].to_owned()
}

/// The code blocks of `kind` (`json`, `text`) in `section`, in order.
fn blocks(section: &str, kind: &str) -> Vec<String> {
    section
        .split(&format!("```{kind}\n"))
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap().to_owned())
        .collect()
}

/// The plate of section 2.2, as the design writes it.
fn plate_text() -> String {
    let plate = blocks(&section("### 2.2 The plate", "### 2.3 Other forms"), "json");
    format!("{}\n", plate[0])
}

fn plate() -> Document {
    read(plate_text().as_bytes()).unwrap()
}

/// `text` with `from` replaced by `to`, where `from` is written exactly once.
fn edited(text: &str, from: &str, to: &str) -> String {
    assert_eq!(text.matches(from).count(), 1, "{from}");
    text.replacen(from, to, 1)
}

/// What the gate refuses of `text`, each error as an example file writes it.
fn refused(text: &str) -> Vec<String> {
    let document = read(text.as_bytes()).unwrap();
    match Checked::new(document) {
        Ok(_) => Vec::new(),
        Err(invalid) => invalid
            .errors()
            .iter()
            .map(|error| wire(&error.to_wire(), Layout::Canonical).unwrap())
            .collect(),
    }
}

#[test]
fn the_plate_is_read_and_written_back_byte_for_byte_and_passes_the_gate() {
    let text = plate_text();
    let document = plate();
    assert_eq!(write(&document).unwrap(), text);
    let checked = Checked::new(document.clone()).unwrap();
    assert_eq!(checked.document(), &document);
    let types: Vec<&str> = document
        .features
        .iter()
        .map(|f| f.operation.type_name())
        .filter(|t| t.ends_with("pattern") || *t == "mirror")
        .collect();
    assert_eq!(types, ["mirror", "linear_pattern", "polar_pattern"]);
}

#[test]
fn each_other_form_is_read_and_written_back_byte_for_byte() {
    // Each feature of section 2.3, as `text` with `feature` gives it, read as a document's
    // feature and written back: a whole-solid mirror, a spaced grid by names with two
    // corners skipped, and spaced spokes about a rim with one skipped.
    let forms = blocks(&section("### 2.3 Other forms", "### 2.4"), "json");
    assert_eq!(forms.len(), 3);
    for form in forms {
        let indented: Vec<String> = form.lines().map(|line| format!("    {line}")).collect();
        let text = format!(
            "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 6,\n  \"features\": [\n{}\n  ]\n}}\n",
            indented.join("\n")
        );
        let document = read(text.as_bytes()).unwrap();
        assert_eq!(write(&document).unwrap(), text, "{form}");
        assert_eq!(
            silico_core::document::write_feature(&document.features[0]).unwrap(),
            form
        );
    }
}

#[test]
fn a_patterns_sets_are_read_in_any_order_and_written_sorted() {
    // copies.features and skip are sets (D5): read in any order, written sorted, by ID and
    // by section 3's order of instances; the document read either way is one document.
    let vents = blocks(&section("### 2.3 Other forms", "### 2.4"), "json")[0].clone();
    let shuffled = edited(
        &vents,
        "  \"skip\": [\n    [0, 2],\n    [5, 2]\n  ]",
        "  \"skip\": [[5, 2], [0, 2]]",
    );
    let shuffled = edited(
        &shuffled,
        "\"features\": [\"q0000004\"]",
        "\"features\": [\"q0000005\", \"q0000004\"]",
    );
    let wrap = |feature: &str| {
        format!(
            "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 6, \"features\": [{feature}]}}"
        )
    };
    let read_shuffled = read(wrap(&shuffled).as_bytes()).unwrap();
    let written = write(&read_shuffled).unwrap();
    assert!(
        written.contains("\"features\": [\"q0000004\", \"q0000005\"]"),
        "{written}"
    );
    assert!(
        written.contains("      \"skip\": [\n        [0, 2],\n        [5, 2]\n      ]"),
        "{written}"
    );
    // The canonical text reads as the same document, as two texts that write it are one.
    let again = read(written.as_bytes()).unwrap();
    assert!(again.canonically_eq(&read_shuffled));
    assert_ne!(again, read_shuffled);
}

#[test]
fn the_outline_writes_each_pattern_as_the_design_gives_it() {
    // Section 2.5's lines, for the plate's patterns and section 2.3's forms, each feature's
    // lines as `outline` writes them, in its order.
    let expected = blocks(&section("### 2.5 The outline", "## 3. Names"), "text")[0].clone();
    let mut document = plate();
    let forms = blocks(&section("### 2.3 Other forms", "### 2.4"), "json");
    for form in &forms {
        let wrapped = format!(
            "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 6, \"features\": [{form}]}}"
        );
        document
            .features
            .extend(read(wrapped.as_bytes()).unwrap().features);
    }
    let outline = silico_core::command::outline(&document);
    // Each feature's line and the lines under it.
    let mut lines: Vec<&str> = Vec::new();
    let mut keep = false;
    for line in outline.lines() {
        if !line.starts_with(' ') {
            keep = [
                "m0000001", "t0000001", "t0000002", "t0000003", "t0000004", "m0000002",
            ]
            .iter()
            .any(|id| line.starts_with(id));
        }
        if keep {
            lines.push(line);
        }
    }
    // The design lists the plate's in timeline order, then the forms'.
    let order = [
        "m0000001", "t0000001", "t0000002", "t0000003", "t0000004", "m0000002",
    ];
    let mut by_feature: Vec<(usize, Vec<&str>)> = Vec::new();
    for line in lines {
        if !line.starts_with(' ') {
            let at = order.iter().position(|id| line.starts_with(id)).unwrap();
            by_feature.push((at, vec![line]));
        } else if let Some((_, block)) = by_feature.last_mut() {
            block.push(line);
        }
    }
    by_feature.sort_by_key(|(at, _)| *at);
    let got: Vec<String> = by_feature
        .into_iter()
        .flat_map(|(_, block)| block.into_iter().map(str::to_owned))
        .collect();
    assert_eq!(got.join("\n"), expected);
}

/// Section 4.1's examples, in order.
fn gate_examples() -> Vec<String> {
    blocks(&section("### 4.1 The gate", "### 4.2 The rebuild"), "json")
}

/// The plate's linear pattern's directions as the file writes them, and their parts.
const DIRECTION_0: &str =
    "\"distribution\": {\"type\": \"extent\", \"length\": 80},\n          \"count\": 3";
const DIRECTION_1: &str =
    "\"distribution\": {\"type\": \"extent\", \"length\": 40},\n          \"count\": 2";
const POLAR: &str = "\"distribution\": {\"type\": \"extent\", \"angle\": 360},\n      \"count\": 4";
const GRID_SKIP: &str = "\"skip\": []\n    },\n    {\n      \"id\": \"s0000004\"";

fn grid_skip(skip: &str) -> String {
    edited(
        &plate_text(),
        GRID_SKIP,
        &format!("\"skip\": {skip}\n    }},\n    {{\n      \"id\": \"s0000004\""),
    )
}

#[test]
fn each_gate_refusal_on_the_plate_is_its_example_byte_for_byte() {
    let examples = gate_examples();
    let text = plate_text();
    let plate = &text;
    let on_plate: Vec<(usize, String)> = vec![
        // A count below 1.
        (
            0,
            edited(
                plate,
                DIRECTION_0,
                &DIRECTION_0.replace("\"count\": 3", "\"count\": 0"),
            ),
        ),
        // A 26 by 40 grid.
        (
            1,
            edited(
                &edited(
                    plate,
                    DIRECTION_0,
                    &DIRECTION_0.replace("\"count\": 3", "\"count\": 26"),
                ),
                DIRECTION_1,
                &DIRECTION_1.replace("\"count\": 2", "\"count\": 40"),
            ),
        ),
        // A length of 0.
        (
            2,
            edited(
                plate,
                DIRECTION_0,
                &DIRECTION_0.replace("\"length\": 80", "\"length\": 0"),
            ),
        ),
        // 1,000 instances 1002 mm apart (direction 1 down to one instance, so the product
        // stays within the cap).
        (
            3,
            edited(
                &edited(
                    plate,
                    DIRECTION_0,
                    "\"distribution\": {\"type\": \"spacing\", \"length\": 1002},\n          \"count\": 1000",
                ),
                DIRECTION_1,
                &DIRECTION_1.replace("\"count\": 2", "\"count\": 1"),
            ),
        ),
        // 21 instances over a nanometre.
        (
            4,
            edited(
                plate,
                DIRECTION_0,
                "\"distribution\": {\"type\": \"extent\", \"length\": 0.000001},\n          \"count\": 21",
            ),
        ),
        // A polar extent over 360°, and −360°.
        (5, edited(plate, POLAR, &POLAR.replace("360", "400"))),
        (6, edited(plate, POLAR, &POLAR.replace("360", "-360"))),
        // 6 instances 90° apart.
        (
            7,
            edited(
                plate,
                POLAR,
                "\"distribution\": {\"type\": \"spacing\", \"angle\": 90},\n      \"count\": 6",
            ),
        ),
        // An empty copies.features, and one listed twice.
        (
            8,
            edited(
                plate,
                "\"copies\": {\"type\": \"features\", \"features\": [\"q0000002\"]},\n      \"directions\"",
                "\"copies\": {\"type\": \"features\", \"features\": []},\n      \"directions\"",
            ),
        ),
        (
            9,
            edited(
                plate,
                "\"copies\": {\"type\": \"features\", \"features\": [\"q0000002\"]},\n      \"directions\"",
                "\"copies\": {\"type\": \"features\", \"features\": [\"q0000002\", \"q0000002\"]},\n      \"directions\"",
            ),
        ),
        // The skip refusals on the 3 by 2 grid.
        (13, grid_skip("[[0]]")),
        (14, grid_skip("[[1, 1, 1]]")),
        (15, grid_skip("[[1], [1]]")),
        (16, grid_skip("[[2, 0]]")),
        // A revolve's x_axis, an origin plane, and a vertex as a direction.
        (
            17,
            edited(plate, "\"along\": \"x\",", "\"along\": \"x_axis\","),
        ),
        (18, edited(plate, "\"along\": \"x\",", "\"along\": \"xy\",")),
        (
            19,
            edited(
                plate,
                "\"along\": \"x\",",
                "\"along\": \"p0000001.pad.Vertex(M s0000001.sketch.Vertex.pa000001)\",",
            ),
        ),
        // A face as a polar axis, and an origin axis as a mirror plane.
        (
            20,
            edited(
                plate,
                "\"axis\": \"z\",",
                "\"axis\": \"p0000001.pad.Face(G s0000001.sketch.Face1)\",",
            ),
        ),
        (21, edited(plate, "\"plane\": \"yz\"", "\"plane\": \"z\"")),
    ];
    let mut checked = BTreeSet::new();
    for (example, text) in on_plate {
        assert_eq!(
            refused(&text),
            [examples[example].clone()],
            "example {example}"
        );
        checked.insert(example);
    }
    // The fillet as an original: a plate padded, its edge filleted, the fillet copied.
    let plate_doc = self::plate();
    let first_two: Vec<String> = plate_doc.features[..2]
        .iter()
        .map(|f| silico_core::document::write_feature(f).unwrap())
        .collect();
    let small = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 6, \"features\": [{}, {}, {}, {}]}}",
        first_two[0],
        first_two[1],
        r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "Rim", "base": "p0000001", "edges": ["p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000001)"], "radius": 1}"#,
        r#"{"id": "t0000001", "type": "linear_pattern", "v": 1, "name": "Copies", "base": "f0000001", "copies": {"type": "features", "features": ["f0000001"]}, "directions": [{"along": "x", "reversed": false, "distribution": {"type": "extent", "length": 80}, "count": 3}], "skip": []}"#
    );
    assert_eq!(refused(&small), [examples[10].clone()]);
    checked.insert(10);
    // An original off the chain: a second body, p0000010, the pattern's base.
    let second = r#"{"id": "s0000010", "type": "sketch", "v": 1, "name": "Second outline", "plane": "xy", "points": [{"id": "pa000001", "x": 200, "y": 0, "construction": false}, {"id": "pa000002", "x": 210, "y": 0, "construction": false}, {"id": "pa000003", "x": 210, "y": 10, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": []},
    {"id": "p0000010", "type": "pad", "v": 1, "name": "Second body", "sketch": "s0000010", "extent": {"type": "one_side", "length": 5, "reversed": false}},
    {
      "id": "t0000001","#;
    let off_chain = edited(
        &edited(plate, "{\n      \"id\": \"t0000001\",", second),
        "\"base\": \"q0000002\",\n      \"copies\"",
        "\"base\": \"p0000010\",\n      \"copies\"",
    );
    assert_eq!(refused(&off_chain), [examples[11].clone()]);
    checked.insert(11);
    // An original after the pattern: the grid put before the corner hole, on the mirror,
    // and the hole on it, as add_feature with `before` naming the hole puts it.
    let mut document = self::plate();
    let at = |d: &Document, f: &str| d.features.iter().position(|x| x.id == id(f)).unwrap();
    let grid = document.features.remove(at(&document, "t0000001"));
    let hole = at(&document, "q0000002");
    document.features.insert(hole, grid);
    let set_base = |d: &mut Document, f: &str, to: &str| {
        let at = d.features.iter().position(|x| x.id == id(f)).unwrap();
        match &mut d.features[at].operation {
            silico_core::document::Operation::LinearPattern { base, .. }
            | silico_core::document::Operation::Pocket { base, .. } => *base = id(to),
            silico_core::document::Operation::Pad { base, .. } => *base = Some(id(to)),
            other => panic!("{other:?}"),
        }
    };
    set_base(&mut document, "t0000001", "m0000001");
    set_base(&mut document, "q0000002", "t0000001");
    set_base(&mut document, "p0000002", "q0000002");
    assert_eq!(refused(&write(&document).unwrap()), [examples[12].clone()]);
    checked.insert(12);
    // Every example of section 4.1 is made.
    assert_eq!(checked, (0..examples.len()).collect::<BTreeSet<_>>());
}

#[test]
fn a_whole_turns_instance_is_named_as_the_design_works_it_by_hand() {
    // From whole nanodegrees: 6 × 90° and 5 × 90° name instance 4, which lands on the
    // original; 4 × 90° is taken; 7 × 60° names instance 6; 5 × 100° names instance 4, at
    // 400°, which passes it.
    let polar = |count: u32, angle: u32| {
        edited(
            &plate_text(),
            POLAR,
            &format!(
                "\"distribution\": {{\"type\": \"spacing\", \"angle\": {angle}}},\n      \"count\": {count}"
            ),
        )
    };
    let named = |text: &str| -> Option<(String, String)> {
        let document = read(text.as_bytes()).unwrap();
        let invalid = Checked::new(document).err()?;
        let error = invalid.errors().first()?.clone();
        Some((error.message().to_owned(), error.elements().join(",")))
    };
    for (count, angle, instance, at, how) in [
        (6, 90, "4", "360", "lands on"),
        (5, 90, "4", "360", "lands on"),
        (7, 60, "6", "360", "lands on"),
        (5, 100, "4", "400", "passes"),
    ] {
        let (message, elements) = named(&polar(count, angle)).unwrap();
        assert!(
            message.ends_with(&format!(
                "instance {instance}, at {at} degrees, {how} the original"
            )),
            "{count} × {angle}: {message}"
        );
        assert_eq!(elements, instance);
    }
    assert_eq!(named(&polar(4, 90)), None);
    assert_eq!(named(&polar(3, 120)), None);
}

#[test]
fn a_counts_and_skips_rules_hold_at_their_edges() {
    // 25 by 40 is 1,000, taken; 1,001 in one direction is named at its count; a skip of the
    // last instance of each direction is taken, of one past it refused; [[3], [2, 1]] is
    // read and written [[2, 1], [3]] (on a 4 by 2 grid).
    let both = |a: u32, b: u32| {
        edited(
            &edited(
                &plate_text(),
                DIRECTION_0,
                &DIRECTION_0.replace("\"count\": 3", &format!("\"count\": {a}")),
            ),
            DIRECTION_1,
            &DIRECTION_1.replace("\"count\": 2", &format!("\"count\": {b}")),
        )
    };
    assert!(refused(&both(25, 40)).is_empty());
    let one = refused(&both(1001, 1));
    assert_eq!(one.len(), 1, "{one:?}");
    assert!(
        one[0].contains("\"parameter\": \"directions.0.count\""),
        "{}",
        one[0]
    );
    assert!(refused(&grid_skip("[[2, 1]]")).is_empty());
    // Past a count, kept (Main, 2026-11-17, following FreeCAD main's 0ea40a10e2).
    assert!(refused(&grid_skip("[[2, 2]]")).is_empty());
    assert!(refused(&grid_skip("[[3]]")).is_empty());
    let four = edited(
        &grid_skip("[[3], [2, 1]]"),
        DIRECTION_0,
        &DIRECTION_0.replace("\"count\": 3", "\"count\": 4"),
    );
    let checked = Checked::new(read(four.as_bytes()).unwrap()).unwrap();
    let written = write(checked.document()).unwrap();
    assert!(
        written.contains("\"skip\": [\n        [2, 1],\n        [3]\n      ]"),
        "{written}"
    );
    // Held sorted once the gate takes it, so skip.0 is [2, 1] in the model as in the text.
    let grid = checked.feature(&id("t0000001")).unwrap();
    let skip: Vec<Vec<u32>> = grid.operation.skip().iter().map(|s| s.0.clone()).collect();
    assert_eq!(skip, [vec![2, 1], vec![3]]);
}

#[test]
fn a_pattern_is_a_child_of_each_original_in_the_designs_edits() {
    // Section 6, on the plate.
    let checked = Checked::new(plate()).unwrap();
    let section6 = section("## 6. Timeline edits", "## 7. The dialogs");
    let examples = blocks(&section6, "json");
    let as_example =
        |error: &silico_core::Error| wire(&error.to_wire(), Layout::Canonical).unwrap();
    // A move of the pattern before its original, and of the original after the pattern.
    let error = silico_core::document::chain::move_feature(
        &checked,
        &id("t0000001"),
        Some(&id("q0000002")),
    )
    .unwrap_err();
    assert_eq!(as_example(&error), examples[1]);
    let error = silico_core::document::chain::move_feature(
        &checked,
        &id("q0000002"),
        Some(&id("s0000004")),
    )
    .unwrap_err();
    assert_eq!(as_example(&error), examples[3]);
    // Removing the original: the pattern's refusal, and the chamfer's, which names both.
    let refusals =
        silico_core::document::chain::removal_refusals(&checked, &id("q0000002"), usize::MAX);
    assert_eq!(refusals.total, 2);
    let refusals = refusals.errors;
    let written: Vec<String> = refusals.iter().map(as_example).collect();
    assert_eq!(written.len(), 2, "{written:?}");
    assert_eq!(written[0], examples[5]);
    assert!(
        written[1].contains("c0000001 names q0000002's elements, so q0000002 can't be removed"),
        "{}",
        written[1]
    );
    // Suppressing the original suppresses its pattern and the chamfer; unsuppressing the
    // pattern brings back its original, and the chamfer stays.
    let suppressed =
        silico_core::document::suppression::suppress(&checked, &id("q0000002")).unwrap();
    let ids = |list: &[FeatureId]| {
        list.iter()
            .map(|f| f.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&suppressed.suppressed),
        ["q0000002", "t0000001", "c0000001"]
    );
    assert!(suppressed.unsuppressed.is_empty());
    let after = Checked::new(suppressed.document).unwrap();
    let back = silico_core::document::suppression::unsuppress(&after, &id("t0000001")).unwrap();
    assert_eq!(ids(&back.unsuppressed), ["q0000002", "t0000001"]);
    assert!(back.suppressed.is_empty());
    let c = back
        .document
        .features
        .iter()
        .find(|f| f.id == id("c0000001"))
        .unwrap();
    assert!(c.suppressed);
    // The gate refuses the pattern unsuppressed under a suppressed original, naming both.
    let mut hand = plate();
    for feature in &mut hand.features {
        if feature.id == id("q0000002") {
            feature.suppressed = true;
        }
    }
    let invalid = Checked::new(hand).unwrap_err();
    let first = &invalid.errors()[0];
    assert_eq!(first.feature(), Some("t0000001"));
    assert_eq!(first.parameter(), Some("copies.features.0"));
    assert_eq!(
        first.message(),
        "t0000001 isn't suppressed, but it copies q0000002, which is suppressed: a feature that depends on a suppressed one is suppressed with it"
    );
    // The timeline lists a pattern's base, then its originals in timeline order, once each.
    let timeline = silico_core::timeline::timeline(
        checked.document(),
        None,
        &silico_core::timeline::Excepted::default(),
    );
    let grid = timeline
        .features
        .iter()
        .find(|e| e.id == id("t0000001"))
        .unwrap();
    assert_eq!(ids(&grid.inputs), ["q0000002"]);
    let mirror = timeline
        .features
        .iter()
        .find(|e| e.id == id("m0000001"))
        .unwrap();
    assert_eq!(ids(&mirror.inputs), ["q0000001"]);
}

#[test]
fn each_patterns_rebuild_is_refused_naming_it_until_m6_3_builds_them() {
    // M6.2 reads and checks the three types; M6.3 builds them. Until then each fails at
    // the rebuild, naming itself, and what's built on it waits on it.
    let engine =
        silico_core::recompute::Recompute::new(silico_core::fake::FakeKernel::new(2).unwrap())
            .with_sketches(silico_core::fake::FakeSketches::new());
    let plate = plate();
    let features: Vec<String> = plate.features[..4]
        .iter()
        .map(|f| silico_core::document::write_feature(f).unwrap())
        .collect();
    for (pattern, word) in [
        (
            r#"{"id": "m0000001", "type": "mirror", "v": 1, "name": "Other notch", "base": "q0000001", "copies": {"type": "features", "features": ["q0000001"]}, "plane": "yz"}"#,
            "mirror",
        ),
        (
            r#"{"id": "m0000001", "type": "linear_pattern", "v": 1, "name": "Notches", "base": "q0000001", "copies": {"type": "solid"}, "directions": [{"along": "x", "reversed": true, "distribution": {"type": "spacing", "length": 10}, "count": 2}], "skip": []}"#,
            "linear_pattern",
        ),
        (
            r#"{"id": "m0000001", "type": "polar_pattern", "v": 1, "name": "Notches", "base": "q0000001", "copies": {"type": "features", "features": ["q0000001"]}, "axis": "z", "reversed": false, "distribution": {"type": "extent", "angle": 360}, "count": 4, "skip": []}"#,
            "polar_pattern",
        ),
    ] {
        let text = format!(
            "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 6, \"features\": [{}, {pattern}, {}]}}",
            features.join(", "),
            r#"{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Rim", "base": "m0000001", "edges": ["p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000001)"], "size": {"distance": 0.5}}"#
        );
        let checked = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
        let Ok(silico_core::recompute::RunOutcome::Published(model)) = engine.start(checked).wait()
        else {
            panic!("not published");
        };
        let errors: Vec<&silico_core::Error> = model.errors().collect();
        assert_eq!(errors.len(), 1, "{word}: {errors:?}");
        let error = errors[0];
        assert_eq!(error.code(), silico_core::ErrorCode::InvalidInput);
        assert_eq!(error.feature(), Some("m0000001"));
        assert_eq!(
            error.message(),
            format!(
                "{word} m0000001 isn't built yet: this Silico reads and checks a {word}, and builds none"
            )
        );
        // The pocket under it built; the chamfer on it waits on it.
        assert!(matches!(
            model.outcome(&id("q0000001")),
            Some(silico_core::recompute::FeatureOutcome::Built { .. })
        ));
        assert!(matches!(
            model.outcome(&id("c0000001")),
            Some(silico_core::recompute::FeatureOutcome::Blocked { .. })
        ));
    }
}

/// What a command answered, within a minute.
fn answered<T>(running: silico_core::command::Running<T>) -> silico_core::command::Response<T> {
    running
        .wait_timeout(std::time::Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("no answer"))
}

#[test]
fn removing_an_original_through_the_command_lists_every_feature_that_cant_do_without_it() {
    // `remove_feature` gives the pattern's refusal and the chamfer's at once
    // (docs/design/m6.0-patterns.md, section 6), the pattern's as the design writes it.
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let sketches = silico_core::fake::FakeSketches::new();
    let sessions = silico_core::command::Sessions::with_stand_ins(
        std::sync::Arc::new(silico_core::fake::FakeKernel::new(2).unwrap()),
        move |cache| silico_core::cache::Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn silico_core::sketch::SketchSolver>),
    );
    let path = dir.path().join("plate.silico");
    std::fs::write(&path, plate_text()).unwrap();
    let session = answered(sessions.open(&path, false)).session.unwrap();
    let response = answered(sessions.remove_feature(&session.id, session.revision, id("q0000002")));
    let problems = response.result.unwrap_err();
    let written: Vec<String> = problems
        .errors()
        .iter()
        .map(|error| wire(&error.to_wire(), Layout::Canonical).unwrap())
        .collect();
    let section6 = section("## 6. Timeline edits", "## 7. The dialogs");
    let examples = blocks(&section6, "json");
    assert_eq!(written.len(), 2, "{written:?}");
    assert_eq!(written[0], examples[5]);
    assert!(
        written[1].contains(
            "\"message\": \"c0000001 names q0000002's elements, so q0000002 can't be removed\""
        ),
        "{}",
        written[1]
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), plate_text());
}

#[test]
fn a_patterns_originals_are_listed_in_timeline_order_not_their_ids() {
    // Two pockets, z0000001 before a0000002, copied by a pattern on a chamfer after both:
    // the timeline lists the chamfer, then z0000001, then a0000002, where the file writes
    // the originals a0000002 first.
    let plate = self::plate();
    let mut features: Vec<String> = plate.features[..2]
        .iter()
        .map(|f| silico_core::document::write_feature(f).unwrap())
        .collect();
    let hole = |hole: &str, base: &str| {
        format!(
            r#"{{"id": "{hole}", "type": "pocket", "v": 1, "name": "Hole {hole}", "base": "{base}", "sketch": "s0000001", "extent": {{"type": "one_side", "length": 1, "reversed": true}}}}"#
        )
    };
    features.push(hole("z0000001", "p0000001"));
    features.push(hole("a0000002", "z0000001"));
    features.push(r#"{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Rim", "base": "a0000002", "edges": ["p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000001)"], "size": {"distance": 0.5}}"#.to_owned());
    features.push(r#"{"id": "t0000001", "type": "linear_pattern", "v": 1, "name": "Holes", "base": "c0000001", "copies": {"type": "features", "features": ["a0000002", "z0000001"]}, "directions": [{"along": "x", "reversed": false, "distribution": {"type": "extent", "length": 80}, "count": 3}], "skip": []}"#.to_owned());
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 6, \"features\": [{}]}}",
        features.join(", ")
    );
    let document = read(text.as_bytes()).unwrap();
    let timeline = silico_core::timeline::timeline(
        &document,
        None,
        &silico_core::timeline::Excepted::default(),
    );
    let grid = timeline
        .features
        .iter()
        .find(|e| e.id == id("t0000001"))
        .unwrap();
    let inputs: Vec<&str> = grid.inputs.iter().map(FeatureId::as_str).collect();
    assert_eq!(inputs, ["c0000001", "z0000001", "a0000002"]);
}

#[test]
fn a_step_of_exactly_occts_tolerance_is_refused_and_one_past_it_taken() {
    // A nanometre over 10 steps is 1e-7 mm a step, OCCT's own tolerance, where a copy lies
    // on the one before it: refused. Over 9 steps it's 1.1e-7 mm: taken.
    let over = |count: u32| {
        edited(
            &plate_text(),
            DIRECTION_0,
            &format!(
                "\"distribution\": {{\"type\": \"extent\", \"length\": 0.000001}},\n          \"count\": {count}"
            ),
        )
    };
    let at = refused(&over(11));
    assert_eq!(at.len(), 1, "{at:?}");
    assert!(
        at[0].contains("puts its 11 instances 1e-7 mm apart (0.000001 mm in 10 steps)"),
        "{}",
        at[0]
    );
    assert!(refused(&over(10)).is_empty());
}

/// The plate with its grid set to FreeCAD's suppression pattern (`TestLinearPattern.py`'s
/// `makeSuppressionPattern`): spacing 10 each way, `counts` instances, `skip` as given.
fn suppression_grid(counts: (u32, u32), skip: &str) -> String {
    edited(
        &edited(
            &grid_skip(skip),
            DIRECTION_0,
            &format!(
                "\"distribution\": {{\"type\": \"spacing\", \"length\": 10}},\n          \"count\": {}",
                counts.0
            ),
        ),
        DIRECTION_1,
        &format!(
            "\"distribution\": {{\"type\": \"spacing\", \"length\": 10}},\n          \"count\": {}",
            counts.1
        ),
    )
}

/// The grid's `skip` as the gate holds it, and those in range.
fn held_skip(text: &str) -> (Vec<Vec<u32>>, Vec<Vec<u32>>) {
    let checked = Checked::new(read(text.as_bytes()).unwrap())
        .unwrap_or_else(|invalid| panic!("{:?}", invalid.errors()));
    let grid = checked.feature(&id("t0000001")).unwrap();
    let silico_core::document::Operation::LinearPattern {
        directions, skip, ..
    } = &grid.operation
    else {
        panic!("not a linear pattern");
    };
    let counts: Vec<u32> = directions.iter().map(|d| d.count).collect();
    let in_range = silico_core::document::skipped_in_range(skip, &counts)
        .into_iter()
        .map(|s| s.0.clone())
        .collect();
    (skip.iter().map(|s| s.0.clone()).collect(), in_range)
}

#[test]
fn a_skip_keeps_its_grid_position_through_counts_lowered_and_raised_as_freecad_mains() {
    // TestLinearPattern.testSuppressionKeepsGridPosition (FreeCAD main, 0ea40a10e2): a 3 × 3
    // grid with (1, 2) suppressed (its flat index 5). Direction 2 raised to 4: still (1, 2).
    // Counts lowered to 1 and 2: kept, and in range no more. Back to 3 and 3: in range
    // again. FreeCAD's volumes (11000, 2000, 8000) are M6.3's to rebuild.
    let skipped = vec![vec![1, 2]];
    assert_eq!(
        held_skip(&suppression_grid((3, 3), "[[1, 2]]")),
        (skipped.clone(), skipped.clone())
    );
    assert_eq!(
        held_skip(&suppression_grid((3, 4), "[[1, 2]]")),
        (skipped.clone(), skipped.clone())
    );
    assert_eq!(
        held_skip(&suppression_grid((1, 2), "[[1, 2]]")),
        (skipped.clone(), Vec::new())
    );
    assert_eq!(
        held_skip(&suppression_grid((3, 3), "[[1, 2]]")),
        (skipped.clone(), skipped)
    );
}

#[test]
fn a_skip_past_a_count_is_saved_and_read_back_as_freecad_mains_is() {
    // TestLinearPattern.testSuppressionPersistence: (1, 2) skipped, direction 2's count 1
    // ("save an out-of-range suppression"), saved and opened: the skip is there, and with
    // direction 2's count 4 it's in range, (1, 2).
    let out = suppression_grid((3, 1), "[[1, 2]]");
    let saved = write(
        &Checked::new(read(out.as_bytes()).unwrap())
            .unwrap()
            .into_document(),
    )
    .unwrap();
    assert!(
        saved.contains("\"skip\": [\n        [1, 2]\n      ]"),
        "{saved}"
    );
    let opened = edited(
        &saved,
        "\"distribution\": {\"type\": \"spacing\", \"length\": 10},\n          \"count\": 1",
        "\"distribution\": {\"type\": \"spacing\", \"length\": 10},\n          \"count\": 4",
    );
    assert_eq!(held_skip(&opened), (vec![vec![1, 2]], vec![vec![1, 2]]));
}

#[test]
fn only_the_skips_inside_the_grid_take_effect_as_freecad_mains_project() {
    // TestLinearPattern.testSuppressedIndicesOnlyProjectPositionsInsideTheGrid: a 3 × 2
    // grid with (0, 1), (2, 1), (3, 0), and (0, 2) skipped (FreeCAD's (-1, 0) has no
    // spelling: Silico has no negative offsets) keeps all four, and takes (0, 1) and
    // (2, 1), FreeCAD's flat [1, 5]. (3, 0) is written [3].
    let (held, in_range) = held_skip(&suppression_grid((3, 2), "[[0, 1], [2, 1], [3], [0, 2]]"));
    assert_eq!(held, [vec![0, 1], vec![0, 2], vec![2, 1], vec![3]]);
    assert_eq!(in_range, [vec![0, 1], vec![2, 1]]);
}

#[test]
fn a_patterns_misspelt_forms_and_stray_members_are_refused_by_the_reader() {
    // Each of the readers' refusals, at its path: a form that isn't one, a member a part
    // can't have, a count or an offset that isn't a whole number or is past 32 bits, and an
    // empty reference.
    let plate = plate_text();
    let copies =
        "\"copies\": {\"type\": \"features\", \"features\": [\"q0000002\"]},\n      \"directions\"";
    let cases: Vec<(&str, String, &str)> = vec![
        (
            DIRECTION_0,
            DIRECTION_0.replace("\"extent\"", "\"extents\""),
            "directions.0.distribution.type",
        ),
        (
            DIRECTION_0,
            DIRECTION_0.replace("\"length\": 80}", "\"length\": 80, \"angle\": 5}"),
            "directions.0.distribution.angle",
        ),
        (
            DIRECTION_0,
            DIRECTION_0.replace("\"count\": 3", "\"count\": 3, \"skip\": 1"),
            "directions.0.skip",
        ),
        (
            copies,
            copies.replace("\"features\", \"features\": [\"q0000002\"]", "\"solids\""),
            "copies.type",
        ),
        (
            copies,
            copies.replace(
                "\"features\": [\"q0000002\"]",
                "\"features\": [\"q0000002\"], \"count\": 1",
            ),
            "copies.count",
        ),
        (
            POLAR,
            POLAR.replace("\"extent\"", "\"spaced\""),
            "distribution.type",
        ),
        (
            DIRECTION_0,
            DIRECTION_0.replace("\"count\": 3", "\"count\": 3.5"),
            "directions.0.count",
        ),
        (
            DIRECTION_0,
            DIRECTION_0.replace("\"count\": 3", "\"count\": -3"),
            "directions.0.count",
        ),
        (
            DIRECTION_0,
            DIRECTION_0.replace("\"count\": 3", "\"count\": 4294967296"),
            "directions.0.count",
        ),
        (
            POLAR,
            POLAR.replace("\"count\": 4", "\"count\": 4294967296"),
            "count",
        ),
        (
            "\"along\": \"x\",",
            "\"along\": \"\",".to_owned(),
            "directions.0.along",
        ),
        ("\"plane\": \"yz\"", "\"plane\": 3".to_owned(), "plane"),
    ];
    for (from, to, parameter) in cases {
        let error = read(edited(&plate, from, &to).as_bytes()).unwrap_err();
        assert_eq!(error.code(), silico_core::ErrorCode::InvalidInput, "{to}");
        assert_eq!(error.parameter(), Some(parameter), "{to}: {error}");
    }
    for (skip, parameter) in [
        ("[[1.5]]", "skip.0.0"),
        ("[[1, 4294967296]]", "skip.0.1"),
        ("[1]", "skip.0"),
    ] {
        let error = read(grid_skip(skip).as_bytes()).unwrap_err();
        assert_eq!(error.parameter(), Some(parameter), "{skip}: {error}");
    }
}

#[test]
fn a_polar_patterns_count_and_angle_are_refused_at_their_edges() {
    // Count 0 and 1,001; an angle of 0, with one instance and with four; and OCCT's angular
    // tolerance, by hand: a nanodegree over n steps turns 1e-9 × π/180 / n radians, at or
    // under 1e-12 from n = 18 (9.696e-13), over it at n = 17 (1.027e-12).
    let polar = |distribution: &str, count: u32| {
        edited(
            &plate_text(),
            POLAR,
            &format!("\"distribution\": {distribution},\n      \"count\": {count}"),
        )
    };
    let whole = "{\"type\": \"extent\", \"angle\": 360}";
    let zero = refused(&polar(whole, 0));
    assert_eq!(zero.len(), 1, "{zero:?}");
    assert!(zero[0].contains("\"parameter\": \"count\""), "{}", zero[0]);
    assert!(
        zero[0].contains("polar_pattern count must be at least 1, got 0"),
        "{}",
        zero[0]
    );
    let over = refused(&polar(whole, 1001));
    assert_eq!(over.len(), 1, "{over:?}");
    assert!(over[0].contains("\"parameter\": \"count\""), "{}", over[0]);
    assert!(
        over[0].contains("has 1001 instances in all, more than"),
        "{}",
        over[0]
    );
    for count in [1, 4] {
        let error = refused(&polar("{\"type\": \"spacing\", \"angle\": 0}", count));
        assert_eq!(error.len(), 1, "{error:?}");
        assert!(error[0].contains("got 0°"), "{}", error[0]);
        assert!(
            error[0].contains("give the polar_pattern an angle above 0°"),
            "{}",
            error[0]
        );
    }
    let tiny = "{\"type\": \"extent\", \"angle\": 0.000000001}";
    let at = refused(&polar(tiny, 19));
    assert_eq!(at.len(), 1, "{at:?}");
    assert!(at[0].contains("within OCCT's 1e-12 radians"), "{}", at[0]);
    assert!(refused(&polar(tiny, 18)).is_empty());
    // A polar skip of two offsets is refused; one past the count is kept.
    let skip = |skip: &str| {
        edited(
            &plate_text(),
            "\"count\": 4,\n      \"skip\": []",
            &format!("\"count\": 4,\n      \"skip\": {skip}"),
        )
    };
    let two = refused(&skip("[[1, 1]]"));
    assert_eq!(two.len(), 1, "{two:?}");
    assert!(
        two[0].contains("has 2 offsets, and polar_pattern t0000002 has 1 direction"),
        "{}",
        two[0]
    );
    assert!(refused(&skip("[[4]]")).is_empty());
}

#[test]
fn a_linear_patterns_reach_directions_and_originals_hold_at_their_edges() {
    // Five instances 250000 mm apart reach exactly a kilometre (taken); a nanometre more each
    // passes it. A length of exactly a kilometre is taken. No direction, and three, are
    // refused. And the originals' other refusals.
    let five = |length: &str| {
        edited(
            &plate_text(),
            DIRECTION_0,
            &format!(
                "\"distribution\": {{\"type\": \"spacing\", \"length\": {length}}},\n          \"count\": 5"
            ),
        )
    };
    assert!(refused(&five("250000")).is_empty());
    let past = refused(&five("250000.000001"));
    assert_eq!(past.len(), 1, "{past:?}");
    assert!(past[0].contains("reaches 1000000.000004 mm"), "{}", past[0]);
    let kilometre = edited(&plate_text(), "\"length\": 80}", "\"length\": 1000000}");
    assert!(refused(&kilometre).is_empty());
    let copies = "\"features\": [\"q0000002\"]},\n      \"directions\"";
    for (to, says, suggestion) in [
        (
            "\"features\": [\"zzzzzzzz\"]},\n      \"directions\"",
            "copies.features.0 zzzzzzzz isn't the ID of any feature",
            "use the ID of a feature before this one",
        ),
        (
            "\"features\": [\"t0000001\"]},\n      \"directions\"",
            "copies.features.0 t0000001 is this feature: a feature can't copy itself",
            "use the ID of a feature before this one",
        ),
        (
            "\"features\": [\"s0000003\"]},\n      \"directions\"",
            "copies.features.0 s0000003 is Corner hole outline, a sketch, not a pad, pocket, revolve, or groove",
            "copy the pads, pockets, revolves, or grooves it's built from",
        ),
    ] {
        let errors = refused(&edited(&plate_text(), copies, to));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains(says), "{}", errors[0]);
        assert!(errors[0].contains(suggestion), "{}", errors[0]);
    }
}

#[test]
fn a_linear_pattern_of_no_direction_or_three_is_refused_at_directions() {
    let at = plate_text();
    let start = at.find("\"directions\": [\n").unwrap();
    let end = at
        .find("\n      \"skip\": []\n    },\n    {\n      \"id\": \"s0000004\"")
        .unwrap();
    let directions = &at[start..end];
    let none = at.replacen(directions, "\"directions\": [],", 1);
    let errors = refused(&none);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("\"parameter\": \"directions\""),
        "{}",
        errors[0]
    );
    assert!(
        errors[0].contains("linear_pattern t0000001 has 0 directions"),
        "{}",
        errors[0]
    );
    let third = directions.replacen(
        "\n      ],",
        ",\n        {\"along\": \"z\", \"reversed\": false, \"distribution\": {\"type\": \"extent\", \"length\": 10}, \"count\": 1}\n      ],",
        1,
    );
    let three = at.replacen(directions, &third, 1);
    let errors = refused(&three);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("linear_pattern t0000001 has 3 directions"),
        "{}",
        errors[0]
    );
}

#[test]
fn three_directions_of_huge_counts_are_refused_not_a_panic() {
    // 2^22 each way three times is 2^66, past u64: the product is past the cap, never a
    // panic or a wrapped 0 (the review's case).
    let at = plate_text();
    let big = 4_194_304;
    let two = edited(
        &edited(
            &at,
            DIRECTION_0,
            &DIRECTION_0.replace("\"count\": 3", &format!("\"count\": {big}")),
        ),
        DIRECTION_1,
        &DIRECTION_1.replace("\"count\": 2", &format!("\"count\": {big}")),
    );
    let third = two.replacen(
        "\n      ],\n      \"skip\"",
        &format!(",\n        {{\"along\": \"z\", \"reversed\": false, \"distribution\": {{\"type\": \"spacing\", \"length\": 0.001}}, \"count\": {big}}}\n      ],\n      \"skip\""),
        1,
    );
    let errors = refused(&third);
    assert!(
        errors.iter().any(|e| e.contains("has 3 directions")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("has more than 18446744073709551615 instances in all")),
        "{errors:?}"
    );
}

#[test]
fn a_long_skip_copies_list_or_chain_is_checked_promptly() {
    // The review's hostile files: 600,000 skips, each past nothing and none repeated;
    // 1,000,000 originals no feature has; and 60,000 pockets in one chain, each an original.
    // Each repeat found by a look-up, each refusal made only if it's listed, and each
    // original's chain read from the roots worked once, so each checks in well under the
    // CPU time a square would take. (Measured on the CI VM: the chain case takes 2 s of CPU
    // checked by roots, and 22 s checked by a list of the chain searched for each original.)
    let n: u32 = 600_000;
    let skips: Vec<String> = (1..n).map(|k| format!("[{k}]")).collect();
    let text = edited(
        &grid_skip(&format!("[{}]", skips.join(", "))),
        DIRECTION_0,
        &DIRECTION_0.replace("\"count\": 3", &format!("\"count\": {n}")),
    );
    // Past the cap (600,000 by 2), so refused at the count that passes it alone, and that
    // alone: none of the 599,999 distinct skips is taken for a repeat.
    let refused = within(Duration::from_secs(10), move || {
        let invalid = Checked::new(read(text.as_bytes()).unwrap()).unwrap_err();
        let errors: Vec<(Option<String>, String)> = invalid
            .errors()
            .iter()
            .map(|e| (e.parameter().map(str::to_owned), e.message().to_owned()))
            .collect();
        (invalid.total(), errors)
    });
    assert_eq!(refused.0, 1, "{:?}", refused.1);
    assert_eq!(
        refused.1[0].0.as_deref(),
        Some("directions.0.count"),
        "{:?}",
        refused.1
    );
    assert!(
        refused.1[0].1.contains("has 1200000 instances in all"),
        "{:?}",
        refused.1
    );

    let ids: Vec<String> = (0..1_000_000u32).map(|k| format!("\"z{k:07}\"")).collect();
    let text = edited(
        &plate_text(),
        "\"features\": [\"q0000002\"]},\n      \"directions\"",
        &format!(
            "\"features\": [{}]}},\n      \"directions\"",
            ids.join(", ")
        ),
    );
    let total = within(Duration::from_secs(10), move || {
        Checked::new(read(text.as_bytes()).unwrap())
            .unwrap_err()
            .total()
    });
    assert_eq!(total, 1_000_000);

    // The plate's outline, plate, and notch sketch, then 60,000 pockets of the notch, each on
    // the one before, and a mirror of them all: every original in its base's chain.
    let plate = plate_text();
    let head = &plate[..plate.find("    {\n      \"id\": \"q0000001\"").unwrap()];
    let pockets = 60_000u32;
    let mut text = head.to_owned();
    let mut base = "p0000001".to_owned();
    for k in 1..=pockets {
        let id = format!("q{k:07}");
        text.push_str(&format!(
            "    {{\"id\": \"{id}\", \"type\": \"pocket\", \"v\": 1, \"name\": \"Notch {k}\", \"base\": \"{base}\", \"sketch\": \"s0000002\", \"extent\": {{\"type\": \"through_all\", \"reversed\": true}}}},\n"
        ));
        base = id;
    }
    let originals: Vec<String> = (1..=pockets).map(|k| format!("\"q{k:07}\"")).collect();
    text.push_str(&format!(
        "    {{\"id\": \"m0000001\", \"type\": \"mirror\", \"v\": 1, \"name\": \"Mirror\", \"base\": \"{base}\", \"copies\": {{\"type\": \"features\", \"features\": [{}]}}, \"plane\": \"yz\"}}\n  ]\n}}\n",
        originals.join(", ")
    ));
    let taken = within(Duration::from_secs(10), move || {
        Checked::new(read(text.as_bytes()).unwrap()).map(|_| ())
    });
    assert!(taken.is_ok(), "{taken:?}");
}

#[test]
fn patterns_that_differ_in_a_value_are_not_canonically_equal() {
    let plate = plate();
    for (from, to) in [
        (POLAR, POLAR.replace("\"count\": 4", "\"count\": 5")),
        (DIRECTION_0, DIRECTION_0.replace("80", "81")),
        ("\"plane\": \"yz\"", "\"plane\": \"xz\"".to_owned()),
        (GRID_SKIP, GRID_SKIP.replace("[]", "[[1]]")),
        (
            "\"copies\": {\"type\": \"features\", \"features\": [\"q0000001\"]},\n      \"plane\"",
            "\"copies\": {\"type\": \"solid\"},\n      \"plane\"".to_owned(),
        ),
    ] {
        let other = read(edited(&plate_text(), from, &to).as_bytes()).unwrap();
        assert!(!plate.canonically_eq(&other), "{to}");
    }
}

#[test]
fn a_pattern_moves_to_just_after_its_original_and_an_original_to_just_before_its_pattern() {
    // The moves the refusals mustn't catch: on the plate, with the boss's sketch moved
    // between the corner hole and its grid, the grid moved to just after the hole, and the
    // hole to just before the grid.
    use silico_core::document::chain::move_feature;
    let checked = Checked::new(plate()).unwrap();
    let spaced = Checked::new(
        move_feature(&checked, &id("s0000004"), Some(&id("t0000001")))
            .unwrap()
            .document,
    )
    .unwrap();
    let order = |document: &Document| {
        document
            .features
            .iter()
            .map(|f| f.id.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    let at = |list: &[String], f: &str| list.iter().position(|x| x == f).unwrap();
    let before = order(spaced.document());
    assert_eq!(at(&before, "s0000004") + 1, at(&before, "t0000001"));
    let grid = move_feature(&spaced, &id("t0000001"), Some(&id("s0000004"))).unwrap();
    let after = order(&grid.document);
    assert_eq!(at(&after, "q0000002") + 1, at(&after, "t0000001"));
    let hole = move_feature(&spaced, &id("q0000002"), Some(&id("t0000001"))).unwrap();
    let after = order(&hole.document);
    assert_eq!(at(&after, "q0000002") + 1, at(&after, "t0000001"));
}

#[test]
fn each_origin_word_of_the_wrong_kind_is_told_the_one_it_means() {
    // A plane as a direction or an axis is offered its normal, with its sense (xz's is −Y,
    // D6); an axis as a mirror plane the plane square to it.
    for (from, to, says) in [
        (
            "\"along\": \"x\",",
            "\"along\": \"xy\",",
            "give the plane's normal, z, or name an edge or a face",
        ),
        (
            "\"along\": \"x\",",
            "\"along\": \"xz\",",
            "give the plane's normal, y, with reversed true, or name an edge or a face",
        ),
        (
            "\"along\": \"x\",",
            "\"along\": \"yz\",",
            "give the plane's normal, x, or name an edge or a face",
        ),
        (
            "\"axis\": \"z\",",
            "\"axis\": \"xy\",",
            "give the plane's normal, z, or name an edge",
        ),
        (
            "\"plane\": \"yz\"",
            "\"plane\": \"x\"",
            "give the plane square to it, yz, or name a planar face",
        ),
        (
            "\"plane\": \"yz\"",
            "\"plane\": \"y\"",
            "give the plane square to it, xz, or name a planar face",
        ),
        (
            "\"plane\": \"yz\"",
            "\"plane\": \"z\"",
            "give the plane square to it, xy, or name a planar face",
        ),
    ] {
        let errors = refused(&edited(&plate_text(), from, to));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains(says), "{to}: {}", errors[0]);
    }
    let axis = refused(&edited(
        &plate_text(),
        "\"axis\": \"z\",",
        "\"axis\": \"xy\",",
    ));
    assert!(
        axis[0].contains("axis, \\\"xy\\\", is an origin plane: an axis is x, y, or z, or a straight or circular edge by its name"),
        "{}",
        axis[0]
    );
}

#[test]
fn the_gate_names_the_original_that_is_suppressed_by_its_place() {
    // The grid copying both pockets of its chain, the notch and the corner hole, the second
    // suppressed: its first refusal is at copies.features.1, the hole.
    let mut hand = plate();
    for feature in &mut hand.features {
        if feature.id == id("q0000002") {
            feature.suppressed = true;
        }
        if feature.id == id("t0000001")
            && let silico_core::document::Operation::LinearPattern { copies, .. } =
                &mut feature.operation
        {
            *copies = silico_core::document::Copies::Features {
                features: vec![id("q0000001"), id("q0000002")],
            };
        }
    }
    let invalid = Checked::new(hand).unwrap_err();
    let first = invalid
        .errors()
        .iter()
        .find(|e| e.feature() == Some("t0000001"))
        .unwrap();
    assert_eq!(first.parameter(), Some("copies.features.1"), "{first}");
    assert!(
        first
            .message()
            .contains("it copies q0000002, which is suppressed"),
        "{first}"
    );
}

#[test]
fn set_field_of_one_skip_gives_back_the_place_it_lands() {
    // The review's case: a grid with skip [[1], [2]], skip.0 set to [5], is written
    // [[2], [5]], and the result's `after` is [5], read where it lands, skip.1.
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let sketches = silico_core::fake::FakeSketches::new();
    let sessions = silico_core::command::Sessions::with_stand_ins(
        std::sync::Arc::new(silico_core::fake::FakeKernel::new(2).unwrap()),
        move |cache| silico_core::cache::Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn silico_core::sketch::SketchSolver>),
    );
    let path = dir.path().join("plate.silico");
    let grid = edited(
        &grid_skip("[[1], [2]]"),
        DIRECTION_0,
        &DIRECTION_0.replace("\"count\": 3", "\"count\": 6"),
    );
    std::fs::write(&path, &grid).unwrap();
    let session = answered(sessions.open(&path, false)).session.unwrap();
    let set = answered(sessions.set_field(
        &session.id,
        session.revision,
        id("t0000001"),
        "skip.0".to_owned(),
        silico_core::document::json::read(b"[5]").unwrap(),
    ))
    .result
    .unwrap();
    // Before the fix, `after` read skip.0 of the sorted set, [2].
    let written = wire(&set.to_wire(), Layout::OneLine).unwrap();
    assert!(
        written.contains("\"before\": [1], \"after\": [5]"),
        "{written}"
    );
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(
        saved.contains("\"skip\": [\n        [2],\n        [5]\n      ]"),
        "{saved}"
    );
}

/// The refusal of removing `removed` from `text` that names `child`, its suggestion.
fn removal_suggestion(text: &str, removed: &str, child: &str) -> String {
    let checked = Checked::new(read(text.as_bytes()).unwrap())
        .unwrap_or_else(|invalid| panic!("{:?}", invalid.errors()));
    let refusals =
        silico_core::document::chain::removal_refusals(&checked, &id(removed), usize::MAX).errors;
    let refusal = refusals
        .iter()
        .find(|e| {
            e.message()
                .starts_with(&format!("{child} names {removed}'s elements"))
        })
        .unwrap_or_else(|| {
            panic!(
                "{:?}",
                refusals.iter().map(|e| e.message()).collect::<Vec<_>>()
            )
        });
    refusal.suggestion().unwrap().to_owned()
}

#[test]
fn a_pattern_naming_an_element_is_told_the_field_that_names_it() {
    // Section 2.3's names: each pattern names an element by its own field, and removing the
    // element's maker says which field to set, not a fillet's `edges`.
    let linear = edited(
        &plate_text(),
        "\"along\": \"x\"",
        "\"along\": \"p0000001.pad.Edge(M s0000001.sketch.Edge.na000001)\"",
    );
    assert_eq!(
        removal_suggestion(&linear, "p0000001", "t0000001"),
        "remove t0000001 first, or set its directions' along to names of other features' elements"
    );
    let polar = edited(
        &plate_text(),
        "\"axis\": \"z\"",
        "\"axis\": \"p0000002.pad.Edge(G s0000004.sketch.Edge.cd000001)\"",
    );
    assert_eq!(
        removal_suggestion(&polar, "p0000002", "t0000002"),
        "remove t0000002 first, or set its axis to names of other features' elements"
    );
    let mirror = edited(
        &plate_text(),
        "\"plane\": \"yz\"",
        "\"plane\": \"p0000001.pad.Face(G s0000001.sketch.Edge.na000003)\"",
    );
    assert_eq!(
        removal_suggestion(&mirror, "p0000001", "m0000001"),
        "remove m0000001 first, or set its plane to names of other features' elements"
    );
    // A mirror of its solid in the notch's wall, moved before the notch: its base would be
    // the plate, not built from the notch, so the lineage refuses it, naming its plane.
    let walled = edited(
        &plate_text(),
        "\"copies\": {\"type\": \"features\", \"features\": [\"q0000001\"]},\n      \"plane\": \"yz\"",
        "\"copies\": {\"type\": \"solid\"},\n      \"plane\": \"q0000001.pocket.Face(G s0000002.sketch.Edge.nb000001)\"",
    );
    let checked = Checked::new(read(walled.as_bytes()).unwrap())
        .unwrap_or_else(|invalid| panic!("{:?}", invalid.errors()));
    let error = silico_core::document::chain::move_feature(
        &checked,
        &id("m0000001"),
        Some(&id("q0000001")),
    )
    .unwrap_err();
    assert_eq!(
        error.message(),
        "m0000001 names elements of q0000001, which its base would no longer be built from once m0000001 moves there"
    );
    assert_eq!(
        error.suggestion(),
        Some(
            "keep m0000001 after q0000001, whose elements it names, or set its plane to names of other features' elements"
        )
    );
}

/// `sessions` on the fakes, with `text` open from a file in `dir`: the session and its path.
fn opened(
    dir: &std::path::Path,
    text: &str,
) -> (
    silico_core::command::Sessions<std::sync::Arc<silico_core::fake::FakeKernel>>,
    silico_core::command::SessionId,
    silico_core::command::Revision,
    PathBuf,
) {
    let secret = dir.join("secret");
    let sketches = silico_core::fake::FakeSketches::new();
    let sessions = silico_core::command::Sessions::with_stand_ins(
        std::sync::Arc::new(silico_core::fake::FakeKernel::new(2).unwrap()),
        move |cache| silico_core::cache::Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn silico_core::sketch::SketchSolver>),
    );
    let path = dir.join("plate.silico");
    std::fs::write(&path, text).unwrap();
    let session = answered(sessions.open(&path, false)).session.unwrap();
    (sessions, session.id, session.revision, path)
}

#[test]
fn set_field_of_one_original_or_of_inside_a_skip_gives_back_where_it_lands() {
    // An original of [q0000001, q0000002] set to p0000001 is written first: `after` is read
    // at copies.features.0, where it lands, not at .1, which is q0000001 once sorted.
    let dir = tempfile::tempdir().unwrap();
    let two = edited(
        &plate_text(),
        "\"features\": [\"q0000002\"]},\n      \"directions\"",
        "\"features\": [\"q0000001\", \"q0000002\"]},\n      \"directions\"",
    );
    let (sessions, session, revision, path) = opened(dir.path(), &two);
    let set = answered(sessions.set_field(
        &session,
        revision,
        id("t0000001"),
        "copies.features.1".to_owned(),
        silico_core::document::json::read(b"\"p0000001\"").unwrap(),
    ))
    .result
    .unwrap();
    let written = wire(&set.to_wire(), Layout::OneLine).unwrap();
    assert!(
        written.contains("\"before\": \"q0000002\", \"after\": \"p0000001\""),
        "{written}"
    );
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(
        saved.contains("\"features\": [\"p0000001\", \"q0000001\"]"),
        "{saved}"
    );

    // Inside an instance: skip.0.0 of [[1], [2]] set to 5 is written [[2], [5]]: `after` is
    // read at skip.1.0, 5, not at skip.0.0, 2.
    let dir = tempfile::tempdir().unwrap();
    let grid = edited(
        &grid_skip("[[1], [2]]"),
        DIRECTION_0,
        &DIRECTION_0.replace("\"count\": 3", "\"count\": 6"),
    );
    let (sessions, session, revision, path) = opened(dir.path(), &grid);
    let set = answered(sessions.set_field(
        &session,
        revision,
        id("t0000001"),
        "skip.0.0".to_owned(),
        silico_core::document::json::read(b"5").unwrap(),
    ))
    .result
    .unwrap();
    let written = wire(&set.to_wire(), Layout::OneLine).unwrap();
    assert!(written.contains("\"before\": 1, \"after\": 5"), "{written}");
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(
        saved.contains("\"skip\": [\n        [2],\n        [5]\n      ]"),
        "{saved}"
    );
}

#[test]
fn removing_an_original_copied_past_the_list_lists_a_hundred_and_counts_them_all() {
    // The notch copied by 150 mirrors, each on the one before: removing it lists the first
    // hundred refusals, in timeline order, and counts all 150.
    let plate = plate_text();
    let head = &plate[..plate.find("    {\n      \"id\": \"m0000001\"").unwrap()];
    let mut text = head.to_owned();
    let mut base = "q0000001".to_owned();
    let mirrors = 150u32;
    for k in 1..=mirrors {
        let id = format!("m{k:07}");
        let comma = if k == mirrors { "" } else { "," };
        text.push_str(&format!(
            "    {{\"id\": \"{id}\", \"type\": \"mirror\", \"v\": 1, \"name\": \"Mirror {k}\", \"base\": \"{base}\", \"copies\": {{\"type\": \"features\", \"features\": [\"q0000001\"]}}, \"plane\": \"yz\"}}{comma}\n"
        ));
        base = id;
    }
    text.push_str("  ]\n}\n");
    let dir = tempfile::tempdir().unwrap();
    let (sessions, session, revision, _) = opened(dir.path(), &text);
    let problems = answered(sessions.remove_feature(&session, revision, id("q0000001")))
        .result
        .unwrap_err();
    assert_eq!(problems.total(), u64::from(mirrors));
    assert_eq!(
        problems.errors().len(),
        silico_core::document::PROBLEMS_LISTED
    );
    for (k, error) in problems.errors().iter().enumerate() {
        assert_eq!(
            error.message(),
            format!(
                "m{:07} copies q0000001, so q0000001 can't be removed",
                k + 1
            )
        );
    }
}
