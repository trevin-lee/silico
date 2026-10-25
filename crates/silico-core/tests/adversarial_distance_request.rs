//! break-it: the core's distance request (`KernelRequest::distance`, M4.5;
//! docs/ports/measure.md) against a worker that lies (`FakeKernel::answer_instead`) or
//! refuses as a worker does (`FakeKernel::refuse_as_worker`), and the fake's own answers at
//! the ends of the element numbers. Real solids are measured on a worker in silico-kernel's
//! tests/adversarial_distance.rs. Every expected value is by hand.

use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    Blob, Distance, ElementKind, ElementRef, Kernel, KernelRequest, Op, Primitive, ShapeData,
};
use silico_core::pool::Outcome;
use silico_protocol::{Failure, Refusal, Reply};

fn run<T: Send + 'static>(
    kernel: &FakeKernel,
    request: KernelRequest<T>,
) -> Result<T, silico_core::Error> {
    match kernel
        .submit(kernel.generation(), request)
        .wait_timeout(Duration::from_secs(5))
    {
        Ok(Outcome::Done(result)) => result,
        Ok(Outcome::Superseded) => panic!("superseded"),
        Err(_) => panic!("no outcome within 5 s (the fake hung)"),
    }
}

fn made(kernel: &FakeKernel, primitive: Primitive) -> ShapeData {
    run(kernel, KernelRequest::make(primitive).unwrap()).unwrap()
}

fn block(kernel: &FakeKernel) -> ShapeData {
    made(
        kernel,
        Primitive::Box {
            dx: 1.0,
            dy: 2.0,
            dz: 3.0,
        },
    )
}

fn vertex(index: u32) -> ElementRef {
    ElementRef {
        kind: ElementKind::Vertex,
        index,
    }
}

fn ask(kernel: &FakeKernel, shape: &ShapeData) -> Result<Distance, silico_core::Error> {
    run(
        kernel,
        KernelRequest::distance(shape, ElementRef::face(1), None, ElementRef::face(2)).unwrap(),
    )
}

/// A fake kernel that answers every distance with `answer`, as a worker at fault would.
fn lying(answer: Reply) -> FakeKernel {
    let kernel = FakeKernel::new(1).unwrap();
    kernel.answer_instead(move |op| match op {
        Op::Distance { .. } => Some(answer.clone()),
        _ => None,
    });
    kernel
}

#[test]
fn a_reply_whose_witness_isnt_its_distance_apart_is_a_protocol_error() {
    // `KernelRequest::distance` refuses "a distance no pair has" (NaN, below 0, no pair),
    // and passes a distance whose own witness pair says otherwise: two points 5 apart sent
    // as 1 apart, and one point twice sent as 7 apart. No two elements have either; the
    // pair is said to be where the distance is met.
    //
    // BREAKS: `distance_fault` (kernel.rs) checks the distance and each point alone, never
    // the points' distance against the value, so both replies are returned as Ok and an
    // agent is told 1 with a witness 5 apart. A check that the witness's length is the
    // distance within OCCT's precision (BRepExtrema keeps solutions within 1e-7 of its
    // value; a numerical sphere pair's witness is 8.7e-10 off, and at a kilometre out an
    // ulp is 1.2e-10) would refuse them.
    for (what, lie) in [
        (
            "5 apart, sent as 1",
            Distance {
                distance: 1.0,
                from: [0.0, 0.0, 0.0],
                to: [5.0, 0.0, 0.0],
                solutions: 1,
            },
        ),
        (
            "one point, sent as 7 apart",
            Distance {
                distance: 7.0,
                from: [1.0, 2.0, 3.0],
                to: [1.0, 2.0, 3.0],
                solutions: 2,
            },
        ),
    ] {
        let kernel = lying(Reply::Distance(lie.clone()));
        let shape = block(&kernel);
        match ask(&kernel, &shape) {
            Ok(d) => panic!("{what}: taken as a distance: {d:?}"),
            Err(e) => assert_eq!(e.code(), ErrorCode::ProtocolError, "{what}: {e}"),
        }
    }
}

#[test]
fn a_reply_of_another_kind_or_with_a_point_at_infinity_is_a_protocol_error() {
    // A worker answering a distance with a shape, or with a witness whose first point is
    // at -inf or NaN: each the worker's fault, and the fake's next distance as it should be.
    let shape_reply = Reply::Shape(ShapeData::unchecked(Blob::inline(vec![1, 2, 3])));
    let kernel = lying(shape_reply);
    let shape = block(&kernel);
    assert_eq!(
        ask(&kernel, &shape).unwrap_err().code(),
        ErrorCode::ProtocolError
    );
    for (what, value) in [("-inf", f64::NEG_INFINITY), ("NaN", f64::NAN)] {
        let kernel = lying(Reply::Distance(Distance {
            distance: 1.0,
            from: [0.0, 0.0, value],
            to: [0.0, 0.0, 1.0],
            solutions: 1,
        }));
        let shape = block(&kernel);
        let e = ask(&kernel, &shape).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ProtocolError, "{what}");
        assert_eq!(
            e.cause(),
            Some(format!("its first point has {value}").as_str()),
            "{what}"
        );
        assert_eq!(e.suggestion(), Some("start a new kernel worker"), "{what}");
    }
}

#[test]
fn a_kernel_that_finds_no_distance_is_its_failure_with_its_cause_and_advice() {
    // OCCT not done, as the worker refuses it: kernel_failed, OCCT's words as the cause,
    // and the refusal's own advice (refusal.rs), not the generic "change the operation's
    // values".
    for cause in [
        "BRepExtrema_DistShapeShape: not done",
        "BRepExtrema_DistShapeShape: done, with no solution",
    ] {
        let kernel = FakeKernel::new(1).unwrap();
        let shape = block(&kernel);
        let words = cause.to_owned();
        kernel.refuse_as_worker(move |op| match op {
            Op::Distance { .. } => Some(Failure {
                refusal: Refusal::DistanceNotDone,
                cause: Some(words.clone()),
            }),
            _ => None,
        });
        let e = ask(&kernel, &shape).unwrap_err();
        assert_eq!(e.code(), ErrorCode::KernelFailed, "{cause}");
        assert_eq!(e.message(), "Distance failed", "{cause}");
        assert_eq!(e.cause(), Some(cause));
        assert_eq!(
            e.suggestion(),
            Some("report it with the document: the kernel couldn't measure this pair")
        );
        assert_eq!(e.parameter(), None, "{cause}");
    }
}

#[test]
fn the_fake_refuses_elements_at_the_ends_of_their_numbers_naming_which() {
    let kernel = FakeKernel::new(1).unwrap();
    let shape = block(&kernel);
    // u32::MAX of each kind; the box's last of each is measured (Face6 and Vertex8 are
    // the fake's points (6, 0, 0) and (8, 0, 0): 2 apart).
    for (first, second, parameter, message) in [
        (
            ElementRef::face(u32::MAX),
            ElementRef::face(1),
            "first",
            "first is Face4294967295, and its shape has 6 faces",
        ),
        (
            ElementRef::face(1),
            ElementRef::edge(u32::MAX),
            "second",
            "second is Edge4294967295, and its shape has 12 edges",
        ),
        (
            vertex(u32::MAX),
            vertex(u32::MAX),
            "first",
            "first is Vertex4294967295, and its shape has 8 vertices",
        ),
    ] {
        let e = run(
            &kernel,
            KernelRequest::distance(&shape, first, None, second).unwrap(),
        )
        .unwrap_err();
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{first} {second}");
        assert_eq!(e.parameter(), Some(parameter), "{first} {second}");
        assert_eq!(e.message(), message);
    }
    let d = run(
        &kernel,
        KernelRequest::distance(&shape, ElementRef::face(6), None, vertex(8)).unwrap(),
    )
    .unwrap();
    assert_eq!(d.distance, 2.0);
    // Of another shape, counted on it: a sphere has 1 face and 3 edges (its seam and
    // poles), as OCCT makes it.
    let ball = made(&kernel, Primitive::Sphere { radius: 1.0 });
    let e = run(
        &kernel,
        KernelRequest::distance(
            &shape,
            ElementRef::edge(12),
            Some(&ball),
            ElementRef::edge(4),
        )
        .unwrap(),
    )
    .unwrap_err();
    assert_eq!(e.parameter(), Some("second"));
    assert_eq!(e.message(), "second is Edge4, and its shape has 3 edges");
    let d = run(
        &kernel,
        KernelRequest::distance(
            &shape,
            ElementRef::edge(12),
            Some(&ball),
            ElementRef::edge(3),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(d.distance, 9.0);
}

#[test]
fn the_fake_given_bytes_it_never_made_refuses_them_and_goes_on() {
    // Bytes no fake shape has, as the shape and as the other shape: refused as input, and
    // the next distance as it should be.
    let kernel = FakeKernel::new(1).unwrap();
    let good = block(&kernel);
    let bad = ShapeData::unchecked(Blob::inline(b"not a fake shape".to_vec()));
    for (shape, other) in [(&bad, &good), (&good, &bad)] {
        let e = run(
            &kernel,
            KernelRequest::distance(shape, ElementRef::face(1), Some(other), ElementRef::face(1))
                .unwrap(),
        )
        .unwrap_err();
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{e}");
    }
    assert_eq!(ask(&kernel, &good).unwrap().distance, 1.0);
}
