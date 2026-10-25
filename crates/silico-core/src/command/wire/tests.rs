//! The envelope's snapshot and the design's examples of it (section 2), byte for byte; and
//! the strict request reader. Expected texts are the design's own (docs/design/
//! m4.1-command-api.md), copied into `tests/contract/commands/examples/`, which is protected:
//! the writer must give them as they are.

use std::path::PathBuf;

use super::{Fields, Json, Layout, ToWire, response, snapshot, write};
use crate::command::{
    ModelChoice, ModelRead, Problems, Response, Revision, SessionId, SessionInfo,
};
use crate::document::json::read;
use crate::{Error, ErrorCode};

fn contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract/commands")
        .join(name)
}

/// The committed file `name`, or, with `SILICO_CONTRACT_RECORD` set, `text` written to it
/// first (the maintainer's to approve: the directory is protected).
fn committed(name: &str, text: &str) -> String {
    let path = contract(name);
    if std::env::var_os("SILICO_CONTRACT_RECORD").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn revision(n: u64) -> Revision {
    let mut r = Revision::OPENED;
    for _ in 1..n {
        r = r.next().unwrap();
    }
    r
}

fn session(revision_number: u64, transaction: bool) -> SessionInfo {
    SessionInfo {
        id: SessionId::parse("s1").unwrap(),
        path: PathBuf::from("bracket.silico"),
        revision: revision(revision_number),
        transaction,
        reloaded: false,
    }
}

/// `begin`'s result, as the design's first example shows it.
struct Began(Revision);

impl ToWire for Began {
    fn to_wire(&self) -> Json<'_> {
        Json::object([("base", self.0.to_wire())])
    }
}

/// `check`'s result with no problems and no warnings, as the second example shows it.
struct Clean;

impl ToWire for Clean {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("problems", Json::array([])),
            ("problems_total", Json::whole(0)),
            ("warnings", Json::array([])),
            ("warnings_total", Json::whole(0)),
        ])
    }
}

/// The design's three envelopes: a command that ran, one that read a model, and one
/// refused.
fn examples() -> [(&'static str, String); 3] {
    let ran = Response {
        session: Some(session(4, false)),
        model: None,
        result: Ok(Began(revision(4))),
    };
    let read_a_model = Response {
        session: Some(session(7, true)),
        model: Some(ModelRead {
            revision: revision(6),
            of: ModelChoice::Published,
        }),
        result: Ok(Clean),
    };
    let refused: Response<Clean> = Response {
        session: Some(session(4, true)),
        model: None,
        result: Err(Error::new(
            ErrorCode::InvalidInput,
            "a transaction is already open in this session",
        )
        .with_parameter("session")
        .with_suggestion("commit or roll back the open transaction, then begin again")
        .into()),
    };
    [
        (
            "examples/envelope/ran.json",
            write(&response(&ran), Layout::Canonical),
        ),
        (
            "examples/envelope/read-a-model.json",
            write(&response(&read_a_model), Layout::Canonical),
        ),
        (
            "examples/envelope/refused.json",
            write(&response(&refused), Layout::Canonical),
        ),
    ]
}

#[test]
fn the_envelopes_schema_is_the_committed_snapshot() {
    let text = snapshot("envelope").unwrap().unwrap();
    assert_eq!(text, committed("envelope.json", &text));
    assert!(snapshot("no_such_command").is_none());
}

#[test]
fn the_designs_envelopes_are_written_byte_for_byte_and_hold_to_the_snapshot() {
    let schema: serde_json::Value = serde_json::from_str(&committed(
        "envelope.json",
        &snapshot("envelope").unwrap().unwrap(),
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for (name, text) in examples() {
        // The file holds the response and a newline, as the error contract's files do.
        let file = committed(name, &format!("{text}\n"));
        assert_eq!(format!("{text}\n"), file, "{name}");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let errors: Vec<String> = validator
            .iter_errors(&value)
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "{name}: {errors:?}");
    }
}

#[test]
fn on_one_line_an_envelope_has_the_same_members_in_the_same_order() {
    let response = Response {
        session: Some(session(4, false)),
        model: None,
        result: Ok(Began(revision(4))),
    };
    assert_eq!(
        write(&super::response(&response), Layout::OneLine),
        r#"{"ok": true, "session": {"id": "s1", "path": "bracket.silico", "revision": 4, "transaction": false, "reloaded": false}, "model": null, "result": {"base": 4}}"#
    );
}

#[test]
fn a_refusal_lists_its_first_hundred_errors_and_counts_them_all() {
    let errors: Vec<Error> = (0..150)
        .map(|i| Error::new(ErrorCode::InvalidInput, format!("problem {i}")))
        .collect();
    let refused: Response<Clean> = Response {
        session: None,
        model: None,
        result: Err(Problems::of(errors)),
    };
    let value: serde_json::Value =
        serde_json::from_str(&write(&response(&refused), Layout::Canonical)).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["session"], serde_json::Value::Null);
    assert_eq!(value["total"], 150);
    // The first hundred, in order.
    let listed = value["errors"].as_array().unwrap();
    assert_eq!(listed.len(), 100);
    assert_eq!(listed[0]["message"], "problem 0");
    assert_eq!(listed[99]["message"], "problem 99");
}

#[test]
fn a_session_id_is_s_and_a_number_from_1() {
    assert_eq!(SessionId::parse("s1").map(SessionId::number), Some(1));
    assert_eq!(SessionId::parse("s42").map(SessionId::number), Some(42));
    for bad in [
        "",
        "s",
        "s0",
        "s01",
        "1",
        "S1",
        "s-1",
        "s1.0",
        "s 1",
        "s18446744073709551616",
    ] {
        assert_eq!(SessionId::parse(bad), None, "{bad:?}");
    }
}

/// What a request's fields refuse: each error's parameter and message.
fn refusals(problems: &Problems) -> Vec<(String, String)> {
    problems
        .errors()
        .iter()
        .map(|e| {
            assert_eq!(e.code(), ErrorCode::InvalidInput);
            (
                e.parameter().unwrap_or_default().to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

#[test]
fn a_request_is_read_strictly_and_every_reason_is_said_in_order() {
    let node =
        read(br#"{"session": "s0", "model": "latest", "limit": 1.5, "feature": 7, "extra": true}"#)
            .unwrap();
    let mut fields = Fields::of(&node, "").unwrap();
    assert_eq!(fields.session(), None);
    assert_eq!(fields.model(), None);
    assert_eq!(fields.u32("limit"), None);
    assert_eq!(fields.nullable_feature("feature"), None);
    assert_eq!(fields.bool("write"), None);
    let refused = fields.done().unwrap_err();
    assert_eq!(
        refusals(&refused),
        [
            (
                "session".to_owned(),
                r#"session must be a session's ID, s1, s2, ..., got "s0""#.to_owned()
            ),
            (
                "model".to_owned(),
                r#"model must be current or published, got "latest""#.to_owned()
            ),
            (
                "limit".to_owned(),
                "limit must be a whole number from 0 to 4294967295, got 1.5".to_owned()
            ),
            (
                "feature".to_owned(),
                "feature must be a feature's ID or null, got 7".to_owned()
            ),
            ("write".to_owned(), "write is missing".to_owned()),
            (
                "extra".to_owned(),
                "extra isn't a field of this request".to_owned()
            ),
        ]
    );
}

#[test]
fn fields_of_their_types_are_read_and_null_only_where_the_shape_allows_it() {
    let node = read(
        br#"{"session": "s3", "model": "published", "limit": 1000, "feature": null, "features": ["k7f2qx3m", "a91cw0de"], "write": false, "name": null}"#,
    )
    .unwrap();
    let mut fields = Fields::of(&node, "").unwrap();
    assert_eq!(fields.session().map(SessionId::number), Some(3));
    assert_eq!(fields.model(), Some(ModelChoice::Published));
    assert_eq!(fields.u32("limit"), Some(1000));
    assert_eq!(fields.nullable_feature("feature"), Some(None));
    let ids = fields.nullable_features("features").unwrap().unwrap();
    assert_eq!(
        ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
        ["k7f2qx3m", "a91cw0de"]
    );
    assert_eq!(fields.bool("write"), Some(false));
    assert_eq!(fields.nullable_string("name"), Some(None));
    assert!(fields.done().is_ok());
    // A bool isn't null.
    let node = read(br#"{"write": null}"#).unwrap();
    let mut fields = Fields::of(&node, "").unwrap();
    assert_eq!(fields.bool("write"), None);
    assert_eq!(
        refusals(&fields.done().unwrap_err()),
        [(
            "write".to_owned(),
            "write must be true or false, got null".to_owned()
        )]
    );
}

#[test]
fn a_whole_number_is_digits_only_from_0_to_u32_max() {
    // (A leading zero, `01`, isn't JSON: the reader refuses it before a field is read.)
    for (text, want) in [
        ("0", Some(0)),
        ("1", Some(1)),
        ("4294967295", Some(u32::MAX)),
        ("4294967296", None),
        ("-1", None),
        ("1e3", None),
        ("1.0", None),
        ("-0", None),
    ] {
        let doc = format!(r#"{{"limit": {text}}}"#);
        let node = read(doc.as_bytes()).unwrap();
        let mut fields = Fields::of(&node, "").unwrap();
        assert_eq!(fields.u32("limit"), want, "{text}");
    }
}

#[test]
fn a_list_member_and_a_nested_field_are_named_by_their_paths() {
    let node =
        read(br#"{"features": ["k7f2qx3m", 5, "NOT AN ID"], "facing": {"within": 1, "extra": 2}}"#)
            .unwrap();
    let mut fields = Fields::of(&node, "").unwrap();
    assert_eq!(fields.nullable_features("features"), None);
    let mut facing = fields.nullable_object("facing").unwrap().unwrap();
    assert_eq!(facing.u32("within"), Some(1));
    fields.absorb(facing);
    assert_eq!(
        refusals(&fields.done().unwrap_err()),
        [
            (
                "features.1".to_owned(),
                "features.1 must be a feature's ID, got 5".to_owned()
            ),
            (
                "features.2".to_owned(),
                r#"features.2 must be a feature's ID, got "NOT AN ID""#.to_owned()
            ),
            (
                "facing.extra".to_owned(),
                "facing.extra isn't a field of this request".to_owned()
            ),
        ]
    );
    // The request itself must be an object.
    let node = read(b"[1]").unwrap();
    let refused = Fields::of(&node, "").err().unwrap();
    assert_eq!(
        refused.errors()[0].message(),
        "the request must be an object, got an array"
    );
}

/// The design's JSON code blocks between the headings `from` and `to`, in order.
fn design_examples(from: &str, to: &str) -> Vec<String> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m4.1-command-api.md");
    let doc = std::fs::read_to_string(&path).unwrap();
    let section = &doc[doc.find(from).unwrap()..doc.find(to).unwrap()];
    section
        .split("```json\n")
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap().to_owned())
        .collect()
}

#[test]
fn the_envelopes_examples_are_the_designs_own_text() {
    // The contract's examples and the design's are one text: section 2's three envelopes,
    // in order, each file the block and a newline.
    let design = design_examples("### The response", "### Layout and key order");
    let files = ["ran", "read-a-model", "refused"].map(|name| {
        std::fs::read_to_string(contract(&format!("examples/envelope/{name}.json"))).unwrap()
    });
    assert_eq!(design.len(), files.len());
    for (block, file) in design.iter().zip(&files) {
        assert_eq!(format!("{block}\n"), *file);
    }
}
