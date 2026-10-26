//! M2.9: the core's commands, `check`, `format`, and `outline`, on the fake kernel. Expected
//! values are written by hand: the example's outline, and its fingerprints as the fake
//! measures them (tests/fingerprint.rs works them out).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::commands;
use super::example;

use commands::{check_bytes, engine, format_bytes};
use example::EXAMPLE;
use silico_core::command::{self, Formatted};
use silico_core::document::{FeatureId, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Op, Primitive};
use silico_core::recompute::Recompute;
use silico_core::{Error, ErrorCode};

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

const OUTLINE: &str = r#"k7f2qx3m "Block" box length 40, width 20, height 10, at (0, 0, 0)
a91cw0de "Pin" cylinder radius 2.5, height 12, at (20, 10, -1)
m3tq8v0z "Hole" cut base k7f2qx3m, tool a91cw0de
  note "Clearance for an M5 bolt"
bodies m3tq8v0z
"#;

/// The example's three fingerprints, as the outline writes them.
const BLOCK: &str =
    "volume 8000, area 2800, box (0, 0, 0) to (40, 20, 10), 6 faces, 12 edges, 8 vertices";
const PIN: &str = "volume 235.619449, area 227.765467, box (17.5, 7.5, -1) to (22.5, 12.5, 11), 3 faces, 3 edges, 2 vertices";
const HOLE: &str = "volume 8235.619449, area 3027.765467, box (0, 0, -1) to (40, 20, 11), 9 faces, 15 edges, 10 vertices";

fn formatted(text: &str) -> Formatted {
    format_bytes(&engine(), text.as_bytes()).unwrap().unwrap()
}

#[test]
fn an_outline_says_each_feature_on_a_line_of_its_own() {
    assert_eq!(
        command::outline(&read(EXAMPLE.as_bytes()).unwrap()),
        OUTLINE
    );
}

#[test]
fn an_outline_says_a_sketchs_elements_a_line_each_and_no_sketch_is_a_body() {
    // M3.2's sketches, in an outline (found on rebasing M2.9 onto them): the sketch's line
    // says its plane, then each point, curve, and constraint on a line of its own, as the
    // file writes it, so an edit to one element changes its line alone. A sketch makes no
    // solid, so it isn't among the bodies.
    let sketch = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Profile", "plane": "xy", "points": [{"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pa000000", "x": 0, "y": 0, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "c0000001", "type": "horizontal", "line": "na000000"}]},"#;
    let text = EXAMPLE.replacen(
        "    {\"id\": \"k7f2qx3m\"",
        &format!("    {sketch}\n    {{\"id\": \"k7f2qx3m\""),
        1,
    );
    assert_ne!(text, EXAMPLE);
    let expected = format!(
        r#"s0000001 "Profile" sketch plane xy
  point {{"id": "pa000000", "x": 0, "y": 0, "construction": false}}
  point {{"id": "pb000000", "x": 10, "y": 0, "construction": false}}
  line {{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}}
  constraint {{"id": "c0000001", "type": "horizontal", "line": "na000000"}}
{OUTLINE}"#
    );
    let outline = command::outline(&read(text.as_bytes()).unwrap());
    assert_eq!(outline, expected);
    // One point moved: its line alone.
    let moved = text.replace(r#""x": 10, "y": 0"#, r#""x": 12, "y": 0"#);
    let after = command::outline(&read(moved.as_bytes()).unwrap());
    let changed: Vec<(&str, &str)> = outline
        .lines()
        .zip(after.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(
        changed,
        [(
            r#"  point {"id": "pb000000", "x": 10, "y": 0, "construction": false}"#,
            r#"  point {"id": "pb000000", "x": 12, "y": 0, "construction": false}"#
        )]
    );
}

#[test]
fn an_outline_says_a_sketchs_circles_and_arcs_by_kind() {
    // Found by the review: no outline test had a circle or an arc. A sketch alone, on yz:
    // its points, circle, arc, and constraint, each by its kind, and no bodies.
    let sketch = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "P", "plane": "yz", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 3, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 3, "construction": false}], "lines": [], "circles": [{"id": "ca000000", "center": "pa000000", "radius": 2.5, "construction": true}], "arcs": [{"id": "aa000000", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}], "constraints": [{"id": "c0000001", "type": "radius", "curve": "ca000000", "value": 2.5, "driving": false}]}"#;
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{sketch}]}}"#
    );
    assert_eq!(
        command::outline(&read(text.as_bytes()).unwrap()),
        r#"s0000001 "P" sketch plane yz
  point {"id": "pa000000", "x": 0, "y": 0, "construction": false}
  point {"id": "pb000000", "x": 3, "y": 0, "construction": false}
  point {"id": "pc000000", "x": 0, "y": 3, "construction": false}
  circle {"id": "ca000000", "center": "pa000000", "radius": 2.5, "construction": true}
  arc {"id": "aa000000", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}
  constraint {"id": "c0000001", "type": "radius", "curve": "ca000000", "value": 2.5, "driving": false}
bodies none
"#
    );
}

#[test]
fn a_document_with_a_sketch_formats_with_the_solids_fingerprints_alone_and_checks_clean() {
    // Found by the review: nothing formatted or checked a document with a sketch. A sketch
    // has no solid, so no fingerprint (the gate would refuse one): `format` writes the
    // solids' alone, and its text checks clean.
    let sketch = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Profile", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []},"#;
    let text = EXAMPLE.replacen(
        "    {\"id\": \"k7f2qx3m\"",
        &format!("    {sketch}\n    {{\"id\": \"k7f2qx3m\""),
        1,
    );
    let done = formatted(&text);
    let written: Vec<String> = done
        .refingerprinted
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(written, ["k7f2qx3m", "a91cw0de", "m3tq8v0z"]);
    let written_back = read(done.text.as_bytes()).unwrap();
    assert!(written_back.features[0].fingerprint.is_none());
    let checked = check_bytes(&engine(), done.text.as_bytes()).unwrap();
    assert!(checked.is_empty(), "{checked:?}");
}

#[test]
fn an_outline_says_a_fillets_or_chamfers_edges_a_line_each() {
    // M2.8's dressings, met on rebasing onto M2.9's outline: a fillet's line says its base
    // and radius, a chamfer's its base and distance, then each edge on a line of its own,
    // quoted, so an edit to one edge changes its line alone.
    let more = r#",
    {"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "m3tq8v0z", "edges": ["k7f2qx3m.box.Edge1", "m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)"], "radius": 0.5},
    {"id": "c0000001", "type": "chamfer", "v": 1, "name": "Bevel", "base": "f0000001", "edges": ["k7f2qx3m.box.Edge5"], "size": {"distance": 1}}
  ]"#;
    let at = EXAMPLE.rfind("\n  ]").unwrap();
    let text = format!("{}{more}{}", &EXAMPLE[..at], &EXAMPLE[at + 4..]);
    let outline = command::outline(&read(text.as_bytes()).unwrap());
    let tail: Vec<&str> = outline.lines().skip(4).collect();
    assert_eq!(
        tail,
        [
            r#"f0000001 "Round" fillet base m3tq8v0z, radius 0.5"#,
            r#"  edge "k7f2qx3m.box.Edge1""#,
            r#"  edge "m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)""#,
            r#"c0000001 "Bevel" chamfer base f0000001, size.distance 1"#,
            r#"  edge "k7f2qx3m.box.Edge5""#,
            "bodies c0000001",
        ]
    );
}

#[test]
fn an_outline_has_each_features_fingerprint_on_its_line() {
    let saved = formatted(EXAMPLE).text;
    let outline = command::outline(&read(saved.as_bytes()).unwrap());
    let lines: Vec<&str> = outline.lines().collect();
    assert_eq!(
        lines[0],
        format!("k7f2qx3m \"Block\" box length 40, width 20, height 10, at (0, 0, 0) | {BLOCK}")
    );
    assert_eq!(
        lines[1],
        format!("a91cw0de \"Pin\" cylinder radius 2.5, height 12, at (20, 10, -1) | {PIN}")
    );
    assert_eq!(
        lines[2],
        format!("m3tq8v0z \"Hole\" cut base k7f2qx3m, tool a91cw0de | {HOLE}")
    );
    assert_eq!(lines.len(), 5);
}

#[test]
fn an_edit_to_one_value_changes_one_line_of_the_outline() {
    // The diff `git diff` shows through `silico show`: the pin's radius, one line; the
    // block's name, one line (nothing names features by name but the feature itself).
    let before = command::outline(&read(EXAMPLE.as_bytes()).unwrap());
    for edited in [
        EXAMPLE.replace(r#""radius": 2.5"#, r#""radius": 3"#),
        EXAMPLE.replace(r#""name": "Block""#, r#""name": "Base plate""#),
    ] {
        let after = command::outline(&read(edited.as_bytes()).unwrap());
        let changed = before
            .lines()
            .zip(after.lines())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(changed, 1, "{after}");
        assert_eq!(before.lines().count(), after.lines().count());
    }
}

#[test]
fn an_outline_escapes_what_could_break_or_hide_in_its_lines() {
    let text = EXAMPLE
        .replace(r#""name": "Block""#, r#""name": "a \"quoted\"\nname\\""#)
        .replace(
            r#""note": "Clearance for an M5 bolt""#,
            r#""note": "rtl\u202e and zero\u200bwidth""#,
        );
    let outline = command::outline(&read(text.as_bytes()).unwrap());
    assert!(
        outline.starts_with(r#"k7f2qx3m "a \"quoted\"\u{a}name\\" box"#),
        "{outline}"
    );
    assert!(
        outline.contains(r#"  note "rtl\u{202e} and zero\u{200b}width""#),
        "{outline}"
    );
    assert_eq!(outline.lines().count(), 5);
    // Every space but the plain one, which would look like it, written so it doesn't
    // (found by the review).
    let spaces: Vec<char> = (0..=0x10ffff_u32)
        .filter_map(char::from_u32)
        .filter(|c| c.is_whitespace() && *c != ' ' && !c.is_control())
        .collect();
    assert!(spaces.contains(&'\u{a0}') && spaces.contains(&'\u{3000}'));
    for space in spaces {
        let text = EXAMPLE.replace(
            r#""name": "Block""#,
            &format!(r#""name": "Top{space}Plate""#),
        );
        let outline = command::outline(&read(text.as_bytes()).unwrap());
        let written = format!("\\u{{{:x}}}", space as u32);
        assert!(
            outline.starts_with(&format!(r#"k7f2qx3m "Top{written}Plate" box"#)),
            "{space:?}: {outline}"
        );
    }
    let text = EXAMPLE.replace(r#""name": "Block""#, r#""name": "Top Plate""#);
    assert!(
        command::outline(&read(text.as_bytes()).unwrap())
            .starts_with(r#"k7f2qx3m "Top Plate" box"#)
    );
}

#[test]
fn a_sound_document_checks_clean() {
    let saved = formatted(EXAMPLE).text;
    let problems = check_bytes(&engine(), saved.as_bytes()).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    // With no fingerprints stored, there's nothing to drift from: clean too.
    assert!(
        check_bytes(&engine(), EXAMPLE.as_bytes())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_document_that_doesnt_read_is_one_problem_saying_where() {
    let problems = check_bytes(&engine(), b"{\"silico\": 1,").unwrap();
    assert_eq!(problems.total(), 1);
    assert_eq!(problems.errors()[0].code(), ErrorCode::InvalidInput);
}

#[test]
fn every_problem_the_gate_finds_is_reported() {
    // The cut uses itself as its tool, and the box is 0 long.
    let text = EXAMPLE
        .replace(r#""tool": "a91cw0de""#, r#""tool": "m3tq8v0z""#)
        .replace(r#""length": 40"#, r#""length": 0"#);
    let problems = check_bytes(&engine(), text.as_bytes()).unwrap();
    let said: Vec<_> = problems
        .errors()
        .iter()
        .map(|e| {
            (
                e.feature().unwrap().to_owned(),
                e.parameter().map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(
        said,
        [
            ("k7f2qx3m".to_owned(), Some("length".to_owned())),
            ("m3tq8v0z".to_owned(), Some("tool".to_owned())),
        ]
    );
    assert_eq!(problems.total(), 2);
}

#[test]
fn a_failure_and_drift_are_problems_and_a_blocked_feature_isnt_one_of_its_own() {
    // Drift: the block stored as 8001.
    let saved = formatted(EXAMPLE).text;
    let drifted = saved.replacen(r#""volume": 8000,"#, r#""volume": 8001,"#, 1);
    let problems = check_bytes(&engine(), drifted.as_bytes()).unwrap();
    let codes: Vec<_> = problems.errors().iter().map(Error::code).collect();
    assert_eq!(codes, [ErrorCode::Drift]);
    // The pin refused: its failure, naming it; the cut, blocked on it, says nothing more.
    let refusing = engine();
    refusing.kernel().refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
            .then(|| Error::new(ErrorCode::KernelFailed, "MakeCylinder failed"))
    });
    let problems = check_bytes(&refusing, EXAMPLE.as_bytes()).unwrap();
    let said: Vec<_> = problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature().unwrap().to_owned()))
        .collect();
    assert_eq!(said, [(ErrorCode::KernelFailed, "a91cw0de".to_owned())]);
}

#[test]
fn formatting_writes_canonical_text_with_fingerprints_and_says_which_it_wrote() {
    // Spelled loosely: on one line, keys out of order, an exponent.
    let loose = r#"{"units": {"resolution": "1nm", "length": "mm"}, "silico": 1, "features": [{"name": "Block", "id": "k7f2qx3m", "type": "box", "v": 1, "position": [0, 0, 0], "length": 4e1, "width": 20, "height": 10}]}"#;
    let out = formatted(loose);
    let document = read(out.text.as_bytes()).unwrap();
    assert_eq!(out.text, silico_core::document::write(&document).unwrap());
    assert_eq!(
        out.refingerprinted,
        [id("k7f2qx3m")],
        "the box's fingerprint, new"
    );
    assert!(out.text.contains(r#""length": 40,"#), "{}", out.text);
    assert!(out.problems.is_empty());
    // Formatting what it wrote changes nothing, and writes no fingerprint.
    let again = formatted(&out.text);
    assert_eq!(again.text, out.text);
    assert!(again.refingerprinted.is_empty());
}

#[test]
fn formatting_rewrites_a_stale_fingerprint_and_says_which() {
    let saved = formatted(EXAMPLE).text;
    let stale = saved.replacen(r#""volume": 8000,"#, r#""volume": 8001,"#, 1);
    let out = formatted(&stale);
    assert_eq!(out.text, saved);
    assert_eq!(out.refingerprinted, [id("k7f2qx3m")]);
    // The drift it fixed isn't a problem it reports.
    assert!(out.problems.is_empty(), "{:?}", out.problems);
}

#[test]
fn formatting_keeps_a_failed_features_fingerprint_and_says_why() {
    let saved = formatted(EXAMPLE).text;
    let refusing = engine();
    refusing.kernel().refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
            .then(|| Error::new(ErrorCode::KernelFailed, "MakeCylinder failed"))
    });
    let out = format_bytes(&refusing, saved.as_bytes()).unwrap().unwrap();
    assert_eq!(out.text, saved);
    let said: Vec<_> = out
        .problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature().unwrap().to_owned()))
        .collect();
    assert_eq!(said, [(ErrorCode::KernelFailed, "a91cw0de".to_owned())]);
}

#[test]
fn a_document_that_doesnt_pass_the_gate_isnt_formatted() {
    let text = EXAMPLE.replace(r#""length": 40"#, r#""length": 0"#);
    let problems = format_bytes(&engine(), text.as_bytes())
        .unwrap()
        .unwrap_err();
    assert_eq!(problems.total(), 1);
    assert_eq!(problems.errors()[0].parameter(), Some("length"));
}

#[test]
fn an_outline_says_every_kind_of_feature_with_its_own_values() {
    // Written by hand from the document: each primitive's values by the names the file
    // gives them, in the file's order, and a fingerprint with no box (a solid with nothing
    // in it).
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [
    {"id": "00000000", "type": "sphere", "v": 1, "name": "S", "position": [1, 2, 3], "radius": 0.000001},
    {"id": "11111111", "type": "cone", "v": 1, "name": "C", "position": [0, 0, 0], "radius1": 5, "radius2": 0, "height": 7.25},
    {"id": "22222222", "type": "torus", "v": 1, "name": "T", "position": [-0.5, 0, 0], "radius1": 10, "radius2": 2},
    {"id": "33333333", "type": "fuse", "v": 1, "name": "F", "base": "00000000", "tool": "11111111"},
    {"id": "44444444", "type": "common", "v": 1, "name": "M", "base": "33333333", "tool": "22222222", "fingerprint": {"volume": 0, "area": 0, "faces": 0, "edges": 0, "vertices": 0}}]}"#;
    assert_eq!(
        command::outline(&read(text.as_bytes()).unwrap()),
        r#"00000000 "S" sphere radius 0.000001, at (1, 2, 3)
11111111 "C" cone radius1 5, radius2 0, height 7.25, at (0, 0, 0)
22222222 "T" torus radius1 10, radius2 2, at (-0.5, 0, 0)
33333333 "F" fuse base 00000000, tool 11111111
44444444 "M" common base 33333333, tool 22222222 | volume 0, area 0, no box, 0 faces, 0 edges, 0 vertices
bodies 44444444
"#
    );
}

#[test]
fn formatting_says_which_solids_it_couldnt_measure_and_keeps_their_fingerprints() {
    // Found by the review: nothing tested it. Every summary refused: each feature keeps
    // what was stored, and says why.
    let saved = formatted(EXAMPLE).text;
    let refusing = engine();
    refusing.kernel().refuse(|op| {
        matches!(op, Op::Summarize { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "Summarize failed"))
    });
    let out = format_bytes(&refusing, saved.as_bytes()).unwrap().unwrap();
    assert_eq!(out.text, saved, "each keeps its fingerprint");
    assert!(out.refingerprinted.is_empty());
    let said: Vec<_> = out
        .problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature().unwrap().to_owned()))
        .collect();
    assert_eq!(
        said,
        [
            (ErrorCode::KernelFailed, "k7f2qx3m".to_owned()),
            (ErrorCode::KernelFailed, "a91cw0de".to_owned()),
            (ErrorCode::KernelFailed, "m3tq8v0z".to_owned()),
        ]
    );
}

#[test]
fn every_kernel_fault_is_set_aside_by_check_and_format_and_held_by_the_model() {
    // A worker that crashed, hung, or couldn't be had, a reply that couldn't be read, a
    // shortage, or a bug: none says anything about the document, so check and format set
    // it aside, building or measuring alike: no problem of the document's, the model holding
    // it, and the save made around it, each feature it stopped keeping what's stored with it.
    let saved = formatted(EXAMPLE).text;
    for code in [
        ErrorCode::WorkerCrashed,
        ErrorCode::WorkerTimeout,
        ErrorCode::ProtocolError,
        ErrorCode::ResourceExhausted,
        ErrorCode::Internal,
        ErrorCode::WorkerUnavailable,
    ] {
        let building = move |op: &Op| {
            matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
                .then(|| Error::new(code, "fault"))
        };
        let measuring =
            move |op: &Op| matches!(op, Op::Summarize { .. }).then(|| Error::new(code, "fault"));
        // Building, the pin isn't made, nor the hole cut with it: the block's fingerprint
        // alone is written, the others kept as stored (none). Measuring, nothing is: every
        // fingerprint kept as stored, the text as it was saved.
        for (what, text, refingerprinted) in [
            ("building", EXAMPLE, vec![id("k7f2qx3m")]),
            ("measuring", saved.as_str(), vec![]),
        ] {
            let engine = engine();
            if what == "building" {
                engine.kernel().refuse(building);
            } else {
                engine.kernel().refuse(measuring);
            }
            let model = command::rebuild(&engine, command::gate(text.as_bytes()).unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(
                model.fault().map(|e| (e.code(), e.message())),
                Some((code, "fault")),
                "{what}, {code:?}"
            );
            assert!(command::check(&model).is_empty(), "check, {what}, {code:?}");
            let out = command::format(&model).unwrap();
            assert!(out.problems.is_empty(), "format, {what}, {code:?}: {out:?}");
            assert_eq!(out.refingerprinted, refingerprinted, "{what}, {code:?}");
            if what == "measuring" {
                assert_eq!(out.text, saved, "{code:?}");
            }
        }
    }
}

#[test]
fn check_and_format_read_the_model_given_and_ask_the_kernel_nothing() {
    // Found by the review: they took the engine and started a recompute on it, so a
    // client's live recompute (the app's) would be superseded by a check. Given a model,
    // they rebuild nothing and leave the engine as it was.
    let engine = engine();
    let saved = formatted(EXAMPLE).text;
    let model = command::rebuild(&engine, command::gate(saved.as_bytes()).unwrap())
        .unwrap()
        .unwrap();
    let calls = engine.kernel().calls().len();
    assert!(command::check(&model).is_empty());
    let out = command::format(&model).unwrap();
    assert_eq!(out.text, saved);
    assert_eq!(engine.kernel().calls().len(), calls);
    assert_eq!(engine.published().as_ref(), Some(&model));
}

#[test]
fn a_rebuild_superseded_by_another_is_none_not_an_error() {
    // A client that shares its engine (the app) has the newer run's model: the older
    // rebuild says it was superseded, as data, not as a bug.
    let engine = engine();
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. })));
    let block = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;
    std::thread::scope(|scope| {
        let older =
            scope.spawn(|| command::rebuild(&engine, command::gate(EXAMPLE.as_bytes()).unwrap()));
        // Once the older one is at the pin, held, a newer one supersedes it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !format!("{:?}", engine.kernel().calls()).contains("Cylinder") {
            assert!(
                std::time::Instant::now() < deadline,
                "the pin never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let newer = command::rebuild(&engine, command::gate(block.as_bytes()).unwrap())
            .unwrap()
            .unwrap();
        hold.release();
        assert!(older.join().unwrap().unwrap().is_none());
        assert_eq!(engine.published().as_ref(), Some(&newer));
    });
}

#[test]
fn engines_sharing_a_kernel_in_turn_dont_supersede_each_other() {
    // The CLI's files in several directories: an engine (a cache) each, one kernel, run
    // one after another. One engine runs several times before the other first runs, so
    // the kernel is generations past the other's own: each starts a generation newer than
    // the kernel's, so each publishes (found by the review: in even turns, an engine that
    // ignored the kernel's generation passed).
    let kernel = std::sync::Arc::new(FakeKernel::new(2).unwrap());
    let first = Recompute::new(std::sync::Arc::clone(&kernel));
    let second = Recompute::new(std::sync::Arc::clone(&kernel));
    let rebuilt = |engine: &Recompute<std::sync::Arc<FakeKernel>>| {
        let model = command::rebuild(engine, command::gate(EXAMPLE.as_bytes()).unwrap())
            .unwrap()
            .expect("superseded");
        assert!(command::check(&model).is_empty());
    };
    for _ in 0..3 {
        rebuilt(&first);
    }
    rebuilt(&second);
    rebuilt(&first);
    for _ in 0..2 {
        rebuilt(&second);
    }
    rebuilt(&first);
}

#[test]
fn a_checks_total_counts_every_broken_reference_listed_or_not() {
    // Found by the rebase's review: a feature lists its first hundred broken references,
    // and a check's total counted a summary of the rest as one problem, so 150 broken
    // references were `total: 101`. The total is every one, as the gate's is.
    let edges: Vec<String> = (13..163)
        .map(|n| format!("\"k7f2qx3m.box.Edge{n}\""))
        .collect();
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [
{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}},
{{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": [{}], "radius": 1}}]}}"#,
        edges.join(", ")
    );
    let problems = check_bytes(&engine(), text.as_bytes()).unwrap();
    assert_eq!(problems.errors().len(), 100);
    assert_eq!(problems.total(), 150);
    assert!(
        problems
            .errors()
            .iter()
            .all(|e| e.code() == ErrorCode::ReferenceBroken)
    );
}

#[test]
fn a_formats_total_counts_every_broken_reference_and_a_failure_counts_once() {
    // Found by the review: only a check's total was tested. A format's counts every
    // broken reference too, and a failed feature is one error, listed and counted.
    let edges: Vec<String> = (13..163)
        .map(|n| format!("\"k7f2qx3m.box.Edge{n}\""))
        .collect();
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [
{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}},
{{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": [{}], "radius": 1}}]}}"#,
        edges.join(", ")
    );
    let done = format_bytes(&engine(), text.as_bytes()).unwrap().unwrap();
    assert_eq!(done.problems.errors().len(), 100);
    assert_eq!(done.problems.total(), 150);
    let refusing = engine();
    refusing.kernel().refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
            .then(|| Error::new(ErrorCode::KernelFailed, "MakeCylinder failed"))
    });
    let model = command::rebuild(&refusing, command::gate(EXAMPLE.as_bytes()).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(model.error_count(), 1);
    assert_eq!(model.unlisted_errors(), 0);
    assert_eq!(command::check(&model).total(), 1);
}

#[test]
fn two_fillets_broken_past_the_hundred_and_a_failure_each_list_their_own_and_count_every_one() {
    // Found by the review: no test had two features with broken references, so a count of
    // the first feature's alone passed, and so did a second feature listing the first's
    // errors. Two fillets, 150 and 130 broken edges, and a cylinder the kernel refuses:
    // listed, the first hundred of each fillet's own and the failure, 201; counted, 281.
    let names = |base: &str, count: u32| -> String {
        (13..13 + count)
            .map(|n| format!("\"{base}.box.Edge{n}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [
{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}},
{{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": [{}], "radius": 1}},
{{"id": "b0000002", "type": "box", "v": 1, "name": "Block2", "position": [100, 0, 0], "length": 40, "width": 20, "height": 10}},
{{"id": "f0000002", "type": "fillet", "v": 1, "name": "Round2", "base": "b0000002", "edges": [{}], "radius": 1}},
{{"id": "y0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [0, 0, 0], "radius": 1, "height": 1}}]}}"#,
        names("k7f2qx3m", 150),
        names("b0000002", 130)
    );
    let refusing = engine();
    refusing.kernel().refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
            .then(|| Error::new(ErrorCode::KernelFailed, "MakeCylinder failed"))
    });
    let problems = check_bytes(&refusing, text.as_bytes()).unwrap();
    let of = |f: &str| {
        problems
            .errors()
            .iter()
            .filter(|e| e.feature() == Some(f))
            .collect::<Vec<_>>()
    };
    assert_eq!(of("f0000001").len(), 100);
    assert_eq!(of("f0000002").len(), 100);
    assert_eq!(of("y0000001").len(), 1);
    // Each fillet's errors are its own names.
    assert_eq!(of("f0000001")[0].elements(), ["k7f2qx3m.box.Edge13"]);
    assert_eq!(of("f0000002")[0].elements(), ["b0000002.box.Edge13"]);
    assert_eq!(problems.errors().len(), 201);
    assert_eq!(problems.total(), 150 + 130 + 1);
}
