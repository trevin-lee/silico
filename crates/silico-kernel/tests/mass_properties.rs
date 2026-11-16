//! M4.5: `mass_properties` of real solids, rebuilt on kernel workers
//! (docs/design/m4.1-command-api.md, section 7): each solid's volume, area, and centre of
//! mass, every expected value by hand from its dimensions.

use super::example;
use super::model::{checked, id, published};

use std::path::Path;

use silico_core::command::measure::{MassProperties, mass_properties};
use silico_core::document::{Length, Measure};
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// A box, a cylinder, and a sphere, apart, each a body.
const THREE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 6,
  "features": [
    {"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 10, "width": 20, "height": 30},
    {"id": "cccccccc", "type": "cylinder", "v": 1, "name": "Cylinder", "position": [100, 0, 0], "radius": 2.5, "height": 10},
    {"id": "ssssssss", "type": "sphere", "v": 1, "name": "Sphere", "position": [300, 0, 0], "radius": 3}
  ]
}
"#;

fn model_of(text: &str) -> std::sync::Arc<Model> {
    let pool = KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap();
    published(&Recompute::new(pool), checked(text))
}

/// `feature`'s mass properties, of `volume` and `area` (in millionths) and centre `center`
/// (in nanometres).
fn body(feature: &str, volume: i128, area: i128, center: [i64; 3]) -> MassProperties {
    MassProperties {
        feature: id(feature),
        volume: Measure::from_millionths(volume),
        area: Measure::from_millionths(area),
        center_of_mass: center.map(Length::from_nm),
    }
}

#[test]
fn each_bodys_volume_area_and_centre_are_its_dimensions() {
    let model = model_of(THREE);
    assert_eq!(
        mass_properties(&model, None).unwrap().bodies,
        [
            // 10 × 20 × 30: 6000, 2 × (200 + 300 + 600) = 2200, centred at (5, 10, 15).
            body(
                "bbbbbbbb",
                6_000_000_000,
                2_200_000_000,
                [5_000_000, 10_000_000, 15_000_000]
            ),
            // r 2.5, h 10: 62.5π = 196.349540849, and its side 50π (157.079632679) with its
            // two ends 12.5π (39.269908170): 62.5π again; centred halfway up.
            body(
                "cccccccc",
                196_349_541,
                196_349_541,
                [100_000_000, 0, 5_000_000]
            ),
            // r 3: 36π = 113.097335529 both, centred on its centre.
            body("ssssssss", 113_097_336, 113_097_336, [300_000_000, 0, 0]),
        ]
    );
}

#[test]
fn a_consumed_features_own_solid_and_the_cut_that_consumed_it_are_each_measured() {
    let model = model_of(example::EXAMPLE);
    // The 40 × 20 × 10 block: 8000, 2800, centred at (20, 10, 5).
    assert_eq!(
        mass_properties(&model, Some(&id("k7f2qx3m")))
            .unwrap()
            .bodies,
        [body(
            "k7f2qx3m",
            8_000_000_000,
            2_800_000_000,
            [20_000_000, 10_000_000, 5_000_000]
        )]
    );
    // The pin, r 2.5, from z −1 to 11: 75π = 235.619449019; 60π + 12.5π = 72.5π =
    // 227.765467385; centred at z 5.
    assert_eq!(
        mass_properties(&model, Some(&id("a91cw0de")))
            .unwrap()
            .bodies,
        [body(
            "a91cw0de",
            235_619_449,
            227_765_467,
            [20_000_000, 10_000_000, 5_000_000]
        )]
    );
    // The block less the hole: 8000 − 62.5π = 7803.650459151; 2800 less the two ends'
    // 12.5π, plus the wall's 50π, = 2800 + 37.5π = 2917.809724510; still centred.
    assert_eq!(
        mass_properties(&model, None).unwrap().bodies,
        [body(
            "m3tq8v0z",
            7_803_650_459,
            2_917_809_725,
            [20_000_000, 10_000_000, 5_000_000]
        )]
    );
}
