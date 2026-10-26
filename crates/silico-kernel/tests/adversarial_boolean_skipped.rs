//! break-it: OCCT's skipped-pair warning (`BOPAlgo_AlertIntersectionOfPairOfShapesFailed`),
//! which the kernel logs naming the pair and doesn't refuse (docs/ports/booleans.md). It
//! comes with right results: a pointed cone a rod rests on by its tip, as in FreeCAD. And
//! with wrong ones: the barrel whose turned face lies outside its own surface, as the bridge
//! built it before the arc-range fix, crossed by tools many times, as the tool, fused with
//! itself; each pair is named once, by each input's numbering, in OCCT's order.
//!
//! Expected values are by hand: a cone's volume πr²h/3, a cylinder's πr²l, solids that
//! touch at a point sharing nothing, the bad barrel's face bounds (the box from (−5, −5,
//! −4) to (5, 5, 4)), and `BRepPrimAPI_MakeBox`'s face order (x low, x high, y low, y high,
//! z low, z high). FreeCAD 1.1.3, on the same OCCT, given these solids as text B-reps
//! (Silico's, their coordinates rounded), gives the cone's right answers at each rod angle
//! here: its cut 130.8996939 (to 1e-15), its fuse 382.227106187 in two solids, its common
//! no solid. They're the hand values, so they don't hang on the rounding, as the
//! cone-side crash of patch 0005 does.

use super::brep_fixture::fixture;
use super::kernel::kernel;
use super::logs::warnings_of;

use std::f64::consts::PI;

use silico_kernel::{
    Axis, BooleanOp, Primitive, Profile, Refusal, Shape, boolean, from_brep, make, revolve,
    summarize, to_brep, translate,
};
use silico_protocol::{Blob, ShapeData};

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

/// What OCCT says it skipped of the cone and a rod on its tip: the cone's side (Face1) with
/// the rod's cylinder (Face2, between its end discs).
const TIP_SKIPPED: &str = "OCCT skipped intersecting the base's Face1 with the tool's Face2";

/// Angles the rod lies at, each of which OCCT raised the alert for: along x, along y, and
/// 300° round.
const ANGLES: [f64; 3] = [0.0, PI / 2.0, 20.0 * PI / 12.0];

fn close(got: f64, want: f64) -> bool {
    ((got - want) / want).abs() <= 1e-9
}

#[test]
fn a_rod_resting_on_a_pointed_cones_tip_cuts_nothing_from_it() {
    // They meet at one point, so the cut is the cone whole, as FreeCAD gives it
    // (130.8996939); yet OCCT says it skipped the cone's side with the rod's face 2: a right
    // result that raises the warning, which is why it's logged, not refused.
    let cone = pointed_cone();
    for t in ANGLES {
        let (cut, said) = warnings_of(|| boolean(BooleanOp::Cut, &cone, &rod_on_the_point(t)));
        let cut = cut.unwrap_or_else(|e| panic!("at {t} rad: {e} ({:?})", e.refusal()));
        assert!(
            said.contains(&format!("{TIP_SKIPPED} in the cut")),
            "at {t} rad: {said}"
        );
        let s = summarize(&cut.shape).unwrap();
        assert!(s.valid, "at {t} rad: {s:?}");
        assert!(close(s.volume, CONE_VOLUME), "at {t} rad: {}", s.volume);
    }
}

#[test]
fn a_rod_resting_on_a_pointed_cones_tip_fuses_with_it_into_both() {
    // FreeCAD's fuse is both solids, whole (382.227106187, 125π/3 + 80π), and so is
    // OCCT's, though it raises the warning.
    let cone = pointed_cone();
    for t in ANGLES {
        let (fuse, said) = warnings_of(|| boolean(BooleanOp::Fuse, &cone, &rod_on_the_point(t)));
        let fuse = fuse.unwrap_or_else(|e| panic!("at {t} rad: {e} ({:?})", e.refusal()));
        assert!(said.contains(TIP_SKIPPED), "at {t} rad: {said}");
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
    // The empty common it is (FreeCAD's has no solid), refused as a common of solids that
    // touch at a point is, though OCCT raises the warning.
    let cone = pointed_cone();
    for t in ANGLES {
        let (common, said) =
            warnings_of(|| boolean(BooleanOp::Common, &cone, &rod_on_the_point(t)));
        assert!(said.contains(TIP_SKIPPED), "at {t} rad: {said}");
        let e = common.map(|_| ()).unwrap_err();
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
        let ((cut, fuse), said) = warnings_of(|| {
            (
                boolean(BooleanOp::Cut, &cone, &rod).unwrap(),
                boolean(BooleanOp::Fuse, &cone, &rod).unwrap(),
            )
        });
        assert!(!said.contains("skipped"), "lifted {lift}: {said}");
        assert!(close(summarize(&cut.shape).unwrap().volume, CONE_VOLUME));
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

/// What a boolean of `base` and `tool` logs, for each op (its result, OCCT's answer or a
/// refusal for what else is wrong, isn't kept).
fn logged(base: &Shape, tool: &Shape) -> Vec<(BooleanOp, String)> {
    OPS.into_iter()
        .map(|op| {
            let (_built, said) = warnings_of(|| boolean(op, base, tool));
            (op, said)
        })
        .collect()
}

#[test]
fn the_bad_barrel_less_a_slab_names_its_pair_once() {
    // The slab beyond y = 2: its face 3 (y = 2) cuts the barrel's turned face, Face2, which
    // OCCT skips, twice over, taking them as not meeting: the cut came back whole (526.334
    // for 395.478), "valid". The pair is named once, in each op's words, and OCCT's answer
    // is taken, as FreeCAD takes it.
    let barrel = bad_barrel();
    let slab = block(40.0, 18.0, 40.0, [-20.0, 2.0, -20.0]);
    for (op, said) in logged(&barrel, &slab) {
        let words = format!(
            "OCCT skipped intersecting the base's Face2 with the tool's Face3 in the {}, \
             taking them as not meeting",
            op.name().to_lowercase()
        );
        assert_eq!(said.matches(&words).count(), 1, "{op:?}: {said}");
    }
    let cut = boolean(BooleanOp::Cut, &barrel, &slab).unwrap();
    assert!(close(
        summarize(&cut.shape).unwrap().volume,
        summarize(&barrel).unwrap().volume
    ));
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
    for (op, said) in logged(&barrel, &probe) {
        assert!(
            said.contains(&format!(
                "the base's Face2 with the tool's Face1, the base's Face2 with the tool's \
                 Face3, the base's Face2 with the tool's Face4, and 2 more pairs in the {}",
                op.name().to_lowercase()
            )),
            "{op:?}: {said}"
        );
    }
    // Up to z = 10, past the bad face's bounds: its top (face 6) isn't tried, so four pairs,
    // and one counted.
    let tall = block(7.0, 2.0, 11.0, [3.0, -1.0, -1.0]);
    for (op, said) in logged(&barrel, &tall) {
        assert!(
            said.contains(&format!(
                "the base's Face2 with the tool's Face4, and 1 more pair in the {}",
                op.name().to_lowercase()
            )),
            "{op:?}: {said}"
        );
    }
}

#[test]
fn the_bad_barrel_as_the_tool_names_the_slabs_face_as_the_bases() {
    // The roles swapped: the slab beyond y = 2 is the base, so the pair is its face 3 (y = 2)
    // with the tool's Face2, in OCCT's order (the base's elements first).
    let barrel = bad_barrel();
    let slab = block(40.0, 18.0, 40.0, [-20.0, 2.0, -20.0]);
    for (op, said) in logged(&slab, &barrel) {
        assert!(
            said.contains("OCCT skipped intersecting the base's Face3 with the tool's Face2 in"),
            "{op:?}: {said}"
        );
    }
}

#[test]
fn the_bad_barrel_fused_with_itself_shares_every_face_and_skips_nothing() {
    // The same shape twice: OCCT has nothing to intersect (every face is shared), so no
    // pair is skipped, and the fuse is the barrel, its volume unchanged.
    let barrel = bad_barrel();
    let (fuse, said) = warnings_of(|| boolean(BooleanOp::Fuse, &barrel, &barrel).unwrap());
    assert!(!said.contains("skipped"), "{said}");
    let (got, want) = (
        summarize(&fuse.shape).unwrap().volume,
        summarize(&barrel).unwrap().volume,
    );
    assert!(close(got, want), "{got} for {want}");
}

fn data(shape: &Shape) -> ShapeData {
    ShapeData::unchecked(Blob::inline(to_brep(shape).unwrap()))
}

#[test]
fn worker_a_rod_resting_on_a_pointed_cones_tip_is_cut_and_the_worker_goes_on() {
    // Through the worker and the core: the cone whole, and the worker goes on.
    let mut k = kernel();
    let (cone, rod) = (data(&pointed_cone()), data(&rod_on_the_point(0.0)));
    let cut = k
        .boolean(BooleanOp::Cut, &cone, &rod)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(cut.shape.valid(), Some(true));
    assert!(close(k.summarize(&cut.shape).unwrap().volume, CONE_VOLUME));
    let again = k.boolean(BooleanOp::Fuse, &rod, &rod).unwrap();
    assert_eq!(again.shape.valid(), Some(true));
}
