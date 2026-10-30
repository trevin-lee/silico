//! Suppression (`document::suppression`: the M5.0 design's `suppress_feature`, A10, and
//! section 12's items 3 and 4; docs/ports/suppression.md): the field read and written, the
//! dependents a suppression takes and an unsuppression gives back on the M4.9 bracket and
//! small documents, each list written out by hand from the design's rules; the gate's
//! refusal of a dependent left unsuppressed, naming both; and the rebuild on the fake kernel,
//! where a suppressed feature is built not at all and what consumes it builds on its
//! effective base, so a document with a feature suppressed builds each solid exactly as the
//! same document with that feature removed and its consumer rewired by the chain (an edit of
//! the text, made by another path: `document::chain::remove`).

use super::bracket::*;
use super::model::{described, made, published};

use silico_core::document::chain;
use silico_core::document::suppression::{self, Suppression};
use silico_core::document::{Checked, Document, FeatureId, read, validate, write};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Recompute};

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn ids(list: &[&str]) -> Vec<FeatureId> {
    list.iter().map(|i| id(i)).collect()
}

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

fn bracket() -> Document {
    document(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001,
    ])
}

fn checked(document: &Document) -> Checked {
    Checked::new(document.clone()).unwrap_or_else(|e| panic!("{e:?}"))
}

/// The IDs of `document`'s features that are suppressed, in timeline order.
fn suppressed_in(document: &Document) -> Vec<FeatureId> {
    document
        .features
        .iter()
        .filter(|f| f.suppressed)
        .map(|f| f.id.clone())
        .collect()
}

/// `document` with the features `list` suppressed, as a hand edit would leave it.
fn with_suppressed(document: &Document, list: &[&str]) -> Document {
    let mut edited = document.clone();
    for f in &mut edited.features {
        if list.contains(&f.id.as_str()) {
            f.suppressed = true;
        }
    }
    edited
}

fn suppress(document: &Document, feature: &str) -> Suppression {
    let done = suppression::suppress(&checked(document), &id(feature)).unwrap();
    validate(&done.document).unwrap_or_else(|e| panic!("{e:?}"));
    done
}

fn unsuppress(document: &Document, feature: &str) -> Suppression {
    let done = suppression::unsuppress(&checked(document), &id(feature)).unwrap();
    validate(&done.document).unwrap_or_else(|e| panic!("{e:?}"));
    done
}

// ---------------------------------------------------------------------------------------
// The field.

#[test]
fn suppressed_true_is_read_and_written_after_the_name_and_false_is_refused_naming_it() {
    let text = text_of(&[
        r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "suppressed": true, "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#,
    ]);
    let read_back = read(text.as_bytes()).unwrap();
    assert!(read_back.features[0].suppressed);
    let written = write(&read_back).unwrap();
    // Right after the name, before the operation's fields: by hand, the canonical line.
    assert!(
        written.contains(
            r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "suppressed": true, "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#
        ),
        "{written}"
    );
    assert_eq!(read(written.as_bytes()).unwrap(), read_back);
    // Anywhere in the object, it's read the same.
    let last = text
        .replace(r#""suppressed": true, "#, "")
        .replace(r#""height": 1}"#, r#""height": 1, "suppressed": true}"#);
    assert_eq!(read(last.as_bytes()).unwrap(), read_back);
    // Left out, it's false, and nothing is written for it.
    let plain = text.replace(r#""suppressed": true, "#, "");
    let unsuppressed = read(plain.as_bytes()).unwrap();
    assert!(!unsuppressed.features[0].suppressed);
    assert!(!write(&unsuppressed).unwrap().contains("suppressed"));

    // `false` is refused, naming the field: one spelling (decision 4).
    let refused = read(text.replace("true", "false").as_bytes()).unwrap_err();
    assert_eq!(refused.parameter(), Some("suppressed"));
    assert_eq!(refused.feature(), Some("b0000001"));
    assert!(
        refused.message().starts_with(
            "suppressed is false: a feature that isn't suppressed leaves suppressed out"
        ),
        "{}",
        refused.message()
    );
    // Anything else but true is the wrong kind.
    for other in ["null", "1", r#""true""#, "[]"] {
        let refused = read(text.replace("true", other).as_bytes()).unwrap_err();
        assert_eq!(refused.parameter(), Some("suppressed"), "{other}");
    }
}

#[test]
fn the_schema_takes_suppressed_as_true_only_and_never_requires_it() {
    let schema: serde_json::Value =
        serde_json::from_str(&silico_core::document::schema().unwrap()).unwrap();
    let feature = &schema["$defs"]["Feature"];
    assert_eq!(feature["properties"]["suppressed"]["const"], true);
    let required: Vec<&str> = feature["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap())
        .collect();
    assert_eq!(required, ["id", "v", "name"]);
    // Its members: the six the format has, `suppressed` the one new.
    let mut keys: Vec<&String> = feature["properties"].as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["fingerprint", "id", "name", "note", "suppressed", "v"]
    );
}

// ---------------------------------------------------------------------------------------
// What a suppression takes, and an unsuppression gives back (the design's lists, by hand).

#[test]
fn suppressing_the_first_hole_suppresses_it_alone_and_leaves_the_part() {
    // h0000002 builds on p0000001 through it; the fillet names the pad's edge, not the
    // hole's: nothing else depends on it (decision 3).
    let done = suppress(&bracket(), "h0000001");
    assert_eq!(done.suppressed, ids(&["h0000001"]));
    assert!(done.unsuppressed.is_empty() && done.changed());
    assert_eq!(suppressed_in(&done.document), ids(&["h0000001"]));
    // The text's links are as written: h0000002's base is still h0000001.
    assert_eq!(
        done.document.features[5].operation.inputs(),
        [("base", &id("h0000001"))]
    );
}

#[test]
fn each_of_the_brackets_suppressions_takes_the_dependents_the_rules_give() {
    // Each by hand from the rules: a sketch's users, a name's makers, and a needed base's
    // chain with nothing unsuppressed.
    for (feature, want) in [
        ("h0000001", &["h0000001"][..]),
        ("h0000002", &["h0000002"][..]),
        ("f0000001", &["f0000001"][..]),
        // h0000001's sketch.
        ("s0000002", &["s0000002", "h0000001"][..]),
        ("s0000003", &["s0000003", "h0000002"][..]),
        // The root: each pocket and the fillet need a base, and every feature up their
        // chains is suppressed; the fillet names the pad's edge too.
        (
            "p0000001",
            &["p0000001", "h0000001", "h0000002", "f0000001"][..],
        ),
        // The pad's sketch, and so the pad and all after it but the other sketches.
        (
            "s0000001",
            &["s0000001", "p0000001", "h0000001", "h0000002", "f0000001"][..],
        ),
    ] {
        let done = suppress(&bracket(), feature);
        assert_eq!(done.suppressed, ids(want), "{feature}");
        assert_eq!(suppressed_in(&done.document), ids(want), "{feature}");
    }
}

#[test]
fn suppressing_a_feature_suppressed_already_or_unsuppressing_one_that_isnt_changes_nothing() {
    let suppressed = with_suppressed(&bracket(), &["h0000001"]);
    let again = suppress(&suppressed, "h0000001");
    assert!(!again.changed());
    assert!(again.document.canonically_eq(&suppressed));
    let not = unsuppress(&bracket(), "h0000002");
    assert!(!not.changed());
    assert!(not.document.canonically_eq(&bracket()));
    // Suppressing a feature whose dependent is suppressed already lists only the new ones.
    let done = suppress(&suppressed, "s0000002");
    assert_eq!(done.suppressed, ids(&["s0000002"]));
}

#[test]
fn unsuppressing_a_child_unsuppresses_its_parents_and_unsuppressing_the_parent_leaves_the_children()
{
    // s0000002 suppressed takes h0000001; unsuppressing h0000001 gives back its sketch.
    let sketch_off = suppress(&bracket(), "s0000002").document;
    let back = unsuppress(&sketch_off, "h0000001");
    assert_eq!(back.unsuppressed, ids(&["s0000002", "h0000001"]));
    assert!(back.document.canonically_eq(&bracket()));
    // Unsuppressing the sketch leaves h0000001 suppressed (SolidWorks' parent).
    let parent = unsuppress(&sketch_off, "s0000002");
    assert_eq!(parent.unsuppressed, ids(&["s0000002"]));
    assert_eq!(suppressed_in(&parent.document), ids(&["h0000001"]));

    // p0000001 suppressed takes every solid; unsuppressing the fillet gives back the pad
    // (a maker of its name, and its chain's root), and only it: the pockets pass it
    // through, so the fillet builds on the pad (the design: [p0000001, f0000001]).
    let root_off = suppress(&bracket(), "p0000001").document;
    let back = unsuppress(&root_off, "f0000001");
    assert_eq!(back.unsuppressed, ids(&["p0000001", "f0000001"]));
    assert_eq!(
        suppressed_in(&back.document),
        ids(&["h0000001", "h0000002"])
    );
    // A pocket unsuppressed gives back its chain's root, the pad.
    let back = unsuppress(&root_off, "h0000002");
    assert_eq!(back.unsuppressed, ids(&["p0000001", "h0000002"]));
    // Unsuppressing the pad leaves what depends on it suppressed.
    let back = unsuppress(&root_off, "p0000001");
    assert_eq!(back.unsuppressed, ids(&["p0000001"]));
    assert_eq!(
        suppressed_in(&back.document),
        ids(&["h0000001", "h0000002", "f0000001"])
    );
    // Unsuppressing each in turn gives the document back exactly.
    let mut d = root_off;
    for f in ["p0000001", "h0000001", "h0000002", "f0000001"] {
        d = unsuppress(&d, f).document;
    }
    assert!(d.canonically_eq(&bracket()));
}

#[test]
fn a_pad_whose_whole_chain_is_suppressed_builds_alone_and_takes_back_only_itself() {
    // The design's optional base: a pad root r, a second pad q on it, and a pocket k on q.
    let r = r#"{"id": "r0000001", "type": "pad", "v": 1, "name": "Root", "sketch": "s0000001", "extent": {"type": "one_side", "length": 40, "reversed": true}}"#;
    let q = r#"{"id": "q0000001", "type": "pad", "v": 1, "name": "Second", "base": "r0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 40, "reversed": true}}"#;
    let k = r#"{"id": "k0000001", "type": "pocket", "v": 1, "name": "Holes", "base": "q0000001", "sketch": "s0000002", "extent": {"type": "one_side", "length": 5, "reversed": true}}"#;
    let d = document(&[S0000001, r, q, S0000002, k]);
    validate(&d).unwrap();
    // The root alone: q's base is optional, so q builds alone, and k on it.
    let root_off = suppress(&d, "r0000001");
    assert_eq!(root_off.suppressed, ids(&["r0000001"]));
    // Then q: k needs a base, and its whole chain is suppressed.
    let both = suppress(&root_off.document, "q0000001");
    assert_eq!(both.suppressed, ids(&["q0000001", "k0000001"]));
    // Unsuppressing q takes back q alone: its base is optional (the design).
    let back = unsuppress(&both.document, "q0000001");
    assert_eq!(back.unsuppressed, ids(&["q0000001"]));
    assert_eq!(
        suppressed_in(&back.document),
        ids(&["r0000001", "k0000001"])
    );
    // k unsuppressed takes back its chain's root, r, as the rule says (nothing up its chain
    // is unsuppressed), not q, which r's return lets k build on through it.
    let back = unsuppress(&both.document, "k0000001");
    assert_eq!(back.unsuppressed, ids(&["r0000001", "k0000001"]));

    // Rebuilt on the fake with the root suppressed: q is made alone, as a pad with no base
    // is, and k cuts it: each solid as in the document where q has no base.
    let mut rootless = document(&[S0000001, q, S0000002, k]);
    if let silico_core::document::Operation::Pad { base, .. } = &mut rootless.features[1].operation
    {
        *base = None;
    }
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let suppressed_model = published(&engine, checked(&root_off.document));
    let other = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let rootless_model = published(&other, checked(&rootless));
    for f in ["q0000001", "k0000001"] {
        assert_eq!(
            described(&suppressed_model, f),
            described(&rootless_model, f),
            "{f}"
        );
    }
    assert_eq!(suppressed_model.bodies(), ids(&["k0000001"]));
    assert_eq!(
        suppressed_model.outcome(&id("r0000001")),
        Some(&FeatureOutcome::Suppressed)
    );
}

#[test]
fn a_chain_of_names_is_followed_to_a_fixed_point() {
    // b, a chamfer c1 of b's edge, a chamfer c2 on c1 of c1's own edge (a maker c1), and a
    // chamfer c3 on c2 of b's edge: suppressing c1 takes c2 (it names c1's element), and c3
    // builds on b through both; suppressing b takes all three (c1 and c3 name it, c2 has
    // nothing unsuppressed up its chain).
    let b = r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#;
    let c1 = r#"{"id": "c0000001", "type": "chamfer", "v": 1, "name": "C1", "base": "b0000001", "edges": ["b0000001.box.Edge1"], "size": {"distance": 1}}"#;
    let c2 = r#"{"id": "c0000002", "type": "chamfer", "v": 1, "name": "C2", "base": "c0000001", "edges": ["c0000001.chamfer.Edge(G b0000001.box.Edge1)"], "size": {"distance": 0.5}}"#;
    let c3 = r#"{"id": "c0000003", "type": "chamfer", "v": 1, "name": "C3", "base": "c0000002", "edges": ["b0000001.box.Edge7"], "size": {"distance": 1}}"#;
    let d = document(&[b, c1, c2, c3]);
    validate(&d).unwrap();
    assert_eq!(
        suppress(&d, "c0000001").suppressed,
        ids(&["c0000001", "c0000002"])
    );
    assert_eq!(
        suppress(&d, "b0000001").suppressed,
        ids(&["b0000001", "c0000001", "c0000002", "c0000003"])
    );
    // Unsuppressing c2 takes back c1, the maker of its name, and with it c1's chain's root,
    // b, which c1 needs.
    let all = suppress(&d, "b0000001").document;
    assert_eq!(
        unsuppress(&all, "c0000002").unsuppressed,
        ids(&["b0000001", "c0000001", "c0000002"])
    );
}

#[test]
fn a_booleans_tool_with_its_whole_chain_suppressed_takes_the_boolean() {
    let base = r#"{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#;
    let tool = r#"{"id": "t0000001", "type": "box", "v": 1, "name": "T", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#;
    let cut = r#"{"id": "m0000001", "type": "cut", "v": 1, "name": "U", "base": "b0000001", "tool": "t0000001"}"#;
    let after = r#"{"id": "c0000001", "type": "chamfer", "v": 1, "name": "C", "base": "m0000001", "edges": ["b0000001.box.Edge5"], "size": {"distance": 1}}"#;
    let d = document(&[base, tool, cut, after]);
    // The tool alone: the cut needs it; the chamfer builds on the cut's base through it.
    let done = suppress(&d, "t0000001");
    assert_eq!(done.suppressed, ids(&["t0000001", "m0000001"]));
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let model = published(&engine, checked(&done.document));
    assert_eq!(model.bodies(), ids(&["c0000001"]));
    assert!(matches!(
        model.outcome(&id("c0000001")),
        Some(FeatureOutcome::Built { .. })
    ));
    // Built on b: the chamfer of the box, as the fake describes a chamfer of b itself.
    let direct = document(&[
        base,
        r#"{"id": "c0000001", "type": "chamfer", "v": 1, "name": "C", "base": "b0000001", "edges": ["b0000001.box.Edge5"], "size": {"distance": 1}}"#,
    ]);
    let other = Recompute::new(FakeKernel::new(2).unwrap());
    assert_eq!(
        described(&model, "c0000001"),
        described(&published(&other, checked(&direct)), "c0000001")
    );
    // Unsuppressing the cut takes back its tool, its chain's root.
    assert_eq!(
        unsuppress(&done.document, "m0000001").unsuppressed,
        ids(&["t0000001", "m0000001"])
    );
}

#[test]
fn an_unknown_feature_is_refused_naming_feature() {
    for refused in [
        suppression::suppress(&checked(&bracket()), &id("z0000001")).unwrap_err(),
        suppression::unsuppress(&checked(&bracket()), &id("z0000001")).unwrap_err(),
    ] {
        assert_eq!(refused.parameter(), Some("feature"));
        assert_eq!(refused.message(), "the document has no feature z0000001");
    }
}

// ---------------------------------------------------------------------------------------
// The gate.

/// The gate's refusals of `document`: each (feature, parameter, message).
fn refusals(document: &Document) -> Vec<(String, String, String)> {
    validate(document)
        .unwrap_err()
        .errors()
        .iter()
        .map(|e| {
            (
                e.feature().unwrap_or("").to_owned(),
                e.parameter().unwrap_or("").to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

#[test]
fn the_gate_refuses_a_dependent_of_a_suppressed_feature_left_unsuppressed_naming_both() {
    // A sketch suppressed by hand, its pocket not.
    assert_eq!(
        refusals(&with_suppressed(&bracket(), &["s0000002"])),
        [(
            "h0000001".to_owned(),
            "sketch".to_owned(),
            "h0000001 isn't suppressed, but its sketch s0000002 is suppressed: a feature that depends on a suppressed one is suppressed with it".to_owned()
        )]
    );
    // The root suppressed, the rest not: the first pocket's chain, and the fillet's name of
    // the pad's edge (its first cause), one refusal a feature. h0000002's base, h0000001,
    // isn't suppressed, so it has no cause of its own: what it builds on is h0000001's to
    // mend.
    assert_eq!(
        refusals(&with_suppressed(&bracket(), &["p0000001"])),
        [
            (
                "h0000001".to_owned(),
                "base".to_owned(),
                "h0000001 isn't suppressed, but its base p0000001 is suppressed with every feature up its chain, and a pocket needs its base: a feature that depends on a suppressed one is suppressed with it".to_owned()
            ),
            (
                "f0000001".to_owned(),
                "edges.0".to_owned(),
                "f0000001 isn't suppressed, but edges.0 names an element of p0000001, which is suppressed: a feature that depends on a suppressed one is suppressed with it".to_owned()
            ),
        ]
    );
    // The root and the first pocket suppressed: now h0000002's whole chain is.
    assert_eq!(
        refusals(&with_suppressed(&bracket(), &["p0000001", "h0000001"]))[0],
        (
            "h0000002".to_owned(),
            "base".to_owned(),
            "h0000002 isn't suppressed, but its base h0000001 is suppressed with every feature up its chain, and a pocket needs its base: a feature that depends on a suppressed one is suppressed with it".to_owned()
        )
    );
    let error = &validate(&with_suppressed(&bracket(), &["s0000002"]))
        .unwrap_err()
        .errors()[0]
        .clone();
    assert_eq!(
        error.suggestion(),
        Some(
            "suppress h0000001 too, or unsuppress s0000002 (suppress_feature does either with what goes with it)"
        )
    );
    // What a suppression gives, the gate takes.
    for f in [
        "s0000001", "p0000001", "s0000002", "h0000001", "h0000002", "f0000001",
    ] {
        validate(&suppress(&bracket(), f).document).unwrap();
    }
    // A pocket suppressed alone, the rest built through it: taken.
    validate(&with_suppressed(&bracket(), &["h0000001"])).unwrap();
}

#[test]
fn a_fillet_of_a_holes_rim_is_a_dependent_of_the_pocket_and_its_sketch() {
    // The design's case: a fillet of h0000001's rim, on the bracket's fillet. Its name's
    // makers are the pocket and the holes' sketch.
    let rim = r#"{"id": "f0000002", "type": "fillet", "v": 1, "name": "Rim", "base": "f0000001", "edges": ["h0000001.pocket.Edge(M s0000002.sketch.Edge.cb000001)"], "radius": 0.5}"#;
    let d = document(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001, rim,
    ]);
    validate(&d).unwrap();
    assert_eq!(
        suppress(&d, "h0000001").suppressed,
        ids(&["h0000001", "f0000002"])
    );
    assert_eq!(
        suppress(&d, "s0000002").suppressed,
        ids(&["s0000002", "h0000001", "f0000002"])
    );
    // Hand-edited with the pocket suppressed and the rim's fillet not: refused, naming both.
    let (feature, parameter, message) = refusals(&with_suppressed(&d, &["h0000001"]))
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        (feature.as_str(), parameter.as_str()),
        ("f0000002", "edges.0")
    );
    assert!(
        message.contains("f0000002") && message.contains("h0000001"),
        "{message}"
    );
    // With the sketch suppressed too, the first maker of the name that's suppressed is
    // named: the pocket, as the name writes it first.
    let (feature, _, message) = refusals(&with_suppressed(&d, &["s0000002", "h0000001"]))
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(feature, "f0000002");
    assert!(message.contains("element of h0000001"), "{message}");
    // Unsuppressing it takes back the pocket and its sketch.
    let all = suppress(&d, "s0000002").document;
    assert_eq!(
        unsuppress(&all, "f0000002").unsuppressed,
        ids(&["s0000002", "h0000001", "f0000002"])
    );
}

// ---------------------------------------------------------------------------------------
// The rebuild.

#[test]
fn a_suppressed_pocket_builds_as_the_pocket_removed_and_runs_nothing_of_its_own() {
    // By another path: the chain's removal, which rewires h0000002 to p0000001 in the text.
    let suppressed = with_suppressed(&bracket(), &["h0000001"]);
    let removed = chain::remove(&checked(&bracket()), &id("h0000001"))
        .unwrap()
        .document;
    let kernel = std::sync::Arc::new(FakeKernel::new(2).unwrap());
    let engine = Recompute::new(std::sync::Arc::clone(&kernel)).with_sketches(FakeSketches::new());
    let model = published(&engine, checked(&suppressed));
    let other = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let without = published(&other, checked(&removed));
    for f in ["p0000001", "h0000002", "f0000001"] {
        assert_eq!(described(&model, f), described(&without, f), "{f}");
    }
    assert_eq!(
        model.outcome(&id("h0000001")),
        Some(&FeatureOutcome::Suppressed)
    );
    // The one body is the fillet: the pad is consumed by h0000002, through h0000001.
    assert_eq!(model.bodies(), ids(&["f0000001"]));
    // The fillet's name of the pad's edge is found on h0000002, built on the pad: no
    // broken reference, no error.
    assert_eq!(model.errors().count(), 0);
    // Its names are those of the document without it.
    for f in ["h0000002", "f0000001"] {
        assert_eq!(
            model.element_map(&id(f)).unwrap(),
            without.element_map(&id(f)).unwrap(),
            "{f}"
        );
    }
    assert!(model.element_map(&id("h0000001")).is_none());
    // One pocket's steps fewer: the bracket makes two cuts (its pockets'; the fillet is
    // no cut), this one, h0000002's alone; counted by the fake's calls.
    let cuts = made(&kernel.calls())
        .into_iter()
        .filter(|name| *name == "Cut")
        .count();
    assert_eq!(cuts, 1);
}

#[test]
fn a_suppressed_sketch_isnt_solved_nor_a_suppressed_fillet_made() {
    let sketches = FakeSketches::new();
    let kernel = std::sync::Arc::new(FakeKernel::new(2).unwrap());
    let engine = Recompute::new(std::sync::Arc::clone(&kernel)).with_sketches(sketches.clone());
    let done = suppress(&bracket(), "s0000002");
    let model = published(&engine, checked(&done.document));
    for f in ["s0000002", "h0000001"] {
        assert_eq!(
            model.outcome(&id(f)),
            Some(&FeatureOutcome::Suppressed),
            "{f}"
        );
    }
    // Two sketches solved, the pad's and the upright's: by hand, the bracket's three less
    // the one suppressed.
    assert_eq!(sketches.requests().len(), 2);
    // No fillet made with the fillet suppressed.
    let kernel = std::sync::Arc::new(FakeKernel::new(2).unwrap());
    let engine = Recompute::new(std::sync::Arc::clone(&kernel)).with_sketches(FakeSketches::new());
    let model = published(&engine, checked(&suppress(&bracket(), "f0000001").document));
    assert!(
        !made(&kernel.calls()).contains(&"Fillet"),
        "{:?}",
        made(&kernel.calls())
    );
    assert_eq!(model.bodies(), ids(&["h0000002"]));
    // Everything suppressed but the sketches: no bodies.
    let model = published(&engine, checked(&suppress(&bracket(), "p0000001").document));
    assert!(model.bodies().is_empty());
}

#[test]
fn a_save_drops_a_suppressed_features_fingerprint_whatever_reaches_it() {
    // A fingerprinted bracket with h0000001 suppressed by hand, its fingerprint left:
    // saved, it carries none (A10), and nothing else's is dropped.
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let fingerprinted = published(&engine, checked(&bracket())).saved();
    assert!(fingerprinted.features[3].fingerprint.is_some());
    let hand = with_suppressed(&fingerprinted, &["h0000001"]);
    let model = published(&engine, checked(&hand));
    let saved = model.saved();
    assert!(saved.features[3].fingerprint.is_none());
    for at in [1, 5, 6] {
        assert!(saved.features[at].fingerprint.is_some(), "{at}");
    }
    // Saved as stored (an unreached feature at commit), it's dropped too.
    let stored = model.saved_as(|_| silico_core::recompute::Save::Stored);
    assert!(stored.features[3].fingerprint.is_none());
    assert!(stored.features[5].fingerprint == fingerprinted.features[5].fingerprint);
    // A suppressed feature is never drift, whatever it stores.
    assert!(model.drifted().iter().all(|(f, _)| f != &id("h0000001")));
}
