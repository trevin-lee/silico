//! M4.5: the core's distance request (`KernelRequest::distance`, docs/ports/measure.md):
//! what it refuses before asking, the fake kernel's answers (each element at its number
//! along x, as its descriptions place it), and a worker's reply checked before it's
//! returned. The distances of real solids are measured on a worker (silico-kernel's
//! tests/distance.rs).

use super::model::WAIT;

use silico_core::ErrorCode;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    Distance, ElementKind, ElementRef, Kernel, KernelRequest, Op, Primitive, ShapeData,
};
use silico_core::pool::{Outcome, Pending};
use silico_protocol::Reply;

fn within<T: std::fmt::Debug>(pending: Pending<T>) -> Outcome<T> {
    match pending.wait_timeout(WAIT) {
        Ok(outcome) => outcome,
        Err(_) => panic!("no outcome within {WAIT:?} (the fake hung)"),
    }
}

fn run<T: Send + std::fmt::Debug + 'static>(
    kernel: &FakeKernel,
    request: KernelRequest<T>,
) -> Result<T, silico_core::Error> {
    match within(kernel.submit(kernel.generation(), request)) {
        Outcome::Done(result) => result,
        Outcome::Superseded => panic!("superseded"),
    }
}

fn block(kernel: &FakeKernel) -> ShapeData {
    run(
        kernel,
        KernelRequest::make(Primitive::Box {
            dx: 1.0,
            dy: 2.0,
            dz: 3.0,
        })
        .unwrap(),
    )
    .unwrap()
}

fn vertex(index: u32) -> ElementRef {
    ElementRef {
        kind: ElementKind::Vertex,
        index,
    }
}

#[test]
fn a_solid_or_an_element_numbered_0_is_refused_before_the_kernel_is_asked() {
    let kernel = FakeKernel::new(1).unwrap();
    let shape = block(&kernel);
    let solid = ElementRef {
        kind: ElementKind::Solid,
        index: 1,
    };
    let Err(refused) = KernelRequest::distance(&shape, ElementRef::face(1), None, solid) else {
        panic!("a solid was asked for");
    };
    assert_eq!(refused.code(), ErrorCode::InvalidInput);
    assert_eq!(refused.parameter(), Some("second"));
    assert_eq!(
        refused.message(),
        "second must be a face, an edge, or a vertex, not a solid"
    );
    let Err(zero) = KernelRequest::distance(&shape, vertex(0), None, vertex(1)) else {
        panic!("an element numbered 0 was asked for");
    };
    assert_eq!(zero.parameter(), Some("first"));
    assert_eq!(
        zero.message(),
        "elements are numbered from 1, so first can't be 0"
    );
    // Worded as a worker's refusal of a value is: what to do, and no cause.
    for error in [&refused, &zero] {
        assert_eq!(
            error.suggestion(),
            Some("change the value the message names")
        );
        assert_eq!(error.cause(), None);
    }
}

#[test]
fn the_fakes_distance_is_how_far_apart_the_elements_numbers_are() {
    let kernel = FakeKernel::new(1).unwrap();
    let shape = block(&kernel);
    let d = run(
        &kernel,
        KernelRequest::distance(&shape, ElementRef::face(2), None, ElementRef::face(6)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        d,
        Distance {
            distance: 4.0,
            from: [2.0, 0.0, 0.0],
            to: [6.0, 0.0, 0.0],
            solutions: 1,
        }
    );
    // Of another shape, counted on it: a box has 8 vertices, so a ninth is refused.
    let other = block(&kernel);
    let refused = run(
        &kernel,
        KernelRequest::distance(&shape, vertex(1), Some(&other), vertex(9)).unwrap(),
    )
    .unwrap_err();
    assert_eq!(refused.parameter(), Some("second"));
    assert_eq!(
        refused.message(),
        "second, vertex 9, doesn't exist: its shape has vertices 1 to 8"
    );
}

#[test]
fn a_reply_no_pair_of_elements_could_have_is_a_protocol_error() {
    for (what, bad) in [
        (
            "its distance has NaN",
            Distance {
                distance: f64::NAN,
                from: [0.0; 3],
                to: [0.0; 3],
                solutions: 1,
            },
        ),
        (
            "its second point has inf",
            Distance {
                distance: 1.0,
                from: [0.0; 3],
                to: [0.0, f64::INFINITY, 0.0],
                solutions: 1,
            },
        ),
        (
            "its distance is -1, below 0",
            Distance {
                distance: -1.0,
                from: [0.0; 3],
                to: [0.0; 3],
                solutions: 1,
            },
        ),
        (
            "it found no pair of points",
            Distance {
                distance: 1.0,
                from: [0.0; 3],
                to: [1.0, 0.0, 0.0],
                solutions: 0,
            },
        ),
    ] {
        let kernel = FakeKernel::new(1).unwrap();
        let shape = block(&kernel);
        let answer = bad.clone();
        kernel.answer_instead(move |op| match op {
            Op::Distance { .. } => Some(Reply::Distance(answer.clone())),
            _ => None,
        });
        let error = run(
            &kernel,
            KernelRequest::distance(&shape, ElementRef::face(1), None, ElementRef::face(2))
                .unwrap(),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ProtocolError, "{what}");
        assert_eq!(error.cause(), Some(what), "{what}");
    }
    // A distance of 0, with its points one point, is a distance two elements can have.
    let kernel = FakeKernel::new(1).unwrap();
    let shape = block(&kernel);
    kernel.answer_instead(|op| match op {
        Op::Distance { .. } => Some(Reply::Distance(Distance {
            distance: 0.0,
            from: [-0.0, 0.0, 0.0],
            to: [0.0, 0.0, 0.0],
            solutions: 3,
        })),
        _ => None,
    });
    let d = run(
        &kernel,
        KernelRequest::distance(&shape, ElementRef::face(1), None, ElementRef::face(3)).unwrap(),
    )
    .unwrap();
    assert_eq!(d.solutions, 3);
}
