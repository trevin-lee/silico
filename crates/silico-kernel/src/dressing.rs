//! Fillet and chamfer, and the edge rule they share (docs/ports/chamfer.md): which edges
//! OCCT may be given, decided here from the bridge's facts about the shape and from what
//! OCCT made of the edges it was given, before it builds anything.

use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;

use silico_protocol::{ChamferKind, DressingKind, EdgeTrouble, Operation, Refusal, Trouble};

use crate::{Built, ChamferSize, KernelError, Shape, checked_result, ffi, history_of};

/// Rounds `edges` of `shape` with a constant `radius`. Edges are numbered from 1 in OCCT's
/// `TopExp::MapShapes` order, which is also FreeCAD's `Edge1`, `Edge2`, and so on, and
/// they're a set: the order they're listed in doesn't matter. Edges OCCT won't round (a
/// seam, a degenerate edge) are refused, naming every edge at fault.
pub fn fillet(shape: &Shape, edges: &[u32], radius: f64) -> Result<Built, KernelError> {
    silico_protocol::check_fillet_radius(radius)?;
    let inner = shape.get()?;
    let topology = Topology::of(inner)?;
    let kind = DressingKind::Fillet;
    let edges = checked_edges(&topology, edges, kind)?;
    let mut dressing = ffi::new_fillet(inner)?;
    dress(
        shape,
        &topology,
        &edges,
        kind,
        dressing.pin_mut(),
        |d, edge, _| d.add_fillet(edge, radius),
    )
}

/// Cuts `edges` of `shape` back by `size` (docs/ports/chamfer.md), measured from each
/// edge's lower-numbered face, or its higher-numbered one when `flip` is set. Edges are
/// numbered and listed as for [`fillet`], and edges OCCT won't cut are refused, naming every
/// edge at fault.
pub fn chamfer(
    shape: &Shape,
    edges: &[u32],
    size: ChamferSize,
    flip: bool,
) -> Result<Built, KernelError> {
    size.check()
        .map_err(|invalid| crate::refused(Some(silico_protocol::SIZE_FIELD), invalid))?;
    let inner = shape.get()?;
    let topology = Topology::of(inner)?;
    let kind = DressingKind::Chamfer {
        kind: match size {
            ChamferSize::Equal { .. } => ChamferKind::Equal,
            ChamferSize::TwoDistances { .. } => ChamferKind::TwoDistances,
            ChamferSize::DistanceAngle { .. } => ChamferKind::DistanceAngle,
        },
    };
    let edges = checked_edges(&topology, edges, kind)?;
    let mut dressing = ffi::new_chamfer(inner)?;
    dress(
        shape,
        &topology,
        &edges,
        kind,
        dressing.pin_mut(),
        |d, edge, faces| {
            // FreeCAD's reference face: the first face OCCT lists for the edge (on a solid, the
            // lower-numbered), or the last when flipped. An edge on no face is never given.
            let face = if flip { faces.last() } else { faces.first() };
            let face = face.copied().unwrap_or(0);
            match size {
                // As FreeCAD's makeElementChamfer: two equal distances, not OCCT's symmetric
                // chamfer, so OCCT takes the same path as for two distances.
                ChamferSize::Equal { distance } => d.add_distances(edge, face, distance, distance),
                ChamferSize::TwoDistances { distance, other } => {
                    d.add_distances(edge, face, distance, other)
                }
                ChamferSize::DistanceAngle { distance, angle } => {
                    d.add_distance_angle(edge, face, distance, silico_protocol::radians(angle))
                }
            }
        },
    )
}

/// Gives OCCT `edges` through `add` (each edge and the faces it lies on), refuses them all
/// if any is one OCCT can't take, and builds the result, checked and with its history.
/// One refusal names every edge OCCT mustn't be given, skips, or follows into a shared
/// corner. When OCCT throws taking an edge, the edges already refused are named, or if
/// there are none, that edge is blamed.
fn dress(
    shape: &Shape,
    topology: &Topology,
    edges: &[u32],
    kind: DressingKind,
    mut dressing: Pin<&mut ffi::Dressing>,
    mut add: impl FnMut(Pin<&mut ffi::Dressing>, u32, &[u32]) -> Result<(), cxx::Exception>,
) -> Result<Built, KernelError> {
    let blame = |error: KernelError| blame_edges(error, edges, kind);
    // Every edge OCCT mustn't be given, decided before it's given any.
    let mut troubles: Vec<(u32, Trouble)> = edges
        .iter()
        .filter_map(|&edge| Some((edge, topology.withheld(edge)?)))
        .collect();
    let given: Vec<u32> = edges
        .iter()
        .copied()
        .filter(|&edge| topology.withheld(edge).is_none())
        .collect();
    for &edge in &given {
        if let Err(e) = add(dressing.as_mut(), edge, topology.faces(edge)) {
            // The edges already refused are said first; otherwise the edge OCCT threw on is
            // the one at fault.
            return Err(match troubles.is_empty() {
                true => blame_edges(e.into(), &[edge], kind),
                false => refuse(&troubles, kind),
            });
        }
    }
    // OCCT skips silently an edge it can't take, and follows each edge it takes along the
    // edges that continue it smoothly, which may reach a corner the given edge doesn't.
    let mut reaches: BTreeMap<u32, Option<u32>> = BTreeMap::new();
    for &edge in &given {
        let contour = dressing.contour_of(edge)?;
        if contour == 0 {
            troubles.push((edge, topology.skipped(edge)));
            continue;
        }
        let reach = match reaches.get(&contour) {
            Some(&reach) => reach,
            None => {
                let chain = dressing.contour_edges(contour)?;
                let reach = chain
                    .into_iter()
                    .find(|&e| topology.ends_at_shared_corner(e));
                reaches.insert(contour, reach);
                reach
            }
        };
        if let Some(far) = reach {
            troubles.push((edge, Trouble::ChainToSharedCorner { far }));
        }
    }
    if !troubles.is_empty() {
        troubles.sort_by_key(|&(edge, _)| edge);
        return Err(refuse(&troubles, kind));
    }
    let mut entries = Vec::new();
    let built = match dressing.as_mut().build(&mut entries) {
        Ok(built) => built,
        Err(e) => return Err(not_built(&dressing, topology, edges, kind, e)),
    };
    let result =
        checked_result(&[shape], built, Operation::Dressing { dressing: kind }).map_err(blame)?;
    let history = history_of(&[shape.get()?], result.get()?, &entries)?;
    Ok(Built {
        shape: result,
        history,
    })
}

/// A shape's edges, as the bridge reports them, and what follows from them.
struct Topology {
    facts: ffi::EdgeFacts,
    /// Where each edge's faces start in `facts.faces`.
    starts: Vec<usize>,
    /// The edges ending at each vertex.
    at_vertex: BTreeMap<u32, Vec<u32>>,
}

impl Topology {
    fn of(shape: &ffi::Shape) -> Result<Self, KernelError> {
        Ok(Self::from_facts(ffi::edge_facts(shape)?))
    }

    fn from_facts(facts: ffi::EdgeFacts) -> Self {
        let mut starts = Vec::with_capacity(facts.edges.len());
        let mut at_vertex: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        let mut start = 0;
        for (i, edge) in facts.edges.iter().enumerate() {
            starts.push(start);
            start += edge.face_count as usize;
            let number = i as u32 + 1;
            let ends: BTreeSet<u32> = [edge.first_vertex, edge.last_vertex].into();
            for vertex in ends.into_iter().filter(|&v| v != 0) {
                at_vertex.entry(vertex).or_default().push(number);
            }
        }
        Self {
            facts,
            starts,
            at_vertex,
        }
    }

    fn count(&self) -> u32 {
        self.facts.edges.len() as u32
    }

    fn fact(&self, edge: u32) -> Option<&ffi::EdgeFact> {
        self.facts.edges.get((edge as usize).checked_sub(1)?)
    }

    /// The faces `edge` lies on, as OCCT lists them (a seam's face twice).
    fn faces(&self, edge: u32) -> &[u32] {
        let (Some(fact), Some(&start)) = (
            self.fact(edge),
            (edge as usize)
                .checked_sub(1)
                .and_then(|i| self.starts.get(i)),
        ) else {
            return &[];
        };
        self.facts
            .faces
            .get(start..start + fact.face_count as usize)
            .unwrap_or(&[])
    }

    fn distinct_faces(&self, edge: u32) -> usize {
        self.faces(edge).iter().collect::<BTreeSet<_>>().len()
    }

    /// Whether the faces around `vertex` fall into more than one fan, each joined to the next
    /// through an edge they share: the shape touches itself, or another solid, at a point.
    fn is_shared_corner(&self, vertex: u32) -> bool {
        let Some(edges) = self.at_vertex.get(&vertex) else {
            return false;
        };
        // A union-find over the faces around the vertex, joined through each edge.
        let mut parent: BTreeMap<u32, u32> = BTreeMap::new();
        fn root(parent: &mut BTreeMap<u32, u32>, face: u32) -> u32 {
            let mut at = face;
            while let Some(&up) = parent.get(&at)
                && up != at
            {
                at = up;
            }
            parent.insert(face, at);
            at
        }
        for &edge in edges {
            let faces = self.faces(edge);
            for &face in faces {
                parent.entry(face).or_insert(face);
            }
            if let Some((&first, rest)) = faces.split_first() {
                for &face in rest {
                    let (a, b) = (root(&mut parent, first), root(&mut parent, face));
                    parent.insert(b, a);
                }
            }
        }
        let faces: Vec<u32> = parent.keys().copied().collect();
        let fans: BTreeSet<u32> = faces.into_iter().map(|f| root(&mut parent, f)).collect();
        fans.len() > 1
    }

    fn ends_at_shared_corner(&self, edge: u32) -> bool {
        self.fact(edge).is_some_and(|f| {
            [f.first_vertex, f.last_vertex]
                .into_iter()
                .any(|v| v != 0 && self.is_shared_corner(v))
        })
    }

    /// Why OCCT mustn't be given `edge`, if it mustn't: it can't use a degenerate edge or
    /// one on no face (FreeCAD's d214306d1b keeps those from it too), and segfaults on an
    /// edge joining more than two faces, or ending where the shape touches itself or
    /// another solid at a point (FreeCAD 1.1.3 with it).
    fn withheld(&self, edge: u32) -> Option<Trouble> {
        let fact = self.fact(edge)?;
        let faces = self.distinct_faces(edge);
        if fact.degenerate {
            Some(Trouble::Degenerate)
        } else if faces == 0 {
            Some(Trouble::NoFace)
        } else if faces > 2 {
            Some(Trouble::ManyFaces)
        } else if self.ends_at_shared_corner(edge) {
            Some(Trouble::SharedCorner)
        } else {
            None
        }
    }

    /// Why OCCT took no contour from `edge`, which it was given. OCCT's reasons
    /// (`ChFi3d_Builder::PerformElement`) are a degenerate edge, fewer than two faces, and
    /// faces it finds tangent; the first is never given to it.
    fn skipped(&self, edge: u32) -> Trouble {
        let seam = self.fact(edge).is_some_and(|f| f.seam);
        if self.distinct_faces(edge) < 2 && !seam {
            Trouble::OneFace
        } else {
            Trouble::Smooth
        }
    }
}

/// A fillet's or chamfer's edges, in increasing order. OCCT's result depends on the order
/// it's given them (every edge of the 10 × 20 × 30 box chamfered at 4.999999 builds in one
/// order and fails in the other, and in a chain of edges OCCT treats as one, the first
/// decides which face a chamfer measures from), so the same set always gets the same order.
/// Refuses a list that's empty, names an edge the shape doesn't have, or names one twice
/// (OCCT would skip the repeat).
fn checked_edges(
    topology: &Topology,
    edges: &[u32],
    dressing: DressingKind,
) -> Result<Vec<u32>, KernelError> {
    if edges.is_empty() {
        return Err(KernelError::new(Refusal::NoEdges { dressing }));
    }
    let count = topology.count();
    let mut seen = BTreeSet::new();
    for &edge in edges {
        if edge == 0 || edge > count {
            return Err(KernelError::new(Refusal::EdgeMissing { edge, count }));
        }
        if !seen.insert(edge) {
            return Err(KernelError::new(Refusal::EdgeTwice { edge }));
        }
    }
    Ok(seen.into_iter().collect())
}

/// A failed fillet or chamfer, as OCCT's failure on `edges`, where OCCT doesn't say which of
/// several edges it was (see `not_built`). Nor does it say why; the refusal says what to try (`Refusal::describe`). An
/// edge that ends, or whose chain ends, on one joining more than two faces fails whatever
/// its size, with OCCT's `Standard_ConstructionError` "ChFi3d_Builder:only 2 faces", which
/// OCCT 7.9.3 (pinned) throws only while taking that one edge (ChFi3d_Builder_1.cxx,
/// `FaceTangency`; a test fails if it's reworded): the refusal's suggestion keys on it.
fn blame_edges(error: KernelError, edges: &[u32], dressing: DressingKind) -> KernelError {
    error
        .during(Operation::Dressing { dressing })
        .on_edges(edges)
}

/// A fillet or chamfer OCCT couldn't build, as OCCT reports it where it says where: the
/// given `edges` in the contours it couldn't build, and in those reaching a corner it
/// couldn't blend (docs/ports/fillet.md). Where it names none, as for a chamfer (whose
/// report OCCT keeps private), the failure is on all the edges, as OCCT's words say.
fn not_built(
    dressing: &ffi::Dressing,
    topology: &Topology,
    edges: &[u32],
    kind: DressingKind,
    e: cxx::Exception,
) -> KernelError {
    let error = blame_edges(e.into(), edges, kind);
    match faulty_edges(dressing, topology, edges) {
        Ok(failed) if failed.is_empty() => error,
        Ok(failed) => KernelError::caused(
            Refusal::EdgesFailed {
                dressing: kind,
                edges: failed,
            },
            error.cause().map(str::to_owned),
        ),
        Err(e) => e,
    }
}

/// The given `edges` (in increasing order) in the contours OCCT reports it couldn't build,
/// or reaching a corner it reports it couldn't blend.
///
/// OCCT 7.9.3 (`ChFi3d_Builder::Compute`) builds each contour's surfaces, then, only if
/// all were built, each corner, then, only if those were too, checks each pair of contours
/// for a collision, stopping at the first. A contour whose own surfaces failed is faulty,
/// mostly with an error status; a corner that failed is a faulty vertex; a collision makes
/// the first contour of the pair faulty, with no error status, and not the other, which
/// may be the only one at fault. So where any faulty contour has an error status, the
/// surfaces failed and every faulty contour is at fault; where none has, they're named
/// only through the corners OCCT reports.
fn faulty_edges(
    dressing: &ffi::Dressing,
    topology: &Topology,
    edges: &[u32],
) -> Result<Vec<u32>, KernelError> {
    let faulty = dressing.faulty_contours()?;
    let mut surfaces_failed = false;
    for &contour in &faulty {
        surfaces_failed |= contour != 0 && dressing.contour_failed(contour)?;
    }
    let mut contours: BTreeSet<u32> = match surfaces_failed {
        true => faulty.into_iter().collect(),
        false => BTreeSet::new(),
    };
    for corner in dressing.faulty_vertices()? {
        for &edge in topology.at_vertex.get(&corner).into_iter().flatten() {
            contours.insert(dressing.contour_of(edge)?);
        }
    }
    // An edge in no contour is in contour 0, which no faulty contour is.
    contours.remove(&0);
    let mut failed = Vec::new();
    for &edge in edges {
        if contours.contains(&dressing.contour_of(edge)?) {
            failed.push(edge);
        }
    }
    Ok(failed)
}

/// The refusal of edges OCCT can't fillet or chamfer (`troubles`, in the edges'
/// increasing order): every one is at fault, and the refusal names them all.
fn refuse(troubles: &[(u32, Trouble)], dressing: DressingKind) -> KernelError {
    if troubles.is_empty() {
        return KernelError::new(Refusal::NoEdgeToRefuse { dressing });
    }
    KernelError::new(Refusal::EdgesRefused {
        dressing,
        troubles: troubles
            .iter()
            .map(|&(edge, trouble)| EdgeTrouble { edge, trouble })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A topology from `(degenerate, seam, first vertex, last vertex, faces)` per edge.
    fn topology(edges: &[(bool, bool, u32, u32, &[u32])]) -> Topology {
        let mut facts = ffi::EdgeFacts {
            edges: Vec::new(),
            faces: Vec::new(),
        };
        for &(degenerate, seam, first_vertex, last_vertex, faces) in edges {
            facts.edges.push(ffi::EdgeFact {
                degenerate,
                seam,
                first_vertex,
                last_vertex,
                face_count: faces.len() as u32,
            });
            facts.faces.extend_from_slice(faces);
        }
        Topology::from_facts(facts)
    }

    /// A corner of a box: three edges from vertex 1, joining faces 1-2, 2-3, and 3-1.
    fn box_corner() -> Vec<(bool, bool, u32, u32, &'static [u32])> {
        vec![
            (false, false, 1, 2, &[1, 2]),
            (false, false, 1, 3, &[2, 3]),
            (false, false, 1, 4, &[3, 1]),
        ]
    }

    #[test]
    fn a_box_corner_is_one_fan() {
        let t = topology(&box_corner());
        assert!(!t.is_shared_corner(1));
        assert!(!t.is_shared_corner(2));
        assert_eq!(t.withheld(1), None);
    }

    #[test]
    fn two_box_corners_touching_are_two_fans() {
        let mut edges = box_corner();
        edges.extend([
            (false, false, 1, 5, &[4u32, 5][..]),
            (false, false, 1, 6, &[5, 6]),
            (false, false, 1, 7, &[6, 4]),
        ]);
        let t = topology(&edges);
        assert!(t.is_shared_corner(1));
        assert!(!t.is_shared_corner(5));
        for edge in 1..=6 {
            assert_eq!(t.withheld(edge), Some(Trouble::SharedCorner), "Edge{edge}");
            assert!(t.ends_at_shared_corner(edge));
        }
    }

    #[test]
    fn fans_joined_by_one_edge_are_one() {
        // Two corners' fans, joined through an edge on a face of each: one fan.
        let mut edges = box_corner();
        edges.extend([
            (false, false, 1, 5, &[4u32, 5][..]),
            (false, false, 1, 6, &[5, 6]),
            (false, false, 1, 7, &[6, 4]),
            (false, false, 1, 8, &[1, 4]),
        ]);
        assert!(!topology(&edges).is_shared_corner(1));
    }

    #[test]
    fn a_seam_and_a_pole_are_one_fan() {
        // A sphere: its seam (Edge2) lies on its one face twice, between two degenerate
        // edges at its poles.
        let t = topology(&[
            (true, false, 1, 1, &[1]),
            (false, true, 1, 2, &[1, 1]),
            (true, false, 2, 2, &[1]),
        ]);
        assert!(!t.is_shared_corner(1) && !t.is_shared_corner(2));
        assert_eq!(t.withheld(1), Some(Trouble::Degenerate));
        assert_eq!(t.withheld(2), None);
        assert_eq!(t.skipped(2), Trouble::Smooth);
    }

    #[test]
    fn each_edge_is_withheld_or_skipped_for_its_reason() {
        let t = topology(&[
            (false, false, 1, 2, &[]),
            (false, false, 2, 3, &[1]),
            (false, false, 3, 4, &[1, 2, 3, 4]),
            (false, false, 4, 5, &[1, 2]),
            (false, true, 5, 5, &[2, 2]),
        ]);
        assert_eq!(t.withheld(1), Some(Trouble::NoFace));
        assert_eq!(t.withheld(2), None);
        assert_eq!(t.skipped(2), Trouble::OneFace);
        assert_eq!(t.withheld(3), Some(Trouble::ManyFaces));
        assert_eq!(t.withheld(4), None);
        assert_eq!(t.skipped(4), Trouble::Smooth);
        assert_eq!(t.skipped(5), Trouble::Smooth);
        // An edge the shape doesn't have has no facts and no faces.
        assert_eq!(t.withheld(6), None);
        assert_eq!(t.faces(0), &[] as &[u32]);
        assert_eq!(t.faces(6), &[] as &[u32]);
    }

    #[test]
    fn a_refusal_names_every_edge() {
        use silico_protocol::ChamferKind;

        use crate::KernelErrorKind;
        let chamfer = DressingKind::Chamfer {
            kind: ChamferKind::Equal,
        };
        let one = refuse(&[(3, Trouble::Smooth)], chamfer);
        assert_eq!(one.elements(), ["Edge3"]);
        assert_eq!(one.suggestion(), Some("leave Edge3 out of the chamfer"));
        assert_eq!(one.kind(), KernelErrorKind::InvalidInput);
        let two = refuse(
            &[(3, Trouble::OneFace), (7, Trouble::Smooth)],
            DressingKind::Fillet,
        );
        assert_eq!(two.elements(), ["Edge3", "Edge7"]);
        assert!(two.message().ends_with("; OCCT won't fillet Edge7 either"));
        assert_eq!(
            two.suggestion(),
            Some("leave Edge3 and Edge7 out of the fillet")
        );
        let three = refuse(
            &[
                (1, Trouble::ChainToSharedCorner { far: 9 }),
                (2, Trouble::Degenerate),
                (5, Trouble::NoFace),
            ],
            chamfer,
        );
        // Every edge at fault, not the one it continues into.
        assert_eq!(three.elements(), ["Edge1", "Edge2", "Edge5"]);
        assert!(
            three
                .message()
                .starts_with("Edge1 continues smoothly into Edge9")
        );
        assert!(
            three
                .message()
                .ends_with("; OCCT won't chamfer Edge2, Edge5 either")
        );
        assert_eq!(
            three.suggestion(),
            Some("leave Edge1, Edge2, and Edge5 out of the chamfer")
        );
        assert_eq!(refuse(&[], chamfer).kind(), KernelErrorKind::Internal);
    }
}
