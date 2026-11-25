//! The plate, M6.5's done test (docs/roadmap/m6-features.md, M6.5 and *The plate*;
//! docs/design/m6.0-patterns.md, section 2.2): a scripted MCP client starts the real `silico
//! serve` in a temporary workspace and replays `tests/plate/session.jsonl`, a recorded agent
//! session that designs the plate of `tests/plate/spec.md` with a mirror, a linear pattern,
//! and a polar pattern, reads each pattern, chamfers a copy's rim found by `query`, measures
//! it, tries two edits and rolls each back, and exports it, sending each line only after the
//! reply to the one before.
//!
//! **Compared byte for byte:** every reply's `structuredContent` with the recording's, as
//! the bracket's are (`common/replay.rs`), each held to its tool's `outputSchema`.
//!
//! **Checked by hand,** never read from the recording: the volumes, worked from the spec's
//! dimensions (prisms, cylinders, and the chamfer's ring by Pappus), the pattern reads'
//! places from the document's numbers (section 9's worked examples), the chamfered rim's
//! circle, and what each edit does to the chamfer's name. The 3MF is printable as M4.8
//! promises, its volume the part's less the chords' segments; and FreeCAD 1.1.3 (the oracle)
//! reads the STEP back as one valid solid of the part's volume and box.
//!
//! **Recording.** With `SILICO_PLATE_RECORD=1` set, the session is first driven live by
//! [`record`], which plays the agent (each request built from the replies before it), and
//! `session.jsonl` is written from the real replies; the replay and every check below then
//! run on what was written. Build the workers first, then record:
//!
//! ```text
//! pixi run cargo build --workspace --bins
//! SILICO_PLATE_RECORD=1 pixi run cargo nextest run -p silico-cli -E 'test(plate)'
//! ```
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

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use hand::{PI, measured};
use replay::{Recorder, Serve, compared_initialize, replay};

/// The export's tolerance (FreeCAD's default).
const TOLERANCE: f64 = 0.1;

fn session_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/plate/session.jsonl")
}

// --- By hand ---------------------------------------------------------------------------------

/// The plate's volume with the plate padded `thick` (the spec's 5, or the edit's 6): the
/// plate, less two notches and six corner holes through it; the boss r 8 padded 10 on its
/// top; less four boss holes r 1 through both; less the chamfer's ring on a rim r 2.75, its
/// section 0.5²/2 at 2.75 + 0.5/3 from the axis, by Pappus.
fn plate(thick: f64) -> f64 {
    let plate = 100.0 * 60.0 * thick;
    let notches = 2.0 * 6.0 * 10.0 * thick;
    let holes = 6.0 * PI * 2.75 * 2.75 * thick;
    let boss = PI * 8.0 * 8.0 * 10.0;
    let boss_holes = 4.0 * PI * (thick + 10.0);
    let chamfer = 2.0 * PI * (2.75 + 0.5 / 3.0) * (0.5 * 0.5 / 2.0);
    plate - notches - holes + boss - boss_holes - chamfer
}

// --- The session's requests, as the agent wrote them -----------------------------------------

/// A rectangle sketch `id` on `xy`, its points and lines lettered `l`, from `(x, y)` `w` by
/// `h`: four horizontal and vertical constraints, its two lengths, and its first corner
/// placed from the origin (each distance written from the lesser coordinate to the greater,
/// so every value is a length above or at 0).
fn rectangle(id: &str, name: &str, l: char, (x, y): (i32, i32), (w, h): (i32, i32)) -> String {
    let corners = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    let points: Vec<String> = corners
        .iter()
        .zip(1..)
        .map(|((x, y), n)| {
            format!(r#"{{"id": "p{l}00000{n}", "x": {x}, "y": {y}, "construction": false}}"#)
        })
        .collect();
    let lines: Vec<String> = (1..=4)
        .map(|n| {
            format!(
                r#"{{"id": "n{l}00000{n}", "from": "p{l}00000{n}", "to": "p{l}00000{}", "construction": false}}"#,
                n % 4 + 1
            )
        })
        .collect();
    let mut constraints: Vec<String> = (1..=4)
        .map(|n| {
            let kind = if n % 2 == 1 { "horizontal" } else { "vertical" };
            format!(r#"{{"id": "k{l}00000{n}", "type": "{kind}", "line": "n{l}00000{n}"}}"#)
        })
        .collect();
    constraints.push(format!(
        r#"{{"id": "k{l}000005", "type": "length", "line": "n{l}000001", "value": {w}, "driving": true}}"#
    ));
    constraints.push(format!(
        r#"{{"id": "k{l}000006", "type": "length", "line": "n{l}000002", "value": {h}, "driving": true}}"#
    ));
    constraints.extend(placed(l, 7, (x, y)));
    format!(
        r#"{{"id": "{id}", "type": "sketch", "name": "{name}", "plane": "xy", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": [{}]}}"#,
        points.join(", "),
        lines.join(", "),
        constraints.join(", ")
    )
}

/// The constraints `k{l}00000{n}` and the next placing the point `p{l}000001` at `(x, y)`
/// from the origin: `distance_x` and `distance_y`, each from the lesser coordinate to the
/// greater.
fn placed(l: char, n: u32, (x, y): (i32, i32)) -> [String; 2] {
    let point = format!("p{l}000001");
    let along = |axis: &str, at: i32, k: u32| {
        let (a, b) = if at < 0 {
            (point.as_str(), "origin")
        } else {
            ("origin", point.as_str())
        };
        format!(
            r#"{{"id": "k{l}00000{k}", "type": "distance_{axis}", "a": "{a}", "b": "{b}", "value": {}, "driving": true}}"#,
            at.abs()
        )
    };
    [along("x", x, n), along("y", y, n + 1)]
}

/// A circle sketch `id` on `plane` (JSON), its centre `p{l}000001` at `(x, y)` and its circle
/// `c{l}000001` of radius `r`: the radius, and the centre placed from the origin.
fn circle(id: &str, name: &str, plane: &str, l: char, (x, y): (i32, i32), r: f64) -> String {
    let [dx, dy] = placed(l, 2, (x, y));
    format!(
        r#"{{"id": "{id}", "type": "sketch", "name": "{name}", "plane": {plane}, "points": [{{"id": "p{l}000001", "x": {x}, "y": {y}, "construction": false}}], "lines": [], "circles": [{{"id": "c{l}000001", "center": "p{l}000001", "radius": {r}, "construction": false}}], "arcs": [], "constraints": [{{"id": "k{l}000001", "type": "radius", "curve": "c{l}000001", "value": {r}, "driving": true}}, {dx}, {dy}]}}"#
    )
}

/// A pad `id` of `sketch` on `base`, `length` up its sketch's normal.
fn pad(id: &str, name: &str, base: Option<&str>, sketch: &str, length: u32) -> String {
    let base = base.map_or(String::new(), |b| format!(r#""base": "{b}", "#));
    format!(
        r#"{{"id": "{id}", "type": "pad", "name": "{name}", {base}"sketch": "{sketch}", "extent": {{"type": "one_side", "length": {length}, "reversed": false}}}}"#
    )
}

/// A pocket through everything of `base`, up from its sketch on `xy` (against the sketch's
/// normal, so `reversed`).
fn pocket(id: &str, name: &str, base: &str, sketch: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "pocket", "name": "{name}", "base": "{base}", "sketch": "{sketch}", "extent": {{"type": "through_all", "reversed": true}}}}"#
    )
}

const MIRROR: &str = r#"{"id": "m0000001", "type": "mirror", "name": "Other notch", "base": "q0000001", "copies": {"type": "features", "features": ["q0000001"]}, "plane": "yz"}"#;
const GRID: &str = r#"{"id": "t0000001", "type": "linear_pattern", "name": "Corner holes", "base": "q0000002", "copies": {"type": "features", "features": ["q0000002"]}, "directions": [{"along": "x", "reversed": false, "distribution": {"type": "extent", "length": 80}, "count": 3}, {"along": "y", "reversed": false, "distribution": {"type": "extent", "length": 40}, "count": 2}], "skip": []}"#;
const TURNED: &str = r#"{"id": "t0000002", "type": "polar_pattern", "name": "Boss holes", "base": "q0000003", "copies": {"type": "features", "features": ["q0000003"]}, "axis": "z", "reversed": false, "distribution": {"type": "extent", "angle": 360}, "count": 4, "skip": []}"#;

const INITIALIZE: &str = r#"{"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "plate-agent", "version": "1"}}}"#;
const INITIALIZED: &str = r#"{"jsonrpc": "2.0", "method": "notifications/initialized"}"#;
const MASS: &str = r#"{"session": "s1", "model": "current", "feature": null}"#;
const CHECK: &str = r#"{"session": "s1", "model": "current", "fresh": false}"#;
const SESSION: &str = r#"{"session": "s1"}"#;

fn read_pattern(pattern: &str) -> String {
    format!(r#"{{"session": "s1", "model": "current", "pattern": "{pattern}"}}"#)
}

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
        "open: a new document for the plate",
        "open",
        r#"{"path": "plate.silico", "create": true}"#,
    );
    r.call("begin a transaction", "begin", SESSION);
    r.add(
        "the plate's outline, 100 by 60 about the origin",
        &rectangle("s0000001", "Plate outline", 'a', (-50, -30), (100, 60)),
    );
    r.add(
        "the plate, padded 5",
        &pad("p0000001", "Plate", None, "s0000001", 5),
    );
    r.add(
        "the notch's outline, x 44 to 50, y -5 to 5",
        &rectangle("s0000002", "Notch outline", 'b', (44, -5), (6, 10)),
    );
    r.add(
        "the notch, pocketed up through the plate",
        &pocket("q0000001", "Notch", "p0000001", "s0000002"),
    );
    r.add("the other notch: the notch mirrored about yz", MIRROR);
    r.call(
        "read the mirror: its copy's centre should be at x = -47",
        "pattern",
        &read_pattern("m0000001"),
    );
    r.add(
        "the corner hole's outline, r 2.75 at (-40, -20)",
        &circle(
            "s0000003",
            "Corner hole outline",
            "\"xy\"",
            'c',
            (-40, -20),
            2.75,
        ),
    );
    r.add(
        "the corner hole, pocketed up through the plate",
        &pocket("q0000002", "Corner hole", "m0000001", "s0000003"),
    );
    r.add(
        "the corner holes: 3 along x over 80, 2 along y over 40",
        GRID,
    );
    r.call(
        "read the grid: copy (2, 1) should be at (40, 20)",
        "pattern",
        &read_pattern("t0000001"),
    );
    let top = r.call(
        "find the plate's top face: the largest planar face facing +Z",
        "query",
        r#"{"session": "s1", "model": "current", "feature": null, "kind": "face", "type": "plane", "stable": true, "facing": {"direction": [0, 0, 1], "within": 0}, "measures": [], "order": {"by": "area", "descending": true}, "limit": 1}"#,
    );
    let face = top["result"]["elements"][0]["name"].clone();
    r.add(
        "the boss's outline, r 8, on the plate's top face at its centre",
        &circle(
            "s0000004",
            "Boss outline",
            &format!(r#"{{"face": {face}, "x": "+x", "reversed": false}}"#),
            'd',
            (0, 0),
            8.0,
        ),
    );
    r.add(
        "the boss, padded 10",
        &pad("p0000002", "Boss", Some("t0000001"), "s0000004", 10),
    );
    r.add(
        "the boss hole's outline, r 1 at (5, 0)",
        &circle("s0000005", "Boss hole outline", "\"xy\"", 'e', (5, 0), 1.0),
    );
    r.add(
        "the boss hole, pocketed up through the plate and the boss",
        &pocket("q0000003", "Boss hole", "p0000002", "s0000005"),
    );
    r.add("the boss holes: 4 about z over a whole turn", TURNED);
    r.call(
        "read the boss holes: each turned 90° about z",
        "pattern",
        &read_pattern("t0000002"),
    );
    let rims = r.call(
        "find the corner holes' rims: circles of radius 2.75",
        "query",
        r#"{"session": "s1", "model": "current", "feature": null, "kind": "edge", "type": "circle", "stable": true, "facing": null, "measures": [{"measure": "radius", "at_least": 2.75, "at_most": 2.75}], "order": null, "limit": 20}"#,
    );
    let rim = rims["result"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["axis"]["point"] == json!([40, 20, 5]))
        .unwrap()["name"]
        .clone();
    r.add(
        "chamfer copy (2, 1)'s top rim, 0.5",
        &format!(
            r#"{{"id": "c0000001", "type": "chamfer", "name": "Corner hole rim", "base": "t0000002", "edges": [{rim}], "size": {{"distance": 0.5}}}}"#
        ),
    );
    r.call(
        "commit: the file is saved",
        "commit",
        r#"{"session": "s1", "path": null, "replace": false}"#,
    );
    r.call("check the saved model", "check", CHECK);
    r.call("measure the plate", "mass_properties", MASS);
    // The plate 6 thick: the chamfer's rim found again.
    r.call("begin: try a thicker plate", "begin", SESSION);
    let revision = r.revision;
    r.call(
        "pad the plate 6, not 5",
        "set_field",
        &format!(
            r#"{{"session": "s1", "revision": {revision}, "feature": "p0000001", "field": "extent.length", "value": 6}}"#
        ),
    );
    r.call("check it: the chamfer still finds its rim", "check", CHECK);
    r.call("measure it 6 thick", "mass_properties", MASS);
    r.call("roll it back", "rollback", SESSION);
    // Two corner holes along x: the chamfer's rim is gone.
    r.call("begin: try two holes along x", "begin", SESSION);
    let revision = r.revision;
    r.call(
        "two corner holes along x, not 3",
        "set_field",
        &format!(
            r#"{{"session": "s1", "revision": {revision}, "feature": "t0000001", "field": "directions.0.count", "value": 2}}"#
        ),
    );
    r.call(
        "check it: copy (2, 1) is gone, so the chamfer's rim is",
        "check",
        CHECK,
    );
    r.call("roll it back", "rollback", SESSION);
    r.call(
        "export it for printing",
        "export",
        &format!(
            r#"{{"session": "s1", "format": "3mf", "path": "plate.3mf", "replace": false, "features": null, "tolerance": {TOLERANCE}}}"#
        ),
    );
    r.call(
        "export it as STEP",
        "export",
        r#"{"session": "s1", "format": "step", "path": "plate.step", "replace": false, "features": null, "tolerance": null}"#,
    );
    r.call("close the session", "close", SESSION);
    r.serve.end();
    r.lines
}

// --- The answers ---------------------------------------------------------------------------

fn mass(answer: &Value) -> &Value {
    let bodies = answer["result"]["bodies"].as_array().unwrap();
    assert_eq!(bodies.len(), 1, "{answer}");
    &bodies[0]
}

/// What FreeCAD 1.1.3 reads of the STEP at `path`: its solids, whether each is valid, and
/// the first's volume and box.
fn freecad_reads_step(path: &Path) -> Value {
    silico_oracle::freecad::script(&format!(
        r#"
import json, Part
shape = Part.read({path:?})
solids = shape.Solids
box = shape.BoundBox
print("SILICO_ORACLE " + json.dumps({{
    "solids": len(solids),
    "valid": all(s.isValid() for s in solids),
    "volume": shape.Volume,
    "box": [box.XMin, box.YMin, box.ZMin, box.XMax, box.YMax, box.ZMax],
}}))
"#,
        path = path.to_str().unwrap()
    ))
    .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn the_plate_session_replays_reads_its_patterns_and_measures_as_by_hand() {
    if std::env::var_os("SILICO_PLATE_RECORD").is_some() {
        let mut text = record().join("\n");
        text.push('\n');
        std::fs::write(session_file(), text).unwrap();
    }
    let run = replay(&session_file());

    // The pattern reads, by hand (section 9): the notch, x 44 to 50 and y −5 to 5, is
    // centred at (47, 0, 0), and its mirror in x = 0 at (−47, 0, 0); the corner hole at
    // (−40, −20) steps 80 / 2 = 40 along +X and 40 / 1 = 40 along +Y, so copy (2, 1) is at
    // (40, 20, 0); the boss hole at (5, 0) turned 90°, 180°, and 270° about +Z lands at
    // (0, 5, 0), (−5, 0, 0), and (0, −5, 0). Every copy built.
    let reads = run.answers("pattern");
    assert_eq!(reads.len(), 3);
    // Each the design's example of section 5, which silico-core holds to the design's text.
    for (read, example) in reads
        .iter()
        .zip(["result-mirror", "result", "result-polar"])
    {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../silico-core/tests/contract/commands/examples/pattern")
            .join(format!("{example}.json"));
        let example: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(read["result"], example);
    }
    let mirror = &reads[0]["result"];
    assert_eq!(mirror["anchor"], json!([47, 0, 0]), "{mirror}");
    assert_eq!(
        mirror["instances"],
        json!([{"index": [1], "offset": null, "angle": null, "anchor": [-47, 0, 0], "state": "built"}])
    );
    let grid = &reads[1]["result"];
    assert_eq!(grid["anchor"], json!([-40, -20, 0]), "{grid}");
    let copies: Vec<(Value, Value, Value, Value)> = grid["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["index"].clone(),
                i["offset"].clone(),
                i["anchor"].clone(),
                i["state"].clone(),
            )
        })
        .collect();
    let by_hand: Vec<(Value, Value, Value, Value)> = [(0, 1), (1, 0), (1, 1), (2, 0), (2, 1)]
        .into_iter()
        .map(|(i, j)| {
            let index = if j == 0 { json!([i]) } else { json!([i, j]) };
            let (x, y) = (40 * i, 40 * j);
            (
                index,
                json!([x, y, 0]),
                json!([x - 40, y - 20, 0]),
                json!("built"),
            )
        })
        .collect();
    assert_eq!(copies, by_hand);
    let turned = &reads[2]["result"];
    assert_eq!(turned["anchor"], json!([5, 0, 0]), "{turned}");
    assert_eq!(
        turned["axis"],
        json!({"point": [0, 0, 0], "direction": [0, 0, 1], "step": 90})
    );
    let anchors: Vec<(Value, Value)> = turned["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["angle"].clone(), i["anchor"].clone()))
        .collect();
    assert_eq!(
        anchors,
        [
            (json!(90), json!([0, 5, 0])),
            (json!(180), json!([-5, 0, 0])),
            (json!(270), json!([0, -5, 0]))
        ]
    );

    // The top face the boss is sketched on, and the rim chamfered: copy (2, 1)'s top, a
    // circle r 2.75 about (40, 20, 5).
    let queries = run.answers("query");
    assert_eq!(queries.len(), 2);
    let top = &queries[0]["result"]["elements"][0];
    assert_eq!(top["normal"], json!([0, 0, 1]), "{top}");
    assert_eq!(top["centroid"], json!([0, 0, 5]), "{top}");
    let chamfer = run
        .arguments("add_feature")
        .into_iter()
        .map(|a| &a["feature"])
        .find(|f| f["type"] == "chamfer")
        .unwrap()
        .clone();
    let rim = queries[1]["result"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == chamfer["edges"][0])
        .unwrap();
    assert_eq!(rim["axis"]["point"], json!([40, 20, 5]));
    measured("the rim's radius", &rim["radius"], 2.75);
    // Named as the design's plate names it, by hand (section 2.2): the edge between the
    // plate's top and copy (2, 1)'s wall.
    let design = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m6.0-patterns.md"),
    )
    .unwrap();
    let named = rim["name"].as_str().unwrap();
    assert_eq!(
        named,
        "t0000001.linear_pattern.Edge(G p0000001.pad.Face(G s0000001.sketch.Face1), G t0000001.linear_pattern.Face(C2,1 q0000002.pocket.Face(G s0000003.sketch.Edge.cc000001)))"
    );
    assert!(design.contains(&format!("\"{named}\"")), "{named}");

    // Committed and checked: nothing to say. The plate: 29,400 + 352.395833π.
    let checks = run.answers("check");
    assert_eq!(checks.len(), 3);
    assert_eq!(checks[0]["result"]["problems_total"], 0, "{}", checks[0]);
    assert_eq!(checks[0]["result"]["warnings_total"], 0, "{}", checks[0]);
    let masses = run.answers("mass_properties");
    assert_eq!(masses.len(), 2);
    let whole = plate(5.0);
    assert!((whole - (29_400.0 + 352.395_833_333_333_3 * PI)).abs() < 1e-6);
    assert!((whole - 30_507.084_161).abs() < 1e-6, "{whole}");
    measured("the plate's volume", &mass(masses[0])["volume"], whole);

    // 6 thick: the chamfer found again, and 35,280 + 303.020833π.
    assert_eq!(checks[1]["result"]["problems_total"], 0, "{}", checks[1]);
    let thick = plate(6.0);
    assert!((thick - 36_231.968_024).abs() < 1e-6, "{thick}");
    measured("6 thick", &mass(masses[1])["volume"], thick);

    // Two along x: copy (2, 1) gone, the chamfer's rim with it, `reference_broken` with
    // candidates, none taken.
    let broken = &checks[2]["result"];
    assert_eq!(broken["problems_total"], 1, "{broken}");
    let problem = &broken["problems"][0];
    assert_eq!(problem["code"], "reference_broken", "{problem}");
    assert_eq!(problem["feature"], "c0000001", "{problem}");
    assert_eq!(problem["parameter"], "edges.0", "{problem}");
    assert!(
        !problem["candidates"].as_array().unwrap().is_empty(),
        "{problem}"
    );
    // Rolled back each time: the file holds the plate as committed.
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(run.work.join("plate.silico")).unwrap())
            .unwrap();
    let features = saved["features"].as_array().unwrap();
    assert_eq!(features.last().unwrap()["id"], "c0000001");
    let fingerprint = &features.last().unwrap()["fingerprint"];
    assert_eq!(fingerprint["bbox"], json!([-50, -30, 0, 50, 30, 15]));
    measured("the saved volume", &fingerprint["volume"], whole);
    let pad = features.iter().find(|f| f["id"] == "p0000001").unwrap();
    assert_eq!(pad["extent"]["length"], 5);

    // The 3MF: one object, printable as M4.8 promises, enclosing the part less its chords'
    // segments, so a little less than the part, within what 0.1 mm chords take off.
    let archive = std::fs::read(run.work.join("plate.3mf")).unwrap();
    let meshes = threemf::model(&archive);
    assert_eq!(meshes.len(), 1);
    let mesh_volume = threemf::printable(&meshes[0]);
    assert!(
        mesh_volume < whole && whole - mesh_volume < 0.01 * whole,
        "{mesh_volume} against {whole}"
    );
    // The STEP: one valid solid, its volume the part's and its box the spec's, as FreeCAD
    // reads it.
    let read = freecad_reads_step(&run.work.join("plate.step"));
    assert_eq!(read["solids"], 1, "{read}");
    assert_eq!(read["valid"], true, "{read}");
    let volume = read["volume"].as_f64().unwrap();
    assert!(
        (volume - whole).abs() < 1e-6 * whole,
        "FreeCAD's {volume}, by hand {whole}"
    );
    let found: [f64; 6] = serde_json::from_value(read["box"].clone()).unwrap();
    for (got, want) in found.into_iter().zip([-50.0, -30.0, 0.0, 50.0, 30.0, 15.0]) {
        assert!((got - want).abs() < 1e-6, "{read}");
    }
}
