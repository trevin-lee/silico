//! A sketch's types (docs/design/m3.2-sketch-format.md, approved by the maintainer): points
//! the curves share (D2), lines, circles, and arcs, construction geometry (D7), and
//! constraints with every form and choice explicit (D4). Coordinates are the solver's and
//! written in full (D3); dimensions are lengths and angles, exact. Each list is written
//! sorted by ID (D5). Solving is M3.3's; this is what the file holds.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

use super::json::{Node, Value};
use super::model::{ElementName, FeatureId, ID_ALPHABET, ID_LENGTH};
use super::quantity::{Angle, Length};

/// A sketch: its plane, its geometry, and its constraints.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sketch {
    pub plane: Plane,
    /// The sketch's points, which its curves share: two curves that meet name the same
    /// point.
    pub points: Vec<SketchPoint>,
    pub lines: Vec<SketchLine>,
    pub circles: Vec<SketchCircle>,
    pub arcs: Vec<SketchArc>,
    pub constraints: Vec<SketchConstraint>,
}

impl Sketch {
    /// Every element's ID with what it is, lists in the format's order
    /// ([`ElementKind::ALL`]), each list in the order held.
    pub fn elements(&self) -> impl Iterator<Item = (&ElementId, ElementKind)> {
        ElementKind::ALL
            .into_iter()
            .flat_map(move |kind| self.ids(kind).into_iter().map(move |id| (id, kind)))
    }

    /// Whether `self` and `other` are the same sketch as its canonical text writes it: the
    /// same plane, and each list the same members once both are in ID order, as the writer
    /// sorts them (stably). So a sketch read from a file whose lists aren't in ID order is
    /// the sketch its canonical text reads as, as two texts that write it are one document.
    pub fn canonically_eq(&self, other: &Sketch) -> bool {
        // Taken apart whole, so a field added to a sketch is a compile error here until
        // it's compared too.
        let Sketch {
            plane,
            points,
            lines,
            circles,
            arcs,
            constraints,
        } = self;
        fn same<T: PartialEq>(a: &[T], b: &[T], id: impl Fn(&T) -> &ElementId) -> bool {
            a.len() == b.len() && in_id_order(a, &id) == in_id_order(b, &id)
        }
        *plane == other.plane
            && same(points, &other.points, |p| &p.id)
            && same(lines, &other.lines, |l| &l.id)
            && same(circles, &other.circles, |c| &c.id)
            && same(arcs, &other.arcs, |a| &a.id)
            && same(constraints, &other.constraints, |c| &c.id)
    }

    /// The IDs of the elements of `kind`, in the order held.
    pub fn ids(&self, kind: ElementKind) -> Vec<&ElementId> {
        match kind {
            ElementKind::Point => self.points.iter().map(|p| &p.id).collect(),
            ElementKind::Line => self.lines.iter().map(|l| &l.id).collect(),
            ElementKind::Circle => self.circles.iter().map(|c| &c.id).collect(),
            ElementKind::Arc => self.arcs.iter().map(|a| &a.id).collect(),
            ElementKind::Constraint => self.constraints.iter().map(|c| &c.id).collect(),
        }
    }
}

/// A sketch list's members in ID order, as the file writes them: stable, so members sharing
/// an ID (which the gate refuses) keep their order.
pub(super) fn in_id_order<T>(items: &[T], id: impl Fn(&T) -> &ElementId) -> Vec<&T> {
    let mut sorted: Vec<&T> = items.iter().collect();
    sorted.sort_by(|a, b| id(a).cmp(id(b)));
    sorted
}

/// The dotted paths of a sketch's members, as every error's `parameter` names them
/// (docs/design/m4.1-command-api.md, *Paths to a field* and decision 15): a member of a
/// list of objects that have an `id` (a sketch's points, lines, circles, arcs, and
/// constraints) by its ID, `constraints.g4h5j6k7`, and its field after it,
/// `constraints.g4h5j6k7.value`.
///
/// A member is named by its ID only when its `id` reads as an ID ([`member_id`]) and no
/// member before it in the sketch holds that ID: IDs are unique within a sketch, whatever
/// their kind, so "before" counts every list in the format's order ([`ElementKind::ALL`],
/// whatever order the file writes their keys in), each list in the order read, and the
/// IDs held start afresh with each sketch. Every other member, whose `id` is missing,
/// isn't an ID, or is an earlier member's, is named by its place in its list, from 0, with
/// no leading zeros (`constraints.3`), as any array that isn't such a list is.
///
/// So no two members share a path, and in a sketch's lists a segment of eight characters
/// is an ID and any shorter one a place: an ID names its first holder alone, and a place
/// is under 10,000,000, seven digits at most. A list the reader reads holds fewer members
/// than that: each is a value in the reader's tree, of at most [`MAX_TREE_BYTES`] (256
/// MiB), and a value takes a `json::Node` there, more than 26 bytes. An edit's sketch is
/// one the gate takes, within `MAX_SKETCH_GEOMETRY` and `MAX_SKETCH_CONSTRAINTS` (10,000
/// each, silico-protocol). A request names a sketch's members by ID only (decision 15):
/// a place path names a member only in an error about a document the reader or gate
/// refuses.
///
/// The reader, the gate, and drift name members through this, taking them in that order,
/// so a member has one path wherever it's named.
///
/// [`MAX_TREE_BYTES`]: super::json::MAX_TREE_BYTES
#[derive(Debug, Default)]
pub(crate) struct MemberPaths {
    /// The IDs earlier members hold.
    held: BTreeSet<ElementId>,
}

/// A member's path, and whether it's named by its ID (or, having no ID of its own, by
/// its place).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Named {
    pub(crate) path: String,
    pub(crate) by_id: bool,
}

impl MemberPaths {
    /// The path of the next member, at `index` in `kind`'s list, whose `id` reads as `id`
    /// (none if it's missing or isn't an ID); that ID held by it from here on.
    pub(crate) fn next(
        &mut self,
        kind: ElementKind,
        index: usize,
        id: Option<&ElementId>,
    ) -> Named {
        match id {
            Some(id) if !self.held.contains(id) => {
                self.held.insert(id.clone());
                Named {
                    path: format!("{}.{id}", kind.list()),
                    by_id: true,
                }
            }
            _ => Named {
                path: format!("{}.{index}", kind.list()),
                by_id: false,
            },
        }
    }

    /// Every member's path in `sketch`, each list's in the order held, the lists named in
    /// [`ElementKind::ALL`]'s order.
    pub(crate) fn of(sketch: &Sketch) -> SketchPaths {
        let mut paths = MemberPaths::default();
        let mut all = SketchPaths::default();
        for kind in ElementKind::ALL {
            let named = sketch
                .ids(kind)
                .into_iter()
                .enumerate()
                .map(|(i, id)| paths.next(kind, i, Some(id)))
                .collect();
            *all.list_mut(kind) = named;
        }
        all
    }
}

/// Every member's path in a sketch ([`MemberPaths::of`]): one list of paths for each of
/// the sketch's lists, each in the order held.
#[derive(Debug, Default)]
pub(crate) struct SketchPaths {
    pub(crate) points: Vec<Named>,
    pub(crate) lines: Vec<Named>,
    pub(crate) circles: Vec<Named>,
    pub(crate) arcs: Vec<Named>,
    pub(crate) constraints: Vec<Named>,
}

impl SketchPaths {
    /// The paths of `kind`'s list.
    pub(crate) fn list(&self, kind: ElementKind) -> &[Named] {
        match kind {
            ElementKind::Point => &self.points,
            ElementKind::Line => &self.lines,
            ElementKind::Circle => &self.circles,
            ElementKind::Arc => &self.arcs,
            ElementKind::Constraint => &self.constraints,
        }
    }

    fn list_mut(&mut self, kind: ElementKind) -> &mut Vec<Named> {
        match kind {
            ElementKind::Point => &mut self.points,
            ElementKind::Line => &mut self.lines,
            ElementKind::Circle => &mut self.circles,
            ElementKind::Arc => &mut self.arcs,
            ElementKind::Constraint => &mut self.constraints,
        }
    }
}

/// The ID a sketch member's object writes in its `id`, read as the reader reads every
/// sketch ID (a string, [`ElementId::new`]): none if the member isn't an object, has no
/// `id`, or its `id` isn't an ID.
pub(crate) fn member_id(node: &Node) -> Option<ElementId> {
    let Value::Object(members) = &node.value else {
        return None;
    };
    let id = members.iter().find(|m| m.key == "id")?;
    id.value.value.as_str().and_then(ElementId::new)
}

/// What a sketch's element is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ElementKind {
    Point,
    Line,
    Circle,
    Arc,
    Constraint,
}

impl ElementKind {
    /// Every kind, in the format's order: the order a sketch's lists are written in, read
    /// in, and counted in for its members' paths ([`MemberPaths`]).
    pub const ALL: [ElementKind; 5] = [
        ElementKind::Point,
        ElementKind::Line,
        ElementKind::Circle,
        ElementKind::Arc,
        ElementKind::Constraint,
    ];

    /// The element as messages name it: "point", "line", ...
    pub fn noun(self) -> &'static str {
        match self {
            ElementKind::Point => "point",
            ElementKind::Line => "line",
            ElementKind::Circle => "circle",
            ElementKind::Arc => "arc",
            ElementKind::Constraint => "constraint",
        }
    }

    /// The element with its article, as messages name one: "a point", "an arc", ...
    pub fn a_noun(self) -> &'static str {
        match self {
            ElementKind::Point => "a point",
            ElementKind::Line => "a line",
            ElementKind::Circle => "a circle",
            ElementKind::Arc => "an arc",
            ElementKind::Constraint => "a constraint",
        }
    }

    /// The list the file holds it in.
    pub fn list(self) -> &'static str {
        match self {
            ElementKind::Point => "points",
            ElementKind::Line => "lines",
            ElementKind::Circle => "circles",
            ElementKind::Arc => "arcs",
            ElementKind::Constraint => "constraints",
        }
    }
}

/// The plane a sketch is drawn on: one of the origin's, by its word (D6), or a planar face
/// of the model, by its name, with the axis the sketch's x follows and whether it's flipped
/// (docs/ports/attacher.md, F1 and F2; M5.9).
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(untagged)]
pub enum Plane {
    Origin(OriginPlane),
    Face(FacePlane),
}

impl Plane {
    /// The origin plane it is, if it's one.
    pub fn origin(&self) -> Option<OriginPlane> {
        match self {
            Plane::Origin(plane) => Some(*plane),
            Plane::Face(_) => None,
        }
    }

    /// The face plane it is, if it's one.
    pub fn face(&self) -> Option<&FacePlane> {
        match self {
            Plane::Origin(_) => None,
            Plane::Face(face) => Some(face),
        }
    }
}

impl From<OriginPlane> for Plane {
    fn from(plane: OriginPlane) -> Self {
        Plane::Origin(plane)
    }
}

/// One of the origin's planes, oriented as FreeCAD's (D6). On `xy`, the sketch's x is +X
/// and its y +Y (normal +Z); on `xz`, x is +X and y +Z (normal −Y); on `yz`, x is +Y and
/// y +Z (normal +X).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OriginPlane {
    Xy,
    Xz,
    Yz,
}

impl OriginPlane {
    /// As the file writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            OriginPlane::Xy => "xy",
            OriginPlane::Xz => "xz",
            OriginPlane::Yz => "yz",
        }
    }

    pub const ALL: [OriginPlane; 3] = [OriginPlane::Xy, OriginPlane::Xz, OriginPlane::Yz];
}

/// The plane as the kernel takes it, for faces made on it (`KernelRequest::make_faces`).
impl From<OriginPlane> for silico_protocol::SketchPlane {
    fn from(plane: OriginPlane) -> Self {
        match plane {
            OriginPlane::Xy => silico_protocol::SketchPlane::Xy,
            OriginPlane::Xz => silico_protocol::SketchPlane::Xz,
            OriginPlane::Yz => silico_protocol::SketchPlane::Yz,
        }
    }
}

/// A sketch on a planar face (docs/ports/attacher.md, *What Silico does*): the face, by its
/// name in Silico's form, found in the solids live at the sketch's place at each rebuild;
/// the global axis the sketch's x follows, projected onto the face; and whether the sketch
/// is flipped, its normal and x negated and its y kept. Where it then sits is the
/// rebuild's to say: nothing of the frame is stored.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FacePlane {
    /// The face, by its name, as Silico names it (`k7f2qx3m.box.Face6`), of a feature
    /// before the sketch.
    pub face: ElementName,
    /// The global axis the sketch's x follows: projected onto the face, it's the sketch's x.
    pub x: AxisToken,
    /// Whether the sketch is flipped: its normal into the face's solid, not out of it, and
    /// its x against the axis's projection; its y kept.
    pub reversed: bool,
}

impl FacePlane {
    /// The feature the face's name begins with: the face's own feature, the outermost of
    /// its name (`k7f2qx3m` of `k7f2qx3m.box.Face6`, and of a fuse's `k7f2qx3m.box.Face6`
    /// kept from its base), as `Name::parse` reads it of every name that parses. None for
    /// text that begins with no ID (which the gate refuses).
    pub fn feature(&self) -> Option<FeatureId> {
        let text = self.face.as_str();
        let id = text.get(..ID_LENGTH)?;
        if text.as_bytes().get(ID_LENGTH) != Some(&b'.') {
            return None;
        }
        FeatureId::new(id)
    }
}

/// A global axis, signed, as a sketch on a face names its x (F1): `+x`, `-x`, `+y`, `-y`,
/// `+z`, `-z`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AxisToken {
    PlusX,
    MinusX,
    PlusY,
    MinusY,
    PlusZ,
    MinusZ,
}

impl AxisToken {
    pub const ALL: [AxisToken; 6] = [
        AxisToken::PlusX,
        AxisToken::MinusX,
        AxisToken::PlusY,
        AxisToken::MinusY,
        AxisToken::PlusZ,
        AxisToken::MinusZ,
    ];

    pub fn word(self) -> &'static str {
        match self {
            AxisToken::PlusX => "+x",
            AxisToken::MinusX => "-x",
            AxisToken::PlusY => "+y",
            AxisToken::MinusY => "-y",
            AxisToken::PlusZ => "+z",
            AxisToken::MinusZ => "-z",
        }
    }

    /// The token `word` writes, if it's one.
    pub fn of_word(word: &str) -> Option<Self> {
        AxisToken::ALL
            .into_iter()
            .find(|token| token.word() == word)
    }

    /// The axis as a unit vector of model space.
    pub fn axis(self) -> [f64; 3] {
        match self {
            AxisToken::PlusX => [1.0, 0.0, 0.0],
            AxisToken::MinusX => [-1.0, 0.0, 0.0],
            AxisToken::PlusY => [0.0, 1.0, 0.0],
            AxisToken::MinusY => [0.0, -1.0, 0.0],
            AxisToken::PlusZ => [0.0, 0.0, 1.0],
            AxisToken::MinusZ => [0.0, 0.0, -1.0],
        }
    }

    /// The default x of a sketch on a planar face of outward normal `n` (F1: FreeCAD's legacy
    /// nearest-axis table, in global axes, docs/ports/attacher.md): the axis `n` is nearest,
    /// ties going to x, then y, then z; nearest x gives `+y` if `n` points along +x, else
    /// `-y`; nearest y gives `-x` if it points along +y, else `+x`; nearest z gives `+x`.
    pub fn default_x(n: [f64; 3]) -> Self {
        let [x, y, z] = n.map(f64::abs);
        if x >= y && x >= z {
            if n[0] > 0.0 {
                AxisToken::PlusY
            } else {
                AxisToken::MinusY
            }
        } else if y >= z {
            if n[1] > 0.0 {
                AxisToken::MinusX
            } else {
                AxisToken::PlusX
            }
        } else {
            AxisToken::PlusX
        }
    }
}

impl fmt::Display for AxisToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

impl JsonSchema for AxisToken {
    fn schema_name() -> Cow<'static, str> {
        "AxisToken".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A global axis, signed, as a sketch on a face names its x: projected onto the face, it's the sketch's x.",
            "type": "string",
            "enum": AxisToken::ALL.map(AxisToken::word)
        })
    }
}

/// A sketch element's ID: 8 characters of Crockford's base32 in lower case, as a feature's,
/// unique within its sketch (D5).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElementId(String);

impl ElementId {
    /// The ID `text` writes, if it's one.
    pub fn new(text: &str) -> Option<Self> {
        let ok = text.chars().count() == ID_LENGTH && text.chars().all(|c| ID_ALPHABET.contains(c));
        ok.then(|| ElementId(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ElementId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl JsonSchema for ElementId {
    fn schema_name() -> Cow<'static, str> {
        "ElementId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let pattern = format!("^[{ID_ALPHABET}]{{{ID_LENGTH}}}$");
        json_schema!({
            "description": "A sketch element's ID: 8 characters of Crockford's base32 in lower case (digits and letters but i, l, o, and u), unique within its sketch.",
            "type": "string",
            "pattern": pattern
        })
    }
}

/// What a constraint names: an element of its sketch, or one of the fixed references every
/// sketch has, the origin and the two axes (D6: FreeCAD's `RootPoint`, `H_Axis`, `V_Axis`).
/// `x_axis` runs from the origin along +x and `y_axis` along +y, as FreeCAD's axes do, so a
/// side of an axis and an angle from one mean what they mean for a line drawn that way.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reference {
    Element(ElementId),
    Origin,
    XAxis,
    YAxis,
}

impl Reference {
    pub const ORIGIN: &'static str = "origin";
    pub const X_AXIS: &'static str = "x_axis";
    pub const Y_AXIS: &'static str = "y_axis";

    /// The reference `text` writes, if it's one.
    pub fn new(text: &str) -> Option<Self> {
        match text {
            Self::ORIGIN => Some(Reference::Origin),
            Self::X_AXIS => Some(Reference::XAxis),
            Self::Y_AXIS => Some(Reference::YAxis),
            _ => ElementId::new(text).map(Reference::Element),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Reference::Element(id) => id.as_str(),
            Reference::Origin => Self::ORIGIN,
            Reference::XAxis => Self::X_AXIS,
            Reference::YAxis => Self::Y_AXIS,
        }
    }

    /// The element, if it's one of the sketch's own.
    pub fn element(&self) -> Option<&ElementId> {
        match self {
            Reference::Element(id) => Some(id),
            Reference::Origin | Reference::XAxis | Reference::YAxis => None,
        }
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl JsonSchema for Reference {
    fn schema_name() -> Cow<'static, str> {
        "SketchReference".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let pattern = format!("^([{ID_ALPHABET}]{{{ID_LENGTH}}}|origin|x_axis|y_axis)$");
        json_schema!({
            "description": "An element of this sketch, by its ID, or one of the fixed references every sketch has: origin, x_axis (from the origin along +x), y_axis (from the origin along +y).",
            "type": "string",
            "pattern": pattern
        })
    }
}

/// A value the solver works out (a point's coordinate, a circle's radius), in
/// millimetres: an `f64`, finite, written in full as Rust writes it (the shortest decimal
/// that reads back as the same `f64`, never an exponent) and read correctly rounded, so
/// the solver starts from exactly where it ended (D3). Two coordinates are equal when
/// their bits are: `-0` and `0` are written, and kept, apart.
#[derive(Debug, Clone, Copy)]
pub struct Coordinate(f64);

impl Coordinate {
    /// `value`, if it's finite.
    pub fn new(value: f64) -> Option<Self> {
        value.is_finite().then_some(Coordinate(value))
    }

    pub fn mm(self) -> f64 {
        self.0
    }

    /// The coordinate a number's text (valid JSON number syntax) writes, correctly
    /// rounded; none if it's past the largest `f64`.
    pub fn parse(text: &str) -> Option<Self> {
        text.parse::<f64>().ok().and_then(Coordinate::new)
    }
}

impl PartialEq for Coordinate {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Coordinate {}

impl fmt::Display for Coordinate {
    /// As the file writes it: Rust's shortest decimal that reads back as the same `f64`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl JsonSchema for Coordinate {
    fn schema_name() -> Cow<'static, str> {
        "Coordinate".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A value the solver works out, in millimetres: a number, read correctly rounded to the nearest 64-bit float, and written in full as the shortest decimal that reads back as it. Within a kilometre of the origin.",
            "type": "number"
        })
    }
}

/// A point of the sketch, which curves share.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SketchPoint {
    pub id: ElementId,
    pub x: Coordinate,
    pub y: Coordinate,
    /// Construction geometry is solved like any other and left out of the profile (D7).
    pub construction: bool,
}

/// A line from one point to another.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SketchLine {
    pub id: ElementId,
    pub from: ElementId,
    pub to: ElementId,
    pub construction: bool,
}

/// A circle about a point.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SketchCircle {
    pub id: ElementId,
    pub center: ElementId,
    /// Worked out by the solver, as coordinates are.
    pub radius: Coordinate,
    pub construction: bool,
}

/// An arc about `center`, counter-clockwise from `start` to `end`. Its points are its
/// definition: its radius and angles are the solver's, worked out from them, and the
/// solve holds its ends on it (D2, arcs).
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SketchArc {
    pub id: ElementId,
    pub center: ElementId,
    pub start: ElementId,
    pub end: ElementId,
    pub construction: bool,
}

/// One constraint: its ID and its form.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SketchConstraint {
    pub id: ElementId,
    #[serde(flatten)]
    pub form: Form,
}

/// Which side of a line, facing from its start (`from`) to its end (`to`); for an axis,
/// facing along it (+x, +y).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LineSide {
    Left,
    Right,
}

impl LineSide {
    pub const ALL: [LineSide; 2] = [LineSide::Left, LineSide::Right];

    /// As the file writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            LineSide::Left => "left",
            LineSide::Right => "right",
        }
    }
}

/// Where a tangent curve lies: for a line and a circle or arc, the side of the line the
/// curve is on (facing from its start to its end); for two circles or arcs, each outside
/// the other, or one inside the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TangentSide {
    Left,
    Right,
    Outside,
    Inside,
}

impl TangentSide {
    pub const ALL: [TangentSide; 4] = [
        TangentSide::Left,
        TangentSide::Right,
        TangentSide::Outside,
        TangentSide::Inside,
    ];

    /// As the file writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            TangentSide::Left => "left",
            TangentSide::Right => "right",
            TangentSide::Outside => "outside",
            TangentSide::Inside => "inside",
        }
    }
}

/// How two curves meeting at a point run there: on in the `same` direction (a smooth
/// join), or back the `opposite` way (a cusp).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Same,
    Opposite,
}

impl Direction {
    pub const ALL: [Direction; 2] = [Direction::Same, Direction::Opposite];

    /// As the file writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Same => "same",
            Direction::Opposite => "opposite",
        }
    }
}

/// A constraint's form: every form its own type, so none has optional references (D4).
/// A dimension's `driving` says whether the solver holds it (`true`) or it only reports
/// what the last solve measured (`false`).
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Form {
    /// The line parallel to x.
    Horizontal {
        line: Reference,
    },
    /// The line parallel to y.
    Vertical {
        line: Reference,
    },
    /// The two points level.
    HorizontalPoints {
        a: Reference,
        b: Reference,
    },
    /// One point above the other.
    VerticalPoints {
        a: Reference,
        b: Reference,
    },
    /// The line's length.
    Length {
        line: Reference,
        value: Length,
        driving: bool,
    },
    /// The distance between two points.
    Distance {
        a: Reference,
        b: Reference,
        value: Length,
        driving: bool,
    },
    /// x(b) − x(a), signed.
    DistanceX {
        a: Reference,
        b: Reference,
        value: Length,
        driving: bool,
    },
    /// y(b) − y(a), signed.
    DistanceY {
        a: Reference,
        b: Reference,
        value: Length,
        driving: bool,
    },
    /// The point's distance from the line (extended), on the given side of it.
    PointLineDistance {
        point: Reference,
        line: Reference,
        value: Length,
        side: LineSide,
        driving: bool,
    },
    /// The point on the curve: a line extended, or an arc's whole circle, as FreeCAD's.
    On {
        point: Reference,
        curve: Reference,
    },
    Parallel {
        a: Reference,
        b: Reference,
    },
    Perpendicular {
        a: Reference,
        b: Reference,
    },
    /// The angle from `a`'s direction to `b`'s, counter-clockwise, signed.
    Angle {
        a: Reference,
        b: Reference,
        value: Angle,
        driving: bool,
    },
    /// A circle's or arc's radius.
    Radius {
        curve: Reference,
        value: Length,
        driving: bool,
    },
    /// A circle's or arc's diameter.
    Diameter {
        curve: Reference,
        value: Length,
        driving: bool,
    },
    /// Two lines of equal length, or two circles or arcs of equal radius.
    Equal {
        a: Reference,
        b: Reference,
    },
    /// Tangent along their edges: a line and a circle or arc, or two circles or arcs.
    Tangent {
        a: Reference,
        b: Reference,
        side: TangentSide,
    },
    /// Tangent where `a` ends on `b`: the point `at`, an end of `a`, lies on `b`, and `a`
    /// runs there along `b`, its direction leaving `at` the `same` as `b`'s or `opposite`
    /// (FreeCAD's endpoint-to-edge `Tangent`).
    TangentOn {
        a: Reference,
        at: Reference,
        b: Reference,
        direction: Direction,
    },
    /// Tangent where two curves meet, at the point `at` they share.
    TangentAt {
        a: Reference,
        b: Reference,
        at: Reference,
        direction: Direction,
    },
    /// `a` and `b` mirror each other about a line or a point.
    Symmetric {
        a: Reference,
        b: Reference,
        about: Reference,
    },
    /// The point midway along the line.
    Midpoint {
        point: Reference,
        line: Reference,
    },
}

/// What a constraint's reference may name (the gate's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Takes {
    /// A point of the sketch, or the origin.
    Point,
    /// A point of the sketch that's an end of both curves the constraint names.
    SharedEnd,
    /// A point of the sketch that's an end of the first curve the constraint names, on the
    /// other (a tangent_on's `at`).
    EndOn,
    /// A line of the sketch.
    Line,
    /// A line of the sketch, or an axis.
    LineOrAxis,
    /// A line, circle, or arc of the sketch, or an axis.
    Curve,
    /// A circle or an arc of the sketch.
    Round,
    /// A line, circle, or arc of the sketch, whose end `at` is.
    EndedCurve,
    /// Two lines, or two circles or arcs (equal's).
    Equal,
    /// A line of the sketch, an axis, or a point (a symmetry's `about`).
    LineOrPoint,
}

impl Form {
    /// A dimension's value as the file writes it (a point–line distance's with its side);
    /// none for a form with no value.
    pub fn value_text(&self) -> Option<String> {
        match self {
            Form::Length { value, .. }
            | Form::Distance { value, .. }
            | Form::DistanceX { value, .. }
            | Form::DistanceY { value, .. }
            | Form::Radius { value, .. }
            | Form::Diameter { value, .. } => Some(value.to_string()),
            Form::PointLineDistance { value, side, .. } => {
                Some(format!("{value} {}", side.as_str()))
            }
            Form::Angle { value, .. } => Some(value.to_string()),
            _ => None,
        }
    }

    /// The `type` the file writes.
    pub fn type_name(&self) -> &'static str {
        match self {
            Form::Horizontal { .. } => "horizontal",
            Form::Vertical { .. } => "vertical",
            Form::HorizontalPoints { .. } => "horizontal_points",
            Form::VerticalPoints { .. } => "vertical_points",
            Form::Length { .. } => "length",
            Form::Distance { .. } => "distance",
            Form::DistanceX { .. } => "distance_x",
            Form::DistanceY { .. } => "distance_y",
            Form::PointLineDistance { .. } => "point_line_distance",
            Form::On { .. } => "on",
            Form::Parallel { .. } => "parallel",
            Form::Perpendicular { .. } => "perpendicular",
            Form::Angle { .. } => "angle",
            Form::Radius { .. } => "radius",
            Form::Diameter { .. } => "diameter",
            Form::Equal { .. } => "equal",
            Form::Tangent { .. } => "tangent",
            Form::TangentOn { .. } => "tangent_on",
            Form::TangentAt { .. } => "tangent_at",
            Form::Symmetric { .. } => "symmetric",
            Form::Midpoint { .. } => "midpoint",
        }
    }

    /// The form's references, in the order the file writes them, with what each may name.
    pub(crate) fn references(&self) -> Vec<(&'static str, &Reference, Takes)> {
        use Takes::*;
        match self {
            Form::Horizontal { line } | Form::Vertical { line } => vec![("line", line, Line)],
            Form::HorizontalPoints { a, b }
            | Form::VerticalPoints { a, b }
            | Form::Distance { a, b, .. }
            | Form::DistanceX { a, b, .. }
            | Form::DistanceY { a, b, .. } => vec![("a", a, Point), ("b", b, Point)],
            Form::Length { line, .. } => vec![("line", line, Line)],
            Form::PointLineDistance { point, line, .. } => {
                vec![("point", point, Point), ("line", line, LineOrAxis)]
            }
            Form::On { point, curve } => vec![("point", point, Point), ("curve", curve, Curve)],
            Form::Parallel { a, b } | Form::Perpendicular { a, b } | Form::Angle { a, b, .. } => {
                vec![("a", a, LineOrAxis), ("b", b, LineOrAxis)]
            }
            Form::Radius { curve, .. } | Form::Diameter { curve, .. } => {
                vec![("curve", curve, Round)]
            }
            Form::Equal { a, b } => vec![("a", a, Equal), ("b", b, Equal)],
            Form::Tangent { a, b, .. } => vec![("a", a, Curve), ("b", b, Curve)],
            Form::TangentOn { a, at, b, .. } => {
                vec![("a", a, EndedCurve), ("at", at, EndOn), ("b", b, Curve)]
            }
            Form::TangentAt { a, b, at, .. } => vec![
                ("a", a, EndedCurve),
                ("b", b, EndedCurve),
                ("at", at, SharedEnd),
            ],
            Form::Symmetric { a, b, about } => vec![
                ("a", a, Point),
                ("b", b, Point),
                ("about", about, LineOrPoint),
            ],
            Form::Midpoint { point, line } => vec![("point", point, Point), ("line", line, Line)],
        }
    }

    /// A length dimension's value, and whether it's signed (may be negative).
    pub fn length(&self) -> Option<(Length, bool)> {
        match *self {
            Form::Length { value, .. }
            | Form::Distance { value, .. }
            | Form::PointLineDistance { value, .. }
            | Form::Radius { value, .. }
            | Form::Diameter { value, .. } => Some((value, false)),
            Form::DistanceX { value, .. } | Form::DistanceY { value, .. } => Some((value, true)),
            _ => None,
        }
    }

    /// A dimension's `driving`; none for a form that isn't one.
    pub fn driving(&self) -> Option<bool> {
        match *self {
            Form::Length { driving, .. }
            | Form::Distance { driving, .. }
            | Form::DistanceX { driving, .. }
            | Form::DistanceY { driving, .. }
            | Form::PointLineDistance { driving, .. }
            | Form::Angle { driving, .. }
            | Form::Radius { driving, .. }
            | Form::Diameter { driving, .. } => Some(driving),
            _ => None,
        }
    }

    /// Whether the form is a dimension: exactly when it carries `driving`, reference
    /// dimensions included. Every other form is a relation.
    pub fn is_dimension(&self) -> bool {
        self.driving().is_some()
    }
}

/// Every constraint type, as the file writes it.
pub const CONSTRAINT_TYPES: [&str; 21] = [
    "horizontal",
    "vertical",
    "horizontal_points",
    "vertical_points",
    "length",
    "distance",
    "distance_x",
    "distance_y",
    "point_line_distance",
    "on",
    "parallel",
    "perpendicular",
    "angle",
    "radius",
    "diameter",
    "equal",
    "tangent",
    "tangent_on",
    "tangent_at",
    "symmetric",
    "midpoint",
];

#[cfg(test)]
mod tests {
    use super::super::json::{MAX_TREE_BYTES, Node};

    #[test]
    fn a_list_the_reader_reads_has_fewer_members_than_a_place_of_eight_digits() {
        // MemberPaths's reasoning: each member is a `Node` in a tree of at most
        // MAX_TREE_BYTES, so a place is at most seven digits, never an ID's eight.
        assert!(size_of::<Node>() > 26, "{}", size_of::<Node>());
        assert!(MAX_TREE_BYTES / size_of::<Node>() < 10_000_000);
    }
}
