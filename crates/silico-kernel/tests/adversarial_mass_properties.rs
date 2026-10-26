//! break-it for M4.5's `mass_properties` on real solids, rebuilt on kernel workers: curved
//! solids whose centre isn't a corner's midpoint (a hemisphere, a cone), small curved solids
//! far from the origin (where the moments OCCT integrates about the origin dwarf the
//! volume), and boxes of edge-biased sizes and places, each against its closed form.

use super::model::{checked, published};

use std::path::Path;
use std::sync::{Arc, OnceLock};

use proptest::prelude::*;
use silico_core::command::measure::{MassProperties, mass_properties};
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

fn engine() -> &'static Recompute<KernelPool> {
    static ENGINE: OnceLock<Recompute<KernelPool>> = OnceLock::new();
    ENGINE
        .get_or_init(|| Recompute::new(KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap()))
}

fn model_of(features: &[String]) -> Arc<Model> {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    );
    published(engine(), checked(&text))
}

fn only(model: &Model) -> MassProperties {
    let mut bodies = mass_properties(model, None).unwrap().bodies;
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    bodies.remove(0)
}

fn mm(text: String) -> f64 {
    text.parse().unwrap()
}

/// Panics unless `got` is `want` to the fingerprint's 6 decimals (half a millionth) or to a
/// relative 1e-9, whichever is looser: the design's promise, as the rounding allows it.
fn near(what: &str, got: f64, want: f64) {
    let allowed = 5e-7 + 1e-9 * want.abs();
    assert!(
        (got - want).abs() <= allowed,
        "{what}: {got}, by hand {want} (off {:e}, allowed {allowed:e})",
        (got - want).abs()
    );
}

fn centre(body: &MassProperties) -> [f64; 3] {
    body.center_of_mass.map(|c| mm(c.to_string()))
}

#[test]
fn a_hemispheres_centre_is_three_eighths_of_its_radius_up() {
    // A sphere of radius 10 less the box below z = 0: 2/3 π r³ = 2094.395102, its dome
    // 2πr² and its disc πr², 3πr² = 942.477796, its centre at 3r/8 = 3.75 above its disc.
    let model = model_of(&[
        r#"{"id": "ssssssss", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 10}"#.to_owned(),
        r#"{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Below", "position": [-20, -20, -20], "length": 40, "width": 40, "height": 20}"#.to_owned(),
        r#"{"id": "cccccccc", "type": "cut", "v": 1, "name": "Dome", "base": "ssssssss", "tool": "bbbbbbbb"}"#.to_owned(),
    ]);
    let body = only(&model);
    near(
        "volume",
        mm(body.volume.to_string()),
        2.0 / 3.0 * std::f64::consts::PI * 1000.0,
    );
    near(
        "area",
        mm(body.area.to_string()),
        3.0 * std::f64::consts::PI * 100.0,
    );
    let [x, y, z] = centre(&body);
    near("x", x, 0.0);
    near("y", y, 0.0);
    near("z", z, 3.75);
}

#[test]
fn a_cones_centre_is_a_quarter_of_its_height_up() {
    // r 10 to a point at h 40: πr²h/3 = 4188.790205; its base πr² and side πr√(r² + h²),
    // 1609.471100; its centre at h/4 = 10 above its base.
    let model = model_of(&[
        r#"{"id": "cccccccc", "type": "cone", "v": 1, "name": "Point", "position": [1, 2, 3], "radius1": 10, "radius2": 0, "height": 40}"#.to_owned(),
    ]);
    let body = only(&model);
    let pi = std::f64::consts::PI;
    near(
        "volume",
        mm(body.volume.to_string()),
        pi * 100.0 * 40.0 / 3.0,
    );
    near(
        "area",
        mm(body.area.to_string()),
        pi * 100.0 + pi * 10.0 * 1700.0_f64.sqrt(),
    );
    let [x, y, z] = centre(&body);
    near("x", x, 1.0);
    near("y", y, 2.0);
    near("z", z, 13.0);
}

#[test]
fn a_small_ball_far_from_the_origin_measures_as_it_does_at_the_origin() {
    // r 1 at x = 999000 (the gate's kilometre, nearly): 4/3 π = 4.188790, 4π = 12.566371,
    // centred on its centre. OCCT integrates moments about the origin, each ~1e6 times the
    // volume, so an adaptive 1e-9 on them is 1e-3 on the volume, and a micrometre on x.
    let model = model_of(&[
        r#"{"id": "ssssssss", "type": "sphere", "v": 1, "name": "Ball", "position": [999000, 0, 0], "radius": 1}"#.to_owned(),
    ]);
    let body = only(&model);
    let pi = std::f64::consts::PI;
    near("volume", mm(body.volume.to_string()), 4.0 / 3.0 * pi);
    near("area", mm(body.area.to_string()), 4.0 * pi);
    let [x, y, z] = centre(&body);
    near("x", x, 999000.0);
    near("y", y, 0.0);
    near("z", z, 0.0);
}

#[test]
fn a_small_ring_far_from_the_origin_measures_as_it_does_at_the_origin() {
    // R 10, r 2 at (-500000, 500000, 250000): 2π²Rr² = 789.568352 and 4π²Rr the same.
    let model = model_of(&[
        r#"{"id": "tttttttt", "type": "torus", "v": 1, "name": "Ring", "position": [-500000, 500000, 250000], "radius1": 10, "radius2": 2}"#.to_owned(),
    ]);
    let body = only(&model);
    let pi = std::f64::consts::PI;
    near(
        "volume",
        mm(body.volume.to_string()),
        2.0 * pi * pi * 10.0 * 4.0,
    );
    near(
        "area",
        mm(body.area.to_string()),
        4.0 * pi * pi * 10.0 * 2.0,
    );
    let [x, y, z] = centre(&body);
    near("x", x, -500000.0);
    near("y", y, 500000.0);
    near("z", z, 250000.0);
}

#[test]
fn an_l_of_two_fused_boxes_is_centred_by_its_volumes_less_their_overlap() {
    // 4 × 1 × 1 and 1 × 3 × 1, overlapping in a unit cube: 4 + 3 − 1 = 6 mm³; its L's two
    // faces 2 × 6 and its sides 14 × 1, 26 mm²; moments 4(2, ½, ½) + 3(½, 3/2, ½) − (½, ½, ½)
    // = (9, 6, 3), so centred at (1.5, 1, 0.5).
    let model = model_of(&[
        r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Long", "position": [0, 0, 0], "length": 4, "width": 1, "height": 1}"#.to_owned(),
        r#"{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Up", "position": [0, 0, 0], "length": 1, "width": 3, "height": 1}"#.to_owned(),
        r#"{"id": "ffffffff", "type": "fuse", "v": 1, "name": "L", "base": "aaaaaaaa", "tool": "bbbbbbbb"}"#.to_owned(),
    ]);
    let body = only(&model);
    near("volume", mm(body.volume.to_string()), 6.0);
    near("area", mm(body.area.to_string()), 26.0);
    let [x, y, z] = centre(&body);
    near("x", x, 1.5);
    near("y", y, 1.0);
    near("z", z, 0.5);
}

#[test]
fn every_body_of_a_fuse_refused_for_making_two_solids_is_its_two_inputs_with_no_word_of_it() {
    // The fuse of two boxes apart is refused (D2: more than one solid), so it consumes
    // nothing and both boxes are bodies: `mass_properties` of every body answers Ok with the
    // two boxes, saying nothing of the failed fuse (as `elements` does; a spec question).
    let model = model_of(&[
        r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Small", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
        r#"{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Large", "position": [10, 0, 0], "length": 2, "width": 2, "height": 2}"#.to_owned(),
        r#"{"id": "ffffffff", "type": "fuse", "v": 1, "name": "Both", "base": "aaaaaaaa", "tool": "bbbbbbbb"}"#.to_owned(),
    ]);
    let bodies = mass_properties(&model, None).unwrap().bodies;
    let features: Vec<&str> = bodies.iter().map(|b| b.feature.as_str()).collect();
    assert_eq!(features, ["aaaaaaaa", "bbbbbbbb"]);
    let refused = mass_properties(
        &model,
        Some(&silico_core::document::FeatureId::new("ffffffff").unwrap()),
    )
    .unwrap_err();
    assert_eq!(
        refused.errors()[0].code(),
        silico_core::ErrorCode::KernelFailed
    );
    assert_eq!(refused.errors()[0].feature(), Some("ffffffff"));
}

/// Lengths in nanometres the gate admits, biased to the edges: 1 nm, the tolerance's
/// neighbours, odd counts (a half-nanometre centre), and large ones.
fn size_nm() -> impl Strategy<Value = i64> {
    prop_oneof![
        Just(1_i64),
        Just(101),
        Just(999),
        Just(1_000),
        Just(1_001),
        Just(123_457),
        Just(1_000_000),
        Just(99_999_999),
        1_i64..2_000,
        1_i64..100_000_000_000,
    ]
}

fn place_nm() -> impl Strategy<Value = i64> {
    prop_oneof![
        Just(0_i64),
        Just(-1),
        Just(1),
        Just(-100_000_000_000),
        Just(100_000_000_000),
        -100_000_000_000_i64..100_000_000_000,
    ]
}

fn text(nm: i64) -> String {
    let sign = if nm < 0 { "-" } else { "" };
    let n = nm.unsigned_abs();
    format!("{sign}{}.{:06}", n / 1_000_000, n % 1_000_000)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, .. ProptestConfig::default() })]

    #[test]
    fn a_boxs_centre_is_its_middle_to_the_nanometre(
        place in prop::array::uniform3(place_nm()),
        size in prop::array::uniform3(size_nm()),
    ) {
        let body = only(&model_of(&[a_box(place, size)]));
        for axis in 0..3 {
            // The middle, in half nanometres: whole or a half, rounded either way.
            let twice = 2 * i128::from(place[axis]) + i128::from(size[axis]);
            let got = i128::from(body.center_of_mass[axis].nm());
            prop_assert!(
                (2 * got - twice).abs() <= 1,
                "axis {axis}: {got} nm, the middle {}/2 nm", twice
            );
        }
    }

}

fn a_box(place: [i64; 3], size: [i64; 3]) -> String {
    format!(
        r#"{{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "B", "position": [{}, {}, {}], "length": {}, "width": {}, "height": {}}}"#,
        text(place[0]),
        text(place[1]),
        text(place[2]),
        text(size[0]),
        text(size[1]),
        text(size[2]),
    )
}
