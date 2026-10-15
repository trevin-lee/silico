//! break-it: the Kernel trait and the in-memory fake kernel (M2.3). Hostile use of the fake
//! as a test author or a recompute engine would use it: rules that panic, waits of "forever",
//! absurd sizes, forged shape bytes, signed zeros, races between `advance` and a finishing
//! request, and concurrent storms of submits, advances, and holds. Expected values come from
//! the pool's documented guarantees (pool.rs) and the real kernel's (silico-kernel lib.rs).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    Blob, BooleanOp, Generation, Kernel, KernelRequest, Op, Primitive, ShapeData,
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
    let kernel = FakeKernel::new(1);
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
        kernel.submit(Generation(0), KernelRequest::echo(Blob::inline(vec![1]))),
        "an ordinary request after a rule panicked",
    );
    assert_eq!(done(next), Blob::inline(vec![1]));
}

// Once: as above, for a hold rule: the thread dies, `running` stays counted, and the
// next request hangs.
#[test]
fn a_panicking_hold_rule_leaves_the_kernel_able_to_answer() {
    let kernel = FakeKernel::new(1);
    let _hold = kernel.hold(|op| match op {
        Op::Echo { data } if data.as_bytes() == [0xff] => panic!("a test's rule has a bug"),
        _ => false,
    });
    let _ = within(
        kernel.submit(Generation(0), KernelRequest::echo(Blob::inline(vec![0xff]))),
        "the request whose rule panicked",
    );
    let next = within(
        kernel.submit(Generation(0), KernelRequest::echo(Blob::inline(vec![1]))),
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
    let kernel = FakeKernel::new(1);
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
    let kernel = FakeKernel::new(1);
    assert!(kernel.wait_running(0, Duration::ZERO));
    assert!(!kernel.wait_running(1, Duration::ZERO));
}

// Once: `FakeKernel::new(usize::MAX)` collects `(0..usize::MAX).map(spawn)` into a Vec,
// which sizes itself up front: a capacity-overflow panic. The pool refuses a size past
// MAX_WORKERS (1024) with InvalidInput; the fake that stands in for it panics.
#[test]
fn a_fake_kernel_of_absurd_size_does_not_panic() {
    let made = std::panic::catch_unwind(|| {
        drop(FakeKernel::new(usize::MAX));
    });
    assert!(made.is_ok(), "FakeKernel::new(usize::MAX) panicked");
}

#[test]
fn a_fake_kernel_of_size_zero_still_runs_requests() {
    let kernel = FakeKernel::new(0);
    run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
}

// --- Shape identity ---------------------------------------------------------------------

// Once: a cone with a radius of -0.0 is the same cone as with 0.0 (Primitive::check
// accepts both; the real kernel writes -0.0 as 0.0 "so the B-rep's bytes don't depend on
// the sign of a zero", silico-kernel/src/lib.rs). The fake names it by `cone(-0, 3, 5)`,
// another hash, so the same cone has different bytes by the sign of a zero.
#[test]
fn a_cone_with_a_negative_zero_radius_is_the_same_shape_as_with_zero() {
    let kernel = FakeKernel::new(1);
    let cone = |radius2: f64| Primitive::Cone {
        radius1: 3.0,
        radius2,
        height: 5.0,
    };
    let zero = run(&kernel, KernelRequest::make(cone(0.0)).unwrap());
    let negative = run(&kernel, KernelRequest::make(cone(-0.0)).unwrap());
    assert_eq!(kernel.counts(&negative), kernel.counts(&zero));
    assert_eq!(
        negative, zero,
        "the sign of a zero radius changed the shape's bytes"
    );
}

// Once: a move by (-0, 0, 0) is a move by nothing; the real kernel returns the shape
// itself for both, the same bytes. The fake's key writes "-0", so the two differ.
#[test]
fn a_move_by_negative_zero_is_the_same_shape_as_a_move_by_zero() {
    let kernel = FakeKernel::new(1);
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
    let kernel = FakeKernel::new(1);
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

// Once: the fake reads a shape's ID with `u128::from_str_radix`, which takes upper-case
// digits and a leading '+'. Bytes the fake never made ("fake:+0…", "FAKE"-cased hex) are
// taken as one of its shapes, though it promises to refuse a shape it didn't make.
#[test]
fn forged_spellings_of_a_shapes_id_are_refused() {
    let kernel = FakeKernel::new(1);
    let made = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    let bytes = std::str::from_utf8(made.brep().as_bytes())
        .unwrap()
        .to_owned();
    let hex = bytes.strip_prefix("fake:").unwrap();
    let upper = format!("fake:{}", hex.to_uppercase());
    let mut forged = vec![upper];
    // Every ID that starts with a 0 digit has a '+' spelling of the same length.
    if let Some(rest) = hex.strip_prefix('0') {
        forged.push(format!("fake:+{rest}"));
    }
    forged.retain(|text| *text != bytes);
    assert!(!forged.is_empty());
    for text in forged {
        let shape = ShapeData::checked(Blob::inline(text.clone().into_bytes()), true);
        assert_eq!(
            kernel.describe(&shape),
            None,
            "{text} isn't a shape the fake made, but it describes it"
        );
    }
}

#[test]
fn malformed_shape_bytes_are_refused_not_panicked_on() {
    let kernel = FakeKernel::new(1);
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
        assert_eq!(kernel.describe(&shape), None, "{bytes:?}");
        assert_eq!(kernel.counts(&shape), None, "{bytes:?}");
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
    let kernel = FakeKernel::new(1);
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
    let kernel = FakeKernel::new(1);
    let hold = kernel.hold(|op| matches!(op, Op::Echo { data } if data.as_bytes() == [0]));
    let blocker = kernel.submit(Generation(5), KernelRequest::echo(Blob::inline(vec![0])));
    assert!(kernel.wait_running(1, WAIT));
    let future = kernel.submit(Generation(5), KernelRequest::echo(Blob::inline(vec![1])));
    let stale = kernel.submit(Generation(2), KernelRequest::echo(Blob::inline(vec![2])));
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
    let kernel = FakeKernel::new(1);
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
    let kernel = FakeKernel::new(1);
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
    let kernel = Arc::new(FakeKernel::new(1));
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
        let kernel = Arc::new(FakeKernel::new(1 + (seed as usize % 4)));
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
        assert!(kernel.wait_running(0, Duration::ZERO));
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
    let kernel = FakeKernel::new(4);
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
    let kernel = FakeKernel::new(1);
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
    let kernel = FakeKernel::new(1);
    let shape = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    let outcomes = [
        within(
            kernel.submit(
                Generation(0),
                KernelRequest::fillet(&shape, &[1], 0.1).unwrap(),
            ),
            "fillet",
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
