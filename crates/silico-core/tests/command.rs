//! M2.9: the core's commands, `check`, `format`, and `outline`, on the fake kernel. Expected
//! values are written by hand: the example's outline, and its fingerprints as the fake
//! measures them (tests/fingerprint.rs works them out).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/example.rs"]
mod example;

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

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap())
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
    command::format(&engine(), text.as_bytes())
        .unwrap()
        .unwrap()
}

#[test]
fn an_outline_says_each_feature_on_a_line_of_its_own() {
    assert_eq!(
        command::outline(&read(EXAMPLE.as_bytes()).unwrap()),
        OUTLINE
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
}

#[test]
fn a_sound_document_checks_clean() {
    let saved = formatted(EXAMPLE).text;
    let problems = command::check(&engine(), saved.as_bytes()).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    // With no fingerprints stored, there's nothing to drift from: clean too.
    assert!(
        command::check(&engine(), EXAMPLE.as_bytes())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_document_that_doesnt_read_is_one_problem_saying_where() {
    let problems = command::check(&engine(), b"{\"silico\": 1,").unwrap();
    assert_eq!(problems.total, 1);
    assert_eq!(problems.problems[0].code(), ErrorCode::InvalidInput);
}

#[test]
fn every_problem_the_gate_finds_is_reported() {
    // The cut uses itself as its tool, and the box is 0 long.
    let text = EXAMPLE
        .replace(r#""tool": "a91cw0de""#, r#""tool": "m3tq8v0z""#)
        .replace(r#""length": 40"#, r#""length": 0"#);
    let problems = command::check(&engine(), text.as_bytes()).unwrap();
    let said: Vec<_> = problems
        .problems
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
    assert_eq!(problems.total, 2);
}

#[test]
fn a_failure_and_drift_are_problems_and_a_blocked_feature_isnt_one_of_its_own() {
    // Drift: the block stored as 8001.
    let saved = formatted(EXAMPLE).text;
    let drifted = saved.replacen(r#""volume": 8000,"#, r#""volume": 8001,"#, 1);
    let problems = command::check(&engine(), drifted.as_bytes()).unwrap();
    let codes: Vec<_> = problems.problems.iter().map(Error::code).collect();
    assert_eq!(codes, [ErrorCode::Drift]);
    // The pin refused: its failure, naming it; the cut, blocked on it, says nothing more.
    let refusing = engine();
    refusing.kernel().refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
            .then(|| Error::new(ErrorCode::KernelFailed, "MakeCylinder failed"))
    });
    let problems = command::check(&refusing, EXAMPLE.as_bytes()).unwrap();
    let said: Vec<_> = problems
        .problems
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
        [(id("k7f2qx3m"), None)],
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
fn formatting_rewrites_a_stale_fingerprint_and_says_what_it_was() {
    let saved = formatted(EXAMPLE).text;
    let stale = saved.replacen(r#""volume": 8000,"#, r#""volume": 8001,"#, 1);
    let out = formatted(&stale);
    assert_eq!(out.text, saved);
    assert_eq!(out.refingerprinted.len(), 1);
    let (feature, before) = &out.refingerprinted[0];
    assert_eq!(feature, &id("k7f2qx3m"));
    assert_eq!(before.unwrap().volume.to_string(), "8001");
}

#[test]
fn formatting_keeps_a_failed_features_fingerprint_and_says_why() {
    let saved = formatted(EXAMPLE).text;
    let refusing = engine();
    refusing.kernel().refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
            .then(|| Error::new(ErrorCode::KernelFailed, "MakeCylinder failed"))
    });
    let out = command::format(&refusing, saved.as_bytes())
        .unwrap()
        .unwrap();
    assert_eq!(out.text, saved);
    let said: Vec<_> = out
        .problems
        .iter()
        .map(|e| (e.code(), e.feature().unwrap().to_owned()))
        .collect();
    assert_eq!(said, [(ErrorCode::KernelFailed, "a91cw0de".to_owned())]);
}

#[test]
fn a_document_that_doesnt_pass_the_gate_isnt_formatted() {
    let text = EXAMPLE.replace(r#""length": 40"#, r#""length": 0"#);
    let problems = command::format(&engine(), text.as_bytes())
        .unwrap()
        .unwrap_err();
    assert_eq!(problems.total, 1);
    assert_eq!(problems.problems[0].parameter(), Some("length"));
}
