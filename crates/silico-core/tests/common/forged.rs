//! For the tests of `distance`'s `center_to_element` on the fake kernel: a box rebuilt on a
//! fake whose descriptions are forged, so its elements have the centres and carriers each
//! rule needs, and the pieces such a description is made of.

use std::sync::Arc;

use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    Axis, Description, EdgeCurve, EdgeDescription, ElementKind, FaceDescription, Op, Surface,
};
use silico_core::recompute::{Model, Recompute};
use silico_protocol::Reply;

use crate::model::{checked, published};

/// A document of one unit box, `bbbbbbbb`, whose description is forged.
pub const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}]}"#;

/// The name of the box `bbbbbbbb`'s element `element` (`Face1`).
pub fn name(element: &str) -> String {
    format!("bbbbbbbb.box.{element}")
}

/// Face1 of each forged box: the plane z = 0, centred (1, 0, 0), where the fake places
/// Face1.
pub fn plane_z0() -> FaceDescription {
    FaceDescription {
        surface: Surface::Plane {
            normal: [0.0, 0.0, 1.0],
            origin: [0.0; 3],
        },
        area: 1.0,
        centroid: [1.0, 0.0, 0.0],
    }
}

/// A cylinder of radius 1 whose axis runs along `direction` through `origin`.
pub fn cylinder(origin: [f64; 3], direction: [f64; 3]) -> FaceDescription {
    FaceDescription {
        surface: Surface::Cylinder {
            axis: Axis { origin, direction },
            radius: 1.0,
        },
        area: 1.0,
        centroid: origin,
    }
}

/// Face `n` of type other, centred (n, 0, 0), as the fake describes it.
pub fn other_face(n: u32) -> FaceDescription {
    FaceDescription {
        surface: Surface::Other,
        area: 1.0,
        centroid: [f64::from(n), 0.0, 0.0],
    }
}

/// A circle or arc of radius 1 about z centred `centre`, from `start` to `end` (a whole
/// circle starts and ends at one point; an arc doesn't).
pub fn circle(centre: [f64; 3], start: [f64; 3], end: [f64; 3]) -> EdgeDescription {
    EdgeDescription {
        curve: EdgeCurve::Circle {
            axis: Axis {
                origin: centre,
                direction: [0.0, 0.0, 1.0],
            },
            radius: 1.0,
        },
        length: 1.0,
        start,
        end,
        middle: start,
    }
}

/// The straight edge along x from the origin to (10, 0, 0).
pub fn line_along_x() -> EdgeDescription {
    EdgeDescription {
        curve: EdgeCurve::Line,
        length: 10.0,
        start: [0.0; 3],
        end: [10.0, 0.0, 0.0],
        middle: [5.0, 0.0, 0.0],
    }
}

/// Edge `n` of type other, from (n, 0, 0) to (n, 1, 0), as the fake describes it.
pub fn other_edge(n: u32) -> EdgeDescription {
    let x = f64::from(n);
    EdgeDescription {
        curve: EdgeCurve::Other,
        length: 1.0,
        start: [x, 0.0, 0.0],
        end: [x, 1.0, 0.0],
        middle: [x, 0.5, 0.0],
    }
}

/// Answers every describe of a box (6 faces, 12 edges, 8 vertices) on `kernel` with its
/// batch of `whole`, or, asked for every element at once, with `whole` itself.
pub fn forge(kernel: &FakeKernel, whole: Description) {
    kernel.answer_instead(move |op| match op {
        Op::DescribeAll { .. } => Some(Reply::DescribedAll(whole.clone())),
        Op::Describe {
            kind, first, count, ..
        } => {
            let from = (*first - 1) as usize;
            let slice = |n: usize| from.min(n)..(from + *count as usize).min(n);
            let mut batch = Description {
                faces: Vec::new(),
                edges: Vec::new(),
                vertices: Vec::new(),
            };
            let total = match kind {
                ElementKind::Face => {
                    batch.faces = whole.faces[slice(6)].to_vec();
                    6
                }
                ElementKind::Edge => {
                    batch.edges = whole.edges[slice(12)].to_vec();
                    12
                }
                _ => {
                    batch.vertices = whole.vertices[slice(8)].to_vec();
                    8
                }
            };
            Some(Reply::Described {
                total,
                description: batch,
            })
        }
        _ => None,
    });
}

/// The document `text` rebuilt on a fake kernel whose descriptions are `whole`'s.
pub fn forged(text: &str, whole: Description) -> (Recompute<FakeKernel>, Arc<Model>) {
    let engine = Recompute::new(FakeKernel::new(1).unwrap());
    forge(engine.kernel(), whole);
    let model = published(&engine, checked(text));
    (engine, model)
}
