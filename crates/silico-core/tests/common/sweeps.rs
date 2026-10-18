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

/// A number as a document writes it.
fn number(v: f64) -> String {
    format!("{v}")
}

/// A sketch `id` on `plane` holding the rectangle from `at`, `w` by `h`: its corners
/// pa000001 to pa000004 counter-clockwise from `at`, and its sides na000001 (along the
/// bottom, from pa000001 to pa000002), na000002 (the right), na000003 (the top), and
/// na000004 (the left, back to pa000001). No constraints: its stored coordinates are its
/// solve.
pub fn rectangle(id: &str, plane: &str, at: [f64; 2], w: f64, h: f64) -> String {
    let [x, y] = at;
    polygon(id, plane, &[[x, y], [x + w, y], [x + w, y + h], [x, y + h]])
}

/// A sketch `id` on `plane` holding the closed polygon through `corners`, in order: its
/// corners pa000001, pa000002, ..., and its sides na000001 (from the first corner to the
/// second), na000002, ..., the last back to the first. At most 9 corners.
pub fn polygon(id: &str, plane: &str, corners: &[[f64; 2]]) -> String {
    let points: Vec<String> = corners
        .iter()
        .enumerate()
        .map(|(i, [x, y])| {
            format!(
                r#"{{"id": "pa00000{}", "x": {}, "y": {}, "construction": false}}"#,
                i + 1,
                number(*x),
                number(*y)
            )
        })
        .collect();
    let n = corners.len();
    let lines: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id": "na00000{}", "from": "pa00000{}", "to": "pa00000{}", "construction": false}}"#,
                i + 1,
                i + 1,
                (i + 1) % n + 1
            )
        })
        .collect();
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "Sketch", "plane": "{plane}", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
        points.join(", "),
        lines.join(", ")
    )
}

/// A sketch `id` on `plane` holding circles, each its centre and radius: centre pc00000n,
/// circle ca00000n, from 1.
pub fn circles(id: &str, plane: &str, circles: &[([f64; 2], f64)]) -> String {
    let points: Vec<String> = circles
        .iter()
        .enumerate()
        .map(|(i, ([x, y], _))| {
            format!(
                r#"{{"id": "pc00000{}", "x": {}, "y": {}, "construction": false}}"#,
                i + 1,
                number(*x),
                number(*y)
            )
        })
        .collect();
    let curves: Vec<String> = circles
        .iter()
        .enumerate()
        .map(|(i, (_, r))| {
            format!(
                r#"{{"id": "ca00000{}", "center": "pc00000{}", "radius": {}, "construction": false}}"#,
                i + 1,
                i + 1,
                number(*r)
            )
        })
        .collect();
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "Sketch", "plane": "{plane}", "points": [{}], "lines": [], "circles": [{}], "arcs": [], "constraints": []}}"#,
        points.join(", "),
        curves.join(", ")
    )
}

/// A box `id` from `position`, `l` by `w` by `h`.
pub fn box_at(id: &str, position: [f64; 3], l: f64, w: f64, h: f64) -> String {
    let [x, y, z] = position.map(number);
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Box", "position": [{x}, {y}, {z}], "length": {}, "width": {}, "height": {}}}"#,
        number(l),
        number(w),
        number(h)
    )
}

/// A sketch `id` on `plane` of rectangles, each `[x0, y0, x1, y1]`: the n-th's corners
/// p<n>000001 to p<n>000004 counter-clockwise from (x0, y0), its sides n<n>000001 (the
/// bottom) to n<n>000004 (the left), the letter n-th from `a` (at most 8).
pub fn rectangles(id: &str, plane: &str, rects: &[[f64; 4]]) -> String {
    let mut points = Vec::new();
    let mut lines = Vec::new();
    for (r, [x0, y0, x1, y1]) in rects.iter().enumerate() {
        let letter = ["a", "b", "c", "d", "e", "f", "g", "h"][r];
        let corners = [[*x0, *y0], [*x1, *y0], [*x1, *y1], [*x0, *y1]];
        for (i, [x, y]) in corners.iter().enumerate() {
            points.push(format!(
                r#"{{"id": "p{letter}00000{}", "x": {}, "y": {}, "construction": false}}"#,
                i + 1,
                number(*x),
                number(*y)
            ));
            lines.push(format!(
                r#"{{"id": "n{letter}00000{}", "from": "p{letter}00000{}", "to": "p{letter}00000{}", "construction": false}}"#,
                i + 1,
                i + 1,
                (i + 1) % 4 + 1
            ));
        }
    }
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "Sketch", "plane": "{plane}", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
        points.join(", "),
        lines.join(", ")
    )
}
