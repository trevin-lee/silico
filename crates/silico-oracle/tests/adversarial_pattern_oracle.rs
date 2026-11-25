//! /break-it on the pattern oracle's harness (M6.4, `silico-oracle`'s `features.rs`): a
//! pattern on a base Silico refuses, a reference Silico can't describe, polar extents at
//! FreeCAD's whole-turn tolerance, a direction of one instance along a face, and skips at
//! each count's edge. Expected values by hand; FreeCAD 1.1.3 as the oracle.
//! Needs the FreeCAD oracle (`pixi install -e oracle`) and the workers built.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_oracle::features::{self, Cases};

/// A document of `features` (JSON, after the header), under scheme 6.
fn document(features: &str) -> Vec<u8> {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 6, \"features\": [{features}]}}"
    )
    .into_bytes()
}

fn load(text: &str) -> Result<Option<Cases>, String> {
    let temp = tempfile::Builder::new()
        .prefix("silico-adversarial-patterns-")
        .tempdir()
        .unwrap();
    std::fs::write(temp.path().join(features::PATTERNS), document(text)).unwrap();
    features::load_patterns(temp.path())
}

/// A 40 x 20 block 10 tall, `p0000001`, sketched on xy as `s0000001`.
const BLOCK: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy",
  "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 40, "y": 0, "construction": false}, {"id": "pa000003", "x": 40, "y": 20, "construction": false}, {"id": "pa000004", "x": 0, "y": 20, "construction": false}],
  "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}],
  "circles": [], "arcs": [], "constraints": []},
  {"id": "p0000001", "type": "pad", "v": 1, "name": "block", "sketch": "s0000001", "extent": {"type": "one_side", "length": 10, "reversed": false}, "note": "Expected: volume 8000."}"#;

/// A boss r 2 at (30, 10), `p0000002`, padded 15 from xy onto the block, 5 above it.
const BOSS: &str = r#"{"id": "s0000002", "type": "sketch", "v": 1, "name": "C", "plane": "xy", "points": [{"id": "pa000001", "x": 30, "y": 10, "construction": false}], "lines": [], "circles": [{"id": "ca000001", "center": "pa000001", "radius": 2, "construction": false}], "arcs": [], "constraints": []},
  {"id": "p0000002", "type": "pad", "v": 1, "name": "boss", "base": "p0000001", "sketch": "s0000002", "extent": {"type": "one_side", "length": 15, "reversed": false}, "note": "Expected: volume 8062.831853."}"#;

#[test]
fn a_pattern_on_a_base_silico_refuses_is_reported_and_the_rest_still_compared() {
    // A pocket whose circle lies wholly off the block removes nothing: Silico refuses it
    // (S7), so a pattern along one of its faces by name has no description to be found by
    // in FreeCAD. The run reports the pocket and the pattern, and still compares the block,
    // which agrees: it doesn't abort, losing every case.
    let features = format!(
        r#"{BLOCK},
        {{"id": "s0000003", "type": "sketch", "v": 1, "name": "H", "plane": "xy", "points": [{{"id": "pa000001", "x": 100, "y": 100, "construction": false}}], "lines": [], "circles": [{{"id": "ca000001", "center": "pa000001", "radius": 2, "construction": false}}], "arcs": [], "constraints": []}},
        {{"id": "q0000001", "type": "pocket", "v": 1, "name": "missing", "base": "p0000001", "sketch": "s0000003", "extent": {{"type": "through_all", "reversed": true}}, "note": "Expected: volume 8000."}},
        {{"id": "t0000001", "type": "linear_pattern", "v": 1, "name": "along", "base": "q0000001", "copies": {{"type": "solid"}}, "directions": [{{"along": "p0000001.pad.Face(M s0000001.sketch.Face1)", "reversed": false, "distribution": {{"type": "spacing", "length": 10}}, "count": 2}}], "skip": [], "note": "Expected: volume 16000."}}"#
    );
    let cases = load(&features).unwrap().unwrap();
    let differ = features::run(&cases).unwrap();
    let names: Vec<&str> = differ.iter().map(|d| d.case.as_str()).collect();
    assert!(names.contains(&"along"), "{differ:?}");
    assert!(!names.contains(&"block"), "{differ:?}");
    let along = differ.iter().find(|d| d.case == "along").unwrap();
    assert!(
        along
            .to_string()
            .contains("p0000001.pad.Face(M s0000001.sketch.Face1)"),
        "{along}"
    );
}

/// A disc r 25 5 thick, `p0000003`, and a boss r 3 at (20, 0) on it, `p0000004`, 5 above it:
/// 3125 pi + 45 pi.
const DISC: &str = r#"{"id": "s0000003", "type": "sketch", "v": 1, "name": "D", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [{"id": "ca000001", "center": "pa000001", "radius": 25, "construction": false}], "arcs": [], "constraints": []},
  {"id": "p0000003", "type": "pad", "v": 1, "name": "disc", "sketch": "s0000003", "extent": {"type": "one_side", "length": 5, "reversed": false}, "note": "Expected: volume 9817.477042."},
  {"id": "s0000004", "type": "sketch", "v": 1, "name": "E", "plane": "xy", "points": [{"id": "pa000001", "x": 20, "y": 0, "construction": false}], "lines": [], "circles": [{"id": "ca000001", "center": "pa000001", "radius": 3, "construction": false}], "arcs": [], "constraints": []},
  {"id": "p0000004", "type": "pad", "v": 1, "name": "lug", "base": "p0000003", "sketch": "s0000004", "extent": {"type": "one_side", "length": 10, "reversed": false}, "note": "Expected: volume 9958.848712."}"#;

#[test]
fn a_polar_extent_at_freecads_whole_turn_tolerance_is_built_alike_or_refused() {
    // PolarPattern's whole turn is |angle - 360| < 1e-7 (PP:270-280), on the same double the
    // harness checks. 1e-7 short of 360 (a double 1.0000002e-7 short) is a whole turn to
    // neither: 4 lugs spread by angle / 3, at 0, 120, 240, and 360 less 1e-7 degrees, the
    // last on the first, so 3 by volume: 3125 pi + 3 x 45 pi = 10241.592051, where a whole
    // turn's 4 would be 3305 pi = 10382.963720. Closer, the harness refuses it, which
    // FreeCAD would spread as a whole turn and Silico wouldn't.
    let polar = |angle: &str| {
        format!(
            r#"{DISC},
            {{"id": "t0000001", "type": "polar_pattern", "v": 1, "name": "turned", "base": "p0000004", "copies": {{"type": "features", "features": ["p0000004"]}}, "axis": "z", "reversed": false, "distribution": {{"type": "extent", "angle": {angle}}}, "count": 4, "skip": [], "note": "Expected: volume 10241.592051."}}"#
        )
    };
    for (angle, refused) in [
        ("359.9999999", false),
        ("359.99999991", true),
        ("359.999999999", true),
        ("360", false),
    ] {
        let loaded = load(&polar(angle));
        assert_eq!(loaded.is_err(), refused, "{angle}: {loaded:?}");
    }
    let cases = load(&polar("359.9999999")).unwrap().unwrap();
    let differ: Vec<String> = features::run(&cases)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(differ.is_empty(), "{differ:?}");
}

#[test]
fn a_direction_of_one_instance_along_a_face_beside_a_real_one_agrees() {
    // Direction 1 of count 1 along the block's front face (from the sketch's line along y =
    // 0, which the boss never touches) places nothing, its reference still found (FreeCAD
    // returns a zero step before reading it, LP:262-296); direction 2 copies the boss 8
    // back along x, to (22, 10): 8062.831853 + 20 pi = 8125.663706 by hand. (Along y it
    // would be parallel to the face's normal, which Silico refuses at any count, decision
    // 11, and FreeCAD builds.)
    let features = format!(
        r#"{BLOCK}, {BOSS},
        {{"id": "t0000001", "type": "linear_pattern", "v": 1, "name": "one", "base": "p0000002", "copies": {{"type": "features", "features": ["p0000002"]}}, "directions": [{{"along": "p0000001.pad.Face(G s0000001.sketch.Edge.na000001)", "reversed": false, "distribution": {{"type": "spacing", "length": 10}}, "count": 1}}, {{"along": "x", "reversed": true, "distribution": {{"type": "spacing", "length": 8}}, "count": 2}}], "skip": [], "note": "Expected: volume 8125.663706; box 0 0 0 40 20 15."}}"#
    );
    let cases = load(&features).unwrap().unwrap();
    let differ: Vec<String> = features::run(&cases)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(differ.is_empty(), "{differ:?}");
}

#[test]
fn skips_at_each_counts_edge_are_refused_only_where_they_take_an_instance_out() {
    // FreeCAD 1.1.3 can't leave an instance out: one in range is refused, naming it; one at
    // or past a count takes nothing out and is built.
    let linear = |counts: &str, skip: &str| {
        format!(
            r#"{BLOCK}, {BOSS},
            {{"id": "t0000001", "type": "linear_pattern", "v": 1, "name": "skipping", "base": "p0000002", "copies": {{"type": "features", "features": ["p0000002"]}}, "directions": [{counts}], "skip": {skip}, "note": "Expected: volume 1."}}"#
        )
    };
    let x = |count: u32| {
        format!(
            r#"{{"along": "x", "reversed": true, "distribution": {{"type": "spacing", "length": 5}}, "count": {count}}}"#
        )
    };
    let y = |count: u32| {
        format!(
            r#"{{"along": "y", "reversed": false, "distribution": {{"type": "spacing", "length": 2}}, "count": {count}}}"#
        )
    };
    for (counts, skip, refused) in [
        (x(3), "[[2]]", Some("[2]")),
        (x(3), "[[3]]", None),
        (x(1), "[[1]]", None),
        (format!("{}, {}", x(3), y(2)), "[[0, 1]]", Some("[0, 1]")),
        (format!("{}, {}", x(3), y(2)), "[[2, 1]]", Some("[2, 1]")),
        (format!("{}, {}", x(3), y(2)), "[[3, 1]]", None),
        (format!("{}, {}", x(3), y(2)), "[[1, 2]]", None),
        (format!("{}, {}", x(3), y(2)), "[[1]]", Some("[1]")),
        (format!("{}, {}", x(3), y(1)), "[[3], [0, 1]]", None),
    ] {
        let loaded = load(&linear(&counts, skip));
        match refused {
            Some(instance) => {
                let e = loaded.unwrap_err();
                assert!(
                    e.contains(&format!("t0000001 skips instance {instance},")),
                    "{counts} {skip}: {e}"
                );
            }
            None => assert!(loaded.unwrap().is_some(), "{counts} {skip}"),
        }
    }
}
