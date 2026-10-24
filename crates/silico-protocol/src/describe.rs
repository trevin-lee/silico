//! What a shape's elements are and measure: [`Op::Describe`](crate::Op::Describe)'s reply,
//! each face's, edge's, and vertex's geometry and measures as OCCT gives them, in
//! `TopExp::MapShapes` order (docs/design/m4.4-describe.md). The core names and orders
//! them.

use serde::{Deserialize, Serialize};

use crate::{Axis, NORMAL_LENGTH_TOLERANCE};

/// Each of a shape's faces, edges, and vertices, in `TopExp::MapShapes` order: its first is
/// the shape's Face1, Edge1, or Vertex1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Description {
    pub faces: Vec<FaceDescription>,
    pub edges: Vec<EdgeDescription>,
    /// Each vertex's point.
    pub vertices: Vec<[f64; 3]>,
}

/// A face: its surface, and its area and centre of area (to the relative
/// [`MEASURE_PRECISION`](crate::MEASURE_PRECISION)).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceDescription {
    pub surface: Surface,
    pub area: f64,
    pub centroid: [f64; 3],
}

/// A face's surface, as OCCT's `GeomAbs_SurfaceType` says, with what each kind is about.
/// Every axis's direction is a unit vector, as the surface has it: of either sign, since a
/// cylinder's, sphere's, or torus's axis has no way out of its own (a plane's normal is
/// what says which way a face faces).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Surface {
    /// Its unit normal, pointing out of a solid (the plane's own, reversed where the face
    /// is reversed).
    Plane {
        normal: [f64; 3],
    },
    Cylinder {
        axis: Axis,
        radius: f64,
    },
    /// Its radius at its axis's origin, and its half-angle (radians, the angle between its
    /// side and its axis, of either sign, as OCCT's `gp_Cone` has it).
    Cone {
        axis: Axis,
        radius: f64,
        half_angle: f64,
    },
    /// Its axis's origin is its centre.
    Sphere {
        axis: Axis,
        radius: f64,
    },
    Torus {
        axis: Axis,
        major_radius: f64,
        minor_radius: f64,
    },
    Bezier,
    Bspline,
    Revolution,
    Extrusion,
    Offset,
    Other,
}

impl Surface {
    /// Its type, as the element list writes it (docs/design/m4.1-command-api.md, *Element
    /// entries*).
    pub fn type_word(&self) -> &'static str {
        match self {
            Surface::Plane { .. } => "plane",
            Surface::Cylinder { .. } => "cylinder",
            Surface::Cone { .. } => "cone",
            Surface::Sphere { .. } => "sphere",
            Surface::Torus { .. } => "torus",
            Surface::Bezier => "bezier",
            Surface::Bspline => "bspline",
            Surface::Revolution => "revolution",
            Surface::Extrusion => "extrusion",
            Surface::Offset => "offset",
            Surface::Other => "other",
        }
    }
}

/// An edge: its curve, its length (to the relative
/// [`MEASURE_PRECISION`](crate::MEASURE_PRECISION)), its ends as it runs in the shape, and
/// its curve's point at the middle of its parameters. A degenerated edge (a sphere's pole,
/// a cone's apex) has no curve: it's `Other`, of length 0, its ends and middle its vertex.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeDescription {
    pub curve: EdgeCurve,
    pub length: f64,
    pub start: [f64; 3],
    pub end: [f64; 3],
    pub middle: [f64; 3],
    pub degenerated: bool,
}

/// An edge's curve, as OCCT's `GeomAbs_CurveType` says, with what each kind is about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EdgeCurve {
    Line,
    /// A whole circle or an arc of it: its centre and unit normal, and its radius.
    Circle {
        axis: Axis,
        radius: f64,
    },
    Ellipse {
        axis: Axis,
        major_radius: f64,
        minor_radius: f64,
    },
    Hyperbola,
    Parabola,
    Bezier,
    /// Its degree and how many poles it has.
    Bspline {
        degree: u32,
        poles: u32,
    },
    Offset,
    Other,
}

impl EdgeCurve {
    /// Its type, as the element list writes it (docs/design/m4.1-command-api.md, *Element
    /// entries*).
    pub fn type_word(&self) -> &'static str {
        match self {
            EdgeCurve::Line => "line",
            EdgeCurve::Circle { .. } => "circle",
            EdgeCurve::Ellipse { .. } => "ellipse",
            EdgeCurve::Hyperbola => "hyperbola",
            EdgeCurve::Parabola => "parabola",
            EdgeCurve::Bezier => "bezier",
            EdgeCurve::Bspline { .. } => "bspline",
            EdgeCurve::Offset => "offset",
            EdgeCurve::Other => "other",
        }
    }

    /// Whether it's straight: a line, or a degree-1 B-spline of two poles.
    pub fn is_straight(&self) -> bool {
        matches!(
            self,
            EdgeCurve::Line
                | EdgeCurve::Bspline {
                    degree: 1,
                    poles: 2
                }
        )
    }
}

impl Description {
    /// Its first value that isn't finite, by element and what it is (`Face3's area`), if
    /// any: what a worker refuses to send.
    pub fn non_finite(&self) -> Option<(String, f64)> {
        let mut values: Vec<(String, f64)> = Vec::new();
        let mut add = |at: &str, what: &str, xs: &[f64]| {
            values.extend(xs.iter().map(|&x| (format!("{at}'s {what}"), x)));
        };
        for (i, face) in self.faces.iter().enumerate() {
            let at = format!("Face{}", i + 1);
            add(&at, "area", &[face.area]);
            add(&at, "centre of area", &face.centroid);
            match &face.surface {
                Surface::Plane { normal } => add(&at, "normal", normal),
                Surface::Cylinder { axis, radius } | Surface::Sphere { axis, radius } => {
                    add(&at, "axis", &[axis.origin, axis.direction].concat());
                    add(&at, "radius", &[*radius]);
                }
                Surface::Cone {
                    axis,
                    radius,
                    half_angle,
                } => {
                    add(&at, "axis", &[axis.origin, axis.direction].concat());
                    add(&at, "radius", &[*radius, *half_angle]);
                }
                Surface::Torus {
                    axis,
                    major_radius,
                    minor_radius,
                } => {
                    add(&at, "axis", &[axis.origin, axis.direction].concat());
                    add(&at, "radius", &[*major_radius, *minor_radius]);
                }
                _ => {}
            }
        }
        for (i, edge) in self.edges.iter().enumerate() {
            let at = format!("Edge{}", i + 1);
            add(&at, "length", &[edge.length]);
            add(&at, "ends", &[edge.start, edge.end, edge.middle].concat());
            match &edge.curve {
                EdgeCurve::Circle { axis, radius } => {
                    add(&at, "axis", &[axis.origin, axis.direction].concat());
                    add(&at, "radius", &[*radius]);
                }
                EdgeCurve::Ellipse {
                    axis,
                    major_radius,
                    minor_radius,
                } => {
                    add(&at, "axis", &[axis.origin, axis.direction].concat());
                    add(&at, "radius", &[*major_radius, *minor_radius]);
                }
                _ => {}
            }
        }
        for (i, vertex) in self.vertices.iter().enumerate() {
            add(&format!("Vertex{}", i + 1), "point", vertex);
        }
        values.into_iter().find(|(_, x)| !x.is_finite())
    }

    /// Whether it can be trusted: every value finite, every direction a unit vector (within
    /// [`NORMAL_LENGTH_TOLERANCE`]), every radius above 0, every area at least 0, and every
    /// edge's length above 0 but a degenerated one's, which is 0. Says what's wrong, by
    /// element (`Face3`), if not.
    pub fn check(&self) -> Result<(), String> {
        for (i, face) in self.faces.iter().enumerate() {
            let at = format!("Face{}", i + 1);
            point(&at, "centre of area", face.centroid)?;
            if !(face.area >= 0.0 && face.area.is_finite()) {
                return Err(format!("{at}'s area is {}", face.area));
            }
            match &face.surface {
                Surface::Plane { normal } => unit(&at, "normal", *normal)?,
                Surface::Cylinder { axis, radius } | Surface::Sphere { axis, radius } => {
                    axis_of(&at, axis)?;
                    positive(&at, "radius", *radius)?;
                }
                Surface::Cone {
                    axis,
                    radius,
                    half_angle,
                } => {
                    axis_of(&at, axis)?;
                    if !(radius.is_finite() && *radius >= 0.0) {
                        return Err(format!("{at}'s radius is {radius}"));
                    }
                    if !(half_angle.is_finite() && half_angle.abs() < std::f64::consts::FRAC_PI_2) {
                        return Err(format!("{at}'s half-angle is {half_angle}"));
                    }
                }
                Surface::Torus {
                    axis,
                    major_radius,
                    minor_radius,
                } => {
                    axis_of(&at, axis)?;
                    positive(&at, "major radius", *major_radius)?;
                    positive(&at, "minor radius", *minor_radius)?;
                }
                _ => {}
            }
        }
        for (i, edge) in self.edges.iter().enumerate() {
            let at = format!("Edge{}", i + 1);
            point(&at, "start", edge.start)?;
            point(&at, "end", edge.end)?;
            point(&at, "middle", edge.middle)?;
            if edge.degenerated {
                if edge.length != 0.0 || edge.curve != EdgeCurve::Other {
                    return Err(format!("{at} is degenerated, but has a curve or a length"));
                }
                continue;
            }
            positive(&at, "length", edge.length)?;
            match &edge.curve {
                EdgeCurve::Circle { axis, radius } => {
                    axis_of(&at, axis)?;
                    positive(&at, "radius", *radius)?;
                }
                EdgeCurve::Ellipse {
                    axis,
                    major_radius,
                    minor_radius,
                } => {
                    axis_of(&at, axis)?;
                    positive(&at, "major radius", *major_radius)?;
                    positive(&at, "minor radius", *minor_radius)?;
                }
                _ => {}
            }
        }
        for (i, vertex) in self.vertices.iter().enumerate() {
            point(&format!("Vertex{}", i + 1), "point", *vertex)?;
        }
        Ok(())
    }
}

fn point(at: &str, what: &str, p: [f64; 3]) -> Result<(), String> {
    if p.iter().all(|c| c.is_finite()) {
        Ok(())
    } else {
        Err(format!("{at}'s {what} is {p:?}"))
    }
}

fn positive(at: &str, what: &str, value: f64) -> Result<(), String> {
    if value > 0.0 && value.is_finite() {
        Ok(())
    } else {
        Err(format!("{at}'s {what} is {value}"))
    }
}

fn unit(at: &str, what: &str, v: [f64; 3]) -> Result<(), String> {
    point(at, what, v)?;
    let length = libm::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
    if (length - 1.0).abs() <= NORMAL_LENGTH_TOLERANCE {
        Ok(())
    } else {
        Err(format!("{at}'s {what} {v:?} isn't a unit vector"))
    }
}

fn axis_of(at: &str, axis: &Axis) -> Result<(), String> {
    point(at, "axis's origin", axis.origin)?;
    unit(at, "axis's direction", axis.direction)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn z_axis() -> Axis {
        Axis {
            origin: [0.0; 3],
            direction: [0.0, 0.0, 1.0],
        }
    }

    fn good() -> Description {
        Description {
            faces: vec![
                FaceDescription {
                    surface: Surface::Plane {
                        normal: [0.0, 0.0, 1.0],
                    },
                    area: 1.0,
                    centroid: [0.5, 0.5, 1.0],
                },
                FaceDescription {
                    surface: Surface::Cylinder {
                        axis: z_axis(),
                        radius: 2.0,
                    },
                    area: 3.0,
                    centroid: [0.0; 3],
                },
            ],
            edges: vec![
                EdgeDescription {
                    curve: EdgeCurve::Line,
                    length: 1.0,
                    start: [0.0; 3],
                    end: [1.0, 0.0, 0.0],
                    middle: [0.5, 0.0, 0.0],
                    degenerated: false,
                },
                EdgeDescription {
                    curve: EdgeCurve::Other,
                    length: 0.0,
                    start: [0.0; 3],
                    end: [0.0; 3],
                    middle: [0.0; 3],
                    degenerated: true,
                },
            ],
            vertices: vec![[0.0; 3]],
        }
    }

    #[test]
    fn a_sound_description_passes_and_each_spoiled_value_is_named() {
        assert_eq!(good().check(), Ok(()));
        type Spoil = fn(&mut Description);
        let spoiled: [(Spoil, &str); 8] = [
            (|d| d.faces[0].area = -1.0, "Face1's area"),
            (
                |d| d.faces[0].centroid[1] = f64::NAN,
                "Face1's centre of area",
            ),
            (
                |d| {
                    d.faces[0].surface = Surface::Plane {
                        normal: [0.0, 0.0, 1.1],
                    }
                },
                "Face1's normal",
            ),
            (
                |d| {
                    d.faces[1].surface = Surface::Cylinder {
                        axis: z_axis(),
                        radius: 0.0,
                    }
                },
                "Face2's radius",
            ),
            (|d| d.edges[0].length = 0.0, "Edge1's length"),
            (|d| d.edges[0].middle[2] = f64::INFINITY, "Edge1's middle"),
            (|d| d.edges[1].length = 1.0, "Edge2 is degenerated"),
            (|d| d.vertices[0][0] = f64::NAN, "Vertex1's point"),
        ];
        for (spoil, says) in spoiled {
            let mut d = good();
            spoil(&mut d);
            let e = d.check().unwrap_err();
            assert!(e.starts_with(says), "{e}");
        }
    }

    #[test]
    fn the_first_value_not_finite_is_named() {
        assert_eq!(good().non_finite(), None);
        let mut d = good();
        d.edges[0].middle[1] = f64::NAN;
        d.vertices[0][2] = f64::INFINITY;
        let (what, value) = d.non_finite().unwrap();
        assert_eq!(what, "Edge1's ends");
        assert!(value.is_nan());
    }

    #[test]
    fn a_straight_edge_is_a_line_or_a_two_pole_degree_one_bspline() {
        assert!(EdgeCurve::Line.is_straight());
        assert!(
            EdgeCurve::Bspline {
                degree: 1,
                poles: 2
            }
            .is_straight()
        );
        assert!(
            !EdgeCurve::Bspline {
                degree: 1,
                poles: 3
            }
            .is_straight()
        );
        assert!(
            !EdgeCurve::Bspline {
                degree: 2,
                poles: 2
            }
            .is_straight()
        );
        assert!(
            !EdgeCurve::Circle {
                axis: z_axis(),
                radius: 1.0
            }
            .is_straight()
        );
    }
}
