//! For the tests of pad, pocket, revolve, and groove (M3.6): documents of them from text.
#![allow(dead_code)]

use silico_core::document::{Checked, FeatureId, Invalid, read, validate};
use silico_core::{Error, ErrorCode};

pub fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

/// A sketch with nothing in it, on `plane`.
pub fn empty_sketch(id: &str, name: &str, plane: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "{name}", "plane": "{plane}", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}}"#
    )
}

/// A document of these features, each a JSON object's text.
pub fn document(features: &[&str]) -> String {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{}]}}",
        features.join(", ")
    )
}

pub fn checked(features: &[&str]) -> Result<(), Invalid> {
    validate(&read(document(features).as_bytes()).unwrap())
}

/// The one problem these features have.
pub fn problem(features: &[&str]) -> Error {
    let invalid = checked(features).unwrap_err();
    assert_eq!(invalid.total(), 1, "{:#?}", invalid.errors());
    invalid.errors()[0].clone()
}

#[track_caller]
pub fn assert_refused(
    error: &Error,
    feature: &str,
    parameter: &str,
    message: &str,
    suggestion: &str,
) {
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.feature(), Some(feature), "{error:?}");
    assert_eq!(error.parameter(), Some(parameter), "{error:?}");
    assert_eq!(error.message(), message);
    assert_eq!(error.suggestion(), Some(suggestion));
    assert!(error.elements().is_empty());
}

pub fn block(id: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}"#
    )
}

/// A sketch on XZ: points pa (0, 0) and pb (0, 10), the construction line na between them;
/// pc (5, 5), pd (6, 5), and pe (5, 6); the line nb from pc to pd, held horizontal by ka; a
/// circle ca about pc; an arc aa about pc from pd to pe.
pub const PROFILE: &str = r#"{"id": "s0000003", "type": "sketch", "v": 1, "name": "Profile", "plane": "xz", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 0, "y": 10, "construction": false}, {"id": "pc000000", "x": 5, "y": 5, "construction": false}, {"id": "pd000000", "x": 6, "y": 5, "construction": false}, {"id": "pe000000", "x": 5, "y": 6, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": true}, {"id": "nb000000", "from": "pc000000", "to": "pd000000", "construction": false}], "circles": [{"id": "ca000000", "center": "pc000000", "radius": 1, "construction": false}], "arcs": [{"id": "aa000000", "center": "pc000000", "start": "pd000000", "end": "pe000000", "construction": false}], "constraints": [{"id": "ka000000", "type": "horizontal", "line": "nb000000"}]}"#;

pub fn pad(id: &str, base: Option<&str>, sketch: &str, extent: &str) -> String {
    let base = base.map_or(String::new(), |b| format!(r#""base": "{b}", "#));
    format!(
        r#"{{"id": "{id}", "type": "pad", "v": 1, "name": "Pad", {base}"sketch": "{sketch}", "extent": {extent}}}"#
    )
}

pub fn pocket(id: &str, base: &str, sketch: &str, extent: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "pocket", "v": 1, "name": "Pocket", "base": "{base}", "sketch": "{sketch}", "extent": {extent}}}"#
    )
}

pub fn turned(
    id: &str,
    type_name: &str,
    base: Option<&str>,
    sketch: &str,
    axis: &str,
    extent: &str,
) -> String {
    let base = base.map_or(String::new(), |b| format!(r#""base": "{b}", "#));
    format!(
        r#"{{"id": "{id}", "type": "{type_name}", "v": 1, "name": "Turned", {base}"sketch": "{sketch}", "axis": "{axis}", "extent": {extent}}}"#
    )
}

pub fn one_side(length: &str) -> String {
    format!(r#"{{"type": "one_side", "length": {length}, "reversed": false}}"#)
}

pub fn angle(angle: &str) -> String {
    format!(r#"{{"type": "one_side", "angle": {angle}, "reversed": false}}"#)
}

pub const TEN: &str = r#"{"type": "one_side", "length": 10, "reversed": false}"#;
pub const TURN: &str = r#"{"type": "one_side", "angle": 360, "reversed": false}"#;

pub fn checked_document(features: &[&str]) -> Checked {
    Checked::new(read(document(features).as_bytes()).unwrap()).unwrap()
}
