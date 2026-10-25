//! `tessellate_at_least_the_floor`: M4.6's renders mesh at half a pixel, which a close view
//! takes under the shape's floor; meshed at the floor instead, never refused for it, and
//! the deflection used said (docs/design/m4.6-render.md, The meshes).

use silico_kernel::{
    LENGTH_TOLERANCE, MIN_RELATIVE_DEFLECTION, make_box, tessellate, tessellate_at_least_the_floor,
};
use silico_protocol::Refusal;

/// The floor `tessellate` refuses `deflection` for, if it does.
fn floor_refusing(shape: &silico_kernel::Shape, deflection: f64) -> Option<f64> {
    match tessellate(shape, deflection) {
        Err(e) => match e.refusal() {
            Refusal::DeflectionTooFine { floor, .. } => Some(*floor),
            other => panic!("{other:?}"),
        },
        Ok(_) => None,
    }
}

#[test]
fn under_the_floor_a_shape_is_meshed_at_its_floor_and_says_so() {
    // The design's example: a 4,096-pixel view of a 100 mm cube asks for 2⁻⁷ mm, under its
    // floor, 1e-4 of its (enlarged) box's diagonal, about 100√3 = 173.2: about 0.0173 mm.
    let cube = make_box(100.0, 100.0, 100.0).unwrap();
    let asked = 1.0 / 128.0;
    let floor = floor_refusing(&cube, asked).expect("refused under the floor");
    assert!(
        floor > 100.0 * 3.0f64.sqrt() * MIN_RELATIVE_DEFLECTION && floor < 0.0174,
        "{floor}"
    );
    let (mesh, used) = tessellate_at_least_the_floor(&cube, asked).unwrap();
    assert_eq!(used.to_bits(), floor.to_bits());
    // The same mesh as asked at the floor itself.
    assert_eq!(mesh, tessellate(&cube, floor).unwrap());
    // Far under it too, at the least a deflection may be.
    let (_, used) = tessellate_at_least_the_floor(&cube, f64::MIN_POSITIVE).unwrap();
    assert_eq!(used.to_bits(), floor.to_bits());
}

#[test]
fn at_or_over_the_floor_the_deflection_asked_is_used() {
    let cube = make_box(100.0, 100.0, 100.0).unwrap();
    let floor = floor_refusing(&cube, 1e-9).unwrap();
    for asked in [floor, 0.05, 1.0] {
        let (mesh, used) = tessellate_at_least_the_floor(&cube, asked).unwrap();
        assert_eq!(used.to_bits(), asked.to_bits(), "{asked}");
        assert_eq!(mesh, tessellate(&cube, asked).unwrap(), "{asked}");
    }
}

#[test]
fn a_tiny_shapes_floor_is_the_length_tolerance() {
    // A box 1e-4 mm on a side: 1e-4 of its diagonal is under LENGTH_TOLERANCE, the floor's
    // least.
    let speck = make_box(1e-4, 1e-4, 1e-4).unwrap();
    let (_, used) = tessellate_at_least_the_floor(&speck, 1e-12).unwrap();
    assert_eq!(used.to_bits(), LENGTH_TOLERANCE.to_bits());
}

#[test]
fn a_deflection_that_isnt_one_is_still_refused() {
    let cube = make_box(1.0, 1.0, 1.0).unwrap();
    for deflection in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let e = tessellate_at_least_the_floor(&cube, deflection).unwrap_err();
        assert!(
            matches!(e.refusal(), Refusal::DeflectionInvalid { .. }),
            "{deflection}: {e:?}"
        );
    }
}

#[test]
fn through_the_worker_the_core_gets_the_mesh_and_the_deflection_used() {
    // The core's typed request, on a worker: the floor's mesh, its deflection, and a
    // classification of the same cube (both M4.6's kernel requests).
    use silico_core::kernel::{KernelRequest, KernelWorker};
    let mut worker = KernelWorker::start_worker(std::path::Path::new(env!(
        "CARGO_BIN_EXE_silico-kernel-worker"
    )))
    .unwrap();
    let cube = worker
        .make(silico_kernel::Primitive::Box {
            dx: 100.0,
            dy: 100.0,
            dz: 100.0,
        })
        .unwrap();
    let local = make_box(100.0, 100.0, 100.0).unwrap();
    let floor = floor_refusing(&local, 1.0 / 128.0).unwrap();
    let (mesh, used) = worker
        .run(KernelRequest::tessellate_at_least_the_floor(
            &cube,
            1.0 / 128.0,
        ))
        .unwrap();
    assert_eq!(used.to_bits(), floor.to_bits());
    assert_eq!(mesh, tessellate(&local, floor).unwrap());
    assert!(
        worker
            .run(KernelRequest::classify(&cube, [50.0, 50.0, 50.0]))
            .unwrap()
    );
    assert!(
        !worker
            .run(KernelRequest::classify(&cube, [150.0, 50.0, 50.0]))
            .unwrap()
    );
}
