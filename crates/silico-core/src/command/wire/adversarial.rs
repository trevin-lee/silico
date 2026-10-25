//! Hostile requests through [`RequestFields`], hostile text through the writer, and every shape of
//! the envelope against its snapshot (`/break-it` on M4.4's wire). Expected values are the
//! design's (docs/design/m4.1-command-api.md, section 2), the error contract's
//! (`tests/contract/error-full.json`), the document reader's own rules for what an error
//! may quote (`document::json::shown`, `decode::field_name`), or JSON's (RFC 8259, read back
//! by serde_json, an independent reader).

use std::path::PathBuf;

use super::{Json, Layout, Refusals, RequestFields, ToWire, problems, response, write};
use crate::command::{
    ModelChoice, ModelRead, Problems, Response, Revision, SessionId, SessionInfo,
};
use crate::document::PROBLEMS_LISTED;
use crate::document::json::{Node, Value, bidi_control, read};
use crate::{Error, ErrorCode};

fn parsed(text: &str) -> Node {
    read(text.as_bytes()).unwrap()
}

/// Whether `text` prints as it reads: no C0 or C1 control, DEL, line or paragraph
/// separator, or bidirectional control, so it's safe to show on a terminal or in a log.
fn printable(text: &str) -> bool {
    !text.chars().any(|c| {
        (c as u32) < 0x20 || matches!(c as u32, 0x7f..=0x9f | 0x2028 | 0x2029) || bidi_control(c)
    })
}

/// Every refusal's parameter and message.
fn said(problems: &Problems) -> Vec<(Option<String>, String)> {
    problems
        .errors()
        .iter()
        .map(|e| {
            assert_eq!(e.code(), ErrorCode::InvalidInput);
            (e.parameter().map(str::to_owned), e.message().to_owned())
        })
        .collect()
}

/// Every refusal of `node`, read by `read`, which reads what it reads and gives no value of
/// its own.
fn refused_by(node: &Node, read: impl FnOnce(&mut RequestFields<'_>)) -> Problems {
    RequestFields::read(node, |fields| {
        read(fields);
        Some(())
    })
    .unwrap_err()
}

/// `node`, read by `read`, refused for nothing.
fn accepted(node: &Node, read: impl FnOnce(&mut RequestFields<'_>)) {
    RequestFields::read(node, |fields| {
        read(fields);
        Some(())
    })
    .unwrap();
}

fn session(path: &str) -> SessionInfo {
    SessionInfo {
        id: SessionId::parse("s1").unwrap(),
        path: path.to_owned(),
        revision: Revision::OPENED,
        transaction: false,
        reloaded: false,
    }
}

/// A result holding `Json` built from hostile strings.
struct Holding(Vec<String>);

impl ToWire for Holding {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("texts", Json::array(self.0.iter().map(|s| Json::str(s)))),
            (
                "nested",
                Json::object([(
                    "deeper",
                    Json::array([Json::object([("a", Json::string("x\ny".to_owned()))])]),
                )]),
            ),
        ])
    }
}

// --- RequestFields: the request's top ---------------------------------------------------------

#[test]
fn a_request_that_is_not_an_object_is_refused_for_every_kind() {
    for (text, kind) in [
        ("null", "null"),
        ("true", "true or false"),
        ("0", "a number"),
        (r#""s1""#, "a string"),
        ("[]", "an array"),
    ] {
        let node = parsed(text);
        let refused = refused_by(&node, |_| {});
        assert_eq!(
            said(&refused),
            [(None, format!("the request must be an object, got {kind}"))],
            "{text}"
        );
        assert_eq!(refused.total(), 1);
    }
}

#[test]
fn a_key_given_twice_is_refused_before_a_field_is_read() {
    // The reader refuses it, at the top and nested, so no field reads the first or the last.
    for text in [
        r#"{"session": "s1", "session": "s2"}"#,
        r#"{"facing": {"within": 1, "within": 2}}"#,
        r#"{"a": 1, "b": 2, "a": 1}"#,
    ] {
        let error = read(text.as_bytes()).unwrap_err();
        assert!(
            error.message.contains("written twice"),
            "{text}: {}",
            error.message
        );
    }
}

#[test]
fn an_empty_request_has_every_field_missing_in_the_order_read() {
    let node = parsed("{}");
    let refused = refused_by(&node, |fields| {
        assert_eq!(fields.session(), None);
        assert_eq!(fields.model(), None);
        assert_eq!(fields.u32("limit"), None);
        assert_eq!(fields.nullable_string("name"), None);
        assert_eq!(fields.nullable_feature_ids("features"), None);
        assert!(fields.nullable_object("facing", |_| Some(())).is_none());
    });
    let missing = ["session", "model", "limit", "name", "features", "facing"];
    assert_eq!(
        said(&refused),
        missing.map(|k| (Some(k.to_owned()), format!("{k} is missing")))
    );
    for e in refused.errors() {
        assert!(
            e.suggestion()
                .is_some_and(|s| s.contains("null where it allows"))
        );
    }
}

#[test]
fn an_empty_object_with_no_fields_asked_is_fine_and_a_nested_one_too() {
    let node = parsed(r#"{"facing": {}}"#);
    accepted(&node, |fields| {
        assert_eq!(
            fields.nullable_object("facing", |_| Some(())),
            Some(Some(()))
        );
    });
}

// --- RequestFields: what a refusal quotes--------------------------------------------------

#[test]
fn an_unknown_key_with_control_or_bidi_characters_is_quoted_safely() {
    // Quoted and escaped as the document's reader quotes it (`decode::field_name`), so no
    // ESC sequence, newline, or right-to-left override reaches a terminal or a log raw.
    // (It did, in the message and the parameter both, until break-it.)
    for key in [
        "\u{1b}[2J",
        "a\nb",
        "evil\u{202e}txt",
        "x\u{2028}y",
        "nul\u{0}",
    ] {
        let text = format!(r#"{{{}: 1}}"#, serde_json::to_string(key).unwrap());
        let node = parsed(&text);
        let refused = refused_by(&node, |_| {});
        let error = &refused.errors()[0];
        assert!(printable(error.message()), "{key:?}: {:?}", error.message());
        assert!(
            error.parameter().is_some_and(printable),
            "{key:?}: {:?}",
            error.parameter()
        );
    }
}

#[test]
fn a_long_unknown_key_is_cut_short_in_its_refusal() {
    // Cut at 40 characters, as the document's reader cuts it: echoed whole, twice (message
    // and parameter), a 1 MB key made a 2 MB error, and a hundred of them a 200 MB response.
    let key = "k".repeat(1 << 20);
    let node = parsed(&format!(r#"{{"{key}": 1}}"#));
    let refused = refused_by(&node, |_| {});
    let error = &refused.errors()[0];
    assert!(
        error.message().len() < 200,
        "{} bytes",
        error.message().len()
    );
    assert!(error.parameter().is_some_and(|p| p.len() < 200));
}

#[test]
fn an_empty_unknown_key_is_named() {
    // Named `""`: as written, the key `""` at the top made an empty path, so the refusal
    // had no parameter and its message was " isn't a field of this request".
    let node = parsed(r#"{"": 1}"#);
    let refused = refused_by(&node, |_| {});
    let error = &refused.errors()[0];
    assert_eq!(error.parameter(), Some(r#""""#));
    assert!(
        error.message().starts_with(r#""""#),
        "{:?}",
        error.message()
    );
}

#[test]
fn an_unknown_key_holding_a_dot_is_not_taken_for_a_nested_field() {
    // `{"facing.extra": 1}` and `{"facing": {"extra": 1}}` were refused with one
    // parameter, `facing.extra`, though one is a key at the top: a dotted path named two
    // places. A key that isn't a plain name is quoted (as `decode::field_name` does).
    let top = parsed(r#"{"facing.extra": 1}"#);
    let top = refused_by(&top, |_| {});
    let nested = parsed(r#"{"facing": {"extra": 1}}"#);
    let nested = refused_by(&nested, |fields| {
        assert_eq!(fields.nullable_object("facing", |_| Some(())), None);
    });
    assert_eq!(nested.errors()[0].parameter(), Some("facing.extra"));
    assert_ne!(top.errors()[0].parameter(), nested.errors()[0].parameter());
}

#[test]
fn a_long_number_is_cut_short_in_its_refusal() {
    // Cut at 40 characters, as the document's reader cuts it (`decode::shown_number`):
    // repeated whole, a 1 MB number made a 1 MB message.
    let digits = format!("1{}", "0".repeat(1 << 20));
    let node = parsed(&format!(r#"{{"limit": {digits}, "feature": {digits}}}"#));
    let refused = refused_by(&node, |fields| {
        assert_eq!(fields.u32("limit"), None);
        assert_eq!(fields.nullable_feature("feature"), None);
    });
    for e in refused.errors() {
        assert!(e.message().len() < 200, "{} bytes", e.message().len());
    }
}

#[test]
fn a_long_or_hostile_string_value_is_cut_short_and_escaped_in_its_refusal() {
    let long = "s".repeat(1 << 20);
    for value in [long.as_str(), "s1\u{1b}[2J", "\u{202e}s1", "s1\n"] {
        let json = serde_json::to_string(value).unwrap();
        let node = parsed(&format!(
            r#"{{"session": {json}, "model": {json}, "feature": {json}, "features": [{json}]}}"#
        ));
        let refused = refused_by(&node, |fields| {
            assert_eq!(fields.session(), None);
            assert_eq!(fields.model(), None);
            assert_eq!(fields.nullable_feature("feature"), None);
            assert_eq!(fields.nullable_feature_ids("features"), None);
        });
        assert_eq!(refused.total(), 4);
        for e in refused.errors() {
            assert!(e.message().len() < 200, "{value:?}: {}", e.message().len());
            assert!(printable(e.message()), "{:?}", e.message());
        }
    }
}

// --- RequestFields: numbers----------------------------------------------------------------

#[test]
fn a_whole_number_is_refused_in_every_spelling_but_its_digits() {
    for text in [
        "4294967296",
        "18446744073709551616",
        "99999999999999999999999999999999",
        "1e3",
        "1E3",
        "1e+3",
        "0e0",
        "1.0",
        "4294967295.0",
        "-0",
        "-0.0",
        "-1",
        "0.5",
    ] {
        let node = parsed(&format!(r#"{{"limit": {text}}}"#));
        let refused = refused_by(&node, |fields| {
            assert_eq!(fields.u32("limit"), None, "{text}");
        });
        assert_eq!(
            said(&refused),
            [(
                Some("limit".to_owned()),
                format!("limit must be a whole number from 0 to 4294967295, got {text}")
            )]
        );
    }
    // Spellings JSON doesn't have never reach a field.
    for text in ["01", "+1", "00", ".5", "1.", "0x10", "Infinity", "NaN"] {
        assert!(
            read(format!(r#"{{"limit": {text}}}"#).as_bytes()).is_err(),
            "{text}"
        );
    }
    for (text, want) in [("0", 0), ("4294967295", u32::MAX), ("2147483648", 1 << 31)] {
        let node = parsed(&format!(r#"{{"limit": {text}}}"#));
        accepted(&node, |fields| {
            assert_eq!(fields.u32("limit"), Some(want));
        });
    }
}

#[test]
fn a_whole_number_of_another_kind_says_what_it_was() {
    for (text, got) in [
        ("null", "null"),
        ("true", "true or false"),
        (r#""7""#, r#""7""#),
        ("[7]", "an array"),
        ("{}", "an object"),
    ] {
        let node = parsed(&format!(r#"{{"limit": {text}}}"#));
        let refused = refused_by(&node, |fields| {
            assert_eq!(fields.u32("limit"), None);
        });
        assert_eq!(
            refused.errors()[0].message(),
            format!("limit must be a whole number from 0 to 4294967295, got {got}")
        );
    }
}

// --- RequestFields: sessions, models, features---------------------------------------------

#[test]
fn a_session_id_is_refused_in_every_spelling_but_its_own() {
    for bad in [
        "s01",
        "s00",
        &format!("s{}", "9".repeat(30)),
        "s18446744073709551616",
        "S1",
        " s1",
        "s1 ",
        "s+1",
        "s\u{661}",
        "s１",
        "ss1",
        "s1s",
        "s\u{0}1",
        "\u{feff}s1",
        "s1\u{200b}",
    ] {
        assert_eq!(SessionId::parse(bad), None, "{bad:?}");
        let node = parsed(&format!(
            r#"{{"session": {}}}"#,
            serde_json::to_string(bad).unwrap()
        ));
        let refused = refused_by(&node, |fields| {
            assert_eq!(fields.session(), None, "{bad:?}");
        });
        let error = &refused.errors()[0];
        assert_eq!(error.parameter(), Some("session"));
        assert_eq!(error.suggestion(), Some("give the ID open returned"));
        assert!(printable(error.message()), "{:?}", error.message());
    }
    // The largest number is an ID; it names no session open, which the session says.
    assert_eq!(
        SessionId::parse("s18446744073709551615").map(SessionId::number),
        Some(u64::MAX)
    );
    // An ID written as a number or null isn't one.
    for (text, got) in [("1", "1"), ("null", "null"), (r#"["s1"]"#, "an array")] {
        let node = parsed(&format!(r#"{{"session": {text}}}"#));
        let refused = refused_by(&node, |fields| {
            assert_eq!(fields.session(), None);
        });
        assert_eq!(
            refused.errors()[0].message(),
            format!("session must be a session's ID, s1, s2, ..., got {got}")
        );
    }
}

#[test]
fn a_session_id_round_trips_through_its_text() {
    for n in [1u64, 9, 10, 99, 1 << 32, u64::MAX] {
        let text = format!("s{n}");
        let id = SessionId::parse(&text).unwrap();
        assert_eq!(id.number(), n);
        assert_eq!(
            write(&id.to_wire(), Layout::Canonical).unwrap(),
            format!("\"{text}\"")
        );
    }
}

#[test]
fn a_model_is_current_or_published_exactly() {
    for bad in [
        "Current",
        "PUBLISHED",
        " current",
        "current ",
        "current\u{0}",
        "",
        "latest",
    ] {
        let node = parsed(&format!(
            r#"{{"model": {}}}"#,
            serde_json::to_string(bad).unwrap()
        ));
        let refused = refused_by(&node, |fields| {
            assert_eq!(fields.model(), None, "{bad:?}");
        });
        assert_eq!(refused.errors()[0].candidates(), ["current", "published"]);
        assert!(printable(refused.errors()[0].message()));
    }
    let node = parsed(r#"{"model": null}"#);
    let refused = refused_by(&node, |fields| assert_eq!(fields.model(), None));
    assert_eq!(
        refused.errors()[0].message(),
        "model must be current or published, got null"
    );
}

#[test]
fn a_list_of_features_names_each_member_at_fault_by_its_place() {
    let node = parsed(r#"{"features": [[], {}, null, "k7f2qx3m", "K7F2QX3M", "k7f2qx3"]}"#);
    let refused = refused_by(&node, |fields| {
        assert_eq!(fields.nullable_feature_ids("features"), None);
    });
    assert_eq!(
        said(&refused),
        [
            (0, "an array"),
            (1, "an object"),
            (2, "null"),
            (4, r#""K7F2QX3M""#),
            (5, r#""k7f2qx3""#)
        ]
        .map(|(i, got)| (
            Some(format!("features.{i}")),
            format!("features.{i} must be a feature's ID, got {got}")
        ))
    );
    // An empty list is a list.
    let node = parsed(r#"{"features": []}"#);
    accepted(&node, |fields| {
        assert_eq!(
            fields.nullable_feature_ids("features"),
            Some(Some(Vec::new()))
        );
    });
    // Not a list.
    // Not a list: what it was is said, as every refusal says it.
    let node = parsed(r#"{"features": "k7f2qx3m"}"#);
    let refused = refused_by(&node, |fields| {
        assert_eq!(fields.nullable_feature_ids("features"), None);
    });
    assert_eq!(
        refused.errors()[0].message(),
        r#"features must be a list of features' IDs or null, got "k7f2qx3m""#
    );
}

#[test]
fn a_nested_object_that_is_not_one_is_refused_at_its_path() {
    let node = parsed(r#"{"facing": [1], "within": {"facing": 3}}"#);
    let refused = refused_by(&node, |fields| {
        assert!(fields.nullable_object("facing", |_| Some(())).is_none());
        let within = fields.nullable_object("within", |within| {
            within.nullable_object("facing", |_| Some(()))
        });
        assert!(within.is_none());
    });
    assert_eq!(refused.total(), 2);
    assert_eq!(
        said(&refused),
        [
            (
                Some("facing".to_owned()),
                "facing must be an object, got an array".to_owned()
            ),
            (
                Some("within.facing".to_owned()),
                "within.facing must be an object, got a number".to_owned()
            ),
        ]
    );
}

// --- RequestFields: counting refusals------------------------------------------------------

#[test]
fn more_than_a_hundred_unknown_keys_are_listed_to_a_hundred_and_all_counted() {
    let members: Vec<String> = (0..250).map(|i| format!(r#""k{i}": {i}"#)).collect();
    let node = parsed(&format!("{{{}}}", members.join(", ")));
    let refused = refused_by(&node, |_| {});
    assert_eq!(refused.total(), 250);
    assert_eq!(refused.errors().len(), PROBLEMS_LISTED);
    for (i, e) in refused.errors().iter().enumerate() {
        assert_eq!(e.parameter(), Some(format!("k{i}").as_str()));
    }
    // And the envelope says so.
    let refused: Response<Holding> = Response {
        session: None,
        model: None,
        result: Err(refused),
    };
    let value: serde_json::Value =
        serde_json::from_str(&write(&response(&refused), Layout::OneLine).unwrap()).unwrap();
    assert_eq!(value["total"], 250);
    assert_eq!(value["errors"].as_array().unwrap().len(), 100);
    assert_eq!(value["errors"][99]["parameter"], "k99");
}

#[test]
fn refusals_at_the_top_and_nested_are_counted_together_in_the_order_found() {
    // 60 bad members of a list, then 70 unknown keys in a nested object, then 30 at the top.
    let bad: Vec<String> = (0..60).map(|i| i.to_string()).collect();
    let inner: Vec<String> = (0..70).map(|i| format!(r#""i{i}": 0"#)).collect();
    let outer: Vec<String> = (0..30).map(|i| format!(r#""o{i}": 0"#)).collect();
    let text = format!(
        r#"{{"features": [{}], "facing": {{{}}}, {}}}"#,
        bad.join(", "),
        inner.join(", "),
        outer.join(", ")
    );
    let node = parsed(&text);
    let refused = refused_by(&node, |fields| {
        assert_eq!(fields.nullable_feature_ids("features"), None);
        assert_eq!(fields.nullable_object("facing", |_| Some(())), None);
    });
    assert_eq!(refused.total(), 160);
    let listed: Vec<String> = refused
        .errors()
        .iter()
        .map(|e| e.parameter().unwrap().to_owned())
        .collect();
    let want: Vec<String> = (0..60)
        .map(|i| format!("features.{i}"))
        .chain((0..40).map(|i| format!("facing.i{i}")))
        .collect();
    assert_eq!(listed, want);
}

#[test]
fn a_flood_of_refusals_keeps_no_more_than_it_lists() {
    // A refusal past the hundredth is only counted. Every refusal was kept until `done`,
    // which then listed a hundred: an 8 MB request of a list of 4 million bad members
    // (within the reader's 256 MiB tree) made 4 million errors, a 1.4 GB peak (measured,
    // release), to list a hundred.
    let items = vec!["1"; 20_000].join(",");
    let node = parsed(&format!(r#"{{"features": [{items}]}}"#));
    let refused = refused_by(&node, |fields| {
        assert_eq!(fields.nullable_feature_ids("features"), None);
        assert!(
            fields.refused.listed.len() <= PROBLEMS_LISTED,
            "{} refusals held",
            fields.refused.listed.len()
        );
    });
    assert_eq!(refused.total(), 20_000);
    assert_eq!(refused.errors().len(), PROBLEMS_LISTED);
}

#[test]
fn a_refusal_past_the_hundredth_is_counted_but_never_made() {
    // Each refusal made past the hundredth was made only to be dropped: a request of 4
    // million bad values formatted 4 million messages and paths to list a hundred.
    let mut refusals = Refusals::default();
    let mut made = 0;
    for i in 0..150 {
        refusals.refuse_with(|| {
            made += 1;
            Error::new(ErrorCode::InvalidInput, i.to_string())
        });
    }
    assert_eq!(made, PROBLEMS_LISTED);
    assert_eq!(
        (refusals.listed.len(), refusals.unlisted),
        (PROBLEMS_LISTED, 50)
    );
    assert_eq!(refusals.listed[99].message(), "99");
}

// --- Output text -----------------------------------------------------------------------

/// Strings no writer may let through raw.
fn hostile() -> Vec<String> {
    let mut all: Vec<String> = (0u32..0x20)
        .chain(0x7f..=0x9f)
        .chain([0x2028, 0x2029, 0x61c, 0x200e, 0x200f])
        .chain(0x202a..=0x202e)
        .chain(0x2066..=0x2069)
        .filter_map(char::from_u32)
        .map(|c| format!("a{c}b"))
        .collect();
    all.extend(
        [
            r#"quote " and backslash \ and both \""#,
            "\\u2028 written out",
            "</script>",
            "emoji 🧪, combining e\u{301}, astral 𝄞",
            "",
        ]
        .map(str::to_owned),
    );
    all
}

#[test]
fn hostile_strings_are_written_escaped_and_read_back_exactly_in_both_layouts() {
    let texts = hostile();
    let held = Holding(texts.clone());
    for layout in [Layout::Canonical, Layout::OneLine] {
        let text = write(&held.to_wire(), layout).unwrap();
        for c in text.chars() {
            assert!(
                c == '\n' && layout == Layout::Canonical || printable(&c.to_string()),
                "{layout:?}: {c:?} written raw"
            );
        }
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let back: Vec<&str> = value["texts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(back, texts.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(value["nested"]["deeper"][0]["a"], "x\ny");
        // Silico's own reader takes it too.
        assert!(read(text.as_bytes()).is_ok());
    }
}

#[test]
fn an_error_with_hostile_text_in_every_field_is_written_on_one_line() {
    let evil = "line\nbreak\r\u{2028}\u{2029}\u{202e}\u{1b}[31m \"quoted\" \\";
    let error = Error::new(ErrorCode::InvalidInput, evil)
        .with_feature(evil)
        .with_elements([evil, evil])
        .with_elements_total(5)
        .with_parameter(evil)
        .with_cause(evil)
        .with_suggestion(evil)
        .with_candidates([evil]);
    let refused: Response<Holding> = Response {
        session: Some(session(evil)),
        model: Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Current,
        }),
        result: Err(error.into()),
    };
    let text = write(&response(&refused), Layout::OneLine).unwrap();
    assert!(printable(&text), "{text}");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let e = &value["errors"][0];
    for key in ["message", "feature", "parameter", "cause", "suggestion"] {
        assert_eq!(e[key], evil, "{key}");
    }
    assert_eq!(e["elements"], serde_json::json!([evil, evil]));
    assert_eq!(e["elements_total"], 5);
    assert_eq!(e["candidates"], serde_json::json!([evil]));
    assert_eq!(value["session"]["path"], evil);
}

#[test]
fn the_canonical_layout_puts_on_one_line_only_what_holds_no_object() {
    // M2.1's D7, by hand: an object or array holding no object at any depth on one line,
    // anything else a member to a line, two spaces deep; `{}` is an object.
    let value = Json::object([
        (
            "flat",
            Json::object([("a", Json::whole(1)), ("b", Json::array([]))]),
        ),
        (
            "grid",
            Json::array([
                Json::array([Json::whole(1), Json::whole(2)]),
                Json::array([]),
            ]),
        ),
        ("empty", Json::object([])),
        (
            "holds",
            Json::array([Json::array([Json::object([("x", Json::null())])])]),
        ),
        ("holds_empty", Json::array([Json::object([])])),
    ]);
    assert_eq!(
        write(&value, Layout::Canonical).unwrap(),
        r#"{
  "flat": {"a": 1, "b": []},
  "grid": [[1, 2], []],
  "empty": {},
  "holds": [
    [
      {"x": null}
    ]
  ],
  "holds_empty": [
    {}
  ]
}"#
    );
    assert_eq!(
        write(&value, Layout::OneLine).unwrap(),
        r#"{"flat": {"a": 1, "b": []}, "grid": [[1, 2], []], "empty": {}, "holds": [[{"x": null}]], "holds_empty": [{}]}"#
    );
    // A value that holds no object is one line either way, and no trailing newline.
    let flat = Json::object([("a", Json::str("b"))]);
    assert_eq!(write(&flat, Layout::Canonical).unwrap(), r#"{"a": "b"}"#);
    assert_eq!(write(&Json::object([]), Layout::Canonical).unwrap(), "{}");
    assert_eq!(write(&Json::null(), Layout::OneLine).unwrap(), "null");
}

#[test]
fn deep_nesting_on_one_line_has_no_newline_and_reads_back() {
    let mut value = Json::object([("leaf", Json::str("x"))]);
    for _ in 0..60 {
        value = Json::array([Json::object([("in", value)])]);
    }
    let one = write(&value, Layout::OneLine).unwrap();
    assert!(!one.contains('\n'));
    let canonical = write(&value, Layout::Canonical).unwrap();
    assert!(!canonical.ends_with('\n'));
    let a: serde_json::Value = serde_json::from_str(&one).unwrap();
    let b: serde_json::Value = serde_json::from_str(&canonical).unwrap();
    assert_eq!(a, b);
}

// --- The envelope ----------------------------------------------------------------------

fn validator() -> jsonschema::Validator {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/contract/commands/envelope.json");
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    jsonschema::validator_for(&schema).unwrap()
}

fn full_error() -> Error {
    // The error contract's full example (`tests/contract/error-full.json`).
    Error::new(ErrorCode::KernelFailed, "fillet failed")
        .with_feature("Fillet1")
        .with_element("Edge3")
        .with_element("Edge5")
        .with_parameter("radius")
        .with_cause("StdFail_NotDone: BRepFilletAPI_MakeFillet")
        .with_suggestion("reduce the radius below 5 mm")
        .with_candidates(["Edge2", "Edge4"])
}

/// A tree's members and items, in the order written, as text: positions dropped.
fn shape(node: &Node) -> String {
    match &node.value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.clone(),
        Value::String(s) => format!("{s:?}"),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(shape).collect::<Vec<_>>().join(",")
        ),
        Value::Object(members) => format!(
            "{{{}}}",
            members
                .iter()
                .map(|m| format!("{:?}:{}", m.key, shape(&m.value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

#[test]
fn an_error_with_every_field_is_the_error_contracts_shape_in_its_order() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/contract/error-full.json");
    let contract = std::fs::read(path).unwrap();
    let error = full_error();
    for layout in [Layout::Canonical, Layout::OneLine] {
        let text = write(&error.to_wire(), layout).unwrap();
        assert_eq!(
            shape(&read(text.as_bytes()).unwrap()),
            shape(&read(&contract).unwrap())
        );
    }
}

#[test]
fn every_shape_of_the_envelope_holds_to_its_snapshot() {
    let validator = validator();
    let full = full_error();
    let elided = Error::new(ErrorCode::ReferenceBroken, "m")
        .with_elements(["a"])
        .with_elements_total(u64::MAX);
    let many: Vec<Error> = (0..150)
        .map(|i| Error::new(ErrorCode::Internal, i.to_string()))
        .collect();
    let read_model = ModelRead {
        revision: Revision::OPENED,
        of: ModelChoice::Current,
    };
    let texts = [
        response(&Response {
            session: None,
            model: None,
            result: Ok(Holding(vec!["x".to_owned()])),
        }),
        response(&Response {
            session: Some(session("a b/ç.silico")),
            model: Some(read_model),
            result: Ok(Holding(Vec::new())),
        }),
        response(&Response::<Holding> {
            session: None,
            model: None,
            result: Err(full.clone().into()),
        }),
        response(&Response::<Holding> {
            session: Some(session("p")),
            model: Some(ModelRead {
                revision: Revision::OPENED,
                of: ModelChoice::Published,
            }),
            result: Err(Problems::of(vec![full, elided])),
        }),
        response(&Response::<Holding> {
            session: None,
            model: None,
            result: Err(Problems::of(many)),
        }),
    ]
    .map(|json| {
        [
            write(&json, Layout::Canonical).unwrap(),
            write(&json, Layout::OneLine).unwrap(),
        ]
    });
    for text in texts.iter().flatten() {
        let value: serde_json::Value = serde_json::from_str(text).unwrap();
        let errors: Vec<String> = validator
            .iter_errors(&value)
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "{text}: {errors:?}");
    }
}

#[test]
fn the_envelopes_snapshot_refuses_what_no_response_is() {
    // The schema can fail: each of these breaks one rule of section 2.
    let validator = validator();
    let s = r#"{"id": "s1", "path": "p", "revision": 1, "transaction": false, "reloaded": false}"#;
    let e = r#"{"code": "internal", "message": "m", "feature": null, "elements": [], "elements_total": 0, "parameter": null, "cause": null, "suggestion": null, "candidates": []}"#;
    for bad in [
        r#"{"ok": "true", "session": null, "model": null, "result": 1}"#.to_owned(),
        r#"{"ok": true, "session": null, "model": null}"#.to_owned(),
        r#"{"ok": false, "session": null, "model": null, "result": 1}"#.to_owned(),
        format!(r#"{{"ok": false, "session": null, "model": null, "errors": [{e}]}}"#),
        format!(r#"{{"ok": false, "session": null, "model": null, "errors": [{e}], "total": -1}}"#),
        r#"{"ok": false, "session": null, "model": null, "errors": [{}], "total": 1}"#.to_owned(),
        format!(
            r#"{{"ok": true, "session": {s}, "model": {{"revision": 1, "of": "latest", "fault": null}}, "result": 1}}"#
        ),
        r#"{"ok": true, "session": {"id": "s1"}, "model": null, "result": 1}"#.to_owned(),
        r#"[]"#.to_owned(),
        // A field no response has, at the top and in each part (`wire_shape`'s
        // `additionalProperties`).
        r#"{"ok": true, "session": null, "model": null, "result": 1, "extra": 1}"#.to_owned(),
        r#"{"ok": false, "session": null, "model": null, "errors": [], "total": 0, "result": 1}"#
            .to_owned(),
        format!(
            r#"{{"ok": true, "session": {}, "model": null, "result": 1}}"#,
            s.replace('}', r#", "extra": 1}"#)
        ),
        r#"{"ok": true, "session": null, "model": {"revision": 1, "of": "current", "fault": null, "extra": 1}, "result": 1}"#.to_owned(),
        format!(
            r#"{{"ok": false, "session": null, "model": null, "errors": [{}], "total": 1}}"#,
            e.replace("[]}", r#"[], "extra": 1}"#)
        ),
        // Past the snapshot's bounds: a refusal listing 101 errors, and revision 0.
        format!(
            r#"{{"ok": false, "session": null, "model": null, "errors": [{}], "total": 101}}"#,
            vec![e; 101].join(", ")
        ),
        format!(
            r#"{{"ok": true, "session": {}, "model": null, "result": 1}}"#,
            s.replace(r#""revision": 1"#, r#""revision": 0"#)
        ),
        r#"{"ok": true, "session": null, "model": {"revision": 0, "of": "current", "fault": null}, "result": 1}"#.to_owned(),
    ] {
        let value: serde_json::Value = serde_json::from_str(&bad).unwrap();
        assert!(!validator.is_valid(&value), "{bad}");
    }
    // And the bounds themselves are held to: 100 errors, and revision 1.
    let hundred = format!(
        r#"{{"ok": false, "session": {s}, "model": null, "errors": [{}], "total": 100}}"#,
        vec![e; 100].join(", ")
    );
    assert!(validator.is_valid(&serde_json::from_str::<serde_json::Value>(&hundred).unwrap()));
}

#[test]
fn the_envelopes_snapshot_requires_every_field_a_response_always_has() {
    // BREAKS: section 2 and the error contract give every field always, `null` where it
    // doesn't apply, but the snapshot, generated from `Option` fields, requires none of
    // `session`, `model`, `fault`, or an error's `feature`, `parameter`, `cause`, and
    // `suggestion`: a response missing them holds to it, and a client generated from it
    // takes them for optional. (Fixing it changes the protected `envelope.json`.)
    let validator = validator();
    let e = r#""code": "internal", "message": "m", "elements": [], "elements_total": 0, "candidates": []"#;
    for bad in [
        r#"{"ok": true, "result": 1}"#.to_owned(),
        r#"{"ok": true, "session": null, "result": 1}"#.to_owned(),
        r#"{"ok": true, "session": null, "model": {"revision": 1, "of": "current"}, "result": 1}"#
            .to_owned(),
        format!(
            r#"{{"ok": false, "session": null, "model": null, "errors": [{{{e}}}], "total": 1}}"#
        ),
    ] {
        let value: serde_json::Value = serde_json::from_str(&bad).unwrap();
        assert!(!validator.is_valid(&value), "{bad}");
    }
}

#[test]
fn a_results_problems_list_a_hundred_and_count_the_unlisted() {
    let listed: Vec<Error> = (0..100)
        .map(|i| Error::new(ErrorCode::Internal, i.to_string()))
        .collect();
    let given = Problems::with_unlisted(listed, 7);
    let (errors, total) = problems(&given);
    let errors: serde_json::Value =
        serde_json::from_str(&write(&errors, Layout::OneLine).unwrap()).unwrap();
    assert_eq!(errors.as_array().unwrap().len(), 100);
    assert_eq!(write(&total, Layout::OneLine).unwrap(), "107");
}

// --- The document writer ---------------------------------------------------------------

#[test]
fn writing_on_one_line_leaves_the_documents_writer_as_it_was() {
    // A document written after a one-line response is written as before it: the flag is
    // the writer's own, never shared.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/example.silico");
    let file = std::fs::read_to_string(path).unwrap();
    let document = crate::document::read(file.as_bytes()).unwrap();
    assert_eq!(crate::document::write(&document).unwrap(), file);
    let _ = write(&Holding(hostile()).to_wire(), Layout::OneLine).unwrap();
    assert_eq!(crate::document::write(&document).unwrap(), file);
}
