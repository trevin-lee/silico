//! M6.2: the linear pattern, polar pattern, and mirror in the document
//! (docs/design/m6.0-patterns.md), held to the design's own text: the plate of section 2.2
//! and each form of section 2.3 read and written back byte for byte; their outline lines,
//! section 2.5's; each gate refusal of section 4.1 made by its edit and equal to its example
//! byte for byte; and section 6's edits, a pattern a child of each original. Every example
//! is read from the design, not copied here, so the two can't drift apart.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use silico_core::command::wire::{Layout, ToWire, write as wire};
use silico_core::document::{Checked, Document, FeatureId, read, write};

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
        (14, grid_skip("[[3]]")),
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
    assert_eq!(refused(&grid_skip("[[2, 2]]")).len(), 1);
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
    let refusals = silico_core::document::chain::removal_refusals(&checked, &id("q0000002"));
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
