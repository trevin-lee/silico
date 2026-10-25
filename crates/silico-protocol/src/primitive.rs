//! Solid primitives, placed as FreeCAD places them, and the rule for which values make
//! one. The rule lives here, with the type, so the core can refuse a value before sending
//! it and a worker can refuse it again before OCCT sees it (docs/ports/primitives.md).

use serde::{Deserialize, Serialize};

/// OCCT's length tolerance (`Precision::Confusion()`): lengths at or below it are zero.
pub const LENGTH_TOLERANCE: f64 = 1e-7;

/// The finest a display mesh is made, as a fraction of its shape's bounding-box diagonal.
/// Finer meshes cost far more than they're worth for display: every tenth of the deflection
/// is about ten times the triangles and seventy times the time (docs/ports/tessellation.md).
pub const MIN_RELATIVE_DEFLECTION: f64 = 1e-4;

/// The display mesh's floor for a shape whose bounding box's diagonal is `diagonal`: the
/// finest deflection it's meshed at, [`MIN_RELATIVE_DEFLECTION`] of the diagonal and never
/// under [`LENGTH_TOLERANCE`]. The kernel applies it to OCCT's enlarged box, the fake kernel
/// to its own.
pub fn mesh_floor(diagonal: f64) -> f64 {
    (diagonal * MIN_RELATIVE_DEFLECTION).max(LENGTH_TOLERANCE)
}

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
    /// The parameter's name, as the check spells it (`radius`, `dx`, `points`, ...).
    pub parameter: &'static str,
    /// What's wrong, naming the parameter and the value.
    pub message: String,
}

/// The field of a translation that's its offset, under which a check's `x`, `y`, and `z` sit.
pub const OFFSET_FIELD: &str = "offset";

/// The field of a chamfer that's its size, under which a check's `distance`, `other`, and
/// `angle` sit.
pub const SIZE_FIELD: &str = "size";

impl InvalidParameter {
    /// The parameter at fault as errors name it: a dotted name, the operation's field, and
    /// the part of it at fault where it has parts (`offset.x`, a chamfer's `size.angle`);
    /// `under`, the field the checked value is a part of, if any. A profile's corners are
    /// the profile. The core and the worker both name parameters this way.
    pub fn name(&self, under: Option<&str>) -> String {
        let name = match self.parameter {
            "points" => "profile",
            parameter => parameter,
        };
        match under {
            Some(under) => format!("{under}.{name}"),
            None => name.to_owned(),
        }
    }
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

    /// The primitive's values, in the order its fields are declared: what a recompute key
    /// (M2.5 keeps them on disk) and a fake kernel's shape write after its name, so changing
    /// the order changes every key (see recompute's `Step::key`).
    pub fn values(&self) -> Vec<f64> {
        match *self {
            Primitive::Box { dx, dy, dz } => vec![dx, dy, dz],
            Primitive::Cylinder { radius, height } => vec![radius, height],
            Primitive::Sphere { radius } => vec![radius],
            Primitive::Cone {
                radius1,
                radius2,
                height,
            } => vec![radius1, radius2, height],
            Primitive::Torus {
                major_radius,
                minor_radius,
            } => vec![major_radius, minor_radius],
        }
    }

    /// The primitive's name, for messages: `box`, `cylinder`, ... A recompute key spells the
    /// primitive with it too, so changing it changes every key (see recompute's `Step::key`).
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
    /// tube no wider than its ring. Refused naming the first value at fault.
    pub fn check(&self) -> Result<(), InvalidParameter> {
        match self.faults(|parameter| parameter).into_iter().next() {
            Some(invalid) => Err(invalid),
            None => Ok(()),
        }
    }

    /// Every value at fault by [`check`](Self::check)'s rule, in the order of the fields,
    /// each parameter called `name(parameter)` in its error and message: a document calls
    /// a box's `dx` its `length`. A torus's tube wider than its ring is a fault only where
    /// each radius alone is valid.
    pub fn faults(&self, name: impl Fn(&'static str) -> &'static str) -> Vec<InvalidParameter> {
        let what = self.name();
        let mut faults = Vec::new();
        let length = |parameter: &'static str, value: f64| {
            let parameter = name(parameter);
            (!(value > LENGTH_TOLERANCE && value <= MAX_LENGTH)).then(|| InvalidParameter {
                parameter,
                message: format!(
                    "{what} {parameter} must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got {}",
                    format_number(value)
                ),
            })
        };
        match *self {
            Primitive::Box { dx, dy, dz } => {
                faults.extend(length("dx", dx));
                faults.extend(length("dy", dy));
                faults.extend(length("dz", dz));
            }
            Primitive::Cylinder { radius, height } => {
                faults.extend(length("radius", radius));
                faults.extend(length("height", height));
            }
            Primitive::Sphere { radius } => faults.extend(length("radius", radius)),
            Primitive::Cone {
                radius1,
                radius2,
                height,
            } => {
                for (parameter, value) in [("radius1", radius1), ("radius2", radius2)] {
                    let parameter = name(parameter);
                    if !(value == 0.0 || (value > LENGTH_TOLERANCE && value <= MAX_LENGTH)) {
                        faults.push(InvalidParameter {
                            parameter,
                            message: format!(
                                "cone {parameter} must be zero, or a length above 1e-7 mm and at most 1e6 mm (a kilometre), got {}",
                                format_number(value)
                            ),
                        });
                    }
                }
                if radius1 == 0.0 && radius2 == 0.0 {
                    let (radius1, radius2) = (name("radius1"), name("radius2"));
                    faults.push(InvalidParameter {
                        parameter: radius1,
                        message: format!(
                            "cone {radius1} must be above zero where {radius2} is zero: a cone with no radius has no volume"
                        ),
                    });
                }
                faults.extend(length("height", height));
            }
            Primitive::Torus {
                major_radius,
                minor_radius,
            } => {
                let major = length("major_radius", major_radius);
                let minor = length("minor_radius", minor_radius);
                let both = major.is_none() && minor.is_none();
                faults.extend(major);
                faults.extend(minor);
                if both && minor_radius - major_radius > LENGTH_TOLERANCE {
                    let (major, minor) = (name("major_radius"), name("minor_radius"));
                    faults.push(InvalidParameter {
                        parameter: minor,
                        message: format!(
                            "torus {minor} must be at most {major} ({}), got {}: the tube would cross the axis",
                            format_number(major_radius),
                            format_number(minor_radius)
                        ),
                    });
                }
            }
        }
        faults
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

/// Whether a move by `offset` (x, y, z) is allowed: each component finite and at most
/// [`MAX_LENGTH`] either way, as far as a primitive may reach. The rule lives here, as
/// [`Primitive::check`] does, so the core refuses an offset before sending it and a worker
/// refuses it again.
pub fn check_offset(offset: [f64; 3]) -> Result<(), InvalidParameter> {
    for (parameter, value) in ["x", "y", "z"].into_iter().zip(offset) {
        if !(value.is_finite() && value.abs() <= MAX_LENGTH) {
            return Err(InvalidParameter {
                parameter,
                message: format!(
                    "the offset's {parameter} must be a finite length of at most {} either way, got {}",
                    format_number(MAX_LENGTH),
                    format_number(value)
                ),
            });
        }
    }
    Ok(())
}
