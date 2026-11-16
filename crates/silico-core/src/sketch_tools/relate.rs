//! Sketch mode's ten constraint tools, headless: Horizontal, Vertical, Coincident, Tangent,
//! Equal, Parallel, Perpendicular, Midpoint, Symmetric, and Fix (Fable's decisions C1 to C9
//! of 2026-11-14, as amended on #478's port review, open item 34 of docs/design/m5.0-app.md,
//! on the study docs/study/constraint-tools.md). Each is a click on a selection made first
//! (C7), which [`relate`] turns into one edit, from the newest `sketch` read's members and the
//! selection in the order it was picked (C9: the sketch's points, lines, circles, and arcs,
//! the origin, and the axes). No viewport, no session: the app sends the edit against the
//! read's revision, and shows the refusal or the message.
//!
//! - **What it checks** is only what it needs to choose a form and its roles: what each
//!   member picked is (a point, the origin among them; a line or an axis; a circle or an
//!   arc), and how many. A selection it can't read is refused with the tool's one sentence
//!   ([`RelationTool::takes`]), the selection as the error's elements. Everything else wrong
//!   with a selection (a curve's own point, only the origin and the axes, a line made equal
//!   to a circle, a mirror among its own points) is the gate's to refuse, in its words, when
//!   the command runs. A member picked twice counts once, where it was first picked.
//! - **The edit** is a `sketch_add` of the constraints written, applied whatever the solve
//!   finds (SD7), but where the document's structure is the relation:
//!   - Coincident of two or more points is a `sketch_merge` of them in pick order, the first
//!     picked surviving (C1), refused where its solve is worse, as every merge is;
//!   - a `tangent_on` or a `midpoint` that implies an `on` the sketch holds replaces it: the
//!     add, then a `sketch_remove` of the `on` (C5), one undo step for the app; where the
//!     relation is there already, the remove alone (C5 and C6 as amended, #478).
//! - **Roles.** `a` is the first picked and `b` the second wherever the structure doesn't
//!   fix them. Several members are a star, each after the first paired with the first (C8),
//!   the origin as any other.
//! - **Already there** (C6): a constraint the click would write is left out where the sketch
//!   holds one equal to it in everything but its ID, `a` and `b` either way round in the
//!   forms where they're interchangeable. With every one left out (and no `on` to remove),
//!   nothing is sent ([`Related::AllThere`]), and the message says so; with some, it counts
//!   them.
//! - **Read from the drawing,** the newest read's coordinates: a tangent's `side` (the side
//!   of the line the round's centre is on, at Smart Dimension's nanometre), a `tangent_at`'s
//!   or `tangent_on`'s `direction` (C3, by `sketch_edit::drawn` and `sketch_edit::leaving`,
//!   trim's own), and Fix's values, written by Smart Dimension's own writer and rounded to
//!   the nanometre (C2).
//! - **Bounded:** a click is refused, in the gate's words, before it writes more constraints
//!   than the sketch solver takes, and each constraint it would write is looked up among the
//!   sketch's once, so its work is in proportion to what it writes and what the sketch holds.

use std::collections::{BTreeMap, BTreeSet};

use silico_protocol::MAX_SKETCH_CONSTRAINTS;

use super::build::Builder;
use super::dimension::{DistanceAxis, NotOff, distance_along, off_line};
use super::geometry::dot;
use super::infer::{View, at_origin};
use super::{Message, NAMED, NewIds, Xy};
use crate::command::Revision;
use crate::document::sketch_edit::{self, Geo, Members, end_ids};
use crate::document::{
    CONSTRAINTS_COUNTED, Direction, ElementId, ElementKind, Form, LineSide, PROBLEMS_LISTED,
    Reference, SPLIT_THE_SKETCH, Sketch, SketchConstraint, TWO_LINES_TANGENT, TangentSide,
    counted_constraints, past_the_solver,
};
use crate::{Error, ErrorCode, either, listed, listed_first};

/// What every tool says with nothing selected: the approved prototype's words.
pub const NOTHING_SELECTED: &str =
    "Select the curves or points first; the relation applies to them.";

/// Which constraint tool: the relation its button makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelationTool {
    Horizontal,
    Vertical,
    Coincident,
    Tangent,
    Equal,
    Parallel,
    Perpendicular,
    Midpoint,
    Symmetric,
    Fix,
}

impl RelationTool {
    /// The ten, in the order the prototype's toolbar gives them.
    pub const ALL: [RelationTool; 10] = [
        RelationTool::Horizontal,
        RelationTool::Vertical,
        RelationTool::Coincident,
        RelationTool::Tangent,
        RelationTool::Equal,
        RelationTool::Parallel,
        RelationTool::Perpendicular,
        RelationTool::Midpoint,
        RelationTool::Symmetric,
        RelationTool::Fix,
    ];

    /// As its button names it, and the undo step its click makes.
    pub fn name(self) -> &'static str {
        match self {
            RelationTool::Horizontal => "Horizontal",
            RelationTool::Vertical => "Vertical",
            RelationTool::Coincident => "Coincident",
            RelationTool::Tangent => "Tangent",
            RelationTool::Equal => "Equal",
            RelationTool::Parallel => "Parallel",
            RelationTool::Perpendicular => "Perpendicular",
            RelationTool::Midpoint => "Midpoint",
            RelationTool::Symmetric => "Symmetric",
            RelationTool::Fix => "Fix",
        }
    }

    /// What it takes: the one sentence it refuses a selection it can't read with.
    pub fn takes(self) -> &'static str {
        match self {
            RelationTool::Horizontal => "Horizontal takes lines, or two or more points",
            RelationTool::Vertical => "Vertical takes lines, or two or more points",
            RelationTool::Coincident => {
                "Coincident takes points and the curves they lie on, or two or more points to make one"
            }
            RelationTool::Tangent => {
                "Tangent takes two curves, at least one a circle or an arc, and at most one point where they meet"
            }
            RelationTool::Equal => "Equal takes two or more lines, or two or more circles and arcs",
            RelationTool::Parallel => "Parallel takes two or more lines",
            RelationTool::Perpendicular => "Perpendicular takes two lines",
            RelationTool::Midpoint => "Midpoint takes a point and a line",
            RelationTool::Symmetric => {
                "Symmetric takes two points and the line or point they mirror about, the point picked last"
            }
            RelationTool::Fix => "Fix takes points",
        }
    }
}

/// The edit a click makes, for the app to send against the read's revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelateEdit {
    /// A `sketch_add` of these constraints.
    Add(Members),
    /// A `sketch_merge` of these points, in pick order, the first surviving (C1).
    Merge(Vec<ElementId>),
    /// A `sketch_add` of `add`, then a `sketch_remove` of the `on`s it implies (C5): one undo
    /// step. The app sends the remove only once the add is taken, against the revision the
    /// add made; if the add is refused, nothing else is sent, and if the remove is refused,
    /// the add stands alone as the step (the `on` then reads redundant, and the same click
    /// again removes it).
    AddThenRemove {
        add: Members,
        remove: Vec<ElementId>,
    },
    /// A `sketch_remove` alone of the `on`s a relation already there implies (C5 and C6 as
    /// amended, #478): the pair an interrupted click, or an agent, left.
    Remove(Vec<ElementId>),
}

/// What a click on a constraint tool makes ([`relate`]), planned on the read of `revision`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Related {
    /// An edit to send against `revision`: `repeated` the sketch's constraints the click
    /// would have written again, left out (C6), and what the app's toast says, if anything.
    Edit {
        revision: Revision,
        edit: RelateEdit,
        repeated: Vec<ElementId>,
        message: Option<Message>,
    },
    /// Nothing to send: every constraint the click would write is there already (C6), as
    /// `repeated`, and nothing it implies is left to remove. The toast says so.
    AllThere {
        revision: Revision,
        repeated: Vec<ElementId>,
        message: Message,
    },
}

impl Related {
    /// The revision of the read the click was planned on: the edit's to be sent against it,
    /// so a session that has moved on refuses it as a conflict (decision 12).
    pub fn revision(&self) -> Revision {
        match self {
            Related::Edit { revision, .. } | Related::AllThere { revision, .. } => *revision,
        }
    }

    /// The edit to send, if any.
    pub fn edit(&self) -> Option<&RelateEdit> {
        match self {
            Related::Edit { edit, .. } => Some(edit),
            Related::AllThere { .. } => None,
        }
    }

    /// The sketch's constraints the click would have written again, left out (C6), in the
    /// order it would have written them.
    pub fn repeated(&self) -> &[ElementId] {
        match self {
            Related::Edit { repeated, .. } | Related::AllThere { repeated, .. } => repeated,
        }
    }

    /// What the app's toast says: none where nothing was left out.
    pub fn message(&self) -> Option<&Message> {
        match self {
            Related::Edit { message, .. } => message.as_ref(),
            Related::AllThere { message, .. } => Some(message),
        }
    }
}

/// What a member picked is, as the tools read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A point of the sketch, or the origin.
    Point,
    /// A line of the sketch, or an axis.
    Line,
    /// A circle or an arc of the sketch.
    Round,
    /// A constraint, or no member the read holds.
    Other,
}

/// What a tool writes, before C6 and the IDs: constraints, with the `on`s the one it writes
/// implies (C5) and what it says if they're all there already; or a merge.
enum Plan {
    Constraints {
        forms: Vec<Form>,
        replaces: Vec<ElementId>,
        /// "line ne000002 is already vertical", for C6's message.
        said_if_there: String,
    },
    Merge(Vec<ElementId>),
}

/// A place two curves can be tangent at (C3): an end they share, or an end of `ending` that
/// a constraint holds on `held_on` (an `on`, a `tangent_on`, or a `midpoint`, [`Form::hold`]),
/// with the `on`s among those holds, which a `tangent_on` there replaces (C5): none where only
/// a `tangent_on` or a `midpoint` holds it.
enum Place<'p> {
    Shared,
    Held {
        ending: &'p Reference,
        held_on: &'p Reference,
        ons: Vec<ElementId>,
    },
}

/// The edit `relation`'s click makes of `picks`, the selection in the order it was picked,
/// in the sketch `read`, the members of the newest `sketch` read, of the model at `revision`,
/// as solved; its new constraints' IDs from `ids`.
///
/// **The read is the newest the app has, and the plan is only good for it:** `relate` knows
/// no edit the app has sent since. The app sends the edit against [`Related::revision`], so
/// if anything changed the session since that read (the click before, its read not in yet),
/// decision 12's check refuses it as a `conflict`, and nothing is written twice or half (the
/// app's double-click test comes with the buttons). A `conflict` answered to a relate edit
/// means only that its plan was stale: the app plans the click again on the next read (or
/// says it's still solving, as Smart Dimension does) rather than showing the conflict. A
/// [`RelateEdit::AddThenRemove`] is sent as its doc says.
///
/// Refused with the tool's sentence ([`RelationTool::takes`]) for a selection it can't read,
/// the selection as the error's elements; with [`NOTHING_SELECTED`] for none; for Tangent,
/// two lines (in the gate's words), a round's centre on the line at the nanometre, a line of
/// no length, a point picked with the curves that isn't one of their places, and more than
/// one place with no point picked to say which (C3); and, in the gate's words, a click that
/// would give the sketch more constraints than the sketch solver takes. Each tool, as C1 to
/// C8 decide:
///
/// - **Horizontal, Vertical:** one `horizontal` (`vertical`) for each line, in pick order;
///   or, for two or more points, a `horizontal_points` (`vertical_points`) of each with the
///   first picked.
/// - **Coincident:** one or more points and one or more curves, an `on` for each point and
///   each curve, points in pick order, then curves (the origin a point, an axis a curve);
///   two or more points, a merge, the first surviving; a point and the origin, #278's A pair,
///   whichever was picked first. The origin with two or more points is refused.
/// - **Tangent:** two curves, and a point if it says which place. The places they can be
///   tangent at are the ends they share and the ends of one that a constraint holds on the
///   other, an `on`, a `tangent_on`, or a `midpoint` ([`Form::hold`]): none, an edge
///   `tangent`, its `side` the side of the line the round's centre is drawn on, at Smart
///   Dimension's nanometre (an axis faces +x or +y), or for two rounds `outside` where their
///   centres are at least the larger radius apart, else `inside` (FreeCAD's rule); one, or
///   the one picked, a `tangent_at` there, `same` where `a` arriving and `b` leaving have a
///   dot product of at least 0, or a `tangent_on` of the curve that ends there, `same` by
///   trim's rule, which replaces each `on` that holds that end (C5): beside a `midpoint` that
///   holds it, which stays, and C6's repeat where a `tangent_on` holds it as drawn.
/// - **Equal:** two or more lines, circles, or arcs, each with the first picked (lines with
///   rounds are the gate's). **Parallel:** two or more lines, likewise. **Perpendicular:**
///   two lines.
/// - **Midpoint:** a point and a line, in either order, replacing an `on` of that point on
///   that line.
/// - **Symmetric:** two points and a mirror, exactly three picks: the one line or axis
///   picked, else, of three points, the last; `a` and `b` the other two in pick order.
/// - **Fix:** for each point, a driving `distance_x` and `distance_y` from the origin, as
///   Smart Dimension writes them for the picks (origin, point), at the read's coordinates
///   rounded to the nanometre; refused where one is past what a length holds, as Smart
///   Dimension refuses it ([`Message::TooFar`]).
pub fn relate(
    read: &Sketch,
    revision: Revision,
    relation: RelationTool,
    picks: &[Reference],
    ids: &mut dyn NewIds,
) -> Result<Related, Error> {
    let _span =
        tracing::debug_span!("relate", relation = relation.name(), picks = picks.len()).entered();
    if picks.is_empty() {
        return Err(Error::new(ErrorCode::InvalidInput, NOTHING_SELECTED));
    }
    // A member picked twice counts once, where it was first picked.
    let mut seen = BTreeSet::new();
    let picks: Vec<Reference> = picks.iter().filter(|r| seen.insert(*r)).cloned().collect();
    let kinds = kinds(read);
    let view = View::of(read, &Members::default());
    let picked = Picked {
        read,
        kinds: &kinds,
        picks: &picks,
    };
    let plan = match relation {
        RelationTool::Horizontal | RelationTool::Vertical => picked.aligned(relation)?,
        RelationTool::Coincident => picked.coincident()?,
        RelationTool::Tangent => picked.tangent(&view)?,
        RelationTool::Equal => {
            picked.with_first(relation, &[Kind::Line, Kind::Round], "equal", |a, b| {
                Form::Equal { a, b }
            })?
        }
        RelationTool::Parallel => {
            picked.with_first(relation, &[Kind::Line], "parallel", |a, b| Form::Parallel {
                a,
                b,
            })?
        }
        RelationTool::Perpendicular => picked.perpendicular()?,
        RelationTool::Midpoint => picked.midpoint()?,
        RelationTool::Symmetric => picked.symmetric()?,
        RelationTool::Fix => picked.fix(&view)?,
    };
    let (forms, replaces, said_if_there) = match plan {
        Plan::Merge(points) => {
            return Ok(Related::Edit {
                revision,
                edit: RelateEdit::Merge(points),
                repeated: Vec::new(),
                message: None,
            });
        }
        Plan::Constraints {
            forms,
            replaces,
            said_if_there,
        } => (forms, replaces, said_if_there),
    };
    // C6: each the sketch holds already left out, found by one lookup.
    let index = Repeats::of(read);
    let mut new = Vec::new();
    let mut repeated = Vec::new();
    for form in forms {
        match index.find(&form) {
            Some(id) => repeated.push(id.clone()),
            None => new.push(form),
        }
    }
    if new.is_empty() {
        return Ok(if replaces.is_empty() {
            Related::AllThere {
                revision,
                repeated,
                message: Message::AllThere(said_if_there),
            }
        } else {
            Related::Edit {
                revision,
                message: Some(Message::RedundantRemoved {
                    said: said_if_there,
                    removed: replaces.clone(),
                }),
                edit: RelateEdit::Remove(replaces),
                repeated,
            }
        });
    }
    picked.within_the_solver(new.len())?;
    let mut builder = Builder::new(ids, &view, false);
    for form in new {
        builder.constraint(form)?;
    }
    let add = builder.members;
    let edit = if replaces.is_empty() {
        RelateEdit::Add(add)
    } else {
        RelateEdit::AddThenRemove {
            add,
            remove: replaces,
        }
    };
    let message = (!repeated.is_empty()).then_some(Message::SomeThere(repeated.len()));
    Ok(Related::Edit {
        revision,
        edit,
        repeated,
        message,
    })
}

/// The sketch's constraints, by their type and their references in order of [`Reference`]
/// (so `a` and `b` either way round find the same entry), each entry's in the sketch's
/// order: built once a click, so each constraint the click would write is one lookup (C6).
struct Repeats<'s>(BTreeMap<(&'static str, Vec<&'s Reference>), Vec<&'s SketchConstraint>>);

impl<'s> Repeats<'s> {
    fn of(read: &'s Sketch) -> Self {
        let mut index: BTreeMap<_, Vec<&SketchConstraint>> = BTreeMap::new();
        for c in &read.constraints {
            index.entry(key(&c.form)).or_default().push(c);
        }
        Repeats(index)
    }

    /// The first of the sketch's constraints that `form` repeats, if any.
    fn find(&self, form: &Form) -> Option<&'s ElementId> {
        self.0.get(&key(form))?.iter().find_map(|c| {
            #[cfg(test)]
            work::COMPARED.with(|n| n.set(n.get() + 1));
            repeats(form, &c.form).then_some(&c.id)
        })
    }
}

/// A form's entry in [`Repeats`]: its type, and its references sorted.
fn key(form: &Form) -> (&'static str, Vec<&Reference>) {
    let mut references: Vec<&Reference> =
        form.references().into_iter().map(|(_, r, _)| r).collect();
    references.sort();
    (form.type_name(), references)
}

/// Whether `form` repeats `other` in everything but an ID (C6): equal, or, in the forms whose `a`
/// and `b` are interchangeable, equal with them swapped: `horizontal_points`,
/// `vertical_points`, `parallel`, `perpendicular`, `equal`, `symmetric` (its `about` the
/// same), `tangent` (its `side` the same: the line's, or symmetric in two rounds), and
/// `tangent_at` (its `at` and `direction` the same: a smooth path is smooth both ways)
/// (C6 as amended, #478).
fn repeats(form: &Form, other: &Form) -> bool {
    form == other
        || match (form, other) {
            (Form::HorizontalPoints { a, b }, Form::HorizontalPoints { a: c, b: d })
            | (Form::VerticalPoints { a, b }, Form::VerticalPoints { a: c, b: d })
            | (Form::Parallel { a, b }, Form::Parallel { a: c, b: d })
            | (Form::Perpendicular { a, b }, Form::Perpendicular { a: c, b: d })
            | (Form::Equal { a, b }, Form::Equal { a: c, b: d }) => a == d && b == c,
            (
                Form::Symmetric { a, b, about },
                Form::Symmetric {
                    a: c,
                    b: d,
                    about: mirror,
                },
            ) => about == mirror && a == d && b == c,
            (
                Form::Tangent { a, b, side },
                Form::Tangent {
                    a: c,
                    b: d,
                    side: other_side,
                },
            ) => side == other_side && a == d && b == c,
            (
                Form::TangentAt {
                    a,
                    b,
                    at,
                    direction,
                },
                Form::TangentAt {
                    a: c,
                    b: d,
                    at: other_at,
                    direction: other_direction,
                },
            ) => at == other_at && direction == other_direction && a == d && b == c,
            _ => false,
        }
}

/// Each member's kind by its ID, the first holder of an ID the one a reference finds, as the
/// gate's.
fn kinds(read: &Sketch) -> BTreeMap<&ElementId, ElementKind> {
    let mut kinds = BTreeMap::new();
    for (id, kind) in read.elements() {
        kinds.entry(id).or_insert(kind);
    }
    kinds
}

/// A selection, read against the sketch.
struct Picked<'p> {
    read: &'p Sketch,
    kinds: &'p BTreeMap<&'p ElementId, ElementKind>,
    picks: &'p [Reference],
}

impl<'p> Picked<'p> {
    fn kind(&self, reference: &Reference) -> Kind {
        match reference {
            Reference::Origin => Kind::Point,
            Reference::XAxis | Reference::YAxis => Kind::Line,
            Reference::Element(id) => match self.kinds.get(id) {
                Some(ElementKind::Point) => Kind::Point,
                Some(ElementKind::Line) => Kind::Line,
                Some(ElementKind::Circle | ElementKind::Arc) => Kind::Round,
                Some(ElementKind::Constraint) | None => Kind::Other,
            },
        }
    }

    /// Whether every member picked is of one of `kinds`.
    fn all(&self, kinds: &[Kind]) -> bool {
        self.picks.iter().all(|r| kinds.contains(&self.kind(r)))
    }

    /// The members picked of `kind`, in pick order.
    fn of(&self, kind: Kind) -> Vec<&'p Reference> {
        self.picks.iter().filter(|r| self.kind(r) == kind).collect()
    }

    /// `relation`'s refusal of a selection it can't read.
    fn refused(&self, relation: RelationTool) -> Error {
        self.refusal(relation.takes())
    }

    /// A refusal saying `message`, the selection as its elements (the first
    /// [`PROBLEMS_LISTED`], all counted).
    fn refusal(&self, message: impl Into<String>) -> Error {
        Error::new(ErrorCode::InvalidInput, message)
            .with_elements(
                self.picks
                    .iter()
                    .take(PROBLEMS_LISTED)
                    .map(|r| r.as_str().to_owned()),
            )
            .with_elements_total(u64::try_from(self.picks.len()).unwrap_or(u64::MAX))
    }

    /// Refused, in the gate's words, where `adding` constraints would give the sketch more
    /// than the sketch solver takes (`MAX_SKETCH_CONSTRAINTS`, counting each arc's rules as
    /// one, as the gate counts them).
    fn within_the_solver(&self, adding: usize) -> Result<(), Error> {
        let n = counted_constraints(self.read).saturating_add(adding);
        if n > MAX_SKETCH_CONSTRAINTS {
            return Err(self
                .refusal(past_the_solver(
                    n,
                    MAX_SKETCH_CONSTRAINTS,
                    CONSTRAINTS_COUNTED,
                ))
                .with_suggestion(SPLIT_THE_SKETCH));
        }
        Ok(())
    }

    /// A reference as messages name it: "point pe000001", "the origin", "x_axis".
    fn noun(&self, reference: &Reference) -> String {
        match reference {
            Reference::Origin => "the origin".to_owned(),
            Reference::XAxis | Reference::YAxis => reference.as_str().to_owned(),
            Reference::Element(id) => match self.kinds.get(id) {
                Some(kind) => format!("{} {id}", kind.noun()),
                None => id.to_string(),
            },
        }
    }

    /// References as messages name them together: "lines ne000001 and ne000003" where all are
    /// members of one kind, else each by its own noun, "point pe000007 and the origin"; the
    /// first [`NAMED`] named, the rest counted ("lines ne000001, ne000002, ne000003, and 47
    /// more").
    fn named(&self, references: &[&Reference]) -> String {
        let kind = |r: &Reference| r.element().and_then(|id| self.kinds.get(id).copied());
        let first = &references[..references.len().min(NAMED)];
        match references.first().and_then(|r| kind(r)) {
            Some(one)
                if references.len() > 1 && references.iter().all(|r| kind(r) == Some(one)) =>
            {
                let ids: Vec<String> = first.iter().map(|r| r.as_str().to_owned()).collect();
                format!("{} {}", one.list(), listed_first(&ids, references.len()))
            }
            _ => {
                let nouns: Vec<String> = first.iter().map(|r| self.noun(r)).collect();
                listed_first(&nouns, references.len())
            }
        }
    }

    /// Horizontal or Vertical: lines, or two or more points, a star from the first.
    fn aligned(&self, relation: RelationTool) -> Result<Plan, Error> {
        let vertical = relation == RelationTool::Vertical;
        let forms: Vec<Form> = if self.all(&[Kind::Line]) {
            self.picks
                .iter()
                .map(|line| {
                    let line = line.clone();
                    made(if vertical {
                        Form::Vertical { line }
                    } else {
                        Form::Horizontal { line }
                    })
                })
                .collect()
        } else if self.all(&[Kind::Point]) && self.picks.len() >= 2 {
            star(self.picks, |a, b| {
                if vertical {
                    Form::VerticalPoints { a, b }
                } else {
                    Form::HorizontalPoints { a, b }
                }
            })
        } else {
            return Err(self.refused(relation));
        };
        let all: Vec<&Reference> = self.picks.iter().collect();
        let word = if vertical { "vertical" } else { "horizontal" };
        Ok(Plan::Constraints {
            forms,
            replaces: Vec::new(),
            said_if_there: format!("{} {} already {word}", self.named(&all), be(all.len())),
        })
    }

    /// Coincident: points and curves, an `on` of each pair; two or more points, a merge; a
    /// point and the origin, #278's A pair (C1). The pairs are counted before they're
    /// written: past what the sketch solver takes, with those the sketch holds already
    /// left out, the click is refused unwritten.
    fn coincident(&self) -> Result<Plan, Error> {
        let refused = || self.refused(RelationTool::Coincident);
        if !self.all(&[Kind::Point, Kind::Line, Kind::Round]) {
            return Err(refused());
        }
        let points = self.of(Kind::Point);
        let curves: Vec<&Reference> = self
            .picks
            .iter()
            .filter(|r| matches!(self.kind(r), Kind::Line | Kind::Round))
            .collect();
        if !points.is_empty() && !curves.is_empty() {
            let (point_set, curve_set): (BTreeSet<_>, BTreeSet<_>) = (
                points.iter().copied().collect(),
                curves.iter().copied().collect(),
            );
            let there: BTreeSet<(&Reference, &Reference)> = self
                .read
                .constraints
                .iter()
                .filter_map(|c| match &c.form {
                    Form::On { point, curve }
                        if point_set.contains(point) && curve_set.contains(curve) =>
                    {
                        Some((point, curve))
                    }
                    _ => None,
                })
                .collect();
            self.within_the_solver(
                points
                    .len()
                    .saturating_mul(curves.len())
                    .saturating_sub(there.len()),
            )?;
            let forms = points
                .iter()
                .flat_map(|point| {
                    curves.iter().map(|curve| {
                        made(Form::On {
                            point: (*point).clone(),
                            curve: (*curve).clone(),
                        })
                    })
                })
                .collect();
            return Ok(Plan::Constraints {
                forms,
                replaces: Vec::new(),
                said_if_there: format!(
                    "{} {} already on {}",
                    self.named(&points),
                    be(points.len()),
                    self.named(&curves)
                ),
            });
        }
        if !curves.is_empty() || points.len() < 2 {
            return Err(refused());
        }
        let members: Vec<&ElementId> = points.iter().filter_map(|p| p.element()).collect();
        match (points.len(), members.as_slice()) {
            (n, _) if members.len() == n => Ok(Plan::Merge(
                members.into_iter().cloned().collect::<Vec<_>>(),
            )),
            (2, [point]) => {
                let point = Reference::Element((*point).clone());
                Ok(Plan::Constraints {
                    said_if_there: format!("{} is already at the origin", self.noun(&point)),
                    forms: at_origin(point).map(made).into(),
                    replaces: Vec::new(),
                })
            }
            _ => Err(refused()),
        }
    }

    /// Equal or Parallel: two or more members, each of `kinds`, a star from the first (C8).
    fn with_first(
        &self,
        relation: RelationTool,
        kinds: &[Kind],
        word: &str,
        form: impl Fn(Reference, Reference) -> Form,
    ) -> Result<Plan, Error> {
        if self.picks.len() < 2 || !self.all(kinds) {
            return Err(self.refused(relation));
        }
        let all: Vec<&Reference> = self.picks.iter().collect();
        Ok(Plan::Constraints {
            forms: star(self.picks, form),
            replaces: Vec::new(),
            said_if_there: format!("{} are already {word}", self.named(&all)),
        })
    }

    /// Perpendicular: two lines.
    fn perpendicular(&self) -> Result<Plan, Error> {
        let [a, b] = self.picks else {
            return Err(self.refused(RelationTool::Perpendicular));
        };
        if !self.all(&[Kind::Line]) {
            return Err(self.refused(RelationTool::Perpendicular));
        }
        Ok(Plan::Constraints {
            forms: vec![made(Form::Perpendicular {
                a: a.clone(),
                b: b.clone(),
            })],
            replaces: Vec::new(),
            said_if_there: format!("{} are already perpendicular", self.named(&[a, b])),
        })
    }

    /// Midpoint: a point and a line, either way round; an `on` of the point on the line
    /// replaced (C5).
    fn midpoint(&self) -> Result<Plan, Error> {
        let (points, lines) = (self.of(Kind::Point), self.of(Kind::Line));
        let ([point], [line], 2) = (points.as_slice(), lines.as_slice(), self.picks.len()) else {
            return Err(self.refused(RelationTool::Midpoint));
        };
        Ok(Plan::Constraints {
            forms: vec![made(Form::Midpoint {
                point: (*point).clone(),
                line: (*line).clone(),
            })],
            replaces: holds(self.read, point, line).ons,
            said_if_there: format!(
                "{} is already the midpoint of {}",
                self.noun(point),
                self.noun(line)
            ),
        })
    }

    /// Symmetric: two points and a mirror (C4), exactly three picks: the one line or axis,
    /// else, of three points, the last.
    fn symmetric(&self) -> Result<Plan, Error> {
        let refused = || self.refused(RelationTool::Symmetric);
        if self.picks.len() != 3 {
            return Err(refused());
        }
        let (points, lines) = (self.of(Kind::Point), self.of(Kind::Line));
        let (a, b, about) = match (points.as_slice(), lines.as_slice()) {
            ([a, b], [line]) => (*a, *b, *line),
            ([a, b, last], []) => (*a, *b, *last),
            _ => return Err(refused()),
        };
        Ok(Plan::Constraints {
            forms: vec![made(Form::Symmetric {
                a: a.clone(),
                b: b.clone(),
                about: about.clone(),
            })],
            replaces: Vec::new(),
            said_if_there: format!(
                "{} and {} are already symmetric about {}",
                self.noun(a),
                self.noun(b),
                self.noun(about)
            ),
        })
    }

    /// Fix: each point's driving `distance_x` and `distance_y` from the origin, as Smart
    /// Dimension writes them for the picks (origin, point), where the read has it (C2).
    fn fix(&self, view: &View<'_>) -> Result<Plan, Error> {
        if !self.all(&[Kind::Point]) {
            return Err(self.refused(RelationTool::Fix));
        }
        let origin = (&Reference::Origin, [0.0, 0.0]);
        let mut forms = Vec::with_capacity(2 * self.picks.len());
        for point in self.picks {
            let Some(at) = place(view, point) else {
                return Err(self.refused(RelationTool::Fix));
            };
            for axis in [DistanceAxis::X, DistanceAxis::Y] {
                let form = distance_along(axis, origin, (point, at))
                    .map_err(|message| self.refusal(message.text()))?;
                forms.push(made(form));
            }
        }
        let all: Vec<&Reference> = self.picks.iter().collect();
        Ok(Plan::Constraints {
            forms,
            replaces: Vec::new(),
            said_if_there: format!("{} {} already fixed", self.named(&all), be(all.len())),
        })
    }

    /// Tangent (C3): two curves, at least one a circle or an arc, and a point if it says
    /// which place; the form the places they can be tangent at give.
    fn tangent(&self, view: &View<'_>) -> Result<Plan, Error> {
        let refused = || self.refused(RelationTool::Tangent);
        if !self.all(&[Kind::Point, Kind::Line, Kind::Round]) {
            return Err(refused());
        }
        let points = self.of(Kind::Point);
        let curves: Vec<&Reference> = self
            .picks
            .iter()
            .filter(|r| matches!(self.kind(r), Kind::Line | Kind::Round))
            .collect();
        let ([a, b], 0 | 1) = (curves.as_slice(), points.len()) else {
            return Err(refused());
        };
        let (a, b) = (*a, *b);
        if self.kind(a) != Kind::Round && self.kind(b) != Kind::Round {
            return Err(self.refusal(TWO_LINES_TANGENT));
        }
        let places = self.places(a, b);
        let at: Vec<String> = places.keys().map(|id| id.to_string()).collect();
        let chosen = match points.first() {
            Some(point) => match point.element().and_then(|id| places.get_key_value(id)) {
                Some(place) => Some(place),
                None => {
                    let not_there = format!(
                        "{} isn't where {} and {} meet: deselect it",
                        self.noun(point),
                        self.noun(a),
                        self.noun(b)
                    );
                    return Err(if at.is_empty() {
                        self.refusal(not_there)
                    } else {
                        self.refusal(format!("{not_there}, or select {}", either(&at)))
                            .with_elements(at)
                    });
                }
            },
            None => match places.len() {
                0 => None,
                1 => places.iter().next(),
                _ => {
                    return Err(self
                        .refusal(format!(
                            "{} and {} could be tangent at more than one point, {}: select the one they're tangent at too",
                            self.noun(a),
                            self.noun(b),
                            listed(&at)
                        ))
                        .with_elements(at));
                }
            },
        };
        let said_if_there = format!("{} and {} are already tangent", self.noun(a), self.noun(b));
        let (form, replaces) = match chosen {
            None => (self.edge_tangent(view, a, b)?, Vec::new()),
            Some((at, Place::Shared)) => {
                let at = Reference::Element((*at).clone());
                let direction = match (
                    sketch_edit::leaving(self.read, a, &at),
                    sketch_edit::leaving(self.read, b, &at),
                ) {
                    // `a` arriving at `at` is the reverse of its leaving it.
                    (Some(out_a), Some(out_b)) if dot([-out_a[0], -out_a[1]], out_b) < 0.0 => {
                        Direction::Opposite
                    }
                    _ => Direction::Same,
                };
                (
                    Form::TangentAt {
                        a: a.clone(),
                        b: b.clone(),
                        at,
                        direction,
                    },
                    Vec::new(),
                )
            }
            Some((
                at,
                Place::Held {
                    ending,
                    held_on,
                    ons,
                },
            )) => {
                let at = Reference::Element((*at).clone());
                (
                    Form::TangentOn {
                        direction: sketch_edit::drawn(self.read, self.read, ending, &at, held_on),
                        a: (*ending).clone(),
                        at,
                        b: (*held_on).clone(),
                    },
                    ons.clone(),
                )
            }
        };
        Ok(Plan::Constraints {
            forms: vec![made(form)],
            replaces,
            said_if_there,
        })
    }

    /// The places `a` and `b` can be tangent at, by point (C3): each end they share, then
    /// each end of one that a constraint holds on the other ([`Form::hold`]).
    fn places(&self, a: &'p Reference, b: &'p Reference) -> BTreeMap<&'p ElementId, Place<'p>> {
        let ends_of = |curve: &Reference| {
            curve
                .element()
                .map(|id| end_ids(self.read, id))
                .unwrap_or_default()
        };
        let (ends_a, ends_b) = (ends_of(a), ends_of(b));
        let mut places = BTreeMap::new();
        for end in &ends_a {
            if ends_b.contains(end) {
                places.entry(*end).or_insert(Place::Shared);
            }
        }
        for (ending, held_on, ending_ends) in [(a, b, &ends_a), (b, a, &ends_b)] {
            for end in ending_ends {
                let holding = holds(self.read, &Reference::Element((*end).clone()), held_on);
                if holding.any {
                    places.entry(*end).or_insert(Place::Held {
                        ending,
                        held_on,
                        ons: holding.ons,
                    });
                }
            }
        }
        places
    }

    /// An edge `tangent` of `a` and `b`, its `side` read from the drawing (C3): for a line
    /// and a round, the side of the line the round's centre is on, as Smart Dimension
    /// measures a point off a line, refused where it's on it at the nanometre.
    fn edge_tangent(&self, view: &View<'_>, a: &Reference, b: &Reference) -> Result<Form, Error> {
        let refused = || self.refused(RelationTool::Tangent);
        let (Some(geo_a), Some(geo_b)) = (view.geo(a), view.geo(b)) else {
            return Err(refused());
        };
        let side = match (&geo_a, &geo_b) {
            (Geo::Line { from, to }, round) | (round, Geo::Line { from, to }) => {
                let (line, round_ref) = if matches!(geo_a, Geo::Line { .. }) {
                    (a, b)
                } else {
                    (b, a)
                };
                let Some((centre, _)) = round.round() else {
                    return Err(refused());
                };
                let direction = [to[0] - from[0], to[1] - from[1]];
                match off_line(*from, direction, centre) {
                    Ok(off) => match off.side {
                        LineSide::Left => TangentSide::Left,
                        LineSide::Right => TangentSide::Right,
                    },
                    Err(NotOff::OnIt) => {
                        return Err(self.refusal(format!(
                            "the centre of {} is on {}: which side it's tangent on is unclear; move one off the other first",
                            self.noun(round_ref),
                            self.noun(line)
                        )));
                    }
                    Err(NotOff::NoDirection) => {
                        return Err(self.refusal(format!(
                            "{} has no length, so it has no side for {} to be tangent on",
                            self.noun(line),
                            self.noun(round_ref)
                        )));
                    }
                    Err(NotOff::TooFar) => return Err(self.refusal(Message::TooFar.text())),
                }
            }
            (round_a, round_b) => {
                let (Some((ca, ra)), Some((cb, rb))) = (round_a.round(), round_b.round()) else {
                    return Err(refused());
                };
                if super::geometry::distance(ca, cb) >= ra.max(rb) {
                    TangentSide::Outside
                } else {
                    TangentSide::Inside
                }
            }
        };
        Ok(Form::Tangent {
            a: a.clone(),
            b: b.clone(),
            side,
        })
    }
}

/// `form`, as a plan makes it: counted in tests ([`work::BUILT`]) where it's made, so a click
/// refused before its forms are made is seen to make none, however they'd be gathered.
fn made(form: Form) -> Form {
    #[cfg(test)]
    work::BUILT.with(|n| n.set(n.get() + 1));
    form
}

/// Each member of `picks` after the first, paired with the first (C8), `form(first, other)`.
fn star(picks: &[Reference], form: impl Fn(Reference, Reference) -> Form) -> Vec<Form> {
    let Some((first, rest)) = picks.split_first() else {
        return Vec::new();
    };
    rest.iter()
        .map(|other| made(form(first.clone(), other.clone())))
        .collect()
}

/// The sketch's holds of `point` on `curve` ([`Form::hold`]): whether there's any, and the
/// `on`s among them, by ID, in the sketch's order.
struct Holds {
    any: bool,
    ons: Vec<ElementId>,
}

fn holds(read: &Sketch, point: &Reference, curve: &Reference) -> Holds {
    let mut found = Holds {
        any: false,
        ons: Vec::new(),
    };
    for c in &read.constraints {
        if c.form.hold() == Some((point, curve)) {
            found.any = true;
            if matches!(c.form, Form::On { .. }) {
                found.ons.push(c.id.clone());
            }
        }
    }
    found
}

/// Where a point picked is: the origin's place, or the read's coordinates.
fn place(view: &View<'_>, point: &Reference) -> Option<Xy> {
    match point {
        Reference::Origin => Some([0.0, 0.0]),
        Reference::Element(id) => view.point(id),
        Reference::XAxis | Reference::YAxis => None,
    }
}

/// "is" for one, "are" for more.
fn be(n: usize) -> &'static str {
    if n == 1 { "is" } else { "are" }
}

/// How much work a click did, counted in tests: each constraint built for a plan, and each
/// of the sketch's constraints one was compared with for C6.
#[cfg(test)]
pub(super) mod work {
    use std::cell::Cell;

    thread_local! {
        pub(in crate::sketch_tools) static BUILT: Cell<usize> = const { Cell::new(0) };
        pub(in crate::sketch_tools) static COMPARED: Cell<usize> = const { Cell::new(0) };
    }
}
