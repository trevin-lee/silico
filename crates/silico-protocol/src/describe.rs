//! What a shape's elements are and measure: [`Op::Describe`](crate::Op::Describe)'s reply,
//! each face's, edge's, or vertex's geometry and measures as OCCT gives them, in
//! `TopExp::MapShapes` order, a batch of one kind at a time (docs/design/m4.4-describe.md).
//! The core joins the batches, and names and orders the elements.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Axis, ElementKind, NORMAL_LENGTH_TOLERANCE};

/// The most elements one [`Op::Describe`](crate::Op::Describe) asks for, so no request nears
/// the kernel's timeout (docs/design/m4.1-command-api.md, the `elements` command).
pub const DESCRIBE_BATCH: u32 = 1000;

/// A shape's faces, edges, and vertices, in `TopExp::MapShapes` order: the first of each is
/// the shape's Face1, Edge1, or Vertex1, unless it's a batch's (`Reply::Described`), which
/// holds elements of one kind from the one its request asked for.
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
    /// Its unit normal, pointing out of a solid: the surface's (Du × Dv, which is the
    /// plane's axis's opposite where its frame is left-handed), reversed where the face is
    /// reversed. An internal or external face's is its surface's.
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

/// An edge: its curve, its length (to the relative
/// [`MEASURE_PRECISION`](crate::MEASURE_PRECISION)), its ends in its curve's own order (its
/// first vertex where its curve starts, as FreeCAD's `Edge.firstVertex()` gives it, so they
/// don't swap with the orientation the faces around it happen to give it), and its curve's
/// point at the middle of its parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeDescription {
    pub curve: EdgeCurve,
    pub length: f64,
    pub start: [f64; 3],
    pub end: [f64; 3],
    pub middle: [f64; 3],
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
    /// Its degree and how many poles it has.
    Bezier {
        degree: u32,
        poles: u32,
    },
    /// Its degree and how many poles it has.
    Bspline {
        degree: u32,
        poles: u32,
    },
    Offset,
    Other,
    /// No curve: a point (a sphere's pole, a cone's apex), of length 0, its ends and middle
    /// its vertex.
    Degenerated,
}

impl EdgeCurve {
    /// Whether it's straight: a line, or a Bézier or B-spline of two poles (FreeCAD's
    /// `de14265ba7`, docs/ports/measure.md).
    pub fn is_straight(&self) -> bool {
        matches!(
            self,
            EdgeCurve::Line
                | EdgeCurve::Bezier { poles: 2, .. }
                | EdgeCurve::Bspline { poles: 2, .. }
        )
    }
}

/// What a value of a [`Description`] is, for a message: its element, by the kernel's number
/// for it among those described (`Face3`), and what of it (`area`). It's formatted only when
/// a message needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quantity {
    pub kind: ElementKind,
    pub number: usize,
    pub what: &'static str,
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            ElementKind::Face => "Face",
            ElementKind::Edge => "Edge",
            ElementKind::Vertex => "Vertex",
            ElementKind::Solid => "Solid",
        };
        write!(f, "the kernel's {kind}{}'s {}", self.number, self.what)
    }
}

impl Description {
    /// Every number it holds, with what each is, in order: one walk, so the worker's refusal
    /// and the core's check name a value alike.
    fn each(&self, f: &mut dyn FnMut(Quantity, f64)) {
        let mut put = |kind: ElementKind, number: usize, what: &'static str, xs: &[f64]| {
            for &x in xs {
                f(Quantity { kind, number, what }, x);
            }
        };
        for (i, face) in self.faces.iter().enumerate() {
            let (k, n) = (ElementKind::Face, i + 1);
            put(k, n, "area", &[face.area]);
            put(k, n, "centre of area", &face.centroid);
            match &face.surface {
                Surface::Plane { normal } => put(k, n, "normal", normal),
                Surface::Cylinder { axis, radius } | Surface::Sphere { axis, radius } => {
                    put(k, n, "axis's origin", &axis.origin);
                    put(k, n, "axis's direction", &axis.direction);
                    put(k, n, "radius", &[*radius]);
                }
                Surface::Cone {
                    axis,
                    radius,
                    half_angle,
                } => {
                    put(k, n, "axis's origin", &axis.origin);
                    put(k, n, "axis's direction", &axis.direction);
                    put(k, n, "radius", &[*radius]);
                    put(k, n, "half-angle", &[*half_angle]);
                }
                Surface::Torus {
                    axis,
                    major_radius,
                    minor_radius,
                } => {
                    put(k, n, "axis's origin", &axis.origin);
                    put(k, n, "axis's direction", &axis.direction);
                    put(k, n, "major radius", &[*major_radius]);
                    put(k, n, "minor radius", &[*minor_radius]);
                }
                _ => {}
            }
        }
        for (i, edge) in self.edges.iter().enumerate() {
            let (k, n) = (ElementKind::Edge, i + 1);
            put(k, n, "length", &[edge.length]);
            put(k, n, "start", &edge.start);
            put(k, n, "end", &edge.end);
            put(k, n, "middle", &edge.middle);
            match &edge.curve {
                EdgeCurve::Circle { axis, radius } => {
                    put(k, n, "axis's origin", &axis.origin);
                    put(k, n, "axis's direction", &axis.direction);
                    put(k, n, "radius", &[*radius]);
                }
                EdgeCurve::Ellipse {
                    axis,
                    major_radius,
                    minor_radius,
                } => {
                    put(k, n, "axis's origin", &axis.origin);
                    put(k, n, "axis's direction", &axis.direction);
                    put(k, n, "major radius", &[*major_radius]);
                    put(k, n, "minor radius", &[*minor_radius]);
                }
                _ => {}
            }
        }
        for (i, vertex) in self.vertices.iter().enumerate() {
            put(ElementKind::Vertex, i + 1, "point", vertex);
        }
    }

    /// Its first value that isn't finite, with what it is, if any: what a worker refuses to
    /// send.
    pub fn non_finite(&self) -> Option<(Quantity, f64)> {
        let mut first = None;
        self.each(&mut |quantity, x| {
            if first.is_none() && !x.is_finite() {
                first = Some((quantity, x));
            }
        });
        first
    }

    /// Whether it can be trusted: every value finite; every direction a unit vector (within
    /// [`NORMAL_LENGTH_TOLERANCE`]); every radius above 0 (a cone's at least 0, at its apex),
    /// a cone's half-angle within a right angle, every area at least 0, and every edge's
    /// length above 0 but a degenerated one's, which is 0. Says what's wrong, by element
    /// (the kernel's `Face3`), if not.
    pub fn validate(&self) -> Result<(), String> {
        if let Some((quantity, x)) = self.non_finite() {
            return Err(format!("{quantity} is {x}"));
        }
        let mut problem = None;
        self.each(&mut |quantity, x| {
            if problem.is_some() {
                return;
            }
            let ok = match quantity.what {
                "area" => x >= 0.0,
                "radius" if is_cone(self, quantity) => x >= 0.0,
                "radius" | "major radius" | "minor radius" => x > 0.0,
                "half-angle" => x.abs() < std::f64::consts::FRAC_PI_2,
                "length" => edge_length_ok(self, quantity, x),
                _ => true,
            };
            if !ok {
                problem = Some(format!("{quantity} is {x}"));
            }
        });
        if let Some(problem) = problem {
            return Err(problem);
        }
        for (i, face) in self.faces.iter().enumerate() {
            let at = |what| Quantity {
                kind: ElementKind::Face,
                number: i + 1,
                what,
            };
            match &face.surface {
                Surface::Plane { normal } => unit(at("normal"), *normal)?,
                Surface::Cylinder { axis, .. }
                | Surface::Sphere { axis, .. }
                | Surface::Cone { axis, .. }
                | Surface::Torus { axis, .. } => unit(at("axis's direction"), axis.direction)?,
                _ => {}
            }
        }
        for (i, edge) in self.edges.iter().enumerate() {
            let at = |what| Quantity {
                kind: ElementKind::Edge,
                number: i + 1,
                what,
            };
            match &edge.curve {
                EdgeCurve::Circle { axis, .. } | EdgeCurve::Ellipse { axis, .. } => {
                    unit(at("axis's direction"), axis.direction)?;
                }
                EdgeCurve::Degenerated if edge.start != edge.end || edge.start != edge.middle => {
                    return Err(format!(
                        "{} isn't its start, though the edge is degenerated",
                        at("end")
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn is_cone(d: &Description, q: Quantity) -> bool {
    q.kind == ElementKind::Face && matches!(d.faces[q.number - 1].surface, Surface::Cone { .. })
}

fn edge_length_ok(d: &Description, q: Quantity, length: f64) -> bool {
    if d.edges[q.number - 1].curve == EdgeCurve::Degenerated {
        length == 0.0
    } else {
        length > 0.0
    }
}

fn unit(quantity: Quantity, v: [f64; 3]) -> Result<(), String> {
    let length = libm::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
    if (length - 1.0).abs() <= NORMAL_LENGTH_TOLERANCE {
        Ok(())
    } else {
        Err(format!("{quantity} {v:?} isn't a unit vector"))
    }
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

    /// An axis whose direction isn't a unit vector.
    fn tilted() -> Axis {
        Axis {
            origin: [0.0; 3],
            direction: [0.0, 0.1, 1.0],
        }
    }

    fn face(surface: Surface) -> FaceDescription {
        FaceDescription {
            surface,
            area: 1.0,
            centroid: [0.5, 0.5, 1.0],
        }
    }

    fn edge(curve: EdgeCurve) -> EdgeDescription {
        EdgeDescription {
            curve,
            length: 1.0,
            start: [0.0; 3],
            end: [1.0, 0.0, 0.0],
            middle: [0.5, 0.0, 0.0],
        }
    }

    /// One of every kind of surface and curve that carries values, sound.
    fn good() -> Description {
        Description {
            faces: vec![
                face(Surface::Plane {
                    normal: [0.0, 0.0, 1.0],
                }),
                face(Surface::Cylinder {
                    axis: z_axis(),
                    radius: 2.0,
                }),
                face(Surface::Cone {
                    axis: z_axis(),
                    radius: 0.0,
                    half_angle: -0.5,
                }),
                face(Surface::Sphere {
                    axis: z_axis(),
                    radius: 3.0,
                }),
                face(Surface::Torus {
                    axis: z_axis(),
                    major_radius: 10.0,
                    minor_radius: 2.0,
                }),
            ],
            edges: vec![
                edge(EdgeCurve::Line),
                edge(EdgeCurve::Circle {
                    axis: z_axis(),
                    radius: 2.5,
                }),
                edge(EdgeCurve::Ellipse {
                    axis: z_axis(),
                    major_radius: 3.0,
                    minor_radius: 1.0,
                }),
                EdgeDescription {
                    curve: EdgeCurve::Degenerated,
                    length: 0.0,
                    start: [0.0, 0.0, 3.0],
                    end: [0.0, 0.0, 3.0],
                    middle: [0.0, 0.0, 3.0],
                },
            ],
            vertices: vec![[0.0; 3]],
        }
    }

    #[test]
    fn a_sound_description_passes_and_each_spoiled_value_is_named() {
        assert_eq!(good().validate(), Ok(()));
        type Spoil = fn(&mut Description);
        let spoiled: Vec<(Spoil, &str)> = vec![
            (
                |d| d.faces[0].area = -1.0,
                "the kernel's Face1's area is -1",
            ),
            (
                |d| d.faces[0].centroid[1] = f64::NAN,
                "the kernel's Face1's centre of area is NaN",
            ),
            (
                |d| {
                    d.faces[0].surface = Surface::Plane {
                        normal: [0.0, 0.0, 1.1],
                    }
                },
                "the kernel's Face1's normal",
            ),
            (
                |d| {
                    d.faces[1].surface = Surface::Cylinder {
                        axis: z_axis(),
                        radius: 0.0,
                    }
                },
                "the kernel's Face2's radius is 0",
            ),
            (
                |d| {
                    d.faces[1].surface = Surface::Cylinder {
                        axis: Axis {
                            origin: [f64::INFINITY, 0.0, 0.0],
                            direction: [0.0, 0.0, 1.0],
                        },
                        radius: 1.0,
                    }
                },
                "the kernel's Face2's axis's origin is inf",
            ),
            (
                |d| {
                    d.faces[2].surface = Surface::Cone {
                        axis: z_axis(),
                        radius: -1.0,
                        half_angle: 0.5,
                    }
                },
                "the kernel's Face3's radius is -1",
            ),
            (
                |d| {
                    d.faces[2].surface = Surface::Cone {
                        axis: z_axis(),
                        radius: 1.0,
                        half_angle: -1.6,
                    }
                },
                "the kernel's Face3's half-angle is -1.6",
            ),
            (
                |d| {
                    d.faces[3].surface = Surface::Sphere {
                        axis: tilted(),
                        radius: 1.0,
                    }
                },
                "the kernel's Face4's axis's direction",
            ),
            (
                |d| {
                    d.faces[4].surface = Surface::Torus {
                        axis: z_axis(),
                        major_radius: 10.0,
                        minor_radius: 0.0,
                    }
                },
                "the kernel's Face5's minor radius is 0",
            ),
            (
                |d| {
                    d.faces[4].surface = Surface::Torus {
                        axis: z_axis(),
                        major_radius: -1.0,
                        minor_radius: 1.0,
                    }
                },
                "the kernel's Face5's major radius is -1",
            ),
            (
                |d| d.edges[0].length = 0.0,
                "the kernel's Edge1's length is 0",
            ),
            (
                |d| d.edges[0].start[0] = f64::NAN,
                "the kernel's Edge1's start is NaN",
            ),
            (
                |d| d.edges[0].end[2] = f64::NEG_INFINITY,
                "the kernel's Edge1's end is -inf",
            ),
            (
                |d| d.edges[0].middle[2] = f64::INFINITY,
                "the kernel's Edge1's middle is inf",
            ),
            (
                |d| {
                    d.edges[1].curve = EdgeCurve::Circle {
                        axis: z_axis(),
                        radius: 0.0,
                    }
                },
                "the kernel's Edge2's radius is 0",
            ),
            (
                |d| {
                    d.edges[1].curve = EdgeCurve::Circle {
                        axis: tilted(),
                        radius: 1.0,
                    }
                },
                "the kernel's Edge2's axis's direction",
            ),
            (
                |d| {
                    d.edges[2].curve = EdgeCurve::Ellipse {
                        axis: z_axis(),
                        major_radius: 3.0,
                        minor_radius: -1.0,
                    }
                },
                "the kernel's Edge3's minor radius is -1",
            ),
            (
                |d| d.edges[3].length = 1.0,
                "the kernel's Edge4's length is 1",
            ),
            (
                |d| d.edges[3].end = [1.0, 0.0, 0.0],
                "the kernel's Edge4's end isn't its start",
            ),
            (
                |d| d.vertices[0][0] = f64::NAN,
                "the kernel's Vertex1's point is NaN",
            ),
        ];
        for (spoil, says) in spoiled {
            let mut d = good();
            spoil(&mut d);
            let e = d.validate().unwrap_err();
            assert!(e.starts_with(says), "{e}, not {says}");
        }
    }

    #[test]
    fn the_first_value_not_finite_is_named_whatever_element_it_is_of() {
        assert_eq!(good().non_finite(), None);
        let first = |spoil: fn(&mut Description)| {
            let mut d = good();
            spoil(&mut d);
            let (quantity, value) = d.non_finite().unwrap();
            (quantity.to_string(), value.is_nan() || value.is_infinite())
        };
        type Spoil = fn(&mut Description);
        let cases: [(Spoil, &str); 4] = [
            (
                |d| d.faces[4].centroid[2] = f64::NAN,
                "the kernel's Face5's centre of area",
            ),
            (
                |d| {
                    d.edges[0].middle[1] = f64::NAN;
                    d.vertices[0][2] = f64::INFINITY;
                },
                "the kernel's Edge1's middle",
            ),
            (
                |d| d.vertices[0][2] = f64::INFINITY,
                "the kernel's Vertex1's point",
            ),
            (
                |d| {
                    d.faces[2].surface = Surface::Cone {
                        axis: z_axis(),
                        radius: 1.0,
                        half_angle: f64::NAN,
                    }
                },
                "the kernel's Face3's half-angle",
            ),
        ];
        for (spoil, says) in cases {
            assert_eq!(first(spoil), (says.to_owned(), true));
        }
    }

    #[test]
    fn a_straight_edge_is_a_line_or_a_two_pole_bezier_or_bspline() {
        assert!(EdgeCurve::Line.is_straight());
        assert!(
            EdgeCurve::Bspline {
                degree: 1,
                poles: 2
            }
            .is_straight()
        );
        assert!(
            EdgeCurve::Bezier {
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
            !EdgeCurve::Bezier {
                degree: 2,
                poles: 3
            }
            .is_straight()
        );
        assert!(!EdgeCurve::Degenerated.is_straight());
        assert!(
            !EdgeCurve::Circle {
                axis: z_axis(),
                radius: 1.0
            }
            .is_straight()
        );
    }
}
