//! break-it: silico_core::sweep's refusals (design S5, S7) given volumes no solid has, and
//! the fake kernel's sweeps at the edges of the protocol's rules.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::ErrorCode;
use silico_core::document::FeatureId;
use silico_core::document::SweepFeature;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    Axis, Curve, FaceRegions, Kernel, KernelRequest, Primitive, Region, SketchPlane,
};
use silico_core::pool::Outcome;
use silico_core::sweep::{Measured, check_result};
use silico_protocol::{MAX_LENGTH, through_all_length};

fn fid(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn solid(volume: f64) -> Measured {
    Measured {
        volume,
        solids: 1,
        faces: 6,
        edges: 12,
        vertices: 8,
    }
}

fn check(kind: SweepFeature, before: f64, after: f64) -> Result<(), silico_core::Error> {
    check_result(
        &fid("p0000001"),
        kind,
        &fid("b0000001"),
        &solid(before),
        &solid(after),
    )
}

#[test]
fn a_volume_that_isnt_a_number_is_no_proof_of_a_change() {
    // Found by break-it, fixed: `change <= MEASURE_PRECISION * before` is false when either
    // volume is NaN, so check_result passed a feature whose result (or base) measured NaN,
    // the precise measure's answer for a broken shape. It now refuses a volume no solid has
    // first, kernel_failed, as the kernel's amount_of (which refuses a sweep with no volume to
    // speak of, or turned inside out) treats NaN as no volume.
    for kind in SweepFeature::ALL {
        for (before, after) in [(1000.0, f64::NAN), (f64::NAN, 1000.0), (f64::NAN, f64::NAN)] {
            let e = check(kind, before, after).unwrap_err();
            assert_eq!(
                e.code(),
                ErrorCode::KernelFailed,
                "{kind:?} {before} {after}"
            );
            assert_eq!(e.feature(), Some("p0000001"));
        }
    }
}

#[test]
fn an_infinite_or_negative_volume_is_no_proof_of_a_change() {
    // Found by break-it, fixed: an infinite result passed a pad (inf − 1000 > 1e-6), and an
    // inverted base (volume −1000, as a mirrored B-rep measures) made the threshold
    // negative, so a pad that added nothing to it passed too. Both are now refused as
    // volumes no solid has.
    for kind in [SweepFeature::Pad, SweepFeature::Revolve] {
        for (before, after) in [(1000.0, f64::INFINITY), (-1000.0, -1000.0)] {
            assert!(
                check(kind, before, after).is_err(),
                "{kind:?}: {before} to {after} passed as a change"
            );
        }
    }
    for kind in [SweepFeature::Pocket, SweepFeature::Groove] {
        for (before, after) in [(f64::INFINITY, 1000.0), (-1000.0, -1000.0)] {
            assert!(
                check(kind, before, after).is_err(),
                "{kind:?}: {before} to {after} passed as a change"
            );
        }
    }
}

#[test]
fn the_largest_counts_and_volumes_are_worded_without_overflow() {
    let many = Measured {
        solids: u32::MAX,
        ..solid(1e18)
    };
    for kind in SweepFeature::ALL {
        let e = check_result(
            &fid("p0000001"),
            kind,
            &fid("b0000001"),
            &solid(1e18),
            &many,
        )
        .unwrap_err();
        assert!(e.message().contains("4294967295 solids"), "{}", e.message());
        assert!(e.suggestion().is_some());
    }
    // A kilometre cube (1e18 mm³) that a 1 mm³ pad changes by 1e-18 of it: refused as
    // nothing, by the design's rule (S7, S11), and the message says the volume.
    let e = check(SweepFeature::Pad, 1e18, 1e18 + 1e9).unwrap_err();
    assert!(e.message().contains("adds nothing"), "{}", e.message());
}

// --- The fake kernel at the rules' edges ---------------------------------------------------

fn done<T: std::fmt::Debug>(outcome: Outcome<T>) -> Result<T, silico_core::Error> {
    match outcome {
        Outcome::Done(result) => result,
        Outcome::Superseded => panic!("superseded"),
    }
}

fn run<T: Send + std::fmt::Debug + 'static>(
    kernel: &FakeKernel,
    request: KernelRequest<T>,
) -> Result<T, silico_core::Error> {
    done(kernel.submit(kernel.generation(), request).wait())
}

fn unit_square() -> FaceRegions {
    let p = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    FaceRegions {
        plane: SketchPlane::Xy,
        regions: vec![Region {
            outer: (0..4)
                .map(|i| Curve::Line {
                    start: p[i],
                    end: p[(i + 1) % 4],
                })
                .collect(),
            holes: vec![],
        }],
    }
}

#[test]
fn a_fake_through_all_at_the_kilometre_is_refused_or_readable() {
    // Found by break-it, fixed: a base d × 1 × 1 whose through-all length is a few ulps
    // under a kilometre, along a direction 2ε long (which check_through_all takes): the fake
    // swept direction × length without checking it and wrote bytes it then refused as not
    // its own; the real kernel refused the request naming `vector`. check_prism now admits
    // the product, and the fake checks it as the kernel does, naming `base` if it fails.
    let direction = [1.0 + 2.0 * f64::EPSILON, 0.0, 0.0];
    let mut d = MAX_LENGTH / 2.02;
    let length = loop {
        match through_all_length([[0.0; 3], [d, 1.0, 1.0]]) {
            Ok(length) if length * direction[0] > MAX_LENGTH => break length,
            Ok(_) => d = d.next_up(),
            Err(_) => d = d.next_down(),
        }
    };
    assert!(length <= MAX_LENGTH);
    let fake = FakeKernel::new(1).unwrap();
    let base = run(
        &fake,
        KernelRequest::make(Primitive::Box {
            dx: d,
            dy: 1.0,
            dz: 1.0,
        })
        .unwrap(),
    )
    .unwrap();
    let faces = run(&fake, KernelRequest::make_faces(&unit_square()).unwrap()).unwrap();
    let swept = run(
        &fake,
        KernelRequest::prism_through_all(&faces.shape, &base, direction).unwrap(),
    );
    if let Ok(swept) = swept {
        let summary = run(&fake, KernelRequest::summarize(&swept.shape));
        assert!(
            summary.is_ok(),
            "the fake made a through-all prism it can't read back: {summary:?}"
        );
    }
}

#[test]
fn a_fake_sweep_of_a_whole_turn_and_any_start_reads_back() {
    // Every value the rules admit, at their edges, written by the fake and read back.
    let fake = FakeKernel::new(1).unwrap();
    let faces = run(&fake, KernelRequest::make_faces(&unit_square()).unwrap()).unwrap();
    let axis = Axis {
        origin: [-1e6, 0.0, 0.0],
        direction: [0.0, 1.0, 0.0],
    };
    for (start, angle) in [
        (-std::f64::consts::TAU, std::f64::consts::TAU),
        (-0.0, 1.0000000000000002e-12),
        (5e-324, std::f64::consts::TAU - 1e-12),
        (std::f64::consts::TAU, 1.0),
    ] {
        let swept = run(
            &fake,
            KernelRequest::revolution(&faces.shape, axis, start, angle).unwrap(),
        )
        .unwrap();
        let summary = run(&fake, KernelRequest::summarize(&swept.shape)).unwrap();
        assert!(summary.volume > 0.0, "{start} {angle}: {summary:?}");
    }
    for (start, vector) in [
        ([-1e6, 1e6, -0.0], [0.0, 0.0, 1e6]),
        ([0.0; 3], [0.0, 0.0, 1.0000000000000002e-7]),
        ([5e-324, 0.0, 0.0], [0.0, 0.0, -1.0]),
    ] {
        let swept = run(
            &fake,
            KernelRequest::prism(&faces.shape, start, vector).unwrap(),
        )
        .unwrap();
        let summary = run(&fake, KernelRequest::summarize(&swept.shape)).unwrap();
        assert!(summary.volume > 0.0, "{start:?} {vector:?}: {summary:?}");
    }
}

#[test]
fn a_fake_sweep_of_what_isnt_faces_is_refused_naming_the_faces_and_the_fake_goes_on() {
    let fake = FakeKernel::new(1).unwrap();
    let cube = run(
        &fake,
        KernelRequest::make(Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        })
        .unwrap(),
    )
    .unwrap();
    let e = run(
        &fake,
        KernelRequest::prism(&cube, [0.0; 3], [0.0, 0.0, 1.0]).unwrap(),
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
    assert_eq!(e.parameter(), Some("faces"), "{e:?}");
    // A sweep of a sweep: not faces either.
    let faces = run(&fake, KernelRequest::make_faces(&unit_square()).unwrap()).unwrap();
    let once = run(
        &fake,
        KernelRequest::prism(&faces.shape, [0.0; 3], [0.0, 0.0, 1.0]).unwrap(),
    )
    .unwrap();
    let e = run(
        &fake,
        KernelRequest::prism(&once.shape, [0.0; 3], [0.0, 0.0, 1.0]).unwrap(),
    )
    .unwrap_err();
    assert_eq!(e.parameter(), Some("faces"), "{e:?}");
    // Moved faces aren't the kernel's faces to the fake, but they are to the kernel: the
    // fake refuses them the same way (a documented difference, not a crash).
    let moved = run(
        &fake,
        KernelRequest::translate(&faces.shape, [1.0, 0.0, 0.0]).unwrap(),
    );
    if let Ok(moved) = moved {
        let _ = run(
            &fake,
            KernelRequest::prism(&moved.shape, [0.0; 3], [0.0, 0.0, 1.0]).unwrap(),
        );
    }
    assert!(
        run(
            &fake,
            KernelRequest::prism(&faces.shape, [0.0; 3], [0.0, 0.0, 2.0]).unwrap()
        )
        .is_ok()
    );
}
