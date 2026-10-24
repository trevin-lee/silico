//! break-it: the Kernel trait and the in-memory fake kernel (M2.3). Hostile use of the fake
//! as a test author or a recompute engine would use it: rules that panic, waits of "forever",
//! absurd sizes, forged shape bytes, signed zeros, races between `advance` and a finishing
//! request, and concurrent storms of submits, advances, and holds. Expected values come from
//! the pool's documented guarantees (pool.rs) and the real kernel's (silico-kernel lib.rs).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::echo::{echo, echoed};

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    Blob, BooleanOp, ChamferSize, ClientId, Clients, Generation, Kernel, KernelRequest, Op,
    Primitive, ShapeData,
};
use silico_core::pool::{Outcome, Pending};

const WAIT: Duration = Duration::from_secs(10);

fn block(dx: f64, dy: f64, dz: f64) -> Primitive {
    Primitive::Box { dx, dy, dz }
}

fn within<T: std::fmt::Debug>(pending: Pending<T>, what: &str) -> Outcome<T> {
    match pending.wait_timeout(Duration::from_secs(5)) {
        Ok(outcome) => outcome,
        Err(_) => panic!("{what}: no outcome within 5 s (the fake hung)"),
    }
}

fn done<T: std::fmt::Debug>(outcome: Outcome<T>) -> T {
    match outcome {
        Outcome::Done(result) => result.unwrap(),
        Outcome::Superseded => panic!("superseded"),
    }
}

fn run<T: Send + std::fmt::Debug + 'static>(kernel: &FakeKernel, request: KernelRequest<T>) -> T {
    done(within(kernel.submit(kernel.generation(), request), "run"))
}

// --- Rules that panic -------------------------------------------------------------------

// Once: a refusal rule that panics kills the fake's only thread (with the kernel's lock
// held). The request's caller is told "the kernel pool stopped before Echo was done", which
// is not what happened, and every later request waits forever: the kernel has no thread
// left to run it, and nothing reports why.
#[test]
fn a_panicking_refusal_rule_leaves_the_kernel_able_to_answer() {
    let kernel = FakeKernel::new(1).unwrap();
    kernel.refuse(|op| match op {
        Op::Echo { data } if data.as_bytes() == [0xff] => panic!("a test's rule has a bug"),
        _ => None,
    });
    let first = within(
        kernel.submit(Generation(0), KernelRequest::echo(Blob::inline(vec![0xff]))),
        "the request whose rule panicked",
    );
    let Outcome::Done(Err(error)) = first else {
        panic!("{first:?}")
    };
    assert_eq!(error.code(), ErrorCode::Internal);
    // The next, ordinary, request must still be answered.
    let next = within(
        kernel.submit(Generation(0), echo(1)),
        "an ordinary request after a rule panicked",
    );
    assert_eq!(done(next), Blob::inline(vec![1]));
}

// Once: as above, for a hold rule: the thread dies, `running` stays counted, and the
// next request hangs.
#[test]
fn a_panicking_hold_rule_leaves_the_kernel_able_to_answer() {
    let kernel = FakeKernel::new(1).unwrap();
    let _hold = kernel.hold(|op| match op {
        Op::Echo { data } if data.as_bytes() == [0xff] => panic!("a test's rule has a bug"),
        _ => false,
    });
    let _ = within(
        kernel.submit(Generation(0), KernelRequest::echo(Blob::inline(vec![0xff]))),
        "the request whose rule panicked",
    );
    let next = within(
        kernel.submit(Generation(0), echo(1)),
        "an ordinary request after a rule panicked",
    );
    assert_eq!(done(next), Blob::inline(vec![1]));
}

// --- Waits and sizes --------------------------------------------------------------------

// Once: `wait_running` adds the timeout to `Instant::now()` unchecked, so a wait of
// "forever" (Duration::MAX) panics with "overflow when adding duration to instant", even
// when the count is already reached.
#[test]
fn waiting_for_running_requests_without_a_time_limit() {
    let kernel = FakeKernel::new(1).unwrap();
    let hold = kernel.hold(|_| true);
    let pending = kernel.submit(Generation(0), KernelRequest::echo(Blob::inline(vec![])));
    assert!(kernel.wait_running(1, WAIT));
    let waited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        kernel.wait_running(1, Duration::MAX)
    }));
    assert!(
        matches!(waited, Ok(true)),
        "wait_running(1, Duration::MAX) with one running panicked or said no"
    );
    hold.release();
    done(within(pending, "the held request"));
}

#[test]
fn waiting_for_no_running_requests_is_at_once() {
    let kernel = FakeKernel::new(1).unwrap();
    assert!(kernel.wait_running(0, Duration::ZERO));
    assert!(!kernel.wait_running(1, Duration::ZERO));
}

// Once: `FakeKernel::new(usize::MAX)` collected `(0..usize::MAX).map(spawn)` into a Vec,
// which sizes itself up front: a capacity-overflow panic. The pool refuses a size past
// MAX_WORKERS (1024) with InvalidInput; the fake that stands in for it panics.
#[test]
fn a_fake_kernel_of_absurd_size_does_not_panic() {
    let made = std::panic::catch_unwind(|| {
        drop(FakeKernel::new(usize::MAX).unwrap());
    });
    assert!(made.is_ok(), "FakeKernel::new(usize::MAX) panicked");
}

#[test]
fn a_fake_kernel_of_size_zero_still_runs_requests() {
    let kernel = FakeKernel::new(0).unwrap();
    run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
}

// --- Shape identity ---------------------------------------------------------------------

// Once: a cone with a radius of -0.0 is the same cone as with 0.0 (Primitive::check
// accepts both; the real kernel writes -0.0 as 0.0 "so the B-rep's bytes don't depend on
// the sign of a zero", silico-kernel/src/lib.rs). The fake names it by `cone(-0, 3, 5)`,
// another hash, so the same cone has different bytes by the sign of a zero.
#[test]
fn a_cone_with_a_negative_zero_radius_is_the_same_shape_as_with_zero() {
    let kernel = FakeKernel::new(1).unwrap();
    let cone = |radius2: f64| Primitive::Cone {
        radius1: 3.0,
        radius2,
        height: 5.0,
    };
    let zero = run(&kernel, KernelRequest::make(cone(0.0)).unwrap());
    let negative = run(&kernel, KernelRequest::make(cone(-0.0)).unwrap());
    assert_eq!(FakeKernel::counts(&negative), FakeKernel::counts(&zero));
    assert_eq!(
        negative, zero,
        "the sign of a zero radius changed the shape's bytes"
    );
}

// Once: a move by (-0, 0, 0) is a move by nothing; the real kernel returns the shape
// itself for both, the same bytes. The fake's key writes "-0", so the two differ.
#[test]
fn a_move_by_negative_zero_is_the_same_shape_as_a_move_by_zero() {
    let kernel = FakeKernel::new(1).unwrap();
    let shape = run(&kernel, KernelRequest::make(block(1.0, 2.0, 3.0)).unwrap());
    let zero = run(
        &kernel,
        KernelRequest::translate(&shape, [0.0, 0.0, 0.0]).unwrap(),
    );
    let negative = run(
        &kernel,
        KernelRequest::translate(&shape, [-0.0, -0.0, -0.0]).unwrap(),
    );
    assert_eq!(negative.history, zero.history);
    assert_eq!(
        negative.shape, zero.shape,
        "the sign of a zero offset changed the shape's bytes"
    );
}

#[test]
fn shapes_differing_by_one_ulp_are_different_shapes() {
    let kernel = FakeKernel::new(1).unwrap();
    let a = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    let b = run(
        &kernel,
        KernelRequest::make(block(1.0, 1.0, f64::from_bits(1.0f64.to_bits() + 1))).unwrap(),
    );
    assert_ne!(a, b);
    let tiny = run(
        &kernel,
        KernelRequest::translate(&a, [f64::from_bits(1), 0.0, 0.0]).unwrap(),
    );
    let none = run(
        &kernel,
        KernelRequest::translate(&a, [0.0, 0.0, 0.0]).unwrap(),
    );
    assert_ne!(tiny.shape, none.shape);
    // A subnormal move is a move: every element modified, none kept.
    assert!(
        tiny.history.inputs[0]
            .faces
            .iter()
            .all(|f| f.kept.is_none())
    );
}

// Once: the fake read a shape's ID with `u128::from_str_radix`, which takes upper-case
// digits and a leading '+', so bytes it never made were taken as one of its shapes. Its
// shapes are now the steps that made them, and only their one canonical spelling is read:
// any other spelling of the same steps is refused, so a shape's bytes are one shape's.
#[test]
fn every_other_spelling_of_a_shapes_steps_is_refused() {
    let canonical = "silico fake shape 1\nbox 1 2 3\ntranslate 0 1 0 0\n";
    let shape = |text: &str| ShapeData::checked(Blob::inline(text.as_bytes().to_vec()), true);
    assert_eq!(
        FakeKernel::describe(&shape(canonical)).as_deref(),
        Some("translate(box(1, 2, 3), 1, 0, 0)")
    );
    let forged = [
        "silico fake shape 1\nbox 1.0 2 3\ntranslate 0 1 0 0\n",
        "silico fake shape 1\nbox +1 2 3\ntranslate 0 1 0 0\n",
        "silico fake shape 1\nbox 1 2 3\ntranslate 00 1 0 0\n",
        "silico fake shape 1\nbox 1 2 3\ntranslate 0 1e0 0 0\n",
        "silico fake shape 1\nbox  1 2 3\ntranslate 0 1 0 0\n",
        "silico fake shape 1\r\nbox 1 2 3\r\ntranslate 0 1 0 0\r\n",
        "silico fake shape 1\nbox 1 2 3\ntranslate 0 1 0 0",
        "silico fake shape 1\nbox 1 2 3\ntranslate 0 1 0 0\n\n",
        "silico fake shape 2\nbox 1 2 3\ntranslate 0 1 0 0\n",
        // A primitive's zero is unsigned.
        "silico fake shape 1\ncone -0 2 3\n",
        // A move by nothing is the shape itself; a move of a move is one move.
        "silico fake shape 1\nbox 1 2 3\ntranslate 0 0 0 0\n",
        "silico fake shape 1\nbox 1 2 3\ntranslate 0 1 0 0\ntranslate 1 1 0 0\n",
        // Inputs out of place, a step no other uses, a reference forward.
        "silico fake shape 1\nbox 1 2 3\nsphere 1\ntranslate 0 1 0 0\n",
        "silico fake shape 1\nbox 1 2 3\nsphere 1\n",
        "silico fake shape 1\nbox 1 2 3\nsphere 1\nfuse 1 0\n",
        "silico fake shape 1\nbox 1 2 3\nsphere 1\nfuse 1 1\n",
        "silico fake shape 1\nfuse 0 0\n",
        "silico fake shape 1\nsphere 1\nbox 1 2 3\nfuse 0 0\n",
        "silico fake shape 1\ntranslate 1 1 0 0\nbox 1 2 3\n",
        "silico fake shape 1\n",
        "",
        // Values the protocol's check refuses, which no request can carry.
        "silico fake shape 1\nbox -1 0 1\n",
        "silico fake shape 1\ncone 0 0 0\n",
        "silico fake shape 1\ncone 1 2 0\n",
        "silico fake shape 1\nsphere 1000000000000000000000000000000\n",
        // Not finite, which no request can carry.
        "silico fake shape 1\nsphere inf\n",
        "silico fake shape 1\nbox 1 2 3\ntranslate 0 NaN 0 0\n",
    ];
    for text in forged {
        assert_eq!(FakeKernel::describe(&shape(text)), None, "{text:?}");
    }
}

#[test]
fn malformed_shape_bytes_are_refused_not_panicked_on() {
    let kernel = FakeKernel::new(1).unwrap();
    let made = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    let real = made.brep().as_bytes().to_vec();
    let mut cases: Vec<Vec<u8>> = vec![
        vec![],
        b"fake:".to_vec(),
        b"fake:-1".to_vec(),
        b"fake:"
            .iter()
            .copied()
            .chain(std::iter::repeat_n(b'f', 33))
            .collect(),
        b"fake:"
            .iter()
            .copied()
            .chain(std::iter::repeat_n(b'z', 32))
            .collect(),
        vec![0xff; 37],
        real.iter().copied().chain(*b"\n").collect(),
        real[..real.len() - 1].to_vec(),
    ];
    // A multi-byte character in place of the last digit keeps the length in bytes wrong.
    let mut multi = real[..real.len() - 1].to_vec();
    multi.extend("é".as_bytes());
    cases.push(multi);
    for bytes in cases {
        let shape = ShapeData::checked(Blob::inline(bytes.clone()), true);
        assert_eq!(FakeKernel::describe(&shape), None, "{bytes:?}");
        assert_eq!(FakeKernel::counts(&shape), None, "{bytes:?}");
        let outcome = within(
            kernel.submit(
                Generation(0),
                KernelRequest::boolean(BooleanOp::Fuse, &made, &shape),
            ),
            "a boolean with a malformed tool",
        );
        let Outcome::Done(Err(error)) = outcome else {
            panic!("{bytes:?}: {outcome:?}")
        };
        assert_eq!(error.code(), ErrorCode::InvalidInput);
    }
    // And the kernel still answers.
    run(&kernel, KernelRequest::make(block(2.0, 1.0, 1.0)).unwrap());
}

#[test]
fn a_boolean_of_a_shape_with_itself_keeps_a_consistent_history() {
    // The fake's model: inputs side by side. Its history must still pass the core's checks
    // (read_built validates every reply).
    let kernel = FakeKernel::new(1).unwrap();
    let shape = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    for op in [BooleanOp::Fuse, BooleanOp::Cut, BooleanOp::Common] {
        let built = run(&kernel, KernelRequest::boolean(op, &shape, &shape));
        assert_eq!(built.history.result.faces, 12);
        assert_eq!(built.history.inputs[1].faces[0].kept, Some(7));
    }
}

// --- Generations ------------------------------------------------------------------------

#[test]
fn a_request_for_a_future_generation_survives_an_advance_short_of_it() {
    // As the pool: advance drops only requests of generations older than the new one.
    let kernel = FakeKernel::new(1).unwrap();
    let hold = kernel.hold(|op| matches!(op, Op::Echo { data } if data.as_bytes() == [0]));
    let blocker = kernel.submit(Generation(5), KernelRequest::echo(Blob::inline(vec![0])));
    assert!(kernel.wait_running(1, WAIT));
    let future = kernel.submit(Generation(5), echo(1));
    let stale = kernel.submit(Generation(2), echo(2));
    kernel.advance(Generation(3));
    assert_eq!(kernel.generation(), Generation(3));
    assert!(matches!(within(stale, "stale"), Outcome::Superseded));
    hold.release();
    assert_eq!(done(within(blocker, "blocker")), Blob::inline(vec![0]));
    assert_eq!(done(within(future, "future")), Blob::inline(vec![1]));
    // Going back changes nothing.
    kernel.advance(Generation(1));
    assert_eq!(kernel.generation(), Generation(3));
}

#[test]
fn the_last_generation_there_is() {
    let kernel = FakeKernel::new(1).unwrap();
    kernel.advance(Generation(u64::MAX));
    let late = kernel.submit(
        Generation(u64::MAX - 1),
        KernelRequest::echo(Blob::inline(vec![])),
    );
    assert!(matches!(within(late, "late"), Outcome::Superseded));
    let now = kernel.submit(
        Generation(u64::MAX),
        KernelRequest::echo(Blob::inline(vec![7])),
    );
    assert_eq!(done(within(now, "now")), Blob::inline(vec![7]));
}

#[test]
fn a_refused_request_superseded_while_held_is_superseded_not_refused() {
    let kernel = FakeKernel::new(1).unwrap();
    kernel.refuse(|_| Some(silico_core::Error::new(ErrorCode::KernelFailed, "no")));
    let _hold = kernel.hold(|_| true);
    let pending = kernel.submit(Generation(0), KernelRequest::echo(Blob::inline(vec![])));
    assert!(kernel.wait_running(1, WAIT));
    kernel.advance(Generation(1));
    assert!(matches!(within(pending, "held"), Outcome::Superseded));
}

// Once (a race, so not every run): the pool decides a result and sends it under the lock
// `advance` takes, so "once `advance` has returned no result of an older generation is
// delivered" (pool.rs, Submitted::run). The fake decides under its lock, lets the lock go,
// and only then sends: an `advance` that lands between the two returns with nothing in the
// channel, and a `Done` for the superseded generation arrives after it. A recompute that
// trusts the pool's rule (anything arriving after advance is current, or superseded) is
// wrong on the fake. Threads waiting in `wait_running` widen the window, as each
// `notify_all` between the decision and the send must wake them.
#[test]
fn no_result_of_an_older_generation_arrives_after_advance_returns() {
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let waiters: Vec<_> = (0..8)
        .map(|_| {
            let kernel = Arc::clone(&kernel);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    kernel.wait_running(usize::MAX, Duration::from_millis(50));
                }
            })
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut late = None;
    let mut generation = 0u64;
    let mut spin = 0u32;
    while Instant::now() < deadline && late.is_none() {
        let pending = kernel.submit(
            Generation(generation),
            KernelRequest::echo(Blob::inline(vec![])),
        );
        // Sweep the advance across the request's run.
        spin = (spin + 37) % 4000;
        for _ in 0..spin {
            std::hint::spin_loop();
        }
        generation += 1;
        kernel.advance(Generation(generation));
        match pending.try_take() {
            Ok(_) => {}
            Err(pending) => {
                if let Outcome::Done(_) = within(pending, "after advance") {
                    late = Some(generation - 1);
                }
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    for waiter in waiters {
        waiter.join().unwrap();
    }
    assert_eq!(
        late, None,
        "a result of a superseded generation arrived after advance returned"
    );
}

/// A small deterministic generator, so a failure's seed reproduces it.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

#[test]
fn a_storm_of_submits_advances_and_holds_never_hangs_and_never_delivers_stale_results() {
    for seed in 0..40u64 {
        let kernel = Arc::new(FakeKernel::new(1 + (seed as usize % 4)).unwrap());
        let holds: Arc<Mutex<Vec<silico_core::fake::Hold>>> = Arc::new(Mutex::new(Vec::new()));
        let threads: Vec<_> = (0..4u64)
            .map(|t| {
                let kernel = Arc::clone(&kernel);
                let holds = Arc::clone(&holds);
                std::thread::spawn(move || {
                    let mut rng = Lcg(seed * 31 + t);
                    let mut pendings = Vec::new();
                    for _ in 0..200 {
                        match rng.next() % 10 {
                            0 => {
                                let current = kernel.generation().0;
                                kernel.advance(Generation(current + 1 + rng.next() % 2));
                            }
                            1 => {
                                let byte = (rng.next() % 4) as u8;
                                let hold = kernel.hold(move |op| {
                                    matches!(op, Op::Echo { data } if data.as_bytes() == [byte])
                                });
                                holds.lock().unwrap().push(hold);
                            }
                            2 => {
                                let mut holds = holds.lock().unwrap();
                                if !holds.is_empty() {
                                    let at = rng.next() as usize % holds.len();
                                    drop(holds.swap_remove(at));
                                }
                            }
                            _ => {
                                let current = kernel.generation().0;
                                // Some stale, most current, some future.
                                let generation = match rng.next() % 6 {
                                    0 => current.saturating_sub(1),
                                    1 => current + 1,
                                    _ => current,
                                };
                                let byte = (rng.next() % 6) as u8;
                                let pending = kernel.submit(
                                    Generation(generation),
                                    KernelRequest::echo(Blob::inline(vec![byte])),
                                );
                                pendings.push((Generation(generation), current, byte, pending));
                            }
                        }
                    }
                    pendings
                })
            })
            .collect();
        let pendings: Vec<_> = threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect();
        // Let everything held go, so every request ends.
        holds.lock().unwrap().clear();
        for (generation, current, byte, pending) in pendings {
            let outcome = within(pending, &format!("seed {seed}"));
            match outcome {
                Outcome::Done(result) => {
                    assert!(
                        generation.0 >= current,
                        "seed {seed}: a request already stale when submitted ran"
                    );
                    assert_eq!(result.unwrap(), Blob::inline(vec![byte]), "seed {seed}");
                }
                Outcome::Superseded => {}
            }
        }
        // Done: no thread is left holding a request.
        assert!(!kernel.wait_running(1, Duration::ZERO));
        let kernel = Arc::try_unwrap(kernel).ok().unwrap();
        let started = Instant::now();
        drop(kernel);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "seed {seed}: drop hung"
        );
    }
}

#[test]
fn dropping_the_kernel_with_requests_held_from_many_threads_ends_them_all() {
    let kernel = FakeKernel::new(4).unwrap();
    let hold = kernel.hold(|_| true);
    let pendings: Vec<_> = (0..64u8)
        .map(|n| kernel.submit(Generation(0), KernelRequest::echo(Blob::inline(vec![n]))))
        .collect();
    assert!(kernel.wait_running(4, WAIT));
    let started = Instant::now();
    drop(kernel);
    assert!(started.elapsed() < Duration::from_secs(5));
    for pending in pendings {
        let Outcome::Done(Err(error)) = within(pending, "after drop") else {
            panic!("a request ended by the drop had an outcome other than the stop's error")
        };
        assert_eq!(error.code(), ErrorCode::Internal);
    }
    // A hold outliving its kernel releases without trouble.
    hold.release();
    drop(hold);
}

#[test]
fn a_caller_that_stops_waiting_does_not_wedge_the_kernel() {
    let kernel = FakeKernel::new(1).unwrap();
    for n in 0..1000u32 {
        // Dropped at once: the reply has nowhere to go.
        drop(kernel.submit(
            Generation(0),
            KernelRequest::echo(Blob::inline(n.to_le_bytes().to_vec())),
        ));
    }
    run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    assert_eq!(kernel.calls().len(), 1001);
}

#[test]
fn every_operation_the_fake_does_not_do_is_a_structured_internal_error() {
    let kernel = FakeKernel::new(1).unwrap();
    let shape = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    let outcomes = [
        within(
            kernel.submit(
                Generation(0),
                KernelRequest::chamfer(
                    &shape,
                    &[1],
                    ChamferSize::TwoDistances {
                        distance: 0.1,
                        other: 0.2,
                    },
                    false,
                )
                .unwrap(),
            ),
            "uneven chamfer",
        )
        .done()
        .unwrap()
        .map(|_| ()),
        within(
            kernel.submit(Generation(0), KernelRequest::tessellate(&shape, 0.1)),
            "tessellate",
        )
        .done()
        .unwrap()
        .map(|_| ()),
        within(
            kernel.submit(Generation(0), KernelRequest::measure_as_freecad(&shape)),
            "measure",
        )
        .done()
        .unwrap()
        .map(|_| ()),
    ];
    for outcome in outcomes {
        let error = outcome.unwrap_err();
        assert_eq!(error.code(), ErrorCode::Internal);
        assert!(error.suggestion().is_some());
    }
    run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
}

// --- Dressings ----------------------------------------------------------------------------

#[test]
fn a_dressing_of_edges_the_shape_hasnt_is_refused_and_its_last_edge_taken() {
    // Found by mutation testing: nothing held the fake's refusal of edges a shape hasn't.
    // A box has 12 edges: none, the 0th, and the 13th are refused, as the kernel refuses
    // them; the 12th is filleted.
    let kernel = FakeKernel::new(1).unwrap();
    let made = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    for edges in [&[][..], &[0], &[13], &[1, 13]] {
        let outcome = within(
            kernel.submit(
                kernel.generation(),
                KernelRequest::fillet(&made, edges, 0.1).unwrap(),
            ),
            "a fillet of edges a box hasn't",
        );
        let Outcome::Done(Err(error)) = outcome else {
            panic!("{edges:?}: {outcome:?}")
        };
        assert_eq!(error.code(), ErrorCode::KernelFailed, "{edges:?}");
    }
    let built = run(&kernel, KernelRequest::fillet(&made, &[12], 0.1).unwrap());
    assert_eq!(
        FakeKernel::describe(&built.shape).as_deref(),
        Some("fillet(box(1, 1, 1), 0.1, [12])")
    );
}

#[test]
fn a_boolean_of_a_dressed_tool_keeps_the_tools_steps_in_place() {
    // Found by mutation testing: a tool's steps are shifted after the base's when a boolean
    // joins them, and nothing held that a dressing's input is shifted too. The fuse reads
    // back, and says the fillet is of the tool's box.
    let kernel = FakeKernel::new(1).unwrap();
    let base = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    let other = run(&kernel, KernelRequest::make(block(2.0, 2.0, 2.0)).unwrap());
    let tool = run(&kernel, KernelRequest::fillet(&other, &[3], 0.5).unwrap()).shape;
    let fused = run(
        &kernel,
        KernelRequest::boolean(BooleanOp::Fuse, &base, &tool),
    )
    .shape;
    assert_eq!(
        FakeKernel::describe(&fused).as_deref(),
        Some("fuse(box(1, 1, 1), fillet(box(2, 2, 2), 0.5, [3]))")
    );
}

#[test]
fn a_dressing_in_shape_bytes_of_no_edge_a_repeated_one_or_one_past_the_shapes_is_refused() {
    // Found by mutation testing: shape bytes naming the 0th edge read as a shape. A fake's
    // dressing names edges of its input from 1, each once, at most its count.
    let kernel = FakeKernel::new(1).unwrap();
    let made = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    let built = run(&kernel, KernelRequest::fillet(&made, &[1], 0.5).unwrap()).shape;
    let real = std::str::from_utf8(built.brep().as_bytes())
        .unwrap()
        .to_owned();
    assert!(real.ends_with(" 1\n"), "{real:?}");
    assert!(FakeKernel::describe(&built).is_some());
    let stem = &real[..real.len() - 2];
    for edges in ["0", "1,1", "13", "1,0"] {
        let bytes = format!("{stem}{edges}\n");
        let shape = ShapeData::checked(Blob::inline(bytes.clone().into_bytes()), true);
        assert_eq!(FakeKernel::describe(&shape), None, "{bytes:?}");
        assert_eq!(FakeKernel::counts(&shape), None, "{bytes:?}");
    }
}

// --- Clients (M4.2, MA2) ------------------------------------------------------------------

/// What a test does to a fake kernel with clients: on the kernel's own client (0) or one of
/// three it gave (1 to 3), submit an echo or advance; or let everything held run, and
/// collect what came of it.
#[derive(Debug, Clone)]
enum Act {
    Submit { client: usize, generation: u64 },
    Advance { client: usize, generation: u64 },
    Settle,
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        4 => (0..4usize, 0..8u64).prop_map(|(client, generation)| Act::Submit { client, generation }),
        2 => (0..4usize, 0..8u64).prop_map(|(client, generation)| Act::Advance { client, generation }),
        1 => Just(Act::Settle),
    ]
}

/// A request submitted and not yet come to anything: whose, for which generation, and the
/// number its echo carries.
struct Unsettled {
    client: usize,
    generation: u64,
    number: u32,
    pending: Pending<Blob>,
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 96,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0xc1_1e_47),
        ..ProptestConfig::default()
    })]

    // Every request is held until a settle, so every advance before it comes before its
    // delivery: it comes to `Superseded` if and only if its own client's newest generation,
    // as the test keeps it, is past the request's. Another client's advances never matter.
    #[test]
    fn a_request_is_superseded_only_by_its_own_clients_advance(
        size in 1..4usize,
        acts in proptest::collection::vec(act(), 1..40),
    ) {
        let kernel = FakeKernel::new(size).unwrap();
        let clients = [kernel.client(), kernel.client(), kernel.client()];
        let ids = [ClientId::KERNEL, clients[0].id(), clients[1].id(), clients[2].id()];
        let submit = |client: usize, generation: u64, number: u32| {
            let request = KernelRequest::echo(Blob::inline(number.to_le_bytes().to_vec()));
            match client {
                0 => kernel.submit(Generation(generation), request),
                c => clients[c - 1].submit(Generation(generation), request),
            }
        };
        let advance = |client: usize, generation: u64| match client {
            0 => kernel.advance(Generation(generation)),
            c => clients[c - 1].advance(Generation(generation)),
        };
        // The test's own account of each client's newest generation.
        let mut newest = [0u64; 4];
        let mut hold = Some(kernel.hold(|_| true));
        let mut out: Vec<Unsettled> = Vec::new();
        // Each request that ran: (client, generation), by its number; and those that never
        // could start.
        let mut ran: BTreeMap<u32, (ClientId, Generation)> = BTreeMap::new();
        let mut never: Vec<u32> = Vec::new();
        let mut number = 0u32;
        let settle = |hold: &mut Option<silico_core::fake::Hold>,
                      out: &mut Vec<Unsettled>,
                      newest: &[u64; 4],
                      ran: &mut BTreeMap<u32, (ClientId, Generation)>|
         -> Result<(), TestCaseError> {
            drop(hold.take());
            for o in out.drain(..) {
                let superseded = o.generation < newest[o.client];
                match within(o.pending, "a request after its settle") {
                    Outcome::Superseded => prop_assert!(
                        superseded,
                        "request {} of client {} at {} superseded, its client at {}",
                        o.number, o.client, o.generation, newest[o.client]
                    ),
                    Outcome::Done(result) => {
                        prop_assert!(
                            !superseded,
                            "request {} of client {} at {} done, its client at {}",
                            o.number, o.client, o.generation, newest[o.client]
                        );
                        let echoed = result.unwrap();
                        prop_assert_eq!(echoed.as_bytes(), o.number.to_le_bytes());
                        ran.insert(o.number, (ids[o.client], Generation(o.generation)));
                    }
                }
            }
            *hold = Some(kernel.hold(|_| true));
            Ok(())
        };
        for act in acts {
            match act {
                Act::Submit { client, generation } => {
                    number += 1;
                    let pending = submit(client, generation, number);
                    if generation < newest[client] {
                        // Superseded at once, before `submit` returns.
                        prop_assert!(matches!(pending.try_take(), Ok(Outcome::Superseded)));
                        never.push(number);
                    } else {
                        out.push(Unsettled { client, generation, number, pending });
                    }
                }
                Act::Advance { client, generation } => {
                    newest[client] = newest[client].max(generation);
                    advance(client, generation);
                }
                Act::Settle => settle(&mut hold, &mut out, &newest, &mut ran)?,
            }
        }
        settle(&mut hold, &mut out, &newest, &mut ran)?;
        for (client, id) in ids.iter().enumerate() {
            let generation = match client {
                0 => kernel.generation(),
                c => clients[c - 1].generation(),
            };
            prop_assert_eq!(generation, Generation(newest[client]), "client {:?}", id);
        }
        // Each call is recorded as its request's client and generation; every request that
        // ran is there once, and none superseded at once is.
        let calls = kernel.calls();
        let mut seen = BTreeMap::new();
        for call in &calls {
            let Op::Echo { data } = &call.op else { panic!("{call:?}") };
            let n = u32::from_le_bytes(data.as_bytes().try_into().unwrap());
            prop_assert!(!never.contains(&n), "request {} started", n);
            *seen.entry(n).or_insert(0u32) += 1;
            if let Some(&(client, generation)) = ran.get(&n) {
                prop_assert_eq!((call.client, call.generation), (client, generation));
            }
        }
        for n in ran.keys() {
            prop_assert_eq!(seen.get(n), Some(&1), "request {}", n);
        }
    }
}

fn stop_error<T: std::fmt::Debug>(outcome: Outcome<T>) {
    let Outcome::Done(Err(error)) = outcome else {
        panic!("expected the stop's error, got {outcome:?}")
    };
    assert_eq!(error.code(), ErrorCode::Internal);
    assert_eq!(
        error.message(),
        "the fake kernel stopped before Echo was done"
    );
}

#[test]
fn a_client_that_outlives_its_kernel_gets_the_stops_error_never_a_hang() {
    let kernel = FakeKernel::new(1).unwrap();
    let client = kernel.client();
    let hold = kernel.hold(|_| true);
    let held = client.submit(Generation(0), echo(1));
    assert!(kernel.wait_running(1, WAIT));
    let waiting = client.submit(Generation(0), echo(2));
    drop(kernel);
    stop_error(within(
        held,
        "a held request of a client, its kernel dropped",
    ));
    stop_error(within(
        waiting,
        "a waiting request of a client, its kernel dropped",
    ));
    // Asked again, after the kernel is gone: the same, at once, and its generations still
    // its own.
    client.advance(Generation(3));
    assert_eq!(client.generation(), Generation(3));
    let late = client.submit(Generation(3), echo(3));
    stop_error(within(late, "a request to a client whose kernel is gone"));
    drop(hold);
    drop(client);
}

#[test]
fn a_retired_clients_turn_passes_to_the_next_client_waiting() {
    let kernel = FakeKernel::new(1).unwrap();
    let (a, b, c) = (kernel.client(), kernel.client(), kernel.client());
    let hold = kernel.hold(|_| true);
    let a1 = a.submit(Generation(0), echo(11));
    assert!(kernel.wait_running(1, WAIT));
    let a2 = a.submit(Generation(0), echo(12));
    let b1 = b.submit(Generation(0), echo(21));
    let c1 = c.submit(Generation(0), echo(31));
    // `a`'s turn was last; retired, its two end superseded, and the kernel goes on to `b`
    // and then `c`.
    drop(a);
    assert!(matches!(within(a1, "a1"), Outcome::Superseded));
    assert!(matches!(within(a2, "a2"), Outcome::Superseded));
    hold.release();
    assert_eq!(done(within(b1, "b1")).as_bytes(), [21]);
    assert_eq!(done(within(c1, "c1")).as_bytes(), [31]);
    let calls = kernel.calls();
    let clients: Vec<u64> = calls.iter().map(|call| call.client.number()).collect();
    assert_eq!(clients, [1, 2, 3]);
    assert_eq!(echoed(&calls), [11, 21, 31]);
}

#[test]
fn a_storm_of_clients_made_advanced_and_dropped_never_supersedes_another_clients_request() {
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let keeper = kernel.client();
    keeper.advance(Generation(1));
    let hold = kernel.hold(|op| matches!(op, Op::Echo { data } if data.as_bytes() == [0xaa]));
    let kept = keeper.submit(Generation(1), echo(0xaa));
    let own = kernel.submit(Generation(0), echo(0xbb));
    assert!(kernel.wait_running(1, WAIT));
    let storm: Vec<_> = (0..8u64)
        .map(|seed| {
            let kernel = Arc::clone(&kernel);
            std::thread::spawn(move || {
                let mut rng = Lcg(seed + 1);
                for _ in 0..50 {
                    let client = kernel.client();
                    let mut pending = Vec::new();
                    for _ in 0..(rng.next() % 4) {
                        let generation = Generation(rng.next() % 1000);
                        client.advance(generation);
                        pending.push(client.submit(Generation(rng.next() % 1000), echo(1)));
                    }
                    // Half dropped with their requests still waiting, half waited for.
                    if rng.next().is_multiple_of(2) {
                        for p in pending {
                            let _ = within(p, "a storm client's request");
                        }
                        drop(client);
                    } else {
                        drop(client);
                        for p in pending {
                            let outcome = within(p, "a dropped storm client's request");
                            assert!(!matches!(outcome, Outcome::Done(Err(_))), "{outcome:?}");
                        }
                    }
                }
            })
        })
        .collect();
    for thread in storm {
        thread.join().unwrap();
    }
    // 400 clients made, advanced to generations up to 999, and gone: the keeper's request
    // and the kernel's own are untouched.
    assert_eq!(keeper.generation(), Generation(1));
    assert_eq!(kernel.generation(), Generation(0));
    hold.release();
    assert_eq!(
        done(within(kept, "the keeper's request")).as_bytes(),
        [0xaa]
    );
    assert_eq!(
        done(within(own, "the kernel's own request")).as_bytes(),
        [0xbb]
    );
    // Numbered in the order given, never again: the keeper 1, the storm's 2 to 401.
    assert_eq!(kernel.client().id().number(), 402);
}

// Sweeps a client's drop across its unheld request's run: whatever point the drop lands at
// (the request waiting, running, or done), a request with no outcome once the drop has
// returned comes to `Superseded`, since an outcome is decided and sent under the lock the
// drop takes. A delivery that read a retired client as current would hand it a result.
#[test]
fn a_running_request_of_a_dropped_client_comes_to_superseded() {
    let kernel = FakeKernel::new(1).unwrap();
    let mut spin = 0u32;
    for round in 0..5000u32 {
        let client = kernel.client();
        let pending = client.submit(Generation(0), echo(1));
        spin = (spin + 37) % 4000;
        for _ in 0..spin {
            std::hint::spin_loop();
        }
        drop(client);
        match pending.try_take() {
            // Arrived before the drop: its result, or superseded by it.
            Ok(Outcome::Done(result)) => assert_eq!(result.unwrap().as_bytes(), [1]),
            Ok(Outcome::Superseded) => {}
            Err(pending) => {
                let outcome = within(pending, "a dropped client's request");
                assert!(
                    matches!(outcome, Outcome::Superseded),
                    "round {round}: a result after the client was dropped: {outcome:?}"
                );
            }
        }
    }
}

// The same sweep for a client's advance, with another client advancing all the while: a
// request of `a`'s with no outcome once `a` has advanced past it comes to `Superseded`,
// and `b`'s advances never decide it.
#[test]
fn a_clients_advance_across_its_running_request_supersedes_it_whatever_another_does() {
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let other = {
        let (kernel, stop) = (Arc::clone(&kernel), Arc::clone(&stop));
        std::thread::spawn(move || {
            let b = kernel.client();
            let mut generation = 0u64;
            while !stop.load(Ordering::Relaxed) {
                generation += 1;
                b.advance(Generation(generation));
            }
            generation
        })
    };
    let a = kernel.client();
    let mut spin = 0u32;
    let mut done_before = 0u32;
    for generation in 0..5000u64 {
        let pending = a.submit(Generation(generation), echo(1));
        spin = (spin + 37) % 4000;
        for _ in 0..spin {
            std::hint::spin_loop();
        }
        a.advance(Generation(generation + 1));
        match pending.try_take() {
            Ok(Outcome::Done(result)) => {
                assert_eq!(result.unwrap().as_bytes(), [1]);
                done_before += 1;
            }
            Ok(Outcome::Superseded) => {}
            Err(pending) => {
                let outcome = within(pending, "a request its client advanced past");
                assert!(
                    matches!(outcome, Outcome::Superseded),
                    "generation {generation}: a result after its client advanced: {outcome:?}"
                );
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    let b_reached = other.join().unwrap();
    assert!(b_reached > 0);
    assert_eq!(a.generation(), Generation(5000));
    // Global generations would supersede nearly every request of `a`'s at `b`'s next
    // advance; most here finish before `a`'s own.
    assert!(done_before > 0);
}
