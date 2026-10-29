//! Adding members to a sketch (docs/ports/sketch-edit.md, §2, *Add a point, a curve, a
//! constraint*; docs/design/m5.0-app.md, `sketch_add`): a batch of points, curves, and
//! constraints, as the document writes them, each with its own ID, checked whole by the
//! validation gate before any of it is added.

use std::collections::BTreeMap;

use super::{Edited, Refused, kinds_of};
use crate::document::sketch::{
    ElementId, ElementKind, Sketch, SketchArc, SketchCircle, SketchConstraint, SketchLine,
    SketchPoint,
};
use crate::document::{an_id_of_its_own, id_held, sketch_problems};
use crate::{Error, ErrorCode};

/// The members a `sketch_add` request gives, as the document writes them, each with the ID
/// it's to have (the request has no `ids`: docs/design/m5.0-app.md, `sketch_add`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Members {
    pub points: Vec<SketchPoint>,
    pub lines: Vec<SketchLine>,
    pub circles: Vec<SketchCircle>,
    pub arcs: Vec<SketchArc>,
    pub constraints: Vec<SketchConstraint>,
}

impl Members {
    /// The IDs of the members of `kind`, in the order given.
    fn ids(&self, kind: ElementKind) -> Vec<&ElementId> {
        match kind {
            ElementKind::Point => self.points.iter().map(|p| &p.id).collect(),
            ElementKind::Line => self.lines.iter().map(|l| &l.id).collect(),
            ElementKind::Circle => self.circles.iter().map(|c| &c.id).collect(),
            ElementKind::Arc => self.arcs.iter().map(|a| &a.id).collect(),
            ElementKind::Constraint => self.constraints.iter().map(|c| &c.id).collect(),
        }
    }

    /// Every member's ID with its kind: the lists in the format's order
    /// ([`ElementKind::ALL`]), each in the order given.
    fn all(&self) -> impl Iterator<Item = (ElementKind, &ElementId)> {
        ElementKind::ALL
            .into_iter()
            .flat_map(move |kind| self.ids(kind).into_iter().map(move |id| (kind, id)))
    }
}

/// `sketch` with `members` added, as given: the batch is checked whole before any of it is
/// added, as FreeCAD's Python `addConstraint` checks a list (`7d8b1f30c3`) and its tools add
/// a shape in one transaction, but by every rule of the gate, not only FreeCAD's index
/// ranges. Each `side` and `direction` is the member's own, never decided from the drawing
/// (D4, where FreeCAD records them: `setOrientation`, `AutoLockTangencyAndPerpty`), and each
/// `construction` too, points' included (D7, `3db4633f9d`). A new curve may use a point the
/// sketch holds, which joins it to that point's curves (shared points, M3.2's D2), and a
/// constraint may name a member of the same batch.
///
/// `added` lists the members by kind (points, lines, circles, arcs, constraints), each kind
/// in the order given; `modified` and `removed` are empty. An empty batch adds nothing and
/// leaves the sketch as it was, so its command changes nothing (FreeCAD's empty lists add
/// nothing too).
///
/// Refused whole, nothing added, with every problem found (the first [`PROBLEMS_LISTED`]
/// listed, all counted), in two stages:
/// - **IDs**, each at the request's field (docs/design/m4.1-command-api.md, section 2, *An
///   edit that repeats an ID*): one the sketch already holds, at the member's ID path
///   (`circles.cb000003.id`), naming the member holding it; one given twice in the batch, at
///   the later member's place (`circles.1.id`), as the gate names a later holder. These
///   come first because the gate would name the sketch's untouched member instead.
/// - **The gate's errors** for the sketch with every member added, all at once, each at its
///   field's ID path (`circles.cb000003.radius`), which is the request's path for it too: a
///   reference to no member (with candidates), a curve of 1e-7 mm or less, a curve's own
///   point constrained onto it, an edge `tangent` between curves sharing an end, a value out
///   of range, and the sketch past what the solver takes.
///
/// The add doesn't solve: the edit's command solves the sketch once, after the gate, and a
/// conflict that solve finds refuses the edit as data (M3.3), where FreeCAD keeps a
/// conflicting constraint and reports it (docs/ports/sketch-edit.md, §2).
///
/// [`PROBLEMS_LISTED`]: crate::document::PROBLEMS_LISTED
pub fn add(sketch: &Sketch, members: &Members) -> Result<Edited, Refused> {
    let _span = tracing::info_span!(
        "sketch_add",
        points = members.points.len(),
        lines = members.lines.len(),
        circles = members.circles.len(),
        arcs = members.arcs.len(),
        constraints = members.constraints.len(),
    )
    .entered();
    let held = kinds_of(sketch);
    let mut refused = Refused::default();
    // Each ID the request gives, with its first holder's kind and path: a later holder is
    // named by its place in its list, as the gate names one.
    let mut given: BTreeMap<&ElementId, (ElementKind, String)> = BTreeMap::new();
    for kind in ElementKind::ALL {
        let list = kind.list();
        for (place, id) in members.ids(kind).into_iter().enumerate() {
            let earlier = given.get(id);
            let path = match earlier {
                Some(_) => format!("{list}.{place}"),
                None => format!("{list}.{id}"),
            };
            let message = match (held.get(id), earlier) {
                (Some(holder), _) => id_held(id, *holder, &format!("{}.{id}", holder.list())),
                (None, Some((first, first_path))) => format!(
                    "id {id} is already the ID of {} {id} ({first_path}), earlier in this request",
                    first.noun()
                ),
                (None, None) => {
                    given.insert(id, (kind, path));
                    continue;
                }
            };
            refused.push(|| {
                Error::new(ErrorCode::InvalidInput, message)
                    .with_element(id.as_str())
                    .with_parameter(format!("{path}.id"))
                    .with_suggestion(an_id_of_its_own(kind))
            });
            given.entry(id).or_insert((kind, path));
        }
    }
    if refused.total > 0 {
        return Err(refused);
    }
    let mut edited = sketch.clone();
    edited.points.extend(members.points.iter().cloned());
    edited.lines.extend(members.lines.iter().cloned());
    edited.circles.extend(members.circles.iter().cloned());
    edited.arcs.extend(members.arcs.iter().cloned());
    edited
        .constraints
        .extend(members.constraints.iter().cloned());
    for problem in sketch_problems(&edited) {
        refused.push(|| problem);
    }
    if refused.total > 0 {
        return Err(refused);
    }
    Ok(Edited {
        sketch: edited,
        added: members.all().map(|(_, id)| id.clone()).collect(),
        modified: Vec::new(),
        removed: Vec::new(),
    })
}
