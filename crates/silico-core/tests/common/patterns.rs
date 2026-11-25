//! For the tests of linear patterns, polar patterns, and mirrors (M6.3), on the fake kernel
//! and the real one: documents of them from text, and what a model says of them.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use silico_core::Error;
use silico_core::document::{Checked, FeatureId, read};
use silico_core::kernel::{ElementKind, Kernel};
use silico_core::naming::ElementMap;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_protocol::ShapeSummary;

use super::sweeps::{circles, pad, pocket, rectangle};

/// Through all, up from the sketch's plane under the part: a pocket's extent.
pub const THROUGH: &str = r#"{"type": "through_all", "reversed": true}"#;
/// 5 mm up: a pad's extent.
pub const FIVE: &str = r#"{"type": "one_side", "length": 5, "reversed": false}"#;
/// The plate's hole, `q0000002`, as a pattern's `copies`.
pub const HOLE: &str = r#"{"type": "features", "features": ["q0000002"]}"#;
/// The base's whole solid, as a pattern's `copies`.
pub const SOLID: &str = r#"{"type": "solid"}"#;

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

/// `n` mm up: a pad's extent.
pub fn up(n: &str) -> String {
    format!(r#"{{"type": "one_side", "length": {n}, "reversed": false}}"#)
}

/// `n` mm down: a pad's extent, reversed.
pub fn down(n: &str) -> String {
    format!(r#"{{"type": "one_side", "length": {n}, "reversed": true}}"#)
}

/// `feature` as a pattern's `copies`.
pub fn of(feature: &str) -> String {
    format!(r#"{{"type": "features", "features": ["{feature}"]}}"#)
}

/// A linear pattern's direction: `count` instances along `along` (a word or a name), its
/// distribution `kind` (`extent` or `spacing`) of `length`.
pub fn direction(along: &str, kind: &str, length: &str, count: u32) -> String {
    format!(
        r#"{{"along": "{along}", "reversed": false, "distribution": {{"type": "{kind}", "length": {length}}}, "count": {count}}}"#
    )
}

/// Linear pattern `id` on `base`, copying `copies` (its JSON) along `directions`, leaving
/// out `skip` (its JSON).
pub fn linear_as(id: &str, base: &str, copies: &str, directions: &[String], skip: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "linear_pattern", "v": 1, "name": "Pattern", "base": "{base}", "copies": {copies}, "directions": [{}], "skip": {skip}}}"#,
        directions.join(", ")
    )
}

/// Polar pattern `id` on `base`, copying `copies` (its JSON) `count` times over an extent of
/// `angle` about `axis` (a word or a name), right-handed, none skipped.
pub fn polar(id: &str, base: &str, copies: &str, axis: &str, angle: &str, count: u32) -> String {
    format!(
        r#"{{"id": "{id}", "type": "polar_pattern", "v": 1, "name": "Pattern", "base": "{base}", "copies": {copies}, "axis": "{axis}", "reversed": false, "distribution": {{"type": "extent", "angle": {angle}}}, "count": {count}, "skip": []}}"#
    )
}

/// Mirror `id` on `base`, copying `copies` (its JSON) in `plane`.
pub fn mirror(id: &str, base: &str, copies: &str, plane: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "mirror", "v": 1, "name": "Mirror", "base": "{base}", "copies": {copies}, "plane": "{plane}"}}"#
    )
}

/// The plate of docs/design/m6.0-patterns.md, section 2.2, to its corner hole: a 100 × 60
/// rectangle from (−50, −30) padded 5, and a hole r 2.75 at `at` pocketed up through it,
/// `q0000002`.
pub fn plate_with_hole_at(at: [f64; 2]) -> Vec<String> {
    vec![
        rectangle("s0000001", "xy", [-50.0, -30.0], 100.0, 60.0),
        pad("p0000001", None, "s0000001", FIVE),
        circles("s0000003", "xy", &[(at, 2.75)]),
        pocket("q0000002", "p0000001", "s0000003", THROUGH),
    ]
}

/// `features`, then `more`.
pub fn with(mut features: Vec<String>, more: &[String]) -> Vec<String> {
    features.extend(more.iter().cloned());
    features
}

/// A document of these features, under naming scheme 6.
pub fn document(features: &[String]) -> String {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 6, \"features\": [{}]}}",
        features.join(", ")
    )
}

/// A document of these features, under naming scheme 6, checked.
pub fn checked(features: &[String]) -> Checked {
    Checked::new(read(document(features).as_bytes()).unwrap()).unwrap()
}

/// `features` rebuilt and published by `engine` within `within`.
pub fn published<K: Kernel + 'static>(
    engine: &Recompute<K>,
    features: &[String],
    within: Duration,
) -> Arc<Model> {
    match engine.start(checked(features)).wait_timeout(within) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the rebuild didn't finish in {within:?}"),
    }
}

/// A built feature's element map.
pub fn map(model: &Model, feature: &str) -> ElementMap {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { .. }) => model.element_map(&id(feature)).unwrap().clone(),
        other => panic!("{feature}: {other:?}"),
    }
}

/// Every name `map` holds, written out, with its element's kind.
pub fn names(map: &ElementMap) -> Vec<(ElementKind, String)> {
    map.iter()
        .flat_map(|(element, names)| names.iter().map(move |n| (element.kind, n.to_string())))
        .collect()
}

/// A feature's failure.
pub fn failure(model: &Model, feature: &str) -> Error {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Failed(error)) => error.clone(),
        other => panic!("{feature}: {other:?}"),
    }
}

/// A built feature's solid's summary.
pub fn summary(model: &Model, feature: &str) -> ShapeSummary {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => built.summary().unwrap().clone(),
        other => panic!("{feature}: {other:?}"),
    }
}
