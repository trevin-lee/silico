//! M2.3: the in-memory kernel. Core tests run on it without OCCT, through the same
//! [`Kernel`] trait the worker pool implements, and see every call it was given. Expected
//! values are written by hand: OCCT's element counts for each primitive (checked against
//! the real kernel in silico-kernel's tests/fake_kernel.rs), and shapes described as built.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    Blob, BooleanOp, BuiltData, Curve, ElementCounts, ElementKind, FaceRegions, Generation, Kernel,
    KernelRequest, Op, Primitive, Region, ShapeData, SketchPlane,
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

#[test]
fn the_fake_summarizes_a_cone_and_a_torus_of_any_radii_by_formula() {
    // Found by the milestone's mutation run: the cone above has a first radius of 2, where
    // a·a = a + a and 2a = 2 + a, and the torus a second radius of 1, where Rr² = Rr/r, so
    // those slips passed every test. Worked by hand: a cone of radii 3 and 1.5, 4 high, has
    // a² + ab + b² = 15.75, so a volume of 21π, a slant of √18.25, an area of
    // π(4.5·√18.25 + 11.25), and its centroid at 4·24.75/63 = 11/7; upside down (radii 1.5
    // and 3), the same volume, area, and bounds, its centroid at 4·38.25/63 = 17/7 (and the
    // bounds of its wider end, the second); a torus of radii 5 and 3 has a volume of 90π²
    // and an area of 60π².
    use std::f64::consts::PI;
    let kernel = FakeKernel::new(1).unwrap();
    let close = |a: f64, b: f64| (a - b).abs() <= 1e-12 * b.abs().max(1.0);
    let summary = |primitive: Primitive| {
        let shape = run(&kernel, KernelRequest::make(primitive).unwrap());
        run(&kernel, KernelRequest::summarize(&shape))
    };
    let cone = summary(Primitive::Cone {
        radius1: 3.0,
        radius2: 1.5,
        height: 4.0,
    });
    assert!(close(cone.volume, 21.0 * PI), "{cone:?}");
    assert!(
        close(cone.area, PI * (4.5 * 18.25f64.sqrt() + 11.25)),
        "{cone:?}"
    );
    assert_eq!(cone.center[..2], [0.0, 0.0]);
    assert!(close(cone.center[2], 11.0 / 7.0), "{cone:?}");
    assert_eq!(cone.bounds, Some([[-3.0, -3.0, 0.0], [3.0, 3.0, 4.0]]));
    let upside_down = summary(Primitive::Cone {
        radius1: 1.5,
        radius2: 3.0,
        height: 4.0,
    });
    assert!(close(upside_down.volume, 21.0 * PI), "{upside_down:?}");
    assert!(
        close(upside_down.area, PI * (4.5 * 18.25f64.sqrt() + 11.25)),
        "{upside_down:?}"
    );
    assert_eq!(upside_down.center[..2], [0.0, 0.0]);
    assert!(close(upside_down.center[2], 17.0 / 7.0), "{upside_down:?}");
    assert_eq!(
        upside_down.bounds,
        Some([[-3.0, -3.0, 0.0], [3.0, 3.0, 4.0]])
    );
    let torus = summary(Primitive::Torus {
        major_radius: 5.0,
        minor_radius: 3.0,
    });
    assert!(close(torus.volume, 90.0 * PI * PI), "{torus:?}");
    assert!(close(torus.area, 60.0 * PI * PI), "{torus:?}");
    assert_eq!(torus.center, [0.0; 3]);
    assert_eq!(torus.bounds, Some([[-8.0, -8.0, -3.0], [8.0, 8.0, 3.0]]));
}

fn line(start: [f64; 2], end: [f64; 2]) -> Curve {
    Curve::Line { start, end }
}

fn circle(center: [f64; 2], radius: f64) -> Curve {
    Curve::Circle { center, radius }
}

/// The `w` × `h` rectangle from the origin, counter-clockwise.
fn rectangle(w: f64, h: f64) -> Vec<Curve> {
    let p = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
    (0..4).map(|i| line(p[i], p[(i + 1) % 4])).collect()
}

/// A slot of half circles r 1 about (0, 0) and (4, 0), counter-clockwise: 8 + π.
fn slot() -> Vec<Curve> {
    use std::f64::consts::FRAC_PI_2;
    let arc = |center: [f64; 2], start, end, start_angle, end_angle| Curve::Arc {
        center,
        start,
        end,
        radius: 1.0,
        start_angle,
        end_angle,
    };
    vec![
        line([0.0, -1.0], [4.0, -1.0]),
        arc([4.0, 0.0], [4.0, -1.0], [4.0, 1.0], -FRAC_PI_2, FRAC_PI_2),
        line([4.0, 1.0], [0.0, 1.0]),
        arc(
            [0.0, 0.0],
            [0.0, 1.0],
            [0.0, -1.0],
            FRAC_PI_2,
            3.0 * FRAC_PI_2,
        ),
    ]
}

fn regions(plane: SketchPlane, regions: Vec<(Vec<Curve>, Vec<Vec<Curve>>)>) -> FaceRegions {
    FaceRegions {
        plane,
        regions: regions
            .into_iter()
            .map(|(outer, holes)| Region { outer, holes })
            .collect(),
    }
}

#[test]
fn faces_are_made_symbolically_with_their_areas_by_formula() {
    use std::f64::consts::PI;
    let kernel = FakeKernel::new(1).unwrap();
    // FreeCAD's FaceMakerBullseye cases, with their areas worked out by hand.
    let cases = [
        (
            vec![(rectangle(4.0, 3.0), vec![])],
            12.0,
            counts(1, 4, 4, 0),
        ),
        (
            vec![(rectangle(4.0, 3.0), vec![vec![circle([2.0, 1.5], 0.5)]])],
            12.0 - PI / 4.0,
            counts(1, 5, 5, 0),
        ),
        (
            vec![(
                rectangle(10.0, 6.0),
                vec![
                    vec![circle([2.0, 3.0], 0.5)],
                    vec![circle([5.0, 3.0], 1.0)],
                    vec![circle([8.0, 3.0], 0.75)],
                ],
            )],
            60.0 - 1.8125 * PI,
            counts(1, 7, 7, 0),
        ),
        (
            vec![
                (rectangle(10.0, 10.0), vec![vec![circle([5.0, 5.0], 4.0)]]),
                (vec![circle([5.0, 5.0], 2.0)], vec![]),
            ],
            100.0 - 12.0 * PI,
            counts(2, 6, 6, 0),
        ),
        (
            vec![(vec![circle([0.0, 0.0], 1.0)], vec![])],
            PI,
            counts(1, 1, 1, 0),
        ),
        (vec![(slot(), vec![])], 8.0 + PI, counts(1, 4, 4, 0)),
    ];
    for (sent, area, want) in cases {
        for plane in SketchPlane::ALL {
            let faces = regions(plane, sent.clone());
            let built = run(&kernel, KernelRequest::make_faces(&faces).unwrap());
            assert_eq!(FakeKernel::counts(&built.shape), Some(want));
            assert_eq!(built.history.result, want);
            let summary = run(&kernel, KernelRequest::summarize(&built.shape));
            assert!(
                (summary.area - area).abs() <= 1e-12 * area,
                "{plane:?}: {} for {area}",
                summary.area
            );
            assert_eq!(summary.volume, 0.0);
            // Every curve kept as the edge of its number; every point as a vertex.
            let input = &built.history.inputs[0];
            assert!(input.faces.is_empty());
            for (n, fate) in input.edges.iter().enumerate() {
                assert_eq!(fate.kept, Some(n as u32 + 1));
            }
            assert_eq!(input.vertices.len(), faces.points().count());
        }
    }
    // The rectangle with a hole, described, on XZ, where the sketch's y is +Z: its box.
    let faces = regions(
        SketchPlane::Xz,
        vec![(rectangle(4.0, 3.0), vec![vec![circle([2.0, 1.5], 0.5)]])],
    );
    let built = run(&kernel, KernelRequest::make_faces(&faces).unwrap());
    assert_eq!(
        FakeKernel::describe(&built.shape).as_deref(),
        Some("faces(xz, 1 regions, 5 curves)")
    );
    let summary = run(&kernel, KernelRequest::summarize(&built.shape));
    assert_eq!(summary.bounds, Some([[0.0, 0.0, 0.0], [4.0, 0.0, 3.0]]));
    // The slot's arcs reach past its points, to x = -1 and x = 5.
    let slotted = run(
        &kernel,
        KernelRequest::make_faces(&regions(SketchPlane::Yz, vec![(slot(), vec![])])).unwrap(),
    );
    let summary = run(&kernel, KernelRequest::summarize(&slotted.shape));
    assert_eq!(summary.bounds, Some([[0.0, -1.0, -1.0], [0.0, 5.0, 1.0]]));
}

#[test]
fn faces_boxes_reach_as_far_as_their_circles_and_arcs_go() {
    use std::f64::consts::PI;
    let kernel = FakeKernel::new(1).unwrap();
    // Arcs r 5 about (2, 1), between points 3-4-5 from it, each closed by its chord.
    let arc = |start: [f64; 2], end: [f64; 2]| {
        let angle = |p: [f64; 2]| libm::atan2(p[1] - 1.0, p[0] - 2.0);
        let (start_angle, mut end_angle) = (angle(start), angle(end));
        if end_angle < start_angle {
            end_angle += 2.0 * PI;
        }
        vec![
            Curve::Arc {
                center: [2.0, 1.0],
                start,
                end,
                radius: 5.0,
                start_angle,
                end_angle,
            },
            line(end, start),
        ]
    };
    // a: the angle of (4, 3) from the centre; b: of (3, 4).
    let (a, b) = (libm::atan2(3.0, 4.0), libm::atan2(4.0, 3.0));
    // Each region, its area by hand, and its box on XY by hand. A segment of angle θ is
    // (r²/2)(θ − sin θ); sin 2a = sin 2b = 2·0.6·0.8 = 0.96.
    let cases = [
        // A ring: circles r 2 and r 1 about (5, 3). Its box is the outer circle's.
        (
            vec![(
                vec![circle([5.0, 3.0], 2.0)],
                vec![vec![circle([5.0, 3.0], 1.0)]],
            )],
            3.0 * PI,
            [[3.0, 1.0, 0.0], [7.0, 5.0, 0.0]],
        ),
        // From (6, 4) over the top to (−2, 4): past 90°, to y = 6.
        (
            vec![(arc([6.0, 4.0], [-2.0, 4.0]), vec![])],
            12.5 * (PI - 2.0 * a - 0.96),
            [[-2.0, 4.0, 0.0], [6.0, 6.0, 0.0]],
        ),
        // From (6, 4) round past the top, the left, and the bottom to (6, −2): past 90°,
        // 180°, and 270°, and not 360° (2π − a, short of it).
        (
            vec![(arc([6.0, 4.0], [6.0, -2.0]), vec![])],
            12.5 * (2.0 * PI - 2.0 * a + 0.96),
            [[-3.0, -4.0, 0.0], [6.0, 6.0, 0.0]],
        ),
        // From (−1, 5), at π − b, past the left to (−1, −3): past 180° only.
        (
            vec![(arc([-1.0, 5.0], [-1.0, -3.0]), vec![])],
            12.5 * (2.0 * b - 0.96),
            [[-3.0, -3.0, 0.0], [-1.0, 5.0, 0.0]],
        ),
    ];
    for (sent, area, bounds) in cases {
        let faces = regions(SketchPlane::Xy, sent);
        let built = run(&kernel, KernelRequest::make_faces(&faces).unwrap());
        let summary = run(&kernel, KernelRequest::summarize(&built.shape));
        assert!(
            (summary.area - area).abs() <= 1e-12 * area,
            "{} for {area}",
            summary.area
        );
        assert_eq!(summary.bounds, Some(bounds), "{faces:?}");
    }
}

#[test]
fn faces_read_back_and_move_as_any_fake_shape() {
    let kernel = FakeKernel::new(1).unwrap();
    let faces = regions(SketchPlane::Xy, vec![(slot(), vec![])]);
    let built = run(&kernel, KernelRequest::make_faces(&faces).unwrap());
    // The same request, the same bytes, on another fake kernel.
    let other = FakeKernel::new(1).unwrap();
    assert_eq!(
        run(&other, KernelRequest::make_faces(&faces).unwrap()),
        built
    );
    let moved = run(
        &kernel,
        KernelRequest::translate(&built.shape, [0.0, 0.0, 2.0]).unwrap(),
    );
    let summary = run(&kernel, KernelRequest::summarize(&moved.shape));
    assert_eq!(summary.bounds, Some([[-1.0, -1.0, 2.0], [5.0, 1.0, 2.0]]));
    assert_eq!(
        FakeKernel::describe(&moved.shape).as_deref(),
        Some("translate(faces(xy, 1 regions, 4 curves), 0, 0, 2)")
    );
}

#[test]
fn regions_that_cant_make_faces_never_reach_the_kernel() {
    let kernel = FakeKernel::new(1).unwrap();
    let mut open = rectangle(4.0, 3.0);
    open.pop();
    let mut nan = rectangle(4.0, 3.0);
    nan[0] = line([0.0, 0.0], [f64::NAN, 0.0]);
    let mut clockwise = rectangle(4.0, 3.0);
    clockwise.reverse();
    for c in &mut clockwise {
        if let Curve::Line { start, end } = c {
            std::mem::swap(start, end);
        }
    }
    let bad = [
        regions(SketchPlane::Xy, vec![]),
        regions(SketchPlane::Xy, vec![(open, vec![])]),
        regions(SketchPlane::Xy, vec![(nan, vec![])]),
        regions(SketchPlane::Xy, vec![(clockwise, vec![])]),
        regions(SketchPlane::Xy, vec![(vec![], vec![])]),
    ];
    for faces in &bad {
        let error = KernelRequest::make_faces(faces).err().unwrap();
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
        assert_eq!(error.parameter(), Some("regions"));
        // Regions come from a profile, which refuses by name all the request does.
        assert_eq!(
            error.suggestion(),
            Some("send the faces of a profile the core made: this is a bug in the profile")
        );
    }
    assert!(kernel.calls().is_empty());
}

// --- Sweeps of faces (M3.6) -----------------------------------------------------------------

/// The fake's faces of `sent` on `plane`.
fn made_faces<K: Kernel>(
    kernel: &K,
    plane: SketchPlane,
    sent: Vec<(Vec<Curve>, Vec<Vec<Curve>>)>,
) -> BuiltData {
    run(
        kernel,
        KernelRequest::make_faces(&regions(plane, sent)).unwrap(),
    )
}

fn z_axis() -> silico_core::kernel::Axis {
    silico_core::kernel::Axis {
        origin: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
    }
}

#[test]
fn a_sweep_reports_each_element_as_the_kernel_does_with_the_fakes_own_numbers() {
    use silico_core::kernel::SweepKind;
    use std::f64::consts::{FRAC_PI_2, TAU};
    let kernel = FakeKernel::new(1).unwrap();
    // A rectangle with a round hole: 1 face, 5 edges, 5 vertices.
    let faces = made_faces(
        &kernel,
        SketchPlane::Xy,
        vec![(rectangle(4.0, 3.0), vec![vec![circle([2.0, 1.5], 0.5)]])],
    );
    let prism = run(
        &kernel,
        KernelRequest::prism(&faces.shape, [0.0, 0.0, -1.0], [0.0, 0.0, 2.0]).unwrap(),
    );
    // As the kernel's prism: twice the faces and the edges' sides; twice the edges and the
    // vertices' sides; twice the vertices; a solid.
    assert_eq!(prism.history.result, counts(7, 15, 10, 1));
    assert_eq!(FakeKernel::counts(&prism.shape), Some(counts(7, 15, 10, 1)));
    assert_eq!(prism.history.validate(1), Ok(()));
    assert_eq!(prism.history.sweep_fault(SweepKind::Prism), None);
    let input = &prism.history.inputs[0];
    // By the fake's numbering: the face starts at Face6, ends at Face7, sweeps Solid1; edge
    // 2 starts at Edge2, ends at Edge7, sweeps Face2; vertex 3 starts at Vertex3, ends at
    // Vertex8, sweeps Edge13.
    let reported = |list: &[silico_core::kernel::Reported]| -> Vec<String> {
        list.iter().map(|r| r.element.to_string()).collect()
    };
    assert_eq!(reported(&input.faces[0].modified), ["Face6"]);
    assert_eq!(reported(&input.faces[0].generated), ["Face7", "Solid1"]);
    assert_eq!(reported(&input.edges[1].modified), ["Edge2"]);
    assert_eq!(reported(&input.edges[1].generated), ["Edge7", "Face2"]);
    assert_eq!(reported(&input.vertices[2].modified), ["Vertex3"]);
    assert_eq!(
        reported(&input.vertices[2].generated),
        ["Vertex8", "Edge13"]
    );
    assert!(
        input
            .faces
            .iter()
            .chain(&input.edges)
            .chain(&input.vertices)
            .all(|f| f.kept.is_none())
    );
    // The hole's side face: its start and end circles and its seam's side edge.
    assert_eq!(prism.history.topology.face_edges[4], [5, 10, 15]);
    assert_eq!(
        FakeKernel::describe(&prism.shape).as_deref(),
        Some("prism(faces(xy, 1 regions, 5 curves), 0, 0, -1, 0, 0, 2)")
    );
    // A turn reports as a prism does; a whole turn: each edge its seam and face, each vertex
    // itself and its circle, the face nothing.
    let turned = run(
        &kernel,
        KernelRequest::revolution(&faces.shape, z_axis(), 0.0, FRAC_PI_2).unwrap(),
    );
    assert_eq!(turned.history.result, counts(7, 15, 10, 1));
    assert_eq!(turned.history.sweep_fault(SweepKind::Turn), None);
    let whole = run(
        &kernel,
        KernelRequest::revolution(&faces.shape, z_axis(), 0.0, TAU).unwrap(),
    );
    assert_eq!(whole.history.result, counts(5, 10, 5, 1));
    assert_eq!(whole.history.validate(1), Ok(()));
    assert_eq!(whole.history.sweep_fault(SweepKind::WholeTurn), None);
    let input = &whole.history.inputs[0];
    assert_eq!(input.faces[0], Default::default());
    assert_eq!(reported(&input.edges[4].modified), ["Edge5"]);
    assert_eq!(reported(&input.edges[4].generated), ["Face5"]);
    assert_eq!(reported(&input.vertices[4].generated), ["Edge10"]);
    // The circle's face: its seam, and its vertex's circle.
    assert_eq!(whole.history.topology.face_edges[4], [5, 10]);
}

#[test]
fn a_sweeps_measures_are_the_fakes_own_model_by_formula() {
    use std::f64::consts::{PI, TAU};
    let kernel = FakeKernel::new(1).unwrap();
    // A 4 × 3 rectangle on XZ (normal −Y), swept two sides 10 and 5: 12 × 15, from y = 5
    // to y = −10.
    let faces = made_faces(
        &kernel,
        SketchPlane::Xz,
        vec![(rectangle(4.0, 3.0), vec![])],
    );
    let prism = run(
        &kernel,
        KernelRequest::prism(&faces.shape, [0.0, 5.0, 0.0], [0.0, -15.0, 0.0]).unwrap(),
    );
    let s = run(&kernel, KernelRequest::summarize(&prism.shape));
    assert_eq!(s.volume, 180.0);
    assert_eq!(s.area, 2.0 * 12.0 + 14.0 * 15.0);
    assert_eq!(s.bounds, Some([[0.0, -10.0, 0.0], [4.0, 5.0, 3.0]]));
    assert_eq!(s.center, [2.0, -2.5, 1.5]);
    assert_eq!((s.faces, s.edges, s.vertices), (6, 12, 8));
    // A circle r 1 at 5 from the z axis, turned a whole turn: Pappus, 2π²·5·1.
    let ring = made_faces(
        &kernel,
        SketchPlane::Xz,
        vec![(vec![circle([5.0, 0.0], 1.0)], vec![])],
    );
    let torus = run(
        &kernel,
        KernelRequest::revolution(&ring.shape, z_axis(), 0.0, TAU).unwrap(),
    );
    let s = run(&kernel, KernelRequest::summarize(&torus.shape));
    assert!(
        (s.volume - 2.0 * PI * PI * 5.0).abs() < 1e-12,
        "{}",
        s.volume
    );
    assert!((s.area - 4.0 * PI * PI * 5.0).abs() < 1e-12, "{}", s.area);
    // Through all sizes by the protocol's rule from the fake's boxes: a 10 mm cube and a
    // square on XY in it, √300 × 2.02 long.
    let cube = run(
        &kernel,
        KernelRequest::make(block(10.0, 10.0, 10.0)).unwrap(),
    );
    let square = made_faces(
        &kernel,
        SketchPlane::Xy,
        vec![(rectangle(2.0, 2.0), vec![])],
    );
    let through = run(
        &kernel,
        KernelRequest::prism_through_all(&square.shape, &cube, [0.0, 0.0, 1.0]).unwrap(),
    );
    let s = run(&kernel, KernelRequest::summarize(&through.shape));
    let length = silico_protocol::THROUGH_ALL_FACTOR * 300f64.sqrt();
    assert_eq!(s.volume, 4.0 * length);
    assert_eq!(s.bounds, Some([[0.0; 3], [2.0, 2.0, length]]));
}

#[test]
fn a_sweep_reads_back_as_built_and_only_faces_are_swept() {
    let kernel = FakeKernel::new(1).unwrap();
    let faces = made_faces(&kernel, SketchPlane::Yz, vec![(slot(), vec![])]);
    let prism = run(
        &kernel,
        KernelRequest::prism(&faces.shape, [0.0; 3], [3.0, 0.0, 0.0]).unwrap(),
    );
    // Another fake kernel reads it, and booleans of it read back.
    let other = FakeKernel::new(1).unwrap();
    let cube = run(
        &other,
        KernelRequest::make(block(10.0, 10.0, 10.0)).unwrap(),
    );
    let cut = run(
        &other,
        KernelRequest::boolean(BooleanOp::Cut, &cube, &prism.shape),
    );
    assert_eq!(
        FakeKernel::describe(&cut.shape).as_deref(),
        Some("cut(box(10, 10, 10), prism(faces(yz, 1 regions, 4 curves), 0, 0, 0, 3, 0, 0))")
    );
    let fused = run(
        &other,
        KernelRequest::boolean(BooleanOp::Fuse, &prism.shape, &cube),
    );
    assert!(FakeKernel::counts(&fused.shape).is_some());
    // A solid isn't faces, and neither is a sweep: refused as the kernel refuses them.
    for shape in [&cube, &prism.shape, &cut.shape] {
        let outcome = kernel
            .submit(
                Generation(0),
                KernelRequest::prism(shape, [0.0; 3], [0.0, 0.0, 1.0]).unwrap(),
            )
            .wait();
        let Outcome::Done(Err(error)) = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(error.parameter(), Some("faces"));
    }
    // Bytes a request couldn't have made: a sweep that isn't of the faces before it, or of
    // values the kernel refuses.
    for text in [
        "silico fake shape 1\nbox 1 1 1\nprism 0 0 0 0 0 0 1\n",
        "silico fake shape 1\nfaces xy region loop circle 0 0 1\nprism 0 0 0 0 0 0 0\n",
        "silico fake shape 1\nfaces xy region loop circle 0 0 1\nrevolution 0 0 0 0 0 0 1 0 7\n",
        "silico fake shape 1\nfaces xy region loop circle 0 0 1\nprism 0 0 0 0 0 0 1 9\n",
    ] {
        let shape = ShapeData::checked(Blob::inline(text.as_bytes().to_vec()), true);
        assert_eq!(FakeKernel::describe(&shape), None, "{text}");
    }
    let good =
        "silico fake shape 1\nfaces xy region loop circle 0 0 1\nrevolution 0 2 0 0 0 1 0 0 1\n";
    let shape = ShapeData::checked(Blob::inline(good.as_bytes().to_vec()), true);
    assert_eq!(
        FakeKernel::describe(&shape).as_deref(),
        Some("revolution(faces(xy, 1 regions, 1 curves), 2, 0, 0, 0, 1, 0, 0, 1)")
    );
}
