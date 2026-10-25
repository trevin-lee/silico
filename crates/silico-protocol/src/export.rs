//! What the kernel worker reports to export a shape's solids as a printable mesh
//! (docs/ports/mesh-export.md): each face's triangulation, meshed as FreeCAD meshes for
//! export, and where its nodes lie on the B-rep's edges and vertices. Facts only: the core
//! joins the faces at the edges they share, checks the mesh, and writes it.

use serde::{Deserialize, Serialize};

use crate::{InvalidParameter, Refusal, format_number};

/// The finest export tolerance, in millimetres, as FreeCAD's export settings take it
/// (`DlgSettingsImportExport.ui:46-56`).
pub const MIN_EXPORT_TOLERANCE: f64 = 1e-5;

/// The coarsest export tolerance, in millimetres, as FreeCAD's export settings take it.
pub const MAX_EXPORT_TOLERANCE: f64 = 1e8;

/// FreeCAD's default export tolerance, in millimetres (`AppMeshPy.cpp:219`).
pub const DEFAULT_EXPORT_TOLERANCE: f64 = 0.1;

/// Whether `tolerance` is one an export takes: a length from [`MIN_EXPORT_TOLERANCE`] to
/// [`MAX_EXPORT_TOLERANCE`] mm. The core checks it before sending, and the worker again.
pub fn check_export_tolerance(tolerance: f64) -> Result<(), InvalidParameter> {
    if (MIN_EXPORT_TOLERANCE..=MAX_EXPORT_TOLERANCE).contains(&tolerance) {
        return Ok(());
    }
    Err(InvalidParameter {
        parameter: "tolerance",
        message: format!(
            "an export's tolerance must be a length from {} mm to {} mm, got {}",
            format_number(MIN_EXPORT_TOLERANCE),
            format_number(MAX_EXPORT_TOLERANCE),
            format_number(tolerance)
        ),
    })
}

/// How far rounding a vertex to the 32-bit floats a 3MF reader takes can move it, at most,
/// for vertices no further than `reach` mm from the origin on any axis: half a 32-bit unit
/// in the last place of `reach` on each of three axes, √3 · |`reach`| · 2⁻²⁴.
pub fn export_rounding(reach: f64) -> f64 {
    3f64.sqrt() * reach.abs() * f64::from(f32::EPSILON) / 2.0
}

/// The deflection an export of a shape that reaches `reach` mm from the origin (on any
/// axis) meshes with, for the mesh, once rounded to 32 bits, to stay within `tolerance`:
/// the tolerance less [`export_rounding`], for a tolerance [`check_export_tolerance`] takes.
/// Refused ([`Refusal::RoundingPastTolerance`]) when the rounding would take more than
/// half the tolerance, which leaves the mesher at least the other half: a part far enough
/// from the origin for the tolerance to be mostly its 32-bit rounding is better moved nearer
/// it, or exported more coarsely.
pub fn export_deflection(tolerance: f64, reach: f64) -> Result<f64, Refusal> {
    let rounding = export_rounding(reach);
    if rounding.is_finite() && rounding <= tolerance / 2.0 {
        return Ok(tolerance - rounding);
    }
    Err(Refusal::RoundingPastTolerance {
        tolerance,
        rounding,
        reach: reach.abs(),
    })
}

/// The most triangles an export's mesh may have, in all (M4.1's limits table): counted on
/// the mesh the worker made, before it's sent, and over every part a 3MF holds. Ten million
/// make about 0.9 GB of 3MF, well within a plain zip's 4 GiB.
pub const MAX_EXPORT_TRIANGLES: u64 = 10_000_000;

/// Refuses a mesh of more `triangles`, in all, than [`MAX_EXPORT_TRIANGLES`]
/// ([`Refusal::TooManyTriangles`]).
pub fn check_export_triangles(triangles: u64) -> Result<(), Refusal> {
    if triangles > MAX_EXPORT_TRIANGLES {
        return Err(Refusal::TooManyTriangles { triangles });
    }
    Ok(())
}

/// The angular deflection FreeCAD meshes with for an export tolerance `tolerance`, in
/// radians: `min(0.1, 5·tolerance + 0.005)` (`TopoShape.cpp:224-232`, `487215d2f5`). Below
/// a radius of about 80 mm at 0.1 mm, it, not the tolerance, sets how fine a curve's mesh is.
pub fn export_angle(tolerance: f64) -> f64 {
    (5.0 * tolerance + 0.005).min(0.1)
}

/// A shape's solids as OCCT meshed them for export, face by face. Every list is flat, split
/// by counts, as [`crate::Mesh`]'s are, so every value on the wire costs no more than a few
/// times its size to read.
///
/// Faces are listed as they're used: solid by solid, shell by shell, each face as its shell
/// holds it (`TopExp_Explorer`), its triangles wound counterclockwise seen from outside.
/// `INTERNAL` and `EXTERNAL` faces, which bound no change of material, are reported apart
/// (`unoriented_faces`), with no triangles.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExportMesh {
    /// The export tolerance it was meshed for, in mm, which its checks are held to.
    pub tolerance: f64,
    /// The deflection OCCT meshed with ([`export_deflection`]): the tolerance less the
    /// rounding the shape's bounding box allows for.
    pub deflection: f64,
    /// Whether OCCT was told not to raise a face's deflection for its own tolerance
    /// (`IMeshTools_Parameters::ForceFaceDeflection`): the mesh an export compares a raised
    /// face's with, never the one it writes.
    pub force_face_deflection: bool,
    /// Every face's nodes, face after face, each where its face places it.
    pub nodes: Vec<[f64; 3]>,
    /// How many of `nodes` each face has.
    pub face_nodes: Vec<u32>,
    /// Three indices into `nodes` per triangle, each a node of the triangle's own face.
    pub triangles: Vec<[u32; 3]>,
    /// How many of `triangles` each face has: 0 for a face OCCT couldn't mesh.
    pub face_triangles: Vec<u32>,
    /// Each face's number in the shape, from 1 (`Face1`), in `TopExp::MapShapes` order.
    pub face_numbers: Vec<u32>,
    /// Each face's deviation from its surface as OCCT estimates it after meshing
    /// (`Poly_Triangulation::Deflection`): a sample at its triangles, not a bound. For
    /// diagnosis only: no rule holds it to the tolerance, as it overstates a cone's 10 to
    /// 1000 times (docs/ports/mesh-export.md, rule 7).
    pub face_deflections: Vec<f64>,
    /// The shell each face is in, counted from 0 over the whole shape.
    pub face_shells: Vec<u32>,
    /// Each face's tolerance as OCCT meshes it: the largest of the face's, its edges', and
    /// its vertices' (`BRepMesh_ShapeTool::MaxFaceTolerance`). OCCT meshes a face no finer
    /// than twice it (`BRepMesh_Deflection.cxx:149-152`).
    pub face_tolerances: Vec<f64>,
    /// Faces of a solid's shell neither FORWARD nor REVERSED (INTERNAL or EXTERNAL), by
    /// number: they bound no change of material, so they're left out of the mesh.
    pub unoriented_faces: Vec<u32>,
    /// The shell each unoriented face is in, as `face_shells` counts shells. A shell of
    /// nothing else is a membrane (as OCCT's general fuse makes of a face inside a solid).
    pub unoriented_shells: Vec<u32>,
    /// The solid each shell is in, counted from 0.
    pub shell_solids: Vec<u32>,
    /// Whether each shell is its solid's outer one (`BRepClass3d::OuterShell`), not a
    /// cavity's.
    pub shell_outer: Vec<bool>,
    /// How many solids the shape has.
    pub solids: u32,
    /// Faces of the shape that are in no solid, by number: what an export would leave out.
    pub loose_faces: Vec<u32>,
    /// Each edge's use by a face: its face, counted from 0 in `face_nodes`' order.
    pub edge_faces: Vec<u32>,
    /// The edge each use is of, from 1 (`Edge1`), in `TopExp::MapShapes` order.
    pub edge_numbers: Vec<u32>,
    /// Whether the edge is degenerate (a sphere's pole, a cone's apex): one point.
    pub edge_degenerate: Vec<bool>,
    /// The vertices at the use's first and last node, from 1 (`Vertex1`).
    pub edge_vertices: Vec<[u32; 2]>,
    /// The use's deflection as OCCT gives it (`Poly_PolygonOnTriangulation::Deflection`),
    /// after any raise to the gap between its vertex and its curve's end.
    pub edge_deflections: Vec<f64>,
    /// How many of `edge_nodes` each use has.
    pub edge_node_counts: Vec<u32>,
    /// Each use's nodes, along its edge in order of the edge's parameter: indices into
    /// `nodes`, each a node of the use's face.
    pub edge_nodes: Vec<u32>,
}

impl ExportMesh {
    /// The faces OCCT couldn't mesh, by number, in order.
    pub fn unmeshed(&self) -> impl Iterator<Item = u32> + '_ {
        self.face_numbers
            .iter()
            .zip(&self.face_triangles)
            .filter(|&(_, &count)| count == 0)
            .map(|(&face, _)| face)
    }

    /// Checks that the report holds together: every list as long as its counts say, every
    /// index in range and within its own face, every number finite, and every shell in a
    /// solid. Run it before indexing a report that came from another process.
    pub fn validate(&self) -> Result<(), String> {
        let faces = self.face_nodes.len();
        for (what, len) in [
            ("triangle counts", self.face_triangles.len()),
            ("face numbers", self.face_numbers.len()),
            ("face deflections", self.face_deflections.len()),
            ("face shells", self.face_shells.len()),
            ("face tolerances", self.face_tolerances.len()),
        ] {
            if len != faces {
                return Err(format!("{len} {what} for {faces} faces"));
            }
        }
        let shells = self.shell_solids.len();
        if self.shell_outer.len() != shells {
            return Err(format!(
                "{} outer flags for {shells} shells",
                self.shell_outer.len()
            ));
        }
        if self.unoriented_shells.len() != self.unoriented_faces.len() {
            return Err(format!(
                "{} shells for {} unoriented faces",
                self.unoriented_shells.len(),
                self.unoriented_faces.len()
            ));
        }
        if let Some(&s) = self
            .face_shells
            .iter()
            .chain(&self.unoriented_shells)
            .find(|&&s| s as usize >= shells)
        {
            return Err(format!(
                "a face is in shell {s}, past the {shells} there are"
            ));
        }
        // Every solid has a shell, so a count of solids costs no more than the shells sent.
        if self.solids as usize > shells {
            return Err(format!("{} solids for {shells} shells", self.solids));
        }
        if let Some(&s) = self.shell_solids.iter().find(|&&s| s >= self.solids) {
            return Err(format!(
                "a shell is in solid {s}, past the {} there are",
                self.solids
            ));
        }
        let counted = |counts: &[u32]| counts.iter().map(|&n| u64::from(n)).sum::<u64>();
        if counted(&self.face_nodes) != self.nodes.len() as u64 {
            return Err(format!(
                "faces account for {} nodes, but there are {}",
                counted(&self.face_nodes),
                self.nodes.len()
            ));
        }
        if counted(&self.face_triangles) != self.triangles.len() as u64 {
            return Err(format!(
                "faces account for {} triangles, but there are {}",
                counted(&self.face_triangles),
                self.triangles.len()
            ));
        }
        // Each face's nodes, as a range of `nodes`.
        let ranges = self.face_ranges();
        let mut triangles = self.triangles.iter();
        for (face, (&count, range)) in self.face_triangles.iter().zip(&ranges).enumerate() {
            for t in triangles.by_ref().take(count as usize) {
                if !t.iter().all(|&i| range.contains(&i)) {
                    return Err(format!("triangle {t:?} names a node outside face {face}'s"));
                }
            }
        }
        let uses = self.edge_faces.len();
        for (what, len) in [
            ("edge numbers", self.edge_numbers.len()),
            ("degenerate flags", self.edge_degenerate.len()),
            ("edge vertices", self.edge_vertices.len()),
            ("edge deflections", self.edge_deflections.len()),
            ("edge node counts", self.edge_node_counts.len()),
        ] {
            if len != uses {
                return Err(format!("{len} {what} for {uses} edge uses"));
            }
        }
        if counted(&self.edge_node_counts) != self.edge_nodes.len() as u64 {
            return Err(format!(
                "edge uses account for {} nodes, but there are {}",
                counted(&self.edge_node_counts),
                self.edge_nodes.len()
            ));
        }
        let mut nodes = self.edge_nodes.iter();
        for (&face, &count) in self.edge_faces.iter().zip(&self.edge_node_counts) {
            let Some(range) = ranges.get(face as usize) else {
                return Err(format!(
                    "an edge use is on face {face}, past the {faces} there are"
                ));
            };
            if let Some(&n) = nodes
                .by_ref()
                .take(count as usize)
                .find(|n| !range.contains(n))
            {
                return Err(format!(
                    "an edge use on face {face} names node {n}, outside its face's"
                ));
            }
        }
        if [
            &self.face_numbers,
            &self.edge_numbers,
            &self.loose_faces,
            &self.unoriented_faces,
        ]
        .into_iter()
        .flatten()
        .any(|&n| n == 0)
            || self.edge_vertices.iter().flatten().any(|&v| v == 0)
        {
            return Err("an element is numbered 0, but elements are numbered from 1".to_owned());
        }
        if check_export_tolerance(self.tolerance).is_err() {
            return Err(format!(
                "the tolerance, {}, isn't one an export takes",
                self.tolerance
            ));
        }
        if !(self.deflection > 0.0 && self.deflection <= self.tolerance) {
            return Err(format!(
                "the deflection, {}, isn't a length within the tolerance, {}",
                self.deflection, self.tolerance
            ));
        }
        let finite = |p: &[f64; 3]| p.iter().all(|c| c.is_finite());
        if !self.nodes.iter().all(finite) {
            return Err("a coordinate isn't finite".to_owned());
        }
        if !self
            .face_deflections
            .iter()
            .chain(&self.edge_deflections)
            .chain(&self.face_tolerances)
            .all(|d| d.is_finite() && *d >= 0.0)
        {
            return Err("a deflection or a tolerance isn't a finite length".to_owned());
        }
        // An unoriented face is left out of the mesh, so it's neither a face of it nor loose.
        let meshed: std::collections::BTreeSet<u32> = self
            .face_numbers
            .iter()
            .chain(&self.loose_faces)
            .copied()
            .collect();
        if let Some(&f) = self.unoriented_faces.iter().find(|f| meshed.contains(f)) {
            return Err(format!(
                "Face{f} is both left out as unoriented and in the mesh or loose"
            ));
        }
        Ok(())
    }

    /// Each face's nodes, as a range of indices into `nodes`. Counts past `nodes` (in a
    /// report that fails [`ExportMesh::validate`]) give ranges past it.
    pub fn face_ranges(&self) -> Vec<std::ops::Range<u32>> {
        let mut start = 0u32;
        self.face_nodes
            .iter()
            .map(|&count| {
                let end = start.saturating_add(count);
                let range = start..end;
                start = end;
                range
            })
            .collect()
    }

    /// Each edge use's nodes, in order: empty where they aren't there (in a report that
    /// fails [`ExportMesh::validate`]).
    pub fn edge_uses(&self) -> impl Iterator<Item = &[u32]> {
        let mut start = 0usize;
        self.edge_node_counts.iter().map(move |&count| {
            let end = start.saturating_add(count as usize);
            let nodes = self.edge_nodes.get(start..end).unwrap_or(&[]);
            start = end;
            nodes
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_export_takes_ten_million_triangles_and_refuses_one_more() {
        assert_eq!(MAX_EXPORT_TRIANGLES, 10_000_000);
        assert!(check_export_triangles(0).is_ok());
        assert!(check_export_triangles(10_000_000).is_ok());
        assert_eq!(
            check_export_triangles(10_000_001),
            Err(Refusal::TooManyTriangles {
                triangles: 10_000_001
            })
        );
        assert!(check_export_triangles(u64::MAX).is_err());
    }

    #[test]
    fn the_tolerance_takes_freecads_range_inclusive() {
        for ok in [1e-5, 0.1, 1.0, 1e8] {
            assert!(check_export_tolerance(ok).is_ok(), "{ok}");
        }
        for bad in [
            1e-5f64.next_down(),
            1e8f64.next_up(),
            0.0,
            -0.1,
            f64::NAN,
            f64::INFINITY,
        ] {
            let invalid = check_export_tolerance(bad).unwrap_err();
            assert_eq!(invalid.parameter, "tolerance", "{bad}");
            assert!(invalid.message.contains("1e-5 mm to 1e8 mm"), "{bad}");
        }
    }

    #[test]
    fn the_deflection_is_the_tolerance_less_the_rounding_and_the_rounding_at_most_half_of_it() {
        // √3 · 2⁻²⁴ ≈ 1.0324e-7 per mm of reach: at 100 mm, 1.0324e-5.
        let per_mm = 3f64.sqrt() / 16_777_216.0;
        assert_eq!(export_rounding(100.0), 100.0 * per_mm);
        assert_eq!(export_deflection(0.1, 100.0).unwrap(), 0.1 - 100.0 * per_mm);
        assert_eq!(export_deflection(0.1, 0.0).unwrap(), 0.1);
        // 1e-3 mm 5e4 mm out: the rounding, 5.16e-3, is more than the tolerance.
        assert_eq!(
            export_deflection(1e-3, 5e4),
            Err(Refusal::RoundingPastTolerance {
                tolerance: 1e-3,
                rounding: 5e4 * per_mm,
                reach: 5e4
            })
        );
        // At the edge: a reach whose rounding is half the tolerance is taken; a little more
        // isn't. The finest tolerance, 1e-5 mm, is taken out to 48.4 mm.
        let reach = 0.05 / per_mm;
        assert!(export_deflection(0.1, reach * (1.0 - 1e-9)).is_ok());
        assert!(export_deflection(0.1, reach * (1.0 + 1e-9)).is_err());
        assert!(export_deflection(1e-5, 48.0).is_ok());
        assert!(export_deflection(1e-5, 49.0).is_err());
        // A reach is a magnitude, and one that isn't finite is refused.
        assert_eq!(
            export_deflection(0.1, -100.0),
            export_deflection(0.1, 100.0)
        );
        assert!(export_deflection(0.1, f64::INFINITY).is_err());
        assert!(export_deflection(0.1, f64::NAN).is_err());
    }

    #[test]
    fn the_angle_is_freecads_rule() {
        // min(0.1, 5·t + 0.005): 0.1 rad at FreeCAD's default 0.1 mm and above; below
        // 0.019 mm, 5·t + 0.005.
        assert_eq!(export_angle(0.1), 0.1);
        assert_eq!(export_angle(1e8), 0.1);
        assert_eq!(export_angle(0.02), 0.1);
        assert_eq!(export_angle(0.01), 5.0 * 0.01 + 0.005);
        assert_eq!(export_angle(1e-5), 5.0 * 1e-5 + 0.005);
    }
}
