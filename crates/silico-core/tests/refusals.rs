//! M2.0: a request whose values can't make the operation is refused before it's sent,
//! naming the parameter at fault as data: the operation's field, as the protocol spells it
//! (`Op` and its parts), and the part of it at fault where it has parts (`offset.x`,
//! `size.angle`), so an agent knows which value to change.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::kernel::{Axis, Blob, ChamferSize, KernelRequest, Primitive, Profile, ShapeData};
use silico_core::{Error, ErrorCode};

/// The parameter a refusal names, the suggestion it gives, and its message.
type Refusal = (String, String, String);

/// A case: the parameter, suggestion, and message expected, and the refusal.
type Case = (&'static str, &'static str, &'static str, Refusal);

fn refusal(refused: Result<impl Sized, Error>) -> Refusal {
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
    (
        parameter.to_owned(),
        suggestion.to_owned(),
        err.message().to_owned(),
    )
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
    let cases: [Case; 16] = [
        (
            "dx",
            "give the box a valid dx",
            "box dx must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got 0",
            refusal(KernelRequest::make(Primitive::Box {
                dx: 0.0,
                dy: 1.0,
                dz: 1.0,
            })),
        ),
        (
            "dz",
            "give the box a valid dz",
            "box dz must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got NaN",
            refusal(KernelRequest::make(Primitive::Box {
                dx: 1.0,
                dy: 1.0,
                dz: f64::NAN,
            })),
        ),
        (
            "radius",
            "give the sphere a valid radius",
            "sphere radius must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got -1",
            refusal(KernelRequest::make(Primitive::Sphere { radius: -1.0 })),
        ),
        (
            "radius1",
            "give the cone a valid radius1",
            "cone radius1 must be zero, or a length above 1e-7 mm and at most 1e6 mm (a kilometre), got -1",
            refusal(KernelRequest::make(Primitive::Cone {
                radius1: -1.0,
                radius2: 1.0,
                height: 1.0,
            })),
        ),
        (
            "minor_radius",
            "give the torus a valid minor_radius",
            "torus minor_radius must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got 0",
            refusal(KernelRequest::make(Primitive::Torus {
                major_radius: 1.0,
                minor_radius: 0.0,
            })),
        ),
        (
            "offset.x",
            OFFSET,
            "the offset's x must be a finite length of at most 1000000 either way, got NaN",
            refusal(KernelRequest::translate(&shape, [f64::NAN, 0.0, 0.0])),
        ),
        (
            "offset.z",
            OFFSET,
            "the offset's z must be a finite length of at most 1000000 either way, got 1e9",
            refusal(KernelRequest::translate(&shape, [0.0, 0.0, 1e9])),
        ),
        (
            "radius",
            "give the fillet a valid radius",
            "the fillet's radius must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got 0",
            refusal(KernelRequest::fillet(&shape, &[1], 0.0)),
        ),
        (
            "size.distance",
            "give the chamfer a valid distance",
            "the chamfer's distance must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got 0",
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
            "the chamfer's other distance must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got -1",
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
            "the chamfer's angle must be more than 0 and less than 180 degrees, got 0",
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
            "a profile needs 3 to 1000 corners, got 2",
            refusal(KernelRequest::extrude(&line, 1.0, None)),
        ),
        (
            "length",
            "give the extrusion a valid length",
            "the extrusion's length must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got NaN",
            refusal(KernelRequest::extrude(&square, f64::NAN, None)),
        ),
        (
            "direction",
            "give the extrusion a valid direction",
            "the extrusion's direction lies in the profile's plane: it must point out of it",
            refusal(KernelRequest::extrude(&square, 1.0, Some([1.0, 0.0, 0.0]))),
        ),
        (
            "angle",
            "give the revolve a valid angle",
            "the revolve's angle must be more than 0 and at most 360 degrees, got 0",
            refusal(KernelRequest::revolve(&square, beside, 0.0)),
        ),
        (
            "axis",
            "give the revolve a valid axis",
            "the axis's direction has no direction: its length is 0",
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
    for (parameter, suggestion, message, got) in cases {
        assert_eq!(
            got,
            (
                parameter.to_owned(),
                suggestion.to_owned(),
                message.to_owned()
            )
        );
    }
}
