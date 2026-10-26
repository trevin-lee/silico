//! M4.4: the design's entries, written as the API writes them, from real solids
//! (docs/design/m4.1-command-api.md, section 2's *Element entries* and section 7): M2.1's
//! example, a 40 × 20 × 10 block less a pin of radius 2.5 through it at (20, 10), rebuilt on
//! kernel workers, its entries listed, queried, and written byte for byte as the design's
//! examples are, each committed in silico-core's `tests/contract/commands/examples/` (the
//! core's tests check each is the design's own text, and holds to its schema). It also
//! writes section 7's `check` example of a `reference_broken` fillet, on the part with its
//! pin shortened, its candidate the real cut's element map's.

use super::example;
use super::model::{checked, published};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use silico_core::cache::Cache;
use silico_core::command::elements::{
    ElementType, Facing, MeasureName, Order, Query, entries, list, query,
};
use silico_core::command::measure::mass_properties;
use silico_core::command::wire::reads::CheckRequest;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{ModelChoice, Response, Revision, Running, Sessions};
use silico_core::document::json::read;
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

/// `running`'s response, failing if it doesn't answer in a minute.
fn answer<T>(running: Running<T>) -> Response<T> {
    match running.wait_timeout(Duration::from_secs(60)) {
        Ok(response) => response,
        Err(_) => panic!("the command didn't answer in a minute"),
    }
}

#[test]
fn the_designs_check_of_a_fillet_whose_edge_an_edit_removed_is_its_example_byte_for_byte() {
    // The example part, its pin made 6 long from z = -1 so the hole stops at z = 5, short of
    // the top (Face6, z = 10), and a fillet of the rim the top had. The rim at the bottom
    // (Face5, z = 0) is still there, and shares the pin's side: the one candidate. The
    // message shows the name cut short, as every error shows a document's string.
    let pin = r#""position": [20, 10, -1], "radius": 2.5, "height": 12"#;
    let hole = r#""note": "Clearance for an M5 bolt"}"#;
    assert!(example::EXAMPLE.contains(pin) && example::EXAMPLE.contains(hole));
    let text = example::EXAMPLE
        .replace(pin, r#""position": [20, 10, -1], "radius": 2.5, "height": 6"#)
        .replace(
            hole,
            &format!(
                r#"{hole},
    {{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "m3tq8v0z", "edges": ["m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)"], "radius": 1}}"#
            ),
        );
    // The same fillet with the pin at its design height (12, through the block): the name is
    // a real edge of the unedited part, and nothing is wrong.
    let unedited = text.replace(r#""height": 6"#, r#""height": 12"#);
    assert_ne!(unedited, text);
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let pool = KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap();
    let sessions =
        Sessions::with_cache_opener(pool, move |cache| Cache::with_secret(cache, &secret));
    let request = CheckRequest::read(&read(example("check/request").as_bytes()).unwrap()).unwrap();
    assert_eq!(request.model.choice(), ModelChoice::Current);
    // `document` saved as `file` and checked, in the next session (s1, then s2).
    let check = |file: &str, document: &str| {
        let path = dir.path().join(file);
        std::fs::write(&path, document).unwrap();
        let session = answer(sessions.open(&path, false)).session.unwrap().id;
        let checked = answer(sessions.check(&session, request.model.choice()));
        // No kernel fault: any problem is the document's.
        assert_eq!(checked.model.as_ref().unwrap().fault, None);
        (session, written(&checked.result.unwrap()))
    };
    let (session, broken) = check("bracket.silico", &text);
    assert_eq!(session, request.session);
    assert_eq!(broken, example("check/reference-broken"));
    let (_, whole) = check("whole.silico", &unedited);
    assert_eq!(whole, example("check/result"));
}
