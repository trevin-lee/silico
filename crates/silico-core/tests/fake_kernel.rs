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
    let kernel = FakeKernel::new(2).unwrap();
    let hole = holed_block(&kernel);
    assert_eq!(
        FakeKernel::describe(&hole.shape).as_deref(),
        Some("cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))")
    );
    assert_eq!(hole.shape.valid(), Some(true));
    // A box's 6, 12, 8 and a cylinder's 3, 3, 2, side by side.
    assert_eq!(FakeKernel::counts(&hole.shape), Some(counts(9, 15, 10, 2)));
}

#[test]
fn each_primitive_has_occts_element_counts() {
    let kernel = FakeKernel::new(1).unwrap();
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
        assert_eq!(FakeKernel::counts(&shape), Some(expected), "{primitive:?}");
    }
}

#[test]
fn a_move_reports_each_element_modified_into_its_own_number_or_kept_by_nothing() {
    let kernel = FakeKernel::new(1).unwrap();
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
    // A move by nothing is the shape it moved, as the kernel gives it back.
    assert_eq!(still.shape, shape);
}

#[test]
fn a_boolean_keeps_the_base_then_numbers_the_tools_elements_after_it() {
    let kernel = FakeKernel::new(1).unwrap();
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
    let one = FakeKernel::new(1).unwrap();
    let other = FakeKernel::new(4).unwrap();
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
    let kernel = FakeKernel::new(1).unwrap();
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
    let kernel = FakeKernel::new(1).unwrap();
    let error = KernelRequest::make(block(0.0, 1.0, 1.0)).err().unwrap();
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert!(kernel.calls().is_empty());
}

#[test]
fn a_newer_generation_supersedes_requests_waiting_and_held() {
    let kernel = FakeKernel::new(1).unwrap();
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
fn a_held_request_superseded_ends_before_it_stops_counting_as_running() {
    // A race, so run many times: a kernel that stopped counting a request before sending
    // its outcome fails in most runs of this.
    let kernel = FakeKernel::new(1).unwrap();
    let _hold = kernel.hold(|_| true);
    for round in 0..1000u64 {
        let held = kernel.submit(Generation(round), KernelRequest::echo(Blob::inline(vec![])));
        assert!(kernel.wait_running(1, WAIT));
        kernel.advance(Generation(round + 1));
        // Once it isn't running, its outcome has been sent: no wait for it.
        while kernel.wait_running(1, Duration::ZERO) {}
        assert!(
            matches!(held.try_take(), Ok(Outcome::Superseded)),
            "round {round}"
        );
    }
    assert!(!kernel.wait_running(1, Duration::from_millis(100)));
}

#[test]
fn a_request_queued_for_the_generation_advance_starts_survives_it() {
    let kernel = FakeKernel::new(1).unwrap();
    let hold = kernel.hold(|op| matches!(op, Op::Echo { data } if data.as_bytes() == [0]));
    let blocker = kernel.submit(Generation(1), KernelRequest::echo(Blob::inline(vec![0])));
    assert!(kernel.wait_running(1, WAIT));
    let queued = kernel.submit(Generation(1), KernelRequest::echo(Blob::inline(vec![1])));
    kernel.advance(Generation(1));
    hold.release();
    assert_eq!(done(blocker.wait()), Blob::inline(vec![0]));
    assert_eq!(done(queued.wait()), Blob::inline(vec![1]));
}

#[test]
fn requests_run_at_once_up_to_the_kernels_size() {
    let kernel = FakeKernel::new(2).unwrap();
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
    let kernel = FakeKernel::new(1).unwrap();
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
    let kernel = FakeKernel::new(1).unwrap();
    let shape = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    let outcome = kernel
        .submit(Generation(0), KernelRequest::measure_as_freecad(&shape))
        .wait();
    let Outcome::Done(Err(error)) = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(error.code(), ErrorCode::Internal);
    assert_eq!(
        error.message(),
        "the fake kernel doesn't do MeasureAsFreecad"
    );
}

#[test]
fn a_shape_one_fake_kernel_made_is_read_by_another_as_a_worker_reads_any_brep() {
    let first = FakeKernel::new(1).unwrap();
    let made = run(&first, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    drop(first);
    let second = FakeKernel::new(1).unwrap();
    let moved = run(
        &second,
        KernelRequest::translate(&made, [1.0, 0.0, 0.0]).unwrap(),
    );
    assert_eq!(
        FakeKernel::describe(&moved.shape).as_deref(),
        Some("translate(box(1, 1, 1), 1, 0, 0)")
    );
}

#[test]
fn a_shape_whose_bytes_are_another_kernels_is_refused() {
    let kernel = FakeKernel::new(1).unwrap();
    let shape = ShapeData::checked(Blob::inline(b"not a shape".to_vec()), true);
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
    assert_eq!(FakeKernel::describe(&shape), None);
}

#[test]
fn a_boolean_of_booleans_reads_back_as_built() {
    let kernel = FakeKernel::new(1).unwrap();
    let a = run(&kernel, KernelRequest::make(block(1.0, 2.0, 3.0)).unwrap());
    let b = run(
        &kernel,
        KernelRequest::make(Primitive::Sphere { radius: 1.0 }).unwrap(),
    );
    let c = run(
        &kernel,
        KernelRequest::make(Primitive::Cylinder {
            radius: 1.0,
            height: 2.0,
        })
        .unwrap(),
    );
    let moved = run(
        &kernel,
        KernelRequest::translate(&c, [1.0, 0.0, 0.0]).unwrap(),
    )
    .shape;
    let inner = run(&kernel, KernelRequest::boolean(BooleanOp::Cut, &b, &moved)).shape;
    let base = run(
        &kernel,
        KernelRequest::translate(&a, [0.0, 1.0, 0.0]).unwrap(),
    )
    .shape;
    let outer = run(
        &kernel,
        KernelRequest::boolean(BooleanOp::Fuse, &base, &inner),
    )
    .shape;
    assert_eq!(
        std::str::from_utf8(outer.brep().as_bytes()).unwrap(),
        "silico fake shape 1\nbox 1 2 3\ntranslate 0 0 1 0\nsphere 1\ncylinder 1 2\ntranslate 3 1 0 0\ncut 2 4\nfuse 1 5\n"
    );
    assert_eq!(
        FakeKernel::describe(&outer).as_deref(),
        Some(
            "fuse(translate(box(1, 2, 3), 0, 1, 0), cut(sphere(1), translate(cylinder(1, 2), 1, 0, 0)))"
        )
    );
    // And a boolean of that with itself, each input's steps shifted after the base's.
    let again = run(
        &kernel,
        KernelRequest::boolean(BooleanOp::Common, &outer, &outer),
    )
    .shape;
    assert_eq!(
        FakeKernel::describe(&again).as_deref(),
        Some(
            "common(fuse(translate(box(1, 2, 3), 0, 1, 0), cut(sphere(1), translate(cylinder(1, 2), 1, 0, 0))), fuse(translate(box(1, 2, 3), 0, 1, 0), cut(sphere(1), translate(cylinder(1, 2), 1, 0, 0))))"
        )
    );
}

#[test]
fn a_boolean_of_booleans_counts_every_input() {
    // A box (6, 12, 8), a sphere (1, 3, 2), a cylinder (3, 3, 2): every boolean has both
    // inputs' elements, so the counts are sums of those, by hand.
    let kernel = FakeKernel::new(1).unwrap();
    let a = run(&kernel, KernelRequest::make(block(1.0, 2.0, 3.0)).unwrap());
    let b = run(
        &kernel,
        KernelRequest::make(Primitive::Sphere { radius: 1.0 }).unwrap(),
    );
    let c = run(
        &kernel,
        KernelRequest::make(Primitive::Cylinder {
            radius: 1.0,
            height: 2.0,
        })
        .unwrap(),
    );
    let inner = run(&kernel, KernelRequest::boolean(BooleanOp::Cut, &b, &c));
    assert_eq!(inner.history.result, counts(4, 6, 4, 2));
    let outer = run(
        &kernel,
        KernelRequest::boolean(BooleanOp::Fuse, &a, &inner.shape),
    );
    assert_eq!(outer.history.result, counts(10, 18, 12, 3));
    let again = run(
        &kernel,
        KernelRequest::boolean(BooleanOp::Common, &outer.shape, &outer.shape),
    );
    assert_eq!(again.history.result, counts(20, 36, 24, 6));
    assert_eq!(
        FakeKernel::counts(&again.shape),
        Some(counts(20, 36, 24, 6))
    );
    let [base, tool] = &again.history.inputs[..] else {
        panic!("{:?}", again.history)
    };
    assert_eq!(base.faces.len(), 10);
    assert_eq!(tool.faces.len(), 10);
    assert_eq!(tool.faces[0].kept, Some(11));
    assert_eq!(tool.faces[9].kept, Some(20));
    let moved = run(
        &kernel,
        KernelRequest::translate(&outer.shape, [1.0, 0.0, 0.0]).unwrap(),
    );
    let last = run(
        &kernel,
        KernelRequest::boolean(BooleanOp::Cut, &moved.shape, &a),
    );
    assert_eq!(last.history.result, counts(16, 30, 20, 4));
}

#[test]
fn a_shapes_bytes_are_its_steps_one_to_a_line() {
    let kernel = FakeKernel::new(1).unwrap();
    let hole = holed_block(&kernel);
    assert_eq!(
        std::str::from_utf8(hole.shape.brep().as_bytes()).unwrap(),
        "silico fake shape 1\nbox 40 20 10\ncylinder 2.5 12\ntranslate 1 20 10 -1\ncut 0 2\n"
    );
}

#[test]
fn dropping_the_kernel_ends_requests_waiting_and_held() {
    let kernel = FakeKernel::new(1).unwrap();
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
        assert_eq!(
            error.message(),
            "the fake kernel stopped before Echo was done"
        );
        assert_eq!(
            error.suggestion(),
            Some("submit the request to a running fake kernel")
        );
    }
    drop(hold);
}

#[test]
fn moves_compose_as_the_kernels_locations_do() {
    let kernel = FakeKernel::new(1).unwrap();
    let unit = run(&kernel, KernelRequest::make(block(1.0, 1.0, 1.0)).unwrap());
    let moved = |shape: &ShapeData, offset| {
        run(&kernel, KernelRequest::translate(shape, offset).unwrap()).shape
    };
    // 0.1 then 0.2 is one move by 0.1 + 0.2 (0.30000000000000004), not by 0.3.
    let twice = moved(&moved(&unit, [0.1, 0.0, 0.0]), [0.2, 0.0, 0.0]);
    assert_eq!(twice, moved(&unit, [0.1 + 0.2, 0.0, 0.0]));
    assert_ne!(twice, moved(&unit, [0.3, 0.0, 0.0]));
    // Moved and moved back is the shape itself.
    let there = moved(&unit, [1.0, -2.0, 3.0]);
    assert_eq!(moved(&there, [-1.0, 2.0, -3.0]), unit);
    // A long chain is one move.
    let mut shape = unit.clone();
    for _ in 0..10_000 {
        shape = moved(&shape, [1.0, 0.0, 0.0]);
    }
    assert_eq!(
        FakeKernel::describe(&shape).as_deref(),
        Some("translate(box(1, 1, 1), 10000, 0, 0)")
    );
}

#[test]
fn the_fake_summarizes_a_shape_by_formula() {
    // Expected values worked in Python, from the formulas: πr²h, 2πrh + 2πr²; a cone's
    // πh(a² + ab + b²)/3 and π(a + b)·slant + πa² + πb², its centroid at
    // h(a² + 2ab + 3b²)/(4(a² + ab + b²)); 4πr³/3 and 4πr²; 2π²Rr² and 4π²Rr.
    let kernel = FakeKernel::new(1).unwrap();
    let close = |a: f64, b: f64| (a - b).abs() <= 1e-12 * b.abs().max(1.0);
    let summary = |primitive: Primitive, offset: [f64; 3]| {
        let shape = run(&kernel, KernelRequest::make(primitive).unwrap());
        let shape = run(&kernel, KernelRequest::translate(&shape, offset).unwrap()).shape;
        (run(&kernel, KernelRequest::summarize(&shape)), shape)
    };
    let (boxed, box_shape) = summary(block(2.0, 3.0, 4.0), [0.0; 3]);
    assert_eq!((boxed.volume, boxed.area), (24.0, 52.0));
    assert_eq!(boxed.center, [1.0, 1.5, 2.0]);
    assert_eq!(boxed.bounds, Some([[0.0; 3], [2.0, 3.0, 4.0]]));
    assert_eq!((boxed.faces, boxed.edges, boxed.vertices), (6, 12, 8));
    assert!(boxed.valid);
    let (cylinder, cylinder_shape) = summary(
        Primitive::Cylinder {
            radius: 1.0,
            height: 2.0,
        },
        [10.0, 0.0, 0.0],
    );
    assert!(
        close(cylinder.volume, std::f64::consts::TAU),
        "{cylinder:?}"
    );
    assert!(close(cylinder.area, 18.84955592153876), "{cylinder:?}");
    assert_eq!(cylinder.center, [10.0, 0.0, 1.0]);
    assert_eq!(cylinder.bounds, Some([[9.0, -1.0, 0.0], [11.0, 1.0, 2.0]]));
    assert_eq!(
        (cylinder.faces, cylinder.edges, cylinder.vertices),
        (3, 3, 2)
    );
    let (cone, _) = summary(
        Primitive::Cone {
            radius1: 2.0,
            radius2: 1.0,
            height: 3.0,
        },
        [0.0; 3],
    );
    assert!(close(cone.volume, 21.991148575128552), "{cone:?}");
    assert!(close(cone.area, 45.511728065337266), "{cone:?}");
    assert!(close(cone.center[2], 1.1785714285714286), "{cone:?}");
    assert_eq!(cone.bounds, Some([[-2.0, -2.0, 0.0], [2.0, 2.0, 3.0]]));
    let (sphere, _) = summary(Primitive::Sphere { radius: 2.0 }, [0.0; 3]);
    assert!(close(sphere.volume, 33.510321638291124), "{sphere:?}");
    assert!(close(sphere.area, 50.26548245743669), "{sphere:?}");
    assert_eq!(sphere.bounds, Some([[-2.0; 3], [2.0; 3]]));
    let (torus, _) = summary(
        Primitive::Torus {
            major_radius: 3.0,
            minor_radius: 1.0,
        },
        [0.0; 3],
    );
    assert!(close(torus.volume, 59.21762640653615), "{torus:?}");
    assert!(close(torus.area, 118.4352528130723), "{torus:?}");
    assert_eq!(torus.bounds, Some([[-4.0, -4.0, -1.0], [4.0, 4.0, 1.0]]));
    // The fake's boolean: both inputs, as if they didn't touch.
    let fused = run(
        &kernel,
        KernelRequest::boolean(BooleanOp::Fuse, &box_shape, &cylinder_shape),
    )
    .shape;
    let fused = run(&kernel, KernelRequest::summarize(&fused));
    assert!(close(fused.volume, 30.283185307179586), "{fused:?}");
    assert!(close(fused.area, 70.84955592153875), "{fused:?}");
    for (got, want) in
        fused
            .center
            .into_iter()
            .zip([2.8673289216775233, 1.188778513053746, 1.7925190087024974])
    {
        assert!(close(got, want), "{fused:?}");
    }
    assert_eq!(fused.bounds, Some([[0.0, -1.0, 0.0], [11.0, 3.0, 4.0]]));
    assert_eq!((fused.faces, fused.edges, fused.vertices), (9, 15, 10));
}
