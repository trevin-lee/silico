//! [`Op::Classify`](crate::Op::Classify): where a point is against a solid, and the rule for
//! which points may be asked, checked by the core before it asks and by the kernel and the
//! fake kernel before they answer.

use serde::{Deserialize, Serialize};

use crate::InvalidParameter;
use crate::render::MAX_RENDER_COORDINATE;

/// The farthest from the origin, in mm, a classified point may be: a render's farthest
/// point, as the eye is the point asked about. Far past it OCCT's classifier answers wrongly
/// (a box holds (1e18, 0, 0), as FreeCAD's `isInside` also says) or throws.
pub const MAX_CLASSIFY_COORDINATE: f64 = MAX_RENDER_COORDINATE;

/// Where a point is against a solid, as OCCT's classifier says: what counts as inside is the
/// caller's to decide (a camera's eye on a body's face counts, M4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointPlace {
    Inside,
    /// On the boundary, to about the shape's tolerance.
    OnBoundary,
    Outside,
}

/// Refuses a point that isn't finite, or is past [`MAX_CLASSIFY_COORDINATE`] of the origin
/// along any axis.
pub fn check_classify_point(point: [f64; 3]) -> Result<(), InvalidParameter> {
    match point_beyond("point", point, MAX_CLASSIFY_COORDINATE) {
        None => Ok(()),
        Some(message) => Err(InvalidParameter {
            parameter: "point",
            message,
        }),
    }
}

/// Why `point`, the value of `parameter`, can't be asked, if it can't: a coordinate that
/// isn't finite or is past `most` mm of the origin. The one rule, in one set of words, for
/// every point a request bounds (a classified point, a camera's eye and target, a point a
/// distance is measured from).
pub(crate) fn point_beyond(parameter: &str, point: [f64; 3], most: f64) -> Option<String> {
    (!point.iter().all(|c| c.abs() <= most))
        .then(|| format!("{parameter} {point:?} must be finite and within {most} mm of the origin"))
}
