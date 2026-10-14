//! break-it: the edges a failed chamfer names (`Refusal::EdgesFailed`), read from OCCT's
//! report through Silico's OCCT patch 0002: two distances, distance and angle, flipped,
//! tangent chains, curved faces, and collisions at corner stripes. Every expectation is a
//! hand calculation: on an axis-aligned box, a chamfer of an edge reaches `distance` across
//! its reference face (the lower-numbered of its two, or the higher when flipped) and
//! `other` (or `distance` tan(angle)) across the other face, so it runs past a face when a
//! reach is over that face's width, and two chamfers that share no corner collide when
//! their reach rectangles overlap. Box faces are numbered as FreeCAD 1.1.3 numbers
//! `Part.makeBox(a, b, c).Faces` (x = 0, x = a, y = 0, y = b, z = 0, z = c), so an edge's
//! lower-numbered face is the one across the lower of its two cross axes. Edges are
//! numbered as `Part.makeBox(a, b, c).Edges` (see adversarial_fillet.rs).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/chamfer_sizes.rs"]
mod chamfer_sizes;
#[path = "common/kernel.rs"]
mod kernel;

use chamfer_sizes::{angled, equal, two};
use kernel::kernel;
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_kernel::{
    Built, ChamferSize, KernelError, KernelErrorKind, Primitive, Refusal, chamfer, fillet, make,
    make_box, summarize, tessellate,
};

/// Each box edge: the axis it runs along, and its place across the other two axes (in
/// increasing axis order), 0 at the low face and 1 at the high.
const EDGES: [(usize, [u8; 2]); 12] = [
    (2, [0, 0]), // Edge1
    (1, [0, 1]), // Edge2
    (2, [0, 1]), // Edge3
    (1, [0, 0]), // Edge4
    (2, [1, 0]), // Edge5
    (1, [1, 1]), // Edge6
    (2, [1, 1]), // Edge7
    (1, [1, 0]), // Edge8
    (0, [0, 0]), // Edge9
    (0, [0, 1]), // Edge10
    (0, [1, 0]), // Edge11
    (0, [1, 1]), // Edge12
];

/// tan of the angles a distance-and-angle case draws from, in square roots.
fn exact_tan(degrees: f64) -> f64 {
    let (r2, r3) = (2f64.sqrt(), 3f64.sqrt());
    match degrees {
        15.0 => 2.0 - r3,
        22.5 => r2 - 1.0,
        30.0 => 1.0 / r3,
        45.0 => 1.0,
        60.0 => r3,
        67.5 => r2 + 1.0,
        75.0 => 2.0 + r3,
        _ => panic!("no exact tan for {degrees}"),
    }
}

/// How far a chamfer of `size` on box edge `e` reaches along each of the edge's two cross
/// axes (p, q, p < q): the reference face (across p's face, or q's when flipped) takes
/// `distance` along the other cross axis.
fn reach(e: u32, size: ChamferSize, flip: bool) -> [(usize, f64); 2] {
    let along = EDGES[(e - 1) as usize].0;
    let cross: Vec<usize> = (0..3).filter(|&a| a != along).collect();
    let (p, q) = (cross[0], cross[1]);
    let (on_reference, on_other) = match size {
        ChamferSize::Equal { distance } => (distance, distance),
        ChamferSize::TwoDistances { distance, other } => (distance, other),
        ChamferSize::DistanceAngle { distance, angle } => (distance, distance * exact_tan(angle)),
    };
    // The reference face is the one across p (unflipped): it spans q, so `distance` is a
    // reach along q, and the other face's reach is along p.
    if flip {
        [(p, on_reference), (q, on_other)]
    } else {
        [(q, on_reference), (p, on_other)]
    }
}

/// The block a chamfer on box edge `e` cuts into: an interval on each axis.
fn strip(dims: [f64; 3], e: u32, size: ChamferSize, flip: bool) -> [(f64, f64); 3] {
    let (along, sides) = EDGES[(e - 1) as usize];
    let cross: Vec<usize> = (0..3).filter(|&a| a != along).collect();
    let mut out = [(0.0, 0.0); 3];
    out[along] = (0.0, dims[along]);
    for (axis, r) in reach(e, size, flip) {
        let side = sides[cross.iter().position(|&a| a == axis).unwrap()];
        out[axis] = if side == 0 {
            (0.0, r)
        } else {
            (dims[axis] - r, dims[axis])
        };
    }
    out
}

/// The box corners edge `e` ends at.
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

/// Whether the chamfer of `e` alone runs past one of its faces (by more than `m`); None
/// within `m` of it.
fn runs_past(dims: [f64; 3], e: u32, size: ChamferSize, flip: bool, m: f64) -> Option<bool> {
    let mut near = false;
    for (axis, r) in reach(e, size, flip) {
        if r > dims[axis] + m {
            return Some(true);
        }
        near |= (r - dims[axis]).abs() <= m;
    }
    if near { None } else { Some(false) }
}

/// Whether `e` is at fault among `given`: it runs past a face, or its block overlaps (by
/// more than `m`) that of another given edge it shares no corner with. None near either.
fn at_fault(
    dims: [f64; 3],
    given: &[u32],
    e: u32,
    size: ChamferSize,
    flip: bool,
    m: f64,
) -> Option<bool> {
    let mut near = match runs_past(dims, e, size, flip, m) {
        Some(true) => return Some(true),
        Some(false) => false,
        None => true,
    };
    let mine = strip(dims, e, size, flip);
    for &other in given {
        if other == e || ends(other).iter().any(|c| ends(e).contains(c)) {
            continue;
        }
        let theirs = strip(dims, other, size, flip);
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

fn failed(result: Result<Built, KernelError>) -> KernelError {
    match result {
        Ok(_) => panic!("built"),
        Err(e) => e,
    }
}

/// The edges a failed chamfer names, if it names some.
fn named(err: &KernelError) -> Option<Vec<u32>> {
    match err.refusal() {
        Refusal::EdgesFailed { edges, .. } => Some(edges.clone()),
        _ => None,
    }
}

/// A box's sides, an edge set, a chamfer size of two distances or a distance and angle,
/// and whether it's flipped.
fn case() -> impl Strategy<Value = ([f64; 3], Vec<u32>, ChamferSize, bool)> {
    let size = prop_oneof![
        (0.2..6.0f64, 0.2..6.0f64).prop_map(|(d, o)| two(d, o)),
        // Skewed: one reach far bigger than the other, so one face is run past and the
        // other isn't.
        (0.2..2.0f64, 3.0..12.0f64).prop_map(|(d, o)| two(d, o)),
        (
            0.2..6.0f64,
            prop::sample::select(vec![15.0, 22.5, 30.0, 45.0, 60.0, 67.5, 75.0])
        )
            .prop_map(|(d, a)| angled(d, a)),
    ];
    (
        [1.0..10.0f64, 1.0..10.0f64, 1.0..10.0f64],
        1u32..4096,
        size,
        any::<bool>(),
    )
        .prop_map(|(dims, mask, size, flip)| {
            let edges = (1..=12).filter(|e| mask & (1 << (e - 1)) != 0).collect();
            (dims, edges, size, flip)
        })
}

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 1000,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0xc4a3_f3e2),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// A failed chamfer of two distances or a distance and angle, flipped or not, names
    /// every given edge whose chamfer runs past one of its faces; and what it names is
    /// given, once each, in increasing order.
    #[test]
    fn a_failed_unequal_chamfer_names_every_edge_that_runs_past_its_face(
        (dims, edges, size, flip) in case()
    ) {
        let m = 1e-3;
        prop_assume!(edges.iter().all(|&e| at_fault(dims, &edges, e, size, flip, m).is_some()));
        let b = make_box(dims[0], dims[1], dims[2]).unwrap();
        let past: Vec<u32> = edges
            .iter()
            .copied()
            .filter(|&e| runs_past(dims, e, size, flip, m) == Some(true))
            .collect();
        let result = chamfer(&b, &edges, size, flip);
        prop_assert!(past.is_empty() || result.is_err(), "{:?} {:?} flip {} built", edges, size, flip);
        let Err(err) = result else { return Ok(()); };
        let named = named(&err);
        prop_assert!(
            past.is_empty() || named.is_some(),
            "{:?} on {:?} {:?} flip {}: {:?} run past, names none: {:?}",
            edges, dims, size, flip, past, err
        );
        let Some(named) = named else { return Ok(()); };
        prop_assert!(!named.is_empty());
        prop_assert!(named.windows(2).all(|p| p[0] < p[1]), "{:?}", named);
        prop_assert!(named.iter().all(|e| edges.contains(e)), "{:?} of {:?}", named, edges);
        for &e in &past {
            prop_assert!(
                named.contains(&e),
                "Edge{} runs past its face ({:?} on {:?}, {:?} flip {}); names {:?}",
                e, edges, dims, size, flip, named
            );
        }
    }

    /// Every edge a failed unequal chamfer names is at fault: it runs past a face, or
    /// collides with another given edge's chamfer.
    #[test]
    fn a_failed_unequal_chamfer_names_only_edges_at_fault((dims, edges, size, flip) in case()) {
        let m = 1e-3;
        prop_assume!(edges.iter().all(|&e| at_fault(dims, &edges, e, size, flip, m).is_some()));
        let b = make_box(dims[0], dims[1], dims[2]).unwrap();
        let Err(err) = chamfer(&b, &edges, size, flip) else { return Ok(()); };
        let Some(named) = named(&err) else { return Ok(()); };
        for &e in &named {
            prop_assert_eq!(
                at_fault(dims, &edges, e, size, flip, m),
                Some(true),
                "names Edge{} among {:?} on {:?} ({:?} flip {}; named {:?}), which neither runs past a face nor collides",
                e, edges, dims, size, flip, named
            );
        }
    }
}

// ---- Targeted ----

#[test]
fn the_hand_calculation_puts_distance_on_the_lower_numbered_face() {
    // Edge1 of 10 × 20 × 30 (along z at x = 0, y = 0) lies on Face1 (x = 0, 20 wide across
    // y) and Face3 (y = 0, 10 wide across x). two(15, 5) puts 15 across Face1: it builds,
    // removing the 15 × 5 triangle over 30; flipped, 15 runs past Face3's 10 and fails.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    assert_eq!(
        runs_past([10.0, 20.0, 30.0], 1, two(15.0, 5.0), false, 1e-3),
        Some(false)
    );
    assert_eq!(
        runs_past([10.0, 20.0, 30.0], 1, two(15.0, 5.0), true, 1e-3),
        Some(true)
    );
    let built = chamfer(&b, &[1], two(15.0, 5.0), false).unwrap();
    let s = summarize(&built.shape).unwrap();
    assert!(s.valid);
    assert!(
        (6000.0 - s.volume - 15.0 * 5.0 / 2.0 * 30.0).abs() < 1e-6,
        "{}",
        s.volume
    );
    let err = failed(chamfer(&b, &[1], two(15.0, 5.0), true));
    assert_eq!(err.element(), Some("Edge1"), "{err:?}");
}

#[test]
fn a_flipped_chamfer_names_the_edge_its_flip_runs_past() {
    // Edge1 and Edge7 (the two vertical edges at opposite corners of 10 × 20 × 30), and
    // Edge9 (along x, y = 0, z = 0), at two(14, 5) flipped. Edge1 and Edge7 lie on an x-face
    // (20 wide across y) and a y-face (10 wide across x): flipped, 14 goes across the y-face,
    // past its 10. Edge9 lies on Face3 (y = 0, 30 tall) and Face5 (z = 0, 20 across y):
    // flipped, 14 across Face5's 20, 5 up Face3's 30: fine, and its block (y below 14) clears
    // Edge7's (y above 15). Expected: Edge1 and Edge7.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let dims = [10.0, 20.0, 30.0];
    for e in [1, 7] {
        assert_eq!(runs_past(dims, e, two(14.0, 5.0), true, 1e-3), Some(true));
    }
    assert_eq!(
        at_fault(dims, &[1, 7, 9], 9, two(14.0, 5.0), true, 1e-3),
        Some(false)
    );
    assert!(chamfer(&b, &[9], two(14.0, 5.0), true).is_ok());
    let err = failed(chamfer(&b, &[1, 7, 9], two(14.0, 5.0), true));
    assert_eq!(err.elements(), ["Edge1", "Edge7"], "{err:?}");
    // Unflipped, the same edges build.
    assert!(chamfer(&b, &[1, 7, 9], two(14.0, 5.0), false).is_ok());
}

#[test]
fn a_distance_and_angle_whose_far_reach_runs_past_names_that_edge() {
    // Edge1 of 10 × 20 × 30 at angled(6, 60): 6 across Face1 (20 wide), 6 tan 60° = 10.39
    // across Face3 (10 wide): past. Edge7 at the same: the same faces' kinds, past too.
    // Edge12 (along x at y = 20, z = 30) lies on Face4 (y = 20, 30 tall) and Face6 (z = 30,
    // 20 across y): 6 down Face4, 10.39 across Face6's 20: fine.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let dims = [10.0, 20.0, 30.0];
    assert_eq!(
        runs_past(dims, 1, angled(6.0, 60.0), false, 1e-3),
        Some(true)
    );
    assert_eq!(
        runs_past(dims, 12, angled(6.0, 60.0), false, 1e-3),
        Some(false)
    );
    assert!(chamfer(&b, &[12], angled(6.0, 60.0), false).is_ok());
    let err = failed(chamfer(&b, &[1, 12], angled(6.0, 60.0), false));
    assert_eq!(err.elements(), ["Edge1"], "{err:?}");
    assert_eq!(err.kind(), KernelErrorKind::Failed);
}

/// The number of the edge of `shape` whose mesh points all satisfy `on`, of those with
/// more than one point.
fn edge_where(shape: &silico_kernel::Shape, on: impl Fn([f64; 3]) -> bool) -> u32 {
    let mesh = tessellate(shape, 0.01).unwrap();
    let found: Vec<u32> = mesh
        .edges()
        .enumerate()
        .filter(|(_, points)| points.len() > 1 && points.iter().all(|&p| on(p)))
        .map(|(i, _)| i as u32 + 1)
        .collect();
    assert_eq!(found.len(), 1, "edges matching: {found:?}");
    found[0]
}

/// The number of the one face of `shape` whose mesh points all satisfy `on`.
fn face_where(shape: &silico_kernel::Shape, on: impl Fn([f64; 3]) -> bool) -> u32 {
    let mesh = tessellate(shape, 0.01).unwrap();
    let mut triangles = mesh.triangles.iter();
    let mut found = Vec::new();
    for (i, &count) in mesh.face_triangles.iter().enumerate() {
        let mut all = true;
        for t in triangles.by_ref().take(count as usize) {
            all &= t.iter().all(|&v| on(mesh.positions[v as usize]));
        }
        if all {
            found.push(i as u32 + 1);
        }
    }
    assert_eq!(found.len(), 1, "faces matching: {found:?}");
    found[0]
}

#[test]
fn a_cylinders_rims_chamfered_past_its_side_are_both_named() {
    // A cylinder of radius 2 and height 10, both rims at 11: each runs 11 down a side 10
    // tall, past it.
    let cyl = make(&Primitive::Cylinder {
        radius: 2.0,
        height: 10.0,
    })
    .unwrap();
    let top = edge_where(&cyl, |p| (p[2] - 10.0).abs() < 1e-9);
    let bottom = edge_where(&cyl, |p| p[2].abs() < 1e-9);
    let mut both = vec![top, bottom];
    both.sort();
    let err = failed(chamfer(&cyl, &both, equal(11.0), false));
    assert_eq!(named(&err), Some(both), "{err:?}");
}

#[test]
fn a_hole_rim_chamfered_past_the_face_around_it_is_named_and_a_box_edge_isnt() {
    // A 10 mm cube with a hole of radius 2 through its middle along z: around each rim the
    // flat face is 3 wide at its narrowest (5 to the cube's side, less the radius), and the
    // hole 10 deep. At two(3.5, 1), the rim's reference face takes 3.5: across the flat
    // face, past it; down the hole, fine (and 1 across the flat face fits). Edge1 (a
    // vertical cube edge, 3.5 and 1 across faces 10 wide) fits either way. So exactly the
    // rims are at fault when their reference face is the flat one, and none otherwise.
    let cube = make_box(10.0, 10.0, 10.0).unwrap();
    let pin = make(&Primitive::Cylinder {
        radius: 2.0,
        height: 12.0,
    })
    .unwrap();
    let pin = silico_kernel::translate(&pin, [5.0, 5.0, -1.0])
        .unwrap()
        .shape;
    let holed = silico_kernel::boolean(silico_kernel::BooleanOp::Cut, &cube, &pin)
        .unwrap()
        .shape;
    let on_rim = |p: [f64; 3], z: f64| {
        (p[2] - z).abs() < 1e-9
            && (((p[0] - 5.0) * (p[0] - 5.0) + (p[1] - 5.0) * (p[1] - 5.0)).sqrt() - 2.0).abs()
                < 1e-6
    };
    let top = edge_where(&holed, |p| on_rim(p, 10.0));
    let bottom = edge_where(&holed, |p| on_rim(p, 0.0));
    let edge1 = edge_where(&holed, |p| p[0].abs() < 1e-9 && p[1].abs() < 1e-9);
    let top_face = face_where(&holed, |p| (p[2] - 10.0).abs() < 1e-9);
    let bottom_face = face_where(&holed, |p| p[2].abs() < 1e-9);
    let hole = face_where(&holed, |p| {
        (((p[0] - 5.0) * (p[0] - 5.0) + (p[1] - 5.0) * (p[1] - 5.0)).sqrt() - 2.0).abs() < 1e-6
    });
    let mut given = vec![edge1, top, bottom];
    given.sort();
    for flip in [false, true] {
        // Each rim's reference face: the lower-numbered of its two, or the higher flipped.
        let flat_first = |flat: u32| (flat < hole) != flip;
        let mut want: Vec<u32> = [(top, top_face), (bottom, bottom_face)]
            .into_iter()
            .filter(|&(_, flat)| flat_first(flat))
            .map(|(rim, _)| rim)
            .collect();
        want.sort();
        let result = chamfer(&holed, &given, two(3.5, 1.0), flip);
        if want.is_empty() {
            let built = result.unwrap_or_else(|e| panic!("flip {flip}: {e:?}"));
            assert!(summarize(&built.shape).unwrap().valid, "flip {flip}");
        } else {
            let err = failed(result);
            assert_eq!(named(&err), Some(want), "flip {flip}: {err:?}");
        }
    }
}

#[test]
fn a_tangent_chain_failing_only_at_its_far_corner_names_no_edge() {
    // A 10 × 20 × 30 box with Edge1 filleted at r = 2: the top outline runs smoothly from
    // the edge on y = 0 round the arc to the edge on x = 0, one contour. Chamfering the top
    // edge on y = 0 at 10.5: across the top from it is 20 (fine), and down its side 30; but
    // the chain follows on to the edge on x = 0, across the top from which is 10: past. A
    // bystander far from the chain, the bottom edge on y = 20 (10.5 up a side 30 tall, 10.5
    // across a bottom 20 wide), fits.
    //
    // OCCT fails it only at a corner (no faulty contour; one faulty vertex, at the chain's
    // far end), and a chamfer names no edge through its corners, which elsewhere reach edges
    // that fit (a_chamfer_failing_at_a_corner_names_none_of_the_edges_there_that_fit). So
    // the refusal names none, though the given chain is at fault, and says to chamfer the
    // edges one at a time; the same fillet, r = 10.5, is named through its contour's error
    // status. Given alone, the edge is named, as any single edge OCCT fails on is.
    let filleted = fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1], 2.0)
        .unwrap()
        .shape;
    let y0_top = edge_where(&filleted, |p| {
        p[1].abs() < 1e-9 && (p[2] - 30.0).abs() < 1e-9
    });
    let x0_top = edge_where(&filleted, |p| {
        p[0].abs() < 1e-9 && (p[2] - 30.0).abs() < 1e-9
    });
    let y20_bottom = edge_where(&filleted, |p| {
        (p[1] - 20.0).abs() < 1e-9 && p[2].abs() < 1e-9
    });
    assert!(chamfer(&filleted, &[y20_bottom], equal(10.5), false).is_ok());
    // The chain alone fails, given from either end.
    let from_y0 = failed(chamfer(&filleted, &[y0_top], equal(10.5), false));
    assert_eq!(
        from_y0.element(),
        Some(format!("Edge{y0_top}").as_str()),
        "{from_y0:?}"
    );
    assert!(chamfer(&filleted, &[x0_top], equal(10.5), false).is_err());
    let mut given = vec![y0_top, y20_bottom];
    given.sort();
    let err = failed(chamfer(&filleted, &given, equal(10.5), false));
    assert!(err.elements().is_empty(), "{err:?}");
    assert!(
        err.suggestion()
            .is_some_and(|s| s.contains("one at a time")),
        "{err:?}"
    );
}

#[test]
fn a_tangent_chain_given_from_both_ends_runs_past_as_from_either() {
    // The chain above at 10.5, given from both its straight edges, or from the edge on
    // x = 0: it still runs past the top across from that edge (10 wide), so no valid solid
    // can be the result.
    let filleted = fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1], 2.0)
        .unwrap()
        .shape;
    let y0_top = edge_where(&filleted, |p| {
        p[1].abs() < 1e-9 && (p[2] - 30.0).abs() < 1e-9
    });
    let x0_top = edge_where(&filleted, |p| {
        p[0].abs() < 1e-9 && (p[2] - 30.0).abs() < 1e-9
    });
    let mut both = vec![y0_top, x0_top];
    both.sort();
    // OCCT builds it from either the edge on x = 0 or both (not from the edge on y = 0
    // alone), into a solid that isn't valid, which is refused.
    for given in [both, vec![x0_top]] {
        let err = failed(chamfer(&filleted, &given, equal(10.5), false));
        assert!(
            matches!(
                err.refusal(),
                Refusal::ResultInvalid { .. } | Refusal::EdgesFailed { .. }
            ),
            "{given:?}: {err:?}"
        );
    }
}

#[test]
fn every_edge_of_a_cube_at_half_its_side_in_every_kind_names_its_edges_alike() {
    // A 10 mm cube, every edge, at sizes whose reaches add to its side on some faces: equal
    // 5, two(4, 6) both ways, angled(5, 45). OCCT's corner stripes collide; whatever the
    // report, the cube's symmetry takes edges to each other only for the equal kinds, so
    // for those the named set is all twelve or none. For every kind: the report is read
    // (an error, KernelFailed through the core), and the worker goes on to build the
    // next chamfer.
    let cube = make_box(10.0, 10.0, 10.0).unwrap();
    let all: Vec<u32> = (1..=12).collect();
    for size in [equal(5.0), angled(5.0, 45.0)] {
        for flip in [false, true] {
            if let Err(err) = chamfer(&cube, &all, size, flip) {
                let n = named(&err).unwrap_or_default();
                assert!(
                    n.is_empty() || n == all,
                    "{size:?} flip {flip}: names {n:?}"
                );
            }
        }
    }
    let mut k = kernel();
    let shape = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 10.0,
            dz: 10.0,
        })
        .unwrap();
    for (size, flip) in [
        (two(4.0, 6.0), false),
        (two(4.0, 6.0), true),
        (two(6.0, 4.0), false),
        (angled(5.0, 45.0), false),
        (angled(4.0, 60.0), true),
    ] {
        match k.chamfer(&shape, &all, size, flip) {
            Ok(_) => {}
            Err(err) => {
                assert_eq!(
                    err.code(),
                    ErrorCode::KernelFailed,
                    "{size:?} {flip}: {err}"
                );
                for e in err.elements() {
                    let n: u32 = e.strip_prefix("Edge").unwrap().parse().unwrap();
                    assert!(all.contains(&n));
                }
            }
        }
        assert!(
            k.chamfer(&shape, &[1], equal(1.0), false).is_ok(),
            "the worker after {size:?} {flip}"
        );
    }
}

#[test]
fn three_edges_at_one_corner_past_their_faces_name_all_three() {
    // Edge1, Edge4, Edge9 meet at the origin of 10 × 20 × 30 (vertical, along y, along x).
    // At equal(11): Edge1 runs past Face3's 10 across x; Edge4 (on Face1, x = 0, and Face5,
    // z = 0, 10 across x) runs past too; Edge9 (Face3, y = 0, 30 tall; Face5, 20 across y)
    // is fine. The corner joins all three (ChFi3d_ChBuilder's three-edge corner).
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let dims = [10.0, 20.0, 30.0];
    assert_eq!(runs_past(dims, 1, equal(11.0), false, 1e-3), Some(true));
    assert_eq!(runs_past(dims, 4, equal(11.0), false, 1e-3), Some(true));
    assert_eq!(runs_past(dims, 9, equal(11.0), false, 1e-3), Some(false));
    let err = failed(chamfer(&b, &[1, 4, 9], equal(11.0), false));
    let n = named(&err).unwrap_or_default();
    assert!(n.contains(&1) && n.contains(&4), "names {n:?}: {err:?}");
    assert!(!n.contains(&9), "names Edge9, which fits: {err:?}");
}

#[test]
fn the_same_failed_chamfer_twice_names_the_same_edges() {
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    for (size, flip) in [
        (two(15.0, 5.0), true),
        (angled(6.0, 60.0), false),
        (equal(11.0), false),
    ] {
        let all: Vec<u32> = (1..=12).collect();
        let a = failed(chamfer(&b, &all, size, flip));
        for _ in 0..3 {
            let again = failed(chamfer(&b, &all, size, flip));
            assert_eq!(again, a, "{size:?} {flip}");
        }
    }
}

#[test]
fn a_concave_edge_chamfered_past_its_faces_is_named_and_a_convex_one_isnt() {
    // An L: the square 20 × 20 less its corner square above (8, 8), extruded 10. Its
    // concave edge, at (8, 8), lies between faces 12 wide (from 8 to 20, each way); the
    // convex edge at the origin between faces 20 wide, whose chamfer (x + y < d) stays clear
    // of the notch while d < 16. At 13 the concave edge's chamfer runs past both its faces
    // (at two(13, 1), past one); the convex one fits, far from it. At 11 both fit.
    let l = silico_kernel::Profile {
        points: vec![
            [0.0, 0.0, 0.0],
            [20.0, 0.0, 0.0],
            [20.0, 8.0, 0.0],
            [8.0, 8.0, 0.0],
            [8.0, 20.0, 0.0],
            [0.0, 20.0, 0.0],
        ],
    };
    let shape = silico_kernel::extrude(&l, 10.0, None).unwrap().shape;
    let concave = edge_where(&shape, |p| {
        (p[0] - 8.0).abs() < 1e-9 && (p[1] - 8.0).abs() < 1e-9
    });
    let convex = edge_where(&shape, |p| p[0].abs() < 1e-9 && p[1].abs() < 1e-9);
    let mut given = vec![concave, convex];
    given.sort();
    assert!(chamfer(&shape, &given, equal(11.0), false).is_ok());
    assert!(chamfer(&shape, &[convex], equal(13.0), false).is_ok());
    let err = failed(chamfer(&shape, &given, equal(13.0), false));
    assert_eq!(err.elements(), [format!("Edge{concave}")], "{err:?}");
    for flip in [false, true] {
        assert!(chamfer(&shape, &[convex], two(13.0, 1.0), flip).is_ok());
        let err = failed(chamfer(&shape, &given, two(13.0, 1.0), flip));
        assert_eq!(
            err.elements(),
            [format!("Edge{concave}")],
            "flip {flip}: {err:?}"
        );
    }
}

#[test]
fn a_named_unequal_chamfer_reaches_the_core_and_the_worker_goes_on() {
    // Through the core (whose check that a refusal fits its request stops the worker if
    // not): the flipped two-distance chamfer of a_flipped_chamfer_names_the_edge_its_flip_
    // runs_past, and a distance-and-angle, each named; after each, the worker builds.
    let mut k = kernel();
    let shape = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    for (edges, size, flip, want) in [
        (vec![1, 7, 9], two(14.0, 5.0), true, vec!["Edge1", "Edge7"]),
        (vec![1, 12], angled(6.0, 60.0), false, vec!["Edge1"]),
    ] {
        let err = k.chamfer(&shape, &edges, size, flip).unwrap_err();
        assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
        assert_eq!(err.elements(), want, "{err}");
        assert!(k.chamfer(&shape, &[9], equal(1.0), false).is_ok());
    }
}

#[test]
fn the_unequal_cases_reach_the_report() {
    // The properties above pass vacuously if their cases never fail, or fail naming none.
    // Of their first 300 cases (by the same strategy), count the ones that fail naming
    // edges and the ones where an edge runs past its face.
    use proptest::strategy::ValueTree;
    let mut runner = proptest::test_runner::TestRunner::new(config());
    let (mut named_count, mut past_count, mut unnamed_past) = (0, 0, 0);
    for _ in 0..300 {
        let (dims, edges, size, flip) = case().new_tree(&mut runner).unwrap().current();
        if !edges
            .iter()
            .all(|&e| at_fault(dims, &edges, e, size, flip, 1e-3).is_some())
        {
            continue;
        }
        let b = make_box(dims[0], dims[1], dims[2]).unwrap();
        let past = edges
            .iter()
            .any(|&e| runs_past(dims, e, size, flip, 1e-3) == Some(true));
        past_count += past as u32;
        if let Err(err) = chamfer(&b, &edges, size, flip) {
            match named(&err) {
                Some(_) => named_count += 1,
                None => unnamed_past += past as u32,
            }
        }
    }
    assert!(
        named_count >= 50 && past_count >= 50 && unnamed_past == 0,
        "named {named_count}, past {past_count}, past but unnamed {unnamed_past}"
    );
}

/// Shapes beyond the box: a tangent chain (10 × 20 × 30 with Edge1 filleted at 2), curved
/// faces (a 10 mm cube with a hole of radius 2 through it), a concave edge (the L of
/// a_concave_edge_chamfered_past_its_faces_is_named_and_a_convex_one_isnt, scaled down),
/// and a cylinder.
fn other_shape(which: u8) -> silico_kernel::Shape {
    match which {
        0 => {
            fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1], 2.0)
                .unwrap()
                .shape
        }
        1 => {
            let pin = make(&Primitive::Cylinder {
                radius: 2.0,
                height: 12.0,
            })
            .unwrap();
            let pin = silico_kernel::translate(&pin, [5.0, 5.0, -1.0])
                .unwrap()
                .shape;
            silico_kernel::boolean(
                silico_kernel::BooleanOp::Cut,
                &make_box(10.0, 10.0, 10.0).unwrap(),
                &pin,
            )
            .unwrap()
            .shape
        }
        2 => {
            let l = silico_kernel::Profile {
                points: vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [10.0, 4.0, 0.0],
                    [4.0, 4.0, 0.0],
                    [4.0, 10.0, 0.0],
                    [0.0, 10.0, 0.0],
                ],
            };
            silico_kernel::extrude(&l, 6.0, None).unwrap().shape
        }
        _ => make(&Primitive::Cylinder {
            radius: 3.0,
            height: 5.0,
        })
        .unwrap(),
    }
}

fn other_case() -> impl Strategy<Value = (u8, u32, ChamferSize, bool)> {
    let size = prop_oneof![
        (0.3..8.0f64).prop_map(equal),
        (0.3..8.0f64, 0.3..8.0f64).prop_map(|(d, o)| two(d, o)),
        (
            0.3..6.0f64,
            prop::sample::select(vec![15.0, 30.0, 45.0, 60.0, 75.0])
        )
            .prop_map(|(d, a)| angled(d, a)),
    ];
    (0u8..4, any::<u32>(), size, any::<bool>())
}

fn other_config() -> ProptestConfig {
    ProptestConfig {
        cases: 600,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x07e4_c4a3),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(other_config())]

    /// On a tangent chain, a hole, a concave edge, or a cylinder, every edge a failed
    /// chamfer names is one leaving it out helps: an edge that chamfers alone, and with all
    /// the edges not named, and the edges not named chamfer together, was named wrongly
    /// (the refusal tells the user to leave it out, or try another size for it, for
    /// nothing).
    #[test]
    fn a_failed_chamfer_on_other_shapes_names_no_edge_that_fits_with_the_rest(
        (which, mask, size, flip) in other_case()
    ) {
        let shape = other_shape(which);
        let count = summarize(&shape).unwrap().edges;
        let edges: Vec<u32> = (1..=count).filter(|e| mask & (1 << ((e - 1) % 32)) != 0).collect();
        prop_assume!(!edges.is_empty());
        let Err(err) = chamfer(&shape, &edges, size, flip) else { return Ok(()); };
        let Some(named) = named(&err) else { return Ok(()); };
        prop_assert!(named.windows(2).all(|p| p[0] < p[1]) && named.iter().all(|e| edges.contains(e)));
        let rest: Vec<u32> = edges.iter().copied().filter(|e| !named.contains(e)).collect();
        if !rest.is_empty() && chamfer(&shape, &rest, size, flip).is_err() {
            return Ok(());
        }
        for &e in &named {
            let alone = chamfer(&shape, &[e], size, flip).is_ok();
            let mut with_rest = rest.clone();
            with_rest.push(e);
            with_rest.sort();
            let together = chamfer(&shape, &with_rest, size, flip).is_ok();
            // With each other edge named: two edges of one tangent chain are one contour,
            // which takes its reference face from the lower-numbered, so an edge that
            // chamfers alone may fail with its chain's other given edge (the bottom chain
            // of the filleted box, Edge9 and Edge13, at angled(5.95, 60)).
            let with_each_named = named
                .iter()
                .filter(|&&f| f != e)
                .all(|&f| chamfer(&shape, &[e.min(f), e.max(f)], size, flip).is_ok());
            prop_assert!(
                !(alone && together && with_each_named),
                "shape {} {:?} {:?} flip {}: names Edge{} (of {:?}), which chamfers alone, with the rest {:?}, and with each edge named",
                which, edges, size, flip, e, named, rest
            );
        }
    }
}

#[test]
fn a_chamfer_failing_at_a_corner_names_none_of_the_edges_there_that_fit() {
    // The filleted box of the tangent-chain tests, its top chain's edge on y = 0 with four
    // edges around the corner (10, 0, 30) and the far top edge, at 7.78: OCCT fails at the
    // corner (10, 0, 30) only, which three given edges reach. The edge on y = 0 fails alone;
    // the others chamfer alone and together, so none of them may be named (a report that
    // read a chamfer's corners would name the two that reach (10, 0, 30) with it).
    let filleted = fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1], 2.0)
        .unwrap()
        .shape;
    let at = |a: [f64; 3], b: [f64; 3]| {
        let on = move |p: [f64; 3]| {
            // On the segment from a to b.
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
            let t = (ab[0] * ap[0] + ab[1] * ap[1] + ab[2] * ap[2])
                / (ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2]);
            (-1e-9..=1.0 + 1e-9).contains(&t)
                && (0..3).all(|i| (a[i] + t * ab[i] - p[i]).abs() < 1e-9)
        };
        edge_where(&filleted, on)
    };
    let y0_top = at([2.0, 0.0, 30.0], [10.0, 0.0, 30.0]);
    let x10_top = at([10.0, 0.0, 30.0], [10.0, 20.0, 30.0]);
    let x10_y0 = at([10.0, 0.0, 0.0], [10.0, 0.0, 30.0]);
    let y20_top = at([0.0, 20.0, 30.0], [10.0, 20.0, 30.0]);
    let x10_y20 = at([10.0, 20.0, 0.0], [10.0, 20.0, 30.0]);
    let size = equal(7.775852615001764);
    let mut fit = vec![x10_top, x10_y0, y20_top, x10_y20];
    fit.sort();
    for &e in &fit {
        assert!(chamfer(&filleted, &[e], size, false).is_ok(), "Edge{e}");
    }
    assert!(chamfer(&filleted, &fit, size, false).is_ok());
    assert!(chamfer(&filleted, &[y0_top], size, false).is_err());
    let mut given = fit.clone();
    given.push(y0_top);
    given.sort();
    let err = failed(chamfer(&filleted, &given, size, false));
    let n = named(&err).unwrap_or_default();
    assert!(
        n.iter().all(|e| !fit.contains(e)),
        "names {n:?}, of which only Edge{y0_top} fails: {err:?}"
    );
}
