//! Exact JSON both ways (docs/design/m4.9-mcp.md, *Exact JSON both ways*; M4.1 decision
//! 23): a call's arguments reach the core as the bytes sent (`0.1` exactly 100,000 nm, a
//! number with too many digits or a key given twice refused, not rounded or kept once), and
//! each answer's `structuredContent` is the core's one-line envelope byte for byte, as is
//! `content`'s text. Expected values: the same commands run on the core's
//! `Sessions::call` directly, in a workspace of their own; the kernel's recorded requests;
//! and lengths worked out by hand.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::value::RawValue;
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer};
use silico_core::kernel::{Op, Primitive};

use crate::client::{BOX, Client, HANG};

#[derive(Deserialize)]
struct Reply<'a> {
    #[serde(borrow)]
    result: Answer<'a>,
}

#[derive(Deserialize)]
struct Answer<'a> {
    content: Vec<Block>,
    #[serde(borrow, rename = "structuredContent")]
    structured: &'a RawValue,
    #[serde(rename = "isError")]
    is_error: bool,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

/// The answer's `structuredContent` as written, checked to be `content`'s one text block's
/// text, byte for byte; and `isError`.
fn structured(line: &str) -> (String, bool) {
    let reply: Reply<'_> = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
    let answer = reply.result;
    assert_eq!(answer.content.len(), 1, "{line}");
    assert_eq!(answer.content[0].kind, "text");
    assert_eq!(
        answer.content[0].text.as_deref(),
        Some(answer.structured.get())
    );
    (answer.structured.get().to_owned(), answer.is_error)
}

/// The calls, each `(name, arguments)`.
const CALLS: [(&str, &str); 7] = [
    ("open", r#"{"path": "part.silico", "create": false}"#),
    (
        "set_field",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 0.1}"#,
    ),
    (
        "set_field",
        r#"{"session": "s1", "revision": 2, "feature": "k7f2qx3m", "field": "width", "value": 0.000001}"#,
    ),
    ("text", r#"{"session": "s1", "feature": null}"#),
    (
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    ),
    (
        "mass_properties",
        r#"{"session": "s1", "model": "current", "feature": null}"#,
    ),
    ("close", r#"{"session": "s1"}"#),
];

#[test]
fn each_answer_is_the_cores_one_line_envelope_byte_for_byte() {
    // The core's own answers, on sessions of their own.
    let dir = tempfile::tempdir().unwrap();
    let work = std::fs::canonicalize(dir.path()).unwrap().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("part.silico"), BOX).unwrap();
    let secret = dir.path().join("secret");
    let sessions = Sessions::with_cache_opener(Arc::new(FakeKernel::new(2).unwrap()), move |c| {
        Cache::with_secret(c, &secret)
    })
    .rendering(FakeRenderer::new())
    .in_workspace(&work)
    .unwrap();
    let core: Vec<(String, bool)> = CALLS
        .iter()
        .map(|(name, arguments)| match sessions.call(name, arguments) {
            Call::Running(calling) => {
                let answered = calling.wait_timeout(HANG).ok().unwrap();
                (answered.text, !answered.ok)
            }
            Call::Unknown => panic!("{name}"),
        })
        .collect();
    let mut c = Client::initialized();
    c.file("part.silico", BOX);
    for (at, ((name, arguments), core)) in CALLS.iter().zip(&core).enumerate() {
        let id = at as u64 + 1;
        c.send_call(id, name, arguments);
        let (line, _) = c.reply_line(&id.to_string());
        let (text, is_error) = structured(&line);
        assert_eq!((&text, is_error), (&core.0, core.1), "{name}");
        // In schema order, never sorted, and a nanometre written as the document writes it.
        assert!(text.starts_with(r#"{"ok": true, "session": "#), "{text}");
        assert!(!text.contains("1e-6"), "{text}");
    }
    let width = &core[2].0;
    assert!(width.contains(r#""after": 0.000001,"#), "{width}");
}

#[test]
fn a_number_reaches_the_core_as_its_digits_say_never_rounded() {
    let mut c = Client::initialized();
    c.file("part.silico", BOX);
    c.open(1, "part.silico");
    // More decimals than a nanometre's: refused for them, not rounded to 0.1.
    let refused = c.envelope(
        2,
        "set_field",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 0.10000000000000000001}"#,
    );
    assert_eq!(refused["ok"], false);
    let message = refused["errors"][0]["message"].as_str().unwrap();
    assert!(message.contains("has more than 6 decimals"), "{message}");
    assert_eq!(c.read_file("part.silico"), BOX);
    // `0.1` is 100,000 nm exactly: saved as 0.1, and sent to the kernel as the double
    // nearest 100,000 nm in millimetres, which is 0.1's.
    let set = c.envelope(
        3,
        "set_field",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 0.1}"#,
    );
    assert_eq!(set["ok"], true, "{set}");
    assert!(c.read_file("part.silico").contains(r#""length": 0.1,"#));
    let sent: Vec<f64> = c
        .kernel
        .calls()
        .iter()
        .filter_map(|call| match call.op {
            Op::MakePrimitive(Primitive::Box { dx, .. }) => Some(dx),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [100_000.0 / 1e6]);
}

#[test]
fn a_key_given_twice_or_nesting_past_the_cores_limit_is_refused_by_the_core() {
    let mut c = Client::initialized();
    let twice = c.envelope(1, "schema", r#"{"of": "document", "of": "open"}"#);
    assert_eq!(twice["ok"], false);
    assert_eq!(twice["errors"][0]["code"], "invalid_input");
    let message = twice["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.starts_with("the request isn't valid JSON: the key \"of\" is written twice"),
        "{message}"
    );
    // 64 deep is read (and refused for its shape); 65 is refused by Silico's reader.
    let nested = |depth: usize| {
        format!(
            r#"{{"of": {}{}}}"#,
            "[".repeat(depth - 1),
            "]".repeat(depth - 1)
        )
    };
    let at_most = c.envelope(2, "schema", &nested(64));
    assert_eq!(at_most["errors"][0]["parameter"], "of");
    let past = c.envelope(3, "schema", &nested(65));
    assert_eq!(
        past["errors"][0]["message"],
        "the request isn't valid JSON: arrays and objects nest more than 64 deep, at line 1, column 71"
    );
}

#[test]
fn renders_png_is_an_image_block_beside_the_envelope_never_in_it() {
    let mut c = Client::initialized();
    c.file("part.silico", BOX);
    c.open(1, "part.silico");
    c.send_call(
        2,
        "render",
        r#"{"session": "s1", "model": "current", "view": "front", "camera": null, "width": 40, "height": 30, "highlight": [], "to": null, "replace": false}"#,
    );
    let (line, _) = c.reply_line("2");
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    let result = &reply["result"];
    assert_eq!(result["isError"], false, "{line}");
    let content = result["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["mimeType"], "image/png");
    // The fake renderer's stand-in, `PNG 40x30`, in base64 worked out by hand: "PNG" is
    // UE5H, " 40" IDQw, and "x30" eDMw.
    assert_eq!(content[1]["data"], "UE5HIDQweDMw");
    let envelope = &result["structuredContent"];
    assert_eq!(envelope["result"]["png_bytes"], "PNG 40x30".len());
    assert!(!content[0]["text"].as_str().unwrap().contains("iVBOR"));
}
