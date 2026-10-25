//! M4.4: the design's entries, written as the API writes them, from real solids
//! (docs/design/m4.1-command-api.md, section 2's *Element entries* and section 7): M2.1's
//! example, a 40 × 20 × 10 block less a pin of radius 2.5 through it at (20, 10), rebuilt on
//! kernel workers, its entries listed, queried, and written byte for byte as the design's
//! examples are, each committed in silico-core's `tests/contract/commands/examples/` (the
//! core's tests check each is the design's own text, and holds to its schema).

use super::example;
use super::model::{checked, published};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use silico_core::command::Revision;
use silico_core::command::elements::{
    ElementType, Facing, MeasureName, Order, Query, entries, list, query,
};
use silico_core::command::measure::mass_properties;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::document::{Angle, FeatureId};
use silico_core::kernel::ElementKind;
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

fn model() -> Arc<Model> {
    let pool = KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap();
    published(&Recompute::new(pool), checked(example::EXAMPLE))
}

/// The example `name` (`elements/first-page`), as committed.
fn example(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../silico-core/tests/contract/commands/examples")
        .join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `value` as the API writes it, and a newline, as each example file holds it.
fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap() + "\n"
}

fn revision(n: u64) -> Revision {
    let mut r = Revision::OPENED;
    for _ in 1..n {
        r = r.next().unwrap();
    }
    r
}

#[test]
fn the_designs_example_part_gives_its_entries_and_results_byte_for_byte() {
    let model = model();
    // `elements`' first page of two, at revision 6, as the design's example reads it.
    let page = list(&model, revision(6), None, None, 2).unwrap();
    assert_eq!(written(&page), example("elements/first-page"));
    // "The largest planar face facing +Z": the top, the hole cut in it.
    let found = query(
        &model,
        &Query {
            feature: None,
            kind: ElementKind::Face,
            r#type: Some(ElementType::Plane),
            stable: Some(true),
            facing: Some(Facing {
                direction: [0.0, 0.0, 1.0],
                within: Angle::from_ndeg(1_000_000_000),
            }),
            measures: Vec::new(),
            order: Some(Order {
                by: MeasureName::Area,
                descending: true,
            }),
            limit: 1,
        },
    )
    .unwrap();
    assert_eq!(written(&found), example("query/largest-face-up-found"));
    // `mass_properties` of every body: the block less its hole.
    let measured = mass_properties(&model, None).unwrap();
    assert_eq!(
        written(&measured),
        example("mass_properties/every-body-measured")
    );
    // Section 2's two entries: the top, and the hole's wall.
    let hole = entries(&model, &FeatureId::new("m3tq8v0z").unwrap()).unwrap();
    let named = |name: &str| {
        hole.iter()
            .find(|entry| entry.name.to_string() == name)
            .unwrap_or_else(|| panic!("no {name}"))
    };
    assert_eq!(
        written(named("k7f2qx3m.box.Face6")),
        example("entry/plane-face")
    );
    assert_eq!(
        written(named("m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)")),
        example("entry/cylinder-face")
    );
}

#[test]
fn the_designs_part_gives_an_edges_and_a_vertexs_entries_field_for_field() {
    // The block's vertical edge at (40, 20), from z 0 to 10 (a box's vertical edges run up,
    // as FreeCAD's Part.makeBox's do), 10 long, straight, its middle at z 5; and its corner
    // at (40, 20, 10). Every field by hand; the names are the element map's (pinned by the
    // naming tests), the rest null.
    let model = model();
    let hole = entries(&model, &FeatureId::new("m3tq8v0z").unwrap()).unwrap();
    let edge = hole
        .iter()
        .find(|e| {
            e.kind == silico_core::kernel::ElementKind::Edge
                && written(*e).contains(r#""midpoint": [40, 20, 5]"#)
        })
        .unwrap();
    assert_eq!(
        written(edge),
        format!(
            r#"{{"name": "{}", "aliases": [], "stable": true, "feature": "m3tq8v0z", "kind": "edge", "type": "line", "area": null, "length": 10, "position": null, "normal": null, "centroid": null, "start": [40, 20, 0], "end": [40, 20, 10], "midpoint": [40, 20, 5], "straight": true, "axis": null, "radius": null, "diameter": null, "major_radius": null, "minor_radius": null, "half_angle": null, "apex": null, "default_x": null}}
"#,
            edge.name
        )
    );
    let corner = hole
        .iter()
        .find(|e| written(*e).contains(r#""position": [40, 20, 10]"#))
        .unwrap();
    assert_eq!(
        written(corner),
        format!(
            r#"{{"name": "{}", "aliases": [], "stable": true, "feature": "m3tq8v0z", "kind": "vertex", "type": "point", "area": null, "length": null, "position": [40, 20, 10], "normal": null, "centroid": null, "start": null, "end": null, "midpoint": null, "straight": null, "axis": null, "radius": null, "diameter": null, "major_radius": null, "minor_radius": null, "half_angle": null, "apex": null, "default_x": null}}
"#,
            corner.name
        )
    );
}
