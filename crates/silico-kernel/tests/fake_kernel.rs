//! M2.3: the in-memory kernel (silico_core::fake) answers as the real one does where it
//! claims to. The same requests go through the `Kernel` trait to the worker pool and to the
//! fake: each primitive has the same faces, edges, and vertices, and a move reports the same
//! history, element for element. (A fake boolean keeps both inputs whole, which the real
//! one doesn't claim; that's the fake's own model, tested in silico-core.)
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::time::Duration;

use silico_core::fake::FakeKernel;
use silico_core::kernel::{BuiltData, Kernel, KernelRequest, Primitive, ShapeData, ShapeSummary};
use silico_core::pool::{KernelPool, Pending};

fn pool() -> KernelPool {
    KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2).unwrap()
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
