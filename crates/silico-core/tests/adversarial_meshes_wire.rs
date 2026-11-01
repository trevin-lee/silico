//! Break-it tests of `meshes` on the wire (docs/design/m5.0-app.md, section 7, `meshes`,
//! M5.5): hostile requests by name, each answered with a refused envelope, never a panic or
//! a hang; the ends of the size; an empty `features`; `held` naming every body, twice, among
//! a hundred thousand others; and a session closed while its read meshes. The fake kernel,
//! whose record says what it was asked to mesh.

use super::example::EXAMPLE;
use super::listing::{Fixture, HANG, TWO_BODIES, answer, fixture};
use silico_core::ErrorCode;
use silico_core::command::meshes::Meshes;
use silico_core::command::{Call, ModelChoice};
use silico_core::kernel::Op;
use silico_core::views::NamedView;

fn meshed(f: &Fixture) -> usize {
    f.kernel
        .calls()
        .iter()
        .filter(|c| matches!(c.op, Op::Tessellate { .. }))
        .count()
}

/// `meshes` called on `arguments`: whether it ran, and its envelope.
fn called(f: &Fixture, arguments: &str) -> (bool, serde_json::Value) {
    match f.sessions.call("meshes", arguments) {
        Call::Running(calling) => match calling.wait_timeout(HANG) {
            Ok(answered) => (
                answered.ok,
                serde_json::from_str(&answered.text)
                    .unwrap_or_else(|e| panic!("{e}: {}", answered.text)),
            ),
            Err(_) => panic!("meshes didn't answer: {arguments}"),
        },
        Call::Unknown => panic!("meshes isn't a command"),
    }
}

fn front(features: &str, held: &str, width: u64, height: u64) -> String {
    format!(
        r#"{{"session": "s1", "model": "current", "view": "front", "camera": null, "width": {width}, "height": {height}, "features": {features}, "held": {held}}}"#
    )
}

#[test]
fn hostile_requests_are_refused_envelopes_and_nothing_is_meshed() {
    let f = fixture();
    f.open("example.silico", EXAMPLE);
    let deep = format!("{}{}", "[".repeat(70), "]".repeat(70));
    let cases = [
        "[]".to_owned(),
        "null".to_owned(),
        r#""meshes""#.to_owned(),
        "{}".to_owned(),
        "{".to_owned(),
        front("null", &deep, 100, 100),
        front("null", "[[]]", 100, 100),
        front("null", r#"{"key": "0", "deflection": 1}"#, 100, 100),
        front(r#"{"a": 1}"#, "[]", 100, 100),
        front("null", "[]", 100, 100).replace(r#""width": 100"#, r#""width": 1e2"#),
        front("null", "[]", 100, 100).replace(r#""width": 100"#, r#""width": 0100"#),
        front("null", "[]", 100, 100).replace(r#""height": 100"#, r#""height": -0"#),
        // A key given twice, at the top.
        front("null", "[]", 100, 100).replace(r#""held": []"#, r#""held": [], "held": []"#),
        // A string with a lone surrogate.
        front(r#"["\ud800"]"#, "[]", 100, 100),
    ];
    for arguments in &cases {
        let (ok, envelope) = called(&f, arguments);
        assert!(!ok, "{arguments}: {envelope}");
        assert_eq!(envelope["ok"], false, "{arguments}");
        assert!(envelope["total"].as_u64().unwrap() >= 1, "{arguments}");
        assert_eq!(
            envelope["errors"][0]["code"], "invalid_input",
            "{arguments}: {envelope}"
        );
    }
    assert_eq!(meshed(&f), 0);
}

#[test]
fn a_size_at_u32s_end_is_refused_naming_each_side_and_nothing_is_meshed() {
    let f = fixture();
    f.open("example.silico", EXAMPLE);
    let (ok, envelope) = called(&f, &front("null", "[]", 4_294_967_295, 4_294_967_295));
    assert!(!ok);
    let parameters: Vec<&str> = envelope["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["parameter"].as_str().unwrap())
        .collect();
    assert_eq!(parameters, ["width", "height"], "{envelope}");
    // One past u32, the reader's.
    let (ok, envelope) = called(&f, &front("null", "[]", 4_294_967_296, 1));
    assert!(!ok);
    assert_eq!(
        envelope["errors"][0]["message"],
        "width must be a whole number from 0 to 4294967295, got 4294967296"
    );
    assert_eq!(meshed(&f), 0);
}

#[test]
fn an_empty_features_list_meshes_nothing_and_answers_no_bodies() {
    let f = fixture();
    f.open("example.silico", EXAMPLE);
    let (ok, envelope) = called(&f, &front("[]", "[]", 100, 100));
    assert!(ok, "{envelope}");
    assert_eq!(envelope["result"]["bodies"], serde_json::json!([]));
    // The named view still fits every body, so its camera is the whole model's: the cut's
    // box, (0, 0, −1) to (40, 20, 11), seen from −y, centred on (20, 10, 5).
    assert_eq!(
        envelope["result"]["camera"]["target"],
        serde_json::json!([20, 10, 5])
    );
    assert_eq!(meshed(&f), 0);
}

#[test]
fn held_naming_every_body_twice_among_a_hundred_thousand_others_meshes_nothing_again() {
    let f = fixture();
    f.open("two.silico", TWO_BODIES);
    let (ok, first) = called(&f, &front("null", "[]", 100, 100));
    assert!(ok, "{first}");
    assert_eq!(meshed(&f), 2);
    let bodies = first["result"]["bodies"].as_array().unwrap();
    assert_eq!(bodies.len(), 2);
    let mut held: Vec<String> = (0..100_000u32)
        .map(|i| format!(r#"{{"key": "{i:064x}", "deflection": 1}}"#))
        .collect();
    for body in bodies.iter().chain(bodies) {
        held.push(format!(
            r#"{{"key": {}, "deflection": {}}}"#,
            body["key"], body["deflection"]
        ));
    }
    let (ok, again) = called(
        &f,
        &front("null", &format!("[{}]", held.join(", ")), 100, 100),
    );
    assert!(ok, "{again}");
    for body in again["result"]["bodies"].as_array().unwrap() {
        assert_eq!(body["held"], true, "{body}");
        assert_eq!(body["positions"], serde_json::json!([]));
    }
    assert_eq!(meshed(&f), 2);
}

#[test]
fn a_published_read_whose_session_closes_mid_mesh_answers_not_open_not_cancelled() {
    let f = fixture();
    let s = f.open("example.silico", EXAMPLE);
    let request = |height| Meshes {
        view: Some(NamedView::Front),
        camera: None,
        width: height,
        height,
        features: None,
        held: vec![],
    };
    answer(f.sessions.meshes(&s, ModelChoice::Current, request(100)))
        .result
        .unwrap();
    // Another size's level, held in the kernel.
    let hold = f.kernel.hold(|op| op.name() == "Tessellate");
    let held = f.sessions.meshes(&s, ModelChoice::Published, request(1000));
    assert!(f.kernel.wait_running(1, HANG));
    answer(f.sessions.close(&s)).result.unwrap();
    let refused = answer(held).result.unwrap_err();
    hold.release();
    let e = &refused.errors()[0];
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("session")),
        "{e:?}"
    );
}
