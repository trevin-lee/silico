//! The M4.9 bracket's features, as its session added them (crates/silico-cli/tests/bracket/session.jsonl,
//! each ID's last), for the timeline edits' tests: two pockets on a pad, then a fillet.
#![allow(dead_code)]

pub const S0000001: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Bracket profile", "plane": "xz", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 60, "y": 0, "construction": false}, {"id": "pa000003", "x": 60, "y": 5, "construction": false}, {"id": "pa000004", "x": 5, "y": 5, "construction": false}, {"id": "pa000005", "x": 5, "y": 50, "construction": false}, {"id": "pa000006", "x": 0, "y": 50, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}, {"id": "na000005", "from": "pa000005", "to": "pa000006", "construction": false}, {"id": "na000006", "from": "pa000006", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}, {"id": "ka000005", "type": "horizontal", "line": "na000005"}, {"id": "ka000006", "type": "vertical", "line": "na000006"}, {"id": "ka000007", "type": "distance_x", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000008", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000009", "type": "length", "line": "na000001", "value": 60, "driving": true}, {"id": "ka00000a", "type": "length", "line": "na000002", "value": 5, "driving": true}, {"id": "ka00000b", "type": "length", "line": "na000005", "value": 5, "driving": true}, {"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}]}"#;

pub const P0000001: &str = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Legs", "sketch": "s0000001", "extent": {"type": "one_side", "length": 40, "reversed": true}}"#;

pub const S0000002: &str = r#"{"id": "s0000002", "type": "sketch", "v": 1, "name": "Base holes", "plane": "xy", "points": [{"id": "pb000001", "x": 40, "y": 12, "construction": false}, {"id": "pb000002", "x": 40, "y": 28, "construction": false}], "lines": [], "circles": [{"id": "cb000001", "center": "pb000001", "radius": 2.75, "construction": false}, {"id": "cb000002", "center": "pb000002", "radius": 2.75, "construction": false}], "arcs": [], "constraints": [{"id": "kb000001", "type": "diameter", "curve": "cb000001", "value": 5.5, "driving": true}, {"id": "kb000002", "type": "equal", "a": "cb000001", "b": "cb000002"}, {"id": "kb000003", "type": "distance_x", "a": "origin", "b": "pb000001", "value": 40, "driving": true}, {"id": "kb000004", "type": "distance_y", "a": "origin", "b": "pb000001", "value": 12, "driving": true}, {"id": "kb000005", "type": "vertical_points", "a": "pb000001", "b": "pb000002"}, {"id": "kb000006", "type": "distance_y", "a": "pb000001", "b": "pb000002", "value": 16, "driving": true}]}"#;

pub const H0000001: &str = r#"{"id": "h0000001", "type": "pocket", "v": 1, "name": "Base holes", "base": "p0000001", "sketch": "s0000002", "extent": {"type": "through_all", "reversed": true}}"#;

pub const S0000003: &str = r#"{"id": "s0000003", "type": "sketch", "v": 1, "name": "Upright holes", "plane": "yz", "points": [{"id": "pb000001", "x": 12, "y": 35, "construction": false}, {"id": "pb000002", "x": 28, "y": 35, "construction": false}], "lines": [], "circles": [{"id": "cb000001", "center": "pb000001", "radius": 2.75, "construction": false}, {"id": "cb000002", "center": "pb000002", "radius": 2.75, "construction": false}], "arcs": [], "constraints": [{"id": "kb000001", "type": "diameter", "curve": "cb000001", "value": 5.5, "driving": true}, {"id": "kb000002", "type": "equal", "a": "cb000001", "b": "cb000002"}, {"id": "kb000003", "type": "distance_x", "a": "origin", "b": "pb000001", "value": 12, "driving": true}, {"id": "kb000004", "type": "distance_y", "a": "origin", "b": "pb000001", "value": 35, "driving": true}, {"id": "kb000005", "type": "horizontal_points", "a": "pb000001", "b": "pb000002"}, {"id": "kb000006", "type": "distance_x", "a": "pb000001", "b": "pb000002", "value": 16, "driving": true}]}"#;

pub const H0000002: &str = r#"{"id": "h0000002", "type": "pocket", "v": 1, "name": "Upright holes", "base": "h0000001", "sketch": "s0000003", "extent": {"type": "through_all", "reversed": true}}"#;

pub const F0000001: &str = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "Inside corner", "base": "h0000002", "edges": ["p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000004)"], "radius": 5}"#;

/// The seven features as one document, in timeline order, as the session's file held them
/// before it was formatted: its header (naming 5), then each feature.
pub fn whole() -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        [
            S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001
        ]
        .join(", ")
    )
}
