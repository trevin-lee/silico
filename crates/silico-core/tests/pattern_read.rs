//! M6.5: the `pattern` read (docs/design/m6.0-patterns.md, section 5), held to the design's
//! own text: its examples, each the design's block; its request read and written back
//! byte for byte; each result example held to the committed snapshot, and broken ones
//! refused by it; and its refusals made on the design's plate, through the session's own
//! command, each equal to its example byte for byte. And the plate's three patterns read on
//! the fake kernel, every place by hand from the document's numbers (section 9's worked
//! examples): the fake places nothing, but a read's geometry is the core's alone. The real
//! kernel's results, byte for byte, are silico-kernel's `pattern_read.rs`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use silico_core::command::pattern::{InstanceState, PatternRead};
use silico_core::command::wire::pattern::PatternRequest;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{ModelChoice, Response, Revision, Running, SessionId, Sessions};
use silico_core::document::json::read;
use silico_core::document::{FeatureId, Length};
use silico_core::fake::{FakeKernel, FakeSketches};

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

/// The design's text between the headings `from` and `to`.
fn section(from: &str, to: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m6.0-patterns.md");
    let doc = std::fs::read_to_string(&path).unwrap();
    let start = doc.find(from).unwrap_or_else(|| panic!("no {from}"));
    let end = start + doc[start..].find(to).unwrap_or_else(|| panic!("no {to}"));
    doc[start..end].to_owned()
}

/// The JSON code blocks in `section`, in order.
fn blocks(section: &str) -> Vec<String> {
    section
        .split("```json\n")
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap().to_owned())
        .collect()
}

/// The plate of section 2.2, as the design writes it.
fn plate_text() -> String {
    let plate = blocks(&section("### 2.2 The plate", "### 2.3 Other forms"));
    format!("{}\n", plate[0])
}

fn contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract")
        .join(name)
}

/// The example `pattern/name`, as committed.
fn example(name: &str) -> String {
    let path = contract(&format!("commands/examples/pattern/{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `value` as the API writes it, and a newline, as each example file holds it.
fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap() + "\n"
}

/// The committed snapshot's `part` (`request` or `result`).
fn schema(part: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(contract("commands/pattern.json")).unwrap();
    serde_json::from_str::<serde_json::Value>(&text).unwrap()[part].clone()
}

fn valid(schema: &serde_json::Value, text: &str) -> Result<(), Vec<String>> {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&value)
        .map(|e| e.to_string())
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[test]
fn the_pattern_examples_are_the_designs_own_text() {
    // Section 5's blocks, in order: the request, the corner holes, the boss holes, the
    // notch's mirror, and the two refusals.
    let blocks = blocks(&section("## 5. The `pattern` read", "## 6. Timeline edits"));
    let names = [
        "request",
        "result",
        "result-polar",
        "result-mirror",
        "refused-not-a-pattern",
        "refused-suppressed",
    ];
    assert_eq!(blocks.len(), names.len());
    for (block, name) in blocks.iter().zip(names) {
        assert_eq!(format!("{block}\n"), example(name), "{name}");
    }
}

#[test]
fn a_pattern_request_is_read_written_back_and_holds_to_its_schema() {
    let text = example("request");
    let request = PatternRequest::read(&read(text.as_bytes()).unwrap()).unwrap();
    assert_eq!(
        request,
        PatternRequest {
            session: SessionId::parse("s1").unwrap(),
            model: ModelChoice::Current,
            pattern: id("t0000001"),
        }
    );
    assert_eq!(written(&request), text);
    let schema = schema("request");
    assert_eq!(valid(&schema, &text), Ok(()));
    let published = r#"{"session": "s4", "model": "published", "pattern": "m0000001"}"#;
    assert_eq!(
        PatternRequest::read(&read(published.as_bytes()).unwrap()).unwrap(),
        PatternRequest {
            session: SessionId::parse("s4").unwrap(),
            model: ModelChoice::Published,
            pattern: id("m0000001"),
        }
    );
    // Every field is given, and no other: each refused by the reader, naming the field, and
    // by the schema.
    for (text, field) in [
        (r#"{"session": "s1", "model": "current"}"#, "pattern"),
        (r#"{"session": "s1", "pattern": "t0000001"}"#, "model"),
        (
            r#"{"session": "s1", "model": "current", "pattern": "t0000001", "feature": "t0000001"}"#,
            "feature",
        ),
        (
            r#"{"session": "s1", "model": "current", "pattern": "T0000001"}"#,
            "pattern",
        ),
        (
            r#"{"session": "s1", "model": "latest", "pattern": "t0000001"}"#,
            "model",
        ),
    ] {
        let problems = PatternRequest::read(&read(text.as_bytes()).unwrap()).unwrap_err();
        let parameters: Vec<Option<&str>> =
            problems.errors().iter().map(|e| e.parameter()).collect();
        assert_eq!(parameters, [Some(field)], "{text}");
        assert!(valid(&schema, text).is_err(), "{text}");
    }
}

#[test]
fn each_result_example_holds_to_its_schema_and_a_broken_one_doesnt() {
    let schema = schema("result");
    let corner = example("result");
    for name in ["result", "result-polar", "result-mirror"] {
        assert_eq!(valid(&schema, &example(name)), Ok(()), "{name}");
    }
    // What the read may give where the rebuild couldn't say or place: a null state, a null
    // anchor, no instances, and a problem.
    for (from, to) in [
        (
            r#""anchor": [40, 20, 0], "state": "built""#,
            r#""anchor": [40, 20, 0], "state": null"#,
        ),
        (
            r#""anchor": [40, 20, 0], "state": "built""#,
            r#""anchor": null, "state": "off_base""#,
        ),
        (
            r#""anchor": [40, 20, 0], "state": "built""#,
            r#""anchor": [40, 20, 0], "state": "changed_nothing""#,
        ),
        (
            r#""anchor": [40, 20, 0], "state": "built""#,
            r#""anchor": [40, 20, 0], "state": "skipped""#,
        ),
        (
            r#"{"direction": [0, 1, 0], "step": 40}"#,
            r#"{"direction": [0, 1, 0], "step": null}"#,
        ),
    ] {
        assert_eq!(corner.matches(from).count(), 1, "{from}");
        let text = corner.replacen(from, to, 1);
        assert_eq!(valid(&schema, &text), Ok(()), "{to}");
    }
    let refused = example("refused-not-a-pattern");
    let unresolved = corner
        .replacen(
            &corner[corner.find("  \"directions\"").unwrap()..corner.find("  \"axis\"").unwrap()],
            "  \"directions\": null,\n",
            1,
        )
        .replacen("\"anchor\": [-40, -20, 0]", "\"anchor\": null", 1)
        .replacen(
            &corner
                [corner.find("  \"instances\"").unwrap()..corner.find("  \"problems\"").unwrap()],
            "  \"instances\": [],\n",
            1,
        )
        .replacen(
            "\"problems\": [],\n  \"problems_total\": 0",
            &format!(
                "\"problems\": [{}],\n  \"problems_total\": 1",
                refused.trim()
            ),
            1,
        );
    assert_eq!(valid(&schema, &unresolved), Ok(()), "{unresolved}");
    // And what isn't one: each refused by the snapshot.
    for (from, to, what) in [
        (
            r#""anchor": [40, 20, 0], "state": "built""#,
            r#""anchor": [40, 20, 0], "state": "satisfied""#,
            "a state not a state's word",
        ),
        (
            r#""type": "linear_pattern""#,
            r#""type": "pattern""#,
            "a type not a pattern's",
        ),
        (
            r#""index": [2, 1]"#,
            r#""index": []"#,
            "an index of no offsets",
        ),
        (
            r#""index": [2, 1]"#,
            r#""index": [2, -1]"#,
            "a negative offset",
        ),
        (
            r#""index": [2, 1]"#,
            r#""index": "2,1""#,
            "an index as its name's text",
        ),
        (
            r#""direction": [1, 0, 0], "step": 40"#,
            r#""direction": [1, 0], "step": 40"#,
            "a direction of two components",
        ),
        (
            r#""offset": [80, 40, 0], "angle": null, "anchor": [40, 20, 0], "state": "built""#,
            r#""offset": [80, 40, 0], "angle": null, "anchor": [40, 20, 0]"#,
            "an instance with no state",
        ),
        (
            r#""offset": [80, 40, 0], "angle": null, "anchor": [40, 20, 0], "state": "built""#,
            r#""offset": [80, 40, 0], "angle": null, "anchor": [40, 20, 0], "state": "built", "name": "C2,1""#,
            "an instance with a field it hasn't",
        ),
        (
            "\"problems_total\": 0",
            "\"problems_total\": -1",
            "a negative total",
        ),
        ("  \"problems\": [],\n", "", "no problems"),
        ("  \"axis\": null,\n", "", "no axis"),
    ] {
        assert_eq!(corner.matches(from).count(), 1, "{what}");
        assert!(
            valid(&schema, &corner.replacen(from, to, 1)).is_err(),
            "{what} held"
        );
    }
}

// --- On the design's plate, through the session's own command ---------------------------

/// What a command answered, within a minute.
fn answered<T>(running: Running<T>) -> Response<T> {
    running
        .wait_timeout(Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("no answer"))
}

/// The fakes' sessions, the plate open from a file in `dir`: the sessions, the session, and
/// its revision.
fn opened(dir: &std::path::Path, text: &str) -> (Sessions<Arc<FakeKernel>>, SessionId, Revision) {
    let secret = dir.join("secret");
    let sketches = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        Arc::new(FakeKernel::new(2).unwrap()),
        move |cache| silico_core::cache::Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn silico_core::sketch::SketchSolver>),
    );
    let path = dir.join("plate.silico");
    std::fs::write(&path, text).unwrap();
    let session = answered(sessions.open(&path, false)).session.unwrap();
    (sessions, session.id, session.revision)
}

/// The one refusal of `response`, written as an example file writes it.
fn refusal<T: std::fmt::Debug>(response: Response<T>) -> String {
    let problems = response.result.unwrap_err();
    assert_eq!(problems.total(), 1, "{:?}", problems.errors());
    written(&problems.errors()[0])
}

#[test]
fn a_feature_that_isnt_a_pattern_or_is_suppressed_is_refused_as_the_design_words_it() {
    let dir = tempfile::tempdir().unwrap();
    let (sessions, session, revision) = opened(dir.path(), &plate_text());
    // A pocket: the plate's patterns, in timeline order, the candidates.
    let pocket = answered(sessions.pattern(&session, ModelChoice::Current, id("q0000002")));
    assert_eq!(refusal(pocket), example("refused-not-a-pattern"));
    // A sketch, and a feature the document hasn't, alike.
    let sketch = answered(sessions.pattern(&session, ModelChoice::Current, id("s0000001")));
    let sketch = refusal(sketch);
    assert!(
        sketch.contains(r#""message": "s0000001 is a sketch, not a pattern""#),
        "{sketch}"
    );
    let none = answered(sessions.pattern(&session, ModelChoice::Current, id("zzzzzzzz")));
    let none = refusal(none);
    for said in [
        r#""message": "the document has no feature zzzzzzzz""#,
        r#""feature": null"#,
        r#""parameter": "pattern""#,
        r#""candidates": ["m0000001", "t0000001", "t0000002"]"#,
    ] {
        assert!(none.contains(said), "{said}: {none}");
    }
    // Suppressed (with the chamfer on it, its dependent): refused, naming it.
    let suppressed = answered(sessions.suppress_feature(&session, revision, id("t0000001"), true));
    assert!(suppressed.result.is_ok(), "{:?}", suppressed.result.err());
    let read = answered(sessions.pattern(&session, ModelChoice::Current, id("t0000001")));
    assert_eq!(refusal(read), example("refused-suppressed"));
    // Its sibling, unsuppressed, is read all the same.
    let polar = answered(sessions.pattern(&session, ModelChoice::Current, id("t0000002")));
    assert!(polar.result.is_ok(), "{:?}", polar.result.err());
}

#[test]
fn a_document_with_no_pattern_offers_none_and_says_so() {
    let plate = plate_text();
    let cut = plate.find("    {\n      \"id\": \"m0000001\"").unwrap();
    let text = format!(
        "{}\n  ]\n}}\n",
        plate[..cut].trim_end().trim_end_matches(',')
    );
    let dir = tempfile::tempdir().unwrap();
    let (sessions, session, _) = opened(dir.path(), &text);
    let read = answered(sessions.pattern(&session, ModelChoice::Current, id("q0000001")));
    let refused = refusal(read);
    for said in [
        r#""message": "q0000001 is a pocket, not a pattern""#,
        r#""candidates": []"#,
        r#""suggestion": "the document has no pattern: add a linear_pattern, polar_pattern, or mirror, and read it by its ID""#,
    ] {
        assert!(refused.contains(said), "{said}: {refused}");
    }
}

/// `mm`, a length.
fn mm(value: f64) -> Length {
    Length::rounded(value).unwrap()
}

fn point(p: [f64; 3]) -> [Length; 3] {
    p.map(mm)
}

/// The plate's pattern `pattern`, read through the session on the fakes.
fn plate_read(pattern: &str) -> PatternRead {
    let dir = tempfile::tempdir().unwrap();
    let (sessions, session, _) = opened(dir.path(), &plate_text());
    let response = answered(sessions.pattern(&session, ModelChoice::Current, id(pattern)));
    response
        .result
        .unwrap_or_else(|problems| panic!("{pattern}: {:?}", problems.errors()))
}

#[test]
fn the_plates_corner_holes_are_read_where_the_design_works_them_by_hand() {
    // The hole r 2.75 at (−40, −20) on xy: its profile's centre is its circle's. Direction 1
    // is +X, 80 over 3 instances, so 40 a step; direction 2 +Y, 40 over 2, so 40. Instance
    // (i, j) moves (40i, 40j, 0), its anchor at (−40 + 40i, −20 + 40j, 0): (2, 1)'s at
    // (40, 20, 0) (section 9).
    let read = plate_read("t0000001");
    assert_eq!(read.anchor, Some(point([-40.0, -20.0, 0.0])));
    let directions = read.directions.as_ref().unwrap();
    assert_eq!(directions.len(), 2);
    assert_eq!(directions[0].direction, [1.0, 0.0, 0.0]);
    assert_eq!(directions[1].direction, [0.0, 1.0, 0.0]);
    assert_eq!(directions[0].step, Some(mm(40.0)));
    assert_eq!(directions[1].step, Some(mm(40.0)));
    assert_eq!((read.axis.is_none(), read.plane.is_none()), (true, true));
    let instances: Vec<(Vec<u32>, [Length; 3], [Length; 3])> = read
        .instances
        .iter()
        .map(|i| {
            (
                i.index.offsets().to_vec(),
                i.offset.unwrap(),
                i.anchor.unwrap(),
            )
        })
        .collect();
    let by_hand: Vec<(Vec<u32>, [Length; 3], [Length; 3])> =
        [(0, 1), (1, 0), (1, 1), (2, 0), (2, 1)]
            .into_iter()
            .map(|(i, j)| {
                let index = if j == 0 { vec![i] } else { vec![i, j] };
                let (x, y) = (40.0 * f64::from(i), 40.0 * f64::from(j));
                (
                    index,
                    point([x, y, 0.0]),
                    point([-40.0 + x, -20.0 + y, 0.0]),
                )
            })
            .collect();
    assert_eq!(instances, by_hand);
    assert_eq!(read.instances[4].anchor, Some(point([40.0, 20.0, 0.0])));
    assert!(read.instances.iter().all(|i| i.angle.is_none()));
    assert_eq!((read.problems.len(), read.problems_total), (0, 0));
}

#[test]
fn the_plates_boss_holes_turn_about_z_where_the_design_works_them_by_hand() {
    // The hole r 1 at (5, 0) on xy, turned about +Z through the origin, 360° over 4, so 90°
    // a step: (5, 0, 0) to (0, 5, 0), (−5, 0, 0), and (0, −5, 0) (section 9).
    let read = plate_read("t0000002");
    assert_eq!(read.anchor, Some(point([5.0, 0.0, 0.0])));
    let axis = read.axis.as_ref().unwrap();
    assert_eq!(axis.point, point([0.0, 0.0, 0.0]));
    assert_eq!(axis.direction, [0.0, 0.0, 1.0]);
    assert_eq!(axis.step.map(|a| a.ndeg()), Some(90_000_000_000));
    let instances: Vec<(Vec<u32>, i64, [Length; 3])> = read
        .instances
        .iter()
        .map(|i| {
            assert!(i.offset.is_none());
            (
                i.index.offsets().to_vec(),
                i.angle.unwrap().ndeg(),
                i.anchor.unwrap(),
            )
        })
        .collect();
    assert_eq!(
        instances,
        [
            (vec![1], 90_000_000_000, point([0.0, 5.0, 0.0])),
            (vec![2], 180_000_000_000, point([-5.0, 0.0, 0.0])),
            (vec![3], 270_000_000_000, point([0.0, -5.0, 0.0])),
        ]
    );
}

#[test]
fn the_plates_notch_mirror_lands_where_the_design_works_it_by_hand() {
    // The notch, x 44 to 50 and y −5 to 5 on xy, centred at (47, 0, 0); mirrored in yz, the
    // plane x = 0 through the origin, its copy's centre at (−47, 0, 0) (section 9).
    let read = plate_read("m0000001");
    assert_eq!(read.anchor, Some(point([47.0, 0.0, 0.0])));
    let plane = read.plane.as_ref().unwrap();
    assert_eq!(plane.origin, point([0.0, 0.0, 0.0]));
    assert_eq!(plane.normal, [1.0, 0.0, 0.0]);
    assert_eq!(read.instances.len(), 1);
    let copy = &read.instances[0];
    assert_eq!(copy.index.offsets(), [1]);
    assert_eq!((copy.offset, copy.angle), (None, None));
    assert_eq!(copy.anchor, Some(point([-47.0, 0.0, 0.0])));
}

#[test]
fn a_skipped_instance_is_listed_skipped_in_its_place() {
    // The corner holes with (1) and (2, 1) skipped: every instance but the original listed,
    // in skip's order, those two skipped; the others are where they were.
    let text = plate_text().replacen(
        "      \"skip\": []\n    },\n    {\n      \"id\": \"s0000004\"",
        "      \"skip\": [\n        [1],\n        [2, 1]\n      ]\n    },\n    {\n      \"id\": \"s0000004\"",
        1,
    );
    assert_ne!(text, plate_text());
    let dir = tempfile::tempdir().unwrap();
    let (sessions, session, _) = opened(dir.path(), &text);
    let read = answered(sessions.pattern(&session, ModelChoice::Current, id("t0000001")))
        .result
        .unwrap();
    let states: Vec<(String, Option<InstanceState>)> = read
        .instances
        .iter()
        .map(|i| (i.index.to_string(), i.state))
        .collect();
    let skipped: Vec<&str> = states
        .iter()
        .filter(|(_, s)| *s == Some(InstanceState::Skipped))
        .map(|(i, _)| i.as_str())
        .collect();
    assert_eq!(skipped, ["1", "2,1"], "{states:?}");
    assert_eq!(
        states.iter().map(|(i, _)| i.as_str()).collect::<Vec<_>>(),
        ["0,1", "1", "1,1", "2", "2,1"]
    );
    assert_eq!(read.instances[4].anchor, Some(point([40.0, 20.0, 0.0])));
}

/// A document under scheme 6 of these features.
fn document(features: &[String]) -> String {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 6, \"features\": [{}]}}\n",
        features.join(", ")
    )
}

/// A sketch `id` on xy: a `w` by `h` rectangle from `(x, y)`, lettered `l`, and, inside it,
/// circles at `holes` (centre and radius), lettered `m`.
fn sketch(
    id: &str,
    l: char,
    (x, y, w, h): (f64, f64, f64, f64),
    holes: &[([f64; 2], f64)],
) -> String {
    let corners = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    let mut points: Vec<String> = corners
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
    let mut circles = Vec::new();
    for (n, ([cx, cy], r)) in holes.iter().enumerate() {
        points.push(format!(
            r#"{{"id": "pm00000{n}", "x": {cx}, "y": {cy}, "construction": false}}"#
        ));
        circles.push(format!(
            r#"{{"id": "cm00000{n}", "center": "pm00000{n}", "radius": {r}, "construction": false}}"#
        ));
    }
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [{}], "circles": [{}], "arcs": [], "constraints": []}}"#,
        points.join(", "),
        lines.join(", "),
        circles.join(", ")
    )
}

fn circle_sketch(id: &str, [x, y]: [f64; 2], r: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{{"id": "pz000001", "x": {x}, "y": {y}, "construction": false}}], "lines": [], "circles": [{{"id": "cz000001", "center": "pz000001", "radius": {r}, "construction": false}}], "arcs": [], "constraints": []}}"#
    )
}

fn feature(id: &str, kind: &str, base: Option<&str>, sketch: &str, extent: &str) -> String {
    let base = base.map_or(String::new(), |b| format!(r#""base": "{b}", "#));
    format!(
        r#"{{"id": "{id}", "type": "{kind}", "v": 1, "name": "F", {base}"sketch": "{sketch}", "extent": {extent}}}"#
    )
}

const UP: &str = r#"{"type": "one_side", "length": 5, "reversed": false}"#;
const THROUGH: &str = r#"{"type": "through_all", "reversed": true}"#;

#[test]
fn several_originals_are_anchored_at_their_profiles_centre_of_area_a_holes_taken_away() {
    // Two pockets copied together: a 10 × 10 square about (20, 0) with a hole r 2 at
    // (22, 0) in it, so a region of area 100 − 4π centred (2000 − 4π · 22) / (100 − 4π)
    // along x; and a disc r 3 at (−20, 10), area 9π. The anchor is their areas' centre:
    // Σ Aᵢcᵢ / Σ Aᵢ, by hand, and the copy 15 along y is it moved 15.
    let pi = std::f64::consts::PI;
    let (a1, a2) = (100.0 - 4.0 * pi, 9.0 * pi);
    let c1 = [(100.0 * 20.0 - 4.0 * pi * 22.0) / a1, 0.0];
    let c2 = [-20.0, 10.0];
    let anchor = [
        (a1 * c1[0] + a2 * c2[0]) / (a1 + a2),
        (a1 * c1[1] + a2 * c2[1]) / (a1 + a2),
        0.0,
    ];
    let text = document(&[
        sketch("s0000001", 'a', (-50.0, -30.0, 100.0, 60.0), &[]),
        feature("p0000001", "pad", None, "s0000001", UP),
        sketch("s0000002", 'b', (15.0, -5.0, 10.0, 10.0), &[([22.0, 0.0], 2.0)]),
        feature("q0000001", "pocket", Some("p0000001"), "s0000002", THROUGH),
        circle_sketch("s0000003", [-20.0, 10.0], 3.0),
        feature("q0000002", "pocket", Some("q0000001"), "s0000003", THROUGH),
        r#"{"id": "t0000001", "type": "linear_pattern", "v": 1, "name": "T", "base": "q0000002", "copies": {"type": "features", "features": ["q0000002", "q0000001"]}, "directions": [{"along": "y", "reversed": true, "distribution": {"type": "spacing", "length": 15}, "count": 2}], "skip": []}"#.to_owned(),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let (sessions, session, _) = opened(dir.path(), &text);
    let read = answered(sessions.pattern(&session, ModelChoice::Current, id("t0000001")))
        .result
        .unwrap_or_else(|p| panic!("{:?}", p.errors()));
    assert_eq!(read.anchor, Some(point(anchor)));
    let directions = read.directions.unwrap();
    assert_eq!(directions[0].direction, [0.0, -1.0, 0.0]);
    assert_eq!(directions[0].step, Some(mm(15.0)));
    assert_eq!(read.instances.len(), 1);
    assert_eq!(read.instances[0].offset, Some(point([0.0, -15.0, 0.0])));
    assert_eq!(
        read.instances[0].anchor,
        Some(point([anchor[0], anchor[1] - 15.0, 0.0]))
    );
}

#[test]
fn a_direction_of_one_instance_has_no_step_by_extent_and_its_spacing_by_spacing() {
    // An extent over one instance has no neighbours to step to; a spacing is its value.
    // Neither places anything along it, and a polar extent of one alike.
    let base = [
        sketch("s0000001", 'a', (-50.0, -30.0, 100.0, 60.0), &[]),
        feature("p0000001", "pad", None, "s0000001", UP),
        circle_sketch("s0000003", [-40.0, -20.0], 2.75),
        feature("q0000002", "pocket", Some("p0000001"), "s0000003", THROUGH),
    ];
    let pattern = r#"{"id": "t0000001", "type": "linear_pattern", "v": 1, "name": "T", "base": "q0000002", "copies": {"type": "features", "features": ["q0000002"]}, "directions": [{"along": "x", "reversed": false, "distribution": {"type": "extent", "length": 80}, "count": 1}, {"along": "y", "reversed": false, "distribution": {"type": "spacing", "length": 7.5}, "count": 2}], "skip": []}"#;
    let polar = r#"{"id": "t0000002", "type": "polar_pattern", "v": 1, "name": "P", "base": "t0000001", "copies": {"type": "features", "features": ["q0000002"]}, "axis": "z", "reversed": true, "distribution": {"type": "extent", "angle": 90}, "count": 1, "skip": []}"#;
    let mut features: Vec<String> = base.to_vec();
    features.push(pattern.to_owned());
    features.push(polar.to_owned());
    let dir = tempfile::tempdir().unwrap();
    let (sessions, session, _) = opened(dir.path(), &document(&features));
    let read = answered(sessions.pattern(&session, ModelChoice::Current, id("t0000001")))
        .result
        .unwrap_or_else(|p| panic!("{:?}", p.errors()));
    let directions = read.directions.unwrap();
    assert_eq!(directions[0].step, None);
    assert_eq!(directions[1].step, Some(mm(7.5)));
    let indexes: Vec<String> = read.instances.iter().map(|i| i.index.to_string()).collect();
    assert_eq!(indexes, ["0,1"]);
    assert_eq!(read.instances[0].offset, Some(point([0.0, 7.5, 0.0])));
    let polar = answered(sessions.pattern(&session, ModelChoice::Current, id("t0000002")))
        .result
        .unwrap_or_else(|p| panic!("{:?}", p.errors()));
    let axis = polar.axis.unwrap();
    assert_eq!(axis.step, None);
    // Reversed: its axis −Z.
    assert_eq!(axis.direction, [0.0, 0.0, -1.0]);
    assert!(polar.instances.is_empty());
}
