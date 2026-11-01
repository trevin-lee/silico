//! M2.3: the in-memory kernel (silico_core::fake) answers as the real one does where it
//! claims to. The same requests go through the `Kernel` trait to the worker pool and to the
//! fake: each primitive has the same faces, edges, and vertices, a move reports the same
//! history, element for element, and so do faces made from a profile's regions, whose area
//! and box the fake works out by formula. (A fake boolean keeps both inputs whole, which the real
//! one doesn't claim; that's the fake's own model, tested in silico-core.)
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::planes;

use std::path::Path;
use std::time::Duration;

use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    BuiltData, Curve, FaceRegions, Kernel, KernelRequest, Primitive, Region, ShapeData,
    ShapeSummary, SketchPlane,
};
use silico_core::pool::{KernelPool, Pending};

fn pool() -> KernelPool {
    KernelPool::start_on_demand(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2).unwrap()
}

fn done<T>(pending: Pending<T>) -> T {
    let outcome = pending
        .wait_timeout(Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("no outcome within a minute"));
    outcome.done().expect("superseded").unwrap()
}

const PRIMITIVES: [Primitive; 7] = [
    Primitive::Box {
        dx: 10.0,
        dy: 20.0,
        dz: 30.0,
    },
    Primitive::Cylinder {
        radius: 2.5,
        height: 12.0,
    },
    Primitive::Sphere { radius: 4.0 },
    Primitive::Cone {
        radius1: 3.0,
        radius2: 1.0,
        height: 5.0,
    },
    Primitive::Cone {
        radius1: 3.0,
        radius2: 0.0,
        height: 5.0,
    },
    Primitive::Cone {
        radius1: 0.0,
        radius2: 3.0,
        height: 5.0,
    },
    Primitive::Torus {
        major_radius: 10.0,
        minor_radius: 2.0,
    },
];

/// `primitive`, and it moved by `offset`, on any kernel.
fn made<K: Kernel>(kernel: &K, primitive: Primitive, offset: [f64; 3]) -> (ShapeData, BuiltData) {
    let generation = kernel.generation();
    let shape = done(kernel.submit(generation, KernelRequest::make(primitive).unwrap()));
    let moved = done(kernel.submit(
        generation,
        KernelRequest::translate(&shape, offset).unwrap(),
    ));
    (shape, moved)
}

#[test]
fn each_primitive_has_the_real_kernels_element_counts() {
    let real = pool();
    let fake = FakeKernel::new(2).unwrap();
    for primitive in PRIMITIVES {
        let (shape, _) = made(&real, primitive, [0.0; 3]);
        let summary: ShapeSummary =
            done(real.submit(real.generation(), KernelRequest::summarize(&shape)));
        let (faked, _) = made(&fake, primitive, [0.0; 3]);
        let counts = FakeKernel::counts(&faked).unwrap();
        assert_eq!(
            (counts.faces, counts.edges, counts.vertices),
            (summary.faces, summary.edges, summary.vertices),
            "{primitive:?}"
        );
        assert_eq!(counts.solids, 1);
    }
}

#[test]
fn a_move_has_the_real_kernels_history() {
    let real = pool();
    let fake = FakeKernel::new(2).unwrap();
    for primitive in PRIMITIVES {
        for offset in [[1.0, -2.0, 3.5], [0.0, 0.0, 0.0]] {
            let (_, real_moved) = made(&real, primitive, offset);
            let (_, fake_moved) = made(&fake, primitive, offset);
            assert_eq!(
                fake_moved.history, real_moved.history,
                "{primitive:?} by {offset:?}"
            );
        }
    }
}

/// Regions with every kind of curve, an arc each way, holes, and an island: a 10 × 6
/// rectangle with a slot hole (walked clockwise) and a circle hole; a circle; and the slot
/// moved along, counter-clockwise.
fn regions(plane: SketchPlane) -> FaceRegions {
    use std::f64::consts::FRAC_PI_2;
    let line = |start, end| Curve::Line { start, end };
    let rectangle = vec![
        line([-3.0, -3.0], [7.0, -3.0]),
        line([7.0, -3.0], [7.0, 3.0]),
        line([7.0, 3.0], [-3.0, 3.0]),
        line([-3.0, 3.0], [-3.0, -3.0]),
    ];
    let arc = |center: [f64; 2], start, end, start_angle, end_angle| Curve::Arc {
        center,
        start,
        end,
        radius: 1.0,
        start_angle,
        end_angle,
    };
    let slot = |dx: f64| {
        vec![
            line([dx, -1.0], [dx + 4.0, -1.0]),
            arc(
                [dx + 4.0, 0.0],
                [dx + 4.0, -1.0],
                [dx + 4.0, 1.0],
                -FRAC_PI_2,
                FRAC_PI_2,
            ),
            line([dx + 4.0, 1.0], [dx, 1.0]),
            arc([dx, 0.0], [dx, 1.0], [dx, -1.0], FRAC_PI_2, 3.0 * FRAC_PI_2),
        ]
    };
    let slot_hole = vec![
        arc(
            [0.0, 0.0],
            [0.0, -1.0],
            [0.0, 1.0],
            3.0 * FRAC_PI_2,
            FRAC_PI_2,
        ),
        line([0.0, 1.0], [4.0, 1.0]),
        arc([4.0, 0.0], [4.0, 1.0], [4.0, -1.0], FRAC_PI_2, -FRAC_PI_2),
        line([4.0, -1.0], [0.0, -1.0]),
    ];
    FaceRegions {
        plane,
        regions: vec![
            Region {
                outer: rectangle,
                holes: vec![
                    slot_hole,
                    vec![Curve::Circle {
                        center: [6.0, 2.0],
                        radius: 0.5,
                    }],
                ],
            },
            Region {
                outer: vec![Curve::Circle {
                    center: [20.0, 0.0],
                    radius: 2.0,
                }],
                holes: vec![],
            },
            Region {
                outer: slot(30.0),
                holes: vec![],
            },
        ],
    }
}

#[test]
fn faces_have_the_real_kernels_history_area_and_box() {
    // On every plane, both face frames among them. Where the plane's axes are model space's
    // the fake is exact: the same volume (0), centre, and box to 1e-9. On the tilted frame
    // the kernel's faces enclose 0 only to rounding (about 1e-16), with a centre of that
    // rounding's, and its box of a tilted circle or arc is tight to about 1e-6, so there the
    // volume is 0 to 1e-12 of the area, the centre isn't compared, and the box is to 1e-6;
    // the history, counts, and area are the kernel's on every plane.
    let real = pool();
    let fake = FakeKernel::new(2).unwrap();
    for plane in planes::every() {
        let square = planes::square().contains(&plane);
        let request = || KernelRequest::make_faces(&regions(plane)).unwrap();
        let real_faces = done(real.submit(real.generation(), request()));
        let fake_faces = done(fake.submit(fake.generation(), request()));
        assert_eq!(fake_faces.history, real_faces.history, "{plane:?}");
        let summary = |kernel: &dyn Fn(KernelRequest<ShapeSummary>) -> ShapeSummary,
                       shape: &ShapeData| {
            kernel(KernelRequest::summarize(shape))
        };
        let real_summary = summary(
            &|r| done(real.submit(real.generation(), r)),
            &real_faces.shape,
        );
        let fake_summary = summary(
            &|r| done(fake.submit(fake.generation(), r)),
            &fake_faces.shape,
        );
        assert_eq!(
            (
                fake_summary.faces,
                fake_summary.edges,
                fake_summary.vertices
            ),
            (
                real_summary.faces,
                real_summary.edges,
                real_summary.vertices
            )
        );
        if square {
            assert_eq!(fake_summary.volume, real_summary.volume, "{plane:?}");
            assert_eq!(fake_summary.center, real_summary.center, "{plane:?}");
        } else {
            assert_eq!(fake_summary.volume, 0.0);
            assert!(
                real_summary.volume.abs() <= 1e-12 * real_summary.area,
                "{plane:?}: {}",
                real_summary.volume
            );
        }
        assert!(
            (fake_summary.area - real_summary.area).abs() < 1e-9 * real_summary.area,
            "{plane:?}: {} and {}",
            fake_summary.area,
            real_summary.area
        );
        let (fake_box, real_box) = (fake_summary.bounds.unwrap(), real_summary.bounds.unwrap());
        let tolerance = if square { 1e-9 } else { 1e-6 };
        for (f, r) in fake_box.iter().flatten().zip(real_box.iter().flatten()) {
            assert!(
                (f - r).abs() < tolerance,
                "{plane:?}: {fake_box:?}, {real_box:?}"
            );
        }
    }
}

#[test]
fn a_prism_and_a_turn_of_faces_have_the_real_kernels_counts_and_rule() {
    // The fake numbers a sweep its own way, but its counts are the kernel's for a prism and
    // for a turn with nothing on the axis, and both keep the design's rule; for a rectangle
    // swept square, its measures are the kernel's too.
    use silico_core::kernel::{Axis, SweepKind};
    let real = pool();
    let fake = FakeKernel::new(2).unwrap();
    let axis = Axis {
        origin: [-20.0, 0.0, 0.0],
        direction: [0.0, 1.0, 0.0],
    };
    // The prisms on every plane whose axes are model space's (the reversed top's among
    // them), as the faces above.
    for plane in planes::square() {
        let sweep = |faces: &ShapeData, turn: bool| {
            if turn {
                KernelRequest::revolution(faces, axis, -0.25, 1.0).unwrap()
            } else {
                KernelRequest::prism(faces, [0.0, 0.0, 0.0], plane.normal().map(|c| 7.0 * c))
                    .unwrap()
            }
        };
        let mut sent = vec![regions(plane)];
        sent.push(FaceRegions {
            plane,
            regions: vec![Region {
                outer: vec![
                    Curve::Line {
                        start: [0.0, 0.0],
                        end: [4.0, 0.0],
                    },
                    Curve::Line {
                        start: [4.0, 0.0],
                        end: [4.0, 3.0],
                    },
                    Curve::Line {
                        start: [4.0, 3.0],
                        end: [0.0, 3.0],
                    },
                    Curve::Line {
                        start: [0.0, 3.0],
                        end: [0.0, 0.0],
                    },
                ],
                holes: vec![],
            }],
        });
        for (n, sent) in sent.iter().enumerate() {
            for turn in [false, true] {
                // The axis, the line x = −20 along +y, lies in the XY plane only, clear of
                // every region, as the fake assumes of a turn's axis.
                if turn && plane != SketchPlane::Xy {
                    continue;
                }
                let kind = if turn {
                    SweepKind::Turn
                } else {
                    SweepKind::Prism
                };
                let real_faces =
                    done(real.submit(real.generation(), KernelRequest::make_faces(sent).unwrap()));
                let fake_faces =
                    done(fake.submit(fake.generation(), KernelRequest::make_faces(sent).unwrap()));
                let real_swept =
                    done(real.submit(real.generation(), sweep(&real_faces.shape, turn)));
                let fake_swept =
                    done(fake.submit(fake.generation(), sweep(&fake_faces.shape, turn)));
                let what = format!("{plane:?} regions {n}, {kind:?}");
                assert_eq!(
                    fake_swept.history.result, real_swept.history.result,
                    "{what}"
                );
                assert_eq!(real_swept.history.sweep_fault(kind), None, "{what}");
                assert_eq!(fake_swept.history.sweep_fault(kind), None, "{what}");
                if n == 1 && !turn {
                    let real_summary: ShapeSummary = done(real.submit(
                        real.generation(),
                        KernelRequest::summarize(&real_swept.shape),
                    ));
                    let fake_summary: ShapeSummary = done(fake.submit(
                        fake.generation(),
                        KernelRequest::summarize(&fake_swept.shape),
                    ));
                    assert!(
                        (fake_summary.volume - real_summary.volume).abs()
                            < 1e-9 * real_summary.volume,
                        "{what}"
                    );
                    assert!(
                        (fake_summary.area - real_summary.area).abs() < 1e-9 * real_summary.area,
                        "{what}"
                    );
                    for (f, r) in fake_summary.center.iter().zip(&real_summary.center) {
                        assert!((f - r).abs() < 1e-9, "{what}");
                    }
                    let (f, r) = (fake_summary.bounds.unwrap(), real_summary.bounds.unwrap());
                    for (f, r) in f.iter().flatten().zip(r.iter().flatten()) {
                        assert!((f - r).abs() < 1e-9, "{what}");
                    }
                }
            }
        }
    }
}
