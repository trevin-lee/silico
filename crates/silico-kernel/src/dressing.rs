//! Fillet and chamfer, and the edge rule they share (docs/ports/chamfer.md): which edges
//! OCCT may be given, decided here from the bridge's facts about the shape and from what
//! OCCT made of the edges it was given, before it builds anything.

use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;

use crate::{
    Built, ChamferSize, KernelError, KernelErrorKind, Shape, checked_result, ffi, history_of,
};

/// Rounds `edges` of `shape` with a constant `radius`. Edges are numbered from 1 in OCCT's
/// `TopExp::MapShapes` order, which is also FreeCAD's `Edge1`, `Edge2`, and so on, and
/// they're a set: the order they're listed in doesn't matter. Edges OCCT won't round (a
/// seam, a degenerate edge) are refused, naming every edge at fault.
pub fn fillet(shape: &Shape, edges: &[u32], radius: f64) -> Result<Built, KernelError> {
    if !(radius.is_finite() && radius > 0.0) {
        return Err(KernelError::invalid_input(format!(
            "fillet radius must be a positive, finite length, got {radius}"
        )));
    }
    let inner = shape.get()?;
    let topology = Topology::of(inner)?;
    let edges = checked_edges(&topology, edges, "fillet")?;
    let advice = Advice {
        operation: "fillet",
        smaller: "a smaller radius",
        instead: "another radius",
    };
    let mut dressing = ffi::new_fillet(inner)?;
    dress(
        shape,
        &topology,
        &edges,
        &advice,
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
        .map_err(|invalid| KernelError::invalid_input(invalid.message))?;
    let inner = shape.get()?;
    let topology = Topology::of(inner)?;
    let edges = checked_edges(&topology, edges, "chamfer")?;
    let (smaller, instead) = match size {
        ChamferSize::Equal { .. } => ("a smaller size", "another size"),
        ChamferSize::TwoDistances { .. } => ("smaller distances", "other distances"),
        ChamferSize::DistanceAngle { .. } => {
            ("a smaller distance or angle", "another distance or angle")
        }
    };
    let advice = Advice {
        operation: "chamfer",
        smaller,
        instead,
    };
    let mut dressing = ffi::new_chamfer(inner)?;
    dress(
        shape,
        &topology,
        &edges,
        &advice,
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

/// What a failed fillet or chamfer suggests trying: a `smaller` size, or one `instead`.
struct Advice {
    operation: &'static str,
    smaller: &'static str,
    instead: &'static str,
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
    advice: &Advice,
    mut dressing: Pin<&mut ffi::Dressing>,
    mut add: impl FnMut(Pin<&mut ffi::Dressing>, u32, &[u32]) -> Result<(), cxx::Exception>,
) -> Result<Built, KernelError> {
    let blame = |error: KernelError| blame_edges(error, edges, advice);
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
                true => blame_edges(e.into(), &[edge], advice),
                false => refuse(&troubles, advice.operation),
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
            troubles.push((edge, Trouble::ChainToSharedCorner(far)));
        }
    }
    if !troubles.is_empty() {
        troubles.sort_by_key(|&(edge, _)| edge);
        return Err(refuse(&troubles, advice.operation));
    }
    let mut entries = Vec::new();
    let built = dressing.build(&mut entries).map_err(|e| blame(e.into()))?;
    let result = checked_result(&[shape], built, advice.operation).map_err(blame)?;
    let history = history_of(&[shape.get()?], result.get()?, &entries)?;
    Ok(Built {
        shape: result,
        history,
    })
}

/// Why OCCT can't fillet or chamfer an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trouble {
    /// A point where a surface closes (a sphere's pole, a cone's tip).
    Degenerate,
    NoFace,
    /// The shape is open there.
    OneFace,
    /// Solids touch along it.
    ManyFaces,
    /// It ends where the shape touches itself or another solid at a point.
    SharedCorner,
    /// It continues smoothly into this edge, which ends at such a point.
    ChainToSharedCorner(u32),
    /// Its faces are tangent there to OCCT, or it's a seam.
    Smooth,
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
    operation: &str,
) -> Result<Vec<u32>, KernelError> {
    if edges.is_empty() {
        return Err(KernelError::invalid_input(format!(
            "a {operation} needs at least one edge"
        )));
    }
    let count = topology.count();
    let mut seen = BTreeSet::new();
    for &n in edges {
        if n == 0 || n > count {
            let has = match count {
                0 => "the shape has no edges".to_owned(),
                _ => format!("the shape has edges 1 to {count}"),
            };
            return Err(
                KernelError::invalid_input(format!("Edge{n} doesn't exist: {has}"))
                    .with_element(format!("Edge{n}")),
            );
        }
        if !seen.insert(n) {
            return Err(
                KernelError::invalid_input(format!("Edge{n} is listed twice"))
                    .with_element(format!("Edge{n}")),
            );
        }
    }
    Ok(seen.into_iter().collect())
}

/// What a failed fillet or chamfer says about its edges: when there's one, it's the one at
/// fault; OCCT doesn't say which of several it was. OCCT doesn't say why, either: a size
/// too large for the faces beside the edge fails, but so do some small ones (on a cone's
/// side) and some edges at every size, so the suggestion offers a smaller size, another,
/// and leaving the edge out. An edge that ends, or whose chain ends, on one joining more
/// than two faces (where solids touch along an edge) fails whatever its size, with OCCT's
/// `Standard_ConstructionError` "ChFi3d_Builder:only 2 faces", which OCCT 7.9.3 (pinned)
/// throws only while taking that one edge (ChFi3d_Builder_1.cxx, `FaceTangency`; a test
/// fails if it's reworded), and the suggestion says so.
fn blame_edges(error: KernelError, edges: &[u32], advice: &Advice) -> KernelError {
    let Advice {
        operation,
        smaller,
        instead,
    } = advice;
    let touching = error.message().contains("only 2 faces");
    let suggestion = match edges {
        [edge] if touching => format!(
            "Edge{edge}, or an edge it continues smoothly into, ends where more than two faces meet (where solids touch along an edge), and OCCT can't {operation} it there: leave it out"
        ),
        [_] => format!(
            "if the {operation} runs past a face beside the edge, try {smaller}; if not, try {instead}, or leave the edge out: OCCT can't {operation} some edges at any size"
        ),
        _ => format!(
            "{operation} the edges one at a time to find the one that fails; then, if its {operation} runs past a face beside it, try {smaller}, and if not, {instead}, or leave it out"
        ),
    };
    let error = match edges {
        [edge] => error.with_element(format!("Edge{edge}")),
        _ => error,
    };
    error.with_suggestion(suggestion)
}

/// The refusal of edges OCCT can't fillet or chamfer (`troubles`, in the edges' increasing
/// order):
/// why for the first, and every one named, in the message and in the suggestion, so one
/// refusal says everything to leave out.
fn refuse(troubles: &[(u32, Trouble)], operation: &str) -> KernelError {
    let Some(&(first, trouble)) = troubles.first() else {
        return KernelError::of_kind(
            KernelErrorKind::Internal,
            format!("no edge to refuse the {operation} for"),
        );
    };
    let edge = format!("Edge{first}");
    let at_a_point = "where the shape touches itself or another solid at a point";
    let mut message = match trouble {
        Trouble::Degenerate => format!(
            "{edge} is degenerate (a point where a surface closes, such as a sphere's pole or a cone's tip), so there's no corner to {operation}"
        ),
        Trouble::NoFace => format!("{edge} is on no face, so there's no corner to {operation}"),
        Trouble::OneFace => format!(
            "{edge} is on only one face (the shape is open there), so there's no corner to {operation}"
        ),
        Trouble::ManyFaces => format!(
            "{edge} joins more than two faces (where solids touch along an edge), and OCCT can't {operation} it"
        ),
        Trouble::SharedCorner => {
            format!("{edge} ends at a corner {at_a_point}, and OCCT can't {operation} it there")
        }
        Trouble::ChainToSharedCorner(far) => format!(
            "{edge} continues smoothly into Edge{far}, which ends at a corner {at_a_point}, and OCCT can't {operation} there"
        ),
        Trouble::Smooth => format!(
            "{edge} joins faces OCCT treats as tangent there (recorded as tangent, or turning by less than 0.1 radian, 5.7°), or a face to itself at a seam: too shallow a corner for OCCT to {operation}"
        ),
    };
    let named: Vec<String> = troubles.iter().map(|(e, _)| format!("Edge{e}")).collect();
    if let Some(others) = named.get(1..)
        && !others.is_empty()
    {
        message.push_str(&format!(
            "; OCCT won't {operation} {} either",
            others.join(", ")
        ));
    }
    KernelError::invalid_input(message)
        .with_element(edge)
        .with_suggestion(format!("leave {} out of the {operation}", listed(&named)))
}

/// `a`, `a and b`, or `a, b, and c`.
fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
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
        let one = refuse(&[(3, Trouble::Smooth)], "chamfer");
        assert_eq!(one.element(), Some("Edge3"));
        assert_eq!(one.suggestion(), Some("leave Edge3 out of the chamfer"));
        assert_eq!(one.kind(), KernelErrorKind::InvalidInput);
        let two = refuse(&[(3, Trouble::OneFace), (7, Trouble::Smooth)], "fillet");
        assert!(two.message().ends_with("; OCCT won't fillet Edge7 either"));
        assert_eq!(
            two.suggestion(),
            Some("leave Edge3 and Edge7 out of the fillet")
        );
        let three = refuse(
            &[
                (1, Trouble::ChainToSharedCorner(9)),
                (2, Trouble::Degenerate),
                (5, Trouble::NoFace),
            ],
            "chamfer",
        );
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
        assert_eq!(refuse(&[], "chamfer").kind(), KernelErrorKind::Internal);
    }
}
