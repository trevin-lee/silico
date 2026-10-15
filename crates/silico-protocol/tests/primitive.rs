//! The rule for primitive values at its exact edges, and every fault it finds. Expected
//! values come from the rule as stated: a length above 1e-7 mm (OCCT's tolerance) and at
//! most 1e6 mm, a cone radius zero or such a length, and a torus's tube no more than 1e-7
//! mm wider than its ring.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_protocol::{LENGTH_TOLERANCE, MAX_LENGTH, Primitive};

fn parameters(primitive: Primitive) -> Vec<&'static str> {
    primitive
        .faults(|p| p)
        .iter()
        .map(|f| f.parameter)
        .collect()
}

#[test]
fn a_length_at_the_tolerance_is_refused_and_just_above_it_is_not() {
    let above = 1.0000001e-7;
    assert!(above > LENGTH_TOLERANCE);
    for (radius, refused) in [(LENGTH_TOLERANCE, true), (above, false)] {
        let sphere = Primitive::Sphere { radius };
        assert_eq!(sphere.check().is_err(), refused, "{radius}");
        let cone = Primitive::Cone {
            radius1: radius,
            radius2: 1.0,
            height: 1.0,
        };
        assert_eq!(cone.check().is_err(), refused, "{radius}");
        assert_eq!(parameters(cone).is_empty(), !refused);
    }
    let longest = Primitive::Box {
        dx: MAX_LENGTH,
        dy: 1.0,
        dz: 1.0,
    };
    assert!(longest.check().is_ok());
}

#[test]
fn a_tube_wider_than_its_ring_by_the_tolerance_is_accepted_and_by_more_is_not() {
    // A ring of 2e-7: a tube 1e-7 wider is wider by exactly the tolerance, in floating
    // point too (both near 2e-7, so the subtraction is exact), so `>` and `>=` differ here.
    let major_radius = 2e-7;
    let at = major_radius + LENGTH_TOLERANCE;
    assert_eq!(at - major_radius, LENGTH_TOLERANCE);
    let past = major_radius + 2.0 * LENGTH_TOLERANCE;
    assert!(past - major_radius > LENGTH_TOLERANCE);
    let torus = |minor_radius| Primitive::Torus {
        major_radius,
        minor_radius,
    };
    assert!(torus(at).check().is_ok());
    let invalid = torus(past).check().unwrap_err();
    assert_eq!(invalid.parameter, "minor_radius");
}

#[test]
fn faults_lists_every_value_at_fault_in_field_order_and_check_is_its_first() {
    let cases = [
        (
            Primitive::Box {
                dx: 0.0,
                dy: 1.0,
                dz: -1.0,
            },
            vec!["dx", "dz"],
        ),
        (
            Primitive::Cone {
                radius1: 0.0,
                radius2: 0.0,
                height: 0.0,
            },
            vec!["radius1", "height"],
        ),
        (
            Primitive::Torus {
                major_radius: 0.0,
                minor_radius: 5.0,
            },
            vec!["major_radius"],
        ),
    ];
    for (primitive, expected) in cases {
        assert_eq!(parameters(primitive), expected, "{primitive:?}");
        assert_eq!(primitive.check().unwrap_err().parameter, expected[0]);
    }
}

#[test]
fn faults_names_each_parameter_as_asked() {
    let faults = Primitive::Torus {
        major_radius: 1.0,
        minor_radius: 2.0,
    }
    .faults(|p| match p {
        "major_radius" => "ring",
        "minor_radius" => "tube",
        other => other,
    });
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].parameter, "tube");
    assert_eq!(
        faults[0].message,
        "torus tube must be at most ring (1), got 2: the tube would cross the axis"
    );
}
