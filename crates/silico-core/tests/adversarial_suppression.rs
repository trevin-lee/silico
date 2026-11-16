//! Hostile inputs for suppression (`document::suppression`, the gate's rule, and the rebuild
//! on the fake kernel): names that reach a solid through a suppressed boolean's tool, the
//! reader's `suppressed` in every wrong shape and place, suppressed features whose own
//! values or references are bad, long chains, and random documents of boxes, chamfers, and
//! booleans put through random suppressions and unsuppressions, each result held to the
//! gate, to a rebuild with no errors, and to its undoing.
//!
//! Every expected value is the design's rules (docs/ports/suppression.md) worked by hand, or
//! the invariant the port note states: "A feature that can't build is suppressed, not left
//! failing", and "a hand edit or a merge can't leave a feature failing for a suppression".

use std::io::Write;
use std::time::{Duration, Instant};

use proptest::prelude::*;

use super::bracket::*;
use super::cpu_work;
use super::model::{id, published};

use silico_core::ErrorCode;
use silico_core::document::suppression;
use silico_core::document::{Checked, Document, FeatureId, read, validate, write};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Model, Recompute};

fn text_of(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

fn document(features: &[&str]) -> Document {
    let text = text_of(features);
    read(text.as_bytes()).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn checked(document: &Document) -> Checked {
    Checked::new(document.clone()).unwrap_or_else(|e| panic!("{e:?}"))
}

fn ids(list: &[&str]) -> Vec<FeatureId> {
    list.iter().map(|i| id(i)).collect()
}

fn a_box(id: &str, at: u32, size: u32) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "B", "position": [{at}, {at}, {at}], "length": {size}, "width": {size}, "height": {size}}}"#
    )
}

fn chamfer(id: &str, base: &str, edge: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "chamfer", "v": 1, "name": "C", "base": "{base}", "edges": ["{edge}"], "size": {{"distance": 0.5}}}}"#
    )
}

fn boolean(id: &str, kind: &str, base: &str, tool: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "{kind}", "v": 1, "name": "O", "base": "{base}", "tool": "{tool}"}}"#
    )
}

/// The errors of `model`, each (feature, code), and every solid-making feature that isn't
/// suppressed but didn't build, with its outcome.
fn troubles(document: &Document, model: &Model) -> Vec<String> {
    let mut out: Vec<String> = model
        .errors()
        .map(|e| format!("{:?} {:?}: {}", e.feature(), e.code(), e.message()))
        .collect();
    for f in &document.features {
        if f.suppressed || !f.operation.makes_solid() {
            continue;
        }
        match model.outcome(&f.id) {
            Some(FeatureOutcome::Built { .. }) => {}
            other => out.push(format!("{} didn't build: {other:?}", f.id)),
        }
    }
    out
}

fn rebuilt(document: &Document) -> std::sync::Arc<Model> {
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    published(&engine, checked(document))
}

// ---------------------------------------------------------------------------------------
// A name that reaches its solid through a suppressed boolean's tool.

/// A box a, a box t overlapping it, their fuse m, and a chamfer c of the fuse naming one of
/// t's edges, which the fuse keeps under t's own name (the fake's map of the fuse:
/// `t0000001.box.Edge1` .. `Edge12` beside `b0000001.box.Edge1` ..).
fn fused_with_a_tool_edge_chamfered() -> Document {
    let a = a_box("b0000001", 0, 10);
    let t = a_box("t0000001", 5, 10);
    let m = boolean("m0000001", "fuse", "b0000001", "t0000001");
    let c = chamfer("c0000001", "m0000001", "t0000001.box.Edge5");
    document(&[&a, &t, &m, &c])
}

#[test]
fn a_chamfer_of_a_fuses_tool_edge_builds_with_nothing_suppressed() {
    // The control: unsuppressed, the chamfer finds t's edge in the fuse and builds.
    let d = fused_with_a_tool_edge_chamfered();
    validate(&d).unwrap();
    let model = rebuilt(&d);
    assert_eq!(troubles(&d, &model), Vec::<String>::new());
}

#[test]
fn suppressing_a_fuse_takes_the_chamfer_of_its_tools_edge_with_it() {
    // BREAKS: the fuse suppressed passes its base, a, through, and the chamfer builds on a,
    // which has no t0000001.box.Edge5: the chamfer fails `reference_broken`. Its name's
    // maker is t, which isn't suppressed, so none of the three rules takes it, and
    // `suppress_feature` hands back a document whose rebuild fails because of the
    // suppression, which the port note says can't happen ("A feature that can't build is
    // suppressed, not left failing"). FreeCAD's fillet of a suppressed feature's element
    // fails the same way; Silico's departure is meant to suppress it.
    let d = fused_with_a_tool_edge_chamfered();
    let done = suppression::suppress(&checked(&d), &id("m0000001")).unwrap();
    validate(&done.document).unwrap();
    let model = rebuilt(&done.document);
    assert_eq!(
        troubles(&done.document, &model),
        Vec::<String>::new(),
        "suppressed {:?}",
        done.suppressed
    );
    assert_eq!(done.suppressed, ids(&["m0000001", "c0000001"]));
}

#[test]
fn the_gate_refuses_a_fuse_suppressed_by_hand_under_a_chamfer_of_its_tools_edge() {
    // BREAKS: the same, by hand or by a merge: the gate takes a document whose chamfer can
    // only fail, because the fuse it reaches t's edge through is suppressed.
    let mut d = fused_with_a_tool_edge_chamfered();
    d.features[2].suppressed = true;
    let refused = validate(&d);
    assert!(
        refused.is_err(),
        "the gate took it; rebuilt: {:?}",
        troubles(&d, &rebuilt(&d))
    );
    // Naming both, in a lost name's words: the chamfer at its edge, the fuse whose tool
    // the edge's maker reaches it through.
    let refused = refused.unwrap_err();
    assert_eq!(refused.total(), 1);
    let error = &refused.errors()[0];
    assert_eq!(
        (error.feature(), error.parameter(), error.message()),
        (
            Some("c0000001"),
            Some("edges.0"),
            "c0000001 isn't suppressed, but edges.0 names an element of t0000001, which reaches its base only as m0000001's tool, and m0000001 is suppressed: a feature that depends on a suppressed one is suppressed with it"
        )
    );
}

#[test]
fn unsuppressing_the_chamfer_of_a_fuses_tool_edge_gives_back_the_fuse() {
    // BREAKS: both suppressed, unsuppressing the chamfer gives back the makers of its name
    // (t, unsuppressed already) and the root of a needed chain with nothing unsuppressed
    // (the fuse's chain has a): so the fuse stays suppressed and the chamfer, built on a,
    // fails `reference_broken`.
    let mut d = fused_with_a_tool_edge_chamfered();
    d.features[2].suppressed = true;
    d.features[3].suppressed = true;
    let done = suppression::unsuppress(&checked(&d), &id("c0000001")).unwrap();
    validate(&done.document).unwrap();
    assert_eq!(
        troubles(&done.document, &rebuilt(&done.document)),
        Vec::<String>::new(),
        "unsuppressed {:?}",
        done.unsuppressed
    );
    // The fuse given back with it, and nothing else suppressed.
    assert_eq!(done.unsuppressed, ids(&["m0000001", "c0000001"]));
    assert!(!done.document.features[2].suppressed);
}

/// Boxes a, a2, and t2; a fuse j of a2 and t2; a fuse m of a and (as its tool) j; and a
/// chamfer c on m naming t2's edge, which both fuses keep under t2's name. Nothing with a
/// tool is suppressed until a cascade suppresses one.
fn fused_twice() -> Document {
    let a = a_box("a0000001", 0, 10);
    let a2 = a_box("a0000002", 5, 10);
    let t2 = a_box("t0000002", 8, 10);
    let j = boolean("j0000002", "fuse", "a0000002", "t0000002");
    let m = boolean("m0000001", "fuse", "a0000001", "j0000002");
    let c = chamfer("c0000001", "m0000001", "t0000002.box.Edge5");
    document(&[&a, &a2, &t2, &j, &m, &c])
}

#[test]
fn a_boolean_the_cascade_suppresses_loses_its_tools_names_too() {
    // The tests review's: suppressing a2 takes j (its base gone, with nothing up its
    // chain), then m (its tool's chain all suppressed); neither is suppressed in the text,
    // and c's name of t2's edge reaches m only through j's and m's tools: lost, so c goes.
    let d = fused_twice();
    validate(&d).unwrap();
    assert_eq!(
        troubles(&d, &rebuilt(&d)),
        Vec::<String>::new(),
        "the control"
    );
    let done = suppression::suppress(&checked(&d), &id("a0000002")).unwrap();
    assert_eq!(
        done.suppressed,
        ids(&["a0000002", "j0000002", "m0000001", "c0000001"])
    );
    validate(&done.document).unwrap();
    assert_eq!(
        troubles(&done.document, &rebuilt(&done.document)),
        Vec::<String>::new()
    );
    // Unsuppressing c gives every one of them back.
    let back = suppression::unsuppress(&checked(&done.document), &id("c0000001")).unwrap();
    assert_eq!(
        back.unsuppressed,
        ids(&["a0000002", "j0000002", "m0000001", "c0000001"])
    );
    assert!(back.document.canonically_eq(&d));
}

#[test]
fn the_gate_refuses_a_cascades_lost_name_in_one_check_naming_the_nearest_boolean() {
    // Only a2 suppressed by hand: j, m, and c refused at once; c's at edges.0, naming j,
    // the boolean nearest t2 whose tool it passes, as having to go with a2.
    let mut d = fused_twice();
    d.features[1].suppressed = true;
    let refused = validate(&d).unwrap_err();
    let said: Vec<(&str, &str)> = refused
        .errors()
        .iter()
        .map(|e| (e.feature().unwrap(), e.parameter().unwrap()))
        .collect();
    assert_eq!(
        said,
        [
            ("j0000002", "base"),
            ("m0000001", "tool"),
            ("c0000001", "edges.0")
        ]
    );
    assert_eq!(
        refused.errors()[2].message(),
        "c0000001 isn't suppressed, but edges.0 names an element of t0000002, which reaches its base only as j0000002's tool, and j0000002 has to be suppressed with a0000002: a feature that depends on a suppressed one is suppressed with it"
    );
}

#[test]
fn suppressing_a_fuse_with_a_chamfer_of_its_bases_edge_leaves_the_chamfer_built_on_the_base() {
    // The other side: a chamfer of a's edge through the suppressed fuse builds on a, and
    // its solid is the chamfer of a itself (the fake says how it was made).
    let a = a_box("b0000001", 0, 10);
    let t = a_box("t0000001", 5, 10);
    let m = boolean("m0000001", "fuse", "b0000001", "t0000001");
    let c = chamfer("c0000001", "m0000001", "b0000001.box.Edge5");
    let d = document(&[&a, &t, &m, &c]);
    let done = suppression::suppress(&checked(&d), &id("m0000001")).unwrap();
    assert_eq!(done.suppressed, ids(&["m0000001"]));
    let model = rebuilt(&done.document);
    assert_eq!(troubles(&done.document, &model), Vec::<String>::new());
    // t is no longer consumed: a body again, beside the chamfer.
    assert_eq!(model.bodies(), ids(&["t0000001", "c0000001"]));
    let direct = document(&[&a, &chamfer("c0000001", "b0000001", "b0000001.box.Edge5")]);
    assert_eq!(
        super::model::described(&model, "c0000001"),
        super::model::described(&rebuilt(&direct), "c0000001")
    );
}

// ---------------------------------------------------------------------------------------
// The reader's `suppressed`, in every wrong shape and place.

fn read_err(text: &str) -> silico_core::Error {
    read(text.as_bytes()).unwrap_err()
}

#[test]
fn suppressed_written_twice_is_refused_naming_it() {
    let text = text_of(&[
        r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "suppressed": true, "suppressed": true, "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#,
    ]);
    // The JSON reader's refusal of any key written twice: placed, and naming the key.
    let refused = read_err(&text);
    assert_eq!(refused.code(), ErrorCode::InvalidInput, "{refused}");
    assert!(
        refused
            .message()
            .contains(r#"the key "suppressed" is written twice"#),
        "{refused}"
    );
    assert!(refused.message().contains("column 164"), "{refused}");
}

#[test]
fn suppressed_true_then_false_is_refused() {
    let text = text_of(&[
        r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "suppressed": true, "suppressed": false, "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#,
    ]);
    let refused = read_err(&text);
    assert!(
        refused
            .message()
            .contains(r#"the key "suppressed" is written twice"#),
        "{refused}"
    );
}

#[test]
fn suppressed_in_a_sketch_member_or_a_nested_object_is_an_unknown_member() {
    // On a sketch's point, in a pad's extent, and in a fingerprint: none of them is a
    // feature, so none takes `suppressed`.
    for text in [
        text_of(&[
            r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false, "suppressed": true}], "lines": [], "circles": [], "arcs": [], "constraints": []}"#,
        ]),
        text_of(&[
            r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}"#,
            r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "P", "sketch": "s0000001", "extent": {"type": "one_side", "length": 1, "reversed": false, "suppressed": true}}"#,
        ]),
        text_of(&[
            r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1, "fingerprint": {"volume": 1, "area": 6, "faces": 6, "edges": 12, "vertices": 8, "suppressed": true}}"#,
        ]),
    ] {
        let refused = read_err(&text);
        assert!(refused.message().contains("suppressed"), "{refused}");
    }
}

#[test]
fn suppressed_on_the_document_or_its_header_is_refused() {
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "suppressed": true, "features": []}"#;
    let refused = read_err(text);
    assert!(refused.message().contains("suppressed"), "{refused}");
}

#[test]
fn suppressed_in_every_wrong_kind_is_refused_naming_it_and_the_feature() {
    for other in [
        "null",
        "0",
        "1",
        "1.0",
        "-0",
        r#""true""#,
        r#""""#,
        "[]",
        "[true]",
        "{}",
        "1e0",
    ] {
        let text = text_of(&[&format!(
            r#"{{"id": "b0000001", "type": "box", "v": 1, "name": "B", "suppressed": {other}, "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}"#
        )]);
        let refused = read_err(&text);
        assert_eq!(
            refused.parameter(),
            Some("suppressed"),
            "{other}: {refused}"
        );
        assert_eq!(refused.feature(), Some("b0000001"), "{other}: {refused}");
        assert_eq!(refused.code(), ErrorCode::InvalidInput, "{other}");
    }
}

#[test]
fn suppressed_anywhere_in_the_object_is_written_after_the_name_byte_for_byte() {
    // Before `id`, after `type`, and last: each read the same, and written in the one
    // canonical place.
    let canonical = text_of(&[
        r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "suppressed": true, "position": [0, 0, 0], "length": 1, "width": 1, "height": 1, "note": "N"}"#,
    ]);
    let want = write(&read(canonical.as_bytes()).unwrap()).unwrap();
    for text in [
        r#"{"suppressed": true, "id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1, "note": "N"}"#,
        r#"{"id": "b0000001", "type": "box", "suppressed": true, "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1, "note": "N"}"#,
        r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1, "note": "N", "suppressed": true}"#,
    ] {
        let got = write(&read(text_of(&[text]).as_bytes()).unwrap()).unwrap();
        assert_eq!(got, want, "{text}");
    }
    // And the canonical form is a fixed point.
    assert_eq!(write(&read(want.as_bytes()).unwrap()).unwrap(), want);
}

// ---------------------------------------------------------------------------------------
// A suppressed feature's own values and references.

#[test]
fn a_suppressed_chamfer_naming_an_element_that_doesnt_exist_rebuilds_with_no_error() {
    // Not built, so its names aren't looked for: no reference_broken from a feature that
    // isn't there.
    let b = a_box("b0000001", 0, 10);
    let c = r#"{"id": "c0000001", "type": "chamfer", "v": 1, "name": "C", "suppressed": true, "base": "b0000001", "edges": ["b0000001.box.Edge99"], "size": {"distance": 0.5}}"#;
    let d = document(&[&b, c]);
    validate(&d).unwrap();
    let model = rebuilt(&d);
    assert_eq!(troubles(&d, &model), Vec::<String>::new());
    assert_eq!(
        model.outcome(&id("c0000001")),
        Some(&FeatureOutcome::Suppressed)
    );
    assert_eq!(model.bodies(), ids(&["b0000001"]));
    // Unsuppressed, the same document fails loudly, naming it: the control.
    let mut on = d.clone();
    on.features[1].suppressed = false;
    let model = rebuilt(&on);
    assert!(
        model
            .errors()
            .any(|e| e.code() == ErrorCode::ReferenceBroken && e.feature() == Some("c0000001")),
        "{:?}",
        troubles(&on, &model)
    );
}

#[test]
fn a_suppressed_pocket_whose_sketch_has_no_profile_rebuilds_with_no_error() {
    // The bracket's first holes' sketch emptied of its circles, with its pocket and the
    // sketch suppressed: nothing of either is solved or built.
    let empty = r#"{"id": "s0000002", "type": "sketch", "v": 1, "name": "Base holes", "suppressed": true, "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}"#;
    let pocket = H0000001.replace(
        r#""name": "Base holes", "#,
        r#""name": "Base holes", "suppressed": true, "#,
    );
    let d = document(&[
        S0000001, P0000001, empty, &pocket, S0000003, H0000002, F0000001,
    ]);
    validate(&d).unwrap();
    let model = rebuilt(&d);
    assert_eq!(troubles(&d, &model), Vec::<String>::new());
    for f in ["s0000002", "h0000001"] {
        assert_eq!(
            model.outcome(&id(f)),
            Some(&FeatureOutcome::Suppressed),
            "{f}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Edits that leave nothing changed, and errors that leave nothing broken.

#[test]
fn suppressing_a_sketch_used_by_nothing_takes_it_alone_and_nothing_rebuilds_differently() {
    let lone = r#"{"id": "s0000009", "type": "sketch", "v": 1, "name": "Lone", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}"#;
    let d = document(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001, lone,
    ]);
    let done = suppression::suppress(&checked(&d), &id("s0000009")).unwrap();
    assert_eq!(done.suppressed, ids(&["s0000009"]));
    validate(&done.document).unwrap();
    let (a, b) = (rebuilt(&d), rebuilt(&done.document));
    assert_eq!(a.bodies(), b.bodies());
    for f in ["p0000001", "h0000001", "h0000002", "f0000001"] {
        assert_eq!(
            super::model::described(&a, f),
            super::model::described(&b, f),
            "{f}"
        );
    }
}

#[test]
fn a_failed_suppression_leaves_the_checked_document_as_it_was_and_the_next_one_works() {
    let d = document(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001,
    ]);
    let c = checked(&d);
    let before = write(c.document()).unwrap();
    // An ID no feature has, one of the wrong shape's neighbour, and the empty-ish ones.
    for unknown in ["z0000001", "s0000004", "p0000002"] {
        let refused = suppression::suppress(&c, &id(unknown)).unwrap_err();
        assert_eq!(refused.parameter(), Some("feature"), "{unknown}");
        assert!(refused.message().contains(unknown), "{unknown}");
        let refused = suppression::unsuppress(&c, &id(unknown)).unwrap_err();
        assert_eq!(refused.parameter(), Some("feature"), "{unknown}");
    }
    assert_eq!(write(c.document()).unwrap(), before);
    let done = suppression::suppress(&c, &id("h0000001")).unwrap();
    assert_eq!(done.suppressed, ids(&["h0000001"]));
}

// ---------------------------------------------------------------------------------------
// Long chains: a suppression and the gate stay prompt.

#[test]
fn a_chain_of_two_thousand_chamfers_suppresses_and_unsuppresses_promptly() {
    // A box and 2,000 chamfers, each on the one before, each of the box's edge 1 + i % 12
    // (the fake doesn't check one is left; only the rules and the gate run here).
    let n = 2000;
    let mut features = vec![a_box("b0000001", 0, 10)];
    for i in 0..n {
        let base = if i == 0 {
            "b0000001".to_owned()
        } else {
            format!("c{:07}", i - 1)
        };
        features.push(chamfer(
            &format!("c{i:07}"),
            &base,
            &format!("b0000001.box.Edge{}", 1 + i % 12),
        ));
    }
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    let d = document(&refs);
    let start = Instant::now();
    let c = checked(&d);
    let all = suppression::suppress(&c, &id("b0000001")).unwrap();
    assert_eq!(all.suppressed.len(), n + 1);
    validate(&all.document).unwrap();
    // The last unsuppressed: the box (its name's maker, and its chain's root) and itself.
    let back =
        suppression::unsuppress(&checked(&all.document), &id(&format!("c{:07}", n - 1))).unwrap();
    assert_eq!(
        back.unsuppressed,
        ids(&["b0000001", &format!("c{:07}", n - 1)])
    );
    validate(&back.document).unwrap();
    // A thousand-odd middles suppressed and the gate run on each: each the middle alone.
    let mid = suppression::suppress(&c, &id("c0001000")).unwrap();
    assert_eq!(mid.suppressed, ids(&["c0001000"]));
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );
}

/// The bound on the long chain's CPU over the probe's, for each of its suppression, its gate
/// pass, and its unsuppressions, from readings on the Linux VM. The step-by-step walk read
/// 35.7, 35.7, and 19.95 (a slow probe before it) for the suppression and the gate together
/// (with each chamfer naming a's edge alone), and 56.2 and 65.2 for the unsuppression of the
/// chain. The consumer forest reads 1.4 to 2.6 for the suppression and the gate, 3.0 to 3.1
/// for the chain's unsuppression, and 0.3 to 0.4 for the fuse's. About 2.5 times from the
/// worst of these and from the walk's least.
const BOUND: f64 = 8.0;

/// The CPU `work` takes over the probe's (`cpu_work`, #292: what a fixed piece of work costs
/// this machine now, the slower of before and after, so a loaded runner doesn't fail it),
/// held under [`BOUND`], and what it gave.
fn within_bound<T>(what: &str, work: impl FnOnce() -> T) -> T {
    let before = cpu_work::cost();
    let cpu = cpu_time::ThreadTime::now();
    let given = work();
    let took = cpu.elapsed();
    let after = cpu_work::cost();
    let probe = before.max(after);
    let ratio = took.as_secs_f64() / probe.as_secs_f64();
    let _ = writeln!(
        std::io::stderr(),
        "{what} took {took:?} of CPU, {ratio:.2} times the probe's {probe:?} (before \
         {before:?}, after {after:?})"
    );
    assert!(
        ratio < BOUND,
        "{what} took {took:?} of CPU, {ratio:.2} times the probe's {probe:?}, past {BOUND}"
    );
    given
}

#[test]
fn a_long_chain_after_a_suppressed_fuse_is_judged_in_linear_work() {
    // Boxes a and t, their fuse m, and 20,000 chamfers c, each on the one before (the first
    // on m), each of a's edge 1 + i % 12 and, but the first, of the edge the chamfer before
    // it made of a's edge 1, and a last, z, naming t's edge 1. Each chamfer's name of a's edge is walked down
    // the chain to its base, wherever a boolean that consumes a tool is suppressed: a walk
    // step by step costs n^2 / 2 steps, about 2 * 10^8, at each of the suppression's and the
    // gate's passes, and at the unsuppression's (issue #306).
    let n = 20_000;
    let mut features = vec![
        a_box("b0000001", 0, 10),
        a_box("t0000001", 5, 10),
        boolean("m0000001", "fuse", "b0000001", "t0000001"),
    ];
    let mut base = "m0000001".to_owned();
    for i in 0..n {
        let c = format!("c{i:07}");
        let mut edges = format!(r#""b0000001.box.Edge{}""#, 1 + i % 12);
        if i > 0 {
            edges.push_str(&format!(r#", "{base}.chamfer.Edge(G b0000001.box.Edge1)""#));
        }
        features.push(format!(
            r#"{{"id": "{c}", "type": "chamfer", "v": 1, "name": "C", "base": "{base}", "edges": [{edges}], "size": {{"distance": 0.5}}}}"#
        ));
        base = c;
    }
    let last = base.clone();
    features.push(chamfer("z0000001", &base, "t0000001.box.Edge1"));
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    let c = checked(&document(&refs));
    let chamfers: Vec<FeatureId> = (0..n).map(|i| id(&format!("c{i:07}"))).collect();

    // Suppressing m, by hand: every chamfer's name of a's edge reaches its base through m as
    // m's base, which m passes through, and its name of the chamfer before's edge is of its
    // base itself, so it stays; z's t reaches its base only as m's tool, so z goes with m.
    let done = within_bound("the suppression of the fuse", || {
        suppression::suppress(&c, &id("m0000001")).unwrap()
    });
    within_bound("the gate", || validate(&done.document).unwrap());
    assert_eq!(done.suppressed, ids(&["m0000001", "z0000001"]));

    // The first chamfer suppressed too, and so every chamfer after it, each naming an edge of
    // the one before.
    let all = suppression::suppress(&checked(&done.document), &chamfers[0]).unwrap();
    assert_eq!(all.suppressed, chamfers);

    // Unsuppressing the last chamfer brings back each chamfer before it, the maker of a name
    // it stores, and each is looked at for a name lost to m (a's way to its base passes m as
    // its base, so none is); m, which none of them needs, and z, which depends on m, stay
    // suppressed. Then unsuppressing m brings back m alone.
    let all = checked(&all.document);
    let back = within_bound("the unsuppression of the chain", || {
        suppression::unsuppress(&all, &id(&last)).unwrap()
    });
    assert_eq!(back.unsuppressed, chamfers);
    let back = checked(&back.document);
    let fuse = within_bound("the unsuppression of the fuse", || {
        suppression::unsuppress(&back, &id("m0000001")).unwrap()
    });
    validate(&fuse.document).unwrap();
    assert_eq!(fuse.unsuppressed, ids(&["m0000001"]));
}

// ---------------------------------------------------------------------------------------
// Random documents, random suppressions.

/// One step of a random document: a box; a chamfer of an unconsumed solid, naming an edge
/// of one of the boxes it's made from; or a fuse or cut of two unconsumed solids.
#[derive(Debug, Clone)]
enum Make {
    Box,
    Chamfer { solid: usize, edge: usize },
    Boolean { cut: bool, base: usize, tool: usize },
}

fn makes() -> impl Strategy<Value = Vec<Make>> {
    prop::collection::vec(
        prop_oneof![
            3 => Just(Make::Box),
            4 => (any::<usize>(), any::<usize>()).prop_map(|(solid, edge)| Make::Chamfer { solid, edge }),
            3 => (any::<bool>(), any::<usize>(), any::<usize>())
                .prop_map(|(cut, base, tool)| Make::Boolean { cut, base, tool }),
        ],
        2..10,
    )
}

/// The document `makes` give: each chamfer of an edge no other chamfer names (so each name
/// is found, unsuppressed), each step that can't be made (no solid to chamfer, fewer than
/// two to combine) skipped.
fn made(makes: &[Make]) -> Document {
    // Each unconsumed solid's ID, with the boxes it's made from.
    let mut open: Vec<(String, Vec<String>)> = Vec::new();
    let mut named: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut features: Vec<String> = Vec::new();
    let mut boxes = 0;
    for (i, make) in makes.iter().enumerate() {
        let fid = format!("f{i:07}");
        match make {
            Make::Box => {
                let bid = format!("b{boxes:07}");
                boxes += 1;
                features.push(a_box(&bid, boxes, 10));
                open.push((bid.clone(), vec![bid]));
            }
            Make::Chamfer { solid, edge } => {
                if open.is_empty() {
                    continue;
                }
                let at = solid % open.len();
                let (base, from) = open[at].clone();
                let edges: Vec<String> = from
                    .iter()
                    .flat_map(|b| (1..=12).map(move |n| format!("{b}.box.Edge{n}")))
                    .filter(|e| !named.contains(e))
                    .collect();
                if edges.is_empty() {
                    continue;
                }
                let e = edges[edge % edges.len()].clone();
                named.insert(e.clone());
                features.push(chamfer(&fid, &base, &e));
                open[at] = (fid, from);
            }
            Make::Boolean { cut, base, tool } => {
                if open.len() < 2 {
                    continue;
                }
                let b = base % open.len();
                let (bid, from) = open.remove(b);
                let t = tool % open.len();
                let (tid, tfrom) = open.remove(t);
                let kind = if *cut { "cut" } else { "fuse" };
                // Only the base's boxes are named after it: a name of a fuse's tool's edge,
                // with the fuse suppressed, is the break the targeted tests above hold
                // (`suppressing_a_fuse_takes_the_chamfer_of_its_tools_edge_with_it`), left
                // out here so the property can find others.
                let _ = tfrom;
                features.push(boolean(&fid, kind, &bid, &tid));
                open.push((fid, from));
            }
        }
    }
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    document(&refs)
}

/// A suppression or an unsuppression, of the feature at a place.
#[derive(Debug, Clone)]
struct Toggle {
    at: usize,
    suppress: bool,
}

fn toggles() -> impl Strategy<Value = Vec<Toggle>> {
    prop::collection::vec(
        (any::<usize>(), any::<bool>()).prop_map(|(at, suppress)| Toggle { at, suppress }),
        1..4,
    )
}

fn flags(document: &Document) -> Vec<bool> {
    document.features.iter().map(|f| f.suppressed).collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    /// From a document that rebuilds clean, every suppression and unsuppression gives a
    /// document the gate takes, that rebuilds with no error, with the feature asked for as
    /// asked; and a suppression is undone exactly by unsuppressing what it took, in
    /// timeline order.
    #[test]
    fn every_suppression_of_a_clean_document_rebuilds_clean_and_is_undone(
        makes in makes(),
        toggles in toggles(),
    ) {
        let mut d = made(&makes);
        prop_assume!(!d.features.is_empty());
        let clean = troubles(&d, &rebuilt(&d));
        prop_assume!(clean.is_empty());
        for toggle in &toggles {
            let at = toggle.at % d.features.len();
            let fid = d.features[at].id.clone();
            let c = checked(&d);
            let done = suppression::set_suppressed(&c, &fid, toggle.suppress).unwrap();
            // The same again: the same document and lists.
            let again = suppression::set_suppressed(&c, &fid, toggle.suppress).unwrap();
            prop_assert!(again.document.canonically_eq(&done.document));
            prop_assert_eq!(&again.suppressed, &done.suppressed);
            prop_assert_eq!(&again.unsuppressed, &done.unsuppressed);
            prop_assert_eq!(done.document.features[at].suppressed, toggle.suppress);
            // Only the listed features changed, and each as listed.
            for (i, (was, now)) in flags(&d).iter().zip(flags(&done.document)).enumerate() {
                let f = &d.features[i].id;
                let listed = done.suppressed.contains(f) || done.unsuppressed.contains(f);
                prop_assert_eq!(*was != now, listed, "{} {:?}", f, toggle);
            }
            if let Err(e) = validate(&done.document) {
                return Err(TestCaseError::fail(format!(
                    "the gate refused {:?} of {fid}: {:?}\n{}",
                    toggle, e, write(&d).unwrap()
                )));
            }
            let trouble = troubles(&done.document, &rebuilt(&done.document));
            prop_assert!(
                trouble.is_empty(),
                "{:?} of {} gave {:?}\nfrom {}",
                toggle, fid, trouble, write(&d).unwrap()
            );
            if toggle.suppress && done.changed() {
                let mut back = done.document.clone();
                for f in &done.suppressed {
                    back = suppression::unsuppress(&checked(&back), f).unwrap().document;
                }
                prop_assert!(
                    back.canonically_eq(&d),
                    "suppressing {} took {:?}; unsuppressing them left {:?}, not {:?}",
                    fid, done.suppressed, flags(&back), flags(&d)
                );
            }
            d = done.document;
        }
    }
}

/// The bracket with a fillet of the first holes' rim on its fillet (the design's case): a
/// dependent by its name of the pocket and of the holes' sketch.
const RIM: &str = r#"{"id": "f0000002", "type": "fillet", "v": 1, "name": "Rim", "base": "f0000001", "edges": ["h0000001.pocket.Edge(M s0000002.sketch.Edge.cb000001)"], "radius": 0.5}"#;

fn rimmed() -> Document {
    document(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001, RIM,
    ])
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    /// The rimmed bracket (sketches, a pad, through-all pockets, and two fillets) put
    /// through random suppressions and unsuppressions: each result the gate takes, rebuilt
    /// with no error, the feature asked for as asked, and the same twice.
    #[test]
    fn every_suppression_of_the_rimmed_bracket_rebuilds_clean(toggles in prop::collection::vec(
        (0usize..8, any::<bool>()), 1..6,
    )) {
        let mut d = rimmed();
        for (at, suppress) in toggles {
            let fid = d.features[at].id.clone();
            let c = checked(&d);
            let done = suppression::set_suppressed(&c, &fid, suppress).unwrap();
            let again = suppression::set_suppressed(&c, &fid, suppress).unwrap();
            prop_assert!(again.document.canonically_eq(&done.document));
            prop_assert_eq!(done.document.features[at].suppressed, suppress);
            prop_assert!(validate(&done.document).is_ok(), "{} {}", fid, suppress);
            let trouble = troubles(&done.document, &rebuilt(&done.document));
            prop_assert!(trouble.is_empty(), "{} {}: {:?}", fid, suppress, trouble);
            d = done.document;
        }
    }
}

#[test]
fn the_timelines_bodies_with_a_fuse_suppressed_are_the_models() {
    // The fuse suppressed: t is no longer consumed, so the document's bodies (the
    // timeline's, judged by the text) and the model's are t and the chamfer of a.
    let a = a_box("b0000001", 0, 10);
    let t = a_box("t0000001", 5, 10);
    let m = boolean("m0000001", "fuse", "b0000001", "t0000001");
    let c = chamfer("c0000001", "m0000001", "b0000001.box.Edge5");
    let d = suppression::suppress(&checked(&document(&[&a, &t, &m, &c])), &id("m0000001"))
        .unwrap()
        .document;
    let model = rebuilt(&d);
    let timeline = silico_core::timeline::timeline(
        &d,
        Some(&model),
        &silico_core::timeline::Excepted::default(),
    );
    assert_eq!(timeline.bodies, ids(&["t0000001", "c0000001"]));
    assert_eq!(model.bodies(), timeline.bodies.as_slice());
    let outline = silico_core::command::outline(&d);
    assert!(
        outline.contains(r#"m0000001 "O" suppressed fuse"#),
        "{outline}"
    );
}

// ---------------------------------------------------------------------------------------
// The session: `set_field` and `add_feature` given `suppressed` in wrong shapes, and the
// session sound after each refusal.

mod session {
    use std::sync::Arc;

    use super::super::bracket::*;
    use super::super::model::id;
    use silico_core::cache::Cache;
    use silico_core::command::{Response, Running, SessionId, Sessions};
    use silico_core::document::json::{Node, read};
    use silico_core::fake::{FakeKernel, FakeSketches};
    use silico_core::sketch::SketchSolver;
    use tempfile::TempDir;

    const HANG: std::time::Duration = std::time::Duration::from_secs(60);

    fn answer<T>(running: Running<T>) -> Response<T> {
        match running.wait_timeout(HANG) {
            Ok(response) => response,
            Err(_) => panic!("the command didn't answer in {HANG:?}"),
        }
    }

    struct Fixture {
        _dir: TempDir,
        sessions: Sessions<Arc<FakeKernel>>,
        id: SessionId,
    }

    fn opened() -> Fixture {
        let text = super::text_of(&[
            S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001,
        ]);
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        let kernel = Arc::new(FakeKernel::new(2).unwrap());
        let sketches = FakeSketches::new();
        let sessions = Sessions::with_stand_ins(
            kernel,
            move |cache| Cache::with_secret(cache, &secret),
            move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
        );
        let path = dir.path().join("bracket.silico");
        std::fs::write(&path, text).unwrap();
        let id = answer(sessions.open(&path, false)).session.unwrap().id;
        answer(sessions.begin(&id)).result.unwrap();
        Fixture {
            _dir: dir,
            sessions,
            id,
        }
    }

    impl Fixture {
        fn revision(&self) -> silico_core::command::Revision {
            self.sessions.status(&self.id).session.unwrap().revision
        }

        fn text(&self) -> String {
            answer(self.sessions.text(&self.id, None))
                .result
                .unwrap()
                .text
        }

        /// Suppressing h0000001 still works, and gives h0000001 alone.
        fn still_works(&self) {
            let done = answer(self.sessions.suppress_feature(
                &self.id,
                self.revision(),
                id("h0000001"),
                true,
            ))
            .result
            .unwrap();
            assert_eq!(done.suppressed, [id("h0000001")]);
        }
    }

    fn node(text: &str) -> Node {
        read(text.as_bytes()).unwrap()
    }

    #[test]
    fn set_field_suppressed_in_a_wrong_kind_is_refused_naming_it_and_leaves_the_document() {
        let f = opened();
        let before = f.text();
        for value in [
            "null",
            "0",
            "1",
            r#""true""#,
            r#""false""#,
            "[]",
            "{}",
            "[true]",
        ] {
            let response = answer(f.sessions.set_field(
                &f.id,
                f.revision(),
                id("h0000001"),
                "suppressed".to_owned(),
                node(value),
            ));
            match &response.result {
                // `null` on a feature that isn't suppressed: nothing to remove.
                Ok(set) if value == "null" => assert!(!set.changed, "{value}"),
                Ok(set) => panic!("{value} taken: {set:?}"),
                Err(problems) => {
                    let e = &problems.errors()[0];
                    assert_eq!(e.feature(), Some("h0000001"), "{value}: {e}");
                    assert!(
                        e.parameter() == Some("suppressed") || e.parameter() == Some("value"),
                        "{value}: {e}"
                    );
                }
            }
            assert_eq!(f.text(), before, "{value}");
        }
        // A path under it is no field of the feature.
        let response = answer(f.sessions.set_field(
            &f.id,
            f.revision(),
            id("h0000001"),
            "suppressed.x".to_owned(),
            node("true"),
        ));
        assert!(response.result.is_err(), "suppressed.x taken");
        assert_eq!(f.text(), before);
        f.still_works();
    }

    #[test]
    fn set_field_suppressed_null_on_a_suppressed_feature_unsuppresses_it_as_false_does() {
        let f = opened();
        let before = f.text();
        f.still_works();
        let set = answer(f.sessions.set_field(
            &f.id,
            f.revision(),
            id("h0000001"),
            "suppressed".to_owned(),
            node("null"),
        ))
        .result
        .unwrap();
        assert!(set.changed);
        // The document as it was (the session writes it canonically once edited).
        let doc = |t: &str| silico_core::document::read(t.as_bytes()).unwrap();
        assert!(doc(&f.text()).canonically_eq(&doc(&before)));
        assert!(!f.text().contains("suppressed"));
    }

    #[test]
    fn add_feature_with_suppressed_false_is_refused_and_with_true_on_a_suppressed_root_taken() {
        let f = opened();
        let before = f.text();
        let added = |feature: &str| {
            answer(
                f.sessions
                    .add_feature(&f.id, f.revision(), None, node(feature)),
            )
        };
        let refused = added(
            r#"{"id": "c0000001", "type": "chamfer", "v": 1, "name": "C", "suppressed": false, "base": "f0000001", "edges": ["p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000004)"], "size": {"distance": 1}}"#,
        );
        let problems = refused.result.as_ref().unwrap_err();
        assert_eq!(problems.errors()[0].parameter(), Some("suppressed"));
        assert_eq!(f.text(), before);
        // With the pad suppressed (and everything after it), a chamfer of the pad's edge on
        // the fillet is taken only suppressed: unsuppressed, the gate refuses it.
        answer(
            f.sessions
                .suppress_feature(&f.id, f.revision(), id("p0000001"), true),
        )
        .result
        .unwrap();
        let chamfer = r#"{"id": "c0000001", "type": "chamfer", "v": 1, "name": "C", "base": "f0000001", "edges": ["p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000004)"], "size": {"distance": 1}}"#;
        let refused = added(chamfer);
        assert!(refused.result.is_err(), "an unsuppressed dependent taken");
        let taken =
            added(&chamfer.replace(r#""name": "C", "#, r#""name": "C", "suppressed": true, "#));
        assert!(taken.result.is_ok(), "{:?}", taken.result);
    }
}

#[test]
fn suppressing_a_cut_takes_the_chamfer_of_an_edge_its_tool_left_in_the_solid() {
    // The cut's side of the fuse's break, which holds: a cut names its tool's edges as its
    // own (`k0000001.cut.Edge(I t0000001.box.Edge5)`, the fake's candidate for the tool's
    // plain name), so the cut is a maker of the chamfer's name and goes with it.
    let a = a_box("b0000001", 0, 10);
    let t = a_box("t0000001", 5, 10);
    let k = boolean("k0000001", "cut", "b0000001", "t0000001");
    let c = chamfer(
        "c0000001",
        "k0000001",
        "k0000001.cut.Edge(I t0000001.box.Edge5)",
    );
    let d = document(&[&a, &t, &k, &c]);
    assert_eq!(
        troubles(&d, &rebuilt(&d)),
        Vec::<String>::new(),
        "the control"
    );
    let done = suppression::suppress(&checked(&d), &id("k0000001")).unwrap();
    assert_eq!(done.suppressed, ids(&["k0000001", "c0000001"]));
    validate(&done.document).unwrap();
    let model = rebuilt(&done.document);
    assert_eq!(troubles(&done.document, &model), Vec::<String>::new());
    assert_eq!(model.bodies(), ids(&["b0000001", "t0000001"]));
    // And unsuppressing the chamfer gives the cut back with it.
    let back = suppression::unsuppress(&checked(&done.document), &id("c0000001")).unwrap();
    assert_eq!(back.unsuppressed, ids(&["k0000001", "c0000001"]));
}

#[test]
fn a_suppressed_feature_whose_own_values_the_gate_refuses_is_still_refused() {
    // Spec probe (reported, not a break): the gate checks a suppressed feature's own
    // values as any other's, so a box of length 0 or a fillet of radius 0 can't be parked
    // by suppressing it. Held here so a change either way is seen.
    let b = r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "suppressed": true, "position": [0, 0, 0], "length": 0, "width": 1, "height": 1}"#;
    let refused = validate(&document(&[b])).unwrap_err();
    assert_eq!(refused.errors()[0].parameter(), Some("length"));
    let base = a_box("b0000001", 0, 10);
    let f = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "F", "suppressed": true, "base": "b0000001", "edges": ["b0000001.box.Edge1"], "radius": 0}"#;
    let refused = validate(&document(&[&base, f])).unwrap_err();
    assert_eq!(refused.errors()[0].parameter(), Some("radius"));
}

#[test]
fn a_request_wrong_in_every_field_is_refused_for_each_naming_it() {
    use silico_core::command::wire::edits::SuppressFeatureRequest;
    let parsed = |text: &str| silico_core::document::json::read(text.as_bytes()).unwrap();
    let refused = SuppressFeatureRequest::read(&parsed(
        r#"{"session": "S1", "revision": 0, "feature": "B0000001", "suppressed": "false"}"#,
    ))
    .unwrap_err();
    let mut parameters: Vec<&str> = refused
        .errors()
        .iter()
        .filter_map(|e| e.parameter())
        .collect();
    parameters.sort_unstable();
    assert_eq!(
        parameters,
        ["feature", "revision", "session", "suppressed"],
        "{:?}",
        refused.errors()
    );
    // A revision past u64, a negative one, and a fraction: each refused, naming it.
    for revision in ["18446744073709551616", "-1", "1.5", "1e3"] {
        let refused = SuppressFeatureRequest::read(&parsed(&format!(
            r#"{{"session": "s1", "revision": {revision}, "feature": "b0000001", "suppressed": true}}"#
        )))
        .unwrap_err();
        assert_eq!(
            refused.errors()[0].parameter(),
            Some("revision"),
            "{revision}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Features that use themselves, which the gate refuses but still reads for suppression.

/// The gate's refusals of `document`, each (feature, parameter, message), from a thread of
/// its own: none if it hasn't answered in 30 seconds (it answers these in microseconds).
fn refused_within_a_while(document: &Document) -> Option<Vec<(String, String, String)>> {
    let (send, receive) = std::sync::mpsc::channel();
    let document = document.clone();
    std::thread::spawn(move || {
        let refusals: Vec<(String, String, String)> = match validate(&document) {
            Ok(()) => Vec::new(),
            Err(invalid) => invalid
                .errors()
                .iter()
                .map(|e| {
                    (
                        e.feature().unwrap_or("").to_owned(),
                        e.parameter().unwrap_or("").to_owned(),
                        e.message().to_owned(),
                    )
                })
                .collect(),
        };
        let _ = send.send(refusals);
    });
    receive.recv_timeout(Duration::from_secs(30)).ok()
}

#[test]
fn the_gate_answers_promptly_where_a_names_maker_is_a_chamfer_of_itself_and_a_cut_is_suppressed() {
    // b, t, a chamfer c whose base is itself, a cut m of b by t (suppressed, so a name may
    // be lost to it and the gate walks each name's makers down to the feature's base), and
    // a fillet f on m naming an edge c made of b's Edge1. By hand: c's base is c, which the
    // gate refuses; c consumes nothing else, and nothing consumes c, so the walk from c, a
    // maker of f's name, stops at once (c never reaches f's base, m); the walk from b, the
    // other maker, reaches m as its base, not its tool, so no name is lost. m passes b
    // through to f, and neither of f's makers is suppressed: f is no dependent. So the one
    // refusal is c's use of itself, given at once.
    let b = a_box("b0000001", 0, 10);
    let t = a_box("t0000001", 5, 10);
    let c = chamfer("c0000001", "c0000001", "b0000001.box.Edge1");
    let m = r#"{"id": "m0000001", "type": "cut", "v": 1, "name": "O", "suppressed": true, "base": "b0000001", "tool": "t0000001"}"#;
    let f = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "F", "base": "m0000001", "edges": ["c0000001.chamfer.Edge(G b0000001.box.Edge1)"], "radius": 0.5}"#;
    let d = document(&[&b, &t, &c, m, f]);
    assert_eq!(
        refused_within_a_while(&d),
        Some(vec![(
            "c0000001".to_owned(),
            "base".to_owned(),
            "base c0000001 is this feature: a feature can't use itself".to_owned()
        )])
    );
}

#[test]
fn a_chamfer_naming_its_own_element_is_refused_once_and_names_no_suppressed_maker() {
    // b, a second box b2 (suppressed, used by nothing), and a chamfer c of b naming an edge
    // of its own made of b2's Edge1. By hand: c can't name its own element (it's no feature
    // before c), so the gate refuses the name, and a name that doesn't read has no makers:
    // b2, suppressed, is no maker of it, so c isn't refused as b2's dependent too. One
    // refusal, of edges.0.
    let b = a_box("b0000001", 0, 10);
    let b2 = r#"{"id": "b0000002", "type": "box", "v": 1, "name": "O", "suppressed": true, "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#;
    let c = chamfer(
        "c0000001",
        "b0000001",
        "c0000001.chamfer.Edge(G b0000002.box.Edge1)",
    );
    let refused = refused_within_a_while(&document(&[&b, b2, &c])).unwrap();
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(
        (refused[0].0.as_str(), refused[0].1.as_str()),
        ("c0000001", "edges.0")
    );
    assert!(
        refused[0].2.contains("no feature c0000001 before this one"),
        "{refused:?}"
    );
}
