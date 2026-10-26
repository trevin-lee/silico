//! break-it for M4.5's `mass_properties` (`command::measure`, `command::wire::measure`):
//! requests wrong in every field, documents with no bodies, many refused bodies, bodies
//! that are tiny, inside out, or empty as a worker says them, centres past what a length
//! holds, results of extreme values held to the committed snapshot, and reads of a session
//! closed, unknown, flooded, and rebuilt from its cache. Expected values come from the
//! design (docs/design/m4.1-command-api.md, section 7: written to the millionth), the
//! snapshot's own descriptions, by-hand volumes (l × w × h), the protocol's `summary_fault`
//! ("a summary no shape has"), and `check`'s word for the same summary.

use super::listing::{BOX, TWO_BODIES, answer, codes, fixture, model_of};
use super::model::{checked, id, published};
use super::references::{BLOCK, HEAD, fillet};

use std::path::PathBuf;
use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::command::measure::{Bodies, MassProperties, mass_properties};
use silico_core::command::wire::measure::MassPropertiesRequest;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{ModelChoice, Problems, SessionId};
use silico_core::document::json::{Node, read};
use silico_core::document::{Length, Measure};
use silico_core::fake::FakeKernel;
use silico_core::kernel::Op;
use silico_core::recompute::{Model, Recompute};
use silico_protocol::{Reply, ShapeSummary};

fn parsed(text: &str) -> Node {
    read(text.as_bytes()).unwrap()
}

fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap()
}

/// Each refusal's parameter (empty for none) and message, in order.
fn said(problems: &Problems) -> Vec<(String, String)> {
    problems
        .errors()
        .iter()
        .map(|e| {
            (
                e.parameter().unwrap_or("").to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

/// The committed snapshot's `part`, or its definition `def` in it.
fn snapshot_part(part: &str, def: Option<&str>) -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract/commands/mass_properties.json");
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let root = &schema[part];
    match def {
        None => root.clone(),
        Some(def) => serde_json::json!({"$defs": root["$defs"], "$ref": format!("#/$defs/{def}")}),
    }
}

fn holds(schema: &serde_json::Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{what}: {errors:?}\n{text}");
}

fn valid(schema: &serde_json::Value, text: &str) -> bool {
    let validator = jsonschema::validator_for(schema).unwrap();
    validator.is_valid(&serde_json::from_str(text).unwrap())
}

fn doc(features: &[String]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

fn boxed(id: &str, position: [&str; 3], size: [&str; 3]) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "B", "position": [{}, {}, {}], "length": {}, "width": {}, "height": {}}}"#,
        position[0], position[1], position[2], size[0], size[1], size[2]
    )
}

/// The ID of the `n`th box (from 0): 8 characters of the IDs' alphabet.
fn nth_id(n: usize) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut out = [b'b'; 8];
    let mut k = n;
    for slot in out.iter_mut().rev().take(4) {
        *slot = DIGITS[k % 32];
        k /= 32;
    }
    String::from_utf8(out.to_vec()).unwrap()
}

/// `BOX` (a 1 mm box) rebuilt on a fake kernel whose worker answers each summary with
/// `summary` instead.
fn forged(summary: ShapeSummary) -> Arc<Model> {
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().answer_instead(move |op| match op {
        Op::Summarize { .. } => Some(Reply::Summary(summary.clone())),
        _ => None,
    });
    published(&engine, checked(BOX))
}

/// A 1 mm box's summary as a worker would give it, with `change` made to it.
fn box_summary(change: impl FnOnce(&mut ShapeSummary)) -> ShapeSummary {
    let mut s = ShapeSummary {
        volume: 1.0,
        area: 6.0,
        center: [0.5, 0.5, 0.5],
        faces: 6,
        edges: 12,
        vertices: 8,
        bounds: Some([[0.0; 3], [1.0; 3]]),
        valid: true,
    };
    change(&mut s);
    s
}

// --- The request, read strictly ---------------------------------------------------------

#[test]
fn a_request_wrong_in_every_field_says_each_reason_in_the_shapes_order() {
    let refused =
        MassPropertiesRequest::read(&parsed(r#"{"session": 1, "model": "now", "feature": 5}"#))
            .unwrap_err();
    let params: Vec<String> = said(&refused).into_iter().map(|(p, _)| p).collect();
    assert_eq!(
        params,
        ["session", "model", "feature"],
        "{:?}",
        said(&refused)
    );
    assert_eq!(refused.total(), 3);
}

#[test]
fn a_request_missing_feature_is_refused_naming_it_as_the_snapshot_requires_it() {
    let text = r#"{"session": "s1", "model": "current"}"#;
    let refused = MassPropertiesRequest::read(&parsed(text)).unwrap_err();
    assert_eq!(
        codes(&refused),
        [(ErrorCode::InvalidInput, Some("feature".to_owned()))],
        "{:?}",
        said(&refused)
    );
    assert!(!valid(&snapshot_part("request", None), text));
}

#[test]
fn a_request_with_an_unknown_field_is_refused_naming_it() {
    let text = r#"{"session": "s1", "model": "current", "feature": null, "density": 7.85}"#;
    let refused = MassPropertiesRequest::read(&parsed(text)).unwrap_err();
    assert_eq!(refused.errors().len(), 1, "{:?}", said(&refused));
    assert_eq!(refused.errors()[0].parameter(), Some("density"));
    assert!(!valid(&snapshot_part("request", None), text));
}

#[test]
fn a_feature_that_isnt_an_id_is_refused_by_the_reader_and_the_snapshot_alike() {
    for feature in [
        r#""K7F2QX3M""#,
        r#""""#,
        r#""null""#,
        r#""k7f2qx3""#,
        r#""k7f2qx3mm""#,
        r#""k7f2qx3i""#,
        r#""k7f2qx3m ""#,
        r#""k7f2qx3\u0000""#,
        r#"["k7f2qx3m"]"#,
        r#"{"id": "k7f2qx3m"}"#,
        "true",
        "12345678",
    ] {
        let text = format!(r#"{{"session": "s1", "model": "current", "feature": {feature}}}"#);
        let refused = MassPropertiesRequest::read(&parsed(&text)).unwrap_err();
        assert_eq!(
            codes(&refused),
            [(ErrorCode::InvalidInput, Some("feature".to_owned()))],
            "{feature}: {:?}",
            said(&refused)
        );
        assert!(
            !valid(&snapshot_part("request", None), &text),
            "the snapshot takes {feature}"
        );
    }
}

#[test]
fn a_feature_given_is_read_and_written_back_byte_for_byte_and_holds_to_its_snapshot() {
    let text = r#"{"session": "s42", "model": "published", "feature": "k7f2qx3m"}"#;
    let request = MassPropertiesRequest::read(&parsed(text)).unwrap();
    assert_eq!(
        request,
        MassPropertiesRequest {
            session: SessionId::parse("s42").unwrap(),
            model: ModelChoice::Published,
            feature: Some(id("k7f2qx3m")),
        }
    );
    assert_eq!(written(&request), text);
    holds(&snapshot_part("request", None), text, "request");
}

#[test]
fn a_request_that_isnt_an_object_is_one_reason() {
    for text in ["null", "[]", r#""mass_properties""#, "0"] {
        let refused = MassPropertiesRequest::read(&parsed(text)).unwrap_err();
        assert_eq!(refused.total(), 1, "{text}: {:?}", said(&refused));
    }
}

// --- What it reads ----------------------------------------------------------------------

#[test]
fn a_document_with_no_features_has_no_bodies_to_measure() {
    let model = model_of(&doc(&[]));
    assert_eq!(
        mass_properties(&model, None).unwrap(),
        Bodies { bodies: Vec::new() }
    );
    assert_eq!(
        written(&mass_properties(&model, None).unwrap()),
        r#"{"bodies": []}"#
    );
}

#[test]
fn bodies_come_in_timeline_order_not_in_the_order_of_their_ids() {
    let text = doc(&[
        boxed("zzzzzzzz", ["0", "0", "0"], ["1", "1", "1"]),
        boxed("00000000", ["5", "0", "0"], ["2", "1", "1"]),
        boxed("mmmmmmmm", ["9", "0", "0"], ["3", "1", "1"]),
    ]);
    let model = model_of(&text);
    let bodies = mass_properties(&model, None).unwrap().bodies;
    let order: Vec<&str> = bodies.iter().map(|b| b.feature.as_str()).collect();
    assert_eq!(order, ["zzzzzzzz", "00000000", "mmmmmmmm"]);
    // 1, 2, 3 mm³ by hand.
    let volumes: Vec<Measure> = bodies.iter().map(|b| b.volume).collect();
    assert_eq!(
        volumes,
        [1_000_000, 2_000_000, 3_000_000].map(Measure::from_millionths)
    );
    // And the same twice over: nothing depends on the run.
    assert_eq!(mass_properties(&model, None).unwrap().bodies, bodies);
}

#[test]
fn a_box_centred_on_the_origin_has_its_centre_written_as_zeros_not_negative_zeros() {
    let model = model_of(&doc(&[boxed(
        "bbbbbbbb",
        ["-0.5", "-0.5", "-0.5"],
        ["1", "1", "1"],
    )]));
    let bodies = mass_properties(&model, None).unwrap();
    assert_eq!(bodies.bodies[0].center_of_mass, [Length::from_nm(0); 3]);
    let text = written(&bodies);
    assert!(text.contains(r#""center_of_mass": [0, 0, 0]"#), "{text}");
    holds(&snapshot_part("result", None), &text, "result");
}

#[test]
fn a_kilometre_cube_far_out_is_measured_exactly_and_written_as_the_snapshot_takes_it() {
    // 1e6 mm on a side: 1e18 mm³, 6e12 mm², centred at (-5e5, 5e5, 5e5) from a corner at
    // (-1e6, 0, 0).
    let model = model_of(&doc(&[boxed(
        "bbbbbbbb",
        ["-1000000", "0", "0"],
        ["1000000", "1000000", "1000000"],
    )]));
    let bodies = mass_properties(&model, None).unwrap();
    assert_eq!(
        bodies.bodies,
        [MassProperties {
            feature: id("bbbbbbbb"),
            volume: Measure::from_millionths(1_000_000_000_000_000_000_000_000),
            area: Measure::from_millionths(6_000_000_000_000_000_000),
            center_of_mass: [-500_000_000_000, 500_000_000_000, 500_000_000_000]
                .map(Length::from_nm),
        }]
    );
    let text = written(&bodies);
    assert!(text.contains(r#""volume": 1000000000000000000"#), "{text}");
    holds(&snapshot_part("result", None), &text, "result");
}

#[test]
fn a_hundred_and_fifty_bodies_refused_list_the_first_hundred_and_count_them_all() {
    let features: Vec<String> = (0..150)
        .map(|n| {
            boxed(
                &nth_id(n),
                [&format!("{}", n * 2), "0", "0"],
                ["1", "1", "1"],
            )
        })
        .collect();
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        (op.name() == "Summarize")
            .then(|| silico_core::Error::new(ErrorCode::InvalidResult, "no measure today"))
    });
    let model = published(&engine, checked(&doc(&features)));
    let refused = mass_properties(&model, None).unwrap_err();
    assert_eq!(refused.errors().len(), 100);
    assert_eq!(refused.total(), 150);
    // In timeline order, each naming its own feature.
    let named: Vec<&str> = refused
        .errors()
        .iter()
        .map(|e| e.feature().unwrap())
        .collect();
    let expected: Vec<String> = (0..100).map(nth_id).collect();
    assert_eq!(named, expected);
}

#[test]
fn one_body_refused_among_good_ones_refuses_the_call_naming_only_it_and_the_rest_still_read() {
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    // The second box alone (2 mm long) is refused its summary.
    engine.kernel().refuse(|op| match op {
        Op::Summarize { shape } => FakeKernel::describe(shape)
            .filter(|made| made.contains("box(2,"))
            .map(|_| silico_core::Error::new(ErrorCode::InvalidResult, "not this one")),
        _ => None,
    });
    let text = doc(&[
        boxed("aaaaaaaa", ["0", "0", "0"], ["1", "1", "1"]),
        boxed("bbbbbbbb", ["5", "0", "0"], ["2", "1", "1"]),
        boxed("cccccccc", ["9", "0", "0"], ["3", "1", "1"]),
    ]);
    let model = published(&engine, checked(&text));
    let refused = mass_properties(&model, None).unwrap_err();
    let named: Vec<Option<&str>> = refused.errors().iter().map(|e| e.feature()).collect();
    assert_eq!(named, [Some("bbbbbbbb")], "{:?}", said(&refused));
    // Asked for by name, it's refused naming `feature`... and its neighbours still read.
    let alone = mass_properties(&model, Some(&id("bbbbbbbb"))).unwrap_err();
    assert_eq!(alone.errors()[0].feature(), Some("bbbbbbbb"));
    let third = mass_properties(&model, Some(&id("cccccccc"))).unwrap();
    assert_eq!(third.bodies[0].volume, Measure::from_millionths(3_000_000));
}

#[test]
fn a_body_whose_references_broke_gives_every_broken_reference_counted() {
    let text = format!(
        "{HEAD}{BLOCK},\n{}]}}",
        fillet(
            "f0000001",
            "k7f2qx3m",
            &[
                "k7f2qx3m.box.Edge13",
                "k7f2qx3m.box.Edge14",
                "k7f2qx3m.box.Edge15"
            ],
            "0.5"
        )
    );
    let model = model_of(&text);
    let refused = mass_properties(&model, Some(&id("f0000001"))).unwrap_err();
    let codes: Vec<ErrorCode> = refused.errors().iter().map(|e| e.code()).collect();
    assert_eq!(
        codes,
        [ErrorCode::ReferenceBroken; 3],
        "{:?}",
        said(&refused)
    );
    assert_eq!(refused.total(), 3);
    // Every body: the fillet failed, so it consumed nothing, and the block is the body, as
    // if the fillet were suppressed (SolidWorks' rebuild; `Model::bodies`): its 40 × 20 ×
    // 10 block's 8000 mm³, with no word of the fillet.
    let every = mass_properties(&model, None).unwrap().bodies;
    assert_eq!(every.len(), 1);
    assert_eq!(every[0].feature, id("k7f2qx3m"));
    assert_eq!(every[0].volume, Measure::from_millionths(8_000_000_000));
}

// --- What a measure holds ---------------------------------------------------------------

#[test]
fn a_micrometre_cube_is_measured_to_the_millionth_its_fingerprint_holds() {
    // Found by /break-it: the design said "precise to a relative 1e-9", which six decimals
    // aren't for a small solid. Decided: the measures are integrated to 1e-9 and written as
    // the fingerprint holds them, to a millionth of a mm³ and mm² (the design and snapshot
    // now say so). 0.001 mm on a side, which the gate admits, is 1e-9 mm³ by hand, 0 to a
    // millionth; its six faces' 6e-6 mm² are 0.000006.
    let model = model_of(&doc(&[boxed(
        "bbbbbbbb",
        ["0", "0", "0"],
        ["0.001", "0.001", "0.001"],
    )]));
    let body = &mass_properties(&model, None).unwrap().bodies[0];
    assert_eq!(body.volume, Measure::from_millionths(0));
    assert_eq!(body.area, Measure::from_millionths(6));
}

#[test]
fn a_small_boxs_volume_and_area_are_rounded_to_the_millionth() {
    // As above: 0.0123 mm on a side is 1.860867e-6 mm³ by hand, 0.000002 to a millionth,
    // and 6 × 0.0123² = 0.00090774 mm², 0.000908.
    let model = model_of(&doc(&[boxed(
        "bbbbbbbb",
        ["0", "0", "0"],
        ["0.0123", "0.0123", "0.0123"],
    )]));
    let body = &mass_properties(&model, None).unwrap().bodies[0];
    assert_eq!(body.volume, Measure::from_millionths(2));
    assert_eq!(body.area, Measure::from_millionths(908));
}

#[test]
fn a_summary_with_a_negative_volume_is_measured_as_its_fingerprint_holds_it() {
    // Found by /break-it, declined: a summary of volume -1 (a solid inside out) is what the
    // feature's fingerprint holds, and `check` reports the same; a measure that refused it
    // would disagree with the document. Whether a rebuild should take such a solid is the
    // rebuild's (a boolean's is refused as `ResultInvalid`), not the measure's.
    let model = forged(box_summary(|s| s.volume = -1.0));
    let body = &mass_properties(&model, None).unwrap().bodies[0];
    assert_eq!(body.volume, Measure::from_millionths(-1_000_000));
}

#[test]
fn a_summary_with_no_volume_is_measured_as_its_fingerprint_holds_it() {
    // As above, declined: a summary of volume 0 and area 0 is measured as 0 and 0.
    let model = forged(box_summary(|s| {
        s.volume = 0.0;
        s.area = 0.0;
    }));
    let body = &mass_properties(&model, None).unwrap().bodies[0];
    assert_eq!(body.volume, Measure::from_millionths(0));
    assert_eq!(body.area, Measure::from_millionths(0));
}

#[test]
fn a_worker_summary_whose_centre_is_past_a_length_is_blamed_on_the_worker_not_on_silico() {
    // Fixed: a worker's summary a `Length` can't hold is the worker's, `protocol_error`;
    // was: `internal`, "a Silico bug". A centre of 1e13 mm is finite, so `summary_fault`
    // passes it, but no `Length` (i64 nanometres, 9.2e12 mm) holds it; the core's word for
    // a reply no shape has is `protocol_error` (`KernelRequest::summarize`).
    let model = forged(box_summary(|s| s.center = [1e13, 0.5, 0.5]));
    let refused = mass_properties(&model, None).unwrap_err();
    let error = &refused.errors()[0];
    assert_eq!(error.feature(), Some("bbbbbbbb"));
    assert_eq!(
        error.code(),
        ErrorCode::ProtocolError,
        "{}",
        error.message()
    );
    assert_eq!(
        error.message(),
        "the kernel worker measured bbbbbbbb's solid past what a measure or a length holds"
    );
    assert_eq!(error.suggestion(), Some("start a new kernel worker"));
}

#[test]
fn a_worker_summary_whose_box_is_past_a_length_is_refused_as_check_refuses_it() {
    // A box past what a fingerprint holds: `check` never calls it a Silico bug (with a
    // fingerprint stored it's `drift`, "measures a fingerprint can't hold"; with none, it
    // says nothing); `mass_properties` must not call the same worker's summary one either.
    // Fixed: it's `protocol_error`; was `internal`, "this is a bug in Silico".
    let model = forged(box_summary(|s| {
        s.bounds = Some([[0.0; 3], [1e13, 1.0, 1.0]])
    }));
    let checked = silico_core::command::check(&model);
    let check_codes: Vec<ErrorCode> = checked.errors().iter().map(|e| e.code()).collect();
    assert!(
        !check_codes.contains(&ErrorCode::Internal),
        "{check_codes:?}"
    );
    let refused = mass_properties(&model, None).unwrap_err();
    assert_eq!(
        refused.errors()[0].code(),
        ErrorCode::ProtocolError,
        "{}",
        refused.errors()[0].message()
    );
}

#[test]
fn a_summary_that_isnt_finite_is_refused_as_check_refuses_it_never_dropped() {
    // Found by the tests review: a worker's NaN volume is refused as the reply's had
    // (`protocol_error`, "start a new kernel worker"), and the solid kept unmeasured; it was
    // answered `kernel_failed`, "couldn't be measured", the geometry blamed, where `check`
    // says the worker's fault; and no test would have seen the body dropped instead.
    let model = forged(box_summary(|s| s.volume = f64::NAN));
    let check: Vec<ErrorCode> = model.drift().iter().map(|e| e.code()).collect();
    assert_eq!(check, [ErrorCode::ProtocolError]);
    let refused = mass_properties(&model, None).unwrap_err();
    assert_eq!(refused.total(), 1);
    let error = &refused.errors()[0];
    assert_eq!(
        (error.code(), error.feature(), error.suggestion()),
        (
            ErrorCode::ProtocolError,
            Some("bbbbbbbb"),
            // The worker's own advice, then that the revision keeps the fault, as
            // `elements` gives it for the same solid.
            Some(
                "start a new kernel worker; to try again here: reading this revision again won't, since it keeps the fault; an edit that changes the document, or reopening the session, tries again; a save that writes only fingerprints keeps the fault"
            )
        ),
        "{}",
        error.message()
    );
    assert_eq!(error.cause(), Some("its volume has NaN"));
}

#[test]
fn a_centre_of_exactly_half_a_nanometre_rounds_one_way_every_time() {
    // 5e-7 mm is a half nanometre (not exact in binary); whichever way it rounds, it rounds
    // so on every read, and the result holds to the snapshot.
    let model = forged(box_summary(|s| s.center = [5e-7, -5e-7, 1.5e-6]));
    let first = mass_properties(&model, None).unwrap();
    for _ in 0..10 {
        assert_eq!(mass_properties(&model, None).unwrap(), first);
    }
    let nm: Vec<i64> = first.bodies[0]
        .center_of_mass
        .iter()
        .map(|l| l.nm())
        .collect();
    // Rust's `{:.6}` rounds the binary value: 5e-7 is 4.99999999999999977e-7 → 0, -5e-7 →
    // -0 → 0, and 1.5e-6 is 1.50000000000000004e-6 → 2.
    assert_eq!(nm, [0, 0, 2]);
    holds(&snapshot_part("result", None), &written(&first), "result");
}

// --- On a session -----------------------------------------------------------------------

#[test]
fn a_session_never_opened_or_closed_is_refused_and_the_next_read_still_works() {
    let f = fixture();
    let never = SessionId::parse("s999999").unwrap();
    // Each refused naming the session (the tests review: `is_err` alone took any refusal).
    for choice in [ModelChoice::Current, ModelChoice::Published] {
        let refused = answer(f.sessions.mass_properties(&never, choice, None));
        let problems = refused.result.unwrap_err();
        let error = &problems.errors()[0];
        assert_eq!(
            (error.code(), error.parameter()),
            (ErrorCode::InvalidInput, Some("session")),
            "{:?}",
            said(&problems)
        );
    }

    let s = f.open("two.silico", TWO_BODIES);
    // A sketch, a feature it hasn't, then a good read: each answers, the last in full.
    let missing = answer(f.sessions.mass_properties(
        &s,
        ModelChoice::Current,
        Some(id("zzzzzzzz")),
    ));
    assert_eq!(
        missing.result.unwrap_err().errors()[0].parameter(),
        Some("feature")
    );
    let good = answer(f.sessions.mass_properties(&s, ModelChoice::Current, None));
    assert_eq!(good.result.unwrap().bodies.len(), 2);

    answer(f.sessions.close(&s)).result.unwrap();
    for choice in [ModelChoice::Current, ModelChoice::Published] {
        let closed = answer(f.sessions.mass_properties(&s, choice, None));
        let problems = closed.result.unwrap_err();
        assert_eq!(problems.errors().len(), 1, "{:?}", said(&problems));
        let error = &problems.errors()[0];
        assert_eq!(
            (error.code(), error.parameter()),
            (ErrorCode::InvalidInput, Some("session")),
            "{:?}",
            said(&problems)
        );
    }
}

#[test]
fn a_published_read_asks_the_kernel_nothing() {
    let f = fixture();
    let s = f.open("two.silico", TWO_BODIES);
    let current = answer(f.sessions.mass_properties(&s, ModelChoice::Current, None));
    let before = f.kernel.calls().len();
    for _ in 0..5 {
        let published = answer(f.sessions.mass_properties(&s, ModelChoice::Published, None));
        assert_eq!(
            published.result.as_ref().unwrap(),
            current.result.as_ref().unwrap()
        );
        let current_again = answer(f.sessions.mass_properties(&s, ModelChoice::Current, None));
        assert_eq!(
            current_again.result.as_ref().unwrap(),
            current.result.as_ref().unwrap()
        );
    }
    assert_eq!(f.kernel.calls().len(), before);
}

#[test]
fn a_flood_of_published_reads_at_once_each_answers_or_is_refused_none_hangs() {
    let f = fixture();
    let s = f.open("two.silico", TWO_BODIES);
    let expected = answer(f.sessions.mass_properties(&s, ModelChoice::Current, None))
        .result
        .unwrap();
    let running: Vec<_> = (0..64)
        .map(|_| f.sessions.mass_properties(&s, ModelChoice::Published, None))
        .collect();
    let mut answered = 0;
    for r in running {
        match answer(r).result {
            Ok(bodies) => {
                assert_eq!(bodies, expected);
                answered += 1;
            }
            Err(problems) => {
                assert_eq!(problems.errors()[0].code(), ErrorCode::InvalidInput);
            }
        }
    }
    assert!(answered >= 1);
}

#[test]
fn a_session_reopened_from_its_cache_measures_as_its_first_rebuild_did() {
    let f = fixture();
    let text = doc(&[
        boxed("aaaaaaaa", ["0.1", "0.2", "0.3"], ["1.7", "2.9", "3.1"]),
        boxed("bbbbbbbb", ["-7", "0", "0"], ["0.333333", "1", "1"]),
    ]);
    let s = f.open("cached.silico", &text);
    let first = answer(f.sessions.mass_properties(&s, ModelChoice::Current, None))
        .result
        .unwrap();
    answer(f.sessions.close(&s)).result.unwrap();
    let path = f.dir.path().join("cached.silico");
    let again = answer(f.sessions.open(&path, false)).session.unwrap().id;
    let second = answer(
        f.sessions
            .mass_properties(&again, ModelChoice::Current, None),
    )
    .result
    .unwrap();
    assert_eq!(first, second);
    // By hand: 1.7 × 2.9 × 3.1 = 15.283, 0.333333 × 1 × 1 = 0.333333.
    let volumes: Vec<Measure> = second.bodies.iter().map(|b| b.volume).collect();
    assert_eq!(volumes, [15_283_000, 333_333].map(Measure::from_millionths));
}
