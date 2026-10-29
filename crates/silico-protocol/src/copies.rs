//! Copies of a tool fused into or cut from a base in one boolean: the kernel's request for
//! the linear pattern, the polar pattern, and mirror (docs/ports/patterns.md, §7, *The
//! kernel*). The core works out where each copy goes from the document's integers, as
//! [`Placement`]s; the worker places each copy of the tool exactly there and runs the one
//! boolean, deciding nothing (ARCHITECTURE.md, invariant 2). The rules for the values live
//! here, so the core refuses a bad request before sending it and the worker refuses it again.

use std::f64::consts::TAU;

use serde::{Deserialize, Serialize};

use crate::primitive::{InvalidParameter, LENGTH_TOLERANCE, format_number};
use crate::profile::Axis;
use crate::sweep::{ANGULAR_TOLERANCE, check_point, check_unit};
use crate::{BooleanOp, check_offset};

/// The most instances a pattern has, the original included: decision 8's cap, fixed in the
/// format and never clamped (docs/ports/patterns.md).
pub const MAX_INSTANCES: usize = 1000;

/// The most copies one request carries: every instance of a pattern but the original, which
/// is in the base already.
pub const MAX_COPIES: usize = MAX_INSTANCES - 1;

/// Whether the copies are fused into the base (a pad's, a revolve's, or a whole solid's) or
/// cut from it (a pocket's or a groove's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CopiesOp {
    Fuse,
    Cut,
}

impl CopiesOp {
    /// The boolean it runs, by which its refusals are worded.
    pub fn boolean(self) -> BooleanOp {
        match self {
            CopiesOp::Fuse => BooleanOp::Fuse,
            CopiesOp::Cut => BooleanOp::Cut,
        }
    }
}

/// Where one copy of the tool goes: a rigid move, or a reflection. Lengths in the document's
/// unit, angles in radians, every direction a unit vector, as the core sends them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Placement {
    /// Moved by `offset`: a linear pattern's instance, or a grid's.
    Translation { offset: [f64; 3] },
    /// Turned `angle` radians about `axis`, right-handed: a polar pattern's instance.
    Rotation { axis: Axis, angle: f64 },
    /// Reflected in the plane through `origin` whose unit normal is `normal`: a mirror's copy.
    Mirror { origin: [f64; 3], normal: [f64; 3] },
    /// Turned `angle` radians about `axis`, right-handed, then moved by `offset`: a grid's
    /// instance of turned copies.
    TurnedAndMoved {
        axis: Axis,
        angle: f64,
        offset: [f64; 3],
    },
}

/// A copies request's values refused: the parameter at fault as errors name it, a dotted
/// name (`placements.2.angle`, the placements counted from 0, as a JSON path counts), and
/// what's wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopiesRefused {
    pub parameter: String,
    pub message: String,
}

impl Placement {
    /// Whether the placement moves a copy somewhere: every value finite, each point within
    /// a kilometre of the origin, each direction a unit vector (to rounding), and no
    /// placement that leaves the copy on the tool. A move is longer than OCCT's tolerance, as
    /// a prism's vector is ([`check_offset`]'s kilometre each way); a turn is more than OCCT's
    /// angular tolerance from none and from a whole turn either way. A reflection always
    /// moves something. Returns the field at fault, under the placement.
    pub fn check(&self) -> Result<(), InvalidParameter> {
        match *self {
            Placement::Translation { offset } => check_move(offset, "translation"),
            Placement::Rotation { axis, angle } => check_turn(axis, angle),
            Placement::Mirror { origin, normal } => {
                check_point("origin", "mirror plane's point", origin)?;
                check_unit("normal", "mirror plane's normal", normal)
            }
            Placement::TurnedAndMoved {
                axis,
                angle,
                offset,
            } => {
                check_turn(axis, angle)?;
                check_move(offset, "turned copy's move")
            }
        }
    }
}

/// That a move by `offset` is one: each component finite and at most a kilometre, as any
/// offset ([`check_offset`]), and the whole longer than OCCT's tolerance, so the copy isn't
/// left on the tool.
fn check_move(offset: [f64; 3], what: &str) -> Result<(), InvalidParameter> {
    check_offset(offset).map_err(|invalid| InvalidParameter {
        parameter: "offset",
        message: format!(
            "the {what}'s {}",
            invalid.message.trim_start_matches("the ")
        ),
    })?;
    let [x, y, z] = offset;
    let size = (x * x + y * y + z * z).sqrt();
    if size > LENGTH_TOLERANCE {
        Ok(())
    } else {
        Err(InvalidParameter {
            parameter: "offset",
            message: format!(
                "the {what} must move the copy more than 1e-7 mm, or it lies on the tool: its offset is {} mm long",
                format_number(size)
            ),
        })
    }
}

/// That a turn of `angle` radians about `axis` is one: the axis a place and a unit vector
/// (as a revolution's), the angle finite, more than OCCT's angular tolerance from none,
/// and less than a whole turn by more than it, either way.
fn check_turn(axis: Axis, angle: f64) -> Result<(), InvalidParameter> {
    check_point("axis", "turn's axis point", axis.origin)?;
    check_unit("axis", "turn's axis direction", axis.direction)?;
    let size = angle.abs();
    if size > ANGULAR_TOLERANCE && size < TAU - ANGULAR_TOLERANCE {
        Ok(())
    } else {
        Err(InvalidParameter {
            parameter: "angle",
            message: format!(
                "the turn's angle must be more than 1e-12 radians from none and from a whole turn, either way, or it leaves the copy on the tool: got {}",
                format_number(angle)
            ),
        })
    }
}

/// Whether a copies request can be sent: from 1 to [`MAX_COPIES`] placements, each one
/// [`Placement::check`] accepts. The parameter at fault is named under its placement.
pub fn check_copies(placements: &[Placement]) -> Result<(), CopiesRefused> {
    if !(1..=MAX_COPIES).contains(&placements.len()) {
        return Err(CopiesRefused {
            parameter: "placements".to_owned(),
            message: format!(
                "a copies request places 1 to {MAX_COPIES} copies (a pattern has at most {MAX_INSTANCES} instances, the original included), got {}",
                placements.len()
            ),
        });
    }
    for (n, placement) in placements.iter().enumerate() {
        placement.check().map_err(|invalid| CopiesRefused {
            parameter: invalid.name(Some(&format!("placements.{n}"))),
            message: format!("placements.{n}: {}", invalid.message),
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{PI, TAU};

    use super::{CopiesOp, MAX_COPIES, Placement, check_copies};
    use crate::{Axis, BooleanOp};

    const Z: Axis = Axis {
        origin: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
    };

    fn refused(placements: &[Placement]) -> (String, String) {
        let r = check_copies(placements).unwrap_err();
        (r.parameter, r.message)
    }

    #[test]
    fn each_op_runs_its_boolean() {
        assert_eq!(CopiesOp::Fuse.boolean(), BooleanOp::Fuse);
        assert_eq!(CopiesOp::Cut.boolean(), BooleanOp::Cut);
    }

    #[test]
    fn every_kind_of_placement_that_moves_a_copy_is_accepted() {
        let placements = [
            Placement::Translation {
                offset: [20.0, 0.0, 0.0],
            },
            // Just past the tolerance, and a kilometre each way.
            Placement::Translation {
                offset: [0.0, 1.1e-7, 0.0],
            },
            Placement::Translation {
                offset: [1e6, -1e6, 1e6],
            },
            Placement::Rotation {
                axis: Z,
                angle: PI / 2.0,
            },
            // A turn less a nanodegree, either way: 2π − 1.745e-11, more than 1e-12 short.
            Placement::Rotation {
                axis: Z,
                angle: TAU - crate::radians(1e-9),
            },
            Placement::Rotation {
                axis: Z,
                angle: -(TAU - crate::radians(1e-9)),
            },
            Placement::Mirror {
                origin: [1.0, 0.0, 0.0],
                normal: [1.0, 0.0, 0.0],
            },
            Placement::TurnedAndMoved {
                axis: Z,
                angle: -PI,
                offset: [0.0, 0.0, 5.0],
            },
        ];
        assert_eq!(check_copies(&placements), Ok(()));
        let most = vec![
            Placement::Translation {
                offset: [1.0, 0.0, 0.0]
            };
            MAX_COPIES
        ];
        assert_eq!(check_copies(&most), Ok(()));
    }

    #[test]
    fn no_copies_or_more_than_the_cap_is_refused_naming_placements() {
        let one = Placement::Translation {
            offset: [1.0, 0.0, 0.0],
        };
        for n in [0, MAX_COPIES + 1] {
            let (parameter, message) = refused(&vec![one; n]);
            assert_eq!(parameter, "placements");
            assert_eq!(
                message,
                format!(
                    "a copies request places 1 to 999 copies (a pattern has at most 1000 instances, the original included), got {n}"
                )
            );
        }
    }

    #[test]
    fn a_move_that_leaves_the_copy_on_the_tool_or_isnt_a_length_is_refused() {
        let at = |offset| {
            refused(&[
                Placement::Translation {
                    offset: [1.0, 0.0, 0.0],
                },
                Placement::Translation { offset },
            ])
        };
        // 1e-7 exactly is OCCT's tolerance: on the tool. So is -0.
        for offset in [[1e-7, 0.0, 0.0], [-0.0, 0.0, 0.0], [6e-8, 0.0, 6e-8]] {
            let (parameter, message) = at(offset);
            assert_eq!(parameter, "placements.1.offset", "{offset:?}");
            assert!(
                message.starts_with(
                    "placements.1: the translation must move the copy more than 1e-7 mm"
                ),
                "{message}"
            );
        }
        for (offset, part) in [
            ([f64::NAN, 0.0, 0.0], "x"),
            ([0.0, f64::INFINITY, 0.0], "y"),
            ([0.0, 0.0, -1.000_000_1e6], "z"),
        ] {
            let (parameter, message) = at(offset);
            assert_eq!(parameter, "placements.1.offset");
            assert!(
                message.starts_with(&format!(
                    "placements.1: the translation's offset's {part} must be a finite length"
                )),
                "{message}"
            );
        }
    }

    #[test]
    fn a_turn_of_nothing_or_of_a_whole_turn_or_about_no_axis_is_refused() {
        let turn = |axis, angle| refused(&[Placement::Rotation { axis, angle }]);
        for angle in [
            0.0,
            -0.0,
            1e-12,
            -1e-12,
            TAU,
            -TAU,
            TAU - 1e-12,
            7.0,
            f64::NAN,
            f64::INFINITY,
        ] {
            let (parameter, _) = turn(Z, angle);
            assert_eq!(parameter, "placements.0.angle", "{angle}");
        }
        for axis in [
            Axis {
                origin: [0.0; 3],
                direction: [0.0, 0.0, 2.0],
            },
            Axis {
                origin: [0.0; 3],
                direction: [0.0, 0.0, 0.0],
            },
            Axis {
                origin: [f64::NAN, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
            Axis {
                origin: [2e6, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
        ] {
            let (parameter, _) = turn(axis, 1.0);
            assert_eq!(parameter, "placements.0.axis", "{axis:?}");
        }
        // The turn of a turned copy that's moved is checked as a turn, and its move as a move.
        let (parameter, _) = refused(&[Placement::TurnedAndMoved {
            axis: Z,
            angle: 0.0,
            offset: [5.0, 0.0, 0.0],
        }]);
        assert_eq!(parameter, "placements.0.angle");
        let (parameter, _) = refused(&[Placement::TurnedAndMoved {
            axis: Z,
            angle: 1.0,
            offset: [0.0; 3],
        }]);
        assert_eq!(parameter, "placements.0.offset");
    }

    #[test]
    fn a_mirror_about_no_plane_is_refused() {
        let mirror = |origin, normal| refused(&[Placement::Mirror { origin, normal }]);
        assert_eq!(mirror([0.0; 3], [0.0, 0.0, 0.0]).0, "placements.0.normal");
        assert_eq!(mirror([0.0; 3], [0.0, 1.0, 1.0]).0, "placements.0.normal");
        assert_eq!(
            mirror([0.0, f64::NEG_INFINITY, 0.0], [0.0, 0.0, 1.0]).0,
            "placements.0.origin"
        );
    }
}
