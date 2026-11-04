//! The kernel's copies of a tool, fused into or cut from a base in one boolean
//! (docs/ports/patterns.md, §7, *The kernel*), on the real kernel worker: FreeCAD's
//! PartDesign pattern tests' volumes (§5), the note's probes worked out by hand, each copy's
//! place by hand, and each result solid's faces.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::f64::consts::PI;
use std::time::Duration;

use super::close::assert_close;
use super::kernel::kernel;
use silico_core::ErrorCode;
use silico_core::kernel::{
    Axis, BuiltData, CopiesOp, Description, Generation, History, KernelRequest, KernelWorker,
    Placement, Primitive, ShapeData, Surface,
};
use silico_core::pool::{KernelPool, Outcome};
use silico_protocol::{BooleanOp, radians};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// The relative precision a summary's volume is integrated to, as the kernel's measure
/// takes it.
const PRECISE: f64 = 1e-9;

/// How far apart two points a test works out by hand may be, in millimetres: rounding of
/// a turn's sine and cosine, and of a face's centroid by integration.
const NEAR: f64 = 1e-9;

const Z: Axis = Axis {
    origin: [0.0; 3],
    direction: [0.0, 0.0, 1.0],
};

fn cube(size: f64) -> Primitive {
    Primitive::Box {
        dx: size,
        dy: size,
        dz: size,
    }
}

/// `primitive` made, and moved by `offset` unless that's nothing.
fn at(k: &mut KernelWorker, primitive: Primitive, offset: [f64; 3]) -> ShapeData {
    let made = k.make(primitive).unwrap();
    if offset == [0.0; 3] {
        made
    } else {
        k.translate(&made, offset).unwrap().shape
    }
}

fn fuse(k: &mut KernelWorker, a: &ShapeData, b: &ShapeData) -> ShapeData {
    k.boolean(BooleanOp::Fuse, a, b).unwrap().shape
}

fn moved(offset: [f64; 3]) -> Placement {
    Placement::Translation { offset }
}

fn turned(degrees: f64) -> Placement {
    Placement::Rotation {
        axis: Z,
        angle: radians(degrees),
    }
}

fn assert_near(what: &str, got: [f64; 3], want: [f64; 3]) {
    let off = (0..3).map(|i| (got[i] - want[i]).abs()).fold(0.0, f64::max);
    assert!(off <= NEAR, "{what}: got {got:?}, want {want:?}");
}

/// The result faces input `input`'s faces are, kept or modified one for one, by the input's
/// face number: what a copy's faces became, numbered as the tool's.
fn faces_of(history: &History, input: usize) -> Vec<(u32, u32)> {
    let fates = &history.inputs[input].faces;
    (1u32..)
        .zip(fates)
        .filter_map(|(n, fate)| match (fate.kept, fate.modified.as_slice()) {
            (Some(kept), []) => Some((n, kept)),
            (None, [one]) => Some((n, one.element.index)),
            _ => None,
        })
        .collect()
}

/// The centroid of the planar face of input `input` (a copy) the result has at height `z`
/// with its normal straight up: a pin's top.
fn top_of(described: &Description, history: &History, input: usize, z: f64) -> [f64; 3] {
    let tops: Vec<[f64; 3]> = faces_of(history, input)
        .into_iter()
        .map(|(_, face)| &described.faces[face as usize - 1])
        .filter(|f| {
            matches!(f.surface, Surface::Plane { normal, .. } if (normal[2] - 1.0).abs() < 1e-12)
                && (f.centroid[2] - z).abs() < NEAR
        })
        .map(|f| f.centroid)
        .collect();
    assert_eq!(tops.len(), 1, "input {input}'s tops at {z}: {tops:?}");
    tops[0]
}

/// Copy `copies`'s run with `op` on `base` and `tool` at `placements`, its history checked
/// for its inputs (the core's reading), and the result's volume, faces, edges, vertices.
fn built(
    k: &mut KernelWorker,
    op: CopiesOp,
    base: &ShapeData,
    tool: &ShapeData,
    placements: &[Placement],
) -> BuiltData {
    let made = k.copies(op, base, tool, placements).unwrap();
    assert_eq!(made.history.inputs.len(), 1 + placements.len());
    assert_eq!(made.shape.valid(), Some(true));
    made
}

#[test]
fn ten_cubes_patterned_along_x_fuse_face_to_face_into_one_solid_of_10000() {
    // FreeCAD's testXAxisLinearPattern: a 10 mm cube, 10 occurrences over 90 along X, so the
    // copies at 10, 20, ..., 90. Ten cubes touching face to face: 10 × 1000 mm³, one solid.
    // Unrefined, by hand: each cube's four sides, and the two ends, 42 faces; four edges
    // along X a cube and four round each of the 11 sections, 84; four vertices a section,
    // 44 (FreeCAD 1.1.3 with Refine off: the same).
    let mut k = kernel();
    let c = k.make(cube(10.0)).unwrap();
    let placements: Vec<Placement> = (1..10)
        .map(|i| moved([10.0 * f64::from(i), 0.0, 0.0]))
        .collect();
    let made = built(&mut k, CopiesOp::Fuse, &c, &c, &placements);
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 10000.0, PRECISE);
    assert_eq!((s.faces, s.edges, s.vertices), (42, 84, 44));
    assert_eq!(made.history.result.solids, 1);
    assert_eq!(
        made.history.topology.solid_faces,
        vec![(1..=42).collect::<Vec<u32>>()]
    );
    assert_eq!(s.bounds, Some([[0.0; 3], [100.0, 10.0, 10.0]]));
}

#[test]
fn four_cubes_turned_a_quarter_about_z_meet_at_the_axis_in_one_solid_of_4000() {
    // FreeCAD's TestPolarPattern: a 10 mm cube turned 4 times over 360° about Z, so by 90,
    // 180, and 270. Four cubes meeting along the axis, filling the 20 × 20 × 10 block about
    // it: 4000 mm³, one solid, its centre on the axis halfway up. Unrefined, by hand: each
    // of the block's four sides in two, and its top and bottom in four, 16 faces; a 3 × 3
    // grid of vertices at the top and the bottom, 18; twelve edges in each grid and an
    // upright one at each of the eight vertices round its rim (none on the axis, inside the
    // solid), 32 (FreeCAD 1.1.3 with Refine off: the same).
    let mut k = kernel();
    let c = k.make(cube(10.0)).unwrap();
    let made = built(
        &mut k,
        CopiesOp::Fuse,
        &c,
        &c,
        &[turned(90.0), turned(180.0), turned(270.0)],
    );
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 4000.0, PRECISE);
    assert_eq!((s.faces, s.edges, s.vertices), (16, 32, 18));
    assert_near("centre", s.center, [0.0, 0.0, 5.0]);
    let [low, high] = s.bounds.unwrap();
    assert_near("lowest corner", low, [-10.0, -10.0, 0.0]);
    assert_near("highest corner", high, [10.0, 10.0, 10.0]);
    assert_eq!(made.history.result.solids, 1);
}

#[test]
fn four_cubes_turned_about_x_and_about_y_fill_the_block_about_that_axis() {
    // FreeCAD's TestPolarPattern about each origin axis: the 10 mm cube turned 4 times
    // over 360° about X, then about Y, 4000 mm³ and the Z case's 16/32/18 by symmetry, the
    // block about the axis. And one quarter turn alone, right-handed, by hand: about X it
    // takes y to z, so the cube's copy lies at y from -10 to 0; about Y it takes z to x, so
    // the copy lies at z from -10 to 0 (the other way round, each would lie at +y or +z's
    // opposite).
    let mut k = kernel();
    let c = k.make(cube(10.0)).unwrap();
    let about = |direction: [f64; 3], degrees: f64| Placement::Rotation {
        axis: Axis {
            origin: [0.0; 3],
            direction,
        },
        angle: radians(degrees),
    };
    for (direction, centre, low, high, quarter_low, quarter_high) in [
        (
            [1.0, 0.0, 0.0],
            [5.0, 0.0, 0.0],
            [0.0, -10.0, -10.0],
            [10.0, 10.0, 10.0],
            [0.0, -10.0, 0.0],
            [10.0, 10.0, 10.0],
        ),
        (
            [0.0, 1.0, 0.0],
            [0.0, 5.0, 0.0],
            [-10.0, 0.0, -10.0],
            [10.0, 10.0, 10.0],
            [0.0, 0.0, -10.0],
            [10.0, 10.0, 10.0],
        ),
    ] {
        let turns = [90.0, 180.0, 270.0].map(|d| about(direction, d));
        let made = built(&mut k, CopiesOp::Fuse, &c, &c, &turns);
        let s = k.summarize(&made.shape).unwrap();
        assert_close("volume", s.volume, 4000.0, PRECISE);
        assert_eq!(
            (s.faces, s.edges, s.vertices),
            (16, 32, 18),
            "{direction:?}"
        );
        assert_near("centre", s.center, centre);
        let [l, h] = s.bounds.unwrap();
        assert_near("lowest corner", l, low);
        assert_near("highest corner", h, high);
        assert_eq!(made.history.result.solids, 1);
        let made = built(&mut k, CopiesOp::Fuse, &c, &c, &[about(direction, 90.0)]);
        let s = k.summarize(&made.shape).unwrap();
        assert_close("quarter volume", s.volume, 2000.0, PRECISE);
        let [l, h] = s.bounds.unwrap();
        assert_near("quarter's lowest corner", l, quarter_low);
        assert_near("quarter's highest corner", h, quarter_high);
    }
}

#[test]
fn a_turn_about_a_skew_axis_lands_the_copy_where_the_turn_takes_it() {
    // A third of a turn about (1, 1, 1)/√3 through the origin, right-handed, takes x to y,
    // y to z, and z to x, so (x, y, z) to (z, x, y): a unit cube at x from 20 to 21 lands at
    // y from 20 to 21 (the other way round it would land at z from 20 to 21). Apart from the
    // tool, so two solids of 1 mm³.
    let mut k = kernel();
    let c = at(&mut k, cube(1.0), [20.0, 0.0, 0.0]);
    let third = 1.0 / 3.0_f64.sqrt();
    let made = built(
        &mut k,
        CopiesOp::Fuse,
        &c,
        &c,
        &[Placement::Rotation {
            axis: Axis {
                origin: [0.0; 3],
                direction: [third, third, third],
            },
            angle: radians(120.0),
        }],
    );
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 2.0, PRECISE);
    assert_eq!(made.history.result.solids, 2);
    let [low, high] = s.bounds.unwrap();
    assert_near("lowest corner", low, [0.0, 0.0, 0.0]);
    assert_near("highest corner", high, [21.0, 21.0, 1.0]);
}

#[test]
fn a_cube_mirrored_in_the_plane_it_stands_on_doubles_into_one_solid() {
    // FreeCAD's testMirroredPrimitiveCase and testMirroredSketchCase: a unit cube mirrored
    // in the plane of one of its faces, 2 mm³. And a 10 mm cube in XY: 2000 mm³, one solid,
    // by hand a 10 × 10 × 20 block split at z = 0: its four sides in two and its top and
    // bottom, 10 faces; three squares of edges and two upright edges at each corner, 20;
    // three squares of vertices, 12 (FreeCAD 1.1.3 with Refine off: the same).
    let mut k = kernel();
    let unit = k.make(cube(1.0)).unwrap();
    let in_yz = Placement::Mirror {
        origin: [0.0; 3],
        normal: [1.0, 0.0, 0.0],
    };
    let made = built(&mut k, CopiesOp::Fuse, &unit, &unit, &[in_yz]);
    assert_close(
        "unit volume",
        k.summarize(&made.shape).unwrap().volume,
        2.0,
        PRECISE,
    );
    let c = k.make(cube(10.0)).unwrap();
    let in_xy = Placement::Mirror {
        origin: [0.0; 3],
        normal: [0.0, 0.0, 1.0],
    };
    let made = built(&mut k, CopiesOp::Fuse, &c, &c, &[in_xy]);
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 2000.0, PRECISE);
    assert_eq!((s.faces, s.edges, s.vertices), (10, 20, 12));
    let [low, high] = s.bounds.unwrap();
    assert_near("lowest corner", low, [0.0, 0.0, -10.0]);
    assert_near("highest corner", high, [10.0, 10.0, 10.0]);
    assert_eq!(made.history.result.solids, 1);
}

#[test]
fn pins_patterned_along_a_block_add_each_pins_volume_above_it() {
    // The note's probe: a cylinder r 3 standing 5 mm on a 100 × 20 × 10 block, three
    // occurrences over 40 along X (copies 20 and 40 along). Each pin adds π · 3² · 5:
    // 20000 + 3 · 45π = 20424.115008.
    let mut k = kernel();
    let block = k
        .make(Primitive::Box {
            dx: 100.0,
            dy: 20.0,
            dz: 10.0,
        })
        .unwrap();
    let pin = at(
        &mut k,
        Primitive::Cylinder {
            radius: 3.0,
            height: 5.0,
        },
        [10.0, 10.0, 10.0],
    );
    let base = fuse(&mut k, &block, &pin);
    let made = built(
        &mut k,
        CopiesOp::Fuse,
        &base,
        &pin,
        &[moved([20.0, 0.0, 0.0]), moved([40.0, 0.0, 0.0])],
    );
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 20000.0 + 135.0 * PI, PRECISE);
    assert_close("probe's volume", s.volume, 20424.115008, 1e-10);
    // Each copy's top where its offset puts the pin's: (10 + 20k, 10, 15).
    let described = k.describe(&made.shape).unwrap();
    for (copy, x) in [(1, 30.0), (2, 50.0)] {
        let top = top_of(&described, &made.history, copy, 15.0);
        assert_near(&format!("copy {copy}'s top"), top, [x, 10.0, 15.0]);
    }
}

/// The area two circles of radius `r` whose centres are `d` apart share (a lens), by hand:
/// two circular segments, each `r²·acos(d / 2r) − (d / 2)·√(r² − (d / 2)²)`.
fn lens(r: f64, d: f64) -> f64 {
    2.0 * r * r * libm::acos(d / (2.0 * r)) - 0.5 * d * (4.0 * r * r - d * d).sqrt()
}

#[test]
fn three_overlapping_holes_remove_their_union() {
    // The note's probe: three holes r 3, 2 mm apart in a row, through a 10 mm plate. By
    // inclusion and exclusion, the union is 3 discs less the lenses of each pair, plus the
    // part all three share; the outer two meet only inside the middle one (their lens's
    // tips are √5 < 3 from its centre), so that part is their lens, and the union is
    // 27π − 2·lens(3, 2): 51.822171 mm², 518.22171 mm³ through 10 mm (FreeCAD 1.1.3:
    // 518.221711 removed). Each copy a tool of its own: one compound of overlapping
    // copies OCCT cuts wrongly.
    let mut k = kernel();
    let plate = k
        .make(Primitive::Box {
            dx: 40.0,
            dy: 20.0,
            dz: 10.0,
        })
        .unwrap();
    let drill = at(
        &mut k,
        Primitive::Cylinder {
            radius: 3.0,
            height: 20.0,
        },
        [10.0, 10.0, -5.0],
    );
    let base = k.boolean(BooleanOp::Cut, &plate, &drill).unwrap().shape;
    let made = built(
        &mut k,
        CopiesOp::Cut,
        &base,
        &drill,
        &[moved([2.0, 0.0, 0.0]), moved([4.0, 0.0, 0.0])],
    );
    let removed = 10.0 * (27.0 * PI - 2.0 * lens(3.0, 2.0));
    assert_close("hand's removal", removed, 518.221711, 1e-8);
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 8000.0 - removed, PRECISE);
    assert_eq!(made.history.result.solids, 1);
}

#[test]
fn a_pin_mirrored_a_millimetre_off_its_axis_fuses_into_the_union_of_two_discs() {
    // The note's probe: a cylinder r 3, 5 mm tall, on a 100 × 20 × 10 block, mirrored in a
    // plane 1 mm off its axis: the two pins' axes 2 mm apart, overlapping in a lens. The
    // union is 2 · 9π − lens(3, 2) over 5 mm: 20000 + 200.241263 (FreeCAD 1.1.3:
    // 20200.241263).
    let mut k = kernel();
    let block = k
        .make(Primitive::Box {
            dx: 100.0,
            dy: 20.0,
            dz: 10.0,
        })
        .unwrap();
    let pin = at(
        &mut k,
        Primitive::Cylinder {
            radius: 3.0,
            height: 5.0,
        },
        [50.0, 10.0, 10.0],
    );
    let base = fuse(&mut k, &block, &pin);
    let mirror = Placement::Mirror {
        origin: [51.0, 0.0, 0.0],
        normal: [1.0, 0.0, 0.0],
    };
    let made = built(&mut k, CopiesOp::Fuse, &base, &pin, &[mirror]);
    let union = 5.0 * (18.0 * PI - lens(3.0, 2.0));
    assert_close("hand's union", 20000.0 + union, 20200.241263, 1e-10);
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 20000.0 + union, PRECISE);
    assert_eq!(made.history.result.solids, 1);
}

/// A point of the plane, in millimetres.
type Point2 = [f64; 2];

/// Twice the signed area of the triangle `a`, `b`, `p`: above nought when `p` is left of
/// the line from `a` to `b`.
fn side(a: Point2, b: Point2, p: Point2) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// The area of a polygon by the shoelace formula, signed: above nought counter-clockwise.
fn signed_area(polygon: &[Point2]) -> f64 {
    let n = polygon.len();
    0.5 * (0..n)
        .map(|i| {
            let (p, q) = (polygon[i], polygon[(i + 1) % n]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum::<f64>()
}

/// The part of convex `polygon` inside convex `window`, both counter-clockwise, by
/// Sutherland and Hodgman: `polygon` clipped to the left of each of `window`'s edges.
fn clipped(polygon: &[Point2], window: &[Point2]) -> Vec<Point2> {
    let mut kept = polygon.to_vec();
    for i in 0..window.len() {
        let (a, b) = (window[i], window[(i + 1) % window.len()]);
        let input = std::mem::take(&mut kept);
        for j in 0..input.len() {
            let (p, q) = (input[j], input[(j + 1) % input.len()]);
            let (sp, sq) = (side(a, b, p), side(a, b, q));
            if sp >= 0.0 {
                kept.push(p);
            }
            if (sp >= 0.0) != (sq >= 0.0) {
                let t = sp / (sp - sq);
                kept.push([p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]);
            }
        }
    }
    kept
}

#[test]
fn copies_a_hair_apart_cut_and_fuse_the_union_of_their_footprints_from_a_plate() {
    // Found by proptest (random_placements_build_a_valid_result_within_the_volumes_they_
    // could_have, adversarial_copies.rs): a unit cube's copies moved a hair, 1.01e-7,
    // along x, along x and y, and mirrored in an upright plane through the cube, cut from
    // and fused onto a 10 × 10 × 1 plate. Without FreeCAD 1.1.3's automatic fuzzy value
    // OCCT lost the whole plate, the cut 0.0048869 and valid. The copies' footprint union
    // U, by hand: the squares S1 = [h, 1 + h] × [0, 1], S2 = [h, 1 + h]², and M, the unit
    // square reflected, by inclusion and exclusion over their convex intersections. The cut
    // sinks each copy half into the plate, removing U / 2; the fuse stands each on it,
    // adding U. Merging the two hairs costs at most 2 × 1.01e-7 mm³ on the fuse, half on
    // the cut, so 1e-6 holds them. FreeCAD 1.1.3, `plate.cut(copies, -1.0)` and
    // `plate.fuse(copies, -1.0)` on the same copies: 99.4630736335 and 101.0738527329
    // (`Shape.Volume`, its fixed order), valid, one solid each.
    let hair = 1e-7 + 1e-9;
    let origin = [1.0664139705275095, -0.0, 0.0];
    let normal = [-0.6477064840690909, -0.7618899595708402, 0.0];
    let placements = [
        moved([hair, -0.0, 0.0]),
        moved([hair, hair, 0.0]),
        Placement::Mirror { origin, normal },
    ];
    let square = |x: f64, y: f64| -> Vec<Point2> {
        vec![[x, y], [x + 1.0, y], [x + 1.0, y + 1.0], [x, y + 1.0]]
    };
    let length = (normal[0] * normal[0] + normal[1] * normal[1]).sqrt();
    let unit = [normal[0] / length, normal[1] / length];
    let reflected = |p: Point2| -> Point2 {
        let d = (p[0] - origin[0]) * unit[0] + (p[1] - origin[1]) * unit[1];
        [p[0] - 2.0 * d * unit[0], p[1] - 2.0 * d * unit[1]]
    };
    // A reflection turns the square clockwise; reversed, counter-clockwise again.
    let mut m: Vec<Point2> = square(0.0, 0.0).into_iter().map(reflected).collect();
    m.reverse();
    let (s1, s2) = (square(hair, 0.0), square(hair, hair));
    let area = |p: &[Point2]| signed_area(p);
    assert_close("M's area", area(&m), 1.0, 1e-12);
    let s1_s2 = clipped(&s1, &s2);
    let union = area(&s1) + area(&s2) + area(&m)
        - area(&s1_s2)
        - area(&clipped(&s1, &m))
        - area(&clipped(&s2, &m))
        + area(&clipped(&s1_s2, &m));
    assert_close("hand's union", union, 1.0738527916, 1e-9);
    let mut k = kernel();
    let slab = Primitive::Box {
        dx: 10.0,
        dy: 10.0,
        dz: 1.0,
    };
    let plate = at(&mut k, slab, [-5.0, -5.0, -1.0]);
    for (op, sunk, want) in [
        (CopiesOp::Cut, -0.5, 100.0 - union / 2.0),
        (CopiesOp::Fuse, 0.0, 100.0 + union),
    ] {
        let tool = at(&mut k, cube(1.0), [0.0, 0.0, sunk]);
        let made = built(&mut k, op, &plate, &tool, &placements);
        assert_eq!(made.history.result.solids, 1, "{op:?}");
        let v = k.summarize(&made.shape).unwrap().volume;
        assert!((v - want).abs() <= 1e-6, "{op:?}: {v}, want {want}");
    }
}

#[test]
fn a_polar_pattern_puts_each_copy_at_its_angle_and_reversed_the_other_way() {
    // Pins r 3, 10 mm tall, 20 mm from the axis, standing in a disk r 30, 5 mm thick: each
    // adds π · 9 · 5 above the disk. A whole turn of four puts copies at 90, 180, and 270;
    // 180° over four at 60, 120, 180; reversed, at −60, −120, −180. Each copy's top centre
    // at 20 (cos θ, sin θ) and 10 up, by hand.
    let mut k = kernel();
    let disk = k
        .make(Primitive::Cylinder {
            radius: 30.0,
            height: 5.0,
        })
        .unwrap();
    let pin = at(
        &mut k,
        Primitive::Cylinder {
            radius: 3.0,
            height: 10.0,
        },
        [20.0, 0.0, 0.0],
    );
    let base = fuse(&mut k, &disk, &pin);
    let root3 = 3f64.sqrt();
    let cases: [(&[f64], [[f64; 2]; 3]); 3] = [
        (
            &[90.0, 180.0, 270.0],
            [[0.0, 20.0], [-20.0, 0.0], [0.0, -20.0]],
        ),
        (
            &[60.0, 120.0, 180.0],
            [[10.0, 10.0 * root3], [-10.0, 10.0 * root3], [-20.0, 0.0]],
        ),
        (
            &[-60.0, -120.0, -180.0],
            [[10.0, -10.0 * root3], [-10.0, -10.0 * root3], [-20.0, 0.0]],
        ),
    ];
    for (angles, tops) in cases {
        let placements: Vec<Placement> = angles.iter().map(|&a| turned(a)).collect();
        let made = built(&mut k, CopiesOp::Fuse, &base, &pin, &placements);
        let s = k.summarize(&made.shape).unwrap();
        assert_close(
            &format!("{angles:?}'s volume"),
            s.volume,
            4500.0 * PI + 4.0 * 45.0 * PI,
            PRECISE,
        );
        let described = k.describe(&made.shape).unwrap();
        for (copy, [x, y]) in (1..).zip(tops) {
            let top = top_of(&described, &made.history, copy, 10.0);
            assert_near(&format!("{angles:?}'s copy {copy}"), top, [x, y, 10.0]);
        }
    }
}

#[test]
fn a_grid_puts_each_copy_i_steps_along_one_direction_and_j_along_the_other() {
    // Pins r 2, 10 mm tall, at (10, 10) on a 100 × 100 × 5 plate; a grid of 3 along X, 20
    // apart, and 2 along Y, 30 apart: instance (i, j) at (20i, 30j), every one but (0, 0)
    // a copy, direction 1 outer. Each copy's top at (10 + 20i, 10 + 30j, 10); six pins of
    // π · 4 · 5 above the plate.
    let mut k = kernel();
    let plate = k
        .make(Primitive::Box {
            dx: 100.0,
            dy: 100.0,
            dz: 5.0,
        })
        .unwrap();
    let pin = at(
        &mut k,
        Primitive::Cylinder {
            radius: 2.0,
            height: 10.0,
        },
        [10.0, 10.0, 0.0],
    );
    let base = fuse(&mut k, &plate, &pin);
    let instances: Vec<(i32, i32)> = (0..3)
        .flat_map(|i| (0..2).map(move |j| (i, j)))
        .filter(|&ij| ij != (0, 0))
        .collect();
    let placements: Vec<Placement> = instances
        .iter()
        .map(|&(i, j)| moved([20.0 * f64::from(i), 30.0 * f64::from(j), 0.0]))
        .collect();
    let made = built(&mut k, CopiesOp::Fuse, &base, &pin, &placements);
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 50000.0 + 6.0 * 20.0 * PI, PRECISE);
    let described = k.describe(&made.shape).unwrap();
    for (copy, (i, j)) in (1..).zip(&instances) {
        let top = top_of(&described, &made.history, copy, 10.0);
        let want = [
            10.0 + 20.0 * f64::from(*i),
            10.0 + 30.0 * f64::from(*j),
            10.0,
        ];
        assert_near(&format!("instance ({i}, {j})"), top, want);
    }
}

#[test]
fn a_turned_and_moved_copy_is_turned_first_then_moved() {
    // A 2 × 4 × 1 block at (10, 0) on a plate, turned 90° about Z, then moved 50 along X:
    // turned, (x, y) becomes (−y, x), so it spans x −4 to 0 and y 10 to 12; moved, x 46 to
    // 50, its top's centre (48, 11, 1). Moved first and turned after, it would be at
    // (−2, 61, 1). Its face at x = 10 (outward −X) faces −Y once turned.
    let mut k = kernel();
    let plate = at(
        &mut k,
        Primitive::Box {
            dx: 100.0,
            dy: 100.0,
            dz: 1.0,
        },
        [-50.0, -50.0, -1.0],
    );
    let block = at(
        &mut k,
        Primitive::Box {
            dx: 2.0,
            dy: 4.0,
            dz: 1.0,
        },
        [10.0, 0.0, 0.0],
    );
    let base = fuse(&mut k, &plate, &block);
    let placement = Placement::TurnedAndMoved {
        axis: Z,
        angle: radians(90.0),
        offset: [50.0, 0.0, 0.0],
    };
    let made = built(&mut k, CopiesOp::Fuse, &base, &block, &[placement]);
    let described = k.describe(&made.shape).unwrap();
    assert_near(
        "the copy's top",
        top_of(&described, &made.history, 1, 1.0),
        [48.0, 11.0, 1.0],
    );
    // The block's Face1 is its x = 10 face (OCCT's box: Face1 at x = 0 of its own).
    let block_faces = k.describe(&block).unwrap().faces;
    assert_near(
        "the block's Face1",
        block_faces[0].centroid,
        [10.0, 2.0, 0.5],
    );
    let face1 = faces_of(&made.history, 1)
        .into_iter()
        .find(|&(n, _)| n == 1)
        .unwrap()
        .1;
    let copy_face1 = &described.faces[face1 as usize - 1];
    assert_near("the copy's Face1", copy_face1.centroid, [48.0, 10.0, 0.5]);
    match copy_face1.surface {
        Surface::Plane { normal, .. } => assert_near("its normal", normal, [0.0, -1.0, 0.0]),
        ref other => panic!("{other:?}"),
    }
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 10000.0 + 2.0 * 8.0, PRECISE);
}

#[test]
fn a_mirrored_copys_faces_are_the_tools_reflected_numbered_as_the_tools() {
    // A 2 mm cube at (5, 0, 0) on a plate, mirrored in x = 0: the copy spans x −7 to −5. Its
    // face n is the tool's face n reflected: centroid (−x, y, z), outward normal (−nx, ny,
    // nz), facing out of the solid as the tool's did. The face standing on the plate is
    // gone into it; every other is kept or modified one for one. Read back by the worker
    // (the summary sends the result again), so it holds no mirroring location.
    let mut k = kernel();
    let plate = at(
        &mut k,
        Primitive::Box {
            dx: 20.0,
            dy: 10.0,
            dz: 1.0,
        },
        [-10.0, 0.0, -1.0],
    );
    let tool = at(&mut k, cube(2.0), [5.0, 0.0, 0.0]);
    let base = fuse(&mut k, &plate, &tool);
    let mirror = Placement::Mirror {
        origin: [0.0; 3],
        normal: [1.0, 0.0, 0.0],
    };
    let made = built(&mut k, CopiesOp::Fuse, &base, &tool, &[mirror]);
    let tools = k.describe(&tool).unwrap().faces;
    let described = k.describe(&made.shape).unwrap();
    let copied = faces_of(&made.history, 1);
    assert_eq!(copied.len(), 5, "{copied:?}");
    for (n, face) in copied {
        let (from, to) = (&tools[n as usize - 1], &described.faces[face as usize - 1]);
        let [x, y, z] = from.centroid;
        assert_near(&format!("Face{n}'s centroid"), to.centroid, [-x, y, z]);
        match (&from.surface, &to.surface) {
            (Surface::Plane { normal: a, .. }, Surface::Plane { normal: b, .. }) => {
                assert_near(&format!("Face{n}'s normal"), *b, [-a[0], a[1], a[2]]);
            }
            other => panic!("Face{n}: {other:?}"),
        }
    }
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 200.0 + 16.0, PRECISE);
    assert!(s.valid);
}

#[test]
fn copies_off_the_base_are_their_own_solids_each_with_its_faces() {
    // A 10 mm cube, and a 2 mm cube's copies at x 20 and x 30, touching nothing: three
    // solids, each with six faces, the base's the ones the base's faces became, and each
    // copy's its own: the core's to judge (decision 6), the kernel's only to say.
    let mut k = kernel();
    let base = k.make(cube(10.0)).unwrap();
    let small = k.make(cube(2.0)).unwrap();
    let made = built(
        &mut k,
        CopiesOp::Fuse,
        &base,
        &small,
        &[moved([20.0, 0.0, 0.0]), moved([30.0, 0.0, 0.0])],
    );
    let history = &made.history;
    assert_eq!(history.result.solids, 3);
    assert_eq!(history.result.faces, 18);
    let solids: BTreeSet<Vec<u32>> = history.topology.solid_faces.iter().cloned().collect();
    let want: BTreeSet<Vec<u32>> = (0..3)
        .map(|input| {
            let mut faces: Vec<u32> = faces_of(history, input)
                .into_iter()
                .map(|(_, f)| f)
                .collect();
            faces.sort_unstable();
            assert_eq!(faces.len(), 6, "input {input}");
            faces
        })
        .collect();
    assert_eq!(solids, want);
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 1000.0 + 16.0, PRECISE);
}

/// A request with every kind of placement: a move, a turn, a mirror, a turned move.
fn mixed(base: &ShapeData, tool: &ShapeData) -> KernelRequest<BuiltData> {
    KernelRequest::copies(
        CopiesOp::Fuse,
        base,
        tool,
        &[
            moved([0.0, 15.0, 0.0]),
            turned(90.0),
            Placement::Mirror {
                origin: [0.0; 3],
                normal: [1.0, 0.0, 0.0],
            },
            Placement::TurnedAndMoved {
                axis: Z,
                angle: radians(180.0),
                offset: [0.0, -10.0, 0.0],
            },
        ],
    )
    .unwrap()
}

#[test]
fn the_same_copies_are_the_same_bytes_every_time_on_any_worker_or_the_pool() {
    // Determinism: a request with a mirror's rebuilt copy, run twice on one worker, once on
    // another, and once through the pool, gives the same B-rep and history, byte for byte.
    let mut k = kernel();
    let plate = at(
        &mut k,
        Primitive::Box {
            dx: 60.0,
            dy: 60.0,
            dz: 2.0,
        },
        [-30.0, -30.0, -2.0],
    );
    let pin = at(
        &mut k,
        Primitive::Cylinder {
            radius: 2.0,
            height: 6.0,
        },
        [12.0, 5.0, 0.0],
    );
    let first = k.run(mixed(&plate, &pin)).unwrap();
    let again = k.run(mixed(&plate, &pin)).unwrap();
    let mut other = kernel();
    let elsewhere = other.run(mixed(&plate, &pin)).unwrap();
    let pool = KernelPool::start_workers(std::path::Path::new(WORKER), 1).unwrap();
    let pooled = match pool
        .submit(Generation(0), mixed(&plate, &pin))
        .wait_timeout(Duration::from_secs(60))
    {
        Ok(Outcome::Done(result)) => result.unwrap(),
        Ok(Outcome::Superseded) => panic!("superseded"),
        Err(_) => panic!("no outcome within a minute"),
    };
    for (what, made) in [
        ("again", &again),
        ("on another worker", &elsewhere),
        ("in the pool", &pooled),
    ] {
        assert_eq!(made.shape, first.shape, "{what}: the B-rep");
        assert_eq!(made.history, first.history, "{what}: the history");
    }
    // Four copies of a pin r 2, 6 tall, standing apart on the plate, at (12, 20), (−5, 12),
    // (−12, 5), and (−12, −15): each adds π · 4 · 6, and the result is one solid.
    let s = k.summarize(&first.shape).unwrap();
    assert_close("volume", s.volume, 7200.0 + 4.0 * 24.0 * PI, PRECISE);
    assert_eq!(first.history.result.solids, 1);
}

#[test]
fn placements_that_leave_a_copy_on_the_tool_are_refused_before_sending() {
    // The core refuses them, naming the placement, and nothing reaches the worker.
    let mut k = kernel();
    let c = k.make(cube(10.0)).unwrap();
    let sent = k.requests_sent();
    for (placements, parameter) in [
        (vec![], "placements"),
        (
            vec![moved([10.0, 0.0, 0.0]), moved([0.0; 3])],
            "placements.1.offset",
        ),
        (vec![turned(360.0)], "placements.0.angle"),
        (
            vec![Placement::Mirror {
                origin: [0.0; 3],
                normal: [0.0, 0.0, 0.5],
            }],
            "placements.0.normal",
        ),
    ] {
        let Err(e) = KernelRequest::copies(CopiesOp::Fuse, &c, &c, &placements) else {
            panic!("{placements:?} wasn't refused");
        };
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{placements:?}");
        assert_eq!(e.parameter(), Some(parameter), "{placements:?}");
    }
    assert_eq!(k.requests_sent(), sent);
}

#[test]
fn an_empty_tool_or_base_is_refused_as_a_booleans_is() {
    // A cut whose tool is an empty compound, as the kernel refuses any boolean's: by role.
    // The kernel's own guard (the core's tools are sweeps and solids), run in-process.
    use silico_kernel::{KernelErrorKind, Refusal, copies, make};
    use silico_protocol::Role;
    let c = make(&cube(10.0)).unwrap();
    let empty = silico_kernel::test_ops::empty_shape().unwrap();
    for (base, tool, role) in [(&c, &empty, Role::Tool), (&empty, &c, Role::Base)] {
        let e = copies(CopiesOp::Cut, base, tool, &[moved([1.0, 0.0, 0.0])])
            .err()
            .unwrap();
        assert_eq!(e.kind(), KernelErrorKind::InvalidInput);
        assert_eq!(
            e.refusal(),
            &Refusal::BooleanInputEmpty {
                op: BooleanOp::Cut,
                role
            }
        );
    }
}
