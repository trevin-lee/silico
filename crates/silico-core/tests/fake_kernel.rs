//! M2.3: the in-memory kernel. Core tests run on it without OCCT, through the same
//! [`Kernel`] trait the worker pool implements, and see every call it was given. Expected
//! values are written by hand: OCCT's element counts for each primitive (checked against
//! the real kernel in silico-kernel's tests/fake_kernel.rs), and shapes described as built.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    Blob, BooleanOp, BuiltData, ElementCounts, ElementKind, Generation, Kernel, KernelRequest, Op,
    Primitive, ShapeData,
};
use silico_core::pool::Outcome;

const WAIT: Duration = Duration::from_secs(10);

fn done<T: std::fmt::Debug>(outcome: Outcome<T>) -> T {
    match outcome {
        Outcome::Done(result) => result.unwrap(),
        Outcome::Superseded => panic!("superseded"),
    }
}

fn run<K: Kernel, T: Send + std::fmt::Debug + 'static>(kernel: &K, request: KernelRequest<T>) -> T {
    done(kernel.submit(kernel.generation(), request).wait())
}

fn block(dx: f64, dy: f64, dz: f64) -> Primitive {
    Primitive::Box { dx, dy, dz }
}

/// The design's example part: a block with a pin cut from it.
fn holed_block<K: Kernel>(kernel: &K) -> BuiltData {
    let block = run(
        kernel,
        KernelRequest::make(block(40.0, 20.0, 10.0)).unwrap(),
    );
    let pin = run(
        kernel,
        KernelRequest::make(Primitive::Cylinder {
            radius: 2.5,
            height: 12.0,
        })
        .unwrap(),
    );
    let pin = run(
        kernel,
        KernelRequest::translate(&pin, [20.0, 10.0, -1.0]).unwrap(),
    );
    run(
        kernel,
        KernelRequest::boolean(BooleanOp::Cut, &block, &pin.shape),
    )
}

fn counts(faces: u32, edges: u32, vertices: u32, solids: u32) -> ElementCounts {
    ElementCounts {
        faces,
        edges,
        vertices,
        solids,
    }
}

#[test]
fn a_part_is_built_symbolically_and_described_as_built() {
    let kernel = FakeKernel::new(2);
    let hole = holed_block(&kernel);
    assert_eq!(
        kernel.describe(&hole.shape).as_deref(),
        Some("cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))")
    );
    assert_eq!(hole.shape.valid(), Some(true));
    // A box's 6, 12, 8 and a cylinder's 3, 3, 2, side by side.
    assert_eq!(kernel.counts(&hole.shape), Some(counts(9, 15, 10, 2)));
}

#[test]
fn each_primitive_has_occts_element_counts() {
    let kernel = FakeKernel::new(1);
    let cases = [
        (block(1.0, 2.0, 3.0), counts(6, 12, 8, 1)),
        (
            Primitive::Cylinder {
                radius: 1.0,
                height: 2.0,
            },
            counts(3, 3, 2, 1),
        ),
        (Primitive::Sphere { radius: 1.0 }, counts(1, 3, 2, 1)),
        (
            Primitive::Cone {
                radius1: 2.0,
                radius2: 1.0,
                height: 3.0,
            },
            counts(3, 3, 2, 1),
        ),
        (
            Primitive::Cone {
                radius1: 2.0,
                radius2: 0.0,
                height: 3.0,
            },
            counts(2, 3, 2, 1),
        ),
        (
            Primitive::Torus {
                major_radius: 5.0,
                minor_radius: 1.0,
            },
            counts(1, 2, 1, 1),
        ),
    ];
    for (primitive, expected) in cases {
        let shape = run(&kernel, KernelRequest::make(primitive).unwrap());
        assert_eq!(kernel.counts(&shape), Some(expected), "{primitive:?}");
    }
}

#[test]
fn a_move_reports_each_element_modified_into_its_own_number_or_kept_by_nothing() {
    let kernel = FakeKernel::new(1);
    let shape = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    let moved = run(
        &kernel,
        KernelRequest::translate(&shape, [1.0, 0.0, 0.0]).unwrap(),
    );
    let input = &moved.history.inputs[0];
    assert_eq!(
        (input.faces.len(), input.edges.len(), input.vertices.len()),
        (6, 12, 8)
    );
    for (n, fate) in input.edges.iter().enumerate() {
        assert_eq!(fate.kept, None);
        assert_eq!(fate.modified.len(), 1);
        assert_eq!(fate.modified[0].element.kind, ElementKind::Edge);
        assert_eq!(fate.modified[0].element.index as usize, n + 1);
        assert_eq!(fate.modified[0].position, 1);
        assert!(fate.generated.is_empty());
    }
    let still = run(
        &kernel,
        KernelRequest::translate(&shape, [0.0, 0.0, 0.0]).unwrap(),
    );
    let input = &still.history.inputs[0];
    for (n, fate) in input.faces.iter().enumerate() {
        assert_eq!(fate.kept, Some(n as u32 + 1));
        assert!(fate.modified.is_empty());
    }
    assert_eq!(
        kernel.describe(&still.shape).as_deref(),
        Some("translate(box(1, 1, 1), 0, 0, 0)")
    );
}

#[test]
fn a_boolean_keeps_the_base_then_numbers_the_tools_elements_after_it() {
    let kernel = FakeKernel::new(1);
    let hole = holed_block(&kernel);
    let [base, tool] = &hole.history.inputs[..] else {
        panic!("{:?}", hole.history)
    };
    assert_eq!(base.faces[5].kept, Some(6));
    assert_eq!(tool.faces[0].kept, Some(7));
    assert_eq!(tool.edges[2].kept, Some(15));
    assert_eq!(tool.vertices[1].kept, Some(10));
    assert_eq!(hole.history.result, counts(9, 15, 10, 2));
}

#[test]
fn the_same_request_makes_the_same_bytes_on_any_fake_kernel() {
    let one = FakeKernel::new(1);
    let other = FakeKernel::new(4);
    let a = holed_block(&one);
    let b = holed_block(&other);
    assert_eq!(a.shape, b.shape);
    // A different value is a different shape.
    let c = run(
        &one,
        KernelRequest::make(block(40.0, 20.0, 10.000001)).unwrap(),
    );
    let d = run(&one, KernelRequest::make(block(40.0, 20.0, 10.0)).unwrap());
    assert_ne!(c, d);
}

#[test]
fn every_call_is_recorded_with_its_generation_and_operation() {
    let kernel = FakeKernel::new(1);
    kernel.advance(Generation(3));
    holed_block(&kernel);
    let calls = kernel.calls();
    let names: Vec<&str> = calls.iter().map(|c| c.op.name()).collect();
    assert_eq!(names, ["MakeBox", "MakeCylinder", "Translate", "Cut"]);
    assert!(calls.iter().all(|c| c.generation == Generation(3)));
    assert_eq!(
        calls[0].op,
        Op::MakePrimitive(Primitive::Box {
            dx: 40.0,
            dy: 20.0,
            dz: 10.0
        })
    );
}

#[test]
fn a_value_the_request_refuses_never_reaches_the_kernel() {
    let kernel = FakeKernel::new(1);
    let error = KernelRequest::make(block(0.0, 1.0, 1.0)).err().unwrap();
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert!(kernel.calls().is_empty());
}

#[test]
fn a_newer_generation_supersedes_requests_waiting_and_held() {
    let kernel = FakeKernel::new(1);
    let hold = kernel.hold(|_| true);
    let first = kernel.submit(
        Generation(0),
        KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap(),
    );
    assert!(kernel.wait_running(1, WAIT));
    let second = kernel.submit(
        Generation(0),
        KernelRequest::make(block(2.0, 2.0, 2.0)).unwrap(),
    );
    kernel.advance(Generation(1));
    assert!(matches!(first.wait(), Outcome::Superseded));
    assert!(matches!(second.wait(), Outcome::Superseded));
    // Only the first started; the second was dropped waiting.
    assert_eq!(kernel.calls().len(), 1);
    // A request for the superseded generation is superseded without starting.
    let late = kernel.submit(Generation(0), KernelRequest::echo(Blob::inline(vec![1])));
    assert!(matches!(late.wait(), Outcome::Superseded));
    assert_eq!(kernel.calls().len(), 1);
    drop(hold);
    let current = kernel.submit(Generation(1), KernelRequest::echo(Blob::inline(vec![2])));
    assert_eq!(done(current.wait()), Blob::inline(vec![2]));
}

#[test]
fn a_result_finished_after_its_generation_is_superseded_is_discarded() {
    let kernel = FakeKernel::new(1);
    // Superseding a held request stops it, so finish it first: refuse nothing, but let the
    // generation advance between its start and its reply by holding the next request.
    let hold = kernel.hold(|op| matches!(op, Op::Echo { .. }));
    let running = kernel.submit(Generation(0), KernelRequest::echo(Blob::inline(vec![])));
    assert!(kernel.wait_running(1, WAIT));
    kernel.advance(Generation(1));
    hold.release();
    assert!(matches!(running.wait(), Outcome::Superseded));
}

#[test]
fn requests_run_at_once_up_to_the_kernels_size() {
    let kernel = FakeKernel::new(2);
    let hold = kernel.hold(|_| true);
    let pending: Vec<_> = (1..=3)
        .map(|n| {
            kernel.submit(
                Generation(0),
                KernelRequest::make(block(f64::from(n), 1.0, 1.0)).unwrap(),
            )
        })
        .collect();
    assert!(kernel.wait_running(2, WAIT));
    assert!(!kernel.wait_running(3, Duration::from_millis(100)));
    hold.release();
    for p in pending {
        done(p.wait());
    }
    assert_eq!(kernel.most_running(), 2);
}

#[test]
fn a_refused_request_comes_back_with_the_error_given() {
    let kernel = FakeKernel::new(1);
    kernel.refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Sphere { .. })).then(|| {
            silico_core::Error::new(ErrorCode::KernelFailed, "MakeSphere failed")
                .with_cause("StdFail_NotDone")
        })
    });
    let outcome = kernel
        .submit(
            Generation(0),
            KernelRequest::make(Primitive::Sphere { radius: 1.0 }).unwrap(),
        )
        .wait();
    let Outcome::Done(Err(error)) = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(error.code(), ErrorCode::KernelFailed);
    assert_eq!(error.cause(), Some("StdFail_NotDone"));
    // Others are answered.
    run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
}

#[test]
fn an_operation_the_fake_doesnt_do_is_an_internal_error() {
    let kernel = FakeKernel::new(1);
    let shape = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    let outcome = kernel
        .submit(Generation(0), KernelRequest::summarize(&shape))
        .wait();
    let Outcome::Done(Err(error)) = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(error.code(), ErrorCode::Internal);
    assert_eq!(error.message(), "the fake kernel doesn't do Summarize");
}

#[test]
fn a_shape_the_fake_didnt_make_is_refused() {
    let kernel = FakeKernel::new(1);
    let other = FakeKernel::new(1);
    let foreign = run(&other, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    for shape in [
        foreign,
        ShapeData::checked(Blob::inline(b"not a shape".to_vec()), true),
    ] {
        let outcome = kernel
            .submit(
                Generation(0),
                KernelRequest::translate(&shape, [1.0, 0.0, 0.0]).unwrap(),
            )
            .wait();
        let Outcome::Done(Err(error)) = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(kernel.describe(&shape), None);
    }
}

#[test]
fn dropping_the_kernel_ends_requests_waiting_and_held() {
    let kernel = FakeKernel::new(1);
    let hold = kernel.hold(|_| true);
    let held = kernel.submit(Generation(0), KernelRequest::echo(Blob::inline(vec![])));
    assert!(kernel.wait_running(1, WAIT));
    let waiting = kernel.submit(Generation(0), KernelRequest::echo(Blob::inline(vec![])));
    drop(kernel);
    for pending in [held, waiting] {
        let Outcome::Done(Err(error)) = pending.wait() else {
            panic!("expected the stop's error")
        };
        assert_eq!(error.code(), ErrorCode::Internal);
    }
    drop(hold);
}

#[test]
fn a_long_chain_of_moves_is_described_without_overflowing_the_stack() {
    // Moves, not booleans: a boolean's history lists every element of its base, which grows
    // along a chain, where a move's stays a box's.
    let kernel = FakeKernel::new(1);
    let mut shape = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    let links = 100_000;
    for _ in 0..links {
        shape = run(
            &kernel,
            KernelRequest::translate(&shape, [1.0, 0.0, 0.0]).unwrap(),
        )
        .shape;
    }
    let text = kernel.describe(&shape).unwrap();
    assert!(text.starts_with("translate(translate(translate("));
    assert_eq!(text.matches("translate(").count(), links);
    assert!(text.contains("(box(1, 1, 1), 1, 0, 0)"));
}
