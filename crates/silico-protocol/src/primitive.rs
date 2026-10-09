//! Solid primitives, placed as FreeCAD places them, and the rule for which values make
//! one. The rule lives here, with the type, so the core can refuse a value before sending
//! it and a worker can refuse it again before OCCT sees it (docs/ports/primitives.md).

use serde::{Deserialize, Serialize};

/// OCCT's length tolerance (`Precision::Confusion()`): lengths at or below it are zero.
pub const LENGTH_TOLERANCE: f64 = 1e-7;

/// The longest length a primitive may have: a kilometre, in millimetres. OCCT measures
/// some valid-looking solids wrongly when their lengths span more than about fourteen
/// orders of magnitude (a torus of radii 1e12 and 1e-3 comes out with a negative volume),
/// and a part this large is beyond any model Silico is for.
pub const MAX_LENGTH: f64 = 1e6;

/// A solid primitive. Each sits at the origin, its axis along +Z.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Primitive {
    /// From the origin to `(dx, dy, dz)`.
    Box { dx: f64, dy: f64, dz: f64 },
    /// Its base circle centered on the origin, rising to `height`.
    Cylinder { radius: f64, height: f64 },
    /// Centered on the origin.
    Sphere { radius: f64 },
    /// `radius1` at its base (centered on the origin), `radius2` at `height`. Either
    /// radius may be zero (a pointed cone), not both; radii within [`LENGTH_TOLERANCE`]
    /// of each other make a cylinder.
    Cone {
        radius1: f64,
        radius2: f64,
        height: f64,
    },
    /// Centered on the origin, around the Z axis: `major_radius` from the axis to the tube's
    /// center, `minor_radius` the tube's. The tube may touch the axis (`minor_radius` equal
    /// to `major_radius`, within [`LENGTH_TOLERANCE`]) but not cross it.
    Torus {
        major_radius: f64,
        minor_radius: f64,
    },
}

/// A primitive value that can't make a solid, naming the parameter at fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidParameter {
    /// The parameter's name, as the operation spells it (`radius`, `dx`, ...).
    pub parameter: &'static str,
    /// What's wrong, naming the parameter and the value.
    pub message: String,
}

impl Primitive {
    /// The name of the operation that makes it: `MakeBox`, `MakeCylinder`, ...
    pub fn op_name(&self) -> &'static str {
        match self {
            Primitive::Box { .. } => "MakeBox",
            Primitive::Cylinder { .. } => "MakeCylinder",
            Primitive::Sphere { .. } => "MakeSphere",
            Primitive::Cone { .. } => "MakeCone",
            Primitive::Torus { .. } => "MakeTorus",
        }
    }

    /// The primitive's name, for messages: `box`, `cylinder`, ...
    pub fn name(&self) -> &'static str {
        match self {
            Primitive::Box { .. } => "box",
            Primitive::Cylinder { .. } => "cylinder",
            Primitive::Sphere { .. } => "sphere",
            Primitive::Cone { .. } => "cone",
            Primitive::Torus { .. } => "torus",
        }
    }

    /// Whether these values make a solid: every length above [`LENGTH_TOLERANCE`] and at
    /// most [`MAX_LENGTH`], except that one of a cone's radii may be zero; and a torus's
    /// tube no wider than its ring.
    pub fn check(&self) -> Result<(), InvalidParameter> {
        let what = self.name();
        let length = |parameter: &'static str, value: f64| {
            if value > LENGTH_TOLERANCE && value <= MAX_LENGTH {
                Ok(())
            } else {
                Err(InvalidParameter {
                    parameter,
                    message: format!(
                        "{what} {parameter} must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got {}",
                        format_number(value)
                    ),
                })
            }
        };
        match *self {
            Primitive::Box { dx, dy, dz } => {
                length("dx", dx)?;
                length("dy", dy)?;
                length("dz", dz)
            }
            Primitive::Cylinder { radius, height } => {
                length("radius", radius)?;
                length("height", height)
            }
            Primitive::Sphere { radius } => length("radius", radius),
            Primitive::Cone {
                radius1,
                radius2,
                height,
            } => {
                for (parameter, value) in [("radius1", radius1), ("radius2", radius2)] {
                    if !(value == 0.0 || (value > LENGTH_TOLERANCE && value <= MAX_LENGTH)) {
                        return Err(InvalidParameter {
                            parameter,
                            message: format!(
                                "cone {parameter} must be zero, or a length above 1e-7 mm and at most 1e6 mm (a kilometre), got {}",
                                format_number(value)
                            ),
                        });
                    }
                }
                if radius1 == 0.0 && radius2 == 0.0 {
                    return Err(InvalidParameter {
                        parameter: "radius1",
                        message: "cone radius1 must be above zero where radius2 is zero: a cone with no radius has no volume".to_owned(),
                    });
                }
                length("height", height)
            }
            Primitive::Torus {
                major_radius,
                minor_radius,
            } => {
                length("major_radius", major_radius)?;
                length("minor_radius", minor_radius)?;
                if minor_radius - major_radius > LENGTH_TOLERANCE {
                    return Err(InvalidParameter {
                        parameter: "minor_radius",
                        message: format!(
                            "torus minor_radius must be at most major_radius ({}), got {}: the tube would cross the axis",
                            format_number(major_radius),
                            format_number(minor_radius)
                        ),
                    });
                }
                Ok(())
            }
        }
    }
}

/// A number for a message: as written for ordinary sizes, in exponent form for very large
/// or very small ones (so `f64::MAX` isn't 309 digits).
pub fn format_number(value: f64) -> String {
    let size = value.abs();
    if value.is_finite() && size != 0.0 && !(1e-3..1e7).contains(&size) {
        format!("{value:e}")
    } else {
        format!("{value}")
    }
}
