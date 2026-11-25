//! The bracket, M4.9's done test (docs/design/m4.9-mcp.md, *The bracket*): a scripted MCP
//! client starts the real `silico serve` in a temporary workspace and replays
//! `tests/bracket/session.jsonl`, a recorded agent session that designs the L-bracket of
//! `tests/bracket/spec.md`, rolls back one wrong edit, renders it, measures it, and
//! exports it, sending each line only after the reply to the one before.
//!
//! **Compared byte for byte:** every reply's `structuredContent` with the recording's, but
//! what's never compared: a PNG's bytes, `render`'s `png_bytes`, progress notifications,
//! and `initialize`'s `serverInfo.version` and the workspace's absolute path in its
//! `instructions` (the session uses only relative paths, so nothing else quotes it). Each
//! reply also holds to its tool's `outputSchema` in silico-mcp's committed `tools.json`.
//!
//! **Checked by hand,** never read from the recording: the expected values are the spec's
//! dimensions worked out on paper (prisms, cylinders, the fillet's spandrel) in
//! `common/hand.rs`, which the app's scripted slice shares, each derivation beside its
//! function there or its assert here. The 3MF is read here from its bytes and checked as M4.8
//! promises a printable mesh (closed, manifold, consistently oriented, no triangle with two
//! corners at one place, enclosing a positive volume), its genus that of a part with four
//! through holes, its chords counted by hand from OCCT's rule (126 points a rim and 33 an
//! arc of the fillet at 0.1 mm: 1,084 vertices and 2,180 triangles), and its volume the
//! part's and the chord segments' (about 0.512 mm³) within the coordinates' 32-bit
//! rounding; and
//! FreeCAD 1.1.3 (the oracle) reads it back, makes a solid of it, and finds that solid
//! valid, with the mesh's volume.
//!
//! **M5.1's timeline commands** on the same bracket and the real kernel: its timeline, its
//! pockets swapped (`move_feature`), the first removed and added back `before` the sketch
//! that followed it, each result the design's example and each volume by hand.
//!
//! **Recording.** With `SILICO_BRACKET_RECORD=1` set, the session is first driven live by
//! [`record`], which plays the agent (each request built from the replies before it, as an
//! agent reads them), and `session.jsonl` is written from the real replies; the replay and
//! every check below then run on what was written. Build the workers first, then record:
//!
//! ```text
//! pixi run cargo build -p silico-kernel -p silico-sketch -p silico-render --bins
//! SILICO_BRACKET_RECORD=1 pixi run cargo nextest run -p silico-cli -E 'test(bracket)'
//! ```
//!
//! It lives in silico-cli's tests, not silico-mcp's, so the `silico` binary it serves with
//! comes from Cargo (`CARGO_BIN_EXE_silico`), built for every run that runs it. Its workers
//! are the ones built beside it, as silico-cli's other tests take them (`pixi run test`
//! builds them).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/hand.rs"]
mod hand;
#[path = "common/replay.rs"]
mod replay;
#[path = "common/run.rs"]
mod run;
#[path = "common/serve.rs"]
mod serve;
#[path = "common/threemf.rs"]
mod threemf;
// The bracket's features as its session added them, which silico-core's timeline edits'
// tests share.
#[path = "../../silico-core/tests/common/bracket.rs"]
mod features;

use std::path::{Path, PathBuf};

use serde_json::Value;

use hand::{
    FILLET_R, HOLE, HOLE_R, PI, assert_bracket_mesh_as_by_hand, bracket, centred, chords, drilled,
    legs, measured, solids,
};
use replay::{Recorder, Reply, Serve, compared_initialize, holds, replay, tools};
use threemf::model;

/// The export's tolerance, the session's (FreeCAD's default).
const TOLERANCE: f64 = 0.1;

fn session_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/bracket/session.jsonl")
}

// --- The session's requests, as the agent wrote them -------------------------------------

/// The L-profile on the XZ plane (the sketch's x along +X, its y along +Z): the base's
/// section 60 × 5 and the upright's 5 × 45 above it, one closed loop of six lines, fixed by
/// six horizontal and vertical constraints, its corner at the origin, and four lengths.
const PROFILE: &str = r#"{"id": "s0000001", "type": "sketch", "name": "Bracket profile", "plane": "xz", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 60, "y": 0, "construction": false}, {"id": "pa000003", "x": 60, "y": 5, "construction": false}, {"id": "pa000004", "x": 5, "y": 5, "construction": false}, {"id": "pa000005", "x": 5, "y": 50, "construction": false}, {"id": "pa000006", "x": 0, "y": 50, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}, {"id": "na000005", "from": "pa000005", "to": "pa000006", "construction": false}, {"id": "na000006", "from": "pa000006", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}, {"id": "ka000005", "type": "horizontal", "line": "na000005"}, {"id": "ka000006", "type": "vertical", "line": "na000006"}, {"id": "ka000007", "type": "distance_x", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000008", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000009", "type": "length", "line": "na000001", "value": 60, "driving": true}, {"id": "ka00000a", "type": "length", "line": "na000002", "value": 5, "driving": true}, {"id": "ka00000b", "type": "length", "line": "na000005", "value": 5, "driving": true}, {"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}]}"#;

/// The pad of the profile, `length` along +Y: the XZ plane's normal is −Y, so `reversed`.
fn pad(length: u32) -> String {
    format!(
        r#"{{"id": "p0000001", "type": "pad", "name": "Legs", "sketch": "s0000001", "extent": {{"type": "one_side", "length": {length}, "reversed": true}}}}"#
    )
}

/// Two holes' circles, 5.5 across and 16 apart, the first's centre at `(x, y)` in the
/// sketch's coordinates and the second's `16` along `along` (`x` or `y`).
fn holes(id: &str, name: &str, plane: &str, (x, y): (u32, u32), along: &str) -> String {
    let (x2, y2, aligned) = match along {
        "x" => (x + 16, y, "horizontal_points"),
        _ => (x, y + 16, "vertical_points"),
    };
    let diameter = 2.0 * HOLE_R;
    format!(
        r#"{{"id": "{id}", "type": "sketch", "name": "{name}", "plane": "{plane}", "points": [{{"id": "pb000001", "x": {x}, "y": {y}, "construction": false}}, {{"id": "pb000002", "x": {x2}, "y": {y2}, "construction": false}}], "lines": [], "circles": [{{"id": "cb000001", "center": "pb000001", "radius": {HOLE_R}, "construction": false}}, {{"id": "cb000002", "center": "pb000002", "radius": {HOLE_R}, "construction": false}}], "arcs": [], "constraints": [{{"id": "kb000001", "type": "diameter", "curve": "cb000001", "value": {diameter}, "driving": true}}, {{"id": "kb000002", "type": "equal", "a": "cb000001", "b": "cb000002"}}, {{"id": "kb000003", "type": "distance_x", "a": "origin", "b": "pb000001", "value": {x}, "driving": true}}, {{"id": "kb000004", "type": "distance_y", "a": "origin", "b": "pb000001", "value": {y}, "driving": true}}, {{"id": "kb000005", "type": "{aligned}", "a": "pb000001", "b": "pb000002"}}, {{"id": "kb000006", "type": "distance_{along}", "a": "pb000001", "b": "pb000002", "value": 16, "driving": true}}]}}"#
    )
}

/// A pocket through everything of `base`, up from its sketch (the pocket's direction is
/// against the sketch's normal, so `reversed`: the XY plane's +Z, the YZ plane's +X).
fn pocket(id: &str, name: &str, base: &str, sketch: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "pocket", "name": "{name}", "base": "{base}", "sketch": "{sketch}", "extent": {{"type": "through_all", "reversed": true}}}}"#
    )
}

const INITIALIZE: &str = r#"{"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "bracket-agent", "version": "1"}}}"#;
const INITIALIZED: &str = r#"{"jsonrpc": "2.0", "method": "notifications/initialized"}"#;
const MASS: &str = r#"{"session": "s1", "model": "current", "feature": null}"#;
const CHECK: &str = r#"{"session": "s1", "model": "current", "fresh": false}"#;
const SESSION: &str = r#"{"session": "s1"}"#;

/// The agent's session, live, as `session.jsonl`'s lines.
fn record() -> Vec<String> {
    let mut r = Recorder {
        serve: Serve::start(),
        id: 0,
        lines: Vec::new(),
        revision: 0,
    };
    let reply = r.serve.send(INITIALIZE).unwrap();
    let expect = compared_initialize(&reply, &r.serve.work);
    r.push("initialize", INITIALIZE, Some(&expect));
    assert!(r.serve.send(INITIALIZED).is_none());
    r.push("initialized", INITIALIZED, None);
    r.call(
        "open: a new document for the bracket",
        "open",
        r#"{"path": "bracket.silico", "create": true}"#,
    );
    r.call("begin a transaction", "begin", SESSION);
    r.add("the L-profile, on the XZ plane", PROFILE);
    r.add("the wrong edit: the legs padded 4 deep, not 40", &pad(4));
    r.call("check the model: it builds, nothing to see", "check", CHECK);
    r.call(
        "measure it: 2,100 mm³, not the spec's 21,000 for the legs",
        "mass_properties",
        MASS,
    );
    r.call("roll the wrong edit back", "rollback", SESSION);
    r.call("begin again", "begin", SESSION);
    r.add("the L-profile again", PROFILE);
    r.add("the legs, padded 40 wide", &pad(40));
    r.call("measure the legs: 21,000 mm³", "mass_properties", MASS);
    r.add(
        "the base's holes, on the XY plane",
        &holes("s0000002", "Base holes", "xy", (40, 12), "y"),
    );
    r.add(
        "pocket them up through the base",
        &pocket("h0000001", "Base holes", "p0000001", "s0000002"),
    );
    r.add(
        "the upright's holes, on the YZ plane",
        &holes("s0000003", "Upright holes", "yz", (12, 35), "x"),
    );
    r.add(
        "pocket them through the upright",
        &pocket("h0000002", "Upright holes", "h0000001", "s0000003"),
    );
    r.call("measure the holes' cut", "mass_properties", MASS);
    let lines = r.call(
        "find the straight edges 40 long: the inside corner is the one at x = 5, z = 5",
        "query",
        r#"{"session": "s1", "model": "current", "feature": null, "kind": "edge", "type": "line", "stable": true, "facing": null, "measures": [{"measure": "length", "at_least": 40, "at_most": 40}], "order": null, "limit": 10}"#,
    );
    let corner = lines["result"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["start"] == serde_json::json!([5, 0, 5]))
        .unwrap()["name"]
        .clone();
    r.add(
        "fillet the inside corner, radius 5",
        &format!(
            r#"{{"id": "f0000001", "type": "fillet", "name": "Inside corner", "base": "h0000002", "edges": [{corner}], "radius": {FILLET_R}}}"#
        ),
    );
    r.call(
        "commit: the file is saved",
        "commit",
        r#"{"session": "s1", "path": null, "replace": false}"#,
    );
    r.call("check the saved model", "check", CHECK);
    r.call("measure the bracket", "mass_properties", MASS);
    let face = r.call(
        "find the base's end face: the smallest planar face facing +X",
        "query",
        r#"{"session": "s1", "model": "current", "feature": null, "kind": "face", "type": "plane", "stable": null, "facing": {"direction": [1, 0, 0], "within": 0}, "measures": [], "order": {"by": "area", "descending": false}, "limit": 1}"#,
    );
    let end = face["result"]["elements"][0]["name"].clone();
    let rims = r.call(
        "find the holes' rims: circles of radius 2.75",
        "query",
        &format!(
            r#"{{"session": "s1", "model": "current", "feature": null, "kind": "edge", "type": "circle", "stable": null, "facing": null, "measures": [{{"measure": "radius", "at_least": {HOLE_R}, "at_most": {HOLE_R}}}], "order": null, "limit": 8}}"#
        ),
    );
    let rim = rims["result"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["axis"]["point"] == serde_json::json!([40, 12, 5]))
        .unwrap()["name"]
        .clone();
    r.call(
        "measure the hole at (40, 12) from the base's end",
        "distance",
        &format!(
            r#"{{"session": "s1", "model": "current", "elements": [{rim}, {end}], "feature": null}}"#
        ),
    );
    r.call(
        "render it from the isometric corner, the measured rim and face highlighted",
        "render",
        &format!(
            r#"{{"session": "s1", "model": "current", "view": "top_front_right", "camera": null, "width": 800, "height": 600, "highlight": [{rim}, {end}], "to": "bracket.png", "replace": false}}"#
        ),
    );
    r.call(
        "export it for printing",
        "export",
        &format!(
            r#"{{"session": "s1", "format": "3mf", "path": "bracket.3mf", "replace": false, "features": null, "tolerance": {TOLERANCE}}}"#
        ),
    );
    r.call("close the session", "close", SESSION);
    r.serve.end();
    r.lines
}

// --- The answers, and FreeCAD's read of the export (the hand's: `hand`) -------------------

fn mass(answer: &Value) -> &Value {
    let bodies = answer["result"]["bodies"].as_array().unwrap();
    assert_eq!(bodies.len(), 1, "{answer}");
    &bodies[0]
}

/// What FreeCAD 1.1.3 reads from the 3MF at `path`: the mesh's own checks, and the shape
/// made from it (`Part.Shape.makeShapeFromMesh`, then `Part.makeSolid`, as FreeCAD's
/// *Part from mesh* does), whether it's a valid solid, and its volume.
fn freecad_reads(path: &Path) -> Value {
    silico_oracle::freecad::script(&format!(
        r#"
import json, Mesh, Part
m = Mesh.Mesh({path:?})
shape = Part.Shape()
shape.makeShapeFromMesh(m.Topology, 0.01)
solid = Part.makeSolid(shape)
box = solid.BoundBox
print("SILICO_ORACLE " + json.dumps({{
    "mesh_solid": m.isSolid(),
    "non_manifold": m.hasNonManifolds(),
    "misoriented": m.countNonUniformOrientedFacets(),
    "points": m.CountPoints,
    "facets": m.CountFacets,
    "type": solid.ShapeType,
    "valid": solid.isValid(),
    "closed": solid.isClosed(),
    "shells": len(solid.Shells),
    "faces": len(solid.Faces),
    "volume": solid.Volume,
    "box": [box.XMin, box.YMin, box.ZMin, box.XMax, box.YMax, box.ZMax],
}}))
"#,
        path = path.to_str().unwrap()
    ))
    .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn the_bracket_session_replays_measures_as_by_hand_and_its_3mf_reads_back_in_the_oracle_as_a_solid()
{
    if std::env::var_os("SILICO_BRACKET_RECORD").is_some() {
        let mut text = record().join("\n");
        text.push('\n');
        std::fs::write(session_file(), text).unwrap();
    }
    let run = replay(&session_file());

    // The wrong edit: the L-profile's section is 60 · 5 + 5 · 45 = 525 mm², its perimeter
    // 60 + 5 + 55 + 45 + 5 + 50 = 220 mm; padded 4: 2,100 mm³, 2 · 525 + 220 · 4 = 1,930
    // mm², its centre 2 along y (the pad goes +Y from the XZ plane, reversed from its −Y
    // normal). It builds: `check` has nothing to say; the volume is what's wrong.
    let masses = run.answers("mass_properties");
    assert_eq!(masses.len(), 4);
    let wrong = mass(masses[0]);
    let thin = legs(4.0);
    measured("the wrong pad's volume", &wrong["volume"], thin.volume);
    assert_eq!(thin.volume, 2100.0);
    measured(
        "the wrong pad's area",
        &wrong["area"],
        2.0 * 525.0 + 220.0 * 4.0,
    );
    centred(
        "the wrong pad's centre",
        &wrong["center_of_mass"],
        thin.centre(),
    );
    let checks = run.answers("check");
    assert_eq!(checks.len(), 2);
    for check in &checks {
        assert_eq!(check["result"]["problems_total"], 0, "{check}");
        assert_eq!(check["result"]["warnings_total"], 0, "{check}");
    }
    // Rolled back to the transaction's base, the empty document: the file never held it.
    let begins = run.answers("begin");
    let rollback = run.answers("rollback")[0];
    assert_eq!(
        rollback["result"]["restored"], begins[0]["result"]["base"],
        "{rollback}"
    );
    assert_eq!(rollback["session"]["transaction"], false);

    // The legs, 40 wide: 12,000 + 9,000 = 21,000 mm³, 2 · 525 + 220 · 40 = 9,850 mm², the
    // centre (9,562.5 / 525, 20, 6,937.5 / 525).
    let by_hand = solids();
    let right = mass(masses[1]);
    let full = &by_hand[0];
    assert_eq!((full.sum.volume, full.area), (21000.0, 9850.0));
    measured("the legs' volume", &right["volume"], full.sum.volume);
    measured("the legs' area", &right["area"], full.area);
    centred(
        "the legs' centre",
        &right["center_of_mass"],
        full.sum.centre(),
    );

    // The four holes: 4 · 37.8125π = 151.25π mm³ less; each takes two discs of π · 2.75²
    // from its leg's faces and adds a wall of 2π · 2.75 · 5, so 4 · (27.5π − 15.125π) =
    // 49.5π mm² more.
    let holed = mass(masses[2]);
    let drilled = &by_hand[2];
    assert_eq!(drilled.area, 9850.0 + 49.5 * PI);
    measured(
        "the drilled legs' volume",
        &holed["volume"],
        21000.0 - 151.25 * PI,
    );
    measured(
        "the drilled legs' volume (sum)",
        &holed["volume"],
        drilled.sum.volume,
    );
    measured("the drilled legs' area", &holed["area"], drilled.area);
    centred(
        "the drilled legs' centre",
        &holed["center_of_mass"],
        drilled.sum.centre(),
    );

    // The corner the agent filleted: of the six edges 40 long (one along y at each of the
    // profile's six corners), the one at x = 5, z = 5.
    let queries = run.answers("query");
    assert_eq!(queries.len(), 3);
    assert_eq!(queries[0]["result"]["matched"], 6, "{}", queries[0]);
    let fillets: Vec<&Value> = run
        .arguments("add_feature")
        .into_iter()
        .map(|a| &a["feature"])
        .filter(|f| f["type"] == "fillet")
        .collect();
    assert_eq!(fillets.len(), 1);
    let fillet = fillets[0];
    assert_eq!(fillet["radius"], FILLET_R);
    let edge = queries[0]["result"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == fillet["edges"][0])
        .unwrap();
    let ends = [&edge["start"], &edge["end"]]
        .map(|p| serde_json::from_value::<[f64; 3]>(p.clone()).unwrap());
    let mut ends = ends.to_vec();
    ends.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(ends, [[5.0, 0.0, 5.0], [5.0, 40.0, 5.0]], "{edge}");

    // Committed: the four solids' fingerprints written.
    let commit = run.answers("commit")[0];
    assert_eq!(commit["result"]["written"], true);
    assert_eq!(
        commit["result"]["refingerprinted"],
        serde_json::json!(["p0000001", "h0000001", "h0000002", "f0000001"])
    );

    // The bracket. Volume: 21,000 − 151.25π + the fillet's (1 − π/4) · 5² · 40 =
    // 22,000 − 401.25π ≈ 20,739.4359 mm³. Area: the fillet takes 5 · 40 from each leg's
    // inner face and adds a quarter cylinder, (π/2) · 5 · 40 = 100π, and each end face
    // gains the spandrel, 25(1 − π/4): 9,850 + 49.5π − 400 + 100π + 50 − 12.5π =
    // 9,500 + 137π ≈ 9,930.3982 mm². Centre: the parts' moments over the volume
    // ([`bracket`]): about (18.0196, 20, 13.0140).
    let done = mass(masses[3]);
    let hand = &by_hand[3];
    let volume = 22000.0 - 401.25 * PI;
    assert!(
        (hand.sum.volume - volume).abs() < 1e-9,
        "{}",
        hand.sum.volume
    );
    assert_eq!(hand.area, 9500.0 + 137.0 * PI);
    measured("the bracket's volume", &done["volume"], volume);
    measured("the bracket's area", &done["area"], hand.area);
    centred(
        "the bracket's centre",
        &done["center_of_mass"],
        hand.sum.centre(),
    );

    // The saved document's last fingerprint: the box [0, 0, 0]–[60, 40, 50]; 13 faces (the
    // L-prism's 8, a wall for each hole, the fillet's), 33 edges (the prism's 18, each hole
    // two rims and a seam, the fillet's two lines and two arcs for the corner's one), and
    // 22 vertices (the prism's 12, each hole's two on its seam, the fillet's four for the
    // corner's two).
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(run.work.join("bracket.silico")).unwrap())
            .unwrap();
    let features = saved["features"].as_array().unwrap();
    let last = &features.last().unwrap()["fingerprint"];
    assert_eq!(last["bbox"], serde_json::json!([0, 0, 0, 60, 40, 50]));
    assert_eq!(hand.counts, [13, 33, 22]);
    assert_eq!(last["faces"], hand.counts[0]);
    assert_eq!(last["edges"], hand.counts[1]);
    assert_eq!(last["vertices"], hand.counts[2]);
    measured("the saved volume", &last["volume"], volume);

    // The base's end face, x = 60: 40 × 5 = 200 mm², facing +X, centred (60, 20, 2.5); the
    // other face facing +X is the upright's inner face, larger.
    assert_eq!(queries[1]["result"]["matched"], 2);
    let end = &queries[1]["result"]["elements"][0];
    measured("the end face's area", &end["area"], 200.0);
    assert_eq!(end["normal"], serde_json::json!([1, 0, 0]));
    assert_eq!(end["centroid"], serde_json::json!([60, 20, 2.5]));
    // Eight rims, two for each hole, each of radius 2.75; the one measured at (40, 12).
    assert_eq!(queries[2]["result"]["matched"], 8);
    let distance = run.answers("distance")[0];
    let measured_pair = &run.arguments("distance")[0]["elements"];
    assert_eq!(measured_pair[1], end["name"]);
    let rim = queries[2]["result"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == measured_pair[0])
        .unwrap();
    measured("the rim's radius", &rim["radius"], HOLE_R);
    assert_eq!(rim["axis"]["point"], serde_json::json!([40, 12, 5]));
    // The hole's centre is 60 − 40 = 20 from the end face; its rim's nearest point,
    // (42.75, 12, 5), 20 − 2.75 = 17.25.
    measured("the rim's distance", &distance["result"]["distance"], 17.25);
    measured(
        "the hole's centre's distance",
        &distance["result"]["center_to_element"],
        20.0,
    );
    assert_eq!(
        distance["result"]["from"],
        serde_json::json!([42.75, 12, 5])
    );
    assert_eq!(distance["result"]["to"], serde_json::json!([60, 12, 5]));

    // The render: 800 × 600, as asked, written to bracket.png and sent beside the result
    // (its bytes never compared); from the top front right, looking at the box's centre
    // (30, 20, 25) along (−1, 1, −1), up (−1, 1, 2)/√6.
    let render = &run.answers("render")[0]["result"];
    assert_eq!(render["width"], 800);
    assert_eq!(render["height"], 600);
    assert_eq!(render["path"], "bracket.png");
    let png = std::fs::read(run.work.join("bracket.png")).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&png[12..16], b"IHDR");
    let be = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    assert_eq!((be(16), be(20)), (800, 600));
    assert_eq!(run.images.len(), 1);
    let camera = &render["camera"];
    assert_eq!(camera["target"], serde_json::json!([30, 20, 25]));
    let eye: [f64; 3] = serde_json::from_value(camera["eye"].clone()).unwrap();
    let away = [eye[0] - 30.0, 20.0 - eye[1], eye[2] - 25.0];
    assert!(
        away[0] > 0.0 && away.iter().all(|a| (a - away[0]).abs() < 1e-6),
        "{eye:?}"
    );
    let up: [f64; 3] = serde_json::from_value(camera["up"].clone()).unwrap();
    let sixth = (1.0f64 / 6.0).sqrt();
    for (u, h) in up.into_iter().zip([-sixth, sixth, 2.0 * sixth]) {
        assert!((u - h).abs() < 1e-12, "{up:?}");
    }

    // The export: one object, the bracket's last feature, its counts and its volume those
    // of the mesh in the file, which is printable as M4.8 promises.
    let export = &run.answers("export")[0]["result"];
    let archive = std::fs::read(run.work.join("bracket.3mf")).unwrap();
    assert_eq!(export["bytes"], archive.len());
    let meshes = model(&archive);
    assert_eq!(meshes.len(), 1, "one object, the bracket's");
    let mesh = &meshes[0];
    assert_eq!(mesh.name, "Inside corner");
    let objects = export["objects"].as_array().unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0]["feature"], "f0000001");
    assert_eq!(objects[0]["triangles"], mesh.triangles.len());
    assert_eq!(objects[0]["vertices"], mesh.vertices.len());
    // Meshed at the tolerance asked, against the hand ([`assert_bracket_mesh_as_by_hand`],
    // its derivation there): at 0.1 mm, 126 chords a rim and 32 an arc of the fillet, so
    // 1,084 vertices and 2,180 triangles, and the volume the part's and its chords'
    // segments' within the 32-bit rounding.
    let rim = chords(2.0 * PI, HOLE_R, TOLERANCE);
    let arc = chords(PI / 2.0, FILLET_R, TOLERANCE);
    assert_eq!((rim, arc), (126, 32));
    let mesh_volume = assert_bracket_mesh_as_by_hand(mesh, TOLERANCE);
    assert_eq!(mesh.vertices.len(), 1084);
    assert_eq!(mesh.triangles.len(), 2180);
    measured("the export's volume", &objects[0]["volume"], mesh_volume);

    // FreeCAD reads it back as a solid: the mesh closed, manifold, and oriented by its own
    // checks, with the file's points and facets; the shape made of it a valid, closed solid
    // of one shell and a face a triangle, its box the part's, and its volume the mesh's.
    let read = freecad_reads(&run.work.join("bracket.3mf"));
    assert_eq!(read["mesh_solid"], true, "{read}");
    assert_eq!(read["non_manifold"], false, "{read}");
    assert_eq!(read["misoriented"], 0, "{read}");
    assert_eq!(read["points"], mesh.vertices.len(), "{read}");
    assert_eq!(read["facets"], mesh.triangles.len(), "{read}");
    assert_eq!(read["type"], "Solid", "{read}");
    assert_eq!(read["valid"], true, "{read}");
    assert_eq!(read["closed"], true, "{read}");
    assert_eq!(read["shells"], 1, "{read}");
    assert_eq!(read["faces"], mesh.triangles.len(), "{read}");
    let box_: [f64; 6] = serde_json::from_value(read["box"].clone()).unwrap();
    for (got, want) in box_.into_iter().zip([0.0, 0.0, 0.0, 60.0, 40.0, 50.0]) {
        assert!((got - want).abs() < 1e-9, "{read}");
    }
    let theirs = read["volume"].as_f64().unwrap();
    assert!(
        (theirs - mesh_volume).abs() <= 1e-9 * mesh_volume,
        "FreeCAD's solid {theirs} mm³, the mesh {mesh_volume}"
    );
}

// --- M5.1's timeline commands on the real kernel ---------------------------------------------

/// silico-core's committed example `name` (`move_feature/result`), read as JSON.
fn core_example(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../silico-core/tests/contract/commands/examples")
        .join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// A tool's answer's envelope, through `serve`.
fn called(serve: &mut Serve, id: u64, tool: &str, arguments: &str) -> Value {
    let send = format!(
        r#"{{"jsonrpc": "2.0", "id": {id}, "method": "tools/call", "params": {{"name": "{tool}", "arguments": {arguments}}}}}"#
    );
    let reply = serve.send(&send).unwrap();
    let answer: Reply = serde_json::from_str(&reply).unwrap();
    let envelope: Value = serde_json::from_str(answer.result.structured.get()).unwrap();
    assert_eq!(
        answer.result.is_error,
        envelope["ok"] == false,
        "{tool}: {envelope}"
    );
    holds(&tools()[tool]["outputSchema"], &envelope, tool);
    envelope
}

#[test]
fn the_bracket_s_timeline_edits_through_serve_measure_as_by_hand() {
    // The design's M5.1 examples on the real kernel (docs/design/m5.0-app.md, section 7):
    // the bracket, formatted so each feature stores its fingerprint, then, inside a
    // transaction, its timeline, the pockets swapped by `move_feature`, the first removed,
    // and it added back before s0000003; each result the design's, each volume by hand.
    use features::*;
    let mut serve = Serve::start();
    std::fs::write(serve.work.join("bracket.silico"), whole()).unwrap();
    serve.send(INITIALIZE).unwrap();
    assert!(serve.send(INITIALIZED).is_none());
    let mut id = 0;
    let mut call = |serve: &mut Serve, tool: &str, arguments: &str| {
        id += 1;
        called(serve, id, tool, arguments)
    };
    let opened = call(
        &mut serve,
        "open",
        r#"{"path": "bracket.silico", "create": false}"#,
    );
    assert_eq!(opened["ok"], true, "{opened}");
    let formatted = call(
        &mut serve,
        "format",
        r#"{"session": "s1", "features": null, "write": true}"#,
    );
    assert_eq!(formatted["result"]["written"], true, "{formatted}");
    let volume = |serve: &mut Serve, call: &mut dyn FnMut(&mut Serve, &str, &str) -> Value| {
        let answer = call(
            serve,
            "mass_properties",
            r#"{"session": "s1", "model": "current", "feature": null}"#,
        );
        mass(&answer)["volume"].clone()
    };
    // All built: the legs, less the four holes, and the fillet's spandrel, 20,739.435948.
    let whole = bracket().volume;
    measured("the bracket", &volume(&mut serve, &mut call), whole);
    let begun = call(&mut serve, "begin", r#"{"session": "s1"}"#);
    let at = begun["session"]["revision"].as_u64().unwrap();

    // The timeline: every feature healthy, as the design's example lists them, the fillet
    // the one body.
    let timeline = call(
        &mut serve,
        "timeline",
        r#"{"session": "s1", "model": "current"}"#,
    );
    let example = core_example("timeline/result");
    assert_eq!(
        timeline["result"]["features"], example["features"],
        "{timeline}"
    );
    assert_eq!(timeline["result"]["bodies"], example["bodies"]);
    assert_eq!(timeline["result"]["revision"], at);
    assert_eq!(timeline["result"]["model_revision"], at);
    assert_eq!(timeline["result"]["problems"], 0);

    // The pockets swapped: the design's result, word for word; the cuts commute, so the
    // volume is the same.
    let moved = call(
        &mut serve,
        "move_feature",
        &format!(
            r#"{{"session": "s1", "revision": {at}, "feature": "h0000001", "before": "f0000001"}}"#
        ),
    );
    assert_eq!(
        moved["result"],
        core_example("move_feature/result"),
        "{moved}"
    );
    measured("the pockets swapped", &volume(&mut serve, &mut call), whole);
    // Rebuilt on their new bases, each feature still healthy: the edits' reach is excepted
    // from drift until the commit writes it.
    let timeline = call(
        &mut serve,
        "timeline",
        r#"{"session": "s1", "model": "current"}"#,
    );
    for entry in timeline["result"]["features"].as_array().unwrap() {
        assert_eq!(entry["health"], "healthy", "{entry}");
    }
    let rolled = call(&mut serve, "rollback", r#"{"session": "s1"}"#);
    let at = rolled["session"]["revision"].as_u64().unwrap();
    call(&mut serve, "begin", r#"{"session": "s1"}"#);

    // The first pocket removed: h0000002 on the pad, so two holes fewer, 20,977.019.
    let removed = call(
        &mut serve,
        "remove_feature",
        &format!(r#"{{"session": "s1", "revision": {at}, "feature": "h0000001"}}"#),
    );
    assert_eq!(
        removed["result"]["rewired"],
        core_example("remove_feature/result-rewired")["rewired"]
    );
    assert_eq!(removed["result"]["index"], 3);
    let one_pocket = whole + 2.0 * HOLE;
    measured(
        "the first pocket removed",
        &volume(&mut serve, &mut call),
        one_pocket,
    );
    // Added back before s0000003, its fingerprint left out: h0000002's base rewired back,
    // and the volume as before.
    let mut back = removed["result"]["feature"].clone();
    back.as_object_mut().unwrap().remove("fingerprint");
    let added = call(
        &mut serve,
        "add_feature",
        &format!(
            r#"{{"session": "s1", "revision": {}, "before": "s0000003", "feature": {back}}}"#,
            at + 1
        ),
    );
    assert_eq!(added["result"]["index"], 3, "{added}");
    assert_eq!(
        added["result"]["rewired"],
        serde_json::json!([{"feature": "h0000002", "field": "base", "from": "p0000001", "to": "h0000001"}])
    );
    measured("the pocket back", &volume(&mut serve, &mut call), whole);
    call(&mut serve, "rollback", r#"{"session": "s1"}"#);
    call(&mut serve, "close", r#"{"session": "s1"}"#);
    serve.end();
}

#[test]
fn the_bracket_s_suppressions_through_serve_measure_as_by_hand() {
    // The design's suppression cases on the real kernel (docs/design/m5.0-app.md, section
    // 7's `suppress_feature` and the test plan's *Suppress*), each inside a transaction from
    // the formatted bracket, rolled back after it; each result the design's, and each
    // volume by hand: the legs (21,000), a hole (π · 2.75² · 5), the fillet's spandrel.
    use features::*;
    let mut serve = Serve::start();
    std::fs::write(serve.work.join("bracket.silico"), whole()).unwrap();
    serve.send(INITIALIZE).unwrap();
    assert!(serve.send(INITIALIZED).is_none());
    let mut id = 0;
    let mut call = |serve: &mut Serve, tool: &str, arguments: &str| {
        id += 1;
        called(serve, id, tool, arguments)
    };
    let opened = call(
        &mut serve,
        "open",
        r#"{"path": "bracket.silico", "create": false}"#,
    );
    assert_eq!(opened["ok"], true, "{opened}");
    call(
        &mut serve,
        "format",
        r#"{"session": "s1", "features": null, "write": true}"#,
    );
    // Fresh: every step built again, never read from the cache (the test plan's).
    let cleared = call(&mut serve, "cache_clear", r#"{"session": "s1"}"#);
    assert_eq!(cleared["ok"], true, "{cleared}");
    let volume = |serve: &mut Serve, call: &mut dyn FnMut(&mut Serve, &str, &str) -> Value| {
        let checked = call(
            serve,
            "check",
            r#"{"session": "s1", "model": "current", "fresh": true}"#,
        );
        assert_eq!(checked["ok"], true, "{checked}");
        let answer = call(
            serve,
            "mass_properties",
            r#"{"session": "s1", "model": "current", "feature": null}"#,
        );
        answer["result"]["bodies"].clone()
    };
    let whole = bracket().volume;
    let one_pocket = whole + 2.0 * HOLE;
    let no_fillet = drilled().volume;
    let no_pockets = legs(40.0).volume + bracket().volume - drilled().volume;
    // By hand: 21,000 + 25 (1 − π/4) · 40.
    assert!((no_pockets - (21000.0 + 25.0 * (1.0 - PI / 4.0) * 40.0)).abs() < 1e-9);

    // Each case: the features suppressed, by one command each, and what's left.
    let cases: [(&[&str], Option<f64>); 6] = [
        // The first hole: the part, h0000002 on the pad (decision 3), 20,977.019.
        (&["h0000001"], Some(one_pocket)),
        (&["h0000002"], Some(one_pocket)),
        // The fillet: the drilled legs, 20,524.834111.
        (&["f0000001"], Some(no_fillet)),
        // Both pockets: 21,214.602.
        (&["h0000001", "h0000002"], Some(no_pockets)),
        // The base holes' sketch: h0000001 with it, 20,977.019.
        (&["s0000002"], Some(one_pocket)),
        // The pad: all four solids; no body.
        (&["p0000001"], None),
    ];
    for (suppressed, hand) in cases {
        let begun = call(&mut serve, "begin", r#"{"session": "s1"}"#);
        let mut at = begun["session"]["revision"].as_u64().unwrap();
        for feature in suppressed {
            let done = call(
                &mut serve,
                "suppress_feature",
                &format!(
                    r#"{{"session": "s1", "revision": {at}, "feature": "{feature}", "suppressed": true}}"#
                ),
            );
            assert_eq!(done["result"]["changed"], true, "{done}");
            at = done["session"]["revision"].as_u64().unwrap();
        }
        let bodies = volume(&mut serve, &mut call);
        match hand {
            Some(hand) => {
                let bodies = bodies.as_array().unwrap();
                assert_eq!(bodies.len(), 1, "{suppressed:?}: {bodies:?}");
                measured(
                    &format!("{suppressed:?} suppressed"),
                    &bodies[0]["volume"],
                    hand,
                );
            }
            None => assert_eq!(bodies, serde_json::json!([]), "{suppressed:?}"),
        }
        call(&mut serve, "rollback", r#"{"session": "s1"}"#);
    }

    // The design's examples on the real kernel: the sketch, then h0000001 unsuppressed with
    // it, the whole bracket again.
    let begun = call(&mut serve, "begin", r#"{"session": "s1"}"#);
    let at = begun["session"]["revision"].as_u64().unwrap();
    let done = call(
        &mut serve,
        "suppress_feature",
        &format!(
            r#"{{"session": "s1", "revision": {at}, "feature": "s0000002", "suppressed": true}}"#
        ),
    );
    assert_eq!(
        done["result"],
        core_example("suppress_feature/result-sketch")
    );
    let back = call(
        &mut serve,
        "suppress_feature",
        &format!(
            r#"{{"session": "s1", "revision": {}, "feature": "h0000001", "suppressed": false}}"#,
            at + 1
        ),
    );
    assert_eq!(
        back["result"],
        core_example("suppress_feature/result-unsuppress")
    );
    let bodies = volume(&mut serve, &mut call);
    measured("unsuppressed again", &bodies[0]["volume"], whole);
    call(&mut serve, "rollback", r#"{"session": "s1"}"#);
    call(&mut serve, "close", r#"{"session": "s1"}"#);
    serve.end();
}

#[test]
fn a_suppressed_fillet_changes_the_shape_and_unsuppressed_gives_it_back() {
    // FreeCAD's TestSuppressed.py::testSuppressChangesShape (main), ported with its values
    // by hand where FreeCAD asserts only that they differ and come back: a 10 mm cube with a
    // 1 mm fillet of one edge, 1000 − (1 − π/4) · 1² · 10 = 997.853982; suppressed, the cube,
    // 1000; unsuppressed, 997.853982 again (FreeCAD 1.1.3, observed: 997.8539816339743,
    // 999.9999999999998, 997.8539816339743).
    let mut serve = Serve::start();
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k0000001", "type": "box", "v": 1, "name": "Cube", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}, {"id": "f0000001", "type": "fillet", "v": 1, "name": "Fillet", "base": "k0000001", "edges": ["k0000001.box.Edge1"], "radius": 1}]}"#;
    std::fs::write(serve.work.join("cube.silico"), text).unwrap();
    serve.send(INITIALIZE).unwrap();
    assert!(serve.send(INITIALIZED).is_none());
    let mut id = 0;
    let mut call = |serve: &mut Serve, tool: &str, arguments: &str| {
        id += 1;
        called(serve, id, tool, arguments)
    };
    call(
        &mut serve,
        "open",
        r#"{"path": "cube.silico", "create": false}"#,
    );
    let volume = |serve: &mut Serve, call: &mut dyn FnMut(&mut Serve, &str, &str) -> Value| {
        let answer = call(
            serve,
            "mass_properties",
            r#"{"session": "s1", "model": "current", "feature": null}"#,
        );
        mass(&answer)["volume"].clone()
    };
    let filleted = 1000.0 - (1.0 - PI / 4.0) * 10.0;
    measured("filleted", &volume(&mut serve, &mut call), filleted);
    // Outside a transaction, each its own commit.
    let off = call(
        &mut serve,
        "suppress_feature",
        r#"{"session": "s1", "revision": 1, "feature": "f0000001", "suppressed": true}"#,
    );
    assert_eq!(off["result"]["suppressed"], serde_json::json!(["f0000001"]));
    measured("suppressed", &volume(&mut serve, &mut call), 1000.0);
    let rev = off["session"]["revision"].as_u64().unwrap();
    let on = call(
        &mut serve,
        "suppress_feature",
        &format!(
            r#"{{"session": "s1", "revision": {rev}, "feature": "f0000001", "suppressed": false}}"#
        ),
    );
    assert_eq!(
        on["result"]["unsuppressed"],
        serde_json::json!(["f0000001"])
    );
    measured("unsuppressed", &volume(&mut serve, &mut call), filleted);
    call(&mut serve, "close", r#"{"session": "s1"}"#);
    serve.end();
}

/// A square's sketch `id` on `xy`, its lower corner at the origin, of side `side`, its
/// points and lines `<prefix>000001` on.
fn square(id: &str, prefix: char, side: u32) -> String {
    let p = |n: u32| format!("p{prefix}00000{n}");
    let l = |n: u32| format!("n{prefix}00000{n}");
    let corners = [(0, 0), (side, 0), (side, side), (0, side)];
    let points: Vec<String> = corners
        .iter()
        .enumerate()
        .map(|(i, (x, y))| {
            format!(
                r#"{{"id": "{}", "x": {x}, "y": {y}, "construction": false}}"#,
                p(i as u32 + 1)
            )
        })
        .collect();
    let lines: Vec<String> = (1..=4)
        .map(|i| {
            format!(
                r#"{{"id": "{}", "from": "{}", "to": "{}", "construction": false}}"#,
                l(i),
                p(i),
                p(i % 4 + 1)
            )
        })
        .collect();
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "Square", "plane": "xy", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
        points.join(", "),
        lines.join(", ")
    )
}

/// `serve` started on `name` holding `features`, initialized and opened as s1, and a call
/// that numbers its requests.
fn served(
    name: &str,
    features: &[&str],
) -> (Serve, impl FnMut(&mut Serve, &str, &str) -> Value + use<>) {
    let mut serve = Serve::start();
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    );
    std::fs::write(serve.work.join(name), text).unwrap();
    serve.send(INITIALIZE).unwrap();
    assert!(serve.send(INITIALIZED).is_none());
    let mut id = 0;
    let mut call = move |serve: &mut Serve, tool: &str, arguments: &str| {
        id += 1;
        called(serve, id, tool, arguments)
    };
    let opened = call(
        &mut serve,
        "open",
        &format!(r#"{{"path": "{name}", "create": false}}"#),
    );
    assert_eq!(opened["ok"], true, "{opened}");
    (serve, call)
}

/// The volume `mass_properties` gives `feature`'s solid, of the current model.
fn volume_of(
    serve: &mut Serve,
    call: &mut impl FnMut(&mut Serve, &str, &str) -> Value,
    feature: &str,
) -> Value {
    let answer = call(
        serve,
        "mass_properties",
        &format!(r#"{{"session": "s1", "model": "current", "feature": "{feature}"}}"#),
    );
    mass(&answer)["volume"].clone()
}

/// `feature` suppressed or unsuppressed outside a transaction, at the session's revision.
fn suppressed(
    serve: &mut Serve,
    call: &mut impl FnMut(&mut Serve, &str, &str) -> Value,
    feature: &str,
    on: bool,
) -> Value {
    let status = call(serve, "status", r#"{"session": "s1"}"#);
    let at = status["session"]["revision"].as_u64().unwrap();
    let done = call(
        serve,
        "suppress_feature",
        &format!(
            r#"{{"session": "s1", "revision": {at}, "feature": "{feature}", "suppressed": {on}}}"#
        ),
    );
    assert_eq!(done["ok"], true, "{done}");
    done
}

#[test]
fn a_fillet_of_an_edge_a_suppressed_notch_shortened_fillets_the_whole_edge() {
    // The port review's case: a 10 mm cube k, a notch n (2 along the edge, 3, and 2: 12 mm³)
    // cut from the end of its Edge12 (y = 10, z = 10, along x), and a 1 mm fillet of that
    // edge, which the cut keeps under its name, shortened to 8 mm. By hand: 1000 − 12 −
    // (1 − π/4) · 8 = 986.283185. With the notch's cut suppressed, the fillet builds on the
    // cube, its edge whole: 1000 − (1 − π/4) · 10 = 997.853982. Silico keeps the reference
    // to an edge that still exists, as SolidWorks does; FreeCAD 1.1.3 fails the fillet
    // ("Invalid edge link"), its name carrying the pocket's tag (docs/ports/suppression.md,
    // *Departures*).
    let k = r#"{"id": "k0000001", "type": "box", "v": 1, "name": "Cube", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#;
    let n = r#"{"id": "n0000001", "type": "box", "v": 1, "name": "Notch", "position": [8, 7, 8], "length": 2, "width": 3, "height": 2}"#;
    let j = r#"{"id": "j0000001", "type": "cut", "v": 1, "name": "Notched", "base": "k0000001", "tool": "n0000001"}"#;
    let f = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "j0000001", "edges": ["k0000001.box.Edge12"], "radius": 1}"#;
    let (mut serve, mut call) = served("notched.silico", &[k, n, j, f]);
    let spandrel = 1.0 - PI / 4.0;
    measured(
        "notched",
        &volume_of(&mut serve, &mut call, "f0000001"),
        1000.0 - 12.0 - spandrel * 8.0,
    );
    let done = suppressed(&mut serve, &mut call, "j0000001", true);
    // The fillet names the cube's edge, which reaches its base by the cut's base: not lost.
    assert_eq!(
        done["result"]["suppressed"],
        serde_json::json!(["j0000001"])
    );
    measured(
        "the notch suppressed",
        &volume_of(&mut serve, &mut call, "f0000001"),
        1000.0 - spandrel * 10.0,
    );
    call(&mut serve, "close", r#"{"session": "s1"}"#);
    serve.end();
}

#[test]
fn a_pocket_through_a_suppressed_pad_cuts_the_pad_beneath() {
    // The port note's FreeCAD case (observed in 1.1.3: 984.292037): a 10 mm cube padded from
    // a square, a 5 mm square padded 15 mm on it (fused: 125 mm³ more above it), and a
    // pocket of a 1 mm circle 5 mm deep into them from below. By hand: 1125 − 5π; with the
    // second pad suppressed, the pocket cuts the cube: 1000 − 5π = 984.292037.
    let s1 = square("s0000001", 'a', 10);
    let p1 = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Cube", "sketch": "s0000001", "extent": {"type": "one_side", "length": 10, "reversed": false}}"#;
    let s2 = square("s0000002", 'b', 5);
    let q1 = r#"{"id": "q0000001", "type": "pad", "v": 1, "name": "Post", "base": "p0000001", "sketch": "s0000002", "extent": {"type": "one_side", "length": 15, "reversed": false}}"#;
    let s3 = r#"{"id": "s0000003", "type": "sketch", "v": 1, "name": "Hole", "plane": "xy", "points": [{"id": "pc000001", "x": 2.5, "y": 2.5, "construction": false}], "lines": [], "circles": [{"id": "cc000001", "center": "pc000001", "radius": 1, "construction": false}], "arcs": [], "constraints": []}"#;
    let k1 = r#"{"id": "k0000001", "type": "pocket", "v": 1, "name": "Hole", "base": "q0000001", "sketch": "s0000003", "extent": {"type": "one_side", "length": 5, "reversed": true}}"#;
    let (mut serve, mut call) = served("post.silico", &[&s1, p1, &s2, q1, s3, k1]);
    measured(
        "the pocket",
        &volume_of(&mut serve, &mut call, "k0000001"),
        1125.0 - 5.0 * PI,
    );
    let done = suppressed(&mut serve, &mut call, "q0000001", true);
    assert_eq!(
        done["result"]["suppressed"],
        serde_json::json!(["q0000001"])
    );
    measured(
        "the post suppressed",
        &volume_of(&mut serve, &mut call, "k0000001"),
        1000.0 - 5.0 * PI,
    );
    call(&mut serve, "close", r#"{"session": "s1"}"#);
    serve.end();
}
