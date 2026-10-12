//! M2.0: a request whose values can't make the operation is refused before it's sent,
//! naming the parameter at fault as data: a path through the operation's fields, as the
//! protocol spells them (`Op` and its parts), so an agent knows which value to change.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::kernel::{Axis, Blob, ChamferSize, KernelRequest, Primitive, Profile, ShapeData};
use silico_core::{Error, ErrorCode};

fn parameter(refused: Result<impl Sized, Error>) -> String {
    let Err(err) = refused else {
        panic!("not refused")
    };
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(err.suggestion().is_some(), "{err}");
    err.parameter()
        .unwrap_or_else(|| panic!("no parameter: {err}"))
        .to_owned()
}

#[test]
fn every_value_refused_before_sending_names_its_parameter() {
    let shape = ShapeData::unchecked(Blob::inline(Vec::new()));
    let square = Profile {
        points: vec![[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
    };
    let line = Profile {
        points: vec![[0.0; 3], [1.0, 0.0, 0.0]],
    };
    let beside = Axis {
        origin: [-1.0, 0.0, 0.0],
        direction: [0.0, 1.0, 0.0],
    };
    let cases: [(&str, String); 16] = [
        (
            "dx",
            parameter(KernelRequest::make(Primitive::Box {
                dx: 0.0,
                dy: 1.0,
                dz: 1.0,
            })),
        ),
        (
            "dz",
            parameter(KernelRequest::make(Primitive::Box {
                dx: 1.0,
                dy: 1.0,
                dz: f64::NAN,
            })),
        ),
        (
            "radius",
            parameter(KernelRequest::make(Primitive::Sphere { radius: -1.0 })),
        ),
        (
            "radius1",
            parameter(KernelRequest::make(Primitive::Cone {
                radius1: -1.0,
                radius2: 1.0,
                height: 1.0,
            })),
        ),
        (
            "minor_radius",
            parameter(KernelRequest::make(Primitive::Torus {
                major_radius: 1.0,
                minor_radius: 0.0,
            })),
        ),
        (
            "offset.x",
            parameter(KernelRequest::translate(&shape, [f64::NAN, 0.0, 0.0])),
        ),
        (
            "offset.z",
            parameter(KernelRequest::translate(&shape, [0.0, 0.0, 1e9])),
        ),
        (
            "radius",
            parameter(KernelRequest::fillet(&shape, &[1], 0.0)),
        ),
        (
            "size.distance",
            parameter(KernelRequest::chamfer(
                &shape,
                &[1],
                ChamferSize::Equal { distance: 0.0 },
                false,
            )),
        ),
        (
            "size.other",
            parameter(KernelRequest::chamfer(
                &shape,
                &[1],
                ChamferSize::TwoDistances {
                    distance: 1.0,
                    other: -1.0,
                },
                false,
            )),
        ),
        (
            "size.angle",
            parameter(KernelRequest::chamfer(
                &shape,
                &[1],
                ChamferSize::DistanceAngle {
                    distance: 1.0,
                    angle: 0.0,
                },
                false,
            )),
        ),
        (
            "profile",
            parameter(KernelRequest::extrude(&line, 1.0, None)),
        ),
        (
            "length",
            parameter(KernelRequest::extrude(&square, f64::NAN, None)),
        ),
        (
            "direction",
            parameter(KernelRequest::extrude(&square, 1.0, Some([1.0, 0.0, 0.0]))),
        ),
        (
            "angle",
            parameter(KernelRequest::revolve(&square, beside, 0.0)),
        ),
        (
            "axis",
            parameter(KernelRequest::revolve(
                &square,
                Axis {
                    origin: [-1.0, 0.0, 0.0],
                    direction: [0.0; 3],
                },
                90.0,
            )),
        ),
    ];
    for (want, got) in cases {
        assert_eq!(got, want);
    }
}
