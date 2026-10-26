//! For the tests of M4.5's measures on real solids (`silico_core::command::measure`): a
//! recompute on kernel workers, documents of boxes, cylinders, and sketches rebuilt on it,
//! elements picked by their entries, and `distance` and `angle` between two by name.

use std::path::Path;
use std::sync::Arc;

use silico_core::command::elements::{Entry, entries};
use silico_core::command::measure::{AngleAnswer, DistanceAnswer, angle, distance};
use silico_core::document::{Checked, FeatureId, Length, read};
use silico_core::fake::FakeSketches;
use silico_core::kernel::ElementKind;
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute, RunOutcome};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

pub fn engine() -> Recompute<KernelPool> {
    Recompute::new(KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap())
        .with_sketches(FakeSketches::new())
}

/// A document of `features`, rebuilt.
pub fn model(engine: &Recompute<KernelPool>, features: &[String]) -> Arc<Model> {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    );
    let checked = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    match engine
        .start(checked)
        .wait_timeout(std::time::Duration::from_secs(60))
    {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the rebuild didn't finish within a minute"),
    }
}

pub fn block(id: &str, at: [f64; 3], size: [f64; 3]) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "B{id}", "position": [{}, {}, {}], "length": {}, "width": {}, "height": {}}}"#,
        at[0], at[1], at[2], size[0], size[1], size[2]
    )
}

pub fn cylinder(id: &str, at: [f64; 3], radius: f64, height: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "cylinder", "v": 1, "name": "C{id}", "position": [{}, {}, {}], "radius": {radius}, "height": {height}}}"#,
        at[0], at[1], at[2]
    )
}

/// A sketch `id` on `plane` of one closed polygon through `points`, its points `p<tag>00000N`
/// and its lines `n<tag>00000N`, line N from point N to the next.
pub fn polygon(id: &str, tag: char, plane: &str, points: &[[f64; 2]]) -> String {
    let point = |i: usize| format!("p{tag}{:06}", i + 1);
    let line = |i: usize| format!("n{tag}{:06}", i + 1);
    let ps: Vec<String> = points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            format!(
                r#"{{"id": "{}", "x": {:?}, "y": {:?}, "construction": false}}"#,
                point(i),
                p[0],
                p[1]
            )
        })
        .collect();
    let ls: Vec<String> = (0..points.len())
        .map(|i| {
            format!(
                r#"{{"id": "{}", "from": "{}", "to": "{}", "construction": false}}"#,
                line(i),
                point(i),
                point((i + 1) % points.len())
            )
        })
        .collect();
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "Sk{id}", "plane": "{plane}", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
        ps.join(", "),
        ls.join(", ")
    )
}

/// A whole turn of `sketch` about `axis` (`x_axis`, `y_axis`, or a line's ID).
pub fn revolve(id: &str, sketch: &str, axis: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "revolve", "v": 1, "name": "R{id}", "sketch": "{sketch}", "axis": "{axis}", "extent": {{"type": "one_side", "angle": 360, "reversed": false}}}}"#
    )
}

/// The entry of the feature `feature`'s element that `pick` picks, the one there is.
pub fn the(model: &Model, feature: &str, pick: impl Fn(&Entry) -> bool) -> Entry {
    let all = entries(model, &FeatureId::new(feature).unwrap()).unwrap();
    let picked: Vec<&Entry> = all.iter().filter(|e| pick(e)).collect();
    assert_eq!(picked.len(), 1, "{feature}: {picked:#?}");
    picked[0].clone()
}

/// The name of the feature `feature`'s element that `pick` picks, the one there is.
pub fn named(model: &Model, feature: &str, pick: impl Fn(&Entry) -> bool) -> String {
    the(model, feature, pick).name.to_string()
}

pub fn mm(nm: i64) -> Length {
    Length::from_nm(nm)
}

pub fn point(p: [f64; 3]) -> [Length; 3] {
    p.map(|c| Length::rounded(c).unwrap())
}

pub fn measured(model: &Model, engine: &Recompute<KernelPool>, a: &str, b: &str) -> DistanceAnswer {
    distance(model, engine.kernel(), &[a.to_owned(), b.to_owned()], None)
        .unwrap_or_else(|e| panic!("{a} to {b}: {:?}", e.errors()))
}

pub fn angled(model: &Model, a: &str, b: &str) -> AngleAnswer {
    angle(model, &[a.to_owned(), b.to_owned()], None)
        .unwrap_or_else(|e| panic!("{a} and {b}: {:?}", e.errors()))
}

/// A face whose outward normal is `n`.
pub fn facing(n: [f64; 3]) -> impl Fn(&Entry) -> bool {
    move |e| e.normal == Some(n)
}

/// The straight edge from `from` to `to`, either way round.
pub fn along(from: [f64; 3], to: [f64; 3]) -> impl Fn(&Entry) -> bool {
    let (from, to) = (point(from), point(to));
    move |e| {
        e.kind == ElementKind::Edge
            && e.straight == Some(true)
            && ((e.start, e.end) == (Some(from), Some(to))
                || (e.start, e.end) == (Some(to), Some(from)))
    }
}

/// The vertex at `p`.
pub fn at(p: [f64; 3]) -> impl Fn(&Entry) -> bool {
    let p = point(p);
    move |e| e.kind == ElementKind::Vertex && e.position == Some(p)
}
