//! For tests of sketches in the document: a document of one sketch from its lists, a base
//! sketch to constrain, and what the gate finds.
#![allow(dead_code)]

use silico_core::Error;
use silico_core::document::{read, validate};

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

/// The gate's problems with `text`, which reads.
pub fn problems(text: &str) -> Vec<Error> {
    match validate(&read(text.as_bytes()).unwrap()) {
        Ok(()) => Vec::new(),
        Err(invalid) => invalid.errors().to_vec(),
    }
}
