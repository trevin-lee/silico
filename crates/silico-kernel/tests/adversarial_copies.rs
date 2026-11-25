//! /break-it on the kernel's copies (`Op::Copies`, docs/ports/patterns.md, §7): counts at
//! the cap, a kilometre of length, a turn less a nanodegree, a mirror plane through the
//! tool, copies that overlap and that land on each other, placements at the edges of what's
//! allowed, hostile tools and bases, a worker that lies, and random placements. Each
//! expected value by hand.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f64::consts::{PI, TAU};
use std::path::Path;

use proptest::prelude::*;

use super::close::assert_close;
use super::kernel::kernel;
use crate::wait::WAIT;
use silico_core::ErrorCode;
use silico_core::kernel::{
    Axis, BuiltData, CopiesOp, Generation, KernelRequest, KernelWorker, MAX_COPIES, Op, Placement,
    Primitive, ShapeData,
};
use silico_core::worker::Worker;
use silico_protocol::{BooleanOp, DebugOp, Misbehavior, Reply, radians};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");

const PRECISE: f64 = 1e-9;

const Z: Axis = Axis {
    origin: [0.0; 3],
    direction: [0.0, 0.0, 1.0],
};

fn block(dx: f64, dy: f64, dz: f64) -> Primitive {
    Primitive::Box { dx, dy, dz }
}

fn at(k: &mut KernelWorker, primitive: Primitive, offset: [f64; 3]) -> ShapeData {
    let made = k.make(primitive).unwrap();
    if offset == [0.0; 3] {
        made
    } else {
        k.translate(&made, offset).unwrap().shape
    }
}

fn moved(offset: [f64; 3]) -> Placement {
    Placement::Translation { offset }
}

fn volume(k: &mut KernelWorker, shape: &ShapeData) -> f64 {
    k.summarize(shape).unwrap().volume
}

/// The copies, their result valid and read back by the worker (its summary), or the error.
fn copied(
    k: &mut KernelWorker,
    op: CopiesOp,
    base: &ShapeData,
    tool: &ShapeData,
    placements: &[Placement],
) -> Result<BuiltData, silico_core::Error> {
    k.copies(op, base, tool, placements)
}

#[test]
fn a_thousand_pins_the_most_a_pattern_has_build_in_one_boolean() {
    // A 40 × 25 grid of pins r 0.4, 3 tall, 2 apart, standing 2 into a 80 × 50 × 2 plate:
    // the seed in the base and 999 copies, the cap. Each pin adds π · 0.16 · 1 above the
    // plate. One request, one boolean (about 9 s on a loaded Mac, within the worker's
    // minute; no wall clock is held here, nextest's slow timeout is).
    let mut k = kernel();
    let plate = k.make(block(80.0, 50.0, 2.0)).unwrap();
    let pin = at(
        &mut k,
        Primitive::Cylinder {
            radius: 0.4,
            height: 3.0,
        },
        [1.0, 1.0, 0.0],
    );
    let base = k.boolean(BooleanOp::Fuse, &plate, &pin).unwrap().shape;
    let placements: Vec<Placement> = (0..40)
        .flat_map(|i| (0..25).map(move |j| (i, j)))
        .filter(|&ij| ij != (0, 0))
        .map(|(i, j)| moved([2.0 * f64::from(i), 2.0 * f64::from(j), 0.0]))
        .collect();
    assert_eq!(placements.len(), MAX_COPIES);
    let made = copied(&mut k, CopiesOp::Fuse, &base, &pin, &placements).unwrap();
    assert_eq!(made.history.inputs.len(), 1000);
    assert_eq!(made.history.result.solids, 1);
    assert_close(
        "volume",
        volume(&mut k, &made.shape),
        8000.0 + 1000.0 * 0.16 * PI,
        PRECISE,
    );
    // One more is refused before it's sent.
    let mut over = placements.clone();
    over.push(moved([0.0, 0.0, 10.0]));
    let e = KernelRequest::copies(CopiesOp::Fuse, &base, &pin, &over)
        .err()
        .unwrap();
    assert_eq!(e.parameter(), Some("placements"));
}

#[test]
fn a_thousand_cubes_in_a_row_fuse_face_to_face_into_one_solid() {
    // 1000 unit cubes, each against the next: 1000 mm³, one solid; unrefined, each cube's
    // four sides and the two ends, 4002 faces; four edges along a cube and four round each
    // of the 1001 sections, 8004; four vertices a section, 4004.
    let mut k = kernel();
    let c = k.make(block(1.0, 1.0, 1.0)).unwrap();
    let placements: Vec<Placement> = (1..1000).map(|i| moved([f64::from(i), 0.0, 0.0])).collect();
    let made = copied(&mut k, CopiesOp::Fuse, &c, &c, &placements).unwrap();
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 1000.0, PRECISE);
    assert_eq!((s.faces, s.edges, s.vertices), (4002, 8004, 4004));
    assert_eq!(made.history.topology.solid_faces.len(), 1);
}

#[test]
fn a_copy_a_kilometre_away_is_built_and_a_kilometre_and_an_ulp_is_refused() {
    // A unit cube copied a kilometre along X, and along all three at once: each copy apart,
    // its own solid, the volumes the cubes'.
    let mut k = kernel();
    let c = k.make(block(1.0, 1.0, 1.0)).unwrap();
    let made = copied(
        &mut k,
        CopiesOp::Fuse,
        &c,
        &c,
        &[moved([1e6, 0.0, 0.0]), moved([-1e6, -1e6, 1e6])],
    )
    .unwrap();
    assert_eq!(made.history.result.solids, 3);
    let s = k.summarize(&made.shape).unwrap();
    assert_close("volume", s.volume, 3.0, PRECISE);
    let [low, high] = s.bounds.unwrap();
    assert_close("lowest x", low[0], -1e6, 1e-12);
    assert_close("highest x", high[0], 1e6 + 1.0, 1e-12);
    assert_close("highest z", high[2], 1e6 + 1.0, 1e-12);
    // A turn about an axis a kilometre out swings the copy two kilometres: built.
    let far = Placement::Rotation {
        axis: Axis {
            origin: [1e6, 0.0, 0.0],
            direction: [0.0, 0.0, 1.0],
        },
        angle: PI,
    };
    let made = copied(&mut k, CopiesOp::Fuse, &c, &c, &[far]).unwrap();
    let [low, high] = k.summarize(&made.shape).unwrap().bounds.unwrap();
    assert_close("swung x", high[0], 2e6, 1e-12);
    assert!(low[0].abs() < 1e-6, "{low:?}");
    let past = f64::from_bits(1e6f64.to_bits() + 1);
    let e = KernelRequest::copies(CopiesOp::Fuse, &c, &c, &[moved([past, 0.0, 0.0])])
        .err()
        .unwrap();
    assert_eq!(e.parameter(), Some("placements.0.offset"));
}

#[test]
fn a_turn_a_nanodegree_short_of_whole_lands_the_copy_on_the_original() {
    // A pin r 3 at 20 from the axis turned 360° less a nanodegree: it moves 20 · 1.75e-11 mm,
    // far inside OCCT's tolerance, so the copy is the seed, which the base holds. The fuse
    // changes nothing: the base's volume, valid, one solid (FreeCAD's 359.9999999 copy fuses
    // into the original the same way). A turn within 1e-12 rad of whole is refused.
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
    let base = k.boolean(BooleanOp::Fuse, &disk, &pin).unwrap().shape;
    let before = volume(&mut k, &base);
    assert_close("base", before, 4500.0 * PI + 45.0 * PI, PRECISE);
    for angle in [TAU - radians(1e-9), -(TAU - radians(1e-9))] {
        let turn = Placement::Rotation { axis: Z, angle };
        let made = copied(&mut k, CopiesOp::Fuse, &base, &pin, &[turn]).unwrap();
        assert_eq!(made.shape.valid(), Some(true));
        assert_eq!(made.history.result.solids, 1);
        assert_close("after", volume(&mut k, &made.shape), before, PRECISE);
    }
    for angle in [TAU - 1e-12, TAU, -TAU + 5e-13] {
        let turn = Placement::Rotation { axis: Z, angle };
        let e = KernelRequest::copies(CopiesOp::Fuse, &base, &pin, &[turn])
            .err()
            .unwrap();
        assert_eq!(e.parameter(), Some("placements.0.angle"), "{angle}");
    }
}

#[test]
fn a_mirror_plane_through_the_tool_overlaps_it_by_what_it_shares() {
    // A 4 × 2 × 2 block on a plate, mirrored in planes through it. Through its middle
    // (x = 2): its copy is itself, and the fuse changes nothing. A quarter in (x = 1): the
    // copy spans x −2 to 2, the union x −2 to 4, 6 × 2 × 2. Through its edge (x = 0): the
    // copy x −4 to 0, touching it face to face, the union 8 × 2 × 2. Each valid, one solid.
    let mut k = kernel();
    let plate = at(&mut k, block(20.0, 20.0, 1.0), [-10.0, -10.0, -1.0]);
    let tool = k.make(block(4.0, 2.0, 2.0)).unwrap();
    let base = k.boolean(BooleanOp::Fuse, &plate, &tool).unwrap().shape;
    for (x, union) in [(2.0, 16.0), (1.0, 24.0), (0.0, 32.0), (-1.0, 16.0 + 16.0)] {
        let mirror = Placement::Mirror {
            origin: [x, 0.0, 0.0],
            normal: [1.0, 0.0, 0.0],
        };
        let made = copied(&mut k, CopiesOp::Fuse, &base, &tool, &[mirror]).unwrap();
        assert_eq!(made.history.result.solids, 1, "x = {x}");
        assert_close(
            &format!("x = {x}"),
            volume(&mut k, &made.shape),
            400.0 + union,
            PRECISE,
        );
    }
    // Cut: a hole mirrored in a plane through its axis is the same hole, removing nothing
    // more.
    let drill = at(
        &mut k,
        Primitive::Cylinder {
            radius: 2.0,
            height: 3.0,
        },
        [0.0, 0.0, -2.0],
    );
    let holed = k.boolean(BooleanOp::Cut, &plate, &drill).unwrap().shape;
    let before = volume(&mut k, &holed);
    let made = copied(
        &mut k,
        CopiesOp::Cut,
        &holed,
        &drill,
        &[Placement::Mirror {
            origin: [0.0; 3],
            normal: [0.0, 1.0, 0.0],
        }],
    )
    .unwrap();
    assert_close("hole on hole", volume(&mut k, &made.shape), before, PRECISE);
    assert_close("hand", before, 400.0 - 4.0 * PI, PRECISE);
}

#[test]
fn a_grid_of_overlapping_copies_fuses_into_their_union_and_cuts_it_out() {
    // 2 mm cubes 1 apart in a 3 × 3 grid: each overlapping its neighbours, together the
    // 4 × 4 square, 2 tall: 32 mm³ as a fuse; and as a cut from a 10 × 10 × 2 plate, the
    // copies but not the seed (below). Each copy a tool of its own (one compound of
    // overlapping tools OCCT cuts wrongly).
    let mut k = kernel();
    let cube = k.make(block(2.0, 2.0, 2.0)).unwrap();
    let grid: Vec<Placement> = (0..3)
        .flat_map(|i| (0..3).map(move |j| (i, j)))
        .filter(|&ij| ij != (0, 0))
        .map(|(i, j)| moved([f64::from(i), f64::from(j), 0.0]))
        .collect();
    let made = copied(&mut k, CopiesOp::Fuse, &cube, &cube, &grid).unwrap();
    assert_close("fused", volume(&mut k, &made.shape), 32.0, PRECISE);
    assert_eq!(made.history.result.solids, 1);
    let plate = k.make(block(10.0, 10.0, 2.0)).unwrap();
    let made = copied(&mut k, CopiesOp::Cut, &plate, &cube, &grid).unwrap();
    // The seed isn't in the plate here, so the eight copies alone: the 4 × 4 square less
    // the seed's corner cell [0, 1]², 15 × 2. That cell is cut off on every side but the
    // plate's own, so it's a solid of its own: two solids (FreeCAD 1.1.3: 170, 2 solids).
    assert_close("cut", volume(&mut k, &made.shape), 200.0 - 30.0, PRECISE);
    assert_eq!(made.history.result.solids, 2);
    // Polar copies of a long bar about its own middle, every 10° of a half turn, all
    // overlapping in the middle: a 1 × 1 × 20 bar centred on the Z axis turned 17 times.
    // Valid and one solid, its volume between one bar's and eighteen's.
    let bar = at(&mut k, block(20.0, 1.0, 1.0), [-10.0, -0.5, 0.0]);
    let turns: Vec<Placement> = (1..18)
        .map(|i| Placement::Rotation {
            axis: Z,
            angle: radians(10.0 * f64::from(i)),
        })
        .collect();
    let made = copied(&mut k, CopiesOp::Fuse, &bar, &bar, &turns).unwrap();
    assert_eq!(made.shape.valid(), Some(true));
    assert_eq!(made.history.result.solids, 1);
    let v = volume(&mut k, &made.shape);
    assert!(v > 20.0 && v < 18.0 * 20.0, "{v}");
}

#[test]
fn copies_that_land_on_each_other_or_on_the_seed_fuse_into_one() {
    // Two placements alike: two copies in one place, one copy's volume (OCCT fuses tools
    // that coincide; no guard is needed). A cylinder turned about its own axis: every copy
    // is the seed. A move of a hair: under the boolean's fuzzy value, 1e-6 of the diagonal
    // of the box round the plate, the pin, and the copy (√825 mm: 2.87e-5), OCCT takes the
    // copy for the seed and the fuse changes nothing, valid: 1.1e-7 (past the check's
    // 1e-7), 1e-6, and 2e-5 off. Above it, 5e-5 and 1e-4 off, the union of the two exactly.
    // FreeCAD 1.1.3's LinearPattern of the same pin on the same plate, a hair long, two
    // occurrences: nothing added up to 2e-5, the union from 3e-5 (8e-4 more at 5e-5).
    let mut k = kernel();
    let plate = at(&mut k, block(20.0, 20.0, 1.0), [-10.0, -10.0, -1.0]);
    let pin = k
        .make(Primitive::Cylinder {
            radius: 2.0,
            height: 4.0,
        })
        .unwrap();
    let base = k.boolean(BooleanOp::Fuse, &plate, &pin).unwrap().shape;
    let one = 400.0 + 16.0 * PI;
    let twice = [moved([5.0, 0.0, 0.0]), moved([5.0, 0.0, 0.0])];
    let made = copied(&mut k, CopiesOp::Fuse, &base, &pin, &twice).unwrap();
    assert_close(
        "twice",
        volume(&mut k, &made.shape),
        one + 16.0 * PI,
        PRECISE,
    );
    let spun: Vec<Placement> = [90.0, 180.0, 270.0]
        .iter()
        .map(|&a| Placement::Rotation {
            axis: Z,
            angle: radians(a),
        })
        .collect();
    let made = copied(&mut k, CopiesOp::Fuse, &base, &pin, &spun).unwrap();
    assert_close("spun", volume(&mut k, &made.shape), one, PRECISE);
    // The union of two discs r 2 whose centres are `hair` apart is π·4 + 4·hair to first
    // order (the rest under 1e-12), 4 tall.
    for (hair, more) in [
        (1.1e-7, 0.0),
        (1e-6, 0.0),
        (2e-5, 0.0),
        (5e-5, 4.0 * 4.0 * 5e-5),
        (1e-4, 4.0 * 4.0 * 1e-4),
    ] {
        let made = copied(
            &mut k,
            CopiesOp::Fuse,
            &base,
            &pin,
            &[moved([hair, 0.0, 0.0])],
        )
        .unwrap();
        assert_eq!(made.shape.valid(), Some(true), "{hair}");
        assert_close(
            &format!("{hair}"),
            volume(&mut k, &made.shape),
            one + more,
            1e-12,
        );
    }
}

#[test]
fn a_copy_spread_far_grows_the_fuzzy_value_and_parts_a_near_one_from_the_plate() {
    // The pin r 2, 4 tall on the 20 × 20 plate, copied 1e-3 along x: the box round the
    // plate and the copies is √825 mm, f = 2.87e-5, and the copy adds its sliver of the
    // union, 16 × 1e-3, one solid. With one more copy a kilometre off, the box is a
    // kilometre and f about 1 mm: the pin and its near copy, a millimetre's thousandth
    // apart, are one, and the volume is right, the plate, one pin, and the far pin, 400 +
    // 32π; but the pin comes apart from the plate it stood on: three solids, the plate's 6
    // faces and two pins' 3, where the true union is two. FreeCAD 1.1.3 does the same, by
    // `fuse([near, far], -1.0)` and by a MultiTransform of two LinearPatterns (three
    // solids, 400 + 32π). Found by break-it on the fuzzy value; a spec question for the
    // pattern feature, which reads each result solid's faces to say which copies lie off
    // the base (docs/ports/patterns.md): here the seed's own pin is a solid apart.
    let mut k = kernel();
    let plate = at(&mut k, block(20.0, 20.0, 1.0), [-10.0, -10.0, -1.0]);
    let pin = k
        .make(Primitive::Cylinder {
            radius: 2.0,
            height: 4.0,
        })
        .unwrap();
    let base = k.boolean(BooleanOp::Fuse, &plate, &pin).unwrap().shape;
    let one = 400.0 + 16.0 * PI;
    let near = moved([1e-3, 0.0, 0.0]);
    let made = copied(&mut k, CopiesOp::Fuse, &base, &pin, &[near]).unwrap();
    assert_eq!(made.history.result.solids, 1);
    assert_close(
        "near",
        volume(&mut k, &made.shape),
        one + 16.0 * 1e-3,
        1e-12,
    );
    let far = moved([1e6, 0.0, 0.0]);
    let made = copied(&mut k, CopiesOp::Fuse, &base, &pin, &[near, far]).unwrap();
    assert_eq!(made.shape.valid(), Some(true));
    let faces: Vec<usize> = made
        .history
        .topology
        .solid_faces
        .iter()
        .map(Vec::len)
        .collect();
    assert_eq!(faces, [3, 6, 3]);
    assert_close(
        "near and far",
        volume(&mut k, &made.shape),
        one + 16.0 * PI,
        1e-12,
    );
}

#[test]
fn every_value_the_check_refuses_the_worker_refuses_too_naming_it() {
    // Sent past the core's own check, as a raw operation: the worker checks again and
    // refuses each, naming the placement and its field, and goes on working.
    let mut k = kernel();
    let c = k.make(block(1.0, 1.0, 1.0)).unwrap();
    let mut w = Worker::spawn(Path::new(WORKER)).unwrap();
    let nearly_unit = 1.0 + 8.0 * f64::EPSILON;
    for (placements, parameter) in [
        (vec![], "placements"),
        (vec![moved([1.0, 0.0, 0.0]); MAX_COPIES + 1], "placements"),
        (vec![moved([f64::NAN, 0.0, 0.0])], "placements.0.offset"),
        (vec![moved([5e-8, 5e-8, 5e-8])], "placements.0.offset"),
        (
            vec![
                moved([2.0, 0.0, 0.0]),
                Placement::Rotation {
                    axis: Axis {
                        origin: [0.0; 3],
                        direction: [0.0, 0.0, nearly_unit],
                    },
                    angle: 1.0,
                },
            ],
            "placements.1.axis",
        ),
        (
            vec![Placement::Rotation {
                axis: Z,
                angle: f64::INFINITY,
            }],
            "placements.0.angle",
        ),
        (
            vec![Placement::Mirror {
                origin: [0.0, 0.0, 1e300],
                normal: [1.0, 0.0, 0.0],
            }],
            "placements.0.origin",
        ),
        (
            vec![Placement::Mirror {
                origin: [0.0; 3],
                normal: [1e300, 0.0, 1e300],
            }],
            "placements.0.normal",
        ),
        (
            vec![Placement::TurnedAndMoved {
                axis: Z,
                angle: 1.0,
                offset: [0.0, 0.0, -0.0],
            }],
            "placements.0.offset",
        ),
    ] {
        let op = Op::Copies {
            base: c.clone(),
            tool: c.clone(),
            op: CopiesOp::Fuse,
            placements,
        };
        let e = w.call(Generation(0), op, WAIT).unwrap_err();
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{parameter}: {e:?}");
        assert_eq!(e.parameter(), Some(parameter), "{e:?}");
    }
    // Still working: a good request after them.
    let good = Op::Copies {
        base: c.clone(),
        tool: c.clone(),
        op: CopiesOp::Fuse,
        placements: vec![moved([1.0, 0.0, 0.0])],
    };
    assert!(matches!(
        w.call(Generation(0), good, WAIT).unwrap(),
        Reply::Built(_)
    ));
}

#[test]
fn hostile_tools_and_bases_are_refused_or_built_never_crashing() {
    let mut k = kernel();
    let c = k.make(block(2.0, 2.0, 2.0)).unwrap();
    // A shape that isn't OCCT's: refused as the bytes they are.
    let junk = ShapeData::unchecked(silico_core::kernel::Blob::inline(vec![1, 2, 3, 4, 5]));
    for (base, tool) in [(&junk, &c), (&c, &junk)] {
        let e = copied(
            &mut k,
            CopiesOp::Fuse,
            base,
            tool,
            &[moved([3.0, 0.0, 0.0])],
        )
        .unwrap_err();
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
    }
    // A cut that removes the whole base: refused as an empty result.
    let e = copied(&mut k, CopiesOp::Cut, &c, &c, &[moved([1e-6, 0.0, 0.0])]);
    match e {
        Err(e) => assert_eq!(e.code(), ErrorCode::KernelFailed, "{e:?}"),
        // Or, OCCT's way with a sliver: what's left is the 1e-6 slab the copy missed.
        Ok(made) => assert_close("sliver", volume(&mut k, &made.shape), 4e-6, 1e-6),
    }
    let big = at(&mut k, block(6.0, 6.0, 6.0), [-2.0, -2.0, -2.0]);
    let e = copied(&mut k, CopiesOp::Cut, &c, &big, &[moved([0.5, 0.0, 0.0])]).unwrap_err();
    assert_eq!(e.code(), ErrorCode::KernelFailed, "{e:?}");
    assert_eq!(e.message(), "CutCopies failed");
    assert_eq!(e.cause(), Some("the cut's result is empty"));
    // A mirrored result copied by a mirror again: the double reflection reads back as the
    // original's place, its volume the two cubes'.
    let in_x = Placement::Mirror {
        origin: [-1.0, 0.0, 0.0],
        normal: [1.0, 0.0, 0.0],
    };
    let once = copied(&mut k, CopiesOp::Fuse, &c, &c, &[in_x]).unwrap();
    assert_close("once", volume(&mut k, &once.shape), 16.0, PRECISE);
    let twice = copied(&mut k, CopiesOp::Fuse, &once.shape, &once.shape, &[in_x]).unwrap();
    assert_close("twice", volume(&mut k, &twice.shape), 16.0, PRECISE);
    // A tool of two solids (a two-region pocket's): each copy's every solid a tool of its
    // own. Two 1 mm cubes 3 apart, as one tool, cut twice from a plate.
    let a = k.make(block(1.0, 1.0, 1.0)).unwrap();
    let b = at(&mut k, block(1.0, 1.0, 1.0), [3.0, 0.0, 0.0]);
    let pair = k.boolean(BooleanOp::Fuse, &a, &b).unwrap();
    assert_eq!(pair.history.result.solids, 2);
    let plate = at(&mut k, block(20.0, 20.0, 1.0), [-1.0, -1.0, 0.0]);
    let made = copied(
        &mut k,
        CopiesOp::Cut,
        &plate,
        &pair.shape,
        &[moved([0.0, 5.0, 0.0]), moved([0.0, 10.0, 0.0])],
    )
    .unwrap();
    assert_close(
        "pocketed",
        volume(&mut k, &made.shape),
        400.0 - 4.0,
        PRECISE,
    );
    assert_eq!(made.history.inputs[1].faces.len(), 12);
}

#[test]
fn a_worker_whose_copies_history_is_bad_is_refused_and_stopped() {
    // The test worker misnumbers its replies' histories: the core refuses the copies as a
    // protocol error and stops the worker.
    for how in [Misbehavior::BadHistory, Misbehavior::HistoryForNoInputs] {
        let mut w = Worker::spawn(Path::new(TEST_WORKER)).unwrap();
        w.call(Generation(0), Op::Debug(DebugOp::Misbehave(how)), WAIT)
            .unwrap();
        let mut k = KernelWorker::from_worker(w);
        let c = k.make(block(1.0, 1.0, 1.0)).unwrap();
        let e = k
            .copies(CopiesOp::Fuse, &c, &c, &[moved([1.0, 0.0, 0.0])])
            .unwrap_err();
        assert_eq!(e.code(), ErrorCode::ProtocolError, "{how:?}: {e:?}");
        let after = k.summarize(&c).unwrap_err();
        assert_eq!(after.code(), ErrorCode::WorkerCrashed, "{after}");
    }
}

/// A placement of a unit cube near a 10 × 10 × 1 plate's top: moved, turned about a
/// vertical axis through the plate, mirrored in an upright plane, or turned and moved; the
/// values biased to the edges (whole millimetres, quarter turns, and a hair past them).
fn placement() -> impl Strategy<Value = Placement> {
    let coordinate = prop_oneof![
        (-4i32..=4).prop_map(f64::from),
        -4.0..4.0f64,
        Just(1e-7 + 1e-9),
        Just(-0.0),
    ];
    let angle = prop_oneof![
        (1..8i32).prop_map(|q| radians(45.0 * f64::from(q))),
        1e-11..TAU - 1e-11,
        Just(TAU - radians(1e-9)),
    ];
    let axis = (coordinate.clone(), coordinate.clone()).prop_map(|(x, y)| Axis {
        origin: [x, y, 0.0],
        direction: [0.0, 0.0, 1.0],
    });
    let normal = (0.0..TAU).prop_map(|t| [libm::cos(t), libm::sin(t), 0.0]);
    let offset = (coordinate.clone(), coordinate.clone()).prop_map(|(x, y)| [x, y, 0.0]);
    prop_oneof![
        offset
            .clone()
            .prop_map(|offset| Placement::Translation { offset }),
        (axis.clone(), angle.clone()).prop_map(|(axis, angle)| Placement::Rotation { axis, angle }),
        ((coordinate.clone(), coordinate), normal).prop_map(|((x, y), normal)| Placement::Mirror {
            origin: [x, y, 0.0],
            normal,
        }),
        (axis, angle, offset).prop_map(|(axis, angle, offset)| Placement::TurnedAndMoved {
            axis,
            angle,
            offset,
        }),
    ]
}

/// OCCT's confusion, `Precision::Confusion()`: points closer than this are one point to it,
/// with a fuzzy value or without, and each input's elements carry it as their tolerance.
const CONFUSION: f64 = 1e-7;

/// Where `placement` puts the unit square [0, 1]², the unit cube's footprint, by hand: its
/// corners in order. Every axis the random placements turn about is vertical and every
/// mirror plane upright, so a copy is its footprint between the tool's own two heights.
fn footprint(placement: &Placement) -> [[f64; 2]; 4] {
    let turned = |[x, y]: [f64; 2], [cx, cy, _]: [f64; 3], angle: f64| {
        let (sin, cos) = (libm::sin(angle), libm::cos(angle));
        let (dx, dy) = (x - cx, y - cy);
        [cx + dx * cos - dy * sin, cy + dx * sin + dy * cos]
    };
    [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|p| match *placement {
        Placement::Translation { offset } => [p[0] + offset[0], p[1] + offset[1]],
        Placement::Rotation { axis, angle } => turned(p, axis.origin, angle),
        Placement::Mirror { origin, normal } => {
            let d = (p[0] - origin[0]) * normal[0] + (p[1] - origin[1]) * normal[1];
            [p[0] - 2.0 * d * normal[0], p[1] - 2.0 * d * normal[1]]
        }
        Placement::TurnedAndMoved {
            axis,
            angle,
            offset,
        } => {
            let [x, y] = turned(p, axis.origin, angle);
            [x + offset[0], y + offset[1]]
        }
    })
}

/// The boolean's fuzzy value, by hand: 1e-6 of the diagonal of the box round every input
/// (FreeCAD 1.1.3's automatic value, docs/ports/booleans.md), the footprints' box in x and
/// y, and `height` in z.
fn fuzzy_value(footprints: &[[[f64; 2]; 4]], height: f64) -> f64 {
    let corners = || footprints.iter().flatten();
    let span = |i: usize| {
        let low = corners().map(|c| c[i]).fold(f64::INFINITY, f64::min);
        let high = corners().map(|c| c[i]).fold(f64::NEG_INFINITY, f64::max);
        high - low
    };
    1e-6 * (span(0) * span(0) + span(1) * span(1) + height * height).sqrt()
}

fn minus(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn length(a: [f64; 2]) -> f64 {
    libm::hypot(a[0], a[1])
}

/// The distance from `p` to the segment from `a` to `b`.
fn to_segment(p: [f64; 2], [a, b]: [[f64; 2]; 2]) -> f64 {
    let ab = minus(b, a);
    let t = ((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / (ab[0] * ab[0] + ab[1] * ab[1]);
    let t = t.clamp(0.0, 1.0);
    length(minus(p, [a[0] + t * ab[0], a[1] + t * ab[1]]))
}

/// The least distance between two segments.
fn between(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> f64 {
    let (da, db) = (minus(a[1], a[0]), minus(b[1], b[0]));
    let across = cross(da, db);
    if across != 0.0 {
        let t = cross(minus(b[0], a[0]), db) / across;
        let u = cross(minus(b[0], a[0]), da) / across;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            return 0.0;
        }
    }
    [
        to_segment(a[0], b),
        to_segment(a[1], b),
        to_segment(b[0], a),
        to_segment(b[1], a),
    ]
    .into_iter()
    .fold(f64::INFINITY, f64::min)
}

fn sides(polygon: &[[f64; 2]; 4]) -> impl Iterator<Item = [[f64; 2]; 2]> + '_ {
    (0..4).map(|i| [polygon[i], polygon[(i + 1) % 4]])
}

/// Where two inputs' sides (the footprints' edges, every input upright between heights
/// whole or half millimetres apart) come within `t` of each other, or of a third input's,
/// without lying on each other (within [`CONFUSION`]), `t` being how far OCCT's boolean
/// reaches. Two sides crossing at an angle θ are within `t` of each other for `t / sin θ`
/// either way along them: a corner of either in that stretch is within `t` of the other
/// side. The vertex OCCT makes of the crossing takes that stretch as its tolerance, so it
/// reaches a third input's side within `t / sin θ + t` of the crossing. Parallel sides, or
/// sides so nearly parallel that the stretch spans one, reach each other within `t`. Says
/// what's within reach, or `None`.
fn within_reach(footprints: &[[[f64; 2]; 4]], t: f64) -> Option<String> {
    let apart = |d: f64| d > CONFUSION;
    for (i, first) in footprints.iter().enumerate() {
        for (j, second) in footprints.iter().enumerate().skip(i + 1) {
            for a in sides(first) {
                for b in sides(second) {
                    let (da, db) = (minus(a[1], a[0]), minus(b[1], b[0]));
                    let sin = cross(da, db).abs() / (length(da) * length(db));
                    let reach = t / sin;
                    if reach >= length(da).min(length(db)) {
                        let ends = [(a[0], b), (a[1], b), (b[0], a), (b[1], a)];
                        if let Some(d) = ends
                            .iter()
                            .map(|&(p, side)| to_segment(p, side))
                            .find(|&d| apart(d) && d <= t)
                        {
                            return Some(format!("inputs {i} and {j} have sides {d:e} apart"));
                        }
                        continue;
                    }
                    if between(a, b) > t {
                        continue;
                    }
                    let along = cross(minus(b[0], a[0]), db) / cross(da, db);
                    let crossing = [a[0][0] + along * da[0], a[0][1] + along * da[1]];
                    for end in [a[0], a[1], b[0], b[1]] {
                        let d = length(minus(end, crossing));
                        if apart(d) && d <= reach {
                            return Some(format!(
                                "inputs {i} and {j} cross at {crossing:?}, a corner {d:e} off"
                            ));
                        }
                    }
                    for (k, third) in footprints.iter().enumerate() {
                        if k == i || k == j {
                            continue;
                        }
                        let d = sides(third)
                            .map(|side| to_segment(crossing, side))
                            .fold(f64::INFINITY, f64::min);
                        if apart(d) && d <= reach + t {
                            return Some(format!(
                                "inputs {i} and {j} cross at {crossing:?} at {:.3}°, {d:e} from input {k}",
                                libm::asin(sin).to_degrees()
                            ));
                        }
                    }
                }
            }
        }
    }
    None
}

/// Where OCCT's boolean under the fuzzy value `fuzzy` can give a valid result that's wrong,
/// as FreeCAD 1.1.3's `-1.0` booleans do on the same inputs (docs/ports/booleans.md, Known
/// wrong results): inputs' sides within twice its reach of each other, or of a third's,
/// that don't lie on each other ([`within_reach`]). Its reach, from OCCT's tolerances
/// (`BOPDS_DS::Init` adds half the fuzzy value to every element's own 1e-7), is `fuzzy +
/// 2e-7`: what's within it is merged, and FreeCAD 1.1.3's cuts at a swept fuzzy value
/// break where it reaches, to its second digit (`the_reach_is_where_freecad_breaks`). Just
/// past it OCCT can be wrong too, as parallel faces are up to about 1.7 times it
/// (`boxes_with_flat_faces_just_past_the_fuzzy_value_are_right_or_occts_known_wrong_answer`)
/// and two corners are at 1.004 times it (the third counterexample below): twice it.
fn merged_unmet(footprints: &[[[f64; 2]; 4]], fuzzy: f64) -> Option<String> {
    within_reach(footprints, 2.0 * (fuzzy + 2.0 * CONFUSION))
}

/// The 10 × 10 × 1 plate the random placements' copies go on, its top at z = 0; the unit
/// cube on it (fused); and the unit cube sunk half into it (cut).
fn plate_and_tools(k: &mut KernelWorker) -> (ShapeData, ShapeData, ShapeData) {
    let plate = at(k, block(10.0, 10.0, 1.0), [-5.0, -5.0, -1.0]);
    let cube = k.make(block(1.0, 1.0, 1.0)).unwrap();
    let sunk = at(k, block(1.0, 1.0, 1.0), [0.0, 0.0, -0.5]);
    (plate, cube, sunk)
}

/// The property, for one case: a request the check admits gives a valid result whose
/// volume is between the base's and the base's with every copy (a fuse) or with every copy
/// removed (a cut), or a structured refusal of the kernel's; never a crash, never a result
/// the worker can't read back. Where the fuzzy value merges inputs' sides that don't meet
/// ([`merged_unmet`]), the volume isn't held: OCCT can be wrong there, as FreeCAD 1.1.3 is
/// on the same inputs, and Silico takes its answer as FreeCAD does (the counterexamples
/// below, each pinned by hand).
fn copies_are_sound(
    k: &mut KernelWorker,
    (plate, cube, sunk): &(ShapeData, ShapeData, ShapeData),
    placements: &[Placement],
    cut: bool,
) -> Result<(), TestCaseError> {
    let n = placements.len() as f64;
    let (op, tool) = if cut {
        (CopiesOp::Cut, sunk)
    } else {
        (CopiesOp::Fuse, cube)
    };
    match k.copies(op, plate, tool, placements) {
        Ok(made) => {
            prop_assert_eq!(made.shape.valid(), Some(true));
            prop_assert_eq!(made.history.inputs.len(), placements.len() + 1);
            let v = k.summarize(&made.shape).unwrap().volume;
            let footprints = plate_and(placements);
            // The plate from z −1 to 0, the cube on it to 1 or the sunk one to 0.5.
            let fuzzy = fuzzy_value(&footprints, if cut { 1.5 } else { 2.0 });
            if merged_unmet(&footprints, fuzzy).is_some() {
                return Ok(());
            }
            let slack = 1e-7 * 100.0;
            if cut {
                prop_assert!(v <= 100.0 + slack && v >= 100.0 - 0.5 * n - slack, "{v}");
            } else {
                prop_assert!(v >= 100.0 - slack && v <= 100.0 + n + slack, "{v}");
            }
        }
        Err(e) => prop_assert!(
            matches!(e.code(), ErrorCode::KernelFailed | ErrorCode::InvalidResult),
            "{:?}",
            e
        ),
    }
    Ok(())
}

#[test]
fn random_placements_build_a_valid_result_within_the_volumes_they_could_have() {
    let mut k = kernel();
    let inputs = plate_and_tools(&mut k);
    // The test runner's closure is `Fn`: the worker is borrowed in it, a case at a time.
    let worker = std::cell::RefCell::new(k);
    let config = ProptestConfig {
        cases: 1000,
        failure_persistence: None,
        // Fixed, so a red run is red every time: unseeded, each run drew 1,000 new cases,
        // and the next run lost a counterexample one had found (those below).
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x00c0_b1e5),
        ..ProptestConfig::default()
    };
    proptest!(config, |(
        placements in proptest::collection::vec(placement(), 1..5),
        cut in any::<bool>(),
    )| {
        prop_assume!(silico_protocol::check_copies(&placements).is_ok());
        copies_are_sound(&mut worker.borrow_mut(), &inputs, &placements, cut)?;
    });
}

/// A turn about a vertical axis through (x, y).
fn turn(x: f64, y: f64, angle: f64) -> Placement {
    Placement::Rotation {
        axis: Axis {
            origin: [x, y, 0.0],
            direction: [0.0, 0.0, 1.0],
        },
        angle,
    }
}

/// A counterexample the random placements found, before their volume was held only out of
/// the fuzzy value's reach: OCCT's answer under FreeCAD 1.1.3's automatic fuzzy value is
/// valid and wrong, and FreeCAD 1.1.3's `plate.cut(copies, -1.0)` or `fuse` gives the same.
struct Counterexample {
    what: &'static str,
    placements: Vec<Placement>,
    cut: bool,
    /// The plate with the copies' footprints within it removed half a millimetre deep (a
    /// cut) or added a millimetre high (a fuse), by hand: each footprint clipped to the
    /// plate and to the others, inclusion and exclusion, in exact rationals from the
    /// corners as [`footprint`] places them. FreeCAD 1.1.3 with no fuzzy value, `cut` or
    /// `fuse(copies, 0.0)`, gives it too (to 1e-11, by its fixed-order measure).
    right: f64,
    /// OCCT's, Silico's and FreeCAD 1.1.3's `-1.0` (to 1e-13).
    occts: f64,
    /// The fuzzy value at which FreeCAD 1.1.3's boolean is last right and first wrong, swept
    /// by hand; `None` where it breaks short of the reach, and right again past it.
    breaks: Option<(f64, f64)>,
}

fn counterexamples() -> [Counterexample; 4] {
    [
        Counterexample {
            // The sunk cube mirrored in an upright plane through (−4, 1.01e-7) at nearly 45°:
            // the copy lands on the plate's corner, [−5, −4]² turned 4.2e-4 rad, one corner
            // 5.34e-7 inside the plate's side y = −5. More is removed than the copy holds of
            // the plate (0.50043 for 0.49990).
            what: "a mirrored copy a hair inside the plate's side",
            placements: vec![Placement::Mirror {
                origin: [-4.0, 1.009_999_999_999_999_9e-7, 0.0],
                normal: [0.706_959_694_594_565, 0.707_253_837_189_137_7, 0.0],
            }],
            cut: true,
            right: 99.500_103_728_468_83,
            occts: 99.499_569_129_073_32,
            breaks: Some((3.2e-7, 3.4e-7)),
        },
        Counterexample {
            // Three turned copies of the sunk cube: about (1.0057, 1.01e-7) by 348.7°, about
            // (1, 0) by 262.2°, and a nanodegree short of a whole turn about the origin (the
            // seed's place). The first two's sides cross at 3.51°, 2.41e-4 from the third's
            // side x = 1, and the plate is all but lost, two solids. Any two of the three
            // are right (`each_counterexample_is_right_or_occts_known_wrong_answer`).
            what: "three turned copies, two crossing near the third",
            placements: three_turns().to_vec(),
            cut: true,
            right: 98.924_888_020_692_8,
            occts: 0.575_111_610_664_817_3,
            breaks: Some((1.37e-5, 1.4e-5)),
        },
        Counterexample {
            // The cube mirrored in an upright plane through (−3.0344, 1.01e-7) whose normal is
            // 1.55e-3 rad off y, and one a nanodegree short of a whole turn: the mirror
            // overlaps it by 1.1e-2 and crosses its sides x = 0 and x = 1 at 3.1e-3 rad,
            // either corner 1.4541e-5 from the other's side, 1.004 times the boolean's reach
            // (f 1.4283e-5): just past it. The fuse loses both copies and some plate.
            // FreeCAD 1.1.3 is right up to 1e-5, invalid at 1.2e-5 and 1.4e-5, 2.1e-3 off
            // at 1.43e-5, and right again from 1.45e-5, where the corners are within reach.
            what: "a mirrored copy crossing the seed's place at 3.1e-3 rad",
            placements: vec![
                Placement::Mirror {
                    origin: [-3.034_440_015_031_443_7, 1.009_999_999_999_999_9e-7, 0.0],
                    normal: [-0.001_547_871_816_311_317_7, 0.999_998_802_045_702_6, 0.0],
                },
                turn(0.0, 0.0, TAU - radians(1e-9)),
            ],
            cut: false,
            right: 101.989_058_133_011,
            occts: 99.995_729_961_765_8,
            breaks: None,
        },
        Counterexample {
            // The sunk cube a nanodegree short of a whole turn about (1.01e-7, 1.01e-7) and
            // moved 2 along −x, and turned 269.92° about (−4, 1.01e-7): the second lands on
            // [−4, −3] × [−5, −4] turned 1.46e-3 rad, one corner 5.44e-6 inside the plate's
            // side y = −5. More is removed than the two hold of the plate.
            what: "a turned copy a hair inside the plate's side",
            placements: vec![
                Placement::TurnedAndMoved {
                    axis: Axis {
                        origin: [1.009_999_999_999_999_9e-7, 1.009_999_999_999_999_9e-7, 0.0],
                        direction: [0.0, 0.0, 1.0],
                    },
                    angle: TAU - radians(1e-9),
                    offset: [-2.0, -0.0, 0.0],
                },
                turn(-4.0, 1.009_999_999_999_999_9e-7, 4.710_927_585_761_918),
            ],
            cut: true,
            right: 99.000_362_633_809_93,
            occts: 98.998_296_060_062_47,
            breaks: Some((5e-6, 6e-6)),
        },
    ]
}

/// The second counterexample's three turns.
fn three_turns() -> [Placement; 3] {
    [
        turn(
            1.005_659_416_672_686_4,
            1.009_999_999_999_999_9e-7,
            6.086_284_090_324_141,
        ),
        turn(1.0, 0.0, 4.576_791_851_566_414),
        turn(0.0, 0.0, 6.283_185_307_162_133),
    ]
}

#[test]
fn each_counterexample_is_sound() {
    let mut k = kernel();
    let inputs = plate_and_tools(&mut k);
    let mut unsound = Vec::new();
    for c in counterexamples() {
        assert_eq!(
            silico_protocol::check_copies(&c.placements),
            Ok(()),
            "{}",
            c.what
        );
        if let Err(e) = copies_are_sound(&mut k, &inputs, &c.placements, c.cut) {
            unsound.push(format!("{}: {e}", c.what));
        }
    }
    assert!(unsound.is_empty(), "{unsound:#?}");
}

#[test]
fn each_counterexample_is_right_or_occts_known_wrong_answer() {
    // Silico takes OCCT's answer, as FreeCAD does (docs/ports/booleans.md, Known wrong
    // results): each is right, or that wrong one.
    let mut k = kernel();
    let (plate, cube, sunk) = plate_and_tools(&mut k);
    let mut built = |placements: &[Placement], cut: bool| {
        let (op, tool) = if cut {
            (CopiesOp::Cut, &sunk)
        } else {
            (CopiesOp::Fuse, &cube)
        };
        let made = copied(&mut k, op, &plate, tool, placements).unwrap();
        assert_eq!(made.shape.valid(), Some(true), "{placements:?}");
        volume(&mut k, &made.shape)
    };
    for c in counterexamples() {
        let v = built(&c.placements, c.cut);
        assert!(
            close(v, c.right) || close(v, c.occts),
            "{}: {v}, right {}, OCCT's known {}",
            c.what,
            c.right,
            c.occts
        );
    }
    // Any two of the three turns are right, so it's the three together: the pairs' unions
    // by hand as above.
    let [first, second, third] = three_turns();
    for (pair, right) in [
        ([first, second], 99.015_288_105_985_46),
        ([second, third], 99.0),
        ([first, third], 99.409_599_914_707_34),
    ] {
        assert_close(&format!("{pair:?}"), built(&pair, true), right, 1e-12);
    }
}

fn close(got: f64, want: f64) -> bool {
    (got - want).abs() <= 1e-9 * want.abs()
}

/// The plate's footprint, then those of the copies `placements` put on it.
fn plate_and(placements: &[Placement]) -> Vec<[[f64; 2]; 4]> {
    let mut footprints = vec![[[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]]];
    footprints.extend(placements.iter().map(footprint));
    footprints
}

#[test]
fn the_reach_is_where_freecad_breaks() {
    // FreeCAD 1.1.3's boolean of each counterexample at fuzzy values f swept by hand is
    // right up to one and wrong from the next; its reach, f + 2e-7, reaches across the
    // gap that makes it wrong between them: the mirrored copy's corner 5.34e-7 inside the
    // plate's side (from f = 3.34e-7), the turns' crossing at 3.51°, 2.41e-4 from the
    // third's side (t / sin θ + t, from f = 1.373e-5), the turned copy's corner 5.44e-6
    // inside the side (from f = 5.24e-6).
    for c in counterexamples() {
        let footprints = plate_and(&c.placements);
        let height = if c.cut { 1.5 } else { 2.0 };
        if let Some((right, wrong)) = c.breaks {
            let reach = |f: f64| within_reach(&footprints, f + 2.0 * CONFUSION);
            assert_eq!(reach(right), None, "{} at {right}", c.what);
            assert!(reach(wrong).is_some(), "{} at {wrong}", c.what);
        } else {
            // Just past the reach at the value the copies run with, and within twice it.
            let fuzzy = fuzzy_value(&footprints, height);
            assert_eq!(
                within_reach(&footprints, fuzzy + 2.0 * CONFUSION),
                None,
                "{}",
                c.what
            );
        }
        // At the value the copies run with, 1e-6 of the box round the plate and them, each
        // is within twice the reach.
        let fuzzy = fuzzy_value(&footprints, height);
        assert!((1.42e-5..1.43e-5).contains(&fuzzy), "{}: {fuzzy}", c.what);
        assert!(merged_unmet(&footprints, fuzzy).is_some(), "{}", c.what);
    }
    // What lies exactly on each other isn't merged: a copy beside the seed's place, one
    // turned a quarter about its corner, one a nanodegree short of a whole turn, and one
    // mirrored onto the plate's corner exactly, sides and corners shared to the rounding.
    let exact = plate_and(&[
        moved([2.0, 0.0, 0.0]),
        turn(1.0, 0.0, PI / 2.0),
        turn(0.0, 0.0, TAU - radians(1e-9)),
        Placement::Mirror {
            origin: [-4.0, 0.0, 0.0],
            normal: [
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
                0.0,
            ],
        },
    ]);
    assert_eq!(merged_unmet(&exact, fuzzy_value(&exact, 1.5)), None);
    // A hair apart: a copy 1.01e-7 past another's side (parallel), or a copy turned 45°
    // with its lowest corner 1e-6 inside the plate's side.
    let hair = plate_and(&[moved([2.0, 0.0, 0.0]), moved([3.0 + 1.01e-7, 0.0, 0.0])]);
    let reason = merged_unmet(&hair, fuzzy_value(&hair, 1.5)).unwrap();
    assert!(reason.contains("sides 1.0"), "{reason}");
    let inside = plate_and(&[Placement::TurnedAndMoved {
        axis: Z,
        angle: PI / 4.0,
        offset: [0.0, -5.0 + 1e-6, 0.0],
    }]);
    let reason = merged_unmet(&inside, fuzzy_value(&inside, 1.5)).unwrap();
    assert!(reason.contains("a corner"), "{reason}");
}
