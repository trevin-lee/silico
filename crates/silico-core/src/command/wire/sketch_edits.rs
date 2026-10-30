//! The sketch edits' wire types (docs/design/m5.0-app.md, section 7, *The sketch edits*):
//! their requests, read strictly ([`SketchAddRequest`], [`SketchRemoveRequest`],
//! [`SketchSplitRequest`], [`SketchTrimRequest`], [`SketchUnjoinRequest`],
//! [`SketchMergeRequest`], [`SketchExtendRequest`]) and written back as a client writes them; their results, written
//! field for field ([`ToWire`] for [`SketchEdited`] and [`SketchCut`]); and the mirrors their
//! snapshots, `commands/sketch_add.json` and the rest, are generated from. A request's shape
//! is read here; the members `sketch_add` gives are read by the document's own reader, in
//! the session, as `add_feature`'s feature is, so the wire and a Rust client meet one set of
//! refusals.

use schemars::JsonSchema;

use super::transaction::CommitResultSchema;
use super::{Json, RequestFields, ToWire, invalid, join, wire_shape, written};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{
    ExtendBy, NewMembers, Problems, Revision, SessionId, SketchCut, SketchEdited,
};
use crate::document::json::{Node, Value};
use crate::document::{
    Angle, Coordinate, ElementId, ElementKind, FeatureId, Length, Reference, SketchArc,
    SketchCircle, SketchConstraint, SketchLine, SketchPoint,
};
use crate::extend::End;

/// A `sketch_add` request: the session, the revision the edit was made against, the sketch,
/// and the members to add, each list as the document writes a sketch's.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchAddRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub sketch: FeatureId,
    pub members: NewMembers,
}

impl SketchAddRequest {
    /// The request `node` holds, or every reason its shape is refused: each list must be
    /// one, its members read by the document's reader when they're added.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let sketch = f.feature("sketch");
            let [points, lines, circles, arcs, constraints] =
                ElementKind::ALL.map(|kind| list(f, kind.list()));
            Some(SketchAddRequest {
                session: session?,
                revision: revision?,
                sketch: sketch?,
                members: NewMembers {
                    points: points?.clone(),
                    lines: lines?.clone(),
                    circles: circles?.clone(),
                    arcs: arcs?.clone(),
                    constraints: constraints?.clone(),
                },
            })
        })
    }
}

/// The list at `key`, taken whole, its items unread: refused if it's missing or isn't a
/// list.
fn list<'a>(f: &mut RequestFields<'a>, key: &str) -> Option<&'a Node> {
    let node = f.take(key)?;
    match node.value {
        Value::Array(_) => Some(node),
        _ => {
            f.wrong(key, format_args!("a list of {key}"), node);
            None
        }
    }
}

impl ToWire for SketchAddRequest {
    /// The request as a client writes it, every field in the shape's order, each member's
    /// as given.
    fn to_wire(&self) -> Json<'_> {
        let NewMembers {
            points,
            lines,
            circles,
            arcs,
            constraints,
        } = &self.members;
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("sketch", self.sketch.to_wire()),
            ("points", Json::node(points)),
            ("lines", Json::node(lines)),
            ("circles", Json::node(circles)),
            ("arcs", Json::node(arcs)),
            ("constraints", Json::node(constraints)),
        ])
    }
}

/// A `sketch_remove` request: the session, the revision the edit was made against, the
/// sketch, and its members to remove, by ID.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchRemoveRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub sketch: FeatureId,
    /// Each as named: a member's ID, or one of the fixed references, which the edit refuses
    /// in its own words (they can't be removed), not as a shape it doesn't know.
    pub members: Vec<Reference>,
}

impl SketchRemoveRequest {
    /// The request `node` holds, or every reason its shape is refused: `members` must be a
    /// list of members' IDs, each at fault named by its place (`members.0`).
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let sketch = f.feature("sketch");
            let members = references(f, "members");
            Some(SketchRemoveRequest {
                session: session?,
                revision: revision?,
                sketch: sketch?,
                members: members?,
            })
        })
    }
}

/// The list at `key` of members' IDs (or fixed references), each item at fault refused at
/// its place.
fn references(f: &mut RequestFields<'_>, key: &str) -> Option<Vec<Reference>> {
    let node = f.take(key)?;
    let Value::Array(items) = &node.value else {
        f.wrong(key, "a list of a sketch's members' IDs", node);
        return None;
    };
    let mut references = Vec::with_capacity(items.len());
    let mut fine = true;
    for (i, item) in items.iter().enumerate() {
        match &item.value {
            Value::String(text) if let Some(reference) = Reference::new(text) => {
                references.push(reference);
            }
            _ => {
                fine = false;
                f.refused.refuse_with(|| {
                    let at = format!("{}.{i}", join(&f.path, key));
                    invalid(
                        &at,
                        format!("{at} must be a sketch member's ID, got {}", written(item)),
                    )
                    .with_suggestion("name a point, curve, or constraint of the sketch by its ID")
                });
            }
        }
    }
    fine.then_some(references)
}

impl ToWire for SketchRemoveRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("sketch", self.sketch.to_wire()),
            (
                "members",
                Json::array(self.members.iter().map(|m| Json::str(m.as_str()))),
            ),
        ])
    }
}

impl ToWire for SketchEdited {
    /// A sketch edit's result: the sketch, the members added, modified, and removed, whether
    /// the document changed, and the commit outside a transaction.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("sketch", self.sketch.to_wire()),
            ("added", members(&self.added)),
            ("modified", members(&self.modified)),
            ("removed", members(&self.removed)),
            ("changed", Json::bool(self.changed)),
            (
                "commit",
                Json::or_null(self.commit.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

/// Members' IDs, in order.
fn members(ids: &[ElementId]) -> Json<'_> {
    Json::array(ids.iter().map(|id| Json::str(id.as_str())))
}

/// `sketch_add`'s request: add members to a sketch, as the document writes them, each with a
/// fresh random 8-character ID, never one a removal freed: a name a feature stores of the
/// removed member would find the new one.
#[derive(JsonSchema)]
#[schemars(rename = "SketchAddRequest", transform = wire_shape)]
pub struct SketchAddRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The sketch, by its feature's ID.
    pub sketch: FeatureId,
    /// The points to add, each with an ID the sketch doesn't hold, a fresh one.
    pub points: Vec<SketchPoint>,
    /// The lines to add: each may end at a point the sketch holds, which joins it to that
    /// point's curves, or at one of the points added.
    pub lines: Vec<SketchLine>,
    /// The circles to add.
    pub circles: Vec<SketchCircle>,
    /// The arcs to add.
    pub arcs: Vec<SketchArc>,
    /// The constraints to add, naming the sketch's members, the members added, or the
    /// fixed references. One that conflicts with others, or repeats them, is added all the
    /// same: `check` then reports the conflict, and the sketch's features fail until it's
    /// mended.
    pub constraints: Vec<SketchConstraint>,
}

/// `sketch_remove`'s request: remove members from a sketch, by ID.
#[derive(JsonSchema)]
#[schemars(rename = "SketchRemoveRequest", transform = wire_shape)]
pub struct SketchRemoveRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The sketch, by its feature's ID.
    pub sketch: FeatureId,
    /// The members to remove, judged as a set (a member named twice is taken once): a
    /// constraint; a curve, with each of its points no other curve uses and every
    /// constraint naming what goes; a point only if no curve uses it after the edit, so a
    /// point and the curves that use it go together.
    pub members: Vec<ElementId>,
}

/// `sketch_add`'s and `sketch_remove`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "SketchEditResult", transform = wire_shape)]
pub struct SketchEditResultSchema {
    /// The sketch, as the request named it.
    pub sketch: FeatureId,
    /// The members added, in the order the edit made them: by kind (points, lines,
    /// circles, arcs, constraints), each kind in the order given.
    pub added: Vec<ElementId>,
    /// The members changed in place (a removed line's midpoint, now a symmetric of its
    /// ends), by kind, then by ID.
    pub modified: Vec<ElementId>,
    /// The members removed, by kind, then by ID.
    pub removed: Vec<ElementId>,
    /// Whether the document changed: false for an empty batch, which makes no new revision
    /// and writes nothing.
    pub changed: bool,
    /// Outside a transaction, the commit that saved the edit; null inside one, or if
    /// nothing changed.
    pub commit: Option<CommitResultSchema>,
}

/// A `sketch_split` request: the session, the revision the edit was made against, the
/// sketch, the curve, where to split it (one position for a line or an arc, two for a
/// circle), and the new members' IDs, or none for the core's.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchSplitRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub sketch: FeatureId,
    pub curve: ElementId,
    pub at: Vec<[f64; 2]>,
    pub ids: Option<Vec<ElementId>>,
}

impl SketchSplitRequest {
    /// The request `node` holds, or every reason its shape is refused: `curve` a member's
    /// ID, `at` a list of positions, each two finite numbers, `ids` a list of members' IDs
    /// or null, each item at fault named by its place (`at.0.1`, `ids.2`). How many
    /// positions and IDs the split takes is the split's to say.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let sketch = f.feature("sketch");
            let curve = member(f, "curve", "a curve");
            let at = positions(f, "at");
            let ids = new_ids(f, "ids");
            Some(SketchSplitRequest {
                session: session?,
                revision: revision?,
                sketch: sketch?,
                curve: curve?,
                at: at?,
                ids: ids?,
            })
        })
    }
}

impl ToWire for SketchSplitRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("sketch", self.sketch.to_wire()),
            ("curve", Json::str(self.curve.as_str())),
            (
                "at",
                Json::array(self.at.iter().map(|&position| position_wire(position))),
            ),
            ("ids", ids_wire(self.ids.as_deref())),
        ])
    }
}

/// A `sketch_trim` request: the session, the revision the edit was made against, the
/// sketch, the curve, where the part to trim away was picked, and the new members' IDs, or
/// none for the core's.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchTrimRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub sketch: FeatureId,
    pub curve: ElementId,
    pub pick: [f64; 2],
    pub ids: Option<Vec<ElementId>>,
}

impl SketchTrimRequest {
    /// The request `node` holds, or every reason its shape is refused, as
    /// [`SketchSplitRequest::read`] refuses one: `pick` two finite numbers.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let sketch = f.feature("sketch");
            let curve = member(f, "curve", "a curve");
            let pick = position(f, "pick");
            let ids = new_ids(f, "ids");
            Some(SketchTrimRequest {
                session: session?,
                revision: revision?,
                sketch: sketch?,
                curve: curve?,
                pick: pick?,
                ids: ids?,
            })
        })
    }
}

impl ToWire for SketchTrimRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("sketch", self.sketch.to_wire()),
            ("curve", Json::str(self.curve.as_str())),
            ("pick", position_wire(self.pick)),
            ("ids", ids_wire(self.ids.as_deref())),
        ])
    }
}

/// A `sketch_unjoin` request: the session, the revision the edit was made against, the
/// sketch, the shared point, the curve to detach from it, and the new point's ID (a list of
/// one), or none for the core's.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchUnjoinRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub sketch: FeatureId,
    pub point: ElementId,
    pub curve: ElementId,
    pub ids: Option<Vec<ElementId>>,
}

impl SketchUnjoinRequest {
    /// The request `node` holds, or every reason its shape is refused: `point` and `curve`
    /// members' IDs, `ids` a list of members' IDs or null.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let sketch = f.feature("sketch");
            let point = member(f, "point", "a point");
            let curve = member(f, "curve", "a curve");
            let ids = new_ids(f, "ids");
            Some(SketchUnjoinRequest {
                session: session?,
                revision: revision?,
                sketch: sketch?,
                point: point?,
                curve: curve?,
                ids: ids?,
            })
        })
    }
}

impl ToWire for SketchUnjoinRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("sketch", self.sketch.to_wire()),
            ("point", Json::str(self.point.as_str())),
            ("curve", Json::str(self.curve.as_str())),
            ("ids", ids_wire(self.ids.as_deref())),
        ])
    }
}

/// A `sketch_merge` request: the session, the revision the edit was made against, the
/// sketch, and the points to make one, the one to keep first.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchMergeRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub sketch: FeatureId,
    pub points: Vec<ElementId>,
}

impl SketchMergeRequest {
    /// The request `node` holds, or every reason its shape is refused: `points` a list of
    /// members' IDs, each at fault named by its place (`points.1`). How many it takes is the
    /// merge's to say.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let sketch = f.feature("sketch");
            let points = members_list(f, "points", "a list of the sketch's points' IDs");
            Some(SketchMergeRequest {
                session: session?,
                revision: revision?,
                sketch: sketch?,
                points: points?,
            })
        })
    }
}

impl ToWire for SketchMergeRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("sketch", self.sketch.to_wire()),
            ("points", members(&self.points)),
        ])
    }
}

/// A `sketch_extend` request: the session, the revision the edit was made against, the
/// sketch, the line or arc, which end moves, and how far, or to where.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchExtendRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub sketch: FeatureId,
    pub curve: ElementId,
    pub end: End,
    pub by: ExtendBy,
}

/// The ends a `sketch_extend` names, each as written.
const ENDS: [(&str, End); 2] = [("start", End::Start), ("end", End::End)];

/// The kinds of `by`, each as written.
#[derive(Clone, Copy)]
enum ByKind {
    Length,
    Angle,
    Point,
    NextCurve,
}

const BY_KINDS: [(&str, ByKind); 4] = [
    ("length", ByKind::Length),
    ("angle", ByKind::Angle),
    ("point", ByKind::Point),
    ("next_curve", ByKind::NextCurve),
];

impl SketchExtendRequest {
    /// The request `node` holds, or every reason its shape is refused: `curve` a member's
    /// ID, `end` `start` or `end`, and `by` an object of a `type`, `length`, `angle`,
    /// `point`, or `next_curve`, with that kind's one field (none for `next_curve`), each
    /// at fault named by its path (`by.length`). Which kind the curve takes is the extend's
    /// to say.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let sketch = f.feature("sketch");
            let curve = member(f, "curve", "a line's or an arc's");
            let end = f.word("end", &ENDS, "ends");
            let by = f.object("by", |g| {
                let Some(kind) = g.word("type", &BY_KINDS, "kinds of extension") else {
                    // A kind refused: its fields are judged by no kind.
                    g.take_rest();
                    return None;
                };
                Some(match kind {
                    ByKind::Length => ExtendBy::Length(g.length("length")?),
                    ByKind::Angle => ExtendBy::Angle(g.angle("angle")?),
                    ByKind::Point => {
                        let [x, y] = position(g, "point")?;
                        ExtendBy::Point([Coordinate::new(x)?, Coordinate::new(y)?])
                    }
                    ByKind::NextCurve => ExtendBy::NextCurve,
                })
            });
            Some(SketchExtendRequest {
                session: session?,
                revision: revision?,
                sketch: sketch?,
                curve: curve?,
                end: end?,
                by: by?,
            })
        })
    }
}

impl ToWire for SketchExtendRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        let end = match self.end {
            End::Start => "start",
            End::End => "end",
        };
        let by = match self.by {
            ExtendBy::Length(length) => Json::object([
                ("type", Json::str("length")),
                ("length", Json::length(length)),
            ]),
            ExtendBy::Angle(angle) => {
                Json::object([("type", Json::str("angle")), ("angle", Json::angle(angle))])
            }
            ExtendBy::Point([x, y]) => Json::object([
                ("type", Json::str("point")),
                ("point", position_wire([x.mm(), y.mm()])),
            ]),
            ExtendBy::NextCurve => Json::object([("type", Json::str("next_curve"))]),
        };
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("sketch", self.sketch.to_wire()),
            ("curve", Json::str(self.curve.as_str())),
            ("end", Json::str(end)),
            ("by", by),
        ])
    }
}

impl ToWire for SketchCut {
    /// A split's or a trim's result: the sketch, the curve's pieces, the members added,
    /// modified, and removed, the features storing a name the curve reaches, whether the
    /// document changed, and the commit outside a transaction.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("sketch", self.sketch.to_wire()),
            ("pieces", members(&self.pieces)),
            ("added", members(&self.added)),
            ("modified", members(&self.modified)),
            ("removed", members(&self.removed)),
            (
                "references",
                Json::array(self.references.iter().map(ToWire::to_wire)),
            ),
            ("changed", Json::bool(self.changed)),
            (
                "commit",
                Json::or_null(self.commit.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

/// A sketch member's ID at `key` (`what` it names, "a curve", for the refusal): the
/// sketch's own members only, never a fixed reference, which no edit here takes.
fn member(f: &mut RequestFields<'_>, key: &str, what: &str) -> Option<ElementId> {
    let node = f.take(key)?;
    match &node.value {
        Value::String(text) if let Some(id) = ElementId::new(text) => Some(id),
        _ => {
            f.wrong(
                key,
                format_args!("{what}'s ID, as the sketch holds it"),
                node,
            );
            None
        }
    }
}

/// The list at `key` of the sketch's members' IDs (`what` it must be, refused), each item
/// at fault refused at its place.
fn members_list(f: &mut RequestFields<'_>, key: &str, what: &str) -> Option<Vec<ElementId>> {
    let node = f.take(key)?;
    let Value::Array(items) = &node.value else {
        f.wrong(key, what, node);
        return None;
    };
    member_items(f, key, items)
}

/// `items`, the list at `key`, each a sketch member's ID, each at fault refused at its
/// place.
fn member_items(f: &mut RequestFields<'_>, key: &str, items: &[Node]) -> Option<Vec<ElementId>> {
    let mut ids = Vec::with_capacity(items.len());
    let mut fine = true;
    for (i, item) in items.iter().enumerate() {
        match &item.value {
            Value::String(text) if let Some(id) = ElementId::new(text) => ids.push(id),
            _ => {
                fine = false;
                f.refused.refuse_with(|| {
                    let at = format!("{}.{i}", join(&f.path, key));
                    invalid(
                        &at,
                        format!("{at} must be a sketch member's ID, got {}", written(item)),
                    )
                    .with_suggestion("give an 8-character ID, as the sketch's members have")
                });
            }
        }
    }
    fine.then_some(ids)
}

/// The new members' IDs at `key`: a list of members' IDs, or `null` for the core's
/// (docs/design/m5.0-app.md, section 7, *New IDs*).
fn new_ids(f: &mut RequestFields<'_>, key: &str) -> Option<Option<Vec<ElementId>>> {
    let node = match f.take_nullable(key)? {
        None => return Some(None),
        Some(node) => node,
    };
    let Value::Array(items) = &node.value else {
        f.wrong(key, "a list of the new members' IDs, or null", node);
        return None;
    };
    member_items(f, key, items).map(Some)
}

/// A position in the sketch's coordinates at `key`: two finite numbers, `[x, y]`, each
/// read correctly rounded to the nearest `f64`, as a sketch's coordinates are (M3.2's D3).
fn position(f: &mut RequestFields<'_>, key: &str) -> Option<[f64; 2]> {
    let node = f.take(key)?;
    position_of(f, key, node)
}

/// `node`, at `at` in the request, as a position.
fn position_of(f: &mut RequestFields<'_>, at: &str, node: &Node) -> Option<[f64; 2]> {
    let what = "a position, two numbers [x, y] in millimetres";
    let Value::Array(items) = &node.value else {
        f.wrong(at, what, node);
        return None;
    };
    let [x, y] = items.as_slice() else {
        f.refused.refuse_with(|| {
            let at = join(&f.path, at);
            invalid(
                &at,
                format!("{at} must be {what}, got {} of them", items.len()),
            )
        });
        return None;
    };
    let path = &f.path;
    let refused = &mut f.refused;
    let mut component = |i: usize, item: &Node| {
        let value = match &item.value {
            Value::Number(text) => Coordinate::parse(text).map(Coordinate::mm),
            _ => None,
        };
        if value.is_none() {
            refused.refuse_with(|| {
                let at = format!("{}.{i}", join(path, at));
                invalid(
                    &at,
                    format!("{at} must be a finite number, got {}", written(item)),
                )
            });
        }
        value
    };
    // Each read, so each member at fault is said.
    let (x, y) = (component(0, x), component(1, y));
    Some([x?, y?])
}

/// The list of positions at `key`, each as [`position`] reads one, at its place
/// (`at.0`).
fn positions(f: &mut RequestFields<'_>, key: &str) -> Option<Vec<[f64; 2]>> {
    let node = f.take(key)?;
    let Value::Array(items) = &node.value else {
        f.wrong(key, "a list of positions, each two numbers [x, y]", node);
        return None;
    };
    let mut positions = Vec::with_capacity(items.len());
    let mut fine = true;
    for (i, item) in items.iter().enumerate() {
        match position_of(f, &format!("{key}.{i}"), item) {
            Some(position) => positions.push(position),
            None => fine = false,
        }
    }
    fine.then_some(positions)
}

/// A position as a client writes one: each coordinate as a sketch's is written.
fn position_wire([x, y]: [f64; 2]) -> Json<'static> {
    Json::array([Json::coordinate(x), Json::coordinate(y)])
}

/// The new members' IDs as given, or `null`.
fn ids_wire(ids: Option<&[ElementId]>) -> Json<'_> {
    Json::or_null(ids, members)
}

/// `sketch_split`'s request: split one curve of a sketch at a position on it.
#[derive(JsonSchema)]
#[schemars(rename = "SketchSplitRequest", transform = wire_shape)]
pub struct SketchSplitRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The sketch, by its feature's ID.
    pub sketch: FeatureId,
    /// The line, arc, or circle to split, by ID. The piece on its start's side keeps the ID
    /// (a circle's, the arc counter-clockwise from `at`'s first position to its second), so
    /// a name a feature stores of it then finds that piece: the result's `references` lists
    /// each such feature.
    pub curve: ElementId,
    /// Where to split it, in the sketch's coordinates: one position for a line or an arc,
    /// two for a circle. Each within 1e-7 mm of the curve, where the new point goes at its
    /// projection, and farther than 1e-7 mm from an end; a point the sketch holds on the
    /// curve there is shared instead.
    pub at: Vec<[Coordinate; 2]>,
    /// The new members' IDs, in the order the result's `added` lists them (its new points
    /// along the curve from its start, its new piece, then a line's join), each a fresh
    /// random 8-character ID; or null for the core to make them. A list of the wrong length
    /// is refused, saying how many the split needs. A replay passes back the `added` it
    /// recorded.
    pub ids: Option<Vec<ElementId>>,
}

/// `sketch_trim`'s request: trim one curve of a sketch back to its neighbours where it was
/// picked.
#[derive(JsonSchema)]
#[schemars(rename = "SketchTrimRequest", transform = wire_shape)]
pub struct SketchTrimRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The sketch, by its feature's ID.
    pub sketch: FeatureId,
    /// The line, arc, or circle to trim, by ID. Every other curve cuts it, construction
    /// too, where they cross, touch, or one's end lies on it; the first piece left keeps
    /// the ID.
    pub curve: ElementId,
    /// Where the part to trim away was picked, in the sketch's coordinates: the piece
    /// between the cuts either side of its projection onto the curve is removed, or the
    /// curve whole where nothing cuts it on a side.
    pub pick: [Coordinate; 2],
    /// The new members' IDs, in the order the result's `added` lists them (its new points
    /// along the curve from its start, its new piece, the constraints copied to it, then
    /// its new ends' ons), each a fresh random 8-character ID; or null for the core to make
    /// them. A list of the wrong length is refused, saying how many the trim needs.
    pub ids: Option<Vec<ElementId>>,
}

/// `sketch_unjoin`'s request: pull one curve's use of a shared point apart.
#[derive(JsonSchema)]
#[schemars(rename = "SketchUnjoinRequest", transform = wire_shape)]
pub struct SketchUnjoinRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The sketch, by its feature's ID.
    pub sketch: FeatureId,
    /// The point two or more curves share, by ID. The curves that stay keep it, and every
    /// constraint on it.
    pub point: ElementId,
    /// The curve to detach, by ID: it gets a new point at the same coordinates, with none
    /// of the point's constraints but every one of its own.
    pub curve: ElementId,
    /// The new point's ID, a list of one fresh random 8-character ID; or null for the core
    /// to make it.
    pub ids: Option<Vec<ElementId>>,
}

/// `sketch_merge`'s request: make several points of a sketch one.
#[derive(JsonSchema)]
#[schemars(rename = "SketchMergeRequest", transform = wire_shape)]
pub struct SketchMergeRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The sketch, by its feature's ID.
    pub sketch: FeatureId,
    /// The points to make one, two or more, by ID: the first survives at its own
    /// coordinates, the sketch's solve starting from there, and the others' curves and
    /// constraints are re-pointed to it.
    pub points: Vec<ElementId>,
}

/// `sketch_split`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "SketchSplitResult", transform = wire_shape)]
pub struct SketchSplitResultSchema {
    /// The sketch, as the request named it.
    pub sketch: FeatureId,
    /// The two pieces, the start-side one first, which keeps the curve's ID.
    pub pieces: Vec<ElementId>,
    /// The members added, in the order the split made them: by kind (points, lines,
    /// circles, arcs, constraints), the new points along the curve from its start.
    pub added: Vec<ElementId>,
    /// The members changed in place (the curve, now its start-side piece; a constraint
    /// moved to the other piece or retyped under its ID, a `length` a `distance`), by kind,
    /// then by ID.
    pub modified: Vec<ElementId>,
    /// The members removed (an `on` the split point makes implied, a line's `equal`), by
    /// kind, then by ID.
    pub removed: Vec<ElementId>,
    /// The features that store a name made from the curve's ID or its points' IDs, in
    /// timeline order: each such name now finds only what the start-side piece makes.
    pub references: Vec<FeatureId>,
    /// Whether the document changed: always, for a split it makes.
    pub changed: bool,
    /// Outside a transaction, the commit that saved the edit; null inside one.
    pub commit: Option<CommitResultSchema>,
}

/// `sketch_trim`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "SketchTrimResult", transform = wire_shape)]
pub struct SketchTrimResultSchema {
    /// The sketch, as the request named it.
    pub sketch: FeatureId,
    /// The pieces left, the first keeping the curve's ID: none when the trim removes the
    /// curve whole, one when it shortens it or opens a circle, two when it takes out its
    /// middle.
    pub pieces: Vec<ElementId>,
    /// The members added, in the order the trim made them: by kind (points, lines,
    /// circles, arcs, constraints), the new points along the curve from its start.
    pub added: Vec<ElementId>,
    /// The members changed in place (the curve, now its first piece; a constraint moved to
    /// a piece or retyped under its ID), by kind, then by ID.
    pub modified: Vec<ElementId>,
    /// The members removed (the part trimmed away, its points no other curve uses, and the
    /// constraints that go with it), by kind, then by ID.
    pub removed: Vec<ElementId>,
    /// The features that store a name made from the curve's ID or its points' IDs, in
    /// timeline order: each such name now finds only what the first piece makes, or
    /// nothing, where the curve is removed whole.
    pub references: Vec<FeatureId>,
    /// Whether the document changed: always, for a trim it makes.
    pub changed: bool,
    /// Outside a transaction, the commit that saved the edit; null inside one.
    pub commit: Option<CommitResultSchema>,
}

/// `sketch_unjoin`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "SketchUnjoinResult", transform = wire_shape)]
pub struct SketchUnjoinResultSchema {
    /// The sketch, as the request named it.
    pub sketch: FeatureId,
    /// The new point, the detached curve's.
    pub added: Vec<ElementId>,
    /// The members changed in place (the detached curve; a `tangent_on` of it at the point,
    /// moved to the new point), by kind, then by ID.
    pub modified: Vec<ElementId>,
    /// The members removed (a `tangent_at` of the detached curve at the point), by kind,
    /// then by ID.
    pub removed: Vec<ElementId>,
    /// Whether the document changed: always, for an unjoin it makes.
    pub changed: bool,
    /// Outside a transaction, the commit that saved the edit; null inside one.
    pub commit: Option<CommitResultSchema>,
}

/// `sketch_merge`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "SketchMergeResult", transform = wire_shape)]
pub struct SketchMergeResultSchema {
    /// The sketch, as the request named it.
    pub sketch: FeatureId,
    /// Always empty: a merge makes no member.
    pub added: Vec<ElementId>,
    /// The members changed in place (the curves and constraints re-pointed to the kept
    /// point; an edge `tangent` now a `tangent_at` at the end the curves share, under its
    /// ID), by kind, then by ID.
    pub modified: Vec<ElementId>,
    /// The members removed (the points merged into the first, an `on` the merge makes hold
    /// an end on its own curve), by kind, then by ID.
    pub removed: Vec<ElementId>,
    /// Whether the document changed: always, for a merge it makes.
    pub changed: bool,
    /// Outside a transaction, the commit that saved the edit; null inside one.
    pub commit: Option<CommitResultSchema>,
}

/// Which end of the curve a `sketch_extend` moves.
#[derive(JsonSchema)]
#[schemars(rename = "ExtendEnd", rename_all = "lowercase")]
pub enum ExtendEndSchema {
    Start,
    End,
}

/// How far a `sketch_extend` takes the end, or to where, by its `type`.
#[derive(JsonSchema)]
#[schemars(
    rename = "ExtendBy",
    tag = "type",
    rename_all = "snake_case",
    transform = wire_shape
)]
pub enum ExtendBySchema {
    /// A line's end along it, in millimetres: a positive length lengthens the line, a
    /// negative one shortens it.
    Length { length: Length },
    /// An arc's end around its circle, in degrees: its start turned back, its end on, so a
    /// positive angle lengthens the arc, a negative one shortens it.
    Angle { angle: Angle },
    /// To a point on the curve's line or circle, on the end's side of its other end
    /// (within 1e-7 mm of the carrier), in the sketch's coordinates: past the end
    /// lengthens, short of it shortens, and the end is put there exactly.
    Point { point: [Coordinate; 2] },
    /// To the nearest curve the line or arc, extended, meets past its end (SolidWorks'
    /// Extend Entities): the core finds it as trim finds its cuts, every other curve,
    /// construction too, and puts the end there exactly.
    NextCurve,
}

/// `sketch_extend`'s request: extend a line's or an arc's end, its other end staying.
#[derive(JsonSchema)]
#[schemars(rename = "SketchExtendRequest", transform = wire_shape)]
pub struct SketchExtendRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the edit was made against: an edit against another is
    /// refused as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The sketch, by its feature's ID.
    pub sketch: FeatureId,
    /// The line or arc, by ID. A line's end moves through the solver's drag, so its
    /// constraints win (a line with a length slides rather than grows) and every curve
    /// sharing the end follows; an arc's end is turned. Nothing is added to hold the end
    /// where it lands.
    pub curve: ElementId,
    /// Which end moves: its start or its end, as the curve runs.
    pub end: ExtendEndSchema,
    /// How far, or to where.
    pub by: ExtendBySchema,
}

/// `sketch_extend`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "SketchExtendResult", transform = wire_shape)]
pub struct SketchExtendResultSchema {
    /// The sketch, as the request named it.
    pub sketch: FeatureId,
    /// Always empty: an extend makes no member.
    pub added: Vec<ElementId>,
    /// The members changed in place: the points moved, then a circle whose radius the move
    /// changed, then a reference dimension it re-measured, each by ID; and with them what
    /// the write-back of the starting solve changed (its points, radii, and reference
    /// values), since the edit starts from the sketch as solved.
    pub modified: Vec<ElementId>,
    /// Always empty: an extend removes no member.
    pub removed: Vec<ElementId>,
    /// Whether the document changed: always, for an extend it makes.
    pub changed: bool,
    /// Outside a transaction, the commit that saved the edit; null inside one.
    pub commit: Option<CommitResultSchema>,
}
