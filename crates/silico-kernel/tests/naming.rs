//! M2.7: element names, built from the kernel's real histories, checked against FreeCAD's:
//! the ported `TopoShapeExpansion` tests (`tests/fixtures/naming/`, FreeCAD's expected
//! names copied verbatim), compared in FreeCAD's form without its tags, which are session
//! object IDs (FreeCAD's own matchers strip them too; this comparison keeps the sources in
//! parentheses and the `K` positions, which FreeCAD's matchers drop).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use silico_core::document::FeatureId;
use silico_core::kernel::{ElementKind, ElementRef};
use silico_core::naming::{ElementMap, Name, OpCode, TypeWord, freecad};
use silico_kernel::{
    BooleanOp, Built, ChamferSize, Primitive, Shape, boolean, chamfer, fillet, make, translate,
};

/// A box's faces, edges, and vertices, as OCCT makes it.
const BOX: silico_core::kernel::ElementCounts = silico_core::kernel::ElementCounts {
    faces: 6,
    edges: 12,
    vertices: 8,
    solids: 1,
};

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn unit_box() -> Shape {
    make(&Primitive::Box {
        dx: 1.0,
        dy: 1.0,
        dz: 1.0,
    })
    .unwrap()
}

/// FreeCAD's `CreateTwoCubes` with the tests' move: a unit cube at the origin, and one at
/// (0.5, -0.5, 0), overlapping it in a 0.5 × 0.5 × 1 block.
fn two_cubes() -> (Shape, Shape) {
    let second = translate(&unit_box(), [0.5, -0.5, 0.0]).unwrap().shape;
    (unit_box(), second)
}

/// FreeCAD's expected names in `fixtures/naming/<case>.txt`, without their tags.
fn expected(case: &str) -> Vec<String> {
    let path = format!(
        "{}/tests/fixtures/naming/{case}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut names: Vec<String> = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(untagged)
        .collect();
    names.sort();
    names
}

/// A FreeCAD name without its tag segments (`;:H<hex>[:<hex>],<type>`).
fn untagged(name: &str) -> String {
    let mut out = String::new();
    let mut rest = name;
    while let Some(at) = rest.find(";:H") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 3..];
        // The tag (hex, maybe negative), the length (`:hex`), then `,F`, `,E`, or `,V`.
        let comma = after.find(',').unwrap();
        rest = &after[comma + 2..];
    }
    out.push_str(rest);
    out
}

/// Every name in `map`, in FreeCAD's form, sorted.
fn written(map: &ElementMap) -> Vec<String> {
    let mut names: Vec<String> = map
        .iter()
        .flat_map(|(_, names)| names.iter().map(|n| freecad::text(n)))
        .collect();
    names.sort();
    names
}

/// The map of `op` on two primitives' solids `base` (made by `aaaaaaaa`) and `tool`
/// (`bbbbbbbb`), by the feature `rrrrrrrr`.
fn named(operation: OpCode, built: &Built) -> ElementMap {
    built.history.validate(2).unwrap();
    let base = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, BOX);
    let tool = ElementMap::by_number(id("bbbbbbbb"), OpCode::Fuse, BOX);
    ElementMap::operation(id("rrrrrrrr"), operation, &[&base, &tool], &built.history)
}

/// Every element of the result has exactly one name, and the names are all different.
fn one_name_each(map: &ElementMap, built: &Built) {
    let counts = built.history.result;
    let elements = map.iter().count() as u32;
    assert_eq!(elements, counts.faces + counts.edges + counts.vertices);
    assert!(map.iter().all(|(_, names)| names.len() == 1));
    for (element, names) in map.iter() {
        assert_eq!(map.element(&names[0]), Some(element));
    }
}

#[test]
fn a_cuts_names_are_freecads() {
    // makeElementBooleanCut: 38 names, one for each of the result's elements.
    let (a, b) = two_cubes();
    let cut = boolean(BooleanOp::Cut, &a, &b).unwrap();
    let map = named(OpCode::Cut, &cut);
    one_name_each(&map, &cut);
    assert_eq!(written(&map), expected("cut"));
}

#[test]
fn a_fuses_names_are_freecads() {
    // makeElementBooleanFuse: 66 names, both the overlap's squares named from both cubes'
    // faces (`Face5;:M2(Face5;K2);FUS`).
    let (a, b) = two_cubes();
    let fuse = boolean(BooleanOp::Fuse, &a, &b).unwrap();
    let map = named(OpCode::Fuse, &fuse);
    one_name_each(&map, &fuse);
    assert_eq!(written(&map), expected("fuse"));
}

/// A source's place in its group and its raw OCCT position: (rank, size, raw).
type Place = (u32, u32, u32);

/// Each element whose name holds a position (`M1/2`), with the name and its sources'
/// places.
fn positioned(map: &ElementMap) -> Vec<(ElementRef, String, Vec<Place>)> {
    map.iter()
        .filter_map(|(element, names)| {
            let Name::Derived { first, more, .. } = &*names[0] else {
                return None;
            };
            let places: Vec<Place> = std::iter::once(first)
                .chain(more)
                .filter_map(|s| {
                    s.place.map(|p| {
                        (
                            p.rank.get(),
                            p.size.get(),
                            s.position.0.map_or(0, |p| p.get()),
                        )
                    })
                })
                .collect();
            (!places.is_empty()).then(|| (element, names[0].to_string(), places))
        })
        .collect()
}

#[test]
fn a_fuse_writes_positions_only_on_the_pieces_of_its_split_edges() {
    // Trevin's decision: a source's position is written only where names would collide.
    // In the two cubes' fuse those are the 8 edges the other cube's faces split in two
    // (each piece modified from its edge alike), and nothing else: 16 names, `M1/2` and
    // `M2/2` of each. Each rank is FreeCAD's raw position (`Edge9;:M;FUS`, `Edge9;:M2;FUS`
    // in the fixture), as two pieces are ranked by OCCT's list.
    let (a, b) = two_cubes();
    let fuse = boolean(BooleanOp::Fuse, &a, &b).unwrap();
    let map = named(OpCode::Fuse, &fuse);
    let held = positioned(&map);
    assert_eq!(held.len(), 16, "{held:#?}");
    let mut split: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for (element, name, places) in &held {
        assert_eq!(element.kind, ElementKind::Edge, "{name}");
        let [(rank, size, raw)] = places[..] else {
            panic!("{name}: {places:?}");
        };
        assert_eq!((size, rank), (2, raw), "{name}");
        let Name::Derived { first, .. } = &*map.name(*element).unwrap() else {
            panic!("{name}");
        };
        split.entry(first.name.to_string()).or_default().push(rank);
    }
    let split: Vec<(String, Vec<u32>)> = split
        .into_iter()
        .map(|(edge, mut ranks)| {
            ranks.sort();
            (edge, ranks)
        })
        .collect();
    let expected: Vec<(String, Vec<u32>)> = [
        "aaaaaaaa.fuse.Edge10",
        "aaaaaaaa.fuse.Edge6",
        "aaaaaaaa.fuse.Edge8",
        "aaaaaaaa.fuse.Edge9",
        "bbbbbbbb.fuse.Edge11",
        "bbbbbbbb.fuse.Edge12",
        "bbbbbbbb.fuse.Edge2",
        "bbbbbbbb.fuse.Edge4",
    ]
    .iter()
    .map(|e| (e.to_string(), vec![1, 2]))
    .collect();
    assert_eq!(split, expected);
    // The cut, the common, and the dressings of the fixtures have no names that collide.
    for (op, code) in [
        (BooleanOp::Cut, OpCode::Cut),
        (BooleanOp::Common, OpCode::Common),
    ] {
        let built = boolean(op, &a, &b).unwrap();
        assert!(positioned(&named(code, &built)).is_empty(), "{op:?}");
    }
    for (chamfered, code) in [(false, OpCode::Fillet), (true, OpCode::Chamfer)] {
        let map = named_dressing(code, &dressed(chamfered));
        assert!(positioned(&map).is_empty(), "{code:?}");
    }
}

#[test]
fn a_commons_first_face_is_freecads() {
    // makeElementBooleanCommon: 26 names (a box), and Face1 is cube 1's Face3 (y = 0) cut
    // down, `Face3;:M;CMN`, the one name FreeCAD's test pins to its element.
    let (a, b) = two_cubes();
    let common = boolean(BooleanOp::Common, &a, &b).unwrap();
    let map = named(OpCode::Common, &common);
    one_name_each(&map, &common);
    assert_eq!(map.iter().count(), 26);
    let face1 = map
        .name(ElementRef {
            kind: ElementKind::Face,
            index: 1,
        })
        .unwrap();
    assert_eq!(freecad::text(&face1), "Face3;:M;CMN");
    // In Silico's form: what the common keeps of cube 1's Face3 lies inside cube 2, a piece,
    // the common's own (N8), so a common made a fuse takes no reference to it.
    assert_eq!(
        face1.to_string(),
        "rrrrrrrr.common.Face(I aaaaaaaa.fuse.Face3)"
    );
}

#[test]
fn a_moves_names_are_its_inputs() {
    // makeElementTransformWithMap: a fuse moved keeps all 66 names; a move is no operation
    // naming knows (Silico's recompute keeps a solid's map through its move).
    let (a, b) = two_cubes();
    let fuse = boolean(BooleanOp::Fuse, &a, &b).unwrap();
    let moved = translate(&fuse.shape, [-0.5, -0.5, 0.0]).unwrap();
    // Every element is reported as itself, moved: the same number, so the same name.
    for (n, fate) in moved.history.inputs[0].faces.iter().enumerate() {
        let to: Vec<u32> = fate.modified.iter().map(|r| r.element.index).collect();
        assert_eq!(to, [n as u32 + 1]);
    }
    assert_eq!(moved.history.result, fuse.history.result);
}

/// A unit cube with every edge filleted (radius 0.05) or chamfered (distance 0.05).
fn dressed(chamfered: bool) -> Built {
    let cube = unit_box();
    let edges: Vec<u32> = (1..=12).collect();
    if chamfered {
        chamfer(&cube, &edges, ChamferSize::Equal { distance: 0.05 }, false).unwrap()
    } else {
        fillet(&cube, &edges, 0.05).unwrap()
    }
}

fn named_dressing(operation: OpCode, built: &Built) -> ElementMap {
    built.history.validate(1).unwrap();
    let cube = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, BOX);
    ElementMap::operation(id("rrrrrrrr"), operation, &[&cube], &built.history)
}

#[test]
fn a_chamfers_names_are_freecads() {
    // makeElementChamfer, every edge of a unit cube: 98 names, faces modified, chamfer faces
    // from the edges, corner faces from the vertices, and every edge and vertex named from
    // a face above it.
    let built = dressed(true);
    let map = named_dressing(OpCode::Chamfer, &built);
    one_name_each(&map, &built);
    assert_eq!(written(&map), expected("chamfer"));
}

#[test]
fn a_fillets_names_are_freecads() {
    // makeElementFillet, every edge of a unit cube: FreeCAD's test checks its 106 names are
    // among the map's; here they're all of it.
    let built = dressed(false);
    let map = named_dressing(OpCode::Fillet, &built);
    one_name_each(&map, &built);
    assert_eq!(written(&map), expected("fillet"));
}

#[test]
fn names_say_how_each_element_came_about_in_silicos_form() {
    // The cut's rim vertex where cube 1's Edge10 meets cube 2's Edge2: FreeCAD's
    // `Edge10;:G(Edge2;K-1);CUT`, Silico's `rrrrrrrr.cut.Vertex(G aaaaaaaa.fuse.Edge10, G
    // bbbbbbbb.fuse.Edge2)` (the cubes' maps are made up by number, as a fuse's).
    let (a, b) = two_cubes();
    let cut = boolean(BooleanOp::Cut, &a, &b).unwrap();
    let map = named(OpCode::Cut, &cut);
    let by_text: BTreeMap<String, Name> = map
        .iter()
        .map(|(_, names)| (freecad::text(&names[0]), (*names[0]).clone()))
        .collect();
    let vertex = &by_text["Edge10;:G(Edge2;K-1);CUT"];
    assert_eq!(
        vertex.to_string(),
        "rrrrrrrr.cut.Vertex(G aaaaaaaa.fuse.Edge10, G bbbbbbbb.fuse.Edge2)"
    );
    // A kept element keeps its name exactly (N2).
    assert_eq!(by_text["Edge1"].to_string(), "aaaaaaaa.fuse.Edge1");
}

#[test]
fn names_dont_depend_on_size() {
    // TestTopologicalNamingProblem's testPartDesignElementMapChamfer and ...Fillet: a 10 mm
    // box, every edge chamfered or filleted by 1. FreeCAD counts 98 names (26 faces, 48
    // edges, 24 vertices) and 106 (the fillet's corners add 8 degenerate edges); the names
    // are the unit cube's (makeElementChamfer, makeElementFillet).
    let cube = make(&Primitive::Box {
        dx: 10.0,
        dy: 10.0,
        dz: 10.0,
    })
    .unwrap();
    let edges: Vec<u32> = (1..=12).collect();
    let chamfered = chamfer(&cube, &edges, ChamferSize::Equal { distance: 1.0 }, false).unwrap();
    let filleted = fillet(&cube, &edges, 1.0).unwrap();
    for (built, operation, case, counts) in [
        (&chamfered, OpCode::Chamfer, "chamfer", (26, 48, 24)),
        (&filleted, OpCode::Fillet, "fillet", (26, 56, 24)),
    ] {
        let r = built.history.result;
        assert_eq!((r.faces, r.edges, r.vertices), counts, "{case}");
        let map = named_dressing(operation, built);
        one_name_each(&map, built);
        assert_eq!(written(&map), expected(case), "{case}");
    }
}

#[test]
fn coincident_solids_fused_are_named_from_both_the_base_first() {
    // testBodySubShapeBinderElementMap: two 10 mm boxes in the same place, fused: 26 names,
    // each element modified from both boxes' (FreeCAD: `Face1;:M(Face1;:H<tool>,F);FUS`),
    // the base's first.
    let block = || {
        make(&Primitive::Box {
            dx: 10.0,
            dy: 10.0,
            dz: 10.0,
        })
        .unwrap()
    };
    let fuse = boolean(BooleanOp::Fuse, &block(), &block()).unwrap();
    let map = named(OpCode::Fuse, &fuse);
    one_name_each(&map, &fuse);
    assert_eq!(map.iter().count(), 26);
    for (element, names) in map.iter() {
        let Name::Derived { first, more, .. } = &*names[0] else {
            panic!("{element}: {}", names[0]);
        };
        let from: Vec<String> = std::iter::once(first)
            .chain(more)
            .map(|s| match &*s.name {
                Name::Element {
                    feature, element, ..
                } => format!("{feature}.{:?}", element.kind),
                other => panic!("{other}"),
            })
            .collect();
        let kind = format!("{:?}", element.kind);
        assert_eq!(
            from,
            [format!("aaaaaaaa.{kind}"), format!("bbbbbbbb.{kind}")],
            "{element}: {}",
            names[0]
        );
        // Named from the same element of each box.
        let text = freecad::text(&names[0]);
        let source = text.split(';').next().unwrap();
        assert_eq!(text, format!("{source};:M({source});FUS"), "{element}");
    }
}

#[test]
fn a_primitives_elements_are_named_by_their_numbers() {
    // testPartDesignElementMapBox, ...Cylinder, ...Sphere, ...Cone, ...Torus: 26, 8, 6, 8,
    // and 4 names, one per element, seams and the sphere's degenerate pole edges too.
    let feature = id("aaaaaaaa");
    for (primitive, count) in [
        (
            Primitive::Box {
                dx: 10.0,
                dy: 10.0,
                dz: 10.0,
            },
            26,
        ),
        (
            Primitive::Cylinder {
                radius: 10.0,
                height: 10.0,
            },
            8,
        ),
        (Primitive::Sphere { radius: 5.0 }, 6),
        (
            Primitive::Cone {
                radius1: 2.0,
                radius2: 4.0,
                height: 10.0,
            },
            8,
        ),
        (
            Primitive::Torus {
                major_radius: 10.0,
                minor_radius: 2.0,
            },
            4,
        ),
    ] {
        let moved = translate(&make(&primitive).unwrap(), [1.0, 0.0, 0.0]).unwrap();
        let r = moved.history.result;
        assert_eq!(r.faces + r.edges + r.vertices, count, "{primitive:?}");
        // The core's counts for the primitive are OCCT's.
        let counts = silico_core::kernel::primitive_counts(&primitive);
        assert_eq!(counts, r, "{primitive:?}");
        let map = ElementMap::by_number(feature.clone(), OpCode::Fuse, counts);
        assert_eq!(map.iter().count(), count as usize, "{primitive:?}");
        let mut names = std::collections::BTreeSet::new();
        for (kind, n) in [
            (ElementKind::Face, r.faces),
            (ElementKind::Edge, r.edges),
            (ElementKind::Vertex, r.vertices),
        ] {
            for index in 1..=n {
                let element = ElementRef { kind, index };
                let name = map.name(element).unwrap();
                assert_eq!(map.element(&name), Some(element));
                assert_eq!(name.to_string(), format!("aaaaaaaa.fuse.{element}"));
                names.insert(name);
            }
        }
        assert_eq!(names.len(), count as usize);
        // Nothing past its elements.
        let past = ElementRef {
            kind: ElementKind::Face,
            index: r.faces + 1,
        };
        assert!(map.name(past).is_none());
    }
    // Another feature's element isn't a map's.
    let map = ElementMap::by_number(feature, OpCode::Fuse, BOX);
    let other = Name::Element {
        feature: id("bbbbbbbb"),
        made: TypeWord::Operation(OpCode::Fuse),
        element: ElementRef {
            kind: ElementKind::Face,
            index: 1,
        },
    };
    assert_eq!(map.element(&other), None);
}

#[test]
fn a_recomputed_holes_names_say_how_each_element_came_about() {
    // The design's example on OCCT: a 40 × 20 × 10 block, a pin (r 2.5) through it, cut.
    use silico_core::document::{Checked, read};
    use silico_core::pool::KernelPool;
    use silico_core::recompute::{Recompute, RunOutcome};
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [
        {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10},
        {"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 12},
        {"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"}
    ]}"#;
    let pool = KernelPool::start_workers(
        std::path::Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")),
        2,
    )
    .unwrap();
    let engine = Recompute::new(pool);
    let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    let Ok(RunOutcome::Published(model)) = engine.start(document).wait() else {
        panic!("not published");
    };
    let hole = model.element_map(&id("m3tq8v0z")).unwrap();
    let all: Vec<String> = hole.iter().map(|(_, names)| names[0].to_string()).collect();
    // The hole's wall: the pin's side face, modified by the cut (docs/ports/history.md),
    // kept inside the block: a piece, the cut's own (N8), FreeCAD's `Face1;:M;CUT`.
    let wall = "m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)";
    assert!(all.contains(&wall.to_owned()), "{all:#?}");
    assert!(
        !all.contains(&"a91cw0de.cylinder.Face1".to_owned()),
        "{all:#?}"
    );
    // Its two rims: edges the cut generated from the block's top (Face6) or bottom (Face5)
    // and the pin's side, as FreeCAD names a cylinder's hole (`Face6;:G2(Face1;K-2);CUT`).
    let rims: Vec<String> = hole
        .iter()
        .filter(|(e, _)| e.kind == ElementKind::Edge)
        .map(|(_, names)| freecad::text(&names[0]))
        .filter(|t| t.contains("(Face1;K-") && t.ends_with(";CUT"))
        .collect();
    let mut tops: Vec<&str> = rims.iter().map(|t| &t[..5]).collect();
    tops.sort_unstable();
    assert_eq!(tops, ["Face5", "Face6"], "{rims:?}");
    // The block's untouched faces keep their names: its four sides.
    for face in 1..=4 {
        assert!(
            all.contains(&format!("k7f2qx3m.box.Face{face}")),
            "{all:#?}"
        );
    }
}

#[test]
fn topo_shape_tests_maps_have_freecads_sizes() {
    // TopoShapeTest.py (src/Mod/Part/parttests): a 1 × 2 × 2 box and a 2 × 1 × 2 box at the
    // origin. Fused, 58 names (testPartFuse, testTopoShapeFuse); common and cut, 26 each
    // (testPartCommon, testPartCut, testTopoShapeCommon, testTopoShapeCut); the first box's
    // Face1 edges filleted or chamfered by 0.1, 42 each (testTopoShapeMakeFillet,
    // ...MakeChamfer). FreeCAD's counts, one name each.
    let one = || {
        make(&Primitive::Box {
            dx: 1.0,
            dy: 2.0,
            dz: 2.0,
        })
        .unwrap()
    };
    let two = make(&Primitive::Box {
        dx: 2.0,
        dy: 1.0,
        dz: 2.0,
    })
    .unwrap();
    for (op, operation, size) in [
        (BooleanOp::Fuse, OpCode::Fuse, 58),
        (BooleanOp::Common, OpCode::Common, 26),
        (BooleanOp::Cut, OpCode::Cut, 26),
    ] {
        let built = boolean(op, &one(), &two).unwrap();
        let map = named(operation, &built);
        one_name_each(&map, &built);
        assert_eq!(map.iter().count(), size, "{op:?}");
    }
    // Face1's edges: OCCT's box's Face1 (x = 0) is bounded by Edges 1 to 4.
    let edges = [1, 2, 3, 4];
    let filleted = fillet(&one(), &edges, 0.1).unwrap();
    let chamfered = chamfer(&one(), &edges, ChamferSize::Equal { distance: 0.1 }, false).unwrap();
    for (built, operation) in [(&filleted, OpCode::Fillet), (&chamfered, OpCode::Chamfer)] {
        let map = named_dressing(operation, built);
        one_name_each(&map, built);
        assert_eq!(map.iter().count(), 42, "{operation:?}");
    }
}

/// A plate (400 × 20 × 5, `pppppppp`) with a hole cut through it at each of `xs`, one cut
/// feature per hole, in that order: pin `y<k>` (r 1, through) at (x, 10, -1), cut `c<k>`,
/// each cut's base the cut before it.
fn plate_with_holes(xs: &[(u32, f64)]) -> silico_core::document::Checked {
    let mut features = vec![r#"{"id": "pppppppp", "type": "box", "v": 1, "name": "Plate", "position": [0, 0, 0], "length": 400, "width": 20, "height": 5}"#.to_owned()];
    let mut base = "pppppppp".to_owned();
    for &(k, x) in xs {
        features.push(format!(r#"{{"id": "y{k:07}", "type": "cylinder", "v": 1, "name": "Pin", "position": [{x}, 10, -1], "radius": 1, "height": 7}}"#));
        let cut = format!("c{k:07}");
        features.push(format!(r#"{{"id": "{cut}", "type": "cut", "v": 1, "name": "Hole", "base": "{base}", "tool": "y{k:07}"}}"#));
        base = cut;
    }
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
        features.join(", ")
    );
    silico_core::document::Checked::new(silico_core::document::read(text.as_bytes()).unwrap())
        .unwrap()
}

fn occt_engine() -> silico_core::recompute::Recompute<silico_core::pool::KernelPool> {
    let pool = silico_core::pool::KernelPool::start_workers(
        std::path::Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")),
        2,
    )
    .unwrap();
    silico_core::recompute::Recompute::new(pool)
}

fn published_on(
    engine: &silico_core::recompute::Recompute<silico_core::pool::KernelPool>,
    document: silico_core::document::Checked,
) -> std::sync::Arc<silico_core::recompute::Model> {
    match engine.start(document).wait().unwrap() {
        silico_core::recompute::RunOutcome::Published(model) => model,
        other => panic!("{other:?}"),
    }
}

/// The names of the edges of `cut`'s solid made from pin `pin`'s side: the hole's rims.
fn rims(model: &silico_core::recompute::Model, cut: &str, pin: &str) -> Vec<String> {
    let map = model.element_map(&id(cut)).unwrap();
    let side = format!("{pin}.cylinder.Face1");
    let mut rims: Vec<String> = map
        .iter()
        .filter(|(e, _)| e.kind == ElementKind::Edge)
        .map(|(_, names)| names[0].to_string())
        .filter(|name| name.contains(&side))
        .collect();
    rims.sort();
    rims
}

#[test]
fn a_later_holes_rims_keep_their_names_whatever_holes_come_before() {
    // Trevin's decision: the plate's top and bottom faces, modified by each cut as its sole
    // result, keep their own names (`pppppppp.box.Face6`, `pppppppp.box.Face5`) however many cuts
    // came before, and a source's position in OCCT's list is written only where names
    // would otherwise collide, so the last hole's rims, named from them and the pin, are the
    // same with no earlier hole, or with one added, removed, or moved, and the earlier holes
    // reordered.
    let engine = occt_engine();
    let last = (9, 300.0);
    let rims_of = |earlier: &[(u32, f64)]| {
        let mut holes = earlier.to_vec();
        holes.push(last);
        let model = published_on(&engine, plate_with_holes(&holes));
        rims(&model, "c0000009", "y0000009")
    };
    let expected = [
        "c0000009.cut.Edge(G pppppppp.box.Face5, G y0000009.cylinder.Face1)",
        "c0000009.cut.Edge(G pppppppp.box.Face6, G y0000009.cylinder.Face1)",
    ];
    for earlier in [
        vec![],
        vec![(1, 50.0)],
        vec![(1, 50.0), (2, 100.0), (3, 150.0)],
        vec![(1, 50.0), (3, 150.0)],
        vec![(1, 50.0), (2, 100.0), (4, 200.0), (3, 150.0)],
        vec![(1, 50.0), (2, 120.0), (3, 150.0)],
        vec![(3, 150.0), (1, 50.0), (2, 100.0)],
    ] {
        assert_eq!(rims_of(&earlier), expected, "{earlier:?}");
    }
}

#[test]
fn a_face_split_by_a_cut_keeps_its_pieces_apart() {
    // A slot (2 wide, 1 deep) across the plate's top splits the top face in two: each piece
    // keeps its level, ranked by OCCT's order among the pieces (`s0000001.cut.Face(M1/2
    // pppppppp.box.Face6)`, `...(M2/2 ...)`), so neither is taken for the face as it was
    // (`pppppppp.box.Face6`, the name of a face only modified, as its sole result): a reference
    // to the top face finds neither piece, and fails, rather than following one. Every
    // name is unique.
    let engine = occt_engine();
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [
        {"id": "pppppppp", "type": "box", "v": 1, "name": "Plate", "position": [0, 0, 0], "length": 40, "width": 20, "height": 5},
        {"id": "t0000001", "type": "box", "v": 1, "name": "Slot", "position": [19, -1, 4], "length": 2, "width": 22, "height": 2},
        {"id": "s0000001", "type": "cut", "v": 1, "name": "Slotted", "base": "pppppppp", "tool": "t0000001"}
    ]}"#;
    let document =
        silico_core::document::Checked::new(silico_core::document::read(text.as_bytes()).unwrap())
            .unwrap();
    let model = published_on(&engine, document);
    let map = model.element_map(&id("s0000001")).unwrap();
    let faces: Vec<String> = map
        .iter()
        .filter(|(e, _)| e.kind == ElementKind::Face)
        .map(|(_, names)| names[0].to_string())
        .collect();
    let pieces: Vec<&String> = faces
        .iter()
        .filter(|f| f.contains("pppppppp.box.Face6"))
        .collect();
    assert_eq!(pieces.len(), 2, "{faces:#?}");
    assert!(
        pieces.contains(&&"s0000001.cut.Face(M1/2 pppppppp.box.Face6)".to_owned()),
        "{faces:#?}"
    );
    assert!(
        !faces.contains(&"pppppppp.box.Face6".to_owned()),
        "{faces:#?}"
    );
    assert!(
        pieces.contains(&&"s0000001.cut.Face(M2/2 pppppppp.box.Face6)".to_owned()),
        "{faces:#?}"
    );
    // One name for each element, and none twice.
    let all: Vec<String> = map.iter().map(|(_, names)| names[0].to_string()).collect();
    let distinct: std::collections::BTreeSet<&String> = all.iter().collect();
    assert_eq!(distinct.len(), all.len());
}

#[test]
fn a_plate_with_a_hundred_and_fifty_holes_has_short_names() {
    // Before the collapse its names nested a level per hole (2.6 KB at 150 holes, 805 KB
    // for one map): now no name is longer than one hole's own.
    let engine = occt_engine();
    let holes: Vec<(u32, f64)> = (1..=150).map(|k| (k, 2.0 * f64::from(k) + 50.0)).collect();
    let model = published_on(&engine, plate_with_holes(&holes));
    let map = model.element_map(&id("c0000150")).unwrap();
    let longest = map
        .iter()
        .map(|(_, names)| names[0].to_string().len())
        .max()
        .unwrap();
    assert!(longest < 120, "longest name {longest}");
    let total: usize = map
        .iter()
        .map(|(_, names)| names[0].to_string().len())
        .sum();
    let elements = map.iter().count();
    assert!(total < 80 * elements, "{total} for {elements} elements");
    let top = map
        .iter()
        .find(|(e, _)| e.kind == ElementKind::Face && e.index == 1)
        .map(|(_, names)| names[0].to_string());
    assert!(top.is_some());
}
