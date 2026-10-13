//! Why a worker refused a request or failed it, as data (M2.0). A worker sends a
//! [`Refusal`]: the reason, the elements at fault as [`ElementRef`]s (every one), the
//! parameter at fault, and the values that matter, but no element names and no sentences.
//! The core names the elements (as the document names them) and has the refusal described
//! with those names ([`Refusal::describe`]): the message and the suggestion are written
//! here, from the data, so both ends say the same thing and the worker says none of it.
//! OCCT's own words, when it gave any, travel beside the refusal as its cause.

use serde::{Deserialize, Serialize};

use crate::{BooleanOp, ElementRef, FailureKind, LENGTH_TOLERANCE, OFFSET_FIELD, format_number};

/// Why a request was refused or failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Refusal {
    /// A shape with nothing in it was given where one was needed.
    NoShape,
    /// OCCT gave back nothing where it should have made something.
    NothingReturned { what: Returned },
    /// OCCT (or Silico's bridge to it) threw, doing `operation`; its words are the cause.
    /// `edges`, for a fillet or chamfer, the edges it was given.
    Occt {
        operation: Operation,
        edges: Vec<u32>,
    },
    /// OCCT did `operation`, but its result isn't valid, so it's refused. `edges`, for a
    /// fillet or chamfer, the edges it was given.
    ResultInvalid {
        operation: Operation,
        edges: Vec<u32>,
    },
    /// OCCT couldn't build a fillet, and said where: `edges`, the given edges, in increasing
    /// order, in the contours it couldn't build and in those reaching a corner it couldn't
    /// blend. Its words, where it threw, are the cause.
    EdgesFailed {
        dressing: DressingKind,
        edges: Vec<u32>,
    },
    /// Bytes too few to hold a B-rep and its checksum.
    BrepTooShort,
    /// A B-rep whose checksum doesn't match.
    BrepCorrupted,
    /// A B-rep OCCT reads, but can't work on correctly.
    BrepDefect { defect: BrepDefect },
    /// The bridge reported a B-rep defect this side doesn't know.
    UnknownBrepDefect { code: u32 },
    /// A value the operation can't take: the worker's own guard, the same check the core
    /// makes before sending (the protocol's), so the message is that check's.
    InvalidParameter { parameter: String, message: String },
    /// OCCT merged a profile's corners `first` and `second` (1-based, next to each other),
    /// finding them within its tolerance as it measures, and made `sides` sides of
    /// `corners` corners.
    CornersMerged {
        corners: u32,
        sides: u32,
        first: u32,
        second: u32,
    },
    /// A boolean's input with nothing in it.
    BooleanInputEmpty { op: BooleanOp, role: Role },
    /// A boolean whose result has nothing in it, or no volume.
    BooleanResultEmpty { op: BooleanOp },
    /// A fillet or chamfer given no edges.
    NoEdges { dressing: DressingKind },
    /// An edge number the shape doesn't have; it has `count` edges.
    EdgeMissing { edge: u32, count: u32 },
    /// An edge listed more than once.
    EdgeTwice { edge: u32 },
    /// Edges OCCT can't fillet or chamfer, in increasing order, and why.
    EdgesRefused {
        dressing: DressingKind,
        troubles: Vec<EdgeTrouble>,
    },
    /// A mesh deflection that isn't a positive, finite length.
    DeflectionInvalid { deflection: f64 },
    /// A mesh deflection finer than this shape allows (`floor`).
    DeflectionTooFine { deflection: f64, floor: f64 },
    /// A shape whose extent is too large, or not finite, to mesh.
    ExtentNotFinite,
    /// Faces OCCT couldn't mesh, on a shape that isn't valid: the core's decision, from a
    /// mesh that says so (a worker never sends it). `faces`, the first of them, 1-based, in
    /// order, up to [`FACES_NAMED`]; `count`, how many there are, at least as many as `faces`
    /// lists (and more only when it lists [`FACES_NAMED`]).
    FacesUnmeshed { faces: Vec<u32>, count: u64 },
    /// A measure of the shape that isn't finite: which (`volume`, `center x`, ...).
    NotFinite { quantity: String, value: f64 },
    /// Silico's bridge has no result check for an operation on this many shapes.
    ResultCheckMissing { inputs: u32 },
    /// Silico's bridge reported a history it shouldn't have.
    MalformedHistory { defect: HistoryDefect },
    /// Silico's own fillet or chamfer code had no edge to refuse.
    NoEdgeToRefuse { dressing: DressingKind },
    /// A test operation sent to a production worker.
    TestOperation { op: String },
    /// Floating-point traps found on after `op` (trap-enable bits `mask`).
    FpTrapsOn { mask: u32, op: String },
    /// Something went wrong in the worker itself, in a test worker's words.
    Internal { what: String },
}

/// What OCCT gave back nothing of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Returned {
    Shape,
    Brep,
    Face,
    Mesh,
}

/// The operation OCCT was doing, for what to suggest when it fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    /// A primitive, by its name (`box`, `sphere`, ...).
    Primitive {
        name: String,
    },
    Translate,
    Extrusion,
    Revolve,
    Boolean {
        op: BooleanOp,
    },
    Dressing {
        dressing: DressingKind,
    },
    Tessellate,
    /// Anything else: reading or writing B-reps, measuring.
    Other,
}

/// A boolean's input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Base,
    Tool,
}

/// A fillet, or a chamfer of one of the size kinds (for what to suggest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DressingKind {
    Fillet,
    Chamfer { kind: ChamferKind },
}

/// The kind of a [`ChamferSize`](crate::ChamferSize).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChamferKind {
    Equal,
    TwoDistances,
    DistanceAngle,
}

/// An edge OCCT can't fillet or chamfer, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeTrouble {
    pub edge: u32,
    pub trouble: Trouble,
}

/// Why OCCT can't fillet or chamfer an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trouble {
    /// A point where a surface closes (a sphere's pole, a cone's tip).
    Degenerate,
    NoFace,
    /// The shape is open there.
    OneFace,
    /// Solids touch along it.
    ManyFaces,
    /// It ends where the shape touches itself or another solid at a point.
    SharedCorner,
    /// It continues smoothly into edge `far`, which ends at such a point.
    ChainToSharedCorner {
        far: u32,
    },
    /// Its faces are tangent there to OCCT, or it's a seam.
    Smooth,
}

/// What OCCT reads in a B-rep but can't work on correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrepDefect {
    MirrorOrScale,
    LocationNotFinite,
    FaceWithoutSurface,
    PointNotFinite,
    CurveNotFinite,
    SurfaceNotFinite,
}

/// What's wrong with a history the bridge reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryDefect {
    UnknownInput,
    WrongKind,
    UnknownElement,
    UnknownRelation,
}

/// A refusal in words: what went wrong, and what to try, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Described {
    pub message: String,
    pub suggestion: Option<String>,
}

/// The most faces a refusal of unmeshed faces names as elements: the first, in order. A
/// mesh can have a face for every byte of it, and the error stays small.
pub const FACES_NAMED: usize = 100;

/// The most faces a refusal of unmeshed faces names in its words; the rest are counted.
const FACES_IN_WORDS: usize = 8;

/// What to try when a shape's geometry can't be meshed.
const UNUSABLE_GEOMETRY: &str = "the shape's geometry is broken (a value too large, or not a number): rebuild it from its inputs";
/// What to try for a B-rep OCCT can't work on.
const REBUILD: &str = "rebuild the shape from its inputs";
/// What to try when a boolean's result was refused as invalid.
const NEARLY_MEET: &str = "faces or edges of the two shapes nearly meet: move one so they clearly overlap or stay clearly apart";
/// What to try when an offset is refused.
const OFFSET: &str = "move the shape by a finite offset, no more than a kilometre each way";

impl Refusal {
    /// What kind of failure it is.
    pub fn kind(&self) -> FailureKind {
        match self {
            Refusal::BrepTooShort
            | Refusal::BrepCorrupted
            | Refusal::BrepDefect { .. }
            | Refusal::InvalidParameter { .. }
            | Refusal::CornersMerged { .. }
            | Refusal::BooleanInputEmpty { .. }
            | Refusal::NoEdges { .. }
            | Refusal::EdgeMissing { .. }
            | Refusal::EdgeTwice { .. }
            | Refusal::EdgesRefused { .. }
            | Refusal::DeflectionInvalid { .. }
            | Refusal::DeflectionTooFine { .. } => FailureKind::InvalidInput,
            Refusal::ResultInvalid { .. } => FailureKind::InvalidResult,
            Refusal::NoShape
            | Refusal::NothingReturned { .. }
            | Refusal::Occt { .. }
            | Refusal::EdgesFailed { .. }
            | Refusal::UnknownBrepDefect { .. }
            | Refusal::BooleanResultEmpty { .. }
            | Refusal::ExtentNotFinite
            | Refusal::FacesUnmeshed { .. }
            | Refusal::NotFinite { .. } => FailureKind::KernelFailed,
            Refusal::TestOperation { .. } => FailureKind::UnknownOp,
            Refusal::ResultCheckMissing { .. }
            | Refusal::MalformedHistory { .. }
            | Refusal::NoEdgeToRefuse { .. }
            | Refusal::FpTrapsOn { .. }
            | Refusal::Internal { .. } => FailureKind::Internal,
        }
    }

    /// Every element at fault, in order, each an element of the operation's one input shape.
    /// A fillet or chamfer OCCT failed on several edges names none, unless OCCT said which
    /// (`EdgesFailed`).
    /// An edge the shape doesn't have, and a profile's merged corners, aren't the shape's
    /// elements: those refusals name their parameter (`edges`, `profile`) and the numbers.
    pub fn elements(&self) -> Vec<ElementRef> {
        match self {
            Refusal::Occt { edges, .. } | Refusal::ResultInvalid { edges, .. } => match edges[..] {
                [edge] => vec![ElementRef::edge(edge)],
                _ => Vec::new(),
            },
            Refusal::EdgeTwice { edge } => vec![ElementRef::edge(*edge)],
            Refusal::EdgesFailed { edges, .. } => {
                edges.iter().map(|&e| ElementRef::edge(e)).collect()
            }
            Refusal::EdgesRefused { troubles, .. } => {
                troubles.iter().map(|t| ElementRef::edge(t.edge)).collect()
            }
            Refusal::FacesUnmeshed { faces, .. } => faces
                .iter()
                .take(FACES_NAMED)
                .map(|&face| ElementRef::face(face))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// How many elements are at fault: as many as [`Refusal::elements`] names, except for
    /// unmeshed faces, which it names only the first of.
    pub fn elements_total(&self) -> u64 {
        match self {
            Refusal::FacesUnmeshed { count, .. } => *count,
            _ => self.elements().len() as u64,
        }
    }

    /// The parameter at fault, as a dotted name (see `InvalidParameter::name`), if a value
    /// was refused.
    pub fn parameter(&self) -> Option<&str> {
        match self {
            Refusal::InvalidParameter { parameter, .. } => Some(parameter),
            Refusal::NoEdges { .. }
            | Refusal::EdgeMissing { .. }
            | Refusal::EdgeTwice { .. }
            | Refusal::EdgesRefused { .. } => Some("edges"),
            Refusal::DeflectionInvalid { .. } | Refusal::DeflectionTooFine { .. } => {
                Some("deflection")
            }
            Refusal::CornersMerged { .. } => Some("profile"),
            Refusal::BooleanInputEmpty { role, .. } => Some(role.name()),
            _ => None,
        }
    }

    /// The refusal in words, naming elements with `name`; `cause`, OCCT's words, if it
    /// gave any (some suggestions depend on them).
    pub fn describe(&self, cause: Option<&str>, name: &dyn Fn(ElementRef) -> String) -> Described {
        let edge = |n: u32| name(ElementRef::edge(n));
        let (message, suggestion): (String, Option<String>) = match self {
            Refusal::NoShape => ("shape is empty".to_owned(), None),
            Refusal::NothingReturned { what } => {
                let what = match what {
                    Returned::Shape => "shape",
                    Returned::Brep => "B-rep",
                    Returned::Face => "face",
                    Returned::Mesh => "mesh",
                };
                (format!("OCCT returned no {what}"), None)
            }
            Refusal::Occt { operation, edges } => (
                format!("OCCT couldn't {}", operation.verb()),
                occt_advice(operation, edges, cause, &edge),
            ),
            Refusal::EdgesFailed { dressing, edges } => {
                let named: Vec<String> = edges.iter().map(|&e| edge(e)).collect();
                (
                    format!("OCCT couldn't {} {}", dressing.name(), listed(&named)),
                    Some(failed_advice(*dressing, edges)),
                )
            }
            Refusal::ResultInvalid { operation, edges } => (
                match operation {
                    Operation::Primitive { name } => format!("the {name} isn't a valid solid"),
                    other => format!("the {}'s result isn't a valid solid", other.noun()),
                },
                invalid_result_advice(operation, edges, &edge),
            ),
            Refusal::BrepTooShort => ("B-rep is too short to be one".to_owned(), None),
            Refusal::BrepCorrupted => (
                "B-rep is corrupted: its checksum doesn't match".to_owned(),
                None,
            ),
            Refusal::BrepDefect { defect } => {
                let (what, suggestion) = match defect {
                    BrepDefect::MirrorOrScale => (
                        "places a shape by a transformation that mirrors or scales, which OCCT can't model consistently",
                        REBUILD,
                    ),
                    BrepDefect::LocationNotFinite => (
                        "places a shape by a transformation whose values aren't finite",
                        REBUILD,
                    ),
                    BrepDefect::FaceWithoutSurface => (
                        "has a face with no surface, which OCCT can't operate on",
                        "rebuild the shape from its inputs; a mesh (a face of only triangles, as STEP or glTF import makes) can't be used as a solid yet",
                    ),
                    BrepDefect::PointNotFinite => {
                        ("has a vertex whose point or tolerance isn't finite", REBUILD)
                    }
                    BrepDefect::CurveNotFinite => (
                        "has an edge whose curve, parameter range, or tolerance isn't finite",
                        REBUILD,
                    ),
                    BrepDefect::SurfaceNotFinite => (
                        "has a face whose surface, mesh, or tolerance isn't finite",
                        REBUILD,
                    ),
                };
                (format!("B-rep {what}"), Some(suggestion.to_owned()))
            }
            Refusal::UnknownBrepDefect { code } => (
                format!("the bridge reported an unknown B-rep defect {code}"),
                None,
            ),
            Refusal::InvalidParameter { parameter, message } => (
                message.clone(),
                parameter
                    .strip_prefix(OFFSET_FIELD)
                    .is_some_and(|rest| rest.starts_with('.'))
                    .then(|| OFFSET.to_owned()),
            ),
            Refusal::CornersMerged {
                corners,
                sides,
                first,
                second,
            } => (
                format!(
                    "OCCT made the profile's {corners} corners into {sides} sides: corners {first} and {second} are within its tolerance ({} mm) of each other as it measures them",
                    format_number(LENGTH_TOLERANCE)
                ),
                Some("move those corners further apart, or make them one corner".to_owned()),
            ),
            Refusal::BooleanInputEmpty { op, role } => {
                let op = op.name().to_lowercase();
                let role = role.name();
                (
                    format!("the {op}'s {role} is empty"),
                    Some(format!(
                        "give the {op} a {role} with something in it: the operation that made it made nothing"
                    )),
                )
            }
            Refusal::BooleanResultEmpty { op } => (
                format!("the {}'s result is empty", op.name().to_lowercase()),
                // OCCT can find nothing for shapes that do overlap (where faces or edges
                // nearly meet, or across a torus's seam), so this doesn't claim more.
                Some(
                    match op {
                        BooleanOp::Fuse => "OCCT lost both shapes: rebuild them, or move one slightly",
                        BooleanOp::Cut => {
                            "OCCT found the tool removes all of the base: move or shrink the tool (if it doesn't, move it slightly)"
                        }
                        BooleanOp::Common => {
                            "OCCT found no overlap: move the shapes so they overlap (if they already do, move one slightly)"
                        }
                    }
                    .to_owned(),
                ),
            ),
            Refusal::NoEdges { dressing } => (
                format!("a {} needs at least one edge", dressing.name()),
                None,
            ),
            Refusal::EdgeMissing { edge: n, count } => {
                let has = match count {
                    0 => "the shape has no edges".to_owned(),
                    _ => format!("the shape has edges 1 to {count}"),
                };
                // Not one of the shape's elements, so a name from the shape (the namer's)
                // would be wrong: by its number, as the request gave it.
                (format!("edge {n} doesn't exist: {has}"), None)
            }
            Refusal::EdgeTwice { edge: n } => (format!("{} is listed twice", edge(*n)), None),
            Refusal::EdgesRefused { dressing, troubles } => {
                return refused_edges(*dressing, troubles, &edge);
            }
            Refusal::DeflectionInvalid { deflection } => (
                format!(
                    "mesh deflection must be a positive, finite length, got {}",
                    format_number(*deflection)
                ),
                None,
            ),
            Refusal::DeflectionTooFine { deflection, floor } => (
                format!(
                    "mesh deflection {} is too fine for this shape: use at least {}",
                    format_number(*deflection),
                    format_number(*floor)
                ),
                None,
            ),
            Refusal::ExtentNotFinite => (
                "the shape's extent is too large or not finite, so it can't be meshed".to_owned(),
                Some(UNUSABLE_GEOMETRY.to_owned()),
            ),
            Refusal::FacesUnmeshed { faces, count } => {
                // Named, a few; the rest counted, so the words stay short however many there
                // are (the error's elements name more).
                let mut named: Vec<String> = faces
                    .iter()
                    .take(FACES_IN_WORDS)
                    .map(|&f| name(ElementRef::face(f)))
                    .collect();
                let unnamed = count.saturating_sub(named.len() as u64);
                match unnamed {
                    0 => {}
                    1 => named.push("1 other face".to_owned()),
                    _ => named.push(format!("{unnamed} other faces")),
                }
                let these = match count {
                    1 => "this face",
                    _ => "these faces",
                };
                (
                    format!("{} couldn't be meshed", listed(&named)),
                    Some(format!(
                        "the shape isn't valid, and {these} of it can't be meshed: rebuild the shape from its inputs"
                    )),
                )
            }
            Refusal::NotFinite { quantity, value } => {
                (format!("the shape's {quantity} isn't finite ({value})"), None)
            }
            Refusal::ResultCheckMissing { inputs } => (
                format!("no result check for an operation on {inputs} shapes"),
                None,
            ),
            Refusal::MalformedHistory { defect } => (
                format!(
                    "the bridge reported a history {}",
                    match defect {
                        HistoryDefect::UnknownInput => "for an input the operation doesn't have",
                        HistoryDefect::WrongKind => "for an element of the wrong kind",
                        HistoryDefect::UnknownElement => "for an element the input doesn't have",
                        HistoryDefect::UnknownRelation => "with an unknown relation",
                    }
                ),
                Some("this is a bug in Silico's kernel bridge: report it".to_owned()),
            ),
            Refusal::NoEdgeToRefuse { dressing } => (
                format!("no edge to refuse the {} for", dressing.name()),
                None,
            ),
            Refusal::TestOperation { op } => (
                format!("{op} is a test operation, not answered by this worker"),
                None,
            ),
            Refusal::FpTrapsOn { mask, op } => (
                format!("floating-point traps are on (trap-enable bits {mask:#x}) after {op}"),
                None,
            ),
            Refusal::Internal { what } => (what.clone(), None),
        };
        Described {
            message,
            suggestion,
        }
    }
}

impl Operation {
    /// What OCCT couldn't do: `build the fillet`, `extrude the profile`, ...
    fn verb(&self) -> String {
        match self {
            Operation::Primitive { name } => format!("make the {name}"),
            Operation::Translate => "move the shape".to_owned(),
            Operation::Extrusion => "extrude the profile".to_owned(),
            Operation::Revolve => "revolve the profile".to_owned(),
            Operation::Boolean { op } => format!("build the {}", op.name().to_lowercase()),
            Operation::Dressing { dressing } => format!("build the {}", dressing.name()),
            Operation::Tessellate => "mesh the shape".to_owned(),
            Operation::Other => "do it".to_owned(),
        }
    }

    /// The operation as a noun: `extrusion`, `fillet`, `cut`, ...
    fn noun(&self) -> String {
        match self {
            Operation::Primitive { name } => name.clone(),
            Operation::Translate => "translation".to_owned(),
            Operation::Extrusion => "extrusion".to_owned(),
            Operation::Revolve => "revolve".to_owned(),
            Operation::Boolean { op } => op.name().to_lowercase(),
            Operation::Dressing { dressing } => dressing.name().to_owned(),
            Operation::Tessellate => "mesh".to_owned(),
            Operation::Other => "operation".to_owned(),
        }
    }
}

impl Role {
    /// `base` or `tool`, as the boolean's fields are named.
    pub fn name(self) -> &'static str {
        match self {
            Role::Base => "base",
            Role::Tool => "tool",
        }
    }
}

impl DressingKind {
    /// `fillet` or `chamfer`.
    pub fn name(self) -> &'static str {
        match self {
            DressingKind::Fillet => "fillet",
            DressingKind::Chamfer { .. } => "chamfer",
        }
    }

    /// What to suggest trying: a smaller size, or another.
    fn sizes(self) -> (&'static str, &'static str) {
        match self {
            DressingKind::Fillet => ("a smaller radius", "another radius"),
            DressingKind::Chamfer {
                kind: ChamferKind::Equal,
            } => ("a smaller size", "another size"),
            DressingKind::Chamfer {
                kind: ChamferKind::TwoDistances,
            } => ("smaller distances", "other distances"),
            DressingKind::Chamfer {
                kind: ChamferKind::DistanceAngle,
            } => ("a smaller distance or angle", "another distance or angle"),
        }
    }
}

/// What to try when OCCT threw doing `operation`.
fn occt_advice(
    operation: &Operation,
    edges: &[u32],
    cause: Option<&str>,
    edge: &dyn Fn(u32) -> String,
) -> Option<String> {
    match operation {
        Operation::Extrusion | Operation::Revolve => sweep_advice(operation),
        Operation::Boolean { .. } => {
            Some("check that the shapes are solids that overlap".to_owned())
        }
        Operation::Dressing { dressing } => {
            // OCCT's "ChFi3d_Builder:only 2 faces", thrown only while taking an edge that
            // ends, or whose chain ends, on one joining more than two faces (where solids
            // touch along an edge): it fails whatever the size.
            let touching = cause.is_some_and(|c| c.contains("only 2 faces"));
            Some(blame(*dressing, edges, touching, edge))
        }
        Operation::Tessellate => Some(UNUSABLE_GEOMETRY.to_owned()),
        Operation::Primitive { .. } | Operation::Translate | Operation::Other => None,
    }
}

/// What to try when OCCT's result of `operation` was refused as invalid.
fn invalid_result_advice(
    operation: &Operation,
    edges: &[u32],
    edge: &dyn Fn(u32) -> String,
) -> Option<String> {
    match operation {
        Operation::Primitive { name } => {
            Some(format!("give the {name} sizes nearer the model's scale"))
        }
        Operation::Extrusion | Operation::Revolve => sweep_advice(operation),
        Operation::Boolean { .. } => Some(NEARLY_MEET.to_owned()),
        Operation::Dressing { dressing } => Some(blame(*dressing, edges, false, edge)),
        Operation::Translate | Operation::Tessellate | Operation::Other => None,
    }
}

/// What to try when OCCT couldn't sweep a profile, or its sweep was refused.
fn sweep_advice(operation: &Operation) -> Option<String> {
    match operation {
        Operation::Extrusion => Some(
            "OCCT couldn't extrude this profile: change its length or direction slightly".to_owned(),
        ),
        Operation::Revolve => Some(
            "OCCT couldn't revolve this profile: move it slightly further from the axis, or onto it"
                .to_owned(),
        ),
        _ => None,
    }
}

/// What a failed fillet or chamfer suggests about its edges: when there's one, it's the
/// one at fault; OCCT doesn't say which of several it was, or why. A size too large for the
/// faces beside the edge fails, but so do some small ones (on a cone's side) and some edges
/// at every size, so the suggestion offers a smaller size, another, and leaving the edge
/// out; or, for an edge `touching` where more than two faces meet, leaving it out.
fn blame(
    dressing: DressingKind,
    edges: &[u32],
    touching: bool,
    edge: &dyn Fn(u32) -> String,
) -> String {
    let operation = dressing.name();
    let (smaller, instead) = dressing.sizes();
    match edges {
        [one] if touching => format!(
            "{}, or an edge it continues smoothly into, ends where more than two faces meet (where solids touch along an edge), and OCCT can't {operation} it there: leave it out",
            edge(*one)
        ),
        [_] => format!(
            "if the {operation} runs past a face beside the edge, try {smaller}; if not, try {instead}, or leave the edge out: OCCT can't {operation} some edges at any size"
        ),
        _ => format!(
            "{operation} the edges one at a time to find the one that fails; then, if its {operation} runs past a face beside it, try {smaller}, and if not, {instead}, or leave it out"
        ),
    }
}

/// What to try when OCCT named the edges it couldn't fillet or chamfer.
fn failed_advice(dressing: DressingKind, edges: &[u32]) -> String {
    let operation = dressing.name();
    let (smaller, instead) = dressing.sizes();
    let (beside, out) = match edges {
        [_] => ("the edge", "the edge"),
        _ => ("these edges", "them"),
    };
    format!(
        "if the {operation} runs past a face beside {beside}, try {smaller}; if not, try {instead}, or leave {out} out: OCCT can't {operation} some edges at any size"
    )
}

/// The refusal of edges OCCT can't fillet or chamfer: why for the first, and every one
/// named, in the message and in the suggestion, so one refusal says everything to leave out.
fn refused_edges(
    dressing: DressingKind,
    troubles: &[EdgeTrouble],
    edge: &dyn Fn(u32) -> String,
) -> Described {
    let operation = dressing.name();
    let Some(&EdgeTrouble {
        edge: first,
        trouble,
    }) = troubles.first()
    else {
        return Refusal::NoEdgeToRefuse { dressing }.describe(None, &|e| e.to_string());
    };
    let named: Vec<String> = troubles.iter().map(|t| edge(t.edge)).collect();
    let first = edge(first);
    let at_a_point = "where the shape touches itself or another solid at a point";
    let mut message = match trouble {
        Trouble::Degenerate => format!(
            "{first} is degenerate (a point where a surface closes, such as a sphere's pole or a cone's tip), so there's no corner to {operation}"
        ),
        Trouble::NoFace => format!("{first} is on no face, so there's no corner to {operation}"),
        Trouble::OneFace => format!(
            "{first} is on only one face (the shape is open there), so there's no corner to {operation}"
        ),
        Trouble::ManyFaces => format!(
            "{first} joins more than two faces (where solids touch along an edge), and OCCT can't {operation} it"
        ),
        Trouble::SharedCorner => {
            format!("{first} ends at a corner {at_a_point}, and OCCT can't {operation} it there")
        }
        Trouble::ChainToSharedCorner { far } => format!(
            "{first} continues smoothly into {}, which ends at a corner {at_a_point}, and OCCT can't {operation} there",
            edge(far)
        ),
        Trouble::Smooth => format!(
            "{first} joins faces OCCT treats as tangent there (recorded as tangent, or turning by less than 0.1 radian, 5.7°), or a face to itself at a seam: too shallow a corner for OCCT to {operation}"
        ),
    };
    if let Some(others) = named.get(1..)
        && !others.is_empty()
    {
        message.push_str(&format!(
            "; OCCT won't {operation} {} either",
            others.join(", ")
        ));
    }
    Described {
        message,
        suggestion: Some(format!("leave {} out of the {operation}", listed(&named))),
    }
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

    /// One of every refusal, with elements where it has them.
    fn every_refusal() -> Vec<Refusal> {
        let fillet = DressingKind::Fillet;
        let chamfer = DressingKind::Chamfer {
            kind: ChamferKind::DistanceAngle,
        };
        vec![
            Refusal::NoShape,
            Refusal::NothingReturned {
                what: Returned::Mesh,
            },
            Refusal::Occt {
                operation: Operation::Dressing { dressing: fillet },
                edges: vec![4],
            },
            Refusal::ResultInvalid {
                operation: Operation::Dressing { dressing: chamfer },
                edges: vec![4],
            },
            Refusal::EdgesFailed {
                dressing: fillet,
                edges: vec![2, 10],
            },
            Refusal::BrepTooShort,
            Refusal::BrepCorrupted,
            Refusal::BrepDefect {
                defect: BrepDefect::FaceWithoutSurface,
            },
            Refusal::UnknownBrepDefect { code: 9 },
            Refusal::InvalidParameter {
                parameter: "radius".to_owned(),
                message: "the fillet's radius must be a length above 1e-7 mm".to_owned(),
            },
            Refusal::CornersMerged {
                corners: 5,
                sides: 4,
                first: 2,
                second: 3,
            },
            Refusal::BooleanInputEmpty {
                op: BooleanOp::Cut,
                role: Role::Tool,
            },
            Refusal::BooleanResultEmpty {
                op: BooleanOp::Common,
            },
            Refusal::NoEdges { dressing: chamfer },
            Refusal::EdgeMissing {
                edge: 13,
                count: 12,
            },
            Refusal::EdgeTwice { edge: 2 },
            Refusal::EdgesRefused {
                dressing: fillet,
                troubles: vec![
                    EdgeTrouble {
                        edge: 3,
                        trouble: Trouble::ChainToSharedCorner { far: 9 },
                    },
                    EdgeTrouble {
                        edge: 5,
                        trouble: Trouble::Smooth,
                    },
                ],
            },
            Refusal::DeflectionInvalid { deflection: 0.0 },
            Refusal::DeflectionTooFine {
                deflection: 1e-9,
                floor: 1e-3,
            },
            Refusal::ExtentNotFinite,
            Refusal::FacesUnmeshed {
                faces: vec![7, 9],
                count: 2,
            },
            Refusal::NotFinite {
                quantity: "volume".to_owned(),
                value: f64::INFINITY,
            },
            Refusal::ResultCheckMissing { inputs: 3 },
            Refusal::MalformedHistory {
                defect: HistoryDefect::WrongKind,
            },
            Refusal::NoEdgeToRefuse { dressing: fillet },
            Refusal::TestOperation {
                op: "DebugSleep".to_owned(),
            },
            Refusal::FpTrapsOn {
                mask: 0x200,
                op: "Fillet".to_owned(),
            },
            Refusal::Internal {
                what: "socket closed".to_owned(),
            },
        ]
    }

    #[test]
    fn every_refusal_crosses_the_wire_and_says_something() {
        for refusal in every_refusal() {
            let bytes = postcard::to_allocvec(&refusal).unwrap();
            let back: Refusal = postcard::from_bytes(&bytes).unwrap();
            assert_eq!(back, refusal);
            let said = refusal.describe(None, &|e| e.to_string());
            assert!(!said.message.is_empty(), "{refusal:?}");
        }
    }

    #[test]
    fn the_elements_at_fault_are_named_only_as_the_namer_names_them() {
        // The core names elements (later, as the document does): every name in the words
        // comes from the namer it gives, and every element at fault is named.
        let name = |e: ElementRef| format!("<{e}>");
        for refusal in every_refusal() {
            let said = refusal.describe(Some("ChFi3d_Builder:only 2 faces"), &name);
            let text = format!("{} {}", said.message, said.suggestion.unwrap_or_default());
            let bare = text.replace('<', " <");
            for kind in ["Edge", "Face", "Vertex"] {
                for word in bare.split_whitespace() {
                    assert!(
                        !word.starts_with(kind) || word.starts_with('<'),
                        "{refusal:?} names {word} itself: {text}"
                    );
                }
            }
            // Every element at fault is named; but OCCT failing on edges names none in its
            // words (the error lists it), and a profile's merged corners are named by their
            // number in the request, as the request numbers them.
            let unnamed = matches!(
                refusal,
                Refusal::ResultInvalid { .. }
                    | Refusal::Occt { .. }
                    | Refusal::CornersMerged { .. }
            );
            for element in refusal.elements() {
                if !unnamed {
                    assert!(text.contains(&name(element)), "{refusal:?}: {text}");
                }
            }
        }
    }

    #[test]
    fn every_refusal_is_the_kind_of_failure_it_was_before_it_was_data() {
        // From each refusal's kind before it was data (b4f2524), where the kernel built it
        // (invalid_input, new: failed, of_kind: invalid result or internal), not from
        // `kind()`.
        use FailureKind::{Internal, InvalidInput, InvalidResult, KernelFailed, UnknownOp};
        let kinds = [
            KernelFailed,  // NoShape
            KernelFailed,  // NothingReturned
            KernelFailed,  // Occt
            InvalidResult, // ResultInvalid
            KernelFailed,  // EdgesFailed (new: OCCT's failure, as Occt)
            InvalidInput,  // BrepTooShort
            InvalidInput,  // BrepCorrupted
            InvalidInput,  // BrepDefect
            KernelFailed,  // UnknownBrepDefect
            InvalidInput,  // InvalidParameter
            InvalidInput,  // CornersMerged
            InvalidInput,  // BooleanInputEmpty
            KernelFailed,  // BooleanResultEmpty
            InvalidInput,  // NoEdges
            InvalidInput,  // EdgeMissing
            InvalidInput,  // EdgeTwice
            InvalidInput,  // EdgesRefused
            InvalidInput,  // DeflectionInvalid
            InvalidInput,  // DeflectionTooFine
            KernelFailed,  // ExtentNotFinite
            KernelFailed,  // FacesUnmeshed
            KernelFailed,  // NotFinite
            Internal,      // ResultCheckMissing
            Internal,      // MalformedHistory
            Internal,      // NoEdgeToRefuse
            UnknownOp,     // TestOperation
            Internal,      // FpTrapsOn
            Internal,      // Internal
        ];
        let refusals = every_refusal();
        assert_eq!(refusals.len(), kinds.len());
        for (refusal, kind) in refusals.iter().zip(kinds) {
            assert_eq!(refusal.kind(), kind, "{refusal:?}");
        }
    }

    #[test]
    fn every_refusal_of_a_value_names_the_parameter_at_fault() {
        // One for each of `every_refusal`, in its order: the field of the operation a value
        // was refused for, as the request names it; none for anything else.
        let parameters = [
            None,               // NoShape
            None,               // NothingReturned
            None,               // Occt
            None,               // ResultInvalid
            None,               // EdgesFailed
            None,               // BrepTooShort
            None,               // BrepCorrupted
            None,               // BrepDefect
            None,               // UnknownBrepDefect
            Some("radius"),     // InvalidParameter
            Some("profile"),    // CornersMerged
            Some("tool"),       // BooleanInputEmpty
            None,               // BooleanResultEmpty
            Some("edges"),      // NoEdges
            Some("edges"),      // EdgeMissing
            Some("edges"),      // EdgeTwice
            Some("edges"),      // EdgesRefused
            Some("deflection"), // DeflectionInvalid
            Some("deflection"), // DeflectionTooFine
            None,               // ExtentNotFinite
            None,               // FacesUnmeshed
            None,               // NotFinite
            None,               // ResultCheckMissing
            None,               // MalformedHistory
            None,               // NoEdgeToRefuse
            None,               // TestOperation
            None,               // FpTrapsOn
            None,               // Internal
        ];
        let refusals = every_refusal();
        assert_eq!(refusals.len(), parameters.len());
        for (refusal, parameter) in refusals.iter().zip(parameters) {
            assert_eq!(refusal.parameter(), parameter, "{refusal:?}");
        }
        let base = Refusal::BooleanInputEmpty {
            op: BooleanOp::Fuse,
            role: Role::Base,
        };
        assert_eq!(base.parameter(), Some("base"));
    }

    #[test]
    fn a_failed_operation_suggests_what_fits_it() {
        let said = |refusal: Refusal| {
            refusal
                .describe(None, &|e| e.to_string())
                .suggestion
                .unwrap_or_default()
        };
        let occt = |operation| Refusal::Occt {
            operation,
            edges: Vec::new(),
        };
        let invalid = |operation| Refusal::ResultInvalid {
            operation,
            edges: Vec::new(),
        };
        let extrude = "OCCT couldn't extrude this profile: change its length or direction slightly";
        let revolve = "OCCT couldn't revolve this profile: move it slightly further from the axis, or onto it";
        assert_eq!(said(occt(Operation::Extrusion)), extrude);
        assert_eq!(said(invalid(Operation::Extrusion)), extrude);
        assert_eq!(said(occt(Operation::Revolve)), revolve);
        assert_eq!(said(invalid(Operation::Revolve)), revolve);
        assert_eq!(said(occt(Operation::Tessellate)), UNUSABLE_GEOMETRY);
        assert_eq!(
            said(occt(Operation::Boolean { op: BooleanOp::Cut })),
            "check that the shapes are solids that overlap"
        );
        assert_eq!(
            said(invalid(Operation::Boolean { op: BooleanOp::Cut })),
            NEARLY_MEET
        );
        assert_eq!(
            said(invalid(Operation::Primitive {
                name: "sphere".to_owned()
            })),
            "give the sphere sizes nearer the model's scale"
        );
        // An offset refused: how far.
        assert_eq!(
            said(Refusal::InvalidParameter {
                parameter: "offset.y".to_owned(),
                message: String::new(),
            }),
            OFFSET
        );
        assert_eq!(
            said(Refusal::InvalidParameter {
                parameter: "offsets".to_owned(),
                message: String::new(),
            }),
            ""
        );
        // OCCT failing, with no words of its own: what it couldn't do.
        let message = |operation| occt(operation).describe(None, &|e| e.to_string()).message;
        assert_eq!(
            message(Operation::Extrusion),
            "OCCT couldn't extrude the profile"
        );
        assert_eq!(
            message(Operation::Dressing {
                dressing: DressingKind::Fillet
            }),
            "OCCT couldn't build the fillet"
        );
        assert_eq!(
            message(Operation::Tessellate),
            "OCCT couldn't mesh the shape"
        );
    }

    #[test]
    fn an_edge_the_shape_doesnt_have_is_written_by_its_number_without_the_namer() {
        let asked = std::cell::RefCell::new(Vec::new());
        let name = |e: ElementRef| {
            asked.borrow_mut().push(e);
            format!("<{e}>")
        };
        for (edge, count, words) in [
            (13, 12, "edge 13 doesn't exist: the shape has edges 1 to 12"),
            (0, 12, "edge 0 doesn't exist: the shape has edges 1 to 12"),
            (1, 0, "edge 1 doesn't exist: the shape has no edges"),
            (
                u32::MAX,
                1,
                "edge 4294967295 doesn't exist: the shape has edges 1 to 1",
            ),
        ] {
            let said = Refusal::EdgeMissing { edge, count }.describe(None, &name);
            assert_eq!(said.message, words);
            assert_eq!(said.suggestion, None);
        }
        assert!(asked.borrow().is_empty(), "named {:?}", asked.borrow());
    }

    #[test]
    fn faces_that_cant_be_meshed_are_each_named() {
        let said = |faces: Vec<u32>| {
            let count = faces.len() as u64;
            let refusal = Refusal::FacesUnmeshed { faces, count };
            let elements = refusal.elements();
            (refusal.describe(None, &|e| e.to_string()), elements)
        };
        let (one, elements) = said(vec![3]);
        assert_eq!(one.message, "Face3 couldn't be meshed");
        assert_eq!(
            one.suggestion.as_deref(),
            Some(
                "the shape isn't valid, and this face of it can't be meshed: rebuild the shape from its inputs"
            )
        );
        assert_eq!(elements, [ElementRef::face(3)]);
        let (two, elements) = said(vec![3, 8]);
        assert_eq!(two.message, "Face3 and Face8 couldn't be meshed");
        assert!(
            two.suggestion
                .is_some_and(|s| s.contains("these faces of it can't be meshed"))
        );
        assert_eq!(elements, [ElementRef::face(3), ElementRef::face(8)]);
        // Many: a few named in the words, the rest counted; the first hundred are elements.
        let many = Refusal::FacesUnmeshed {
            faces: (1..=100).collect(),
            count: 1_000_000,
        };
        let words = many.describe(None, &|e| e.to_string()).message;
        assert_eq!(
            words,
            "Face1, Face2, Face3, Face4, Face5, Face6, Face7, Face8, and 999992 other faces couldn't be meshed"
        );
        assert_eq!(many.elements().len(), FACES_NAMED);
        assert_eq!(many.elements()[99], ElementRef::face(100));
        // One beyond those named in the words is one face.
        let nine = Refusal::FacesUnmeshed {
            faces: (1..=9).collect(),
            count: 9,
        };
        assert_eq!(
            nine.describe(None, &|e| e.to_string()).message,
            "Face1, Face2, Face3, Face4, Face5, Face6, Face7, Face8, and 1 other face couldn't be meshed"
        );
    }

    #[test]
    fn the_edges_occt_couldnt_fillet_are_each_named() {
        let said = |edges: Vec<u32>| {
            let refusal = Refusal::EdgesFailed {
                dressing: DressingKind::Fillet,
                edges,
            };
            (
                refusal.describe(None, &|e| e.to_string()),
                refusal.elements(),
            )
        };
        let (one, elements) = said(vec![2]);
        assert_eq!(one.message, "OCCT couldn't fillet Edge2");
        assert_eq!(
            one.suggestion.as_deref(),
            Some(
                "if the fillet runs past a face beside the edge, try a smaller radius; if not, try another radius, or leave the edge out: OCCT can't fillet some edges at any size"
            )
        );
        assert_eq!(elements, [ElementRef::edge(2)]);
        let (three, elements) = said(vec![2, 4, 10]);
        assert_eq!(
            three.message,
            "OCCT couldn't fillet Edge2, Edge4, and Edge10"
        );
        assert_eq!(
            three.suggestion.as_deref(),
            Some(
                "if the fillet runs past a face beside these edges, try a smaller radius; if not, try another radius, or leave them out: OCCT can't fillet some edges at any size"
            )
        );
        assert_eq!(
            elements,
            [
                ElementRef::edge(2),
                ElementRef::edge(4),
                ElementRef::edge(10)
            ]
        );
    }

    #[test]
    fn every_edge_refused_is_an_element_at_fault() {
        let refused = Refusal::EdgesRefused {
            dressing: DressingKind::Fillet,
            troubles: vec![
                EdgeTrouble {
                    edge: 3,
                    trouble: Trouble::Smooth,
                },
                EdgeTrouble {
                    edge: 5,
                    trouble: Trouble::OneFace,
                },
            ],
        };
        assert_eq!(
            refused.elements(),
            [ElementRef::edge(3), ElementRef::edge(5)]
        );
        // OCCT failing on one edge blames it; on several, it doesn't say which.
        let on = |edges: Vec<u32>| Refusal::Occt {
            operation: Operation::Dressing {
                dressing: DressingKind::Fillet,
            },
            edges,
        };
        assert_eq!(on(vec![4]).elements(), [ElementRef::edge(4)]);
        assert!(on(vec![4, 6]).elements().is_empty());
    }
}
