//! A solid's elements, each named and measured (M4.4, docs/design/m4.4-describe.md): the
//! kernel's description of the solid ([`KernelRequest::describe`](crate::kernel::KernelRequest::describe))
//! named by its element map (M2.7), in the element list's order: faces, then edges, then
//! vertices, each by name, byte by byte (docs/design/m4.1-command-api.md, `elements`), so
//! the list doesn't follow OCCT's numbering, which an upstream edit can change where it
//! keeps the names.

use std::sync::Arc;

use crate::kernel::{Description, EdgeDescription, ElementKind, ElementRef, FaceDescription};
use crate::naming::{ElementMap, Name};
use crate::{Error, ErrorCode};

/// One of a solid's faces, edges, or vertices: its own name (the first of its names, which a
/// reference by any of them finds), which element of the solid it is, and what it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub name: Arc<Name>,
    pub element: ElementRef,
    pub geometry: Geometry,
}

/// What an element is and measures, as the kernel describes it.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    Face(FaceDescription),
    Edge(EdgeDescription),
    /// A vertex's point.
    Vertex([f64; 3]),
}

/// Each of a solid's elements, named by `map` and described by `description` (the solid's,
/// in OCCT's order), in the element list's order. Refused, as a fault, if the two don't
/// describe the same solid: a count that differs, or an element the map doesn't name.
pub fn elements(map: &ElementMap, description: &Description) -> Result<Vec<Element>, Error> {
    let mut out = Vec::new();
    for (kind, geometries) in [
        (
            ElementKind::Face,
            description
                .faces
                .iter()
                .map(|f| Geometry::Face(f.clone()))
                .collect::<Vec<_>>(),
        ),
        (
            ElementKind::Edge,
            description
                .edges
                .iter()
                .map(|e| Geometry::Edge(e.clone()))
                .collect(),
        ),
        (
            ElementKind::Vertex,
            description
                .vertices
                .iter()
                .map(|&v| Geometry::Vertex(v))
                .collect(),
        ),
    ] {
        let named = map.count(kind);
        if named != geometries.len() {
            return Err(mismatch(
                map,
                format!(
                    "the kernel describes {} {}, and its element map names {named}",
                    geometries.len(),
                    kind_words(kind).1
                ),
            ));
        }
        let mut of_kind = Vec::with_capacity(geometries.len());
        for (index, geometry) in (1..).zip(geometries) {
            let element = ElementRef { kind, index };
            let name = map.name(element).ok_or_else(|| {
                mismatch(
                    map,
                    format!(
                        "its element map doesn't name its {} {index}",
                        kind_words(kind).0
                    ),
                )
            })?;
            of_kind.push((
                name.to_string(),
                Element {
                    name,
                    element,
                    geometry,
                },
            ));
        }
        of_kind.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        out.extend(of_kind.into_iter().map(|(_, element)| element));
    }
    Ok(out)
}

/// A kind's word, for one and for many.
fn kind_words(kind: ElementKind) -> (&'static str, &'static str) {
    match kind {
        ElementKind::Face => ("face", "faces"),
        ElementKind::Edge => ("edge", "edges"),
        ElementKind::Vertex => ("vertex", "vertices"),
        ElementKind::Solid => ("solid", "solids"),
    }
}

fn mismatch(map: &ElementMap, cause: String) -> Error {
    Error::new(
        ErrorCode::Internal,
        "a solid's description and its element map don't describe the same solid",
    )
    .with_feature(map.feature().as_str())
    .with_cause(cause)
    .with_suggestion("report this with the document: it's a fault in Silico")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::FeatureId;
    use crate::kernel::{EdgeCurve, ElementCounts, Surface};
    use crate::naming::OpCode;

    fn map(faces: u32, edges: u32, vertices: u32) -> ElementMap {
        ElementMap::by_number(
            FeatureId::new("k7f2qx3m").unwrap(),
            OpCode::Cut,
            ElementCounts {
                faces,
                edges,
                vertices,
                solids: 1,
            },
        )
    }

    /// Faces of areas 1 and 2, edges whose lengths are their numbers, and a vertex at
    /// (1, 2, 3): so each name can be seen to be on its own element.
    fn description(faces: usize, edges: usize) -> Description {
        Description {
            faces: (1..=faces)
                .map(|i| FaceDescription {
                    surface: Surface::Other,
                    area: i as f64,
                    centroid: [0.0; 3],
                })
                .collect(),
            edges: (1..=edges)
                .map(|i| EdgeDescription {
                    curve: EdgeCurve::Line,
                    length: i as f64,
                    start: [0.0; 3],
                    end: [i as f64, 0.0, 0.0],
                    middle: [i as f64 / 2.0, 0.0, 0.0],
                    degenerated: false,
                })
                .collect(),
            vertices: vec![[1.0, 2.0, 3.0]],
        }
    }

    #[test]
    fn faces_edges_and_vertices_are_listed_in_turn_each_by_name_byte_by_byte() {
        let listed = elements(&map(2, 11, 1), &description(2, 11)).unwrap();
        let names: Vec<String> = listed.iter().map(|e| e.name.to_string()).collect();
        let want: Vec<String> = [
            "Face1", "Face2", "Edge1", "Edge10", "Edge11", "Edge2", "Edge3", "Edge4", "Edge5",
            "Edge6", "Edge7", "Edge8", "Edge9", "Vertex1",
        ]
        .iter()
        .map(|e| format!("k7f2qx3m.cut.{e}"))
        .collect();
        assert_eq!(names, want);
        // Each name on its own element: Edge10 is OCCT's tenth edge, 10 long.
        let edge10 = &listed[3];
        assert_eq!(edge10.element, ElementRef::edge(10));
        match &edge10.geometry {
            Geometry::Edge(e) => assert_eq!(e.length, 10.0),
            other => panic!("{other:?}"),
        }
        match &listed[1].geometry {
            Geometry::Face(f) => assert_eq!(f.area, 2.0),
            other => panic!("{other:?}"),
        }
        assert_eq!(listed[13].geometry, Geometry::Vertex([1.0, 2.0, 3.0]));
    }

    #[test]
    fn a_description_of_another_solid_is_a_fault_naming_the_feature_and_the_counts() {
        for (map, description, cause) in [
            (
                map(2, 11, 1),
                description(3, 11),
                "the kernel describes 3 faces, and its element map names 2",
            ),
            (
                map(2, 11, 1),
                description(2, 10),
                "the kernel describes 10 edges, and its element map names 11",
            ),
            (
                map(2, 11, 2),
                description(2, 11),
                "the kernel describes 1 vertices, and its element map names 2",
            ),
        ] {
            let e = elements(&map, &description).unwrap_err();
            assert_eq!(e.code(), ErrorCode::Internal);
            assert_eq!(e.feature(), Some("k7f2qx3m"));
            assert_eq!(e.cause(), Some(cause));
        }
    }
}
