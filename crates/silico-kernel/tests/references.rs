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
        KernelPool::start_on_demand(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2)
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

const TOP_RIM: &str = "c0000001.cut.Edge(G pppppppp.box.Face6, G y0000001.cylinder.Face1)";
const BOTTOM_RIM: &str = "c0000001.cut.Edge(G pppppppp.box.Face5, G y0000001.cylinder.Face1)";

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
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("f0000001")) else {
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
    let edge =
        "cccccccc.chamfer.Edge(U cccccccc.chamfer.Face(G aaaaaaaa.box.Edge7), aaaaaaaa.box.Face2)";
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
    let arc =
        "ffffffff.fillet.Edge(U ffffffff.fillet.Face(G aaaaaaaa.box.Edge7), aaaaaaaa.box.Face6)";
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
    format!(
        "cccccccc.chamfer.Edge(U cccccccc.chamfer.Face(G aaaaaaaa.box.Edge7), aaaaaaaa.box.Face{face})"
    )
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
            "cccccccc.chamfer.Edge(U cccccccc.chamfer.Face(G aaaaaaaa.box.Edge7), cccccccc.chamfer.Face(G aaaaaaaa.box.Edge8))",
        ),
        (
            "aaaaaaaa.box.Edge12",
            &top,
            &bottom,
            "cccccccc.chamfer.Edge(U cccccccc.chamfer.Face(G aaaaaaaa.box.Edge7), cccccccc.chamfer.Face(G aaaaaaaa.box.Edge12))",
        ),
    ] {
        let chamfer = ["aaaaaaaa.box.Edge7", added];
        let model = publish(&engine, &chamfered(&chamfer, lost));
        let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("ffffffff")) else {
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
    let arc =
        "ffffffff.fillet.Edge(U ffffffff.fillet.Face(G aaaaaaaa.box.Edge1), aaaaaaaa.box.Face6)";
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

/// Feature `feature`'s one error in `model`.
fn error_of<'a>(model: &'a Model, feature: &str) -> &'a Error {
    let errors: Vec<&Error> = model
        .errors()
        .filter(|e| e.feature() == Some(feature))
        .collect();
    match errors[..] {
        [error] => error,
        _ => panic!("{feature}: {errors:?}"),
    }
}

/// That `error`'s words name no edge by its number (`Edge18`), as no document does.
fn assert_no_numbers(error: &Error) {
    for words in [Some(error.message()), error.cause(), error.suggestion()]
        .into_iter()
        .flatten()
    {
        let numbered = words
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| {
                word.strip_prefix("Edge")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            });
        assert!(!numbered, "{words:?} in {error:?}");
    }
}

#[test]
fn a_fillet_occt_cant_build_names_its_edge_as_the_document_does() {
    // The inside corner of fused boxes, rounded by 30 where the boxes are 10 across: OCCT
    // can't. The error names the edge by the document's name and its place in `edges`,
    // never by the fuse's own number for it (its 18th edge), which no document holds.
    let engine = engine();
    let text = fused(10.0, "bbbbbbbb.box.Edge2").replace(r#""radius": 1}"#, r#""radius": 30}"#);
    let model = publish(&engine, &text);
    let error = error_of(&model, "ffffffff");
    assert_eq!(
        (error.code(), error.elements(), error.parameter()),
        (
            ErrorCode::KernelFailed,
            &["bbbbbbbb.box.Edge2".to_owned()][..],
            Some("edges.0")
        ),
        "{error:?}"
    );
    assert_no_numbers(error);
}

#[test]
fn an_edge_the_kernel_refuses_is_named_by_its_place_and_its_name() {
    // A hole's rim and the pin's seam (its Edge1, what the cut keeps of it a piece, N8):
    // the kernel refuses the seam, which joins the cylinder's face to itself, tangent. The
    // refusal names the seam, the second of `edges`, by its name and its place, and leaves
    // the rim, the first, out.
    let engine = engine();
    let seam = "c0000001.cut.Edge(I y0000001.cylinder.Edge1)";
    let model = publish(&engine, &holed(20.0, 3.0, &[TOP_RIM, seam], 0.5));
    let error = error_of(&model, "f0000001");
    assert_eq!(
        (error.code(), error.elements(), error.parameter()),
        (
            ErrorCode::InvalidInput,
            &[seam.to_owned()][..],
            Some("edges.1")
        ),
        "{error:?}"
    );
    assert!(error.message().contains("edges.1"), "{error:?}");
    assert_no_numbers(error);
}

// --- N8: an operation's names say the operation; pieces are the feature's own ---------

/// The milestone review's document (MF1): a box (10³, `aaaaaaaa`), a pin (r 2,
/// `bbbbbbbb`) on the box's edge at x = 10, y = 0, their boolean `cccccccc` of type `op`
/// (of `base` and `tool`), and a fillet of radius 0.5 (`ffffffff`) of `edge`.
fn boolean_of(op: &str, base: &str, tool: &str, edge: &str) -> String {
    format!(
        r#"{HEAD}
{{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}},
{{"id": "bbbbbbbb", "type": "cylinder", "v": 1, "name": "Pin", "position": [10, 0, -1], "radius": 2, "height": 12}},
{{"id": "cccccccc", "type": "{op}", "v": 1, "name": "Op", "base": "{base}", "tool": "{tool}"}},
{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "cccccccc", "edges": [{edge:?}], "radius": 0.5}}
]}}"#
    )
}

/// What a fillet of radius 0.5 of a straight edge `length` long between faces at a right
/// angle takes away (convex) or adds (concave): (1 − π/4) r² × length.
fn blend(length: f64) -> f64 {
    (1.0 - PI / 4.0) * 0.25 * length
}

/// The fillet's change to its base's volume, in `model`.
fn filleted_by(model: &Model) -> f64 {
    volume(model, "ffffffff") - volume(model, "cccccccc")
}

/// Feature `feature`'s broken references' candidates, the first error's, and its
/// parameter, in `model`.
fn broken(model: &Model, feature: &str) -> (Option<String>, Vec<String>) {
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id(feature)) else {
        panic!("{feature}: {:?}", model.outcome(&id(feature)));
    };
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(
        errors[0].code(),
        ErrorCode::ReferenceBroken,
        "{:?}",
        errors[0]
    );
    (
        errors[0].parameter().map(str::to_owned),
        errors[0].candidates().to_vec(),
    )
}

#[test]
fn a_boolean_retyped_or_swapped_breaks_a_reference_to_a_piece_it_kept_never_re_points() {
    // The box's Edge9 (along x at y = 0, z = 0), the pin across its end at x = 10. A cut
    // keeps the box's outside, x in [0, 8], the edge bitten: its name is the box's. A common
    // keeps the inside, x in [8, 10], and the cut with base and tool swapped keeps it too, a
    // notch's concave edge: pieces, the boolean's own. A fuse keeps the outside, as the cut.
    // Before N8 every one resolved `aaaaaaaa.box.Edge9`, to two different edges, silently.
    let engine = engine();
    let edge9 = "aaaaaaaa.box.Edge9";
    // Cut: x in [0, 8] rounded (the blend's end at the pin aside, a few ten-thousandths).
    let cut = publish(&engine, &boolean_of("cut", "aaaaaaaa", "bbbbbbbb", edge9));
    assert!(
        cut.errors().next().is_none(),
        "{:?}",
        cut.errors().collect::<Vec<_>>()
    );
    assert!(
        (filleted_by(&cut) + blend(8.0)).abs() < 1e-3,
        "{}",
        filleted_by(&cut)
    );
    // Fuse: the same edge, bitten alike.
    let fuse = publish(&engine, &boolean_of("fuse", "aaaaaaaa", "bbbbbbbb", edge9));
    assert!(
        fuse.errors().next().is_none(),
        "{:?}",
        fuse.errors().collect::<Vec<_>>()
    );
    assert!(
        (filleted_by(&fuse) + blend(8.0)).abs() < 1e-3,
        "{}",
        filleted_by(&fuse)
    );
    // Common and the swapped cut: broken, the piece the one candidate.
    for (op, base, tool, piece, sign) in [
        (
            "common",
            "aaaaaaaa",
            "bbbbbbbb",
            "cccccccc.common.Edge(I aaaaaaaa.box.Edge9)",
            -1.0,
        ),
        (
            "cut",
            "bbbbbbbb",
            "aaaaaaaa",
            "cccccccc.cut.Edge(I aaaaaaaa.box.Edge9)",
            1.0,
        ),
    ] {
        let model = publish(&engine, &boolean_of(op, base, tool, edge9));
        assert_eq!(
            broken(&model, "ffffffff"),
            (Some("edges.0".to_owned()), vec![piece.to_owned()]),
            "{op} of {base} and {tool}"
        );
        // The piece by its name: x in [8, 10], 2 long, taken away under the common and,
        // a concave edge, added under the swapped cut.
        let model = publish(&engine, &boolean_of(op, base, tool, piece));
        assert!(
            model.errors().next().is_none(),
            "{:?}",
            model.errors().collect::<Vec<_>>()
        );
        assert!(
            (filleted_by(&model) - sign * blend(2.0)).abs() < 1e-3,
            "{op} of {base} and {tool}: {}",
            filleted_by(&model)
        );
    }
    // The bitten edge's name is never a piece's, nor a piece's the edge it came from.
    let piece = "cccccccc.cut.Edge(I aaaaaaaa.box.Edge9)";
    let model = publish(&engine, &boolean_of("cut", "aaaaaaaa", "bbbbbbbb", piece));
    assert_eq!(
        broken(&model, "ffffffff"),
        (Some("edges.0".to_owned()), vec![edge9.to_owned()])
    );
}

#[test]
fn a_rim_says_the_operation_that_made_it_and_breaks_when_it_changes() {
    // The edge the cut generated from the box's top (Face6) and the pin's side: under a
    // fuse or a common the same faces meet in an edge of another solid, which isn't the
    // cut's rim; the reference breaks, the operation's own rim its first candidate.
    let engine = engine();
    let rim =
        |op: &str| format!("cccccccc.{op}.Edge(G aaaaaaaa.box.Face6, G bbbbbbbb.cylinder.Face1)");
    let cut = publish(
        &engine,
        &boolean_of("cut", "aaaaaaaa", "bbbbbbbb", &rim("cut")),
    );
    assert!(
        cut.errors().next().is_none(),
        "{:?}",
        cut.errors().collect::<Vec<_>>()
    );
    for op in ["fuse", "common"] {
        let model = publish(
            &engine,
            &boolean_of(op, "aaaaaaaa", "bbbbbbbb", &rim("cut")),
        );
        let (parameter, candidates) = broken(&model, "ffffffff");
        assert_eq!(parameter.as_deref(), Some("edges.0"));
        assert_eq!(candidates.first(), Some(&rim(op)), "{op}: {candidates:?}");
        let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("ffffffff")) else {
            unreachable!()
        };
        assert!(
            errors[0].message().contains(&format!(
                "names an element a cut made, but cccccccc is a {op} now"
            )),
            "{}",
            errors[0]
        );
    }
}

#[test]
fn a_holes_wall_is_the_cuts_own_and_keeps_its_name_through_later_holes_and_a_boss() {
    // A plate (40 × 20 × 5), a hole (r 2 at (10, 10)), a second hole overlapping it (r 2 at
    // (13, 10)), and a boss (r 2 at (30, 10), rising from inside the plate to 3 above it),
    // fused. The first hole's wall is what the cut kept of the pin inside the plate, a
    // piece: `c0000001.cut.Face(I y0000001.cylinder.Face1)`, FreeCAD's `Face1;:M;CUT`. The
    // second hole and the boss keep what of it lies outside them (bites), so it keeps that
    // name; the plate's top keeps its own through all three; the boss's side, what the fuse
    // keeps of it outside the plate, its own. The second hole's wall, opened by the first
    // across its own seam, is two pieces, ranked.
    let engine = engine();
    let text = format!(
        r#"{HEAD}
{{"id": "pppppppp", "type": "box", "v": 1, "name": "Plate", "position": [0, 0, 0], "length": 40, "width": 20, "height": 5}},
{{"id": "y0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [10, 10, -1], "radius": 2, "height": 7}},
{{"id": "c0000001", "type": "cut", "v": 1, "name": "Hole", "base": "pppppppp", "tool": "y0000001"}},
{{"id": "y0000002", "type": "cylinder", "v": 1, "name": "Pin2", "position": [13, 10, -1], "radius": 2, "height": 7}},
{{"id": "c0000002", "type": "cut", "v": 1, "name": "Hole2", "base": "c0000001", "tool": "y0000002"}},
{{"id": "y0000003", "type": "cylinder", "v": 1, "name": "Boss", "position": [30, 10, 4], "radius": 2, "height": 4}},
{{"id": "b0000001", "type": "fuse", "v": 1, "name": "Bossed", "base": "c0000002", "tool": "y0000003"}}
]}}"#
    );
    let model = publish(&engine, &text);
    assert!(
        model.errors().next().is_none(),
        "{:?}",
        model.errors().collect::<Vec<_>>()
    );
    let wall = "c0000001.cut.Face(I y0000001.cylinder.Face1)";
    let first = model.element_map(&id("c0000001")).unwrap();
    let (_, names) = first
        .iter()
        .find(|(_, names)| names[0].to_string() == wall)
        .unwrap_or_else(|| panic!("no {wall} in the first hole"));
    assert_eq!(
        silico_core::naming::freecad::text(&names[0]),
        "Face1;:M;CUT"
    );
    let last: Vec<String> = model
        .element_map(&id("b0000001"))
        .unwrap()
        .iter()
        .filter(|(e, _)| e.kind == silico_core::kernel::ElementKind::Face)
        .map(|(_, names)| names[0].to_string())
        .collect();
    for face in [
        wall,
        "c0000002.cut.Face(I1/2 y0000002.cylinder.Face1)",
        "c0000002.cut.Face(I2/2 y0000002.cylinder.Face1)",
        "pppppppp.box.Face6",
        "y0000003.cylinder.Face1",
    ] {
        assert!(last.contains(&face.to_owned()), "no {face} in {last:#?}");
    }
}

/// A box (10³, `aaaaaaaa`), its Edge7 (vertical, at x = y = 10) rounded or cut back by 1 as
/// `dressing` (`ffffffff`), and a fillet of radius 0.5 (`gggggggg`) of `edge`.
fn dressed_as(dressing: &str, edge: &str) -> String {
    let size = match dressing {
        "fillet" => r#""radius": 1"#,
        _ => r#""size": {"distance": 1}"#,
    };
    dressed_twice(
        ("ffffffff", dressing, size),
        &["aaaaaaaa.box.Edge7"],
        "gggggggg",
        edge,
    )
}

#[test]
fn a_fillet_made_a_chamfer_keeps_the_box_edges_it_shortened_and_breaks_its_own() {
    // The box's top edge along x at y = 10 (Edge12) ends at Edge7's top: a fillet or a
    // chamfer of Edge7 shortens it by 1, keeping what lies outside the blend (a bite), so it
    // keeps its name under either. The blend's own top edge, named from the blend's face and
    // the box's top, is the fillet's (`ffffffff.fillet.Edge(U ...)`): under a chamfer it
    // breaks, the chamfer's edge its first candidate.
    let engine = engine();
    let kept = "aaaaaaaa.box.Edge12";
    for dressing in ["fillet", "chamfer"] {
        let model = publish(&engine, &dressed_as(dressing, kept));
        let map = model.element_map(&id("ffffffff")).unwrap();
        let name = Name::parse(kept, &|f| match f.as_str() {
            "aaaaaaaa" => Some(Maker::Itself(PrimitiveKind::Box)),
            _ => None,
        })
        .unwrap();
        assert!(map.element(&name).is_some(), "{dressing}");
        assert!(!map.is_unstable(&name), "{dressing}");
    }
    let own = |dressing: &str| {
        format!(
            "ffffffff.{dressing}.Edge(U ffffffff.{dressing}.Face(G aaaaaaaa.box.Edge7), aaaaaaaa.box.Face6)"
        )
    };
    let filleted = publish(&engine, &dressed_as("fillet", &own("fillet")));
    assert!(
        filleted.errors().next().is_none(),
        "{:?}",
        filleted.errors().collect::<Vec<_>>()
    );
    let chamfered = publish(&engine, &dressed_as("chamfer", &own("fillet")));
    let (parameter, candidates) = broken(&chamfered, "gggggggg");
    assert_eq!(parameter.as_deref(), Some("edges.0"));
    assert_eq!(candidates.first(), Some(&own("chamfer")), "{candidates:?}");
}

/// A 10 mm cube `aaaaaaaa` and a pin `bbbbbbbb` (r 1, z from `bottom` to `bottom` + 6 at
/// (5, 5)): their boolean `cccccccc` of `op`, and a fillet (`ffffffff`, r 0.2) of `edge`.
fn pinned(op: &str, bottom: f64, edge: &str) -> String {
    format!(
        r#"{HEAD}
{{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}},
{{"id": "bbbbbbbb", "type": "cylinder", "v": 1, "name": "Pin", "position": [5, 5, {bottom}], "radius": 1, "height": 6}},
{{"id": "cccccccc", "type": "{op}", "v": 1, "name": "Op", "base": "aaaaaaaa", "tool": "bbbbbbbb"}},
{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "cccccccc", "edges": [{edge:?}], "radius": 0.2}}
]}}"#
    )
}

/// Feature `feature`'s element names in `model`, each element's own.
fn names_in(model: &Model, feature: &str) -> Vec<String> {
    model
        .element_map(&id(feature))
        .unwrap()
        .iter()
        .map(|(_, names)| names[0].to_string())
        .collect()
}

#[test]
fn a_voids_elements_are_the_cuts_own_whole_or_trimmed_so_an_edit_that_trims_one_keeps_its_name() {
    // N9 (Fable's, 2026-10-18), on purpose the opposite of what N8 held here: there, a
    // tool's element kept whole kept its bare name and became a piece once an edit trimmed
    // it, so a reference to it broke. Now everything a cut keeps of its tool is the cut's
    // own, `I`, whole or trimmed: the void's seam is `cccccccc.cut.Edge(I
    // bbbbbbbb.cylinder.Edge1)` with the pin inside the block, and the same name once the pin
    // is moved down through the floor and the seam is trimmed, as a bitten face keeps its
    // name through a hole. Made a common, the result is the pin, every element the common's
    // own, and the void's rim breaks with the common's as its first candidate.
    let engine = engine();
    let seam = "cccccccc.cut.Edge(I bbbbbbbb.cylinder.Edge1)";
    let rim = "cccccccc.cut.Edge(I bbbbbbbb.cylinder.Edge3)";
    let void = publish(&engine, &pinned("cut", 2.0, rim));
    assert!(
        void.errors().next().is_none(),
        "{:?}",
        void.errors().collect::<Vec<_>>()
    );
    let void_names = names_in(&void, "cccccccc");
    for edge in 1..=3 {
        let name = format!("cccccccc.cut.Edge(I bbbbbbbb.cylinder.Edge{edge})");
        assert!(void_names.contains(&name), "{name}: {void_names:#?}");
    }
    assert!(
        !void_names.iter().any(|n| n.starts_with("bbbbbbbb.")),
        "{void_names:#?}"
    );
    // The void's rim, concave, rounded: the ring 2π (R − c) (1 − π/4) r² added, R = 1.
    let c = 0.2 * (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI));
    let ring = 2.0 * PI * (1.0 - c) * (1.0 - PI / 4.0) * 0.04;
    let added = volume(&void, "ffffffff") - volume(&void, "cccccccc");
    assert!((added - ring).abs() < 1e-3 * ring, "{added} against {ring}");
    // (A seam is a smooth edge, which OCCT won't fillet: the rim is the reference, and the
    // seam's name is checked in the map.)
    let through = publish(&engine, &pinned("cut", -1.0, rim));
    assert!(
        through.errors().next().is_none(),
        "{:?}",
        through.errors().collect::<Vec<_>>()
    );
    assert!(names_in(&through, "cccccccc").contains(&seam.to_owned()));
    let common = publish(&engine, &pinned("common", 2.0, rim));
    let (parameter, candidates) = broken(&common, "ffffffff");
    assert_eq!(parameter.as_deref(), Some("edges.0"));
    assert_eq!(
        candidates.first().map(String::as_str),
        Some("cccccccc.common.Edge(I bbbbbbbb.cylinder.Edge3)"),
        "{candidates:?}"
    );
    let common_names = names_in(&common, "cccccccc");
    assert!(
        common_names
            .iter()
            .all(|n| n.starts_with("cccccccc.common.")),
        "{common_names:#?}"
    );
    // The pin's rim, convex, rounded by the common's name: the same ring taken away.
    let pin = publish(
        &engine,
        &pinned(
            "common",
            2.0,
            "cccccccc.common.Edge(I bbbbbbbb.cylinder.Edge3)",
        ),
    );
    let taken = volume(&pin, "cccccccc") - volume(&pin, "ffffffff");
    assert!((taken - ring).abs() < 1e-3 * ring, "{taken} against {ring}");
}

/// The format reviewer's fork (round 2): the box `aaaaaaaa` and a tool `tttttttt`, a jaw
/// (8 × 4 × 2 at (-1, -3, -1)) less a slot (4 × 2 × 4 at (1, -0.5, -2)), which covers the
/// box's Edge9 (along x at y = z = 0) at x in [0, 1] and [5, 7]. Then `cccccccc`, a cut of
/// `base` and `tool`, and a fillet `ffffffff` of `edge`, radius 0.2.
fn forked(base: &str, tool: &str, edge: &str) -> String {
    format!(
        r#"{HEAD}
{{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}},
{{"id": "jjjjjjjj", "type": "box", "v": 1, "name": "Jaw", "position": [-1, -3, -1], "length": 8, "width": 4, "height": 2}},
{{"id": "ssssssss", "type": "box", "v": 1, "name": "Slot", "position": [1, -0.5, -2], "length": 4, "width": 2, "height": 4}},
{{"id": "tttttttt", "type": "cut", "v": 1, "name": "Fork", "base": "jjjjjjjj", "tool": "ssssssss"}},
{{"id": "cccccccc", "type": "cut", "v": 1, "name": "Op", "base": "{base}", "tool": "{tool}"}},
{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "cccccccc", "edges": [{edge:?}], "radius": 0.2}}
]}}"#
    )
}

#[test]
fn a_cuts_swap_names_a_split_edges_inside_pieces_i_and_breaks_a_reference_to_the_outside_ones() {
    // N9 (round 2's R2F1): a cut keeps its base's outside, x in [1, 5] and [7, 10], pieces
    // `M1/2`, `M2/2`; with base and tool swapped it keeps the box's inside, x in [0, 1] and
    // [5, 7], concave, the cut's own, `I1/2`, `I2/2`. A reference to an outside piece breaks
    // on the swap, the inside pieces its candidates. A fillet of radius 0.2 of a straight edge
    // l long takes (convex) or adds (concave) (1 − π/4) 0.04 l.
    let straight = |length: f64| (1.0 - PI / 4.0) * 0.04 * length;
    let engine = engine();
    let outside = publish(
        &engine,
        &forked(
            "aaaaaaaa",
            "tttttttt",
            "cccccccc.cut.Edge(M1/2 aaaaaaaa.box.Edge9)",
        ),
    );
    assert!(
        outside.errors().next().is_none(),
        "{:?}",
        outside.errors().collect::<Vec<_>>()
    );
    let taken = volume(&outside, "cccccccc") - volume(&outside, "ffffffff");
    assert!(
        (taken - straight(4.0)).abs() < 1e-6,
        "{taken} against {}",
        straight(4.0)
    );
    let names = names_in(&outside, "cccccccc");
    for piece in ["M1/2", "M2/2"] {
        let name = format!("cccccccc.cut.Edge({piece} aaaaaaaa.box.Edge9)");
        assert!(names.contains(&name), "{name}: {names:#?}");
    }
    // (The fork's own elements inside the box are the cut's, `I`; the box's, bitten, never.)
    assert!(
        !names
            .iter()
            .any(|n| n.contains("(I aaaaaaaa.") || n.contains("I1/2 aaaaaaaa.")),
        "{names:#?}"
    );
    let inside = publish(
        &engine,
        &forked(
            "tttttttt",
            "aaaaaaaa",
            "cccccccc.cut.Edge(M1/2 aaaaaaaa.box.Edge9)",
        ),
    );
    let (parameter, candidates) = broken(&inside, "ffffffff");
    assert_eq!(parameter.as_deref(), Some("edges.0"));
    assert_eq!(
        candidates.first().map(String::as_str),
        Some("cccccccc.cut.Edge(I1/2 aaaaaaaa.box.Edge9)"),
        "{candidates:?}"
    );
    let names = names_in(&inside, "cccccccc");
    assert!(
        !names.iter().any(|n| n.contains("M1/2 aaaaaaaa.box.Edge9")),
        "{names:#?}"
    );
    for (piece, length) in [("I1/2", 1.0), ("I2/2", 2.0)] {
        let model = publish(
            &engine,
            &forked(
                "tttttttt",
                "aaaaaaaa",
                &format!("cccccccc.cut.Edge({piece} aaaaaaaa.box.Edge9)"),
            ),
        );
        assert!(
            model.errors().next().is_none(),
            "{:?}",
            model.errors().collect::<Vec<_>>()
        );
        let added = volume(&model, "ffffffff") - volume(&model, "cccccccc");
        assert!((added - straight(length)).abs() < 1e-6, "{piece}: {added}");
    }
}

#[test]
fn commons_in_a_row_each_make_a_piece_and_deleting_one_breaks_a_reference_through_it() {
    // A box (10³) kept by two commons in turn, each with a box reaching past the others'
    // sides but short in x: x to 8, then x to 7. The box's top edge along x at y = 0
    // (Edge10) is trimmed by each, kept inside: a piece of a piece, FreeCAD's
    // `Edge10;:M;CMN;:M;CMN`. With the first common deleted (the second keeping the box),
    // the edge is one piece, `c0000005.common.Edge(I pppppppp.box.Edge10)`: a reference
    // through the first names a feature there isn't, and the gate refuses it before
    // anything is rebuilt; the new name, through the second alone, finds the edge.
    let engine = engine();
    let keeper = |id: &str, x: f64| {
        format!(
            r#"{{"id": "{id}", "type": "box", "v": 1, "name": "{id}", "position": [-1, -1, -1], "length": {x}, "width": 12, "height": 12}}"#
        )
    };
    let document = |first: bool, edge: &str| {
        let mut features = vec![
            r#"{"id": "pppppppp", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#.to_owned(),
        ];
        let base = if first {
            features.push(keeper("k0000000", 9.0));
            features.push(r#"{"id": "c0000000", "type": "common", "v": 1, "name": "C0", "base": "pppppppp", "tool": "k0000000"}"#.to_owned());
            "c0000000"
        } else {
            "pppppppp"
        };
        features.push(keeper("k0000005", 8.0));
        features.push(format!(
            r#"{{"id": "c0000005", "type": "common", "v": 1, "name": "C5", "base": "{base}", "tool": "k0000005"}}"#
        ));
        features.push(format!(
            r#"{{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "c0000005", "edges": [{edge:?}], "radius": 0.5}}"#
        ));
        format!("{HEAD}\n{}\n]}}", features.join(",\n"))
    };
    let twice = "c0000005.common.Edge(I c0000000.common.Edge(I pppppppp.box.Edge10))";
    let model = publish(&engine, &document(true, twice));
    assert!(
        model.errors().next().is_none(),
        "{:?}",
        model.errors().collect::<Vec<_>>()
    );
    let map = model.element_map(&id("c0000005")).unwrap();
    let (_, names) = map
        .iter()
        .find(|(_, names)| names[0].to_string() == twice)
        .unwrap();
    assert_eq!(
        silico_core::naming::freecad::text(&names[0]),
        "Edge10;:M;CMN;:M;CMN"
    );
    // Rounded along x in [0, 7]: 7 long.
    let rounded = volume(&model, "f0000001") - volume(&model, "c0000005");
    assert!((rounded + blend(7.0)).abs() < 1e-3, "{rounded}");
    let invalid = Checked::new(read(document(false, twice).as_bytes()).unwrap()).unwrap_err();
    let said: Vec<(ErrorCode, Option<&str>)> = invalid
        .errors()
        .iter()
        .map(|e| (e.code(), e.parameter()))
        .collect();
    assert_eq!(said, [(ErrorCode::InvalidInput, Some("edges.0"))]);
    assert!(
        invalid.errors()[0]
            .message()
            .contains("no feature c0000000"),
        "{:?}",
        invalid.errors()
    );
    let once = publish(
        &engine,
        &document(false, "c0000005.common.Edge(I pppppppp.box.Edge10)"),
    );
    assert!(
        once.errors().next().is_none(),
        "{:?}",
        once.errors().collect::<Vec<_>>()
    );
    let rounded = volume(&once, "f0000001") - volume(&once, "c0000005");
    assert!((rounded + blend(7.0)).abs() < 1e-3, "{rounded}");
}

#[test]
fn a_fuses_or_a_commons_operands_swapped_keep_its_bites_and_pieces_and_reorder_its_rims_loudly() {
    // A fuse or a common is the same solid whichever operand is its base. What it keeps of
    // an element alone (a bite, a piece) is named alike either way, and a reference to it
    // still finds it: the box's Edge9, bitten by the fuse, a piece of the common. A rim,
    // generated from both, lists its sources base first, as Silico orders sources (by role,
    // where FreeCAD orders them by object), so with the operands swapped its name is
    // another: the reference breaks, loudly, the reordered rim its first candidate. Found
    // writing N8's tests, which had expected every name alike.
    let engine = engine();
    for (op, edge9) in [
        ("fuse", "aaaaaaaa.box.Edge9".to_owned()),
        (
            "common",
            "cccccccc.common.Edge(I aaaaaaaa.box.Edge9)".to_owned(),
        ),
    ] {
        let names = |base: &str, tool: &str| -> std::collections::BTreeSet<String> {
            let model = publish(&engine, &boolean_of(op, base, tool, &edge9));
            assert!(model.errors().next().is_none(), "{op}, {base} first");
            model
                .element_map(&id("cccccccc"))
                .unwrap()
                .iter()
                .map(|(_, names)| names[0].to_string())
                .filter(|name| !name.contains("(G "))
                .collect()
        };
        assert_eq!(
            names("aaaaaaaa", "bbbbbbbb"),
            names("bbbbbbbb", "aaaaaaaa"),
            "{op}"
        );
        let rim = |first: &str, second: &str| format!("cccccccc.{op}.Edge(G {first}, G {second})");
        let (top, side) = ("aaaaaaaa.box.Face6", "bbbbbbbb.cylinder.Face1");
        let swapped = publish(
            &engine,
            &boolean_of(op, "bbbbbbbb", "aaaaaaaa", &rim(top, side)),
        );
        let (parameter, candidates) = broken(&swapped, "ffffffff");
        assert_eq!(parameter.as_deref(), Some("edges.0"));
        assert_eq!(
            candidates.first(),
            Some(&rim(side, top)),
            "{op}: {candidates:?}"
        );
    }
}
