//! `drag_begin`, `drag_move`, and `drag_end`'s wire types (docs/design/m5.0-app.md, section
//! 7, M5.3): their requests, read strictly ([`DragBeginRequest`], [`DragMoveRequest`],
//! [`DragEndRequest`]) and written back as a client writes them; their results, written
//! field for field ([`ToWire`] for [`DragBegun`], [`DragFrame`], and [`DragEnded`]), and the
//! drag `status` gives ([`DragOpen`]); and the mirrors their snapshots,
//! `commands/drag_begin.json`, `drag_move.json`, and `drag_end.json`, are generated from.
//!
//! A cursor or a press is two numbers in the sketch's coordinates, each read as a
//! [`Coordinate`] is, correctly rounded to the nearest `f64`; one past the largest `f64` is
//! read as infinite and refused by the drag's own check (M3.7's `check_drag_target`), naming
//! the field, as a Rust client's is, so the wire and a typed client meet one set of refusals.
//!
//! What a drag grabs is one shape with every field present (M4.1's *Requests are strict*):
//! `point`, or `curve` and `press`, the others `null`. Not two shapes: an MCP tool's input
//! schema is one object, never a choice at its root, which agents' clients refuse.

use schemars::JsonSchema;

use super::transaction::CommitResultSchema;
use super::{Json, RequestFields, ToWire, invalid, join, wire_shape, words, written};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{
    DragBegun, DragEnded, DragFrame, DragGrab, DragOpen, FrameEnd, Problems, SessionId,
};
use crate::document::json::Value;
use crate::document::{Coordinate, ElementId, FeatureId};
use crate::drag::Kept;

/// A `drag_begin` request: the session, the sketch, and what the drag grabs.
#[derive(Debug, Clone, PartialEq)]
pub struct DragBeginRequest {
    pub session: SessionId,
    pub sketch: FeatureId,
    pub grab: DragGrab,
}

impl DragBeginRequest {
    /// The request `node` holds, or every reason its shape is refused: `point`, or `curve`
    /// and `press`, given, and the others `null`.
    pub fn read(node: &crate::document::json::Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let sketch = f.feature("sketch");
            let point = nullable_element(f, "point");
            let curve = nullable_element(f, "curve");
            let press = nullable_cursor(f, "press");
            let grab = grabbed(f, point?, curve?, press?);
            Some(DragBeginRequest {
                session: session?,
                sketch: sketch?,
                grab: grab?,
            })
        })
    }
}

/// What a drag grabs, of a request's `point`, `curve`, and `press`: one of the two forms,
/// or refused, naming the field at fault.
fn grabbed(
    f: &mut RequestFields<'_>,
    point: Option<ElementId>,
    curve: Option<ElementId>,
    press: Option<[f64; 2]>,
) -> Option<DragGrab> {
    let refuse = |f: &mut RequestFields<'_>, key: &str, message: &str, suggestion: &str| {
        let at = join(&f.path, key);
        f.refuse(invalid(&at, message.to_owned()).with_suggestion(suggestion));
        None
    };
    match (point, curve, press) {
        (Some(point), None, None) => Some(DragGrab::Point(point)),
        (None, Some(curve), Some(press)) => Some(DragGrab::Edge { curve, press }),
        (Some(_), Some(_), _) => refuse(
            f,
            "curve",
            "the request names both a point and a curve, and a drag grabs one",
            "give point to drag a point, or curve and press to drag a curve's edge, the other null",
        ),
        (None, None, _) => refuse(
            f,
            "point",
            "the request names neither a point nor a curve, and a drag grabs one",
            "give point to drag a point, or curve and press to drag a curve's edge, the other null",
        ),
        (Some(_), None, Some(_)) => refuse(
            f,
            "press",
            "press is given with a point, and only a curve's edge is pressed",
            "give press null to drag a point",
        ),
        (None, Some(_), None) => refuse(
            f,
            "press",
            "press is null, and a drag of a curve's edge needs where the press on it landed",
            "give press, the point on the curve the press landed at, in the sketch's coordinates",
        ),
    }
}

/// The member's ID at `key`, or `None` inside for `null`: refused if it's neither.
fn nullable_element(f: &mut RequestFields<'_>, key: &str) -> Option<Option<ElementId>> {
    let node = f.take_nullable(key)?;
    let Some(node) = node else {
        return Some(None);
    };
    match &node.value {
        Value::String(text) if let Some(id) = ElementId::new(text) => Some(Some(id)),
        _ => {
            f.wrong(key, "a sketch member's ID (8 characters), or null", node);
            None
        }
    }
}

/// The cursor at `key`: two numbers, `[x, y]`, in the sketch's coordinates, each read
/// correctly rounded (one past the largest `f64` as infinite, for the drag's check to
/// refuse), a member at fault named by its place (`to.1`).
fn cursor(f: &mut RequestFields<'_>, key: &str) -> Option<[f64; 2]> {
    let node = f.take(key)?;
    cursor_of(f, key, node)
}

/// [`cursor`], or `None` inside for `null`.
fn nullable_cursor(f: &mut RequestFields<'_>, key: &str) -> Option<Option<[f64; 2]>> {
    match f.take_nullable(key)? {
        None => Some(None),
        Some(node) => cursor_of(f, key, node).map(Some),
    }
}

fn cursor_of(
    f: &mut RequestFields<'_>,
    key: &str,
    node: &crate::document::json::Node,
) -> Option<[f64; 2]> {
    let what = "two numbers [x, y], in the sketch's coordinates";
    let Value::Array(items) = &node.value else {
        f.wrong(key, what, node);
        return None;
    };
    let [x, y] = items.as_slice() else {
        let at = join(&f.path, key);
        f.refuse(invalid(
            &at,
            format!("{at} must be {what}, got {} of them", items.len()),
        ));
        return None;
    };
    let mut component = |i: usize, item: &crate::document::json::Node| {
        // Valid JSON number syntax always parses: past the largest `f64`, as infinite.
        let value = match &item.value {
            Value::Number(text) => text.parse::<f64>().ok(),
            _ => None,
        };
        if value.is_none() {
            let at = format!("{}.{i}", join(&f.path, key));
            f.refuse(invalid(
                &at,
                format!("{at} must be a number, got {}", written(item)),
            ));
        }
        value
    };
    let (x, y) = (component(0, x), component(1, y));
    Some([x?, y?])
}

/// Two numbers, as a cursor is written: each as a coordinate is.
fn pair(at: [f64; 2]) -> Json<'static> {
    Json::array(at.map(Json::coordinate))
}

/// What a drag grabs, as requests, results, and `status` write it: `point`, `curve`, and
/// `press`, each `null` where it doesn't apply.
fn grab_members(grab: &DragGrab) -> [(&'static str, Json<'_>); 3] {
    match grab {
        DragGrab::Point(point) => [
            ("point", Json::str(point.as_str())),
            ("curve", Json::null()),
            ("press", Json::null()),
        ],
        DragGrab::Edge { curve, press } => [
            ("point", Json::null()),
            ("curve", Json::str(curve.as_str())),
            ("press", pair(*press)),
        ],
    }
}

impl ToWire for DragBeginRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        let [point, curve, press] = grab_members(&self.grab);
        Json::object([
            ("session", self.session.to_wire()),
            ("sketch", self.sketch.to_wire()),
            point,
            curve,
            press,
        ])
    }
}

/// A `drag_move` request: the session, and the cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct DragMoveRequest {
    pub session: SessionId,
    pub to: [f64; 2],
}

impl DragMoveRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &crate::document::json::Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let to = cursor(f, "to");
            Some(DragMoveRequest {
                session: session?,
                to: to?,
            })
        })
    }
}

impl ToWire for DragMoveRequest {
    /// The request as a client writes it.
    fn to_wire(&self) -> Json<'_> {
        Json::object([("session", self.session.to_wire()), ("to", pair(self.to))])
    }
}

/// A `drag_end` request: the session, and the final cursor, or none to end the drag with no
/// edit (Esc).
#[derive(Debug, Clone, PartialEq)]
pub struct DragEndRequest {
    pub session: SessionId,
    pub to: Option<[f64; 2]>,
}

impl DragEndRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &crate::document::json::Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let to = nullable_cursor(f, "to");
            Some(DragEndRequest {
                session: session?,
                to: to?,
            })
        })
    }
}

impl ToWire for DragEndRequest {
    /// The request as a client writes it.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("to", Json::or_null(self.to, pair)),
        ])
    }
}

impl ToWire for DragBegun {
    /// `drag_begin`'s result: the sketch and what the drag grabbed, its degrees of freedom
    /// left, and the points of the part it moves.
    fn to_wire(&self) -> Json<'_> {
        let [point, curve, press] = grab_members(&self.grab);
        Json::object([
            ("sketch", self.sketch.to_wire()),
            point,
            curve,
            press,
            ("dofs", Json::integer(self.dofs.into())),
            (
                "moves",
                Json::array(self.moves.iter().map(|id| Json::str(id.as_str()))),
            ),
        ])
    }
}

impl ToWire for DragFrame {
    /// `drag_move`'s result: the cursor, how its frame ended, and the part's points and free
    /// radii (none but with `solved`).
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("to", pair(self.to)),
            ("frame", Json::str(self.frame.as_str())),
            (
                "points",
                Json::array(self.points.iter().map(|(id, [x, y])| {
                    Json::object([
                        ("id", Json::str(id.as_str())),
                        ("x", Json::coordinate(*x)),
                        ("y", Json::coordinate(*y)),
                    ])
                })),
            ),
            (
                "radii",
                Json::array(self.radii.iter().map(|(id, radius)| {
                    Json::object([
                        ("id", Json::str(id.as_str())),
                        ("radius", Json::coordinate(*radius)),
                    ])
                })),
            ),
        ])
    }
}

/// Which state a drag's end kept, as the wire says it.
const fn kept_word(kept: Kept) -> &'static str {
    match kept {
        Kept::Cursor => "cursor",
        Kept::Begin => "begin",
    }
}

impl ToWire for DragEnded {
    /// `drag_end`'s result: the sketch, the state kept, whether the document changed, and
    /// the commit outside a transaction.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("sketch", self.sketch.to_wire()),
            ("kept", Json::str(kept_word(self.kept))),
            ("changed", Json::bool(self.changed)),
            (
                "commit",
                Json::or_null(self.commit.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

impl ToWire for DragOpen {
    /// The drag `status` says is open: its sketch, and what it grabbed.
    fn to_wire(&self) -> Json<'_> {
        let [point, curve, press] = grab_members(&self.grab);
        Json::object([("sketch", self.sketch.to_wire()), point, curve, press])
    }
}

/// `drag_begin`'s request: begin a drag of a sketch's point, or of a curve's edge, on a
/// sketch worker of its own. It queues behind the session's commands before it, and drags
/// the sketch as the session holds it when it runs. One drag is open in a session at once.
#[derive(JsonSchema)]
#[schemars(rename = "DragBeginRequest", transform = wire_shape)]
pub struct DragBeginRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The sketch, by its feature's ID.
    pub sketch: FeatureId,
    /// The point to drag, by its ID (a line's end, an arc's start, end, or centre, a
    /// circle's centre, a lone point); null to drag a curve's edge.
    pub point: Option<ElementId>,
    /// The curve whose edge to drag, by its ID: a line moves whole, a circle or an arc is
    /// sized through the cursor. Null to drag a point.
    pub curve: Option<ElementId>,
    /// Where the press on the curve's edge landed, `[x, y]` in the sketch's coordinates:
    /// each move's offset is from it, and a release there is a click, which changes
    /// nothing. Null to drag a point.
    pub press: Option<[Coordinate; 2]>,
}

/// What a drag grabbed, as the request named it.
#[derive(JsonSchema)]
#[schemars(rename = "DragOpen", transform = wire_shape)]
pub struct DragOpenSchema {
    /// The sketch, by its feature's ID.
    pub sketch: FeatureId,
    /// The point dragged, by its ID; null for a curve's edge.
    pub point: Option<ElementId>,
    /// The curve whose edge is dragged, by its ID; null for a point.
    pub curve: Option<ElementId>,
    /// Where the press on the curve's edge landed, in the sketch's coordinates; null for a
    /// point.
    pub press: Option<[Coordinate; 2]>,
}

/// `drag_begin`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "DragBeginResult", transform = wire_shape)]
pub struct DragBeginResultSchema {
    /// The sketch, as the request named it.
    pub sketch: FeatureId,
    /// The point dragged, as the request named it; null for a curve's edge.
    pub point: Option<ElementId>,
    /// The curve whose edge is dragged, as the request named it; null for a point.
    pub curve: Option<ElementId>,
    /// The press, as the request gave it; null for a point.
    pub press: Option<[Coordinate; 2]>,
    /// The sketch's degrees of freedom left as it began. A point no constraint leaves free
    /// still begins a drag, and its frames don't move it.
    pub dofs: i64,
    /// The points of the part of the sketch the drag moves, by ID: every other point stays
    /// where it began. A diagnostic.
    pub moves: Vec<ElementId>,
}

/// `drag_move`'s request: move the drag's cursor. It runs beside the session's queue, and
/// only the newest move is solved: one replaced before it's solved answers `dropped`. Send
/// moves only once `drag_begin` has answered.
#[derive(JsonSchema)]
#[schemars(rename = "DragMoveRequest", transform = wire_shape)]
pub struct DragMoveRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The cursor, `[x, y]` in the sketch's coordinates.
    pub to: [Coordinate; 2],
}

/// How a drag's frame ended.
pub struct FrameEndSchema;

impl JsonSchema for FrameEndSchema {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FrameEnd".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        words(
            "How the frame ended: solved (the cursor solved for, the part's points and free \
             radii given); failed (the solver didn't reach the cursor; the drag goes on); \
             invalid (it reached geometry that can't be built); or dropped (a newer move \
             replaced it before it was solved). Only solved gives values: otherwise keep the \
             last frame.",
            [
                FrameEnd::Solved,
                FrameEnd::Failed,
                FrameEnd::Invalid,
                FrameEnd::Dropped,
            ]
            .map(FrameEnd::as_str),
        )
    }
}

/// A point of a drag's frame.
#[derive(JsonSchema)]
#[schemars(rename = "FramePoint", transform = wire_shape)]
pub struct FramePointSchema {
    /// The point's ID.
    pub id: ElementId,
    pub x: Coordinate,
    pub y: Coordinate,
}

/// A free radius of a drag's frame.
#[derive(JsonSchema)]
#[schemars(rename = "FrameRadius", transform = wire_shape)]
pub struct FrameRadiusSchema {
    /// The circle's or arc's ID.
    pub id: ElementId,
    pub radius: Coordinate,
}

/// `drag_move`'s result: a frame, a diagnostic, never a result (which moves are solved
/// depends on the worker's speed).
#[derive(JsonSchema)]
#[schemars(rename = "DragMoveResult", transform = wire_shape)]
pub struct DragMoveResultSchema {
    /// The cursor solved for, as the request gave it.
    pub to: [Coordinate; 2],
    pub frame: FrameEndSchema,
    /// With solved, every point of the part the drag moves, by ID; otherwise empty.
    pub points: Vec<FramePointSchema>,
    /// With solved, each circle and arc of the part whose radius is free, by ID; otherwise
    /// empty.
    pub radii: Vec<FrameRadiusSchema>,
}

/// `drag_end`'s request: end the drag at the final cursor, one edit at a new revision
/// (undoable inside a transaction, saved outside one), or with `to` null, with no edit
/// (Esc). It queues behind the session's commands before it.
#[derive(JsonSchema)]
#[schemars(rename = "DragEndRequest", transform = wire_shape)]
pub struct DragEndRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The final cursor, `[x, y]` in the sketch's coordinates: the sketch is moved there
    /// from where the drag began, then solved as a whole. Null to end the drag with no edit.
    pub to: Option<[Coordinate; 2]>,
}

/// Which state a drag's end kept.
pub struct KeptSchema;

impl JsonSchema for KeptSchema {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Kept".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        words(
            "Which state the drag's end kept: cursor (the sketch moved to the final cursor \
             and solved as a whole); or begin (the sketch as the drag began: to null, a \
             release where the drag began, or a final move or solve that failed).",
            [Kept::Cursor, Kept::Begin].map(kept_word),
        )
    }
}

/// `drag_end`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "DragEndResult", transform = wire_shape)]
pub struct DragEndResultSchema {
    /// The sketch dragged.
    pub sketch: FeatureId,
    pub kept: KeptSchema,
    /// Whether the document changed: one edit, at a new revision.
    pub changed: bool,
    /// Outside a transaction, the commit that saved the edit; null inside one, or if
    /// nothing changed.
    pub commit: Option<CommitResultSchema>,
}
