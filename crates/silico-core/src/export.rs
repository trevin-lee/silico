//! A feature's solids as a printable mesh, and the mesh written as 3MF
//! (docs/ports/mesh-export.md).
//!
//! The kernel worker meshes each face at the export tolerance and reports where its
//! nodes lie on the solid's edges and vertices ([`ExportMesh`]). [`printable`] joins the
//! faces there, by the B-rep's topology rather than by distance, and checks what a slicer
//! needs, in this order, refusing a mesh that fails and naming the faces at fault:
//!
//! 1. every face meshed;
//! 2. the faces' nodes on each shared edge paired one for one (and a face's nodes at one
//!    place, as OCCT can leave inside a cone's face, one vertex: exactly, not by distance);
//! 3. closed and manifold: every edge of the mesh on exactly two triangles;
//! 4. consistently oriented: each shared edge running opposite ways in its two triangles;
//! 5. no triangle with two corners at one place in the 32-bit coordinates a reader gets,
//!    which would read them as one vertex (a triangle whose corners are apart but on a line,
//!    as OCCT leaves on a cone at times, is written: 3MF asks only that triangles SHOULD
//!    have area, core.md:588, and FreeCAD writes them);
//! 6. facing outward: each outer shell enclosing a positive volume, each cavity a negative
//!    one, and each solid a positive volume in all;
//! 7. meshed at the tolerance, less what rounding to 32 bits can move a vertex: no face
//!    meshed more coarsely for its own tolerance (OCCT meshes a face no finer than twice
//!    it, so a face it raised is compared with OCCT's mesh of it at the tolerance, and
//!    refused where its nodes differ), each edge's deflection as OCCT gives it (it raises
//!    one to the gap between a vertex and its curve), and each joined vertex's nodes.
//!
//! The export tolerance is the mesher's deflection, as FreeCAD's export takes it, and a
//! face's interior is OCCT's to meet: it holds on planes, cylinders, cones, and tori, and a
//! sphere's misses it along its seam by up to 2.1 times (docs/ports/mesh-export.md, rule 7).
//! INTERNAL and EXTERNAL faces, which bound no change of material, are left out and said.
//!
//! [`three_mf`] writes printable meshes as a 3MF archive whose bytes depend on nothing but
//! the meshes and their names: entries in a fixed order, stored, with the zip epoch as
//! every time.

use std::collections::BTreeMap;

use silico_protocol::{ElementRef, ExportMesh, FACES_NAMED};

use crate::naming::ElementMap;
use crate::{Error, ErrorCode};

/// A solid's mesh, ready to print: closed, manifold, oriented outward, no triangle with two
/// corners at one place (a triangle whose corners are apart but on a line can occur, as 3MF
/// allows), meshed at the export tolerance with its edges and joined vertices within it
/// (a face's interior is OCCT's: a sphere's misses by up to 2.1 times, as FreeCAD's does).
/// Vertices are 32-bit, as 3MF's readers take them.
#[derive(Debug, Clone, PartialEq)]
pub struct PrintableMesh {
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    volume: f64,
    left_out: Vec<String>,
}

impl PrintableMesh {
    /// The vertices, in the order the triangles first name them.
    pub fn vertices(&self) -> &[[f32; 3]] {
        &self.vertices
    }

    /// Three vertex indices per triangle, counterclockwise seen from outside.
    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }

    /// The volume the mesh encloses, in mm³, from its 32-bit vertices, summed in 64 bits:
    /// the mesh's own, not the B-rep's (a curved solid's mesh, inscribed, is a little less).
    pub fn volume(&self) -> f64 {
        self.volume
    }

    /// Faces of the solid left out, named as the feature's element map names them (as
    /// FreeCAD numbers them, `Face7`, without one): INTERNAL and EXTERNAL faces, which bound
    /// no change of material (as OCCT's own volume leaves them out, `BRepGProp.cxx:264-270`).
    pub fn left_out(&self) -> &[String] {
        &self.left_out
    }
}

/// The names of `faces` of a shape `names` is the element map of (or, without one, as
/// FreeCAD numbers them, `Face3`), in order: a STEP export's `left_out`, named as a mesh's
/// is. A map that doesn't name one of them is another shape's, Silico's bug.
pub(crate) fn face_names(names: Option<&ElementMap>, faces: &[u32]) -> Result<Vec<String>, Error> {
    if let Some(map) = names
        && let Some(&missing) = faces
            .iter()
            .find(|&&f| map.name(ElementRef::face(f)).is_none())
    {
        return Err(Error::new(
            ErrorCode::Internal,
            format!(
                "the element map given for the export names no Face{missing}: it isn't the map \
                 of the shape exported"
            ),
        )
        .with_suggestion(crate::worker::SILICO_BUG));
    }
    let namer = FaceNamer { names };
    Ok(faces.iter().map(|&f| namer.name(f)).collect())
}

/// Names faces in a refusal as the feature's element map names them (or as FreeCAD numbers
/// them, `Face3`, without one), and makes the refusal: the first [`FACES_NAMED`], and how
/// many there are.
struct FaceNamer<'a> {
    names: Option<&'a ElementMap>,
}

impl FaceNamer<'_> {
    fn name(&self, face: u32) -> String {
        let element = ElementRef::face(face);
        self.names
            .and_then(|map| map.name(element))
            .map_or_else(|| element.to_string(), |name| name.to_string())
    }

    /// `message` as an error of `code`, naming `faces` (each once, in order).
    fn refuse(
        &self,
        code: ErrorCode,
        message: String,
        faces: impl IntoIterator<Item = u32>,
        suggestion: &str,
    ) -> Error {
        let faces: std::collections::BTreeSet<u32> = faces.into_iter().collect();
        Error::new(code, message)
            .with_elements(faces.iter().take(FACES_NAMED).map(|&f| self.name(f)))
            .with_elements_total(faces.len() as u64)
            .with_suggestion(suggestion)
    }
}

/// What a refusal of a solid inside out suggests.
const INSIDE_OUT: &str = "the kernel's solid is inside out where the faces named are: check the \
                          feature makes a valid solid";

/// What a refusal of a mesh that doesn't close suggests.
const FINER: &str = "export with another tolerance, or change the part where the faces named \
                     meet: the kernel's mesh of them isn't a closed solid";

/// Whether OCCT meshed a face of `mesh` more coarsely than asked for the face's own
/// tolerance: a face whose tolerance is more than half the deflection, which OCCT raises the
/// face's deflection to twice (`BRepMesh_Deflection.cxx:149-152`, OCCT's own test). Then
/// [`printable`] asks for the unraised mesh of the same shape too, to see whether the raise
/// changed the face's nodes.
pub fn needs_unraised(mesh: &ExportMesh) -> bool {
    raised_faces(mesh).next().is_some()
}

/// The faces of `mesh` OCCT raised the deflection of, by index.
fn raised_faces(mesh: &ExportMesh) -> impl Iterator<Item = usize> + '_ {
    (0..mesh.face_tolerances.len()).filter(|&f| 2.0 * mesh.face_tolerances[f] > mesh.deflection)
}

/// Joins the faces of `mesh` at the edges they share, and checks the result is printable
/// at the tolerance it was meshed for (the module's rules, in order). `unraised` gives
/// OCCT's mesh of the same shape at the same tolerance with no face's deflection raised
/// ([`crate::kernel::KernelRequest::unraised_export_mesh`]), which a face OCCT raised
/// ([`needs_unraised`]) is compared with: it's called only then, and its error is the
/// export's. `names` names the faces in a refusal, as the feature's element map does, and
/// must name every face of the shape; without it they're numbered (`Face3`). Each solid is
/// joined on its own, so two solids that touch never share a vertex.
pub fn printable(
    mesh: &ExportMesh,
    unraised: impl FnOnce() -> Result<ExportMesh, Error>,
    names: Option<&ElementMap>,
) -> Result<PrintableMesh, Error> {
    let namer = FaceNamer { names };
    let left_out: Vec<String> = mesh
        .unoriented_faces
        .iter()
        .map(|&f| namer.name(f))
        .collect();
    if !left_out.is_empty() {
        tracing::info!(
            faces = left_out.join(", "),
            "left out of the export as bounding no material (INTERNAL or EXTERNAL)"
        );
    }
    // A refusal of a solid with faces left out says so, as they're why it may be open.
    checked(mesh, unraised, names).map_err(|error| match (error.cause(), left_out.is_empty()) {
        (None, false) => {
            let cause = format!(
                "left out of the mesh as bounding no material (INTERNAL or EXTERNAL): {}",
                left_out.join(", ")
            );
            error.with_cause(cause)
        }
        _ => error,
    })
}

/// [`printable`]'s checks, its faces left out unsaid.
fn checked(
    mesh: &ExportMesh,
    unraised: impl FnOnce() -> Result<ExportMesh, Error>,
    names: Option<&ElementMap>,
) -> Result<PrintableMesh, Error> {
    let _span = tracing::info_span!(
        "printable",
        faces = mesh.face_nodes.len(),
        triangles = mesh.triangles.len(),
        solids = mesh.solids
    )
    .entered();
    // A report that doesn't hold together can't be indexed: one from the worker is checked
    // as it's read (`KernelRequest::export_mesh`), and any other here.
    held_together(mesh, "the export mesh")?;
    let tolerance = mesh.tolerance;
    let faces = FaceNamer { names };
    // A map that doesn't name a face of the shape is another shape's: the error would name
    // the face by a number no document holds.
    if let Some(map) = names
        && let Some(&missing) = mesh
            .face_numbers
            .iter()
            .chain(&mesh.loose_faces)
            .chain(&mesh.unoriented_faces)
            .find(|&&f| map.name(ElementRef::face(f)).is_none())
    {
        return Err(Error::new(
            ErrorCode::Internal,
            format!(
                "the element map given for the export names no Face{missing}: it isn't the map \
                 of the shape exported"
            ),
        )
        .with_suggestion(crate::worker::SILICO_BUG));
    }
    if mesh.solids == 0 {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            "the shape has no solid to export: a printable mesh is a solid's",
        )
        .with_suggestion("export a feature that makes a solid"));
    }
    if !mesh.loose_faces.is_empty() {
        return Err(faces.refuse(
            ErrorCode::InvalidInput,
            format!(
                "{} of the shape's faces are in no solid, and an export would leave them out",
                mesh.loose_faces.len()
            ),
            mesh.loose_faces.iter().copied(),
            "export a feature whose faces all bound a solid",
        ));
    }
    // 1. Every face meshed.
    let unmeshed: Vec<u32> = mesh.unmeshed().collect();
    if !unmeshed.is_empty() {
        // M4.1's words (`export`): a face, or so many.
        let (have, faces_said) = match unmeshed.len() {
            1 => ("a face has".to_owned(), "the face"),
            n => (format!("{n} faces have"), "the faces"),
        };
        return Err(faces
            .refuse(
                ErrorCode::InvalidResult,
                format!("the export mesh isn't closed: {have} no triangles"),
                unmeshed,
                &format!(
                    "this solid can't be exported until the kernel meshes {faces_said}: there's \
                     no workaround in the request"
                ),
            )
            .with_cause(format!("OCCT made no triangles for {faces_said}")));
    }

    let ranges = mesh.face_ranges();
    let solid_of_face = |face: usize| -> u32 { mesh.shell_solids[mesh.face_shells[face] as usize] };
    // Each node's face, to name it.
    let mut node_face = vec![0usize; mesh.nodes.len()];
    for (face, range) in ranges.iter().enumerate() {
        for n in range.clone() {
            node_face[n as usize] = face;
        }
    }
    let number = |face: usize| mesh.face_numbers[face];

    // 2. Joined at the edges the faces share. Each edge's uses, and each vertex, per solid.
    let mut joined = Joined::new(mesh.nodes.len());
    let mut edges: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    for (k, &face) in mesh.edge_faces.iter().enumerate() {
        let key = (solid_of_face(face as usize), mesh.edge_numbers[k]);
        edges.entry(key).or_default().push(k);
    }
    let uses: Vec<&[u32]> = mesh.edge_uses().collect();
    let mut at_vertex: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    let mut unpaired: Vec<u32> = Vec::new();
    for (&(solid, _), group) in &edges {
        // The first use with nodes is the one the others pair with.
        let first = group.iter().map(|&k| uses[k]).find(|u| !u.is_empty());
        let mut paired = true;
        for &k in group {
            let nodes = uses[k];
            if mesh.edge_degenerate[k] {
                // A pole: every node of its polygon is its one vertex.
                for &n in nodes {
                    joined.join_vertex(&mut at_vertex, (solid, mesh.edge_vertices[k][0]), n);
                }
                continue;
            }
            match (nodes, first) {
                (&[], _) | (_, None) => paired = false,
                (nodes, Some(first)) if nodes.len() != first.len() => paired = false,
                (nodes, Some(first)) => {
                    for (&a, &b) in nodes.iter().zip(first) {
                        joined.union(a, b);
                    }
                    let [v0, v1] = mesh.edge_vertices[k];
                    if let (Some(&start), Some(&end)) = (nodes.first(), nodes.last()) {
                        joined.join_vertex(&mut at_vertex, (solid, v0), start);
                        joined.join_vertex(&mut at_vertex, (solid, v1), end);
                    }
                }
            }
        }
        if !paired {
            // Every face on the edge: they don't meet in the mesh there.
            unpaired.extend(group.iter().map(|&k| number(mesh.edge_faces[k] as usize)));
        }
    }
    // A face's own nodes at one place are one vertex (OCCT leaves two inside a cone's face
    // at times, NIST's CTC 01): joined only where their coordinates are the same, so it's
    // exact, and within one face, so two faces never meet but at their edges. Triangles they
    // flatten are left out below, as FreeCAD's merge leaves them (`BRepMesh.cpp:275-279`).
    for range in &ranges {
        let mut at: BTreeMap<[u64; 3], u32> = BTreeMap::new();
        for n in range.clone() {
            let place = mesh.nodes[n as usize].map(|c| (c + 0.0).to_bits());
            let first = *at.entry(place).or_insert(n);
            joined.union(first, n);
        }
    }
    if !unpaired.is_empty() {
        return Err(faces.refuse(
            ErrorCode::InvalidResult,
            "the faces named don't meet in the mesh: the kernel meshed an edge they share \
             differently on each"
                .to_owned(),
            unpaired,
            FINER,
        ));
    }

    // The joined vertices, numbered as the triangles first name them, each where its
    // lowest node is (the root it's joined under); and the triangles, less those whose
    // corners join into one vertex (a pole's), whose remaining edges cancel.
    let mut vertex_of: BTreeMap<u32, u32> = BTreeMap::new();
    let mut positions: Vec<[f64; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut triangle_face: Vec<usize> = Vec::new();
    let mut triangle_shell: Vec<u32> = Vec::new();
    let mut start = 0usize;
    for (face, &count) in mesh.face_triangles.iter().enumerate() {
        let end = start + count as usize;
        for t in &mesh.triangles[start..end] {
            let roots = t.map(|n| joined.find(n));
            if roots[0] == roots[1] || roots[1] == roots[2] || roots[0] == roots[2] {
                continue;
            }
            let corners = roots.map(|root| {
                *vertex_of.entry(root).or_insert_with(|| {
                    positions.push(mesh.nodes[root as usize]);
                    (positions.len() - 1) as u32
                })
            });
            triangles.push(corners);
            triangle_face.push(face);
            triangle_shell.push(mesh.face_shells[face]);
        }
        start = end;
    }

    // 3 and 4. Each edge of the mesh on exactly two triangles, running opposite ways.
    let mut sides: BTreeMap<(u32, u32), Vec<(bool, usize)>> = BTreeMap::new();
    for (t, corners) in triangles.iter().enumerate() {
        for i in 0..3 {
            let (a, b) = (corners[i], corners[(i + 1) % 3]);
            sides
                .entry((a.min(b), a.max(b)))
                .or_default()
                .push((a < b, triangle_face[t]));
        }
    }
    let open: Vec<u32> = sides
        .values()
        .filter(|s| s.len() == 1)
        .flat_map(|s| s.iter().map(|&(_, f)| number(f)))
        .collect();
    if !open.is_empty() {
        return Err(faces.refuse(
            ErrorCode::InvalidResult,
            "the mesh is open: an edge of the faces named is on one triangle only".to_owned(),
            open,
            FINER,
        ));
    }
    let shared: Vec<u32> = sides
        .values()
        .filter(|s| s.len() > 2)
        .flat_map(|s| s.iter().map(|&(_, f)| number(f)))
        .collect();
    if !shared.is_empty() {
        return Err(faces.refuse(
            ErrorCode::InvalidResult,
            "the mesh isn't manifold: an edge of the faces named is on more than two \
             triangles, where the solid touches itself"
                .to_owned(),
            shared,
            "change the part so the faces named don't meet along an edge from both sides",
        ));
    }
    let flipped: Vec<u32> = sides
        .values()
        .filter(|s| s[0].0 == s[1].0)
        .flat_map(|s| s.iter().map(|&(_, f)| number(f)))
        .collect();
    if !flipped.is_empty() {
        return Err(faces.refuse(
            ErrorCode::InvalidResult,
            "the faces named are oriented inconsistently: an edge they share runs the same \
             way in both its triangles"
                .to_owned(),
            flipped,
            FINER,
        ));
    }

    // The vertices as a reader gets them: 32-bit, and 0 for -0. First, the worker's own
    // rule on the vertices the mesh has: the rounding may take at most half the tolerance
    // (and a vertex past 32 bits takes it all), or the checks below would be of other
    // vertices than are written.
    let reach = positions
        .iter()
        .flatten()
        .fold(0.0f64, |m, c| m.max(c.abs()));
    let allowed = match silico_protocol::export_deflection(tolerance, reach) {
        Ok(allowed) => allowed,
        Err(refusal) => {
            return Err(crate::worker::failure_error(
                "ExportMesh",
                silico_protocol::Failure::new(refusal),
            ));
        }
    };
    let vertices: Vec<[f32; 3]> = positions
        .iter()
        .map(|p| p.map(|c| (c as f32) + 0.0))
        .collect();
    let wide = |v: u32| vertices[v as usize].map(f64::from);

    // 5. No triangle with two corners at one place in the 32-bit coordinates.
    let flat: Vec<u32> = (0..triangles.len())
        .filter(|&t| {
            let [a, b, c] = triangles[t].map(|v| vertices[v as usize]);
            a == b || b == c || a == c
        })
        .map(|t| number(triangle_face[t]))
        .collect();
    if !flat.is_empty() {
        return Err(faces.refuse(
            ErrorCode::InvalidResult,
            "a triangle of the faces named has two corners at one place once rounded to the \
             32 bits a 3MF reader takes, which would read them as one vertex"
                .to_owned(),
            flat,
            "move the part nearer the origin, or make the faces named larger: 32-bit \
             coordinates can't hold their triangles' corners apart where they are",
        ));
    }

    // 6. Facing outward: each outer shell encloses a positive volume, each cavity a
    // negative one, and each solid a positive volume in all. A shell with no triangles
    // encloses none, and is refused with the rest; so is a volume that isn't a number.
    let mut shell_volumes = vec![0.0f64; mesh.shell_solids.len()];
    for (t, corners) in triangles.iter().enumerate() {
        let [a, b, c] = corners.map(wide);
        shell_volumes[triangle_shell[t] as usize] += dot(a, cross(b, c)) / 6.0;
    }
    let shell_faces = |shell: usize| {
        (0..mesh.face_shells.len())
            .filter(move |&f| mesh.face_shells[f] as usize == shell)
            .map(number)
    };
    // A shell of unoriented faces only is a membrane, no boundary: it encloses nothing, and
    // isn't held to.
    let shells_with = |of: &[u32]| {
        let mut with = vec![false; mesh.shell_solids.len()];
        for &s in of {
            with[s as usize] = true;
        }
        with
    };
    let (oriented, unoriented) = (
        shells_with(&mesh.face_shells),
        shells_with(&mesh.unoriented_shells),
    );
    let membrane = |shell: usize| !oriented[shell] && unoriented[shell];
    let inward: Vec<u32> = (0..shell_volumes.len())
        .filter(|&shell| !membrane(shell))
        .filter(|&shell| {
            let volume = shell_volumes[shell];
            !encloses(volume, mesh.shell_outer[shell])
        })
        .flat_map(shell_faces)
        .collect();
    let unbounded = (0..shell_volumes.len()).any(|s| !membrane(s) && shell_volumes[s] == 0.0);
    if unbounded || !inward.is_empty() {
        return Err(faces.refuse(
            ErrorCode::InvalidResult,
            if unbounded {
                "a shell of the solid encloses no volume: its mesh has no triangles, or none \
                 that enclose anything"
                    .to_owned()
            } else {
                "the faces named face inward: their shell encloses the wrong side".to_owned()
            },
            inward,
            INSIDE_OUT,
        ));
    }
    let mut solid_volumes = vec![0.0f64; mesh.solids as usize];
    for (shell, &solid) in mesh.shell_solids.iter().enumerate() {
        solid_volumes[solid as usize] += shell_volumes[shell];
    }
    let hollow: Vec<u32> = (0..mesh.shell_solids.len())
        .filter(|&shell| !encloses(solid_volumes[mesh.shell_solids[shell] as usize], true))
        .flat_map(shell_faces)
        .collect();
    if !hollow.is_empty() || solid_volumes.iter().any(|&v| !encloses(v, true)) {
        return Err(faces.refuse(
            ErrorCode::InvalidResult,
            "a solid encloses no volume in all: its cavities enclose as much as its outer \
             shell, or it has no shell"
                .to_owned(),
            hollow,
            INSIDE_OUT,
        ));
    }

    // 7. Meshed at the tolerance, less what rounding to 32 bits moves a vertex
    // (the worker meshed with the tolerance less the most its bounding box allows). First,
    // OCCT's own raise: it meshes a face no finer than twice its tolerance (the largest of
    // the face's, its edges', and its vertices', `BRepMesh_Deflection.cxx:149-152`). Where
    // it raised a face, the face is compared with OCCT's mesh of it unraised: the raise
    // changes nothing on a plane or a cylinder (no nodes inside them), and often nothing on
    // a sphere, cone, or torus (a step count rounded the same), so a face is refused only
    // where its nodes differ. Its triangles may differ where its nodes don't: the face's
    // deflection also sets the scale a cylinder is triangulated in (`myDu`, through
    // `computeDelta`), so its quads' diagonals can fall the other way, and the mesh is as far
    // from the surface either way; the nodes are what the deflection places.
    let raised: Vec<usize> = raised_faces(mesh).collect();
    if !raised.is_empty() {
        let unraised = unraised()?;
        same_shape(mesh, &unraised)?;
        let unraised_ranges = unraised.face_ranges();
        let places = |m: &ExportMesh, range: &std::ops::Range<u32>| {
            let mut places: Vec<[u64; 3]> = m.nodes[range.start as usize..range.end as usize]
                .iter()
                .map(|p| p.map(|c| (c + 0.0).to_bits()))
                .collect();
            places.sort_unstable();
            places
        };
        // By their bits, -0 as 0 (one place, as joined above), in any order.
        let (changed, same): (Vec<usize>, Vec<usize>) = raised
            .into_iter()
            .partition(|&f| places(mesh, &ranges[f]) != places(&unraised, &unraised_ranges[f]));
        if !same.is_empty() {
            let named: Vec<String> = same.iter().map(|&f| faces.name(number(f))).collect();
            tracing::info!(
                faces = named.join(", "),
                "OCCT raised the faces' deflection for their own tolerance and placed the same \
                 nodes as unraised"
            );
        }
        if !changed.is_empty() {
            let loosest = changed
                .iter()
                .map(|&f| mesh.face_tolerances[f])
                .fold(0.0f64, f64::max);
            return Err(faces
                .refuse(
                    ErrorCode::InvalidResult,
                    format!(
                        "the kernel meshed the faces named at {raised} mm, not the {asked} mm \
                         asked, and placed other nodes than it would at {asked}: the part's \
                         geometry there is only exact to {loosest} mm, and OCCT meshes a face \
                         no finer than twice its tolerance",
                        raised = silico_protocol::format_number(2.0 * loosest),
                        asked = silico_protocol::format_number(tolerance),
                        loosest = silico_protocol::format_number(loosest)
                    ),
                    changed.into_iter().map(number),
                    &match unraising_tolerance(2.0 * loosest, mesh.tolerance, mesh.deflection) {
                        Some(at) => format!(
                            "export with a tolerance of at least {} mm, or rebuild the part so \
                             the faces named meet more precisely",
                            silico_protocol::format_number(at)
                        ),
                        None => "rebuild the part so the faces named meet more precisely: no \
                                 tolerance an export takes is coarse enough for them"
                            .to_owned(),
                    },
                )
                .with_parameter("tolerance"));
        }
    }
    let mut far: Vec<u32> = (0..mesh.edge_faces.len())
        .filter(|&k| mesh.edge_deflections[k] > allowed)
        .map(|k| number(mesh.edge_faces[k] as usize))
        .collect();
    // Each joined vertex's nodes within the tolerance of the one it's placed at.
    for n in 0..mesh.nodes.len() as u32 {
        let first = joined.find(n);
        if first != n && distance(mesh.nodes[n as usize], mesh.nodes[first as usize]) > allowed {
            far.push(number(node_face[n as usize]));
            far.push(number(node_face[first as usize]));
        }
    }
    if !far.is_empty() {
        return Err(faces.refuse(
            ErrorCode::InvalidResult,
            format!(
                "the mesh of the faces named is further than the tolerance, {} mm, from the \
                 part once rounding to 32 bits is allowed for: the kernel meshed an edge of \
                 theirs more coarsely than asked, or joined vertices further apart",
                silico_protocol::format_number(tolerance)
            ),
            far,
            "export with a coarser tolerance: the kernel can't mesh the edges of the faces \
             named as finely as asked (their vertices lie off their curves by more)",
        ));
    }

    let volume = solid_volumes.iter().sum();
    Ok(PrintableMesh {
        vertices,
        triangles,
        volume,
        left_out: mesh
            .unoriented_faces
            .iter()
            .map(|&f| faces.name(f))
            .collect(),
    })
}

/// `mesh` refused, as `what`, if it doesn't hold together.
fn held_together(mesh: &ExportMesh, what: &str) -> Result<(), Error> {
    mesh.validate().map_err(|problem| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("{what} doesn't hold together"),
        )
        .with_cause(problem)
        .with_suggestion("export the mesh the kernel worker reports, as it reports it")
    })
}

/// Checks `unraised` is OCCT's unraised mesh of the shape `mesh` is of: one that holds
/// together, meshed unraised at the same tolerance and deflection, of the same faces.
/// Another's is a bug of the caller's.
fn same_shape(mesh: &ExportMesh, unraised: &ExportMesh) -> Result<(), Error> {
    held_together(unraised, "the unraised export mesh")?;
    if !unraised.force_face_deflection
        || unraised.tolerance.to_bits() != mesh.tolerance.to_bits()
        || unraised.deflection.to_bits() != mesh.deflection.to_bits()
        || unraised.face_numbers != mesh.face_numbers
    {
        return Err(Error::new(
            ErrorCode::Internal,
            "OCCT meshed a face of the export more coarsely for its own tolerance, and the \
             unraised mesh given to compare it with is another's",
        )
        .with_suggestion(crate::worker::SILICO_BUG));
    }
    Ok(())
}

/// The least tolerance in whole nanometres (1e-6 mm, a length a document holds) at which
/// the worker meshes with a deflection of at least `least`, so OCCT raises no face whose
/// tolerance is half of `least` or less; none if it's past the largest an export takes.
/// The worker meshes with the tolerance less the rounding its bounding box allows for, the
/// same at every tolerance, and known here as `tolerance` less `deflection`: that is the
/// rounding to within an ulp of the tolerance, no more than an ulp of the one suggested
/// (which is past it), and the worker's own subtraction rounds by half an ulp of the
/// tolerance it's given, so two ulps of the suggestion are allowed for both (a model of the
/// arithmetic needs one, over 1.2 million cases at nanometre steps). An ulp is far under a
/// nanometre at every tolerance an export takes (1.5e-8 mm at 1e8), so a nanometre more
/// always clears them.
fn unraising_tolerance(least: f64, tolerance: f64, deflection: f64) -> Option<f64> {
    let margin = tolerance - deflection;
    let ulp = |x: f64| x.next_up() - x;
    let enough = |at: f64| at - margin - 2.0 * ulp(at) >= least;
    let mut nanometres = ((least + margin) * 1e6).ceil();
    if !enough(nanometres / 1e6) {
        nanometres += 1.0;
    }
    let at = nanometres / 1e6;
    (enough(at) && silico_protocol::check_export_tolerance(at).is_ok()).then_some(at)
}

/// Whether a shell's or solid's signed `volume` is enclosed the right way: positive for an
/// outer shell or a solid, negative for a cavity. Not a number is neither.
fn encloses(volume: f64, outer: bool) -> bool {
    if outer { volume > 0.0 } else { volume < 0.0 }
}

/// Nodes joined into the vertices they share: a union-find over node indices.
struct Joined {
    parent: Vec<u32>,
}

impl Joined {
    fn new(nodes: usize) -> Self {
        Self {
            parent: (0..nodes as u32).collect(),
        }
    }

    fn find(&mut self, mut n: u32) -> u32 {
        while self.parent[n as usize] != n {
            let up = self.parent[self.parent[n as usize] as usize];
            self.parent[n as usize] = up;
            n = up;
        }
        n
    }

    /// Joins `a` and `b`, the lower root the root, so each vertex's root is its lowest node
    /// and the result doesn't depend on the order joined.
    fn union(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        let (low, high) = (a.min(b), a.max(b));
        self.parent[high as usize] = low;
    }

    /// Joins `node` with the nodes already at `vertex` (a solid's vertex, by number).
    fn join_vertex(&mut self, at: &mut BTreeMap<(u32, u32), u32>, vertex: (u32, u32), node: u32) {
        let first = *at.entry(vertex).or_insert(node);
        self.union(first, node);
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = sub(a, b);
    dot(d, d).sqrt()
}

/// One object of a 3MF file: a feature's printable mesh, named by the feature's name, its
/// ID as the part number.
#[derive(Debug, Clone, Copy)]
pub struct Object<'a> {
    pub name: &'a str,
    pub part_number: &'a str,
    pub mesh: &'a PrintableMesh,
}

/// The largest archive [`three_mf`] writes: a plain zip's limit, 4 GiB less a byte, since
/// ZIP64 is left out.
pub(crate) const MAX_ARCHIVE: u64 = u32::MAX as u64;

/// Nothing, if `triangles` in all are few enough for one file
/// ([`silico_protocol::MAX_EXPORT_TRIANGLES`]); otherwise the kernel's refusal of a mesh
/// that large, naming the tolerance, as [`three_mf`] refuses its objects.
pub(crate) fn within_triangles(triangles: u64) -> Result<(), Error> {
    silico_protocol::check_export_triangles(triangles, silico_protocol::MAX_EXPORT_TRIANGLES)
        .map_err(|refusal| {
            crate::worker::failure_error("ExportMesh", silico_protocol::Failure::new(refusal))
        })
}

/// `objects` as a 3MF archive (3MF Core 1.4.0): each a `type="model"` object in
/// millimetres, in the order given, with one build item each, its vertices in document
/// coordinates. Its bytes depend on the objects alone: the entries are
/// `[Content_Types].xml`, `_rels/.rels`, and `3D/3dmodel.model`, in that order, stored, each
/// with the zip epoch (1980-01-01 00:00) as its time. A name or part number XML can't hold
/// (a control character other than tab, line feed, and carriage return) is refused, naming
/// the object; so are objects of more than [`silico_protocol::MAX_EXPORT_TRIANGLES`]
/// triangles in all, and an archive past a plain zip's 4 GiB, each naming the tolerance.
pub fn three_mf(objects: &[Object<'_>]) -> Result<Vec<u8>, Error> {
    let _span = tracing::info_span!("three_mf", objects = objects.len()).entered();
    if objects.is_empty() {
        // As FreeCAD refuses an export with nothing to mesh (04005b5e45).
        return Err(Error::new(
            ErrorCode::InvalidInput,
            "a 3MF file needs at least one object, and there are none to write",
        )
        .with_suggestion("export at least one feature that makes a solid"));
    }
    within_triangles(
        objects
            .iter()
            .map(|object| object.mesh.triangles.len() as u64)
            .sum(),
    )?;
    let mut model = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" \
         xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n \
         <metadata name=\"Application\">Silico</metadata>\n <resources>\n",
    );
    for (i, object) in objects.iter().enumerate() {
        let id = i + 1;
        let name =
            attribute(object.name).ok_or_else(|| unwritable(object.name, "name", object.name))?;
        let part = attribute(object.part_number)
            .ok_or_else(|| unwritable(object.name, "part number", object.part_number))?;
        model.push_str(&format!(
            "  <object id=\"{id}\" type=\"model\" name=\"{name}\" partnumber=\"{part}\">\n   \
             <mesh>\n    <vertices>\n"
        ));
        for v in &object.mesh.vertices {
            model.push_str(&format!(
                "     <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>\n",
                v[0], v[1], v[2]
            ));
        }
        model.push_str("    </vertices>\n    <triangles>\n");
        for t in &object.mesh.triangles {
            model.push_str(&format!(
                "     <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>\n",
                t[0], t[1], t[2]
            ));
        }
        model.push_str("    </triangles>\n   </mesh>\n  </object>\n");
    }
    model.push_str(" </resources>\n <build>\n");
    for i in 1..=objects.len() {
        model.push_str(&format!("  <item objectid=\"{i}\"/>\n"));
    }
    model.push_str(" </build>\n</model>\n");
    let entries: [(&str, &[u8]); 3] = [
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", RELATIONSHIPS.as_bytes()),
        ("3D/3dmodel.model", model.as_bytes()),
    ];
    zip(&entries)
}

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types \
     xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n <Default \
     Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n \
     <Default Extension=\"model\" \
     ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\n</Types>\n";

const RELATIONSHIPS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships \
     xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n <Relationship \
     Target=\"/3D/3dmodel.model\" Id=\"rel0\" \
     Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/>\n</Relationships>\n";

/// The refusal of the object `name`'s `what`, `text`, which a 3MF file can't hold, naming
/// the object.
fn unwritable(name: &str, what: &str, text: &str) -> Error {
    Error::new(ErrorCode::InvalidInput, unholdable(what, text))
        .with_element(crate::references::cut_short(name))
        .with_suggestion(UNHOLDABLE_ADVICE)
}

/// Whether a 3MF file can hold `text` as a name: XML 1.0 can hold every character of it,
/// as [`three_mf`] writes it.
pub(crate) fn holds_in_3mf(text: &str) -> bool {
    attribute(text).is_some()
}

/// What's said of `text`, the `what` of an object, that a 3MF file can't hold
/// ([`holds_in_3mf`]), by [`three_mf`] and by the `export` command alike.
pub(crate) fn unholdable(what: &str, text: &str) -> String {
    format!(
        "the {what}, {}, has a character a 3MF file can't hold (XML 1.0 holds no control \
         character but tab, line feed, and carriage return, and neither U+FFFE nor U+FFFF)",
        crate::document::json::shown(text)
    )
}

/// The advice for a name a 3MF file can't hold.
pub(crate) const UNHOLDABLE_ADVICE: &str = "rename the feature without that character";

/// `text` escaped for an XML attribute, or none if XML 1.0 can't hold a character of it.
/// Tab, line feed, and carriage return are written as references, which a parser would
/// otherwise read as spaces.
fn attribute(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

/// The zip epoch, 1980-01-01, as a DOS date: day 1 in bits 0-4, month 1 in bits 5-8, and
/// year 0 (1980) above them.
const DOS_DATE: u16 = 0x0021;

/// `entries` as a zip archive, each stored (not compressed), with no extra fields, no data
/// descriptors, no comment, and the zip epoch as every time. Refused if it would pass a
/// plain zip's 4 GiB.
fn zip(entries: &[(&str, &[u8])]) -> Result<Vec<u8>, Error> {
    let total = archive_size(entries);
    within_archive_limit(total)?;
    let mut out = Vec::with_capacity(total as usize);
    let mut central = Vec::new();
    for (name, data) in entries {
        // Within the archive's limit, checked above, so each fits in 32 bits.
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        let name_len = name.len() as u16;
        // The local header.
        put32(&mut out, 0x0403_4b50);
        put16(&mut out, 10); // version needed: 1.0, stored
        put16(&mut out, 0); // flags
        put16(&mut out, 0); // stored
        put16(&mut out, 0); // time: 00:00
        put16(&mut out, DOS_DATE);
        put32(&mut out, crc);
        put32(&mut out, size);
        put32(&mut out, size);
        put16(&mut out, name_len);
        put16(&mut out, 0); // no extra field
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        // Its central directory entry.
        put32(&mut central, 0x0201_4b50);
        put16(&mut central, 20); // made by: MS-DOS, 2.0
        put16(&mut central, 10);
        put16(&mut central, 0);
        put16(&mut central, 0);
        put16(&mut central, 0);
        put16(&mut central, DOS_DATE);
        put32(&mut central, crc);
        put32(&mut central, size);
        put32(&mut central, size);
        put16(&mut central, name_len);
        put16(&mut central, 0); // no extra field
        put16(&mut central, 0); // no comment
        put16(&mut central, 0); // disk 0
        put16(&mut central, 0); // internal attributes
        put32(&mut central, 0); // external attributes
        put32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let directory_offset = out.len() as u32;
    let directory_size = central.len() as u32;
    out.extend_from_slice(&central);
    let count = entries.len() as u16;
    put32(&mut out, 0x0605_4b50);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, count);
    put16(&mut out, count);
    put32(&mut out, directory_size);
    put32(&mut out, directory_offset);
    put16(&mut out, 0); // no comment
    Ok(out)
}

/// How long [`zip`] makes the archive of `entries`: per entry, a 30-byte local header and a
/// 46-byte central directory entry, each with the name, and the data; and the 22-byte end
/// of the central directory.
fn archive_size(entries: &[(&str, &[u8])]) -> u64 {
    entries
        .iter()
        .map(|(name, data)| 76 + 2 * name.len() as u64 + data.len() as u64)
        .sum::<u64>()
        + 22
}

/// Refuses an archive of `size` bytes past a plain zip's limit: a request's limit, as the
/// triangles' is (which keeps an export well within it), so invalid input, naming the
/// tolerance.
fn within_archive_limit(size: u64) -> Result<(), Error> {
    if size > MAX_ARCHIVE {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("the 3MF archive would be {size} bytes, past a zip's 4 GiB"),
        )
        .with_parameter("tolerance")
        .with_suggestion("export with a coarser tolerance, or fewer parts at once"));
    }
    Ok(())
}

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// The CRC-32 zip uses (IEEE 802.3, reflected, polynomial 0xEDB88320).
fn crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_million_triangles_in_all_are_taken_and_one_more_refused_naming_the_tolerance() {
        // The tools' limits table: 10,000,000 (`MAX_EXPORT_TRIANGLES`), which the command
        // counts as each solid's mesh is taken.
        assert!(within_triangles(10_000_000).is_ok());
        let e = within_triangles(10_000_001).unwrap_err();
        assert_eq!(
            (e.code(), e.parameter()),
            (ErrorCode::InvalidInput, Some("tolerance"))
        );
        assert!(e.message().contains("10000001"), "{e:?}");
    }

    #[test]
    fn crc32_is_zips() {
        // The standard check value of CRC-32/ISO-HDLC, and the empty input's.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
    }

    #[test]
    fn an_archive_is_as_long_as_counted_by_hand() {
        // 30 + 46 bytes of headers per entry, the name in each, the data, and 22 for the end.
        let one: [(&str, &[u8]); 1] = [("abc", b"hello")];
        assert_eq!(archive_size(&one), 30 + 46 + 2 * 3 + 5 + 22);
        assert_eq!(zip(&one).unwrap().len() as u64, archive_size(&one));
        let two: [(&str, &[u8]); 2] = [("abc", b"hello"), ("_rels/.rels", b"")];
        assert_eq!(archive_size(&two), 76 + 6 + 5 + 76 + 22 + 22);
        assert_eq!(zip(&two).unwrap().len() as u64, archive_size(&two));
        // Every time is the zip epoch: 00:00 and 1980-01-01, 0x0021, in each local header
        // (bytes 10-13) and its central directory entry (bytes 12-15).
        let archive = zip(&one).unwrap();
        assert_eq!(archive[10..14], [0, 0, 0x21, 0]);
        let central = 30 + 3 + 5;
        assert_eq!(archive[central..central + 4], 0x0201_4b50u32.to_le_bytes());
        assert_eq!(archive[central + 12..central + 16], [0, 0, 0x21, 0]);
    }

    #[test]
    fn an_archive_up_to_4_gib_less_a_byte_is_written_and_a_longer_one_refused() {
        assert!(within_archive_limit(u64::from(u32::MAX)).is_ok());
        let error = within_archive_limit(u64::from(u32::MAX) + 1).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
        assert_eq!(error.parameter(), Some("tolerance"), "{error}");
        assert!(error.message().contains("4294967296 bytes"), "{error}");
        assert!(error.suggestion().is_some(), "{error}");
    }

    #[test]
    fn a_3mf_of_more_than_ten_million_triangles_in_all_is_refused_naming_the_tolerance() {
        // Three parts of 3,333,334 triangles each: 10,000,002 in all, though each is under.
        let mesh = PrintableMesh {
            vertices: Vec::new(),
            triangles: vec![[0, 1, 2]; 3_333_334],
            volume: 1.0,
            left_out: Vec::new(),
        };
        let object = Object {
            name: "Part",
            part_number: "f1",
            mesh: &mesh,
        };
        let error = three_mf(&[object; 3]).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
        assert_eq!(error.parameter(), Some("tolerance"), "{error}");
        assert!(error.message().contains("10000002 triangles"), "{error}");
        assert!(error.message().contains("10000000"), "{error}");
        assert!(
            error.suggestion().is_some_and(|s| s.contains("coarser")),
            "{error}"
        );
    }

    #[test]
    fn attributes_are_escaped_and_control_characters_refused() {
        assert_eq!(
            attribute("a&b<c>d\"e'f").as_deref(),
            Some("a&amp;b&lt;c&gt;d&quot;e&apos;f")
        );
        assert_eq!(
            attribute("a\tb\nc\rd").as_deref(),
            Some("a&#9;b&#10;c&#13;d")
        );
        assert_eq!(attribute("Bracket ü").as_deref(), Some("Bracket ü"));
        for bad in ["a\u{0}b", "\u{1b}[31m", "x\u{FFFE}", "x\u{FFFF}", "\u{1f}"] {
            assert_eq!(attribute(bad), None, "{bad:?}");
        }
    }
}
