//! For tests of sketches in the document: a document of one sketch from its lists, a base
//! sketch to constrain, and what the gate finds.
#![allow(dead_code)]

use silico_core::Error;
use silico_core::document::{Operation, read, validate};

/// The sketch feature's ID.
pub const FEATURE: &str = "k7f2qx3m";

/// A document of one sketch with these fields (the JSON after `"name": "S"`).
pub fn document(fields: &str) -> String {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"{FEATURE}\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S\", {fields}}}]}}"
    )
}

/// A sketch's fields: plane xy, these lists (each the JSON inside its brackets).
pub fn fields(points: &str, lines: &str, circles: &str, arcs: &str, constraints: &str) -> String {
    format!(
        "\"plane\": \"xy\", \"points\": [{points}], \"lines\": [{lines}], \"circles\": [{circles}], \"arcs\": [{arcs}], \"constraints\": [{constraints}]"
    )
}

/// A point's JSON, its coordinates as `{x}` writes a float: the shortest decimal that reads
/// back exactly, without an exponent (`10`, `0.5`, `0.0000001`). The builders here are every
/// sketch test's, in every crate (included by `#[path]`), so a sketch's JSON is written one way.
pub fn point(id: &str, x: f64, y: f64) -> String {
    format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
}

/// A point's JSON, its coordinates as `{x:?}` writes a float: always with a point or an
/// exponent (`10.0`, `1e-7`, `5e-324`). The stored-coordinate tests write the values at the
/// edges of what a file holds this way.
pub fn point_debug(id: &str, x: f64, y: f64) -> String {
    format!(r#"{{"id": "{id}", "x": {x:?}, "y": {y:?}, "construction": false}}"#)
}

/// A line's JSON, from one point to another.
pub fn line(id: &str, from: &str, to: &str) -> String {
    format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": false}}"#)
}

/// A circle's JSON, its radius written as [`point`]'s coordinates are.
pub fn circle(id: &str, center: &str, radius: f64) -> String {
    format!(r#"{{"id": "{id}", "center": "{center}", "radius": {radius}, "construction": false}}"#)
}

/// An arc's JSON, counter-clockwise from `start` to `end` about `center`.
pub fn arc(id: &str, center: &str, start: &str, end: &str) -> String {
    format!(
        r#"{{"id": "{id}", "center": "{center}", "start": "{start}", "end": "{end}", "construction": false}}"#
    )
}

/// pa (0, 0), pb (10, 0), pc (0, 10); line na from pa to pb; circle ca about pa, radius 5;
/// arc aa about pa from pb to pc: the base sketch the gate's tests constrain.
pub const POINTS: &str = r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 10, "construction": false}"#;
pub const LINE: &str =
    r#"{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}"#;
pub const CIRCLE: &str =
    r#"{"id": "ca000000", "center": "pa000000", "radius": 5, "construction": false}"#;
pub const ARC: &str = r#"{"id": "aa000000", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}"#;

/// The base sketch with these constraints.
pub fn constrained(constraints: &str) -> String {
    document(&fields(POINTS, LINE, CIRCLE, ARC, constraints))
}

/// The gate's problems with `text`, which reads; and, failing the test if not, each
/// refusal of a constraint naming its ID.
#[track_caller]
pub fn problems(text: &str) -> Vec<Error> {
    let document = read(text.as_bytes()).unwrap();
    let errors = match validate(&document) {
        Ok(()) => Vec::new(),
        Err(invalid) => invalid.errors().to_vec(),
    };
    // A constraint's refusal names it by its ID too: its field is its place in the list,
    // which `fmt` reorders, so the ID is what stays its name (the M3 audit). Every refusal
    // of one, in every test.
    for error in &errors {
        let place = error
            .parameter()
            .and_then(|p| p.strip_prefix("constraints."))
            .and_then(|rest| rest.split('.').next())
            .and_then(|n| n.parse::<usize>().ok());
        let sketch = document.features.iter().find_map(|f| match &f.operation {
            Operation::Sketch(s) if Some(f.id.as_str()) == error.feature() => Some(s),
            _ => None,
        });
        if let (Some(i), Some(sketch)) = (place, sketch) {
            let id = sketch.constraints[i].id.as_str();
            assert!(
                error.elements().iter().any(|e| e == id),
                "{id} unnamed: {error:?}"
            );
        }
    }
    errors
}
