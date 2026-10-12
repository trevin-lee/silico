//! M2.0: a request whose values can't make the operation is refused before it's sent,
//! naming the parameter at fault as data: the operation's field, as the protocol spells it
//! (`Op` and its parts), and the part of it at fault where it has parts (`offset.x`,
//! `size.angle`), so an agent knows which value to change.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::kernel::{Axis, Blob, ChamferSize, KernelRequest, Primitive, Profile, ShapeData};
use silico_core::{Error, ErrorCode};

/// The parameter a refusal names, and the suggestion it gives.
fn refusal(refused: Result<impl Sized, Error>) -> (String, String) {
    let Err(err) = refused else {
        panic!("not refused")
    };
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    let parameter = err
        .parameter()
        .unwrap_or_else(|| panic!("no parameter: {err}"));
    let suggestion = err
        .suggestion()
        .unwrap_or_else(|| panic!("no suggestion: {err}"));
    (parameter.to_owned(), suggestion.to_owned())
}

/// What a refused translation suggests.
const OFFSET: &str = "move the shape by a finite offset, no more than a kilometre each way";

#[test]
fn every_value_the_request_constructors_check_names_its_parameter_and_says_what_to_give() {
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
    let cases: [(&str, &str, (String, String)); 16] = [
        (
            "dx",
            "give the box a valid dx",
            refusal(KernelRequest::make(Primitive::Box {
                dx: 0.0,
                dy: 1.0,
                dz: 1.0,
            })),
        ),
        (
            "dz",
            "give the box a valid dz",
            refusal(KernelRequest::make(Primitive::Box {
                dx: 1.0,
                dy: 1.0,
                dz: f64::NAN,
            })),
        ),
        (
            "radius",
            "give the sphere a valid radius",
            refusal(KernelRequest::make(Primitive::Sphere { radius: -1.0 })),
        ),
        (
            "radius1",
            "give the cone a valid radius1",
            refusal(KernelRequest::make(Primitive::Cone {
                radius1: -1.0,
                radius2: 1.0,
                height: 1.0,
            })),
        ),
        (
            "minor_radius",
            "give the torus a valid minor_radius",
            refusal(KernelRequest::make(Primitive::Torus {
                major_radius: 1.0,
                minor_radius: 0.0,
            })),
        ),
        (
            "offset.x",
            OFFSET,
            refusal(KernelRequest::translate(&shape, [f64::NAN, 0.0, 0.0])),
        ),
        (
            "offset.z",
            OFFSET,
            refusal(KernelRequest::translate(&shape, [0.0, 0.0, 1e9])),
        ),
        (
            "radius",
            "give the fillet a valid radius",
            refusal(KernelRequest::fillet(&shape, &[1], 0.0)),
        ),
        (
            "size.distance",
            "give the chamfer a valid distance",
            refusal(KernelRequest::chamfer(
                &shape,
                &[1],
                ChamferSize::Equal { distance: 0.0 },
                false,
            )),
        ),
        (
            "size.other",
            "give the chamfer a valid other distance",
            refusal(KernelRequest::chamfer(
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
            "give the chamfer a valid angle",
            refusal(KernelRequest::chamfer(
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
            "give the extrusion a valid profile",
            refusal(KernelRequest::extrude(&line, 1.0, None)),
        ),
        (
            "length",
            "give the extrusion a valid length",
            refusal(KernelRequest::extrude(&square, f64::NAN, None)),
        ),
        (
            "direction",
            "give the extrusion a valid direction",
            refusal(KernelRequest::extrude(&square, 1.0, Some([1.0, 0.0, 0.0]))),
        ),
        (
            "angle",
            "give the revolve a valid angle",
            refusal(KernelRequest::revolve(&square, beside, 0.0)),
        ),
        (
            "axis",
            "give the revolve a valid axis",
            refusal(KernelRequest::revolve(
                &square,
                Axis {
                    origin: [-1.0, 0.0, 0.0],
                    direction: [0.0; 3],
                },
                90.0,
            )),
        ),
    ];
    for (parameter, suggestion, got) in cases {
        assert_eq!(got, (parameter.to_owned(), suggestion.to_owned()));
    }
}
