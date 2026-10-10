//! How far a chamfer cuts back from its edges, and the rule for which sizes can work. The
//! rule lives here, with the type, so the core refuses a size before sending it and a
//! worker refuses it again before OCCT sees it (docs/ports/chamfer.md).

use serde::{Deserialize, Serialize};

use crate::primitive::{InvalidParameter, LENGTH_TOLERANCE, format_number};
use crate::profile::{check_length, radians};

/// How a chamfer cuts back from each of its edges. An edge's reference face is the
/// lower-numbered of the two faces it joins, or the higher-numbered one when the chamfer is
/// flipped (FreeCAD's `FlipDirection`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ChamferSize {
    /// `distance` back from the edge along both faces.
    Equal { distance: f64 },
    /// `distance` back along the reference face, `other` along the other face.
    TwoDistances { distance: f64, other: f64 },
    /// `distance` back along the reference face, the chamfer face at `angle` degrees to
    /// it: on a square edge, it reaches `distance · tan(angle)` along the other face.
    DistanceAngle { distance: f64, angle: f64 },
}

impl ChamferSize {
    /// Whether the sizes can make a chamfer on some edge: each distance above OCCT's
    /// tolerance and at most a kilometre, and an angle above 0 and below 180 degrees whose
    /// cut, the distance times the angle in radians, is deeper than the tolerance. Whether
    /// they fit a particular edge is OCCT's to decide.
    pub fn check(&self) -> Result<(), InvalidParameter> {
        match *self {
            ChamferSize::Equal { distance } => {
                check_length("distance", "chamfer's distance", distance)
            }
            ChamferSize::TwoDistances { distance, other } => {
                check_length("distance", "chamfer's distance", distance)?;
                check_length("other", "chamfer's other distance", other)
            }
            ChamferSize::DistanceAngle { distance, angle } => {
                check_length("distance", "chamfer's distance", distance)?;
                if !(angle > 0.0 && angle < 180.0) {
                    return Err(InvalidParameter {
                        parameter: "angle",
                        message: format!(
                            "the chamfer's angle must be more than 0 and less than 180 degrees, got {}",
                            format_number(angle)
                        ),
                    });
                }
                // On a square edge the cut reaches distance · tan(angle) along the other
                // face, a hair more than the distance times the angle in radians, the
                // measure used here (as a revolve's farthest corner moves its distance times
                // the angle). Under OCCT's tolerance that's no cut: OCCT fails at 1e-9
                // degrees, and at 1e-6 builds one 1.7e-8 deep. (On the flattest edge OCCT
                // takes, the cut reaches up to 20 times further.)
                if distance * radians(angle) <= LENGTH_TOLERANCE {
                    return Err(InvalidParameter {
                        parameter: "angle",
                        message: format!(
                            "the chamfer's angle of {} degrees is too small for its distance of {}: the distance times the angle in radians must be more than 1e-7 mm",
                            format_number(angle),
                            format_number(distance)
                        ),
                    });
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_that_can_work_pass() {
        for size in [
            ChamferSize::Equal { distance: 2.0 },
            ChamferSize::Equal {
                distance: 1.0000001e-7,
            },
            ChamferSize::Equal { distance: 1e6 },
            ChamferSize::TwoDistances {
                distance: 1.0,
                other: 3.0,
            },
            ChamferSize::DistanceAngle {
                distance: 1.0,
                angle: 30.0,
            },
            // The cut, 1 · 1e-5° in radians, is 1.7e-7: deeper than the tolerance.
            ChamferSize::DistanceAngle {
                distance: 1.0,
                angle: 1e-5,
            },
            ChamferSize::DistanceAngle {
                distance: 1e6,
                angle: 1e-11,
            },
            ChamferSize::DistanceAngle {
                distance: 1.0,
                angle: 179.99999999999997,
            },
        ] {
            assert_eq!(size.check(), Ok(()), "{size:?}");
        }
    }

    #[test]
    fn sizes_that_cant_work_name_their_parameter() {
        let bad_lengths = [
            0.0,
            -0.0,
            -1.0,
            1e-7,
            1e6 * (1.0 + f64::EPSILON),
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];
        for bad in bad_lengths {
            let cases = [
                (ChamferSize::Equal { distance: bad }, "distance"),
                (
                    ChamferSize::TwoDistances {
                        distance: bad,
                        other: 1.0,
                    },
                    "distance",
                ),
                (
                    ChamferSize::TwoDistances {
                        distance: 1.0,
                        other: bad,
                    },
                    "other",
                ),
                (
                    ChamferSize::DistanceAngle {
                        distance: bad,
                        angle: 45.0,
                    },
                    "distance",
                ),
            ];
            for (size, parameter) in cases {
                let invalid = size.check().unwrap_err();
                assert_eq!(invalid.parameter, parameter, "{size:?}");
                assert!(invalid.message.contains("chamfer's"), "{}", invalid.message);
            }
        }
        // Too shallow a cut: the distance times the angle in radians at or under 1e-7.
        for (distance, angle) in [
            (1.0, 1e-6),
            (1.0, 5.7e-6),
            (1e-6, 5.0),
            (1e6, 5e-12),
            (1.0, 5e-324),
        ] {
            let size = ChamferSize::DistanceAngle { distance, angle };
            let invalid = size.check().unwrap_err();
            assert_eq!(invalid.parameter, "angle", "{size:?}");
            assert!(invalid.message.contains("too small"), "{}", invalid.message);
        }
        for angle in [0.0, -0.0, -30.0, 180.0, 270.0, f64::NAN, f64::INFINITY] {
            let size = ChamferSize::DistanceAngle {
                distance: 1.0,
                angle,
            };
            let invalid = size.check().unwrap_err();
            assert_eq!(invalid.parameter, "angle", "{size:?}");
            assert!(
                invalid.message.contains("less than 180 degrees"),
                "{}",
                invalid.message
            );
        }
    }
}
