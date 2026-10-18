//! M2.7, Trevin's decision on names that would collide (docs/ports/naming.md): a source's
//! position in OCCT's list is written only where two elements' names would otherwise be
//! the same, as the element's rank among them and how many there are (`G1/2`, `M2/3`), and
//! a face only modified, as its operation's sole result of it, keeps its own name. Built
//! on the real kernel. Expected names are worked out by hand from the geometry: which
//! input faces and edges meet where, and how many pieces a cut leaves.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use silico_core::document::{Checked, FeatureId, read};
use silico_core::kernel::{ElementKind, ElementRef};
use silico_core::naming::{ElementMap, freecad};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

const HEAD: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": ["#;

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn boxed(id: &str, at: [f64; 3], size: [f64; 3]) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "{id}", "position": [{}, {}, {}], "length": {}, "width": {}, "height": {}}}"#,
        at[0], at[1], at[2], size[0], size[1], size[2]
    )
}

fn cylinder(id: &str, at: [f64; 3], radius: f64, height: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "cylinder", "v": 1, "name": "{id}", "position": [{}, {}, {}], "radius": {radius}, "height": {height}}}"#,
        at[0], at[1], at[2]
    )
}

fn sphere(id: &str, at: [f64; 3], radius: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "sphere", "v": 1, "name": "{id}", "position": [{}, {}, {}], "radius": {radius}}}"#,
        at[0], at[1], at[2]
    )
}

fn torus(id: &str, at: [f64; 3], major: f64, minor: f64) -> String {
    format!(
        r#"{{"id": "{id}", "type": "torus", "v": 1, "name": "{id}", "position": [{}, {}, {}], "radius1": {major}, "radius2": {minor}}}"#,
        at[0], at[1], at[2]
    )
}

fn op(id: &str, kind: &str, base: &str, tool: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "{kind}", "v": 1, "name": "{id}", "base": "{base}", "tool": "{tool}"}}"#
    )
}

fn document(features: &[String]) -> Checked {
    let text = format!("{HEAD}{}]}}", features.join(",\n"));
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

fn engine() -> Recompute<KernelPool> {
    Recompute::new(
        KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2)
            .unwrap(),
    )
}

fn publish(engine: &Recompute<KernelPool>, features: &[String]) -> Arc<Model> {
    match engine.start(document(features)).wait().unwrap() {
        RunOutcome::Published(model) => model,
        other => panic!("{other:?}"),
    }
}

/// The primary names of the map's elements of `kind`, sorted.
fn names_of(map: &ElementMap, kind: ElementKind) -> Vec<String> {
    let mut names: Vec<String> = map
        .iter()
        .filter(|(e, _)| e.kind == kind)
        .map(|(_, names)| names[0].to_string())
        .collect();
    names.sort();
    names
}

/// Every element has one name, no two alike, none unstable, and each finds its element:
/// every name in the map is one a reference can hold.
fn all_referenceable(map: &ElementMap) {
    let mut seen = BTreeSet::new();
    for (element, names) in map.iter() {
        assert_eq!(names.len(), 1, "{element:?}");
        let name = &names[0];
        assert!(seen.insert(name.to_string()), "{name} twice");
        assert!(!map.is_unstable(name), "{name} unstable");
        assert_eq!(map.element(name), Some(element), "{name}");
    }
}

/// The plate (40 × 20 × 5, `pppppppp`) every case below starts from.
fn plate() -> String {
    boxed("pppppppp", [0.0, 0.0, 0.0], [40.0, 20.0, 5.0])
}

/// A through hole (r 1) at (x, 10) in `base`, as pin `y<k>` and cut `c<k>`.
fn hole(k: u32, x: f64, base: &str) -> [String; 2] {
    [
        cylinder(&format!("y{k:07}"), [x, 10.0, -1.0], 1.0, 7.0),
        op(&format!("c{k:07}"), "cut", base, &format!("y{k:07}")),
    ]
}

#[test]
fn a_balls_through_hole_has_two_rims_and_two_seam_points_told_apart_by_rank() {
    // A cylinder (r 2) through a ball (r 10) along its axis: the pin's side meets the
    // ball's one face in two circles, and the pin's seam meets the ball's seam at two
    // points, each pair coming about alike. Only their order in OCCT's lists tells them
    // apart, so only they hold it, as ranks among two.
    let model = publish(
        &engine(),
        &[
            sphere("bbbbbbbb", [0.0, 0.0, 0.0], 10.0),
            cylinder("pppppppp", [0.0, 0.0, -15.0], 2.0, 30.0),
            op("cccccccc", "cut", "bbbbbbbb", "pppppppp"),
        ],
    );
    let map = model.element_map(&id("cccccccc")).unwrap();
    all_referenceable(map);
    assert_eq!(
        names_of(map, ElementKind::Edge),
        [
            "bbbbbbbb.sphere.Edge2",
            "cccccccc.Edge(G1/2 bbbbbbbb.sphere.Face1, G1/2 pppppppp.cylinder.Face1)",
            "cccccccc.Edge(G2/2 bbbbbbbb.sphere.Face1, G2/2 pppppppp.cylinder.Face1)",
            "pppppppp.cylinder.Edge1",
        ]
    );
    assert_eq!(
        names_of(map, ElementKind::Vertex),
        [
            "cccccccc.Vertex(G1/2 bbbbbbbb.sphere.Edge2, G1/2 pppppppp.cylinder.Edge1)",
            "cccccccc.Vertex(G2/2 bbbbbbbb.sphere.Edge2, G2/2 pppppppp.cylinder.Edge1)",
        ]
    );
    // FreeCAD's form still writes OCCT's positions, which the ranks are.
    let mut texts: Vec<(String, String)> = map
        .iter()
        .filter(|(_, names)| names[0].to_string().contains("/2 bbbbbbbb.sphere.Face1"))
        .map(|(_, names)| (names[0].to_string(), freecad::text(&names[0])))
        .collect();
    texts.sort();
    assert_eq!(
        texts,
        [
            (
                "cccccccc.Edge(G1/2 bbbbbbbb.sphere.Face1, G1/2 pppppppp.cylinder.Face1)"
                    .to_owned(),
                "Face1;:G(Face1;K-1);CUT".to_owned()
            ),
            (
                "cccccccc.Edge(G2/2 bbbbbbbb.sphere.Face1, G2/2 pppppppp.cylinder.Face1)"
                    .to_owned(),
                "Face1;:G2(Face1;K-2);CUT".to_owned()
            ),
        ]
    );
    // The faces, each modified as its sole result, keep their own names.
    assert_eq!(
        names_of(map, ElementKind::Face),
        ["bbbbbbbb.sphere.Face1", "pppppppp.cylinder.Face1"]
    );
}

#[test]
fn a_pin_through_a_torus_tube_has_four_loops_told_apart_by_rank() {
    // A pin (r 1) through a torus's tube (R 10, r 3), upright: it meets the torus's face in
    // four loops (two on top, two below, either side of the tube's crest), and the torus's
    // seam (Edge2) is cut in three. Each set comes about alike, and is ranked among itself.
    let model = publish(
        &engine(),
        &[
            torus("tttttttt", [0.0, 0.0, 0.0], 10.0, 3.0),
            cylinder("pppppppp", [10.0, 0.0, -5.0], 1.0, 10.0),
            op("cccccccc", "cut", "tttttttt", "pppppppp"),
        ],
    );
    let map = model.element_map(&id("cccccccc")).unwrap();
    all_referenceable(map);
    let edges = names_of(map, ElementKind::Edge);
    for n in 1..=4 {
        let rim =
            format!("cccccccc.Edge(G{n}/4 tttttttt.torus.Face1, G{n}/4 pppppppp.cylinder.Face1)");
        assert!(edges.contains(&rim), "{rim}: {edges:#?}");
    }
    for n in 1..=3 {
        let piece = format!("cccccccc.Edge(M{n}/3 tttttttt.torus.Edge2)");
        assert!(edges.contains(&piece), "{piece}: {edges:#?}");
    }
    // The seam is gone as it was.
    assert!(
        !edges.contains(&"tttttttt.torus.Edge2".to_owned()),
        "{edges:#?}"
    );
}

#[test]
fn a_slot_splits_the_top_and_its_edges_into_ranked_pieces() {
    // A slot across the plate's top splits the top face, and the top's front and back
    // edges (Edge10 and Edge12, along x at the top), each in two. Neither piece is the
    // face or edge as it was: those names are gone from the slot's map.
    let model = publish(
        &engine(),
        &[
            plate(),
            boxed("t0000001", [19.0, -1.0, 4.0], [2.0, 22.0, 2.0]),
            op("s0000001", "cut", "pppppppp", "t0000001"),
        ],
    );
    let map = model.element_map(&id("s0000001")).unwrap();
    all_referenceable(map);
    let faces = names_of(map, ElementKind::Face);
    for piece in [
        "s0000001.Face(M1/2 pppppppp.box.Face6)",
        "s0000001.Face(M2/2 pppppppp.box.Face6)",
    ] {
        assert!(faces.contains(&piece.to_owned()), "{faces:#?}");
    }
    assert!(
        !faces.contains(&"pppppppp.box.Face6".to_owned()),
        "{faces:#?}"
    );
    let edges = names_of(map, ElementKind::Edge);
    for edge in ["pppppppp.box.Edge10", "pppppppp.box.Edge12"] {
        for n in [1, 2] {
            let piece = format!("s0000001.Edge(M{n}/2 {edge})");
            assert!(edges.contains(&piece), "{piece}: {edges:#?}");
        }
        assert!(!edges.contains(&edge.to_owned()), "{edge}: {edges:#?}");
    }
}

#[test]
fn a_piece_keeps_its_name_through_a_hole_before_the_split_and_one_after() {
    // A hole (the top face modified, one from one: its own name), then the slot (the top
    // split in two, ranked), then a hole in the second piece (that piece modified, one
    // from one: its name from the slot, exactly). The pieces' names are what the slot
    // gives them with no hole at all.
    let features = [
        vec![plate()],
        hole(1, 5.0, "pppppppp").to_vec(),
        vec![
            boxed("t0000001", [19.0, -1.0, 4.0], [2.0, 22.0, 2.0]),
            op("s0000001", "cut", "c0000001", "t0000001"),
        ],
        hole(2, 30.0, "s0000001").to_vec(),
    ]
    .concat();
    let model = publish(&engine(), &features);
    let after_first = model.element_map(&id("c0000001")).unwrap();
    assert!(names_of(after_first, ElementKind::Face).contains(&"pppppppp.box.Face6".to_owned()));
    let pieces = [
        "s0000001.Face(M1/2 pppppppp.box.Face6)",
        "s0000001.Face(M2/2 pppppppp.box.Face6)",
    ];
    let slotted = model.element_map(&id("s0000001")).unwrap();
    let last = model.element_map(&id("c0000002")).unwrap();
    all_referenceable(last);
    for piece in pieces {
        let in_slot = slotted
            .iter()
            .find(|(_, names)| names[0].to_string() == piece)
            .unwrap_or_else(|| panic!("{piece}"))
            .1[0]
            .clone();
        // The same name in the last hole's map (the piece with that hole, modified, and
        // the other, kept).
        assert!(last.element(&in_slot).is_some(), "{piece}");
    }
    // The second hole's rim on the top is named from the piece it's in.
    let edges = names_of(last, ElementKind::Edge);
    assert!(
        edges.contains(
            &"c0000002.Edge(G s0000001.Face(M2/2 pppppppp.box.Face6), G y0000002.cylinder.Face1)"
                .to_owned()
        ),
        "{edges:#?}"
    );
}

#[test]
fn a_groove_across_a_u_shaped_face_leaves_three_ranked_pieces() {
    // A notch from the plate's front leaves its top U-shaped (one face, its own name), and
    // a groove across both of the U's arms cuts it in three: the arms' two ends and the
    // rest. The U's name is gone; the three pieces are ranked among three.
    let model = publish(
        &engine(),
        &[
            plate(),
            boxed("nnnnnnnn", [15.0, -1.0, -1.0], [10.0, 12.0, 7.0]),
            op("vvvvvvvv", "cut", "pppppppp", "nnnnnnnn"),
            boxed("gggggggg", [-1.0, 3.0, 4.0], [42.0, 2.0, 2.0]),
            op("rrrrrrrr", "cut", "vvvvvvvv", "gggggggg"),
        ],
    );
    let notched = model.element_map(&id("vvvvvvvv")).unwrap();
    assert!(names_of(notched, ElementKind::Face).contains(&"pppppppp.box.Face6".to_owned()));
    let map = model.element_map(&id("rrrrrrrr")).unwrap();
    all_referenceable(map);
    let tops: Vec<String> = names_of(map, ElementKind::Face)
        .into_iter()
        .filter(|f| f.contains("pppppppp.box.Face6"))
        .collect();
    assert_eq!(
        tops,
        [
            "rrrrrrrr.Face(M1/3 pppppppp.box.Face6)",
            "rrrrrrrr.Face(M2/3 pppppppp.box.Face6)",
            "rrrrrrrr.Face(M3/3 pppppppp.box.Face6)",
        ]
    );
}

/// `feature`'s faces, each by name with its edges' names, and its edges, each with its
/// vertices': its elements and how they join, as a reference sees them, leaving out every
/// name that mentions one of `leave_out`.
fn joined(model: &Model, feature: &str, leave_out: &[&str]) -> BTreeSet<(String, Vec<String>)> {
    let map = model.element_map(&id(feature)).unwrap();
    let Some((_, FeatureOutcome::Built { built, .. })) =
        model.features().iter().find(|(f, _)| f == &id(feature))
    else {
        panic!("{feature} didn't build");
    };
    let topology = &built.history.as_ref().unwrap().topology;
    let keep = |name: &String| !leave_out.iter().any(|f| name.contains(f));
    let name = |kind, index| map.name(ElementRef { kind, index }).unwrap().to_string();
    let mut out = BTreeSet::new();
    for (kind, lower, lists) in [
        (ElementKind::Face, ElementKind::Edge, &topology.face_edges),
        (
            ElementKind::Edge,
            ElementKind::Vertex,
            &topology.edge_vertices,
        ),
    ] {
        for (i, list) in lists.iter().enumerate() {
            let own = name(kind, i as u32 + 1);
            if !keep(&own) {
                continue;
            }
            let mut below: Vec<String> =
                list.iter().map(|&j| name(lower, j)).filter(keep).collect();
            below.sort();
            out.insert((own, below));
        }
    }
    out
}

#[test]
fn a_hole_before_a_fuse_a_slot_or_a_notch_changes_none_of_its_other_names() {
    // The same operation on the plate as it is, and on the plate with a hole cut first,
    // well away: every name not about the hole is the same, byte for byte, and joins the
    // same names. Before Trevin's decision, the hole's cut renamed the top face (`*`), and
    // so everything named from it.
    let engine = engine();
    let operations: [(&str, Vec<String>); 3] = [
        (
            "fuse",
            vec![
                boxed("t0000001", [30.0, 5.0, 4.0], [4.0, 4.0, 3.0]),
                op("r0000001", "fuse", "BASE", "t0000001"),
            ],
        ),
        (
            "slot",
            vec![
                boxed("t0000001", [19.0, -1.0, 4.0], [2.0, 22.0, 2.0]),
                op("r0000001", "cut", "BASE", "t0000001"),
            ],
        ),
        (
            "notch",
            vec![
                boxed("t0000001", [36.0, -1.0, -1.0], [5.0, 5.0, 7.0]),
                op("r0000001", "cut", "BASE", "t0000001"),
            ],
        ),
    ];
    for (what, operation) in operations {
        let on = |base: &str| -> Vec<String> {
            operation.iter().map(|f| f.replace("BASE", base)).collect()
        };
        let mut plain = vec![plate()];
        plain.extend(on("pppppppp"));
        let mut drilled = vec![plate()];
        drilled.extend(hole(1, 5.0, "pppppppp"));
        drilled.extend(on("c0000001"));
        let leave_out = ["c0000001", "y0000001"];
        let before = joined(&publish(&engine, &plain), "r0000001", &leave_out);
        let after = joined(&publish(&engine, &drilled), "r0000001", &leave_out);
        assert!(before.len() > 20, "{what}: {before:#?}");
        assert_eq!(after, before, "{what}");
    }
}
