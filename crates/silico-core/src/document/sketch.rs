//! A sketch's types (docs/design/m3.2-sketch-format.md, approved by the maintainer): points
//! the curves share (D2), lines, circles, and arcs, construction geometry (D7), and
//! constraints with every form and choice explicit (D4). Coordinates are the solver's and
//! written in full (D3); dimensions are lengths and angles, exact. Each list is written
//! sorted by ID (D5). Solving is M3.3's; this is what the file holds.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

use super::model::{ID_ALPHABET, ID_LENGTH};
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
    /// Every element's ID with what it is, lists in the file's order (points, lines,
    /// circles, arcs, constraints), each list in the order held.
    pub fn elements(&self) -> impl Iterator<Item = (&ElementId, ElementKind)> {
        let points = self.points.iter().map(|p| (&p.id, ElementKind::Point));
        let lines = self.lines.iter().map(|l| (&l.id, ElementKind::Line));
        let circles = self.circles.iter().map(|c| (&c.id, ElementKind::Circle));
        let arcs = self.arcs.iter().map(|a| (&a.id, ElementKind::Arc));
        let constraints = self
            .constraints
            .iter()
            .map(|c| (&c.id, ElementKind::Constraint));
        points
            .chain(lines)
            .chain(circles)
            .chain(arcs)
            .chain(constraints)
    }
}

/// The dotted paths of a sketch's members, as every error's `parameter` names them
/// (docs/design/m4.1-command-api.md, *Paths to a field* and decision 15): a member of a
/// list of objects that have an `id` (a sketch's points, lines, circles, arcs, and
/// constraints) by its ID, `constraints.g4h5j6k7`, and its field after it,
/// `constraints.g4h5j6k7.value`.
///
/// A member is named by its ID only when its `id` reads as an ID and no member before it
/// in the sketch holds that ID: IDs are unique within a sketch, whatever their kind, so
/// "before" counts every list in the format's order (points, lines, circles, arcs,
/// constraints, as [`Sketch::elements`] gives them, whatever order the file writes their
/// keys in), each list in the order read. Every other member, whose `id` is missing, isn't
/// an ID, or is an earlier member's, is named by its place in its list, from 0, with no
/// leading zeros (`constraints.3`), as any array that isn't such a list is. So no two
/// members share a path: an ID names its first holder alone, and a place, under 10,000,000
/// in any document the reader takes (16 MiB, at least three bytes a member), is never an
/// ID's eight characters.
///
/// The reader and the gate both name members through this, taking the members in that
/// order, so a member has one path wherever it's refused.
#[derive(Debug, Default)]
pub(crate) struct MemberPaths {
    /// The IDs earlier members hold.
    held: BTreeSet<ElementId>,
}

impl MemberPaths {
    /// The path of the next member, at `index` in `list`, whose `id` reads as `id` (none if
    /// it's missing or isn't an ID); that ID held by it from here on.
    pub(crate) fn next(&mut self, list: &str, index: usize, id: Option<&ElementId>) -> String {
        match id {
            Some(id) if !self.held.contains(id) => {
                self.held.insert(id.clone());
                format!("{list}.{id}")
            }
            _ => format!("{list}.{index}"),
        }
    }
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

/// The plane a sketch is drawn on: one of the origin's, oriented as FreeCAD's (D6). On
/// `xy`, the sketch's x is +X and its y +Y (normal +Z); on `xz`, x is +X and y +Z (normal
/// −Y); on `yz`, x is +Y and y +Z (normal +X).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Plane {
    Xy,
    Xz,
    Yz,
}

impl Plane {
    /// As the file writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Plane::Xy => "xy",
            Plane::Xz => "xz",
            Plane::Yz => "yz",
        }
    }

    pub const ALL: [Plane; 3] = [Plane::Xy, Plane::Xz, Plane::Yz];
}

/// The plane as the kernel takes it, for faces made on it (`KernelRequest::make_faces`).
impl From<Plane> for silico_protocol::SketchPlane {
    fn from(plane: Plane) -> Self {
        match plane {
            Plane::Xy => silico_protocol::SketchPlane::Xy,
            Plane::Xz => silico_protocol::SketchPlane::Xz,
            Plane::Yz => silico_protocol::SketchPlane::Yz,
        }
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
