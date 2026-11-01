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
use crate::document::sketch_gate::size;
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
///   of range, and the sketch past what the solver takes. A limit the batch passes is
///   refused at the last of the request's lists (points, lines, circles, arcs, constraints)
///   whose members add to what's counted, where the gate names a list of its own for each
///   limit (the values' at `points`, though a batch of driving dimensions passed it).
///
/// The add doesn't solve, and neither does its command (`sketch_add`): once the gate takes
/// it, it's applied, and a conflict or a redundancy the rebuild's solve then finds is the
/// sketch's state, never a refusal (docs/design/m5.0-app.md, SD7), as FreeCAD keeps a
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
    let mut problems = sketch_problems(&edited);
    if !problems.is_empty() {
        limits_at_the_requests_lists(sketch, members, &edited, &mut problems);
    }
    for problem in problems {
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

/// Each of the solver's limits ([`size`]) that `edited`, `sketch` with `members` added,
/// passes, its refusal among `problems` (found by its list and its words, "the sketch has
/// {n} {what}, ...") moved to the last of the request's lists whose members add to the
/// count. The gate names one list for each limit, whichever list a file's sketch passed it
/// with; an edit's request names what it added. A limit no list of the batch adds to (one
/// the sketch passed already) keeps the gate's list.
fn limits_at_the_requests_lists(
    sketch: &Sketch,
    members: &Members,
    edited: &Sketch,
    problems: &mut [Error],
) {
    // A sketch of only the batch's members of `kind`, and of none: what each kind adds to
    // each count is the difference.
    let only = |kind: Option<ElementKind>| {
        let of = |k: ElementKind| kind == Some(k);
        Sketch {
            plane: sketch.plane.clone(),
            points: if of(ElementKind::Point) {
                members.points.clone()
            } else {
                Vec::new()
            },
            lines: if of(ElementKind::Line) {
                members.lines.clone()
            } else {
                Vec::new()
            },
            circles: if of(ElementKind::Circle) {
                members.circles.clone()
            } else {
                Vec::new()
            },
            arcs: if of(ElementKind::Arc) {
                members.arcs.clone()
            } else {
                Vec::new()
            },
            constraints: if of(ElementKind::Constraint) {
                members.constraints.clone()
            } else {
                Vec::new()
            },
        }
    };
    let none = size(&only(None));
    let added: Vec<(ElementKind, _)> = ElementKind::ALL
        .into_iter()
        .map(|kind| (kind, size(&only(Some(kind)))))
        .collect();
    for (row, &(list, n, most, what)) in size(edited).iter().enumerate() {
        if n <= most {
            continue;
        }
        let Some((kind, _)) = added
            .iter()
            .rev()
            .find(|(_, counts)| counts[row].1 > none[row].1)
        else {
            continue;
        };
        let words = format!("the sketch has {n} {what}, ");
        for problem in problems
            .iter_mut()
            .filter(|p| p.parameter() == Some(list) && p.message().starts_with(&words))
        {
            *problem = problem.clone().with_parameter(kind.list());
        }
    }
}
