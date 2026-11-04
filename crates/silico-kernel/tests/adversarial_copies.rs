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

#[test]
fn random_placements_build_a_valid_result_within_the_volumes_they_could_have() {
    // Whatever the placements, a request the check admits gives a valid result whose volume
    // is between the base's and the base's with every copy (a fuse) or with every copy
    // removed (a cut), or a structured refusal of the kernel's; never a crash, never a
    // result the worker can't read back.
    let mut k = kernel();
    let plate = at(&mut k, block(10.0, 10.0, 1.0), [-5.0, -5.0, -1.0]);
    let cube = k.make(block(1.0, 1.0, 1.0)).unwrap();
    let sunk = at(&mut k, block(1.0, 1.0, 1.0), [0.0, 0.0, -0.5]);
    // The test runner's closure is `Fn`: the worker is borrowed in it, a case at a time.
    let worker = std::cell::RefCell::new(k);
    let config = ProptestConfig {
        cases: 1000,
        failure_persistence: None,
        ..ProptestConfig::default()
    };
    proptest!(config, |(
        placements in proptest::collection::vec(placement(), 1..5),
        cut in any::<bool>(),
    )| {
        prop_assume!(silico_protocol::check_copies(&placements).is_ok());
        let n = placements.len() as f64;
        let (op, tool) = if cut { (CopiesOp::Cut, &sunk) } else { (CopiesOp::Fuse, &cube) };
        let mut k = worker.borrow_mut();
        match k.copies(op, &plate, tool, &placements) {
            Ok(made) => {
                prop_assert_eq!(made.shape.valid(), Some(true));
                prop_assert_eq!(made.history.inputs.len(), placements.len() + 1);
                let v = k.summarize(&made.shape).unwrap().volume;
                let slack = 1e-7 * 100.0;
                if cut {
                    prop_assert!(v <= 100.0 + slack && v >= 100.0 - 0.5 * n - slack, "{v}");
                } else {
                    prop_assert!(v >= 100.0 - slack && v <= 100.0 + n + slack, "{v}");
                }
            }
            Err(e) => prop_assert!(
                matches!(e.code(), ErrorCode::KernelFailed | ErrorCode::InvalidResult),
                "{:?}", e
            ),
        }
    });
}
