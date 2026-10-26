//! break-it: a boolean OCCT made skipping a pair of elements it couldn't intersect, refused
//! (`Refusal::BooleanPairsSkipped`, PR #171), on inputs other than the one it was found
//! with: a pointed cone a rod rests on by its tip (a right result OCCT raises the alert
//! for), tools that cross the barrel's bad face many times, the barrel as the tool, fused
//! with itself, and moved onto itself; the refusal run again and through fresh workers; and
//! the inputs and the worker after it.
//!
//! Expected values are by hand: a cone's volume πr²h/3, a cylinder's πr²l, solids that
//! touch at a point sharing nothing, the bad barrel's face bounds (the box from (−5, −5,
//! −4) to (5, 5, 4)), and `BRepPrimAPI_MakeBox`'s face order (x low, x high, y low, y high,
//! z low, z high). FreeCAD 1.1.3, on the same OCCT, given these very solids (Silico's
//! B-reps, written as text), gives the cone's right answers at each rod angle here: its cut
//! 130.8996939 (to 1e-15), its fuse 382.227106187 in two solids, its common no solid.

use super::brep_fixture::fixture;
use super::kernel::kernel;

use std::f64::consts::PI;

use silico_core::ErrorCode;
use silico_kernel::{
    Axis, BooleanOp, Primitive, Profile, Refusal, Shape, boolean, from_brep, make, revolve,
    summarize, to_brep, translate,
};
use silico_protocol::{Blob, ElementKind, ElementRef, Role, ShapeData, Skipped};

const OPS: [BooleanOp; 3] = [BooleanOp::Fuse, BooleanOp::Cut, BooleanOp::Common];

/// The cone of radius 5 at z = 0 to its point at (0, 0, 5): 125π/3.
fn pointed_cone() -> Shape {
    make(&Primitive::Cone {
        radius1: 5.0,
        radius2: 0.0,
        height: 5.0,
    })
    .unwrap()
}
const CONE_VOLUME: f64 = 125.0 * PI / 3.0;

/// A rod of radius 2 and length 20 along (cos t, sin t, 0), its axis through (0, 0, 7): it
/// rests on the cone's point, touching it there and nowhere else. Turned from a rectangle,
/// as a revolve feature makes one.
fn rod_on_the_point(t: f64) -> Shape {
    let (c, s) = (libm::cos(t), libm::sin(t));
    let at = |a: f64, b: f64| [a * c - b * s, a * s + b * c, 0.0];
    let profile = Profile {
        points: vec![at(-10.0, 0.0), at(10.0, 0.0), at(10.0, 2.0), at(-10.0, 2.0)],
    };
    let axis = Axis {
        origin: [0.0; 3],
        direction: [c, s, 0.0],
    };
    let rod = revolve(&profile, &axis, 360.0).unwrap().shape;
    translate(&rod, [0.0, 0.0, 7.0]).unwrap().shape
}
const ROD_VOLUME: f64 = 80.0 * PI;

/// Angles the rod lies at, each of which OCCT raised the alert for: along x, along y, and
/// 300° round.
const ANGLES: [f64; 3] = [0.0, PI / 2.0, 20.0 * PI / 12.0];

fn close(got: f64, want: f64) -> bool {
    ((got - want) / want).abs() <= 1e-9
}

#[test]
fn a_rod_resting_on_a_pointed_cones_tip_cuts_nothing_from_it() {
    // BREAKS: refused as `BooleanPairsSkipped` (the cone's Face1 with the rod's face 2),
    // though OCCT's result is right: they meet at one point, so the cut is the cone whole,
    // as FreeCAD gives it (130.8996939).
    let cone = pointed_cone();
    for t in ANGLES {
        let cut = boolean(BooleanOp::Cut, &cone, &rod_on_the_point(t))
            .unwrap_or_else(|e| panic!("at {t} rad: {e} ({:?})", e.refusal()));
        let s = summarize(&cut.shape).unwrap();
        assert!(s.valid, "at {t} rad: {s:?}");
        assert!(close(s.volume, CONE_VOLUME), "at {t} rad: {}", s.volume);
    }
}

#[test]
fn a_rod_resting_on_a_pointed_cones_tip_fuses_with_it_into_both() {
    // BREAKS: refused as `BooleanPairsSkipped`; FreeCAD's fuse is both solids, whole
    // (382.227106187, 125π/3 + 80π).
    let cone = pointed_cone();
    for t in ANGLES {
        let fuse = boolean(BooleanOp::Fuse, &cone, &rod_on_the_point(t))
            .unwrap_or_else(|e| panic!("at {t} rad: {e} ({:?})", e.refusal()));
        let s = summarize(&fuse.shape).unwrap();
        assert!(s.valid, "at {t} rad: {s:?}");
        assert!(
            close(s.volume, CONE_VOLUME + ROD_VOLUME),
            "at {t} rad: {}",
            s.volume
        );
    }
}

#[test]
fn a_rod_resting_on_a_pointed_cones_tip_has_no_common_with_it() {
    // BREAKS: refused as `BooleanPairsSkipped`, not as the empty common it is (FreeCAD's
    // has no solid), which is what a common of solids that touch at a point is refused as.
    let cone = pointed_cone();
    for t in ANGLES {
        let e = boolean(BooleanOp::Common, &cone, &rod_on_the_point(t))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            e.refusal(),
            &Refusal::BooleanResultEmpty {
                op: BooleanOp::Common
            },
            "at {t} rad: {e}"
        );
    }
}

#[test]
fn a_rod_lifted_off_a_pointed_cones_tip_or_into_it_is_cut_and_fused_as_ever() {
    // The edge of the break: a rod a micrometre or a tenth above the point is built (no
    // pair skipped), so it's the touch that OCCT can't intersect.
    let cone = pointed_cone();
    for lift in [1e-3, 1e-1] {
        let rod = translate(&rod_on_the_point(0.0), [0.0, 0.0, lift])
            .unwrap()
            .shape;
        let cut = boolean(BooleanOp::Cut, &cone, &rod).unwrap();
        assert!(close(summarize(&cut.shape).unwrap().volume, CONE_VOLUME));
        let fuse = boolean(BooleanOp::Fuse, &cone, &rod).unwrap();
        assert!(close(
            summarize(&fuse.shape).unwrap().volume,
            CONE_VOLUME + ROD_VOLUME
        ));
    }
    // Into the point by 0.1: the cone loses its tip inside the rod, a sliver well under a
    // thousandth of its volume (FreeCAD: 0.00103).
    let rod = translate(&rod_on_the_point(0.0), [0.0, 0.0, -0.1])
        .unwrap()
        .shape;
    let cut = boolean(BooleanOp::Cut, &cone, &rod).unwrap();
    let v = summarize(&cut.shape).unwrap().volume;
    assert!(v < CONE_VOLUME && v > CONE_VOLUME * 0.999, "{v}");
}

fn bad_barrel() -> Shape {
    from_brep(&fixture("barrel_range_off_its_surface.bin")).unwrap()
}

fn block(dx: f64, dy: f64, dz: f64, at: [f64; 3]) -> Shape {
    translate(&make(&Primitive::Box { dx, dy, dz }).unwrap(), at)
        .unwrap()
        .shape
}

fn face(role: Role, index: u32) -> Skipped {
    Skipped {
        kind: ElementKind::Face,
        of: Some((role, index)),
    }
}

#[test]
fn a_probe_through_the_bad_face_names_each_of_its_faces_that_meets_it_once_in_order() {
    // The box from (3, −1, −1) to (10, 1, 1) pokes through the barrel's side. Of its faces,
    // x = 3 (face 1), y = ±1 (3, 4), and z = ±1 (5, 6) lie within the bad face's bounds,
    // the box from (−5, −5, −4) to (5, 5, 4); x = 10 (face 2) is beyond them. So OCCT tries
    // five pairs with the barrel's Face2, and fails each: five pairs, in its order (the
    // tool's faces ascending), three named and two counted.
    let barrel = bad_barrel();
    let probe = block(7.0, 2.0, 2.0, [3.0, -1.0, -1.0]);
    let want: Vec<[Skipped; 2]> = [1, 3, 4, 5, 6]
        .into_iter()
        .map(|n| [face(Role::Base, 2), face(Role::Tool, n)])
        .collect();
    for op in OPS {
        let e = boolean(op, &barrel, &probe).map(|_| ()).unwrap_err();
        assert_eq!(
            e.refusal(),
            &Refusal::BooleanPairsSkipped {
                op,
                pairs: want.clone()
            },
            "{op:?}"
        );
        assert_eq!(e.refusal().elements(), vec![ElementRef::face(2)], "{op:?}");
        assert!(
            e.to_string().contains(
                "the base's Face2 with the tool's face 1, the base's Face2 with the tool's \
                 face 3, the base's Face2 with the tool's face 4, and 2 more pairs in the"
            ),
            "{e}"
        );
    }
}

#[test]
fn the_bad_barrel_as_the_tool_names_the_slabs_face_as_the_bases() {
    // The roles swapped: the slab beyond y = 2 is the base, so the pair is its face 3 (y = 2)
    // with the tool's Face2, in OCCT's order (the base's elements first), and the error's
    // element is the slab's face, the one input a boolean's error names.
    let barrel = bad_barrel();
    let slab = block(40.0, 18.0, 40.0, [-20.0, 2.0, -20.0]);
    for op in OPS {
        let e = boolean(op, &slab, &barrel).map(|_| ()).unwrap_err();
        assert_eq!(
            e.refusal(),
            &Refusal::BooleanPairsSkipped {
                op,
                pairs: vec![[face(Role::Base, 3), face(Role::Tool, 2)]]
            },
            "{op:?}"
        );
        assert_eq!(e.refusal().elements(), vec![ElementRef::face(3)]);
    }
}

#[test]
fn the_bad_barrel_fused_with_itself_shares_every_face_and_skips_nothing() {
    // The same shape twice: OCCT has nothing to intersect (every face is shared), so no
    // pair is skipped, and the fuse is the barrel, its volume unchanged.
    let barrel = bad_barrel();
    let fuse = boolean(BooleanOp::Fuse, &barrel, &barrel).unwrap();
    let (got, want) = (
        summarize(&fuse.shape).unwrap().volume,
        summarize(&barrel).unwrap().volume,
    );
    assert!(close(got, want), "{got} for {want}");
}

#[test]
fn the_bad_barrel_moved_half_a_millimetre_onto_itself_is_refused_naming_both_bad_faces() {
    // Each copy's turned face is bad, so every pair has one or the other in it, and none is
    // named twice.
    let barrel = bad_barrel();
    let moved = translate(&barrel, [0.5, 0.0, 0.0]).unwrap().shape;
    for op in OPS {
        let e = boolean(op, &barrel, &moved).map(|_| ()).unwrap_err();
        let Refusal::BooleanPairsSkipped { pairs, .. } = e.refusal() else {
            panic!("{op:?}: {e}");
        };
        assert!(
            pairs
                .iter()
                .all(|p| p.contains(&face(Role::Base, 2)) || p.contains(&face(Role::Tool, 2))),
            "{op:?}: {pairs:?}"
        );
        for (i, p) in pairs.iter().enumerate() {
            assert!(!pairs[..i].contains(p), "{op:?}: {p:?} named twice");
        }
    }
}

#[test]
fn a_refused_boolean_leaves_its_inputs_as_they_were_and_the_next_one_builds() {
    let barrel = bad_barrel();
    let slab = block(40.0, 18.0, 40.0, [-20.0, 2.0, -20.0]);
    let (barrel_bytes, slab_bytes) = (to_brep(&barrel).unwrap(), to_brep(&slab).unwrap());
    for op in OPS {
        boolean(op, &barrel, &slab).map(|_| ()).unwrap_err();
    }
    assert_eq!(to_brep(&barrel).unwrap(), barrel_bytes);
    assert_eq!(to_brep(&slab).unwrap(), slab_bytes);
    // The slab less a box inside it: 40 × 18 × 40 − 10³.
    let inside = block(10.0, 10.0, 10.0, [0.0, 5.0, 0.0]);
    let cut = boolean(BooleanOp::Cut, &slab, &inside).unwrap();
    assert!(close(
        summarize(&cut.shape).unwrap().volume,
        40.0 * 18.0 * 40.0 - 1000.0
    ));
}

fn data(shape: &Shape) -> ShapeData {
    ShapeData::unchecked(Blob::inline(to_brep(shape).unwrap()))
}

#[test]
fn the_refusal_is_the_same_every_run_and_from_every_worker() {
    let barrel = bad_barrel();
    let probe = block(7.0, 2.0, 2.0, [3.0, -1.0, -1.0]);
    let first = boolean(BooleanOp::Cut, &barrel, &probe)
        .map(|_| ())
        .unwrap_err()
        .refusal()
        .clone();
    for _ in 0..10 {
        let again = boolean(BooleanOp::Cut, &barrel, &probe)
            .map(|_| ())
            .unwrap_err();
        assert_eq!(again.refusal(), &first);
    }
    let (b, p) = (data(&barrel), data(&probe));
    let said: Vec<_> = (0..2)
        .map(|_| {
            let e = kernel().boolean(BooleanOp::Cut, &b, &p).unwrap_err();
            assert_eq!(e.code(), ErrorCode::KernelFailed, "{e}");
            assert_eq!(e.elements(), ["Face2"], "{e}");
            e.to_string()
        })
        .collect();
    assert_eq!(said[0], said[1]);
}

#[test]
fn worker_a_rod_resting_on_a_pointed_cones_tip_is_cut_and_the_worker_goes_on() {
    // BREAKS: through the worker and the core, `kernel_failed` naming the cone's Face1, for
    // the cone whole.
    let mut k = kernel();
    let (cone, rod) = (data(&pointed_cone()), data(&rod_on_the_point(0.0)));
    let cut = k.boolean(BooleanOp::Cut, &cone, &rod);
    // The worker goes on either way.
    let again = k.boolean(BooleanOp::Fuse, &rod, &rod).unwrap();
    assert_eq!(again.shape.valid(), Some(true));
    let cut = cut.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(cut.shape.valid(), Some(true));
}

/// A rod of radius 1 lying level against the pointed cone's side at its half height, 300°
/// round (the foot (2.5 cos t, 2.5 sin t, 2.5)), across the cone's slope: it touches the
/// cone at that one point. Its axis is 1 from the foot along the side's outward normal,
/// (cos t, sin t, 1)/√2.
fn rod_against_the_side() -> Shape {
    let t = 20.0 * PI / 12.0;
    let (c, s) = (libm::cos(t), libm::sin(t));
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let centre = [2.5 * c + c * h, 2.5 * s + s * h, 2.5 + h];
    let u = t + PI / 2.0;
    let (cu, su) = (libm::cos(u), libm::sin(u));
    let at = |a: f64, b: f64| [a * cu - b * su, a * su + b * cu, 0.0];
    let profile = Profile {
        points: vec![at(-10.0, 0.0), at(10.0, 0.0), at(10.0, 1.0), at(-10.0, 1.0)],
    };
    let axis = Axis {
        origin: [0.0; 3],
        direction: [cu, su, 0.0],
    };
    let rod = revolve(&profile, &axis, 360.0).unwrap().shape;
    translate(&rod, centre).unwrap().shape
}

#[test]
fn worker_a_rod_against_a_pointed_cones_side_is_cut_fused_and_commoned() {
    // BREAKS, though not by this PR: OCCT segfaults in the boolean itself
    // (BOPAlgo_PaveFiller::MakeBlocks, BOPTools_AlgoTools::MakePCurve given a null curve),
    // for each op, and the worker dies (`worker_crashed`). FreeCAD 1.1.3, given these very
    // B-reps bit for bit, builds each: the cut the cone whole (130.8996939), the fuse both
    // (193.731546971, 125π/3 + 20π, two solids), the common nothing. Run in-process, the
    // crash takes the test program with it, so it's run on a worker.
    let (cone, rod) = (data(&pointed_cone()), data(&rod_against_the_side()));
    let want = [
        (BooleanOp::Fuse, CONE_VOLUME + 20.0 * PI),
        (BooleanOp::Cut, CONE_VOLUME),
    ];
    for (op, volume) in want {
        let mut k = kernel();
        let built = k
            .boolean(op, &cone, &rod)
            .unwrap_or_else(|e| panic!("{op:?}: {e}"));
        let s = k.summarize(&built.shape).unwrap();
        assert!(s.valid, "{op:?}: {s:?}");
        assert!(close(s.volume, volume), "{op:?}: {} for {volume}", s.volume);
    }
    let e = kernel()
        .boolean(BooleanOp::Common, &cone, &rod)
        .map(|_| ())
        .unwrap_err();
    assert!(
        e.to_string().contains("the common's result is empty"),
        "{e}"
    );
}
