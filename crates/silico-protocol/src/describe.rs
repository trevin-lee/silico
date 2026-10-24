//! What a shape's elements are and measure: [`Op::Describe`](crate::Op::Describe)'s reply,
//! each face's, edge's, or vertex's geometry and measures as OCCT gives them, in
//! `TopExp::MapShapes` order, a batch of one kind at a time (docs/design/m4.4-describe.md).
//! The core joins the batches, and names and orders the elements.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Axis, ElementKind, ElementRef, NORMAL_LENGTH_TOLERANCE};

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
/// for it (`Face3`), and what of it (`area`). It's formatted only when a message needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quantity {
    pub element: ElementRef,
    pub what: What,
}

/// Which of an element's values a [`Quantity`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    Area,
    Centroid,
    Normal,
    AxisOrigin,
    AxisDirection,
    Radius,
    MajorRadius,
    MinorRadius,
    HalfAngle,
    Length,
    Start,
    End,
    Middle,
    Point,
}

impl fmt::Display for What {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            What::Area => "area",
            What::Centroid => "centre of area",
            What::Normal => "normal",
            What::AxisOrigin => "axis's origin",
            What::AxisDirection => "axis's direction",
            What::Radius => "radius",
            What::MajorRadius => "major radius",
            What::MinorRadius => "minor radius",
            What::HalfAngle => "half-angle",
            What::Length => "length",
            What::Start => "start",
            What::End => "end",
            What::Middle => "middle",
            What::Point => "point",
        })
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the kernel's {}'s {}", self.element, self.what)
    }
}

impl Description {
    /// Every number it holds, with what each is, in order, its lists' first elements numbered
    /// `first` (1 for a whole description, a batch's first otherwise): one walk, so the
    /// worker's refusal and the core's check name a value alike.
    fn each(&self, first: u32, f: &mut dyn FnMut(Quantity, f64)) {
        let mut put = |kind: ElementKind, i: usize, what: What, xs: &[f64]| {
            let index = first.saturating_add(u32::try_from(i).unwrap_or(u32::MAX));
            for &x in xs {
                f(
                    Quantity {
                        element: ElementRef { kind, index },
                        what,
                    },
                    x,
                );
            }
        };
        let axis = |put: &mut dyn FnMut(What, &[f64]), axis: &Axis| {
            put(What::AxisOrigin, &axis.origin);
            put(What::AxisDirection, &axis.direction);
        };
        for (i, face) in self.faces.iter().enumerate() {
            let mut at = |what, xs: &[f64]| put(ElementKind::Face, i, what, xs);
            at(What::Area, &[face.area]);
            at(What::Centroid, &face.centroid);
            match &face.surface {
                Surface::Plane { normal } => at(What::Normal, normal),
                Surface::Cylinder { axis: a, radius } | Surface::Sphere { axis: a, radius } => {
                    axis(&mut at, a);
                    at(What::Radius, &[*radius]);
                }
                Surface::Cone {
                    axis: a,
                    radius,
                    half_angle,
                } => {
                    axis(&mut at, a);
                    at(What::Radius, &[*radius]);
                    at(What::HalfAngle, &[*half_angle]);
                }
                Surface::Torus {
                    axis: a,
                    major_radius,
                    minor_radius,
                } => {
                    axis(&mut at, a);
                    at(What::MajorRadius, &[*major_radius]);
                    at(What::MinorRadius, &[*minor_radius]);
                }
                _ => {}
            }
        }
        for (i, edge) in self.edges.iter().enumerate() {
            let mut at = |what, xs: &[f64]| put(ElementKind::Edge, i, what, xs);
            at(What::Length, &[edge.length]);
            at(What::Start, &edge.start);
            at(What::End, &edge.end);
            at(What::Middle, &edge.middle);
            match &edge.curve {
                EdgeCurve::Circle { axis: a, radius } => {
                    axis(&mut at, a);
                    at(What::Radius, &[*radius]);
                }
                EdgeCurve::Ellipse {
                    axis: a,
                    major_radius,
                    minor_radius,
                } => {
                    axis(&mut at, a);
                    at(What::MajorRadius, &[*major_radius]);
                    at(What::MinorRadius, &[*minor_radius]);
                }
                _ => {}
            }
        }
        for (i, vertex) in self.vertices.iter().enumerate() {
            put(ElementKind::Vertex, i, What::Point, vertex);
        }
    }

    /// Its first value that isn't finite, with what it is, if any, its lists' first elements
    /// numbered `first`: what a worker refuses to send.
    pub fn non_finite(&self, first: u32) -> Option<(Quantity, f64)> {
        let mut found = None;
        self.each(first, &mut |quantity, x| {
            if found.is_none() && !x.is_finite() {
                found = Some((quantity, x));
            }
        });
        found
    }

    /// Whether it can be trusted: every value finite; every direction a unit vector (within
    /// [`NORMAL_LENGTH_TOLERANCE`]); every radius above 0 (a cone's at least 0, at its apex),
    /// a cone's half-angle within a right angle, every area at least 0, and every edge's
    /// length above 0 but a degenerated one's, which is 0, its ends and middle one point.
    /// Says what's wrong, naming the value by the kernel's number for its element, its
    /// lists' first numbered `first` (1 for a whole description, a batch's first otherwise),
    /// if not.
    pub fn validate(&self, first: u32) -> Result<(), String> {
        if let Some((quantity, x)) = self.non_finite(first) {
            return Err(format!("{quantity} is {x}"));
        }
        let mut problem = None;
        let mut direction = [0.0; 3];
        let mut k = 0;
        self.each(first, &mut |quantity, x| {
            if problem.is_some() {
                return;
            }
            let at = |q: Quantity| (q.element.index - first) as usize;
            let ok = match quantity.what {
                What::Area => x >= 0.0,
                What::Radius => match quantity.element.kind {
                    ElementKind::Face
                        if matches!(self.faces[at(quantity)].surface, Surface::Cone { .. }) =>
                    {
                        x >= 0.0
                    }
                    _ => x > 0.0,
                },
                What::MajorRadius | What::MinorRadius => x > 0.0,
                What::HalfAngle => x.abs() < std::f64::consts::FRAC_PI_2,
                What::Length => match self.edges[at(quantity)].curve {
                    EdgeCurve::Degenerated => x == 0.0,
                    _ => x > 0.0,
                },
                What::Normal | What::AxisDirection => {
                    // A direction comes as its three coordinates in turn.
                    direction[k] = x;
                    k += 1;
                    if k < 3 {
                        return;
                    }
                    k = 0;
                    let [a, b, c] = direction;
                    if (libm::sqrt(a * a + b * b + c * c) - 1.0).abs() > NORMAL_LENGTH_TOLERANCE {
                        problem = Some(format!("{quantity} {direction:?} isn't a unit vector"));
                    }
                    return;
                }
                _ => true,
            };
            if !ok {
                problem = Some(format!("{quantity} is {x}"));
            }
        });
        if let Some(problem) = problem {
            return Err(problem);
        }
        for (i, edge) in self.edges.iter().enumerate() {
            if edge.curve != EdgeCurve::Degenerated {
                continue;
            }
            let quantity = |what| Quantity {
                element: ElementRef {
                    kind: ElementKind::Edge,
                    index: first.saturating_add(i as u32),
                },
                what,
            };
            for (what, at) in [(What::End, edge.end), (What::Middle, edge.middle)] {
                if at != edge.start {
                    return Err(format!(
                        "{} isn't its start, though the edge is degenerated",
                        quantity(what)
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Every one of a shape's faces, edges, and vertices described, a batch of at most
/// [`DESCRIBE_BATCH`] after another, joined: `batch(kind, first, count)` gets one (its kind's
/// total, and its elements), and `changed(kind, was, now)` is the error for a kind whose
/// total isn't what an earlier batch said. Each side, the core over a worker and the kernel
/// in its own process, asks this way.
pub fn describe_in_batches<E>(
    mut batch: impl FnMut(ElementKind, u32, u32) -> Result<(u32, Description), E>,
    changed: impl Fn(ElementKind, u32, u32) -> E,
) -> Result<Description, E> {
    let mut whole = Description {
        faces: Vec::new(),
        edges: Vec::new(),
        vertices: Vec::new(),
    };
    for kind in [ElementKind::Face, ElementKind::Edge, ElementKind::Vertex] {
        let (mut first, mut total) = (1, None);
        loop {
            let (now, d) = batch(kind, first, DESCRIBE_BATCH)?;
            if let Some(was) = total
                && was != now
            {
                return Err(changed(kind, was, now));
            }
            total = Some(now);
            let got = d.faces.len() + d.edges.len() + d.vertices.len();
            whole.faces.extend(d.faces);
            whole.edges.extend(d.edges);
            whole.vertices.extend(d.vertices);
            first = first.saturating_add(u32::try_from(got).unwrap_or(u32::MAX));
            if got == 0 || first > now {
                break;
            }
        }
    }
    Ok(whole)
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
        assert_eq!(good().validate(1), Ok(()));
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
            let e = d.validate(1).unwrap_err();
            assert!(e.starts_with(says), "{e}, not {says}");
        }
    }

    #[test]
    fn the_first_value_not_finite_is_named_whatever_element_it_is_of() {
        assert_eq!(good().non_finite(1), None);
        let first = |spoil: fn(&mut Description)| {
            let mut d = good();
            spoil(&mut d);
            let (quantity, value) = d.non_finite(1).unwrap();
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
    fn a_batchs_values_are_named_by_the_kernels_numbers_from_its_first() {
        // A batch of edges from 1001: its second is the kernel's Edge1002.
        let mut d = Description {
            faces: Vec::new(),
            edges: vec![edge(EdgeCurve::Line); 2],
            vertices: Vec::new(),
        };
        d.edges[1].length = 0.0;
        assert_eq!(
            d.validate(1001),
            Err("the kernel's Edge1002's length is 0".to_owned())
        );
        d.edges[1].length = f64::NAN;
        let (quantity, _) = d.non_finite(1001).unwrap();
        assert_eq!(quantity.to_string(), "the kernel's Edge1002's length");
    }

    #[test]
    fn a_face_of_no_area_passes_and_a_degenerated_edges_middle_must_be_its_point() {
        let mut d = good();
        d.faces[0].area = 0.0;
        assert_eq!(d.validate(1), Ok(()));
        d.edges[3].middle = [0.0, 0.0, 2.0];
        assert_eq!(
            d.validate(1),
            Err(
                "the kernel's Edge4's middle isn't its start, though the edge is degenerated"
                    .to_owned()
            )
        );
    }

    /// `n` lines, the ith `i` long: a batch from `first` of a kind with `total`.
    fn lines(first: u32, n: u32) -> Description {
        Description {
            faces: Vec::new(),
            edges: (first..first + n)
                .map(|i| EdgeDescription {
                    length: f64::from(i),
                    ..edge(EdgeCurve::Line)
                })
                .collect(),
            vertices: Vec::new(),
        }
    }

    #[test]
    fn batches_are_joined_in_order_and_a_total_that_changes_is_refused() {
        // 2,500 edges, in batches from 1, 1001, and 2001; no faces or vertices, one request
        // each that finds none.
        let mut asked = Vec::new();
        let whole = describe_in_batches(
            |kind, first, count| {
                asked.push((kind, first, count));
                Ok::<_, String>(match kind {
                    ElementKind::Edge => (2500, lines(first, count.min(2501 - first))),
                    _ => (0, lines(1, 0)),
                })
            },
            |_, _, _| "changed".to_owned(),
        )
        .unwrap();
        assert_eq!(
            asked,
            [
                (ElementKind::Face, 1, DESCRIBE_BATCH),
                (ElementKind::Edge, 1, DESCRIBE_BATCH),
                (ElementKind::Edge, 1001, DESCRIBE_BATCH),
                (ElementKind::Edge, 2001, DESCRIBE_BATCH),
                (ElementKind::Vertex, 1, DESCRIBE_BATCH),
            ]
        );
        assert_eq!(whole.edges.len(), 2500);
        assert!(
            whole
                .edges
                .iter()
                .zip(1..)
                .all(|(e, i)| e.length == f64::from(i))
        );
        // The second batch says there are 1,500: refused, saying so.
        let refused = describe_in_batches(
            |kind, first, count| {
                Ok::<_, String>(match (kind, first) {
                    (ElementKind::Edge, 1) => (2500, lines(1, count)),
                    (ElementKind::Edge, _) => (1500, lines(first, 500)),
                    _ => (0, lines(1, 0)),
                })
            },
            |kind, was, now| format!("{kind:?}: {was} then {now}"),
        );
        assert_eq!(refused.unwrap_err(), "Edge: 2500 then 1500");
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
