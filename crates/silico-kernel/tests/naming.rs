//! M2.7: element names, built from the kernel's real histories, checked against FreeCAD's:
//! the ported `TopoShapeExpansion` tests (`tests/fixtures/naming/`, FreeCAD's expected
//! names copied verbatim), compared in FreeCAD's form without its tags, which are session
//! object IDs (FreeCAD's own matchers strip them too; this comparison keeps the sources in
//! parentheses and the `K` positions, which FreeCAD's matchers drop).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use silico_core::document::FeatureId;
use silico_core::kernel::{ElementKind, ElementRef};
use silico_core::naming::{ElementMap, Name, OpCode, freecad};
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
    let base = ElementMap::primitive(id("aaaaaaaa"), BOX);
    let tool = ElementMap::primitive(id("bbbbbbbb"), BOX);
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
    // In Silico's form: the common modified cube 1's Face3.
    assert_eq!(face1.to_string(), "rrrrrrrr.Face(M aaaaaaaa.Face3)");
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
    let cube = ElementMap::primitive(id("aaaaaaaa"), BOX);
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
    // `Edge10;:G(Edge2;K-1);CUT`, Silico's `rrrrrrrr.Vertex(G aaaaaaaa.Edge10, G
    // bbbbbbbb.Edge2)`.
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
        "rrrrrrrr.Vertex(G aaaaaaaa.Edge10, G bbbbbbbb.Edge2)"
    );
    // A kept element keeps its name exactly (N2).
    assert_eq!(by_text["Edge1"].to_string(), "aaaaaaaa.Edge1");
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
                Name::Element { feature, element } => format!("{feature}.{:?}", element.kind),
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
        let map = ElementMap::primitive(feature.clone(), counts);
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
                assert_eq!(name.to_string(), format!("aaaaaaaa.{element}"));
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
    let map = ElementMap::primitive(feature, BOX);
    let other = Name::Element {
        feature: id("bbbbbbbb"),
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
    // The hole's wall: the pin's side face, modified by the cut (docs/ports/history.md).
    assert!(
        all.contains(&"m3tq8v0z.Face(M a91cw0de.Face1)".to_owned()),
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
        assert!(all.contains(&format!("k7f2qx3m.Face{face}")), "{all:#?}");
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
