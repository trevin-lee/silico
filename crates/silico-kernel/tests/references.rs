//! M2.8 on the real kernel: a fillet's edges, named in the document, followed through
//! upstream edits to the element they meant, or failing loudly (docs/ports/references.md).
//! Expected volumes are worked out by hand: a fillet of radius r on a hole's rim (radius R)
//! takes away the ring Pappus's theorem gives, 2π (R + c) (1 − π/4) r², where (1 − π/4) r²
//! is the area between the corner and the fillet's arc and c = r (10 − 3π) / (3 (4 − π)) is
//! how far that area's centroid is from the corner; a fillet of any other edge takes away
//! another amount.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f64::consts::PI;
use std::path::Path;
use std::sync::Arc;

use silico_core::document::{Checked, FeatureId, read};
use silico_core::naming::{Maker, Name, OpCode, PrimitiveKind};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};

const HEAD: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": ["#;

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn engine() -> Recompute<KernelPool> {
    Recompute::new(
        KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2)
            .unwrap(),
    )
}

fn publish(engine: &Recompute<KernelPool>, text: &str) -> Arc<Model> {
    let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    match engine.start(document).wait().unwrap() {
        RunOutcome::Published(model) => model,
        other => panic!("{other:?}"),
    }
}

/// A plate (40 × 20 × 5, `pppppppp`) with a through hole of radius `radius` at (x, 10)
/// (pin `y0000001`, cut `c0000001`), and a fillet `f0000001` of `edges` of the cut, `r`.
fn holed(x: f64, radius: f64, edges: &[&str], r: f64) -> String {
    let edges: Vec<String> = edges.iter().map(|e| format!("{e:?}")).collect();
    format!(
        r#"{HEAD}
{{"id": "pppppppp", "type": "box", "v": 1, "name": "Plate", "position": [0, 0, 0], "length": 40, "width": 20, "height": 5}},
{{"id": "y0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [{x}, 10, -1], "radius": {radius}, "height": 7}},
{{"id": "c0000001", "type": "cut", "v": 1, "name": "Hole", "base": "pppppppp", "tool": "y0000001"}},
{{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "c0000001", "edges": [{}], "radius": {r}}}
]}}"#,
        edges.join(", ")
    )
}

const TOP_RIM: &str = "c0000001.Edge(G pppppppp.box.Face6, G y0000001.cylinder.Face1)";
const BOTTOM_RIM: &str = "c0000001.Edge(G pppppppp.box.Face5, G y0000001.cylinder.Face1)";

/// What a fillet of radius `r` takes from a hole's rim of radius `radius`.
fn rim_fillet(radius: f64, r: f64) -> f64 {
    let c = r * (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI));
    2.0 * PI * (radius + c) * (1.0 - PI / 4.0) * r * r
}

fn volume(model: &Model, feature: &str) -> f64 {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => built.summary().unwrap().volume,
        other => panic!("{feature}: {other:?}"),
    }
}

/// The height of what `fillet` took from `base`: the centroid of what the base had and the
/// fillet hasn't.
fn taken_at(model: &Model, base: &str, fillet: &str) -> f64 {
    let measures = |feature: &str| match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => {
            let summary = built.summary().unwrap();
            (summary.volume, summary.center[2])
        }
        other => panic!("{feature}: {other:?}"),
    };
    let ((had, had_at), (has, has_at)) = (measures(base), measures(fillet));
    (had * had_at - has * has_at) / (had - has)
}

fn assert_close(got: f64, want: f64, what: &str) {
    assert!(
        (got - want).abs() <= 1e-6 * want.abs(),
        "{what}: {got} against {want}"
    );
}

#[test]
fn a_fillet_of_a_holes_rim_follows_the_rim_as_the_hole_moves_and_grows() {
    // The rim's name holds no position or size: moved along the plate, or made wider, the
    // hole's top rim is still `TOP_RIM`, and the fillet takes from it what a rim fillet of
    // that size takes, from the top (the plate is as thick above its middle as below, so
    // what's taken alone doesn't say which rim it was taken from; found by the tests
    // review). The ring's section, with u out from the rim and w down from the top, is the
    // unit square less the quarter disc about (1, 1): area A = 1 − π/4, centroid c each
    // way, and ∫uw dA = 1/4 − (π/4 − 2/3 + 1/8) = 19/24 − π/4. A ring weights its section
    // by radius R + u, so its centroid is at 5 − ∫w (R + u) dA / ∫(R + u) dA:
    // 5 − (R c A + 19/24 − π/4) / (A (R + c)).
    let engine = engine();
    let plate = 40.0 * 20.0 * 5.0;
    let c = (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI));
    let area = 1.0 - PI / 4.0;
    let taken_from_the_top =
        |radius: f64| 5.0 - (radius * c * area + 19.0 / 24.0 - PI / 4.0) / (area * (radius + c));
    for (x, radius) in [(20.0, 2.5), (8.0, 2.5), (30.0, 2.5), (20.0, 4.0)] {
        let model = publish(&engine, &holed(x, radius, &[TOP_RIM], 1.0));
        let errors: Vec<&Error> = model.errors().collect();
        assert!(errors.is_empty(), "x {x}, R {radius}: {errors:?}");
        let hole = plate - PI * radius * radius * 5.0;
        assert_close(volume(&model, "c0000001"), hole, "the hole");
        assert_close(
            volume(&model, "f0000001"),
            hole - rim_fillet(radius, 1.0),
            &format!("the fillet, x {x}, R {radius}"),
        );
        assert_close(
            taken_at(&model, "c0000001", "f0000001"),
            taken_from_the_top(radius),
            &format!("where the fillet took it, x {x}, R {radius}"),
        );
    }
}

#[test]
fn both_rims_filleted_take_twice_one_rim() {
    let engine = engine();
    let model = publish(&engine, &holed(20.0, 2.5, &[TOP_RIM, BOTTOM_RIM], 0.5));
    let hole = 40.0 * 20.0 * 5.0 - PI * 2.5 * 2.5 * 5.0;
    assert_close(
        volume(&model, "f0000001"),
        hole - 2.0 * rim_fillet(2.5, 0.5),
        "both rims",
    );
}

#[test]
fn a_rim_an_edit_takes_away_breaks_the_fillet_naming_the_rim_still_there() {
    // The pin made blind (from z = 2 up): the hole has no bottom rim. The fillet of it
    // fails, `reference_broken`, never taking the top rim for it, and offers the top rim
    // (it shares the pin's side) as a candidate.
    let engine = engine();
    let text = holed(20.0, 2.5, &[BOTTOM_RIM], 0.5).replace(
        r#""position": [20, 10, -1], "radius": 2.5, "height": 7"#,
        r#""position": [20, 10, 2], "radius": 2.5, "height": 7"#,
    );
    let model = publish(&engine, &text);
    let Some(FeatureOutcome::Broken(errors)) = model.outcome(&id("f0000001")) else {
        panic!("{:?}", model.outcome(&id("f0000001")));
    };
    assert_eq!(errors.len(), 1);
    let error = &errors[0];
    assert_eq!(error.code(), ErrorCode::ReferenceBroken);
    assert_eq!(error.parameter(), Some("edges.0"));
    assert_eq!(error.elements(), [BOTTOM_RIM]);
    assert!(
        error.candidates().iter().any(|c| c == TOP_RIM),
        "{:?}",
        error.candidates()
    );
}

#[test]
fn a_fillet_named_before_an_earlier_hole_is_added_still_finds_its_rim() {
    // N7: an earlier hole leaves the plate's top its own name, so the later hole's rim is
    // named as it was, and the fillet of it builds as before.
    let engine = engine();
    let one = publish(&engine, &holed(30.0, 2.5, &[TOP_RIM], 1.0));
    let before = volume(&one, "f0000001");
    let text = holed(30.0, 2.5, &[TOP_RIM], 1.0)
        .replace(
            r#"{"id": "c0000001", "type": "cut", "v": 1, "name": "Hole", "base": "pppppppp", "tool": "y0000001"}"#,
            r#"{"id": "y0000000", "type": "cylinder", "v": 1, "name": "Earlier", "position": [8, 10, -1], "radius": 1, "height": 7},
{"id": "c0000000", "type": "cut", "v": 1, "name": "Earlier hole", "base": "pppppppp", "tool": "y0000000"},
{"id": "c0000001", "type": "cut", "v": 1, "name": "Hole", "base": "c0000000", "tool": "y0000001"}"#,
        );
    let two = publish(&engine, &text);
    assert!(
        two.errors().next().is_none(),
        "{:?}",
        two.errors().collect::<Vec<_>>()
    );
    // The same fillet, less the earlier hole.
    assert_close(
        volume(&two, "f0000001"),
        before - PI * 1.0 * 5.0,
        "after an earlier hole",
    );
}

/// A box (10³, `aaaaaaaa`), a dressing `first` of its `edges` (`kind` and its size's
/// field, as a document writes them), and a fillet of radius 0.5 (`second`) of `edge`, an
/// edge of that.
fn dressed_twice(
    (first, kind, size): (&str, &str, &str),
    edges: &[&str],
    second: &str,
    edge: &str,
) -> String {
    let edges: Vec<String> = edges.iter().map(|e| format!("{e:?}")).collect();
    format!(
        r#"{HEAD}
{{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}},
{{"id": "{first}", "type": "{kind}", "v": 1, "name": "First", "base": "aaaaaaaa", "edges": [{}], {size}}},
{{"id": "{second}", "type": "fillet", "v": 1, "name": "Round", "base": "{first}", "edges": [{edge:?}], "radius": 0.5}}
]}}"#,
        edges.join(", ")
    )
}

/// A box (10³, `aaaaaaaa`) with `edges` chamfered by 1 (`cccccccc`), and a fillet of radius
/// 0.5 (`ffffffff`) of `fillet`, an edge of the chamfer.
fn chamfered(edges: &[&str], fillet: &str) -> String {
    dressed_twice(
        ("cccccccc", "chamfer", r#""size": {"distance": 1}"#),
        edges,
        "ffffffff",
        fillet,
    )
}

#[test]
fn a_fillet_of_a_chamfers_edge_survives_an_edge_dropped_from_the_chamfer() {
    // FreeCAD's testPartDesignTNPChamfer, as Silico has it: the box's vertical Edges 1, 5,
    // and 7 chamfered (each takes 1 × 1 / 2 × 10 = 5), then a fillet of an edge of Edge7's
    // chamfer, named by the two faces it bounds, the chamfer's and the box's Face2 (x = 10):
    // a vertical one, where the chamfer meets that side at 135°, so the fillet takes
    // (cot 67.5° − π/8) r² L = (√2 − 1 − π/8) 0.25 × 10. Edge1 dropped from the chamfer:
    // the fillet still names that edge, and takes the same.
    let engine = engine();
    let edge = "cccccccc.Edge(U cccccccc.Face(G aaaaaaaa.box.Edge7), aaaaaaaa.box.Face2)";
    let filleted = (2.0_f64.sqrt() - 1.0 - PI / 8.0) * 0.25 * 10.0;
    for (edges, chamfered_volume) in [
        (
            &[
                "aaaaaaaa.box.Edge1",
                "aaaaaaaa.box.Edge5",
                "aaaaaaaa.box.Edge7",
            ][..],
            1000.0 - 3.0 * 5.0,
        ),
        (
            &["aaaaaaaa.box.Edge5", "aaaaaaaa.box.Edge7"][..],
            1000.0 - 2.0 * 5.0,
        ),
    ] {
        let model = publish(&engine, &chamfered(edges, edge));
        let errors: Vec<&Error> = model.errors().collect();
        assert!(errors.is_empty(), "{edges:?}: {errors:?}");
        assert_close(volume(&model, "cccccccc"), chamfered_volume, "the chamfer");
        assert_close(
            volume(&model, "ffffffff"),
            chamfered_volume - filleted,
            &format!("the fillet, {edges:?}"),
        );
    }
    // Edge7 dropped instead: the chamfer face the fillet's edge is named from is gone, so
    // the fillet fails, loudly.
    let model = publish(
        &engine,
        &chamfered(&["aaaaaaaa.box.Edge1", "aaaaaaaa.box.Edge5"], edge),
    );
    let errors: Vec<ErrorCode> = model.errors().map(Error::code).collect();
    assert_eq!(errors, [ErrorCode::ReferenceBroken]);
}

/// A box (10³, `aaaaaaaa`) with `edges` filleted by 1 (`ffffffff`), and a fillet of radius
/// 0.5 (`gggggggg`) of `edge`, an edge of that fillet.
fn filleted_twice(edges: &[&str], edge: &str) -> String {
    dressed_twice(
        ("ffffffff", "fillet", r#""radius": 1"#),
        edges,
        "gggggggg",
        edge,
    )
}

#[test]
fn a_fillet_of_a_fillets_edge_survives_an_edge_dropped_from_the_fillet() {
    // FreeCAD's testPartDesignTNPFillet, as Silico has it (found missing by the port
    // review): the box's vertical Edges 1, 5, and 7 filleted by 1 (each takes
    // (1 − π/4) × 10), then a fillet of Edge7's fillet's top arc, named by the faces it
    // bounds. Edge1 dropped from the first fillet: the name still finds that arc, the
    // quarter circle of radius 1 about (9, 9) at the top, wherever the fillet numbers it.
    // (What the second fillet takes isn't the same: OCCT carries a fillet on along the
    // edges it meets smoothly, and the top's edges meet smoothly round each rounded corner.)
    let engine = engine();
    let mut kernel = silico_core::kernel::KernelWorker::start_worker(Path::new(env!(
        "CARGO_BIN_EXE_silico-kernel-worker"
    )))
    .unwrap();
    let arc = "ffffffff.Edge(U ffffffff.Face(G aaaaaaaa.box.Edge7), aaaaaaaa.box.Face6)";
    for (edges, rounded) in [
        (
            &[
                "aaaaaaaa.box.Edge1",
                "aaaaaaaa.box.Edge5",
                "aaaaaaaa.box.Edge7",
            ][..],
            3.0,
        ),
        (&["aaaaaaaa.box.Edge5", "aaaaaaaa.box.Edge7"][..], 2.0),
    ] {
        let model = publish(&engine, &filleted_twice(edges, arc));
        let errors: Vec<&Error> = model.errors().collect();
        assert!(errors.is_empty(), "{edges:?}: {errors:?}");
        assert_close(
            volume(&model, "ffffffff"),
            1000.0 - rounded * (1.0 - PI / 4.0) * 10.0,
            "the first fillet",
        );
        // The element the name finds, read as a reference is read (found by the tests
        // review: looking it up by its written name skipped the reading).
        let makers = |f: &FeatureId| match f.as_str() {
            "aaaaaaaa" => Some(Maker::Itself(PrimitiveKind::Box)),
            "ffffffff" => Some(Maker::By(OpCode::Fillet)),
            _ => None,
        };
        let map = model.element_map(&id("ffffffff")).unwrap();
        let element = map.element(&Name::parse(arc, &makers).unwrap()).unwrap();
        let Some(FeatureOutcome::Built { built, .. }) = model.outcome(&id("ffffffff")) else {
            panic!("{edges:?}");
        };
        let mesh = kernel.tessellate(&built.shape, 0.01).unwrap();
        let points = mesh.edges().nth(element.index as usize - 1).unwrap();
        assert!(points.len() > 2, "{edges:?}: {points:?}");
        for p in points {
            let (dx, dy) = (p[0] - 9.0, p[1] - 9.0);
            let r = (dx * dx + dy * dy).sqrt();
            assert!(
                (p[2] - 10.0).abs() < 1e-9
                    && (r - 1.0).abs() < 1e-6
                    && p[0] >= 9.0 - 1e-9
                    && p[1] >= 9.0 - 1e-9,
                "{edges:?}: {element} isn't Edge7's top arc: {p:?}"
            );
        }
    }
    // Edge7 dropped instead: the fillet face the arc is named from is gone, so the second
    // fillet fails, loudly.
    let model = publish(
        &engine,
        &filleted_twice(&["aaaaaaaa.box.Edge1", "aaaaaaaa.box.Edge5"], arc),
    );
    let errors: Vec<ErrorCode> = model.errors().map(Error::code).collect();
    assert_eq!(errors, [ErrorCode::ReferenceBroken]);
}

/// An edge of Edge7's chamfer named by the faces it bounds: the chamfer's and the box's
/// `face`.
fn of_edge7s_chamfer(face: u32) -> String {
    format!("cccccccc.Edge(U cccccccc.Face(G aaaaaaaa.box.Edge7), aaaaaaaa.box.Face{face})")
}

#[test]
fn an_edge_named_by_its_faces_breaks_when_an_edit_takes_it_away_and_never_moves() {
    // Trevin's (2026-10-17), the case the port review found re-pointing as FreeCAD's upper
    // pass names it (FreeCAD 1.1.3 moves it too, docs/ports/naming.md): the box's vertical
    // Edge7 (at x = y = 10) chamfered, its chamfer's bottom edge (on Face5, z = 0) and top
    // edge (on Face6, z = 10) each √2 long, where the chamfer meets the bottom or the top at
    // 90°. A fillet of radius r takes the (1 − π/4) r² corner along it, between the box's
    // sides (x = 10, y = 10), which cut it at 45°: as long as √2 + 2c where the corner's
    // centroid is, c = r (10 − 3π) / (3 (4 − π)) in from the edge. Then Edge8 (the bottom
    // of x = 10) chamfered too: the bottom edge is gone, so its fillet breaks, offered the
    // edge now between the two chamfers but never taking it; the top edge, untouched, still
    // takes the same. And the same at the top, with Edge12 (the top of y = 10).
    let engine = engine();
    let r: f64 = 0.5;
    let c = r * (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI));
    let filleted = (1.0 - PI / 4.0) * r * r * (2.0_f64.sqrt() + 2.0 * c);
    let (bottom, top) = (of_edge7s_chamfer(5), of_edge7s_chamfer(6));
    for edge in [&bottom, &top] {
        let model = publish(&engine, &chamfered(&["aaaaaaaa.box.Edge7"], edge));
        let errors: Vec<&Error> = model.errors().collect();
        assert!(errors.is_empty(), "{edge}: {errors:?}");
        assert_close(volume(&model, "cccccccc"), 1000.0 - 5.0, "the chamfer");
        assert_close(volume(&model, "ffffffff"), 995.0 - filleted, edge);
    }
    for (added, lost, kept, between) in [
        (
            "aaaaaaaa.box.Edge8",
            &bottom,
            &top,
            "cccccccc.Edge(U cccccccc.Face(G aaaaaaaa.box.Edge7), cccccccc.Face(G aaaaaaaa.box.Edge8))",
        ),
        (
            "aaaaaaaa.box.Edge12",
            &top,
            &bottom,
            "cccccccc.Edge(U cccccccc.Face(G aaaaaaaa.box.Edge7), cccccccc.Face(G aaaaaaaa.box.Edge12))",
        ),
    ] {
        let chamfer = ["aaaaaaaa.box.Edge7", added];
        let model = publish(&engine, &chamfered(&chamfer, lost));
        let Some(FeatureOutcome::Broken(errors)) = model.outcome(&id("ffffffff")) else {
            panic!("{added}: {:?}", model.outcome(&id("ffffffff")));
        };
        assert_eq!(errors.len(), 1, "{added}: {errors:?}");
        assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
        assert!(
            errors[0].candidates().iter().any(|c| c == between),
            "{added}: {:?}",
            errors[0].candidates()
        );
        let model = publish(&engine, &chamfered(&chamfer, kept));
        let errors: Vec<&Error> = model.errors().collect();
        assert!(errors.is_empty(), "{added}, {kept}: {errors:?}");
        assert_close(
            volume(&model, "ffffffff"),
            volume(&model, "cccccccc") - filleted,
            &format!("{added}, {kept}"),
        );
    }
}

/// A box (10³, `aaaaaaaa`), with a hole far from its Edge1 if `hole` (pin `y0000001`, cut
/// `c0000001`), a fillet of radius 1 of its Edge1 (`ffffffff`), and a fillet of radius 0.5
/// (`gggggggg`) of `edge`, an edge of that.
fn rounded(hole: bool, edge: &str) -> String {
    let (holed, base) = if hole {
        (
            r#"{"id": "y0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [7, 7, -1], "radius": 1, "height": 12},
{"id": "c0000001", "type": "cut", "v": 1, "name": "Hole", "base": "aaaaaaaa", "tool": "y0000001"},
"#,
            "c0000001",
        )
    } else {
        ("", "aaaaaaaa")
    };
    format!(
        r#"{HEAD}
{{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}},
{holed}{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "{base}", "edges": ["aaaaaaaa.box.Edge1"], "radius": 1}},
{{"id": "gggggggg", "type": "fillet", "v": 1, "name": "Round001", "base": "ffffffff", "edges": [{edge:?}], "radius": 0.5}}
]}}"#
    )
}

#[test]
fn a_fillets_own_edges_and_vertices_keep_their_names_through_an_unrelated_edit() {
    // Trevin's (2026-10-17): a fillet's face's edges and vertices, which OCCT gives no
    // history, are named by the named faces (or edges) they bound, so each is a reference of
    // its own, none unstable, and a hole elsewhere (which renumbers them) changes none of
    // their names: a fillet of the first fillet's top arc still finds that arc, and takes
    // the same amount.
    let engine = engine();
    let arc = "ffffffff.Edge(U ffffffff.Face(G aaaaaaaa.box.Edge1), aaaaaaaa.box.Face6)";
    let mut made = Vec::new();
    let mut taken = Vec::new();
    for hole in [false, true] {
        let model = publish(&engine, &rounded(hole, arc));
        let errors: Vec<&Error> = model.errors().collect();
        assert!(errors.is_empty(), "hole {hole}: {errors:?}");
        let map = model.element_map(&id("ffffffff")).unwrap();
        // The fillet's face, its four edges, and their four ends.
        let own: std::collections::BTreeSet<String> = map
            .iter()
            .map(|(_, names)| names[0].to_string())
            .filter(|name| name.starts_with("ffffffff."))
            .collect();
        assert_eq!(own.len(), 1 + 4 + 4, "hole {hole}: {own:#?}");
        for (element, names) in map.iter() {
            assert!(
                names.iter().all(|n| !map.is_unstable(n)),
                "hole {hole}: {element} {names:?}"
            );
        }
        made.push(own);
        taken.push(volume(&model, "ffffffff") - volume(&model, "gggggggg"));
    }
    assert_eq!(made[0], made[1]);
    assert!(made[0].contains(arc), "{:#?}", made[0]);
    assert_close(taken[1], taken[0], "the arc's fillet, with the hole");
    assert!(taken[0] > 0.0, "{taken:?}");
}

/// Two boxes fused (`rrrrrrrr`): a 10³ at the origin, and one beside it, 10 × `width` × 5
/// at (10, 0, 0); and a fillet of radius 1 (`ffffffff`) of `edge` of the fuse.
fn fused(width: f64, edge: &str) -> String {
    format!(
        r#"{HEAD}
{{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}},
{{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Box001", "position": [10, 0, 0], "length": 10, "width": {width}, "height": 5}},
{{"id": "rrrrrrrr", "type": "fuse", "v": 1, "name": "Fusion", "base": "aaaaaaaa", "tool": "bbbbbbbb"}},
{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "rrrrrrrr", "edges": [{edge:?}], "radius": 1}}
]}}"#
    )
}

#[test]
fn a_fillet_follows_its_edge_when_an_upstream_edit_renumbers_it() {
    // FreeCAD's testBaseFeatureAttachmentSupport, as Silico has it: the second box's top
    // meets the first box's side along the second box's Edge2 (its top edge at x = 10,
    // `width` long), an inside corner: a fillet adds (1 − π/4) r² × width. The second box
    // narrowed from 10 to 3, the fuse numbers its edges otherwise (the edge is its 18th,
    // then its 19th: FreeCAD's stored `Face8` became `Face9`), and the name, unchanged in
    // the document, still finds it.
    let engine = engine();
    let edge = "bbbbbbbb.box.Edge2";
    let mut numbers = Vec::new();
    for width in [10.0, 3.0] {
        let model = publish(&engine, &fused(width, edge));
        let errors: Vec<&Error> = model.errors().collect();
        assert!(errors.is_empty(), "width {width}: {errors:?}");
        let fuse = 1000.0 + 10.0 * width * 5.0;
        assert_close(volume(&model, "rrrrrrrr"), fuse, "the fuse");
        assert_close(
            volume(&model, "ffffffff"),
            fuse + (1.0 - PI / 4.0) * width,
            &format!("the fillet, width {width}"),
        );
        let map = model.element_map(&id("rrrrrrrr")).unwrap();
        let (element, _) = map
            .iter()
            .find(|(_, names)| names[0].to_string() == edge)
            .unwrap();
        numbers.push(element.index);
    }
    assert_ne!(
        numbers[0], numbers[1],
        "the edit renumbers the edge: {numbers:?}"
    );
}
