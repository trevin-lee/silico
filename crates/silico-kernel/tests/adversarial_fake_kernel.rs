//! break-it: the in-memory fake kernel (silico_core::fake) against the real one, through
//! the `Kernel` trait both implement. Primitives at the edges of what `Primitive::check`
//! lets through (radii a hair above 1e-7, cones whose radii are within OCCT's tolerance of
//! each other, horn tori, a signed zero), moves by nothing, by -0, by a subnormal, and by a
//! kilometre, and the generation rules the trait promises, each run on both kernels. The
//! real kernel is the oracle: where the fake claims to answer as it does (element counts,
//! move histories, which shapes are the same shape), it must.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::time::Duration;

use proptest::prelude::*;
use proptest::test_runner::{Config, TestCaseError, TestRunner};
use silico_core::Error;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    Blob, BuiltData, Generation, Kernel, KernelRequest, Op, Primitive, ShapeData, ShapeSummary,
};
use silico_core::pool::{KernelPool, Outcome, Pending};

fn pool() -> KernelPool {
    KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2).unwrap()
}

fn outcome<T>(pending: Pending<T>) -> Outcome<T> {
    pending
        .wait_timeout(Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("no outcome within a minute"))
}

fn result<T>(pending: Pending<T>) -> Result<T, Error> {
    outcome(pending).done().expect("superseded")
}

fn make<K: Kernel>(kernel: &K, primitive: Primitive) -> Result<ShapeData, Error> {
    result(kernel.submit(kernel.generation(), KernelRequest::make(primitive)?))
}

fn translate<K: Kernel>(
    kernel: &K,
    shape: &ShapeData,
    offset: [f64; 3],
) -> Result<BuiltData, Error> {
    result(kernel.submit(
        kernel.generation(),
        KernelRequest::translate(shape, offset)?,
    ))
}

const ABOVE_TOLERANCE: f64 = 1.0000000000000002e-7;

// --- Which shapes are the same shape ----------------------------------------------------

// Once (the fake's half): the real kernel makes a cone with a radius of -0.0 byte for
// byte the cone with 0.0 (lib.rs writes -0.0 as 0.0 for that reason). The fake gives them
// different bytes, so a core test on the fake sees two shapes where the real kernel has one.
#[test]
fn a_negative_zero_radius_makes_the_same_bytes_as_zero_on_both_kernels() {
    let real = pool();
    let fake = FakeKernel::new(1).unwrap();
    let cone = |radius1: f64| Primitive::Cone {
        radius1,
        radius2: 3.0,
        height: 5.0,
    };
    let real_zero = make(&real, cone(0.0)).unwrap();
    let real_negative = make(&real, cone(-0.0)).unwrap();
    assert_eq!(real_negative, real_zero, "the real kernel's bytes");
    let fake_zero = make(&fake, cone(0.0)).unwrap();
    let fake_negative = make(&fake, cone(-0.0)).unwrap();
    assert_eq!(fake_negative, fake_zero, "the fake's bytes");
}

// Once (the fake's half): a move by nothing is the shape itself to the real kernel
// (bridge.cpp, `translate`: `is_identity(move) ? shape.shape`): the same bytes as its input,
// for (0, 0, 0) and (-0, -0, -0) alike. The fake makes a new shape,
// `translate(box, 0, 0, 0)`, and another for -0: three shapes where the real kernel has one.
#[test]
fn a_move_by_nothing_is_its_input_on_both_kernels() {
    let real = pool();
    let fake = FakeKernel::new(1).unwrap();
    let block = Primitive::Box {
        dx: 1.0,
        dy: 2.0,
        dz: 3.0,
    };
    for (name, kernel) in [("real", &real as &dyn Run), ("fake", &fake as &dyn Run)] {
        let shape = kernel.make(block);
        for offset in [[0.0, 0.0, 0.0], [-0.0, -0.0, -0.0], [0.0, -0.0, 0.0]] {
            let moved = kernel.translate(&shape, offset);
            assert_eq!(
                moved.shape.brep(),
                shape.brep(),
                "{name}: a move by {offset:?} changed the bytes"
            );
        }
    }
}

/// The two kernels behind one object-safe face, for tests that run the same steps on each.
trait Run {
    fn make(&self, primitive: Primitive) -> ShapeData;
    fn translate(&self, shape: &ShapeData, offset: [f64; 3]) -> BuiltData;
}

impl<K: Kernel> Run for K {
    fn make(&self, primitive: Primitive) -> ShapeData {
        make(self, primitive).unwrap()
    }
    fn translate(&self, shape: &ShapeData, offset: [f64; 3]) -> BuiltData {
        translate(self, shape, offset).unwrap()
    }
}

// A known difference, pinned both ways: OCCT writes a freshly made cylinder or torus
// differently once it has read it back, so its bytes change once, on the first operation it
// passes through, even a move by nothing or there and back; a box, a sphere, and a cone's
// don't. The fake's never change. Probed for each primitive; a change on either side shows.
#[test]
fn which_primitives_the_real_kernel_rewrites_on_their_first_pass_is_a_known_difference() {
    let real = pool();
    let fake = FakeKernel::new(1).unwrap();
    let primitives = [
        (
            "box",
            Primitive::Box {
                dx: 1.0,
                dy: 2.0,
                dz: 3.0,
            },
            false,
        ),
        (
            "cylinder",
            Primitive::Cylinder {
                radius: 1.0,
                height: 2.0,
            },
            true,
        ),
        ("sphere", Primitive::Sphere { radius: 1.0 }, false),
        (
            "cone",
            Primitive::Cone {
                radius1: 3.0,
                radius2: 1.0,
                height: 5.0,
            },
            false,
        ),
        (
            "pointed cone",
            Primitive::Cone {
                radius1: 3.0,
                radius2: 0.0,
                height: 5.0,
            },
            false,
        ),
        (
            "torus",
            Primitive::Torus {
                major_radius: 5.0,
                minor_radius: 1.0,
            },
            true,
        ),
    ];
    for (name, primitive, rewritten) in primitives {
        for (kernel_name, kernel, changes) in [
            ("real", &real as &dyn Run, rewritten),
            ("fake", &fake as &dyn Run, false),
        ] {
            let shape = kernel.make(primitive);
            let still = kernel.translate(&shape, [0.0; 3]).shape;
            let there = kernel.translate(&shape, [1.0, 0.0, 0.0]).shape;
            let back = kernel.translate(&there, [-1.0, 0.0, 0.0]).shape;
            assert_eq!(
                still.brep() != shape.brep(),
                changes,
                "{kernel_name} {name}: by nothing"
            );
            assert_eq!(
                back.brep() != shape.brep(),
                changes,
                "{kernel_name} {name}: there and back"
            );
            // Once through, settled: a further move by nothing changes nothing.
            assert_eq!(
                kernel.translate(&still, [0.0; 3]).shape.brep(),
                still.brep()
            );
        }
    }
}

#[test]
fn a_move_and_its_inverse_is_the_shape_itself_on_both_kernels() {
    let real = pool();
    let fake = FakeKernel::new(1).unwrap();
    let block = Primitive::Box {
        dx: 1.0,
        dy: 2.0,
        dz: 3.0,
    };
    for (name, kernel) in [("real", &real as &dyn Run), ("fake", &fake as &dyn Run)] {
        let shape = kernel.make(block);
        let there = kernel.translate(&shape, [1.0, 0.0, 0.0]);
        let back = kernel.translate(&there.shape, [-1.0, 0.0, 0.0]);
        assert_eq!(back.shape.brep(), shape.brep(), "{name}");
    }
}

#[test]
fn a_move_with_a_signed_zero_is_its_own_shape_on_both_kernels() {
    // Only a move by nothing at all ignores the sign of its zeros: the kernel keeps a
    // location's -0, so (-0, 1, 0) and (0, 1, 0) are different shapes on both.
    let real = pool();
    let fake = FakeKernel::new(1).unwrap();
    let block = Primitive::Box {
        dx: 1.0,
        dy: 2.0,
        dz: 3.0,
    };
    for (signed, plain) in [
        ([-0.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
        ([1.0, -0.0, -0.0], [1.0, 0.0, 0.0]),
    ] {
        for (name, kernel) in [("real", &real as &dyn Run), ("fake", &fake as &dyn Run)] {
            let shape = kernel.make(block);
            let a = kernel.translate(&shape, signed).shape;
            let b = kernel.translate(&shape, plain).shape;
            assert_ne!(a.brep(), b.brep(), "{name}: {signed:?} against {plain:?}");
        }
    }
}

#[test]
fn shapes_made_are_checked_valid_on_both_kernels() {
    let real = pool();
    let fake = FakeKernel::new(1).unwrap();
    let block = Primitive::Box {
        dx: 1.0,
        dy: 2.0,
        dz: 3.0,
    };
    let real_shape = real.make(block);
    let fake_shape = fake.make(block);
    assert_eq!(fake_shape.valid(), real_shape.valid());
    let real_moved = real.translate(&real_shape, [1.0, 0.0, 0.0]);
    let fake_moved = fake.translate(&fake_shape, [1.0, 0.0, 0.0]);
    assert_eq!(fake_moved.shape.valid(), real_moved.shape.valid());
}

// --- Counts and histories at the edges --------------------------------------------------

/// A length `Primitive::check` accepts, biased to its ends and to OCCT's tolerance.
fn length() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(ABOVE_TOLERANCE),
        Just(1.5e-7),
        Just(2e-7),
        Just(1e-6),
        Just(1.0),
        Just(1e6),
        Just(f64::from_bits(1e6f64.to_bits() - 1)),
        1e-3..1e3f64,
    ]
}

fn primitive() -> impl Strategy<Value = Primitive> {
    let cone_radius = prop_oneof![Just(0.0), Just(-0.0), length()];
    prop_oneof![
        (length(), length(), length()).prop_map(|(dx, dy, dz)| Primitive::Box { dx, dy, dz }),
        (length(), length()).prop_map(|(radius, height)| Primitive::Cylinder { radius, height }),
        length().prop_map(|radius| Primitive::Sphere { radius }),
        (cone_radius.clone(), cone_radius, length()).prop_map(|(radius1, radius2, height)| {
            Primitive::Cone {
                radius1,
                radius2,
                height,
            }
        }),
        // Radii within, at, and just past OCCT's tolerance of each other.
        (
            length(),
            prop_oneof![
                Just(0.0),
                Just(5e-8),
                Just(9.9e-8),
                Just(1e-7),
                Just(1.01e-7),
                Just(-5e-8)
            ],
            length()
        )
            .prop_map(|(radius1, gap, height)| Primitive::Cone {
                radius1,
                radius2: radius1 + gap,
                height,
            }),
        (length(), length()).prop_map(|(major_radius, minor_radius)| Primitive::Torus {
            major_radius,
            minor_radius,
        }),
        // Horn tori, and tubes a hair wider than the ring (allowed up to 1e-7).
        (
            length(),
            prop_oneof![Just(0.0), Just(5e-8), Just(1e-7), Just(-1e-7)]
        )
            .prop_map(|(major_radius, gap)| Primitive::Torus {
                major_radius,
                minor_radius: major_radius + gap,
            }),
    ]
    .prop_filter("values Primitive::check accepts", |p| p.check().is_ok())
}

fn offset_part() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(f64::from_bits(1)),
        Just(-f64::from_bits(1)),
        Just(f64::MIN_POSITIVE),
        Just(1e-300),
        Just(1e-7),
        Just(1e6),
        Just(-1e6),
        -1e3..1e3f64,
    ]
}

/// Every primitive both kernels make has the same counts on each; every one the real kernel
/// refuses, the fake refuses too, but for the documented difference ([`tube_wider`]); a
/// move of it, and a move of that, reports the same history on each; and which of those are
/// the same shape (moved against unmoved, moved twice against moved once by the sum) is the
/// same on each.
#[test]
fn the_fake_answers_as_the_real_kernel_does_at_the_edges() {
    // The strategy is deep enough to overflow a test thread's stack in a debug build.
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(compare_at_the_edges)
        .unwrap()
        .join()
        .unwrap();
}

fn compare_at_the_edges() {
    let real = pool();
    let fake = FakeKernel::new(2).unwrap();
    let mut runner = TestRunner::new(Config {
        cases: 2000,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20_261_015),
        ..Config::default()
    });
    runner
        .run(
            &(
                primitive(),
                proptest::array::uniform3(offset_part()),
                proptest::array::uniform3(offset_part()),
            ),
            |(primitive, first, second)| {
                let real_shape = make(&real, primitive);
                let fake_shape = make(&fake, primitive);
                let (real_shape, fake_shape) = match (real_shape, fake_shape) {
                    (Ok(r), Ok(f)) => (r, f),
                    (Err(_), Err(_)) => return Ok(()),
                    // The fake's documented difference: see
                    // `a_torus_with_a_tube_wider_than_its_ring_is_a_known_difference`.
                    (Err(_), Ok(_)) if tube_wider(&primitive) => return Ok(()),
                    (r, f) => {
                        return Err(TestCaseError::fail(format!(
                            "{primitive:?}: real {:?}, fake {:?}",
                            r.map(|_| "made"),
                            f.map(|_| "made")
                        )));
                    }
                };
                // The real kernel writes some primitives (a cylinder, a torus) differently when
                // it makes them than once it has read them back, so a shape's bytes settle
                // after its first pass through an operation (a move by nothing); compare which
                // shapes are the same from there. That difference is pinned by
                // `which_primitives_the_real_kernel_rewrites_on_their_first_pass_is_a_known_difference`.
                let real_shape = translate(&real, &real_shape, [0.0; 3]).unwrap().shape;
                let summary: ShapeSummary =
                    result(real.submit(real.generation(), KernelRequest::summarize(&real_shape)))
                        .unwrap();
                let counts = FakeKernel::counts(&fake_shape).unwrap();
                prop_assert_eq!(
                    (counts.faces, counts.edges, counts.vertices),
                    (summary.faces, summary.edges, summary.vertices),
                    "{:?}",
                    primitive
                );
                let real_moved = translate(&real, &real_shape, first).unwrap();
                let fake_moved = translate(&fake, &fake_shape, first).unwrap();
                prop_assert_eq!(
                    &fake_moved.history,
                    &real_moved.history,
                    "{:?} by {:?}",
                    primitive,
                    first
                );
                let real_again = translate(&real, &real_moved.shape, second).unwrap();
                let fake_again = translate(&fake, &fake_moved.shape, second).unwrap();
                prop_assert_eq!(
                    &fake_again.history,
                    &real_again.history,
                    "{:?} by {:?} then {:?}",
                    primitive,
                    first,
                    second
                );
                // Which shapes are the same shape, on each: moved against unmoved, and
                // moved twice against moved once by the sum, where that's a move the
                // protocol allows.
                let same = |what: &str, real_pair: [&ShapeData; 2], fake_pair: [&ShapeData; 2]| {
                    prop_assert_eq!(
                        fake_pair[0] == fake_pair[1],
                        real_pair[0].brep() == real_pair[1].brep(),
                        "{}: {:?} by {:?} then {:?}",
                        what,
                        primitive,
                        first,
                        second
                    );
                    Ok(())
                };
                same(
                    "moved",
                    [&real_moved.shape, &real_shape],
                    [&fake_moved.shape, &fake_shape],
                )?;
                same(
                    "again",
                    [&real_again.shape, &real_shape],
                    [&fake_again.shape, &fake_shape],
                )?;
                let sum = [0, 1, 2].map(|i| first[i] + second[i]);
                if silico_protocol::check_offset(sum).is_ok() {
                    let real_once = translate(&real, &real_shape, sum).unwrap().shape;
                    let fake_once = translate(&fake, &fake_shape, sum).unwrap().shape;
                    same(
                        "again, once",
                        [&real_again.shape, &real_once],
                        [&fake_again.shape, &fake_once],
                    )?;
                }
                Ok(())
            },
        )
        .unwrap();
}

// --- Generations, as the trait promises them --------------------------------------------

/// The same generation script on any kernel: what each request came to.
fn script<K: Kernel>(kernel: &K) -> Vec<&'static str> {
    let echo = |byte: u8| KernelRequest::echo(Blob::inline(vec![byte]));
    let name = |outcome: Outcome<Blob>| match outcome {
        Outcome::Done(Ok(_)) => "done",
        Outcome::Done(Err(_)) => "failed",
        Outcome::Superseded => "superseded",
    };
    let mut seen = Vec::new();
    kernel.advance(Generation(4));
    // Older: superseded at once.
    seen.push(name(outcome(kernel.submit(Generation(3), echo(1)))));
    // Current, and newer than current: run.
    seen.push(name(outcome(kernel.submit(Generation(4), echo(2)))));
    seen.push(name(outcome(kernel.submit(Generation(9), echo(3)))));
    // Going back, or standing still, changes nothing.
    kernel.advance(Generation(2));
    kernel.advance(Generation(4));
    seen.push(name(outcome(kernel.submit(Generation(4), echo(4)))));
    kernel.advance(Generation(u64::MAX));
    seen.push(name(outcome(
        kernel.submit(Generation(u64::MAX - 1), echo(5)),
    )));
    seen.push(name(outcome(kernel.submit(Generation(u64::MAX), echo(6)))));
    seen
}

#[test]
fn both_kernels_keep_the_same_generation_rules() {
    let real = pool();
    let fake = FakeKernel::new(2).unwrap();
    let expected = ["superseded", "done", "done", "done", "superseded", "done"];
    assert_eq!(script(&real), expected, "the pool");
    assert_eq!(script(&fake), expected, "the fake");
    assert_eq!(real.generation(), fake.generation());
}

/// A torus whose tube is wider than its ring, by no more than OCCT's tolerance, which the
/// protocol's check allows: OCCT may refuse it, by the floating point of its revolve, with
/// no rule the fake could copy, and the fake builds it (fake.rs, the module's doc).
fn tube_wider(primitive: &Primitive) -> bool {
    matches!(primitive, Primitive::Torus { major_radius, minor_radius } if minor_radius > major_radius)
}

// Once a break, now a documented difference: the real kernel refuses some tori whose tube is
// wider than their ring (KernelFailed, "MakeTorus failed", StdFail_NotDone), tiny and full
// size alike, and builds others; the fake builds them all. A test of that refusal gives it
// with `FakeKernel::refuse`. Pinned both ways, so a change on either side is seen.
#[test]
fn a_torus_with_a_tube_wider_than_its_ring_is_a_known_difference() {
    let real = pool();
    let fake = FakeKernel::new(1).unwrap();
    let refused = [
        (ABOVE_TOLERANCE, 2.0000000000000002e-7),
        (15.264435234694666, 15.264435334694666),
    ];
    for (major_radius, minor_radius) in refused {
        let torus = Primitive::Torus {
            major_radius,
            minor_radius,
        };
        assert!(torus.check().is_ok() && tube_wider(&torus));
        let real_error = make(&real, torus).unwrap_err();
        assert_eq!(
            real_error.code(),
            silico_core::ErrorCode::KernelFailed,
            "{torus:?}"
        );
        assert!(make(&fake, torus).is_ok(), "{torus:?}");
    }
    // And one the real kernel builds.
    let built = Primitive::Torus {
        major_radius: 50.0,
        minor_radius: 50.0 + 5e-8,
    };
    assert!(tube_wider(&built));
    assert!(make(&real, built).is_ok());
    assert!(make(&fake, built).is_ok());
    // Refused on request, as the real kernel refuses it.
    let torus = Primitive::Torus {
        major_radius: refused[1].0,
        minor_radius: refused[1].1,
    };
    fake.refuse(move |op| {
        (*op == Op::MakePrimitive(torus)).then(|| {
            silico_core::Error::new(silico_core::ErrorCode::KernelFailed, "MakeTorus failed")
        })
    });
    assert_eq!(
        make(&fake, torus).unwrap_err().code(),
        silico_core::ErrorCode::KernelFailed
    );
}

// Once (found by the property test above): the fake counted a cone with a top radius of
// zero as an apex (2 faces) and any other as a frustum (3 faces). OCCT decides by the
// geometry, not by zero: a cone whose small radius is just over 1e-7 (allowed by
// `Primitive::check`) can come out of OCCT without the small cap, as an apex, with 2 faces,
// 3 edges, and 2 vertices, and the fake says 3 faces. Every mismatch is listed.
#[test]
fn a_cone_with_a_small_radius_just_above_tolerance_has_the_real_kernels_counts() {
    let real = pool();
    let fake = FakeKernel::new(1).unwrap();
    let mut mismatches = Vec::new();
    for small in [ABOVE_TOLERANCE, 1.5e-7, 2e-7, 1e-6, 1e-5] {
        for height in [ABOVE_TOLERANCE, 1e-6, 1e-3, 1.0, 1e3] {
            for large in [1e-6, 1.0, 1e3] {
                for cone in [
                    Primitive::Cone {
                        radius1: large,
                        radius2: small,
                        height,
                    },
                    Primitive::Cone {
                        radius1: small,
                        radius2: large,
                        height,
                    },
                ] {
                    if cone.check().is_err() {
                        continue;
                    }
                    let Ok(real_shape) = make(&real, cone) else {
                        continue;
                    };
                    let summary: ShapeSummary = result(
                        real.submit(real.generation(), KernelRequest::summarize(&real_shape)),
                    )
                    .unwrap();
                    let counts = FakeKernel::counts(&fake.make(cone)).unwrap();
                    let real_counts = (summary.faces, summary.edges, summary.vertices);
                    let fake_counts = (counts.faces, counts.edges, counts.vertices);
                    if real_counts != fake_counts {
                        mismatches.push(format!(
                            "{cone:?}: real {real_counts:?}, fake {fake_counts:?}"
                        ));
                    }
                }
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
