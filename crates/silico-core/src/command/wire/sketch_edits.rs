//! `sketch_add` and `sketch_remove`'s wire types (docs/design/m5.0-app.md, section 7, *The
//! sketch edits*): their requests, read strictly ([`SketchAddRequest`],
//! [`SketchRemoveRequest`]) and written back as a client writes them; their result, written
//! field for field ([`ToWire`] for [`SketchEdited`]); and the mirrors their snapshots,
//! `commands/sketch_add.json` and `sketch_remove.json`, are generated from. A request's shape
//! is read here; the members `sketch_add` gives are read by the document's own reader, in
//! the session, as `add_feature`'s feature is, so the wire and a Rust client meet one set of
//! refusals.

use schemars::JsonSchema;

use super::transaction::CommitResultSchema;
use super::{Json, RequestFields, ToWire, invalid, join, wire_shape, written};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{NewMembers, Problems, Revision, SessionId, SketchEdited};
use crate::document::json::{Node, Value};
use crate::document::{
    ElementId, ElementKind, FeatureId, Reference, SketchArc, SketchCircle, SketchConstraint,
    SketchLine, SketchPoint,
};

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

/// `sketch_add`'s request: add members to a sketch, as the document writes them.
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
    /// The points to add, each with an ID the sketch doesn't hold.
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
    /// same: the sketch then reads as over defined, and its features fail until it's mended.
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
