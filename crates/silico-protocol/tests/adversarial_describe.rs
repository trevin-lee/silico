//! break-it: descriptions a broken worker could send (docs/design/m4.4-describe.md): what
//! the worker's refusal and the core's check name for each value that can't be.

use silico_protocol::{Axis, Description, FaceDescription, Surface};

fn cone(radius: f64, half_angle: f64) -> Description {
    Description {
        faces: vec![FaceDescription {
            surface: Surface::Cone {
                axis: Axis {
                    origin: [0.0; 3],
                    direction: [0.0, 0.0, 1.0],
                },
                radius,
                half_angle,
            },
            area: 1.0,
            centroid: [0.0; 3],
            orientation: silico_protocol::FaceOrientation::Forward,
        }],
        edges: Vec::new(),
        vertices: Vec::new(),
    }
}

#[test]
fn a_cones_half_angle_not_finite_is_named_as_its_half_angle() {
    // The worker refuses a description with a value that isn't finite, naming it
    // (`Refusal::NotFinite { quantity }`); the core's check names a cone's half-angle as
    // "the kernel's Face1's half-angle". A NaN half-angle is the half-angle, not the radius.
    //
    // (It filed a cone's half-angle under "radius", so the worker's refusal said
    // "Face1's radius" was NaN while the radius, 4, was fine: fixed by one walk of a
    // description's values for both.)
    let d = cone(4.0, f64::NAN);
    assert!(
        d.validate(1)
            .unwrap_err()
            .problem
            .starts_with("the kernel's Face1's half-angle")
    );
    let (what, value) = d.non_finite(1).unwrap();
    assert!(value.is_nan());
    assert_eq!(what.to_string(), "the kernel's Face1's half-angle");
}

#[test]
fn a_cones_half_angle_at_a_right_angle_or_its_radius_below_zero_is_refused() {
    // A half-angle of ±π/2 is a plane, not a cone; a radius below 0 isn't one; a radius of
    // 0 (its position at its apex) and any half-angle strictly between are.
    use std::f64::consts::FRAC_PI_2;
    for (radius, half_angle, ok) in [
        (4.0, FRAC_PI_2, false),
        (4.0, -FRAC_PI_2, false),
        (-1e-300, 0.5, false),
        (0.0, 0.5, true),
        (4.0, -0.5, true),
        (4.0, f64::INFINITY, false),
    ] {
        assert_eq!(
            cone(radius, half_angle).validate(1).is_ok(),
            ok,
            "radius {radius}, half-angle {half_angle}"
        );
    }
}
