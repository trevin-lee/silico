//! break-it: the edges a failed fillet names (M2.0d, `Refusal::EdgesFailed`), read from
//! OCCT's faulty contours and corners. Every expectation is a hand calculation on an
//! axis-aligned box: a fillet of radius r on an edge cuts into the r × r strip along it, so
//! it runs past one of its faces when r is over that face's width, and two fillets that
//! share no corner collide when their strips overlap (two parallel edges on a face w wide
//! when 2r > w). An edge is at fault when one of those holds for it; the refusal's
//! suggestion is to leave the edges it names out, so it must name only edges at fault.
//! Box edges are numbered as FreeCAD 1.1.3 numbers `Part.makeBox(a, b, c).Edges`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/kernel.rs"]
mod kernel;
#[path = "common/oracle.rs"]
mod oracle;

use kernel::kernel;
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::kernel::{Blob, ShapeData};
use silico_kernel::{
    ChamferSize, KernelError, KernelErrorKind, Primitive, Refusal, chamfer, fillet, from_brep,
    from_text_brep, make, make_box, to_brep, translate,
};

/// Each box edge: the axis it runs along, and its place across the other two axes (in
/// increasing axis order), 0 at the low face and 1 at the high. From FreeCAD 1.1.3:
/// `Part.makeBox(10, 20, 30).Edges[0]` runs from (0, 0, 30) to (0, 0, 0), and so on.
const EDGES: [(usize, [u8; 2]); 12] = [
    (2, [0, 0]), // Edge1: along z, x = 0, y = 0
    (1, [0, 1]), // Edge2: along y, x = 0, z = c
    (2, [0, 1]), // Edge3: along z, x = 0, y = b
    (1, [0, 0]), // Edge4: along y, x = 0, z = 0
    (2, [1, 0]), // Edge5: along z, x = a, y = 0
    (1, [1, 1]), // Edge6: along y, x = a, z = c
    (2, [1, 1]), // Edge7: along z, x = a, y = b
    (1, [1, 0]), // Edge8: along y, x = a, z = 0
    (0, [0, 0]), // Edge9: along x, y = 0, z = 0
    (0, [0, 1]), // Edge10: along x, y = 0, z = c
    (0, [1, 0]), // Edge11: along x, y = b, z = 0
    (0, [1, 1]), // Edge12: along x, y = b, z = c
];

/// The strip a fillet of radius `r` on box edge `e` cuts into: an interval on each axis.
fn strip(dims: [f64; 3], e: u32, r: f64) -> [(f64, f64); 3] {
    let (along, sides) = EDGES[(e - 1) as usize];
    let mut sides = sides.iter();
    let mut out = [(0.0, 0.0); 3];
    for (axis, slot) in out.iter_mut().enumerate() {
        *slot = if axis == along {
            (0.0, dims[axis])
        } else if *sides.next().unwrap() == 0 {
            (0.0, r)
        } else {
            (dims[axis] - r, dims[axis])
        };
    }
    out
}

/// The box corners (0 or 1 on each axis) edge `e` ends at.
fn ends(e: u32) -> [[u8; 3]; 2] {
    let (along, sides) = EDGES[(e - 1) as usize];
    let mut sides = sides.iter();
    let mut low = [0u8; 3];
    for (axis, slot) in low.iter_mut().enumerate() {
        if axis != along {
            *slot = *sides.next().unwrap();
        }
    }
    let mut high = low;
    high[along] = 1;
    [low, high]
}

/// Whether the fillet of `e` alone runs past one of its faces (by more than `m`); None
/// within `m` of the boundary.
fn runs_past(dims: [f64; 3], e: u32, r: f64, m: f64) -> Option<bool> {
    let along = EDGES[(e - 1) as usize].0;
    let mut near = false;
    for (axis, &w) in dims.iter().enumerate() {
        if axis == along {
            continue;
        }
        if r > w + m {
            return Some(true);
        }
        near |= (r - w).abs() <= m;
    }
    if near { None } else { Some(false) }
}

/// Whether `e` is at fault among `given` at radius `r`: its fillet runs past one of its
/// faces, or its strip overlaps (by more than `m`) that of another given edge it shares no
/// corner with. None within `m` of either boundary.
fn at_fault(dims: [f64; 3], given: &[u32], e: u32, r: f64, m: f64) -> Option<bool> {
    let mut near = match runs_past(dims, e, r, m) {
        Some(true) => return Some(true),
        Some(false) => false,
        None => true,
    };
    let mine = strip(dims, e, r);
    for &other in given {
        if other == e || ends(other).iter().any(|c| ends(e).contains(c)) {
            continue;
        }
        let theirs = strip(dims, other, r);
        let depth = (0..3)
            .map(|a| mine[a].1.min(theirs[a].1) - mine[a].0.max(theirs[a].0))
            .fold(f64::INFINITY, f64::min);
        if depth > m {
            return Some(true);
        }
        near |= depth.abs() <= m;
    }
    if near { None } else { Some(false) }
}

/// The refusal of an operation expected to fail.
fn failed(result: Result<silico_kernel::Built, KernelError>) -> KernelError {
    match result {
        Ok(_) => panic!("built"),
        Err(e) => e,
    }
}

/// The edges a failed fillet names, if it failed naming some.
fn named(err: &KernelError) -> Option<Vec<u32>> {
    match err.refusal() {
        Refusal::EdgesFailed { edges, .. } => Some(edges.clone()),
        _ => None,
    }
}

/// The box 5.25 × 2.85 × 5. Its top face (z = 5) is 2.85 wide across y, so at r = 1.45 the
/// fillets of Edge10 and Edge12, the top face's two edges along x, take 2.9 of it and
/// collide. Edge2 and Edge6, its edges along y, take 2.9 of 5.25 across x and 1.45 of their
/// 5-high sides: neither runs past a face, and each shares a corner with both of the others.
const TOP_RING: ([f64; 3], [u32; 4], f64) = ([5.25, 2.85, 5.0], [2, 6, 10, 12], 1.45);

#[test]
fn a_top_face_too_narrow_for_two_fillets_names_one_of_them_not_a_bystander() {
    let (dims, edges, r) = TOP_RING;
    let b = make_box(dims[0], dims[1], dims[2]).unwrap();
    // The hand calculation, and OCCT agreeing with it: without Edge12 (or Edge10) the rest
    // fillets; without Edge2 it still fails.
    assert_eq!(at_fault(dims, &edges, 2, r, 1e-3), Some(false));
    assert_eq!(at_fault(dims, &edges, 6, r, 1e-3), Some(false));
    assert_eq!(at_fault(dims, &edges, 10, r, 1e-3), Some(true));
    assert_eq!(at_fault(dims, &edges, 12, r, 1e-3), Some(true));
    assert!(fillet(&b, &[2, 6, 10], r).is_ok());
    assert!(fillet(&b, &[2, 6, 12], r).is_ok());
    assert!(fillet(&b, &[6, 10, 12], r).is_err());
    let err = failed(fillet(&b, &edges, r));
    // Named [2] once: OCCT's intersection check between two contours
    // (ChFi3d_Builder::Compute, ChFi3d_StripeEdgeInter) appends only the first contour of
    // the pair it checks to its faulty list, and Edge2's contour is first. The refusal then
    // said "OCCT couldn't fillet Edge2 ... leave the edge out", which doesn't help (Edge6,
    // Edge10, Edge12 still fail) and costs a fillet that builds. Edges at fault, or none.
    let named = named(&err).unwrap_or_default();
    assert!(
        named.iter().all(|e| [10, 12].contains(e)),
        "names {named:?}, of which only Edge10 and Edge12 are at fault: {}",
        err.message()
    );
}

#[test]
fn following_the_advice_on_the_top_face_drops_no_fillet_that_builds() {
    // Leaving out the edges each refusal names, until the fillet builds, should cost only
    // edges at fault: one of Edge10 and Edge12 (TOP_RING).
    let (dims, edges, r) = TOP_RING;
    let b = make_box(dims[0], dims[1], dims[2]).unwrap();
    let mut left = edges.to_vec();
    let mut dropped = Vec::new();
    for _ in 0..edges.len() {
        let Err(err) = fillet(&b, &left, r) else {
            break;
        };
        // A refusal naming none says to find the edge one at a time: the advice ends here.
        let Some(named) = named(&err) else {
            break;
        };
        left.retain(|e| !named.contains(e));
        dropped.extend(named);
    }
    // Dropped [2, 12] once; Edge2's fillet builds with Edge6 and Edge10.
    assert!(
        dropped.len() <= 1 && dropped.iter().all(|e| [10, 12].contains(e)),
        "dropped {dropped:?}"
    );
}

#[test]
fn a_cube_filleted_all_round_at_half_its_side_names_its_edges_alike() {
    // Every edge of a 10 mm cube at r = 5: on every face, the fillets of each pair of
    // parallel edges take exactly its width. The cube's symmetry takes any edge to any
    // other, so a refusal from the geometry names all twelve or none.
    let cube = make_box(10.0, 10.0, 10.0).unwrap();
    let all: Vec<u32> = (1..=12).collect();
    let err = failed(fillet(&cube, &all, 5.0));
    assert_eq!(err.kind(), KernelErrorKind::Failed, "{err:?}");
    let named = named(&err).unwrap_or_default();
    // Named [1] alone once, the first edge given (OCCT's first contour of a colliding pair).
    // Leaving out each edge named in turn took nine more refusals, naming one edge each, to
    // reach a fillet that builds, of only Edge7, Edge8, and Edge9.
    assert!(
        named.is_empty() || named == all,
        "names {named:?} of the cube's twelve alike edges"
    );
}

/// A box's sides, an edge set, and a radius.
fn box_case() -> impl Strategy<Value = ([f64; 3], Vec<u32>, f64)> {
    (
        [1.0..10.0f64, 1.0..10.0f64, 1.0..10.0f64],
        1u32..4096,
        0.2..6.0f64,
    )
        .prop_map(|(dims, mask, r)| {
            let edges = (1..=12).filter(|e| mask & (1 << (e - 1)) != 0).collect();
            (dims, edges, r)
        })
}

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 1000,
        failure_persistence: None,
        // Fixed, so a red run is red every time.
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x5ea1_f111),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// A refusal naming edges names some, each given, once, in increasing order (what the
    /// core's `refusal_fits` demands, or the worker is stopped), and among them every edge
    /// whose fillet runs past a face whatever else is given. The listed order doesn't
    /// change it.
    #[test]
    fn a_failed_fillet_names_every_edge_that_runs_past_its_face(
        (dims, edges, r) in box_case()
    ) {
        let m = 1e-3;
        prop_assume!(edges.iter().all(|&e| at_fault(dims, &edges, e, r, m).is_some()));
        let b = make_box(dims[0], dims[1], dims[2]).unwrap();
        let Err(err) = fillet(&b, &edges, r) else { return Ok(()); };
        let Some(named) = named(&err) else { return Ok(()); };
        prop_assert!(!named.is_empty());
        prop_assert!(named.windows(2).all(|p| p[0] < p[1]), "{:?}", named);
        prop_assert!(named.iter().all(|e| edges.contains(e)), "{:?} of {:?}", named, edges);
        for &e in &edges {
            if runs_past(dims, e, r, m) == Some(true) {
                prop_assert!(named.contains(&e), "Edge{} runs past its face; names {:?}", e, named);
            }
        }
        let reversed: Vec<u32> = edges.iter().rev().copied().collect();
        let again = failed(fillet(&b, &reversed, r));
        prop_assert_eq!(again.refusal(), err.refusal());
    }

    /// Every edge a failed fillet names is at fault: its fillet runs past a face, or
    /// collides with another given edge's.
    #[test]
    fn a_failed_fillet_names_only_edges_at_fault((dims, edges, r) in box_case()) {
        let m = 1e-3;
        prop_assume!(edges.iter().all(|&e| at_fault(dims, &edges, e, r, m).is_some()));
        let b = make_box(dims[0], dims[1], dims[2]).unwrap();
        let Err(err) = fillet(&b, &edges, r) else { return Ok(()); };
        let Some(named) = named(&err) else { return Ok(()); };
        // OCCT's report of a collision names the first contour of the pair, which may be an
        // edge that collides with nothing (see
        // a_top_face_too_narrow_for_two_fillets_names_one_of_them_not_a_bystander): once
        // named, about one failed fillet in a hundred on boxes.
        for &e in &named {
            prop_assert_eq!(
                at_fault(dims, &edges, e, r, m), Some(true),
                "names Edge{} among {:?} (named {:?}), which neither runs past a face nor collides",
                e, edges, named
            );
        }
    }
}

#[test]
fn the_same_failed_fillet_names_the_same_edges_in_process_and_through_workers() {
    // A 10 × 10 × 2 slab at r = 2.5: the eight edges beside its 2 mm sides fail.
    let (edges, r) = ([2, 4, 6, 8, 9, 10, 11, 12], 2.5);
    let b = make_box(10.0, 10.0, 2.0).unwrap();
    let first = failed(fillet(&b, &edges, r));
    for _ in 0..3 {
        assert_eq!(failed(fillet(&b, &edges, r)), first);
    }
    let elements: Vec<String> = first.elements().to_vec();
    assert!(!elements.is_empty(), "{first:?}");
    let shape = ShapeData::unchecked(Blob::inline(to_brep(&b).unwrap()));
    let mut reversed = edges;
    reversed.reverse();
    for _ in 0..2 {
        let mut k = kernel();
        let err = k.fillet(&shape, &reversed, r).unwrap_err();
        assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
        assert_eq!(err.message(), "Fillet failed", "{err}");
        assert_eq!(err.elements(), elements, "{err}");
        assert!(err.suggestion().is_some(), "{err}");
        // The worker carries on: the vertical edges fillet.
        let built = k.fillet(&shape, &[1, 3, 5, 7], r).unwrap();
        assert!(k.summarize(&built.shape).unwrap().valid);
    }
}

#[test]
fn a_chamfer_too_wide_for_its_face_names_no_edge() {
    // The same collision chamfered (2 × 1.45 of the 2.85 top face): OCCT keeps a chamfer's
    // report private, so the refusal names none of several edges, never a guess.
    let (dims, edges, _) = TOP_RING;
    let b = make_box(dims[0], dims[1], dims[2]).unwrap();
    let size = ChamferSize::Equal { distance: 1.45 };
    let err = failed(chamfer(&b, &edges, size, false));
    assert!(
        matches!(err.refusal(), Refusal::Occt { edges: given, .. } if given == &edges),
        "{err:?}"
    );
    assert!(err.elements().is_empty(), "{err:?}");
    let shape = ShapeData::unchecked(Blob::inline(to_brep(&b).unwrap()));
    let err = kernel().chamfer(&shape, &edges, size, false).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert!(err.elements().is_empty(), "{err}");
}

#[test]
fn a_fillet_past_its_face_by_any_margin_names_that_edge_alone() {
    // Edge1 of the 10 × 20 × 30 box lies beside a face 10 wide; Edge11, beside faces 20 and
    // 30 wide, shares no face with it and their strips don't meet (2r < 20). Past 10, by
    // one ULP or by a millimetre, only Edge1 is at fault.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    for r in [10.0f64.next_up(), 10.000_000_1, 10.001, 11.0, 19.9] {
        let err = failed(fillet(&b, &[1, 11], r));
        assert_eq!(named(&err), Some(vec![1]), "r = {r:e}: {err:?}");
        assert_eq!(err.elements(), ["Edge1"]);
        assert!(fillet(&b, &[11], r).is_ok(), "r = {r:e}");
    }
    assert!(fillet(&b, &[1, 11], 9.999).is_ok());
}

#[test]
fn a_corner_on_a_moved_shape_names_the_same_edges() {
    // The pyramid of fillet.rs, whose apex OCCT reports it couldn't blend: the corner is
    // found among the shape's vertices by identity, location included. Moved (a located
    // shape), moved back (two locations), and moved and read back from bytes, it is the
    // same corner and the same edges, which include every edge FreeCAD finds at the apex.
    let pyramid = "Part.makeWedge(0, 0, 0, 2, 2, 4, 20, 4, 2, 2)";
    let at_apex: Vec<String> = serde_json::from_value(oracle::freecad(&format!(
        "import json, Part, FreeCAD\ns = {pyramid}\n\
         print('SILICO_ORACLE ' + json.dumps(['Edge%d' % (i + 1) for i, e in enumerate(s.Edges) \
         if any((v.Point - FreeCAD.Vector(2, 20, 2)).Length < 1e-9 for v in e.Vertexes)]))\n"
    )))
    .unwrap();
    assert_eq!(at_apex.len(), 4);
    let text: String = serde_json::from_value(oracle::freecad(&format!(
        "import json, Part\nprint('SILICO_ORACLE ' + json.dumps({pyramid}.exportBrepToString()))\n"
    )))
    .unwrap();
    let shape = from_text_brep(text.as_bytes()).unwrap();
    let all: Vec<u32> = (1..=8).collect();
    let here = failed(fillet(&shape, &all, 2.0));
    for edge in &at_apex {
        assert!(here.elements().contains(edge), "{edge}: {here:?}");
    }
    let moved = translate(&shape, [1000.0, -7.0, 3.0]).unwrap().shape;
    let back = translate(&moved, [-1000.0, 7.0, -3.0]).unwrap().shape;
    let read = from_brep(&to_brep(&moved).unwrap()).unwrap();
    for (what, s) in [("moved", &moved), ("moved back", &back), ("read", &read)] {
        let err = failed(fillet(s, &all, 2.0));
        assert_eq!(err.refusal(), here.refusal(), "{what}");
    }
}

#[test]
fn a_fillet_too_large_for_a_cylinder_names_both_rims() {
    // A cylinder 5 in radius and 4 high: its rims are Edge2 and Edge3 (Edge1 is its seam).
    // At r = 4 the rims' fillets take 8 of the 4-high side; at 5.5 each runs past the
    // 5-wide caps too. Below 2 the two fit (2r < 4).
    let cylinder = make(&Primitive::Cylinder {
        radius: 5.0,
        height: 4.0,
    })
    .unwrap();
    for r in [4.0, 5.5] {
        let err = failed(fillet(&cylinder, &[2, 3], r));
        assert_eq!(err.elements(), ["Edge2", "Edge3"], "r = {r}: {err:?}");
    }
    assert!(fillet(&cylinder, &[2, 3], 1.9).is_ok());
}
