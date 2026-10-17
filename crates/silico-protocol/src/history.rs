//! What an operation did to its inputs' faces, edges, and vertices, as OCCT's builders
//! report it (docs/ports/history.md). Topological naming (M2) is built on it.

use serde::{Deserialize, Serialize};

/// A kind of sub-shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ElementKind {
    Vertex,
    Edge,
    Face,
    /// A whole solid: what an extrusion reports its profile face generated.
    Solid,
}

/// A sub-shape of a shape, by kind and 1-based number in OCCT's `TopExp::MapShapes` order
/// (FreeCAD's `Face1`, `Edge1`, ...): of a result, in a history; of the operation's input,
/// in a refusal (`Refusal::elements`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ElementRef {
    pub kind: ElementKind,
    pub index: u32,
}

impl ElementRef {
    pub fn edge(index: u32) -> Self {
        Self {
            kind: ElementKind::Edge,
            index,
        }
    }

    pub fn face(index: u32) -> Self {
        Self {
            kind: ElementKind::Face,
            index,
        }
    }
}

/// FreeCAD's name for an element: `Face1`, `Edge3`, `Vertex2`, `Solid1`.
impl std::fmt::Display for ElementRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            ElementKind::Vertex => "Vertex",
            ElementKind::Edge => "Edge",
            ElementKind::Face => "Face",
            ElementKind::Solid => "Solid",
        };
        write!(f, "{kind}{}", self.index)
    }
}

/// A result element OCCT reported for an input element, and where in OCCT's list it was:
/// 1-based, counting entries the result doesn't have. FreeCAD's element names record the
/// position (`;:G2`, the second reported), so naming needs it; the list itself is sorted
/// by element, so its order never depends on OCCT's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Reported {
    pub element: ElementRef,
    pub position: u32,
}

/// What became of one input face, edge, or vertex.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementFate {
    /// Its number in the result (the same kind), when it's there unchanged.
    pub kept: Option<u32>,
    /// The result elements of the same kind it became, sorted by element.
    pub modified: Vec<Reported>,
    /// The result elements it created, of any kind (a filleted edge generates a face, an
    /// extruded face its solid), sorted by element.
    pub generated: Vec<Reported>,
}

impl ElementFate {
    /// Neither kept nor modified: gone from the result (though it may have generated
    /// something, as a filleted edge does).
    pub fn is_deleted(&self) -> bool {
        self.kept.is_none() && self.modified.is_empty()
    }
}

/// The history of one input shape: a fate for each of its faces, edges, and vertices, in
/// order (`faces[0]` is `Face1`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputHistory {
    pub faces: Vec<ElementFate>,
    pub edges: Vec<ElementFate>,
    pub vertices: Vec<ElementFate>,
}

/// How many of each kind of element a shape has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementCounts {
    pub faces: u32,
    pub edges: u32,
    pub vertices: u32,
    pub solids: u32,
}

impl ElementCounts {
    pub fn of(&self, kind: ElementKind) -> u32 {
        match kind {
            ElementKind::Vertex => self.vertices,
            ElementKind::Edge => self.edges,
            ElementKind::Face => self.faces,
            ElementKind::Solid => self.solids,
        }
    }
}

/// The result's faces a sweep starts and ends with (an extrusion's bottom and top, a
/// partial revolve's two sides), as OCCT's `FirstShape` and `LastShape` give them: its
/// history doesn't name them otherwise (FreeCAD guesses the top by its plane). Each list
/// is sorted; both are empty for an operation that isn't a sweep, and for a full revolve,
/// which has no ends (docs/ports/extrude-revolve.md).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ends {
    pub start: Vec<u32>,
    pub end: Vec<u32>,
}

/// How the result's elements bound each other, as naming needs it (docs/ports/naming.md):
/// FreeCAD names an element nothing reports from the elements above it (an edge from a
/// face it bounds) or below it (a face from its edges), so the core needs the result's
/// topology, which only OCCT can read. Each list numbers elements as the result does.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    /// Each face's edges, in `TopExp::MapShapes(face, EDGE)` order (a seam once): an edge
    /// named from a face is named by its place among them.
    pub face_edges: Vec<Vec<u32>>,
    /// The edges of each face's outer wire, sorted (`BRepTools::OuterWire`): a face is
    /// named from these, never from a hole's.
    pub outer_edges: Vec<Vec<u32>>,
    /// Each edge's vertices, in `TopExp::MapShapes(edge, VERTEX)` order (a closed edge's
    /// one vertex once).
    pub edge_vertices: Vec<Vec<u32>>,
}

/// An operation's history: one [`InputHistory`] per input shape, in the operation's order,
/// the result's element counts, so every reference can be checked against them, a sweep's
/// [`Ends`], and the result's [`Topology`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    pub inputs: Vec<InputHistory>,
    pub result: ElementCounts,
    pub ends: Ends,
    pub topology: Topology,
}

impl History {
    /// The fate of input `input`'s element (`input` from 0, the element numbered from 1),
    /// if the history has it. Use this rather than indexing: a history from another
    /// process is checked by [`History::validate`] for its references, not its lengths.
    pub fn fate(&self, input: usize, element: ElementRef) -> Option<&ElementFate> {
        let input = self.inputs.get(input)?;
        let fates = match element.kind {
            ElementKind::Face => &input.faces,
            ElementKind::Edge => &input.edges,
            ElementKind::Vertex => &input.vertices,
            ElementKind::Solid => return None,
        };
        fates.get((element.index as usize).checked_sub(1)?)
    }

    /// Checks that the history is for `inputs` input shapes, that every element it names is
    /// within the result's counts, and that it keeps its own promises: each `modified` is
    /// the input element's kind, every position is from 1, each list is sorted by element
    /// without repeats, and the topology has a list for each face and edge, each without
    /// repeats, a face's outer edges sorted and among its edges. Run it before using a
    /// history that came from another process. Two things it can't check: how many elements each input has (read fates
    /// with [`History::fate`]), and that `result` counts the shape that came with it (the
    /// core can't count without OCCT, so look result elements up with `get`).
    pub fn validate(&self, inputs: usize) -> Result<(), String> {
        if self.inputs.len() != inputs {
            return Err(format!(
                "the history is for {} input shapes, but the operation had {inputs}",
                self.inputs.len()
            ));
        }
        let counts = self.result;
        for (i, input) in self.inputs.iter().enumerate() {
            let kinds = [
                (ElementKind::Face, &input.faces),
                (ElementKind::Edge, &input.edges),
                (ElementKind::Vertex, &input.vertices),
            ];
            for (kind, fates) in kinds {
                for (n, fate) in fates.iter().enumerate() {
                    let what = || format!("input {} {kind:?}{}", i + 1, n + 1);
                    for (list, name) in
                        [(&fate.modified, "modified"), (&fate.generated, "generated")]
                    {
                        if !list.windows(2).all(|w| w[0].element < w[1].element) {
                            return Err(format!(
                                "{}'s {name} list isn't sorted by element, or repeats one",
                                what()
                            ));
                        }
                        if list.iter().any(|r| r.position == 0) {
                            return Err(format!("{} has a {name} element at position 0", what()));
                        }
                    }
                    if fate.modified.iter().any(|r| r.element.kind != kind) {
                        return Err(format!(
                            "{} is modified into an element of another kind",
                            what()
                        ));
                    }
                    if let Some(kept) = fate.kept
                        && !(1..=counts.of(kind)).contains(&kept)
                    {
                        return Err(format!(
                            "{} is kept as {kind:?}{kept}, which the result doesn't have",
                            what()
                        ));
                    }
                    for r in fate.modified.iter().chain(&fate.generated) {
                        let r = r.element;
                        if !(1..=counts.of(r.kind)).contains(&r.index) {
                            return Err(format!(
                                "{} names {:?}{}, which the result doesn't have",
                                what(),
                                r.kind,
                                r.index
                            ));
                        }
                    }
                }
            }
        }
        self.topology.validate(counts)?;
        for (list, name) in [(&self.ends.start, "start"), (&self.ends.end, "end")] {
            if !list.windows(2).all(|w| w[0] < w[1]) {
                return Err(format!("the {name} faces aren't sorted, or repeat one"));
            }
            if let Some(face) = list.iter().find(|&&f| !(1..=counts.faces).contains(&f)) {
                return Err(format!(
                    "the {name} faces name Face{face}, which the result doesn't have"
                ));
            }
        }
        Ok(())
    }
}

impl Topology {
    /// Checks the topology against the result's counts: see [`History::validate`].
    pub fn validate(&self, counts: ElementCounts) -> Result<(), String> {
        let lists = [
            ("faces' edges", &self.face_edges, counts.faces, counts.edges),
            (
                "faces' outer edges",
                &self.outer_edges,
                counts.faces,
                counts.edges,
            ),
            (
                "edges' vertices",
                &self.edge_vertices,
                counts.edges,
                counts.vertices,
            ),
        ];
        for (what, lists, owners, elements) in lists {
            if lists.len() != owners as usize {
                return Err(format!(
                    "the topology lists {} {what} for a result of {owners}",
                    lists.len()
                ));
            }
            for (n, list) in lists.iter().enumerate() {
                if let Some(e) = list.iter().find(|&&e| !(1..=elements).contains(&e)) {
                    return Err(format!(
                        "the topology's {what} for element {} name {e}, which the result doesn't have",
                        n + 1
                    ));
                }
                let mut sorted = list.clone();
                sorted.sort_unstable();
                if sorted.windows(2).any(|w| w[0] == w[1]) {
                    return Err(format!(
                        "the topology's {what} for element {} repeat one",
                        n + 1
                    ));
                }
            }
        }
        for (n, (outer, all)) in self.outer_edges.iter().zip(&self.face_edges).enumerate() {
            if !outer.windows(2).all(|w| w[0] < w[1]) {
                return Err(format!("Face{}'s outer edges aren't sorted", n + 1));
            }
            let mut all = all.clone();
            all.sort_unstable();
            if let Some(e) = outer.iter().find(|e| all.binary_search(e).is_err()) {
                return Err(format!(
                    "Face{}'s outer edges name Edge{e}, which isn't one of its edges",
                    n + 1
                ));
            }
        }
        Ok(())
    }
}
